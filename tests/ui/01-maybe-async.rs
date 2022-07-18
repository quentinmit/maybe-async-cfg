#![allow(dead_code)]

#[maybe_async_cfg::maybe(send="Send", sync(feature = "is_sync"), async(not(feature = "is_sync"), async_trait::async_trait))]
trait Trait {
    fn sync_fn() {}

    async fn declare_async(&self);

    async fn async_fn(&self) {
        self.declare_async().await
    }
}

#[maybe_async_cfg::maybe(send="?Send", sync(feature = "is_sync"), async(not(feature = "is_sync"), async_trait::async_trait))]
pub trait PubTrait {
    fn sync_fn() {}

    async fn declare_async(&self);

    async fn async_fn(&self) {
        self.declare_async().await
    }
}

#[maybe_async_cfg::maybe(sync(feature = "is_sync", keep_self), async(not(feature = "is_sync"), self = "async"))]
mod blocking {
    pub async fn async_fn() {}
}

#[maybe_async_cfg::maybe(keep_self, sync(feature = "is_sync"), async(not(feature = "is_sync")))]
async fn async_fn() {}

#[maybe_async_cfg::maybe(keep_self, sync(feature = "is_sync"), async(not(feature = "is_sync")))]
pub async fn pub_async_fn() {
}

struct Struct;

#[maybe_async_cfg::maybe(keep_self, idents(Trait), sync(feature = "is_sync"), async(not(feature = "is_sync"), async_trait::async_trait))]
impl Trait for Struct {
    fn sync_fn() {}

    async fn declare_async(&self) {}

    async fn async_fn(&self) {
        async { self.declare_async().await }.await
    }
}

#[cfg(feature = "is_sync")]
fn main() -> std::result::Result<(), ()> {
    let s = Struct;
    s.declare_async();
    s.async_fn();
    async_fn();
    pub_async_fn();
    r#async::async_fn();
    Ok(())
}


#[cfg(not(feature = "is_sync"))]
#[async_std::main]
async fn main() {
    let s = Struct;
    s.declare_async().await;
    s.async_fn().await;
    async_fn().await;
    pub_async_fn().await;
    r#async::async_fn().await;
}
