//! A hot fn never ends a run below O3 with nothing pending. The one way a body can name what only
//! the owner may make: its promoted constants (`&CONST-EXPR`) and a new vtable (each method a slot
//! nobody made yet), which a background build's front off the owner got no value for: its O3 build
//! gave up for good (STUCK below O3). And a hot fn whose callees came from the disk cache inlines
//! them: none is left a call for want of a front made already. And O3 code a warm launch links
//! from the disk cache that kept a callee as a call (incomplete: built before the callee could be
//! inlined) is built again once it can be, as the build that stored it would have been.

use super::*;

const SOURCE: &str = r#"
const fn three() -> u32 { 3 }
const K: u32 = 40 + three();
fn table() -> &'static [u32; 4] { &[3, 1, 4, K] }
fn pieces(x: u32) -> u32 {
    assert!(x < 1 << 20, "out of range: {x}");
    assert!(x != 1 << 19, "not this one: {}", x);
    x
}
pub fn hot(out: &mut u32) {
    let t = table();
    let one: &'static u32 = &(1 + K);
    let mut i = 0u32;
    let mut s = 0u32;
    while i < 64 { s = s.wrapping_add(t[(i & 3) as usize]).wrapping_add(pieces(i)).wrapping_add(*one); i += 1; }
    *out = s;
}
pub fn clamped(out: &mut u32) {
    *out = 2.5f32.clamp(0.0, 1.0).to_bits() ^ 'z'.to_digit(36).unwrap();
}
trait Shape { fn a(&self) -> u32; fn b(&self) -> u32; fn c(&self) -> u32; fn d(&self) -> u32; fn e(&self) -> u32; fn f(&self) -> u32; }
struct Sq(u32);
impl Shape for Sq {
    fn a(&self) -> u32 { self.0 } fn b(&self) -> u32 { self.0 + 1 } fn c(&self) -> u32 { self.0 + 2 }
    fn d(&self) -> u32 { self.0 + 3 } fn e(&self) -> u32 { self.0 + 4 } fn f(&self) -> u32 { self.0 + 5 }
}
#[inline(never)]
fn total(s: &dyn Shape) -> u32 { s.a() ^ s.b() ^ s.c() ^ s.d() ^ s.e() ^ s.f() }
pub fn dynsum(out: &mut u32) {
    let q = Sq(*out);
    *out = total(&q);
}
fn leaf(x: u32) -> u32 { x.rotate_left(5) ^ 0x9e37 }
fn h1(x: u32) -> u32 { leaf(x).wrapping_add(1) }
fn h2(x: u32) -> u32 { leaf(x ^ 3).wrapping_mul(7) }
fn mid(v: &mut [u32; 16]) { let mut i = 0; while i < 16 { v[i] = h1(v[i]) ^ h2(i as u32); i += 1; } }
pub fn spin(out: &mut u32) {
    let mut v = [*out; 16];
    let mut k = 0;
    while k < 8 { mid(&mut v); k += 1; }
    *out = v[3] ^ v[11];
}
pub fn sorted(out: &mut u32) {
    let mut v = [0u32; 200];
    let mut i = 0;
    while i < 200 { v[i] = (i as u32).wrapping_mul(2654435761) >> 7; i += 1; }
    v.sort_by_key(|x| *x ^ 5);
    *out = v[0] ^ v[100].rotate_left(3) ^ v[199];
}
fn main() {}
"#;

#[test]
fn hot_fns_reach_o3() {
    isolated_test!(hot_fns_reach_o3);
    crate::borrowck::lazy::disable_gandalf();
    if std::env::var_os("MAKEPAD_RUN_STD").is_none() {
        std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    }
    let dir = std::env::temp_dir().join(format!("relax-tier-stuck-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("input.rs");
    std::fs::write(&path, SOURCE).unwrap();
    let (prog, core, krate) = crate::load_with_std(&std::sync::Arc::new(crate::Session::default()), path.to_str().unwrap(), "tier_stuck", false).unwrap();
    let def = |prog: &Program, n: &str| prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = DefId(i as u32);
        (d.krate == krate && prog.name(id) == n).then_some(id)
    }).unwrap();
    let (hot_d, clamped_d, dynsum_d) = (def(&prog, "hot"), def(&prog, "clamped"), def(&prog, "dynsum"));
    let (spin_d, sorted_d) = (def(&prog, "spin"), def(&prog, "sorted"));
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    let mut u = Unit::new(prog, tcx, core);
    let up = &mut *u as *mut Unit;
    unsafe { UNIT = up };
    assert!(u.tier1.is_some(), "the fixture needs O3");
    // as `makepad run` builds: candidate fronts off the owner (enable_pipelining)
    enable_pipelining(&u.prog.session.options);

    let run = |id: u32| -> u32 {
        let entry = unsafe { (&*up).fns[id as usize].code };
        let mut r = 0u32;
        unsafe { call_jit(up, entry, &mut r as *mut u32 as u64) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        r
    };
    // baseline code without RIR, turned hot: what a slot linked from the disk cache is when it heats up
    let ids: Vec<u32> = crate::owner::call(crate::owner::Class::Program, || {
        [hot_d, clamped_d].iter().map(|d| {
            let id = u.fn_id(FnKey::Inst(*d, Vec::new()));
            let c = unsafe { compile_fn(up, id) }.unwrap();
            u.install(id, &c.0, &c.2, c.1, 0).unwrap();
            id
        }).collect()
    });
    let expected: Vec<u32> = ids.iter().map(|&id| run(id)).collect();
    crate::owner::call(crate::owner::Class::Program, || {
        for &id in &ids {
            u.fns[id as usize].rir = None;
            u.fns[id as usize].o3.store(false, Ordering::Relaxed);
            unsafe { (*tier_rec(&u.slots, id).add(3)).store(T_QUEUED, Ordering::Release) };
            let FnKey::Inst(d, args) = u.fns[id as usize].key.clone() else { unreachable!() };
            u.front.unbuild(&(d, args));
        }
        // the census sees them: built by nothing, below O3
        let stuck = tier_stuck(&u);
        for &id in &ids {
            assert!(stuck.iter().any(|s| s.0 == id), "slot {} is below O3 with nothing pending: {:?}", u.fns[id as usize].name, tier_explain(&u, id).0);
        }
    });
    // a front whose body makes a new vtable, each method a slot nobody made yet (code from the disk
    // cache made none of them): the owner makes the vtable in its drain, the front is made, once
    let dynsum = crate::owner::call(crate::owner::Class::Program, || u.fn_id(FnKey::Inst(dynsum_d, Vec::new())));
    let front = crate::owner::call(crate::owner::Class::Program, || u.front.clone());
    assert!(prefront_slot(&front, dynsum), "the front of dynsum (a new 6-method vtable) was not made off the owner");
    // their O3 builds, off the owner as a tier worker runs them (rule I-PURE: the owner evaluates
    // the promoted constants between requests, the front lowers the body with their values)
    for &id in &ids {
        let name = crate::owner::call(crate::owner::Class::Program, || u.fns[id as usize].name.to_string());
        assert!(unsafe { compile_tier1(up, id) }, "the O3 build of {} gave up", name);
    }
    crate::owner::call(crate::owner::Class::Program, || {
        for &id in &ids {
            let k = crate::inspect::codever::kind_at(&u, u.fns[id as usize].code);
            assert_eq!(k.tier_name(), "O3", "{}", u.fns[id as usize].name);
            let note = O3_BUILD_NOTES.lock().unwrap_or_else(|e| e.into_inner()).get(&(&*u as *const Unit as usize, id)).cloned().unwrap_or_default();
            assert!(!note.contains("incomplete"), "{}: {}", u.fns[id as usize].name, note);
        }
        let stuck = tier_stuck(&u);
        assert!(stuck.is_empty(), "stuck below O3: {:?}", stuck.iter().map(|s| (u.fns[s.0 as usize].name.to_string(), s.1.clone())).collect::<Vec<_>>());
    });
    assert_eq!(ids.iter().map(|&id| run(id)).collect::<Vec<u32>>(), expected, "O3 code must compute what baseline code did");
    // an instance's promoted constants are one object, evaluated once by the owner for the slot's
    // body: the callees' (table, pieces, f32::clamp, char::to_digit) whose bodies the O3 builds took
    crate::owner::call(crate::owner::Class::Program, || {
        let epoch = u.tcx.collect_epoch.load(Ordering::SeqCst);
        let named: Vec<String> = u.front.body_consts.snapshot().keys().filter(|k| k.0 == epoch && !k.3).map(|k| u.fns[k.1 as usize].name.to_string()).collect();
        for want in ["tier_stuck::table", "tier_stuck::pieces", "f32::clamp", "char::to_digit"] {
            assert!(named.iter().any(|n| n.ends_with(want)), "no promoted value evaluated for {}: {:?}", want, named);
        }
    });
    // hot fns whose callees came from the disk cache (code, no RIR): their O3 builds make the
    // callees' fronts and O1 builds and inline them. Two candidates that inline one callee (h1 and
    // h2 both call leaf; sort's stable_partition and small_sort share theirs) shared its one
    // lowering ahead, and a candidate that gave back lost those it took: needed again, never made
    // again, the candidate stayed a call for the run (sort_by_key 2.8x slower on warm launches)
    let warm: Vec<u32> = crate::owner::call(crate::owner::Class::Program, || {
        [spin_d, sorted_d].iter().map(|d| {
            let id = u.fn_id(FnKey::Inst(*d, Vec::new()));
            let c = unsafe { compile_fn(up, id) }.unwrap();
            u.install(id, &c.0, &c.2, c.1, 0).unwrap();
            id
        }).collect()
    });
    let warm_expected: Vec<u32> = warm.iter().map(|&id| run(id)).collect();
    let roots: Vec<u32> = crate::owner::call(crate::owner::Class::Program, || {
        // every slot as a warm launch links it: its code kept, no RIR, no front made
        for id in 0..u.fns.len() as u32 {
            if u.fns[id as usize].rir.take().is_some() {
                if let FnKey::Inst(d, args) = u.fns[id as usize].key.clone() {
                    u.front.unbuild(&(d, args));
                }
            }
        }
        let quicksort = (0..u.fns.len() as u32).find(|&id| u.fns[id as usize].compiled && u.fns[id as usize].name.ends_with("quicksort::quicksort"))
            .expect("sorting 200 keys runs the stable quicksort");
        let roots = vec![warm[0], quicksort];
        for &id in &roots {
            u.fns[id as usize].o3.store(false, Ordering::Relaxed);
            unsafe { (*tier_rec(&u.slots, id).add(3)).store(T_QUEUED, Ordering::Release) };
        }
        roots
    });
    for &id in &roots {
        assert!(unsafe { compile_tier1(up, id) }, "the O3 build of slot {} gave up", id);
    }
    crate::owner::call(crate::owner::Class::Program, || {
        for &id in &roots {
            let k = crate::inspect::codever::kind_at(&u, u.fns[id as usize].code);
            assert_eq!(k.tier_name(), "O3", "{}", u.fns[id as usize].name);
            // (a candidate left out for want of a front: "compiled, no RIR, not building")
            let note = O3_BUILD_NOTES.lock().unwrap_or_else(|e| e.into_inner()).get(&(&*u as *const Unit as usize, id)).cloned().unwrap_or_default();
            assert!(!note.contains("NOT built again"), "{} left a callee from the cache a call: {}", u.fns[id as usize].name, note);
        }
    });
    assert_eq!(warm.iter().map(|&id| run(id)).collect::<Vec<u32>>(), warm_expected, "O3 code must compute what the cached code did");
    // (process exit owns the runtime's teardown)
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

/// The stable quicksort's O3 build keeps the sort's closure as a call (its verdict pending: its
/// code unverified) and is stored so; the next launch links that code, the closure's verdict clean
/// by then. Its O3 build turns the closure hot: the linked quicksort is queued again (a retier) and
/// its build inlines the closure. Before, the linked code was final: a warm sort_by_key ran the
/// cold run's quicksort for good, 2.3x slower than with the disk cache off.
#[test]
fn linked_incomplete_o3_is_built_again() {
    const PHASE: &str = "MAKEPAD_RUN_LINKED_INCOMPLETE_O3_TEST";
    const DIR: &str = "MAKEPAD_RUN_LINKED_INCOMPLETE_O3_TEST_DIR";
    let Ok(phase) = std::env::var(PHASE) else {
        let dir = std::env::temp_dir().join(format!("relax-linked-incomplete-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("input.rs"), SOURCE).unwrap();
        let mut said = String::new();
        for phase in ["cold", "warm"] {
            let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "jit::tier_stuck_tests::linked_incomplete_o3_is_built_again", "--nocapture", "--test-threads=1"])
                .env(PHASE, phase).env(DIR, &dir).env("MAKEPAD_RUN_SPEC", "0")
                // (nothing turns hot by its counts: the O3 builds are the test's own, in order)
                .env("MAKEPAD_RUN_O3_CALLS", "1000000000"));
            said += &format!("{phase}: {}\n{}\n", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            assert!(output.status.success(), "{said}");
        }
        std::fs::remove_dir_all(dir).unwrap();
        return;
    };
    if std::env::var_os("MAKEPAD_RUN_STD").is_none() {
        std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    }
    let dir = std::path::PathBuf::from(std::env::var_os(DIR).unwrap());
    if phase == "cold" {
        // the input is the app: its bodies' verdicts are held (pending), so the key closure's code
        // is unverified and the verified sort calls it (unverified code is not inlined into it). A
        // flag set by hand was cleared by the closure's next O1 build (its verdict read clean)
        SPECV_HOLD.store(1, Ordering::Relaxed);
        CHECK_FIRST.store(false, Ordering::Relaxed);
        crate::borrowck::event::set_app_scope(vec![dir.join("input.rs").to_string_lossy().into_owned()]);
    } else {
        crate::borrowck::lazy::disable_gandalf();
    }
    std::env::set_var("MAKEPAD_RUN_CACHE_DIR", dir.join("cache"));
    let (prog, core, krate) = crate::load_with_std(&std::sync::Arc::new(crate::Session::default()), dir.join("input.rs").to_str().unwrap(), "tier_stuck", false).unwrap();
    let sorted_d = prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = DefId(i as u32);
        (d.krate == krate && prog.name(id) == "sorted").then_some(id)
    }).unwrap();
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    let mut u = Unit::new(prog, tcx, core);
    let up = &mut *u as *mut Unit;
    unsafe { UNIT = up };
    enable_pipelining(&u.prog.session.options);
    let sorted = crate::owner::call(crate::owner::Class::Program, || {
        let mut cache = crate::cache::Cache::open(&u, crate::cache::Mode::ReadOnly).unwrap();
        cache.mode = crate::cache::Mode::On;
        u.cache = Some(Box::new(crate::cache::CacheCell::new(cache)));
        let id = u.fn_id(FnKey::Inst(sorted_d, Vec::new()));
        let c = unsafe { compile_fn(up, id) }.unwrap();
        u.install(id, &c.0, &c.2, c.1, 0).unwrap();
        id
    });
    let run = || -> u32 {
        let entry = unsafe { (&*up).fns[sorted as usize].code };
        let mut r = 0u32;
        unsafe { call_jit(up, entry, &mut r as *mut u32 as u64) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        r
    };
    let result = run();
    // the stable quicksort and the closure it calls (through sort_by_key's comparator); (the
    // checks read on the owner, asserted here: a panic on the owner prints nothing)
    let (quicksort, key_fn) = crate::owner::call(crate::owner::Class::Program, || {
        let q = (0..u.fns.len() as u32).find(|&id| u.fns[id as usize].compiled && u.fns[id as usize].name.ends_with("quicksort::quicksort"));
        let k = (0..u.fns.len() as u32).find(|&id| matches!(u.fns[id as usize].key, FnKey::Closure(_, _, p) if p == sorted));
        (q, k)
    });
    let (quicksort, key_fn) = (quicksort.expect("sorting 200 keys runs the stable quicksort"), key_fn.expect("the key closure has a slot"));
    let note = |u: &Unit, id: u32| O3_BUILD_NOTES.lock().unwrap_or_else(|e| e.into_inner()).get(&(u as *const Unit as usize, id)).cloned().unwrap_or_default();
    if phase == "cold" {
        // (every RIR dropped: the O3 build makes its candidates' fronts as a warm launch's does)
        crate::owner::call(crate::owner::Class::Program, || {
            for id in 0..u.fns.len() as u32 {
                if u.fns[id as usize].rir.take().is_some() {
                    if let FnKey::Inst(d, args) = u.fns[id as usize].key.clone() {
                        u.front.unbuild(&(d, args));
                    }
                }
            }
        });
        // (no worker builds a candidate's RIR beside this build: one that came meanwhile is kept a
        // call, and the walk does not reach the closure under it)
        queue::stop_workers(std::time::Duration::from_secs(10));
        assert!(unsafe { compile_tier1(up, quicksort) }, "the quicksort's O3 build gave up");
        let (n, spill) = crate::owner::call(crate::owner::Class::Program, || {
            u.flush_cache_records();
            let mut cache = u.cache.take().unwrap();
            let spill = cache.spill_take();
            u.cache = Some(cache);
            (note(&u, quicksort), spill)
        });
        assert!(n.contains("incomplete") && n.contains("unverified: again"), "the fixture: the quicksort keeps the unverified closure a call: {n}");
        eprintln!("cold: the quicksort's O3 build: {n}");
        assert!(spill.expect("records to write").write().unwrap() > 0);
        std::fs::write(dir.join("result"), result.to_string()).unwrap();
    } else {
        assert_eq!(result.to_string(), std::fs::read_to_string(dir.join("result")).unwrap(), "the linked code computes what the cold run's did");
        let (linked, waits, why) = crate::owner::call(crate::owner::Class::Program, || {
            let why = format!("closure #{key_fn} at O3 {}, compiled {}, quicksort #{quicksort} retiers {:?}; kept users {:?}; unverified left {:?}",
                u.fns[key_fn as usize].o3.load(Ordering::Relaxed), u.fns[key_fn as usize].compiled, u.retiers.get(&quicksort), u.kept_users,
                UNVERIFIED_LEFT.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|x| (x.1, x.2)).collect::<Vec<_>>());
            (u.fns[quicksort as usize].o3.load(Ordering::Relaxed) && u.fns[quicksort as usize].rir.is_none(), u.kept_users.get(&key_fn).is_some_and(|v| v.contains(&quicksort)), why)
        });
        assert!(linked, "the quicksort links its O3 code from the cache");
        assert!(waits, "the linked quicksort waits for the closure it kept a call: {why}");
        // (the retier below is queued: no worker may build it beside this thread's build)
        queue::stop_workers(std::time::Duration::from_secs(10));
        // the closure turns hot: its O3 build queues the linked quicksort again
        assert!(unsafe { compile_tier1(up, key_fn) }, "the closure's O3 build gave up");
        let retiers = crate::owner::call(crate::owner::Class::Program, || u.retiers.get(&quicksort).copied());
        assert_eq!(retiers, Some(1), "the linked quicksort is built again once the closure is hot");
        assert!(unsafe { compile_tier1(up, quicksort) }, "the quicksort's O3 build gave up");
        let (tier, inlined, n) = crate::owner::call(crate::owner::Class::Program, || {
            let k = crate::inspect::codever::kind_at(&u, u.fns[quicksort as usize].code);
            (k.tier_name(), u.fns[key_fn as usize].history().inlined_into.contains(&quicksort), note(&u, quicksort))
        });
        assert_eq!(tier, "O3");
        assert!(inlined, "the quicksort's build again inlines the closure: {n}");
        assert_eq!(run(), result, "O3 code must compute what the linked code did");
    }
    // (process exit owns the runtime's teardown)
    std::mem::forget(u);
}
