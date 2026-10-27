use super::lineage::Lineage;
use crate::engine::thumbnail::{image_thumbnail, placeholder_thumbnail};
use crate::engine::results::{
    MeshStats, PublishEvent, ResultFile, ResultFiles, ResultKind, ResultMedia, ResultRole, ResultStore, ResultThumb,
};
use super::{Executor, Poll, one_inputs, one_outputs, param, string_param};
use crate::{Literal, Node, PortType, PortValue, Value};
use makepad_ai_hub::registry::Domain;
use makepad_strict_json::Value as Json;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

/// The run a Publish belongs to, for its provenance.
#[derive(Clone, Debug, Default)]
pub struct PublishRun {
    pub flow: String,
    pub instance: String,
    pub run: String,
    pub revision: u64,
    /// The description when the node sets none: an upstream text input.
    pub description_default: String,
    pub lineage: Lineage,
}

/// The generator named in a published result's annotation and provenance.
pub const PUBLISH_GENERATOR: &str = "makepad-flow";

// Field bounds a store's catalog may enforce: a value past one is refused
// there, so fields are cut to fit here instead.
const MAX_TITLE_BYTES: usize = 200;
const MAX_DESCRIPTION_BYTES: usize = 4096;
const MAX_NAME_BYTES: usize = 128;
const MAX_PROMPT_BYTES: usize = 8192;
const MAX_LABELS: usize = 24;
const MAX_LABEL_BYTES: usize = 48;
/// A generator's model name past this is not a real record.
const MAX_MANIFEST_NAME_BYTES: usize = 64;

/// What a generated value becomes in the library: its catalog kind, file
/// role, category and tags. One row per hub domain; the nearest generator
/// upstream of a published value names the row, and a value no generator
/// made is published by its port type alone.
#[derive(Clone, Copy, Debug)]
pub struct Product {
    pub kind: ResultKind,
    pub role: ResultRole,
    pub category: &'static str,
    pub tags: &'static [&'static str],
}

pub fn product(domain: Domain) -> Product {
    const fn row(kind: ResultKind, role: ResultRole, category: &'static str, tags: &'static [&'static str]) -> Product {
        Product { kind, role, category, tags }
    }
    use ResultKind as K;
    use ResultRole as R;
    match domain {
        Domain::Image => row(K::Texture, R::Texture, "image", &[]),
        Domain::Edit => row(K::Texture, R::Texture, "image", &["edit"]),
        Domain::Upscale => row(K::Texture, R::Texture, "image", &["upscaled"]),
        Domain::Inpaint => row(K::Texture, R::Texture, "image", &["inpaint"]),
        Domain::Control => row(K::Texture, R::Texture, "image", &["control"]),
        Domain::Matte => row(K::Texture, R::Texture, "image", &["matte"]),
        Domain::Depth => row(K::Texture, R::Depth, "image", &["depth"]),
        Domain::Segment => row(K::Texture, R::Texture, "image", &["segment"]),
        Domain::Video => row(K::Video, R::Video, "generated", &[]),
        Domain::Enhance => row(K::Video, R::Video, "generated", &["enhanced"]),
        Domain::Audio => row(K::Audio, R::Audio, "sfx", &[]),
        Domain::Music => row(K::Audio, R::Audio, "music", &[]),
        Domain::Speech => row(K::Audio, R::Audio, "speech", &[]),
        Domain::Stems => row(K::Audio, R::Audio, "music", &["stem"]),
        Domain::Mesh => row(K::Mesh, R::RenderGlb, "prop", &[]),
        Domain::Paint => row(K::Mesh, R::RenderGlb, "prop", &["pbr"]),
        Domain::Rig => row(K::Character, R::RenderGlb, "dancer", &["rigged"]),
        Domain::Motion => row(K::Character, R::RenderGlb, "dancer", &["animated"]),
        Domain::Splat => row(K::World, R::Splat, "splat", &["object"]),
        Domain::World => row(K::World, R::Splat, "splat", &["world"]),
        Domain::Text | Domain::Vision | Domain::Ocr | Domain::Stt => row(K::Data, R::Source, "text", &[]),
        Domain::Body => row(K::Data, R::Source, "data", &["pose"]),
        Domain::Beats => row(K::Data, R::Source, "data", &["beats"]),
        Domain::Notes => row(K::Data, R::Source, "data", &["notes"]),
        Domain::Sections => row(K::Data, R::Source, "data", &["sections"]),
        Domain::Garment => row(K::Data, R::Source, "data", &["garment"]),
    }
}

pub struct PublishExecutor {
    store: Option<Arc<dyn ResultStore>>,
    run: PublishRun,
    queue: VecDeque<Poll>,
    reply: Option<Receiver<PublishEvent>>,
    cancelled: bool,
    published_meta: Option<(String, String, ResultKind)>,
}

impl PublishExecutor {
    pub fn new(store: Option<Arc<dyn ResultStore>>, run: PublishRun) -> Self {
        Self {
            store,
            run,
            queue: VecDeque::new(),
            reply: None,
            cancelled: false,
            published_meta: None,
        }
    }
}

impl Executor for PublishExecutor {
    fn start(&mut self, node: &Node, inputs: &[(String, PortValue)]) -> Result<(), String> {
        self.cancelled = false;
        let inputs = one_inputs(node, inputs)?;
        let value = inputs
            .iter()
            .find_map(|(name, value)| (name == "value").then_some(value))
            .ok_or_else(|| format!("Publish node `{}` has no value", node.id))?;
        let flow = self.run.flow.clone();
        let namespace = nonempty(string_param(node, "namespace"), "flows");
        let title = cut(
            &sanitize_annotation(nonempty(
                string_param(node, "title"),
                &format!("{flow} · {}", node.label.as_deref().unwrap_or(&node.id)),
            )),
            MAX_TITLE_BYTES,
        );
        let description = cut(
            &sanitize_annotation(nonempty(string_param(node, "description"), &self.run.description_default)),
            MAX_DESCRIPTION_BYTES,
        );
        let alias_text = string_param(node, "alias");
        let alias = if alias_text.trim().is_empty() {
            None
        } else {
            let full = if alias_text.contains('/') {
                alias_text
            } else {
                format!("{namespace}/{}", slug(&alias_text))
            };
            let alias_namespace = full.split('/').next().unwrap_or("");
            if alias_namespace != namespace {
                return Err(format!(
                    "Publish node `{}` alias namespace does not match `{namespace}`",
                    node.id
                ));
            }
            Some(full)
        };
        let mut tags: Vec<String> = match param(node, "tags") {
            Some(Literal::Arr(values)) if !values.is_empty() => values
                .iter()
                .filter_map(|value| match value {
                    Literal::Str(value) | Literal::Id(value) => Some(value.clone()),
                    _ => None,
                })
                .collect(),
            _ => vec!["flow".to_string(), flow.clone()],
        };
        let bpm = match param(node, "bpm") {
            Some(Literal::Num(bpm)) if bpm.is_finite() && *bpm > 0.0 => Some(*bpm),
            _ => None,
        };
        if let Some(bpm) = bpm {
            tags.push(format!("bpm-{}", bpm.round()));
        }
        let (mut kind, mut file, thumbnail) = publish_parts(value)?;
        // The nearest generator's product, when this value is its kind of
        // media (a picture made from a mesh's render stays a picture).
        let made = self.run.lineage.generators.first().and_then(|record| Domain::parse(&record.domain)).map(product)
            .filter(|made| made.role.allows(file.media) && (made.kind == kind || matches!((made.kind, kind), (ResultKind::Character, ResultKind::Mesh))));
        if let Some(made) = made {
            kind = made.kind;
            file.role = made.role;
            tags.extend(made.tags.iter().map(|tag| tag.to_string()));
        }
        let tags = labels(&tags);
        let mesh_stats = if file.media == ResultMedia::Glb { Some(glb_stats(&value.bytes)?) } else { None };
        let lineage = &self.run.lineage;
        let nearest = lineage.generators.first();
        // A host that runs a recipe names what the run makes (its recipe
        // digest: recipe, revision, params, seed and inputs); otherwise the
        // nearest generator's parameters do.
        let recipe_digest = string_param(node, "recipe_digest");
        let recipe_digest = if recipe_digest.is_empty() { None } else {
            Some(parse_digest(&recipe_digest).ok_or_else(|| format!("Publish node `{}` recipe_digest is not 64 hex digits", node.id))?)
        };
        // Only a real record: the model and seed the job reported.
        let record = nearest.filter(|record| record.seed.is_some() && !record.model.is_empty() && record.model.len() <= MAX_MANIFEST_NAME_BYTES);
        let files = ResultFiles {
            kind,
            namespace: namespace.clone(),
            title: title.clone(),
            description,
            alias,
            category: made.map(|made| made.category.to_string()),
            tags,
            files: vec![file],
            thumbnail,
            mesh_stats,
            creator: cut(&sanitize_annotation(format!("flow:{flow}/{}", self.run.instance)), MAX_NAME_BYTES),
            artist: cut(&sanitize_annotation(string_param(node, "artist")), MAX_NAME_BYTES),
            generator: PUBLISH_GENERATOR.to_string(),
            model: cut(&sanitize_annotation(nearest.map(|record| record.model.clone()).unwrap_or_default()), MAX_NAME_BYTES),
            prompt: cut(&lineage.prompt, MAX_PROMPT_BYTES),
            seed: record.and_then(|record| record.seed),
            digest: record.map(|record| recipe_digest.unwrap_or_else(|| params_digest(&record.params))),
            generator_version: env!("CARGO_PKG_VERSION").to_string(),
            provenance: provenance_fields(&self.run, &node.id, bpm),
            value_digest: value.digest,
            sources: lineage.sources.clone(),
        };
        self.published_meta = Some((namespace, title, kind));
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| "no result store is configured".to_string())?;
        self.reply = Some(store.publish(files)?);
        self.queue.push_back(Poll::Progress { permille: 50, stage: "connecting".to_string() });
        Ok(())
    }

    fn poll(&mut self) -> Poll {
        if self.cancelled {
            return Poll::Failed("publish cancelled".to_string());
        }
        if let Some(poll) = self.queue.pop_front() {
            return poll;
        }
        let Some(reply) = self.reply.as_ref() else { return Poll::Pending };
        match reply.try_recv() {
            Ok(PublishEvent::Progress(stage)) => Poll::Progress {
                permille: if stage == "uploading" { 500 } else { 50 },
                stage: stage.to_string(),
            },
            Ok(PublishEvent::Done(Ok(published))) => {
                self.reply = None;
                let (namespace, title, kind) = self.published_meta.take().unwrap();
                let json = Json::Obj(vec![
                    ("id".to_string(), Json::Str(published.id)),
                    ("revision".to_string(), Json::Str(published.revision.unwrap_or_default())),
                    ("alias".to_string(), published.alias.map(Json::Str).unwrap_or(Json::Null)),
                    ("namespace".to_string(), Json::Str(namespace)),
                    ("title".to_string(), Json::Str(title)),
                    ("kind".to_string(), Json::Str(kind.name().to_string())),
                ]);
                self.queue.push_back(Poll::Done(one_outputs(vec![("asset".to_string(), Value::json(json.to_json()))])));
                Poll::Progress { permille: 1_000, stage: "published".to_string() }
            }
            Ok(PublishEvent::Done(Err(error))) => {
                self.reply = None;
                Poll::Failed(error)
            }
            Err(TryRecvError::Empty) => Poll::Pending,
            Err(TryRecvError::Disconnected) => {
                self.reply = None;
                Poll::Failed("result store stopped during publish".to_string())
            }
        }
    }

    fn cancel(&mut self) {
        self.cancelled = true;
        self.reply = None;
        self.queue.clear();
    }
}

fn glb_stats(bytes: &[u8]) -> Result<MeshStats, String> {
    let parsed = makepad_gltf::parse_glb_bytes(bytes).map_err(|error| format!("inspect GLB: {error}"))?;
    let accessors = parsed.document.accessors.as_deref().unwrap_or(&[]);
    let meshes = parsed.document.meshes.as_deref().unwrap_or(&[]);
    let mut vertices = 0u32;
    let mut triangles = 0u32;
    for mesh in meshes {
        for primitive in &mesh.primitives {
            let count = |index: usize| accessors.get(index).map_or(0, |a| a.count.min(u32::MAX as usize) as u32);
            let positions = primitive.attributes.get("POSITION").map_or(0, |index| count(*index));
            vertices = vertices.saturating_add(positions);
            let indices = primitive.indices.map_or(positions, count);
            let faces = match primitive.mode() {
                4 => indices / 3,
                5 | 6 => indices.saturating_sub(2),
                _ => 0,
            };
            triangles = triangles.saturating_add(faces);
        }
    }
    let joints = parsed.document.skins.as_deref().unwrap_or(&[]).iter()
        .filter_map(|skin| match skin.key("joints") {
            Some(makepad_micro_serde::JsonValue::Array(joints)) => Some(joints.len()),
            _ => None,
        }).fold(0usize, usize::saturating_add).min(u16::MAX as usize) as u16;
    Ok(MeshStats {
        triangles,
        vertices,
        joints,
        clips: parsed.document.animations.as_ref().map_or(0, |v| v.len().min(u16::MAX as usize) as u16),
    })
}

/// The provenance record's fields, without `parents` (the store adds them).
fn provenance_fields(run: &PublishRun, node: &str, bpm: Option<f64>) -> Vec<(String, Json)> {
    let generators = run
        .lineage
        .generators
        .iter()
        .map(|record| {
            let mut fields = vec![
                ("node".to_string(), Json::Str(record.node.clone())),
                ("type".to_string(), Json::Str(record.type_name.clone())),
            ];
            if !record.domain.is_empty() {
                fields.push(("domain".to_string(), Json::Str(record.domain.clone())));
            }
            if !record.model.is_empty() {
                fields.push(("model".to_string(), Json::Str(record.model.clone())));
            }
            if let Some(seed) = record.seed {
                fields.push(("seed".to_string(), Json::Int(seed.min(i64::MAX as u64) as i64)));
            }
            fields.push((
                "params".to_string(),
                Json::Obj(record.params.iter().map(|(name, value)| (name.clone(), crate::values::literal_json(value))).collect()),
            ));
            Json::Obj(fields)
        })
        .collect();
    let mut fields = vec![
        ("flow".to_string(), Json::Str(run.flow.clone())),
        ("revision".to_string(), Json::Int(run.revision.min(i64::MAX as u64) as i64)),
        ("run".to_string(), Json::Str(run.run.clone())),
        ("instance".to_string(), Json::Str(run.instance.clone())),
        ("node".to_string(), Json::Str(node.to_string())),
        ("generators".to_string(), Json::Arr(generators)),
    ];
    if let Some(bpm) = bpm {
        fields.push(("bpm".to_string(), Json::F64(bpm)));
    }
    fields
}

/// Fill a request's parents from the worker's origins and complete its
/// provenance text. Returns the parents found, nearest first.
fn parse_digest(text: &str) -> Option<[u8; 32]> {
    let text = text.trim();
    if text.len() != 64 {
        return None;
    }
    let mut digest = [0u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(digest)
}

/// SHA-256 over a generator's parameters in their canonical JSON form.
fn params_digest(params: &[(String, Literal)]) -> [u8; 32] {
    let mut sorted = params.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let json = Json::Obj(sorted.iter().map(|(name, value)| (name.clone(), crate::values::literal_json(value))).collect()).to_json();
    let mut sha = crate::digest::Sha256::new();
    sha.update(json.as_bytes());
    sha.finish()
}

/// Tags as the store's labels take them (`[a-z0-9][a-z0-9_-]*`, at most 48
/// bytes, at most 24): each slugged, empty ones dropped, duplicates once.
pub fn labels(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tag in tags {
        let mut label = String::new();
        for ch in tag.trim().to_ascii_lowercase().chars() {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                label.push(ch);
            } else if !label.is_empty() && !label.ends_with('-') {
                label.push('-');
            }
        }
        let mut label = label.trim_start_matches(['-', '_']).to_string();
        label.truncate(MAX_LABEL_BYTES);
        let label = label.trim_end_matches('-').to_string();
        if label.is_empty() || !label.as_bytes()[0].is_ascii_alphanumeric() || out.contains(&label) {
            continue;
        }
        out.push(label);
    }
    if out.len() > MAX_LABELS {
        eprintln!("[flow] publish: {} tags, keeping the first {MAX_LABELS}", out.len());
        out.truncate(MAX_LABELS);
    }
    out
}

/// `text` cut to at most `max` bytes on a character boundary.
fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

fn nonempty(value: String, fallback: &str) -> String {
    if value.trim().is_empty() { fallback.to_string() } else { value }
}

fn sanitize_annotation(value: String) -> String {
    value.chars().map(|ch| if ch.is_control() { ' ' } else { ch }).collect()
}

fn slug(value: &str) -> String {
    let mut out = String::new();
    for ch in value.to_ascii_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn publish_parts(value: &Value) -> Result<(ResultKind, ResultFile, Option<ResultThumb>), String> {
    let (kind, role, media, dims) = match &value.ty {
        PortType::Image => {
            let (media, dims) = image_type_and_dims(&value.bytes, &value.content_type)?;
            (ResultKind::Texture, ResultRole::Texture, media, Some(dims))
        }
        PortType::Audio => (
            ResultKind::Audio,
            ResultRole::Audio,
            media_type(&value.content_type, &[ResultMedia::Wav, ResultMedia::Ogg, ResultMedia::Mp3])?,
            None,
        ),
        PortType::Video => (
            ResultKind::Video,
            ResultRole::Video,
            media_type(&value.content_type, &[ResultMedia::Mp4])?,
            None,
        ),
        PortType::Mesh if value.bytes.starts_with(b"ply\n") || value.bytes.starts_with(b"ply\r\n") => (
            ResultKind::World,
            ResultRole::Splat,
            ResultMedia::Ply,
            None,
        ),
        PortType::Mesh => (
            ResultKind::Mesh,
            ResultRole::RenderGlb,
            media_type(&value.content_type, &[ResultMedia::Glb])?,
            None,
        ),
        PortType::Text => (ResultKind::Data, ResultRole::Source, ResultMedia::Text, None),
        PortType::Json | PortType::List => (ResultKind::Data, ResultRole::Source, ResultMedia::Json, None),
        PortType::Bytes => (ResultKind::Data, ResultRole::Source, ResultMedia::Bin, None),
        PortType::Registered(_) => {
            return Err("registered artifact asset publication unsupported".to_string());
        }
    };
    let media_millis = match media {
        ResultMedia::Wav | ResultMedia::Ogg | ResultMedia::Mp3 => audio_duration_millis(&value.bytes, media)?,
        ResultMedia::Mp4 => video_duration_millis(&value.bytes)?,
        _ => 0,
    };
    // A video's poster is made by the store, off the engine thread.
    let thumbnail = if value.ty == PortType::Image {
        match image_thumbnail(&value.bytes, media) {
            Ok(thumbnail) => Some(thumbnail),
            Err(error) => {
                eprintln!("[flow] publish: no picture thumbnail ({error}); using the plain tile");
                Some(placeholder_thumbnail()?)
            }
        }
    } else if media == ResultMedia::Mp4 {
        None
    } else {
        Some(placeholder_thumbnail()?)
    };
    Ok((
        kind,
        ResultFile {
            bytes: value.bytes.to_vec(),
            media,
            role,
            media_millis: u32::try_from(media_millis)
                .map_err(|_| "media duration is too long".to_string())?,
            dims,
        },
        thumbnail,
    ))
}

pub(crate) fn audio_duration_millis(bytes: &[u8], media: ResultMedia) -> Result<u64, String> {
    let seconds = match media {
        ResultMedia::Wav => wav_duration_secs(bytes)?,
        ResultMedia::Ogg | ResultMedia::Mp3 => makepad_audio_decode::probe_duration(bytes)
            .map_err(|error| format!("probe audio duration: {error}"))?,
        _ => return Err("not an audio media type".to_string()),
    };
    let millis = (seconds * 1_000.0).ceil() as u64;
    (millis > 0).then_some(millis).ok_or_else(|| "audio duration is zero".to_string())
}

fn wav_duration_secs(bytes: &[u8]) -> Result<f64, String> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".to_string());
    }
    let mut pos = 12usize;
    let mut block_align = None;
    let mut rate = None;
    let mut data_len = None;
    while pos + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let start = pos + 8;
        let end = start.checked_add(size).ok_or_else(|| "WAV chunk overflow".to_string())?;
        if end > bytes.len() { return Err("truncated WAV chunk".to_string()); }
        match &bytes[pos..pos + 4] {
            b"fmt " if size >= 16 => {
                rate = Some(u32::from_le_bytes(bytes[start + 4..start + 8].try_into().unwrap()));
                block_align = Some(u16::from_le_bytes(bytes[start + 12..start + 14].try_into().unwrap()));
            }
            b"data" => data_len = Some(size),
            _ => {}
        }
        pos = end + (size & 1);
    }
    let rate = rate.filter(|rate| *rate > 0).ok_or_else(|| "WAV sample rate is zero or missing".to_string())?;
    let align = block_align.filter(|align| *align > 0).ok_or_else(|| "WAV block alignment missing".to_string())?;
    let data = data_len.ok_or_else(|| "WAV data chunk missing".to_string())?;
    Ok(data as f64 / align as f64 / rate as f64)
}

fn video_duration_millis(bytes: &[u8]) -> Result<u64, String> {
    let index = makepad_mp4_index::parse_file(bytes).map_err(|error| format!("probe MP4 duration: {error}"))?;
    let millis = ((index.duration_100ns as f64) / 10_000.0).ceil() as u64;
    (millis > 0).then_some(millis).ok_or_else(|| "MP4 duration is zero".to_string())
}

fn media_type(content_type: &str, allowed: &[ResultMedia]) -> Result<ResultMedia, String> {
    let base = content_type.split(';').next().unwrap_or(content_type).trim().to_ascii_lowercase();
    let found = match base.as_str() {
        "audio/wav" | "audio/x-wav" => ResultMedia::Wav,
        "audio/ogg" | "application/ogg" => ResultMedia::Ogg,
        "audio/mpeg" => ResultMedia::Mp3,
        "video/mp4" => ResultMedia::Mp4,
        "model/gltf-binary" | "application/octet-stream" => ResultMedia::Glb,
        _ => return Err(format!("unsupported content type `{content_type}` for Publish")),
    };
    allowed.contains(&found)
        .then_some(found)
        .ok_or_else(|| format!("content type `{content_type}` does not match the value type"))
}

pub(crate) fn image_type_and_dims(bytes: &[u8], content_type: &str) -> Result<(ResultMedia, (u32, u32)), String> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") && bytes.len() >= 24 {
        return Ok((
            ResultMedia::Png,
            (
                u32::from_be_bytes(bytes[16..20].try_into().unwrap()),
                u32::from_be_bytes(bytes[20..24].try_into().unwrap()),
            ),
        ));
    }
    if bytes.starts_with(&[0xff, 0xd8]) {
        let mut at = 2;
        while at + 9 < bytes.len() {
            if bytes[at] != 0xff { at += 1; continue; }
            let marker = bytes[at + 1];
            if marker == 0xd8 || marker == 0xd9 { at += 2; continue; }
            let len = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
            if len < 2 || at + 2 + len > bytes.len() { break; }
            if matches!(marker, 0xc0 | 0xc1 | 0xc2 | 0xc3 | 0xc5 | 0xc6 | 0xc7 | 0xc9 | 0xca | 0xcb | 0xcd | 0xce | 0xcf) {
                return Ok((ResultMedia::Jpeg, (
                    u16::from_be_bytes([bytes[at + 7], bytes[at + 8]]) as u32,
                    u16::from_be_bytes([bytes[at + 5], bytes[at + 6]]) as u32,
                )));
            }
            at += 2 + len;
        }
    }
    Err(format!("Publish image is not a valid PNG or JPEG (`{content_type}`)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_is_alias_safe() {
        assert_eq!(slug("My Result!"), "my-result");
    }

    #[test]
    fn tags_follow_the_store_label_rule() {
        let tags = ["flow", "studio.Flux-Schnell", "Stage Flow!", "-x", "flow", "bpm-124", "", "  ", "_a"]
            .map(str::to_string);
        assert_eq!(labels(&tags), ["flow", "studio-flux-schnell", "stage-flow", "x", "bpm-124", "a"]);
        let long = labels(&["a".repeat(60) + "-b"]);
        assert_eq!(long, ["a".repeat(48)]);
        let many: Vec<String> = (0..30).map(|index| format!("t{index}")).collect();
        assert_eq!(labels(&many).len(), 24);
        for label in labels(&tags) {
            assert!(label.len() <= 48 && label.as_bytes()[0].is_ascii_alphanumeric());
            assert!(label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_'));
        }
    }

    #[test]
    fn png_dimensions_are_read_from_ihdr() {
        let pixels = vec![255; 300 * 260 * 4];
        let png = makepad_ai_hub::testpattern::encode_png_rgba(&pixels, 300, 260).unwrap();
        assert_eq!(image_type_and_dims(&png, "image/png").unwrap().1, (300, 260));
    }

    #[test]
    fn image_cards_are_resized_before_upload() {
        let pixels = vec![255; 1024 * 512 * 4];
        let png = makepad_ai_hub::testpattern::encode_png_rgba(&pixels, 1024, 512).unwrap();
        let thumbnail = image_thumbnail(&png, ResultMedia::Png).unwrap();
        assert_eq!((thumbnail.width, thumbnail.height), (512, 256));
        assert_eq!(image_type_and_dims(&thumbnail.png, "image/png").unwrap().1, (512, 256));
    }

    #[test]
    fn thumbnails_are_made_end_to_end_at_any_aspect() {
        use crate::engine::thumbnail::thumbnail_layout;
        for (width, height) in [(1024, 576), (576, 1024), (512, 512), (2048, 40), (40, 2048)] {
            let pixels = vec![200; width * height * 4];
            let png = makepad_ai_hub::testpattern::encode_png_rgba(&pixels, width, height).unwrap();
            let thumbnail = image_thumbnail(&png, ResultMedia::Png).unwrap();
            let layout = thumbnail_layout(width, height);
            assert_eq!((thumbnail.width as usize, thumbnail.height as usize), (layout.width, layout.height));
            assert_eq!(image_type_and_dims(&thumbnail.png, "image/png").unwrap().1, (thumbnail.width, thumbnail.height));
            let value = Value::media(PortType::Image, "image/png", png);
            let (_, _, thumbnail) = publish_parts(&value).unwrap();
            let thumbnail = thumbnail.unwrap();
            assert!((256..=4096).contains(&thumbnail.width) && (256..=4096).contains(&thumbnail.height));
        }
    }

    #[test]
    fn mesh_media_is_classified_and_glb_stats_are_measured() {
        let glb = makepad_gltf::write_glb_mesh(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], &[0, 1, 2]);
        let value = Value::media(PortType::Mesh, "model/gltf-binary", glb);
        let (kind, file, _) = publish_parts(&value).unwrap();
        assert_eq!(kind, ResultKind::Mesh);
        assert_eq!(file.media, ResultMedia::Glb);
        let stats = glb_stats(&value.bytes).unwrap();
        assert_eq!((stats.vertices, stats.triangles), (3, 1));

        let ply = Value::media(PortType::Mesh, "application/ply", b"ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n0 0 0\n1 0 0\n0 1 0\n3 0 1 2\n".to_vec());
        let (kind, file, _) = publish_parts(&ply).unwrap();
        assert_eq!(kind, ResultKind::World);
        assert_eq!(file.media, ResultMedia::Ply);
    }
}

#[cfg(test)]
mod product_tests {
    use super::*;
    use makepad_ai_hub::registry::Answer;

    #[test]
    fn every_hub_domain_maps_to_one_product_of_its_answer() {
        for domain in Domain::ALL {
            let made = product(domain);
            let media = match domain.io().answer {
                Answer::Image => ResultMedia::Png,
                Answer::Video => ResultMedia::Mp4,
                Answer::Audio => ResultMedia::Wav,
                Answer::Mesh => ResultMedia::Glb,
                Answer::Splat => ResultMedia::Ply,
                Answer::Text => ResultMedia::Text,
                Answer::Json => ResultMedia::Json,
            };
            assert!(made.role.allows(media), "{}: {:?} cannot carry {:?}", domain.as_str(), made.role, media);
            assert!(!made.category.is_empty());
        }
    }
}
