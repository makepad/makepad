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
/// does not overflow.
const CHECKS: &str = r#"
pub const ON: usize = if cfg!(debug_assertions) { 1 } else { 0 };
fn add(a: u8, b: u8) -> u8 { a + b }
fn shl(a: u32, b: u32) -> u32 { a << b }
fn neg(a: i32) -> i32 { -a }
pub fn values(out: &mut [usize; 7]) {
    out[0] = cfg!(debug_assertions) as usize;
    out[1] = ON;
    out[2] = std::panic::catch_unwind(|| debug_assert!(std::hint::black_box(1) == 2)).is_err() as usize;
    out[3] = std::panic::catch_unwind(|| add(std::hint::black_box(255), 1)).is_err() as usize;
    out[4] = std::panic::catch_unwind(|| shl(1, std::hint::black_box(32))).is_err() as usize;
    out[5] = std::panic::catch_unwind(|| neg(std::hint::black_box(i32::MIN))).is_err() as usize;
    out[6] = add(std::hint::black_box(250), 5) as usize;
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
    crate::jit::CHECK_FIRST.store(false, std::sync::atomic::Ordering::Relaxed);
    std::env::set_var("RELAX_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-values-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("lib.rs");
    std::fs::write(&path, src).unwrap();
    let (prog, core, krate) = crate::load_with_std_checks(&std::sync::Arc::new(crate::Session::default()), path.to_str().unwrap(), name, false, checks).unwrap();
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
    let (first, o3, refused): ([usize; 7], [usize; 7], _) = values_at_o3(CHECKS, "cfg_checked", CHECKED);
    assert_eq!(first, [1, 1, 1, 1, 1, 1, 255], "the test profile's checks at the first tier");
    assert_eq!(o3, first, "the test profile's checks at O3 (not built: {refused:?})");
    let (first, o3, _): ([usize; 7], [usize; 7], _) = values_at_o3(CHECKS, "cfg_released", RELEASE);
    assert_eq!((first, o3), ([0, 0, 0, 0, 0, 0, 255], [0, 0, 0, 0, 0, 0, 255]), "the release profile's: no check, at the first tier and at O3");
}

/// `IoSlice` / `IoSliceMut` (now real std's `iovec`-shaped wrappers, WARNZERO class 8): the
/// slices they borrow, `advance_slices` across them, a vectored write, writes through a mutable
/// one, and the `iovec` layout (base, then length) FFI code relies on.
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
    let words: [usize; 2] = unsafe { core::mem::transmute(IoSlice::new(&b)) };
    out[6] = (words[0] == b.as_ptr() as usize) as usize;
    out[7] = words[1];
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
/// fail EISDIR), so a virtual one (an overlay run's sources) is fsync'd, not refused EISDIR.
#[test]
fn a_virtual_folder_opens_for_reading_as_a_disk_folder_does() {
    isolated_test!(a_virtual_folder_opens_for_reading_as_a_disk_folder_does);
    let fixture = vfs_store::Fixture::new(&[("app/cache/a.bin", b"a")]);
    let root = "/relax-values-vfs-dir/vfs/m";
    crate::vfs::mount(root, fixture.store.clone(), &fixture.commit).unwrap();
    let src = format!(r#"
pub fn values(out: &mut [usize; 5]) {{
    let dir = std::fs::File::open("{root}/app/cache");
    out[0] = dir.is_ok() as usize;
    if let Ok(mut d) = dir {{
        out[1] = d.sync_all().is_ok() as usize;
        out[2] = d.metadata().map_or(0, |m| m.is_dir() as usize);
        let mut buf = [0u8; 4];
        out[3] = matches!(std::io::Read::read(&mut d, &mut buf), Err(e) if e.kind() == std::io::ErrorKind::IsADirectory) as usize;
    }}
    // a write still needs the edits folder
    out[4] = std::fs::OpenOptions::new().write(true).open("{root}/app/cache").is_err() as usize;
}}
"#);
    assert_eq!(values::<5>(&src, "vfs_dir"), [1, 1, 1, 1, 1]);
}
