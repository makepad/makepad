// KNOWN FAILING (all tiers): a panic caught inside a thread-local's destructor at thread exit has the
// payload "explicit panic" under Relax, rustc keeps the message ("tally exit too long"). Relax std keeps the
// payload in a thread-local (std/panic.rs PAYLOAD) that is already destroyed then, and its LocalKey destroys
// keys of types without drop glue too (rustc's never are). Found by FIX-TLS 2026-10-21. Move to ../diff/ when
// fixed.
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

fn work() {
    REPORTS.lock().unwrap().clear();
    let h = std::panic::take_hook(); std::panic::set_hook(Box::new(|_| {}));
    drop(T("main"));
    std::thread::spawn(|| drop(T("live"))).join().unwrap();
    std::thread::spawn(|| K.with(|k| *k.borrow_mut() = Some(T("exit")))).join().unwrap();
    for r in REPORTS.lock().unwrap().iter() { d!("{}", r); }
    std::panic::set_hook(h);
}
