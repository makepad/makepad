//! Desk Link — the viewer of the window manager's remote desktop.
//!
//! The side panel lists the desktops it knows (saved hosts in `~/.makepad/desklink/hosts`,
//! `--connect host[:port]`, and the fleet nodes this machine has met): each with its state and
//! round-trip time, each kept open and paused while it is not the one shown. Click one to show
//! it. The picture is the remote screen at its own resolution, scaled to the window
//! (aspect-locked, letterboxed) or shown 1:1; input is mapped back through the same placement.
//! Keyboard input goes to the desktop once the picture was clicked, until the panel is.
//! Fullscreen hides the panel (the picture is the whole screen, 1:1); Cmd+Shift+P (Ctrl+Shift+P
//! off macOS) shows or hides it in any window.
//!
//! Dev flags: `--connect host[:port]` (repeatable), `--select N`, `--stats` (latency lines on
//! stderr once a second), `--dump-check <file.h264>` (decode an Annex-B dump with the hardware
//! decoder, print what came out, exit), `--dump-play <dump dir | file.h264[@x0,y0],...>` (show a dump, or the tiles of one, as a desktop, at
//! 60 pictures a second, looping; `--cpu-decode` takes the CPU NV12 path instead of GPU pictures, `--still N` stops at picture N),
//! `--decode-bench <dir>` (the tile streams of a `desk-link-probe --dump`: each on its own decoder
//! thread as the viewer runs them, every picture's tiles at once; prints the decode time of a
//! whole picture and the pictures a second the decoders sustain, exit).

mod conn;
mod view;

pub use makepad_widgets;
use conn::{Connection, Latency, State};
use makepad_desk_link::{self as proto, ClientMsg, GamepadState};
use makepad_widgets::*;
use view::{ConnList, ConnListAction, RemoteView, Row};

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
                    split := View{
                        width: Fill height: Fill flow: Right
                        panel := View{
                            width: 250 height: Fill flow: Down spacing: 6
                            padding: Inset{left: 8 right: 8 top: 10 bottom: 10}
                            show_bg: true
                            draw_bg +: { color: #x18191d }
                            Label{text: "Desktops" draw_text +: {color: #x9a9aa2 text_style: theme.font_bold{font_size: 9}}}
                            conn_list := ConnList{height: Fill}
                            host_input := TextInput{width: Fill empty_text: "host:port"}
                            add_host := Button{width: Fill text: "Connect"}
                            one_to_one := CheckBox{text: "1:1 pixels"}
                            integer_scale := CheckBox{text: "Whole-pixel scale" active: true}
                            details := CheckBox{text: "Latency details"}
                            screen_next := Button{width: Fill text: "Next screen"}
                            split_mode := Button{width: Fill text: "Engines: tiles"}
                            rate_cap := Button{width: Fill text: "Rate: auto"}
                        }
                        View{
                            width: Fill height: Fill
                            remote := RemoteView{}
                        }
                    }
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
    /// The window is fullscreen: the panel goes (the picture is the whole screen, 1:1) and comes
    /// back on leaving fullscreen or with the panel toggle (Cmd+Shift+P).
    #[rust]
    fullscreen: bool,
    #[rust]
    panel_shown: bool,
    /// The window's inner size in device pixels (fullscreen: the screen's backing size).
    #[rust]
    window_px: [u32; 2],
    #[rust]
    stats_log: bool,
    /// The engine split asked of the desktop (`proto::split::*`; desktops start with tiles).
    #[rust(proto::split::TILES)]
    split: u8,
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
}

fn port_of(spec: &str) -> (String, u16) {
    match spec.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && p.parse::<u16>().is_ok() && !h.contains(']') || h.ends_with(']') && p.parse::<u16>().is_ok() => {
            (h.trim_start_matches('[').trim_end_matches(']').to_string(), p.parse().unwrap())
        }
        _ => (spec.trim_start_matches('[').trim_end_matches(']').to_string(), proto::DEFAULT_PORT),
    }
}

/// The desktops to list: saved hosts, `--connect` ones, the fleet nodes this machine knows
/// (`true`: found in `known_nodes`, maybe without a desktop: retried rarely while offline).
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
    let home = makepad_network::fleet::makepad_home();
    if let Ok(text) = std::fs::read_to_string(home.join("desklink/hosts")) {
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let mut parts = line.split_whitespace();
            match (parts.next(), parts.next()) {
                (Some(name), Some(spec)) => add(name.to_string(), spec),
                (Some(spec), None) => add(spec.to_string(), spec),
                _ => {}
            }
        }
    }
    // fleet nodes met before (their AI hub's address): their desktop is on the Desk Link port
    let known = makepad_network::fleet::auth::client_dir().join(makepad_network::fleet::auth::KNOWN_NODES);
    if let Ok(text) = std::fs::read_to_string(known) {
        for line in text.lines() {
            let Some(addr) = line.split_whitespace().next() else { continue };
            let (host, port) = port_of(addr);
            if port == proto::DEFAULT_PORT {
                continue;
            }
            put(host.clone(), &host, true);
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
        if let Some(c) = self.conns.get(self.selected) {
            c.show(false);
        }
        self.selected = index;
        self.conns[index].show(true);
        self.last_pointer = None;
        if let Some(mut v) = self.remote(cx).borrow_mut::<RemoteView>() {
            v.clear(cx);
        }
        if let Some(mut l) = self.ui.widget(cx, ids!(conn_list)).borrow_mut::<ConnList>() {
            l.selected = index;
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

    /// The panel rows, the picture, the cursor, the overlay lines.
    fn refresh(&mut self, cx: &mut Cx) {
        let rows: Vec<Row> = self
            .conns
            .iter()
            .map(|c| {
                let s = c.shared.lock().unwrap();
                let state = s.state.clone().unwrap_or(State::Connecting);
                let detail = match &state {
                    State::Connected => format!("{}:{} · {}", c.host, c.port, s.rtt_ms.map(|r| format!("{r:.1} ms")).unwrap_or_else(|| "…".into())),
                    State::Connecting => format!("{}:{} · connecting", c.host, c.port),
                    State::Reconnecting => format!("{}:{} · reconnecting", c.host, c.port),
                    State::Offline(why) => format!("offline · {why}"),
                };
                Row { name: c.name.clone(), detail, state }
            })
            .collect();
        if let Some(mut l) = self.ui.widget(cx, ids!(conn_list)).borrow_mut::<ConnList>() {
            l.set_rows(cx, rows);
        }
        let Some(c) = self.conns.get(self.selected) else { return };
        let picture = c.latest.lock().unwrap().take();
        let (state, welcome, cursor, offset, fps, mbps, warning, stream_fps, read_gap_ms, decode_queue, stream_rate) = {
            let s = c.shared.lock().unwrap();
            (s.state.clone().unwrap_or(State::Connecting), s.welcome.clone(), s.cursor, s.offset_ns, s.fps, s.mbps, s.warning.clone(), s.stream_fps, s.read_gap_ms, s.decode_queue, s.stream_rate)
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
            view.set_picture(cx, p.width, p.height, p.parts, p.frame, p.decoded_ns);
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
        lines.push(view::rate_line(fps, mbps, stream_fps, crate::conn::DISPLAY_HZ.load(std::sync::atomic::Ordering::Acquire), self.shown_rate));
        if let Some(rate) = stream_rate {
            lines.push(view::stream_rate_line(rate));
        }
        // view only (the desktop cannot inject input): always said, never a silent dead mouse
        if let Some(w) = welcome.as_ref().filter(|w| !w.input) {
            lines.push(if w.input_note.is_empty() { "input unavailable on this host".to_string() } else { w.input_note.clone() });
        }
        if self.details {
            lines.push(view::latency_line(&self.latency));
            lines.push(view::flow_line(read_gap_ms, decode_queue));
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
    }

    fn pointer_event(&mut self, cx: &mut Cx, event: &Event) {
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
            Event::KeyDown(e) | Event::KeyUp(e) if is_panel_toggle(e) => {}
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

/// The rate cap button's steps (Mbit/s; 0 = auto).
const RATE_CAPS: [u32; 6] = [0, 800, 400, 250, 150, 80];

/// The panel toggle: Cmd+Shift+P on macOS, Ctrl+Shift+P elsewhere (never sent to the desktop).
fn is_panel_toggle(e: &KeyEvent) -> bool {
    e.key_code == KeyCode::KeyP && e.modifiers.shift && if cfg!(target_os = "macos") { e.modifiers.logo } else { e.modifiers.control }
}

impl App {
    /// The side panel shown or gone (gone: the picture has the whole window, fullscreen the
    /// whole screen at 1:1).
    fn show_panel(&mut self, cx: &mut Cx, on: bool) {
        self.panel_shown = on;
        self.ui.view(cx, ids!(panel)).set_visible(cx, on);
        self.ui.redraw(cx);
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
        self.panel_shown = true;
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
        for action in actions.filter_widget_actions_cast::<ConnListAction>(self.ui.widget(cx, ids!(conn_list)).widget_uid()) {
            if let ConnListAction::Select(i) = action {
                self.keyboard = false;
                self.select(cx, i);
            }
        }
        let input = self.ui.text_input(cx, ids!(host_input));
        let typed = input.returned(actions).map(|(t, _)| t);
        let clicked = self.ui.button(cx, ids!(add_host)).clicked(actions).then(|| input.text());
        if let Some(spec) = typed.or(clicked).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
            let (host, port) = port_of(&spec);
            match self.conns.iter().position(|c| c.host == host && c.port == port) {
                Some(i) => self.select(cx, i),
                None => {
                    self.add(cx, spec.clone(), host, port, false);
                    let last = self.conns.len() - 1;
                    self.select(cx, last);
                }
            }
        }
        if let Some(on) = self.ui.check_box(cx, ids!(details)).changed(actions) {
            self.details = on;
            self.remote(cx).redraw(cx);
        }
        if let Some(on) = self.ui.check_box(cx, ids!(integer_scale)).changed(actions) {
            if let Some(mut v) = self.remote(cx).borrow_mut::<RemoteView>() {
                v.integer_scale = on;
                v.redraw(cx);
            }
        }
        if let Some(on) = self.ui.check_box(cx, ids!(one_to_one)).changed(actions) {
            if let Some(mut v) = self.remote(cx).borrow_mut::<RemoteView>() {
                v.one_to_one = on;
                v.redraw(cx);
            }
        }
        // a desktop with several encode engines: tiles (lowest latency) -> interleave (most
        // pictures per second) -> one engine
        if self.ui.button(cx, ids!(split_mode)).clicked(actions) {
            self.split = match self.split {
                proto::split::TILES => proto::split::INTERLEAVE,
                proto::split::INTERLEAVE => proto::split::OFF,
                _ => proto::split::TILES,
            };
            let label = match self.split {
                proto::split::TILES => "Engines: tiles",
                proto::split::INTERLEAVE => "Engines: interleave",
                _ => "Engines: one",
            };
            self.ui.button(cx, ids!(split_mode)).set_text(cx, label);
            if let Some(c) = self.current() {
                c.send(ClientMsg::SetSplit { split: self.split });
                c.send(ClientMsg::RequestKeyframe);
            }
        }
        // the stream's rate: auto (the desktop adapts it to the link and its encoder, up to its
        // own cap) or at most a chosen rate
        if self.ui.button(cx, ids!(rate_cap)).clicked(actions) {
            use std::sync::atomic::Ordering;
            let now = crate::conn::RATE_CAP_MBPS.load(Ordering::Acquire);
            let next = RATE_CAPS.iter().position(|c| *c == now).map_or(0, |i| (i + 1) % RATE_CAPS.len());
            let cap = RATE_CAPS[next];
            crate::conn::RATE_CAP_MBPS.store(cap, Ordering::Release);
            let label = if cap == 0 { "Rate: auto".to_string() } else { format!("Rate: at most {cap} Mbit/s") };
            self.ui.button(cx, ids!(rate_cap)).set_text(cx, &label);
        }
        if self.ui.button(cx, ids!(screen_next)).clicked(actions) {
            if let Some(c) = self.current() {
                let next = c.shared.lock().unwrap().welcome.as_ref().map(|w| (w.screen + 1) % w.screens.len().max(1) as u32);
                if let Some(index) = next {
                    c.send(ClientMsg::SelectScreen { index });
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
        view::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Shutdown = event {
            // every connection's threads end and are joined before the app goes
            self.conns.clear();
            return;
        }
        if let Event::KeyDown(e) = event {
            if is_panel_toggle(e) && !e.is_repeat {
                let on = !self.panel_shown;
                self.show_panel(cx, on);
            }
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
            if g.is_fullscreen != self.fullscreen {
                self.fullscreen = g.is_fullscreen;
                self.show_panel(cx, !self.fullscreen);
            }
        }
        if self.pad_poll.is_event(event).is_some() {
            self.poll_pads(cx);
        }
        if self.tick.is_event(event).is_some() {
            let dropped = self.current().map_or(0, |c| std::mem::take(&mut c.shared.lock().unwrap().dropped));
            self.shown_rate = (std::mem::take(&mut self.shown), dropped);
            self.refresh(cx);
            if let (Some((frame, decode_us, present_us)), Some(c)) = (self.last_stats.take(), self.current()) {
                c.send(ClientMsg::Stats { frame, decode_us, present_us });
            }
            if self.stats_log {
                let (fps, mbps, rtt, stream_fps, gap, queue) = self.current().map(|c| {
                    let s = c.shared.lock().unwrap();
                    (s.fps, s.mbps, s.rtt_ms.unwrap_or(0.0), s.stream_fps, s.read_gap_ms, s.decode_queue)
                }).unwrap_or_default();
                eprintln!("desklink stats: rtt {rtt:.2} ms · {} · {} · {}", view::latency_line(&self.latency), view::rate_line(fps, mbps, stream_fps, crate::conn::DISPLAY_HZ.load(std::sync::atomic::Ordering::Acquire), self.shown_rate), view::flow_line(gap, queue));
            }
        }
        if let Event::Draw(_) = event {
            // a picture drawn: its present time is known now
            let drawn = self.remote(cx).borrow::<RemoteView>().is_some_and(|v| v.drawn.is_some());
            if drawn {
                self.signal.set();
            }
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

