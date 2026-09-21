//! Browser-local music overlay backed by the embedded asset store.

use makepad_asset_client::PublishRequest as ClientPublishRequest;
use makepad_asset_data::{sha256, AssetId, AssetKind, AssetManifest, AssetRevisionId, BlobId, FileRole, MediaType};
use makepad_asset_importer::music_import::TrackOutcome;
use makepad_asset_store::{
    AssetAnnotation, BlobUpload, Budgets, EmbeddedStore, PublishBatchItem,
    PublishRequest as StorePublishRequest, QuotaPolicy, SearchViewer, StorageCommand,
    StorageValues, ViewerScope, Visibility,
};
use makepad_widgets::makepad_platform::{
    Cx, Event, StorageError, StorageEstimate, StorageHandle, StorageRequestId, StorageResult,
};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;

const STORE_ID: &str = "makepad-vj-browser-library-v1";
const LIST_LIMIT: u32 = 1024;

#[derive(Clone)]
pub struct BrowserTrack {
    pub asset: AssetId,
    pub revision: AssetRevisionId,
    pub title: String,
    pub artist: String,
    pub alias: String,
    pub media_blob: BlobId,
    pub media_len: u64,
    pub media: MediaType,
    pub bytes: Arc<[u8]>,
}

#[derive(Default)]
struct Values {
    map: BTreeMap<String, Vec<u8>>,
    changes: Vec<StorageCommand>,
}

enum WorkerCommand {
    Publish { name: String, request: ClientPublishRequest },
}

enum WorkerEvent {
    Ready {
        tracks: Vec<BrowserTrack>,
        changes: Vec<StorageCommand>,
        elapsed_ms: f64,
    },
    Published {
        name: String,
        result: Result<TrackOutcome, String>,
        tracks: Vec<BrowserTrack>,
        changes: Vec<StorageCommand>,
    },
    Failed(String),
}

struct WorkerStore {
    values: Values,
    store: EmbeddedStore,
    tracks: Vec<BrowserTrack>,
}

impl Values {
    fn usage(&self) -> u64 {
        self.map.values().map(|value| value.len() as u64).sum()
    }

    fn take_changes(&mut self) -> Vec<StorageCommand> {
        std::mem::take(&mut self.changes)
    }
}

impl StorageValues for Values {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        Ok(self.map.get(key).cloned())
    }

    fn set(&mut self, key: &str, value: Vec<u8>) -> Result<(), StorageError> {
        self.map.insert(key.to_string(), value.clone());
        self.changes.push(StorageCommand::Set { key: key.to_string(), value });
        Ok(())
    }

    fn delete(&mut self, key: &str) -> Result<(), StorageError> {
        self.map.remove(key);
        self.changes.push(StorageCommand::Delete { key: key.to_string() });
        Ok(())
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        Ok(self
            .map
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect())
    }
}

#[derive(Default)]
pub struct BrowserStore {
    handle: Option<StorageHandle>,
    list_request: Option<StorageRequestId>,
    list_keys: Vec<String>,
    gets: HashMap<StorageRequestId, String>,
    writes: HashMap<StorageRequestId, String>,
    values: Values,
    worker_tx: Option<Sender<WorkerCommand>>,
    worker_rx: Option<Receiver<WorkerEvent>>,
    publish_results: VecDeque<(String, Result<TrackOutcome, String>)>,
    ready: bool,
    tracks: Vec<BrowserTrack>,
    error: Option<String>,
}

impl BrowserStore {
    pub fn start(&mut self, cx: &mut Cx) {
        if self.handle.is_some() {
            return;
        }
        let namespace = match makepad_asset_store::storage_namespace(STORE_ID) {
            Ok(namespace) => namespace,
            Err(error) => {
                self.error = Some(format!("browser store namespace: {error:?}"));
                return;
            }
        };
        let handle = cx.storage(&namespace);
        self.list_request = Some(handle.list(cx, "", None, LIST_LIMIT));
        self.handle = Some(handle);
    }

    pub fn is_ready(&self) -> bool {
        self.ready
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn tracks(&self) -> &[BrowserTrack] {
        &self.tracks
    }

    pub fn track(&self, asset: &AssetId) -> Option<&BrowserTrack> {
        self.tracks.iter().find(|track| track.asset == *asset)
    }

    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> bool {
        let mut changed = self.poll_worker(cx);
        let Event::Storage(responses) = event else { return changed };
        let Some(handle) = self.handle.clone() else { return changed };
        let namespace = handle.namespace().to_string();
        for response in responses.iter().filter(|response| response.namespace == namespace) {
            if let Some(key) = self.writes.remove(&response.request_id) {
                if let Err(error) = &response.result {
                    self.error = Some(format!("persist {key}: {error}"));
                    changed = true;
                }
                continue;
            }
            if self.list_request == Some(response.request_id) {
                self.list_request = None;
                match &response.result {
                    Ok(StorageResult::List(page)) => {
                        self.list_keys.extend(page.keys.iter().cloned());
                        if let Some(cursor) = page.next_cursor.clone() {
                            self.list_request = Some(handle.list(cx, "", Some(cursor), LIST_LIMIT));
                        } else if self.list_keys.is_empty() {
                            changed |= self.start_restore(cx);
                        } else {
                            for key in std::mem::take(&mut self.list_keys) {
                                let id = handle.get(cx, &key);
                                self.gets.insert(id, key);
                            }
                        }
                    }
                    Ok(_) => self.error = Some("browser store list returned the wrong payload".into()),
                    Err(error) => self.error = Some(format!("browser store list: {error}")),
                }
                continue;
            }
            let Some(key) = self.gets.remove(&response.request_id) else { continue };
            match &response.result {
                Ok(StorageResult::Value(Some(value))) => {
                    self.values.map.insert(key, value.clone());
                }
                Ok(StorageResult::Value(None)) => {}
                Ok(_) => self.error = Some("browser store get returned the wrong payload".into()),
                Err(error) => self.error = Some(format!("browser store get: {error}")),
            }
            if self.gets.is_empty() {
                changed |= self.start_restore(cx);
            }
        }
        changed
    }

    pub fn publish(
        &mut self,
        name: String,
        request: ClientPublishRequest,
    ) -> Result<(), String> {
        if !self.ready {
            return Err("browser library is still opening".into());
        }
        self.worker_tx
            .as_ref()
            .ok_or("browser library worker is unavailable")?
            .send(WorkerCommand::Publish { name, request })
            .map_err(|_| "browser library worker stopped".to_string())
    }

    pub fn take_publish_result(&mut self) -> Option<(String, Result<TrackOutcome, String>)> {
        self.publish_results.pop_front()
    }

    fn start_restore(&mut self, cx: &mut Cx) -> bool {
        if self.ready || self.worker_tx.is_some() || self.error.is_some() {
            return false;
        }
        let values = Arc::new(std::sync::Mutex::new(Some(std::mem::take(&mut self.values))));
        let worker_values = values.clone();
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let options = makepad_widgets::makepad_platform::thread::ThreadOptions {
            name: Some("vj-browser-store".into()),
            ..Default::default()
        };
        match cx.thread_spawner().spawn_worker(options, move || {
            let started = Cx::monotonic_now();
            let mut values = worker_values.lock().unwrap().take().unwrap();
            let store = match EmbeddedStore::open_durable(
                &mut values,
                Budgets::default_v1(),
                QuotaPolicy::default(),
            ) {
                Ok(store) => store,
                Err(error) => {
                    let _ = event_tx.send(WorkerEvent::Failed(format!(
                        "open browser library: {error:?}"
                    )));
                    return;
                }
            };
            let tracks = match WorkerStore::restored_tracks(&store, &values) {
                Ok(tracks) => tracks,
                Err(error) => {
                    let _ = event_tx.send(WorkerEvent::Failed(error));
                    return;
                }
            };
            let mut worker = WorkerStore { values, store, tracks };
            let ready = WorkerEvent::Ready {
                tracks: worker.tracks.clone(),
                changes: worker.values.take_changes(),
                elapsed_ms: (Cx::monotonic_now() - started) * 1e3,
            };
            if event_tx.send(ready).is_err() {
                return;
            }
            while let Ok(command) = command_rx.recv() {
                match command {
                    WorkerCommand::Publish { name, request } => {
                        let result = worker.publish(request);
                        let event = WorkerEvent::Published {
                            name,
                            result,
                            tracks: worker.tracks.clone(),
                            changes: worker.values.take_changes(),
                        };
                        if event_tx.send(event).is_err() {
                            return;
                        }
                    }
                }
            }
        }) {
            Ok(handle) => {
                handle.detach();
                self.worker_tx = Some(command_tx);
                self.worker_rx = Some(event_rx);
                false
            }
            Err(error) => {
                self.values = values.lock().unwrap().take().unwrap_or_default();
                self.error = Some(format!("start browser library worker: {error}"));
                true
            }
        }
    }

    fn poll_worker(&mut self, cx: &mut Cx) -> bool {
        let mut changed = false;
        loop {
            let event = match self.worker_rx.as_ref().map(Receiver::try_recv) {
                Some(Ok(event)) => event,
                Some(Err(TryRecvError::Empty)) | None => break,
                Some(Err(TryRecvError::Disconnected)) => {
                    self.worker_rx = None;
                    self.worker_tx = None;
                    self.ready = false;
                    self.error = Some("browser library worker stopped without an answer".into());
                    changed = true;
                    break;
                }
            };
            changed = true;
            match event {
                WorkerEvent::Ready { tracks, changes, elapsed_ms } => {
                    self.ready = true;
                    self.tracks = tracks;
                    self.flush_changes(cx, changes);
                    makepad_widgets::log!(
                        "browser library: ready in {elapsed_ms:.0}ms off UI thread"
                    );
                }
                WorkerEvent::Published { name, result, tracks, changes } => {
                    self.tracks = tracks;
                    self.flush_changes(cx, changes);
                    self.publish_results.push_back((name, result));
                }
                WorkerEvent::Failed(error) => {
                    self.error = Some(error);
                    self.ready = false;
                    self.worker_rx = None;
                    self.worker_tx = None;
                    break;
                }
            }
        }
        changed
    }

    fn flush_changes(&mut self, cx: &mut Cx, changes: Vec<StorageCommand>) {
        let Some(handle) = self.handle.as_ref() else { return };
        for command in changes {
            let (id, key) = match command {
                StorageCommand::Set { key, value } => (handle.set(cx, &key, value), key),
                StorageCommand::Delete { key } => (handle.delete(cx, &key), key),
                StorageCommand::Get { key } => {
                    self.error = Some(format!("unexpected embedded-store get command for {key}"));
                    continue;
                }
            };
            self.writes.insert(id, key);
        }
    }
}

impl WorkerStore {
    fn publish(&mut self, request: ClientPublishRequest) -> Result<TrackOutcome, String> {
        let alias = request.alias.clone().ok_or("prepared track has no alias")?;
        let existing = self.store
            .resolve_alias(&alias)
            .map_err(|error| format!("resolve local alias: {error:?}"))?;
        let asset = request.asset_id.or(existing.map(|target| target.asset_id)).unwrap_or_else(|| {
            let mut input = b"makepad-vj-browser-track-v1\0".to_vec();
            input.extend_from_slice(alias.as_str().as_bytes());
            let digest = sha256(&input);
            let mut bytes = [0; 16];
            bytes.copy_from_slice(&digest[..16]);
            AssetId::from_bytes(bytes)
        });
        let (manifest_bytes, revision) = request
            .manifest_for_asset(asset)
            .map_err(|error| format!("build local manifest: {error}"))?;
        let audio_blob = BlobId::hash_of(&request.artifact.bytes);
        let thumbnail_blob = BlobId::hash_of(&request.thumbnail.bytes);
        let store_request = StorePublishRequest {
            blobs: vec![
                BlobUpload { expected: audio_blob, bytes: request.artifact.bytes.clone() },
                BlobUpload { expected: thumbnail_blob, bytes: request.thumbnail.bytes.clone() },
            ],
            items: vec![PublishBatchItem {
                namespace: request.namespace.clone(),
                manifest_bytes,
                annotation: AssetAnnotation {
                    title: request.title.clone(),
                    description: request.description.clone(),
                    kind: Some(request.kind),
                    categories: request.categories.clone(),
                    tags: request.tags.clone(),
                    creator: request.creator.clone(),
                    artist: request.artist.clone(),
                    artist_url: request.artist_url.clone(),
                    album: request.album.clone(),
                    source_url: request.source_url.clone(),
                    license: request.license.clone(),
                    license_url: request.license_url.clone(),
                    owner: None,
                    generator: request.generator.clone(),
                    backend: request.backend.clone(),
                    model: request.model.clone(),
                    prompt: request.prompt.clone(),
                    provenance: request.provenance.clone(),
                    visibility: Visibility::Public,
                },
                alias: Some(alias.clone()),
            }],
            now_ms: (Cx::time_now().max(0.0) * 1000.0) as u64,
        };
        let usage = self.values.usage();
        let outcomes = self.store
            .publish_durable(
                &mut self.values,
                StorageEstimate { usage, quota: usage.saturating_add(2 * 1024 * 1024 * 1024) },
                store_request,
            )
            .map_err(|error| format!("publish local track: {error:?}"))?;
        let already = outcomes.first().is_some_and(|outcome| outcome.already_published);
        self.tracks.retain(|track| track.asset != asset);
        self.tracks.push(BrowserTrack {
            asset,
            revision,
            title: request.title,
            artist: request.creator,
            alias: alias.to_string(),
            media_blob: audio_blob,
            media_len: request.artifact.bytes.len() as u64,
            media: request.artifact.media,
            bytes: Arc::from(request.artifact.bytes),
        });
        self.tracks.sort_by(|left, right| left.title.to_lowercase().cmp(&right.title.to_lowercase()));
        Ok(if already {
            TrackOutcome::Unchanged
        } else if existing.is_some() {
            TrackOutcome::Updated
        } else {
            TrackOutcome::Published
        })
    }

    fn restored_tracks(
        store: &EmbeddedStore,
        values: &Values,
    ) -> Result<Vec<BrowserTrack>, String> {
        let viewer = SearchViewer { principal: None, scope: ViewerScope::All };
        let mut cursor = None;
        let mut tracks = Vec::new();
        loop {
            let page = store
                .list(Some("music"), 256, &viewer, cursor.as_deref())
                .map_err(|error| format!("list browser library: {error:?}"))?;
            for hit in page.hits {
                if hit.kind != Some(AssetKind::Audio) {
                    continue;
                }
                let detail = store
                    .detail(&hit.asset_id)
                    .map_err(|error| format!("read browser track: {error:?}"))?
                    .ok_or("browser track detail disappeared")?;
                let target = detail.aliases.first().map(|(_, target)| *target)
                    .ok_or("browser track has no live alias")?;
                let manifest_bytes = store
                    .read_revision(&target.revision)
                    .map_err(|error| format!("read browser manifest: {error:?}"))?
                    .ok_or("browser track manifest disappeared")?;
                let manifest = AssetManifest::from_canonical_bytes(&manifest_bytes)
                    .map_err(|error| format!("decode browser manifest: {error}"))?;
                let file = manifest.files.iter().find(|file| file.role == FileRole::Audio)
                    .ok_or("browser track has no audio file")?;
                let bytes = store
                    .read_blob_durable(values, &file.blob)
                    .map_err(|error| format!("read browser audio: {error:?}"))?;
                tracks.push(BrowserTrack {
                    asset: hit.asset_id,
                    revision: target.revision,
                    title: hit.title,
                    artist: detail.annotation.as_ref().map_or_else(String::new, |value| value.creator.clone()),
                    alias: hit.alias.unwrap_or_default(),
                    media_blob: file.blob,
                    media_len: file.byte_len,
                    media: file.media,
                    bytes: Arc::from(bytes),
                });
            }
            let Some(next) = page.cursor else { break };
            cursor = Some(next);
        }
        Ok(tracks)
    }
}
