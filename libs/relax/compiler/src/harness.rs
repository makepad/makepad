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
/// crate; without `no_fail_fast` the crates after it do not run).
pub(crate) fn run(u: &mut Unit, up: *mut Unit, groups: &[(String, u32)], no_fail_fast: bool) -> i32 {
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
            if !no_fail_fast {
                eprintln!("error: test failed, to rerun pass `--no-fail-fast` to run the remaining targets");
                break;
            }
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
