//! A command under a lease, run by the master itself (verify legs, allapps): the lease is asked on
//! the master's own control socket like any CLI's, this process holds it and beats it, the node
//! runs the command in the lease's tree at the pinned commits, and its output streams into a log
//! file here. The lease ends with the master: a restarted master starts the job again.

use crate::cli::ask;
use crate::lease::sched::{parse_needs, Req};
use crate::lease::Kind;
use makepad_remote::link::{Client, Target};
use makepad_strict_json::Value;
use std::io::Write;
use std::time::{Duration, Instant};

const BEAT: Duration = Duration::from_secs(2);
/// How long node calls may fail in a row (a link coming back) before the run counts as lost.
const NODE_GRACE: Duration = Duration::from_secs(90);

pub struct Spec {
    pub lane: String,
    pub agent: String,
    pub kind: Kind,
    /// `arch=x64,os=linux,role=pool`
    pub needs: String,
    pub prio: i32,
    /// the pair's `--rev` (pinned here)
    pub rev: String,
    pub repo_dir: String,
    /// a preferred node (a twin of the same arch and os may take it)
    pub node: Option<String>,
}

/// A granted lease this process holds; released on drop.
pub struct Held {
    pub id: String,
    pub node: String,
    ctl: Client,
    released: bool,
}

fn json(l: &str) -> Option<Value> {
    makepad_strict_json::parse(l.as_bytes()).ok()
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn first(lines: Result<Vec<String>, String>, what: &str) -> Result<Value, String> {
    let lines = lines?;
    let v = lines.first().and_then(|l| json(l)).ok_or_else(|| format!("{what}: no answer"))?;
    match v.get("err").and_then(|e| e.as_str()) {
        Some(e) => Err(e.to_string()),
        None => Ok(v),
    }
}

impl Held {
    /// Asks for the lease and waits for its grant; `on_wait` hears each queue state line, and
    /// `stop` (asked every beat) gives the request up.
    pub fn acquire(spec: &Spec, on_wait: &mut dyn FnMut(&str), stop: &dyn Fn() -> bool) -> Result<Held, String> {
        let ctl = Client::new(Target::Unix(crate::proto::control_socket()), "devhub-orch");
        let mut req = Req::new(&spec.lane, spec.kind);
        req.needs = parse_needs(&spec.needs);
        req.prio = spec.prio;
        req.node = spec.node.clone();
        req.repo_dir = Some(spec.repo_dir.clone());
        req.rev = Some(crate::lease::pin_revs(std::path::Path::new(&spec.repo_dir), &spec.rev)?);
        req.holder = Some(std::process::id());
        req.agent = Some(spec.agent.clone());
        let mut v = first(ask(&ctl, None, "lease", &req.to_json()), "lease")?;
        let id = s(&v, "id");
        let mut last = String::new();
        loop {
            match s(&v, "state").as_str() {
                "granted" => return Ok(Held { id, node: s(&v, "node"), ctl, released: false }),
                "queued" | "starting" => {
                    let line = format!("{} {} {}", id, s(&v, "state"), s(&v, "reason"));
                    if line != last {
                        on_wait(&line);
                        last = line;
                    }
                }
                _ => return Err(format!("{id} {}: {}", s(&v, "how"), s(&v, "reason"))),
            }
            std::thread::sleep(BEAT);
            if stop() {
                let _ = ask(&ctl, None, "release", &id);
                return Err(format!("{id}: given up"));
            }
            v = first(ask(&ctl, None, "beat", &id), "beat")?;
        }
    }

    fn node_call(&self, service: &str, line: &str) -> Result<Value, String> {
        first(ask(&self.ctl, Some(&self.node), service, line), service).map_err(|e| format!("{}: {e}", self.node))
    }

    /// Runs `argv` in the lease's tree; its output goes to `log`; its exit code.
    pub fn run(&self, argv: &[String], log: &mut dyn Write) -> Result<i32, String> {
        let b64 = makepad_network::digest::base64_encode(argv.join("\0").as_bytes());
        let v = self.node_call("lease-run", &format!("lease-run {} . {b64}", self.id))?;
        let n = v.get("n").and_then(crate::lease::num).map_or(1, |x| x as u64);
        let (mut off, mut beat, mut failing) = (0u64, Instant::now(), None::<Instant>);
        loop {
            match self.node_call("lease-log", &format!("lease-log {} {n} {off}", self.id)) {
                Ok(v) => {
                    failing = None;
                    let data = crate::lease::master::b64_decode(&s(&v, "data")).ok_or("lease-log: bad data")?;
                    let _ = log.write_all(&data);
                    off = v.get("off").and_then(crate::lease::num).map_or(off + data.len() as u64, |x| x as u64);
                    if data.is_empty() {
                        if let Some(code) = v.get("exit").filter(|e| !e.is_null()).and_then(crate::lease::num) {
                            let _ = log.flush();
                            return Ok(code as i32);
                        }
                        std::thread::sleep(Duration::from_millis(500));
                    }
                }
                Err(e) => {
                    let t = *failing.get_or_insert_with(Instant::now);
                    if t.elapsed() > NODE_GRACE {
                        return Err(e);
                    }
                    std::thread::sleep(Duration::from_secs(2));
                }
            }
            if beat.elapsed() >= BEAT {
                beat = Instant::now();
                let st = first(ask(&self.ctl, None, "beat", &self.id), "beat")?;
                if s(&st, "state") == "ended" {
                    return Err(format!("{} ended ({})", self.id, s(&st, "how")));
                }
            }
        }
    }

    /// Runs `argv` and returns what it printed (short commands: `git rev-parse`).
    pub fn output(&self, argv: &[&str]) -> Result<(i32, String), String> {
        let mut buf = Vec::new();
        let argv: Vec<String> = argv.iter().map(|a| a.to_string()).collect();
        let code = self.run(&argv, &mut buf)?;
        Ok((code, String::from_utf8_lossy(&buf).trim().to_string()))
    }

    pub fn release(mut self, exit: Option<i32>) {
        self.end(exit);
    }

    fn end(&mut self, exit: Option<i32>) {
        if self.released {
            return;
        }
        self.released = true;
        let line = match exit {
            Some(c) => format!("{} released exit={c}", self.id),
            None => self.id.clone(),
        };
        let _ = ask(&self.ctl, None, "release", &line);
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        self.end(None);
    }
}
