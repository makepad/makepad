//! The studio's API for the rest of the host app: how the AI (and an app's
//! own pages, such as a video maker) fires generation presets.
//!
//! Everything is in-process and non-blocking, on the UI thread, like the
//! FLOW page itself: [`FlowStudio::run_preset`] records a job and returns
//! its id at once; the studio's worker prepares it and its engines run it;
//! the caller reads what happened with [`FlowStudio::take_job_events`] or
//! [`FlowStudio::job`] on its own pump. Jobs appear in the FLOW page's
//! RUNS list as `<origin> · <preset>`, with their graphs, faces and results,
//! so nothing the AI does is hidden. Results publish into the host's result
//! store through `Publish` nodes exactly as page runs do (provenance from the
//! Publish executor, placement tags from `prepare.rs`).
//!
//! Several jobs run at once: each is its own run, spread over the LAN's AI
//! Hub nodes by the fleet scorer (only nodes that have the model installed).
//! A job without `preview` fails when no node has a model it needs; with
//! `preview` it runs that generator on the in-process preview generator.

use super::models::{self, NeedState};
use super::presets::{self, Group};
use super::{FlowStudio, Picked, RunPhase};
use makepad_flow::engine::results::ResultRef;
use makepad_flow::{Literal, PortType, Value};
use makepad_widgets::*;
use std::collections::HashMap;

/// A preset as the AI sees it.
#[derive(Clone, Debug)]
pub struct PresetInfo {
    /// The key `run_preset` takes (`studio:flux-schnell-fast`,
    /// `bundled:image-to-video`, `ai:image-candidates`, `saved:<id>`).
    pub key: String,
    pub label: String,
    pub brief: String,
    /// IMAGE, VIDEO, AUDIO · SPEECH, MUSIC, …
    pub group: &'static str,
    /// `(input node, type, label, design value)`; media inputs take library
    /// assets, the rest text (lists and JSON as JSON text).
    pub inputs: Vec<(String, PortType, String, String)>,
    /// Where each published result lands: `VJ · IMAGE`, `DJ LIBRARY`, …
    pub outputs: Vec<&'static str>,
    /// `(generator node, hub domain, pinned model or "" for the hub's
    /// choice)`, and the LAN nodes that have it installed now.
    pub needs: Vec<(String, String, String, Vec<String>)>,
    /// The nodes that can run the whole preset now (empty: none can, or
    /// nothing is generated).
    pub nodes: Vec<String>,
    /// Every generator's model is installed on some node right now.
    pub runnable: bool,
    /// Its scalar parameters, `(node, key, value)`, as `RunRequest::param`
    /// may set them.
    pub params: Vec<(String, String, String)>,
    /// A `Choose` gate: the job stops for a pick unless `RunRequest::pick`.
    pub chooses: bool,
    /// Why the preset cannot run as written (a pinned model of another
    /// kind, an input nothing feeds); `run_preset` refuses it with these.
    pub problems: Vec<String>,
}

/// What a job runs with. Every field is optional.
#[derive(Clone, Debug, Default)]
pub struct RunRequest {
    /// Who fires it, shown in the FLOW history (`AI video`).
    pub origin: String,
    /// Text (and list/JSON as JSON text) inputs by input node id.
    pub text: Vec<(String, String)>,
    /// Media inputs by input node id: stored results.
    pub media: Vec<(String, ResultRef)>,
    /// Parameter overrides: `(node, key, value)`; a `None` node sets `key`
    /// on every generator that has it (e.g. `width` on image and video).
    pub params: Vec<(Option<String>, String, Literal)>,
    /// Beats per minute recorded on published music (tag `bpm-<n>`).
    pub bpm: Option<f64>,
    /// Answer every `Choose` gate with this candidate up front.
    pub pick: Option<usize>,
    /// Run a generator no LAN node offers on the preview generator (test
    /// pictures and tones) instead of failing.
    pub preview: bool,
    /// More tags for the published assets (store labels: `[a-z0-9_-]`).
    pub tags: Vec<String>,
    /// Run only this node, what it needs upstream and the Publish nodes it
    /// alone feeds (a node's own Run on the FLOW canvas).
    pub only: Option<String>,
}

impl RunRequest {
    pub fn new(origin: &str) -> Self {
        Self { origin: origin.to_string(), ..Default::default() }
    }

    pub fn text(mut self, node: &str, value: &str) -> Self {
        self.text.push((node.to_string(), value.to_string()));
        self
    }

    pub fn media(mut self, node: &str, asset: ResultRef) -> Self {
        self.media.push((node.to_string(), asset));
        self
    }

    pub fn param(mut self, node: Option<&str>, key: &str, value: Literal) -> Self {
        self.params.push((node.map(str::to_string), key.to_string(), value));
        self
    }

    /// Picture or video size on every generator that takes one. Sizes step
    /// by 16 on the image models: 1024×576 and 576×1024 for 16:9 and 9:16
    /// sketches, 1920×1088 / 1088×1920 for full HD.
    pub fn size(self, width: u32, height: u32) -> Self {
        self.param(None, "width", Literal::Num(width as f64)).param(None, "height", Literal::Num(height as f64))
    }

    /// Length in seconds on every generator that takes one (music, SFX).
    pub fn seconds(self, seconds: f64) -> Self {
        self.param(None, "seconds", Literal::Num(seconds))
    }

    pub fn seed(self, seed: u64) -> Self {
        self.param(None, "seed", Literal::Num(seed as f64))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobPhase {
    /// Queued: being prepared or waiting for the engine.
    Preparing,
    Running,
    /// Stopped at a `Choose` gate for [`FlowStudio::choose_job`].
    Choosing,
    Done,
    Failed,
    Cancelled,
}

pub(super) fn phase_of(phase: &RunPhase) -> (JobPhase, Option<String>) {
    match phase {
        RunPhase::Waiting | RunPhase::Preparing => (JobPhase::Preparing, None),
        RunPhase::Loading | RunPhase::Running => (JobPhase::Running, None),
        RunPhase::Done => (JobPhase::Done, None),
        RunPhase::Failed(error) => (JobPhase::Failed, Some(error.clone())),
        RunPhase::Cancelled => (JobPhase::Cancelled, None),
    }
}

/// What happened in a job, in order.
#[derive(Clone, Debug)]
pub enum JobEvent {
    /// A node's state: `running` (with `permille` when known), `done`,
    /// `failed`, `waiting`, `inactive` (skipped or cancelled), `idle`.
    Node { node: String, state: String, permille: Option<u16>, stage: String, error: Option<String> },
    /// A generator finished: the model that ran, the LAN node (`hub`, empty
    /// for the preview generator) and the seed it used.
    Ran { node: String, model: String, hub: String, seed: String },
    /// A `Choose` gate waits: the candidates' values in order (images,
    /// audio, …). Answer with [`FlowStudio::choose_job`].
    Choice { node: String, question: String, candidates: Vec<Value> },
    /// A result is in the host's result store.
    Published { node: String, asset: ResultRef, title: String, kind: String, place: &'static str },
    /// An `Output` node's value (text answers, unpublished media).
    Output { node: String, value: Value },
    /// The last event of a job.
    Finished { phase: JobPhase, secs: f64, error: Option<String> },
}

/// A job's state now.
#[derive(Clone, Debug)]
pub struct JobSnapshot {
    pub phase: JobPhase,
    pub error: Option<String>,
    pub secs: f64,
    /// Published results so far: `(asset, title, place)`.
    pub assets: Vec<(ResultRef, String, &'static str)>,
    /// `(generator node, model, LAN node)` for the generators that ran.
    pub ran_on: Vec<(String, String, String)>,
}

/// A job's expected time, by the fleet's own ETA model.
#[derive(Clone, Debug)]
pub struct EstimateInfo {
    /// Per generator: `(node, model, LAN node, eta in ms)`; generators run
    /// one after another along the graph, so the total is their sum.
    pub steps: Vec<(String, String, String, u64)>,
    pub total_ms: u64,
    /// Generators no node can run now.
    pub unavailable: Vec<String>,
}

impl FlowStudio {
    /// Start the studio for an API caller (the worker, the catalogue, the
    /// hub probes), whether or not FLOW has been opened.
    pub fn api_start(&mut self, cx: &mut Cx) {
        self.api_active = true;
        self.boot(cx);
    }

    /// The catalogue is read and the hub probed at least once.
    pub fn api_ready(&self) -> bool {
        !self.presets.is_empty() && self.snapshot.mode.is_some()
    }

    /// Every preset, with what it needs and where it can run now.
    pub fn presets_info(&self) -> Vec<PresetInfo> {
        self.all_presets().map(|preset| {
            let problems = models::graph_problems(&preset.graph, &self.snapshot);
            let needs: Vec<(String, String, String, Vec<String>)> = preset.needs.iter().map(|need| {
                let nodes = match models::need_state(need, &self.snapshot) {
                    NeedState::Ready { nodes, .. } => nodes,
                    _ => Vec::new(),
                };
                (need.node.clone(), need.domain.clone(), need.model.clone(), nodes)
            }).collect();
            PresetInfo {
                key: preset.key.clone(),
                label: preset.label.clone(),
                brief: preset.brief.clone(),
                group: preset.group.label(),
                inputs: preset.inputs.iter().map(|input| (input.node.clone(), input.ty.clone(), input.label.clone(), input.default.clone())).collect(),
                outputs: preset.destinations.iter().map(|destination| destination.place()).collect(),
                runnable: needs.iter().all(|need| !need.3.is_empty()) && problems.is_empty(),
                problems,
                nodes: models::nodes_for(&preset.needs, &self.snapshot),
                needs,
                params: super::param_slots(&preset.graph).into_iter().filter_map(|slot| {
                    let node = preset.graph.nodes.iter().find(|node| node.id == slot.node)?;
                    Some((slot.node.clone(), slot.key.clone(), presets::text_param(node, &slot.key).unwrap_or_default()))
                }).collect(),
                chooses: preset.graph.nodes.iter().any(|node| node.kind == "choose"),
            }
        }).collect()
    }

    /// Fire preset `key` with `request`. Returns the job id at once; the job
    /// shows in FLOW's RUNS. Fails only when the preset is unknown, an input
    /// or parameter does not exist or does not parse, or a media input is
    /// missing.
    pub fn run_preset(&mut self, cx: &mut Cx, key: &str, request: RunRequest) -> Result<u64, String> {
        self.api_start(cx);
        let preset = self.preset(key).cloned().ok_or_else(|| {
            if self.presets.is_empty() { "the preset library is still loading".to_string() } else { format!("no preset `{key}`") }
        })?;
        let (mut graph, mut text, mut media) = apply_request(&preset, &request)?;
        let mut preset = preset;
        if let Some(node) = &request.only {
            if !graph.nodes.iter().any(|candidate| &candidate.id == node) {
                return Err(format!("preset `{key}` has no node `{node}`"));
            }
            graph = super::editors::upstream_of(&graph, node);
            preset.inputs.retain(|input| graph.nodes.iter().any(|candidate| candidate.id == input.node));
            text.retain(|node, _| graph.nodes.iter().any(|candidate| &candidate.id == node));
            media.retain(|node, _| graph.nodes.iter().any(|candidate| &candidate.id == node));
        }
        let origin = if request.origin.trim().is_empty() { "API".to_string() } else { request.origin.clone() };
        let tags = request.tags.iter().map(|tag| {
            let mut tag = presets::slug(tag);
            tag.truncate(presets::MAX_TAG_BYTES);
            tag.trim_end_matches('-').to_string()
        }).filter(|tag| !tag.is_empty()).collect();
        let lan_only = !request.preview && self.mode == models::GenMode::Fleet;
        let shown_key = preset.key.clone();
        let id = self.start_run(preset, &graph, text, media, origin, lan_only, tags)?;
        self.rows_dirty = true;
        // A run of the preset the page shows (from the chat's card, too)
        // comes onto the canvas, where its nodes can be cancelled.
        if self.chosen.as_ref().is_some_and(|chosen| chosen.key == shown_key) && self.running_shown().is_none() {
            self.show_next = Some(id);
        }
        Ok(id)
    }

    /// Everything that happened in job `id` since the last call.
    pub fn take_job_events(&mut self, id: u64) -> Vec<JobEvent> {
        self.runs.iter_mut().find(|run| run.id == id).map(|run| std::mem::take(&mut run.events)).unwrap_or_default()
    }

    pub fn job(&self, id: u64) -> Option<JobSnapshot> {
        let run = self.runs.iter().find(|run| run.id == id)?;
        let (mut phase, error) = phase_of(&run.phase);
        if run.choice.is_some() {
            phase = JobPhase::Choosing;
        }
        Some(JobSnapshot {
            phase,
            error,
            secs: if run.active() { Cx::time_now() - run.started } else { run.secs },
            assets: run.published.iter().map(|asset| (asset.reference.clone(), asset.title.clone(), asset.place)).collect(),
            ran_on: run.used.iter().map(|(node, (_, model, hub))| (node.clone(), model.clone(), hub.clone())).collect(),
        })
    }

    /// Answer job `id`'s open `Choose` gate with candidate `index` (from
    /// its `JobEvent::Choice`).
    pub fn choose_job(&mut self, id: u64, index: usize) -> Result<(), String> {
        self.answer_choice(id, index)
    }

    pub fn cancel_job(&mut self, id: u64) {
        self.cancel_run(id);
    }

    /// How long preset `key` would take with `request` on the LAN now, by
    /// the fleet's ETA model (queue, model load, generation). `None` before
    /// the first hub probe or for an unknown preset.
    pub fn estimate(&self, key: &str, request: &RunRequest) -> Option<EstimateInfo> {
        let preset = self.preset(key)?;
        self.snapshot.mode.as_ref()?;
        let mut info = EstimateInfo { steps: Vec::new(), total_ms: 0, unavailable: Vec::new() };
        for need in &preset.needs {
            let node = preset.graph.nodes.iter().find(|node| node.id == need.node)?;
            let number = |key: &str| -> Option<f64> {
                request.params.iter().rev()
                    .find(|(target, name, _)| name == key && target.as_ref().is_none_or(|target| target == &need.node))
                    .and_then(|(_, _, value)| match value { Literal::Num(value) => Some(*value), _ => None })
                    .or_else(|| presets::text_param(node, key).and_then(|text| text.parse().ok()))
            };
            let generate = makepad_ai_hub::protocol::GenerateRequestJson {
                model: need.model.clone(),
                width: number("width").map(|value| value as u32),
                height: number("height").map(|value| value as u32),
                steps: number("steps").map(|value| value as u32),
                seconds: number("seconds"),
                ..Default::default()
            };
            let picked = if need.model.is_empty() {
                makepad_ai_hub::fleet::pick_for_domain_eta_request(&self.snapshot.boxes, &need.domain, &generate)
                    .map(|(index, model, eta)| (index, model, eta))
            } else {
                makepad_ai_hub::fleet::pick_for_model_eta(&self.snapshot.boxes, &need.model, &generate)
                    .map(|(index, eta)| (index, need.model.clone(), eta))
            };
            match picked {
                Some((index, model, eta)) => {
                    let hub = makepad_ai_hub::fleet::node_label(&self.snapshot.boxes[index].base_url);
                    info.total_ms += eta;
                    info.steps.push((need.node.clone(), model, hub, eta));
                }
                None => info.unavailable.push(need.node.clone()),
            }
        }
        Some(info)
    }
}

/// A stored result as a tool call names it: `id`, `id@revision`, or an
/// alias (`namespace/name`).
pub fn parse_result_ref(reference: &str) -> Result<ResultRef, String> {
    let reference = reference.trim();
    if reference.is_empty() {
        return Err("an input names no stored result".into());
    }
    if reference.contains('/') {
        return Ok(ResultRef::alias(reference));
    }
    Ok(match reference.split_once('@') {
        Some((id, revision)) => ResultRef { id: id.to_string(), revision: Some(revision.to_string()), alias: None },
        None => ResultRef::id(reference),
    })
}

/// The key prefix of the shared generation presets in the studio's
/// catalogue; the gen.* tools name them without it.
pub const GEN_KEY: &str = "gen:";

/// The studio key a gen.* preset id names: a shared preset's bare id, or
/// one of the studio's own keys (`studio:…`, `bundled:…`, `saved:…`, `ai:…`).
pub fn gen_key(preset: &str) -> String {
    if preset.contains(':') { preset.to_string() } else { format!("{GEN_KEY}{preset}") }
}

fn gen_id(key: &str) -> &str {
    key.strip_prefix(GEN_KEY).unwrap_or(key)
}

type Json = makepad_strict_json::Value;

/// A `gen.run` request's prompt, text inputs, parameters and seed as a
/// studio request (the input assets are `FlowStudio::gen_request`'s).
fn gen_text_request(preset: &super::presets::Preset, run: &makepad_gen::tools::RunRequest) -> Result<RunRequest, String> {
    let mut request = RunRequest::new("AI (control API)");
    if let Some(prompt) = run.prompt.as_deref().filter(|prompt| !prompt.trim().is_empty()) {
        let node = preset.inputs.iter().find(|input| input.node == "prompt" && input.ty == PortType::Text)
            .or_else(|| preset.inputs.iter().find(|input| input.ty == PortType::Text))
            .ok_or_else(|| format!("`{}` takes no prompt", run.preset))?;
        request = request.text(&node.node.clone(), prompt);
    }
    for (name, value) in &run.params {
        if preset.inputs.iter().any(|input| &input.node == name && !input.ty.is_media()) {
            let text = match value {
                Json::Str(text) => text.clone(),
                other => other.to_json(),
            };
            request = request.text(name, &text);
        } else {
            request = request.param(None, name, makepad_gen::tools::json_literal(value));
        }
    }
    if let Some(seed) = run.seed {
        request = request.seed(seed);
    }
    Ok(request)
}

/// The preset's prompt input: `prompt`, else the first text input.
fn prompt_input(preset: &super::presets::Preset) -> Option<&super::presets::PresetInput> {
    preset.inputs.iter().find(|input| input.node == "prompt" && input.ty == PortType::Text)
        .or_else(|| preset.inputs.iter().find(|input| input.ty == PortType::Text))
}

/// The gen_run settings card of `preset`: its id, its prompt, and in
/// `params` every other text input and generator setting with the design
/// value (what the card's Parameters line edits, as JSON). Input assets
/// are the person's to give (`inputs`, which the card shows empty), and
/// the seed stays unset: random.
pub fn gen_prefill(preset: &super::presets::Preset) -> Json {
    let mut fields = vec![("preset".to_string(), Json::Str(gen_id(&preset.key).to_string()))];
    let prompt = prompt_input(preset);
    if let Some(prompt) = prompt.filter(|input| !input.default.trim().is_empty()) {
        fields.push(("prompt".to_string(), Json::Str(prompt.default.clone())));
    }
    let mut params: Vec<(String, Json)> = Vec::new();
    for input in preset.inputs.iter().filter(|input| !input.ty.is_media() && Some(&input.node) != prompt.map(|prompt| &prompt.node)) {
        params.push((input.node.clone(), Json::Str(input.default.clone())));
    }
    for slot in super::param_slots(&preset.graph) {
        if slot.key == "seed" || params.iter().any(|(name, _)| *name == slot.key) {
            continue;
        }
        let value = preset.graph.nodes.iter().find(|node| node.id == slot.node)
            .and_then(|node| node.params.iter().find(|(name, _)| *name == slot.key)).map(|(_, value)| value);
        let value = match value {
            Some(Literal::Num(value)) if value.fract() == 0.0 && value.abs() < 1e15 => Json::Int(*value as i64),
            Some(Literal::Num(value)) => Json::F64(*value),
            Some(Literal::Str(text) | Literal::Id(text)) if !text.is_empty() => Json::Str(text.clone()),
            _ => continue,
        };
        params.push((slot.key, value));
    }
    if !params.is_empty() {
        fields.push(("params".to_string(), Json::Obj(params)));
    }
    Json::Obj(fields)
}

/// The input assets a preset takes, in the order `inputs` lists them:
/// `node (type)`.
fn media_inputs(preset: &super::presets::Preset) -> Vec<String> {
    preset.inputs.iter().filter(|input| input.ty.is_media()).map(|input| format!("{} ({})", input.label, input.ty.as_str())).collect()
}

/// The chosen preset, as the chat's FLOW target describes it.
#[derive(Clone, Debug)]
pub struct ChosenPreset {
    pub label: String,
    pub brief: String,
    /// The gen_run card's prefill.
    pub prefill: Json,
    /// The name Save as preset starts with.
    pub save_name: String,
    /// The input assets it takes, `label (type)` each.
    pub takes: Vec<String>,
}

/// What the chat's FLOW target shows of the page.
#[derive(Clone, Debug)]
pub struct FlowChat {
    pub preset: Option<ChosenPreset>,
    /// The job running now (a generation the chat started).
    pub running: Option<u64>,
    /// The AI Hub's line.
    pub hub: String,
}

impl FlowStudio {
    /// Every preset as `gen.presets` lists it: the shared catalogue under
    /// its own ids, then the studio's own presets under their keys, with what the
    /// LAN can run now.
    pub fn gen_presets(&self, kind: Option<&str>) -> Vec<makepad_strict_json::Value> {
        self.all_presets().filter_map(|preset| {
            let shared = makepad_gen::catalog::preset(gen_id(&preset.key)).filter(|_| preset.key.starts_with(GEN_KEY));
            let preset_kind = match (shared, preset.group) {
                (Some(shared), _) => shared.kind,
                (None, Group::Image) => "image",
                (None, Group::Video) => "video",
                (None, Group::Audio | Group::Music) => "audio",
                (None, Group::Mesh) => "mesh",
                (None, _) => "other",
            };
            if kind.is_some_and(|kind| kind != preset_kind) {
                return None;
            }
            let info = makepad_gen::catalog::describe(gen_id(&preset.key), preset_kind, &preset.graph);
            let missing: Vec<String> = match models::readiness(&preset.needs, &self.snapshot) {
                models::Readiness::Missing(missing) => missing,
                models::Readiness::NoHub => preset.needs.iter().map(|need| need.domain.clone()).collect(),
                _ => Vec::new(),
            };
            let eta = self.estimate(&preset.key, &RunRequest::default()).filter(|estimate| estimate.unavailable.is_empty())
                .map(|estimate| estimate.total_ms as f64 / 1000.0);
            Some(makepad_gen::tools::preset_json(&info, Some(&makepad_gen::tools::Readiness { missing, eta_secs: eta })))
        }).collect()
    }

    /// A `gen.run` request as a studio run request: the prompt, the input
    /// assets (`asset_id@revision`) in the preset's input order, text
    /// inputs and parameters by name, the seed.
    pub fn gen_request(&self, run: &makepad_gen::tools::RunRequest) -> Result<(String, RunRequest), String> {
        let key = gen_key(&run.preset);
        let preset = self.preset(&key).ok_or_else(|| format!("no preset `{}`: gen_presets lists them", run.preset))?;
        let mut request = gen_text_request(preset, run)?;
        let media: Vec<&super::presets::PresetInput> = preset.inputs.iter().filter(|input| input.ty.is_media()).collect();
        if run.inputs.len() != media.len() {
            return Err(format!("`{}` takes {} input asset(s): {}", run.preset, media.len(),
                media.iter().map(|input| format!("{} ({})", input.node, input.ty.as_str())).collect::<Vec<_>>().join(", ")));
        }
        for (input, reference) in media.iter().zip(&run.inputs) {
            request = request.media(&input.node.clone(), parse_result_ref(reference)?);
        }
        Ok((key, request))
    }

    /// What the chat's FLOW target knows of the page: the chosen preset
    /// with the cards it opens, the job running, the hub.
    pub fn chat_state(&self) -> FlowChat {
        let chosen = self.chosen.as_ref().filter(|_| !matches!(self.selected, Some(super::Selected::Live(_))));
        FlowChat {
            preset: chosen.map(|preset| ChosenPreset {
                label: preset.label.clone(),
                brief: preset.brief.clone(),
                prefill: gen_prefill(preset),
                save_name: if preset.group == Group::Saved { preset.label.clone() } else { format!("{} (custom)", preset.label) },
                takes: media_inputs(preset),
            }),
            running: self.runs.iter().find(|run| run.active()).map(|run| run.id),
            hub: self.hub_line(),
        }
    }

    /// The gen_run card's prefill for preset `key`.
    pub fn gen_prefill(&self, key: &str) -> Option<Json> {
        self.preset(key).map(gen_prefill)
    }

    /// Save a preset to the library with the prompt, parameters and seed
    /// of `run` set (what the old Save as preset button saved): the worker
    /// publishes it and the SAVED group reloads. Returns the name it is
    /// saved under.
    pub fn save_as_preset(&mut self, run: &makepad_gen::tools::RunRequest, name: Option<&str>) -> Result<String, String> {
        let preset = self.preset(&gen_key(&run.preset)).cloned().ok_or_else(|| format!("no preset `{}`: gen_presets lists them", run.preset))?;
        let request = gen_text_request(&preset, run)?;
        let (graph, text, _) = apply_request(&preset, &request)?;
        let name = match name.map(str::trim).filter(|name| !name.is_empty()) {
            Some(name) => name.to_string(),
            None if preset.group == Group::Saved => preset.label.clone(),
            None => format!("{} (custom)", preset.label),
        };
        let values = text.into_iter().collect();
        self.send(super::worker::Job::SavePreset { name: name.clone(), group: preset.group, graph: std::sync::Arc::new(graph), values });
        self.notice = format!("saving “{name}” to the library…");
        Ok(name)
    }

    /// Job `id` as the gen.* tools report it.
    pub fn gen_view(&self, id: u64) -> Option<makepad_gen::tools::JobView> {
        use makepad_gen::tools::{AssetResult, Choice, JobView, Phase};
        let run = self.runs.iter().find(|run| run.id == id)?;
        let snapshot = self.job(id)?;
        let phase = match snapshot.phase {
            JobPhase::Preparing | JobPhase::Running => Phase::Running,
            JobPhase::Choosing => Phase::Choosing,
            JobPhase::Done => Phase::Done,
            JobPhase::Failed => Phase::Failed,
            JobPhase::Cancelled => Phase::Cancelled,
        };
        let graph = run.graph.as_ref().map(|graph| graph.as_ref()).unwrap_or(run.work_graph.as_ref());
        let planned = graph.nodes.len().max(1) as f32;
        let finished = run.statuses.values().filter(|status| matches!(status.state.as_str(), "done" | "inactive")).count() as f32;
        let running = run.statuses.iter().find(|(_, status)| status.state == "running");
        // The running generator's own share: the hub's progress for its job.
        let partial = running.filter(|(_, status)| status.has_progress).map_or(0.0, |(_, status)| status.permille as f32 / 1000.0);
        Some(JobView {
            id: id.to_string(),
            preset: gen_id(&run.preset).to_string(),
            phase,
            progress: ((finished + partial) / planned).min(1.0),
            stage: running.map(|(node, status)| if status.stage.is_empty() { node.clone() } else { format!("{node}: {}", status.stage) }).unwrap_or_default(),
            seconds: snapshot.secs,
            seed: None,
            assets: run.published.iter().map(|asset| AssetResult {
                node: asset.node.clone(), asset_id: asset.reference.id.clone(), revision: asset.reference.revision.clone().unwrap_or_default(),
                alias: asset.reference.alias.clone(), kind: asset.kind.clone(), title: asset.title.clone(),
            }).collect(),
            character: None,
            text: Vec::new(),
            reused: false,
            error: snapshot.error,
            choice: run.choice.as_ref().map(|(node, question, candidates)| Choice { node: node.clone(), question: question.clone(), candidates: *candidates }),
        })
    }
}

/// A preset's graph and inputs with `request` applied: parameters, BPM,
/// up-front picks, text and media inputs. Pure; checks every name.
pub(super) fn apply_request(preset: &super::presets::Preset, request: &RunRequest)
    -> Result<(makepad_flow::Graph, HashMap<String, String>, HashMap<String, Picked>), String> {
    let mut graph = (*preset.graph).clone();
    for (node, key, value) in &request.params {
        let targets: Vec<String> = match node {
            Some(node) => {
                if !graph.nodes.iter().any(|candidate| &candidate.id == node) {
                    return Err(format!("preset `{}` has no node `{node}`", preset.key));
                }
                vec![node.clone()]
            }
            None => graph.nodes.iter()
                .filter(|candidate| matches!(candidate.kind.as_str(), "gen" | "chat") && candidate.params.iter().any(|(name, _)| name == key))
                .map(|candidate| candidate.id.clone()).collect(),
        };
        if targets.is_empty() {
            return Err(format!("no generator in `{}` takes `{key}`", preset.key));
        }
        for target in targets {
            graph = makepad_flow_ui::graph_edit::set_param(&graph, &target, key, value.clone());
        }
    }
    if let Some(bpm) = request.bpm {
        for node in graph.nodes.clone().iter().filter(|node| node.kind == "publish") {
            graph = makepad_flow_ui::graph_edit::set_param(&graph, &node.id, "bpm", Literal::Num(bpm.round()));
        }
    }
    if let Some(pick) = request.pick {
        for node in graph.nodes.clone().iter().filter(|node| node.kind == "choose") {
            graph = makepad_flow_ui::graph_edit::set_param(&graph, &node.id, "pick", Literal::Num(pick as f64));
        }
    }
    let mut text: HashMap<String, String> = preset.inputs.iter().filter(|input| !input.ty.is_media())
        .map(|input| (input.node.clone(), input.default.clone())).collect();
    for (node, value) in &request.text {
        if !preset.inputs.iter().any(|input| &input.node == node && !input.ty.is_media()) {
            return Err(format!("preset `{}` has no text input `{node}`", preset.key));
        }
        text.insert(node.clone(), value.clone());
    }
    let mut media = HashMap::new();
    for (node, asset) in &request.media {
        if !preset.inputs.iter().any(|input| &input.node == node && input.ty.is_media()) {
            return Err(format!("preset `{}` has no media input `{node}`", preset.key));
        }
        media.insert(node.clone(), Picked { reference: asset.clone() });
    }
    Ok((graph, text, media))
}

/// The fan-out the AI uses to "make N, pick the best": one picture per
/// seed across the LAN, a `Choose` gate, the chosen one published.
pub const IMAGE_CANDIDATES: &str = r#"use mod.flow.*

/** What to paint, several ways. */
let prompt = Input{ type: @text default: "a neon koi circling a flooded arcade, night" }
/** One seed per candidate; the fleet spreads the jobs over its nodes. */
let seeds = Input{ type: @list default: [11, 23, 37, 41] }
/** The picture job, run once per seed by `candidates`. */
let image = Image{ prompt: prompt.text() width: 1024 height: 576 steps: 4 model: "flux1-schnell" }
let candidates = Map{ type: @image many: 16 body: image.image() over: seeds.list() bind: @seed concurrency: 4 }
let pick = Choose{ type: @image many: 16 from: candidates.items() question: "Which picture is best?" }
/** The chosen picture, into the library's IMAGE lane. */
let published = Publish{ value: pick.image() namespace: "flows" tags: ["flow", "image"] }

Flow{
    label: "Image candidates, choose"
    brief: "Paints one picture per seed across the LAN, waits for a pick (or takes a given one) and publishes it."
    prompt, seeds, image, candidates, pick, published
}
"#;

/// The studio's in-code presets, for the AI (and listed on the page).
pub(super) fn sources() -> Vec<(String, Option<Group>, &'static str, std::sync::Arc<str>, String)> {
    vec![("ai:image-candidates".to_string(), Some(Group::Image), "", std::sync::Arc::from(IMAGE_CANDIDATES), "image-candidates.splash".to_string())]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::{sources, Preset};

    fn preset(key: &str) -> Preset {
        let (key, group, template_group, source, file) = sources().into_iter().find(|entry| entry.0 == key).unwrap();
        Preset::describe(key, group, template_group, source, file).unwrap()
    }

    fn param(graph: &makepad_flow::Graph, node: &str, key: &str) -> Option<Literal> {
        graph.nodes.iter().find(|candidate| candidate.id == node)?.params.iter().find(|(name, _)| name == key).map(|(_, value)| value.clone())
    }

    #[test]
    fn a_request_sets_sizes_on_every_generator_and_checks_names() {
        let dream = preset("bundled:dream");
        let (graph, text, _) = apply_request(&dream, &RunRequest::new("AI").size(1088, 1920).text("prompt", "a comet")).unwrap();
        for node in ["image", "video"] {
            assert_eq!(param(&graph, node, "width"), Some(Literal::Num(1088.0)), "{node}");
            assert_eq!(param(&graph, node, "height"), Some(Literal::Num(1920.0)), "{node}");
        }
        assert_eq!(text.get("prompt").map(String::as_str), Some("a comet"));
        assert!(apply_request(&dream, &RunRequest::new("AI").text("nope", "x")).is_err());
        assert!(apply_request(&dream, &RunRequest::new("AI").param(None, "bogus", Literal::Num(1.0))).is_err());
        assert!(apply_request(&preset("bundled:image-upscale"), &RunRequest::new("AI").text("image", "x")).is_err());
    }

    /// The prefill parsed as `gen_run`'s arguments, then as the studio's request.
    fn request_of(preset: &Preset, prefill: &makepad_strict_json::Value) -> (makepad_gen::tools::RunRequest, RunRequest) {
        let Ok(makepad_gen::tools::GenCall::Run { request, .. }) = makepad_gen::tools::parse("gen_run", prefill) else { panic!("{} is not gen_run's arguments", prefill.to_json()) };
        let run = gen_text_request(preset, &request).unwrap();
        (request, run)
    }

    #[test]
    fn a_preset_becomes_the_prefill_of_its_gen_run_card() {
        use makepad_strict_json::Value as Json;
        let dream = preset("bundled:dream");
        let fill = gen_prefill(&dream);
        let prompt = dream.inputs.iter().find(|input| input.node == "prompt").unwrap().default.clone();
        assert_eq!(fill.get("preset"), Some(&Json::Str("bundled:dream".into())));
        assert_eq!(fill.get("prompt"), Some(&Json::Str(prompt)), "{}", fill.to_json());
        // The generators' own sizes and settings are the card's Parameters, the seed stays random.
        let params = fill.get("params").expect("params");
        let width = param(&dream.graph, "image", "width").and_then(|value| if let Literal::Num(value) = value { Some(value as i64) } else { None }).unwrap();
        assert_eq!(params.get("width"), Some(&Json::Int(width)), "{}", params.to_json());
        assert!(params.get("seed").is_none() && fill.get("seed").is_none() && fill.get("inputs").is_none());
        // The prefill is a valid gen_run request, and the preset accepts what it says as it is.
        let (_, run) = request_of(&dream, &fill);
        assert!(apply_request(&dream, &run).is_ok());

        // A preset's other text inputs ride in params as text.
        let candidates = preset("ai:image-candidates");
        let fill = gen_prefill(&candidates);
        assert_eq!(fill.get("params").and_then(|params| params.get("seeds")), Some(&Json::Str("[11, 23, 37, 41]".into())), "{}", fill.to_json());
        let (_, run) = request_of(&candidates, &fill);
        let (_, text, _) = apply_request(&candidates, &run).unwrap();
        assert_eq!(text.get("seeds").map(String::as_str), Some("[11, 23, 37, 41]"));

        // Input assets are the person's: none in the prefill, and the chat is told what the preset takes.
        let upscale = preset("bundled:image-upscale");
        assert!(gen_prefill(&upscale).get("inputs").is_none());
        assert_eq!(media_inputs(&upscale).len(), upscale.inputs.iter().filter(|input| input.ty.is_media()).count());
        assert!(!media_inputs(&upscale).is_empty());
    }

    #[test]
    fn saving_a_preset_applies_the_prompt_and_parameters_of_the_card() {
        let dream = preset("bundled:dream");
        let run = makepad_gen::tools::RunRequest {
            preset: "bundled:dream".into(), prompt: Some("a comet".into()), inputs: Vec::new(),
            params: vec![("width".into(), makepad_strict_json::Value::Int(576))], seed: Some(5),
        };
        let request = gen_text_request(&dream, &run).unwrap();
        let (graph, text, _) = apply_request(&dream, &request).unwrap();
        assert_eq!(text.get("prompt").map(String::as_str), Some("a comet"));
        assert_eq!(param(&graph, "image", "width"), Some(Literal::Num(576.0)));
        assert_eq!(param(&graph, "image", "seed"), Some(Literal::Num(5.0)));
        // A parameter nothing takes is refused, not dropped.
        let bad = makepad_gen::tools::RunRequest { params: vec![("bogus".into(), makepad_strict_json::Value::Int(1))], ..run };
        assert!(apply_request(&dream, &gen_text_request(&dream, &bad).unwrap()).is_err());
    }

    #[test]
    fn music_takes_its_length_and_bpm_and_a_fan_out_its_pick() {
        let track = preset("bundled:expanded-prompt-to-music");
        let (graph, _, _) = apply_request(&track, &RunRequest { bpm: Some(124.0), ..RunRequest::new("AI").seconds(90.0) }).unwrap();
        assert_eq!(param(&graph, "music", "seconds"), Some(Literal::Num(90.0)));
        assert_eq!(param(&graph, "published", "bpm"), Some(Literal::Num(124.0)));
        let candidates = preset("ai:image-candidates");
        let (graph, text, _) = apply_request(&candidates, &RunRequest { pick: Some(2), ..RunRequest::new("AI").text("seeds", "[1, 2, 3]") }).unwrap();
        let pick = graph.nodes.iter().find(|node| node.kind == "choose").unwrap();
        assert!(pick.params.iter().any(|(name, value)| name == "pick" && *value == Literal::Num(2.0))
            || pick.inputs.iter().any(|input| input.port == "pick"), "{pick:?}");
        assert_eq!(text.get("seeds").map(String::as_str), Some("[1, 2, 3]"));
        // The prepared graph still writes and evaluates.
        let source = makepad_flow::graph::write(&graph).unwrap();
        makepad_flow::graph::evaluate(&source, "candidates.splash").unwrap();
    }
}
