use crate::process::{self, Control, Result};
use makepad_strict_json::{self as json, Value};
use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub fn stamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(u64::MAX as u128) as u64
}
pub fn safe_name(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct Stage {
    pub name: String,
    pub state: String,
    pub seconds: f64,
    pub command: String,
    pub detail: String,
    pub started: Option<Instant>,
}
impl Stage {
    pub fn json(&self) -> Value {
        json::obj(vec![
            ("name", json::s(&self.name)),
            ("state", json::s(&self.state)),
            ("seconds", Value::F64(self.seconds)),
            ("command", json::s(&self.command)),
            ("detail", json::s(&self.detail)),
        ])
    }
}
#[derive(Clone, Debug)]
pub struct ScriptState {
    pub name: String,
    pub verdict: String,
    pub previous: String,
    pub failed_at: u64,
    pub steps: Vec<Stage>,
    pub seconds: f64,
    pub started: Option<Instant>,
    pub detail: String,
    pub grabs: Vec<String>,
    pub log: String,
}
impl ScriptState {
    pub fn waiting(name: &str) -> Self {
        Self {
            name: name.into(),
            verdict: "waiting".into(),
            previous: "waiting".into(),
            failed_at: 0,
            steps: Vec::new(),
            seconds: 0.0,
            started: None,
            detail: String::new(),
            grabs: Vec::new(),
            log: String::new(),
        }
    }
    pub fn counts(&self) -> (usize, usize, usize) {
        (
            self.steps.iter().filter(|s| s.state == "passed").count(),
            self.steps
                .iter()
                .filter(|s| matches!(s.state.as_str(), "warning" | "skipped"))
                .count(),
            self.steps.iter().filter(|s| s.state == "failed").count(),
        )
    }
    pub fn color_verdict(&self) -> &str {
        if self.steps.iter().any(|s| s.state == "failed")
            || self.verdict == "red"
            || (self.previous == "red" && self.verdict != "green")
        {
            "red"
        } else if self.verdict == "running" {
            &self.previous
        } else {
            &self.verdict
        }
    }
    pub fn json(&self) -> Value {
        json::obj(vec![
            ("name", json::s(&self.name)),
            ("verdict", json::s(&self.verdict)),
            ("previous", json::s(&self.previous)),
            ("failed_at", Value::Int(self.failed_at as i64)),
            ("seconds", Value::F64(self.seconds)),
            ("detail", json::s(&self.detail)),
            (
                "steps",
                Value::Arr(self.steps.iter().map(Stage::json).collect()),
            ),
            (
                "grabs",
                Value::Arr(self.grabs.iter().map(json::s).collect()),
            ),
            ("log", json::s(&self.log)),
        ])
    }
    pub fn from_json(v: &Value) -> Result<Self> {
        let string = |key: &str| v.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        let mut s = Self::waiting(&string("name"));
        if s.name.is_empty() {
            return Err("script state missing name".into());
        }
        s.verdict = string("verdict");
        s.previous = string("previous");
        s.failed_at = v.get("failed_at").and_then(Value::as_u64).unwrap_or(0);
        s.detail = string("detail");
        s.log = string("log");
        s.seconds = number(v.get("seconds"));
        if let Some(a) = v.get("grabs").and_then(Value::as_arr) {
            s.grabs = a
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
        }
        if let Some(a) = v.get("steps").and_then(Value::as_arr) {
            for v in a {
                let f = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                s.steps.push(Stage {
                    name: f("name"),
                    state: f("state"),
                    command: f("command"),
                    detail: f("detail"),
                    seconds: number(v.get("seconds")),
                    started: None,
                });
            }
        }
        if s.verdict == "running" {
            s.verdict = "red".into();
            s.detail = "previous run was interrupted".into();
        }
        Ok(s)
    }
}
fn number(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::F64(v)) => *v,
        Some(Value::Int(v)) => *v as f64,
        _ => 0.0,
    }
}
pub fn verdict(stages: &[Stage], failed: bool) -> &'static str {
    if failed || stages.iter().any(|s| s.state == "failed") {
        "red"
    } else if stages
        .iter()
        .any(|s| matches!(s.state.as_str(), "warning" | "skipped"))
    {
        "orange"
    } else {
        "green"
    }
}

#[derive(Clone, Debug)]
pub struct Branch {
    pub name: String,
    pub tip: String,
    pub verdict: String,
    pub finished: u64,
    pub scripts: Vec<ScriptState>,
    pub detail: String,
}
#[derive(Clone)]
pub enum Update {
    Log(String),
    Branches(Vec<Branch>),
    Begin(String, String),
    Stages(Vec<Stage>),
    Failed(String),
    Grab(PathBuf, Arc<Vec<u8>>),
    Model(String),
    Done(bool, PathBuf),
    Idle,
    Exited,
    Scripts(String, Vec<ScriptState>),
    Script(String, String, Box<Update>),
}
pub type Notify = Arc<dyn Fn(Update) + Send + Sync>;

pub struct Run {
    pub root: PathBuf,
    pub dir: PathBuf,
    pub branch: String,
    pub tip: String,
    pub stages: Vec<Stage>,
    pub failed: bool,
    failure_stamp: u64,
    pub control: Control,
    pub notify: Notify,
    pub evidence: Vec<Value>,
    pub grabs: Vec<String>,
    pub held_pids: Vec<u32>,
    pub scripts: Vec<ScriptState>,
    log: File,
    started: Instant,
    serial: usize,
}
impl Run {
    pub fn new(
        base: &Path,
        root: PathBuf,
        branch: &str,
        tip: &str,
        control: Control,
        notify: Notify,
    ) -> Result<Self> {
        fs::create_dir_all(base.join("local/ci/runs")).map_err(|e| e.to_string())?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = base
            .join("local/ci/runs")
            .join(format!("{stamp}-{}", safe_name(branch)));
        fs::create_dir(&dir).map_err(|e| e.to_string())?;
        let log = File::create(dir.join("log.txt")).map_err(|e| e.to_string())?;
        notify(Update::Begin(branch.into(), tip.into()));
        Ok(Self {
            root,
            dir,
            branch: branch.into(),
            tip: tip.into(),
            stages: Vec::new(),
            failed: false,
            failure_stamp: 0,
            control,
            notify,
            evidence: Vec::new(),
            grabs: Vec::new(),
            held_pids: Vec::new(),
            scripts: Vec::new(),
            log,
            started: Instant::now(),
            serial: 0,
        })
    }
    pub fn fork(&self, name: &str, index: usize) -> Result<Self> {
        let dir = self
            .dir
            .join(format!("script-{index:04}-{}", safe_name(name)));
        fs::create_dir(&dir).map_err(|e| e.to_string())?;
        let log = File::create(dir.join("log.txt")).map_err(|e| e.to_string())?;
        let notify = self.notify.clone();
        let branch = self.branch.clone();
        let name = name.to_string();
        let notify: Notify = Arc::new(move |update| {
            notify(Update::Script(
                branch.clone(),
                name.clone(),
                Box::new(update),
            ))
        });
        Ok(Self {
            root: self.root.clone(),
            dir,
            branch: self.branch.clone(),
            tip: self.tip.clone(),
            stages: Vec::new(),
            failed: false,
            failure_stamp: 0,
            control: self.control.clone(),
            notify,
            evidence: Vec::new(),
            grabs: Vec::new(),
            held_pids: Vec::new(),
            scripts: Vec::new(),
            log,
            started: Instant::now(),
            serial: 0,
        })
    }
    pub fn summary(&self, name: &str) -> ScriptState {
        let mut s = ScriptState::waiting(name);
        s.verdict = verdict(&self.stages, self.failed).into();
        s.steps = self.stages.clone();
        s.failed_at = self.failure_stamp;
        s.seconds = self.started.elapsed().as_secs_f64();
        s.detail = self
            .stages
            .iter()
            .find(|s| s.state == "failed")
            .or_else(|| {
                self.stages
                    .iter()
                    .find(|s| matches!(s.state.as_str(), "warning" | "skipped"))
            })
            .map(|s| s.detail.clone())
            .unwrap_or_default();
        s.grabs = self.grabs.clone();
        s.log = fs::read_to_string(self.dir.join("log.txt")).unwrap_or_default();
        trim_log(&mut s.log);
        s
    }
    pub fn log(&mut self, text: &str) {
        let line = format!("[{:.1}s] {text}", self.started.elapsed().as_secs_f64());
        let _ = writeln!(std::io::stdout(), "{line}");
        if let Err(e) = writeln!(self.log, "{line}").and_then(|_| self.log.flush()) {
            self.failed = true;
            (self.notify)(Update::Failed(format!("cannot write run log: {e}")));
        }
        (self.notify)(Update::Log(line));
    }
    pub fn publish(&self) {
        (self.notify)(Update::Stages(self.stages.clone()));
    }
    pub fn plan(&mut self, name: &str) -> usize {
        let i = self.stages.len();
        self.stages.push(Stage {
            name: name.into(),
            state: "waiting".into(),
            seconds: 0.0,
            command: String::new(),
            detail: String::new(),
            started: None,
        });
        self.publish();
        i
    }
    pub fn begin(&mut self, i: usize, command: &str) {
        let s = &mut self.stages[i];
        s.state = "running".into();
        s.started = Some(Instant::now());
        s.command = command.into();
        self.publish();
    }
    pub fn annotate(&mut self, i: usize, detail: &str) {
        if !detail.is_empty() && !self.stages[i].detail.contains(detail) {
            self.stages[i].detail = if self.stages[i].detail.is_empty() {
                detail.into()
            } else {
                format!("{detail}\n{}", self.stages[i].detail)
            };
            self.publish();
        }
    }
    pub fn end(&mut self, i: usize, state: &str, detail: &str) {
        let s = &mut self.stages[i];
        s.seconds = s.started.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
        s.state = state.into();
        if !detail.is_empty() && !s.detail.contains(detail) {
            s.detail = if s.detail.is_empty() { detail.into() } else { format!("{detail}\n{}", s.detail) };
        }
        if state == "failed" {
            self.failure_stamp = stamp();
            self.failed = true;
            (self.notify)(Update::Failed(format!("{}: {detail}", s.name)));
        }
        self.log(&format!("{}: {state} — {detail}", self.stages[i].name));
        self.publish();
    }
    pub fn fail(&mut self, name: &str, detail: &str) {
        let i = self.plan(name);
        self.end(i, "failed", detail);
    }
    pub fn path(&mut self, name: &str) -> PathBuf {
        self.serial += 1;
        self.dir
            .join(format!("{:04}-{}", self.serial, safe_name(name)))
    }
    pub fn command(
        &mut self,
        program: &str,
        args: &[String],
        cwd: &Path,
        env: &[(String, String)],
        timeout: u64,
    ) -> Result<process::Output> {
        if program == "cargo" {
            return self.cargo_command(program, args, cwd, env, timeout).map(|r| r.output);
        }
        let path = self.path(&format!("{program}.log"));
        let control = self.control.clone();
        self.log(&format!(
            "$ {} (cwd={})",
            display_command(program, args),
            cwd.display()
        ));
        process::run(
            program,
            args,
            cwd,
            env,
            timeout,
            path,
            &control,
            &mut |line| self.log(line),
        )
    }
    pub fn cargo_command(
        &mut self,
        program: &str,
        args: &[String],
        cwd: &Path,
        env: &[(String, String)],
        timeout: u64,
    ) -> Result<crate::cargo::CargoResult> {
        let capture = self.path("cargo-output.log");
        let raw = capture.with_extension("jsonl");
        let mut json_file = File::create(&raw).map_err(|e| e.to_string())?;
        let control = self.control.clone();
        let start = Instant::now();
        self.log(&format!("$ {} (cwd={})", display_command(program, args), cwd.display()));
        let mut write_error = None;
        let output = process::run(program, args, cwd, env, timeout, capture.clone(), &control, &mut |line| {
            if json::parse_depth(line.as_bytes(), 128).is_ok() {
                if let Err(e) = writeln!(json_file, "{line}") {
                    write_error = Some(e.to_string());
                }
            }
            if let Some(text) = crate::cargo::rendered_diagnostic(line) {
                self.log(&text);
            }
        });
        let files = format!("raw cargo JSON: {}\nfull cargo output: {}", raw.display(), capture.display());
        // Includes metadata and timeout/spawn failures, whose caller may have
        // no CargoResult to attach. Nested target steps have their own entry.
        if let Some(i) = self.stages.iter().rposition(|s| s.state == "running") {
            self.annotate(i, &files);
        }
        let result = output.and_then(|out| {
            if let Some(e) = write_error {
                return Err(format!("cannot write cargo JSON: {e}"));
            }
            let mut result = crate::cargo::CargoResult::parse(out);
            result.raw_json = Some(raw);
            Ok(result)
        });
        match &result {
            Ok(r) => self.log(&format!("cargo summary: exit {}; {} warnings; {} error lines; {:.1}s",
                r.output.code, r.warnings.values().sum::<u64>(), r.errors.len(), start.elapsed().as_secs_f64())),
            Err(e) => self.log(&format!("cargo summary: {e}; {:.1}s", start.elapsed().as_secs_f64())),
        }
        result.map_err(|e| format!("{e}\n{files}"))
    }
    pub fn finish(mut self) -> i32 {
        for i in 0..self.stages.len() {
            if matches!(self.stages[i].state.as_str(), "waiting" | "running") {
                self.end(i, "failed", "dependency failed or run stopped");
            }
        }
        let verdict = verdict(
            &self.stages,
            self.failed || self.scripts.iter().any(|s| s.verdict == "red"),
        );
        let value = json::obj(vec![
            (
                "scripts",
                Value::Arr(self.scripts.iter().map(ScriptState::json).collect()),
            ),
            ("branch", json::s(&self.branch)),
            ("tip", json::s(&self.tip)),
            ("checkout", json::s(self.root.display().to_string())),
            ("verdict", json::s(verdict)),
            ("seconds", Value::F64(self.started.elapsed().as_secs_f64())),
            (
                "stages",
                Value::Arr(self.stages.iter().map(Stage::json).collect()),
            ),
            ("evidence", Value::Arr(self.evidence.clone())),
            (
                "grabs",
                Value::Arr(self.grabs.iter().map(json::s).collect()),
            ),
            (
                "human_owned_pids",
                Value::Arr(
                    self.held_pids
                        .iter()
                        .map(|x| Value::Int(*x as i64))
                        .collect(),
                ),
            ),
        ]);
        let text = format!(
            "{} {} {verdict}\n{}\n",
            self.branch,
            self.tip,
            self.stages
                .iter()
                .map(|s| format!(
                    "{}: {} ({:.1}s)\n{}\n{}",
                    s.name, s.state, s.seconds, s.command, s.detail
                ))
                .collect::<Vec<_>>()
                .join("\n")
        );
        if let Err(e) = fs::write(self.dir.join("report.json"), value.to_json())
            .and_then(|_| fs::write(self.dir.join("report.txt"), text))
        {
            self.failed = true;
            (self.notify)(Update::Failed(format!("report write failed: {e}")));
        }
        let label = if self.failed {
            "RED".to_string()
        } else {
            verdict.to_ascii_uppercase()
        };
        self.log(&label);
        if !self.scripts.is_empty() {
            self.log(&cli_summary(&self.scripts));
        }
        let code = if self.failed { 1 } else { exit_code(verdict) };
        (self.notify)(Update::Done(code != 1, self.dir.clone()));
        code
    }
}
pub fn exit_code(verdict: &str) -> i32 {
    match verdict {
        "green" => 0,
        "orange" => 2,
        _ => 1,
    }
}
pub fn cli_summary(scripts: &[ScriptState]) -> String {
    let mut lines = vec!["Script summary:".to_string()];
    for script in scripts {
        lines.push(format!("{} {}", script.verdict.to_ascii_uppercase(), script.name));
    }
    for script in scripts {
        for step in &script.steps {
            if step.state != "passed" {
                let problem = step.detail.lines().find(|s| !s.trim().is_empty()).unwrap_or(&step.state);
                lines.push(format!("  {} [{}]: {problem}", step.name, step.state));
            }
        }
        if script.verdict == "red" && !script.steps.iter().any(|s| s.state == "failed") {
            lines.push(format!("  {}: {}", script.name, script.detail.lines().next().unwrap_or("script failed")));
        }
    }
    lines.join("\n")
}
pub fn display_command(program: &str, args: &[String]) -> String {
    std::iter::once(program.to_string())
        .chain(args.iter().map(|a| format!("{a:?}")))
        .collect::<Vec<_>>()
        .join(" ")
}
pub fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

pub fn trim_log(text: &mut String) {
    if text.len() > 24000 {
        let mut cut = text.len() - 24000;
        while !text.is_char_boundary(cut) {
            cut += 1;
        }
        text.drain(..cut);
    }
}

/// Merge runner snapshots without replacing a still-running sibling with its
/// original waiting entry. Completed entries always carry authoritative state.
pub fn merge_scripts(current: &mut Vec<ScriptState>, incoming: &[ScriptState]) {
    let mut next = Vec::with_capacity(incoming.len());
    for s in incoming {
        if s.verdict == "waiting" {
            if let Some(old) = current
                .iter()
                .find(|old| old.name == s.name && old.started.is_some())
            {
                next.push(old.clone());
                continue;
            }
        }
        next.push(s.clone());
    }
    *current = next;
}
pub fn script_update(state: &mut ScriptState, update: &Update) -> bool {
    match update {
        Update::Begin(_, _) => {
            state.verdict = "running".into();
            state.started = Some(Instant::now());
            state.steps.clear();
            state.log.clear();
            state.grabs.clear();
            true
        }
        Update::Stages(steps) => {
            if steps.iter().filter(|s| s.state == "failed").count() > state.counts().2 {
                state.failed_at = stamp();
            }
            state.steps = steps.clone();
            state.detail = steps
                .iter()
                .find(|s| s.state == "failed")
                .or_else(|| steps.iter().find(|s| s.state == "warning"))
                .map(|s| s.detail.clone())
                .unwrap_or_default();
            true
        }
        Update::Failed(detail) => {
            state.detail = detail.clone();
            true
        }
        Update::Log(line) => {
            state.log.push_str(line);
            state.log.push('\n');
            trim_log(&mut state.log);
            false
        }
        Update::Grab(path, _) => {
            let path = path.display().to_string();
            if !state.grabs.contains(&path) {
                state.grabs.push(path);
            }
            true
        }
        Update::Done(passed, _) => {
            state.seconds = state
                .started
                .map(|s| s.elapsed().as_secs_f64())
                .unwrap_or(0.0);
            state.verdict = verdict(&state.steps, !passed).into();
            true
        }
        _ => false,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn persisted_red_survives_warning_and_restarted_running_state() {
        let mut s = ScriptState::waiting("apps/wm");
        s.previous = "red".into();
        s.verdict = "orange".into();
        assert_eq!(s.color_verdict(), "red");
        let restored = ScriptState::from_json(&s.json()).unwrap();
        assert_eq!(restored.color_verdict(), "red");
        s.verdict = "green".into();
        assert_eq!(s.color_verdict(), "green");
        s.verdict = "running".into();
        let restored = ScriptState::from_json(&s.json()).unwrap();
        assert_eq!(restored.verdict, "red");
    }
    #[test]
    fn finishing_a_sibling_preserves_active_steps() {
        let mut active = ScriptState::waiting("a");
        script_update(&mut active, &Update::Begin("work".into(), "tip".into()));
        let mut states = vec![active];
        merge_scripts(
            &mut states,
            &[ScriptState::waiting("a"), ScriptState::waiting("b")],
        );
        assert_eq!(states[0].verdict, "running");
        assert_eq!(states[1].verdict, "waiting");
    }
    #[test]
    fn final_summary_lists_script_colours_then_first_problem_lines() {
        assert_eq!((exit_code("green"), exit_code("orange"), exit_code("red")), (0, 2, 1));
        let mut green = ScriptState::waiting(".");
        green.verdict = "green".into();
        let mut orange = ScriptState::waiting("apps/wm");
        orange.verdict = "orange".into();
        orange.previous = "red".into(); // UI history must not colour this CLI run.
        orange.steps.push(Stage {
            name: "apps/wm / check wasm".into(), state: "warning".into(), seconds: 0.0,
            command: String::new(), detail: "no lib target for web\nraw cargo JSON: file".into(), started: None,
        });
        let mut red = ScriptState::waiting("apps/demo");
        red.verdict = "red".into();
        red.steps.push(Stage {
            name: "apps/demo / build".into(), state: "failed".into(), seconds: 0.0,
            command: String::new(), detail: "error: broken\nsource.rs:2".into(), started: None,
        });
        assert_eq!(cli_summary(&[green, orange, red]),
            "Script summary:\nGREEN .\nORANGE apps/wm\nRED apps/demo\n  apps/wm / check wasm [warning]: no lib target for web\n  apps/demo / build [failed]: error: broken");
    }
    #[cfg(unix)]
    #[test]
    fn cargo_capture_keeps_json_out_of_log_and_links_step_evidence() {
        let root = std::env::temp_dir().join(format!("makepad-ci-cargo-{}-{}", std::process::id(), stamp()));
        let mut run = Run::new(&root, root.clone(), "capture", "test", Control::default(), Arc::new(|_| {})).unwrap();
        let i = run.plan("app / check");
        run.begin(i, "fixture cargo output");
        let output = "{\"reason\":\"compiler-artifact\",\"fresh\":true}\n{\"reason\":\"compiler-message\",\"package_id\":\"path+file:///repo#app@1\",\"message\":{\"level\":\"warning\",\"rendered\":\"warning: example\\n  --> source.rs:1\\n\"}}\n{\"reason\":\"build-finished\",\"success\":true}\n";
        // printf is a shell builtin; this emits a fixture, never invokes Cargo,
        // a CI runtime, a model, a socket or a graphics backend.
        let result = run.cargo_command("/bin/sh", &strings(&["-c", "printf '%s' \"$CI_CARGO_FIXTURE\""]), &root,
            &[("CI_CARGO_FIXTURE".into(), output.into())], 5).unwrap();
        run.annotate(i, &result.warning_text(Some("app")));
        run.end(i, "warning", "");
        let raw = result.raw_json.unwrap();
        assert_eq!(fs::read_to_string(&raw).unwrap(), output);
        assert!(run.stages[i].detail.contains(&raw.display().to_string()));
        assert!(run.stages[i].detail.starts_with("app: 1 warnings"));
        let log = fs::read_to_string(run.dir.join("log.txt")).unwrap();
        assert!(log.contains("warning: example\n  --> source.rs:1"));
        assert!(!log.contains("compiler-artifact"));
        assert!(!log.contains("compiler-message"));
        assert!(!log.contains("build-finished"));
        assert_eq!(log.matches("cargo summary:").count(), 1);
        assert_eq!(run.finish(), 2);
        fs::remove_dir_all(root).unwrap();
    }

}
