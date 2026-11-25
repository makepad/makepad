//! makepad inspect tier clarity (lane INSPECT-TIERS): a fixture with a small fn (`leaf`) inlined
//! into its callers at O1 and at O3. Every code body says which version it is; a pc in an inlined
//! copy names the inlined fn and the code it is in; `count` of the inlined fn rebuilds the
//! inliners so every call is counted, later O3 builds keep it, a later install that brings an
//! inlined copy is rebuilt too; `rm` brings the O3 code back; a `tier o1` pin of the fn runs the
//! inliners at O1 and its removal restores their O3 code.

use super::*;
use crate::inspect::codever;
use crate::inspect::patches;

const SOURCE: &str = r#"
#[inline]
pub fn leaf(x: u32) -> u32 {
    let y = x << 1;
    let z = y ^ (x >> 3);
    z ^ 7
}
pub fn caller(out: &mut u32) {
    let mut i = 0u32;
    let mut s = 0u32;
    while i < 100 { s = s.wrapping_add(leaf(i)); i += 1; }
    *out = s;
}
pub fn caller2(out: &mut u32) {
    let mut i = 0u32;
    let mut s = 0u32;
    while i < 10 { s = s.wrapping_add(leaf(i)); i += 1; }
    *out = s;
}
pub fn caller4(out: &mut u32) {
    let mut i = 0u32;
    let mut s = 0u32;
    while i < 3 { s = s.wrapping_add(leaf(i)); i += 1; }
    *out = s;
}
pub fn caller3(out: &mut u32) {
    let mut i = 0u32;
    let mut s = 0u32;
    while i < 5 { s = s.wrapping_add(leaf(i)); i += 1; }
    *out = s;
}
fn main() {}
"#;

fn kind_now(u: &Unit, id: u32) -> CodeKind {
    codever::kind_at(u, u.fns[id as usize].code)
}

unsafe fn run(up: *mut Unit, id: u32) -> u32 {
    let entry = (&*up).fns[id as usize].code;
    let mut r = 0u32;
    call_jit(up, entry, &mut r as *mut u32 as u64).unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
    r
}

/// Does slot `id`'s installed code hold an instruction of `leaf`'s lines (an inlined copy)?
fn holds_leaf(u: &Unit, id: u32, leaf: u32) -> bool {
    let lines = codever::target_lines(u, &[(leaf, None)]);
    codever::map_holds(u, &u.fns[id as usize].pc_map, &lines)
}

#[test]
fn inlined_fn_versions_probes_and_pins() {
    isolated_test!(inlined_fn_versions_probes_and_pins);
    crate::borrowck::lazy::disable_gandalf();
    if std::env::var_os("RELAX_STD").is_none() {
        std::env::set_var("RELAX_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    }
    let dir = std::env::temp_dir().join(format!("relax-inspect-tiers-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("input.rs");
    std::fs::write(&path, SOURCE).unwrap();
    let (prog, core, krate) = crate::load_with_std(&std::sync::Arc::new(crate::Session::default()), path.to_str().unwrap(), "inspect_tiers", false).unwrap();
    let def = |prog: &Program, n: &str| prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = DefId(i as u32);
        (d.krate == krate && prog.name(id) == n).then_some(id)
    }).unwrap();
    let (leaf_d, caller_d, caller2_d, caller3_d, caller4_d) = (def(&prog, "leaf"), def(&prog, "caller"), def(&prog, "caller2"), def(&prog, "caller3"), def(&prog, "caller4"));
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    let mut u = Unit::new(prog, tcx, core);
    let up = &mut *u as *mut Unit;
    // inspect finds the program's unit as the runtime's
    unsafe { UNIT = up };
    assert!(u.tier1.is_some(), "the fixture needs O3");

    // ---- O1: every slot gets fresh O1 code; the callers' O1 code inlines leaf
    let (leaf, caller, caller2, caller3, pre3) = crate::owner::call(crate::owner::Class::Program, || {
        let ids: Vec<u32> = [leaf_d, caller_d, caller2_d].iter().map(|d| u.fn_id(FnKey::Inst(*d, Vec::new()))).collect();
        for &id in &ids {
            let c = unsafe { compile_fn(up, id) }.unwrap();
            u.install(id, &c.0, &c.2, c.1, 0).unwrap();
            let k = kind_now(&u, id);
            assert_eq!((k.tier_name(), k.origin_name()), ("O1", "fresh"), "{}", u.fns[id as usize].name);
        }
        // caller3's O1 code is built now (with leaf inlined) and installed only after `count`:
        // a build that started before the probe
        let c3 = u.fn_id(FnKey::Inst(caller3_d, Vec::new()));
        let pre3 = unsafe { compile_fn(up, c3) }.unwrap();
        (ids[0], ids[1], ids[2], c3, pre3)
    });
    assert_eq!(unsafe { run(up, caller) }, 9984);
    let o1_inlines = crate::owner::call(crate::owner::Class::Program, || holds_leaf(&u, caller, leaf));
    eprintln!("inspect-tiers: caller's O1 code {} leaf", if o1_inlines { "inlines" } else { "calls" });

    // ---- O3: the caller's O3 code inlines leaf; its O1 body is retired, still described as O1
    let o1_start = u.fns[caller as usize].code;
    assert!(unsafe { compile_tier1(up, caller) }, "O3 build of caller");
    let o3_start = crate::owner::call(crate::owner::Class::Program, || {
        let k = kind_now(&u, caller);
        assert_eq!((k.tier_name(), k.origin_name()), ("O3", "fresh"));
        assert_eq!(codever::kind_at(&u, o1_start).tier_name(), "O1", "the retired body keeps its kind");
        let bodies = codever::bodies(&u, caller);
        assert!(bodies.iter().any(|b| b.start == o1_start && b.state == "retired"), "the O1 body is a retired version");
        if !holds_leaf(&u, caller, leaf) {
            let lines = codever::target_lines(&u, &[(leaf, None)]);
            let mut pos = Vec::new();
            u.fns[caller as usize].pc_map.each(|o, p| { pos.push((o, codever::pos_line(&u, p))); true });
            let inl = u.code.read().unwrap().inlined_at(u.fns[caller as usize].code).map(|s| s.to_vec());
            panic!("the fixture needs leaf inlined into caller's O3 code: leaf lines {:?}, caller map {:?}, inlined {:?}, rir {}", lines, pos, inl, u.fns[caller as usize].rir.as_ref().map(|r| r.dump()).unwrap_or_default());
        }
        let code = u.fns[caller as usize].code;
        let inl = u.code.read().unwrap().inlined_at(code).map(|s| s.to_vec()).unwrap_or_default();
        // (leaf came in with caller's O1 RIR: the slot's body_deps name it; the O3 body records
        // what O3 inlined on top)
        assert!(inl.contains(&leaf) || u.body_deps.get(&caller).is_some_and(|d| d.contains(&leaf)), "the code records what it inlined: {:?}", inl);
        // a pc in the inlined copy: named as leaf, inlined into caller's O3 code
        let lines = codever::target_lines(&u, &[(leaf, None)]);
        let mut off = None;
        u.fns[caller as usize].pc_map.each(|o, p| {
            if lines.iter().any(|l| l.holds(&u, p)) {
                off = Some(o);
                return false;
            }
            true
        });
        let pc = code + off.expect("an instruction of leaf in caller's O3 code") as u64;
        let mut r = codever::Resolver::new();
        let d = codever::describe_pc(&u, &mut r, pc).unwrap();
        assert_eq!(d.slot, caller);
        assert_eq!(d.state, "installed");
        let (name, at, module, json) = codever::frame_fields(&u, &d);
        assert!(name.ends_with("leaf"), "inlined frame names the inlined fn: {}", name);
        assert!(at.contains("input.rs:"), "{}", at);
        assert!(module.contains("O3 fresh") && module.contains("inlined at") && module.contains("input.rs:11:") && module.contains("caller"), "{}", module);
        assert!(json.contains("\"tier\":\"O3\"") && json.contains("\"inlined_into\"") && json.contains(&format!("\"inlined_slot\":{}", leaf)) && json.contains("\"inline_stack\":[{\"fn\":\"inspect_tiers::leaf\""), "{}", json);
        // and without the owner: the code tables alone name the version
        let (_, _, m2, j2) = codever::frame_fields_no_owner(&u, pc).unwrap();
        assert!(m2.contains("O3 fresh") && j2.contains(&format!("{:#x}", code)), "{} {}", m2, j2);
        code
    });
    assert_eq!(unsafe { run(up, caller) }, 9984);

    // ---- cache verify: caller's copy of leaf is one an edit of leaf rebuilds; with that edge
    // gone (leaf no longer names caller a host: the bug class it looks for) it is the one
    // untracked copy, named, unguarded (no live session) and among the recorded inputs
    crate::owner::call(crate::owner::Class::Program, || {
        let c = codever::check_copies(&u, &[caller], 8);
        assert!(c.copies >= 1 && c.untracked == 0, "caller's copy of leaf is tracked: {} copies, {} untracked", c.copies, c.untracked);
        let kept = std::mem::take(&mut u.fns[leaf as usize].history.get_or_insert_with(Default::default).inlined_into);
        let c = codever::check_copies(&u, &[caller], 8);
        u.fns[leaf as usize].history.get_or_insert_with(Default::default).inlined_into = kept;
        assert_eq!((c.untracked, c.unguarded), (1, 1), "the copy whose edge is gone: {:?}", c.rows.iter().map(|r| r.0).collect::<Vec<_>>());
        assert!(c.rows.iter().any(|r| r.0 == caller && r.1.contains(&leaf_d) && !r.2 && r.3), "named as leaf's copy in caller, recorded");
    });

    // ---- count leaf: leaf's own code is instrumented; its inliners are rebuilt at their own tier
    // (caller at O3, caller2 at O1) without leaf inlined, so all calls count
    let (group, sites, inliners) = crate::inspect::instrument::count_for_test("inspect_tiers::leaf").unwrap();
    eprintln!("inspect-tiers count rows: {:?} {:?}", sites, inliners);
    let site = sites.iter().find(|s| s.2 == leaf).expect("leaf's site").0;
    crate::owner::call(crate::owner::Class::Program, || {
        let k = kind_now(&u, leaf);
        assert_eq!((k.tier_name(), k.origin_name()), ("instrumented", "inspect"));
        let k = kind_now(&u, caller);
        assert_eq!((k.tier_name(), k.origin_name()), ("O3", "inspect"), "caller rebuilt without inlining, at O3 as before");
        assert!(!holds_leaf(&u, caller, leaf), "caller's new code calls leaf");
        let k = kind_now(&u, caller2);
        assert_eq!((k.tier_name(), k.origin_name()), ("O1", "inspect"), "caller2 rebuilt without inlining, at O1 as before");
        assert!(!holds_leaf(&u, caller2, leaf));
        assert!(patches::no_inline(leaf));
        assert!(inliners.iter().any(|r| r.contains(&format!("\"slot\":{}", caller)) && r.contains("\"rebuilt\":true") && r.contains("\"tier\":\"O3\"")), "the inliner row names the O3 code it replaced: {:?}", inliners);
        assert!(!inliners.iter().any(|r| r.contains("\"note\"") || r.contains("tier_note")), "no tier was lowered: {:?}", inliners);
        assert!(sites.iter().any(|s| s.3.contains("\"replaced\"") && s.3.contains("\"now\"")), "{:?}", sites);
    });
    assert_eq!(unsafe { run(up, caller) }, 9984);
    assert_eq!(crate::inspect::instrument::site_hits_for_test(site), 100, "every call of leaf counted, also from its former inliner");
    assert_eq!(unsafe { run(up, caller2) }, 106);
    assert_eq!(crate::inspect::instrument::site_hits_for_test(site), 110);

    // ---- a later build (O1, then O3) of another caller does not inline the counted fn
    let caller4 = crate::owner::call(crate::owner::Class::Program, || {
        let id = u.fn_id(FnKey::Inst(caller4_d, Vec::new()));
        let c = unsafe { compile_fn(up, id) }.unwrap();
        u.install(id, &c.0, &c.2, c.1, 0).unwrap();
        assert!(!holds_leaf(&u, id, leaf), "new O1 code calls the counted leaf");
        id
    });
    assert!(unsafe { compile_tier1(up, caller4) }, "O3 build of caller4");
    crate::owner::call(crate::owner::Class::Program, || {
        assert_eq!(kind_now(&u, caller4).tier_name(), "O3");
        assert!(!holds_leaf(&u, caller4, leaf), "new O3 code calls the counted leaf");
    });
    assert_eq!(unsafe { run(up, caller4) }, 15);
    assert_eq!(crate::inspect::instrument::site_hits_for_test(site), 113);
    // an O3 build of an instrumented slot is dropped (the probe stays)
    assert!(!unsafe { compile_tier1(up, caller) }, "caller stays as inspect rebuilt it");

    // ---- an install that brings an inlined copy (built before the probe) is rebuilt too
    let had_copy = crate::owner::call(crate::owner::Class::Program, || {
        let map = PcMap::new(&pre3.1);
        let lines = codever::target_lines(&u, &[(leaf, None)]);
        let had = codever::map_holds(&u, &map, &lines);
        u.install(caller3, &pre3.0, &pre3.2, pre3.1.clone(), 0).unwrap();
        had
    });
    // (the rebuild is posted to the owner: one more owner call runs it)
    crate::owner::call(crate::owner::Class::Program, || {});
    crate::owner::call(crate::owner::Class::Program, || {
        if had_copy {
            assert!(patches::active(caller3), "caller3 was rebuilt for the probe");
            assert!(!holds_leaf(&u, caller3, leaf));
        } else {
            eprintln!("inspect-tiers: caller3's O1 code had no inlined copy (nothing to rebuild)");
        }
    });
    assert_eq!(unsafe { run(up, caller3) }, 31);
    if had_copy {
        assert_eq!(crate::inspect::instrument::site_hits_for_test(site), 118);
    }

    // ---- rm: caller's O3 code (the same version) comes back, leaf its O1 code
    crate::inspect::instrument::remove_for_test(group).unwrap();
    crate::owner::call(crate::owner::Class::Program, || {
        assert_eq!(u.fns[caller as usize].code, o3_start, "the O3 version is back");
        assert_eq!(kind_now(&u, caller).tier_name(), "O3");
        assert_eq!(kind_now(&u, leaf).tier_name(), "O1");
        assert!(!patches::no_inline(leaf));
    });
    assert_eq!(unsafe { run(up, caller) }, 9984);

    // ---- tier leaf o1: leaf and its inliner run at O1; no O3 build replaces them; unpinned the O3
    // code comes back
    let pin = patches::Owner::Pin(9001);
    let rows = crate::owner::call(crate::owner::Class::Program, || unsafe {
        let _k = crate::zone_alloc::kept();
        assert!(patches::add(up, leaf, pin, patches::Edit::Pin).unwrap());
        codever::cover_inliners(up, pin, &[(leaf, None)], true)
    });
    eprintln!("inspect-tiers pin rows: {:?}", rows);
    crate::owner::call(crate::owner::Class::Program, || {
        let k = kind_now(&u, caller);
        assert_eq!((k.tier_name(), k.origin_name()), ("O1", "inspect"), "the inliner is pinned at O1");
        assert!(patches::pinned(caller) && patches::pinned(leaf));
        assert!(rows.iter().any(|r| r.contains(&format!("\"slot\":{}", caller)) && r.contains("\"tier\":\"O3\"")), "{:?}", rows);
        let slots = patches::owner_slots(pin);
        assert!(slots.iter().any(|s| s.0 == caller && s.1 == "pinned inliner" && s.2 == o3_start), "{:?}", slots);
        assert!(slots.iter().any(|s| s.0 == leaf && s.1 == "target"), "{:?}", slots);
    });
    assert_eq!(unsafe { run(up, caller) }, 9984);
    assert!(!unsafe { compile_tier1(up, caller) }, "a pinned slot gets no O3 code");
    crate::owner::call(crate::owner::Class::Program, || unsafe {
        let _k = crate::zone_alloc::kept();
        patches::remove_owner(up, pin);
        assert_eq!((&*up).fns[caller as usize].code, o3_start, "unpinned: the O3 version is back");
    });
    assert_eq!(unsafe { run(up, caller) }, 9984);

    // ---- tier caller o3 fresh: a new O3 version, described as such
    assert!(unsafe { compile_tier1(up, caller) });
    crate::owner::call(crate::owner::Class::Program, || {
        let now = u.fns[caller as usize].code;
        assert_ne!(now, o3_start, "a fresh O3 version");
        assert_eq!(kind_now(&u, caller).tier_name(), "O3");
        assert!(codever::bodies(&u, caller).len() >= 3, "installed + earlier versions");
    });
    assert_eq!(unsafe { run(up, caller) }, 9984);

    // ---- count caller (O3 code): the instrumented build is O3 too -- the code under inspection
    // keeps its tier; `tier=o1` is the explicit lower tier
    let (_, sites, _) = crate::inspect::instrument::count_for_test("inspect_tiers::caller").unwrap();
    let site = sites.iter().find(|s| s.2 == caller).expect("caller's site").0;
    crate::owner::call(crate::owner::Class::Program, || {
        let k = kind_now(&u, caller);
        assert_eq!((k.tier_name(), k.origin_name()), ("O3-instrumented", "inspect"), "{:?}", sites);
        assert!(!sites.iter().any(|s| s.3.contains("\"note\"")), "{:?}", sites);
    });
    assert_eq!(unsafe { run(up, caller) }, 9984);
    assert_eq!(crate::inspect::instrument::site_hits_for_test(site), 1);
    unsafe { UNIT = std::ptr::null_mut() };
    std::fs::remove_dir_all(&dir).ok();
}

/// `if !plain(a) { two stores; an empty patchable call }` inside an inlined callee, after a call
/// (a wait point, so the callee's copies keep their guards): the callee's O3 body holds the empty
/// call's guarded copy, its jumps threaded, so the `if`'s body ends in the copy's guard and the
/// guard's fast side is the `if`'s join (an emulator's lane bus read)
const SPLIT_SOURCE: &str = r#"
pub struct Bus { pub tick: u32, pub seen: u32 }
pub struct Lane<'a> { pub bus: &'a mut Bus, pub tick: u32, pub slow: u32 }
#[inline]
pub fn note(_a: u32) {}
#[inline]
pub fn plain(a: u32) -> bool { a >= 64 }
#[inline(never)]
pub fn probe(a: u32) -> u32 { a }
#[inline]
pub fn lane_read(l: &mut Lane, a: u32) -> u32 {
    let a = probe(a);
    if !plain(a) {
        l.slow = a | 1;
        l.bus.tick = l.tick;
        note(a);
    }
    l.bus.seen = l.bus.seen.wrapping_mul(31).wrapping_add(l.bus.tick);
    a
}
pub fn caller(out: &mut u32) {
    let mut bus = Bus { tick: 0, seen: 0 };
    let mut i = 0u32;
    let mut s = 0u32;
    while i < 100 {
        let mut l = Lane { bus: &mut bus, tick: i, slow: 0 };
        s = s.wrapping_add(lane_read(&mut l, i));
        s = s.wrapping_add(l.slow);
        i += 1;
    }
    *out = s.wrapping_add(bus.seen);
}
"#;

#[test]
fn a_split_keeps_a_branch_whose_body_ends_in_an_empty_guarded_copy() {
    isolated_test!(a_split_keeps_a_branch_whose_body_ends_in_an_empty_guarded_copy);
    // A live session's O3 build of `caller` splits (relax_codegen::patched). The fast half must
    // keep `lane_read`'s `if` and both stores of its body: the split read the `if` as a guard test
    // (its body's block ends in `note`'s guard, with the same fast side) and the fast half went
    // straight to the join, so `slow` stayed 0 and the bus kept its old stamp (rustc: 1980309814;
    // with the stores lost: 4950, which this returned before the fix). An emulator's register
    // accesses lost their stamps (11-19).
    crate::borrowck::lazy::disable_gandalf();
    if std::env::var_os("RELAX_STD").is_none() {
        std::env::set_var("RELAX_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    }
    let dir = std::env::temp_dir().join(format!("relax-split-branch-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("input.rs");
    std::fs::write(&path, SPLIT_SOURCE).unwrap();
    let mut session = crate::Session::default();
    session.options.live = true;
    let (prog, core, krate) = crate::load_with_std(&std::sync::Arc::new(session), path.to_str().unwrap(), "split_branch", false).unwrap();
    let def = |prog: &Program, n: &str| prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = DefId(i as u32);
        (d.krate == krate && prog.name(id) == n).then_some(id)
    }).unwrap();
    let defs = ["note", "plain", "probe", "lane_read", "caller"].map(|n| def(&prog, n));
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    let mut u = Unit::new(prog, tcx, core);
    let up = &mut *u as *mut Unit;
    unsafe { UNIT = up };
    assert!(u.tier1.is_some(), "the fixture needs O3");
    let ids = crate::owner::call(crate::owner::Class::Program, || {
        let ids = defs.map(|d| u.fn_id(FnKey::Inst(d, Vec::new())));
        for &id in &ids {
            let c = unsafe { compile_fn(up, id) }.unwrap();
            u.install(id, &c.0, &c.2, c.1, 0).unwrap();
        }
        ids
    });
    let caller = ids[4];
    assert_eq!(unsafe { run(up, caller) }, 1980309814, "the O1 build");
    // `lane_read` hot first: its O3 body is the one `caller`'s build takes in, guards threaded
    assert!(unsafe { compile_tier1(up, ids[3]) }, "O3 build of lane_read");
    assert!(unsafe { compile_tier1(up, caller) }, "O3 build of caller");
    crate::owner::call(crate::owner::Class::Program, || {
        let f = &u.fns[caller as usize];
        let mut pat = None;
        f.pc_map.each(|o, p| {
            if o < f.code_len && relax_backend::rir::pos_patched(p) {
                pat = Some(f.code + u64::from(o));
                return false;
            }
            true
        });
        let pc = pat.unwrap_or_else(|| panic!("caller's O3 code has a patched path: {}", f.rir.as_ref().map(|r| r.dump()).unwrap_or_default()));
        assert!(codever::describe_pc(&u, &mut codever::Resolver::new(), pc).unwrap().patched);
        assert!(codever::in_patched_path(&u, caller, f.code, pc));
    });
    assert_eq!(unsafe { run(up, caller) }, 1980309814, "the split O3 build's fast half keeps the branch and both stores");
    unsafe { UNIT = std::ptr::null_mut() };
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_frame_in_a_patched_path_is_named() {
    isolated_test!(a_frame_in_a_patched_path_is_named);
    // Both machine functions belong to one install. An old frame keeps that install's
    // source and inline context when the slot moves to another patch generation.
    crate::borrowck::lazy::disable_gandalf();
    if std::env::var_os("RELAX_STD").is_none() {
        std::env::set_var("RELAX_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    }
    let dir = std::env::temp_dir().join(format!("relax-patched-path-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("input.rs");
    std::fs::write(&path, SOURCE).unwrap();
    let mut session = crate::Session::default();
    session.options.live = true;
    session.options.opt = vec!["+outline".into()];
    std::env::remove_var("RELAX_OUTLINE_WHOLE");
    let (prog, core, krate) = crate::load_with_std(&std::sync::Arc::new(session), path.to_str().unwrap(), "patched_path", false).unwrap();
    let def = |prog: &Program, n: &str| prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = DefId(i as u32);
        (d.krate == krate && prog.name(id) == n).then_some(id)
    }).unwrap();
    let (leaf_d, caller_d) = (def(&prog, "leaf"), def(&prog, "caller"));
    let file = prog.def(leaf_d).file;
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    let mut u = Unit::new(prog, tcx, core);
    let up = &mut *u as *mut Unit;
    unsafe { UNIT = up };
    assert!(u.tier1.is_some(), "the fixture needs O3");
    let (leaf, caller) = crate::owner::call(crate::owner::Class::Program, || {
        let ids = [leaf_d, caller_d].map(|d| u.fn_id(FnKey::Inst(d, Vec::new())));
        for &id in &ids {
            let c = unsafe { compile_fn(up, id) }.unwrap();
            u.install(id, &c.0, &c.2, c.1, 0).unwrap();
        }
        (ids[0], ids[1])
    });
    // A slot compiled from an edited file tiers into that generation's code group.
    crate::owner::call(crate::owner::Class::Program, || {
        u.file_gen.insert(file, 1);
        assert!(crate::invalid::group_of(&u, caller, 0, &[]).is_some());
    });
    assert!(unsafe { compile_tier1(up, caller) }, "O3 build of caller");
    let ver = |u: &Unit| unsafe { (*tier_rec(&u.slots, caller).add((TIER_COPY / 8) as usize)).load(Ordering::Acquire) };
    let code = u.fns[caller as usize].code;
    let part = joined_install(&u, caller, 2, origin::FRESH);
    let (pc, location) = crate::owner::call(crate::owner::Class::Program, || {
        let mut pc = None;
        let lines = codever::target_lines(&u, &[(leaf, None)]);
        u.fns[caller as usize].pc_map.each(|o, p| {
            let at = code + u64::from(o);
            if (part..code + u64::from(u.fns[caller as usize].code_len)).contains(&at)
                && relax_backend::rir::pos_ctx(p) != 0 && lines.iter().any(|l| l.holds(&u, p)) {
                pc = Some((at, u.pos_str(relax_backend::rir::pos_file(p), p as u32)));
                return false;
            }
            true
        });
        pc.expect("an inlined instruction of leaf in P")
    });
    let check = |u: &Unit, state: &str| {
        let mut r = codever::Resolver::new();
        let d = codever::describe_pc(u, &mut r, pc).unwrap();
        assert_eq!((d.slot, d.start, d.state), (caller, code, state));
        let (name, at, module, json) = codever::frame_fields(u, &d);
        assert_eq!(name, "patched_path::leaf");
        assert_eq!(at, location, "the saved instruction's file, line and column");
        assert!(module.contains("patched path of patched_path::caller") && module.contains("inlined at"), "{module}");
        assert!(json.contains("\"patched_path\"") && json.contains("\"inlined_into\":\"patched_path::caller\"")
            && json.contains("\"inline_stack\":[{\"fn\":\"patched_path::leaf\""), "{json}");
        assert!(d.patched && codever::in_patched_path(u, caller, code, pc));
        assert!(codever::describe_pc(u, &mut r, part).unwrap().patched, "P's entry before its first source line");
        assert!(codever::in_patched_path(u, caller, code, part));
        let fast = codever::describe_pc(u, &mut r, code).unwrap();
        assert!(!fast.patched && !codever::frame_fields(u, &fast).2.contains("patched path"));
        assert!(!codever::in_patched_path(u, caller, code, code + u64::from(d.len)), "past this install");
    };
    crate::owner::call(crate::owner::Class::Program, || {
        check(&u, "installed");
        assert_eq!(ver(&u), 0);
    });
    let run_saved = || {
        let mut result = 0u32;
        unsafe { call_jit(up, code, &mut result as *mut u32 as u64) }.map_err(|p| p.message).unwrap();
        result
    };
    assert_eq!(run_saved(), 9984);
    let change = unsafe { crate::reload::apply(up, vec![crate::reload::Edit {
        file, path: path.to_string_lossy().into_owned(), src: SOURCE.replace("z ^ 7", "(z ^ 7) + 1").replacen("*out = s;", "*out = s + 2;", 1).into_bytes(),
    }]) };
    assert_eq!(change.rung, crate::reload::Rung::Patch, "{:?}", change.lines);
    assert_ne!(ver(&u), 0, "the changed leaf moves the copy version the saved host reads");
    assert_eq!(run_saved(), 10084, "the original entry runs the changed leaf through P");
    assert_eq!(unsafe { run(up, caller) }, 10086, "the replacement also adds two");
    crate::owner::call(crate::owner::Class::Program, || {
        check(&u, "replaced");
        u.restore_before(caller, change.group);
        check(&u, "installed");
    });
    unsafe { UNIT = std::ptr::null_mut() };
    std::fs::remove_dir_all(&dir).ok();
}

fn joined_install(u: &Unit, slot: u32, version: u32, origin: u8) -> u64 {
    let f = &u.fns[slot as usize];
    let start = f.code;
    let t = u.code.read().unwrap();
    assert_eq!(t.version_of(start).map(|(v, n, _)| (v, n)), Some((version, version)), "one version per install");
    assert_eq!(t.kind_of(start), CodeKind::new(tier::O3, origin), "O3 install origin");
    let part = t.function_base(start, start + u64::from(f.code_len) - 1);
    assert!(part > start, "P has its own function base");
    assert_eq!(t.function_base(start, part - 1), start, "last byte of the host");
    assert_eq!(t.function_base(start, part), part, "first byte of P");
    assert_eq!(t.entry_at(part), Some((start, f.code_len, slot)), "P belongs to the host slot");
    assert_eq!(unwind_starts(&t, start, start + u64::from(f.code_len)), [start, part], "both unwind trailers");
    part
}

#[test]
fn joined_body_unwind_uses_each_functions_frame() {
    use relax_backend::rir::{part_trailer_words, split_unwind_info, unwind_info_words};
    const LEN: u32 = 48;
    const P_OFF: u32 = 24;
    // The image keeps these bytes mapped for the lifetime of the code tables.
    static BODY: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    let body = BODY.get_or_init(|| {
        let mut words = vec![0; LEN as usize / 4];
        words.extend(unwind_info_words(&[3], &[], &[(P_OFF, 16)], 48));
        let p = unwind_info_words(&[12, 0x108], &[], &[(8, 32)], 80);
        let footer = part_trailer_words(P_OFF, p.len());
        words.extend(p);
        words.extend(footer);
        words.into_iter().flat_map(u32::to_le_bytes).collect()
    });

    let mut tables = CodeTables::default();
    let split = split_unwind_info(body);
    tables.add(0x2000, split.code_len as u32, 7, split.host, split.part, CodeKind::default(), false);
    assert_eq!(tables.index.len(), 1, "one code range for both functions");

    // Image order can differ from address order. The second install uses its mapped trailers.
    tables.add_image_trailers(0x1000, LEN, &body[LEN as usize..], &PcMap::default());
    tables.add(0x1000, LEN, 8, None, None, CodeKind::default(), false);
    assert_eq!(tables.index.len(), 2, "one more code range for the mapped body");
    for (start, slot) in [(0x2000, 7), (0x1000, 8)] {
        let p = start + P_OFF as u64;
        assert_eq!(tables.entry_at(p + 7), Some((start, LEN, slot)), "P belongs to its owner's slot");
        assert_eq!(tables.version_of(start).map(|(v, n, _)| (v, n)), Some((1, 1)), "one version per install");
        assert_eq!(tables.function_base(start, p - 1), start, "last byte of host");
        assert_eq!(tables.function_base(start, p), p, "first byte of P");
        assert_eq!(tables.frame_pointer_at(start + 8, 1000), Ok(1048), "host frame");
        assert_eq!(tables.frame_pointer_at(p, 1000), Ok(1064), "call ending at P belongs to host");
        assert_eq!(tables.frame_pointer_at(p + 8, 1000), Ok(1112), "P's staged bytes at its relative return offset");
        assert_eq!(tables.frame_pointer_at(start + LEN as u64, 1000), Ok(1080), "last call in P");
        assert_eq!(tables.saved_regs_at(p - 1), Some((start, vec![3])), "host register saves");
        assert_eq!(tables.saved_regs_at(p), Some((p, vec![12, 0x108])), "P's register saves and base");
    }
}

fn unwind_starts(tables: &CodeTables, start: u64, end: u64) -> Vec<u64> {
    let mut frames: Vec<_> = tables.unwind.keys().copied().chain(tables.image_unwind.iter().map(|t| t.0))
        .filter(|a| (start..end).contains(a)).collect();
    frames.sort_unstable();
    frames
}

fn joined_body_phases(test: &str, program: &str, whole: &str, phases: &[&str]) -> Option<(String, std::path::PathBuf)> {
    const PHASE: &str = "RELAX_JOINED_BODY_PHASE";
    const DIR: &str = "RELAX_JOINED_BODY_DIR";
    if let Ok(phase) = std::env::var(PHASE) {
        let path = std::path::PathBuf::from(std::env::var_os(DIR).unwrap()).join("lib.rs");
        return Some((phase, path));
    }
    let dir = std::env::temp_dir().join(format!("relax-{test}-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("lib.rs"), program).unwrap();
    for &phase in phases {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child.args(["--exact", &format!("jit::inspect_tiers_tests::{test}"), "--nocapture", "--test-threads=1"])
            .env(PHASE, phase).env(DIR, &dir).env("RELAX_CACHE_DIR", dir.join("cache"))
            .env("RELAX_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"))
            .env("RELAX_OUTLINE_WHOLE", whole).env_remove("RELAX_INSPECT")
            .env("RELAX_FLIGHTREC", "0").env("RELAX_PANIC_PARK", "600").env("RELAX_EXPECTED_CRASH", "the joined body test inspects and releases its park");
        let start = std::time::Instant::now();
        let out = crate::test_child::output(&mut child);
        let stdout = String::from_utf8_lossy(&out.stdout);
        eprintln!("joined bodies {phase}: {:.2?}", start.elapsed());
        assert!(out.status.success() && stdout.contains("1 passed"), "{phase} ({}): {stdout}\n{}", out.status, String::from_utf8_lossy(&out.stderr));
    }
    std::fs::remove_dir_all(dir).unwrap();
    None
}

#[test]
fn joined_bodies_unwind_once_and_show_the_parts_locals() {
    const PROGRAM: &str = r#"
use std::sync::atomic::{AtomicU32, Ordering::SeqCst};
static DROPS: AtomicU32 = AtomicU32::new(0);
struct D<F: Fn()>(u32, F);
impl<F: Fn()> Drop for D<F> { fn drop(&mut self) { (self.1)(); DROPS.fetch_add(self.0, SeqCst); } }
#[inline(never)]
pub fn outlined_leaf(n: u32) -> u32 {
    let seen = n * 3 + 1;
    // Both locals are still needed when the panic unwinds this frame.
    let _d = D(10, || { std::hint::black_box((&n, &seen)); });
    if n > 2 { panic!("outlined_leaf panics") }
    seen
}
#[inline(never)]
pub fn outlined_host(n: u32) -> u32 { let _d = D(1, || {}); outlined_leaf(n) + 1 }
pub fn normal(out: &mut [u32; 2]) {
    out[0] = outlined_host(1);
    out[1] = DROPS.load(SeqCst);
}
pub fn panicking(out: &mut [u32; 2]) {
    out[0] = std::thread::spawn(|| outlined_host(7)).join().is_err() as u32;
    out[1] = DROPS.load(SeqCst);
}
"#;
    let Some((phase, path)) = joined_body_phases("joined_bodies_unwind_once_and_show_the_parts_locals",
        PROGRAM, "outlined_leaf,outlined_host", &["cold", "warm", "image"]) else { return };
    crate::borrowck::lazy::disable_gandalf();
    let (prog, core, krate) = crate::load_with_std(&std::sync::Arc::new(crate::Session::default()), path.to_str().unwrap(), "joined_body", false).unwrap();
    let def = |name| (0..prog.defs.len() as u32).map(DefId).find(|&d| prog.def(d).krate == krate && prog.name(d) == name).unwrap();
    let [leaf, host, normal, panicking] = ["outlined_leaf", "outlined_host", "normal", "panicking"].map(def);
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    let mut u = Unit::new(prog, tcx, core);
    let up = &mut *u as *mut Unit;
    let (ids, plain, [normal, panicking]) = crate::owner::call(crate::owner::Class::Program, || {
        let mut cache = crate::cache::Cache::open(&u, crate::cache::Mode::ReadOnly).unwrap();
        cache.mode = crate::cache::Mode::On;
        if phase == "image" {
            cache.load_manifest(krate);
            assert!(cache.has_startup_image(), "the cold run wrote a startup image");
        }
        u.cache = Some(Box::new(crate::cache::CacheCell::new(cache)));
        let ids = [leaf, host].map(|d| u.fn_id(FnKey::Inst(d, vec![])));
        let plain = u.fn_id(FnKey::Inst(normal, vec![]));
        (ids, plain, [normal, panicking].map(|d| u.entry(u.fn_id(FnKey::Inst(d, vec![])))))
    });
    if phase == "image" {
        assert!(unsafe { prelink_startup(up) } >= ids.len(), "startup image placed before the first call");
    }
    let mut out = [0u32; 2];
    unsafe { call_jit(up, normal, out.as_mut_ptr() as u64) }.map_err(|p| p.message).unwrap();
    assert_eq!(out, [5, 11], "{phase}: return and normal drops");
    // Releasing a watched panic can instrument its leaf for the next restart. Inspect the
    // installs that will run, and identify that same leaf by its body start at the park.
    let starts = crate::owner::call(crate::owner::Class::Program, || ids.map(|id| u.fns[id as usize].code));
    let origin = match phase.as_str() { "warm" => "warm-cache", "image" => "warm-image", _ => "fresh" };
    for ((id, start), name) in ids.into_iter().zip(starts).zip(["outlined_leaf", "outlined_host"]) {
        assert_eq!(codever::kind_at(&u, start).origin_name(), origin, "{phase}: {}", name);
        let tables = u.code.read().unwrap();
        let (_, len, slot) = tables.entry_at(start).expect("installed code");
        assert_eq!(slot, id);
        assert_eq!(tables.version_of(start).map(|(v, n, _)| (v, n)), Some((1, 1)), "{phase}: {}", name);
        let end = start + u64::from(len);
        let base = tables.function_base(start, end - 1);
        assert_ne!(base, start, "{phase}: {} part in code tables", name);
        assert_eq!(unwind_starts(&tables, start, end), [start, base], "{} has both unwind trailers", name);
        assert_eq!(tables.function_base(start, base - 1), start);
        assert_eq!(tables.function_base(start, base), base);
        assert_eq!(tables.entry_at(base), Some((start, len, id)));
    }
    // A function the option does not name takes the path an unset option takes: unchanged
    if phase == "cold" {
        let start = crate::owner::call(crate::owner::Class::Program, || u.fns[plain as usize].code);
        let tables = u.code.read().unwrap();
        let (_, len, _) = tables.entry_at(start).expect("installed code");
        let end = start + u64::from(len);
        assert_eq!(tables.function_base(start, end - 1), start, "an unnamed function has no part");
        assert_eq!(unwind_starts(&tables, start, end), [start], "an unnamed function has only its own unwind trailer");
    }
    // Snapshot those same installs for disk and image before the inspector can replace one.
    if phase == "cold" {
        crate::owner::call(crate::owner::Class::Program, || {
            u.flush_cache_records();
            let mut cache = u.cache.take().unwrap();
            assert!(cache.record_manifest(&u, krate, &ids, ids.len(), &|| false), "record the two functions in the startup image");
            u.cache = Some(cache);
        });
    }
    crate::inspect::start("joined_body");
    let (ran, seen) = watch_part(up, starts[0], "outlined_leaf", ["seen", "n"], || {}, || {
        unsafe { call_jit(up, panicking, out.as_mut_ptr() as u64) }
    });
    assert_eq!(seen.len(), 1, "{phase}: one park: {seen:?}");
    assert!(seen[0].1.contains("outlined_leaf panics"), "{phase}: {seen:?}");
    assert_eq!(seen[0].2, Ok([22, 7]), "{phase}: seen and n in the leaf's part");
    ran.map_err(|p| p.message).unwrap();
    assert_eq!(out, [1, 22], "{phase}: join returned the panic and unwinding added exactly 11 drops");
    if phase == "cold" {
        crate::owner::call(crate::owner::Class::Program, || u.cache.as_mut().unwrap().flush().unwrap());
    }
    std::mem::forget(u);
}

fn watch_part<T>(up: *mut Unit, start: u64, host: &str, locals: [&str; 2], mut on_wait: impl FnMut() + Send, run: impl FnOnce() -> T)
    -> (T, Vec<(u64, String, Result<[u32; 2], String>)>) {
    fn parked_locals(u: &Unit, start: u64, regs: &[u64], host: &str, locals: [&str; 2]) -> Result<[u32; 2], String> {
        use crate::inspect::vars;
        let stop = vars::Stop::from_block(u, regs).ok_or("short parked register block")?;
        let frame = {
            let tables = u.code.read().unwrap();
            stop.frames.iter().position(|&(pc, _)| tables.entry_at(pc).is_some_and(|(owner, _, _)| owner == start && tables.function_base(owner, pc) != owner))
        }.ok_or_else(|| format!("parked stack has no frame in {host}'s part"))?;
        let levels = vars::levels(u, &stop, Some(frame));
        let level = levels.iter().flat_map(|(_, levels)| levels).find(|l| l.inlined_into.is_none() && l.name.ends_with(host))
            .ok_or_else(|| format!("no root level for {host}'s part"))?;
        let read = |name| {
            let local = level.locals.iter().find(|l| l.name == name).ok_or_else(|| format!("missing {name}"))?;
            match &local.got {
                vars::Got::Bytes(b, _) => b.as_slice().try_into().map(u32::from_le_bytes).map_err(|_| format!("{name}: expected u32")),
                vars::Got::No(why) => Err(format!("{name}: {why}")),
            }
        };
        Ok([read(locals[0])?, read(locals[1])?])
    }
    let addr = up as usize;
    let (done, finished) = std::sync::mpsc::channel::<()>();
    std::thread::scope(|scope| {
        let watcher = scope.spawn(move || {
            use crate::inspect::events;
            let mut seen = Vec::new();
            loop {
                on_wait();
                for (id, _, _, message, _) in events::parked_now() {
                    if seen.iter().any(|(k, _, _)| *k == id) { continue; }
                    let values = events::parked_regs(Some(id)).ok_or("missing parked registers".to_string()).and_then(|(_, regs)| {
                        crate::owner::call(crate::owner::Class::Program, || parked_locals(unsafe { &*(addr as *const Unit) }, start, &regs, host, locals))
                    });
                    seen.push((id, message, values));
                    events::quit_releases();
                }
                if finished.recv_timeout(std::time::Duration::from_millis(2)) != Err(std::sync::mpsc::RecvTimeoutError::Timeout) { break; }
            }
            seen
        });
        let ran = run();
        drop(done);
        (ran, watcher.join().unwrap())
    })
}

#[test]
fn a_live_exit_unwinds_and_shows_the_hosts_locals_in_the_part() {
    const PROGRAM: &str = r#"
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering::SeqCst};
static ADDRESS: AtomicUsize = AtomicUsize::new(0);
struct D<F: Fn()>(F);
impl<F: Fn()> Drop for D<F> { fn drop(&mut self) { (self.0)(); } }
#[inline]
fn leaf(n: u32) -> u32 { n + 1 }
#[inline(never)]
fn escape(shared: *mut u32, gate: &AtomicU32) {
    ADDRESS.store(shared as usize, SeqCst);
    if gate.load(SeqCst) == 4 { return; }
    gate.store(1, SeqCst);
    while gate.load(SeqCst) == 1 { std::thread::yield_now(); }
}
#[inline(never)]
pub fn host(state: &[AtomicU32; 4]) {
    let mut shared = 7u32;
    let _before = D(|| { state[1].fetch_add(1, SeqCst); });
    escape(&raw mut shared, &state[0]);
    let seen = leaf(shared);
    shared = seen + 1;
    let _after = D(|| {
        state[3].store(shared * 100 + seen, SeqCst);
        state[2].fetch_add(1, SeqCst);
    });
    let alias = ADDRESS.load(SeqCst) as *const u32;
    if unsafe { *alias } == 11 && shared == 11 && state[0].load(SeqCst) != 4 { panic!("patched host panics") }
}
pub fn drive(addr: usize) {
    let result = std::thread::spawn(move || host(unsafe { &*(addr as *const [AtomicU32; 4]) })).join();
    let state = unsafe { &*(addr as *const [AtomicU32; 4]) };
    if result.is_err() { state[0].store(3, SeqCst); }
}
"#;
    let Some((phase, path)) = joined_body_phases("a_live_exit_unwinds_and_shows_the_hosts_locals_in_the_part",
        PROGRAM, "", &["cold", "warm", "image", "corrupt"]) else { return };
    crate::borrowck::lazy::disable_gandalf();
    let mut session = crate::Session::default();
    session.options.live = true;
    session.options.opt = vec!["+outline".into()];
    let (prog, core, krate) = crate::load_with_std(&std::sync::Arc::new(session), path.to_str().unwrap(), "live_part", false).unwrap();
    let [leaf, host, driver] = ["leaf", "host", "drive"].map(|name|
        (0..prog.defs.len() as u32).map(DefId).find(|&d| prog.def(d).krate == krate && prog.name(d) == name).unwrap());
    let file = prog.def(leaf).file;
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    let mut u = Unit::new(prog, tcx, core);
    let up = &mut *u as *mut Unit;
    unsafe { UNIT = up };
    let (leaf, host, drive) = crate::owner::call(crate::owner::Class::Program, || {
        let mut cache = crate::cache::Cache::open(&u, crate::cache::Mode::ReadOnly).unwrap();
        if phase == "cold" { cache.mode = crate::cache::Mode::On; }
        if matches!(phase.as_str(), "image" | "corrupt") {
            cache.load_manifest(krate);
            assert!(cache.has_startup_image(), "the zero-version host has a startup image");
        }
        u.cache = Some(Box::new(crate::cache::CacheCell::new(cache)));
        let [leaf, host, driver] = [leaf, host, driver].map(|d| u.fn_id(FnKey::Inst(d, vec![])));
        if phase == "cold" {
            let (body, map, relocs) = unsafe { compile_fn(up, leaf) }.unwrap();
            u.install(leaf, &body, &relocs, map, 0).unwrap();
            unsafe { compile_o3_always(up, host) }.expect("the producer builds the host's first O3 install");
        }
        (leaf, host, u.entry(driver))
    });
    if phase == "image" {
        assert!(unsafe { prelink_startup(up) } > 0, "the image placed code before the first call");
    } else if phase == "corrupt" {
        unsafe { prelink_startup(up) };
        assert!(!u.fns[host as usize].compiled, "prelink left the malformed host uninstalled");
        assert!(u.cache.as_ref().unwrap().stats.prelink_failed > 0, "prelink rejected the malformed image slot");
    }
    // The same call links the disk entry, or compiles the host on a miss.
    let normal = [const { std::sync::atomic::AtomicU32::new(0) }; 4];
    normal[0].store(4, Ordering::Release);
    let call = |at| unsafe { call_jit(up, at, normal.as_ptr() as u64) }.map_err(|p| p.message).unwrap();
    call(u.entry(host));
    assert_eq!(normal[3].load(Ordering::Acquire), 908, "{phase}: the unpatched call returns shared=9 and seen=8");
    if phase == "corrupt" {
        let t = u.code.read().unwrap();
        let bodies = t.bodies_of(host);
        assert_eq!(bodies.len(), 1, "the corrupt entry never installed");
        assert_eq!(bodies[0].2.origin, origin::FRESH, "a malformed part compiles fresh on demand");
        assert!(u.cache.as_ref().unwrap().stats.link_fail > 0, "the recorded entry failed to link");
        unsafe { UNIT = std::ptr::null_mut() };
        return;
    }
    let start = u.fns[host as usize].code;
    crate::owner::call(crate::owner::Class::Program, || {
        let origin = match phase.as_str() { "cold" => origin::FRESH, "warm" => origin::CACHE, "image" => origin::IMAGE, _ => unreachable!() };
        joined_install(&u, host, 1, origin);
        assert_eq!(copy_ver(&u, host).unwrap().1, 0, "{phase}: exits start at the recorded copy version");
        assert!(u.fns[leaf as usize].history().copy_hosts.contains(&host), "{phase}: P's leaf flag guards the host");
        if phase == "cold" {
            u.flush_cache_records();
            let mut cache = u.cache.take().unwrap();
            assert!(cache.record_manifest(&u, krate, &[host], 1, &|| false), "snapshot the real O3 install before the edit");
            cache.flush().unwrap();
            cache.mode = crate::cache::Mode::ReadOnly;
            u.cache = Some(cache);
        } else if phase == "image" {
            // The final process tries prelink and demand-link on this same host.
            let mut cache = u.cache.take().unwrap();
            cache.corrupt_recorded_part(&u, host);
            u.cache = Some(cache);
        }
    });
    let state = [const { std::sync::atomic::AtomicU32::new(0) }; 4];
    let mut change = None;
    let addr = up as usize;
    crate::inspect::start("live_part");
    let (ran, seen) = watch_part(up, start, "live_part::host", ["shared", "seen"], || {
        if state[0].load(Ordering::Acquire) == 1 {
            let outcome = unsafe { crate::reload::apply(addr as *mut Unit, vec![crate::reload::Edit {
                file, path: path.to_string_lossy().into_owned(), src: PROGRAM.replace("{ n + 1 }", "{ n + 3 }").into_bytes(),
            }]) };
            change = Some((outcome.rung, outcome.lines));
            state[0].store(2, Ordering::Release);
        }
    }, || unsafe { call_jit(up, drive, state.as_ptr() as u64) });
    let (rung, lines) = change.expect("the fast half escaped its slot before the edit");
    assert_eq!(rung, crate::reload::Rung::Patch, "{lines:?}");
    assert_eq!(seen.len(), 1, "one panic park: {seen:?}");
    assert!(seen[0].1.contains("patched host panics"), "{seen:?}");
    assert_eq!(seen[0].2, Ok([11, 10]), "the shared slot and the patched leaf's result at the park");
    ran.map_err(|p| p.message).unwrap();
    assert_eq!(state[0].load(Ordering::Acquire), 3, "the JIT thread unwound to its join");
    assert_eq!(state[1].load(Ordering::Acquire), 1, "the guard created before the exit dropped once");
    assert_eq!(state[2].load(Ordering::Acquire), 1, "the guard created in P dropped once");
    assert_eq!(state[3].load(Ordering::Acquire), 1110, "the cleanup read both locals from their live homes");
    assert!(u.fns[leaf as usize].patched.load(Ordering::Acquire), "{phase}: the edit set P's leaf flag");
    assert_ne!(copy_ver(&u, host).unwrap().1, 0, "{phase}: patching the leaf moved the host's copy version");
    call(start);
    assert_eq!(normal[3].load(Ordering::Acquire), 1110, "{phase}: the saved exit returns the slow leaf's values");
    unsafe { UNIT = std::ptr::null_mut() };
}
