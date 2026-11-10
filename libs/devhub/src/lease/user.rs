//! A person at this node: the node hub's user-present state, fed by our WM (`makepad-wm
//! activity`, every [`POLL`]). The WM counts only input its device readers took from a physical
//! device or the user's Desk Link viewer (`makepad_hal::user_input`): injected input, agent
//! input, a spinning app, a video or load never mark a person. A user's own app launch comes
//! from such input (a click, a key), so it marks them at that moment and not after.
//!
//! A Mac has no WM of ours: its hub reads the HID system's idle time instead (`HIDIdleTime`,
//! `hid_idle_ms`: the last keyboard, mouse or trackpad input of any session), the same answer
//! in the same rules ([`hid_answer`]). Agents drive apps through their remote sockets, which the
//! HID system never sees.
//!
//! Input within `user_idle_min` minutes (lease.conf, default 3): the node is "user": the
//! master grants it no new lease and the node takes no network writes; running leases finish.
//! That long without input, it takes work again. The user's dev switch is separate and wins:
//! a node they switched off stays off. A node with neither our WM nor a Mac's HID idle time
//! (Windows) senses nothing; its switch alone decides. Events: `node_user_present`,
//! `node_user_idle`.
//!
//! A Desk Link viewer showing the node ([`UserTrack::watched`]: its tab selected in a visible
//! window, which the WM counts as an unpaused viewer) holds back what a person watching would see
//! (a WM restart) while its input is under [`VIEWER_HOLDS_MIN`] old; it takes no box out of the
//! pool. A tab merely connected holds nothing: the viewer keeps tabs on many nodes (two nodes'
//! WM restarts waited on one for good, 11-02).
//!
//! A person whose last input came through Desk Link is at the node only while a viewer shows it:
//! once its tab goes elsewhere the node takes work at once, not `user_idle_min` later (the user
//! drove a pool box from the Mac's viewer, moved on, and the box stood idle with 20 leases queued,
//! 11-07). And the person can hand the node back (`devhub node <n> back`,
//! [`UserTrack::hand_back`]): the input so far no longer holds it; new input does again.

use super::UserSense;

/// How long after the last input a Desk Link viewer showing the node still means a person.
pub const VIEWER_HOLDS_MIN: u64 = 10;

/// How often the hub asks its WM.
pub const POLL: std::time::Duration = std::time::Duration::from_secs(2);

/// A Mac's HID idle time as the WM's `activity` answer: input `idle_ms` ago from its device.
pub fn hid_answer(idle_ms: u64) -> String {
    format!("{{\"input_ms\":{idle_ms},\"origin\":\"device\",\"what\":\"input\"}}")
}

/// How long ago the HID system took input, ms: IOHIDSystem's `HIDIdleTime` (ns). None: no such
/// service or no number in it.
#[cfg(target_os = "macos")]
pub fn hid_idle_ms() -> Option<u64> {
    use core::ffi::{c_char, c_void};
    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOServiceMatching(name: *const c_char) -> *mut c_void;
        fn IOServiceGetMatchingService(main_port: u32, matching: *mut c_void) -> u32;
        fn IORegistryEntryCreateCFProperty(entry: u32, key: *const c_void, allocator: *const c_void, options: u32) -> *const c_void;
        fn IOObjectRelease(object: u32) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(allocator: *const c_void, text: *const c_char, encoding: u32) -> *const c_void;
        fn CFGetTypeID(cf: *const c_void) -> usize;
        fn CFNumberGetTypeID() -> usize;
        fn CFNumberGetValue(number: *const c_void, kind: isize, out: *mut c_void) -> u8;
        fn CFRelease(cf: *const c_void);
    }
    const UTF8: u32 = 0x0800_0100;
    const SINT64: isize = 4;
    // SAFETY: each call gets live arguments; the matching dictionary is consumed by the lookup,
    // and the service, key and property are released here once read.
    unsafe {
        let service = IOServiceGetMatchingService(0, IOServiceMatching(c"IOHIDSystem".as_ptr()));
        if service == 0 {
            return None;
        }
        let key = CFStringCreateWithCString(core::ptr::null(), c"HIDIdleTime".as_ptr(), UTF8);
        let value = if key.is_null() { core::ptr::null() } else { IORegistryEntryCreateCFProperty(service, key, core::ptr::null(), 0) };
        let mut ns: i64 = -1;
        if !value.is_null() && CFGetTypeID(value) == CFNumberGetTypeID() {
            CFNumberGetValue(value, SINT64, &mut ns as *mut i64 as *mut c_void);
        }
        for cf in [value, key] {
            if !cf.is_null() {
                CFRelease(cf);
            }
        }
        IOObjectRelease(service);
        u64::try_from(ns).ok().map(|ns| ns / 1_000_000)
    }
}

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
    /// the last input came through a Desk Link viewer
    desk_link: bool,
    /// wall ms up to which the input was handed back ([`Self::hand_back`])
    handed_back_ms: Option<u64>,
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
                        self.desk_link = s("origin") == "desk link";
                    }
                }
                let idle_ms = self.last_input_ms.map(|l| now.saturating_sub(l));
                self.viewers = v.get("viewers").and_then(super::num).unwrap_or(0.0) > 0.0;
                self.watched_now = now;
                let present = self.person(now, idle_min);
                self.sense = Some(UserSense { present, idle_ms, what: self.what.clone(), idle_min });
            }
            // no WM (none of ours, or it restarts): what was seen still holds until it times out
            None => {
                let present = self.person(now, idle_min);
                self.sense = self.sense.take().and_then(|mut s| {
                    s.idle_ms = self.last_input_ms.map(|l| now.saturating_sub(l));
                    s.present = present;
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

    /// A person at the node at wall ms `now`: input under `idle_min` old that was not handed
    /// back, and for Desk Link input, a viewer still showing the node.
    fn person(&self, now: u64, idle_min: u32) -> bool {
        self.last_input_ms.is_some_and(|l| now.saturating_sub(l) < idle_min as u64 * 60_000 && self.handed_back_ms.is_none_or(|b| l > b))
            && (!self.desk_link || self.viewers)
    }

    /// The person hands the node back at wall ms `now` (`devhub node <n> back`, on their say-so):
    /// the input so far no longer holds it, new input does again. Whether they were present.
    pub fn hand_back(&mut self, now: u64) -> bool {
        let was = self.present();
        self.handed_back_ms = Some(now);
        if let Some(s) = self.sense.as_mut() {
            s.present = false;
        }
        was
    }

    /// A viewer showing the node, with input recent enough to mean someone is watching.
    pub fn watched(&self) -> bool {
        self.viewers && self.last_input_ms.is_some_and(|l| self.watched_now.saturating_sub(l) < VIEWER_HOLDS_MIN * 60_000)
    }

    pub fn present(&self) -> bool {
        self.sense.as_ref().is_some_and(|s| s.present)
    }

    /// Wall ms of the last person input this hub saw (None: none since it started).
    pub fn last_input_ms(&self) -> Option<u64> {
        self.last_input_ms
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
    fn a_macs_hid_idle_time_is_a_person_by_the_same_rules() {
        let mut t = UserTrack::default();
        let t0 = 10 * MIN;
        // the HID idle time grows between inputs: one input, then three quiet minutes
        assert_eq!(t.update(t0, Ok(&hid_answer(2 * MIN)), 3), Some(Flip::Present));
        assert_eq!(t.sense.as_ref().unwrap().what, "input from its device");
        assert_eq!(t.update(t0 + MIN - 1, Ok(&hid_answer(3 * MIN - 1)), 3), None, "under 3 min since the input");
        assert_eq!(t.update(t0 + MIN, Ok(&hid_answer(3 * MIN)), 3), Some(Flip::Idle));
        // a new input resets it
        assert_eq!(t.update(t0 + 2 * MIN, Ok(&hid_answer(10)), 3), Some(Flip::Present));
        // a Mac nobody touched since it booted
        let mut n = UserTrack::default();
        assert_eq!(n.update(t0, Ok(&hid_answer(40 * 24 * 60 * MIN)), 3), None);
        assert!(!n.present());
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
        // a viewer showing the node holds a WM restart while its input is under 10 min old
        let viewer = |ms: &str| format!("{{\"input_ms\":{ms},\"seq\":3,\"origin\":\"desk link\",\"what\":\"pointer\",\"viewers\":1}}");
        let mut v = UserTrack::default();
        v.update(MIN, Ok(&viewer("null")), 3);
        assert!(!v.watched(), "a viewer with no input since the WM started");
        v.update(2 * MIN, Ok(&viewer("0")), 3);
        assert!(v.watched());
        v.update(12 * MIN, Ok(&viewer(&(10 * MIN).to_string())), 3);
        assert!(!v.watched());
        // no WM ever: nothing sensed
        let mut n = UserTrack::default();
        assert_eq!(n.update(MIN, Err("no WM".into()), 3), None);
        assert!(n.sense.is_none());
    }

    #[test]
    fn a_desk_link_person_holds_the_node_while_their_viewer_shows_it() {
        let link = |ms: u64, viewers: u32| format!("{{\"input_ms\":{ms},\"seq\":3,\"origin\":\"desk link\",\"what\":\"pointer\",\"viewers\":{viewers}}}");
        let mut t = UserTrack::default();
        assert_eq!(t.update(MIN, Ok(&link(0, 1)), 3), Some(Flip::Present));
        assert_eq!(t.update(MIN + 2000, Ok(&link(2000, 1)), 3), None);
        // the viewer's tab goes to another node: the node takes work at once, not 3 min later
        assert_eq!(t.update(MIN + 4000, Ok(&link(4000, 0)), 3), Some(Flip::Idle));
        assert_eq!(t.update(MIN + 6000, Ok(&link(6000, 1)), 3), Some(Flip::Present), "back on its tab, within the minutes");
        // a device's input keeps its minutes, whatever the viewers
        let mut d = UserTrack::default();
        assert_eq!(d.update(MIN, Ok(&input(0)), 3), Some(Flip::Present));
        assert_eq!(d.update(2 * MIN, Ok(&input(MIN)), 3), None);
        assert!(d.present());
    }

    #[test]
    fn a_node_handed_back_takes_work_until_new_input() {
        let mut t = UserTrack::default();
        t.update(MIN, Ok(&input(0)), 3);
        assert!(t.hand_back(MIN + 1000));
        assert!(!t.present());
        assert_eq!(t.update(MIN + 2000, Ok(&input(2000)), 3), None, "the same input, handed back");
        assert!(!t.present());
        assert_eq!(t.update(MIN + 4000, Ok(&input(500)), 3), Some(Flip::Present), "new input counts again");
        assert!(!UserTrack::default().hand_back(MIN), "nobody to hand back");
    }
}
