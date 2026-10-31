//! The picture of a dropped song's video: a lyric video in the style Amp wears. The title and
//! artist open it, then the timed lyrics play: the line being sung big (its own language) with
//! its translation under it and the next line dim below, the sung words lit in the style's accent
//! when the song has word timing (from the model's score), else the whole line as it starts.
//! Section tags (VERSE, CHORUS) sit small above the line. A song without timed lyrics is the
//! title card for its whole length.
//!
//! Drawn on the CPU with the UI's own text stack (`makepad-font-core`: the same font tables,
//! shaping and glyph outlines the app's text uses; the coverage rasteriser of `makepad-draw`)
//! straight into the encoder's NV12 planes, on the drag's producer thread: no GPU, no UI thread.
//! A frame is drawn only when what it shows changes (a line comes in, a word is sung); a line
//! change crossfades over a few frames. The movie is constant-rate at [`FPS`] (what messaging apps
//! expect): a held picture is pushed again each tick, which the H.264 encoder turns into
//! near-empty frames.

use crate::lyrics::{TimedLyrics, Timing, Word};
use makepad_font_core::font::{Font, FontId};
use makepad_font_core::font_face::{FontData, FontFace};
use makepad_font_core::glyph_outline::Command;
use makepad_font_core::rustybuzz::UnicodeBuffer;
use makepad_video::VideoColor;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

/// The picture is square: chat apps show a square video large in the conversation. 720 px keeps
/// a constant-rate movie quick to encode and small to send (chat apps scale it down anyway).
pub const SIZE: u32 = 720;
/// The movie's frame rate, constant: lyric lines change slowly, and each frame costs the encoder
/// ~1.3 ms (10 fps: a 3.5 min song in ~3 s; 30 fps took 9 s).
pub const FPS: i64 = 10;
/// Bumped when the look or the file layout changes, so cached videos are made again (2: constant
/// rate + fast start; 3: 10 fps).
pub const STYLE_VERSION: u32 = 3;
/// The colour space of the frames.
pub const COLOR: VideoColor = VideoColor::BT709;

/// A line shows this long before it is sung (s), so it can be read as it starts.
const LEAD: f64 = 0.6;
/// A line stays this long after its last word (s) when the next is far off.
const HOLD: f64 = 1.6;
/// After the last line, this long (s) before the title card returns.
const OUTRO: f64 = 2.5;
/// A line change crossfades over this many frames.
const FADE_FRAMES: i64 = 3;

// ---------------------------------------------------------------------------
// What is drawn
// ---------------------------------------------------------------------------

/// The style's colours (Amp's `hw_palette`: the screen's floor, text, secondary text, accent),
/// linear 0..1 sRGB values.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub background: [f32; 3],
    pub text: [f32; 3],
    pub muted: [f32; 3],
    pub accent: [f32; 3],
    /// Titles and headings in capitals (the style's `caps`).
    pub caps: bool,
}

impl Default for Look {
    /// Amp's own colours (its default theme).
    fn default() -> Self {
        Look { background: rgb(0x18191b), text: rgb(0xe8e9eb), muted: rgb(0x9b9ea6), accent: rgb(0x1ed760), caps: false }
    }
}

impl Hash for Look {
    fn hash<H: Hasher>(&self, h: &mut H) {
        for c in [self.background, self.text, self.muted, self.accent] {
            for v in c {
                v.to_bits().hash(h);
            }
        }
        self.caps.hash(h);
    }
}

fn rgb(hex: u32) -> [f32; 3] {
    [((hex >> 16) & 255) as f32 / 255.0, ((hex >> 8) & 255) as f32 / 255.0, (hex & 255) as f32 / 255.0]
}

/// The font files of each role, in fallback order (the first that has a character draws it).
#[derive(Clone, Debug, Default, Hash)]
pub struct Fonts {
    pub bold: Vec<PathBuf>,
    pub regular: Vec<PathBuf>,
    pub italic: Vec<PathBuf>,
}

/// The UI's fonts per role, as `crate_resource` logical paths (Plex, with the CJK face behind it).
pub const BOLD_FONTS: [&str; 2] = ["makepad_widgets/resources/IBMPlexSans-SemiBold.ttf", "makepad_widgets/resources/LXGWWenKaiBold.ttf"];
pub const REGULAR_FONTS: [&str; 2] = ["makepad_widgets/resources/IBMPlexSans-Text.ttf", "makepad_widgets/resources/LXGWWenKaiRegular.ttf"];
pub const ITALIC_FONTS: [&str; 2] = ["makepad_widgets/resources/IBMPlexSans-Italic.ttf", "makepad_widgets/resources/LXGWWenKaiRegular.ttf"];

/// A song as the video shows it.
#[derive(Clone, Debug, Default)]
pub struct Song {
    pub title: String,
    pub artist: Option<String>,
    /// Its timed lyrics (with their translation), when it has them.
    pub lyrics: Option<TimedLyrics>,
}

impl Song {
    /// Title, artist and timed lyrics from a file's tags (Vorbis comments as Amp writes them:
    /// SYNCEDLYRICS, LYRICS_TIMING, TRANSLATION); `name` when it has no title.
    pub fn from_tags(tags: &[(String, String)], name: &str) -> Song {
        let get = |key: &str| tags.iter().find(|(k, v)| k.eq_ignore_ascii_case(key) && !v.trim().is_empty()).map(|(_, v)| v.trim().to_string());
        let timing = match get("LYRICS_TIMING") {
            Some(t) if t.contains("score") => Timing::Score,
            _ => Timing::Estimated,
        };
        let lyrics = get("SYNCEDLYRICS")
            .map(|lrc| crate::lyrics::from_lrc(&lrc, &get("TRANSLATION").unwrap_or_default(), timing))
            .filter(|l| l.lines().next().is_some());
        Song { title: get("TITLE").unwrap_or_else(|| name.to_string()), artist: get("ARTIST"), lyrics }
    }
}

// ---------------------------------------------------------------------------
// The timeline
// ---------------------------------------------------------------------------

/// What the picture shows at a moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scene {
    /// The title card (before the first line, after the last, or the whole song).
    Title,
    /// A line (flat index) with this many of its words lit.
    Line { line: usize, lit: usize },
    /// Between lines: the coming one, dim.
    Gap { next: usize },
}

struct Flat<'a> {
    section: &'a str,
    words: &'a [Word],
    start: f64,
    end: f64,
    translation: &'a str,
}

fn flat(lyrics: &TimedLyrics) -> Vec<Flat<'_>> {
    lyrics
        .sections
        .iter()
        .flat_map(|s| s.lines.iter().map(move |l| Flat { section: &s.name, words: &l.words, start: l.start, end: l.end, translation: &l.translation }))
        .collect()
}

/// The scenes of a song, each with the frame tick it starts at (ascending, the first at 0,
/// no two alike in a row).
pub fn timeline(song: &Song) -> Vec<(i64, Scene)> {
    let mut keys = vec![(0i64, Scene::Title)];
    let Some(lyrics) = &song.lyrics else { return keys };
    let lines = flat(lyrics);
    if lines.is_empty() {
        return keys;
    }
    let karaoke = lyrics.timing == Timing::Score;
    // a line appears LEAD before it is sung, never before the previous one did
    let mut appear = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        let a = (line.start - LEAD).max(0.0);
        appear.push(if i == 0 { a } else { a.max(appear[i - 1]) });
    }
    let scene_at = |t: f64| -> Scene {
        let Some(i) = appear.iter().rposition(|&a| a <= t) else { return Scene::Title };
        let line = &lines[i];
        let last = i + 1 == lines.len();
        if last && t >= line.end + OUTRO {
            return Scene::Title;
        }
        if !last && t >= line.end + HOLD {
            return Scene::Gap { next: i + 1 };
        }
        let lit = if karaoke { line.words.iter().filter(|w| w.start <= t).count() } else if t >= line.start { line.words.len() } else { 0 };
        Scene::Line { line: i, lit }
    };
    let mut times: Vec<f64> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        times.push(appear[i]);
        times.push(line.start);
        times.push(line.end + HOLD);
        times.push(line.end + OUTRO);
        if karaoke {
            times.extend(line.words.iter().map(|w| w.start));
        }
    }
    let mut ticks: Vec<i64> = times.iter().map(|t| (t * FPS as f64).ceil().max(0.0) as i64).collect();
    ticks.sort_unstable();
    ticks.dedup();
    for tick in ticks {
        let scene = scene_at(tick as f64 / FPS as f64 + 1e-6);
        let last = keys.last().unwrap();
        if last.1 == scene {
            continue;
        }
        if last.0 == tick {
            keys.pop();
            if keys.last().is_some_and(|k| k.1 == scene) {
                continue;
            }
        }
        keys.push((tick, scene));
    }
    keys
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

/// A role's fonts, in fallback order.
struct Chain {
    fonts: Vec<Font>,
}

impl Chain {
    fn load(paths: &[PathBuf], ids: &mut u64) -> Chain {
        let fonts = paths
            .iter()
            .filter_map(|p| FontData::from_file_mmap_or_read(p).ok())
            .filter_map(|data| FontFace::from_data_and_index(data, 0))
            .map(|face| {
                *ids += 1;
                Font::new(FontId::from(*ids), face, 0.0, 0.0)
            })
            .collect();
        Chain { fonts }
    }

    /// The first font that has `c`.
    fn pick(&self, c: char) -> usize {
        self.fonts.iter().position(|f| f.with_ttf_parser_face(|face| face.glyph_index(c).is_some_and(|g| g.0 != 0))).unwrap_or(0)
    }
}

struct Glyph {
    font: usize,
    id: u16,
    x: f32,
    y: f32,
    advance: f32,
    /// Byte offset of its cluster in the shaped text.
    byte: usize,
}

/// `text` shaped at `px` (pixels per em) with `tracking` px between glyphs: runs of the font that
/// has their characters, each shaped as a whole.
fn shape(chain: &Chain, text: &str, px: f32, tracking: f32) -> (Vec<Glyph>, f32) {
    let mut glyphs = Vec::new();
    if chain.fonts.is_empty() {
        return (glyphs, 0.0);
    }
    let mut runs: Vec<(usize, usize, usize)> = Vec::new();
    for (at, c) in text.char_indices() {
        let font = if c.is_whitespace() { runs.last().map_or(0, |r| r.0) } else { chain.pick(c) };
        match runs.last_mut() {
            Some(run) if run.0 == font => run.2 = at + c.len_utf8(),
            _ => runs.push((font, at, at + c.len_utf8())),
        }
    }
    let mut x = 0.0;
    for (font_index, from, to) in runs {
        let font = &chain.fonts[font_index];
        let scale = px / font.units_per_em();
        let mut buffer = UnicodeBuffer::new();
        buffer.push_str(&text[from..to]);
        let out = font.shape(&[], buffer);
        for (info, pos) in out.glyph_infos().iter().zip(out.glyph_positions()) {
            let advance = pos.x_advance as f32 * scale;
            glyphs.push(Glyph { font: font_index, id: info.glyph_id as u16, x: x + pos.x_offset as f32 * scale, y: pos.y_offset as f32 * scale, advance, byte: from + info.cluster as usize });
            x += advance + tracking;
        }
    }
    let width = if glyphs.is_empty() { 0.0 } else { x - tracking };
    (glyphs, width)
}

/// Coverage (0..255) of a row of text, `ascent` px from its top to the baseline.
struct Mask {
    w: usize,
    h: usize,
    a: Vec<u8>,
    ascent: f32,
}

const PAD: f32 = 3.0;

fn raster(chain: &Chain, glyphs: &[Glyph], width: f32, px: f32) -> Mask {
    let first = chain.fonts.first();
    let ascent = first.map_or(0.8, |f| f.ascender_in_ems()).max(0.8) * px + PAD;
    let descent = first.map_or(0.25, |f| -f.descender_in_ems()).max(0.25) * px + PAD;
    let (w, h) = ((width + 2.0 * PAD).ceil().max(1.0) as usize, (ascent + descent).ceil() as usize);
    let mut raster = ab_glyph_rasterizer::Rasterizer::new(w, h);
    let p = |x: f32, y: f32| ab_glyph_rasterizer::point(x, y);
    for g in glyphs {
        let font = &chain.fonts[g.font];
        let Some(outline) = font.glyph_outline_rc(g.id) else { continue };
        let scale = px / font.units_per_em();
        let (ox, oy) = (PAD + g.x, ascent - g.y);
        let at = |q: makepad_font_core::geom::Point<f32>| p(ox + q.x * scale, oy - q.y * scale);
        let (mut last, mut start) = (p(0.0, 0.0), None);
        for command in outline.commands() {
            match *command {
                Command::MoveTo(q) => {
                    last = at(q);
                    start = Some(last);
                }
                Command::LineTo(q) => {
                    let n = at(q);
                    raster.draw_line(last, n);
                    last = n;
                }
                Command::QuadTo(c, q) => {
                    let n = at(q);
                    raster.draw_quad(last, at(c), n);
                    last = n;
                }
                Command::CurveTo(c1, c2, q) => {
                    let n = at(q);
                    raster.draw_cubic(last, at(c1), at(c2), n);
                    last = n;
                }
                Command::Close => {
                    if let Some(s) = start.take() {
                        if s != last {
                            raster.draw_line(last, s);
                        }
                        last = s;
                    }
                }
            }
        }
    }
    let mut a = vec![0u8; w * h];
    raster.for_each_pixel_2d(|x, y, c| {
        let (x, y) = (x as usize, y as usize);
        if x < w && y < h {
            a[y * w + x] = (c.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    });
    Mask { w, h, a, ascent }
}

/// A row of a laid-out text: its mask, and the right edge (px in the mask) of each of its words.
struct Row {
    mask: Mask,
    width: f32,
    /// (word index, right edge) of the words on this row.
    words: Vec<(usize, f32)>,
}

/// Text in rows, centred or left-aligned by the caller.
struct Block {
    rows: Vec<Row>,
    line_h: f32,
}

impl Block {
    fn height(&self) -> f32 {
        self.rows.len() as f32 * self.line_h
    }
}

/// `words` (text, a space after it) at `max_px`, shrunk or broken into up to three balanced rows
/// to fit `max_w`: one row while it fits at 80% of the size, else the fewest rows that fit at
/// `min_px` (or three, smaller still: lyrics are never cut).
fn layout(chain: &Chain, words: &[(String, bool)], max_px: f32, min_px: f32, max_w: f32, tracking_em: f32, line_spacing: f32) -> Block {
    let word_w: Vec<f32> = words.iter().map(|(t, _)| shape(chain, t, max_px, tracking_em * max_px).1).collect();
    let space_w = shape(chain, " ", max_px, 0.0).1;
    let gap = |i: usize| (if words[i].1 { space_w } else { 0.0 }) + tracking_em * max_px;
    let total: f32 = (0..words.len()).map(|i| word_w[i] + if i + 1 < words.len() { gap(i) } else { 0.0 }).sum();
    let mut breaks: Vec<usize> = Vec::new();
    let mut px = max_px;
    for rows in 1..=3usize {
        // balanced greedy: a row breaks before the word that takes it past an even share
        breaks.clear();
        if rows > 1 {
            let target = total / rows as f32;
            let mut w = 0.0;
            for i in 0..words.len() {
                let add = word_w[i];
                if w > 0.0 && w + add > target * 1.08 && breaks.len() + 1 < rows {
                    breaks.push(i);
                    w = 0.0;
                }
                w += add + gap(i);
            }
        }
        let widest = row_ranges(&breaks, words.len())
            .map(|(a, b)| (a..b).map(|i| word_w[i] + if i + 1 < b { gap(i) } else { 0.0 }).sum::<f32>())
            .fold(0.0f32, f32::max);
        px = if widest > max_w { max_px * max_w / widest } else { max_px };
        let floor = if rows == 1 { min_px.max(max_px * 0.8) } else { min_px };
        if px >= floor || rows == 3 {
            break;
        }
    }
    let rows = row_ranges(&breaks, words.len())
        .map(|(a, b)| {
            let mut text = String::new();
            let mut spans = Vec::new();
            for i in a..b {
                let from = text.len();
                text.push_str(&words[i].0);
                spans.push((i, from, text.len()));
                if words[i].1 && i + 1 < b {
                    text.push(' ');
                }
            }
            let (glyphs, width) = shape(chain, &text, px, tracking_em * px);
            let words = spans
                .iter()
                .map(|&(i, from, to)| {
                    let right = glyphs.iter().filter(|g| g.byte >= from && g.byte < to).map(|g| g.x + g.advance).fold(0.0f32, f32::max);
                    (i, PAD + right)
                })
                .collect();
            Row { mask: raster(chain, &glyphs, width, px), width, words }
        })
        .collect();
    Block { rows, line_h: px * line_spacing }
}

fn row_ranges(breaks: &[usize], n: usize) -> impl Iterator<Item = (usize, usize)> + '_ {
    let starts = std::iter::once(0).chain(breaks.iter().copied());
    let ends = breaks.iter().copied().chain(std::iter::once(n));
    starts.zip(ends).filter(|(a, b)| a < b)
}

fn words_of(text: &str) -> Vec<(String, bool)> {
    crate::lyrics::tokens(text)
}

// ---------------------------------------------------------------------------
// The canvas (NV12, straight into the encoder's planes)
// ---------------------------------------------------------------------------

/// A colour in the stream's Y'CbCr, with an opacity.
#[derive(Clone, Copy)]
struct Ink {
    y: f32,
    u: f32,
    v: f32,
    a: f32,
}

fn ink(c: [f32; 3], a: f32) -> Ink {
    let px = [to_u8(c[0]), to_u8(c[1]), to_u8(c[2]), 255];
    let rgba: Vec<u8> = px.iter().copied().cycle().take(16).collect();
    let mut nv12 = Vec::new();
    makepad_video::nv12::rgba8_to_nv12(&rgba, 2, 2, COLOR, &mut nv12);
    Ink { y: nv12[0] as f32, u: nv12[4] as f32, v: nv12[5] as f32, a }
}

fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
}

fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Blend `mask` with its top-left at (x, y): left of `split` (px in the mask) in `left`, from
/// there in `right`.
fn blit(frame: &mut [u8], mask: &Mask, x: i32, y: i32, split: f32, left: Ink, right: Ink) {
    let s = SIZE as i32;
    for my in 0..mask.h as i32 {
        let py = y + my;
        if py < 0 || py >= s {
            continue;
        }
        let row = &mask.a[my as usize * mask.w..(my as usize + 1) * mask.w];
        for (mx, &c) in row.iter().enumerate() {
            let px = x + mx as i32;
            if c == 0 || px < 0 || px >= s {
                continue;
            }
            let ink = if (mx as f32) < split { left } else { right };
            let a = c as f32 / 255.0 * ink.a;
            let d = &mut frame[(py * s + px) as usize];
            *d = (*d as f32 + (ink.y - *d as f32) * a).round() as u8;
        }
    }
    // chroma: one sample per 2x2 block, its average coverage
    let uv = (s * s) as usize;
    let (cy0, cy1) = ((y.max(0)) / 2, ((y + mask.h as i32).min(s) + 1) / 2);
    let (cx0, cx1) = ((x.max(0)) / 2, ((x + mask.w as i32).min(s) + 1) / 2);
    let cov = |px: i32, py: i32| -> u32 {
        let (mx, my) = (px - x, py - y);
        if mx < 0 || my < 0 || mx >= mask.w as i32 || my >= mask.h as i32 {
            0
        } else {
            mask.a[my as usize * mask.w + mx as usize] as u32
        }
    };
    for cy in cy0..cy1 {
        for cx in cx0..cx1 {
            let (px, py) = (cx * 2, cy * 2);
            let sum = cov(px, py) + cov(px + 1, py) + cov(px, py + 1) + cov(px + 1, py + 1);
            if sum == 0 {
                continue;
            }
            let ink = if ((px - x) as f32) < split { left } else { right };
            let a = sum as f32 / (4.0 * 255.0) * ink.a;
            let at = uv + (cy * s + cx * 2) as usize;
            frame[at] = (frame[at] as f32 + (ink.u - frame[at] as f32) * a).round() as u8;
            frame[at + 1] = (frame[at + 1] as f32 + (ink.v - frame[at + 1] as f32) * a).round() as u8;
        }
    }
}

/// `to` over `from` at `t` (0..1), into `out`.
pub fn crossfade(from: &[u8], to: &[u8], t: f32, out: &mut Vec<u8>) {
    let k = (t.clamp(0.0, 1.0) * 256.0) as u32;
    out.clear();
    out.extend(from.iter().zip(to).map(|(&a, &b)| ((a as u32 * (256 - k) + b as u32 * k + 128) >> 8) as u8));
}

/// The background: the style's screen floor, a little deeper at the top, a faint wash of the
/// accent low in the middle and darker corners; dithered so the encoder keeps it smooth.
fn background(look: &Look) -> Vec<u8> {
    let s = SIZE as usize;
    let bg = look.background;
    let dark = luma(bg) < 0.5;
    let top = mix(bg, [0.0; 3], if dark { 0.35 } else { 0.06 });
    let bottom = mix(bg, look.accent, if dark { 0.10 } else { 0.06 });
    let mut rgba = Vec::with_capacity(s * s * 4);
    let mut seed = 0x9e37_79b9u32;
    for y in 0..s {
        let ty = y as f32 / (s - 1) as f32;
        let base = mix(top, bottom, ty * ty * (3.0 - 2.0 * ty));
        for x in 0..s {
            let (dx, dy) = (x as f32 / s as f32 - 0.5, ty - 0.62);
            let glow = (1.0 - (dx * dx + dy * dy).sqrt() / 0.55).max(0.0);
            let mut c = mix(base, look.accent, glow * glow * if dark { 0.16 } else { 0.08 });
            let (vx, vy) = (x as f32 / s as f32 - 0.5, ty - 0.5);
            let vignette = 1.0 - 0.32 * ((vx * vx + vy * vy) * 2.0).min(1.0).powf(1.6);
            c = c.map(|v| v * vignette);
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let n = (seed >> 24) as f32 / 255.0 - 0.5;
            rgba.extend_from_slice(&[to_u8(c[0] + n / 255.0), to_u8(c[1] + n / 255.0), to_u8(c[2] + n / 255.0), 255]);
        }
    }
    let mut nv12 = Vec::new();
    makepad_video::nv12::rgba8_to_nv12(&rgba, SIZE, SIZE, COLOR, &mut nv12);
    nv12
}

// ---------------------------------------------------------------------------
// The painter
// ---------------------------------------------------------------------------

const MARGIN: f32 = 88.0 * K;
/// The layout is drawn for 1080 px and scaled to [`SIZE`].
const K: f32 = SIZE as f32 / 1080.0;

/// A line's laid-out text, made the first time it shows.
struct LineArt {
    main: Block,
    translation: Option<Block>,
    next: Block,
    section: Option<Block>,
}

/// Draws the scenes of one song.
pub struct Painter<'a> {
    song: &'a Song,
    look: &'a Look,
    lines: Vec<Flat<'a>>,
    bold: Chain,
    regular: Chain,
    italic: Chain,
    background: Vec<u8>,
    header: Option<(Block, Block)>,
    title_card: Option<(Block, Block)>,
    art: HashMap<usize, LineArt>,
}

impl<'a> Painter<'a> {
    pub fn new(song: &'a Song, look: &'a Look, fonts: &Fonts) -> Painter<'a> {
        let mut ids = 0x6c79_7269_6300u64;
        Painter {
            song,
            look,
            lines: song.lyrics.as_ref().map(flat).unwrap_or_default(),
            bold: Chain::load(&fonts.bold, &mut ids),
            regular: Chain::load(&fonts.regular, &mut ids),
            italic: Chain::load(&fonts.italic, &mut ids),
            background: background(look),
            header: None,
            title_card: None,
            art: HashMap::new(),
        }
    }

    fn caps(&self, text: &str) -> String {
        if self.look.caps { text.to_uppercase() } else { text.to_string() }
    }

    /// The scene as an NV12 frame (SIZE x SIZE) in `out`.
    pub fn paint(&mut self, scene: Scene, out: &mut Vec<u8>) {
        out.clear();
        out.extend_from_slice(&self.background);
        match scene {
            Scene::Title => self.paint_title(out),
            Scene::Line { line, lit } => {
                self.paint_header(out);
                self.paint_line(out, line, Some(lit));
            }
            Scene::Gap { next } => {
                self.paint_header(out);
                self.paint_line(out, next, None);
            }
        }
    }

    fn paint_title(&mut self, out: &mut [u8]) {
        let max_w = SIZE as f32 - 2.0 * MARGIN;
        if self.title_card.is_none() {
            let title = layout(&self.bold, &words_of(&self.caps(&self.song.title)), 104.0 * K, 60.0 * K, max_w, 0.0, 1.12);
            let artist = layout(&self.regular, &words_of(self.song.artist.as_deref().unwrap_or("")), 44.0 * K, 30.0 * K, max_w, 0.0, 1.3);
            self.title_card = Some((title, artist));
        }
        let (title, artist) = self.title_card.as_ref().unwrap();
        let rule_gap = 40.0 * K;
        let block = 8.0 * K + rule_gap + title.height() + 22.0 * K + artist.height();
        let top = (SIZE as f32 - block) * 0.46;
        let accent = ink(self.look.accent, 1.0);
        fill_rect(out, MARGIN as i32, top as i32, (132.0 * K) as i32, (8.0 * K) as i32, accent);
        let mut y = top + 8.0 * K + rule_gap;
        let text = ink(self.look.text, 1.0);
        for row in &title.rows {
            blit(out, &row.mask, (MARGIN - PAD) as i32, (y + title.line_h * 0.82 - row.mask.ascent) as i32, f32::INFINITY, text, text);
            y += title.line_h;
        }
        y += 22.0 * K;
        let muted = ink(self.look.muted, 1.0);
        for row in &artist.rows {
            blit(out, &row.mask, (MARGIN - PAD) as i32, (y + artist.line_h * 0.8 - row.mask.ascent) as i32, f32::INFINITY, muted, muted);
            y += artist.line_h;
        }
    }

    fn paint_header(&mut self, out: &mut [u8]) {
        if self.header.is_none() {
            let max_w = SIZE as f32 - 2.0 * MARGIN - 24.0 * K;
            let title = layout(&self.bold, &words_of(&self.caps(&self.song.title)), 30.0 * K, 22.0 * K, max_w * 0.62, 0.0, 1.2);
            let artist = layout(&self.regular, &words_of(self.song.artist.as_deref().unwrap_or("")), 28.0 * K, 20.0 * K, max_w * 0.36, 0.0, 1.2);
            self.header = Some((title, artist));
        }
        let (title, artist) = self.header.as_ref().unwrap();
        let base = 96.0 * K;
        fill_rect(out, MARGIN as i32, (base - 24.0 * K) as i32, (6.0 * K).max(3.0) as i32, (30.0 * K) as i32, ink(self.look.accent, 1.0));
        let x = MARGIN + 22.0 * K;
        let text = ink(self.look.text, 0.9);
        let mut right = x;
        if let Some(row) = title.rows.first() {
            blit(out, &row.mask, (x - PAD) as i32, (base - row.mask.ascent) as i32, f32::INFINITY, text, text);
            right = x + row.width;
        }
        let muted = ink(self.look.muted, 0.9);
        if let Some(row) = artist.rows.first() {
            blit(out, &row.mask, (right + 20.0 * K - PAD) as i32, (base - row.mask.ascent) as i32, f32::INFINITY, muted, muted);
        }
    }

    fn art(&mut self, line: usize) -> &LineArt {
        if !self.art.contains_key(&line) {
            let max_w = SIZE as f32 - 2.0 * MARGIN;
            let l = &self.lines[line];
            let words: Vec<(String, bool)> = l.words.iter().map(|w| (w.text.clone(), w.space)).collect();
            let main = layout(&self.bold, &words, 76.0 * K, 46.0 * K, max_w, 0.0, 1.18);
            let translation = (!l.translation.trim().is_empty()).then(|| layout(&self.italic, &words_of(l.translation), 42.0 * K, 30.0 * K, max_w, 0.0, 1.25));
            let next = layout(&self.regular, &words, 40.0 * K, 28.0 * K, max_w, 0.0, 1.25);
            let section = (!l.section.trim().is_empty())
                .then(|| layout(&self.bold, &words_of(&l.section.to_uppercase()), 24.0 * K, 18.0 * K, max_w, 0.2, 1.2));
            self.art.insert(line, LineArt { main, translation, next, section });
        }
        &self.art[&line]
    }

    /// Line `line` in the middle: lit up to `lit` words (`None`: dim, the line coming up), its
    /// translation, and the line after it dim below.
    fn paint_line(&mut self, out: &mut [u8], line: usize, lit: Option<usize>) {
        let look = self.look.clone();
        let next = (line + 1 < self.lines.len()).then_some(line + 1);
        if let Some(n) = next {
            self.art(n);
        }
        self.art(line);
        let art = &self.art[&line];
        let next_art = next.map(|n| &self.art[&n]);
        let section_h = art.section.as_ref().map_or(0.0, |s| s.height() + 26.0 * K);
        let trans_h = art.translation.as_ref().map_or(0.0, |t| 18.0 * K + t.height());
        // the main line keeps its place: the block centres on the line and its translation
        let core = art.main.height() + trans_h;
        let mut y = (SIZE as f32 * 0.52 - core * 0.5 - section_h).max(150.0 * K);
        let centre = SIZE as f32 * 0.5;
        if let Some(section) = &art.section {
            let label = ink(look.muted, 0.9);
            for row in &section.rows {
                blit(out, &row.mask, (centre - row.width * 0.5 - PAD) as i32, (y + section.line_h * 0.8 - row.mask.ascent) as i32, f32::INFINITY, label, label);
                y += section.line_h;
            }
            y += 26.0 * K;
        }
        let (lit_ink, unlit_ink) = match lit {
            Some(_) => (ink(look.accent, 1.0), ink(look.text, 1.0)),
            None => (ink(look.text, 0.34), ink(look.text, 0.34)),
        };
        let lit = lit.unwrap_or(0);
        for row in &art.main.rows {
            // lit words: the row up to the right edge of its last lit word
            let split = match row.words.iter().rev().find(|(i, _)| *i < lit) {
                None => f32::NEG_INFINITY,
                Some(&(i, _)) if row.words.last().is_some_and(|(j, _)| *j == i) => f32::INFINITY,
                Some(&(_, right)) => right + 0.5,
            };
            blit(out, &row.mask, (centre - row.width * 0.5 - PAD) as i32, (y + art.main.line_h * 0.8 - row.mask.ascent) as i32, split, lit_ink, unlit_ink);
            y += art.main.line_h;
        }
        if let Some(t) = &art.translation {
            y += 18.0 * K;
            let muted = ink(look.muted, if lit_ink.a < 1.0 { 0.4 } else { 1.0 });
            for row in &t.rows {
                blit(out, &row.mask, (centre - row.width * 0.5 - PAD) as i32, (y + t.line_h * 0.8 - row.mask.ascent) as i32, f32::INFINITY, muted, muted);
                y += t.line_h;
            }
        }
        if let Some(n) = next_art {
            y += 64.0 * K;
            let dim = ink(mix(look.text, look.background, 0.25), 0.36);
            for row in &n.next.rows {
                if y > SIZE as f32 - 40.0 * K {
                    break;
                }
                blit(out, &row.mask, (centre - row.width * 0.5 - PAD) as i32, (y + n.next.line_h * 0.8 - row.mask.ascent) as i32, f32::INFINITY, dim, dim);
                y += n.next.line_h;
            }
        }
    }
}

fn fill_rect(frame: &mut [u8], x: i32, y: i32, w: i32, h: i32, ink: Ink) {
    let mask = Mask { w: w.max(0) as usize, h: h.max(0) as usize, a: vec![255; (w.max(0) * h.max(0)) as usize], ascent: 0.0 };
    blit(frame, &mask, x, y, f32::INFINITY, ink, ink);
}

/// A scene change worth a crossfade (a new line, the card); a word lighting up snaps.
pub fn fades(from: Scene, to: Scene) -> bool {
    !matches!((from, to), (Scene::Line { line: a, .. }, Scene::Line { line: b, .. }) if a == b)
}

/// The frames of the movie: (tick, how to make the picture), ending with the last tick before
/// `end_tick`. Scenes are drawn when they start; a fade blends the shown picture into the new one
/// over [`FADE_FRAMES`]; a held picture repeats every tick (constant rate).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Frame {
    /// Draw the scene of this key of the timeline and show it.
    Scene(usize),
    /// The fade from the picture shown into the scene of this key, this far (0..1).
    Fade(usize, f32),
    /// The picture shown last, again.
    Repeat,
}

pub fn frames(keys: &[(i64, Scene)], end_tick: i64) -> Vec<(i64, Frame)> {
    let end_tick = end_tick.max(1);
    let mut out: Vec<(i64, Frame)> = Vec::new();
    for (k, &(tick, scene)) in keys.iter().enumerate() {
        if tick >= end_tick {
            break;
        }
        let until = keys.get(k + 1).map_or(end_tick, |n| n.0.min(end_tick));
        let mut t = tick;
        if k > 0 && fades(keys[k - 1].1, scene) {
            for j in 0..FADE_FRAMES {
                if t >= until {
                    break;
                }
                let x = (j + 1) as f32 / FADE_FRAMES as f32;
                out.push((t, Frame::Fade(k, x * x * (3.0 - 2.0 * x))));
                t += 1;
            }
        } else {
            out.push((t, Frame::Scene(k)));
            t += 1;
        }
        // held: the same picture every tick. Messaging apps expect a constant frame rate (a
        // variable-rate movie stalled WhatsApp's upload); repeats encode to near-empty frames.
        out.extend((t..until).map(|t| (t, Frame::Repeat)));
    }
    out
}
