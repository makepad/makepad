//! Pictures for the model: reference images a person drops into the chat,
//! and captures a host takes of its own window. Every picture reaches the
//! model as one still PNG of at most [`MAX_SIDE`] pixels a side, whatever
//! it started as (PNG, JPEG or WebP; a window grab at device pixels). A
//! dropped picture is also kept as it came ([`Dropped`]): the chat keeps it,
//! and an app's tools read it at its own size.
//!
//! Decoding, hashing and scaling are CPU work: hosts run [`load`] and
//! [`normalize`] on the task pool, never on the UI thread.

use makepad_ai_services::engine::{picture_file, NewPicture};
use makepad_ai_services::pictures::{encode_rgba_png, RgbaPicture};
use makepad_widgets::makepad_platform::clipboard_read::{ClipboardContent, ClipboardImage};
use makepad_widgets::makepad_platform::thread::{SubmitError, TaskPool};
use makepad_widgets::*;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::io;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

/// The longest side a picture keeps; what the CLIs accept everywhere.
pub const MAX_SIDE: usize = 1024;
/// Above eight megapixels even a still JPEG needs over 56 MiB for RGB and RGBA.
/// Only the scheduling class changes at this size.
const LARGE_DECODE_PIXELS: usize = 8 * 1024 * 1024;

pub(super) fn large_decode(width: usize, height: usize) -> bool {
    width.saturating_mul(height) >= LARGE_DECODE_PIXELS
}

/// A reason for the person, with the original diagnostic when the two differ.
#[derive(Debug)]
pub(super) struct PictureError {
    reason: &'static str,
    detail: Option<String>,
}

impl PictureError {
    fn plain(reason: &'static str) -> Self {
        Self { reason, detail: None }
    }

    pub(super) fn rewritten(reason: &'static str, detail: impl ToString) -> Self {
        Self { reason, detail: Some(detail.to_string()) }
    }

    pub(super) fn open(kind: io::ErrorKind, detail: impl ToString) -> Self {
        let reason = match kind {
            io::ErrorKind::NotFound => "not found",
            io::ErrorKind::PermissionDenied => "no permission to read it",
            _ => "couldn't be opened, try again",
        };
        Self::rewritten(reason, detail)
    }

    pub(super) fn load(detail: impl ToString) -> Self {
        Self::rewritten("couldn't be loaded, try again", detail)
    }

    pub(super) fn report(&self, name: &str, mut log: impl FnMut(&str, &str)) -> &'static str {
        if let Some(detail) = &self.detail { log(name, detail); }
        self.reason
    }

    fn diagnostic(&self, name: &str) -> String {
        format!("{name}: {}", self.detail.as_deref().unwrap_or(self.reason))
    }
}

/// A picture the person dropped (or pasted, or picked): as it came, and the copy the model gets.
#[derive(Clone, Debug)]
pub struct Dropped {
    /// The picture's bytes as they came (a pasted bitmap: a PNG of it).
    pub original: Arc<[u8]>,
    /// Its file name in the chat's pictures: the SHA-256 of `original`, and its format's extension.
    pub file: String,
    /// The original's pixels.
    pub width: u32,
    pub height: u32,
    /// The copy for the model, with a small one for the composer's chip.
    pub sent: Attachment,
}

impl Dropped {
    /// The picture for the chat's next line.
    pub fn into_new(self) -> NewPicture {
        NewPicture { name: self.sent.name, original: self.original, file: self.file, width: self.width, height: self.height, for_model: self.sent.png }
    }
}

/// A picture on its way to the model.
#[derive(Clone, Debug)]
pub struct Attachment {
    /// A short name for the transcript and the model (`keys.webp`).
    pub name: String,
    pub png: Arc<[u8]>,
    pub width: usize,
    pub height: usize,
    /// A small copy for the composer's chip (dropped pictures only).
    pub thumb: Option<Arc<[u8]>>,
}

pub fn image_name_supported(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "webp" | "heic" | "heif"))
}

/// HEIC/HEIF photos are taken (a phone's default) but not decoded here yet:
/// the person is told what to do instead of seeing a decode error.
fn heic_name(name: &str) -> bool {
    Path::new(name).extension().and_then(|s| s.to_str()).is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "heic" | "heif"))
}

/// Where a dropped picture comes from.
#[derive(Clone, Debug)]
pub enum Source {
    Path(std::path::PathBuf),
    Bytes { name: String, bytes: Arc<[u8]> },
    /// Pixels already decoded (a pasted bitmap): top-down RGBA8.
    Rgba { name: String, width: usize, height: usize, rgba: Arc<Vec<u8>> },
}

impl Source {
    /// The picture a drag carries, if it carries one.
    pub fn from_drag_item(item: &DragItem) -> Option<Source> {
        match item {
            DragItem::FilePath { path, .. } if image_name_supported(path) && Path::new(path).is_absolute() && path.len() <= 4096 => {
                Some(Source::Path(std::path::PathBuf::from(path)))
            }
            DragItem::VirtualFile(file) if image_name_supported(&file.name) => {
                Some(Source::Bytes { name: file.name.clone(), bytes: file.bytes.clone() })
            }
            _ => None,
        }
    }

    /// A pasted line that names an image file (a path or a `file://` URL).
    pub fn from_pasted_text(text: &str) -> Option<Source> {
        let text = text.trim();
        let path = text.strip_prefix("file://").unwrap_or(text);
        let path = path.replace("%20", " ");
        let path = Path::new(&path);
        (path.is_absolute() && image_name_supported(&path.to_string_lossy()) && path.is_file()).then(|| Source::Path(path.to_path_buf()))
    }

    pub fn name(&self) -> String {
        match self {
            Source::Path(path) => path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "image".into()),
            Source::Bytes { name, .. } | Source::Rgba { name, .. } => name.clone(),
        }
    }

    /// All copied picture files, in order, or the bitmap when the clipboard offers no picture files.
    pub fn from_clipboard(content: &ClipboardContent, n: usize) -> Vec<Source> {
        let files: Vec<_> = content.files.iter()
            .filter(|file| image_name_supported(&file.path) && Path::new(&file.path).is_absolute())
            .map(|file| Source::Path(std::path::PathBuf::from(&file.path)))
            .collect();
        if !files.is_empty() {
            return files;
        }
        let name = format!("pasted-{n}.png");
        match &content.image {
            Some(ClipboardImage::Png(png)) => vec![Source::Bytes { name, bytes: Arc::from(png.as_slice()) }],
            Some(ClipboardImage::Rgba { width, height, rgba }) => {
                vec![Source::Rgba { name, width: *width as usize, height: *height as usize, rgba: rgba.clone() }]
            }
            None => Vec::new(),
        }
    }
}

/// Read, decode and hash a dropped picture, and bring a copy down to the model's size.
pub fn load(source: Source) -> Result<Dropped, String> {
    let name = source.name();
    load_picture(source, &name).map_err(|e| e.diagnostic(&name))
}

pub(super) fn load_picture(source: Source, name: &str) -> Result<Dropped, PictureError> {
    PreparedPicture::read(source, name)?.decode(name)
}

enum Original {
    Encoded(Arc<[u8]>, &'static str),
    Bitmap(Arc<Vec<u8>>),
}

pub(super) struct PreparedPicture {
    original: Original,
    width: usize,
    height: usize,
}

impl PreparedPicture {
    pub(super) fn read(source: Source, name: &str) -> Result<Self, PictureError> {
        if heic_name(name) {
            return Err(PictureError::plain("HEIC photos cannot be read here yet; save it as JPEG or PNG and drop that"));
        }
        let bytes = match source {
            Source::Rgba { width, height, rgba, .. } => {
                let pixels = makepad_image_decode::checked_pixel_count(width, height).map_err(decode_error)?;
                if rgba.len() < pixels * 4 {
                    return Err(PictureError::rewritten("empty picture", "missing clipboard pixels"));
                }
                return Ok(Self { original: Original::Bitmap(rgba), width, height });
            }
            Source::Path(path) => {
                let open_error = |e: io::Error| PictureError::open(e.kind(), e);
                let meta = std::fs::metadata(&path).map_err(open_error)?;
                if !meta.is_file() {
                    return Err(PictureError::plain("choose a picture file"));
                }
                check_input_bytes(meta.len())?;
                Arc::from(std::fs::read(path).map_err(open_error)?)
            }
            Source::Bytes { bytes, .. } => bytes,
        };
        check_input_bytes(bytes.len() as u64)?;
        let ext = picture_extension(&bytes)?;
        let (width, height) = makepad_image_decode::image_size_by_data(&bytes, Path::new("")).map_err(decode_error)?;
        Ok(Self { original: Original::Encoded(bytes, ext), width, height })
    }

    pub(super) fn is_large(&self) -> bool { large_decode(self.width, self.height) }

    pub(super) fn encoded_bytes(&self) -> Option<&[u8]> {
        match &self.original {
            Original::Encoded(bytes, _) => Some(bytes),
            Original::Bitmap(_) => None,
        }
    }

    pub(super) fn decode(self, name: &str) -> Result<Dropped, PictureError> {
        let (original, ext, picture) = match self.original {
            Original::Bitmap(rgba) => {
                let (width, height) = (self.width as u32, self.height as u32);
                let mut rgba = Arc::try_unwrap(rgba).unwrap_or_else(|shared| (*shared).clone());
                rgba.truncate(self.width * self.height * 4);
                let png = encode_rgba_png(self.width, self.height, &rgba).map_err(PictureError::load)?;
                (Arc::from(png), "png", RgbaPicture { width, height, rgba })
            }
            Original::Encoded(bytes, ext) => {
                let picture = rgba_picture(&bytes)?;
                (bytes, ext, picture)
            }
        };
        let (width, height) = (picture.width as usize, picture.height as usize);
        let mut sent = scale_to(name, width, height, &picture.rgba, MAX_SIDE)?;
        sent.thumb = scale_to(name, width, height, &picture.rgba, THUMB_SIDE).ok().map(|t| t.png);
        let file = picture_file(&original, ext);
        Ok(Dropped { original, file, width: picture.width, height: picture.height, sent })
    }
}

// The panel holds the start closure while its picture is wanted. Clearing a
// selection drops the queued bytes immediately. A running task owns its permit
// until its work ends, including after a Clear or an unwind.
type StartDecode = Cell<Option<Box<dyn FnOnce(Option<Arc<()>>)>>>;
#[derive(Default)]
pub(super) struct DecodeQueue {
    waiting: VecDeque<std::rc::Weak<StartDecode>>,
    running: std::sync::Weak<()>,
}

impl DecodeQueue {
    fn schedule(&mut self, large: bool, start: impl FnOnce(Option<Arc<()>>) + 'static) -> Option<Rc<StartDecode>> {
        if !large {
            start(None);
            return None;
        }
        let job: Rc<StartDecode> = Rc::new(Cell::new(Some(Box::new(start))));
        self.waiting.push_back(Rc::downgrade(&job));
        self.pump();
        Some(job)
    }

    pub(super) fn pump(&mut self) {
        while self.running.strong_count() == 0 {
            let Some(job) = self.waiting.pop_front() else { break };
            if let Some(start) = job.upgrade().and_then(|job| job.take()) {
                let permit = Arc::new(());
                self.running = Arc::downgrade(&permit);
                start(Some(permit));
            }
        }
    }

    pub(super) fn submit<T: Send + 'static>(
        &mut self, pool: TaskPool, class: Class, large: bool, work: impl FnOnce() -> T + Send + 'static,
    ) -> DecodeTask<T> {
        let handle = Rc::new(RefCell::new(None));
        let submitted = handle.clone();
        let queued = self.schedule(large, move |permit| {
            let result = pool.submit(class, move || {
                let _permit = permit;
                work()
            });
            *submitted.borrow_mut() = Some(result);
        });
        DecodeTask { _queued: queued, handle }
    }
}

pub(super) struct DecodeTask<T> {
    _queued: Option<Rc<StartDecode>>,
    handle: Rc<RefCell<Option<Result<TaskHandle<T>, SubmitError>>>>,
}

impl<T> DecodeTask<T> {
    pub(super) fn try_take(&self) -> Option<Result<T, String>> {
        match self.handle.borrow_mut().as_mut()? {
            Ok(handle) => handle.try_take().map(|result| result.map_err(|e| e.to_string())),
            Err(error) => Some(Err(error.to_string())),
        }
    }

    pub(super) fn cancel(self) {
        if let Some(Ok(handle)) = self.handle.borrow_mut().take() {
            handle.cancel();
            handle.detach();
        }
    }
}

pub(super) enum SlotState<H, T> {
    Reading(H),
    Ready(T),
    Failed(&'static str),
}

pub(super) struct PictureSlot<H, T> {
    pub id: u64,
    pub name: String,
    pub state: SlotState<H, T>,
}

/// The chips are the selection's record of its errors, in the person's order.
struct LoadBatch<H, T> {
    slots: Vec<PictureSlot<H, T>>,
    reading: usize,
}

impl<H, T> LoadBatch<H, T> {
    fn report_if_settled(&mut self, notices: &mut Vec<String>) {
        if self.reading != 0 { return; }
        let total = self.slots.len();
        let mut failures = Vec::new();
        self.slots.retain(|slot| {
            if let SlotState::Failed(reason) = slot.state {
                failures.push((slot.name.clone(), reason));
                return false;
            }
            true
        });
        match failures.as_slice() {
            [] => {}
            [(name, reason)] => notices.push(format!("{name} couldn't be read ({reason})")),
            several => {
                let details = several.iter().map(|(name, reason)| format!("{name} ({reason})")).collect::<Vec<_>>();
                notices.push(format!("{} of {total} couldn't be read: {}", several.len(), details.join(", ")));
            }
        }
    }
}

pub(super) struct PictureDraft<H, T> {
    batches: Vec<LoadBatch<H, T>>,
    next_id: u64,
    pub revision: u64,
    pub reading: usize,
    pub count: usize,
}

impl<H, T> Default for PictureDraft<H, T> {
    fn default() -> Self {
        Self { batches: Vec::new(), next_id: 0, revision: 0, reading: 0, count: 0 }
    }
}

impl<H, T> PictureDraft<H, T> {
    pub fn slots(&self) -> impl Iterator<Item = &PictureSlot<H, T>> {
        self.batches.iter().flat_map(|batch| &batch.slots)
    }

    fn slot(&mut self, name: String, state: SlotState<H, T>) -> PictureSlot<H, T> {
        self.next_id += 1;
        PictureSlot { id: self.next_id, name, state }
    }

    fn changed(&mut self) {
        self.batches.retain(|batch| !batch.slots.is_empty());
        self.reading = self.batches.iter().map(|batch| batch.reading).sum();
        self.count = self.slots().filter(|slot| !matches!(slot.state, SlotState::Failed(_))).count();
        self.revision += 1;
    }

    pub fn select(&mut self, items: impl Iterator<Item = (String, Result<H, PictureError>)>,
        notices: &mut Vec<String>, mut log: impl FnMut(&str, &str)) {
        let mut batch = LoadBatch { slots: Vec::new(), reading: 0 };
        for (name, item) in items {
            let state = match item {
                Ok(handle) => { batch.reading += 1; SlotState::Reading(handle) }
                Err(error) => SlotState::Failed(error.report(&name, &mut log)),
            };
            batch.slots.push(self.slot(name, state));
        }
        if batch.slots.is_empty() { return; }
        batch.report_if_settled(notices);
        self.batches.push(batch);
        self.changed();
    }

    pub fn prepend_ready(&mut self, items: impl Iterator<Item = (String, T)>) {
        let slots: Vec<_> = items.map(|(name, value)| self.slot(name, SlotState::Ready(value))).collect();
        if !slots.is_empty() {
            self.batches.insert(0, LoadBatch { slots, reading: 0 });
            self.changed();
        }
    }

    pub fn poll_batches(&mut self, mut take: impl FnMut(&str, &mut H) -> Option<Result<T, PictureError>>,
        notices: &mut Vec<String>, mut log: impl FnMut(&str, &str)) {
        let mut changed = false;
        for batch in &mut self.batches {
            if batch.reading == 0 { continue; }
            for slot in &mut batch.slots {
                let SlotState::Reading(handle) = &mut slot.state else { continue };
                let Some(result) = take(&slot.name, handle) else { continue };
                slot.state = match result {
                    Ok(picture) => SlotState::Ready(picture),
                    Err(error) => SlotState::Failed(error.report(&slot.name, &mut log)),
                };
                batch.reading -= 1;
                changed = true;
            }
            batch.report_if_settled(notices);
        }
        if changed { self.changed(); }
    }

    /// Even a loading slot beyond the provider's limit keeps the whole draft here.
    pub fn ready(&self) -> Option<Vec<&T>> {
        self.slots().map(|slot| match &slot.state {
            SlotState::Ready(value) => Some(value),
            _ => None,
        }).collect()
    }

    pub fn sent(&mut self, mut count: usize, mut keep: impl FnMut(T)) {
        for batch in &mut self.batches {
            if count == 0 { break; }
            let n = count.min(batch.slots.len());
            for slot in batch.slots.drain(..n) {
                if let SlotState::Ready(picture) = slot.state { keep(picture); }
            }
            count -= n;
        }
        self.changed();
    }

    pub fn remove(&mut self, id: u64, notices: &mut Vec<String>) -> Option<SlotState<H, T>> {
        for batch in &mut self.batches {
            if let Some(at) = batch.slots.iter().position(|slot| slot.id == id) {
                let slot = batch.slots.remove(at);
                if matches!(slot.state, SlotState::Reading(_)) { batch.reading -= 1; }
                batch.report_if_settled(notices);
                self.changed();
                return Some(slot.state);
            }
        }
        None
    }

    pub fn clear(&mut self, mut cancel: impl FnMut(H)) {
        if self.batches.is_empty() { return; }
        for slot in self.batches.drain(..).flat_map(|batch| batch.slots) {
            if let SlotState::Reading(handle) = slot.state { cancel(handle); }
        }
        self.changed();
    }
}

/// The side of a picture's small copy (the composer's chip, the transcript's line).
const THUMB_SIDE: usize = 96;

/// A picture's small copy for the transcript, from its bytes as they came.
pub fn thumb(bytes: &[u8], name: &str) -> Result<Arc<[u8]>, String> {
    let (_, picture) = decode(bytes, name)?;
    scale_to(name, picture.width as usize, picture.height as usize, &picture.rgba, THUMB_SIDE).map(|t| t.png).map_err(|e| e.diagnostic(name))
}

/// A picture's pixels from its bytes, and the extension of the format they are in.
pub fn decode(bytes: &[u8], name: &str) -> Result<(&'static str, RgbaPicture), String> {
    decode_picture(bytes).map_err(|e| e.diagnostic(name))
}

fn decode_picture(bytes: &[u8]) -> Result<(&'static str, RgbaPicture), PictureError> {
    Ok((picture_extension(bytes)?, rgba_picture(bytes)?))
}

fn picture_extension(bytes: &[u8]) -> Result<&'static str, PictureError> {
    image_ext(bytes).ok_or_else(|| PictureError::plain("not a PNG, JPEG, WebP, GIF or BMP picture"))
}

fn check_input_bytes(bytes: u64) -> Result<(), PictureError> {
    let limit = makepad_image_decode::MAX_IMAGE_DECODED_BYTES;
    if bytes > limit as u64 {
        return Err(PictureError::rewritten("too large to read", format!("input is {bytes} bytes, decoder limit is {limit}")));
    }
    Ok(())
}

fn decode_error(error: makepad_image_decode::ImageError) -> PictureError {
    use makepad_image_decode::{BmpDecoderErrors, GifDecodeErrors, ImageError::*, JpgDecodeErrors, PngDecodeErrors, WebpDecodeErrors, MAX_IMAGE_DIMENSION};
    let side = "over 16384 pixels a side";
    let reason = match &error {
        DimensionsTooLarge { width: 0, .. } | DimensionsTooLarge { height: 0, .. } | EmptyData => "empty picture",
        DimensionsTooLarge { width, height } if *width > MAX_IMAGE_DIMENSION || *height > MAX_IMAGE_DIMENSION => side,
        BmpDecode(BmpDecoderErrors::TooLargeDimensions(..)) => side,
        PngDecode(PngDecodeErrors::Generic(s)) if s.contains("larger than maximum configured") => side,
        JpgDecode(JpgDecodeErrors::Format(s)) if s.contains("greater than width limit") || s.contains("greater than height limit") => side,
        DimensionsTooLarge { .. } | DataTooLarge { .. } | GifDecode(GifDecodeErrors::MemoryLimit)
        | WebpDecode(WebpDecodeErrors::MemoryLimitExceeded | WebpDecodeErrors::ImageTooLarge) => "too large to read",
        _ => "damaged or unreadable picture",
    };
    PictureError::rewritten(reason, format!("cannot decode ({error})"))
}

pub(super) fn rgba_picture(bytes: &[u8]) -> Result<RgbaPicture, PictureError> {
    let (width, height, rgba) = makepad_image_decode::decode_rgba(bytes).map_err(decode_error)?;
    Ok(RgbaPicture { width: width as u32, height: height as u32, rgba })
}

/// The extension of the picture format `bytes` start as.
fn image_ext(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG") {
        Some("png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("jpg")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(&b"WEBP"[..]) {
        Some("webp")
    } else if bytes.starts_with(b"GIF8") {
        Some("gif")
    } else if bytes.starts_with(b"BM") {
        Some("bmp")
    } else {
        None
    }
}

/// RGBA pixels (top-down, tightly packed) to a model-sized PNG: a box
/// filter down to at most [`MAX_SIDE`] a side.
pub fn normalize(name: String, width: usize, height: usize, rgba: &[u8]) -> Result<Attachment, String> {
    scale_to(&name, width, height, rgba, MAX_SIDE).map_err(|e| e.diagnostic(&name))
}

fn scale_to(name: &str, width: usize, height: usize, rgba: &[u8], max_side: usize) -> Result<Attachment, PictureError> {
    let (width, height, png) = fitted_png(rgba, width, width, height, max_side)?;
    Ok(Attachment { name: name.to_owned(), png: Arc::from(png), width, height, thumb: None })
}

pub(super) fn fitted_size(width: usize, height: usize, max_side: usize) -> (usize, usize) {
    let scale = (max_side as f64 / width.max(height) as f64).min(1.0);
    (((width as f64 * scale).round() as usize).max(1), ((height as f64 * scale).round() as usize).max(1))
}

/// Fit the rectangle starting at the first RGBA pixel. A crop borrows its
/// frame's rows, so only the fitted image needs a new pixel buffer.
pub(super) fn fitted_png(rgba: &[u8], stride: usize, width: usize, height: usize, max_side: usize) -> Result<(usize, usize, Vec<u8>), PictureError> {
    let end = height.checked_sub(1).and_then(|y| y.checked_mul(stride))
        .and_then(|offset| offset.checked_add(width)).and_then(|pixels| pixels.checked_mul(4));
    if width == 0 || width > stride || !end.is_some_and(|len| len <= rgba.len()) {
        return Err(PictureError::rewritten("empty picture", "no pixels"));
    }
    let (w, h) = fitted_size(width, height, max_side);
    let mut out = Vec::new();
    let pixels = if (w, h) == (width, height) && stride == width {
        &rgba[..width * height * 4]
    } else if (w, h) == (width, height) {
        out.reserve(w * h * 4);
        for row in 0..h {
            let offset = row * stride * 4;
            out.extend_from_slice(&rgba[offset..offset + w * 4]);
        }
        &out
    } else {
        out.resize(w * h * 4, 0);
        for y in 0..h {
            let y0 = y * height / h;
            let y1 = (((y + 1) * height).div_ceil(h)).clamp(y0 + 1, height);
            for x in 0..w {
                let x0 = x * width / w;
                let x1 = (((x + 1) * width).div_ceil(w)).clamp(x0 + 1, width);
                let mut sum = [0u64; 4];
                for sy in y0..y1 {
                    let row = &rgba[(sy * stride + x0) * 4..(sy * stride + x1) * 4];
                    for px in row.chunks_exact(4) {
                        for c in 0..4 {
                            sum[c] += u64::from(px[c]);
                        }
                    }
                }
                let n = ((y1 - y0) * (x1 - x0)) as u64;
                for c in 0..4 {
                    out[(y * w + x) * 4 + c] = (sum[c] / n) as u8;
                }
            }
        }
        &out
    };
    let png = encode_rgba_png(w, h, pixels).map_err(PictureError::load)?;
    Ok((w, h, png))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasted_pictures_keep_pixels_and_encode_alpha_only_when_needed() {
        for (alpha, colour_type) in [(255, 2), (0, 6)] {
            let expected = RgbaPicture { width: 2, height: 1, rgba: vec![1, 2, 3, 255, 4, 5, 6, alpha] };
            let picture = load(Source::Rgba {
                name: "pasted.png".into(), width: 2, height: 1, rgba: Arc::new(expected.rgba.clone()),
            }).expect("a two-pixel pasted picture must load");
            for (kind, png) in [("model copy", picture.sent.png.as_ref()), ("pasted original", picture.original.as_ref())] {
                assert_eq!(png[25], colour_type, "the {kind} must use PNG colour type {colour_type} when alpha is {alpha}");
                let decoded = makepad_ai_services::pictures::decode_picture(png).expect("the encoded picture must decode");
                assert_eq!(decoded, expected, "the {kind} must retain the original pixels and dimensions when alpha is {alpha}");
            }
        }
    }

    #[test]
    fn a_large_decode_waits_without_holding_up_small_decodes() {
        let mut queue = DecodeQueue::default();
        let starts = Rc::new(RefCell::new(Vec::new()));
        let running = Rc::new(RefCell::new(std::collections::HashMap::new()));
        let submit = |queue: &mut DecodeQueue, name: &'static str, side| {
            let (starts, running) = (starts.clone(), running.clone());
            let job = queue.schedule(large_decode(side, side), move |permit| {
                starts.borrow_mut().push(name);
                running.borrow_mut().insert(name, permit);
            });
            (name.to_owned(), Ok((name, job)))
        };
        let mut draft = PictureDraft::default();
        let mut notes = Vec::new();
        draft.select([
            submit(&mut queue, "A", 4000), submit(&mut queue, "remove", 4000),
            submit(&mut queue, "B", 4000), submit(&mut queue, "c", 100),
        ].into_iter(), &mut notes, |_, _| {});
        assert_eq!(*starts.borrow(), ["A", "c"], "small c must start while the large decodes wait for A");
        assert_eq!(draft.reading, 4, "all four slots must count, including queued starts");
        draft.poll_batches(|name, _| (name == "c").then_some(Ok("c")), &mut notes, |_, _| {});
        assert!(matches!(draft.slots().last().unwrap().state, SlotState::Ready("c")), "c's own chip must fill before A");
        assert!(matches!(draft.slots().next().unwrap().state, SlotState::Reading(_)), "A must still own its placeholder");
        let remove = draft.slots().nth(1).unwrap().id;
        drop(draft.remove(remove, &mut notes));
        assert_eq!(draft.slots().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["A", "B", "c"], "removal must preserve the other slots' order");
        assert!(draft.ready().is_none(), "even one placeholder must keep the draft from being submitted");
        running.borrow_mut().remove("A");
        queue.pump();
        assert_eq!(*starts.borrow(), ["A", "c", "B"], "the queue must skip the removed start and start B");
        draft.select(std::iter::once(submit(&mut queue, "d", 1)), &mut notes, |_, _| {});
        assert_eq!(*starts.borrow(), ["A", "c", "B", "d"], "a later small picture must start immediately beside B");
        draft.poll_batches(|_, (name, _)| Some(Ok(*name)), &mut notes, |_, _| {});
        assert_eq!(draft.ready().unwrap(), [&"A", &"B", &"c", &"d"], "the sendable pictures must keep selection order across gestures");
        assert!(notes.is_empty(), "cancelling a queued placeholder must not report a read failure");
    }

    #[test]
    fn large_file_sizes_admit_the_original_and_decoder_limits_reach_the_person() {
        let png = Cx::encode_rgba_as_png(1, 1, &[1, 2, 3, 255]).unwrap();
        let item = DragItem::VirtualFile(makepad_platform::file_dialogs::VirtualFile {
            name: "view.png".into(), mime: "image/png".into(), size: (32 << 20) + 1, bytes: png.clone().into(),
        });
        let picture = load(Source::from_drag_item(&item).expect("a virtual picture over 32 MiB must enter its batch"))
            .expect("the virtual picture must load from its actual bytes");
        assert_eq!(&*picture.original, png.as_slice(), "the virtual picture must keep its original bytes");

        // A sparse 1×1 BMP with its pixel at the end exercises the actual file
        // size check without generating or decoding millions of pixels.
        use std::io::Write;
        let pixel_offset = 32u32 << 20;
        let file_size = pixel_offset + 4;
        let mut header = [0u8; 54];
        header[..2].copy_from_slice(b"BM");
        header[2..6].copy_from_slice(&file_size.to_le_bytes());
        header[10..14].copy_from_slice(&pixel_offset.to_le_bytes());
        header[14..18].copy_from_slice(&40u32.to_le_bytes());
        header[18..22].copy_from_slice(&1u32.to_le_bytes());
        header[22..26].copy_from_slice(&1u32.to_le_bytes());
        header[26..28].copy_from_slice(&1u16.to_le_bytes());
        header[28..30].copy_from_slice(&24u16.to_le_bytes());
        let path = std::env::temp_dir().join(format!("aichat-large-picture-{}.bmp", std::process::id()));
        let mut file = std::fs::File::create_new(&path).unwrap();
        file.write_all(&header).unwrap();
        file.set_len(u64::from(file_size)).unwrap();
        drop(file);
        let read = PreparedPicture::read(Source::Path(path.clone()), "view.bmp");
        std::fs::remove_file(path).unwrap();
        let source = read.expect("a real file over 32 MiB must reach the decoder");
        let Original::Encoded(bytes, _) = source.original else { panic!("a file must retain its original bytes") };
        assert_eq!(bytes.len(), file_size as usize, "the full original must be read from disk");
        let pixels = rgba_picture(&bytes).expect("a valid picture in a file over 32 MiB must decode");
        assert_eq!((pixels.width, pixels.height), (1, 1), "a sparse BMP must retain its pixel dimensions");

        let from_header = |width: u32, height: u32| {
            let mut bytes = png.clone();
            bytes[16..20].copy_from_slice(&width.to_be_bytes());
            bytes[20..24].copy_from_slice(&height.to_be_bytes());
            PreparedPicture::read(Source::Bytes { name: "view.png".into(), bytes: bytes.into() }, "view.png")
        };
        assert!(from_header(8000, 6000).expect("a phone-sized header must be admitted").is_large(),
            "the header alone must select the large queue before any pixels are decoded");
        let error = from_header(16385, 1).err().expect("an over-wide PNG must fail at its header");
        let mut detail = String::new();
        assert_eq!(error.report("view.png", |_, text| detail.push_str(text)), "over 16384 pixels a side",
            "the side refusal must name the decoder's limit");
        assert!(detail.contains("16385"), "the decoder's original size diagnostic must reach the log");
        let error = check_input_bytes(makepad_image_decode::MAX_IMAGE_DECODED_BYTES as u64 + 1).unwrap_err();
        assert_eq!(error.report("view.png", |_, _| {}), "too large to read", "the decoder's byte limit must have a plain reason");
    }
}
