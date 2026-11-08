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
use makepad_ai_services::pictures::RgbaPicture;
use makepad_widgets::makepad_platform::clipboard_read::{ClipboardContent, ClipboardImage};
use makepad_widgets::*;
use std::path::Path;
use std::sync::Arc;

/// The longest side a picture keeps; what the CLIs accept everywhere.
pub const MAX_SIDE: usize = 1024;
/// Bytes a dropped file may have.
pub const MAX_FILE_BYTES: usize = 32 * 1024 * 1024;
/// Pixels a dropped file may decode to.
const MAX_PIXELS: usize = 40 * 1024 * 1024;

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
            DragItem::VirtualFile(file) if image_name_supported(&file.name) && file.bytes.len() <= MAX_FILE_BYTES => {
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

    /// The picture on the clipboard (a copied image, or a copied image file),
    /// numbered so several pasted pictures keep apart in the transcript.
    pub fn from_clipboard(content: &ClipboardContent, n: usize) -> Option<Source> {
        let name = format!("pasted-{n}.png");
        match &content.image {
            Some(ClipboardImage::Png(png)) => Some(Source::Bytes { name, bytes: Arc::from(png.as_slice()) }),
            Some(ClipboardImage::Rgba { width, height, rgba }) => {
                Some(Source::Rgba { name, width: *width as usize, height: *height as usize, rgba: rgba.clone() })
            }
            None => content
                .files
                .iter()
                .find(|file| image_name_supported(&file.path) && Path::new(&file.path).is_absolute())
                .map(|file| Source::Path(std::path::PathBuf::from(&file.path))),
        }
    }
}

/// Read, decode and hash a dropped picture, and bring a copy down to the model's size.
pub fn load(source: Source) -> Result<Dropped, String> {
    let name = source.name();
    if heic_name(&name) {
        return Err(format!("{name}: HEIC photos cannot be read here yet; save it as JPEG or PNG and drop that"));
    }
    let (original, ext, picture): (Arc<[u8]>, &str, RgbaPicture) = match source {
        Source::Rgba { width, height, rgba, .. } => {
            if width == 0 || height == 0 || width.saturating_mul(height) > MAX_PIXELS || rgba.len() < width * height * 4 {
                return Err(format!("{name}: the picture is empty or too large"));
            }
            let rgba = rgba[..width * height * 4].to_vec();
            let png = Cx::encode_rgba_as_png(width as u32, height as u32, &rgba)?;
            (Arc::from(png), "png", RgbaPicture { width: width as u32, height: height as u32, rgba })
        }
        Source::Path(path) => {
            let meta = std::fs::metadata(&path).map_err(|e| format!("{name}: {e}"))?;
            if !meta.is_file() || meta.len() as usize > MAX_FILE_BYTES {
                return Err(format!("{name}: not a regular image file of at most 32 MiB"));
            }
            let bytes: Arc<[u8]> = Arc::from(std::fs::read(&path).map_err(|e| format!("{name}: {e}"))?);
            let (ext, picture) = decode(&bytes, &name)?;
            (bytes, ext, picture)
        }
        Source::Bytes { bytes, .. } => {
            let (ext, picture) = decode(&bytes, &name)?;
            (bytes, ext, picture)
        }
    };
    let (width, height) = (picture.width as usize, picture.height as usize);
    let mut sent = normalize(name.clone(), width, height, &picture.rgba)?;
    sent.thumb = scale_to(name, width, height, &picture.rgba, THUMB_SIDE).ok().map(|t| t.png);
    let file = picture_file(&original, ext);
    Ok(Dropped { original, file, width: picture.width, height: picture.height, sent })
}

/// The side of a picture's small copy (the composer's chip, the transcript's line).
const THUMB_SIDE: usize = 96;

/// A picture's small copy for the transcript, from its bytes as they came.
pub fn thumb(bytes: &[u8], name: &str) -> Result<Arc<[u8]>, String> {
    let (_, picture) = decode(bytes, name)?;
    scale_to(name.to_string(), picture.width as usize, picture.height as usize, &picture.rgba, THUMB_SIDE).map(|t| t.png)
}

/// A picture's pixels from its bytes, and the extension of the format they are in.
pub fn decode(bytes: &[u8], name: &str) -> Result<(&'static str, RgbaPicture), String> {
    let ext = image_ext(bytes).ok_or_else(|| format!("{name}: not a PNG, JPEG, WebP, GIF or BMP picture"))?;
    let (width, height, rgba) = makepad_image_decode::decode_rgba(bytes).map_err(|e| format!("{name}: cannot decode ({e})"))?;
    if width == 0 || height == 0 || width.saturating_mul(height) > MAX_PIXELS {
        return Err(format!("{name}: the picture is empty or too large"));
    }
    Ok((ext, RgbaPicture { width: width as u32, height: height as u32, rgba }))
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
    scale_to(name, width, height, rgba, MAX_SIDE)
}

fn scale_to(name: String, width: usize, height: usize, rgba: &[u8], max_side: usize) -> Result<Attachment, String> {
    if width == 0 || height == 0 || rgba.len() < width * height * 4 {
        return Err(format!("{name}: no pixels"));
    }
    let scale = (max_side as f64 / width.max(height) as f64).min(1.0);
    let (w, h) = (((width as f64 * scale).round() as usize).max(1), ((height as f64 * scale).round() as usize).max(1));
    let pixels = if (w, h) == (width, height) {
        rgba[..width * height * 4].to_vec()
    } else {
        let mut out = vec![0u8; w * h * 4];
        for y in 0..h {
            let y0 = y * height / h;
            let y1 = (((y + 1) * height).div_ceil(h)).clamp(y0 + 1, height);
            for x in 0..w {
                let x0 = x * width / w;
                let x1 = (((x + 1) * width).div_ceil(w)).clamp(x0 + 1, width);
                let mut sum = [0u32; 4];
                for sy in y0..y1 {
                    let row = &rgba[(sy * width + x0) * 4..(sy * width + x1) * 4];
                    for px in row.chunks_exact(4) {
                        for c in 0..4 {
                            sum[c] += px[c] as u32;
                        }
                    }
                }
                let n = ((y1 - y0) * (x1 - x0)) as u32;
                for c in 0..4 {
                    out[(y * w + x) * 4 + c] = (sum[c] / n) as u8;
                }
            }
        }
        out
    };
    let png = Cx::encode_rgba_as_png(w as u32, h as u32, &pixels)?;
    Ok(Attachment { name, png: Arc::from(png), width: w, height: h, thumb: None })
}
