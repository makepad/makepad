//! Who watches this node's screen: the node hub's view, fed by our WM (`makepad-wm activity`,
//! every [`POLL`]). The WM counts only input its device readers took from a physical device or
//! the user's Desk Link viewer (`makepad_hal::user_input`): injected input, agent input, a
//! spinning app, a video or load never mark a person. A Mac has no WM of ours: its hub reads the
//! HID system's idle time instead (`HIDIdleTime`, [`hid_idle_ms`]: the last keyboard, mouse or
//! trackpad input of any session) as the same answer ([`hid_answer`]). Agents drive apps through
//! their remote sockets, which neither sees.
//!
//! It holds back only what a person watching would see ([`UserTrack::watched`]): a WM restart, the
//! WM's AI pane restarting for a landing, a close of what the WM opened, a screen lease's app
//! ended as forgotten. Someone watches while input is under [`VIEWER_HOLDS_MIN`] old and either a
//! Desk Link viewer shows the node (its tab selected in a visible window, which the WM counts as
//! an unpaused viewer) or the input came from the box's own devices. A tab merely connected
//! holds nothing: the viewer keeps tabs on many nodes (two nodes' WM restarts waited on one for
//! good, 11-02). The last input also keeps a visible live session from its idle end.
//!
//! It takes no box out of the dev pool: only the box's dev switch does. Input at a box, a Desk
//! Link session on it or a Mac's idle time stops no lease and no network write there (the user,
//! 11-18: "turn off the autodetection when i click on a box in desklink so they 'drop out of the
//! devpool'" ... "just make that manual").

/// How long after the last input someone still watches the node's screen ([`UserTrack::watched`]).
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
    /// a Desk Link viewer showing the node at the last answer (at wall ms `watched_now`)
    viewers: bool,
    watched_now: u64,
    /// the last input came through a Desk Link viewer
    desk_link: bool,
}

impl UserTrack {
    /// One WM answer at wall ms `now` (`Err`: no WM of ours answers: what was seen stays).
    pub fn update(&mut self, now: u64, answer: Result<&str, String>) {
        let Some(v) = answer.ok().and_then(|a| makepad_strict_json::parse(a.as_bytes()).ok()) else { return };
        if let Some(ms) = v.get("input_ms").and_then(super::num) {
            let at = now.saturating_sub(ms as u64);
            if self.last_input_ms.is_none_or(|l| at > l) {
                self.last_input_ms = Some(at);
                self.desk_link = v.get("origin").and_then(|x| x.as_str()) == Some("desk link");
            }
        }
        self.viewers = v.get("viewers").and_then(super::num).unwrap_or(0.0) > 0.0;
        self.watched_now = now;
    }

    /// Someone watches the node's screen: input under [`VIEWER_HOLDS_MIN`] old from its own
    /// devices, or through a Desk Link viewer that shows it now.
    pub fn watched(&self) -> bool {
        self.last_input_ms.is_some_and(|l| self.watched_now.saturating_sub(l) < VIEWER_HOLDS_MIN * 60_000) && (self.viewers || !self.desk_link)
    }

    /// A Desk Link viewer shows the node now (its tab selected in a visible window), input or
    /// not: a WM restart waits on it while apps run there, which the person sees
    /// (`sync::serve` restart_blockers). Watching a picture takes no input.
    pub fn shown(&self) -> bool {
        self.viewers
    }

    /// Wall ms of the last person input this hub saw (None: none since it started).
    pub fn last_input_ms(&self) -> Option<u64> {
        self.last_input_ms
    }

    /// How long before wall ms `now` the last input came (None: none since the hub started).
    pub fn idle_ms(&self, now: u64) -> Option<u64> {
        self.last_input_ms.map(|l| now.saturating_sub(l))
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
    fn a_spinning_box_without_input_is_watched_by_nobody() {
        // a WM that took no input (apps spinning, videos, synthetic and agent input are not
        // counted by it), a viewer's tab connected or not: nobody watches
        let mut t = UserTrack::default();
        for i in 0..200 {
            t.update(1_000_000 + i * 2000, Ok(NONE));
            assert!(!t.watched());
        }
        assert_eq!(t.last_input_ms(), None);
        let viewer = |ms: &str| format!("{{\"input_ms\":{ms},\"seq\":3,\"origin\":\"desk link\",\"what\":\"pointer\",\"viewers\":1}}");
        t.update(2_000_000, Ok(&viewer("null")));
        assert!(!t.watched(), "a viewer with no input since the WM started");
    }

    /// Input holds what a person would see while it is under 10 min old: from the box's own
    /// devices, or through a viewer that still shows the node. A WM that restarts keeps it.
    #[test]
    fn input_holds_what_a_person_sees_for_ten_minutes_while_they_can_see_it() {
        let link = |ms: u64, viewers: u32| format!("{{\"input_ms\":{ms},\"seq\":3,\"origin\":\"desk link\",\"what\":\"pointer\",\"viewers\":{viewers}}}");
        let mut v = UserTrack::default();
        v.update(MIN, Ok(&link(0, 1)));
        assert!(v.watched(), "a viewer showing the node, input now");
        v.update(MIN + 2000, Ok(&link(2000, 0)));
        assert!(!v.watched(), "its tab went to another node");
        v.update(5 * MIN, Ok(&link(4 * MIN, 1)));
        assert!(v.watched(), "back on its tab");
        v.update(11 * MIN, Ok(&link(10 * MIN, 1)));
        assert!(!v.watched(), "10 min without input");
        assert!(v.shown(), "its viewer still shows the node: a person may be watching a picture");
        v.update(11 * MIN + 2000, Ok(&link(10 * MIN, 0)));
        assert!(!v.shown(), "its tab went to another node");
        let mut d = UserTrack::default();
        d.update(MIN, Ok(&input(0)));
        assert!(d.watched(), "the box's own keyboard");
        // the WM is gone for a moment, then answers with no input seen yet
        d.update(MIN + 2000, Err("no WM".into()));
        d.update(MIN + 4000, Ok(NONE));
        assert!(d.watched(), "a WM restart keeps the input");
        d.update(11 * MIN, Ok(NONE));
        assert!(!d.watched());
        // a Mac's HID idle time is its own devices' input
        let mut m = UserTrack::default();
        m.update(10 * MIN, Ok(&hid_answer(2 * MIN)));
        assert!(m.watched());
        assert_eq!(m.idle_ms(10 * MIN), Some(2 * MIN));
        m.update(18 * MIN, Ok(&hid_answer(10 * MIN)));
        assert!(!m.watched());
    }
}
