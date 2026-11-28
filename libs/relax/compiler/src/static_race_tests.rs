//! Reverse static lookups off the owner (`syms::provenance` -> `Unit::static_sym_at`, from the
//! optimiser's `ro_read` and lowering ahead's `read_leaf`) while the owner lays statics out. The
//! spans live in `Syms` behind a leaf lock; `Unit::statics` stays the owner's (`OwnerStatics`
//! panics when an off-owner front reader touches it); the startup prelink report is taken on the
//! owner, so its caller never reads `Unit::cache`.

use super::*;
use crate::syms::{DataKind, SymKind};
use relax_backend::passes::RoRead;
use relax_backend::rir::{Mem, Sym};
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Barrier};

/// A `#![no_std]` program from `source`, loaded alone. `checked`: the session checks constant
/// evaluation (`const_fuel`) and the crate is validated, so a failing initializer is rejected by
/// the check-mode interpreter.
fn load(tag: &str, source: &str, checked: bool) -> Box<Unit> {
    crate::borrowck::lazy::disable_gandalf();
    let path = std::env::temp_dir().join(format!("relax-static-race-{tag}-{}.rs", std::process::id()));
    std::fs::write(&path, source).unwrap();
    let mut session = crate::Session::default();
    session.options.const_fuel = checked;
    let session = std::sync::Arc::new(session);
    let mut prog = Program::new(&session);
    let krate = prog.load_crate(tag, path.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&session, &[]), vec![]).unwrap();
    if checked {
        prog.crates[krate as usize].validate = true;
    }
    let _ = std::fs::remove_file(&path);
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    Unit::new(prog, tcx, 0)
}

fn def(u: &Unit, name: &str) -> DefId {
    (0..u.prog.defs.len()).map(|i| DefId(i as u32)).find(|&d| u.prog.name(d) == name).unwrap_or_else(|| panic!("no def {name}"))
}

fn show(r: &RoRead) -> String {
    match r {
        RoRead::Bytes(b) => format!("Bytes({b:#x})"),
        RoRead::Ptr(s) => format!("Ptr({s:?})"),
        RoRead::Range(n) => format!("Range({n})"),
        RoRead::Fn(f) => format!("Fn({f})"),
        RoRead::No => "No".into(),
    }
}

/// Link-section waves laid out by the owner, then one wave of standalone statics.
const SECTION_WAVES: usize = 8;
const WAVES: usize = SECTION_WAVES + 1;
const PER_SECTION: usize = 512;
const STANDALONE: usize = 128;
/// Two optimiser readers (`ro_read`), two lowering-ahead readers (`read_leaf` under a front read).
const READERS: usize = 4;

fn section_name(w: usize) -> String {
    if cfg!(target_vendor = "apple") {
        format!("__DATA,__relax_r{w}")
    } else if cfg!(windows) {
        format!("relax_r{w}$m")
    } else {
        format!("relax_r{w}")
    }
}

/// The barriers a reader still owes the owner: met on drop, so a reader that panics never leaves
/// the owner waiting at a barrier.
struct ReaderOwes {
    start: Arc<Barrier>,
    end: Arc<Barrier>,
    wave: usize,
    started: bool,
}

impl Drop for ReaderOwes {
    fn drop(&mut self) {
        while self.wave < WAVES {
            if !self.started {
                self.start.wait();
            }
            self.end.wait();
            self.started = false;
            self.wave += 1;
        }
    }
}

/// Clears the section insert hook however the owner's waves end.
struct ClearInsertHook;

impl Drop for ClearInsertHook {
    fn drop(&mut self) {
        crate::sections::set_sections_insert_hook(None);
    }
}

/// Per reader, this wave's target: (its own constant `&u64` object, that object's symbol, the
/// static symbol the pointer in it must resolve to).
type Targets = std::sync::Mutex<Vec<(u64, Sym, Sym)>>;

/// The owner publishes 4096 link-section spans in eight bursts and 128 standalone statics while
/// four off-owner readers resolve a pointer to the newest static of the previous burst, through
/// the optimiser's `ro_read` and lowering ahead's `read_leaf` (under its front read). Before the
/// fix the readers walked the owner's unsynchronized span map and `Unit::statics`.
#[test]
fn off_owner_ro_read_and_read_leaf_do_not_race_static_layout_burst() {
    isolated_test!(off_owner_ro_read_and_read_leaf_do_not_race_static_layout_burst);
    use std::fmt::Write;
    let mut source = String::from("#![no_std]\nstatic S0: u64 = 1;\nconst REF_TY_HOLDER: &u64 = &S0;\n");
    for w in 0..SECTION_WAVES {
        let section = section_name(w);
        for i in 0..PER_SECTION {
            writeln!(source, "#[link_section = \"{section}\"] static S_{w}_{i}: u64 = {};", 2 + w * 1000 + i).unwrap();
        }
    }
    for i in 0..STANDALONE {
        writeln!(source, "static STANDALONE_{i}: u64 = {};", 9000 + i).unwrap();
    }
    let mut u = load("burst", &source, false);
    let by_name: std::collections::HashMap<String, DefId> = (0..u.prog.defs.len()).map(|i| DefId(i as u32)).map(|d| (u.prog.name(d).to_string(), d)).collect();
    let s0 = by_name["S0"];
    let waves: Vec<Vec<DefId>> = (0..SECTION_WAVES).map(|w| (0..PER_SECTION).map(|i| by_name[&format!("S_{w}_{i}")]).collect()).collect();
    let standalone: Vec<DefId> = (0..STANDALONE).map(|i| by_name[&format!("STANDALONE_{i}")]).collect();
    let ref_ty = u.tcx.const_ty(&u.prog, by_name["REF_TY_HOLDER"]).expect("&u64");
    let up = &mut *u as *mut Unit as usize;

    let wave_start = Arc::new(Barrier::new(READERS + 1));
    let wave_end = Arc::new(Barrier::new(READERS + 1));
    let burst_done = Arc::new(AtomicBool::new(false));
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let targets: Arc<Targets> = Arc::new(std::sync::Mutex::new(vec![(0, Sym { id: 0, off: 0 }, Sym { id: 0, off: 0 }); READERS]));

    let readers: Vec<_> = (0..READERS).map(|r| {
        let (start, end, burst_done, targets) = (wave_start.clone(), wave_end.clone(), burst_done.clone(), targets.clone());
        std::thread::spawn(move || {
            let u = unsafe { &*(up as *const Unit) };
            let mut owes = ReaderOwes { start, end, wave: 0, started: false };
            let mut reads = 0u64;
            while owes.wave < WAVES {
                owes.start.wait();
                owes.started = true;
                let (data, data_sym, want) = targets.lock().unwrap()[r];
                loop {
                    // a fresh kind drops the cached pointer words: the next read walks them again
                    // (obj_ptrs -> provenance -> static_sym_at)
                    u.syms.set_data_kind(data, DataKind::Typed(ref_ty));
                    let got = if r < 2 {
                        crate::syms::ro_read(up, data_sym, 8)
                    } else {
                        let _front = front_read("lower_ahead");
                        crate::syms::read_leaf(u, data_sym, Mem::Int(8, false))
                    };
                    assert!(matches!(got, RoRead::Ptr(s) if s == want), "reader {r}, wave {}: read {} for a pointer to {want:?}", owes.wave, show(&got));
                    reads += 1;
                    if burst_done.load(Ordering::Acquire) {
                        break;
                    }
                }
                owes.end.wait();
                owes.started = false;
                owes.wave += 1;
            }
            reads
        })
    }).collect();

    owner::call(owner::Class::Program, || {
        let u = unsafe { &mut *(up as *mut Unit) };
        let s0_addr = u.static_addr(s0);
        assert_ne!(s0_addr, 0, "S0 has no storage: {:?}", u.errors);
        let mut anchor = (s0_addr, u.syms.intern(SymKind::Static(s0), s0_addr));
        let (start, calls) = (wave_start.clone(), hook_calls.clone());
        crate::sections::set_sections_insert_hook(Some(Arc::new(move || {
            calls.fetch_add(1, Ordering::SeqCst);
            start.wait();
        })));
        let _clear = ClearInsertHook;
        for w in 0..WAVES {
            assert_eq!(u.static_sym_at(anchor.0), Some(anchor.1), "wave {w}: the anchor does not resolve on the owner");
            {
                let mut t = targets.lock().unwrap();
                for slot in t.iter_mut() {
                    let data = u.data(&anchor.0.to_le_bytes(), 8, DataKind::Typed(ref_ty));
                    u.syms.set_data_kind(data, DataKind::Typed(ref_ty));
                    *slot = (data, u.data_sym(data), anchor.1);
                }
            }
            burst_done.store(false, Ordering::Release);
            if w < SECTION_WAVES {
                // lay_out meets the readers at wave_start (the hook), right before its span insert
                let a = u.static_addr(waves[w][0]);
                assert_ne!(a, 0, "wave {w}: no storage: {:?}", u.errors);
            } else {
                wave_start.wait();
                for &d in &standalone {
                    assert_ne!(u.static_addr(d), 0, "standalone static has no storage: {:?}", u.errors);
                }
            }
            burst_done.store(true, Ordering::Release);
            wave_end.wait();
            if w < SECTION_WAVES {
                // the newest static of this burst: the rightmost span the next burst grows past
                let (d, a) = waves[w].iter().map(|&d| (d, *u.statics.get(&d).unwrap())).max_by_key(|&(_, a)| a).unwrap();
                assert_ne!(a, 0);
                anchor = (a, u.syms.intern(SymKind::Static(d), a));
            }
        }
    });

    let mut reads = Vec::new();
    for h in readers {
        match h.join() {
            Ok(n) => reads.push(n),
            Err(p) => std::panic::resume_unwind(p),
        }
    }
    assert_eq!(hook_calls.load(Ordering::SeqCst), SECTION_WAVES, "the section insert hook ran once per section");
    // (at least one read per wave each: the readers' loop reads before it checks burst_done)
    assert!(reads.iter().all(|&n| n >= WAVES as u64), "{reads:?}");
    // (the isolated process owns the runtime globals)
    std::mem::forget(u);
}

/// A lookup of an address outside the writable static region (null, the top of the address
/// space, a stack word, read-only data) answers without the span lock: it completes while
/// another thread holds that lock.
#[test]
fn static_sym_at_in_rw_fast_path_skips_lock() {
    isolated_test!(static_sym_at_in_rw_fast_path_skips_lock);
    let mut u = load("fast_path", "#![no_std]\nstatic S0: u64 = 1;\n", false);
    let s0 = def(&u, "S0");
    let up = &mut *u as *mut Unit as usize;
    let s0_addr = owner::call(owner::Class::Program, || unsafe { (&mut *(up as *mut Unit)).static_addr(s0) });
    assert_ne!(s0_addr, 0, "S0 has no storage: {:?}", u.errors);
    let ro = u.data(&7u64.to_le_bytes(), 8, DataKind::Bytes);
    // (channels made before the lock is taken: a lookup that waits on the lock fails at the
    // deadline instead of hanging the test, and the lock is free again before the join)
    let (locked_tx, locked_rx) = std::sync::mpsc::sync_channel::<()>(1);
    let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
    let probing = Arc::new(AtomicUsize::new(usize::MAX));

    let looker = {
        let probing = probing.clone();
        std::thread::spawn(move || {
            locked_rx.recv().unwrap();
            let u = unsafe { &*(up as *const Unit) };
            let local = 0u64;
            let at = [0, u64::MAX, &local as *const u64 as u64, ro];
            let looked = std::array::from_fn::<_, 4, _>(|i| {
                probing.store(i, Ordering::Release);
                (at[i], u.static_sym_at(at[i]))
            });
            let _ = done_tx.send(looked);
        })
    };
    let (received, stalled_at) = u.syms.with_static_spans_locked(|| {
        let _ = locked_tx.send(());
        let r = done_rx.recv_timeout(std::time::Duration::from_secs(5));
        (r, probing.load(Ordering::Acquire))
    });
    // the span lock is free now, so even a looker that waited on it unblocks and joins
    if let Err(p) = looker.join() {
        std::mem::forget(u);
        std::panic::resume_unwind(p);
    }
    let looked = match received {
        Ok(v) => v,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            std::mem::forget(u);
            panic!("static_sym_at waited on Syms::static_spans for probe #{stalled_at}, outside the writable region (no answer in 5 s)");
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            std::mem::forget(u);
            panic!("the looking thread ended without its answers");
        }
    };
    for (a, s) in looked {
        assert_eq!(s, None, "{a:#x}, outside the writable region, was attributed to a static");
    }
    // with the lock free, the static's own address is attributed
    let id = u.syms.intern(SymKind::Static(s0), s0_addr).id;
    assert_eq!(u.static_sym_at(s0_addr), Some(Sym { id, off: 0 }));
    std::mem::forget(u);
}

/// A static's span covers exactly its bytes (a zero-sized one only its start), and a static whose
/// initializer is rejected leaves no span: its storage is attributed to nothing, on the owner and
/// through the optimiser's `ro_read` off it, without reading `Unit::statics`.
#[test]
fn static_sym_at_bounds_zst_and_failed_init_revocation() {
    isolated_test!(static_sym_at_bounds_zst_and_failed_init_revocation);
    let source = "#![no_std]\nstatic S: [u64; 2] = [1, 2];\nstatic Z: () = ();\nstatic FAIL: u64 = { let a = [0u64; 1]; a[2] };\nconst REF_TY_HOLDER: &u64 = &7;\n";
    let mut u = load("bounds", source, true);
    let (s, z, fail) = (def(&u, "S"), def(&u, "Z"), def(&u, "FAIL"));
    let ref_ty = u.tcx.const_ty(&u.prog, def(&u, "REF_TY_HOLDER")).expect("&u64");
    let up = &mut *u as *mut Unit as usize;
    let (s_buf, z_buf, before, fail_addr, after) = owner::call(owner::Class::Program, || {
        let u = unsafe { &mut *(up as *mut Unit) };
        let s_buf = u.static_addr(s);
        // writable memory no static owns: s_buf + 16 lies in the region but in no span
        let _gap = u.rw_data(8, 8);
        let z_buf = u.static_addr(z);
        let before = u.rw_next.load(Ordering::Acquire);
        let fail_addr = u.static_addr(fail);
        (s_buf, z_buf, before, fail_addr, u.rw_next.load(Ordering::Acquire))
    });
    assert_ne!(s_buf, 0, "S has no storage: {:?}", u.errors);
    assert_ne!(z_buf, 0, "Z has no storage: {:?}", u.errors);
    assert_eq!(fail_addr, 0, "FAIL's initializer was accepted");
    let fail_buf = (before + 7) & !7;
    assert_eq!(after, fail_buf + 8, "FAIL's storage is not the 8 bytes from {fail_buf:#x}");

    let s_id = u.syms.intern(SymKind::Static(s), s_buf).id;
    let z_id = u.syms.intern(SymKind::Static(z), z_buf).id;
    assert_eq!(u.static_sym_at(s_buf), Some(Sym { id: s_id, off: 0 }));
    assert_eq!(u.static_sym_at(s_buf + 15), Some(Sym { id: s_id, off: 15 }));
    assert!(u.in_rw(s_buf + 16, 1));
    assert_eq!(u.static_sym_at(s_buf + 16), None, "one past S's end");
    assert_eq!(u.static_sym_at(z_buf), Some(Sym { id: z_id, off: 0 }));
    assert!(u.in_rw(z_buf + 1, 1));
    assert_eq!(u.static_sym_at(z_buf + 1), None, "past the zero-sized Z's start");
    assert!(u.in_rw(fail_buf, 8));
    assert_eq!(u.static_sym_at(fail_buf), None, "the rejected FAIL's storage");

    // off the owner, through the optimiser's reader: S's word resolves (the control), FAIL's not
    let in_s = u.data(&(s_buf + 8).to_le_bytes(), 8, DataKind::Typed(ref_ty));
    let in_fail = u.data(&fail_buf.to_le_bytes(), 8, DataKind::Typed(ref_ty));
    let (in_s, in_fail) = (u.data_sym(in_s), u.data_sym(in_fail));
    let reader = std::thread::spawn(move || (crate::syms::ro_read(up, in_s, 8), crate::syms::ro_read(up, in_fail, 8)));
    let (read_s, read_fail) = match reader.join() {
        Ok(v) => v,
        Err(p) => std::panic::resume_unwind(p),
    };
    assert!(matches!(read_s, RoRead::Ptr(p) if p == Sym { id: s_id, off: 8 }), "a pointer into S read {}", show(&read_s));
    assert!(matches!(read_fail, RoRead::No), "a pointer to FAIL's storage read {}", show(&read_fail));
    std::mem::forget(u);
}

/// The startup prelink report carries what `run` logs (the startup image, the cache stats),
/// taken on the owner: it holds while the owner has the unit's cache out.
#[test]
fn prelink_startup_report_captures_stats_on_owner_before_notes_thread() {
    isolated_test!(prelink_startup_report_captures_stats_on_owner_before_notes_thread);
    let mut u = load("prelink", "#![no_std]\npub fn value() -> u32 { 3 }\n", false);
    let cache_dir = std::env::temp_dir().join(format!("relax-static-race-cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cache_dir);
    let up = &mut *u as *mut Unit as usize;
    owner::call(owner::Class::Program, || {
        let u = unsafe { &mut *(up as *mut Unit) };
        let cache = crate::cache::Cache::open_in(u, crate::cache::Mode::ReadOnly, cache_dir.clone()).unwrap();
        u.cache = Some(Box::new(crate::cache::CacheCell::new(cache)));
    });
    // asked off the owner, as run's main thread asks
    let report = unsafe { prelink_startup_report(up as *mut Unit) };

    let held = Arc::new(Barrier::new(2));
    let checked = Arc::new(Barrier::new(2));
    let holder = {
        let (held, checked) = (held.clone(), checked.clone());
        std::thread::spawn(move || owner::call(owner::Class::Program, || {
            let u = unsafe { &mut *(up as *mut Unit) };
            let taken = u.cache.take();
            held.wait();
            checked.wait();
            let had = taken.is_some();
            u.cache = taken;
            had
        }))
    };
    held.wait();
    // (the unit's cache is out on the owner now: the report is all this thread reads)
    let seen = (report.prelinked, report.has_startup_image, report.stats.is_some());
    checked.wait();
    let had_cache = match holder.join() {
        Ok(v) => v,
        Err(p) => std::panic::resume_unwind(p),
    };
    assert!(had_cache, "the cache was not installed");
    assert_eq!(seen, (0, false, true), "(prelinked, has_startup_image, stats taken)");
    std::mem::forget(u);
    let _ = std::fs::remove_dir_all(&cache_dir);
}
