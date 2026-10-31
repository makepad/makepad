//! Pre-warm: after a sync moved an idle dev-on node's one tree to the new shared tip, the node
//! builds its one JIT and warms its one JIT cache for the smoke apps in the background, so the
//! first lease or dock launch after a landing starts warm.
//!
//! - Node side ([`start`], checkout builds): one background thread, at the lowest priority
//!   (`nice -n 19`; Windows: the lease job's below-normal class). For each smoke app, one after
//!   another and never two at once (they share the cache): `makepad check <package>`, then a short
//!   headless present (`makepad run --report-to …`, hidden windows, muted) that ends once the
//!   app reports `present`. The JIT is always `cargo run --profile host -p makepad-run --bin
//!   makepad -- …` in the tree (cargo builds it on the first step, a no-op after).
//! - A real lease preempts it at once ([`cancel`], from the lease start and from a newer sync):
//!   the running step's process group gets TERM (the JIT's quit handler commits its cache; its
//!   stores commit atomically, so even the KILL that follows 5 s later leaves the cache
//!   consistent), the thread is joined before the lease takes the tree.
//! - Every stage is a `prewarm` record on the node's stream; the master keeps the last one per
//!   node ([`Board`], `prewarm.state` in its state dir) for `devhub prewarm` and the panel.
//!
//! Only dev-on nodes that hold no lease and are under their load limits start one: the master
//! asks for it in the apply order (`prewarm=on`), the node checks its own switch and leases again.

use crate::proto::{self, Record};
use makepad_remote::json_str;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// The smoke apps: (registry id, package, dir from the tree's root, extra run args).
pub const SMOKE: &[(&str, &str, &str, &[&str])] = &[
    ("stage", "makepad-stage", "apps/commercial", &[]),
    ("scope", "makepad-scope", "apps/commercial", &[]),
    ("fab", "makepad-fab", "apps/commercial", &[]),
    ("sandbox", "makepad-sandbox", "apps/commercial", &[]),
    ("synth", "makepad-app-synth", ".", &[]),
    ("calculator", "makepad-app-calculator", ".", &["--no-default-features", "--features", "standalone"]),
];

/// The master's view: the last pre-warm record per node.
pub struct Board {
    /// the master's one state document (its `prewarm` section)
    store: std::sync::Arc<crate::state::Store>,
    last: Mutex<BTreeMap<String, (String, String, u64)>>,
}

impl Board {
    pub fn new(home: PathBuf) -> Board {
        let store = crate::state::Store::open_quiet(&home, "");
        let mut last = BTreeMap::new();
        for v in store.read(|d| d.prewarm.clone()) {
            let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
            if !s("node").is_empty() {
                last.insert(s("node"), (s("sev"), s("line"), v.get("at_ms").and_then(|x| x.as_u64()).unwrap_or(0)));
            }
        }
        Board { store, last: Mutex::new(last) }
    }

    /// Takes a node's `prewarm` record.
    pub fn see(&self, r: &Record) {
        if r.kind != "prewarm" {
            return;
        }
        let text = {
            let mut l = self.last.lock().unwrap();
            l.insert(r.machine.clone(), (r.sev.as_str().to_string(), r.line.clone(), r.at_ms));
            l.iter().filter_map(|(n, (s, line, at))| makepad_strict_json::parse(Self::json(n, s, line, *at).as_bytes()).ok()).collect::<Vec<_>>()
        };
        self.store.update(|d| d.prewarm = text);
    }

    fn json(node: &str, sev: &str, line: &str, at: u64) -> String {
        format!("{{\"node\":{},\"sev\":{},\"line\":{},\"at_ms\":{at}}}", json_str(node), json_str(sev), json_str(line))
    }

    /// The `prewarm` control answer: one line per node.
    pub fn serve(&self, out: &mut Vec<String>) {
        out.extend(self.last.lock().unwrap().iter().map(|(n, (s, l, at))| Self::json(n, s, l, *at)));
    }
}

/// The `prewarm` answer as (sev, line) rows (panel and CLI).
pub fn rows(lines: &[String]) -> Vec<(String, String)> {
    let now = proto::wall_ms();
    lines
        .iter()
        .filter_map(|l| {
            let v = makepad_strict_json::parse(l.as_bytes()).ok()?;
            let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
            let ago = now.saturating_sub(v.get("at_ms").and_then(|x| x.as_u64()).unwrap_or(0)) / 60_000;
            Some((s("sev"), format!("{:<8} {}  ({ago} min ago)", s("node"), s("line").trim_start_matches("prewarm: "))))
        })
        .collect()
}

#[cfg(test)]
mod board_tests {
    use super::*;
    use crate::proto::Sev;

    #[test]
    fn the_board_keeps_the_last_record_per_node_across_restarts() {
        let home = std::env::temp_dir().join(format!("devhub-board-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let b = Board::new(home.clone());
        let rec = |node: &str, kind: &str, line: &str| proto::hub_record(node, "", 0, kind, Sev::Info, line.into(), String::new());
        b.see(&rec("101", "prewarm", "prewarm: started at a/b"));
        b.see(&rec("101", "sync", "sync: not a prewarm"));
        b.see(&rec("101", "prewarm", "prewarm: done at a/b in 90 s"));
        b.see(&rec("107", "prewarm", "prewarm: cancelled after 3 s at a/b (lease L7 (x)): 1 of 12 steps done"));
        let b = Board::new(home.clone());
        let mut out = Vec::new();
        b.serve(&mut out);
        let r = rows(&out);
        assert_eq!(r.len(), 2);
        assert!(r[0].1.starts_with("101      done at a/b in 90 s"), "{r:?}");
        assert!(r[1].1.contains("cancelled") && r[1].1.contains("L7"));
        let _ = std::fs::remove_dir_all(&home);
    }
}

#[cfg(feature = "checkout")]
pub use node::*;

#[cfg(feature = "checkout")]
mod node {
    use super::SMOKE;
    use crate::proto::{self, hub_record, Sev};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// How long a present may take to report (the first after a landing compiles its crate).
    const PRESENT_WAIT: Duration = Duration::from_secs(600);
    /// TERM, then KILL after this.
    const TERM_GRACE: Duration = Duration::from_secs(5);

    /// One step: what it runs, in which dir, and (a present) the report file that ends it.
    #[derive(Clone, Debug, PartialEq)]
    pub struct Step {
        pub app: String,
        pub what: &'static str,
        /// cargo (at the lowest priority)
        pub program: PathBuf,
        pub dir: PathBuf,
        pub argv: Vec<String>,
        pub env: Vec<(String, String)>,
        pub report: Option<PathBuf>,
    }

    /// The JIT through cargo, in the tree: `cargo run --profile host -q --manifest-path
    /// <tree>/Cargo.toml -p makepad-run --bin makepad -- <args>`.
    fn jit(root: &Path, args: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = ["run", "--profile", "host", "-q", "--manifest-path"].iter().map(|s| s.to_string()).collect();
        v.push(root.join("Cargo.toml").display().to_string());
        v.extend(["-p", "makepad-run", "--bin", "makepad", "--"].iter().map(|s| s.to_string()));
        v.extend(args.iter().map(|s| s.to_string()));
        v
    }

    /// The pre-warm's steps for a tree: per smoke app a check, then a present (sequential).
    pub fn plan(root: &Path, scratch: &Path) -> Vec<Step> {
        let mut steps = Vec::new();
        let cargo = makepad_app_registry::cargo_bin();
        for (id, pkg, dir, extra) in SMOKE {
            let dir = root.join(dir);
            let mut env = makepad_app_registry::machine::launch_env(id, true);
            steps.push(Step { app: id.to_string(), what: "check", program: cargo.clone(), dir: dir.clone(), argv: jit(root, &["check", pkg]), env: env.clone(), report: None });
            let report = scratch.join(format!("{id}.report"));
            let mut args = vec!["run", "--report-to"];
            let rp = report.display().to_string();
            args.push(&rp);
            args.push(pkg);
            args.extend(extra.iter().copied());
            args.extend(["--", "--remote"]);
            env.extend([("MAKEPAD_HIDE_WINDOWS", "1"), ("SANDBOX_MUTE", "1")].iter().map(|(k, v)| (k.to_string(), v.to_string())));
            steps.push(Step { app: id.to_string(), what: "present", program: cargo.clone(), dir, argv: jit(root, &args), env, report: Some(report) });
        }
        steps
    }

    struct Run {
        cancel: Arc<AtomicBool>,
        why: Arc<Mutex<String>>,
        thread: std::thread::JoinHandle<()>,
    }

    /// The node's pre-warm slot: at most one runs.
    pub struct Prewarm {
        run: Mutex<Option<Run>>,
    }

    /// This node's ([`start`], [`cancel`], [`running`]).
    static NODE: Prewarm = Prewarm { run: Mutex::new(None) };

    /// Whether a pre-warm runs on this node now.
    pub fn running() -> bool {
        NODE.running()
    }

    /// Stops this node's running pre-warm (if any) and waits until its processes are gone: `why`
    /// goes into its record. True when one was running.
    pub fn cancel(why: &str) -> bool {
        NODE.cancel(why)
    }

    impl Prewarm {
        pub fn running(&self) -> bool {
            self.run.lock().unwrap().as_ref().is_some_and(|r| !r.thread.is_finished())
        }

        pub fn cancel(&self, why: &str) -> bool {
            let Some(r) = self.run.lock().unwrap().take() else { return false };
            let was = !r.thread.is_finished();
            *r.why.lock().unwrap() = why.to_string();
            r.cancel.store(true, Ordering::Release);
            let _ = r.thread.join();
            was
        }
    }

    /// The program a step runs: cargo at the lowest priority.
    fn command(step: &Step) -> Command {
        #[cfg(unix)]
        let mut cmd = {
            let mut c = Command::new("nice");
            c.arg("-n").arg("19").arg(&step.program);
            c
        };
        #[cfg(not(unix))]
        let mut cmd = Command::new(&step.program);
        cmd.args(&step.argv).current_dir(&step.dir).stdin(Stdio::null());
        makepad_app_registry::cargo_env(&mut cmd);
        for k in ["CARGO_TARGET_DIR", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTC_WRAPPER", "CARGO_INCREMENTAL"] {
            cmd.env_remove(k);
        }
        cmd.env("CARGO_INCREMENTAL", "0");
        for (k, v) in &step.env {
            cmd.env(k, v);
        }
        cmd
    }

    /// Runs one step in its own process group, logging to `log`; Ok(true) when it did its job,
    /// Err on cancel or failure.
    fn run_step(step: &Step, log: &Path, cancel: &AtomicBool) -> Result<(), String> {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(log).map_err(|e| format!("{}: {e}", log.display()))?;
        let _ = writeln!(file, "== {} {}: {}", step.app, step.what, step.argv.join(" "));
        if let Some(r) = &step.report {
            let _ = std::fs::remove_file(r);
        }
        let mut cmd = command(step);
        cmd.stdout(file.try_clone().map_err(|e| e.to_string())?).stderr(file.try_clone().map_err(|e| e.to_string())?);
        if cancel.load(Ordering::Acquire) {
            return Err("cancelled".into());
        }
        let (mut child, hold) = crate::os::lease_spawn(&mut cmd).map_err(|e| format!("{} {}: {e}", step.app, step.what))?;
        let pid = child.id();
        let t0 = Instant::now();
        let stop = |child: &mut std::process::Child| {
            crate::os::signal_group(pid, false, &hold);
            let end = Instant::now() + TERM_GRACE;
            loop {
                if let Ok(Some(_)) = child.try_wait() {
                    break;
                }
                if Instant::now() > end {
                    crate::os::signal_group(pid, true, &hold);
                    let _ = child.wait();
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            // the group's other members (a cargo-run JIT child) get the KILL too once the leader is gone
            crate::os::signal_group(pid, true, &hold);
        };
        loop {
            if cancel.load(Ordering::Acquire) {
                stop(&mut child);
                return Err("cancelled".into());
            }
            if let Some(r) = &step.report {
                if std::fs::read_to_string(r).is_ok_and(|t| t.lines().any(|l| l.trim() == "present")) {
                    stop(&mut child);
                    return Ok(());
                }
                if t0.elapsed() > PRESENT_WAIT {
                    stop(&mut child);
                    return Err(format!("{} did not present in {} s", step.app, PRESENT_WAIT.as_secs()));
                }
            }
            match child.try_wait() {
                Ok(Some(st)) => {
                    // the group ends with its leader
                    crate::os::signal_group(pid, true, &hold);
                    let present = step.report.as_ref().is_some_and(|r| std::fs::read_to_string(r).is_ok_and(|t| t.lines().any(|l| l.trim() == "present")));
                    return if (st.success() && step.report.is_none()) || present { Ok(()) } else { Err(format!("{} {} exited ({st}) (log {})", step.app, step.what, log.display())) };
                }
                Ok(None) => {}
                Err(e) => return Err(e.to_string()),
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    impl Prewarm {
        /// Starts the pre-warm of `root` (at `at`, for the records) in the background, replacing a
        /// running one. `run` executes one step (tests pass a fake).
        pub fn start_with(&self, root: PathBuf, at: String, publish: crate::local::Publish, machine: String, run: Arc<dyn Fn(&Step, &Path, &AtomicBool) -> Result<(), String> + Send + Sync>) {
            self.cancel("a newer pre-warm");
            let cancel_flag = Arc::new(AtomicBool::new(false));
            let why = Arc::new(Mutex::new(String::new()));
            let (c2, w2) = (cancel_flag.clone(), why.clone());
            let scratch = proto::home().join("prewarm");
            let _ = std::fs::create_dir_all(&scratch);
            let logs = super::super::build::logs_dir();
            let _ = std::fs::create_dir_all(&logs);
            let log = logs.join(format!("prewarm-{}.log", proto::wall_ms()));
            let thread = std::thread::Builder::new().name("devhub-prewarm".into()).spawn(move || {
                crate::os::nice_this_thread();
                let ev = |sev: Sev, line: String| publish(hub_record(&machine, "", 0, "prewarm", sev, line, format!("{{\"log\":{}}}", makepad_remote::json_str(&log.display().to_string()))));
                let steps = plan(&root, &scratch);
                let t0 = Instant::now();
                ev(Sev::Info, format!("prewarm: started at {at}: build the JIT, check + present {} apps", SMOKE.len()));
                let mut done = Vec::new();
                let mut failed = Vec::new();
                for s in &steps {
                    if c2.load(Ordering::Acquire) {
                        break;
                    }
                    match run(s, &log, &c2) {
                        Ok(()) => done.push(format!("{} {}", s.app, s.what)),
                        Err(_) if c2.load(Ordering::Acquire) => break,
                        Err(e) => {
                            ev(Sev::Warn, format!("prewarm: {} {} failed at {at}: {}", s.app, s.what, proto::clean(&e, 200)));
                            failed.push(format!("{} {}", s.app, s.what));
                        }
                    }
                }
                let secs = t0.elapsed().as_secs();
                if c2.load(Ordering::Acquire) {
                    ev(Sev::Info, format!("prewarm: cancelled after {secs} s at {at} ({}): {} of {} steps done", w2.lock().unwrap(), done.len(), steps.len()));
                } else if failed.is_empty() {
                    ev(Sev::Info, format!("prewarm: done at {at} in {secs} s: {} apps checked and presented", SMOKE.len()));
                } else {
                    ev(Sev::Warn, format!("prewarm: done at {at} in {secs} s, {} failed: {}", failed.len(), failed.join(", ")));
                }
            });
            match thread {
                Ok(thread) => *self.run.lock().unwrap() = Some(Run { cancel: cancel_flag, why, thread }),
                Err(e) => eprintln!("devhub prewarm: cannot start: {e}"),
            }
        }
    }

    /// `devhub prewarm run [--cancel-after <s>]`: the pre-warm of the tree at `root` in the
    /// foreground (no hub), its records on stdout; `cancel_after`: cancelled then, the way a lease
    /// preempts it. 0 when it finished (or was cancelled as asked) with no failed step.
    pub fn foreground(root: PathBuf, cancel_after: Option<u64>) -> i32 {
        let pw = Prewarm { run: Mutex::new(None) };
        let failed = Arc::new(AtomicBool::new(false));
        let f2 = failed.clone();
        let t0 = Instant::now();
        let at = crate::sync::git::run(Some(&root), None, &[], &["rev-parse", "--short=10", "HEAD"]).map_or_else(|_| "-".to_string(), |s| s.trim().to_string());
        pw.start_with(root, at, Arc::new(move |r: crate::proto::Record| {
            if r.sev != Sev::Info {
                f2.store(true, Ordering::Release);
            }
            println!("[{:>4} s] {}", t0.elapsed().as_secs(), r.line);
        }), proto::default_machine_name(), Arc::new(run_step));
        if let Some(s) = cancel_after {
            let end = t0 + Duration::from_secs(s);
            while pw.running() && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(100));
            }
            if pw.running() {
                let c = Instant::now();
                pw.cancel("dry-run preemption");
                println!("[{:>4} s] cancel returned in {} ms (every step process gone)", t0.elapsed().as_secs(), c.elapsed().as_millis());
            }
        }
        while pw.running() {
            std::thread::sleep(Duration::from_millis(200));
        }
        // joins the finished thread (its last record printed)
        pw.cancel("done");
        if failed.load(Ordering::Acquire) { 1 } else { 0 }
    }

    /// This node's pre-warm with the real steps.
    pub fn start(root: PathBuf, at: String, publish: crate::local::Publish, machine: String) {
        NODE.start_with(root, at, publish, machine, Arc::new(run_step));
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::atomic::AtomicUsize;

        fn wait_for(f: impl Fn() -> bool) {
            let end = Instant::now() + Duration::from_secs(10);
            while !f() {
                assert!(Instant::now() < end, "timed out");
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        #[test]
        fn the_plan_is_check_then_present_per_app_through_cargo_run() {
            let steps = plan(Path::new("/t"), Path::new("/s"));
            assert_eq!(steps.len(), 2 * SMOKE.len());
            assert_eq!(steps.iter().map(|s| s.app.as_str()).collect::<Vec<_>>().chunks(2).map(|c| c[0]).collect::<Vec<_>>(), vec!["stage", "scope", "fab", "sandbox", "synth", "calculator"]);
            let head = ["run", "--profile", "host", "-q", "--manifest-path", "/t/Cargo.toml", "-p", "makepad-run", "--bin", "makepad", "--"];
            for (i, s) in steps.iter().enumerate() {
                assert_eq!(&s.argv[..head.len()], head);
                assert_eq!(s.what, if i % 2 == 0 { "check" } else { "present" });
                assert_eq!(s.report.is_some(), s.what == "present");
            }
            assert_eq!(steps[0].argv[head.len()..], ["check", "makepad-stage"]);
            assert_eq!(steps[0].dir, Path::new("/t/apps/commercial"));
            let calc = steps.last().unwrap();
            assert_eq!(calc.argv[head.len()..], ["run", "--report-to", "/s/calculator.report", "makepad-app-calculator", "--no-default-features", "--features", "standalone", "--", "--remote"]);
            assert!(calc.env.contains(&("MAKEPAD_HIDE_WINDOWS".into(), "1".into())) && calc.env.contains(&("SANDBOX_MUTE".into(), "1".into())));
        }

        /// A lease preempts at once: the running step sees the cancel, the thread is joined before
        /// `cancel` returns, no later step starts, and the record says why. Steps never overlap.
        #[test]
        fn a_lease_preempts_the_prewarm() {
            let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
            let ev = events.clone();
            let started = Arc::new(AtomicUsize::new(0));
            let live = Arc::new(AtomicUsize::new(0));
            let (st, lv) = (started.clone(), live.clone());
            let run: Arc<dyn Fn(&Step, &Path, &AtomicBool) -> Result<(), String> + Send + Sync> = Arc::new(move |s: &Step, _: &Path, cancel: &AtomicBool| {
                assert_eq!(lv.fetch_add(1, Ordering::SeqCst), 0, "two steps at once");
                st.fetch_add(1, Ordering::SeqCst);
                let r = if s.what == "present" && s.app == "scope" {
                    // a long present: runs until cancelled
                    while !cancel.load(Ordering::Acquire) {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err("cancelled".to_string())
                } else {
                    Ok(())
                };
                lv.fetch_sub(1, Ordering::SeqCst);
                r
            });
            let pw = Prewarm { run: Mutex::new(None) };
            pw.start_with(PathBuf::from("/t"), "abc/def".into(), Arc::new(move |r: crate::proto::Record| ev.lock().unwrap().push(r.line)), "n1".into(), run);
            // stage check+present, scope check, then the scope present blocks
            wait_for(|| started.load(Ordering::SeqCst) == 4);
            assert!(pw.running());
            assert!(pw.cancel("lease L7"));
            // joined: nothing runs, nothing more starts
            assert_eq!(live.load(Ordering::SeqCst), 0);
            assert!(!pw.running());
            std::thread::sleep(Duration::from_millis(30));
            assert_eq!(started.load(Ordering::SeqCst), 4);
            let ev = events.lock().unwrap().clone();
            assert!(ev[0].starts_with("prewarm: started at abc/def"), "{ev:?}");
            assert!(ev.last().unwrap().contains("cancelled") && ev.last().unwrap().contains("lease L7") && ev.last().unwrap().contains("3 of 12 steps"), "{ev:?}");
            // a cancel with nothing running is a no-op
            assert!(!pw.cancel("lease L8"));
        }

        /// The real step runner: a cancel ends a running step's process group at once; a present
        /// ends when its report says `present`.
        #[cfg(unix)]
        #[test]
        fn the_step_runner_stops_on_cancel_and_on_present() {
            let dir = std::env::temp_dir().join(format!("devhub-prewarm-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let log = dir.join("log");
            let step = |program: &str, argv: &[&str], report: Option<PathBuf>| Step { app: "x".into(), what: "present", program: program.into(), dir: dir.clone(), argv: argv.iter().map(|s| s.to_string()).collect(), env: Vec::new(), report };
            // a present that never reports, cancelled after 200 ms
            let flag = Arc::new(AtomicBool::new(false));
            let f2 = flag.clone();
            let canceller = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                f2.store(true, Ordering::Release);
            });
            let t0 = Instant::now();
            let r = run_step(&step("sleep", &["30"], Some(dir.join("never.report"))), &log, &flag);
            assert_eq!(r, Err("cancelled".to_string()));
            assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
            canceller.join().unwrap();
            // a present whose report says present: Ok (cp writes the report and exits)
            let src = dir.join("src");
            std::fs::write(&src, "startup\npresent\n").unwrap();
            let rp = dir.join("ok.report");
            let r = run_step(&step("cp", &[src.to_str().unwrap(), rp.to_str().unwrap()], Some(rp.clone())), &log, &AtomicBool::new(false));
            assert_eq!(r, Ok(()));
            // a failing check: Err with the log named
            let mut s = step("false", &[], None);
            s.what = "check";
            assert!(run_step(&s, &log, &AtomicBool::new(false)).unwrap_err().contains("exited"));
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
