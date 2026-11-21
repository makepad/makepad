//! A box a person uses through an app of ours that runs outside devhub: `held: Fab in use by a
//! person via an XR headset`. The master then puts no work there that does not name the box, as
//! beside a person's whole interactive lease: their frames share its CPU and GPU (an unpinned
//! Metal render suite ran on a Mac beside the user's own Fab session, 11-13).
//!
//! A use is the app's own say (`makepad_platform::restart::set_in_use`): an XR headset streaming
//! its view. An app merely open holds nothing: a WM-hosted app on a pool box, opened by whom
//! nobody knows, holds nothing by being there. Nor does a box-wide sign: input at the box or a
//! Desk Link viewer showing it never keeps work off a box (only its dev switch takes it out of
//! the pool, the user, 11-18).
//!
//! The hold ends when its app exits, or [`GRACE_MS`] after the use ended.

use super::HandHeld;

/// How long a hold outlasts the use it was for while its app runs: a person who takes the
/// headset off for a moment, or looks at another tab, comes back to the box as they left it.
pub const GRACE_MS: u64 = 2 * 60_000;

/// What a person uses now, by one look: `apps`, the apps of ours that run outside every lease
/// (pid, label, [`crate::top::CpuSplit::apps`]); `records`, the uses processes said (pid, how).
/// Only an app's own say holds, and it names that app.
pub fn sense(apps: &[(u32, String)], records: &[(u32, String)]) -> Option<HandHeld> {
    records.iter().find_map(|(pid, how)| apps.iter().find(|(p, _)| p == pid).map(|(_, app)| HandHeld { app: app.clone(), pid: *pid, how: how.clone() }))
}

/// The hold between looks: one ended keeps [`GRACE_MS`] while its app runs.
#[derive(Default, Debug)]
pub struct HandTrack {
    /// the last use seen and when (wall ms)
    last: Option<(HandHeld, u64)>,
}

impl HandTrack {
    /// The app the last look held for, said (`Fab via an XR headset`).
    pub fn held(&self) -> Option<String> {
        self.last.as_ref().map(|(h, _)| format!("{} via {}", h.app, h.how))
    }

    /// One look at wall ms `now`: `seen` ([`sense`]) and the apps of ours outside the leases now.
    pub fn update(&mut self, now: u64, seen: Option<HandHeld>, apps: &[(u32, String)]) -> Option<HandHeld> {
        if let Some(h) = seen {
            self.last = Some((h.clone(), now));
            return Some(h);
        }
        let kept = self.last.as_ref().filter(|(h, at)| apps.iter().any(|(p, _)| *p == h.pid) && now.saturating_sub(*at) < GRACE_MS)
            .map(|(h, at)| HandHeld { how: format!("{}, ended {} s ago", h.how, now.saturating_sub(*at) / 1000), ..h.clone() });
        if kept.is_none() {
            self.last = None;
        }
        kept
    }
}

/// The uses this machine's processes said (`makepad_platform::restart::set_in_use`), (pid, how):
/// live processes' only. A file of a process that is gone is removed (an app killed in an XR
/// session leaves it).
#[cfg(all(unix, feature = "checkout"))]
pub fn in_use_records() -> Vec<(u32, String)> {
    makepad_app_registry::machine::in_use_dir().map_or_else(Vec::new, |dir| records_in(&dir, &crate::procinfo::alive))
}

/// [`in_use_records`] of `dir`, `alive` telling a live pid (plain files: its test runs on every OS).
#[cfg(any(test, all(unix, feature = "checkout")))]
fn records_in(dir: &std::path::Path, alive: &dyn Fn(u32) -> bool) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        // (`<pid>.new` is a write in progress)
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        if !alive(pid) {
            let _ = std::fs::remove_file(entry.path());
            continue;
        }
        for how in std::fs::read_to_string(entry.path()).unwrap_or_default().lines().map(str::trim).filter(|l| !l.is_empty()) {
            out.push((pid, crate::proto::clean(how, 120)));
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-run app holds its box by its own say (an XR headset), and not while it is merely
    /// open; another process's say (a Desk Host's viewer) holds nothing; a hold keeps its grace
    /// after the use while its app runs, and ends when the app exits.
    #[test]
    fn a_hand_run_app_holds_its_box_by_its_own_say_and_not_while_it_is_merely_open() {
        let fab = vec![(41, "Fab".to_string())];
        let held = |how: &str| Some(HandHeld { app: "Fab".into(), pid: 41, how: how.into() });
        assert_eq!(sense(&fab, &[]), None, "open, unused: no hold");
        assert_eq!(sense(&fab, &[(7, "a Desk Link viewer (viewer-a)".into())]), None, "a Desk Link session holds no box");
        assert_eq!(sense(&fab, &[(7, "a Desk Link viewer".into()), (41, "an XR headset".into())]), held("an XR headset"), "the app's own say");
        assert_eq!(sense(&[], &[(41, "an XR headset".into())]), None, "no app of ours outside the leases: no hold");

        let mut t = HandTrack::default();
        assert_eq!(t.update(0, sense(&fab, &[(41, "an XR headset".into())]), &fab), held("an XR headset"));
        assert_eq!(t.update(60_000, None, &fab), held("an XR headset, ended 60 s ago"), "the headset off a minute: held");
        assert_eq!(t.update(GRACE_MS, None, &fab), None, "its grace over: free");
        assert_eq!(t.update(GRACE_MS + 1, None, &fab), None);
        assert!(t.update(GRACE_MS + 2, sense(&fab, &[(41, "an XR headset".into())]), &fab).is_some());
        assert_eq!(t.update(GRACE_MS + 3, None, &[]), None, "the app exited: free at once");

        // the records: a live process's lines, a dead one's file removed, a write in progress skipped
        let dir = std::env::temp_dir().join(format!("devhub-hand-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("41"), "an XR headset\n\n").unwrap();
        std::fs::write(dir.join("42.new"), "half\n").unwrap();
        std::fs::write(dir.join("43"), "a Desk Link viewer\n").unwrap();
        assert_eq!(records_in(&dir, &|pid| pid == 41), vec![(41, "an XR headset".to_string())]);
        assert!(!dir.join("43").exists() && dir.join("42.new").exists(), "a gone process's file goes, a write in progress stays");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
