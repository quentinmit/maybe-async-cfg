use proc_macro_error::abort;

#[allow(unused_imports)]
use std::iter::FromIterator;

use proc_macro::TokenStream;

use proc_macro2::TokenStream as TokenStream2;
use syn::{
    parse_macro_input, spanned::Spanned, visit_mut::VisitMut, File, ImplItem, ItemEnum, ItemFn,
    ItemImpl, ItemStruct, ItemTrait, ItemUse, ItemMod, TraitItem, Type, TypePath,
};

#[allow(unused_imports)]
use quote::{quote, ToTokens};

use crate::{
    MACRO_MAYBE_NAME,
    params::{ConvertMode, MacroParameters},
    utils::{make_attr_from_str, unwrap_or_error},
    visit_ext::Visitor,
    visitor_async::AsyncAwaitVisitor,
    visitor_content::ContentVisitor,
    debug::*
};

////////////////////////////////////////////////////////////////////////////////////////////////////

pub fn maybe(args: TokenStream, input: TokenStream) -> TokenStream {
    dump_maybe!(&args, &input);

    let params = unwrap_or_error!(MacroParameters::from_tokens(args));
    dump_params!("maybe params", &params);

    if params.disable_get() {
        return input;
    }

    if let Some(convert_mode) = params.mode_get() {
        return convert(params, input, convert_mode)
    }


    let mut tokens = TokenStream::new();

    for version in &params.versions {
        let mut ts = TokenStream2::new();

        match version.kind {
            ConvertMode::IntoAsync | ConvertMode::IntoSync => {
                let _ = unwrap_or_error!(version
                    .params
                    .extend_tokenstream2_with_cfg_outer_attrs(&mut ts));
                let name = params.make_self_path(MACRO_MAYBE_NAME);
                let args = version.params.to_tokens(Some(version.kind));
                ts.extend(quote!(#[#name(#args)]));

                let _ =
                    unwrap_or_error!(version.params.extend_tokenstream2_with_inner_attrs(&mut ts));
            }
        }

        let ts: TokenStream = ts.into();
        tokens.extend(ts);
        tokens.extend(input.clone());
    }

    dump_tokens!("maybe after", &tokens);

    tokens
}

////////////////////////////////////////////////////////////////////////////////////////////////////

pub fn convert(mut params: MacroParameters, input: TokenStream, convert_mode: ConvertMode) -> TokenStream {
    dump_tokens!("convert before", &input);

    let mut file = parse_macro_input!(input as File);
    for item in &mut file.items {
        match item {
            syn::Item::Impl(item) => convert_impl(&mut params, item, convert_mode),
            syn::Item::Struct(item) => convert_struct(&mut params, item, convert_mode),
            syn::Item::Enum(item) => convert_enum(&mut params, item, convert_mode),
            syn::Item::Trait(item) => convert_trait(&mut params, item, convert_mode),
            syn::Item::Fn(item) => convert_fn(&mut params, item, convert_mode),
            syn::Item::Use(item) => convert_use(&mut params, item, convert_mode),
            syn::Item::Mod(item) => convert_mod(&mut params, item, convert_mode),
            _ => {
                abort!(item.span(), "Allowed impl, struct, enum, trait, fn or use items only");
            }
        }
    }
    let ts = quote!(#file);

    dump_tokens2!("convert after", &ts);
    ts.into()
}

fn convert_impl(params: &mut MacroParameters, item: &mut ItemImpl, convert_mode: ConvertMode) {
    match &mut *item.self_ty {
        Type::Path(TypePath { path, .. }) => {
            if let Some(last) = path.segments.last_mut() {
                params.original_self_name_set(last.ident.to_string(), false);
            }
        }
        _ => {}
    };

    let send = params.send_get();

    let mut visitor = Visitor::new(AsyncAwaitVisitor::new(params, convert_mode));

    match convert_mode {
        ConvertMode::IntoSync => {
            for inner in &mut item.items {
                if let ImplItem::Method(ref mut method) = inner {
                    if method.sig.asyncness.is_some() {
                        method.sig.asyncness = None;
                    }
                }
            }
        }
        ConvertMode::IntoAsync => {
            if let Some(send) = send {
                let attr_str = if send {
                    "async_trait::async_trait"
                } else {
                    "async_trait::async_trait(?Send)"
                };
                let attr = make_attr_from_str(attr_str, item.span()).unwrap();
                item.attrs.push(attr);
            }
        }
    }

    visitor.visit_item_impl_mut(item)
}

fn convert_struct(params: &mut MacroParameters, item: &mut ItemStruct, convert_mode: ConvertMode) {
    params.original_self_name_set(item.ident.to_string(), false);

    let mut visitor = Visitor::new(AsyncAwaitVisitor::new(params, convert_mode));
    visitor.visit_item_struct_mut(item)
}

fn convert_enum(params: &mut MacroParameters, item: &mut ItemEnum, convert_mode: ConvertMode) {
    params.original_self_name_set(item.ident.to_string(), false);

    let mut visitor = Visitor::new(AsyncAwaitVisitor::new(params, convert_mode));
    visitor.visit_item_enum_mut(item)
}

fn convert_trait(params: &mut MacroParameters, item: &mut ItemTrait, convert_mode: ConvertMode) {
    params.original_self_name_set(item.ident.to_string(), false);

    let mut visitor = Visitor::new(AsyncAwaitVisitor::new(params, convert_mode));

    match convert_mode {
        ConvertMode::IntoSync => {
            for inner in &mut item.items {
                if let TraitItem::Method(ref mut method) = inner {
                    if method.sig.asyncness.is_some() {
                        method.sig.asyncness = None;
                    }
                }
            }
        }
        ConvertMode::IntoAsync => {}
    }

    visitor.visit_item_trait_mut(item)
}

fn convert_fn(params: &mut MacroParameters, item: &mut ItemFn, convert_mode: ConvertMode) {
    params.original_self_name_set(item.sig.ident.to_string(), true);

    let mut visitor = Visitor::new(AsyncAwaitVisitor::new(params, convert_mode));

    match convert_mode {
        ConvertMode::IntoSync => {
            if item.sig.asyncness.is_some() {
                item.sig.asyncness = None;
            }
        }
        ConvertMode::IntoAsync => {}
    }

    visitor.visit_item_fn_mut(item)
}

fn convert_use(params: &mut MacroParameters, item: &mut ItemUse, convert_mode: ConvertMode) {
    let mut visitor = Visitor::new(AsyncAwaitVisitor::new(params, convert_mode));
    visitor.visit_item_use_mut(item)
}

fn convert_mod(params: &mut MacroParameters, item: &mut ItemMod, convert_mode: ConvertMode) {
    params.original_self_name_set(item.ident.to_string(), true);
    let mut visitor = Visitor::new(AsyncAwaitVisitor::new(params, convert_mode));
    visitor.visit_item_mod_mut(item)
}

////////////////////////////////////////////////////////////////////////////////////////////////////

pub fn content(body: TokenStream) -> TokenStream {
    dump_tokens!("content before", &body);

    let mut visitor = Visitor::new(ContentVisitor::new());
    let ts: TokenStream = visitor.process(body.into()).into();

    dump_tokens!("content after", &ts);
    ts
}
