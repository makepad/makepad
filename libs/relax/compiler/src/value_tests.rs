//! Values the JIT computes at run time are rustc's: each case runs a small program's function
//! (writing its results to `out`) and compares them with what rustc's build gives.
//!
//! `cfg!` is the literal the crate's cfg gives (rustc_builtin_macros cfg.rs expand_cfg) wherever
//! it is read: in run-time code, and in the consts the type layer evaluates for it (an array
//! length, a const generic argument, a constant pattern). a crate's
//! `const _: () = assert!(cfg!(target_endian = "little"))` failed `makepad check` (E0080) on
//! x86_64: the check-mode interpreter runs MIR built by the type layer, which had made every
//! `cfg!` false (check_lint_tests covers that path). The run-time lowering reads the crate's cfg
//! itself; this pins that it and the type layer agree. A release `debug_assert!` in a const is
//! skipped as rustc's `if cfg!(debug_assertions)` is. Under a cargo profile with both checks
//! (`makepad test`'s `test` profile, relax issue 141) the crate's `cfg!(debug_assertions)` holds
//! in run-time code and consts, its `debug_assert!` runs and its arithmetic's overflow panics, at
//! the first tier and at O3.

const CFG: &str = r#"
pub const WIDTH: usize = if cfg!(target_pointer_width = "64") { 64 } else { 32 };
const _: () = assert!(cfg!(target_endian = "little"));
pub struct Lanes<const N: usize>;
impl<const N: usize> Lanes<N> { pub fn n(&self) -> usize { N } }
pub fn values(out: &mut [usize; 9]) {
    out[0] = cfg!(target_endian = "little") as usize;
    out[1] = cfg!(target_pointer_width = "64") as usize;
    out[2] = if cfg!(target_arch = "x86_64") { 1 } else if cfg!(target_arch = "aarch64") { 2 } else { 0 };
    out[3] = WIDTH;
    out[4] = core::mem::size_of::<[u8; WIDTH]>();
    out[5] = Lanes::<{ if cfg!(target_endian = "little") { 4 } else { 8 } }>.n();
    out[6] = [0u8; { debug_assert!(1 == 2); 7 }].len();
    out[7] = Lanes::<WIDTH>.n();
    out[8] = match core::mem::size_of::<usize>() * 8 { WIDTH => 1, _ => 0 };
}
"#;

/// The test profile's checks (cargo's dev profile): a `makepad test` crate's without a manifest setting.
const CHECKED: crate::resolve::ProfileChecks = crate::resolve::ProfileChecks { debug_assertions: true, overflow_checks: true };

/// The release profile's: none (a run's crates, `makepad test --release`).
const RELEASE: crate::resolve::ProfileChecks = crate::resolve::ProfileChecks { debug_assertions: false, overflow_checks: false };

/// The checks' values: `cfg!(debug_assertions)` at run time and in a const, a failing
/// `debug_assert!`'s panic, the overflow panics of an add, a shift and a negation, and a sum that
/// does not overflow. Then std's code that inherits the program's checks (rustc's
/// `#[rustc_inherit_overflow_checks]`, `intrinsics::overflow_checks`): a `RangeFrom` stepping
/// past its type's maximum, which panics with the add's message under checks and wraps without,
/// and `core::range::RangeFrom`'s iterator, which yields the maximum first.
const CHECKS: &str = r#"
pub const ON: usize = if cfg!(debug_assertions) { 1 } else { 0 };
fn add(a: u8, b: u8) -> u8 { a + b }
fn shl(a: u32, b: u32) -> u32 { a << b }
fn neg(a: i32) -> i32 { -a }
fn message(e: Box<dyn std::any::Any + Send>) -> usize { (e.downcast_ref::<&str>() == Some(&"attempt to add with overflow")) as usize }
pub fn values(out: &mut [usize; 11]) {
    out[0] = cfg!(debug_assertions) as usize;
    out[1] = ON;
    out[2] = std::panic::catch_unwind(|| debug_assert!(std::hint::black_box(1) == 2)).is_err() as usize;
    out[3] = std::panic::catch_unwind(|| add(std::hint::black_box(255), 1)).is_err() as usize;
    out[4] = std::panic::catch_unwind(|| shl(1, std::hint::black_box(32))).is_err() as usize;
    out[5] = std::panic::catch_unwind(|| neg(std::hint::black_box(i32::MIN))).is_err() as usize;
    out[6] = add(std::hint::black_box(250), 5) as usize;
    let past = std::panic::catch_unwind(|| { let mut r = std::hint::black_box(255u8)..; r.next(); r.start as usize });
    out[7] = past.as_ref().map_or(999, |s| *s);
    out[8] = past.err().map_or(0, message);
    let mut it = core::range::RangeFrom::from(std::hint::black_box(254u8)..).into_iter();
    out[9] = it.next().unwrap() as usize * 1000 + it.next().unwrap() as usize;
    out[10] = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| it.next().unwrap() as usize)).unwrap_or(999);
}
"#;

/// Runs `pub fn values(out: &mut [usize; N])` of the program `src` and returns `out`.
fn values<const N: usize>(src: &str, name: &str) -> [usize; N] {
    let (mut u, krate) = unit(src, name, RELEASE);
    run(&mut u, krate)
}

/// `values` at the first tier, then again once every fn of the program that ran was rebuilt at
/// O3 (inlined into its callers, optimized); and the fns whose O3 build was refused. `checks`:
/// the cargo profile's the program's crate builds with.
fn values_at_o3<const N: usize>(src: &str, name: &str, checks: crate::resolve::ProfileChecks) -> ([usize; N], [usize; N], Vec<String>) {
    // O3 builds wait for no borrow check verdict
    crate::borrowck::lazy::disable_gandalf();
    let (mut u, krate) = unit(src, name, checks);
    let first = run(&mut u, krate);
    assert!(u.tier1.is_some(), "the fixture needs O3");
    let up = &mut *u as *mut crate::jit::Unit;
    let ran: Vec<u32> = (0..u.fns.len() as u32).filter(|&id| u.fns[id as usize].code != 0 && match &u.fns[id as usize].key {
        crate::jit::FnKey::Inst(d, args) => args.is_empty() && u.prog.def(*d).krate == krate,
        _ => false,
    }).collect();
    let refused: Vec<String> = ran.into_iter().filter(|&id| !unsafe { crate::jit::compile_tier1(up, id) }).map(|id| u.fns[id as usize].name.to_string()).collect();
    (first, run(&mut u, krate), refused)
}

fn unit(src: &str, name: &str, checks: crate::resolve::ProfileChecks) -> (Box<crate::jit::Unit>, u32) {
    unit_in(crate::Session::default(), src, name, checks)
}

fn unit_in(session: crate::Session, src: &str, name: &str, checks: crate::resolve::ProfileChecks) -> (Box<crate::jit::Unit>, u32) {
    crate::jit::CHECK_FIRST.store(false, std::sync::atomic::Ordering::Relaxed);
    std::env::set_var("RELAX_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-values-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("lib.rs");
    std::fs::write(&path, src).unwrap();
    let (prog, core, krate) = crate::load_with_std_checks(&std::sync::Arc::new(session), path.to_str().unwrap(), name, false, checks).unwrap();
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(dir).unwrap();
    let cfg = &prog.crates[krate as usize].cfg;
    assert_eq!((cfg.holds("debug_assertions"), cfg.holds("overflow_checks")), (checks.debug_assertions, checks.overflow_checks), "the crate's cfg is its profile's");
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    (crate::jit::Unit::new(prog, tcx, core), krate)
}

fn run<const N: usize>(u: &mut crate::jit::Unit, krate: u32) -> [usize; N] {
    let up = &mut *u as *mut crate::jit::Unit;
    let def = (0..u.prog.defs.len()).map(|i| crate::program::DefId(i as u32)).find(|&d| u.prog.def(d).krate == krate && u.prog.name(d) == "values").unwrap();
    let entry = crate::owner::call(crate::owner::Class::Program, || { let f = u.fn_id(crate::jit::FnKey::Inst(def, vec![])); u.entry(f) });
    let mut out = [0usize; N];
    unsafe { crate::jit::call_jit(up, entry, &mut out as *mut [usize; N] as u64) }.map_err(|p| p.message).unwrap();
    out
}

#[test]
fn cfg_macro_is_the_crates_cfg_at_run_time_and_in_consts() {
    isolated_test!(cfg_macro_is_the_crates_cfg_at_run_time_and_in_consts);
    let out: [usize; 9] = values(CFG, "cfg_values");
    let width = if cfg!(target_pointer_width = "64") { 64 } else { 32 };
    let arch = if cfg!(target_arch = "x86_64") { 1 } else if cfg!(target_arch = "aarch64") { 2 } else { 0 };
    assert_eq!(out, [cfg!(target_endian = "little") as usize, cfg!(target_pointer_width = "64") as usize, arch, width, width, 4, 7, width, 1]);
    let (first, o3, refused): ([usize; 11], [usize; 11], _) = values_at_o3(CHECKS, "cfg_checked", CHECKED);
    assert_eq!(first, [1, 1, 1, 1, 1, 1, 255, 999, 1, 254_255, 999], "the test profile's checks at the first tier");
    assert_eq!(o3, first, "the test profile's checks at O3 (not built: {refused:?})");
    let (first, o3, _): ([usize; 11], [usize; 11], _) = values_at_o3(CHECKS, "cfg_released", RELEASE);
    let released = [0, 0, 0, 0, 0, 0, 255, 0, 0, 254_255, 0];
    assert_eq!((first, o3), (released, released), "the release profile's: no check, at the first tier and at O3");
}

/// Products of integers widened to 128 bits, under the test profile's overflow checks: signed
/// values widened by their sign into i128 and unsigned ones by zero into u128 (one widening
/// multiply, never an overflow), narrower ones, and the products that stay general: a signed by
/// an unsigned value, and unsigned values widened into i128, whose product can overflow.
const WIDENED: &str = r#"
use std::hint::black_box;
fn halves(v: i128, out: &mut [usize; 13], k: usize) { out[k] = v as u64 as usize; out[k + 1] = (v >> 64) as u64 as usize; }
fn signed(a: i64, b: i64) -> i128 { a as i128 * b as i128 }
fn narrow(a: i32, b: i8) -> i128 { (a as i128) * (b as i128) }
fn unsigned(a: u64, b: u64) -> u128 { a as u128 * b as u128 }
fn mixed(a: i64, b: u64) -> i128 { a as i128 * b as i128 }
fn unsigned_into_signed(a: u64, b: u64) -> i128 { a as i128 * b as i128 }
pub fn values(out: &mut [usize; 13]) {
    halves(signed(black_box(i64::MIN), black_box(i64::MIN)), out, 0);
    halves(signed(black_box(i64::MIN), black_box(i64::MAX)), out, 2);
    halves(narrow(black_box(-3), black_box(-128)), out, 4);
    halves(unsigned(black_box(u64::MAX), black_box(u64::MAX)) as i128, out, 6);
    halves(mixed(black_box(i64::MIN), black_box(u64::MAX)), out, 8);
    halves(unsigned_into_signed(black_box(1 << 63), black_box(3)), out, 10);
    out[12] = std::panic::catch_unwind(|| unsigned_into_signed(black_box(u64::MAX), black_box(u64::MAX))).is_err() as usize;
}
"#;

#[test]
fn widened_128_bit_products_are_exact_and_overflow_only_where_they_can() {
    isolated_test!(widened_128_bit_products_are_exact_and_overflow_only_where_they_can);
    use std::hint::black_box;
    let halves = |v: i128| [v as u64 as usize, (v >> 64) as u64 as usize];
    let mut want = [0usize; 13];
    for (k, v) in [
        black_box(i64::MIN) as i128 * black_box(i64::MIN) as i128,
        black_box(i64::MIN) as i128 * black_box(i64::MAX) as i128,
        black_box(-3i32) as i128 * black_box(-128i8) as i128,
        (black_box(u64::MAX) as u128 * black_box(u64::MAX) as u128) as i128,
        black_box(i64::MIN) as i128 * black_box(u64::MAX) as i128,
        black_box(1u64 << 63) as i128 * black_box(3u64) as i128,
    ].into_iter().enumerate() {
        want[2 * k..2 * k + 2].copy_from_slice(&halves(v));
    }
    want[12] = 1;
    let (first, o3, refused): ([usize; 13], [usize; 13], _) = values_at_o3(WIDENED, "widened_products", CHECKED);
    assert_eq!(first, want, "the products and the one overflow at the first tier");
    assert_eq!(o3, want, "the products and the one overflow at O3 (not built: {refused:?})");
}

/// 128-bit products of a full value and a factor of one word (a widened i64 or u64 either side, a
/// literal), and 128-bit quotients and remainders, signed and unsigned, under the test profile's
/// checks: each value, or (third word 1) the overflow or zero-divisor panic. The values are paired
/// with the cases' constants in the test (rustc's checked_mul / checked_div / checked_rem).
const BY_WORD: &str = r#"
use std::hint::black_box;
const XS: [i128; 10] = [0, 1, -1, 0xffff_ffff_ffff_ffff, 1 << 64, -(1 << 64), 1 << 126, -(1 << 126), i128::MAX, i128::MIN];
const FS: [i64; 7] = [0, 1, -1, 2, -2, i64::MAX, i64::MIN];
const XU: [u128; 6] = [0, 1, 0xffff_ffff_ffff_ffff, 1 << 64, u128::MAX / 3, u128::MAX];
const FU: [u64; 5] = [0, 1, 2, 3, u64::MAX];
const DS: [i128; 6] = [3, -7, -1, 0, 1 << 70, -(1 << 65) + 1];
const DU: [u128; 3] = [3, 0, 1 << 70];
fn by(x: i128, f: i64) -> i128 { x * f as i128 }
fn by_rev(x: i128, f: i64) -> i128 { (f as i128) * x }
fn two(x: i128) -> i128 { 2 * x }
fn minus_two(x: i128) -> i128 { x * -2 }
fn uby(x: u128, f: u64) -> u128 { x * f as u128 }
fn three(x: u128) -> u128 { 3 * x }
fn put(out: &mut [usize; 1056], k: &mut usize, r: Result<u128, Box<dyn std::any::Any + Send>>) {
    match r {
        Ok(v) => { out[*k] = v as u64 as usize; out[*k + 1] = (v >> 64) as u64 as usize; }
        Err(_) => out[*k + 2] = 1,
    }
    *k += 3;
}
pub fn values(out: &mut [usize; 1056]) {
    use std::panic::catch_unwind as c;
    let k = &mut 0;
    for x in XS {
        for f in FS {
            put(out, k, c(|| by(black_box(x), black_box(f)) as u128));
            put(out, k, c(|| by_rev(black_box(x), black_box(f)) as u128));
        }
        put(out, k, c(|| two(black_box(x)) as u128));
        put(out, k, c(|| minus_two(black_box(x)) as u128));
    }
    for x in XU {
        for f in FU { put(out, k, c(|| uby(black_box(x), black_box(f)))); }
        put(out, k, c(|| three(black_box(x))));
    }
    for x in XS {
        for d in DS {
            put(out, k, c(|| (black_box(x) / black_box(d)) as u128));
            put(out, k, c(|| (black_box(x) % black_box(d)) as u128));
        }
    }
    for x in XU {
        for d in DU {
            put(out, k, c(|| black_box(x) / black_box(d)));
            put(out, k, c(|| black_box(x) % black_box(d)));
        }
    }
}
"#;

#[test]
fn a_128_bit_value_by_one_word_and_its_quotients_are_rustcs() {
    isolated_test!(a_128_bit_value_by_one_word_and_its_quotients_are_rustcs);
    const XS: [i128; 10] = [0, 1, -1, 0xffff_ffff_ffff_ffff, 1 << 64, -(1 << 64), 1 << 126, -(1 << 126), i128::MAX, i128::MIN];
    const FS: [i64; 7] = [0, 1, -1, 2, -2, i64::MAX, i64::MIN];
    const XU: [u128; 6] = [0, 1, 0xffff_ffff_ffff_ffff, 1 << 64, u128::MAX / 3, u128::MAX];
    const FU: [u64; 5] = [0, 1, 2, 3, u64::MAX];
    const DS: [i128; 6] = [3, -7, -1, 0, 1 << 70, -(1 << 65) + 1];
    const DU: [u128; 3] = [3, 0, 1 << 70];
    let mut want = Vec::new();
    let mut put = |r: Option<u128>| want.extend(match r { Some(v) => [v as u64 as usize, (v >> 64) as u64 as usize, 0], None => [0, 0, 1] });
    for x in XS {
        for f in FS {
            put(x.checked_mul(f as i128).map(|v| v as u128));
            put((f as i128).checked_mul(x).map(|v| v as u128));
        }
        put(2i128.checked_mul(x).map(|v| v as u128));
        put(x.checked_mul(-2).map(|v| v as u128));
    }
    for x in XU {
        for f in FU { put(x.checked_mul(f as u128)); }
        put(3u128.checked_mul(x));
    }
    for x in XS {
        for d in DS {
            put(x.checked_div(d).map(|v| v as u128));
            put(x.checked_rem(d).map(|v| v as u128));
        }
    }
    for x in XU {
        for d in DU {
            put(x.checked_div(d));
            put(x.checked_rem(d));
        }
    }
    assert_eq!(want.len(), 1056);
    let (first, o3, refused): ([usize; 1056], [usize; 1056], _) = values_at_o3(BY_WORD, "by_word", CHECKED);
    // the first case that differs: (case, the JIT's value lo / hi / panicked, rustc's)
    let differs = |out: &[usize; 1056]| (0..352).find(|&i| out[3 * i..3 * i + 3] != want[3 * i..3 * i + 3]).map(|i| (i, out[3 * i..3 * i + 3].to_vec(), want[3 * i..3 * i + 3].to_vec()));
    assert_eq!(differs(&first), None, "at the first tier");
    assert_eq!(differs(&o3), None, "at O3 (not built: {refused:?})");
}

/// `IoSlice` / `IoSliceMut` (now real std's `iovec`-shaped wrappers, WARNZERO class 8): the
/// slices they borrow, `advance_slices` across them, a vectored write, writes through a mutable
/// one, and the layout FFI code relies on: unix's `iovec` (base, then length), Windows' `WSABUF`
/// (a 32-bit length, then base), as rustc's std has them.
const IO_SLICES: &str = r#"
use std::io::{IoSlice, IoSliceMut, Write};
pub fn values(out: &mut [usize; 8]) {
    let (a, b) = ([1u8, 2, 3], [4u8, 5]);
    let mut bufs = [IoSlice::new(&a), IoSlice::new(&b)];
    out[0] = bufs[0].len() + bufs[1].len() * 10;
    let mut rest = &mut bufs[..];
    IoSlice::advance_slices(&mut rest, 4);
    out[1] = rest.len() * 10 + rest[0][0] as usize;
    let mut v = Vec::new();
    out[2] = v.write_vectored(&[IoSlice::new(&a), IoSlice::new(&b)]).unwrap();
    out[3] = v.iter().map(|&x| x as usize).sum();
    let mut m = [0u8; 4];
    {
        let mut s = IoSliceMut::new(&mut m);
        s.advance(1);
        s[0] = 9;
        out[4] = s.len();
    }
    out[5] = m[1] as usize;
    #[cfg(not(windows))] #[repr(C)] struct Raw { base: *const u8, len: usize }
    #[cfg(windows)] #[repr(C)] struct Raw { len: u32, base: *const u8 }
    let raw: Raw = unsafe { core::mem::transmute(IoSlice::new(&b)) };
    out[6] = (raw.base == b.as_ptr()) as usize;
    out[7] = raw.len as usize;
}
"#;

#[test]
fn io_slices_borrow_their_buffers_as_an_iovec() {
    isolated_test!(io_slices_borrow_their_buffers_as_an_iovec);
    let out: [usize; 8] = values(IO_SLICES, "io_slices");
    assert_eq!(out, [23, 15, 5, 15, 3, 9, 1, 2]);
}

/// Guards bound out of tuples are dropped where rustc drops them, at the first tier and at O3
/// (makepad-skyline's band.rs: `let (cx, rig, hal, info, _one) = open_rig(..)` with the
/// `MutexGuard` that keeps its GPU tests one at a time; its next `open_rig` waited forever, under
/// the JIT and natively alike). The log is the drops (a `D`'s number) and whether the guard is
/// held (501) or free (500) at each point, as rustc's build of this program prints it: tuple
/// returns destructured, tuple temporaries with `_` parts, a match on a tuple, an early return, a
/// by-ref destructure, a parameter pattern, band.rs's two rig loops, and unwinds that drop (and
/// poison) the guard.
const TUPLE_GUARDS: &str = r#"
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard};
static ONE: Mutex<()> = Mutex::new(());
static mut LOG: [usize; 80] = [0; 80];
static mut AT: usize = 0;
fn log(v: usize) { unsafe { if AT < 80 { LOG[AT] = v; AT += 1; } } }
/// whether the guard is free (poisoned or not: a test that failed holding it poisons it)
fn free() -> bool { !matches!(ONE.try_lock(), Err(std::sync::TryLockError::WouldBlock)) }
fn lock() -> MutexGuard<'static, ()> { ONE.lock().unwrap_or_else(|e| e.into_inner()) }
/// 501: the guard is held here, 500: free
fn held() { log(500 + !free() as usize); }
struct D(usize);
impl Drop for D { fn drop(&mut self) { log(self.0); } }
#[derive(Clone, Copy)]
pub struct Info { pub session: u32, pub backend: u64 }
/// skyline band.rs's open_rig: a guard returned in a tuple with the session's handles
fn open_rig(follow: bool) -> (Rc<RefCell<u32>>, Rc<RefCell<Option<D>>>, &'static Mutex<()>, Info, MutexGuard<'static, ()>) {
    let one = lock();
    let rig: Rc<RefCell<Option<D>>> = Rc::new(RefCell::new(None));
    let handler = rig.clone();
    *handler.borrow_mut() = Some(D(if follow { 21 } else { 22 }));
    (Rc::new(RefCell::new(3)), rig, &ONE, Info { session: 7, backend: 1 }, one)
}
fn render_band(follow: bool) -> usize {
    let (cx, rig, m, info, _one) = open_rig(follow);
    held();
    let mut round = 0;
    let band = loop {
        round += 1;
        let c = cx.borrow_mut();
        let r = rig.borrow_mut();
        if round == 2 { break *c as usize + r.is_some() as usize + info.session as usize + info.backend as usize; }
    };
    let _ = m;
    band
}
fn early(k: usize) -> Option<usize> {
    let (_c, _r, _m, info, _one) = open_rig(true);
    if k == 1 { return None; }
    Some(info.session as usize)
}
fn differs(a: &Vec<usize>, b: &Vec<usize>) -> Option<&'static str> { if a != b { Some("colour") } else { None } }
/// band.rs's render_band_lit: the band settles over rounds (a failed one, `continue` from a
/// let-else that assigns a tuple; a labelled break out of nested loops; `break` from a match)
fn render_lit(k: usize) -> Vec<usize> {
    held();
    if !free() { return Vec::new(); }
    let (cx, rig, _hal, info, _one) = open_rig(true);
    let mut last: Option<Vec<usize>> = None;
    let (mut round, mut differed, start) = (0, String::new(), 5usize);
    let band = loop {
        round += 1;
        assert!(round == 1 || start < 100, "never settled ({} rounds): {differed}", round - 1);
        let (tx, rx) = std::sync::mpsc::channel();
        {
            let mut c = cx.borrow_mut();
            let r = rig.borrow_mut();
            *c += r.is_some() as u32;
            let _ = tx.send(if round == 1 { Err(()) } else { Ok(vec![k, info.session as usize]) });
        }
        let got = 'job: loop {
            loop { let c = cx.borrow_mut(); if *c > 0 { break; } }
            if let Ok(b) = rx.try_recv() { break 'job b; }
            *cx.borrow_mut() = 1;
        };
        let Ok(b) = got else { (last, differed) = (None, "no band".into()); continue };
        match last.as_ref().map(|l| differs(l, &b)) {
            Some(None) => break b,
            Some(Some(plane)) => differed = format!("differed in {plane}"),
            None => differed = "none before".into(),
        }
        last = Some(b);
    };
    let _ = info.backend;
    band
}
/// band.rs's bands_from_one_draw: a closure borrowing the Cx, a warm-up loop, boxed senders
fn bands(requests: &[(usize, usize)]) -> Vec<Option<(Vec<usize>, f64, usize)>> {
    held();
    if !free() { return Vec::new(); }
    let (cx, rig, _hal, info, _one) = open_rig(false);
    let turn = || assert!(*cx.borrow_mut() < 1000);
    let mut out = vec![];
    let (mut warm, mut tries) = (false, 0);
    while !warm && tries < 10 {
        tries += 1;
        let (tx, rx) = std::sync::mpsc::channel::<Result<Vec<usize>, ()>>();
        {
            let mut c = cx.borrow_mut();
            *c += 1;
            let f: Box<dyn FnOnce(Result<Vec<usize>, ()>)> = Box::new(move |b| { let _ = tx.send(b); });
            f(if tries < 2 { Err(()) } else { Ok(vec![1]) });
        }
        loop {
            turn();
            if let Ok(b) = rx.try_recv() { warm = b.is_ok(); break; }
            if tries > 100 { break; }
        }
        while rig.borrow().is_none() && tries < 5 { turn(); }
    }
    for (p, w) in requests {
        let (tx, rx) = std::sync::mpsc::channel::<Result<Vec<usize>, ()>>();
        {
            let c = cx.borrow_mut();
            let f: Box<dyn FnOnce(Result<Vec<usize>, ()>)> = Box::new(move |b| { let _ = tx.send(b); });
            f(Ok(vec![*p, *c as usize]));
        }
        let (t0, draws) = (1.5f64, rig.borrow().as_ref().map_or(0, |d| d.0));
        let got = loop {
            turn();
            if let Ok(b) = rx.try_recv() { break b.ok().map(|b| (b, t0 * 2.0)); }
            break None;
        };
        let drew = rig.borrow().as_ref().map_or(0, |d| d.0) - draws + *w;
        let _s = format!("band {}: {}", w, got.as_ref().map_or("none".into(), |(_, ms)| format!("{ms:.1} ms")));
        out.push(got.map(|(b, ms)| (b, ms, drew)));
    }
    let _ = info.session;
    out
}
/// a band test that fails holding the guard: its unwind drops (and poisons) it, in a closure,
/// in a labelled loop, in a callee (a RefCell borrowed twice)
fn fails(k: usize) -> usize {
    let (cx, rig, _m, info, _one) = open_rig(k == 1);
    let turn = || assert!(*cx.borrow_mut() < if k == 1 { 3 } else { 100 }, "turn failed");
    let mut round = 0;
    let band = loop {
        round += 1;
        let got = 'job: loop {
            loop { turn(); *cx.borrow_mut() += 1; if round > 0 { break; } }
            if k == 2 { assert!(round < 1, "band timed out"); }
            if k == 3 { let _a = rig.borrow_mut(); let _b = rig.borrow_mut(); }
            break 'job round;
        };
        if got > 5 { break got; }
    };
    band + info.session as usize
}
fn take((g, d): (MutexGuard<'static, ()>, D)) -> usize { held(); let _ = &g; d.0 }
pub fn values(out: &mut [usize; 80]) {
    unsafe { AT = 0; }
    log(render_band(true));
    held();
    if free() { log(render_band(false)); } else { log(999); }
    held();
    log(600);
    {
        let (g, x) = (lock(), D(31));
        held();
        log(x.0 + 1);
        let _ = &g;
    }
    held();
    log(601);
    {
        let (_, x) = (D(71), 7);
        held();
        log(x);
    }
    held();
    log(602);
    {
        let (_g, _) = (lock(), D(41));
        held();
    }
    held();
    log(603);
    match (lock(), D(51)) {
        (_g, ref d) if d.0 == 51 => held(),
        _ => log(998),
    }
    held();
    log(604);
    if free() { log(early(1).is_none() as usize); } else { log(997); }
    held();
    if free() { log(early(2).unwrap_or(0)); } else { log(996); }
    held();
    log(605);
    if free() {
        let t = open_rig(false);
        let (_, ref r, .., ref g) = t;
        held();
        log(r.borrow().as_ref().map_or(0, |d| d.0) + 1);
        let _ = g;
    }
    held();
    log(606);
    if free() { log(take((lock(), D(61)))); } else { log(995); }
    held();
    log(607);
    let (own, floor) = (render_lit(81), render_lit(82));
    log(own.first().copied().unwrap_or(994));
    log(floor.first().copied().unwrap_or(993));
    held();
    let wet = render_lit(83);
    log(wet.len());
    held();
    log(608);
    let got = bands(&[(5, 1), (6, 2)]);
    log(got.len());
    log(got.iter().filter(|g| g.is_some()).count());
    held();
    log(render_lit(91).len());
    held();
    log(609);
    for k in 1..4 {
        if free() { log(std::panic::catch_unwind(|| fails(k)).is_err() as usize); } else { log(992); }
        held();
    }
    log(matches!(ONE.try_lock(), Err(std::sync::TryLockError::Poisoned(_))) as usize);
    unsafe { for i in 0..AT { out[i] = LOG[i]; } }
}
"#;

#[test]
fn guards_bound_out_of_tuples_drop_where_rustc_drops_them() {
    isolated_test!(guards_bound_out_of_tuples_drop_where_rustc_drops_them);
    let (first, o3, refused): ([usize; 80], [usize; 80], _) = values_at_o3(TUPLE_GUARDS, "tuple_guards", RELEASE);
    // (the logging leaves stay at their first tier)
    assert!(refused.iter().all(|f| f == "tuple_guards::log" || f.ends_with("<D as Drop>::drop")), "no O3 build of {refused:?}");
    let (n, m) = (first.iter().rposition(|&v| v != 0).map_or(0, |i| i + 1), o3.iter().rposition(|&v| v != 0).map_or(0, |i| i + 1));
    // what rustc's build of the program prints (-O, panic=unwind)
    let rustc: &[usize] = &[501, 21, 12, 500, 501, 22, 12, 500, 600, 501, 32, 31, 500, 601, 71, 500, 7, 500, 602, 41, 501, 500, 603, 501, 51, 500, 604, 21, 1, 500, 21, 7, 500, 605, 501, 23, 22, 500, 606, 501, 61, 61, 500, 607, 500, 21, 500, 21, 81, 82, 500, 500, 21, 2, 500, 608, 500, 22, 2, 2, 500, 500, 21, 2, 500, 609, 21, 1, 500, 22, 1, 500, 22, 1, 500, 1];
    assert_eq!(&first[..n], rustc, "first tier");
    assert_eq!(&o3[..m], rustc, "O3");
}

#[path = "../../base/tests/support/vfs_store.rs"]
pub(crate) mod vfs_store;

/// A game's store (asset client cache.rs fsync_dir: `File::open(dir)?.sync_all()`): a folder
/// opens for reading as it does on the disk (open(2) of a directory O_RDONLY succeeds; its reads
/// fail EISDIR), so a virtual one (an overlay run's sources) is fsync'd, not refused EISDIR. On
/// Windows rustc's std opens no folder (no backup semantics: access denied), and the virtual
/// one is refused as the disk's is.
#[test]
fn a_virtual_folder_opens_for_reading_as_a_disk_folder_does() {
    isolated_test!(a_virtual_folder_opens_for_reading_as_a_disk_folder_does);
    let fixture = vfs_store::Fixture::new(&[("app/cache/a.bin", b"a")]);
    let root = "/relax-values-vfs-dir/vfs/m";
    crate::vfs::mount(root, fixture.store.clone(), &fixture.commit).unwrap();
    let disk = std::env::temp_dir().join(format!("relax-values-disk-dir-{}", std::process::id()));
    std::fs::create_dir_all(&disk).unwrap();
    let (disk_dir, virtual_dir) = (disk.to_str().unwrap().to_string(), format!("{root}/app/cache"));
    let src = format!(r#"
pub fn values(out: &mut [usize; 12]) {{
    for (k, path) in [{disk_dir:?}, {virtual_dir:?}].into_iter().enumerate() {{
        let out = &mut out[k * 6..k * 6 + 6];
        let dir = std::fs::File::open(path);
        out[0] = dir.is_ok() as usize;
        out[5] = dir.as_ref().err().map_or(0, |e| e.raw_os_error().map_or(usize::MAX, |c| c as usize));
        if let Ok(mut d) = dir {{
            out[1] = d.sync_all().is_ok() as usize;
            out[2] = d.metadata().map_or(0, |m| m.is_dir() as usize);
            let mut buf = [0u8; 4];
            out[3] = matches!(std::io::Read::read(&mut d, &mut buf), Err(e) if e.kind() == std::io::ErrorKind::IsADirectory) as usize;
        }}
        // a write still needs the edits folder
        out[4] = std::fs::OpenOptions::new().write(true).open(path).is_err() as usize;
    }}
}}
"#);
    let got = values::<12>(&src, "vfs_dir");
    std::fs::remove_dir_all(&disk).unwrap();
    assert_eq!(&got[6..], &got[..6], "the virtual folder as the disk folder: {got:?}");
    if cfg!(unix) { assert_eq!(&got[..6], [1, 1, 1, 1, 1, 0], "a folder opens for reading"); }
}

/// A test that keeps its scratch folder in its own source tree (an overlay run's sources, with an
/// edits folder): show_control's preset bank makes a folder where a preset file was, removes it,
/// puts a dangling symlink there, probes it, and removes the whole bank. In a virtual folder
/// remove_dir went to rmdir(2) on the virtual path (NotFound for the folder create_dir had just
/// made), the symlink went there too, its probe followed it (Missing), and the dangling link could
/// not be removed (11-25). Each step answers as it does in a disk folder.
#[cfg(unix)]
#[test]
fn folders_and_symlinks_made_in_a_virtual_folder_behave_as_on_the_disk() {
    isolated_test!(folders_and_symlinks_made_in_a_virtual_folder_behave_as_on_the_disk);
    let fixture = vfs_store::Fixture::new(&[("app/a.rs", b"a")]);
    let root = "/relax-values-vfs-links/vfs/m";
    crate::vfs::mount(root, fixture.store.clone(), &fixture.commit).unwrap();
    let edits = std::env::temp_dir().join(format!("relax-values-vfs-links-edits-{}", std::process::id()));
    crate::vfs::set_edits(Some(edits.clone()));
    let disk = std::env::temp_dir().join(format!("relax-values-disk-links-{}", std::process::id()));
    std::fs::create_dir_all(disk.join("app")).unwrap();
    std::fs::write(disk.join("app/a.rs"), "a").unwrap();
    let disk_root = disk.to_str().unwrap().to_string();
    let src = format!(r#"
use std::io::ErrorKind as K;
fn code<T>(r: std::io::Result<T>) -> usize {{
    match r {{
        Ok(_) => 1,
        Err(e) if e.kind() == K::NotFound => 2,
        Err(e) if e.kind() == K::AlreadyExists => 3,
        Err(e) if e.kind() == K::InvalidInput => 4,
        Err(_) => 9,
    }}
}}
pub fn values(out: &mut [usize; 22]) {{
    for (k, root) in [{disk_root:?}, {root:?}].into_iter().enumerate() {{
        let out = &mut out[k * 11..k * 11 + 11];
        let app = std::path::Path::new(root).join("app");
        let (dir, slot) = (app.join("t"), app.join("t/slot.ron"));
        out[0] = code(std::fs::create_dir_all(&dir));
        out[1] = code(std::fs::create_dir(&slot));
        out[2] = code(std::fs::remove_dir(&slot));
        out[3] = code(std::os::unix::fs::symlink(dir.join("missing.ron"), &slot));
        out[4] = code(std::fs::read_to_string(&slot));
        out[5] = std::fs::symlink_metadata(&slot).map_or(0, |m| 1 + m.file_type().is_symlink() as usize);
        out[6] = (std::fs::read_link(&slot).ok() == Some(dir.join("missing.ron"))) as usize;
        out[7] = code(std::os::unix::fs::symlink("x", &slot));
        out[8] = code(std::os::unix::fs::symlink("x", app.join("a.rs")));
        out[9] = code(std::fs::read_link(app.join("a.rs")));
        out[10] = code(std::fs::remove_dir_all(&dir)) * 10 + std::fs::symlink_metadata(&dir).is_err() as usize;
    }}
}}
"#);
    let got = values::<22>(&src, "vfs_links");
    crate::vfs::set_edits(None);
    std::fs::remove_dir_all(&disk).unwrap();
    let _ = std::fs::remove_dir_all(&edits);
    // made, made, removed, linked, read NotFound, a symlink, its text, EEXIST over the link and
    // over a file, a file is no link (EINVAL), the folder removed with the link in it
    assert_eq!(&got[..11], [1, 1, 1, 1, 2, 2, 1, 3, 3, 4, 11], "the disk folder: {got:?}");
    assert_eq!(&got[11..], &got[..11], "the virtual folder as the disk folder: {got:?}");
}

/// std::backtrace (it was a stub that printed "unsupported backtrace", so a lane learned who held a
/// stale id only from a native run): a capture prints real std's frames from its caller on, each
/// fn with the file and line it was at, innermost first. `mid` is `#[inline]`: where a tier
/// inlines it into `outer`, it is still a frame of its own (the line table's inline stack).
/// Debug is real std's list; a disabled backtrace prints as std's does.
const BACKTRACE: &str = r#"
use std::backtrace::{Backtrace, BacktraceStatus};
#[inline(never)]
fn inner() -> Backtrace { let b = Backtrace::force_capture(); std::hint::black_box(&b); b }
#[inline]
fn mid() -> Backtrace { let b = inner(); std::hint::black_box(&b); b }
#[inline(never)]
fn outer() -> Backtrace { let b = mid(); std::hint::black_box(&b); b }
/// frame `k`'s fn (its name's last segment) and line
fn frame(lines: &[&str], k: usize) -> (String, usize) {
    let head = format!("{:4}: ", k);
    let Some(i) = lines.iter().position(|l| l.starts_with(&head)) else { return (String::new(), 0) };
    let name = lines[i][head.len()..].rsplit("::").next().unwrap_or("").to_string();
    let line = lines.get(i + 1).and_then(|l| l.trim_start().strip_prefix("at ")).and_then(|a| a.rsplitn(3, ':').nth(1)).and_then(|l| l.parse().ok()).unwrap_or(0);
    (name, line)
}
pub fn values(out: &mut [usize; 8]) {
    let b = outer();
    out[0] = (b.status() == BacktraceStatus::Captured) as usize;
    let text = b.to_string();
    let lines: Vec<&str> = text.lines().collect();
    let fs: Vec<(String, usize)> = (0..4).map(|k| frame(&lines, k)).collect();
    out[1] = (fs.iter().map(|f| f.0.as_str()).collect::<Vec<_>>() == ["inner", "mid", "outer", "values"]) as usize;
    out[2] = fs[0].1;
    out[3] = fs[1].1;
    out[4] = fs[2].1;
    out[5] = lines.get(1).is_some_and(|l| l.contains("lib.rs:4:")) as usize;
    let dbg = format!("{:?}", b);
    out[6] = (dbg.starts_with("Backtrace [{ fn: \"") && dbg.contains("::inner\", file: \"")) as usize;
    out[7] = (Backtrace::disabled().to_string() == "disabled backtrace" && format!("{:?}", Backtrace::disabled()) == "<disabled>") as usize;
}
"#;

#[test]
fn a_backtrace_names_its_callers_with_their_lines() {
    isolated_test!(a_backtrace_names_its_callers_with_their_lines);
    let (first, o3, refused): ([usize; 8], [usize; 8], _) = values_at_o3(BACKTRACE, "backtrace", RELEASE);
    assert_eq!(first, [1, 1, 4, 6, 8, 1, 1, 1], "captured; frames inner, mid, outer, values; their lines; the file; Debug; disabled (first tier)");
    assert_eq!(o3, first, "the same at O3 (not built: {refused:?})");
}

/// A panic parks (inspect events.rs on_panic) only where it leaves the program: one the program's
/// catch_unwind catches goes back to it, which handles it, and parks nothing (a backtrace sample's
/// caught panic parked, and devhub ended the idle lease, 11-18); one that ends a thread parks, as
/// does a caught one rethrown out of a thread (resume_unwind searches anew) and one at a retry
/// boundary (makepad's event dispatch: it catches only to retry). The run's endpoint is up with
/// nobody attached, so each parks once and is released as a quit releases it (it unwinds).
const PANIC_PARK: &str = r#"
extern "C" {
    fn relax_retry_enter(name: *const u8, len: usize);
    fn relax_retry_leave();
}
fn rethrown(f: impl FnOnce() + std::panic::UnwindSafe) {
    if let Err(p) = std::panic::catch_unwind(f) {
        std::panic::resume_unwind(p);
    }
}
pub fn values(out: &mut [usize; 5]) {
    out[0] = std::panic::catch_unwind(|| { let v: Vec<u32> = Vec::new(); std::hint::black_box(&v)[3] }).is_err() as usize;
    out[1] = std::panic::catch_unwind(|| rethrown(|| panic!("caught after a rethrow"))).is_err() as usize;
    out[2] = std::thread::spawn(|| { let x: Option<u32> = std::hint::black_box(None); x.unwrap() }).join().is_err() as usize;
    out[3] = std::thread::spawn(|| rethrown(|| panic!("rethrown out of a thread"))).join().is_err() as usize;
    let name = "Signal";
    unsafe { relax_retry_enter(name.as_ptr(), name.len()) };
    out[4] = std::panic::catch_unwind(|| panic!("at a retry boundary")).is_err() as usize;
    unsafe { relax_retry_leave() };
}
"#;

#[test]
fn a_panic_parks_only_where_it_leaves_the_program() {
    isolated_test!(a_panic_parks_only_where_it_leaves_the_program);
    use std::sync::atomic::{AtomicBool, Ordering};
    std::env::remove_var("RELAX_INSPECT");
    std::env::set_var("RELAX_PANIC_PARK", "600");
    // the parks are this test's own, and it releases them itself: a devhub node's observer releases
    // a park in a run of a lease nobody attends unless its process says so, and released before the
    // watcher looked, a park went uncounted (11-20: 183 of 240 runs under load failed)
    std::env::set_var("RELAX_EXPECTED_CRASH", "a_panic_parks_only_where_it_leaves_the_program parks its panics and releases them");
    crate::inspect::start("panic_park");
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let watcher = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            // each park counted once, by its id, and released only once counted: a park released
            // stays listed until its thread wakes, and the program's next panic parks only after
            // that release. Counted again, the released one parked twice; released by a later look
            // that still listed it, the next park went with it unseen
            let mut seen: Vec<(String, String)> = Vec::new();
            let mut ids = std::collections::HashSet::new();
            while !stop.load(Ordering::Relaxed) {
                let new: Vec<_> = crate::inspect::events::parked_now().into_iter().filter(|p| ids.insert(p.0)).collect();
                if !new.is_empty() {
                    seen.extend(new.into_iter().map(|p| (p.3, p.4)));
                    crate::inspect::events::quit_releases();
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            seen
        })
    };
    let got: [usize; 5] = values(PANIC_PARK, "panic_park");
    stop.store(true, Ordering::Relaxed);
    let seen = watcher.join().unwrap();
    let parked = |m: &str| seen.iter().filter(|p| p.0.contains(m)).count();
    assert_eq!(got, [1; 5], "catch_unwind and join return each panic");
    assert_eq!(parked("index out of bounds"), 0, "a panic catch_unwind catches parks nothing: {seen:?}");
    assert_eq!(parked("caught after a rethrow"), 0, "nor one caught again after a rethrow: {seen:?}");
    assert_eq!(parked("on a `None` value"), 1, "a panic that ends a thread parks: {seen:?}");
    assert_eq!(parked("rethrown out of a thread"), 1, "a caught panic rethrown out of a thread parks once: {seen:?}");
    assert!(seen.iter().any(|p| p.0.contains("rethrown out of a thread") && p.1.ends_with(":15:48")), "the rethrow's park says where the panic began: {seen:?}");
    assert_eq!(parked("at a retry boundary"), 1, "a panic at a retry boundary parks: {seen:?}");
}

/// A panic through a restartable fn (`restartable <fn>`) parks even where the program catches it
/// (the one deciding asked), and each decision does what it says: `restart` runs the fn again
/// from its entry, `return <v>` makes the call return v, `panic` unwinds on to the catch. Every
/// frame's drops run on the way, innermost first, as rustc's unwinder runs them (digits of the
/// log: leaf 1, mid's box 2, mid 3, the fn's inner block 4, the fn 5).
const RESTARTABLE: &str = r#"
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
static LOG: AtomicUsize = AtomicUsize::new(0);
static ATTEMPT: AtomicUsize = AtomicUsize::new(0);
struct G(usize);
impl Drop for G {
    fn drop(&mut self) { LOG.store(LOG.load(SeqCst) * 10 + self.0, SeqCst); }
}
#[inline(never)]
fn leaf(n: usize) -> u64 { let _g = G(1); if std::hint::black_box(n) > 0 { panic!("attempt {n}") } 0 }
#[inline(never)]
fn mid(n: usize) -> u64 { let _a = G(3); let _b = Box::new(G(2)); let r = leaf(n); let _c = G(9); r }
#[inline(never)]
pub fn restartable_work(x: u64) -> u64 {
    if x == 0 { return 0; }
    let _w = G(5);
    let n = ATTEMPT.fetch_add(1, SeqCst) + 1;
    if n != 2 { let _i = G(4); mid(n); }
    x + 1000
}
pub fn values(out: &mut [usize; 7]) {
    restartable_work(0);
    let (ready, go) = (std::env::var("RESTART_READY").unwrap(), std::env::var("RESTART_GO").unwrap());
    std::fs::write(&ready, "").unwrap();
    while !std::path::Path::new(&go).exists() { std::thread::sleep(std::time::Duration::from_millis(2)); }
    out[0] = std::panic::catch_unwind(|| restartable_work(7)).unwrap_or(0) as usize;
    out[1] = LOG.swap(0, SeqCst);
    out[2] = std::panic::catch_unwind(|| restartable_work(7)).unwrap_or(0) as usize;
    out[3] = LOG.swap(0, SeqCst);
    out[4] = std::panic::catch_unwind(|| restartable_work(7)).is_err() as usize;
    out[5] = LOG.swap(0, SeqCst);
    out[6] = ATTEMPT.load(SeqCst);
}
"#;

#[test]
fn a_panic_through_a_restartable_fn_restarts_returns_or_unwinds_as_decided() {
    isolated_test!(a_panic_through_a_restartable_fn_restarts_returns_or_unwinds_as_decided);
    use std::sync::atomic::{AtomicBool, Ordering};
    std::env::remove_var("RELAX_INSPECT");
    std::env::set_var("RELAX_PANIC_PARK", "600");
    // the parks are this test's own, and its decider resumes them: a devhub node's observer releases a park
    // in a run of a lease nobody attends unless its process says so, and released (`panic`) before the
    // decider looked, an attempt unwound undecided (11-20 own-tests run: 3 attempts, decided [])
    std::env::set_var("RELAX_EXPECTED_CRASH", "a_panic_through_a_restartable_fn_restarts_returns_or_unwinds_as_decided parks its panics and decides them");
    crate::inspect::start("restartable");
    let dir = std::env::temp_dir().join(format!("relax-restartable-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ready, go) = (dir.join("ready"), dir.join("go"));
    std::env::set_var("RESTART_READY", &ready);
    std::env::set_var("RESTART_GO", &go);
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let decider = {
        let (stop, go) = (stop.clone(), go.clone());
        std::thread::spawn(move || {
            let say = |f: fn(&crate::inspect::Req, &mut crate::inspect::Out), line: &str| String::from_utf8_lossy(&makepad_remote::cmd::answer(f, line)).into_owned();
            let wait = |until: &mut dyn FnMut() -> bool| {
                while !until() && !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            };
            // armed once the program's first call compiled the fn
            wait(&mut || ready.exists());
            let armed = say(crate::inspect::instrument::cmd, "restartable restartable_work");
            std::fs::write(&go, "").unwrap();
            let mut decided = Vec::new();
            for how in ["restart", "return 4242", "panic"] {
                let mut id = None;
                wait(&mut || { id = crate::inspect::events::parked_now().first().map(|p| p.0); id.is_some() });
                let Some(id) = id else { break };
                decided.push((how, say(crate::inspect::events::cmd_resume, &format!("resume P{id} {how}"))));
                wait(&mut || !crate::inspect::events::parked_now().iter().any(|p| p.0 == id));
            }
            (armed, decided)
        })
    };
    let got: [usize; 7] = values(RESTARTABLE, "restartable");
    stop.store(true, Ordering::Relaxed);
    let (armed, decided) = decider.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got[6], 4, "four attempts: one restarted, then a return, then an unwind (armed: {armed}; decided: {decided:?})");
    assert_eq!((got[0], got[1]), (1007, 123455), "restart: the fn ran again and returned, its frames' drops ran first ({decided:?})");
    assert_eq!((got[2], got[3]), (4242, 12345), "return 4242: the call returned it ({decided:?})");
    assert_eq!((got[4], got[5]), (1, 12345), "panic: it unwound on to the catch ({decided:?})");
}

/// A constant's initializer that runs as code (ctfe.rs off) runs constant-evaluation twins (ctfe_twin.rs), which count rustc's const-eval steps;
/// the program's own code is untouched: `count` called at run time counts nothing.
const CTFE_TWINS: &str = r#"
const fn bump(x: &mut u32) { *x += 1; }
pub const fn count(n: u32) -> u32 { let mut i = 0; while i < n { bump(&mut i); } i }
pub const AT_COMPILE: u32 = count(5);
pub fn values(out: &mut [usize; 2]) {
    out[0] = AT_COMPILE as usize;
    out[1] = count(std::hint::black_box(7)) as usize;
}
"#;

#[test]
fn a_constants_code_counts_its_steps_and_the_programs_does_not() {
    isolated_test!(a_constants_code_counts_its_steps_and_the_programs_does_not);
    use relax_backend::rir::{Callee, Inst};
    let mut session = crate::Session::default();
    session.options.ctfe = crate::options::Ctfe::Off;
    let (mut u, krate) = unit_in(session, CTFE_TWINS, "ctfe_twins", RELEASE);
    // (the slots keep their RIR for the look below)
    u.keep_compile_state = true;
    assert_eq!(run::<2>(&mut u, krate), [5, 7], "the constant's value and the run-time call's");
    let tick = u.syms.intern(crate::syms::SymKind::Rt("const_fuel"), crate::lower::const_fuel_addr());
    let ticks = |f: &relax_backend::rir::Func| f.blocks.iter().flat_map(|b| &b.insts).filter(|i| matches!(i, Inst::Call(Callee::Host(s), ..) if *s == tick)).count();
    let (mut twins, mut built) = (Vec::new(), 0);
    for s in u.fns.iter() {
        let Some(rir) = &s.rir else { continue };
        if matches!(s.key, crate::jit::FnKey::Ctfe(_)) {
            twins.push((s.name.to_string(), ticks(rir)));
        } else {
            built += 1;
            assert_eq!(ticks(rir), 0, "{}: the program's code counts constant-evaluation steps", s.name);
        }
    }
    assert!(built > 0 && twins.len() >= 3, "the initializer, count and bump each run as a twin, beside the program's code: {twins:?}");
    let of = |name: &str| twins.iter().find(|t| t.0.contains(name)).map_or(usize::MAX, |t| t.1);
    assert_eq!((of("AT_COMPILE"), of("count"), of("bump")), (1, 2, 0), "steps counted at the calls of fns and at the loop's back edge: {twins:?}");
}

/// A static whose value holds a union has no value entry in the disk cache (the data walk cannot
/// tell its pointer words: syms.rs obj_ptrs), so every run runs its initializer, as println!'s
/// STDOUT (a OnceLock) does; ctfe.rs cannot build the union and falls back to code. The
/// constant-evaluation twins that run (the initializer's, the const fns' it calls) are cached like
/// the program's code, and an initializer whose twin is cached runs it without trying ctfe.rs
/// first: the next run links every twin and type-checks none of their bodies (before, every warm
/// run did both again: the JIT legs' cache_warm_statics warm misses).
const CTFE_WARM: &str = r#"
const fn bump(x: &mut u32) { *x += 1; }
pub const fn count(n: u32) -> u32 { let mut i = 0; while i < n { bump(&mut i); } i }
const fn bits(n: u32) -> Bits { Bits { n: count(n) } }
pub union Bits { n: u32, f: f32 }
pub static HITS: Bits = bits(5);
pub fn values(out: &mut [usize; 1]) {
    out[0] = unsafe { HITS.n } as usize;
}
"#;

#[test]
fn a_constants_twins_link_from_the_disk_cache_in_the_next_run() {
    const PHASE: &str = "RELAX_CTFE_WARM_TEST";
    const DIR: &str = "RELAX_CTFE_WARM_TEST_DIR";
    let Ok(phase) = std::env::var(PHASE) else {
        let dir = std::env::temp_dir().join(format!("relax-ctfe-warm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("lib.rs"), CTFE_WARM).unwrap();
        let mut said = String::new();
        for phase in ["cold", "warm"] {
            let out = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "value_tests::a_constants_twins_link_from_the_disk_cache_in_the_next_run", "--nocapture", "--test-threads=1"])
                .env(PHASE, phase).env(DIR, &dir).env("RELAX_WARM_STRICT", "1"));
            let err = String::from_utf8_lossy(&out.stderr).into_owned();
            said += &format!("{phase}: {}\n{err}\n", String::from_utf8_lossy(&out.stdout));
            assert!(out.status.success(), "{said}");
            // (RELAX_WARM_STRICT's report: the fixture's own bodies; std's load is not this test's)
            let misses: Vec<&str> = err.lines().filter(|l| l.contains("warm miss:") && l.contains("ctfe_warm::")).collect();
            assert!(phase == "cold" || misses.is_empty(), "the next run type-checks no body its twins came from: {misses:?}\n{said}");
        }
        std::fs::remove_dir_all(dir).unwrap();
        return;
    };
    crate::jit::CHECK_FIRST.store(false, std::sync::atomic::Ordering::Relaxed);
    std::env::set_var("RELAX_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::path::PathBuf::from(std::env::var_os(DIR).unwrap());
    std::env::set_var("RELAX_CACHE_DIR", dir.join("cache"));
    let (prog, core, krate) = crate::load_with_std_checks(&std::sync::Arc::new(crate::Session::default()), dir.join("lib.rs").to_str().unwrap(), "ctfe_warm", false, RELEASE).unwrap();
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    crate::owner::call(crate::owner::Class::Program, || {
        let mut cache = crate::cache::Cache::open(&u, crate::cache::Mode::ReadOnly).unwrap();
        cache.mode = crate::cache::Mode::On;
        u.cache = Some(Box::new(crate::cache::CacheCell::new(cache)));
    });
    // the warm start, validated (run.rs classify): what it type-checks from here is a warm miss
    if phase == "warm" {
        relax_codegen::warm::classify(true);
    }
    assert_eq!(run::<1>(&mut u, krate), [5], "HITS as its initializer left it");
    // the twins that ran, with where their code came from
    let twins: Vec<(String, &str)> = crate::owner::call(crate::owner::Class::Program, || {
        u.fns.iter().filter(|f| f.compiled && matches!(f.key, crate::jit::FnKey::Ctfe(_)))
            .map(|f| (f.name.to_string(), crate::inspect::codever::kind_at(&u, f.code).origin_name())).collect()
    });
    let ran = |name: &str| twins.iter().any(|t| t.0.contains(name));
    assert!(ran("HITS") && ran("bits") && ran("count") && ran("bump"), "the initializer and the const fns it calls run as twins ({phase} run): {twins:?}");
    if phase == "cold" {
        assert!(twins.iter().all(|t| t.1 == "fresh"), "the first run builds them: {twins:?}");
        crate::owner::call(crate::owner::Class::Program, || {
            u.flush_cache_records();
            u.cache.as_mut().unwrap().flush().unwrap();
        });
    } else {
        let built: Vec<_> = twins.iter().filter(|t| t.1 != "warm-cache").collect();
        assert!(built.is_empty(), "the next run links every twin from the disk cache; built again: {built:?} of {twins:?}");
        relax_codegen::warm::finish();
    }
    // (process exit owns the runtime's teardown)
    std::mem::forget(u);
}

/// A `global_asm!` function with its own unwind info (`.seh_*`) and no frame pointer, as
/// rustc's would leave it: `seh_through(data, f)` calls `f(data)`. From `inside`, below it, a
/// walk by the function tables (RtlLookupFunctionEntry / RtlVirtualUnwind, as the exception
/// dispatcher and std's backtraces walk) goes through its frame (the code area's entry for Relax
/// frames would take the caller's frame pointer as its own and skip the caller) to `outer` and on
/// as `outer`'s own walk goes; an exception raised in `inside` reaches its `.seh_handler`, which
/// resumes the raise.
#[cfg(all(windows, target_arch = "x86_64"))]
const SEH: &str = r#"
use std::sync::atomic::{AtomicUsize, Ordering};
std::arch::global_asm!(
    ".globl seh_through",
    "seh_through:",
    ".seh_proc seh_through",
    ".seh_handler seh_through_handler, @except",
    "push rbx",
    ".seh_pushreg rbx",
    "push rsi",
    ".seh_pushreg rsi",
    "sub rsp, 40",
    ".seh_stackalloc 40",
    ".seh_endprologue",
    "call rdx",
    "nop",
    "add rsp, 40",
    "pop rsi",
    "pop rbx",
    "ret",
    ".seh_endproc",
);
extern "C" {
    fn seh_through(data: *mut usize, f: extern "C" fn(*mut usize));
}
/// CONTEXT (x64): rsp and rip by their offsets
#[repr(C, align(16))]
struct Context([u64; 154]);
const RSP: usize = 0x98 / 8;
const RIP: usize = 0xf8 / 8;
#[link(name = "kernel32")]
extern "system" {
    fn RtlCaptureContext(c: *mut Context);
    fn RtlLookupFunctionEntry(pc: u64, base: *mut u64, history: *mut u8) -> *const u8;
    fn RtlVirtualUnwind(kind: u32, base: u64, pc: u64, f: *const u8, c: *mut Context, data: *mut *mut u8, frame: *mut u64, pointers: *mut u8) -> *mut u8;
    fn RaiseException(code: u32, flags: u32, n: u32, args: *const usize);
}
const CODE: u32 = 0xE0DE_0001;
static HANDLED: AtomicUsize = AtomicUsize::new(0);
static OUTER: [AtomicUsize; 8] = [const { AtomicUsize::new(0) }; 8];
#[no_mangle]
unsafe extern "system" fn seh_through_handler(record: *const u32, _frame: *mut u8, _context: *mut u8, _dispatch: *mut u8) -> u32 {
    if *record != CODE {
        return 1;
    }
    HANDLED.fetch_add(1, Ordering::Relaxed);
    0
}
/// The return addresses from its caller's on (the caller's frame first), by the function tables.
#[inline(never)]
fn walk(t: &mut [usize; 8]) -> usize {
    let mut c = Context([0; 154]);
    unsafe { RtlCaptureContext(&mut c) };
    let mut n = 0;
    while n < t.len() {
        let pc = c.0[RIP];
        let mut base = 0;
        let f = unsafe { RtlLookupFunctionEntry(pc, &mut base, std::ptr::null_mut()) };
        if f.is_null() {
            // a leaf: its return address at rsp
            c.0[RIP] = unsafe { *(c.0[RSP] as *const u64) };
            c.0[RSP] += 8;
        } else {
            let (mut data, mut frame) = (std::ptr::null_mut(), 0);
            unsafe { RtlVirtualUnwind(0, base, pc, f, &mut c, &mut data, &mut frame, std::ptr::null_mut()) };
        }
        if c.0[RIP] == 0 {
            break;
        }
        t[n] = c.0[RIP] as usize;
        n += 1;
    }
    n
}
extern "C" fn inside(out: *mut usize) {
    let mut t = [0usize; 8];
    let n = walk(&mut t);
    // here, seh_through, outer, then the frames outer's own walk saw after itself
    let walked = (1..=4).filter(|&k| k + 2 < n && t[k + 2] == OUTER[k].load(Ordering::Relaxed)).count();
    unsafe {
        *out = walked;
        RaiseException(CODE, 0, 0, std::ptr::null());
        *out.add(1) = HANDLED.load(Ordering::Relaxed);
    }
}
#[inline(never)]
fn outer(out: &mut [usize; 3]) {
    let mut t = [0usize; 8];
    walk(&mut t);
    for (o, a) in OUTER.iter().zip(t) {
        o.store(a, Ordering::Relaxed);
    }
    unsafe { seh_through(out.as_mut_ptr(), inside) };
}
pub fn values(out: &mut [usize; 3]) {
    outer(out);
    out[2] = 7;
}
"#;

#[cfg(all(windows, target_arch = "x86_64"))]
#[test]
fn an_asm_functions_own_unwind_info_walks_its_frame_and_calls_its_handler() {
    isolated_test!(an_asm_functions_own_unwind_info_walks_its_frame_and_calls_its_handler);
    let [walked, handled, done] = values::<3>(SEH, "seh_unwind");
    assert_eq!(walked, 4, "a walk from below the asm frame goes on through its caller as the caller's own walk does");
    assert_eq!(handled, 1, "an exception raised below the asm frame reaches its handler, which resumes it");
    assert_eq!(done, 7, "the program goes on after the asm function returns");
}
