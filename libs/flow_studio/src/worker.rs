//! The studio worker: one long-lived thread fed by a bounded channel. It
//! owns everything slow or blocking the studio needs — evaluating and
//! writing Splash, the host's result store (opened here), hub probes,
//! reading stored media for inputs and decoding previews — so the UI thread
//! only moves small results.

use super::models::{self, GenMode, HubModel, HubSnapshot};
use super::prepare::{prepare, with_input_values, Provenance, STUDIO_TAG};
use super::presets::{self, Group, Preset};
use makepad_flow::engine::results::{
    PublishEvent, ResultFile, ResultFiles, ResultKind, ResultMedia, ResultQuery, ResultRef, ResultRole, ResultStore,
    ResultThumb, StoreStatus, PRESET_TAG,
};
use makepad_flow::{Graph, PortType, Value};
use makepad_widgets::*;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::Arc;

const PRESET_PAGE: u32 = 60;
/// A preview's long side, in pixels.
const PREVIEW_EDGE: usize = 360;
/// A library tile's long side.
const MAX_MEDIA_BYTES: u64 = 512 * 1024 * 1024;
const WAVE_BINS: usize = 120;

/// A picture decoded for the UI: `0xAARRGGBB` pixels.
#[derive(Clone, Debug)]
pub struct Thumb {
    pub width: usize,
    pub height: usize,
    pub pixels: Arc<Vec<u32>>,
}

/// What a node face shows.
#[derive(Clone, Debug)]
pub enum Face {
    Picture(Thumb, String),
    Wave(Arc<Vec<f32>>, String),
    Text(String),
}

/// A stored result chosen for a media input.
#[derive(Clone, Debug)]
pub struct MediaPick {
    pub node: String,
    pub port: String,
    pub ty: PortType,
    pub reference: ResultRef,
}

/// A run made ready: its source and media inputs, or the stored product a
/// repeated fixed-seed run already made.
pub enum Prepared {
    Run(String, Vec<(String, String, Value)>),
    /// `title`: the store's title for it (empty when it has none).
    Reused { node: String, reference: ResultRef, title: String, kind: String },
}

fn set_param(node: &mut makepad_flow::Node, key: &str, value: makepad_flow::Literal) {
    match node.params.iter_mut().find(|(name, _)| name == key) {
        Some((_, slot)) => *slot = value,
        None => node.params.push((key.to_string(), value)),
    }
}

pub enum Job {
    Boot,
    Catalogue,
    Saved,
    Probe(GenMode),
    Prepare { run: u64, graph: Arc<Graph>, provenance: Provenance, media: Vec<MediaPick> },
    Preview { run: u64, node: String, value: Value },
    Poster { run: u64, node: String, asset: ResultRef },
    SavePreset { name: String, group: Group, graph: Arc<Graph>, values: Vec<(String, String)> },
}

pub enum Done {
    /// `retry`: the host's store is not up yet (it may start with the app);
    /// ask again shortly. The engines wait for it, so no result is lost.
    Booted { results: Option<Arc<dyn ResultStore>>, note: String, retry: bool },
    Catalogue { presets: Vec<Preset>, errors: Vec<String> },
    Saved(Result<Vec<Preset>, String>),
    Probe(HubSnapshot),
    Prepared { run: u64, result: Result<Prepared, String> },
    Preview { run: u64, node: String, face: Face },
    PresetSaved(Result<String, String>),
}

pub struct StudioWorker {
    jobs: Option<SyncSender<Job>>,
    done: Receiver<Done>,
    pub error: Option<String>,
}

impl StudioWorker {
    pub fn new(cx: &mut Cx) -> Self {
        let (job_tx, job_rx) = sync_channel::<Job>(16);
        let (done_tx, done_rx) = sync_channel::<Done>(16);
        let options = ThreadOptions { name: Some(Arc::from("flow-studio")), stack_size: Some(8 * 1024 * 1024), priority: CxThreadPriority::Utility };
        let mut worker = Self { jobs: None, done: done_rx, error: None };
        match cx.thread_spawner().spawn_worker(options, move || run(job_rx, done_tx)) {
            Ok(handle) => {
                handle.detach();
                worker.jobs = Some(job_tx);
            }
            Err(error) => worker.error = Some(format!("flow studio worker unavailable: {error:?}")),
        }
        worker
    }

    /// `Err(job)` when the queue is full (keep it for the next pump) or the
    /// worker is gone.
    pub fn try_send(&self, job: Job) -> Result<(), Job> {
        let Some(jobs) = &self.jobs else { return Err(job) };
        jobs.try_send(job).map_err(|error| match error {
            TrySendError::Full(job) | TrySendError::Disconnected(job) => job,
        })
    }

    pub fn alive(&self) -> bool {
        self.jobs.is_some()
    }

    pub fn try_recv(&self) -> Option<Done> {
        self.done.try_recv().ok()
    }

    pub fn stop(&mut self) {
        self.jobs.take();
    }
}

struct State {
    results: Option<Arc<dyn ResultStore>>,
    registry: Option<Arc<makepad_flow::document::Registry>>,
    /// Each node's models as it last answered: a node busy loading a model
    /// may miss one probe, and keeps its models while its beacon lives.
    known: std::collections::HashMap<String, Vec<HubModel>>,
}

impl State {
    fn results(&self) -> Result<&Arc<dyn ResultStore>, String> {
        self.results.as_ref().ok_or_else(|| "the result store is not open".to_string())
    }

    fn registry(&mut self) -> Result<Arc<makepad_flow::document::Registry>, String> {
        if self.registry.is_none() {
            self.registry = Some((crate::host::host().registries)()?.0);
        }
        Ok(self.registry.clone().unwrap())
    }
}

fn run(jobs: Receiver<Job>, done: SyncSender<Done>) {
    let mut state = State { results: None, registry: None, known: Default::default() };
    while let Ok(job) = jobs.recv() {
        let replies = handle(&mut state, job);
        for reply in replies {
            // The UI takes replies every pump; waiting here is this thread's
            // own time, never the UI's.
            if done.send(reply).is_err() {
                return;
            }
        }
    }
    state.results = None;
}

fn handle(state: &mut State, job: Job) -> Vec<Done> {
    match job {
        Job::Boot => {
            let host = crate::host::host();
            let reply = match (host.open_results)() {
                crate::host::Opening::Open(store) => {
                    state.results = Some(store.clone());
                    match store.status() {
                        StoreStatus::Ready => Done::Booted { results: Some(store), note: format!("results go to {}", host.library), retry: false },
                        StoreStatus::Starting(note) => Done::Booted { results: None, note: format!("waiting for {} ({note})", host.library), retry: true },
                        StoreStatus::Failed(error) => Done::Booted { results: None, note: format!("results cannot be stored: {error}"), retry: false },
                    }
                }
                crate::host::Opening::Starting(note) => Done::Booted { results: None, note: format!("waiting for {} ({note})", host.library), retry: true },
                crate::host::Opening::Failed(error) => Done::Booted { results: None, note: format!("results cannot be stored: {error}"), retry: false },
            };
            vec![reply]
        }
        Job::Catalogue => {
            let mut presets = Vec::new();
            let mut errors = Vec::new();
            for (key, group, template_group, source, file) in presets::sources() {
                match Preset::describe(key.clone(), group, template_group, source, file) {
                    Ok(preset) => presets.push(preset),
                    Err(error) => errors.push(format!("{key}: {error}")),
                }
            }
            vec![Done::Catalogue { presets, errors }]
        }
        Job::Saved => vec![Done::Saved(saved_presets(state))],
        Job::Probe(mode) => vec![Done::Probe(probe(mode, &mut state.known))],
        Job::Prepare { run, graph, provenance, media } => {
            let result = (|| {
                let registry = state.registry()?;
                let mut prepared = prepare(&graph, &provenance);
                let mut inputs = Vec::new();
                for pick in media {
                    let store = state.results()?;
                    let value = read_media(store.as_ref(), &pick)?;
                    // Published results then name this one as a parent.
                    store.note_origin(value.digest, &pick.reference);
                    inputs.push((pick.node, pick.port, value));
                }
                // Every generator on a fixed seed: the run is repeatable, so
                // its products carry its recipe digest, and one already
                // stored is its answer (the one generation cache).
                let fixed = prepared.nodes.iter().filter(|node| node.kind == "gen")
                    .all(|node| matches!(node.params.iter().find(|(name, _)| name == "seed"), Some((_, makepad_flow::Literal::Num(_)))));
                if fixed && prepared.nodes.iter().any(|node| node.kind == "gen") {
                    let source = makepad_flow::graph::write_with_registry(&prepared, &registry).map_err(|error| error.to_string())?;
                    let digests: Vec<String> = inputs.iter().map(|(_, _, value): &(String, String, Value)| makepad_gen::digest::hex(&value.digest)).collect();
                    let digest = makepad_gen::digest::recipe_digest(&makepad_gen::digest::RecipeRun {
                        recipe: &provenance.preset, source: &source, params: &[], seed: 0, inputs: &digests,
                    });
                    let primary = prepared.nodes.iter().find(|node| node.kind == "publish" && node.id == "published").map(|node| {
                        let namespace = presets::text_param(node, "namespace").filter(|namespace| !namespace.is_empty()).unwrap_or_else(|| "flows".into());
                        (node.id.clone(), namespace)
                    });
                    if let Some((node, _)) = &primary {
                        if let Some((reference, info)) = makepad_gen::digest::find(state.results()?.as_ref(), &digest) {
                            return Ok(Prepared::Reused { node: node.clone(), reference, title: info.title, kind: info.kind.name().to_string() });
                        }
                    }
                    let hex = makepad_gen::digest::hex(&digest);
                    for node in prepared.nodes.iter_mut().filter(|node| node.kind == "publish") {
                        set_param(node, "recipe_digest", makepad_flow::Literal::Str(hex.clone()));
                        if primary.as_ref().is_some_and(|(id, _)| id == &node.id) {
                            let alias = makepad_gen::digest::reuse_alias(&primary.as_ref().unwrap().1, &digest);
                            set_param(node, "alias", makepad_flow::Literal::Str(alias.to_string()));
                        }
                    }
                }
                let source = makepad_flow::graph::write_with_registry(&prepared, &registry).map_err(|error| error.to_string())?;
                Ok(Prepared::Run(source, inputs))
            })();
            vec![Done::Prepared { run, result }]
        }
        Job::Preview { run, node, value } => match face_of(&value) {
            Some(face) => vec![Done::Preview { run, node, face }],
            None => Vec::new(),
        },
        Job::Poster { run, node, asset } => {
            let Ok(store) = state.results() else { return Vec::new() };
            match stored_thumbnail(store.as_ref(), &asset, PREVIEW_EDGE) {
                Some(thumb) => vec![Done::Preview { run, node, face: Face::Picture(thumb, "published".into()) }],
                None => Vec::new(),
            }
        }
        Job::SavePreset { name, group, graph, values } => vec![Done::PresetSaved(save_preset(state, &name, group, &graph, &values))],
    }
}

/// What the hub(s) have. HTTP to the hubs the generation route would use,
/// file stats for this machine's weights.
fn probe(mode: GenMode, known: &mut std::collections::HashMap<String, Vec<HubModel>>) -> HubSnapshot {
    let mut snapshot = HubSnapshot { mode: Some(mode.clone()), local: models::local_models(), ..Default::default() };
    let urls: Vec<String> = match &mode {
        GenMode::Fleet => {
            let urls = lan_nodes();
            // Nodes whose beacon stopped are forgotten.
            known.retain(|node, _| urls.iter().any(|url| &makepad_ai_hub::fleet::node_label(url) == node));
            urls
        }
        GenMode::Preview => return snapshot,
        GenMode::Fixed(url) => vec![url.clone()],
    };
    let mut errors = Vec::new();
    for url in urls {
        // Health and models together: what the fleet's ETA scoring reads.
        match makepad_ai_hub::job::probe_node(&url) {
            Ok(boxed) => {
                let node = makepad_ai_hub::fleet::node_label(&url);
                let models: Vec<HubModel> = boxed.models.iter().map(|info| HubModel::from_info(&node, info)).collect();
                snapshot.boxes.push(boxed);
                known.insert(node.clone(), models.clone());
                snapshot.models.extend(models);
                snapshot.hubs.push(node);
            }
            Err(error) => {
                let node = makepad_ai_hub::fleet::node_label(&url);
                match known.get(&node) {
                    // Busy, not gone: its beacon is still heard.
                    Some(models) => {
                        snapshot.models.extend(models.iter().cloned());
                        snapshot.hubs.push(node);
                    }
                    None => errors.push(format!("{node}: {error}")),
                }
            }
        }
    }
    if !errors.is_empty() {
        snapshot.error = Some(errors.join("; "));
    }
    snapshot
}

/// The AI Hub nodes on the LAN, from their discovery beacons: the same set
/// the fleet routing picks from. A new listener gets one beacon interval to
/// hear them, once per process.
fn lan_nodes() -> Vec<String> {
    let discovery = makepad_ai_hub::discovery::start_listener();
    static WAITED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if WAITED.get().is_none() {
        // Every node beacons once per interval: listen that long, not just
        // until the first one is heard.
        std::thread::sleep(std::time::Duration::from_millis(2_500));
        let _ = WAITED.set(());
    }
    let mut urls: Vec<String> = discovery.nodes().into_iter().map(|node| node.base_url.trim_end_matches('/').to_string()).collect();
    urls.sort();
    urls.dedup();
    urls
}

fn read_media(store: &dyn ResultStore, pick: &MediaPick) -> Result<Value, String> {
    if !matches!(pick.ty, PortType::Image | PortType::Audio | PortType::Video | PortType::Mesh) {
        return Err(format!("input `{}` takes no stored media", pick.node));
    }
    let content = store.read(&pick.reference).map_err(|error| format!("input `{}`: {error}", pick.node))?;
    if content.bytes.is_empty() || content.bytes.len() as u64 > MAX_MEDIA_BYTES {
        return Err(format!("input `{}`: the file is {} bytes", pick.node, content.bytes.len()));
    }
    Ok(Value::media(pick.ty.clone(), &content.content_type, content.bytes))
}

/// A stored result's own thumbnail (a video's poster), decoded.
fn stored_thumbnail(store: &dyn ResultStore, reference: &ResultRef, edge: usize) -> Option<Thumb> {
    let thumb = store.thumbnail(reference).ok()??;
    if thumb.bytes.is_empty() || thumb.bytes.len() > 16 * 1024 * 1024 {
        return None;
    }
    let image = if thumb.bytes.starts_with(b"\x89PNG") { ImageBuffer::from_png(&thumb.bytes).ok()? } else { ImageBuffer::from_jpg(&thumb.bytes).ok()? };
    Some(shrink(&image.data, image.width, image.height, edge))
}

/// What a finished node's value looks like on its face.
pub fn face_of(value: &Value) -> Option<Face> {
    match value.ty {
        PortType::Image => {
            let image = if value.bytes.starts_with(b"\x89PNG") { ImageBuffer::from_png(&value.bytes).ok()? } else { ImageBuffer::from_jpg(&value.bytes).ok()? };
            let caption = format!("{}×{}", image.width, image.height);
            Some(Face::Picture(shrink(&image.data, image.width, image.height, PREVIEW_EDGE), caption))
        }
        PortType::Audio => {
            let (peaks, seconds) = wav_peaks(&value.bytes, WAVE_BINS)?;
            Some(Face::Wave(Arc::new(peaks), format!("{seconds:.1} s")))
        }
        PortType::Video => Some(Face::Text(format!("video · {:.1} MB", value.bytes.len() as f64 / 1_048_576.0))),
        PortType::Mesh => Some(Face::Text(format!("mesh · {:.1} MB", value.bytes.len() as f64 / 1_048_576.0))),
        PortType::Text => {
            let text = String::from_utf8_lossy(&value.bytes);
            let text: String = text.chars().take(400).collect();
            Some(Face::Text(text))
        }
        _ => None,
    }
}

/// Nearest-neighbour shrink to `edge` on the long side (never enlarges).
fn shrink(pixels: &[u32], width: usize, height: usize, edge: usize) -> Thumb {
    let scale = (edge as f64 / width.max(height).max(1) as f64).min(1.0);
    let (w, h) = (((width as f64 * scale).round() as usize).max(1), ((height as f64 * scale).round() as usize).max(1));
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        let sy = (y * height / h).min(height.saturating_sub(1));
        for x in 0..w {
            let sx = (x * width / w).min(width.saturating_sub(1));
            out.push(pixels.get(sy * width + sx).copied().unwrap_or(0xff000000));
        }
    }
    Thumb { width: w, height: h, pixels: Arc::new(out) }
}

/// Peak levels of a PCM WAV in `bins`, and its length in seconds.
pub fn wav_peaks(bytes: &[u8], bins: usize) -> Option<(Vec<f32>, f64)> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let (mut channels, mut rate, mut bits, mut data) = (0u16, 0u32, 0u16, None);
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().ok()?) as usize;
        let body = at + 8;
        let end = body.checked_add(len)?.min(bytes.len());
        if id == b"fmt " && end >= body + 16 {
            channels = u16::from_le_bytes([bytes[body + 2], bytes[body + 3]]);
            rate = u32::from_le_bytes(bytes[body + 4..body + 8].try_into().ok()?);
            bits = u16::from_le_bytes([bytes[body + 14], bytes[body + 15]]);
        } else if id == b"data" {
            data = Some(&bytes[body..end]);
        }
        at = body + len + (len & 1);
    }
    let data = data?;
    let width = match bits { 16 => 2, 24 => 3, 32 => 4, _ => return None };
    if channels == 0 || rate == 0 {
        return None;
    }
    let frame = width * channels as usize;
    let frames = data.len() / frame;
    let mut peaks = vec![0f32; bins];
    for index in 0..frames {
        let sample = &data[index * frame..index * frame + width];
        let value = match width {
            2 => i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32768.0,
            3 => (i32::from_le_bytes([0, sample[0], sample[1], sample[2]]) >> 8) as f32 / 8_388_608.0,
            _ => f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]),
        };
        let bin = (index * bins / frames.max(1)).min(bins - 1);
        peaks[bin] = peaks[bin].max(value.abs().min(1.0));
    }
    Some((peaks, frames as f64 / rate as f64))
}

fn saved_presets(state: &mut State) -> Result<Vec<Preset>, String> {
    let store = state.results()?.clone();
    let page = store.list(ResultQuery { text: String::new(), namespace: Some("flows".into()), kind: Some(ResultKind::Flow), limit: PRESET_PAGE, cursor: None })
        .map_err(|error| format!("saved presets: {error}"))?;
    let mut presets = Vec::new();
    for item in page.items {
        let Ok(source) = store.read(&ResultRef::id(item.id.clone())) else { continue };
        let Ok(source) = String::from_utf8(source.bytes) else { continue };
        let key = format!("saved:{}", item.id);
        match Preset::describe(key, Some(Group::Saved), "", Arc::from(source.as_str()), format!("{}.splash", presets::slug(&item.title))) {
            Ok(mut preset) => {
                if !item.title.is_empty() {
                    preset.label = item.title.clone();
                }
                presets.push(preset);
            }
            Err(error) => log!("[flow-studio] saved preset {}: {error}", item.id),
        }
    }
    Ok(presets)
}

fn save_preset(state: &mut State, name: &str, group: Group, graph: &Graph, values: &[(String, String)]) -> Result<String, String> {
    let registry = state.registry()?;
    let mut graph = with_input_values(graph, values);
    graph.label = name.to_string();
    let source = makepad_flow::graph::write_with_registry(&graph, &registry).map_err(|error| error.to_string())?;
    let store = state.results()?;
    let pixels = super::preview_gen::pattern(name, 7, 256, 256);
    let png = makepad_ai_hub::testpattern::encode_png_rgba(&pixels, 256, 256).map_err(|error| error.to_string())?;
    let bytes = source.into_bytes();
    let value_digest = {
        let mut sha = makepad_ai_hub::sha256::Sha256::new();
        sha.update(&bytes);
        sha.finish()
    };
    let files = ResultFiles {
        kind: ResultKind::Flow,
        namespace: "flows".into(),
        title: name.to_string(),
        description: graph.brief.clone(),
        alias: None,
        category: None,
        tags: vec![PRESET_TAG.into(), "flow".into(), STUDIO_TAG.into(), presets::tag("group", group.label())],
        files: vec![ResultFile { role: ResultRole::Source, media: ResultMedia::Text, bytes, media_millis: 0, dims: None }],
        thumbnail: Some(ResultThumb { png, width: 256, height: 256 }),
        mesh_stats: None,
        creator: "flow-studio".into(),
        artist: String::new(),
        generator: "flow-studio".into(),
        model: String::new(),
        prompt: String::new(),
        seed: None,
        digest: None,
        generator_version: env!("CARGO_PKG_VERSION").into(),
        provenance: Vec::new(),
        value_digest,
        sources: Vec::new(),
    };
    let events = store.publish(files)?;
    loop {
        match events.recv() {
            Ok(PublishEvent::Progress(_)) => continue,
            Ok(PublishEvent::Done(result)) => return result.map(|saved| format!("saved “{name}” ({})", saved.id)),
            Err(_) => return Err("the result store stopped".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_peaks_read_the_preview_chord() {
        let wav = super::super::preview_gen::chord("x", 1, 1.0);
        let (peaks, seconds) = wav_peaks(&wav, 10).unwrap();
        assert!((seconds - 1.0).abs() < 1e-3);
        assert!(peaks.iter().all(|peak| *peak > 0.0 && *peak <= 1.0));
    }

    #[test]
    fn shrink_keeps_the_aspect_and_never_enlarges() {
        let thumb = shrink(&vec![0xffffffff; 400 * 200], 400, 200, 100);
        assert_eq!((thumb.width, thumb.height), (100, 50));
        let same = shrink(&vec![0; 20 * 10], 20, 10, 100);
        assert_eq!((same.width, same.height), (20, 10));
    }
}
