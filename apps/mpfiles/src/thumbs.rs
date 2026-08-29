//! Thumbnails and type icons — one widget draws both.
//!
//! Pictures get a real thumbnail: the file is read and decoded on a worker
//! thread (never the UI thread), box-filtered down to at most [`THUMB_PX`] on
//! its long edge, and handed back as BGRA pixels the UI turns into a texture.
//! Decoded thumbs live in a bounded LRU so browsing a 20k-file photo folder
//! costs a fixed amount of GPU memory.
//!
//! Playable video gets the same treatment through the platform's standalone
//! file decoder: its first frame is the thumbnail. Videos the decoder does not
//! demux keep the film-strip icon.
//!
//! Everything else gets its kind's SVG, drawn by the same `Image` widget —
//! which is why [`MpfThumb`] exists: it remembers what it is already showing,
//! so a list item repopulated every frame does not re-parse an SVG or reset a
//! texture (and, through `Image::set_texture`'s redraw, spin the frame clock).

use makepad_widgets::*;
use makepad_widgets::makepad_platform::thread::SignalToUI;
use makepad_widgets::makepad_platform::video_file::VideoFileDecoder;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        mpsc::{channel, Receiver, Sender},
        Arc, OnceLock,
    },
    thread,
};

use crate::model::FileKind;

/// Longest edge of a decoded thumbnail, in pixels. Big enough for the icon
/// grid on a retina screen, small enough that 256 of them are pocket change.
pub const THUMB_PX: usize = 192;
/// How many decoded thumbnails stay resident.
pub const THUMB_CACHE_CAP: usize = 256;
/// Files larger than this are not thumbnailed: the decode would cost more
/// than the picture is worth in a grid cell.
pub const THUMB_MAX_FILE_BYTES: u64 = 96 * 1024 * 1024;
/// Decode workers. Two keeps one slow JPEG from stalling the whole grid.
const WORKERS: usize = 2;

script_mod! {
    use mod.prelude.widgets.*

    mod.widgets.MpfThumbBase = #(MpfThumb::register_widget(vm))
    mod.widgets.MpfThumb = set_type_default() do mod.widgets.MpfThumbBase{
        width: Fit
        height: Fit
        align: Align{x: 0.5 y: 0.5}
        img := Image{
            width: 72
            height: 56
            fit: ImageFit.Smallest
        }
    }
}

/// Pixels handed back by a worker: BGRA `0xAARRGGBB`, row-major.
pub struct ThumbPixels {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u32>,
}

struct ThumbDone {
    path: PathBuf,
    pixels: Option<ThumbPixels>,
}

struct CacheSlot {
    /// `None` once a decode failed — remembered so we never retry in a loop.
    texture: Option<Texture>,
    tick: u64,
}

/// The thumbnail cache: request pictures, drain finished decodes, look them up.
pub struct Thumbs {
    senders: Vec<Sender<PathBuf>>,
    results: Receiver<ThumbDone>,
    slots: HashMap<PathBuf, CacheSlot>,
    inflight: HashMap<PathBuf, ()>,
    tick: u64,
    next_worker: usize,
}

impl Default for Thumbs {
    fn default() -> Self {
        Self::new()
    }
}

impl Thumbs {
    pub fn new() -> Self {
        let (done_tx, results) = channel::<ThumbDone>();
        let mut senders = Vec::with_capacity(WORKERS);
        for _ in 0..WORKERS {
            let (tx, rx) = channel::<PathBuf>();
            let done = done_tx.clone();
            // A dedicated channel per worker (instead of one shared, mutex-guarded
            // receiver) keeps a blocking `recv` from serializing the pool.
            thread::spawn(move || {
                while let Ok(path) = rx.recv() {
                    let pixels = decode_thumb(&path);
                    if done.send(ThumbDone { path, pixels }).is_err() {
                        return;
                    }
                    SignalToUI::set_ui_signal();
                }
            });
            senders.push(tx);
        }
        Self {
            senders,
            results,
            slots: HashMap::new(),
            inflight: HashMap::new(),
            tick: 0,
            next_worker: 0,
        }
    }

    /// The texture for `path` if it is decoded; queues a decode if it is not.
    /// Returns `None` while the decode is pending or after it failed.
    pub fn get_or_request(&mut self, path: &Path) -> Option<Texture> {
        self.tick += 1;
        let tick = self.tick;
        if let Some(slot) = self.slots.get_mut(path) {
            slot.tick = tick;
            return slot.texture.clone();
        }
        if self.inflight.contains_key(path) {
            return None;
        }
        self.inflight.insert(path.to_path_buf(), ());
        let worker = self.next_worker % self.senders.len();
        self.next_worker = self.next_worker.wrapping_add(1);
        let _ = self.senders[worker].send(path.to_path_buf());
        None
    }

    /// Turn everything the workers finished into textures. Returns true when
    /// something landed, i.e. the views need a redraw.
    pub fn drain(&mut self, cx: &mut Cx) -> bool {
        let done: Vec<ThumbDone> = self.results.try_iter().collect();
        if done.is_empty() {
            return false;
        }
        for item in done {
            self.inflight.remove(&item.path);
            let texture = item.pixels.map(|p| {
                Texture::new_with_format(
                    cx,
                    TextureFormat::VecBGRAu8_32 {
                        width: p.width,
                        height: p.height,
                        data: Some(p.data),
                        updated: TextureUpdated::Full,
                    },
                )
            });
            self.tick += 1;
            let tick = self.tick;
            self.slots.insert(item.path, CacheSlot { texture, tick });
        }
        self.evict();
        true
    }

    /// Drop the least recently looked-at slots down to the cap.
    fn evict(&mut self) {
        while self.slots.len() > THUMB_CACHE_CAP {
            let Some(oldest) = self
                .slots
                .iter()
                .min_by_key(|(_, slot)| slot.tick)
                .map(|(path, _)| path.clone())
            else {
                return;
            };
            self.slots.remove(&oldest);
        }
    }

    /// How many thumbnails are decoded and resident.
    pub fn resident(&self) -> usize {
        self.slots.len()
    }
}

/// Read, decode and downscale one file's picture. Runs on a worker thread.
///
/// The *kind* comes from the name the browser shows, and the *bytes* from
/// whatever file actually backs it — the two are the same thing on a real
/// disk and deliberately different in the demo, which is what lets a made-up
/// photo have a real thumbnail.
fn decode_thumb(path: &Path) -> Option<ThumbPixels> {
    let real = crate::vfs::vfs().real_path(path);
    if crate::model::is_playable_video(path) {
        // A video is never read whole — the decoder demuxes to the first
        // frame — so the picture-sized file cap does not apply here.
        return decode_video_thumb(&real);
    }
    let meta = std::fs::metadata(&real).ok()?;
    if meta.len() > THUMB_MAX_FILE_BYTES {
        return None;
    }
    let data = std::fs::read(&real).ok()?;
    let image = decode_image_from_data(&data).ok()?;
    Some(downscale(image.width, image.height, &image.data))
}

/// The first frame of a video, through the platform's hardware file decoder —
/// the same seam the importer's video probe uses, minus its crate.
fn decode_video_thumb(path: &Path) -> Option<ThumbPixels> {
    let mut decoder = VideoFileDecoder::open(path.to_str()?).ok()?;
    let frame = decoder.next_frame().ok()??;
    let rgb = frame.to_rgb8();
    let (width, height) = (frame.width as usize, frame.height as usize);
    if width == 0 || height == 0 || rgb.len() < width * height * 3 {
        return None;
    }
    let bgra: Vec<u32> = rgb
        .chunks_exact(3)
        .map(|p| 0xff00_0000 | ((p[0] as u32) << 16) | ((p[1] as u32) << 8) | p[2] as u32)
        .collect();
    Some(downscale(width, height, &bgra))
}

/// Box-filter `src` (BGRA `0xAARRGGBB`) down so its long edge is at most
/// [`THUMB_PX`]. Images already that small are copied through.
fn downscale(width: usize, height: usize, src: &[u32]) -> ThumbPixels {
    let long = width.max(height);
    if width == 0 || height == 0 || long <= THUMB_PX {
        return ThumbPixels {
            width,
            height,
            data: src.to_vec(),
        };
    }
    let scale = THUMB_PX as f64 / long as f64;
    let dst_w = ((width as f64 * scale).round() as usize).max(1);
    let dst_h = ((height as f64 * scale).round() as usize).max(1);
    let mut data = Vec::with_capacity(dst_w * dst_h);
    for y in 0..dst_h {
        let y0 = y * height / dst_h;
        let y1 = (((y + 1) * height).div_ceil(dst_h)).min(height).max(y0 + 1);
        for x in 0..dst_w {
            let x0 = x * width / dst_w;
            let x1 = (((x + 1) * width).div_ceil(dst_w)).min(width).max(x0 + 1);
            // Alpha-weighted so transparent texels don't bleed their
            // undefined color into the average.
            let (mut b, mut g, mut r, mut a, mut n) = (0u64, 0u64, 0u64, 0u64, 0u64);
            for sy in y0..y1 {
                let row = sy * width;
                for sx in x0..x1 {
                    let p = src[row + sx];
                    let pa = ((p >> 24) & 0xff) as u64;
                    b += (p & 0xff) as u64 * pa;
                    g += ((p >> 8) & 0xff) as u64 * pa;
                    r += ((p >> 16) & 0xff) as u64 * pa;
                    a += pa;
                    n += 1;
                }
            }
            data.push(if a == 0 {
                0
            } else {
                let out_a = (a / n.max(1)) as u32;
                (out_a << 24) | (((r / a) as u32) << 16) | (((g / a) as u32) << 8) | (b / a) as u32
            });
        }
    }
    ThumbPixels {
        width: dst_w,
        height: dst_h,
        data,
    }
}

/// The kind icons, compiled in and shared: `Image::load_svg_from_shared_data`
/// skips a re-parse when handed the same allocation, so every tile showing a
/// folder shares one `Arc` and parses once.
fn kind_svg(kind: FileKind) -> Arc<[u8]> {
    static ICONS: OnceLock<HashMap<&'static str, Arc<[u8]>>> = OnceLock::new();
    let icons = ICONS.get_or_init(|| {
        let mut map: HashMap<&'static str, Arc<[u8]>> = HashMap::new();
        map.insert("folder", Arc::from(&include_bytes!("../resources/icons/folder.svg")[..]));
        map.insert("file", Arc::from(&include_bytes!("../resources/icons/file.svg")[..]));
        map.insert("image", Arc::from(&include_bytes!("../resources/icons/image.svg")[..]));
        map.insert("text", Arc::from(&include_bytes!("../resources/icons/text.svg")[..]));
        map.insert("code", Arc::from(&include_bytes!("../resources/icons/code.svg")[..]));
        map.insert("audio", Arc::from(&include_bytes!("../resources/icons/audio.svg")[..]));
        map.insert("video", Arc::from(&include_bytes!("../resources/icons/video.svg")[..]));
        map.insert("archive", Arc::from(&include_bytes!("../resources/icons/archive.svg")[..]));
        map.insert("pdf", Arc::from(&include_bytes!("../resources/icons/pdf.svg")[..]));
        map
    });
    icons
        .get(kind.icon_name())
        .cloned()
        .unwrap_or_else(|| icons["file"].clone())
}

/// What an [`MpfThumb`] currently shows.
#[derive(Clone, Debug, Default, PartialEq)]
enum Shown {
    #[default]
    Nothing,
    Kind(FileKind),
    Thumb(PathBuf),
}

/// One icon slot: a picture's thumbnail when it has one, its kind's SVG
/// otherwise, told apart so repopulating a list item costs a comparison.
#[derive(Script, ScriptHook, Widget)]
pub struct MpfThumb {
    #[deref]
    view: View,
    #[rust]
    shown: Shown,
}

impl MpfThumb {
    pub fn show_kind(&mut self, cx: &mut Cx, kind: FileKind) {
        if self.shown == Shown::Kind(kind) {
            return;
        }
        self.shown = Shown::Kind(kind);
        let slot = self.view.image(cx, ids!(img));
        if let Some(mut image) = slot.borrow_mut() {
            let _ = image.load_svg_from_shared_data(cx, kind_svg(kind));
        };
    }

    /// Show nothing: an icon slot in a grid cell the folder does not fill.
    pub fn show_nothing(&mut self, cx: &mut Cx) {
        if self.shown == Shown::Nothing {
            return;
        }
        self.shown = Shown::Nothing;
        self.view.image(cx, ids!(img)).set_texture(cx, None);
    }

    pub fn show_thumb(&mut self, cx: &mut Cx, path: &Path, texture: Texture) {
        if self.shown == Shown::Thumb(path.to_path_buf()) {
            return;
        }
        self.shown = Shown::Thumb(path.to_path_buf());
        self.view.image(cx, ids!(img)).set_texture(cx, Some(texture));
    }
}

impl Widget for MpfThumb {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

/// Blank an icon slot.
pub fn clear_thumb(cx: &mut Cx, slot: &WidgetRef) {
    if let Some(mut thumb) = slot.borrow_mut::<MpfThumb>() {
        thumb.show_nothing(cx);
    };
}

/// Set a thumb slot from an entry: real thumbnail when the picture is
/// decoded, its kind's icon until then (and forever, for non-pictures).
pub fn fill_thumb(cx: &mut Cx, slot: &WidgetRef, entry: &crate::model::FileEntry, thumbs: &mut Thumbs) {
    let Some(mut thumb) = slot.borrow_mut::<MpfThumb>() else {
        return;
    };
    if crate::model::is_thumbnailable(&entry.path) {
        if let Some(texture) = thumbs.get_or_request(&entry.path) {
            thumb.show_thumb(cx, &entry.path, texture);
            return;
        }
    }
    thumb.show_kind(cx, entry.kind);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downscale_keeps_aspect_and_caps_long_edge() {
        let src = vec![0xff112233u32; 400 * 200];
        let out = downscale(400, 200, &src);
        assert_eq!(out.width, THUMB_PX);
        assert_eq!(out.height, THUMB_PX / 2);
        assert_eq!(out.data.len(), out.width * out.height);
        // A flat image survives the box filter exactly.
        assert_eq!(out.data[0], 0xff112233);
    }

    #[test]
    fn downscale_passes_small_images_through() {
        let src = vec![0xff445566u32; 8 * 4];
        let out = downscale(8, 4, &src);
        assert_eq!((out.width, out.height), (8, 4));
        assert_eq!(out.data.len(), 32);
    }

    #[test]
    fn every_kind_has_an_icon() {
        for kind in [
            FileKind::Folder,
            FileKind::Image,
            FileKind::Text,
            FileKind::Code,
            FileKind::Audio,
            FileKind::Video,
            FileKind::Archive,
            FileKind::Pdf,
            FileKind::Generic,
        ] {
            assert!(!kind_svg(kind).is_empty(), "{:?} has no icon", kind);
        }
        // Distinct kinds get distinct drawings.
        assert!(!Arc::ptr_eq(&kind_svg(FileKind::Folder), &kind_svg(FileKind::Generic)));
        // The same kind shares one allocation, which is what lets the SVG
        // load be skipped on repopulate.
        assert!(Arc::ptr_eq(&kind_svg(FileKind::Audio), &kind_svg(FileKind::Audio)));
    }
}
