use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::mem::{align_of_val, size_of_val};
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

struct Counting;
#[global_allocator]
static ALLOCATOR: Counting = Counting;
static WATCH: AtomicUsize = AtomicUsize::new(0);
static SIZE: AtomicUsize = AtomicUsize::new(0);
static ALIGN: AtomicUsize = AtomicUsize::new(1);
static FREES: AtomicUsize = AtomicUsize::new(0);
static FREED_SIZE: AtomicUsize = AtomicUsize::new(0);
static FREED_ALIGN: AtomicUsize = AtomicUsize::new(0);
static DROPS: AtomicUsize = AtomicUsize::new(0);
static SUM: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ptr as usize == WATCH.load(SeqCst) {
            FREES.fetch_add(1, SeqCst);
            FREED_SIZE.store(layout.size(), SeqCst);
            FREED_ALIGN.store(layout.align(), SeqCst);
            WATCH.store(0, SeqCst);
            // Record a bad layout without passing it to System and corrupting the test heap.
            let size = SIZE.load(SeqCst);
            if size != 0 {
                System.dealloc(ptr, Layout::from_size_align_unchecked(size, ALIGN.load(SeqCst)));
            }
        } else {
            System.dealloc(ptr, layout);
        }
    }
}

fn watch<T: ?Sized>(b: &Box<T>) {
    SIZE.store(size_of_val(&**b), SeqCst);
    ALIGN.store(align_of_val(&**b), SeqCst);
    FREES.store(0, SeqCst);
    FREED_SIZE.store(0, SeqCst);
    FREED_ALIGN.store(0, SeqCst);
    DROPS.store(0, SeqCst);
    SUM.store(0, SeqCst);
    WATCH.store((&**b as *const T as *const u8) as usize, SeqCst);
}

fn report(label: &str) {
    WATCH.store(0, SeqCst);
    let size = SIZE.load(SeqCst);
    let align = ALIGN.load(SeqCst);
    let frees = FREES.load(SeqCst);
    let freed_size = FREED_SIZE.load(SeqCst);
    let freed_align = FREED_ALIGN.load(SeqCst);
    // Closure layout is unspecified; Relax may include capture drop flags. Compare the
    // allocator's layout with this implementation's actual allocation, not rustc's size.
    println!("D {label}: free_count_ok={} free_size_ok={} free_align_ok={} drops={} sum={}",
        frees == if size == 0 { 0 } else { 1 }, freed_size == size,
        freed_align == if size == 0 { 0 } else { align }, DROPS.load(SeqCst), SUM.load(SeqCst));
    assert_eq!(frees, if size == 0 { 0 } else { 1 });
    assert_eq!(freed_size, size);
    assert_eq!(freed_align, if size == 0 { 0 } else { align });
}

struct State<const N: usize>([u8; N]);
impl<const N: usize> Drop for State<N> {
    fn drop(&mut self) { DROPS.fetch_add(1, SeqCst); }
}
#[inline(never)]
fn consume<const N: usize>(state: State<N>) {
    SUM.fetch_add(state.0.iter().map(|&x| x as usize).sum::<usize>(), SeqCst);
}

#[repr(align(256))]
struct Aligned(State<24>);
#[inline(never)]
fn consume_aligned(state: Aligned) { consume(state.0); }

#[inline(never)]
fn call(b: Box<dyn FnOnce()>) { b(); }

fn closure<const N: usize>() {
    let state = black_box(State([3; N]));
    let b: Box<dyn FnOnce()> = black_box(Box::new(move || consume(state)));
    watch(&b);
    call(b);
    report("closure");
}

#[inline(never)]
fn watched_box() -> Box<State<24>> {
    let b = black_box(Box::new(State([5; 24])));
    watch(&b);
    b
}

fn work() {
    let b: Box<dyn FnOnce()> = black_box(Box::new(|| SUM.store(23, SeqCst)));
    watch(&b);
    assert_eq!(SIZE.load(SeqCst), 0);
    call(b);
    report("zero-capture closure");
    closure::<0>();
    closure::<1>();
    closure::<24>();
    closure::<4096>();
    let state = black_box(Aligned(State([7; 24])));
    let b: Box<dyn FnOnce()> = black_box(Box::new(move || consume_aligned(state)));
    watch(&b);
    call(b);
    report("aligned closure");

    local_move();
    consume(*watched_box());
    report("temporary move");

    // Unsized locals are not legal Rust. Move elements through into_vec/into_iter, or
    // recover a sized array before moving out; transfer str storage through into_string.
    let b: Box<[State<1>]> = black_box(Box::new([State([11]), State([13])]));
    watch(&b);
    for state in b.into_vec() { consume(state); }
    report("slice into_vec");
    let b: Box<[State<1>]> = black_box(Box::new([State([17]), State([19])]));
    watch(&b);
    array_move(b);
    report("slice into array move");
    let b: Box<str> = black_box(String::from("moved string").into_boxed_str());
    watch(&b);
    for byte in b.into_string().into_bytes() { SUM.fetch_add(byte as usize, SeqCst); }
    report("str into_string");
    let b: Box<[State<0>]> = black_box(Box::new([State([]), State([])]));
    watch(&b);
    for state in b.into_vec() { consume(state); }
    report("zero-sized slice");
    let b: Box<str> = black_box(String::new().into_boxed_str());
    watch(&b);
    drop(b.into_string());
    report("empty str");
}

#[inline(never)]
fn local_move_body() {
    let b = black_box(Box::new(State([9; 24])));
    watch(&b);
    consume(*b);
}
fn local_move() {
    local_move_body();
    report("local move");
}
#[inline(never)]
fn array_move(b: Box<[State<1>]>) {
    let b: Box<[State<1>; 2]> = match b.try_into() { Ok(b) => b, Err(_) => panic!("length") };
    let [a, b] = *b;
    consume(a);
    consume(b);
}

#[test]
fn run() {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for round in 0..4 {
        work();
        if round < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); }
    }
}
