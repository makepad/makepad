//! Orchestration: leases instead of locks (DESIGN.md §10; interfaces in the lane's LEASE.SPEC.md).
//!
//! A lease is a share of a node (or all of it) for one lane and one kind of work. The master
//! schedules ([`sched`], [`master`]); the node starts, watches and cleans up what runs under a
//! lease ([`exec`]), measures what it has ([`caps`]) and guards its disk ([`disk`]). Leases end
//! on release or when their holder stops beating, never for running long.

pub mod caps;
pub mod client;
pub mod disk;
pub mod exec;
pub mod master;
pub mod sched;
pub mod ship;
pub mod wt;

use makepad_remote::json_str;

/// What a lease guards (DESIGN.md §10.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Build,
    Test,
    Screen,
    Long,
    Timed,
    Gpu,
    Inspect,
    Gate,
}

pub const KINDS: [Kind; 8] = [Kind::Build, Kind::Test, Kind::Screen, Kind::Long, Kind::Timed, Kind::Gpu, Kind::Inspect, Kind::Gate];

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Build => "build",
            Kind::Test => "test",
            Kind::Screen => "screen",
            Kind::Long => "long",
            Kind::Timed => "timed",
            Kind::Gpu => "gpu",
            Kind::Inspect => "inspect",
            Kind::Gate => "gate",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        KINDS.iter().copied().find(|k| k.as_str() == s)
    }

    /// Everything but `inspect` writes to the node (builds, runs, windows): off on a node whose
    /// dev switch is off.
    pub fn writes(self) -> bool {
        self != Kind::Inspect
    }

    /// Shares the node with other leases, counted against its slots.
    pub fn shared(self) -> bool {
        matches!(self, Kind::Build | Kind::Test | Kind::Screen | Kind::Long | Kind::Gate)
    }
}

/// What a lease costs a node: cores, RAM and disk in GB, GPUs; `whole` = the whole node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cost {
    pub cores: f64,
    pub ram_gb: f64,
    pub disk_gb: f64,
    pub gpus: u32,
    pub whole: bool,
}

impl Cost {
    /// The kind's default (DESIGN.md §10.8); `warm` = the lane already has a target on the node.
    pub fn default_for(kind: Kind, warm: bool) -> Cost {
        let c = |cores, ram_gb, disk_gb, gpus| Cost { cores, ram_gb, disk_gb, gpus, whole: false };
        match kind {
            Kind::Build => c(8.0, 12.0, if warm { 2.0 } else { 20.0 }, 0),
            Kind::Gate => c(8.0, 12.0, if warm { 2.0 } else { 20.0 }, 0),
            Kind::Test => c(4.0, 6.0, 2.0, 0),
            Kind::Screen => c(2.0, 4.0, 1.0, 0),
            Kind::Long => c(4.0, 8.0, 5.0, 0),
            Kind::Gpu => c(2.0, 4.0, 1.0, 1),
            Kind::Inspect => c(0.0, 0.0, 0.0, 0),
            Kind::Timed => Cost { cores: 0.0, ram_gb: 0.0, disk_gb: 2.0, gpus: 0, whole: true },
        }
    }
}

/// A node's role (`lease.conf role =`): user WMs and devices differ in budgets and disk policy, not in the dev switch (default on).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Pool,
    Suite,
    Bench,
    UserWm,
    Device,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Pool => "pool",
            Role::Suite => "suite",
            Role::Bench => "bench",
            Role::UserWm => "user-wm",
            Role::Device => "device",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        [Role::Pool, Role::Suite, Role::Bench, Role::UserWm, Role::Device].into_iter().find(|r| r.as_str() == s)
    }

    /// The user's machines: dev off until the user switches the node on.
    pub fn users(self) -> bool {
        matches!(self, Role::UserWm | Role::Device)
    }
}

/// A node's capabilities and health, as its hub measures them (`caps` service).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Caps {
    pub node: String,
    pub arch: String,
    pub os: String,
    pub cpu: String,
    pub cores: u32,
    pub load1: f64,
    pub ram_gb: f64,
    pub ram_avail_gb: f64,
    pub disk_free_gb: f64,
    pub disk_total_gb: f64,
    /// `name:vram_gb` per GPU
    pub gpus: Vec<(String, f64)>,
    /// per GPU, in the order of `gpus`: (use %, VRAM used GB)
    pub gpu_busy: Vec<(f64, f64)>,
    /// CPU use of all cores since the previous measure (None: the first one)
    pub cpu_pct: Option<f64>,
    /// a display / WM session the node can show windows on
    pub display: Option<String>,
    /// percent and charging, on devices with a battery
    pub battery: Option<(u32, bool)>,
    pub role: Option<Role>,
    pub spec: String,
    /// slots per kind from the node's lease.conf (kind, n)
    pub slots: Vec<(Kind, u32)>,
    pub disk_floor_gb: f64,
    pub disk_warn_gb: f64,
    pub timed: bool,
    /// `checkout` (desktop) or `store` (JIT hosts): DEVHUB2's applier
    pub apply: String,
    /// The Desk Link endpoint of a JIT host that embeds one ([`crate::embed`]); None elsewhere.
    pub desk_link: Option<crate::embed::DeskLinkState>,
    /// A dev switch flip made on the node itself (its tray / task view), not yet taken by the
    /// master: the master adopts it (saves, pushes), and the push clears it here.
    pub dev_req: Option<bool>,
}

impl Caps {
    pub fn role(&self) -> Role {
        self.role.unwrap_or(Role::Pool)
    }

    pub fn slots(&self, kind: Kind) -> u32 {
        self.slots.iter().find(|(k, _)| *k == kind).map_or(0, |(_, n)| *n)
    }

    pub fn to_json(&self) -> String {
        let gpus: Vec<String> = self
            .gpus
            .iter()
            .enumerate()
            .map(|(i, (n, v))| {
                let busy = self.gpu_busy.get(i).map_or(String::new(), |(u, m)| format!(",\"util_pct\":{u:.0},\"used_gb\":{m:.1}"));
                format!("{{\"name\":{},\"vram_gb\":{v:.1}{busy}}}", json_str(n))
            })
            .collect();
        let slots: Vec<String> = self.slots.iter().map(|(k, n)| format!("\"{}\":{n}", k.as_str())).collect();
        format!(
            "{{\"node\":{},\"arch\":{},\"os\":{},\"cpu\":{},\"cores\":{},\"cpu_pct\":{},\"load1\":{:.2},\"ram_gb\":{:.1},\"ram_avail_gb\":{:.1},\"disk_free_gb\":{:.1},\"disk_total_gb\":{:.1},\"gpus\":[{}],\"display\":{},\"battery\":{},\"role\":\"{}\",\"spec\":{},\"slots\":{{{}}},\"disk_floor_gb\":{:.0},\"disk_warn_gb\":{:.0},\"timed\":{},\"apply\":{},\"desk_link\":{},\"dev_req\":{}}}",
            json_str(&self.node),
            json_str(&self.arch),
            json_str(&self.os),
            json_str(&self.cpu),
            self.cores,
            self.cpu_pct.map_or("null".to_string(), |p| format!("{p:.1}")),
            self.load1,
            self.ram_gb,
            self.ram_avail_gb,
            self.disk_free_gb,
            self.disk_total_gb,
            gpus.join(","),
            self.display.as_deref().map_or("null".to_string(), json_str),
            self.battery.map_or("null".to_string(), |(p, c)| format!("{{\"pct\":{p},\"charging\":{c}}}")),
            self.role().as_str(),
            json_str(&self.spec),
            slots.join(","),
            self.disk_floor_gb,
            self.disk_warn_gb,
            self.timed,
            json_str(&self.apply),
            self.desk_link.as_ref().map_or("null".to_string(), |d| d.to_json()),
            match self.dev_req { Some(true) => "true", Some(false) => "false", None => "null" },
        )
    }

    pub fn parse(text: &str) -> Option<Caps> {
        let v = makepad_strict_json::parse(text.as_bytes()).ok()?;
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let f = |k: &str| v.get(k).and_then(num).unwrap_or(0.0);
        let mut c = Caps {
            node: s("node"),
            arch: s("arch"),
            os: s("os"),
            cpu: s("cpu"),
            cores: f("cores") as u32,
            cpu_pct: v.get("cpu_pct").and_then(num),
            load1: f("load1"),
            ram_gb: f("ram_gb"),
            ram_avail_gb: f("ram_avail_gb"),
            disk_free_gb: f("disk_free_gb"),
            disk_total_gb: f("disk_total_gb"),
            display: v.get("display").and_then(|x| x.as_str()).map(str::to_string),
            role: Role::parse(&s("role")),
            spec: s("spec"),
            disk_floor_gb: f("disk_floor_gb"),
            disk_warn_gb: f("disk_warn_gb"),
            timed: v.get("timed").and_then(|x| x.as_bool()).unwrap_or(false),
            apply: s("apply"),
            desk_link: v.get("desk_link").and_then(crate::embed::DeskLinkState::parse),
            dev_req: v.get("dev_req").and_then(|x| x.as_bool()),
            ..Default::default()
        };
        if let Some(a) = v.get("gpus").and_then(|x| x.as_arr()) {
            for g in a {
                c.gpus.push((g.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string(), g.get("vram_gb").and_then(num).unwrap_or(0.0)));
                if let Some(u) = g.get("util_pct").and_then(num) {
                    c.gpu_busy.push((u, g.get("used_gb").and_then(num).unwrap_or(0.0)));
                }
            }
        }
        if let Some(b) = v.get("battery").filter(|b| b.get("pct").is_some()) {
            c.battery = Some((b.get("pct").and_then(num).unwrap_or(0.0) as u32, b.get("charging").and_then(|x| x.as_bool()).unwrap_or(false)));
        }
        if let Some(sl) = v.get("slots") {
            for k in KINDS {
                if let Some(n) = sl.get(k.as_str()).and_then(num) {
                    c.slots.push((k, n as u32));
                }
            }
        }
        Some(c)
    }
}

/// A JSON number as f64 (strict-json keeps integers and floats apart).
pub fn num(v: &makepad_strict_json::Value) -> Option<f64> {
    match v {
        makepad_strict_json::Value::Int(i) => Some(*i as f64),
        makepad_strict_json::Value::F64(f) => Some(*f),
        _ => None,
    }
}

/// The lease words of `makepad devhub` (`wt` runs locally; the rest talk to the master).
pub fn cli(args: &[&str]) -> Option<i32> {
    if let Some(code) = wt::cli(args) {
        return Some(code);
    }
    client::cli(args)
}

/// One side of a lease's `--rev`: the branch and the commit it named when the lease was ordered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevPin {
    pub branch: String,
    pub sha: String,
}

/// Pins `--rev <branch>[@<sha>][,<commercial branch>[@<sha>]]` once, at order time, in the lane's
/// repo (`repo_dir`; the commercial side in its `apps/commercial`): every branch name becomes
/// `branch@<full sha>`. The master then ships exactly those commits, so a branch that another
/// landing moves while this lease queues or ships can neither mismatch the bundle nor pair a
/// makepad commit with a later commercial one.
pub fn pin_revs(repo_dir: &std::path::Path, spec: &str) -> Result<String, String> {
    let mut out = Vec::new();
    for (i, part) in spec.split(',').enumerate() {
        let dir = if i == 0 { repo_dir.to_path_buf() } else { repo_dir.join("apps/commercial") };
        if i > 1 {
            return Err(format!("--rev `{spec}`: at most a makepad and a commercial side"));
        }
        let (branch, sha) = part.split_once('@').map_or((part, None), |(b, s)| (b, Some(s)));
        if branch.is_empty() || branch.starts_with('-') || branch.contains(char::is_whitespace) {
            return Err(format!("--rev `{spec}`: `{branch}` is not a branch name"));
        }
        let what = match sha {
            Some(s) if (7..=40).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) => format!("{s}^{{commit}}"),
            Some(s) => return Err(format!("--rev `{spec}`: `{s}` is not a commit id")),
            None => format!("refs/heads/{branch}^{{commit}}"),
        };
        let o = makepad_remote::group::command("git")
            .arg("-C")
            .arg(&dir)
            .args(["rev-parse", "-q", "--verify", &what])
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("git: {e}"))?;
        let full = String::from_utf8_lossy(&o.stdout).trim().to_string();
        if !o.status.success() || full.len() != 40 {
            return Err(format!("--rev: {} has no {}", dir.display(), sha.map_or(format!("branch {branch}"), |s| format!("commit {s}"))));
        }
        out.push(format!("{branch}@{full}"));
    }
    Ok(out.join(","))
}

/// The pinned sides of a `--rev` ([`pin_revs`]'s output): the makepad side and the commercial one.
pub fn parse_pinned(spec: &str) -> Result<(RevPin, Option<RevPin>), String> {
    let one = |p: &str| {
        let (branch, sha) = p.split_once('@').ok_or_else(|| format!("rev `{p}` names no commit (branch@sha): the CLI pins it at order time"))?;
        if branch.is_empty() || sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
            return Err(format!("rev `{p}` is not branch@<40-hex sha>"));
        }
        Ok(RevPin { branch: branch.to_string(), sha: sha.to_string() })
    };
    let mut it = spec.split(',');
    let mk = one(it.next().unwrap_or(""))?;
    let com = it.next().map(one).transpose()?;
    if it.next().is_some() {
        return Err(format!("rev `{spec}`: at most a makepad and a commercial side"));
    }
    Ok((mk, com))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_round_trip() {
        let c = Caps {
            node: "217".into(),
            arch: "x86_64".into(),
            os: "linux".into(),
            cpu: "AMD Ryzen 7 9800X3D".into(),
            cores: 16,
            load1: 3.5,
            ram_gb: 60.0,
            ram_avail_gb: 40.0,
            disk_free_gb: 105.0,
            disk_total_gb: 1200.0,
            gpus: vec![("RTX 5090".into(), 32.0)],
            gpu_busy: vec![(37.0, 4.5)],
            cpu_pct: Some(12.5),
            display: Some("wm".into()),
            battery: None,
            role: Some(Role::Pool),
            spec: "9800X3D+5090".into(),
            slots: vec![(Kind::Build, 2), (Kind::Test, 2)],
            disk_floor_gb: 25.0,
            disk_warn_gb: 50.0,
            timed: true,
            apply: "checkout".into(),
            desk_link: Some(crate::embed::DeskLinkState { on: true, port: 8790, note: "listens on 10.0.0.201:8790".into() }),
            dev_req: Some(true),
        };
        assert_eq!(Caps::parse(&c.to_json()), Some(c.clone()));
        let none = Caps { desk_link: None, ..c };
        assert_eq!(Caps::parse(&none.to_json()), Some(none));
    }
}
