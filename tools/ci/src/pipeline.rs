use crate::{
    process::{Control, Result},
    report::{self, strings, Branch, Notify, Run, Update},
    smoke::{self, Script},
    uihub,
    watch::{self, Config, Poll, Queue},
};
use makepad_strict_json::{self as json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{atomic::Ordering, mpsc},
    time::Duration,
};

#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct TestResults {
    pub passed: u64,
    pub failed: u64,
    pub failed_names: BTreeSet<String>,
}
pub fn test_results(out: &str) -> TestResults {
    let mut r = TestResults::default();
    let mut names = false;
    for line in out.lines() {
        let l = line.trim();
        if l.starts_with("test result:") {
            for segment in l.split(';') {
                let words: Vec<_> = segment.split_whitespace().collect();
                for pair in words.windows(2) {
                    if let Ok(n) = pair[0].parse::<u64>() {
                        match pair[1] {
                            "passed" => r.passed += n,
                            "failed" => r.failed += n,
                            _ => {}
                        }
                    }
                }
            }
            names = false;
        }
        if let Some(test) = l
            .strip_prefix("test ")
            .and_then(|s| s.strip_suffix(" ... FAILED"))
        {
            r.failed_names.insert(test.into());
        }
        if l == "failures:" {
            names = true;
            continue;
        }
        if names && !l.is_empty() && !l.contains(' ') && !l.starts_with('-') {
            r.failed_names.insert(l.into());
        }
    }
    r
}
pub fn warning_counts(out: &str) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();
    for line in out.lines() {
        let Ok(v) = json::parse_depth(line.as_bytes(), 128) else {
            continue;
        };
        if v.get("reason").and_then(Value::as_str) != Some("compiler-message") {
            continue;
        }
        let Some(message) = v.get("message") else {
            continue;
        };
        if message.get("level").and_then(Value::as_str) != Some("warning") {
            continue;
        }
        let package = v.get("package_id").and_then(Value::as_str).unwrap_or("");
        // Modern IDs encode the package in #name@version. If the name is
        // omitted (#version), it is the URL's final path segment. Legacy
        // Cargo IDs start with `name version (...)`. Never use target.name:
        // binaries and dependency library targets need not match a package.
        let name = crate::cargo::package_name(package).to_string();
        *counts.entry(name).or_insert(0) += 1;
    }
    counts
}
fn sync(run: &mut Run, base: &Path, config: &Config, i: usize) -> Result<()> {
    run.begin(i, "git clone / fetch / checkout --detach");
    let url = watch::remote_url(base, config, &run.control)?;
    let checkout = &run.root.clone();
    if checkout.exists() {
        let absolute = checkout.canonicalize().map_err(|e| e.to_string())?;
        if absolute == base.canonicalize().map_err(|e| e.to_string())? {
            return Err("refusing to sync the developer checkout".into());
        }
        if !checkout.join(".git/ci-owner").is_file() {
            return Err(format!("{} is not a CI-owned checkout (missing .git/ci-owner); choose an empty checkout path",checkout.display()));
        }
        let owner =
            fs::read_to_string(checkout.join(".git/ci-owner")).map_err(|e| e.to_string())?;
        if owner != base.display().to_string() {
            return Err("checkout belongs to a different CI root".into());
        }
    } else {
        let parent = checkout.parent().ok_or("checkout has no parent")?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let args = vec![
            "clone".into(),
            "--no-checkout".into(),
            "--origin".into(),
            config.remote.clone(),
            "--".into(),
            url,
            checkout.display().to_string(),
        ];
        let out = run.command("git", &args, base, &[], 600)?;
        if out.code != 0 {
            return Err(crate::process::error_lines(&out.out));
        }
        fs::write(checkout.join(".git/ci-owner"), base.display().to_string())
            .map_err(|e| e.to_string())?;
    }
    for args in [
        vec!["fetch".into(), config.remote.clone(), run.branch.clone()],
        vec!["checkout".into(), "--detach".into(), run.tip.clone()],
    ] {
        run.stages[i].command = report::display_command("git", &args);
        run.publish();
        let out = run.command("git", &args, checkout, &[], 600)?;
        if out.code != 0 {
            return Err(crate::process::error_lines(&out.out));
        }
    }
    for name in ["stage", "scope"] {
        let dir = checkout.join("apps").join(name);
        if !dir.join(".git").exists() {
            run.log(&format!("apps/{name}: not present"));
            continue;
        }
        let out = run.command("git", &strings(&["pull", "--ff-only"]), &dir, &[], 300)?;
        if out.code != 0 {
            return Err(format!(
                "apps/{name}: {}",
                crate::process::error_lines(&out.out)
            ));
        }
        let out = run.command("git", &strings(&["rev-parse", "HEAD"]), &dir, &[], 30)?;
        if out.code != 0 {
            return Err(format!("apps/{name}: could not record tip"));
        }
        run.evidence.push(json::obj(vec![
            ("repository", json::s(format!("apps/{name}"))),
            ("tip", json::s(out.out.trim())),
        ]));
    }
    Ok(())
}
pub fn run_once(
    base: &Path,
    config: &Config,
    branch: &str,
    tip: &str,
    control: Control,
    notify: Notify,
) -> Result<Branch> {
    let mut state = watch::load_state(base, config)?;
    let previous = state
        .get(branch)
        .map(|b| b.scripts.clone())
        .unwrap_or_default();
    // Only script workers touch this recorder. The UI consumes notifications
    // without sharing its lock, and failures survive an interrupted CI process.
    let persisted = std::sync::Arc::new(std::sync::Mutex::new(state.clone()));
    let save_error = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let records = persisted.clone();
    let errors = save_error.clone();
    let directory = base.to_path_buf();
    let under_test = tip.to_string();
    let live_notify: Notify = std::sync::Arc::new(move |update| {
        if let Ok(mut state) = records.lock() {
            let mut changed = false;
            let scope = match &update {
                Update::Scripts(branch, _) | Update::Script(branch, _, _) => Some(branch),
                _ => None,
            };
            if let Some(branch) = scope {
                let b = state.entry(branch.clone()).or_insert_with(|| Branch {
                    name: branch.clone(),
                    tip: String::new(),
                    verdict: "waiting".into(),
                    finished: 0,
                    detail: String::new(),
                    scripts: Vec::new(),
                });
                // The header names the tip being tested, not the last one tested.
                b.tip = under_test.clone();
                match &update {
                    Update::Scripts(_, scripts) => {
                        report::merge_scripts(&mut b.scripts, scripts);
                        changed = true;
                    }
                    Update::Script(_, name, message) => {
                        if !b.scripts.iter().any(|s| &s.name == name) {
                            b.scripts.push(report::ScriptState::waiting(name));
                        }
                        let s = b.scripts.iter_mut().find(|s| &s.name == name).unwrap();
                        changed = report::script_update(s, message);
                    }
                    _ => {}
                }
                b.verdict = if b.scripts.iter().any(|s| s.color_verdict() == "red") {
                    "red"
                } else {
                    "running"
                }
                .into();
            }
            if changed {
                if let Err(e) = watch::save_state(&directory, &state) {
                    *errors.lock().unwrap() = Some(e);
                }
            }
        }
        notify(update);
    });
    let mut run = Run::new(
        base,
        config.checkout.clone(),
        branch,
        tip,
        control,
        live_notify,
    )?;
    let i = run.plan("sync");
    match sync(&mut run, base, config, i) {
        Ok(()) => {
            run.end(i, "passed", "");
            match smoke::discover(&run.root, config) {
                Ok((_, scripts)) => {
                    if let Err(e) = crate::runner::run(&mut run, config, scripts, &previous) {
                        run.fail("scripts", &e);
                    }
                }
                Err(e) => run.fail("discovery", &e),
            }
        }
        Err(e) => run.end(i, "failed", &e),
    }
    if let Some(e) = save_error.lock().unwrap().as_ref() {
        run.fail("state persistence", e);
    }
    let mut branch = Branch {
        name: branch.into(),
        tip: tip.into(),
        verdict: report::verdict(
            &run.stages,
            run.failed || run.scripts.iter().any(|s| s.verdict == "red"),
        )
        .into(),
        finished: report::now(),
        detail: run
            .stages
            .iter()
            .find(|s| s.state == "failed")
            .map(|s| s.detail.clone())
            .unwrap_or_default(),
        scripts: if run.scripts.is_empty() {
            previous
        } else {
            run.scripts.clone()
        },
    };
    if run.finish() == 1 {
        branch.verdict = "red".into();
    }
    state.insert(branch.name.clone(), branch.clone());
    watch::save_state(base, &state)?;
    Ok(branch)
}
pub fn run_quick(
    base: &Path,
    config: &Config,
    path: &Path,
    control: Control,
    notify: Notify,
) -> Result<i32> {
    let mut run = Run::new(base, base.into(), "script", "working-tree", control, notify)?;
    let source = base.join(path).canonicalize().map_err(|e| e.to_string())?;
    let relative = source
        .strip_prefix(base)
        .map_err(|_| "script must be inside checkout")?
        .to_path_buf();
    let name = relative
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| ".".into());
    // --run does no sync and makes no assumption about what the script builds.
    let script = Script {
        path: relative,
        name,
        target: None,
    };
    if let Err(e) = crate::runner::run(&mut run, config, vec![script], &[]) {
        run.fail("script runner", &e);
    }
    Ok(run.finish())
}

pub enum Command {
    RunNow,
    Install,
    LoadGrab(PathBuf),
}
pub fn watch_loop(
    base: PathBuf,
    config: Config,
    commands: mpsc::Receiver<Command>,
    control: Control,
    notify: Notify,
) -> Result<()> {
    let mut state = watch::load_state(&base, &config)?;
    watch::publish(&state, &notify);
    let poller = watch::Poller::start(base.clone(), config.clone())?;
    let mut queue = Queue::default();
    let mut force = false;
    while !control.shutdown.load(Ordering::Acquire) {
        for message in poller.drain() {
            match message {
                Poll::Tip(branch, tip) => {
                    if state.get(&branch).is_none_or(|s| s.tip != tip) {
                        queue.push(branch, tip);
                    }
                }
                Poll::Failed(branch, error) => {
                    notify(Update::Failed(format!("watch {branch}: {error}")));
                    notify(Update::Log(error.clone()));
                    if let Some(b) = state.get_mut(&branch) {
                        b.verdict = "red".into();
                        b.detail = error;
                    }
                    watch::save_state(&base, &state)?;
                    watch::publish(&state, &notify);
                }
            }
        }
        while let Ok(command) = commands.try_recv() {
            match command {
                Command::RunNow => force = true,
                Command::LoadGrab(path) => match fs::read(&path) {
                    Ok(bytes) => notify(Update::Grab(path, std::sync::Arc::new(bytes))),
                    Err(e) => notify(Update::Log(format!("read grab {}: {e}", path.display()))),
                },
                Command::Install => {
                    control.stop.store(false, Ordering::Release);
                    uihub::install(&config.model, true, &control, &notify)?;
                }
            }
        }
        if force {
            for branch in &config.branches {
                match watch::tip(&base, &config, branch, &control) {
                    Ok(tip) => queue.push(branch.clone(), tip),
                    Err(e) => notify(Update::Failed(e)),
                }
            }
            force = false;
        }
        if let Some((branch, tip)) = queue.pop() {
            control.stop.store(false, Ordering::Release);
            let mut run_control = control.clone();
            run_control.model_cancel = makepad_ai_hub::backend::CancelToken::new();
            let before = state.clone();
            let result = run_once(&base, &config, &branch, &tip, run_control, notify.clone());
            if control.shutdown.load(Ordering::Acquire) {
                // The process is going away mid-run. Nothing was tested to the
                // end, so the last finished run stays the record and this tip
                // stays untested: the next start runs it again.
                watch::save_state(&base, &before)?;
                break;
            }
            let stopped = control.stop.load(Ordering::Acquire);
            let entry = match result {
                Ok(b) => b,
                Err(e) => {
                    notify(Update::Failed(e.clone()));
                    Branch {
                        name: branch.clone(),
                        tip,
                        verdict: "red".into(),
                        finished: report::now(),
                        detail: e,
                        scripts: state
                            .get(&branch)
                            .map(|b| b.scripts.clone())
                            .unwrap_or_default(),
                    }
                }
            };
            let mut entry = entry;
            if stopped && entry.verdict != "red" {
                // The user stopped it: what finished keeps its result, the
                // rest is untested, and the branch says so instead of passing.
                entry.verdict = "waiting".into();
                entry.detail = "stopped before it finished".into();
            }
            state.insert(branch, entry);
            watch::save_state(&base, &state)?;
            watch::publish(&state, &notify);
            notify(Update::Idle);
        } else {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_test_totals_and_failed_names() {
        let r=test_results("test alpha ... ok\ntest beta ... FAILED\nfailures:\n    beta\ntest result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.1s\ntest result: ok. 4 passed; 0 failed; 0 ignored;");
        assert_eq!(r.passed, 6);
        assert_eq!(r.failed, 1);
        assert_eq!(r.failed_names, BTreeSet::from(["beta".into()]));
    }
    #[test]
    fn counts_only_top_level_warning_diagnostics_by_crate() {
        let output = r#"{"reason":"compiler-message","package_id":"path+file:///repo/platform#makepad-platform@1.0.0","message":{"level":"warning","children":[{"level":"note"}]}}
{"reason":"compiler-message","package_id":"path+file:///repo/apps/demo#makepad-demo@0.1.0","message":{"level":"warning"}}
{"reason":"compiler-message","package_id":"path+file:///repo/platform#makepad-platform@1.0.0","message":{"level":"error"}}"#;
        let r = warning_counts(output);
        assert_eq!(r.get("makepad-platform"), Some(&1));
        assert_eq!(r.get("makepad-demo"), Some(&1));
        assert_eq!(r.len(), 2);
    }
}
