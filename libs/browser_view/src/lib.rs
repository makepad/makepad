//! Web pages for makepad apps, with Chromium out of the app's process.
//!
//! [`BrowserViews`] is an app's set of pages. The first [`BrowserViews::open`]
//! starts the browser host (`makepad-app-browser-host`, a native makepad app
//! that owns CEF) as a hosted child through `makepad_app_host::Children`;
//! each view is one window of that one process, so each has its own shared
//! texture ring, and the last [`BrowserViews::close`] stops it. The owner
//! keeps its own chrome (tab strip, address bar): it draws a view's
//! [`BrowserViews::frame`], sizes it with [`BrowserViews::set_size`], hands
//! it the view's hits ([`BrowserViews::forward_hit`]) and drives it with the
//! `ViewOp`s.
//!
//! Frames: every view has a frame ring (`makepad_app_host::frame_ring`) of
//! shared textures the browser host copies Chromium's GPU frames into, one
//! GPU copy, no CPU copy. A view shows the newest (latest wins); in capture
//! mode ([`BrowserViews::set_capture`]) every frame also reaches the owner,
//! in order, stamped, with back-pressure instead of losses
//! ([`BrowserViews::take_captured`]), and its sound arrives through a
//! shared-memory audio ring on the same clock ([`BrowserViews::set_audio`],
//! [`BrowserViews::take_audio`]).
//!
//! Without a built browser host, `open` returns None and says so once in
//! the log: an app shows no page, no dialog.

pub mod cef_input;
pub mod protocol;

use std::collections::VecDeque;
use std::path::PathBuf;

use makepad_app_host::{forward_hit, ChildEvent, ChildSpec, Children, Forwarded, Home, HostAudioRing, HostRing, HostedApp, Life, RingSpec};
use makepad_error_log::log;
use makepad_platform::*;
use makepad_studio_protocol::AppToStudio;

pub use makepad_app_host::{AudioRecord, AudioRingPacket, RingFrameInfo, RingLease, RingShown};
pub use protocol::{ViewEvent, ViewOp};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ViewId(pub u64);

/// A console line of a page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsoleLine {
    pub level: i32,
    pub message: String,
    pub source: String,
    pub line: i32,
}

/// One page as its owner sees it.
#[derive(Clone, Debug, Default)]
pub struct ViewState {
    pub title: String,
    pub url: String,
    pub loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    /// The committed main-frame navigation.
    pub navigation_epoch: u64,
    /// The page failed (or the host went away); it shows nothing more.
    pub error: Option<String>,
    window: Option<usize>,
    popups: Vec<String>,
    console: Vec<ConsoleLine>,
    evaluations: Vec<(u64, Result<String, String>)>,
    /// Sent once the host is connected.
    queued: Vec<ViewOp>,
}

/// Slots of a displaying ring.
const DISPLAY_SLOTS: usize = 3;
/// Ring textures are allocated on this grid, so a resize walks many page
/// sizes inside one ring.
const RING_GRID: u32 = 256;
/// The audio ring: a little over a second of 48 kHz stereo.
const AUDIO_RING_BYTES: usize = 512 * 1024;

/// What a capturing owner asks for.
#[derive(Clone, Debug, PartialEq)]
pub struct CaptureSpec {
    /// Ring depth: frames the owner may hold before the page waits.
    pub slots: usize,
    /// Pixels read back with every frame (see `RingSpec::probes`).
    pub probes: Vec<[u32; 2]>,
}

struct View {
    id: ViewId,
    state: ViewState,
    /// The page's size in pixels, as last set.
    page_px: Option<(u32, u32)>,
    /// Its size in points and its dpi, as last set: given to its window
    /// once that exists.
    size: Option<(Vec2d, f64)>,
    geometry_dirty: bool,
    ring_dirty: bool,
    ring: Option<HostRing>,
    /// Replaced rings, kept while their frames are held or shown.
    retired: Vec<HostRing>,
    generation: u64,
    capture: Option<CaptureSpec>,
    captured: VecDeque<RingLease>,
    audio: Option<HostAudioRing>,
    audio_setup: Option<String>,
    audio_records: VecDeque<AudioRecord>,
}

impl View {
    fn new(id: ViewId, state: ViewState) -> View {
        View {
            id,
            state,
            page_px: None,
            size: None,
            geometry_dirty: false,
            ring_dirty: false,
            ring: None,
            retired: Vec::new(),
            generation: 0,
            capture: None,
            captured: VecDeque::new(),
            audio: None,
            audio_setup: None,
            audio_records: VecDeque::new(),
        }
    }

    /// A new ring when the page outgrew the current one or the capture
    /// asks for another depth or other probes.
    fn ensure_ring(&mut self, cx: &mut Cx) {
        let Some((width, height)) = self.page_px else { return };
        let up = |v: u32| v.max(1).div_ceil(RING_GRID) * RING_GRID;
        let (slots, probes, capture) = match &self.capture {
            Some(spec) => (spec.slots.max(2), spec.probes.clone(), true),
            None => (DISPLAY_SLOTS, Vec::new(), false),
        };
        if let Some(ring) = &mut self.ring {
            let spec = ring.spec();
            let fits = spec.width >= width && spec.height >= height;
            if fits && spec.slots == slots && spec.probes == probes {
                if spec.capture != capture {
                    ring.set_capture(capture);
                }
                return;
            }
        }
        self.generation += 1;
        let spec = RingSpec { slots, width: up(width), height: up(height), capture, probes };
        let ring = HostRing::new(cx, self.id.0, self.generation, spec);
        if let Some(mut old) = self.ring.replace(ring) {
            old.close();
            self.retired.push(old);
        }
    }
}

pub struct BrowserViews {
    children: Option<Children<(), ()>>,
    /// The profile the pages share: cookies, logins, storage.
    profile: Option<PathBuf>,
    views: Vec<View>,
    next_view: u64,
    next_eval: u64,
    /// Why there are no pages (said once).
    unavailable: Option<String>,
    cursor: MouseCursor,
    /// The browser host is to be started (at the next `service`).
    start_pending: bool,
    /// Pages closed since the last `service`, with their windows.
    closing: Vec<(ViewId, Option<usize>)>,
}

impl BrowserViews {
    /// `profile`: the name of the profile the app's pages keep (the app's
    /// binary name, so a person's logins survive moving the app). A hidden
    /// app (tests, agents) gets a throwaway profile.
    pub fn new(profile: &str) -> BrowserViews {
        let profile = if std::env::var_os("MAKEPAD_HIDE_WINDOWS").is_some() {
            None
        } else {
            home_dir().map(|home| home.join(".makepad-cef").join(profile))
        };
        BrowserViews { children: None, profile, views: Vec::new(), next_view: 1, next_eval: 1, unavailable: None, cursor: MouseCursor::Default, start_pending: false, closing: Vec::new() }
    }

    /// Whether pages can be shown: from a checkout the browser host's sources are there
    /// (it starts with `cargo run`, built from them when needed, like every app);
    /// installed, its binary is.
    pub fn availability() -> Result<(), String> {
        let entry = makepad_app_registry::find(protocol::HOST_APP).ok_or("no browser host in the app registry")?;
        if makepad_app_registry::is_available(entry) {
            Ok(())
        } else {
            Err(format!("the browser host is not installed ({})", entry.bin))
        }
    }

    /// Open a page (the browser host starts at the next `service`). None
    /// when there is no browser host (said once).
    pub fn open(&mut self, url: &str, dark: bool, background: u32) -> Option<ViewId> {
        if self.unavailable.is_some() {
            return None;
        }
        if self.children.as_ref().is_none_or(|children| children.slot(&()).is_none()) {
            if let Err(why) = Self::availability() {
                log!("browser view: no pages: {why}");
                self.unavailable = Some(why);
                return None;
            }
            self.start_pending = true;
        }
        let id = ViewId(self.next_view);
        self.next_view += 1;
        let mut state = ViewState { url: url.to_string(), loading: true, ..ViewState::default() };
        state.queued.push(ViewOp::Open { url: url.to_string(), dark, background });
        self.views.push(View::new(id, state));
        self.send_queued();
        Some(id)
    }

    fn start(&mut self, cx: &mut Cx) {
        // The browser host's log lives under the app's pid; an app that
        // ended before its reaper ran leaves its folder, removed here.
        let root = std::env::temp_dir().join("makepad-browser-view");
        if let Ok(entries) = std::fs::read_dir(&root) {
            for entry in entries.flatten() {
                let dead = entry.file_name().to_str().and_then(|name| name.parse::<u32>().ok()).is_some_and(|pid| !makepad_app_host::launch::process_alive(pid));
                if dead {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
        let scratch = root.join(std::process::id().to_string());
        let children = self.children.get_or_insert_with(|| Children::new(cx, "browser view:", scratch));
        let mut spec = ChildSpec::new(protocol::HOST_APP);
        spec.home = Home::Inherit;
        spec.multi_window = true;
        // Pages arrive through their frame rings; the windows carry input.
        spec.frames_external = true;
        // Pages play sound: the host app's own mute applies to its pages.
        spec.audio = std::env::var_os("SANDBOX_MUTE").is_none();
        if let Some(profile) = &self.profile {
            spec.env.push((protocol::PROFILE_ENV.to_string(), profile.display().to_string()));
        }
        children.prepare((), spec, "browser host".to_string(), ());
        children.launch(cx, &());
    }

    /// Close a page; the last one stops the browser host (at the next
    /// `service`).
    pub fn close(&mut self, id: ViewId) {
        let Some(at) = self.views.iter().position(|view| view.id == id) else { return };
        let view = self.views.remove(at);
        if self.views.is_empty() {
            self.start_pending = false;
        }
        self.closing.push((id, view.state.window));
    }

    fn close_pending(&mut self, cx: &mut Cx) {
        let closing = std::mem::take(&mut self.closing);
        let Some(children) = &mut self.children else { return };
        if self.views.is_empty() {
            if !closing.is_empty() {
                children.quit(cx, &());
            }
            return;
        }
        for (id, window) in closing {
            children.send_custom(&(), protocol::op_to_json(id.0, &ViewOp::Close));
            if let (Some(slot), Some(window)) = (children.slot_mut(&()), window) {
                slot.close_window(window);
            }
        }
        children.flush();
    }

    /// Close every page and stop the browser host.
    pub fn shutdown(&mut self, cx: &mut Cx) {
        self.views.clear();
        if let Some(children) = &mut self.children {
            children.shutdown(cx);
        }
    }

    /// Drive a page: navigation, focus, visibility, appearance.
    pub fn send(&mut self, id: ViewId, op: ViewOp) {
        let Some(view) = self.views.iter_mut().find(|view| view.id == id) else { return };
        if let ViewOp::Navigate { url } = &op {
            view.state.url = url.clone();
            view.state.title.clear();
            view.state.loading = true;
        }
        view.state.queued.push(op);
        self.send_queued();
    }

    /// Evaluate `expression` in the page; the answer comes from
    /// [`BrowserViews::take_evaluations`] under the returned number.
    pub fn evaluate(&mut self, id: ViewId, expression: &str) -> u64 {
        let number = self.next_eval;
        self.next_eval += 1;
        self.send(id, ViewOp::Eval { id: number, expression: expression.to_string() });
        number
    }

    pub fn ids(&self) -> impl Iterator<Item = ViewId> + '_ {
        self.views.iter().map(|view| view.id)
    }

    pub fn state(&self, id: ViewId) -> Option<&ViewState> {
        self.views.iter().find(|view| view.id == id).map(|view| &view.state)
    }

    pub fn take_popups(&mut self, id: ViewId) -> Vec<String> {
        self.view_mut(id).map(|view| std::mem::take(&mut view.state.popups)).unwrap_or_default()
    }

    pub fn take_console(&mut self, id: ViewId) -> Vec<ConsoleLine> {
        self.view_mut(id).map(|view| std::mem::take(&mut view.state.console)).unwrap_or_default()
    }

    pub fn take_evaluations(&mut self, id: ViewId) -> Vec<(u64, Result<String, String>)> {
        self.view_mut(id).map(|view| std::mem::take(&mut view.state.evaluations)).unwrap_or_default()
    }

    /// The cursor the pages asked for.
    pub fn cursor(&self) -> MouseCursor {
        self.cursor
    }

    fn view_mut(&mut self, id: ViewId) -> Option<&mut View> {
        self.views.iter_mut().find(|view| view.id == id)
    }

    /// The page's newest frame: the slot texture and the part of it the
    /// frame covers.
    pub fn frame(&self, id: ViewId) -> Option<RingShown> {
        let view = self.views.iter().find(|view| view.id == id)?;
        view.ring.as_ref().and_then(HostRing::shown).or_else(|| view.retired.iter().rev().find_map(HostRing::shown))
    }

    /// Capture page `id` (Some) or only show it (None): in capture mode
    /// every frame is kept for [`BrowserViews::take_captured`], in order.
    pub fn set_capture(&mut self, id: ViewId, capture: Option<CaptureSpec>) {
        let Some(view) = self.view_mut(id) else { return };
        if view.capture == capture {
            return;
        }
        view.capture = capture;
        if view.capture.is_none() {
            // What was captured and not taken is given back now.
            view.captured.clear();
        }
        view.ring_dirty = true;
    }

    /// The newest frame as a lease of its own (display mode too): what a
    /// consumer holds while it reads the frame on another thread.
    pub fn hold_frame(&self, id: ViewId) -> Option<RingLease> {
        let view = self.views.iter().find(|view| view.id == id)?;
        view.ring.as_ref()?.hold_shown()
    }

    /// The frames captured since the last call, oldest first. Each holds
    /// its slot until dropped (the page waits when all are held).
    pub fn take_captured(&mut self, id: ViewId) -> Vec<RingLease> {
        self.view_mut(id).map(|view| view.captured.drain(..).collect()).unwrap_or_default()
    }

    /// Capture the page's sound (48 kHz stereo, 480-frame packets) into its
    /// audio ring, or stop.
    pub fn set_audio(&mut self, id: ViewId, on: bool) {
        let Some(view) = self.view_mut(id) else { return };
        if on == view.audio.is_some() {
            return;
        }
        if on {
            match HostAudioRing::new(id.0, AUDIO_RING_BYTES) {
                Ok(ring) => {
                    view.audio_setup = Some(ring.setup().to_json());
                    view.audio = Some(ring);
                }
                Err(error) => log!("browser view {}: no audio ring: {error}", id.0),
            }
        } else {
            view.audio = None;
            view.audio_setup = None;
            view.state.queued.push(ViewOp::AudioStop);
        }
        self.send_queued();
    }

    /// The sound records (stream starts, packets, stops) since the last call.
    pub fn take_audio(&mut self, id: ViewId) -> Vec<AudioRecord> {
        self.view_mut(id).map(|view| view.audio_records.drain(..).collect()).unwrap_or_default()
    }

    /// The oldest sound record not taken yet.
    pub fn take_audio_one(&mut self, id: ViewId) -> Option<AudioRecord> {
        self.view_mut(id)?.audio_records.pop_front()
    }

    /// The hosted window showing page `id`.
    pub fn hosted_mut(&mut self, id: ViewId) -> Option<&mut HostedApp> {
        let window = self.views.iter().find(|view| view.id == id)?.state.window?;
        self.children.as_mut()?.slot_mut(&())?.window_mut(window)
    }

    /// The page is `size` points at `dpi`: its frames are that big (from
    /// the next `service`).
    pub fn set_size(&mut self, id: ViewId, size: Vec2d, dpi: f64) {
        let Some(view) = self.view_mut(id) else { return };
        if view.size == Some((size, dpi)) {
            return;
        }
        view.size = Some((size, dpi));
        view.geometry_dirty = true;
        let px = (((size.x * dpi).round().max(1.0)) as u32, ((size.y * dpi).round().max(1.0)) as u32);
        if view.page_px != Some(px) {
            view.page_px = Some(px);
            view.ring_dirty = true;
        }
    }

    /// What needs a `Cx`: start the browser host, size the pages' windows,
    /// make their rings, send setups, take frames and sound. `handle_event`
    /// does it; a draw that changed a size may call it at once.
    pub fn service(&mut self, cx: &mut Cx) -> bool {
        self.close_pending(cx);
        if std::mem::take(&mut self.start_pending) {
            self.start(cx);
        }
        for view in &mut self.views {
            if std::mem::take(&mut view.ring_dirty) {
                view.ensure_ring(cx);
            }
            if view.geometry_dirty {
                let window = view.state.window;
                let hosted = window.and_then(|window| self.children.as_mut()?.slot_mut(&())?.window_mut(window));
                if let (Some(hosted), Some((size, dpi))) = (hosted, view.size) {
                    hosted.set_geometry(cx, Rect { pos: Vec2d::default(), size }, dpi);
                    hosted.ensure_beat(cx);
                    view.geometry_dirty = false;
                }
            }
        }
        if !self.views.is_empty() {
            self.send_queued();
        }
        let changed = self.pump_rings(cx);
        self.flush();
        changed
    }

    /// Hand a hit of the view showing page `id` to it; `local` maps an
    /// absolute position to the page's points.
    pub fn forward_hit(&mut self, cx: &mut Cx, id: ViewId, hit: &Hit, local: impl Fn(Vec2d) -> Option<Vec2d>) -> Forwarded {
        let Some(hosted) = self.hosted_mut(id) else { return Forwarded::No };
        let forwarded = forward_hit(hosted, hit, local);
        hosted.ensure_beat(cx);
        if let Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) = hit {
            cx.set_cursor(self.cursor);
        }
        self.flush();
        forwarded
    }

    fn flush(&mut self) {
        if let Some(children) = &mut self.children {
            children.flush();
        }
    }

    /// Ops wait until the host's socket is up.
    fn send_queued(&mut self) {
        let Some(children) = &mut self.children else { return };
        if !children.slot(&()).is_some_and(|slot| slot.is_connected()) {
            return;
        }
        for view in &mut self.views {
            for op in view.state.queued.drain(..) {
                children.send_custom(&(), protocol::op_to_json(view.id.0, &op));
            }
            if let Some(setup) = view.audio_setup.take() {
                children.send_custom(&(), setup);
            }
        }
        children.flush();
    }

    /// Ring setups (once their textures are shared), new frames, sound.
    fn pump_rings(&mut self, cx: &mut Cx) -> bool {
        let connected = self.children.as_ref().is_some_and(|children| children.slot(&()).is_some_and(|slot| slot.is_connected()));
        let mut changed = false;
        for view in &mut self.views {
            if let Some(ring) = &mut view.ring {
                if connected {
                    if let Some(setup) = ring.take_setup(cx) {
                        if let Some(children) = &mut self.children {
                            children.send_custom(&(), setup);
                        }
                    }
                }
                let (leases, shown) = ring.poll();
                changed |= shown;
                if view.capture.is_some() {
                    view.captured.extend(leases);
                }
            }
            for old in &mut view.retired {
                let (leases, shown) = old.poll();
                changed |= shown;
                if view.capture.is_some() {
                    view.captured.extend(leases);
                }
            }
            // A replaced ring goes once nothing of it is held and the new
            // one shows a frame.
            let current_shows = view.ring.as_ref().is_some_and(|ring| ring.shown().is_some());
            view.retired.retain(|old| old.leases() > 0 || !current_shows);
            if let Some(audio) = &mut view.audio {
                view.audio_records.extend(audio.poll());
            }
        }
        if let Some(children) = &mut self.children {
            children.flush();
        }
        changed
    }

    /// Every event. True when a page changed (a frame, its state): the
    /// owner redraws.
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> bool {
        if let Event::GpuReset = event {
            // The rings' shared textures belonged to the replaced GPU device: each page gets a
            // ring of a new generation (made and set up at the next `service`), the producer
            // renders into that one.
            for view in &mut self.views {
                if let Some(mut old) = view.ring.take() {
                    old.close();
                }
                view.retired.clear();
                view.ring_dirty = true;
            }
        }
        let Some(children) = &mut self.children else { return self.service(cx) };
        children.handle_event(cx, event);
        let mut changed = false;
        for child_event in children.take_events() {
            match child_event {
                ChildEvent::Custom { json, .. } => {
                    if let Some((view, event)) = protocol::parse_event(&json) {
                        changed |= Self::apply(cx, &mut self.views, children, ViewId(view), event);
                    }
                }
                ChildEvent::Message { msg: AppToStudio::SetCursor(cursor), .. } => {
                    self.cursor = cursor.into();
                }
                ChildEvent::Message { msg: AppToStudio::SetClipboard(text), .. } => cx.copy_to_clipboard(&text),
                ChildEvent::Changed(()) => {
                    let life = children.slot(&()).map(|slot| slot.life.clone());
                    if let Some(Life::Missing(why) | Life::Exited(why)) = life {
                        log!("browser view: the browser host ended: {why}");
                        for view in &mut self.views {
                            view.state.error = Some(format!("the browser host ended: {why}"));
                            view.state.window = None;
                        }
                        changed = true;
                    }
                }
                _ => {}
            }
        }
        changed |= self.service(cx);
        changed
    }

    fn apply(cx: &mut Cx, views: &mut [View], children: &mut Children<(), ()>, id: ViewId, event: ViewEvent) -> bool {
        let Some(view) = views.iter_mut().find(|view| view.id == id) else {
            // A view closed meanwhile: its window goes with it.
            if let (ViewEvent::Window { window_id }, Some(slot)) = (&event, children.slot_mut(&())) {
                slot.close_window(*window_id);
            }
            return false;
        };
        let state = &mut view.state;
        match event {
            ViewEvent::Window { window_id } => {
                // A window the host reuses announces no new window of its own.
                if let Some(slot) = children.slot_mut(&()) {
                    if slot.life == Life::Live {
                        slot.open_window(cx, window_id);
                    }
                }
                state.window = Some(window_id);
                // A size set before the window existed applies now.
                view.geometry_dirty = true;
            }
            ViewEvent::State { title, url, loading, can_go_back, can_go_forward, epoch } => {
                state.navigation_epoch = epoch;
                state.title = title;
                state.url = url;
                state.loading = loading;
                state.can_go_back = can_go_back;
                state.can_go_forward = can_go_forward;
            }
            ViewEvent::Popup { url } => state.popups.push(url),
            ViewEvent::Console { level, message, source, line } => {
                state.console.push(ConsoleLine { level, message, source, line });
                return false;
            }
            ViewEvent::Evaluated { id, ok, value } => {
                state.evaluations.push((id, if ok { Ok(value) } else { Err(value) }));
                return false;
            }
            ViewEvent::Error { message } => {
                log!("browser view {}: {message}", id.0);
                state.error = Some(message);
            }
            ViewEvent::Closed => return false,
        }
        true
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).filter(|home| !home.is_empty()).map(PathBuf::from)
}
