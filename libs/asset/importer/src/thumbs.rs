//! Thumbnail + media inspection helpers for the worker: byte-level PNG/JPEG
//! dimension probes (no image crate needed for dims), a bounded WAV parser,
//! the canonical 512×512 waveform strip, and JPEG encoding of BGRA pixels.
//!
//! Everything here is pure and hermetically tested; nothing touches the
//! network or the library directory.

/// Canonical generated-thumbnail size.
pub const THUMB_DIM: usize = 512;
/// Bounded WAV decode (30 min at 48 kHz — library clips are far smaller).
pub const MAX_WAV_FRAMES: usize = 48_000 * 60 * 30;

/// PNG pixel dimensions from the IHDR chunk (signature + first chunk).
pub fn png_dims(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[0..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    (width > 0 && height > 0).then_some((width, height))
}

/// JPEG pixel dimensions from the first SOF0/1/2 marker.
pub fn jpeg_dims(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }
    let mut at = 2usize;
    while at + 9 < bytes.len() {
        if bytes[at] != 0xFF {
            return None;
        }
        let marker = bytes[at + 1];
        // Standalone markers without a length.
        if (0xD0..=0xD9).contains(&marker) || marker == 0x01 {
            at += 2;
            continue;
        }
        let len = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
        if len < 2 {
            return None;
        }
        if matches!(marker, 0xC0 | 0xC1 | 0xC2) {
            if at + 9 >= bytes.len() {
                return None;
            }
            let height = u16::from_be_bytes([bytes[at + 5], bytes[at + 6]]) as u32;
            let width = u16::from_be_bytes([bytes[at + 7], bytes[at + 8]]) as u32;
            return (width > 0 && height > 0).then_some((width, height));
        }
        at += 2 + len;
    }
    None
}

/// Interleaved-to-stereo PCM out of a RIFF/WAVE file (PCM16 + float32),
/// bounded — the same shape the VJ and ai-content players parse.
pub struct WavPcm {
    pub frames: Vec<(f32, f32)>,
    pub sample_rate: u32,
}

impl WavPcm {
    pub fn millis(&self) -> u32 {
        if self.sample_rate == 0 {
            return 0;
        }
        ((self.frames.len() as u64 * 1000) / self.sample_rate as u64).min(u32::MAX as u64) as u32
    }
}

pub fn parse_wav(bytes: &[u8]) -> Result<WavPcm, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }
    let mut format = 0u16;
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    let mut bits = 0u16;
    let mut data: Option<&[u8]> = None;
    let mut at = 12usize;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body_end = (at + 8 + size).min(bytes.len());
        let body = &bytes[at + 8..body_end];
        match id {
            b"fmt " if body.len() >= 16 => {
                format = u16::from_le_bytes(body[0..2].try_into().unwrap());
                channels = u16::from_le_bytes(body[2..4].try_into().unwrap());
                sample_rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                bits = u16::from_le_bytes(body[14..16].try_into().unwrap());
            }
            b"data" => data = Some(body),
            _ => {}
        }
        at = body_end + (size & 1);
    }
    let data = data.ok_or("wav: no data chunk")?;
    if channels == 0 || sample_rate == 0 {
        return Err("wav: no fmt chunk".into());
    }
    let ch = channels as usize;
    let mut frames = Vec::new();
    let push = |frames: &mut Vec<(f32, f32)>, l: f32, r: f32| -> Result<(), String> {
        if frames.len() >= MAX_WAV_FRAMES {
            return Err("wav exceeds the decode budget".into());
        }
        frames.push((l, r));
        Ok(())
    };
    match (format, bits) {
        (1, 16) => {
            for frame in data.chunks_exact(2 * ch) {
                let sample = |i: usize| {
                    i16::from_le_bytes(frame[i * 2..i * 2 + 2].try_into().unwrap()) as f32
                        / 32768.0
                };
                push(&mut frames, sample(0), sample(ch - 1))?;
            }
        }
        (3, 32) => {
            for frame in data.chunks_exact(4 * ch) {
                let sample = |i: usize| {
                    f32::from_le_bytes(frame[i * 4..i * 4 + 4].try_into().unwrap())
                        .clamp(-1.0, 1.0)
                };
                push(&mut frames, sample(0), sample(ch - 1))?;
            }
        }
        other => return Err(format!("wav: unsupported format {other:?}")),
    }
    if frames.is_empty() {
        return Err("wav: empty data".into());
    }
    Ok(WavPcm { frames, sample_rate })
}

/// The canonical 512×512 waveform strip, freshly rendered from PCM (never a
/// stale sidecar), as BGRA pixels.
pub fn waveform_bgra_512(pcm: &WavPcm) -> Vec<u32> {
    const BG: u32 = 0xff14_181c;
    const FG: u32 = 0xff58_c4a0;
    const MID: u32 = 0xff2a_3238;
    let (width, height) = (THUMB_DIM, THUMB_DIM);
    let mut out = vec![BG; width * height];
    let mid_y = height / 2;
    for x in 0..width {
        out[mid_y * width + x] = MID;
    }
    if pcm.frames.is_empty() {
        return out;
    }
    let per_col = (pcm.frames.len() as f64 / width as f64).max(1.0);
    for x in 0..width {
        let start = ((x as f64 * per_col) as usize).min(pcm.frames.len() - 1);
        let end = (((x + 1) as f64 * per_col) as usize).clamp(start + 1, pcm.frames.len());
        let (mut lo, mut hi) = (0.0f32, 0.0f32);
        for &(l, r) in &pcm.frames[start..end] {
            let mono = (l + r) * 0.5;
            lo = lo.min(mono);
            hi = hi.max(mono);
        }
        let half = (height / 2) as f32;
        let y0 = (mid_y as f32 - hi.clamp(-1.0, 1.0) * (half - 1.0)) as usize;
        let y1 = (mid_y as f32 - lo.clamp(-1.0, 1.0) * (half - 1.0)) as usize;
        for y in y0.min(height - 1)..=y1.min(height - 1) {
            out[y * width + x] = FG;
        }
    }
    out
}

/// A flat 512×512 placeholder tile (honest "no visual available"), BGRA.
pub fn placeholder_bgra_512() -> Vec<u32> {
    const BG: u32 = 0xff20_262e;
    const GRID: u32 = 0xff2a_323c;
    let mut out = vec![BG; THUMB_DIM * THUMB_DIM];
    for y in 0..THUMB_DIM {
        for x in 0..THUMB_DIM {
            if x % 64 == 0 || y % 64 == 0 {
                out[y * THUMB_DIM + x] = GRID;
            }
        }
    }
    out
}

/// Encode BGRA pixels as a JPEG (quality 90).
pub fn encode_jpeg_bgra(bgra: &[u32], width: usize, height: usize) -> Result<Vec<u8>, String> {
    let bytes: &[u8] = bytemuck_cast(bgra);
    let mut out = Vec::new();
    let encoder = jpeg_encoder::Encoder::new(&mut out, 90);
    encoder
        .encode(bytes, width as u16, height as u16, jpeg_encoder::ColorType::Bgra)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// Little-endian `u32` BGRA words viewed as bytes (B,G,R,A order — exactly
/// the encoder's `ColorType::Bgra`).
fn bytemuck_cast(words: &[u32]) -> &[u8] {
    // SAFETY: u32 -> u8 view of the same allocation; length scales by 4;
    // u8 has no alignment requirement.
    unsafe { std::slice::from_raw_parts(words.as_ptr() as *const u8, words.len() * 4) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav_pcm16(frames: &[(i16, i16)], rate: u32) -> Vec<u8> {
        let mut data = Vec::new();
        for (l, r) in frames {
            data.extend_from_slice(&l.to_le_bytes());
            data.extend_from_slice(&r.to_le_bytes());
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * 4).to_le_bytes());
        out.extend_from_slice(&4u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);
        out
    }

    #[test]
    fn png_and_jpeg_dims_parse_and_refuse_garbage() {
        // Minimal PNG header with IHDR 640×352.
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&640u32.to_be_bytes());
        png.extend_from_slice(&352u32.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0]);
        assert_eq!(png_dims(&png), Some((640, 352)));
        assert_eq!(png_dims(b"not a png"), None);

        // A real (tiny) JPEG via the encoder round-trips its dimensions.
        let bgra = vec![0xff33_66aa_u32; 32 * 16];
        let jpeg = encode_jpeg_bgra(&bgra, 32, 16).unwrap();
        assert_eq!(jpeg_dims(&jpeg), Some((32, 16)));
        assert_eq!(jpeg_dims(b"\xff\xd8junk"), None);
        assert_eq!(jpeg_dims(&png), None);
    }

    #[test]
    fn wav_parse_waveform_and_thumbnail_encode() {
        let frames: Vec<(i16, i16)> =
            (0..2_000).map(|i| if i % 2 == 0 { (12_000, 12_000) } else { (-9_000, -9_000) }).collect();
        let pcm = parse_wav(&wav_pcm16(&frames, 24_000)).unwrap();
        assert_eq!(pcm.frames.len(), 2_000);
        assert_eq!(pcm.millis(), 2_000 * 1000 / 24_000);
        let strip = waveform_bgra_512(&pcm);
        assert_eq!(strip.len(), THUMB_DIM * THUMB_DIM);
        // The strip carries signal (not all background).
        assert!(strip.iter().any(|p| *p == 0xff58_c4a0));
        let jpeg = encode_jpeg_bgra(&strip, THUMB_DIM, THUMB_DIM).unwrap();
        assert_eq!(jpeg_dims(&jpeg), Some((THUMB_DIM as u32, THUMB_DIM as u32)));
        // Garbage refuses.
        assert!(parse_wav(b"garbage").is_err());
        // Placeholder is well-formed too.
        let jpeg = encode_jpeg_bgra(&placeholder_bgra_512(), THUMB_DIM, THUMB_DIM).unwrap();
        assert_eq!(jpeg_dims(&jpeg), Some((512, 512)));
    }
}
