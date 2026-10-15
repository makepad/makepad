// Game-engine pattern: compile/load, then init with Budget::new(2_000_000, 48 MiB).
// Each tick resets ONE Budget to (500_000, 48 MiB) shared by all callback exports.
// Module::with_host lends the GameCtx; concrete host API functions resolve Host<GameCtx>.
// guard(deadline, ...) may wrap this as an additional wall-clock safety net.
use std::relax::{Budget, BudgetError, Host, Module};

pub struct GameCtx { pub count: u32, pub next: fn() -> u32 }
pub fn reenter(h: Host<GameCtx>) -> u32 {
    h.with(|ctx| (ctx.next)()).unwrap_or(0)
}
pub fn busy(h: Host<GameCtx>) -> bool {
    h.with(|_| h.with(|_| ()).is_err()).unwrap_or(false)
}

extern "C" { fn getrusage(who: i32, out: *mut [i64; 18]) -> i32; }
fn peak_rss() -> u64 {
    let mut usage = [0i64; 18];
    assert_eq!(unsafe { getrusage(0, &mut usage) }, 0);
    if cfg!(target_os = "macos") { usage[4] as u64 } else { usage[4] as u64 * 1024 }
}
pub fn spawn(h: Host<GameCtx>, n: u32) -> bool {
    h.with(|ctx| { ctx.count += n; }).is_ok()
}

const CAP: u64 = 48 * 1024 * 1024;
const SOURCE: &str = r#"
use std::relax::Host;
use test_crate::{GameCtx, spawn};
pub fn init() -> u32 { 42 }
pub fn first(n: u64) -> u64 {
    let mut x = 1u64;
    let mut i = 0;
    while i < n { x = x.wrapping_mul(6364136223846793005).wrapping_add(i); i += 1; }
    x
}
pub fn second(n: u64) -> u64 { first(n) + 1 }
pub fn third(n: u64) -> u64 { first(n) + 2 }
pub fn forever() { loop { std::hint::black_box(1); } }
pub fn allocate() {
    let mut keep = Vec::new();
    loop { keep.push(vec![0u8; 1024 * 1024]); }
}
pub fn reuse() { for _ in 0..100 { let v = Vec::<u8>::with_capacity(1024); std::hint::black_box(v); } }
pub fn callback(h: Host<GameCtx>) -> bool { spawn(h, 7) }
pub fn nested(h: Host<GameCtx>) -> u32 { test_crate::reenter(h) }
pub fn busy(h: Host<GameCtx>) -> bool { test_crate::busy(h) }
pub fn text() -> String { String::from("hello 🦀") }
pub fn answer(ok: bool) -> Result<(u32, [u64; 4]), String> {
    if ok { Ok((42, [7; 4])) } else { Err(String::from("game error")) }
}
pub fn panic_now() { panic!("game panic") }
"#;

#[test]
fn budget() {
    let m = Module::load_source("game_budget", "game_budget.rs", SOURCE, &["test_crate"]).unwrap();
    let init: fn() -> u32 = m.get("init").unwrap();
    let first: fn(u64) -> u64 = m.get("first").unwrap();
    let second: fn(u64) -> u64 = m.get("second").unwrap();
    let third: fn(u64) -> u64 = m.get("third").unwrap();
    let forever: fn() = m.get("forever").unwrap();
    let allocate: fn() = m.get("allocate").unwrap();
    let reuse: fn() = m.get("reuse").unwrap();
    let callback: fn(Host<GameCtx>) -> bool = m.get("callback").unwrap();
    let nested: fn(Host<GameCtx>) -> u32 = m.get("nested").unwrap();
    let busy: fn(Host<GameCtx>) -> bool = m.get("busy").unwrap();
    let text: fn() -> String = m.get("text").unwrap();
    let answer: fn(bool) -> Result<(u32, [u64; 4]), String> = m.get("answer").unwrap();
    let panic_now: fn() = m.get("panic_now").unwrap();
    let mut eval = Budget::new(2_000_000, CAP);
    assert_eq!(m.call_with(&mut eval, init).unwrap(), 42);
    let mut tick = Budget::new(500_000, CAP);
    m.call_with(&mut tick, || first(100)).unwrap();
    let a = tick.consumed_fuel();
    m.call_with(&mut tick, || second(100)).unwrap();
    let b = tick.consumed_fuel();
    m.call_with(&mut tick, || third(100)).unwrap();
    let count = tick.consumed_fuel();
    assert!(a > 0 && b > a && count > b && tick.remaining_fuel() == 500_000 - count);
    // Warm each export and wait for tier installation, then compare the same paths.
    for _ in 0..3000 { first(3); second(3); third(3); }
    std::thread::sleep(std::time::Duration::from_millis(800));
    tick.reset(500_000, CAP);
    m.call_with(&mut tick, || { first(100); second(100); third(100); }).unwrap();
    assert_eq!(tick.consumed_fuel(), count);
    println!("out shared tick fuel: {}", count);
    // One less than the measured total fails even though every callback alone fits.
    tick.reset(count - 1, CAP);
    m.call_with(&mut tick, || first(100)).unwrap();
    m.call_with(&mut tick, || second(100)).unwrap();
    assert!(matches!(m.call_with(&mut tick, || third(100)), Err(BudgetError::Fuel { .. })));
    eval.reset(2_000_000, CAP);
    assert!(matches!(m.call_with(&mut eval, forever), Err(BudgetError::Fuel { consumed: 2_000_000 })));
    tick.reset(500_000, CAP);
    assert_eq!(m.call_with(&mut tick, init).unwrap(), 42);
    println!("out fuel recovery: true");
    let mut memory = Budget::new(2_000_000, CAP);
    let rss = peak_rss();
    assert!(matches!(m.call_with(&mut memory, allocate), Err(BudgetError::Alloc { .. })));
    assert!(memory.consumed_alloc_growth() <= CAP && memory.consumed_alloc_growth() > 46 * 1024 * 1024);
    let rss_growth = peak_rss().saturating_sub(rss);
    assert!(rss_growth < 128 * 1024 * 1024);
    eprintln!("allocation RSS growth: {} bytes", rss_growth);
    memory.reset(500_000, CAP);
    assert_eq!(m.call_with(&mut memory, init).unwrap(), 42);
    m.call_with(&mut memory, reuse).unwrap();
    assert!(memory.consumed_alloc_growth() <= 2048);
    println!("out allocation recovery and reuse: true");
    tick.reset(500_000, CAP);
    assert!(matches!(m.call_with(&mut tick, panic_now), Err(BudgetError::Fault(_))));
    assert!(tick.consumed_fuel() > 0);
    tick.reset(500_000, CAP);
    assert_eq!(m.call_with(&mut tick, init).unwrap(), 42);
    let other = Module::load_source("other_game", "other_game.rs", "pub fn callback(h: std::relax::Host<test_crate::GameCtx>) -> bool { test_crate::spawn(h, 7) }", &["test_crate"]).unwrap();
    let foreign: fn(Host<GameCtx>) -> bool = other.get("callback").unwrap();
    let mut ctx = GameCtx { count: 0, next: init };
    let stale = m.with_host(&mut ctx, |h| {
        assert!(m.call_with(&mut tick, || callback(h)).unwrap());
        assert!(!foreign(h));
        assert!(busy(h));
        let before = tick.consumed_fuel();
        assert_eq!(m.call_with(&mut tick, || nested(h)).unwrap(), 42);
        let nested_cost = tick.consumed_fuel() - before;
        assert!(nested_cost > 0);
        let before = tick.consumed_fuel();
        assert_eq!(m.call_with(&mut tick, || nested(h)).unwrap(), 42);
        assert_eq!(tick.consumed_fuel() - before, nested_cost);
        h
    }).unwrap();
    assert_eq!(ctx.count, 7);
    assert!(!callback(stale));
    assert_eq!(ctx.count, 7);
    println!("out host handles: true");
    let saved = m.call_with(&mut tick, text).unwrap();
    assert_eq!(saved, "hello 🦀");
    assert_eq!(m.call_with(&mut tick, || answer(true)).unwrap().unwrap(), (42, [7; 4]));
    assert_eq!(m.call_with(&mut tick, || answer(false)).unwrap().unwrap_err(), "game error");
    // Repeated strings must release arena allocations rather than host-free their pointers.
    for _ in 0..1000 { assert_eq!(text(), "hello 🦀"); }
    let start = std::time::Instant::now();
    for _ in 0..10000 { std::hint::black_box(init()); }
    let plain_ns = start.elapsed().as_nanos() as f64 / 10000.0;
    let start = std::time::Instant::now();
    for _ in 0..10000 { tick.reset(500_000, CAP); std::hint::black_box(m.call_with(&mut tick, init).unwrap()); }
    let budget_ns = start.elapsed().as_nanos() as f64 / 10000.0;
    eprintln!("budget cost: {:.1} ns plain, {:.1} ns budgeted, {:.1} ns extra", plain_ns, budget_ns, budget_ns - plain_ns);
    // An outer watchdog skips the inner call_with continuation; counters still survive.
    tick.reset(u64::MAX, CAP);
    assert!(std::relax::guard(20, || m.call_with(&mut tick, forever)).is_err());
    assert!(tick.consumed_fuel() > 0);
    tick.reset(500_000, CAP);
    assert_eq!(m.call_with(&mut tick, init).unwrap(), 42);
    println!("out watchdog accounting: true");
    other.unload();
    m.unload();
    assert_eq!(saved, "hello 🦀");
    println!("out owned string results: true");
}
