//! Turning a track's BYTES into the two pictures a well draws.
//!
//! A thumbnail is a picture of a track someone baked once, at one size, at
//! one moment. It is the right thing to show instantly — it is already in
//! the grid — but it is not the track. Given the actual file, a well can
//! draw the spectrogram and the waveform of THIS clip, at the size it has,
//! and let the viewer switch between them.
//!
//! Decoding a six-minute MP3 and transforming it two thousand times is not
//! something to do on the frame thread, so this module owns a worker: hand
//! it bytes, poll it, take the pictures when they land. It never opens a
//! file and never asks anyone for anything — the host supplies the bytes,
//! which is the same seam every other widget here keeps.

use makepad_audio_decode::{decode_audio_limited, AudioFormat, Limits};
use makepad_audio_picture::spectrogram::spectrogram_rgba;
use makepad_audio_picture::wave::{wave_rgba, WavePalette};
use makepad_widgets::*;
use makepad_widgets::makepad_platform::thread::{Lane, TaskHandle};

/// Decode ceiling: thirty minutes at 48 kHz, the same bound the importer
/// uses. A well is not the place to discover a two-hour file.
const MAX_FRAMES: usize = 48_000 * 60 * 30;

/// Picture size. The same high definition the catalog bakes at, so a well
/// and a thumbnail are the same picture rather than two resolutions of an
/// idea, and wide enough that a bar of music is tens of pixels.
const PICTURE_W: usize = 2048;
const FFT_H: usize = 512;
const WAVE_H: usize = 256;

/// Which picture of the clip a well is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ClipFace {
    /// The log-frequency spectrogram: what the piece IS.
    Fft,
    /// The waveform: where the loud parts are, and the thing you scrub
    /// against. The default, because a preview well with a transport is a
    /// player, and a player shows a waveform.
    #[default]
    Wave,
}

impl ClipFace {
    pub fn other(self) -> Self {
        match self {
            ClipFace::Fft => ClipFace::Wave,
            ClipFace::Wave => ClipFace::Fft,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ClipFace::Fft => "FFT",
            ClipFace::Wave => "WAVE",
        }
    }
}

/// The container a host says its bytes are in. The well does not sniff:
/// guessing a codec from a magic number is the host's job, because the host
/// is the one that knows what it fetched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipFormat {
    Wav,
    Mp3,
    Ogg,
}

/// What a finished decode produced: both pictures, and how long the clip is.
pub struct ClipPictures {
    pub fft: Option<(Vec<u8>, usize, usize)>,
    pub wave: Option<(Vec<u8>, usize, usize)>,
    pub millis: u32,
}

/// A decode in flight, or the pictures it produced.
#[derive(Default)]
pub struct ClipDecoder {
    pending: Option<TaskHandle<ClipPictures>>,
    /// Bumped per request, so a late result from an abandoned clip is
    /// dropped rather than drawn over the current one.
    generation: u64,
}

impl ClipDecoder {
    /// Start decoding and rendering. Any decode already running is
    /// abandoned — its channel is dropped, the thread finds nobody home.
    pub fn start(&mut self, cx: &Cx, bytes: Vec<u8>, format: ClipFormat) {
        self.generation = self.generation.wrapping_add(1);
        if let Some(pending) = &self.pending {
            pending.cancel();
        }
        self.pending = cx
            .task_pool()
            .submit(Lane::Heavy, move || render(&bytes, format))
            .ok();
    }

    /// Forget any decode in flight and any pictures from one.
    pub fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if let Some(pending) = &self.pending {
            pending.cancel();
        }
        self.pending = None;
    }

    pub fn is_running(&self) -> bool {
        self.pending.is_some()
    }

    /// The pictures, once. `None` while the worker is still going; the
    /// receiver is dropped either way once it has answered.
    pub fn poll(&mut self) -> Option<ClipPictures> {
        let done = self.pending.as_mut()?.try_take()?;
        self.pending = None;
        done.ok()
    }
}

/// RGBA8 pixels into a texture, ready to hand an `Image`.
pub fn texture_from_rgba(cx: &mut Cx, rgba: &[u8], width: usize, height: usize) -> Texture {
    let texture = Texture::new_with_format(
        cx,
        TextureFormat::VecBGRAu8_32 {
            width,
            height,
            data: Some(
                rgba.chunks_exact(4)
                    .map(|px| {
                        (px[3] as u32) << 24
                            | (px[0] as u32) << 16
                            | (px[1] as u32) << 8
                            | px[2] as u32
                    })
                    .collect(),
            ),
            updated: TextureUpdated::Full,
        },
    );
    texture
}

fn render(bytes: &[u8], format: ClipFormat) -> ClipPictures {
    let Some((mono, rate, frames)) = decode(bytes, format) else {
        return ClipPictures { fft: None, wave: None, millis: 0 };
    };
    let millis = if rate == 0 {
        0
    } else {
        ((frames as u64 * 1000) / rate as u64).min(u32::MAX as u64) as u32
    };
    ClipPictures {
        fft: spectrogram_rgba(&mono, rate, PICTURE_W, FFT_H)
            .map(|rgba| (rgba, PICTURE_W, FFT_H)),
        wave: wave_rgba(&mono, PICTURE_W, WAVE_H, WavePalette::default())
            .map(|rgba| (rgba, PICTURE_W, WAVE_H)),
        millis,
    }
}

/// Mono samples + rate + frame count out of a container.
fn decode(bytes: &[u8], format: ClipFormat) -> Option<(Vec<f32>, u32, usize)> {
    if let ClipFormat::Wav = format {
        return decode_wav(bytes);
    }
    let format = match format {
        ClipFormat::Mp3 => AudioFormat::Mp3,
        ClipFormat::Ogg => AudioFormat::OggVorbis,
        ClipFormat::Wav => unreachable!(),
    };
    let audio = decode_audio_limited(bytes, format, Limits::with_max_frames(MAX_FRAMES)).ok()?;
    let channels = audio.channels.max(1) as usize;
    let mono: Vec<f32> = audio
        .pcm_interleaved_f32
        .chunks_exact(channels)
        .map(|frame| (frame[0] + frame[channels - 1]) * 0.5)
        .collect();
    let frames = mono.len();
    (frames > 0).then(|| (mono, audio.rate.max(1), frames))
}

/// RIFF/WAVE, PCM8/PCM16/float32 — the same shapes the catalog carries.
fn decode_wav(bytes: &[u8]) -> Option<(Vec<f32>, u32, usize)> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let (mut format, mut channels, mut rate, mut bits) = (0u16, 0u16, 0u32, 0u16);
    let mut data: Option<&[u8]> = None;
    let mut at = 12usize;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().ok()?) as usize;
        let end = (at + 8 + size).min(bytes.len());
        let body = &bytes[at + 8..end];
        match id {
            b"fmt " if body.len() >= 16 => {
                format = u16::from_le_bytes(body[0..2].try_into().ok()?);
                channels = u16::from_le_bytes(body[2..4].try_into().ok()?);
                rate = u32::from_le_bytes(body[4..8].try_into().ok()?);
                bits = u16::from_le_bytes(body[14..16].try_into().ok()?);
            }
            b"data" => data = Some(body),
            _ => {}
        }
        at = end + (size & 1);
    }
    let (data, ch) = (data?, channels.max(1) as usize);
    if rate == 0 {
        return None;
    }
    let mut mono = Vec::new();
    let mut push = |l: f32, r: f32| {
        if mono.len() < MAX_FRAMES {
            mono.push((l + r) * 0.5);
        }
    };
    match (format, bits) {
        (1, 8) => {
            for frame in data.chunks_exact(ch) {
                let s = |i: usize| (frame[i] as f32 - 128.0) / 128.0;
                push(s(0), s(ch - 1));
            }
        }
        (1, 16) => {
            for frame in data.chunks_exact(2 * ch) {
                let s = |i: usize| {
                    i16::from_le_bytes(frame[i * 2..i * 2 + 2].try_into().unwrap()) as f32 / 32768.0
                };
                push(s(0), s(ch - 1));
            }
        }
        (3, 32) => {
            for frame in data.chunks_exact(4 * ch) {
                let s = |i: usize| {
                    f32::from_le_bytes(frame[i * 4..i * 4 + 4].try_into().unwrap()).clamp(-1.0, 1.0)
                };
                push(s(0), s(ch - 1));
            }
        }
        _ => return None,
    }
    let frames = mono.len();
    (frames > 0).then(|| (mono, rate, frames))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn wav_pcm16(frames: &[(i16, i16)], rate: u32) -> Vec<u8> {
        let mut out = b"RIFF".to_vec();
        let data_len = frames.len() * 4;
        out.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * 4).to_le_bytes());
        out.extend_from_slice(&4u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data_len as u32).to_le_bytes());
        for (l, r) in frames {
            out.extend_from_slice(&l.to_le_bytes());
            out.extend_from_slice(&r.to_le_bytes());
        }
        out
    }

    fn tone(rate: u32, secs: f32) -> Vec<(i16, i16)> {
        let n = (rate as f32 * secs) as usize;
        (0..n)
            .map(|i| {
                let v = ((2.0 * PI * 220.0 * i as f32 / rate as f32).sin() * 12_000.0) as i16;
                (v, v)
            })
            .collect()
    }

    /// The well draws the REAL file: both pictures come out of the bytes the
    /// host handed over, at the definition the catalog bakes at.
    #[test]
    fn a_clip_renders_both_faces_from_its_own_bytes() {
        let rate = 44_100;
        let bytes = wav_pcm16(&tone(rate, 2.0), rate);
        let pictures = render(&bytes, ClipFormat::Wav);
        let (fft, fw, fh) = pictures.fft.expect("a tone has a spectrum");
        assert_eq!((fw, fh), (PICTURE_W, FFT_H));
        assert_eq!(fft.len(), fw * fh * 4);
        let (wave, ww, wh) = pictures.wave.expect("and a shape");
        assert_eq!((ww, wh), (PICTURE_W, WAVE_H));
        assert_eq!(wave.len(), ww * wh * 4);
        assert_eq!(pictures.millis, 2_000);
    }

    /// Bytes that are not a track produce no pictures, rather than a black
    /// rectangle a viewer would read as "this track is silent".
    #[test]
    fn rubbish_produces_no_pictures() {
        let pictures = render(b"not a wav at all", ClipFormat::Wav);
        assert!(pictures.fft.is_none());
        assert!(pictures.wave.is_none());
        assert_eq!(pictures.millis, 0);
        // Digital silence decodes fine but has nothing to show.
        let rate = 22_050;
        let silence = wav_pcm16(&vec![(0, 0); rate as usize], rate);
        let pictures = render(&silence, ClipFormat::Wav);
        assert!(pictures.fft.is_none());
        assert!(pictures.wave.is_none());
        assert_eq!(pictures.millis, 1_000, "but its length is still known");
    }

    /// The faces are a pair, and the toggle is its own inverse.
    #[test]
    fn the_face_toggle_is_its_own_inverse() {
        assert_eq!(ClipFace::default(), ClipFace::Wave, "a player shows a waveform");
        assert_eq!(ClipFace::Fft.other(), ClipFace::Wave);
        assert_eq!(ClipFace::Wave.other().other(), ClipFace::Wave);
        assert_eq!(ClipFace::Fft.label(), "FFT");
    }

    /// A second request abandons the first: a well that is clicked through a
    /// list must never draw the track before last.
    #[test]
    fn a_new_clip_abandons_the_one_in_flight() {
        let rate = 22_050;
        let cx = Cx::new(Box::new(|_, _| {}));
        let mut decoder = ClipDecoder::default();
        assert!(!decoder.is_running());
        decoder.start(&cx, wav_pcm16(&tone(rate, 1.0), rate), ClipFormat::Wav);
        let first = decoder.generation;
        assert!(decoder.is_running());
        decoder.start(&cx, wav_pcm16(&tone(rate, 1.0), rate), ClipFormat::Wav);
        assert_ne!(decoder.generation, first);
        decoder.clear();
        assert!(!decoder.is_running());
        assert!(decoder.poll().is_none());
    }
}
