//! The master at a glance: what the Mac's menu-bar item shows. One in-memory read of the master
//! (nodes, leases, the landing train, the verifier, the lanes, the recent feed): no git, no files,
//! no network, so the app takes it on its UI timer. The text changes only when the state does (ages
//! are whole minutes), so the menu is rebuilt rarely and nothing notifies.

use crate::master::Master;
use crate::proto::Sev;
use crate::tray_glyph::Glyph;

/// Recent events the menu lists.
pub const EVENTS: usize = 4;
/// A menu line's longest text, characters.
const LINE: usize = 72;

/// The verifier's newest word.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Verify {
    /// no verifier here (a master without the checkout support)
    #[default]
    None,
    Off,
    /// the newest verified pair, minutes ago (no failure since)
    Ok { pair: String, min: u64 },
    /// the newest failed pair and its blame (newer than the newest verified)
    Fail { pair: String, why: String },
    /// nothing verified yet
    Never,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Glance {
    /// the fleet identity is here (without it the master sees this Mac only)
    pub fleet: bool,
    pub nodes_up: usize,
    pub nodes: usize,
    pub leases_running: usize,
    pub leases_waiting: usize,
    pub land: crate::land::Counts,
    /// why landing is paused
    pub land_paused: Option<String>,
    pub verify: Verify,
    /// the pair a verify run tests now
    pub verifying: Option<String>,
    pub lanes: usize,
    /// unseen inbox entries, and how many of them are errors
    pub unseen: usize,
    pub unseen_err: usize,
    /// the newest events, newest first
    pub events: Vec<String>,
}

fn clip(s: &str) -> String {
    let s = s.lines().next().unwrap_or("").trim();
    if s.chars().count() <= LINE {
        return s.to_string();
    }
    format!("{}\u{2026}", s.chars().take(LINE - 1).collect::<String>())
}

/// Whole minutes since `ms`.
fn minutes(ms: u64) -> u64 {
    crate::proto::wall_ms().saturating_sub(ms) / 60_000
}

fn ago(min: u64) -> String {
    match min {
        0 => "just now".into(),
        1..=119 => format!("{min} min ago"),
        120..=2879 => format!("{} h ago", min / 60),
        _ => format!("{} d ago", min / 1440),
    }
}

impl Glance {
    /// The master's state now.
    pub fn collect(m: &Master, fleet: bool) -> Glance {
        let nodes = m.leases.nodes();
        let leases = m.leases.view();
        use crate::lease::sched::State;
        let mut g = Glance {
            fleet,
            nodes_up: nodes.iter().filter(|n| n.up).count(),
            nodes: nodes.len(),
            leases_running: leases.iter().filter(|l| matches!(l.state, State::Starting | State::Granted)).count(),
            leases_waiting: leases.iter().filter(|l| l.state == State::Queued).count(),
            land: m.land.counts(),
            land_paused: m.land.paused(),
            ..Default::default()
        };
        {
            let inbox = m.inbox.lock().unwrap();
            g.unseen = inbox.unseen();
            g.unseen_err = inbox.entries.iter().filter(|e| !e.seen && e.record.sev == Sev::Err).count();
        }
        g.events = m.recent.lock().unwrap().iter().rev().filter(|r| r.kind != "hub" && r.app.is_empty()).take(EVENTS).map(|r| clip(&r.line)).collect();
        #[cfg(feature = "checkout")]
        if let Some(ctx) = crate::orch::ctx() {
            ctx.store.read(|d| {
                let v = &d.verify;
                let ok = v.verified.last();
                let fail = v.failures.last().filter(|f| ok.is_none_or(|t| f.at_ms > t.at_ms));
                g.verify = match (v.on, ok, fail) {
                    (_, _, Some(f)) => Verify::Fail {
                        pair: f.pair.short(),
                        why: f.blame.as_ref().map_or(f.unresolved.clone(), |b| format!("first bad {}:{} {}", b.repo, crate::state::short(&b.sha), b.subject)),
                    },
                    (false, _, None) => Verify::Off,
                    (true, Some(t), None) => Verify::Ok { pair: t.pair.short(), min: minutes(t.at_ms) },
                    (true, None, None) => Verify::Never,
                };
                g.verifying = v.run.as_ref().map(|r| r.pair.short());
                g.lanes = d.lanes.iter().filter(|l| l.state == "live").count();
            });
        }
        g
    }

    /// The menu-bar glyph: off without the fleet, attention on a failure, busy while work runs.
    pub fn glyph(&self) -> Glyph {
        if !self.fleet {
            Glyph::Off
        } else if self.land.failed > 0 || matches!(self.verify, Verify::Fail { .. }) || self.unseen_err > 0 {
            Glyph::Attention
        } else if self.land.gating > 0 || self.verifying.is_some() || self.leases_running > 0 {
            Glyph::Busy
        } else {
            Glyph::Quiet
        }
    }

    /// One word for the glyph's state (the tooltip's).
    pub fn word(&self) -> &'static str {
        match self.glyph() {
            Glyph::Off => "no fleet identity",
            Glyph::Attention => "needs a look",
            Glyph::Busy => "busy",
            Glyph::Quiet => "idle",
        }
    }

    /// The menu's status lines (the events go below them).
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        out.push(format!(
            "Nodes {}/{} up \u{b7} leases {} running, {} waiting{}",
            self.nodes_up,
            self.nodes,
            self.leases_running,
            self.leases_waiting,
            if self.fleet { "" } else { " \u{b7} no fleet identity" }
        ));
        let l = &self.land;
        let mut land = Vec::new();
        for (n, w) in [(l.queued, "queued"), (l.gating, "gating"), (l.failed, "failed"), (l.held, "held")] {
            if n > 0 {
                land.push(format!("{n} {w}"));
            }
        }
        let mut s = format!("Land: {}", if land.is_empty() { "empty".into() } else { land.join(" \u{b7} ") });
        if let Some(p) = &self.land_paused {
            s += &format!(" \u{b7} PAUSED ({p})");
        }
        out.push(clip(&s));
        let v = match &self.verify {
            Verify::None => None,
            Verify::Off => Some("Verify: off".to_string()),
            Verify::Never => Some("Verify: nothing verified yet".to_string()),
            Verify::Ok { pair, min } => Some(format!("Verify: OK {pair} {}", ago(*min))),
            Verify::Fail { pair, why } => Some(format!("Verify: FAIL {pair}: {why}")),
        };
        if let Some(mut v) = v {
            if let Some(p) = &self.verifying {
                v += &format!(" \u{b7} running {p}");
            }
            out.push(clip(&v));
        }
        out.push(format!("Lanes: {}{}", self.lanes, if self.unseen > 0 { format!(" \u{b7} inbox {} new", self.unseen) } else { String::new() }));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_glyph_follows_failure_then_work_then_idle() {
        let mut g = Glance { fleet: true, ..Default::default() };
        assert_eq!(g.glyph(), Glyph::Quiet);
        g.leases_running = 2;
        assert_eq!(g.glyph(), Glyph::Busy);
        g.verify = Verify::Fail { pair: "a,b".into(), why: "first bad work:abc x".into() };
        assert_eq!(g.glyph(), Glyph::Attention);
        g.verify = Verify::Ok { pair: "a,b".into(), min: 3 };
        g.land.failed = 1;
        assert_eq!(g.glyph(), Glyph::Attention);
        g.fleet = false;
        assert_eq!(g.glyph(), Glyph::Off);
    }

    #[test]
    fn the_lines_say_nodes_leases_land_verify_lanes() {
        let g = Glance {
            fleet: true,
            nodes_up: 6,
            nodes: 7,
            leases_running: 3,
            leases_waiting: 1,
            land: crate::land::Counts { queued: 2, gating: 1, failed: 0, held: 0 },
            land_paused: Some("user review".into()),
            verify: Verify::Ok { pair: "aaaa,bbbb".into(), min: 12 },
            verifying: Some("cccc,dddd".into()),
            lanes: 9,
            unseen: 0,
            unseen_err: 0,
            events: vec![],
        };
        assert_eq!(
            g.lines(),
            vec![
                "Nodes 6/7 up \u{b7} leases 3 running, 1 waiting".to_string(),
                "Land: 2 queued \u{b7} 1 gating \u{b7} PAUSED (user review)".into(),
                "Verify: OK aaaa,bbbb 12 min ago \u{b7} running cccc,dddd".into(),
                "Lanes: 9".into(),
            ]
        );
        assert_eq!(Glance { fleet: true, ..Default::default() }.lines()[1], "Land: empty");
        assert_eq!(clip(&"x".repeat(100)).chars().count(), LINE);
    }
}
