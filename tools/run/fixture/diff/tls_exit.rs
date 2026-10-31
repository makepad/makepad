// diff_fast corpus program: `#[test] run` calls work() in rounds (the O3 tier installs between rounds); every
// result line is `D ...`. rustc --test -O and `makepad test` (baseline and O3) must print identical D lines.
// Thread exit: each thread's thread-locals are destroyed after its closure returns (or unwinds). Their Drop
// impls are code the thread never called before, so under Relax they are compiled lazily while the thread's
// host thread-locals are already gone (the compiler must not touch them then, nor the runtime the panic
// goes through). The destructors format, allocate, catch a panic of their own and report into a static the
// joining thread prints.
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Mutex;

static REPORTS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn report(s: String) { REPORTS.lock().unwrap().push(s); }

struct Tally { tag: &'static str, log: Vec<u64> }
impl Drop for Tally {
    fn drop(&mut self) {
        // reached only from the destructor: compiled at thread exit
        let sum: u64 = self.log.iter().map(|x| x.wrapping_mul(2654435761) >> 7).fold(0u64, |a, b| a.rotate_left(5) ^ b);
        let caught = catch_unwind(AssertUnwindSafe(|| { if self.log.len() > 3 { panic!("tally {} too long", self.tag); } self.log.len() }));
        // (the payload's message at thread exit: known/tls_exit_payload.rs)
        let msg = match &caught { Ok(n) => format!("ok:{}", n), Err(_) => "caught".to_string() };
        report(format!("{} n={} sum={:x} {}", self.tag, self.log.len(), sum, msg));
    }
}

struct Names(BTreeMap<String, Vec<String>>);
impl Drop for Names {
    fn drop(&mut self) {
        let joined: Vec<String> = self.0.iter().map(|(k, v)| format!("{}=[{}]", k, v.join(","))).collect();
        report(format!("names {}", joined.join(";")));
    }
}

thread_local! {
    static TALLY: RefCell<Tally> = RefCell::new(Tally { tag: "tally", log: Vec::new() });
    static NAMES: RefCell<Names> = RefCell::new(Names(BTreeMap::new()));
    static GUARD: RefCell<Option<Tally>> = const { RefCell::new(None) };
}

fn work() {
    REPORTS.lock().unwrap().clear();
    let h = std::panic::take_hook(); std::panic::set_hook(Box::new(|_| {}));
    // threads that exit normally after using their thread-locals
    let hs: Vec<_> = (0..3u64).map(|t| std::thread::spawn(move || {
        for i in 0..(t + 2) { TALLY.with(|c| c.borrow_mut().log.push(i * 7 + t)); }
        NAMES.with(|n| { let mut n = n.borrow_mut(); n.0.entry(format!("t{}", t)).or_default().push(format!("{}", t * 11)); });
        t
    })).collect();
    for h in hs { d!("joined {}", h.join().unwrap()); }
    // a thread that panics while its thread-local holds a value with a destructor
    let r = std::thread::spawn(|| {
        GUARD.with(|g| *g.borrow_mut() = Some(Tally { tag: "guard", log: vec![1, 2, 3, 4, 5] }));
        if std::hint::black_box(true) { panic!("in thread {}", 7); }
        0u32
    }).join();
    let e = r.unwrap_err();
    d!("panicked {}", e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default());
    let mut reps = REPORTS.lock().unwrap().clone();
    reps.sort();
    for r in reps { d!("{}", r); }
    std::panic::set_hook(h);
}
