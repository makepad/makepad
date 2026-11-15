// std-core differential test (core_diff.sh, agree.sh): Box's allocator parameter, which rustc 1.100.0
// stabilized (relax issue 156). Boxes in an allocator of our own that counts and prints what goes
// through it, by reference and by value (one too big for registers): new_in, the raw round trips,
// zero-sized contents, the drop order of contents, block and allocator, moves out of `*b` (whole,
// partly, from a temporary), a contents drop that unwinds, take and map, clone, unsizing to slices,
// trait objects and supertraits, downcasts, calls through Box<dyn Fn*, A>, leak, into_pin, a boxed
// allocator, and the layouts. "| " lines must equal rustc's.
use std::alloc::{AllocError, Allocator, Global, Layout, System};
use std::any::Any;
use std::cell::Cell;
use std::fmt::Debug;
use std::mem::size_of;
use std::pin::Pin;
use std::ptr::NonNull;

#[derive(Default)]
struct Counting {
    allocs: Cell<usize>,
    frees: Cell<usize>,
    live: Cell<usize>,
}

unsafe impl Allocator for Counting {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        self.allocs.set(self.allocs.get() + 1);
        self.live.set(self.live.get() + layout.size());
        Global.allocate(layout)
    }
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        self.frees.set(self.frees.get() + 1);
        self.live.set(self.live.get() - layout.size());
        unsafe { Global.deallocate(ptr, layout) }
    }
}

impl Counting {
    fn line(&self, what: &str) {
        println!("| {what}: allocs {} frees {} live {}", self.allocs.get(), self.frees.get(), self.live.get());
    }
}

/// An allocator held by value that says when it frees and when it is dropped.
struct Named<'a>(&'a Counting, &'static str);

unsafe impl Allocator for Named<'_> {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        self.0.allocate(layout)
    }
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        println!("| {} frees {} bytes", self.1, layout.size());
        unsafe { self.0.deallocate(ptr, layout) }
    }
}

impl Drop for Named<'_> {
    fn drop(&mut self) {
        println!("| {} dropped", self.1);
    }
}

/// An allocator too big for registers: a Box holding it lives in memory.
struct Big<'a> {
    inner: &'a Counting,
    pad: [u64; 9],
}

unsafe impl Allocator for Big<'_> {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        self.inner.allocate(layout)
    }
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        println!("| big frees {} bytes (pad {})", layout.size(), self.pad[8]);
        unsafe { self.inner.deallocate(ptr, layout) }
    }
}

struct Loud(&'static str);

impl Drop for Loud {
    fn drop(&mut self) {
        println!("| {} dropped", self.0);
    }
}

struct Panics;

impl Drop for Panics {
    fn drop(&mut self) {
        println!("| panics dropping");
        panic!("contents drop");
    }
}

trait Shape: Debug {
    fn area(&self) -> u32;
}
trait Named2: Shape {
    fn name(&self) -> &'static str;
}

#[derive(Debug, Clone, PartialEq)]
struct Sq(u32);

impl Shape for Sq {
    fn area(&self) -> u32 {
        self.0 * self.0
    }
}
impl Named2 for Sq {
    fn name(&self) -> &'static str {
        "square"
    }
}

#[test]
fn a01_new_in_and_raw_round_trips() {
    let a = Counting::default();
    {
        let b = Box::new_in([7u64; 4], &a);
        a.line("new_in");
        println!("| {:?} ours {}", *b, std::ptr::eq(*Box::allocator(&b), &a));
        let (raw, alloc) = Box::into_raw_with_allocator(b);
        let b = unsafe { Box::from_raw_in(raw, alloc) };
        let (nn, alloc) = Box::into_non_null_with_allocator(b);
        let mut b = unsafe { Box::from_non_null_in(nn, alloc) };
        b[1] = 9;
        let same = Box::as_ptr(&b) == Box::as_mut_ptr(&mut b) as *const _;
        println!("| round trips {:?} {}", &b[..2], same);
        let unit = Box::new_in((), &a);
        a.line("zero-sized contents");
        drop(unit);
        let e: Box<[u8; 0], &Counting> = Box::new_in([], &a);
        println!("| empty array {}", e.len());
    }
    a.line("dropped");
    let g = Box::new_in(5u16, Global);
    let s = Box::new_in(6u16, System);
    println!("| global {} system {}", *g + 1, *s + 1);
}

#[test]
fn a02_drop_order_and_moves_out() {
    let a = Counting::default();
    drop(Box::new_in(Loud("contents"), Named(&a, "alloc")));
    println!("| --");
    {
        let b = Box::new_in((Loud("first"), Loud("second")), Named(&a, "pair"));
        let whole = *b;
        println!("| moved out {}", whole.1 .0);
    }
    println!("| --");
    {
        let b = Box::new_in((Loud("kept"), Loud("left")), Named(&a, "partly"));
        let k = b.0;
        println!("| took {}", k.0);
    }
    println!("| --");
    let make = || Box::new_in(Loud("temporary"), Named(&a, "temp"));
    let t = *make();
    println!("| from a temporary {}", t.0);
    drop(t);
    a.line("moves");
}

#[test]
fn a03_unwinding_contents_still_free() {
    let a = Counting::default();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(Box::new_in((Loud("beside"), Panics), &a));
    }));
    println!("| whole drop unwound {}", r.is_err());
    a.line("after the whole drop");
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let b = Box::new_in((Loud("moved"), Panics), &a);
        let m = b.0;
        println!("| took {}", m.0);
    }));
    println!("| partial drop unwound {}", r.is_err());
    a.line("after the partial drop");
}

#[test]
fn a04_take_map_clone() {
    let a = Counting::default();
    let b = Box::new_in(41u32, &a);
    let (v, slot) = Box::take(b);
    let b = Box::write(slot, v + 1);
    a.line("take and write");
    let same = Box::map(b, |x| x as i32 * 2);
    a.line("map, same size");
    let wider = Box::map(same, |x| (x as u64, x as u64));
    a.line("map, other size");
    println!("| mapped {:?}", *wider);
    let c = wider.clone();
    a.line("clone");
    let mut d = Box::new_in((0u64, 0u64), &a);
    d.clone_from(&c);
    a.line("clone_from");
    println!("| clones {:?} {:?} {}", *c, *d, c == d);
    drop((wider, c, d));
    let z = Box::map(Box::new_in((), &a), |()| 3u8);
    a.line("map from zero-sized");
    println!("| {}", *z);
    drop(z);
    a.line("all dropped");
}

#[test]
fn a05_unsizing_and_dyn() {
    let a = Counting::default();
    {
        let s: Box<[u32], &Counting> = Box::new_in([1, 2, 3], &a);
        println!("| slice {:?} len {}", s, s.len());
        let back: Result<Box<[u32; 3], &Counting>, _> = s.try_into();
        println!("| back to an array {:?}", back.map(|b| b[2]).ok());
        let d: Box<dyn Shape, &Counting> = Box::new_in(Sq(4), &a);
        println!("| dyn {:?} area {}", d, d.area());
        let n: Box<dyn Named2, &Counting> = Box::new_in(Sq(5), &a);
        println!("| {}", n.name());
        let up: Box<dyn Shape, &Counting> = n;
        println!("| upcast area {}", up.area());
        let any: Box<dyn Any, &Counting> = Box::new_in(Sq(6), &a);
        match any.downcast::<u8>() {
            Ok(_) => println!("| wrong downcast"),
            Err(any) => println!("| downcast to Sq {:?}", any.downcast::<Sq>().map(|b| b.0).ok()),
        }
        a.line("dyn values live");
        let f: Box<dyn Fn(u32) -> u32, &Counting> = Box::new_in(|x| x + 1, &a);
        let mut total = 0;
        let mut g: Box<dyn FnMut(u32), &Counting> = Box::new_in(|x| total += x, &a);
        g(f(2));
        g(f(10));
        drop(g);
        let s = String::from("owned");
        let h: Box<dyn FnOnce() -> usize, &Counting> = Box::new_in(move || s.len(), &a);
        println!("| calls {} {}", total, h());
    }
    a.line("dyn dropped");
}

#[test]
fn a06_by_value_allocators() {
    let a = Counting::default();
    {
        let mut big = Big { inner: &a, pad: [0; 9] };
        big.pad[8] = 77;
        let b = Box::new_in(Loud("in big"), big);
        println!("| big holds {} pad {}", b.0, Box::allocator(&b).pad[8]);
        let d: Box<dyn Any, Big> = Box::new_in(Sq(3), Big { inner: &a, pad: [1; 9] });
        println!("| big dyn is Sq {}", d.is::<Sq>());
        let pair = Box::new_in((Loud("big first"), Loud("big second")), Big { inner: &a, pad: [2; 9] });
        let (x, _) = *pair;
        println!("| big moved out {}", x.0);
    }
    a.line("big dropped");
    let s: Box<[Loud], Named> = Box::new_in([Loud("e0"), Loud("e1")], Named(&a, "slice"));
    println!("| named slice {}", s.len());
    drop(s);
    let d: Box<dyn Shape, Named> = Box::new_in(Sq(2), Named(&a, "dyn"));
    println!("| named dyn {}", d.area());
    drop(d);
    a.line("named dropped");
}

#[test]
fn a07_leak_pin_boxed_allocator_layouts() {
    let a = Counting::default();
    let l: &mut u32 = Box::leak(Box::new_in(8u32, &a));
    *l += 1;
    println!("| leaked {}", *l);
    a.line("leak");
    let p: Pin<Box<u32, System>> = Box::into_pin(Box::new_in(3, System));
    let q: Pin<Box<u32, &Global>> = Pin::from(Box::new_in(4, &Global));
    println!("| pinned {} {}", *p, *q);
    let boxed: Box<Counting> = Box::new(Counting::default());
    let layout = Layout::new::<u64>();
    let m = boxed.allocate(layout).unwrap();
    unsafe { boxed.deallocate(m.cast(), layout) };
    boxed.line("a boxed allocator");
    fn send<T: Send>(_: &T) -> bool {
        true
    }
    println!("| a System box is Send {}", send(&Box::new_in(1u8, System)));
    println!("| Box<u8> {} Option {} dyn {} slice {}", size_of::<Box<u8>>(), size_of::<Option<Box<u8>>>(), size_of::<Box<dyn Shape>>(), size_of::<Box<[u8]>>());
    println!("| Box<u8, &A> {} Option {} dyn {} Big {}", size_of::<Box<u8, &Counting>>(), size_of::<Option<Box<u8, &Counting>>>(), size_of::<Box<dyn Shape, &Counting>>(), size_of::<Box<u8, Big>>());
    println!("| Box<u8, Global> {} System {}", size_of::<Box<u8, Global>>(), size_of::<Box<u8, System>>());
}
