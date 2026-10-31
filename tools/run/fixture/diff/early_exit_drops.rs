// diff_fast corpus program: `#[test] run` calls work() in rounds (the O3 tier installs between rounds); every
// result line is `D ...`. rustc --test -O and `makepad test` (baseline and O3) must print identical D lines.
// Early exits (return, `?`, break, continue, let-else) out of scopes that hold a guard must drop it, also when
// the exit sits inside an if-let / match whose scrutinee borrows through the guard's DerefMut. And `matches!`
// is a match: its scrutinee's temporaries (the guard in `matches!(m.lock().unwrap().phase, ..)`) drop at the
// statement's end. Sandbox's SandboxStore::hosting kept its Mutex locked that way and the next frame's
// `resolved()` deadlocked.
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

use std::cell::RefCell;
use std::ops::{Deref, DerefMut};
use std::sync::Mutex;

thread_local! { static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) }; }
fn log(s: String) { LOG.with(|l| l.borrow_mut().push(s)); }
fn take_log() -> String { LOG.with(|l| std::mem::take(&mut *l.borrow_mut()).join(",")) }

// A guard that logs its drop and derefs (mutably) to its payload, like MutexGuard.
struct G<T> { name: &'static str, v: T }
impl<T> Deref for G<T> { type Target = T; fn deref(&self) -> &T { &self.v } }
impl<T> DerefMut for G<T> { fn deref_mut(&mut self) -> &mut T { &mut self.v } }
impl<T> Drop for G<T> { fn drop(&mut self) { log(format!("drop {}", self.name)); } }
fn g<T>(name: &'static str, v: T) -> G<T> { G { name, v } }

enum Phase { Resolving(Option<u32>), Decided(u32) }
struct State { phase: Phase, generation: u32 }

struct Store { state: Mutex<State> }
impl Store {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> { self.state.lock().unwrap_or_else(|e| e.into_inner()) }
    // Sandbox's shape: let-else return inside an if-let over &mut guard.field.
    #[inline(never)]
    fn resolved(&self) -> bool {
        let mut state = self.lock();
        if let Phase::Resolving(handle) = &mut state.phase {
            let Some(v) = handle.take() else { return false };
            state.phase = Phase::Decided(v);
            state.generation += 1;
        }
        true
    }
    fn arm(&self, v: Option<u32>) { self.lock().phase = Phase::Resolving(v); }
}

#[inline(never)]
fn let_else_in_if_let(x: Option<u32>) -> u32 {
    let mut s = g("s", Some(1u32));
    if let Some(a) = &mut *s {
        let Some(b) = x else { return 0 };
        *a += b;
    }
    s.unwrap_or(9)
}
#[inline(never)]
fn plain_return(n: u32) -> u32 { let s = g("pr", n); if *s > 2 { return 1; } *s + 10 }
#[inline(never)]
fn early_return_in_if_let(n: Option<u32>) -> u32 {
    let mut s = g("ri", n);
    if let Some(v) = &mut *s { if *v > 3 { return 7; } *v += 1; }
    s.unwrap_or(0)
}
#[inline(never)]
fn question(n: Option<u32>) -> Option<u32> { let s = g("q", 5u32); let v = n?; Some(*s + v) }
#[inline(never)]
fn question_in_if_let(n: Option<u32>, m: Result<u32, u32>) -> Result<u32, u32> {
    let mut s = g("qi", n);
    if let Some(v) = &mut *s { let w = m?; *v += w; }
    Ok(s.unwrap_or(0))
}
#[inline(never)]
fn break_continue(xs: &[Option<u32>]) -> u32 {
    let mut acc = 0;
    for x in xs {
        let mut s = g("bc", *x);
        if let Some(v) = &mut *s {
            if *v == 0 { continue; }
            if *v > 50 { break; }
            let Some(w) = v.checked_mul(2) else { continue };
            acc += w;
        }
    }
    acc
}
#[inline(never)]
fn labeled_break(xs: &[Option<u32>]) -> u32 {
    let mut acc = 0;
    'outer: loop {
        for x in xs {
            let mut s = g("lb", *x);
            if let Some(v) = &mut *s {
                let Some(w) = v.checked_sub(1) else { break 'outer };
                acc += w;
            }
        }
        break;
    }
    acc
}
#[inline(never)]
fn let_else_in_match(p: u32, x: Option<u32>) -> u32 {
    let mut s = g("lm", p);
    match &mut *s {
        0 => { let Some(v) = x else { return 100 }; v }
        n if *n < 5 => { *n += 1; let Some(v) = x else { return 200 }; v + *n }
        _ => 300,
    }
}
#[inline(never)]
fn nested_if_let(a: Option<Option<u32>>) -> u32 {
    let mut s = g("ni", a);
    if let Some(inner) = &mut *s {
        if let Some(v) = inner {
            let Some(w) = v.checked_add(1) else { return 1 };
            if w == 3 { return 3; }
            *v = w;
        }
    }
    match *s { Some(Some(v)) => v, Some(None) => 50, None => 60 }
}
#[inline(never)]
fn let_else_two_guards(x: Option<u32>) -> u32 {
    let mut a = g("a", 1u32);
    let mut b = g("b", x);
    if let Some(v) = &mut *b {
        let _c = g("c", ());
        let Some(w) = v.checked_add(*a) else { return 0 };
        *a = w;
    }
    let Some(_) = *b else { return 99 };
    *a
}
#[inline(never)]
fn block_value(x: Option<u32>) -> u32 {
    let s = g("bv", x);
    let r = 'blk: { if let Some(v) = &*s { let Some(w) = v.checked_add(1) else { break 'blk 0 }; break 'blk w; } 5 };
    r + 1
}

struct Locked { m: Mutex<Phase> }
impl Locked {
    fn lock(&self) -> std::sync::MutexGuard<'_, Phase> { self.m.lock().unwrap_or_else(|e| e.into_inner()) }
    #[inline(never)] fn tail(&self) -> bool { matches!(*self.lock(), Phase::Decided(_)) }
    #[inline(never)] fn tail_guard(&self) -> bool { matches!(*self.lock(), Phase::Decided(v) if v > 3) }
    #[inline(never)] fn in_let(&self) -> bool { let r = matches!(*self.lock(), Phase::Resolving(Some(_))); r }
    #[inline(never)] fn in_if(&self) -> u32 { if matches!(*self.lock(), Phase::Decided(_)) { self.free() as u32 + 10 } else { 2 } }
    fn free(&self) -> bool { self.m.try_lock().is_ok() }
}
struct Wrap { phase: Phase }
#[inline(never)]
fn mk(name: &'static str, p: Phase) -> G<Wrap> { g(name, Wrap { phase: p }) }
#[inline(never)]
fn matches_temps(p: u32) -> String {
    let ph = || if p == 0 { Phase::Resolving(None) } else { Phase::Decided(p) };
    let a = matches!(mk("m1", ph()).phase, Phase::Decided(_));
    log(format!("a {a}"));
    let b = matches!(mk("m2", ph()).phase, Phase::Decided(v) if v == 2);
    log(format!("b {b}"));
    let c = matches!(g("m3", Some(g("m3in", p))).take(), Some(x) if *x > 1);
    log(format!("c {c}"));
    let d = if matches!(mk("m4", ph()).phase, Phase::Resolving(_)) { log("then".into()); 1 } else { 0 };
    log(format!("d {d}"));
    take_log()
}

fn work() {
    take_log();
    // Sandbox shape with a real Mutex: the first call returns false and must unlock.
    let store = Store { state: Mutex::new(State { phase: Phase::Resolving(None), generation: 0 }) };
    let a = store.resolved();
    let locked = store.state.try_lock().is_err();
    d!("resolved {} still_locked {}", a, locked);
    // a leaked guard would deadlock the next lock(): stop the Mutex part instead of hanging
    if !locked {
        store.arm(Some(7));
        let b = store.resolved();
        let c = store.resolved();
        let gen = store.lock().generation;
        let decided = matches!(store.lock().phase, Phase::Decided(7));
        d!("resolved {} {} gen {} decided {} locked {}", b, c, gen, decided, store.state.try_lock().is_err());
    } else {
        std::mem::forget(store);
    }

    for x in [None, Some(4)] { let r = let_else_in_if_let(x); d!("lei {:?} {} [{}]", x, r, take_log()); }
    for n in [1, 5] { let r = plain_return(n); d!("pr {} {} [{}]", n, r, take_log()); }
    for n in [None, Some(2), Some(9)] { let r = early_return_in_if_let(n); d!("ri {:?} {} [{}]", n, r, take_log()); }
    for n in [None, Some(3)] { let r = question(n); d!("q {:?} {:?} [{}]", n, r, take_log()); }
    for (n, m) in [(None, Err(1)), (Some(1), Err(2)), (Some(1), Ok(4))] { let r = question_in_if_let(n, m); d!("qi {:?} {:?} {:?} [{}]", n, m, r, take_log()); }
    let xs = [Some(1), Some(0), None, Some(u32::MAX), Some(3), Some(60), Some(2)];
    let r = break_continue(&xs); d!("bc {} [{}]", r, take_log());
    let r = labeled_break(&[Some(3), None, Some(0), Some(5)]); d!("lb {} [{}]", r, take_log());
    for (p, x) in [(0, None), (0, Some(4)), (2, None), (3, Some(1)), (9, None)] { let r = let_else_in_match(p, x); d!("lm {} {:?} {} [{}]", p, x, r, take_log()); }
    for a in [None, Some(None), Some(Some(u32::MAX)), Some(Some(2)), Some(Some(5))] { let r = nested_if_let(a); d!("ni {:?} {} [{}]", a, r, take_log()); }
    for x in [None, Some(u32::MAX), Some(4)] { let r = let_else_two_guards(x); d!("tg {:?} {} [{}]", x, r, take_log()); }
    for p in [Phase::Resolving(Some(1)), Phase::Decided(2), Phase::Decided(5)] {
        let l = Locked { m: Mutex::new(p) };
        let mut out = Vec::new();
        for k in 0..4 {
            if !l.free() { out.push(format!("{k}: leaked")); break; }
            let r = match k { 0 => l.tail() as u32, 1 => l.tail_guard() as u32, 2 => l.in_let() as u32, _ => l.in_if() };
            out.push(format!("{k}: {r} free {}", l.free()));
        }
        d!("locked {}", out.join("; "));
        if !l.free() { std::mem::forget(l); }
    }
    for p in [0, 2, 3] { d!("mt {} [{}]", p, matches_temps(p)); }
    for x in [None, Some(u32::MAX), Some(4)] { let r = block_value(x); d!("bv {:?} {} [{}]", x, r, take_log()); }
}
