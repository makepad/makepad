//! browser_host: Chromium for one makepad app, in a process of its own.
//!
//! Started by libs/browser_view as a hosted child (`--stdin-loop`), never by
//! hand. Each page the app opens is a CEF browser (accelerated paint) and a
//! window here that is never drawn: it only receives the page's geometry
//! and input, the hosted-app way (no shared swapchain is made for it). The
//! page is driven by `ViewOp`s and reports `ViewEvent`s on the custom
//! channel (libs/browser_view/src/protocol.rs).
//!
//! Frames: the app sends each page a frame ring (makepad_app_host::
//! frame_ring); CEF's accelerated paint is copied straight into the ring's
//! next slot on the GPU and stamped (a lossless ring makes CEF's paint wait
//! for a free slot). Sound: an audio ring setup turns on CEF's audio capture
//! for the page and its packets go into that shared-memory ring.
//!
//! A closed page's window is kept and the next page opens in it: a hosted
//! child cannot close one of its windows on its own. Its browser goes.

pub use makepad_widgets;

use makepad_app_host::{AudioRecord, AudioRingPacket, AudioRingSetup, ProducerAudioRing, ProducerRing, RingSetup};
use makepad_browser_view::cef_input;
use makepad_browser_view::protocol::{self, ViewEvent, ViewOp};
use makepad_cef::{AudioCaptureConfig, AudioEvent, BootstrapResult, Browser, CaptureTimestamp, FrameSink, SinkTarget};
use makepad_widgets::*;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;

script_mod! {
    use mod.prelude.widgets.*
    load_all_resources() do #(App::script_component(vm)){
    }
}

/// How often the pages are pumped and their state read (titles, loading,
/// consoles, sound).
const POLL: f64 = 1.0 / 60.0;
/// Two presses this close (seconds, points) are a double click.
const MULTI_CLICK_SECONDS: f64 = 0.5;
const MULTI_CLICK_DISTANCE: f64 = 4.0;

/// What the app was last told about a page.
#[derive(Clone, PartialEq)]
struct Shown {
    title: String,
    url: String,
    loading: bool,
    can_go_back: bool,
    can_go_forward: bool,
    epoch: u64,
}

struct Page {
    /// The app's view id; None: a free window waiting for the next page.
    view: Option<u64>,
    window: WindowHandle,
    browser: Option<Browser>,
    /// The page's size in points and its dpi, from the window's geometry.
    size: Option<(Vec2d, f64)>,
    /// What to load once the page exists (it needs its size first).
    url: Option<String>,
    error: Option<String>,
    shown: Option<Shown>,
    error_sent: bool,
    pressed: MouseButton,
    scroll_remainder: Vec2d,
    /// A paste arrived as text ahead of its key chord.
    pasted_text: bool,
    /// The last press: when, where, and its click count.
    last_press: (f64, Vec2d, i32),
    /// Applied at the next poll once the page exists.
    dark: Option<bool>,
    hidden: Option<bool>,
    evals: Vec<(u64, String)>,
    /// CEF's evaluation numbers and the app's.
    eval_ids: Vec<(u64, u64)>,
    /// The page's frame ring (CEF's sink once the page exists).
    ring: Option<Arc<RingSink>>,
    /// The page's sound: its ring, and records it had no room for yet.
    audio: Option<ProducerAudioRing>,
    audio_start: bool,
    audio_backlog: VecDeque<AudioRecord>,
}

/// A frame ring as CEF's frame sink.
struct RingSink(ProducerRing);

impl FrameSink for RingSink {
    fn acquire(&self, _width: usize, _height: usize) -> Option<SinkTarget> {
        let slot = self.0.acquire()?;
        let target = &self.0.setup.slots[slot];
        Some(SinkTarget { slot, handle: target.handle, width: target.width as usize, height: target.height as usize })
    }

    fn probes(&self) -> Vec<[u32; 2]> {
        self.0.probes().collect()
    }

    fn publish(&self, slot: usize, copied: (usize, usize), timestamp: CaptureTimestamp, probes: &[u8]) {
        self.0.publish(slot, copied.0 as u32, copied.1 as u32, timestamp.callback_unix_ms, timestamp.callback_elapsed_ns, timestamp.navigation_epoch, probes);
    }

    fn abandon(&self, slot: usize) {
        self.0.abandon(slot);
    }
}

/// Packets a page's sound arrives in.
const AUDIO_CONFIG: AudioCaptureConfig = AudioCaptureConfig { sample_rate: 48_000, channels: 2, frames_per_buffer: 480 };

#[derive(Script, ScriptHook)]
pub struct App {
    #[rust]
    pages: Vec<Page>,
    #[rust]
    poll: Timer,
    /// The page holding the keyboard (its view id).
    #[rust]
    focus: Option<u64>,
}

impl Page {
    fn new(cx: &mut Cx) -> Page {
        Page {
            view: None,
            window: WindowHandle::new(cx),
            browser: None,
            size: None,
            url: None,
            error: None,
            shown: None,
            error_sent: false,
            pressed: MouseButton::empty(),
            scroll_remainder: Vec2d::default(),
            pasted_text: false,
            last_press: (f64::MIN, Vec2d::default(), 0),
            dark: None,
            hidden: None,
            evals: Vec::new(),
            eval_ids: Vec::new(),
            ring: None,
            audio: None,
            audio_start: false,
            audio_backlog: VecDeque::new(),
        }
    }

    /// The page in pixels.
    fn pixels(&self) -> Option<(usize, usize, f32)> {
        let (size, dpi) = self.size?;
        Some((((size.x * dpi).round().max(1.0)) as usize, ((size.y * dpi).round().max(1.0)) as usize, dpi as f32))
    }

    /// Make the page once it has a size and somewhere to go.
    fn ensure_browser(&mut self) {
        if self.browser.is_some() || self.error.is_some() {
            return;
        }
        let Some((width, height, dpi)) = self.pixels() else { return };
        let Some(url) = self.url.take() else { return };
        match Browser::new(&url, width, height, dpi) {
            Ok(browser) => self.browser = Some(browser),
            Err(error) => {
                eprintln!("browser host: page {:?}: {error}", self.view);
                self.error = Some(error.to_string());
            }
        }
    }

    fn navigate(&mut self, url: &str) {
        match &mut self.browser {
            Some(browser) => {
                if let Err(error) = browser.set_url(url) {
                    self.error = Some(error.to_string());
                }
            }
            None => self.url = Some(url.to_string()),
        }
        self.ensure_browser();
    }

    /// A copy or cut: the selection goes on the shared clipboard and the
    /// page runs its own command too (its copy/cut event, a cut's removal).
    fn clipboard(browser: &mut Browser, event: &TextClipboardEvent, cut: bool) {
        let text = browser.selected_text();
        let _ = if cut { browser.cut_selection() } else { browser.copy_selection() };
        if !text.is_empty() {
            *event.response.borrow_mut() = Some(text);
        }
    }

    fn point(abs: Vec2d) -> (i32, i32) {
        (abs.x.round() as i32, abs.y.round() as i32)
    }

    /// One of the window's input events, as CEF input.
    fn input(&mut self, cx: &mut Cx, event: &Event) {
        let Some(browser) = &mut self.browser else { return };
        match event {
            Event::MouseDown(e) => {
                let clicks = if e.time - self.last_press.0 < MULTI_CLICK_SECONDS && (e.abs - self.last_press.1).length() < MULTI_CLICK_DISTANCE { self.last_press.2 + 1 } else { 1 };
                self.last_press = (e.time, e.abs, clicks);
                self.pressed.insert(e.button);
                let _ = browser.set_focus(true);
                let (x, y) = Self::point(e.abs);
                let modifiers = cef_input::cef_modifiers(e.modifiers, self.pressed);
                let _ = browser.send_mouse_move(x, y, modifiers, false);
                let _ = browser.send_mouse_click(x, y, modifiers, cef_input::cef_mouse_button(Some(e.button)), false, clicks);
            }
            Event::MouseMove(e) => {
                let (x, y) = Self::point(e.abs);
                let _ = browser.send_mouse_move(x, y, cef_input::cef_modifiers(e.modifiers, self.pressed), false);
            }
            Event::MouseUp(e) => {
                if self.pressed.contains(e.button) {
                    let (x, y) = Self::point(e.abs);
                    let modifiers = cef_input::cef_modifiers(e.modifiers, self.pressed);
                    let _ = browser.send_mouse_move(x, y, modifiers, false);
                    let _ = browser.send_mouse_click(x, y, modifiers, cef_input::cef_mouse_button(Some(e.button)), true, self.last_press.2.max(1));
                }
                self.pressed.remove(e.button);
            }
            Event::MouseLeave(e) => {
                let (x, y) = Self::point(e.abs);
                let _ = browser.send_mouse_move(x, y, cef_input::cef_modifiers(e.modifiers, self.pressed), true);
            }
            Event::Scroll(e) => {
                let (dx, dy) = cef_input::cef_scroll_delta(e.scroll, &mut self.scroll_remainder);
                if dx != 0 || dy != 0 {
                    let (x, y) = Self::point(e.abs);
                    let modifiers = cef_input::cef_modifiers(e.modifiers, self.pressed) | cef_input::EVENTFLAG_PRECISION_SCROLLING_DELTA;
                    let _ = browser.send_mouse_wheel(x, y, modifiers, dx, dy);
                }
            }
            Event::KeyDown(key) => {
                if !cef_input::page_takes_key_down(key, &mut self.pasted_text) {
                    return;
                }
                let modifiers = cef_input::key_event_modifiers(key);
                let (windows_key_code, native_key_code) = (cef_input::windows_key_code(key.key_code), cef_input::native_key_code(key.key_code));
                let typing = !(key.modifiers.control || key.modifiers.alt || key.modifiers.logo);
                let character = if typing { cef_input::key_char(key.key_code, key.modifiers.shift).map_or(0, |ch| ch as u16) } else { 0 };
                // Off-screen pages take RAWKEYDOWN (KEYDOWN is Chromium's own follow-up).
                let _ = browser.send_key_event(cef_input::KEY_EVENT_RAWKEYDOWN, modifiers, windows_key_code, native_key_code, character, character, false);
                if character != 0 && typing && cef_input::sends_char_on_keydown(key.key_code) {
                    let _ = browser.send_key_event(cef_input::KEY_EVENT_CHAR, modifiers, windows_key_code, native_key_code, character, character, false);
                }
            }
            Event::KeyUp(key) => {
                let modifiers = cef_input::key_event_modifiers(key);
                let typing = !(key.modifiers.control || key.modifiers.alt || key.modifiers.logo);
                let character = if typing { cef_input::key_char(key.key_code, key.modifiers.shift).map_or(0, |ch| ch as u16) } else { 0 };
                let _ = browser.send_key_event(cef_input::KEY_EVENT_KEYUP, modifiers, cef_input::windows_key_code(key.key_code), cef_input::native_key_code(key.key_code), character, character, false);
            }
            Event::TextInput(text) => {
                if text.was_paste {
                    self.pasted_text = true;
                }
                match cef_input::char_event_data(&text.input) {
                    Some((windows_key_code, character)) if !text.was_paste && !text.replace_last => {
                        let modifiers = cef_input::cef_modifiers(cx.keyboard.modifiers(), MouseButton::empty());
                        let native_key_code = cef_input::native_key_code_for_char(char::from_u32(character as u32).unwrap_or('\0'));
                        let _ = browser.send_key_event(cef_input::KEY_EVENT_CHAR, modifiers, windows_key_code, native_key_code, character, character, false);
                    }
                    _ => {
                        let _ = browser.ime_commit_text(&text.input);
                    }
                }
            }
            Event::TextCopy(event) => Self::clipboard(browser, event, false),
            Event::TextCut(event) => Self::clipboard(browser, event, true),
            _ => {}
        }
    }
}

impl App {
    fn send(view: u64, event: ViewEvent) {
        Cx::send_studio_message(makepad_widgets::makepad_platform::studio::AppToStudio::Custom(protocol::event_to_json(view, &event)));
    }

    fn page_mut(&mut self, view: u64) -> Option<&mut Page> {
        self.pages.iter_mut().find(|page| page.view == Some(view))
    }

    fn op(&mut self, cx: &mut Cx, view: u64, op: ViewOp) {
        if let ViewOp::Open { url, dark, background } = op {
            makepad_cef::set_background_color(background);
            makepad_cef::set_application_dark_mode(dark);
            let at = match self.pages.iter().position(|page| page.view.is_none()) {
                Some(at) => at,
                None => {
                    self.pages.push(Page::new(cx));
                    self.pages.len() - 1
                }
            };
            let page = &mut self.pages[at];
            page.view = Some(view);
            page.shown = None;
            page.error = None;
            page.error_sent = false;
            page.dark = Some(dark);
            page.hidden = Some(false);
            page.navigate(&url);
            Self::send(view, ViewEvent::Window { window_id: page.window.window_id().id() });
            return;
        }
        if let ViewOp::Focus { on } = op {
            if on {
                self.focus = Some(view);
            } else if self.focus == Some(view) {
                self.focus = None;
            }
        }
        let Some(page) = self.page_mut(view) else { return };
        if let ViewOp::Navigate { url } = &op {
            page.navigate(url);
            return;
        }
        if let ViewOp::Close = op {
            // The window stays for the next page; the browser goes.
            page.browser = None;
            page.url = None;
            page.ring = None;
            page.audio = None;
            page.audio_backlog.clear();
            page.view = None;
            page.evals.clear();
            page.eval_ids.clear();
            page.pressed = MouseButton::empty();
            if self.focus == Some(view) {
                self.focus = None;
            }
            Self::send(view, ViewEvent::Closed);
            return;
        }
        match op {
            ViewOp::Hidden { on } => page.hidden = Some(on),
            ViewOp::Appearance { dark, background } => {
                makepad_cef::set_background_color(background);
                makepad_cef::set_application_dark_mode(dark);
                page.dark = Some(dark);
            }
            ViewOp::Eval { id, expression } => page.evals.push((id, expression)),
            ViewOp::AudioStop => {
                page.audio = None;
                page.audio_backlog.clear();
                if let Some(browser) = &mut page.browser {
                    browser.disable_audio_capture();
                }
            }
            op => {
                let Some(browser) = &mut page.browser else { return };
                let _ = match op {
                    ViewOp::Back => browser.go_back(),
                    ViewOp::Forward => browser.go_forward(),
                    ViewOp::Reload => browser.reload(),
                    ViewOp::Stop => browser.stop_load(),
                    ViewOp::Focus { on } => browser.set_focus(on),
                    ViewOp::MouseMove { x, y, modifiers, leave } => browser.send_mouse_move(x, y, modifiers, leave),
                    ViewOp::MouseClick { x, y, modifiers, button, up, clicks } => browser.send_mouse_click(x, y, modifiers, button, up, clicks),
                    ViewOp::MouseWheel { x, y, modifiers, dx, dy } => browser.send_mouse_wheel(x, y, modifiers, dx, dy),
                    ViewOp::Key { kind, modifiers, windows_key_code, native_key_code, character, unmodified_character, system } => {
                        browser.send_key_event(kind, modifiers, windows_key_code, native_key_code, character, unmodified_character, system)
                    }
                    ViewOp::Text { text } => browser.ime_commit_text(&text),
                    ViewOp::CaptureLost => browser.send_capture_lost_event(),
                    _ => Ok(()),
                };
            }
        }
    }

    /// The app's frame ring for page `view`: CEF copies its frames into it
    /// from now on (once the page exists).
    fn ring_setup(&mut self, setup: RingSetup) {
        let view = setup.ring;
        match ProducerRing::open(setup) {
            Ok(ring) => {
                if let Some(page) = self.page_mut(view) {
                    page.ring = Some(Arc::new(RingSink(ring)));
                }
            }
            Err(error) => eprintln!("browser host: page {view}: frame ring: {error}"),
        }
    }

    /// The app's audio ring for page `view`: its sound is captured into it.
    fn audio_setup(&mut self, setup: AudioRingSetup) {
        let view = setup.ring;
        match ProducerAudioRing::open(setup) {
            Ok(ring) => {
                if let Some(page) = self.page_mut(view) {
                    page.audio = Some(ring);
                    page.audio_start = true;
                    page.audio_backlog.clear();
                }
            }
            Err(error) => eprintln!("browser host: page {view}: audio ring: {error}"),
        }
    }

    /// Apply what waits for a page and report what changed.
    fn poll(&mut self) {
        makepad_cef::do_message_loop_work();
        for page in &mut self.pages {
            let Some(view) = page.view else { continue };
            if let Some(error) = page.browser.as_mut().and_then(Browser::take_error) {
                page.error = Some(error);
                page.browser = None;
            }
            if let Some(message) = page.error.clone() {
                if !std::mem::replace(&mut page.error_sent, true) {
                    Self::send(view, ViewEvent::Error { message });
                }
                continue;
            }
            let (dark, hidden) = (page.dark, page.hidden);
            let evals = std::mem::take(&mut page.evals);
            let ring = page.ring.clone();
            let audio_start = page.audio.is_some() && page.audio_start;
            let mut audio_events = Vec::new();
            let polled = page.browser.as_mut().map(|cef| {
                if let Some(ring) = ring {
                    cef.set_frame_sink(Some(ring));
                }
                if audio_start {
                    cef.enable_audio_capture(AUDIO_CONFIG);
                }
                while let Some(event) = cef.poll_audio() {
                    audio_events.push(match event {
                        AudioEvent::Started(format) => AudioRecord::Started {
                            epoch: format.epoch,
                            sample_rate: format.sample_rate,
                            channels: format.channels,
                            frames_per_buffer: format.frames_per_buffer,
                        },
                        AudioEvent::Packet(mut packet) => {
                            let record = AudioRecord::Packet(AudioRingPacket {
                                epoch: packet.epoch,
                                channels: packet.channels,
                                frames: packet.frames as u32,
                                pts_ms: packet.pts_ms,
                                unix_ms: packet.capture.callback_unix_ms,
                                elapsed_ns: packet.capture.callback_elapsed_ns,
                                content_epoch: packet.capture.navigation_epoch,
                                samples: packet.samples.clone(),
                            });
                            packet.samples.clear();
                            cef.recycle_audio_packet(packet);
                            record
                        }
                        AudioEvent::Stopped => AudioRecord::Stopped,
                        AudioEvent::Error(error) => AudioRecord::Error(error),
                    });
                }
                if let Some(dark) = dark {
                    let _ = cef.set_dark_mode(dark);
                }
                if let Some(hidden) = hidden {
                    let _ = cef.set_hidden(hidden);
                }
                let started: Vec<_> = evals.iter().filter_map(|(id, expression)| match cef.evaluate_javascript(expression) {
                    Ok(number) => Some(Ok((number, *id))),
                    Err(error) => Some(Err((*id, error.to_string()))),
                }).collect();
                let shown = Shown {
                    title: cef.title(),
                    url: cef.url(),
                    loading: cef.is_loading(),
                    can_go_back: cef.can_go_back(),
                    can_go_forward: cef.can_go_forward(),
                    epoch: cef.navigation_epoch(),
                };
                (started, shown, cef.take_popup_requests(), cef.take_console_messages(), cef.take_evaluations())
            });
            let Some((started, shown, popups, console, evaluations)) = polled else {
                // No page yet (it waits for its size): keep what waits.
                page.evals = evals;
                continue;
            };
            page.dark = None;
            page.hidden = None;
            page.ring = None;
            if audio_start {
                page.audio_start = false;
            }
            if let Some(audio) = &mut page.audio {
                page.audio_backlog.extend(audio_events);
                while let Some(record) = page.audio_backlog.front() {
                    if !audio.write(record) {
                        break;
                    }
                    page.audio_backlog.pop_front();
                }
            }
            for started in started {
                match started {
                    Ok(pair) => page.eval_ids.push(pair),
                    Err((id, value)) => Self::send(view, ViewEvent::Evaluated { id, ok: false, value }),
                }
            }
            if page.shown.as_ref() != Some(&shown) {
                Self::send(view, ViewEvent::State {
                    title: shown.title.clone(),
                    url: shown.url.clone(),
                    loading: shown.loading,
                    can_go_back: shown.can_go_back,
                    can_go_forward: shown.can_go_forward,
                    epoch: shown.epoch,
                });
                page.shown = Some(shown);
            }
            for url in popups {
                Self::send(view, ViewEvent::Popup { url });
            }
            for line in console {
                Self::send(view, ViewEvent::Console { level: line.level, message: line.message, source: line.source, line: line.line });
            }
            for evaluation in evaluations {
                let Some(at) = page.eval_ids.iter().position(|(number, _)| *number == evaluation.number) else { continue };
                let (_, id) = page.eval_ids.remove(at);
                let (ok, value) = match evaluation.result {
                    Ok(value) => (true, value),
                    Err(value) => (false, value),
                };
                Self::send(view, ViewEvent::Evaluated { id, ok, value });
            }
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            Event::Startup => {
                if !cx.in_makepad_studio() {
                    log!("browser host: runs only hosted (libs/browser_view starts it)");
                    cx.quit();
                    return;
                }
                self.poll = cx.start_interval(POLL);
            }
            Event::Custom(json) => {
                if let Some((view, op)) = protocol::parse_op(json) {
                    self.op(cx, view, op);
                } else if let Some(setup) = RingSetup::parse(json) {
                    self.ring_setup(setup);
                } else if let Some(setup) = AudioRingSetup::parse(json) {
                    self.audio_setup(setup);
                }
            }
            Event::WindowGeomChange(e) => {
                if let Some(page) = self.pages.iter_mut().find(|page| page.window.window_id().id() == e.window_id.id()) {
                    page.size = Some((e.new_geom.inner_size, e.new_geom.dpi_factor));
                    match (page.pixels(), &mut page.browser) {
                        (Some((width, height, dpi)), Some(browser)) => {
                            let _ = browser.resize(width, height, dpi);
                        }
                        _ => page.ensure_browser(),
                    }
                }
            }
            Event::Shutdown => {
                self.pages.clear();
                return;
            }
            _ => {}
        }
        if self.poll.is_event(event).is_some() {
            self.poll();
        }
        // Pointer input goes to its window's page, keys to the focused one.
        let window = match event {
            Event::MouseDown(e) => Some(e.window_id),
            Event::MouseMove(e) => Some(e.window_id),
            Event::MouseUp(e) => Some(e.window_id),
            Event::MouseLeave(e) => Some(e.window_id),
            Event::Scroll(e) => Some(e.window_id),
            _ => None,
        };
        let page = match window {
            Some(window) => self.pages.iter_mut().find(|page| page.view.is_some() && page.window.window_id().id() == window.id()),
            None if matches!(event, Event::KeyDown(_) | Event::KeyUp(_) | Event::TextInput(_) | Event::TextCopy(_) | Event::TextCut(_)) => {
                let focus = self.focus;
                self.pages.iter_mut().find(|page| page.view.is_some() && page.view == focus)
            }
            None => None,
        };
        if let Some(page) = page {
            if let Event::MouseDown(_) = event {
                self.focus = page.view;
            }
            page.input(cx, event);
        }
    }
}

fn main() {
    if let Some(code) = makepad_cef::run_helper_if_subprocess() {
        std::process::exit(code);
    }
    run_app(app_main);
}

/// CEF's process bootstrap, then the hosted makepad event loop.
pub fn app_main() {
    match makepad_cef::bootstrap() {
        Ok(BootstrapResult::Continue) => {}
        Ok(BootstrapResult::Exit(code)) => std::process::exit(code),
        Err(err) => panic!("CEF bootstrap failed: {err}"),
    }
    Cx::init_log();
    if Cx::pre_start() {
        return;
    }
    if let Err(err) = makepad_cef::prepare() {
        panic!("CEF prepare failed: {err}");
    }
    let cx = Rc::new(RefCell::new(Cx::new(makepad_widgets::_app_main_event_closure!(App))));
    let studio_http = makepad_widgets::resolve_studio_http();
    cx.borrow_mut().init_websockets(&studio_http);
    if makepad_widgets::should_run_stdin_loop_from_env() {
        cx.borrow_mut().in_makepad_studio = true;
    }
    cx.borrow_mut().init_cx_os();
    Cx::event_loop(cx.clone());
    drop(cx);
    makepad_cef::shutdown();
}
