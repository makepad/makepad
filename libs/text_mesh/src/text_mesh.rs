//! Text mesh — shaped text turned into extruded glyph solids on the shared
//! fx vertex stream.
//!
//! The text engine (`engines_text.rs`) moves LETTERS, not vertices: every
//! stock mode and both vertex hooks receive one glyph as a unit and place it
//! somewhere. That is only possible if the mesh already knows which vertex
//! belongs to which letter and where that letter rests — so this module
//! bakes the whole reading into the stream once (the slow path, a few
//! milliseconds when the picked text changes) and the shader does the rest
//! every frame off `time_beat`. Nothing here touches a `Cx`: a laid-out
//! text (`DrawText::layout` / the `Layouter`) goes in, an `FxMesh` comes
//! out, and the unit tests run the real bundled fonts without a window.
//!
//! # The pipeline, per glyph
//!
//! 1. `Font::glyph_outline_rc` gives the outline in font units; glyph id 0
//!    (missing) and ink-less glyphs (space) are skipped — they still count
//!    as word separators, never as glyph ordinals.
//! 2. Quadratic and cubic curves are flattened to segments at `detail` em
//!    (default 0.012, clamped 0.004..0.05), the segment count taken from the
//!    control polygon length so a long sweep gets more pieces than a serif.
//! 3. Contours that cross each other or themselves (a `%` whose slash runs
//!    through its rings, a font built from overlapping strokes) are split
//!    at every intersection so no two constraint edges cross; then
//!    consecutive points closer than 1e-4 em are welded and contours with
//!    fewer than three points or under 1e-6 em² are dropped.
//! 4. Caps: ONE constrained Delaunay triangulation over every contour point
//!    of the glyph with every contour edge as a constraint, then the
//!    triangles whose centroid has NONZERO winding against the contours are
//!    kept — the TrueType/CFF fill rule, so `O`, `B`, `8`, `g`, `@`, `&`,
//!    `%` keep their counters whichever way the font winds them. Guard: the
//!    result is verified — every constraint edge must come back. If one is
//!    missing (the vendored flip loop can stall on a long edge crossing a
//!    dense fan) the CDT is retried with the constraints reversed, then the
//!    failing edges are refined at their midpoints (points ON the contour,
//!    so nothing moves) up to three rounds; only when that fails, or no
//!    triangle survives, is the largest contour ear-clipped alone and the
//!    glyph counted in `TextMeshReport::fallbacks`. No glyph can panic the
//!    build.
//! 5. Extrusion: front cap at `+depth/2` (normal +z), back cap at `-depth/2`
//!    with reversed winding, one flat quad per contour edge with the 2D
//!    outward normal — "outward" is decided per EDGE by probing the winding
//!    on both sides of its midpoint, so a hole's walls face INTO the hole
//!    whatever the font's orientation convention, and an edge with ink on
//!    both sides (buried in an overlap) gets no wall. `bevel > 0` (clamped to
//!    0.4·depth and 0.3·size) insets the caps along per-vertex mitred
//!    normals and joins ring and outline with a bevel band shaped by
//!    `bevel_type` (below). `depth: 0` emits the front cap only.
//! 6. The solid is pushed in the glyph's LOCAL frame: origin at the glyph's
//!    rest centre (the centre of its ink box at its laid-out position),
//!    axes turned only where the layout bends the line (ring, tunnel,
//!    helix) or lays it down (grid).
//!
//! Steps 1-4 (the glyph SHAPE: contours, caps, wall sides, mitres) depend on
//! the font, the glyph id and `detail` only, and they are nearly all of a
//! build's cost. The shape store (`ShapeStore`) keeps them on the building
//! thread across builds, so a rebuild whose glyphs were seen before —
//! a scramble step, a word-by-word reveal, a new lyric line in the same
//! face, a budget pass at an earlier `detail` — pays only for placing and
//! emitting (steps 5-6). A stored shape is the one a cold build makes (the
//! build is deterministic), so a store hit never changes a float.
//!
//! The FIRST sight of a glyph still costs its shape (0.2-0.8 ms, nearly all
//! of it the constrained triangulation). `ShapeWarmup` is the queue a host
//! spreads that over frames with: it queues a text's glyphs that are not
//! stored yet (never one twice), and `drain` builds them into the store in
//! queue order under a time budget, at least one per call. A warmed shape is
//! the shape a build would have made, so warming changes no float either.
//!
//! # The attribute table (the contract shader authors read)
//!
//! | channel    | meaning                                                          |
//! |------------|------------------------------------------------------------------|
//! | `geom_pos` | vertex in the glyph's local frame, world units, origin = rest centre |
//! | `a_id`     | glyph ordinal 0..N-1 in reading order (ink glyphs only); -1 on the floor |
//! | `normal`   | unit face normal in the local frame; VOXEL cubes: the morph's FROM rest centre |
//! | `a_aux`    | face class, table below                                           |
//! | `uv`       | the glyph's REST CENTRE in text space — the pivot the shader places it at |
//! | `a_r0`     | word ordinal 0..W-1 (a word boundary is whitespace in the row text), + 0.5 when the glyph changed |
//! | `a_r1`     | line ordinal 0..L-1 (grid: row; voxel: layer; cloud: the word's weight 0..1), + 1000·copy |
//!
//! Face classes on `a_aux`: 0 front cap, 1 back cap, 2 side wall, 3 bevel,
//! 4 letter voxel, 5 waste voxel, 6 floor, 7 mirrored copy (7 + the
//! original class / 16), 8 dying voxel, 9 born voxel (8 and 9 carry + 0.5
//! when the cube is a waste cube), 10 the backdrop quad. Decode: `class = floor(a_aux)`, for 7
//! `original = fract(a_aux) * 16`, for 8/9 `waste = fract(a_aux) > 0.25`.
//! `a_r0`: `word = floor(a_r0)`, `changed = fract(a_r0) > 0.25`. `a_r1`:
//! `copy = floor(a_r1 / 1000)`, `line = a_r1 - 1000·copy` (exact in f32).
//!
//! Text space is y-UP (the layouter's y-down is flipped exactly once), the
//! block is centred on the origin, and the first row's cap height maps to
//! `params.size` world units. For `line` / `block` / `cloud` the pivot is
//! literally the rest centre, `world = uv + geom_pos`. The bent layouts
//! reuse `uv` as (arc position, height) and the shader rebuilds the pivot:
//!
//! - `ring` — a vertical cylinder of radius R (`report.ring_radius`) around
//!   Y, θ = uv.x / R, pivot = (R·sin θ, uv.y, R·cos θ); the local frame is
//!   already turned by θ about Y so the cap normal is the cylinder normal
//!   and local +x is the tangent. `ring_close` stretches the arc pitch of
//!   the first row so it meets itself with a 5 % gap; R defaults to
//!   `row width / τ · 1.05` for the same reason.
//! - `tunnel` — every WORD on its own ring around the -Z axis, ring w at
//!   z = −w·`tunnel_pitch`, the word centred on the angle w·golden
//!   (2.39996 rad), φ = w·golden − uv.x / R, uv.y = the glyph's height
//!   above its row baseline, pivot = ((R + uv.y)·cos φ, (R + uv.y)·sin φ,
//!   −w·pitch). The local frame is turned by φ − π/2 about Z, so the
//!   letters stand on the ring facing +z (the `fly` camera comes down −z)
//!   with their tops pointing outward. R is `ring_radius` or the longest
//!   word's width / τ · 1.05, never under 2·size so the camera fits through.
//! - `helix` (index 5) — every row, end to end with half a cap of space
//!   between rows, wound on a helix of radius R (`report.ring_radius`;
//!   `ring_radius` or max(1.5·size, length / 3τ), about three turns) about
//!   +Y, descending `helix_pitch` (`report.helix_pitch`, auto 1.6·size)
//!   per turn as the text reads. Exactly the ring's pivot: θ = uv.x / R,
//!   pivot = (R·sin θ, uv.y, R·cos θ), uv.y already carries the descent.
//!   The local frame is turned by θ about Y and then tilted about its own z
//!   by atan(−pitch / τR), so local +x runs along the coil.
//! - `grid` (index 6) — the glyphs in reading order on a `grid_cols` ×
//!   rows lattice (`grid_cols` 0 = ⌈√N⌉) at `grid_pitch` (0 = 1.5·size),
//!   centred on the origin, lying in the XZ plane facing +Y: uv = the cell
//!   centre (x, z), pivot = (uv.x, 0, uv.y), row 0 farthest (−z) so a low
//!   camera at +z reads the field like a page on the floor; `a_r1` = row.
//!   Local frame: letter x → +X, letter up → −Z, cap normal → +Y.
//!   `report.width`/`height` are the field's x/z extents.
//! - `voxel` (index 7) — see "Voxel letters".
//!
//! # Copies (`copies`, 1..32; `copies_turn`)
//!
//! The whole laid-out text is pushed `n` times, copy c after copy c−1, the
//! copy index in `a_r1` (`+ 1000·c`). With `copies_turn: false` every copy
//! is an exact duplicate AT THE SAME POSE — nothing baked, `copy_radius`
//! and `copy_pitch` 0, and the shader turns nothing — so the hooks can
//! place copies as props without solving a turn backwards. Otherwise
//! (the default), where `uv` can carry the turned pivot
//! the builder BAKES the turn (uv and the local frame of copy c already
//! include it, the shader does nothing extra):
//! - ring, helix: turned τ·c/n about Y (uv.x shifted by τ·c/n·R);
//! - tunnel: turned τ·c/n about the tunnel axis (uv.x shifted by −τ·c/n·R);
//! - grid: turned τ·c/n about Y (uv = the turned cell centre, frame turned).
//!
//! The others are exact duplicates of copy 0 and the SHADER applies the
//! copy transform C_c to the finished position and normal of the glyph
//! (after the mode and the hooks), with `turn = τ·c/n`:
//! - line: the paddle wheel — p' = R_x(turn)·(p + (0, 0, r)), r =
//!   `report.copy_radius` (the apothem of an n-gon of line-high faces, never
//!   less than 0.6·depth + 0.05·size);
//! - cloud: the carousel — p' = R_y(turn)·(p + (0, 0, r)), r the same from
//!   the cloud's width;
//! - block and voxel: layers — p' = p − (0, 0, c·`report.copy_pitch`)
//!   (`tunnel_pitch`).
//! With n = 1 nothing changes anywhere (r = 0).
//!
//! # Changed glyphs (`previous_glyphs`, `changed`)
//!
//! Per glyph ordinal: a flagged glyph carries `word + 0.5` on `a_r0`
//! (voxel cubes: the flag of their glyph), so a split-flap clock flips only
//! the digits that changed. The flags are counted over the SHAPED glyphs
//! the mesh actually carries — the same ordinals as `a_id` — never over
//! characters: a character the font lacks, a combining accent that shapes
//! into its letter or a ligature would otherwise shift every later flag.
//! With `previous_glyphs` (the last build's `report.glyph_keys`) a glyph is
//! changed when its key (font and glyph id) differs from the key at its
//! ordinal there, or there was none; without it, `changed` is taken as
//! given (empty = nothing changed). `report.changed` is what was stamped.
//!
//! # The backdrop (`prepend_backdrop`)
//!
//! A full-frame quad for the text family's content backdrop, put FIRST in
//! the stream (so it is drawn before everything, the floor's blend
//! included): four clip-space corners (±1, ±1, 0) in `geom_pos`, class 10
//! (`FACE_BACKDROP`), `a_id` −2, `uv` the screen uv (0,0 top left); the
//! shader passes it through at far depth and paints it with `fx_backdrop`.
//!
//! # Bevel profiles (`bevel_type`, `bevel_rings`)
//!
//! The band between the outline (at z = ±(depth/2 − bevel)) and the inset
//! cap (inset `bevel` along the mitred normals, at z = ±depth/2) is a
//! profile of rings (u = inset fraction, v = rise fraction, both 0 → 1):
//! `chamfer` straight, `bevel_rings` segments (one ring is the classic
//! 45° chamfer, bit for bit); `round` a convex quarter circle (leaves the
//! wall vertically, meets the cap flat); `cove` a concave quarter circle
//! (leaves the wall flat, meets the cap vertically); `step` max(2,
//! `bevel_rings`) terraces, each a riser and a flat tread; `ogee` an S — a
//! cove over the outer half, a round over the inner half (segments rounded
//! up to an even count, at least 2). The back band is the mirror image.
//! Every band vertex's normal is `(n2d · a, ±b)` where (a, b) is the unit
//! normal of the profile at that ring — the analytic one for round, cove
//! and ogee (smooth shading across rings), the segment's own for chamfer
//! and step (flat facets). `report.bevel_rings` is the segment count per
//! side.
//!
//! # The floor (`floor: Some(FloorParams)`)
//!
//! After the letters (all copies) the stream carries, in this index order:
//! 1. a MIRRORED copy of every letter vertex: class 7 + original/16, every
//!    other channel identical to its letter, triangles rewound. The SHADER
//!    reflects it: build the letter's world position and normal exactly as
//!    for the letter (mode, hooks, copy transform), then
//!    `wpos.y = 2·floor_y − wpos.y`, `n.y = −n.y` — so every mode moves the
//!    reflection with its letter, its dy negated, and the rewound triangles
//!    face the right way after the reflection.
//! 2. the floor plane: `grid` × `grid` cells (default 48) of edge
//!    `floor_size` centred on the origin at y = `floor_y`; class 6, `a_id`
//!    = −1, normal +Y, `uv` = the vertex's rest position (x, z) on the plane,
//!    `geom_pos` = (x, floor_y, z) (pivot = origin), `a_r0` = `a_r1` = 0.
//! `report.floor_y` defaults to the lowest descender line of the text
//! (minus 0.05·size; below the paddle wheel for line copies, the bottom of
//! the block for voxels, under the rings for the tunnel, below the laid-down
//! letters for the grid); `report.floor_size` to 3 × the text's footprint
//! (width; 2R for ring/helix; the field for grid). The floor is last in the
//! index buffer, so with alpha blending its depth never hides a letter.
//!
//! # Voxel letters (`layout: Voxel`)
//!
//! The text block (the ink box of the whole text, centred) is cut into a
//! lattice of square cells of `size / voxel_res` (`voxel_res` 4..40 cells
//! per cap height) and `voxel_layers` (1..12) layers spanning `depth` (depth
//! 0 with several layers: layers one cell thick). A cell whose CENTRE has
//! nonzero winding in a glyph's cap is a LETTER cell of that glyph (class
//! 4); every other cell of the block is WASTE (class 5) and takes the
//! nearest glyph (distance to its ink box). One cube per cell and layer:
//! 24 vertices (4 per face), `geom_pos` relative to the cube's centre
//! (half extents `report.voxel_half`), `uv` = the cube's rest centre (x, y),
//! `a_r1` = layer (+ 1000·copy), `a_r0` = the glyph's word (+ 0.5 changed),
//! `a_id` = the glyph ordinal. The rest centre's z follows from the layer:
//! `z = voxel_half.z · (voxel_layers − 1 − 2·layer)` (layer 0 in front).
//! Letter cubes come first in the stream (row by row from the top, left to
//! right, layers innermost), then the waste cubes in the same order.
//! `voxel_waste: false` emits NO waste cubes: the stream, the reported set
//! and the morph are the letter cubes alone (the lattice and the block's
//! extents are unchanged, the empty cells are simply not built).
//! `voxel_layers: 1` with `depth: 0` emits FLAT voxels: one quad per cell
//! (4 vertices, z = 0, face normal +z). Budget: `voxel_res` coarsens ×0.75
//! until the cubes fit (a warning per step), then cubes are truncated.
//!
//! Face normals of a cube are NOT in the stream: the shader derives them
//! from the local position — `k = argmax_i |geom_pos_i| / voxel_half_i`,
//! `n = sign(geom_pos_k)·e_k` (flat voxels: +z). Every face's four corners
//! are pulled in by `VOXEL_FACE_INSET` (0.1 %) along the face, so the rule
//! is exact at every VERTEX (a corner is otherwise shared by three faces)
//! as well as per pixel on the interpolated local position. That frees the
//! `normal` channel for the morph.
//!
//! # Voxel morph (`previous_voxels`)
//!
//! `report.voxels` is the build's cubes at rest (copy 0, emission order);
//! handed back as `previous_voxels` on the next build, the cubes of the old
//! text travel to the new one. For every cube the `normal` channel carries
//! its FROM rest centre (x, y, z) and `uv` + layer its TO rest centre; with
//! no previous set FROM = TO. Pairing: letter cubes with letter cubes and
//! waste with waste (a class the old set lacks borrows the whole old set),
//! each side sorted by (x normalised over its set's x range, y, layer) and
//! paired rank to rank. Without waste the old set's waste cubes (a set
//! built with waste) are ignored, so letters pair with letters only.
//! Extra new cubes are BORN (class 9): FROM = the old
//! cube at the same rank wrapped over the old count; the shader grows them
//! from scale 0. Surplus old cubes are DYING (class 8), appended after the
//! new cubes: FROM = their old centre, TO = the nearest new cube of their
//! class (its uv, layer, glyph and word), shrinking to 0; with no new cube
//! at all they shrink in place. Every cube is emitted at the NEW build's
//! size. The shader eases `m` and places the cube at `mix(FROM, TO, m)`
//! (born: scale m; dying: 1 − m), then runs the mode on that rest centre.
//! Cubes per copy = `report.voxel_cubes` = Σ per class max(old, new).
//!
//! # Laws
//!
//! - Deterministic: the same input, params and seed give the same floats;
//!   the cloud's spiral start angles come from `FxRng(seed)` alone, the
//!   morph's ranks break ties by emission index.
//! - Never a panic, never a non-finite float: params are sanitised, every
//!   glyph path is guarded, the CDT is verified before it is trusted.
//! - The budget (`vert_budget`) counts every copy, mirror and floor vertex
//!   and is enforced in a FIXED order, every step reported in `warnings`,
//!   never silent: the floor grid shrinks to a quarter of the budget, then
//!   coarsen `detail` ×1.5, again, drop the bevel, truncate glyphs (voxels:
//!   coarsen `voxel_res` ×0.75 per step, then truncate cubes).
//! - The vertex layout is the frozen 12-float `FxMesh` stream; this module
//!   only gives its channels the meanings above.

use makepad_geom3d::fx_mesh::{FxMesh, FxRng, VERT_FLOATS};
use makepad_csg_boolean::cdt::{Point2, CDT};
use makepad_draw::text::glyph_outline::Command;
use makepad_draw::text::layouter::{LaidoutGlyph, LaidoutText};
use makepad_draw::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;

/// How the laid-out text is arranged in 3D. [`TextLayout::index`] is the
/// number the shader switches on — a contract, never renumbered.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TextLayout {
    /// The rows as laid out, block centred on the origin.
    Line,
    /// Same, but the caller laid the text out wrapped (`wrap`).
    Block,
    /// The first row bent around a vertical cylinder (Y axis).
    Ring,
    /// Each word on its own ring around -Z, at `tunnel_pitch` per word.
    Tunnel,
    /// A word cloud: one laid-out word per entry, sized by weight.
    Cloud,
    /// Every row, end to end, wound on a helix about +Y: radius
    /// `ring_radius`, `helix_pitch` world units per turn (the coil spring).
    Helix,
    /// The glyphs on a `grid_cols` x rows lattice at `grid_pitch` in reading
    /// order, lying in the XZ plane facing +Y (the letter field).
    Grid,
    /// Every glyph's cap sampled into cubes (`voxel_res`, `voxel_layers`)
    /// inside a block of waste cubes (module docs, "Voxel letters").
    Voxel,
}

impl Default for TextLayout {
    fn default() -> Self {
        Self::Line
    }
}

impl TextLayout {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "line" => Some(Self::Line),
            "block" => Some(Self::Block),
            "ring" => Some(Self::Ring),
            "tunnel" => Some(Self::Tunnel),
            "cloud" => Some(Self::Cloud),
            "helix" => Some(Self::Helix),
            "grid" => Some(Self::Grid),
            "voxel" => Some(Self::Voxel),
            _ => None,
        }
    }
    pub fn index(self) -> f32 {
        match self {
            Self::Line => 0.0,
            Self::Block => 1.0,
            Self::Ring => 2.0,
            Self::Tunnel => 3.0,
            Self::Cloud => 4.0,
            Self::Helix => 5.0,
            Self::Grid => 6.0,
            Self::Voxel => 7.0,
        }
    }
}

/// The cross section of the bevel band between the outline and the inset
/// cap, mirrored on the back (module docs, "Bevel profiles").
/// [`BevelType::index`] is the shader's number (`text_d.w`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum BevelType {
    /// Straight 45° cut; `bevel_rings` subdivide it (one ring = the classic chamfer).
    Chamfer,
    /// Convex quarter circle: leaves the wall vertically, meets the cap flat.
    Round,
    /// Concave quarter circle: leaves the wall flat, meets the cap vertically.
    Cove,
    /// Flat terraces: max(2, `bevel_rings`) steps, each a riser and a tread.
    Step,
    /// An S: a cove over the outer half, a round over the inner half.
    Ogee,
}

impl Default for BevelType {
    fn default() -> Self {
        Self::Chamfer
    }
}

impl BevelType {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "chamfer" => Some(Self::Chamfer),
            "round" => Some(Self::Round),
            "cove" => Some(Self::Cove),
            "step" => Some(Self::Step),
            "ogee" => Some(Self::Ogee),
            _ => None,
        }
    }
    pub fn index(self) -> f32 {
        match self {
            Self::Chamfer => 0.0,
            Self::Round => 1.0,
            Self::Cove => 2.0,
            Self::Step => 3.0,
            Self::Ogee => 4.0,
        }
    }
}

/// The semi-reflective floor (module docs, "The floor").
#[derive(Clone, PartialEq, Debug)]
pub struct FloorParams {
    /// Floor height in text space; None = the text's lowest descender line
    /// minus 0.05·size.
    pub y: Option<f32>,
    /// Edge length of the square plane; None = 3 × the text's footprint.
    pub size: Option<f32>,
    /// Cells per side of the subdivided plane (1..256).
    pub grid: usize,
}

impl Default for FloorParams {
    fn default() -> Self {
        Self { y: None, size: None, grid: 48 }
    }
}

/// One build's voxel cubes at rest (copy 0, emission order, dying cubes
/// excluded): what the next build morphs FROM (`previous_voxels`).
#[derive(Clone, PartialEq, Debug, Default)]
pub struct VoxelSet {
    /// Rest centre (x, y, z) in text space.
    pub centres: Vec<[f32; 3]>,
    /// Half extents (x, y, z) of the cube as emitted.
    pub scales: Vec<[f32; 3]>,
    /// true = a letter cube, false = a waste cube.
    pub letter: Vec<bool>,
    /// Layer index, 0 = the front layer.
    pub layers: Vec<u32>,
}

impl VoxelSet {
    pub fn len(&self) -> usize {
        self.centres.len()
    }
    pub fn is_empty(&self) -> bool {
        self.centres.is_empty()
    }
    /// Every column the same length and every centre finite — a set that
    /// fails this is ignored rather than trusted.
    fn is_sound(&self) -> bool {
        let n = self.centres.len();
        self.scales.len() == n
            && self.letter.len() == n
            && self.layers.len() == n
            && self.centres.iter().all(|c| c.iter().all(|f| f.is_finite()))
    }
    /// The letter cubes alone, in order (a sound set only).
    fn letters_only(&self) -> VoxelSet {
        let keep: Vec<usize> = (0..self.len()).filter(|&i| self.letter[i]).collect();
        VoxelSet {
            centres: keep.iter().map(|&i| self.centres[i]).collect(),
            scales: keep.iter().map(|&i| self.scales[i]).collect(),
            letter: vec![true; keep.len()],
            layers: keep.iter().map(|&i| self.layers[i]).collect(),
        }
    }
}

/// Geometry parameters, world units unless stated.
#[derive(Clone, PartialEq, Debug)]
pub struct TextMeshParams {
    /// Cap height of the text in world units (the row's cap height maps to this).
    pub size: f32,
    /// Extrusion depth; 0 = front caps only.
    pub depth: f32,
    /// Chamfer width; 0 = none. Clamped to 0.4*depth and 0.3*size.
    pub bevel: f32,
    /// Curve flattening tolerance in em (0.004..0.05).
    pub detail: f32,
    pub layout: TextLayout,
    /// Ring / helix radius; None = auto (ring: row width / tau * 1.05;
    /// helix: max(1.5*size, length / 3tau)).
    pub ring_radius: Option<f32>,
    /// Ring: stretch the row so it meets itself.
    pub ring_close: bool,
    /// Tunnel: distance between word rings along -Z; block/voxel copies:
    /// the step between layers along -Z.
    pub tunnel_pitch: f32,
    /// Cloud: at most this many words are placed.
    pub words: usize,
    pub seed: u64,
    /// Vertex budget; over it the builder degrades (see the report).
    pub vert_budget: usize,
    /// Helix: world units the coil descends per turn; <= 0 = auto (1.6*size).
    pub helix_pitch: f32,
    /// Grid: columns of the lattice; 0 = auto (ceil(sqrt(glyphs))).
    pub grid_cols: usize,
    /// Grid: distance between cell centres; <= 0 = auto (1.5*size).
    pub grid_pitch: f32,
    /// Instances of the whole text (1..32), module docs "Copies".
    pub copies: usize,
    /// Copies turn apart (the paddle wheel, the carousel, the layers, the
    /// baked ring turns); false = every copy at the same pose.
    pub copies_turn: bool,
    /// Per glyph ordinal: the glyph differs from the previous text (a_r0 +
    /// 0.5) — used when `previous_glyphs` is None.
    pub changed: Vec<bool>,
    /// The previous build's `report.glyph_keys`: when set, the changed
    /// flags are computed against it (module docs, "Changed glyphs").
    pub previous_glyphs: Option<Vec<u64>>,
    /// Profile of the bevel band.
    pub bevel_type: BevelType,
    /// Rings of the bevel band (1..8).
    pub bevel_rings: usize,
    /// The reflective floor; None = no floor.
    pub floor: Option<FloorParams>,
    /// Voxel: lattice cells per cap height (4..40).
    pub voxel_res: usize,
    /// Voxel: layers spanning `depth` (1..12).
    pub voxel_layers: usize,
    /// Voxel: the previous build's `report.voxels`, to morph from.
    pub previous_voxels: Option<VoxelSet>,
    /// Voxel: fill the rest of the block with waste cubes (class 5); false
    /// = letter cubes only (module docs, "Voxel letters").
    pub voxel_waste: bool,
    /// Extra letter spacing in em of each glyph's font size: glyph k of a
    /// row moves right by `k * tracking` em (every laid-out glyph counts,
    /// spaces included), so each advance widens by `tracking`. 0 = the
    /// layouter's own spacing, bit for bit. Clamped -0.5..2.
    pub tracking: f32,
}

impl Default for TextMeshParams {
    fn default() -> Self {
        Self {
            size: 1.0,
            depth: 0.35,
            bevel: 0.0,
            detail: 0.012,
            layout: TextLayout::Line,
            ring_radius: None,
            ring_close: false,
            tunnel_pitch: 2.5,
            words: 40,
            seed: 1,
            vert_budget: 200_000,
            helix_pitch: 0.0,
            grid_cols: 0,
            grid_pitch: 0.0,
            copies: 1,
            copies_turn: true,
            changed: Vec::new(),
            previous_glyphs: None,
            bevel_type: BevelType::Chamfer,
            bevel_rings: 1,
            floor: None,
            voxel_res: 12,
            voxel_layers: 2,
            previous_voxels: None,
            voxel_waste: true,
            tracking: 0.0,
        }
    }
}

/// What a build produced — the engine turns this into uniforms and the
/// document warnings.
#[derive(Clone, Default, Debug)]
pub struct TextMeshReport {
    pub glyphs: usize,
    pub words: usize,
    pub lines: usize,
    /// Extents of the text block in text space (world units). Grid: the
    /// field's x and z extents; voxel: the lattice block's.
    pub width: f32,
    pub height: f32,
    pub vertices: usize,
    pub triangles: usize,
    /// The ring radius used (Ring, Tunnel and Helix layouts), else 0.
    pub ring_radius: f32,
    /// Glyphs whose caps came from the ear-clip fallback.
    pub fallbacks: usize,
    /// Budget degradation steps and anything else the author should know.
    pub warnings: Vec<String>,
    /// Copies of the whole text in the stream (1..32).
    pub copies: usize,
    /// Line / cloud copies: the offset from the turning axis the shader
    /// applies (module docs, "Copies"); else 0.
    pub copy_radius: f32,
    /// Block / voxel copies: the step along -Z per copy the shader applies; else 0.
    pub copy_pitch: f32,
    /// Helix: world units per turn used; else 0.
    pub helix_pitch: f32,
    /// Grid: the lattice used; else 0.
    pub grid_cols: usize,
    pub grid_rows: usize,
    pub grid_pitch: f32,
    /// Bevel band segments per side actually emitted (0 = no bevel).
    pub bevel_rings: usize,
    /// The floor plane's height, edge length and cells per side (0 without a floor).
    pub floor_y: f32,
    pub floor_size: f32,
    pub floor_grid: usize,
    /// Voxel: cells per cap height after the budget, layers, and the
    /// emitted cube's half extents (x, y, z; z = 0 for flat voxels).
    pub voxel_res: f32,
    pub voxel_layers: usize,
    pub voxel_half: [f32; 3],
    /// Voxel: cubes per copy, born and dying included.
    pub voxel_cubes: usize,
    /// Voxel morph: cubes born (class 9) and dying (class 8) per copy.
    pub morph_born: usize,
    pub morph_dying: usize,
    /// Voxel: this build's cubes at rest — hand them back as
    /// `previous_voxels` so the next text morphs out of this one.
    pub voxels: Option<VoxelSet>,
    /// Per emitted glyph ordinal: its key (font and glyph id) — hand them
    /// back as `previous_glyphs` so the next build flags what changed.
    pub glyph_keys: Vec<u64>,
    /// Per emitted glyph ordinal: the changed flag this build stamped.
    pub changed: Vec<bool>,
}

/// One laid-out word of a cloud with its weight 0..1 (1 = the heaviest).
pub struct CloudWord {
    pub text: Rc<LaidoutText>,
    pub weight: f32,
}

/// Face classes on `a_aux`.
pub const FACE_FRONT: f32 = 0.0;
pub const FACE_BACK: f32 = 1.0;
pub const FACE_SIDE: f32 = 2.0;
pub const FACE_BEVEL: f32 = 3.0;
pub const FACE_VOXEL_LETTER: f32 = 4.0;
pub const FACE_VOXEL_WASTE: f32 = 5.0;
pub const FACE_FLOOR: f32 = 6.0;
/// A mirrored vertex: `FACE_MIRROR + original class / MIRROR_CLASS_SCALE`.
pub const FACE_MIRROR: f32 = 7.0;
pub const FACE_VOXEL_DYING: f32 = 8.0;
pub const FACE_VOXEL_BORN: f32 = 9.0;
/// Added to a dying / born class when the cube is a waste cube.
pub const FACE_WASTE_FLAG: f32 = 0.5;
/// Divides the original class carried in a mirrored vertex's fraction.
pub const MIRROR_CLASS_SCALE: f32 = 16.0;
/// `a_r1 = line + COPY_STRIDE * copy`.
pub const COPY_STRIDE: f32 = 1000.0;
/// The most copies a build pushes.
pub const MAX_COPIES: usize = 32;
/// `a_r0 = word + CHANGED_FLAG` for a changed glyph.
pub const CHANGED_FLAG: f32 = 0.5;
/// The floor's `a_id`.
pub const FLOOR_ID: f32 = -1.0;
/// The backdrop quad's face class and `a_id` (module docs, "The backdrop").
pub const FACE_BACKDROP: f32 = 10.0;
pub const BACKDROP_ID: f32 = -2.0;
/// The fraction every cube face's corners are pulled in along the face, so
/// the largest normalised local component names the face at each vertex.
pub const VOXEL_FACE_INSET: f32 = 1e-3;

/// The angle between consecutive word rings of the tunnel, radians.
pub const TUNNEL_GOLDEN_ANGLE: f32 = 2.399_963;

const TAU: f32 = std::f32::consts::TAU;
/// Consecutive contour points closer than this (em) are one point.
const WELD_EM: f32 = 1e-4;
/// Contours under this area (em²) are noise.
const MIN_AREA_EM2: f32 = 1e-6;
/// How far beside an edge the ink probe looks (em).
const PROBE_EM: f32 = 1e-3;
/// Flattening tolerance clamp, em.
const DETAIL_MIN: f32 = 0.004;
const DETAIL_MAX: f32 = 0.05;
/// Mitre length cap at sharp corners (1 / cos of the half angle).
const MITRE_MAX: f32 = 1.0 / 0.35;

/// Build the mesh for one laid-out text (every layout but Cloud). The text
/// was laid out at a reference size; `size` rescales it so the first row's
/// cap height equals `params.size` world units.
pub fn build_text_mesh(
    laidout: &LaidoutText,
    params: &TextMeshParams,
    mesh: &mut FxMesh,
) -> TextMeshReport {
    let sane = Sane::new(params);
    let world_per_lpx = sane.size / cap_height_lpx(laidout);
    if sane.layout == TextLayout::Voxel {
        return build_voxel_mesh(laidout, &sane, world_per_lpx, params.previous_voxels.as_ref(), mesh);
    }
    build_scene(&sane, mesh, |detail, warnings| {
        let mut cache = ShapeCache::default();
        let mut words = 0usize;
        let mut placed = collect_glyphs(laidout, world_per_lpx, detail, sane.tracking, &mut cache, &mut words);
        let mut scene = Scene {
            words,
            lines: laidout.rows.len(),
            layout: sane.layout,
            ..Scene::default()
        };
        centre_block(&mut placed, &mut scene);
        match sane.layout {
            TextLayout::Line | TextLayout::Block | TextLayout::Cloud | TextLayout::Voxel => {
                for g in placed.iter_mut() {
                    g.uv = g.rest;
                }
            }
            TextLayout::Ring => arrange_ring(&mut placed, &sane, laidout, world_per_lpx, &mut scene, warnings),
            TextLayout::Tunnel => arrange_tunnel(&mut placed, &sane, &mut scene),
            TextLayout::Helix => arrange_helix(&mut placed, &sane, &mut scene),
            TextLayout::Grid => arrange_grid(&mut placed, &sane, &mut scene),
        }
        scene.glyphs = placed;
        scene
    })
}

/// Build a word cloud from separately laid-out words (layout Cloud).
pub fn build_cloud_mesh(
    words: &[CloudWord],
    params: &TextMeshParams,
    mesh: &mut FxMesh,
) -> TextMeshReport {
    let sane = Sane::new(params);
    // Heaviest first (stable), capped at `words`: the big ones take the
    // centre of the spiral and the placement order IS the word ordinal.
    let mut order: Vec<usize> = (0..words.len()).collect();
    order.sort_by(|&a, &b| sane_weight(words[b].weight).total_cmp(&sane_weight(words[a].weight)));
    order.truncate(sane.words);
    build_scene(&sane, mesh, |detail, warnings| {
        let mut cache = ShapeCache::default();
        let mut rng = FxRng::new(sane.seed);
        let pad = 0.10 * sane.size;
        let mut rects: Vec<(Vec2f, Vec2f)> = Vec::new();
        let mut placed: Vec<Placed> = Vec::new();
        let mut placed_words = 0usize;
        let mut skipped = 0usize;
        for &wi in &order {
            let word = &words[wi];
            let weight = sane_weight(word.weight);
            let wsize = sane.size * (0.35 + 0.65 * weight.sqrt());
            let world_per_lpx = wsize / cap_height_lpx(&word.text);
            let mut wcount = 0usize;
            let mut glyphs = collect_glyphs(&word.text, world_per_lpx, detail, sane.tracking, &mut cache, &mut wcount);
            // The start angle is drawn even for an empty word so a blank
            // entry never shifts the placement of the ones after it.
            let theta0 = rng.next_f32() * TAU;
            if glyphs.is_empty() {
                continue;
            }
            let (bmin, bmax) = bbox_of(&glyphs);
            let c = mul(add(bmin, bmax), 0.5);
            let half = add(mul(sub(bmax, bmin), 0.5), v2(pad, pad));
            match spiral_place(&rects, half, theta0, sane.size) {
                Some(at) => {
                    rects.push((sub(at, half), add(at, half)));
                    for g in glyphs.iter_mut() {
                        g.rest = add(at, sub(g.rest, c));
                        g.baseline += at.y - c.y;
                        g.word = placed_words;
                        g.r1 = weight;
                    }
                    placed.append(&mut glyphs);
                    placed_words += 1;
                }
                None => skipped += 1,
            }
        }
        if skipped > 0 {
            warnings.push(format!("cloud: {skipped} word(s) found no free spot on the spiral"));
        }
        let mut scene = Scene {
            words: placed_words,
            lines: 1,
            layout: TextLayout::Cloud,
            ..Scene::default()
        };
        centre_block(&mut placed, &mut scene);
        for g in placed.iter_mut() {
            g.uv = g.rest;
        }
        scene.glyphs = placed;
        scene
    })
}

// ---------------------------------------------------------------------------
// Parameters and the budget driver.
// ---------------------------------------------------------------------------

/// The floor parameters after sanitising.
#[derive(Clone, Copy)]
struct FloorSane {
    y: Option<f32>,
    size: Option<f32>,
    grid: usize,
}

/// The parameters after sanitising: finite, in range, never a NaN reaching
/// a vertex.
struct Sane {
    size: f32,
    depth: f32,
    bevel: f32,
    detail: f32,
    layout: TextLayout,
    ring_radius: Option<f32>,
    ring_close: bool,
    tunnel_pitch: f32,
    words: usize,
    seed: u64,
    budget: usize,
    helix_pitch: f32,
    grid_cols: usize,
    grid_pitch: f32,
    copies: usize,
    copies_turn: bool,
    changed: Vec<bool>,
    previous_glyphs: Option<Vec<u64>>,
    bevel_type: BevelType,
    bevel_rings: usize,
    floor: Option<FloorSane>,
    voxel_res: f32,
    voxel_layers: usize,
    voxel_waste: bool,
    tracking: f32,
}

impl Sane {
    fn new(p: &TextMeshParams) -> Self {
        let size = if p.size.is_finite() && p.size > 1e-4 { p.size.min(1e4) } else { 1.0 };
        let depth = if p.depth.is_finite() && p.depth > 0.0 { p.depth.min(size * 20.0) } else { 0.0 };
        let bevel = if p.bevel.is_finite() && p.bevel > 0.0 {
            p.bevel.min(0.4 * depth).min(0.3 * size)
        } else {
            0.0
        };
        let detail = sane_detail(p.detail);
        let ring_radius = p.ring_radius.filter(|r| r.is_finite() && *r > 1e-3);
        let tunnel_pitch = if p.tunnel_pitch.is_finite() && p.tunnel_pitch > 1e-3 {
            p.tunnel_pitch
        } else {
            2.5
        };
        let positive = |v: f32, auto: f32| if v.is_finite() && v > 1e-3 { v.min(1e5) } else { auto };
        let floor = p.floor.as_ref().map(|f| FloorSane {
            y: f.y.filter(|y| y.is_finite()),
            size: f.size.filter(|s| s.is_finite() && *s > 1e-3).map(|s| s.min(1e6)),
            grid: f.grid.clamp(1, 256),
        });
        Self {
            size,
            depth,
            bevel,
            detail,
            layout: p.layout,
            ring_radius,
            ring_close: p.ring_close,
            tunnel_pitch,
            words: p.words.max(1),
            seed: p.seed,
            budget: p.vert_budget.max(64),
            helix_pitch: positive(p.helix_pitch, 1.6 * size),
            grid_cols: p.grid_cols.min(4096),
            grid_pitch: positive(p.grid_pitch, 1.5 * size),
            copies: p.copies.clamp(1, MAX_COPIES),
            copies_turn: p.copies_turn,
            changed: p.changed.clone(),
            previous_glyphs: p.previous_glyphs.clone(),
            bevel_type: p.bevel_type,
            bevel_rings: p.bevel_rings.clamp(1, 8),
            floor,
            voxel_res: p.voxel_res.clamp(4, 40) as f32,
            voxel_layers: p.voxel_layers.clamp(1, 12),
            voxel_waste: p.voxel_waste,
            tracking: if p.tracking.is_finite() { p.tracking.clamp(-0.5, 2.0) } else { 0.0 },
        }
    }

    /// How many times every letter vertex is pushed: once per copy, and
    /// once more as its reflection when a floor is on.
    fn repeat(&self) -> usize {
        self.copies * if self.floor.is_some() { 2 } else { 1 }
    }
}

fn sane_weight(w: f32) -> f32 {
    if w.is_finite() {
        w.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// The reference cap height of a laid-out text in lpx: the first row's,
/// with a 0.7 em guess for a face that reports none.
fn cap_height_lpx(text: &LaidoutText) -> f32 {
    let row = match text.rows.first() {
        Some(r) => r,
        None => return 70.0,
    };
    if row.cap_height_in_lpxs.is_finite() && row.cap_height_in_lpxs > 1e-3 {
        return row.cap_height_in_lpxs;
    }
    let fs = row.glyphs.first().map(|g| g.font_size_in_lpxs).unwrap_or(100.0);
    (0.7 * fs).max(1e-3)
}

/// One glyph ready to emit: its 2D shape, scale, pivot, frame and channels.
struct Placed {
    shape: Rc<GlyphShape>,
    /// em -> world units.
    em_to_world: f32,
    /// Rest centre in text space (y up), world units.
    rest: Vec2f,
    /// Half extents of the ink box, world units.
    half: Vec2f,
    /// The row baseline in text space (y up), world units.
    baseline: f32,
    /// The row's descender below the baseline, world units (>= 0).
    descent: f32,
    word: usize,
    /// The `uv` channel: the pivot (or arc/height on the bent layouts).
    uv: Vec2f,
    /// The `a_r1` channel: the line ordinal, or the cloud weight.
    r1: f32,
    frame: Frame,
    /// The glyph's identity (font and glyph id): what the changed flags
    /// compare between builds.
    key: u64,
}

impl Placed {
    /// Vertices this glyph will push at the given extrusion, per copy.
    fn cost(&self, depth: f32, bevel: f32, bevel_segments: usize) -> usize {
        let p = self.shape.pts.len();
        let e = self.shape.wall_count();
        if depth <= 0.0 {
            p
        } else if bevel > 0.0 {
            2 * p + 4 * e + 8 * e * bevel_segments
        } else {
            2 * p + 4 * e
        }
    }
}

#[derive(Default)]
struct Scene {
    glyphs: Vec<Placed>,
    words: usize,
    lines: usize,
    width: f32,
    height: f32,
    ring_radius: f32,
    layout: TextLayout,
    /// Helix: the pitch used and the frame's tilt about its local z.
    helix_pitch: f32,
    helix_tilt: f32,
    /// Grid: the lattice used.
    grid_cols: usize,
    grid_rows: usize,
    grid_pitch: f32,
}

/// The budget driver shared by both builders: `make` lays the scene out at
/// a detail, and this degrades in the fixed order until the vertex count
/// fits, reporting every step; then it pushes the copies, the mirrors and
/// the floor.
fn build_scene<F>(sane: &Sane, mesh: &mut FxMesh, mut make: F) -> TextMeshReport
where
    F: FnMut(f32, &mut Vec<String>) -> Scene,
{
    let mut warnings = Vec::new();
    let mut detail = sane.detail;
    let mut bevel = sane.bevel;
    let depth = sane.depth;
    let profile = Profile::new(sane.bevel_type, sane.bevel_rings);
    let segs = profile.segments();
    let repeat = sane.repeat();
    let floor_grid = sane.floor.map(|f| fit_floor_grid(f.grid, sane.budget, &mut warnings));
    let floor_verts = floor_grid.map(floor_vertex_count).unwrap_or(0);
    let mut coarsened = 0;
    let (scene, emit_count) = loop {
        let mut scene_warnings = Vec::new();
        PERF_PASSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let scene = make(detail, &mut scene_warnings);
        let cost: usize =
            scene.glyphs.iter().map(|g| g.cost(depth, bevel, segs)).sum::<usize>() * repeat + floor_verts;
        if cost <= sane.budget {
            warnings.append(&mut scene_warnings);
            let n = scene.glyphs.len();
            break (scene, n);
        }
        if coarsened < 2 {
            coarsened += 1;
            detail *= 1.5;
            warnings.push(format!(
                "budget: {cost} vertices over {}, detail coarsened to {detail:.4} em",
                sane.budget
            ));
            continue;
        }
        if bevel > 0.0 {
            bevel = 0.0;
            warnings.push(format!("budget: {cost} vertices over {}, bevel dropped", sane.budget));
            continue;
        }
        warnings.append(&mut scene_warnings);
        let mut used = floor_verts;
        let mut keep = 0usize;
        for g in &scene.glyphs {
            let c = g.cost(depth, bevel, segs) * repeat;
            if used + c > sane.budget {
                break;
            }
            used += c;
            keep += 1;
        }
        warnings.push(format!(
            "budget: {cost} vertices over {}, {} of {} glyphs dropped",
            sane.budget,
            scene.glyphs.len() - keep,
            scene.glyphs.len()
        ));
        break (scene, keep);
    };

    mesh.clear();
    let glyphs = &scene.glyphs[..emit_count];
    let fallbacks = glyphs.iter().filter(|g| g.shape.fallback).count();
    let flags = glyph_flags(glyphs, sane);
    for copy in 0..sane.copies {
        for (ordinal, g) in glyphs.iter().enumerate() {
            let (uv, frame) = copy_place(g, copy, sane.copies, &scene, sane.copies_turn);
            let at = Attrs {
                a_id: ordinal as f32,
                uv,
                frame,
                r0: word_channel(g.word, ordinal, &flags),
                r1: g.r1 + COPY_STRIDE * copy as f32,
            };
            emit_glyph(mesh, g, &at, depth, bevel, &profile);
        }
    }
    let copy_radius = copy_radius(&scene, sane);
    let (mut floor_y, mut floor_size) = (0.0, 0.0);
    if let (Some(f), Some(grid)) = (sane.floor, floor_grid) {
        let (ground, footprint) = scene_ground(&scene, glyphs, sane, copy_radius);
        floor_y = f.y.unwrap_or(ground - 0.05 * sane.size);
        floor_size = f.size.unwrap_or(3.0 * footprint);
        push_mirrors(mesh);
        push_floor(mesh, floor_y, floor_size, grid);
    }
    TextMeshReport {
        glyphs: emit_count,
        words: scene.words,
        lines: scene.lines,
        width: scene.width,
        height: scene.height,
        vertices: mesh.vertex_count(),
        triangles: mesh.triangle_count(),
        ring_radius: scene.ring_radius,
        fallbacks,
        warnings,
        copies: sane.copies,
        copy_radius,
        copy_pitch: if scene.layout == TextLayout::Block && sane.copies_turn { sane.tunnel_pitch } else { 0.0 },
        helix_pitch: scene.helix_pitch,
        grid_cols: scene.grid_cols,
        grid_rows: scene.grid_rows,
        grid_pitch: scene.grid_pitch,
        bevel_rings: if bevel > 0.0 && depth > 0.0 { segs } else { 0 },
        floor_y,
        floor_size,
        floor_grid: floor_grid.unwrap_or(0),
        glyph_keys: glyphs.iter().map(|g| g.key).collect(),
        changed: flags,
        ..TextMeshReport::default()
    }
}

/// The `a_r0` channel: the word ordinal, + 0.5 when the glyph changed.
fn word_channel(word: usize, ordinal: usize, changed: &[bool]) -> f32 {
    let flag = if changed.get(ordinal).copied().unwrap_or(false) { CHANGED_FLAG } else { 0.0 };
    word as f32 + flag
}

// ---------------------------------------------------------------------------
// Reading the layout: which glyph, where, in which word and line.
// ---------------------------------------------------------------------------

/// One build's glyph shapes: (font, glyph id, detail bits). Backed by the
/// thread's `ShapeStore`, so a miss here is usually a store hit.
type ShapeCache = HashMap<(usize, u16, u32), Option<Rc<GlyphShape>>>;

/// The most shapes the store keeps; past it the store is emptied and fills
/// again from the builds that follow (a new face or `detail`, a huge glyph
/// set). A few hundred bytes to a few KB per shape.
const SHAPE_STORE_MAX: usize = 4096;

/// Glyph shapes kept across builds on one thread (module docs, "The
/// pipeline, per glyph"). Each entry holds the font's `Rc`, so the pointer
/// in its key can never be reused by another font while the entry lives.
#[derive(Default)]
struct ShapeStore {
    map: HashMap<(usize, u16, u32), (Rc<makepad_draw::text::font::Font>, Option<Rc<GlyphShape>>)>,
    /// Shapes built from outlines on this thread (store misses).
    built: u64,
}

thread_local! {
    static SHAPE_STORE: std::cell::RefCell<ShapeStore> = std::cell::RefCell::new(ShapeStore::default());
}

/// Shapes this thread has built from outlines so far (store misses). A
/// rebuild over glyphs already seen adds nothing.
pub fn shapes_built() -> u64 {
    SHAPE_STORE.with(|s| s.borrow().built)
}

/// How many shapes this thread's store holds.
pub fn shapes_stored() -> usize {
    SHAPE_STORE.with(|s| s.borrow().map.len())
}

/// `detail` as a build uses it: clamped to 0.004..0.05 em, 0.012 when not
/// finite. The shape store is keyed by this value.
pub fn sane_detail(detail: f32) -> f32 {
    if detail.is_finite() {
        detail.clamp(DETAIL_MIN, DETAIL_MAX)
    } else {
        0.012
    }
}

/// Printable ASCII, space to tilde: the set a host warms for a face before
/// a pick needs it (`ShapeWarmup`).
pub const PRINTABLE_ASCII: &str =
    " !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~";

type ShapeKey = (usize, u16, u32);

fn shape_key(font: &Rc<makepad_draw::text::font::Font>, id: u16, detail: f32) -> ShapeKey {
    (Rc::as_ptr(font) as usize, id, detail.to_bits())
}

fn shape_is_stored(key: &ShapeKey) -> bool {
    SHAPE_STORE.with(|s| s.borrow().map.contains_key(key))
}

/// One glyph the warm-up will shape.
struct WarmGlyph {
    font: Rc<makepad_draw::text::font::Font>,
    id: u16,
    detail: f32,
}

impl WarmGlyph {
    fn key(&self) -> ShapeKey {
        shape_key(&self.font, self.id, self.detail)
    }
}

/// The shape warm-up queue (module docs, "The pipeline, per glyph"): the
/// glyphs a host wants in this thread's store before a build needs them.
/// A glyph is queued only while it is neither stored nor already queued,
/// so queueing the same text again adds nothing.
#[derive(Default)]
pub struct ShapeWarmup {
    queue: VecDeque<WarmGlyph>,
    queued: HashSet<ShapeKey>,
}

impl ShapeWarmup {
    /// Glyphs still queued.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Forget everything queued (a new document).
    pub fn clear(&mut self) {
        self.queue.clear();
        self.queued.clear();
    }

    /// Queue the distinct glyphs of `text` that this thread has not shaped
    /// at `detail`, in reading order. `first` puts them ahead of everything
    /// queued (a glyph already queued further back moves up); otherwise they
    /// go to the back and a glyph already queued stays where it is. Returns
    /// how many glyphs were queued or moved.
    pub fn queue_text(&mut self, text: &LaidoutText, detail: f32, first: bool) -> usize {
        let mut seen = HashSet::new();
        let mut fresh = Vec::new();
        for glyph in text.rows.iter().flat_map(|row| row.glyphs.iter()) {
            if glyph.id == 0 {
                continue;
            }
            let key = shape_key(&glyph.font, glyph.id, detail);
            if !seen.insert(key) || shape_is_stored(&key) {
                continue;
            }
            if self.queued.contains(&key) && !first {
                continue;
            }
            fresh.push(WarmGlyph { font: glyph.font.clone(), id: glyph.id, detail });
        }
        if fresh.is_empty() {
            return 0;
        }
        if first {
            let moving: HashSet<ShapeKey> = fresh.iter().map(WarmGlyph::key).collect();
            self.queue.retain(|g| !moving.contains(&g.key()));
            for glyph in fresh.iter().rev() {
                self.queue.push_front(WarmGlyph { font: glyph.font.clone(), id: glyph.id, detail: glyph.detail });
            }
        } else {
            for glyph in &fresh {
                self.queue.push_back(WarmGlyph { font: glyph.font.clone(), id: glyph.id, detail: glyph.detail });
            }
        }
        self.queued.extend(fresh.iter().map(WarmGlyph::key));
        fresh.len()
    }

    /// True when every glyph of `text` is in this thread's store at `detail`:
    /// a build of it shapes nothing.
    pub fn text_is_warm(text: &LaidoutText, detail: f32) -> bool {
        text.rows
            .iter()
            .flat_map(|row| row.glyphs.iter())
            .filter(|glyph| glyph.id != 0)
            .all(|glyph| shape_is_stored(&shape_key(&glyph.font, glyph.id, detail)))
    }

    /// Shape queued glyphs into the store in queue order: always the first,
    /// then more until `now()` (seconds) is `budget` seconds past the call.
    /// A glyph a build stored meanwhile is dropped without cost. Returns
    /// how many shapes this call built.
    pub fn drain(&mut self, budget: f64, now: &mut dyn FnMut() -> f64) -> usize {
        let start = now();
        let mut built = 0;
        while let Some(glyph) = self.queue.pop_front() {
            let key = glyph.key();
            self.queued.remove(&key);
            if shape_is_stored(&key) {
                continue;
            }
            stored_shape(&glyph.font, glyph.id, glyph.detail);
            built += 1;
            if now() - start >= budget {
                break;
            }
        }
        built
    }
}

/// The glyphs of a laid-out text as `Placed` entries in text space (y up,
/// world units, NOT yet centred), with word ordinals continuing from
/// `*word_counter`. `tracking` (em) moves glyph k of each row right by
/// k * tracking em of its font size.
fn collect_glyphs(
    text: &LaidoutText,
    world_per_lpx: f32,
    detail: f32,
    tracking: f32,
    cache: &mut ShapeCache,
    word_counter: &mut usize,
) -> Vec<Placed> {
    let mut out = Vec::new();
    for (line, row) in text.rows.iter().enumerate() {
        let (word_of_byte, row_words) = word_map(&row.text);
        let descent = if row.descender_in_lpxs.is_finite() { row.descender_in_lpxs.abs() * world_per_lpx } else { 0.0 };
        let base = *word_counter;
        for (k, glyph) in row.glyphs.iter().enumerate() {
            let shape = match glyph_shape(glyph, detail, cache) {
                Some(s) => s,
                None => continue,
            };
            let fs = glyph.font_size_in_lpxs;
            let em_to_world = fs * world_per_lpx;
            let mut pen_x = row.origin_in_lpxs.x + glyph.origin_in_lpxs.x + glyph.offset_in_lpxs();
            if tracking != 0.0 {
                pen_x += tracking * fs * k as f32;
            }
            let baseline_y = -row.origin_in_lpxs.y * world_per_lpx;
            let c = mul(add(shape.min, shape.max), 0.5);
            let rest = v2(pen_x * world_per_lpx + c.x * em_to_world, baseline_y + c.y * em_to_world);
            let half = mul(sub(shape.max, shape.min), 0.5 * em_to_world);
            let local_word = word_of_byte
                .get(glyph.cluster.min(word_of_byte.len().saturating_sub(1)))
                .copied()
                .unwrap_or(0);
            out.push(Placed {
                shape,
                em_to_world,
                rest,
                half,
                baseline: baseline_y,
                descent,
                word: base + local_word,
                uv: rest,
                r1: line as f32,
                frame: Frame::identity(),
                key: glyph_key(glyph),
            });
        }
        *word_counter += row_words;
    }
    out
}

/// Per byte of a row's text: the ordinal of the word it belongs to
/// (whitespace bytes join the word before them), plus the row's word count.
fn word_map(text: &str) -> (Vec<usize>, usize) {
    let mut map = vec![0usize; text.len()];
    let mut in_word = false;
    let mut word: isize = -1;
    for (i, ch) in text.char_indices() {
        if ch.is_whitespace() {
            in_word = false;
        } else if !in_word {
            in_word = true;
            word += 1;
        }
        let w = word.max(0) as usize;
        for b in map.iter_mut().skip(i).take(ch.len_utf8()) {
            *b = w;
        }
    }
    (map, (word + 1).max(0) as usize)
}

/// A glyph's identity for the changed flags: its font and glyph id (two
/// characters that shape to the same glyph look the same, so they are the
/// same key).
fn glyph_key(glyph: &LaidoutGlyph) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    glyph.font.id().hash(&mut h);
    glyph.id.hash(&mut h);
    h.finish()
}

/// Per glyph ordinal: the changed flag (module docs, "Changed glyphs").
fn glyph_flags(glyphs: &[Placed], sane: &Sane) -> Vec<bool> {
    match &sane.previous_glyphs {
        Some(prev) => glyphs.iter().enumerate().map(|(i, g)| prev.get(i) != Some(&g.key)).collect(),
        None => (0..glyphs.len()).map(|i| sane.changed.get(i).copied().unwrap_or(false)).collect(),
    }
}

/// `VJFX_PERF`: nanoseconds spent building glyph shapes (cache misses),
/// how many were built, and how many scene passes the budget loop ran.
pub static PERF_SHAPE_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static PERF_SHAPES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static PERF_PASSES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn glyph_shape(glyph: &LaidoutGlyph, detail: f32, cache: &mut ShapeCache) -> Option<Rc<GlyphShape>> {
    if glyph.id == 0 {
        return None;
    }
    let key = shape_key(&glyph.font, glyph.id, detail);
    if let Some(hit) = cache.get(&key) {
        return hit.clone();
    }
    let built = stored_shape(&glyph.font, glyph.id, detail);
    cache.insert(key, built.clone());
    built
}

/// A glyph's shape from this thread's store, built from its outline and
/// stored on a miss (the only place a shape is built).
fn stored_shape(font: &Rc<makepad_draw::text::font::Font>, id: u16, detail: f32) -> Option<Rc<GlyphShape>> {
    let key = shape_key(font, id, detail);
    let stored = SHAPE_STORE.with(|s| s.borrow().map.get(&key).map(|(_, shape)| shape.clone()));
    if let Some(shape) = stored {
        return shape;
    }
    // Natively timed; the browser's std has no clock.
    #[cfg(not(target_arch = "wasm32"))]
    let perf_t0 = std::time::Instant::now();
    let built = font
        .glyph_outline_rc(id)
        .and_then(|outline| GlyphShape::build(outline.commands(), font.units_per_em(), detail))
        .map(Rc::new);
    #[cfg(not(target_arch = "wasm32"))]
    PERF_SHAPE_NS.fetch_add(perf_t0.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
    PERF_SHAPES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    SHAPE_STORE.with(|s| {
        let mut s = s.borrow_mut();
        if s.map.len() >= SHAPE_STORE_MAX {
            s.map.clear();
        }
        s.map.insert(key, (font.clone(), built.clone()));
        s.built += 1;
    });
    built
}

fn bbox_of(glyphs: &[Placed]) -> (Vec2f, Vec2f) {
    let mut min = v2(f32::MAX, f32::MAX);
    let mut max = v2(f32::MIN, f32::MIN);
    for g in glyphs {
        min = v2(min.x.min(g.rest.x - g.half.x), min.y.min(g.rest.y - g.half.y));
        max = v2(max.x.max(g.rest.x + g.half.x), max.y.max(g.rest.y + g.half.y));
    }
    if glyphs.is_empty() {
        (v2(0.0, 0.0), v2(0.0, 0.0))
    } else {
        (min, max)
    }
}

/// Centre the ink of the block on the origin and record its extents.
fn centre_block(glyphs: &mut [Placed], scene: &mut Scene) {
    let (min, max) = bbox_of(glyphs);
    let c = mul(add(min, max), 0.5);
    for g in glyphs.iter_mut() {
        g.rest = sub(g.rest, c);
        g.baseline -= c.y;
    }
    scene.width = max.x - min.x;
    scene.height = max.y - min.y;
}

// ---------------------------------------------------------------------------
// The bent layouts.
// ---------------------------------------------------------------------------

/// A row's laid-out width plus what `tracking` adds: every glyph after the
/// first moves right by `tracking` em of its font size.
fn tracked_width_lpx(row: &makepad_draw::text::layouter::LaidoutRow, tracking: f32) -> f32 {
    if tracking == 0.0 {
        return row.width_in_lpxs;
    }
    let fs = row.glyphs.first().map(|g| g.font_size_in_lpxs).unwrap_or(0.0);
    row.width_in_lpxs + tracking * fs * row.glyphs.len().saturating_sub(1) as f32
}

fn arrange_ring(
    glyphs: &mut [Placed],
    sane: &Sane,
    text: &LaidoutText,
    world_per_lpx: f32,
    scene: &mut Scene,
    warnings: &mut Vec<String>,
) {
    let first_width = text
        .rows
        .first()
        .map(|r| tracked_width_lpx(r, sane.tracking) * world_per_lpx)
        .filter(|w| w.is_finite() && *w > 1e-4)
        .unwrap_or(scene.width.max(1e-3));
    let widest = text
        .rows
        .iter()
        .map(|r| tracked_width_lpx(r, sane.tracking) * world_per_lpx)
        .filter(|w| w.is_finite())
        .fold(first_width, f32::max)
        .max(1e-3);
    let radius = sane.ring_radius.unwrap_or(widest / TAU * 1.05).max(1e-3);
    // Arc pitch stretch: the first row spans the circumference minus the
    // 5 % gap. Auto radius on a one-row text gives exactly 1.
    let stretch = if sane.ring_close { (TAU * radius / 1.05) / first_width } else { 1.0 };
    if sane.ring_close && (stretch < 0.5 || stretch > 2.0) {
        warnings.push(format!("ring_close stretches the arc pitch by {stretch:.2}"));
    }
    for g in glyphs.iter_mut() {
        let arc = g.rest.x * stretch;
        g.uv = v2(arc, g.rest.y);
        g.frame = Frame::rot_y(arc / radius);
    }
    scene.ring_radius = radius;
}

fn arrange_tunnel(glyphs: &mut [Placed], sane: &Sane, scene: &mut Scene) {
    // Per word: the x span of its glyphs, so each word centres on its ring.
    let mut span: HashMap<usize, (f32, f32)> = HashMap::new();
    for g in glyphs.iter() {
        let e = span.entry(g.word).or_insert((f32::MAX, f32::MIN));
        e.0 = e.0.min(g.rest.x - g.half.x);
        e.1 = e.1.max(g.rest.x + g.half.x);
    }
    let longest = span.values().map(|(a, b)| b - a).fold(0.0f32, f32::max);
    let radius = sane
        .ring_radius
        .unwrap_or((longest / TAU * 1.05).max(2.0 * sane.size))
        .max(1e-3);
    for g in glyphs.iter_mut() {
        let (x0, x1) = span.get(&g.word).copied().unwrap_or((0.0, 0.0));
        let arc = g.rest.x - 0.5 * (x0 + x1);
        g.uv = v2(arc, g.rest.y - g.baseline);
        let phi = g.word as f32 * TUNNEL_GOLDEN_ANGLE - arc / radius;
        g.frame = Frame::rot_z(phi - std::f32::consts::FRAC_PI_2);
    }
    scene.ring_radius = radius;
}

/// Every row end to end (half a cap of space between rows), wound on a
/// helix about +Y that descends `helix_pitch` per turn as the text reads.
fn arrange_helix(glyphs: &mut [Placed], sane: &Sane, scene: &mut Scene) {
    let lines = glyphs.iter().map(|g| g.r1 as usize + 1).max().unwrap_or(0);
    let mut span = vec![(f32::MAX, f32::MIN); lines];
    for g in glyphs.iter() {
        let s = &mut span[g.r1 as usize];
        s.0 = s.0.min(g.rest.x - g.half.x);
        s.1 = s.1.max(g.rest.x + g.half.x);
    }
    let gap = 0.5 * sane.size;
    let mut offset = vec![0.0f32; lines];
    let mut run = 0.0f32;
    for (l, s) in span.iter().enumerate() {
        offset[l] = run;
        if s.1 >= s.0 {
            run += (s.1 - s.0) + gap;
        }
    }
    let total = (run - gap).max(0.0);
    let radius = sane
        .ring_radius
        .unwrap_or((total / (3.0 * TAU)).max(1.5 * sane.size))
        .max(1e-3);
    let pitch = sane.helix_pitch;
    let slope = -pitch / (TAU * radius);
    let tilt = slope.atan();
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for g in glyphs.iter_mut() {
        let l = g.r1 as usize;
        let arc = g.rest.x - span[l].0 + offset[l] - 0.5 * total;
        let h = g.rest.y - g.baseline + slope * arc;
        g.uv = v2(arc, h);
        lo = lo.min(h - g.half.y);
        hi = hi.max(h + g.half.y);
    }
    let shift = if lo <= hi { 0.5 * (lo + hi) } else { 0.0 };
    for g in glyphs.iter_mut() {
        g.uv.y -= shift;
        g.frame = helix_frame(g.uv.x / radius, tilt);
    }
    scene.ring_radius = radius;
    scene.helix_pitch = pitch;
    scene.helix_tilt = tilt;
}

/// The glyphs in reading order on a cols x rows lattice lying in the XZ
/// plane facing +Y, row 0 farthest (-z).
fn arrange_grid(glyphs: &mut [Placed], sane: &Sane, scene: &mut Scene) {
    let n = glyphs.len();
    let cols = if sane.grid_cols > 0 {
        sane.grid_cols
    } else {
        ((n as f32).sqrt().ceil() as usize).max(1)
    };
    let rows = (n + cols - 1) / cols;
    let pitch = sane.grid_pitch;
    let cx = 0.5 * (cols as f32 - 1.0);
    let cz = 0.5 * (rows as f32 - 1.0);
    for (i, g) in glyphs.iter_mut().enumerate() {
        let (col, row) = (i % cols, i / cols);
        g.uv = v2((col as f32 - cx) * pitch, (row as f32 - cz) * pitch);
        g.r1 = row as f32;
        g.frame = Frame::grid_flat();
    }
    scene.grid_cols = cols;
    scene.grid_rows = rows;
    scene.grid_pitch = pitch;
    scene.lines = rows;
    scene.width = cols as f32 * pitch;
    scene.height = rows as f32 * pitch;
}

/// The helix frame at angle θ: turned about Y, then tilted about its own z
/// so local +x runs along the coil.
fn helix_frame(theta: f32, tilt: f32) -> Frame {
    Frame::rot_y(theta).compose(&Frame::rot_z(tilt))
}

/// Where copy `copy` of `n` puts a glyph: the layouts whose turned pivot
/// the `uv` channel can carry get the turn BAKED (ring, helix, tunnel: the
/// arc shifted by the copy's share of the circumference; grid: the cell
/// centre turned about Y); line, block and cloud copies are exact
/// duplicates and the shader applies the copy transform (module docs).
fn copy_place(g: &Placed, copy: usize, n: usize, scene: &Scene, turn_copies: bool) -> (Vec2f, Frame) {
    if copy == 0 || n < 2 || !turn_copies {
        return (g.uv, g.frame);
    }
    let turn = TAU * copy as f32 / n as f32;
    let r = scene.ring_radius.max(1e-3);
    match scene.layout {
        TextLayout::Ring => {
            let uv = v2(g.uv.x + turn * r, g.uv.y);
            (uv, Frame::rot_y(uv.x / r))
        }
        TextLayout::Helix => {
            let uv = v2(g.uv.x + turn * r, g.uv.y);
            (uv, helix_frame(uv.x / r, scene.helix_tilt))
        }
        TextLayout::Tunnel => {
            let uv = v2(g.uv.x - turn * r, g.uv.y);
            let phi = g.word as f32 * TUNNEL_GOLDEN_ANGLE - uv.x / r;
            (uv, Frame::rot_z(phi - std::f32::consts::FRAC_PI_2))
        }
        TextLayout::Grid => {
            let (s, c) = turn.sin_cos();
            let uv = v2(g.uv.x * c + g.uv.y * s, -g.uv.x * s + g.uv.y * c);
            (uv, Frame::rot_y(turn).compose(&Frame::grid_flat()))
        }
        _ => (g.uv, g.frame),
    }
}

/// Line / cloud copies: how far from the turning axis the shader sets each
/// copy — the apothem of an n-gon of faces as high (line) or wide (cloud)
/// as the text, never less than the letters' own depth needs.
fn copy_radius(scene: &Scene, sane: &Sane) -> f32 {
    let n = sane.copies;
    if n < 2 || !sane.copies_turn {
        return 0.0;
    }
    let clear = 0.6 * sane.depth + 0.05 * sane.size;
    let t = (std::f32::consts::PI / n as f32).tan();
    let apothem = |extent: f32| if t.is_finite() && t > 1e-3 { 0.55 * extent / t } else { 0.0 };
    match scene.layout {
        TextLayout::Line => apothem(scene.height).max(clear),
        TextLayout::Cloud => apothem(scene.width).max(clear),
        _ => 0.0,
    }
}

/// The auto floor: (the lowest point the text stands on, the footprint the
/// plane's edge is three times of).
fn scene_ground(scene: &Scene, glyphs: &[Placed], sane: &Sane, copy_radius: f32) -> (f32, f32) {
    let size = sane.size;
    let (ground, footprint) = match scene.layout {
        TextLayout::Tunnel => {
            let reach = scene.ring_radius + 1.5 * size;
            (-reach, 2.0 * reach)
        }
        TextLayout::Grid => (-0.5 * sane.depth, scene.width.max(scene.height)),
        _ => {
            // uv.y is the pivot height on every remaining layout.
            let mut ground = glyphs
                .iter()
                .map(|g| g.uv.y + (g.baseline - g.rest.y) - g.descent)
                .fold(f32::MAX, f32::min);
            if glyphs.is_empty() || !ground.is_finite() {
                ground = -0.5 * size;
            }
            let mut footprint = match scene.layout {
                TextLayout::Ring | TextLayout::Helix => 2.0 * scene.ring_radius,
                _ => scene.width,
            };
            if sane.copies > 1 && sane.copies_turn {
                match scene.layout {
                    TextLayout::Line => {
                        ground = ground.min(-(copy_radius.hypot(0.5 * scene.height) + 0.5 * sane.depth));
                    }
                    TextLayout::Cloud => footprint = footprint.max(2.0 * copy_radius + scene.width),
                    TextLayout::Block => {
                        footprint = footprint.max(2.0 * sane.tunnel_pitch * (sane.copies - 1) as f32)
                    }
                    _ => {}
                }
            }
            (ground, footprint)
        }
    };
    (ground, footprint.max(size))
}

/// Archimedean spiral search for a free spot: the first candidate whose
/// padded rectangle touches none placed. The cloud is squashed to 0.7
/// vertically so it reads as a wide cloud rather than a disc.
fn spiral_place(rects: &[(Vec2f, Vec2f)], half: Vec2f, theta0: f32, size: f32) -> Option<Vec2f> {
    let turn = 0.5 * size;
    let step = 0.2 * size;
    let mut theta = 0.0f32;
    for _ in 0..8000 {
        let r = turn * theta / TAU;
        let at = v2(r * (theta + theta0).cos(), 0.7 * r * (theta + theta0).sin());
        let free = rects.iter().all(|(min, max)| {
            let oc = mul(add(*min, *max), 0.5);
            let oh = mul(sub(*max, *min), 0.5);
            (at.x - oc.x).abs() >= half.x + oh.x || (at.y - oc.y).abs() >= half.y + oh.y
        });
        if free {
            return Some(at);
        }
        theta += (step / r.max(step)).clamp(0.05, 0.8);
    }
    None
}

// ---------------------------------------------------------------------------
// The 2D glyph shape: flattened contours + cap triangles, in em, y up.
// ---------------------------------------------------------------------------

struct Contour {
    start: usize,
    len: usize,
    /// Signed shoelace area, em².
    area: f32,
}

struct GlyphShape {
    /// Every contour point, em, y up.
    pts: Vec<Vec2f>,
    contours: Vec<Contour>,
    /// Per point i, for the edge i -> next: +1 when the right-hand normal
    /// (dy, -dx) points out of the ink, -1 when the left-hand one does, 0
    /// when the edge has ink on both sides (no wall).
    edge_out: Vec<f32>,
    /// Cap triangles, CCW (normal +z), indices into `pts`.
    tris: Vec<[u32; 3]>,
    /// Per point: the mitred outward direction times the mitre length, so
    /// the inset point at bevel b is `p - mitre * b`.
    mitre: Vec<Vec2f>,
    /// Per point: the largest bevel (em) its inset may take before it
    /// crosses the middle of the stroke — see `inset_limits`.
    inset_max: Vec<f32>,
    min: Vec2f,
    max: Vec2f,
    /// The caps came from the ear-clip fallback.
    fallback: bool,
    /// Intersection points inserted where contours crossed (read by the
    /// tests; the build itself only needs the split contours).
    #[cfg_attr(not(test), allow(dead_code))]
    intersections: usize,
}

impl GlyphShape {
    fn build(commands: &[Command], units_per_em: f32, tol_em: f32) -> Option<Self> {
        let upem = if units_per_em.is_finite() && units_per_em > 0.0 { units_per_em } else { 1000.0 };
        let (split, intersections) = split_intersections(flatten_outline(commands, upem, tol_em));
        let mut rings: Vec<Vec<Vec2f>> = split
            .into_iter()
            .filter_map(|raw| weld_contour(raw).map(|(w, _)| w))
            .collect();
        if rings.is_empty() || rings.iter().flatten().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
            return None;
        }
        let mut min = v2(f32::MAX, f32::MAX);
        let mut max = v2(f32::MIN, f32::MIN);
        for p in rings.iter().flatten() {
            min = v2(min.x.min(p.x), min.y.min(p.y));
            max = v2(max.x.max(p.x), max.y.max(p.y));
        }
        // Caps: the verified CDT, retried and refined before the fallback.
        let mut refinements = 0;
        let (pts, contours, tris, fallback) = loop {
            let (pts, contours) = assemble_rings(&rings);
            let first = cdt_caps(&pts, &contours, min, max, false);
            let missing = match first {
                Ok(tris) if !tris.is_empty() => break (pts, contours, tris, false),
                Ok(_) => Vec::new(),
                Err(missing) => missing,
            };
            if !missing.is_empty() {
                if let Ok(tris) = cdt_caps(&pts, &contours, min, max, true) {
                    if !tris.is_empty() {
                        break (pts, contours, tris, false);
                    }
                }
                if refinements < 3 {
                    refinements += 1;
                    refine_rings(&mut rings, &missing);
                    continue;
                }
            }
            let tris = ear_clip_largest(&pts, &contours);
            break (pts, contours, tris, true);
        };
        // Which side of each edge is ink: probe both sides of its midpoint.
        let mut edge_out = vec![0.0f32; pts.len()];
        for c in &contours {
            for k in 0..c.len {
                let a = pts[c.start + k];
                let b = pts[c.start + (k + 1) % c.len];
                let e = sub(b, a);
                let l = len2(e).max(1e-12);
                let left = v2(-e.y / l, e.x / l);
                let mid = mul(add(a, b), 0.5);
                let d = PROBE_EM.min(0.2 * l);
                let ink_left = winding_number(&pts, &contours, add(mid, mul(left, d))) != 0;
                let ink_right = winding_number(&pts, &contours, sub(mid, mul(left, d))) != 0;
                edge_out[c.start + k] = match (ink_left, ink_right) {
                    (true, false) => 1.0,
                    (false, true) => -1.0,
                    _ => 0.0,
                };
            }
        }
        let mitre = mitred_normals(&pts, &contours, &edge_out);
        let inset_max = inset_limits(&pts, &contours, &mitre);
        Some(Self { pts, contours, edge_out, tris, mitre, inset_max, min, max, fallback, intersections })
    }

    /// Edges that get a side wall (ink on exactly one side).
    fn wall_count(&self) -> usize {
        self.edge_out.iter().filter(|o| **o != 0.0).count()
    }
}

/// Concatenate the rings into the flat point list + contour table.
fn assemble_rings(rings: &[Vec<Vec2f>]) -> (Vec<Vec2f>, Vec<Contour>) {
    let mut pts = Vec::with_capacity(rings.iter().map(|r| r.len()).sum());
    let mut contours = Vec::with_capacity(rings.len());
    for ring in rings {
        contours.push(Contour { start: pts.len(), len: ring.len(), area: shoelace(ring) });
        pts.extend_from_slice(ring);
    }
    (pts, contours)
}

/// Insert the midpoint of every named edge (contour index, edge index) —
/// a point on the contour, so the shape is unchanged while the constraint
/// gets shorter and easier for the triangulator.
fn refine_rings(rings: &mut [Vec<Vec2f>], edges: &[(usize, usize)]) {
    let mut sorted: Vec<(usize, usize)> = edges.to_vec();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    sorted.dedup();
    for (ci, k) in sorted {
        let ring = match rings.get_mut(ci) {
            Some(r) => r,
            None => continue,
        };
        let n = ring.len();
        if n < 2 || k >= n {
            continue;
        }
        let mid = mul(add(ring[k], ring[(k + 1) % n]), 0.5);
        ring.insert(k + 1, mid);
    }
}

/// Cut every contour at the points where it crosses another contour (or
/// itself) so the triangulation's constraints never cross. Both contours
/// receive the identical point, and a T-junction reuses the touching
/// vertex, so the global weld in the CDT makes them one vertex.
fn split_intersections(contours: Vec<Vec<Vec2f>>) -> (Vec<Vec<Vec2f>>, usize) {
    struct E {
        c: usize,
        k: usize,
        a: Vec2f,
        b: Vec2f,
    }
    let mut edges: Vec<E> = Vec::new();
    for (c, pts) in contours.iter().enumerate() {
        let n = pts.len();
        if n < 2 {
            continue;
        }
        for k in 0..n {
            edges.push(E { c, k, a: pts[k], b: pts[(k + 1) % n] });
        }
    }
    let mut splits: Vec<Vec<(f32, Vec2f)>> = vec![Vec::new(); edges.len()];
    let mut count = 0usize;
    const EPS: f32 = 1e-6;
    for i in 0..edges.len() {
        let e1 = &edges[i];
        let (min1, max1) = (
            v2(e1.a.x.min(e1.b.x), e1.a.y.min(e1.b.y)),
            v2(e1.a.x.max(e1.b.x), e1.a.y.max(e1.b.y)),
        );
        for j in (i + 1)..edges.len() {
            let e2 = &edges[j];
            if e2.a.x.max(e2.b.x) < min1.x
                || e2.a.x.min(e2.b.x) > max1.x
                || e2.a.y.max(e2.b.y) < min1.y
                || e2.a.y.min(e2.b.y) > max1.y
            {
                continue;
            }
            if e1.c == e2.c {
                let n = contours[e1.c].len();
                if (e1.k + 1) % n == e2.k || (e2.k + 1) % n == e1.k {
                    continue;
                }
            }
            let r = sub(e1.b, e1.a);
            let s = sub(e2.b, e2.a);
            let denom = cross2(r, s);
            if denom.abs() < 1e-12 {
                continue;
            }
            let qp = sub(e2.a, e1.a);
            let t = cross2(qp, s) / denom;
            let u = cross2(qp, r) / denom;
            let t_in = t > EPS && t < 1.0 - EPS;
            let u_in = u > EPS && u < 1.0 - EPS;
            let t_end = (-EPS..=EPS).contains(&t) || (1.0 - EPS..=1.0 + EPS).contains(&t);
            let u_end = (-EPS..=EPS).contains(&u) || (1.0 - EPS..=1.0 + EPS).contains(&u);
            if t_in && u_in {
                let p = add(e1.a, mul(r, t));
                splits[i].push((t, p));
                splits[j].push((u, p));
                count += 1;
            } else if t_in && u_end {
                let p = if u < 0.5 { e2.a } else { e2.b };
                splits[i].push((t, p));
                count += 1;
            } else if u_in && t_end {
                let p = if t < 0.5 { e1.a } else { e1.b };
                splits[j].push((u, p));
                count += 1;
            }
        }
    }
    if count == 0 {
        return (contours, 0);
    }
    let mut out: Vec<Vec<Vec2f>> = contours.iter().map(|c| Vec::with_capacity(c.len() + 4)).collect();
    for (i, e) in edges.iter().enumerate() {
        out[e.c].push(e.a);
        let cuts = &mut splits[i];
        cuts.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut last_t = -1.0f32;
        for (t, p) in cuts.iter() {
            if *t - last_t > 1e-7 {
                out[e.c].push(*p);
            }
            last_t = *t;
        }
    }
    (out, count)
}

/// Outline commands (font units) -> raw contours in em. Curves are cut by
/// their control polygon length over the tolerance.
fn flatten_outline(commands: &[Command], upem: f32, tol_em: f32) -> Vec<Vec<Vec2f>> {
    let s = 1.0 / upem;
    let tol = if tol_em.is_finite() && tol_em > 1e-5 { tol_em } else { 0.012 };
    let mut out: Vec<Vec<Vec2f>> = Vec::new();
    let mut cur: Vec<Vec2f> = Vec::new();
    let mut last = v2(0.0, 0.0);
    let em = |p: makepad_draw::text::geom::Point<f32>| v2(p.x * s, p.y * s);
    for cmd in commands {
        match *cmd {
            Command::MoveTo(p) => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                last = em(p);
                cur.push(last);
            }
            Command::LineTo(p) => {
                last = em(p);
                cur.push(last);
            }
            Command::QuadTo(c, p) => {
                let (c, p) = (em(c), em(p));
                let n = seg_count(len2(sub(c, last)) + len2(sub(p, c)), tol);
                for i in 1..=n {
                    let t = i as f32 / n as f32;
                    let u = 1.0 - t;
                    cur.push(add(add(mul(last, u * u), mul(c, 2.0 * u * t)), mul(p, t * t)));
                }
                last = p;
            }
            Command::CurveTo(c1, c2, p) => {
                let (c1, c2, p) = (em(c1), em(c2), em(p));
                let n = seg_count(
                    len2(sub(c1, last)) + len2(sub(c2, c1)) + len2(sub(p, c2)),
                    tol,
                );
                for i in 1..=n {
                    let t = i as f32 / n as f32;
                    let u = 1.0 - t;
                    let q = add(
                        add(mul(last, u * u * u), mul(c1, 3.0 * u * u * t)),
                        add(mul(c2, 3.0 * u * t * t), mul(p, t * t * t)),
                    );
                    cur.push(q);
                }
                last = p;
            }
            Command::Close => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn seg_count(poly_len: f32, tol: f32) -> usize {
    let n = (poly_len / (2.0 * tol)).max(0.0).sqrt().ceil();
    if n.is_finite() {
        (n as usize).clamp(1, 64)
    } else {
        1
    }
}

/// Weld near-duplicate consecutive points (and the closing pair), drop
/// contours too small to mean anything. Returns the points + signed area.
fn weld_contour(raw: Vec<Vec2f>) -> Option<(Vec<Vec2f>, f32)> {
    let w2 = WELD_EM * WELD_EM;
    let mut w: Vec<Vec2f> = Vec::with_capacity(raw.len());
    for p in raw {
        if let Some(l) = w.last() {
            if dist2(*l, p) < w2 {
                continue;
            }
        }
        w.push(p);
    }
    while w.len() > 1 && dist2(w[0], w[w.len() - 1]) < w2 {
        w.pop();
    }
    if w.len() < 3 {
        return None;
    }
    let area = shoelace(&w);
    if !area.is_finite() || area.abs() < MIN_AREA_EM2 {
        return None;
    }
    Some((w, area))
}

fn shoelace(pts: &[Vec2f]) -> f32 {
    let n = pts.len();
    let mut a = 0.0f32;
    for i in 0..n {
        let p = pts[i];
        let q = pts[(i + 1) % n];
        a += p.x * q.y - q.x * p.y;
    }
    0.5 * a
}

/// Winding number of `p` against every contour (the nonzero fill rule).
fn winding_number(pts: &[Vec2f], contours: &[Contour], p: Vec2f) -> i32 {
    let mut wn = 0i32;
    for c in contours {
        for k in 0..c.len {
            let a = pts[c.start + k];
            let b = pts[c.start + (k + 1) % c.len];
            if a.y <= p.y {
                if b.y > p.y && cross2(sub(b, a), sub(p, a)) > 0.0 {
                    wn += 1;
                }
            } else if b.y <= p.y && cross2(sub(b, a), sub(p, a)) < 0.0 {
                wn -= 1;
            }
        }
    }
    wn
}

/// The unit outward normal of the edge a->b on a contour with `outward`.
fn edge_normal(a: Vec2f, b: Vec2f, outward: f32) -> Vec2f {
    let e = sub(b, a);
    let l = len2(e).max(1e-12);
    v2(e.y / l * outward, -e.x / l * outward)
}

fn mitred_normals(pts: &[Vec2f], contours: &[Contour], edge_out: &[f32]) -> Vec<Vec2f> {
    let mut out = vec![v2(0.0, 0.0); pts.len()];
    for c in contours {
        for k in 0..c.len {
            let ip = c.start + (k + c.len - 1) % c.len;
            let ic = c.start + k;
            let prev = pts[ip];
            let cur = pts[ic];
            let next = pts[c.start + (k + 1) % c.len];
            // An edge buried in an overlap has no outward side; lean on
            // its neighbour, and at a fully buried corner do not inset.
            let (op, on) = (edge_out[ip], edge_out[ic]);
            if op == 0.0 && on == 0.0 {
                continue;
            }
            let n_p = if op != 0.0 { edge_normal(prev, cur, op) } else { edge_normal(cur, next, on) };
            let n_n = if on != 0.0 { edge_normal(cur, next, on) } else { n_p };
            let m = add(n_p, n_n);
            let ml = len2(m);
            let (dir, scale) = if ml < 1e-6 {
                (n_n, 1.0)
            } else {
                let dir = mul(m, 1.0 / ml);
                (dir, (1.0 / dot2(dir, n_n).max(1e-3)).min(MITRE_MAX))
            };
            out[c.start + k] = mul(dir, scale);
        }
    }
    out
}

/// How far each outline point may be inset (in bevel units, em) before
/// its cap ring folds over. A wide bevel on a thin stroke (the bowls of a
/// `B`, a serif, a hairline) would push the inset ring past the middle of
/// the stroke, across the ring coming from the other side, and the front
/// cap would turn inside out there. So cast a ray from the point along its
/// inset direction (−mitre) to the nearest edge not touching the point:
/// that is the local stroke width w, and both sides move in, so the point
/// may travel at most 0.45·w — a bevel of 0.45·w / |mitre|. Points whose
/// inset stays inside that limit (every point, for a bevel narrower than
/// half the thinnest stroke) are unchanged bit for bit.
fn inset_limits(pts: &[Vec2f], contours: &[Contour], mitre: &[Vec2f]) -> Vec<f32> {
    let mut out = vec![f32::MAX; pts.len()];
    for c in contours {
        for k in 0..c.len {
            let i = c.start + k;
            let m = len2(mitre[i]);
            if m < 1e-6 {
                continue;
            }
            let o = pts[i];
            let d = mul(mitre[i], -1.0 / m);
            let mut nearest = f32::MAX;
            for e in contours {
                for q in 0..e.len {
                    let a = e.start + q;
                    let b = e.start + (q + 1) % e.len;
                    if a == i || b == i {
                        continue;
                    }
                    // Ray o + t·d against segment pts[a]..pts[b].
                    let (pa, ab) = (pts[a], sub(pts[b], pts[a]));
                    let den = d.x * ab.y - d.y * ab.x;
                    if den.abs() < 1e-12 {
                        continue;
                    }
                    let ao = sub(pa, o);
                    let t = (ao.x * ab.y - ao.y * ab.x) / den;
                    let u = (ao.x * d.y - ao.y * d.x) / den;
                    if t > 1e-6 && (0.0..=1.0).contains(&u) {
                        nearest = nearest.min(t);
                    }
                }
            }
            if nearest < f32::MAX {
                out[i] = 0.45 * nearest / m;
            }
        }
    }
    // A short edge whose two ends inset toward each other (a small facet
    // on a tight curve, the notch where a bowl meets the stem) collapses
    // and then reverses once the ends pass each other: the inset edge
    // e − (m_b − m_a)·bevel must keep pointing along e, so both ends stop
    // at 0.8 of the bevel where it would turn round.
    for c in contours {
        for k in 0..c.len {
            let a = c.start + k;
            let b = c.start + (k + 1) % c.len;
            let e = sub(pts[b], pts[a]);
            let closing = dot2(sub(mitre[b], mitre[a]), e);
            if closing > 1e-9 {
                let limit = 0.8 * dot2(e, e) / closing;
                out[a] = out[a].min(limit);
                out[b] = out[b].min(limit);
            }
        }
    }
    out
}

/// Caps through the constrained Delaunay triangulation. `Err` carries the
/// (contour, edge) constraints missing from the result, which is then not
/// to be trusted — the caller retries or falls back. `reverse` enforces
/// the constraints in the opposite order (the library's flip loop is
/// order-sensitive). Points are globally welded first: the CDT silently
/// drops an exact duplicate, which would make its constraint unenforceable.
fn cdt_caps(
    pts: &[Vec2f],
    contours: &[Contour],
    min: Vec2f,
    max: Vec2f,
    reverse: bool,
) -> Result<Vec<[u32; 3]>, Vec<(usize, usize)>> {
    let margin = ((max.x - min.x) + (max.y - min.y)).max(1e-3) * 0.05;
    let mut cdt = CDT::new(
        Point2 { x: (min.x - margin) as f64, y: (min.y - margin) as f64 },
        Point2 { x: (max.x + margin) as f64, y: (max.y + margin) as f64 },
    );
    let mut key_to_uniq: HashMap<(i64, i64), u32> = HashMap::new();
    let mut uniq_pts: Vec<Vec2f> = Vec::new();
    let mut uniq_first: Vec<u32> = Vec::new();
    let mut raw_of_uniq: Vec<u32> = Vec::new();
    let mut uniq_of_pt: Vec<u32> = Vec::with_capacity(pts.len());
    let mut raw_base: Option<u32> = None;
    for (i, p) in pts.iter().enumerate() {
        let key = ((p.x * 1e5).round() as i64, (p.y * 1e5).round() as i64);
        let u = match key_to_uniq.get(&key) {
            Some(&u) => u,
            None => {
                let raw = cdt.insert_point(p.x as f64, p.y as f64);
                if raw_base.is_none() {
                    raw_base = Some(raw);
                }
                let u = uniq_pts.len() as u32;
                uniq_pts.push(*p);
                uniq_first.push(i as u32);
                raw_of_uniq.push(raw);
                key_to_uniq.insert(key, u);
                u
            }
        };
        uniq_of_pt.push(u);
    }
    if raw_base.is_none() || uniq_pts.len() < 3 {
        return Ok(Vec::new());
    }
    let mut constraints: Vec<(u32, u32, usize, usize)> = Vec::new();
    for (ci, c) in contours.iter().enumerate() {
        for k in 0..c.len {
            let a = uniq_of_pt[c.start + k];
            let b = uniq_of_pt[c.start + (k + 1) % c.len];
            if a != b {
                constraints.push((a.min(b), a.max(b), ci, k));
            }
        }
    }
    if reverse {
        for (a, b, _, _) in constraints.iter().rev() {
            cdt.add_constraint(raw_of_uniq[*a as usize], raw_of_uniq[*b as usize]);
        }
    } else {
        for (a, b, _, _) in constraints.iter() {
            cdt.add_constraint(raw_of_uniq[*a as usize], raw_of_uniq[*b as usize]);
        }
    }
    cdt.finalize();
    let tris = cdt.get_triangles();
    // The library subtracts its own super vertices, so its indices are our
    // unique indices in insertion order; anything else is a broken result.
    let raw_to_uniq = |r: u32| -> Option<usize> {
        let idx = r as usize;
        (idx < uniq_pts.len()).then_some(idx)
    };
    let mut edges: HashSet<(u32, u32)> = HashSet::with_capacity(tris.len() * 3);
    let mut mapped: Vec<[usize; 3]> = Vec::with_capacity(tris.len());
    for t in &tris {
        let (a, b, c) = match (raw_to_uniq(t[0]), raw_to_uniq(t[1]), raw_to_uniq(t[2])) {
            (Some(a), Some(b), Some(c)) => (a, b, c),
            // An index outside our points: the result is not to be trusted.
            _ => return Ok(Vec::new()),
        };
        for (x, y) in [(a, b), (b, c), (c, a)] {
            let (x, y) = (x as u32, y as u32);
            edges.insert((x.min(y), x.max(y)));
        }
        mapped.push([a, b, c]);
    }
    let missing: Vec<(usize, usize)> = constraints
        .iter()
        .filter(|(a, b, _, _)| !edges.contains(&(*a, *b)))
        .map(|(_, _, ci, k)| (*ci, *k))
        .collect();
    if !missing.is_empty() {
        return Err(missing);
    }
    let mut out = Vec::with_capacity(mapped.len());
    for [a, b, c] in mapped {
        let (pa, pb, pc) = (uniq_pts[a], uniq_pts[b], uniq_pts[c]);
        let area2 = cross2(sub(pb, pa), sub(pc, pa));
        if area2.abs() < 1e-12 {
            continue;
        }
        let centroid = mul(add(add(pa, pb), pc), 1.0 / 3.0);
        if winding_number(pts, contours, centroid) == 0 {
            continue;
        }
        let (a, b, c) = if area2 > 0.0 { (a, b, c) } else { (a, c, b) };
        out.push([uniq_first[a], uniq_first[b], uniq_first[c]]);
    }
    Ok(out)
}

/// The fallback cap: the largest contour ear-clipped on its own (holes are
/// lost, the letter is not). Tolerant of collinear runs; stops rather than
/// spins on a contour it cannot clip.
fn ear_clip_largest(pts: &[Vec2f], contours: &[Contour]) -> Vec<[u32; 3]> {
    let c = match contours.iter().max_by(|a, b| a.area.abs().total_cmp(&b.area.abs())) {
        Some(c) => c,
        None => return Vec::new(),
    };
    let mut idx: Vec<usize> = (0..c.len).collect();
    if c.area < 0.0 {
        idx.reverse();
    }
    let p = |k: usize| pts[c.start + k];
    let mut tris: Vec<[u32; 3]> = Vec::with_capacity(c.len.saturating_sub(2));
    let mut guard = c.len * c.len + 16;
    while idx.len() > 3 && guard > 0 {
        guard -= 1;
        let n = idx.len();
        let mut clipped = false;
        let mut flattest: Option<(usize, f32)> = None;
        for i in 0..n {
            let (a, b, cc) = (idx[(i + n - 1) % n], idx[i], idx[(i + 1) % n]);
            let (pa, pb, pc) = (p(a), p(b), p(cc));
            let cr = cross2(sub(pb, pa), sub(pc, pa));
            if cr.abs() < 1e-12 {
                if flattest.map_or(true, |(_, v)| cr.abs() < v) {
                    flattest = Some((i, cr.abs()));
                }
                continue;
            }
            if cr < 0.0 {
                continue;
            }
            let blocked = idx
                .iter()
                .any(|&k| k != a && k != b && k != cc && point_in_tri(p(k), pa, pb, pc));
            if blocked {
                continue;
            }
            tris.push([(c.start + a) as u32, (c.start + b) as u32, (c.start + cc) as u32]);
            idx.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            match flattest {
                Some((i, _)) => {
                    idx.remove(i);
                }
                None => break,
            }
        }
    }
    if idx.len() == 3 {
        let (a, b, cc) = (idx[0], idx[1], idx[2]);
        if cross2(sub(p(b), p(a)), sub(p(cc), p(a))) > 1e-12 {
            tris.push([(c.start + a) as u32, (c.start + b) as u32, (c.start + cc) as u32]);
        }
    }
    tris
}

fn point_in_tri(p: Vec2f, a: Vec2f, b: Vec2f, c: Vec2f) -> bool {
    let d1 = cross2(sub(b, a), sub(p, a));
    let d2 = cross2(sub(c, b), sub(p, b));
    let d3 = cross2(sub(a, c), sub(p, c));
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

// ---------------------------------------------------------------------------
// Extrusion into the stream.
// ---------------------------------------------------------------------------

/// A rotation of the glyph's local frame: three columns.
#[derive(Clone, Copy)]
struct Frame {
    x: Vec3f,
    y: Vec3f,
    z: Vec3f,
}

impl Frame {
    fn identity() -> Self {
        Self { x: vec3f(1.0, 0.0, 0.0), y: vec3f(0.0, 1.0, 0.0), z: vec3f(0.0, 0.0, 1.0) }
    }
    /// Turn about Y so local +z points to (sin θ, 0, cos θ) and local +x
    /// runs along the tangent of the ring.
    fn rot_y(theta: f32) -> Self {
        let (s, c) = theta.sin_cos();
        Self { x: vec3f(c, 0.0, -s), y: vec3f(0.0, 1.0, 0.0), z: vec3f(s, 0.0, c) }
    }
    /// Turn about Z so local +y points to (−sin α, cos α, 0).
    fn rot_z(alpha: f32) -> Self {
        let (s, c) = alpha.sin_cos();
        Self { x: vec3f(c, s, 0.0), y: vec3f(-s, c, 0.0), z: vec3f(0.0, 0.0, 1.0) }
    }
    /// A letter lying in the XZ plane: letter x -> +X, letter up -> -Z,
    /// cap normal -> +Y.
    fn grid_flat() -> Self {
        Self { x: vec3f(1.0, 0.0, 0.0), y: vec3f(0.0, 0.0, -1.0), z: vec3f(0.0, 1.0, 0.0) }
    }
    /// `self` after `inner`: apply `inner` first, then `self`.
    fn compose(&self, inner: &Frame) -> Self {
        Self { x: self.apply(inner.x), y: self.apply(inner.y), z: self.apply(inner.z) }
    }
    #[inline]
    fn apply(&self, v: Vec3f) -> Vec3f {
        vec3f(
            self.x.x * v.x + self.y.x * v.y + self.z.x * v.z,
            self.x.y * v.x + self.y.y * v.y + self.z.y * v.z,
            self.x.z * v.x + self.y.z * v.y + self.z.z * v.z,
        )
    }
}

/// The per-copy channels of one glyph as it is pushed.
struct Attrs {
    a_id: f32,
    uv: Vec2f,
    frame: Frame,
    r0: f32,
    r1: f32,
}

/// Push one glyph solid. `bevel` is in world units and already clamped.
fn emit_glyph(mesh: &mut FxMesh, g: &Placed, at: &Attrs, depth: f32, bevel: f32, profile: &Profile) {
    let shape = &g.shape;
    let s = g.em_to_world;
    let c = mul(add(shape.min, shape.max), 0.5);
    let frame = at.frame;
    let (a_id, uv, r0, r1) = (at.a_id, at.uv, at.r0, at.r1);
    let local = |p: Vec2f, z: f32| frame.apply(vec3f((p.x - c.x) * s, (p.y - c.y) * s, z));
    let nrm = |n: Vec3f| frame.apply(n);
    let bevel_em = if s > 0.0 { bevel / s } else { 0.0 };
    // Outline point i inset by the fraction u of the bevel (u 0 = the
    // outline itself, u 1 = the cap ring).
    // Each point's own bevel: clamped where the stroke is too thin for it
    // (`inset_limits`), then halved around any cap triangle the inset would
    // still turn over (a triangle spanning a stroke whose far point moves
    // past its opposite edge), until every cap triangle keeps its winding.
    // Nothing changes for a glyph whose inset folds nowhere.
    let mut reach: Vec<f32> = shape.inset_max.iter().map(|m| bevel_em.min(*m)).collect();
    if bevel_em > 0.0 {
        let at = |reach: &[f32], i: usize| sub(shape.pts[i], mul(shape.mitre[i], reach[i]));
        for _ in 0..8 {
            let mut folded = false;
            for t in &shape.tris {
                let (a, b, c) = (at(&reach, t[0] as usize), at(&reach, t[1] as usize), at(&reach, t[2] as usize));
                let (p, q, r) = (shape.pts[t[0] as usize], shape.pts[t[1] as usize], shape.pts[t[2] as usize]);
                let rest = (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x);
                let now = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
                if rest > 0.0 && now <= 0.0 {
                    for &i in t {
                        reach[i as usize] *= 0.5;
                    }
                    folded = true;
                }
            }
            if !folded {
                break;
            }
        }
    }
    let inset = |i: usize, u: f32| {
        if u <= 0.0 {
            shape.pts[i]
        } else {
            sub(shape.pts[i], mul(shape.mitre[i], u * reach[i]))
        }
    };
    let cap_pt = |i: usize| if bevel > 0.0 { inset(i, 1.0) } else { shape.pts[i] };
    let front_z = if depth > 0.0 { 0.5 * depth } else { 0.0 };
    let back_z = -front_z;

    // Front cap.
    let n_front = nrm(vec3f(0.0, 0.0, 1.0));
    let base = mesh.vertex_count() as u32;
    for i in 0..shape.pts.len() {
        mesh.push_vert(local(cap_pt(i), front_z), a_id, n_front, FACE_FRONT, uv, r0, r1);
    }
    for t in &shape.tris {
        mesh.push_tri(base + t[0], base + t[1], base + t[2]);
    }
    if depth <= 0.0 {
        return;
    }
    // Back cap, reversed.
    let n_back = nrm(vec3f(0.0, 0.0, -1.0));
    let base = mesh.vertex_count() as u32;
    for i in 0..shape.pts.len() {
        mesh.push_vert(local(cap_pt(i), back_z), a_id, n_back, FACE_BACK, uv, r0, r1);
    }
    for t in &shape.tris {
        mesh.push_tri(base + t[0], base + t[2], base + t[1]);
    }
    // Walls and bevel bands, one flat quad per contour edge and segment.
    let z0 = back_z + bevel;
    let z1 = front_z - bevel;
    // The height of profile rise v on the front band and on its mirror on
    // the back; the cap ring lands exactly on the cap plane.
    let front_band_z = |v: f32| {
        if v >= 1.0 {
            front_z
        } else if v <= 0.0 {
            z1
        } else {
            z1 + v * bevel
        }
    };
    let back_band_z = |v: f32| {
        if v >= 1.0 {
            back_z
        } else if v <= 0.0 {
            z0
        } else {
            z0 - v * bevel
        }
    };
    for cont in &shape.contours {
        for k in 0..cont.len {
            let i = cont.start + k;
            let j = cont.start + (k + 1) % cont.len;
            let outward = shape.edge_out[i];
            if outward == 0.0 {
                continue;
            }
            let (pi, pj) = (shape.pts[i], shape.pts[j]);
            let n2 = edge_normal(pi, pj, outward);
            let n = nrm(vec3f(n2.x, n2.y, 0.0));
            let a = mesh.push_vert(local(pi, z0), a_id, n, FACE_SIDE, uv, r0, r1);
            let b = mesh.push_vert(local(pj, z0), a_id, n, FACE_SIDE, uv, r0, r1);
            let cc = mesh.push_vert(local(pj, z1), a_id, n, FACE_SIDE, uv, r0, r1);
            let d = mesh.push_vert(local(pi, z1), a_id, n, FACE_SIDE, uv, r0, r1);
            push_outward_quad(mesh, outward, a, b, cc, d);
            if bevel <= 0.0 {
                continue;
            }
            for seg in 0..profile.segments() {
                let ((u0, v0), (u1, v1)) = (profile.pts[seg], profile.pts[seg + 1]);
                let (m0, m1) = profile.nrm[seg];
                let n0 = nrm(vec3f(n2.x * m0.0, n2.y * m0.0, m0.1));
                let n1 = nrm(vec3f(n2.x * m1.0, n2.y * m1.0, m1.1));
                let (za, zb) = (front_band_z(v0), front_band_z(v1));
                let a = mesh.push_vert(local(inset(i, u0), za), a_id, n0, FACE_BEVEL, uv, r0, r1);
                let b = mesh.push_vert(local(inset(j, u0), za), a_id, n0, FACE_BEVEL, uv, r0, r1);
                let cc = mesh.push_vert(local(inset(j, u1), zb), a_id, n1, FACE_BEVEL, uv, r0, r1);
                let d = mesh.push_vert(local(inset(i, u1), zb), a_id, n1, FACE_BEVEL, uv, r0, r1);
                push_outward_quad(mesh, outward, a, b, cc, d);
            }
            for seg in 0..profile.segments() {
                let ((u0, v0), (u1, v1)) = (profile.pts[seg], profile.pts[seg + 1]);
                let (m0, m1) = profile.nrm[seg];
                let n0 = nrm(vec3f(n2.x * m0.0, n2.y * m0.0, -m0.1));
                let n1 = nrm(vec3f(n2.x * m1.0, n2.y * m1.0, -m1.1));
                let (za, zb) = (back_band_z(v0), back_band_z(v1));
                let a = mesh.push_vert(local(inset(i, u1), zb), a_id, n1, FACE_BEVEL, uv, r0, r1);
                let b = mesh.push_vert(local(inset(j, u1), zb), a_id, n1, FACE_BEVEL, uv, r0, r1);
                let cc = mesh.push_vert(local(inset(j, u0), za), a_id, n0, FACE_BEVEL, uv, r0, r1);
                let d = mesh.push_vert(local(inset(i, u0), za), a_id, n0, FACE_BEVEL, uv, r0, r1);
                push_outward_quad(mesh, outward, a, b, cc, d);
            }
        }
    }
}

/// The quad (a, b, c, d) as laid out faces the edge's RIGHT-hand normal;
/// flip it when the contour's outward side is the left one.
#[inline]
fn push_outward_quad(mesh: &mut FxMesh, outward: f32, a: u32, b: u32, c: u32, d: u32) {
    if outward > 0.0 {
        mesh.push_quad(a, b, c, d);
    } else {
        mesh.push_quad(a, d, c, b);
    }
}

// ---------------------------------------------------------------------------
// Bevel profiles.
// ---------------------------------------------------------------------------

/// The bevel band's cross section: ring points (u = inset fraction, v =
/// rise fraction) from the outline (0, 0) to the cap ring (1, 1), and per
/// segment the unit profile normal (along the 2D outward normal, along z)
/// at its lower and upper ring.
struct Profile {
    pts: Vec<(f32, f32)>,
    nrm: Vec<((f32, f32), (f32, f32))>,
}

impl Profile {
    fn new(kind: BevelType, rings: usize) -> Self {
        let rings = rings.clamp(1, 8);
        let quarter = std::f32::consts::FRAC_PI_2;
        match kind {
            BevelType::Chamfer => {
                // The classic chamfer's exact normal, so one ring is today's
                // geometry bit for bit.
                let r = std::f32::consts::FRAC_1_SQRT_2;
                let pts = (0..=rings).map(|k| {
                    let t = k as f32 / rings as f32;
                    (t, t)
                });
                Self { pts: pts.collect(), nrm: vec![((r, r), (r, r)); rings] }
            }
            BevelType::Round => Self::smooth(rings, |t| {
                let th = t * quarter;
                ((1.0 - th.cos(), th.sin()), (th.cos(), th.sin()))
            }),
            BevelType::Cove => Self::smooth(rings, |t| {
                let th = t * quarter;
                ((th.sin(), 1.0 - th.cos()), (th.sin(), th.cos()))
            }),
            BevelType::Ogee => {
                let m = (rings.max(2) + 1) / 2 * 2;
                Self::smooth(m, |t| {
                    if t <= 0.5 {
                        let th = t * std::f32::consts::PI;
                        ((0.5 * th.sin(), 0.5 * (1.0 - th.cos())), (th.sin(), th.cos()))
                    } else {
                        let th = (t - 0.5) * std::f32::consts::PI;
                        ((0.5 + 0.5 * (1.0 - th.cos()), 0.5 + 0.5 * th.sin()), (th.cos(), th.sin()))
                    }
                })
            }
            BevelType::Step => {
                let steps = rings.max(2);
                let mut pts = vec![(0.0f32, 0.0f32)];
                for s in 0..steps {
                    let (a, b) = (s as f32 / steps as f32, (s + 1) as f32 / steps as f32);
                    pts.push((a, b));
                    pts.push((b, b));
                }
                let nrm = pts
                    .windows(2)
                    .map(|w| {
                        let (du, dv) = (w[1].0 - w[0].0, w[1].1 - w[0].1);
                        let l = (du * du + dv * dv).sqrt().max(1e-12);
                        let n = (dv / l, du / l);
                        (n, n)
                    })
                    .collect();
                Self { pts, nrm }
            }
        }
    }

    /// A smooth profile sampled at `m` segments: `f(t)` gives the point and
    /// its analytic unit normal; the end rings are pinned exactly.
    fn smooth(m: usize, f: impl Fn(f32) -> ((f32, f32), (f32, f32))) -> Self {
        let mut pts = Vec::with_capacity(m + 1);
        let mut ns = Vec::with_capacity(m + 1);
        for k in 0..=m {
            let (p, n) = f(k as f32 / m as f32);
            pts.push(p);
            ns.push(n);
        }
        pts[0] = (0.0, 0.0);
        pts[m] = (1.0, 1.0);
        let nrm = (0..m).map(|k| (ns[k], ns[k + 1])).collect();
        Self { pts, nrm }
    }

    fn segments(&self) -> usize {
        self.nrm.len()
    }
}

// ---------------------------------------------------------------------------
// The floor: mirrored letters and the plane.
// ---------------------------------------------------------------------------

/// The floor grid that fits a quarter of the budget, reported when reduced.
fn fit_floor_grid(grid: usize, budget: usize, warnings: &mut Vec<String>) -> usize {
    let max_grid = ((((budget / 4).max(4)) as f32).sqrt() as usize).saturating_sub(1).max(1);
    if grid > max_grid {
        warnings.push(format!("budget: floor grid {grid} reduced to {max_grid}"));
        max_grid
    } else {
        grid
    }
}

fn floor_vertex_count(grid: usize) -> usize {
    (grid + 1) * (grid + 1)
}

/// Append a copy of every vertex and triangle in the mesh as its mirror
/// image: class 7 + the original class / 16, triangles rewound (the shader
/// reflects the placed position about the floor, which flips handedness).
fn push_mirrors(mesh: &mut FxMesh) {
    let n = mesh.verts.len();
    let base = mesh.vertex_count() as u32;
    let il = mesh.idx.len();
    mesh.verts.extend_from_within(..n);
    for v in mesh.verts[n..].chunks_mut(VERT_FLOATS) {
        v[7] = FACE_MIRROR + v[7] / MIRROR_CLASS_SCALE;
    }
    mesh.idx.reserve(il);
    for t in 0..il / 3 {
        let (a, b, c) = (mesh.idx[3 * t], mesh.idx[3 * t + 1], mesh.idx[3 * t + 2]);
        mesh.push_tri(base + a, base + c, base + b);
    }
}

/// Put the full-frame backdrop quad FIRST in the stream (module docs, "The
/// backdrop"): every index already there moves up by four.
pub fn prepend_backdrop(mesh: &mut FxMesh) {
    let n = vec3f(0.0, 0.0, 1.0);
    let mut quad = FxMesh::default();
    for (x, y, u, v) in [(-1.0, -1.0, 0.0, 1.0), (1.0, -1.0, 1.0, 1.0), (1.0, 1.0, 1.0, 0.0), (-1.0, 1.0, 0.0, 0.0)] {
        quad.push_vert(vec3f(x, y, 0.0), BACKDROP_ID, n, FACE_BACKDROP, v2(u, v), 0.0, 0.0);
    }
    quad.push_quad(0, 1, 2, 3);
    for i in mesh.idx.iter_mut() {
        *i += 4;
    }
    mesh.verts.splice(0..0, quad.verts.iter().copied());
    mesh.idx.splice(0..0, quad.idx.iter().copied());
}

/// Append the subdivided floor plane at `y`, `size` wide, facing +Y.
fn push_floor(mesh: &mut FxMesh, y: f32, size: f32, grid: usize) {
    let grid = grid.max(1);
    let n = grid + 1;
    let base = mesh.vertex_count() as u32;
    let up = vec3f(0.0, 1.0, 0.0);
    for j in 0..n {
        let z = -0.5 * size + size * j as f32 / grid as f32;
        for i in 0..n {
            let x = -0.5 * size + size * i as f32 / grid as f32;
            mesh.push_vert(vec3f(x, y, z), FLOOR_ID, up, FACE_FLOOR, v2(x, z), 0.0, 0.0);
        }
    }
    for j in 0..grid {
        for i in 0..grid {
            let a = base + (j * n + i) as u32;
            let b = a + 1;
            let d = a + n as u32;
            let c = d + 1;
            mesh.push_quad(a, d, c, b);
        }
    }
}

// ---------------------------------------------------------------------------
// Voxel letters and the voxel morph.
// ---------------------------------------------------------------------------

/// One lattice cube at rest.
struct Cube {
    centre: [f32; 3],
    letter: bool,
    layer: u32,
    /// The glyph it samples (waste: the nearest glyph).
    glyph: usize,
}

/// One cube as pushed: where it travels from and to, and its class.
struct VoxelEmit {
    from: [f32; 3],
    to: [f32; 3],
    layer: u32,
    glyph: Option<usize>,
    class: f32,
}

struct VoxelPlan {
    emits: Vec<VoxelEmit>,
    born: usize,
    dying: usize,
}

/// The six faces of a cube: (normal, u, v) with u x v = normal, so the
/// corners (-u-v, +u-v, +u+v, -u+v) run counter-clockwise seen from outside.
const CUBE_FACES: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
    ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
    ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
    ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
    ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ([0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
];
const QUAD_CORNERS: [(f32, f32); 4] = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];

/// The voxel layout (module docs, "Voxel letters" and "Voxel morph").
fn build_voxel_mesh(
    laidout: &LaidoutText,
    sane: &Sane,
    world_per_lpx: f32,
    previous: Option<&VoxelSet>,
    mesh: &mut FxMesh,
) -> TextMeshReport {
    let mut warnings = Vec::new();
    let mut cache = ShapeCache::default();
    let mut words = 0usize;
    let mut glyphs = collect_glyphs(laidout, world_per_lpx, sane.detail, sane.tracking, &mut cache, &mut words);
    let mut scene = Scene {
        words,
        lines: laidout.rows.len(),
        layout: TextLayout::Voxel,
        ..Scene::default()
    };
    centre_block(&mut glyphs, &mut scene);
    let layers = sane.voxel_layers;
    let flat = sane.depth <= 0.0 && layers == 1;
    let per_cube = if flat { 4 } else { 24 } * sane.repeat();
    let floor_grid = sane.floor.map(|f| fit_floor_grid(f.grid, sane.budget, &mut warnings));
    let floor_verts = floor_grid.map(floor_vertex_count).unwrap_or(0);
    let room = sane.budget.saturating_sub(floor_verts);
    let previous = previous.filter(|p| !p.is_empty() && p.is_sound());
    // Without waste the morph pairs letters only: an old set's waste cubes
    // (built with waste) are left out rather than shrunk away.
    let letters_only = match previous {
        Some(p) if !sane.voxel_waste && p.letter.iter().any(|l| !l) => Some(p.letters_only()),
        _ => None,
    };
    let previous = match &letters_only {
        Some(p) if p.is_empty() => None,
        Some(p) => Some(p),
        None => previous,
    };
    let lattice = |res: f32| -> (f32, usize, usize) {
        let cell = sane.size / res;
        if glyphs.is_empty() {
            return (cell, 0, 0);
        }
        let count = |extent: f32| ((extent / cell).ceil().clamp(1.0, 1e6)) as usize;
        (cell, count(scene.width), count(scene.height))
    };
    let mut res = sane.voxel_res;
    // Coarsen on the plain lattice count first (no sampling needed), then
    // on the morph plan, which can only be larger.
    let (cell, nx, ny, half, cubes, plan) = loop {
        let (cell, nx, ny) = lattice(res);
        // Without waste the letters are a fraction of the block, so the
        // block count is no lower bound on them: the plan decides alone.
        let plain = if sane.voxel_waste { nx * ny * layers * per_cube } else { 0 };
        if plain > room && res > 1.0 {
            res *= 0.75;
            warnings.push(format!(
                "budget: {} vertices over {}, voxel_res coarsened to {res:.2}",
                plain + floor_verts,
                sane.budget
            ));
            continue;
        }
        let dz = if flat {
            0.0
        } else if sane.depth > 0.0 {
            sane.depth / layers as f32
        } else {
            cell
        };
        let half = [0.5 * cell, 0.5 * cell, 0.5 * dz];
        let cubes = voxelise(&glyphs, cell, nx, ny, layers, half[2], sane.voxel_waste);
        let plan = pair_voxels(&cubes, previous);
        let cost = plan.emits.len() * per_cube;
        if cost > room && res > 1.0 {
            res *= 0.75;
            warnings.push(format!(
                "budget: {} vertices over {}, voxel_res coarsened to {res:.2}",
                cost + floor_verts,
                sane.budget
            ));
            continue;
        }
        break (cell, nx, ny, half, cubes, plan);
    };
    let keep = (room / per_cube.max(1)).min(plan.emits.len());
    if keep < plan.emits.len() {
        warnings.push(format!(
            "budget: {} vertices over {}, {} of {} voxels dropped",
            plan.emits.len() * per_cube + floor_verts,
            sane.budget,
            plan.emits.len() - keep,
            plan.emits.len()
        ));
    }

    mesh.clear();
    let flags = glyph_flags(&glyphs, sane);
    for copy in 0..sane.copies {
        let r1_copy = COPY_STRIDE * copy as f32;
        for e in &plan.emits[..keep] {
            let (a_id, r0) = match e.glyph {
                Some(k) if k < glyphs.len() => (k as f32, word_channel(glyphs[k].word, k, &flags)),
                _ => (0.0, 0.0),
            };
            emit_cube(mesh, half, flat, e.from, v2(e.to[0], e.to[1]), a_id, e.class, r0, e.layer as f32 + r1_copy);
        }
    }
    let (width, height) = (nx as f32 * cell, ny as f32 * cell);
    let (mut floor_y, mut floor_size) = (0.0, 0.0);
    if let (Some(f), Some(grid)) = (sane.floor, floor_grid) {
        let ground = if glyphs.is_empty() { -0.5 * sane.size } else { -0.5 * height };
        floor_y = f.y.unwrap_or(ground - 0.05 * sane.size);
        floor_size = f.size.unwrap_or(3.0 * width.max(sane.size));
        push_mirrors(mesh);
        push_floor(mesh, floor_y, floor_size, grid);
    }
    let set = VoxelSet {
        centres: cubes.iter().map(|c| c.centre).collect(),
        scales: vec![half; cubes.len()],
        letter: cubes.iter().map(|c| c.letter).collect(),
        layers: cubes.iter().map(|c| c.layer).collect(),
    };
    TextMeshReport {
        glyphs: glyphs.len(),
        words: scene.words,
        lines: scene.lines,
        width,
        height,
        vertices: mesh.vertex_count(),
        triangles: mesh.triangle_count(),
        fallbacks: glyphs.iter().filter(|g| g.shape.fallback).count(),
        warnings,
        copies: sane.copies,
        copy_pitch: if sane.copies_turn { sane.tunnel_pitch } else { 0.0 },
        floor_y,
        floor_size,
        floor_grid: floor_grid.unwrap_or(0),
        voxel_res: res,
        voxel_layers: layers,
        voxel_half: half,
        voxel_cubes: keep,
        morph_born: plan.born,
        morph_dying: plan.dying,
        voxels: Some(set),
        glyph_keys: glyphs.iter().map(|g| g.key).collect(),
        changed: flags,
        ..TextMeshReport::default()
    }
}

/// Sample the block on the lattice: letter cubes first (row by row from
/// the top, left to right, layers innermost), then the waste cubes (none
/// when `with_waste` is off).
fn voxelise(glyphs: &[Placed], cell: f32, nx: usize, ny: usize, layers: usize, half_z: f32, with_waste: bool) -> Vec<Cube> {
    let x0 = -0.5 * nx as f32 * cell;
    let y0 = -0.5 * ny as f32 * cell;
    let mut letters = Vec::new();
    let mut waste = Vec::new();
    for iy in (0..ny).rev() {
        let y = y0 + (iy as f32 + 0.5) * cell;
        for ix in 0..nx {
            let x = x0 + (ix as f32 + 0.5) * cell;
            let p = v2(x, y);
            let (letter, glyph) = match glyphs.iter().position(|g| glyph_covers(g, p)) {
                Some(k) => (true, k),
                None if with_waste => (false, nearest_glyph(glyphs, p)),
                None => continue,
            };
            let out = if letter { &mut letters } else { &mut waste };
            for layer in 0..layers {
                let z = half_z * (layers as f32 - 1.0 - 2.0 * layer as f32);
                out.push(Cube { centre: [x, y, z], letter, layer: layer as u32, glyph });
            }
        }
    }
    letters.append(&mut waste);
    letters
}

/// The nonzero winding test of a text-space point against a glyph's cap.
fn glyph_covers(g: &Placed, p: Vec2f) -> bool {
    if g.em_to_world <= 0.0 || (p.x - g.rest.x).abs() > g.half.x || (p.y - g.rest.y).abs() > g.half.y {
        return false;
    }
    let c = mul(add(g.shape.min, g.shape.max), 0.5);
    let em = add(c, mul(sub(p, g.rest), 1.0 / g.em_to_world));
    winding_number(&g.shape.pts, &g.shape.contours, em) != 0
}

/// The glyph whose ink box is nearest to `p` (the lowest ordinal on a tie).
fn nearest_glyph(glyphs: &[Placed], p: Vec2f) -> usize {
    let mut best = (f32::MAX, 0usize);
    for (k, g) in glyphs.iter().enumerate() {
        let dx = ((p.x - g.rest.x).abs() - g.half.x).max(0.0);
        let dy = ((p.y - g.rest.y).abs() - g.half.y).max(0.0);
        let d = dx * dx + dy * dy;
        if d < best.0 {
            best = (d, k);
        }
    }
    best.1
}

/// The spatial rank key of every cube of a set: (x normalised over the
/// set's x range, y, layer).
fn rank_keys(cubes: &[([f32; 3], u32)]) -> Vec<(f32, f32, u32)> {
    let (lo, hi) = cubes.iter().fold((f32::MAX, f32::MIN), |a, (c, _)| (a.0.min(c[0]), a.1.max(c[0])));
    let span = (hi - lo).max(1e-6);
    cubes.iter().map(|(c, l)| ((c[0] - lo) / span, c[1], *l)).collect()
}

fn sort_by_rank(idx: &mut [usize], keys: &[(f32, f32, u32)]) {
    idx.sort_by(|&a, &b| {
        keys[a]
            .0
            .total_cmp(&keys[b].0)
            .then(keys[a].1.total_cmp(&keys[b].1))
            .then(keys[a].2.cmp(&keys[b].2))
            .then(a.cmp(&b))
    });
}

/// Pair this build's cubes with the previous build's (module docs, "Voxel
/// morph"). Without a previous set every cube travels from where it rests.
fn pair_voxels(cubes: &[Cube], previous: Option<&VoxelSet>) -> VoxelPlan {
    let class_of = |letter: bool| if letter { FACE_VOXEL_LETTER } else { FACE_VOXEL_WASTE };
    let waste_flag = |letter: bool| if letter { 0.0 } else { FACE_WASTE_FLAG };
    let mut emits: Vec<VoxelEmit> = cubes
        .iter()
        .map(|c| VoxelEmit { from: c.centre, to: c.centre, layer: c.layer, glyph: Some(c.glyph), class: class_of(c.letter) })
        .collect();
    let old = match previous {
        Some(p) if !p.is_empty() => p,
        _ => return VoxelPlan { emits, born: 0, dying: 0 },
    };
    let new_keys = rank_keys(&cubes.iter().map(|c| (c.centre, c.layer)).collect::<Vec<_>>());
    let old_keys = rank_keys(&old.centres.iter().copied().zip(old.layers.iter().copied()).collect::<Vec<_>>());
    let all_new: Vec<usize> = (0..cubes.len()).collect();
    let (mut born, mut dying) = (0usize, 0usize);
    let mut tail = Vec::new();
    for letter in [true, false] {
        let mut new_c: Vec<usize> = (0..cubes.len()).filter(|&i| cubes[i].letter == letter).collect();
        let mut old_c: Vec<usize> = (0..old.len()).filter(|&i| old.letter[i] == letter).collect();
        let own = !old_c.is_empty();
        if !own {
            old_c = (0..old.len()).collect();
        }
        sort_by_rank(&mut new_c, &new_keys);
        sort_by_rank(&mut old_c, &old_keys);
        for (rank, &ni) in new_c.iter().enumerate() {
            let e = &mut emits[ni];
            e.from = old.centres[old_c[rank % old_c.len()]];
            if rank >= old_c.len() {
                e.class = FACE_VOXEL_BORN + waste_flag(letter);
                born += 1;
            }
        }
        if own && old_c.len() > new_c.len() {
            let targets: &[usize] = if new_c.is_empty() { &all_new } else { &new_c };
            for &oi in &old_c[new_c.len()..] {
                let from = old.centres[oi];
                let mut best: Option<(f32, usize)> = None;
                for &t in targets {
                    let c = cubes[t].centre;
                    let d = (c[0] - from[0]).powi(2) + (c[1] - from[1]).powi(2) + (c[2] - from[2]).powi(2);
                    if best.map_or(true, |(bd, _)| d < bd) {
                        best = Some((d, t));
                    }
                }
                let (to, layer, glyph) = match best {
                    Some((_, t)) => (cubes[t].centre, cubes[t].layer, Some(cubes[t].glyph)),
                    None => (from, old.layers[oi], None),
                };
                tail.push(VoxelEmit { from, to, layer, glyph, class: FACE_VOXEL_DYING + waste_flag(letter) });
                dying += 1;
            }
        }
    }
    emits.append(&mut tail);
    VoxelPlan { emits, born, dying }
}

/// Push one cube: 24 vertices (flat: one +z quad), `geom_pos` about the
/// cube's centre, `normal` = the FROM rest centre, `uv` = the TO centre.
#[allow(clippy::too_many_arguments)]
fn emit_cube(
    mesh: &mut FxMesh,
    half: [f32; 3],
    flat: bool,
    from: [f32; 3],
    uv: Vec2f,
    a_id: f32,
    class: f32,
    r0: f32,
    r1: f32,
) {
    let from = vec3f(from[0], from[1], from[2]);
    if flat {
        let base = mesh.vertex_count() as u32;
        for (su, sv) in QUAD_CORNERS {
            mesh.push_vert(vec3f(su * half[0], sv * half[1], 0.0), a_id, from, class, uv, r0, r1);
        }
        mesh.push_quad(base, base + 1, base + 2, base + 3);
        return;
    }
    let k = 1.0 - VOXEL_FACE_INSET;
    for (n, u, v) in CUBE_FACES.iter() {
        let base = mesh.vertex_count() as u32;
        for (su, sv) in QUAD_CORNERS {
            let p = |a: usize| (n[a] + (su * u[a] + sv * v[a]) * k) * half[a];
            mesh.push_vert(vec3f(p(0), p(1), p(2)), a_id, from, class, uv, r0, r1);
        }
        mesh.push_quad(base, base + 1, base + 2, base + 3);
    }
}

// ---------------------------------------------------------------------------
// Tiny 2D helpers (kept explicit so the maths above reads as maths).
// ---------------------------------------------------------------------------

#[inline]
fn v2(x: f32, y: f32) -> Vec2f {
    vec2f(x, y)
}
#[inline]
fn add(a: Vec2f, b: Vec2f) -> Vec2f {
    vec2f(a.x + b.x, a.y + b.y)
}
#[inline]
fn sub(a: Vec2f, b: Vec2f) -> Vec2f {
    vec2f(a.x - b.x, a.y - b.y)
}
#[inline]
fn mul(a: Vec2f, s: f32) -> Vec2f {
    vec2f(a.x * s, a.y * s)
}
#[inline]
fn dot2(a: Vec2f, b: Vec2f) -> f32 {
    a.x * b.x + a.y * b.y
}
#[inline]
fn cross2(a: Vec2f, b: Vec2f) -> f32 {
    a.x * b.y - a.y * b.x
}
#[inline]
fn len2(a: Vec2f) -> f32 {
    (a.x * a.x + a.y * a.y).sqrt()
}
#[inline]
fn dist2(a: Vec2f, b: Vec2f) -> f32 {
    let d = sub(a, b);
    d.x * d.x + d.y * d.y
}

// ---------------------------------------------------------------------------
// Tests: the real bundled fonts through the real layouter, no Cx.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use makepad_geom3d::fx_mesh::VERT_FLOATS;
    use super::*;
    use makepad_draw::text::font::FontId;
    use makepad_draw::text::font_family::FontFamilyId;
    use makepad_draw::text::layouter::{BorrowedLayoutParams, LayoutOptions, Layouter, Settings, Style};
    use makepad_draw::text::loader::{FontDefinition, FontFamilyDefinition};
    use makepad_draw::SharedBytes;
    use std::path::PathBuf;

    const FONTS: [&str; 4] = [
        "IBMPlexSans-Text.ttf",
        "IBMPlexSans-SemiBold.ttf",
        "IBMPlexSans-Italic.ttf",
        "LiberationMono-Regular.ttf",
    ];
    /// 100 lpx reference size, in points.
    const REF_PTS: f32 = 100.0 * 72.0 / 96.0;

    fn layouter() -> (Layouter, Vec<FontFamilyId>) {
        let mut layouter = Layouter::new(Settings::default());
        let resources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../widgets/resources");
        let mut families = Vec::new();
        for (k, file) in FONTS.iter().enumerate() {
            let bytes = std::fs::read(resources.join(file)).expect("bundled font bytes should load");
            let font_id: FontId = (0xE222_0001_u64 + k as u64).into();
            let family_id: FontFamilyId = (0xE222_0101_u64 + k as u64).into();
            layouter.define_font(
                font_id,
                FontDefinition {
                    data: SharedBytes::from_vec(bytes),
                    index: 0,
                    ascender_fudge_in_ems: 0.0,
                    descender_fudge_in_ems: 0.0,
                    weight: None,
                    variations: Vec::new(),
                },
            );
            layouter.define_font_family(
                family_id,
                FontFamilyDefinition {
                    font_ids: vec![font_id],
                    expected_member_count: 1,
                    diagnostics: Default::default(),
                },
            );
            families.push(family_id);
        }
        (layouter, families)
    }

    fn layout(l: &mut Layouter, family: FontFamilyId, text: &str, wrap: Option<f32>) -> Rc<LaidoutText> {
        l.get_or_layout(BorrowedLayoutParams {
            text,
            style: Style { font_family_id: family, font_size_in_pts: REF_PTS, color: None },
            options: LayoutOptions {
                max_width_in_lpxs: wrap,
                wrap: wrap.is_some(),
                align: 0.5,
                ..LayoutOptions::default()
            },
        })
    }

    #[derive(Clone, Copy)]
    struct V {
        pos: Vec3f,
        id: f32,
        n: Vec3f,
        aux: f32,
        uv: Vec2f,
        r0: f32,
        r1: f32,
    }

    fn verts(mesh: &FxMesh) -> Vec<V> {
        mesh.verts
            .chunks(VERT_FLOATS)
            .map(|v| V {
                pos: vec3f(v[0], v[1], v[2]),
                id: v[3],
                n: vec3f(v[4], v[5], v[6]),
                aux: v[7],
                uv: vec2f(v[8], v[9]),
                r0: v[10],
                r1: v[11],
            })
            .collect()
    }

    fn printable_ascii() -> String {
        (0x21u8..=0x7E).map(|b| b as char).collect()
    }

    /// The reference cap areas of a one-glyph layout, world²: the signed
    /// shoelace sum of its winding-classified contours (outer minus holes,
    /// exact when no contours overlap) and a 2000-row scanline integral of
    /// the nonzero-winding region (right even when they do), plus the
    /// intersection count that says which one applies.
    fn reference_areas(text: &LaidoutText, params: &TextMeshParams) -> (f32, f32, usize) {
        let row = &text.rows[0];
        let glyph = &row.glyphs[0];
        let mut cache = ShapeCache::default();
        let shape = glyph_shape(glyph, params.detail, &mut cache).expect("glyph has ink");
        let em_to_world = glyph.font_size_in_lpxs * params.size / cap_height_lpx(text);
        let k = em_to_world * em_to_world;
        let shoelace: f32 = shape.contours.iter().map(|c| c.area).sum();
        (shoelace.abs() * k, scanline_area(&shape) * k, shape.intersections)
    }

    fn scanline_area(shape: &GlyphShape) -> f32 {
        let rows = 2000;
        let dy = (shape.max.y - shape.min.y) / rows as f32;
        let mut area = 0.0f32;
        let mut xs: Vec<(f32, i32)> = Vec::new();
        for r in 0..rows {
            let y = shape.min.y + (r as f32 + 0.5) * dy;
            xs.clear();
            for c in &shape.contours {
                for k in 0..c.len {
                    let a = shape.pts[c.start + k];
                    let b = shape.pts[c.start + (k + 1) % c.len];
                    if (a.y <= y) != (b.y <= y) {
                        let t = (y - a.y) / (b.y - a.y);
                        xs.push((a.x + t * (b.x - a.x), if b.y > a.y { 1 } else { -1 }));
                    }
                }
            }
            xs.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut wn = 0;
            let mut prev = 0.0f32;
            for (x, d) in &xs {
                if wn != 0 {
                    area += (x - prev) * dy;
                }
                wn += d;
                prev = *x;
            }
        }
        area
    }

    fn cap_area(mesh: &FxMesh) -> f32 {
        let vs = verts(mesh);
        let mut area = 0.0f32;
        for t in mesh.idx.chunks(3) {
            let (a, b, c) = (vs[t[0] as usize], vs[t[1] as usize], vs[t[2] as usize]);
            if a.aux != FACE_FRONT || b.aux != FACE_FRONT || c.aux != FACE_FRONT {
                continue;
            }
            let e1 = vec2f(b.pos.x - a.pos.x, b.pos.y - a.pos.y);
            let e2 = vec2f(c.pos.x - a.pos.x, c.pos.y - a.pos.y);
            area += 0.5 * cross2(e1, e2);
        }
        area
    }

    fn fingerprint(mesh: &FxMesh) -> (usize, usize, u64) {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for f in &mesh.verts {
            h = (h ^ f.to_bits() as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
        for i in &mesh.idx {
            h = (h ^ *i as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
        (mesh.vertex_count(), mesh.idx.len(), h)
    }

    #[test]
    fn todays_geometry_is_pinned_bit_for_bit() {
        // Captured from the builder before bevel profiles, copies, the floor
        // and the voxels existed: with the defaults (chamfer, one ring, one
        // copy, no floor) every float and index must come back unchanged
        // (the bevelled cases re-pinned for the thin-stroke clamp).
        let (mut l, families) = layouter();
        let a = layout(&mut l, families[0], "OB8g@&", None);
        let c = layout(&mut l, families[1], "Kinetic", None);
        let d = layout(&mut l, families[3], "the quick brown fox jumps over", Some(500.0));
        let cases: Vec<(&str, &LaidoutText, TextMeshParams, (usize, usize, u64))> = vec![
            ("plain", &a, TextMeshParams::default(), (3720, 7488, 17580915930908050101)),
            // Re-pinned (T3, 2026-09-29) with the thin-stroke bevel clamp
            // (`inset_limits`, the fold guard in `emit_glyph`): the caps of
            // these texts turned inside out in places; the counts are unchanged.
            ("chamfer", &a, TextMeshParams { bevel: 0.05, ..Default::default() }, (8680, 14928, 4507394498162120197)),
            ("flat", &c, TextMeshParams { depth: 0.0, ..Default::default() }, (240, 672, 439485006973457876)),
            (
                "block",
                &d,
                TextMeshParams { layout: TextLayout::Block, bevel: 0.03, ..Default::default() },
                // Re-pinned with the thin-stroke bevel clamp too (folds at 0.03 in this face).
                (19726, 33588, 13167278121124364398),
            ),
        ];
        for (name, t, p, want) in cases {
            let mut m = FxMesh::default();
            build_text_mesh(t, &p, &mut m);
            assert_eq!(fingerprint(&m), want, "{name}: the geometry moved");
            // Spelling the chamfer out changes nothing.
            let explicit = TextMeshParams { bevel_type: BevelType::Chamfer, bevel_rings: 1, copies: 1, ..p.clone() };
            let mut e = FxMesh::default();
            build_text_mesh(t, &explicit, &mut e);
            assert_eq!(fingerprint(&e), want, "{name}: explicit chamfer");
        }
    }

    #[test]
    fn every_printable_ascii_glyph_builds_in_every_font() {
        let (mut l, families) = layouter();
        let text = printable_ascii();
        for (k, family) in families.iter().enumerate() {
            let laid = layout(&mut l, *family, &text, None);
            let params = TextMeshParams { bevel: 0.05, ..Default::default() };
            let mut mesh = FxMesh::default();
            let report = build_text_mesh(&laid, &params, &mut mesh);
            assert_eq!(report.glyphs, text.chars().count(), "{}: every printable glyph has ink", FONTS[k]);
            assert!(report.warnings.is_empty(), "{}: {:?}", FONTS[k], report.warnings);
            assert!(mesh.triangle_count() > 0);
            let n = mesh.vertex_count() as u32;
            for i in &mesh.idx {
                assert!(*i < n, "{}: index out of range", FONTS[k]);
            }
            let mut seen = vec![false; report.glyphs];
            for v in verts(&mesh) {
                for f in [v.pos.x, v.pos.y, v.pos.z, v.id, v.n.x, v.n.y, v.n.z, v.aux, v.uv.x, v.uv.y, v.r0, v.r1] {
                    assert!(f.is_finite(), "{}: non-finite vertex data", FONTS[k]);
                }
                let nl = (v.n.x * v.n.x + v.n.y * v.n.y + v.n.z * v.n.z).sqrt();
                assert!((nl - 1.0).abs() < 1e-3, "{}: normal length {nl}", FONTS[k]);
                assert!((0.0..=3.0).contains(&v.aux) && v.aux.fract() == 0.0, "{}: face class {}", FONTS[k], v.aux);
                assert_eq!(v.r1, 0.0, "one line");
                seen[v.id as usize] = true;
            }
            assert!(seen.iter().all(|s| *s), "{}: a glyph ordinal is missing", FONTS[k]);
            assert_eq!(report.words, 1, "{}: no whitespace, one word", FONTS[k]);
            assert_eq!(report.lines, 1);
            assert!(report.width > 0.0 && report.height > 0.0);
            println!(
                "{}: {} glyphs, {} verts, {} tris, {} fallbacks, extents {:.2}x{:.2}",
                FONTS[k], report.glyphs, report.vertices, report.triangles, report.fallbacks, report.width, report.height
            );
            // The cap of every glyph must match the nonzero-winding area of
            // its contours, whichever way the font winds or overlaps them.
            let mut off = Vec::new();
            let mut overlapping = Vec::new();
            for ch in text.chars() {
                let one = layout(&mut l, *family, &ch.to_string(), None);
                let p = TextMeshParams { depth: 0.0, ..Default::default() };
                let mut m = FxMesh::default();
                let r = build_text_mesh(&one, &p, &mut m);
                if r.glyphs == 0 {
                    continue;
                }
                let (shoelace, scan, crossings) = reference_areas(&one, &p);
                let want = if crossings == 0 { shoelace } else { scan };
                if crossings > 0 {
                    overlapping.push(ch);
                }
                let got = cap_area(&m);
                let err = (got - want).abs() / want.max(1e-6);
                if err > 0.01 || r.fallbacks > 0 {
                    off.push(format!("{ch:?} err {:.3}% fallback {} crossings {crossings}", err * 100.0, r.fallbacks));
                }
            }
            println!("{}: glyphs with crossing contours: {:?}", FONTS[k], overlapping);
            println!("{}: glyphs off the winding area by >1% or on fallback: {:?}", FONTS[k], off);
            assert!(off.is_empty(), "{}: cap areas off for {:?}", FONTS[k], off);
        }
    }

    #[test]
    fn caps_of_glyphs_with_holes_match_the_shoelace_area() {
        let (mut l, families) = layouter();
        for family in &families {
            for ch in "OB8g@&%".chars() {
                let one = layout(&mut l, *family, &ch.to_string(), None);
                let params = TextMeshParams { depth: 0.0, ..Default::default() };
                let mut mesh = FxMesh::default();
                let report = build_text_mesh(&one, &params, &mut mesh);
                assert_eq!(report.glyphs, 1);
                assert_eq!(report.fallbacks, 0, "{ch}: the CDT path must hold");
                let (shoelace, scan, crossings) = reference_areas(&one, &params);
                let got = cap_area(&mesh);
                let err_scan = (got - scan).abs() / scan;
                let err_shoe = (got - shoelace).abs() / shoelace;
                println!(
                    "{ch}: cap {got:.5} scanline {scan:.5} ({:.3}%) shoelace {shoelace:.5} ({:.3}%) crossings {crossings}",
                    err_scan * 100.0,
                    err_shoe * 100.0
                );
                assert!(err_scan < 0.01, "{ch}: cap area {got} vs scanline {scan}");
                if crossings == 0 {
                    assert!(err_shoe < 0.01, "{ch}: cap area {got} vs shoelace {shoelace}");
                }
                // Front cap only: every vertex is a front-cap vertex at z 0.
                for v in verts(&mesh) {
                    assert_eq!(v.aux, FACE_FRONT);
                    assert_eq!(v.pos.z, 0.0);
                }
            }
        }
    }

    #[test]
    fn side_quads_equal_contour_edges_and_bevel_doubles_them() {
        let (mut l, families) = layouter();
        let laid = layout(&mut l, families[0], "OB8", None);
        let params = TextMeshParams { depth: 0.4, bevel: 0.0, ..Default::default() };
        let mut mesh = FxMesh::default();
        let report = build_text_mesh(&laid, &params, &mut mesh);
        let mut cache = ShapeCache::default();
        let mut edges = 0usize;
        let mut points = 0usize;
        for row in &laid.rows {
            for g in &row.glyphs {
                let s = glyph_shape(g, params.detail, &mut cache).unwrap();
                assert_eq!(s.intersections, 0);
                assert_eq!(s.wall_count(), s.contours.iter().map(|c| c.len).sum::<usize>());
                edges += s.wall_count();
                points += s.pts.len();
            }
        }
        let vs = verts(&mesh);
        let sides = vs.iter().filter(|v| v.aux == FACE_SIDE).count();
        let fronts = vs.iter().filter(|v| v.aux == FACE_FRONT).count();
        let backs = vs.iter().filter(|v| v.aux == FACE_BACK).count();
        assert_eq!(sides, 4 * edges, "one quad per contour edge");
        assert_eq!(fronts, points);
        assert_eq!(backs, points);
        assert_eq!(report.vertices, 2 * points + 4 * edges);
        // Caps sit at ±depth/2, walls span the full depth without a bevel.
        for v in &vs {
            if v.aux == FACE_FRONT {
                assert!((v.pos.z - 0.2).abs() < 1e-6);
                assert!((v.n.z - 1.0).abs() < 1e-6);
            }
            if v.aux == FACE_BACK {
                assert!((v.pos.z + 0.2).abs() < 1e-6);
                assert!((v.n.z + 1.0).abs() < 1e-6);
            }
            if v.aux == FACE_SIDE {
                assert!(v.n.z.abs() < 1e-6);
                assert!((v.pos.z.abs() - 0.2).abs() < 1e-6);
            }
        }
        // A bevel adds two chamfer quads per edge, with 45° normals.
        let params = TextMeshParams { depth: 0.4, bevel: 0.05, ..Default::default() };
        let mut mesh = FxMesh::default();
        let report = build_text_mesh(&laid, &params, &mut mesh);
        let vs = verts(&mesh);
        let bevels = vs.iter().filter(|v| v.aux == FACE_BEVEL).count();
        assert_eq!(bevels, 8 * edges);
        assert_eq!(report.vertices, 2 * points + 12 * edges);
        for v in vs.iter().filter(|v| v.aux == FACE_BEVEL) {
            assert!((v.n.z.abs() - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-4);
        }
        for v in vs.iter().filter(|v| v.aux == FACE_SIDE) {
            assert!((v.pos.z.abs() - 0.15).abs() < 1e-6, "walls stop where the chamfer starts");
        }
    }

    #[test]
    fn walls_face_out_of_the_ink() {
        // For every side quad the outward normal, followed a little way from
        // the wall's midpoint, must leave the ink; followed backwards it must
        // land in it. Holes therefore point INTO the hole.
        let (mut l, families) = layouter();
        let laid = layout(&mut l, families[0], "O8@", None);
        let params = TextMeshParams { depth: 0.3, ..Default::default() };
        let mut mesh = FxMesh::default();
        build_text_mesh(&laid, &params, &mut mesh);
        let mut cache = ShapeCache::default();
        let vs = verts(&mesh);
        for row in &laid.rows {
            for (k, g) in row.glyphs.iter().enumerate() {
                let s = glyph_shape(g, params.detail, &mut cache).unwrap();
                let c = mul(add(s.min, s.max), 0.5);
                let em_to_world = g.font_size_in_lpxs * params.size / cap_height_lpx(&laid);
                let mut quads = 0;
                let side: Vec<&V> = vs.iter().filter(|v| v.id == k as f32 && v.aux == FACE_SIDE).collect();
                for q in side.chunks(4) {
                    let mid = vec2f(
                        (q[0].pos.x + q[1].pos.x) * 0.5 / em_to_world + c.x,
                        (q[0].pos.y + q[1].pos.y) * 0.5 / em_to_world + c.y,
                    );
                    let n = vec2f(q[0].n.x, q[0].n.y);
                    let out = add(mid, mul(n, 0.0015));
                    let inn = sub(mid, mul(n, 0.0015));
                    assert_eq!(winding_number(&s.pts, &s.contours, out), 0, "glyph {k}: wall normal points into ink");
                    assert_ne!(winding_number(&s.pts, &s.contours, inn), 0, "glyph {k}: wall normal has no ink behind it");
                    quads += 1;
                }
                assert_eq!(quads, s.wall_count());
            }
        }
    }

    #[test]
    fn block_wraps_and_numbers_words_and_lines() {
        let (mut l, families) = layouter();
        let text = "the quick brown fox jumps over the lazy dog";
        let laid = layout(&mut l, families[0], text, Some(500.0));
        assert!(laid.rows.len() > 1, "the test text must wrap at 500 lpx");
        let params = TextMeshParams { layout: TextLayout::Block, ..Default::default() };
        let mut mesh = FxMesh::default();
        let report = build_text_mesh(&laid, &params, &mut mesh);
        assert!(report.lines > 1);
        assert_eq!(report.lines, laid.rows.len());
        assert_eq!(report.words, 9);
        assert_eq!(report.glyphs, text.chars().filter(|c| !c.is_whitespace()).count());
        let vs = verts(&mesh);
        let max_line = vs.iter().map(|v| v.r1).fold(0.0f32, f32::max);
        let max_word = vs.iter().map(|v| v.r0).fold(0.0f32, f32::max);
        assert_eq!(max_line as usize + 1, report.lines);
        assert_eq!(max_word as usize + 1, report.words);
        // Word ordinals never decrease along the glyph ordinals, and the
        // first row's cap height is the size: the block is centred.
        let mut last = (0.0f32, 0.0f32);
        for v in &vs {
            assert!(v.id >= last.0);
            if v.id > last.0 {
                assert!(v.r0 >= last.1);
                last = (v.id, v.r0);
            }
        }
        let xs: Vec<f32> = vs.iter().map(|v| v.uv.x + v.pos.x).collect();
        let ys: Vec<f32> = vs.iter().map(|v| v.uv.y + v.pos.y).collect();
        let (x0, x1) = xs.iter().fold((f32::MAX, f32::MIN), |a, x| (a.0.min(*x), a.1.max(*x)));
        let (y0, y1) = ys.iter().fold((f32::MAX, f32::MIN), |a, y| (a.0.min(*y), a.1.max(*y)));
        assert!((x0 + x1).abs() < 1e-3 && (y0 + y1).abs() < 1e-3, "block centred on the origin");
        assert!((x1 - x0 - report.width).abs() < 1e-3);
        assert!((y1 - y0 - report.height).abs() < 1e-3);
        // Upper rows sit above lower rows in y-up text space.
        let row0_y = vs.iter().filter(|v| v.r1 == 0.0).map(|v| v.uv.y).fold(f32::MIN, f32::max);
        let row1_y = vs.iter().filter(|v| v.r1 == 1.0).map(|v| v.uv.y).fold(f32::MIN, f32::max);
        assert!(row0_y > row1_y, "y is up");
    }

    #[test]
    fn ring_holds_its_radius_and_turns_the_frame() {
        let (mut l, families) = layouter();
        let laid = layout(&mut l, families[1], "RING AROUND THE ROSES", None);
        for close in [false, true] {
            let params = TextMeshParams {
                layout: TextLayout::Ring,
                ring_close: close,
                ring_radius: if close { Some(3.0) } else { None },
                ..Default::default()
            };
            let mut mesh = FxMesh::default();
            let report = build_text_mesh(&laid, &params, &mut mesh);
            let r = report.ring_radius;
            assert!(r > 0.0);
            let vs = verts(&mesh);
            let mut min_arc = f32::MAX;
            let mut max_arc = f32::MIN;
            for v in &vs {
                let theta = v.uv.x / r;
                let rest = vec3f(r * theta.sin(), v.uv.y, r * theta.cos());
                let radial = (rest.x * rest.x + rest.z * rest.z).sqrt();
                assert!((radial - r).abs() < 1e-3, "rest centre off the cylinder");
                min_arc = min_arc.min(v.uv.x);
                max_arc = max_arc.max(v.uv.x);
                if v.aux == FACE_FRONT {
                    // The cap normal is the cylinder normal at the pivot and
                    // the cap plane sits depth/2 out along it.
                    assert!((v.n.x - theta.sin()).abs() < 1e-4 && (v.n.z - theta.cos()).abs() < 1e-4);
                    let along = v.pos.x * v.n.x + v.pos.y * v.n.y + v.pos.z * v.n.z;
                    assert!((along - params.depth * 0.5).abs() < 1e-4);
                }
            }
            if close {
                // The arc pitch was stretched to the circumference (5 % gap).
                assert!(max_arc - min_arc > TAU * r * 0.8, "arc {} of {}", max_arc - min_arc, TAU * r);
                assert!(max_arc - min_arc < TAU * r);
            } else {
                let row_width = laid.rows[0].width_in_lpxs * params.size / cap_height_lpx(&laid);
                assert!((r - row_width / TAU * 1.05).abs() < 1e-4, "auto radius");
            }
        }
    }

    #[test]
    fn tunnel_puts_every_word_on_its_own_ring() {
        let (mut l, families) = layouter();
        let laid = layout(&mut l, families[0], "FLY THROUGH THE WORDS", None);
        let params = TextMeshParams { layout: TextLayout::Tunnel, ..Default::default() };
        let mut mesh = FxMesh::default();
        let report = build_text_mesh(&laid, &params, &mut mesh);
        assert_eq!(report.words, 4);
        assert!(report.ring_radius >= 2.0 * params.size);
        let r = report.ring_radius;
        for v in verts(&mesh) {
            let phi = v.r0 * TUNNEL_GOLDEN_ANGLE - v.uv.x / r;
            if v.aux == FACE_FRONT {
                // Letters face +z, tops outward (local +y = radial).
                assert!((v.n.z - 1.0).abs() < 1e-5);
                let up = Frame::rot_z(phi - std::f32::consts::FRAC_PI_2).apply(vec3f(0.0, 1.0, 0.0));
                assert!((up.x - phi.cos()).abs() < 1e-4 && (up.y - phi.sin()).abs() < 1e-4);
            }
            assert!(v.uv.x.abs() < TAU * r, "arc offset within one turn");
        }
    }

    #[test]
    fn cloud_rectangles_never_overlap_and_a_seed_repeats() {
        let (mut l, families) = layouter();
        let names = [
            "stage", "light", "beat", "drop", "bass", "night", "pulse", "echo", "neon", "wave", "kick",
            "loop", "flash", "rise", "fall", "glow", "dust", "fire", "cold", "warm", "deep", "high",
            "slow", "fast", "loud", "soft", "dark", "gold", "blue", "red",
        ];
        let words: Vec<CloudWord> = names
            .iter()
            .enumerate()
            .map(|(i, w)| CloudWord {
                text: layout(&mut l, families[(i % 3) as usize], w, None),
                weight: ((i * 7) % 30) as f32 / 29.0,
            })
            .collect();
        let params = TextMeshParams { layout: TextLayout::Cloud, seed: 7, words: 40, ..Default::default() };
        let mut mesh = FxMesh::default();
        let report = build_cloud_mesh(&words, &params, &mut mesh);
        assert_eq!(report.words, names.len(), "{:?}", report.warnings);
        assert!(report.warnings.is_empty());
        let vs = verts(&mesh);
        // One rectangle per word from the emitted geometry.
        let mut rects: HashMap<usize, (f32, f32, f32, f32)> = HashMap::new();
        for v in &vs {
            let (x, y) = (v.uv.x + v.pos.x, v.uv.y + v.pos.y);
            let e = rects.entry(v.r0 as usize).or_insert((f32::MAX, f32::MAX, f32::MIN, f32::MIN));
            e.0 = e.0.min(x);
            e.1 = e.1.min(y);
            e.2 = e.2.max(x);
            e.3 = e.3.max(y);
        }
        assert_eq!(rects.len(), names.len());
        let list: Vec<(usize, (f32, f32, f32, f32))> = rects.iter().map(|(k, v)| (*k, *v)).collect();
        for (i, (wa, a)) in list.iter().enumerate() {
            for (wb, b) in list.iter().skip(i + 1) {
                let apart = a.2 <= b.0 + 1e-4 || b.2 <= a.0 + 1e-4 || a.3 <= b.1 + 1e-4 || b.3 <= a.1 + 1e-4;
                assert!(apart, "words {wa} and {wb} overlap: {a:?} {b:?}");
            }
        }
        // Weights ride a_r1; the heaviest word is ordinal 0 and the largest.
        let heaviest = (0..names.len()).max_by(|a, b| words[*a].weight.total_cmp(&words[*b].weight)).unwrap();
        let w0: Vec<&V> = vs.iter().filter(|v| v.r0 == 0.0).collect();
        assert!((w0[0].r1 - words[heaviest].weight).abs() < 1e-6);
        let h0 = rects[&0].3 - rects[&0].1;
        let lightest = (0..names.len()).min_by(|a, b| words[*a].weight.total_cmp(&words[*b].weight)).unwrap();
        let ord_light = vs.iter().find(|v| (v.r1 - words[lightest].weight).abs() < 1e-6).unwrap().r0 as usize;
        let h1 = rects[&ord_light].3 - rects[&ord_light].1;
        assert!(h0 > h1, "heavier words are bigger: {h0} vs {h1}");
        // Same seed, same floats; another seed, another cloud.
        let mut again = FxMesh::default();
        build_cloud_mesh(&words, &params, &mut again);
        assert_eq!(mesh.verts, again.verts);
        assert_eq!(mesh.idx, again.idx);
        let mut other = FxMesh::default();
        build_cloud_mesh(&words, &TextMeshParams { seed: 8, ..params.clone() }, &mut other);
        assert_ne!(mesh.verts, other.verts);
        // Centred on the origin.
        let (x0, x1) = vs.iter().fold((f32::MAX, f32::MIN), |a, v| (a.0.min(v.uv.x + v.pos.x), a.1.max(v.uv.x + v.pos.x)));
        assert!((x0 + x1).abs() < 1e-3);
    }

    #[test]
    fn budget_degrades_in_order_and_reports_every_step() {
        let (mut l, families) = layouter();
        let text = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghijklmnopqrstuvwx";
        let laid = layout(&mut l, families[0], text, None);
        let full = TextMeshParams { bevel: 0.05, ..Default::default() };
        let mut mesh = FxMesh::default();
        let base = build_text_mesh(&laid, &full, &mut mesh);
        assert!(base.warnings.is_empty());
        let budget = base.vertices / 8;
        let params = TextMeshParams { vert_budget: budget, ..full.clone() };
        let mut mesh = FxMesh::default();
        let report = build_text_mesh(&laid, &params, &mut mesh);
        println!("{:?}", report.warnings);
        assert!(report.vertices <= budget);
        assert_eq!(report.vertices, mesh.vertex_count());
        assert_eq!(report.warnings.len(), 4, "{:?}", report.warnings);
        assert!(report.warnings[0].contains("detail coarsened"));
        assert!(report.warnings[1].contains("detail coarsened"));
        assert!(report.warnings[2].contains("bevel dropped"));
        assert!(report.warnings[3].contains("glyphs dropped"));
        assert!(report.glyphs < base.glyphs && report.glyphs > 0);
        assert!(verts(&mesh).iter().all(|v| v.aux != FACE_BEVEL), "the bevel went before the glyphs");
        // A budget the coarsening alone can meet stops after one step.
        let mut probe = FxMesh::default();
        let coarse = build_text_mesh(&laid, &TextMeshParams { detail: 0.018, ..full.clone() }, &mut probe);
        let mut mesh = FxMesh::default();
        let report = build_text_mesh(&laid, &TextMeshParams { vert_budget: coarse.vertices, ..full.clone() }, &mut mesh);
        assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
        assert!(report.warnings[0].contains("detail coarsened"));
        assert_eq!(report.glyphs, base.glyphs);
    }

    #[test]
    fn nonsense_params_never_panic_or_leak_nan() {
        let (mut l, families) = layouter();
        let laid = layout(&mut l, families[3], "safe @ all costs", None);
        for layout in [TextLayout::Line, TextLayout::Ring, TextLayout::Tunnel] {
            let params = TextMeshParams {
                size: f32::NAN,
                depth: f32::INFINITY,
                bevel: -3.0,
                detail: f32::NAN,
                layout,
                ring_radius: Some(f32::NAN),
                tunnel_pitch: -1.0,
                vert_budget: 0,
                ..Default::default()
            };
            let mut mesh = FxMesh::default();
            let report = build_text_mesh(&laid, &params, &mut mesh);
            assert!(mesh.verts.iter().all(|f| f.is_finite()), "{layout:?} leaked non-finite data");
            for f in [report.width, report.height, report.ring_radius] {
                assert!(f.is_finite());
            }
        }
        let empty = layout(&mut l, families[0], "   ", None);
        let mut mesh = FxMesh::default();
        let report = build_text_mesh(&empty, &TextMeshParams::default(), &mut mesh);
        assert_eq!(report.glyphs, 0);
        assert_eq!(mesh.vertex_count(), 0);
        let mut mesh = FxMesh::default();
        let report = build_cloud_mesh(&[], &TextMeshParams::default(), &mut mesh);
        assert_eq!(report.words, 0);
    }

    #[test]
    fn ear_clip_fallback_handles_a_concave_contour() {
        // A hand-made C shape: the fallback must cover exactly its area.
        let pts: Vec<Vec2f> = [
            (0.0, 0.0), (1.0, 0.0), (1.0, 0.2), (0.2, 0.2), (0.2, 0.8), (1.0, 0.8), (1.0, 1.0), (0.0, 1.0),
        ]
        .iter()
        .map(|(x, y)| vec2f(*x, *y))
        .collect();
        let area = shoelace(&pts);
        let contours = vec![Contour { start: 0, len: pts.len(), area }];
        let tris = ear_clip_largest(&pts, &contours);
        let sum: f32 = tris
            .iter()
            .map(|t| 0.5 * cross2(sub(pts[t[1] as usize], pts[t[0] as usize]), sub(pts[t[2] as usize], pts[t[0] as usize])))
            .sum();
        assert_eq!(tris.len(), pts.len() - 2);
        assert!((sum - area.abs()).abs() < 1e-6, "{sum} vs {area}");
        assert!(tris.iter().all(|t| {
            0.5 * cross2(sub(pts[t[1] as usize], pts[t[0] as usize]), sub(pts[t[2] as usize], pts[t[0] as usize])) > 0.0
        }), "fallback triangles are CCW");
    }

    fn square(x0: f32, y0: f32, x1: f32, y1: f32, ccw: bool) -> Vec<Command> {
        use makepad_draw::text::geom::Point;
        let mut c = vec![
            Command::MoveTo(Point::new(x0, y0)),
            Command::LineTo(Point::new(x1, y0)),
            Command::LineTo(Point::new(x1, y1)),
            Command::LineTo(Point::new(x0, y1)),
            Command::Close,
        ];
        if !ccw {
            c = vec![
                Command::MoveTo(Point::new(x0, y0)),
                Command::LineTo(Point::new(x0, y1)),
                Command::LineTo(Point::new(x1, y1)),
                Command::LineTo(Point::new(x1, y0)),
                Command::Close,
            ];
        }
        c
    }

    fn shape_area(shape: &GlyphShape) -> f32 {
        shape
            .tris
            .iter()
            .map(|t| {
                0.5 * cross2(
                    sub(shape.pts[t[1] as usize], shape.pts[t[0] as usize]),
                    sub(shape.pts[t[2] as usize], shape.pts[t[0] as usize]),
                )
            })
            .sum()
    }

    #[test]
    fn overlapping_contours_triangulate_to_their_union() {
        // Two 1 em squares overlapping by a quarter: the nonzero fill is
        // their union (1.75 em²), the crossing edges are split, the halves
        // buried in the overlap get no wall.
        let mut cmds = square(0.0, 0.0, 1000.0, 1000.0, true);
        cmds.extend(square(500.0, 500.0, 1500.0, 1500.0, true));
        let shape = GlyphShape::build(&cmds, 1000.0, 0.012).expect("ink");
        assert_eq!(shape.intersections, 2);
        assert!(!shape.fallback);
        assert!((shape_area(&shape) - 1.75).abs() < 1e-4, "{}", shape_area(&shape));
        assert_eq!(shape.wall_count(), 8);
        assert_eq!(shape.edge_out.iter().filter(|o| **o == 0.0).count(), 4);
        assert!(shape.tris.iter().all(|t| {
            let c = mul(
                add(add(shape.pts[t[0] as usize], shape.pts[t[1] as usize]), shape.pts[t[2] as usize]),
                1.0 / 3.0,
            );
            winding_number(&shape.pts, &shape.contours, c) != 0
        }));
    }

    #[test]
    fn nested_contours_fill_by_nonzero_winding_in_either_orientation() {
        // A square with a square hole, the hole wound the other way
        // (TrueType and CFF conventions both): area 1 - 0.25, and the
        // hole's walls face into the hole.
        for outer_ccw in [true, false] {
            let mut cmds = square(0.0, 0.0, 1000.0, 1000.0, outer_ccw);
            cmds.extend(square(250.0, 250.0, 750.0, 750.0, !outer_ccw));
            let shape = GlyphShape::build(&cmds, 1000.0, 0.012).expect("ink");
            assert_eq!(shape.intersections, 0);
            assert!(!shape.fallback);
            assert!((shape_area(&shape) - 0.75).abs() < 1e-5);
            assert_eq!(shape.wall_count(), 8);
            let hole = &shape.contours[1];
            for k in 0..hole.len {
                let a = shape.pts[hole.start + k];
                let b = shape.pts[hole.start + (k + 1) % hole.len];
                let n = edge_normal(a, b, shape.edge_out[hole.start + k]);
                let mid = mul(add(a, b), 0.5);
                let inward = add(mid, mul(n, 0.05));
                assert!(inward.x > 0.25 && inward.x < 0.75 && inward.y > 0.25 && inward.y < 0.75, "hole wall faces the hole");
            }
            // Same-direction nesting would be a filled square under nonzero.
            let mut cmds = square(0.0, 0.0, 1000.0, 1000.0, outer_ccw);
            cmds.extend(square(250.0, 250.0, 750.0, 750.0, outer_ccw));
            let shape = GlyphShape::build(&cmds, 1000.0, 0.012).expect("ink");
            assert!((shape_area(&shape) - 1.0).abs() < 1e-5);
            assert_eq!(shape.wall_count(), 4, "the inner ring is buried");
        }
    }

    // -----------------------------------------------------------------------
    // Phase B: layouts, copies, changed flags, bevel profiles, the floor,
    // voxels and the voxel morph.
    // -----------------------------------------------------------------------

    fn norm3(v: Vec3f) -> f32 {
        (v.x * v.x + v.y * v.y + v.z * v.z).sqrt()
    }

    fn close3(a: Vec3f, b: Vec3f, eps: f32) -> bool {
        (a.x - b.x).abs() < eps && (a.y - b.y).abs() < eps && (a.z - b.z).abs() < eps
    }

    /// Total contour points and walled edges of a laid-out text's glyphs.
    fn shape_totals(laid: &LaidoutText) -> (usize, usize) {
        let mut cache = ShapeCache::default();
        let (mut points, mut edges) = (0, 0);
        for row in &laid.rows {
            for g in &row.glyphs {
                if let Some(s) = glyph_shape(g, 0.012, &mut cache) {
                    points += s.pts.len();
                    edges += s.wall_count();
                }
            }
        }
        (points, edges)
    }

    /// The face a cube vertex lies on, by the rule the shader uses.
    fn derived_face(p: Vec3f, half: [f32; 3]) -> Vec3f {
        let r = [p.x.abs() / half[0], p.y.abs() / half[1], p.z.abs() / half[2]];
        let k = if r[0] >= r[1] && r[0] >= r[2] {
            0
        } else if r[1] >= r[2] {
            1
        } else {
            2
        };
        let c = [p.x, p.y, p.z];
        let mut n = [0.0f32; 3];
        n[k] = c[k].signum();
        vec3f(n[0], n[1], n[2])
    }

    #[test]
    fn layout_and_bevel_names_parse_to_their_contract_indices() {
        let layouts = [
            ("line", 0.0),
            ("block", 1.0),
            ("ring", 2.0),
            ("tunnel", 3.0),
            ("cloud", 4.0),
            ("helix", 5.0),
            ("grid", 6.0),
            ("voxel", 7.0),
        ];
        for (name, index) in layouts {
            assert_eq!(TextLayout::parse(name).map(TextLayout::index), Some(index), "{name}");
        }
        assert_eq!(TextLayout::parse("spiral"), None);
        let bevels = [("chamfer", 0.0), ("round", 1.0), ("cove", 2.0), ("step", 3.0), ("ogee", 4.0)];
        for (name, index) in bevels {
            assert_eq!(BevelType::parse(name).map(BevelType::index), Some(index), "{name}");
        }
        assert_eq!(BevelType::parse("fillet"), None);
    }

    #[test]
    fn bevel_profiles_shape_the_band_and_mirror_it_on_the_back() {
        let (mut l, families) = layouter();
        let laid = layout(&mut l, families[0], "OB8", None);
        let (points, edges) = shape_totals(&laid);
        let (depth, bevel) = (0.4f32, 0.06f32);
        let (front, z1) = (0.5 * depth, 0.5 * depth - bevel);
        let kinds = [BevelType::Chamfer, BevelType::Round, BevelType::Cove, BevelType::Step, BevelType::Ogee];
        for kind in kinds {
            for rings in [1usize, 3, 8] {
                let prof = Profile::new(kind, rings);
                assert_eq!(prof.pts[0], (0.0, 0.0));
                assert_eq!(*prof.pts.last().unwrap(), (1.0, 1.0));
                for w in prof.pts.windows(2) {
                    assert!(w[1].0 >= w[0].0 - 1e-6 && w[1].1 >= w[0].1 - 1e-6, "{kind:?}: the profile turns back");
                }
                let want = match kind {
                    BevelType::Step => 2 * rings.max(2),
                    BevelType::Ogee => (rings.max(2) + 1) / 2 * 2,
                    _ => rings,
                };
                assert_eq!(prof.segments(), want, "{kind:?} {rings}");
                let params = TextMeshParams { depth, bevel, bevel_type: kind, bevel_rings: rings, ..Default::default() };
                let mut mesh = FxMesh::default();
                let report = build_text_mesh(&laid, &params, &mut mesh);
                let segs = prof.segments();
                assert_eq!(report.bevel_rings, segs);
                assert_eq!(report.vertices, 2 * points + 4 * edges + 8 * edges * segs, "{kind:?} {rings}");
                let vs = verts(&mesh);
                let band: Vec<&V> = vs.iter().filter(|v| v.aux == FACE_BEVEL).collect();
                assert_eq!(band.len(), 8 * edges * segs);
                let mut zsum = 0.0f64;
                for v in &band {
                    assert!((norm3(v.n) - 1.0).abs() < 1e-3, "{kind:?}: normal length {}", norm3(v.n));
                    let az = v.pos.z.abs();
                    assert!(az >= z1 - 1e-5 && az <= front + 1e-5, "{kind:?}: band vertex at z {}", v.pos.z);
                    assert!(v.n.z * v.pos.z >= -1e-5, "{kind:?}: the band faces away from its cap");
                    zsum += v.pos.z as f64;
                    let at_cap = (az - front).abs() < 1e-6;
                    let at_outline = (az - z1).abs() < 1e-6;
                    match kind {
                        BevelType::Chamfer => {
                            assert!((v.n.z.abs() - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6)
                        }
                        BevelType::Step => assert!(v.n.z.abs() < 1e-4 || v.n.z.abs() > 0.9999, "{}", v.n.z),
                        BevelType::Round => {
                            if at_cap {
                                assert!(v.n.z.abs() > 0.999, "round meets the cap flat");
                            }
                            if at_outline {
                                assert!(v.n.z.abs() < 1e-3, "round leaves the wall vertically");
                            }
                        }
                        BevelType::Cove => {
                            if at_cap {
                                assert!(v.n.z.abs() < 1e-3, "cove meets the cap vertically");
                            }
                            if at_outline {
                                assert!(v.n.z.abs() > 0.999, "cove leaves the wall flat");
                            }
                        }
                        BevelType::Ogee => {
                            if at_cap || at_outline {
                                assert!(v.n.z.abs() > 0.999, "the S starts and ends flat");
                            }
                        }
                    }
                }
                assert!(zsum.abs() < 1e-2, "{kind:?}: the back band mirrors the front ({zsum})");
                // The caps sit on the inset ring whatever the profile.
                for v in vs.iter().filter(|v| v.aux == FACE_FRONT) {
                    assert!((v.pos.z - front).abs() < 1e-6);
                }
            }
        }
    }

    #[test]
    fn helix_winds_every_row_down_a_coil() {
        let (mut l, families) = layouter();
        let laid = layout(&mut l, families[1], "COIL SPRING", Some(400.0));
        assert_eq!(laid.rows.len(), 2, "the test text must wrap into two rows");
        let (r, pitch) = (1.0f32, 1.5f32);
        let params = TextMeshParams {
            layout: TextLayout::Helix,
            ring_radius: Some(r),
            helix_pitch: pitch,
            ..Default::default()
        };
        let mut mesh = FxMesh::default();
        let report = build_text_mesh(&laid, &params, &mut mesh);
        assert_eq!(report.ring_radius, r);
        assert_eq!(report.helix_pitch, pitch);
        assert_eq!(report.glyphs, 10);
        let vs = verts(&mesh);
        let mut pivots: Vec<Vec2f> = vec![vec2f(f32::NAN, f32::NAN); report.glyphs];
        for v in &vs {
            pivots[v.id as usize] = v.uv;
            if v.aux == FACE_FRONT {
                // The cap faces out of the cylinder whatever the tilt.
                let theta = v.uv.x / r;
                assert!(close3(v.n, vec3f(theta.sin(), 0.0, theta.cos()), 1e-4));
            }
        }
        // Reading order runs along the arc, both rows wound end to end.
        for w in pivots.windows(2) {
            assert!(w[1].x > w[0].x, "the arc grows with the ordinal");
        }
        assert!(pivots[9].x - pivots[0].x > TAU * r, "more than one turn");
        // Take the descent out and every glyph sits within a cap of the mean.
        let resid: Vec<f32> = pivots.iter().map(|p| p.y + pitch * p.x / (TAU * r)).collect();
        let mean = resid.iter().sum::<f32>() / resid.len() as f32;
        assert!(resid.iter().all(|x| (x - mean).abs() < params.size), "{resid:?}");
        let drop = pivots[0].y - pivots[9].y;
        let want = pitch * (pivots[9].x - pivots[0].x) / (TAU * r);
        assert!((drop - want).abs() < 0.3 * params.size, "the coil descends as it reads: {drop} vs {want}");
        // Local +x runs along the coil.
        let tilt = (-pitch / (TAU * r)).atan();
        let f = helix_frame(0.7, tilt);
        let t = vec3f(r * 0.7f32.cos(), -pitch / TAU, -r * 0.7f32.sin());
        let tl = norm3(t);
        assert!(close3(f.x, vec3f(t.x / tl, t.y / tl, t.z / tl), 1e-5));
        // Copies turn the coil by a share of the circumference.
        let mut three = FxMesh::default();
        let rep3 = build_text_mesh(&laid, &TextMeshParams { copies: 3, ..params.clone() }, &mut three);
        assert_eq!(rep3.vertices, 3 * report.vertices);
        let v3 = verts(&three);
        let n = report.vertices;
        for c in 0..3 {
            for i in (0..n).step_by(7) {
                let (a, b) = (vs[i], v3[c * n + i]);
                assert!((b.uv.x - a.uv.x - c as f32 * TAU * r / 3.0).abs() < 1e-3);
                assert_eq!(b.uv.y, a.uv.y);
                assert_eq!(b.r1, a.r1 + 1000.0 * c as f32);
            }
        }
    }

    #[test]
    fn grid_lays_the_glyphs_flat_on_a_lattice() {
        let (mut l, families) = layouter();
        let laid = layout(&mut l, families[0], "ABCDEFGHIJ", None);
        let params = TextMeshParams { layout: TextLayout::Grid, grid_cols: 4, grid_pitch: 1.5, ..Default::default() };
        let mut mesh = FxMesh::default();
        let report = build_text_mesh(&laid, &params, &mut mesh);
        assert_eq!((report.grid_cols, report.grid_rows, report.grid_pitch), (4, 3, 1.5));
        assert_eq!(report.lines, 3);
        assert!((report.width - 6.0).abs() < 1e-6 && (report.height - 4.5).abs() < 1e-6);
        let vs = verts(&mesh);
        for v in &vs {
            let id = v.id as usize;
            let (col, row) = (id % 4, id / 4);
            let want = vec2f((col as f32 - 1.5) * 1.5, (row as f32 - 1.0) * 1.5);
            assert!((v.uv.x - want.x).abs() < 1e-5 && (v.uv.y - want.y).abs() < 1e-5, "glyph {id} off its cell");
            assert_eq!(v.r1, row as f32);
            if v.aux == FACE_FRONT {
                assert!(close3(v.n, vec3f(0.0, 1.0, 0.0), 1e-6), "letters lie facing up");
                assert!((v.pos.y - 0.5 * params.depth).abs() < 1e-6);
            }
        }
        // The first row lies farthest from a camera at +z.
        assert!(vs.iter().filter(|v| v.id == 0.0).all(|v| v.uv.y < 0.0));
        // Auto lattice: ceil(sqrt(10)) columns at 1.5 caps.
        let mut auto = FxMesh::default();
        let rep = build_text_mesh(&laid, &TextMeshParams { layout: TextLayout::Grid, ..Default::default() }, &mut auto);
        assert_eq!((rep.grid_cols, rep.grid_rows), (4, 3));
        assert!((rep.grid_pitch - 1.5).abs() < 1e-6);
        // Two copies: the second is the field turned half a turn about Y.
        let mut two = FxMesh::default();
        build_text_mesh(&laid, &TextMeshParams { copies: 2, ..params.clone() }, &mut two);
        let v2s = verts(&two);
        let n = vs.len();
        for i in 0..n {
            let (a, b) = (vs[i], v2s[n + i]);
            assert!((a.uv.x + b.uv.x).abs() < 1e-4 && (a.uv.y + b.uv.y).abs() < 1e-4);
            if b.aux == FACE_FRONT {
                assert!(close3(b.n, vec3f(0.0, 1.0, 0.0), 1e-5));
            }
        }
    }

    #[test]
    fn copies_instance_the_whole_text_and_encode_the_copy() {
        let (mut l, families) = layouter();
        let laid = layout(&mut l, families[1], "PADDLE", None);
        let mut one = FxMesh::default();
        let r1 = build_text_mesh(&laid, &TextMeshParams::default(), &mut one);
        assert_eq!((r1.copies, r1.copy_radius), (1, 0.0));
        let mut four = FxMesh::default();
        let r4 = build_text_mesh(&laid, &TextMeshParams { copies: 4, ..Default::default() }, &mut four);
        assert_eq!(r4.copies, 4);
        assert!(r4.copy_radius > 0.0, "the paddle wheel has a radius");
        assert_eq!(r4.vertices, 4 * r1.vertices);
        let (n, il) = (one.vertex_count(), one.idx.len());
        assert_eq!(four.idx.len(), 4 * il);
        let (v1, v4) = (verts(&one), verts(&four));
        for c in 0..4 {
            for i in 0..n {
                let (a, b) = (v1[i], v4[c * n + i]);
                // Line copies are exact duplicates; the shader turns them.
                assert!(a.pos == b.pos && a.n == b.n && a.uv == b.uv && a.id == b.id && a.aux == b.aux && a.r0 == b.r0);
                assert_eq!(b.r1, a.r1 + 1000.0 * c as f32);
                assert_eq!((b.r1 / COPY_STRIDE).floor() as usize, c);
            }
            for k in 0..il {
                assert_eq!(four.idx[c * il + k], one.idx[k] + (c * n) as u32);
            }
        }
        // The count is clamped to 1..32.
        let mut m = FxMesh::default();
        assert_eq!(build_text_mesh(&laid, &TextMeshParams { copies: 0, ..Default::default() }, &mut m).copies, 1);
        let big = build_text_mesh(&laid, &TextMeshParams { copies: 99, vert_budget: 10_000_000, ..Default::default() }, &mut m);
        assert_eq!(big.copies, MAX_COPIES);
        assert_eq!(big.vertices, MAX_COPIES * r1.vertices);
        // Ring copies are baked: copy c sits a c/4 share of the circumference on.
        let ring = TextMeshParams { layout: TextLayout::Ring, ..Default::default() };
        let mut a = FxMesh::default();
        let ra = build_text_mesh(&laid, &ring, &mut a);
        let mut b = FxMesh::default();
        build_text_mesh(&laid, &TextMeshParams { copies: 4, ..ring.clone() }, &mut b);
        let (va, vb) = (verts(&a), verts(&b));
        let r = ra.ring_radius;
        for c in 0..4 {
            for i in 0..va.len() {
                let (x, y) = (va[i], vb[c * va.len() + i]);
                assert!((y.uv.x - x.uv.x - c as f32 * TAU * r / 4.0).abs() < 1e-3);
                if y.aux == FACE_FRONT {
                    let theta = y.uv.x / r;
                    assert!(close3(y.n, vec3f(theta.sin(), 0.0, theta.cos()), 1e-4));
                }
            }
        }
        // Block copies step back by the tunnel pitch (applied by the shader).
        let block = build_text_mesh(
            &laid,
            &TextMeshParams { layout: TextLayout::Block, copies: 3, tunnel_pitch: 4.0, ..Default::default() },
            &mut m,
        );
        assert_eq!((block.copy_pitch, block.copy_radius), (4.0, 0.0));
    }

    #[test]
    fn changed_glyphs_carry_half_a_word() {
        let (mut l, families) = layouter();
        let laid = layout(&mut l, families[0], "AB CD", None);
        let params = TextMeshParams { changed: vec![false, true, false, true, true, true], ..Default::default() };
        let mut mesh = FxMesh::default();
        build_text_mesh(&laid, &params, &mut mesh);
        for v in verts(&mesh) {
            let id = v.id as usize;
            let flag = if id == 1 || id == 3 { 0.5 } else { 0.0 };
            assert_eq!(v.r0.fract(), flag, "glyph {id}");
            assert_eq!(v.r0.floor(), (id / 2) as f32, "glyph {id}: its word");
        }
        let mut plain = FxMesh::default();
        build_text_mesh(&laid, &TextMeshParams::default(), &mut plain);
        assert!(verts(&plain).iter().all(|v| v.r0.fract() == 0.0));
        // Voxel cubes carry the flag of their glyph.
        let vox = TextMeshParams { layout: TextLayout::Voxel, changed: vec![true], ..Default::default() };
        let mut m = FxMesh::default();
        build_text_mesh(&laid, &vox, &mut m);
        for v in verts(&m) {
            assert_eq!(v.r0.fract(), if v.id == 0.0 { 0.5 } else { 0.0 });
        }
    }

    #[test]
    fn changed_flags_follow_the_shaped_glyphs_not_the_characters() {
        let (mut l, families) = layouter();
        // U+E000 has no glyph in the font: it is skipped by the mesh, so
        // the glyph ordinals run A B C while the characters run A ? B C.
        let first = layout(&mut l, families[0], "A\u{E000}BC", None);
        let mut m = FxMesh::default();
        let r = build_text_mesh(&first, &TextMeshParams { previous_glyphs: Some(Vec::new()), ..Default::default() }, &mut m);
        assert_eq!(r.glyphs, 3);
        assert_eq!(r.glyph_keys.len(), 3);
        assert_eq!(r.changed, vec![true; 3], "nothing before: every glyph is new");
        let next = layout(&mut l, families[0], "A\u{E000}BD", None);
        let params = TextMeshParams { previous_glyphs: Some(r.glyph_keys.clone()), ..Default::default() };
        let r2 = build_text_mesh(&next, &params, &mut m);
        assert_eq!(r2.changed, vec![false, false, true], "the D, not a glyph past the end");
        for v in verts(&m) {
            assert_eq!(v.r0.fract(), if v.id == 2.0 { 0.5 } else { 0.0 }, "glyph {}", v.id);
        }
        // The same text again: nothing changed.
        let r3 = build_text_mesh(&next, &TextMeshParams { previous_glyphs: Some(r2.glyph_keys.clone()), ..Default::default() }, &mut m);
        assert!(r3.changed.iter().all(|c| !c));
        // Voxels flag by the same glyph ordinals.
        let vox = TextMeshParams { layout: TextLayout::Voxel, previous_glyphs: Some(r.glyph_keys.clone()), ..Default::default() };
        let rv = build_text_mesh(&next, &vox, &mut m);
        assert_eq!(rv.changed, vec![false, false, true]);
    }

    #[test]
    fn copies_without_the_turn_stand_at_one_pose() {
        let (mut l, families) = layouter();
        let laid = layout(&mut l, families[1], "PROP", None);
        for layout in [TextLayout::Line, TextLayout::Block, TextLayout::Cloud, TextLayout::Ring, TextLayout::Helix, TextLayout::Grid, TextLayout::Voxel] {
            let params = TextMeshParams { layout, copies: 3, copies_turn: false, ..Default::default() };
            let mut m = FxMesh::default();
            let r = if layout == TextLayout::Cloud {
                let words = vec![CloudWord { text: laid.clone(), weight: 1.0 }];
                build_cloud_mesh(&words, &params, &mut m)
            } else {
                build_text_mesh(&laid, &params, &mut m)
            };
            assert_eq!((r.copy_radius, r.copy_pitch), (0.0, 0.0), "{layout:?}");
            let vs = verts(&m);
            let n = vs.len() / 3;
            for c in 1..3 {
                for i in 0..n {
                    let (a, b) = (vs[i], vs[c * n + i]);
                    assert!(a.pos == b.pos && a.n == b.n && a.uv == b.uv, "{layout:?}: copy {c} moved");
                    assert_eq!(b.r1, a.r1 + 1000.0 * c as f32);
                }
            }
        }
    }

    #[test]
    fn the_backdrop_goes_first_and_shifts_every_index() {
        let (mut l, families) = layouter();
        let laid = layout(&mut l, families[0], "Bd", None);
        let mut plain = FxMesh::default();
        let params = TextMeshParams { floor: Some(FloorParams { grid: 4, ..Default::default() }), ..Default::default() };
        build_text_mesh(&laid, &params, &mut plain);
        let mut m = FxMesh::default();
        build_text_mesh(&laid, &params, &mut m);
        prepend_backdrop(&mut m);
        assert_eq!(m.vertex_count(), plain.vertex_count() + 4);
        assert_eq!(m.idx.len(), plain.idx.len() + 6);
        let vs = verts(&m);
        for v in &vs[..4] {
            assert_eq!((v.aux, v.id), (FACE_BACKDROP, BACKDROP_ID));
            assert!(v.pos.x.abs() == 1.0 && v.pos.y.abs() == 1.0 && v.pos.z == 0.0, "clip-space corners");
            assert!((0.0..=1.0).contains(&v.uv.x) && (0.0..=1.0).contains(&v.uv.y));
        }
        assert!(vs[4..].iter().all(|v| v.aux != FACE_BACKDROP));
        assert!(m.idx[..6].iter().all(|&i| i < 4), "drawn first");
        for (k, &i) in plain.idx.iter().enumerate() {
            assert_eq!(m.idx[6 + k], i + 4);
        }
    }

    #[test]
    fn the_floor_mirrors_every_letter_and_lies_under_them() {
        let (mut l, families) = layouter();
        let laid = layout(&mut l, families[1], "Floor", None);
        let mut plain = FxMesh::default();
        build_text_mesh(&laid, &TextMeshParams::default(), &mut plain);
        let params = TextMeshParams { floor: Some(FloorParams { grid: 8, ..Default::default() }), ..Default::default() };
        let mut mesh = FxMesh::default();
        let report = build_text_mesh(&laid, &params, &mut mesh);
        let (n, il) = (plain.vertex_count(), plain.idx.len());
        assert_eq!(report.floor_grid, 8);
        assert_eq!(mesh.vertex_count(), 2 * n + 81);
        assert_eq!(report.vertices, mesh.vertex_count());
        let (v0, vs) = (verts(&plain), verts(&mesh));
        for i in 0..n {
            let (a, b, m) = (v0[i], vs[i], vs[n + i]);
            assert!(a.pos == b.pos && a.aux == b.aux, "letters come first, unchanged");
            assert!(m.pos == a.pos && m.n == a.n && m.uv == a.uv && m.id == a.id && m.r0 == a.r0 && m.r1 == a.r1);
            assert_eq!(m.aux, FACE_MIRROR + a.aux / MIRROR_CLASS_SCALE);
            assert_eq!(((m.aux - FACE_MIRROR) * MIRROR_CLASS_SCALE).round(), a.aux);
        }
        assert_eq!(&mesh.idx[..il], &plain.idx[..]);
        for t in 0..il / 3 {
            let (a, b, c) = (plain.idx[3 * t], plain.idx[3 * t + 1], plain.idx[3 * t + 2]);
            let base = n as u32;
            assert_eq!(&mesh.idx[il + 3 * t..il + 3 * t + 3], &[a + base, c + base, b + base], "mirrors are rewound");
        }
        let floor: Vec<&V> = vs[2 * n..].iter().collect();
        let half = 0.5 * report.floor_size;
        for v in &floor {
            assert_eq!((v.aux, v.id), (FACE_FLOOR, FLOOR_ID));
            assert_eq!(v.pos.y, report.floor_y);
            assert!(close3(v.n, vec3f(0.0, 1.0, 0.0), 1e-6));
            assert_eq!((v.uv.x, v.uv.y), (v.pos.x, v.pos.z));
            assert!(v.pos.x.abs() <= half + 1e-4 && v.pos.z.abs() <= half + 1e-4);
        }
        assert!(floor.iter().any(|v| (v.pos.x + half).abs() < 1e-4) && floor.iter().any(|v| (v.pos.z - half).abs() < 1e-4));
        for t in mesh.idx[2 * il..].chunks(3) {
            let (a, b, c) = (vs[t[0] as usize].pos, vs[t[1] as usize].pos, vs[t[2] as usize].pos);
            let (e1, e2) = (vec3f(b.x - a.x, 0.0, b.z - a.z), vec3f(c.x - a.x, 0.0, c.z - a.z));
            assert!(e1.z * e2.x - e1.x * e2.z > 0.0, "the floor faces up");
        }
        assert_eq!(mesh.idx.len(), 2 * il + 6 * 64);
        // Auto: just under the lowest descender, three text widths wide.
        let lowest = v0.iter().map(|v| v.uv.y + v.pos.y).fold(f32::MAX, f32::min);
        assert!(report.floor_y < lowest && report.floor_y > lowest - 0.8 * params.size, "{} vs {lowest}", report.floor_y);
        assert!((report.floor_size - 3.0 * report.width).abs() < 1e-4);
        // Explicit placement is honoured.
        let mut m = FxMesh::default();
        let rep = build_text_mesh(
            &laid,
            &TextMeshParams { floor: Some(FloorParams { y: Some(-2.0), size: Some(10.0), grid: 2 }), ..Default::default() },
            &mut m,
        );
        assert_eq!((rep.floor_y, rep.floor_size, rep.floor_grid), (-2.0, 10.0, 2));
        assert_eq!(m.vertex_count(), 2 * n + 9);
        // A floor grid the budget cannot carry shrinks, and says so.
        let rep = build_text_mesh(
            &laid,
            &TextMeshParams { floor: Some(FloorParams { grid: 256, ..Default::default() }), vert_budget: 20_000, ..Default::default() },
            &mut m,
        );
        assert!(rep.floor_grid < 256 && rep.vertices <= 20_000);
        assert!(rep.warnings[0].contains("floor grid"), "{:?}", rep.warnings);
    }

    /// Cubes of a voxel mesh: its vertices in groups of 24 (4 when flat).
    fn cubes_of(mesh: &FxMesh, flat: bool) -> Vec<Vec<V>> {
        verts(mesh).chunks(if flat { 4 } else { 24 }).map(|c| c.to_vec()).collect()
    }

    #[test]
    fn voxel_letters_fill_the_caps_inside_a_block_of_waste() {
        let (mut l, families) = layouter();
        let laid = layout(&mut l, families[1], "HI", None);
        let params = TextMeshParams { layout: TextLayout::Voxel, voxel_res: 20, voxel_layers: 2, ..Default::default() };
        let mut mesh = FxMesh::default();
        let report = build_text_mesh(&laid, &params, &mut mesh);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        let set = report.voxels.clone().expect("a voxel build reports its cubes");
        let half = report.voxel_half;
        let cell = 2.0 * half[0];
        assert!((cell - 1.0 / 20.0).abs() < 1e-6 && half[1] == half[0]);
        assert!((half[2] - 0.35 / 4.0).abs() < 1e-6, "two layers span the depth");
        assert_eq!((report.voxel_res, report.voxel_layers), (20.0, 2));
        assert_eq!(set.len(), report.voxel_cubes);
        assert_eq!(report.vertices, 24 * report.voxel_cubes);
        let (nx, ny) = ((report.width / cell).round() as usize, (report.height / cell).round() as usize);
        assert_eq!(nx * ny * 2, report.voxel_cubes, "the whole block is filled");
        let letters = set.letter.iter().filter(|b| **b).count();
        assert!(letters > 0 && letters < set.len());
        assert!(set.letter[..letters].iter().all(|b| *b), "letter cubes come first");
        let cubes = cubes_of(&mesh, false);
        assert_eq!(cubes.len(), set.len());
        let (x_lo, x_hi) = (-0.5 * report.width + 0.5 * cell, 0.5 * report.width - 0.5 * cell);
        for (k, cube) in cubes.iter().enumerate() {
            let c0 = cube[0];
            let layer = c0.r1;
            let z = half[2] * (2.0 - 1.0 - 2.0 * layer);
            assert!(cube.iter().all(|v| v.uv == c0.uv && v.n == c0.n && v.id == c0.id && v.aux == c0.aux && v.r1 == c0.r1));
            assert!(close3(c0.n, vec3f(c0.uv.x, c0.uv.y, z), 1e-6), "no previous set: FROM = TO");
            assert_eq!(set.centres[k], [c0.uv.x, c0.uv.y, z]);
            assert_eq!(set.layers[k] as f32, layer);
            assert_eq!(c0.aux, if set.letter[k] { FACE_VOXEL_LETTER } else { FACE_VOXEL_WASTE });
            let centroid = cube.iter().fold(vec3f(0.0, 0.0, 0.0), |a, v| vec3f(a.x + v.pos.x, a.y + v.pos.y, a.z + v.pos.z));
            assert!(norm3(centroid) < 1e-5, "the local frame is the cube's centre");
            for face in cube.chunks(4) {
                let (a, b, d) = (face[0].pos, face[1].pos, face[3].pos);
                let (e1, e2) = (vec3f(b.x - a.x, b.y - a.y, b.z - a.z), vec3f(d.x - a.x, d.y - a.y, d.z - a.z));
                let g = vec3f(e1.y * e2.z - e1.z * e2.y, e1.z * e2.x - e1.x * e2.z, e1.x * e2.y - e1.y * e2.x);
                let gl = norm3(g);
                let g = vec3f(g.x / gl, g.y / gl, g.z / gl);
                for v in face {
                    assert!(close3(derived_face(v.pos, half), g, 1e-5), "the shader's face rule fails at {:?}", v.pos);
                }
            }
            // Waste at the block's ends belongs to the glyph beside it.
            if !set.letter[k] && (c0.uv.x - x_lo).abs() < 1e-4 {
                assert_eq!(c0.id, 0.0);
            }
            if !set.letter[k] && (c0.uv.x - x_hi).abs() < 1e-4 {
                assert_eq!(c0.id, 1.0);
            }
        }
        // The letter cells cover the cap: an O at 40 cells per cap height.
        let one = layout(&mut l, families[0], "O", None);
        let p = TextMeshParams { layout: TextLayout::Voxel, voxel_res: 40, voxel_layers: 1, ..Default::default() };
        let mut m = FxMesh::default();
        let r = build_text_mesh(&one, &p, &mut m);
        let s = r.voxels.unwrap();
        let area = s.letter.iter().filter(|b| **b).count() as f32 * (2.0 * r.voxel_half[0]).powi(2);
        let (shoelace, _, _) = reference_areas(&one, &p);
        assert!((area - shoelace).abs() / shoelace < 0.08, "voxel area {area} vs cap {shoelace}");
        // Flat voxels: one +z quad per cell at z 0.
        let flat = TextMeshParams { depth: 0.0, voxel_layers: 1, ..params.clone() };
        let mut f = FxMesh::default();
        let rf = build_text_mesh(&laid, &flat, &mut f);
        assert_eq!(rf.vertices, 4 * rf.voxel_cubes);
        assert_eq!(rf.voxel_cubes, nx * ny);
        assert_eq!(rf.voxel_half[2], 0.0);
        for quad in cubes_of(&f, true) {
            assert!(quad.iter().all(|v| v.pos.z == 0.0));
            let (a, b, d) = (quad[0].pos, quad[1].pos, quad[3].pos);
            assert!((b.x - a.x) * (d.y - a.y) - (b.y - a.y) * (d.x - a.x) > 0.0, "flat voxels face +z");
        }
    }

    #[test]
    fn voxel_budget_coarsens_the_lattice_and_says_so() {
        let (mut l, families) = layouter();
        let laid = layout(&mut l, families[0], "BUDGET", None);
        let full = TextMeshParams {
            layout: TextLayout::Voxel,
            voxel_res: 40,
            voxel_layers: 4,
            vert_budget: 10_000_000,
            ..Default::default()
        };
        let mut mesh = FxMesh::default();
        let base = build_text_mesh(&laid, &full, &mut mesh);
        assert!(base.warnings.is_empty(), "{:?}", base.warnings);
        let budget = base.vertices / 5;
        let report = build_text_mesh(&laid, &TextMeshParams { vert_budget: budget, ..full.clone() }, &mut mesh);
        println!("{:?}", report.warnings);
        assert!(!report.warnings.is_empty());
        assert!(report.warnings.iter().all(|w| w.contains("voxel_res coarsened")));
        assert!((report.voxel_res - 40.0 * 0.75f32.powi(report.warnings.len() as i32)).abs() < 1e-3);
        assert!(report.vertices <= budget && report.vertices == mesh.vertex_count());
        // A budget no lattice fits: coarsened all the way, then truncated.
        let tiny = build_text_mesh(&laid, &TextMeshParams { vert_budget: 64, ..full.clone() }, &mut mesh);
        assert!(tiny.vertices <= 64);
        assert!(tiny.warnings.last().unwrap().contains("voxels dropped"), "{:?}", tiny.warnings);
    }

    #[test]
    fn voxel_morph_carries_the_old_cubes_to_the_new_text() {
        let (mut l, families) = layouter();
        let ab = layout(&mut l, families[1], "AB", None);
        let abc = layout(&mut l, families[1], "ABC", None);
        let base = TextMeshParams { layout: TextLayout::Voxel, voxel_res: 10, voxel_layers: 2, ..Default::default() };
        let mut m = FxMesh::default();
        let old = build_text_mesh(&ab, &base, &mut m).voxels.unwrap();
        let new = build_text_mesh(&abc, &base, &mut m).voxels.unwrap();
        let key = |c: [f32; 3]| (c[0].to_bits(), c[1].to_bits(), c[2].to_bits());
        let from_of = |cube: &Vec<V>| [cube[0].n.x, cube[0].n.y, cube[0].n.z];
        // AB -> ABC: more cubes, every old one travels, the extra ones are born.
        let p = TextMeshParams { previous_voxels: Some(old.clone()), ..base.clone() };
        let mut mesh = FxMesh::default();
        let r = build_text_mesh(&abc, &p, &mut mesh);
        assert_eq!(r.voxel_cubes, old.len().max(new.len()), "the pool is the larger set");
        assert_eq!(r.voxels.as_ref(), Some(&new), "the reported set is the new text's own");
        assert!(r.morph_born > 0);
        assert_eq!(r.morph_dying, 0);
        let cubes = cubes_of(&mesh, false);
        assert_eq!(cubes.len(), r.voxel_cubes);
        let old_keys: HashSet<_> = old.centres.iter().map(|c| key(*c)).collect();
        let mut travelled = HashSet::new();
        let mut born = 0;
        for (k, cube) in cubes.iter().enumerate() {
            let from = from_of(cube);
            assert!(from.iter().all(|f| f.is_finite()));
            assert!(old_keys.contains(&key(from)), "cube {k} starts from no old cube");
            match cube[0].aux.floor() {
                c if c == FACE_VOXEL_BORN => born += 1,
                c if c == FACE_VOXEL_LETTER || c == FACE_VOXEL_WASTE => {
                    travelled.insert(key(from));
                }
                c => panic!("unexpected class {c}"),
            }
            // TO is the new cube's rest centre.
            assert_eq!([cube[0].uv.x, cube[0].uv.y], [new.centres[k][0], new.centres[k][1]]);
        }
        assert_eq!(born, r.morph_born);
        assert_eq!(travelled.len(), old.len(), "every old cube is paired");
        // The same inputs give the same floats.
        let mut again = FxMesh::default();
        build_text_mesh(&abc, &p, &mut again);
        assert_eq!(mesh.verts, again.verts);
        assert_eq!(mesh.idx, again.idx);
        // ABC -> AB: fewer cubes; the surplus old ones die travelling to
        // the nearest new cube, appended after the new text's own cubes.
        let q = TextMeshParams { previous_voxels: Some(new.clone()), ..base.clone() };
        let mut back = FxMesh::default();
        let r2 = build_text_mesh(&ab, &q, &mut back);
        assert_eq!(r2.voxel_cubes, old.len().max(new.len()));
        assert!(r2.morph_dying > 0);
        assert_eq!(r2.morph_born, 0);
        let cubes = cubes_of(&back, false);
        let new_keys: HashSet<_> = old.centres.iter().map(|c| key(*c)).collect();
        let mut froms = Vec::new();
        for (k, cube) in cubes.iter().enumerate() {
            let dying = cube[0].aux.floor() == FACE_VOXEL_DYING;
            assert_eq!(dying, k >= old.len(), "dying cubes follow the new ones");
            let z = r2.voxel_half[2] * (2.0 - 1.0 - 2.0 * cube[0].r1);
            assert!(new_keys.contains(&key([cube[0].uv.x, cube[0].uv.y, z])), "cube {k} travels to no new cube");
            froms.push(key(from_of(cube)));
        }
        froms.sort();
        let mut all_new: Vec<_> = new.centres.iter().map(|c| key(*c)).collect();
        all_new.sort();
        assert_eq!(froms, all_new, "every old cube leaves exactly once");
    }

    #[test]
    fn voxel_waste_off_builds_the_letter_cubes_alone() {
        let (mut l, families) = layouter();
        let hi = layout(&mut l, families[1], "HI", None);
        let with = TextMeshParams { layout: TextLayout::Voxel, voxel_res: 20, voxel_layers: 2, ..Default::default() };
        let without = TextMeshParams { voxel_waste: false, ..with.clone() };
        let mut m = FxMesh::default();
        let full = build_text_mesh(&hi, &with, &mut m);
        let full_set = full.voxels.clone().unwrap();
        let letters = full_set.letter.iter().filter(|b| **b).count();
        assert!(letters > 0 && letters < full_set.len(), "the block has waste to leave out");
        let mut mesh = FxMesh::default();
        let r = build_text_mesh(&hi, &without, &mut mesh);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        let set = r.voxels.clone().unwrap();
        // The cube count is the letter count; the set carries no waste.
        assert_eq!(r.voxel_cubes, letters);
        assert_eq!(set.len(), letters);
        assert!(set.letter.iter().all(|b| *b), "no waste in the set");
        assert_eq!(r.vertices, 24 * letters, "cheaper: the waste is never built");
        // The letter cubes are the same cubes, in the same order, and the
        // block keeps its extents.
        assert_eq!(&set.centres[..], &full_set.centres[..letters]);
        assert_eq!((r.width, r.height, r.voxel_half), (full.width, full.height, full.voxel_half));
        let cubes = cubes_of(&mesh, false);
        assert_eq!(cubes.len(), letters);
        assert!(cubes.iter().all(|c| c[0].aux == FACE_VOXEL_LETTER));

        // The morph still pairs: AB -> ABC, letters with letters.
        let ab = layout(&mut l, families[1], "AB", None);
        let abc = layout(&mut l, families[1], "ABC", None);
        let base = TextMeshParams { voxel_res: 10, ..without.clone() };
        let old = build_text_mesh(&ab, &base, &mut m).voxels.unwrap();
        let new = build_text_mesh(&abc, &base, &mut m).voxels.unwrap();
        assert!(old.letter.iter().chain(new.letter.iter()).all(|b| *b));
        let key = |c: [f32; 3]| (c[0].to_bits(), c[1].to_bits(), c[2].to_bits());
        let old_keys: HashSet<_> = old.centres.iter().map(|c| key(*c)).collect();
        let p = TextMeshParams { previous_voxels: Some(old.clone()), ..base.clone() };
        let mut mm = FxMesh::default();
        let r = build_text_mesh(&abc, &p, &mut mm);
        assert_eq!(r.voxel_cubes, new.len().max(old.len()));
        assert!(r.morph_born > 0 && r.morph_dying == 0);
        let mut travelled = HashSet::new();
        for (k, cube) in cubes_of(&mm, false).iter().enumerate() {
            let from = [cube[0].n.x, cube[0].n.y, cube[0].n.z];
            assert!(old_keys.contains(&key(from)), "cube {k} starts from no old letter cube");
            match cube[0].aux {
                c if c == FACE_VOXEL_LETTER => {
                    travelled.insert(key(from));
                }
                c if c == FACE_VOXEL_BORN => {}
                c => panic!("cube {k}: class {c}, only letters and born letters without waste"),
            }
        }
        assert_eq!(travelled.len(), old.len(), "every old letter cube is paired");
        // An old set built WITH waste: its waste cubes are left out, the
        // letters still pair, nothing emitted is waste.
        let old_full = build_text_mesh(&ab, &TextMeshParams { voxel_waste: true, ..base.clone() }, &mut m).voxels.unwrap();
        assert!(old_full.letter.iter().any(|b| !b));
        let q = TextMeshParams { previous_voxels: Some(old_full.clone()), ..base.clone() };
        let mut mq = FxMesh::default();
        let rq = build_text_mesh(&abc, &q, &mut mq);
        assert_eq!(rq.voxel_cubes, new.len().max(old.len()));
        assert_eq!(rq.voxels.as_ref(), Some(&new));
        for cube in cubes_of(&mq, false) {
            let class = cube[0].aux;
            assert!(class == FACE_VOXEL_LETTER || class == FACE_VOXEL_BORN || class == FACE_VOXEL_DYING, "{class}");
            assert!(old_keys.contains(&key([cube[0].n.x, cube[0].n.y, cube[0].n.z])));
        }
    }

    #[test]
    fn new_layouts_survive_nonsense_params() {
        let (mut l, families) = layouter();
        let laid = layout(&mut l, families[3], "safe @ all costs", None);
        let junk_set = VoxelSet { centres: vec![[f32::NAN, 0.0, 0.0]], ..Default::default() };
        for layout in [TextLayout::Helix, TextLayout::Grid, TextLayout::Voxel, TextLayout::Line, TextLayout::Cloud] {
            for copies in [0usize, 3, 1000] {
                let params = TextMeshParams {
                    size: f32::NAN,
                    depth: f32::INFINITY,
                    bevel: 5.0,
                    detail: f32::NAN,
                    layout,
                    ring_radius: Some(-1.0),
                    helix_pitch: f32::NAN,
                    grid_pitch: -3.0,
                    grid_cols: usize::MAX,
                    copies,
                    bevel_rings: 0,
                    bevel_type: BevelType::Ogee,
                    voxel_res: 0,
                    voxel_layers: 99,
                    floor: Some(FloorParams { y: Some(f32::NAN), size: Some(f32::INFINITY), grid: 0 }),
                    previous_voxels: Some(junk_set.clone()),
                    changed: vec![true; 3],
                    vert_budget: 5000,
                    ..Default::default()
                };
                let mut mesh = FxMesh::default();
                let report = build_text_mesh(&laid, &params, &mut mesh);
                assert!(mesh.verts.iter().all(|f| f.is_finite()), "{layout:?} x{copies} leaked non-finite data");
                assert!(report.vertices <= 5000, "{layout:?} x{copies}: {}", report.vertices);
                for f in [report.width, report.height, report.floor_y, report.floor_size, report.copy_radius, report.voxel_res] {
                    assert!(f.is_finite());
                }
            }
        }
        // No ink: no cubes; morphing out of a text into nothing shrinks
        // every old cube where it stands.
        let empty = layout(&mut l, families[0], "   ", None);
        let vox = TextMeshParams { layout: TextLayout::Voxel, ..Default::default() };
        let mut mesh = FxMesh::default();
        let r = build_text_mesh(&empty, &vox, &mut mesh);
        assert_eq!((r.voxel_cubes, mesh.vertex_count()), (0, 0));
        assert_eq!(r.voxels.map(|s| s.len()), Some(0));
        let ab = layout(&mut l, families[0], "AB", None);
        let old = build_text_mesh(&ab, &vox, &mut mesh).voxels.unwrap();
        let r = build_text_mesh(&empty, &TextMeshParams { previous_voxels: Some(old.clone()), ..vox.clone() }, &mut mesh);
        assert_eq!((r.voxel_cubes, r.morph_dying), (old.len(), old.len()));
        for cube in cubes_of(&mesh, false) {
            assert_eq!(cube[0].aux.floor(), FACE_VOXEL_DYING);
            assert_eq!([cube[0].n.x, cube[0].n.y], [cube[0].uv.x, cube[0].uv.y], "dies in place");
        }
    }

    #[test]
    fn tracking_widens_every_advance_and_zero_is_bit_exact() {
        let (mut l, families) = layouter();
        let laid = layout(&mut l, families[0], "ABCD", None);
        let em_world = laid.rows[0].glyphs[0].font_size_in_lpxs * 1.0 / cap_height_lpx(&laid);
        let mut plain = FxMesh::default();
        let r0 = build_text_mesh(&laid, &TextMeshParams::default(), &mut plain);
        let mut zero = FxMesh::default();
        build_text_mesh(&laid, &TextMeshParams { tracking: 0.0, ..Default::default() }, &mut zero);
        assert_eq!(fingerprint(&plain), fingerprint(&zero), "tracking 0 moved the geometry");
        for tracking in [0.25f32, 0.5, -0.1] {
            let mut m = FxMesh::default();
            let r = build_text_mesh(&laid, &TextMeshParams { tracking, ..Default::default() }, &mut m);
            // Four glyphs: the last one moved three advances of `tracking` em.
            let want = r0.width + 3.0 * tracking * em_world;
            assert!((r.width - want).abs() < 1e-3, "tracking {tracking}: width {} want {want}", r.width);
            assert_eq!(m.vertex_count(), plain.vertex_count(), "tracking only moves glyphs");
            // The glyph centres stay evenly re-spaced: neighbours differ by
            // the plain spacing plus `tracking` em.
            let centres = |mesh: &FxMesh| {
                let mut c: Vec<(f32, f32)> = verts(mesh).iter().map(|v| (v.id, v.uv.x)).collect();
                c.dedup_by(|a, b| a.0 == b.0);
                c
            };
            let (a, b) = (centres(&plain), centres(&m));
            for k in 1..a.len() {
                let d = (b[k].1 - b[k - 1].1) - (a[k].1 - a[k - 1].1);
                assert!((d - tracking * em_world).abs() < 1e-3, "gap {k} grew by {d}");
            }
        }
        // Nonsense stays finite and clamped.
        let mut m = FxMesh::default();
        let r = build_text_mesh(&laid, &TextMeshParams { tracking: f32::NAN, ..Default::default() }, &mut m);
        assert_eq!(r.width, r0.width);
        let r = build_text_mesh(&laid, &TextMeshParams { tracking: 1e9, ..Default::default() }, &mut m);
        assert!((r.width - (r0.width + 3.0 * 2.0 * em_world)).abs() < 1e-2);
        // The ring's auto radius follows the tracked row width.
        let ring = TextMeshParams { layout: TextLayout::Ring, ..Default::default() };
        let r_plain = build_text_mesh(&laid, &ring, &mut m).ring_radius;
        let r_wide = build_text_mesh(&laid, &TextMeshParams { tracking: 0.5, ..ring.clone() }, &mut m).ring_radius;
        assert!(r_wide > r_plain, "ring radius {r_wide} should grow past {r_plain}");
    }

    /// The shape store law: a rebuild over glyphs this thread has shaped
    /// before (the same pick again, a scramble step over the same letters,
    /// another layout at the same detail, a budget pass) shapes nothing
    /// new, and a store hit gives the same floats as a cold build.
    #[test]
    fn a_rebuild_over_seen_glyphs_shapes_nothing_and_changes_no_float() {
        let params = TextMeshParams { depth: 0.3, bevel: 0.05, ..Default::default() };
        // Cold: a fresh thread (an empty store) builds "DECODE".
        let cold = {
            let params = params.clone();
            std::thread::spawn(move || {
                let (mut l, families) = layouter();
                let laid = layout(&mut l, families[0], "DECODE", None);
                let mut m = FxMesh::default();
                build_text_mesh(&laid, &params, &mut m);
                (fingerprint(&m), shapes_built())
            })
            .join()
            .unwrap()
        };
        assert!(cold.1 > 0, "a cold build shapes its glyphs");
        let (mut l, families) = layouter();
        // Warm the store with the same letters in another order.
        let warm_up = layout(&mut l, families[0], "CODED", None);
        let mut m = FxMesh::default();
        build_text_mesh(&warm_up, &params, &mut m);
        let before = shapes_built();
        let laid = layout(&mut l, families[0], "DECODE", None);
        build_text_mesh(&laid, &params, &mut m);
        assert_eq!(shapes_built(), before, "a scramble step over seen letters shaped glyphs");
        assert_eq!(fingerprint(&m), cold.0, "a store hit changed the geometry");
        // The same pick again, ten times: nothing shaped.
        for _ in 0..10 {
            m.clear();
            build_text_mesh(&laid, &params, &mut m);
        }
        assert_eq!(shapes_built(), before, "an unchanged pick shaped glyphs");
        assert_eq!(fingerprint(&m), cold.0);
        // Other layouts at the same detail share the shapes.
        for layout_kind in [TextLayout::Ring, TextLayout::Helix, TextLayout::Grid, TextLayout::Voxel] {
            build_text_mesh(&laid, &TextMeshParams { layout: layout_kind, ..params.clone() }, &mut m);
        }
        assert_eq!(shapes_built(), before, "another layout re-shaped seen glyphs");
        // A word cloud of seen words too.
        let words: Vec<CloudWord> = ["CODE", "DECODE"]
            .iter()
            .map(|w| CloudWord { text: layout(&mut l, families[0], w, None), weight: 1.0 })
            .collect();
        build_cloud_mesh(&words, &params, &mut m);
        assert_eq!(shapes_built(), before, "the cloud re-shaped seen glyphs");
        // A new detail is a new shape.
        build_text_mesh(&laid, &TextMeshParams { detail: 0.03, ..params.clone() }, &mut m);
        assert!(shapes_built() > before, "a new detail must shape again");
    }

    /// The store is bounded: past `SHAPE_STORE_MAX` it empties and refills,
    /// and a build after that is still exact.
    #[test]
    fn the_shape_store_stays_bounded() {
        let (mut l, families) = layouter();
        let laid = layout(&mut l, families[0], &printable_ascii(), None);
        let mut m = FxMesh::default();
        let glyphs = laid.rows.iter().map(|r| r.glyphs.len()).sum::<usize>();
        let rounds = SHAPE_STORE_MAX / glyphs.max(1) + 2;
        for k in 0..rounds {
            let detail = 0.004 + 0.0004 * k as f32;
            let mut cache = ShapeCache::default();
            for g in laid.rows.iter().flat_map(|r| r.glyphs.iter()) {
                glyph_shape(g, detail, &mut cache);
            }
            assert!(shapes_stored() <= SHAPE_STORE_MAX, "store holds {}", shapes_stored());
        }
        let small = layout(&mut l, families[0], "Bound", None);
        build_text_mesh(&small, &TextMeshParams::default(), &mut m);
        let again = fingerprint(&m);
        m.clear();
        build_text_mesh(&small, &TextMeshParams::default(), &mut m);
        assert_eq!(fingerprint(&m), again);
    }

    /// The distinct shaped glyphs of a laid-out text (glyph id 0 skipped).
    fn distinct_glyphs(text: &LaidoutText) -> usize {
        let keys: HashSet<(usize, u16)> = text
            .rows
            .iter()
            .flat_map(|r| r.glyphs.iter())
            .filter(|g| g.id != 0)
            .map(|g| (Rc::as_ptr(&g.font) as usize, g.id))
            .collect();
        keys.len()
    }

    /// The warm-up queue drains in queue order (a pick queued `first` ahead
    /// of the ASCII set behind it) and stops at the first shape that ends
    /// past the budget, never building fewer than one.
    #[test]
    fn the_warmup_drains_in_order_and_stops_at_the_budget() {
        std::thread::spawn(|| {
            let (mut l, families) = layouter();
            let detail = sane_detail(0.012);
            let ascii = layout(&mut l, families[0], PRINTABLE_ASCII, None);
            let pick = layout(&mut l, families[0], "zyx", None);
            let mut w = ShapeWarmup::default();
            let queued = w.queue_text(&ascii, detail, false);
            assert_eq!(queued, distinct_glyphs(&ascii));
            assert!(queued >= 90, "the ASCII set queued {queued} glyphs");
            // The pick goes first, moving its letters out of the ASCII run.
            assert_eq!(w.queue_text(&pick, detail, true), 3);
            assert_eq!(w.len(), queued, "a moved glyph was queued twice");
            // A clock that moves one second per read: start 0, then 1, 2, 3.
            let mut t = 0.0;
            let mut clock = move || {
                let now = t;
                t += 1.0;
                now
            };
            let before = shapes_built();
            assert_eq!(w.drain(2.5, &mut clock), 3, "a 2.5 s budget at 1 s per shape builds 3");
            assert_eq!(shapes_built(), before + 3);
            // Those three were the pick's.
            assert!(ShapeWarmup::text_is_warm(&pick, detail), "the pick was not warmed first");
            // Next in line is the ASCII set's first glyph, the space, then '!'.
            let space = layout(&mut l, families[0], " ", None);
            let bang = layout(&mut l, families[0], "!", None);
            assert!(!ShapeWarmup::text_is_warm(&space, detail));
            assert!(!ShapeWarmup::text_is_warm(&bang, detail));
            // A zero budget still builds one shape per call.
            let mut clock = || 0.0;
            assert_eq!(w.drain(0.0, &mut clock), 1);
            assert!(ShapeWarmup::text_is_warm(&space, detail), "the queue skipped the space");
            assert!(!ShapeWarmup::text_is_warm(&bang, detail), "one call built two shapes");
            assert_eq!(w.drain(0.0, &mut clock), 1);
            assert!(ShapeWarmup::text_is_warm(&bang, detail), "the queue skipped ahead of '!'");
            // An endless budget empties it; an empty queue builds nothing.
            let rest = w.len();
            assert_eq!(w.drain(f64::INFINITY, &mut clock), rest);
            assert!(w.is_empty());
            assert_eq!(w.drain(f64::INFINITY, &mut clock), 0);
            assert!(ShapeWarmup::text_is_warm(&ascii, detail));
        })
        .join()
        .unwrap();
    }

    /// A shape the warm-up built is the shape a build makes: a build after
    /// a full warm-up shapes nothing and gives the same floats as a cold
    /// build on an empty store.
    #[test]
    fn warmed_shapes_are_the_floats_a_build_makes() {
        let params = TextMeshParams { depth: 0.3, bevel: 0.05, ..Default::default() };
        let text = "Warm Shapes 0123 &@%";
        let cold = {
            let params = params.clone();
            std::thread::spawn(move || {
                let (mut l, families) = layouter();
                let laid = layout(&mut l, families[0], text, None);
                let mut m = FxMesh::default();
                build_text_mesh(&laid, &params, &mut m);
                fingerprint(&m)
            })
            .join()
            .unwrap()
        };
        std::thread::spawn(move || {
            let (mut l, families) = layouter();
            let laid = layout(&mut l, families[0], text, None);
            let detail = sane_detail(params.detail);
            let mut w = ShapeWarmup::default();
            assert!(!ShapeWarmup::text_is_warm(&laid, detail));
            w.queue_text(&laid, detail, true);
            let mut clock = || 0.0;
            while !w.is_empty() {
                w.drain(0.0, &mut clock);
            }
            assert!(ShapeWarmup::text_is_warm(&laid, detail));
            let before = shapes_built();
            let mut m = FxMesh::default();
            build_text_mesh(&laid, &params, &mut m);
            assert_eq!(shapes_built(), before, "the build after a warm-up shaped glyphs");
            assert_eq!(fingerprint(&m), cold, "a warmed shape changed the geometry");
        })
        .join()
        .unwrap();
    }

    /// An unchanged document queues nothing twice: queueing the same text
    /// again adds nothing while it waits, and nothing once it is warm.
    #[test]
    fn an_unchanged_document_queues_nothing_twice() {
        std::thread::spawn(|| {
            let (mut l, families) = layouter();
            let detail = sane_detail(0.012);
            let ascii = layout(&mut l, families[0], PRINTABLE_ASCII, None);
            let pick = layout(&mut l, families[0], "Hello hello", None);
            let mut w = ShapeWarmup::default();
            let distinct = w.queue_text(&pick, detail, true);
            assert_eq!(distinct, distinct_glyphs(&pick));
            // The ASCII set adds only what the pick did not already queue.
            let n = w.queue_text(&ascii, detail, false);
            assert_eq!(n, distinct_glyphs(&ascii) - distinct);
            assert_eq!(w.queue_text(&ascii, detail, false), 0, "the ASCII set queued twice");
            assert_eq!(w.queue_text(&pick, detail, false), 0, "the pick queued twice");
            assert_eq!(w.len(), distinct + n);
            let mut clock = || 0.0;
            w.drain(f64::INFINITY, &mut clock);
            assert!(w.is_empty());
            assert_eq!(w.queue_text(&pick, detail, true), 0, "a warm pick queued again");
            assert_eq!(w.queue_text(&ascii, detail, false), 0, "a warm set queued again");
            // Another detail is another set of shapes.
            assert!(w.queue_text(&pick, sane_detail(0.03), true) > 0);
        })
        .join()
        .unwrap();
    }
}
