//! A hot fn never ends a run below O3 with nothing pending. The one way a body can name what only
//! the owner may make: its promoted constants (`&CONST-EXPR`) and a new vtable (each method a slot
//! nobody made yet), which a background build's front off the owner got no value for: its O3 build
//! gave up for good (STUCK below O3).

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
fn main() {}
"#;

#[test]
fn hot_fn_with_promoted_constants_reaches_o3() {
    isolated_test!(hot_fn_with_promoted_constants_reaches_o3);
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
    // (process exit owns the runtime's teardown)
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}
