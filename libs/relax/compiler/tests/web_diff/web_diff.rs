// Web target differential program (run.sh): the same output from rustc's build of it (a
// wasm32-unknown-unknown cdylib run in node for wasm32; a host binary for wasm64, whose data
// layout is x86_64's) and from Relax running it as the web target in the trace host's mode
// (MAKEPAD_RUN_CHECK_TARGET=wasm{32,64}-unknown-unknown). Output goes through the page-side
// import `relax_web_print` (a page has no stdout). Pointer-heavy on purpose: fat pointers,
// trait objects, vtables, statics holding pointers, Box/Rc/Vec/String, enums with pointer niches,
// fn pointers, closures, unaligned/packed fields, 64- and 128-bit math, atomics.
#![allow(dead_code)]
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write;
use std::rc::Rc;
use std::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering};

#[cfg(target_family = "wasm")]
extern "C" {
    fn relax_web_print(ptr: *const u8, len: usize);
}
#[cfg(target_family = "wasm")]
fn out(s: &str) {
    unsafe { relax_web_print(s.as_ptr(), s.len()) }
}
#[cfg(not(target_family = "wasm"))]
fn out(s: &str) {
    use std::io::Write;
    let _ = std::io::stderr().write_all(s.as_bytes());
}

macro_rules! say {
    ($($t:tt)*) => {{
        let mut s = String::new();
        let _ = writeln!(s, $($t)*);
        out(&s);
    }};
}

trait Shape {
    fn area(&self) -> f64;
    fn name(&self) -> String;
}
struct Sq(f64);
struct Circ { r: f64, label: &'static str }
impl Shape for Sq {
    fn area(&self) -> f64 { self.0 * self.0 }
    fn name(&self) -> String { format!("sq{}", self.0) }
}
impl Shape for Circ {
    fn area(&self) -> f64 { 3.0 * self.r * self.r }
    fn name(&self) -> String { format!("circ:{}", self.label) }
}

static NAMES: [&str; 3] = ["alpha", "beta", "gamma"];
static TABLE: &[(&str, fn(u32) -> u32)] = &[("dbl", dbl), ("sq", sqr), ("inc", |x| x + 1)];
static COUNT: AtomicUsize = AtomicUsize::new(0);
static BIG: AtomicU64 = AtomicU64::new(1 << 40);
static mut CELL: [u32; 4] = [1, 2, 3, 4];
thread_local! {
    static TL: Cell<u32> = const { Cell::new(7) };
}

fn dbl(x: u32) -> u32 { x * 2 }
fn sqr(x: u32) -> u32 { x * x }

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Leaf(i64),
    Pair(Box<Node>, Box<Node>),
    Name(String),
    Empty,
}

fn eval(n: &Node) -> i64 {
    match n {
        Node::Leaf(v) => *v,
        Node::Pair(a, b) => eval(a) * 3 + eval(b),
        Node::Name(s) => s.len() as i64,
        Node::Empty => -1,
    }
}

#[repr(C, packed)]
struct Packed { a: u8, b: u32, c: u16 }

#[repr(C)]
struct Header { tag: u8, ptr: *const u8, len: usize, next: Option<&'static Header> }
unsafe impl Sync for Header {}
static H2: Header = Header { tag: 2, ptr: b"two".as_ptr(), len: 3, next: None };
static H1: Header = Header { tag: 1, ptr: b"one!".as_ptr(), len: 4, next: Some(&H2) };

fn walk(h: &Header) -> String {
    let mut s = String::new();
    let mut cur = Some(h);
    while let Some(x) = cur {
        let bytes = unsafe { std::slice::from_raw_parts(x.ptr, x.len) };
        let _ = write!(s, "{}:{} ", x.tag, std::str::from_utf8(bytes).unwrap());
        cur = x.next;
    }
    s
}

fn body() {
    say!("usize {} ptr {} fat {} dyn {} opt_box {} vec {} string {}",
        std::mem::size_of::<usize>(), std::mem::size_of::<*const u8>(), std::mem::size_of::<&[u8]>(),
        std::mem::size_of::<&dyn Shape>(), std::mem::size_of::<Option<Box<u8>>>(), std::mem::size_of::<Vec<u8>>(),
        std::mem::size_of::<String>());
    // trait objects through vtables, in a Vec<Box<dyn>>
    let shapes: Vec<Box<dyn Shape>> = vec![Box::new(Sq(2.0)), Box::new(Circ { r: 1.5, label: "c1" }), Box::new(Sq(0.5))];
    let total: f64 = shapes.iter().map(|s| s.area()).sum();
    let names: Vec<String> = shapes.iter().map(|s| s.name()).collect();
    say!("shapes {:.3} {:?}", total, names);
    // statics of pointers: &str arrays, fn pointer tables, a linked list of statics
    for (i, n) in NAMES.iter().enumerate() {
        say!("name {} {} {}", i, n, n.len());
    }
    for (n, f) in TABLE {
        say!("fn {} {}", n, f(7));
    }
    say!("walk {}", walk(&H1));
    // recursive boxed enum
    let tree = Node::Pair(Box::new(Node::Pair(Box::new(Node::Leaf(4)), Box::new(Node::Name("hello".to_string())))), Box::new(Node::Leaf(-9)));
    say!("tree {} {:?}", eval(&tree), tree.clone() == tree);
    say!("empty {} {:?}", eval(&Node::Empty), Node::Name("x".into()));
    // Rc / RefCell graph
    let shared = Rc::new(RefCell::new(Vec::<u32>::new()));
    let handles: Vec<Rc<RefCell<Vec<u32>>>> = (0..4).map(|_| shared.clone()).collect();
    for (i, h) in handles.iter().enumerate() {
        h.borrow_mut().push(i as u32 * 10);
    }
    say!("rc {} {:?}", Rc::strong_count(&shared), shared.borrow());
    // closures capturing by reference and by move, boxed FnMut
    let mut acc = 0u64;
    let mut add = |x: u64| acc += x;
    for i in 1..=10 {
        add(i * i);
    }
    let k = 5u64;
    let mut fs: Vec<Box<dyn FnMut(u64) -> u64>> = Vec::new();
    fs.push(Box::new(move |x| x * k));
    let mut st = 0u64;
    fs.push(Box::new(move |x| { st += x; st }));
    let r: Vec<u64> = (0..3).map(|i| { let a = fs[0](i); fs[1](a) }).collect();
    say!("closures {} {:?}", acc, r);
    // strings, chars, formatting
    let mut s = String::from("Grüße, wasm32!");
    s.push_str(" ✓");
    let up: String = s.chars().rev().filter(|c| !c.is_whitespace()).collect();
    say!("str {} {} {} {:?}", s.len(), s.chars().count(), up, s.find("wasm"));
    say!("fmt {:>8}|{:<6}|{:^7}|{:08.3}|{:x}|{:#b}|{:e}", "r", "l", "c", -3.14159, 0xdead_beefu32, 5u8, 1234.5f64);
    // collections
    let mut m = BTreeMap::new();
    for w in "the quick brown fox jumps over the lazy dog the end".split(' ') {
        *m.entry(w).or_insert(0usize) += 1;
    }
    say!("btree {:?}", m);
    let mut q: VecDeque<i32> = (1..=5).collect();
    q.rotate_left(2);
    q.push_front(-1);
    say!("deque {:?}", q);
    let mut v: Vec<i64> = (0..40).map(|i| (i * 7919) % 101 - 50).collect();
    v.sort_unstable();
    v.dedup();
    say!("sorted {} {:?} {:?}", v.len(), &v[..5], v.binary_search(&0));
    // 64/128-bit integer math, wrapping, checked
    let a: u64 = 0xffff_ffff_0000_0001;
    let b: u128 = (a as u128) * (a as u128);
    say!("int {} {} {:?} {:?} {}", a.wrapping_mul(3), b, a.checked_add(u64::MAX), 7usize.checked_sub(9), (-7i64).rem_euclid(3));
    say!("usize {} {} {}", usize::MAX, isize::MIN, (usize::MAX as u64).count_ones());
    // floats
    let xs = [0.1f64, 2.5, -3.75, 1e10, 1.0 / 3.0];
    let prod: f64 = xs.iter().product();
    say!("float {} {:.6} {} {}", prod, xs.iter().map(|x| x.abs().sqrt()).sum::<f64>(), (2.5f32).powi(3), f64::EPSILON);
    // packed and repr(C) fields
    let p = Packed { a: 1, b: 0x0102_0304, c: 0x0506 };
    let pb = p.b;
    let pc = p.c;
    say!("packed {} {} {} {}", std::mem::size_of::<Packed>(), p.a, pb, pc);
    // atomics on pointers and wide integers
    let x = Box::into_raw(Box::new(41u32));
    let ap = AtomicPtr::new(std::ptr::null_mut());
    ap.store(x, Ordering::SeqCst);
    unsafe { *ap.load(Ordering::SeqCst) += 1 };
    COUNT.fetch_add(3, Ordering::Relaxed);
    BIG.fetch_add(5, Ordering::Relaxed);
    say!("atomic {} {} {}", unsafe { *x }, COUNT.load(Ordering::Relaxed), BIG.load(Ordering::Relaxed));
    unsafe { drop(Box::from_raw(x)) };
    // raw pointer arithmetic, pointer <-> integer casts
    let arr = [10u16, 20, 30, 40];
    let p0 = arr.as_ptr();
    let p3 = unsafe { p0.add(3) };
    let diff = (p3 as usize - p0 as usize) / std::mem::size_of::<u16>();
    let back = (p0 as usize + 2 * std::mem::size_of::<u16>()) as *const u16;
    say!("ptr {} {} {}", diff, unsafe { *back }, unsafe { p3.offset_from(p0) });
    unsafe {
        let c = &mut *std::ptr::addr_of_mut!(CELL);
        c[2] += 40;
        say!("static mut {:?}", c);
    }
    TL.with(|t| t.set(t.get() * 6));
    say!("tls {}", TL.with(|t| t.get()));
    // Option<&T> / Option<Box<T>> niches
    let some: Option<&u32> = arr.iter().map(|_| &7u32).next();
    let none: Option<Box<u64>> = None;
    say!("niche {:?} {:?} {}", some, none, std::mem::size_of_val(&none));
    say!("done");
}

/// rustc's wasm32 build: node calls this (run.sh)
#[no_mangle]
pub extern "C" fn run() {
    body()
}

fn main() {
    body()
}
