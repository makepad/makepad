// diff_fast corpus program: `#[test] run` calls work() in rounds (the O3 tier installs between rounds); every
// result line is `D ...`. rustc --test -O and `makepad test` (baseline and O3) must print identical D lines.
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::cell::RefCell;
thread_local! { static ORDER: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) }; }
struct G(&'static str);
impl Drop for G { fn drop(&mut self) { ORDER.with(|o| o.borrow_mut().push(self.0)); } }
#[inline(never)]
fn inner(n: i32) -> i32 { let _g = G("inner"); if n > 2 { panic!("boom {}", n); } n }
#[inline(never)]
fn outer(n: i32) -> i32 { let _a = G("outer-a"); let v = vec![G("v1"), G("v2")]; let r = inner(n); drop(v); let _b = G("outer-b"); r + 1 }
fn msg(e: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = e.downcast_ref::<String>() { format!("S:{}", s) } else if let Some(s) = e.downcast_ref::<&str>() { format!("s:{}", s) } else if let Some(n) = e.downcast_ref::<i32>() { format!("i:{}", n) } else { "?".into() }
}
fn work() {
    ORDER.with(|o| o.borrow_mut().clear());
    let h = std::panic::take_hook(); std::panic::set_hook(Box::new(|_| {}));
    for n in [1, 2, 3, 9] {
        let r = catch_unwind(|| outer(n));
        d!("{} -> {:?}", n, r.as_ref().map_err(msg));
    }
    d!("{:?}", ORDER.with(|o| o.borrow().clone()));
    let r = catch_unwind(AssertUnwindSafe(|| { let v: Vec<u8> = Vec::new(); v[std::hint::black_box(2)] })); d!("{}", msg(&r.unwrap_err()));
    let r = catch_unwind(|| { let x: Option<i32> = std::hint::black_box(None); x.unwrap() }); d!("{}", msg(&r.unwrap_err()));
    let r = catch_unwind(|| { let r: Result<i32, String> = Err("bad".into()); r.expect("ctx") }); d!("{}", msg(&r.unwrap_err()));
    let r = catch_unwind(|| resume_unwind(Box::new(42i32))); d!("{}", msg(&r.unwrap_err()));
    let r = catch_unwind(|| { let r = catch_unwind(|| panic!("first")); if let Err(e) = r { resume_unwind(e) } }); d!("{}", msg(&r.unwrap_err()));
    let t = std::thread::spawn(|| { let _g = G("thread"); if true { panic!("in thread"); } 5 }).join(); d!("{}", msg(&t.unwrap_err()));
    std::panic::set_hook(h);
}
