//! B1, the shapes: a text laid out and turned into one mesh per distinct
//! glyph, each around its own rest centre, plus the rest pose of every
//! element (a glyph, or a voxel cell) the animator places.
//!
//! The glyph work is makepad-text-mesh's (`build_text3d`: shaping with any
//! variable axis, counters, bevel profiles, the verified triangulation);
//! this module only deduplicates the letters into shapes, adds the shapes a
//! kit may switch to (`alphabet`), the voxel cells and their cube, and
//! packs every mesh as `geom.CubeVertex` (12 floats):
//!
//! | floats | field | meaning |
//! |---|---|---|
//! | 0..3 | `geom_pos` | position relative to the glyph's rest centre |
//! | 3 | `geom_id` | the shape id |
//! | 4..7 | `geom_normal` | face normal |
//! | 7 | `geom_pad` | face class: 0 front, 1 back, 2 side wall, 3 bevel, 4 cube |
//! | 8..10 | `geom_uv` | uv in the glyph's own ink box, 0..1, y up |
//! | 10, 11 | tail pads | the glyph's ink extent (width, height), for looks that work in world units |
//!
//! Pure CPU work and `Send`: a host builds a set on its task pool when the
//! text changes (a few milliseconds for a line) and uploads the meshes.

use makepad_text_mesh::letters::{build_text3d, BevelProfile, FontSource, Letter, Text3dParams, TextAlign, TextPart};

/// Face classes on `geom_pad`.
pub const FACE_FRONT: f32 = 0.0;
pub const FACE_BACK: f32 = 1.0;
pub const FACE_SIDE: f32 = 2.0;
pub const FACE_BEVEL: f32 = 3.0;
pub const FACE_CUBE: f32 = 4.0;

/// Floats per packed vertex (`geom.CubeVertex`).
pub const VERT_FLOATS: usize = 12;

/// Voxel cells: the letters sampled on a lattice.
#[derive(Clone, Debug, PartialEq)]
pub struct CellSpec {
    /// Cells per cap height (4..40).
    pub res: u32,
    /// Layers through the depth (1..12).
    pub layers: u32,
    /// Fill the whole block, not only the letters: the extra cells are
    /// waste (`ink` 0), for carving.
    pub block: bool,
}

/// What to build.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeSpec {
    pub text: String,
    pub font: FontSource,
    pub weight: Option<f32>,
    pub axes: Vec<(u32, f32)>,
    /// Cap height in world units.
    pub size: f32,
    pub depth: f32,
    pub bevel: f32,
    pub bevel_profile: BevelProfile,
    pub bevel_segments: u32,
    pub detail: f32,
    pub tracking: f32,
    pub line_height: f32,
    pub align: TextAlign,
    /// Wrap width in cap heights.
    pub wrap: Option<f32>,
    /// Glyphs an animator may switch to (`o.shape = alpha0 + k`).
    pub alphabet: String,
    pub cells: Option<CellSpec>,
    /// A word cloud: every distinct word once, sized by how often it
    /// occurs, packed round the centre without overlaps.
    pub cloud: bool,
}

impl Default for ShapeSpec {
    fn default() -> Self {
        Self {
            text: String::new(),
            font: FontSource::Bundled("bold".into()),
            weight: None,
            axes: Vec::new(),
            size: 1.0,
            depth: 0.35,
            bevel: 0.0,
            bevel_profile: BevelProfile::Chamfer,
            bevel_segments: 1,
            detail: 0.012,
            tracking: 0.0,
            line_height: 1.2,
            align: TextAlign::Center,
            wrap: None,
            alphabet: String::new(),
            cells: None,
            cloud: false,
        }
    }
}

/// One mesh, in `geom.CubeVertex` floats.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shape {
    /// The code point it shows (`u32::MAX` for the cube).
    pub key: u32,
    pub verts: Vec<f32>,
    pub indices: Vec<u32>,
    /// Ink extent (width, height, depth).
    pub size: [f32; 3],
}

/// The rest pose of one element.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rest {
    pub pivot: [f32; 3],
    pub size: [f32; 3],
    pub word: usize,
    pub line: usize,
    pub ch: char,
    pub shape: usize,
    /// 1 for a letter (or a letter's cell), 0 for a waste cell.
    pub ink: f32,
    pub layer: usize,
    /// The letter ordinal (a cell's letter; -1 for waste).
    pub glyph: i32,
    /// Index of the char in the text (for karaoke), or usize::MAX.
    pub char_index: usize,
    /// The element's base scale (a cloud word's size; 1 elsewhere).
    pub scale: f32,
    /// Its word's weight 0..1 (a cloud word's share of the heaviest; 1 elsewhere).
    pub weight: f32,
}

/// A built text: its shapes and every element at rest.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlyphSet {
    pub shapes: Vec<Shape>,
    pub elements: Vec<Rest>,
    /// The letters (for cells: the letters the cells came from).
    pub letters: usize,
    pub words: usize,
    pub lines: usize,
    pub word_centers: Vec<[f32; 3]>,
    pub line_centers: Vec<[f32; 3]>,
    pub bounds: ([f32; 3], [f32; 3]),
    /// First alphabet shape and how many.
    pub alpha0: usize,
    pub alphas: usize,
    /// The cube's shape id (cells).
    pub cube: Option<usize>,
    pub warnings: Vec<String>,
}

fn params(spec: &ShapeSpec, text: &str, voxels: Option<(u32, u32)>) -> Text3dParams {
    Text3dParams {
        text: text.to_string(),
        font: spec.font.clone(),
        weight: spec.weight,
        axes: spec.axes.clone(),
        size: spec.size,
        depth: spec.depth,
        bevel: spec.bevel,
        bevel_segments: spec.bevel_segments,
        bevel_profile: spec.bevel_profile,
        detail: spec.detail,
        tracking: spec.tracking,
        line_height: spec.line_height,
        align: spec.align,
        wrap: spec.wrap.map(|w| w * spec.size),
        voxels,
    }
}

fn face_of(part: TextPart) -> f32 {
    match part {
        TextPart::Front => FACE_FRONT,
        TextPart::Back => FACE_BACK,
        TextPart::Side => FACE_SIDE,
        TextPart::Bevel => FACE_BEVEL,
    }
}

/// A letter's faces packed, with its shape id.
fn pack_letter(letter: &Letter, id: usize) -> Shape {
    let (min, max) = letter.mesh.bounds().unwrap_or(([0.0; 3], [0.0; 3]));
    let size = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
    let (w, h) = (size[0].max(1e-6), size[1].max(1e-6));
    let mut s = Shape { key: letter.ch as u32, size, ..Shape::default() };
    for (part, m) in &letter.parts {
        let base = (s.verts.len() / VERT_FLOATS) as u32;
        for (k, p) in m.positions.iter().enumerate() {
            let n = m.normals.get(k).copied().unwrap_or([0.0, 0.0, 1.0]);
            s.verts.extend_from_slice(&[p[0], p[1], p[2], id as f32, n[0], n[1], n[2], face_of(*part), (p[0] - min[0]) / w, (p[1] - min[1]) / h, size[0], size[1]]);
        }
        s.indices.extend(m.indices.iter().map(|i| i + base));
    }
    s
}

/// A cube of half extents `half`, every face its own four vertices.
fn cube_shape(half: [f32; 3], id: usize) -> Shape {
    let mut s = Shape { key: u32::MAX, size: [half[0] * 2.0, half[1] * 2.0, half[2] * 2.0], ..Shape::default() };
    let faces: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
        ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
    ];
    for (n, u, v) in faces {
        let base = (s.verts.len() / VERT_FLOATS) as u32;
        for (a, b) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let p = [0, 1, 2].map(|k| (n[k] + u[k] * a + v[k] * b) * half[k]);
            s.verts.extend_from_slice(&[p[0], p[1], p[2], id as f32, n[0], n[1], n[2], FACE_CUBE, (a + 1.0) * 0.5, (b + 1.0) * 0.5, s.size[0], s.size[1]]);
        }
        s.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    s
}

/// The char index of every inked char, in reading order (the builder's
/// letter order: whitespace is not a letter).
fn ink_chars(text: &str) -> Vec<(usize, char)> {
    text.chars().enumerate().filter(|(_, c)| !c.is_whitespace()).collect()
}

/// The distinct words of `text` in first-seen order with their counts.
fn word_counts(text: &str) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = Vec::new();
    for w in text.split_whitespace() {
        match out.iter_mut().find(|(k, _)| k == w) {
            Some(e) => e.1 += 1,
            None => out.push((w.to_string(), 1)),
        }
    }
    out
}

/// A word cloud: the distinct words set one per line, then each word
/// scaled by its weight and packed on a spiral (heaviest first, nearest the
/// centre) so no two word boxes overlap.
fn build_cloud(spec: &ShapeSpec) -> Result<GlyphSet, String> {
    let words = word_counts(&spec.text);
    let max = words.iter().map(|w| w.1).max().unwrap_or(1).max(1) as f32;
    let lines: Vec<&str> = words.iter().map(|w| w.0.as_str()).collect();
    let mut set = build(&ShapeSpec { text: lines.join("\n"), cloud: false, wrap: None, ..spec.clone() })?;
    let n = words.len();
    // Each word's ink box (from its letters) and its scale.
    let mut boxes = vec![([f32::MAX; 2], [f32::MIN; 2]); n];
    for e in &set.elements {
        let b = &mut boxes[e.line.min(n.saturating_sub(1))];
        b.0[0] = b.0[0].min(e.pivot[0] - e.size[0] * 0.5);
        b.0[1] = b.0[1].min(e.pivot[1] - e.size[1] * 0.5);
        b.1[0] = b.1[0].max(e.pivot[0] + e.size[0] * 0.5);
        b.1[1] = b.1[1].max(e.pivot[1] + e.size[1] * 0.5);
    }
    let weight: Vec<f32> = words.iter().map(|w| w.1 as f32 / max).collect();
    let scale: Vec<f32> = weight.iter().map(|w| 0.55 + 0.95 * w).collect();
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|a, b| weight[*b].partial_cmp(&weight[*a]).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(b)));
    let gap = spec.size * 0.12;
    let mut placed: Vec<([f32; 2], [f32; 2])> = Vec::new();
    let mut centre = vec![[0.0f32; 2]; n];
    for &w in &order {
        let (lo, hi) = boxes[w];
        let half = [(hi[0] - lo[0]).max(0.0) * 0.5 * scale[w] + gap, (hi[1] - lo[1]).max(0.0) * 0.5 * scale[w] + gap];
        let mut k = 0u32;
        loop {
            // An Archimedean spiral, wider than tall (a 16:9 frame).
            let a = k as f32 * 0.35;
            let r = spec.size * 0.12 * a;
            let c = [r * a.cos() * 1.7, r * a.sin()];
            let hit = placed.iter().any(|(pc, ph)| (pc[0] - c[0]).abs() < ph[0] + half[0] && (pc[1] - c[1]).abs() < ph[1] + half[1]);
            if !hit || k > 20000 {
                placed.push((c, half));
                centre[w] = c;
                break;
            }
            k += 1;
        }
    }
    let mut min = [f32::MAX; 3];
    let mut max3 = [f32::MIN; 3];
    let mids: Vec<[f32; 2]> = boxes.iter().map(|(lo, hi)| [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5]).collect();
    for e in &mut set.elements {
        let w = e.line.min(n.saturating_sub(1));
        let s = scale[w];
        e.pivot = [centre[w][0] + (e.pivot[0] - mids[w][0]) * s, centre[w][1] + (e.pivot[1] - mids[w][1]) * s, e.pivot[2] * s];
        e.scale = s;
        e.weight = weight[w];
        e.word = w;
        e.line = 0;
        for k in 0..3 {
            min[k] = min[k].min(e.pivot[k] - e.size[k] * 0.5 * s);
            max3[k] = max3[k].max(e.pivot[k] + e.size[k] * 0.5 * s);
        }
    }
    if !set.elements.is_empty() {
        set.bounds = (min, max3);
    }
    set.word_centers = centre.iter().map(|c| [c[0], c[1], 0.0]).collect();
    set.words = n;
    set.lines = 1;
    set.line_centers = vec![[0.0; 3]];
    Ok(set)
}

/// Build the set for `spec` (the shapes of the text, then the alphabet's,
/// then the cube for cells).
pub fn build(spec: &ShapeSpec) -> Result<GlyphSet, String> {
    if spec.cloud {
        return build_cloud(spec);
    }
    let voxels = spec.cells.as_ref().map(|c| (c.res.clamp(4, 40), c.layers.clamp(1, 12)));
    let t3 = build_text3d(&params(spec, &spec.text, voxels))?;
    let mut set = GlyphSet {
        letters: t3.letters.len(),
        words: t3.words.max(t3.word_centers.len()),
        lines: t3.lines.max(t3.line_centers.len()),
        word_centers: t3.word_centers.clone(),
        line_centers: t3.line_centers.clone(),
        bounds: t3.bounds,
        warnings: t3.warnings.clone(),
        ..GlyphSet::default()
    };
    let chars = ink_chars(&spec.text);
    let mut by_key: Vec<(u32, usize)> = Vec::new();
    let mut letter_shape = Vec::with_capacity(t3.letters.len());
    for l in &t3.letters {
        let key = l.ch as u32;
        let id = match by_key.iter().find(|(k, _)| *k == key) {
            Some((_, id)) => *id,
            None => {
                let id = set.shapes.len();
                set.shapes.push(pack_letter(l, id));
                by_key.push((key, id));
                id
            }
        };
        letter_shape.push(id);
    }
    // The alphabet: its letters' shapes, in the order given (a repeated or
    // ink-less char still takes its slot, drawn as the first one's).
    set.alpha0 = set.shapes.len();
    if !spec.alphabet.is_empty() {
        let a = build_text3d(&params(spec, &spec.alphabet, None))?;
        let ink = ink_chars(&spec.alphabet);
        for (k, (_, ch)) in ink.iter().enumerate() {
            let shape = match a.letters.iter().find(|l| l.index == k) {
                Some(l) => pack_letter(l, set.shapes.len()),
                None => Shape { key: *ch as u32, ..Shape::default() },
            };
            set.shapes.push(shape);
        }
        set.alphas = ink.len();
    }
    match &spec.cells {
        None => {
            for (k, l) in t3.letters.iter().enumerate() {
                let s = &set.shapes[letter_shape[k]];
                set.elements.push(Rest {
                    pivot: l.pivot,
                    size: s.size,
                    word: l.word,
                    line: l.line,
                    ch: l.ch,
                    shape: letter_shape[k],
                    ink: 1.0,
                    layer: 0,
                    glyph: k as i32,
                    char_index: chars.get(l.index).map_or(usize::MAX, |c| c.0),
                    scale: 1.0,
                    weight: 1.0,
                });
            }
        }
        Some(cells) => {
            let half = t3.voxel_half;
            let cube = set.shapes.len();
            set.shapes.push(cube_shape(half, cube));
            set.cube = Some(cube);
            let size = [half[0] * 2.0, half[1] * 2.0, half[2] * 2.0];
            let layers = voxels.map_or(1, |v| v.1) as usize;
            let layer_of = |z: f32| if half[2] > 0.0 { (((layers as f32 - 1.0) - z / half[2]) * 0.5).round().clamp(0.0, layers as f32 - 1.0) as usize } else { 0 };
            let mut taken = std::collections::HashSet::new();
            let pitch = (size[0].max(1e-6), size[1].max(1e-6));
            for v in &t3.voxels {
                let l = t3.letters.iter().find(|l| l.index == v.glyph);
                let layer = layer_of(v.center[2]);
                taken.insert(((v.center[0] / pitch.0).round() as i64, (v.center[1] / pitch.1).round() as i64, layer));
                set.elements.push(Rest {
                    pivot: v.center,
                    size,
                    word: l.map_or(0, |l| l.word),
                    line: l.map_or(0, |l| l.line),
                    ch: l.map_or(' ', |l| l.ch),
                    shape: cube,
                    ink: 1.0,
                    layer,
                    glyph: v.glyph as i32,
                    char_index: chars.get(v.glyph).map_or(usize::MAX, |c| c.0),
                    scale: 1.0,
                    weight: 1.0,
                });
            }
            if cells.block && !t3.voxels.is_empty() {
                // The lattice the letter cells sit on, over the block's
                // bounds with a one-cell margin.
                let (min, max) = t3.bounds;
                let x0 = (min[0] / pitch.0).floor() as i64 - 1;
                let x1 = (max[0] / pitch.0).ceil() as i64 + 1;
                let y0 = (min[1] / pitch.1).floor() as i64 - 1;
                let y1 = (max[1] / pitch.1).ceil() as i64 + 1;
                let phase_x = t3.voxels[0].center[0] - (t3.voxels[0].center[0] / pitch.0).round() * pitch.0;
                let phase_y = t3.voxels[0].center[1] - (t3.voxels[0].center[1] / pitch.1).round() * pitch.1;
                for layer in 0..layers {
                    let z = half[2] * (layers as f32 - 1.0 - 2.0 * layer as f32);
                    for y in y0..=y1 {
                        for x in x0..=x1 {
                            if taken.contains(&(x, y, layer)) {
                                continue;
                            }
                            set.elements.push(Rest {
                                pivot: [x as f32 * pitch.0 + phase_x, y as f32 * pitch.1 + phase_y, z],
                                size,
                                word: 0,
                                line: 0,
                                ch: ' ',
                                shape: cube,
                                ink: 0.0,
                                layer,
                                glyph: -1,
                                char_index: usize::MAX,
                                scale: 1.0,
                                weight: 1.0,
                            });
                        }
                    }
                }
            }
        }
    }
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_share_shapes_and_carry_their_faces() {
        let set = build(&ShapeSpec { text: "ABBA ok".into(), alphabet: "#0".into(), ..ShapeSpec::default() }).unwrap();
        assert_eq!(set.letters, 6);
        assert_eq!(set.elements.len(), 6);
        assert_eq!(set.alpha0, 4, "A B o k, then the alphabet");
        assert_eq!(set.alphas, 2);
        assert_eq!(set.shapes.len(), 6);
        assert_eq!(set.elements[1].shape, set.elements[2].shape, "both Bs draw one mesh");
        assert_eq!((set.words, set.elements[4].word, set.elements[4].char_index), (2, 1, 5));
        let a = &set.shapes[0];
        assert!(!a.indices.is_empty() && a.indices.iter().all(|&i| (i as usize) < a.verts.len() / VERT_FLOATS));
        let faces: std::collections::BTreeSet<i32> = a.verts.chunks(VERT_FLOATS).map(|v| v[7] as i32).collect();
        assert!(faces.contains(&0) && faces.contains(&1) && faces.contains(&2), "{faces:?}");
        // Local: the mesh sits around the rest centre.
        let xs: Vec<f32> = a.verts.chunks(VERT_FLOATS).map(|v| v[0]).collect();
        let (lo, hi) = (xs.iter().cloned().fold(f32::MAX, f32::min), xs.iter().cloned().fold(f32::MIN, f32::max));
        assert!((lo + hi).abs() < 0.05, "centred: {lo} {hi}");
    }

    #[test]
    fn a_cloud_sizes_words_by_count_and_never_overlaps() {
        let set = build(&ShapeSpec { text: "sing sing sing along with me sing along".into(), cloud: true, ..ShapeSpec::default() }).unwrap();
        assert_eq!(set.words, 4);
        let sing = set.elements.iter().find(|e| e.ch == 'S' || e.ch == 's').unwrap();
        let me = set.elements.iter().find(|e| e.ch == 'm').unwrap();
        assert!(sing.weight == 1.0 && me.weight == 0.25 && sing.scale > me.scale);
        // Word boxes are apart.
        let bx = |w: usize| {
            let es: Vec<_> = set.elements.iter().filter(|e| e.word == w).collect();
            let lo = es.iter().fold([f32::MAX; 2], |a, e| [a[0].min(e.pivot[0] - e.size[0] * 0.5 * e.scale), a[1].min(e.pivot[1] - e.size[1] * 0.5 * e.scale)]);
            let hi = es.iter().fold([f32::MIN; 2], |a, e| [a[0].max(e.pivot[0] + e.size[0] * 0.5 * e.scale), a[1].max(e.pivot[1] + e.size[1] * 0.5 * e.scale)]);
            (lo, hi)
        };
        for a in 0..4 {
            for b in a + 1..4 {
                let (p, q) = (bx(a), bx(b));
                let apart = p.1[0] <= q.0[0] + 1e-3 || q.1[0] <= p.0[0] + 1e-3 || p.1[1] <= q.0[1] + 1e-3 || q.1[1] <= p.0[1] + 1e-3;
                assert!(apart, "words {a} and {b} overlap: {p:?} {q:?}");
            }
        }
    }

    #[test]
    fn cells_fill_the_letters_or_the_block() {
        let ink = build(&ShapeSpec { text: "I".into(), cells: Some(CellSpec { res: 8, layers: 2, block: false }), ..ShapeSpec::default() }).unwrap();
        let block = build(&ShapeSpec { text: "I".into(), cells: Some(CellSpec { res: 8, layers: 2, block: true }), ..ShapeSpec::default() }).unwrap();
        assert!(!ink.elements.is_empty() && ink.elements.iter().all(|e| e.ink == 1.0 && Some(e.shape) == ink.cube));
        assert!(ink.elements.iter().any(|e| e.layer == 1));
        let waste = block.elements.iter().filter(|e| e.ink == 0.0).count();
        assert!(waste > 0 && block.elements.len() == ink.elements.len() + waste);
        let cube = &block.shapes[block.cube.unwrap()];
        assert_eq!(cube.verts.len() / VERT_FLOATS, 24);
    }
}
