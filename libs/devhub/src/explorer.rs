//! The explorer's data: machines → JITs, parsed from `ps` answers (this machine's directly, every
//! node's through the master's router), plus drill-down into any inspect view. Collected on a
//! background thread; the UI only reads the last [`Snapshot`].

use crate::master::Master;
use makepad_remote::hub;
use makepad_remote::link::{Client, Target};
use makepad_strict_json::Value;
use std::sync::Arc;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct JitRow {
    pub pid: u32,
    pub app: String,
    pub up_s: u64,
    pub cpu_pct: Option<f64>,
    pub rss_mb: u64,
    pub threads: u64,
    /// a Relax run (else a native makepad-remote endpoint)
    pub jit: bool,
    /// the first verdict line of its `jit` answer
    pub state: String,
    /// tier counts and warm links (`session:` line of `jit`)
    pub session: String,
    pub fps: Option<f64>,
    pub warm: Option<bool>,
    pub code_mb: Option<f64>,
    pub panics: u32,
    pub restart_pending: u32,
    /// its binary was rebuilt since it started
    pub exe_replaced: bool,
    /// (kind, line, age s), newest first
    pub recent: Vec<(String, String, u64)>,
}

fn dur(s: u64) -> String {
    if s >= 3600 { format!("{}h{:02}m", s / 3600, (s / 60) % 60) } else if s >= 60 { format!("{}m{:02}s", s / 60, s % 60) } else { format!("{s}s") }
}

impl JitRow {
    /// `sandbox #4412 · up 5m02s · cpu 12% · 900 MB · 80 thr`
    pub fn head(&self) -> String {
        let cpu = self.cpu_pct.map_or("-".to_string(), |c| format!("{c:.0}%"));
        format!("{} #{}{} \u{b7} up {} \u{b7} cpu {cpu} \u{b7} {} MB \u{b7} {} thr", self.app, self.pid, if self.jit { "" } else { " (native)" }, dur(self.up_s), self.rss_mb, self.threads)
    }

    /// `idle · 60 fps · warm · O2 120 O3 40 · code 12.3 MB · 1 parked panic · stale: …`
    pub fn facts(&self) -> String {
        if !self.jit {
            return String::new();
        }
        let mut f = vec![self.state.split(" \u{b7} ").next().unwrap_or("").to_string()];
        if let Some(fps) = self.fps {
            f.push(format!("{fps:.0} fps"));
        }
        if let Some(w) = self.warm {
            f.push(if w { "warm" } else { "cold" }.to_string());
        }
        // `session: compiled 900 first (...), O2 120, O3 40; warm links 3000` -> the tier counts
        let tiers: Vec<String> = self.session.split([',', ';']).filter_map(|p| {
            let p = p.trim();
            p.strip_prefix("O2 ").or_else(|| p.strip_prefix("O3 ")).map(|n| format!("{} {}", &p[..2], n))
        }).collect();
        if let Some(first) = self.session.strip_prefix("session: compiled ").and_then(|r| r.split_whitespace().next()) {
            f.push(format!("{first} fns{}", if tiers.is_empty() { String::new() } else { format!(" ({})", tiers.join(", ")) }));
        }
        if let Some(mb) = self.code_mb {
            f.push(format!("code {mb:.1} MB"));
        }
        if self.panics > 0 {
            f.push(format!("{} parked panic{}", self.panics, if self.panics == 1 { "" } else { "s" }));
        }
        if let Some(st) = self.stale() {
            f.push(format!("stale: {st}"));
        }
        f.retain(|x| !x.is_empty());
        f.join(" \u{b7} ")
    }

    /// Not running what the checkout holds: a rebuilt binary, or edits waiting for a restart.
    pub fn stale(&self) -> Option<String> {
        match (self.exe_replaced, self.restart_pending) {
            (true, _) => Some("binary rebuilt: restart to run it".into()),
            (false, n) if n > 0 => Some(format!("{n} edits wait for a restart")),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MachineRow {
    pub name: String,
    pub os: String,
    pub cpus: u64,
    pub load1: f64,
    pub mem_total_mb: u64,
    pub mem_avail_mb: u64,
    /// a battery device: level 0..=100 and charging
    pub battery: Option<(u32, bool)>,
    pub jits: Vec<JitRow>,
    /// why it could not be asked (link down, refused)
    pub err: Option<String>,
    /// this machine (the master's own)
    pub local: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub machines: Vec<MachineRow>,
    pub at_ms: u64,
}

fn num(v: &Value, k: &str) -> f64 {
    match v.get(k) {
        Some(Value::Int(i)) => *i as f64,
        Some(Value::F64(f)) => *f,
        _ => 0.0,
    }
}

fn text(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

/// One machine from its `ps` lines.
pub fn parse_ps(name: &str, lines: &[String]) -> MachineRow {
    let mut m = MachineRow { name: name.to_string(), ..MachineRow::default() };
    for l in lines {
        let Ok(v) = makepad_strict_json::parse(l.as_bytes()) else { continue };
        if v.get("machine").is_some() {
            m.name = text(&v, "machine");
            m.os = text(&v, "os");
            m.cpus = num(&v, "cpus") as u64;
            m.load1 = num(&v, "load1");
            m.mem_total_mb = num(&v, "mem_total_mb") as u64;
            m.mem_avail_mb = num(&v, "mem_avail_mb") as u64;
            if v.get("battery").is_some() {
                m.battery = Some((num(&v, "battery") as u32, matches!(v.get("charging"), Some(Value::Bool(true)))));
            }
        } else if v.get("pid").is_some() {
            m.jits.push(JitRow {
                pid: num(&v, "pid") as u32,
                app: text(&v, "app"),
                up_s: num(&v, "up_s") as u64,
                cpu_pct: match v.get("cpu_pct") {
                    Some(Value::Int(_)) | Some(Value::F64(_)) => Some(num(&v, "cpu_pct")),
                    _ => None,
                },
                rss_mb: num(&v, "rss_mb") as u64,
                threads: num(&v, "threads") as u64,
                jit: v.get("jit").and_then(|j| j.as_bool()).unwrap_or(false),
                state: text(&v, "state"),
                session: text(&v, "session"),
                fps: v.get("fps").filter(|x| !x.is_null()).map(|_| num(&v, "fps")),
                warm: v.get("warm").and_then(|w| w.as_bool()),
                code_mb: v.get("code_mb").filter(|x| !x.is_null()).map(|_| num(&v, "code_mb")),
                panics: num(&v, "panics") as u32,
                restart_pending: num(&v, "restart_pending") as u32,
                exe_replaced: v.get("exe_replaced").and_then(|x| x.as_bool()).unwrap_or(false),
                recent: v.get("recent").and_then(|a| a.as_arr()).map_or_else(Vec::new, |a| a.iter().map(|e| (text(e, "kind"), text(e, "line"), num(e, "age_s") as u64)).collect()),
            });
        } else if let Some(e) = v.get("err").and_then(|e| e.as_str()) {
            m.err = Some(e.to_string());
        }
    }
    m
}

/// Every machine the master knows, asked in parallel (a slow node never holds up the others
/// past the link's own timeouts).
pub fn collect(master: &Master) -> Snapshot {
    let mut local = Vec::new();
    master.local.ps(&mut local);
    let mut me = parse_ps(&master.name, &local);
    me.local = true;
    let hosts = master.hosts();
    let threads: Vec<_> = hosts
        .hosts
        .into_iter()
        .map(|h| {
            std::thread::spawn(move || {
                let c = Client::new(Target::Unix(crate::proto::control_socket()), "devhub-explorer");
                let mut lines = Vec::new();
                match hub::ask(&c, &h.name, "ps", "", "ps", &mut |l| {
                    lines.push(l.to_string());
                    true
                }) {
                    // the master's name for it (nodes.conf): what routing and the lease view use
                    Ok(_) => MachineRow { name: h.name.clone(), ..parse_ps(&h.name, &lines) },
                    Err(e) => MachineRow { name: h.name.clone(), err: Some(e), ..MachineRow::default() },
                }
            })
        })
        .collect();
    let mut machines = vec![me];
    for t in threads {
        if let Ok(m) = t.join() {
            machines.push(m);
        }
    }
    Snapshot { machines, at_ms: crate::proto::wall_ms() }
}

/// An inspect view of one JIT (`cmd` = any inspect command), as answer lines.
pub fn drill(master: &Arc<Master>, machine: &str, pid: u32, cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    if machine == master.name {
        match Client::connect(&pid.to_string()).and_then(|c| c.cmd(cmd)) {
            Ok(lines) => out.extend(lines.map(|l| l.unwrap_or_else(|e| format!("{{\"err\":\"{e}\"}}"))).filter(|l| !l.starts_with("{\"measured\""))),
            Err(e) => out.push(format!("error: {e}")),
        }
        return out;
    }
    let c = Client::new(Target::Unix(crate::proto::control_socket()), "devhub-explorer");
    if let Err(e) = hub::ask(&c, &format!("{machine}:{pid}"), "cmd", "", cmd, &mut |l| {
        // the answer's provenance line is for agents; the panel shows the answer
        if !l.starts_with("{\"measured\"") {
            out.push(l.to_string());
        }
        true
    }) {
        out.push(format!("error: {e}"));
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn ps_lines_become_rows() {
        let lines = vec![
            r#"{"machine":"217","os":"linux","cpus":16,"load1":2.5,"mem_total_mb":61000,"mem_avail_mb":40000,"uptime_s":5,"jits":1}"#.to_string(),
            r#"{"pid":42,"app":"sandbox","up_s":90,"cpu_pct":12.5,"rss_mb":900,"threads":80,"jit":true,"observe":true,"run":"x","state":"idle"}"#.to_string(),
            r#"{"pid":43,"app":"wm","up_s":9,"cpu_pct":null,"rss_mb":300,"threads":60,"jit":false,"observe":false,"run":"","state":""}"#.to_string(),
        ];
        let m = super::parse_ps("x", &lines);
        assert_eq!(m.name, "217");
        assert_eq!(m.jits.len(), 2);
        assert_eq!(m.jits[0].cpu_pct, Some(12.5));
        assert_eq!(m.jits[1].cpu_pct, None);
        assert!(m.jits[0].jit && !m.jits[1].jit);
    }
}
