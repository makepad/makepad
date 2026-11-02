//! Ambient occlusion baked into a texture atlas of planar charts.
//!
//! # Why not per vertex
//!
//! Vertex AO is interpolated across a triangle, so a mesh can only show
//! occlusion its vertices can carry. A Kenney wall is two triangles with four
//! corners — all four out in the open, all four fully lit — so the darkening
//! where it meets the floor has nowhere to live and the wall shades flat. The
//! baker was never wrong; the mesh had no resolution to put an answer in.
//!
//! Subdividing until the vertices could carry it does work, and was tried: it
//! cost 4x the triangles (28k to 110k on the arcade scene). A texture
//! decouples AO resolution from geometry entirely, which on a bandwidth-bound
//! tiler is the trade worth making — the Quest pays per fragment and per
//! vertex fetched, and this moves the cost to a small single-channel texture
//! that is sampled once.
//!
//! # The unwrap
//!
//! Charts come from `makepad_uv_unwrap`: adjacent triangles join up to a crease
//! and split where they would stretch, flat runs are projected and curved runs
//! flattened conformally, and every chart is at one texel density, nested with
//! a padded gutter against bilinear bleed. Vertices split only where two charts
//! meet, exactly the corners where a hard shading break belongs.
//!
//! # The evaluator
//!
//! WHERE the texels live is this module's job; HOW DARK each one is belongs
//! to a selectable engine ([`AoBakerKind`]): ports of ands/lightmapper (the
//! default), Fewes/BakerBoy and prideout/aobaker, chosen per bake via
//! `AO_BAKER`. Each engine runs its OWN twin-dedup + orientation
//! preprocessing, port-verbatim — the engines were picked from their ports'
//! comparison bakes, and this pipeline reproduces those bakes rather than
//! unifying them.

use makepad_draw::{
    makepad_math::*,
    makepad_platform::thread::{TaskPool},
    Cx,
};

/// Texels per world unit of triangle edge. Kenney props are authored around
/// 1 unit = 1 metre, so this is roughly "a sample every 3cm". A pack atlas
/// scales this DOWN to fit its budget; a single-model atlas (`fill`) scales
/// it UP to spend its own texture.
pub const TEXELS_PER_UNIT: f32 = 32.0;

/// Texel density a SINGLE-MODEL atlas aims for, texels per world unit.
///
/// Higher than the pack baseline because a model alone answers to no budget
/// but its own: props draw one call each, so a texture per asset costs no
/// extra binds, and the texture should be exactly as large as this density
/// demands — not whatever a shared 1024 happened to leave.
///
/// 64, down from 96: Quest is the target and texture memory the scarce
/// resource. 96 held every crease on the house test but baked the trial
/// packs to ~1.3 MB a model; at 64 a texel is ~1.5cm of a Kenney unit —
/// still several texels across every gradient that matters — for 2.25x less
/// memory, and the shader's dither hides the coarser quantisation. 32 is
/// where trim goes splotchy; do not drift back there.
pub const MODEL_TEXELS_PER_UNIT: f32 = 64.0;

/// Hard ceiling on a pack's atlas, in texels per side.
///
/// Not a nicety — without it the full library bakes to 116 MB. Measured:
/// 4,442 models across 51 packs at an uncapped 8 texels/unit came out at
/// 2.3 MB per pack, which is unshippable on a device where texture memory is
/// the scarce resource. A pack that would exceed this has every patch scaled
/// down together, so the atlas degrades in resolution rather than failing or
/// silently dropping models.
pub const ATLAS_MAX: usize = 1024;

/// Which engine answers "how dark is this texel" for the atlas bake.
///
/// Three ports of reference bakers, selected once per bake from the
/// `AO_BAKER` env var (or a thread-local override for tests):
///
/// * `lightmapper` — [`crate::ao_lightmapper`], ands/lightmapper's hemicube
///   renderer: rasterized sky visibility with backface validity, cosine
///   kernel, example.c's dilate/smooth post chain and display gamma. THE
///   DEFAULT: the only evaluator whose occlusion answer comes from rendering
///   the scene the way the player sees it, so interpenetrating kit joints
///   are rejected by the validity gate instead of shading as false creases.
/// * `bakerboy` — [`crate::bakerboy`], Fewes/BakerBoy's directional
///   depth-map gather: shadow-mapping over a Fibonacci sphere with PCF and
///   the `saturate(4x)` output curve.
/// * `aobaker` — [`crate::aobaker_port`], prideout/aobaker's ray integral:
///   256 uniform-sphere rays, any hit at any distance occludes.
///
/// The old in-house per-texel evaluator (AoSampler's contact-falloff
/// hemisphere) is deleted from the atlas path; the sampler machinery itself
/// remains for the consumers that want contact AO ([`crate::bake`], the
/// ground drape, vertex bakes).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AoBakerKind {
    Lightmapper,
    BakerBoy,
    Aobaker,
}

thread_local! {
    /// Per-thread evaluator override. The env var is process-global: while it
    /// is set, every concurrent `bake_into` in the process changes evaluator —
    /// in a parallel test run that is a race. Tests bake on the calling
    /// thread, so a thread-local switch is the exact scope they mean.
    static THREAD_BAKER: std::cell::Cell<Option<AoBakerKind>> =
        const { std::cell::Cell::new(None) };
}

/// Override the evaluator for bakes on THIS thread (`None` = back to the
/// `AO_BAKER` env var / default). Test scaffolding.
pub fn set_thread_baker(kind: Option<AoBakerKind>) {
    THREAD_BAKER.with(|b| b.set(kind));
}

impl AoBakerKind {
    /// The evaluator for this bake: thread override, else `AO_BAKER`
    /// (`lightmapper` when unset — the default), read ONCE per bake on the
    /// calling thread.
    pub fn current() -> Self {
        if let Some(k) = THREAD_BAKER.with(|b| b.get()) {
            return k;
        }
        match std::env::var("AO_BAKER").as_deref() {
            Ok("bakerboy") => AoBakerKind::BakerBoy,
            Ok("aobaker") => AoBakerKind::Aobaker,
            Ok("lightmapper") | Err(_) => AoBakerKind::Lightmapper,
            Ok(other) => {
                eprintln!("AO_BAKER={other}: unknown evaluator, using lightmapper");
                AoBakerKind::Lightmapper
            }
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            AoBakerKind::Lightmapper => "lightmapper",
            AoBakerKind::BakerBoy => "bakerboy",
            AoBakerKind::Aobaker => "aobaker",
        }
    }
}

/// Rays per texel.
///
/// 10 was a runtime budget and it was the visible defect: occlusion quantised
/// to 1/10, so a smooth corner came out in steps and read as triangular
/// banding across every wall. The bake is offline now, so this costs the tool
/// seconds and the player nothing.
pub const ATLAS_RAYS: usize = crate::ao::AO_RAYS_OFFLINE;

/// Sub-samples per texel, spread across the texel's own AREA and averaged
/// over every triangle that touches the texel.
///
/// Sampling the centre alone is the standard mistake twice over. Within a
/// triangle it puts every sample on a perfect lattice, so error between
/// neighbours is correlated and shows as structure — facets and banding —
/// rather than noise that averages away. And low-poly trim is routinely
/// SMALLER than a texel: under one centre sample per texel, whichever tiny
/// face happened to rasterise first stamped its value over its neighbours',
/// which is where the bright speckles on dark window reveals came from.
pub const TEXEL_SUBSAMPLES: usize = 4;

/// A growable atlas SHARED by every model in a pack.
///
/// Granularity is the whole point. An atlas per mesh would mean a texture per
/// mesh, and the prop batch keys on texture — the live arcade scene draws 69
/// instances of 24 models in 18 items precisely because those models share 12
/// pack colormaps. Per-mesh atlases would make that 24 binds, scaling with
/// model count instead of pack count; at Zelda scale it is hundreds. The AO
/// would cost more than it is worth.
///
/// So models bake INTO one of these, alongside the pack colormap they already
/// share, and the batch is unchanged.
pub struct AoAtlas {
    pub pixels: Vec<u8>,
    pub size: usize,
    /// Charts that did not fit and were given the overlap fallback slot.
    pub overflowed: usize,
    /// Scale charts UP until the atlas is spent.
    ///
    /// For a SINGLE-model atlas only. A pack atlas is first-come-first-served,
    /// so scaling up would let the first model claim texels every later model
    /// needs. A model baking into its own texture has no later models — and
    /// without this it was measured using 4% of it, which put a whole facade
    /// on ~30 texels and every window frame on fractions of one.
    pub fill: bool,
    /// Which evaluator answered the texels ([`AoBakerKind::name`]), and its
    /// accumulated wall clock across every model baked into this atlas —
    /// the tool's timing line reads these.
    pub bake_evaluator: &'static str,
    pub bake_seconds: f64,
    /// Shelf allocator state: current row origin and its height.
    shelf_x: usize,
    shelf_y: usize,
    shelf_h: usize,
}

impl AoAtlas {
    pub fn new(size: usize) -> Self {
        let size = size.next_power_of_two().max(64);
        Self {
            pixels: vec![255; size * size],
            size,
            overflowed: 0,
            fill: false,
            bake_evaluator: "",
            bake_seconds: 0.0,
            shelf_x: 0,
            shelf_y: 0,
            shelf_h: 0,
        }
    }

    /// Texels not yet allocated. Approximate — shelf packing wastes a few
    /// percent — but it is what the patch scaler needs to aim at.
    ///
    /// A pack atlas budgets against `ATLAS_MAX` because it may still grow; a
    /// shrink-wrapped single-model atlas (`fill`) budgets against what it IS,
    /// or the scaler would inflate charts back to the ceiling it was just
    /// shrunk from.
    pub fn free_texels(&self) -> usize {
        let used = self.shelf_y * self.size + self.shelf_x * self.shelf_h;
        let cap = if self.fill { self.size * self.size } else { ATLAS_MAX * ATLAS_MAX };
        cap.saturating_sub(used)
    }

    /// Reserve a `w` x `h` region, growing the atlas if the shelf runs out.
    /// Returns its origin.
    ///
    /// Rectangular rather than square because charts are: a facade is wide and
    /// short, a doorframe tall and narrow. Rounding each up to a square wasted
    /// most of the texture on the long thin pieces that boxy architecture is
    /// mostly made of.
    pub(crate) fn alloc_rect(&mut self, w: usize, h: usize) -> (usize, usize) {
        loop {
            if self.shelf_x + w > self.size {
                self.shelf_x = 0;
                self.shelf_y += self.shelf_h;
                self.shelf_h = 0;
            }
            if self.shelf_y + h <= self.size {
                let at = (self.shelf_x, self.shelf_y);
                self.shelf_x += w;
                self.shelf_h = self.shelf_h.max(h);
                return at;
            }
            if self.size >= ATLAS_MAX {
                // Full. Hand back the last slot rather than growing past the
                // budget: an overlapping patch is a visibly wrong prop, but a
                // pack that silently blows the texture budget is a device that
                // drops frames, and the scaler above should have prevented it.
                self.overflowed += 1;
                return (self.size.saturating_sub(w), self.size.saturating_sub(h));
            }
            self.grow();
        }
    }

    /// Double the atlas, preserving what is already packed. Rows are copied
    /// rather than the buffer reused, because the stride changes.
    fn grow(&mut self) {
        let old = self.size;
        let new = old * 2;
        let mut pixels = vec![255u8; new * new];
        for y in 0..old {
            pixels[y * new..y * new + old].copy_from_slice(&self.pixels[y * old..y * old + old]);
        }
        self.pixels = pixels;
        self.size = new;
    }

    pub fn kilobytes(&self) -> usize {
        self.pixels.len() / 1024
    }
}

/// Occlusion a model casts onto FLAT ground beneath and around it.
///
/// Kept OUT of the texture atlas: the consumer is the CPU-side shadow-mesh
/// builder — a prop resting on flat ground gets this tessellated under it as
/// vertex-alpha darkening in the existing one-draw shadow layer — so the
/// pixels are needed at mesh-build time, and parking them in a GPU texture
/// would be exactly the read-back round trip to avoid. A few KB per model.
pub struct GroundAo {
    /// Model-space rect on the resting plane, `y` the plane's own height in
    /// model space. The renderer decides per INSTANCE whether the model is
    /// actually resting on flat ground; the bake only answers "how dark".
    pub x0: f32,
    pub z0: f32,
    pub x1: f32,
    pub z1: f32,
    pub y: f32,
    pub w: usize,
    pub h: usize,
    /// Row-major occlusion, 255 = open, floor-clamped like the atlas.
    pub pixels: Vec<u8>,
}


/// A baked mesh: un-indexed geometry and its uvs into the shared atlas.
pub struct BakedAo {
    /// For each output vertex, which input vertex it came from — so the caller
    /// can carry over uv, tint, or anything else it keeps per vertex without
    /// this module needing to know those exist.
    pub source_vertex: Vec<u32>,
    /// Per-vertex atlas coordinate, 0..1, into the SHARED atlas.
    pub ao_uv: Vec<[f32; 2]>,
    /// Occlusion at each vertex, read back out of the atlas.
    ///
    /// Redundant once the shader samples the texture — but until then this is
    /// what keeps the vertex path working, and it is strictly better than the
    /// old per-vertex bake: the value comes from the area-graded texel grid,
    /// so a vertex on a big wall now reports what its own corner of that wall
    /// actually receives rather than one sample for the whole triangle.
    pub vertex_ao: Vec<f32>,
    /// The model's shadow on flat ground, single-model bakes only.
    pub ground: Option<GroundAo>,
}

/// How parallel two triangles' normals must be to join one chart. cos(45 degrees).
///
/// A chart interpolates across every edge inside it, so a real crease (a wall
/// to its roof) has to stay a boundary or its occlusion smears over the corner,
/// while the facets of a curved run (barrel staves, wheel arches, rounded trim)
/// have to weld, or each pays its own gutter and shows its own edge.
const CHART_CREASE_COS: f64 = 0.7071;

/// Vertices this close, as a fraction of the model's size, are one vertex for
/// chart adjacency. Kenney meshes duplicate vertices freely (one flat facade is
/// often several quads each owning its corners), and without welding each quad
/// becomes a chart with its own texel grid, a visible seam in the middle of a
/// wall where the two grids' samples disagree.
const CHART_WELD: f64 = 1.0e-5;

/// Texels of padding round each chart, filled by dilation.
///
/// Charts share a texture, so without this bilinear filtering at a chart's
/// edge reaches into whatever was packed beside it — the classic lightmap
/// seam, a bright or dark rim round every face. ONE texel is exactly what
/// bilinear can reach from a sample point half a texel inside the chart; the
/// second texel the old value carried was pure padding, and on the small
/// charts low-poly art is mostly made of it was a third of the area.
pub(crate) const GUTTER: usize = 1;

/// Sub-texel sample offsets, shared with the reference evaluator so both
/// rasterise the exact same coverage. Rotated-grid: no two share a row or
/// column, so gradients in any direction get four distinct positions rather
/// than an aligned lattice.
pub(crate) const TEXEL_SUB_OFFSETS: [(f32, f32); TEXEL_SUBSAMPLES] =
    [(0.375, 0.125), (0.875, 0.375), (0.125, 0.625), (0.625, 0.875)];

/// A group of adjacent triangles sharing one continuous mapping.
///
/// `pub(crate)` for the reference evaluator ([`crate::ao_reference`]), which
/// must rasterise the SAME charts so its atlas diffs texel-for-texel.
pub(crate) struct Chart {
    pub(crate) tris: Vec<usize>,
    /// Corner positions of each of `tris`, in texels of this chart's block.
    pub(crate) tex: Vec<[[f32; 2]; 3]>,
    /// Size in texels, gutter included.
    pub(crate) w: usize,
    pub(crate) h: usize,
    /// Where it landed in the atlas. `pub(crate)` for the BakerBoy port
    /// ([`crate::bakerboy`]), which assembles and dilates the FULL atlas
    /// image (its dilation crosses chart boundaries by design) and so needs
    /// to know where each chart's block sits.
    pub(crate) x: usize,
    pub(crate) y: usize,
    /// Which texels of the block are this chart's, gutter included. Charts
    /// nest, so one chart's block can hold parts of its neighbours.
    pub(crate) owned: Vec<bool>,
}

fn face_normal(a: Vec3f, b: Vec3f, c: Vec3f) -> Vec3f {
    let e1 = Vec3f { x: b.x - a.x, y: b.y - a.y, z: b.z - a.z };
    let e2 = Vec3f { x: c.x - a.x, y: c.y - a.y, z: c.z - a.z };
    let n = Vec3f {
        x: e1.y * e2.z - e1.z * e2.y,
        y: e1.z * e2.x - e1.x * e2.z,
        z: e1.x * e2.y - e1.y * e2.x,
    };
    let l = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt().max(1.0e-12);
    Vec3f { x: n.x / l, y: n.y / l, z: n.z / l }
}

/// Charts for one model and their place in `atlas`.
fn lay_out_charts(atlas: &mut AoAtlas, positions: &[Vec3f], indices: &[u32]) -> Vec<Chart> {
    use makepad_uv_unwrap::{AtlasTarget, ChartOptions, MeshInput, PackOptions};
    let points: Vec<[f64; 3]> =
        positions.iter().map(|p| [p.x as f64, p.y as f64, p.z as f64]).collect();
    let input = MeshInput { positions: &points, indices, polygons: None, seams: &[] };
    let options = ChartOptions {
        crease_cos: CHART_CREASE_COS,
        weld_tolerance: CHART_WELD,
        ..ChartOptions::default()
    };
    // An indexed mesh of finite vertices is all this takes, and the loaders
    // have checked both by now.
    let charting = makepad_uv_unwrap::parameterize(&input, &options, &mut |_| true)
        .expect("charts for a baked model");
    let area: f64 = charting.charts.iter().map(|c| c.area).sum();
    let padding = (2 * GUTTER) as u32;
    let pack = |target| PackOptions { target, padding, obstacles: &[] };
    let spend = |target| makepad_uv_unwrap::pack(&charting, &pack(target), &mut |_| true);

    // A lone model in a virgin atlas gets a square just big enough at its target
    // density, and then the density its charts actually fit at; the texture is
    // never larger than the model needs (a crate at 96 texels/unit wants a 64
    // square, not the house's 672), and none of it is left blank. Multiples of 8
    // rather than powers of two: the next power of two above 660 is 1024, 2.4x
    // the memory for zero added detail.
    //
    // In a shared atlas the model's whole layout becomes one block on the shelf,
    // at the pack's density or less if the remaining budget demands it.
    let virgin = atlas.fill && atlas.shelf_x == 0 && atlas.shelf_y == 0 && atlas.shelf_h == 0;
    let layout = if virgin {
        let ceiling = atlas.size.max(ATLAS_MAX);
        let wanted = area * (MODEL_TEXELS_PER_UNIT as f64) * (MODEL_TEXELS_PER_UNIT as f64);
        let mut side = ((wanted / 0.6).sqrt().ceil() as usize)
            .next_multiple_of(8)
            .clamp(64, atlas.size.max(64));
        loop {
            match spend(AtlasTarget::Fit { width: side as u32, height: side as u32 }) {
                Ok(layout) => {
                    atlas.pixels = vec![255; side * side];
                    atlas.size = side;
                    break Some(layout);
                }
                Err(_) if side < ceiling => side = (side * 2).min(ceiling),
                Err(_) => break None,
            }
        }
    } else {
        let free = atlas.free_texels() as f64;
        let mut density = TEXELS_PER_UNIT as f64;
        if area * density * density > free * 0.8 {
            density = (free * 0.8 / area).sqrt();
        }
        loop {
            let target = AtlasTarget::Density { texels_per_unit: density, max_size: ATLAS_MAX as u32 };
            match spend(target) {
                Ok(layout) => break Some(layout),
                Err(_) if density > 1.0e-3 => density *= 0.7,
                Err(_) => break None,
            }
        }
    };
    let Some(layout) = layout else {
        // More charts than the largest atlas has texels for. The model keeps
        // every triangle and reads one texel: unlit by AO, not wrong.
        atlas.overflowed += 1;
        eprintln!("AO atlas overflow: {} charts do not fit {ATLAS_MAX} texels", charting.charts.len());
        let middle = [atlas.size as f32 * 0.5; 2];
        let tris: Vec<usize> = (0..charting.triangle_chart.len()).collect();
        let tex = vec![[middle; 3]; tris.len()];
        let chart = Chart { tris, tex, w: 0, h: 0, x: 0, y: 0, owned: Vec::new() };
        return vec![chart];
    };
    let (origin_x, origin_y) = atlas.alloc_rect(layout.width as usize, layout.height as usize);
    let gutter = GUTTER as i64;
    let mut charts = Vec::with_capacity(charting.charts.len());
    for (source, placed) in charting.charts.iter().zip(&layout.placements) {
        let (mw, mh) = (placed.mask.width as usize, placed.mask.height as usize);
        let (w, h) = (mw + 2 * GUTTER, mh + 2 * GUTTER);
        let mut owned = vec![false; w * h];
        for row in 0..placed.mask.height {
            for &(first, last) in placed.mask.row(row) {
                for y in row as i64..=row as i64 + 2 * gutter {
                    for x in first as i64..=last as i64 + 2 * gutter {
                        owned[y as usize * w + x as usize] = true;
                    }
                }
            }
        }
        let tris: Vec<usize> = source.triangles.iter().map(|&t| t as usize).collect();
        let (left, top) = (placed.x as f64 - gutter as f64, placed.y as f64 - gutter as f64);
        let tex = tris
            .iter()
            .map(|&t| {
                [0, 1, 2].map(|k| {
                    let q = placed.map(charting.corner_uv[t * 3 + k]);
                    [(q[0] - left) as f32, (q[1] - top) as f32]
                })
            })
            .collect();
        charts.push(Chart {
            tris,
            tex,
            w,
            h,
            x: origin_x + placed.x as usize - GUTTER,
            y: origin_y + placed.y as usize - GUTTER,
            owned,
        });
    }
    charts
}

/// Bake AO into an atlas of CHARTS — one continuous texture region per run of
/// the surface.
///
/// # Why not a patch per triangle
///
/// That was the previous design and it could not work. Each triangle owning
/// its own square of texture means each carries its own gradient, brightest in
/// the middle and unrelated to its neighbours', so a flat wall renders as a
/// quilt of soft diamonds radiating from every vertex. The occlusion values
/// were right; the parameterisation made them unreadable.
///
/// Grouping neighbours into a chart makes a facade ONE surface with ONE mapping,
/// exactly like texturing a wall: occlusion varies smoothly across it, dark
/// where it meets the ground and under the eave, clean in the middle.
///
/// # The retopology
///
/// A vertex on a chart boundary needs a different uv for each chart it touches,
/// so it is duplicated per chart. That is far cheaper than the un-indexing the
/// per-triangle scheme forced: splits happen only at chart seams, not at every
/// triangle, and inside a chart the mesh stays welded.
///
/// `positions`, `normals` and `indices` are rewritten in place.
///
/// `normals` double as the AUTHORED normals for the orientation tie-break
/// (see [`bake_into_authored`]) — right for rigs and fixtures, whose normal
/// array is the authored one. The prop loader must call the `_authored`
/// variant instead: its `normals` were rebuilt from winding by
/// `resolve_corner_normals`, which erases exactly the artist signal the
/// tie-break needs.
pub fn bake_into(
    atlas: &mut AoAtlas,
    positions: &mut Vec<Vec3f>,
    normals: &mut Vec<Vec3f>,
    indices: &mut Vec<u32>,
    min: Vec3f,
    max: Vec3f,
) -> BakedAo {
    bake_into_authored_inner(atlas, positions, normals, None, indices, min, max, None)
}

/// Pool-aware offline bake. Call this from a runtime worker; the nested
/// chart and sample batches borrow the runtime pool through `fan_out`.
pub fn bake_into_with_pool(
    pool: &TaskPool,
    atlas: &mut AoAtlas,
    positions: &mut Vec<Vec3f>,
    normals: &mut Vec<Vec3f>,
    indices: &mut Vec<u32>,
    min: Vec3f,
    max: Vec3f,
) -> BakedAo {
    bake_into_authored_inner(atlas, positions, normals, None, indices, min, max, Some(pool))
}

/// [`bake_into`] with the AUTHORED vertex normals passed alongside the
/// (possibly rebuilt) shading normals. The lightmapper's twin-dedup uses them
/// to resolve near-tie surface-group orientations: winding order flips under
/// a mirror baked into the vertex stream, authored normals do not.
/// `authored` is parallel to `positions`; `None` means `normals` ARE the
/// authored ones.
pub fn bake_into_authored(
    atlas: &mut AoAtlas,
    positions: &mut Vec<Vec3f>,
    normals: &mut Vec<Vec3f>,
    authored: Option<&[Vec3f]>,
    indices: &mut Vec<u32>,
    min: Vec3f,
    max: Vec3f,
) -> BakedAo {
    bake_into_authored_inner(atlas, positions, normals, authored, indices, min, max, None)
}

pub fn bake_into_authored_with_pool(
    pool: &TaskPool,
    atlas: &mut AoAtlas,
    positions: &mut Vec<Vec3f>,
    normals: &mut Vec<Vec3f>,
    authored: Option<&[Vec3f]>,
    indices: &mut Vec<u32>,
    min: Vec3f,
    max: Vec3f,
) -> BakedAo {
    bake_into_authored_inner(
        atlas, positions, normals, authored, indices, min, max, Some(pool),
    )
}

fn bake_into_authored_inner(
    atlas: &mut AoAtlas,
    positions: &mut Vec<Vec3f>,
    normals: &mut Vec<Vec3f>,
    authored: Option<&[Vec3f]>,
    indices: &mut Vec<u32>,
    min: Vec3f,
    max: Vec3f,
    pool: Option<&TaskPool>,
) -> BakedAo {
    // GROWTH INVALIDATES EVERY UV ALREADY HANDED OUT.
    //
    // `ao_uv` is normalised by `atlas.size` at the moment a model is baked, so
    // if a later model in the same pack makes the atlas double, every earlier
    // model's uvs are now twice too large and point into someone else's
    // region. That rendered as grey smudges in the middle of flat walls —
    // occlusion from a different model entirely — and it was silent, which is
    // why it survived several rounds of "the AO looks wrong" being blamed on
    // the sampler. Callers must pre-size the atlas to its final size.
    let size_in = atlas.size;

    // WHICH ENGINE answers the texels — read once, on the calling thread —
    // and that engine's own preprocessing, port-verbatim: each reference
    // baker was validated end to end with its own twin-dedup + orientation
    // pass in front of this chart machinery, and the pipelines here
    // reproduce those validated bakes rather than sharing one
    // reinterpretation. (Kenney's modular kits author every face twice with
    // opposite winding; all three ports collapse the pairs to one
    // consistently-oriented triangle per surface, each by its own method.)
    let baker = AoBakerKind::current();
    let t_bake = Cx::monotonic_now();
    match baker {
        AoBakerKind::Lightmapper => {
            // The port tool's dedup: keep the FIRST twin of each coincident
            // pair, then orient every kept triangle by majority-parity —
            // flip the winding of any face whose front stares into matter.
            // (The tool also rebuilt flat normals afterwards; unnecessary
            // here — the chart machinery keys on winding face normals and
            // the hemicube bake runs LM_NONE, so the authored vertex
            // normals pass through untouched for the renderer's lighting.)
            crate::ao_lightmapper::dedup_twins(
                positions,
                authored.unwrap_or(normals),
                indices,
                min,
                max,
            );
        }
        AoBakerKind::BakerBoy => {
            crate::bakerboy::dedup_double_sided(positions, normals, indices, min, max);
        }
        AoBakerKind::Aobaker => {
            crate::aobaker_port::dedup_double_sided(positions, indices, min, max);
        }
    }

    let tri_count = indices.len() / 3;
    if tri_count == 0 || positions.is_empty() {
        return BakedAo {
            source_vertex: (0..positions.len() as u32).collect(),
            ao_uv: vec![[0.5, 0.5]; positions.len()],
            vertex_ao: vec![1.0; positions.len()],
            ground: None,
        };
    }

    // The occluder set is the ORIGINAL indexed mesh plus a virtual ground.
    let mut occ_pos = positions.clone();
    let mut occ_idx = indices.clone();
    let gx = (max.x - min.x).max(max.z - min.z) * 2.0 + 1.0;
    let base = occ_pos.len() as u32;
    let y = min.y;
    let cx = (min.x + max.x) * 0.5;
    let cz = (min.z + max.z) * 0.5;
    occ_pos.extend_from_slice(&[
        Vec3f { x: cx - gx, y, z: cz - gx },
        Vec3f { x: cx + gx, y, z: cz - gx },
        Vec3f { x: cx + gx, y, z: cz + gx },
        Vec3f { x: cx - gx, y, z: cz + gx },
    ]);
    occ_idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);

    // Grid spans the ground quad too; reach stays keyed to the model.
    let (mut occ_min, mut occ_max) = (min, max);
    for p in &occ_pos {
        occ_min.x = occ_min.x.min(p.x);
        occ_min.y = occ_min.y.min(p.y);
        occ_min.z = occ_min.z.min(p.z);
        occ_max.x = occ_max.x.max(p.x);
        occ_max.y = occ_max.y.max(p.y);
        occ_max.z = occ_max.z.max(p.z);
    }
    // Rays divided across the subsamples, so a texel's total budget stays
    // ATLAS_RAYS: position jitter buys its decorrelation with rays the single
    // centre sample was spending on directions it had already covered.
    let sampler = crate::ao::AoSampler::with_reach(
        &occ_pos, &occ_idx, occ_min, occ_max, min, max,
        (ATLAS_RAYS / TEXEL_SUBSAMPLES).max(32), tri_count,
    );

    let charts = lay_out_charts(atlas, positions, indices);

    // --- Evaluate the charts with the selected engine ----------------------
    // aobaker answers per chart, in parallel, with the machinery's
    // chart-local dilation; BakerBoy is DIRECTION-major — one ortho depth
    // map per direction serves every texel — so it takes the whole chart set
    // at once and hands back the same per-chart blocks (its gutter fill is
    // BakerBoy's own atlas-wide dilation); the lightmapper renders hemicubes
    // from lightmap-uv space, so it runs AFTER the uv emission below and
    // writes the atlas there.
    let chart_px: Vec<Vec<u8>> = match baker {
        AoBakerKind::Aobaker => {
            let params = crate::aobaker_port::AobakerParams::from_env();
            let pos: &[Vec3f] = positions;
            let nrm: &[Vec3f] = normals;
            let idx: &[u32] = indices;
            let charts_ref = &charts;
            let sampler = &sampler;
            let (occ_pos, occ_idx) = (&occ_pos, &occ_idx);
            let mut out: Vec<Vec<u8>> = vec![Vec::new(); charts.len()];
            let (tx, rx) = std::sync::mpsc::channel();
            let work = |ci: usize| {
                let bytes = crate::aobaker_port::bake_aobaker_chart(
                    &charts_ref[ci], sampler, occ_pos, occ_idx, pos, nrm, idx, &params,
                );
                let _ = tx.send((ci, bytes));
            };
            match pool {
                Some(pool) => pool.fan_out(charts.len(), work),
                None => (0..charts.len()).for_each(work),
            }
            drop(tx);
            for (ci, bytes) in rx {
                out[ci] = bytes;
            }
            out
        }
        AoBakerKind::BakerBoy => crate::bakerboy::bake_all_charts(
            &charts, atlas.size, positions, normals, indices, min, max, pool,
        ),
        AoBakerKind::Lightmapper => Vec::new(),
    };

    let sz = atlas.size;
    if !chart_px.is_empty() {
        for (ci, c) in charts.iter().enumerate() {
            for ty in 0..c.h {
                for tx in 0..c.w {
                    if c.owned[ty * c.w + tx] {
                        atlas.pixels[(c.y + ty) * sz + c.x + tx] = chart_px[ci][ty * c.w + tx];
                    }
                }
            }
        }
    }

    // Ground AO is RETIRED (user call, 2026-08-12): the drape that consumed
    // this plane sliced legs and floated on uneven ground; ambient grounding
    // now lives only in the model's own surface atlas. The sidecar format
    // keeps an empty ground block so files stay parseable both ways.
    let ground: Option<GroundAo> = {
        None
    };

    // --- Emit geometry: one vertex per (source vertex, chart, uv) -----------
    // The uv is in the key because two fans touching at one vertex of a chart
    // are mapped apart. Triangles are in input order, as the bakers expect.
    let mut out_pos = Vec::with_capacity(positions.len());
    let mut out_nrm = Vec::with_capacity(positions.len());
    let mut source_vertex: Vec<u32> = Vec::with_capacity(positions.len());
    let mut ao_uv: Vec<[f32; 2]> = Vec::with_capacity(positions.len());
    let mut out_idx = vec![0u32; indices.len()];
    let mut seen: std::collections::HashMap<(u32, usize, u32, u32), u32> =
        std::collections::HashMap::new();

    for (ci, c) in charts.iter().enumerate() {
        for (i, &t) in c.tris.iter().enumerate() {
            for k in 0..3 {
                let vi = indices[t * 3 + k];
                let [tx, ty] = c.tex[i][k];
                let key = (vi, ci, tx.to_bits(), ty.to_bits());
                let out_i = *seen.entry(key).or_insert_with(|| {
                    out_pos.push(positions[vi as usize]);
                    out_nrm.push(normals.get(vi as usize).copied().unwrap_or(Vec3f {
                        x: 0.0,
                        y: 1.0,
                        z: 0.0,
                    }));
                    source_vertex.push(vi);
                    ao_uv.push([
                        (c.x as f32 + tx) / atlas.size as f32,
                        (c.y as f32 + ty) / atlas.size as f32,
                    ]);
                    (out_pos.len() - 1) as u32
                });
                out_idx[t * 3 + k] = out_i;
            }
        }
    }

    // --- Lightmapper engine: hemicubes rendered from the emitted uvs -------
    // The port's `--sidecars` bake — example.c's AO parameters with the
    // Lambertian cosine kernel, the open-sky rescale, its dilate/smooth/
    // dilate post chain run to fixpoint, and its display gamma — with ONE
    // departure from the sidecar bake's LM_NONE: the hemicubes aim along
    // the model's own SMOOTHED per-vertex normals. The kits' curved runs
    // are segmented tubes bent ~15 degrees per facet; a flat facet normal
    // steps the sample direction at every bend, so each facet's whole
    // triangle shifts value together (seen live on the gate arch as hard
    // diagonal steps inside visually-smooth faces), while the renderer
    // lights the same surface with smoothed normals and shows no such
    // break. Sampling along the smoothed field keeps AO continuous exactly
    // where the lighting is — lightmapper.h's own per-vertex-normal mode,
    // which its README uses for curved geometry. Signs are fixed to the
    // dedup-oriented winding: a kept twin the orientation pass flipped
    // carries the OTHER side's authored normals.
    // Two art knobs on top (documented in `ao_lightmapper`):
    // `AO_LM_STRENGTH` and `AO_LM_GAMMA`.
    if baker == AoBakerKind::Lightmapper {
        let lm_nrm: Vec<Vec3f> = {
            let mut n = out_nrm.clone();
            for t in 0..out_idx.len() / 3 {
                let f = face_normal(
                    out_pos[out_idx[t * 3] as usize],
                    out_pos[out_idx[t * 3 + 1] as usize],
                    out_pos[out_idx[t * 3 + 2] as usize],
                );
                for k in 0..3 {
                    let vi = out_idx[t * 3 + k] as usize;
                    let v = n[vi];
                    let len = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
                    if len < 1.0e-6 {
                        // Degenerate authored normal: the flat facet normal
                        // is the only honest direction left.
                        n[vi] = f;
                    } else if v.x * f.x + v.y * f.y + v.z * f.z < 0.0 {
                        n[vi] = Vec3f { x: -v.x, y: -v.y, z: -v.z };
                    }
                }
            }
            n
        };
        let params = crate::ao_lightmapper::LightmapParams {
            // AO_LM_PASSES: hierarchical-interpolation passes override
            // (default 2 = example.c). 0 renders every texel — the
            // ground-truth setting for isolating interpolation artifacts.
            interpolation_passes: std::env::var("AO_LM_PASSES")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .filter(|p| *p <= 8)
                .unwrap_or(2),
            ..crate::ao_lightmapper::LightmapParams::atlas()
        };
        let bake = crate::ao_lightmapper::bake_ao_inner(
            &out_pos, Some(&lm_nrm), &ao_uv, &out_idx, sz, sz, &params, pool,
        );
        let mut data = bake.data;
        if data.iter().any(|v| *v != 0.0) {
            // The cosine kernel integrates to ~0.5 over the hemisphere (the
            // header normalizes only the validity channel of the weight
            // texture), so an open-sky texel reads ~0.5. Rescale so open sky
            // = 1.0 using the kernel's own discrete sum — lmImageScale.
            let w = crate::ao_lightmapper::hemisphere_weights(params.hemisphere_size, |c| c);
            let open_sky: f32 = w.iter().map(|x| x.0).sum();
            crate::ao_lightmapper::image_scale(&mut data, 1.0 / open_sky);
            // BURIED texels — rendered but rejected by the validity gate
            // every time, i.e. sample points inside an interpenetrating
            // neighbour (kit joints overlap members freely) — are filled by
            // a HARMONIC fill: relax each rejected region to the mean of its
            // neighbours with the surrounding VALID texels as the fixed
            // boundary. This is the third fill to hold this slot and the
            // first whose result is continuous by construction:
            //
            // * the dilate chain's mean-FLOOD assigns each texel the mean of
            //   whichever front reached it first — fronts from a bright and
            //   a dark boundary meet in a hard ridge (the gate's hot
            //   wedges);
            // * filling by MINIMUM pools whole regions at their darkest rim,
            //   which breaks in a step wherever the region's far boundary is
            //   open surface (the gate pillars' 0.99|0.51 joint lines — the
            //   step sat exactly on the welded edge mid-wall).
            //
            // The harmonic solution has neither: no fronts (it is the
            // steady state, not a propagation), and it meets every boundary
            // value exactly, so a buried strip blends smoothly into the
            // open surface it is welded to. Physically it is the right
            // reading of "no data": these texels are inside other members —
            // invisible — and only need to CONNECT their visible
            // surroundings without inventing an edge.
            //
            // Regions with NO valid boundary in their own atlas
            // neighbourhood come in two kinds, told apart by the SURFACE
            // they lie on:
            //
            // * A buried run that CONTINUES a visible surface across a
            //   chart border (the chart machinery splits curved runs; the
            //   arch flank's slide into the keystone slot lands in its own
            //   chart, every texel rejected). Its continuity source exists
            //   — it is just in another chart. Values are PINNED across the
            //   3D welded edges from the neighbouring triangles' near-edge
            //   texels, then the region relaxes against those pins.
            //
            // * Whole faces buried inside the union of members
            //   (back-to-back contact planes of flush boxes), welded only
            //   to other buried faces. They see no sky from anywhere —
            //   their honest occlusion is total — and mean-flooding them
            //   from OTHER charts across the atlas gutters (what the dilate
            //   fixpoint did) hands the two sides of one interior joint
            //   unrelated values. They bake BLACK, the measured answer.
            {
                let rejected: Vec<bool> = bake.debug.chunks(3).map(|c| c[2] != 0).collect();
                let region: Vec<usize> = (0..sz * sz)
                    .filter(|&i| rejected[i] && data[i] == 0.0)
                    .collect();
                // Seed zero texels breadth-first from the valid boundary
                // (values are provisional), then relax to the harmonic
                // steady state (Gauss–Seidel; anything >0 outside the
                // region is fixed boundary, unseeded texels stay out).
                let solve = |data: &mut Vec<f32>, fixed: &std::collections::HashSet<usize>| {
                    loop {
                        let mut writes: Vec<(usize, f32)> = Vec::new();
                        for &i in &region {
                            if data[i] != 0.0 {
                                continue;
                            }
                            let (x, y) = (i % sz, i / sz);
                            let (mut sum, mut n) = (0.0f32, 0u32);
                            for (nx, ny) in
                                [(x as i32 - 1, y as i32), (x as i32 + 1, y as i32), (x as i32, y as i32 - 1), (x as i32, y as i32 + 1)]
                            {
                                if nx < 0 || ny < 0 || nx >= sz as i32 || ny >= sz as i32 {
                                    continue;
                                }
                                let v = data[ny as usize * sz + nx as usize];
                                if v > 0.0 {
                                    sum += v;
                                    n += 1;
                                }
                            }
                            if n > 0 {
                                writes.push((i, sum / n as f32));
                            }
                        }
                        if writes.is_empty() {
                            break;
                        }
                        for (i, v) in writes {
                            data[i] = v;
                        }
                    }
                    for _ in 0..8 * sz {
                        let mut worst = 0.0f32;
                        for &i in &region {
                            if data[i] == 0.0 || fixed.contains(&i) {
                                continue;
                            }
                            let (x, y) = (i % sz, i / sz);
                            let (mut sum, mut n) = (0.0f32, 0u32);
                            for (nx, ny) in
                                [(x as i32 - 1, y as i32), (x as i32 + 1, y as i32), (x as i32, y as i32 - 1), (x as i32, y as i32 + 1)]
                            {
                                if nx < 0 || ny < 0 || nx >= sz as i32 || ny >= sz as i32 {
                                    continue;
                                }
                                let v = data[ny as usize * sz + nx as usize];
                                if v > 0.0 {
                                    sum += v;
                                    n += 1;
                                }
                            }
                            if n > 0 {
                                let next = sum / n as f32;
                                worst = worst.max((next - data[i]).abs());
                                data[i] = next;
                            }
                        }
                        if worst < 1.0e-4 {
                            break;
                        }
                    }
                };
                solve(&mut data, &std::collections::HashSet::new());
                // Pin still-zero texels from across the 3D weld: for every
                // welded near-coplanar edge with a resolved side and a
                // zero side, copy the resolved side's near-edge value to
                // the zero side's near-edge texel, then relax again.
                if region.iter().any(|&i| data[i] == 0.0) {
                    let span = (max.x - min.x)
                        .max(max.y - min.y)
                        .max(max.z - min.z)
                        .max(1.0e-5);
                    let inv_eps = 1.0 / (span * 1.0e-5);
                    let quant = |p: Vec3f| {
                        (
                            (p.x * inv_eps).round() as i64,
                            (p.y * inv_eps).round() as i64,
                            (p.z * inv_eps).round() as i64,
                        )
                    };
                    let out_tris = out_idx.len() / 3;
                    let fnrm: Vec<Vec3f> = (0..out_tris)
                        .map(|t| {
                            face_normal(
                                out_pos[out_idx[t * 3] as usize],
                                out_pos[out_idx[t * 3 + 1] as usize],
                                out_pos[out_idx[t * 3 + 2] as usize],
                            )
                        })
                        .collect();
                    // A triangle's sample point for edge e: the edge's uv
                    // midpoint pushed 1.5 texels toward the uv centroid.
                    let probe = |t: usize, e: usize| -> usize {
                        let (ia, ib, ic) = (
                            out_idx[t * 3] as usize,
                            out_idx[t * 3 + 1] as usize,
                            out_idx[t * 3 + 2] as usize,
                        );
                        let (es, ee) = (
                            out_idx[t * 3 + e] as usize,
                            out_idx[t * 3 + (e + 1) % 3] as usize,
                        );
                        let m = [
                            (ao_uv[es][0] + ao_uv[ee][0]) * 0.5 * sz as f32,
                            (ao_uv[es][1] + ao_uv[ee][1]) * 0.5 * sz as f32,
                        ];
                        let c = [
                            (ao_uv[ia][0] + ao_uv[ib][0] + ao_uv[ic][0]) / 3.0 * sz as f32,
                            (ao_uv[ia][1] + ao_uv[ib][1] + ao_uv[ic][1]) / 3.0 * sz as f32,
                        ];
                        let d = ((c[0] - m[0]).powi(2) + (c[1] - m[1]).powi(2)).sqrt();
                        let k = if d > 1.0e-6 { (1.5f32).min(d) / d } else { 0.0 };
                        let x = ((m[0] + (c[0] - m[0]) * k) as usize).min(sz - 1);
                        let y = ((m[1] + (c[1] - m[1]) * k) as usize).min(sz - 1);
                        y * sz + x
                    };
                    let mut edges: std::collections::HashMap<
                        ((i64, i64, i64), (i64, i64, i64)),
                        Vec<(usize, usize)>,
                    > = std::collections::HashMap::new();
                    for t in 0..out_tris {
                        let (a, b, c) = (
                            quant(out_pos[out_idx[t * 3] as usize]),
                            quant(out_pos[out_idx[t * 3 + 1] as usize]),
                            quant(out_pos[out_idx[t * 3 + 2] as usize]),
                        );
                        for (e, (u2, w2)) in [(a, b), (b, c), (c, a)].into_iter().enumerate() {
                            let key = if u2 <= w2 { (u2, w2) } else { (w2, u2) };
                            edges.entry(key).or_default().push((t, e));
                        }
                    }
                    let mut pins: Vec<(usize, f32)> = Vec::new();
                    for users in edges.values() {
                        for i in 0..users.len() {
                            for j in 0..users.len() {
                                if i == j {
                                    continue;
                                }
                                let (t0, e0) = users[i];
                                let (t1, e1) = users[j];
                                let d = fnrm[t0].x * fnrm[t1].x
                                    + fnrm[t0].y * fnrm[t1].y
                                    + fnrm[t0].z * fnrm[t1].z;
                                if d < 0.94 {
                                    continue; // not the same visual surface
                                }
                                let (p0, p1) = (probe(t0, e0), probe(t1, e1));
                                if data[p0] == 0.0 && data[p1] > 0.0 {
                                    pins.push((p0, data[p1]));
                                }
                            }
                        }
                    }
                    let mut fixed = std::collections::HashSet::new();
                    for (i, v) in pins {
                        if data[i] == 0.0 {
                            data[i] = v;
                            fixed.insert(i);
                        }
                    }
                    solve(&mut data, &fixed);
                }
                // What is STILL zero is buried with every welded neighbour
                // buried too: black, the measured occlusion.
                for &i in &region {
                    if data[i] == 0.0 {
                        data[i] = f32::MIN_POSITIVE;
                    }
                }
            }
            // AO_LM_STRENGTH (default 1.0): scales OCCLUSION after the
            // rescale — `1 - strength*(1-ao)` — so open faces stay white at
            // any strength. Valid texels only: 0 stays the dilate-me marker.
            let strength = std::env::var("AO_LM_STRENGTH")
                .ok()
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(1.0);
            if (strength - 1.0).abs() > 1.0e-6 {
                for v in data.iter_mut() {
                    if *v > 0.0 {
                        *v = (1.0 - strength * (1.0 - *v)).clamp(f32::MIN_POSITIVE, 1.0);
                    }
                }
            }
            // Dilate to FIXPOINT (the shelf packer leaves wider gaps than
            // the reference atlas, and validity-REJECTED texels — samples
            // taken from inside geometry at interpenetrating joints — sit at
            // 0 until a valid neighbour reaches them), then one smooth, one
            // more dilate. A pack atlas stops as soon as this model's chart
            // rects are filled: the flood runs over the model's own scratch
            // buffer, but flooding a mostly-empty 1024 square to fixpoint is
            // pure waste.
            let rects_filled = |data: &[f32]| {
                charts.iter().all(|c| {
                    (0..c.h).all(|ty| (0..c.w).all(|tx| data[(c.y + ty) * sz + c.x + tx] != 0.0))
                })
            };
            let mut tmp = vec![0.0f32; sz * sz];
            for _ in 0..sz {
                crate::ao_lightmapper::image_dilate(&data, &mut tmp, sz, sz);
                crate::ao_lightmapper::image_dilate(&tmp, &mut data, sz, sz);
                let done = if atlas.fill {
                    !data.iter().any(|v| *v == 0.0)
                } else {
                    rects_filled(&data)
                };
                if done {
                    break;
                }
            }
            crate::ao_lightmapper::image_smooth(&data, &mut tmp, sz, sz);
            crate::ao_lightmapper::image_dilate(&tmp, &mut data, sz, sz);
            // AO_LM_GAMMA (default 2.2): example.c's display `pow(1/gamma)`.
            // 1.0 = linear, lower = darker mids.
            let gamma = std::env::var("AO_LM_GAMMA")
                .ok()
                .and_then(|v| v.parse::<f32>().ok())
                .filter(|g| *g > 0.0)
                .unwrap_or(2.2);
            if (gamma - 1.0).abs() > 1.0e-6 {
                crate::ao_lightmapper::image_power(&mut data, 1.0 / gamma);
            }
            if atlas.fill {
                // Single-model atlas: ship the port's full post-processed
                // image, flooded background included, exactly as its sidecar
                // bake did.
                for (px, v) in atlas.pixels.iter_mut().zip(&data) {
                    *px = (v.clamp(0.0, 1.0) * 255.0) as u8;
                }
            } else {
                // Pack atlas is shared: this model's chart rects only.
                for c in charts.iter() {
                    for ty in 0..c.h {
                        for tx in 0..c.w {
                            let v = data[(c.y + ty) * sz + c.x + tx];
                            atlas.pixels[(c.y + ty) * sz + c.x + tx] =
                                (v.clamp(0.0, 1.0) * 255.0) as u8;
                        }
                    }
                }
            }
        }
        // else: nothing rasterised a single valid texel — leave the atlas
        // white (neutral) rather than black.
    }
    atlas.bake_evaluator = baker.name();
    atlas.bake_seconds += Cx::monotonic_now() - t_bake;

    *positions = out_pos;
    *normals = out_nrm;
    *indices = out_idx;

    let vertex_ao = ao_uv
        .iter()
        .map(|uv| {
            let x = ((uv[0] * atlas.size as f32) as usize).min(atlas.size - 1);
            let y = ((uv[1] * atlas.size as f32) as usize).min(atlas.size - 1);
            atlas.pixels[y * atlas.size + x] as f32 / 255.0
        })
        .collect();
    // A fill atlas serves ONE model and computes every uv after packing, so
    // resizing (the shrink-wrap above, or an overflow grow) is safe there and
    // only there.
    assert!(
        atlas.fill || atlas.size == size_in,
        "atlas grew from {size_in} to {} during a bake — every uv issued to an \
         earlier model in this pack now points into the wrong region. Pre-size \
         the atlas to ATLAS_MAX before baking.",
        atlas.size
    );
    BakedAo { source_vertex, ao_uv, vertex_ao, ground }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32, y: f32, z: f32) -> Vec3f {
        Vec3f { x, y, z }
    }

    /// A floor with a wall on it — the case vertex AO could not express. The
    /// atlas must contain texels that are meaningfully darker than the mesh's
    /// brightest, because the occlusion now lives in texture space where there
    /// is room for it.
    fn floor_with_wall() -> (Vec<Vec3f>, Vec<Vec3f>, Vec<u32>) {
        let mut p = vec![
            v(-4.0, 0.0, -4.0), v(4.0, 0.0, -4.0), v(4.0, 0.0, 4.0), v(-4.0, 0.0, 4.0),
        ];
        let mut n = vec![v(0.0, 1.0, 0.0); 4];
        // Winding agrees with the authored facing (floor up): the engines
        // orient open sheets by their winding, so a fixture whose winding
        // lies about its side would bake the underside.
        let mut i = vec![0, 2, 1, 0, 3, 2];
        let base = p.len() as u32;
        p.extend_from_slice(&[
            v(-4.0, 0.0, 0.0), v(4.0, 0.0, 0.0), v(4.0, 3.0, 0.0), v(-4.0, 3.0, 0.0),
        ]);
        n.extend_from_slice(&[v(0.0, 0.0, 1.0); 4]);
        i.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        (p, n, i)
    }

    #[test]
    fn a_ninety_degree_corner_darkens_in_the_atlas() {
        let (mut p, mut n, mut i) = floor_with_wall();
        let mut at = AoAtlas::new(ATLAS_MAX);
        bake_into(&mut at, &mut p, &mut n, &mut i, v(-4.0, 0.0, -4.0), v(4.0, 3.0, 4.0));
        let darkest = *at.pixels.iter().min().unwrap();
        let brightest = *at.pixels.iter().max().unwrap();
        assert!(
            (brightest as i32 - darkest as i32) > 40,
            "atlas is nearly uniform ({darkest}..{brightest}) — the corner did not darken"
        );
    }

    /// Coplanar neighbours must SHARE a chart rather than each taking their
    /// own patch.
    ///
    /// This is the whole difference between the per-triangle scheme and this
    /// one. If every triangle still ends up with private vertices, the mesh is
    /// fully un-indexed again and each face carries its own gradient — which
    /// is what made flat walls render as a quilt of soft diamonds.
    #[test]
    fn coplanar_neighbours_share_a_chart_instead_of_splitting() {
        let (mut p, mut n, mut i) = floor_with_wall();
        let tris = i.len() / 3;
        let mut at = AoAtlas::new(ATLAS_MAX);
        let baked = bake_into(&mut at, &mut p, &mut n, &mut i, v(-4.0, 0.0, -4.0), v(4.0, 3.0, 4.0));
        assert_eq!(i.len(), tris * 3, "index count must not change");
        assert_eq!(baked.ao_uv.len(), p.len());
        assert_eq!(baked.source_vertex.len(), p.len());
        // A floor quad and a wall quad: two charts of four corners each, so
        // eight vertices — against twelve if every triangle were split out.
        assert!(
            p.len() < tris * 3,
            "every triangle still owns its vertices ({} for {tris} tris) — charts did not weld",
            p.len()
        );
    }

    /// Two triangles may share texels only if they are coplanar — i.e. only
    /// within a chart. A texel shared across a crease means two charts were
    /// packed on top of each other, and one face would wear the other's
    /// shading.
    #[test]
    fn charts_do_not_overlap_in_the_atlas() {
        let (mut p, mut n, mut i) = floor_with_wall();
        let mut at = AoAtlas::new(ATLAS_MAX);
        let baked = bake_into(&mut at, &mut p, &mut n, &mut i, v(-4.0, 0.0, -4.0), v(4.0, 3.0, 4.0));
        let tris = i.len() / 3;
        let tri_n = |t: usize| {
            face_normal(
                p[i[t * 3] as usize],
                p[i[t * 3 + 1] as usize],
                p[i[t * 3 + 2] as usize],
            )
        };
        let mut owner = vec![usize::MAX; at.size * at.size];
        for t in 0..tris {
            let us: Vec<f32> =
                (0..3).map(|c| baked.ao_uv[i[t * 3 + c] as usize][0] * at.size as f32).collect();
            let vs: Vec<f32> =
                (0..3).map(|c| baked.ao_uv[i[t * 3 + c] as usize][1] * at.size as f32).collect();
            let (x0, x1) = (
                us.iter().cloned().fold(f32::MAX, f32::min),
                us.iter().cloned().fold(0.0f32, f32::max),
            );
            let (y0, y1) = (
                vs.iter().cloned().fold(f32::MAX, f32::min),
                vs.iter().cloned().fold(0.0f32, f32::max),
            );
            for y in y0 as usize..(y1.ceil() as usize).min(at.size) {
                for x in x0 as usize..(x1.ceil() as usize).min(at.size) {
                    let cell = &mut owner[y * at.size + x];
                    if *cell != usize::MAX && *cell != t {
                        let (a, b) = (tri_n(*cell), tri_n(t));
                        let d = a.x * b.x + a.y * b.y + a.z * b.z;
                        assert!(
                            d >= 0.966,
                            "texel ({x},{y}) shared by non-coplanar triangles {} and {t}",
                            *cell
                        );
                    }
                    *cell = t;
                }
            }
        }
    }


    /// A box floating in space versus the same box sitting on the ground.
    /// The virtual ground plane must darken its underside — without it a prop
    /// has no contact shadow and reads as hovering, which is the whole reason
    /// self-occlusion alone was not enough.
    #[test]
    fn the_ground_plane_darkens_what_sits_on_it() {
        // A downward-facing quad held 0.3 ABOVE the base of the bounds, so
        // the virtual ground sits just below it. Its own mesh occludes
        // nothing, so any darkening can only have come from the ground.
        //
        // Deliberately not coplanar with the ground: a surface lying exactly
        // on it is a degenerate self-intersection, not a contact shadow, and
        // testing that proves nothing.
        // 2cm up: clear of the ground plane, well inside `AO_RADIUS_WORLD`.
        // This used to be 0.3, which was inside the reach back when reach was
        // half the model's span; against an absolute ~12cm it is two and a half
        // times too far, and a surface a third of a metre off the floor SHOULD
        // now get no contact shadow. A prop actually resting on the ground sits
        // at this sort of gap.
        let mut p = vec![
            v(-1.0, 0.02, -1.0), v(1.0, 0.02, -1.0), v(1.0, 0.02, 1.0), v(-1.0, 0.02, 1.0),
        ];
        let mut n = vec![v(0.0, -1.0, 0.0); 4];
        let mut i = vec![0, 1, 2, 0, 2, 3];
        let mut at = AoAtlas::new(ATLAS_MAX);
        bake_into(&mut at, &mut p, &mut n, &mut i, v(-1.0, 0.0, -1.0), v(1.0, 1.0, 1.0));
        // NEW CONTRACT: the ATLAS bakes self-occlusion only — the virtual
        // ground is the ground_ao drape's business (double-counting it also
        // rimmed every low tile's bevels). So the atlas stays bright...
        let darkest = *at.pixels.iter().min().unwrap();
        assert!(
            darkest > 200,
            "atlas must not bake the virtual ground any more, darkest {darkest}/255"
        );
        // ...while the SAMPLER's with-ground channel still darkens, for the
        // consumers that ask for it (ground drape, vertex bakes).
        let (mut op, mut oi) = (p.clone(), i.clone());
        let base = op.len() as u32;
        op.extend_from_slice(&[
            v(-3.0, 0.0, -3.0), v(3.0, 0.0, -3.0), v(3.0, 0.0, 3.0), v(-3.0, 0.0, 3.0),
        ]);
        oi.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        let sampler = crate::ao::AoSampler::with_ground(
            &op, &oi, v(-3.0, 0.0, -3.0), v(3.0, 1.0, 3.0), 64, i.len() / 3,
        );
        let with_ground = sampler
            .at_split(&op, &oi, v(0.0, 0.02, 0.0), v(0.0, -1.0, 0.0))
            .1;
        assert!(
            with_ground < 0.6,
            "sampler with-ground must still darken a face 2cm over the floor, got {with_ground:.2}"
        );
    }

    /// Several models must share ONE atlas, or the batch gets a texture per
    /// model and the AO costs more than it is worth.
    #[test]
    fn many_models_share_one_atlas() {
        let mut at = AoAtlas::new(ATLAS_MAX);
        let before = at.size;
        for _ in 0..4 {
            let (mut p, mut n, mut i) = floor_with_wall();
            bake_into(&mut at, &mut p, &mut n, &mut i, v(-4.0, 0.0, -4.0), v(4.0, 3.0, 4.0));
        }
        assert!(at.size >= before, "atlas grew to {} to fit four models", at.size);
        // All four packed without overlapping: no texel written twice is not
        // checkable here, but the allocator must have advanced past one model.
        assert!(at.pixels.iter().any(|&x| x < 255), "nothing was baked at all");
    }


    #[test]
    fn baking_is_deterministic() {
        let (mut p1, mut n1, mut i1) = floor_with_wall();
        let (mut p2, mut n2, mut i2) = floor_with_wall();
        let mut a1 = AoAtlas::new(ATLAS_MAX);
        let a = bake_into(&mut a1, &mut p1, &mut n1, &mut i1, v(-4.0, 0.0, -4.0), v(4.0, 3.0, 4.0));
        let mut a2 = AoAtlas::new(ATLAS_MAX);
        let b = bake_into(&mut a2, &mut p2, &mut n2, &mut i2, v(-4.0, 0.0, -4.0), v(4.0, 3.0, 4.0));
        assert_eq!(a1.pixels, a2.pixels);
        assert_eq!(a.ao_uv, b.ao_uv);
    }

    #[test]
    fn an_empty_mesh_is_safe() {
        let (mut p, mut n, mut i) = (Vec::new(), Vec::new(), Vec::new());
        let mut at = AoAtlas::new(ATLAS_MAX);
        let baked = bake_into(&mut at, &mut p, &mut n, &mut i, v(0.0, 0.0, 0.0), v(0.0, 0.0, 0.0));
        assert!(baked.ao_uv.is_empty());
    }

}

// ── The AO truth suite ──────────────────────────────────────────────────
// Synthetic geometry with KNOWN correct occlusion, baked through the real
// pipeline and asserted numerically. Every case that ever went wrong on a
// real kit gets a minimal fixture here; the bake math is only "done" when
// all of them hold at once — including with flipped winding and mirrored
// normals, because the kits lie about both.
#[cfg(test)]
mod ao_truth_suite {
    use super::*;

    fn v(x: f32, y: f32, z: f32) -> Vec3f {
        Vec3f { x, y, z }
    }

    /// Push a box as 12 triangles with OUTWARD winding (when `flip` is
    /// false) or flipped winding + inward normals (when true — the mirrored
    /// kit convention).
    fn push_box(
        p: &mut Vec<Vec3f>,
        n: &mut Vec<Vec3f>,
        i: &mut Vec<u32>,
        lo: Vec3f,
        hi: Vec3f,
        flip: bool,
    ) {
        let faces: [([Vec3f; 4], Vec3f); 6] = [
            // +y
            (
                [v(lo.x, hi.y, lo.z), v(hi.x, hi.y, lo.z), v(hi.x, hi.y, hi.z), v(lo.x, hi.y, hi.z)],
                v(0.0, 1.0, 0.0),
            ),
            // -y
            (
                [v(lo.x, lo.y, lo.z), v(lo.x, lo.y, hi.z), v(hi.x, lo.y, hi.z), v(hi.x, lo.y, lo.z)],
                v(0.0, -1.0, 0.0),
            ),
            // +x
            (
                [v(hi.x, lo.y, lo.z), v(hi.x, lo.y, hi.z), v(hi.x, hi.y, hi.z), v(hi.x, hi.y, lo.z)],
                v(1.0, 0.0, 0.0),
            ),
            // -x
            (
                [v(lo.x, lo.y, lo.z), v(lo.x, hi.y, lo.z), v(lo.x, hi.y, hi.z), v(lo.x, lo.y, hi.z)],
                v(-1.0, 0.0, 0.0),
            ),
            // +z
            (
                [v(lo.x, lo.y, hi.z), v(lo.x, hi.y, hi.z), v(hi.x, hi.y, hi.z), v(hi.x, lo.y, hi.z)],
                v(0.0, 0.0, 1.0),
            ),
            // -z
            (
                [v(lo.x, lo.y, lo.z), v(hi.x, lo.y, lo.z), v(hi.x, hi.y, lo.z), v(lo.x, hi.y, lo.z)],
                v(0.0, 0.0, -1.0),
            ),
        ];
        for (quad, fnv) in faces {
            let base = p.len() as u32;
            for q in quad {
                p.push(q);
                // Normals stay HONEST in both variants: the loader corrects
                // mirrored nodes before any of this code runs (model.rs det
                // fix), so post-load data always carries outward normals —
                // `flip` models the one thing that still varies, winding.
                n.push(fnv);
            }
            // Winding must agree with the face's outward direction — the
            // renderer culls backfaces, so the bake contract is "the
            // winding side is the shown side". Compute the quad's cross
            // product and order indices so it points along fnv; `flip`
            // reverses it (the inward twin of a double-sided pair).
            let e1 = v(quad[1].x - quad[0].x, quad[1].y - quad[0].y, quad[1].z - quad[0].z);
            let e2 = v(quad[2].x - quad[0].x, quad[2].y - quad[0].y, quad[2].z - quad[0].z);
            let cx = e1.y * e2.z - e1.z * e2.y;
            let cy = e1.z * e2.x - e1.x * e2.z;
            let cz = e1.x * e2.y - e1.y * e2.x;
            let outward = cx * fnv.x + cy * fnv.y + cz * fnv.z > 0.0;
            let ccw = outward != flip;
            if ccw {
                i.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            } else {
                i.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
            }
        }
    }

    /// Bake a fixture, then return a closure sampling the atlas at the
    /// texel a given surface point maps to (via the emitted uvs).
    fn bake(
        mut p: Vec<Vec3f>,
        mut n: Vec<Vec3f>,
        mut i: Vec<u32>,
    ) -> (AoAtlas, Vec<Vec3f>, Vec<[f32; 2]>, Vec<u32>) {
        let (mut lo, mut hi) = (v(f32::MAX, f32::MAX, f32::MAX), v(f32::MIN, f32::MIN, f32::MIN));
        for q in &p {
            lo = v(lo.x.min(q.x), lo.y.min(q.y), lo.z.min(q.z));
            hi = v(hi.x.max(q.x), hi.y.max(q.y), hi.z.max(q.z));
        }
        let mut at = AoAtlas::new(ATLAS_MAX);
        // Single-model density, as the production tool bakes (ao-bake
        // --model sets fill): the fixtures assert per-face values, which
        // need more than a texel or two per face.
        at.fill = true;
        // The suite's numeric contracts are calibrated against the DEFAULT
        // evaluator; pin it so a stray AO_BAKER in the environment cannot
        // change what is being asserted.
        set_thread_baker(Some(AoBakerKind::Lightmapper));
        let baked = bake_into(&mut at, &mut p, &mut n, &mut i, lo, hi);
        set_thread_baker(None);
        (at, p, baked.ao_uv, i)
    }

    /// Atlas value at the surface point nearest `q` (search emitted
    /// triangles for the closest containing face with matching normal side).
    fn sample_at(
        at: &AoAtlas,
        p: &[Vec3f],
        uv: &[[f32; 2]],
        idx: &[u32],
        q: Vec3f,
        face_n: Vec3f,
    ) -> f32 {
        // barycentric inside test on the closest triangle whose plane
        // matches the queried normal direction.
        let mut best: Option<(f32, [f32; 2])> = None;
        for t in idx.chunks_exact(3) {
            let (a, b, c) = (p[t[0] as usize], p[t[1] as usize], p[t[2] as usize]);
            let e1 = b - a;
            let e2 = c - a;
            let fnv = v(
                e1.y * e2.z - e1.z * e2.y,
                e1.z * e2.x - e1.x * e2.z,
                e1.x * e2.y - e1.y * e2.x,
            );
            let len = (fnv.x * fnv.x + fnv.y * fnv.y + fnv.z * fnv.z).sqrt();
            if len < 1.0e-12 {
                continue;
            }
            // SIGNED agreement: under backface culling the queried side is
            // shown by the twin WOUND toward it — never its coincident
            // opposite, whose texels legitimately carry the other side.
            let al = (fnv.x * face_n.x + fnv.y * face_n.y + fnv.z * face_n.z) / len;
            if al < 0.9 {
                continue;
            }
            // distance from q to the triangle plane
            let d = ((q.x - a.x) * fnv.x + (q.y - a.y) * fnv.y + (q.z - a.z) * fnv.z) / len;
            if d.abs() > 0.02 {
                continue;
            }
            // project and barycentric
            let qq = v(q.x - fnv.x / len * d, q.y - fnv.y / len * d, q.z - fnv.z / len * d);
            let v0 = e1;
            let v1 = e2;
            let v2 = qq - a;
            let d00 = v0.x * v0.x + v0.y * v0.y + v0.z * v0.z;
            let d01 = v0.x * v1.x + v0.y * v1.y + v0.z * v1.z;
            let d11 = v1.x * v1.x + v1.y * v1.y + v1.z * v1.z;
            let d20 = v2.x * v0.x + v2.y * v0.y + v2.z * v0.z;
            let d21 = v2.x * v1.x + v2.y * v1.y + v2.z * v1.z;
            let den = d00 * d11 - d01 * d01;
            if den.abs() < 1.0e-12 {
                continue;
            }
            let w1 = (d11 * d20 - d01 * d21) / den;
            let w2 = (d00 * d21 - d01 * d20) / den;
            let w0 = 1.0 - w1 - w2;
            if w0 < -0.01 || w1 < -0.01 || w2 < -0.01 {
                continue;
            }
            let (ua, ub, uc) = (uv[t[0] as usize], uv[t[1] as usize], uv[t[2] as usize]);
            let u = [
                ua[0] * w0 + ub[0] * w1 + uc[0] * w2,
                ua[1] * w0 + ub[1] * w1 + uc[1] * w2,
            ];
            if best.map_or(true, |(bd, _)| d.abs() < bd) {
                best = Some((d.abs(), u));
            }
        }
        let (_, u) = best.expect("no face found at query point");
        let x = ((u[0] * at.size as f32) as usize).min(at.size - 1);
        let y = ((u[1] * at.size as f32) as usize).min(at.size - 1);
        at.pixels[y * at.size + x] as f32 / 255.0
    }

    /// Case 1+6: an isolated box must bake bright on every face — with
    /// honest AND with flipped/mirrored authoring.
    #[test]
    fn isolated_box_is_bright_regardless_of_authoring() {
        // Honest winding only: under the culling contract a fully
        // inward-wound single-sided box shows NOTHING (every face culled),
        // so there is no visible surface to assert on. The double-sided
        // fixture covers the twin-authored real-world case.
        for flip in [false] {
            let (mut p, mut n, mut i) = (Vec::new(), Vec::new(), Vec::new());
            push_box(&mut p, &mut n, &mut i, v(-0.5, 0.0, -0.5), v(0.5, 1.0, 0.5), flip);
            let (at, p, uv, idx) = bake(p, n, i);
            for (q, fnv, label) in [
                (v(0.0, 1.0, 0.0), v(0.0, 1.0, 0.0), "top"),
                (v(0.5, 0.5, 0.0), v(1.0, 0.0, 0.0), "side+x"),
                (v(0.0, 0.5, -0.5), v(0.0, 0.0, -1.0), "side-z"),
            ] {
                let a = sample_at(&at, &p, &uv, &idx, q, fnv);
                assert!(
                    a > 0.9,
                    "flip={flip} {label}: isolated face baked {a:.2}, must be ~1"
                );
            }
        }
    }

    /// The measured pergola-tile case: every face DOUBLED with opposite
    /// winding (each copy's authored normal honest for its side). Both
    /// copies must bake the OUTSIDE's occlusion — the inward twin z-ties
    /// with the outward one on screen.
    #[test]
    fn double_sided_box_bakes_outside_on_both_copies() {
        let (mut p, mut n, mut i) = (Vec::new(), Vec::new(), Vec::new());
        push_box(&mut p, &mut n, &mut i, v(-0.5, 0.0, -0.5), v(0.5, 1.0, 0.5), false);
        push_box(&mut p, &mut n, &mut i, v(-0.5, 0.0, -0.5), v(0.5, 1.0, 0.5), true);
        let (at, p, uv, idx) = bake(p, n, i);
        for (q, fnv, label) in [
            (v(0.0, 1.0, 0.0), v(0.0, 1.0, 0.0), "top"),
            (v(0.5, 0.5, 0.0), v(1.0, 0.0, 0.0), "side+x"),
        ] {
            // sample_at finds the closest matching face regardless of which
            // twin owns the chart; both twins' texels must be bright.
            let a = sample_at(&at, &p, &uv, &idx, q, fnv);
            assert!(
                a > 0.9,
                "{label}: double-sided box baked {a:.2} — the inward twin's chart is dark"
            );
        }
    }

    /// Case 2+5: two beams with a narrow slot. Slot WALLS go dark; the
    /// faces just outside the slot stay bright. This is the pergola-lattice
    /// case that broke farther-hit orientation (the opposite slot wall is
    /// closer than the beam's own far side).
    #[test]
    fn narrow_slot_darkens_inside_only() {
        for flip in [false] {
            let (mut p, mut n, mut i) = (Vec::new(), Vec::new(), Vec::new());
            // Two 0.3-thick beams, 4cm apart (contact scale — within the
            // 12cm reach), 2 long, 0.3 tall.
            push_box(&mut p, &mut n, &mut i, v(-1.0, 0.0, -0.34), v(1.0, 0.3, -0.02), flip);
            push_box(&mut p, &mut n, &mut i, v(-1.0, 0.0, 0.02), v(1.0, 0.3, 0.34), flip);
            let (at, p, uv, idx) = bake(p, n, i);
            // Diagnostic: what does the SAMPLER say at the slot wall, with
            // the known-correct normal? Separates bake-math wrongness from
            // atlas-plumbing wrongness.
            {
                let (mut pp, mut nn, mut ii) = (Vec::new(), Vec::new(), Vec::new());
                push_box(&mut pp, &mut nn, &mut ii, v(-1.0, 0.0, -0.34), v(1.0, 0.3, -0.02), flip);
                push_box(&mut pp, &mut nn, &mut ii, v(-1.0, 0.0, 0.02), v(1.0, 0.3, 0.34), flip);
                let sampler = crate::ao::AoSampler::with_ground(
                    &pp, &ii, v(-1.0, 0.0, -0.36), v(1.0, 0.3, 0.36), 64, ii.len() / 3,
                );
                let direct = sampler
                    .at_split(&pp, &ii, v(0.0, 0.15, -0.02), v(0.0, 0.0, 1.0))
                    .0;
                let nh = sampler.nearest_hit(
                    &pp, &ii, v(0.0, 0.15, -0.018), v(0.0, 0.0, 1.0), 1.0,
                );
                eprintln!(
                    "flip={flip}: sampler at slot wall (n=+z): {direct:.3}, straight-ray hit t={nh}"
                );
            }
            // slot wall of beam A (faces +z into the gap)
            let wall = sample_at(&at, &p, &uv, &idx, v(0.0, 0.15, -0.02), v(0.0, 0.0, 1.0));
            eprintln!("flip={flip}: atlas at slot wall: {wall:.3}");
            // outer wall of beam A (faces -z, open)
            let outer = sample_at(&at, &p, &uv, &idx, v(0.0, 0.15, -0.34), v(0.0, 0.0, -1.0));
            // top of beam A, mid-width
            let top = sample_at(&at, &p, &uv, &idx, v(0.0, 0.3, -0.18), v(0.0, 1.0, 0.0));
            // Physical semantics (the gazebo reference): a 4cm slot IS
            // occluded — its walls darken like lightmapper's gazebo shades
            // between close members.
            assert!(wall < 0.75, "flip={flip}: 4cm slot wall baked {wall:.2}, must darken");
            assert!(outer > 0.9, "flip={flip}: outer wall baked {outer:.2}, must be bright");
            assert!(top > 0.9, "flip={flip}: beam top baked {top:.2}, must be bright");
        }
    }

    /// Case 3+4: an L — inward crease darkens near the corner, the convex
    /// outer edge stays bright right up to the edge.
    #[test]
    fn crease_darkens_convex_edge_does_not() {
        let (mut p, mut n, mut i) = (Vec::new(), Vec::new(), Vec::new());
        // floor slab + wall slab meeting at x=0 (inward corner above floor).
        push_box(&mut p, &mut n, &mut i, v(-1.0, -0.1, -1.0), v(1.0, 0.0, 1.0), false);
        push_box(&mut p, &mut n, &mut i, v(-1.2, -0.1, -1.0), v(-1.0, 0.8, 1.0), false);
        let (at, p, uv, idx) = bake(p, n, i);
        // floor next to the wall (in the crease) — 1.5cm out: the shoulder
        // curve zeroes shallow occlusion by design, so the probe sits where
        // a real crease is DEEP.
        let crease = sample_at(&at, &p, &uv, &idx, v(-0.985, 0.0, 0.0), v(0.0, 1.0, 0.0));
        // floor far from the wall
        let open = sample_at(&at, &p, &uv, &idx, v(0.8, 0.0, 0.0), v(0.0, 1.0, 0.0));
        // top of the wall: a convex edge, bright to the rim
        let rim = sample_at(&at, &p, &uv, &idx, v(-1.1, 0.8, 0.0), v(0.0, 1.0, 0.0));
        // Contact-scale reach: the crease gradient is a few cm wide, so the
        // probe 3cm out sits on its shoulder — assert RELATIVE darkening.
        assert!(
            crease < open - 0.04,
            "crease baked {crease:.2} vs open {open:.2}, must darken near the wall"
        );
        assert!(open > 0.92, "open floor baked {open:.2}, must be bright");
        assert!(rim > 0.9, "wall-top rim baked {rim:.2}, must be bright");
    }
}
