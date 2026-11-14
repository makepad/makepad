//! `makepad test <package>`: the crate's `#[test]` fns, as rustc's `--test` collects them
//! (rustc_builtin_macros/src/test.rs: `#[test]`, `#[ignore]`, `#[should_panic(expected = ..)]`,
//! the test name = the fn's module path inside its crate), handed as one table to the harness that
//! runs in the program (std::relax::test::main: libtest's semantics).

use crate::jit::{self, FnKey, Unit};
use crate::program::{DefId, DefKind};

/// The loader's side of std::relax::test::Test (repr(C), same field order).
#[repr(C)]
struct Test {
    name: *const u8,
    name_len: usize,
    run: usize,
    start: usize,
    ignore: u32,
    should_panic: u32,
    expected: *const u8,
    expected_len: usize,
    file: *const u8,
    file_len: usize,
    line: u32,
    col: u32,
}

#[repr(C)]
struct Tests {
    tests: *const Test,
    len: usize,
    code: i32,
}

struct Found {
    def: DefId,
    name: String,
    ignore: bool,
    should_panic: u32,
    expected: String,
}

/// The `#[test]` fns of `krates` (each crate's root module is not part of the name).
fn collect(prog: &crate::program::Program, krates: &[u32]) -> Vec<Found> {
    let mut out = Vec::new();
    for (i, d) in prog.defs.iter().enumerate() {
        if d.kind != DefKind::Fn || !krates.contains(&d.krate) {
            continue;
        }
        let f = &prog.files[d.file as usize];
        let it = f.ast.item(d.item);
        let (mut test, mut ignore, mut should_panic, mut expected) = (false, false, 0u32, String::new());
        for a in f.ast.list(it.attrs) {
            if a.toks.hi <= a.toks.lo {
                continue;
            }
            let toks: Vec<&str> = (a.toks.lo..a.toks.hi).map(|t| f.tok_text(t)).collect();
            match toks[0] {
                "test" if toks.len() == 1 => test = true,
                "ignore" => ignore = true,
                "should_panic" => {
                    should_panic = 1;
                    // `should_panic = ".."` or `should_panic(expected = "..")`
                    if let Some(k) = toks.iter().position(|t| *t == "=") {
                        if let Some(lit) = toks.get(k + 1) {
                            expected = relax_front::lit::unescape_str(lit);
                            should_panic = 2;
                        }
                    }
                }
                _ => {}
            }
        }
        if !test {
            continue;
        }
        let path = prog.def_path(DefId(i as u32));
        let name = path.split_once("::").map_or(path.clone(), |(_, rest)| rest.to_string());
        out.push(Found { def: DefId(i as u32), name, ignore, should_panic, expected });
    }
    out
}

fn find_fn(u: &Unit, path: &str) -> Option<DefId> {
    (0..u.prog.defs.len() as u32).map(DefId).find(|d| u.prog.def(*d).kind == DefKind::Fn && u.prog.def_path(*d) == path)
}

/// Runs each crate's tests through the program's harness, crate after crate as cargo runs test
/// binaries (`label`: what cargo's "Running" line names); the exit status (101 after a failing
/// crate; the crates after it still run).
pub(crate) fn run(u: &mut Unit, up: *mut Unit, groups: &[(String, u32)]) -> i32 {
    let mut code = 0;
    for (label, krate) in groups {
        eprintln!("     Running {label}");
        let c = match run_crate(u, up, *krate) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("makepad test: {e}");
                101
            }
        };
        if c != 0 {
            code = 101;
        }
    }
    code
}

fn run_crate(u: &mut Unit, up: *mut Unit, krate: u32) -> Result<i32, String> {
    let found = collect(&u.prog, &[krate]);
    let main = find_fn(u, "std::relax::test::main").ok_or("std has no relax::test::main (the test harness)")?;
    let start = find_fn(u, "std::relax::test::__relax_test_start");
    let (addrs, main_entry) = crate::owner::call(crate::owner::Class::Program, || -> Result<_, String> {
        let mut addrs = Vec::with_capacity(found.len());
        for t in &found {
            let ret = u.tcx.fn_sig(&u.prog, t.def).map(|s| s.ret);
            let id = u.fn_id(FnKey::Inst(t.def, Vec::new()));
            let run = u.entry(id) as usize;
            // a test returning a value runs through Termination (libtest assert_test_result)
            let start = match ret {
                Some(r) if r != u.tcx.tys.unit => {
                    let sd = start.ok_or("std has no relax::test::__relax_test_start")?;
                    let sid = u.fn_id(FnKey::Inst(sd, vec![r]));
                    u.entry(sid) as usize
                }
                _ => 0,
            };
            addrs.push((run, start));
        }
        let id = u.fn_id(FnKey::Inst(main, Vec::new()));
        Ok((addrs, u.entry(id)))
    })?;
    // the table lives for the process (the harness's threads read it until the exit): the kept heap
    let tests: &'static mut Tests = crate::zone_alloc::kept_with(|| {
        let table: Vec<Test> = found
            .iter()
            .zip(&addrs)
            .map(|(t, &(run, start))| {
                let name: &'static str = Box::leak(t.name.clone().into_boxed_str());
                let expected: &'static str = Box::leak(t.expected.clone().into_boxed_str());
                Test {
                    name: name.as_ptr(),
                    name_len: name.len(),
                    run,
                    start,
                    ignore: t.ignore as u32,
                    should_panic: t.should_panic,
                    expected: expected.as_ptr(),
                    expected_len: expected.len(),
                    file: std::ptr::null(),
                    file_len: 0,
                    line: 0,
                    col: 0,
                }
            })
            .collect();
        let table: &'static [Test] = Box::leak(table.into_boxed_slice());
        Box::leak(Box::new(Tests { tests: table.as_ptr(), len: table.len(), code: 101 }))
    });
    let tests: *mut Tests = tests;
    match unsafe { jit::call_jit(up, main_entry, tests as u64) } {
        Ok(_) => Ok(unsafe { std::ptr::read_volatile(&(*tests).code) }),
        Err(p) => Err(format!("the test harness {}: {}", p.kind, p.message)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = r#"
static mut FLAGS: [u8; 2] = [0; 2];
trait Work { fn indirect_target(&self) -> u32; }
struct Job;
impl Work for Job {
    #[inline(never)]
    fn indirect_target(&self) -> u32 { 7 }
}
#[inline(never)]
fn invoke(job: &dyn Work) -> u32 { job.indirect_target() }
#[inline(never)]
fn table_first() -> u32 { 11 }
#[inline(never)]
fn table_second() -> u32 { 13 }
const CALLBACKS: (u32, [fn() -> u32; 2]) = (1, [table_first, table_second]);
#[test]
fn chosen_a() {
    extern "C" { fn __relax_precompile(roots: *const usize, len: usize, exhaustive: bool) -> u64; }
    // Supplied by the host, so these addresses are not statically reachable from this body.
    let roots: Vec<usize> = std::env::args().next().unwrap().split(',').map(|s| s.parse().unwrap()).collect();
    assert_eq!(unsafe { __relax_precompile(roots.as_ptr(), roots.len(), true) }, 0,
        "selected tests, their Termination adapter and indirect callees must be prepared before any test starts");
    unsafe { FLAGS[0] += 1; }
}
#[test]
fn chosen_result() -> Result<(), ()> {
    assert_eq!(invoke(&Job), 7);
    // Host input keeps both table elements reachable instead of folding a constant index.
    assert_eq!(CALLBACKS.1[std::env::args().count() % 2](), 11);
    unsafe { FLAGS[1] += 1; }
    Ok(())
}
#[test]
#[ignore]
fn chosen_ignored() { panic!("ignored test ran"); }
#[test]
fn excluded() { panic!("filtered test ran"); }
pub fn flags(out: *mut [u8; 2]) { unsafe { *out = FLAGS; } }
"#;

    fn fixture(args: &[&str]) -> (Box<Unit>, u32, std::path::PathBuf, u32) {
        std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
        let root = std::env::temp_dir().join(format!("relax-harness-prepare-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("lib.rs");
        std::fs::write(&path, SOURCE).unwrap();
        let mut session = crate::Session::default();
        session.options.tier1 = false;
        session.options.tyck_ahead = false;
        let (prog, core, krate) = crate::load_with_std(&std::sync::Arc::new(session), path.to_str().unwrap(), "harness_fixture", true).unwrap();
        crate::layout::set_root_crate(krate);
        let mut tcx = crate::tcx::Tcx::new();
        tcx.collect(&prog);
        assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
        let mut u = Unit::new(prog, tcx, core);
        unsafe { jit::UNIT = &mut *u; }
        let (first_arg, callback) = crate::owner::call(crate::owner::Class::Program, || {
            let result = find_fn(&u, "harness_fixture::chosen_result").unwrap();
            let ret = u.tcx.fn_sig(&u.prog, result).unwrap().ret;
            let start = find_fn(&u, "std::relax::test::__relax_test_start").unwrap();
            let adapter = u.fn_id(FnKey::Inst(start, vec![ret]));
            let selected = u.fn_id(FnKey::Inst(result, Vec::new()));
            let def = u.prog.defs.iter().enumerate().find_map(|(i, d)| {
                (u.prog.syms.str(d.name) == "indirect_target" && u.prog.def(d.parent).kind == DefKind::Impl)
                    .then_some(DefId(i as u32))
            }).unwrap();
            let callback = u.fn_id(FnKey::Inst(def, Vec::new()));
            let table: Vec<_> = ["harness_fixture::table_first", "harness_fixture::table_second"].iter().map(|name| {
                let def = find_fn(&u, name).unwrap();
                let slot = u.fn_id(FnKey::Inst(def, Vec::new()));
                u.entry(slot).to_string()
            }).collect();
            (format!("{},{},{},{}", u.entry(adapter), u.entry(selected), u.entry(callback), table.join(",")).into_bytes(), callback)
        });
        let mut argv = vec![first_arg];
        argv.extend(args.iter().map(|s| s.as_bytes().to_vec()));
        jit::set_program_args(argv);
        (u, krate, root, callback)
    }

    fn finish(mut u: Box<Unit>, root: std::path::PathBuf, executed: [u8; 2], callback: u32) {
        crate::owner::call(crate::owner::Class::Program, || {
            for name in ["harness_fixture::chosen_ignored", "harness_fixture::excluded"] {
                let def = find_fn(&u, name).unwrap();
                let slot = u.fn_id(FnKey::Inst(def, Vec::new()));
                assert!(!u.fns[slot as usize].compiled, "unselected body was prepared: {name}");
            }
        });
        assert_eq!(u.fns[callback as usize].compiled, executed[0] != 0, "indirect callback preparation");
        let entry = crate::owner::call(crate::owner::Class::Program, || {
            let def = find_fn(&u, "harness_fixture::flags").unwrap();
            let slot = u.fn_id(FnKey::Inst(def, Vec::new()));
            u.entry(slot)
        });
        let up = &mut *u as *mut Unit;
        let mut flags = [0u8; 2];
        unsafe { jit::call_jit(up, entry, &mut flags as *mut [u8; 2] as u64) }.map_err(|p| p.message).unwrap();
        assert_eq!(flags, executed);
        // The one-shot CLI keeps its unit alive while worker threads retire after reporting.
        Box::leak(u);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn precompile_respects_filter_ignore_and_return_values() {
        isolated_test!(precompile_respects_filter_ignore_and_return_values);
        let (mut u, krate, root, callback) = fixture(&["chosen", "--precompile", "--test-threads=1"]);
        let found = collect(&u.prog, &[krate]);
        assert_eq!(found.len(), 4);
        assert_eq!(found.iter().filter(|t| t.ignore).count(), 1);
        let up = &mut *u as *mut Unit;
        assert_eq!(run(&mut u, up, &[("fixture".into(), krate)]), 0);
        finish(u, root, [1, 1], callback); // each selected body ran exactly once; none ran as a warm-up
    }

    #[test]
    fn precompile_follows_previously_compiled_code_after_ir_eviction() {
        isolated_test!(precompile_follows_previously_compiled_code_after_ir_eviction);
        let (mut u, krate, root, callback) = fixture(&["chosen", "--precompile", "--test-threads=1"]);
        let slot = crate::owner::call(crate::owner::Class::Program, || {
            let def = find_fn(&u, "harness_fixture::chosen_result").unwrap();
            u.fn_id(FnKey::Inst(def, Vec::new()))
        });
        let up = &mut *u as *mut Unit;
        unsafe { jit::compile_slot(up, slot); }
        crate::owner::call(crate::owner::Class::Program, || {
            // Model completed-body eviction before this run's code has reached the disk cache.
            u.fns[slot as usize].rir = None;
            u.cache = None;
        });
        assert_eq!(run(&mut u, up, &[("fixture".into(), krate)]), 0);
        finish(u, root, [1, 1], callback);
    }

    #[test]
    fn precompile_list_does_not_prepare_or_run_test_bodies() {
        isolated_test!(precompile_list_does_not_prepare_or_run_test_bodies);
        let (mut u, krate, root, callback) = fixture(&["chosen", "--list", "--precompile"]);
        let up = &mut *u as *mut Unit;
        assert_eq!(run(&mut u, up, &[("fixture".into(), krate)]), 0);
        crate::owner::call(crate::owner::Class::Program, || {
            for t in collect(&u.prog, &[krate]) {
                let slot = u.fn_id(FnKey::Inst(t.def, Vec::new()));
                assert!(!u.fns[slot as usize].compiled, "--list prepared {}", t.name);
            }
        });
        finish(u, root, [0, 0], callback);
    }
}
