use core::future::Future;
use core::pin::Pin;
use std::sync::Arc;

#[cfg_attr(all(), mac::m)]
fn plain() -> u32 { 3 }

#[cfg_attr(not(target_family = "wasm"), mac::m(arg))]
#[cfg_attr(target_family = "wasm", mac::m(arg))]
fn argument() -> u32 { 4 }

#[cfg_attr(any(), mac::boom)]
fn inactive() -> u32 { 7 }

#[cfg_attr(all(), cfg_attr(all(), mac::m))]
fn nested() -> u32 { 5 }

#[cfg_attr(all(), mac::test)]
fn qualified() -> u32 { 5 }

#[mac::check_attrs(doc, mac::m, inline, doc)]
/// before cfg_attr
#[cfg_attr(all(), mac::m, inline)]
/// after cfg_attr
fn multiple() -> u32 { 6 }

// Attribute order matters: (2 * (3 + 1)) + 1 = 9.
#[mac::m(arg)]
#[cfg_attr(all(), mac::m, mac::m(arg))]
#[cfg_attr(any(), mac::boom)]
fn ordered() -> u32 { 3 }

#[cfg_attr(all(), mac::m, derive(mac::Marked, Clone))]
#[cfg_attr(any(), mac::boom)]
struct Derived;

// Exercise a derive without an attribute macro rewriting the source first.
#[cfg_attr(all(), cfg_attr(all(), derive(mac::Marked)))]
struct Direct;

#[cfg_attr(target_family = "wasm", mac::m(?Send))]
#[cfg_attr(not(target_family = "wasm"), mac::m)]
trait Store: Send + Sync {
    async fn load(&self) -> u32;
    fn value(&self) -> u32;
}

struct Data;
impl Store for Data {
    #[cfg(not(target_family = "wasm"))]
    fn load(&self) -> Pin<Box<dyn Future<Output = u32> + Send + '_>> {
        Box::pin(async { 11 })
    }
    #[cfg(target_family = "wasm")]
    fn load(&self) -> Pin<Box<dyn Future<Output = u32> + '_>> {
        Box::pin(async { 11 })
    }
    fn value(&self) -> u32 { 11 }
}

#[cfg_attr(all(), mac::m(?Send))]
trait LocalStore {
    async fn load(&self) -> u32;
}

impl LocalStore for Data {
    fn load(&self) -> Pin<Box<dyn Future<Output = u32> + '_>> {
        Box::pin(async { 13 })
    }
}

mod imported {
    #[cfg_attr(all(), test)]
    pub fn selected() -> u32 { 7 }

    #[cfg_attr(all(), bench)]
    pub fn benchmark() -> u32 { 8 }

    use mac::{test, bench};

    pub mod child {
        #[cfg_attr(all(), test)]
        fn dropped() { panic!("builtin test must be removed"); }
        pub fn dropped() -> u32 { 19 }
    }
}

fn require_send_sync<T: Send + Sync>(_: &T) {}
fn require_send<T: Send>(_: T) {}

fn main() {
    #[cfg_attr(all(), mac::m)]
    fn local() -> u32 { 9 }
    #[cfg_attr(all(), test)]
    fn imported_local() -> u32 { 10 }
    use mac::test;

    let store: Arc<dyn Store> = Arc::new(Data);
    require_send_sync(&store);
    #[cfg(not(target_family = "wasm"))]
    require_send(store.load());
    let local_store: &dyn LocalStore = &Data;
    drop(local_store.load());
    let _copy = Derived.clone();
    assert_eq!((plain(), argument(), inactive(), nested(), multiple(), ordered()), (6, 5, 7, 10, 12, 9));
    assert_eq!((Derived::derived(), Direct::derived(), store.value()), (17, 17, 11));
    assert_eq!((imported::selected(), imported::benchmark(), imported::child::dropped()), (14, 16, 19));
    assert_eq!((local(), imported_local()), (18, 20));
    assert_eq!(qualified(), 10);
    println!("cfg attr macros ok");
}
