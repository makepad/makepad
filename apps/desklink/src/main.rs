//! Desk Link — the viewer of the window manager's remote desktop.
//!
//! The window is the picture alone. The desktops it knows (saved hosts in
//! `~/.makepad/desklink/hosts`, `--connect host[:port]`; never this machine itself) are each
//! kept open and paused while not shown; the menu-bar (tray) icon's dropdown
//! (`menu`) lists them with whether they answer and their round trip, and a click shows one in
//! the window (opening or raising it). Closing the window keeps the app in the menu bar (the
//! shown desktop pauses). The picture is the remote screen at its own resolution, scaled to the
//! window at a whole-pixel scale where it fits (aspect-locked, letterboxed); input is mapped back
//! through the same placement. Keyboard input goes to the desktop once the picture was clicked.
//! The stream is the desktop's choice (codec and rate auto).
//!
//! Shortcuts (Cmd on macOS, Ctrl+Shift elsewhere; never sent to the desktop): Cmd+1..9 shows
//! desktop N, Cmd+R reconnects, Cmd+Shift+N the desktop's next screen, Cmd+Shift+L latency
//! details, Cmd+Shift+H pauses into the history (the left arrow too, while the keyboard is not
//! driving the desktop) and back to live. The tray's right-click menu has the same.
//!
//! Dev flags: `--connect host[:port]` (repeatable; `--connect-only`: no other desktops), `--select N`, `--stats` (latency lines on
//! stderr once a second), `--dump-check <file.h264>` (decode an Annex-B dump with the hardware
//! decoder, print what came out, exit), `--dump-play <dump dir | file.h264[@x0,y0],...>` (show a dump, or the tiles of one, as a desktop, at
//! 60 pictures a second, looping; `--cpu-decode` takes the CPU NV12 path instead of GPU pictures, `--still N` stops at picture N),
//! `--decode-bench <dir>` (the tile streams of a `desk-link-probe --dump`: each on its own decoder
//! thread as the viewer runs them, every picture's tiles at once; prints the decode time of a
//! whole picture and the pictures a second the decoders sustain, exit).
//! `--scrub-check SECS`: after SECS of pictures, pause, scrub the history (steps back and
//! forward, a far jump, a saved clip), print the timings on stderr and exit.
//!
//! Reconnect: Cmd+R (Ctrl+Shift+R elsewhere) drops the shown desktop's session and opens it
//! again at once.
//!
//! History: every connection keeps the last 30 s of its received bitstream. Cmd+Shift+H (or the
//! left arrow while the keyboard is not driving the desktop) freezes it into a timeline:
//! left / right step one picture (shift: ten), Home / End, space plays, Escape goes back to live;
//! "Save clip" writes the range (In / Out, else a second around the shown picture) to
//! local/screencap: the bitstream as received, PNGs and frame notes.

mod clip;
mod conn;
mod history;
mod menu;
mod scrub;
mod view;

pub use makepad_widgets;
use conn::{Connection, Latency, State};
use makepad_desk_link::{self as proto, ClientMsg, GamepadState};
use makepad_widgets::*;
use scrub::Scrub;
use std::sync::{Arc, Mutex};
use makepad_desk_link_panel::{DesktopRow, Reach};
use makepad_shell_kit::tray_app::{DropdownChange, TrayDropdown};
use menu::{DeskMenuAction, DeskMenuView};
use view::{RemoteView, Timeline, TimelineAction, TimelineData};

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Desk Link" window.inner_size: vec2(1400, 860)
                pass +: {clear_color: #x101114}
                body +: {
                    padding: 0 spacing: 0
                    View{
                        width: Fill height: Fill
                        flow: Overlay
                            remote := RemoteView{}
                            timeline := Timeline{}
                    }
                }
            }
            // the menu-bar icon's dropdown: the desktops
            menu_window := Window{
                create_on_start: false
                show_caption_bar: false
                chrome: WindowChrome.None
                window.title: "Desk Link"
                window.dropdown: true
                window.inner_size: vec2(360, 400)
                body +: {
                    menu_view := mod.widgets.DeskMenuView{card: true}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    conns: Vec<Connection>,
    #[rust]
    selected: usize,
    #[rust]
    signal: SignalToUI,
    /// Keyboard to the desktop (the picture was clicked last).
    #[rust]
    keyboard: bool,
    /// Pointer buttons held over the picture (they keep driving it when the pointer leaves).
    #[rust]
    held: u32,
    #[rust]
    last_pointer: Option<(u16, u16)>,
    #[rust]
    local_over: bool,
    #[rust]
    pads: Vec<Option<GamepadState>>,
    #[rust]
    pad_poll: NextFrame,
    #[rust]
    tick: Timer,
    /// Measures this display's refresh (vsync to vsync), told to every desktop.
    #[rust]
    vsync: NextFrame,
    #[rust]
    vsync_times: Vec<f64>,
    #[rust]
    fullscreen: bool,
    /// The window shows (closed: the app lives in the menu bar, the shown desktop paused).
    #[rust(true)]
    window_shown: bool,
    #[rust(TrayDropdown::new("desklink", TRAY, 360.0))]
    dropdown: TrayDropdown,
    #[rust]
    tray_key: String,
    /// The window's inner size in device pixels (fullscreen: the screen's backing size).
    #[rust]
    window_px: [u32; 2],
    #[rust]
    stats_log: bool,
    /// The overlay shows the per-stage latency (the panel's "Latency details", or `--stats`).
    #[rust]
    details: bool,
    #[rust]
    latency: Latency,
    /// Pictures drawn this second so far, and over the last second: (shown, decoded but
    /// never shown) a second.
    #[rust]
    shown: u32,
    #[rust]
    shown_rate: (u32, u32),
    /// Present times waiting for their report: (frame, decode_us, present_us).
    #[rust]
    last_stats: Option<(u32, u32, u32)>,
    /// Decode times (ms, a picture's slowest stream) of the pictures taken since the last
    /// `Stats`, summed, and how many: `Stats` reports their mean (the desktop sizes its bands by
    /// it).
    #[rust]
    decode_sum: (f64, u32),
    /// The paused history (None: live).
    #[rust]
    scrub: Option<Scrub>,
    /// The picture of the history to show, and the one shown.
    #[rust]
    cur: usize,
    #[rust]
    hist_shown: Option<usize>,
    /// The range to save (In / Out).
    #[rust]
    range: (Option<usize>, Option<usize>),
    /// Playing the history: (speed, start time, the picture's host time at the start).
    #[rust]
    playing: Option<(f64, f64, u64)>,
    #[rust]
    play_frame: NextFrame,
    #[rust]
    clip_status: Arc<Mutex<String>>,
    /// `--scrub-check`.
    #[rust]
    check: Option<Check>,
}

/// `--scrub-check`: a scripted scrub of the history, timed on the UI thread.
struct Check {
    /// Seconds of pictures before pausing.
    after: f64,
    started: Option<std::time::Instant>,
    /// The steps still to do: picture deltas (`JUMP`: to the middle, `SAVE`: a clip).
    steps: std::collections::VecDeque<i64>,
    /// The step being waited for: (its kind, when it was asked).
    waiting: Option<(i64, std::time::Instant)>,
    /// (kind, ms until the picture showed).
    times: Vec<(i64, f64)>,
    last_step: Option<std::time::Instant>,
    /// `--check-reconnect`: reconnect halfway through the live part; (asked at, seen offline,
    /// ms until connected again).
    reconnect: Option<(std::time::Instant, bool, Option<f64>)>,
    reconnect_wanted: bool,
    created: std::time::Instant,
    timer: Timer,
}

impl Check {
    const JUMP: i64 = i64::MIN + 1;
    const SAVE: i64 = i64::MIN;
    /// The first picture shown after pausing.
    const FIRST: i64 = 0;
}

fn port_of(spec: &str) -> (String, u16) {
    match spec.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && p.parse::<u16>().is_ok() && !h.contains(']') || h.ends_with(']') && p.parse::<u16>().is_ok() => {
            (h.trim_start_matches('[').trim_end_matches(']').to_string(), p.parse().unwrap())
        }
        _ => (spec.trim_start_matches('[').trim_end_matches(']').to_string(), proto::DEFAULT_PORT),
    }
}

/// Whether `host` (an address) is this machine.
fn is_self(host: &str) -> bool {
    let Ok(ip) = host.parse::<std::net::IpAddr>() else { return false };
    ip.is_loopback() || makepad_core_util::netif::interfaces().iter().any(|i| std::net::IpAddr::V4(i.addr) == ip)
}

/// The desktops to list: `--connect` ones and saved hosts (the bool: found by discovery rather
/// than named; retried rarely while offline).
fn hosts() -> Vec<(String, String, u16, bool)> {
    let mut out: Vec<(String, String, u16, bool)> = Vec::new();
    let mut put = |name: String, spec: &str, discovered: bool| {
        let (host, port) = port_of(spec);
        if !host.is_empty() && !out.iter().any(|(_, h, p, _)| *h == host && *p == port) {
            out.push((name, host, port, discovered));
        }
    };
    let mut add = |name: String, spec: &str| put(name, spec, false);
    let args: Vec<String> = std::env::args().collect();
    for w in args.windows(2) {
        if w[0] == "--connect" {
            add(w[1].clone(), &w[1]);
        }
    }
    // `--connect-only`: just the `--connect` desktops (a check that must not knock on other machines)
    if args.iter().any(|a| a == "--connect-only") {
        return out;
    }
    let home = makepad_network::fleet::makepad_home();
    if let Ok(text) = std::fs::read_to_string(home.join("desklink/hosts")) {
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let mut parts = line.split_whitespace();
            match (parts.next(), parts.next()) {
                // this machine is never one of its desktops (`--connect` may still name it)
                (Some(_), Some(spec)) | (Some(spec), None) if is_self(&port_of(spec).0) => {}
                (Some(name), Some(spec)) => add(name.to_string(), spec),
                (Some(spec), None) => add(spec.to_string(), spec),
                _ => {}
            }
        }
    }
    out
}

const CURSORS: [MouseCursor; 36] = [
    MouseCursor::Hidden,
    MouseCursor::Default,
    MouseCursor::Crosshair,
    MouseCursor::Hand,
    MouseCursor::Arrow,
    MouseCursor::Move,
    MouseCursor::Text,
    MouseCursor::Wait,
    MouseCursor::Help,
    MouseCursor::NotAllowed,
    MouseCursor::Grab,
    MouseCursor::Grabbing,
    MouseCursor::Default, // Progress: no such shape here
    MouseCursor::Default, // ContextMenu: no such shape here
    MouseCursor::Default, // Cell: no such shape here
    MouseCursor::Default, // VerticalText: no such shape here
    MouseCursor::Default, // Alias: no such shape here
    MouseCursor::Default, // Copy: no such shape here
    MouseCursor::Default, // NoDrop: no such shape here
    MouseCursor::Default, // AllScroll: no such shape here
    MouseCursor::Default, // ZoomIn: no such shape here
    MouseCursor::Default, // ZoomOut: no such shape here
    MouseCursor::NResize,
    MouseCursor::NeResize,
    MouseCursor::EResize,
    MouseCursor::SeResize,
    MouseCursor::SResize,
    MouseCursor::SwResize,
    MouseCursor::WResize,
    MouseCursor::NwResize,
    MouseCursor::NsResize,
    MouseCursor::NeswResize,
    MouseCursor::EwResize,
    MouseCursor::NwseResize,
    MouseCursor::ColResize,
    MouseCursor::RowResize,
];

fn mods_of(m: &KeyModifiers) -> u8 {
    (m.shift as u8 * proto::mods::SHIFT) | (m.control as u8 * proto::mods::CONTROL) | (m.alt as u8 * proto::mods::ALT) | (m.logo as u8 * proto::mods::LOGO)
}

fn button_of(b: MouseButton) -> Option<u8> {
    if b.is_primary() {
        Some(proto::button::LEFT)
    } else if b.is_secondary() {
        Some(proto::button::RIGHT)
    } else if b.is_middle() {
        Some(proto::button::MIDDLE)
    } else if b.contains(MouseButton::BACK) {
        Some(proto::button::BACK)
    } else if b.contains(MouseButton::FORWARD) {
        Some(proto::button::FORWARD)
    } else {
        None
    }
}

fn pad_of(s: &makepad_widgets::makepad_platform::event::game_input::GamepadState) -> GamepadState {
    use proto::pad;
    let mut b = 0;
    for (v, bit) in [
        (s.a, pad::A),
        (s.b, pad::B),
        (s.x, pad::X),
        (s.y, pad::Y),
        (s.left_shoulder, pad::LEFT_SHOULDER),
        (s.right_shoulder, pad::RIGHT_SHOULDER),
        (s.select, pad::SELECT),
        (s.start, pad::START),
        (s.home, pad::HOME),
        (s.left_thumb, pad::LEFT_THUMB),
        (s.right_thumb, pad::RIGHT_THUMB),
        (s.dpad_up, pad::DPAD_UP),
        (s.dpad_down, pad::DPAD_DOWN),
        (s.dpad_left, pad::DPAD_LEFT),
        (s.dpad_right, pad::DPAD_RIGHT),
    ] {
        if v > 0.5 {
            b |= bit;
        }
    }
    let axis = |v: f32| (v.clamp(-1.0, 1.0) * 32767.0) as i16;
    let trig = |v: f32| (v.clamp(0.0, 1.0) * 255.0) as u8;
    GamepadState {
        buttons: b,
        left: [axis(s.left_stick.x), axis(s.left_stick.y)],
        right: [axis(s.right_stick.x), axis(s.right_stick.y)],
        triggers: [trig(s.left_trigger), trig(s.right_trigger)],
    }
}

impl App {
    fn remote(&self, cx: &mut Cx) -> WidgetRef {
        self.ui.widget(cx, ids!(remote))
    }

    fn current(&self) -> Option<&Connection> {
        self.conns.get(self.selected)
    }

    fn select(&mut self, cx: &mut Cx, index: usize) {
        if index >= self.conns.len() {
            return;
        }
        if self.scrub.is_some() {
            // the history is the shown connection's
            self.leave_history(cx);
        }
        if let Some(c) = self.conns.get(self.selected) {
            c.show(false);
        }
        self.selected = index;
        self.conns[index].show(true);
        self.last_pointer = None;
        if let Some(mut v) = self.remote(cx).borrow_mut::<RemoteView>() {
            v.clear(cx);
        }
        self.refresh(cx);
    }

    fn add(&mut self, cx: &mut Cx, name: String, host: String, port: u16, discovered: bool) {
        let show = self.conns.is_empty();
        self.conns.push(Connection::open(name, host, port, !show, discovered, self.signal.clone()));
        if show {
            self.selected = 0;
        }
        self.refresh(cx);
    }

    /// The desktops as the menu-bar dropdown lists them.
    fn desktop_rows(&self) -> Vec<DesktopRow> {
        self.conns
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let s = c.shared.lock().unwrap();
                let reach = match s.state.clone().unwrap_or(State::Connecting) {
                    State::Connected => Reach::Online(s.rtt_ms),
                    State::Connecting | State::Reconnecting => Reach::Connecting,
                    State::Offline(why) => Reach::Offline(why),
                };
                DesktopRow { name: c.name.clone(), addr: format!("{}:{}", c.host, c.port), reach, shown: self.window_shown && i == self.selected }
            })
            .collect()
    }

    /// The tray (its tooltip says how many desktops answer) and the open dropdown.
    fn refresh_tray(&mut self, cx: &mut Cx) {
        // a menu-bar entry where the desktop gives the icon's click to the app (macOS, Windows,
        // the makepad WM)
        if !cx.desktop_has_tray_dropdown() {
            return;
        }
        let rows = self.desktop_rows();
        let online = rows.iter().filter(|r| matches!(r.reach, Reach::Online(_))).count();
        let status = match self.conns.get(self.selected).filter(|_| self.window_shown) {
            Some(c) => format!("{} of {} desktops answer \u{b7} showing {}", online, rows.len(), c.name),
            None => format!("{} of {} desktops answer", online, rows.len()),
        };
        let key = format!("{status}|{}", self.details);
        if key != self.tray_key {
            self.tray_key = key;
            let menu = vec![
                TrayMenuItem::Item { id: live_id!(status), label: status.clone(), enabled: false, checked: None },
                TrayMenuItem::Line,
                TrayMenuItem::item(SHOW, "Show window"),
                TrayMenuItem::item(NEXT_SCREEN, "Next screen of the desktop"),
                TrayMenuItem::check(DETAILS, "Latency details", self.details),
                TrayMenuItem::item(HISTORY, "Pause: history / back to live"),
                TrayMenuItem::Line,
                TrayMenuItem::item(QUIT, "Quit Desk Link"),
            ];
            cx.set_tray(TrayIcon { id: TRAY, image: makepad_desk_link_panel::tray_glyph::image(makepad_desk_link_panel::tray_glyph::Glyph::Viewer), tooltip: format!("Desk Link \u{b7} {status}"), menu, dropdown: true });
        }
        if self.dropdown.is_open() {
            if let Some(mut v) = self.ui.widget(cx, ids!(menu_view)).borrow_mut::<DeskMenuView>() {
                v.set_rows(cx, rows);
            }
        }
    }

    /// Shows desktop `index` in the window (made, shown and raised; the Dock icon with it).
    fn open_desktop(&mut self, cx: &mut Cx, index: usize) {
        if index >= self.conns.len() {
            return;
        }
        self.keyboard = false;
        if !self.window_shown {
            self.window_shown = true;
            cx.show_in_dock(true);
        }
        self.select(cx, index);
        self.ui.window(cx, ids!(main_window)).show(cx);
        menu::selftest(&format!("showing desktop {index} ({})", self.conns[index].name));
        self.refresh_tray(cx);
    }

    /// The picture, the cursor, the overlay lines.
    fn refresh(&mut self, cx: &mut Cx) {
        self.refresh_tray(cx);
        let Some(c) = self.conns.get(self.selected) else { return };
        // paused: live pictures go on arriving (and recording) but are not shown
        let picture = c.latest.lock().unwrap().take().filter(|_| self.scrub.is_none());
        let history_line = {
            let h = c.history.lock().unwrap();
            format!("history {:.1} s · {} packets · {:.0} MB", h.span_ns() as f64 / 1e9, h.len(), h.bytes() as f64 / (1 << 20) as f64)
        };
        let (state, welcome, cursor, offset, fps, mbps, warning, stream_fps, read_gap_ms, decode_queue, stream_rate) = {
            let s = c.shared.lock().unwrap();
            (s.state.clone().unwrap_or(State::Connecting), s.welcome.clone(), s.cursor, s.offset_ns, s.fps, s.mbps, s.warning.clone(), s.stream_fps, s.read_gap_ms, s.decode_queue, s.stream_rate.clone())
        };
        let remote = self.remote(cx);
        let Some(mut view) = remote.borrow_mut::<RemoteView>() else { return };
        if let Some(p) = picture {
            // server times -> this clock: t_local = t_server - offset
            let local = |t: u64| t as i64 - offset;
            let l = &mut self.latency;
            l.encode = (p.encoded_ns as f64 - p.built_ns as f64) / 1e6;
            l.network = (p.received_ns as i64 - local(p.sent_ns)) as f64 / 1e6;
            l.decode = p.decode_ns as f64 / 1e6;
            self.decode_sum.0 += l.decode;
            self.decode_sum.1 += 1;
            view.set_picture(cx, p.width, p.height, p.parts, p.frame, p.decoded_ns);
        }
        if let Some(d) = self.scrub.as_ref().and_then(Scrub::take_ready) {
            let frame = self.scrub.as_ref().map_or(0, |s| s.snap.pictures[d.index].frame);
            view.set_picture(cx, d.width, d.height, d.view_parts(), frame, 0);
            self.hist_shown = Some(d.index);
        }
        if self.scrub.is_some() {
            // a history picture's draw is no latency figure
            view.drawn = None;
        }
        if let Some((frame, decoded, drawn)) = view.drawn.take() {
            self.latency.present = drawn.saturating_sub(decoded) as f64 / 1e6;
            self.latency.total = self.latency.encode + self.latency.network + self.latency.decode + self.latency.present;
            self.last_stats = Some((frame, (self.latency.decode * 1000.0) as u32, (self.latency.present * 1000.0) as u32));
            self.shown += 1;
        }
        view.note = match &state {
            State::Connected => None,
            State::Connecting => Some(format!("connecting to {}:{} …", c.host, c.port)),
            State::Reconnecting => Some("reconnecting …".to_string()),
            State::Offline(why) => Some(why.clone()),
        };
        let mut lines = Vec::new();
        if let (Some(w), true) = (&welcome, self.details) {
            let screen = w.screens.get(w.screen as usize).map(|s| s.name.clone()).unwrap_or_default();
            lines.push(format!("{} · {screen} {}x{}{}", w.server, w.stream[0], w.stream[1], if w.input { String::new() } else { format!(" · {}", w.input_note) }));
            // how the picture is shown: the window in device pixels, the scale (whole: each remote
            // pixel a whole block, nothing resampled)
            lines.push(format!("window {}x{} px{} · scale {:.4}{}", self.window_px[0], self.window_px[1], if self.fullscreen { " (fullscreen)" } else { "" }, view.scale, if view.whole { " whole-pixel" } else { " resampled" }));
        }
        lines.insert(0, view::summary_line(self.shown_rate, mbps, stream_rate.as_ref()));
        // view only (the desktop cannot inject input): always said, never a silent dead mouse
        if let Some(w) = welcome.as_ref().filter(|w| !w.input) {
            lines.push(if w.input_note.is_empty() { "input unavailable on this host".to_string() } else { w.input_note.clone() });
        }
        if self.details {
            lines.push(view::rate_details_line(fps, stream_fps, crate::conn::DISPLAY_HZ.load(std::sync::atomic::Ordering::Acquire), stream_rate.as_ref()));
            lines.push(view::latency_line(&self.latency));
            lines.push(view::flow_line(read_gap_ms, decode_queue));
            lines.push(history_line);
        }
        if let Some(w) = warning {
            lines.push(format!("WARNING: {w}"));
        }
        view.lines = lines;
        // the remote cursor: the OS cursor takes its shape while ours is over the picture;
        // the sprite shows where it is when someone else moves it
        if let Some((x, y, shape, visible)) = cursor {
            let cursor = CURSORS.get(shape as usize).copied().unwrap_or(MouseCursor::Default);
            if self.local_over {
                cx.set_cursor(cursor);
            }
            let ours = self.last_pointer.is_some_and(|(px, py)| (px as i32 - x as i32).abs() < 200 && (py as i32 - y as i32).abs() < 200);
            view.remote_cursor = (visible && !(self.local_over && ours)).then_some((x, y));
        }
        view.redraw(cx);
        drop(view);
        self.update_timeline(cx);
    }

    fn pointer_event(&mut self, cx: &mut Cx, event: &Event) {
        if self.scrub.is_some() {
            // paused: the keyboard and mouse scrub the history, nothing goes to the desktop
            return;
        }
        let placement = match self.remote(cx).borrow::<RemoteView>() {
            Some(v) => v.placement,
            None => return,
        };
        let Some(conn) = self.conns.get(self.selected) else { return };
        if conn.state() != State::Connected {
            return;
        }
        match event {
            Event::MouseMove(e) => {
                if let Some(mut v) = self.remote(cx).borrow_mut::<RemoteView>() {
                    v.set_pointer(cx, e.abs);
                }
                let over = placement.to_stream(e.abs);
                self.local_over = over.is_some();
                let at = if self.held != 0 { Some(placement.to_stream_clamped(e.abs)) } else { over };
                if let Some((x, y)) = at.filter(|p| Some(*p) != self.last_pointer) {
                    self.last_pointer = Some((x, y));
                    conn.send(ClientMsg::Pointer { x, y });
                }
            }
            Event::MouseDown(e) => {
                let Some((x, y)) = placement.to_stream(e.abs) else {
                    // a click outside the picture (the panel) takes the keyboard back
                    self.keyboard = false;
                    return;
                };
                self.keyboard = true;
                if let Some(b) = button_of(e.button) {
                    conn.send(ClientMsg::Pointer { x, y });
                    conn.send(ClientMsg::Button { down: true, button: b });
                    self.held |= 1 << b;
                }
            }
            Event::MouseUp(e) => {
                if let Some(b) = button_of(e.button).filter(|b| self.held & (1 << b) != 0) {
                    self.held &= !(1 << b);
                    conn.send(ClientMsg::Button { down: false, button: b });
                }
            }
            Event::Scroll(e) => {
                if placement.to_stream(e.abs).is_some() {
                    // makepad points (40 per wheel notch) -> 1/120 notches; +dy = wheel away
                    let dx = (e.scroll.x * 3.0).round() as i32;
                    let dy = (-e.scroll.y * 3.0).round() as i32;
                    if dx != 0 || dy != 0 {
                        conn.send(ClientMsg::Scroll { dx, dy });
                    }
                }
            }
            Event::KeyDown(e) | Event::KeyUp(e) if shortcut(e).is_some() => {}
            Event::KeyDown(e) if self.keyboard => conn.send(ClientMsg::Key { down: true, keycode: e.key_code.to_index(), mods: mods_of(&e.modifiers) }),
            Event::KeyUp(e) if self.keyboard => conn.send(ClientMsg::Key { down: false, keycode: e.key_code.to_index(), mods: mods_of(&e.modifiers) }),
            _ => {}
        }
    }

    fn poll_pads(&mut self, cx: &mut Cx) {
        let states: Vec<Option<GamepadState>> = cx
            .game_input_states()
            .iter()
            .map(|s| match s {
                makepad_widgets::makepad_platform::event::game_input::GameInputState::Gamepad(g) => Some(pad_of(g)),
                _ => None,
            })
            .collect();
        let Some(conn) = self.conns.get(self.selected) else { return };
        for (slot, state) in states.iter().enumerate() {
            if self.pads.get(slot).cloned().flatten() != *state {
                match state {
                    Some(s) => conn.send(ClientMsg::Gamepad { slot: slot as u8, state: s.clone() }),
                    None => conn.send(ClientMsg::GamepadGone { slot: slot as u8 }),
                }
            }
        }
        for slot in states.len()..self.pads.len() {
            if self.pads[slot].is_some() {
                conn.send(ClientMsg::GamepadGone { slot: slot as u8 });
            }
        }
        self.pads = states;
        if !self.pads.is_empty() {
            self.pad_poll = cx.new_next_frame();
        }
    }
}

const TRAY: LiveId = live_id!(desklink_tray);
const SHOW: LiveId = live_id!(show_window);
const NEXT_SCREEN: LiveId = live_id!(next_screen);
const DETAILS: LiveId = live_id!(latency_details);
const HISTORY: LiveId = live_id!(history);
const QUIT: LiveId = live_id!(quit_desklink);

/// The viewer's own shortcuts (never sent to the desktop): Cmd (macOS) / Ctrl+Shift (elsewhere)
/// with 1..9 (desktop N), R (reconnect); with Shift on macOS: N (next screen), L (latency
/// details), H (history).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Shortcut {
    Desktop(usize),
    Reconnect,
    NextScreen,
    Details,
    History,
}

fn shortcut(e: &KeyEvent) -> Option<Shortcut> {
    let m = &e.modifiers;
    let (plain, shifted) = if cfg!(target_vendor = "apple") { (m.logo && !m.shift, m.logo && m.shift) } else { (m.control && m.shift, m.control && m.shift) };
    let digits = [KeyCode::Key1, KeyCode::Key2, KeyCode::Key3, KeyCode::Key4, KeyCode::Key5, KeyCode::Key6, KeyCode::Key7, KeyCode::Key8, KeyCode::Key9];
    if plain {
        if let Some(i) = digits.iter().position(|k| *k == e.key_code) {
            return Some(Shortcut::Desktop(i));
        }
        if e.key_code == KeyCode::KeyR {
            return Some(Shortcut::Reconnect);
        }
    }
    if shifted {
        return match e.key_code {
            KeyCode::KeyN => Some(Shortcut::NextScreen),
            KeyCode::KeyL => Some(Shortcut::Details),
            KeyCode::KeyH => Some(Shortcut::History),
            _ => None,
        };
    }
    None
}

/// The history: pausing, stepping, playing, saving.
impl App {
    /// Drops connection `i`'s session and opens it again now (the panel's "reconnect", Cmd+R).
    fn reconnect(&mut self, cx: &mut Cx, i: usize) {
        if let Some(c) = self.conns.get(i) {
            c.reconnect();
            if i == self.selected {
                self.held = 0;
                self.last_pointer = None;
            }
        }
        self.refresh(cx);
    }

    fn enter_history(&mut self, cx: &mut Cx) {
        let Some(c) = self.conns.get(self.selected) else { return };
        let snap = Arc::new(c.history.lock().unwrap().snapshot());
        if snap.len() == 0 {
            return;
        }
        // buttons held on the desktop are let go (the mouse now scrubs)
        for b in 0..8u8 {
            if self.held & (1 << b) != 0 {
                c.send(ClientMsg::Button { down: false, button: b });
            }
        }
        self.held = 0;
        self.cur = snap.len() - 1;
        self.hist_shown = None;
        self.range = (None, None);
        self.playing = None;
        self.scrub = Some(Scrub::start(snap, self.cur, self.signal.clone()));
        if let Some(mut v) = self.remote(cx).borrow_mut::<RemoteView>() {
            v.inset_bottom = view::TIMELINE_H;
        }
        self.show_cur(cx);
    }

    fn leave_history(&mut self, cx: &mut Cx) {
        self.scrub = None;
        self.playing = None;
        self.hist_shown = None;
        if let Some(mut v) = self.remote(cx).borrow_mut::<RemoteView>() {
            v.inset_bottom = 0.0;
        }
        self.refresh(cx);
    }

    /// Shows picture `cur`: at once when it is decoded already, else when the scrub decoder
    /// has it (`Scrub::take_ready`, on the signal).
    fn show_cur(&mut self, cx: &mut Cx) {
        let Some(scrub) = &self.scrub else { return };
        if let Some(d) = scrub.seek(self.cur) {
            let frame = scrub.snap.pictures[d.index].frame;
            if let Some(mut v) = self.remote(cx).borrow_mut::<RemoteView>() {
                v.set_picture(cx, d.width, d.height, d.view_parts(), frame, 0);
            }
            self.hist_shown = Some(d.index);
        }
        if self.hist_shown != Some(self.cur) {
            // the timeline follows the decode until the picture is there
            self.play_frame = cx.new_next_frame();
        }
        self.update_timeline(cx);
    }

    fn step(&mut self, cx: &mut Cx, delta: i64) {
        let Some(n) = self.scrub.as_ref().map(|s| s.snap.len()) else { return };
        self.playing = None;
        self.cur = (self.cur as i64 + delta).clamp(0, n as i64 - 1) as usize;
        self.show_cur(cx);
    }

    fn update_timeline(&mut self, cx: &mut Cx) {
        let data = self.scrub.as_ref().map(|scrub| {
            let (behind, ahead, work, failed) = scrub.status();
            let mut status = format!("decoded -{behind}/+{ahead}");
            if self.hist_shown != Some(self.cur) {
                status.push_str(" · decoding");
            }
            if !work.is_empty() {
                status.push_str(&format!(" · {work}"));
            }
            if let Some(f) = failed {
                status.push_str(&format!(" · FAILED: {f}"));
            }
            let saved = self.clip_status.lock().unwrap().clone();
            if !saved.is_empty() {
                status.push_str(&format!(" · {saved}"));
            }
            TimelineData { snap: scrub.snap.clone(), cur: self.cur, range: self.range, playing: self.playing.map(|p| p.0), status }
        });
        if let Some(mut t) = self.ui.widget(cx, ids!(timeline)).borrow_mut::<Timeline>() {
            t.set_data(cx, data);
        }
    }

    /// The pictures to save: In..Out, from the one set to the shown one, or a second around it.
    fn save(&mut self, cx: &mut Cx, frame_only: bool) {
        let (Some(scrub), Some(c)) = (&self.scrub, self.conns.get(self.selected)) else { return };
        let n = scrub.snap.len();
        let (from, to) = match self.range {
            (Some(a), Some(b)) => (a.min(b), a.max(b)),
            (Some(a), None) | (None, Some(a)) => (a.min(self.cur), a.max(self.cur)),
            (None, None) => (self.cur.saturating_sub(60), (self.cur + 60).min(n - 1)),
        };
        let chosen = self.range != (None, None);
        let req = clip::ClipRequest { snap: scrub.snap.clone(), from, to, shown: self.cur, frame_only, chosen, host: c.host.clone() };
        clip::save(req, self.clip_status.clone(), self.signal.clone());
        self.update_timeline(cx);
    }

    fn timeline_action(&mut self, cx: &mut Cx, action: TimelineAction) {
        match action {
            TimelineAction::Seek(i) => {
                self.playing = None;
                self.cur = i.min(self.scrub.as_ref().map_or(0, |s| s.snap.len().saturating_sub(1)));
                self.show_cur(cx);
            }
            TimelineAction::Step(d) => self.step(cx, d),
            TimelineAction::Live => self.leave_history(cx),
            TimelineAction::Play(speed) => {
                let built = self.scrub.as_ref().map_or(0, |s| s.snap.pictures[self.cur.min(s.snap.len() - 1)].built_ns);
                // from the start when at the end
                if self.scrub.as_ref().is_some_and(|s| self.cur + 1 >= s.snap.len()) {
                    self.cur = 0;
                }
                let built = self.scrub.as_ref().map_or(built, |s| s.snap.pictures[self.cur].built_ns);
                self.playing = Some((speed, -1.0, built));
                self.play_frame = cx.new_next_frame();
                self.update_timeline(cx);
            }
            TimelineAction::Stop => {
                self.playing = None;
                self.update_timeline(cx);
            }
            TimelineAction::In => {
                self.range.0 = Some(self.cur);
                self.update_timeline(cx);
            }
            TimelineAction::Out => {
                self.range.1 = Some(self.cur);
                self.update_timeline(cx);
            }
            TimelineAction::SaveClip => self.save(cx, false),
            TimelineAction::SaveFrame => self.save(cx, true),
            TimelineAction::None => {}
        }
    }

    /// A frame while playing (the picture whose host time is due) or waiting for a decode.
    fn play_tick(&mut self, cx: &mut Cx, time: f64) {
        let Some(scrub) = &self.scrub else { return };
        if let Some((speed, t0, built0)) = self.playing {
            let t0 = if t0 < 0.0 { time } else { t0 };
            self.playing = Some((speed, t0, built0));
            let due = built0 + ((time - t0) * speed * 1e9) as u64;
            let i = scrub.snap.at_time(due).max(self.cur);
            let end = i + 1 >= scrub.snap.len();
            if i != self.cur {
                self.cur = i;
                let playing = self.playing;
                self.show_cur(cx);
                self.playing = playing;
            }
            if end {
                self.playing = None;
            }
        }
        if self.playing.is_some() || self.hist_shown != Some(self.cur) {
            self.play_frame = cx.new_next_frame();
        }
        self.update_timeline(cx);
    }

    fn history_key(&mut self, cx: &mut Cx, e: &KeyEvent) {
        let big = if e.modifiers.shift { 10 } else { 1 };
        match e.key_code {
            KeyCode::ArrowLeft => self.step(cx, -big),
            KeyCode::ArrowRight => self.step(cx, big),
            KeyCode::Home => self.timeline_action(cx, TimelineAction::Seek(0)),
            KeyCode::End => self.timeline_action(cx, TimelineAction::Seek(usize::MAX / 2)),
            KeyCode::Space if !e.is_repeat => self.timeline_action(cx, if self.playing.is_some() { TimelineAction::Stop } else { TimelineAction::Play(1.0) }),
            KeyCode::Escape if !e.is_repeat => self.leave_history(cx),
            _ => {}
        }
    }

    /// `--scrub-check`: one step of the script each tick, once the shown picture is there.
    fn check_tick(&mut self, cx: &mut Cx) {
        let Some(mut check) = self.check.take() else { return };
        let have = self.conns.get(self.selected).is_some_and(|c| c.history.lock().unwrap().len() > 0);
        if check.started.is_none() && have {
            check.started = Some(std::time::Instant::now());
        }
        // no pictures at all for a minute: the check cannot run (says why, exits 1)
        let waited = check.created.elapsed().as_secs_f64();
        if check.started.is_none() && waited > 60.0 {
            let state = self.conns.get(self.selected).map(|c| format!("{:?}", c.state())).unwrap_or_else(|| "no desktop".into());
            eprintln!("scrub-check: FAILED: no pictures in {waited:.0} s ({state})");
            std::process::exit(1);
        }
        if self.scrub.is_none() {
            let elapsed = check.started.map_or(0.0, |t| t.elapsed().as_secs_f64());
            if check.reconnect_wanted && check.reconnect.is_none() && elapsed >= check.after / 2.0 {
                let i = self.selected;
                self.reconnect(cx, i);
                check.reconnect = Some((std::time::Instant::now(), false, None));
            }
            if let Some((at, offline, None)) = check.reconnect {
                let state = self.conns.get(self.selected).map(|c| c.state());
                if state != Some(State::Connected) {
                    check.reconnect = Some((at, true, None));
                } else if offline && self.conns.get(self.selected).is_some_and(|c| c.shared.lock().unwrap().welcome.is_some()) {
                    check.reconnect = Some((at, true, Some(at.elapsed().as_secs_f64() * 1000.0)));
                }
            }
            if check.started.is_some_and(|t| t.elapsed().as_secs_f64() >= check.after) {
                self.enter_history(cx);
                check.waiting = Some((Check::FIRST, std::time::Instant::now()));
            }
            self.check = Some(check);
            return;
        }
        if let Some((kind, at)) = check.waiting {
            let done = if kind == Check::SAVE {
                let s = self.clip_status.lock().unwrap().clone();
                s.starts_with("saved") || s.starts_with("save failed")
            } else {
                self.hist_shown == Some(self.cur)
            };
            if !done {
                self.check = Some(check);
                return;
            }
            check.times.push((kind, at.elapsed().as_secs_f64() * 1000.0));
            check.waiting = None;
        }
        let Some(step) = check.steps.pop_front() else {
            report_check(&check, self);
            if std::env::args().any(|a| a == "--check-hold") {
                // stays paused on the last picture (a grab of the timeline)
                self.update_timeline(cx);
                return;
            }
            self.scrub = None;
            std::process::exit(0);
        };
        // steps at the pace of a held arrow key (about 30 a second)
        if check.last_step.is_some_and(|t| t.elapsed().as_secs_f64() < 1.0 / 30.0) {
            check.steps.push_front(step);
            self.check = Some(check);
            return;
        }
        let n = self.scrub.as_ref().map_or(0, |s| s.snap.len());
        let now = std::time::Instant::now();
        check.last_step = Some(now);
        check.waiting = Some((step, now));
        self.check = Some(check);
        match step {
            Check::SAVE => self.save(cx, false),
            Check::JUMP => self.timeline_action(cx, TimelineAction::Seek(n / 2)),
            d => self.step(cx, d),
        }
        // a decoded picture shows at once: no wait at all
        if step != Check::SAVE && self.hist_shown == Some(self.cur) {
            if let Some(c) = self.check.as_mut() {
                c.times.push((step, 0.0));
                c.waiting = None;
            }
        }
    }
}

/// The `--scrub-check` summary (one line, answer first).
fn report_check(check: &Check, app: &App) {
    let pick = |f: &dyn Fn(i64) -> bool| -> Vec<f64> { check.times.iter().filter(|(k, _)| f(*k)).map(|t| t.1).collect() };
    let stats = |mut v: Vec<f64>| -> String {
        if v.is_empty() {
            return "none".into();
        }
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let instant = v.iter().filter(|t| **t == 0.0).count();
        format!("n {} median {:.1} p90 {:.1} max {:.1} ms ({} instant)", v.len(), v[v.len() / 2], v[v.len() * 9 / 10], v[v.len() - 1], instant)
    };
    let (n, span, mb) = app.scrub.as_ref().map_or((0, 0.0, 0.0), |s| {
        let p = &s.snap.pictures;
        (p.len(), p.last().map_or(0, |l| l.built_ns - p[0].built_ns) as f64 / 1e9, s.snap.packets.iter().map(|p| p.data.len()).sum::<usize>() as f64 / (1 << 20) as f64)
    });
    let first = pick(&|k| k == Check::FIRST).first().copied().unwrap_or(-1.0);
    let jump = pick(&|k| k == Check::JUMP).first().copied().unwrap_or(-1.0);
    let save = pick(&|k| k == Check::SAVE).first().copied().unwrap_or(-1.0);
    if let Some((_, offline, ms)) = check.reconnect {
        eprintln!("scrub-check: reconnect: {}", match ms {
            Some(ms) => format!("session dropped and back (Welcome) in {ms:.0} ms"),
            None if offline => "dropped, not back before the pause".into(),
            None => "the session never dropped".into(),
        });
    }
    eprintln!(
        "scrub-check: history {n} pictures {span:.1} s {mb:.0} MB · first {first:.1} ms · back {} · forward {} · far jump {jump:.1} ms · save {save:.0} ms: {}",
        stats(pick(&|k| k < 0 && k > Check::JUMP)),
        stats(pick(&|k| k > 0)),
        app.clip_status.lock().unwrap()
    );
}

impl App {
    fn toggle_history(&mut self, cx: &mut Cx) {
        if self.scrub.is_some() {
            self.leave_history(cx);
        } else {
            self.enter_history(cx);
        }
    }

    fn next_screen(&mut self) {
        if let Some(c) = self.current() {
            let next = c.shared.lock().unwrap().welcome.as_ref().map(|w| (w.screen + 1) % w.screens.len().max(1) as u32);
            if let Some(index) = next {
                c.send(ClientMsg::SelectScreen { index });
            }
        }
    }

    fn toggle_details(&mut self, cx: &mut Cx) {
        self.details = !self.details;
        self.remote(cx).redraw(cx);
        self.refresh_tray(cx);
    }

    fn do_shortcut(&mut self, cx: &mut Cx, s: Shortcut) {
        match s {
            Shortcut::Desktop(i) => self.open_desktop(cx, i),
            Shortcut::Reconnect => {
                let i = self.selected;
                self.reconnect(cx, i);
            }
            Shortcut::NextScreen => self.next_screen(),
            Shortcut::Details => self.toggle_details(cx),
            Shortcut::History => self.toggle_history(cx),
        }
    }

    /// The window closed into the menu bar: the shown desktop pauses, no Dock icon.
    fn hide_window(&mut self, cx: &mut Cx) {
        self.window_shown = false;
        if self.scrub.is_some() {
            self.leave_history(cx);
        }
        if let Some(c) = self.conns.get(self.selected) {
            c.show(false);
        }
        self.ui.window(cx, ids!(main_window)).set_visible(cx, false);
        cx.show_in_dock(false);
        menu::selftest("window closed: hidden, Desk Link stays in the menu bar");
        self.refresh_tray(cx);
    }

    /// Starts measuring the display's refresh: [`Self::vsync_frame`] over the next frames.
    fn measure_refresh(&mut self, cx: &mut Cx) {
        self.vsync_times.clear();
        self.vsync = cx.new_next_frame();
    }

    /// One vsync of the measurement; after 40 intervals the median one gives the refresh (Hz),
    /// which every connection tells its desktop (it streams at most that many pictures).
    fn vsync_frame(&mut self, cx: &mut Cx, time: f64) {
        self.vsync_times.push(time);
        if self.vsync_times.len() <= 40 {
            self.vsync = cx.new_next_frame();
            return;
        }
        let mut dts: Vec<f64> = self.vsync_times.windows(2).map(|w| w[1] - w[0]).filter(|d| *d > 0.0).collect();
        self.vsync_times.clear();
        dts.sort_by(|a, b| a.partial_cmp(b).unwrap());
        if let Some(dt) = dts.get(dts.len() / 2) {
            let hz = (1.0 / dt).round().clamp(1.0, 1000.0) as u32;
            crate::conn::DISPLAY_HZ.store(hz, std::sync::atomic::Ordering::Release);
        }
    }
}

/// `--dump-check <file>`: the hardware decoder over an Annex-B dump (one access unit per
/// coded picture; parameter sets go with the picture after them).
fn dump_check(path: &str) -> ! {
    use makepad_widgets::makepad_platform::video_file::{annex_b, StreamVideoCodec, VideoStreamDecoder};
    let data = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("desklink: {path}: {e}");
        std::process::exit(2)
    });
    let nals = annex_b::split_annex_b(&data);
    // .h265 / .hevc: HEVC (a check of what this machine's decoder takes), else H.264
    let hevc = path.ends_with(".h265") || path.ends_with(".hevc");
    let codec = if hevc { StreamVideoCodec::Hevc } else { StreamVideoCodec::H264 };
    // the viewer's decoder: GPU pictures where the platform has them
    let mut decoder = VideoStreamDecoder::new_gpu(codec).unwrap_or_else(|e| {
        eprintln!("desklink: hardware decoder: {e:?}");
        std::process::exit(2)
    });
    let (mut unit, mut pictures, mut decoded, mut keys, mut gpu) = (Vec::new(), 0, 0, 0, 0u32);
    let mut size = (0, 0);
    let started = std::time::Instant::now();
    for nal in nals {
        unit.extend_from_slice(&[0, 0, 0, 1]);
        unit.extend_from_slice(nal);
        let (vcl, key) = if hevc {
            let ty = annex_b::hevc_nal_unit_type(nal);
            (ty < 32, (16..=21).contains(&ty))
        } else {
            let ty = annex_b::nal_unit_type(nal);
            (ty == 1 || ty == 5, ty == 5)
        };
        if vcl {
            keys += key as u32;
            match decoder.push_packet(&unit, pictures) {
                Ok(frames) => {
                    for f in frames {
                        decoded += 1;
                        gpu += f.gpu.is_some() as u32;
                        size = (f.width, f.height);
                    }
                }
                Err(e) => eprintln!("desklink: picture {pictures}: {e:?}"),
            }
            pictures += 1;
            unit.clear();
        }
    }
    if let Ok(frames) = decoder.flush() {
        decoded += frames.len();
    }
    println!(
        "dump-check: {pictures} pictures ({keys} IDR), {decoded} decoded at {}x{} ({gpu} as GPU pictures), {:.1} ms per picture, hardware decoder: {}",
        size.0,
        size.1,
        started.elapsed().as_secs_f64() * 1000.0 / pictures.max(1) as f64,
        match decoder.hardware() {
            Some(true) => "yes",
            Some(false) => "no",
            None => "not reported",
        }
    );
    std::process::exit(if decoded > 0 && decoded as i64 == pictures { 0 } else { 1 });
}

/// `--decode-bench <dir>`: `tiles.txt` (`<stream> <x0> <y0>` lines) and `stream-<i>.h264`.
/// Lock-step: picture k's tiles decode at once (one thread each, released together), the
/// picture's time is its slowest tile's. Free-running: each thread decodes its whole stream as
/// fast as it can (the pictures a second the decoders sustain together).
fn decode_bench(dir: &str) -> ! {
    use makepad_widgets::makepad_platform::video_file::{annex_b, StreamVideoCodec, VideoStreamDecoder};
    use std::sync::{Arc, Barrier};
    let dir = std::path::Path::new(dir);
    let tiles = std::fs::read_to_string(dir.join("tiles.txt")).unwrap_or_else(|e| {
        eprintln!("desklink: {}: {e}", dir.join("tiles.txt").display());
        std::process::exit(2)
    });
    // per stream: its access units (one per coded picture, parameter sets with the next)
    let mut streams: Vec<(String, Vec<Vec<u8>>)> = Vec::new();
    for line in tiles.lines() {
        let mut w = line.split_whitespace();
        let (Some(i), Some(x), Some(y)) = (w.next(), w.next(), w.next()) else { continue };
        let path = dir.join(format!("stream-{i}.h264"));
        let data = std::fs::read(&path).unwrap_or_else(|e| {
            eprintln!("desklink: {}: {e}", path.display());
            std::process::exit(2)
        });
        let mut units = Vec::new();
        let mut unit = Vec::new();
        for nal in annex_b::split_annex_b(&data) {
            unit.extend_from_slice(&[0, 0, 0, 1]);
            unit.extend_from_slice(nal);
            let ty = annex_b::nal_unit_type(nal);
            if ty == 1 || ty == 5 {
                units.push(std::mem::take(&mut unit));
            }
        }
        streams.push((format!("{i}@{x},{y}"), units));
    }
    let pictures = streams.iter().map(|(_, u)| u.len()).min().unwrap_or(0);
    if pictures == 0 {
        eprintln!("desklink: no pictures in {}", dir.display());
        std::process::exit(2);
    }
    let run = |lockstep: bool| -> (Vec<Vec<f64>>, f64, Vec<(u32, u32)>, u32) {
        let barrier = Arc::new(Barrier::new(streams.len()));
        let started = std::time::Instant::now();
        let handles: Vec<_> = streams.iter().map(|(_, units)| {
            let units: Vec<Vec<u8>> = units[..pictures].to_vec();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut decoder = VideoStreamDecoder::new_gpu(StreamVideoCodec::H264).expect("hardware decoder");
                let (mut times, mut size, mut errors) = (Vec::new(), (0, 0), 0u32);
                for (k, unit) in units.iter().enumerate() {
                    if lockstep {
                        barrier.wait();
                    }
                    let t = std::time::Instant::now();
                    match decoder.push_packet(unit, k as i64) {
                        Ok(frames) => {
                            for f in frames {
                                size = (f.width, f.height);
                            }
                        }
                        Err(_) => errors += 1,
                    }
                    times.push(t.elapsed().as_secs_f64() * 1000.0);
                }
                (times, size, errors)
            })
        }).collect();
        let out: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let wall = started.elapsed().as_secs_f64();
        let errors = out.iter().map(|o| o.2).sum();
        let sizes = out.iter().map(|o| o.1).collect();
        (out.into_iter().map(|o| o.0).collect(), wall, sizes, errors)
    };
    let pct = |v: &mut Vec<f64>, p: usize| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v.get((v.len() * p / 100).min(v.len().saturating_sub(1))).copied().unwrap_or(f64::NAN)
    };
    // warm up the decoders' first key frame path once, then measure
    let (times, _, sizes, errors) = run(true);
    let mut picture: Vec<f64> = (0..pictures).map(|k| times.iter().map(|t| t[k]).fold(0.0, f64::max)).collect();
    let (_, free_wall, _, free_errors) = run(false);
    for ((name, _), (t, size)) in streams.iter().zip(times.iter().zip(&sizes)) {
        let mut t = t.clone();
        println!("decode-bench: tile {name} {}x{}: median {:.2} ms, p90 {:.2} ms", size.0, size.1, pct(&mut t, 50), pct(&mut t, 90));
    }
    println!(
        "decode-bench: {pictures} pictures of {} tiles; lock-step picture decode median {:.2} ms, p90 {:.2} ms, max {:.2} ms; free-running {:.1} pictures/s; errors {errors}+{free_errors}",
        streams.len(),
        pct(&mut picture.clone(), 50),
        pct(&mut picture.clone(), 90),
        pct(&mut picture, 100),
        pictures as f64 / free_wall
    );
    std::process::exit(if errors + free_errors == 0 { 0 } else { 1 });
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        let args: Vec<String> = std::env::args().collect();
        if let Some(i) = args.iter().position(|a| a == "--decode-bench") {
            decode_bench(args.get(i + 1).map(String::as_str).unwrap_or(""));
        }
        if let Some(i) = args.iter().position(|a| a == "--dump-check") {
            dump_check(args.get(i + 1).map(String::as_str).unwrap_or(""));
        }
        self.stats_log = args.iter().any(|a| a == "--stats");
        if let Some(after) = args.windows(2).find(|w| w[0] == "--scrub-check").and_then(|w| w[1].parse::<f64>().ok()) {
            // pause, the newest picture; 120 steps back, 60 forward; the middle; a saved clip
            let mut steps: std::collections::VecDeque<i64> = std::iter::repeat_n(-1, 120).chain(std::iter::repeat_n(1, 60)).collect();
            steps.push_back(Check::JUMP);
            steps.extend(std::iter::repeat_n(-1, 30));
            steps.push_back(Check::SAVE);
            self.check = Some(Check { after, started: None, steps, waiting: None, times: Vec::new(), last_step: None, reconnect: None, reconnect_wanted: args.iter().any(|a| a == "--check-reconnect"), created: std::time::Instant::now(), timer: cx.start_interval(0.004) });
        }
        if let Some(path) = args.windows(2).find(|w| w[0] == "--dump-play").map(|w| w[1].clone()) {
            let gpu = !args.iter().any(|a| a == "--cpu-decode");
            let still = args.windows(2).find(|w| w[0] == "--still").and_then(|w| w[1].parse().ok());
            self.conns.push(Connection::play_dump(path, gpu, still, self.signal.clone()));
            self.selected = 0;
            self.refresh(cx);
            self.tick = cx.start_interval(1.0);
            return;
        }
        self.measure_refresh(cx);
        for (name, host, port, discovered) in hosts() {
            self.add(cx, name, host, port, discovered);
        }
        if let Some(n) = args.windows(2).find(|w| w[0] == "--select").and_then(|w| w[1].parse::<usize>().ok()) {
            self.select(cx, n);
        }
        self.tick = cx.start_interval(1.0);
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions.filter_widget_actions_cast::<TimelineAction>(self.ui.widget(cx, ids!(timeline)).widget_uid()) {
            self.timeline_action(cx, action);
        }
        for action in actions.filter_widget_actions_cast::<DeskMenuAction>(self.ui.widget(cx, ids!(menu_view)).widget_uid()) {
            if let DeskMenuAction::Open(i) = action {
                let window = self.ui.window(cx, ids!(menu_window));
                self.dropdown.close(cx, &window, "desktop chosen");
                self.open_desktop(cx, i);
            }
        }
    }

    fn handle_signal(&mut self, cx: &mut Cx) {
        if self.signal.check_and_clear() {
            self.refresh(cx);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        makepad_shell_kit::eval_default_theme(vm);
        makepad_shell_kit::script_mod(vm);
        view::script_mod(vm);
        menu::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Shutdown = event {
            // every connection's threads (and the scrub decoder) end and are joined before the app goes
            self.scrub = None;
            self.conns.clear();
            return;
        }
        match event {
            // the viewer's shortcuts (never sent to the desktop)
            Event::KeyDown(e) if !e.is_repeat && shortcut(e).is_some() => {
                if let Some(s) = shortcut(e) {
                    self.do_shortcut(cx, s);
                }
                return;
            }
            Event::TrayMenu(e) if e.tray == TRAY => {
                match e.item {
                    id if id == SHOW => {
                        let i = self.selected;
                        self.open_desktop(cx, i);
                    }
                    id if id == NEXT_SCREEN => self.next_screen(),
                    id if id == DETAILS => self.toggle_details(cx),
                    id if id == HISTORY => self.toggle_history(cx),
                    id if id == QUIT => {
                        menu::selftest("quit from the tray");
                        cx.remove_tray(TRAY);
                        cx.quit();
                    }
                    _ => {}
                }
                return;
            }
            // a desktop whose tray click is no dropdown's: the window
            Event::TrayClick(e) if e.tray == TRAY && !cx.has_tray_dropdown() => {
                let i = self.selected;
                self.open_desktop(cx, i);
                return;
            }
            // closing the window keeps the app in the menu bar where there is one
            Event::WindowCloseRequested(e) if cx.has_tray() && Some(e.window_id) == self.ui.window(cx, ids!(main_window)).window_id() => {
                e.accept_close.set(false);
                self.hide_window(cx);
                return;
            }
            Event::KeyDown(e) if self.scrub.is_some() => self.history_key(cx, e),
            // the left arrow while the keyboard is not driving the desktop: pause and step back
            Event::KeyDown(e) if !self.keyboard && e.key_code == KeyCode::ArrowLeft && cx.key_focus().is_empty() => {
                self.enter_history(cx);
                self.step(cx, -1);
            }
            _ => {}
        }
        if let Some(e) = self.play_frame.is_event(event) {
            self.play_tick(cx, e.time);
        }
        if self.check.as_ref().is_some_and(|c| c.timer.is_event(event).is_some()) {
            self.check_tick(cx);
        }
        self.pointer_event(cx, event);
        if let Event::GameInputConnected(_) = event {
            self.pad_poll = cx.new_next_frame();
        }
        if let Some(e) = self.vsync.is_event(event) {
            self.vsync_frame(cx, e.time);
        }
        if let Event::WindowGeomChange(e) = event {
            // another screen (a 120 Hz panel, a 60 Hz external one): measured again
            self.measure_refresh(cx);
            let g = &e.new_geom;
            self.window_px = [(g.inner_size.x * g.dpi_factor).round() as u32, (g.inner_size.y * g.dpi_factor).round() as u32];
            self.fullscreen = g.is_fullscreen;
        }
        if self.pad_poll.is_event(event).is_some() {
            self.poll_pads(cx);
        }
        if self.tick.is_event(event).is_some() {
            let dropped = self.current().map_or(0, |c| std::mem::take(&mut c.shared.lock().unwrap().dropped));
            self.shown_rate = (std::mem::take(&mut self.shown), dropped);
            self.refresh(cx);
            let (sum, n) = std::mem::take(&mut self.decode_sum);
            if let (Some((frame, last_us, present_us)), Some(c)) = (self.last_stats.take(), self.current()) {
                let decode_us = if n == 0 { last_us } else { (sum * 1000.0 / n as f64) as u32 };
                c.send(ClientMsg::Stats { frame, decode_us, present_us });
            }
            if self.stats_log {
                let (fps, mbps, rtt, stream_fps, gap, queue, rate) = self.current().map(|c| {
                    let s = c.shared.lock().unwrap();
                    (s.fps, s.mbps, s.rtt_ms.unwrap_or(0.0), s.stream_fps, s.read_gap_ms, s.decode_queue, s.stream_rate.clone())
                }).unwrap_or_default();
                let display = crate::conn::DISPLAY_HZ.load(std::sync::atomic::Ordering::Acquire);
                eprintln!("desklink stats: rtt {rtt:.2} ms · {} · {} · {} · {}", view::latency_line(&self.latency), view::summary_line(self.shown_rate, mbps, rate.as_ref()), view::rate_details_line(fps, stream_fps, display, rate.as_ref()), view::flow_line(gap, queue));
            }
        }
        if let Event::Draw(_) = event {
            // a picture drawn: its present time is known now
            let drawn = self.remote(cx).borrow::<RemoteView>().is_some_and(|v| v.drawn.is_some());
            if drawn {
                self.signal.set();
            }
        }
        let window = self.ui.window(cx, ids!(menu_window));
        let content = self.ui.widget(cx, ids!(menu_view)).borrow::<DeskMenuView>().map_or(0.0, |v| v.content_height());
        if self.dropdown.handle_event(cx, event, &window, content) == Some(DropdownChange::Opened) {
            self.tray_key.clear();
            self.refresh_tray(cx);
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if let Event::Draw(_) = event {
            let content = self.ui.widget(cx, ids!(menu_view)).borrow::<DeskMenuView>().map_or(0.0, |v| v.content_height());
            self.dropdown.fit(cx, &window, content);
        }
    }
}

