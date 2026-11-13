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
    if std::env::var_os("RELAX_STD").is_none() {
        std::env::set_var("RELAX_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
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

const OPAQUE_SOURCE: &str = r#"
pub struct Handle { x: u32 }
impl Handle {
    pub fn get(&self) -> u32 { self.x.rotate_left(3) ^ 7 }
}
impl Clone for Handle {
    fn clone(&self) -> Self { Handle { x: self.x } }
}
fn main() {
    let h = Handle { x: 5 };
    let _ = h.clone().get();
}
"#;

/// A provided crate's opaque type (as the loader binds makepad-script's ScriptHeap) keeps the
/// source bodies of its inherent methods: the owner's build lowers them, so their fronts are made
/// off the owner too. They were refused there as if every impl's method were bridged, and the O3
/// builds of their hot callers left them calls for the rest of the run ("its front is not made off
/// the owner: refused": ScriptHeap::heap_key and ::value under a text view's draw).
#[test]
fn an_opaque_types_own_methods_are_made_off_the_owner() {
    isolated_test!(an_opaque_types_own_methods_are_made_off_the_owner);
    crate::borrowck::lazy::disable_gandalf();
    if std::env::var_os("RELAX_STD").is_none() {
        std::env::set_var("RELAX_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    }
    let dir = std::env::temp_dir().join(format!("relax-opaque-front-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("input.rs");
    std::fs::write(&path, OPAQUE_SOURCE).unwrap();
    let (prog, core, krate) = crate::load_with_std(&std::sync::Arc::new(crate::Session::default()), path.to_str().unwrap(), "opaque_front", false).unwrap();
    let def = |prog: &Program, n: &str, kind: DefKind| prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = DefId(i as u32);
        (d.krate == krate && d.kind == kind && prog.name(id) == n).then_some(id)
    }).unwrap();
    let (handle_d, get_d, clone_d) = (def(&prog, "Handle", DefKind::Struct), def(&prog, "get", DefKind::AssocFn), def(&prog, "clone", DefKind::AssocFn));
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    let mut u = Unit::new(prog, tcx, core);
    // the fixture's crate provided as the loader provides makepad-script: Handle an opaque handle
    // laid out as rustc does, no op listed
    unsafe extern "C" fn no_panic_text(_: *mut *const u8, _: *mut usize) {}
    let fixture = crate::provided::ProvidedCrate { name: "opaque_front".into(), shims: Default::default(), types: Default::default(), layouts: Default::default(), per_side: Default::default(), last_panic: no_panic_text, source: 0, binding: 0, manifest: &[] };
    u.provided.push((krate, fixture));
    u.provided_types.insert(handle_d, 0);
    assert!(u.provided_rustc_layout(handle_d));
    // the owner's build lowers both from source (no bridge, no forward), so the front may as well
    for (d, what) in [(get_d, "an inherent method"), (clone_d, "a Clone its attribute does not list")] {
        assert!(matches!(unsafe { provided_fmt_hash(&mut u, d, &[], 0) }, Ok(None)), "{what}: the owner's build is its source body");
        assert!(provided_source_body(&u, d), "{what}: the owner lowers its source body, and the front was refused");
    }
    let up = &mut *u as *mut Unit;
    unsafe { UNIT = up };
    enable_pipelining(&u.prog.session.options);
    let get = crate::owner::call(crate::owner::Class::Program, || u.fn_id(FnKey::Inst(get_d, Vec::new())));
    let front = crate::owner::call(crate::owner::Class::Program, || u.front.clone());
    assert!(prefront_slot(&front, get), "the front of Handle::get was not made off the owner");
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
    const PHASE: &str = "RELAX_LINKED_INCOMPLETE_O3_TEST";
    const DIR: &str = "RELAX_LINKED_INCOMPLETE_O3_TEST_DIR";
    let Ok(phase) = std::env::var(PHASE) else {
        let dir = std::env::temp_dir().join(format!("relax-linked-incomplete-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("input.rs"), SOURCE).unwrap();
        let mut said = String::new();
        for phase in ["cold", "warm"] {
            let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "jit::tier_stuck_tests::linked_incomplete_o3_is_built_again", "--nocapture", "--test-threads=1"])
                .env(PHASE, phase).env(DIR, &dir).env("RELAX_SPEC", "0")
                // (nothing turns hot by its counts: the O3 builds are the test's own, in order)
                .env("RELAX_O3_CALLS", "1000000000"));
            said += &format!("{phase}: {}\n{}\n", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            assert!(output.status.success(), "{said}");
        }
        std::fs::remove_dir_all(dir).unwrap();
        return;
    };
    if std::env::var_os("RELAX_STD").is_none() {
        std::env::set_var("RELAX_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
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
    std::env::set_var("RELAX_CACHE_DIR", dir.join("cache"));
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

const TLS_SOURCE: &str = r#"
use std::collections::HashMap;
fn main() {
    let mut m: HashMap<u32, u32> = HashMap::new();
    m.insert(1, 2);
}
"#;

/// A body that reads a `#[thread_local]` static (core's hash keys: every HashMap::new) is made off
/// the owner after the settle rebuild too. The rebuild gives the Unit a new front state, which has
/// none of the places the owner made before it; the owner's drain placed the static again for a
/// front that asked, found it placed already and published nothing, so every such front failed
/// for the rest of the run ("needs unit" at the place, all its tries) and the hot callers' O3
/// builds kept next_keys a call.
#[test]
fn a_thread_local_readers_front_is_made_after_the_settle_rebuild() {
    isolated_test!(a_thread_local_readers_front_is_made_after_the_settle_rebuild);
    crate::borrowck::lazy::disable_gandalf();
    if std::env::var_os("RELAX_STD").is_none() {
        std::env::set_var("RELAX_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    }
    let dir = std::env::temp_dir().join(format!("relax-tls-front-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("input.rs");
    std::fs::write(&path, TLS_SOURCE).unwrap();
    let (prog, core, _) = crate::load_with_std(&std::sync::Arc::new(crate::Session::default()), path.to_str().unwrap(), "tls_front", false).unwrap();
    let core_def = |prog: &Program, n: &str, kind: DefKind| prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = DefId(i as u32);
        (d.krate == core && d.kind == kind && prog.name(id) == n && prog.def_path(id).contains("random")).then_some(id)
    }).unwrap();
    let (next_keys, keys) = (core_def(&prog, "next_keys", DefKind::Fn), core_def(&prog, "KEYS", DefKind::Static));
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    let mut u = Unit::new(prog, tcx, core);
    let up = &mut *u as *mut Unit;
    unsafe { UNIT = up };
    enable_pipelining(&u.prog.session.options);
    let id = crate::owner::call(crate::owner::Class::Program, || u.fn_id(FnKey::Inst(next_keys, Vec::new())));
    // the owner placed KEYS before the rebuild (the first HashMap's build), then the rebuild's
    // replace: a new front state, the Unit's places kept
    crate::owner::call(crate::owner::Class::Program, || {
        assert!(u.thread_local_place(keys).is_some(), "KEYS is placed on the owner");
        let front = FrontShared::new(&u.prog.session);
        for (i, f) in u.fns.iter().enumerate() {
            front.slot_keys.insert(i as u32, f.key.clone());
        }
        u.front.retire_memos();
        queue::retire_front(&u.front);
        u.front = std::sync::Arc::new(front);
        set_shared_front(&mut u, core);
    });
    let front = crate::owner::call(crate::owner::Class::Program, || u.front.clone());
    assert!(prefront_slot(&front, id), "the front of next_keys was not made off the owner after the rebuild");
    // (process exit owns the runtime's teardown)
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

const TAKE_SOURCE: &str = r#"
pub fn twice(x: u32) -> u32 { x.wrapping_mul(2) }
fn main() {
    let _ = twice(3);
}
"#;

/// A take that bails gives back the lowerings its preparations took (TakenBack), also one taken for
/// the normal body of a slot whose RIR is an O2 build's full body: callers below O2 inline that
/// normal body, built from a fresh lowering. The give-back asked only whether the slot had RIR,
/// kept it taken, and the next round needed its front made again: an O3 build left the candidate
/// out once the tries were spent (a large app's draw: i32::saturating_add, a damage walk's closure).
#[test]
fn a_bailed_take_gives_back_a_full_bodys_normal_lowering() {
    isolated_test!(a_bailed_take_gives_back_a_full_bodys_normal_lowering);
    crate::borrowck::lazy::disable_gandalf();
    if std::env::var_os("RELAX_STD").is_none() {
        std::env::set_var("RELAX_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    }
    let dir = std::env::temp_dir().join(format!("relax-take-back-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("input.rs");
    std::fs::write(&path, TAKE_SOURCE).unwrap();
    let (prog, core, krate) = crate::load_with_std(&std::sync::Arc::new(crate::Session::default()), path.to_str().unwrap(), "take_back", false).unwrap();
    let twice = prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = DefId(i as u32);
        (d.krate == krate && d.kind == DefKind::Fn && prog.name(id) == "twice").then_some(id)
    }).unwrap();
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    let mut u = Unit::new(prog, tcx, core);
    let up = &mut *u as *mut Unit;
    unsafe { UNIT = up };
    crate::owner::call(crate::owner::Class::Program, || {
        let u = unsafe { &mut *up };
        let id = u.fn_id(FnKey::Inst(twice, Vec::new()));
        let file = u.fns[id as usize].file;
        // an O2 build's full body, no normal body beside it, and the lowering made ahead for that
        let mut full = relax_backend::rir::Func::new("twice".into(), file);
        full.full = true;
        u.fns[id as usize].rir = Some(std::sync::Arc::new(full));
        assert!(needs_normal(&u.fns[id as usize]));
        give_back_lowered(u, id, relax_backend::rir::Func::new("twice".into(), file));
        {
            let _take = TakenBack::start();
            let f = take_lowered_ahead(u, id).expect("the lowering made ahead is taken");
            TakenBack::note(up, id, f);
            // (the take bails here: a later candidate needs a front)
        }
        assert!(matches!(u.front.lowered.lock().unwrap_or_else(|e| e.into_inner()).get(&id), Some(LoweredState::Ready(_))),
            "the bailed take kept the lowering for the full body's normal one: its next round needs the front made again");
    });
    // (process exit owns the runtime's teardown)
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

const LEFT_SOURCE: &str = r#"
const K1: u32 = 40 + 2;
const K2: u32 = 6 * 7;
extern "C" {
    fn relax_left_out_fixture_absent(x: u32) -> u32;
}
pub fn leaf(x: u32) -> u32 { if x == 0 { unsafe { relax_left_out_fixture_absent(x) } } else { x ^ 7 } }
pub fn mid(x: u32) -> u32 { leaf(x).wrapping_add(K2) }
pub fn root(x: u32) -> u32 { mid(x).wrapping_add(K1) }
fn main() {
    let _ = root(1);
}
"#;

/// An O3 build that leaves an inline candidate out is built again once per thing it waited on, and
/// never in a loop. One that waits on an item its fronts asked the owner's drain for is queued
/// again when the drain makes that item, not when a drain makes anything else (a large app's
/// decoder build was queued after every drain that made anything: 480 builds in a run, each leaving
/// the same allocator calls out). One whose front cannot be made off the owner and asked for
/// nothing (a call of a C function no loaded image defines: only the owner loads the program's
/// libraries) has that slot built on the owner once and is queued once; a later left-out needing
/// the same slot waits for nothing.
#[test]
fn a_left_out_build_is_built_again_once_per_thing_it_waited_on() {
    isolated_test!(a_left_out_build_is_built_again_once_per_thing_it_waited_on);
    crate::borrowck::lazy::disable_gandalf();
    if std::env::var_os("RELAX_STD").is_none() {
        std::env::set_var("RELAX_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    }
    let dir = std::env::temp_dir().join(format!("relax-left-out-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("input.rs");
    std::fs::write(&path, LEFT_SOURCE).unwrap();
    let (prog, core, krate) = crate::load_with_std(&std::sync::Arc::new(crate::Session::default()), path.to_str().unwrap(), "left_out", false).unwrap();
    let def = |prog: &Program, n: &str, kind: DefKind| prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = DefId(i as u32);
        (d.krate == krate && d.kind == kind && prog.name(id) == n).then_some(id)
    }).unwrap();
    let (root_d, mid_d, leaf_d) = (def(&prog, "root", DefKind::Fn), def(&prog, "mid", DefKind::Fn), def(&prog, "leaf", DefKind::Fn));
    let (k1, k2) = (def(&prog, "K1", DefKind::Const), def(&prog, "K2", DefKind::Const));
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    let mut u = Unit::new(prog, tcx, core);
    let up = &mut *u as *mut Unit;
    unsafe { UNIT = up };
    let (root, mid, leaf) = crate::owner::call(crate::owner::Class::Program, || (u.fn_id(FnKey::Inst(root_d, Vec::new())), u.fn_id(FnKey::Inst(mid_d, Vec::new())), u.fn_id(FnKey::Inst(leaf_d, Vec::new()))));
    crate::inspect::tierlog::init();
    // (the owner runs the drain steps between its requests: each call below ends with them; a
    // worker may take a queued build at once, so the decision is read from the tier log)
    let retiers = |id: u32| crate::inspect::tierlog::history(id).0.map_or(0, |h| h.n[crate::inspect::tierlog::ev::RETIER as usize]);
    let drain = |w: Wanted| {
        let epoch = crate::owner::call(crate::owner::Class::Program, || u.tcx.collect_epoch.load(Ordering::SeqCst));
        want(w, epoch);
        crate::owner::call(crate::owner::Class::Job, const_drain);
        crate::owner::call(crate::owner::Class::Program, || ());
    };
    let epoch = crate::owner::call(crate::owner::Class::Program, || u.tcx.collect_epoch.load(Ordering::SeqCst));

    // root's build left mid out, its fronts having asked the drain for K1
    assert!(o3_left_out(up, root, mid, &[mid], &HashMap::new(), &[wanted_key(&Wanted::Const(k1, Vec::new()), epoch)]), "mid has no RIR: its build waits on K1");
    crate::owner::call(crate::owner::Class::Program, || ());
    assert_eq!(retiers(root), 0, "built again before the item it waits on was made");
    drain(Wanted::Const(k2, Vec::new()));
    assert_eq!(retiers(root), 0, "built again after a drain that made something it did not wait on (K2)");
    drain(Wanted::Const(k1, Vec::new()));
    assert_eq!(retiers(root), 1, "not built again once the drain made K1, the item it waited on");
    drain(Wanted::Const(k1, Vec::new()));
    drain(Wanted::Const(k2, Vec::new()));
    assert_eq!(retiers(root), 1, "built again by later drains: one item it waited on builds it once");

    // leaf's front cannot be made off the owner, and it asks the drain for nothing
    let front = crate::owner::call(crate::owner::Class::Program, || u.front.clone());
    let notes = want_notes();
    let _ = PREFRONT_WHY.try_with(|w| w.set(AF_OK));
    assert!(!prefront_slot(&front, leaf), "the fixture's front was made off the owner: it must need the unit");
    let why = PREFRONT_WHY.try_with(|w| w.get()).unwrap_or(AF_OK);
    assert_eq!(want_notes(), notes, "the fixture's front asked the drain for something: it must need the owner itself");
    // mid's build leaves leaf out: leaf is built on the owner, then the build is queued, once
    let unmade: HashMap<u32, u8> = [(leaf, why)].into_iter().collect();
    assert!(o3_left_out(up, mid, leaf, &[leaf], &unmade, &[]), "leaf has no RIR: its build waits on the owner");
    crate::owner::call(crate::owner::Class::Program, || ());
    assert_eq!(retiers(mid), 1, "the build that waits on the owner is not queued again after the owner's drain");
    assert!(inline_body(&u.fns[leaf as usize]).is_some(), "the owner did not build the slot the left-out build needed");
    drain(Wanted::Const(k2, Vec::new()));
    assert_eq!(retiers(mid), 1, "built again by a later drain: the owner's build of its slot queues it once");
    // another build needing the same slot again (made, and needed again) waits for nothing more
    assert!(!o3_left_out(up, root, mid, &[leaf], &unmade, &[]), "a left-out needing a slot the owner built already waits again");
    crate::owner::call(crate::owner::Class::Program, || ());
    assert_eq!(retiers(root), 1, "queued again for a slot the owner built already: a loop");
    // (process exit owns the runtime's teardown)
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

const TWIN_SOURCE: &str = r#"
pub struct S { a: Vec<u32> }
pub fn inner(s: &mut S, t: &S) -> u32 {
    s.a.push(t.a.len() as u32);
    s.a.len() as u32
}
pub fn outer(s: &mut S, t: &S) -> u32 {
    inner(s, t)
}
fn main() {
    let mut s = S { a: Vec::new() };
    let t = S { a: vec![1] };
    let _ = outer(&mut s, &t);
}
"#;

/// SPEC-V: an unverified-mode twin's front is made off the owner when it calls (aliasing-prone) a
/// callee that has no slot yet: its call goes to the callee's twin, named by a temporary reference
/// the take makes after the callee's slot. Before, the lowering needed the unit and nothing made
/// the callee's slot, so the twin's front failed for the rest of the run (a large app: a glyph lookup's
/// twin, whose base inlines the callee) and its hot caller's O3 build left it out every time.
#[test]
fn a_twins_front_is_made_when_its_callee_has_no_slot_yet() {
    isolated_test!(a_twins_front_is_made_when_its_callee_has_no_slot_yet);
    crate::borrowck::lazy::disable_gandalf();
    if std::env::var_os("RELAX_STD").is_none() {
        std::env::set_var("RELAX_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    }
    let dir = std::env::temp_dir().join(format!("relax-twin-front-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("input.rs");
    std::fs::write(&path, TWIN_SOURCE).unwrap();
    let (prog, core, krate) = crate::load_with_std(&std::sync::Arc::new(crate::Session::default()), path.to_str().unwrap(), "twin_front", false).unwrap();
    let def = |prog: &Program, n: &str| prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = DefId(i as u32);
        (d.krate == krate && d.kind == DefKind::Fn && prog.name(id) == n).then_some(id)
    }).unwrap();
    let (outer_d, inner_d) = (def(&prog, "outer"), def(&prog, "inner"));
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    let mut u = Unit::new(prog, tcx, core);
    let up = &mut *u as *mut Unit;
    unsafe { UNIT = up };
    // outer and its twin have slots, inner none yet
    let (outer, twin) = crate::owner::call(crate::owner::Class::Program, || {
        let outer = u.fn_id(FnKey::Inst(outer_d, Vec::new()));
        (outer, u.fn_id(FnKey::Plain(outer)))
    });
    let inner_key = FnKey::Inst(inner_d, Vec::new());
    assert!(u.fn_map.get(&inner_key).is_none(), "the fixture's callee must have no slot yet");
    let front = crate::owner::call(crate::owner::Class::Program, || u.front.clone());
    assert!(prefront_slot(&front, twin), "the twin's front was not made off the owner: its callee had no slot");
    // the take makes the callee's slot, then its twin, and the twin's call goes there
    let func = crate::owner::call(crate::owner::Class::Program, || take_lowered_ahead(&mut u, twin)).expect("the twin's lowering is taken");
    let inner = u.fn_map.get(&inner_key).expect("the take made the callee's slot");
    let inner_twin = u.fn_map.get(&FnKey::Plain(inner)).expect("the take made the callee's twin");
    let calls: Vec<u32> = func.blocks.iter().flat_map(|b| b.insts.iter()).filter_map(|i| match i {
        relax_backend::rir::Inst::Call(relax_backend::rir::Callee::Fn(c), _, _) => Some(*c),
        _ => None,
    }).collect();
    assert!(calls.contains(&inner_twin), "the twin's call does not go to its callee's twin: {calls:?} (callee #{inner}, its twin #{inner_twin}, outer #{outer})");
    // (process exit owns the runtime's teardown)
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

const CLOSURE_TWIN_SOURCE: &str = r#"
pub struct S { a: Vec<u32> }
pub fn inner(s: &mut S, t: &S) -> u32 {
    s.a.push(t.a.len() as u32);
    s.a.len() as u32
}
pub fn outer(s: &mut S, t: &S) -> u32 {
    let mut f = |k: u32| inner(s, t) + k;
    f(1)
}
fn main() {
    let mut s = S { a: Vec::new() };
    let t = S { a: vec![1] };
    let _ = outer(&mut s, &t);
}
"#;

/// SPEC-V: a closure's unverified-mode twin gets its front off the owner, lowered from its parent's
/// checked body in twin mode (its aliasing-prone call goes to the callee's twin). Before, the twin's
/// front took an instance's body only and failed ("no body") for the rest of the run, so the O3
/// build of a hot caller that inlines the closure's twin left it out (a large app: a view's draw closures).
#[test]
fn a_closures_twin_gets_its_front_off_the_owner() {
    isolated_test!(a_closures_twin_gets_its_front_off_the_owner);
    crate::borrowck::lazy::disable_gandalf();
    if std::env::var_os("RELAX_STD").is_none() {
        std::env::set_var("RELAX_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    }
    let dir = std::env::temp_dir().join(format!("relax-closure-twin-front-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("input.rs");
    std::fs::write(&path, CLOSURE_TWIN_SOURCE).unwrap();
    let (prog, core, krate) = crate::load_with_std(&std::sync::Arc::new(crate::Session::default()), path.to_str().unwrap(), "closure_twin_front", false).unwrap();
    let outer_d = prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = DefId(i as u32);
        (d.krate == krate && d.kind == DefKind::Fn && prog.name(id) == "outer").then_some(id)
    }).unwrap();
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    let mut u = Unit::new(prog, tcx, core);
    let up = &mut *u as *mut Unit;
    unsafe { UNIT = up };
    // outer built (its lowering makes the closure's slot), then the closure's twin
    let (closure, twin) = crate::owner::call(crate::owner::Class::Program, || {
        let outer = u.fn_id(FnKey::Inst(outer_d, Vec::new()));
        unsafe { compile_fn(up, outer) }.unwrap();
        let closure = (0..u.fns.len() as u32).find(|&id| matches!(u.fns[id as usize].key, FnKey::Closure(_, _, p) if p == outer));
        (closure, closure.map(|c| u.fn_id(FnKey::Plain(c))))
    });
    let (closure, twin) = (closure.expect("outer's closure has a slot"), twin.unwrap());
    let front = crate::owner::call(crate::owner::Class::Program, || u.front.clone());
    assert!(prefront_slot(&front, twin), "the closure's twin's front was not made off the owner");
    // taken for the twin, lowered in twin mode: the closure's call to inner goes to inner's twin
    let func = crate::owner::call(crate::owner::Class::Program, || take_lowered_ahead(&mut u, twin)).expect("the twin's lowering is taken");
    let twins: Vec<u32> = func.blocks.iter().flat_map(|b| b.insts.iter()).filter_map(|i| match i {
        relax_backend::rir::Inst::Call(relax_backend::rir::Callee::Fn(c), _, _) => Some(*c),
        _ => None,
    }).filter(|&c| matches!(u.fns[c as usize].key, FnKey::Plain(_))).collect();
    assert!(!twins.is_empty(), "the closure's twin was lowered as verified code: no call goes to a twin (closure #{closure}, twin #{twin})");
    // (process exit owns the runtime's teardown)
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

const ALLOCATOR_SOURCE: &str = r#"
use std::alloc::{GlobalAlloc, Layout, System};
struct Counting;
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 { unsafe { System.alloc(l) } }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) { unsafe { System.dealloc(p, l) } }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;
fn main() {
    let v = vec![1u32];
    let _ = v.len();
}
"#;

/// The front of core::heap::alloc (and its allocator-shim siblings) is made off the owner in a
/// program with a `#[global_allocator]`: the shim names the allocator static, which no body names,
/// and lowering ahead asked the owner for nothing when it found the static unpublished. A warm
/// launch's disk-cache code initializes that static without publishing it, so the fronts failed
/// ("needs unit") for the rest of the run, and every hot caller's O3 build left the four heap fns
/// out, built again after each drain that made anything (2425 loud lines in one run of a large app).
#[test]
fn the_allocator_shims_static_is_made_for_a_front_off_the_owner() {
    isolated_test!(the_allocator_shims_static_is_made_for_a_front_off_the_owner);
    crate::borrowck::lazy::disable_gandalf();
    if std::env::var_os("RELAX_STD").is_none() {
        std::env::set_var("RELAX_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    }
    let dir = std::env::temp_dir().join(format!("relax-alloc-front-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("input.rs");
    std::fs::write(&path, ALLOCATOR_SOURCE).unwrap();
    let (prog, core, krate) = crate::load_with_std(&std::sync::Arc::new(crate::Session::default()), path.to_str().unwrap(), "alloc_front", false).unwrap();
    crate::layout::set_root_crate(krate);
    let alloc_d = prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = DefId(i as u32);
        (d.krate == core && d.kind == DefKind::Fn && prog.name(id) == "alloc" && prog.def_path(id).ends_with("heap::alloc")).then_some(id)
    }).expect("core::heap::alloc");
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    let mut u = Unit::new(prog, tcx, core);
    let up = &mut *u as *mut Unit;
    unsafe { UNIT = up };
    let (allocator, _) = u.global_allocator().expect("the fixture's program has a #[global_allocator]");
    assert!(u.front.consts.get(&(allocator, Vec::new())).is_none(), "the fixture's allocator static must not be published yet");
    let id = crate::owner::call(crate::owner::Class::Program, || u.fn_id(FnKey::Inst(alloc_d, Vec::new())));
    let front = crate::owner::call(crate::owner::Class::Program, || u.front.clone());
    assert!(prefront_slot(&front, id), "the front of core::heap::alloc was not made off the owner: the allocator static was never asked for");
    assert!(u.front.consts.get(&(allocator, Vec::new())).is_some(), "the owner's drain did not publish the allocator static");
    // (process exit owns the runtime's teardown)
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}
