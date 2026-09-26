//! Pictures for the model: reference images a person drops into the chat,
//! and captures a host takes of its own window. Every picture reaches the
//! model as one still PNG of at most [`MAX_SIDE`] pixels a side, whatever
//! it started as (PNG, JPEG or WebP; a window grab at device pixels).
//!
//! Decoding and scaling are CPU work: hosts run [`normalize`] on the task
//! pool, never on the UI thread.

use makepad_widgets::*;
use std::path::Path;
use std::sync::Arc;

/// The longest side a picture keeps; what the CLIs accept everywhere.
pub const MAX_SIDE: usize = 1024;
/// Bytes a dropped file may have.
pub const MAX_FILE_BYTES: usize = 32 * 1024 * 1024;
/// Pixels a dropped file may decode to.
const MAX_PIXELS: usize = 40 * 1024 * 1024;

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
        .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "webp"))
}

/// Where a dropped picture comes from.
#[derive(Clone, Debug)]
pub enum Source {
    Path(std::path::PathBuf),
    Bytes { name: String, bytes: Arc<[u8]> },
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
            Source::Bytes { name, .. } => name.clone(),
        }
    }
}

/// Read, decode and bring a dropped picture down to the model's size.
pub fn load(source: Source) -> Result<Attachment, String> {
    let name = source.name();
    let bytes: Arc<[u8]> = match source {
        Source::Path(path) => {
            let meta = std::fs::metadata(&path).map_err(|e| format!("{name}: {e}"))?;
            if !meta.is_file() || meta.len() as usize > MAX_FILE_BYTES {
                return Err(format!("{name}: not a regular image file of at most 32 MiB"));
            }
            Arc::from(std::fs::read(&path).map_err(|e| format!("{name}: {e}"))?)
        }
        Source::Bytes { bytes, .. } => bytes,
    };
    let decoded = decode_image_from_data(&bytes).map_err(|e| format!("{name}: cannot decode ({e})"))?;
    if decoded.width == 0 || decoded.height == 0 || decoded.width.saturating_mul(decoded.height) > MAX_PIXELS {
        return Err(format!("{name}: the picture is empty or too large"));
    }
    let mut rgba = Vec::with_capacity(decoded.width * decoded.height * 4);
    for p in &decoded.data[..decoded.width * decoded.height] {
        rgba.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8, (p >> 24) as u8]);
    }
    let mut out = normalize(name.clone(), decoded.width, decoded.height, &rgba)?;
    out.thumb = scale_to(name, decoded.width, decoded.height, &rgba, 96).ok().map(|t| t.png);
    Ok(out)
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
