//! 3D extruded type with one mesh per letter, for kinetic 3D typography.
//!
//! The glyph work (outlines, counters, bevels, the verified triangulation)
//! is [`crate::text_mesh`]'s builder; this module lays the text out with a
//! window-less [`Layouter`], builds it in the `Block` layout and splits the
//! stream into letters, each around its own rest centre so a document can
//! move, turn and scale every letter about itself. Each letter also keeps
//! its faces by part (front and back caps, side walls, bevel band: the
//! builder's face classes), so the caps, walls and bevel can wear different
//! materials, and the text records the rest centre of every word and line
//! so a word or a line can turn about its own middle.

use makepad_geom3d::mesh::*;
use makepad_geom3d::fx_mesh::{FxMesh, VERT_FLOATS};
use crate::text_mesh::{build_text_mesh, BevelType, TextLayout, TextMeshParams};
use makepad_draw::text::geom::Size;
use makepad_draw::text::font::FontId;
use makepad_draw::text::font_family::FontFamilyId;
use makepad_draw::text::layouter::{BorrowedLayoutParams, LaidoutText, LayoutOptions, Layouter, Settings, Style};
use makepad_draw::text::loader::{FontDefinition, FontFamilyDefinition};
use makepad_draw::SharedBytes;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

/// Where a font comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum FontSource {
    /// A font shipped with Makepad: "sans" (IBM Plex Sans), "bold" (Plex
    /// SemiBold), "italic", "bold_italic", "mono" (Liberation Mono), "inter"
    /// (Inter, variable weight), "roboto" (Roboto Flex), "noto" (Noto Sans).
    Bundled(String),
    /// A .ttf/.otf file.
    Path(PathBuf),
    /// Font file bytes (a library asset).
    Bytes(Arc<Vec<u8>>),
}

impl Default for FontSource {
    fn default() -> Self {
        FontSource::Bundled("sans".into())
    }
}

/// Makepad's bundled font directory (`widgets/resources`), or
/// `MOTION3D_FONTS_DIR` when set.
pub fn fonts_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("MOTION3D_FONTS_DIR") {
        return PathBuf::from(dir);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../widgets/resources")
}

/// The file behind a bundled font name, `None` for an unknown name.
pub fn bundled_font_file(name: &str) -> Option<&'static str> {
    Some(match name.trim().to_ascii_lowercase().as_str() {
        "sans" | "regular" | "plex" | "default" => "IBMPlexSans-Text.ttf",
        "bold" | "semibold" => "IBMPlexSans-SemiBold.ttf",
        "italic" => "IBMPlexSans-Italic.ttf",
        "bold_italic" => "IBMPlexSans-BoldItalic.ttf",
        "mono" => "LiberationMono-Regular.ttf",
        "inter" => "Inter.ttf",
        "roboto" => "RobotoFlex.ttf",
        "noto" => "NotoSans-Regular.ttf",
        _ => return None,
    })
}

/// Every bundled font name, for messages.
pub const BUNDLED_FONTS: &[&str] = &["sans", "bold", "italic", "bold_italic", "mono", "inter", "roboto", "noto"];

impl FontSource {
    /// The font file's bytes, or a message naming the fix.
    pub fn load(&self) -> Result<Vec<u8>, String> {
        match self {
            FontSource::Bundled(name) => {
                let file = bundled_font_file(name).ok_or_else(|| format!("no bundled font `{name}`; bundled fonts: {}", BUNDLED_FONTS.join(", ")))?;
                let path = fonts_dir().join(file);
                std::fs::read(&path).map_err(|e| format!("bundled font `{name}` ({}): {e}", path.display()))
            }
            FontSource::Path(path) => std::fs::read(path).map_err(|e| format!("font {}: {e}", path.display())),
            FontSource::Bytes(bytes) => Ok(bytes.as_ref().clone()),
        }
    }
}

/// Horizontal alignment of the lines within the block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    #[default]
    Center,
    Right,
}

/// The profile of the edge band between the side walls and the caps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BevelProfile {
    /// No band: a hard edge (the same as `bevel: 0`).
    Flat,
    /// A straight 45° cut (`bevel_segments` subdivide it).
    #[default]
    Chamfer,
    /// A convex quarter round.
    Round,
    /// A concave quarter round.
    Cove,
    /// Flat terraces.
    Step,
    /// An S moulding: a cove running into a round.
    Ogee,
}

impl BevelProfile {
    pub const NAMES: &'static [&'static str] = &["flat", "chamfer", "round", "cove", "step", "ogee"];
    pub fn by_name(s: &str) -> Option<Self> {
        Some(match s {
            "flat" | "none" => Self::Flat,
            "chamfer" => Self::Chamfer,
            "round" | "rounded" => Self::Round,
            "cove" => Self::Cove,
            "step" | "steps" => Self::Step,
            "ogee" => Self::Ogee,
            _ => return None,
        })
    }
}

/// The faces of a letter by part.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextPart {
    /// The front cap (+z).
    Front,
    /// The back cap (−z).
    Back,
    /// The side walls.
    Side,
    /// The bevel band (both sides).
    Bevel,
}

impl TextPart {
    pub const ALL: [TextPart; 4] = [TextPart::Front, TextPart::Back, TextPart::Side, TextPart::Bevel];
    fn of_class(class: f32) -> Option<Self> {
        Some(match class as i32 {
            0 => Self::Front,
            1 => Self::Back,
            2 => Self::Side,
            3 => Self::Bevel,
            _ => return None,
        })
    }
}

/// What to build.
#[derive(Clone, Debug, PartialEq)]
pub struct Text3dParams {
    /// The text; `\n` breaks lines.
    pub text: String,
    /// The font. Default bundled "sans".
    pub font: FontSource,
    /// Variable-font weight (100..900) for fonts with a `wght` axis ("inter"); None = the font's own.
    pub weight: Option<f32>,
    /// Other variable-font axes as (tag, value), e.g. `wdth` 125 or `slnt`
    /// -10 (four-letter tags packed big-endian); axes the font lacks are
    /// ignored. Default none.
    pub axes: Vec<(u32, f32)>,
    /// Cap height of the first line, world units. Default 1.
    pub size: f32,
    /// Extrusion depth; 0 = flat front faces only. Default 0.3.
    pub depth: f32,
    /// Bevel width (clamped to 0.4 depth and 0.3 size); 0 = none (default).
    pub bevel: f32,
    /// Rings across the bevel band (1..8). Default 1 (4 for round profiles).
    pub bevel_segments: u32,
    /// The bevel band's profile. Default chamfer.
    pub bevel_profile: BevelProfile,
    /// Curve flattening tolerance in em (0.004..0.05). Default 0.012.
    pub detail: f32,
    /// Extra letter spacing in em (-0.5..2). Default 0.
    pub tracking: f32,
    /// Line spacing multiplier. Default 1.
    pub line_height: f32,
    /// Alignment of lines within the block. Default Center.
    pub align: TextAlign,
    /// Wrap width in world units; None = explicit lines only (default).
    pub wrap: Option<f32>,
    /// Also sample the letters into cubes: (cells per cap height 4..40,
    /// layers 1..12) — vj_fx's voxel layout, letter cells only.
    pub voxels: Option<(u32, u32)>,
}

impl Default for Text3dParams {
    fn default() -> Self {
        Self {
            text: String::new(),
            font: FontSource::default(),
            weight: None,
            axes: Vec::new(),
            size: 1.0,
            depth: 0.3,
            bevel: 0.0,
            bevel_segments: 1,
            bevel_profile: BevelProfile::Chamfer,
            detail: 0.012,
            tracking: 0.0,
            line_height: 1.0,
            align: TextAlign::Center,
            wrap: None,
            voxels: None,
        }
    }
}

/// One letter: its solid around its own rest centre.
#[derive(Clone, Debug, PartialEq)]
pub struct Letter {
    /// Vertices relative to `pivot`, every part.
    pub mesh: Mesh,
    /// The same faces split by part (parts without faces are left out).
    pub parts: Vec<(TextPart, Mesh)>,
    /// Rest centre in text space (block centred on the origin, y up, front faces +z).
    pub pivot: [f32; 3],
    /// Ordinal among the letters with ink (spaces are not letters).
    pub index: usize,
    pub word: usize,
    pub line: usize,
    pub ch: char,
}

/// A built text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Text3d {
    pub letters: Vec<Letter>,
    pub words: usize,
    pub lines: usize,
    /// Rest centre of every word and every line (ink bounds), text space.
    pub word_centers: Vec<[f32; 3]>,
    pub line_centers: Vec<[f32; 3]>,
    /// Bounds of the whole block in text space, `(min, max)`.
    pub bounds: ([f32; 3], [f32; 3]),
    /// What the builder had to degrade or could not do.
    pub warnings: Vec<String>,
    /// With `Text3dParams::voxels`: the letter cells as cubes.
    pub voxels: Vec<Voxel>,
    /// Half extents of one voxel cube.
    pub voxel_half: [f32; 3],
}

/// One cube of a voxel letter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Voxel {
    /// Rest centre in text space.
    pub center: [f32; 3],
    /// The glyph ordinal it belongs to (`Letter::index`).
    pub glyph: usize,
}

impl Text3d {
    /// Every letter placed at its pivot, as one mesh.
    pub fn merged(&self) -> Mesh {
        let mut out = Mesh::new();
        for l in &self.letters {
            let mut m = l.mesh.clone();
            m.positions.iter_mut().for_each(|p| *p = add3(*p, l.pivot));
            out.append(&m);
        }
        out
    }
}

/// 100 lpx reference size, in points (as vj_fx lays text out).
const REF_PTS: f32 = 100.0 * 72.0 / 96.0;

static RECORDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

type GlyphRecord = (u64, std::collections::BTreeSet<u16>);

fn recorded() -> &'static (std::sync::mpsc::Sender<GlyphRecord>, std::sync::Mutex<std::sync::mpsc::Receiver<GlyphRecord>>) {
    static Q: std::sync::OnceLock<(std::sync::mpsc::Sender<GlyphRecord>, std::sync::Mutex<std::sync::mpsc::Receiver<GlyphRecord>>)> = std::sync::OnceLock::new();
    Q.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        (tx, std::sync::Mutex::new(rx))
    })
}

/// Keeps (or stops keeping) the glyphs every [`build_text3d`] from now on
/// lays out, on any thread: how a build tool learns which glyphs of which
/// font files 3D type uses (for instance to ship only those).
pub fn record_glyphs(on: bool) {
    RECORDING.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// The glyphs recorded since the last call: per build, the font file's
/// content hash (FNV-1a 64 over its bytes) and the glyph ids laid out.
pub fn take_recorded_glyphs() -> Vec<(u64, std::collections::BTreeSet<u16>)> {
    let Ok(rx) = recorded().1.lock() else { return Vec::new() };
    rx.try_iter().collect()
}

fn fnv64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

/// Lay out and extrude `params.text`.
pub fn build_text3d(params: &Text3dParams) -> Result<Text3d, String> {
    let bytes = params.font.load()?;
    let recording = RECORDING.load(std::sync::atomic::Ordering::Relaxed).then(|| fnv64(&bytes));
    // Only shaping and metrics are needed: keep the rasterizer's atlas tiny
    // (the default allocates a 2048² atlas per layouter).
    let mut settings = Settings::default();
    settings.loader.rasterizer.atlas_size = Size::new(16, 16);
    settings.cache_size = 8;
    let mut layouter = Layouter::new(settings);
    layouter.record_glyphs(recording.is_some());
    let font_id: FontId = 0x4D33_4400_0001_u64.into();
    let family: FontFamilyId = 0x4D33_4400_0101_u64.into();
    layouter.define_font(
        font_id,
        FontDefinition {
            data: SharedBytes::from_vec(bytes),
            index: 0,
            ascender_fudge_in_ems: 0.0,
            descender_fudge_in_ems: 0.0,
            weight: params.weight,
            variations: params.axes.clone(),
        },
    );
    layouter.define_font_family(family, FontFamilyDefinition { font_ids: vec![font_id], expected_member_count: 1, diagnostics: Default::default() });
    let align = match params.align {
        TextAlign::Left => 0.0,
        TextAlign::Center => 0.5,
        TextAlign::Right => 1.0,
    };
    let line_height = if params.line_height.is_finite() && params.line_height > 0.0 { params.line_height } else { 1.0 };
    let mut layout = |max: Option<f32>, wrap: bool| -> Rc<LaidoutText> {
        layouter.get_or_layout(BorrowedLayoutParams {
            text: &params.text,
            style: Style { font_family_id: family, font_size_in_pts: REF_PTS, color: None },
            options: LayoutOptions { max_width_in_lpxs: max, wrap, align, line_spacing_scale: line_height, ..LayoutOptions::default() },
        })
    };
    let mut laidout = layout(None, false);
    let size = if params.size.is_finite() && params.size > 0.0 { params.size } else { 1.0 };
    if let Some(wrap) = params.wrap.filter(|w| w.is_finite() && *w > 0.0) {
        let cap = laidout.rows.first().map(|r| r.cap_height_in_lpxs).filter(|c| *c > 0.0).unwrap_or(70.0);
        laidout = layout(Some(wrap * cap / size), true);
    } else if laidout.rows.len() > 1 && align > 0.0 {
        // Explicit lines align within the widest one (the layouter aligns
        // a row within the max width, which defaults to the row's own).
        let widest = laidout.rows.iter().map(|r| r.width_in_lpxs).fold(0.0f32, f32::max);
        laidout = layout(Some(widest), false);
    }
    if let Some(hash) = recording {
        let glyphs = layouter.take_recorded_glyphs().into_iter().flat_map(|r| r.glyphs).collect();
        recorded().0.send((hash, glyphs)).ok();
    }
    let mut fx = FxMesh::default();
    let report = build_text_mesh(
        &laidout,
        &TextMeshParams {
            size,
            depth: params.depth.max(0.0),
            bevel: if params.bevel_profile == BevelProfile::Flat { 0.0 } else { params.bevel.max(0.0) },
            detail: params.detail,
            layout: TextLayout::Block,
            tracking: params.tracking,
            bevel_type: match params.bevel_profile {
                BevelProfile::Flat | BevelProfile::Chamfer => BevelType::Chamfer,
                BevelProfile::Round => BevelType::Round,
                BevelProfile::Cove => BevelType::Cove,
                BevelProfile::Step => BevelType::Step,
                BevelProfile::Ogee => BevelType::Ogee,
            },
            bevel_rings: params.bevel_segments.clamp(1, 8) as usize,
            ..TextMeshParams::default()
        },
        &mut fx,
    );
    let (voxels, voxel_half) = match params.voxels {
        Some((res, layers)) => {
            let mut vx = FxMesh::default();
            let r = build_text_mesh(
                &laidout,
                &TextMeshParams {
                    size,
                    depth: params.depth.max(0.0),
                    detail: params.detail,
                    layout: TextLayout::Voxel,
                    tracking: params.tracking,
                    voxel_res: res.clamp(4, 40) as usize,
                    voxel_layers: layers.clamp(1, 12) as usize,
                    voxel_waste: false,
                    ..TextMeshParams::default()
                },
                &mut vx,
            );
            // 24 vertices per cube; letter cubes (class 4) only.
            let half = r.voxel_half;
            let layers = r.voxel_layers.max(1) as f32;
            let cubes = vx
                .verts
                .chunks_exact(VERT_FLOATS * 24)
                .filter(|c| c[7].floor() == 4.0)
                .map(|c| {
                    let layer = (c[11].max(0.0) % 1000.0).floor();
                    Voxel { center: [c[8], c[9], half[2] * (layers - 1.0 - 2.0 * layer)], glyph: c[3].max(0.0).round() as usize }
                })
                .collect();
            (cubes, half)
        }
        None => (Vec::new(), [0.0; 3]),
    };
    // The characters of the ink glyphs, in the builder's order.
    let mut chars = Vec::new();
    for row in &laidout.rows {
        let text = row.text.as_str();
        for g in &row.glyphs {
            if let Some(c) = text.get(g.cluster.min(text.len())..).and_then(|s| s.chars().next()) {
                if !c.is_whitespace() {
                    chars.push(c);
                }
            }
        }
    }
    let mut out = Text3d { words: report.words, lines: report.lines, warnings: report.warnings.clone(), voxels, voxel_half, ..Default::default() };
    let v = |i: usize| &fx.verts[i * VERT_FLOATS..(i + 1) * VERT_FLOATS];
    let n_verts = fx.verts.len() / VERT_FLOATS;
    // Glyph ordinal -> (letter slot, vertex remap); per part the same.
    let mut slots: Vec<Option<usize>> = Vec::new();
    let mut remap = vec![u32::MAX; n_verts];
    let mut part_remap = vec![u32::MAX; n_verts];
    for tri in fx.idx.chunks_exact(3) {
        let first = v(tri[0] as usize);
        let class = first[7].floor();
        if !(0.0..=3.0).contains(&class) || first[3] < 0.0 {
            continue;
        }
        let glyph = first[3].round() as usize;
        if slots.len() <= glyph {
            slots.resize(glyph + 1, None);
        }
        let slot = *slots[glyph].get_or_insert_with(|| {
            out.letters.push(Letter {
                mesh: Mesh::new(),
                parts: Vec::new(),
                pivot: [first[8], first[9], 0.0],
                index: glyph,
                word: first[10].max(0.0).floor() as usize,
                line: (first[11].max(0.0) % 1000.0) as usize,
                ch: chars.get(glyph).copied().unwrap_or('?'),
            });
            out.letters.len() - 1
        });
        let mut ids = [0u32; 3];
        for (k, &i) in tri.iter().enumerate() {
            let i = i as usize;
            if remap[i] == u32::MAX {
                let s = v(i);
                remap[i] = out.letters[slot].mesh.vertex([s[0], s[1], s[2]], normalize_or_up([s[4], s[5], s[6]]), [0.0, 0.0]);
            }
            ids[k] = remap[i];
        }
        out.letters[slot].mesh.tri(ids[0], ids[1], ids[2]);
        // The same triangle in its part's mesh (a vertex belongs to one
        // class: the builder never shares vertices across classes).
        let Some(part) = TextPart::of_class(class) else { continue };
        let parts = &mut out.letters[slot].parts;
        let k = match parts.iter().position(|(p, _)| *p == part) {
            Some(k) => k,
            None => {
                parts.push((part, Mesh::new()));
                parts.len() - 1
            }
        };
        let mut pids = [0u32; 3];
        for (j, &i) in tri.iter().enumerate() {
            let i = i as usize;
            if part_remap[i] == u32::MAX {
                let s = v(i);
                part_remap[i] = parts[k].1.vertex([s[0], s[1], s[2]], normalize_or_up([s[4], s[5], s[6]]), [0.0, 0.0]);
            }
            pids[j] = part_remap[i];
        }
        parts[k].1.tri(pids[0], pids[1], pids[2]);
    }
    for l in &mut out.letters {
        l.parts.sort_by_key(|(p, _)| TextPart::ALL.iter().position(|q| q == p));
    }
    out.letters.sort_by_key(|l| l.index);
    // Block bounds, then planar uvs across the whole block.
    let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
    for l in &out.letters {
        for p in &l.mesh.positions {
            let q = add3(*p, l.pivot);
            for k in 0..3 {
                min[k] = min[k].min(q[k]);
                max[k] = max[k].max(q[k]);
            }
        }
    }
    if out.letters.is_empty() {
        min = [0.0; 3];
        max = [0.0; 3];
    }
    out.bounds = (min, max);
    let (w, h) = ((max[0] - min[0]).max(1e-6), (max[1] - min[1]).max(1e-6));
    for l in &mut out.letters {
        let pivot = l.pivot;
        let uv = |p: &[f32; 3]| [(p[0] + pivot[0] - min[0]) / w, (p[1] + pivot[1] - min[1]) / h];
        l.mesh.uvs = l.mesh.positions.iter().map(uv).collect();
        for (_, m) in &mut l.parts {
            m.uvs = m.positions.iter().map(uv).collect();
        }
    }
    // Word and line rest centres: the middle of their letters' ink.
    let centres = |key: &dyn Fn(&Letter) -> usize, n: usize| -> Vec<[f32; 3]> {
        let mut bb = vec![([f32::MAX; 3], [f32::MIN; 3]); n];
        for l in &out.letters {
            let Some((mn, mx)) = l.mesh.bounds() else { continue };
            let b = &mut bb[key(l).min(n.saturating_sub(1))];
            for k in 0..3 {
                b.0[k] = b.0[k].min(mn[k] + l.pivot[k]);
                b.1[k] = b.1[k].max(mx[k] + l.pivot[k]);
            }
        }
        bb.iter().map(|(a, b)| if a[0] > b[0] { [0.0; 3] } else { [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5, (a[2] + b[2]) * 0.5] }).collect()
    };
    let words = out.letters.iter().map(|l| l.word + 1).max().unwrap_or(0);
    let lines = out.letters.iter().map(|l| l.line + 1).max().unwrap_or(0);
    out.word_centers = centres(&|l| l.word, words);
    out.line_centers = centres(&|l| l.line, lines);
    Ok(out)
}


// ---- traced letters: centre lines, outlines, tubes, LED dots ----------

/// A letter's front cap rasterized on a square lattice (letter-local
/// coordinates, the letter's pivot at the origin).
#[derive(Clone, Debug)]
pub struct InkGrid {
    pub w: usize,
    pub h: usize,
    /// Centre of cell (0, 0).
    pub origin: [f32; 2],
    pub cell: f32,
    pub ink: Vec<bool>,
}

impl InkGrid {
    pub fn at(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h && self.ink[y as usize * self.w + x as usize]
    }
    pub fn centre(&self, x: usize, y: usize) -> [f32; 2] {
        [self.origin[0] + x as f32 * self.cell, self.origin[1] + y as f32 * self.cell]
    }
}

/// Which lattice cells (centres) lie inside the letter's front cap.
pub fn ink_grid(letter: &Letter, cell: f32) -> InkGrid {
    let cell = cell.max(1e-3);
    let tris: Vec<[[f32; 2]; 3]> = letter
        .parts
        .iter()
        .filter(|(p, _)| *p == TextPart::Front)
        .flat_map(|(_, m)| m.indices.chunks_exact(3).map(move |t| [0, 1, 2].map(|k| { let q = m.positions[t[k] as usize]; [q[0], q[1]] })))
        .collect();
    let (mut mn, mut mx) = ([f32::MAX; 2], [f32::MIN; 2]);
    for t in &tris {
        for q in t {
            mn = [mn[0].min(q[0]), mn[1].min(q[1])];
            mx = [mx[0].max(q[0]), mx[1].max(q[1])];
        }
    }
    if tris.is_empty() {
        return InkGrid { w: 0, h: 0, origin: [0.0; 2], cell, ink: Vec::new() };
    }
    // One empty cell of margin all round (thinning needs a border).
    let origin = [mn[0] - cell, mn[1] - cell];
    let w = ((mx[0] - mn[0]) / cell).ceil() as usize + 3;
    let h = ((mx[1] - mn[1]) / cell).ceil() as usize + 3;
    let mut ink = vec![false; w * h];
    let side = |a: [f32; 2], b: [f32; 2], p: [f32; 2]| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
    for t in &tris {
        let lo = [t.iter().map(|q| q[0]).fold(f32::MAX, f32::min), t.iter().map(|q| q[1]).fold(f32::MAX, f32::min)];
        let hi = [t.iter().map(|q| q[0]).fold(f32::MIN, f32::max), t.iter().map(|q| q[1]).fold(f32::MIN, f32::max)];
        let x0 = (((lo[0] - origin[0]) / cell).floor().max(0.0)) as usize;
        let x1 = (((hi[0] - origin[0]) / cell).ceil() as usize).min(w - 1);
        let y0 = (((lo[1] - origin[1]) / cell).floor().max(0.0)) as usize;
        let y1 = (((hi[1] - origin[1]) / cell).ceil() as usize).min(h - 1);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let p = [origin[0] + x as f32 * cell, origin[1] + y as f32 * cell];
                let (a, b, c) = (side(t[0], t[1], p), side(t[1], t[2], p), side(t[2], t[0], p));
                if (a >= 0.0 && b >= 0.0 && c >= 0.0) || (a <= 0.0 && b <= 0.0 && c <= 0.0) {
                    ink[y * w + x] = true;
                }
            }
        }
    }
    InkGrid { w, h, origin, cell, ink }
}

/// The letter's centre lines: its ink thinned to one cell (Zhang–Suen),
/// traced into polylines between ends and junctions, spurs shorter than
/// `min_len` dropped, then smoothed. Each path says whether it is closed.
pub fn centre_lines(grid: &InkGrid, min_len: f32) -> Vec<(Vec<[f32; 2]>, bool)> {
    let (w, h) = (grid.w, grid.h);
    if w < 3 || h < 3 {
        return Vec::new();
    }
    let mut on = grid.ink.clone();
    let idx = |x: usize, y: usize| y * w + x;
    // Zhang–Suen thinning.
    loop {
        let mut changed = false;
        for pass in 0..2 {
            let mut clear = Vec::new();
            for y in 1..h - 1 {
                for x in 1..w - 1 {
                    if !on[idx(x, y)] {
                        continue;
                    }
                    // p2..p9 clockwise from north.
                    let n = [
                        on[idx(x, y + 1)], on[idx(x + 1, y + 1)], on[idx(x + 1, y)], on[idx(x + 1, y - 1)],
                        on[idx(x, y - 1)], on[idx(x - 1, y - 1)], on[idx(x - 1, y)], on[idx(x - 1, y + 1)],
                    ];
                    let b = n.iter().filter(|v| **v).count();
                    if !(2..=6).contains(&b) {
                        continue;
                    }
                    let a = (0..8).filter(|k| !n[*k] && n[(k + 1) % 8]).count();
                    if a != 1 {
                        continue;
                    }
                    let (p2, p4, p6, p8) = (n[0], n[2], n[4], n[6]);
                    let ok = if pass == 0 { !(p2 && p4 && p6) && !(p4 && p6 && p8) } else { !(p2 && p4 && p8) && !(p2 && p6 && p8) };
                    if ok {
                        clear.push(idx(x, y));
                    }
                }
            }
            changed |= !clear.is_empty();
            for i in clear {
                on[i] = false;
            }
        }
        if !changed {
            break;
        }
    }
    // Trace: nodes are cells with other than two neighbours.
    const D: [(i32, i32); 8] = [(0, 1), (1, 1), (1, 0), (1, -1), (0, -1), (-1, -1), (-1, 0), (-1, 1)];
    let get = |on: &[bool], x: i32, y: i32| x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && on[y as usize * w + x as usize];
    // 8-connected, but a diagonal step only where no side step connects the
    // two cells already (a staircase then reads as a line, not a fork).
    let neighbours = |on: &[bool], x: i32, y: i32| -> Vec<(i32, i32)> {
        D.iter()
            .filter(|(dx, dy)| get(on, x + dx, y + dy) && (*dx == 0 || *dy == 0 || (!get(on, x + dx, y) && !get(on, x, y + dy))))
            .map(|(dx, dy)| (x + dx, y + dy))
            .collect()
    };
    // Prune spurs: a branch from an end cell to a junction shorter than
    // `min_len` is thinning noise (a stroke's corner), not a stroke.
    let spur = (min_len / grid.cell).ceil() as usize;
    for _ in 0..3 {
        let mut removed = false;
        let ends: Vec<(i32, i32)> = (0..h).flat_map(|y| (0..w).map(move |x| (x as i32, y as i32))).filter(|(x, y)| on[*y as usize * w + *x as usize] && neighbours(&on, *x, *y).len() == 1).collect();
        for e in ends {
            let mut branch = vec![e];
            let (mut prev, mut cur) = (e, e);
            let reached_junction = loop {
                let ns: Vec<(i32, i32)> = neighbours(&on, cur.0, cur.1).into_iter().filter(|n| *n != prev).collect();
                if ns.len() != 1 {
                    break ns.len() > 1;
                }
                prev = cur;
                cur = ns[0];
                if neighbours(&on, cur.0, cur.1).len() > 2 {
                    break true;
                }
                branch.push(cur);
                if branch.len() > spur {
                    break false;
                }
            };
            if reached_junction && branch.len() <= spur {
                for c in branch {
                    on[c.1 as usize * w + c.0 as usize] = false;
                }
                removed = true;
            }
        }
        if !removed {
            break;
        }
    }
    let mut used_edge = std::collections::HashSet::new();
    let key = |a: (i32, i32), b: (i32, i32)| if a <= b { (a, b) } else { (b, a) };
    let mut paths: Vec<(Vec<(i32, i32)>, bool)> = Vec::new();
    let cells: Vec<(i32, i32)> = (0..h).flat_map(|y| (0..w).map(move |x| (x as i32, y as i32))).filter(|(x, y)| on[*y as usize * w + *x as usize]).collect();
    let is_node = |on: &[bool], c: (i32, i32)| neighbours(on, c.0, c.1).len() != 2;
    let walk = |start: (i32, i32), first: (i32, i32), used: &mut std::collections::HashSet<((i32, i32), (i32, i32))>, on: &[bool]| -> (Vec<(i32, i32)>, bool) {
        let mut path = vec![start, first];
        used.insert(key(start, first));
        let (mut prev, mut cur) = (start, first);
        loop {
            if cur == start {
                return (path, true);
            }
            if is_node(on, cur) {
                return (path, false);
            }
            let next = neighbours(on, cur.0, cur.1).into_iter().find(|n| *n != prev && !used.contains(&key(cur, *n)));
            let Some(next) = next else { return (path, false) };
            used.insert(key(cur, next));
            path.push(next);
            prev = cur;
            cur = next;
        }
    };
    for &c in &cells {
        if is_node(&on, c) {
            for n in neighbours(&on, c.0, c.1) {
                if !used_edge.contains(&key(c, n)) {
                    paths.push(walk(c, n, &mut used_edge, &on));
                }
            }
        }
    }
    // Loops with no node (an O): start anywhere on them.
    for &c in &cells {
        for n in neighbours(&on, c.0, c.1) {
            if !used_edge.contains(&key(c, n)) {
                paths.push(walk(c, n, &mut used_edge, &on));
            }
        }
    }
    let mut out = Vec::new();
    for (cells, closed) in paths {
        let mut pts: Vec<[f32; 2]> = cells.iter().map(|(x, y)| grid.centre(*x as usize, *y as usize)).collect();
        if closed {
            pts.pop();
        }
        let len: f32 = pts.windows(2).map(|q| ((q[1][0] - q[0][0]).powi(2) + (q[1][1] - q[0][1]).powi(2)).sqrt()).sum();
        if len < min_len && !closed {
            continue;
        }
        if pts.len() < 2 {
            continue;
        }
        out.push((smooth(&simplify(&pts, grid.cell * 0.6, closed), closed), closed));
    }
    out
}

/// Douglas–Peucker.
fn simplify(pts: &[[f32; 2]], tol: f32, closed: bool) -> Vec<[f32; 2]> {
    fn rec(p: &[[f32; 2]], tol: f32, out: &mut Vec<[f32; 2]>) {
        let (a, b) = (p[0], p[p.len() - 1]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l = (dx * dx + dy * dy).sqrt().max(1e-9);
        let (mut far, mut at) = (0.0, 0);
        for (i, q) in p.iter().enumerate().take(p.len() - 1).skip(1) {
            let d = ((q[0] - a[0]) * dy - (q[1] - a[1]) * dx).abs() / l;
            if d > far {
                far = d;
                at = i;
            }
        }
        if far > tol {
            rec(&p[..=at], tol, out);
            out.pop();
            rec(&p[at..], tol, out);
        } else {
            out.push(a);
            out.push(b);
        }
    }
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let mut out = Vec::new();
    if closed {
        // A loop starts and ends on the same point: split it at the point
        // farthest from the start and simplify the two halves.
        let d2 = |q: &[f32; 2]| (q[0] - pts[0][0]).powi(2) + (q[1] - pts[0][1]).powi(2);
        let far = (0..pts.len()).max_by(|a, b| d2(&pts[*a]).partial_cmp(&d2(&pts[*b])).unwrap()).unwrap_or(0).max(1);
        let mut second = pts[far..].to_vec();
        second.push(pts[0]);
        rec(&pts[..=far], tol, &mut out);
        out.pop();
        rec(&second, tol, &mut out);
        out.pop();
    } else {
        rec(pts, tol, &mut out);
    }
    out
}

/// Chaikin corner cutting, two rounds (ends of open paths kept).
fn smooth(pts: &[[f32; 2]], closed: bool) -> Vec<[f32; 2]> {
    let mut p = pts.to_vec();
    for _ in 0..2 {
        if p.len() < 3 {
            break;
        }
        let n = p.len();
        let mut q = Vec::with_capacity(n * 2);
        if !closed {
            q.push(p[0]);
        }
        let segs = if closed { n } else { n - 1 };
        for i in 0..segs {
            let (a, b) = (p[i], p[(i + 1) % n]);
            q.push([a[0] * 0.75 + b[0] * 0.25, a[1] * 0.75 + b[1] * 0.25]);
            q.push([a[0] * 0.25 + b[0] * 0.75, a[1] * 0.25 + b[1] * 0.75]);
        }
        if !closed {
            q.push(p[n - 1]);
        }
        p = q;
    }
    p
}

/// The front cap's boundary as closed loops (letter-local).
pub fn outline_loops(letter: &Letter) -> Vec<Vec<[f32; 2]>> {
    use std::collections::HashMap;
    let key = |p: [f32; 3]| ((p[0] * 1e4).round() as i64, (p[1] * 1e4).round() as i64);
    let mut count: HashMap<((i64, i64), (i64, i64)), i32> = HashMap::new();
    let mut pos: HashMap<(i64, i64), [f32; 2]> = HashMap::new();
    for (part, m) in &letter.parts {
        if *part != TextPart::Front {
            continue;
        }
        for t in m.indices.chunks_exact(3) {
            for k in 0..3 {
                let (a, b) = (m.positions[t[k] as usize], m.positions[t[(k + 1) % 3] as usize]);
                let (ka, kb) = (key(a), key(b));
                pos.insert(ka, [a[0], a[1]]);
                pos.insert(kb, [b[0], b[1]]);
                if ka == kb {
                    continue;
                }
                *count.entry((ka, kb)).or_insert(0) += 1;
            }
        }
    }
    // Boundary edges: no twin running the other way.
    let mut next: HashMap<(i64, i64), Vec<(i64, i64)>> = HashMap::new();
    for (&(a, b), _) in count.iter().filter(|(e, _)| !count.contains_key(&(e.1, e.0))) {
        next.entry(a).or_default().push(b);
    }
    let mut loops = Vec::new();
    let mut starts: Vec<(i64, i64)> = next.keys().copied().collect();
    starts.sort();
    for s in starts {
        while let Some(first) = next.get_mut(&s).and_then(|v| v.pop()) {
            let mut lp = vec![pos[&s]];
            let mut cur = first;
            let mut guard = 0;
            while cur != s && guard < 100_000 {
                lp.push(pos[&cur]);
                match next.get_mut(&cur).and_then(|v| v.pop()) {
                    Some(n) => cur = n,
                    None => break,
                }
                guard += 1;
            }
            if lp.len() >= 3 {
                loops.push(smooth(&lp, true));
            }
        }
    }
    loops
}

/// Round tubes of `radius` along 2D paths in the z = 0 plane, with round
/// ends on open paths.
pub fn tube_mesh(paths: &[(Vec<[f32; 2]>, bool)], radius: f32, sides: u32) -> Mesh {
    let sides = sides.clamp(3, 24) as usize;
    let mut m = Mesh::new();
    let tau = std::f32::consts::TAU;
    for (pts, closed) in paths {
        let n = pts.len();
        if n < 2 {
            continue;
        }
        let tangent = |i: usize| -> [f32; 2] {
            let (a, b) = if *closed { (pts[(i + n - 1) % n], pts[(i + 1) % n]) } else { (pts[i.saturating_sub(1)], pts[(i + 1).min(n - 1)]) };
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let l = (dx * dx + dy * dy).sqrt().max(1e-9);
            [dx / l, dy / l]
        };
        let base = m.positions.len() as u32;
        let rings = if *closed { n + 1 } else { n };
        for r in 0..rings {
            let i = r % n;
            let t = tangent(i);
            let side = [-t[1], t[0]];
            for k in 0..sides {
                let a = k as f32 / sides as f32 * tau;
                let nrm = [side[0] * a.cos(), side[1] * a.cos(), a.sin()];
                m.vertex([pts[i][0] + nrm[0] * radius, pts[i][1] + nrm[1] * radius, nrm[2] * radius], nrm, [r as f32 / rings as f32, k as f32 / sides as f32]);
            }
        }
        for r in 0..rings - 1 {
            for k in 0..sides {
                let a = base + (r * sides + k) as u32;
                let b = base + (r * sides + (k + 1) % sides) as u32;
                let c = base + ((r + 1) * sides + (k + 1) % sides) as u32;
                let d = base + ((r + 1) * sides + k) as u32;
                m.quad(a, d, c, b);
            }
        }
        if !*closed {
            // Round ends: a small half-sphere as a fan of rings.
            for (end, dir) in [(0usize, -1.0f32), (n - 1, 1.0)] {
                let t = tangent(end);
                let side = [-t[1], t[0]];
                let c = pts[end];
                let cap = m.positions.len() as u32;
                let steps = 3;
                for s in 0..=steps {
                    let phi = s as f32 / steps as f32 * std::f32::consts::FRAC_PI_2;
                    for k in 0..sides {
                        let a = k as f32 / sides as f32 * tau;
                        let ring = [side[0] * a.cos(), side[1] * a.cos(), a.sin()];
                        let nrm = [ring[0] * phi.cos() + t[0] * dir * phi.sin(), ring[1] * phi.cos() + t[1] * dir * phi.sin(), ring[2] * phi.cos()];
                        m.vertex([c[0] + nrm[0] * radius, c[1] + nrm[1] * radius, nrm[2] * radius], nrm, [0.0, 0.0]);
                    }
                }
                for s in 0..steps {
                    for k in 0..sides {
                        let a = cap + (s * sides + k) as u32;
                        let b = cap + (s * sides + (k + 1) % sides) as u32;
                        let cc = cap + ((s + 1) * sides + (k + 1) % sides) as u32;
                        let d = cap + ((s + 1) * sides + k) as u32;
                        if dir > 0.0 { m.quad(a, d, cc, b) } else { m.quad(a, b, cc, d) }
                    }
                }
            }
        }
    }
    m
}

/// LED dots: lattice points at `pitch` inside the letter (letter-local).
pub fn led_dots(letter: &Letter, pitch: f32) -> Vec<[f32; 2]> {
    let g = ink_grid(letter, pitch);
    (0..g.h).flat_map(|y| (0..g.w).map(move |x| (x, y))).filter(|(x, y)| g.ink[y * g.w + x]).map(|(x, y)| g.centre(x, y)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(text: &str) -> Text3d {
        build_text3d(&Text3dParams { text: text.into(), ..Default::default() }).unwrap()
    }

    #[test]
    fn letters_with_holes() {
        let t = build("HOB@8");
        assert_eq!(t.letters.len(), 5);
        assert_eq!(t.letters.iter().map(|l| l.ch).collect::<String>(), "HOB@8");
        for l in &t.letters {
            l.mesh.validate().unwrap();
            assert!(l.mesh.triangle_count() > 8);
        }
        // The O keeps its counter: no front-cap triangle covers its centre.
        let o = &t.letters[1];
        for tri in o.mesh.indices.chunks_exact(3) {
            let ps: Vec<[f32; 3]> = tri.iter().map(|&i| o.mesh.positions[i as usize]).collect();
            if ps.iter().all(|p| p[2] > 0.14) {
                let g = [(ps[0][0] + ps[1][0] + ps[2][0]) / 3.0, (ps[0][1] + ps[1][1] + ps[2][1]) / 3.0];
                assert!(g[0].hypot(g[1]) > 0.08, "cap triangle over the O's counter at {g:?}");
            }
        }
        // Letters read left to right, block centred.
        assert!(t.letters.windows(2).all(|w| w[0].pivot[0] < w[1].pivot[0]));
        assert!((t.bounds.0[0] + t.bounds.1[0]).abs() < 0.05);
        // Cap height is `size`.
        let h = &t.letters[0];
        let (mn, mx) = h.mesh.bounds().unwrap();
        assert!(((mx[1] - mn[1]) - 1.0).abs() < 0.03, "{}", mx[1] - mn[1]);
    }

    #[test]
    fn words_lines_and_determinism() {
        let t = build("AB CD\nEF");
        assert_eq!(t.letters.len(), 6);
        assert_eq!(t.letters.iter().map(|l| (l.word, l.line)).collect::<Vec<_>>(), vec![(0, 0), (0, 0), (1, 0), (1, 0), (2, 1), (2, 1)]);
        assert_eq!(t, build("AB CD\nEF"));
        assert!(t.letters[4].pivot[1] < t.letters[0].pivot[1]);
    }

    #[test]
    fn fonts_and_errors() {
        let mono = build_text3d(&Text3dParams { text: "ii".into(), font: FontSource::Bundled("mono".into()), depth: 0.0, ..Default::default() }).unwrap();
        assert_eq!(mono.letters.len(), 2);
        assert!(mono.letters[0].mesh.positions.iter().all(|p| p[2].abs() < 1e-5));
        let err = build_text3d(&Text3dParams { text: "x".into(), font: FontSource::Bundled("comic".into()), ..Default::default() }).unwrap_err();
        assert!(err.contains("bundled fonts"));
        let bytes = std::fs::read(fonts_dir().join("Inter.ttf")).unwrap();
        let t = build_text3d(&Text3dParams { text: "Hi".into(), font: FontSource::Bytes(Arc::new(bytes)), weight: Some(800.0), bevel: 0.03, bevel_segments: 3, ..Default::default() }).unwrap();
        assert_eq!(t.letters.len(), 2);
        let m = t.merged();
        m.validate().unwrap();
    }

    #[test]
    fn wrap_breaks_lines() {
        let t = build_text3d(&Text3dParams { text: "one two three four".into(), wrap: Some(4.0), ..Default::default() }).unwrap();
        assert!(t.lines >= 2, "{}", t.lines);
        assert!(t.bounds.1[0] - t.bounds.0[0] <= 4.5);
    }

    /// Every edge of the welded solid is shared by exactly two triangles
    /// running it in opposite directions (closed, consistently wound), and
    /// the solid encloses a positive volume (outward normals).
    fn assert_watertight(m: &Mesh, what: &str) {
        use std::collections::HashMap;
        let key = |p: [f32; 3]| [(p[0] * 1e4).round() as i64, (p[1] * 1e4).round() as i64, (p[2] * 1e4).round() as i64];
        let mut ids: HashMap<[i64; 3], u32> = HashMap::new();
        let weld: Vec<u32> = m.positions.iter().map(|p| { let n = ids.len() as u32; *ids.entry(key(*p)).or_insert(n) }).collect();
        let mut edges: HashMap<(u32, u32), i32> = HashMap::new();
        let mut volume = 0.0f64;
        for t in m.indices.chunks_exact(3) {
            let v: Vec<u32> = t.iter().map(|&i| weld[i as usize]).collect();
            if v[0] == v[1] || v[1] == v[2] || v[0] == v[2] {
                continue;
            }
            for k in 0..3 {
                *edges.entry((v[k], v[(k + 1) % 3])).or_insert(0) += 1;
            }
            let p: Vec<[f64; 3]> = t.iter().map(|&i| { let q = m.positions[i as usize]; [q[0] as f64, q[1] as f64, q[2] as f64] }).collect();
            volume += (p[0][0] * (p[1][1] * p[2][2] - p[1][2] * p[2][1]) - p[0][1] * (p[1][0] * p[2][2] - p[1][2] * p[2][0]) + p[0][2] * (p[1][0] * p[2][1] - p[1][1] * p[2][0])) / 6.0;
        }
        let open: Vec<_> = edges.iter().filter(|((a, b), n)| **n != 1 || edges.get(&(*b, *a)).copied() != Some(1)).take(4).collect();
        assert!(open.is_empty(), "{what}: {} open or doubled edges, e.g. {open:?}", edges.iter().filter(|((a, b), n)| **n != 1 || edges.get(&(*b, *a)).copied() != Some(1)).count());
        assert!(volume > 0.0, "{what}: inside out (volume {volume})");
    }

    #[test]
    fn solids_are_watertight_with_counters() {
        for profile in [BevelProfile::Flat, BevelProfile::Chamfer, BevelProfile::Round, BevelProfile::Ogee] {
            let t = build_text3d(&Text3dParams { text: "oAB8".into(), bevel: 0.06, bevel_segments: 3, bevel_profile: profile, ..Default::default() }).unwrap();
            assert_eq!(t.letters.len(), 4);
            for l in &t.letters {
                assert_watertight(&l.mesh, &format!("{:?} {}", profile, l.ch));
                // The parts cover the letter exactly.
                let tris: usize = l.parts.iter().map(|(_, m)| m.triangle_count()).sum();
                assert_eq!(tris, l.mesh.triangle_count());
                let has = |p: TextPart| l.parts.iter().any(|(q, _)| *q == p);
                assert!(has(TextPart::Front) && has(TextPart::Back) && has(TextPart::Side), "{:?}", l.parts.iter().map(|p| p.0).collect::<Vec<_>>());
                assert_eq!(has(TextPart::Bevel), profile != BevelProfile::Flat, "{profile:?} {}", l.ch);
                // Front caps face +z, back caps -z.
                for (part, m) in &l.parts {
                    let want = match part {
                        TextPart::Front => 1.0,
                        TextPart::Back => -1.0,
                        _ => continue,
                    };
                    assert!(m.normals.iter().all(|n| (n[2] - want).abs() < 1e-3), "{part:?} normals");
                    for tri in m.indices.chunks_exact(3) {
                        let p: Vec<[f32; 3]> = tri.iter().map(|&i| m.positions[i as usize]).collect();
                        let z = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[1][1] - p[0][1]) * (p[2][0] - p[0][0]);
                        assert!(z * want >= -1e-7, "{profile:?} {} {part:?} triangle wound against its normal: {z} {p:?}", l.ch);
                    }
                }
            }
            // The counters of o, A, B and 8 stay open: some point inside
            // each letter's box is not covered by its front cap.
        }
    }

    /// A wide bevel on thin strokes keeps every cap triangle's winding
    /// (vj_fx clamps the inset per point to the local stroke width).
    #[test]
    fn wide_bevels_never_fold_the_caps() {
        for font in ["sans", "italic", "mono", "bold"] {
            for profile in [BevelProfile::Chamfer, BevelProfile::Round] {
                let t = build_text3d(&Text3dParams { text: "oAB8gR&@%ew".into(), font: FontSource::Bundled(font.into()), bevel: 0.1, bevel_profile: profile, ..Default::default() }).unwrap();
                for l in &t.letters {
                    assert_watertight(&l.mesh, &format!("{font} {profile:?} {}", l.ch));
                    for (part, m) in &l.parts {
                        let want = match part {
                            TextPart::Front => 1.0,
                            TextPart::Back => -1.0,
                            _ => continue,
                        };
                        for tri in m.indices.chunks_exact(3) {
                            let p: Vec<[f32; 3]> = tri.iter().map(|&i| m.positions[i as usize]).collect();
                            let z = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[1][1] - p[0][1]) * (p[2][0] - p[0][0]);
                            assert!(z * want >= -1e-7, "{font} {profile:?} {} {part:?} cap folded: {z}", l.ch);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn unit_centres() {
        let t = build("AB CD\nEF");
        assert_eq!(t.word_centers.len(), 3);
        assert_eq!(t.line_centers.len(), 2);
        assert!(t.word_centers[0][0] < t.word_centers[1][0]);
        assert!(t.line_centers[1][1] < t.line_centers[0][1]);
        let mid = (t.letters[0].pivot[0] + t.letters[1].pivot[0]) * 0.5;
        assert!((t.word_centers[0][0] - mid).abs() < 0.2);
    }

    #[test]
    fn explicit_lines_align() {
        let t = build("WIDE LINE\nI");
        let i = t.letters.last().unwrap();
        // Centred: the short line's letter sits near the middle.
        assert!(i.pivot[0].abs() < 0.3, "{:?}", i.pivot);
        let left = build_text3d(&Text3dParams { text: "WIDE LINE\nI".into(), align: TextAlign::Left, ..Default::default() }).unwrap();
        assert!(left.letters.last().unwrap().pivot[0] < -1.0);
    }

    #[test]
    fn voxels_fill_the_letters() {
        let t = build_text3d(&Text3dParams { text: "HI".into(), voxels: Some((10, 2)), ..Default::default() }).unwrap();
        assert!(t.voxels.len() > 40, "{}", t.voxels.len());
        assert!(t.voxel_half[0] > 0.0);
        // Every cube sits inside its glyph's ink box.
        for v in &t.voxels {
            let l = t.letters.iter().find(|l| l.index == v.glyph).unwrap();
            let (mn, mx) = l.mesh.bounds().unwrap();
            assert!(v.center[0] >= mn[0] + l.pivot[0] - 0.05 && v.center[0] <= mx[0] + l.pivot[0] + 0.05, "{v:?}");
        }
        assert!(t.voxels.iter().any(|v| v.glyph == 0) && t.voxels.iter().any(|v| v.glyph == 1));
    }

    #[test]
    fn centre_lines_trace_strokes() {
        // Inter: a sans I (Plex's I has serif bars, traced as three strokes).
        let t = build_text3d(&Text3dParams { text: "OIL".into(), font: FontSource::Bundled("inter".into()), weight: Some(700.0), depth: 0.1, ..Default::default() }).unwrap();
        // A width axis widens the letters (Roboto Flex's wdth).
        let wide = |w: f32| {
            let t = build_text3d(&Text3dParams { text: "OIL".into(), font: FontSource::Bundled("roboto".into()), axes: vec![(u32::from_be_bytes(*b"wdth"), w)], depth: 0.1, ..Default::default() }).unwrap();
            let xs = t.letters.iter().flat_map(|l| l.mesh.positions.iter().map(move |p| p[0] + l.pivot[0]));
            let (lo, hi) = xs.fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)));
            hi - lo
        };
        assert!(wide(151.0) > wide(25.0) * 1.2, "wdth 151 is wider: {} vs {}", wide(151.0), wide(25.0));
        let o = centre_lines(&ink_grid(&t.letters[0], 0.02), 0.1);
        assert_eq!(o.len(), 1, "O is one loop: {:?}", o.iter().map(|p| (p.0.len(), p.1)).collect::<Vec<_>>());
        assert!(o[0].1, "closed");
        assert!(o[0].0.len() > 12, "the loop keeps its shape: {}", o[0].0.len());
        let i = centre_lines(&ink_grid(&t.letters[1], 0.02), 0.1);
        assert_eq!(i.len(), 1);
        assert!(!i[0].1);
        // The I's centre line runs up its middle, about a cap height long.
        let (a, b) = (i[0].0.first().unwrap(), i[0].0.last().unwrap());
        assert!((a[1] - b[1]).abs() > 0.7, "{a:?} {b:?}");
        assert!(a[0].abs() < 0.03 && b[0].abs() < 0.03);
        let l = centre_lines(&ink_grid(&t.letters[2], 0.02), 0.1);
        assert!(!l.is_empty() && l.len() <= 2, "{}", l.len());
        let tube = tube_mesh(&o, 0.03, 8);
        tube.validate().unwrap();
        assert_eq!(outline_loops(&t.letters[0]).len(), 2, "O: outer and counter");
        assert!(led_dots(&t.letters[1], 0.08).len() >= 10);
        let k = build_text3d(&Text3dParams { text: "OK".into(), font: FontSource::Bundled("inter".into()), ..Default::default() }).unwrap();
        for l in &k.letters {
            let p = centre_lines(&ink_grid(l, 0.018), 0.09);
            assert!(!p.is_empty(), "{}: no centre lines", l.ch);
        }
    }
}
