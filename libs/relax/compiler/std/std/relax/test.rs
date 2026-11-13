//! The test harness of `makepad test <package>`: what rustc's `--test` main does with libtest
//! (library/test: lib.rs run_test, test_result.rs calc_result, console.rs, cli.rs), running in
//! the program. The loader collects the crate's `#[test]` fns and hands their table to `main`;
//! the command line after `--` is libtest's (filters, --exact, --skip, --ignored,
//! --include-ignored, --test-threads, --list, --nocapture, -q). Relax's own `--precompile` builds
//! the selected tests and all they reach before the first one starts, so no lazy compile runs
//! inside a test's own deadlines.
//! Not here: output capture (every test prints as under --nocapture), benches, --format.

use crate::string::String;
use crate::vec::Vec;
use crate::sync::{mpsc, Arc};
use crate::io::Write;

/// One `#[test]` fn (the loader's table; repr(C), filled by the loader).
#[repr(C)]
pub struct Test {
    pub name: *const u8,
    pub name_len: usize,
    /// the test fn (`fn()`), or, with `start`, the `fn() -> T` that `start` runs
    pub run: usize,
    /// `__relax_test_start::<T>` for a test returning a value, else 0
    pub start: usize,
    /// `#[ignore]`
    pub ignore: u32,
    /// 0: no `#[should_panic]`, 1: `#[should_panic]`, 2: with `expected`
    pub should_panic: u32,
    pub expected: *const u8,
    pub expected_len: usize,
    pub file: *const u8,
    pub file_len: usize,
    pub line: u32,
    pub col: u32,
    /// Last result: 0 unrun/filtered, 1 passed, 2 failed, 3 ignored.
    pub result: u32,
    /// Whether this run includes the test; a skipped test keeps its last result.
    pub selected: u32,
    /// Included by the libtest filters.
    pub eligible: u32,
}

#[repr(C)]
pub struct Tests {
    pub tests: *mut Test,
    pub len: usize,
    /// main's answer: the exit status (0, or 101 when a test failed)
    pub code: i32,
    /// Host callbacks and context; zero callbacks mean the ordinary one-shot harness.
    pub next: usize,
    pub fault: usize,
    pub reach: usize,
    pub context: usize,
    pub revision: u64,
    pub verbose: u32,
}

struct Desc {
    index: usize,
    name: String,
    run: usize,
    start: usize,
    ignore: bool,
    should_panic: u32,
    expected: String,
    file: String,
    line: u32,
    col: u32,
}

unsafe fn text(p: *const u8, n: usize) -> String {
    if n == 0 {
        return String::new();
    }
    String::from_utf8_lossy(unsafe { crate::slice::from_raw_parts(p, n) }).into_owned()
}

/// A test fn returning a value: `report()` must be success (libtest assert_test_result).
#[doc(hidden)]
pub fn __relax_test_start<T: crate::process::Termination>(test: fn() -> T) {
    let code = test().report().to_i32();
    if code != 0 {
        panic!("the test returned a termination value with a non-zero status code ({}) which indicates a failure", code);
    }
}

struct Opts {
    filters: Vec<String>,
    skip: Vec<String>,
    exact: bool,
    ignored: bool,
    include_ignored: bool,
    list: bool,
    /// libtest OutputFormat::Terse (`-q` / `--format=terse`): one character per test
    quiet: bool,
    threads: usize,
    /// `--precompile`: build every selected test's code graph before any test starts
    precompile: bool,
}

/// libtest cli.rs get_format: `--format` (pretty, terse; json and junit only on nightly with
/// -Z unstable-options), terse also with `--quiet`.
fn format_terse(v: &str) -> Result<bool, String> {
    match v {
        "pretty" => Ok(false),
        "terse" => Ok(true),
        "json" | "junit" => Err(format!("The \"{}\" format is only accepted on the nightly compiler with -Z unstable-options", v)),
        _ => Err(format!("argument for --format must be pretty, terse, json or junit (was {})", v)),
    }
}

fn parse_args() -> Result<Opts, String> {
    let mut o = Opts { filters: Vec::new(), skip: Vec::new(), exact: false, ignored: false, include_ignored: false, list: false, quiet: false, threads: 0, precompile: false };
    let mut format: Option<bool> = None;
    let mut args = crate::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--exact" => o.exact = true,
            "--precompile" => o.precompile = true,
            "--ignored" => o.ignored = true,
            "--include-ignored" => o.include_ignored = true,
            "--list" => o.list = true,
            "-q" | "--quiet" => o.quiet = true,
            // output is never captured here
            "--nocapture" | "--no-capture" | "--show-output" => {}
            "--skip" => o.skip.push(args.next().ok_or("--skip needs a value")?),
            "--test-threads" => o.threads = args.next().and_then(|v| v.parse().ok()).filter(|&n| n > 0).ok_or("--test-threads must be a positive integer")?,
            _ if a.starts_with("--test-threads=") => o.threads = a[15..].parse().ok().filter(|&n| n > 0).ok_or("--test-threads must be a positive integer")?,
            _ if a.starts_with("--skip=") => o.skip.push(a[7..].into()),
            "--format" => format = Some(format_terse(&args.next().ok_or("--format needs a value")?)?),
            _ if a.starts_with("--format=") => format = Some(format_terse(&a[9..])?),
            _ if a.starts_with('-') => return Err(format!("Unrecognized option: '{}'", a.trim_start_matches('-'))),
            _ => o.filters.push(a),
        }
    }
    // an explicit --format wins over --quiet (get_format: `None if quiet => Terse`)
    if let Some(terse) = format {
        o.quiet = terse;
    }
    if o.threads == 0 {
        o.threads = match crate::env::var("RUST_TEST_THREADS") {
            Ok(v) => v.parse().ok().filter(|&n: &usize| n > 0).ok_or("RUST_TEST_THREADS is `{v}`, should be a positive integer.")?,
            Err(_) => crate::thread::available_parallelism().map_or(1, |n| n.get()),
        };
    }
    Ok(o)
}

enum Outcome {
    Ok,
    Failed(Option<String>),
    Ignored,
}

/// libtest calc_result: the test's panic (payload) against its `#[should_panic]`.
fn calc_result(d: &Desc, panic: Option<&(dyn crate::any::Any + Send)>) -> Outcome {
    match (d.should_panic, panic) {
        (0, None) | (1, Some(_)) => Outcome::Ok,
        (2, Some(err)) => {
            let s = err.downcast_ref::<String>().map(|e| e.as_str()).or_else(|| err.downcast_ref::<&'static str>().copied());
            match s {
                Some(e) if e.contains(d.expected.as_str()) => Outcome::Ok,
                Some(e) => Outcome::Failed(Some(format!(
                    "panic did not contain expected string\n      panic message: {:?}\n expected substring: {:?}",
                    e, d.expected
                ))),
                None => Outcome::Failed(Some(format!(
                    "expected panic with string value,\n found non-string value: `{:?}`\n     expected substring: {:?}",
                    (*err).type_id(), d.expected
                ))),
            }
        }
        (_, None) => {
            let at = if d.file.is_empty() { String::new() } else { format!(" at {}:{}:{}", d.file, d.line, d.col) };
            Outcome::Failed(Some(format!("test did not panic as expected{}", at)))
        }
        (_, Some(_)) => Outcome::Failed(None),
    }
}

fn run_one(d: &Desc, fault: usize, reach: usize, verbose: bool) -> Outcome {
    let record: Option<extern "C" fn(usize, u32)> = if reach == 0 { None } else { Some(unsafe { crate::mem::transmute(reach) }) };
    if let Some(record) = record { record(d.index, 0); }
    let (run, start) = (d.run, d.start);
    let began = verbose.then(crate::time::Instant::now);
    let body_begin = crate::cell::Cell::new(None);
    let body_end = crate::cell::Cell::new(None);
    // a test's panic fails it, unless it should panic: that one the test expects
    let body = crate::panic::AssertUnwindSafe(|| unsafe {
        if verbose { body_begin.set(Some(crate::time::Instant::now())); }
        if start == 0 {
            let f: fn() = crate::mem::transmute(run);
            f()
        } else {
            let f: fn(usize) = crate::mem::transmute(start);
            f(run)
        }
        if verbose { body_end.set(Some(crate::time::Instant::now())); }
    });
    let r = if d.should_panic != 0 { crate::panic::catch_unwind(body) } else { crate::panic::catch_boundary(body) };
    let caught = verbose.then(crate::time::Instant::now);
    let faulted = if fault != 0 {
        let fault: extern "C" fn() -> u32 = unsafe { crate::mem::transmute(fault) };
        fault() != 0
    } else { false };
    let result = if faulted { Outcome::Failed(Some(String::from("a hardware fault in test code"))) } else { match r {
        Ok(()) => calc_result(d, None),
        Err(p) => calc_result(d, Some(&*p)),
    } };
    if let (Some(began), Some(caught), Some(body_begin)) = (began, caught, body_begin.get()) {
        let body_end = body_end.get().unwrap_or(caught);
        let body_ms = body_end.duration_since(body_begin).as_secs_f64() * 1000.0;
        let catch_ms = body_begin.duration_since(began).as_secs_f64() * 1000.0 + caught.duration_since(body_end).as_secs_f64() * 1000.0;
        eprintln!("relax test timing: test={:?} body_unwind_ms={:.3} catch_ms={:.3} result_ms={:.3}",
            d.name, body_ms, catch_ms, caught.elapsed().as_secs_f64() * 1000.0);
    }
    if let Some(record) = record { record(d.index, if faulted { 2 } else { 1 }); }
    result
}

/// Runs the table's tests; a live host callback waits for the next verified edit.
pub fn main(t: &mut Tests) {
    let setup = crate::time::Instant::now();
    let opts = match parse_args() {
        Ok(o) => o,
        Err(e) => { eprintln!("error: {}", e); t.code = 101; return; }
    };
    let (mut descs, mut filtered_out) = prepare(t, &opts);
    let mut revision = t.revision;
    if t.verbose != 0 { eprintln!("relax test timing: harness_setup_ms={:.3} reused=false", setup.elapsed().as_secs_f64() * 1000.0); }
    if opts.list {
        for d in descs.iter() { println!("{}: test", d.name); }
        if !opts.quiet { println!("\n{} tests, 0 benchmarks", descs.len()); }
        t.code = 0;
        return;
    }
    loop {
        let started = crate::time::Instant::now();
        if revision != t.revision {
            let setup = crate::time::Instant::now();
            (descs, filtered_out) = prepare(t, &opts);
            revision = t.revision;
            if t.verbose != 0 { eprintln!("relax test timing: harness_setup_ms={:.3} reused=false", setup.elapsed().as_secs_f64() * 1000.0); }
        }
        t.code = run(t, &opts, descs.clone(), filtered_out);
        if t.next == 0 { break; }
        let next: extern "C" fn(*mut Tests, f64) -> u32 = unsafe { crate::mem::transmute(t.next) };
        if next(t, started.elapsed().as_secs_f64() * 1000.0) == 0 { break; }
    }
}

fn prepare(t: &Tests, opts: &Opts) -> (Arc<Vec<Desc>>, usize) {
    let mut all: Vec<Desc> = Vec::with_capacity(t.len);
    for i in 0..t.len {
        let x = unsafe { &*t.tests.add(i) };
        all.push(Desc {
            index: i,
            name: unsafe { text(x.name, x.name_len) },
            run: x.run,
            start: x.start,
            ignore: x.ignore != 0,
            should_panic: x.should_panic,
            expected: unsafe { text(x.expected, x.expected_len) },
            file: unsafe { text(x.file, x.file_len) },
            line: x.line,
            col: x.col,
        });
    }
    // libtest filter_tests: name filters (substring, or --exact), --skip, then --ignored
    let total = all.len();
    let matches = |name: &str, f: &str| if opts.exact { name == f } else { name.contains(f) };
    all.retain(|d| (opts.filters.is_empty() || opts.filters.iter().any(|f| matches(&d.name, f))) && !opts.skip.iter().any(|f| matches(&d.name, f)));
    if opts.ignored {
        all.retain(|d| d.ignore);
        for d in &mut all {
            d.ignore = false;
        }
    } else if opts.include_ignored {
        for d in &mut all {
            d.ignore = false;
        }
    }
    for d in &all { unsafe { (*t.tests.add(d.index)).eligible = 1; } }
    let filtered_out = total - all.len();
    all.sort_by(|a, b| a.name.cmp(&b.name));
    (Arc::new(all), filtered_out)
}

fn run(t: &mut Tests, opts: &Opts, descs: Arc<Vec<Desc>>, filtered_out: usize) -> i32 {
    let selected: Vec<usize> = (0..descs.len()).filter(|&i| unsafe { (*t.tests.add(descs[i].index)).selected != 0 }).collect();
    if t.verbose != 0 {
        let names: Vec<&str> = selected.iter().map(|&i| descs[i].name.as_str()).collect();
        eprintln!("relax test selection: selected={:?}", names);
    }
    // every test that will run, with its return value's adapter, in one walk before the first
    // starts: a build beside a running test would still spend that test's deadlines
    if opts.precompile {
        extern "C" {
            fn __relax_precompile(roots: *const usize, len: usize, exhaustive: bool) -> u64;
        }
        let mut roots = Vec::with_capacity(selected.len() * 2);
        for &i in &selected {
            let d = &descs[i];
            if !d.ignore {
                roots.push(d.run);
                if d.start != 0 {
                    roots.push(d.start);
                }
            }
        }
        let built = unsafe { __relax_precompile(roots.as_ptr(), roots.len(), true) };
        if t.verbose != 0 {
            eprintln!("relax test precompile: roots={} built={}", roots.len(), built);
        }
    }
    let started = crate::time::Instant::now();
    println!();
    println!("running {} test{}", selected.len(), if selected.len() == 1 { "" } else { "s" });
    let mut passed = 0usize;
    let mut ignored = 0usize;
    let mut failures: Vec<(String, Option<String>)> = Vec::new();
    // libtest formatters/terse.rs: the tests reported so far and the characters on this line
    let (mut test_count, mut test_column) = (0usize, 0usize);
    let total_count = selected.len();
    // libtest's pretty formatter names the mode of a `#[should_panic]` test
    let mut report = |name: &str, should_panic: bool, o: Outcome, failures: &mut Vec<(String, Option<String>)>| {
        let mode = if should_panic { " - should panic" } else { "" };
        let word = match &o {
            Outcome::Ok => "ok",
            Outcome::Failed(_) => "FAILED",
            Outcome::Ignored => "ignored",
        };
        if opts.quiet {
            // TerseFormatter: a character per passed / ignored test, the progress every 88
            // columns, a line per failed one
            const QUIET_MODE_MAX_COLUMN: usize = 88;
            match &o {
                Outcome::Failed(_) => {
                    if test_column != 0 {
                        print!(" {}/{}\n", test_count, total_count);
                        test_column = 0;
                    }
                    test_count += 1;
                    println!("{} --- FAILED", name);
                }
                _ => {
                    print!("{}", if matches!(o, Outcome::Ok) { "." } else { "i" });
                    test_count += 1;
                    test_column += 1;
                    if test_column % QUIET_MODE_MAX_COLUMN == QUIET_MODE_MAX_COLUMN - 1 {
                        print!(" {}/{}\n", test_count, total_count);
                        test_column = 0;
                    }
                }
            }
        } else {
            println!("test {}{} ... {}", name, mode, word);
        }
        let _ = crate::io::stdout().flush();
        match o {
            Outcome::Ok => passed += 1,
            Outcome::Ignored => ignored += 1,
            Outcome::Failed(m) => failures.push((String::from(name), m)),
        }
    };
    // libtest run_tests: up to `threads` tests at once, each on a thread named after it
    let (tx, rx) = mpsc::channel::<(usize, Outcome)>();
    let mut next = 0;
    let mut running = 0usize;
    let mut done = 0usize;
    let n = selected.len();
    let live = t.next != 0;
    let fault = t.fault;
    let reach = t.reach;
    let verbose = t.verbose != 0;
    let mut threads = Vec::new();
    let mut spawn_ms = 0.0;
    let mut report_ms = 0.0;
    let mut spawn = |i: usize, running: &mut usize| {
        let d = descs.clone();
        let thread_tx = tx.clone();
        let name = d[i].name.clone();
        let launched = verbose.then(crate::time::Instant::now);
        let h = crate::thread::Builder::new().name(name).spawn(move || {
            let run = || {
                if let Some(launched) = launched {
                    eprintln!("relax test timing: test={:?} thread_start_ms={:.3}", d[i].name, launched.elapsed().as_secs_f64() * 1000.0);
                }
                run_one(&d[i], fault, reach, verbose)
            };
            // A panic before recording must finish the run with unknown reach.
            let o = if live {
                match crate::panic::catch_boundary(crate::panic::AssertUnwindSafe(run)) {
                    Ok(o) => o,
                    Err(_) => Outcome::Failed(Some(String::from("test thread panicked outside the test body"))),
                }
            } else { run() };
            let _ = thread_tx.send((i, o));
        });
        if let Some(launched) = launched { spawn_ms += launched.elapsed().as_secs_f64() * 1000.0; }
        match h {
            Ok(h) => { if live { threads.push(h); } *running += 1; }
            Err(e) => {
                let _ = tx.send((i, Outcome::Failed(Some(format!("could not spawn test thread: {}", e)))));
                *running += 1;
            }
        }
    };
    while done < n {
        while running < opts.threads {
            if next == n { break; }
            let i = selected[next];
            next += 1;
            if descs[i].ignore {
                let _ = tx.send((i, Outcome::Ignored));
                running += 1;
                continue;
            }
            spawn(i, &mut running);
        }
        let Ok((i, o)) = rx.recv() else { break };
        running -= 1;
        done += 1;
        if live {
            unsafe { (*t.tests.add(descs[i].index)).result = match &o { Outcome::Ok => 1, Outcome::Failed(_) => 2, Outcome::Ignored => 3 }; }
        }
        let reporting = verbose.then(crate::time::Instant::now);
        report(&descs[i].name, descs[i].should_panic != 0, o, &mut failures);
        if let Some(reporting) = reporting { report_ms += reporting.elapsed().as_secs_f64() * 1000.0; }
    }
    // A live patch must not race the previous run's thread-local destructors.
    let joined = verbose.then(crate::time::Instant::now);
    let join_count = threads.len();
    for thread in threads { let _ = thread.join(); }
    if let Some(joined) = joined {
        eprintln!("relax test timing: tests={} spawn_ms={:.3} report_ms={:.3} joins={} join_ms={:.3} capture=false new_domains=0 isolates=0",
            selected.len() - ignored, spawn_ms, report_ms, join_count, joined.elapsed().as_secs_f64() * 1000.0);
    }
    // (terse: the line of characters ends with the next section's leading newline)
    let ok = failures.is_empty();
    if !ok {
        println!();
        println!("failures:");
        println!();
        for (name, m) in &failures {
            if let Some(m) = m {
                println!("---- {} stdout ----", name);
                println!("{}", m);
                println!();
            }
        }
        println!();
        println!("failures:");
        for (name, _) in &failures {
            println!("    {}", name);
        }
    }
    let secs = started.elapsed().as_secs_f64();
    println!();
    println!(
        "test result: {}. {} passed; {} failed; {} ignored; 0 measured; {} filtered out; finished in {:.2}s",
        if ok { "ok" } else { "FAILED" },
        passed,
        failures.len(),
        ignored,
        filtered_out,
        secs
    );
    println!();
    let _ = crate::io::stdout().flush();
    // a filter that ran no test is no pass (makepad_remote::hot::NO_TESTS_EXIT); under
    // `makepad test` (TEST_PACKAGE_ENV) it says so once for all its targets
    if ok && passed == 0 && !opts.filters.is_empty() {
        if crate::env::var("MAKEPAD_TEST_PACKAGE").is_err() {
            eprintln!("error: no test matched {}{}", opts.filters.join(" "), if ignored > 0 { format!(" ({} matched and ignored: --ignored runs them)", ignored) } else { String::new() });
        }
        return 87;
    }
    if ok { 0 } else { 101 }
}
