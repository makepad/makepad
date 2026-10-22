//! Session 0's text/pass renderer. No platform client, draw tree, widgets or Cx.
mod activity;
mod caption;
mod document;
mod dump;
mod field;
mod gpu;
mod groups;
mod icon;
mod layer;
mod layout;
mod text;

use document::{Program, ERROR_BAND, FIELD, GLYPH, ICON, PIPE_BANK_SIZE};
pub use groups::order as order_diagnostics;
pub use icon::Icon;
pub use activity::Want as ActivityWant;
pub use layer::Want as LayerWant;
use makepad_hal::{
    backend_id, buffer_kind, clock_ns, dialect, event_tag, format, info_flags, input, lifecycle,
    pass_load, session::HOST, target, usage, window_kind, window_state, Attr, CmdWriter, EventBuf, EventQueue,
    FrameMeta, Hal, HalEvent, HalInfo, Id, Pass, RawEvent,
};
use makepad_script::{id, LiveId};
pub use relax_base::{
    diag::Diagnostic,
    run_state::{Mode, State},
};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

pub const DEFAULT_DOCUMENT: &str = include_str!("../resources/boot.splash");

/// Shell/HAL data, in the same logical points as Geom and RawEvent coordinates.
/// Android's dpi factor is densityDpi / 160 (420 dpi -> 2.625 pixels/dp).
/// Insets are left, top, right, bottom: the union of cutout, bars and keyboard.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Environment {
    pub insets: [f32; 4],
    pub dark: Option<bool>,
    pub touch: bool,
    pub reduce_motion: bool,
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            insets: [0.0; 4],
            dark: None,
            touch: cfg!(target_os = "android"),
            reduce_motion: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Restart,
    Copy,
    Open,
    Dismiss,
    Resume(usize),
    Submit,
    Cancel,
    Select(usize),
    /// The window's close button: the run ends, as when the window is asked to close.
    Close,
    /// The caption's minimise button (`Boot::window_control`).
    Minimize,
    /// The caption's maximise / restore button (`Boot::window_control`).
    Maximize,
}

#[derive(Clone)]
pub struct Document {
    /// where it came from (diagnostics name it)
    pub path: PathBuf,
    pub source: Arc<str>,
    /// a package's own document (`package.metadata.makepad.bootloader`): edits of the file are
    /// shown live. The bundled one belongs to the binary: it is never read from or watched in
    /// a source tree (a tree of another build may hold a document in another format).
    pub live: bool,
}
impl Document {
    pub fn bundled() -> Self {
        Self {
            path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/boot.splash"),
            source: DEFAULT_DOCUMENT.into(),
            live: false,
        }
    }
}

/// Input is interpreted by Relax itself, never by the document or renderer.
pub enum Event {
    KeyDown(RawEvent),
    Text(String),
    Input(RawEvent, Vec<u8>),
    DocumentErrors(Vec<Diagnostic>),
}

pub fn diagnostic_text(diagnostic: &Diagnostic) -> String {
    let mut text = if diagnostic.file.is_empty() {
        diagnostic.message.clone()
    } else {
        format!(
            "{}:{}:{} {}",
            diagnostic.file, diagnostic.line, diagnostic.col, diagnostic.message
        )
    };
    if let Some(item) = &diagnostic.item {
        text.push('\n');
        text.push_str(item);
    }
    for note in &diagnostic.notes {
        text.push_str("\nnote: ");
        text.push_str(note);
    }
    if let Some(help) = &diagnostic.suggestion {
        text.push_str("\nhelp: ");
        text.push_str(&help.message);
    }
    text
}

struct TextLine {
    run: Arc<text::Shaped>,
    position: [f32; 2],
    style: document::Line,
    fixed: bool,
}

struct Card {
    rect: [f32; 4],
    action: Action,
    /// the card's look (document `error_band.accent`): 0 follow-on, 1 root cause or panic,
    /// 2 a member row of a folded group, 3 a button, 4 a window button of the caption
    accent: f32,
    /// the list entries the card shows: it is drawn selected while one of them is
    members: std::ops::Range<usize>,
    fixed: bool,
}

#[derive(Default)]
struct Layout {
    panel: [f32; 4],
    /// the halted view's frame (the whole window, over the app too) and the region its galaxy
    /// is drawn in
    frame: [f32; 4],
    galaxy: [f32; 4],
    clip: [f32; 4],
    text: Vec<TextLine>,
    text_ends: Vec<f32>,
    fixed_text: Vec<TextLine>,
    cards: Vec<Card>,
    icon: Option<[f32; 4]>,
    icon_fixed: bool,
    content_height: f32,
}

struct Renderer {
    active: Option<Program>,
    pending: Option<Program>,
    generation: u32,
    text: text::Text,
    layout: Layout,
    dirty: bool,
    redraw: bool,
    began: Instant,
    input: Option<String>,
    selected: usize,
    reveal_selection: bool,
    scroll: f32,
    dialect: u32,
    bundled: bool,
    clear_errors_on_install: bool,
    icon_aspect: Option<f32>,
    pressed: Option<Action>,
    /// the caption's window button under the pointer
    hovered: Option<Action>,
    layout_revision: u64,
    presented_revision: u64,
    presented_scroll: f32,
    field: field::Field,
    layer: layer::Layer,
}

pub struct Boot {
    window: Option<Id>,
    geometry: [f32; 4],
    dpi: f32,
    frame: u64,
    queue: EventQueue,
    events: Vec<Event>,
    state: State,
    renderer: Option<Renderer>,
    overlay: Option<[f32; 4]>,
    environment: Environment,
    /// `measure_input`: how long each native input event of the window waited, from the window
    /// system's push to this session's answer (event tag, ns)
    latencies: Option<Vec<(u16, u64)>>,
    /// the last turn: ns its pump took beyond the asked timeout, ns its present took
    turn_parts: (u64, u64),
    dump: Option<dump::Dump>,
    /// the app has the window: the field is drawn offscreen (a dump with `MAKEPAD_RUN_BOOT_HOLD`)
    offscreen: bool,
    /// `MAKEPAD_RUN_BOOT_STATS=1`: the HAL's GPU time of the boot session's frames, printed as it goes
    gpu: bool,
    /// the backend composes a window layer over a lent window (`info_flags::WINDOW_LAYERS`)
    layers: bool,
    /// the window system composites badges over the window (`info_flags::WINDOW_BADGES`)
    badges: bool,
    /// the JIT activity badge
    activity: activity::Activity,
    /// the window's chrome as the platform reports it: the caption strip follows it
    chrome: caption::Chrome,
}

impl Boot {
    pub fn new(
        info: HalInfo,
        document: Document,
        state: State,
        geometry: [f32; 4],
    ) -> Result<Self, String> {
        let mut boot = Self {
            window: None,
            geometry,
            dpi: if info.dpi.is_finite() && info.dpi > 0.0 {
                info.dpi
            } else {
                1.0
            },
            frame: 0,
            queue: EventQueue::new(),
            events: Vec::new(),
            state,
            renderer: None,
            overlay: None,
            environment: Environment::default(),
            latencies: None,
            turn_parts: (0, 0),
            dump: dump::Dump::from_env(),
            offscreen: false,
            gpu: std::env::var_os("MAKEPAD_RUN_BOOT_STATS").is_some(),
            layers: info.flags & info_flags::WINDOW_LAYERS != 0,
            badges: info.flags & info_flags::WINDOW_BADGES != 0,
            activity: activity::Activity::new(),
            chrome: caption::Chrome {
                client: info.flags & info_flags::CLIENT_CHROME != 0,
                ..Default::default()
            },
        };
        if let Some(chrome) = boot.dump.as_ref().and_then(|d| d.chrome) {
            boot.chrome = chrome;
        }
        // A backend without a shader dialect has no native HAL window to draw into.
        // In particular Linux/Windows do not manufacture a clear-only fake window.
        if info.backend == backend_id::HEADLESS || info.dialect == dialect::NONE {
            return Ok(boot);
        }
        let (program, bundled, clear_errors_on_install) =
            match Program::compile(&document, info.dialect, 0, 1) {
                Ok(program) => (program, document.source.as_ref() == DEFAULT_DOCUMENT, true),
                Err(errors) => {
                    boot.events.push(Event::DocumentErrors(errors));
                    (
                        Program::compile(&Document::bundled(), info.dialect, 0, 1).map_err(
                            |errors| {
                                errors
                                    .iter()
                                    .map(diagnostic_text)
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            },
                        )?,
                        true,
                        false,
                    )
                }
            };
        let hal = Hal::global();
        let window = hal.with_world(|world| world.host_window(geometry));
        let mut writer = CmdWriter::new(HOST);
        writer.window_create(window, window_kind::NORMAL, Id::NONE, geometry, false);
        writer.str_def(1, boot.state.app.as_bytes());
        writer.window_title(window, 1);
        let vertices: Vec<u8> = [0.0_f32, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let indices: Vec<u8> = [0_u32, 1, 2, 0, 2, 3]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        writer.buffer_create(
            gpu::VERTICES,
            buffer_kind::VERTEX,
            vertices.len() as u64,
            Attr::default(),
        );
        writer.buffer_update(gpu::VERTICES, 0, &vertices);
        writer.buffer_create(
            gpu::INDICES,
            buffer_kind::INDEX,
            indices.len() as u64,
            Attr::default(),
        );
        writer.buffer_update(gpu::INDICES, 0, &indices);
        let text = text::Text::new(&mut writer)?;
        program.create(&mut writer, info.dialect);
        hal.submit(&writer.buf());
        boot.window = Some(window);
        boot.renderer = Some(Renderer {
            active: None,
            pending: Some(program),
            generation: 1,
            text,
            layout: Layout::default(),
            dirty: true,
            redraw: true,
            began: Instant::now(),
            input: None,
            selected: 0,
            reveal_selection: false,
            scroll: 0.0,
            dialect: info.dialect,
            bundled,
            clear_errors_on_install,
            icon_aspect: None,
            pressed: None,
            hovered: None,
            layout_revision: 0,
            presented_revision: 0,
            presented_scroll: 0.0,
            field: field::Field::new(),
            layer: layer::Layer::new(),
        });
        Ok(boot)
    }

    pub fn window(&self) -> Option<Id> {
        self.window
    }
    pub fn events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    pub fn environment(&mut self, mut environment: Environment) {
        environment.insets = environment
            .insets
            .map(|v| if v.is_finite() { v.max(0.0) } else { 0.0 });
        if self.environment != environment {
            self.environment = environment;
            if let Some(renderer) = &mut self.renderer {
                renderer.dirty = true;
            }
        }
    }

    pub fn icon(&mut self, icon: Icon) {
        if let Some(renderer) = &mut self.renderer {
            let mut writer = CmdWriter::new(HOST);
            icon.upload(&mut writer);
            renderer.icon_aspect = Some(icon.aspect());
            renderer.dirty = true;
            Hal::global().submit(&writer.buf());
        }
    }

    pub fn layout_revision(&self) -> u64 {
        self.renderer.as_ref().map_or(0, |r| r.layout_revision)
    }

    pub fn hit_test(&self, point: [f32; 2]) -> Option<Action> {
        let renderer = self.renderer.as_ref()?;
        if renderer.dirty
            || renderer.layout_revision != renderer.presented_revision
            || renderer.scroll != renderer.presented_scroll
        {
            return None;
        }
        renderer
            .layout
            .visible_cards(renderer.scroll)
            .rev()
            .find_map(|card| {
                let mut rect = card.rect;
                if !card.fixed {
                    if !contains(renderer.layout.clip, point) {
                        return None;
                    }
                    rect[1] -= renderer.scroll;
                }
                contains(rect, point).then_some(card.action)
            })
    }

    pub fn scrollable_at(&self, point: [f32; 2]) -> bool {
        self.renderer
            .as_ref()
            .is_some_and(|r| contains(r.layout.clip, point))
    }

    /// What a press at `point` starts, the window system's caption question (`DragQuery`): the
    /// window's move where the boot screen does not take the press itself -- off every card,
    /// and off the scroll area while its content overflows (a press there scrolls). The window
    /// has no system caption bar, so this is the only way it moves.
    pub fn drag_answer(&self, point: [f32; 2]) -> u32 {
        let Some(renderer) = &self.renderer else {
            return input::drag_query::CAPTION;
        };
        let layout = &renderer.layout;
        let on_card = layout.visible_cards(renderer.scroll).any(|card| {
            let mut rect = card.rect;
            if !card.fixed {
                if !contains(layout.clip, point) {
                    return false;
                }
                rect[1] -= renderer.scroll;
            }
            contains(rect, point)
        });
        let scrolls = layout.content_height > layout.clip[3] && contains(layout.clip, point);
        if on_card || scrolls {
            input::drag_query::CLIENT
        } else {
            input::drag_query::CAPTION
        }
    }

    /// Records from now on how long each native input event waited for this session
    /// (`take_latencies`): the run's boot probe.
    pub fn measure_input(&mut self) {
        self.latencies.get_or_insert_with(Vec::new);
    }

    pub fn take_latencies(&mut self) -> Vec<(u16, u64)> {
        self.latencies.as_mut().map(std::mem::take).unwrap_or_default()
    }

    pub fn press(&mut self, action: Option<Action>) {
        if let Some(renderer) = &mut self.renderer {
            if renderer.pressed != action {
                renderer.pressed = action;
                renderer.redraw = true;
            }
        }
    }

    /// The pointer is at `point` (`None`: it left the window): the caption's window button under
    /// it shows its hover.
    pub fn hover(&mut self, point: Option<[f32; 2]>) {
        let caption = self.caption();
        if let Some(renderer) = &mut self.renderer {
            let hovered = point.and_then(|p| caption.button_at(p));
            if renderer.hovered != hovered {
                renderer.hovered = hovered;
                renderer.redraw = true;
            }
        }
    }

    /// The caption's minimise and maximise / restore buttons: the window's state changes.
    pub fn window_control(&mut self, action: Action) {
        let Some(window) = self.window else { return };
        let state = match action {
            Action::Minimize => window_state::MINIMIZE,
            Action::Maximize if self.chrome.state == window_state::MAXIMIZE => window_state::RESTORE,
            Action::Maximize => window_state::MAXIMIZE,
            _ => return,
        };
        let mut writer = CmdWriter::new(HOST);
        writer.window_state(window, state);
        Hal::global().submit(&writer.buf());
    }

    /// The caption strip the window has now.
    fn caption(&self) -> caption::Caption {
        self.chrome.caption(self.geometry[2], self.environment.touch)
    }

    pub fn reload(&mut self, document: Document) {
        self.replace_document(document, true);
    }

    fn replace_document(&mut self, document: Document, clear_errors: bool) {
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        renderer.generation += 1;
        let bank = renderer.active.as_ref().map_or(0, |p| {
            if p.pipes[0].id.index == 0 {
                PIPE_BANK_SIZE
            } else {
                0
            }
        });
        match Program::compile(&document, renderer.dialect, bank, renderer.generation) {
            Ok(program) => {
                let mut writer = CmdWriter::new(HOST);
                if let Some(old) = renderer.pending.take() {
                    old.destroy(&mut writer);
                }
                renderer.bundled = document.source.as_ref() == DEFAULT_DOCUMENT;
                program.create(&mut writer, renderer.dialect);
                renderer.pending = Some(program);
                renderer.clear_errors_on_install = clear_errors;
                Hal::global().submit(&writer.buf());
            }
            Err(errors) => {
                // A newer invalid edit supersedes any earlier pending reload.
                // At startup, keep the only renderable document as the fallback.
                if renderer.active.is_some() {
                    if let Some(old) = renderer.pending.take() {
                        let mut writer = CmdWriter::new(HOST);
                        old.destroy(&mut writer);
                        Hal::global().submit(&writer.buf());
                    }
                }
                renderer.clear_errors_on_install = false;
                self.events.push(Event::DocumentErrors(errors));
            }
        }
    }

    pub fn update(&mut self, state: State) {
        if state.mode != Mode::Overlay {
            self.set_overlay(None);
        }
        if let Some(renderer) = &mut self.renderer {
            if state.elapsed < self.state.elapsed {
                renderer.began = Instant::now();
                renderer.scroll = 0.0;
                renderer.field.reset();
            }
            renderer.dirty |= state.mode != self.state.mode
                || state.app != self.state.app
                || state.phase != self.state.phase
                || state.done != self.state.done
                || state.total != self.state.total
                || state.current != self.state.current
                || state.warm != self.state.warm
                || state.domain != self.state.domain
                || state.parked != self.state.parked
                || state.can_retry != self.state.can_retry
                || state.can_restart != self.state.can_restart
                || !Arc::ptr_eq(&state.diagnostics, &self.state.diagnostics);
        }
        self.state = state;
    }

    pub fn choose(&mut self, diagnostic: usize) {
        if let Some(renderer) = &mut self.renderer {
            // the selection changes the text (a folded group shows its selected member)
            renderer.dirty |= renderer.selected != diagnostic;
            renderer.selected = diagnostic;
            renderer.redraw = true;
        }
    }

    pub fn select(&mut self, diagnostic: usize) {
        self.choose(diagnostic);
        if let Some(renderer) = &mut self.renderer {
            // a selected member of a folded group may not have a row yet: lay out again
            renderer.dirty = true;
            renderer.reveal_selection = true;
        }
    }

    pub fn scroll(&mut self, pages: f32) {
        let height = self.renderer.as_ref().map_or(0.0, |r| r.layout.clip[3]);
        self.scroll_points(pages * height);
    }

    pub fn scroll_points(&mut self, points: f32) {
        if let Some(renderer) = &mut self.renderer {
            let max = (renderer.layout.content_height - renderer.layout.clip[3]).max(0.0);
            renderer.scroll = (renderer.scroll + points).clamp(0.0, max);
            renderer.redraw = true;
        }
    }

    pub fn input(&mut self, input: Option<String>) {
        if let Some(renderer) = &mut self.renderer {
            if renderer.input.is_some() != input.is_some() {
                if let Some(window) = self.window {
                    let mut writer = CmdWriter::new(HOST);
                    if input.is_some() {
                        writer.ime_show(window, renderer.layout.panel, 0);
                    } else {
                        writer.ime_hide(window);
                    }
                    Hal::global().submit(&writer.buf());
                }
            }
            renderer.input = input;
            renderer.dirty = true;
        }
    }

    fn set_overlay(&mut self, rect: Option<[f32; 4]>) {
        if self.overlay == rect {
            return;
        }
        self.overlay = rect;
        if rect.is_some() {
            // a panel replaces the window layer (the HAL keeps one overlay per window)
            if let Some(renderer) = &mut self.renderer {
                if renderer.layer.on {
                    renderer.layer.on = false;
                    renderer.layer.set(None);
                }
            }
        }
        if let Some(window) = self.window {
            let mut writer = CmdWriter::new(HOST);
            writer.window_overlay(window, rect.is_some(), rect.unwrap_or_default(), Id::NONE);
            if rect.is_some()
                && self
                    .renderer
                    .as_ref()
                    .is_some_and(|renderer| renderer.input.is_none())
            {
                writer.ime_hide(window);
            }
            Hal::global().submit(&writer.buf());
        }
    }

    pub fn turn(&mut self, timeout_ns: u64, present: bool) -> bool {
        if let Some(dark) = makepad_hal::host_dark() {
            let mut environment = self.environment;
            environment.dark = Some(dark);
            self.environment(environment);
        }
        let hal = Hal::global();
        // Pump backend completions, then take session 0's owned queue. A zero-cap
        // pump leaves the events queued. Taking the arena avoids truncating large
        // diagnostics/pastes at an arbitrary EventStore capacity.
        let pumped = Instant::now();
        hal.pump(
            timeout_ns,
            &mut EventBuf {
                session: HOST,
                events: std::ptr::null_mut(),
                cap: 0,
                arena: std::ptr::null_mut(),
                arena_cap: 0,
                arena_len: 0,
            },
        );
        let over = (pumped.elapsed().as_nanos() as u64).saturating_sub(timeout_ns);
        self.turn_parts = (over, 0);
        let mut queue = std::mem::take(&mut self.queue);
        let live = hal.with_world(|world| {
            let Some(session) = world.session_mut(HOST) else {
                return false;
            };
            std::mem::swap(&mut queue, &mut session.events);
            true
        });
        if !live {
            eprintln!("makepad-run: the boot window ends: its HAL session is gone");
            return false;
        }
        let mut alive = true;
        let mut drag_queries = Vec::new();
        let window = self.window;
        let mut latencies = self.latencies.take();
        queue.drain_with(|mut raw, payload| {
            raw.data.off = 0;
            let Some(event) = raw.decode(payload) else {
                return;
            };
            // a drag query counts when it is answered (below)
            if let Some(latencies) = latencies
                .as_mut()
                .filter(|_| Some(raw.window) == window && raw.tag != event_tag::DragQuery)
            {
                latencies.push((raw.tag, clock_ns().saturating_sub(raw.time_ns)));
            }
            if let Some(renderer) = &mut self.renderer {
                match renderer.text.event(&event) {
                    Ok(changed) => renderer.redraw |= changed,
                    Err(error) => {
                        eprintln!("makepad-run: {error}");
                        eprintln!("makepad-run: the boot window ends: its text renderer failed");
                        alive = false;
                    }
                }
            }
            match event {
                HalEvent::MouseDown { window, .. }
                | HalEvent::MouseMove { window, .. }
                | HalEvent::MouseUp { window, .. }
                | HalEvent::MouseLeave { window, .. }
                | HalEvent::Scroll { window, .. }
                | HalEvent::Touch { window, .. }
                | HalEvent::Focus { window, .. }
                    if Some(window) == self.window =>
                {
                    self.events.push(Event::Input(raw, payload.to_vec()));
                }
                HalEvent::KeyDown { window, .. } if Some(window) == self.window => {
                    self.events.push(Event::KeyDown(raw))
                }
                HalEvent::DragQuery { window, x, y } if Some(window) == self.window => {
                    drag_queries.push((window, [x as f32, y as f32], raw.time_ns))
                }
                HalEvent::Text { window, text, .. } if Some(window) == self.window => self
                    .events
                    .push(Event::Text(String::from_utf8_lossy(text).into_owned())),
                HalEvent::Ime {
                    window,
                    kind: makepad_hal::input::ime::COMMIT,
                    text,
                    ..
                } if Some(window) == self.window => self
                    .events
                    .push(Event::Text(String::from_utf8_lossy(text).into_owned())),
                HalEvent::Geom {
                    window,
                    x,
                    y,
                    w,
                    h,
                    dpi,
                    state,
                } if Some(window) == self.window => {
                    self.geometry = [x as f32, y as f32, w as f32, h as f32];
                    self.chrome.state = state;
                    if dpi.is_finite() && dpi > 0.0 {
                        self.dpi = dpi;
                    }
                    if let Some(renderer) = &mut self.renderer {
                        renderer.dirty = true;
                    }
                }
                HalEvent::Chrome { window, x, y, w, h, .. } if Some(window) == self.window => {
                    let native = [x as f32, y as f32, w as f32, h as f32];
                    let native = if native.iter().all(|v| v.is_finite()) { native } else { [0.0; 4] };
                    let fixed = self.dump.as_ref().is_some_and(|d| d.chrome.is_some());
                    if self.chrome.native != native && !fixed {
                        if self.dump.is_some() {
                            eprintln!("makepad-run: boot window chrome: native window buttons at {native:?}");
                        }
                        self.chrome.native = native;
                        if let Some(renderer) = &mut self.renderer {
                            renderer.dirty = true;
                        }
                    }
                }
                HalEvent::Display { window, dpi, .. } if Some(window) == self.window => {
                    if dpi.is_finite() && dpi > 0.0 {
                        self.dpi = dpi;
                    }
                    if let Some(renderer) = &mut self.renderer {
                        renderer.dirty = true;
                    }
                }
                HalEvent::PipelineReady { pipeline } => {
                    if let Some(renderer) = &mut self.renderer {
                        if let Some(program) = &mut renderer.pending {
                            for pipe in &mut program.pipes {
                                if pipe.id == pipeline {
                                    pipe.ready = true;
                                }
                            }
                        }
                    }
                }
                HalEvent::PipelineFailed { pipeline, log } => {
                    let Some(renderer) = &mut self.renderer else {
                        return;
                    };
                    if renderer
                        .pending
                        .as_ref()
                        .is_some_and(|p| p.pipes.iter().any(|p| p.id == pipeline))
                    {
                        let program = renderer.pending.take().unwrap();
                        let mut writer = CmdWriter::new(HOST);
                        program.destroy(&mut writer);
                        hal.submit(&writer.buf());
                        self.events.push(Event::DocumentErrors(vec![Diagnostic::new(
                            "codegen",
                            program.path.to_string_lossy(),
                            1,
                            1,
                            String::from_utf8_lossy(log),
                        )]));
                        if renderer.active.is_none() {
                            if renderer.bundled {
                                eprintln!("makepad-run: the boot window ends: its own document's pipeline failed ({})", String::from_utf8_lossy(log));
                                alive = false;
                            } else {
                                self.replace_document(Document::bundled(), false);
                            }
                        }
                    }
                }
                HalEvent::Readback { req, rect, pixels, .. } => {
                    if let Some(dump) = &mut self.dump {
                        dump.ready(req, rect, pixels);
                    }
                }
                HalEvent::CloseRequested { window } if Some(window) == self.window => {
                    eprintln!("makepad-run: the boot window ends: the window was asked to close");
                    alive = false
                }
                HalEvent::Closed { window } if Some(window) == self.window => {
                    eprintln!("makepad-run: the boot window ends: the window was closed");
                    alive = false
                }
                HalEvent::Lifecycle {
                    kind: lifecycle::QUIT_REQUESTED,
                } => {
                    eprintln!("makepad-run: the boot window ends: the system asked the app to quit");
                    alive = false
                }
                HalEvent::DeviceLost { .. } => {
                    eprintln!("makepad-run: the boot window ends: the graphics device was lost");
                    alive = false
                }
                _ => {}
            }
        });
        self.queue = queue;
        self.latencies = latencies;
        if !drag_queries.is_empty() {
            // answered at once: the window system starts the move only while the button is down
            let mut writer = CmdWriter::new(HOST);
            for &(window, point, _) in &drag_queries {
                writer.drag_reply(window, input::reply::QUERY, self.drag_answer(point));
            }
            hal.submit(&writer.buf());
            if let Some(latencies) = &mut self.latencies {
                let now = clock_ns();
                for (_, _, time) in drag_queries {
                    latencies.push((event_tag::DragQuery, now.saturating_sub(time)));
                }
            }
        }
        if !alive {
            return false;
        }
        if let Some(renderer) = &mut self.renderer {
            if renderer.pending.as_ref().is_some_and(Program::ready) {
                let mut writer = CmdWriter::new(HOST);
                if let Some(old) = renderer.active.take() {
                    old.destroy(&mut writer);
                }
                renderer.active = renderer.pending.take();
                renderer.dirty = true;
                hal.submit(&writer.buf());
                if renderer.clear_errors_on_install {
                    self.events.push(Event::DocumentErrors(Vec::new()));
                }
            }
        }
        if present {
            let presented = Instant::now();
            self.present();
            self.turn_parts.1 = presented.elapsed().as_nanos() as u64;
        }
        true
    }

    /// The last turn: how long its pump took beyond the asked timeout and its present took (ns).
    pub fn turn_parts(&self) -> (u64, u64) {
        self.turn_parts
    }

    fn present(&mut self) {
        if !self.geometry[2..].iter().all(|v| v.is_finite() && *v > 0.0) {
            return;
        }
        let Some(window) = self.window else { return };
        if self.offscreen && !self.field_mode() {
            return;
        }
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        if renderer.active.is_none() {
            return;
        }
        if renderer.layer.on {
            self.present_field();
            return;
        }
        if self.state.mode == Mode::Boot
            && self.state.diagnostics.is_empty()
            && self.state.parked.is_none()
        {
            self.present_field();
            return;
        }
        // the halted view: drawn when something changed, never animated (the field is frozen)
        if !renderer.redraw && !renderer.dirty {
            return;
        }
        let caption = self.chrome.caption(self.geometry[2], self.environment.touch);
        if renderer.dirty {
            renderer.layout(
                &self.state,
                [self.geometry[2], self.geometry[3]],
                self.environment,
                &caption,
            );
            renderer.dirty = false;
        }
        let rect = (self.state.mode == Mode::Overlay).then_some(renderer.layout.panel);
        self.set_overlay(rect);
        let renderer = self.renderer.as_mut().unwrap();
        let program = renderer.active.as_ref().unwrap();
        let build_start_ns = makepad_hal::clock_ns();
        let mut writer = CmdWriter::new(HOST);
        let viewport = [self.geometry[2], self.geometry[3]];
        let dpi = self.dpi;
        let time = renderer.began.elapsed().as_secs_f32();
        let field = renderer.field.frame(
            &mut writer,
            &program.pipes[FIELD],
            &self.state,
            time,
            renderer.layout.galaxy,
            self.environment.reduce_motion,
            true,
            -1.0,
            false,
        );
        let max_scroll = (renderer.layout.content_height - renderer.layout.clip[3]).max(0.0);
        renderer.scroll = renderer.scroll.min(max_scroll);
        let scroll = renderer.scroll;
        let clip = renderer.layout.clip;
        let frame = renderer.layout.frame;
        let mut data = vec![Vec::new(); program.pipes.len()];
        if let Some(mut rect) = renderer.layout.icon.filter(|_| program.icon) {
            if !renderer.layout.icon_fixed {
                rect[1] -= scroll;
            }
            let bounds = if renderer.layout.icon_fixed { frame } else { clip };
            program.pipes[ICON].instances.write(
                &[(id!(rect), &rect), (id!(clip_rect), &bounds)],
                &mut data[ICON],
            );
        }
        for card in renderer.layout.visible_cards(scroll) {
            let mut rect = card.rect;
            let bounds = if card.fixed { frame } else { clip };
            if !card.fixed {
                rect[1] -= scroll;
            }
            let selected = (!card.fixed && card.members.contains(&renderer.selected))
                || (card.accent == CAPTION_BUTTON && renderer.hovered == Some(card.action));
            program.pipes[ERROR_BAND].instances.write(
                &[
                    (id!(rect), &rect),
                    (id!(clip_rect), &bounds),
                    (id!(selected), &[selected as u8 as f32]),
                    (id!(accent), &[card.accent]),
                    (id!(action), &[button_glyph(card.action)]),
                    (
                        id!(pressed),
                        &[(renderer.pressed == Some(card.action)) as u8 as f32],
                    ),
                ],
                &mut data[ERROR_BAND],
            );
        }
        renderer.text.begin_frame();
        let mut requests = Vec::new();
        // Only visible rows request glyphs. The content is ordered vertically.
        let content = &renderer.layout.text;
        let start = renderer
            .layout
            .text_ends
            .partition_point(|end| *end < clip[1] + scroll);
        let visible = content[start..]
            .iter()
            .take_while(|line| line.position[1] <= clip[1] + clip[3] + scroll);
        let mut glyphs = Vec::new();
        for line in visible.chain(renderer.layout.fixed_text.iter()) {
            let y = line.position[1] - if line.fixed { 0.0 } else { scroll };
            let bounds = if line.fixed { frame } else { clip };
            renderer
                .text
                .glyph_instances(program, line, y, bounds, dpi, &mut requests, &mut glyphs);
        }
        data[GLYPH] = glyphs;
        let ready = renderer.text.finish_frame(&mut writer, &requests);
        if !ready {
            // Submit glyph requests without a FramePacket; retain the last image
            // and retry on later turns while input and completions keep flowing.
            Hal::global().submit(&writer.buf());
            return;
        }
        let sdf = makepad_font_core::raster::Settings::default().sdfer;
        let field_frame = frame;
        let fault = [1.0, 0.0, caption.height, 0.0];
        let mut uniforms = vec![Vec::new(); program.pipes.len()];
        for (i, pipe) in program.pipes.iter().enumerate() {
            pipe.uniforms.write(
                &[
                    (id!(viewport), &viewport),
                    (id!(dpi), &[dpi]),
                    (
                        id!(reduce_motion),
                        &[self.environment.reduce_motion as u8 as f32],
                    ),
                    (id!(time), &[time]),
                    (id!(view), &field.view),
                    (id!(field_params), &field.params),
                    (id!(field_fall), &field.fall),
                    (id!(field_frame), &field_frame),
                    (id!(field_fault), &fault),
                    (id!(radius), &[sdf.radius]),
                    (id!(cutoff), &[sdf.cutoff]),
                ],
                &mut uniforms[i],
            );
        }
        self.frame += 1;
        writer.frame_begin(
            self.frame,
            FrameMeta {
                build_start_ns,
                ..Default::default()
            },
        );
        writer.pass(Pass {
            load: [pass_load::CLEAR; 5],
            clear: [[0.0; 4]; 4],
            viewport: [0.0, 0.0, viewport[0] * dpi, viewport[1] * dpi],
            dpi,
            ..Default::default()
        });
        let scissor = frame.map(|v| v * dpi);
        program.pipes[FIELD].draw_buffer(
            &mut writer,
            field::INSTANCES,
            field.count,
            &uniforms[FIELD],
            scissor,
        );
        for i in [ERROR_BAND, ICON, GLYPH] {
            if i == ICON && !program.icon {
                continue;
            }
            program.pipes[i].draw(
                &mut writer,
                &data[i],
                &uniforms[i],
                if i == ICON {
                    icon::TEXTURE
                } else {
                    renderer.text.atlas()
                },
                scissor,
            );
        }
        writer.frame_set_build_end(makepad_hal::clock_ns());
        let snapshot = self.dump.as_mut().and_then(|d| d.now());
        writer.frame_end(
            window,
            target::WINDOW | if snapshot.is_some() { target::SNAPSHOT } else { 0 },
        );
        if let Some(req) = snapshot {
            writer.snapshot(window, req);
        }
        Hal::global().submit(&writer.buf());
        renderer.presented_revision = renderer.layout_revision;
        renderer.presented_scroll = renderer.scroll;
        renderer.redraw = false;
    }
}

impl Boot {
    /// Whether the boot window shows the Ignition Field (no errors or parked panic to list).
    pub fn field_mode(&self) -> bool {
        self.state.mode == Mode::Boot
            && self.state.diagnostics.is_empty()
            && self.state.parked.is_none()
            && self.renderer.as_ref().is_some_and(|r| r.active.is_some())
    }

    /// Whether a dump keeps the field going after the app has the window.
    pub fn hold(&self) -> bool {
        self.dump.as_ref().is_some_and(|d| d.hold)
    }

    pub fn offscreen(&mut self, offscreen: bool) {
        self.offscreen = offscreen;
    }

    /// The time between the window's frames: the field animates at 30 fps at most.
    pub fn frame_period_ns(&self) -> u64 {
        let over_app = self.renderer.as_ref().is_some_and(|r| {
            r.layer.on && !matches!(r.layer.want(), None | Some(LayerWant::Boot))
        });
        if over_app {
            // the fades over the app, and the app's own frames shown through us
            16_666_667
        } else if self.field_mode() {
            33_333_333
        } else {
            16_666_667
        }
    }

    /// Whether the window layer is on (the field over the running app).
    pub fn layer_on(&self) -> bool {
        self.renderer.as_ref().is_some_and(|r| r.layer.on)
    }

    /// Whether this window can draw a window layer (backend and document).
    pub fn layers(&self) -> bool {
        self.layers
            && self.window.is_some()
            && self
                .renderer
                .as_ref()
                .is_some_and(|r| r.active.as_ref().is_some_and(|p| p.layer.is_some()))
    }

    /// The run's wish for the window layer this turn, and the app's presented frames so far (a
    /// new one is blurred again). `None` takes the layer away at once.
    pub fn layer(&mut self, want: Option<LayerWant>, app_frames: u64) {
        let want = want.filter(|_| self.layers() && self.overlay.is_none());
        let size = [
            (self.geometry[2] * self.dpi).round().max(1.0) as u32,
            (self.geometry[3] * self.dpi).round().max(1.0) as u32,
        ];
        let Some(window) = self.window else { return };
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        renderer.layer.set(want);
        renderer.layer.app_frames = app_frames;
        let mut writer = CmdWriter::new(HOST);
        match want {
            None if renderer.layer.on => {
                writer.window_overlay(window, false, [0.0; 4], Id::NONE);
                layer_textures(&mut writer, &renderer.layer, None);
                renderer.layer.on = false;
                renderer.layer.size = [0; 2];
                renderer.dirty = true;
            }
            Some(_) if !renderer.layer.on || renderer.layer.size != size => {
                if renderer.layer.size != [0; 2] {
                    layer_textures(&mut writer, &renderer.layer, None);
                }
                renderer.layer.gen = renderer.layer.gen.wrapping_add(1).max(1);
                renderer.layer.size = size;
                layer_textures(&mut writer, &renderer.layer, Some(size));
                writer.window_overlay(
                    window,
                    true,
                    [0.0, 0.0, self.geometry[2], self.geometry[3]],
                    renderer.layer.texture(layer::APP),
                );
                renderer.layer.on = true;
                renderer.layer.blurred = None;
            }
            _ => return,
        }
        Hal::global().submit(&writer.buf());
    }

    /// Whether frames are dumped (`MAKEPAD_RUN_BOOT_DUMP`): the run looks in at the dump's pace.
    pub fn dumping(&self) -> bool {
        self.dump.is_some()
    }

    /// A dump keeps the running app's frames while no badge and no layer is over them (the HAL's
    /// snapshot of the window), at the dump's pace.
    pub fn dump_app(&mut self) {
        let (Some(window), None, false) = (self.window, self.activity.window, self.layer_on()) else { return };
        if let Some(req) = self.dump.as_mut().and_then(|d| d.due()) {
            let mut writer = CmdWriter::new(HOST);
            writer.snapshot(window, req);
            Hal::global().submit(&writer.buf());
        }
    }

    /// ms on the dump's clock (its frames' file names), when frames are dumped.
    pub fn dump_ms(&self) -> Option<u128> {
        self.dump.as_ref().map(|d| d.ms())
    }

    /// Whether this window can show the JIT activity badge (the window system composites badges).
    pub fn badges(&self) -> bool {
        self.badges && self.window.is_some() && self.renderer.as_ref().is_some_and(|r| r.active.is_some())
    }

    /// The run's wish for the JIT activity badge this turn (`None`: the JIT is idle; a shown badge
    /// fades out, then goes). Draws the badge's frame when one is due; returns how long until the
    /// next (`None`: no badge).
    pub fn activity(&mut self, want: Option<ActivityWant>) -> Option<Duration> {
        let want = want.filter(|_| self.badges());
        let window = self.window?;
        self.activity.set(want);
        let mut writer = CmdWriter::new(HOST);
        let place = activity::Activity::place([self.geometry[2], self.geometry[3]]);
        let badge = match self.activity.window {
            Some(badge) if self.activity.done() || !self.badges() => {
                writer.window_close(badge);
                Hal::global().submit(&writer.buf());
                self.activity.window = None;
                return None;
            }
            Some(badge) => {
                if self.activity.placed != place {
                    writer.window_geom(badge, place);
                    self.activity.placed = place;
                }
                badge
            }
            None if self.activity.wanted() => {
                // one slot for the run's badges, a new generation each time one is made
                let slot = *self
                    .activity
                    .slot
                    .get_or_insert_with(|| Hal::global().with_world(|w| w.host_window_id()));
                let badge = Id::new(slot.index, slot.gen);
                self.activity.slot = Some(Id::new(slot.index, slot.gen.wrapping_add(1).max(1)));
                writer.window_create(badge, window_kind::BADGE, window, place, false);
                self.activity.window = Some(badge);
                self.activity.placed = place;
                badge
            }
            None => return None,
        };
        Hal::global().submit(&writer.buf());
        let now = Instant::now();
        if now >= self.activity.next_frame {
            self.present_badge(badge);
            self.activity.next_frame = now + activity::FRAME;
        }
        Some(self.activity.next_frame.saturating_duration_since(Instant::now()))
    }

    /// One frame of the activity badge: its galaxy, one quad over transparent pixels.
    fn present_badge(&mut self, badge: Id) {
        let reduce_motion = self.environment.reduce_motion;
        let dpi = self.dpi;
        let Some(renderer) = self.renderer.as_mut() else { return };
        let Some(program) = renderer.active.as_ref() else { return };
        let pipe = &program.pipes[ERROR_BAND];
        let look = self.activity.look(reduce_motion);
        let side = activity::SIDE;
        let viewport = [side, side];
        let mut uniforms = Vec::new();
        pipe.uniforms.write(
            &[
                (id!(viewport), &viewport),
                (id!(dpi), &[dpi]),
                (id!(reduce_motion), &[reduce_motion as u8 as f32]),
            ],
            &mut uniforms,
        );
        let mut data = Vec::new();
        pipe.instances.write(
            &[
                (id!(rect), &look.rect),
                (id!(accent), &[ACTIVITY_GALAXY]),
                (id!(selected), &[look.level]),
                (id!(block), &[look.block]),
                (id!(pressed), &[look.opacity]),
                (id!(action), &[look.spin]),
                (id!(beat), &[look.beat]),
            ],
            &mut data,
        );
        self.frame += 1;
        let mut writer = CmdWriter::new(HOST);
        writer.frame_begin(
            self.frame,
            FrameMeta {
                build_start_ns: makepad_hal::clock_ns(),
                ..Default::default()
            },
        );
        writer.pass(Pass {
            color: [Id::NONE; 4],
            load: [pass_load::CLEAR; 5],
            clear: [[0.0; 4]; 4],
            viewport: [0.0, 0.0, side * dpi, side * dpi],
            dpi,
            ..Default::default()
        });
        pipe.draw(&mut writer, &data, &uniforms, renderer.text.atlas(), [0.0, 0.0, side * dpi, side * dpi]);
        writer.frame_set_build_end(makepad_hal::clock_ns());
        // a dump keeps the app's frame with the badge over it (both read back, put together)
        let snapshot = match (&mut self.dump, self.window) {
            (Some(dump), Some(window)) => dump.due().map(|req| (window, req, dump.badge(self.activity.placed, dpi))),
            _ => None,
        };
        writer.frame_end(badge, target::WINDOW | if snapshot.is_some() { target::SNAPSHOT } else { 0 });
        if let Some((window, app, over)) = snapshot {
            writer.snapshot(window, app);
            writer.snapshot(badge, over);
        }
        Hal::global().submit(&writer.buf());
    }

    /// The layer has faded out (the ready lift): the run may take it away.
    pub fn layer_done(&mut self) -> bool {
        self.renderer
            .as_mut()
            .is_some_and(|r| r.layer.on && r.layer.peek_done())
    }

    /// The Ignition Field: the boot window, or the window layer over the running app.
    fn present_field(&mut self) {
        let Some(window) = self.window else { return };
        let viewport = [self.geometry[2], self.geometry[3]];
        let dpi = self.dpi;
        let reduce_motion = self.environment.reduce_motion;
        let caption = self.caption();
        let build_start_ns = makepad_hal::clock_ns();
        let renderer = self.renderer.as_mut().unwrap();
        let program = renderer.active.as_ref().unwrap();
        let mut writer = CmdWriter::new(HOST);
        let time = renderer.began.elapsed().as_secs_f32();
        let layered = renderer.layer.on && program.layer.is_some();
        let look = layered.then(|| renderer.layer.look(reduce_motion));
        let region = [0.0, 0.0, viewport[0], viewport[1]];
        renderer.text.begin_layout();
        let frame = renderer.field.frame(
            &mut writer,
            &program.pipes[FIELD],
            &self.state,
            time,
            region,
            reduce_motion,
            false,
            look.map_or(-1.0, |l| l.resolve),
            renderer.layer.on && !matches!(renderer.layer.want(), None | Some(LayerWant::Boot)),
        );
        let mut hint = program.label(layout::phase_key(self.state.phase));
        let mut status = None;
        if let Some(look) = look.filter(|l| l.progress >= 0.0) {
            hint = format!("warming up · {} functions", field::group(renderer.layer.ahead));
            status = Some(if look.resolve >= 0.0 { "ready" } else { "warming up" });
        }
        renderer.field.hud(
            &mut renderer.text,
            &self.state,
            &hint,
            status,
            viewport,
            frame.view,
            frame.params[0],
            time,
            &caption,
            self.offscreen,
            &mut renderer.layout,
        );
        renderer.text.end_layout();
        renderer.text.begin_frame();
        let mut requests = Vec::new();
        let mut glyphs = Vec::new();
        let bounds = [0.0, 0.0, viewport[0], viewport[1]];
        let fade = look.map_or(1.0, |l| l.galaxy);
        for line in &mut renderer.layout.fixed_text {
            let y = line.position[1];
            let mut line = TextLine {
                run: line.run.clone(),
                position: line.position,
                style: line.style,
                fixed: line.fixed,
            };
            line.style.color[3] *= fade;
            renderer.text.glyph_instances(
                program,
                &line,
                y,
                bounds,
                dpi,
                &mut requests,
                &mut glyphs,
            );
        }
        renderer.field.prime(&mut renderer.text, dpi, &mut requests);
        let ready = renderer.text.finish_frame(&mut writer, &requests);
        if !ready {
            Hal::global().submit(&writer.buf());
            return;
        }
        let sdf = makepad_font_core::raster::Settings::default().sdfer;
        let field_layer = look.map_or([0.0; 4], |l| [1.0, l.galaxy, l.backdrop, l.progress]);
        let field_fault = [0.0, 0.0, caption.height, 0.0];
        let common = |pipe: &gpu::Pipeline, out: &mut Vec<f32>| {
            pipe.uniforms.write(
                &[
                    (id!(viewport), &viewport),
                    (id!(dpi), &[dpi]),
                    (id!(dark), &[1.0]),
                    (id!(reduce_motion), &[reduce_motion as u8 as f32]),
                    (id!(time), &[time]),
                    (id!(view), &frame.view),
                    (id!(field_params), &frame.params),
                    (id!(field_fall), &frame.fall),
                    (id!(field_layer), &field_layer),
                    (id!(field_fault), &field_fault),
                    (id!(radius), &[sdf.radius]),
                    (id!(cutoff), &[sdf.cutoff]),
                ],
                out,
            );
        };
        let mut field_uniforms = Vec::new();
        common(&program.pipes[FIELD], &mut field_uniforms);
        let mut glyph_uniforms = Vec::new();
        common(&program.pipes[GLYPH], &mut glyph_uniforms);
        // the caption's window buttons (the field's only cards)
        let mut buttons = Vec::new();
        for card in renderer.layout.cards.iter().filter(|c| c.accent == CAPTION_BUTTON) {
            program.pipes[ERROR_BAND].instances.write(
                &[
                    (id!(rect), &card.rect),
                    (id!(selected), &[(renderer.hovered == Some(card.action)) as u8 as f32]),
                    (id!(accent), &[card.accent]),
                    (id!(action), &[button_glyph(card.action)]),
                    (id!(pressed), &[(renderer.pressed == Some(card.action)) as u8 as f32]),
                ],
                &mut buttons,
            );
        }
        let mut band_uniforms = Vec::new();
        common(&program.pipes[ERROR_BAND], &mut band_uniforms);
        self.frame += 1;
        let offscreen = if self.offscreen && !layered {
            let size = [(viewport[0] * dpi) as u32, (viewport[1] * dpi) as u32];
            Some((self.dump.as_mut().unwrap().target(&mut writer, size), size))
        } else {
            None
        };
        writer.frame_begin(
            self.frame,
            FrameMeta {
                build_start_ns,
                ..Default::default()
            },
        );
        // the blurred app: made again from a new app frame (the HAL copied the app's image into
        // APP at this frame's start): full -> half (a box), half -> quarter across, then down
        let layer_pipe = program.layer.map(|i| &program.pipes[i]);
        // the draws borrow their instance and uniform memory until the submit below
        let mut kept: Vec<Vec<f32>> = Vec::new();
        if let (Some(look), Some(pipe)) = (look, layer_pipe) {
            let layer = &mut renderer.layer;
            if look.blur && layer.blurred != Some(layer.app_frames) {
                layer.blurred = Some(layer.app_frames);
                let [w, h] = layer.size;
                let half = [w.div_ceil(2).max(1), h.div_ceil(2).max(1)];
                let quarter = [w.div_ceil(4).max(1), h.div_ceil(4).max(1)];
                for (source, target, size, mode, dir) in [
                    (layer::APP, layer::HALF, half, 0.0, [0.0, 0.0]),
                    (layer::HALF, layer::QUARTER_X, quarter, 1.0, [2.0 / half[0] as f32, 0.0]),
                    (layer::QUARTER_X, layer::QUARTER, quarter, 1.0, [0.0, 1.0 / quarter[1] as f32]),
                ] {
                    let size = [size[0] as f32, size[1] as f32];
                    let mut color = [Id::NONE; 4];
                    color[0] = layer.texture(target);
                    writer.pass(Pass {
                        color,
                        load: [pass_load::CLEAR; 5],
                        clear: [[0.0; 4]; 4],
                        viewport: [0.0, 0.0, size[0], size[1]],
                        dpi: 1.0,
                        ..Default::default()
                    });
                    let mut uniforms = Vec::new();
                    pipe.uniforms
                        .write(&[(id!(viewport), &size), (id!(dpi), &[1.0])], &mut uniforms);
                    let mut data = Vec::new();
                    pipe.instances.write(
                        &[
                            (id!(rect), &[0.0, 0.0, size[0], size[1]]),
                            (id!(mode), &[mode]),
                            (id!(dir), &dir),
                            (id!(sigma), &[layer::SIGMA]),
                        ],
                        &mut data,
                    );
                    pipe.draw(&mut writer, &data, &uniforms, layer.texture(source), [0.0, 0.0, size[0], size[1]]);
                    kept.push(data);
                    kept.push(uniforms);
                }
            }
        }
        let mut color = [Id::NONE; 4];
        if let Some((target, _)) = offscreen {
            color[0] = target;
        }
        writer.pass(Pass {
            color,
            load: [pass_load::CLEAR; 5],
            clear: [[0.0; 4]; 4],
            viewport: [0.0, 0.0, viewport[0] * dpi, viewport[1] * dpi],
            dpi,
            ..Default::default()
        });
        let scissor = [0.0, 0.0, viewport[0] * dpi, viewport[1] * dpi];
        let mut app = |writer: &mut CmdWriter, texture: u32, opacity: f32, dim: f32| {
            if let Some(pipe) = layer_pipe.filter(|_| opacity > 0.0) {
                let mut uniforms = Vec::new();
                pipe.uniforms
                    .write(&[(id!(viewport), &viewport), (id!(dpi), &[dpi])], &mut uniforms);
                let mut data = Vec::new();
                pipe.instances.write(
                    &[
                        (id!(rect), &[0.0, 0.0, viewport[0], viewport[1]]),
                        (id!(opacity), &[opacity]),
                        (id!(dim), &[dim]),
                    ],
                    &mut data,
                );
                pipe.draw(writer, &data, &uniforms, renderer.layer.texture(texture), scissor);
                kept.push(data);
                kept.push(uniforms);
            }
        };
        if look.is_some_and(|l| l.blur) {
            app(&mut writer, layer::QUARTER, 1.0, layer::BLUR_DIM);
        }
        if fade > 0.0 || look.is_some_and(|l| l.backdrop > 0.0) {
            program.pipes[FIELD].draw_buffer(
                &mut writer,
                field::INSTANCES,
                frame.count,
                &field_uniforms,
                scissor,
            );
        }
        if !buttons.is_empty() {
            program.pipes[ERROR_BAND].draw(
                &mut writer,
                &buttons,
                &band_uniforms,
                renderer.text.atlas(),
                scissor,
            );
        }
        program.pipes[GLYPH].draw(
            &mut writer,
            &glyphs,
            &glyph_uniforms,
            renderer.text.atlas(),
            scissor,
        );
        if let Some(look) = look {
            app(&mut writer, layer::APP, look.sharp, 0.0);
        }
        writer.frame_set_build_end(makepad_hal::clock_ns());
        let snapshot = self.dump.as_mut().and_then(|d| d.due());
        if let Some((target, size)) = offscreen {
            writer.frame_end(target, target::TEXTURE);
            if let Some(req) = snapshot {
                writer.texture_readback(target, req, [0, 0, size[0], size[1]]);
            }
        } else {
            writer.frame_end(
                window,
                target::WINDOW
                    | if snapshot.is_some() {
                        target::SNAPSHOT
                    } else {
                        0
                    },
            );
            if let Some(req) = snapshot {
                writer.snapshot(window, req);
            }
        }
        Hal::global().submit(&writer.buf());
        drop(kept);
        if self.gpu && self.frame % 60 == 0 {
            let stats = Hal::global().with_world(|w| {
                w.session(HOST)
                    .map(|s| (s.stats.gpu_frames, s.stats.gpu_ns_total, s.stats.gpu_ns_max))
            });
            if let Some((frames, total, max)) = stats.filter(|s| s.0 > 0) {
                eprintln!(
                    "makepad-run: boot window gpu: {} frames, mean {:.0} us, max {:.0} us",
                    frames,
                    total as f64 / frames as f64 / 1e3,
                    max as f64 / 1e3
                );
            }
        }
        renderer.dirty = false;
        renderer.redraw = false;
        renderer.presented_revision = renderer.layout_revision;
        renderer.presented_scroll = renderer.scroll;
    }
}

/// The window layer's textures: made at `size` (window pixels), or destroyed (`None`).
fn layer_textures(writer: &mut CmdWriter, layer: &layer::Layer, size: Option<[u32; 2]>) {
    for (index, divide) in [
        (layer::APP, 1),
        (layer::HALF, 2),
        (layer::QUARTER_X, 4),
        (layer::QUARTER, 4),
    ] {
        let id = layer.texture(index);
        match size {
            Some([w, h]) => writer.texture_create(
                id,
                usage::RENDER_TARGET | usage::SAMPLED,
                format::BGRA8,
                w.div_ceil(divide).max(1),
                h.div_ceil(divide).max(1),
                [1, 1],
                Attr::default(),
            ),
            None => writer.texture_destroy(id),
        }
    }
}

impl Layout {
    /// The cards on screen at `scroll`: the list's cards that meet the clip, and the fixed ones
    /// (last). A member row lies inside its group's card and comes after it.
    fn visible_cards(&self, scroll: f32) -> impl DoubleEndedIterator<Item = &Card> {
        let top = self.clip[1] + scroll;
        let bottom = top + self.clip[3];
        self.cards
            .iter()
            .filter(move |c| c.fixed || (c.rect[1] + c.rect[3] >= top && c.rect[1] <= bottom))
    }
}

/// `Card::accent` of a window button of the caption.
const CAPTION_BUTTON: f32 = 4.0;
/// `error_band.accent` of the activity badge's galaxy (activity.rs).
const ACTIVITY_GALAXY: f32 = 5.0;

/// The glyph a window button of the caption shows (document `error_band.action`): 0 minimise,
/// 1 maximise / restore, 2 close.
fn button_glyph(action: Action) -> f32 {
    match action {
        Action::Minimize => 0.0,
        Action::Maximize => 1.0,
        _ => 2.0,
    }
}

fn contains(rect: [f32; 4], point: [f32; 2]) -> bool {
    point[0] >= rect[0]
        && point[1] >= rect[1]
        && point[0] < rect[0] + rect[2]
        && point[1] < rect[1] + rect[3]
}

fn crop(mut rect: [f32; 4], mut uv: [f32; 4], clip: [f32; 4]) -> ([f32; 4], [f32; 4]) {
    if rect[2] <= 0.0 || rect[3] <= 0.0 {
        return (rect, uv);
    }
    for axis in 0..2 {
        let start = rect[axis].max(clip[axis]);
        let end = (rect[axis] + rect[axis + 2]).min(clip[axis] + clip[axis + 2]);
        let scale = (uv[axis + 2] - uv[axis]) / rect[axis + 2];
        uv[axis + 2] = uv[axis] + (end - rect[axis]) * scale;
        uv[axis] += (start - rect[axis]) * scale;
        rect[axis] = start;
        rect[axis + 2] = (end - start).max(0.0);
    }
    (rect, uv)
}
