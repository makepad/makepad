//! What needs a look: the fleet's stuck things, worst first. A job paused on a parked panic
//! (its box stays held), a lease whose holder stopped beating (it expires at 10 s), a lease the
//! scheduler said runs past twice its measured run, a queue
//! head waiting over 10 min or overtaken by later requests, an unreachable hub, a self-update
//! that does not finish, a failed post-land bucket or landing, a lane with nothing of its own
//! for an hour, and the inbox's recent errors and warnings that name none of those. Each says
//! what it is, how long it has been so, which lease or node it is about, and the devhub
//! commands that act on it (the panel only reads: it shows them for copying).

use crate::fmt::dur;
use crate::model::{Fleet, LONG_WAIT_MS};
use makepad_devhub::land::St;
use makepad_devhub::lease::PREBUILD_LANE_PREFIX;

/// A self-update this long is stuck.
pub const UPDATE_STALL_MS: u64 = 15 * 60_000;
/// A lane with no commit, lease, job or landing this long is quiet.
pub const QUIET_LANE_MS: u64 = 60 * 60_000;
/// Inbox entries newer than this count.
pub const INBOX_RECENT_MS: u64 = 30 * 60_000;
/// A waiting request this long that later ones overtook.
pub const OVERTAKEN_MS: u64 = 5 * 60_000;
/// A holder beats every 2 s; this long without one, its lease is on its way to expiring.
pub const STALE_BEAT_MS: u64 = 5_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Sev {
    Red,
    Amber,
    Info,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum What {
    Paused,
    Beat,
    Stalled,
    QueueHead,
    Overtaken,
    Down,
    Updating,
    Bucket,
    Landing,
    Quiet,
    Inbox,
    Held,
    Restart,
}

impl What {
    pub fn tag(self) -> &'static str {
        match self {
            What::Paused => "PAUSED",
            What::Beat => "NO BEAT",
            What::Stalled => "STALLED",
            What::QueueHead => "WAITING",
            What::Overtaken => "OVERTAKEN",
            What::Down => "DOWN",
            What::Updating => "UPDATE",
            What::Bucket => "BUCKET",
            What::Landing => "LANDING",
            What::Quiet => "QUIET",
            What::Inbox => "INBOX",
            What::Held => "HELD",
            What::Restart => "RESTART",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Need {
    pub sev: Sev,
    pub what: What,
    /// one line: who and where
    pub title: String,
    /// one line: what is wrong, in numbers
    pub detail: String,
    pub lease: Option<String>,
    pub node: Option<String>,
    pub job: Option<String>,
    /// how long it has been so (within a severity, the longest first)
    pub for_ms: u64,
    /// the devhub commands that act on it
    pub act: Vec<String>,
}

impl Need {
    /// A stable key across answers (selection survives refreshes).
    pub fn key(&self) -> String {
        format!("{}:{}", self.what.tag(), self.lease.as_deref().or(self.job.as_deref()).or(self.node.as_deref()).unwrap_or(&self.title))
    }
}

fn need(sev: Sev, what: What, title: String, detail: String, for_ms: u64) -> Need {
    Need { sev, what, title, detail, lease: None, node: None, job: None, for_ms, act: Vec::new() }
}

const DH: &str = "makepad devhub";

/// Everything that needs a look in `f`, worst first.
pub fn needs(f: &Fleet) -> Vec<Need> {
    let now = f.now_ms;
    let mut v: Vec<Need> = Vec::new();
    // jobs paused on a parked panic hold their box until someone acts (or 10 min pass)
    for j in f.jobs.iter().flatten().filter(|j| j.state == "paused") {
        let at = j.panic.as_ref().map_or(j.started_ms, |p| p.at_ms);
        let what = j.panic.as_ref().map_or("a parked panic".to_string(), |p| format!("{} at {}", p.message, p.location));
        let pid = j.panic.as_ref().map(|p| p.id.as_str()).filter(|p| !p.is_empty()).unwrap_or("P1");
        let mut n = need(Sev::Red, What::Paused, format!("{} {} of {} on {}", j.id, j.kind, j.lane, j.node), format!("paused on {what}"), now.saturating_sub(at));
        n.lease = (!j.lease.is_empty()).then(|| j.lease.clone());
        n.node = (!j.node.is_empty()).then(|| j.node.clone());
        n.job = Some(j.id.clone());
        n.act = vec![format!("{DH} job inspect {} stacks", j.id), format!("{DH} job resume {} {pid} retry", j.id), format!("{DH} job release {}", j.id)];
        v.push(n);
    }
    let paused: Vec<String> = v.iter().filter_map(|n| n.lease.clone()).collect();
    // leases whose holder stopped beating: they expire at 10 s, freeing the box mid-run
    for r in f.rows.iter().filter(|r| r.ended_ms.is_none() && !paused.contains(&r.id)) {
        let Some(age) = r.beat_ms.map(|b| now.saturating_sub(b)).filter(|a| *a >= STALE_BEAT_MS) else { continue };
        let mut n = need(Sev::Red, What::Beat, format!("{} {} of {} on {}", r.id, r.kind, r.lane, r.node), format!("no beat for {} (it expires at 10 s)", dur(age)), age);
        n.lease = Some(r.id.clone());
        n.node = (!r.node.is_empty()).then(|| r.node.clone());
        n.act = vec![format!("{DH} top {}", r.node), format!("{DH} cancel {}", r.id)];
        v.push(n);
    }
    // leases the scheduler said run past twice their measured run
    for r in f.rows.iter().filter(|r| r.stalled && r.ended_ms.is_none() && !r.live && !paused.contains(&r.id)) {
        let ran = now.saturating_sub(r.granted_ms.unwrap_or(now));
        let measured = r.measured_ms.map_or("its measured run".to_string(), |m| format!("its measured {}", dur(m)));
        let mut n = need(Sev::Amber, What::Stalled, format!("{} {} of {} on {}", r.id, r.kind, r.lane, r.node), format!("running {} against {measured}", dur(ran)), ran);
        n.lease = Some(r.id.clone());
        n.node = Some(r.node.clone());
        let job = f.job_of(&r.id).map(|j| j.id.clone());
        if let Some(j) = &job {
            n.act.push(format!("{DH} job result {j} --tail"));
        }
        n.act.push(format!("{DH} cancel {}", r.id));
        n.job = job;
        v.push(n);
    }
    // the queue: heads waiting over 10 min, and requests later ones overtook
    for r in f.rows.iter().filter(|r| r.state == "queued" && !r.lane.starts_with(PREBUILD_LANE_PREFIX)) {
        let waited = f.wait_ms(r);
        let overtook = f
            .rows
            .iter()
            .filter(|o| o.kind == r.kind && o.priority == r.priority && o.asked_ms > r.asked_ms && o.granted_ms.is_some_and(|g| g >= r.asked_ms))
            .count();
        let (what, sev) = if r.pos <= 1 && waited >= LONG_WAIT_MS {
            (What::QueueHead, if waited >= 3 * LONG_WAIT_MS { Sev::Red } else { Sev::Amber })
        } else if overtook >= 2 && waited >= OVERTAKEN_MS {
            (What::Overtaken, Sev::Amber)
        } else {
            continue;
        };
        let mut detail = format!("waiting {}", dur(waited));
        if overtook > 0 {
            detail.push_str(&format!(", {overtook} later requests went first"));
        }
        if !r.reason.is_empty() {
            detail.push_str(&format!(": {}", r.reason));
        }
        let mut n = need(sev, what, format!("{} {} of {} (#{} in its queue)", r.id, r.kind, r.lane, r.pos.max(1)), detail, waited);
        n.lease = Some(r.id.clone());
        n.act = vec![format!("{DH} why-queued {}", r.id), format!("{DH} job bump {}", r.id)];
        v.push(n);
    }
    // hubs: unreachable, or a self-update that does not finish
    for nv in &f.nodes {
        if !nv.up {
            let mut n = need(Sev::Amber, What::Down, format!("{} unreachable", nv.name), if nv.leases > 0 { format!("down with {} leases on it", nv.leases) } else { "its hub does not answer".to_string() }, 0);
            if nv.leases > 0 {
                n.sev = Sev::Red;
            }
            n.node = Some(nv.name.clone());
            n.act = vec![format!("{DH} hubs"), format!("{DH} nodes")];
            v.push(n);
        } else if let (Some(why), Some(since)) = (nv.updating.as_ref(), f.updating_since.get(&nv.name)) {
            let long = now.saturating_sub(*since);
            if long >= UPDATE_STALL_MS {
                let mut n = need(Sev::Amber, What::Updating, format!("{} self-update for {}", nv.name, dur(long)), why.clone(), long);
                n.node = Some(nv.name.clone());
                n.act = vec![format!("{DH} hubs"), format!("{DH} top {}", nv.name)];
                v.push(n);
            }
        }
    }
    // the last run of each bucket, failed
    let mut last: Vec<&crate::model::Bucket> = Vec::new();
    for b in &f.buckets {
        match last.iter_mut().find(|x| x.name == b.name) {
            Some(x) if x.ended_ms <= b.ended_ms => *x = b,
            Some(_) => {}
            None => last.push(b),
        }
    }
    for b in last.iter().filter(|b| !b.ok) {
        let failing = if b.failing.is_empty() { String::new() } else { format!(": {}", b.failing.join(", ")) };
        let mut n = need(Sev::Red, What::Bucket, format!("bucket {} failed at {}", b.name, b.tip), format!("ended {} ago{failing}", dur(now.saturating_sub(b.ended_ms))), now.saturating_sub(b.ended_ms));
        n.act = vec![format!("{DH} bucket status")];
        v.push(n);
    }
    // landings that failed or conflict and were not handed in again
    for e in f.land.iter().filter(|e| matches!(e.state, St::Failed | St::Conflict)) {
        if f.land.iter().any(|x| x.branch == e.branch && x.id > e.id) {
            continue;
        }
        let first = e.detail.first().cloned().unwrap_or_default();
        let mut n = need(Sev::Amber, What::Landing, format!("#{} {} {}", e.id, e.label(), e.state.as_str()), first, now.saturating_sub(e.changed_ms));
        n.act = vec![format!("{DH} land status {}", e.branch), format!("{DH} land retry {}", e.branch)];
        v.push(n);
    }
    // lanes with nothing of their own for an hour (no commit, lease, job or landing)
    for l in f.lanes.iter().filter(|l| !matches!(l.result.as_str(), "landed" | "dormant" | "")) {
        let lane = l.branch.strip_prefix("rc-").unwrap_or(&l.branch);
        let lease = f.rows.iter().filter(|r| r.lane == lane).map(|r| r.ended_ms.unwrap_or(now).max(r.asked_ms)).max().unwrap_or(0);
        let landed = f.land.iter().filter(|e| e.branch == l.branch).map(|e| e.changed_ms).max().unwrap_or(0);
        let active = l.active_ms.max(lease).max(landed);
        let quiet = now.saturating_sub(active);
        if active == 0 || quiet < QUIET_LANE_MS {
            continue;
        }
        let sev = if matches!(l.result.as_str(), "conflict" | "failed") { Sev::Amber } else { Sev::Info };
        let mut n = need(sev, What::Quiet, format!("lane {lane} quiet for {}", dur(quiet)), format!("no commit, lease, job or landing; its branch: {}", l.result), quiet);
        n.act = vec![format!("{DH} lane list"), format!("{DH} land status {}", l.branch)];
        v.push(n);
    }
    // a show held at its pair while the checkout's tip is not proven for its app: its row says
    // why, once, and goes when it takes the tip (never a popup)
    for x in f.shows.iter().filter(|x| x.active() && !x.held.is_empty()) {
        let app = x.app.strip_prefix("makepad-app-").or_else(|| x.app.strip_prefix("makepad-")).unwrap_or(&x.app);
        let mut n = need(Sev::Info, What::Held, format!("show {app} on {}", x.device), x.held.clone(), 0);
        n.act = vec![format!("{DH} shows")];
        v.push(n);
    }
    // a show whose JIT holds landed edits for the person's restart (devhub never restarts it):
    // its row says since which sha and what waits, once
    for x in f.shows.iter().filter(|x| x.active() && !x.restart.is_empty()) {
        let app = x.app.strip_prefix("makepad-app-").or_else(|| x.app.strip_prefix("makepad-")).unwrap_or(&x.app);
        let mut n = need(Sev::Info, What::Restart, format!("show {app} on {}", x.device), x.restart.clone(), 0);
        n.act = vec![format!("{DH} shows")];
        v.push(n);
    }
    // the inbox's recent errors and warnings that name nothing above
    let named: Vec<String> = v.iter().filter_map(|n| n.lease.clone()).collect();
    for e in f.inbox_list.iter().filter(|e| now.saturating_sub(e.last_ms) <= INBOX_RECENT_MS) {
        let sev = match e.sev() {
            "ERR" => Sev::Red,
            "WARN" => Sev::Amber,
            _ => continue,
        };
        if e.line.contains(" hitch ") || named.iter().any(|id| e.line.contains(id.as_str())) {
            continue;
        }
        let text = e.line.splitn(3, ' ').nth(2).unwrap_or(&e.line).to_string();
        let (head, rest) = text.split_once(": ").unwrap_or((&text, ""));
        let count = if e.count > 1 { format!(" (\u{d7}{})", e.count) } else { String::new() };
        let mut n = need(sev, What::Inbox, format!("{head}{count}"), rest.to_string(), now.saturating_sub(e.last_ms));
        n.node = head.split_whitespace().next().filter(|w| f.node(w).is_some()).map(str::to_string);
        n.act = vec![format!("{DH} show {}", e.id), format!("{DH} seen {}", e.id)];
        v.push(n);
    }
    v.sort_by(|a, b| a.sev.cmp(&b.sev).then(b.for_ms.cmp(&a.for_ms)));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The demo fleet's planted trouble comes out worst first, each with what to do.
    #[test]
    fn the_demo_fleet_needs_a_look() {
        let start = 1_800_000_000_000;
        let d = crate::demo::Demo::new(start);
        let f = d.fleet(start);
        let v = needs(&f);
        let kinds: Vec<What> = v.iter().map(|n| n.what).collect();
        for w in [What::Paused, What::Beat, What::Stalled, What::QueueHead, What::Down, What::Updating, What::Quiet] {
            assert!(kinds.contains(&w), "{w:?} in {kinds:?}");
        }
        assert!(v.windows(2).all(|p| p[0].sev <= p[1].sev), "worst first");
        assert_eq!(v[0].what, What::Paused, "a paused job first");
        assert!(v.iter().all(|n| !n.act.is_empty() && n.act.iter().all(|a| a.starts_with(DH))), "each says what to run");
        let stalled = v.iter().find(|n| n.what == What::Stalled).unwrap();
        assert!(stalled.lease.as_deref().and_then(|l| f.row(l)).is_some_and(|r| r.ended_ms.is_none()), "a stall names its running lease");
    }

    /// A show held at its pair says why in its row, a quiet one; one that took the tip or ended says nothing.
    #[test]
    fn a_held_show_says_why_in_its_row() {
        use makepad_devhub::state::Show;
        let held = "held at aaaaaaaaaa,ffffffffff: tip bbbbbbbbbb,ffffffffff waits for the fast bucket (its run of it since 22:10)";
        let show = |id: &str, app: &str, state: &str, held: &str| Show { id: id.into(), app: app.into(), device: "mac".into(), state: state.into(), held: held.into(), ..Default::default() };
        let f = Fleet { shows: vec![show("V1", "makepad-app-synth", "running", held), show("V2", "makepad-app-notes", "running", ""), show("V3", "makepad-app-paint", "closed", held)], ..Default::default() };
        let v: Vec<Need> = needs(&f).into_iter().filter(|n| n.what == What::Held).collect();
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!((v[0].sev, v[0].title.as_str(), v[0].detail.as_str()), (Sev::Info, "show synth on mac", held));
    }

    /// A show whose JIT holds landed edits for a restart says so in its row, a quiet one.
    #[test]
    fn a_show_with_a_restart_pending_says_so_in_its_row() {
        use makepad_devhub::state::Show;
        let restart = "restart pending since b518cd2ffb: 2 landed edits not running (first: 1 files changed: band.rs: the type alias `Found` changed)";
        let show = |id: &str, state: &str, restart: &str| Show { id: id.into(), app: "makepad-app-synth".into(), device: "boxa".into(), state: state.into(), restart: restart.into(), ..Default::default() };
        let f = Fleet { shows: vec![show("V1", "running", restart), show("V2", "running", ""), show("V3", "closed", restart)], ..Default::default() };
        let v: Vec<Need> = needs(&f).into_iter().filter(|n| n.what == What::Restart).collect();
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!((v[0].sev, v[0].title.as_str(), v[0].detail.as_str()), (Sev::Info, "show synth on boxa", restart));
    }
}
