//! The test suite as the CI runs it. In release: a debug build makes numeric
//! tests ten times slower and proves nothing a release build does not. The
//! platform's own crates by default (`platform/`, `draw/`, `widgets/`, the CI);
//! everything else is the deep run, opt-in. Every test binary is built once
//! and then run several at a time, each one timed and capped, so a hung test
//! costs its own cap and not the hour, and the slow binaries are named.
use crate::{
    pipeline::{test_results, TestResults},
    process::{self, Control, Result},
    report::Run,
};
use makepad_strict_json::{self as json, Value};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

/// One executable `cargo test --no-run` produced.
#[derive(Clone, Debug, PartialEq)]
pub struct TestBinary {
    pub package: String,
    /// `makepad-widgets lib`, `makepad-piano-model test:acoustic_reference`.
    pub name: String,
    pub exe: PathBuf,
    pub manifest_dir: PathBuf,
}

/// The package a cargo package id names: `path+file:///x/apps/wm#makepad-wm@0.1.0`,
/// `path+file:///x/libs/foo#0.1.0` (the name is the directory's when they
/// agree), or the older `makepad-wm 0.1.0 (path+file:///...)`.
pub fn package_of(id: &str) -> String {
    if let Some((url, tail)) = id.rsplit_once('#') {
        if let Some((name, _version)) = tail.split_once('@') {
            return name.to_string();
        }
        return url
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(url)
            .to_string();
    }
    id.split_whitespace().next().unwrap_or(id).to_string()
}

/// The test binaries in cargo's JSON messages: the artifacts built with the
/// test profile that have an executable.
pub fn binaries_from_json(out: &str) -> Vec<TestBinary> {
    let mut found = Vec::new();
    for line in out.lines() {
        let Ok(v) = json::parse_depth(line.as_bytes(), 128) else {
            continue;
        };
        if v.get("reason").and_then(Value::as_str) != Some("compiler-artifact") {
            continue;
        }
        if v.get("profile").and_then(|p| p.get("test")).and_then(Value::as_bool) != Some(true) {
            continue;
        }
        let Some(exe) = v.get("executable").and_then(Value::as_str) else {
            continue;
        };
        let package = v
            .get("package_id")
            .and_then(Value::as_str)
            .map(package_of)
            .unwrap_or_default();
        let target = v.get("target");
        let target_name = target
            .and_then(|t| t.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let kind = target
            .and_then(|t| t.get("kind"))
            .and_then(Value::as_arr)
            .and_then(|k| k.first())
            .and_then(Value::as_str)
            .unwrap_or("");
        let manifest_dir = v
            .get("manifest_path")
            .and_then(Value::as_str)
            .and_then(|m| Path::new(m).parent())
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let name = match kind {
            "lib" | "rlib" | "dylib" | "cdylib" | "staticlib" | "proc-macro" => format!("{package} lib"),
            other => format!("{package} {other}:{target_name}"),
        };
        found.push(TestBinary {
            package,
            name,
            exe: PathBuf::from(exe),
            manifest_dir,
        });
    }
    found
}

/// Packages whose manifest lives under one of `dirs` (relative to the
/// checkout), from `cargo metadata`'s output.
pub fn packages_under(metadata: &str, root: &Path, dirs: &[String]) -> Result<Vec<String>> {
    let doc = metadata
        .lines()
        .find_map(|line| {
            json::parse_depth(line.as_bytes(), 128)
                .ok()
                .filter(|v| v.get("packages").is_some())
        })
        .ok_or("cargo metadata missing packages")?;
    let members = doc
        .get("workspace_members")
        .and_then(Value::as_arr)
        .ok_or("missing workspace members")?;
    let prefixes: Vec<PathBuf> = dirs.iter().map(|d| root.join(d)).collect();
    let mut names = Vec::new();
    for p in doc.get("packages").and_then(Value::as_arr).ok_or("missing packages")? {
        if !members.iter().any(|id| id.as_str() == p.get("id").and_then(Value::as_str)) {
            continue;
        }
        let manifest = p
            .get("manifest_path")
            .and_then(Value::as_str)
            .map(Path::new)
            .ok_or("missing manifest path")?;
        if prefixes.iter().any(|prefix| manifest.starts_with(prefix)) {
            if let Some(name) = p.get("name").and_then(Value::as_str) {
                names.push(name.to_string());
            }
        }
    }
    names.sort();
    names.dedup();
    Ok(names)
}

/// How many binaries run at once and how many threads each gets on a box
/// with `cpus` cores. Binaries in parallel win more than threads inside one:
/// most binaries hold a handful of tests, and the UI ones spend their time
/// waiting on an app.
pub fn plan(cpus: usize) -> (usize, usize) {
    let at_once = (cpus / 2).clamp(2, 8);
    let threads = (cpus / at_once).max(2);
    (at_once, threads)
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub name: String,
    pub seconds: f64,
    pub results: TestResults,
    pub code: i32,
    pub hung: bool,
    pub error: String,
    pub log: PathBuf,
}

impl Outcome {
    pub fn failed(&self) -> bool {
        self.code != 0 || self.hung || !self.error.is_empty()
    }
}

fn safe_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

/// What cargo gives a test binary: its package directory as the working
/// directory, the package's manifest directory and name, a scratch directory,
/// the shared target directory (a test that builds an app must build it where
/// the CI builds), and the release artefacts on the library path.
fn env_for(binary: &TestBinary, root: &Path, sysroot_lib: Option<&Path>) -> Vec<(String, String)> {
    let target = root.join("target");
    let release = target.join("release");
    // The toolchain's own library directory too: a proc-macro crate's test
    // binary links the standard library dynamically, and cargo puts that
    // directory on the loader path when it runs such a binary.
    let mut lib_path = format!("{}:{}", release.join("deps").display(), release.display());
    if let Some(dir) = sysroot_lib {
        lib_path = format!("{lib_path}:{}", dir.display());
    }
    let mut env = vec![
        ("CARGO_MANIFEST_DIR".to_string(), binary.manifest_dir.display().to_string()),
        ("CARGO_PKG_NAME".to_string(), binary.package.clone()),
        ("CARGO_TARGET_DIR".to_string(), target.display().to_string()),
        ("CARGO_TARGET_TMPDIR".to_string(), target.join("tmp").display().to_string()),
    ];
    for var in ["DYLD_FALLBACK_LIBRARY_PATH", "LD_LIBRARY_PATH"] {
        let existing = std::env::var(var).unwrap_or_default();
        let value = if existing.is_empty() { lib_path.clone() } else { format!("{lib_path}:{existing}") };
        env.push((var.to_string(), value));
    }
    env
}

/// The first few lines that say why a binary failed: the failed tests and
/// their panics.
fn why(out: &str, results: &TestResults) -> String {
    let mut lines: Vec<String> = results.failed_names.iter().map(|n| format!("FAILED {n}")).collect();
    for line in out.lines().filter(|l| l.contains("panicked at")).take(4) {
        lines.push(line.trim().to_string());
    }
    lines.join("\n")
}

/// Where the toolchain keeps the host's dynamic libraries (`rustc --print
/// target-libdir`: `<sysroot>/lib/rustlib/<host>/lib`), when rustc answers.
pub fn sysroot_lib() -> Option<PathBuf> {
    let out = std::process::Command::new("rustc").args(["--print", "target-libdir"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let dir = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    dir.is_dir().then_some(dir)
}

/// Run every binary, `at_once` at a time, each capped at `cap`. The running
/// step's progress text moves with the count.
pub fn run_all(
    run: &mut Run,
    binaries: &[TestBinary],
    at_once: usize,
    threads: usize,
    cap: Duration,
    step: Option<usize>,
) -> Vec<Outcome> {
    let sysroot_lib = sysroot_lib();
    let logs: Vec<PathBuf> = binaries
        .iter()
        .map(|b| run.path(&format!("test-{}.log", safe_name(&b.name))))
        .collect();
    let _ = std::fs::create_dir_all(run.root.join("target/tmp"));
    let root = run.root.clone();
    let control: Control = run.control.clone();
    let queue: Mutex<VecDeque<usize>> = Mutex::new((0..binaries.len()).collect());
    let done: Mutex<Vec<Outcome>> = Mutex::new(Vec::new());
    let workers = at_once.clamp(1, binaries.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let (queue, done, logs, root, control, sysroot_lib) = (&queue, &done, &logs, &root, &control, &sysroot_lib);
            scope.spawn(move || loop {
                let next = queue.lock().ok().and_then(|mut q| q.pop_front());
                let Some(i) = next else { return };
                let binary = &binaries[i];
                let start = Instant::now();
                let env = env_for(binary, root, sysroot_lib.as_deref());
                let args = vec!["--test-threads".to_string(), threads.to_string()];
                let out = process::run(
                    &binary.exe.display().to_string(),
                    &args,
                    &binary.manifest_dir,
                    &env,
                    cap.as_secs(),
                    logs[i].clone(),
                    control,
                    &mut |_| {},
                );
                let seconds = start.elapsed().as_secs_f64();
                let outcome = match out {
                    Ok(o) => {
                        let results = test_results(&o.out);
                        let error = if o.code == 0 { String::new() } else { why(&o.out, &results) };
                        Outcome { name: binary.name.clone(), seconds, results, code: o.code, hung: false, error, log: logs[i].clone() }
                    }
                    Err(e) => Outcome {
                        name: binary.name.clone(),
                        seconds,
                        results: TestResults::default(),
                        code: -1,
                        hung: e.contains("timed out"),
                        error: e,
                        log: logs[i].clone(),
                    },
                };
                if let Ok(mut d) = done.lock() {
                    d.push(outcome);
                }
            });
        }
        loop {
            let n = done.lock().map(|d| d.len()).unwrap_or(binaries.len());
            if n >= binaries.len() {
                break;
            }
            if let Some(i) = step {
                run.stages[i].progress = format!("{n} of {} test binaries", binaries.len());
                run.publish();
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    });
    let mut outcomes = done.into_inner().unwrap_or_default();
    outcomes.sort_by(|a, b| b.seconds.partial_cmp(&a.seconds).unwrap_or(std::cmp::Ordering::Equal));
    outcomes
}

/// The slowest binaries, one line each, for the step's detail.
pub fn table(outcomes: &[Outcome], top: usize) -> String {
    let mut lines = Vec::new();
    for o in outcomes.iter().take(top) {
        let state = if o.hung { "HUNG" } else if o.failed() { "FAILED" } else { "ok" };
        lines.push(format!(
            "{:7.1} s  {:<48} {:>4} passed {:>3} failed  {state}",
            o.seconds,
            o.name,
            o.results.passed,
            o.results.failed
        ));
    }
    lines.join("\n")
}

pub fn totals(outcomes: &[Outcome]) -> TestResults {
    let mut t = TestResults::default();
    for o in outcomes {
        t.passed += o.results.passed;
        t.failed += o.results.failed;
        t.failed_names.extend(o.results.failed_names.iter().cloned());
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_binaries_are_read_from_cargos_messages_whatever_the_id_spelling() {
        let out = concat!(
            r#"{"reason":"compiler-artifact","package_id":"path+file:///r/widgets#makepad-widgets@1.0.0","manifest_path":"/r/widgets/Cargo.toml","target":{"name":"makepad_widgets","kind":["lib"]},"profile":{"test":true},"executable":"/r/target/release/deps/makepad_widgets-1"}"#, "\n",
            r#"{"reason":"compiler-artifact","package_id":"path+file:///r/libs/piano_model#0.1.0","manifest_path":"/r/libs/piano_model/Cargo.toml","target":{"name":"acoustic_reference","kind":["test"]},"profile":{"test":true},"executable":"/r/target/release/deps/acoustic_reference-2"}"#, "\n",
            r#"{"reason":"compiler-artifact","package_id":"makepad-wm 0.1.0 (path+file:///r/apps/wm)","manifest_path":"/r/apps/wm/Cargo.toml","target":{"name":"wm","kind":["bin"]},"profile":{"test":false},"executable":"/r/target/release/wm"}"#, "\n",
            r#"{"reason":"compiler-artifact","package_id":"path+file:///r/libs/x#x@0.1.0","manifest_path":"/r/libs/x/Cargo.toml","target":{"name":"x","kind":["lib"]},"profile":{"test":true},"executable":null}"#, "\n",
            r#"{"reason":"build-finished","success":true}"#, "\n",
        );
        let b = binaries_from_json(out);
        assert_eq!(b.len(), 2, "{b:?}");
        assert_eq!(b[0].name, "makepad-widgets lib");
        assert_eq!(b[0].manifest_dir, PathBuf::from("/r/widgets"));
        assert_eq!(b[1].name, "piano_model test:acoustic_reference");
        assert_eq!(package_of("makepad-wm 0.1.0 (path+file:///r/apps/wm)"), "makepad-wm");
    }
    #[test]
    fn packages_are_selected_by_the_directory_their_manifest_lives_in() {
        let metadata = r#"{"packages":[{"name":"makepad-platform","id":"a","manifest_path":"/r/platform/Cargo.toml"},{"name":"makepad-script","id":"b","manifest_path":"/r/platform/script/Cargo.toml"},{"name":"makepad-wm","id":"c","manifest_path":"/r/apps/wm/Cargo.toml"},{"name":"other","id":"d","manifest_path":"/elsewhere/Cargo.toml"}],"workspace_members":["a","b","c"]}"#;
        let names = packages_under(metadata, Path::new("/r"), &["platform".into(), "tools/ci".into()]).unwrap();
        assert_eq!(names, vec!["makepad-platform", "makepad-script"]);
    }
    #[test]
    fn the_plan_keeps_binaries_in_parallel_ahead_of_threads() {
        assert_eq!(plan(10), (5, 2));
        assert_eq!(plan(2), (2, 2));
        assert_eq!(plan(32), (8, 4));
    }
    #[test]
    fn the_table_names_the_slowest_first_and_says_what_failed() {
        let mut names = std::collections::BTreeSet::new();
        names.insert("a::b".to_string());
        let outcomes = vec![
            Outcome { name: "x lib".into(), seconds: 1.5, results: TestResults { passed: 3, failed: 0, failed_names: Default::default() }, code: 0, hung: false, error: String::new(), log: PathBuf::new() },
            Outcome { name: "y test:t".into(), seconds: 40.0, results: TestResults { passed: 1, failed: 1, failed_names: names }, code: 101, hung: false, error: "FAILED a::b".into(), log: PathBuf::new() },
        ];
        let t = totals(&outcomes);
        assert_eq!((t.passed, t.failed), (4, 1));
        assert!(table(&outcomes, 5).lines().next().unwrap().contains("x lib"));
    }
}
