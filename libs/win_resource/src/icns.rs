//! Apple icon (.icns) reader: the same file the macOS bundles use as
//! AppIcon.icns. Modern entries hold PNG data; the 16 and 32 px `ic04`/`ic05`
//! entries written by iconutil hold `ARGB` planes in Apple's PackBits variant.
use crate::image::{Image, PNG_SIGNATURE};

#[derive(Clone, Debug, PartialEq)]
pub struct IcnsEntry {
    pub ostype: [u8; 4],
    pub data: Vec<u8>,
}

/// A decoded source picture, with its original PNG bytes when it had any, so
/// a 256 px icon can be stored without re-encoding.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceImage {
    pub image: Image,
    pub png: Option<Vec<u8>>,
}

pub fn parse_icns(bytes: &[u8]) -> Result<Vec<IcnsEntry>, String> {
    if bytes.len() < 8 || &bytes[..4] != b"icns" {
        return Err("not an .icns file".into());
    }
    let total = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
    if total > bytes.len() || total < 8 {
        return Err(format!(".icns header says {total} bytes, file has {}", bytes.len()));
    }
    let mut entries = Vec::new();
    let mut at = 8;
    while at < total {
        if at + 8 > total {
            return Err(format!(".icns entry header truncated at {at}"));
        }
        let ostype: [u8; 4] = bytes[at..at + 4].try_into().unwrap();
        let len = u32::from_be_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        if len < 8 || at + len > total {
            return Err(format!(".icns entry {} at {at} has bad length {len}", String::from_utf8_lossy(&ostype)));
        }
        entries.push(IcnsEntry { ostype, data: bytes[at + 8..at + len].to_vec() });
        at += len;
    }
    Ok(entries)
}

/// Decode an entry into pixels. `None` for entries that are not pictures
/// (`info`, `TOC `, `name`) or use formats we do not read (legacy RGB with
/// separate masks, JPEG 2000); the modern entries always cover those sizes.
pub fn decode_entry(entry: &IcnsEntry) -> Option<Result<SourceImage, String>> {
    if entry.data.starts_with(PNG_SIGNATURE) {
        return Some(Image::decode_png(&entry.data).map(|image| SourceImage { image, png: Some(entry.data.clone()) }));
    }
    if entry.data.starts_with(b"ARGB") {
        let size = match &entry.ostype {
            b"ic04" => 16,
            b"ic05" => 32,
            b"icsb" => 18,
            _ => return None,
        };
        return Some(decode_argb(&entry.data[4..], size).map(|image| SourceImage { image, png: None }));
    }
    None
}

pub fn decode_icns(bytes: &[u8]) -> Result<Vec<SourceImage>, String> {
    let mut images = Vec::new();
    for entry in parse_icns(bytes)? {
        if let Some(image) = decode_entry(&entry) {
            images.push(image.map_err(|e| format!(".icns entry {}: {e}", String::from_utf8_lossy(&entry.ostype)))?);
        }
    }
    Ok(images)
}

/// Four planes (A, R, G, B) of `size * size` bytes, each run-length encoded:
/// a header byte below 0x80 copies the next `n + 1` bytes, otherwise the next
/// byte repeats `n - 125` times. Planes may continue across one run.
fn decode_argb(data: &[u8], size: u32) -> Result<Image, String> {
    let pixels = (size * size) as usize;
    let mut planes = Vec::with_capacity(pixels * 4);
    let mut at = 0;
    while planes.len() < pixels * 4 {
        let n = *data.get(at).ok_or("ARGB data truncated")? as usize;
        at += 1;
        if n < 0x80 {
            let run = data.get(at..at + n + 1).ok_or("ARGB literal run truncated")?;
            planes.extend_from_slice(run);
            at += n + 1;
        } else {
            let value = *data.get(at).ok_or("ARGB repeat run truncated")?;
            planes.extend(std::iter::repeat(value).take(n - 125));
            at += 1;
        }
    }
    if planes.len() != pixels * 4 {
        return Err(format!("ARGB planes decoded to {} bytes, expected {}", planes.len(), pixels * 4));
    }
    let mut rgba = Vec::with_capacity(pixels * 4);
    for i in 0..pixels {
        rgba.extend_from_slice(&[planes[pixels + i], planes[2 * pixels + i], planes[3 * pixels + i], planes[i]]);
    }
    Image::new(size, size, rgba)
}

/// Encode `ARGB` planes the way iconutil does; used to test the decoder.
#[cfg(test)]
pub(crate) fn encode_argb(image: &Image) -> Vec<u8> {
    let pixels = (image.width * image.height) as usize;
    let mut planes = Vec::with_capacity(pixels * 4);
    for channel in [3, 0, 1, 2] {
        planes.extend((0..pixels).map(|i| image.rgba[i * 4 + channel]));
    }
    let mut out = b"ARGB".to_vec();
    let mut i = 0;
    while i < planes.len() {
        let mut run = 1;
        while i + run < planes.len() && planes[i + run] == planes[i] && run < 130 { run += 1; }
        if run >= 3 {
            out.extend_from_slice(&[(run + 125) as u8, planes[i]]);
            i += run;
        } else {
            let start = i;
            while i < planes.len() && i - start < 128 {
                if i + 2 < planes.len() && planes[i] == planes[i + 1] && planes[i] == planes[i + 2] { break; }
                i += 1;
            }
            out.push((i - start - 1) as u8);
            out.extend_from_slice(&planes[start..i]);
        }
    }
    out
}
