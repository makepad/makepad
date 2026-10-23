// Web target differential program (run.sh): the same output from rustc's build of it (a
// wasm32-unknown-unknown cdylib run in node for wasm32; a host binary for wasm64, whose data
// layout is x86_64's) and from Relax running it as the web target in the trace host's mode
// (MAKEPAD_RUN_CHECK_TARGET=wasm{32,64}-unknown-unknown). Output goes through the page-side
// import `js_console_log` (a page has no stdout). Pointer-heavy on purpose: fat pointers,
// trait objects, vtables, statics holding pointers, Box/Rc/Vec/String, enums with pointer niches,
// fn pointers, closures, unaligned/packed fields, 64- and 128-bit math, atomics.
#![allow(dead_code)]
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write;
use std::rc::Rc;
use std::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

#[cfg(target_family = "wasm")]
extern "C" {
    fn js_console_log(ptr: *const u8, len: usize);
}
#[cfg(target_family = "wasm")]
fn out(s: &str) {
    unsafe { js_console_log(s.as_ptr(), s.len()) }
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

struct OnceState<const N: usize>([u8; N]);
impl<const N: usize> Drop for OnceState<N> {
    fn drop(&mut self) { ONCE_DROPS.fetch_add(1, Ordering::SeqCst); }
}
static ONCE_DROPS: AtomicUsize = AtomicUsize::new(0);
#[inline(never)]
fn take_once<const N: usize>(state: OnceState<N>) -> usize {
    state.0.iter().map(|&x| x as usize).sum()
}
#[inline(never)]
fn call_once_box(f: Box<dyn FnOnce() -> usize>) -> usize { f() }
fn once_size<const N: usize>() -> usize {
    let state = OnceState([3; N]);
    call_once_box(Box::new(move || take_once(state)))
}
#[repr(align(256))]
struct OnceAligned(OnceState<24>);
#[inline(never)]
fn take_aligned(state: OnceAligned) -> usize { take_once(state.0) }

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

// C-unwind callbacks have the ScriptProtoPropsFn shape. Keep the table and its
// invocation out of line so these exercise the indirect C ABI, including taking
// the address of a by-value argument and returning an aggregate by value.
#[repr(C)]
#[derive(Clone, Copy)]
struct Object8 { index: u32, generation: u16 }
#[repr(C)]
#[derive(Clone, Copy)]
struct Object12 { a: u32, b: u32, c: u32 }
#[repr(C)]
#[derive(Clone, Copy)]
struct Object24 { a: u64, b: u64, c: u64 }
#[repr(C)]
#[derive(Clone, Copy)]
struct ObjectMixed { a: f64, b: u32 }
#[repr(C)]
#[derive(Clone, Copy)]
struct ObjectFloat24 { a: f64, b: f64, c: f64 }
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct ObjectPacked { a: u8, b: u32 }
#[repr(C)]
struct Vm { words: [u64; 96] }
#[repr(C)]
struct Props { sum: u64, calls: u32 }

trait Object: Copy {
    fn sum(&self) -> u64;
    fn changed(self) -> Self;
}
impl Object for Object8 {
    fn sum(&self) -> u64 { self.index as u64 + self.generation as u64 }
    fn changed(self) -> Self { Self { index: self.index + 7, generation: self.generation + 3 } }
}
impl Object for Object12 {
    fn sum(&self) -> u64 { self.a as u64 + self.b as u64 + self.c as u64 }
    fn changed(self) -> Self { Self { a: self.a + 1, b: self.b + 2, c: self.c + 3 } }
}
impl Object for Object24 {
    fn sum(&self) -> u64 { self.a + self.b + self.c }
    fn changed(self) -> Self { Self { a: self.a + 4, b: self.b + 5, c: self.c + 6 } }
}
impl Object for ObjectMixed {
    fn sum(&self) -> u64 { self.a as u64 + self.b as u64 }
    fn changed(self) -> Self { Self { a: self.a + 0.5, b: self.b + 9 } }
}
impl Object for ObjectFloat24 {
    fn sum(&self) -> u64 { (self.a + self.b + self.c) as u64 }
    fn changed(self) -> Self { Self { a: self.a + 0.25, b: self.b + 0.5, c: self.c + 0.75 } }
}
impl Object for ObjectPacked {
    fn sum(&self) -> u64 { self.a as u64 + self.b as u64 }
    fn changed(self) -> Self { Self { a: self.a + 1, b: self.b + 19 } }
}
struct Callbacks<T> {
    props: extern "C-unwind" fn(&mut Vm, T, &mut Props),
    result: extern "C-unwind" fn(&mut Vm, T, &mut Props) -> T,
}
#[inline(never)]
fn object_sum<T: Object>(object: &T) -> u64 { object.sum() }
#[inline(never)]
extern "C-unwind" fn proto_props_c<T: Object>(vm: &mut Vm, object: T, props: &mut Props) {
    let n = object_sum(&object);
    vm.words[83] += n;
    props.sum += n * 3;
    props.calls += 1;
}
#[inline(never)]
extern "C-unwind" fn proto_return_c<T: Object>(vm: &mut Vm, object: T, props: &mut Props) -> T {
    proto_props_c(vm, object, props);
    object.changed()
}
#[inline(never)]
fn call_callbacks<T: Object>(table: &Callbacks<T>, vm: &mut Vm, object: T, props: &mut Props) -> T {
    (table.props)(vm, object, props);
    (table.result)(vm, object, props)
}
struct SpillCallback {
    f: extern "C-unwind" fn(&mut Vm, u64, u64, u64, u64, u64, Object12, &mut Props) -> Object12,
}
#[inline(never)]
extern "C-unwind" fn proto_spill_c(vm: &mut Vm, a: u64, b: u64, c: u64, d: u64, e: u64, object: Object12, props: &mut Props) -> Object12 {
    // All six SysV integer registers are occupied before the 12-byte object.
    vm.words[7] = a + b + c + d + e;
    proto_return_c(vm, object, props)
}
#[inline(never)]
fn call_spill(table: &SpillCallback, vm: &mut Vm, object: Object12, props: &mut Props) -> Object12 {
    (table.f)(vm, 101, 203, 307, 409, 503, object, props)
}
fn c_aggregates() {
    let mut vm = Vm { words: [0; 96] };
    let mut props = Props { sum: 0, calls: 0 };
    let a = call_callbacks(&Callbacks { props: proto_props_c, result: proto_return_c }, &mut vm,
        Object8 { index: 0x12345678, generation: 0x4321 }, &mut props);
    say!("c8 {} {} {} {} {}", a.index, a.generation, vm.words[83], props.sum, props.calls);
    let b = call_callbacks(&Callbacks { props: proto_props_c, result: proto_return_c }, &mut vm,
        Object12 { a: 11, b: 23, c: 47 }, &mut props);
    say!("c12 {} {} {} {} {} {}", b.a, b.b, b.c, vm.words[83], props.sum, props.calls);
    let c = call_callbacks(&Callbacks { props: proto_props_c, result: proto_return_c }, &mut vm,
        Object24 { a: 1 << 40, b: 101, c: 203 }, &mut props);
    say!("c24 {} {} {} {} {} {}", c.a, c.b, c.c, vm.words[83], props.sum, props.calls);
    let m = call_callbacks(&Callbacks { props: proto_props_c, result: proto_return_c }, &mut vm,
        ObjectMixed { a: 17.25, b: 31 }, &mut props);
    say!("c_mixed {} {} {} {} {}", m.a, m.b, vm.words[83], props.sum, props.calls);
    let f = call_callbacks(&Callbacks { props: proto_props_c, result: proto_return_c }, &mut vm,
        ObjectFloat24 { a: 1.5, b: 2.25, c: 3.75 }, &mut props);
    say!("c_float24 {} {} {} {} {} {}", f.a, f.b, f.c, vm.words[83], props.sum, props.calls);
    let p = call_callbacks(&Callbacks { props: proto_props_c, result: proto_return_c }, &mut vm,
        ObjectPacked { a: 3, b: 98765 }, &mut props);
    let pb = p.b;
    say!("c_packed {} {} {} {} {}", p.a, pb, vm.words[83], props.sum, props.calls);
    let s = call_spill(&SpillCallback { f: proto_spill_c }, &mut vm,
        Object12 { a: 17, b: 37, c: 73 }, &mut props);
    say!("c_spill {} {} {} {} {} {} {}", s.a, s.b, s.c, vm.words[7], vm.words[83], props.sum, props.calls);
}

// Coverage probes execute between pointer uses in these small callees and in
// the string/lock implementations. Keep them hot and cross call boundaries.
#[inline(never)]
fn coverage_chars(text: &str, total: &Mutex<u64>, count: &AtomicUsize) -> u64 {
    let mut sum = 0u64;
    for (offset, ch) in text.char_indices() {
        sum = sum.wrapping_add(ch as u64 + offset as u64);
        count.fetch_or(1 << (offset % 16), Ordering::Relaxed);
    }
    *total.lock().unwrap() += sum;
    sum + count.load(Ordering::Acquire) as u64
}

fn coverage_hot() {
    let total = Mutex::new(0u64);
    let count = AtomicUsize::new(0);
    let texts = [String::from("ASCII"), String::from("Grüße ✓ 🦀"), String::new()];
    let mut sum = 0u64;
    for i in 0..4096 {
        sum += coverage_chars(&texts[i % texts.len()], &total, &count);
    }
    say!("coverage {} {} {}", sum, *total.lock().unwrap(), count.load(Ordering::Relaxed));
}

fn body() {
    numeric_casts();
    web_runtime_cases();
    coverage_hot();
    c_aggregates();
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
    // Box<dyn FnOnce> moves its environment out before freeing the allocation. Its
    // compiler-called free helper must retain the vtable on both wasm pointer widths.
    let state = OnceAligned(OnceState([7; 24]));
    let aligned = call_once_box(Box::new(move || take_aligned(state)));
    say!("box once {} {} {} {} {} drops {}", once_size::<0>(), once_size::<1>(),
        once_size::<24>(), once_size::<4096>(), aligned, ONCE_DROPS.load(Ordering::SeqCst));
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

fn web_runtime_cases() {
    use std::hint::black_box;
    let unsigned = [0u128, 1, u64::MAX as u128, 1u128 << 64, 1u128 << 127, u128::MAX];
    let mut sum = 0u128;
    for a in unsigned {
        for b in unsigned {
            if b != 0 {
                let a = black_box(a);
                let b = black_box(b);
                sum = sum.wrapping_add(a / b).wrapping_add(a % b);
            }
        }
    }
    let signed = [i128::MIN, -123456789012345678901i128, -1, 0, 1, i128::MAX];
    for a in signed {
        for b in signed {
            if b != 0 && !(a == i128::MIN && b == -1) {
                let a = black_box(a);
                let b = black_box(b);
                sum = sum.wrapping_add((a / b) as u128).wrapping_add((a % b) as u128);
            }
        }
    }
    let text = String::from("a runtime string longer than sixteen bytes");
    let same = text.clone();
    let mut different = text.clone();
    different.push('!');
    assert!(black_box(text.as_str()) == black_box(same.as_str()));
    assert!(black_box(text.as_str()) != black_box(different.as_str()));
    different.pop();
    different.replace_range(20..21, "X");
    assert!(black_box(text.as_str()) != black_box(different.as_str()));
    unsafe {
        use std::alloc::{GlobalAlloc, Layout, System};
        let layout = Layout::from_size_align(131072, 256).unwrap();
        let p = System.alloc_zeroed(layout);
        assert!(!p.is_null());
        assert_eq!(p as usize % 256, 0);
        for i in 0..layout.size() { assert_eq!(*p.add(i), 0); *p.add(i) = (i % 251) as u8; }
        let q = System.realloc(p, layout, 393216);
        assert!(!q.is_null());
        for i in 0..layout.size() { assert_eq!(*q.add(i), (i % 251) as u8); }
        let large = Layout::from_size_align(393216, 256).unwrap();
        let p = System.realloc(q, large, 65536);
        assert!(!p.is_null());
        for i in 0..65536 { assert_eq!(*p.add(i), (i % 251) as u8); }
        System.dealloc(p, Layout::from_size_align(65536, 256).unwrap());
    }
    say!("web_runtime {sum}");
}

fn numeric_casts() {
    use std::hint::black_box;
    // Both signednesses and precisions, including halfway values on either side
    // of an even significand and carries into the next exponent (f32 infinity).
    for value in [0u128, 1, (1u128 << 24) + 1, (1u128 << 24) + 3,
        (1u128 << 53) + 1, (1u128 << 53) + 3, 1u128 << 63,
        u64::MAX as u128, 1u128 << 64, (1u128 << 100) + (1u128 << 76) + 1,
        (1u128 << 100) + (1u128 << 47) + 1, 1u128 << 127, u128::MAX] {
        let x = black_box(value);
        let y = black_box(value as i128);
        say!("wide_cast {} {} {} {} {}", x, (x as f32).to_bits(), (x as f64).to_bits(),
            (y as f32).to_bits(), (y as f64).to_bits());
        let n = black_box(y.wrapping_neg());
        say!("wide_negative {} {} {}", n, (n as f32).to_bits(), (n as f64).to_bits());
        let x = black_box(value as u64);
        let y = black_box(value as i64);
        say!("int_cast {} {} {} {} {}", x, (x as f32).to_bits(), (x as f64).to_bits(),
            (y as f32).to_bits(), (y as f64).to_bits());
    }
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.0, 0.0, -1.0, 1.5,
        -129.0, 256.0, -32769.0, 65536.0, -2147483649.0, 4294967296.0,
        9007199254740992.0, 9223372036854775808.0, 18446744073709551616.0,
        170141183460469231731687303715884105728.0, 340282366920938463463374607431768211456.0] {
        let x = black_box(value);
        let y = black_box(value as f32);
        say!("f64_cast {} {} {} {} {} {} {} {} {} {}", x as i8, x as u8, x as i16,
            x as u16, x as i32, x as u32, x as i64, x as u64, x as i128, x as u128);
        say!("f32_cast {} {} {} {} {} {} {} {} {} {}", y as i8, y as u8, y as i16,
            y as u16, y as i32, y as u32, y as i64, y as u64, y as i128, y as u128);
    }
}
