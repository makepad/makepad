//! Native overview of real iteration state. The worker supplies immutable
//! snapshots; this widget performs no filesystem, process or network work.
use crate::iteration::{Artifact, BuildPhase, Engine, Flow, FlowLifecycle, RunRole, TodoState};
use crate::iteration_host_view::IterationRunView;
use crate::{
    canvas::CanvasCamera,
    canvas_input::{remap_event, sync_handled},
};
use crate::canvas_draw::DrawCanvasCard;
use makepad_terminal::widget::{MpTerm, TerminalPresentationFrame};
use makepad_widgets::image::DrawImage;
use makepad_widgets::makepad_platform::{event::TouchState, thread::lock_from_ui};
use makepad_widgets::tip::TipAction;
use makepad_widgets::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    mod.widgets.StudioIterationView = #(StudioIterationView::register_widget(vm)){
        width: Fill height: Fill
        background: theme.color_bg_app
        surface: mix(theme.color_bg_app, theme.color_text, 0.028)
        raised: mix(theme.color_bg_app, theme.color_text, 0.055)
        edge: mix(theme.color_bg_app, theme.color_text, 0.12)
        ink: theme.color_text
        muted: mix(theme.color_bg_app, theme.color_text, 0.57)
        accent: theme.color_focus
        draw_card +: {shadow_color: #0002}
        draw_title +: {text_style: theme.font_regular{font_size: 9.5} color: theme.color_text}
        draw_text +: {text_style: theme.font_regular{font_size: 9} color: theme.color_text}
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum IterationViewAction {
    SelectFlow {
        id: String,
    },
    ConnectTerminal {
        flow: String,
    },
    StartFlow {
        flow: String,
    },
    StopFlow {
        flow: String,
    },
    ToggleSplit {
        flow: String,
    },
    SplitFlow {
        flow: String,
        before: String,
    },
    ArchiveFlow {
        flow: String,
    },
    FoldBuild {
        flow: String,
        artifact: String,
    },
    BuildStandalone {
        flow: String,
        source_revision: u64,
    },
    BuildEmbedded {
        flow: String,
        source_revision: u64,
    },
    InspectCode {
        flow: String,
        artifact: Option<String>,
    },
    PlayRecording {
        flow: String,
        id: String,
    },
    OpenImage {
        flow: String,
        preview_id: String,
        title: String,
    },
    ClearHistory {
        flow: String,
    },
    DeleteRecordings {
        flow: String,
    },
    FoldItem {
        flow: String,
        key: String,
    },
    CloseFreeze {
        flow: String,
        run_id: String,
    },
    TerminalResized {
        flow: String,
        height: f64,
    },
    LaneWidth {
        flow: String,
        width: f64,
    },
    PopOut {
        flow: String,
        run_id: String,
    },
    AttachImage {
        flow: String,
        path: PathBuf,
    },
    PromptSubmitted {
        flow: String,
    },
    ToggleAttachmentTray {
        flow: String,
    },
    ToggleNavigator {
        flow: String,
    },
    TestTileToggled {
        flow: String,
        id: String,
        preview_id: String,
        open: bool,
    },
    #[default]
    None,
}

#[derive(Clone)]
struct Item {
    key: String,
    y: f64,
    height: f64,
    // Keep the expanded content at its natural size while its row is clipped.
    content_height: f64,
    fold_open: Option<f64>,
    title: String,
    lines: Vec<String>,
    line_height: f64,
    action: Option<IterationViewAction>,
    emphasized: bool,
    preview: Option<String>,
    preview_action: Option<IterationViewAction>,
    preview_height: f64,
    live_app: bool,
    pills: Vec<TodoPill>,
    controls: Vec<FrameControl>,
}
struct FoldAnimation {
    flow: String,
    key: String,
    started: f64,
    from_height: f64,
    from_open: f64,
    // Header positions relative to the history viewport, independent of its
    // bottom slack. Explicit user scrolling cancels this temporary anchor.
    header_anchor: Option<(f64, f64)>,
    target: Item,
    content: Item,
}
#[derive(Clone)]
struct FrameControl {
    label: &'static str,
    action: IterationViewAction,
}
#[derive(Clone)]
struct TodoPill {
    label: String,
    state: TodoState,
    rect: Rect,
}
struct Lane {
    id: String,
    title: String,
    subtitle: String,
    items: Arc<Vec<Item>>,
    height: f64,
}
struct Target {
    rect: Rect,
    action: IterationViewAction,
}
struct LiveHost {
    widget: WidgetRef,
    draw_list: Option<DrawList2d>,
    screen: Option<Rect>,
    clip: Option<Rect>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum LiveBody {
    Terminal(String),
    App(String),
}
impl LiveBody {
    fn flow(&self) -> &str {
        match self {
            Self::Terminal(flow) | Self::App(flow) => flow,
        }
    }
}
struct Preview {
    texture: Texture,
    width: u32,
    height: u32,
}
#[derive(Clone, Debug, PartialEq)]
pub struct TestTile {
    pub id: String,
    pub title: String,
    pub preview_id: String,
    /// Original capture dimensions, before thumbnail downsampling.
    pub width: u32,
    pub height: u32,
    pub active: bool,
    pub complete: bool,
    pub playable: bool,
    pub recorded_at: u64,
    pub elapsed_ms: u64,
}
#[derive(Clone)]
enum NavigationDrag {
    LaneWidth {
        flow: String,
        start: DVec2,
        width: f64,
    },
    Terminal {
        flow: String,
        start: DVec2,
        height: f64,
    },
    Scrollbar {
        flow: String,
        start_y: f64,
        offset: f64,
    },
    Minimap {
        flow: String,
    },
}

#[derive(Script, ScriptHook, WidgetRegister, WidgetRef, WidgetSet)]
pub struct StudioIterationView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[live]
    draw_shape: DrawColor,
    #[live]
    draw_marks: DrawVector,
    #[live]
    draw_card: DrawCanvasCard,
    #[live]
    draw_title: DrawText,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_image: DrawImage,
    #[live]
    background: Vec4f,
    #[live]
    surface: Vec4f,
    #[live]
    raised: Vec4f,
    #[live]
    edge: Vec4f,
    #[live]
    ink: Vec4f,
    #[live]
    muted: Vec4f,
    #[live]
    accent: Vec4f,
    #[rust]
    area: Area,
    #[rust]
    viewport: Rect,
    #[rust]
    engine: Option<Arc<Engine>>,
    #[rust]
    lanes: Vec<Lane>,
    #[rust]
    scroll: BTreeMap<String, f64>,
    #[rust]
    scroll_target: BTreeMap<String, f64>,
    #[rust]
    follow_tail: BTreeSet<String>,
    #[rust]
    scroll_frame: NextFrame,
    #[rust]
    scroll_time: f64,
    #[rust]
    terminals: BTreeMap<String, LiveHost>,
    #[rust]
    terminals_enabled: bool,
    #[rust]
    connection_labels: BTreeMap<String, String>,
    #[rust]
    apps: BTreeMap<String, LiveHost>,
    #[rust]
    app_artifacts: BTreeMap<String, String>,
    #[rust]
    terminal_heights: BTreeMap<String, f64>,
    #[rust]
    lane_widths: BTreeMap<String, f64>,
    #[rust]
    width_hover: Option<String>,
    #[rust]
    body_capture: Option<LiveBody>,
    #[rust]
    body_touch_capture: Option<(u64, LiveBody)>,
    #[rust]
    body_hover: Option<LiveBody>,
    #[rust]
    navigation_drag: Option<NavigationDrag>,
    #[rust]
    previews: BTreeMap<String, Preview>,
    #[rust]
    attachments: BTreeMap<String, Vec<String>>,
    #[rust]
    attachment_tray: BTreeMap<String, Vec<String>>,
    #[rust]
    collapsed_trays: BTreeSet<String>,
    #[rust]
    tray_draw_list: Option<DrawList2d>,
    #[rust]
    test_tiles: BTreeMap<String, Vec<TestTile>>,
    #[rust]
    recording_history: BTreeMap<String, Vec<TestTile>>,
    #[rust]
    recording_artifacts: BTreeMap<String, Vec<(String, String)>>,
    #[rust]
    pixel_view: Option<(String, String)>,
    #[rust]
    minimap_open: Option<String>,
    #[rust]
    cut_flow: Option<String>,
    #[rust]
    cut_hover: Option<String>,
    #[rust]
    archived_flow: Option<String>,
    #[rust]
    pixel_pan: DVec2,
    #[rust]
    pixel_drag: Option<(DVec2, DVec2)>,
    #[rust]
    pixel_source_size: DVec2,
    #[rust]
    full_preview: Option<(String, Preview)>,
    #[rust]
    pixel_draw_list: Option<DrawList2d>,
    #[rust]
    expanded: BTreeSet<String>,
    #[rust]
    fold_animations: BTreeMap<String, FoldAnimation>,
    #[rust]
    fold_frame: NextFrame,
    #[rust]
    fold_time: f64,
    #[rust]
    selected: Option<String>,
    #[rust]
    hovered: Option<IterationViewAction>,
    #[rust]
    focused_terminal: Option<String>,
    #[rust]
    targets: Vec<Target>,
    #[rust]
    zoom: f64,
    #[rust]
    pan: DVec2,
    #[rust]
    needs_fit: bool,
    #[rust]
    fitted: bool,
    #[rust]
    drag: Option<(DVec2, DVec2)>,
    #[rust]
    history_drag: Option<(String, f64)>,
    #[rust]
    pressed: Option<IterationViewAction>,
}

const COLUMN_WIDTH: f64 = 800.0;
const GAP: f64 = 20.0;
const LEFT: f64 = 20.0;
const TERMINAL_TOP: f64 = 2.0;
const TERMINAL_DEFAULT_HEIGHT: f64 = 104.0;
const TERMINAL_PADDING: f64 = 10.0;
const APP_HEIGHT: f64 = 450.0;
const MINI_WIDTH: f64 = 72.0;
const MINI_HEIGHT: f64 = 110.0;
const FOLD_DURATION: f64 = 0.2;

fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect {
        pos: dvec2(x, y),
        size: dvec2(w, h),
    }
}
fn short(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let mut value: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        value.push('…');
    }
    value
}
fn wrap(text: &str, max_lines: usize, columns: usize) -> Vec<String> {
    let mut lines = vec![];
    let mut line = String::new();
    let mut remaining = false;
    for word in text.split_whitespace() {
        if line.chars().count() + word.chars().count() + 1 > columns && !line.is_empty() {
            lines.push(line);
            line = String::new();
            if lines.len() >= max_lines {
                remaining = true;
                break;
            }
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&short(word, columns));
    }
    if !line.is_empty() && lines.len() < max_lines {
        lines.push(line);
    }
    if remaining {
        if let Some(last) = lines.last_mut() {
            last.push('…');
        }
    }
    lines
}
fn bullet_lines(text: &str, columns: usize) -> Vec<String> {
    let text = text
        .trim()
        .trim_start_matches("• ")
        .trim_start_matches("- ");
    wrap(text, 64, columns.saturating_sub(3))
        .into_iter()
        .enumerate()
        .map(|(index, line)| format!("{}{line}", if index == 0 { "• " } else { "   " }))
        .collect()
}
fn feedback_details(summary: &str, columns: usize) -> (String, Vec<String>) {
    let mut paragraphs = summary
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let first = paragraphs.next().unwrap_or_default();
    if let Some(tweaks) = first.strip_prefix("Live design requirements, generation ") {
        let heading = tweaks.split('.').next().unwrap_or(tweaks);
        let mut lines = vec![];
        for paragraph in paragraphs {
            if let Some((widget, change)) = paragraph.split_once(" · ") {
                if let Some((value, origin)) = change.rsplit_once(" @ ") {
                    lines.extend(bullet_lines(value, columns));
                    lines.extend(
                        wrap(
                            &format!("{widget} · {origin}"),
                            64,
                            columns.saturating_sub(3),
                        )
                        .into_iter()
                        .map(|line| format!("   {line}")),
                    );
                    continue;
                }
            }
            lines.extend(bullet_lines(paragraph, columns));
        }
        (format!("Design tweaks · generation {heading}"), lines)
    } else {
        let lines = std::iter::once(first)
            .chain(paragraphs)
            .flat_map(|paragraph| bullet_lines(paragraph, columns))
            .collect();
        (short(first, 80), lines)
    }
}
fn push_item(
    lane: &mut Lane,
    key: String,
    title: String,
    lines: Vec<String>,
    action: Option<IterationViewAction>,
    emphasized: bool,
) {
    let height = if lines.is_empty() {
        28.0
    } else {
        // Details start at y=26; leave breathing room below the last line.
        26.0 + lines.len() as f64 * 14.0 + 12.0
    };
    Arc::make_mut(&mut lane.items).push(Item {
        key,
        y: lane.height,
        height,
        content_height: height,
        fold_open: None,
        title,
        lines,
        line_height: 14.0,
        action,
        emphasized,
        preview: None,
        preview_action: None,
        preview_height: 0.0,
        live_app: false,
        pills: vec![],
        controls: vec![],
    });
    lane.height += height + 10.0;
}

impl StudioIterationView {
    pub fn set_connection_labels(&mut self, cx: &mut Cx, labels: BTreeMap<String, String>) {
        if self.connection_labels != labels {
            self.connection_labels = labels;
            self.redraw(cx);
        }
    }
    pub fn set_engine(&mut self, cx: &mut Cx, engine: Arc<Engine>) {
        if self
            .engine
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &engine))
        {
            return;
        }
        if self.engine.is_none() {
            for flow in engine.flows.values() {
                if let Some(artifact) = flow.artifacts.last() {
                    self.expanded.insert(format!("{}/{}", flow.id, artifact.id));
                }
            }
        }
        let anchors: Vec<_> = self
            .lanes
            .iter()
            .filter_map(|lane| {
                if self.follow_tail.contains(&lane.id)
                    || self.fold_animations.values().any(|animation| {
                        animation.flow == lane.id && animation.header_anchor.is_some()
                    })
                {
                    return None;
                }
                let offset = self.scroll.get(&lane.id).copied().unwrap_or(0.0);
                if offset <= 1.0 {
                    return None;
                }
                lane.items
                    .iter()
                    .find(|item| item.y + item.height > offset)
                    .map(|item| (lane.id.clone(), item.key.clone(), item.y - offset))
            })
            .collect();
        let same_project = self.engine.as_ref().is_none_or(|current| {
            current.flows.iter().all(|(id, flow)| {
                engine
                    .flows
                    .get(id)
                    .is_none_or(|next| next.config.repo == flow.config.repo)
            })
        });
        if !same_project {
            self.scroll.clear();
            self.scroll_target.clear();
            self.lane_widths.clear();
            self.width_hover = None;
            self.expanded.clear();
            self.fold_animations.clear();
            self.cut_flow = None;
            self.cut_hover = None;
            self.archived_flow = None;
            self.selected = None;
            self.previews.clear();
            self.attachments.clear();
            self.attachment_tray.clear();
            self.collapsed_trays.clear();
            self.test_tiles.clear();
            self.recording_history.clear();
            self.pixel_view = None;
            self.full_preview = None;
            for host in self.terminals.values() {
                Self::anchor_terminal(cx, &host.widget, None);
            }
            for host in self.apps.values() {
                Self::detach_app(cx, &host.widget);
            }
            self.terminals.clear();
            self.apps.clear();
            self.app_artifacts.clear();
            self.body_capture = None;
            self.body_touch_capture = None;
            self.body_hover = None;
            cx.widget_tree_mark_dirty(self.uid);
        }
        self.engine = Some(engine);
        self.sync_app_artifacts();
        self.rebuild();
        if same_project {
            for (id, key, relative_y) in anchors {
                if let Some(item) = self
                    .lanes
                    .iter()
                    .find(|lane| lane.id == id)
                    .and_then(|lane| lane.items.iter().find(|item| item.key == key))
                {
                    self.scroll.insert(id, (item.y - relative_y).max(0.0));
                }
            }
        }
        self.scroll_target = self.scroll.clone();
        if let Some(engine) = &self.engine {
            self.lane_widths
                .retain(|id, _| engine.flows.contains_key(id));
            let before = self.terminals.len() + self.apps.len();
            self.terminals.retain(|id, host| {
                let keep = engine
                    .flows
                    .get(id)
                    .is_some_and(|flow| flow.lifecycle != FlowLifecycle::Archived);
                if !keep {
                    Self::anchor_terminal(cx, &host.widget, None);
                }
                keep
            });
            self.apps.retain(|id, host| {
                let keep = engine.flows.contains_key(id);
                if !keep {
                    Self::detach_app(cx, &host.widget);
                }
                keep
            });
            self.attachments
                .retain(|id, _| engine.flows.contains_key(id));
            self.attachment_tray
                .retain(|id, _| engine.flows.contains_key(id));
            self.collapsed_trays
                .retain(|id| engine.flows.contains_key(id));
            self.test_tiles
                .retain(|id, _| engine.flows.contains_key(id));
            self.recording_history
                .retain(|id, _| engine.flows.contains_key(id));
            self.previews.retain(|id, _| {
                self.test_tiles
                    .values()
                    .chain(self.recording_history.values())
                    .any(|tiles| tiles.iter().any(|tile| tile.preview_id == *id))
                    || self.attachments.values().any(|ids| ids.contains(id))
                    || engine
                        .flows
                        .values()
                        .any(|flow| flow.captures.iter().any(|capture| capture.id == *id))
            });
            if before != self.terminals.len() + self.apps.len() {
                cx.widget_tree_mark_dirty(self.uid);
            }
        }
        self.area.redraw(cx);
    }

    /// Host the existing terminal tab (or MpTerm itself). This shares its PTY
    /// and scrollback; resizing and redraw never construct another session.
    pub fn set_terminal(&mut self, cx: &mut Cx, flow: String, widget: WidgetRef) {
        if flow.len() > 96 {
            return;
        }
        if widget.is_empty() {
            self.clear_terminal(cx, &flow);
            return;
        }
        if self
            .terminals
            .get(&flow)
            .is_some_and(|host| host.widget.widget_uid() == widget.widget_uid())
        {
            return;
        }
        if let Some(old) = self.terminals.remove(&flow) {
            Self::anchor_terminal(cx, &old.widget, None);
        }
        if !widget.is_empty()
            && self.terminals.len() < 4
            && !self
                .terminals
                .values()
                .chain(self.apps.values())
                .any(|host| host.widget.widget_uid() == widget.widget_uid())
        {
            self.terminals.insert(
                flow,
                LiveHost {
                    widget,
                    draw_list: None,
                    screen: None,
                    clip: None,
                },
            );
        }
        cx.widget_tree_mark_dirty(self.uid);
        self.area.redraw(cx);
    }

    /// Detach only after the host has preserved/stopped the owned provider.
    /// This releases the view and IME references; stored lane history remains.
    pub fn clear_terminal(&mut self, cx: &mut Cx, flow: &str) {
        if let Some(host) = self.terminals.remove(flow) {
            Self::anchor_terminal(cx, &host.widget, None);
            if cx.has_key_focus(host.widget.area()) {
                cx.set_key_focus(Area::Empty);
                cx.hide_text_ime();
            }
            let body = LiveBody::Terminal(flow.to_owned());
            if self.body_capture.as_ref() == Some(&body) {
                self.body_capture = None;
            }
            if self
                .body_touch_capture
                .as_ref()
                .is_some_and(|(_, current)| *current == body)
            {
                self.body_touch_capture = None;
            }
            if self.body_hover.as_ref() == Some(&body) {
                self.body_hover = None;
            }
            cx.widget_tree_mark_dirty(self.uid);
            self.area.redraw(cx);
        }
    }

    /// Exactly one presentation owns the shared terminals' events. The host
    /// enables this only while Flows is visible and excludes their tab IDs
    /// from StudioSurface at the same time. Application timers remain live.
    pub fn set_terminals_enabled(&mut self, cx: &mut Cx, enabled: bool) {
        if !enabled {
            self.cut_flow = None;
            self.cut_hover = None;
        }
        if self.terminals_enabled == enabled {
            return;
        }
        self.terminals_enabled = enabled;
        self.body_capture = None;
        self.body_touch_capture = None;
        self.body_hover = None;
        self.navigation_drag = None;
        self.width_hover = None;
        self.drag = None;
        self.history_drag = None;
        self.pressed = None;
        for host in self.terminals.values() {
            Self::anchor_terminal(cx, &host.widget, None);
            if cx.has_key_focus(host.widget.area()) {
                cx.set_key_focus(Area::Empty);
                cx.hide_text_ime();
            }
        }
        for host in self.apps.values() {
            if let Some(mut app) = host.widget.borrow_mut::<IterationRunView>() {
                app.clear_pointer_hover();
                if !enabled {
                    app.release_keyboard(cx);
                }
            }
        }
        if enabled {
            cx.set_key_focus(self.area);
        } else {
            if cx.has_key_focus(self.area) {
                cx.set_key_focus(Area::Empty);
            }
            if self.pixel_view.is_some() {
                self.close_pixel_view(cx);
            }
        }
        self.area.redraw(cx);
    }

    /// Attach the actual child-process view to the flow's active immutable
    /// artifact. Its expanded build card scrolls with history; folding does
    /// not stop the application or discard its GPU/input state.
    pub fn set_app(&mut self, cx: &mut Cx, flow: String, widget: WidgetRef) {
        if flow.is_empty() || flow.len() > 96 {
            return;
        }
        if self
            .apps
            .get(&flow)
            .is_some_and(|host| host.widget.widget_uid() == widget.widget_uid())
        {
            return;
        }
        if widget.is_empty() {
            self.clear_app(cx, &flow);
            return;
        }
        if (!self.apps.contains_key(&flow) && self.apps.len() >= 4)
            || self
                .terminals
                .values()
                .chain(self.apps.values())
                .any(|host| host.widget.widget_uid() == widget.widget_uid())
        {
            return;
        }
        if let Some(old) = self.apps.remove(&flow) {
            Self::detach_app(cx, &old.widget);
        }
        self.apps.insert(
            flow,
            LiveHost {
                widget,
                draw_list: None,
                screen: None,
                clip: None,
            },
        );
        self.sync_app_artifacts();
        self.rebuild();
        cx.widget_tree_mark_dirty(self.uid);
        self.area.redraw(cx);
    }

    /// Stops the presentation timer and releases input before detaching the
    /// view. Process closure remains owned by the worker's run lifecycle.
    pub fn clear_app(&mut self, cx: &mut Cx, flow: &str) {
        if let Some(host) = self.apps.remove(flow) {
            Self::detach_app(cx, &host.widget);
            self.app_artifacts.remove(flow);
            let body = LiveBody::App(flow.to_owned());
            if self.body_capture.as_ref() == Some(&body) {
                self.body_capture = None;
            }
            if self
                .body_touch_capture
                .as_ref()
                .is_some_and(|(_, current)| *current == body)
            {
                self.body_touch_capture = None;
            }
            if self.body_hover.as_ref() == Some(&body) {
                self.body_hover = None;
            }
            self.rebuild();
            cx.widget_tree_mark_dirty(self.uid);
            self.area.redraw(cx);
        }
    }

    fn sync_app_artifacts(&mut self) {
        let Some(engine) = &self.engine else {
            return;
        };
        for flow in self.apps.keys() {
            let artifact = engine
                .flows
                .get(flow)
                .and_then(|flow| {
                    flow.runs.iter().rev().find(|run| {
                        run.role == RunRole::Human
                            && !run.closed
                            && !run.observation_lost
                            && run.mode == crate::iteration::LaunchMode::Embedded
                    })
                })
                .map(|run| run.artifact_id.clone());
            match artifact {
                Some(artifact) => {
                    if self.app_artifacts.get(flow) != Some(&artifact) {
                        self.expanded.insert(format!("{flow}/{artifact}"));
                        self.app_artifacts.insert(flow.clone(), artifact);
                    }
                }
                None => {
                    self.app_artifacts.remove(flow);
                }
            }
        }
    }

    pub fn terminal_height(&self, flow: &str) -> f64 {
        self.terminal_heights
            .get(flow)
            .copied()
            .unwrap_or(TERMINAL_DEFAULT_HEIGHT)
    }
    pub fn lane_width(&self, flow: &str) -> f64 {
        self.lane_widths.get(flow).copied().unwrap_or(COLUMN_WIDTH)
    }
    /// Widths are independent layout points. Resizing reflows this lane and
    /// resizes its existing terminal; the PTY and its focus are retained.
    pub fn set_lane_width(&mut self, cx: &mut Cx, flow: &str, width: f64) {
        if !width.is_finite()
            || flow.is_empty()
            || flow.len() > 96
            || (!self.lane_widths.contains_key(flow)
                && self.lane_widths.len() >= crate::iteration::MAX_STORED_FLOWS)
        {
            return;
        }
        let width = width.clamp(420.0, 1600.0);
        if self.lane_width(flow) == width {
            return;
        }
        let index = self.lanes.iter().position(|lane| lane.id == flow);
        let anchor = index.and_then(|index| {
            let offset = self.scroll.get(flow).copied().unwrap_or(0.0);
            self.lanes[index]
                .items
                .iter()
                .find(|item| item.y + item.height > offset)
                .map(|item| (item.key.clone(), item.y - offset))
        });
        self.lane_widths.insert(flow.to_owned(), width);
        // Resolve any partial fold at its intended state before text reflows.
        self.fold_animations
            .retain(|_, animation| animation.flow != flow);
        if let Some(index) = index {
            if let Some(flow) = self
                .engine
                .as_ref()
                .and_then(|engine| engine.flows.get(flow))
            {
                self.lanes[index] = self.make_lane(flow);
            }
            let maximum = (self.lanes[index].height - self.body_height(flow)).max(0.0);
            let offset = if self.follow_tail.contains(flow) {
                maximum
            } else {
                anchor
                    .and_then(|(key, relative_y)| {
                        self.lanes[index]
                            .items
                            .iter()
                            .find(|item| item.key == key)
                            .map(|item| item.y - relative_y)
                    })
                    .unwrap_or_else(|| self.scroll.get(flow).copied().unwrap_or(0.0))
                    .clamp(0.0, maximum)
            };
            self.scroll.insert(flow.to_owned(), offset);
            self.scroll_target.insert(flow.to_owned(), offset);
        }
        self.clear_cut_hover();
        self.clear_app_hovers();
        self.fitted = false;
        self.area.redraw(cx);
    }
    fn lane_x(&self, index: usize) -> f64 {
        LEFT + self
            .lanes
            .iter()
            .take(index)
            .map(|lane| self.lane_width(&lane.id) + GAP)
            .sum::<f64>()
    }
    fn lane_width_at(&self, index: usize) -> f64 {
        self.lanes
            .get(index)
            .map(|lane| self.lane_width(&lane.id))
            .unwrap_or(COLUMN_WIDTH)
    }
    fn scene_width(&self) -> f64 {
        LEFT * 2.0
            + if self.lanes.is_empty() {
                COLUMN_WIDTH
            } else {
                self.lanes
                    .iter()
                    .map(|lane| self.lane_width(&lane.id))
                    .sum::<f64>()
                    + GAP * self.lanes.len().saturating_sub(1) as f64
            }
    }
    /// Heights are world/layout points and can be persisted independently from
    /// camera zoom. Invalid settings are ignored; no terminal is restarted.
    pub fn set_terminal_height(&mut self, cx: &mut Cx, flow: &str, height: f64) {
        if !height.is_finite()
            || flow.len() > 96
            || (!self.terminal_heights.contains_key(flow)
                && self.terminal_heights.len() >= crate::iteration::MAX_STORED_FLOWS)
        {
            return;
        }
        let height = height.clamp(80.0, 1200.0);
        if self.terminal_height(flow) != height {
            self.terminal_heights.insert(flow.to_owned(), height);
            self.area.redraw(cx);
        }
    }

    /// All delivered attachments, oldest first. History is independent from
    /// the unsubmitted floating tray, so prompt submission never erases media.
    pub fn set_attachments(&mut self, cx: &mut Cx, flow: String, ids: Vec<String>) {
        if !self
            .engine
            .as_ref()
            .is_some_and(|engine| engine.flows.contains_key(&flow))
        {
            return;
        }
        let accepted = bounded_attachments(ids, 256);
        if self.attachments.get(&flow) != Some(&accepted) {
            self.attachments.insert(flow, accepted);
            self.rebuild();
            self.area.redraw(cx);
        }
    }

    /// Only unsubmitted delivered attachments, oldest first. Empty hides the
    /// tray while set_attachments continues to supply durable image history.
    pub fn set_attachment_tray(&mut self, cx: &mut Cx, flow: String, ids: Vec<String>) {
        if !self
            .engine
            .as_ref()
            .is_some_and(|engine| engine.flows.contains_key(&flow))
        {
            return;
        }
        let accepted = bounded_attachments(ids, 8);
        if self.attachment_tray.get(&flow) != Some(&accepted) {
            let new_image = accepted.iter().any(|id| {
                !self
                    .attachment_tray
                    .get(&flow)
                    .is_some_and(|old| old.contains(id))
            });
            if new_image {
                self.collapsed_trays.remove(&flow);
            }
            self.attachment_tray.insert(flow, accepted);
            self.clear_app_hovers();
            self.area.redraw(cx);
        }
    }

    pub fn set_test_tiles(&mut self, cx: &mut Cx, flow: String, tiles: Vec<TestTile>) {
        if !self
            .engine
            .as_ref()
            .is_some_and(|engine| engine.flows.contains_key(&flow))
        {
            return;
        }
        let accepted = bounded_recordings(tiles, 4);
        if self.test_tiles.get(&flow) != Some(&accepted) {
            let lost = self.pixel_view.as_ref().is_some_and(|(current, id)| {
                *current == flow
                    && !accepted.iter().any(|tile| tile.id == *id)
                    && !self
                        .recording_history
                        .get(&flow)
                        .is_some_and(|tiles| tiles.iter().any(|tile| tile.id == *id))
            });
            if lost {
                self.close_pixel_view(cx);
            }
            self.test_tiles.insert(flow, accepted);
            self.rebuild();
            self.area.redraw(cx);
        }
    }

    /// Complete bounded recording history, newest first. The separate top
    /// strip stays at four entries; all supplied entries remain reachable by
    /// lane scrolling, minimap navigation and original-frame inspection.
    pub fn set_recording_history(&mut self, cx: &mut Cx, flow: String, tiles: Vec<TestTile>) {
        if !self
            .engine
            .as_ref()
            .is_some_and(|engine| engine.flows.contains_key(&flow))
        {
            return;
        }
        let accepted = bounded_recordings(tiles, 256)
            .into_iter()
            .filter(|tile| {
                self.engine.as_ref().is_some_and(|engine| {
                    engine.history_visible(&flow, &format!("recording/{}", tile.id))
                })
            })
            .collect::<Vec<_>>();
        if self.recording_history.get(&flow) != Some(&accepted) {
            let lost = self.pixel_view.as_ref().is_some_and(|(current, id)| {
                *current == flow
                    && !accepted.iter().any(|tile| tile.id == *id)
                    && !self
                        .test_tiles
                        .get(&flow)
                        .is_some_and(|tiles| tiles.iter().any(|tile| tile.id == *id))
            });
            if lost {
                self.close_pixel_view(cx);
            }
            let previous = self.recording_history.get(&flow);
            for tile in &accepted {
                if !previous.is_some_and(|tiles| tiles.iter().any(|old| old.id == tile.id)) {
                    self.expanded
                        .insert(format!("{flow}/recording/{}", tile.id));
                }
            }
            if let Some(previous) = previous {
                for tile in previous {
                    if !accepted.iter().any(|next| next.id == tile.id) {
                        self.expanded
                            .remove(&format!("{flow}/recording/{}", tile.id));
                    }
                }
            }
            self.recording_history.insert(flow, accepted);
            self.rebuild();
            self.area.redraw(cx);
        }
    }

    pub fn set_recording_artifacts(
        &mut self,
        cx: &mut Cx,
        flow: String,
        pairs: Vec<(String, String)>,
    ) {
        if !self
            .engine
            .as_ref()
            .is_some_and(|engine| engine.flows.contains_key(&flow))
        {
            return;
        }
        let pairs: Vec<_> = pairs
            .into_iter()
            .filter(|(id, artifact)| id.len() <= 96 && artifact.len() <= 96)
            .take(256)
            .collect();
        if self.recording_artifacts.get(&flow) != Some(&pairs) {
            self.recording_artifacts.insert(flow, pairs);
            self.rebuild();
            self.area.redraw(cx);
        }
    }

    fn recording_tile(&self, flow: &str, id: &str) -> Option<&TestTile> {
        self.recording_history
            .get(flow)
            .into_iter()
            .flatten()
            .chain(self.test_tiles.get(flow).into_iter().flatten())
            .find(|tile| tile.id == id)
    }

    /// One original frame for the expanded read-only tile. The host loads it
    /// off UI; thumbnail pixels are never upscaled and called pixel accurate.
    pub fn set_full_preview(
        &mut self,
        cx: &mut Cx,
        id: String,
        width: u32,
        height: u32,
        pixels: Arc<Vec<u32>>,
    ) -> Result<(), String> {
        let expected = self
            .pixel_view
            .as_ref()
            .and_then(|(flow, selected)| self.recording_tile(flow, selected));
        if !expected.is_some_and(|tile| {
            tile.preview_id == id && tile.width == width && tile.height == height
        }) {
            return Err("original frame does not match the currently selected test tile".into());
        }
        let count = width as usize * height as usize;
        if count > 16 * 1024 * 1024 || pixels.len() != count {
            return Err(
                "original preview exceeds 16M pixels or has inconsistent dimensions".into(),
            );
        }
        let pixels = Arc::try_unwrap(pixels).unwrap_or_else(|pixels| (*pixels).clone());
        let texture = Texture::new_with_format(
            cx,
            TextureFormat::VecBGRAu8_32 {
                width: width as usize,
                height: height as usize,
                data: Some(pixels),
                updated: TextureUpdated::Full,
            },
        );
        self.full_preview = Some((
            id,
            Preview {
                texture,
                width,
                height,
            },
        ));
        self.area.redraw(cx);
        Ok(())
    }

    /// Pixels are decoded/downsampled by the host worker, packed 0xAARRGGBB.
    pub fn has_preview_source(&self, id: &str) -> bool {
        self.test_tiles
            .values()
            .chain(self.recording_history.values())
            .any(|tiles| tiles.iter().any(|tile| tile.preview_id == id))
            || self
                .attachments
                .values()
                .any(|ids| ids.iter().any(|item| item == id))
            || self.engine.as_ref().is_some_and(|engine| {
                engine
                    .flows
                    .values()
                    .any(|flow| flow.captures.iter().any(|capture| capture.id == id))
            })
    }

    /// Only observed captures or host-acknowledged attachments acquire previews. A recording's poster is
    /// actual decoded media supplied through this same API, never generated.
    pub fn set_preview(
        &mut self,
        cx: &mut Cx,
        id: String,
        width: u32,
        height: u32,
        pixels: Arc<Vec<u32>>,
    ) -> Result<(), String> {
        if !self.has_preview_source(&id) {
            return Err(
                "preview requires an observed capture, test tile or delivered attachment ID".into(),
            );
        }
        let count = width as usize * height as usize;
        if width == 0 || height == 0 || width > 1024 || height > 1024 || pixels.len() != count {
            return Err(
                "preview must have matching pixels and dimensions within 1024 × 1024".into(),
            );
        }
        let retained: usize = self
            .previews
            .iter()
            .filter(|(key, _)| **key != id)
            .map(|(_, preview)| preview.width as usize * preview.height as usize)
            .sum();
        if retained + count > 8 * 1024 * 1024
            || (!self.previews.contains_key(&id) && self.previews.len() >= 256)
        {
            return Err("preview budget is 256 frames / 32 MiB; supply smaller thumbnails".into());
        }
        let pixels = Arc::try_unwrap(pixels).unwrap_or_else(|pixels| (*pixels).clone());
        if let Some(preview) = self.previews.get_mut(&id) {
            preview
                .texture
                .set_data_u32(cx, width as usize, height as usize, pixels);
            preview.width = width;
            preview.height = height;
        } else {
            let texture = Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    width: width as usize,
                    height: height as usize,
                    data: Some(pixels),
                    updated: TextureUpdated::Full,
                },
            );
            self.previews.insert(
                id,
                Preview {
                    texture,
                    width,
                    height,
                },
            );
        }
        self.area.redraw(cx);
        Ok(())
    }

    pub fn select_flow(&mut self, cx: &mut Cx, selected: Option<String>) {
        if selected != self.selected {
            self.selected = selected;
            if let Some(id) = self.selected.clone() {
                self.reveal_flow(cx, &id);
            }
            self.area.redraw(cx);
        }
    }
    pub fn reveal_flow(&mut self, cx: &mut Cx, id: &str) {
        let Some(index) = self.lanes.iter().position(|lane| lane.id == id) else {
            return;
        };
        let x = self.lane_x(index);
        let left = self.pan.x + x * self.zoom.max(0.2);
        let right = left + self.lane_width(id) * self.zoom.max(0.2);
        if right <= 0.0 || left >= self.viewport.size.x {
            self.pan.x = 12.0 - x * self.zoom.max(0.2);
            self.fitted = false;
            self.area.redraw(cx);
        }
    }
    pub fn show_archived_flow(&mut self, cx: &mut Cx, flow: Option<String>) {
        let flow = flow.filter(|id| {
            self.engine.as_ref().is_some_and(|engine| {
                engine
                    .flows
                    .get(id)
                    .is_some_and(|flow| flow.lifecycle == FlowLifecycle::Archived)
            })
        });
        if self.archived_flow == flow {
            return;
        }
        self.archived_flow = flow;
        self.cut_flow = None;
        self.cut_hover = None;
        self.rebuild();
        if let Some(id) = &self.archived_flow {
            if let Some(index) = self.lanes.iter().position(|lane| &lane.id == id) {
                self.pan.x = 12.0 - (self.lane_x(index)) * self.zoom.max(0.2);
                self.fitted = false;
            }
        } else if let Some(id) = self.selected.clone() {
            self.reveal_flow(cx, &id);
        }
        self.area.redraw(cx);
    }
    fn is_archived(&self, id: &str) -> bool {
        self.engine.as_ref().is_some_and(|engine| {
            engine
                .flows
                .get(id)
                .is_some_and(|flow| flow.lifecycle == FlowLifecycle::Archived)
        })
    }
    pub fn fit(&mut self, cx: &mut Cx) {
        self.needs_fit = true;
        self.area.redraw(cx);
    }
    pub fn zoom_by(&mut self, cx: &mut Cx, factor: f64) {
        if factor.is_finite() && factor > 0.0 {
            self.zoom_at(
                cx,
                self.viewport.pos + self.viewport.size * 0.5,
                factor.clamp(0.1, 10.0),
            );
        }
    }

    fn rebuild(&mut self) {
        let Some(engine) = self.engine.as_ref() else {
            self.lanes.clear();
            return;
        };
        // Numeric creation IDs keep established columns stable when flow-10
        // arrives after flow-2; BTreeMap's lexical order would move the latter.
        let mut flows: Vec<_> = engine
            .flows
            .values()
            .filter(|flow| flow.lifecycle != FlowLifecycle::Archived)
            .collect();
        flows.sort_by_key(|flow| {
            flow.id
                .strip_prefix("flow-")
                .and_then(|id| id.parse::<u64>().ok())
                .unwrap_or(u64::MAX)
        });
        let old: BTreeSet<_> = self.lanes.iter().map(|lane| lane.id.clone()).collect();
        self.lanes = flows
            .into_iter()
            .take(4)
            .map(|flow| self.make_lane(flow))
            .collect();
        if let Some(flow) = self
            .archived_flow
            .as_ref()
            .and_then(|id| engine.flows.get(id))
            .filter(|flow| flow.lifecycle == FlowLifecycle::Archived)
        {
            self.lanes.push(self.make_lane(flow));
        }
        if self.cut_flow.as_ref().is_some_and(|id| {
            !engine
                .flows
                .get(id)
                .is_some_and(|flow| flow.lifecycle == FlowLifecycle::Active)
        }) {
            self.cut_flow = None;
            self.cut_hover = None;
        }
        for lane in &self.lanes {
            if !old.contains(&lane.id) {
                self.follow_tail.insert(lane.id.clone());
            }
        }
        self.scroll.retain(|id, _| engine.flows.contains_key(id));
        if self.selected.as_ref().is_some_and(|id| {
            !engine.flows.get(id).is_some_and(|flow| {
                flow.lifecycle != FlowLifecycle::Archived || self.archived_flow.as_ref() == Some(id)
            })
        }) {
            self.selected = None;
        }
        self.fold_animations.retain(|_, animation| {
            self.lanes.iter().any(|lane| {
                lane.id == animation.flow && lane.items.iter().any(|item| item.key == animation.key)
            })
        });
        for animation in self.fold_animations.values_mut() {
            if let Some(item) = self
                .lanes
                .iter()
                .find(|lane| lane.id == animation.flow)
                .and_then(|lane| lane.items.iter().find(|item| item.key == animation.key))
            {
                animation.target = item.clone();
                if item.fold_open == Some(1.0) {
                    animation.content = item.clone();
                }
            }
        }
        self.apply_fold_animations(self.fold_time, false, true);
    }

    fn make_lane(&self, flow: &Flow) -> Lane {
        let text_columns = ((self.lane_width(&flow.id) - 24.0) / 5.5).clamp(32.0, 100.0) as usize;
        let phase = flow
            .job
            .as_ref()
            .map(|job| job.phase.as_str())
            .unwrap_or("ready");
        let mut lane = Lane {
            id: flow.id.clone(),
            title: flow.title.clone(),
            subtitle: if flow.lifecycle == FlowLifecycle::Active {
                phase.replace('_', " ")
            } else {
                flow.lifecycle.as_str().into()
            },
            items: Arc::new(vec![]),
            height: 0.0,
        };
        // Merge observed revisions and feedback by their real event time.
        let events: Vec<_> = self
            .engine
            .as_ref()
            .map(|engine| engine.events(&flow.id).collect())
            .unwrap_or_default();
        let mut history = Vec::<(u64, u8, usize)>::new();
        for (index, artifact) in flow.artifacts.iter().enumerate() {
            if !Engine::artifact_in_use(flow, artifact)
                && !(self.apps.contains_key(&flow.id)
                    && self.app_artifacts.get(&flow.id) == Some(&artifact.id))
                && !self.engine.as_ref().is_some_and(|engine| {
                    engine.history_visible(&flow.id, &format!("artifact/{}", artifact.id))
                })
            {
                continue;
            }
            let at = events
                .iter()
                .find(|event| {
                    event
                        .operation
                        .get("observation")
                        .is_some_and(|observation| {
                            observation.get("kind").and_then(|value| value.as_str())
                                == Some("build_succeeded")
                                && observation
                                    .get("artifact_id")
                                    .and_then(|value| value.as_str())
                                    == Some(artifact.id.as_str())
                        })
                })
                .map(|event| event.at)
                .unwrap_or(0);
            history.push((at, 0, index));
        }
        for (index, record) in flow.feedback.iter().enumerate() {
            if !self.engine.as_ref().is_some_and(|engine| {
                engine.history_visible(&flow.id, &format!("feedback/{}", record.id))
            }) {
                continue;
            }
            let sequence = record
                .id
                .strip_prefix("feedback-")
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);
            history.push((
                events
                    .iter()
                    .find(|event| event.sequence == sequence)
                    .map(|event| event.at)
                    .unwrap_or(0),
                1,
                index,
            ));
        }
        let attachments = self.attachments.get(&flow.id).cloned().unwrap_or_default();
        for (index, id) in attachments.iter().enumerate() {
            if flow.captures.iter().any(|capture| &capture.id == id) {
                continue;
            }
            let at = id
                .split('-')
                .nth(1)
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);
            history.push((at, 2, index));
        }
        let recordings = self
            .recording_history
            .get(&flow.id)
            .cloned()
            .unwrap_or_default();
        for (index, recording) in recordings
            .iter()
            .enumerate()
            .filter(|(_, tile)| !tile.active)
        {
            history.push((recording.recorded_at, 3, index));
        }
        history.sort_by_key(|(at, kind, index)| (*at, *kind, *index));
        for (_, kind, index) in history {
            match kind {
                0 => self.artifact_items(&mut lane, flow, &flow.artifacts[index]),
                1 => {
                    let record = &flow.feedback[index];
                    let expanded = self
                        .expanded
                        .contains(&format!("{}/{}", flow.id, record.id));
                    let caption = if record.from_human {
                        "Feedback"
                    } else {
                        "Agent update"
                    };
                    let (heading, details) =
                        feedback_details(&record.feedback.summary, text_columns);
                    let lines = if expanded { details } else { vec![] };
                    push_item(
                        &mut lane,
                        record.id.clone(),
                        format!("{caption}  ·  {heading}"),
                        lines,
                        Some(IterationViewAction::FoldItem {
                            flow: flow.id.clone(),
                            key: record.id.clone(),
                        }),
                        false,
                    );
                    if expanded {
                        if let Some(item) = Arc::make_mut(&mut lane.items).last_mut() {
                            let extra = item.lines.len() as f64 * 4.0;
                            item.line_height = 18.0;
                            item.height += extra;
                            item.content_height = item.height;
                            lane.height += extra;
                        }
                        if let Some(capture) = record.feedback.evidence.iter().find_map(|id| {
                            flow.captures
                                .iter()
                                .find(|capture| capture.id == id.capture_id)
                        }) {
                            Self::add_preview(&mut lane, capture.id.clone(), 150.0);
                        }
                    }
                }
                3 => {
                    let recording = &recordings[index];
                    let key = format!("recording/{}", recording.id);
                    let expanded = self.expanded.contains(&format!("{}/{}", flow.id, key));
                    let seconds = recording.elapsed_ms / 1000;
                    push_item(
                        &mut lane,
                        key.clone(),
                        format!(
                            "{}  ·  {}:{:02}{}",
                            recording.title,
                            seconds / 60,
                            seconds % 60,
                            if recording.playable {
                                ""
                            } else if recording.complete {
                                " · video unavailable"
                            } else {
                                " · unfinished"
                            }
                        ),
                        vec![],
                        Some(IterationViewAction::FoldItem {
                            flow: flow.id.clone(),
                            key,
                        }),
                        false,
                    );
                    if expanded {
                        Self::add_preview(&mut lane, recording.preview_id.clone(), 90.0);
                        if recording.playable {
                            if let Some(item) = Arc::make_mut(&mut lane.items).last_mut() {
                                item.preview_action = Some(IterationViewAction::PlayRecording {
                                    flow: flow.id.clone(),
                                    id: recording.id.clone(),
                                });
                            }
                        }
                    }
                }
                _ => {
                    let id = &attachments[index];
                    let key = format!("attachment/{id}");
                    let expanded = self.expanded.contains(&format!("{}/{}", flow.id, key));
                    push_item(
                        &mut lane,
                        key.clone(),
                        "Image attached to chat".into(),
                        vec![],
                        Some(IterationViewAction::FoldItem {
                            flow: flow.id.clone(),
                            key,
                        }),
                        false,
                    );
                    if expanded {
                        Self::add_preview(&mut lane, id.clone(), 140.0);
                    }
                }
            }
        }
        if let Some(error) = &flow.workspace_error {
            push_item(
                &mut lane,
                "workspace-error".into(),
                "Workspace needs attention".into(),
                wrap(error, 2, text_columns),
                None,
                true,
            );
        }
        if let Some(job) = &flow.job {
            if !matches!(job.phase, BuildPhase::Succeeded | BuildPhase::Superseded)
                && self.engine.as_ref().is_some_and(|engine| {
                    engine.history_visible(&flow.id, &format!("job/{}", job.id))
                })
            {
                let expanded = self.expanded.contains(&format!("{}/job", flow.id));
                let lines = if expanded {
                    job.error
                        .as_ref()
                        .map(|error| wrap(error, 6, text_columns))
                        .unwrap_or_default()
                } else {
                    vec![]
                };
                let title = if job.phase == BuildPhase::WaitingForClose {
                    "Next build waits for this app to close".into()
                } else {
                    format!("Build  ·  {}", job.phase.as_str().replace('_', " "))
                };
                push_item(
                    &mut lane,
                    "job".into(),
                    title,
                    lines,
                    Some(IterationViewAction::FoldItem {
                        flow: flow.id.clone(),
                        key: "job".into(),
                    }),
                    true,
                );
            }
        }
        let awaiting_launch = flow
            .job
            .as_ref()
            .is_some_and(|job| job.phase == BuildPhase::Succeeded)
            && flow.artifacts.last().is_some_and(|artifact| {
                !flow
                    .runs
                    .iter()
                    .any(|run| run.role == RunRole::Human && run.artifact_id == artifact.id)
            });
        let can_build = flow.lifecycle == FlowLifecycle::Active
            && !flow
                .runs
                .iter()
                .any(|run| run.role == RunRole::AiTest && !run.closed)
            && !awaiting_launch
            && flow.worktree.is_some()
            && flow.prepared.as_ref().is_some_and(|prepared| {
                prepared.source_revision == flow.source_revision
                    && prepared.requirements_revision == flow.requirements_revision
            })
            && !flow.job.as_ref().is_some_and(|job| {
                matches!(
                    job.phase,
                    BuildPhase::WaitingForClose
                        | BuildPhase::CheckpointRequested
                        | BuildPhase::Ready
                        | BuildPhase::Building
                )
            });
        if can_build {
            push_item(
                &mut lane,
                "source".into(),
                "Next revision".into(),
                vec![],
                None,
                false,
            );
            if let Some(item) = Arc::make_mut(&mut lane.items).last_mut() {
                item.controls = vec![
                    FrameControl {
                        label: "Build",
                        action: IterationViewAction::BuildEmbedded {
                            flow: flow.id.clone(),
                            source_revision: flow.source_revision,
                        },
                    },
                    FrameControl {
                        label: "Standalone",
                        action: IterationViewAction::BuildStandalone {
                            flow: flow.id.clone(),
                            source_revision: flow.source_revision,
                        },
                    },
                ];
            }
        }
        if !flow.todos.is_empty() {
            let mut pills = vec![];
            let (mut x, mut y) = (28.0, 4.0);
            let expanded = self.expanded.contains(&format!("{}/tasks", flow.id));
            for todo in flow.todos.iter().take(if expanded { 64 } else { 8 }) {
                let label = short(&todo.text, 42);
                let width = (label.chars().count() as f64 * 6.5 + 32.0).clamp(90.0, 300.0);
                if x + width > self.lane_width(&flow.id) - 8.0 {
                    x = 28.0;
                    y += 25.0;
                }
                pills.push(TodoPill {
                    label,
                    state: todo.state,
                    rect: rect(x, y, width, 21.0),
                });
                x += width + 5.0;
            }
            let height = y + 25.0;
            let lines = if expanded {
                flow.todos
                    .iter()
                    .flat_map(|todo| {
                        bullet_lines(
                            &format!("{} — {}", todo.text, todo.state.label()),
                            text_columns,
                        )
                    })
                    .collect()
            } else {
                vec![]
            };
            let extra = if expanded {
                lines.len() as f64 * 18.0 + 16.0
            } else {
                0.0
            };
            Arc::make_mut(&mut lane.items).push(Item {
                key: "tasks".into(),
                y: lane.height,
                height: height + extra,
                content_height: height + extra,
                fold_open: None,
                title: String::new(),
                lines,
                line_height: 18.0,
                action: Some(IterationViewAction::FoldItem {
                    flow: flow.id.clone(),
                    key: "tasks".into(),
                }),
                emphasized: false,
                preview: None,
                preview_action: None,
                preview_height: 0.0,
                live_app: false,
                pills,
                controls: vec![],
            });
            lane.height += height + extra + 10.0;
        }
        for item in Arc::make_mut(&mut lane.items) {
            if flow.lifecycle == FlowLifecycle::Archived {
                item.controls.retain(|control| {
                    matches!(
                        control.action,
                        IterationViewAction::FoldBuild { .. }
                            | IterationViewAction::InspectCode { .. }
                            | IterationViewAction::PlayRecording { .. }
                            | IterationViewAction::OpenImage { .. }
                            | IterationViewAction::TestTileToggled { .. }
                    )
                });
            }
            let key = match &item.action {
                Some(IterationViewAction::FoldItem { flow, key }) => Some(format!("{flow}/{key}")),
                Some(IterationViewAction::FoldBuild { flow, artifact }) => {
                    Some(format!("{flow}/{artifact}"))
                }
                _ => None,
            };
            if let Some(key) = key {
                item.fold_open = Some(if self.expanded.contains(&key) {
                    1.0
                } else {
                    0.0
                });
            }
        }
        lane
    }

    fn add_preview(lane: &mut Lane, preview: String, height: f64) {
        if let Some(item) = Arc::make_mut(&mut lane.items).last_mut() {
            item.preview = Some(preview);
            item.preview_height = height;
            item.height += height + 8.0;
            item.content_height = item.height;
        }
        lane.height += height + 8.0;
    }

    fn artifact_items(&self, lane: &mut Lane, flow: &Flow, artifact: &Artifact) {
        let key = format!("{}/{}", flow.id, artifact.id);
        let expanded = self.expanded.contains(&key);
        let live_app = self.apps.contains_key(&flow.id)
            && self.app_artifacts.get(&flow.id) == Some(&artifact.id);
        let run = flow
            .runs
            .iter()
            .rev()
            .find(|run| run.artifact_id == artifact.id && run.role == RunRole::Human);
        let number = flow
            .artifacts
            .iter()
            .position(|item| item.id == artifact.id)
            .unwrap_or(0)
            + 1;
        let state = if live_app {
            "Running"
        } else if run.is_some_and(|run| run.observation_lost) {
            "Connection lost"
        } else if run.is_some_and(|run| !run.closed) {
            "Standalone"
        } else {
            "Recorded"
        };
        let tile = self
            .recording_history
            .get(&flow.id)
            .into_iter()
            .flatten()
            .find(|tile| {
                self.recording_artifacts.get(&flow.id).is_some_and(|pairs| {
                    pairs
                        .iter()
                        .any(|(id, aid)| id == &tile.id && aid == &artifact.id)
                })
            });
        let mut lines = vec![];
        if expanded && !live_app {
            lines.push(format!(
                "{} · {} · {}",
                artifact.mode.as_str(),
                short(&artifact.commit, 12),
                if run.is_some_and(|run| run.closed) {
                    "closed"
                } else {
                    "retained build"
                }
            ));
        }
        push_item(
            lane,
            key,
            format!("Revision {number}  ·  {state}"),
            lines,
            Some(IterationViewAction::FoldBuild {
                flow: flow.id.clone(),
                artifact: artifact.id.clone(),
            }),
            false,
        );
        if let Some(item) = Arc::make_mut(&mut lane.items).last_mut() {
            if let Some(run) = run.filter(|run| !run.closed || !run.human_requested) {
                item.controls.push(FrameControl {
                    label: "Close",
                    action: IterationViewAction::CloseFreeze {
                        flow: flow.id.clone(),
                        run_id: run.id.clone(),
                    },
                });
                if !run.closed
                    && !run.observation_lost
                    && run.mode == crate::iteration::LaunchMode::Embedded
                {
                    item.controls.push(FrameControl {
                        label: "Pop out",
                        action: IterationViewAction::PopOut {
                            flow: flow.id.clone(),
                            run_id: run.id.clone(),
                        },
                    });
                }
            }
            if let Some(tile) = tile.filter(|tile| !tile.active && tile.playable) {
                item.preview_action = Some(IterationViewAction::PlayRecording {
                    flow: flow.id.clone(),
                    id: tile.id.clone(),
                });
            }
            if live_app && expanded {
                item.live_app = true;
                item.height += APP_HEIGHT + 8.0;
                item.content_height = item.height;
            }
        }
        if live_app && expanded {
            lane.height += APP_HEIGHT + 8.0;
        } else if expanded {
            if let Some(preview) = tile.map(|tile| tile.preview_id.clone()).or_else(|| {
                flow.captures
                    .iter()
                    .rev()
                    .find(|capture| capture.artifact_id == artifact.id)
                    .map(|capture| capture.id.clone())
            }) {
                Self::add_preview(lane, preview, 230.0);
            }
        }
    }

    // Vertical motion belongs to each lane's history. The terminal and its
    // caption are pinned to the viewport; horizontal pan/zoom are shared.
    fn screen(&self, world: Rect) -> Rect {
        Rect {
            pos: self.viewport.pos + dvec2(self.pan.x, 0.0) + world.pos * self.zoom,
            size: world.size * self.zoom,
        }
    }
    fn terminal_top(&self, flow: &str) -> f64 {
        (self.viewport.size.y / self.zoom.max(0.01)
            - self.visible_terminal_height(flow)
            - TERMINAL_PADDING * 2.0
            - 2.0)
            .max(44.0)
    }
    fn visible_terminal_height(&self, flow: &str) -> f64 {
        self.terminal_height(flow)
            .min((self.viewport.size.y / self.zoom.max(0.01) - 100.0).max(40.0))
    }
    fn body_top(&self, _flow: &str) -> f64 {
        36.0
    }
    fn body_height(&self, flow: &str) -> f64 {
        if self.is_archived(flow) {
            return (self.viewport.size.y / self.zoom.max(0.01) - self.body_top(flow) - 2.0)
                .max(0.0);
        }
        (self.terminal_top(flow) - self.body_top(flow)).max(0.0)
    }
    fn history_top(&self, index: usize) -> f64 {
        let lane = &self.lanes[index];
        self.body_top(&lane.id) + (self.body_height(&lane.id) - lane.height).max(0.0)
    }
    fn tray_rect(&self, index: usize) -> Option<Rect> {
        let lane = self.lanes.get(index)?;
        if self.is_archived(&lane.id) {
            return None;
        }
        let count = self.attachment_tray.get(&lane.id)?.len();
        if count == 0 {
            return None;
        }
        let collapsed = self.collapsed_trays.contains(&lane.id);
        let width = if collapsed {
            84.0
        } else {
            (count.min(4) as f64 * 58.0 + 12.0).max(96.0)
        };
        let height = if collapsed {
            22.0
        } else {
            21.0 + count.div_ceil(4) as f64 * 43.0
        };
        Some(rect(
            self.lane_x(index) + self.lane_width(&lane.id) - width - 96.0,
            self.terminal_top(&lane.id) - height - 12.0,
            width,
            height,
        ))
    }
    fn tile_strip_rect(&self, index: usize) -> Option<Rect> {
        let lane = self.lanes.get(index)?;
        if self.is_archived(&lane.id) {
            return None;
        }
        let count = self
            .test_tiles
            .get(&lane.id)?
            .iter()
            .filter(|tile| tile.active)
            .count();
        if count == 0 {
            return None;
        }
        let width = count as f64 * 90.0 + 8.0;
        let top = self.body_top(&lane.id) + 4.0;
        Some(rect(
            self.lane_x(index) + self.lane_width(&lane.id) - width - 16.0,
            top,
            width,
            66.0,
        ))
    }
    fn over_tray(&self, abs: DVec2) -> bool {
        self.lanes.iter().enumerate().any(|(index, _)| {
            self.lane_clip(index).contains(abs)
                && [self.tray_rect(index), self.tile_strip_rect(index)]
                    .into_iter()
                    .flatten()
                    .any(|world| self.screen(world).contains(abs))
        })
    }
    fn lane_clip(&self, index: usize) -> Rect {
        let id = &self.lanes[index].id;
        self.screen(rect(
            self.lane_x(index),
            self.body_top(id),
            self.lane_width(id),
            self.body_height(id),
        ))
    }
    fn terminal_rect(&self, index: usize) -> Rect {
        let id = &self.lanes[index].id;
        rect(
            self.lane_x(index),
            self.terminal_top(id),
            self.lane_width(id),
            self.visible_terminal_height(id) + TERMINAL_PADDING * 2.0,
        )
    }
    fn width_grip_at(&self, abs: DVec2) -> Option<String> {
        if !self.viewport.contains(abs) {
            return None;
        }
        self.lanes
            .iter()
            .enumerate()
            .filter_map(|(index, lane)| {
                let edge = self
                    .screen(rect(
                        self.lane_x(index) + self.lane_width(&lane.id) + 7.0,
                        0.0,
                        0.0,
                        0.0,
                    ))
                    .pos
                    .x;
                let distance = (abs.x - edge).abs();
                (distance <= 5.0).then_some((distance, lane.id.clone()))
            })
            .min_by(|(left, _), (right, _)| left.total_cmp(right))
            .map(|(_, id)| id)
    }
    fn grip_at(&self, abs: DVec2) -> Option<String> {
        if !self.viewport.contains(abs) {
            return None;
        }
        self.lanes
            .iter()
            .enumerate()
            .find(|(index, _)| {
                if self.is_archived(&self.lanes[*index].id) {
                    return false;
                }
                let terminal = self.screen(self.terminal_rect(*index));
                rect(
                    terminal.pos.x,
                    terminal.pos.y - 3.0 * self.zoom,
                    terminal.size.x,
                    (9.0 * self.zoom).max(8.0),
                )
                .contains(abs)
            })
            .map(|(_, lane)| lane.id.clone())
    }
    fn terminal_at(&self, abs: DVec2) -> Option<String> {
        if !self.terminals_enabled
            || !self.viewport.contains(abs)
            || self.grip_at(abs).is_some()
            || self.width_grip_at(abs).is_some()
        {
            return None;
        }
        self.lanes
            .iter()
            .enumerate()
            .find(|(index, lane)| {
                self.terminals.contains_key(&lane.id)
                    && self.screen(self.terminal_rect(*index)).contains(abs)
            })
            .map(|(_, lane)| lane.id.clone())
    }
    fn app_rect(&self, index: usize) -> Option<Rect> {
        let lane = self.lanes.get(index)?;
        let item = lane.items.iter().find(|item| item.live_app)?;
        let offset = self.scroll.get(&lane.id).copied().unwrap_or(0.0);
        Some(rect(
            self.lane_x(index) + 8.0,
            self.history_top(index) + item.y - offset + item.content_height - APP_HEIGHT - 8.0,
            self.lane_width(&lane.id) - 16.0,
            APP_HEIGHT,
        ))
    }
    fn body_at(&self, abs: DVec2) -> Option<LiveBody> {
        if self.width_grip_at(abs).is_some() {
            return None;
        }
        if let Some(flow) = self.terminal_at(abs) {
            return Some(LiveBody::Terminal(flow));
        }
        if self.cut_flow.as_ref().is_some_and(|flow| {
            self.lanes
                .iter()
                .position(|lane| &lane.id == flow)
                .is_some_and(|index| self.lane_clip(index).contains(abs))
        }) {
            return None;
        }
        if !self.viewport.contains(abs)
            || self.over_tray(abs)
            || self.mini_at(abs).is_some()
            || self.scrollbar_at(abs).is_some()
        {
            return None;
        }
        self.lanes
            .iter()
            .enumerate()
            .find(|(index, lane)| {
                self.apps
                    .get(&lane.id)
                    .is_some_and(|host| host.clip.is_some_and(|clip| clip.contains(abs)))
                    && self.lane_clip(*index).contains(abs)
                    && self
                        .app_rect(*index)
                        .is_some_and(|world| self.screen(world).contains(abs))
            })
            .map(|(_, lane)| LiveBody::App(lane.id.clone()))
    }
    fn host(&self, body: &LiveBody) -> Option<&LiveHost> {
        match body {
            LiveBody::Terminal(flow) => self.terminals.get(flow),
            LiveBody::App(flow) => self.apps.get(flow),
        }
    }
    fn host_mut(&mut self, body: &LiveBody) -> Option<&mut LiveHost> {
        match body {
            LiveBody::Terminal(flow) => self.terminals.get_mut(flow),
            LiveBody::App(flow) => self.apps.get_mut(flow),
        }
    }
    fn camera(&self) -> CanvasCamera {
        let mut camera = CanvasCamera::default();
        camera.view = self.viewport;
        camera.pan = dvec2(self.pan.x, 0.0);
        camera.scale = self.zoom.max(0.01);
        camera
    }
    fn anchor_terminal(
        cx: &mut Cx,
        widget: &WidgetRef,
        anchor: Option<(Area, PopupAnchorTransform)>,
    ) {
        if let Some(mut term) = widget.borrow_mut::<MpTerm>() {
            term.set_background_dimming(cx, if anchor.is_some() { 0.42 } else { 0.0 });
            term.set_presentation_font_scale(cx, if anchor.is_some() { 0.8 } else { 1.0 });
            if anchor.is_none() {
                term.set_presentation_frame(cx, None);
            }
            term.canvas_ime_anchor = anchor;
        } else if let Some(mut term) = widget.widget(cx, ids!(term)).borrow_mut::<MpTerm>() {
            term.set_background_dimming(cx, if anchor.is_some() { 0.42 } else { 0.0 });
            term.set_presentation_font_scale(cx, if anchor.is_some() { 0.8 } else { 1.0 });
            if anchor.is_none() {
                term.set_presentation_frame(cx, None);
            }
            term.canvas_ime_anchor = anchor;
        }
    }
    fn terminal_focused(cx: &mut Cx, widget: &WidgetRef) -> bool {
        if let Some(term) = widget.borrow::<MpTerm>() {
            return term.has_input_focus(cx);
        }
        let child = widget.widget(cx, ids!(term));
        let focused = child
            .borrow::<MpTerm>()
            .is_some_and(|term| term.has_input_focus(cx));
        focused
    }
    fn detach_app(cx: &mut Cx, widget: &WidgetRef) {
        if let Some(mut app) = widget.borrow_mut::<IterationRunView>() {
            app.canvas_ime_anchor = None;
            app.release_keyboard(cx);
            app.clear_run_target(cx);
        }
    }
    fn clear_app_hovers(&self) {
        for host in self.apps.values() {
            if let Some(mut app) = host.widget.borrow_mut::<IterationRunView>() {
                app.clear_pointer_hover();
            }
        }
    }
    fn minimap_rect(&self, index: usize) -> Option<Rect> {
        if self.minimap_open.as_ref() != Some(&self.lanes[index].id) {
            return None;
        }
        let clip = self.lane_clip(index);
        if clip.size.x < MINI_WIDTH + 26.0 || clip.size.y < MINI_HEIGHT + 30.0 {
            return None;
        }
        Some(rect(
            clip.pos.x + 6.0,
            clip.pos.y + 8.0,
            MINI_WIDTH,
            MINI_HEIGHT,
        ))
    }
    fn mini_at(&self, abs: DVec2) -> Option<String> {
        self.lanes
            .iter()
            .enumerate()
            .find(|(index, _)| {
                self.minimap_rect(*index)
                    .is_some_and(|mini| mini.contains(abs))
            })
            .map(|(_, lane)| lane.id.clone())
    }
    fn scrollbar(&self, index: usize) -> Option<(Rect, Rect)> {
        let lane = &self.lanes[index];
        let body = self.body_height(&lane.id);
        let clip = self.lane_clip(index);
        if body <= 0.0 || lane.height <= body {
            return None;
        }
        let track = rect(clip.pos.x + clip.size.x - 8.0, clip.pos.y, 8.0, clip.size.y);
        let thumb_height =
            (track.size.y * body / lane.height).clamp(18.0_f64.min(track.size.y), track.size.y);
        let offset = self.scroll.get(&lane.id).copied().unwrap_or(0.0);
        let y = (offset / (lane.height - body)).clamp(0.0, 1.0) * (track.size.y - thumb_height);
        Some((
            track,
            rect(track.pos.x, track.pos.y + y, track.size.x, thumb_height),
        ))
    }
    fn scrollbar_at(&self, abs: DVec2) -> Option<String> {
        self.lanes
            .iter()
            .enumerate()
            .find(|(index, _)| {
                self.scrollbar(*index)
                    .is_some_and(|(track, _)| track.contains(abs))
            })
            .map(|(_, lane)| lane.id.clone())
    }
    fn set_scroll(&mut self, id: &str, offset: f64, smooth: bool, cx: &mut Cx) {
        if self.cut_flow.as_deref() == Some(id) {
            self.clear_cut_hover();
        }
        for animation in self
            .fold_animations
            .values_mut()
            .filter(|animation| animation.flow == id)
        {
            animation.header_anchor = None;
        }
        let maximum = self
            .lanes
            .iter()
            .find(|lane| lane.id == id)
            .map(|lane| (lane.height - self.body_height(id)).max(0.0))
            .unwrap_or(0.0);
        let offset = offset.clamp(0.0, maximum);
        if offset >= maximum - 2.0 {
            self.follow_tail.insert(id.to_owned());
        } else {
            self.follow_tail.remove(id);
        }
        self.scroll_target.insert(id.to_owned(), offset);
        if smooth {
            self.scroll_frame = cx.new_next_frame();
        } else {
            self.scroll.insert(id.to_owned(), offset);
        }
        self.area.redraw(cx);
    }
    fn drag_history(&mut self, cx: &mut Cx, id: &str, offset: f64) {
        if self.cut_flow.as_deref() == Some(id) {
            self.clear_cut_hover();
        }
        for animation in self
            .fold_animations
            .values_mut()
            .filter(|animation| animation.flow == id)
        {
            animation.header_anchor = None;
        }
        self.follow_tail.remove(id);
        let maximum = self
            .lanes
            .iter()
            .find(|lane| lane.id == id)
            .map(|lane| (lane.height - self.body_height(id)).max(0.0))
            .unwrap_or(0.0);
        let bound = offset.clamp(0.0, maximum);
        let excess = offset - bound;
        // A bounded rubber band leaves the pinned terminal in place; only
        // history moves, then converges back to the newest/oldest edge.
        let limit = 56.0 / self.zoom.max(0.2);
        let elastic = excess / (1.0 + excess.abs() / limit);
        self.scroll_target.remove(id);
        self.scroll.insert(id.to_owned(), bound + elastic);
        self.area.redraw(cx);
    }
    fn release_history_drag(&mut self, cx: &mut Cx) {
        if let Some((id, _)) = self.history_drag.take() {
            let offset = self.scroll.get(&id).copied().unwrap_or(0.0);
            self.set_scroll(&id, offset, true, cx);
        }
    }
    fn navigate_minimap(&mut self, cx: &mut Cx, id: &str, abs: DVec2) {
        let Some(index) = self.lanes.iter().position(|lane| lane.id == id) else {
            return;
        };
        let Some(mini) = self.minimap_rect(index) else {
            return;
        };
        let offset = ((abs.y - mini.pos.y) / mini.size.y).clamp(0.0, 1.0)
            * self.lanes[index].height
            - self.body_height(id) * 0.5;
        self.set_scroll(id, offset, false, cx);
    }
    fn zoom_at(&mut self, cx: &mut Cx, abs: DVec2, factor: f64) {
        let old_zoom = self.zoom.max(0.01);
        let world_x = (abs.x - self.viewport.pos.x - self.pan.x) / old_zoom;
        let lane = self.lane_at(abs);
        let anchor = lane.as_ref().map(|id| {
            self.scroll.get(id).copied().unwrap_or(0.0) + (abs.y - self.viewport.pos.y) / old_zoom
                - self.body_top(id)
        });
        self.zoom = (self.zoom.max(0.25) * factor).clamp(0.20, 3.0);
        self.pan.x = abs.x - self.viewport.pos.x - world_x * self.zoom;
        self.pan.y = 0.0;
        if let (Some(id), Some(anchor)) = (lane, anchor) {
            if abs.y >= self.screen(rect(0.0, self.body_top(&id), 0.0, 0.0)).pos.y {
                let offset =
                    anchor - (abs.y - self.viewport.pos.y) / self.zoom + self.body_top(&id);
                self.set_scroll(&id, offset, false, cx);
            }
        }
        self.fitted = false;
        self.area.redraw(cx);
    }
    fn lane_at(&self, abs: DVec2) -> Option<String> {
        if !self.viewport.contains(abs) {
            return None;
        }
        self.lanes
            .iter()
            .enumerate()
            .find(|(index, _)| {
                let body = self.lane_clip(*index);
                let top = self.screen(rect(0.0, TERMINAL_TOP, 0.0, 0.0)).pos.y;
                rect(
                    body.pos.x,
                    top,
                    body.size.x,
                    self.viewport.pos.y + self.viewport.size.y - top,
                )
                .contains(abs)
            })
            .map(|(_, lane)| lane.id.clone())
    }
    fn drop_flow(&self, abs: DVec2) -> Option<String> {
        self.pixel_view
            .as_ref()
            .map(|(flow, _)| flow.clone())
            .or_else(|| self.lane_at(abs))
            .filter(|flow| !self.is_archived(flow))
    }
    fn clear_cut_hover(&mut self) {
        self.cut_hover = None;
        if matches!(self.hovered, Some(IterationViewAction::SplitFlow { .. })) {
            self.hovered = None;
        }
    }
    fn hit_at(&self, abs: DVec2) -> Option<IterationViewAction> {
        if !self.viewport.contains(abs)
            || self.mini_at(abs).is_some()
            || self.width_grip_at(abs).is_some()
        {
            return None;
        }
        if let Some((flow, before)) = self.split_at(abs) {
            return Some(IterationViewAction::SplitFlow { flow, before });
        }
        self.targets
            .iter()
            .rev()
            .find(|target| target.rect.contains(abs))
            .map(|target| target.action.clone())
    }

    fn history_key(&self, lane: &Lane, item: &Item) -> Option<String> {
        let engine = self.engine.as_ref()?;
        let flow = engine.flows.get(&lane.id)?;
        let key = if item.key.starts_with("attachment/") || item.key.starts_with("recording/") {
            item.key.clone()
        } else if let Some(feedback) = flow
            .feedback
            .iter()
            .find(|feedback| feedback.id == item.key)
        {
            format!("feedback/{}", feedback.id)
        } else if let Some(artifact) = flow
            .artifacts
            .iter()
            .find(|artifact| item.key == format!("{}/{}", lane.id, artifact.id))
        {
            format!("artifact/{}", artifact.id)
        } else {
            return None;
        };
        engine.history_visible(&lane.id, &key).then_some(key)
    }

    fn split_at(&self, abs: DVec2) -> Option<(String, String)> {
        let flow = self.cut_flow.as_ref()?;
        if !self.viewport.contains(abs)
            || self.over_tray(abs)
            || self.mini_at(abs).is_some()
            || self.scrollbar_at(abs).is_some()
            || self.width_grip_at(abs).is_some()
        {
            return None;
        }
        let index = self.lanes.iter().position(|lane| &lane.id == flow)?;
        if !self.lane_clip(index).contains(abs) {
            return None;
        }
        let lane = &self.lanes[index];
        let offset = self.scroll.get(flow).copied().unwrap_or(0.0);
        let y =
            (abs.y - self.viewport.pos.y) / self.zoom.max(0.01) - self.history_top(index) + offset;
        let item = lane
            .items
            .iter()
            .find(|item| y >= item.y - 5.0 && y < item.y + item.height)?;
        Some((flow.clone(), self.history_key(lane, item)?))
    }

    fn draw_split_marker(&mut self, cx: &mut Cx2d, index: usize) {
        let lane = &self.lanes[index];
        if self.cut_flow.as_ref() != Some(&lane.id) {
            return;
        }
        let Some(key) = &self.cut_hover else {
            return;
        };
        let Some(item) = lane
            .items
            .iter()
            .find(|item| self.history_key(lane, item).as_ref() == Some(key))
        else {
            return;
        };
        let offset = self.scroll.get(&lane.id).copied().unwrap_or(0.0);
        let y = (self.history_top(index) + item.y - offset - 4.0).max(self.body_top(&lane.id));
        let x = self.lane_x(index);
        let width = self.lane_width_at(index);
        let line = self.screen(rect(x + 3.0, y, width - 6.0, 0.0));
        self.draw_shape.color = self.accent;
        self.draw_shape
            .draw_abs(cx, rect(line.pos.x, line.pos.y, line.size.x, 1.5));
        let label = rect(x + width - 94.0, y, 90.0, 19.0);
        self.draw_shape.color = self.background;
        let label_screen = self.screen(label);
        self.draw_shape.draw_abs(cx, label_screen);
        self.text(
            cx,
            rect(label.pos.x + 6.0, y + 3.0, 80.0, 15.0),
            "Split here",
            false,
            self.accent,
        );
    }

    fn fold(&mut self, cx: &mut Cx, flow: &str, artifact: &str) {
        self.clear_app_hovers();
        let key = format!("{flow}/{artifact}");
        let before = self
            .lanes
            .iter()
            .find(|lane| lane.id == flow)
            .and_then(|lane| {
                lane.items
                    .iter()
                    .find(|item| item.key == key || item.key == artifact)
            })
            .cloned();
        let body_height = self.body_height(flow);
        let header_before = before.as_ref().and_then(|item| {
            self.lanes.iter().find(|lane| lane.id == flow).map(|lane| {
                let slack = (body_height - lane.height).max(0.0);
                let offset = self.scroll.get(flow).copied().unwrap_or(0.0);
                (slack + item.y - offset).clamp(0.0, (body_height - 28.0).max(0.0))
            })
        });
        let opening = !self.expanded.contains(&key);
        if opening {
            // Revealing a card is navigation. Following the lane's end here
            // would pull a tall card's header behind the fixed lane caption.
            self.follow_tail.remove(flow);
            for animation in self
                .fold_animations
                .values_mut()
                .filter(|animation| animation.flow == flow)
            {
                animation.header_anchor = None;
            }
        }
        // A second click reverses from the displayed height and angle, without
        // first snapping to the previous animation's destination.
        self.fold_animations.remove(&key);
        if !self.expanded.remove(&key) {
            self.expanded.insert(key.clone());
        }
        self.rebuild();
        let after = self
            .lanes
            .iter()
            .find(|lane| lane.id == flow)
            .and_then(|lane| {
                lane.items
                    .iter()
                    .find(|item| item.key == key || item.key == artifact)
            })
            .cloned();
        if let (Some(before), Some(after)) = (before, after) {
            let after_y = after.y;
            *self.scroll.entry(flow.to_owned()).or_default() += after.y - before.y;
            let header_anchor = opening
                .then(|| {
                    header_before
                        .map(|from| (from, from.min((body_height - after.height).max(0.0))))
                })
                .flatten();
            let mut animated = false;
            if let (Some(from_open), Some(to_open)) = (before.fold_open, after.fold_open) {
                // Animate the shared header and clipped content for every
                // card, including a live revision's separately hosted body.
                if self.fold_animations.len() < 32 {
                    self.fold_time = cx.seconds_since_app_start();
                    let content = if to_open > 0.5 {
                        after.clone()
                    } else {
                        before.clone()
                    };
                    self.fold_animations.insert(
                        key,
                        FoldAnimation {
                            flow: flow.to_owned(),
                            key: after.key.clone(),
                            started: self.fold_time,
                            from_height: before.height,
                            from_open,
                            header_anchor,
                            target: after,
                            content,
                        },
                    );
                    self.apply_fold_animations(self.fold_time, false, true);
                    self.fold_frame = cx.new_next_frame();
                    animated = true;
                }
            }
            if !animated {
                if let Some((_, header)) = header_anchor {
                    if let Some(lane) = self.lanes.iter().find(|lane| lane.id == flow) {
                        let slack = (body_height - lane.height).max(0.0);
                        let maximum = (lane.height - body_height).max(0.0);
                        let offset = (slack + after_y - header).clamp(0.0, maximum);
                        self.scroll.insert(flow.to_owned(), offset);
                    }
                }
            }
        }
        if let Some(lane) = self.lanes.iter().find(|lane| lane.id == flow) {
            let maximum = (lane.height - self.body_height(flow)).max(0.0);
            if let Some(offset) = self.scroll.get_mut(flow) {
                *offset = offset.clamp(0.0, maximum);
                self.scroll_target.insert(flow.to_owned(), *offset);
            }
        }
    }

    fn apply_fold_animations(&mut self, now: f64, preserve_anchor: bool, refresh_content: bool) {
        if self.fold_animations.is_empty() {
            return;
        }
        let mut finished = Vec::new();
        let body_heights: BTreeMap<_, _> = self
            .lanes
            .iter()
            .map(|lane| (lane.id.clone(), self.body_height(&lane.id)))
            .collect();
        for lane in &mut self.lanes {
            if !self
                .fold_animations
                .values()
                .any(|animation| animation.flow == lane.id)
            {
                continue;
            }
            let offset = self.scroll.get(&lane.id).copied().unwrap_or(0.0);
            let header_anchor = self
                .fold_animations
                .values()
                .filter(|animation| animation.flow == lane.id && animation.header_anchor.is_some())
                .max_by(|a, b| a.started.total_cmp(&b.started))
                .map(|animation| {
                    let (from, to) = animation.header_anchor.unwrap();
                    let t = ((now - animation.started) / FOLD_DURATION).clamp(0.0, 1.0);
                    let eased = t * t * (3.0 - 2.0 * t);
                    (animation.key.clone(), from + (to - from) * eased)
                });
            let anchor = (preserve_anchor
                && header_anchor.is_none()
                && !self.follow_tail.contains(&lane.id)
                && offset > 0.0)
                .then(|| {
                    lane.items
                        .iter()
                        .find(|item| item.y + item.height > offset)
                        .map(|item| (item.key.clone(), item.y))
                })
                .flatten();
            let mut delta = 0.0;
            for item in Arc::make_mut(&mut lane.items) {
                let y = item.y + delta;
                if let Some((id, animation)) = self
                    .fold_animations
                    .iter()
                    .find(|(_, animation)| animation.flow == lane.id && animation.key == item.key)
                {
                    let t = ((now - animation.started) / FOLD_DURATION).clamp(0.0, 1.0);
                    let eased = t * t * (3.0 - 2.0 * t);
                    let old_height = item.height;
                    if t >= 1.0 {
                        *item = animation.target.clone();
                        finished.push(id.clone());
                    } else {
                        if refresh_content {
                            *item = animation.content.clone();
                        }
                        item.height = animation.from_height
                            + (animation.target.height - animation.from_height) * eased;
                        item.fold_open = Some(
                            animation.from_open
                                + (animation.target.fold_open.unwrap_or(0.0) - animation.from_open)
                                    * eased,
                        );
                    }
                    delta += item.height - old_height;
                }
                item.y = y;
            }
            lane.height += delta;
            if let Some((key, before)) = anchor {
                if let Some(item) = lane.items.iter().find(|item| item.key == key) {
                    let adjustment = item.y - before;
                    *self.scroll.entry(lane.id.clone()).or_default() += adjustment;
                    if let Some(target) = self.scroll_target.get_mut(&lane.id) {
                        *target += adjustment;
                    }
                }
            }
            let maximum = (lane.height - body_heights[&lane.id]).max(0.0);
            if let Some((key, header)) = header_anchor {
                if let Some(item) = lane.items.iter().find(|item| item.key == key) {
                    let slack = (body_heights[&lane.id] - lane.height).max(0.0);
                    let offset = (slack + item.y - header.max(0.0)).clamp(0.0, maximum);
                    self.follow_tail.remove(&lane.id);
                    self.scroll.insert(lane.id.clone(), offset);
                    self.scroll_target.insert(lane.id.clone(), offset);
                }
            }
            if self
                .history_drag
                .as_ref()
                .is_none_or(|(id, _)| id != &lane.id)
            {
                if let Some(offset) = self.scroll.get_mut(&lane.id) {
                    *offset = offset.clamp(0.0, maximum);
                }
                if let Some(target) = self.scroll_target.get_mut(&lane.id) {
                    *target = target.clamp(0.0, maximum);
                }
            }
        }
        for key in finished {
            self.fold_animations.remove(&key);
        }
    }

    fn action_tip(&self, action: &IterationViewAction) -> Option<&'static str> {
        Some(match action {
            IterationViewAction::ConnectTerminal { .. } => "Connect to a running Studio terminal",
            IterationViewAction::ToggleNavigator { .. } => "Navigate lane history",
            IterationViewAction::StartFlow { .. } => "Resume this lane",
            IterationViewAction::StopFlow { .. } => "Stop this lane and save its conversation",
            IterationViewAction::ToggleSplit { .. } => "Split lane",
            IterationViewAction::SplitFlow { .. } => "Split here",
            IterationViewAction::ArchiveFlow { .. } => "Archive this lane",
            IterationViewAction::ClearHistory { .. } => "Clear lane history",
            IterationViewAction::DeleteRecordings { .. } => "Delete this lane’s video history",
            IterationViewAction::CloseFreeze { .. } => "Close and freeze this app",
            IterationViewAction::PopOut { .. } => "Pop out this build as a standalone app",
            IterationViewAction::BuildStandalone { .. } => "Build and run a standalone app",
            IterationViewAction::BuildEmbedded { .. } => "Build and run in this lane",
            IterationViewAction::InspectCode { .. } => "Inspect this revision’s code changes",
            IterationViewAction::PlayRecording { .. } => "Open video",
            IterationViewAction::OpenImage { .. } => "Open image",
            IterationViewAction::TestTileToggled { .. } => "Open image",
            IterationViewAction::FoldBuild { flow, artifact } => {
                if self.expanded.contains(&format!("{flow}/{artifact}")) {
                    "Fold this revision"
                } else {
                    "Expand this revision"
                }
            }
            IterationViewAction::FoldItem { flow, key } => {
                if self.expanded.contains(&format!("{flow}/{key}")) {
                    "Hide details"
                } else {
                    "Show details"
                }
            }
            _ => return None,
        })
    }

    fn frame_button(
        &mut self,
        cx: &mut Cx2d,
        world: Rect,
        label: &str,
        action: IterationViewAction,
        clip: Rect,
    ) {
        let hovered = self.hovered.as_ref() == Some(&action)
            || matches!(&action, IterationViewAction::ToggleSplit { flow } if self.cut_flow.as_ref() == Some(flow));
        if hovered {
            let screen = self.screen(world);
            self.draw_card.color = self.raised;
            self.draw_card.border_size = 0.0;
            self.draw_card.border_radius = (3.0 * self.zoom) as f32;
            self.draw_card.shadow_radius = 0.0;
            self.draw_card.outline_size = 0.0;
            self.draw_card.draw_abs(cx, screen);
        }
        let color = if hovered { self.ink } else { self.muted };
        let r = self.screen(rect(
            world.pos.x + (world.size.x - 13.0) * 0.5,
            world.pos.y + (world.size.y - 13.0) * 0.5,
            13.0,
            13.0,
        ));
        let x = r.pos.x as f32;
        let y = r.pos.y as f32;
        let u = r.size.x as f32 / 16.0;
        self.draw_marks.begin();
        self.draw_marks
            .set_color(color.x, color.y, color.z, color.w);
        // A consistent, light 16-unit icon grid. Hit targets stay larger than
        // their glyphs; labels live in the window's delayed tooltip layer.
        let lines: &[&[(f32, f32)]] = match label {
            "Connect" => &[
                &[(1., 2.), (15., 2.), (15., 14.), (1., 14.), (1., 2.)],
                &[(4., 5.), (7., 8.), (4., 11.)],
                &[(9., 11.), (12., 11.)],
            ],
            "Navigator" => &[
                &[(1., 1.), (15., 1.), (15., 15.), (1., 15.), (1., 1.)],
                &[(4., 4.), (12., 4.)],
                &[(4., 7.), (9., 7.)],
                &[(4., 10.), (12., 10.)],
            ],
            "Close" => &[&[(3., 3.), (13., 13.)], &[(13., 3.), (3., 13.)]],
            "Code" => &[
                &[(5., 3.), (1., 8.), (5., 13.)],
                &[(11., 3.), (15., 8.), (11., 13.)],
                &[(9., 2.), (7., 14.)],
            ],
            "Play" | "Resume" => &[&[(4., 2.), (14., 8.), (4., 14.), (4., 2.)]],
            "Fold" => &[&[(3., 10.), (8., 5.), (13., 10.)]],
            "Expand" => &[&[(3., 6.), (8., 11.), (13., 6.)]],
            "Inspect" => &[
                &[(6., 1.), (1., 1.), (1., 6.)],
                &[(10., 1.), (15., 1.), (15., 6.)],
                &[(1., 10.), (1., 15.), (6., 15.)],
                &[(10., 15.), (15., 15.), (15., 10.)],
            ],
            "Pop out" | "Standalone" => &[
                &[(8., 3.), (2., 3.), (2., 14.), (13., 14.), (13., 8.)],
                &[(8., 1.), (15., 1.), (15., 8.)],
                &[(15., 1.), (7., 9.)],
            ],
            "Stop" => &[&[(3., 3.), (13., 3.), (13., 13.), (3., 13.), (3., 3.)]],
            "Split" => &[
                &[
                    (2., 2.),
                    (5., 2.),
                    (6., 4.),
                    (5., 6.),
                    (2., 6.),
                    (1., 4.),
                    (2., 2.),
                ],
                &[
                    (2., 10.),
                    (5., 10.),
                    (6., 12.),
                    (5., 14.),
                    (2., 14.),
                    (1., 12.),
                    (2., 10.),
                ],
                &[(5., 5.), (15., 13.)],
                &[(5., 11.), (15., 3.)],
            ],
            "Archive" => &[
                &[(1., 2.), (15., 2.), (15., 5.), (1., 5.), (1., 2.)],
                &[(3., 5.), (3., 14.), (13., 14.), (13., 5.)],
                &[(6., 8.), (10., 8.)],
            ],
            "Erase" => &[
                &[(2., 9.), (9., 2.), (15., 8.), (8., 15.), (2., 9.)],
                &[(5., 6.), (11., 12.)],
                &[(8., 15.), (15., 15.)],
            ],
            "Clear" => &[
                &[(2., 4.), (14., 4.)],
                &[(6., 4.), (6., 1.), (10., 1.), (10., 4.)],
                &[(4., 4.), (5., 14.), (11., 14.), (12., 4.)],
            ],
            "Build" => &[
                &[
                    (8., 1.),
                    (14., 4.),
                    (14., 12.),
                    (8., 15.),
                    (2., 12.),
                    (2., 4.),
                    (8., 1.),
                ],
                &[(2., 4.), (8., 7.), (14., 4.)],
                &[(8., 7.), (8., 15.)],
            ],
            _ => &[],
        };
        for line in lines {
            if let Some((first, rest)) = line.split_first() {
                self.draw_marks.move_to(x + first.0 * u, y + first.1 * u);
                for point in rest {
                    self.draw_marks.line_to(x + point.0 * u, y + point.1 * u);
                }
            }
        }
        self.draw_marks.stroke((1.2 * self.zoom as f32).max(0.8));
        self.draw_marks.end(cx);
        self.targets.push(Target {
            rect: intersect_rect(self.screen(world), clip),
            action,
        });
    }

    fn todo_mark(&mut self, cx: &mut Cx2d, world: Rect, state: TodoState, color: Vec4f) {
        let r = self.screen(world);
        let x = r.pos.x as f32;
        let y = r.pos.y as f32;
        let w = r.size.x as f32;
        let h = r.size.y as f32;
        self.draw_marks.begin();
        self.draw_marks
            .set_color(color.x, color.y, color.z, color.w);
        match state {
            TodoState::Implemented => {
                self.draw_marks.move_to(x, y + h * 0.5);
                self.draw_marks.line_to(x + w * 0.35, y + h);
                self.draw_marks.line_to(x + w, y);
                self.draw_marks.stroke(1.4);
            }
            TodoState::Working => {
                self.draw_marks.move_to(x + 1.0, y);
                self.draw_marks.line_to(x + w, y + h * 0.5);
                self.draw_marks.line_to(x + 1.0, y + h);
                self.draw_marks.close();
                self.draw_marks.fill();
            }
            TodoState::Queued => {
                self.draw_marks.rounded_rect(x, y, w, h, 2.0);
                self.draw_marks.stroke(1.0);
            }
            TodoState::Blocked => {
                self.draw_marks.move_to(x + w * 0.5, y);
                self.draw_marks.line_to(x + w * 0.5, y + h * 0.65);
                self.draw_marks.stroke(1.4);
                self.draw_marks
                    .rounded_rect(x + w * 0.5 - 0.7, y + h - 1.4, 1.4, 1.4, 0.7);
                self.draw_marks.fill();
            }
        }
        self.draw_marks.end(cx);
    }

    fn fold_chevron(&mut self, cx: &mut Cx2d, center: DVec2, open: f64, hovered: bool) {
        let center = self.screen(rect(center.x, center.y, 0.0, 0.0)).pos;
        let angle = open.clamp(0.0, 1.0) * std::f64::consts::FRAC_PI_2;
        let (sin, cos) = angle.sin_cos();
        let color = if hovered { self.ink } else { self.muted };
        self.draw_marks.begin();
        self.draw_marks
            .set_color(color.x, color.y, color.z, color.w);
        for (index, (px, py)) in [(-2.5, -4.0), (2.0, 0.0), (-2.5, 4.0)]
            .into_iter()
            .enumerate()
        {
            let x = (center.x + (px * cos - py * sin) * self.zoom) as f32;
            let y = (center.y + (px * sin + py * cos) * self.zoom) as f32;
            if index == 0 {
                self.draw_marks.move_to(x, y);
            } else {
                self.draw_marks.line_to(x, y);
            }
        }
        self.draw_marks.stroke((1.15 * self.zoom as f32).max(0.8));
        self.draw_marks.end(cx);
    }

    fn card(&mut self, cx: &mut Cx2d, world: Rect, selected: bool, raised: bool) {
        let screen = self.screen(world);
        self.draw_card.color = if raised { self.raised } else { self.surface };
        self.draw_card.border_color = if selected { self.accent } else { self.edge };
        self.draw_card.border_size = if selected { 1.6 } else { 0.7 };
        self.draw_card.border_radius = (6.0 * self.zoom) as f32;
        self.draw_card.outline_size = 0.0;
        self.draw_card.shadow_radius = if world.size.y > 24.0 {
            (3.0 * self.zoom) as f32
        } else {
            0.0
        };
        self.draw_card.draw_abs(cx, screen);
    }
    fn text(&mut self, cx: &mut Cx2d, world: Rect, value: &str, bold: bool, color: Vec4f) {
        let screen = self.screen(world);
        let size = (if bold { 9.5 } else { 9.0 }) * self.zoom;
        let draw = if bold {
            &mut self.draw_title
        } else {
            &mut self.draw_text
        };
        draw.color = color;
        draw.text_style.font_size = size as f32;
        draw.font_scale = 1.0;
        let mut value = short(value, (screen.size.x / (size * 0.50)).max(1.0) as usize);
        while value.chars().count() > 1
            && draw
                .layout(cx, 0.0, 0.0, None, false, Align::default(), &value)
                .size_in_lpxs
                .width as f64
                > screen.size.x
        {
            if value.ends_with('…') {
                value.pop();
            }
            value.pop();
            value.push('…');
        }
        cx.push_clip_rect(screen);
        draw.draw_abs(cx, screen.pos, &value);
        cx.pop_clip_rect();
    }

    fn draw_terminal(&mut self, cx: &mut Cx2d, scope: &mut Scope, index: usize) {
        if !self.terminals_enabled {
            return;
        }
        let id = self.lanes[index].id.clone();
        let world = self.terminal_rect(index);
        if !self.terminals.contains_key(&id) {
            let stopped = self
                .engine
                .as_ref()
                .and_then(|engine| engine.flows.get(&id))
                .is_some_and(|flow| flow.lifecycle == FlowLifecycle::Stopped);
            self.text(
                cx,
                rect(
                    world.pos.x + 8.0,
                    world.pos.y + 12.0,
                    world.size.x - 16.0,
                    24.0,
                ),
                if stopped {
                    "Lane stopped · Resume to continue"
                } else {
                    "Terminal is starting…"
                },
                false,
                self.muted,
            );
            return;
        }
        let widget = self.terminals[&id].widget.clone();
        let focused = Self::terminal_focused(cx, &widget);
        let frame = Some(TerminalPresentationFrame {
            padding: 8.0,
            radius: 6.0,
            border_width: (if focused { 1.6 } else { 0.7 }) / self.zoom.max(0.01) as f32,
            border_color: if focused { self.accent } else { self.edge },
        });
        if let Some(mut term) = widget.borrow_mut::<MpTerm>() {
            term.set_presentation_frame(cx, frame);
        } else if let Some(mut term) = widget.widget(cx, ids!(term)).borrow_mut::<MpTerm>() {
            term.set_presentation_frame(cx, frame);
        }
        self.draw_live_body(cx, scope, LiveBody::Terminal(id), world, self.viewport);
    }

    fn draw_live_body(
        &mut self,
        cx: &mut Cx2d,
        scope: &mut Scope,
        body: LiveBody,
        world: Rect,
        clip: Rect,
    ) {
        let camera = self.camera();
        let screen = self.screen(world);
        let Some(host) = self.host_mut(&body) else {
            return;
        };
        let geometry_changed = host.screen != Some(screen) || host.clip != Some(clip);
        host.screen = Some(screen);
        host.clip = Some(clip);
        let widget = host.widget.clone();
        let mut list = host.draw_list.take().unwrap_or_else(|| DrawList2d::new(cx));
        let local = Rect {
            pos: camera.screen_to_local(screen.pos),
            size: world.size,
        };
        let translation = screen.pos - local.pos * camera.scale;
        let anchor = PopupAnchorTransform {
            scale: camera.scale,
            translation,
        };
        match &body {
            LiveBody::Terminal(_) => Self::anchor_terminal(cx, &widget, Some((self.area, anchor))),
            LiveBody::App(_) => {
                if let Some(mut app) = widget.borrow_mut::<IterationRunView>() {
                    app.canvas_ime_anchor = Some((self.area, anchor));
                    if geometry_changed || self.pixel_view.is_some() {
                        app.clear_pointer_hover();
                    }
                }
            }
        }
        list.begin_always(cx);
        cx.begin_root_turtle(dvec2(131072.0, 131072.0), Layout::flow_overlay());
        cx.push_clip_rect(Rect {
            pos: camera.screen_to_local(self.viewport.pos),
            size: self.viewport.size / camera.scale,
        });
        cx.push_clip_rect(Rect {
            pos: camera.screen_to_local(clip.pos),
            size: clip.size / camera.scale,
        });
        cx.push_clip_rect(local);
        cx.begin_turtle(
            Walk {
                abs_pos: Some(local.pos),
                width: Size::Fixed(local.size.x),
                height: Size::Fixed(local.size.y),
                ..Default::default()
            },
            Layout {
                flow: makepad_widgets::Flow::Down,
                clip_x: true,
                clip_y: true,
                ..Default::default()
            },
        );
        widget.draw_walk_all(cx, scope, Walk::fill());
        cx.end_turtle();
        cx.pop_clip_rect();
        cx.pop_clip_rect();
        cx.pop_clip_rect();
        cx.end_pass_sized_turtle();
        list.end(cx);
        let mut matrix = Mat4f::identity();
        matrix.v[0] = camera.scale as f32;
        matrix.v[5] = camera.scale as f32;
        matrix.v[12] = translation.x as f32;
        matrix.v[13] = translation.y as f32;
        list.set_view_transform(cx, &matrix);
        if let Some(host) = self.host_mut(&body) {
            host.draw_list = Some(list);
        }
    }

    fn preview_rect(&self, target: Rect, id: &str) -> Option<Rect> {
        let preview = self.previews.get(id)?;
        let ratio =
            (target.size.x / preview.width as f64).min(target.size.y / preview.height as f64);
        let size = dvec2(preview.width as f64, preview.height as f64) * ratio;
        Some(Rect {
            pos: target.pos + (target.size - size) * 0.5,
            size,
        })
    }

    fn draw_preview(&mut self, cx: &mut Cx2d, target: Rect, id: &str) -> bool {
        let Some(preview) = self.previews.get(id) else {
            return false;
        };
        let ratio =
            (target.size.x / preview.width as f64).min(target.size.y / preview.height as f64);
        let size = dvec2(preview.width as f64, preview.height as f64) * ratio;
        self.draw_image.draw_vars.set_texture(0, &preview.texture);
        self.draw_image.draw_abs(
            cx,
            Rect {
                pos: target.pos + (target.size - size) * 0.5,
                size,
            },
        );
        true
    }

    fn draw_minimap(&mut self, cx: &mut Cx2d, index: usize) {
        let Some(panel) = self.minimap_rect(index) else {
            return;
        };
        let lane = &self.lanes[index];
        let total = lane.height.max(self.body_height(&lane.id)).max(1.0);
        let slack = (total - lane.height).max(0.0);
        let offset = self.scroll.get(&lane.id).copied().unwrap_or(0.0);
        let visible = self.body_height(&lane.id);
        let items = lane.items.clone();
        self.draw_card.color = self.background;
        self.draw_card.border_color = self.edge;
        self.draw_card.border_size = 1.0;
        self.draw_card.border_radius = 4.0;
        self.draw_card.shadow_radius = 4.0;
        self.draw_card.outline_size = 0.0;
        self.draw_card.draw_abs(
            cx,
            rect(
                panel.pos.x - 4.0,
                panel.pos.y - 4.0,
                panel.size.x + 8.0,
                panel.size.y + 8.0,
            ),
        );
        cx.push_clip_rect(panel);
        for item in items.iter() {
            let block = rect(
                panel.pos.x + 3.0,
                panel.pos.y + (item.y + slack) / total * panel.size.y,
                panel.size.x - 6.0,
                (item.height / total * panel.size.y).max(1.0),
            );
            self.draw_shape.color = if item.emphasized {
                self.accent
            } else {
                self.raised
            };
            self.draw_shape.draw_abs(cx, block);
            if let Some(preview) = &item.preview {
                self.draw_preview(cx, block, preview);
            }
        }
        let top = panel.pos.y + offset / total * panel.size.y;
        let height = (visible / total * panel.size.y).min(panel.size.y).max(2.0);
        self.draw_shape.color = self.accent;
        for edge in [
            rect(panel.pos.x, top, panel.size.x, 1.5),
            rect(panel.pos.x, top + height - 1.5, panel.size.x, 1.5),
            rect(panel.pos.x, top, 1.5, height),
            rect(panel.pos.x + panel.size.x - 1.5, top, 1.5, height),
        ] {
            self.draw_shape.draw_abs(cx, edge);
        }
        cx.pop_clip_rect();
    }

    fn draw_floating_trays(&mut self, cx: &mut Cx2d) {
        let mut list = self
            .tray_draw_list
            .take()
            .unwrap_or_else(|| DrawList2d::new(cx));
        list.begin_overlay_reuse(cx);
        cx.push_clip_rect(self.viewport);
        for index in 0..self.lanes.len() {
            let id = self.lanes[index].id.clone();
            let clip = self.lane_clip(index);
            if clip.size.y <= 0.0 {
                continue;
            }
            cx.push_clip_rect(clip);
            if let Some(panel) = self.tray_rect(index) {
                let attachments = self.attachment_tray.get(&id).cloned().unwrap_or_default();
                let collapsed = self.collapsed_trays.contains(&id);
                self.card(cx, panel, false, true);
                self.text(
                    cx,
                    rect(
                        panel.pos.x + 7.0,
                        panel.pos.y + 6.0,
                        panel.size.x - 14.0,
                        16.0,
                    ),
                    &format!(
                        "{} image{}",
                        attachments.len(),
                        if attachments.len() == 1 { "" } else { "s" }
                    ),
                    false,
                    self.ink,
                );
                self.targets.push(Target {
                    rect: intersect_rect(self.screen(panel), clip),
                    action: IterationViewAction::SelectFlow { id: id.clone() },
                });
                self.targets.push(Target {
                    rect: intersect_rect(
                        self.screen(rect(panel.pos.x, panel.pos.y, panel.size.x, 24.0)),
                        clip,
                    ),
                    action: IterationViewAction::ToggleAttachmentTray { flow: id.clone() },
                });
                if !collapsed {
                    for (slot, capture) in attachments.iter().enumerate() {
                        let target = rect(
                            panel.pos.x + 6.0 + (slot % 4) as f64 * 58.0,
                            panel.pos.y + 21.0 + (slot / 4) as f64 * 43.0,
                            54.0,
                            38.0,
                        );
                        self.card(cx, target, false, false);
                        let target_screen = self.screen(target);
                        if !self.draw_preview(cx, target_screen, capture) {
                            self.text(cx, target, "Loading…", false, self.muted);
                        }
                        self.targets.push(Target {
                            rect: intersect_rect(target_screen, clip),
                            action: IterationViewAction::OpenImage {
                                flow: id.clone(),
                                preview_id: capture.clone(),
                                title: "Attached image".into(),
                            },
                        });
                    }
                }
            }
            if let Some(panel) = self.tile_strip_rect(index) {
                let tiles: Vec<_> = self
                    .test_tiles
                    .get(&id)
                    .into_iter()
                    .flatten()
                    .filter(|tile| tile.active)
                    .cloned()
                    .collect();
                self.card(cx, panel, false, true);
                self.text(
                    cx,
                    rect(
                        panel.pos.x + 6.0,
                        panel.pos.y + 4.0,
                        panel.size.x - 12.0,
                        16.0,
                    ),
                    "Recording · read-only",
                    false,
                    self.muted,
                );
                for (slot, tile) in tiles.iter().enumerate() {
                    let target = rect(
                        panel.pos.x + 4.0 + slot as f64 * 90.0,
                        panel.pos.y + 20.0,
                        86.0,
                        52.0,
                    );
                    self.card(cx, target, tile.active, false);
                    let preview_screen = self.screen(rect(
                        target.pos.x + 2.0,
                        target.pos.y + 2.0,
                        target.size.x - 4.0,
                        30.0,
                    ));
                    if !self.draw_preview(cx, preview_screen, &tile.preview_id) {
                        self.text(cx, target, "Pending", false, self.muted);
                    }
                    self.text(
                        cx,
                        rect(
                            target.pos.x + 3.0,
                            target.pos.y + 33.0,
                            target.size.x - 6.0,
                            14.0,
                        ),
                        &tile.title,
                        false,
                        if tile.active { self.accent } else { self.muted },
                    );
                    self.targets.push(Target {
                        rect: intersect_rect(self.screen(target), clip),
                        action: IterationViewAction::OpenImage {
                            flow: id.clone(),
                            preview_id: tile.preview_id.clone(),
                            title: tile.title.clone(),
                        },
                    });
                }
            }
            cx.pop_clip_rect();
        }
        cx.pop_clip_rect();
        list.end(cx);
        list.set_view_transform(cx, &Mat4f::identity());
        self.tray_draw_list = Some(list);
    }

    fn pixel_clip(&self) -> Rect {
        rect(
            self.viewport.pos.x + 16.0,
            self.viewport.pos.y + 52.0,
            (self.viewport.size.x - 32.0).max(1.0),
            (self.viewport.size.y - 76.0).max(1.0),
        )
    }
    fn clamp_pixel_pan(&mut self) {
        let room = (self.pixel_source_size - self.pixel_clip().size) * 0.5;
        self.pixel_pan.x = self.pixel_pan.x.clamp(-room.x.max(0.0), room.x.max(0.0));
        self.pixel_pan.y = self.pixel_pan.y.clamp(-room.y.max(0.0), room.y.max(0.0));
    }
    fn close_pixel_view(&mut self, cx: &mut Cx) {
        if let Some((flow, id)) = self.pixel_view.take() {
            if let Some(tile) = self.recording_tile(&flow, &id) {
                cx.widget_action(
                    self.uid,
                    IterationViewAction::TestTileToggled {
                        flow,
                        id,
                        preview_id: tile.preview_id.clone(),
                        open: false,
                    },
                );
            }
        }
        self.pixel_drag = None;
        self.full_preview = None;
        if let Some(list) = &self.pixel_draw_list {
            list.redraw(cx);
        }
        self.area.redraw(cx);
    }
    fn draw_pixel_view(&mut self, cx: &mut Cx2d) {
        let Some(tile) = self
            .pixel_view
            .as_ref()
            .and_then(|(flow, id)| self.recording_tile(flow, id))
            .cloned()
        else {
            // Clear a previously used overlay list so closing cannot leave its
            // last image composited above the live lane view.
            if let Some(list) = &mut self.pixel_draw_list {
                list.begin_overlay_reuse(cx);
                list.end(cx);
            }
            return;
        };
        let mut list = self
            .pixel_draw_list
            .take()
            .unwrap_or_else(|| DrawList2d::new(cx));
        list.begin_overlay_reuse(cx);
        cx.push_clip_rect(self.viewport);
        self.draw_shape.color = self.background;
        self.draw_shape.draw_abs(cx, self.viewport);
        self.draw_title.color = self.ink;
        self.draw_title.text_style.font_size = 14.0;
        self.draw_title.draw_abs(
            cx,
            self.viewport.pos + dvec2(16.0, 15.0),
            &format!(
                "{} · {} · read-only",
                tile.title,
                if tile.active {
                    "recording active"
                } else {
                    "captured UI"
                }
            ),
        );
        self.draw_text.color = self.muted;
        self.draw_text.text_style.font_size = 10.0;
        self.draw_text.draw_abs(cx, self.viewport.pos + dvec2(16.0, 35.0), "One capture pixel per display pixel · scroll or drag to pan · click again / Escape to close");
        let clip = self.pixel_clip();
        let dpi = cx.current_dpi_factor().max(0.1);
        self.pixel_source_size = dvec2(tile.width as f64, tile.height as f64) / dpi;
        self.clamp_pixel_pan();
        cx.push_clip_rect(clip);
        let original = self
            .full_preview
            .as_ref()
            .filter(|(id, image)| {
                *id == tile.preview_id && image.width == tile.width && image.height == tile.height
            })
            .map(|(_, image)| image)
            .or_else(|| {
                self.previews
                    .get(&tile.preview_id)
                    .filter(|image| image.width == tile.width && image.height == tile.height)
            });
        if let Some(original) = original {
            let pos = clip.pos + (clip.size - self.pixel_source_size) * 0.5 + self.pixel_pan;
            let pos = dvec2((pos.x * dpi).round() / dpi, (pos.y * dpi).round() / dpi);
            self.draw_image.draw_vars.set_texture(0, &original.texture);
            self.draw_image.draw_abs(
                cx,
                Rect {
                    pos,
                    size: self.pixel_source_size,
                },
            );
        } else {
            self.draw_text.draw_abs(cx, clip.pos + dvec2(12.0, 12.0), "Waiting for original capture pixels; the thumbnail is not enlarged as a substitute.");
        }
        cx.pop_clip_rect();
        cx.pop_clip_rect();
        list.end(cx);
        list.set_view_transform(cx, &Mat4f::identity());
        self.pixel_draw_list = Some(list);
    }
}

impl WidgetNode for StudioIterationView {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _: &mut Cx) -> Walk {
        self.walk
    }
    fn area(&self) -> Area {
        self.area
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
        for host in self.terminals.values().chain(self.apps.values()) {
            host.widget.redraw(cx);
        }
    }
    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        for (id, host) in &self.terminals {
            visit(LiveId::from_str(id), host.widget.clone());
        }
        for (id, host) in &self.apps {
            visit(LiveId::from_str(&format!("app/{id}")), host.widget.clone());
        }
    }
    fn find_widgets_from_point(&self, cx: &Cx, p: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        if self.pixel_view.is_none() {
            if let Some(host) = self.body_at(p).as_ref().and_then(|body| self.host(body)) {
                host.widget
                    .find_widgets_from_point(cx, self.camera().screen_to_local(p), found);
            }
        }
    }
}

impl Widget for StudioIterationView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let view = cx.walk_turtle(walk);
        let resized = view.size != self.viewport.size;
        self.viewport = view;
        cx.add_rect_area(&mut self.area, view);
        if self.minimap_open.is_some() && !cx.has_key_focus(self.area) {
            self.minimap_open = None;
        }
        if self.zoom <= 0.0 && !self.needs_fit {
            self.zoom =
                ((view.size.x - 24.0) / (self.lane_width_at(0) + LEFT * 2.0)).clamp(0.25, 1.0);
            self.pan = dvec2(0.0, 0.0);
            self.fitted = false;
        } else if self.needs_fit || (resized && self.fitted) {
            let scene_width = self.scene_width();
            self.zoom = ((view.size.x - 24.0) / scene_width).clamp(0.20, 1.15);
            self.pan = dvec2((view.size.x - scene_width * self.zoom) * 0.5, 0.0);
            self.fitted = true;
            self.needs_fit = false;
        }
        self.targets.clear();
        cx.push_clip_rect(view);
        self.draw_shape.color = self.background;
        self.draw_shape.draw_abs(cx, view);
        for index in 0..self.lanes.len() {
            let x = self.lane_x(index);
            let width = self.lane_width_at(index);
            let lane_rect = self.screen(rect(
                x - 7.0,
                2.0,
                width + 14.0,
                self.viewport.size.y / self.zoom - 4.0,
            ));
            self.draw_marks.begin();
            let color = blend_color(self.background, self.ink, 0.20);
            self.draw_marks
                .set_color(color.x, color.y, color.z, color.w);
            self.draw_marks.rounded_rect(
                lane_rect.pos.x as f32,
                lane_rect.pos.y as f32,
                lane_rect.size.x as f32,
                lane_rect.size.y as f32,
                (8.0 * self.zoom) as f32,
            );
            self.draw_marks.stroke(1.0);
            self.draw_marks.end(cx);
            let id = self.lanes[index].id.clone();
            let resizing = matches!(&self.navigation_drag, Some(NavigationDrag::LaneWidth { flow, .. }) if flow == &id);
            let edge_x = self.screen(rect(x + width + 7.0, 0.0, 0.0, 0.0)).pos.x;
            self.draw_shape.color = if resizing || self.width_hover.as_ref() == Some(&id) {
                self.accent
            } else {
                self.edge
            };
            let grip_height = (view.size.y - 32.0).clamp(0.0, 28.0);
            self.draw_shape.draw_abs(
                cx,
                rect(
                    edge_x - 1.0,
                    view.pos.y + (view.size.y - grip_height) * 0.5,
                    2.0,
                    grip_height,
                ),
            );
            let title = self.lanes[index].title.clone();
            let subtitle = if self.cut_flow.as_ref() == Some(&id) {
                "Choose split point · Esc".to_owned()
            } else {
                self.connection_labels
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| self.lanes[index].subtitle.clone())
            };
            let items = self.lanes[index].items.clone();
            if self.follow_tail.contains(&id) {
                let end = (self.lanes[index].height - self.body_height(&id)).max(0.0);
                self.scroll.insert(id.clone(), end);
                self.scroll_target.insert(id.clone(), end);
            }
            let offset = self.scroll.get(&id).copied().unwrap_or(0.0);
            self.text(
                cx,
                rect(x + 29.0, 9.0, width - 312.0, 18.0),
                &title,
                true,
                self.ink,
            );
            self.text(
                cx,
                rect(x + width - 279.0, 10.0, 120.0, 16.0),
                &subtitle,
                false,
                self.muted,
            );
            if !self.is_archived(&id) {
                self.targets.push(Target {
                    rect: self.screen(rect(x, 0.0, width, 30.0)),
                    action: IterationViewAction::SelectFlow { id: id.clone() },
                });
            }
            self.frame_button(
                cx,
                rect(x + 2.0, 5.0, 22.0, 22.0),
                "Navigator",
                IterationViewAction::ToggleNavigator { flow: id.clone() },
                self.viewport,
            );
            let stopped = self
                .engine
                .as_ref()
                .and_then(|engine| engine.flows.get(&id))
                .is_some_and(|flow| flow.lifecycle == FlowLifecycle::Stopped);
            if !self.is_archived(&id) {
                for (button, label, action) in [
                    (
                        rect(x + width - 146.0, 5.0, 22.0, 22.0),
                        "Erase",
                        IterationViewAction::ClearHistory { flow: id.clone() },
                    ),
                    (
                        rect(x + width - 122.0, 5.0, 22.0, 22.0),
                        "Connect",
                        IterationViewAction::ConnectTerminal { flow: id.clone() },
                    ),
                    (
                        rect(x + width - 98.0, 5.0, 22.0, 22.0),
                        if stopped { "Resume" } else { "Stop" },
                        if stopped {
                            IterationViewAction::StartFlow { flow: id.clone() }
                        } else {
                            IterationViewAction::StopFlow { flow: id.clone() }
                        },
                    ),
                    (
                        rect(x + width - 74.0, 5.0, 22.0, 22.0),
                        "Split",
                        IterationViewAction::ToggleSplit { flow: id.clone() },
                    ),
                    (
                        rect(x + width - 50.0, 5.0, 22.0, 22.0),
                        "Archive",
                        IterationViewAction::ArchiveFlow { flow: id.clone() },
                    ),
                    (
                        rect(x + width - 26.0, 5.0, 22.0, 22.0),
                        "Clear",
                        IterationViewAction::DeleteRecordings { flow: id.clone() },
                    ),
                ] {
                    self.frame_button(cx, button, label, action, self.viewport);
                }
            }
            let clip = self.lane_clip(index);
            let app_world = self.app_rect(index);
            let mut app_drawn = false;
            cx.push_clip_rect(clip);
            for item in items.iter() {
                let world = rect(
                    x,
                    self.history_top(index) + item.y - offset,
                    width,
                    item.height,
                );
                let screen = self.screen(world);
                if screen.pos.y >= clip.pos.y + clip.size.y
                    || screen.pos.y + screen.size.y <= clip.pos.y
                {
                    continue;
                }
                let hovered = item
                    .action
                    .as_ref()
                    .is_some_and(|action| self.hovered.as_ref() == Some(action));
                self.card(cx, world, hovered, false);
                let item_clip = intersect_rect(screen, clip);
                cx.push_clip_rect(item_clip);
                if let Some(action) = &item.action {
                    self.targets.push(Target {
                        rect: item_clip,
                        action: action.clone(),
                    });
                }
                if let Some(open) = item.fold_open {
                    self.fold_chevron(cx, dvec2(x + 14.0, world.pos.y + 14.0), open, hovered);
                }
                let control_width = item.controls.len() as f64 * 24.0;
                let title_inset = if item.fold_open.is_some() { 27.0 } else { 10.0 };
                self.text(
                    cx,
                    rect(
                        x + title_inset,
                        world.pos.y + 8.0,
                        width - title_inset - 14.0 - control_width,
                        16.0,
                    ),
                    &item.title,
                    true,
                    if item.emphasized {
                        self.muted
                    } else {
                        self.ink
                    },
                );
                let mut right = x + width - 5.0;
                for button in item.controls.iter().rev() {
                    let width = 22.0;
                    right -= width;
                    self.frame_button(
                        cx,
                        rect(right, world.pos.y + 3.0, width, 22.0),
                        button.label,
                        button.action.clone(),
                        item_clip,
                    );
                    right -= 2.0;
                }
                let lines_top = if item.pills.is_empty() {
                    26.0
                } else {
                    item.pills
                        .iter()
                        .map(|pill| pill.rect.pos.y + pill.rect.size.y)
                        .fold(0.0, f64::max)
                        + 8.0
                };
                for (line, text) in item.lines.iter().enumerate() {
                    self.text(
                        cx,
                        rect(
                            x + 10.0,
                            world.pos.y + lines_top + line as f64 * item.line_height,
                            width - 20.0,
                            item.line_height,
                        ),
                        text,
                        false,
                        if text.starts_with("• ") {
                            self.ink
                        } else {
                            self.muted
                        },
                    );
                }
                for pill in &item.pills {
                    let pill_world = Rect {
                        pos: world.pos + pill.rect.pos,
                        size: pill.rect.size,
                    };
                    let color = match pill.state {
                        TodoState::Queued => self.muted,
                        TodoState::Working => blend_color(self.ink, self.accent, 0.7),
                        TodoState::Implemented => vec4(0.30, 0.77, 0.42, 1.0),
                        TodoState::Blocked => vec4(0.95, 0.65, 0.30, 1.0),
                    };
                    self.draw_card.color = blend_color(self.background, color, 0.075);
                    self.draw_card.border_color = blend_color(self.background, color, 0.20);
                    self.draw_card.border_size = 0.6;
                    self.draw_card.border_radius = (4.0 * self.zoom) as f32;
                    self.draw_card.shadow_radius = 0.0;
                    let pill_screen = self.screen(pill_world);
                    self.draw_card.draw_abs(cx, pill_screen);
                    self.todo_mark(
                        cx,
                        rect(pill_world.pos.x + 7.0, pill_world.pos.y + 5.0, 10.0, 10.0),
                        pill.state,
                        color,
                    );
                    self.text(
                        cx,
                        rect(
                            pill_world.pos.x + 23.0,
                            pill_world.pos.y + 5.0,
                            pill_world.size.x - 29.0,
                            14.0,
                        ),
                        &pill.label,
                        false,
                        color,
                    );
                }
                if let Some(preview) = &item.preview {
                    let area = rect(
                        x + 8.0,
                        world.pos.y + item.content_height - item.preview_height - 8.0,
                        width - 16.0,
                        item.preview_height,
                    );
                    let preview_screen = self.screen(area);
                    if !self.draw_preview(cx, preview_screen, preview) {
                        self.text(cx, area, "Loading frame…", false, self.muted);
                    }
                    let target = intersect_rect(
                        self.preview_rect(preview_screen, preview)
                            .unwrap_or(preview_screen),
                        item_clip,
                    );
                    if target.size.x > 0.0 && target.size.y > 0.0 {
                        self.targets.push(Target {
                            rect: target,
                            action: item.preview_action.clone().unwrap_or_else(|| {
                                IterationViewAction::OpenImage {
                                    flow: id.clone(),
                                    preview_id: preview.clone(),
                                    title: item.title.clone(),
                                }
                            }),
                        });
                    }
                }
                if item.live_app {
                    if let Some(world) = app_world {
                        self.draw_live_body(cx, scope, LiveBody::App(id.clone()), world, item_clip);
                        app_drawn = true;
                    }
                }
                cx.pop_clip_rect();
            }
            if self.apps.contains_key(&id) && !app_drawn {
                let world = app_world
                    .unwrap_or_else(|| rect(x + 8.0, self.body_top(&id), width - 16.0, APP_HEIGHT));
                self.draw_live_body(
                    cx,
                    scope,
                    LiveBody::App(id.clone()),
                    world,
                    Rect {
                        pos: clip.pos,
                        size: dvec2(0.0, 0.0),
                    },
                );
            }
            self.draw_split_marker(cx, index);
            if let Some((track, thumb)) = self.scrollbar(index) {
                self.draw_shape.color = self.surface;
                self.draw_shape.draw_abs(cx, track);
                self.draw_shape.color = self.edge;
                self.draw_shape.draw_abs(cx, thumb);
            }
            self.draw_minimap(cx, index);
            cx.pop_clip_rect();
            if self.is_archived(&id) {
                continue;
            }
            let top = self.terminal_top(&id);
            let focused = self
                .terminals
                .get(&id)
                .is_some_and(|host| Self::terminal_focused(cx, &host.widget));
            if !self.terminals.contains_key(&id) {
                self.card(cx, self.terminal_rect(index), false, false);
            }
            self.draw_terminal(cx, scope, index);
            let grip = self.screen(rect(x + width * 0.5 - 18.0, top + 3.0, 36.0, 2.0));
            self.draw_shape.color = if focused { self.accent } else { self.edge };
            self.draw_shape.draw_abs(cx, grip);
        }
        cx.pop_clip_rect();
        self.draw_floating_trays(cx);
        self.draw_pixel_view(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::MouseDown(pointer) = event {
            if self.cut_flow.is_some()
                && self.split_at(pointer.abs).is_none()
                && self.mini_at(pointer.abs).is_none()
                && self.scrollbar_at(pointer.abs).is_none()
                && !matches!(
                    self.hit_at(pointer.abs),
                    Some(IterationViewAction::ToggleSplit { .. })
                )
            {
                self.cut_flow = None;
                self.cut_hover = None;
                self.area.redraw(cx);
            }
            if self.minimap_open.is_some()
                && self.mini_at(pointer.abs).is_none()
                && !matches!(
                    self.hit_at(pointer.abs),
                    Some(IterationViewAction::ToggleNavigator { .. })
                )
            {
                self.minimap_open = None;
                self.area.redraw(cx);
            }
        }
        if !self.terminals_enabled && live_input(event) {
            return;
        }
        // Image drops always go through the host once. It copies/registers the
        // evidence, then calls MpTerm::ai_drop_file and acknowledges delivery.
        // Forwarding the same Drop to the child would attach it twice.
        match event.drag_hits(cx, self.area) {
            DragHit::Drag(drag)
                if self.drop_flow(drag.abs).is_some() && drag.items.iter().any(image_drop_path) =>
            {
                *lock_from_ui(&drag.response) = DragResponse::Copy;
            }
            DragHit::Drop(drop) => {
                if let Some(flow) = self.drop_flow(drop.abs) {
                    if let Some(DragItem::FilePath { path, .. }) =
                        drop.items.iter().find(|item| image_drop_path(item))
                    {
                        cx.widget_action(
                            self.uid,
                            IterationViewAction::AttachImage {
                                flow,
                                path: PathBuf::from(path),
                            },
                        );
                    }
                }
            }
            _ => {}
        }
        if self.pixel_view.is_some() {
            let input = event.requires_visibility()
                || matches!(
                    event,
                    Event::MouseUp(_)
                        | Event::MouseLeave(_)
                        | Event::LongPress(_)
                        | Event::SelectionHandleDrag(_)
                        | Event::KeyDown(_)
                        | Event::KeyUp(_)
                        | Event::TextInput(_)
                        | Event::TextRangeReplace(_)
                        | Event::TextCopy(_)
                        | Event::TextCut(_)
                        | Event::ImeAction(_)
                        | Event::Drag(_)
                        | Event::Drop(_)
                        | Event::DragEnd
                );
            if input {
                // The expanded capture is pixels only. No pointer, key, scroll
                // or drop is forwarded into the terminal or the tested app.
                self.clear_app_hovers();
                match event.hits(cx, self.area) {
                    Hit::FingerDown(event) => {
                        cx.set_key_focus(self.area);
                        self.pixel_drag = Some((event.abs, self.pixel_pan));
                    }
                    Hit::FingerMove(event) => {
                        if let Some((start, pan)) = self.pixel_drag {
                            self.pixel_pan = pan + event.abs - start;
                            self.clamp_pixel_pan();
                            self.area.redraw(cx);
                        }
                    }
                    Hit::FingerUp(event) => {
                        if self
                            .pixel_drag
                            .take()
                            .is_some_and(|(start, _)| (event.abs - start).length() < 4.0)
                        {
                            self.close_pixel_view(cx);
                        }
                    }
                    Hit::FingerScroll(event) => {
                        self.pixel_pan -= event.scroll;
                        self.clamp_pixel_pan();
                        self.area.redraw(cx);
                    }
                    Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => {
                        cx.set_cursor(MouseCursor::Hand)
                    }
                    Hit::KeyDown(event) if event.key_code == KeyCode::Escape => {
                        self.close_pixel_view(cx)
                    }
                    _ => {}
                }
                return;
            }
        }
        if self.fold_frame.is_event(event).is_some() {
            self.clear_cut_hover();
            self.fold_time = cx.seconds_since_app_start();
            self.apply_fold_animations(self.fold_time, true, false);
            if !self.fold_animations.is_empty() {
                self.fold_frame = cx.new_next_frame();
            }
            self.area.redraw(cx);
        }
        if let Some(frame) = self.scroll_frame.is_event(event) {
            let dt = if self.scroll_time > 0.0 {
                (frame.time - self.scroll_time).clamp(1.0 / 240.0, 0.05)
            } else {
                1.0 / 60.0
            };
            self.scroll_time = frame.time;
            let alpha = 1.0 - (-dt / 0.065).exp();
            let mut moving = false;
            let mut cut_moved = false;
            for (id, target) in &self.scroll_target {
                let offset = self.scroll.entry(id.clone()).or_default();
                let delta = *target - *offset;
                if delta != 0.0 && self.cut_flow.as_ref() == Some(id) {
                    cut_moved = true;
                }
                if delta.abs() * self.zoom > 0.4 {
                    *offset += delta * alpha;
                    moving = true;
                } else {
                    *offset = *target;
                }
            }
            if cut_moved {
                self.clear_cut_hover();
            }
            if moving {
                self.scroll_frame = cx.new_next_frame();
            }
            self.area.redraw(cx);
        }
        let pointer = match event {
            Event::MouseDown(event) => Some(event.abs),
            Event::MouseMove(event) => Some(event.abs),
            Event::MouseUp(event) => Some(event.abs),
            Event::MouseLeave(event) => Some(event.abs),
            Event::Scroll(event) => Some(event.abs),
            Event::TouchUpdate(event) => self
                .body_touch_capture
                .as_ref()
                .and_then(|(uid, _)| event.touches.iter().find(|touch| touch.uid == *uid))
                .or_else(|| {
                    event
                        .touches
                        .iter()
                        .find(|touch| touch.state != TouchState::Stable)
                })
                .map(|touch| touch.abs),
            _ => None,
        };
        let body_at = if matches!(event, Event::MouseLeave(_)) {
            None
        } else {
            pointer.and_then(|point| self.body_at(point))
        };
        let zoom_scroll = matches!(event, Event::Scroll(event) if event.modifiers.control || event.modifiers.logo || event.modifiers.shift);
        let input = event.requires_visibility()
            || matches!(
                event,
                Event::MouseUp(_)
                    | Event::MouseLeave(_)
                    | Event::LongPress(_)
                    | Event::SelectionHandleDrag(_)
                    | Event::KeyDown(_)
                    | Event::KeyUp(_)
                    | Event::TextInput(_)
                    | Event::TextRangeReplace(_)
                    | Event::TextCopy(_)
                    | Event::TextCut(_)
                    | Event::ImeAction(_)
                    | Event::Drag(_)
                    | Event::Drop(_)
                    | Event::DragEnd
            );
        let captured = match event {
            Event::MouseMove(_) | Event::MouseUp(_) | Event::MouseLeave(_) => {
                self.body_capture.clone()
            }
            Event::TouchUpdate(event) => self
                .body_touch_capture
                .as_ref()
                .filter(|(uid, _)| event.touches.iter().any(|touch| touch.uid == *uid))
                .map(|(_, body)| body.clone()),
            _ => None,
        };
        let camera = self.camera();
        // Deliver the old body's HoverOut first; its default cursor must not
        // overwrite the new body's native terminal/app cursor afterward.
        if matches!(event, Event::MouseMove(_) | Event::MouseLeave(_))
            && self.body_hover != body_at
            && captured.is_none()
        {
            if let Some(body) = self.body_hover.as_ref() {
                if matches!(body, LiveBody::App(_)) && body_at.is_none() {
                    if let Some(host) = self.host(body) {
                        if let Some(mut app) = host.widget.borrow_mut::<IterationRunView>() {
                            app.clear_pointer_hover();
                        }
                    }
                } else if let Some(host) = self.host(body) {
                    let mapped = remap_event(event, &camera);
                    host.widget
                        .handle_event(cx, mapped.as_ref().unwrap_or(event), scope);
                    if let Some(mapped) = &mapped {
                        sync_handled(event, mapped, &camera);
                    }
                }
            }
            self.body_hover = body_at.clone();
        }
        if !input {
            // Resident children keep signals, timers and their GPU transport
            // even if the lane is offscreen or the live build is folded.
            for host in self
                .terminals
                .values()
                .filter(|_| self.terminals_enabled)
                .chain(self.apps.values())
            {
                host.widget.handle_event(cx, event, scope);
            }
        } else if !matches!(event, Event::Drag(_) | Event::Drop(_) | Event::DragEnd)
            && (pointer.is_some() || self.cut_flow.is_none())
            && !zoom_scroll
            && (pointer.is_none()
                || captured.is_some()
                || (self.drag.is_none() && self.navigation_drag.is_none() && body_at.is_some()))
        {
            let mapped = remap_event(event, &camera);
            let delivered = mapped.as_ref().unwrap_or(event);
            let target = captured.as_ref().or(body_at.as_ref());
            let press = matches!(event, Event::MouseDown(event) if event.handled.get().is_empty())
                || matches!(event, Event::TouchUpdate(event) if event.touches.iter().any(|touch| touch.state == TouchState::Start && touch.handled.get().is_empty()));
            if pointer.is_none() {
                // Native key-focus filtering selects the actual focused child.
                // A tray submission is an actual terminal Enter, never an
                // application/editor key or Shift+Enter multiline input.
                for host in self.terminals.values().filter(|_| self.terminals_enabled) {
                    host.widget.handle_event(cx, delivered, scope);
                }
                for host in self.apps.values() {
                    host.widget.handle_event(cx, delivered, scope);
                }
            } else if let Some(host) = target.and_then(|body| self.host(body)) {
                host.widget.handle_event(cx, delivered, scope);
            }
            if let Some(mapped) = &mapped {
                sync_handled(event, mapped, &camera);
            }
            let accepted_press = press
                && (matches!(event, Event::MouseDown(event) if !event.handled.get().is_empty())
                    || matches!(event, Event::TouchUpdate(event) if event.touches.iter().any(|touch| touch.state == TouchState::Start && !touch.handled.get().is_empty())));
            if accepted_press {
                if let Some(body) = target {
                    let flow = body.flow().to_owned();
                    self.selected = Some(flow.clone());
                    cx.widget_action(self.uid, IterationViewAction::SelectFlow { id: flow });
                }
                match event {
                    Event::MouseDown(_) => self.body_capture = body_at.clone(),
                    Event::TouchUpdate(event) if self.body_touch_capture.is_none() => {
                        if let Some(touch) = event.touches.iter().find(|touch| {
                            touch.state == TouchState::Start && !touch.handled.get().is_empty()
                        }) {
                            self.body_touch_capture = body_at.clone().map(|body| (touch.uid, body));
                        }
                    }
                    _ => {}
                }
            }
        }
        if matches!(event, Event::MouseUp(_)) {
            self.body_capture = None;
        }
        if let Event::TouchUpdate(event) = event {
            if self.body_touch_capture.as_ref().is_some_and(|(uid, _)| {
                event
                    .touches
                    .iter()
                    .any(|touch| touch.uid == *uid && touch.state == TouchState::Stop)
            }) {
                self.body_touch_capture = None;
            }
        }
        let scroll_handled =
            matches!(event, Event::Scroll(event) if event.handled_x.get() || event.handled_y.get());
        match event.hits(cx, self.area) {
            Hit::FingerDown(event) => {
                if let Some(flow) = self.width_grip_at(event.abs) {
                    self.navigation_drag = Some(NavigationDrag::LaneWidth {
                        width: self.lane_width(&flow),
                        flow,
                        start: event.abs,
                    });
                    self.pressed = None;
                    self.drag = None;
                    self.clear_app_hovers();
                    cx.set_cursor(MouseCursor::EwResize);
                    self.area.redraw(cx);
                } else if let Some(flow) = self.grip_at(event.abs) {
                    self.navigation_drag = Some(NavigationDrag::Terminal {
                        height: self.terminal_height(&flow),
                        flow,
                        start: event.abs,
                    });
                    cx.set_cursor(MouseCursor::NsResize);
                } else if let Some(flow) = self.mini_at(event.abs) {
                    self.navigate_minimap(cx, &flow, event.abs);
                    self.navigation_drag = Some(NavigationDrag::Minimap { flow });
                    cx.set_cursor(MouseCursor::Grabbing);
                } else if let Some(flow) = self.scrollbar_at(event.abs) {
                    if let Some(index) = self.lanes.iter().position(|lane| lane.id == flow) {
                        if let Some((track, thumb)) = self.scrollbar(index) {
                            if !thumb.contains(event.abs) {
                                let ratio = ((event.abs.y - track.pos.y - thumb.size.y * 0.5)
                                    / (track.size.y - thumb.size.y).max(1.0))
                                .clamp(0.0, 1.0);
                                self.set_scroll(
                                    &flow,
                                    ratio * (self.lanes[index].height - self.body_height(&flow)),
                                    false,
                                    cx,
                                );
                            }
                        }
                    }
                    self.navigation_drag = Some(NavigationDrag::Scrollbar {
                        offset: self.scroll.get(&flow).copied().unwrap_or(0.0),
                        flow,
                        start_y: event.abs.y,
                    });
                    cx.set_cursor(MouseCursor::Grabbing);
                } else if body_at.is_none() {
                    cx.set_key_focus(self.area);
                    self.drag = Some((event.abs, self.pan));
                    self.history_drag = self
                        .lane_at(event.abs)
                        .filter(|flow| {
                            !self.over_tray(event.abs)
                                && self
                                    .lanes
                                    .iter()
                                    .position(|lane| lane.id == *flow)
                                    .is_some_and(|index| self.lane_clip(index).contains(event.abs))
                        })
                        .map(|flow| {
                            let offset = self.scroll.get(&flow).copied().unwrap_or(0.0);
                            (flow, offset)
                        });
                    self.pressed = self.hit_at(event.abs);
                    cx.set_cursor(if self.pressed.is_some() {
                        MouseCursor::Hand
                    } else {
                        MouseCursor::Grabbing
                    });
                }
            }
            Hit::FingerMove(event) => {
                if let Some(navigation) = self.navigation_drag.clone() {
                    match navigation {
                        NavigationDrag::LaneWidth { flow, start, width } => {
                            self.set_lane_width(
                                cx,
                                &flow,
                                width + (event.abs.x - start.x) / self.zoom.max(0.01),
                            );
                            cx.set_cursor(MouseCursor::EwResize);
                        }
                        NavigationDrag::Terminal {
                            flow,
                            start,
                            height,
                        } => self.set_terminal_height(
                            cx,
                            &flow,
                            height - (event.abs.y - start.y) / self.zoom,
                        ),
                        NavigationDrag::Minimap { flow } => {
                            self.navigate_minimap(cx, &flow, event.abs)
                        }
                        NavigationDrag::Scrollbar {
                            flow,
                            start_y,
                            offset,
                        } => {
                            if let Some(index) = self.lanes.iter().position(|lane| lane.id == flow)
                            {
                                if let Some((track, thumb)) = self.scrollbar(index) {
                                    let delta = (event.abs.y - start_y)
                                        / (track.size.y - thumb.size.y).max(1.0)
                                        * (self.lanes[index].height - self.body_height(&flow));
                                    self.set_scroll(&flow, offset + delta, false, cx);
                                }
                            }
                        }
                    }
                } else if let Some((start, pan)) = self.drag {
                    if (event.abs - start).length() > 3.0 {
                        self.pan.x = pan.x + event.abs.x - start.x;
                        self.pan.y = 0.0;
                        if let Some((flow, offset)) = self.history_drag.clone() {
                            self.drag_history(
                                cx,
                                &flow,
                                offset - (event.abs.y - start.y) / self.zoom.max(0.01),
                            );
                        }
                        self.fitted = false;
                        self.area.redraw(cx);
                    }
                }
            }
            Hit::FingerUp(event) => {
                match self.navigation_drag.take() {
                    Some(NavigationDrag::LaneWidth { flow, .. }) => {
                        cx.widget_action(
                            self.uid,
                            IterationViewAction::LaneWidth {
                                width: self.lane_width(&flow),
                                flow,
                            },
                        );
                        self.area.redraw(cx);
                    }
                    Some(NavigationDrag::Terminal { flow, .. }) => {
                        cx.widget_action(
                            self.uid,
                            IterationViewAction::TerminalResized {
                                height: self.terminal_height(&flow),
                                flow,
                            },
                        );
                    }
                    _ => {}
                }
                let click = self
                    .drag
                    .take()
                    .is_some_and(|(start, _)| (event.abs - start).length() < 4.0);
                self.release_history_drag(cx);
                if click {
                    if let Some(action) = self
                        .pressed
                        .take()
                        .filter(|action| self.hit_at(event.abs).as_ref() == Some(action))
                    {
                        match &action {
                            IterationViewAction::ToggleSplit { flow } => {
                                self.cut_flow = if self.cut_flow.as_ref() == Some(flow) {
                                    None
                                } else if self.engine.as_ref().is_some_and(|engine| {
                                    engine
                                        .flows
                                        .get(flow)
                                        .is_some_and(|flow| flow.lifecycle == FlowLifecycle::Active)
                                }) {
                                    Some(flow.clone())
                                } else {
                                    None
                                };
                                self.cut_hover = None;
                                self.minimap_open = None;
                                self.clear_app_hovers();
                                cx.set_key_focus(self.area);
                            }
                            IterationViewAction::SplitFlow { .. } => {
                                self.cut_flow = None;
                                self.cut_hover = None;
                            }
                            IterationViewAction::SelectFlow { id } => {
                                self.selected = Some(id.clone())
                            }
                            IterationViewAction::ToggleNavigator { flow } => {
                                self.minimap_open = if self.minimap_open.as_ref() == Some(flow) {
                                    None
                                } else {
                                    Some(flow.clone())
                                };
                                self.clear_app_hovers();
                                cx.set_key_focus(self.area);
                            }
                            IterationViewAction::FoldBuild { flow, artifact } => {
                                self.fold(cx, flow, artifact)
                            }
                            IterationViewAction::FoldItem { flow, key } => self.fold(cx, flow, key),
                            IterationViewAction::ToggleAttachmentTray { flow } => {
                                if !self.collapsed_trays.remove(flow) {
                                    self.collapsed_trays.insert(flow.clone());
                                }
                                self.clear_app_hovers();
                            }
                            IterationViewAction::TestTileToggled {
                                flow,
                                id,
                                open: true,
                                ..
                            } => {
                                self.clear_app_hovers();
                                self.pixel_view = Some((flow.clone(), id.clone()));
                                self.pixel_pan = dvec2(0.0, 0.0);
                                self.pixel_drag = None;
                                self.full_preview = None;
                                cx.set_key_focus(self.area);
                            }
                            _ => {}
                        }
                        cx.widget_action(self.uid, action);
                        self.area.redraw(cx);
                    }
                }
                self.pressed = None;
            }
            Hit::FingerScroll(event) if !scroll_handled && (body_at.is_none() || zoom_scroll) => {
                if event.modifiers.control || event.modifiers.logo {
                    self.zoom_at(
                        cx,
                        event.abs,
                        (-event.scroll.y * 0.005).exp().clamp(0.1, 10.0),
                    );
                } else if event.modifiers.shift {
                    self.pan.x -= if event.scroll.x.abs() > event.scroll.y.abs() {
                        event.scroll.x
                    } else {
                        event.scroll.y
                    };
                    self.fitted = false;
                    self.area.redraw(cx);
                } else if let Some(id) = self.lane_at(event.abs) {
                    let offset = self
                        .scroll_target
                        .get(&id)
                        .or_else(|| self.scroll.get(&id))
                        .copied()
                        .unwrap_or(0.0);
                    self.set_scroll(&id, offset + event.scroll.y / self.zoom.max(0.01), true, cx);
                }
            }
            Hit::FingerHoverIn(event) | Hit::FingerHoverOver(event) => {
                let hovered = self.hit_at(event.abs);
                let cut_hover = match &hovered {
                    Some(IterationViewAction::SplitFlow { before, .. }) => Some(before.clone()),
                    _ => None,
                };
                let width_hover = self.width_grip_at(event.abs);
                if self.width_hover != width_hover || self.cut_hover != cut_hover {
                    self.width_hover = width_hover;
                    self.cut_hover = cut_hover;
                    self.area.redraw(cx);
                }
                if self.width_hover.is_some() {
                    cx.set_cursor(MouseCursor::EwResize);
                } else if self.grip_at(event.abs).is_some() {
                    cx.set_cursor(MouseCursor::NsResize);
                } else if self.mini_at(event.abs).is_some()
                    || self.scrollbar_at(event.abs).is_some()
                {
                    cx.set_cursor(MouseCursor::Hand);
                } else if body_at.is_none() {
                    cx.set_cursor(if hovered.is_some() {
                        MouseCursor::Hand
                    } else {
                        MouseCursor::Grab
                    });
                }
                if self.hovered != hovered {
                    cx.widget_action(self.uid, TipAction::HoverOut);
                    if let Some((text, rect)) = hovered.as_ref().and_then(|action| {
                        let text = self.action_tip(action)?;
                        let target = self
                            .targets
                            .iter()
                            .rev()
                            .find(|target| &target.action == action)?;
                        Some((text, target.rect))
                    }) {
                        cx.widget_action(self.uid, TipAction::HoverIn(text.to_owned(), rect));
                    }
                    self.hovered = hovered;
                    self.area.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                self.cut_hover = None;
                if self.width_hover.take().is_some() {
                    self.area.redraw(cx);
                }
                cx.widget_action(self.uid, TipAction::HoverOut);
                if self.hovered.take().is_some() {
                    self.area.redraw(cx);
                }
            }
            Hit::KeyDown(event) if event.key_code == KeyCode::KeyF => self.fit(cx),
            Hit::KeyFocusLost(_) => {
                if self.cut_flow.take().is_some() {
                    self.cut_hover = None;
                    self.area.redraw(cx);
                }
                if self.minimap_open.take().is_some() {
                    self.area.redraw(cx);
                }
            }
            Hit::KeyDown(event) if event.key_code == KeyCode::Escape => {
                if self.cut_flow.take().is_some() {
                    self.cut_hover = None;
                    self.area.redraw(cx);
                }
                if self.minimap_open.take().is_some() {
                    self.area.redraw(cx);
                }
                self.drag = None;
                self.release_history_drag(cx);
                self.navigation_drag = None;
                self.pressed = None;
            }
            _ => {}
        }
        let focused = self
            .terminals
            .iter()
            .find(|(_, host)| Self::terminal_focused(cx, &host.widget))
            .map(|(flow, _)| flow.clone());
        if self.focused_terminal != focused {
            self.focused_terminal = focused;
            self.area.redraw(cx);
        }
    }
}

fn image_drop_path(item: &DragItem) -> bool {
    let DragItem::FilePath { path, .. } = item else {
        return false;
    };
    if path.len() > 4096 || path.chars().any(char::is_control) {
        return false;
    }
    let path = std::path::Path::new(path);
    path.is_absolute()
        && path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                matches!(
                    extension.to_ascii_lowercase().as_str(),
                    "png"
                        | "jpg"
                        | "jpeg"
                        | "webp"
                        | "gif"
                        | "bmp"
                        | "tif"
                        | "tiff"
                        | "heic"
                        | "avif"
                )
            })
}

fn live_input(event: &Event) -> bool {
    event.requires_visibility()
        || matches!(
            event,
            Event::MouseUp(_)
                | Event::MouseLeave(_)
                | Event::LongPress(_)
                | Event::SelectionHandleDrag(_)
                | Event::KeyDown(_)
                | Event::KeyUp(_)
                | Event::TextInput(_)
                | Event::TextRangeReplace(_)
                | Event::TextCopy(_)
                | Event::TextCut(_)
                | Event::ImeAction(_)
                | Event::Drag(_)
                | Event::Drop(_)
                | Event::DragEnd
        )
}

fn bounded_recordings(tiles: Vec<TestTile>, limit: usize) -> Vec<TestTile> {
    let mut accepted: Vec<TestTile> = vec![];
    for tile in tiles {
        if tile.id.is_empty()
            || tile.id.len() > 96
            || tile.title.len() > 120
            || tile.preview_id.is_empty()
            || tile.preview_id.len() > 128
            || tile.width == 0
            || tile.height == 0
            || tile.width > 8192
            || tile.height > 8192
            || accepted.iter().any(|item| item.id == tile.id)
        {
            continue;
        }
        accepted.push(tile);
        if accepted.len() >= limit {
            break;
        }
    }
    accepted
}

fn bounded_attachments(ids: Vec<String>, limit: usize) -> Vec<String> {
    let mut accepted = vec![];
    for id in ids.into_iter().rev() {
        if !id.is_empty()
            && id.len() <= 128
            && !id.chars().any(char::is_control)
            && !accepted.contains(&id)
        {
            accepted.push(id);
        }
        if accepted.len() >= limit {
            break;
        }
    }
    accepted
}
fn intersect_rect(a: Rect, b: Rect) -> Rect {
    let x = a.pos.x.max(b.pos.x);
    let y = a.pos.y.max(b.pos.y);
    rect(
        x,
        y,
        (a.pos.x + a.size.x).min(b.pos.x + b.size.x).max(x) - x,
        (a.pos.y + a.size.y).min(b.pos.y + b.size.y).max(y) - y,
    )
}
fn blend_color(a: Vec4f, b: Vec4f, t: f32) -> Vec4f {
    vec4(
        a.x + (b.x - a.x) * t,
        a.y + (b.y - a.y) * t,
        a.z + (b.z - a.z) * t,
        1.0,
    )
}
