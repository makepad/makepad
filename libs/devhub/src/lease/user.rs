//! A person at this node: the node hub's user-present state, fed by our WM (`makepad-wm
//! activity`, every [`POLL`]). The WM counts only input its device readers took from a physical
//! device or the user's Desk Link viewer (`makepad_hal::user_input`): injected input, agent
//! input, a spinning app, a video or load never mark a person. A user's own app launch comes
//! from such input (a click, a key), so it marks them at that moment and not after.
//!
//! Input within `user_idle_min` minutes (lease.conf, default 3): the node is "user": the
//! master grants it no new lease and the node takes no network writes; running leases finish.
//! That long without input, it takes work again. The user's dev switch is separate and wins:
//! a node they switched off stays off. A node without our WM (the Macs, Windows) senses
//! nothing; its switch alone decides. Events: `node_user_present`, `node_user_idle`.
//!
//! A Desk Link viewer attached ([`UserTrack::watched`]) holds back what a person watching would see
//! (a WM restart) for [`VIEWER_HOLDS_MIN`] after the last input; it takes no box out of the pool:
//! viewer tabs stay open on many nodes.

use super::UserSense;

/// How long after the last input an attached Desk Link viewer still means a person.
pub const VIEWER_HOLDS_MIN: u64 = 60;

/// How often the hub asks its WM.
pub const POLL: std::time::Duration = std::time::Duration::from_secs(2);

/// What the hub keeps between answers.
#[derive(Default, Debug)]
pub struct UserTrack {
    /// wall ms of the last input (kept across WM restarts: a restarted WM has seen none yet)
    last_input_ms: Option<u64>,
    what: String,
    /// the last state (None: no WM answers)
    pub sense: Option<UserSense>,
    /// a Desk Link viewer attached at the last answer (at wall ms `watched_now`)
    viewers: bool,
    watched_now: u64,
}

/// A change worth an event.
#[derive(Debug, PartialEq)]
pub enum Flip {
    Present,
    Idle,
}

impl UserTrack {
    /// One WM answer at wall ms `now` (`Err`: no WM of ours answers): the new state, and the
    /// flip when presence changed.
    pub fn update(&mut self, now: u64, answer: Result<&str, String>, idle_min: u32) -> Option<Flip> {
        let was = self.present();
        match answer.ok().and_then(|a| makepad_strict_json::parse(a.as_bytes()).ok()) {
            Some(v) => {
                if let Some(ms) = v.get("input_ms").and_then(super::num) {
                    let at = now.saturating_sub(ms as u64);
                    if self.last_input_ms.is_none_or(|l| at > l) {
                        self.last_input_ms = Some(at);
                        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
                        self.what = format!("{} from its {}", s("what"), s("origin"));
                    }
                }
                let idle_ms = self.last_input_ms.map(|l| now.saturating_sub(l));
                self.viewers = v.get("viewers").and_then(super::num).unwrap_or(0.0) > 0.0;
                self.watched_now = now;
                let present = idle_ms.is_some_and(|i| i < idle_min as u64 * 60_000);
                self.sense = Some(UserSense { present, idle_ms, what: self.what.clone(), idle_min });
            }
            // no WM (none of ours, or it restarts): what was seen still holds until it times out
            None => {
                self.sense = self.sense.take().and_then(|mut s| {
                    s.idle_ms = self.last_input_ms.map(|l| now.saturating_sub(l));
                    s.present = s.idle_ms.is_some_and(|i| i < idle_min as u64 * 60_000);
                    s.idle_min = idle_min;
                    s.present.then_some(s)
                });
            }
        }
        match (was, self.present()) {
            (false, true) => Some(Flip::Present),
            (true, false) => Some(Flip::Idle),
            _ => None,
        }
    }

    /// A viewer attached, with input recent enough to mean someone may be watching.
    pub fn watched(&self) -> bool {
        self.viewers && self.last_input_ms.is_none_or(|l| self.watched_now.saturating_sub(l) < VIEWER_HOLDS_MIN * 60_000)
    }

    pub fn present(&self) -> bool {
        self.sense.as_ref().is_some_and(|s| s.present)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 60_000;

    fn input(ms: u64) -> String {
        format!("{{\"input_ms\":{ms},\"seq\":3,\"origin\":\"device\",\"what\":\"key\"}}")
    }
    const NONE: &str = "{\"input_ms\":null,\"seq\":0,\"origin\":null,\"what\":null}";

    #[test]
    fn a_spinning_box_without_input_stays_free() {
        // a WM that took no input (apps spinning, videos, synthetic and agent input are not
        // counted by it): never present
        let mut t = UserTrack::default();
        for i in 0..200 {
            assert_eq!(t.update(1_000_000 + i * 2000, Ok(NONE), 3), None);
            assert!(!t.present());
        }
        assert_eq!(t.sense.as_ref().map(|s| s.idle_ms), Some(None));
    }

    #[test]
    fn one_key_press_is_present_and_three_quiet_minutes_rejoin() {
        let mut t = UserTrack::default();
        let t0 = 10 * MIN;
        assert_eq!(t.update(t0, Ok(NONE), 3), None);
        // one key press, half a second before the poll
        assert_eq!(t.update(t0 + 2000, Ok(&input(500)), 3), Some(Flip::Present));
        assert_eq!(t.sense.as_ref().unwrap().what, "key from its device");
        // the WM keeps answering the same input, older each time
        let at = t0 + 1500;
        assert_eq!(t.update(at + 2 * MIN, Ok(&input(2 * MIN)), 3), None);
        assert!(t.present());
        assert_eq!(t.update(at + 3 * MIN - 1, Ok(&input(3 * MIN - 1)), 3), None);
        assert_eq!(t.update(at + 3 * MIN, Ok(&input(3 * MIN)), 3), Some(Flip::Idle));
        // configurable per node
        let mut t = UserTrack::default();
        t.update(t0, Ok(&input(0)), 10);
        assert_eq!(t.update(t0 + 9 * MIN, Ok(&input(9 * MIN)), 10), None);
        assert_eq!(t.update(t0 + 10 * MIN, Ok(&input(10 * MIN)), 10), Some(Flip::Idle));
    }

    #[test]
    fn a_wm_restart_keeps_the_person() {
        let mut t = UserTrack::default();
        t.update(MIN, Ok(&input(0)), 3);
        // the WM is gone for a moment, then answers with no input seen yet
        assert_eq!(t.update(MIN + 2000, Err("no WM".into()), 3), None);
        assert_eq!(t.update(MIN + 4000, Ok(NONE), 3), None);
        assert!(t.present());
        assert_eq!(t.update(4 * MIN, Ok(NONE), 3), Some(Flip::Idle));
        // no WM ever: nothing sensed
        let mut n = UserTrack::default();
        assert_eq!(n.update(MIN, Err("no WM".into()), 3), None);
        assert!(n.sense.is_none());
    }
}
