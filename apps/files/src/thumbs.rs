//! Thumbnails and type icons — one widget draws both.
//!
//! Pictures get a real thumbnail: native files are decoded on workers; demo
//! paths select from a tiny embedded pool and decode inline. Both are
//! box-filtered down to at most [`THUMB_PX`] on their long edge and handed
//! back as BGRA pixels the UI turns into a texture.
//! Decoded thumbs live in a bounded LRU so browsing a 20k-file photo folder
//! costs a fixed amount of GPU memory.
//!
//! Native video uses the platform decoder's first frame. Demo video uses one
//! embedded still and never opens a demuxer or decoder.
//!
//! Everything else gets its kind's SVG, drawn by the same `Image` widget —
//! which is why [`MpfThumb`] exists: it remembers what it is already showing,
//! so a list item repopulated every frame does not re-parse an SVG or reset a
//! texture (and, through `Image::set_texture`'s redraw, spin the frame clock).

use makepad_widgets::*;
use makepad_widgets::makepad_platform::thread::SignalToUI;
#[cfg(not(target_arch = "wasm32"))]
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
    done_tx: Sender<ThumbDone>,
    senders: Vec<Sender<PathBuf>>,
    results: Receiver<ThumbDone>,
    slots: HashMap<PathBuf, CacheSlot>,
    inflight: HashMap<PathBuf, ()>,
    tick: u64,
    next_worker: usize,
    instant: bool,
    started: bool,
}

impl Default for Thumbs {
    fn default() -> Self {
        Self::new()
    }
}

impl Thumbs {
    pub fn new() -> Self {
        let (done_tx, results) = channel::<ThumbDone>();
        Self {
            done_tx,
            senders: Vec::new(),
            results,
            slots: HashMap::new(),
            inflight: HashMap::new(),
            tick: 0,
            next_worker: 0,
            instant: false,
            started: false,
        }
    }

    fn ensure_started(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        self.instant = crate::vfs::vfs().is_instant();
        if self.instant {
            return;
        }
        self.senders.reserve(WORKERS);
        for _ in 0..WORKERS {
            let (tx, rx) = channel::<PathBuf>();
            let done = self.done_tx.clone();
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
            self.senders.push(tx);
        }
    }

    /// The texture for `path` if it is decoded; queues a decode if it is not.
    /// Returns `None` while the decode is pending or after it failed.
    pub fn get_or_request(&mut self, cx: &mut Cx, path: &Path) -> Option<Texture> {
        self.ensure_started();
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
        if self.instant {
            let item = ThumbDone { path: path.to_path_buf(), pixels: decode_thumb(path) };
            self.finish(cx, item);
            self.evict();
            return self.slots.get(path).and_then(|slot| slot.texture.clone());
        }
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
            self.finish(cx, item);
        }
        self.evict();
        true
    }

    fn finish(&mut self, cx: &mut Cx, item: ThumbDone) {
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

trait ThumbSource {
    fn decode(&self, path: &Path) -> Option<ThumbPixels>;
}

struct NativeThumbSource;

impl ThumbSource for NativeThumbSource {
    fn decode(&self, path: &Path) -> Option<ThumbPixels> {
        let real = crate::vfs::vfs().real_path(path);
        if crate::model::is_playable_video(path) {
            #[cfg(not(target_arch = "wasm32"))]
            return decode_video_thumb(&real);
            #[cfg(target_arch = "wasm32")]
            return None;
        }
        let meta = std::fs::metadata(&real).ok()?;
        if meta.len() > THUMB_MAX_FILE_BYTES {
            return None;
        }
        let data = std::fs::read(&real).ok()?;
        decode_image_bytes(&data)
    }
}

struct DemoThumbSource;

impl DemoThumbSource {
    const IMAGES: [&'static [u8]; 8] = [
        include_bytes!("../demos/rubber-duck-illustration.png"),
        include_bytes!("../demos/amusement-ride.jpg"),
        include_bytes!("../demos/royal-esplanade-panorama.jpg"),
        include_bytes!(concat!(env!("OUT_DIR"), "/aurora-vignette.png")),
        include_bytes!(concat!(env!("OUT_DIR"), "/canyon-vignette.png")),
        include_bytes!(concat!(env!("OUT_DIR"), "/lagoon-vignette.png")),
        include_bytes!(concat!(env!("OUT_DIR"), "/meadow-vignette.png")),
        include_bytes!(concat!(env!("OUT_DIR"), "/twilight-vignette.png")),
    ];
    const VIDEO_STILL: &'static [u8] = include_bytes!(concat!(env!("OUT_DIR"), "/cinema-still.png"));

    fn bytes(path: &Path) -> &'static [u8] {
        if crate::model::is_playable_video(path) {
            return Self::VIDEO_STILL;
        }
        let hash = path
            .as_os_str()
            .to_string_lossy()
            .bytes()
            .fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
                (hash ^ byte as u64).wrapping_mul(0x1000_0000_01b3)
            });
        Self::IMAGES[hash as usize % Self::IMAGES.len()]
    }
}

impl ThumbSource for DemoThumbSource {
    fn decode(&self, path: &Path) -> Option<ThumbPixels> {
        decode_image_bytes(Self::bytes(path))
    }
}

fn decode_image_bytes(data: &[u8]) -> Option<ThumbPixels> {
    let image = decode_image_from_data(data).ok()?;
    Some(downscale(image.width, image.height, &image.data))
}

/// Dispatch at the filesystem seam: a demo path never becomes a host path,
/// and a video in the closed demo is just one embedded still.
fn decode_thumb(path: &Path) -> Option<ThumbPixels> {
    if crate::vfs::vfs().is_demo() {
        DemoThumbSource.decode(path)
    } else {
        NativeThumbSource.decode(path)
    }
}

/// The first frame of a video, through the platform's hardware file decoder —
/// the same seam the importer's video probe uses, minus its crate.
#[cfg(not(target_arch = "wasm32"))]
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
        if let Some(texture) = thumbs.get_or_request(cx, &entry.path) {
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

    #[test]
    fn demo_source_pool_is_distinct_decodable_and_uses_one_video_still() {
        let mut seen = std::collections::HashSet::new();
        assert_eq!(DemoThumbSource::IMAGES.len() + 1, 9);
        for (slot, bytes) in DemoThumbSource::IMAGES.iter().copied().enumerate() {
            assert!(seen.insert(bytes), "demo picture slots contain duplicate bytes at slot {slot}");
            assert!(decode_image_from_data(bytes).is_ok(), "demo picture slot {slot} did not decode");
        }
        assert!(seen.insert(DemoThumbSource::VIDEO_STILL), "the designated video still duplicates a picture slot");
        assert!(decode_image_from_data(DemoThumbSource::VIDEO_STILL).is_ok(), "the designated video still did not decode");

        for path in [Path::new("/Demo/Videos/clip-0001.mp4"), Path::new("/Demo/Videos/camera-0003.mkv")] {
            assert_eq!(DemoThumbSource::bytes(path), DemoThumbSource::VIDEO_STILL);
            assert!(DemoThumbSource.decode(path).is_some());
        }
        let total_bytes = DemoThumbSource::IMAGES.iter().map(|bytes| bytes.len()).sum::<usize>()
            + DemoThumbSource::VIDEO_STILL.len();
        assert!(total_bytes < 1_500_000, "embedded demo picture pool is {total_bytes} bytes");
    }
}
