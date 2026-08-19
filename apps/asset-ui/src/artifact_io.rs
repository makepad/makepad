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

use makepad_widgets::makepad_platform::thread::SignalToUI;
use makepad_widgets::{decode_image_from_data, ImageBuffer};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

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
    /// Gallery-card preview: read + DECODE an encoded image (sidecar, or an
    /// image payload that is its own preview) on the worker, so card draws
    /// never touch the filesystem or a PNG decoder.
    GalleryPreviewEncoded,
    /// Gallery-card preview for a legacy sidecarless WAV: read + parse +
    /// min/max scan on the worker.
    GalleryPreviewWav,
    /// Stateful billboard: decode the preview state's native-size frames.
    GalleryPreviewBillboard,
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
}

pub struct ArtifactIo {
    tx: Sender<IoRequest>,
    rx: Receiver<IoDone>,
    gallery: Arc<GalleryStack>,
}

impl ArtifactIo {
    pub fn start() -> Self {
        let (request_tx, request_rx) = channel::<IoRequest>();
        let (done_tx, done_rx) = channel::<IoDone>();
        let gallery = Arc::new(GalleryStack::new());
        std::thread::Builder::new()
            .name("asset-ui-artifact-io".into())
            .spawn({
                let done_tx = done_tx.clone();
                let gallery = Arc::clone(&gallery);
                move || dispatch_loop(request_rx, done_tx, gallery)
            })
            .expect("artifact io dispatcher");
        let n = gallery_worker_count();
        for i in 0..n {
            let done_tx = done_tx.clone();
            let gallery = Arc::clone(&gallery);
            std::thread::Builder::new()
                .name(format!("asset-ui-preview-{i}"))
                .spawn(move || gallery_loop(gallery, done_tx))
                .expect("gallery decode worker");
        }
        Self {
            tx: request_tx,
            rx: done_rx,
            gallery,
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

impl Drop for ArtifactIo {
    fn drop(&mut self) {
        self.gallery.shutdown();
    }
}

fn is_gallery(purpose: &IoPurpose) -> bool {
    matches!(
        purpose,
        IoPurpose::GalleryPreviewEncoded
            | IoPurpose::GalleryPreviewWav
            | IoPurpose::GalleryPreviewBillboard
    )
}

fn dispatch_loop(rx: Receiver<IoRequest>, tx: Sender<IoDone>, gallery: Arc<GalleryStack>) {
    while let Ok(request) = rx.recv() {
        if is_gallery(&request.purpose) {
            gallery.push_latest(request);
            continue;
        }
        let done = process(request);
        if tx.send(done).is_err() {
            return;
        }
        SignalToUI::set_ui_signal();
    }
    gallery.shutdown();
}

fn gallery_loop(gallery: Arc<GalleryStack>, tx: Sender<IoDone>) {
    while let Some(request) = gallery.pop_latest() {
        let file = request.file.clone();
        let done = process(request);
        gallery.finish(&file);
        if tx.send(done).is_err() {
            return;
        }
        SignalToUI::set_ui_signal();
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
    inner: Mutex<GalleryInner>,
    cv: Condvar,
}

impl GalleryStack {
    fn new() -> Self {
        Self {
            inner: Mutex::new(GalleryInner {
                stack: Vec::new(),
                decoding: HashSet::new(),
                shutdown: false,
            }),
            cv: Condvar::new(),
        }
    }

    fn push_latest(&self, request: IoRequest) {
        let mut g = self.inner.lock().expect("gallery stack");
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
        self.cv.notify_one();
    }

    fn pop_latest(&self) -> Option<IoRequest> {
        let mut g = self.inner.lock().expect("gallery stack");
        loop {
            if g.shutdown {
                return None;
            }
            while let Some(request) = g.stack.pop() {
                if g.decoding.insert(request.file.clone()) {
                    return Some(request);
                }
            }
            g = self.cv.wait(g).expect("gallery stack");
        }
    }

    fn finish(&self, file: &str) {
        let mut g = self.inner.lock().expect("gallery stack");
        g.decoding.remove(file);
    }

    fn shutdown(&self) {
        let mut g = self.inner.lock().expect("gallery stack");
        g.shutdown = true;
        self.cv.notify_all();
    }
}

fn gallery_worker_count() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(2, 8)
}

fn process(request: IoRequest) -> IoDone {
    match request.purpose {
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
        IoPurpose::GalleryPreviewEncoded => {
            let decoded = std::fs::read(&request.path)
                .ok()
                .and_then(|bytes| decode_image_from_data(&bytes).ok());
            // Walk/idle sheets only ever live in importer-written `.thumb`
            // sidecars. An image PAYLOAD is always a still: a 1024×1024
            // Flux render passes the sheet dimension test too and must not
            // be chopped into 64 cycling tiles.
            let sidecar = request
                .path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("thumb"));
            let (pixels, sequence, fps) = match decoded {
                Some(image) if sidecar => split_encoded_preview(image),
                Some(image) => (Some(PreviewPixels::Encoded(image)), Vec::new(), 0.0),
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

/// A 128-tile walk/idle sheet becomes the same multi-frame preview the
/// library already cycles for billboards. Square stills stay a single texture.
fn split_encoded_preview(
    image: ImageBuffer,
) -> (Option<PreviewPixels>, Vec<PreviewPixels>, f32) {
    let level0 = image.width.saturating_mul(image.height);
    let pixels = if image.data.len() >= level0 {
        &image.data[..level0]
    } else {
        image.data.as_slice()
    };
    if let Some(frames) = makepad_asset_importer::anim_icon::split_sheet_bgra(
        image.width,
        image.height,
        pixels,
    ) {
        let tile = makepad_asset_importer::anim_icon::TILE;
        let sequence: Vec<PreviewPixels> = frames
            .into_iter()
            .map(|data| PreviewPixels::Raw {
                width: tile,
                height: tile,
                data,
            })
            .collect();
        let pixels = sequence.first().cloned();
        (
            pixels,
            sequence,
            makepad_asset_importer::anim_icon::SHEET_PREVIEW_FPS,
        )
    } else {
        (Some(PreviewPixels::Encoded(image)), Vec::new(), 0.0)
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
    for frame in bb.preview_frames() {
        let path = bb.resolve_frame(&request.path, frame);
        if let Some(img) = std::fs::read(&path)
            .ok()
            .and_then(|bytes| decode_image_from_data(&bytes).ok())
        {
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
    ) -> Option<PendingOpen> {
        self.next_generation += 1;
        self.wanted = self.next_generation;
        let open = PendingOpen {
            file: file.to_string(),
            path,
            copy_to,
            generation: self.next_generation,
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
            .click("lib-1.png", PathBuf::from("/a"), None)
            .expect("first click submits");
        // Two more clicks while A is in flight: only the NEWEST waits.
        assert!(gate.click("lib-2.png", PathBuf::from("/b"), None).is_none());
        assert!(gate.click("lib-3.png", PathBuf::from("/c"), None).is_none());

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
        assert!(gate.click("lib-4.png", PathBuf::from("/d"), None).is_some());
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

        let io = ArtifactIo::start();
        io.request(IoRequest {
            file: "clip.mp4".into(),
            path: payload.clone(),
            purpose: IoPurpose::ViewerOpen {
                generation: 7,
                copy_to: Some(copy.clone()),
            },
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
        let png = makepad_asset_ai::testpattern::encode_png_rgba(
            &[255u8; 6 * 4 * 4],
            6,
            4,
        )
        .unwrap();
        std::fs::write(&sidecar, &png).unwrap();
        io.request(IoRequest {
            file: "lib-1.glb".into(),
            path: sidecar.clone(),
            purpose: IoPurpose::GalleryPreviewEncoded,
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
            purpose: IoPurpose::GalleryPreviewEncoded,
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
        let png = makepad_asset_importer::anim_icon::pack_sheet(&[red, green]).unwrap();
        std::fs::write(&sheet, &png).unwrap();
        io.request(IoRequest {
            file: "lib-kaykit.glb".into(),
            path: sheet,
            purpose: IoPurpose::GalleryPreviewEncoded,
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
        // but is its own still preview: never split into cycling tiles.
        let render = dir.join("lib-flux.png");
        let mut rgba = vec![0u8; 1024 * 1024 * 4];
        for (i, px) in rgba.chunks_exact_mut(4).enumerate() {
            let x = (i % 1024) as u8;
            let y = (i / 1024) as u8;
            px.copy_from_slice(&[x, y, 128, 255]);
        }
        let png = makepad_asset_ai::testpattern::encode_png_rgba(&rgba, 1024, 1024).unwrap();
        std::fs::write(&render, &png).unwrap();
        io.request(IoRequest {
            file: "lib-flux.png".into(),
            path: render,
            purpose: IoPurpose::GalleryPreviewEncoded,
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
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gallery_stack_is_last_requested_first_and_rebumps() {
        let stack = GalleryStack::new();
        let mk = |file: &str| IoRequest {
            file: file.into(),
            path: PathBuf::from(file),
            purpose: IoPurpose::GalleryPreviewEncoded,
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
        let stack = GalleryStack::new();
        for i in 0..(GALLERY_STACK_CAP + 10) {
            stack.push_latest(IoRequest {
                file: format!("f{i}"),
                path: PathBuf::from("x"),
                purpose: IoPurpose::GalleryPreviewEncoded,
            });
        }
        let latest = stack.pop_latest().unwrap();
        assert_eq!(latest.file, format!("f{}", GALLERY_STACK_CAP + 9));
        stack.shutdown();
    }
}
