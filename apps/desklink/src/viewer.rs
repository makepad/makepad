//! A viewer window: the machine it shows (its tabs are every machine the app knows, the same
//! in every window), its input, its stats, and its history scrubber. The app keeps one `Viewer`
//! per window; any number of windows may show one machine (they share its connection).

use crate::caption::{CaptionToggle, DeskTabs, MenuItem, PopMenu, TabInfo, TabReach};
use crate::conn::{Latency, State};
use crate::held::Held;
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

/// A connection's newest picture as the windows show it: imported once, `seq` counting up.
#[derive(Clone)]
pub struct ConnPicture {
    pub seq: u64,
    pub parts: Arc<Vec<crate::view::ShownPart>>,
    pub width: u32,
    pub height: u32,
    pub frame: u32,
    pub built_ns: u64,
    pub encoded_ns: u64,
    pub sent_ns: u64,
    pub received_ns: u64,
    pub decoded_ns: u64,
    pub decode_ns: u64,
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
    /// The connection it shows (its selected tab).
    pub selected: Option<usize>,
    /// The connection's picture last shown (`App::pictures`, 0: none yet).
    pub pic_seq: u64,
    /// Keyboard to the desktop (the picture was clicked last).
    pub keyboard: bool,
    /// Keys and buttons held down on the desktop it drives (a drag keeps driving the picture
    /// when the pointer leaves it); let go whenever the window stops driving it.
    pub input: Held,
    pub last_pointer: Option<(u16, u16)>,
    /// Scroll below a 1/120 notch, carried to the next (trackpad pixels stay smooth).
    pub scroll_rest: (f64, f64),
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
    /// A tab's menu.
    Tab(usize),
    /// The scrubber's Save menu.
    Save,
}

const OPEN_NEW_WINDOW: u64 = 1;
const RECONNECT: u64 = 2;
const SAVE_CLIP: u64 = 1;
const SAVE_FRAME: u64 = 2;

impl Viewer {
    pub fn new(window: WidgetRef, extra: bool) -> Viewer {
        Viewer {
            window,
            extra,
            drawn: false,
            visible: true,
            selected: None,
            pic_seq: 0,
            keyboard: false,
            input: Held::default(),
            last_pointer: None,
            scroll_rest: (0.0, 0.0),
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

    /// Pauses every connection that no visible window shows and resumes the shown ones (each
    /// window streams its machine live, whatever the others show).
    pub fn sync_shown(&self) {
        for (i, c) in self.conns.iter().enumerate() {
            c.show(self.viewers.iter().any(|v| v.visible && v.selected == Some(i)));
        }
    }

    /// Window `vi` decides the size of the desktop it shows (a desktop that follows its viewer
    /// takes it): the window that showed it or had the focus last, of the windows showing it.
    pub fn claim_size(&mut self, cx: &mut Cx, vi: usize) {
        let Some(conn) = self.viewers.get(vi).and_then(|v| v.selected) else { return };
        let sink = self.conns.get(conn).map(|c| c.view.clone());
        for (k, v) in self.viewers.iter().enumerate() {
            if v.selected != Some(conn) {
                continue;
            }
            if let Some(mut r) = v.window.widget(cx, ids!(remote)).borrow_mut::<RemoteView>() {
                r.sink = if k == vi { sink.clone() } else { None };
                r.redraw(cx);
            }
        }
    }

    /// Shows connection `conn` in window `vi` (any other window may show it too: they share its
    /// connection and its pictures).
    pub fn select(&mut self, cx: &mut Cx, vi: usize, conn: usize) {
        if conn >= self.conns.len() || self.viewers[vi].selected == Some(conn) {
            return;
        }
        // a desktop that does not answer is tried again now (no waiting out its backoff)
        if matches!(self.conns[conn].state(), State::Offline(_)) {
            self.conns[conn].reconnect();
        }
        if self.viewers[vi].scrub_open {
            // the history is the shown connection's
            self.close_scrubber(cx, vi);
        }
        self.release_held(vi, "tab changed");
        let v = &mut self.viewers[vi];
        v.selected = Some(conn);
        v.last_pointer = None;
        v.keyboard = false;
        v.pic_seq = 0;
        if let Some(mut r) = self.viewers[vi].widget(cx, ids!(remote)).borrow_mut::<RemoteView>() {
            r.clear(cx);
            r.sink = None;
        }
        self.claim_size(cx, vi);
        self.sync_shown();
        self.restore_dirty = true;
        self.refresh_viewer(cx, vi);
        selftest(&format!("window {vi} shows {}", self.conns[conn].name));
    }

    /// Closes window `vi` as its caption's close button does (Cmd+W): the first window hides
    /// when `keep_on_close` says so, else it closes.
    pub fn close_window(&mut self, cx: &mut Cx, vi: usize) {
        if self.keep_on_close(vi) {
            self.hide_main(cx);
        } else {
            self.viewers[vi].window.as_window().close(cx);
        }
    }

    /// A new window showing connection `conn` (None: the first machine no window shows, else
    /// the focused window's), at `at`/`size` (screen points; else beside the focused window,
    /// half its width: two machines side by side). Nothing moves: every window lists every
    /// machine.
    pub fn new_window(&mut self, cx: &mut Cx, conn: Option<usize>, at: Option<(Vec2d, Vec2d)>) -> usize {
        let window = make_window(cx);
        self.place_new_window(cx, &window, at);
        self.viewers.push(Viewer::new(window, true));
        let vi = self.viewers.len() - 1;
        let shown: Vec<usize> = self.viewers.iter().filter(|v| v.visible).filter_map(|v| v.selected).collect();
        let focused = self.viewers.get(self.focused).and_then(|v| v.selected);
        let conn = conn.or_else(|| (0..self.conns.len()).find(|i| !shown.contains(i))).or(focused).or((!self.conns.is_empty()).then_some(0));
        if let Some(conn) = conn {
            self.select(cx, vi, conn);
        }
        self.focused = vi;
        self.restore_dirty = true;
        self.refresh_viewer(cx, vi);
        selftest(&format!("window {vi} made for {}", conn.and_then(|c| self.conns.get(c)).map_or("nothing", |c| c.name.as_str())));
        cx.redraw_all();
        vi
    }

    /// Where a new window goes, before the platform makes it: `at` (position, inner size; a
    /// restored window), else beside the focused window at half its width.
    fn place_new_window(&self, cx: &mut Cx, window: &WidgetRef, at: Option<(Vec2d, Vec2d)>) {
        let (pos, size) = match at {
            Some(at) => at,
            None => {
                let src = self.focused.min(self.viewers.len().saturating_sub(1));
                let (pos, size) = match self.viewers.get(src) {
                    Some(v) => {
                        let w = v.window.as_window();
                        (w.get_position(cx), w.get_inner_size(cx))
                    }
                    None => (dvec2(80.0, 80.0), dvec2(1400.0, 860.0)),
                };
                let size = dvec2((size.x * 0.5).max(640.0), size.y.max(400.0));
                (pos + dvec2(size.x.min(700.0), 0.0), size)
            }
        };
        if let Some(id) = window.as_window().window_id() {
            if cx.windows.is_valid(id) {
                let w = &mut cx.windows[id];
                w.create_inner_size = Some(size);
                w.create_position = Some(pos);
            }
        }
    }

    /// A window made at run time closed: what it held goes up; its machines stay (every window
    /// lists them all), paused when no window shows them.
    pub fn window_closed(&mut self, cx: &mut Cx, vi: usize) {
        self.release_held(vi, "window closed");
        let v = self.viewers.remove(vi);
        // another window on the same machine sizes it now
        if let Some(other) = v.selected.and_then(|c| self.viewers.iter().position(|o| o.visible && o.selected == Some(c))) {
            self.claim_size(cx, other);
        }
        drop(v);
        if self.focused >= self.viewers.len() || self.focused == vi {
            self.focused = 0;
        } else if self.focused > vi {
            self.focused -= 1;
        }
        self.sync_shown();
        self.restore_dirty = true;
        // no window left on screen: the app ends (the installed Dock app stays in the Dock)
        if !crate::installed::is_dock_app() && self.viewers.iter().all(|v| !v.visible) {
            self.save_windows(cx);
            cx.quit();
        }
    }

    /// Whether closing window `vi` keeps it (hidden, its machine kept): the first window of the
    /// installed Dock app (it stays in the Dock) or while another window shows.
    pub fn keep_on_close(&self, vi: usize) -> bool {
        vi == 0 && (crate::installed::is_dock_app() || self.viewers.iter().skip(1).any(|v| v.visible))
    }

    /// A window on screen when none is (the app opened again): the first one.
    pub fn show_some_window(&mut self, cx: &mut Cx) {
        if self.viewers.iter().any(|v| v.visible) {
            return;
        }
        self.viewers[0].visible = true;
        self.viewers[0].keyboard = false;
        self.viewers[0].window.as_window().show(cx);
        self.focused = 0;
        self.claim_size(cx, 0);
        self.sync_shown();
        self.restore_dirty = true;
        selftest("opened again: window shown");
    }

    /// The first window closed: hidden (its machine pauses unless another window shows it).
    pub fn hide_main(&mut self, cx: &mut Cx) {
        self.release_held(0, "window closed");
        self.viewers[0].visible = false;
        if self.viewers[0].scrub_open {
            self.close_scrubber(cx, 0);
        }
        self.viewers[0].window.as_window().set_visible(cx, false);
        // another window on the same machine sizes it now
        if let Some(other) = (1..self.viewers.len()).find(|k| self.viewers[*k].visible && self.viewers[*k].selected.is_some() && self.viewers[*k].selected == self.viewers[0].selected) {
            self.claim_size(cx, other);
        }
        self.sync_shown();
        self.restore_dirty = true;
        selftest("first window closed: hidden");
    }

    /// Drops connection `i`'s session and opens it again now (a tab's "Reconnect", Cmd+R).
    pub fn reconnect(&mut self, cx: &mut Cx, i: usize) {
        if let Some(c) = self.conns.get(i) {
            c.reconnect();
        }
        for v in &mut self.viewers {
            if v.input.on.is_some_and(|(c, _)| c == i) {
                // the session ends: the desktop lets go of what it held
                v.input.release();
                v.last_pointer = None;
            }
        }
        self.refresh(cx);
    }

    pub fn next_screen(&mut self, vi: usize) {
        self.release_held(vi, "screen changed");
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
                if k < self.conns.len() {
                    self.select(cx, vi, k);
                }
            }
            Shortcut::Reconnect => {
                if let Some(i) = self.viewers[vi].selected {
                    self.reconnect(cx, i);
                }
            }
            Shortcut::NewWindow => {
                self.new_window(cx, None, None);
            }
            Shortcut::NextTab(d) => {
                let n = self.conns.len() as i32;
                if n > 0 {
                    let at = self.viewers[vi].selected.map_or(0, |k| k as i32 + d);
                    self.select(cx, vi, at.rem_euclid(n) as usize);
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
            Shortcut::CloseWindow => self.close_window(cx, vi),
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
        // every machine the app knows, in every window; only the selection differs
        let v = &self.viewers[vi];
        self.conns
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
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
        self.take_picture(cx, ci);
        // paused: live pictures go on arriving (and recording) but are not shown; a picture is
        // shown once in each window on this machine
        let picture = self.pictures.get(&ci).filter(|p| !paused && p.seq != self.viewers[vi].pic_seq).cloned();
        let c = &self.conns[ci];
        let history_line = {
            let h = c.history.lock().unwrap();
            format!("history {:.1} s · {} packets · {:.0} MB", h.span_ns() as f64 / 1e9, h.len(), h.bytes() as f64 / (1 << 20) as f64)
        };
        let (state, welcome, cursor, offset, fps, mbps, warning, stream_fps, read_gap_ms, decode_queue, stream_rate, machine) = {
            let s = c.shared.lock().unwrap();
            (s.state.clone().unwrap_or(State::Connecting), s.welcome.clone(), s.cursor, s.offset_ns, s.fps, s.mbps, s.warning.clone(), s.stream_fps, s.read_gap_ms, s.decode_queue, s.stream_rate.clone(), s.machine.clone())
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
        if state != State::Connected && v.input.on.is_some_and(|(c, _)| c == ci) {
            // the session is gone: the desktop let go of what it held
            v.input.release();
        }
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
            v.pic_seq = p.seq;
            view.set_shared(cx, p.width, p.height, p.parts, p.frame, p.decoded_ns);
        }
        if let Some(w) = welcome.as_ref().filter(|_| state == State::Connected && !paused) {
            // the picture area is known before the first picture: input maps from the start
            view.await_picture(cx, w.stream);
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
        let host_note = stream_rate.as_ref().filter(|r| stream_fps == Some(0) && !r.why.is_empty()).map(|r| r.why.clone());
        view.note = match &state {
            State::Connected => host_note.clone(),
            State::Connecting => host_note.or_else(|| Some(format!("connecting to {host}:{port} …"))),
            State::Reconnecting => Some("reconnecting …".to_string()),
            State::Offline(why) => Some(why.clone()),
        };
        let mut lines = Vec::new();
        if details {
            if let Some(w) = &welcome {
                let screen = w.screens.get(w.screen as usize).map(|s| s.name.clone()).unwrap_or_default();
                // the desktop's own name, and the fleet machine its certificate proved
                let machine = machine.as_ref().map(|m| format!(" (fleet: {m})")).unwrap_or_default();
                lines.push(format!("{}{machine} · {screen} {}x{}{}", w.server, w.stream[0], w.stream[1], if w.input { String::new() } else { format!(" · {}", w.input_note) }));
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

    /// Connection `ci`'s newest decoded picture, imported once (its textures shared by every
    /// window showing it).
    fn take_picture(&mut self, cx: &mut Cx, ci: usize) {
        let Some(p) = self.conns[ci].latest.lock().unwrap().take() else { return };
        let seq = self.pictures.get(&ci).map_or(1, |o| o.seq + 1);
        let parts = Arc::new(crate::view::import_parts(cx, p.parts, &mut Vec::new()));
        let shown = ConnPicture { seq, parts, width: p.width, height: p.height, frame: p.frame, built_ns: p.built_ns, encoded_ns: p.encoded_ns, sent_ns: p.sent_ns, received_ns: p.received_ns, decoded_ns: p.decoded_ns, decode_ns: p.decode_ns };
        self.pictures.insert(ci, shown);
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
                let at = if v.input.dragging() { Some(placement.to_stream_clamped(e.abs)) } else { over };
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
                    self.input_on(vi, ci);
                    let conn = &self.conns[ci];
                    conn.send(ClientMsg::Pointer { x, y });
                    conn.send(ClientMsg::Button { down: true, button: b });
                    self.viewers[vi].input.button_down(b);
                }
            }
            Event::MouseUp(e) if Some(e.window_id) == wid => {
                if let Some(b) = button_of(e.button).filter(|b| v.input.button_up(*b)) {
                    self.send_held(vi, ClientMsg::Button { down: false, button: b });
                }
            }
            Event::Scroll(e) if Some(e.window_id) == wid && live => {
                if placement.to_stream(e.abs).is_some() {
                    // in the content direction this machine's user saw (its OS setting
                    // applied, e.g. a Mac's natural scrolling)
                    let (dx, dy) = makepad_desk_link::scroll_to_wire(&mut v.scroll_rest, e.scroll.x, e.scroll.y);
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
        // the up of a key whose down went to the desktop goes there too, whatever took the
        // keyboard since (a shortcut, a menu, the scrubber); any other up is the viewer's
        if !down && self.viewers[vi].input.key_up(e.key_code.to_index()) {
            self.send_held(vi, ClientMsg::Key { down: false, keycode: e.key_code.to_index(), mods: mods_of(&e.modifiers) });
            return;
        }
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
        if down && self.viewers[vi].keyboard && !paused {
            if let Some(ci) = self.viewers[vi].selected.filter(|i| *i < self.conns.len()) {
                self.input_on(vi, ci);
                let keycode = e.key_code.to_index();
                self.conns[ci].send(ClientMsg::Key { down, keycode, mods: mods_of(&e.modifiers) });
                self.viewers[vi].input.key_down(keycode);
            }
        }
    }

    /// Window `vi` drives connection `ci` now: what it held on another desktop (or on an ended
    /// session) is let go first.
    fn input_on(&mut self, vi: usize, ci: usize) {
        let session = self.conns[ci].session();
        if self.viewers[vi].input.on != Some((ci, session)) {
            self.release_held(vi, "desktop changed");
            self.viewers[vi].input.on = Some((ci, session));
        }
    }

    /// An up for a down window `vi` sent: to the desktop (and session) that got the down.
    fn send_held(&self, vi: usize, msg: ClientMsg) {
        if let Some((ci, session)) = self.viewers[vi].input.on {
            if let Some(c) = self.conns.get(ci).filter(|c| c.session() == session) {
                c.send(msg);
            }
        }
    }

    /// Window `vi` stops driving its desktop (it lost the focus, its tab changed, the history
    /// paused it, it closed, the app quits): the up of every key and button it holds down
    /// there goes out, modifiers last, and nothing is held any more.
    pub fn release_held(&mut self, vi: usize, why: &str) {
        let Some(v) = self.viewers.get_mut(vi) else { return };
        let on = v.input.on;
        let ups = v.input.release();
        let Some((ci, session)) = on.filter(|_| !ups.is_empty()) else { return };
        // a session that ended took its downs with it: the desktop let them go
        let Some(c) = self.conns.get(ci).filter(|c| c.session() == session) else { return };
        let n = ups.len();
        for m in ups {
            c.send(m);
        }
        selftest(&format!("window {vi}: {n} held released on {} ({why})", c.name));
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
        self.release_held(vi, "scrubber opened");
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
        // keys and buttons held on the desktop are let go (the mouse and keys now scrub)
        self.release_held(vi, "history");
        let cur = at.map_or(snap.len() - 1, |t| snap.at_time(t));
        let scrub = Scrub::start(snap, cur, self.signal.clone());
        let v = &mut self.viewers[vi];
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

    /// A tab's menu (a right click): the machine in a new window (this one keeps what it
    /// shows), reconnect (only when it does not answer).
    pub fn tab_menu(&mut self, cx: &mut Cx, vi: usize, conn: usize, anchor: Rect) {
        let key = if cfg!(target_vendor = "apple") { "\u{2318}R" } else { "Ctrl+Shift+R" };
        let mut items = vec![MenuItem::new("Open in new window", OPEN_NEW_WINDOW)];
        if self.conns.get(conn).is_some_and(|c| c.state() != State::Connected) {
            items.push(MenuItem::new("Reconnect", RECONNECT).hint(key));
        }
        self.open_menu(cx, vi, MenuFor::Tab(conn), anchor, false, items);
    }

    pub fn menu_chose(&mut self, cx: &mut Cx, vi: usize, id: u64) {
        let what = std::mem::take(&mut self.viewers[vi].menu_for);
        match what {
            MenuFor::Tab(conn) => match id {
                OPEN_NEW_WINDOW => {
                    self.new_window(cx, Some(conn), None);
                }
                RECONNECT => self.reconnect(cx, conn),
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
            TabsAction::Select(conn) => self.select(cx, vi, conn),
            TabsAction::Menu(conn, r) => self.tab_menu(cx, vi, conn, r),
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
                    if self.conns.len() > 1 {
                        self.new_window(cx, Some(1), None);
                    }
                }
                _ => {}
            }
        }
    }
}
