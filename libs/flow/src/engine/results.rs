//! Where a flow's results go and come from.
//!
//! A Publish node, the archive of generated media and an Output that
//! publishes hand a [`ResultFiles`] to a [`ResultStore`]; the host's
//! `/v1/assets/...` routes list and read what a store holds. The engine knows
//! only this trait. [`FolderStore`] is the plain-files implementation every
//! host gets by default; a product plugs in its own (a media library) by
//! passing an `Arc<dyn ResultStore>` where the engine takes one.
//!
//! Publication is asynchronous in the shape the Publish executor polls:
//! [`ResultStore::publish`] returns a channel of [`PublishEvent`]s and never
//! blocks the engine thread on the store.

use crate::FlowAsset;
use makepad_strict_json::Value as Json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};

/// What kind of thing a result is: the catalog class a library files it under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultKind {
    Texture,
    Mesh,
    Character,
    Audio,
    Video,
    World,
    Data,
    /// A flow source (`.splash`): a saved preset. Presets carry the tag
    /// [`PRESET_TAG`].
    Flow,
}

/// The tag a saved preset carries; [`ResultStore::list`] with
/// [`ResultKind::Flow`] returns the presets.
pub const PRESET_TAG: &str = "flow-preset";

impl ResultKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Texture => "texture",
            Self::Mesh => "mesh",
            Self::Character => "character",
            Self::Audio => "audio",
            Self::Video => "video",
            Self::World => "world",
            Self::Data => "data",
            Self::Flow => "flow",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        [Self::Texture, Self::Mesh, Self::Character, Self::Audio, Self::Video, Self::World, Self::Data, Self::Flow]
            .into_iter()
            .find(|kind| kind.name() == name)
    }
}

/// The part a file plays in its result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultRole {
    Texture,
    Depth,
    Video,
    Audio,
    RenderGlb,
    Splat,
    Source,
}

impl ResultRole {
    /// Media a role may carry.
    pub fn allows(self, media: ResultMedia) -> bool {
        use ResultMedia::*;
        match self {
            Self::RenderGlb => matches!(media, Glb),
            Self::Texture => matches!(media, Png | Jpeg | Bin),
            Self::Video => matches!(media, Mp4),
            Self::Audio => matches!(media, Wav | Ogg | Mp3),
            Self::Source => true,
            Self::Depth => matches!(media, Png),
            Self::Splat => matches!(media, Ply),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultMedia {
    Png,
    Jpeg,
    Glb,
    Ply,
    Wav,
    Ogg,
    Mp3,
    Mp4,
    Text,
    Json,
    Bin,
}

impl ResultMedia {
    pub fn content_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Glb => "model/gltf-binary",
            Self::Ply => "application/ply",
            Self::Wav => "audio/wav",
            Self::Ogg => "audio/ogg",
            Self::Mp3 => "audio/mpeg",
            Self::Mp4 => "video/mp4",
            Self::Text => "text/plain; charset=utf-8",
            Self::Json => "application/json",
            Self::Bin => "application/octet-stream",
        }
    }

    pub fn parse(content_type: &str) -> Option<Self> {
        [Self::Png, Self::Jpeg, Self::Glb, Self::Ply, Self::Wav, Self::Ogg, Self::Mp3, Self::Mp4, Self::Text, Self::Json, Self::Bin]
            .into_iter()
            .find(|media| media.content_type() == content_type)
    }
}

#[derive(Clone, Debug)]
pub struct ResultFile {
    pub role: ResultRole,
    pub media: ResultMedia,
    pub bytes: Vec<u8>,
    /// Playing time of audio and video, 0 otherwise.
    pub media_millis: u32,
    /// Pixel size of an image.
    pub dims: Option<(u32, u32)>,
}

/// A PNG preview picture.
#[derive(Clone, Debug)]
pub struct ResultThumb {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Measured mesh figures.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MeshStats {
    pub triangles: u32,
    pub vertices: u32,
    pub joints: u16,
    pub clips: u16,
}

/// One result to store, with everything the run knows about it.
#[derive(Clone, Debug)]
pub struct ResultFiles {
    pub kind: ResultKind,
    /// A library's grouping (`flows`).
    pub namespace: String,
    pub title: String,
    pub description: String,
    /// A stable name that points at the newest publication of this result
    /// (`namespace/slug`); also how a fixed-seed run finds its earlier result.
    pub alias: Option<String>,
    pub category: Option<String>,
    pub tags: Vec<String>,
    /// The files; today exactly one (the value itself).
    pub files: Vec<ResultFile>,
    /// A picture for lists. `None` for a video: the store makes the poster
    /// off the engine thread (see [`video_poster`]).
    pub thumbnail: Option<ResultThumb>,
    pub mesh_stats: Option<MeshStats>,
    pub creator: String,
    pub artist: String,
    /// The generator that made the value, its model and prompt.
    pub generator: String,
    pub model: String,
    pub prompt: String,
    /// The seed the nearest generator job reported.
    pub seed: Option<u64>,
    /// The reuse key: the recipe digest a host that runs a recipe names, else
    /// the nearest generator's parameters digest. [`ResultStore::find_by_digest`]
    /// finds the result again by it.
    pub digest: Option<[u8; 32]>,
    /// Version of the generator that wrote the record.
    pub generator_version: String,
    /// The provenance record's fields: flow, run, node, the generator jobs.
    pub provenance: Vec<(String, Json)>,
    /// The SHA-256 of the value's bytes (`Value::digest`).
    pub value_digest: [u8; 32],
    /// Digests of the media values upstream of this one: library inputs among
    /// them (see [`ResultStore::note_origin`]) become its parents.
    pub sources: Vec<[u8; 32]>,
}

/// A stored result. An id names it; an alias names its newest publication.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResultRef {
    pub id: String,
    pub revision: Option<String>,
    pub alias: Option<String>,
}

impl ResultRef {
    pub fn id(id: impl Into<String>) -> Self {
        Self { id: id.into(), revision: None, alias: None }
    }

    /// A reference by alias only (`id` empty).
    pub fn alias(alias: impl Into<String>) -> Self {
        Self { id: String::new(), revision: None, alias: Some(alias.into()) }
    }
}

#[derive(Clone, Debug)]
pub struct ResultInfo {
    pub title: String,
    pub kind: ResultKind,
    pub content_type: String,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ResultBytes {
    pub content_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct ResultQuery {
    pub text: String,
    /// `None` widens the query to every namespace.
    pub namespace: Option<String>,
    pub kind: Option<ResultKind>,
    pub limit: u32,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ResultPage {
    pub items: Vec<FlowAsset>,
    pub cursor: Option<String>,
}

pub enum PublishEvent {
    Progress(&'static str),
    Done(Result<ResultRef, String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreStatus {
    Ready,
    /// The store is coming up; try again.
    Starting(String),
    Failed(String),
}

/// The seam. Errors are plain text; a host maps a message that contains
/// `not found` to 404, `invalid` to 400 and `unavailable` to 503.
pub trait ResultStore: Send + Sync {
    fn status(&self) -> StoreStatus {
        StoreStatus::Ready
    }

    /// Whether generated media and terminal outputs are archived here as the
    /// run produces them.
    fn archive_outputs(&self) -> bool {
        false
    }

    fn publish(&self, files: ResultFiles) -> Result<Receiver<PublishEvent>, String>;

    /// The result whose reuse key is this digest (64 hex digits), within the
    /// store's own namespace.
    fn find_by_digest(&self, digest: &str) -> Option<ResultRef>;

    fn list(&self, query: ResultQuery) -> Result<ResultPage, String>;

    /// The result's content.
    fn read(&self, result: &ResultRef) -> Result<ResultBytes, String>;

    /// The bounded representation a library card shows: the picture, or a
    /// short text.
    fn preview(&self, result: &ResultRef) -> Result<ResultBytes, String>;

    fn thumbnail(&self, result: &ResultRef) -> Result<Option<ResultBytes>, String>;

    fn info(&self, result: &ResultRef) -> Option<ResultInfo>;

    /// Tell the store that the value with this digest is (the file of) this
    /// result, so a publication made from it records it as a parent. Never
    /// blocks.
    fn note_origin(&self, input_digest: [u8; 32], result: &ResultRef);
}

/// The first video frame as a 256-pixel poster, for a store that makes
/// posters off the engine thread.
#[cfg(feature = "media")]
pub fn video_poster(bytes: &[u8]) -> Result<ResultThumb, String> {
    super::thumbnail::video_thumbnail(bytes)
}

// ---- FolderStore ----------------------------------------------------------

const MAX_LIST: u32 = 100;
const MAX_TEXT_PREVIEW_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug)]
struct Entry {
    id: String,
    namespace: String,
    title: String,
    description: String,
    kind: ResultKind,
    content_type: String,
    tags: Vec<String>,
    alias: Option<String>,
    created_ms: u64,
    digest: Option<String>,
    value_digest: String,
    parents: Vec<String>,
    provenance: Json,
    has_thumb: bool,
}

impl Entry {
    fn to_json(&self) -> Json {
        let text = |value: &str| Json::Str(value.to_string());
        Json::Obj(vec![
            ("id".into(), text(&self.id)),
            ("namespace".into(), text(&self.namespace)),
            ("title".into(), text(&self.title)),
            ("description".into(), text(&self.description)),
            ("kind".into(), text(self.kind.name())),
            ("content_type".into(), text(&self.content_type)),
            ("tags".into(), Json::Arr(self.tags.iter().map(|tag| text(tag)).collect())),
            ("alias".into(), self.alias.as_deref().map(text).unwrap_or(Json::Null)),
            ("created_ms".into(), Json::Int(self.created_ms.min(i64::MAX as u64) as i64)),
            ("digest".into(), self.digest.as_deref().map(text).unwrap_or(Json::Null)),
            ("value_digest".into(), text(&self.value_digest)),
            ("parents".into(), Json::Arr(self.parents.iter().map(|parent| text(parent)).collect())),
            ("provenance".into(), self.provenance.clone()),
            ("has_thumb".into(), Json::Bool(self.has_thumb)),
        ])
    }

    fn from_json(json: &Json) -> Option<Self> {
        let text = |key: &str| json.get(key).and_then(Json::as_str).map(str::to_string);
        let list = |key: &str| {
            json.get(key)
                .and_then(Json::as_arr)
                .map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect())
                .unwrap_or_default()
        };
        Some(Self {
            id: text("id")?,
            namespace: text("namespace")?,
            title: text("title")?,
            description: text("description").unwrap_or_default(),
            kind: ResultKind::parse(&text("kind")?)?,
            content_type: text("content_type")?,
            tags: list("tags"),
            alias: text("alias"),
            created_ms: json.get("created_ms").and_then(Json::as_u64).unwrap_or(0),
            digest: text("digest"),
            value_digest: text("value_digest").unwrap_or_default(),
            parents: list("parents"),
            provenance: json.get("provenance").cloned().unwrap_or(Json::Null),
            has_thumb: json.get("has_thumb").and_then(Json::as_bool).unwrap_or(false),
        })
    }

    fn asset(&self) -> FlowAsset {
        FlowAsset {
            id: self.id.clone(),
            alias: self.alias.clone(),
            namespace: self.namespace.clone(),
            title: self.title.clone(),
            kind: self.kind.name().to_string(),
            tags: self.tags.clone(),
            created_ms: self.created_ms,
        }
    }
}

#[derive(Default)]
struct Index {
    entries: Vec<Entry>,
    /// Published values by the digest of their bytes: the parents a later
    /// publication names.
    origins: HashMap<[u8; 32], String>,
}

/// Results as plain files in a folder: `<id>.bin` with a small `<id>.json`
/// sidecar (title, tags, provenance, digests) and an optional
/// `<id>.thumb.png`. The sidecars are read once at [`FolderStore::open`] and
/// kept as the index.
pub struct FolderStore {
    dir: PathBuf,
    namespace: String,
    archive: bool,
    index: Arc<Mutex<Index>>,
}

impl FolderStore {
    /// Open (creating it if needed) the folder; `namespace` scopes
    /// [`ResultStore::find_by_digest`].
    pub fn open(dir: impl Into<PathBuf>, namespace: impl Into<String>) -> Result<Self, String> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir).map_err(|error| format!("create results folder: {error}"))?;
        let mut index = Index::default();
        let read = std::fs::read_dir(&dir).map_err(|error| format!("read results folder: {error}"))?;
        for item in read.flatten() {
            let path = item.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else { continue };
            let Ok(json) = makepad_strict_json::parse(&bytes) else { continue };
            if let Some(entry) = Entry::from_json(&json) {
                if let Some(digest) = parse_hex32(&entry.value_digest) {
                    index.origins.insert(digest, entry.id.clone());
                }
                index.entries.push(entry);
            }
        }
        Ok(Self { dir, namespace: namespace.into(), archive: false, index: Arc::new(Mutex::new(index)) })
    }

    /// Archive generated media and terminal outputs here as runs produce them.
    pub fn with_archive_outputs(mut self, archive: bool) -> Self {
        self.archive = archive;
        self
    }

    fn resolve(&self, result: &ResultRef) -> Result<Entry, String> {
        let index = self.index.lock().unwrap();
        let found = if !result.id.is_empty() {
            index.entries.iter().find(|entry| entry.id == result.id)
        } else if let Some(alias) = &result.alias {
            index.entries.iter().filter(|entry| entry.alias.as_deref() == Some(alias)).max_by_key(|entry| entry.created_ms)
        } else {
            return Err("invalid result reference".to_string());
        };
        found.cloned().ok_or_else(|| "result not found".to_string())
    }

    fn file(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.bin"))
    }
}

impl ResultStore for FolderStore {
    fn archive_outputs(&self) -> bool {
        self.archive
    }

    fn publish(&self, files: ResultFiles) -> Result<Receiver<PublishEvent>, String> {
        if files.files.len() != 1 {
            return Err("a result takes exactly one file".to_string());
        }
        let (tx, rx) = mpsc::channel();
        let dir = self.dir.clone();
        let index = self.index.clone();
        std::thread::Builder::new()
            .name("flow-results".to_string())
            .spawn(move || {
                let _ = tx.send(PublishEvent::Progress("uploading"));
                let _ = tx.send(PublishEvent::Done(write_result(&dir, &index, files)));
            })
            .map_err(|error| format!("start result write: {error}"))?;
        Ok(rx)
    }

    fn find_by_digest(&self, digest: &str) -> Option<ResultRef> {
        let index = self.index.lock().unwrap();
        index
            .entries
            .iter()
            .filter(|entry| entry.namespace == self.namespace && entry.digest.as_deref() == Some(digest))
            .max_by_key(|entry| entry.created_ms)
            .map(|entry| ResultRef { id: entry.id.clone(), revision: None, alias: entry.alias.clone() })
    }

    fn list(&self, query: ResultQuery) -> Result<ResultPage, String> {
        let limit = query.limit.clamp(1, MAX_LIST) as usize;
        let start = match &query.cursor {
            None => 0,
            Some(cursor) => cursor.strip_prefix("at-").and_then(|n| n.parse::<usize>().ok()).ok_or("invalid result cursor")?,
        };
        let text = query.text.trim().to_ascii_lowercase();
        let index = self.index.lock().unwrap();
        let mut rows: Vec<&Entry> = index
            .entries
            .iter()
            .filter(|entry| query.namespace.as_deref().map_or(true, |ns| entry.namespace == ns))
            .filter(|entry| query.kind.map_or(true, |kind| entry.kind == kind))
            .filter(|entry| {
                text.is_empty()
                    || entry.title.to_ascii_lowercase().contains(&text)
                    || entry.tags.iter().any(|tag| tag.contains(&text))
            })
            .collect();
        rows.sort_by(|left, right| right.created_ms.cmp(&left.created_ms).then_with(|| left.id.cmp(&right.id)));
        let items: Vec<FlowAsset> = rows.iter().skip(start).take(limit).map(|entry| entry.asset()).collect();
        let cursor = (start + limit < rows.len()).then(|| format!("at-{}", start + limit));
        Ok(ResultPage { items, cursor })
    }

    fn read(&self, result: &ResultRef) -> Result<ResultBytes, String> {
        let entry = self.resolve(result)?;
        let bytes = std::fs::read(self.file(&entry.id)).map_err(|_| "result content not found".to_string())?;
        Ok(ResultBytes { content_type: entry.content_type, bytes })
    }

    fn preview(&self, result: &ResultRef) -> Result<ResultBytes, String> {
        let entry = self.resolve(result)?;
        if entry.has_thumb {
            if let Ok(bytes) = std::fs::read(self.dir.join(format!("{}.thumb.png", entry.id))) {
                return Ok(ResultBytes { content_type: "image/png".to_string(), bytes });
            }
        }
        let path = self.file(&entry.id);
        let length = std::fs::metadata(&path).map_err(|_| "result content not found".to_string())?.len();
        if matches!(entry.kind, ResultKind::Data | ResultKind::Flow) && length as usize <= MAX_TEXT_PREVIEW_BYTES {
            let bytes = std::fs::read(&path).map_err(|_| "result content not found".to_string())?;
            return Ok(ResultBytes { content_type: entry.content_type, bytes });
        }
        let label = match entry.kind {
            ResultKind::Audio => "Audio",
            ResultKind::Video => "Video",
            ResultKind::Texture => "Image",
            ResultKind::Mesh | ResultKind::Character | ResultKind::World => "3D model",
            ResultKind::Data | ResultKind::Flow => "Preview unavailable",
        };
        Ok(ResultBytes {
            content_type: "text/plain; charset=utf-8".to_string(),
            bytes: format!("{label} · {length} bytes").into_bytes(),
        })
    }

    fn thumbnail(&self, result: &ResultRef) -> Result<Option<ResultBytes>, String> {
        let entry = self.resolve(result)?;
        if !entry.has_thumb {
            return Ok(None);
        }
        let bytes = std::fs::read(self.dir.join(format!("{}.thumb.png", entry.id)))
            .map_err(|_| "result thumbnail not found".to_string())?;
        Ok(Some(ResultBytes { content_type: "image/png".to_string(), bytes }))
    }

    fn info(&self, result: &ResultRef) -> Option<ResultInfo> {
        let entry = self.resolve(result).ok()?;
        Some(ResultInfo { title: entry.title, kind: entry.kind, content_type: entry.content_type, tags: entry.tags })
    }

    fn note_origin(&self, input_digest: [u8; 32], result: &ResultRef) {
        if let Ok(entry) = self.resolve(result) {
            self.index.lock().unwrap().origins.insert(input_digest, entry.id);
        }
    }
}

fn write_result(dir: &std::path::Path, index: &Mutex<Index>, files: ResultFiles) -> Result<ResultRef, String> {
    let file = &files.files[0];
    let id = crate::digest::to_hex(&files.value_digest)[..32].to_string();
    let parents: Vec<String> = {
        let index = index.lock().unwrap();
        let mut parents = Vec::new();
        for source in files.sources.iter().filter(|source| **source != files.value_digest) {
            if let Some(parent) = index.origins.get(source) {
                if *parent != id && !parents.contains(parent) {
                    parents.push(parent.clone());
                }
            }
        }
        parents
    };
    let mut provenance = files.provenance.clone();
    if !parents.is_empty() {
        provenance.push(("parents".to_string(), Json::Arr(parents.iter().cloned().map(Json::Str).collect())));
    }
    let created_ms = now_ms();
    let entry = Entry {
        id: id.clone(),
        namespace: files.namespace.clone(),
        title: files.title.clone(),
        description: files.description.clone(),
        kind: files.kind,
        content_type: file.media.content_type().to_string(),
        tags: files.tags.clone(),
        alias: files.alias.clone(),
        created_ms,
        digest: files.digest.map(|digest| crate::digest::to_hex(&digest)),
        value_digest: crate::digest::to_hex(&files.value_digest),
        parents,
        provenance: Json::Obj(provenance),
        has_thumb: false,
    };
    let thumb = match (&files.thumbnail, file.media) {
        (Some(thumb), _) => Some(thumb.png.clone()),
        #[cfg(feature = "media")]
        (None, ResultMedia::Mp4) => video_poster(&file.bytes).ok().map(|thumb| thumb.png),
        _ => None,
    };
    let mut entry = entry;
    entry.has_thumb = thumb.is_some();
    // Content first, sidecar last: a sidecar is what makes a result exist.
    write_atomic(&dir.join(format!("{id}.bin")), &file.bytes)?;
    if let Some(png) = &thumb {
        write_atomic(&dir.join(format!("{id}.thumb.png")), png)?;
    }
    write_atomic(&dir.join(format!("{id}.json")), entry.to_json().to_json().as_bytes())?;
    let reference = ResultRef { id: id.clone(), revision: None, alias: entry.alias.clone() };
    let mut index = index.lock().unwrap();
    index.origins.insert(files.value_digest, id.clone());
    if let Some(slot) = index.entries.iter_mut().find(|existing| existing.id == id) {
        *slot = entry;
    } else {
        index.entries.push(entry);
    }
    if let Some(alias) = &reference.alias {
        // The alias points at the newest publication only.
        for other in index.entries.iter_mut().filter(|other| other.id != id && other.alias.as_deref() == Some(alias)) {
            other.alias = None;
        }
    }
    Ok(reference)
}

fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|error| format!("write result: {error}"))?;
    std::fs::rename(&tmp, path).map_err(|error| format!("write result: {error}"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn parse_hex32(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}
