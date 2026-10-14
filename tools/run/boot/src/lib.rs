//! Session 0's text/pass renderer. No platform client, draw tree, widgets or Cx.
mod document;
mod gpu;
mod icon;
mod layout;
mod text;

use document::{Program, BACKGROUND, ERROR_BAND, GLYPH, ICON, PIPE_BANK_SIZE, PROGRESS};
pub use icon::Icon;
use makepad_hal::{
    backend_id, buffer_kind, clock_ns, dialect, event_tag, font::wire, input, lifecycle, pass_load,
    session::HOST, target, window_kind, Attr, CmdWriter, EventBuf, EventQueue, FrameMeta, Hal,
    HalEvent, HalInfo, Id, Pass, RawEvent,
};
use makepad_script::{id, LiveId};
pub use relax_base::{
    diag::Diagnostic,
    run_state::{Mode, State},
};
use std::{path::PathBuf, sync::Arc, time::Instant};

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
}

#[derive(Clone)]
pub struct Document {
    pub path: PathBuf,
    pub source: Arc<str>,
}
impl Document {
    pub fn bundled() -> Self {
        Self {
            path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/boot.splash"),
            source: DEFAULT_DOCUMENT.into(),
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
    button: bool,
    fixed: bool,
}

#[derive(Default)]
struct Layout {
    panel: [f32; 4],
    clip: [f32; 4],
    text: Vec<TextLine>,
    text_ends: Vec<f32>,
    fixed_text: Vec<TextLine>,
    cards: Vec<Card>,
    progress: Option<[f32; 4]>,
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
    from: f32,
    target: f32,
    changed: f32,
    began: Instant,
    input: Option<String>,
    selected: usize,
    reveal_selection: bool,
    scroll: f32,
    dialect: u32,
    bundled: bool,
    clear_errors_on_install: bool,
    function_failed: bool,
    icon_aspect: Option<f32>,
    pressed: Option<Action>,
    layout_revision: u64,
    presented_revision: u64,
    presented_scroll: f32,
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
        };
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
            from: 0.0,
            target: 0.0,
            changed: 0.0,
            began: Instant::now(),
            input: None,
            selected: 0,
            reveal_selection: false,
            scroll: 0.0,
            dialect: info.dialect,
            bundled,
            clear_errors_on_install,
            function_failed: false,
            icon_aspect: None,
            pressed: None,
            layout_revision: 0,
            presented_revision: 0,
            presented_scroll: 0.0,
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
            let motion_changed = self.environment.reduce_motion != environment.reduce_motion;
            self.environment = environment;
            if let Some(renderer) = &mut self.renderer {
                if motion_changed {
                    // Leaving reduced motion must not interpolate backwards
                    // from an endpoint older than the already-visible target.
                    renderer.from = renderer.target;
                    renderer.changed = renderer.began.elapsed().as_secs_f32();
                }
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
            let reset = state.elapsed < self.state.elapsed;
            if reset {
                renderer.from = 0.0;
                renderer.target = 0.0;
                renderer.changed = 0.0;
                renderer.began = Instant::now();
                renderer.scroll = 0.0;
            }
            if let Some(program) = renderer
                .active
                .as_mut()
                .filter(|_| !renderer.function_failed)
            {
                let now = renderer.began.elapsed().as_secs_f32();
                let values = [
                    renderer.target as f64,
                    state.completed_weight,
                    state.phase_weight,
                    state.total_weight,
                    state.done.unwrap_or(0) as f64,
                    state.total.unwrap_or(0) as f64,
                    state.phase_elapsed,
                ];
                match program.call(id!(phase_target), &values) {
                    Ok(target) if target != renderer.target => {
                        match program.call(
                            id!(ease),
                            &[
                                renderer.from as f64,
                                renderer.target as f64,
                                (now - renderer.changed) as f64,
                            ],
                        ) {
                            Ok(from) => {
                                renderer.from = if self.environment.reduce_motion {
                                    target
                                } else {
                                    from
                                }
                            }
                            Err(errors) => {
                                renderer.function_failed = true;
                                self.events.push(Event::DocumentErrors(errors));
                            }
                        }
                        renderer.redraw = true;
                        renderer.target = target;
                        renderer.changed = now;
                    }
                    Ok(_) => {}
                    Err(errors) => {
                        renderer.function_failed = true;
                        self.events.push(Event::DocumentErrors(errors));
                    }
                }
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
            let has_file = |i: usize| {
                self.state
                    .diagnostics
                    .get(i)
                    .is_some_and(|d| !d.file.is_empty())
            };
            renderer.dirty |=
                self.environment.touch && has_file(renderer.selected) != has_file(diagnostic);
            renderer.selected = diagnostic;
            renderer.redraw = true;
        }
    }

    pub fn select(&mut self, diagnostic: usize) {
        self.choose(diagnostic);
        if let Some(renderer) = &mut self.renderer {
            if renderer.dirty {
                renderer.reveal_selection = true;
            } else if let Some(card) = renderer
                .layout
                .cards
                .iter()
                .find(|c| c.action == Action::Select(diagnostic))
            {
                renderer.scroll = (card.rect[1] - renderer.layout.clip[1]).max(0.0);
            }
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
        if let Some(window) = self.window {
            let mut writer = CmdWriter::new(HOST);
            writer.window_overlay(window, rect.is_some(), rect.unwrap_or_default());
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
                    ..
                } if Some(window) == self.window => {
                    self.geometry = [x as f32, y as f32, w as f32, h as f32];
                    if dpi.is_finite() && dpi > 0.0 {
                        self.dpi = dpi;
                    }
                    if let Some(renderer) = &mut self.renderer {
                        renderer.dirty = true;
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
                renderer.from = renderer.target;
                renderer.changed = renderer.began.elapsed().as_secs_f32();
                renderer.dirty = true;
                renderer.function_failed = false;
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
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        if renderer.active.is_none() {
            return;
        }
        let animating = renderer.layout.progress.is_some()
            && !self.environment.reduce_motion
            && renderer.began.elapsed().as_secs_f32() - renderer.changed
                < renderer.active.as_ref().unwrap().settle_seconds;
        if !renderer.redraw && !renderer.dirty && !animating {
            return;
        }
        if renderer.dirty {
            renderer.layout(
                &self.state,
                [self.geometry[2], self.geometry[3]],
                self.environment,
            );
            renderer.dirty = false;
        }
        let rect = (self.state.mode == Mode::Overlay).then_some(renderer.layout.panel);
        self.set_overlay(rect);
        let renderer = self.renderer.as_mut().unwrap();
        let program = renderer.active.as_ref().unwrap();
        let build_start_ns = makepad_hal::clock_ns();
        let mut writer = CmdWriter::new(HOST);
        let mut data = vec![Vec::new(); program.pipes.len()];
        let viewport = [self.geometry[2], self.geometry[3]];
        let background = if self.state.mode == Mode::Overlay {
            renderer.layout.panel
        } else {
            [0.0, 0.0, viewport[0], viewport[1]]
        };
        program.pipes[BACKGROUND]
            .instances
            .write(&[(id!(rect), &background)], &mut data[BACKGROUND]);
        let max_scroll = (renderer.layout.content_height - renderer.layout.clip[3]).max(0.0);
        renderer.scroll = renderer.scroll.min(max_scroll);
        let scroll = renderer.scroll;
        let clip = renderer.layout.clip;
        if let Some(mut rect) = renderer.layout.progress {
            rect[1] -= scroll;
            program.pipes[PROGRESS]
                .instances
                .write(&[(id!(rect), &rect)], &mut data[PROGRESS]);
        }
        if let Some(mut rect) = renderer.layout.icon.filter(|_| program.pipes.len() > ICON) {
            if !renderer.layout.icon_fixed {
                rect[1] -= scroll;
            }
            program.pipes[ICON].instances.write(
                &[
                    (id!(rect), &rect),
                    (
                        id!(clip_rect),
                        &if renderer.layout.icon_fixed {
                            renderer.layout.panel
                        } else {
                            clip
                        },
                    ),
                ],
                &mut data[ICON],
            );
        }
        for card in renderer.layout.visible_cards(scroll) {
            let mut rect = card.rect;
            let bounds = if card.fixed {
                renderer.layout.panel
            } else {
                clip
            };
            if !card.fixed {
                rect[1] -= scroll;
            }
            if rect[1] + rect[3] < bounds[1] || rect[1] > bounds[1] + bounds[3] {
                continue;
            }
            program.pipes[ERROR_BAND].instances.write(
                &[
                    (id!(rect), &rect),
                    (id!(clip_rect), &bounds),
                    (
                        id!(selected),
                        &[(card.action == Action::Select(renderer.selected)) as u8 as f32],
                    ),
                    (id!(action), &[card.button as u8 as f32]),
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
        for line in visible.chain(renderer.layout.fixed_text.iter()) {
            let y = line.position[1] - if line.fixed { 0.0 } else { scroll };
            let bounds = if line.fixed {
                renderer.layout.panel
            } else {
                clip
            };
            for glyph in line.run.visible(
                bounds[1] - y,
                bounds[1] + bounds[3] - y,
                line.style.size,
                line.style.height,
            ) {
                let baseline = [
                    line.position[0]
                        + glyph.x
                        + (line.run.width - glyph.row_width) * line.style.align,
                    y + glyph.y,
                ];
                if !line.fixed
                    && (baseline[1] + line.style.size < clip[1]
                        || baseline[1] - line.style.size > clip[1] + clip[3])
                {
                    continue;
                }
                let Some(entry) =
                    renderer
                        .text
                        .lookup(glyph.id, line.style.size, self.dpi, &mut requests)
                else {
                    continue;
                };
                if entry.atlas_kind == wire::glyph::EMPTY {
                    continue;
                }
                let (rect, uv) = renderer.text.quad(entry, baseline, line.style.size);
                // Clip in the vertex shader's coordinate space by cropping rect
                // and UV together, so fixed key hints can share one glyph draw.
                let (rect, uv) = crop(rect, uv, bounds);
                if rect[2] <= 0.0 || rect[3] <= 0.0 {
                    continue;
                }
                program.pipes[GLYPH].instances.write(
                    &[
                        (id!(rect), &rect),
                        (id!(uv), &uv),
                        (id!(color), &line.style.color),
                        (id!(atlas_plane), &[entry.plane as f32]),
                        (
                            id!(color_glyph),
                            &[if entry.atlas_kind == wire::glyph::COLOR {
                                1.0
                            } else {
                                0.0
                            }],
                        ),
                    ],
                    &mut data[GLYPH],
                );
            }
        }
        let ready = renderer.text.finish_frame(&mut writer, &requests);
        if !ready {
            // Submit glyph requests without a FramePacket; retain the last image
            // and retry on later turns while input and completions keep flowing.
            Hal::global().submit(&writer.buf());
            return;
        }
        let mut uniforms = vec![Vec::new(); program.pipes.len()];
        let mut marks = [0.0; 5];
        let mut completed = 0.0;
        for (i, weight) in self.state.phase_weights.iter().take(5).enumerate() {
            completed += weight;
            marks[i] = (completed / self.state.total_weight.max(f64::EPSILON)) as f32;
        }
        let sdf = makepad_font_core::raster::Settings::default().sdfer;
        for (i, pipe) in program.pipes.iter().enumerate() {
            pipe.uniforms.write(
                &[
                    (id!(viewport), &viewport),
                    (id!(insets), &self.environment.insets),
                    (
                        id!(dark),
                        &[self.environment.dark.unwrap_or(false) as u8 as f32],
                    ),
                    (id!(touch), &[self.environment.touch as u8 as f32]),
                    (
                        id!(reduce_motion),
                        &[self.environment.reduce_motion as u8 as f32],
                    ),
                    (id!(dpi), &[self.dpi]),
                    (
                        id!(overlay),
                        &[(self.state.mode == Mode::Overlay) as u8 as f32],
                    ),
                    (id!(phase_marks), &marks[..4]),
                    (id!(phase_last), &marks[4..]),
                    (id!(phase), &[self.state.phase as usize as f32]),
                    (id!(elapsed), &[self.state.elapsed as f32]),
                    (id!(warm), &[self.state.warm as u8 as f32]),
                    (id!(time), &[renderer.began.elapsed().as_secs_f32()]),
                    (id!(from), &[renderer.from]),
                    (id!(previous), &[renderer.target]),
                    (id!(changed), &[renderer.changed]),
                    (id!(completed), &[self.state.completed_weight as f32]),
                    (id!(weight), &[self.state.phase_weight as f32]),
                    (id!(total_weight), &[self.state.total_weight as f32]),
                    (id!(done), &[self.state.done.unwrap_or(0) as f32]),
                    (id!(total), &[self.state.total.unwrap_or(0) as f32]),
                    (id!(phase_elapsed), &[self.state.phase_elapsed as f32]),
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
            viewport: [0.0, 0.0, viewport[0] * self.dpi, viewport[1] * self.dpi],
            dpi: self.dpi,
            ..Default::default()
        });
        for i in 0..program.pipes.len() {
            let rect = if i == PROGRESS { clip } else { background };
            program.pipes[i].draw(
                &mut writer,
                &data[i],
                &uniforms[i],
                if i == ICON {
                    icon::TEXTURE
                } else {
                    renderer.text.atlas()
                },
                rect.map(|v| v * self.dpi),
            );
        }
        writer.frame_set_build_end(makepad_hal::clock_ns());
        writer.frame_end(window, target::WINDOW);
        Hal::global().submit(&writer.buf());
        renderer.presented_revision = renderer.layout_revision;
        renderer.presented_scroll = renderer.scroll;
        renderer.redraw = false;
    }
}

impl Layout {
    fn visible_cards(&self, scroll: f32) -> impl Iterator<Item = &Card> {
        let (content, fixed) = self
            .cards
            .split_at(self.cards.partition_point(|c| !c.fixed));
        let start = content.partition_point(|c| c.rect[1] + c.rect[3] < self.clip[1] + scroll);
        content[start..]
            .iter()
            .take_while(move |c| c.rect[1] <= self.clip[1] + self.clip[3] + scroll)
            .chain(fixed.iter())
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
