use crate::document::PdfDocument;
use crate::filter;
use crate::lexer::*;
use crate::object::*;

/// Decoded image data ready for rendering.
#[derive(Clone, Debug)]
pub struct PdfImage {
    pub width: usize,
    pub height: usize,
    /// RGBA pixel data, 4 bytes per pixel.
    pub rgba: Vec<u8>,
    /// If true, the raw stream is JPEG data that can be passed directly to a JPEG decoder.
    pub is_jpeg: bool,
    /// Raw JPEG/PNG data if applicable (for passing to external decoders).
    pub raw_data: Option<Vec<u8>>,
}

/// Extract and decode an XObject image.
pub fn extract_image(doc: &mut PdfDocument, obj_ref: ObjRef) -> PdfResult<PdfImage> {
    let obj = doc.resolve_ref(obj_ref)?;
    let stream = obj
        .as_stream()
        .ok_or_else(|| PdfError::new("image XObject is not a stream"))?;

    let dict = &stream.dict;
    let width = dict
        .get_int("Width")
        .ok_or_else(|| PdfError::new("image missing /Width"))? as usize;
    let height = dict
        .get_int("Height")
        .ok_or_else(|| PdfError::new("image missing /Height"))? as usize;
    let bpc = dict.get_int("BitsPerComponent").unwrap_or(8) as usize;

    // Check if it's a JPEG (DCTDecode) — pass raw data through
    let filter_name = dict.get_name("Filter").unwrap_or("");
    if filter_name == "DCTDecode" {
        return Ok(PdfImage {
            width,
            height,
            rgba: Vec::new(),
            is_jpeg: true,
            raw_data: Some(stream.data.clone()),
        });
    }

    // Decompress the stream
    let decoded = filter::decode_stream(&stream.data, dict)?;

    // Determine color space
    let color_space = dict
        .get("ColorSpace")
        .and_then(|cs| match cs {
            PdfObj::Name(n) => Some(n.as_str().to_string()),
            PdfObj::Array(arr) => arr.first().and_then(|o| o.as_name()).map(|s| s.to_string()),
            _ => None,
        })
        .unwrap_or_else(|| "DeviceRGB".to_string());

    let is_mask = dict
        .get("ImageMask")
        .and_then(|o| o.as_bool())
        .unwrap_or(false);

    // Convert to RGBA
    let rgba = convert_to_rgba(&decoded, width, height, bpc, &color_space, is_mask);

    Ok(PdfImage {
        width,
        height,
        rgba,
        is_jpeg: false,
        raw_data: None,
    })
}

/// Extract and decode an inline image from content stream.
pub fn decode_inline_image(dict: &PdfDict, data: &[u8]) -> PdfResult<PdfImage> {
    let width = dict
        .get_int("Width")
        .ok_or_else(|| PdfError::new("inline image missing Width"))? as usize;
    let height = dict
        .get_int("Height")
        .ok_or_else(|| PdfError::new("inline image missing Height"))? as usize;
    let bpc = dict.get_int("BitsPerComponent").unwrap_or(8) as usize;

    let filter_name = dict.get_name("Filter").unwrap_or("");

    // Check for JPEG
    if filter_name == "DCTDecode" || filter_name == "DCT" {
        return Ok(PdfImage {
            width,
            height,
            rgba: Vec::new(),
            is_jpeg: true,
            raw_data: Some(data.to_vec()),
        });
    }

    // Decompress
    let decoded = filter::decode_stream(data, dict)?;

    let color_space = resolve_inline_colorspace(dict.get_name("ColorSpace").unwrap_or("DeviceRGB"));

    let is_mask = dict
        .get("ImageMask")
        .and_then(|o| o.as_bool())
        .unwrap_or(false);

    let rgba = convert_to_rgba(&decoded, width, height, bpc, &color_space, is_mask);

    Ok(PdfImage {
        width,
        height,
        rgba,
        is_jpeg: false,
        raw_data: None,
    })
}

fn resolve_inline_colorspace(name: &str) -> String {
    match name {
        "G" => "DeviceGray".to_string(),
        "RGB" => "DeviceRGB".to_string(),
        "CMYK" => "DeviceCMYK".to_string(),
        "I" => "Indexed".to_string(),
        other => other.to_string(),
    }
}

fn convert_to_rgba(
    data: &[u8],
    width: usize,
    height: usize,
    bpc: usize,
    color_space: &str,
    is_mask: bool,
) -> Vec<u8> {
    let pixels = width * height;
    let mut rgba = vec![255u8; pixels * 4]; // default white, fully opaque

    if is_mask || color_space == "DeviceGray" {
        if bpc == 1 {
            // 1-bit: each byte has 8 pixels
            for y in 0..height {
                for x in 0..width {
                    let byte_idx = y * ((width + 7) / 8) + x / 8;
                    let bit = 7 - (x % 8);
                    let val = if byte_idx < data.len() {
                        if (data[byte_idx] >> bit) & 1 == 1 {
                            0u8
                        } else {
                            255u8
                        }
                    } else {
                        255
                    };
                    let px = (y * width + x) * 4;
                    if px + 3 < rgba.len() {
                        if is_mask {
                            rgba[px] = 0;
                            rgba[px + 1] = 0;
                            rgba[px + 2] = 0;
                            rgba[px + 3] = 255 - val;
                        } else {
                            rgba[px] = val;
                            rgba[px + 1] = val;
                            rgba[px + 2] = val;
                            rgba[px + 3] = 255;
                        }
                    }
                }
            }
        } else if bpc == 8 {
            for i in 0..pixels.min(data.len()) {
                let px = i * 4;
                if px + 3 < rgba.len() {
                    rgba[px] = data[i];
                    rgba[px + 1] = data[i];
                    rgba[px + 2] = data[i];
                    rgba[px + 3] = 255;
                }
            }
        }
    } else if color_space == "DeviceRGB" {
        for i in 0..pixels {
            let src = i * 3;
            let dst = i * 4;
            if src + 2 < data.len() && dst + 3 < rgba.len() {
                rgba[dst] = data[src];
                rgba[dst + 1] = data[src + 1];
                rgba[dst + 2] = data[src + 2];
                rgba[dst + 3] = 255;
            }
        }
    } else if color_space == "DeviceCMYK" {
        for i in 0..pixels {
            let src = i * 4;
            let dst = i * 4;
            if src + 3 < data.len() && dst + 3 < rgba.len() {
                let c = data[src] as f32 / 255.0;
                let m = data[src + 1] as f32 / 255.0;
                let y = data[src + 2] as f32 / 255.0;
                let k = data[src + 3] as f32 / 255.0;
                rgba[dst] = (255.0 * (1.0 - c) * (1.0 - k)) as u8;
                rgba[dst + 1] = (255.0 * (1.0 - m) * (1.0 - k)) as u8;
                rgba[dst + 2] = (255.0 * (1.0 - y) * (1.0 - k)) as u8;
                rgba[dst + 3] = 255;
            }
        }
    } else {
        // Unknown color space: treat as grayscale
        for i in 0..pixels.min(data.len()) {
            let px = i * 4;
            if px + 3 < rgba.len() {
                rgba[px] = data[i];
                rgba[px + 1] = data[i];
                rgba[px + 2] = data[i];
                rgba[px + 3] = 255;
            }
        }
    }

    rgba
}
