//! Which AI hub models light up in the panel, and how much. A pure state
//! machine over the hub's snapshots and a clock (seconds): a model with a
//! running job glows fully; when its last job ends the glow fades over
//! [`FADE`] seconds so a quick request is still seen; a model that finished
//! something keeps a faint "used Ns ago" note. No drawing and no clock of its
//! own, so the mapping is unit-tested.

use crate::hub_control::{Activity, HubSnapshot, Request};

/// How long a finished job's glow takes to fade.
pub const FADE: f64 = 1.5;

#[derive(Clone, Debug, Default)]
struct Model {
    id: String,
    /// The jobs running now: (app, tokens/s, progress).
    jobs: Vec<(String, Option<f32>, Option<f32>)>,
    /// When its last job stopped (or a finished request was first seen).
    ended: Option<f64>,
    /// Used before this tracker saw it (the hub's `recent` has no times).
    used_earlier: bool,
}

/// What a row draws.
#[derive(Clone, Debug, PartialEq)]
pub struct RowGlow {
    /// 0 = none, 1 = full: working, or fading out after the work.
    pub strength: f32,
    pub working: bool,
    /// The live line for a working row ("translator - 31 tok/s").
    pub live: Option<String>,
    /// The faint note for a used, resting row ("used 20s ago").
    pub used: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct GlowTracker {
    models: Vec<Model>,
    recent: Vec<Request>,
    seen: bool,
    /// The "used" notes last shown, to know when a redraw is due.
    notes: Vec<Option<String>>,
}

impl GlowTracker {
    fn model(&mut self, id: &str) -> &mut Model {
        if let Some(i) = self.models.iter().position(|m| m.id == id) {
            return &mut self.models[i];
        }
        self.models.push(Model { id: id.to_string(), ..Default::default() });
        self.models.last_mut().unwrap()
    }

    /// Read a snapshot taken at `now`.
    pub fn update(&mut self, snap: &HubSnapshot, now: f64) {
        // Requests that finished since the last snapshot: `recent` is newest first, so
        // those are the entries ahead of the previous newest one.
        let fresh = match self.recent.first() {
            _ if !self.seen => 0,
            Some(prev) => snap.recent.iter().position(|r| r == prev).unwrap_or(snap.recent.len()),
            None => snap.recent.len(),
        };
        if !self.seen {
            for r in &snap.recent {
                self.model(&r.model).used_earlier = true;
            }
        }
        for r in snap.recent.iter().take(fresh) {
            let m = self.model(&r.model);
            m.used_earlier = false;
            // A request that began and ended between two snapshots: its fade starts now,
            // unless the end of its job was already seen a moment ago.
            if m.jobs.is_empty() && m.ended.is_none_or(|e| now - e > 2.0) {
                m.ended = Some(now);
            }
        }
        // Jobs now.
        for a in &snap.activity {
            self.model(&a.model);
        }
        for m in &mut self.models {
            let had = !m.jobs.is_empty();
            m.jobs = snap.activity.iter().filter(|a| a.model == m.id).map(job).collect();
            if had && m.jobs.is_empty() {
                m.ended = Some(now);
            }
        }
        self.recent = snap.recent.clone();
        self.seen = true;
    }

    /// The glow of one model at `now`.
    pub fn row(&self, id: &str, now: f64) -> RowGlow {
        let none = RowGlow { strength: 0.0, working: false, live: None, used: None };
        let Some(m) = self.models.iter().find(|m| m.id == id) else { return none };
        if !m.jobs.is_empty() {
            return RowGlow { strength: 1.0, working: true, live: Some(live_line(&m.jobs)), used: None };
        }
        let since = m.ended.map(|e| (now - e).max(0.0));
        let strength = since.map_or(0.0, |s| (1.0 - s / FADE).clamp(0.0, 1.0) as f32);
        let used = match since {
            Some(s) => Some(format!("used {}", ago(s))),
            None if m.used_earlier => Some("used earlier".to_string()),
            None => None,
        };
        RowGlow { strength, working: false, live: None, used }
    }

    /// Does anything need animation frames now (a job runs or a fade is running)?
    pub fn animating(&self, now: f64) -> bool {
        self.models.iter().any(|m| !m.jobs.is_empty() || m.ended.is_some_and(|e| now - e < FADE))
    }

    /// True once when a "used Ns ago" note changed since the last call: the
    /// panel redraws then, so the notes stay right without idle redraws.
    pub fn notes_changed(&mut self, now: f64) -> bool {
        let notes: Vec<Option<String>> = self.models.iter().map(|m| self.row(&m.id, now).used).collect();
        let changed = notes != self.notes;
        self.notes = notes;
        changed
    }
}

fn job(a: &Activity) -> (String, Option<f32>, Option<f32>) {
    (a.app.clone(), a.tokens_per_s, a.progress)
}

fn live_line(jobs: &[(String, Option<f32>, Option<f32>)]) -> String {
    let (app, tok, progress) = &jobs[0];
    let mut s = if app.is_empty() { "working".to_string() } else { app.clone() };
    if let Some(t) = tok {
        s.push_str(&format!(" \u{b7} {:.0} tok/s", t));
    } else if let Some(p) = progress {
        s.push_str(&format!(" \u{b7} {:.0}%", p * 100.0));
    }
    if jobs.len() > 1 {
        s.push_str(&format!(" +{}", jobs.len() - 1));
    }
    s
}

/// "just now", "20s ago", "3m ago", "2h ago": seconds in steps of five so the
/// note changes rarely.
pub fn ago(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s < 5 {
        "just now".into()
    } else if s < 60 {
        format!("{}s ago", s / 5 * 5)
    } else if s < 3600 {
        format!("{}m ago", s / 60)
    } else {
        format!("{}h ago", s / 3600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(model: &str, app: &str, tok: Option<f32>) -> Activity {
        Activity { model: model.into(), app: app.into(), tokens_per_s: tok, progress: None, stage: String::new() }
    }
    fn req(model: &str) -> Request {
        Request { model: model.into(), app: "x".into(), millis: 100, ok: true }
    }
    fn snap(activity: Vec<Activity>, recent: Vec<Request>) -> HubSnapshot {
        HubSnapshot { available: true, activity, recent, ..Default::default() }
    }

    #[test]
    fn a_working_model_glows_with_its_app_and_rate() {
        let mut t = GlowTracker::default();
        t.update(&snap(vec![act("chat", "translator", Some(31.2))], vec![]), 10.0);
        let r = t.row("chat", 10.5);
        assert!(r.working && r.strength == 1.0);
        assert_eq!(r.live.as_deref(), Some("translator \u{b7} 31 tok/s"));
        assert_eq!(t.row("image", 10.5).strength, 0.0);
        assert!(t.animating(10.5));
    }

    #[test]
    fn a_finished_job_fades_over_one_and_a_half_seconds() {
        let mut t = GlowTracker::default();
        t.update(&snap(vec![act("chat", "a", None)], vec![]), 10.0);
        t.update(&snap(vec![], vec![req("chat")]), 11.0);
        assert!((t.row("chat", 11.0).strength - 1.0).abs() < 1e-6);
        assert!((t.row("chat", 11.75).strength - 0.5).abs() < 1e-6);
        assert_eq!(t.row("chat", 12.6).strength, 0.0);
        assert!(!t.row("chat", 11.5).working);
        assert!(t.animating(11.5));
        assert!(!t.animating(12.6), "no animation frames once the fade is over");
        assert_eq!(t.row("chat", 31.0).used.as_deref(), Some("used 20s ago"));
    }

    #[test]
    fn a_request_between_two_snapshots_still_shows() {
        let mut t = GlowTracker::default();
        t.update(&snap(vec![], vec![req("vision")]), 1.0);
        assert_eq!(t.row("vision", 1.0).strength, 0.0, "history from before is not a glow");
        assert_eq!(t.row("vision", 1.0).used.as_deref(), Some("used earlier"));
        t.update(&snap(vec![], vec![req("chat"), req("vision")]), 5.0);
        assert!(t.row("chat", 5.2).strength > 0.8);
        assert_eq!(t.row("chat", 5.2).used.as_deref(), Some("used just now"));
        assert_eq!(t.row("vision", 5.2).used.as_deref(), Some("used earlier"));
    }

    #[test]
    fn the_recent_entry_of_a_job_just_seen_ending_does_not_restart_the_fade() {
        let mut t = GlowTracker::default();
        t.update(&snap(vec![act("chat", "a", None)], vec![]), 10.0);
        t.update(&snap(vec![], vec![]), 11.0);
        t.update(&snap(vec![], vec![req("chat")]), 12.0);
        assert!(t.row("chat", 12.0).strength < 0.4, "the fade still runs from the end seen at 11 s");
    }

    #[test]
    fn two_jobs_on_one_model_and_progress() {
        let mut t = GlowTracker::default();
        let mut a = act("image", "paint", None);
        a.progress = Some(0.4);
        t.update(&snap(vec![a, act("image", "notes", None)], vec![]), 1.0);
        assert_eq!(t.row("image", 1.0).live.as_deref(), Some("paint \u{b7} 40% +1"));
    }

    #[test]
    fn notes_change_in_five_second_steps() {
        let mut t = GlowTracker::default();
        t.update(&snap(vec![act("chat", "a", None)], vec![]), 0.0);
        t.update(&snap(vec![], vec![req("chat")]), 1.0);
        assert!(t.notes_changed(1.0));
        assert!(!t.notes_changed(1.5));
        assert!(t.notes_changed(6.0));
        assert!(!t.notes_changed(8.0));
        assert_eq!(ago(59.0), "55s ago");
        assert_eq!(ago(125.0), "2m ago");
        assert_eq!(ago(7300.0), "2h ago");
    }
}
