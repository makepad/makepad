//! State-preserving restart in live sessions (Relax's hot reload, local/plans/hotreload.md §0
//! and §4.8): an edit Relax cannot patch in place (a struct's layout, a signature, a module) is
//! applied by restarting the app, and the user keeps what they were looking at.
//!
//! At the app's next event after Relax asks for a restart:
//! 1. `AppMain::can_restart` may postpone it (unsaved work: the app says why, and the restart
//!    waits until the app allows it);
//! 2. the app's own state (`AppMain::restart_state`) and the window frames are written;
//! 3. Relax replaces the process with a fresh run of the same command line;
//! 4. that run's `Startup` gives the app its state back (`AppMain::restore_state`) and opens
//!    the windows at their old frames.
//!
//! Only under Relax (`cfg(relax)`); a rustc build has no live session and none of this runs.

use crate::{cx::Cx, makepad_math::Vec2d};

/// What crosses a restart: each window's frame (by window index), the widgets' own state by
/// stable identity (their id path from the root, `crate::restart` users in the widgets crate)
/// and the app's own text.
#[derive(Default, Debug, Clone, PartialEq)]
pub struct RestartState {
    pub windows: Vec<(usize, Vec2d, Vec2d, bool)>,
    pub widgets: Vec<(String, String)>,
    /// why the previous run restarted (Relax's classifier), for the kept/lost report
    pub reason: String,
    pub app: Option<String>,
}

impl RestartState {
    /// Text form: `window <index> <x> <y> <w> <h> <fullscreen>` lines, `widget <path> <bytes>`
    /// lines each followed by that many bytes and a newline, then `app <bytes>` and the app's
    /// text.
    pub fn to_text(&self) -> String {
        let mut s = String::from("makepad-restart 2\n");
        if !self.reason.is_empty() {
            s.push_str(&format!("reason {}\n{}\n", self.reason.len(), self.reason));
        }
        for (i, p, z, f) in &self.windows {
            s.push_str(&format!("window {} {} {} {} {} {}\n", i, p.x, p.y, z.x, z.y, *f as u8));
        }
        for (path, st) in &self.widgets {
            s.push_str(&format!("widget {} {}\n{}\n", path, st.len(), st));
        }

        if let Some(a) = &self.app {
            s.push_str(&format!("app {}\n{}", a.len(), a));
        }
        s
    }

    pub fn from_text(t: &str) -> Option<RestartState> {
        let head = "makepad-restart 2\n";
        if !t.starts_with(head) {
            return None;
        }
        let mut st = RestartState::default();
        let mut at = head.len();
        while at < t.len() {
            let eol = t[at..].find('\n').map_or(t.len(), |k| at + k);
            let w: Vec<&str> = t[at..eol].split_whitespace().collect();
            at = (eol + 1).min(t.len());
            match w.as_slice() {
                ["window", i, x, y, ww, h, f] => {
                    let n = |v: &str| v.parse::<f64>().unwrap_or(0.0);
                    st.windows.push((i.parse().ok()?, Vec2d { x: n(x), y: n(y) }, Vec2d { x: n(ww), y: n(h) }, *f == "1"));
                }
                ["widget", path, len] => {
                    let len: usize = len.parse().ok()?;
                    st.widgets.push((path.to_string(), t.get(at..at + len)?.to_string()));
                    at = (at + len + 1).min(t.len());
                }
                ["reason", len] => {
                    let len: usize = len.parse().ok()?;
                    st.reason = t.get(at..at + len)?.to_string();
                    at = (at + len + 1).min(t.len());
                }

                ["app", len] => {
                    let len: usize = len.parse().ok()?;
                    st.app = Some(t.get(at..at + len)?.to_string());
                    at += len;
                }
                _ => {}
            }
        }
        Some(st)
    }
}

use std::sync::Mutex;

/// The widgets' state, written by the widget root at the event a restart is about to happen
/// (`wants_snapshot`), read by `try_restart` right after it.
static WIDGETS_OUT: Mutex<Option<Vec<(String, String)>>> = Mutex::new(None);
/// After a restart: the widgets' state still to give back (the widget root takes it as the
/// widgets appear) and what is known so far for the kept/lost report.
static WIDGETS_IN: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
static REPORT: Mutex<Option<Report>> = Mutex::new(None);
static SNAPSHOT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What a restart kept and lost (logged once the widgets settle, `restart_report`).
#[derive(Default, Debug, Clone)]
pub struct Report {
    pub reason: String,
    pub windows: usize,
    pub app_state: Option<bool>,
    pub widgets_kept: usize,
    pub widgets_lost: Vec<String>,
}

/// The widget root, at each event while a restart is due: whether to write the widgets' state
/// now (`stash_widgets`) because the restart is asked for right after this event.
pub fn wants_snapshot() -> bool {
    SNAPSHOT.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn stash_widgets(w: Vec<(String, String)>) {
    *WIDGETS_OUT.lock().unwrap() = Some(w);
}

/// After a restart: the widgets' state not given back yet (empty otherwise: one lock).
pub fn take_restored_widgets() -> Vec<(String, String)> {
    std::mem::take(&mut *WIDGETS_IN.lock().unwrap())
}

/// At `Startup` of a restarted run: the app's state was given back (or there was none); with
/// no widget state to wait for, the report is final now.
pub fn restored_app(app: bool, no_widgets: bool) {
    let mut g = REPORT.lock().unwrap();
    if let Some(r) = g.as_mut() {
        r.app_state = Some(app);
        if no_widgets {
            crate::log!("{}", r.text());
            log_report(r);
        }
    }
}

/// At this run's first frame on screen: a restarted run logs save -> back on screen to the live
/// session log (Relax's scoreboard).
pub fn first_present() {
    #[cfg(relax)]
    {
        if REPORT.lock().unwrap().is_some() {
            if let Some(t0) = std::relax::restart_saved_ms() {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64() * 1e3);
                std::relax::live_log(&format!("{{\"event\":\"restart_present\",\"pid\":{},\"ms\":{:.0}}}", std::process::id(), now - t0));
            }
        }
    }
}

fn log_report(r: &Report) {
    #[cfg(relax)]
    {
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let lost: Vec<String> = r.widgets_lost.iter().map(|l| format!("\"{}\"", esc(l))).collect();
        std::relax::live_log(&format!(
            "{{\"event\":\"restart_report\",\"pid\":{},\"kept\":{{\"windows\":{},\"app_state\":{},\"widgets\":{}}},\"lost\":{{\"app_fields_fresh\":{},\"widgets\":[{}]}}}}",
            std::process::id(),
            r.windows,
            r.app_state == Some(true),
            r.widgets_kept,
            r.app_state == Some(false),
            lost.join(",")
        ));
    }
    #[cfg(not(relax))]
    let _ = r;
}

/// The widget root: these entries found their widgets; `lost` never did (final).
pub fn widgets_settled(kept: usize, lost: Vec<String>) {
    if let Some(r) = REPORT.lock().unwrap().as_mut() {
        r.widgets_kept = kept;
        r.widgets_lost = lost;
        crate::log!("{}", r.text());
        log_report(r);
    }
}

/// The last restart's kept/lost report (None in a run that was not a restart).
pub fn restart_report() -> Option<Report> {
    REPORT.lock().unwrap().clone()
}

impl Report {
    pub fn text(&self) -> String {
        let app = match self.app_state {
            Some(true) => "app state kept",
            Some(false) => "app fields start fresh (the app has no AppMain::restart_state)",
            None => "no app state",
        };
        let mut s = format!(
            "restart ({}): kept {} window frame(s), {}, {} widget state(s)",
            if self.reason.is_empty() { "live edit".to_string() } else { self.reason.lines().next().unwrap_or("").chars().take(160).collect::<String>() },
            self.windows,
            app,
            self.widgets_kept
        );
        if !self.widgets_lost.is_empty() {
            s.push_str(&format!("; LOST {} widget state(s) (no widget at that id path now): {}", self.widgets_lost.len(), self.widgets_lost.join(", ")));
        }
        s
    }
}

impl Cx {
    /// The created windows' frames (a restart reopens them there).
    pub fn restart_window_frames(&self) -> Vec<(usize, Vec2d, Vec2d, bool)> {
        let mut out = Vec::new();
        for id in self.windows.id_iter() {
            let i = id.id();
            let w = &self.windows[id];
            if w.is_created && !w.is_popup {
                out.push((i, w.get_position(), w.get_inner_size(), w.window_geom.is_fullscreen));
            }
        }
        out
    }

    /// Seeds the windows not created yet with the frames a restart carried over.
    pub fn restart_apply_window_frames(&mut self, frames: &[(usize, Vec2d, Vec2d, bool)]) {
        let ids: Vec<_> = self.windows.id_iter().collect();
        for &(i, pos, size, full) in frames {
            if let Some(&id) = ids.iter().find(|id| id.id() == i) {
                let w = &mut self.windows[id];
                if !w.is_created && !w.is_popup {
                    w.create_position = Some(pos);
                    w.create_inner_size = Some(size);
                    w.is_fullscreen = full;
                }
            }
        }
    }
}

/// Whether Relax asks for a restart (one atomic load).
pub fn requested() -> bool {
    #[cfg(relax)]
    {
        std::relax::restart_requested()
    }
    #[cfg(not(relax))]
    {
        false
    }
}

/// Whether this app runs hosted in a Relax domain (started by a window manager) rather than as the
/// process's own app. A hosted app leaves process-wide resources (signal handlers) to the process and
/// ends its background workers with itself (`stop_workers`): domain teardown waits for every thread.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn hosted() -> bool {
    #[cfg(relax)]
    {
        std::relax::domain() != 0
    }
    #[cfg(not(relax))]
    {
        false
    }
}

/// A hosted app ends: its background workers (the logger, the restart watcher, the remote's
/// grab encoder) end with it.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn stop_workers() {
    #[cfg(relax)]
    STOP_WAKER.store(true, std::sync::atomic::Ordering::Release);
    #[cfg(not(target_arch = "wasm32"))]
    crate::log::stop_sink();
    crate::remote::stop_encoder();
}

#[cfg(relax)]
static STOP_WAKER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Tells Relax this app restarts with its state (called once at startup).
/// Relax only sets a flag when it wants a restart; an idle app sees no event, so a watcher
/// thread (a 20 ms sleep loop, one atomic load each; under Relax only) sends the app an
/// `Event::Signal` then.
pub fn announce() {
    #[cfg(relax)]
    {
        std::relax::restart_supported();
        use std::sync::atomic::{AtomicBool, Ordering};
        static WAKER: AtomicBool = AtomicBool::new(false);
        if !WAKER.swap(true, Ordering::Relaxed) {
            let _ = std::thread::Builder::new().name("restart-waker".into()).spawn(|| while !STOP_WAKER.load(Ordering::Acquire) {
                std::thread::sleep(std::time::Duration::from_millis(20));
                if std::relax::restart_requested() {
                    crate::thread::SignalToUI::set_ui_signal();
                    std::thread::sleep(std::time::Duration::from_millis(400));
                }
            });
        }
    }
}

/// The state the previous run left, if this run is a restart.
pub fn restored() -> Option<RestartState> {
    #[cfg(relax)]
    {
        let st = std::relax::restored_state().and_then(|t| RestartState::from_text(&t))?;
        *WIDGETS_IN.lock().unwrap() = st.widgets.clone();
        *REPORT.lock().unwrap() = Some(Report { reason: st.reason.clone(), windows: st.windows.len(), app_state: None, widgets_kept: 0, widgets_lost: Vec::new() });
        Some(st)
    }
    #[cfg(not(relax))]
    {
        None
    }
}

/// Restarts with `state`. A hosted Relax app returns for cooperative event-loop shutdown;
/// a directly running program returns only if replacing its process failed.
pub fn restart_now(state: &RestartState) {
    #[cfg(relax)]
    std::relax::restart_now(&state.to_text());
    #[cfg(not(relax))]
    let _ = state;
}

/// At an event after a restart request: asks the app, then snapshots and restarts.
pub fn try_restart(app: &mut dyn crate::app_main::AppMain, cx: &mut Cx) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST_ASK_MS: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let last = LAST_ASK_MS.load(Ordering::Relaxed);
    if last != 0 && now < last + 1000 {
        return;
    }
    // the widgets write their state at the next event (they see `wants_snapshot` while they
    // handle it); the app is asked after that event
    if !SNAPSHOT.swap(true, Ordering::Relaxed) {
        cx.redraw_all();
        return;
    }
    LAST_ASK_MS.store(now, Ordering::Relaxed);
    match app.can_restart(cx) {
        Err(why) => {
            SNAPSHOT.store(false, Ordering::Relaxed);
            if last == 0 {
                crate::log!("restart postponed ({}): {}", reason(), why);
            }
        }
        Ok(()) => {
            let widgets = WIDGETS_OUT.lock().unwrap().take().unwrap_or_default();
            let st = RestartState { windows: cx.restart_window_frames(), widgets, reason: reason(), app: app.restart_state(cx) };
            restart_now(&st);
            SNAPSHOT.store(false, Ordering::Relaxed);
        }
    }
}

/// Why the restart is due (for a postponed-restart notice).
pub fn reason() -> String {
    #[cfg(relax)]
    {
        std::relax::restart_reason()
    }
    #[cfg(not(relax))]
    {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trip() {
        let st = RestartState {
            windows: vec![(0, Vec2d { x: 10.0, y: 20.5 }, Vec2d { x: 800.0, y: 600.0 }, false)],
            widgets: vec![("main_window/body/input".to_string(), "3 3\nab\nc".to_string()), ("w".to_string(), String::new())],
            reason: "struct `App` changed:\nits layout".to_string(),
            app: Some("a b\nc".to_string()),
        };
        assert_eq!(RestartState::from_text(&st.to_text()), Some(st));
    }
}

/// Named fields for `restart_state` / `restore_state`, so an edit that adds, removes or retypes a
/// field keeps every other field, and reports the ones it could not give back. Hand-written
/// hooks (what fields cannot express) add their own named entries to the same set.
#[derive(Default, Debug, Clone, PartialEq)]
pub struct RestartFields {
    pub entries: Vec<(String, String)>,
}

impl RestartFields {
    pub fn put<T: RestartValue + ?Sized>(&mut self, name: &str, value: &T) {
        self.entries.push((name.to_string(), value.to_restart()));
    }

    /// The value kept under `name`, if there is one of type `T`.
    pub fn get<T: RestartValue>(&self, name: &str) -> Option<T> {
        self.entries.iter().find(|(n, _)| n == name).and_then(|(_, v)| T::from_restart(v))
    }

    /// Restores `name` into `slot`; `false` (kept as it is) when missing or of another type.
    pub fn restore<T: RestartValue>(&self, name: &str, slot: &mut T) -> bool {
        match self.get::<T>(name) {
            Some(v) => {
                *slot = v;
                true
            }
            None => false,
        }
    }

    /// `<name length> <name><value length> <value>` records, back to back.
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        for (n, v) in &self.entries {
            s.push_str(&format!("{} {}{} {}", n.len(), n, v.len(), v));
        }
        s
    }

    pub fn from_text(t: &str) -> RestartFields {
        let mut f = RestartFields::default();
        let mut rest = t;
        let take = |rest: &mut &str| -> Option<String> {
            let (len, after) = rest.split_once(' ')?;
            let len: usize = len.parse().ok()?;
            let v = after.get(..len)?.to_string();
            *rest = &after[len..];
            Some(v)
        };
        while !rest.is_empty() {
            match (take(&mut rest), take(&mut rest)) {
                (Some(n), Some(v)) => f.entries.push((n, v)),
                _ => break,
            }
        }
        f
    }
}

/// A field's value as text for a restart (`RestartFields`). The generator derives it for the
/// app's own plain types; these cover the standard ones.
pub trait RestartValue: Sized {
    fn to_restart(&self) -> String;
    fn from_restart(s: &str) -> Option<Self>;
}

macro_rules! restart_value_parse {
    ($($t:ty),*) => {$(
        impl RestartValue for $t {
            fn to_restart(&self) -> String {
                self.to_string()
            }
            fn from_restart(s: &str) -> Option<Self> {
                s.parse().ok()
            }
        }
    )*};
}
restart_value_parse!(u8, u16, u32, u64, usize, i8, i16, i32, i64, isize, f32, f64, bool, char);

impl RestartValue for String {
    fn to_restart(&self) -> String {
        self.clone()
    }
    fn from_restart(s: &str) -> Option<Self> {
        Some(s.to_string())
    }
}

impl<T: RestartValue> RestartValue for Option<T> {
    fn to_restart(&self) -> String {
        match self {
            Some(v) => format!("S{}", v.to_restart()),
            None => "N".to_string(),
        }
    }
    fn from_restart(s: &str) -> Option<Self> {
        match s.as_bytes().first() {
            Some(b'N') if s.len() == 1 => Some(None),
            Some(b'S') => T::from_restart(&s[1..]).map(Some),
            _ => None,
        }
    }
}

impl<T: RestartValue> RestartValue for Vec<T> {
    fn to_restart(&self) -> String {
        let mut f = RestartFields::default();
        for v in self {
            f.entries.push((String::new(), v.to_restart()));
        }
        f.to_text()
    }
    fn from_restart(s: &str) -> Option<Self> {
        RestartFields::from_text(s).entries.iter().map(|(_, v)| T::from_restart(v)).collect()
    }
}

#[cfg(test)]
mod fields_tests {
    use super::*;

    #[test]
    fn fields_round_trip() {
        let mut f = RestartFields::default();
        f.put("query", &"a b\nc".to_string());
        f.put("count", &7u32);
        f.put("sel", &Some(3i64));
        f.put("none", &None::<f64>);
        f.put("list", &vec!["x".to_string(), "y 2".to_string()]);
        let g = RestartFields::from_text(&f.to_text());
        assert_eq!(g.get::<String>("query").as_deref(), Some("a b\nc"));
        assert_eq!(g.get::<u32>("count"), Some(7));
        assert_eq!(g.get::<Option<i64>>("sel"), Some(Some(3)));
        assert_eq!(g.get::<Option<f64>>("none"), Some(None));
        assert_eq!(g.get::<Vec<String>>("list"), Some(vec!["x".to_string(), "y 2".to_string()]));
        // a field that changed type is not given back
        assert_eq!(g.get::<u32>("query"), None);
        let mut slot = 1u32;
        assert!(!g.restore("missing", &mut slot));
        assert_eq!(slot, 1);
    }
}

/// Called by the Splash app entrypoint after each application event.
pub fn after_event(app: &mut dyn crate::app_main::AppMain, cx: &mut Cx, event: &crate::event::Event) {
    #[cfg(relax)]
    {
        if matches!(event, crate::event::Event::Startup) {
            announce();
            if let Some(state) = restored() {
                cx.restart_apply_window_frames(&state.windows);
                if let Some(text) = &state.app { app.restore_state(cx, text); }
                restored_app(state.app.is_some(), state.widgets.is_empty());
            }
        }
        if requested() { try_restart(app, cx); }
    }
    #[cfg(not(relax))]
    let _ = (app, cx, event);
}
