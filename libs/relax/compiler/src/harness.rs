//! `makepad test <package>`: the crate's `#[test]` fns, as rustc's `--test` collects them
//! (rustc_builtin_macros/src/test.rs: `#[test]`, `#[ignore]`, `#[should_panic(expected = ..)]`,
//! the test name = the fn's module path inside its crate), handed as one table to the harness that
//! runs in the program (std::relax::test::main: libtest's semantics).

use crate::jit::{self, FnKey, Unit};
use crate::program::{DefId, DefKind};

#[path = "harness_live.rs"]
pub(crate) mod live;
#[path = "harness_reach.rs"]
pub(crate) mod reach;

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
    result: u32,
    selected: u32,
    eligible: u32,
}

#[repr(C)]
struct Tests {
    tests: *mut Test,
    len: usize,
    code: i32,
    next: usize,
    fault: usize,
    reach: usize,
    context: usize,
    revision: u64,
    verbose: u32,
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
        if live::enabled() && prog.mods[d.scope as usize].values.get(&d.name) != Some(&DefId(i as u32)) {
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
/// crate; the crates after it still run; NO_TESTS_EXIT when a filter matched no test in any).
pub(crate) fn run(u: &mut Unit, up: *mut Unit, groups: &[(String, u32)]) -> i32 {
    let (mut code, mut matched, mut unmatched) = (0, false, false);
    for (label, krate) in groups {
        // a silent run's stderr is the program's (a test re-running itself reads it)
        if !crate::run::silent() {
            eprintln!("     Running {label}");
        }
        let c = match run_crate(u, up, *krate) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("relax test: {e}");
                101
            }
        };
        if c == makepad_remote::hot::NO_TESTS_EXIT {
            unmatched = true;
            continue;
        }
        matched = true;
        if c != 0 {
            code = 101;
        }
    }
    if unmatched && !matched { makepad_remote::hot::NO_TESTS_EXIT } else { code }
}

struct Run {
    up: *mut Unit,
    krate: u32,
    found: Vec<Found>,
    table: Vec<Test>,
    why: Option<String>,
    initial: bool,
    overhead_ms: f64,
}

/// Recollect after an edit so added tests and changed attributes take the same filters.
fn prepare(u: &mut Unit, krate: u32) -> Result<(Vec<Found>, Vec<Test>, u64), String> {
    let started = std::time::Instant::now();
    let _k = crate::zone_alloc::kept();
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
    let table: Vec<Test> = found
        .iter()
        .zip(&addrs)
        .map(|(t, &(run, start))| Test {
            name: t.name.as_ptr(),
            name_len: t.name.len(),
            run,
            start,
            ignore: t.ignore as u32,
            should_panic: t.should_panic,
            expected: t.expected.as_ptr(),
            expected_len: t.expected.len(),
            file: std::ptr::null(),
            file_len: 0,
            line: 0,
            col: 0,
            result: 0,
            selected: 1,
            eligible: 0,
        })
        .collect();
    if u.session.options.verbose {
        eprintln!("relax test timing: prepare_ms={:.3} tests={} reused=false", started.elapsed().as_secs_f64() * 1000.0, table.len());
    }
    Ok((found, table, main_entry))
}

fn run_crate(u: &mut Unit, up: *mut Unit, krate: u32) -> Result<i32, String> {
    let (tests, main_entry) = crate::zone_alloc::kept_with(|| -> Result<_, String> {
        let (found, table, main_entry) = crate::owner::call(crate::owner::Class::Program, || prepare(u, krate))?;
        let run = Box::leak(Box::new(Run { up, krate, found, table, why: None, initial: true, overhead_ms: 0.0 }));
        if live::enabled() { reach::reset(run.table.len()); }
        let tests = Box::leak(Box::new(Tests {
            tests: run.table.as_mut_ptr(), len: run.table.len(), code: 101,
            next: if live::enabled() { next_run as *const () as usize } else { 0 },
            fault: if live::enabled() { live::take_fault as *const () as usize } else { 0 },
            reach: if live::enabled() { reach::test as *const () as usize } else { 0 },
            context: run as *mut Run as usize,
            revision: 0,
            verbose: u.session.options.verbose as u32,
        }));
        Ok((tests, main_entry))
    })?;
    let tests: *mut Tests = tests;
    let _program = crate::os::program_thread();
    match unsafe { jit::call_jit(up, main_entry, tests as u64) } {
        Ok(_) => Ok(unsafe { std::ptr::read_volatile(&(*tests).code) }),
        Err(p) => Err(format!("the test harness {}: {}", p.kind, p.message)),
    }
}

/// The program calls only after its test threads joined; no program frame reads the old table.
extern "C" fn next_run(t: *mut Tests, ms: f64) -> u32 {
    jit::host_boundary("test next run", || unsafe {
        let _k = crate::zone_alloc::kept();
        let t = &mut *t;
        let run = &mut *(t.context as *mut Run);
        if run.initial {
            let n = run.table.iter().filter(|t| t.eligible != 0 && t.result != 3).count();
            run.why = Some(format!("unknown reach: {n} tests"));
            run.initial = false;
        }
        let mut v = live::Verdict { ran: 0, passed: 0, ignored: 0, failures: Vec::new(), stale: Vec::new(), why: run.why.clone(), skipped: 0, tests: Vec::with_capacity(run.table.len()) };
        for (test, found) in run.table.iter().zip(&run.found) {
            let status = match test.result {
                1 => live::TestStatus::Passed,
                2 => live::TestStatus::Failed,
                3 => live::TestStatus::Ignored,
                _ => live::TestStatus::Filtered,
            };
            v.tests.push(live::TestResult { name: found.name.clone(), status, ran: test.selected != 0 && matches!(test.result, 1 | 2) });
            if test.result != 0 && test.selected == 0 { v.skipped += 1; }
            if matches!(test.result, 1 | 2) && test.selected != 0 { v.ran += 1; }
            match test.result {
                1 => v.passed += 1,
                2 => {
                    v.failures.push(found.name.clone());
                    if test.selected == 0 { v.stale.push(found.name.clone()); }
                }
                3 => v.ignored += 1,
                _ => {}
            }
        }
        t.code = if v.failures.is_empty() { 0 } else { 101 };
        let edit = live::complete(v, ms + run.overhead_ms);
        let started = std::time::Instant::now();
        crate::owner::call(crate::owner::Class::Program, || {
            let u = &mut *run.up;
            run.why = match edit.changed.as_deref() {
                None => Some("non-body edit".to_string()),
                Some(changed) => {
                    // an unattributed changed fn may run in any test: all; so may one no test
                    // reached here (it runs in a process a test started, or before the captures,
                    // or is reached only through data such as Splash script): when unsure, run.
                    // A test whose reach is unknown (it never finished a recorded run) or that
                    // started a process (an app it drives) reruns itself, the rest by reach
                    let (reach, unattributed) = reach::snapshot(u);
                    let why = changed.iter().find(|d| unattributed.contains(*d)).map(|d| format!("unattributed: {}", u.prog.def_path(*d)))
                        .or_else(|| changed.iter().find(|d| !reach.iter().any(|r| r.reaches(**d))).map(|d| format!("reached by no test here: {}", u.prog.def_path(*d))));
                    for (i, test) in run.table.iter_mut().enumerate() {
                        let unknown = reach.get(i).is_none_or(|r| r.unknown || r.spawned);
                        test.selected = (test.eligible != 0 && (why.is_some() || unknown ||
                            test.result != 3 && reach[i].affected(changed))) as u32;
                    }
                    why
                }
            };
            if edit.changed.is_none() {
                match prepare(u, run.krate) {
                    Ok((found, table, _)) => {
                        run.table = table;
                        run.found = found;
                        reach::reset(run.table.len());
                        t.tests = run.table.as_mut_ptr();
                        t.len = run.table.len();
                        t.revision += 1;
                    }
                    Err(e) => { eprintln!("relax test: {e}"); t.code = 101; return 0; }
                }
            }
            for (i, test) in run.table.iter().enumerate() {
                if test.selected != 0 { reach::pending(i); }
            }
            run.overhead_ms = started.elapsed().as_secs_f64() * 1000.0;
            if t.verbose != 0 {
                let changed: Vec<_> = edit.changed.iter().flatten().map(|d| u.prog.def_path(*d)).collect();
                eprintln!("relax test selection: changed={changed:?} select_ms={:.3} why={:?}", run.overhead_ms, run.why);
            }
            1
        })
    })
}

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    live::reset_statics();
    reach::reset_statics();
}

#[cfg(test)]
mod tests {
    use super::*;

    // The first test asks the loader to build the second test and its return value's adapter
    // (their addresses come in as argv[0]): with `--precompile` the harness built them, with all
    // they call, before the first test started, so nothing is left to build.
    const SOURCE: &str = r#"
use std::sync::atomic::{AtomicU32, Ordering};
static RAN: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];
extern "C" {
    fn __relax_precompile(roots: *const usize, len: usize, exhaustive: bool) -> u64;
}
#[test]
fn chosen_first() {
    let roots: Vec<usize> = std::env::args().next().unwrap().split(',').map(|s| s.parse().unwrap()).collect();
    let built = unsafe { __relax_precompile(roots.as_ptr(), roots.len(), true) };
    assert_eq!(built, 0, "a selected test still had code to build when the first test started");
    RAN[0].fetch_add(1, Ordering::SeqCst);
}
#[test]
fn chosen_second() -> Result<(), String> {
    RAN[1].fetch_add(1, Ordering::SeqCst);
    Ok(())
}
#[test]
#[ignore]
fn chosen_ignored() {
    panic!("an ignored test ran");
}
#[test]
fn other() {
    panic!("a filtered-out test ran");
}
pub fn ran(out: *mut [u32; 2]) {
    unsafe { *out = [RAN[0].load(Ordering::SeqCst), RAN[1].load(Ordering::SeqCst)] };
}
"#;

    /// `-- --precompile` with a filter and one thread: the selected tests (one returning a value)
    /// are built before the first starts, the ignored and the filtered-out ones are not built, and
    /// each selected test runs once (none as a warm-up).
    #[test]
    fn precompile_builds_the_selected_tests_before_the_first_starts() {
        isolated_test!(precompile_builds_the_selected_tests_before_the_first_starts);
        if std::env::var_os("RELAX_STD").is_none() {
            std::env::set_var("RELAX_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
        }
        let dir = std::env::temp_dir().join(format!("relax-harness-precompile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lib.rs");
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
        let up = &mut *u as *mut Unit;
        unsafe { jit::UNIT = up };
        let slot = |u: &mut Unit, path: &str| {
            let d = find_fn(u, path).unwrap_or_else(|| panic!("no {path}"));
            u.fn_id(FnKey::Inst(d, Vec::new()))
        };
        let roots = crate::owner::call(crate::owner::Class::Program, || {
            let second = find_fn(&u, "harness_fixture::chosen_second").unwrap();
            let ret = u.tcx.fn_sig(&u.prog, second).unwrap().ret;
            let start = find_fn(&u, "std::relax::test::__relax_test_start").unwrap();
            let adapter = u.fn_id(FnKey::Inst(start, vec![ret]));
            let second = slot(&mut u, "harness_fixture::chosen_second");
            format!("{},{}", u.entry(second), u.entry(adapter))
        });
        jit::set_program_args(vec![roots.into_bytes(), b"chosen".to_vec(), b"--precompile".to_vec(), b"--test-threads=1".to_vec()]);
        assert_eq!(run(&mut u, up, &[("fixture".into(), krate)]), 0, "the selected tests pass");
        let (unbuilt, ran) = crate::owner::call(crate::owner::Class::Program, || {
            let unbuilt: Vec<bool> = ["harness_fixture::chosen_ignored", "harness_fixture::other"]
                .iter()
                .map(|p| {
                    let id = slot(&mut u, p);
                    !u.fns[id as usize].compiled
                })
                .collect();
            let ran = slot(&mut u, "harness_fixture::ran");
            (unbuilt, u.entry(ran))
        });
        assert_eq!(unbuilt, [true, true], "the ignored and the filtered-out test stay unbuilt");
        let mut counts = [0u32; 2];
        unsafe { jit::call_jit(up, ran, &mut counts as *mut [u32; 2] as u64) }.unwrap_or_else(|p| panic!("{}", p.message));
        assert_eq!(counts, [1, 1], "each selected test ran once");
        // the harness's threads may still be retiring: the unit outlives them, as the CLI's does
        Box::leak(u);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
