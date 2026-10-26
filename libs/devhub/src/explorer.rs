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
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MachineRow {
    pub name: String,
    pub os: String,
    pub cpus: u64,
    pub load1: f64,
    pub mem_total_mb: u64,
    pub mem_avail_mb: u64,
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
                    Ok(_) => parse_ps(&h.name, &lines),
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
            Ok(lines) => out.extend(lines.map(|l| l.unwrap_or_else(|e| format!("{{\"err\":\"{e}\"}}")))),
            Err(e) => out.push(format!("error: {e}")),
        }
        return out;
    }
    let c = Client::new(Target::Unix(crate::proto::control_socket()), "devhub-explorer");
    if let Err(e) = hub::ask(&c, &format!("{machine}:{pid}"), "cmd", "", cmd, &mut |l| {
        out.push(l.to_string());
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
