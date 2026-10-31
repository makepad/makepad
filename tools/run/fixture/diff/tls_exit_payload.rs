// A panic caught inside a thread-local's destructor at thread exit keeps its payload ("tally exit too
// long"), and a thread-local without drop glue stays readable in other thread-locals' destructors (rustc never
// destroys those). Relax std once kept the payload in a destroyed thread-local ("explicit panic") and destroyed
// keys of types without drop glue (FIX-TLS 2026-10-21; fixed by ALLOC: late keys, std/thread/local.rs).
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

use std::cell::RefCell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Mutex;

static REPORTS: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct T(&'static str);
impl Drop for T {
    fn drop(&mut self) {
        let c = catch_unwind(AssertUnwindSafe(|| { if self.0.len() > 1 { panic!("tally {} too long", self.0); } 1 }));
        let m = match &c { Ok(_) => "ok".to_string(), Err(e) => e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default() };
        REPORTS.lock().unwrap().push(format!("{} {}", self.0, m));
    }
}
thread_local! { static K: RefCell<Option<T>> = const { RefCell::new(None) }; }
thread_local! { static N: std::cell::Cell<u32> = const { std::cell::Cell::new(7) }; }
struct R;
impl Drop for R {
    fn drop(&mut self) {
        let n = N.try_with(|n| { n.set(n.get() + 1); n.get() });
        REPORTS.lock().unwrap().push(format!("late {:?}", n.ok()));
    }
}
thread_local! { static L: RefCell<Option<R>> = const { RefCell::new(None) }; }

fn work() {
    REPORTS.lock().unwrap().clear();
    let h = std::panic::take_hook(); std::panic::set_hook(Box::new(|_| {}));
    drop(T("main"));
    std::thread::spawn(|| drop(T("live"))).join().unwrap();
    std::thread::spawn(|| K.with(|k| *k.borrow_mut() = Some(T("exit")))).join().unwrap();
    // N is set before L (first thread) or first touched in L's destructor at exit (second)
    std::thread::spawn(|| { N.with(|n| n.set(40)); L.with(|l| *l.borrow_mut() = Some(R)); }).join().unwrap();
    std::thread::spawn(|| L.with(|l| *l.borrow_mut() = Some(R))).join().unwrap();
    for r in REPORTS.lock().unwrap().iter() { d!("{}", r); }
    std::panic::set_hook(h);
}
