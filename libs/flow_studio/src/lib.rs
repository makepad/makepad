//! Flow's generation studio over the AI Hub: the job API every app that
//! generates calls, and the page that shows it.
//!
//! Left, the preset library: every prefab workflow as a card, grouped
//! (LIVE, IMAGE, VIDEO, AUDIO · SPEECH, MUSIC, TEXT · VISION, 3D, UTILITY,
//! SAVED), with what it takes, where its results land and whether the hub
//! has its models. Centre, the canvas: the chosen preset's graph with each
//! node's run state and, on the faces of finished nodes, what they made.
//! Right, the inspector: the preset's inputs and its generators'
//! parameters, the models it needs with their licences, the runs of this
//! session and their published results.
//!
//! The host's live roots ([`LiveRoots`]) are listed as LIVE: selecting one
//! shows its running graph as it runs (`live.rs`). Nothing here
//! instantiates a live root. Every other preset fires up as a job flow on
//! the studio's own engine (`engine.rs`); its results publish through
//! `Publish` nodes into the host's result store, tagged so the host finds
//! them (`prepare.rs`, [`host::Place`]). SAVE AS PRESET writes the
//! customised graph back to the store as a flow preset. The host (results,
//! node types, its own presets) is [`host::StudioHost`].
//!
//! Main-loop cost: the pump drains at most a bounded batch of engine
//! replies/transitions and worker results (small messages; pictures arrive
//! decoded and shrunk to preview size), uploads at most two textures, and
//! rebuilds card rows only when the catalogue, the filter or the hub
//! snapshot changed. Splash is evaluated, written and published on the
//! studio worker, never on the UI thread; nothing here waits.

mod api;
mod canvas;
mod cards;
mod editors;
mod engine;
pub mod host;
mod live;
mod models;
pub mod nodes;
#[cfg(not(target_arch = "wasm32"))]
pub mod headless;
mod prepare;
pub mod presets;
mod preview_gen;
pub mod testpattern;
pub mod work;
mod worker;

pub use live::{LiveRoots, NoLiveRoots, Phase, RootState};
use canvas::{FaceContent, StudioCanvasAction, StudioCanvasWidgetRefExt};
use cards::{CardRow, CardsAction, StudioCardsWidgetRefExt};
use engine::{NodeEvent, RunSpec, StudioEngine, StudioEvent};
pub use makepad_flow::engine::results::ResultRef;
use makepad_flow::engine::results::ResultStore;
use makepad_ai_hub::job::FixedRouter;
use makepad_flow::engine::{HubChat, HubHttp, Seams};
use makepad_flow::{Graph, Literal, NodeState, PortType, PortValue, RunState};
use makepad_flowgraph::NodeStatus;
use makepad_flow_ui::graph_edit;
use makepad_widgets::*;
use models::{GenMode, HubSnapshot, NeedState, Readiness};
use prepare::Provenance;
use presets::{Group, Preset};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use worker::{Done, Face, Job, MediaPick, StudioWorker, Thumb};
pub use api::{gen_key, parse_result_ref, ChosenPreset, EstimateInfo, FlowChat, JobEvent, JobPhase, JobSnapshot, PresetInfo, RunRequest};

/// Textures made per pump.
const UPLOADS_PER_PUMP: usize = 2;
/// Worker and engine results handled per pump.
const RESULTS_PER_PUMP: usize = 24;
/// How soon to ask the result store again while it starts.
const BOOT_RETRY_SECS: f64 = 1.0;
/// How often the hub is asked about its models while FLOW shows.
const PROBE_SECS: f64 = 10.0;
/// Parameters worth a row, in the order they are offered.
pub(crate) const PARAMS: [&str; 13] = ["width", "height", "steps", "seed", "seconds", "frames", "voice", "speed", "strength", "upscale", "interpolate", "temperature", "model"];

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    let stage = mod.stage

    let Pane = RoundedView{width: Fill height: Fill flow: Down spacing: 0 padding: 0 show_bg: true
        draw_bg +: {color: stage.panel border_color: stage.separator border_size: 1 border_radius: 5}}
    let PaneHead = View{width: Fill height: 34 flow: Right spacing: 6 padding: Inset{left: 12 right: 8} align: Align{y: 0.5}}
    let Rule = SolidView{width: Fill height: 1 draw_bg +: {color: stage.separator}}
    let Caps = StageTick{width: Fit}
    let Tool = StageButton{width: Fit height: 22 margin: 0 padding: Inset{left: 8 right: 8}}
    let Chip = StageButton{width: Fit height: 20 margin: 0 padding: Inset{left: 7 right: 7}
        draw_text +: {text_style: theme.font_bold{font_size: 7}}}
    let Note = Label{width: Fill height: Fit text: "" draw_text +: {color: stage.secondary text_style: theme.font_regular{font_size: 9}}}
    let Section = Label{width: Fill height: Fit padding: Inset{left: 12 top: 10 bottom: 4} text: ""
        draw_text +: {color: stage.label text_style: theme.font_bold{font_size: 8}}}

    mod.widgets.FlowStudioPage = View{width: Fill height: Fill flow: Right spacing: 8
        // The preset library.
        Pane{width: 340
            PaneHead{Caps{text: "PRESETS"} View{width: Fill height: 1}
                Tip{text: "Search presets by name, description or model" studio_search := TextInput{width: 160 height: 22 margin: 0
                    padding: Inset{left: 6 right: 6 top: 3 bottom: 3} empty_text: "Search"
                    draw_text +: {text_style: theme.font_regular{font_size: 9}}}}}
            Rule{}
            View{width: Fill height: Fit flow: Flow.Right{wrap: true} spacing: 4 padding: Inset{left: 8 right: 8 top: 6 bottom: 6}
                studio_group_all := Chip{text: "ALL"}
                studio_group_live := Chip{text: "LIVE"}
                studio_group_image := Chip{text: "IMAGE"}
                studio_group_video := Chip{text: "VIDEO"}
                studio_group_audio := Chip{text: "AUDIO"}
                studio_group_music := Chip{text: "MUSIC"}
                studio_group_text := Chip{text: "TEXT"}
                studio_group_mesh := Chip{text: "3D"}
                studio_group_utility := Chip{text: "UTILITY"}
                studio_group_saved := Chip{text: "SAVED"}
            }
            Rule{}
            studio_presets := StudioCards{}
            Rule{}
            View{width: Fill height: Fit padding: Inset{left: 10 right: 10 top: 6 bottom: 8}
                studio_hub := Note{text: "checking the AI Hub…"}}
        }
        // The graph.
        Pane{width: Fill
            PaneHead{
                studio_title := Label{width: Fit max_lines: 1 text: "Choose a preset" draw_text +: {color: stage.text_hi text_style: theme.font_bold{font_size: 10}}}
                studio_state := Label{width: Fill max_lines: 1 text: "" draw_text +: {color: stage.dim text_style: theme.font_regular{font_size: 9}}}
                Tip{text: "Run the preset with its defaults (its card in the chat sets prompt and params); while it runs: Cancel" studio_run := Tool{text: "Run"}}
                Tip{text: "Fit the graph to the canvas" studio_fit := Tool{text: "Fit"}}
            }
            Rule{}
            View{width: Fill height: Fill padding: 2
                studio_canvas := StudioCanvas{}
            }
            Rule{}
            View{width: Fill height: 28 flow: Right padding: Inset{left: 12 right: 12} align: Align{y: 0.5}
                studio_status := Label{width: Fill max_lines: 1 text: "" draw_text +: {color: stage.secondary text_style: theme.font_regular{font_size: 9}}}}
        }
        // The inspector.
        Pane{width: 360
            PaneHead{
                studio_inspect := Label{width: Fill max_lines: 1 text: "Preset" draw_text +: {color: stage.text_hi text_style: theme.font_bold{font_size: 10}}}
            }
            Rule{}
            ScrollYView{width: Fill height: Fill flow: Down
                studio_brief := View{width: Fill height: Fit padding: Inset{left: 12 right: 12 top: 8 bottom: 2}
                    studio_brief_text := Note{}}
                studio_live_box := View{visible: false width: Fill height: Fit flow: Down padding: Inset{left: 12 right: 12 top: 8}
                    studio_live_text := Note{}}
                studio_models_head := Section{text: "MODELS"}
                View{width: Fill height: Fit padding: Inset{left: 12 right: 12 bottom: 4}
                    studio_models := Note{}}
                Section{text: "RUNS"}
                View{width: Fill height: 150 padding: Inset{left: 2 right: 2} studio_runs := StudioCards{}}
                Section{text: "RESULTS"}
                View{width: Fill height: 190 padding: Inset{left: 2 right: 2 bottom: 6} studio_results := StudioCards{}}
            }
        }
    }
}

/// The page's widgets, then the page.
pub fn register(vm: &mut ScriptVm) {
    cards::script_mod(vm);
    canvas::script_mod(vm);
    script_mod(vm);
}

const GROUP_CHIPS: [(LiveId, Option<Group>); 10] = [
    (live_id!(studio_group_all), None),
    (live_id!(studio_group_live), Some(Group::Live)),
    (live_id!(studio_group_image), Some(Group::Image)),
    (live_id!(studio_group_video), Some(Group::Video)),
    (live_id!(studio_group_audio), Some(Group::Audio)),
    (live_id!(studio_group_music), Some(Group::Music)),
    (live_id!(studio_group_text), Some(Group::Text)),
    (live_id!(studio_group_mesh), Some(Group::Mesh)),
    (live_id!(studio_group_utility), Some(Group::Utility)),
    (live_id!(studio_group_saved), Some(Group::Saved)),
];

/// What happened on the page that the host must act on.
#[derive(Default)]
pub struct StudioOutcome {
    /// Music or audio landed in the store: its audio lists ask again.
    pub refresh_audio: bool,
    /// Pictures or video landed: its picture lists ask again.
    pub refresh_pictures: bool,
    /// A preset card was clicked: its key. The host opens the chat with
    /// the preset's gen_run card (generation happens in the chat).
    pub propose: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
enum Selected {
    Preset(String),
    Live(usize),
}

/// A media input's choice.
#[derive(Clone, Debug)]
struct Picked {
    reference: ResultRef,
}

/// One generator setting a preset offers: which node and key it writes.
#[derive(Clone, Debug)]
struct ParamSlot {
    node: String,
    key: String,
}

#[derive(Clone, Debug, PartialEq)]
enum RunPhase {
    Waiting,
    Preparing,
    Loading,
    Running,
    Done,
    Failed(String),
    Cancelled,
}

#[derive(Clone, Debug)]
struct PublishedAsset {
    node: String,
    reference: ResultRef,
    title: String,
    kind: String,
    place: &'static str,
}

struct RunRecord {
    id: u64,
    /// Who fired it: empty for the page, else the caller's name (the AI).
    origin: String,
    /// Run only on LAN nodes that have the models (no preview fallback).
    lan_only: bool,
    /// What happened, for an API caller to take (`take_job_events`).
    events: Vec<JobEvent>,
    /// Each fan-out's candidate values, in order, for an API caller's pick.
    candidate_values: HashMap<String, Vec<makepad_flow::Value>>,
    preset: String,
    label: String,
    phase: RunPhase,
    started: f64,
    secs: f64,
    /// What the run executes, once the engine evaluated it.
    graph: Option<Arc<Graph>>,
    /// The inputs as fired, so a run can be taken up again.
    work_graph: Arc<Graph>,
    statuses: HashMap<String, NodeStatus>,
    faces: HashMap<String, FaceContent>,
    published: Vec<PublishedAsset>,
    /// A choice gate waiting on the user: the Choose node, its question
    /// and how many candidates it offers.
    choice: Option<(String, String, usize)>,
    /// Each fan-out's candidates, in order, as their pictures arrive.
    candidates: HashMap<String, Vec<Option<Texture>>>,
    /// `node → (seed, model, AI Hub node)` the generators reported.
    used: HashMap<String, (String, String, String)>,
    warnings: Vec<String>,
}

pub struct FlowStudio {
    worker: Option<StudioWorker>,
    engine: Option<StudioEngine>,
    /// The engine for runs that must stay on the LAN (no preview fallback).
    lan_engine: Option<StudioEngine>,
    results: Option<Arc<dyn ResultStore>>,
    /// An API caller is using the studio: keep the hub probed while hidden.
    api_active: bool,
    /// When to ask for the result store again (it was not up yet).
    boot_retry_at: Option<f64>,
    mode: GenMode,
    /// The worker's outbox: jobs its queue had no room for, in order.
    outbox: VecDeque<Job>,
    booted: bool,
    assets_note: String,
    presets: Vec<Preset>,
    saved: Vec<Preset>,
    catalogue_error: Option<String>,
    snapshot: HubSnapshot,
    probe_at: f64,
    probing: bool,
    filter: Option<Group>,
    search: String,
    selected: Option<Selected>,
    /// The preset the page shows (its graph is read-only).
    chosen: Option<Preset>,
    /// A preset card was clicked since the last pump.
    proposed: Option<String>,
    runs: Vec<RunRecord>,
    shown_run: Option<u64>,
    next_run: u64,
    /// Prepared runs waiting for the engine to be ready.
    pending_specs: Vec<(u64, String, Vec<(String, String, makepad_flow::Value)>)>,
    /// Faces made while a record was borrowed, put on in the same pump.
    pending_faces: Vec<(u64, String, FaceContent)>,
    /// Decoded pictures waiting for a texture: `(run, node, thumb, caption)`.
    uploads: VecDeque<(u64, String, Thumb, String)>,
    rows_dirty: bool,
    inspector_dirty: bool,
    live_revisions: Vec<(usize, String)>,
    /// The selected live root on the canvas, with its pictures.
    live: live::LiveView,
    /// The selected live root's label.
    live_label: String,
    notice: String,
    outcome: StudioOutcome,
    stopping: bool,
    /// The chosen preset's node edits (`editors.rs`).
    draft: editors::Draft,
    /// The node whose own Run fired the run shown, and that run.
    node_run: Option<(String, u64)>,
    /// A run to put on the canvas at the next pump (started elsewhere).
    show_next: Option<u64>,
}

impl Default for FlowStudio {
    fn default() -> Self {
        Self {
            worker: None,
            engine: None,
            lan_engine: None,
            results: None,
            api_active: false,
            boot_retry_at: None,
            mode: GenMode::from_env(),
            outbox: VecDeque::new(),
            booted: false,
            assets_note: String::new(),
            presets: Vec::new(),
            saved: Vec::new(),
            catalogue_error: None,
            snapshot: HubSnapshot::default(),
            probe_at: 0.0,
            probing: false,
            filter: None,
            search: String::new(),
            selected: None,
            chosen: None,
            proposed: None,
            runs: Vec::new(),
            shown_run: None,
            next_run: 1,
            pending_specs: Vec::new(),
            pending_faces: Vec::new(),
            uploads: VecDeque::new(),
            rows_dirty: true,
            inspector_dirty: true,
            live_revisions: Vec::new(),
            live: live::LiveView::default(),
            live_label: String::new(),
            notice: String::new(),
            outcome: StudioOutcome::default(),
            stopping: false,
            draft: editors::Draft::default(),
            node_run: None,
            show_next: None,
        }
    }
}

fn group_tint(p: &makepad_look::palette::Palette, group: Group) -> u32 {
    match group {
        Group::Live => p.live,
        Group::Image => p.amber,
        Group::Video => p.accent,
        Group::Audio => p.info,
        Group::Music => p.teal,
        Group::Text => p.secondary,
        Group::Mesh => p.warm,
        Group::Utility => p.muted,
        Group::Saved => p.saved,
    }
}

fn texture_of(cx: &mut Cx, thumb: &Thumb) -> Texture {
    Texture::new_with_format(cx, TextureFormat::VecBGRAu8_32 {
        width: thumb.width, height: thumb.height, data: Some((*thumb.pixels).clone()), updated: TextureUpdated::Full,
    })
}

impl FlowStudio {
    fn send(&mut self, job: Job) {
        let Some(worker) = &self.worker else { return };
        if !self.outbox.is_empty() {
            self.outbox.push_back(job);
            return;
        }
        if let Err(job) = worker.try_send(job) {
            if worker.alive() {
                self.outbox.push_back(job);
            }
        }
    }

    fn flush_outbox(&mut self) {
        let Some(worker) = &self.worker else { return };
        while let Some(job) = self.outbox.pop_front() {
            if let Err(job) = worker.try_send(job) {
                self.outbox.push_front(job);
                break;
            }
        }
    }

    /// First showing: the worker, the catalogue, the store and the hub.
    fn boot(&mut self, cx: &mut Cx) {
        if self.worker.is_some() || self.stopping {
            return;
        }
        let worker = StudioWorker::new(cx);
        if let Some(error) = &worker.error {
            self.notice = error.clone();
        }
        self.worker = Some(worker);
        self.send(Job::Boot);
        self.send(Job::Catalogue);
        self.send(Job::Saved);
        self.probing = true;
        self.send(Job::Probe(self.mode.clone()));
    }

    fn seams(&self, fallback: bool) -> Seams {
        match &self.mode {
            // The LAN's nodes; a generator no node offers runs as a preview
            // (with `fallback`) or fails.
            GenMode::Fleet => Seams { gen: Arc::new(preview_gen::LanOrPreview { fallback }), ..Seams::real() },
            GenMode::Fixed(url) => Seams { chat: Arc::new(HubChat::from_env()), gen: Arc::new(FixedRouter(url.clone())), http: Arc::new(HubHttp) },
            GenMode::Preview => Seams { chat: Arc::new(crate::testpattern::TestpatternChat), gen: Arc::new(preview_gen::PreviewGen), http: Arc::new(HubHttp) },
        }
    }

    pub fn stop(&mut self) {
        self.stopping = true;
        for engine in [&mut self.engine, &mut self.lan_engine].into_iter().flatten() {
            engine.stop();
        }
        if let Some(worker) = &mut self.worker {
            worker.stop();
        }
        self.outbox.clear();
    }

    /// The engine has let go of its runs (or never started).
    pub fn stopped(&self) -> bool {
        self.engine.as_ref().is_none_or(StudioEngine::stopped) && self.lan_engine.as_ref().is_none_or(StudioEngine::stopped)
    }

    // ---- the catalogue -------------------------------------------------

    fn all_presets(&self) -> impl Iterator<Item = &Preset> {
        self.presets.iter().chain(self.saved.iter())
    }

    fn preset(&self, key: &str) -> Option<&Preset> {
        self.all_presets().find(|preset| preset.key == key)
    }

    fn matches(&self, preset: &Preset) -> bool {
        if self.filter.is_some_and(|group| group != preset.group) {
            return false;
        }
        let search = self.search.trim().to_lowercase();
        search.is_empty() || [preset.label.as_str(), preset.brief.as_str(), preset.key.as_str()].iter()
            .any(|text| text.to_lowercase().contains(&search))
            || preset.needs.iter().any(|need| need.model.to_lowercase().contains(&search) || need.domain.contains(&search))
    }

    fn preset_rows(&self, cx: &mut Cx, runtime: &dyn LiveRoots) -> Vec<CardRow> {
        let p = makepad_look::palette::get(cx);
        let mut rows = Vec::new();
        for group in Group::ALL {
            if self.filter.is_some_and(|filter| filter != group) {
                continue;
            }
            let mut cards = Vec::new();
            if group == Group::Live {
                for root in 0..runtime.count() {
                    let label = format!("{} root", runtime.label(root));
                    if !self.search.trim().is_empty() && !label.to_lowercase().contains(&self.search.trim().to_lowercase()) {
                        continue;
                    }
                    let state = runtime.state(root);
                    let (badge, badge_tint, detail) = match &state.phase {
                        Phase::Running => ("LIVE", p.live, format!("running revision {} · edited in place", state.revision)),
                        Phase::Loading | Phase::Instantiating => ("STARTING", p.amber, "starting".to_string()),
                        Phase::Unlinked => ("OFF", p.dim, "not in this build".to_string()),
                        Phase::Failed(error) => ("FAILED", p.danger, error.clone()),
                    };
                    cards.push(CardRow {
                        key: format!("live:{}", runtime.definition(root)), title: label, detail,
                        glyph: Group::Live.glyph().into(), tint: group_tint(&p, group),
                        badge: badge.into(), badge_tint, quiet: !runtime.linked(root), ..Default::default()
                    });
                }
            }
            for preset in self.all_presets().filter(|preset| preset.group == group && self.matches(preset)) {
                let readiness = models::readiness(&preset.needs, &self.snapshot);
                let badge_tint = match readiness {
                    Readiness::Ready | Readiness::Local => p.live,
                    Readiness::Preview => p.info,
                    Readiness::NoHub => p.amber,
                    Readiness::Missing(_) => p.danger,
                };
                let restricted = preset.needs.iter().any(|need| models::license(&need.model).is_some_and(|(_, warning)| warning.is_some()));
                let problems = models::graph_problems(&preset.graph, &self.snapshot);
                let mut detail = preset.summary();
                if readiness == Readiness::Ready {
                    let nodes = models::nodes_for(&preset.needs, &self.snapshot);
                    if !nodes.is_empty() {
                        detail.push_str(&format!(" · on {}", nodes.join(", ")));
                    }
                }
                if restricted {
                    detail.push_str(" · restricted licence");
                }
                if let Some(problem) = problems.first() {
                    detail = problem.clone();
                }
                cards.push(CardRow {
                    key: preset.key.clone(), title: preset.label.clone(), detail,
                    glyph: group.glyph().into(), tint: group_tint(&p, group),
                    badge: match (&self.snapshot.mode, &readiness) {
                        _ if !problems.is_empty() => "INCOMPATIBLE".into(),
                        (None, _) => String::new(),
                        // On the LAN a generator no node offers runs as a preview.
                        (Some(GenMode::Fleet), Readiness::Missing(_) | Readiness::NoHub) => "PREVIEW ONLY".into(),
                        _ => readiness.badge().into(),
                    }, badge_tint: if problems.is_empty() { badge_tint } else { p.danger },
                    quiet: matches!(readiness, Readiness::Missing(_)) || !problems.is_empty(),
                    ..Default::default()
                });
            }
            if !cards.is_empty() {
                rows.push(CardRow { key: format!("group:{}", group.label()), header: true, title: format!("{}  {}", group.label(), cards.len()), ..Default::default() });
                rows.extend(cards);
            }
        }
        rows
    }

    // ---- choosing a preset ----------------------------------------------

    fn select(&mut self, cx: &mut Cx, ui: &WidgetRef, key: &str, runtime: &dyn LiveRoots) {
        if let Some(root) = (0..runtime.count()).find(|root| key == format!("live:{}", runtime.definition(*root))) {
            self.selected = Some(Selected::Live(root));
            self.live_label = runtime.label(root).to_string();
            self.chosen = None;
            self.shown_run = None;
            let canvas = ui.studio_canvas(cx, ids!(studio_canvas));
            self.live.forget(cx, &canvas);
            canvas.clear_faces(cx);
            canvas.clear_run(cx);
            // Its running graph, and from here on the pump keeps it current.
            self.live.sync(cx, &canvas, runtime, root, true);
            self.inspector_dirty = true;
            return;
        }
        let Some(preset) = self.preset(key).cloned() else { return };
        self.live.forget(cx, &ui.studio_canvas(cx, ids!(studio_canvas)));
        if self.chosen.as_ref().is_none_or(|chosen| chosen.key != preset.key) {
            self.draft = editors::Draft::default();
            self.node_run = None;
        }
        self.selected = Some(Selected::Preset(key.to_string()));
        self.chosen = Some(preset);
        // Generating happens in the chat: the product opens the preset's card there.
        self.proposed = Some(key.to_string());
        self.shown_run = None;
        let canvas = ui.studio_canvas(cx, ids!(studio_canvas));
        canvas.clear_faces(cx);
        canvas.clear_run(cx);
        canvas.set_graph(cx, self.chosen.as_ref().map(|preset| &*preset.graph), true);
        self.inspector_dirty = true;
    }

    /// The run shown on the canvas while it is still under way.
    fn running_shown(&self) -> Option<u64> {
        let run = self.shown_run.and_then(|id| self.runs.iter().find(|run| run.id == id))?;
        matches!(run.phase, RunPhase::Waiting | RunPhase::Preparing | RunPhase::Loading | RunPhase::Running).then_some(run.id)
    }

    /// Run: the chosen preset with the gen_run card's own prefill, through
    /// the functions gen_run calls (gen_request, run_preset); Cancel while
    /// it runs (gen_cancel's cancel_job).
    fn run_button(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        if let Some(id) = self.running_shown() {
            self.cancel_job(id);
            return;
        }
        let Some(preset) = self.chosen.clone().filter(|_| !matches!(self.selected, Some(Selected::Live(_)))) else { return };
        // The page's own edits on the nodes; the chat's card has its own.
        let request = self.draft_request(&preset);
        let started = self.run_preset(cx, &preset.key, request);
        self.node_run = None;
        match started {
            Ok(id) => {
                self.notice.clear();
                self.show_run(cx, ui, Some(id));
            }
            Err(error) => self.notice = format!("Run: {error}"),
        }
        self.inspector_dirty = true;
    }

    /// Show a past run: its preset and its graph as fired.
    fn take_up_run(&mut self, cx: &mut Cx, ui: &WidgetRef, id: u64) {
        let Some(record) = self.runs.iter().find(|run| run.id == id) else { return };
        let Some(preset) = self.preset(&record.preset).cloned() else { return };
        self.live.forget(cx, &ui.studio_canvas(cx, ids!(studio_canvas)));
        self.selected = Some(Selected::Preset(preset.key.clone()));
        self.chosen = Some(preset);
        self.show_run(cx, ui, Some(id));
        self.inspector_dirty = true;
        self.rows_dirty = true;
    }

    /// Put a run (or the chosen preset's graph) on the canvas.
    fn show_run(&mut self, cx: &mut Cx, ui: &WidgetRef, id: Option<u64>) {
        self.shown_run = id;
        let canvas = ui.studio_canvas(cx, ids!(studio_canvas));
        canvas.clear_faces(cx);
        canvas.clear_run(cx);
        let record = id.and_then(|id| self.runs.iter().find(|run| run.id == id));
        match record {
            Some(record) => {
                let graph = record.graph.clone().unwrap_or_else(|| record.work_graph.clone());
                canvas.set_graph(cx, Some(&graph), true);
                for (node, status) in &record.statuses {
                    canvas.set_status(cx, node, status.clone());
                }
                for (node, face) in &record.faces {
                    canvas.set_face(cx, node, face.clone());
                }
            }
            None => {
                canvas.set_graph(cx, self.chosen.as_ref().map(|preset| &*preset.graph), true);
            }
        }
    }

    // ---- firing up --------------------------------------------------------

    /// The graph a run executes: `text` as the text/list/JSON inputs'
    /// values, media inputs left to arrive with the run.
    fn input_graph(preset: &Preset, base: &Graph, text: &HashMap<String, String>) -> Result<Graph, String> {
        let mut graph = base.clone();
        for (node, value) in text {
            let ty = preset.inputs.iter().find(|input| &input.node == node).map(|input| input.ty.clone());
            let literal = match ty {
                Some(PortType::Text) | None => Literal::Str(value.clone()),
                Some(ty) => presets::json_literal(value).map_err(|error| format!("{node} takes {} as JSON: {error}", ty.as_str()))?,
            };
            graph = graph_edit::set_param(&graph, node, "value", literal);
        }
        // A media input's value arrives with the run (`SetInputs`); its
        // design value is none, so the instance takes no literal for it.
        for input in preset.inputs.iter().filter(|input| input.ty.is_media()) {
            graph = graph_edit::set_param(&graph, &input.node, "value", Literal::Null);
        }
        Ok(graph)
    }

    /// Fire a run: record it in the history and hand it to the worker to
    /// prepare. The page and the API both start runs here.
    fn start_run(&mut self, preset: Preset, base: &Graph, text: HashMap<String, String>, media: HashMap<String, Picked>,
        origin: String, lan_only: bool, tags: Vec<String>) -> Result<u64, String> {
        let missing: Vec<String> = preset.inputs.iter()
            .filter(|input| input.ty.is_media() && !media.contains_key(&input.node))
            .map(|input| input.label.clone()).collect();
        if !missing.is_empty() {
            return Err(format!("choose the inputs first: {}", missing.join(", ")));
        }
        let graph = Self::input_graph(&preset, base, &text)?;
        // Never send a job a model cannot take: say why instead.
        let problems = models::graph_problems(&graph, &self.snapshot);
        if !problems.is_empty() {
            return Err(format!("not run: {}", problems.join("; ")));
        }
        let graph = Arc::new(graph);
        let prompt = preset.inputs.iter().find(|input| input.ty == PortType::Text)
            .and_then(|input| text.get(&input.node)).cloned().unwrap_or_default();
        let provenance = Provenance { preset: preset.key.clone(), label: preset.label.clone(), prompt, tags };
        let picks: Vec<MediaPick> = preset.inputs.iter().filter_map(|input| {
            let picked = media.get(&input.node)?;
            Some(MediaPick { node: input.node.clone(), port: input.ty.as_str().to_string(), ty: input.ty.clone(), reference: picked.reference.clone() })
        }).collect();
        let id = self.next_run;
        self.next_run += 1;
        let label = if origin.is_empty() { preset.label.clone() } else { format!("{origin} · {}", preset.label) };
        let record = RunRecord {
            id, origin, lan_only, events: Vec::new(), candidate_values: HashMap::new(),
            preset: preset.key.clone(), label, phase: RunPhase::Preparing,
            started: Cx::time_now(), secs: 0.0, graph: None, work_graph: graph.clone(),
            statuses: HashMap::new(), faces: HashMap::new(),
            published: Vec::new(), used: HashMap::new(), warnings: Vec::new(),
            choice: None, candidates: HashMap::new(),
        };
        self.runs.insert(0, record);
        self.send(Job::Prepare { run: id, graph, provenance, media: picks });
        self.inspector_dirty = true;
        Ok(id)
    }

    fn cancel_run(&mut self, id: u64) {
        let Some(record) = self.runs.iter_mut().find(|run| run.id == id) else { return };
        match record.phase {
            RunPhase::Waiting | RunPhase::Preparing => {
                record.phase = RunPhase::Cancelled;
                record.events.push(JobEvent::Finished { phase: JobPhase::Cancelled, secs: Cx::time_now() - record.started, error: None });
                self.pending_specs.retain(|(run, _, _)| *run != id);
            }
            RunPhase::Loading | RunPhase::Running => {
                let engine = if record.lan_only { &mut self.lan_engine } else { &mut self.engine };
                if let Some(engine) = engine {
                    engine.cancel(id);
                }
            }
            _ => {}
        }
        self.inspector_dirty = true;
    }

    fn start_engine(&mut self, cx: &mut Cx, results: Option<Arc<dyn ResultStore>>) {
        if self.engine.is_some() || self.stopping {
            return;
        }
        self.results = results.clone();
        match StudioEngine::start(cx, self.seams(true), results) {
            Ok(engine) => self.engine = Some(engine),
            Err(error) => self.notice = format!("the studio engine could not start: {error}"),
        }
    }

    fn submit(&mut self, cx: &mut Cx, id: u64, source: String, inputs: Vec<(String, String, makepad_flow::Value)>) {
        let lan_only = self.runs.iter().find(|run| run.id == id).is_some_and(|run| run.lan_only);
        // The LAN-only engine starts the first time a run needs it.
        if lan_only && self.lan_engine.is_none() && self.engine.is_some() && !self.stopping {
            match StudioEngine::start(cx, self.seams(false), self.results.clone()) {
                Ok(engine) => self.lan_engine = Some(engine),
                Err(error) => {
                    self.run_failed(id, format!("the studio's LAN engine could not start: {error}"));
                    return;
                }
            }
        }
        let Some(record) = self.runs.iter_mut().find(|run| run.id == id) else { return };
        if record.phase == RunPhase::Cancelled {
            return;
        }
        let engine = if lan_only { &mut self.lan_engine } else { &mut self.engine };
        let Some(engine) = engine.as_mut().filter(|engine| engine.ready()) else {
            record.phase = RunPhase::Waiting;
            self.pending_specs.push((id, source, inputs));
            return;
        };
        record.phase = RunPhase::Loading;
        engine.run(RunSpec {
            run: id,
            // One definition per job: two jobs of one preset are independent
            // (a reload of a shared definition supersedes its older load).
            definition: engine::definition_name(&record.preset, id),
            source,
            file_name: format!("{}.splash", presets::slug(&record.label)),
            inputs,
        });
    }
}

impl RunRecord {
    fn active(&self) -> bool {
        matches!(self.phase, RunPhase::Waiting | RunPhase::Preparing | RunPhase::Loading | RunPhase::Running)
    }
}

/// The parameter rows a graph offers: its generators' and language
/// models' scalar settings, in a stable order.
fn param_slots(graph: &Graph) -> Vec<ParamSlot> {
    let mut slots = Vec::new();
    for node in graph.nodes.iter().filter(|node| matches!(node.kind.as_str(), "gen" | "chat")) {
        for key in PARAMS {
            let Some((_, value)) = node.params.iter().find(|(name, _)| name == key) else { continue };
            if matches!(value, Literal::Num(_) | Literal::Str(_) | Literal::Id(_)) {
                slots.push(ParamSlot { node: node.id.clone(), key: key.to_string() });
            }
        }
    }
    slots
}

impl FlowStudio {
    pub fn handle_actions(&mut self, cx: &mut Cx, ui: &WidgetRef, actions: &Actions, runtime: &dyn LiveRoots) {
        if self.worker.is_none() {
            return;
        }
        for (id, group) in GROUP_CHIPS {
            if ui.button(cx, &[id]).clicked(actions) {
                self.filter = group;
                self.rows_dirty = true;
                ui.studio_cards(cx, ids!(studio_presets)).reset_scroll(cx);
            }
        }
        if let Some(text) = ui.text_input(cx, ids!(studio_search)).changed(actions) {
            self.search = text;
            self.rows_dirty = true;
        }
        for action in ui.studio_cards(cx, ids!(studio_presets)).actions(actions) {
            if let CardsAction::Select(key) | CardsAction::Activate(key) = action {
                if !key.starts_with("group:") {
                    self.select(cx, ui, &key, runtime);
                }
            }
        }
        if ui.button(cx, ids!(studio_run)).clicked(actions) {
            self.run_button(cx, ui);
        }
        if ui.button(cx, ids!(studio_fit)).clicked(actions) {
            ui.studio_canvas(cx, ids!(studio_canvas)).fit(cx);
        }
        for action in ui.studio_cards(cx, ids!(studio_runs)).actions(actions) {
            if let CardsAction::Select(key) | CardsAction::Activate(key) = action {
                if let Some(id) = key.strip_prefix("run:").and_then(|id| id.parse().ok()) {
                    self.take_up_run(cx, ui, id);
                }
            }
        }
        for action in ui.studio_cards(cx, ids!(studio_results)).actions(actions) {
            if let CardsAction::Select(key) | CardsAction::Activate(key) = action {
                if let Some(id) = key.split(':').nth(1).and_then(|id| id.parse().ok()) {
                    self.take_up_run(cx, ui, id);
                }
            }
        }
        for action in ui.studio_canvas(cx, ids!(studio_canvas)).actions(actions) {
            match action {
                StudioCanvasAction::Select(node) => self.describe_node(node),
                StudioCanvasAction::Run(node) => self.run_node(cx, ui, &node),
                StudioCanvasAction::None => {}
                edit => self.edit(edit),
            }
        }
    }

    /// A node picked on the canvas: what it is and how it ran, on the
    /// status line.
    fn describe_node(&mut self, node: Option<String>) {
        let Some(node) = node else {
            self.notice.clear();
            return;
        };
        let run = self.shown_run.and_then(|id| self.runs.iter().find(|run| run.id == id));
        let graph = run.and_then(|run| run.graph.clone()).or_else(|| self.chosen.as_ref().map(|preset| preset.graph.clone()));
        let Some(spec) = graph.as_ref().and_then(|graph| graph.nodes.iter().find(|candidate| candidate.id == node)).cloned() else { return };
        let mut parts = vec![format!("{} · {}", spec.id, spec.type_name)];
        if let Some(domain) = spec.domain.as_ref().filter(|domain| !domain.is_empty()) {
            parts.push(format!("hub domain {domain}"));
        }
        if let Some(run) = run {
            if let Some(status) = run.statuses.get(&node) {
                parts.push(match &status.error {
                    Some(error) => format!("failed: {error}"),
                    None if status.stage.is_empty() => status.state.clone(),
                    None => format!("{} · {}", status.state, status.stage),
                });
            }
            if let Some((seed, model, hub)) = run.used.get(&node) {
                let on = if hub.is_empty() { String::new() } else { format!(" on {hub}") };
                parts.push(format!("ran {model}{on} with seed {seed}"));
            }
        }
        self.notice = parts.join(" · ");
    }

    /// Answer a run's open choice gate with candidate `index`.
    fn answer_choice(&mut self, run: u64, index: usize) -> Result<(), String> {
        let record = self.runs.iter().find(|record| record.id == run).ok_or("no such run")?;
        let (node, _, count) = record.choice.clone().ok_or("the run is not waiting for a pick")?;
        if index >= count {
            return Err(format!("there are {count} candidates; {index} is out of range"));
        }
        let engine = if record.lan_only { &mut self.lan_engine } else { &mut self.engine };
        engine.as_mut().ok_or("the studio engine is not running")?.answer(run, &node, index)
    }

    // ---- the pump ------------------------------------------------------------

    pub fn pump(&mut self, cx: &mut Cx, ui: &WidgetRef, shown: bool, runtime: &dyn LiveRoots) -> StudioOutcome {
        // A selected live root follows what runs: a new revision's graph,
        // and its pictures while the page shows (let go of when it hides).
        if let Some(Selected::Live(root)) = self.selected {
            self.live.sync(cx, &ui.studio_canvas(cx, ids!(studio_canvas)), runtime, root, shown && !self.stopping);
        }
        if shown {
            self.boot(cx);
        }
        if self.worker.is_none() {
            return std::mem::take(&mut self.outcome);
        }
        self.flush_outbox();
        let now = Cx::time_now();
        if self.boot_retry_at.is_some_and(|at| now >= at) {
            self.boot_retry_at = None;
            self.send(Job::Boot);
        }
        if (shown || self.api_active) && !self.probing && now >= self.probe_at {
            self.probing = true;
            self.send(Job::Probe(self.mode.clone()));
        }
        let mut results = 0;
        while results < RESULTS_PER_PUMP {
            let Some(done) = self.worker.as_ref().and_then(StudioWorker::try_recv) else { break };
            results += 1;
            self.worker_done(cx, ui, done);
        }
        let mut events = Vec::new();
        for engine in [&mut self.engine, &mut self.lan_engine].into_iter().flatten() {
            engine.pump(&mut events);
        }
        for event in events {
            self.engine_event(cx, ui, event);
        }
        if let Some(id) = self.show_next.take() {
            if self.running_shown().is_none() {
                self.show_run(cx, ui, Some(id));
                self.inspector_dirty = true;
            }
        }
        for (run, node, face) in std::mem::take(&mut self.pending_faces) {
            self.set_face(cx, ui, run, &node, face);
        }
        for _ in 0..UPLOADS_PER_PUMP {
            let Some((run, node, thumb, caption)) = self.uploads.pop_front() else { break };
            let texture = texture_of(cx, &thumb);
            self.set_face(cx, ui, run, &node, FaceContent::Picture { texture, caption });
        }
        if shown {
            // The live roots' cards follow their revisions.
            let live: Vec<(usize, String)> = (0..runtime.count()).map(|root| {
                let state = runtime.state(root);
                (root, format!("{:?}{}", state.phase, state.revision))
            }).collect();
            if live != self.live_revisions {
                self.live_revisions = live;
                self.rows_dirty = true;
            }
            if self.rows_dirty {
                self.rows_dirty = false;
                let rows = self.preset_rows(cx, runtime);
                let presets = ui.studio_cards(cx, ids!(studio_presets));
                let empty = if self.presets.is_empty() { "reading the preset library…" } else { "no preset matches" };
                presets.set_rows(cx, rows, empty);
                let key = match &self.selected {
                    Some(Selected::Preset(key)) => Some(key.clone()),
                    Some(Selected::Live(root)) => Some(format!("live:{}", runtime.definition(*root))),
                    None => None,
                };
                presets.set_selected(cx, key.as_deref());
                self.paint_chips(cx, ui);
                ui.label(cx, ids!(studio_hub)).set_text(cx, &self.hub_line());
            }
            self.sync_inspector(cx, ui, false);
            self.sync_header(cx, ui, runtime);
        }
        self.outcome.propose = self.proposed.take();
        std::mem::take(&mut self.outcome)
    }

    fn hub_line(&self) -> String {
        let mut line = self.snapshot.line();
        if !self.assets_note.is_empty() {
            line.push_str(" · ");
            line.push_str(&self.assets_note);
        }
        if let Some(error) = &self.catalogue_error {
            line.push_str(" · ");
            line.push_str(error);
        }
        line
    }

    fn paint_chips(&self, cx: &mut Cx, ui: &WidgetRef) {
        let palette = makepad_look::palette::get(cx);
        for (id, group) in GROUP_CHIPS {
            let selected = self.filter == group;
            let face = Vec4f::from_u32(if selected { palette.accent } else { palette.raised });
            let hover = Vec4f::from_u32(if selected { palette.accent_hover } else { palette.control_top });
            let down = Vec4f::from_u32(if selected { palette.accent_down } else { palette.row });
            let ink = Vec4f::from_u32(if selected { palette.on_accent } else { palette.label });
            let ink_hover = Vec4f::from_u32(if selected { palette.on_accent } else { palette.accent });
            let mut button = ui.button(cx, &[id]);
            script_apply_eval!(cx, button, {
                draw_bg +: {color: #(face) color_focus: #(face) color_hover: #(hover) color_down: #(down)}
                draw_text +: {color: #(ink) color_focus: #(ink) color_hover: #(ink_hover) color_down: #(ink)}
            });
            button.redraw(cx);
        }
    }

    fn worker_done(&mut self, cx: &mut Cx, ui: &WidgetRef, done: Done) {
        match done {
            Done::Booted { results, note, retry } => {
                self.assets_note = note;
                self.rows_dirty = true;
                if retry {
                    // The engines start with the store; runs wait for them.
                    self.boot_retry_at = Some(Cx::time_now() + BOOT_RETRY_SECS);
                    return;
                }
                self.booted = true;
                self.start_engine(cx, results);
            }
            Done::Catalogue { presets, errors } => {
                self.presets = presets;
                if !errors.is_empty() {
                    log!("[flow-studio] presets that did not evaluate: {}", errors.join("; "));
                    self.catalogue_error = Some(format!("{} presets did not load", errors.len()));
                }
                self.rows_dirty = true;
            }
            Done::Saved(result) => {
                match result {
                    Ok(saved) => self.saved = saved,
                    Err(error) => log!("[flow-studio] saved presets: {error}"),
                }
                self.rows_dirty = true;
            }
            Done::Probe(snapshot) => {
                self.snapshot = snapshot;
                self.probing = false;
                self.probe_at = Cx::time_now() + PROBE_SECS;
                self.rows_dirty = true;
                self.inspector_dirty = true;
            }
            Done::Prepared { run, result } => match result {
                Ok(worker::Prepared::Run(source, inputs)) => self.submit(cx, run, source, inputs),
                Ok(worker::Prepared::Reused { node, reference, title, kind }) => self.run_reused(run, node, reference, title, kind),
                Err(error) => self.run_failed(run, error),
            },
            Done::Preview { run, node, face } => match face {
                Face::Picture(thumb, caption) => self.uploads.push_back((run, node, thumb, caption)),
                Face::Wave(peaks, caption) => self.set_face(cx, ui, run, &node, FaceContent::Wave { peaks, caption }),
                Face::Text(text) => self.set_face(cx, ui, run, &node, FaceContent::Text(text)),
            },
            Done::PresetSaved(result) => {
                match result {
                    Ok(note) => {
                        self.notice = note;
                        self.send(Job::Saved);
                    }
                    Err(error) => self.notice = format!("saving the preset failed: {error}"),
                }
            }
        }
    }

    fn set_face(&mut self, cx: &mut Cx, ui: &WidgetRef, run: u64, node: &str, face: FaceContent) {
        let Some(record) = self.runs.iter_mut().find(|record| record.id == run) else { return };
        // `<map>#<i>`: a fan-out candidate, shown in the CHOOSE picker.
        if let Some((map, index)) = node.split_once('#').and_then(|(map, index)| Some((map, index.parse::<usize>().ok()?))) {
            if let (Some(slots), FaceContent::Picture { texture, .. }) = (record.candidates.get_mut(map), face) {
                if let Some(slot) = slots.get_mut(index) {
                    *slot = Some(texture);
                }
            }
            self.inspector_dirty = true;
            return;
        }
        record.faces.insert(node.to_string(), face.clone());
        if self.shown_run == Some(run) {
            ui.studio_canvas(cx, ids!(studio_canvas)).set_face(cx, node, face);
        }
        self.inspector_dirty = true;
    }

    fn set_status(&mut self, cx: &mut Cx, ui: &WidgetRef, run: u64, node: &str, status: NodeStatus) {
        let Some(record) = self.runs.iter_mut().find(|record| record.id == run) else { return };
        if !record.origin.is_empty() {
            record.events.push(JobEvent::Node {
                node: node.to_string(), state: status.state.clone(), permille: status.has_progress.then_some(status.permille),
                stage: status.stage.clone(), error: status.error.clone(),
            });
        }
        record.statuses.insert(node.to_string(), status.clone());
        if self.shown_run == Some(run) {
            ui.studio_canvas(cx, ids!(studio_canvas)).set_status(cx, node, status);
            // The node's line beside Run follows its stage.
            self.inspector_dirty = true;
        }
    }

    /// A repeated fixed-seed run whose product is already stored:
    /// that product is its result, and nothing runs.
    fn run_reused(&mut self, run: u64, node: String, reference: ResultRef, title: String, kind: String) {
        if let Some(record) = self.runs.iter_mut().find(|record| record.id == run) {
            record.phase = RunPhase::Done;
            record.secs = Cx::time_now() - record.started;
            // The result shows by its own title; the run line says it was reused.
            let title = if title.trim().is_empty() { "already stored (same recipe, inputs and seed)".to_string() } else { title };
            if !record.origin.is_empty() {
                record.events.push(JobEvent::Published { node: node.clone(), asset: reference.clone(), title: title.clone(), kind: kind.clone(), place: "STORED · REUSED" });
                record.events.push(JobEvent::Finished { phase: JobPhase::Done, secs: record.secs, error: None });
            }
            record.published.push(PublishedAsset { node, reference, title, kind, place: "STORED · REUSED" });
        }
        self.rows_dirty = true;
        self.inspector_dirty = true;
    }

    fn run_failed(&mut self, run: u64, error: String) {
        if let Some(record) = self.runs.iter_mut().find(|record| record.id == run) {
            record.phase = RunPhase::Failed(error.clone());
            record.secs = Cx::time_now() - record.started;
            if !record.origin.is_empty() {
                record.events.push(JobEvent::Finished { phase: JobPhase::Failed, secs: record.secs, error: Some(error) });
            }
        }
        self.inspector_dirty = true;
    }

    fn engine_event(&mut self, cx: &mut Cx, ui: &WidgetRef, event: StudioEvent) {
        match event {
            StudioEvent::Ready => {
                for (run, source, inputs) in std::mem::take(&mut self.pending_specs) {
                    self.submit(cx, run, source, inputs);
                }
            }
            StudioEvent::Loaded { run, graph } => {
                let graph = Arc::new(graph);
                if let Some(record) = self.runs.iter_mut().find(|record| record.id == run) {
                    record.graph = Some(graph.clone());
                }
                if self.shown_run == Some(run) {
                    ui.studio_canvas(cx, ids!(studio_canvas)).set_graph(cx, Some(&graph), true);
                }
            }
            StudioEvent::Started { run, planned } => {
                if let Some(record) = self.runs.iter_mut().find(|record| record.id == run) {
                    record.phase = RunPhase::Running;
                }
                for node in planned {
                    self.set_status(cx, ui, run, &node, NodeStatus::new("idle", 0, false, "queued", None));
                }
                self.inspector_dirty = true;
            }
            StudioEvent::Node { run, node, event } => self.node_event(cx, ui, run, node, event),
            StudioEvent::Resync { run, nodes } => {
                for (node, state, error) in nodes {
                    let status = match state {
                        NodeState::Done => NodeStatus::new("done", 1000, true, "", None),
                        NodeState::Failed => NodeStatus::new("failed", 0, false, "", error),
                        NodeState::Running => NodeStatus::new("running", 0, false, "", None),
                        NodeState::Waiting => NodeStatus::new("waiting", 0, false, "", None),
                        NodeState::Skipped | NodeState::Cancelled => NodeStatus::new("inactive", 0, false, "", None),
                        NodeState::Pending | NodeState::Ready => NodeStatus::new("idle", 0, false, "", None),
                    };
                    self.set_status(cx, ui, run, &node, status);
                }
            }
            StudioEvent::Finished { run, state, secs, warnings, outputs: values } => {
                // Output nodes are read, not run: they finished with their
                // sources.
                let outputs: Vec<String> = self.runs.iter().find(|record| record.id == run)
                    .filter(|_| state == RunState::Done)
                    .and_then(|record| record.graph.as_ref().map(|graph| (record, graph)))
                    .map(|(record, graph)| graph.nodes.iter().filter(|node| node.kind == "output")
                        .filter(|node| graph.edges.iter().any(|edge| edge.to_node == node.id
                            && record.statuses.get(&edge.from_node).is_some_and(|status| status.state == "done")))
                        .map(|node| node.id.clone()).collect())
                    .unwrap_or_default();
                for node in outputs {
                    self.set_status(cx, ui, run, &node, NodeStatus::new("done", 1000, true, "", None));
                }
                // A cancelled or failed run leaves nothing looking busy.
                if state != RunState::Done {
                    let unfinished: Vec<String> = self.runs.iter().find(|record| record.id == run)
                        .map(|record| record.statuses.iter()
                            .filter(|(_, status)| matches!(status.state.as_str(), "running" | "idle"))
                            .map(|(node, _)| node.clone()).collect())
                        .unwrap_or_default();
                    let stage = if state == RunState::Cancelled { "cancelled" } else { "not run" };
                    for node in unfinished {
                        self.set_status(cx, ui, run, &node, NodeStatus::new("inactive", 0, false, stage, None));
                    }
                }
                if self.shown_run == Some(run) && self.notice.starts_with("picked candidate") {
                    self.notice.clear();
                }
                if let Some(record) = self.runs.iter_mut().find(|record| record.id == run) {
                    // A gate still open when the run ended is answered by nobody.
                    record.choice = None;
                    record.secs = if secs > 0.0 { secs } else { Cx::time_now() - record.started };
                    record.warnings = warnings;
                    let failure = record.statuses.iter().find_map(|(node, status)| status.error.as_ref().map(|error| format!("{node}: {error}")));
                    record.phase = match state {
                        RunState::Done => RunPhase::Done,
                        RunState::Cancelled => RunPhase::Cancelled,
                        _ => RunPhase::Failed(failure.unwrap_or_else(|| "the run failed".into())),
                    };
                    if !record.origin.is_empty() {
                        for (node, value) in values {
                            match value {
                                PortValue::One(value) => record.events.push(JobEvent::Output { node, value }),
                                PortValue::Seq(sequence) => for value in sequence.values {
                                    record.events.push(JobEvent::Output { node: node.clone(), value });
                                },
                            }
                        }
                        let (phase, error) = api::phase_of(&record.phase);
                        record.events.push(JobEvent::Finished { phase, secs: record.secs, error });
                    }
                }
                self.inspector_dirty = true;
            }
            StudioEvent::Failed { run, error } => self.run_failed(run, error),
            StudioEvent::Stopped => {}
        }
    }

    fn node_event(&mut self, cx: &mut Cx, ui: &WidgetRef, run: u64, node: String, event: NodeEvent) {
        match event {
            NodeEvent::Started => self.set_status(cx, ui, run, &node, NodeStatus::new("running", 0, false, "", None)),
            NodeEvent::Progress { permille, stage } => {
                // Progress comes on its own lane and may trail the node's end.
                let ended = self.runs.iter().find(|record| record.id == run)
                    .and_then(|record| record.statuses.get(&node))
                    .is_some_and(|status| matches!(status.state.as_str(), "done" | "failed" | "inactive"));
                if ended {
                    return;
                }
                let status = NodeStatus::new("running", permille.unwrap_or(0), permille.is_some(), &stage, None);
                self.set_status(cx, ui, run, &node, status);
            }
            NodeEvent::Failed(error) => {
                self.set_status(cx, ui, run, &node, NodeStatus::new("failed", 0, false, "", Some(error)));
                self.inspector_dirty = true;
            }
            NodeEvent::Skipped(reason) => {
                // A Map's body runs inside it, once per element.
                let stage = match reason.strip_prefix("runs inside Map `").and_then(|rest| rest.split('`').next()) {
                    Some(map) => format!("part of {map}"),
                    None => reason,
                };
                self.set_status(cx, ui, run, &node, NodeStatus::new("inactive", 0, false, &stage, None));
            }
            NodeEvent::Waiting { question, options } => {
                self.set_status(cx, ui, run, &node, NodeStatus::new("waiting", 0, false, "pick one in the inspector", None));
                if let Some(record) = self.runs.iter_mut().find(|record| record.id == run) {
                    if !record.origin.is_empty() {
                        // The candidates come from the fan-out wired into `from`.
                        let map = record.graph.as_ref().and_then(|graph| graph.edges.iter()
                            .find(|edge| edge.to_node == node && edge.to_port == "from")).map(|edge| edge.from_node.clone());
                        let candidates = map.and_then(|map| record.candidate_values.get(&map)).cloned().unwrap_or_default();
                        record.events.push(JobEvent::Choice { node: node.clone(), question: question.clone(), candidates });
                    }
                    record.choice = Some((node.clone(), question, options));
                }
                self.pending_faces.push((run, node, FaceContent::Text(format!("waiting for your pick: {options} candidates, in the inspector"))));
                self.inspector_dirty = true;
            }
            NodeEvent::Answered => {
                if let Some(record) = self.runs.iter_mut().find(|record| record.id == run) {
                    record.choice = None;
                }
                self.set_status(cx, ui, run, &node, NodeStatus::new("running", 0, false, "picked", None));
                self.inspector_dirty = true;
            }
            NodeEvent::Done(outputs) => {
                self.set_status(cx, ui, run, &node, NodeStatus::new("done", 1000, true, "", None));
                self.node_done(run, &node, outputs);
            }
        }
    }

    /// A finished node: its face, what it reported, and what it published.
    fn node_done(&mut self, run: u64, node: &str, outputs: Vec<(String, PortValue)>) {
        let Some(record) = self.runs.iter_mut().find(|record| record.id == run) else { return };
        let Some(graph) = record.graph.clone() else { return };
        let Some(spec) = graph.nodes.iter().find(|candidate| candidate.id == node) else { return };
        let one = |port: &str| outputs.iter().find(|(name, _)| name == port).and_then(|(_, value)| value.one().cloned());
        if spec.kind == "gen" {
            let text = |port: &str| one(port).map(|value| String::from_utf8_lossy(&value.bytes).trim().trim_matches('"').to_string()).unwrap_or_default();
            let url = text("provider_url");
            let (model, hub) = match url.as_str() {
                // No LAN node had the model: say so, not "preview on preview".
                preview_gen::PREVIEW_NODE => ("preview generator (no LAN node has the model)".to_string(), String::new()),
                "" | "provider" => (text("model_used"), String::new()),
                url => (text("model_used"), makepad_ai_hub::fleet::node_label(url)),
            };
            if !record.origin.is_empty() {
                record.events.push(JobEvent::Ran { node: node.to_string(), model: model.clone(), hub: hub.clone(), seed: text("seed_used") });
            }
            record.used.insert(node.to_string(), (text("seed_used"), model, hub));
        }
        if spec.kind == "publish" {
            let Some(value) = one("asset") else { return };
            let Ok(json) = makepad_strict_json::parse(&value.bytes) else { return };
            let field = |key: &str| json.get(key).and_then(|value| value.as_str()).unwrap_or_default().to_string();
            let id = field("id");
            if id.is_empty() { return }
            let revision = Some(field("revision")).filter(|revision| !revision.is_empty());
            let alias = Some(field("alias")).filter(|alias| !alias.is_empty());
            let reference = ResultRef { id, revision, alias };
            let from = presets::publish_points(&graph).into_iter().find(|point| point.node == node);
            let destination = from.as_ref().map(|point| presets::Destination::of(&point.ty, point.domain.as_deref()));
            match destination {
                Some(presets::Destination::Music | presets::Destination::Voice | presets::Destination::Sound | presets::Destination::Stem) => self.outcome.refresh_audio = true,
                Some(presets::Destination::Image | presets::Destination::Video) => self.outcome.refresh_pictures = true,
                _ => {}
            }
            if !record.origin.is_empty() {
                record.events.push(JobEvent::Published {
                    node: node.to_string(), asset: reference.clone(), title: field("title"), kind: field("kind"),
                    place: destination.map(presets::Destination::place).unwrap_or("STORED"),
                });
            }
            record.published.push(PublishedAsset {
                node: node.to_string(), reference: reference.clone(), title: field("title"), kind: field("kind"),
                place: destination.map(presets::Destination::place).unwrap_or("STORED"),
            });
            let face = FaceContent::Text(format!("published → {}\n{}", destination.map(presets::Destination::place).unwrap_or("STORED"), field("title")));
            if matches!(destination, Some(presets::Destination::Video)) {
                self.send(Job::Poster { run, node: node.to_string(), asset: reference });
            }
            self.pending_faces.push((run, node.to_string(), face));
            return;
        }
        if spec.kind == "map" {
            // The candidates, in order: each one's picture for the picker.
            let Some((_, PortValue::Seq(items))) = outputs.iter().find(|(name, value)| name == "items" && matches!(value, PortValue::Seq(_))) else { return };
            record.candidates.insert(node.to_string(), vec![None; items.values.len()]);
            let values = items.values.clone();
            record.candidate_values.insert(node.to_string(), values.clone());
            self.pending_faces.push((run, node.to_string(), FaceContent::Text(format!("{} candidates", values.len()))));
            for (index, value) in values.into_iter().enumerate() {
                self.send(Job::Preview { run, node: format!("{node}#{index}"), value });
            }
            return;
        }
        if !matches!(spec.kind.as_str(), "gen" | "chat" | "input" | "choose") {
            return;
        }
        // The declared output the face shows; the generator's own reports
        // (`seed_used`, `model_used`, ...) are not declared ports.
        for port in &spec.outputs {
            let Some(value) = one(&port.name) else { continue };
            if spec.kind == "input" && !value.ty.is_media() {
                // The run's own text (a prompt, lyrics) as it went in, so a
                // run started from the API or the chat shows what it used.
                let text = String::from_utf8_lossy(&value.bytes).trim().to_string();
                // The node's own text box already reads what the page typed.
                let typed = self.draft.text.get(node).cloned()
                    .or_else(|| self.chosen.as_ref().and_then(|preset| preset.inputs.iter().find(|input| input.node == node)).map(|input| input.default.clone()));
                if !text.is_empty() && typed.is_none_or(|typed| typed.trim() != text) {
                    self.pending_faces.push((run, node.to_string(), FaceContent::Text(text)));
                }
                break;
            }
            self.send(Job::Preview { run, node: node.to_string(), value });
            break;
        }
    }

    // ---- presenting ----------------------------------------------------------

    fn sync_header(&mut self, cx: &mut Cx, ui: &WidgetRef, runtime: &dyn LiveRoots) {
        let (title, state) = match &self.selected {
            Some(Selected::Live(root)) => {
                let root_state = runtime.state(*root);
                (format!("{} root", runtime.label(*root)), format!("live · revision {} · the app's own graph", root_state.revision))
            }
            _ => match &self.chosen {
                Some(preset) => {
                    let run = self.shown_run.and_then(|id| self.runs.iter().find(|run| run.id == id));
                    let state = match run {
                        Some(run) => format!("run #{} · {}", run.id, phase_text(&run.phase)),
                        None => "the preset's graph · Run, or set it up in the chat".into(),
                    };
                    (preset.label.clone(), state)
                }
                None => ("Choose a preset".into(), String::new()),
            },
        };
        ui.label(cx, ids!(studio_title)).set_text(cx, &title);
        ui.label(cx, ids!(studio_state)).set_text(cx, &state);
        let runnable = self.chosen.is_some() && !matches!(self.selected, Some(Selected::Live(_)));
        let run = ui.button(cx, ids!(studio_run));
        run.set_text(cx, if self.running_shown().is_some() { "Cancel" } else { "Run" });
        run.set_enabled(cx, runnable);
        let status = if !self.notice.is_empty() {
            self.notice.clone()
        } else if let Some(run) = self.shown_run.and_then(|id| self.runs.iter().find(|run| run.id == id)) {
            run_line(run)
        } else {
            self.mode.describe()
        };
        ui.label(cx, ids!(studio_status)).set_text(cx, &status);
    }

    fn sync_inspector(&mut self, cx: &mut Cx, ui: &WidgetRef, force_values: bool) {
        if !self.inspector_dirty && !force_values {
            return;
        }
        self.inspector_dirty = false;
        let live = match &self.selected {
            Some(Selected::Live(root)) => Some(*root),
            _ => None,
        };
        ui.view(cx, ids!(studio_live_box)).set_visible(cx, live.is_some());
        if live.is_some() {
            ui.label(cx, ids!(studio_live_text)).set_text(cx, &format!(
                "The {} root is part of the running app: it is never started as a job. Its graph shows here as it runs.",
                self.live_label.to_lowercase()));
        }
        let preset = self.chosen.as_ref().filter(|_| live.is_none());
        ui.label(cx, ids!(studio_inspect)).set_text(cx, match (live, preset) {
            (Some(_), _) => "Live root",
            (None, Some(_)) => "Preset",
            (None, None) => "Inspector",
        });
        ui.view(cx, ids!(studio_brief)).set_visible(cx, preset.is_some());
        ui.label(cx, ids!(studio_brief_text)).set_text(cx, preset.map(|preset| preset.brief.as_str()).unwrap_or(""));
        ui.widget(cx, ids!(studio_models_head)).set_visible(cx, preset.is_some());
        ui.label(cx, ids!(studio_models)).set_text(cx, &preset.map(|preset| self.models_text(preset)).unwrap_or_default());
        let specs = self.editor_specs();
        ui.studio_canvas(cx, ids!(studio_canvas)).set_editors(cx, specs);
        self.sync_lists(cx, ui);
    }

    fn models_text(&self, preset: &Preset) -> String {
        let mut lines = Vec::new();
        for need in &preset.needs {
            let pinned = preset.graph.nodes.iter().find(|node| node.id == need.node)
                .and_then(|node| presets::text_param(node, "model")).unwrap_or_default();
            let need = presets::ModelNeed { model: pinned, ..need.clone() };
            let wanted = if need.model.is_empty() { format!("{} (hub picks)", need.domain) } else { need.model.clone() };
            let state = match models::need_state(&need, &self.snapshot) {
                NeedState::Ready { model, nodes } => format!("installed: {model} on {}", nodes.join(", ")),
                NeedState::Missing(reason) if self.mode == GenMode::Fleet => format!("NOT ON THE LAN: {reason}; runs as a preview"),
                NeedState::Missing(reason) => format!("NOT INSTALLED: {reason}"),
                NeedState::NoHub(_) if self.mode == GenMode::Fleet => "no node answers; runs as a preview".into(),
                NeedState::NoHub(true) => "weights on this machine; no hub answers".into(),
                NeedState::NoHub(false) => "no hub answers; not installed here".into(),
                NeedState::Preview => "preview generator".into(),
            };
            let licence = models::license(&need.model).map(|(name, warning)| match warning {
                Some(warning) => format!("\n    licence: {name} ({warning})"),
                None => format!("\n    licence: {name}"),
            }).unwrap_or_default();
            lines.push(format!("{} · {wanted}\n    {state}{licence}", need.node));
        }
        if preset.llm_nodes > 0 {
            lines.push(format!("{} language-model turn{} through the hub's chat", preset.llm_nodes, if preset.llm_nodes == 1 { "" } else { "s" }));
        }
        if lines.is_empty() {
            "no generators: runs on this machine".into()
        } else {
            lines.join("\n")
        }
    }

    fn sync_lists(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        let p = makepad_look::palette::get(cx);
        let rows = self.runs.iter().map(|run| {
            let (badge, badge_tint) = match &run.phase {
                RunPhase::Done => ("DONE", p.live),
                RunPhase::Failed(_) => ("FAILED", p.danger),
                RunPhase::Cancelled => ("CANCELLED", p.dim),
                _ => ("RUNNING", p.info),
            };
            let group = self.preset(&run.preset).map(|preset| preset.group).unwrap_or(Group::Utility);
            CardRow {
                key: format!("run:{}", run.id), title: format!("#{} {}", run.id, run.label), detail: run_line(run),
                glyph: format!("{}", run.id), tint: group_tint(&p, group), badge: badge.into(), badge_tint, ..Default::default()
            }
        }).collect();
        let runs = ui.studio_cards(cx, ids!(studio_runs));
        runs.set_rows(cx, rows, "fired-up presets show here");
        runs.set_selected(cx, self.shown_run.map(|id| format!("run:{id}")).as_deref());
        let mut results = Vec::new();
        for run in &self.runs {
            for (index, asset) in run.published.iter().enumerate() {
                // The picture the published value came from, else the
                // publish node's own (a video's poster).
                let source = run.graph.as_ref().and_then(|graph| presets::publish_points(graph).into_iter().find(|point| point.node == asset.node))
                    .and_then(|point| run.faces.get(&point.from_node));
                let texture = match source.filter(|face| matches!(face, FaceContent::Picture { .. })).or_else(|| run.faces.get(&asset.node)) {
                    Some(FaceContent::Picture { texture, .. }) => Some(texture.clone()),
                    _ => None,
                };
                results.push(CardRow {
                    key: format!("result:{}:{index}", run.id), title: asset.title.clone(),
                    detail: format!("{} · {} · {}", asset.place, asset.kind, short_id(&asset.reference.id)),
                    glyph: asset.kind.chars().take(3).collect::<String>().to_uppercase(), tint: p.raised, texture, ..Default::default()
                });
            }
        }
        ui.studio_cards(cx, ids!(studio_results)).set_rows(cx, results, "published results show here");
    }
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn phase_text(phase: &RunPhase) -> String {
    match phase {
        RunPhase::Waiting => "waiting for the engine".into(),
        RunPhase::Preparing => "preparing".into(),
        RunPhase::Loading => "loading".into(),
        RunPhase::Running => "running".into(),
        RunPhase::Done => "done".into(),
        RunPhase::Failed(error) => format!("failed: {error}"),
        RunPhase::Cancelled => "cancelled".into(),
    }
}

fn run_line(run: &RunRecord) -> String {
    let mut parts = vec![phase_text(&run.phase)];
    let secs = if run.active() { Cx::time_now() - run.started } else { run.secs };
    parts.push(format!("{secs:.1} s"));
    let used: Vec<String> = run.used.iter().map(|(node, (seed, model, hub))| {
        let model = if model.is_empty() { String::new() } else { format!(" {model}") };
        let hub = if hub.is_empty() { String::new() } else { format!(" on {hub}") };
        let seed = if seed.is_empty() { String::new() } else { format!(" seed {seed}") };
        format!("{node}:{model}{hub}{seed}")
    }).collect();
    if !used.is_empty() {
        parts.push(used.join(", "));
    }
    if !run.published.is_empty() {
        parts.push(format!("{} published", run.published.len()));
    }
    parts.join(" · ")
}
