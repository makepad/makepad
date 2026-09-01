//! ARCHIVE.ORG: the panel's state, its worker glue, the swatch player and
//! the import publisher. Nothing here draws — the DSL lives in `main.rs`
//! with the rest of the chrome.
//!
//! The search, the item lookups, the tile pictures and the media downloads
//! all run in `makepad_archive_org`'s worker lanes; this module turns their
//! events into panel state and asks for the next thing. Two jobs are the
//! VJ's own and live here:
//!
//! * **the swatch** — a clip is auditioned from the archive's own small
//!   transcode, decoded on a thread of its own at the file's real pace and
//!   handed to the UI as BGRA frames for one texture. It starts while the
//!   file is still coming down: the player reads the growing part file,
//!   and when it reaches the download frontier it waits, reopens and
//!   picks up where it was (see [`SwatchPlayer`]). Silent on purpose: a
//!   swatch is a look, not a cue, and the mixer bus belongs to the decks.
//! * **the import** — only on IMPORT. The chosen file lands in the cache,
//!   gets a thumbnail and a rights record (the archive's Creative Commons
//!   URL mapped honestly, `Unknown` never upgraded to a grant) and is
//!   published to the session's asset server as owned bytes, under an alias
//!   derived from (item, file) so a second IMPORT of the same clip is a
//!   no-op rather than a duplicate. Then the normal catalog path shows it on
//!   the VJ grid like anything else.

use crate::archive_stream::{StreamFailure, StreamFrame, StreamSwatch};
use makepad_archive_org::cache::slug;
use makepad_archive_org::{
    details_url, identifier_key, license_from_url, ArchiveWorker, Cmd, Error as ArchiveError,
    Ev, FileKind, Grant, Item, ItemFile, MediaFilter, Purpose, SearchPage, SearchQuery,
};
use makepad_asset_client::{
    ApiEndpoints, AssetClient, ClientConfig, ClientError, PublishBundle, PublishBundleFile,
    PublishRights, PublishThumbnail,
};
use makepad_asset_data::{
    AssetAlias, AssetId, AssetKind, DerivativePolicy, FileRole, MediaType, Redistribution,
    ThumbnailMedia,
};
use makepad_asset_importer::thumbs;
use makepad_asset_importer::videothumb::probe_video;
use makepad_widgets::makepad_platform::video_file::{nv12, VideoFileDecoder};
use makepad_widgets::{ImageBuffer, Texture};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Namespace archive imports land in — its own, so a search filter or a
/// wipe can address "what came from the archive" without touching local
/// imports or generated content.
pub const NAMESPACE: &str = "archive";
/// Tiles per page: five rows of eight, the pad matrix's own count, so a
/// page of results reads like one bank.
pub const PAGE_ROWS: u32 = 40;
/// A still bigger than this is not decoded for the swatch (it would still
/// import fine — the import path never decodes the payload).
const MAX_STILL_BYTES: u64 = 32 * 1024 * 1024;
const MAX_STILL_DIM: usize = 4096;
const MAX_THUMB_DIM: usize = 2048;

/// A decoded picture, ready to become a texture on the UI thread.
pub struct Pixels {
    pub bgra: Vec<u32>,
    pub width: usize,
    pub height: usize,
}

impl std::fmt::Debug for Pixels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pixels({}x{})", self.width, self.height)
    }
}

/// Where a tile's picture is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ThumbState {
    Requested,
    Ready,
    Failed,
}

/// What the preview well shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Swatch {
    Empty,
    /// The item's metadata is being read.
    Looking,
    /// The swatch file is coming down.
    Loading { loaded: u64, total: Option<u64> },
    /// Playing (or paused). `loading` is the download still streaming in
    /// behind the picture; `None` once it is on disk. `head` is set when
    /// only the first `head` bytes of a bigger file are being auditioned.
    Video {
        playing: bool,
        duration_secs: f64,
        /// The file-based fallback's download behind the picture.
        loading: Option<(u64, Option<u64>)>,
        head: Option<u64>,
        /// The range stream is waiting for bytes.
        buffering: bool,
        /// Bytes the range stream has pulled so far (0 on the file path).
        fetched: u64,
    },
    Still,
    Failed(String),
}

/// The IMPORT button's story.
#[derive(Clone, Debug, PartialEq)]
pub enum ImportState {
    Idle,
    Downloading { loaded: u64, total: Option<u64> },
    Publishing,
    Done(String),
    AlreadyPresent,
    Failed(String),
}

impl ImportState {
    pub fn busy(&self) -> bool {
        matches!(self, ImportState::Downloading { .. } | ImportState::Publishing)
    }
}

/// What changed since the last poll — the host repaints only what it must.
#[derive(Debug)]
pub enum ArchiveChange {
    /// A new page (or none): rebuild the grid.
    Results,
    /// One tile's picture decoded.
    Thumb { identifier: String, pixels: Pixels },
    /// The selection / swatch / import readouts moved.
    Panel,
    /// A still for the well decoded.
    Still(Pixels),
    /// The swatch player has a new frame.
    Frame(Pixels),
    /// An import landed in the store: the catalog grids should look again.
    Published,
    /// A double-click asked for a deck and the item's metadata has now
    /// arrived: cue this onto the next available deck.
    AutoCue { url: String, title: String },
}

/// The session an import publishes into — taken from the live connection
/// at the moment IMPORT is pressed.
#[derive(Clone)]
pub struct PublishTarget {
    pub endpoints: ApiEndpoints,
    pub server_id: [u8; 16],
    pub token: String,
    pub cache: PathBuf,
}

enum DecodeReq {
    Thumb { identifier: String, bytes: Vec<u8> },
    Still { path: PathBuf },
}

enum DecodeDone {
    Thumb { identifier: String, result: Result<Pixels, String> },
    Still { result: Result<Pixels, String> },
}

enum PublishMsg {
    Done(Result<PublishOutcome, String>),
}

enum PublishOutcome {
    Published(String),
    AlreadyPresent,
}

/// What is playing in the well: the range stream (the normal case) or the
/// file-based player the panel falls back to when a file cannot be
/// range-streamed (not H.264, no range support, not an MP4).
pub enum SwatchBackend {
    Stream(StreamSwatch),
    File(SwatchPlayer),
}

impl SwatchBackend {
    fn set_paused(&self, paused: bool) {
        match self {
            SwatchBackend::Stream(s) => s.set_paused(paused),
            SwatchBackend::File(f) => f.set_paused(paused),
        }
    }
    fn is_paused(&self) -> bool {
        match self {
            SwatchBackend::Stream(s) => s.is_paused(),
            SwatchBackend::File(f) => f.is_paused(),
        }
    }
    fn position_secs(&self) -> f64 {
        match self {
            SwatchBackend::Stream(s) => s.position_secs(),
            SwatchBackend::File(f) => f.position_secs(),
        }
    }
    fn duration_secs(&self) -> f64 {
        match self {
            SwatchBackend::Stream(s) => s.duration_secs(),
            SwatchBackend::File(f) => f.duration_secs(),
        }
    }
    fn seek_fraction(&self, fraction: f64) {
        match self {
            SwatchBackend::Stream(s) => s.seek_fraction(fraction),
            SwatchBackend::File(f) => f.seek_fraction(fraction),
        }
    }
    fn take_frame(&self) -> Option<(Vec<u32>, u32, u32)> {
        match self {
            SwatchBackend::Stream(s) => match s.take_frame() {
                Some(StreamFrame::Bgra(bgra, w, h)) => Some((bgra, w, h)),
                // The swatch opens in BGRA mode; NV12 cannot arrive here.
                Some(StreamFrame::Nv12(..)) | None => None,
            },
            SwatchBackend::File(f) => f.take_frame(),
        }
    }
    fn is_buffering(&self) -> bool {
        match self {
            SwatchBackend::Stream(s) => s.is_buffering(),
            SwatchBackend::File(_) => false,
        }
    }
    fn bytes_fetched(&self) -> u64 {
        match self {
            SwatchBackend::Stream(s) => s.bytes_fetched(),
            SwatchBackend::File(_) => 0,
        }
    }
    /// `Some((why, setup))`: `setup` = it never started (the file path may
    /// still play this candidate), else it broke mid-play.
    fn failure(&self) -> Option<(String, bool)> {
        match self {
            SwatchBackend::Stream(s) => s.failure().map(|f| {
                let setup = matches!(f, StreamFailure::Setup(_));
                (f.message().to_string(), setup)
            }),
            SwatchBackend::File(f) => f.failure().map(|why| (why, false)),
        }
    }
}

/// The panel's whole state.
pub struct ArchivePanel {
    pub text: String,
    pub media: MediaFilter,
    pub page: u32,
    pub results: Option<SearchPage>,
    pub searching: bool,
    /// One human line about the search (count, page, error).
    pub status: String,
    /// Index into `results.hits`.
    pub selected: Option<usize>,
    pub item: Option<Item>,
    pub swatch: Swatch,
    pub import: ImportState,
    /// Textures the host created for the tiles, by identifier.
    pub thumb_textures: HashMap<String, Texture>,
    /// The well's texture (a still, or the swatch player's frames).
    pub swatch_texture: Option<Texture>,
    thumbs: HashMap<String, ThumbState>,
    /// Tile key → identifier, for the grid's opaque ids.
    tiles: HashMap<AssetId, String>,
    search_gen: u64,
    item_gen: u64,
    download_gen: u64,
    thumb_epoch: u64,
    /// Which download generation is the import's (the swatch's is
    /// `download_gen` while nothing else is in flight).
    import_gen: Option<u64>,
    /// The selection's swatch candidates in try order, and which one is
    /// up: a decoder refusal (or a failed download) moves to the next.
    swatch_candidates: Vec<ItemFile>,
    swatch_index: usize,
    pending_import: Option<(PublishTarget, ItemFile)>,
    worker: Option<ArchiveWorker>,
    cache_dir: PathBuf,
    decode_tx: Option<Sender<DecodeReq>>,
    decode_rx: Option<Receiver<DecodeDone>>,
    player: Option<SwatchBackend>,
    /// The candidate up now is on the file-based fallback already (its
    /// range stream refused it), so a failure there moves on.
    swatch_on_file_path: bool,
    /// A double-click landed before the item's metadata: fire the deck
    /// autocue the moment the candidates exist.
    pending_autocue: bool,
    publish_rx: Option<Receiver<PublishMsg>>,
}

impl Default for ArchivePanel {
    fn default() -> Self {
        Self {
            text: String::new(),
            media: MediaFilter::ImagesAndVideo,
            page: 1,
            results: None,
            searching: false,
            status: "search the internet archive".to_string(),
            selected: None,
            item: None,
            swatch: Swatch::Empty,
            import: ImportState::Idle,
            thumb_textures: HashMap::new(),
            swatch_texture: None,
            thumbs: HashMap::new(),
            tiles: HashMap::new(),
            search_gen: 0,
            item_gen: 0,
            download_gen: 0,
            thumb_epoch: 0,
            import_gen: None,
            swatch_candidates: Vec::new(),
            swatch_index: 0,
            pending_import: None,
            worker: None,
            cache_dir: PathBuf::new(),
            decode_tx: None,
            decode_rx: None,
            player: None,
            swatch_on_file_path: false,
            pending_autocue: false,
            publish_rx: None,
        }
    }
}

impl ArchivePanel {
    /// Where downloads and tiles are cached: `<cache_parent>/archive-cache`.
    pub fn set_cache_parent(&mut self, cache_parent: &Path) {
        self.cache_dir = cache_parent.join("archive-cache");
    }

    fn worker(&mut self) -> &ArchiveWorker {
        if self.worker.is_none() {
            let dir = if self.cache_dir.as_os_str().is_empty() {
                std::env::temp_dir().join("makepad-vj-archive-cache")
            } else {
                self.cache_dir.clone()
            };
            self.worker = Some(ArchiveWorker::spawn(dir));
        }
        self.worker.as_ref().unwrap()
    }

    fn cache_dir_or_default(&self) -> PathBuf {
        match self.worker.as_ref() {
            Some(w) => w.cache_dir().clone(),
            None => self.cache_dir.clone(),
        }
    }

    fn decode_lane(&mut self) -> &Sender<DecodeReq> {
        if self.decode_tx.is_none() {
            let (req_tx, req_rx) = mpsc::channel::<DecodeReq>();
            let (done_tx, done_rx) = mpsc::channel::<DecodeDone>();
            thread::Builder::new()
                .name("vj-archive-decode".into())
                .spawn(move || {
                    while let Ok(req) = req_rx.recv() {
                        let done = match req {
                            DecodeReq::Thumb { identifier, bytes } => DecodeDone::Thumb {
                                identifier,
                                result: decode_picture(&bytes, MAX_THUMB_DIM),
                            },
                            DecodeReq::Still { path } => DecodeDone::Still {
                                result: std::fs::metadata(&path)
                                    .map_err(|e| e.to_string())
                                    .and_then(|m| {
                                        if m.len() > MAX_STILL_BYTES {
                                            Err(format!("still over {} MB", MAX_STILL_BYTES >> 20))
                                        } else {
                                            std::fs::read(&path).map_err(|e| e.to_string())
                                        }
                                    })
                                    .and_then(|bytes| decode_picture(&bytes, MAX_STILL_DIM)),
                            },
                        };
                        if done_tx.send(done).is_err() {
                            return;
                        }
                    }
                })
                .expect("spawn archive decode thread");
            self.decode_tx = Some(req_tx);
            self.decode_rx = Some(done_rx);
        }
        self.decode_tx.as_ref().unwrap()
    }

    /// The identifier behind a grid tile key.
    pub fn identifier_for(&self, key: AssetId) -> Option<&str> {
        self.tiles.get(&key).map(|s| s.as_str())
    }

    pub fn tile_key(identifier: &str) -> AssetId {
        AssetId::from_bytes(identifier_key(identifier))
    }

    pub fn hits(&self) -> &[makepad_archive_org::SearchHit] {
        self.results.as_ref().map(|r| r.hits.as_slice()).unwrap_or(&[])
    }

    pub fn selected_hit(&self) -> Option<&makepad_archive_org::SearchHit> {
        self.selected.and_then(|i| self.hits().get(i))
    }

    /// Run a search from page one. Empty text with a media filter still
    /// searches (the archive answers with its most-downloaded items).
    pub fn search(&mut self, text: &str) {
        self.text = text.trim().to_string();
        self.page = 1;
        self.run_search();
    }

    pub fn set_media(&mut self, media: MediaFilter) {
        if self.media == media {
            return;
        }
        self.media = media;
        if self.results.is_some() || !self.text.is_empty() {
            self.page = 1;
            self.run_search();
        }
    }

    pub fn pages(&self) -> u32 {
        self.results.as_ref().map(|r| r.pages()).unwrap_or(1)
    }

    pub fn next_page(&mut self) {
        if self.page < self.pages() {
            self.page += 1;
            self.run_search();
        }
    }

    pub fn prev_page(&mut self) {
        if self.page > 1 {
            self.page -= 1;
            self.run_search();
        }
    }

    fn run_search(&mut self) {
        self.search_gen += 1;
        self.thumb_epoch += 1;
        let gen = self.search_gen;
        let mut query = SearchQuery::new(self.text.clone());
        query.media = self.media;
        query.page = self.page;
        query.rows = PAGE_ROWS;
        self.searching = true;
        self.status = if self.text.is_empty() {
            format!("browsing {}…", self.media.label())
        } else {
            format!("searching “{}”…", self.text)
        };
        self.results = None;
        self.thumbs.clear();
        self.tiles.clear();
        self.clear_selection();
        self.worker().send(Cmd::Search { gen, query });
    }

    fn clear_selection(&mut self) {
        self.selected = None;
        self.item = None;
        self.swatch_candidates.clear();
        self.swatch_index = 0;
        self.swatch_on_file_path = false;
        self.pending_autocue = false;
        self.swatch = Swatch::Empty;
        self.player = None;
        self.swatch_texture = None;
        // A swatch download in flight is for a tile nobody is looking at.
        if self.import_gen.is_none() {
            if let Some(w) = self.worker.as_ref() {
                w.cancel_download();
            }
        }
    }

    /// A tile was clicked: look the item up and audition it.
    pub fn select(&mut self, key: AssetId) {
        let Some(identifier) = self.tiles.get(&key).cloned() else { return };
        let Some(index) = self.hits().iter().position(|h| h.identifier == identifier) else {
            return;
        };
        if self.selected == Some(index) && !matches!(self.swatch, Swatch::Failed(_)) {
            return;
        }
        self.clear_selection();
        self.selected = Some(index);
        self.swatch = Swatch::Looking;
        if !self.import.busy() {
            self.import = ImportState::Idle;
        }
        self.item_gen += 1;
        let gen = self.item_gen;
        self.worker().send(Cmd::Item { gen, identifier });
    }

    /// PLAY / PAUSE on the swatch.
    pub fn toggle_play(&mut self) {
        if let Some(player) = self.player.as_ref() {
            let paused = !player.is_paused();
            player.set_paused(paused);
            if let Swatch::Video { playing, .. } = &mut self.swatch {
                *playing = !paused;
            }
        }
    }

    pub fn is_playing(&self) -> bool {
        matches!(self.swatch, Swatch::Video { playing: true, .. })
    }

    /// The scrub bar: jump to a fraction of the swatch.
    pub fn seek_fraction(&mut self, fraction: f64) {
        if let Some(player) = self.player.as_ref() {
            player.seek_fraction(fraction);
        }
    }

    /// `(position, duration)` in seconds while a swatch is up.
    pub fn swatch_time(&self) -> Option<(f64, f64)> {
        let player = self.player.as_ref()?;
        Some((player.position_secs(), player.duration_secs()))
    }

    /// The file IMPORT would take, if the selection has one: the best
    /// video that fits the import limit; for an item with no videos at
    /// all, its primary image. A video item whose every file is over the
    /// limit offers NOTHING — importing its 20 KB cover in place of the
    /// film would be a trick, not an import.
    pub fn import_candidate(&self) -> Option<&ItemFile> {
        Self::import_file_of(self.item.as_ref()?)
    }

    fn import_file_of(item: &Item) -> Option<&ItemFile> {
        match item.import_video_within(makepad_archive_org::MAX_IMPORT_BYTES) {
            Some(video) => Some(video),
            None if item.preview_videos().is_empty() => item.primary_image(),
            None => None,
        }
    }

    /// The video the CUE A / CUE B buttons send to a deck: the candidate
    /// being auditioned when it is a video, else the item's best streamable
    /// one. `(url, title, duration_secs)`.
    pub fn deck_candidate(&self) -> Option<(String, String, f64)> {
        let item = self.item.as_ref()?;
        let file = match self.swatch_candidates.get(self.swatch_index) {
            Some(f) if f.kind() == FileKind::Video => f.clone(),
            _ => item.preview_videos().into_iter().next().cloned()?,
        };
        Some((
            file.download_url(&item.identifier),
            clean_text(&item.title, 80),
            file.length_secs,
        ))
    }

    /// Why there is nothing to import, when there is nothing.
    pub fn import_refusal(&self) -> Option<String> {
        let item = self.item.as_ref()?;
        if Self::import_file_of(item).is_some() {
            return None;
        }
        let smallest = item.preview_videos().into_iter().map(|f| f.size).min();
        Some(match smallest {
            Some(size) => format!(
                "videos here are {}+ — over the {} import limit",
                human_bytes(size),
                human_bytes(makepad_archive_org::MAX_IMPORT_BYTES)
            ),
            None => "no importable video or image in this item".to_string(),
        })
    }

    /// A double-click before the item loaded: remember, and emit
    /// [`ArchiveChange::AutoCue`] when the candidates land.
    pub fn arm_autocue(&mut self) {
        self.pending_autocue = true;
    }

    /// Fetch the swatch candidate at `swatch_index`, or give up with the
    /// reason the last one failed.
    fn start_swatch_candidate(&mut self, last_failure: Option<String>) {
        let Some(identifier) = self.item.as_ref().map(|i| i.identifier.clone()) else { return };
        self.swatch_on_file_path = false;
        match self.swatch_candidates.get(self.swatch_index).cloned() {
            // A video streams by byte range: nothing lands on disk, and
            // the first frame is a few requests away whatever the size.
            Some(file) if file.kind() == FileKind::Video => {
                let url = file.download_url(&identifier);
                makepad_widgets::log!(
                    "archive swatch: range-streaming {} [{}] {} of {}",
                    file.base_name(),
                    file.format,
                    human_bytes(file.size),
                    identifier
                );
                // The swatch starts PAUSED on its poster frame: the well is
                // a look, and a paused stream stops fetching once its few
                // read-ahead windows are in — so auditioning then throwing
                // the clip on a deck never streams the file twice.
                let stream = StreamSwatch::open(url);
                stream.set_paused(true);
                self.player = Some(SwatchBackend::Stream(stream));
                self.swatch_texture = None;
                self.swatch = Swatch::Video {
                    playing: false,
                    duration_secs: file.length_secs,
                    loading: None,
                    head: None,
                    buffering: true,
                    fetched: 0,
                };
            }
            Some(file) => {
                self.download_gen += 1;
                let gen = self.download_gen;
                self.swatch = Swatch::Loading { loaded: 0, total: Some(file.size) };
                self.worker().send(Cmd::Download {
                    gen,
                    identifier,
                    file,
                    purpose: Purpose::Preview,
                });
            }
            None => {
                self.swatch = Swatch::Failed(
                    last_failure.unwrap_or_else(|| "no playable video or image in this item".into()),
                );
            }
        }
    }

    /// The range stream refused this candidate before playing anything:
    /// try the same file through the file-based player (it downloads a
    /// head and decodes with the file decoder, which knows more codecs).
    fn fall_back_to_file(&mut self, why: String) {
        let Some(identifier) = self.item.as_ref().map(|i| i.identifier.clone()) else { return };
        let Some(file) = self.swatch_candidates.get(self.swatch_index).cloned() else { return };
        makepad_widgets::log!(
            "archive swatch ({identifier}): {} cannot range-stream ({why}); trying the file path",
            file.base_name()
        );
        self.player = None;
        self.swatch_on_file_path = true;
        self.download_gen += 1;
        let gen = self.download_gen;
        self.swatch = Swatch::Loading { loaded: 0, total: Some(file.size) };
        self.worker().send(Cmd::Download { gen, identifier, file, purpose: Purpose::Preview });
    }

    /// One candidate failed: log it and move on to the next.
    fn next_swatch_candidate(&mut self, why: String) {
        let identifier = self.item.as_ref().map(|i| i.identifier.clone()).unwrap_or_default();
        let tried = self
            .swatch_candidates
            .get(self.swatch_index)
            .map(|f| f.base_name().to_string())
            .unwrap_or_default();
        self.swatch_index += 1;
        let remaining = self.swatch_candidates.len().saturating_sub(self.swatch_index);
        makepad_widgets::log!(
            "archive swatch ({identifier}): {tried} failed — {why}; {remaining} candidate(s) left"
        );
        self.start_swatch_candidate(Some(why));
    }

    /// IMPORT: fetch the best file (cache hit when it was the swatch) and
    /// publish it into `target`.
    pub fn import(&mut self, target: PublishTarget) -> Result<(), String> {
        if self.import.busy() {
            return Err("an import is already running".to_string());
        }
        let Some(item) = self.item.as_ref() else {
            return Err("pick a result first".to_string());
        };
        let Some(file) = Self::import_file_of(item).cloned() else {
            return Err(self
                .import_refusal()
                .unwrap_or_else(|| "nothing to import".to_string()));
        };
        let identifier = item.identifier.clone();
        self.download_gen += 1;
        let gen = self.download_gen;
        self.import_gen = Some(gen);
        self.pending_import = Some((target, file.clone()));
        self.import = ImportState::Downloading { loaded: 0, total: Some(file.size) };
        self.worker().send(Cmd::Download { gen, identifier, file, purpose: Purpose::Import });
        Ok(())
    }

    /// Drain the lanes. The host applies each change; `Frame`s only come
    /// while a swatch plays, so a quiet panel costs a `try_recv`.
    pub fn poll(&mut self) -> Vec<ArchiveChange> {
        let mut changes = Vec::new();
        let events = self.worker.as_ref().map(|w| w.poll()).unwrap_or_default();
        for ev in events {
            self.on_event(ev, &mut changes);
        }
        loop {
            let Some(rx) = self.decode_rx.as_ref() else { break };
            match rx.try_recv() {
                Ok(DecodeDone::Thumb { identifier, result }) => match result {
                    Ok(pixels) => {
                        self.thumbs.insert(identifier.clone(), ThumbState::Ready);
                        changes.push(ArchiveChange::Thumb { identifier, pixels });
                    }
                    Err(_) => {
                        self.thumbs.insert(identifier, ThumbState::Failed);
                    }
                },
                Ok(DecodeDone::Still { result }) => match result {
                    Ok(pixels) => {
                        if matches!(self.swatch, Swatch::Loading { .. }) {
                            self.swatch = Swatch::Still;
                            changes.push(ArchiveChange::Still(pixels));
                            changes.push(ArchiveChange::Panel);
                        }
                    }
                    Err(why) => {
                        if matches!(self.swatch, Swatch::Loading { .. }) {
                            self.next_swatch_candidate(format!("still: {why}"));
                            changes.push(ArchiveChange::Panel);
                        }
                    }
                },
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.decode_rx = None;
                    self.decode_tx = None;
                    break;
                }
            }
        }
        if let Some(rx) = self.publish_rx.as_ref() {
            match rx.try_recv() {
                Ok(PublishMsg::Done(result)) => {
                    self.publish_rx = None;
                    self.import = match result {
                        Ok(PublishOutcome::Published(title)) => {
                            makepad_widgets::log!("archive import: published “{title}”");
                            changes.push(ArchiveChange::Published);
                            ImportState::Done(title)
                        }
                        Ok(PublishOutcome::AlreadyPresent) => {
                            makepad_widgets::log!("archive import: already in the library");
                            ImportState::AlreadyPresent
                        }
                        Err(why) => {
                            makepad_widgets::log!("archive import FAILED: {why}");
                            ImportState::Failed(why)
                        }
                    };
                    changes.push(ArchiveChange::Panel);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.publish_rx = None;
                    self.import = ImportState::Failed("publisher died without a verdict".into());
                    changes.push(ArchiveChange::Panel);
                }
            }
        }
        if let Some(player) = self.player.as_ref() {
            if let Some((why, setup)) = player.failure() {
                self.player = None;
                self.swatch_texture = None;
                if setup && !self.swatch_on_file_path {
                    self.fall_back_to_file(why);
                } else {
                    self.next_swatch_candidate(format!("cannot play this file: {why}"));
                }
                changes.push(ArchiveChange::Panel);
            } else {
                let duration = player.duration_secs();
                let buffering_now = player.is_buffering();
                let fetched_now = player.bytes_fetched();
                if let Swatch::Video { duration_secs, buffering, fetched, .. } = &mut self.swatch {
                    if (*duration_secs - duration).abs() > 0.01 && duration > 0.0 {
                        *duration_secs = duration;
                        changes.push(ArchiveChange::Panel);
                    }
                    if *buffering != buffering_now {
                        *buffering = buffering_now;
                        changes.push(ArchiveChange::Panel);
                    }
                    *fetched = fetched_now;
                }
                if let Some((bgra, width, height)) = player.take_frame() {
                    changes.push(ArchiveChange::Frame(Pixels {
                        bgra,
                        width: width as usize,
                        height: height as usize,
                    }));
                }
            }
        }
        changes
    }

    /// A swatch is playing: the host should keep its frame pump armed.
    pub fn wants_frames(&self) -> bool {
        self.player.as_ref().is_some_and(|p| !p.is_paused())
    }

    fn on_event(&mut self, ev: Ev, changes: &mut Vec<ArchiveChange>) {
        match ev {
            Ev::Search { gen, result } => {
                if gen != self.search_gen {
                    return;
                }
                self.searching = false;
                match result {
                    Ok(page) => {
                        self.status = if page.total == 0 {
                            "no results".to_string()
                        } else {
                            format!(
                                "{} result{} · page {} / {}",
                                page.total,
                                if page.total == 1 { "" } else { "s" },
                                page.page,
                                page.pages()
                            )
                        };
                        let epoch = self.thumb_epoch;
                        let ids: Vec<String> =
                            page.hits.iter().map(|h| h.identifier.clone()).collect();
                        self.results = Some(page);
                        for id in ids {
                            self.tiles.insert(Self::tile_key(&id), id.clone());
                            if self.thumb_textures.contains_key(&id) {
                                self.thumbs.insert(id, ThumbState::Ready);
                                continue;
                            }
                            self.thumbs.insert(id.clone(), ThumbState::Requested);
                            self.worker().send(Cmd::Thumb { epoch, identifier: id });
                        }
                    }
                    Err(why) => {
                        self.status = format!("search failed: {why}");
                        self.results = None;
                    }
                }
                changes.push(ArchiveChange::Results);
                changes.push(ArchiveChange::Panel);
            }
            Ev::Thumb { identifier, result } => {
                if self.thumb_textures.contains_key(&identifier) {
                    return;
                }
                match result {
                    Ok(bytes) => {
                        let _ = self.decode_lane().send(DecodeReq::Thumb { identifier, bytes });
                    }
                    // Dropped unstarted (a newer page took its slot): no
                    // mark, so a later page that wants it asks again.
                    Err(ArchiveError::Cancelled) => {
                        self.thumbs.remove(&identifier);
                    }
                    Err(_) => {
                        self.thumbs.insert(identifier, ThumbState::Failed);
                    }
                }
            }
            Ev::Item { gen, result } => {
                if gen != self.item_gen {
                    return;
                }
                match result {
                    Ok(item) => {
                        let identifier = item.identifier.clone();
                        // Videos in try order, then the still as the last
                        // resort — an item is auditioned by whatever plays.
                        let mut candidates: Vec<ItemFile> =
                            item.preview_videos().into_iter().cloned().collect();
                        if let Some(image) = item.primary_image() {
                            candidates.push(image.clone());
                        }
                        makepad_widgets::log!(
                            "archive item {}: {} files, swatch candidates {:?}, import file {:?}",
                            identifier,
                            item.files.len(),
                            candidates
                                .iter()
                                .map(|f| format!("{} [{}] {}", f.base_name(), f.format, human_bytes(f.size)))
                                .collect::<Vec<_>>(),
                            item.import_video_within(makepad_archive_org::MAX_IMPORT_BYTES)
                                .or_else(|| item.primary_image())
                                .map(|f| format!("{} [{}] {}", f.base_name(), f.format, human_bytes(f.size)))
                        );
                        self.item = Some(item);
                        self.swatch_candidates = candidates;
                        self.swatch_index = 0;
                        self.start_swatch_candidate(None);
                        if std::mem::take(&mut self.pending_autocue) {
                            if let Some((url, title, _)) = self.deck_candidate() {
                                changes.push(ArchiveChange::AutoCue { url, title });
                            }
                        }
                    }
                    Err(why) => {
                        self.swatch = Swatch::Failed(format!("{why}"));
                    }
                }
                changes.push(ArchiveChange::Panel);
            }
            Ev::DownloadStarted { gen, purpose, identifier, file, part, total, head } => {
                if purpose != Purpose::Preview || gen != self.download_gen {
                    return;
                }
                if self.selected_hit().map(|h| h.identifier.as_str()) != Some(identifier.as_str()) {
                    return;
                }
                makepad_widgets::log!(
                    "archive swatch: streaming {} ({}, {}{}) of {}",
                    file.base_name(),
                    file.format,
                    human_bytes(file.size),
                    head.map(|h| format!(", first {}", human_bytes(h))).unwrap_or_default(),
                    identifier
                );
                // A video swatch starts NOW, on the growing file; a still
                // waits for its last byte (a half-decoded picture is noise).
                if file.kind() == FileKind::Video {
                    // The finished name the part will take: the whole file,
                    // or the head of it (the worker's naming).
                    let whole = makepad_archive_org::cache_file_for(
                        &self.cache_dir_or_default(),
                        &identifier,
                        &file.name,
                    );
                    let final_path = if head.is_some() {
                        makepad_archive_org::head_file_for(&whole)
                    } else {
                        whole
                    };
                    let player = SwatchPlayer::open(SwatchSource { part: Some(part), final_path });
                    // Paused poster here too — one behavior, whatever the
                    // backend (the download itself continues; it is the
                    // point of this path).
                    player.set_paused(true);
                    self.swatch = Swatch::Video {
                        playing: false,
                        duration_secs: file.length_secs,
                        loading: Some((0, total)),
                        head,
                        buffering: false,
                        fetched: 0,
                    };
                    self.player = Some(SwatchBackend::File(player));
                    changes.push(ArchiveChange::Panel);
                }
            }
            Ev::Progress { gen, purpose, progress } => {
                if gen != self.download_gen {
                    return;
                }
                match purpose {
                    Purpose::Preview => match &mut self.swatch {
                        Swatch::Loading { loaded, total } => {
                            *loaded = progress.loaded;
                            *total = progress.total;
                        }
                        Swatch::Video { loading: Some((loaded, total)), .. } => {
                            *loaded = progress.loaded;
                            *total = progress.total;
                        }
                        _ => {}
                    },
                    Purpose::Import => {
                        if let ImportState::Downloading { loaded, total } = &mut self.import {
                            *loaded = progress.loaded;
                            *total = progress.total;
                        }
                    }
                }
                changes.push(ArchiveChange::Panel);
            }
            Ev::Download { gen, purpose, identifier, file, result } => match purpose {
                Purpose::Preview => {
                    if gen != self.download_gen || Some(gen) == self.import_gen {
                        return;
                    }
                    if self.selected_hit().map(|h| h.identifier.as_str()) != Some(identifier.as_str()) {
                        return;
                    }
                    match result {
                        Ok(path) => match file.kind() {
                            FileKind::Video => {
                                // Streaming swatch already up: the file is
                                // whole now, the player loops it from here.
                                if let Swatch::Video { loading, .. } = &mut self.swatch {
                                    *loading = None;
                                } else {
                                    // A cache hit (no DownloadStarted): the file — or
                                    // its head — is already there.
                                    let head = path
                                        .file_name()
                                        .and_then(|n| n.to_str())
                                        .is_some_and(|n| n.contains(".head."))
                                        .then_some(makepad_archive_org::PREVIEW_HEAD_BYTES);
                                    let player = SwatchPlayer::open(SwatchSource {
                                        part: None,
                                        final_path: path,
                                    });
                                    player.set_paused(true);
                                    self.swatch = Swatch::Video {
                                        playing: false,
                                        duration_secs: file.length_secs,
                                        loading: None,
                                        head,
                                        buffering: false,
                                        fetched: 0,
                                    };
                                    self.player = Some(SwatchBackend::File(player));
                                }
                            }
                            FileKind::Image => {
                                let _ = self.decode_lane().send(DecodeReq::Still { path });
                            }
                            _ => {
                                self.swatch = Swatch::Failed(format!(
                                    "{} is not a playable video or image",
                                    file.base_name()
                                ));
                            }
                        },
                        Err(ArchiveError::Cancelled) => {}
                        Err(why) => {
                            let line = describe_error(&why, &file, Purpose::Preview);
                            self.next_swatch_candidate(line);
                        }
                    }
                    changes.push(ArchiveChange::Panel);
                }
                Purpose::Import => {
                    if Some(gen) != self.import_gen {
                        return;
                    }
                    self.import_gen = None;
                    let Some((target, wanted)) = self.pending_import.take() else { return };
                    match result {
                        Ok(path) if wanted.name == file.name => {
                            let Some(item) = self.item.clone().filter(|i| i.identifier == identifier)
                            else {
                                self.import = ImportState::Failed("selection changed mid-import".into());
                                changes.push(ArchiveChange::Panel);
                                return;
                            };
                            self.import = ImportState::Publishing;
                            let (tx, rx) = mpsc::channel();
                            self.publish_rx = Some(rx);
                            thread::Builder::new()
                                .name("vj-archive-publish".into())
                                .spawn(move || {
                                    let verdict = publish(target, &item, &file, &path);
                                    let _ = tx.send(PublishMsg::Done(verdict));
                                })
                                .ok();
                        }
                        Ok(_) => {
                            self.import = ImportState::Failed("downloaded the wrong file".into());
                        }
                        Err(why) => {
                            let line = describe_error(&why, &file, Purpose::Import);
                            makepad_widgets::log!("archive import download FAILED ({identifier}): {line}");
                            self.import = ImportState::Failed(line);
                        }
                    }
                    changes.push(ArchiveChange::Panel);
                }
            },
        }
    }
}

/// JPEG/PNG bytes → BGRA, bounded.
fn decode_picture(bytes: &[u8], max_dim: usize) -> Result<Pixels, String> {
    let image = if bytes.starts_with(&[0xff, 0xd8]) {
        ImageBuffer::from_jpg(bytes)
    } else if bytes.starts_with(b"\x89PNG") {
        ImageBuffer::from_png(bytes)
    } else {
        return Err("not a jpeg or png".to_string());
    }
    .map_err(|e| format!("decode failed: {e:?}"))?;
    let (w, h) = (image.width, image.height);
    if w == 0 || h == 0 || w > max_dim || h > max_dim {
        return Err(format!("picture dimensions out of bounds: {w}x{h}"));
    }
    let mut data = image.data;
    data.truncate(w * h);
    if data.len() < w * h {
        return Err("short pixel buffer".to_string());
    }
    Ok(Pixels { bgra: data, width: w, height: h })
}

// ---------------------------------------------------------------------------
// The swatch player
// ---------------------------------------------------------------------------

/// Where the swatch's bytes are. `part` is the growing download (opened
/// while it grows); `final_path` is the finished file, which appears —
/// by rename — when the last byte lands.
pub struct SwatchSource {
    pub part: Option<PathBuf>,
    pub final_path: PathBuf,
}

impl SwatchSource {
    /// The finished file once it exists, else the growing one.
    fn path(&self) -> Option<String> {
        if self.final_path.exists() {
            return self.final_path.to_str().map(str::to_string);
        }
        self.part.as_ref().and_then(|p| p.to_str()).map(str::to_string)
    }

    fn growing(&self) -> bool {
        !self.final_path.exists()
    }
}

struct SwatchShared {
    stop: AtomicBool,
    paused: AtomicBool,
    position_100ns: AtomicI64,
    duration_100ns: AtomicI64,
    /// A pending seek target (-1 = none), taken by the decode thread.
    seek_100ns: AtomicI64,
    width: AtomicU32,
    height: AtomicU32,
    /// The newest converted frame, replaced (never queued) so the UI
    /// always shows now.
    frame: Mutex<Option<(Vec<u32>, u32, u32)>>,
    failure: Mutex<Option<String>>,
}

/// One small looping video, decoded on its own thread at the file's pace.
/// No audio, no mixer: the swatch is only ever looked at.
///
/// STREAMING: the thread opens whatever is on disk. A file whose header
/// has not landed yet refuses to open — it retries; a file that ends
/// early because the download is still behind it is the FRONTIER, not
/// the end — the thread waits a beat, reopens (the finished file if it
/// has appeared by then) and seeks back to where it was. Only when the
/// file is whole does an end-of-stream mean "loop".
pub struct SwatchPlayer {
    shared: Arc<SwatchShared>,
}

/// How long the thread waits at the frontier / for the header.
const FRONTIER_WAIT: Duration = Duration::from_millis(250);
/// Bytes that must be on disk before the first open is even tried: the
/// header of a streamable mp4 fits, and a decoder is not asked to sniff
/// an empty file every quarter second.
const MIN_OPEN_BYTES: u64 = 256 * 1024;

impl SwatchPlayer {
    pub fn open(source: SwatchSource) -> SwatchPlayer {
        let shared = Arc::new(SwatchShared {
            stop: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            position_100ns: AtomicI64::new(0),
            duration_100ns: AtomicI64::new(0),
            seek_100ns: AtomicI64::new(-1),
            width: AtomicU32::new(0),
            height: AtomicU32::new(0),
            frame: Mutex::new(None),
            failure: Mutex::new(None),
        });
        let thread_shared = shared.clone();
        if let Err(e) = thread::Builder::new()
            .name("vj-archive-swatch".into())
            .spawn(move || swatch_loop(source, thread_shared))
        {
            *shared.failure.lock().unwrap() = Some(e.to_string());
        }
        SwatchPlayer { shared }
    }

    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Release);
    }

    pub fn is_paused(&self) -> bool {
        self.shared.paused.load(Ordering::Acquire)
    }

    pub fn position_secs(&self) -> f64 {
        self.shared.position_100ns.load(Ordering::Acquire) as f64 / 10_000_000.0
    }

    pub fn duration_secs(&self) -> f64 {
        self.shared.duration_100ns.load(Ordering::Acquire).max(0) as f64 / 10_000_000.0
    }

    pub fn seek_fraction(&self, fraction: f64) {
        let duration = self.shared.duration_100ns.load(Ordering::Acquire);
        if duration <= 0 {
            return;
        }
        let target = (fraction.clamp(0.0, 1.0) * duration as f64) as i64;
        self.shared.seek_100ns.store(target.max(0), Ordering::Release);
    }

    pub fn take_frame(&self) -> Option<(Vec<u32>, u32, u32)> {
        self.shared.frame.lock().ok()?.take()
    }

    pub fn failure(&self) -> Option<String> {
        self.shared.failure.lock().ok()?.clone()
    }
}

impl Drop for SwatchPlayer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
    }
}

fn file_len(path: &str) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn swatch_loop(source: SwatchSource, shared: Arc<SwatchShared>) {
    let mut decoder: Option<VideoFileDecoder> = None;
    let mut origin = Instant::now();
    let mut base_100ns: i64 = -1;
    // The last frame shown; a reopen seeks back to it and skips what it
    // already showed.
    let mut last_100ns: i64 = -1;
    let mut bgra: Vec<u32> = Vec::new();
    // Show ONE frame even while paused: the poster after an open, and the
    // scrubbed frame after a seek.
    let mut poster = true;
    loop {
        if shared.stop.load(Ordering::Acquire) {
            return;
        }
        // A pause holds the picture and the clock alike: the origin moves
        // forward with the wall clock so resuming does not fast-forward.
        // The park waits for the poster frame first, and a pending seek
        // breaks it so a paused scrub still shows where it landed.
        while shared.paused.load(Ordering::Acquire)
            && !poster
            && shared.seek_100ns.load(Ordering::Acquire) < 0
        {
            if shared.stop.load(Ordering::Acquire) {
                return;
            }
            thread::sleep(Duration::from_millis(20));
            origin += Duration::from_millis(20);
        }
        let seek = shared.seek_100ns.swap(-1, Ordering::AcqRel);
        if seek >= 0 {
            poster = true;
            if let Some(d) = decoder.as_mut() {
                if d.seek(seek).is_ok() {
                    last_100ns = seek - 1;
                    base_100ns = -1;
                } else {
                    // Reopen below at the new position.
                    decoder = None;
                    last_100ns = seek - 1;
                    base_100ns = -1;
                }
            } else {
                last_100ns = seek - 1;
            }
        }
        if decoder.is_none() {
            let Some(path) = source.path() else {
                *shared.failure.lock().unwrap() = Some("no swatch file".into());
                return;
            };
            if source.growing() && file_len(&path) < MIN_OPEN_BYTES {
                thread::sleep(FRONTIER_WAIT);
                continue;
            }
            match VideoFileDecoder::open(&path) {
                Ok(d) => {
                    let info = d.info().clone();
                    shared.width.store(info.width, Ordering::Release);
                    shared.height.store(info.height, Ordering::Release);
                    shared.duration_100ns.store(info.duration_100ns, Ordering::Release);
                    let mut d = d;
                    if last_100ns >= 0 {
                        let _ = d.seek(last_100ns + 1);
                    }
                    decoder = Some(d);
                    base_100ns = -1;
                }
                Err(e) => {
                    if source.growing() {
                        // Header not in yet (or a non-streamable file whose
                        // index is at the end): wait for more bytes. Give
                        // up only if the whole file arrived and still will
                        // not open — that is handled on the next pass,
                        // when `growing()` turns false.
                        thread::sleep(FRONTIER_WAIT);
                        continue;
                    }
                    *shared.failure.lock().unwrap() = Some(e.to_string());
                    return;
                }
            }
        }
        let Some(d) = decoder.as_mut() else { continue };
        match d.next_frame() {
            Ok(Some(frame)) => {
                if frame.pts_100ns <= last_100ns {
                    // Already shown before a reopen: skip to the frontier.
                    continue;
                }
                if base_100ns < 0 {
                    base_100ns = frame.pts_100ns;
                    origin = Instant::now();
                }
                let due = Duration::from_nanos(((frame.pts_100ns - base_100ns).max(0) as u64) * 100);
                // Behind by more than a beat: skip the conversion and catch
                // up on the next frame rather than sliding ever later. The
                // poster frame is never skipped — it IS the picture.
                if !poster && origin.elapsed() > due + Duration::from_millis(120) {
                    last_100ns = frame.pts_100ns;
                    continue;
                }
                // Saturating: the clock can cross `due` between the test
                // and the subtraction, and a bare `-` panics the thread.
                loop {
                    let remaining = due.saturating_sub(origin.elapsed());
                    if remaining.is_zero() || shared.stop.load(Ordering::Acquire) {
                        break;
                    }
                    thread::sleep(remaining.min(Duration::from_millis(4)));
                }
                if shared.stop.load(Ordering::Acquire) {
                    return;
                }
                nv12::nv12_to_bgra_u32(&frame.nv12, frame.width, frame.height, &mut bgra);
                shared.position_100ns.store(frame.pts_100ns, Ordering::Release);
                last_100ns = frame.pts_100ns;
                if let Ok(mut slot) = shared.frame.lock() {
                    *slot = Some((std::mem::take(&mut bgra), frame.width, frame.height));
                }
                poster = false;
            }
            Ok(None) | Err(_) if source.growing() => {
                // The download frontier: wait for bytes, then reopen and
                // resume after the last frame shown. A frontier that never
                // moves (a stalled download) is not a hang here — the
                // panel's own download failure ends the swatch.
                decoder = None;
                thread::sleep(FRONTIER_WAIT);
                // The clock restarts at the resume point.
                base_100ns = -1;
            }
            Ok(None) => {
                // A real end: loop from the head.
                last_100ns = -1;
                base_100ns = -1;
                if d.seek(0).is_err() {
                    decoder = None;
                }
            }
            Err(e) => {
                *shared.failure.lock().unwrap() = Some(e.to_string());
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The import publisher
// ---------------------------------------------------------------------------

fn media_of(file: &ItemFile) -> Option<(AssetKind, FileRole, MediaType, &'static str)> {
    match file.ext().as_str() {
        "mp4" | "m4v" | "mov" => Some((AssetKind::Video, FileRole::Video, MediaType::Mp4, "video")),
        "jpg" | "jpeg" => Some((AssetKind::Texture, FileRole::Texture, MediaType::Jpeg, "image")),
        "png" => Some((AssetKind::Texture, FileRole::Texture, MediaType::Png, "image")),
        _ => None,
    }
}

fn hex8(bytes: &[u8]) -> String {
    let digest = makepad_network_sha256(bytes);
    digest[..4].iter().map(|b| format!("{b:02x}")).collect()
}

fn makepad_network_sha256(bytes: &[u8]) -> [u8; 32] {
    makepad_widgets::makepad_platform::makepad_network::digest::sha256_hash(bytes)
}

/// `archive/<item>/<file>-<hex8>`: one alias per (item, file), so a second
/// IMPORT of the same clip finds it instead of duplicating it.
pub fn alias_for(identifier: &str, file: &ItemFile) -> Option<AssetAlias> {
    let text = format!(
        "{NAMESPACE}/{}/{}-{}",
        slug(identifier, 40),
        slug(file.base_name(), 30),
        hex8(format!("{identifier}/{}", file.name).as_bytes())
    );
    AssetAlias::new(text).ok()
}

fn grant_to_redistribution(grant: Grant) -> Redistribution {
    match grant {
        Grant::Allowed => Redistribution::Allowed,
        Grant::AttributionRequired => Redistribution::AttributionRequired,
        // Nothing declared: nothing is cleared. The catalog carries this
        // forever inside an immutable revision, so it says only what the
        // archive said.
        Grant::Forbidden | Grant::Unknown => Redistribution::Forbidden,
    }
}

fn grant_to_derivatives(grant: Grant) -> DerivativePolicy {
    match grant {
        Grant::Allowed => DerivativePolicy::Allowed,
        Grant::AttributionRequired => DerivativePolicy::AttributionRequired,
        Grant::Forbidden => DerivativePolicy::Forbidden,
        // Unknown: like the operator's own library — usable in a set,
        // not cleared for redistribution.
        Grant::Unknown => DerivativePolicy::Allowed,
    }
}

/// The rights record an archive item carries into the store.
///
/// Attribution licenses need a credit line and many items name no
/// creator; the credit then names the work and its archive page, which is
/// what CC attribution asks for at minimum (title + source).
pub fn rights_for(item: &Item) -> PublishRights {
    let license = license_from_url(&item.license_url);
    let creator = clean_text(&item.creator, 200);
    let credits = if creator.is_empty() {
        format!("{} ({})", clean_text(&item.title, 200), details_url(&item.identifier))
    } else {
        creator.clone()
    };
    PublishRights {
        license: license.id.clone(),
        license_revision: String::new(),
        terms_digest: None,
        terms_url: license.url.clone(),
        credits,
        source: details_url(&item.identifier),
        source_archive: None,
        redistribution: grant_to_redistribution(license.redistribution),
        derivatives: grant_to_derivatives(license.derivatives),
    }
}

/// Tags: the lane, the item, its media type and its subjects, so the
/// archive's own keywords are searchable the moment a clip lands.
pub fn tags_for(item: &Item) -> Vec<String> {
    let mut tags = vec![
        "archive".to_string(),
        "archiveorg".to_string(),
        slug(&item.identifier, 32),
        match item.mediatype {
            makepad_archive_org::ItemMediaType::Movies => "movies".to_string(),
            makepad_archive_org::ItemMediaType::Image => "image".to_string(),
            makepad_archive_org::ItemMediaType::Audio => "audio".to_string(),
            makepad_archive_org::ItemMediaType::Texts => "texts".to_string(),
            makepad_archive_org::ItemMediaType::Other(ref s) => slug(s, 24),
        },
    ];
    for subject in item.subject.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).take(8) {
        let tag = slug(subject, 32);
        if !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    tags
}

fn publish(target: PublishTarget, item: &Item, file: &ItemFile, path: &Path) -> Result<PublishOutcome, String> {
    let (kind, role, media, class) = media_of(file).ok_or("not an importable file type")?;
    let alias = alias_for(&item.identifier, file).ok_or("cannot derive an alias")?;
    let mut config = ClientConfig::new(target.cache.join("archive-import-cache"));
    config.token = Some(target.token);
    let mut client = AssetClient::connect(config, target.endpoints, Some(target.server_id))
        .map_err(|e| format!("asset server: {e}"))?;
    match client.resolve_alias(&alias) {
        Ok(_) => return Ok(PublishOutcome::AlreadyPresent),
        Err(ClientError::NotFound { .. }) => {}
        Err(error) => return Err(format!("alias probe: {error}")),
    }
    let bytes = std::fs::read(path).map_err(|e| format!("read: {e}"))?;
    let (thumbnail, media_millis, dims) = match kind {
        AssetKind::Video => {
            let probe = probe_video(path)?;
            (
                PublishThumbnail::plain(
                    probe.thumbnail_jpeg,
                    ThumbnailMedia::Jpeg,
                    thumbs::THUMB_DIM as u32,
                    thumbs::THUMB_DIM as u32,
                ),
                probe.duration_ms,
                None,
            )
        }
        _ => {
            let dims = thumbs::png_dims(&bytes)
                .or_else(|| thumbs::jpeg_dims(&bytes))
                .ok_or("unreadable image header")?;
            let thumb = match makepad_asset_importer::import::usable_image_thumb(&bytes) {
                Some((thumb, media, w, h)) => PublishThumbnail::plain(thumb, media, w, h),
                None => {
                    let jpeg = thumbs::encode_jpeg_bgra(
                        &thumbs::placeholder_bgra_512(),
                        thumbs::THUMB_DIM,
                        thumbs::THUMB_DIM,
                    )?;
                    PublishThumbnail::plain(
                        jpeg,
                        ThumbnailMedia::Jpeg,
                        thumbs::THUMB_DIM as u32,
                        thumbs::THUMB_DIM as u32,
                    )
                }
            };
            (thumb, 0, Some(dims))
        }
    };
    // Only a picture declares pixel dims in the manifest — the store
    // refuses them on video (the archive's own width/height for a clip
    // stays in the item metadata, not on the file).
    let dims = if kind == AssetKind::Texture { dims.or_else(|| file.dims()) } else { None };
    let title = clean_text(&item.title, 200);
    let mut bundle = PublishBundle::new(
        NAMESPACE,
        kind,
        if title.is_empty() { item.identifier.clone() } else { title.clone() },
        vec![PublishBundleFile::bytes(role, media, bytes, dims)],
        thumbnail,
        rights_for(item),
    );
    bundle.alias = Some(alias);
    bundle.media_millis = media_millis;
    bundle.categories = vec!["imported".to_string(), class.to_string(), "archive".to_string()];
    bundle.tags = tags_for(item);
    bundle.creator = clean_text(&item.creator, 200);
    bundle.generator = "makepad-vj archive.org import".to_string();
    bundle.provenance = file.download_url(&item.identifier);
    bundle.description = clean_text(&item.description, 2000);
    client
        .publish_bundle(&bundle)
        .map(|_| PublishOutcome::Published(bundle.title.clone()))
        .map_err(|e| format!("{e}"))
}

/// A refusal the operator can act on: the size and the limit, not "over
/// the size limit".
fn describe_error(error: &ArchiveError, file: &ItemFile, purpose: Purpose) -> String {
    match error {
        ArchiveError::TooLarge => format!(
            "{} is {} — over the {} {} limit",
            file.base_name(),
            human_bytes(file.size),
            human_bytes(purpose.max_bytes()),
            match purpose {
                Purpose::Preview => "swatch",
                Purpose::Import => "import",
            }
        ),
        other => format!("{other}"),
    }
}

/// One line of store-safe text: the archive's descriptions carry
/// newlines, tabs and the odd control byte, and the store refuses control
/// characters in any annotation. Whitespace runs collapse to one space;
/// the result is bounded at `max` chars on a char boundary.
pub fn clean_text(text: &str, max: usize) -> String {
    let mut out = String::with_capacity(text.len().min(max));
    let mut space = true;
    for c in text.chars() {
        if c.is_control() || c.is_whitespace() {
            if !space {
                out.push(' ');
                space = true;
            }
        } else {
            out.push(c);
            space = false;
        }
        if out.chars().count() >= max {
            break;
        }
    }
    out.trim_end().to_string()
}

/// `1.2 MB` / `640×360` / `9:56` — the meta line under the well.
pub fn human_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= KB * KB * KB {
        format!("{:.1} GB", b / (KB * KB * KB))
    } else if b >= KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

pub fn human_secs(secs: f64) -> String {
    let total = secs.max(0.0).round() as u64;
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_archive_org::{FileSource, ItemMediaType};

    fn file(name: &str) -> ItemFile {
        ItemFile {
            name: name.into(),
            source: FileSource::Original,
            format: String::new(),
            size: 10,
            width: 0,
            height: 0,
            length_secs: 0.0,
            md5: String::new(),
        }
    }

    fn item(license: &str) -> Item {
        Item {
            identifier: "BigBuckBunny_124".into(),
            title: "Big Buck Bunny".into(),
            description: String::new(),
            creator: "Blender Foundation".into(),
            date: String::new(),
            mediatype: ItemMediaType::Movies,
            license_url: license.into(),
            subject: "animation, bunny, , animation".into(),
            files: Vec::new(),
        }
    }

    #[test]
    fn alias_is_stable_and_bounded() {
        let a = alias_for("BigBuckBunny_124", &file("Content/big_buck_bunny_720p_surround.mp4")).unwrap();
        let b = alias_for("BigBuckBunny_124", &file("Content/big_buck_bunny_720p_surround.mp4")).unwrap();
        assert_eq!(a, b);
        let c = alias_for("BigBuckBunny_124", &file("Content/other.mp4")).unwrap();
        assert_ne!(a, c);
        assert!(a.as_str().starts_with("archive/bigbuckbunny_124/"));
        let long = alias_for(&"x".repeat(100), &file(&format!("{}.mp4", "y".repeat(200)))).unwrap();
        assert!(long.as_str().len() <= 128);
    }

    #[test]
    fn rights_follow_the_license_honestly() {
        let r = rights_for(&item("https://creativecommons.org/licenses/by/3.0/"));
        assert_eq!(r.license, "CC-BY-3.0");
        assert_eq!(r.redistribution, Redistribution::AttributionRequired);
        assert_eq!(r.derivatives, DerivativePolicy::AttributionRequired);
        assert_eq!(r.source, "https://archive.org/details/BigBuckBunny_124");
        assert_eq!(r.credits, "Blender Foundation");
        let mut anonymous = item("https://creativecommons.org/licenses/by/3.0/");
        anonymous.creator = String::new();
        assert_eq!(
            rights_for(&anonymous).credits,
            "Big Buck Bunny (https://archive.org/details/BigBuckBunny_124)"
        );
        let r = rights_for(&item(""));
        assert_eq!(r.license, "LicenseRef-Archive-Org-Unspecified");
        assert_eq!(r.redistribution, Redistribution::Forbidden);
        assert_eq!(r.derivatives, DerivativePolicy::Allowed);
        let r = rights_for(&item("https://creativecommons.org/licenses/by-nd/4.0/"));
        assert_eq!(r.derivatives, DerivativePolicy::Forbidden);
    }

    #[test]
    fn tags_carry_subjects_once() {
        let tags = tags_for(&item(""));
        assert_eq!(tags, vec!["archive", "archiveorg", "bigbuckbunny_124", "movies", "animation", "bunny"]);
    }

    #[test]
    fn media_classes() {
        assert_eq!(media_of(&file("a.MP4")).unwrap().0, AssetKind::Video);
        assert_eq!(media_of(&file("a.mov")).unwrap().2, MediaType::Mp4);
        assert_eq!(media_of(&file("a.png")).unwrap().2, MediaType::Png);
        assert!(media_of(&file("a.ogv")).is_none());
    }

    #[test]
    fn text_is_store_safe() {
        assert_eq!(clean_text("  Elephants\tDream\n\nby  Orange\u{0}\r", 100), "Elephants Dream by Orange");
        assert_eq!(clean_text("abcdef", 4), "abcd");
        assert_eq!(clean_text("", 10), "");
        let r = rights_for(&Item { creator: "A\nB".into(), ..item("") });
        assert_eq!(r.credits, "A B");
    }

    #[test]
    fn import_candidate_never_swaps_a_film_for_its_cover() {
        let big = ItemFile { size: makepad_archive_org::MAX_IMPORT_BYTES + 1, ..file("ep1.mp4") };
        let cover = file("cover.jpg");
        let mut panel = ArchivePanel::default();
        panel.item = Some(Item { files: vec![big.clone(), cover.clone()], ..item("") });
        assert!(panel.import_candidate().is_none());
        assert!(panel.import_refusal().unwrap().contains("over the"));
        let small = ItemFile { size: 10, format: "h.264".into(), ..file("ep1_512kb.mp4") };
        panel.item = Some(Item { files: vec![big, small, cover.clone()], ..item("") });
        assert_eq!(panel.import_candidate().unwrap().name, "ep1_512kb.mp4");
        assert!(panel.import_refusal().is_none());
        panel.item = Some(Item { files: vec![cover], ..item("") });
        assert_eq!(panel.import_candidate().unwrap().name, "cover.jpg");
    }

    #[test]
    fn readouts() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(61878609), "59.0 MB");
        assert_eq!(human_secs(596.5), "9:57");
        assert_eq!(human_secs(3601.0), "1:00:01");
    }

    #[test]
    fn tile_keys_round_trip_through_the_panel() {
        let mut panel = ArchivePanel::default();
        let key = ArchivePanel::tile_key("apple-fukkireta");
        panel.tiles.insert(key, "apple-fukkireta".into());
        assert_eq!(panel.identifier_for(key), Some("apple-fukkireta"));
        assert_eq!(panel.identifier_for(ArchivePanel::tile_key("other")), None);
    }
}
