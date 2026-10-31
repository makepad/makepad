// One closure expression of a generic fn whose instances differ in closure kind: in
// `LocalKey<Cell<T>>::set`, `|cell| cell.set(value)` borrows a Copy `value` (Fn, its state one
// reference, passed in a register) and moves any other (FnOnce, its state in memory with drop
// flags). A warm run that learns the FnOnce instance first (`b` first) must still build the Fn
// instance's callees as the cached caller passes the state. Also: thread-locals and statics
// holding heap pointers set at run time, read from another thread.
use std::cell::Cell;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::OnceLock;

thread_local! {
    static NUM: Cell<u64> = const { Cell::new(0) };
    static NAME: Cell<String> = Cell::new(String::new());
}

static BOXED: AtomicPtr<u64> = AtomicPtr::new(std::ptr::null_mut());
static ONCE: OnceLock<Vec<u64>> = OnceLock::new();

fn use_num(v: u64) -> u64 {
    std::thread::spawn(move || {
        NUM.set(v);
        NUM.get()
    })
    .join()
    .unwrap()
}

fn use_name(s: &str) -> u64 {
    let s = s.to_string();
    std::thread::spawn(move || {
        NAME.set(s);
        let t = NAME.take();
        t.len() as u64
    })
    .join()
    .unwrap()
}

fn main() {
    let order = std::env::args().nth(1).unwrap_or_default();
    BOXED.store(Box::into_raw(Box::new(7)), Ordering::Release);
    ONCE.get_or_init(|| vec![1, 2, 3]);
    let mut r = 0;
    if order == "b" {
        r += use_name("hello");
        r += use_num(5) * 10;
    } else {
        r += use_num(5) * 10;
        r += use_name("hello");
    }
    let shared = std::thread::spawn(|| unsafe { *BOXED.load(Ordering::Acquire) } + ONCE.get().unwrap().iter().sum::<u64>())
        .join()
        .unwrap();
    println!("cache_warm_closure_once result {}", r * 100 + shared);
}
