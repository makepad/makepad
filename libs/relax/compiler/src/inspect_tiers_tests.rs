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
    if std::env::var_os("MAKEPAD_RUN_STD").is_none() {
        std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
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
