//! A song as a video for chat apps: the song's audio as AAC with a still title card (title and
//! artist) as an H.264 picture, in an MP4. Chat apps (WhatsApp and the like) take an MP4 and
//! play it inline with its sound, where an audio file is often refused or sent as a document.
//! The OS encoders do the work (`makepad-platform-video`: VideoToolbox + the system AAC encoder
//! behind AVAssetWriter on macOS, Media Foundation on Windows, GStreamer on Linux).
//!
//! The card is drawn on the CPU with the UI's own fonts (IBM Plex SemiBold, the CJK face for what
//! Plex lacks), handed over as file paths from the UI thread when the drag starts.

use makepad_video::{PcmAudioTrackOptions, VideoColor, VideoFileCodec, VideoFileEncoder, VideoFileEncoderOptions};
use std::path::{Path, PathBuf};

pub const WIDTH: u32 = 1280;
pub const HEIGHT: u32 = 720;
const AAC_RATES: [u32; 2] = [44_100, 48_000];

/// The text of the card.
#[derive(Clone, Debug, Default)]
pub struct Card {
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
}

/// The fonts the card draws with, in fallback order (missing files are skipped).
#[derive(Clone, Debug, Default)]
pub struct CardFonts {
    pub paths: Vec<PathBuf>,
}

/// The font resources the card uses, as `crate_resource` logical paths.
pub const CARD_FONTS: [&str; 2] = [
    "makepad_widgets/resources/IBMPlexSans-SemiBold.ttf",
    "makepad_widgets/resources/LXGWWenKaiBold.ttf",
];

/// Decode the song at `src` and write the MP4 to `dest` (its extension may be anything: the
/// container is MPEG-4). The card shows the song's title and artist tags, else `name`.
pub fn write_song_video(src: &Path, dest: &Path, name: &str, fonts: &CardFonts) -> Result<(), String> {
    let bytes = std::fs::read(src).map_err(|e| format!("reading {}: {e}", src.display()))?;
    let tags = makepad_audio_decode::read_tags(&bytes).unwrap_or_default();
    let clean = |v: Option<String>| v.map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let card = Card { title: clean(tags.title).unwrap_or_else(|| name.to_string()), artist: clean(tags.artist), album: clean(tags.album) };
    let audio = makepad_audio_decode::decode_any(&bytes).map_err(|e| format!("decoding {}: {e:?}", src.display()))?;
    drop(bytes);
    let (rate, channels, pcm) = aac_ready(audio.rate, audio.channels, &audio.pcm_interleaved_f32)?;
    drop(audio);
    let picture = render_card(&card, fonts);
    write_video(dest, rate, channels, &pcm, &picture)
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

/// Frames a second the movie is timed in (only the last second uses them, see `write_video`).
const TICKS: i64 = 10;

/// One still `picture` (RGBA, WIDTH x HEIGHT) for the length of the audio: a frame a second,
/// each second's audio pushed after its frame so the writer interleaves the two tracks, and in
/// the last second a frame every tenth until the audio ends, so the picture track (and the
/// movie) ends within a tenth of a second of the sound instead of up to a second after it.
pub fn write_video(dest: &Path, rate: u32, channels: u16, pcm: &[i16], picture: &[u8]) -> Result<(), String> {
    let per_second = rate as usize * channels as usize;
    let options = VideoFileEncoderOptions {
        codec: VideoFileCodec::H264,
        width: WIDTH,
        height: HEIGHT,
        fps_num: TICKS as u32,
        fps_den: 1,
        video_bitrate_bps: 600_000,
        audio: Some(PcmAudioTrackOptions { sample_rate: rate, channels, aac_bitrate_bps: if channels == 1 { 128_000 } else { 192_000 } }),
        keyframe_only: false,
        color: VideoColor::BT709,
    };
    let mut nv12 = Vec::new();
    makepad_video::nv12::rgba8_to_nv12(picture, WIDTH, HEIGHT, options.color, &mut nv12);
    // the frame times in tenths: whole seconds, then tenths through the last second
    let end_ticks = ((pcm.len() / channels as usize) as u64 * TICKS as u64).div_ceil(rate as u64).max(1) as i64;
    let last_whole = (end_ticks - 1) / TICKS * TICKS;
    let ticks = (0..last_whole).step_by(TICKS as usize).chain(last_whole..end_ticks);
    let path = dest.to_string_lossy();
    let mut encoder = VideoFileEncoder::new(&path, options).map_err(|e| format!("video encoder: {e}"))?;
    let mut pushed = 0;
    for tick in ticks {
        encoder.push_frame_nv12(&nv12, Some(tick * 10_000_000 / TICKS)).map_err(|e| format!("video frame: {e}"))?;
        // the audio up to the next whole second goes in after the frame that starts it
        let upto = (((tick / TICKS + 1) as usize) * per_second).min(pcm.len());
        if upto > pushed {
            encoder.push_audio_i16(&pcm[pushed..upto]).map_err(|e| format!("audio: {e}"))?;
            pushed = upto;
        }
    }
    if pushed < pcm.len() {
        encoder.push_audio_i16(&pcm[pushed..]).map_err(|e| format!("audio: {e}"))?;
    }
    encoder.finish().map_err(|e| format!("finishing the video: {e}"))
}

// ---------------------------------------------------------------------------
// The title card
// ---------------------------------------------------------------------------

const BG_TOP: [f32; 3] = [0.075, 0.082, 0.118];
const BG_BOTTOM: [f32; 3] = [0.165, 0.110, 0.220];
const ACCENT: [f32; 3] = [0.98, 0.55, 0.22];

/// The card as RGBA, WIDTH x HEIGHT: a dark gradient, an accent rule, the title large and the
/// artist (and album) under it, each line shrunk to fit and cut with an ellipsis past that.
pub fn render_card(card: &Card, fonts: &CardFonts) -> Vec<u8> {
    let (w, h) = (WIDTH as usize, HEIGHT as usize);
    let mut rgb = vec![[0f32; 3]; w * h];
    for y in 0..h {
        let t = y as f32 / (h - 1) as f32;
        let c = [0, 1, 2].map(|i| BG_TOP[i] + (BG_BOTTOM[i] - BG_TOP[i]) * t);
        rgb[y * w..(y + 1) * w].fill(c);
    }
    let data: Vec<Vec<u8>> = fonts.paths.iter().filter_map(|p| std::fs::read(p).ok()).collect();
    let faces: Vec<ttf_parser::Face> = data.iter().filter_map(|d| ttf_parser::Face::parse(d, 0).ok()).collect();
    let margin = 96.0;
    let max_w = w as f32 - 2.0 * margin;
    let title_px = fit_size(&faces, &card.title, 84.0, 48.0, max_w);
    let sub = card.artist.iter().chain(card.album.iter()).cloned().collect::<Vec<_>>().join("  ·  ");
    let sub_px = fit_size(&faces, &sub, 40.0, 28.0, max_w);
    // the block of text sits a little above the middle
    let block = title_px * 1.25 + if sub.is_empty() { 0.0 } else { sub_px * 1.6 };
    let top = (h as f32 - block) * 0.45;
    let rule_y = (top - 34.0).max(8.0) as usize;
    for y in rule_y..rule_y + 6 {
        for x in margin as usize..margin as usize + 120 {
            rgb[y * w + x] = ACCENT;
        }
    }
    let title_base = top + title_px;
    draw_line(&mut rgb, w, h, &faces, &ellipsize(&faces, &card.title, title_px, max_w), title_px, margin, title_base, [1.0, 1.0, 1.0], 1.0);
    if !sub.is_empty() {
        let sub_base = title_base + sub_px * 1.6;
        draw_line(&mut rgb, w, h, &faces, &ellipsize(&faces, &sub, sub_px, max_w), sub_px, margin, sub_base, [1.0, 1.0, 1.0], 0.72);
    }
    let mut out = Vec::with_capacity(w * h * 4);
    for c in rgb {
        out.extend_from_slice(&[to_u8(c[0]), to_u8(c[1]), to_u8(c[2]), 255]);
    }
    out
}

fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// (face index, glyph) for each character: the first face that has it.
fn glyphs(faces: &[ttf_parser::Face], text: &str) -> Vec<(usize, ttf_parser::GlyphId)> {
    text.chars()
        .filter_map(|c| faces.iter().enumerate().find_map(|(i, f)| f.glyph_index(c).filter(|g| g.0 != 0).map(|g| (i, g))))
        .collect()
}

fn line_width(faces: &[ttf_parser::Face], text: &str, px: f32) -> f32 {
    glyphs(faces, text)
        .iter()
        .map(|(i, g)| faces[*i].glyph_hor_advance(*g).unwrap_or(0) as f32 * px / faces[*i].units_per_em() as f32)
        .sum()
}

/// The largest size from `max` down to `min` at which `text` fits `width`.
fn fit_size(faces: &[ttf_parser::Face], text: &str, max: f32, min: f32, width: f32) -> f32 {
    let at_max = line_width(faces, text, max);
    if at_max <= width || at_max <= 0.0 {
        return max;
    }
    (max * width / at_max).max(min)
}

/// `text` cut to fit `width` at `px`, with an ellipsis when cut.
fn ellipsize(faces: &[ttf_parser::Face], text: &str, px: f32, width: f32) -> String {
    if line_width(faces, text, px) <= width {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let cut: String = chars.iter().collect::<String>().trim_end().to_string() + "…";
        if line_width(faces, &cut, px) <= width {
            return cut;
        }
    }
    String::new()
}

struct Outline<'a> {
    raster: &'a mut ab_glyph_rasterizer::Rasterizer,
    scale: f32,
    x: f32,
    base: f32,
    start: ab_glyph_rasterizer::Point,
    last: ab_glyph_rasterizer::Point,
}

impl Outline<'_> {
    fn p(&self, x: f32, y: f32) -> ab_glyph_rasterizer::Point {
        ab_glyph_rasterizer::point(self.x + x * self.scale, self.base - y * self.scale)
    }
}

impl ttf_parser::OutlineBuilder for Outline<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.start = self.p(x, y);
        self.last = self.start;
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.p(x, y);
        self.raster.draw_line(self.last, p);
        self.last = p;
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (c, p) = (self.p(x1, y1), self.p(x, y));
        self.raster.draw_quad(self.last, c, p);
        self.last = p;
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (c1, c2, p) = (self.p(x1, y1), self.p(x2, y2), self.p(x, y));
        self.raster.draw_cubic(self.last, c1, c2, p);
        self.last = p;
    }
    fn close(&mut self) {
        if self.last != self.start {
            self.raster.draw_line(self.last, self.start);
        }
        self.last = self.start;
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_line(rgb: &mut [[f32; 3]], w: usize, h: usize, faces: &[ttf_parser::Face], text: &str, px: f32, x: f32, base: f32, color: [f32; 3], alpha: f32) {
    if faces.is_empty() || text.is_empty() {
        return;
    }
    let mut raster = ab_glyph_rasterizer::Rasterizer::new(w, h);
    let mut pen = x;
    for (i, g) in glyphs(faces, text) {
        let face = &faces[i];
        let scale = px / face.units_per_em() as f32;
        let zero = ab_glyph_rasterizer::point(0.0, 0.0);
        let mut outline = Outline { raster: &mut raster, scale, x: pen, base, start: zero, last: zero };
        face.outline_glyph(g, &mut outline);
        pen += face.glyph_hor_advance(g).unwrap_or(0) as f32 * scale;
    }
    raster.for_each_pixel_2d(|px_x, px_y, coverage| {
        let (px_x, px_y) = (px_x as usize, px_y as usize);
        if px_x < w && px_y < h {
            let a = coverage.clamp(0.0, 1.0) * alpha;
            let dst = &mut rgb[px_y * w + px_x];
            for c in 0..3 {
                dst[c] += (color[c] - dst[c]) * a;
            }
        }
    });
}
