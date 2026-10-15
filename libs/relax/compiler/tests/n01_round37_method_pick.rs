// rustc_hir_typeck/src/method/probe.rs:1331-1450,1772-1843,2371-2406.
trait ByRef { fn pick(&self) -> u32; }
trait ByValue { fn pick(self) -> u32; }
impl ByRef for u32 { fn pick(&self) -> u32 { 1 } }
impl ByValue for &u32 { fn pick(self) -> u32 { 2 } }
trait Get<T> { fn get(&self) -> T; }
impl Get<u32> for () { fn get(&self) -> u32 { 3 } }
impl Get<u8> for () { fn get(&self) -> u8 { 4 } }
fn same_trait<T: Get<u32> + Get<u8>>(x: T) -> u32 { x.get() }
trait Parent { fn parent(&self) -> u32 { 5 } }
trait Child: Parent {}
impl Parent for () {}
impl Child for () {}
fn inherited<T: Child>(x: T) -> u32 { x.parent() }
trait Associated { type Item: Parent; fn item(&self) -> Self::Item; }
impl Associated for () { type Item = (); fn item(&self) {} }
fn projected<T: Associated>(x: T) -> u32 { x.item().parent() }
#[test]
fn native_bound_candidates() {
    assert_eq!(same_trait(()), 3);
    assert_eq!(inherited(()), 5);
    assert_eq!(projected(()), 5);
}
struct Order;
impl Order { fn call(&mut self) -> u32 { 6 } }
trait Shared { fn call(&self) -> u32; }
impl Shared for Order { fn call(&self) -> u32 { 7 } }
#[test]
fn shared_autoref_precedes_mutable_inherent() { assert_eq!(Order.call(), 7); }

fn opaque_recursive(n: bool) -> impl Iterator<Item = u32> {
    if n { assert_eq!(opaque_recursive(false).next(), Some(9)); }
    [9].into_iter()
}
trait DynParent { fn value(&self) -> u32 { 11 } }
trait DynChild: DynParent {}
impl DynParent for () {}
impl DynChild for () {}
#[test]
fn opaque_and_object_candidates() {
    assert_eq!(opaque_recursive(true).next(), Some(9));
    let value: &dyn DynChild = &();
    assert_eq!(value.value(), 11);
    let slice: &[u32] = &[1, 2];
    assert_eq!([1, 2].len(), slice.len());
}

trait OwnedObject { fn owned(self: std::sync::Arc<Self>) -> u32; }
impl OwnedObject for u32 { fn owned(self: std::sync::Arc<Self>) -> u32 { *self } }
#[test]
fn object_pick_extracts_self_from_typed_receiver() {
    let value: std::sync::Arc<dyn OwnedObject> = std::sync::Arc::new(17u32);
    assert_eq!(value.owned(), 17);
}
