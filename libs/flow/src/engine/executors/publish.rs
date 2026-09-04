use super::{param, string_param, Executor, Poll};
use crate::{AssetsResponse, FlowAsset, Literal, Node, PortType, Value};
use makepad_asset_client::{
    content_client_caps, ApiEndpoints, AssetClient, CatalogHit, CatalogQuery, ClientConfig,
    ClientError, DiscoveryListener, PublishFile, PublishRequest, PublishThumbnail, Published,
};
use makepad_asset_data::{AssetAlias, AssetId, AssetKind, FileRole, MediaType, ThumbnailMedia};
use makepad_strict_json::Value as Json;
use makepad_zune_jpeg::JpegDecoder;
use makepad_zune_png::PngDecoder;
use makepad_zune_png::makepad_zune_core::{
    bytestream::ZCursor,
    colorspace::ColorSpace,
    options::DecoderOptions,
    result::DecodingResult,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const DISCOVERY_TTL_MS: u64 = 10_000;

#[derive(Clone, Debug)]
pub struct AssetStoreConfig {
    /// Archive generated media and terminal outputs in hosts with a configured asset store.
    pub archive_outputs: bool,
    pub cache_dir: PathBuf,
    pub token: Option<String>,
    pub endpoints: Option<ApiEndpoints>,
    pub server_id: Option<[u8; 16]>,
    pub discovery_port: u16,
    pub discovery_wait_ms: u64,
}

impl AssetStoreConfig {
    pub fn new(cache_dir: PathBuf) -> Self {
        Self {
            archive_outputs: false,
            cache_dir,
            token: None,
            endpoints: None,
            server_id: None,
            discovery_port: makepad_asset_client::wire::DEFAULT_DISCOVERY_PORT,
            discovery_wait_ms: 1_000,
        }
    }
}

#[derive(Clone, Debug)]
pub struct AssetListQuery {
    pub text: String,
    /// `None` widens the query to every namespace. `Some("flows")` also
    /// includes assets carrying the conventional `flow` tag.
    pub namespace: Option<String>,
    pub limit: u32,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ThumbnailBytes {
    pub content_type: String,
    pub bytes: Vec<u8>,
}

pub enum PublishWorkerEvent {
    Progress(&'static str),
    Done(Result<Published, String>),
}

enum AssetCommand {
    Publish(PublishRequest, Sender<PublishWorkerEvent>),
    List(AssetListQuery, Sender<Result<AssetsResponse, String>>),
    Thumbnail(AssetAlias, Sender<Result<ThumbnailBytes, String>>),
    Read(AssetId, bool, Sender<Result<ThumbnailBytes, String>>),
    ReadPreview(AssetId, Sender<Result<ThumbnailBytes, String>>),
    Stop,
}

#[derive(Clone)]
pub struct AssetWorkerHandle {
    tx: Sender<AssetCommand>,
    pub(crate) archive_outputs: bool,
}

impl AssetWorkerHandle {
    pub fn read_asset(&self, id: AssetId, thumbnail: bool) -> Result<ThumbnailBytes, String> {
        let (tx, rx) = mpsc::channel();
        self.tx.send(AssetCommand::Read(id, thumbnail, tx))
            .map_err(|_| "asset worker is not running".to_string())?;
        rx.recv_timeout(Duration::from_secs(60))
            .map_err(|_| "asset content request timed out".to_string())?
    }

    /// Read the small, safe representation used by the asset library cards.
    /// Full content remains available through `read_asset(id, false)` for the
    /// viewer; callers must opt into this bound explicitly.
    pub fn read_asset_preview(&self, id: AssetId) -> Result<ThumbnailBytes, String> {
        let (tx, rx) = mpsc::channel();
        self.tx.send(AssetCommand::ReadPreview(id, tx))
            .map_err(|_| "asset worker is not running".to_string())?;
        rx.recv_timeout(Duration::from_secs(60))
            .map_err(|_| "asset preview request timed out".to_string())?
    }

    pub fn publish(&self, request: PublishRequest) -> Result<Receiver<PublishWorkerEvent>, String> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(AssetCommand::Publish(request, tx))
            .map_err(|_| "asset worker is not running".to_string())?;
        Ok(rx)
    }

    pub fn list(&self, query: AssetListQuery) -> Result<Vec<FlowAsset>, String> {
        self.list_page(query).map(|page| page.assets)
    }

    pub fn list_page(&self, query: AssetListQuery) -> Result<AssetsResponse, String> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(AssetCommand::List(query, tx))
            .map_err(|_| "asset worker is not running".to_string())?;
        rx.recv_timeout(Duration::from_secs(15))
            .map_err(|_| "asset library request timed out".to_string())?
    }

    pub fn thumbnail(&self, alias: AssetAlias) -> Result<ThumbnailBytes, String> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(AssetCommand::Thumbnail(alias, tx))
            .map_err(|_| "asset worker is not running".to_string())?;
        rx.recv_timeout(Duration::from_secs(15))
            .map_err(|_| "asset thumbnail request timed out".to_string())?
    }
}

pub struct AssetWorker {
    handle: AssetWorkerHandle,
    join: Option<JoinHandle<()>>,
}

impl AssetWorker {
    pub fn start(config: AssetStoreConfig) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel();
        let handle = AssetWorkerHandle { tx, archive_outputs: config.archive_outputs };
        let join = std::thread::Builder::new()
            .name("flow-asset-store".to_string())
            .spawn(move || worker_loop(config, rx))
            .map_err(|error| format!("start asset worker: {error}"))?;
        Ok(Self { handle, join: Some(join) })
    }

    pub fn handle(&self) -> AssetWorkerHandle {
        self.handle.clone()
    }

    pub fn stop(&mut self) {
        let _ = self.handle.tx.send(AssetCommand::Stop);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for AssetWorker {
    fn drop(&mut self) {
        self.stop();
    }
}

fn worker_loop(config: AssetStoreConfig, rx: Receiver<AssetCommand>) {
    let mut client = None;
    let mut pages: HashMap<String, (Instant, AssetPageCursor)> = HashMap::new();
    while let Ok(command) = rx.recv() {
        match command {
            AssetCommand::Stop => break,
            AssetCommand::Read(id, thumbnail, reply) => {
                let result = connected_client(&config, &mut client)
                    .and_then(|client| read_asset(client, id, thumbnail));
                let _ = reply.send(result);
            }
            AssetCommand::ReadPreview(id, reply) => {
                let result = connected_client(&config, &mut client)
                    .and_then(|client| read_asset_preview(client, id));
                let _ = reply.send(result);
            }
            AssetCommand::Publish(mut request, reply) => {
                let result = (|| {
                    let client = connected_client(&config, &mut client)?;
                    if request.artifact.media == MediaType::Mp4 {
                        // Decode on this long-lived asset worker, never on
                        // the engine/UI thread. Keep the original MP4 intact.
                        match video_thumbnail(&request.artifact.bytes) {
                            Ok(thumbnail) => request.thumbnail = thumbnail,
                            Err(error) => eprintln!("[flow] video poster unavailable: {error}"),
                        }
                    }
                    let _ = reply.send(PublishWorkerEvent::Progress("uploading"));
                    if let Some(alias) = request.alias.as_ref() {
                        match client.resolve_alias(alias) {
                            Ok(found) => request.asset_id = Some(found.asset_id),
                            Err(ClientError::NotFound { .. }) => {}
                            Err(error) => return Err(format!("resolve publish alias: {error}")),
                        }
                    }
                    client
                        .publish_artifact(&request)
                        .map_err(|error| format!("publish asset: {error}"))
                })();
                let _ = reply.send(PublishWorkerEvent::Done(result));
            }
            AssetCommand::List(query, reply) => {
                let result = connected_client(&config, &mut client)
                    .and_then(|client| list_assets(client, &query, &mut pages));
                let _ = reply.send(result);
            }
            AssetCommand::Thumbnail(alias, reply) => {
                let result = connected_client(&config, &mut client).and_then(|client| {
                    let bytes = client
                        .thumbnail_alias_bytes(&alias)
                        .map_err(|error| format!("fetch asset thumbnail: {error}"))?;
                    let content_type = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                        "image/png"
                    } else if bytes.starts_with(&[0xff, 0xd8]) {
                        "image/jpeg"
                    } else {
                        return Err("asset thumbnail has an unsupported image type".to_string());
                    };
                    Ok(ThumbnailBytes { content_type: content_type.to_string(), bytes })
                });
                let _ = reply.send(result);
            }
        }
    }
}

fn connected_client<'a>(
    config: &AssetStoreConfig,
    slot: &'a mut Option<AssetClient>,
) -> Result<&'a mut AssetClient, String> {
    if slot.is_none() {
        let mut client_config = ClientConfig::new(config.cache_dir.clone());
        client_config.token = config.token.clone();
        let connected = if let Some(endpoints) = config.endpoints {
            AssetClient::connect(client_config, endpoints, config.server_id)
                .map_err(|error| format!("connect to asset server: {error}"))?
        } else {
            if config.discovery_wait_ms == 0 {
                return Err("no asset server discovered on this LAN".to_string());
            }
            let listener = DiscoveryListener::start(
                config.discovery_port,
                DISCOVERY_TTL_MS,
                makepad_asset_client::util::now_ms,
            )
            .map_err(|error| format!("start asset server discovery: {error}"))?;
            let deadline = Instant::now() + Duration::from_millis(config.discovery_wait_ms);
            let discovered = loop {
                if let Some(server) =
                    listener.pick(content_client_caps(), makepad_asset_client::util::now_ms())
                {
                    break server;
                }
                if Instant::now() >= deadline {
                    return Err("no asset server discovered on this LAN".to_string());
                }
                std::thread::sleep(Duration::from_millis(25));
            };
            AssetClient::connect_discovered(client_config, &discovered)
                .map_err(|error| format!("connect to discovered asset server: {error}"))?
        };
        *slot = Some(connected);
    }
    Ok(slot.as_mut().unwrap())
}

fn read_asset(client: &mut AssetClient, id: AssetId, thumbnail: bool) -> Result<ThumbnailBytes, String> {
    let detail = client.asset_detail(&id).map_err(|e| format!("read asset: {e}"))?;
    let revision = detail.latest_published().ok_or("asset has no published revision")?.revision;
    let manifest = client.fetch_asset_manifest(&revision).map_err(|e| format!("read asset manifest: {e}"))?;
    if thumbnail {
        return read_manifest_thumbnail(client, &manifest);
    }
    let (blob, len, content_type) = {
        let file = manifest.files.iter().min_by_key(|file| {
            let priority = match file.role {
                FileRole::RenderGlb | FileRole::Splat | FileRole::Video | FileRole::Audio | FileRole::Texture => 0,
                FileRole::Source => 1,
                _ => 2,
            };
            (priority, file.lod)
        }).ok_or("asset content not found")?;
        let content_type = match file.media {
            MediaType::Png => "image/png", MediaType::Jpeg => "image/jpeg",
            MediaType::Glb => "model/gltf-binary", MediaType::Ply => "application/ply",
            MediaType::Wav => "audio/wav", MediaType::Ogg => "audio/ogg", MediaType::Mp3 => "audio/mpeg",
            MediaType::Mp4 => "video/mp4", MediaType::Text => "text/plain; charset=utf-8",
            MediaType::Json => "application/json", MediaType::Bin => "application/octet-stream",
        };
        (file.blob, file.byte_len, content_type)
    };
    let bytes = client.fetch_blob_bytes(&blob, Some(len)).map_err(|e| format!("fetch asset content: {e}"))?;
    Ok(ThumbnailBytes { content_type: content_type.to_string(), bytes })
}

const MAX_LEGACY_VIDEO_POSTER_BYTES: u64 = 64 * 1024 * 1024;

fn read_manifest_thumbnail(
    client: &mut AssetClient,
    manifest: &makepad_asset_data::AssetManifest,
) -> Result<ThumbnailBytes, String> {
    let thumb = manifest.thumbnail.as_ref().ok_or("asset thumbnail not found")?;
    let bytes = client
        .fetch_blob_bytes(&thumb.blob, Some(thumb.byte_len))
        .map_err(|e| format!("fetch asset thumbnail: {e}"))?;

    // Older Flow revisions stored a solid tile for videos. Only fetch the
    // original when its declared size is bounded; card previews must never
    // turn into an unbounded video download.
    if bytes == placeholder_thumbnail()?.bytes {
        if let Some(video) = manifest
            .files
            .iter()
            .find(|file| file.media == MediaType::Mp4 && file.byte_len <= MAX_LEGACY_VIDEO_POSTER_BYTES)
        {
            if let Ok(original) = client.fetch_blob_bytes(&video.blob, Some(video.byte_len)) {
                if let Ok(poster) = video_thumbnail(&original) {
                    return Ok(ThumbnailBytes { content_type: "image/png".into(), bytes: poster.bytes });
                }
            }
        }
    }

    let media = match thumb.media {
        ThumbnailMedia::Png => MediaType::Png,
        ThumbnailMedia::Jpeg => MediaType::Jpeg,
    };
    match image_thumbnail(&bytes, media) {
        Ok(resized) => Ok(ThumbnailBytes { content_type: "image/png".into(), bytes: resized.bytes }),
        // Oversized/unsupported legacy images must not bypass the worker's
        // limit and become full-resolution textures in the scrolling list.
        Err(_) => Ok(ThumbnailBytes {
            content_type: "image/png".into(),
            bytes: placeholder_thumbnail()?.bytes,
        }),
    }
}

const MAX_TEXT_PREVIEW_BYTES: u64 = 16 * 1024;

fn read_asset_preview(client: &mut AssetClient, id: AssetId) -> Result<ThumbnailBytes, String> {
    let detail = client.asset_detail(&id).map_err(|e| format!("read asset: {e}"))?;
    let revision = detail.latest_published().ok_or("asset has no published revision")?.revision;
    let manifest = client.fetch_asset_manifest(&revision).map_err(|e| format!("read asset manifest: {e}"))?;

    // Image/video/mesh/audio cards always use the manifest thumbnail. This
    // Legacy video tiles may use the bounded poster backfill above.
    if manifest.kind != AssetKind::Data && manifest.thumbnail.is_some() {
        return read_manifest_thumbnail(client, &manifest);
    }

    let file = manifest.files.iter().min_by_key(|file| {
        let priority = match file.role {
            FileRole::RenderGlb | FileRole::Splat | FileRole::Video | FileRole::Audio | FileRole::Texture => 0,
            FileRole::Source => 1,
            _ => 2,
        };
        (priority, file.lod)
    }).ok_or("asset content not found")?;
    let content_type = media_content_type(file.media);
    match file.media {
        MediaType::Text | MediaType::Json if file.byte_len <= MAX_TEXT_PREVIEW_BYTES => {
            let bytes = client.fetch_blob_bytes(&file.blob, Some(file.byte_len))
                .map_err(|e| format!("fetch asset preview: {e}"))?;
            Ok(ThumbnailBytes { content_type: content_type.into(), bytes })
        }
        // The blob API is exact-length, so refusing oversized text here is
        // safer than accidentally reading an unbounded source into the UI.
        MediaType::Text | MediaType::Json => Ok(preview_fallback(content_type, file.byte_len)),
        _ => Ok(preview_fallback(content_type, file.byte_len)),
    }
}

fn media_content_type(media: MediaType) -> &'static str {
    match media {
        MediaType::Png => "image/png", MediaType::Jpeg => "image/jpeg",
        MediaType::Glb => "model/gltf-binary", MediaType::Ply => "application/ply",
        MediaType::Wav => "audio/wav", MediaType::Ogg => "audio/ogg", MediaType::Mp3 => "audio/mpeg",
        MediaType::Mp4 => "video/mp4", MediaType::Text => "text/plain; charset=utf-8",
        MediaType::Json => "application/json", MediaType::Bin => "application/octet-stream",
    }
}

fn preview_fallback(content_type: &str, byte_len: u64) -> ThumbnailBytes {
    let label = match content_type {
        "application/octet-stream" => "Binary data",
        "model/gltf-binary" | "application/ply" => "3D model",
        value if value.starts_with("audio/") => "Audio",
        value if value.starts_with("video/") => "Video",
        value if value.starts_with("image/") => "Image",
        _ => "Preview unavailable",
    };
    ThumbnailBytes {
        content_type: "text/plain; charset=utf-8".into(),
        bytes: format!("{label} · {byte_len} bytes").into_bytes(),
    }
}

#[derive(Clone)]
struct AssetPageCursor {
    query: String,
    namespace: Option<String>,
    limit: u32,
    primary: Option<makepad_asset_client::PageCursor>,
    tagged: Option<makepad_asset_client::PageCursor>,
}

fn list_assets(client: &AssetClient, query: &AssetListQuery, pages: &mut HashMap<String, (Instant, AssetPageCursor)>) -> Result<AssetsResponse, String> {
    let limit = query.limit.clamp(1, 100);
    pages.retain(|_, (created, _)| created.elapsed() < Duration::from_secs(600));
    let saved = query.cursor.as_ref().map(|cursor| pages.get(cursor).map(|(_, saved)| saved.clone())
        .ok_or_else(|| "invalid asset cursor; refresh the library".to_string())).transpose()?;
    if saved.as_ref().is_some_and(|saved| saved.query != query.text || saved.namespace != query.namespace || saved.limit != limit) {
        return Err("invalid asset cursor for this query".to_string());
    }
    let make_query = |namespace: Option<String>, tag: Option<String>| {
        // Each source contributes one bounded page; merged pages can contain
        // up to twice the requested limit when flow-tagged imports overlap.
        let mut value = if query.text.trim().is_empty() {
            let mut value = CatalogQuery::browse(limit);
            value.newest = true;
            value
        } else {
            CatalogQuery::text(query.text.trim(), limit)
        };
        value.namespace = namespace;
        value.tag = tag;
        value
    };
    let mut hits = Vec::new();
    let mut seen = HashSet::new();
    let mut next = AssetPageCursor { query: query.text.clone(), namespace: query.namespace.clone(), limit, primary: None, tagged: None };
    for tagged in [false, true] {
        if tagged && query.namespace.as_deref() != Some("flows") { continue; }
        let cursor = saved.as_ref().and_then(|saved| if tagged { saved.tagged.as_ref() } else { saved.primary.as_ref() });
        if saved.is_some() && cursor.is_none() { continue; }
        let catalog_query = if tagged { make_query(None, Some("flow".into())) } else { make_query(query.namespace.clone(), None) };
        let page = client.catalog_search(&catalog_query, cursor)
            .map_err(|error| format!("browse asset catalog: {error}"))?;
        if tagged { next.tagged = page.next; } else { next.primary = page.next; }
        for hit in page.hits {
            if seen.insert(hit.asset_id) { hits.push(hit); }
        }
    }
    let cursor = if next.primary.is_some() || next.tagged.is_some() {
        // Pagination state remains on the asset worker, preserving the
        // upstream client's typed server binding without expanding URLs.
        if pages.len() >= 256 {
            if let Some(oldest) = pages.iter().min_by_key(|(_, (at, _))| *at).map(|(id, _)| id.clone()) {
                pages.remove(&oldest);
            }
        }
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let id = format!("page-{nonce:x}");
        pages.insert(id.clone(), (Instant::now(), next));
        Some(id)
    } else { None };
    if hits.is_empty() { return Ok(AssetsResponse { assets: Vec::new(), cursor }); }
    let ids = hits
        .iter()
        .map(|hit| format!("'{}'", hex(hit.asset_id.as_bytes())))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT lower(hex(a.asset_id)), a.created_ms, COALESCE(group_concat(CASE WHEN l.kind='tag' THEN l.label END, ','), '') FROM assets a LEFT JOIN search_labels l ON l.asset_id=a.asset_id WHERE lower(hex(a.asset_id)) IN ({ids}) GROUP BY a.asset_id, a.created_ms"
    );
    let metadata = client
        .assets_query(&sql)
        .map_err(|error| format!("read asset creation metadata: {error}"))?;
    let mut by_id = HashMap::new();
    for row in metadata.rows {
        if row.len() == 3 {
            let tags = row[2]
                .split(',')
                .filter(|tag| !tag.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>();
            by_id.insert(row[0].clone(), (row[1].parse::<u64>().unwrap_or(0), tags));
        }
    }
    let mut rows = hits
        .into_iter()
        .map(|hit| flow_asset(hit, &by_id))
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        right
            .created_ms
            .cmp(&left.created_ms)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(AssetsResponse { assets: rows, cursor })
}

fn flow_asset(hit: CatalogHit, metadata: &HashMap<String, (u64, Vec<String>)>) -> FlowAsset {
    let key = hex(hit.asset_id.as_bytes());
    let (created_ms, tags) = metadata.get(&key).cloned().unwrap_or((hit.updated_ms, Vec::new()));
    FlowAsset {
        id: hit.asset_id.to_string(),
        alias: hit.alias.map(|alias| alias.to_string()),
        namespace: hit.namespace,
        title: hit.title,
        kind: hit
            .kind
            .map(makepad_asset_client::dto::kind_name)
            .unwrap_or("unknown")
            .to_string(),
        tags,
        created_ms,
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 15) as usize] as char);
    }
    out
}

pub struct PublishExecutor {
    worker: Option<AssetWorkerHandle>,
    flow: String,
    instance: String,
    description_default: String,
    queue: VecDeque<Poll>,
    reply: Option<Receiver<PublishWorkerEvent>>,
    cancelled: bool,
    published_meta: Option<(String, String, String)>,
}

impl PublishExecutor {
    pub fn new(
        worker: Option<AssetWorkerHandle>,
        flow: String,
        instance: String,
        description_default: String,
    ) -> Self {
        Self {
            worker,
            flow,
            instance,
            description_default,
            queue: VecDeque::new(),
            reply: None,
            cancelled: false,
            published_meta: None,
        }
    }
}

impl Executor for PublishExecutor {
    fn start(&mut self, node: &Node, inputs: &[(String, Value)]) -> Result<(), String> {
        self.cancelled = false;
        let value = inputs
            .iter()
            .find_map(|(name, value)| (name == "value").then_some(value))
            .ok_or_else(|| format!("Publish node `{}` has no value", node.id))?;
        let namespace = nonempty(string_param(node, "namespace"), "flows");
        let title = nonempty(
            string_param(node, "title"),
            &format!("{} · {}", self.flow, node.label.as_deref().unwrap_or(&node.id)),
        );
        let description = sanitize_annotation(nonempty(string_param(node, "description"), &self.description_default));
        let alias_text = string_param(node, "alias");
        let alias = if alias_text.trim().is_empty() {
            None
        } else {
            let full = if alias_text.contains('/') {
                alias_text
            } else {
                format!("{namespace}/{}", slug(&alias_text))
            };
            let parsed = AssetAlias::new(full)
                .map_err(|error| format!("Publish node `{}` alias: {error}", node.id))?;
            if parsed.namespace() != namespace {
                return Err(format!(
                    "Publish node `{}` alias namespace does not match `{namespace}`",
                    node.id
                ));
            }
            Some(parsed)
        };
        let tags = match param(node, "tags") {
            Some(Literal::Arr(values)) if !values.is_empty() => values
                .iter()
                .filter_map(|value| match value {
                    Literal::Str(value) | Literal::Id(value) => Some(value.clone()),
                    _ => None,
                })
                .collect(),
            _ => vec!["flow".to_string(), self.flow.clone()],
        };
        let (kind, artifact, thumbnail) = publish_parts(value)?;
        let mut request = PublishRequest::new(namespace.clone(), kind, title.clone(), artifact, thumbnail);
        if request.artifact.media == MediaType::Glb {
            request.stats = glb_stats(&value.bytes)?;
        }
        request.description = description;
        request.alias = alias;
        request.tags = tags;
        request.creator = format!("flow:{}/{}", self.flow, self.instance);
        self.published_meta = Some((namespace, title, makepad_asset_client::dto::kind_name(kind).to_string()));
        let worker = self
            .worker
            .as_ref()
            .ok_or_else(|| "no asset server discovered on this LAN".to_string())?;
        self.reply = Some(worker.publish(request)?);
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
            Ok(PublishWorkerEvent::Progress(stage)) => Poll::Progress {
                permille: if stage == "uploading" { 500 } else { 50 },
                stage: stage.to_string(),
            },
            Ok(PublishWorkerEvent::Done(Ok(published))) => {
                self.reply = None;
                let (namespace, title, kind) = self.published_meta.take().unwrap();
                let alias = published.alias.as_ref().map(ToString::to_string);
                let json = Json::Obj(vec![
                    ("id".to_string(), Json::Str(published.asset_id.to_string())),
                    ("revision".to_string(), Json::Str(published.revision.to_string())),
                    ("alias".to_string(), alias.map(Json::Str).unwrap_or(Json::Null)),
                    ("namespace".to_string(), Json::Str(namespace)),
                    ("title".to_string(), Json::Str(title)),
                    ("kind".to_string(), Json::Str(kind)),
                ]);
                self.queue.push_back(Poll::Done(vec![("asset".to_string(), Value::json(json.to_json()))]));
                Poll::Progress { permille: 1_000, stage: "published".to_string() }
            }
            Ok(PublishWorkerEvent::Done(Err(error))) => {
                self.reply = None;
                Poll::Failed(error)
            }
            Err(TryRecvError::Empty) => Poll::Pending,
            Err(TryRecvError::Disconnected) => {
                self.reply = None;
                Poll::Failed("asset worker stopped during publish".to_string())
            }
        }
    }

    fn cancel(&mut self) {
        self.cancelled = true;
        self.reply = None;
        self.queue.clear();
    }
}

fn glb_stats(bytes: &[u8]) -> Result<makepad_asset_client::PublishStats, String> {
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
    Ok(makepad_asset_client::PublishStats {
        triangles,
        vertices,
        joints,
        clips: parsed.document.animations.as_ref().map_or(0, |v| v.len().min(u16::MAX as usize) as u16),
    })
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

fn publish_parts(value: &Value) -> Result<(AssetKind, PublishFile, PublishThumbnail), String> {
    let (kind, role, media, dims) = match value.ty {
        PortType::Image => {
            let (media, dims) = image_type_and_dims(&value.bytes, &value.content_type)?;
            (AssetKind::Texture, FileRole::Texture, media, Some(dims))
        }
        PortType::Audio => (
            AssetKind::Audio,
            FileRole::Audio,
            media_type(&value.content_type, &[MediaType::Wav, MediaType::Ogg, MediaType::Mp3])?,
            None,
        ),
        PortType::Video => (
            AssetKind::Video,
            FileRole::Video,
            media_type(&value.content_type, &[MediaType::Mp4])?,
            None,
        ),
        PortType::Mesh if value.bytes.starts_with(b"ply\n") || value.bytes.starts_with(b"ply\r\n") => (
            AssetKind::World,
            FileRole::Splat,
            MediaType::Ply,
            None,
        ),
        PortType::Mesh => (
            AssetKind::Mesh,
            FileRole::RenderGlb,
            media_type(&value.content_type, &[MediaType::Glb])?,
            None,
        ),
        PortType::Text => (AssetKind::Data, FileRole::Source, MediaType::Text, None),
        PortType::Json | PortType::List => (AssetKind::Data, FileRole::Source, MediaType::Json, None),
        PortType::Bytes => (AssetKind::Data, FileRole::Source, MediaType::Bin, None),
    };
    let media_millis = match media {
        MediaType::Wav | MediaType::Ogg | MediaType::Mp3 => audio_duration_millis(&value.bytes, media)?,
        MediaType::Mp4 => video_duration_millis(&value.bytes)?,
        _ => 0,
    };
    let thumbnail = if value.ty == PortType::Image {
        let (width, height) = dims.unwrap();
        if (256..=4096).contains(&width) && (256..=4096).contains(&height) {
            image_thumbnail(&value.bytes, media)?
        } else {
            placeholder_thumbnail()?
        }
    } else {
        placeholder_thumbnail()?
    };
    Ok((
        kind,
        PublishFile {
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

fn audio_duration_millis(bytes: &[u8], media: MediaType) -> Result<u64, String> {
    let seconds = match media {
        MediaType::Wav => wav_duration_secs(bytes)?,
        MediaType::Ogg | MediaType::Mp3 => makepad_audio_decode::probe_duration(bytes)
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

fn media_type(content_type: &str, allowed: &[MediaType]) -> Result<MediaType, String> {
    let base = content_type.split(';').next().unwrap_or(content_type).trim().to_ascii_lowercase();
    let found = match base.as_str() {
        "audio/wav" | "audio/x-wav" => MediaType::Wav,
        "audio/ogg" | "application/ogg" => MediaType::Ogg,
        "audio/mpeg" => MediaType::Mp3,
        "video/mp4" => MediaType::Mp4,
        "model/gltf-binary" | "application/octet-stream" => MediaType::Glb,
        _ => return Err(format!("unsupported content type `{content_type}` for Publish")),
    };
    allowed.contains(&found)
        .then_some(found)
        .ok_or_else(|| format!("content type `{content_type}` does not match the value type"))
}

fn image_type_and_dims(bytes: &[u8], content_type: &str) -> Result<(MediaType, (u32, u32)), String> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") && bytes.len() >= 24 {
        return Ok((
            MediaType::Png,
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
                return Ok((MediaType::Jpeg, (
                    u16::from_be_bytes([bytes[at + 7], bytes[at + 8]]) as u32,
                    u16::from_be_bytes([bytes[at + 5], bytes[at + 6]]) as u32,
                )));
            }
            at += 2 + len;
        }
    }
    Err(format!("Publish image is not a valid PNG or JPEG (`{content_type}`)"))
}

/// Decode and shrink image cards on the long-lived asset worker. The source
/// artifact remains untouched; the catalog thumbnail is capped at 256 px per
/// side so browsing cannot allocate a 4096² texture for every row.
fn image_thumbnail(bytes: &[u8], media: MediaType) -> Result<PublishThumbnail, String> {
    let (pixels, width, height, colorspace) = match media {
        MediaType::Png => {
            let options = DecoderOptions::default()
                .set_max_width(4096)
                .set_max_height(4096)
                .png_set_strip_to_8bit(true);
            let mut decoder = PngDecoder::new_with_options(ZCursor::new(bytes), options);
            decoder.decode_headers().map_err(|error| format!("decode PNG thumbnail: {error}"))?;
            let (width, height) = decoder.dimensions().ok_or("PNG thumbnail dimensions missing")?;
            let colorspace = decoder.colorspace().ok_or("PNG thumbnail colorspace missing")?;
            let pixels = match decoder.decode().map_err(|error| format!("decode PNG thumbnail: {error}"))? {
                DecodingResult::U8(pixels) => pixels,
                _ => return Err("PNG thumbnail did not decode to 8-bit pixels".into()),
            };
            (pixels, width, height, colorspace)
        }
        MediaType::Jpeg => {
            let options = DecoderOptions::default()
                .set_max_width(4096)
                .set_max_height(4096)
                .jpeg_set_out_colorspace(ColorSpace::RGB);
            let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
            decoder.decode_headers().map_err(|error| format!("decode JPEG thumbnail: {error}"))?;
            let (width, height) = decoder.dimensions().ok_or("JPEG thumbnail dimensions missing")?;
            let pixels = decoder.decode().map_err(|error| format!("decode JPEG thumbnail: {error}"))?;
            (pixels, width, height, ColorSpace::RGB)
        }
        _ => return Err("image thumbnail requires PNG or JPEG".into()),
    };
    let rgba = rgba_pixels(&pixels, colorspace)?;
    let (out_width, out_height) = bounded_image_size(width, height);
    let mut out = vec![0u8; out_width * out_height * 4];
    for y in 0..out_height {
        let source_y = y * height / out_height;
        for x in 0..out_width {
            let source_x = x * width / out_width;
            let source = (source_y * width + source_x) * 4;
            let target = (y * out_width + x) * 4;
            out[target..target + 4].copy_from_slice(&rgba[source..source + 4]);
        }
    }
    let png = makepad_ai_hub::testpattern::encode_png_rgba(&out, out_width, out_height)
        .map_err(|error| format!("encode image thumbnail: {error}"))?;
    Ok(PublishThumbnail::plain(png, ThumbnailMedia::Png, out_width as u32, out_height as u32))
}

fn bounded_image_size(width: usize, height: usize) -> (usize, usize) {
    let max_side = width.max(height).max(1);
    let out_width = (width * 256 / max_side).max(1);
    let out_height = (height * 256 / max_side).max(1);
    (out_width, out_height)
}

fn rgba_pixels(pixels: &[u8], colorspace: ColorSpace) -> Result<Vec<u8>, String> {
    let components = colorspace.num_components();
    if components == 0 || pixels.len() % components != 0 {
        return Err("decoded image has invalid channel data".into());
    }
    let mut rgba = Vec::with_capacity(pixels.len() / components * 4);
    for chunk in pixels.chunks_exact(components) {
        let (r, g, b, a) = match colorspace {
            ColorSpace::RGB | ColorSpace::YCbCr => (chunk[0], chunk[1], chunk[2], 255),
            ColorSpace::RGBA => (chunk[0], chunk[1], chunk[2], chunk[3]),
            ColorSpace::BGR => (chunk[2], chunk[1], chunk[0], 255),
            ColorSpace::BGRA => (chunk[2], chunk[1], chunk[0], chunk[3]),
            ColorSpace::Luma => (chunk[0], chunk[0], chunk[0], 255),
            ColorSpace::LumaA => (chunk[0], chunk[0], chunk[0], chunk[1]),
            ColorSpace::ARGB => (chunk[1], chunk[2], chunk[3], chunk[0]),
            _ => return Err("unsupported image colorspace".into()),
        };
        rgba.extend_from_slice(&[r, g, b, a]);
    }
    Ok(rgba)
}

fn video_thumbnail(bytes: &[u8]) -> Result<PublishThumbnail, String> {
    let frame = makepad_video::decode_first_frame_from_bytes(bytes)
        .map_err(|error| format!("decode first video frame: {error}"))?;
    let rgb = frame.to_rgb8();
    let (width, height) = (frame.width as usize, frame.height as usize);
    if width == 0 || height == 0 || rgb.len() != width.saturating_mul(height).saturating_mul(3) {
        return Err("video poster has invalid dimensions".into());
    }
    // Fill the square library tile with a centered crop. The full video
    // remains available in the viewer at its original aspect ratio.
    const SIDE: usize = 256;
    let crop = width.min(height);
    let (left, top) = ((width - crop) / 2, (height - crop) / 2);
    let mut rgba = vec![255; SIDE * SIDE * 4];
    for y in 0..SIDE {
        for x in 0..SIDE {
            let source = ((top + y * crop / SIDE) * width + left + x * crop / SIDE) * 3;
            let target = (y * SIDE + x) * 4;
            rgba[target..target + 3].copy_from_slice(&rgb[source..source + 3]);
        }
    }
    let png = makepad_ai_hub::testpattern::encode_png_rgba(&rgba, SIDE, SIDE)
        .map_err(|error| format!("encode video poster: {error}"))?;
    Ok(PublishThumbnail::plain(png, ThumbnailMedia::Png, SIDE as u32, SIDE as u32))
}

fn placeholder_thumbnail() -> Result<PublishThumbnail, String> {
    let mut pixels = vec![0u8; 256 * 256 * 4];
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[39, 45, 55, 255]);
    }
    let bytes = makepad_ai_hub::testpattern::encode_png_rgba(&pixels, 256, 256)
        .map_err(|error| format!("build Publish thumbnail: {error}"))?;
    Ok(PublishThumbnail::plain(bytes, ThumbnailMedia::Png, 256, 256))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn video_posters_decode_h264_and_hevc_without_replacing_the_original() {
        use makepad_video::{VideoFileCodec, VideoFileEncoder, VideoFileEncoderOptions};
        let dir = std::env::temp_dir().join(format!("flow-video-poster-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        for codec in [VideoFileCodec::H264, VideoFileCodec::H265] {
            let mut posters = Vec::new();
            for (index, color) in [[210u8, 30, 60], [20u8, 90, 220]].into_iter().enumerate() {
                let path = dir.join(format!("{codec:?}-{index}.mp4"));
                let mut encoder = VideoFileEncoder::new(path.to_str().unwrap(), VideoFileEncoderOptions {
                    codec, width: 128, height: 64, fps_num: 24, fps_den: 1,
                    video_bitrate_bps: 1_000_000, audio: None, keyframe_only: true,
                }).unwrap();
                encoder.push_frame_rgb8(&color.repeat(128 * 64), None).unwrap();
                encoder.finish().unwrap();
                let original = std::fs::read(&path).unwrap();
                let poster = video_thumbnail(&original).unwrap();
                assert_eq!((poster.width, poster.height), (256, 256));
                assert_eq!(poster.media, ThumbnailMedia::Png);
                assert_ne!(poster.bytes, placeholder_thumbnail().unwrap().bytes);
                assert_eq!(std::fs::read(&path).unwrap(), original);
                posters.push(poster.bytes);
            }
            assert_ne!(posters[0], posters[1], "poster must reflect the source frame's pixels");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn slug_is_alias_safe() {
        assert_eq!(slug("My Result!"), "my-result");
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
        let thumbnail = image_thumbnail(&png, MediaType::Png).unwrap();
        assert_eq!((thumbnail.width, thumbnail.height), (256, 128));
        assert_eq!(image_type_and_dims(&thumbnail.bytes, "image/png").unwrap().1, (256, 128));
    }

    #[test]
    fn binary_preview_refuses_payload_and_reports_declared_size() {
        let preview = preview_fallback("application/octet-stream", 9 * 1024 * 1024);
        assert_eq!(preview.content_type, "text/plain; charset=utf-8");
        assert_eq!(preview.bytes, "Binary data · 9437184 bytes".as_bytes());
    }

    #[test]
    fn mesh_media_is_classified_and_glb_stats_are_measured() {
        let glb = makepad_gltf::write_glb_mesh(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], &[0, 1, 2]);
        let value = Value::media(PortType::Mesh, "model/gltf-binary", glb);
        let (kind, file, _) = publish_parts(&value).unwrap();
        assert_eq!(kind, AssetKind::Mesh);
        assert_eq!(file.media, MediaType::Glb);
        let stats = glb_stats(&value.bytes).unwrap();
        assert_eq!((stats.vertices, stats.triangles), (3, 1));

        let ply = Value::media(PortType::Mesh, "application/ply", b"ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n0 0 0\n1 0 0\n0 1 0\n3 0 1 2\n".to_vec());
        let (kind, file, _) = publish_parts(&ply).unwrap();
        assert_eq!(kind, AssetKind::World);
        assert_eq!(file.media, MediaType::Ply);
    }
}
