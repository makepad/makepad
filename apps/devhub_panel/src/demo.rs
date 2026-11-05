//! A synthetic fleet built in code (`DEVHUB_PANEL_DEMO=1`, and the tests): ten invented nodes
//! and a small scheduler run over four hours (three before the start, one after), so the panel
//! has leases that queue, run, end and pile up behind budgets and timed work, landings, buckets
//! and pressure. [`Demo::answer`] cuts what a master would answer at `now` out of that run: a
//! demo panel left open keeps moving for an hour.

use crate::model::{Bucket, Fleet, InboxCounts, InboxEntry, JobBrief, JobDetail, JobPanic, LaneBrief, Link, Merge, Side};
use makepad_devhub::land::{Entry as LandEntry, St};
use makepad_devhub::lease::panel::{Answer, Row, Sample, Wait};
use makepad_devhub::lease::sched::NodeView;
use makepad_devhub::lease::{Caps, Kind, Role};

const STEP_MS: u64 = 5_000;
const SAMPLE_MS: u64 = 10_000;
const BEFORE_MS: u64 = 3 * 3_600_000;
const AFTER_MS: u64 = 3_600_000;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn f(&mut self) -> f64 {
        (self.next() % 1_000_000) as f64 / 1_000_000.0
    }
    fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[(self.next() % v.len() as u64) as usize]
    }
}

struct DemoNode {
    view: NodeView,
}

/// What one request asked for.
struct Ask {
    lane: &'static str,
    agent: &'static str,
    kind: Kind,
    cores: f64,
    ram: f64,
    gpus: u32,
    run_ms: u64,
    macos: bool,
    interactive: bool,
}

pub struct Demo {
    nodes: Vec<DemoNode>,
    /// every lease of the run, with its final times (granted/ended may lie after a given now)
    rows: Vec<Row>,
    samples: Vec<Vec<Sample>>,
    land: Vec<LandEntry>,
    buckets: Vec<Bucket>,
    start_ms: u64,
}

/// The planted trouble: a lease running far past its measure, a job paused on a panic, a
/// timed request at the head of its queue for 26 min, a holder that stopped beating, a lane
/// quiet for over an hour.
const STALLED: &str = "L269990";
const NO_BEAT: &str = "L272101";
const PAUSED: &str = "L270444";
const PAUSED_JOB: &str = "J8812";
const HEAD: &str = "L271990";
/// how long before the start the panel has watched (the self-update counts from there)
pub const WATCHED_MS: u64 = 20 * 60_000;

const LANES: [(&str, &str); 12] = [
    ("menu-icons", "MENUICONS"),
    ("table-sort", "TABLESORT"),
    ("log-filter", "LOGFILTER"),
    ("dark-theme", "DARKTHEME"),
    ("font-cache", "FONTCACHE"),
    ("undo-stack", "UNDOSTACK"),
    ("drag-ghost", "DRAGGHOST"),
    ("docs-index", "DOCSINDEX"),
    ("zip-reader", "ZIPREADER"),
    ("color-picker", "COLORPICKER"),
    ("tab-reorder", "TABREORDER"),
    ("json-speedup", "JSONSPEEDUP"),
];

fn node(name: &str, role: Role, arch: &str, os: &str, cores: u32, ram: f64, gpu: Option<f64>, up: bool) -> DemoNode {
    let caps = Caps {
        node: name.into(),
        arch: arch.into(),
        os: os.into(),
        cpu: "CPU-A".into(),
        cores,
        ram_gb: ram,
        ram_avail_gb: ram * 0.7,
        disk_free_gb: 400.0 + cores as f64 * 20.0,
        disk_total_gb: 2000.0,
        disk_floor_gb: 25.0,
        gpus: gpu.map(|v| vec![("GPU-A".to_string(), v)]).unwrap_or_default(),
        gpu_busy: gpu.map(|_| vec![(5.0, 1.5)]).unwrap_or_default(),
        cpu_pct: Some(10.0),
        role: Some(role),
        timed: role == Role::Bench,
        hub_rev: "5b2c9e14d0a7f3e8c1b6a9d4e2f0c7b3a8d5e1f6".into(),
        deploy: "5b2c9e14d0a7f3e8c1b6a9d4e2f0c7b3a8d5e1f6,0c4f8e2a6b1d9c3e7f5a2b8d4c6e0f1a3b9d7c5e".into(),
        hub_proto: makepad_devhub::lease::HUB_PROTO,
        ..Default::default()
    };
    DemoNode { view: NodeView { name: name.into(), addr: String::new(), up, dev: up, dev_set: false, caps: up.then_some(caps), ..Default::default() } }
}

impl Demo {
    pub fn new(start_ms: u64) -> Demo {
        let nodes = vec![
            node("node-a", Role::Pool, "x86_64", "linux", 16, 64.0, Some(16.0), true),
            node("node-b", Role::Pool, "x86_64", "linux", 16, 32.0, Some(16.0), true),
            node("node-c", Role::Bench, "x86_64", "linux", 16, 64.0, Some(16.0), true),
            node("node-d", Role::Bench, "x86_64", "linux", 16, 64.0, Some(16.0), true),
            node("node-e", Role::Pool, "x86_64", "windows", 16, 64.0, Some(16.0), true),
            node("node-f", Role::Pool, "x86_64", "linux", 32, 128.0, Some(32.0), true),
            node("node-g", Role::Long, "x86_64", "linux", 16, 64.0, Some(16.0), true),
            node("node-h", Role::Hot, "aarch64", "macos", 8, 16.0, None, true),
            node("node-i", Role::Bench, "aarch64", "macos", 8, 16.0, None, true),
            node("node-j", Role::Pool, "aarch64", "linux", 8, 8.0, None, false),
        ];
        let mut d = Demo { samples: vec![Vec::new(); nodes.len()], nodes, rows: Vec::new(), land: Vec::new(), buckets: Vec::new(), start_ms };
        d.run(start_ms);
        d.plant(start_ms);
        d
    }

    fn ask(rng: &mut Rng) -> Ask {
        let (lane, agent) = *rng.pick(&LANES);
        let roll = rng.f();
        let mk = |lane, agent, kind, cores, ram, gpus, min_s: u64, max_s: u64, rng: &mut Rng| Ask { lane, agent, kind, cores, ram, gpus, run_ms: (min_s + rng.next() % (max_s - min_s)) * 1000, macos: false, interactive: false };
        if roll < 0.42 {
            mk(lane, agent, Kind::Test, 4.0, 6.0, 0, 40, 700, rng)
        } else if roll < 0.60 {
            mk(lane, agent, Kind::Build, 8.0, 12.0, 0, 30, 300, rng)
        } else if roll < 0.68 {
            mk(lane, agent, Kind::Gpu, 2.0, 4.0, 1, 40, 200, rng)
        } else if roll < 0.75 {
            mk("ci", "devhub-ci", Kind::Ci, 2.0, 4.0, 0, 50, 140, rng)
        } else if roll < 0.80 {
            mk("landq-fast", "devhub-bucket", Kind::Gate, 8.0, 12.0, 0, 240, 360, rng)
        } else if roll < 0.85 {
            mk("allapps", "devhub-allapps", Kind::Long, 4.0, 8.0, 0, 400, 520, rng)
        } else if roll < 0.88 {
            mk(lane, agent, Kind::Timed, 0.0, 0.0, 0, 180, 420, rng)
        } else if roll < 0.92 {
            let mut a = mk(lane, agent, Kind::Test, 4.0, 6.0, 0, 60, 300, rng);
            a.macos = true;
            a
        } else if roll < 0.96 {
            let mut a = mk("live-W42", "", Kind::Test, 4.0, 6.0, 0, 120, 900, rng);
            a.interactive = true;
            a
        } else {
            mk("jitlegs-full-a", "devhub-jitlegs", Kind::Test, 4.0, 6.0, 0, 300, 420, rng)
        }
    }

    /// The run: requests arrive, a tiny scheduler places them (budgets, timed exclusion, the
    /// Macs only for macOS work, the lane cap), and each node is sampled every 10 s.
    fn run(&mut self, start_ms: u64) {
        let mut rng = Rng(0x5eed_d00d);
        let t0 = start_ms - BEFORE_MS;
        let t1 = start_ms + AFTER_MS;
        // queued: (row index, ask)
        let mut queue: Vec<(usize, Ask)> = Vec::new();
        // per node: running (row index, ends at)
        let mut running: Vec<Vec<(usize, u64)>> = vec![Vec::new(); self.nodes.len()];
        let mut next_id = 270_000u64;
        let mut t = t0;
        let mut next_sample = t0;
        let mut cpu_noise = vec![0.0f64; self.nodes.len()];
        while t <= t1 {
            // the day's rhythm: busier in waves
            let wave = 0.55 + 0.45 * ((t - t0) as f64 / 1_900_000.0).sin().abs();
            let arrivals = if rng.f() < 0.28 * wave { 1 + (rng.next() % 2) as usize } else { 0 };
            for _ in 0..arrivals {
                let a = Self::ask(&mut rng);
                next_id += 1 + rng.next() % 3;
                // the planted leases' ids stay theirs
                while [STALLED, NO_BEAT, PAUSED, HEAD].iter().any(|p| p[1..].parse() == Ok(next_id)) {
                    next_id += 1;
                }
                self.rows.push(Row {
                    id: format!("L{next_id}"),
                    lane: a.lane.into(),
                    agent: a.agent.into(),
                    kind: a.kind.as_str().into(),
                    state: "queued".into(),
                    asked_ms: t,
                    priority: if a.interactive { "interactive" } else { "background" }.into(),
                    live: a.interactive,
                    cost: if a.run_ms < 90_000 { "small" } else if a.run_ms < 1_200_000 { "medium" } else { "large" }.into(),
                    cost_source: format!("measured {}m{:02}s", a.run_ms / 60_000, a.run_ms / 1000 % 60),
                    measured_ms: Some(a.run_ms),
                    cores: a.cores,
                    ram_gb: a.ram,
                    gpus: a.gpus,
                    src: if matches!(a.kind, Kind::Test | Kind::Build | Kind::Gpu) { "overlay" } else { "tree" }.into(),
                    class: format!("{} makepad.jit test", a.kind.as_str()),
                    running: match a.kind {
                        Kind::Gate => "bash land_gate.sh fast".into(),
                        Kind::Long => "bash allapps_check.sh".into(),
                        Kind::Ci => "makepad ci shard".into(),
                        _ => "makepad.jit test".into(),
                    },
                    disk_mb: Some(400.0 + (rng.next() % 1400) as f64),
                    ..Default::default()
                });
                queue.push((self.rows.len() - 1, a));
            }
            // ends
            for run in running.iter_mut() {
                for &(i, end) in run.iter() {
                    if end <= t {
                        self.rows[i].ended_ms = Some(end);
                        self.rows[i].state = "ended".into();
                        self.rows[i].how = "released".into();
                    }
                }
                run.retain(|&(_, end)| end > t);
            }
            // placement, oldest first (interactive ahead)
            queue.sort_by_key(|(i, a)| (!a.interactive, self.rows[*i].asked_ms));
            // a waiting timed request holds a bench box: nothing new starts there until it runs
            let mut holds: Vec<Option<usize>> = vec![None; self.nodes.len()];
            for (i, a) in queue.iter().filter(|(_, a)| a.kind == Kind::Timed) {
                let bench = self.nodes.iter().enumerate().position(|(ni, n)| holds[ni].is_none() && n.view.up && n.view.caps.as_ref().is_some_and(|c| c.role() == Role::Bench && (c.os == "macos") == a.macos));
                if let Some(ni) = bench {
                    holds[ni] = Some(*i);
                }
            }
            let mut still = Vec::new();
            for (i, a) in queue.drain(..) {
                let lane_running = running.iter().flatten().filter(|(j, _)| self.rows[*j].lane == a.lane).count();
                let mut reason = String::new();
                let mut placed = None;
                for (ni, n) in self.nodes.iter().enumerate() {
                    let Some(c) = n.view.caps.as_ref().filter(|_| n.view.up) else { continue };
                    let mac = c.os == "macos";
                    if mac != a.macos || (a.kind == Kind::Timed && c.role() != Role::Bench) || (c.role() == Role::Hot && matches!(a.kind, Kind::Build | Kind::Gate | Kind::Long | Kind::Timed)) {
                        continue;
                    }
                    if a.gpus > 0 && c.gpus.is_empty() {
                        continue;
                    }
                    let held: Vec<&(usize, u64)> = running[ni].iter().collect();
                    if let Some(&&(j, _)) = held.iter().find(|(j, _)| self.rows[*j].kind == "timed") {
                        reason = format!("held by {} (timed of {})", self.rows[j].id, self.rows[j].lane);
                        continue;
                    }
                    match holds[ni] {
                        Some(h) if h != i => {
                            reason = format!("held for timed {}", self.rows[h].id);
                            continue;
                        }
                        None if a.kind == Kind::Timed => continue,
                        _ => {}
                    }
                    if a.kind == Kind::Timed && !held.is_empty() {
                        reason = format!("hold for timed: {} running there", held.len());
                        continue;
                    }
                    let cores: f64 = held.iter().map(|(j, _)| self.rows[*j].cores).sum();
                    let ram: f64 = held.iter().map(|(j, _)| self.rows[*j].ram_gb).sum();
                    if cores + a.cores > c.cores as f64 {
                        reason = format!("budget cores {}, in use {cores:.0}, asked {:.0}", c.cores, a.cores);
                        continue;
                    }
                    if ram + a.ram > c.ram_gb * 0.85 {
                        reason = format!("budget ram {:.1} GB, in use {ram:.1} GB, asked {:.1} GB (85% of {:.0} GB)", c.ram_gb * 0.85, a.ram, c.ram_gb);
                        continue;
                    }
                    placed = Some(ni);
                    break;
                }
                if placed.is_some() && lane_running >= 3 && !a.interactive && a.lane != "ci" {
                    placed = None;
                    reason = format!("lane cap {lane_running}/2: other lanes' leases go first");
                }
                let blocker = makepad_devhub::lease::prof::blocker(&reason);
                let r = &mut self.rows[i];
                let open = r.waits.last().is_some_and(|w| w.end_ms == 0);
                match placed {
                    Some(ni) => {
                        if open {
                            r.waits.last_mut().unwrap().end_ms = t;
                        } else if t > r.asked_ms {
                            r.waits.push(Wait { start_ms: r.asked_ms, end_ms: t, blocker: "unobserved".into() });
                        }
                        r.granted_ms = Some(t);
                        r.node = self.nodes[ni].view.name.clone();
                        r.state = "granted".into();
                        r.reason.clear();
                        running[ni].push((i, t + a.run_ms));
                    }
                    None => {
                        let reason = if reason.is_empty() { "no fitting node up".to_string() } else { reason };
                        if !open || r.waits.last().is_some_and(|w| w.blocker != blocker) {
                            if let Some(w) = r.waits.last_mut().filter(|w| w.end_ms == 0) {
                                w.end_ms = t;
                            }
                            r.waits.push(Wait { start_ms: t, end_ms: 0, blocker });
                        }
                        r.reason = reason;
                        still.push((i, a));
                    }
                }
            }
            queue = still;
            // samples
            if t >= next_sample {
                next_sample += SAMPLE_MS;
                for (ni, n) in self.nodes.iter().enumerate() {
                    let Some(c) = n.view.caps.as_ref().filter(|_| n.view.up) else { continue };
                    let held = &running[ni];
                    let cores: f64 = held.iter().map(|(j, _)| self.rows[*j].cores).sum();
                    let ram: f64 = held.iter().map(|(j, _)| self.rows[*j].ram_gb).sum();
                    let gpu = held.iter().any(|(j, _)| self.rows[*j].kind == "gpu");
                    let timed = held.iter().any(|(j, _)| self.rows[*j].kind == "timed");
                    cpu_noise[ni] = cpu_noise[ni] * 0.7 + (rng.f() - 0.5) * 9.0;
                    let busy = if timed { 0.55 } else { (cores / c.cores as f64).min(1.0) };
                    let cpu = (4.0 + busy * 88.0 + cpu_noise[ni]).clamp(1.0, 100.0);
                    self.samples[ni].push(Sample {
                        t_ms: t,
                        cpu_pct: cpu as f32,
                        load1: (cpu / 100.0 * c.cores as f64 * 1.1) as f32,
                        ram_used_gb: (3.0 + ram * 0.75 + rng.f() * 1.5).min(c.ram_gb) as f32,
                        res_cores: cores as f32,
                        res_ram_gb: ram as f32,
                        gpu_pct: if c.gpus.is_empty() { -1.0 } else if gpu { (60.0 + rng.f() * 35.0) as f32 } else { (2.0 + rng.f() * 6.0) as f32 },
                        gpu_used_gb: if c.gpus.is_empty() { 0.0 } else if gpu { (6.0 + rng.f() * 8.0) as f32 } else { 1.5 },
                    });
                }
            }
            t += STEP_MS;
        }
        // landings every few minutes, each followed by its hot bucket; fast every 10 min
        let mut id = 900;
        let mut t = t0 + 120_000;
        let mut tip = 0x7edb_4685u64;
        while t <= t1 {
            id += 1;
            tip = tip.wrapping_mul(2862933555777941757).wrapping_add(3037000493);
            let (lane, agent) = *rng.pick(&LANES);
            let sha = format!("{:016x}{:016x}{:08x}", tip, tip.rotate_left(17), tip as u32);
            let failed = rng.f() < 0.06;
            let queued_ms = t - 30_000 - rng.next() % 90_000;
            self.land.push(LandEntry {
                id,
                branch: format!("rc-{lane}"),
                cwt: String::new(),
                cbranch: String::new(),
                state: if failed { St::Failed } else { St::Landed },
                held: String::new(),
                ms: String::new(),
                cs: String::new(),
                landed: if failed { String::new() } else { format!("work {sha} main {sha}") },
                pre: String::new(),
                detail: if failed { vec!["error[E0599]: no method named `pack` found".into()] } else { Vec::new() },
                agent: agent.into(),
                proof: false,
                asked_ms: queued_ms,
                changed_ms: t,
            });
            self.buckets.push(Bucket { name: "hot".into(), ok: rng.f() > 0.04, failing: Vec::new(), tip: format!("{:08x},{:08x}", tip as u32, (tip >> 32) as u32), started_ms: t + 20_000, ended_ms: t + 75_000 });
            t += 150_000 + rng.next() % 240_000;
        }
        let mut t = t0 + 600_000;
        while t <= t1 {
            let ok = rng.f() > 0.1;
            self.buckets.push(Bucket { name: "fast".into(), ok, failing: if ok { Vec::new() } else { vec!["makepad-app-clock".into()] }, tip: "7edb4685,f095f44c".into(), started_ms: t, ended_ms: t + 300_000 });
            t += 600_000;
        }
        self.buckets.sort_by_key(|b| b.ended_ms);
    }

    /// What a master would answer to `panel <since>` at `now`.
    pub fn answer(&self, since: u64, now: u64) -> Answer {
        let rows = self
            .rows
            .iter()
            .filter(|r| r.asked_ms <= now)
            .filter(|r| r.ended_ms.is_none_or(|e| e >= since))
            .map(|r| {
                let mut r = r.clone();
                if r.ended_ms.is_some_and(|e| e > now) {
                    r.ended_ms = None;
                    r.how.clear();
                    r.state = "granted".into();
                }
                if r.granted_ms.is_some_and(|g| g > now) || r.granted_ms.is_none() {
                    r.granted_ms = None;
                    r.node.clear();
                    r.state = "queued".into();
                    r.waits.retain(|w| w.start_ms <= now);
                    if let Some(w) = r.waits.last_mut() {
                        if w.end_ms > now {
                            w.end_ms = 0;
                        }
                    }
                    if r.reason.is_empty() {
                        r.reason = r.waits.last().map_or("no fitting node up".to_string(), |w| w.blocker.clone());
                    }
                } else {
                    r.reason.clear();
                }
                if r.state == "ended" {
                    r.running.clear();
                }
                // holders beat every 2 s; the planted one stopped 7 s ago
                r.beat_ms = (r.state == "granted").then(|| if r.id == NO_BEAT { now - 7_000 } else { now - (now % 2_000) });
                // the scheduler's rule: running past twice its measure, and 5 min over it
                r.stalled = !r.live
                    && r.ended_ms.is_none()
                    && r.measured_ms.filter(|m| *m >= 60_000).zip(r.granted_ms).is_some_and(|(m, g)| now.saturating_sub(g) > (2 * m).max(m + 300_000));
                r
            })
            .collect::<Vec<Row>>();
        let mut nodes: Vec<NodeView> = self.nodes.iter().map(|n| n.view.clone()).collect();
        for (i, n) in nodes.iter_mut().enumerate() {
            n.leases = rows.iter().filter(|r| r.node == n.name && r.state != "ended").count();
            // the node's caps say what its last sample measured
            if let (Some(c), Some(s)) = (n.caps.as_mut(), self.samples[i].iter().rev().find(|s| s.t_ms <= now)) {
                c.cpu_pct = Some(s.cpu_pct as f64);
                c.load1 = s.load1 as f64;
                c.ram_avail_gb = (c.ram_gb - s.ram_used_gb as f64).max(0.0);
                if let Some(g) = c.gpu_busy.first_mut() {
                    *g = (s.gpu_pct.max(0.0) as f64, s.gpu_used_gb as f64);
                }
            }
        }
        nodes[6].updating = Some("deployment has the landing; restarts when idle".into());
        let pressure = self.nodes.iter().zip(&self.samples).map(|(n, s)| (n.view.name.clone(), s.iter().filter(|x| x.t_ms >= since && x.t_ms <= now).copied().collect())).collect();
        Answer { now_ms: now, since_ms: since, nodes, rows, pressure }
    }

    /// The landing train at `now`: the landed and failed hand-ins so far, the next one queued.
    pub fn land(&self, now: u64) -> Vec<LandEntry> {
        let mut v: Vec<LandEntry> = self.land.iter().filter(|e| e.changed_ms <= now).cloned().collect();
        if let Some(next) = self.land.iter().find(|e| e.changed_ms > now && e.asked_ms <= now) {
            let mut q = next.clone();
            q.state = St::Queued;
            q.landed.clear();
            q.detail.clear();
            q.changed_ms = q.asked_ms;
            v.push(q);
        }
        v
    }

    pub fn buckets(&self, now: u64) -> Vec<crate::model::Bucket> {
        self.buckets.iter().filter(|b| b.ended_ms <= now).cloned().collect()
    }

    pub fn inbox(&self) -> InboxCounts {
        InboxCounts { unseen: 37, err: 4, warn: 29, req: 4 }
    }

    /// The planted trouble (see [`STALLED`]); their ends lie past the run, so they hold all hour.
    fn plant(&mut self, start: u64) {
        let min = 60_000;
        let row = |id: &str, lane: &str, agent: &str, kind: &str, node: &str, asked: u64, granted: Option<u64>, measured: u64, running: &str| Row {
            id: id.into(),
            lane: lane.into(),
            agent: agent.into(),
            kind: kind.into(),
            state: if granted.is_some() { "granted" } else { "queued" }.into(),
            node: if granted.is_some() { node.into() } else { String::new() },
            asked_ms: asked,
            granted_ms: granted,
            priority: "background".into(),
            cost: "medium".into(),
            cost_source: format!("measured {}m{:02}s", measured / min, measured / 1000 % 60),
            measured_ms: Some(measured),
            cores: if kind == "timed" { 0.0 } else { 4.0 },
            ram_gb: 6.0,
            src: "overlay".into(),
            class: format!("{kind} makepad.jit test"),
            running: running.into(),
            disk_mb: Some(900.0),
            waits: granted.filter(|g| *g > asked).map(|g| vec![Wait { start_ms: asked, end_ms: g, blocker: "budget cores".into() }]).unwrap_or_default(),
            ..Default::default()
        };
        self.rows.push(row(STALLED, "json-speedup", "JSONSPEEDUP", "test", "node-a", start - 38 * min, Some(start - 37 * min), 9 * min, "makepad.jit test -p makepad-demo-json --lib merge::"));
        self.rows.push(row(NO_BEAT, "blur-pass", "BLURPASS", "build", "node-c", start - 5 * min, Some(start - 4 * min), 6 * min, "makepad check makepad-demo-table"));
        self.rows.push(row(PAUSED, "drag-ghost", "DRAGGHOST", "test", "node-d", start - 23 * min, Some(start - 22 * min), 6 * min, "makepad.jit test -p makepad-demo-table --lib rows::"));
        let reason = "held by L271950 (timed of table-sort)";
        let mut head = row(HEAD, "dark-theme", "DARKTHEME", "timed", "", start - 26 * min, None, 7 * min, "");
        head.pos = 1;
        head.reason = reason.into();
        head.waits = vec![Wait { start_ms: start - 26 * min, end_ms: 0, blocker: makepad_devhub::lease::prof::blocker(reason) }];
        self.rows.push(head);
    }

    /// The open jobs at `now`: the planted paused one, and every third running test, build or
    /// GPU lease run as a job.
    pub fn jobs(&self, now: u64) -> Vec<JobBrief> {
        let mut v = Vec::new();
        for r in self.rows.iter().filter(|r| r.granted_ms.is_some_and(|g| g <= now) && r.ended_ms.is_none_or(|e| e > now)) {
            let n: u64 = r.id.trim_start_matches('L').parse().unwrap_or(0);
            let paused = r.id == PAUSED;
            if !paused && (n % 3 != 0 || !matches!(r.kind.as_str(), "test" | "build" | "gpu")) {
                continue;
            }
            let g = r.granted_ms.unwrap_or(now);
            v.push(JobBrief {
                id: if paused { PAUSED_JOB.into() } else { format!("J{}", n / 3 % 10_000) },
                lane: r.lane.clone(),
                agent: r.agent.clone(),
                kind: r.kind.clone(),
                lease: r.id.clone(),
                node: r.node.clone(),
                state: if paused && now >= self.start_ms - 14 * 60_000 { "paused" } else { "running" }.into(),
                cmd: r.running.clone(),
                started_ms: g + 2_000,
                panic: (paused && now >= self.start_ms - 14 * 60_000).then(|| JobPanic { id: "P1".into(), message: "attempt to subtract with overflow".into(), location: "libs/demo_table/src/rows.rs:412:17".into(), at_ms: self.start_ms - 14 * 60_000 }),
                touched_ms: 0,
            });
        }
        v
    }

    /// A job's output at `now`: its last lines, the panic's if it sits on one.
    pub fn job_detail(&self, id: &str, now: u64) -> Option<JobDetail> {
        let job = self.jobs(now).into_iter().find(|j| j.id == id)?;
        let mut tail: Vec<String> = ["running 214 tests", "test rows::sort_keeps_ties_in_order ... ok", "test rows::empty_table_sorts ... ok", "test rows::wide_cells_wrap ... ok"].iter().map(|s| s.to_string()).collect();
        let mut errors = Vec::new();
        if let Some(p) = &job.panic {
            tail.push(format!("thread 'rows::sort_by_two_columns' panicked at {}:", p.location));
            tail.push(p.message.clone());
            tail.push(format!("panic {} parked: waiting for `job resume` (the box stays held)", p.id));
            errors.push(format!("panicked at {}: {}", p.location, p.message));
        } else {
            tail.push(format!("test rows::case_{} ... ok", now / 1000 % 97));
        }
        Some(JobDetail { job, errors, tail, err: String::new() })
    }

    /// The lanes' branches at `now`: each demo lane last active at its last request, a lane
    /// quiet since well before the start, one whose branch conflicts with the tip.
    pub fn lanes(&self, now: u64) -> Vec<LaneBrief> {
        let mut v: Vec<LaneBrief> = LANES
            .iter()
            .map(|(lane, _)| LaneBrief { branch: format!("rc-{lane}"), result: "merged".into(), active_ms: self.rows.iter().filter(|r| r.lane == *lane && r.asked_ms <= now).map(|r| r.asked_ms).max().unwrap_or(0) })
            .collect();
        v.push(LaneBrief { branch: "rc-sidebar-width".into(), result: "merged".into(), active_ms: self.start_ms - 95 * 60_000 });
        v.push(LaneBrief { branch: "rc-scroll-fade".into(), result: "conflict".into(), active_ms: self.start_ms - 72 * 60_000 });
        v.push(LaneBrief { branch: "rc-old-sketch".into(), result: "landed".into(), active_ms: self.start_ms - 300 * 60_000 });
        v
    }

    /// The unseen inbox at `now`, newest first.
    pub fn inbox_list(&self, now: u64) -> Vec<InboxEntry> {
        let at = |m: u64| self.start_ms - m * 60_000;
        let mut v = vec![
            InboxEntry { id: "node-e.hub.1".into(), count: 1, last_ms: at(5), line: "15:04:51Z WARN node-e sync: the WM restart waits for: screen lease L270310 of drag-ghost (a live session): close it to let the WM update".into() },
            InboxEntry { id: "master.ci.2".into(), count: 3, last_ms: at(12), line: "14:57:40Z ERR master ci 7edb4685,f095f44c: no shard ended in 30 min (19 of 38 ended)".into() },
            InboxEntry { id: "master.lease.3".into(), count: 1, last_ms: at(16), line: format!("14:53:02Z WARN master lease: test of json-speedup on node-a: running 21m against its measured 9m (test makepad.jit test): stalled? `devhub cancel {STALLED}` frees the box") },
        ];
        v.retain(|e| e.last_ms <= now);
        v
    }

    /// Everything beside the leases at `now`, as the master's other services would say it.
    pub fn side(&self, s: &mut Side, now: u64, detail: Option<&str>) {
        s.land = self.land(now);
        s.buckets = self.buckets(now);
        s.inbox = Some(self.inbox());
        s.jobs = Some(self.jobs(now));
        s.lanes = self.lanes(now);
        s.inbox_list = self.inbox_list(now);
        s.detail = detail.and_then(|id| self.job_detail(id, now));
    }

    /// The panel's model of the demo fleet at `now`, watched since [`WATCHED_MS`] before it.
    pub fn fleet(&self, now: u64) -> Fleet {
        let mut m = Merge::default();
        m.apply(self.answer(now - crate::model::KEEP_MS, now - WATCHED_MS));
        m.apply(self.answer(m.next_since(0), now));
        let mut f = m.fleet(Link::Demo);
        let mut s = Side::default();
        self.side(&mut s, now, None);
        s.onto(&mut f);
        f
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_demo_fleet_queues_without_starving() {
        let start = 1_800_000_000_000u64;
        let d = Demo::new(start);
        let mut worst = 0;
        let mut t = start - BEFORE_MS + 1_800_000;
        while t <= start + AFTER_MS {
            let a = d.answer(t - 60_000, t);
            // the planted queue head waits on purpose
            let waiting: Vec<&Row> = a.rows.iter().filter(|r| r.state == "queued" && r.id != HEAD).collect();
            worst = worst.max(waiting.len());
            assert!(waiting.iter().all(|r| t - r.asked_ms < 3_600_000), "a request starves for an hour at {t}");
            t += 600_000;
        }
        assert!(worst <= 30, "the queue grows to {worst}");
        let timed = d.rows.iter().filter(|r| r.kind == "timed" && r.id != HEAD);
        assert!(timed.clone().count() > 3 && timed.clone().all(|r| r.granted_ms.is_some() || r.asked_ms > start), "timed work runs");
        assert!(d.rows.iter().any(|r| r.granted_ms.is_some_and(|g| g - r.asked_ms >= 600_000)), "some waits are long enough to stand out");
    }
}
