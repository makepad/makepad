// Route under Relax (makepad_mbtile_reader codec.rs + brotli): (1) `<T as Trait<A>>::Assoc::f()`,
// a qualified path to an associated type and then an associated fn on it; (2) a generic body
// with two bounds of one trait at different args (`A8: Allocator<u8>`, `AH: Allocator<Hc>`)
// instantiated with one type implementing both: each call goes through the bound of the
// receiver's declared param, as rustc resolves it on the generic body
trait Allocator<T> { type AllocatedMemory: Default + AsRef<[T]>; fn alloc_cell(&mut self, n: usize) -> Self::AllocatedMemory; }
#[derive(Default)]
struct Mem<T>(Vec<T>);
impl<T> AsRef<[T]> for Mem<T> { fn as_ref(&self) -> &[T] { &self.0 } }
#[derive(Default, Clone, Copy, Debug, PartialEq)]
struct Hc(u16);
struct HeapAlloc<T> { d: T }
impl<T: Clone + Default> Allocator<T> for HeapAlloc<T> {
    type AllocatedMemory = Mem<T>;
    fn alloc_cell(&mut self, n: usize) -> Mem<T> { Mem(vec![self.d.clone(); n]) }
}
// one allocator for every element type (brotli's StandardAlloc)
struct StdAlloc;
impl<T: Clone + Default> Allocator<T> for StdAlloc {
    type AllocatedMemory = Mem<T>;
    fn alloc_cell(&mut self, n: usize) -> Mem<T> { Mem(vec![T::default(); n]) }
}
struct Inner<AH: Allocator<Hc>> { trees: AH::AllocatedMemory }
struct State<A8: Allocator<u8>, AH: Allocator<Hc>> { a8: A8, ah: AH, ring: A8::AllocatedMemory, inner: Inner<AH> }
impl<A8: Allocator<u8>, AH: Allocator<Hc>> State<A8, AH> {
    fn new(a8: A8, ah: AH) -> Self {
        let mut s = State { a8, ah, ring: A8::AllocatedMemory::default(), inner: Inner::<AH> { trees: AH::AllocatedMemory::default() } };
        s.inner.trees = s.ah.alloc_cell(3);
        s
    }
    fn fill(&mut self) -> usize {
        self.ring = self.a8.alloc_cell(2);
        let old = core::mem::replace(&mut self.inner.trees, AH::AllocatedMemory::default());
        let n = old.as_ref().len();
        self.inner.trees = self.ah.alloc_cell(4);
        n
    }
}
#[test]
fn qualified_assoc_type_then_fn() {
    let mut a = HeapAlloc::<u8> { d: 7 };
    let m = if a.d == 0 { a.alloc_cell(2) } else { <HeapAlloc<u8> as Allocator<u8>>::AllocatedMemory::default() };
    assert_eq!(m.as_ref().len(), 0);
    assert_eq!(<HeapAlloc<u8> as Allocator<u8>>::AllocatedMemory::default().as_ref().len(), 0);
    assert_eq!(a.alloc_cell(3).as_ref(), &[7, 7, 7]);
}
#[test]
fn bounds_equal_in_the_instance() {
    let mut s = State::new(StdAlloc, StdAlloc);
    assert_eq!(s.fill(), 3);
    assert_eq!(s.ring.as_ref(), &[0u8, 0]);
    assert_eq!(s.inner.trees.as_ref(), &[Hc(0); 4]);
    let mut h = State::new(HeapAlloc { d: 1u8 }, HeapAlloc { d: Hc(5) });
    assert_eq!(h.fill(), 3);
    assert_eq!(h.inner.trees.as_ref(), &[Hc(5); 4]);
}
