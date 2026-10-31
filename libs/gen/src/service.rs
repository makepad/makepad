//! Generation for a shell without a studio: presets run on a headless
//! [`GenHost`], jobs tracked across tool calls, fixed-seed runs reused from
//! the app's result store. Blocking; call it from a worker.

use crate::catalog::{self, PresetInfo};
use crate::digest::{self, RecipeRun};
use crate::host::{GenHost, GenHostConfig, GenRunHandle};
use crate::tools::{self, AssetResult, Choice, GenCall, JobView, Phase, Readiness, RunRequest};
use makepad_ai_hub::fleet;
use makepad_ai_hub::job::fleet_snapshots;
use makepad_ai_hub::protocol::GenerateRequestJson;
use makepad_flow::engine::results::{ResultRef, ResultStore};
use makepad_flow::{CreateInstanceRequest, Graph, InputValueDto, Literal, NodeState, PortType, RunState};
use makepad_strict_json::Value as Json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long `gen.run` and `gen.job` wait before answering with the job id.
pub const WAIT_BUDGET: Duration = Duration::from_secs(10 * 60);
const POLL: Duration = Duration::from_millis(250);
const FLEET_TTL: Duration = Duration::from_secs(10);

struct Job {
    preset: String,
    graph: Graph,
    run: Option<GenRunHandle>,
    started: Instant,
    seed: u64,
    /// The view of a job that ended (or never ran: a reused result).
    ended: Option<JobView>,
}

pub struct GenService {
    host: GenHost,
    results: Arc<dyn ResultStore>,
    namespace: String,
    jobs: Mutex<HashMap<String, Job>>,
    next: AtomicU64,
    fleet: Mutex<Option<(Instant, Vec<fleet::BoxSnapshot>)>>,
}

fn job_id(n: u64) -> String {
    format!("gen-{n}")
}

/// A fresh seed from the OS entropy behind `RandomState`.
fn draw_seed() -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    RandomState::new().build_hasher().finish() % 1_000_000_000
}

fn set_param(graph: &mut Graph, node: &str, key: &str, value: Literal) {
    if let Some(node) = graph.nodes.iter_mut().find(|candidate| candidate.id == node) {
        match node.params.iter_mut().find(|(name, _)| name == key) {
            Some((_, slot)) => *slot = value,
            None => node.params.push((key.to_string(), value)),
        }
    }
}

fn media_matches(ty: &PortType, content_type: &str) -> bool {
    match ty {
        PortType::Image => content_type.starts_with("image/"),
        PortType::Video => content_type.starts_with("video/"),
        PortType::Audio => content_type.starts_with("audio/"),
        PortType::Mesh => content_type.starts_with("model/") || content_type.contains("ply"),
        _ => false,
    }
}

impl GenService {
    pub fn start(config: GenHostConfig) -> Result<GenService, String> {
        let results = config.results.clone();
        let namespace = config.namespace.clone();
        Ok(GenService {
            host: GenHost::start(config)?,
            results,
            namespace,
            jobs: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            fleet: Mutex::new(None),
        })
    }

    /// Where this service publishes.
    pub fn results(&self) -> &Arc<dyn ResultStore> {
        &self.results
    }

    /// The fleet as the scorer reads it, probed at most every 10 seconds.
    fn fleet(&self) -> Vec<fleet::BoxSnapshot> {
        let mut cached = self.fleet.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, snapshots)) = cached.as_ref() {
            if at.elapsed() < FLEET_TTL {
                return snapshots.clone();
            }
        }
        let snapshots = fleet_snapshots(&[]);
        *cached = Some((Instant::now(), snapshots.clone()));
        snapshots
    }

    fn readiness(&self, info: &PresetInfo, params: &[(String, Json)]) -> (Readiness, Vec<Json>) {
        let snapshots = self.fleet();
        let mut ready = Readiness::default();
        let mut rows = Vec::new();
        let mut total = 0.0;
        for generator in &info.generators {
            let mut request = GenerateRequestJson { model: generator.model.clone(), ..Default::default() };
            for (name, value) in params {
                match (name.as_str(), value) {
                    ("model", Json::Str(model)) => request.model = model.clone(),
                    ("width", Json::Int(n)) => request.width = Some(*n as u32),
                    ("height", Json::Int(n)) => request.height = Some(*n as u32),
                    ("steps", Json::Int(n)) => request.steps = Some(*n as u32),
                    ("frames", Json::Int(n)) => request.frames = Some(*n as u32),
                    ("seconds", Json::Int(n)) => request.seconds = Some(*n as f64),
                    ("seconds", Json::F64(n)) => request.seconds = Some(*n),
                    _ => {}
                }
            }
            let pick = if request.model.is_empty() {
                fleet::pick_for_domain_eta_request(&snapshots, &generator.domain, &request).map(|(index, model, eta)| (index, model, eta))
            } else {
                fleet::pick_for_model_eta(&snapshots, &request.model, &request).map(|(index, eta)| (index, request.model.clone(), eta))
            };
            match pick {
                Some((index, model, eta)) => {
                    total += eta as f64 / 1000.0;
                    rows.push(Json::Obj(vec![
                        ("stage".into(), Json::Str(generator.node.clone())),
                        ("domain".into(), Json::Str(generator.domain.clone())),
                        ("model".into(), Json::Str(model)),
                        ("node".into(), Json::Str(fleet::node_label(&snapshots[index].base_url))),
                        ("eta_seconds".into(), Json::Int((eta / 1000) as i64)),
                    ]));
                }
                None => {
                    let model = if request.model.is_empty() { "any model".to_string() } else { request.model.clone() };
                    ready.missing.push(format!("{}: {model}", generator.domain));
                    rows.push(Json::Obj(vec![
                        ("stage".into(), Json::Str(generator.node.clone())),
                        ("domain".into(), Json::Str(generator.domain.clone())),
                        ("error".into(), Json::Str(fleet::unroutable_request_error(&snapshots, &generator.domain, &request))),
                    ]));
                }
            }
        }
        ready.eta_secs = (!info.generators.is_empty() && ready.missing.is_empty()).then_some(total);
        (ready, rows)
    }

    /// The catalog, with what the fleet can run now.
    pub fn presets(&self, kind: Option<&str>) -> Result<Json, String> {
        if let Some(kind) = kind.filter(|kind| !catalog::KINDS.contains(kind)) {
            return Err(format!("unknown kind `{kind}`: one of {}", catalog::KINDS.join(", ")));
        }
        let presets = catalog::presets().into_iter().filter(|info| kind.is_none_or(|kind| info.kind == kind)).map(|info| {
            let (ready, _) = self.readiness(&info, &[]);
            tools::preset_json(&info, Some(&ready))
        }).collect();
        Ok(Json::Obj(vec![("presets".into(), Json::Arr(presets))]))
    }

    pub fn estimate(&self, preset: &str, params: &[(String, Json)]) -> Result<Json, String> {
        let entry = catalog::preset(preset).ok_or_else(|| format!("unknown preset `{preset}`: gen.presets lists them"))?;
        let info = catalog::describe(entry.id, entry.kind, &catalog::graph(entry.source, &format!("{preset}.splash"))?);
        let (ready, rows) = self.readiness(&info, params);
        let mut fields = vec![("preset".to_string(), Json::Str(preset.to_string())), ("generators".to_string(), Json::Arr(rows))];
        if let Some(eta) = ready.eta_secs {
            fields.push(("eta_seconds".into(), Json::Int(eta.round() as i64)));
        }
        if !ready.missing.is_empty() {
            fields.push(("missing".into(), Json::Arr(ready.missing.into_iter().map(Json::Str).collect())));
        }
        Ok(Json::Obj(fields))
    }

    /// Start a preset; with `wait`, block until it ends, stops at a
    /// choice, `cancelled` says so, or the wait budget runs out.
    pub fn run(&self, request: &RunRequest, wait: bool, cancelled: &dyn Fn() -> bool, progress: &mut dyn FnMut(&str, u16)) -> Result<JobView, String> {
        let entry = catalog::preset(&request.preset)
            .ok_or_else(|| format!("unknown preset `{}`: gen.presets lists them", request.preset))?;
        let file_name = format!("{}.splash", entry.id);
        let mut graph = catalog::graph(entry.source, &file_name)?;
        let info = catalog::describe(entry.id, entry.kind, &graph);
        let mut inputs: HashMap<String, HashMap<String, InputValueDto>> = HashMap::new();
        let text_value = |text: &str| HashMap::from([("text".to_string(), InputValueDto {
            ty: PortType::Text, text: Some(text.to_string()), json: None, digest: None, values: None,
        })]);
        // The run's own words and settings, as the digest names them.
        let mut named: Vec<(String, Json)> = Vec::new();
        match (&info.prompt, request.prompt.as_deref().map(str::trim).filter(|prompt| !prompt.is_empty())) {
            (Some(_), Some(prompt)) => {
                inputs.insert("prompt".into(), text_value(prompt));
                named.push(("prompt".into(), Json::Str(prompt.to_string())));
            }
            (Some(_), None) => return Err(format!("`{}` needs a prompt", entry.id)),
            (None, Some(_)) => return Err(format!("`{}` takes no prompt; its inputs are assets", entry.id)),
            (None, None) => {}
        }
        for (name, value) in &request.params {
            if let Some(text) = info.texts.iter().find(|text| &text.node == name) {
                let Json::Str(value) = value else { return Err(format!("`{name}` is text")) };
                inputs.insert(text.node.clone(), text_value(value));
            } else if name == "model" || info.params.iter().any(|(known, _)| known == name) {
                let literal = tools::json_literal(value);
                let mut set = false;
                for node in graph.nodes.iter_mut().filter(|node| node.kind == "gen") {
                    if name == "model" || node.params.iter().any(|(known, _)| known == name) {
                        match node.params.iter_mut().find(|(known, _)| known == name) {
                            Some((_, slot)) => *slot = literal.clone(),
                            None => node.params.push((name.clone(), literal.clone())),
                        }
                        set = true;
                    }
                }
                debug_assert!(set);
            } else {
                let known: Vec<&str> = info.texts.iter().map(|text| text.node.as_str()).chain(info.params.iter().map(|(name, _)| name.as_str())).chain(["model"]).collect();
                return Err(format!("`{}` has no param `{name}`: it takes {}", entry.id, known.join(", ")));
            }
            named.push((name.clone(), value.clone()));
        }
        for text in info.texts.iter().filter(|text| text.default.is_empty()) {
            if !request.params.iter().any(|(name, value)| name == &text.node && matches!(value, Json::Str(value) if !value.trim().is_empty())) {
                return Err(format!("`{}` needs `{}` in params ({})", entry.id, text.node, text.doc));
            }
        }
        if request.inputs.len() != info.media.len() {
            let wanted: Vec<String> = info.media.iter().map(|input| format!("{} ({})", input.node, input.ty.as_str())).collect();
            return Err(if wanted.is_empty() { format!("`{}` takes no input assets", entry.id) }
                else { format!("`{}` takes {} input asset(s): {}", entry.id, wanted.len(), wanted.join(", ")) });
        }
        let mut input_digests = Vec::new();
        for (input, reference) in info.media.iter().zip(&request.inputs) {
            let content = self.results.read(&result_ref(reference)).map_err(|error| format!("read input `{reference}`: {error}"))?;
            if !media_matches(&input.ty, &content.content_type) {
                return Err(format!("input `{reference}` is {}, and `{}` takes {}", content.content_type, input.node, input.ty.as_str()));
            }
            let digest = self.host.client().put_value(input.ty.clone(), &content.content_type, &content.bytes)
                .map_err(|error| format!("stage input `{reference}`: {error}"))?.digest;
            inputs.insert(input.node.clone(), HashMap::from([(input.ty.as_str().to_string(), InputValueDto {
                ty: input.ty.clone(), text: None, json: None, digest: Some(digest.clone()), values: None,
            })]));
            input_digests.push(digest);
        }
        let seed = request.seed.unwrap_or_else(draw_seed);
        let recipe_digest = digest::recipe_digest(&RecipeRun { recipe: entry.id, source: entry.source, params: &named, seed, inputs: &input_digests });
        let id = job_id(self.next.fetch_add(1, Ordering::Relaxed));
        let started = Instant::now();
        if request.seed.is_some() {
            if let Some((found, info)) = digest::find(self.results.as_ref(), &recipe_digest) {
                let view = JobView {
                    id: id.clone(), preset: entry.id.to_string(), phase: Phase::Done, progress: 1.0, seed: Some(seed), reused: true,
                    assets: vec![AssetResult { node: "published".into(), asset_id: found.id, revision: found.revision.unwrap_or_default(),
                        alias: Some(digest::reuse_alias(&self.namespace, &recipe_digest)), kind: entry.kind.to_string(), title: info.title }],
                    ..Default::default()
                };
                self.jobs.lock().unwrap_or_else(|e| e.into_inner()).insert(id, Job {
                    preset: entry.id.into(), graph, run: None, started, seed, ended: Some(view.clone()),
                });
                return Ok(view);
            }
        }
        // Every generator gets its own seed from the run's, in document
        // order, so the whole run repeats from one number.
        let mut offset = 0u64;
        let title = title_for(&info.label, request.prompt.as_deref().unwrap_or(""));
        let digest_hex = digest::hex(&recipe_digest);
        let reuse_alias = request.seed.is_some().then(|| digest::reuse_alias(&self.namespace, &recipe_digest));
        let nodes: Vec<(String, String)> = graph.nodes.iter().map(|node| (node.id.clone(), node.kind.clone())).collect();
        for (node, kind) in nodes {
            match kind.as_str() {
                "gen" => {
                    set_param(&mut graph, &node, "seed", Literal::Num(seed.wrapping_add(offset) as f64));
                    offset += 1;
                }
                "publish" => {
                    set_param(&mut graph, &node, "namespace", Literal::Str(self.namespace.clone()));
                    set_param(&mut graph, &node, "recipe_digest", Literal::Str(digest_hex.clone()));
                    set_param(&mut graph, &node, "title", Literal::Str(if node == "published" { title.clone() } else { format!("{title} · {node}") }));
                    if let (Some(alias), true) = (&reuse_alias, node == "published") {
                        set_param(&mut graph, &node, "alias", Literal::Str(alias.to_string()));
                    }
                }
                // A media input's value arrives with the instance; its
                // design value is none.
                "input" if info.media.iter().any(|input| input.node == node) => set_param(&mut graph, &node, "value", Literal::Null),
                _ => {}
            }
        }
        let source = makepad_flow::graph::write_with_registry(&graph, catalog::registry()).map_err(|error| format!("{error:?}"))?;
        let name = format!("gen-{}-{}", entry.id.replace('.', "-"), &digest_hex[..16]);
        self.host.client().put_source(&name, &source).map_err(|error| error.to_string())?;
        let run = self.host.submit(&name, CreateInstanceRequest { label: Some(entry.id.to_string()), inputs: Some(inputs), pin: Some(false) })?;
        self.jobs.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone(), Job {
            preset: entry.id.into(), graph, run: Some(run), started, seed, ended: None,
        });
        if wait { self.wait(&id, cancelled, progress) } else { self.view(&id) }
    }

    /// Where a job is now.
    pub fn view(&self, id: &str) -> Result<JobView, String> {
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        let job = jobs.get_mut(id).ok_or_else(|| format!("no generation job `{id}`"))?;
        if let Some(view) = &job.ended {
            return Ok(view.clone());
        }
        let run = job.run.clone().expect("a running job has a run");
        let row = self.host.snapshot(&run)?;
        let mut view = JobView {
            id: id.to_string(), preset: job.preset.clone(), seed: Some(job.seed), seconds: job.started.elapsed().as_secs_f64(),
            ..Default::default()
        };
        view.phase = match row.state {
            RunState::Done => Phase::Done,
            RunState::Failed => Phase::Failed,
            RunState::Cancelled => Phase::Cancelled,
            RunState::Waiting => Phase::Choosing,
            _ => Phase::Running,
        };
        let planned = row.planned_nodes.len().max(1) as f32;
        let mut done = 0.0;
        for node in &row.planned_nodes {
            let Some(status) = row.nodes.get(node) else { continue };
            match status.state {
                NodeState::Done | NodeState::Skipped => done += 1.0,
                NodeState::Running => {
                    done += status.progress.unwrap_or(0) as f32 / 1000.0;
                    if let Some(stage) = &status.stage {
                        view.stage = format!("{node}: {stage}");
                    }
                }
                NodeState::Failed if view.error.is_none() => view.error = status.error.clone().map(|error| format!("{node}: {error}")),
                NodeState::Waiting => {
                    let question = job.graph.nodes.iter().find(|candidate| &candidate.id == node)
                        .and_then(|candidate| candidate.params.iter().find(|(name, _)| name == "question"))
                        .map(|(_, value)| match value { Literal::Str(text) => text.clone(), _ => String::new() }).unwrap_or_default();
                    let candidates = row.nodes.iter().filter(|(other, _)| job.graph.nodes.iter().any(|candidate| &candidate.id == *other && candidate.kind == "map"))
                        .map(|(_, status)| status.sequences.iter().flatten().map(|sequence| sequence.values.len()).max().unwrap_or(0)).max().unwrap_or(0);
                    view.choice = Some(Choice { node: node.clone(), question, candidates });
                }
                _ => {}
            }
        }
        view.progress = done / planned;
        for node in &job.graph.nodes {
            let Some(status) = row.nodes.get(&node.id).filter(|status| status.state == NodeState::Done) else { continue };
            let value = |port: &str| -> Result<Option<String>, String> {
                let Some(output) = status.outputs.iter().find(|output| output.port == port) else { return Ok(None) };
                let bytes = self.host.client().value(&output.value.digest).map_err(|error| error.to_string())?;
                String::from_utf8(bytes.bytes.to_vec()).map(Some).map_err(|error| error.to_string())
            };
            match node.kind.as_str() {
                "publish" => {
                    let Some(text) = value("asset")? else { continue };
                    let asset = makepad_strict_json::parse(text.as_bytes()).map_err(|error| error.to_string())?;
                    let field = |name: &str| asset.get(name).and_then(Json::as_str).unwrap_or_default().to_string();
                    view.assets.push(AssetResult {
                        node: node.id.clone(), asset_id: field("id"), revision: field("revision"),
                        alias: asset.get("alias").and_then(Json::as_str).map(str::to_string), kind: field("kind"), title: field("title"),
                    });
                }
                "provider" if node.type_name == "Validate" => {
                    if let Some(text) = value("facts")? {
                        view.character = Some(makepad_strict_json::parse(text.as_bytes()).map_err(|error| error.to_string())?);
                    }
                }
                "output" => {
                    if node.inputs.first().is_some_and(|input| input.ty == PortType::Text) {
                        if let Some(text) = row.outputs.get(&node.id).map(|value| value.digest.clone()) {
                            let bytes = self.host.client().value(&text).map_err(|error| error.to_string())?;
                            view.text.push((node.id.clone(), String::from_utf8_lossy(&bytes.bytes).into_owned()));
                        }
                    }
                }
                _ => {}
            }
        }
        // The primary product first.
        view.assets.sort_by_key(|asset| asset.node != "published");
        if view.phase.ended() {
            job.ended = Some(view.clone());
        }
        Ok(view)
    }

    /// Block on a job until it ends or stops at a choice, `cancelled` says
    /// so (the job is cancelled), or the wait budget runs out.
    pub fn wait(&self, id: &str, cancelled: &dyn Fn() -> bool, progress: &mut dyn FnMut(&str, u16)) -> Result<JobView, String> {
        let deadline = Instant::now() + WAIT_BUDGET;
        loop {
            if cancelled() {
                return self.cancel(id);
            }
            let view = self.view(id)?;
            if view.phase != Phase::Running || Instant::now() >= deadline {
                return Ok(view);
            }
            progress(&view.stage, (view.progress * 1000.0) as u16);
            std::thread::sleep(POLL);
        }
    }

    pub fn choose(&self, id: &str, pick: usize) -> Result<JobView, String> {
        let view = self.view(id)?;
        let choice = view.choice.ok_or_else(|| format!("job `{id}` is not waiting at a choice"))?;
        let instance = self.jobs.lock().unwrap_or_else(|e| e.into_inner()).get(id).and_then(|job| job.run.clone()).map(|run| run.instance)
            .ok_or_else(|| format!("no generation job `{id}`"))?;
        let answer = makepad_flow::engine::executors::choose::choice_answer(pick);
        let dto = Json::Obj(vec![(choice.node.clone(), Json::Obj(vec![(makepad_flow::engine::executors::choose::PICK_PORT.to_string(),
            Json::Obj(vec![("type".into(), Json::Str(answer.ty.as_str().to_string())), ("json".into(), Json::Int(pick as i64))]))]))]);
        self.host.client().put_inputs(&instance, &choice.node, &dto).map_err(|error| error.to_string())?;
        self.view(id)
    }

    pub fn cancel(&self, id: &str) -> Result<JobView, String> {
        let run = self.jobs.lock().unwrap_or_else(|e| e.into_inner()).get(id).map(|job| job.run.clone())
            .ok_or_else(|| format!("no generation job `{id}`"))?;
        if let Some(run) = run {
            self.host.cancel(&run)?;
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let view = self.view(id)?;
            if view.phase.ended() || Instant::now() >= deadline {
                return Ok(view);
            }
            std::thread::sleep(POLL);
        }
    }

    /// One `gen.*` tool call, answered as the tools answer.
    pub fn call(&self, call: &GenCall, cancelled: &dyn Fn() -> bool, progress: &mut dyn FnMut(&str, u16)) -> Result<Json, String> {
        match call {
            GenCall::Presets { kind } => self.presets(kind.as_deref()),
            GenCall::Run { request, wait } => self.run(request, *wait, cancelled, progress).map(|view| tools::job_json(&view)),
            GenCall::Job { id, wait } => if *wait { self.wait(id, cancelled, progress) } else { self.view(id) }.map(|view| tools::job_json(&view)),
            GenCall::Choose { id, pick } => self.choose(id, *pick).map(|view| tools::job_json(&view)),
            GenCall::Cancel { id } => self.cancel(id).map(|view| tools::job_json(&view)),
            GenCall::Estimate { preset, params } => self.estimate(preset, params),
        }
    }
}

/// `<preset> · <the prompt's first words>`.
pub fn title_for(label: &str, prompt: &str) -> String {
    let prompt = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if prompt.is_empty() {
        return label.to_string();
    }
    let mut excerpt: String = prompt.chars().take(56).collect();
    if prompt.chars().count() > 56 {
        excerpt = excerpt.trim_end().to_string() + "…";
    }
    format!("{label} · {excerpt}")
}

/// An input named by id, or by alias (`namespace/name`).
fn result_ref(reference: &str) -> ResultRef {
    if reference.contains('/') { ResultRef::alias(reference) } else { ResultRef::id(reference) }
}
