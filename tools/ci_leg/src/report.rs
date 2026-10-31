//! A shard's steps and verdict as lines on stdout (devhub keeps the log) and in `<dir>/steps.json`.

use makepad_strict_json::{self as json, Value};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub name: String,
    /// `running`, `passed`, `failed`, `warning`, `skipped`
    pub state: String,
    pub secs: f64,
    pub detail: String,
}

pub struct Run {
    /// the tree under test (the lease's tree)
    pub root: PathBuf,
    /// where grabs and app logs go (`local/ci-leg/<shard>/` in the tree)
    pub dir: PathBuf,
    pub steps: Vec<Step>,
    started: Vec<Instant>,
    pub failed: bool,
    pub evidence: Vec<Value>,
}

impl Run {
    pub fn new(root: PathBuf, dir: PathBuf) -> Run {
        let _ = std::fs::create_dir_all(&dir);
        Run { root, dir, steps: Vec::new(), started: Vec::new(), failed: false, evidence: Vec::new() }
    }

    pub fn log(&self, line: &str) {
        println!("  {line}");
    }

    /// A new step, running; its index.
    pub fn begin(&mut self, name: &str) -> usize {
        println!("STEP running {name}");
        self.steps.push(Step { name: name.to_string(), state: "running".into(), secs: 0.0, detail: String::new() });
        self.started.push(Instant::now());
        self.steps.len() - 1
    }

    pub fn end(&mut self, i: usize, state: &str, detail: &str) {
        let Some(s) = self.steps.get_mut(i) else { return };
        if s.state != "running" {
            return;
        }
        s.state = state.to_string();
        s.secs = self.started[i].elapsed().as_secs_f64();
        s.detail = detail.to_string();
        println!("STEP {state} {} ({:.1} s)", s.name, s.secs);
        if state == "failed" {
            self.failed = true;
            for l in detail.lines().filter(|l| !l.trim().is_empty()).take(12) {
                println!("CIERR {}: {l}", s.name);
            }
        } else if !detail.is_empty() {
            for l in detail.lines().take(6) {
                println!("  {l}");
            }
        }
    }

    /// A failure outside any step.
    pub fn fail(&mut self, name: &str, detail: &str) {
        let i = self.begin(name);
        self.end(i, "failed", detail);
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Writes `steps.json`, prints the verdict line; the exit code.
    pub fn finish(&mut self, what: &str) -> i32 {
        for i in 0..self.steps.len() {
            if self.steps[i].state == "running" {
                self.end(i, "failed", "did not finish");
            }
        }
        let steps = Value::Arr(
            self.steps
                .iter()
                .map(|s| json::obj(vec![("name", json::s(&s.name)), ("state", json::s(&s.state)), ("secs", Value::F64(s.secs)), ("detail", json::s(&s.detail))]))
                .collect(),
        );
        let doc = json::obj(vec![("what", json::s(what)), ("steps", steps), ("evidence", Value::Arr(self.evidence.clone()))]);
        let _ = std::fs::write(self.dir.join("steps.json"), doc.to_json());
        let failed: Vec<&Step> = self.steps.iter().filter(|s| s.state == "failed").collect();
        match failed.first() {
            None => {
                println!("CI-OK {what}: {} steps ({} warnings)", self.steps.len(), self.steps.iter().filter(|s| s.state == "warning").count());
                0
            }
            Some(f) => {
                println!("CI-FAIL {what}: {} ({} of {} steps failed): {}", f.name, failed.len(), self.steps.len(), f.detail.lines().next().unwrap_or(""));
                1
            }
        }
    }
}
