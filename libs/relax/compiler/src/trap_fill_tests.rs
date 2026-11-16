//! Trap fill (invalid.rs): a dead patch range beside live code in its generation's arena becomes
//! a trap at every offset a fetch can start at, written while its pages are out of reach. A
//! neighbour in the same page runs on throughout (its faults wait for the fill and run again); a
//! call, or a jump into the middle, of the dead range is a stale access naming it; once nothing in
//! a chunk lives it is inaccessible and a read of it is a stale access too.

use super::*;
use crate::invalid::{self, Group};

/// `return n` as machine code (x86-64 `mov eax, n; ret`; arm64 `movz w0, #n; ret`).
fn ret_code(n: u32) -> Vec<u8> {
    #[cfg(target_arch = "x86_64")]
    {
        let mut c = vec![0xb8];
        c.extend(n.to_le_bytes());
        c.push(0xc3);
        c
    }
    #[cfg(target_arch = "aarch64")]
    {
        let mut c = (0x5280_0000u32 | (n & 0xffff) << 5).to_le_bytes().to_vec();
        c.extend(0xd65f_03c0u32.to_le_bytes());
        c
    }
}

fn call_at(at: u64) -> u32 {
    let f: extern "C" fn() -> u32 = unsafe { std::mem::transmute(at as usize) };
    f()
}

/// `return *(arg0 as *const u8)` as machine code (x86-64 `movzx eax, byte [rdi or rcx]; ret`;
/// arm64 `ldrb w0, [x0]; ret`).
fn load_code() -> Vec<u8> {
    #[cfg(all(target_arch = "x86_64", windows))]
    return vec![0x0f, 0xb6, 0x01, 0xc3];
    #[cfg(all(target_arch = "x86_64", not(windows)))]
    return vec![0x0f, 0xb6, 0x07, 0xc3];
    #[cfg(target_arch = "aarch64")]
    {
        let mut c = 0x3940_0000u32.to_le_bytes().to_vec();
        c.extend(0xd65f_03c0u32.to_le_bytes());
        c
    }
}

fn source_unit(name: &str, source: &str) -> (Box<Unit>, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!("relax-{}-{}.rs", name, std::process::id()));
    std::fs::write(&path, source).unwrap();
    let session = std::sync::Arc::new(crate::Session::default());
    let mut prog = Program::new(&session);
    prog.load_crate(name, path.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&session, &[]), vec![]).unwrap();
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    (Unit::new(prog, tcx, 0), path)
}

/// The slot of fn `name`, and its def.
fn slot(u: &mut Unit, name: &str) -> (u32, DefId) {
    let d = u.prog.defs.iter().enumerate().find_map(|(i, _)| (u.prog.name(DefId(i as u32)) == name).then_some(DefId(i as u32))).unwrap();
    (u.fn_id(FnKey::Inst(d, vec![])), d)
}

/// Per case a process, which ends by its stale access as an unattended run does (the report,
/// then the fault's signal): `call` calls the dead range, `odd` jumps into its middle, `closed`
/// reads a chunk nothing lives in any more. Every case first fills 100 ranges in the page a
/// looping thread runs its neighbour in, the thread seen running before the first fill. Then a
/// fault seen only after its fill ended is that fill's: the same instruction's fault in the next
/// fill runs again too (taken for the next fill's, the neighbour's fault in that fill was taken
/// for a repeat and ended the process with no report).
/// Each access comes from code of the unit (the dead range's own, or a reader of another
/// generation's arena), as in a run: under the JIT this test's own code is the JIT's, which
/// contains a fault in its code before any handler the program installed (jit.rs
/// `containment`), so a read made here ended as the JIT's fault.
#[test]
fn a_trap_filled_range_traps_at_any_offset_while_its_page_neighbour_runs_on() {
    const CHILD: &str = "RELAX_TRAP_FILL_CASE";
    let Ok(case) = std::env::var(CHILD) else {
        for case in ["call", "odd", "closed"] {
            let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "jit::trap_fill_tests::a_trap_filled_range_traps_at_any_offset_while_its_page_neighbour_runs_on", "--nocapture", "--test-threads=1"])
                .env(CHILD, case).env("RELAX_CACHE", "off").env_remove("RELAX_STALE_PARK"));
            let all = format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            // the access it makes and the verdict that access must get, as the child said them
            let expect = all.lines().find_map(|l| l.strip_prefix("trap fill expects: ")).unwrap_or_else(|| panic!("{case}: the child did not get to its stale access (it ended: {}):\n{all}", output.status));
            assert!(all.lines().any(|l| l.starts_with("relax: stale access: ") && l.contains(expect)), "{case}: no stale access report with {expect:?}:\n{all}");
            assert!(!output.status.success() && !all.contains("test result: ok"), "{case}: the process ends by the stale access:\n{all}");
        }
        return;
    };
    use std::sync::atomic::{AtomicBool, AtomicU64};
    let (mut u, path) = source_unit("trap_fill", "#![no_std] pub fn dead() {} pub fn live() {}");
    let (dead, live) = (slot(&mut u, "dead").0, slot(&mut u, "live").0);
    let page = crate::os::mem::page_size();
    // the live neighbour: patch generation 1's code of `live`, called in a loop from here on
    let live_g = Group::Patch { slot: live, gen: 1 };
    let live_at = u.emit_grouped(Some(live_g), &ret_code(7), &[]).unwrap();
    invalid::note_install(&u, live_g, live, live_at, 6, 1);
    static STOP: AtomicBool = AtomicBool::new(false);
    static CALLS: AtomicU64 = AtomicU64::new(0);
    static WRONG: AtomicU64 = AtomicU64::new(0);
    let looping = std::thread::spawn(move || {
        while !STOP.load(Ordering::Relaxed) {
            if call_at(live_at) != 7 {
                WRONG.fetch_add(1, Ordering::Relaxed);
            }
            CALLS.fetch_add(1, Ordering::Relaxed);
        }
    });
    // the loop runs before the first fill
    let t0 = std::time::Instant::now();
    while CALLS.load(Ordering::Relaxed) == 0 {
        assert!(t0.elapsed().as_secs() < 30, "the neighbour's loop never ran");
        std::thread::yield_now();
    }
    // (each fill holds the pages out of reach long enough that the loop surely meets them)
    invalid::TRANSIT_HOLD_US.store(200, Ordering::Relaxed);
    let mut same_page = 0;
    for k in 0..100u32 {
        // a group of the same generation (its own slot number: a group dies once), run, then killed
        let g = Group::Patch { slot: 1_000_000 + k, gen: 1 };
        let at = u.emit_grouped(Some(g), &ret_code(100 + k), &[]).unwrap();
        assert_eq!(call_at(at), 100 + k, "range {k} runs before it dies");
        same_page += (at & !(page - 1) == live_at & !(page - 1)) as u32;
        invalid::invalidate(&mut u, |x| x == g, 2, "invalidated code", None, "a test's layout patch");
        assert!(crate::stale::lookup(at).is_some_and(|r| r.lo <= at && r.hi >= at + 16), "range {k} is a stale range");
        let bytes = unsafe { std::slice::from_raw_parts(at as *const u8, 16) };
        assert!(bytes.chunks(4).all(|w| w == invalid::TRAP), "range {k} holds the trap at every offset: {:02x?}", bytes);
    }
    invalid::TRANSIT_HOLD_US.store(0, Ordering::Relaxed);
    STOP.store(true, Ordering::Relaxed);
    looping.join().unwrap();
    let stats = invalid::stats_line();
    assert!(same_page > 0, "the dead ranges share the neighbour's page (a generation's arena): {stats}");
    assert_eq!(WRONG.load(Ordering::Relaxed), 0, "every call of the neighbour returned its value ({} calls)", CALLS.load(Ordering::Relaxed));
    assert!(invalid::TRANSIT_WAITS.load(Ordering::Relaxed) > 0, "the neighbour met a fill's pages and waited: {stats}");
    assert!(crate::stale::verdicts().is_empty(), "no stale access while the neighbour ran: {:?}", crate::stale::verdicts());
    assert_eq!(call_at(live_at), 7, "the neighbour runs on");
    assert!(invalid::PROT_BYTES.load(Ordering::Relaxed) == 0 && invalid::FILLED_BYTES.load(Ordering::Relaxed) == 100 * 16 && stats.contains("in 100 fills"), "100 ranges filled, nothing inaccessible: {stats}");
    eprintln!("{} calls of the neighbour; {stats}", CALLS.load(Ordering::Relaxed));
    // A fault whose handler reads the fill state only after its fill ended: a thread asks as the
    // handler does (jit.rs `transit_retry`, the neighbour's fetch at `live_at`), then asks on
    // until one of its asks met the next fill and waited for it. Both must run the instruction
    // again. Fills go on until it met one; nothing else runs into their pages meanwhile.
    static LATE: AtomicBool = AtomicBool::new(false);
    static MET: AtomicU64 = AtomicU64::new(0);
    let fill = |u: &mut Unit, k: u32| {
        let g = Group::Patch { slot: 3_000_000 + k, gen: 1 };
        let at = u.emit_grouped(Some(g), &ret_code(k), &[]).unwrap();
        assert_eq!(at & !(page - 1), live_at & !(page - 1), "fill {k} is in the neighbour's page");
        invalid::invalidate(u, |x| x == g, 2, "invalidated code", None, "a test's layout patch");
    };
    fill(&mut u, 0);
    let asking = std::thread::spawn(move || {
        let late = crate::jit::transit_retry(live_at, live_at);
        LATE.store(true, Ordering::SeqCst);
        loop {
            let waits = invalid::TRANSIT_WAITS.load(Ordering::SeqCst);
            let again = crate::jit::transit_retry(live_at, live_at);
            if invalid::TRANSIT_WAITS.load(Ordering::SeqCst) != waits {
                MET.store(if again { 1 } else { 2 }, Ordering::SeqCst);
                return late;
            }
        }
    });
    let t0 = std::time::Instant::now();
    while !LATE.load(Ordering::SeqCst) {
        assert!(t0.elapsed().as_secs() < 30, "the asking thread never asked");
        std::thread::yield_now();
    }
    invalid::TRANSIT_HOLD_US.store(200, Ordering::Relaxed);
    let mut k = 1;
    while MET.load(Ordering::SeqCst) == 0 {
        assert!(t0.elapsed().as_secs() < 30, "the asking thread met none of {k} fills");
        fill(&mut u, k);
        k += 1;
    }
    invalid::TRANSIT_HOLD_US.store(0, Ordering::Relaxed);
    let late = asking.join().unwrap();
    assert!(late, "a fault seen after its fill ended runs again");
    assert_eq!(MET.load(Ordering::SeqCst), 1, "the same instruction's fault in the next fill runs again (fills: {k})");
    // the stale access of this case
    let dead_g = Group::Patch { slot: dead, gen: 1 };
    let dead_at = u.emit_grouped(Some(dead_g), &ret_code(5), &[]).unwrap();
    invalid::note_install(&u, dead_g, dead, dead_at, 6, 1);
    invalid::invalidate(&mut u, |x| x == dead_g, 2, "invalidated code", None, "a test's layout patch");
    let name = u.fns[dead as usize].name.to_string();
    match case.as_str() {
        "call" | "odd" => {
            // x86-64: one byte in; arm64: the range's second word
            let to = if case == "call" { dead_at } else if cfg!(target_arch = "x86_64") { dead_at + 1 } else { dead_at + 4 };
            eprintln!("trap fill expects: ran code at {:#x} = invalidated code `{}` v1 + {:#x}", to, name, to - dead_at);
            std::fs::remove_file(&path).unwrap();
            call_at(to);
        }
        _ => {
            // the reader: live code in another generation's arena (its own chunk)
            let reader = u.emit_grouped(Some(Group::Patch { slot: 2_000_000, gen: 7 }), &load_code(), &[]).unwrap();
            assert_ne!(reader & !(page - 1), live_at & !(page - 1), "the reader's page is not the chunk's");
            let load: extern "C" fn(u64) -> u32 = unsafe { std::mem::transmute(reader as usize) };
            assert_eq!(load(live_at) as u8, ret_code(7)[0], "the reader reads the neighbour's first byte while it lives");
            // the neighbour dies too: nothing lives in the chunk, it is inaccessible
            invalid::invalidate(&mut u, |x| x == live_g, 3, "invalidated code", None, "a test's rollback");
            assert!(invalid::PROT_BYTES.load(Ordering::Relaxed) >= page, "the chunk is inaccessible: {}", invalid::stats_line());
            eprintln!("trap fill expects: read/wrote {:#x} = invalidated code `{}` v1 + 0x0", live_at, u.fns[live as usize].name);
            std::fs::remove_file(&path).unwrap();
            load(live_at);
        }
    }
    panic!("the stale access returned");
}

/// A layout patch kills the earlier version of the fn it rebuilt, while the earlier version of a
/// fn of the same patch generation it did not rebuild -- in the same arena chunk -- is current and
/// runs on: the dead one is trap-filled, nothing is made inaccessible.
#[test]
fn a_layout_patch_trap_fills_the_old_version_beside_live_code() {
    isolated_test!(a_layout_patch_trap_fills_the_old_version_beside_live_code);
    crate::borrowck::lazy::disable_gandalf();
    let base = "#![no_std] pub struct S { pub a: u32 } #[inline(never)] pub fn f(out: *mut u32) { let s = S { a: 1 }; unsafe { *out = s.a + 10; } } #[inline(never)] pub fn g(out: *mut u32) { unsafe { *out = 20; } }";
    let (mut u, path) = source_unit("trap_fill_layout", base);
    let ((f, f_def), g) = (slot(&mut u, "f"), slot(&mut u, "g").0);
    let file = u.prog.def(f_def).file;
    let up = &mut *u as *mut Unit;
    let run = |entry: u64| -> u32 {
        let mut r = 0u32;
        unsafe { call_jit(up, entry, &mut r as *mut u32 as u64) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        r
    };
    let (fe, ge) = (u.entry(f), u.entry(g));
    assert_eq!((run(fe), run(ge)), (11, 20));
    let edit = |src: String| crate::reload::Edit { file, path: path.to_string_lossy().into_owned(), src: src.into_bytes() };
    // generation 1: both bodies
    let gen1 = base.replace("s.a + 10", "s.a + 11").replace("= 20", "= 21");
    let out = unsafe { crate::reload::apply(up, vec![edit(gen1.clone())]) };
    assert_eq!(out.rung, crate::reload::Rung::Patch, "{:?}", out.lines);
    assert_eq!((run(fe), run(ge)), (12, 21));
    let (f1, g1) = (u.fns[f as usize].code, u.fns[g as usize].code);
    assert!(crate::stale::lookup(f1).is_none() && crate::stale::lookup(g1).is_none());
    // generation 2: S's fields change, f is rebuilt, g is not
    let gen2 = gen1.replace("pub a: u32 }", "pub a: u32, pub b: u32 }").replace("S { a: 1 }", "S { a: 1, b: 2 }").replace("s.a + 11", "s.a + s.b + 11");
    let out = unsafe { crate::reload::apply(up, vec![edit(gen2)]) };
    assert_eq!(out.rung, crate::reload::Rung::Patch, "{:?}", out.lines);
    assert_eq!((run(fe), run(ge)), (14, 21), "f runs its new code, g its generation-1 code");
    assert_eq!(u.fns[g as usize].code, g1, "g was not rebuilt");
    let r = crate::stale::lookup(f1).unwrap_or_else(|| panic!("f's generation-1 code is a stale range: {}", invalid::stats_line()));
    assert!(r.name.contains(&*u.fns[f as usize].name) && r.reason.contains("layout-changing patch gen 2"), "{} / {}", r.name, r.reason);
    let bytes = unsafe { std::slice::from_raw_parts(r.lo as *const u8, (r.hi - r.lo) as usize) };
    assert!(bytes.chunks(4).all(|w| w == invalid::TRAP), "f's generation-1 code is the trap: {:02x?}", &bytes[..16]);
    assert!(crate::stale::lookup(g1).is_none(), "g's code beside it lives");
    let stats = invalid::stats_line();
    assert!(invalid::PROT_BYTES.load(Ordering::Relaxed) == 0 && invalid::FILLED_BYTES.load(Ordering::Relaxed) >= 16, "a fill, nothing inaccessible: {stats}");
    std::fs::remove_file(path).unwrap();
    std::mem::forget(u);
}
