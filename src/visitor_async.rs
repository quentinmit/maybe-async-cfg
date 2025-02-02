#[allow(unused_imports)]
use std::{collections::HashMap, iter::FromIterator};

#[allow(unused_imports)]
use proc_macro::TokenStream;
use quote::{quote, ToTokens};
use syn::visit_mut::VisitMut;

use crate::{
    MACRO_NOOP_NAME, MACRO_REMOVE_NAME, MACRO_ONLY_IF_NAME, MACRO_REMOVE_IF_NAME,
    params::{ConvertMode, MacroParameters},
    utils::{AttributeArgsInParens, PunctuatedList},
    visit_ext::{IdentMode, VisitMutExt, Visitor},
};

pub struct AsyncAwaitVisitor<'p> {
    convert_mode: ConvertMode,
    params: &'p mut MacroParameters,
    generics: Vec<HashMap<String, syn::PathSegment>>,
}

impl<'p> AsyncAwaitVisitor<'p> {
    pub fn new(params: &'p mut MacroParameters, convert_mode: ConvertMode) -> Self {
        Self {
            convert_mode,
            params,
            generics: vec![],
        }
    }

    fn generics_get<S: AsRef<str>>(&self, key: S) -> Option<&syn::PathSegment> {
        for gens in &self.generics {
            if let Some(ps) = gens.get(key.as_ref()) {
                return Some(ps);
            }
        }

        None
    }
}

fn search_future_trait_bound(bound: &syn::TypeParamBound) -> Option<syn::PathSegment> {
    if let syn::TypeParamBound::Trait(trait_bound) = bound {
        let segment = &trait_bound.path.segments[trait_bound.path.segments.len() - 1];
        let name = segment.ident.to_string();
        if name.eq("Future") {
            // match Future<Output=Type>
            if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                // binding: Output=Type
                if let syn::GenericArgument::Binding(binding) = &args.args[0] {
                    if let syn::Type::Path(p) = &binding.ty {
                        return Some(p.path.segments[0].clone());
                    }
                }
            }
        }
    };

    None
}

impl<'p> AsyncAwaitVisitor<'p> {
    fn process_replace_features_meta(&self, meta: &mut syn::Meta) -> syn::Result<bool> {
        let mut changed = false;

        match meta {
            syn::Meta::NameValue(syn::MetaNameValue {
                path,
                lit: syn::Lit::Str(s),
                ..
            }) => {
                if let Some(ident) = path.get_ident() {
                    if ident.to_string() == "feature" {
                        let prev = s.value();
                        if let Some(new) = self.params.replace_features_get(&prev) {
                            *s = syn::LitStr::new(new, s.span());
                            changed = true;
                        }
                    }
                }
            }
            syn::Meta::List(list) => {
                for nm in &mut list.nested {
                    if let syn::NestedMeta::Meta(m) = nm {
                        changed |= self.process_replace_features_meta(m)?;
                    }
                }
            }
            _ => {}
        }

        Ok(changed)
    }

    fn process_attribute_if(&mut self, attr: &mut syn::Attribute, not: bool) -> syn::Result<()> {
        let args =
            syn::parse_macro_input::parse::<AttributeArgsInParens>(attr.tokens.clone().into())?;

        let arg = match &args.args.len() {
            0 => {
                return Err(syn::Error::new_spanned(
                    attr.to_token_stream(),
                    "Expected ident",
                ))
            }
            1 => &args.args[0],
            _ => {
                return Err(syn::Error::new_spanned(
                    args.args[1].to_token_stream(),
                    "Too many arguments",
                ))
            }
        };

        let key = match arg {
            syn::NestedMeta::Lit(syn::Lit::Str(s)) => s.value(),
            syn::NestedMeta::Meta(syn::Meta::Path(ref p)) => {
                if let Some(s) = p.get_ident() {
                    s.to_string()
                } else {
                    return Err(syn::Error::new_spanned(
                        arg.to_token_stream(),
                        "Wrong ident",
                    ));
                }
            }
            syn::NestedMeta::Meta(syn::Meta::NameValue(syn::MetaNameValue {
                path,
                lit: syn::Lit::Str(value),
                ..
            })) if path.is_ident("key") => value.value(),
            _ => {
                return Err(syn::Error::new_spanned(
                    arg.to_token_stream(),
                    "Wrong ident",
                ))
            }
        };

        let success = if let Some(current_key) = self.params.key_get() {
            (current_key == &key) ^ not
        } else {
            false
        };

        let new_name = if success { MACRO_NOOP_NAME } else { MACRO_REMOVE_NAME };
        attr.path = self.params.make_self_path(new_name);

        Ok(())
    }

    fn process_attrs(&mut self, attrs: &mut Vec<syn::Attribute>) -> syn::Result<()> {
        for attr in attrs.iter_mut() {
            if let Some(name) = self.params.is_our_attr(attr) {
                match name.as_str() {
                    MACRO_ONLY_IF_NAME => self.process_attribute_if(attr, false)?,
                    MACRO_REMOVE_IF_NAME => self.process_attribute_if(attr, true)?,
                    _ => {
                        // Attribute stays unchanged. Unknown attributes will be
                        // rejected by compiter later.
                    }
                }
            }
        }

        if !self.params.drop_attrs_is_empty() {
            attrs.retain(|attr| {
                if let Some(ident) = attr.path.get_ident() {
                    let ident = ident.to_string();
                    !self.params.drop_attrs_contains(&ident)
                } else {
                    true
                }
            });
        }

        if !self.params.replace_features_is_empty() {
            for attr in attrs {
                if let Some(ident) = attr.path.get_ident() {
                    if ident.to_string() == "cfg" {
                        if let Ok(mut meta) = attr.parse_meta() {
                            if self.process_replace_features_meta(&mut meta)? {
                                if let syn::Meta::List(syn::MetaList { nested, .. }) = meta {
                                    attr.tokens = quote!((#nested));
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn process_expr(&mut self, node: &mut syn::Expr) -> syn::Result<()> {
        match self.convert_mode {
            ConvertMode::IntoSync => {
                // async -> sync, remove async_impl blocks
                match node {
                    syn::Expr::Await(expr) => {
                        *node = (*expr.base).clone()
                    }

                    syn::Expr::Async(expr) => {
                        let inner = &expr.block;
                        let sync_expr = if inner.stmts.len() == 1 {
                            // remove useless braces when there is only one statement
                            let stmt = &inner.stmts.get(0).unwrap();
                            // convert statement to Expr
                            syn::parse_quote!(#stmt)
                        } else {
                            syn::Expr::Block(syn::ExprBlock {
                                attrs: expr.attrs.clone(),
                                block: inner.clone(),
                                label: None,
                            })
                        };
                        *node = sync_expr;
                    }

                    _ => {}
                }
            }
            ConvertMode::IntoAsync => {
                // stay async, just remove sync_impl blocks
                match node {
                    _ => {}
                }
            }
        };

        Ok(())
    }

    fn process_item(&mut self, node: &mut syn::Item) -> syn::Result<()> {
        match self.convert_mode {
            ConvertMode::IntoSync => {
                // find generic parameter of Future and replace it with its Output type
                if let syn::Item::Fn(item_fn) = node {
                    let mut gens: HashMap<String, syn::PathSegment> = HashMap::new();

                    // generic params: <T:Future<Output=()>, F>
                    for param in &item_fn.sig.generics.params {
                        // generic param: T:Future<Output=()>
                        if let syn::GenericParam::Type(type_param) = param {
                            let generic_type_name = &type_param.ident;

                            // bound: Future<Output=()>
                            for bound in &type_param.bounds {
                                if let Some(ps) = search_future_trait_bound(bound) {
                                    gens.insert(generic_type_name.to_string(), ps);
                                }
                            }
                        }
                    }

                    if let Some(where_clause) = &item_fn.sig.generics.where_clause {
                        for predicate in &where_clause.predicates {
                            if let syn::WherePredicate::Type(predicate_type) = predicate {
                                let generic_type_name =
                                    if let syn::Type::Path(p) = &predicate_type.bounded_ty {
                                        &p.path.segments[0].ident
                                    } else {
                                        panic!("Please submit an issue");
                                    };

                                for bound in &predicate_type.bounds {
                                    if let Some(ps) = search_future_trait_bound(bound) {
                                        gens.insert(generic_type_name.to_string(), ps);
                                    }
                                }
                            }
                        }
                    }

                    self.generics.push(gens);
                }

                if let syn::Item::Fn(item_fn) = node {
                    // remove generic type from generics <T, F>
                    let args = item_fn
                        .sig
                        .generics
                        .params
                        .iter()
                        .filter_map(|param| {
                            if let syn::GenericParam::Type(type_param) = &param {
                                if let Some(_) = self.generics_get(type_param.ident.to_string()) {
                                    return None;
                                }
                            };
                            Some(param)
                        })
                        .collect::<Vec<_>>();

                    item_fn.sig.generics.params = syn::punctuated::Punctuated::from_iter(
                        args.into_iter().map(|p| p.clone()).collect::<Vec<_>>(),
                    );

                    // remove generic type from where clause
                    if let Some(where_clause) = &mut item_fn.sig.generics.where_clause {
                        let new_where_clause = where_clause
                            .predicates
                            .iter()
                            .filter_map(|predicate| {
                                if let syn::WherePredicate::Type(predicate_type) = predicate {
                                    if let syn::Type::Path(p) = &predicate_type.bounded_ty {
                                        if let Some(_) =
                                            self.generics_get(p.path.segments[0].ident.to_string())
                                        {
                                            return None;
                                        }
                                    }
                                };
                                Some(predicate)
                            })
                            .collect::<Vec<_>>();

                        where_clause.predicates = syn::punctuated::Punctuated::from_iter(
                            new_where_clause
                                .into_iter()
                                .map(|c| c.clone())
                                .collect::<Vec<_>>(),
                        );
                    };
                }
            }
            ConvertMode::IntoAsync => {}
        };

        Ok(())
    }

    fn after_process_item(&mut self, node: &mut syn::Item) -> syn::Result<()> {
        match self.convert_mode {
            ConvertMode::IntoSync => {
                // find generic parameter of Future and replace it with its Output type
                if let syn::Item::Fn(_item_fn) = node {
                    self.generics.pop();
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn process_path_segment(&mut self, node: &mut syn::PathSegment) -> syn::Result<()> {
        let ident = &mut node.ident;
        let ident_s = ident.to_string();

        // replace generic type with target type
        if let Some(ps) = self.generics_get(&ident_s) {
            *node = ps.clone();
            return Ok(());
        }

        Ok(())
    }

    fn process_ident(&mut self, ident: &mut syn::Ident, mode: IdentMode) -> syn::Result<()> {
        if mode == IdentMode::Use {
            return Ok(());
        };

        if let Some(ir) = self.params.idents_get(ident.to_string()) {
            *ident = ir.ident_add_suffix(ident, self.convert_mode, self.params.key_get());
            return Ok(());
        }

        Ok(())
    }

    fn process_type_param(&mut self, node: &mut syn::TypeParam) -> syn::Result<()> {
        let ident = &mut node.ident;

        if let Some(ir) = self.params.idents_get(&ident.to_string()) {
            *ident = ir.ident_add_suffix(ident, self.convert_mode, self.params.key_get());
        }

        Ok(())
    }

    fn process_use_tree(&mut self, node: &mut syn::UseTree) -> syn::Result<()> {
        match node {
            syn::UseTree::Path(syn::UsePath { ident, .. }) => {
                if let Some(ir) = self.params.idents_get(&ident.to_string()) {
                    if !ir.use_mode {
                        *ident = ir.ident_add_suffix(ident, self.convert_mode, self.params.key_get());
                    }
                }
            }
            syn::UseTree::Name(name) => {
                let ident = &mut name.ident;

                if let Some(ir) = self.params.idents_get(&ident.to_string()) {
                    if ir.use_mode {
                        *node = syn::UseTree::Rename(syn::UseRename {
                            ident: ident.clone(),
                            as_token: syn::Token![as](ident.span()),
                            rename: ir.ident_add_suffix(ident, self.convert_mode, self.params.key_get()),
                        });
                    } else {
                        *ident = ir.ident_add_suffix(ident, self.convert_mode, self.params.key_get());
                    }
                }
            }
            _ => {}
        };

        Ok(())
    }
}

impl<'p> VisitMutExt for Visitor<AsyncAwaitVisitor<'p>> {
    fn process_attrs(&mut self, attrs: &mut Vec<syn::Attribute>) -> syn::Result<()> {
        self.inner.process_attrs(attrs)
    }
    fn process_ident(&mut self, ident: &mut syn::Ident, mode: IdentMode) -> syn::Result<()> {
        self.inner.process_ident(ident, mode)
    }
    fn process_expr(&mut self, node: &mut syn::Expr) -> syn::Result<()> {
        self.inner.process_expr(node)
    }
    fn process_item(&mut self, node: &mut syn::Item) -> syn::Result<()> {
        self.inner.process_item(node)
    }
    fn after_process_item(&mut self, node: &mut syn::Item) -> syn::Result<()> {
        self.inner.after_process_item(node)
    }

    fn process_macro(&mut self, node: &mut syn::Macro) -> syn::Result<()> {
        if let Some(ident) = node.path.get_ident() {
            if self
                .inner
                .params
                .standard_macros()
                .contains(&ident.to_string().as_str())
            {
                let mut args = syn::parse2::<PunctuatedList>(node.tokens.clone())?;

                for arg in &mut args.list {
                    self.visit_expr_mut(arg);
                }

                node.tokens = args.list.into_token_stream();
            }
        };
        Ok(())
    }
    fn process_path_segment(&mut self, node: &mut syn::PathSegment) -> syn::Result<()> {
        self.inner.process_path_segment(node)
    }
    fn process_type_param(&mut self, node: &mut syn::TypeParam) -> syn::Result<()> {
        self.inner.process_type_param(node)
    }
    fn process_use_tree(&mut self, node: &mut syn::UseTree) -> syn::Result<()> {
        self.inner.process_use_tree(node)
    }
}

impl<'p> AsyncAwaitVisitor<'p> {}
