//! The test harness of `makepad test <package>`: what rustc's `--test` main does with libtest
//! (library/test: lib.rs run_test, test_result.rs calc_result, console.rs, cli.rs), running in
//! the program. The loader collects the crate's `#[test]` fns and hands their table to `main`;
//! the command line after `--` is libtest's (filters, --exact, --skip, --ignored,
//! --include-ignored, --test-threads, --list, --nocapture, -q).
//! Not here: output capture (every test prints as under --nocapture), benches, --format.

use crate::string::String;
use crate::vec::Vec;
use crate::sync::{mpsc, Arc, Mutex};
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
}

#[repr(C)]
pub struct Tests {
    pub tests: *const Test,
    pub len: usize,
    /// main's answer: the exit status (0, or 101 when a test failed)
    pub code: i32,
}

struct Desc {
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
    quiet: bool,
    threads: usize,
}

fn parse_args() -> Result<Opts, String> {
    let mut o = Opts { filters: Vec::new(), skip: Vec::new(), exact: false, ignored: false, include_ignored: false, list: false, quiet: false, threads: 0 };
    let mut args = crate::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--exact" => o.exact = true,
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
            _ if a.starts_with('-') => return Err(format!("Unrecognized option: '{}'", a.trim_start_matches('-'))),
            _ => o.filters.push(a),
        }
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

fn run_one(d: &Desc) -> Outcome {
    let (run, start) = (d.run, d.start);
    let r = crate::panic::catch_unwind(move || unsafe {
        if start == 0 {
            let f: fn() = crate::mem::transmute(run);
            f()
        } else {
            let f: fn(usize) = crate::mem::transmute(start);
            f(run)
        }
    });
    match r {
        Ok(()) => calc_result(d, None),
        Err(p) => calc_result(d, Some(&*p)),
    }
}

/// Runs the table's tests; the exit status goes to `t.code`.
pub fn main(t: &mut Tests) {
    t.code = run(t);
}

fn run(t: &Tests) -> i32 {
    let opts = match parse_args() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: {}", e);
            return 101;
        }
    };
    let mut all: Vec<Desc> = Vec::with_capacity(t.len);
    for i in 0..t.len {
        let x = unsafe { &*t.tests.add(i) };
        all.push(Desc {
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
    let filtered_out = total - all.len();
    all.sort_by(|a, b| a.name.cmp(&b.name));
    if opts.list {
        for d in &all {
            println!("{}: test", d.name);
        }
        if !opts.quiet {
            println!();
            println!("{} tests, 0 benchmarks", all.len());
        }
        return 0;
    }
    let started = crate::time::Instant::now();
    println!();
    println!("running {} test{}", all.len(), if all.len() == 1 { "" } else { "s" });
    let mut passed = 0usize;
    let mut ignored = 0usize;
    let mut failures: Vec<(String, Option<String>)> = Vec::new();
    // libtest's pretty formatter names the mode of a `#[should_panic]` test
    let mut report = |name: &str, should_panic: bool, o: Outcome, failures: &mut Vec<(String, Option<String>)>| {
        let mode = if should_panic { " - should panic" } else { "" };
        let word = match &o {
            Outcome::Ok => "ok",
            Outcome::Failed(_) => "FAILED",
            Outcome::Ignored => "ignored",
        };
        if opts.quiet {
            print!("{}", match &o { Outcome::Ok => ".", Outcome::Failed(_) => "F", Outcome::Ignored => "i" });
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
    let descs: Arc<Vec<Desc>> = Arc::new(all);
    let (tx, rx) = mpsc::channel::<(usize, Outcome)>();
    let next = Arc::new(Mutex::new(0usize));
    let mut running = 0usize;
    let mut done = 0usize;
    let n = descs.len();
    let spawn = |i: usize, running: &mut usize| {
        let d = descs.clone();
        let tx = tx.clone();
        let name = d[i].name.clone();
        let h = crate::thread::Builder::new().name(name).spawn(move || {
            let o = run_one(&d[i]);
            let _ = tx.send((i, o));
        });
        match h {
            Ok(_) => *running += 1,
            Err(_) => {
                let o = run_one(&descs[i]);
                let _ = tx.send((i, o));
                *running += 1;
            }
        }
    };
    while done < n {
        while running < opts.threads {
            let i = {
                let mut g = next.lock().unwrap();
                if *g >= n {
                    break;
                }
                // ignored tests are reported without running
                while *g < n && descs[*g].ignore {
                    let k = *g;
                    *g += 1;
                    let _ = tx.send((k, Outcome::Ignored));
                    running += 1;
                }
                if *g >= n {
                    break;
                }
                *g += 1;
                *g - 1
            };
            spawn(i, &mut running);
        }
        let Ok((i, o)) = rx.recv() else { break };
        running -= 1;
        done += 1;
        report(&descs[i].name, descs[i].should_panic != 0, o, &mut failures);
    }
    if opts.quiet {
        println!();
    }
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
    if ok { 0 } else { 101 }
}
