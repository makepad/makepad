//! A song as a video for chat apps: the song's audio as AAC with a lyric video (lyric_video.rs:
//! the title, then the timed lyrics with their translation, in the style Amp wears) as an H.264
//! picture, in an MP4. Chat apps (WhatsApp and the like) take an MP4 and play it inline with its
//! sound, where an audio file is often refused or sent as a document. A song without timed lyrics
//! shows its title card. The OS encoders do the work (`makepad-platform-video`: VideoToolbox + the
//! system AAC encoder behind AVAssetWriter on macOS, Media Foundation on Windows, GStreamer on
//! Linux).
//!
//! Made on the drag's producer thread from the moment the drag starts, and kept per song: the
//! cache key is the song file's bytes (audio, lyrics, translation: all in its tags), the look and
//! [`lyric_video::STYLE_VERSION`], so a song dragged again in the same style is handed over at once.

use crate::lyric_video::{self, Fonts, Frame, Look, Painter, Song, FPS, SIZE};
use makepad_video::{PcmAudioTrackOptions, VideoFileCodec, VideoFileEncoder, VideoFileEncoderOptions};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

pub const WIDTH: u32 = SIZE;
pub const HEIGHT: u32 = SIZE;
const AAC_RATES: [u32; 2] = [44_100, 48_000];
/// Videos kept in the cache (the newest).
const CACHE_KEEP: usize = 12;

/// What a video is drawn with, taken from the UI when the drag starts.
#[derive(Clone, Debug, Default)]
pub struct Kit {
    pub fonts: Fonts,
    pub look: Look,
}

/// How a video was made: the timings the report and the tests read.
#[derive(Clone, Debug, Default)]
pub struct Made {
    pub cached: bool,
    pub decode_ms: f64,
    pub frames: usize,
    pub drawn: usize,
    pub paint_ms: f64,
    pub encode_ms: f64,
    pub total_ms: f64,
    pub lyric_lines: usize,
}

/// The cache folder of the dropped videos.
pub fn cache_dir() -> PathBuf {
    std::env::temp_dir().join("makepad-amp-drop-video")
}

/// Decode the song at `src` and write the MP4 to `dest` (its extension may be anything: the
/// container is MPEG-4), from the cache when this song was made in this look before.
pub fn write_song_video(src: &Path, dest: &Path, name: &str, kit: &Kit) -> Result<Made, String> {
    let started = std::time::Instant::now();
    let bytes = std::fs::read(src).map_err(|e| format!("reading {}: {e}", src.display()))?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    kit.look.hash(&mut hasher);
    kit.fonts.hash(&mut hasher);
    lyric_video::STYLE_VERSION.hash(&mut hasher);
    let dir = cache_dir();
    let cached = dir.join(format!("{:016x}.mp4", hasher.finish()));
    if cached.is_file() && hand_over(&cached, dest).is_ok() {
        return Ok(Made { cached: true, total_ms: ms(started), ..Made::default() });
    }
    let tags = song_tags(&bytes);
    let song = Song::from_tags(&tags, name);
    let audio = makepad_audio_decode::decode_any(&bytes).map_err(|e| format!("decoding {}: {e:?}", src.display()))?;
    drop(bytes);
    let (rate, channels, pcm) = aac_ready(audio.rate, audio.channels, &audio.pcm_interleaved_f32)?;
    drop(audio);
    let decode_ms = ms(started);
    let _ = std::fs::create_dir_all(&dir);
    static PARTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let part = cached.with_extension(format!("{}-{}.part", std::process::id(), PARTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let mut made = write_video(&part, rate, channels, &pcm, &song, kit)?;
    made.decode_ms = decode_ms;
    if std::fs::rename(&part, &cached).is_err() || hand_over(&cached, dest).is_err() {
        // no cache (a read-only temp folder): straight to the destination
        let _ = std::fs::remove_file(&part);
        write_video(dest, rate, channels, &pcm, &song, kit)?;
    }
    prune(&dir);
    made.total_ms = ms(started);
    Ok(made)
}

fn ms(since: std::time::Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// The song's tags in full (Vorbis comments verbatim: timed lyrics run past the tag reader's
/// length cap), else what the tag reader finds.
fn song_tags(bytes: &[u8]) -> Vec<(String, String)> {
    if let Ok(comments) = makepad_audio_decode::vorbis::read_comments_verbatim(bytes) {
        if !comments.is_empty() {
            return comments;
        }
    }
    makepad_audio_decode::read_tags(bytes).map(|t| t.all).unwrap_or_default()
}

/// The cached file at `dest`: a hard link (the cache's copy can go later), else a copy.
fn hand_over(cached: &Path, dest: &Path) -> std::io::Result<()> {
    let _ = std::fs::remove_file(dest);
    std::fs::hard_link(cached, dest).or_else(|_| std::fs::copy(cached, dest).map(|_| ()))
}

/// Keep the newest [`CACHE_KEEP`] videos.
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "mp4"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in files.into_iter().skip(CACHE_KEEP) {
        let _ = std::fs::remove_file(path);
    }
}

/// Interleaved f32 at any rate and channel count as 16-bit PCM the AAC encoder takes: 44.1 or
/// 48 kHz (other rates resampled to 48 kHz), mono or stereo (more channels folded to two).
pub fn aac_ready(rate: u32, channels: u16, pcm: &[f32]) -> Result<(u32, u16, Vec<i16>), String> {
    if rate == 0 || channels == 0 {
        return Err("the song has no audio".into());
    }
    let n = channels as usize;
    let frames = pcm.len() / n;
    if frames == 0 {
        return Err("the song has no audio".into());
    }
    let out_ch = if n == 1 { 1 } else { 2 };
    let mut planes: Vec<Vec<f32>> = (0..out_ch).map(|_| Vec::with_capacity(frames)).collect();
    for f in 0..frames {
        let frame = &pcm[f * n..f * n + n];
        if n <= 2 {
            for c in 0..out_ch {
                planes[c].push(frame[c]);
            }
        } else {
            // even channels to the left, odd to the right
            let (mut l, mut r, mut nl, mut nr) = (0.0, 0.0, 0.0f32, 0.0f32);
            for (c, v) in frame.iter().enumerate() {
                if c % 2 == 0 { l += v; nl += 1.0 } else { r += v; nr += 1.0 }
            }
            planes[0].push(l / nl);
            planes[1].push(r / nr.max(1.0));
        }
    }
    let out_rate = if AAC_RATES.contains(&rate) { rate } else { 48_000 };
    if out_rate != rate {
        for plane in &mut planes {
            *plane = makepad_audio_encode::mp3::resample::resample(plane, rate, out_rate);
        }
    }
    let len = planes.iter().map(Vec::len).min().unwrap_or(0);
    let mut out = Vec::with_capacity(len * out_ch);
    for f in 0..len {
        for plane in &planes {
            out.push((plane[f].clamp(-1.0, 1.0) * 32767.0).round() as i16);
        }
    }
    Ok((out_rate, out_ch as u16, out))
}

/// The movie: the song's lyric video (or title card) over its sound, ending with the sound. The
/// frames come from [`lyric_video::frames`] (one per tick, constant rate); after each frame the
/// sound up to the next whole second goes in, so the writer interleaves the two tracks. The file
/// is laid out index-first ("fast start").
pub fn write_video(dest: &Path, rate: u32, channels: u16, pcm: &[i16], song: &Song, kit: &Kit) -> Result<Made, String> {
    let per_second = rate as usize * channels as usize;
    let options = VideoFileEncoderOptions {
        codec: VideoFileCodec::H264,
        width: WIDTH,
        height: HEIGHT,
        fps_num: FPS as u32,
        fps_den: 1,
        video_bitrate_bps: 250_000,
        audio: Some(PcmAudioTrackOptions { sample_rate: rate, channels, aac_bitrate_bps: if channels == 1 { 96_000 } else { 128_000 } }),
        keyframe_only: false,
        color: lyric_video::COLOR,
    };
    let end_tick = ((pcm.len() / channels as usize) as u64 * FPS as u64).div_ceil(rate as u64).max(1) as i64;
    let keys = lyric_video::timeline(song);
    let frames = lyric_video::frames(&keys, end_tick);
    let mut painter = Painter::new(song, &kit.look, &kit.fonts);
    let mut made = Made { frames: frames.len(), lyric_lines: song.lyrics.as_ref().map_or(0, |l| l.lines().count()), ..Made::default() };
    let path = dest.to_string_lossy();
    let mut encoder = VideoFileEncoder::new(&path, options).map_err(|e| format!("video encoder: {e}"))?;
    let (mut shown, mut target, mut from) = (Vec::new(), Vec::new(), Vec::new());
    let (mut pushed, mut fading) = (0, None);
    for (tick, frame) in frames {
        let paint = std::time::Instant::now();
        match frame {
            Frame::Scene(k) => {
                painter.paint(keys[k].1, &mut shown);
                made.drawn += 1;
            }
            Frame::Fade(k, x) => {
                if fading != Some(k) {
                    // the fade's first frame: draw where it goes, from what is shown
                    painter.paint(keys[k].1, &mut target);
                    std::mem::swap(&mut from, &mut shown);
                    fading = Some(k);
                    made.drawn += 1;
                }
                lyric_video::crossfade(&from, &target, x, &mut shown);
            }
            Frame::Repeat => {}
        }
        made.paint_ms += ms(paint);
        let push = std::time::Instant::now();
        encoder.push_frame_nv12(&shown, Some(tick * 10_000_000 / FPS)).map_err(|e| format!("video frame: {e}"))?;
        let upto = (((tick / FPS + 1) as usize) * per_second).min(pcm.len());
        if upto > pushed {
            encoder.push_audio_i16(&pcm[pushed..upto]).map_err(|e| format!("audio: {e}"))?;
            pushed = upto;
        }
        made.encode_ms += ms(push);
    }
    let push = std::time::Instant::now();
    if pushed < pcm.len() {
        encoder.push_audio_i16(&pcm[pushed..]).map_err(|e| format!("audio: {e}"))?;
    }
    encoder.finish().map_err(|e| format!("finishing the video: {e}"))?;
    // the index first: messaging apps read it before the media
    crate::faststart::faststart(dest)?;
    made.encode_ms += ms(push);
    Ok(made)
}
