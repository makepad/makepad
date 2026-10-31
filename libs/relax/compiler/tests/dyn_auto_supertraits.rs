// matrix-sdk-common AsyncTraitDeps: `dyn CryptoStore` is Sync through two levels of supertraits
// (CryptoStore: AsyncTraitDeps: SyncOutsideWasm: Sync). rustc elaborates an object type's
// supertraits for auto traits (rustc_trait_selection select/candidate_assembly.rs
// assemble_candidates_from_object_ty; next solver: assemble_object_bound_candidates over the
// elaborated predicates), so Arc<dyn CryptoStore> is Send + Sync.
use std::sync::Arc;

pub trait SendOutsideWasm: Send {}
impl<T: Send> SendOutsideWasm for T {}
pub trait SyncOutsideWasm: Sync {}
impl<T: Sync> SyncOutsideWasm for T {}
pub trait AsyncTraitDeps: std::fmt::Debug + SendOutsideWasm + SyncOutsideWasm {}
impl<T: std::fmt::Debug + SendOutsideWasm + SyncOutsideWasm> AsyncTraitDeps for T {}

pub trait CryptoStore: AsyncTraitDeps {
    fn id(&self) -> u32;
}

#[derive(Debug)]
struct Mem(u32);
impl CryptoStore for Mem {
    fn id(&self) -> u32 { self.0 }
}

#[derive(Debug)]
struct Wrapper {
    store: Arc<dyn CryptoStore>,
}

fn is_send_sync<T: Send + Sync + ?Sized>(_: &T) {}

#[test]
fn object_auto_traits_from_supertraits() {
    let w = Wrapper { store: Arc::new(Mem(7)) };
    is_send_sync(&w);
    is_send_sync(&*w.store);
    let w = Arc::new(w);
    let h = std::thread::spawn(move || w.store.id());
    assert_eq!(h.join().unwrap(), 7);
}

#[test]
fn future_holding_the_object_is_send() {
    fn assert_send<T: Send>(_: &T) {}
    async fn tick() {}
    async fn uses(s: Arc<dyn CryptoStore>) -> u32 { tick().await; s.id() }
    let f = uses(Arc::new(Mem(3)));
    assert_send(&f);
    drop(f);
}
