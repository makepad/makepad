//! A bounded pool of script workers; all blocking state is worker-owned.
use crate::{
    drive,
    process::{Control, Result},
    report::{Run, ScriptState, Update},
    smoke::Script,
    uihub::{Judge, JudgeWorker},
    watch::Config,
};
use std::{
    sync::{mpsc, Arc, Condvar, Mutex},
    thread,
    time::Duration,
};
#[derive(Default)]
struct GateState {
    active: usize,
    exclusive: bool,
    waiting: usize,
}
#[derive(Default)]
pub struct Gate {
    state: Mutex<GateState>,
    changed: Condvar,
}
pub struct Permit {
    gate: Arc<Gate>,
    exclusive: bool,
    active: bool,
}
impl Gate {
    pub fn is_exclusive(&self) -> bool {
        self.state.lock().map(|s| s.exclusive).unwrap_or(true)
    }
    pub fn enter(self: &Arc<Self>, control: &Control) -> Result<Permit> {
        let mut s = self.state.lock().map_err(|e| e.to_string())?;
        while s.exclusive || s.waiting > 0 {
            control.check()?;
            s = self
                .changed
                .wait_timeout(s, Duration::from_millis(100))
                .map_err(|e| e.to_string())?
                .0;
        }
        s.active += 1;
        Ok(Permit {
            gate: self.clone(),
            exclusive: false,
            active: true,
        })
    }
}
impl Permit {
    pub fn exclusive(&mut self, control: &Control) -> Result<()> {
        if self.exclusive {
            return Ok(());
        }
        let mut s = self.gate.state.lock().map_err(|e| e.to_string())?;
        s.active -= 1;
        self.active = false;
        s.waiting += 1;
        self.gate.changed.notify_all();
        while s.active > 0 || s.exclusive {
            if let Err(e) = control.check() {
                s.waiting -= 1;
                self.gate.changed.notify_all();
                return Err(e);
            }
            s = self
                .gate
                .changed
                .wait_timeout(s, Duration::from_millis(100))
                .map_err(|e| e.to_string())?
                .0;
        }
        s.waiting -= 1;
        s.exclusive = true;
        self.exclusive = true;
        Ok(())
    }
}
impl Permit {
    /// Give the wall back. A script that ran alone for the part only it may do
    /// alone (the workspace script warming the cache every other script reads)
    /// goes on as an ordinary script, and the waiting ones start beside it.
    pub fn shared(&mut self) {
        if !self.exclusive {
            return;
        }
        if let Ok(mut s) = self.gate.state.lock() {
            s.exclusive = false;
            s.active += 1;
            self.exclusive = false;
            self.active = true;
            self.gate.changed.notify_all();
        }
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        if let Ok(mut s) = self.gate.state.lock() {
            if self.exclusive {
                s.exclusive = false;
            } else if self.active {
                s.active -= 1;
            }
            self.gate.changed.notify_all();
        }
    }
}

pub fn ordered(mut scripts: Vec<Script>) -> Vec<Script> {
    scripts.sort_by(|a, b| {
        let root = |s: &Script| s.path == std::path::Path::new("ci.splash");
        root(b).cmp(&root(a)).then(a.name.cmp(&b.name))
    });
    scripts
}
fn execute(mut run: Run, config: Config, judge: Judge, script: Script, gate: &Arc<Gate>) -> Run {
    let mut permit = match gate.enter(&run.control) {
        Ok(p) => p,
        Err(e) => {
            run.fail(&script.name, &e);
            return run;
        }
    };
    if script.path == std::path::Path::new("ci.splash") {
        if let Err(e) = permit.exclusive(&run.control) {
            run.fail(&script.name, &e);
            return run;
        }
    }
    (run.notify)(Update::Begin(run.branch.clone(), run.tip.clone()));
    drive::run_script(run, config, judge, &script, Some(permit))
}
fn collect(parent: &mut Run, mut child: Run, name: &str, previous: &str) {
    // A script the user stopped was not tested. Its only "failures" are the
    // commands the stop itself cut short, so it goes back to waiting (grey)
    // and does not fail the run; a failure it had before the stop stays red.
    let stopped_untested = child.control.stopped()
        && !child.stages.iter().any(|s| s.state == "failed" && !s.detail.contains("stopped by user"));
    let mut state = child.summary(name);
    state.previous = previous.into();
    if stopped_untested {
        child.failed = false;
        child.stages.retain(|s| matches!(s.state.as_str(), "passed" | "warning"));
    }
    parent.failed |= child.failed;
    parent.stages.extend(child.stages.clone());
    parent.evidence.extend(child.evidence.clone());
    parent.grabs.extend(child.grabs.clone());
    parent.held_pids.extend(child.held_pids.clone());
    let code = child.finish();
    parent.failed |= code == 1;
    if code == 1 {
        state.verdict = "red".into();
        if state.detail.is_empty() {
            state.detail = "script finalization failed; see run log".into();
        }
    } else if stopped_untested {
        state.verdict = "waiting".into();
        state.detail = "stopped before it finished".into();
        state.steps.clear();
        state.failed_at = 0;
    }
    if let Some(old) = parent.scripts.iter_mut().find(|s| s.name == name) {
        if state.failed_at == 0 {
            state.failed_at = old.failed_at;
        }
        *old = state;
    }
    (parent.notify)(Update::Scripts(
        parent.branch.clone(),
        parent.scripts.clone(),
    ));
}
pub fn run(
    parent: &mut Run,
    config: &Config,
    scripts: Vec<Script>,
    previous: &[ScriptState],
) -> Result<()> {
    parent.app_targets = Arc::new(crate::smoke::discover(&parent.root, config)?.0);
    let scripts = ordered(scripts);
    parent.scripts = scripts
        .iter()
        .map(|s| {
            let mut state = ScriptState::waiting(&s.name);
            if let Some(old) = previous.iter().find(|old| old.name == s.name) {
                // What the last FINISHED attempt said; an untested one says nothing.
                state.previous = match old.color_verdict() {
                    "waiting" | "running" => old.previous.clone(),
                    verdict => verdict.into(),
                };
                state.detail = old.detail.clone();
                state.failed_at = old.failed_at;
            }
            state
        })
        .collect();
    (parent.notify)(Update::Scripts(
        parent.branch.clone(),
        parent.scripts.clone(),
    ));
    let worker = JudgeWorker::start(&config.model, config.no_vision)?;
    let gate = Arc::new(Gate::default());
    let result = (|| {
        let mut scripts = scripts.into_iter().enumerate().peekable();
        // The workspace script goes first and ALONE (it warms the cache every
        // other script reads), but through the same pool as the rest: once it
        // gives the wall back (`ci.shared()`), the others start beside it
        // instead of waiting for its checks and its whole test suite.
        let mut root_alone = scripts
            .peek()
            .is_some_and(|(_, s)| s.path == std::path::Path::new("ci.splash"));
        let (jobs_tx, jobs_rx) = mpsc::sync_channel::<(Run, Script, String)>(config.parallel);
        let jobs = Arc::new(Mutex::new(jobs_rx));
        let (done_tx, done_rx) = mpsc::channel();
        thread::scope(|scope| -> Result<()> {
            for index in 0..config.parallel {
                let jobs = jobs.clone();
                let done = done_tx.clone();
                let config = config.clone();
                let judge = worker.judge.clone();
                let gate = gate.clone();
                thread::Builder::new()
                    .name(format!("ci-script-{index}"))
                    .spawn_scoped(scope, move || loop {
                        let job = match jobs.lock() {
                            Ok(rx) => rx.recv(),
                            Err(_) => return,
                        };
                        let Ok((run, script, previous)) = job else {
                            return;
                        };
                        let name = script.name.clone();
                        let run = execute(run, config.clone(), judge.clone(), script, &gate);
                        if done.send((run, name, previous)).is_err() {
                            return;
                        }
                    })
                    .map_err(|e| e.to_string())?;
            }
            drop(done_tx);
            let mut pending = 0;
            loop {
                while pending < config.parallel {
                    // Nothing else is handed out until the workspace script
                    // holds the wall; the gate keeps them out until it lets go.
                    if root_alone && pending > 0 {
                        if gate.is_exclusive() {
                            root_alone = false;
                        } else {
                            break;
                        }
                    }
                    let Some((index, script)) = scripts.next() else {
                        break;
                    };
                    let previous = parent.scripts[index].previous.clone();
                    let child = parent.fork(&script.name, index)?;
                    jobs_tx
                        .send((child, script, previous))
                        .map_err(|_| "script pool disconnected")?;
                    pending += 1;
                }
                if pending == 0 {
                    break;
                }
                let (child, name, previous) = if root_alone {
                    // Only the workspace script is out: look again soon, it
                    // may hold the wall by then.
                    match done_rx.recv_timeout(Duration::from_millis(50)) {
                        Ok(done) => done,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(mpsc::RecvTimeoutError::Disconnected) => return Err("script worker failed".into()),
                    }
                } else {
                    done_rx.recv().map_err(|_| "script worker failed")?
                };
                // Whatever finished while it was alone was the workspace script.
                root_alone = false;
                pending -= 1;
                collect(parent, child, &name, &previous);
            }
            drop(jobs_tx);
            Ok(())
        })
    })();
    worker.finish();
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn root_is_first_and_gate_respects_exclusive() {
        let script = |name: &str| Script {
            path: name.into(),
            name: name.into(),
            target: None,
        };
        let order = ordered(vec![
            script("apps/z/ci.splash"),
            script("ci.splash"),
            script("apps/a/ci.splash"),
        ]);
        assert_eq!(order[0].name, "ci.splash");
        let gate = Arc::new(Gate::default());
        let c = Control::default();
        let mut p = gate.enter(&c).unwrap();
        assert_eq!(gate.state.lock().unwrap().active, 1);
        p.exclusive(&c).unwrap();
        assert!(gate.state.lock().unwrap().exclusive);
        assert_eq!(gate.state.lock().unwrap().active, 0);
        drop(p);
        assert!(!gate.state.lock().unwrap().exclusive);
    }
}

#[cfg(test)]
mod concurrency_tests {
    use super::*;
    #[test]
    fn a_script_that_gives_the_wall_back_lets_the_waiting_ones_in() {
        let gate = Arc::new(Gate::default());
        let c = Control::default();
        let mut root = gate.enter(&c).unwrap();
        root.exclusive(&c).unwrap();
        assert!(gate.state.lock().unwrap().exclusive);
        root.shared();
        {
            let s = gate.state.lock().unwrap();
            assert!(!s.exclusive);
            assert_eq!(s.active, 1, "the root script goes on as an ordinary one");
        }
        let app = gate.enter(&c).unwrap();
        assert_eq!(gate.state.lock().unwrap().active, 2);
        drop(app);
        drop(root);
        assert_eq!(gate.state.lock().unwrap().active, 0);
    }
    #[test]
    fn exclusive_waits_for_active_scripts_and_blocks_new_entries() {
        let gate = Arc::new(Gate::default());
        let control = Control::default();
        let mut first = gate.enter(&control).unwrap();
        let second = gate.enter(&control).unwrap();
        let (entered, entry) = mpsc::sync_channel(1);
        let (release, leave) = mpsc::sync_channel(1);
        let c = control.clone();
        let exclusive = thread::spawn(move || {
            first.exclusive(&c).unwrap();
            entered.send(()).unwrap();
            leave.recv().unwrap();
        });
        let mut state = gate.state.lock().unwrap();
        while state.waiting == 0 {
            let (s, timeout) = gate
                .changed
                .wait_timeout(state, Duration::from_secs(5))
                .unwrap();
            assert!(!timeout.timed_out());
            state = s;
        }
        assert_eq!(state.active, 1);
        drop(state);
        assert!(entry.try_recv().is_err());
        let (next_tx, next_rx) = mpsc::sync_channel(1);
        let next_gate = gate.clone();
        let next_control = control.clone();
        let next = thread::spawn(move || {
            let _p = next_gate.enter(&next_control).unwrap();
            next_tx.send(()).unwrap();
        });
        drop(second);
        entry.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(next_rx.try_recv().is_err());
        release.send(()).unwrap();
        next_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        exclusive.join().unwrap();
        next.join().unwrap();
        assert_eq!(gate.state.lock().unwrap().active, 0);
    }
}
