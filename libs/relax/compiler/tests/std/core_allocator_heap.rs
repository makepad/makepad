// std-core differential test (core_diff.sh, agree.sh): the Allocator trait rustc 1.100.0 stabilized
// (relax issue 156), called directly: Global and System (allocate, allocate_zeroed, grow, grow_zeroed,
// shrink, deallocate, zero sizes, another alignment), an allocator of our own through the trait's
// defaults and through `&A` / `&mut A`, and one that refuses. "| " lines must equal rustc's.
use std::alloc::{AllocError, Allocator, Global, Layout, System};
use std::cell::Cell;
use std::ptr::NonNull;

fn fill(p: NonNull<[u8]>, n: usize) {
    for i in 0..n {
        unsafe { *p.as_ptr().cast::<u8>().add(i) = i as u8 * 3 + 1 };
    }
}

fn kept(p: NonNull<[u8]>, n: usize) -> bool {
    (0..n).all(|i| unsafe { *p.as_ptr().cast::<u8>().add(i) } == i as u8 * 3 + 1)
}

fn zero_from(p: NonNull<[u8]>, from: usize, to: usize) -> bool {
    (from..to).all(|i| unsafe { *p.as_ptr().cast::<u8>().add(i) } == 0)
}

fn addr(p: NonNull<[u8]>) -> usize {
    p.as_ptr().cast::<u8>() as usize
}

fn walk<A: Allocator>(name: &str, a: &A) {
    let l16 = Layout::from_size_align(16, 8).unwrap();
    let l40 = Layout::from_size_align(40, 8).unwrap();
    let l64z = Layout::from_size_align(64, 8).unwrap();
    let l8 = Layout::from_size_align(8, 8).unwrap();
    let p = a.allocate(l16).unwrap();
    println!("| {name}: allocate 16 -> len {} aligned {}", p.len(), addr(p) % 8 == 0);
    fill(p, 16);
    let p = unsafe { a.grow(p.cast(), l16, l40) }.unwrap();
    println!("| {name}: grow 16->40 len {} kept {}", p.len(), kept(p, 16));
    let p = unsafe { a.grow_zeroed(p.cast(), l40, l64z) }.unwrap();
    println!("| {name}: grow_zeroed 40->64 len {} kept {} zeroed tail {}", p.len(), kept(p, 16), zero_from(p, 40, 64));
    let p = unsafe { a.shrink(p.cast(), l64z, l8) }.unwrap();
    println!("| {name}: shrink 64->8 len {} kept {}", p.len(), kept(p, 8));
    // another alignment: not one realloc
    let l8a32 = Layout::from_size_align(8, 32).unwrap();
    let l96a32 = Layout::from_size_align(96, 32).unwrap();
    let p = unsafe { a.grow(p.cast(), l8, l96a32) }.unwrap();
    println!("| {name}: grow 8/8->96/32 len {} aligned32 {} kept {}", p.len(), addr(p) % 32 == 0, kept(p, 8));
    let p = unsafe { a.shrink(p.cast(), l96a32, l8a32) }.unwrap();
    println!("| {name}: shrink 96->8 at 32 len {} kept {}", p.len(), kept(p, 8));
    unsafe { a.deallocate(p.cast(), l8a32) };
    let z = a.allocate_zeroed(Layout::from_size_align(100, 4).unwrap()).unwrap();
    println!("| {name}: allocate_zeroed 100 len {} all zero {}", z.len(), zero_from(z, 0, 100));
    let z = unsafe { a.shrink(z.cast(), Layout::from_size_align(100, 4).unwrap(), Layout::from_size_align(0, 4).unwrap()) }.unwrap();
    println!("| {name}: shrink to 0 len {} dangling {}", z.len(), addr(z) == 4);
    let e = a.allocate(Layout::from_size_align(0, 16).unwrap()).unwrap();
    println!("| {name}: allocate 0 at 16 len {} dangling {}", e.len(), addr(e) == 16);
    let g = unsafe { a.grow(e.cast(), Layout::from_size_align(0, 16).unwrap(), Layout::from_size_align(24, 16).unwrap()) }.unwrap();
    println!("| {name}: grow 0->24 len {} aligned {}", g.len(), addr(g) % 16 == 0);
    unsafe { a.deallocate(g.cast(), Layout::from_size_align(24, 16).unwrap()) };
    unsafe { a.deallocate(e.cast(), Layout::from_size_align(0, 16).unwrap()) };
}

/// Counts its calls and takes the trait's defaults for everything but allocate and deallocate.
#[derive(Default)]
struct Counting {
    allocs: Cell<usize>,
    frees: Cell<usize>,
    live: Cell<isize>,
}

unsafe impl Allocator for Counting {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        self.allocs.set(self.allocs.get() + 1);
        self.live.set(self.live.get() + layout.size() as isize);
        Global.allocate(layout)
    }
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        self.frees.set(self.frees.get() + 1);
        self.live.set(self.live.get() - layout.size() as isize);
        unsafe { Global.deallocate(ptr, layout) }
    }
}

impl Counting {
    fn line(&self, what: &str) {
        println!("| {what}: allocs {} frees {} live {}", self.allocs.get(), self.frees.get(), self.live.get());
    }
}

/// Never gives memory.
struct Refusing;

unsafe impl Allocator for Refusing {
    fn allocate(&self, _: Layout) -> Result<NonNull<[u8]>, AllocError> {
        Err(AllocError)
    }
    unsafe fn deallocate(&self, _: NonNull<u8>, _: Layout) {}
}

#[test]
fn a01_global_and_system() {
    walk("Global", &Global);
    walk("System", &System);
    let g: Global = Default::default();
    let s: System = Default::default();
    let (g2, s2) = (g.clone(), s.clone());
    println!("| {:?} {:?} {:?} {:?}", g, g2, s, s2);
}

#[test]
fn a02_the_trait_s_defaults_and_references() {
    let c = Counting::default();
    walk("Counting", &c);
    c.line("Counting by the defaults");
    let c = Counting::default();
    walk("&Counting", &&c);
    c.line("through &A");
    let mut c = Counting::default();
    {
        let m = &mut c;
        walk("&mut Counting", &m);
    }
    c.line("through &mut A");
}

#[test]
fn a03_refusal() {
    let l = Layout::from_size_align(32, 8).unwrap();
    let r = Refusing.allocate(l);
    println!("| allocate {:?}", r);
    println!("| zeroed {:?}", (&Refusing).allocate_zeroed(l));
    let base = Global.allocate(l).unwrap();
    let grown = unsafe { (&Refusing).grow(base.cast(), l, Layout::from_size_align(64, 8).unwrap()) };
    println!("| grow {:?}", grown.map(|p| p.len()));
    unsafe { Global.deallocate(base.cast(), l) };
    let e = AllocError;
    let as_error: &dyn std::error::Error = &e;
    println!("| {} / {:?} / {} / {}", e, e, as_error, e == AllocError);
}
