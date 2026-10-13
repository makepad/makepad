//! Pictures the person attaches (a screenshot dropped on the chat): an
//! absolute path to an image file is a reference the model SEES, not text.
//! Each one is brought down to the model's size (a box filter to at most
//! [`MAX_SIDE`] pixels a side) and handed to the chat session with the turn.

use makepad_code_agent::pictures::{ToolImage, MAX_SIDE};
use makepad_widgets::*;
use std::path::Path;
use std::sync::Arc;

/// Bytes a picture file may have, and pixels it may decode to.
const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_PIXELS: usize = 40 * 1024 * 1024;

/// Registers the decoder with the agent core (the core has no image codec of its own).
pub fn install() {
    makepad_code_agent::pictures::set_loader(load);
}

/// The picture at `reference` as the model gets it.
pub fn load(reference: &str) -> Result<ToolImage, String> {
    let path = Path::new(reference);
    let name = path.file_name().map_or_else(|| "image".to_string(), |n| n.to_string_lossy().into_owned());
    let meta = std::fs::metadata(path).map_err(|e| format!("{name}: {e}"))?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return Err(format!("{name}: not a regular image file of at most 32 MiB"));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{name}: {e}"))?;
    let decoded = decode_image_from_data(&bytes).map_err(|e| format!("{name}: cannot decode ({e})"))?;
    if decoded.width == 0 || decoded.height == 0 || decoded.width.saturating_mul(decoded.height) > MAX_PIXELS {
        return Err(format!("{name}: the picture is empty or too large"));
    }
    let mut rgba = Vec::with_capacity(decoded.width * decoded.height * 4);
    for p in &decoded.data[..decoded.width * decoded.height] {
        rgba.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8, (p >> 24) as u8]);
    }
    let (w, h, pixels) = scale_down(decoded.width, decoded.height, &rgba, MAX_SIDE);
    let png = Cx::encode_rgba_as_png(w as u32, h as u32, &pixels)?;
    Ok(ToolImage { label: format!("attached: {name}"), png: Arc::from(png) })
}

/// RGBA pixels brought down to at most `max_side` a side (never up).
pub fn scale_down(width: usize, height: usize, rgba: &[u8], max_side: usize) -> (usize, usize, Vec<u8>) {
    let scale = (max_side as f64 / width.max(height) as f64).min(1.0);
    let (w, h) = (((width as f64 * scale).round() as usize).max(1), ((height as f64 * scale).round() as usize).max(1));
    if (w, h) == (width, height) {
        return (w, h, rgba[..width * height * 4].to_vec());
    }
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let y0 = y * height / h;
        let y1 = (((y + 1) * height).div_ceil(h)).clamp(y0 + 1, height);
        for x in 0..w {
            let x0 = x * width / w;
            let x1 = (((x + 1) * width).div_ceil(w)).clamp(x0 + 1, width);
            let mut sum = [0u32; 4];
            for sy in y0..y1 {
                for px in rgba[(sy * width + x0) * 4..(sy * width + x1) * 4].chunks_exact(4) {
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
    (w, h, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_big_picture_is_scaled_to_the_model_and_a_small_one_is_kept() {
        let big = vec![200u8; 2560 * 1600 * 4];
        let (w, h, px) = scale_down(2560, 1600, &big, MAX_SIDE);
        assert_eq!((w, h), (1024, 640));
        assert_eq!(px.len(), 1024 * 640 * 4);
        assert!(px.iter().all(|v| *v == 200));
        let (w, h, _) = scale_down(300, 200, &vec![0u8; 300 * 200 * 4], MAX_SIDE);
        assert_eq!((w, h), (300, 200));
    }

    #[test]
    fn a_screenshot_png_loads_as_a_model_image() {
        let dir = std::env::temp_dir().join(format!("code-pictures-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shot.png");
        let rgba: Vec<u8> = (0..2000 * 1000).flat_map(|i| [(i % 251) as u8, 80, 160, 255]).collect();
        std::fs::write(&path, Cx::encode_rgba_as_png(2000, 1000, &rgba).unwrap()).unwrap();
        let image = load(path.to_str().unwrap()).unwrap();
        assert!(image.png.starts_with(b"\x89PNG\r\n\x1a\n"));
        let w = u32::from_be_bytes(image.png[16..20].try_into().unwrap());
        let h = u32::from_be_bytes(image.png[20..24].try_into().unwrap());
        assert_eq!((w, h), (1024, 512));
        assert_eq!(image.label, "attached: shot.png");
        assert!(load(dir.join("missing.png").to_str().unwrap()).is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }
}
