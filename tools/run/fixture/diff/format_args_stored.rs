// diff_fast corpus program: `#[test] run` calls work() in rounds (the O3 tier installs between rounds); every
// result line is `D ...`. rustc --test -O and `makepad test` (baseline and O3) must print identical D lines.
// format_args! keeps its arguments in `super let`s (rustc_ast_lowering format.rs, stable since 1.89): a stored
// `Arguments` holds them for as long as its variable lives, through a block tail, an `if` and a tuple struct
// constructor too; anywhere else they end with the enclosing statement. The log shows when each argument is made,
// shown and dropped. A stored `format_args!("the {}", name())` used to free the String before the read.
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

use std::cell::RefCell;
use std::fmt;
thread_local! { static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) }; }
fn log(s: &str) { LOG.with(|l| l.borrow_mut().push(s.to_string())); }
fn take() -> String { LOG.with(|l| std::mem::take(&mut *l.borrow_mut())).join(",") }
struct Loud(&'static str);
impl Drop for Loud { fn drop(&mut self) { log(&format!("drop {}", self.0)); } }
impl fmt::Display for Loud { fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { f.write_str(self.0) } }
fn loud(s: &'static str) -> Loud { log(&format!("make {s}")); Loud(s) }
fn name() -> String { std::hint::black_box("word").to_string() }
fn show(h: &dyn fmt::Display) -> String { let s = format!("{h}"); log(&format!("show {s}")); s }
fn id<T>(x: T) -> T { x }
struct Hold<'a>(fmt::Arguments<'a>);
fn work() {
    {
        let holds = format_args!("the {}", name());
        d!("eval {}", show(&holds));
        take();
    }
    {
        let a = format_args!("{} and {}", loud("x"), loud("y"));
        log("let");
        show(&a);
        log("end");
    }
    d!("stored {}", take());
    {
        let a = format_args!("{:>4}|{:<3}|{:w$}|{:.prec$}", loud("p"), loud("q"), loud("r"), 1.25f64, w = 3, prec = 1);
        show(&a);
    }
    d!("formatted {}", take());
    {
        let a = format_args!("{1} {0} {1}", loud("m"), loud("n"));
        show(&a);
    }
    d!("reordered {}", take());
    {
        let a = { format_args!("{}", loud("tail")) };
        log("let");
        show(&a);
    }
    d!("block-tail {}", take());
    {
        let c = std::hint::black_box(true);
        let a = if c { format_args!("{}", loud("then")) } else { format_args!("none") };
        show(&a);
    }
    d!("if {}", take());
    {
        let h = Hold(format_args!("{}", loud("ctor")));
        log("let");
        show(&h.0);
    }
    d!("ctor {}", take());
    {
        show(&format_args!("{}", loud("tmp")));
        log("next");
    }
    d!("statement {}", take());
    {
        let s = id(format_args!("{}", loud("arg")).to_string());
        log("let");
        log(&s);
    }
    d!("receiver {}", take());
    {
        match format_args!("{}", loud("scrut")) { a => { show(&a); log("arm"); } }
        log("after");
    }
    d!("scrutinee {}", take());
    {
        let a = format_args!("{} {}", 1, "two");
        show(&a);
        let b = format_args!("lit");
        d!("literals {} {:?} {:?}", show(&b), a.as_str(), b.as_str());
    }
    d!("literals {}", take());
}
