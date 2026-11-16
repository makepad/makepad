//! A lean code window (`Unit::code_lean`): a loop that installs a slot's code again and again
//! (code is never freed) stops once less of the window is left than the slots' first code took,
//! before it fills the window. A slot without code still gets its first code, the program runs on
//! with the code it has, and a live edit is answered with a restart.

use super::*;

const SOURCE: &str = "pub fn one(out: &mut u32) { *out = 1; }\npub fn two(out: &mut u32) { *out = 2; }\n";

#[test]
fn a_reinstall_loop_stops_at_the_lean_point_and_first_code_still_installs() {
    isolated_test!(a_reinstall_loop_stops_at_the_lean_point_and_first_code_still_installs);
    crate::borrowck::lazy::disable_gandalf();
    if std::env::var_os("RELAX_STD").is_none() {
        std::env::set_var("RELAX_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    }
    let dir = std::env::temp_dir().join(format!("relax-code-window-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("lib.rs").to_string_lossy().into_owned();
    std::fs::write(&path, SOURCE).unwrap();
    let (prog, core, krate) = crate::load_with_std(&std::sync::Arc::new(crate::Session::default()), &path, "code_window", false).unwrap();
    let def = |prog: &Program, n: &str| prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = DefId(i as u32);
        (d.krate == krate && prog.name(id) == n).then_some(id)
    }).unwrap();
    let (one_d, two_d) = (def(&prog, "one"), def(&prog, "two"));
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    let mut u = Unit::new(prog, tcx, core);
    let up = &mut *u as *mut Unit;
    let run = |entry: u64| -> u32 {
        let mut r = 0u32;
        unsafe { call_jit(up, entry, &mut r as *mut u32 as u64) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        r
    };
    let (one, two, one_entry, two_entry) = crate::owner::call(crate::owner::Class::Program, || {
        let (one, two) = (u.fn_id(FnKey::Inst(one_d, Vec::new())), u.fn_id(FnKey::Inst(two_d, Vec::new())));
        (one, two, u.entry(one), u.entry(two))
    });
    assert_eq!(run(one_entry), 1);
    crate::owner::call(crate::owner::Class::Program, || {
        // `one` built again and again, into a window with room for a few of its versions past
        // what the slots' first code took
        let (code, map, relocs) = unsafe { compile_fn(up, one) }.unwrap();
        u.shrink_code_window(u.first_room() + 8 * code.len() as u64);
        let mut installs = 0;
        while !u.lean_now() && installs < 1000 {
            u.install(one, &code, &relocs, map.clone(), 0).unwrap();
            installs += 1;
        }
        assert!(u.lean_now(), "{} installs and the window is not lean: {}", installs, u.window_line());
        let left = u.code_end - u.code_next.load(Ordering::Relaxed);
        assert!(left > 0 && left < u.first_room(), "the lean point is where less is left than first code took: {} B left, {}", left, u.window_line());
        // from here on `one` keeps its code: no window byte, no version
        let (at, version, used) = (u.fns[one as usize].code, u.fns[one as usize].version, u.code_next.load(Ordering::Relaxed));
        for _ in 0..100 {
            assert_eq!(u.install(one, &code, &relocs, map.clone(), 0).unwrap(), at, "a slot that has code keeps it");
        }
        assert_eq!((u.fns[one as usize].version, u.code_next.load(Ordering::Relaxed)), (version, used), "re-installs past the lean point take nothing");
        assert!(u.window_line().contains("LEAN"), "{}", u.window_line());
        let report = u.lean_report().expect("the lean point is reported");
        assert!(report.contains(&format!("{} x{}", u.fns[one as usize].name, version)), "the report names the slot built again and again: {}", report);
    });
    // a slot without code gets its first code (its lazy compile), and the program runs on
    assert_eq!(run(two_entry), 2);
    crate::owner::call(crate::owner::Class::Program, || {
        assert!(u.fns[two as usize].compiled && u.fns[two as usize].version == 1, "first code installs past the lean point");
    });
    assert_eq!(run(one_entry), 1);
    // a live edit waits for a restart
    let file = u.prog.def(one_d).file;
    let outcome = unsafe { crate::reload::apply(up, vec![crate::reload::Edit { file, path: path.clone(), src: SOURCE.replace("= 1;", "= 11;").into_bytes() }]) };
    assert_eq!(outcome.rung, crate::reload::Rung::Restart, "{:?}", outcome.lines);
    assert!(outcome.lines.iter().any(|l| l.contains("code window is lean")), "{:?}", outcome.lines);
    assert_eq!(run(one_entry), 1, "the edit installed nothing");
    std::fs::remove_dir_all(dir).unwrap();
}
