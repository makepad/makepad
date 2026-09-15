//! Background artifact IO: one viewer/thumb lane, a LIFO gallery decode pool.
//!
//! Gallery clicks and thumbnail regeneration used to read (and for video/
//! splat REWRITE) whole payloads on the UI thread — a 100 MB world splat
//! froze the click. Viewer opens stay latest-selection-wins on a single
//! lane. Gallery-card previews are a stack: last requested pops first, and
//! several cores inflate PNGs at once so scrolling does not wait on cards
//! that already left the viewport.
//!
//! - at most one viewer read in flight; while it runs, only the NEWEST
//!   click waits behind it ([`ViewerOpenGate`], latest-selection-wins);
//! - at most one thumbnail-source read in flight on the same lane;
//! - gallery preview decodes run on a small worker pool (2..=8 threads)
//!   pulling a last-in-first-out stack, capped so old off-screen work drops.

use makepad_widgets::makepad_platform::thread::{SignalToUI, ThreadOptions, ThreadSpawner};
use makepad_widgets::{decode_image_from_data, ImageBuffer};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};

/// What one background read is for.
pub enum IoPurpose {
    /// Viewer open of a gallery selection. `generation` implements
    /// latest-selection-wins; `copy_to` pre-writes video/splat payloads into
    /// the artifacts dir so the UI thread never rewrites a payload on click.
    ViewerOpen {
        generation: u64,
        copy_to: Option<PathBuf>,
    },
    /// GLB payload feed for the offscreen model-thumbnail renderer.
    ThumbModel,
    /// WAV payload → waveform-strip PNG, parsed and encoded ON the worker.
    ThumbAudioWaveform,
    /// Gallery-card preview: read + DECODE an encoded image (sidecar, a
    /// store thumbnail object, or an image payload that is its own preview)
    /// on the worker, so card draws never touch the filesystem or a PNG
    /// decoder.
    ///
    /// `views` is the manifest's DECLARATION of what the picture is
    /// (`ThumbnailMeta::views`) — the shared `makepad_asset_widgets::thumb`
    /// interpreter obeys it. Empty means the manifest declared nothing; the
    /// picture's own stamped layout (a declaration too, written by the
    /// packer) is then consulted, and failing that the whole image draws as
    /// one still. Dimensions are NEVER measured to decide what a picture
    /// means.
    GalleryPreviewEncoded {
        views: Vec<makepad_asset_data::ThumbnailView>,
    },
    /// Gallery-card preview for a legacy sidecarless WAV: read + parse +
    /// min/max scan on the worker.
    GalleryPreviewWav,
    /// Stateful billboard: decode the preview state's native-size frames.
    GalleryPreviewBillboard,
    /// Materialise a catalog asset's THUMBNAIL into the client's verified
    /// cache and hand back its path. One per rail card, so it must stay
    /// small: a wall of cards never pulls payloads.
    CatalogThumb,
    /// Materialise a catalog asset's PAYLOAD into the client's verified
    /// cache and hand back its path — what the tools that take a file (AO
    /// bake, rig, drag-out) work from. Requested when an asset is actually
    /// used, never per card.
    CatalogPayload,
    /// Materialise a catalog asset's SOURCE-role file. A stateful billboard
    /// is one asset carrying two blobs — the packed sheet (`Texture`, which
    /// is the payload) and the `.billboard` manifest that says which cells
    /// are which state and which rotation. A viewer that wants to PLAY the
    /// actor needs both; a viewer that only draws its picture never asks.
    CatalogSource,
}

/// Decoded preview pixels, ready for a cheap UI-thread texture upload.
#[derive(Clone)]
pub enum PreviewPixels {
    /// Full decoded image (keeps the stock mipmapping path).
    Encoded(ImageBuffer),
    /// Raw BGRA rows (waveform strips — no mip chain, same as before).
    Raw {
        width: usize,
        height: usize,
        data: Vec<u32>,
    },
}

pub struct IoRequest {
    pub file: String,
    pub path: PathBuf,
    pub purpose: IoPurpose,
    /// Where the bytes come from. `None` = the `path` above (drops, staged
    /// files, the retiring local library). `Some` = the asset store, which
    /// is the direction of travel: the app is a thin client over the
    /// catalog, so a viewer open resolves an asset to its head revision and
    /// streams the blob instead of trusting a local copy that can go stale.
    pub store: Option<StoreSource>,
}

/// One store-resolved read: which asset, which roles are drawable, and the
/// session to fetch with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreSource {
    pub asset: makepad_asset_data::AssetId,
    /// Roles a viewer can draw, best first (`RenderGlb`, then `Splat`, …).
    pub prefer: Vec<makepad_asset_data::FileRole>,
    pub session: crate::import::ServerSession,
}

pub enum IoDone {
    ViewerOpen {
        file: String,
        generation: u64,
        /// The pre-written artifacts-dir copy, when the read succeeded and a
        /// copy was requested.
        copy_to: Option<PathBuf>,
        bytes: Result<Vec<u8>, String>,
    },
    ThumbModel {
        file: String,
        bytes: Result<Vec<u8>, String>,
    },
    ThumbAudioWaveform {
        file: String,
        png: Option<Vec<u8>>,
    },
    /// Decoded gallery preview. `cache_source` is the cache-validation key
    /// the widgets compare against `GalleryEntry::preview_path` (`Some` for
    /// encoded sources, `None` for payload-derived waveforms); `None` pixels
    /// = the source failed to read/decode (pin a badge, don't loop).
    GalleryPreview {
        file: String,
        cache_source: Option<PathBuf>,
        pixels: Option<PreviewPixels>,
        sequence: Vec<PreviewPixels>,
        fps: f32,
    },
    /// A catalog asset materialised to a verified cache path. `payload` is
    /// false for the thumbnail lane. The path is digest-named, so holding
    /// it is not holding a copy: it is a pointer at content the revision
    /// still names.
    CatalogFile {
        file: String,
        payload: bool,
        role: Option<makepad_asset_data::FileRole>,
        /// The container the manifest declares. The role says what a file is
        /// FOR; this says what it IS, and the two are not the same question
        /// for audio (WAV/Ogg/MP3) or video.
        media: Option<makepad_asset_data::MediaType>,
        revision: Option<String>,
        path: Result<PathBuf, String>,
        /// The manifest's declared thumbnail views, carried with the path so
        /// the decode that follows obeys the declaration.
        views: Vec<makepad_asset_data::ThumbnailView>,
    },
    /// A catalog asset's SOURCE-role file, materialised the same way. Its
    /// own variant rather than a flag on `CatalogFile`: it is a SECOND file
    /// of the same asset, and the payload slot must not be overwritten by
    /// it — the sheet and the manifest are both needed at once.
    CatalogSource {
        file: String,
        path: Result<PathBuf, String>,
    },
}

pub struct ArtifactIo {
    tx: Sender<IoRequest>,
    rx: Receiver<IoDone>,
}

impl ArtifactIo {
    pub fn start(spawner: ThreadSpawner) -> Self {
        let (request_tx, request_rx) = channel::<IoRequest>();
        let (done_tx, done_rx) = channel::<IoDone>();
        let (gallery_tx, gallery_rx) = channel::<IoRequest>();
        spawner
            .spawn_worker(
                ThreadOptions {
                    name: Some("asset-ui-artifact-io".into()),
                    ..Default::default()
                },
                {
                    let done_tx = done_tx.clone();
                    move || dispatch_loop(request_rx, done_tx, gallery_tx)
                },
            )
            .expect("artifact io dispatcher")
            .detach();
        spawner
            .spawn_worker(
                ThreadOptions {
                    name: Some("asset-ui-preview".into()),
                    ..Default::default()
                },
                move || gallery_loop(gallery_rx, done_tx),
            )
            .expect("gallery decode worker")
            .detach();
        Self {
            tx: request_tx,
            rx: done_rx,
        }
    }

    pub fn request(&self, request: IoRequest) {
        let _ = self.tx.send(request);
    }

    /// Non-blocking: everything the worker finished since the last drain.
    pub fn drain(&self) -> Vec<IoDone> {
        self.rx.try_iter().collect()
    }
}

fn is_gallery(purpose: &IoPurpose) -> bool {
    matches!(
        purpose,
        IoPurpose::GalleryPreviewEncoded { .. }
            | IoPurpose::GalleryPreviewWav
            | IoPurpose::GalleryPreviewBillboard
    )
}

fn dispatch_loop(rx: Receiver<IoRequest>, tx: Sender<IoDone>, gallery: Sender<IoRequest>) {
    // One connected client per session, reused across opens: against the
    // local server a fresh connect costs more than the payload.
    let mut store: Option<(crate::import::ServerSession, makepad_asset_client::AssetClient)> = None;
    while let Ok(request) = rx.recv() {
        if is_gallery(&request.purpose) {
            let _ = gallery.send(request);
            continue;
        }
        let done = process_with_store(request, &mut store);
        if tx.send(done).is_err() {
            return;
        }
        SignalToUI::set_ui_signal();
    }
}

fn gallery_loop(rx: Receiver<IoRequest>, tx: Sender<IoDone>) {
    let mut gallery = GalleryStack::new();
    while let Ok(request) = rx.recv() {
        gallery.push_latest(request);
        for request in rx.try_iter() {
            gallery.push_latest(request);
        }
        while let Some(request) = gallery.pop_latest() {
            let file = request.file.clone();
            let done = process(request);
            gallery.finish(&file);
            if tx.send(done).is_err() {
                return;
            }
            SignalToUI::set_ui_signal();
            for request in rx.try_iter() {
                gallery.push_latest(request);
            }
        }
    }
}

/// Last-requested-first stack for gallery PNG inflate. A re-request of a
/// file already waiting is moved to the top (the card the user just
/// scrolled onto wins). Oldest waiting work past [`GALLERY_STACK_CAP`]
/// is dropped — those cards will re-record a miss on the next draw.
const GALLERY_STACK_CAP: usize = 96;

struct GalleryInner {
    stack: Vec<IoRequest>,
    decoding: HashSet<String>,
    shutdown: bool,
}

struct GalleryStack {
    inner: GalleryInner,
}

impl GalleryStack {
    fn new() -> Self {
        Self {
            inner: GalleryInner {
                stack: Vec::new(),
                decoding: HashSet::new(),
                shutdown: false,
            },
        }
    }

    fn push_latest(&mut self, request: IoRequest) {
        let g = &mut self.inner;
        if g.shutdown {
            return;
        }
        if g.decoding.contains(&request.file) {
            return;
        }
        g.stack.retain(|queued| queued.file != request.file);
        g.stack.push(request);
        if g.stack.len() > GALLERY_STACK_CAP {
            let drop_n = g.stack.len() - GALLERY_STACK_CAP;
            g.stack.drain(0..drop_n);
        }
    }

    fn pop_latest(&mut self) -> Option<IoRequest> {
        let g = &mut self.inner;
        if g.shutdown {
            return None;
        }
        while let Some(request) = g.stack.pop() {
            if g.decoding.insert(request.file.clone()) {
                return Some(request);
            }
        }
        None
    }

    fn finish(&mut self, file: &str) {
        self.inner.decoding.remove(file);
    }

    fn shutdown(&mut self) {
        self.inner.shutdown = true;
    }
}

/// A read whose bytes may come from the store. Everything past the fetch is
/// the same decode the local-file path uses — one viewer, two sources.
fn process_with_store(
    request: IoRequest,
    store: &mut Option<(crate::import::ServerSession, makepad_asset_client::AssetClient)>,
) -> IoDone {
    let Some(source) = request.store.clone() else {
        return process(request);
    };
    if matches!(
        request.purpose,
        IoPurpose::CatalogThumb | IoPurpose::CatalogPayload | IoPurpose::CatalogSource
    ) {
        return materialize_from_store(store, &request, &source);
    }
    let bytes = fetch_from_store(store, &source);
    match request.purpose {
        IoPurpose::ViewerOpen { generation, copy_to } => {
            let copy_to = match (&bytes, copy_to) {
                (Ok(bytes), Some(target)) => std::fs::write(&target, bytes)
                    .map(|_| target)
                    .map_err(|error| format!("viewer copy failed: {error}"))
                    .ok(),
                _ => None,
            };
            IoDone::ViewerOpen {
                file: request.file,
                generation,
                copy_to,
                bytes,
            }
        }
        IoPurpose::ThumbModel => IoDone::ThumbModel {
            bytes,
            file: request.file,
        },
        IoPurpose::CatalogThumb | IoPurpose::CatalogPayload | IoPurpose::CatalogSource => {
            unreachable!("catalog materialisation never reads bytes first")
        }
        // Store-sourced reads only serve the viewer and the model
        // thumbnailer today; anything else falls back to the path.
        _ => process(IoRequest {
            store: None,
            ..request
        }),
    }
}

/// Resolve the asset to its head revision and put the wanted file in the
/// client's verified cache, returning the path. Nothing is copied into an
/// app-owned location: the object is named by its digest and re-hashed
/// before the path is handed out, so a tool reading it is reading exactly
/// what the revision names.
fn materialize_from_store(
    store: &mut Option<(crate::import::ServerSession, makepad_asset_client::AssetClient)>,
    request: &IoRequest,
    source: &StoreSource,
) -> IoDone {
    let payload = matches!(request.purpose, IoPurpose::CatalogPayload);
    let want_source = matches!(request.purpose, IoPurpose::CatalogSource);
    let outcome = client_for(store, source).and_then(|client| {
        if want_source {
            crate::store_content::materialize(
                client,
                &source.asset,
                &[makepad_asset_data::FileRole::Source],
            )
        } else if payload {
            crate::store_content::materialize(client, &source.asset, &source.prefer)
        } else {
            crate::store_content::materialize_thumbnail(client, &source.asset)
        }
    });
    if want_source {
        return IoDone::CatalogSource {
            file: request.file.clone(),
            path: outcome.map(|file| file.path),
        };
    }
    match outcome {
        Ok(file) => IoDone::CatalogFile {
            file: request.file.clone(),
            payload,
            role: Some(file.role),
            media: Some(file.media),
            revision: Some(file.revision),
            path: Ok(file.path),
            views: file.views,
        },
        Err(error) => IoDone::CatalogFile {
            file: request.file.clone(),
            payload,
            role: None,
            media: None,
            revision: None,
            path: Err(error),
            views: Vec::new(),
        },
    }
}

/// Resolve the asset to its head revision's drawable file and stream it.
/// The client verifies every byte against the digest and holds it in its
/// budgeted RAM cache — the app keeps no copy of its own.
fn fetch_from_store(
    store: &mut Option<(crate::import::ServerSession, makepad_asset_client::AssetClient)>,
    source: &StoreSource,
) -> Result<Vec<u8>, String> {
    let client = client_for(store, source)?;
    crate::store_content::fetch_viewable(client, &source.asset, &source.prefer)
        .map(|payload| payload.bytes)
}

/// The connected client for this request's session, reconnecting only when
/// the session actually changed. One client owns the verified cache, so
/// every lane above shares one set of on-disk objects.
fn client_for<'a>(
    store: &'a mut Option<(crate::import::ServerSession, makepad_asset_client::AssetClient)>,
    source: &StoreSource,
) -> Result<&'a mut makepad_asset_client::AssetClient, String> {
    let fresh = match store {
        Some((session, _)) => session.endpoints != source.session.endpoints
            || session.server_id != source.session.server_id
            || session.token != source.session.token,
        None => true,
    };
    if fresh {
        let cache = crate::asset_store_state::instance_cache_parent().join("store-cache");
        let client = crate::store_content::connect(&source.session, &cache)
            .ok_or("cannot reach the asset server")?;
        *store = Some((source.session.clone(), client));
    }
    Ok(&mut store.as_mut().expect("connected above").1)
}

fn process(request: IoRequest) -> IoDone {
    match request.purpose {
        // Catalog materialisation has no local form: without a session
        // there is no asset to resolve, and inventing a path would be
        // exactly the stale local copy this lane exists to remove.
        IoPurpose::CatalogSource => IoDone::CatalogSource {
            file: request.file,
            path: Err("no asset store session".to_string()),
        },
        IoPurpose::CatalogThumb | IoPurpose::CatalogPayload => IoDone::CatalogFile {
            file: request.file,
            payload: matches!(request.purpose, IoPurpose::CatalogPayload),
            role: None,
            media: None,
            revision: None,
            path: Err("no asset store session".to_string()),
            views: Vec::new(),
        },
        IoPurpose::ViewerOpen { generation, copy_to } => {
            let bytes = std::fs::read(&request.path).map_err(|error| error.to_string());
            let copy_to = match (&bytes, copy_to) {
                (Ok(bytes), Some(target)) => std::fs::write(&target, bytes)
                    .map(|_| target)
                    .map_err(|error| {
                        // The viewer falls back to writing on the UI thread.
                        format!("viewer copy failed: {error}")
                    })
                    .ok(),
                _ => None,
            };
            IoDone::ViewerOpen {
                file: request.file,
                generation,
                copy_to,
                bytes,
            }
        }
        IoPurpose::ThumbModel => IoDone::ThumbModel {
            bytes: std::fs::read(&request.path).map_err(|error| error.to_string()),
            file: request.file,
        },
        IoPurpose::ThumbAudioWaveform => {
            let png = std::fs::read(&request.path)
                .ok()
                .and_then(|bytes| crate::audio::parse_wav(&bytes).ok())
                .as_ref()
                .and_then(crate::audio::waveform_thumbnail_png);
            IoDone::ThumbAudioWaveform {
                file: request.file,
                png,
            }
        }
        IoPurpose::GalleryPreviewEncoded { views } => {
            // The BYTES first, so a stamped sheet's declared layout can be
            // read before the picture is decoded into pixels.
            let bytes = std::fs::read(&request.path).ok();
            let layout = bytes
                .as_deref()
                .and_then(makepad_asset_importer::anim_icon::read_layout);
            let decoded = bytes.and_then(|bytes| decode_image_from_data(&bytes).ok());
            // What the picture IS, by declaration only — the manifest's
            // views first, the packer's stamped layout second, one still
            // image otherwise. `makepad_asset_widgets::thumb` is the single
            // interpreter; there is no dimension guess left to fall to.
            let plan = if !views.is_empty() {
                makepad_asset_widgets::plan_views(&views)
            } else if let Some((cells, fps)) = layout {
                makepad_asset_widgets::ThumbPlan::Cells(cells, fps)
            } else {
                makepad_asset_widgets::ThumbPlan::Whole
            };
            let (pixels, sequence, fps) = match decoded {
                Some(image) => cut_planned_preview(image, &plan),
                None => (None, Vec::new(), 0.0),
            };
            IoDone::GalleryPreview {
                file: request.file,
                cache_source: Some(request.path),
                pixels,
                sequence,
                fps,
            }
        }
        IoPurpose::GalleryPreviewWav => {
            let pixels = std::fs::read(&request.path)
                .ok()
                .and_then(|bytes| crate::audio::parse_wav(&bytes).ok())
                .map(|pcm| PreviewPixels::Raw {
                    width: crate::audio::WAVEFORM_THUMB_W,
                    height: crate::audio::WAVEFORM_THUMB_H,
                    data: crate::audio::waveform_bgra(
                        &pcm,
                        crate::audio::WAVEFORM_THUMB_W,
                        crate::audio::WAVEFORM_THUMB_H,
                    ),
                });
            IoDone::GalleryPreview {
                file: request.file,
                cache_source: None,
                pixels,
                sequence: Vec::new(),
                fps: 0.0,
            }
        }
        IoPurpose::GalleryPreviewBillboard => decode_billboard_preview(request),
    }
}

/// Run the shared interpreter's plan over a decoded picture. A whole-image
/// plan keeps the encoded buffer (the stock mipmapping path); a region or
/// cell plan cuts through `makepad_asset_widgets::thumb` — the ONE cutter.
fn cut_planned_preview(
    image: ImageBuffer,
    plan: &makepad_asset_widgets::ThumbPlan,
) -> (Option<PreviewPixels>, Vec<PreviewPixels>, f32) {
    if matches!(plan, makepad_asset_widgets::ThumbPlan::Whole) {
        return (Some(PreviewPixels::Encoded(image)), Vec::new(), 0.0);
    }
    let level0 = image.width.saturating_mul(image.height);
    let pixels = if image.data.len() >= level0 {
        &image.data[..level0]
    } else {
        image.data.as_slice()
    };
    match makepad_asset_widgets::cut_plan_bgra(image.width, image.height, pixels, plan) {
        makepad_asset_widgets::ThumbPixels::Still { width, height, bgra } => {
            // A cut that degraded to the full picture keeps the encoded
            // buffer instead of a second copy of the same pixels.
            if width == image.width && height == image.height {
                (Some(PreviewPixels::Encoded(image)), Vec::new(), 0.0)
            } else {
                (
                    Some(PreviewPixels::Raw { width, height, data: bgra }),
                    Vec::new(),
                    0.0,
                )
            }
        }
        makepad_asset_widgets::ThumbPixels::Frames { width, height, frames, fps } => {
            let sequence: Vec<PreviewPixels> = frames
                .into_iter()
                .map(|data| PreviewPixels::Raw { width, height, data })
                .collect();
            let first = sequence.first().cloned();
            (first, sequence, fps)
        }
    }
}

fn decode_billboard_preview(request: IoRequest) -> IoDone {
    let text = std::fs::read_to_string(&request.path).unwrap_or_default();
    let Ok(bb) = makepad_asset_importer::stateful_billboard::StatefulBillboard::parse(&text) else {
        // Tile PNGs are stored as billboard-domain image payloads.
        if let Some(image) = std::fs::read(&request.path)
            .ok()
            .and_then(|b| decode_image_from_data(&b).ok())
        {
            return IoDone::GalleryPreview {
                file: request.file,
                cache_source: Some(request.path),
                pixels: Some(PreviewPixels::Encoded(image)),
                sequence: Vec::new(),
                fps: 0.0,
            };
        }
        return IoDone::GalleryPreview {
            file: request.file,
            cache_source: Some(request.path),
            pixels: None,
            sequence: Vec::new(),
            fps: 0.0,
        };
    };
    let fps = bb.preview_fps() as f32;
    let mut sequence = Vec::new();
    // One packed sheet decodes once; its cells become the same per-frame
    // pixels loose frame PNGs used to give us.
    let mut pixels = crate::billboard_view::BillboardFrames::new(&request.path, &bb);
    for frame in bb.preview_frames() {
        if let Some(img) = pixels.image(frame) {
            sequence.push(PreviewPixels::Encoded(img));
        }
    }
    let pixels = sequence.first().cloned();
    IoDone::GalleryPreview {
        file: request.file,
        cache_source: Some(request.path),
        pixels,
        sequence,
        fps,
    }
}

// ---------------------------------------------------------------------------
// Latest-selection-wins gate (pure, unit-tested)
// ---------------------------------------------------------------------------

/// One queued/submitted viewer open. Carries everything needed to submit the
/// read, so a click superseded while another is in flight can be fired later
/// without consulting UI state again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingOpen {
    pub file: String,
    pub path: PathBuf,
    pub copy_to: Option<PathBuf>,
    pub generation: u64,
    /// Set when the bytes come from the catalog rather than `path` — the
    /// same latest-click-wins gate serves both sources.
    pub store: Option<StoreSource>,
}

/// At most one viewer read in flight; only the newest click waits behind
/// it; a completion is displayable only when it IS the newest click.
#[derive(Default)]
pub struct ViewerOpenGate {
    next_generation: u64,
    /// The latest click the user made — the only one worth displaying.
    wanted: u64,
    in_flight: Option<u64>,
    queued: Option<PendingOpen>,
}

impl ViewerOpenGate {
    /// Register a click. `Some(open)` = submit this read now; `None` = it
    /// waits (replacing any older waiter) until the in-flight read lands.
    pub fn click(
        &mut self,
        file: &str,
        path: PathBuf,
        copy_to: Option<PathBuf>,
        store: Option<StoreSource>,
    ) -> Option<PendingOpen> {
        self.next_generation += 1;
        self.wanted = self.next_generation;
        let open = PendingOpen {
            file: file.to_string(),
            path,
            copy_to,
            generation: self.next_generation,
            store,
        };
        if self.in_flight.is_some() {
            self.queued = Some(open);
            None
        } else {
            self.in_flight = Some(open.generation);
            Some(open)
        }
    }

    /// A completion arrived: `(display_it, next_read_to_submit)`.
    pub fn complete(&mut self, generation: u64) -> (bool, Option<PendingOpen>) {
        if self.in_flight == Some(generation) {
            self.in_flight = None;
        }
        let display = generation == self.wanted;
        let next = if self.in_flight.is_none() {
            self.queued.take()
        } else {
            None
        };
        if let Some(open) = &next {
            self.in_flight = Some(open.generation);
        }
        (display, next)
    }
}

// ---------------------------------------------------------------------------
// Viewer content state machine (pure, unit-tested)
// ---------------------------------------------------------------------------

/// What the central viewer is committed to right now. Selection (the blue
/// ring) and content are DIFFERENT states: selection moves instantly on
/// every click, while content transitions Loading → Showing/Failed only
/// through the still-current async commit. The old content is cleared at
/// Loading entry, so a slow or failed read can never leave prior content
/// under a new caption.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum ViewerContent {
    /// Nothing selected/shown (startup, or the shown item was deleted).
    #[default]
    Empty,
    /// A click cleared the viewer; the payload read is in flight.
    Loading(String),
    /// Content + caption committed together for this file.
    Showing(String),
    /// The read/decode failed; an honest error state is on screen.
    Failed(String),
}

impl ViewerContent {
    pub fn file(&self) -> Option<&str> {
        match self {
            ViewerContent::Empty => None,
            ViewerContent::Loading(file)
            | ViewerContent::Showing(file)
            | ViewerContent::Failed(file) => Some(file.as_str()),
        }
    }

    /// Returning to the Create surface: does the viewer need a fresh async
    /// open to match the current selection? True when a completion landed
    /// (and selected itself) while another surface was up. A Failed state
    /// for the SAME selection does not retry on every surface flip — the
    /// user retries by clicking the card.
    pub fn needs_reopen(&self, selected: Option<&str>) -> bool {
        match selected {
            None => false,
            Some(selected) => self.file() != Some(selected),
        }
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn viewer_content_transitions_keep_selection_and_content_separate() {
        // Startup: nothing shown, nothing to reopen.
        let mut viewer = ViewerContent::default();
        assert_eq!(viewer.file(), None);
        assert!(!viewer.needs_reopen(None));
        // A selection exists but the viewer never showed it (completion
        // landed while another surface was up): reopen on return.
        assert!(viewer.needs_reopen(Some("lib-2.mp4")));

        // Click → Loading; the loading file IS the current selection, so a
        // surface round-trip must not double-open it.
        viewer = ViewerContent::Loading("lib-2.mp4".into());
        assert!(!viewer.needs_reopen(Some("lib-2.mp4")));

        // Commit → Showing for the same file; a NEWER selection (completion
        // self-selecting while hidden) demands a reopen.
        viewer = ViewerContent::Showing("lib-2.mp4".into());
        assert!(!viewer.needs_reopen(Some("lib-2.mp4")));
        assert!(viewer.needs_reopen(Some("lib-3.wav")));

        // Failure is sticky for its own selection: no retry loop on surface
        // flips; a different selection still reopens.
        viewer = ViewerContent::Failed("lib-2.mp4".into());
        assert!(!viewer.needs_reopen(Some("lib-2.mp4")));
        assert!(viewer.needs_reopen(Some("lib-3.wav")));

        // Deleting the shown item empties the viewer.
        viewer = ViewerContent::Empty;
        assert!(!viewer.needs_reopen(None));
    }

    #[test]
    fn stale_completions_are_suppressed_and_the_latest_click_wins() {
        let mut gate = ViewerOpenGate::default();
        let a = gate
            .click("lib-1.png", PathBuf::from("/a"), None, None)
            .expect("first click submits");
        // Two more clicks while A is in flight: only the NEWEST waits.
        assert!(gate.click("lib-2.png", PathBuf::from("/b"), None, None).is_none());
        assert!(gate.click("lib-3.png", PathBuf::from("/c"), None, None).is_none());

        // A lands: it is no longer the wanted selection — do not display,
        // and the queued newest click (C, not B) goes out.
        let (display, next) = gate.complete(a.generation);
        assert!(!display, "superseded completion must not display");
        let c = next.expect("newest click submits on completion");
        assert_eq!(c.file, "lib-3.png");

        // A duplicate/unknown stale completion changes nothing.
        let (display, next) = gate.complete(a.generation);
        assert!(!display);
        assert!(next.is_none());

        // C lands and is displayable; nothing further waits.
        let (display, next) = gate.complete(c.generation);
        assert!(display);
        assert!(next.is_none());

        // A fresh click with the pipe idle submits immediately again.
        assert!(gate.click("lib-4.png", PathBuf::from("/d"), None, None).is_some());
    }

    #[test]
    fn worker_reads_prewrites_and_reports_missing_files_hermetically() {
        let dir = std::env::temp_dir().join(format!(
            "ai-content-io-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let payload = dir.join("clip.mp4");
        std::fs::write(&payload, b"mp4-bytes").unwrap();
        let copy = dir.join("viewer-open.mp4");

        let cx = makepad_widgets::Cx::new(Box::new(|_, _| {}));
        let io = ArtifactIo::start(cx.thread_spawner());
        io.request(IoRequest {
            file: "clip.mp4".into(),
            path: payload.clone(),
            purpose: IoPurpose::ViewerOpen {
                generation: 7,
                copy_to: Some(copy.clone()),
            },
            store: None,
        });
        let done = io.rx.recv_timeout(Duration::from_secs(5)).unwrap();
        match done {
            IoDone::ViewerOpen {
                file,
                generation,
                copy_to,
                bytes,
            } => {
                assert_eq!(file, "clip.mp4");
                assert_eq!(generation, 7);
                assert_eq!(bytes.unwrap(), b"mp4-bytes");
                assert_eq!(copy_to.as_deref(), Some(copy.as_path()));
                assert_eq!(std::fs::read(&copy).unwrap(), b"mp4-bytes");
            }
            _ => panic!("wrong completion kind"),
        }

        // A missing payload reports an error instead of wedging the worker.
        io.request(IoRequest {
            file: "gone.glb".into(),
            path: dir.join("gone.glb"),
            purpose: IoPurpose::ThumbModel,
            store: None,
        });
        match io.rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            IoDone::ThumbModel { file, bytes } => {
                assert_eq!(file, "gone.glb");
                assert!(bytes.is_err());
            }
            _ => panic!("wrong completion kind"),
        }

        // Gallery previews decode ON the worker: a real PNG comes back as
        // pixels + its cache-source key; garbage comes back as an honest
        // None (the app pins a badge instead of retrying forever).
        let sidecar = dir.join("lib-1.glb.thumb");
        let png = makepad_ai_hub::testpattern::encode_png_rgba(
            &[255u8; 6 * 4 * 4],
            6,
            4,
        )
        .unwrap();
        std::fs::write(&sidecar, &png).unwrap();
        io.request(IoRequest {
            file: "lib-1.glb".into(),
            path: sidecar.clone(),
            purpose: IoPurpose::GalleryPreviewEncoded { views: Vec::new() },
            store: None,
        });
        match io.rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            IoDone::GalleryPreview { file, cache_source, pixels, sequence, fps } => {
                assert_eq!(file, "lib-1.glb");
                assert_eq!(cache_source.as_deref(), Some(sidecar.as_path()));
                assert!(sequence.is_empty());
                assert_eq!(fps, 0.0);
                match pixels {
                    Some(PreviewPixels::Encoded(image)) => {
                        assert_eq!((image.width, image.height), (6, 4));
                    }
                    _ => panic!("expected decoded image pixels"),
                }
            }
            _ => panic!("wrong completion kind"),
        }
        std::fs::write(&sidecar, b"not an image").unwrap();
        io.request(IoRequest {
            file: "lib-1.glb".into(),
            path: sidecar,
            purpose: IoPurpose::GalleryPreviewEncoded { views: Vec::new() },
            store: None,
        });
        match io.rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            IoDone::GalleryPreview { pixels, .. } => assert!(pixels.is_none()),
            _ => panic!("wrong completion kind"),
        }

        // KayKit (and Quake MDL) thumbs are 1024×128 walk sheets — they
        // must land as a cycling sequence, same as a billboard.
        let sheet = dir.join("lib-kaykit.glb.thumb");
        let red = makepad_asset_importer::anim_icon::fit_tile(&[255, 0, 0, 255], 1, 1);
        let green = makepad_asset_importer::anim_icon::fit_tile(&[0, 255, 0, 255], 1, 1);
        let png = makepad_asset_importer::anim_icon::pack_sheet(&[red, green]).unwrap().png;
        std::fs::write(&sheet, &png).unwrap();
        io.request(IoRequest {
            file: "lib-kaykit.glb".into(),
            path: sheet,
            purpose: IoPurpose::GalleryPreviewEncoded { views: Vec::new() },
            store: None,
        });
        match io.rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            IoDone::GalleryPreview {
                file,
                pixels,
                sequence,
                fps,
                ..
            } => {
                assert_eq!(file, "lib-kaykit.glb");
                assert_eq!(sequence.len(), 2);
                assert_eq!(fps, makepad_asset_importer::anim_icon::SHEET_PREVIEW_FPS);
                match pixels {
                    Some(PreviewPixels::Raw { width, height, .. }) => {
                        assert_eq!((width, height), (128, 128));
                    }
                    _ => panic!("expected first tile as raw pixels"),
                }
            }
            _ => panic!("wrong completion kind"),
        }

        // A generated 1024×1024 image payload has sheet-shaped dimensions
        // but declares nothing — no manifest views, no stamp — so it is its
        // own still preview: never split into cycling tiles. Dimensions are
        // not a declaration.
        let render = dir.join("lib-flux.png");
        let mut rgba = vec![0u8; 1024 * 1024 * 4];
        for (i, px) in rgba.chunks_exact_mut(4).enumerate() {
            let x = (i % 1024) as u8;
            let y = (i / 1024) as u8;
            px.copy_from_slice(&[x, y, 128, 255]);
        }
        let png = makepad_ai_hub::testpattern::encode_png_rgba(&rgba, 1024, 1024).unwrap();
        std::fs::write(&render, &png).unwrap();
        io.request(IoRequest {
            file: "lib-flux.png".into(),
            path: render,
            purpose: IoPurpose::GalleryPreviewEncoded { views: Vec::new() },
            store: None,
        });
        match io.rx.recv_timeout(Duration::from_secs(10)).unwrap() {
            IoDone::GalleryPreview { file, pixels, sequence, fps, .. } => {
                assert_eq!(file, "lib-flux.png");
                assert!(sequence.is_empty(), "image payload must stay a still");
                assert_eq!(fps, 0.0);
                match pixels {
                    Some(PreviewPixels::Encoded(image)) => {
                        assert_eq!((image.width, image.height), (1024, 1024));
                    }
                    _ => panic!("expected the full still image"),
                }
            }
            _ => panic!("wrong completion kind"),
        }
        // A CATALOG thumbnail is a digest-named cache object with no
        // extension to infer anything from — and by contract it can be a
        // packed animation strip. The packer STAMPED its layout into the
        // PNG (a declaration), so it is cut, and the card gets ONE frame
        // instead of a filmstrip of tiny ones.
        let object = dir.join("a3f9c1");
        let blue = makepad_asset_importer::anim_icon::fit_tile(&[0, 0, 255, 255], 1, 1);
        let white = makepad_asset_importer::anim_icon::fit_tile(&[255, 255, 255, 255], 1, 1);
        let png = makepad_asset_importer::anim_icon::pack_sheet(&[blue, white]).unwrap().png;
        std::fs::write(&object, &png).unwrap();
        io.request(IoRequest {
            file: "store:0102030405060708090a0b0c0d0e0f10".into(),
            path: object,
            purpose: IoPurpose::GalleryPreviewEncoded { views: Vec::new() },
            store: None,
        });
        match io.rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            IoDone::GalleryPreview { pixels, sequence, .. } => {
                assert_eq!(sequence.len(), 2, "the strip is cut into its cells");
                match pixels {
                    Some(PreviewPixels::Raw { width, height, .. }) => {
                        assert_eq!(
                            (width, height),
                            (128, 128),
                            "the tile shows one cell, not the whole sheet"
                        );
                    }
                    _ => panic!("expected the first cell as raw pixels"),
                }
            }
            _ => panic!("wrong completion kind"),
        }

        // An AUDIO COMPOSITE: the manifest declares an FFT region and a
        // wave region — RECT views, no animation. The card is the STATIC
        // spectrogram crop, whatever the picture's dimensions. This is the
        // exact asset that was once guessed into a cycling sheet.
        use makepad_asset_data::{
            ThumbnailLayout, ThumbnailRect, ThumbnailView, ThumbnailViewKind,
        };
        let composite = dir.join("b4e2d7");
        let mut rgba = vec![0u8; 512 * 128 * 4];
        for (i, px) in rgba.chunks_exact_mut(4).enumerate() {
            let y = i / 512;
            // FFT half bright, wave half dark: the crop is checkable.
            let v = if y < 64 { 200 } else { 20 };
            px.copy_from_slice(&[v, v, v, 255]);
        }
        let png = makepad_ai_hub::testpattern::encode_png_rgba(&rgba, 512, 128).unwrap();
        std::fs::write(&composite, &png).unwrap();
        let fft_views = vec![
            ThumbnailView {
                kind: ThumbnailViewKind::Fft,
                layout: ThumbnailLayout::Rect(ThumbnailRect { x: 0, y: 0, w: 512, h: 64 }),
                fps: None,
            },
            ThumbnailView {
                kind: ThumbnailViewKind::Wave,
                layout: ThumbnailLayout::Rect(ThumbnailRect { x: 0, y: 64, w: 512, h: 64 }),
                fps: None,
            },
        ];
        io.request(IoRequest {
            file: "store:0102030405060708090a0b0c0d0e0f11".into(),
            path: composite,
            purpose: IoPurpose::GalleryPreviewEncoded { views: fft_views.clone() },
            store: None,
        });
        match io.rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            IoDone::GalleryPreview { pixels, sequence, fps, .. } => {
                assert!(sequence.is_empty(), "an audio composite NEVER cycles");
                assert_eq!(fps, 0.0);
                match pixels {
                    Some(PreviewPixels::Raw { width, height, data }) => {
                        assert_eq!((width, height), (512, 64), "the declared FFT region");
                        assert!(
                            data.iter().all(|px| (px & 0xff) > 100),
                            "the crop is the bright FFT half, not the wave"
                        );
                    }
                    _ => panic!("expected the FFT crop as raw pixels"),
                }
            }
            _ => panic!("wrong completion kind"),
        }

        // Precedence: the MANIFEST's declaration outranks the picture's own
        // stamp. A stamped two-cell sheet whose manifest says "this is one
        // still image region" draws still.
        let stamped = dir.join("c5f3e8");
        let red2 = makepad_asset_importer::anim_icon::fit_tile(&[255, 0, 0, 255], 1, 1);
        let cyan = makepad_asset_importer::anim_icon::fit_tile(&[0, 255, 255, 255], 1, 1);
        let sheet2 = makepad_asset_importer::anim_icon::pack_sheet(&[red2, cyan]).unwrap();
        std::fs::write(&stamped, &sheet2.png).unwrap();
        io.request(IoRequest {
            file: "store:0102030405060708090a0b0c0d0e0f12".into(),
            path: stamped,
            purpose: IoPurpose::GalleryPreviewEncoded {
                views: vec![ThumbnailView {
                    kind: ThumbnailViewKind::Image,
                    layout: ThumbnailLayout::Rect(ThumbnailRect {
                        x: 0,
                        y: 0,
                        w: sheet2.width,
                        h: sheet2.height,
                    }),
                    fps: None,
                }],
            },
            store: None,
        });
        match io.rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            IoDone::GalleryPreview { sequence, .. } => {
                assert!(
                    sequence.is_empty(),
                    "manifest views outrank the stamp: no cycling"
                );
            }
            _ => panic!("wrong completion kind"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gallery_stack_is_last_requested_first_and_rebumps() {
        let mut stack = GalleryStack::new();
        let mk = |file: &str| IoRequest {
            file: file.into(),
            path: PathBuf::from(file),
            purpose: IoPurpose::GalleryPreviewEncoded { views: Vec::new() },
            store: None,
        };
        stack.push_latest(mk("old"));
        stack.push_latest(mk("mid"));
        stack.push_latest(mk("new"));
        stack.push_latest(mk("old")); // scroll back — old becomes latest
        let a = stack.pop_latest().unwrap();
        let b = stack.pop_latest().unwrap();
        let c = stack.pop_latest().unwrap();
        assert_eq!(a.file, "old");
        assert_eq!(b.file, "new");
        assert_eq!(c.file, "mid");
        stack.finish("old");
        stack.finish("new");
        stack.finish("mid");
        stack.shutdown();
    }

    #[test]
    fn gallery_stack_drops_oldest_when_capped() {
        let mut stack = GalleryStack::new();
        for i in 0..(GALLERY_STACK_CAP + 10) {
            stack.push_latest(IoRequest {
                file: format!("f{i}"),
                path: PathBuf::from("x"),
                purpose: IoPurpose::GalleryPreviewEncoded { views: Vec::new() },
            store: None,
        });
        }
        let latest = stack.pop_latest().unwrap();
        assert_eq!(latest.file, format!("f{}", GALLERY_STACK_CAP + 9));
        stack.shutdown();
    }
}
