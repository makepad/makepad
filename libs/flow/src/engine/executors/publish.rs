use super::{param, string_param, Executor, Poll};
use crate::{FlowAsset, Literal, Node, PortType, Value};
use makepad_asset_client::{
    content_client_caps, ApiEndpoints, AssetClient, CatalogHit, CatalogQuery, ClientConfig,
    ClientError, DiscoveryListener, PublishFile, PublishRequest, PublishThumbnail, Published,
};
use makepad_asset_data::{AssetAlias, AssetKind, FileRole, MediaType, ThumbnailMedia};
use makepad_strict_json::Value as Json;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const DISCOVERY_TTL_MS: u64 = 10_000;

#[derive(Clone, Debug)]
pub struct AssetStoreConfig {
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
    List(AssetListQuery, Sender<Result<Vec<FlowAsset>, String>>),
    Thumbnail(AssetAlias, Sender<Result<ThumbnailBytes, String>>),
    Stop,
}

#[derive(Clone)]
pub struct AssetWorkerHandle {
    tx: Sender<AssetCommand>,
}

impl AssetWorkerHandle {
    pub fn publish(&self, request: PublishRequest) -> Result<Receiver<PublishWorkerEvent>, String> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(AssetCommand::Publish(request, tx))
            .map_err(|_| "asset worker is not running".to_string())?;
        Ok(rx)
    }

    pub fn list(&self, query: AssetListQuery) -> Result<Vec<FlowAsset>, String> {
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
        let handle = AssetWorkerHandle { tx };
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
    while let Ok(command) = rx.recv() {
        match command {
            AssetCommand::Stop => break,
            AssetCommand::Publish(mut request, reply) => {
                let result = (|| {
                    let client = connected_client(&config, &mut client)?;
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
                    .and_then(|client| list_assets(client, &query));
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

fn list_assets(client: &AssetClient, query: &AssetListQuery) -> Result<Vec<FlowAsset>, String> {
    let limit = query.limit.clamp(1, 100);
    let make_query = |namespace: Option<String>, tag: Option<String>| {
        let mut value = if query.text.trim().is_empty() {
            CatalogQuery::browse(limit)
        } else {
            CatalogQuery::text(query.text.trim(), limit)
        };
        value.namespace = namespace;
        value.tag = tag;
        value
    };
    let mut hits = Vec::new();
    let mut seen = HashSet::new();
    let primary = make_query(query.namespace.clone(), None);
    for hit in client
        .catalog_search(&primary, None)
        .map_err(|error| format!("browse asset catalog: {error}"))?
        .hits
    {
        if seen.insert(hit.asset_id) {
            hits.push(hit);
        }
    }
    if query.namespace.as_deref() == Some("flows") {
        let tagged = make_query(None, Some("flow".to_string()));
        for hit in client
            .catalog_search(&tagged, None)
            .map_err(|error| format!("browse flow-tagged assets: {error}"))?
            .hits
        {
            if seen.insert(hit.asset_id) {
                hits.push(hit);
            }
        }
    }
    if hits.is_empty() {
        return Ok(Vec::new());
    }
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
    rows.truncate(limit as usize);
    Ok(rows)
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
        let description = nonempty(string_param(node, "description"), &self.description_default);
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

fn nonempty(value: String, fallback: &str) -> String {
    if value.trim().is_empty() { fallback.to_string() } else { value }
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
        PortType::Mesh => (
            AssetKind::Mesh,
            FileRole::RenderGlb,
            media_type(&value.content_type, &[MediaType::Glb])?,
            None,
        ),
        PortType::Text => (AssetKind::Data, FileRole::Source, MediaType::Text, None),
        PortType::Json | PortType::List => (AssetKind::Data, FileRole::Source, MediaType::Json, None),
        PortType::Bytes => return Err("Publish does not accept an untyped bytes value".to_string()),
    };
    let thumbnail = if value.ty == PortType::Image {
        let (width, height) = dims.unwrap();
        if (256..=4096).contains(&width) && (256..=4096).contains(&height) {
            PublishThumbnail::plain(
                value.bytes.to_vec(),
                if media == MediaType::Png { ThumbnailMedia::Png } else { ThumbnailMedia::Jpeg },
                width,
                height,
            )
        } else {
            placeholder_thumbnail()?
        }
    } else {
        placeholder_thumbnail()?
    };
    Ok((
        kind,
        PublishFile { bytes: value.bytes.to_vec(), media, role, media_millis: 0, dims },
        thumbnail,
    ))
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
}
