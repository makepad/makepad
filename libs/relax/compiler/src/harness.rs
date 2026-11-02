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
        eprintln!("makepad test timing: prepare_ms={:.3} tests={} reused=false", started.elapsed().as_secs_f64() * 1000.0, table.len());
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
        let mut v = live::Verdict { ran: 0, passed: 0, ignored: 0, failures: Vec::new(), stale: Vec::new(), why: run.why.clone(), skipped: 0 };
        for (test, found) in run.table.iter().zip(&run.found) {
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
                    Err(e) => { eprintln!("makepad test: {e}"); t.code = 101; return 0; }
                }
            }
            for (i, test) in run.table.iter().enumerate() {
                if test.selected != 0 { reach::pending(i); }
            }
            run.overhead_ms = started.elapsed().as_secs_f64() * 1000.0;
            if t.verbose != 0 {
                let changed: Vec<_> = edit.changed.iter().flatten().map(|d| u.prog.def_path(*d)).collect();
                eprintln!("makepad test selection: changed={changed:?} select_ms={:.3} why={:?}", run.overhead_ms, run.why);
            }
            1
        })
    })
}
