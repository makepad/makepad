//! A viewer window: its tabs (the machines open in it), the one it shows, its input, its stats,
//! and its history scrubber. The app keeps one `Viewer` per window; every connection is open in
//! at most one of them.

use crate::caption::{CaptionToggle, DeskTabs, MenuItem, PopMenu, TabInfo, TabReach};
use crate::conn::{Latency, State};
use crate::history::Snapshot;
use crate::scrub::Scrub;
use crate::scrubber::{ScrubAction, ScrubData, Scrubber, SPAN_NS};
use crate::view::RemoteView;
use crate::{button_of, mods_of, shortcut, App, Shortcut, CURSORS};
use makepad_desk_link::ClientMsg;
use makepad_widgets::*;
use std::sync::Arc;

/// A line for a selftest (`MAKEPAD_TRAY_SELFTEST`): `desklink selftest: <what>` (where things
/// are drawn, what happened).
pub fn selftest(what: &str) {
    makepad_shell_kit::tray_app::selftest_note("desklink", what);
}

/// A new viewer window (`DeskWindow`), made by the platform at its first draw.
fn make_window(cx: &mut Cx) -> WidgetRef {
    cx.with_vm(|vm| {
        let value = script_eval!(vm, {
            use mod.prelude.widgets.*
            use mod.widgets.*
            mod.widgets.DeskWindow{}
        });
        WidgetRef::script_from_value(vm, value)
    })
}

pub struct Viewer {
    /// Its `Window` widget.
    pub window: WidgetRef,
    /// Made at run time (the app routes its events and draws it); the first window is the
    /// root's.
    pub extra: bool,
    /// Drawn once (a new window's lists exist only after its first draw).
    pub drawn: bool,
    /// The window shows (the first one may be closed into the menu bar).
    pub visible: bool,
    /// The connections open in it, in tab order, and the one it shows.
    pub tabs: Vec<usize>,
    pub selected: Option<usize>,
    /// Keyboard to the desktop (the picture was clicked last).
    pub keyboard: bool,
    /// Pointer buttons held over the picture (they keep driving it when the pointer leaves).
    pub held: u32,
    pub last_pointer: Option<(u16, u16)>,
    pub local_over: bool,
    /// The window's inner size in device pixels (fullscreen: the screen's backing size).
    pub window_px: [u32; 2],
    pub fullscreen: bool,
    pub latency: Latency,
    /// Pictures drawn this second so far, and over the last second: (shown, decoded but never
    /// shown) a second.
    pub shown: u32,
    pub shown_rate: (u32, u32),
    /// Present times waiting for their report: (frame, decode_us, present_us).
    pub last_stats: Option<(u32, u32, u32)>,
    /// Decode times (ms, a picture's slowest stream) of the pictures taken since the last
    /// `Stats`, summed, and how many: `Stats` reports their mean.
    pub decode_sum: (f64, u32),
    pub title: String,
    pub stats_text: String,
    /// The scrubber shows (over a live history, or the paused one).
    pub scrub_open: bool,
    /// The paused history (None: live).
    pub scrub: Option<Scrub>,
    /// Thumbnails of a live history (no picture decoded until a moment is hovered).
    pub preview: Option<Scrub>,
    /// The live history as the scrubber shows it (refreshed a few times a second).
    pub live_snap: Option<Arc<Snapshot>>,
    /// The picture of the history to show, and the one shown.
    pub cur: usize,
    pub hist_shown: Option<usize>,
    /// The range to save (In / Out).
    pub range: (Option<usize>, Option<usize>),
    /// Playing the history: (speed, negative backward; start time; the picture's host time at
    /// the start).
    pub playing: Option<(f64, f64, u64)>,
    pub play_frame: NextFrame,
    /// What the window's popup menu is open for.
    pub menu_for: MenuFor,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum MenuFor {
    #[default]
    None,
    /// "+": the desktops not open anywhere.
    Plus,
    /// A tab's menu.
    Tab(usize),
    /// The scrubber's Save menu.
    Save,
}

const OPEN_NEW_WINDOW: u64 = 1;
const RECONNECT: u64 = 2;
const CLOSE_TAB: u64 = 3;
const SAVE_CLIP: u64 = 1;
const SAVE_FRAME: u64 = 2;

impl Viewer {
    pub fn new(window: WidgetRef, extra: bool) -> Viewer {
        Viewer {
            window,
            extra,
            drawn: false,
            visible: true,
            tabs: Vec::new(),
            selected: None,
            keyboard: false,
            held: 0,
            last_pointer: None,
            local_over: false,
            window_px: [0, 0],
            fullscreen: false,
            latency: Latency::default(),
            shown: 0,
            shown_rate: (0, 0),
            last_stats: None,
            decode_sum: (0.0, 0),
            title: String::new(),
            stats_text: String::new(),
            scrub_open: false,
            scrub: None,
            preview: None,
            live_snap: None,
            cur: 0,
            hist_shown: None,
            range: (None, None),
            playing: None,
            play_frame: NextFrame::default(),
            menu_for: MenuFor::None,
        }
    }

    pub fn window_id(&self) -> Option<WindowId> {
        self.window.as_window().window_id()
    }

    pub fn widget(&self, cx: &mut Cx, path: &[LiveId]) -> WidgetRef {
        self.window.widget(cx, path)
    }
}

impl App {
    pub fn viewer_of_window(&self, id: WindowId) -> Option<usize> {
        self.viewers.iter().position(|v| v.window_id() == Some(id))
    }

    pub fn viewer_of_conn(&self, conn: usize) -> Option<usize> {
        self.viewers.iter().position(|v| v.tabs.contains(&conn))
    }

    /// Pauses every connection that no visible window shows and resumes the shown ones.
    pub fn sync_shown(&self) {
        for (i, c) in self.conns.iter().enumerate() {
            c.show(self.viewers.iter().any(|v| v.visible && v.selected == Some(i)));
        }
    }

    /// Shows connection `conn` (a tab of viewer `vi`) in that window.
    pub fn select(&mut self, cx: &mut Cx, vi: usize, conn: usize) {
        if self.viewers[vi].selected == Some(conn) {
            return;
        }
        if self.viewers[vi].scrub_open {
            // the history is the shown connection's
            self.close_scrubber(cx, vi);
        }
        let v = &mut self.viewers[vi];
        v.selected = Some(conn);
        v.last_pointer = None;
        v.held = 0;
        let sink = self.conns.get(conn).map(|c| c.view.clone());
        if let Some(mut r) = self.viewers[vi].widget(cx, ids!(remote)).borrow_mut::<RemoteView>() {
            r.clear(cx);
            r.sink = sink;
        }
        self.sync_shown();
        self.refresh_viewer(cx, vi);
    }

    /// Opens connection `conn` as a tab of viewer `vi` (taken from the window that had it) and
    /// shows it.
    pub fn add_tab(&mut self, cx: &mut Cx, vi: usize, conn: usize) {
        if let Some(from) = self.viewer_of_conn(conn) {
            if from == vi {
                self.select(cx, vi, conn);
                return;
            }
            self.close_tab(cx, from, conn);
        }
        self.viewers[vi].tabs.push(conn);
        self.select(cx, vi, conn);
    }

    /// Closes a tab: its connection stays open (paused; "+" or the menu bar shows it again).
    /// A window made at run time closes with its last tab.
    pub fn close_tab(&mut self, cx: &mut Cx, vi: usize, conn: usize) {
        let v = &mut self.viewers[vi];
        let Some(k) = v.tabs.iter().position(|c| *c == conn) else { return };
        v.tabs.remove(k);
        if v.selected == Some(conn) {
            if v.scrub_open {
                self.close_scrubber(cx, vi);
            }
            let v = &mut self.viewers[vi];
            v.selected = None;
            let next = v.tabs.get(k.min(v.tabs.len().saturating_sub(1))).copied();
            if let Some(mut r) = self.viewers[vi].widget(cx, ids!(remote)).borrow_mut::<RemoteView>() {
                r.clear(cx);
                r.sink = None;
            }
            if let Some(n) = next {
                self.select(cx, vi, n);
            }
        }
        if self.viewers[vi].tabs.is_empty() && self.viewers[vi].extra {
            self.viewers[vi].window.as_window().close(cx);
        }
        self.sync_shown();
        self.refresh_viewer(cx, vi);
    }

    /// The user closes a tab (Cmd+W, the tab's "Close Tab"): the last one closes its window too.
    pub fn close_tab_closing_window(&mut self, cx: &mut Cx, vi: usize, conn: usize) {
        self.close_tab(cx, vi, conn);
        if self.viewers[vi].tabs.is_empty() && !self.viewers[vi].extra {
            self.close_window(cx, vi);
        }
    }

    /// Closes window `vi` as its caption's close button does: the first window hides when
    /// `keep_on_close` says so, else it closes.
    pub fn close_window(&mut self, cx: &mut Cx, vi: usize) {
        if self.keep_on_close(vi) {
            self.hide_main(cx);
        } else {
            self.viewers[vi].window.as_window().close(cx);
        }
    }

    /// A new window showing connection `conn` (taken from the window that had it), at `at`
    /// (screen points; else beside the window it came from).
    pub fn new_window(&mut self, cx: &mut Cx, conn: usize, at: Option<Vec2d>) -> usize {
        let window = make_window(cx);
        let from = self.viewer_of_conn(conn);
        self.place_new_window(cx, &window, from.map(|f| (f, at)));
        self.viewers.push(Viewer::new(window, true));
        let vi = self.viewers.len() - 1;
        self.add_tab(cx, vi, conn);
        self.focused = vi;
        selftest(&format!("window {vi} made for {}", self.conns.get(conn).map_or("?", |c| c.name.as_str())));
        cx.redraw_all();
        vi
    }

    /// Where a new window goes, before the platform makes it: beside the window `from` (at `at`,
    /// screen points, when a tab was dropped there), half its width: two machines side by side.
    fn place_new_window(&self, cx: &mut Cx, window: &WidgetRef, from: Option<(usize, Option<Vec2d>)>) {
        let src = from.map(|f| f.0).unwrap_or(self.focused.min(self.viewers.len().saturating_sub(1)));
        let (pos, size) = match self.viewers.get(src) {
            Some(v) => {
                let w = v.window.as_window();
                (w.get_position(cx), w.get_inner_size(cx))
            }
            None => (dvec2(80.0, 80.0), dvec2(1400.0, 860.0)),
        };
        let size = dvec2((size.x * 0.5).max(640.0), size.y.max(400.0));
        let pos = from.and_then(|f| f.1).unwrap_or(pos + dvec2(size.x.min(700.0), 0.0));
        if let Some(id) = window.as_window().window_id() {
            if cx.windows.is_valid(id) {
                let w = &mut cx.windows[id];
                w.create_inner_size = Some(size);
                w.create_position = Some(pos);
            }
        }
    }

    /// A window made at run time closed: its tabs close with it.
    pub fn window_closed(&mut self, cx: &mut Cx, vi: usize) {
        let v = self.viewers.remove(vi);
        drop(v);
        if self.focused >= self.viewers.len() || self.focused == vi {
            self.focused = 0;
        } else if self.focused > vi {
            self.focused -= 1;
        }
        self.sync_shown();
        // no window left on screen: the app ends (the installed Dock app stays in the Dock)
        if !crate::installed::is_dock_app() && self.viewers.iter().all(|v| !v.visible) {
            cx.quit();
        }
    }

    /// The window that has connection `conn` shows it (made visible and raised), or a new
    /// window does.
    pub fn open_desktop(&mut self, cx: &mut Cx, conn: usize) {
        if conn >= self.conns.len() {
            return;
        }
        // a desktop that does not answer is tried again now (no waiting out its backoff)
        if matches!(self.conns[conn].state(), State::Offline(_)) {
            self.conns[conn].reconnect();
        }
        let vi = match self.viewer_of_conn(conn) {
            Some(vi) => vi,
            // the first window, closed into the menu bar with nothing open, takes it
            None if !self.viewers[0].visible && self.viewers[0].tabs.is_empty() => {
                self.add_tab(cx, 0, conn);
                0
            }
            None => self.new_window(cx, conn, None),
        };
        self.viewers[vi].visible = true;
        self.viewers[vi].keyboard = false;
        self.select(cx, vi, conn);
        self.viewers[vi].window.as_window().show(cx);
        self.focused = vi;
        self.sync_shown();
        selftest(&format!("showing desktop {conn} ({}) in window {vi}", self.conns[conn].name));
    }

    /// Whether closing window `vi` keeps it (hidden, its tabs kept): the first window of the
    /// installed Dock app (it stays in the Dock) or while another window shows.
    pub fn keep_on_close(&self, vi: usize) -> bool {
        vi == 0 && (crate::installed::is_dock_app() || self.viewers.iter().skip(1).any(|v| v.visible))
    }

    /// A window on screen when none is (the app opened again): the first one.
    pub fn show_some_window(&mut self, cx: &mut Cx) {
        if self.viewers.iter().any(|v| v.visible) {
            return;
        }
        match self.viewers[0].selected.or_else(|| self.viewers[0].tabs.first().copied()) {
            Some(conn) => self.open_desktop(cx, conn),
            None => {
                self.viewers[0].visible = true;
                self.viewers[0].window.as_window().show(cx);
                self.sync_shown();
            }
        }
        selftest("opened again: window shown");
    }

    /// File → New Window / Cmd+N: a window with the first desktop open in no window, else empty
    /// (its "+" opens one).
    pub fn new_empty_window(&mut self, cx: &mut Cx) {
        if let Some(conn) = (0..self.conns.len()).find(|i| self.viewer_of_conn(*i).is_none()) {
            self.new_window(cx, conn, None);
            return;
        }
        let window = make_window(cx);
        self.place_new_window(cx, &window, None);
        self.viewers.push(Viewer::new(window, true));
        let vi = self.viewers.len() - 1;
        self.focused = vi;
        self.refresh_viewer(cx, vi);
        selftest(&format!("window {vi} made empty"));
        cx.redraw_all();
    }

    /// The first window closed: hidden with its tabs (their desktops pause).
    pub fn hide_main(&mut self, cx: &mut Cx) {
        self.viewers[0].visible = false;
        if self.viewers[0].scrub_open {
            self.close_scrubber(cx, 0);
        }
        self.viewers[0].window.as_window().set_visible(cx, false);
        self.sync_shown();
        selftest("first window closed: hidden, its tabs kept");
    }

    /// Drops connection `i`'s session and opens it again now (a tab's "Reconnect", Cmd+R).
    pub fn reconnect(&mut self, cx: &mut Cx, i: usize) {
        if let Some(c) = self.conns.get(i) {
            c.reconnect();
        }
        if let Some(vi) = self.viewer_of_conn(i) {
            let v = &mut self.viewers[vi];
            if v.selected == Some(i) {
                v.held = 0;
                v.last_pointer = None;
            }
        }
        self.refresh(cx);
    }

    pub fn next_screen(&mut self, vi: usize) {
        if let Some(c) = self.viewers.get(vi).and_then(|v| v.selected).and_then(|i| self.conns.get(i)) {
            let next = c.shared.lock().unwrap().welcome.as_ref().map(|w| (w.screen + 1) % w.screens.len().max(1) as u32);
            if let Some(index) = next {
                c.send(ClientMsg::SelectScreen { index });
            }
        }
    }

    pub fn do_shortcut(&mut self, cx: &mut Cx, vi: usize, s: Shortcut) {
        match s {
            Shortcut::Desktop(k) => {
                if let Some(conn) = self.viewers[vi].tabs.get(k).copied() {
                    self.open_desktop(cx, conn);
                }
            }
            Shortcut::Reconnect => {
                if let Some(i) = self.viewers[vi].selected {
                    self.reconnect(cx, i);
                }
            }
            Shortcut::NewWindow => self.new_empty_window(cx),
            Shortcut::NextTab(d) => {
                let v = &self.viewers[vi];
                let n = v.tabs.len() as i32;
                if n > 0 {
                    let at = v.selected.and_then(|c| v.tabs.iter().position(|t| *t == c)).map_or(0, |k| k as i32 + d);
                    let conn = v.tabs[at.rem_euclid(n) as usize];
                    self.open_desktop(cx, conn);
                }
            }
            Shortcut::Fullscreen => {
                let w = self.viewers[vi].window.as_window();
                if w.is_fullscreen(cx) {
                    w.restore(cx);
                } else {
                    w.maximize(cx);
                }
            }
            Shortcut::CloseTab => match self.viewers[vi].selected.or_else(|| self.viewers[vi].tabs.last().copied()) {
                Some(conn) => self.close_tab_closing_window(cx, vi, conn),
                None => self.close_window(cx, vi),
            },
            Shortcut::NextScreen => self.next_screen(vi),
            Shortcut::Details => {
                self.details = !self.details;
                self.refresh(cx);
            }
            Shortcut::History => self.toggle_scrubber(cx, vi),
        }
    }

    /// Every window: its picture, caption and scrubber.
    pub fn refresh(&mut self, cx: &mut Cx) {
        for vi in 0..self.viewers.len() {
            self.refresh_viewer(cx, vi);
        }
    }

    fn tab_infos(&self, vi: usize) -> Vec<TabInfo> {
        let v = &self.viewers[vi];
        v.tabs
            .iter()
            .filter_map(|&i| {
                let c = self.conns.get(i)?;
                let reach = match c.state() {
                    State::Connected => TabReach::Live,
                    State::Connecting | State::Reconnecting => TabReach::Connecting,
                    State::Offline(_) => TabReach::Offline,
                };
                Some(TabInfo { conn: i, name: c.name.clone(), reach, selected: v.selected == Some(i) })
            })
            .collect()
    }

    /// One window: its picture, the remote cursor, the caption (stats, tabs, toggle), why there
    /// is no picture.
    pub fn refresh_viewer(&mut self, cx: &mut Cx, vi: usize) {
        let tabs = self.tab_infos(vi);
        let win = self.viewers[vi].window.clone();
        if let Some(mut t) = win.widget(cx, ids!(tabs)).borrow_mut::<DeskTabs>() {
            t.set_tabs(cx, tabs);
        }
        let open = self.viewers[vi].scrub_open;
        if let Some(mut t) = win.widget(cx, ids!(history_toggle)).borrow_mut::<CaptionToggle>() {
            t.set_on(cx, open);
        }
        let Some(ci) = self.viewers[vi].selected.filter(|i| *i < self.conns.len()) else {
            self.set_caption(cx, vi, "Desk Link".to_string(), String::new());
            if let Some(mut view) = win.widget(cx, ids!(remote)).borrow_mut::<RemoteView>() {
                view.note = Some("No desktop open here \u{b7} + opens one".to_string());
                view.lines.clear();
                view.remote_cursor = None;
                view.redraw(cx);
            }
            return;
        };
        let paused = self.viewers[vi].scrub.is_some();
        let c = &self.conns[ci];
        // paused: live pictures go on arriving (and recording) but are not shown
        let picture = c.latest.lock().unwrap().take().filter(|_| !paused);
        let history_line = {
            let h = c.history.lock().unwrap();
            format!("history {:.1} s · {} packets · {:.0} MB", h.span_ns() as f64 / 1e9, h.len(), h.bytes() as f64 / (1 << 20) as f64)
        };
        let (state, welcome, cursor, offset, fps, mbps, warning, stream_fps, read_gap_ms, decode_queue, stream_rate) = {
            let s = c.shared.lock().unwrap();
            (s.state.clone().unwrap_or(State::Connecting), s.welcome.clone(), s.cursor, s.offset_ns, s.fps, s.mbps, s.warning.clone(), s.stream_fps, s.read_gap_ms, s.decode_queue, s.stream_rate.clone())
        };
        let (host, port, name) = (c.host.clone(), c.port, c.name.clone());
        let details = self.details;
        let ready = self.viewers[vi].scrub.as_ref().and_then(Scrub::take_ready).map(|d| {
            let frame = self.viewers[vi].scrub.as_ref().map_or(0, |s| s.snap.pictures[d.index].frame);
            (d, frame)
        });
        let got_history = ready.is_some();
        let thumb = self.viewers[vi].scrub.as_ref().or(self.viewers[vi].preview.as_ref()).and_then(Scrub::take_thumb);
        let v = &mut self.viewers[vi];
        let remote = win.widget(cx, ids!(remote));
        let Some(mut view) = remote.borrow_mut::<RemoteView>() else { return };
        if let Some(p) = picture {
            // server times -> this clock: t_local = t_server - offset
            let local = |t: u64| t as i64 - offset;
            let l = &mut v.latency;
            l.encode = (p.encoded_ns as f64 - p.built_ns as f64) / 1e6;
            l.network = (p.received_ns as i64 - local(p.sent_ns)) as f64 / 1e6;
            l.decode = p.decode_ns as f64 / 1e6;
            v.decode_sum.0 += l.decode;
            v.decode_sum.1 += 1;
            view.set_picture(cx, p.width, p.height, p.parts, p.frame, p.decoded_ns);
        }
        if let Some((d, frame)) = ready {
            view.set_picture(cx, d.width, d.height, d.view_parts(), frame, 0);
            v.hist_shown = Some(d.index);
        }
        if paused {
            // a history picture's draw is no latency figure
            view.drawn = None;
        }
        if let Some((frame, decoded, drawn)) = view.drawn.take() {
            v.latency.present = drawn.saturating_sub(decoded) as f64 / 1e6;
            v.latency.total = v.latency.encode + v.latency.network + v.latency.decode + v.latency.present;
            v.last_stats = Some((frame, (v.latency.decode * 1000.0) as u32, (v.latency.present * 1000.0) as u32));
            v.shown += 1;
        }
        view.note = match &state {
            State::Connected => None,
            State::Connecting => Some(format!("connecting to {host}:{port} …")),
            State::Reconnecting => Some("reconnecting …".to_string()),
            State::Offline(why) => Some(why.clone()),
        };
        let mut lines = Vec::new();
        if details {
            if let Some(w) = &welcome {
                let screen = w.screens.get(w.screen as usize).map(|s| s.name.clone()).unwrap_or_default();
                lines.push(format!("{} · {screen} {}x{}{}", w.server, w.stream[0], w.stream[1], if w.input { String::new() } else { format!(" · {}", w.input_note) }));
                // how the picture is shown: the window in device pixels, the scale (whole: each
                // remote pixel a whole block, nothing resampled)
                lines.push(format!("window {}x{} px{} · scale {:.4}{}", v.window_px[0], v.window_px[1], if v.fullscreen { " (fullscreen)" } else { "" }, view.scale, if view.whole { " whole-pixel" } else { " resampled" }));
            }
            lines.push(crate::view::rate_details_line(fps, stream_fps, crate::conn::DISPLAY_HZ.load(std::sync::atomic::Ordering::Acquire), stream_rate.as_ref()));
            lines.push(crate::view::latency_line(&v.latency));
            lines.push(crate::view::flow_line(read_gap_ms, decode_queue));
            lines.push(history_line);
            if let Some(w) = &warning {
                lines.push(format!("WARNING: {w}"));
            }
        }
        view.lines = lines;
        // the caption's stats: the stream, and what the user must know (no input, a warning)
        let mut stats = match (&state, paused) {
            // paused: the stream goes on arriving (and recording), nothing is shown live
            (State::Connected, true) => format!("paused \u{b7} {:>3.0} Mbit/s", mbps.clamp(0.0, 9999.0)),
            (State::Connected, false) => crate::view::summary_line(v.shown_rate, mbps, stream_rate.as_ref()),
            _ => String::new(),
        };
        if let Some(w) = welcome.as_ref().filter(|w| !w.input && state == State::Connected) {
            stats.push_str(&format!(" \u{b7} {}", if w.input_note.is_empty() { "view only" } else { w.input_note.as_str() }));
        }
        if let Some(w) = &warning {
            // the caption says it short (the latency details have it whole)
            let short: String = w.chars().take(48).collect();
            stats.push_str(&format!(" \u{b7} \u{26a0} {short}{}", if w.chars().count() > 48 { "\u{2026}" } else { "" }));
        }
        // the remote cursor: the OS cursor takes its shape while ours is over the picture;
        // the sprite shows where it is when someone else moves it
        if let Some((x, y, shape, visible)) = cursor {
            let cursor = CURSORS.get(shape as usize).copied().unwrap_or(MouseCursor::Default);
            if v.local_over && !paused {
                cx.set_cursor(cursor);
            }
            let ours = v.last_pointer.is_some_and(|(px, py)| (px as i32 - x as i32).abs() < 200 && (py as i32 - y as i32).abs() < 200);
            view.remote_cursor = (visible && !(v.local_over && ours)).then_some((x, y));
        }
        view.redraw(cx);
        drop(view);
        self.set_caption(cx, vi, format!("Desk Link \u{b7} {name}"), stats);
        if let Some(d) = thumb {
            if let Some(mut s) = win.widget(cx, ids!(scrubber)).borrow_mut::<Scrubber>() {
                s.set_thumb(cx, &d);
            }
        }
        // the scrubber says what changed: a picture of the history decoded, a save going on
        // (live pictures arriving while paused change nothing there)
        if self.viewers[vi].scrub.is_some() && (got_history || cx.seconds_since_app_start() - self.clip_at < 10.0) {
            self.update_scrubber(cx, vi);
        }
    }

    fn set_caption(&mut self, cx: &mut Cx, vi: usize, title: String, stats: String) {
        let v = &mut self.viewers[vi];
        if v.title != title {
            v.title = title.clone();
            v.window.as_window().set_title(cx, &title);
        }
        if v.stats_text != stats {
            v.stats_text = stats.clone();
            let w = v.window.clone();
            let label = w.widget(cx, ids!(stats));
            label.set_text(cx, &stats);
            label.set_visible(cx, !stats.is_empty());
        }
    }

    /// Mouse, wheel and keys for the desktop window `vi` shows (live, over the picture).
    pub fn pointer_event(&mut self, cx: &mut Cx, vi: usize, event: &Event) {
        let win = self.viewers[vi].window.clone();
        let wid = self.viewers[vi].window_id();
        let over_ui = |cx: &mut Cx, abs: Vec2d| {
            let menu = win.widget(cx, ids!(menu));
            let menu_open = menu.borrow::<PopMenu>().is_some_and(|m| m.is_open());
            let bar = win.widget(cx, ids!(scrubber)).borrow::<Scrubber>().is_some_and(|s| s.covers(abs));
            menu_open || bar
        };
        let (placement, scrubbing) = match win.widget(cx, ids!(remote)).borrow::<RemoteView>() {
            Some(r) => (r.placement, self.viewers[vi].scrub.is_some()),
            None => return,
        };
        let Some(ci) = self.viewers[vi].selected else { return };
        let Some(conn) = self.conns.get(ci) else { return };
        let live = conn.state() == State::Connected && !scrubbing;
        let v = &mut self.viewers[vi];
        match event {
            Event::MouseMove(e) if Some(e.window_id) == wid => {
                if !live {
                    return;
                }
                let over = placement.to_stream(e.abs).filter(|_| !over_ui(cx, e.abs));
                v.local_over = over.is_some();
                let at = if v.held != 0 { Some(placement.to_stream_clamped(e.abs)) } else { over };
                if let Some((x, y)) = at.filter(|p| Some(*p) != v.last_pointer) {
                    v.last_pointer = Some((x, y));
                    conn.send(ClientMsg::Pointer { x, y });
                }
            }
            Event::MouseDown(e) if Some(e.window_id) == wid => {
                self.focused = vi;
                let v = &mut self.viewers[vi];
                let Some((x, y)) = placement.to_stream(e.abs).filter(|_| live && !over_ui(cx, e.abs)) else {
                    // a click outside the picture (the caption, the scrubber) takes the keyboard back
                    v.keyboard = false;
                    return;
                };
                v.keyboard = true;
                if let Some(b) = button_of(e.button) {
                    conn.send(ClientMsg::Pointer { x, y });
                    conn.send(ClientMsg::Button { down: true, button: b });
                    v.held |= 1 << b;
                }
            }
            Event::MouseUp(e) if Some(e.window_id) == wid => {
                if let Some(b) = button_of(e.button).filter(|b| v.held & (1 << b) != 0) {
                    v.held &= !(1 << b);
                    conn.send(ClientMsg::Button { down: false, button: b });
                }
            }
            Event::Scroll(e) if Some(e.window_id) == wid && live => {
                if placement.to_stream(e.abs).is_some() {
                    // makepad points (40 per wheel notch) -> 1/120 notches; +dy = wheel away
                    let dx = (e.scroll.x * 3.0).round() as i32;
                    let dy = (-e.scroll.y * 3.0).round() as i32;
                    if dx != 0 || dy != 0 {
                        conn.send(ClientMsg::Scroll { dx, dy });
                    }
                }
            }
            _ => {}
        }
    }

    /// A key in window `vi` (it has the keyboard): the viewer's shortcuts, the scrubber's keys
    /// while the keyboard is not driving the desktop, else the desktop's.
    pub fn key_event(&mut self, cx: &mut Cx, vi: usize, event: &Event) {
        let (e, down) = match event {
            Event::KeyDown(e) => (e, true),
            Event::KeyUp(e) => (e, false),
            _ => return,
        };
        if shortcut(e).is_some() {
            if down && !e.is_repeat {
                if let Some(s) = shortcut(e) {
                    self.do_shortcut(cx, vi, s);
                }
            }
            return;
        }
        // an open menu takes the keys (Escape closes it)
        let menu_open = self.viewers[vi].widget(cx, ids!(menu)).borrow::<PopMenu>().is_some_and(|m| m.is_open());
        if menu_open {
            return;
        }
        let v = &self.viewers[vi];
        let paused = v.scrub.is_some();
        if down && (paused || (!v.keyboard && cx.key_focus().is_empty())) && self.scrub_key(cx, vi, e) {
            return;
        }
        if self.viewers[vi].keyboard && !paused {
            if let Some(conn) = self.viewers[vi].selected.and_then(|i| self.conns.get(i)) {
                conn.send(ClientMsg::Key { down, keycode: e.key_code.to_index(), mods: mods_of(&e.modifiers) });
            }
        }
    }

    /// The scrubber's keys; false when the key is not one of them.
    fn scrub_key(&mut self, cx: &mut Cx, vi: usize, e: &KeyEvent) -> bool {
        let paused = self.viewers[vi].scrub.is_some();
        let open = self.viewers[vi].scrub_open;
        let big = if e.modifiers.shift { 10 } else { 1 };
        let poke = |app: &mut App, cx: &mut Cx| {
            if let Some(mut s) = app.viewers[vi].widget(cx, ids!(scrubber)).borrow_mut::<Scrubber>() {
                s.poke(cx);
            }
        };
        let action = match e.key_code {
            KeyCode::ArrowLeft => ScrubAction::Step(-big),
            KeyCode::ArrowRight if paused => ScrubAction::Step(big),
            KeyCode::Home if paused => ScrubAction::Seek(0),
            KeyCode::End if paused => ScrubAction::Seek(usize::MAX / 2),
            KeyCode::Space | KeyCode::KeyK if !e.is_repeat && (paused || open) => {
                if paused && self.viewers[vi].playing.is_none() && e.key_code == KeyCode::Space {
                    ScrubAction::Play(1.0)
                } else {
                    ScrubAction::Stop
                }
            }
            KeyCode::KeyL if !e.is_repeat && paused => {
                let s = self.viewers[vi].playing.map_or(0.0, |p| p.0);
                ScrubAction::Play(if s > 0.0 { (s * 2.0).min(4.0) } else { 1.0 })
            }
            KeyCode::KeyJ if !e.is_repeat && (paused || open) => {
                let s = self.viewers[vi].playing.map_or(0.0, |p| p.0);
                ScrubAction::Play(if s < 0.0 { (s * 2.0).max(-4.0) } else { -1.0 })
            }
            KeyCode::KeyI if !e.is_repeat && (paused || open) => ScrubAction::In,
            KeyCode::KeyO if !e.is_repeat && (paused || open) => ScrubAction::Out,
            KeyCode::Escape if !e.is_repeat && paused => ScrubAction::Live,
            KeyCode::Escape if !e.is_repeat && open => {
                self.close_scrubber(cx, vi);
                return true;
            }
            _ => return false,
        };
        poke(self, cx);
        self.scrub_action(cx, vi, action);
        true
    }

    /// The caption's history icon / Cmd+Shift+H: the scrubber opens (over the live history) or
    /// closes (back to live).
    pub fn toggle_scrubber(&mut self, cx: &mut Cx, vi: usize) {
        if self.viewers[vi].scrub_open {
            self.close_scrubber(cx, vi);
        } else {
            self.open_scrubber(cx, vi);
        }
    }

    pub fn open_scrubber(&mut self, cx: &mut Cx, vi: usize) {
        if self.viewers[vi].selected.is_none() {
            return;
        }
        self.viewers[vi].scrub_open = true;
        self.refresh_live_snap(cx, vi);
        self.update_snap_tick(cx);
        if let Some(mut s) = self.viewers[vi].widget(cx, ids!(scrubber)).borrow_mut::<Scrubber>() {
            s.poke(cx);
        }
        self.update_scrubber(cx, vi);
        self.refresh_viewer(cx, vi);
        selftest(&format!("scrubber open in window {vi}"));
    }

    pub fn close_scrubber(&mut self, cx: &mut Cx, vi: usize) {
        let v = &mut self.viewers[vi];
        v.scrub_open = false;
        v.preview = None;
        v.live_snap = None;
        if v.scrub.is_some() {
            self.leave_history(cx, vi);
        }
        self.update_snap_tick(cx);
        self.update_scrubber(cx, vi);
        self.refresh_viewer(cx, vi);
    }

    /// The live history's timeline follows a few times a second while a scrubber shows one.
    pub fn update_snap_tick(&mut self, cx: &mut Cx) {
        let wanted = self.viewers.iter().any(|v| v.scrub_open && v.scrub.is_none());
        let running = !self.snap_tick.is_empty();
        if wanted && !running {
            self.snap_tick = cx.start_interval(0.25);
        } else if !wanted && running {
            cx.stop_timer(self.snap_tick);
            self.snap_tick = Timer::empty();
        }
    }

    pub fn refresh_live_snap(&mut self, cx: &mut Cx, vi: usize) {
        let v = &self.viewers[vi];
        if !v.scrub_open || v.scrub.is_some() {
            return;
        }
        let Some(c) = v.selected.and_then(|i| self.conns.get(i)) else { return };
        let snap = Arc::new(c.history.lock().unwrap().snapshot());
        self.viewers[vi].live_snap = Some(snap);
        self.update_scrubber(cx, vi);
    }

    /// Pauses window `vi` on its history, at the picture nearest host time `at` (None: the
    /// newest).
    pub fn enter_history(&mut self, cx: &mut Cx, vi: usize, at: Option<u64>) {
        let Some(c) = self.viewers[vi].selected.and_then(|i| self.conns.get(i)) else { return };
        let snap = Arc::new(c.history.lock().unwrap().snapshot());
        if snap.len() == 0 {
            return;
        }
        // buttons held on the desktop are let go (the mouse now scrubs)
        let v = &self.viewers[vi];
        for b in 0..8u8 {
            if v.held & (1 << b) != 0 {
                c.send(ClientMsg::Button { down: false, button: b });
            }
        }
        let cur = at.map_or(snap.len() - 1, |t| snap.at_time(t));
        let scrub = Scrub::start(snap, cur, self.signal.clone());
        let v = &mut self.viewers[vi];
        v.held = 0;
        v.cur = cur;
        v.hist_shown = None;
        v.range = (None, None);
        v.playing = None;
        v.scrub = Some(scrub);
        v.preview = None;
        v.live_snap = None;
        v.scrub_open = true;
        self.update_snap_tick(cx);
        self.show_cur(cx, vi);
        self.refresh_viewer(cx, vi);
    }

    /// Back to live (the scrubber stays open over the live history).
    pub fn leave_history(&mut self, cx: &mut Cx, vi: usize) {
        let v = &mut self.viewers[vi];
        v.scrub = None;
        v.playing = None;
        v.hist_shown = None;
        v.range = (None, None);
        self.refresh_live_snap(cx, vi);
        self.update_snap_tick(cx);
        self.update_scrubber(cx, vi);
        self.refresh_viewer(cx, vi);
    }

    /// Shows picture `cur`: at once when it is decoded already, else when the scrub decoder
    /// has it (`Scrub::take_ready`, on the signal).
    pub fn show_cur(&mut self, cx: &mut Cx, vi: usize) {
        let win = self.viewers[vi].window.clone();
        let v = &mut self.viewers[vi];
        let Some(scrub) = &v.scrub else { return };
        if let Some(d) = scrub.seek(v.cur) {
            let frame = scrub.snap.pictures[d.index].frame;
            if let Some(mut r) = win.widget(cx, ids!(remote)).borrow_mut::<RemoteView>() {
                r.set_picture(cx, d.width, d.height, d.view_parts(), frame, 0);
            }
            v.hist_shown = Some(d.index);
        }
        if v.hist_shown != Some(v.cur) {
            // the scrubber follows the decode until the picture is there
            v.play_frame = cx.new_next_frame();
        }
        self.update_scrubber(cx, vi);
    }

    pub fn step(&mut self, cx: &mut Cx, vi: usize, delta: i64) {
        let v = &mut self.viewers[vi];
        let Some(n) = v.scrub.as_ref().map(|s| s.snap.len()) else { return };
        v.playing = None;
        v.cur = (v.cur as i64 + delta).clamp(0, n as i64 - 1) as usize;
        self.show_cur(cx, vi);
    }

    /// The scrubber's data (None: closed).
    pub fn update_scrubber(&mut self, cx: &mut Cx, vi: usize) {
        let v = &self.viewers[vi];
        let toast = if cx.seconds_since_app_start() - self.clip_at < 8.0 {
            let s = self.clip_status.lock().unwrap().clone();
            if s.chars().count() > 72 { format!("\u{2026}{}", s.chars().rev().take(71).collect::<Vec<_>>().into_iter().rev().collect::<String>()) } else { s }
        } else {
            String::new()
        };
        let data = if let Some(scrub) = &v.scrub {
            let (behind, ahead, work, failed) = scrub.status();
            let mut status = format!("decoded \u{2212}{behind}/+{ahead}");
            if v.hist_shown != Some(v.cur) {
                status.push_str(" · decoding");
            }
            if !work.is_empty() {
                status.push_str(&format!(" · {work}"));
            }
            if let Some(f) = failed {
                status.push_str(&format!(" · FAILED: {f}"));
            }
            Some(ScrubData { snap: scrub.snap.clone(), live: false, cur: v.cur, range: v.range, playing: v.playing.map(|p| p.0), status, toast, wall0: self.wall0 })
        } else if v.scrub_open {
            v.live_snap.clone().map(|snap| ScrubData { cur: snap.len().saturating_sub(1), snap, live: true, range: (None, None), playing: Some(1.0), status: "live".to_string(), toast, wall0: self.wall0 })
        } else {
            None
        };
        let win = v.window.clone();
        if let Some(mut s) = win.widget(cx, ids!(scrubber)).borrow_mut::<Scrubber>() {
            s.set_data(cx, data);
        }
    }

    /// The pictures to save: In..Out, from the one set to the shown one, or a second around it.
    pub fn save(&mut self, cx: &mut Cx, vi: usize, frame_only: bool) {
        let v = &self.viewers[vi];
        let (Some(scrub), Some(c)) = (&v.scrub, v.selected.and_then(|i| self.conns.get(i))) else { return };
        let n = scrub.snap.len();
        let (from, to) = match v.range {
            (Some(a), Some(b)) => (a.min(b), a.max(b)),
            (Some(a), None) | (None, Some(a)) => (a.min(v.cur), a.max(v.cur)),
            (None, None) => (v.cur.saturating_sub(60), (v.cur + 60).min(n - 1)),
        };
        let chosen = v.range != (None, None);
        let req = crate::clip::ClipRequest { snap: scrub.snap.clone(), from, to, shown: v.cur, frame_only, chosen, host: c.host.clone() };
        crate::clip::save(req, self.clip_status.clone(), self.signal.clone());
        self.clip_at = cx.seconds_since_app_start();
        self.update_scrubber(cx, vi);
    }

    /// The newest picture's host time of the live history shown (or of the connection).
    fn live_time(&self, vi: usize, i: usize) -> Option<u64> {
        let snap = self.viewers[vi].live_snap.as_ref()?;
        snap.pictures.get(i.min(snap.len().saturating_sub(1))).map(|p| p.built_ns)
    }

    pub fn scrub_action(&mut self, cx: &mut Cx, vi: usize, action: ScrubAction) {
        let paused = self.viewers[vi].scrub.is_some();
        match action {
            ScrubAction::Seek(i) if !paused => {
                let t = self.live_time(vi, i);
                self.enter_history(cx, vi, t);
            }
            ScrubAction::Seek(i) => {
                let v = &mut self.viewers[vi];
                v.playing = None;
                v.cur = i.min(v.scrub.as_ref().map_or(0, |s| s.snap.len().saturating_sub(1)));
                self.show_cur(cx, vi);
            }
            ScrubAction::Hover(Some(i)) => self.want_thumb(cx, vi, i),
            ScrubAction::Hover(None) => {}
            ScrubAction::Step(d) => {
                if !paused {
                    self.enter_history(cx, vi, None);
                }
                if d < 0 || paused {
                    self.step(cx, vi, d);
                }
            }
            ScrubAction::Live => self.leave_history(cx, vi),
            ScrubAction::Play(speed) => {
                if !paused {
                    self.enter_history(cx, vi, None);
                }
                let v = &mut self.viewers[vi];
                let Some(n) = v.scrub.as_ref().map(|s| s.snap.len()) else { return };
                // forward from the start when at the end, backward from the end when at the start
                if speed > 0.0 && v.cur + 1 >= n {
                    v.cur = 0;
                }
                if speed < 0.0 && v.cur == 0 {
                    v.cur = n - 1;
                }
                let built = v.scrub.as_ref().map_or(0, |s| s.snap.pictures[v.cur].built_ns);
                v.playing = Some((speed, -1.0, built));
                v.play_frame = cx.new_next_frame();
                self.update_scrubber(cx, vi);
            }
            ScrubAction::Stop if !paused => self.enter_history(cx, vi, None),
            ScrubAction::Stop => {
                self.viewers[vi].playing = None;
                self.update_scrubber(cx, vi);
            }
            ScrubAction::In | ScrubAction::Out => {
                if !paused {
                    self.enter_history(cx, vi, None);
                }
                let v = &mut self.viewers[vi];
                if matches!(action, ScrubAction::In) {
                    v.range.0 = Some(v.cur);
                } else {
                    v.range.1 = Some(v.cur);
                }
                self.update_scrubber(cx, vi);
            }
            ScrubAction::SaveMenu(r) => {
                if !paused {
                    self.enter_history(cx, vi, None);
                }
                let v = &self.viewers[vi];
                let clip = match v.range {
                    (Some(_), Some(_)) => "In to Out",
                    (Some(_), None) | (None, Some(_)) => "the mark to here",
                    _ => "a second around this picture",
                };
                let items = vec![MenuItem::new("Save clip", SAVE_CLIP).hint(clip), MenuItem::new("Save frame", SAVE_FRAME).hint("PNG + notes")];
                self.open_menu(cx, vi, MenuFor::Save, r, true, items);
            }
            ScrubAction::None => {}
        }
    }

    /// A thumbnail of picture `i` of the history the scrubber shows.
    fn want_thumb(&mut self, cx: &mut Cx, vi: usize, i: usize) {
        let v = &mut self.viewers[vi];
        if let Some(scrub) = &v.scrub {
            scrub.want_thumb(i);
            return;
        }
        let Some(snap) = v.live_snap.clone() else { return };
        let Some(t) = snap.pictures.get(i).map(|p| p.built_ns) else { return };
        // a preview of the live history: made again when its history is older than the moment
        let stale = v.preview.as_ref().is_none_or(|p| p.snap.pictures.last().is_none_or(|l| l.built_ns + 2_000_000_000 < t));
        if stale {
            v.preview = Some(Scrub::preview(snap, self.signal.clone()));
        }
        if let Some(p) = &v.preview {
            p.want_thumb(p.snap.at_time(t));
        }
        let _ = cx;
    }

    /// A frame while playing (the picture whose host time is due) or waiting for a decode.
    pub fn play_tick(&mut self, cx: &mut Cx, vi: usize, time: f64) {
        let v = &mut self.viewers[vi];
        let Some(scrub) = &v.scrub else { return };
        if let Some((speed, t0, built0)) = v.playing {
            let t0 = if t0 < 0.0 { time } else { t0 };
            v.playing = Some((speed, t0, built0));
            let due = (built0 as f64 + (time - t0) * speed * 1e9).max(0.0) as u64;
            let n = scrub.snap.len();
            let i = if speed >= 0.0 { scrub.snap.at_time(due).max(v.cur) } else { scrub.snap.at_time(due).min(v.cur) };
            let end = if speed >= 0.0 { i + 1 >= n } else { i == 0 };
            if i != v.cur {
                v.cur = i;
                let playing = v.playing;
                self.show_cur(cx, vi);
                self.viewers[vi].playing = playing;
            }
            if end {
                self.viewers[vi].playing = None;
            }
        }
        let v = &mut self.viewers[vi];
        if v.playing.is_some() || v.hist_shown != Some(v.cur) {
            v.play_frame = cx.new_next_frame();
        }
        self.update_scrubber(cx, vi);
    }

    pub fn open_menu(&mut self, cx: &mut Cx, vi: usize, what: MenuFor, anchor: Rect, above: bool, items: Vec<MenuItem>) {
        self.viewers[vi].menu_for = what;
        let win = self.viewers[vi].window.clone();
        if let Some(mut m) = win.widget(cx, ids!(menu)).borrow_mut::<PopMenu>() {
            m.open(cx, 0, anchor, above, items);
        }
    }

    /// The "+": the desktops open in no window.
    pub fn plus_menu(&mut self, cx: &mut Cx, vi: usize, anchor: Rect) {
        let mut items: Vec<MenuItem> = self
            .conns
            .iter()
            .enumerate()
            .filter(|(i, _)| self.viewer_of_conn(*i).is_none())
            .map(|(i, c)| {
                let hint = match c.state() {
                    State::Connected => c.shared.lock().unwrap().rtt_ms.map_or("live".to_string(), |r| format!("live \u{b7} {r:.0} ms")),
                    State::Connecting | State::Reconnecting => "connecting".to_string(),
                    State::Offline(_) => "offline".to_string(),
                };
                MenuItem::new(format!("{}  {}", c.name, c.host), i as u64).hint(hint)
            })
            .collect();
        if items.is_empty() {
            items.push(MenuItem::new("Every desktop is open", u64::MAX).hint("~/.makepad/desklink/hosts").disabled());
        }
        self.open_menu(cx, vi, MenuFor::Plus, anchor, false, items);
    }

    /// A tab's menu (a right click): open it in a window of its own, reconnect (only when it
    /// does not answer), close it.
    pub fn tab_menu(&mut self, cx: &mut Cx, vi: usize, conn: usize, anchor: Rect) {
        let key = if cfg!(target_vendor = "apple") { "\u{2318}R" } else { "Ctrl+Shift+R" };
        let mut items = vec![];
        let mut open = MenuItem::new("Open in new window", OPEN_NEW_WINDOW).hint("or drag the tab out");
        if self.viewers[vi].tabs.len() < 2 {
            open = open.disabled();
        }
        items.push(open);
        if self.conns.get(conn).is_some_and(|c| c.state() != State::Connected) {
            items.push(MenuItem::new("Reconnect", RECONNECT).hint(key));
        }
        let key = if cfg!(target_vendor = "apple") { "\u{2318}W" } else { "Ctrl+Shift+W" };
        items.push(MenuItem::new("Close Tab", CLOSE_TAB).hint(key));
        self.open_menu(cx, vi, MenuFor::Tab(conn), anchor, false, items);
    }

    pub fn menu_chose(&mut self, cx: &mut Cx, vi: usize, id: u64) {
        let what = std::mem::take(&mut self.viewers[vi].menu_for);
        match what {
            MenuFor::Plus if (id as usize) < self.conns.len() => self.add_tab(cx, vi, id as usize),
            MenuFor::Tab(conn) => match id {
                OPEN_NEW_WINDOW => {
                    self.new_window(cx, conn, None);
                }
                RECONNECT => self.reconnect(cx, conn),
                CLOSE_TAB => self.close_tab_closing_window(cx, vi, conn),
                _ => {}
            },
            MenuFor::Save => self.save(cx, vi, id == SAVE_FRAME),
            _ => {}
        }
    }

    /// The caption's tab strip.
    pub fn tabs_action(&mut self, cx: &mut Cx, vi: usize, action: crate::caption::TabsAction) {
        use crate::caption::TabsAction;
        self.focused = vi;
        match action {
            TabsAction::Select(conn) => {
                self.viewers[vi].keyboard = false;
                self.select(cx, vi, conn);
            }
            TabsAction::Menu(conn, r) => self.tab_menu(cx, vi, conn, r),
            TabsAction::Plus(r) => self.plus_menu(cx, vi, r),
            TabsAction::DragOut(conn, abs) => {
                if self.viewers[vi].tabs.len() > 1 {
                    let origin = self.viewers[vi].window.as_window().get_position(cx);
                    self.new_window(cx, conn, Some(origin + abs - dvec2(120.0, 14.0)));
                }
            }
            TabsAction::None => {}
        }
    }

    /// Visible window: once a second, its stats and its desktop's `Stats`.
    pub fn viewer_second(&mut self, vi: usize) {
        let v = &mut self.viewers[vi];
        let Some(c) = v.selected.and_then(|i| self.conns.get(i)) else { return };
        let dropped = std::mem::take(&mut c.shared.lock().unwrap().dropped);
        v.shown_rate = (std::mem::take(&mut v.shown), dropped);
        let (sum, n) = std::mem::take(&mut v.decode_sum);
        if let Some((frame, last_us, present_us)) = v.last_stats.take() {
            let decode_us = if n == 0 { last_us } else { (sum * 1000.0 / n as f64) as u32 };
            c.send(ClientMsg::Stats { frame, decode_us, present_us });
        }
    }

    /// `--check-scrubber` / `--check-split` once window 0 shows pictures: open the scrubber and
    /// hover the middle of its timeline; open the second tab in a window of its own.
    pub fn check_ui_tick(&mut self, cx: &mut Cx) {
        let have = self.viewers[0].selected.and_then(|i| self.conns.get(i)).is_some_and(|c| c.history.lock().unwrap().span_ns() > 12_000_000_000);
        if !have || self.check_ui.is_empty() {
            return;
        }
        for what in std::mem::take(&mut self.check_ui) {
            match what.as_str() {
                "scrubber" => {
                    self.open_scrubber(cx, 0);
                    let n = self.viewers[0].live_snap.as_ref().map_or(0, |s| s.len());
                    // a moment 10 s back: its thumbnail
                    let t = self.live_time(0, n.saturating_sub(1)).unwrap_or(0).saturating_sub(SPAN_NS / 3);
                    let i = self.viewers[0].live_snap.as_ref().map_or(0, |s| s.at_time(t));
                    self.want_thumb(cx, 0, i);
                    if let Some(mut s) = self.viewers[0].widget(cx, ids!(scrubber)).borrow_mut::<Scrubber>() {
                        s.set_hover(cx, i);
                    }
                    selftest(&format!("scrubber hovering picture {i} (10 s back)"));
                }
                "split" => {
                    if let Some(&conn) = self.viewers[0].tabs.get(1) {
                        self.new_window(cx, conn, None);
                    }
                }
                _ => {}
            }
        }
    }
}
