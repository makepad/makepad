//! Thumbnail + media inspection helpers for the worker: byte-level PNG/JPEG
//! dimension probes (no image crate needed for dims), a bounded WAV parser,
//! the canonical 512×512 waveform strip, and JPEG encoding of BGRA pixels.
//!
//! Everything here is pure and hermetically tested; nothing touches the
//! network or the library directory.

use makepad_asset_data::MediaType;
use makepad_audio_decode::{decode_audio_limited, AudioFormat, Limits};

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
        // 8-bit unsigned PCM (classic game sfx: Quake II, Doom).
        (1, 8) => {
            for frame in data.chunks_exact(ch) {
                let sample = |i: usize| (frame[i] as f32 - 128.0) / 128.0;
                push(&mut frames, sample(0), sample(ch - 1))?;
            }
        }
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

/// Decode any audio media the catalog carries into the same bounded PCM the
/// waveform strip and the duration measurement want.
///
/// WAV parses here; MP3 and Ogg Vorbis go through this repo's own decoders
/// (`makepad-audio-decode`), so importing a music library needs no platform
/// codec and behaves identically on every host the worker runs on.
pub fn decode_audio(bytes: &[u8], media: MediaType) -> Result<WavPcm, String> {
    let format = match media {
        MediaType::Wav => return parse_wav(bytes),
        MediaType::Mp3 => AudioFormat::Mp3,
        MediaType::Ogg => AudioFormat::OggVorbis,
        other => return Err(format!("not an audio media type: {other:?}")),
    };
    let audio = decode_audio_limited(bytes, format, Limits::with_max_frames(MAX_WAV_FRAMES))
        .map_err(|e| format!("{format:?}: {e}"))?;
    let channels = audio.channels.max(1) as usize;
    let mut frames = Vec::with_capacity(audio.frames());
    for frame in audio.pcm_interleaved_f32.chunks_exact(channels) {
        frames.push((frame[0].clamp(-1.0, 1.0), frame[channels - 1].clamp(-1.0, 1.0)));
    }
    if frames.is_empty() {
        return Err(format!("{format:?}: empty data"));
    }
    Ok(WavPcm { frames, sample_rate: audio.rate.max(1) })
}

/// Duration in milliseconds without decoding the audio, for the formats that
/// carry it in a header. Falls back to a full decode for WAV, which is cheap.
///
/// This answers for the same samples [`decode_audio`] returns, which for MP3
/// means the gapless trim comes off the header's frame count: an importer that
/// stores this and later renders a waveform from the PCM must not find the two
/// disagreeing by the encoder's delay and padding.
pub fn audio_millis(bytes: &[u8], media: MediaType) -> Result<u32, String> {
    let secs = match media {
        MediaType::Mp3 => {
            let decoder = makepad_audio_decode::mp3::Mp3Decoder::new(bytes)
                .map_err(|e| e.to_string())?;
            let rate = decoder.rate().max(1) as f64;
            let (front, back) = decoder.trim();
            let total = makepad_audio_decode::mp3::probe_duration(bytes)
                .map_err(|e| e.to_string())?;
            (total - (front + back) as f64 / rate).max(0.0)
        }
        MediaType::Ogg => {
            makepad_audio_decode::vorbis::probe_duration(bytes).map_err(|e| e.to_string())?
        }
        other => return Ok(decode_audio(bytes, other)?.millis()),
    };
    Ok((secs * 1000.0).clamp(0.0, u32::MAX as f64) as u32)
}

/// The canonical 512×512 picture of an audio asset, freshly rendered from
/// PCM (never a stale sidecar), as BGRA pixels.
///
/// A SPECTROGRAM, not a waveform: every mastered track's waveform is the
/// same filled rectangle, while a spectrogram shows what the thing is — a
/// beat, a voice, a pad, a field recording — at icon size. Silence and
/// scraps too short to transform fall back to the waveform, which at least
/// says "there is nothing here" honestly.
pub fn waveform_bgra_512(pcm: &WavPcm) -> Vec<u32> {
    let mono: Vec<f32> = pcm.frames.iter().map(|(l, r)| (l + r) * 0.5).collect();
    if let Some(rgba) =
        crate::spectrogram::spectrogram_rgba(&mono, pcm.sample_rate, THUMB_DIM, THUMB_DIM)
    {
        return rgba
            .chunks_exact(4)
            .map(|px| {
                (px[3] as u32) << 24 | (px[0] as u32) << 16 | (px[1] as u32) << 8 | px[2] as u32
            })
            .collect();
    }
    waveform_strip_bgra_512(pcm)
}

/// The old min/max strip: the honest picture of something with no spectrum
/// to show.
fn waveform_strip_bgra_512(pcm: &WavPcm) -> Vec<u32> {
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

/// Tolerance for "this image is one flat colour": the importer's own
/// placeholder tile is a dark ground with a slightly lighter 64px grid, so a
/// hair more than exact equality is needed to recognise it.
const PLACEHOLDER_RANGE: u8 = 18;

/// Is this thumbnail a placeholder rather than a picture of the asset?
///
/// The catalog must not fill up with rows whose thumbnail is the "no visual
/// available" tile, a flat colour, or a fully transparent image — a grid of
/// those is indistinguishable from a broken import. Undecodable bytes are
/// NOT called placeholders: the pack validator refuses them on its own terms.
pub fn thumbnail_is_placeholder(bytes: &[u8]) -> bool {
    let Some((rgba, w, h)) = decode_rgba(bytes) else {
        return false;
    };
    let pixels = (w as usize) * (h as usize);
    if pixels == 0 || rgba.len() < pixels * 4 {
        return false;
    }
    // Sample at most ~65k pixels: a 4096² thumbnail must not cost a second.
    let stride = (pixels / 65_536).max(1);
    let mut lo = [255u8; 3];
    let mut hi = [0u8; 3];
    let mut max_alpha = 0u8;
    let mut seen = 0usize;
    for i in (0..pixels).step_by(stride) {
        let p = &rgba[i * 4..i * 4 + 4];
        for c in 0..3 {
            lo[c] = lo[c].min(p[c]);
            hi[c] = hi[c].max(p[c]);
        }
        max_alpha = max_alpha.max(p[3]);
        seen += 1;
    }
    if seen < 16 {
        return false;
    }
    if max_alpha < 8 {
        return true;
    }
    (0..3).all(|c| hi[c].saturating_sub(lo[c]) <= PLACEHOLDER_RANGE)
}

/// Decode PNG or JPEG bytes to RGBA. Only what [`thumbnail_is_placeholder`]
/// needs — the pack path validates the containers separately.
fn decode_rgba(bytes: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        use makepad_zune_png::makepad_zune_core::bytestream::ZCursor;
        use makepad_zune_png::makepad_zune_core::colorspace::ColorSpace;
        let mut dec = makepad_zune_png::PngDecoder::new(ZCursor::new(bytes));
        let pixels = dec.decode_raw().ok()?;
        let (w, h) = dec.dimensions()?;
        let channels = match dec.colorspace()? {
            ColorSpace::RGBA => 4,
            ColorSpace::RGB => 3,
            ColorSpace::Luma => 1,
            ColorSpace::LumaA => 2,
            _ => return None,
        };
        return Some((to_rgba(&pixels, w * h, channels), w as u32, h as u32));
    }
    if bytes.starts_with(&[0xff, 0xd8]) {
        use makepad_zune_jpeg::makepad_zune_core::bytestream::ZCursor;
        use makepad_zune_jpeg::makepad_zune_core::colorspace::ColorSpace;
        use makepad_zune_jpeg::makepad_zune_core::options::DecoderOptions;
        let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
        let mut dec =
            makepad_zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), options);
        let pixels = dec.decode().ok()?;
        let (w, h) = dec.dimensions()?;
        let channels = if pixels.len() >= w * h * 4 { 4 } else { 3 };
        return Some((to_rgba(&pixels, w * h, channels), w as u32, h as u32));
    }
    None
}

fn to_rgba(src: &[u8], pixels: usize, channels: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixels * 4);
    for i in 0..pixels {
        let p = i * channels;
        if p + channels > src.len() {
            break;
        }
        match channels {
            4 => out.extend_from_slice(&src[p..p + 4]),
            3 => out.extend_from_slice(&[src[p], src[p + 1], src[p + 2], 255]),
            2 => out.extend_from_slice(&[src[p], src[p], src[p], src[p + 1]]),
            _ => out.extend_from_slice(&[src[p], src[p], src[p], 255]),
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
        // The fixture alternates every sample: a tone at Nyquist. Its
        // picture is bright along the TOP (the highest band) and dark in
        // the middle — which is exactly what a spectrogram should say
        // about it, and what a waveform strip could never show.
        let brightness = |y: usize| {
            let p = strip[y * THUMB_DIM + THUMB_DIM / 2];
            ((p >> 16) & 0xff) + ((p >> 8) & 0xff) + (p & 0xff)
        };
        assert!(
            brightness(0) > brightness(THUMB_DIM / 2),
            "a Nyquist tone lights the top band: {} vs {}",
            brightness(0),
            brightness(THUMB_DIM / 2)
        );
        // Silence has no spectrum, and falls back to the honest strip.
        let quiet = parse_wav(&wav_pcm16(&vec![(0, 0); 2_000], 24_000)).unwrap();
        let quiet_strip = waveform_bgra_512(&quiet);
        assert!(
            quiet_strip.iter().any(|p| *p == 0xff58_c4a0),
            "digital silence falls back to the strip and draws its flat line"
        );
        let jpeg = encode_jpeg_bgra(&strip, THUMB_DIM, THUMB_DIM).unwrap();
        assert_eq!(jpeg_dims(&jpeg), Some((THUMB_DIM as u32, THUMB_DIM as u32)));
        // Garbage refuses.
        assert!(parse_wav(b"garbage").is_err());
        // Placeholder is well-formed too.
        let jpeg = encode_jpeg_bgra(&placeholder_bgra_512(), THUMB_DIM, THUMB_DIM).unwrap();
        assert_eq!(jpeg_dims(&jpeg), Some((512, 512)));
    }

    #[test]
    fn decode_audio_routes_by_media_and_refuses_the_rest() {
        let frames: Vec<(i16, i16)> = (0..500).map(|i| (i as i16 * 60, -(i as i16) * 60)).collect();
        let wav = wav_pcm16(&frames, 24_000);
        // WAV still goes through the RIFF parser, byte for byte.
        let direct = parse_wav(&wav).unwrap();
        let routed = decode_audio(&wav, MediaType::Wav).unwrap();
        assert_eq!(direct.sample_rate, routed.sample_rate);
        assert_eq!(direct.frames.len(), routed.frames.len());
        assert_eq!(audio_millis(&wav, MediaType::Wav).unwrap(), direct.millis());

        // Compressed media that is not actually compressed audio errors with
        // the format named, and never panics.
        assert!(decode_audio(b"not an mp3", MediaType::Mp3).is_err());
        assert!(decode_audio(b"not an ogg", MediaType::Ogg).is_err());
        assert!(decode_audio(&[], MediaType::Mp3).is_err());
        assert!(audio_millis(b"OggS not really", MediaType::Ogg).is_err());
        // Non-audio media is refused by name rather than misparsed.
        match decode_audio(&wav, MediaType::Glb) {
            Err(err) => assert!(err.contains("Glb"), "{err}"),
            Ok(_) => panic!("a GLB is not audio"),
        }
    }
}

#[cfg(test)]
mod placeholder_tests {
    use super::*;
    use crate::classic_import::encode_png_rgba;

    fn png(pixels: impl Fn(u32, u32) -> [u8; 4], w: u32, h: u32) -> Vec<u8> {
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(&pixels(x, y));
            }
        }
        encode_png_rgba(&rgba, w, h).unwrap()
    }

    #[test]
    fn a_flat_tile_is_a_placeholder() {
        let flat = png(|_, _| [32, 38, 46, 255], 256, 256);
        assert!(thumbnail_is_placeholder(&flat));
    }

    #[test]
    fn the_importers_own_placeholder_tile_is_caught() {
        // Same shape as `placeholder_bgra_512`: dark ground, faint grid.
        let tile = png(
            |x, y| {
                if x % 64 == 0 || y % 64 == 0 {
                    [60, 50, 42, 255]
                } else {
                    [46, 38, 32, 255]
                }
            },
            512,
            512,
        );
        assert!(thumbnail_is_placeholder(&tile));
    }

    #[test]
    fn a_fully_transparent_image_is_a_placeholder() {
        let empty = png(|_, _| [200, 30, 30, 0], 256, 256);
        assert!(thumbnail_is_placeholder(&empty));
    }

    #[test]
    fn a_real_render_is_kept() {
        let render = png(
            |x, y| {
                let v = ((x * 3 + y * 5) % 200) as u8;
                [v, 40 + v / 2, 255 - v, 255]
            },
            256,
            256,
        );
        assert!(!thumbnail_is_placeholder(&render));
    }

    #[test]
    fn an_animated_strip_of_sprites_is_kept() {
        // 1024x256 strip: dark studio clear with one bright sprite tile.
        let strip = png(
            |x, y| {
                if x < 128 && y < 128 && (x + y) % 3 != 0 {
                    [220, 180, 60, 255]
                } else {
                    [26, 31, 41, 255]
                }
            },
            1024,
            256,
        );
        assert!(!thumbnail_is_placeholder(&strip));
    }

    #[test]
    fn undecodable_bytes_are_not_called_placeholders() {
        assert!(!thumbnail_is_placeholder(b"not an image"));
    }
}
