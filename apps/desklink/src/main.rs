//! Desk Link — the viewer of the window manager's remote desktop.
//!
//! A window is the picture alone under its caption. The caption carries, left to right: the
//! machines as tabs (a live / connecting / offline dot and the short name), the stream's stats,
//! and the history toggle. The machines are the app's, not a window's: every window lists every
//! machine the app knows (saved hosts in `~/.makepad/desklink/hosts`, `--connect host[:port]`,
//! the devhub's JIT hosts in `~/.makepad/devhub/desk-links`: PROTOCOL.md "Finding desktops";
//! never this machine itself) and differs only in which one it shows. Each window streams its
//! machine live at once with the others (no connection pauses or closes for another); windows
//! on the same machine share its connection and pictures; the window that showed it or had the
//! focus last sizes a desktop that follows its viewer. A machine no window shows stays connected
//! and paused. Keys go to the focused window's machine only, and what a window holds down goes
//! up when it loses the focus. Cmd+N (File → New Window, the Dock icon's menu) or a tab's "Open
//! in new window" opens one more window; Cmd+W closes one (the first one of the installed Dock
//! app hides into the Dock; the Dock icon shows it again). The windows and the machine each
//! showed are restored at launch (`restore`). The picture is the remote screen at its own
//! resolution, scaled to the window at a whole-pixel scale where it fits (aspect-locked,
//! letterboxed); input is mapped back through the same placement. Keyboard input goes to the
//! desktop once the picture was clicked. The stream is the desktop's choice (codec and rate
//! auto).
//!
//! Shortcuts (Cmd on macOS, Ctrl+Shift elsewhere; never sent to the desktop): Cmd+N a new
//! window (File → New Window, also in the Dock icon's menu), Cmd+W closes the window, Cmd+1..9 shows tab N, Cmd+R
//! reconnects, Cmd+Shift+S the desktop's next screen, Cmd+Shift+L latency details,
//! Cmd+Shift+H the history scrubber.
//!
//! History: every connection keeps the last 30 s of its received bitstream. The scrubber
//! (`scrubber`, the caption's history icon or Cmd+Shift+H) floats over the bottom of the
//! picture: a 30 s timeline with the playhead, thumbnails of the hovered moment, key frames and
//! drops marked. Scrubbing pauses the window on that picture (live pictures go on arriving and
//! recording); the keys, while the keyboard is not driving the desktop: left / right step
//! (shift: ten), space plays / pauses, J / K / L play backward / pause / play forward (again:
//! faster), I / O mark the range, Home / End, Escape back to live (again: closes the scrubber).
//! "Save clip" writes the range (In / Out, else a second around the shown picture) to
//! local/screencap: the bitstream as received, PNGs and frame notes.
//!
//! Dev flags: `--connect host[:port]` (repeatable; `--connect-only`: no other desktops), `--select N`, `--stats` (latency lines on
//! stderr once a second), `--dump-check <file.h264>` (decode an Annex-B dump with the hardware
//! decoder, print what came out, exit), `--dump-play <dump dir | file.h264[@x0,y0],...>` (show a dump, or the tiles of one, as a desktop, at
//! 60 pictures a second, looping; `--cpu-decode` takes the CPU NV12 path instead of GPU pictures, `--still N` stops at picture N),
//! `--decode-bench <dir>` (the tile streams of a `desk-link-probe --dump`: each on its own decoder
//! thread as the viewer runs them, every picture's tiles at once; prints the decode time of a
//! whole picture and the pictures a second the decoders sustain, exit).
//! `--scrub-check SECS`: after SECS of pictures, pause, scrub the history (steps back and
//! forward, a far jump, a saved clip), print the timings on stderr and exit (`--check-hold`:
//! stay paused). `--check-scrubber`: open the scrubber once pictures arrive and hover the
//! middle of its timeline (a thumbnail), `--check-split`: the second machine in a new window
//! (selftest lines say where things are drawn). `--no-restore`: not the last run's windows.

mod caption;
mod clip;
mod a11y;
mod conn;
mod held;
mod history;
mod installed;
mod restore;
mod scrub;
mod scrubber;
mod view;
mod viewer;

pub use makepad_widgets;
use caption::{PopMenuAction, TabsAction, ToggleAction};
use conn::{Connection, State};
use makepad_desk_link::{self as proto, ClientMsg, GamepadState};
use makepad_widgets::*;
use scrubber::ScrubAction;
use std::sync::{Arc, Mutex};
use view::RemoteView;
use viewer::Viewer;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.draw.KeyCode

    // One viewer window: the caption (the room of the window buttons, the machines as tabs,
    // the stats, the history toggle) over the picture, with the scrubber and the menus floating
    // on it. New windows are made from it (`App::new_window`). Fullscreen is the picture alone
    // (the shortcuts stay).
    mod.widgets.DeskWindow = Window{
        window.title: "Desk Link" window.inner_size: vec2(1400, 860)
        fullscreen_caption: false
        // a video player's dark around the picture (letterbox), whatever the theme
        pass +: {clear_color: #x101114}
        caption_bar +: {
            caption_label +: {
                flow: Right spacing: 0 align: Align{y: 0.5}
                caption_icon +: {width: 0 height: 0 margin: 0}
                label +: {visible: false}
                // the traffic lights' room (shown only where the OS draws them over the caption)
                caption_window_gap := View{width: 78 height: 1}
                // the tabs start at a fixed place: nothing left of them changes width
                tabs := DeskTabs{margin: Inset{left: 6}}
                View{width: Fill height: Fill}
                // the stats right-aligned in a fixed room, fixed-width digits: changing
                // numbers move nothing
                View{
                    width: 360 height: Fill align: Align{x: 1.0 y: 0.5}
                    stats := Label{text: "" draw_text +: {text_style: theme.font_code{}}}
                }
                history_toggle := CaptionToggle{margin: Inset{left: 8 right: 8}}
            }
        }
        // the menu bar (macOS): New Window also in the Dock icon's menu (the system lists the
        // windows there)
        window_menu +: {
            main := MenuItem.Main{items:[@app_menu, @file_menu, @window_menu]}
            app_menu := MenuItem.Sub{name:"Desk Link" items:[@quit]}
            quit := MenuItem.Item{name:"Quit Desk Link" key: KeyCode.KeyQ enabled: true}
            file_menu := MenuItem.Sub{name:"File" items:[@new_window, @close_window]}
            new_window := MenuItem.Item{name:"New Window" key: KeyCode.KeyN enabled: true dock: true}
            close_window := MenuItem.Item{name:"Close Window" key: KeyCode.KeyW enabled: true}
        }
        body +: {
            padding: 0 spacing: 0
            View{
                width: Fill height: Fill
                flow: Overlay
                remote := RemoteView{}
                scrubber := Scrubber{}
                menu := PopMenu{}
            }
            layers := OverlayLayers{}
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := mod.widgets.DeskWindow{}
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    conns: Vec<Connection>,
    /// Each connection's newest picture, imported once for every window that shows it.
    #[rust]
    pictures: std::collections::HashMap<usize, viewer::ConnPicture>,
    /// The windows changed since they were last saved (`restore`).
    #[rust]
    restore_dirty: bool,
    /// The viewer windows (the first is `main_window`; the others made at run time).
    #[rust]
    viewers: Vec<Viewer>,
    /// The viewer whose window has the keyboard.
    #[rust]
    focused: usize,
    #[rust]
    signal: SignalToUI,
    #[rust]
    pads: Vec<Option<GamepadState>>,
    #[rust]
    pad_poll: NextFrame,
    #[rust]
    tick: Timer,
    /// While a scrubber shows a live history: its timeline follows (a few times a second).
    #[rust]
    snap_tick: Timer,
    /// Measures this display's refresh (vsync to vsync), told to every desktop.
    #[rust]
    vsync: NextFrame,
    #[rust]
    vsync_times: Vec<f64>,
    #[rust]
    stats_log: bool,
    /// The latency details over the picture (Cmd+Shift+L, or `--stats`).
    #[rust]
    details: bool,
    #[rust]
    clip_status: Arc<Mutex<String>>,
    /// When the last save was asked (the scrubber says how it went for a while).
    #[rust]
    clip_at: f64,
    /// The wall clock (UNIX seconds) at `conn::now_ns` 0.
    #[rust]
    wall0: f64,
    /// `--scrub-check`.
    #[rust]
    check: Option<Check>,
    /// `--check-scrubber` / `--check-split`: done once pictures arrive.
    #[rust]
    check_ui: Vec<String>,
}

/// `--scrub-check`: a scripted scrub of the history, timed on the UI thread.
pub(crate) struct Check {
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

pub(crate) fn port_of(spec: &str) -> (String, u16) {
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

/// A machine's short name for its tab: the name its line gives, else `n` and the address's
/// last number (`10.0.0.165` -> `n165`), else the host as written.
pub(crate) fn short_name(name: &str, host: &str) -> String {
    let named = name != host && !name.contains(':') && !name.is_empty();
    if named {
        return name.to_string();
    }
    match host.parse::<std::net::Ipv4Addr>() {
        Ok(ip) if ip.is_loopback() => "local".to_string(),
        Ok(ip) => format!("n{}", ip.octets()[3]),
        Err(_) => host.split('.').next().unwrap_or(host).to_string(),
    }
}

/// The Desk Link endpoints of the devhub's JIT hosts (a headset, a phone serving its own
/// render): the master writes the ones its nodes report to `~/.makepad/devhub/desk-links`, one
/// `<node> <ip>:<port>` line each (no LAN discovery here: PROTOCOL.md, "Finding desktops").
fn devhub_desktops() -> Vec<(String, String, u16)> {
    let path = makepad_network::fleet::makepad_home().join("devhub").join("desk-links");
    parse_desk_links(&std::fs::read_to_string(path).unwrap_or_default())
}

fn parse_desk_links(text: &str) -> Vec<(String, String, u16)> {
    text.lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            let (name, spec) = (w.next()?, w.next()?);
            let (host, port) = port_of(spec);
            (!host.is_empty()).then(|| (short_name(name, &host), host, port))
        })
        .collect()
}

/// The desktops to list: `--connect` ones, saved hosts, the devhub's JIT hosts.
fn hosts() -> Vec<(String, String, u16)> {
    let mut out: Vec<(String, String, u16)> = Vec::new();
    let mut add = |name: String, spec: &str| {
        let (host, port) = port_of(spec);
        if !host.is_empty() && !out.iter().any(|(_, h, p)| *h == host && *p == port) {
            out.push((short_name(&name, &host), host, port));
        }
    };
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
    for (name, host, port) in devhub_desktops() {
        if !is_self(&host) {
            add(name, &format!("{host}:{port}"));
        }
    }
    out
}

pub(crate) const CURSORS: [MouseCursor; 36] = [
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

pub(crate) fn mods_of(m: &KeyModifiers) -> u8 {
    (m.shift as u8 * proto::mods::SHIFT) | (m.control as u8 * proto::mods::CONTROL) | (m.alt as u8 * proto::mods::ALT) | (m.logo as u8 * proto::mods::LOGO)
}

pub(crate) fn button_of(b: MouseButton) -> Option<u8> {
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

/// The viewer's own shortcuts (never sent to the desktop): Cmd (macOS) / Ctrl+Shift (elsewhere)
/// with 1..9 (tab N), R (reconnect), W (close the window); with Shift on
/// macOS: S (the desktop's next screen), L (latency details), H (history); Ctrl+Shift+N a new
/// window (macOS: Cmd+N, the menu's); Ctrl+Tab / Ctrl+Shift+Tab the next / previous tab;
/// fullscreen on and off: Cmd+Ctrl+F (macOS), F11 elsewhere. All of them work in fullscreen,
/// where the caption is hidden.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shortcut {
    Desktop(usize),
    /// A new window (Cmd+N; Ctrl+Shift+N elsewhere).
    NewWindow,
    /// Closes the window (Cmd+W; Ctrl+Shift+W elsewhere).
    CloseWindow,
    /// The next (+1) or previous (-1) tab, round (Ctrl+Tab, Ctrl+Shift+Tab).
    NextTab(i32),
    /// Fullscreen on / off (Cmd+Ctrl+F on macOS, F11 elsewhere).
    Fullscreen,
    Reconnect,
    NextScreen,
    Details,
    History,
}

pub(crate) fn shortcut(e: &KeyEvent) -> Option<Shortcut> {
    let m = &e.modifiers;
    let (plain, shifted) = if cfg!(target_vendor = "apple") { (m.logo && !m.shift, m.logo && m.shift) } else { (m.control && m.shift, m.control && m.shift) };
    if e.key_code == KeyCode::Tab && m.control && !m.logo && !m.alt {
        return Some(Shortcut::NextTab(if m.shift { -1 } else { 1 }));
    }
    let fullscreen = if cfg!(target_vendor = "apple") {
        e.key_code == KeyCode::KeyF && m.logo && m.control && !m.shift && !m.alt
    } else {
        e.key_code == KeyCode::F11 && !m.logo && !m.control && !m.shift && !m.alt
    };
    if fullscreen {
        return Some(Shortcut::Fullscreen);
    }
    let digits = [KeyCode::Key1, KeyCode::Key2, KeyCode::Key3, KeyCode::Key4, KeyCode::Key5, KeyCode::Key6, KeyCode::Key7, KeyCode::Key8, KeyCode::Key9];
    if plain {
        if let Some(i) = digits.iter().position(|k| *k == e.key_code) {
            return Some(Shortcut::Desktop(i));
        }
        if e.key_code == KeyCode::KeyR {
            return Some(Shortcut::Reconnect);
        }
        // macOS: the menu's key equivalent takes Cmd+N first (the key then never arrives here)
        if e.key_code == KeyCode::KeyN && cfg!(target_vendor = "apple") {
            return Some(Shortcut::NewWindow);
        }
        if e.key_code == KeyCode::KeyW && cfg!(target_vendor = "apple") {
            return Some(Shortcut::CloseWindow);
        }
    }
    if shifted {
        return match e.key_code {
            KeyCode::KeyN if !cfg!(target_vendor = "apple") => Some(Shortcut::NewWindow),
            KeyCode::KeyW if !cfg!(target_vendor = "apple") => Some(Shortcut::CloseWindow),
            KeyCode::KeyS => Some(Shortcut::NextScreen),
            KeyCode::KeyL => Some(Shortcut::Details),
            KeyCode::KeyH => Some(Shortcut::History),
            _ => None,
        };
    }
    None
}

impl App {
    /// A JIT host the devhub lists since the launch (`makepad devhub desk <node> on`) becomes a
    /// tab, connected and paused like every desktop no window shows (`--connect-only`: none).
    fn add_devhub_desktops(&mut self) {
        if std::env::args().any(|a| a == "--connect-only") {
            return;
        }
        for (name, host, port) in devhub_desktops() {
            if self.conns.iter().any(|c| c.host == host && c.port == port) || is_self(&host) {
                continue;
            }
            self.conns.push(Connection::open(name, host, port, true, self.signal.clone()));
        }
    }
}

impl App {
    fn poll_pads(&mut self, cx: &mut Cx) {
        let states: Vec<Option<GamepadState>> = cx
            .game_input_states()
            .iter()
            .map(|s| match s {
                makepad_widgets::makepad_platform::event::game_input::GameInputState::Gamepad(g) => Some(pad_of(g)),
                _ => None,
            })
            .collect();
        // the window with the keyboard drives its desktop
        let Some(conn) = self.viewers.get(self.focused).and_then(|v| v.selected).and_then(|i| self.conns.get(i)) else { return };
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

    /// `--scrub-check`: one step of the script each tick, once the shown picture is there (the
    /// first window).
    fn check_tick(&mut self, cx: &mut Cx) {
        let Some(mut check) = self.check.take() else { return };
        let vi = 0;
        let sel = self.viewers[vi].selected;
        let have = sel.and_then(|i| self.conns.get(i)).is_some_and(|c| c.history.lock().unwrap().len() > 0);
        if check.started.is_none() && have {
            check.started = Some(std::time::Instant::now());
        }
        // no pictures at all for a minute: the check cannot run (says why, exits 1)
        let waited = check.created.elapsed().as_secs_f64();
        if check.started.is_none() && waited > 60.0 {
            let state = sel.and_then(|i| self.conns.get(i)).map(|c| format!("{:?}", c.state())).unwrap_or_else(|| "no desktop".into());
            eprintln!("scrub-check: FAILED: no pictures in {waited:.0} s ({state})");
            std::process::exit(1);
        }
        if self.viewers[vi].scrub.is_none() {
            let elapsed = check.started.map_or(0.0, |t| t.elapsed().as_secs_f64());
            if check.reconnect_wanted && check.reconnect.is_none() && elapsed >= check.after / 2.0 {
                if let Some(i) = sel {
                    self.reconnect(cx, i);
                }
                check.reconnect = Some((std::time::Instant::now(), false, None));
            }
            if let (Some((at, offline, None)), Some(c)) = (check.reconnect, sel.and_then(|i| self.conns.get(i))) {
                if c.state() != State::Connected {
                    check.reconnect = Some((at, true, None));
                } else if offline && c.shared.lock().unwrap().welcome.is_some() {
                    check.reconnect = Some((at, true, Some(at.elapsed().as_secs_f64() * 1000.0)));
                }
            }
            if check.started.is_some_and(|t| t.elapsed().as_secs_f64() >= check.after) {
                self.enter_history(cx, vi, None);
                check.waiting = Some((Check::FIRST, std::time::Instant::now()));
            }
            self.check = Some(check);
            return;
        }
        let v = &self.viewers[vi];
        if let Some((kind, at)) = check.waiting {
            let done = if kind == Check::SAVE {
                let s = self.clip_status.lock().unwrap().clone();
                s.starts_with("saved") || s.starts_with("save failed")
            } else {
                v.hist_shown == Some(v.cur)
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
                // stays paused on the last picture (a grab of the scrubber)
                self.update_scrubber(cx, vi);
                return;
            }
            self.viewers[vi].scrub = None;
            std::process::exit(0);
        };
        // steps at the pace of a held arrow key (about 30 a second)
        if check.last_step.is_some_and(|t| t.elapsed().as_secs_f64() < 1.0 / 30.0) {
            check.steps.push_front(step);
            self.check = Some(check);
            return;
        }
        let n = self.viewers[vi].scrub.as_ref().map_or(0, |s| s.snap.len());
        let now = std::time::Instant::now();
        check.last_step = Some(now);
        check.waiting = Some((step, now));
        self.check = Some(check);
        match step {
            Check::SAVE => self.save(cx, vi, false),
            Check::JUMP => self.scrub_action(cx, vi, ScrubAction::Seek(n / 2)),
            d => self.step(cx, vi, d),
        }
        // a decoded picture shows at once: no wait at all
        let v = &self.viewers[vi];
        if step != Check::SAVE && v.hist_shown == Some(v.cur) {
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
    let (n, span, mb) = app.viewers[0].scrub.as_ref().map_or((0, 0.0, 0.0), |s| {
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
        installed::write_pid();
        let args: Vec<String> = std::env::args().collect();
        if let Some(i) = args.iter().position(|a| a == "--decode-bench") {
            decode_bench(args.get(i + 1).map(String::as_str).unwrap_or(""));
        }
        if let Some(i) = args.iter().position(|a| a == "--dump-check") {
            dump_check(args.get(i + 1).map(String::as_str).unwrap_or(""));
        }
        self.wall0 = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64()) - conn::now_ns() as f64 / 1e9;
        let main = self.ui.widget(cx, ids!(main_window));
        self.viewers = vec![Viewer::new(main, false)];
        self.stats_log = args.iter().any(|a| a == "--stats");
        for (flag, what) in [("--check-scrubber", "scrubber"), ("--check-split", "split")] {
            if args.iter().any(|a| a == flag) {
                self.check_ui.push(what.to_string());
            }
        }
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
        }
        self.measure_refresh(cx);
        for (name, host, port) in hosts() {
            let paused = !self.conns.is_empty();
            self.conns.push(Connection::open(name, host, port, paused, self.signal.clone()));
        }
        // the last run's windows, each on its machine (`--select N`: the first window's)
        let first = args.windows(2).find(|w| w[0] == "--select").and_then(|w| w[1].parse::<usize>().ok()).filter(|n| *n < self.conns.len());
        self.restore_windows(cx, first);
        self.refresh(cx);
        self.tick = cx.start_interval(1.0);
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for vi in (0..self.viewers.len()).rev() {
            let Some(win) = self.viewers.get(vi).map(|v| v.window.clone()) else { continue };
            for a in actions.filter_widget_actions_cast::<TabsAction>(win.widget(cx, ids!(tabs)).widget_uid()) {
                if vi < self.viewers.len() {
                    self.tabs_action(cx, vi, a);
                }
            }
            for a in actions.filter_widget_actions_cast::<ToggleAction>(win.widget(cx, ids!(history_toggle)).widget_uid()) {
                if let (ToggleAction::Clicked, true) = (a, vi < self.viewers.len()) {
                    self.focused = vi;
                    self.toggle_scrubber(cx, vi);
                }
            }
            for a in actions.filter_widget_actions_cast::<ScrubAction>(win.widget(cx, ids!(scrubber)).widget_uid()) {
                if vi < self.viewers.len() {
                    self.focused = vi;
                    self.scrub_action(cx, vi, a);
                }
            }
            for a in actions.filter_widget_actions_cast::<PopMenuAction>(win.widget(cx, ids!(menu)).widget_uid()) {
                match a {
                    PopMenuAction::Chose(id) if vi < self.viewers.len() => self.menu_chose(cx, vi, id),
                    PopMenuAction::Dismissed if vi < self.viewers.len() => self.viewers[vi].menu_for = viewer::MenuFor::None,
                    _ => {}
                }
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
        caption::script_mod(vm);
        scrubber::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Shutdown = event {
            installed::clear_pid();
            self.save_windows(cx);
            // every connection's threads (and the scrub decoders) end and are joined before the app goes
            // nothing stays pressed on a desktop: the ups go out before the connections close
            for vi in 0..self.viewers.len() {
                self.release_held(vi, "quit");
            }
            for v in &mut self.viewers {
                v.scrub = None;
                v.preview = None;
            }
            self.conns.clear();
            return;
        }
        if self.viewers.is_empty() {
            // until the startup event made the first viewer
            self.match_event(cx, event);
            self.ui.handle_event(cx, event, &mut Scope::empty());
            return;
        }
        match event {
            // the first window closes into the Dock (macOS) or while other windows show; the
            // others close
            Event::WindowCloseRequested(e) => {
                if let Some(vi) = self.viewer_of_window(e.window_id) {
                    if self.keep_on_close(vi) {
                        e.accept_close.set(false);
                        self.hide_main(cx);
                        return;
                    }
                }
            }
            // opened again (the Dock icon, Finder, a second launch) with no window on screen
            Event::AppOpen(e) if e.items.is_empty() => {
                self.show_some_window(cx);
                return;
            }
            // the menu bar's File → New Window (Cmd-N) and the Dock icon's New Window
            Event::MacosMenuCommand(id) if *id == live_id!(new_window) => {
                self.new_window(cx, None, None);
                return;
            }
            // File → Close Window (Cmd-W): the focused window
            Event::MacosMenuCommand(id) if *id == live_id!(close_window) => {
                let vi = self.focused.min(self.viewers.len() - 1);
                self.do_shortcut(cx, vi, Shortcut::CloseWindow);
                return;
            }
            // the focused window drives its machine (its keys) and sizes a desktop that follows
            Event::WindowGotFocus(id) => {
                if let Some(vi) = self.viewer_of_window(*id) {
                    self.focused = vi;
                    self.claim_size(cx, vi);
                }
            }
            // focus gone (Cmd-Tab, another window, a sheet): what this window holds down on its
            // desktop is let go, the modifiers too; the OS sends us no ups for it any more
            Event::WindowLostFocus(id) => {
                if let Some(vi) = self.viewer_of_window(*id) {
                    self.release_held(vi, "window lost focus");
                }
            }
            Event::Pause | Event::Background => {
                for vi in 0..self.viewers.len() {
                    self.release_held(vi, "app inactive");
                }
            }
            Event::KeyDown(_) | Event::KeyUp(_) => {
                let vi = self.focused.min(self.viewers.len() - 1);
                self.key_event(cx, vi, event);
            }
            _ => {}
        }
        for vi in 0..self.viewers.len() {
            if let Some(e) = self.viewers[vi].play_frame.is_event(event) {
                self.play_tick(cx, vi, e.time);
            }
        }
        if self.check.as_ref().is_some_and(|c| c.timer.is_event(event).is_some()) {
            self.check_tick(cx);
        }
        let pointer_window = match event {
            Event::MouseMove(e) => Some(e.window_id),
            Event::MouseDown(e) => Some(e.window_id),
            Event::MouseUp(e) => Some(e.window_id),
            Event::Scroll(e) => Some(e.window_id),
            _ => None,
        };
        if let Some(vi) = pointer_window.and_then(|id| self.viewer_of_window(id)) {
            self.pointer_event(cx, vi, event);
        }
        if let Event::GameInputConnected(_) = event {
            self.pad_poll = cx.new_next_frame();
        }
        if let Some(e) = self.vsync.is_event(event) {
            self.vsync_frame(cx, e.time);
        }
        if let Event::WindowGeomChange(e) = event {
            if let Some(vi) = self.viewer_of_window(e.window_id) {
                // another screen (a 120 Hz panel, a 60 Hz external one): measured again
                self.measure_refresh(cx);
                let g = &e.new_geom;
                let v = &mut self.viewers[vi];
                v.window_px = [(g.inner_size.x * g.dpi_factor).round() as u32, (g.inner_size.y * g.dpi_factor).round() as u32];
                let changed = v.fullscreen != g.is_fullscreen;
                self.restore_dirty = true;
                v.fullscreen = g.is_fullscreen;
                if changed {
                    viewer::selftest(&format!("window {vi} fullscreen {}", g.is_fullscreen));
                    self.release_held(vi, "fullscreen changed");
                }
            }
        }
        if self.pad_poll.is_event(event).is_some() {
            self.poll_pads(cx);
        }
        if self.snap_tick.is_event(event).is_some() {
            for vi in 0..self.viewers.len() {
                self.refresh_live_snap(cx, vi);
            }
        }
        if self.tick.is_event(event).is_some() {
            for vi in 0..self.viewers.len() {
                self.viewer_second(vi);
            }
            self.add_devhub_desktops();
            self.refresh(cx);
            self.check_ui_tick(cx);
            if self.restore_dirty {
                self.save_windows(cx);
            }
            if self.stats_log {
                for v in &self.viewers {
                    let Some(c) = v.selected.and_then(|i| self.conns.get(i)) else { continue };
                    let (fps, mbps, rtt, stream_fps, gap, queue, rate) = {
                        let s = c.shared.lock().unwrap();
                        (s.fps, s.mbps, s.rtt_ms.unwrap_or(0.0), s.stream_fps, s.read_gap_ms, s.decode_queue, s.stream_rate.clone())
                    };
                    let display = crate::conn::DISPLAY_HZ.load(std::sync::atomic::Ordering::Acquire);
                    eprintln!("desklink stats {}: rtt {rtt:.2} ms · {} · {} · {} · {}", c.name, view::latency_line(&v.latency), view::summary_line(v.shown_rate, mbps, rate.as_ref()), view::rate_details_line(fps, stream_fps, display, rate.as_ref()), view::flow_line(gap, queue));
                }
            }
        }
        if let Event::Draw(_) = event {
            // a picture drawn: its present time is known now
            let drawn = self.viewers.iter().any(|v| v.window.widget(cx, ids!(remote)).borrow::<RemoteView>().is_some_and(|r| r.drawn.is_some()));
            if drawn {
                self.signal.set();
            }
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        self.extra_windows_event(cx, event);
    }
}

impl App {
    /// The windows made at run time: their events and their drawing (the root has the first).
    fn extra_windows_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::WindowClosed(e) = event {
            if let Some(vi) = self.viewer_of_window(e.window_id).filter(|vi| self.viewers[*vi].extra) {
                self.window_closed(cx, vi);
                return;
            }
        }
        for vi in 0..self.viewers.len() {
            if !self.viewers[vi].extra {
                continue;
            }
            let window = self.viewers[vi].window.clone();
            let id = self.viewers[vi].window_id();
            if let Event::Draw(draw) = event {
                let created = id.is_some_and(|id| cx.windows.is_valid(id) && cx.windows[id].is_created);
                let mut cx_draw = CxDraw::new(cx, draw);
                window.draw_all(&mut Cx2d::new(&mut cx_draw), &mut Scope::empty());
                if created && draw.redraw_all {
                    self.viewers[vi].drawn = true;
                }
                continue;
            }
            window.handle_event(cx, event, &mut Scope::empty());
            // a window's first draw: its lists do not exist yet, so a redraw of it alone would
            // not reach it
            if !self.viewers[vi].drawn && id.is_some_and(|id| cx.windows.is_valid(id) && cx.windows[id].is_created) {
                cx.redraw_all();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_have_short_names() {
        assert_eq!(short_name("n165", "10.0.0.165"), "n165");
        assert_eq!(short_name("10.0.0.165:8790", "10.0.0.165"), "n165");
        assert_eq!(short_name("10.0.0.123", "10.0.0.123"), "n123");
        assert_eq!(short_name("127.0.0.1:18796", "127.0.0.1"), "local");
        assert_eq!(short_name("mini.lan", "mini.lan"), "mini");
    }

    #[test]
    fn devhub_endpoints_are_named_by_node() {
        let list = parse_desk_links("quest 10.0.0.201:8790\nphone 10.0.0.50\n\nbroken\n");
        assert_eq!(list, vec![("quest".to_string(), "10.0.0.201".to_string(), 8790), ("phone".to_string(), "10.0.0.50".to_string(), proto::DEFAULT_PORT)]);
    }
}
