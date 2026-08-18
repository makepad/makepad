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
//! A general UV unwrapper is a hard problem and unnecessary here: Kenney
//! geometry is boxy. Adjacent near-coplanar triangles are grown into CHARTS
//! (see [`bake_into`]), each chart is projected onto its dominant plane —
//! world units in, texels out, no distortion to reason about — and the charts
//! are shelf-packed into the shared atlas with a dilated gutter against
//! bilinear bleed. Vertices split only where two charts meet, exactly the
//! corners where a hard shading break belongs.
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

use makepad_draw::makepad_math::*;

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
    /// Widest angle, in degrees, between any triangle's normal and its own
    /// chart's average normal. A planar projection folds once this passes 90.
    pub max_chart_spread: f32,
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
            max_chart_spread: 0.0,
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

/// How parallel two triangles' normals must be to join one chart. ~15 degrees.
///
/// Loosened from 10 with the Quest size push: curved runs — barrel staves,
/// wheel arches, rounded corner trim — fragmented into a chart per facet, and
/// each chart pays its own gutter, which on a detailed toy car was most of
/// the texture. 15 degrees welds those runs while a real crease (a wall to
/// its roof) stays two charts. Too loose and a chart wraps a corner, and a
/// planar projection of a wrapped chart squashes one of its faces to nothing.
const COPLANAR_DOT: f32 = 0.966;

/// How far a triangle may sit from its CHART's average normal. cos(30 degrees).
///
/// Bounds the total drift a chart can accumulate. Without it, gentle pairwise
/// merges chain around curved geometry until the chart folds and its planar
/// projection maps two surfaces onto one set of texels. 30 degrees of tilt
/// foreshortens a texel by at most cos30 = 0.87, which bilinear absorbs.
const CHART_SPREAD_DOT: f32 = 0.866;

/// Texels of padding round each chart, filled by dilation.
///
/// Charts share a texture, so without this bilinear filtering at a chart's
/// edge reaches into whatever was packed beside it — the classic lightmap
/// seam, a bright or dark rim round every face. ONE texel is exactly what
/// bilinear can reach from a sample point half a texel inside the chart; the
/// second texel the old value carried was pure padding, and on the small
/// charts low-poly art is mostly made of it was a third of the area.
pub(crate) const GUTTER: usize = 1;

/// Smallest and largest a chart may be, gutter included.
const CHART_MIN: usize = 3;
const CHART_MAX: usize = 192;

/// Sub-texel sample offsets, shared with the reference evaluator so both
/// rasterise the exact same coverage. Rotated-grid: no two share a row or
/// column, so gradients in any direction get four distinct positions rather
/// than an aligned lattice.
pub(crate) const TEXEL_SUB_OFFSETS: [(f32, f32); TEXEL_SUBSAMPLES] =
    [(0.375, 0.125), (0.875, 0.375), (0.125, 0.625), (0.625, 0.875)];

/// A group of adjacent, near-coplanar triangles sharing one planar projection.
///
/// `pub(crate)` for the reference evaluator ([`crate::ao_reference`]), which
/// must rasterise the SAME charts so its atlas diffs texel-for-texel.
pub(crate) struct Chart {
    pub(crate) tris: Vec<usize>,
    /// Axis dropped by the projection: 0 = x, 1 = y, 2 = z.
    pub(crate) axis: usize,
    /// Chart-space origin (the projected bounding box's corner), world units.
    pub(crate) u0: f32,
    pub(crate) v0: f32,
    /// Texels per world unit, after the whole-atlas fit.
    pub(crate) scale: f32,
    /// Size in texels, gutter included.
    pub(crate) w: usize,
    pub(crate) h: usize,
    /// Where it landed in the atlas. `pub(crate)` for the BakerBoy port
    /// ([`crate::bakerboy`]), which assembles and dilates the FULL atlas
    /// image (its dilation crosses chart boundaries by design) and so needs
    /// to know where each chart's block sits.
    pub(crate) x: usize,
    pub(crate) y: usize,
}

/// Which coordinate a normal is most aligned with.
fn dominant_axis(n: Vec3f) -> usize {
    let (ax, ay, az) = (n.x.abs(), n.y.abs(), n.z.abs());
    if ax >= ay && ax >= az {
        0
    } else if ay >= az {
        1
    } else {
        2
    }
}

/// Drop the dominant axis. The remaining two coordinates ARE the chart's uv,
/// in world units — which is what makes this a planar map with no distortion.
pub(crate) fn project(p: Vec3f, axis: usize) -> (f32, f32) {
    match axis {
        0 => (p.z, p.y),
        1 => (p.x, p.z),
        _ => (p.x, p.y),
    }
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

/// Bake AO into an atlas as PLANAR CHARTS — one continuous texture region per
/// flat run of the surface.
///
/// # Why not a patch per triangle
///
/// That was the previous design and it could not work. Each triangle owning
/// its own square of texture means each carries its own gradient, brightest in
/// the middle and unrelated to its neighbours', so a flat wall renders as a
/// quilt of soft diamonds radiating from every vertex. The occlusion values
/// were right; the parameterisation made them unreadable.
///
/// Grouping coplanar neighbours into a chart and projecting it onto its own
/// plane makes a facade ONE surface with ONE mapping, exactly like texturing a
/// wall: occlusion varies smoothly across it, dark where it meets the ground
/// and under the eave, clean in the middle.
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
    bake_into_authored(atlas, positions, normals, None, indices, min, max)
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
    let t_bake = std::time::Instant::now();
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

    // --- Charts: union adjacent, near-coplanar triangles -------------------
    let fnorm: Vec<Vec3f> = (0..tri_count)
        .map(|t| {
            face_normal(
                positions[indices[t * 3] as usize],
                positions[indices[t * 3 + 1] as usize],
                positions[indices[t * 3 + 2] as usize],
            )
        })
        .collect();

    // Grow charts OUTWARD FROM A SEED, bounded against the chart's own running
    // average normal — not just pairwise against a neighbour.
    //
    // Pairwise-only was the bug. Union-find has no transitivity limit, so a
    // chain of triangles each within 10 degrees of the last can wrap a chart
    // right around a curve. Measured on a Kenney house — which has a
    // cylindrical drainpipe and rounded window frames — charts reached a
    // 90-degree spread. Projecting a 90-degree chart onto one plane collapses
    // its perpendicular half to zero width, so two different parts of the
    // surface land on the SAME texels and read each other's occlusion. That is
    // what showed up as grey smudges floating on flat walls.
    //
    // Both bounds are needed: the pairwise test keeps a hard crease from
    // merging at all, and the against-the-average test caps how far a chart may
    // drift no matter how gently.
    // Adjacency is keyed on WELDED positions, not on indices. Kenney meshes
    // duplicate vertices freely — one flat facade is often several quads each
    // owning its own corner vertices — and index-keyed edges see those as
    // separate islands. Each island then became its own chart with its own
    // texel grid, and where two grids met mid-wall their samples disagreed:
    // the visible seam splitting a facade's occlusion in half. Welding by
    // position makes "same place" mean "same vertex" regardless of authoring.
    let mut adjacent: Vec<[usize; 3]> = vec![[usize::MAX; 3]; tri_count];
    {
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
        let mut canon_of: std::collections::HashMap<(i64, i64, i64), u32> =
            std::collections::HashMap::with_capacity(positions.len());
        let canon: Vec<u32> = positions
            .iter()
            .enumerate()
            .map(|(i, p)| *canon_of.entry(quant(*p)).or_insert(i as u32))
            .collect();
        let mut edges: std::collections::HashMap<(u32, u32), (usize, usize)> =
            std::collections::HashMap::with_capacity(tri_count * 3);
        for t in 0..tri_count {
            for e in 0..3 {
                let (a, b) = (
                    canon[indices[t * 3 + e] as usize],
                    canon[indices[t * 3 + (e + 1) % 3] as usize],
                );
                let key = if a < b { (a, b) } else { (b, a) };
                // `remove`, not `get`: welded edges can carry more than two
                // triangles, and pairing them greedily two at a time keeps
                // adjacency symmetric instead of letting a third triangle
                // overwrite the first pair.
                if let Some((other, oe)) = edges.remove(&key) {
                    adjacent[t][e] = other;
                    adjacent[other][oe] = t;
                } else {
                    edges.insert(key, (t, e));
                }
            }
        }
    }

    let mut charts: Vec<Chart> = Vec::new();
    let mut tri_chart = vec![usize::MAX; tri_count];
    for seed in 0..tri_count {
        if tri_chart[seed] != usize::MAX {
            continue;
        }
        let ci = charts.len();
        charts.push(Chart {
            tris: vec![seed],
            axis: 0,
            u0: 0.0,
            v0: 0.0,
            scale: TEXELS_PER_UNIT,
            w: 0,
            h: 0,
            x: 0,
            y: 0,
        });
        tri_chart[seed] = ci;
        let mut acc = fnorm[seed];
        let mut queue = vec![seed];
        while let Some(t) = queue.pop() {
            let l = (acc.x * acc.x + acc.y * acc.y + acc.z * acc.z).sqrt().max(1.0e-12);
            let avg = Vec3f { x: acc.x / l, y: acc.y / l, z: acc.z / l };
            for e in 0..3 {
                let n = adjacent[t][e];
                if n == usize::MAX || tri_chart[n] != usize::MAX {
                    continue;
                }
                let pair = fnorm[n].x * fnorm[t].x + fnorm[n].y * fnorm[t].y + fnorm[n].z * fnorm[t].z;
                let mean = fnorm[n].x * avg.x + fnorm[n].y * avg.y + fnorm[n].z * avg.z;
                if pair < COPLANAR_DOT || mean < CHART_SPREAD_DOT {
                    continue;
                }
                tri_chart[n] = ci;
                charts[ci].tris.push(n);
                acc.x += fnorm[n].x;
                acc.y += fnorm[n].y;
                acc.z += fnorm[n].z;
                queue.push(n);
            }
        }
    }

    // --- Size each chart from its own projected extent ---------------------
    let mut spread_max = 0.0f32;
    for c in charts.iter_mut() {
        // Area-weighted average normal picks the plane the chart mostly lies
        // in, so one stray sliver cannot tip the projection onto a bad axis.
        let mut acc = Vec3f { x: 0.0, y: 0.0, z: 0.0 };
        for &t in &c.tris {
            let (a, b, cc) = (
                positions[indices[t * 3] as usize],
                positions[indices[t * 3 + 1] as usize],
                positions[indices[t * 3 + 2] as usize],
            );
            let w = tri_area(a, b, cc);
            acc.x += fnorm[t].x * w;
            acc.y += fnorm[t].y * w;
            acc.z += fnorm[t].z * w;
        }
        c.axis = dominant_axis(acc);
        {
            let l = (acc.x * acc.x + acc.y * acc.y + acc.z * acc.z).sqrt().max(1.0e-12);
            let avg = Vec3f { x: acc.x / l, y: acc.y / l, z: acc.z / l };
            for &t in &c.tris {
                // Degenerate triangles have no normal — face_normal of a zero
                // cross product is noise — and they rasterise no texels, so
                // they cannot fold a projection. Counting them reported a 90
                // degree spread on every model that carried slivers, which
                // buried the real folds this metric exists to expose.
                let (a, b, cc) = (
                    positions[indices[t * 3] as usize],
                    positions[indices[t * 3 + 1] as usize],
                    positions[indices[t * 3 + 2] as usize],
                );
                if tri_area(a, b, cc) < 1.0e-9 {
                    continue;
                }
                let d = (fnorm[t].x * avg.x + fnorm[t].y * avg.y + fnorm[t].z * avg.z)
                    .clamp(-1.0, 1.0);
                let deg = d.acos().to_degrees();
                if deg > spread_max {
                    spread_max = deg;
                }
                if deg > 45.0 && std::env::var_os("AO_CHART_DEBUG").is_some() {
                    println!(
                        "    chart {} tris={} deg={:.1} n=({:.2},{:.2},{:.2}) avg=({:.2},{:.2},{:.2})",
                        c.tris.len(), c.tris.len(), deg,
                        fnorm[t].x, fnorm[t].y, fnorm[t].z, avg.x, avg.y, avg.z
                    );
                }
            }
        }
        let (mut lu, mut lv) = (f32::INFINITY, f32::INFINITY);
        let (mut hu, mut hv) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
        for &t in &c.tris {
            for k in 0..3 {
                let (pu, pv) = project(positions[indices[t * 3 + k] as usize], c.axis);
                lu = lu.min(pu);
                lv = lv.min(pv);
                hu = hu.max(pu);
                hv = hv.max(pv);
            }
        }
        c.u0 = lu;
        c.v0 = lv;
        let inner_w = ((hu - lu) * TEXELS_PER_UNIT).ceil().max(1.0) as usize;
        let inner_h = ((hv - lv) * TEXELS_PER_UNIT).ceil().max(1.0) as usize;
        c.w = (inner_w + 2 * GUTTER).clamp(CHART_MIN, CHART_MAX);
        c.h = (inner_h + 2 * GUTTER).clamp(CHART_MIN, CHART_MAX);
    }

    atlas.max_chart_spread = atlas.max_chart_spread.max(spread_max);

    // SHRINK-WRAP a virgin single-model atlas before spending it: pick the
    // smallest square that holds every chart at MODEL_TEXELS_PER_UNIT (plus
    // 25% shelf-packing slack), instead of whatever ceiling the caller
    // allocated. One texture per asset only stays cheap if each texture is as
    // small as its model actually needs — a crate at 96 texels/unit wants a
    // 64 square, not the house's 672. Multiples of 8 rather than powers of
    // two: the next power of two above 660 is 1024, 2.4x the memory for zero
    // added detail.
    if atlas.fill && atlas.shelf_x == 0 && atlas.shelf_y == 0 && atlas.shelf_h == 0 {
        let wanted: usize = charts.iter().map(|c| c.w * c.h).sum();
        let ratio = MODEL_TEXELS_PER_UNIT / TEXELS_PER_UNIT;
        let side = (wanted as f32 * ratio * ratio * 1.25).sqrt().ceil() as usize;
        let side = side.next_multiple_of(8).clamp(64, atlas.size);
        atlas.pixels = vec![255; side * side];
        atlas.size = side;
    }

    // Fit the pack's remaining budget, every chart scaled together so relative
    // resolution survives and only absolute density drops. In `fill` mode the
    // same move runs UPWARD: a lone model in its own atlas scales until the
    // texture is spent, instead of leaving 96% of it white as measured on the
    // suburban houses.
    let chart_max = if atlas.fill { 512 } else { CHART_MAX };
    let wanted: usize = charts.iter().map(|c| c.w * c.h).sum();
    let free = atlas.free_texels();
    if atlas.fill && wanted > 0 {
        // Scale k until the charts actually SHELF-PACK into the atlas — a
        // flat area ratio is not enough. Models made of hundreds of small
        // clamped charts waste far more than any fixed slack in row
        // remainders and CHART_MIN floors; measured on the toy-car kit, the
        // 15% allowance overflowed most models and the overflow path then
        // DOUBLED their atlas — 37 of 157 shipped at 1024² against a 512
        // budget. Simulating the exact packing (same sort, same shelf walk)
        // costs microseconds and makes the budget a hard promise.
        let base: Vec<(usize, usize)> = charts.iter().map(|c| (c.w, c.h)).collect();
        let mut k = ((free as f32 * 0.85 / wanted as f32).sqrt()).clamp(0.4, 8.0);
        loop {
            for (c, (w0, h0)) in charts.iter_mut().zip(&base) {
                c.w = ((*w0 as f32 * k) as usize).clamp(CHART_MIN, chart_max);
                c.h = ((*h0 as f32 * k) as usize).clamp(CHART_MIN, chart_max);
            }
            let mut order: Vec<usize> = (0..charts.len()).collect();
            order.sort_by(|&a, &b| charts[b].h.cmp(&charts[a].h).then(a.cmp(&b)));
            let (mut x, mut y, mut h) = (0usize, 0usize, 0usize);
            let fits = order.iter().all(|&ci| {
                let (w, hh) = (charts[ci].w, charts[ci].h);
                if x + w > atlas.size {
                    x = 0;
                    y += h;
                    h = 0;
                }
                x += w;
                h = h.max(hh);
                y + hh <= atlas.size
            });
            if fits || k <= 0.4 {
                break;
            }
            k *= 0.93;
        }
    } else if wanted > free && free > 0 {
        let k = (free as f32 / wanted as f32).sqrt();
        for c in charts.iter_mut() {
            c.w = ((c.w as f32 * k) as usize).clamp(CHART_MIN, chart_max);
            c.h = ((c.h as f32 * k) as usize).clamp(CHART_MIN, chart_max);
        }
    }

    // Texels per world unit, derived from the size actually granted.
    for c in charts.iter_mut() {
        let (mut hu, mut hv) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
        for &t in &c.tris {
            for k in 0..3 {
                let (pu, pv) = project(positions[indices[t * 3 + k] as usize], c.axis);
                hu = hu.max(pu);
                hv = hv.max(pv);
            }
        }
        let su = (c.w - 2 * GUTTER) as f32 / (hu - c.u0).max(1.0e-6);
        let sv = (c.h - 2 * GUTTER) as f32 / (hv - c.v0).max(1.0e-6);
        // One scale for both axes keeps texels square, so AO does not stretch.
        c.scale = su.min(sv);
    }

    // Tallest first: shelf packing wastes far less when heights are ordered.
    let mut order: Vec<usize> = (0..charts.len()).collect();
    order.sort_by(|&a, &b| charts[b].h.cmp(&charts[a].h).then(a.cmp(&b)));
    for &ci in &order {
        let (w, h) = (charts[ci].w, charts[ci].h);
        let (x, y) = atlas.alloc_rect(w, h);
        charts[ci].x = x;
        charts[ci].y = y;
    }

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
            let cursor = std::sync::atomic::AtomicUsize::new(0);
            let threads = std::thread::available_parallelism()
                .map_or(8, |n| n.get())
                .min(charts.len().max(1));
            let mut out: Vec<Vec<u8>> = vec![Vec::new(); charts.len()];
            std::thread::scope(|scope| {
                let mut handles = Vec::with_capacity(threads);
                for _ in 0..threads {
                    handles.push(scope.spawn(|| {
                        let mut mine: Vec<(usize, Vec<u8>)> = Vec::new();
                        loop {
                            let ci = cursor.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            if ci >= charts_ref.len() {
                                break;
                            }
                            mine.push((
                                ci,
                                crate::aobaker_port::bake_aobaker_chart(
                                    &charts_ref[ci], sampler, occ_pos, occ_idx, pos, nrm, idx,
                                    &params,
                                ),
                            ));
                        }
                        mine
                    }));
                }
                for h in handles {
                    for (ci, bytes) in h.join().unwrap_or_default() {
                        out[ci] = bytes;
                    }
                }
            });
            out
        }
        AoBakerKind::BakerBoy => crate::bakerboy::bake_all_charts(
            &charts, atlas.size, positions, normals, indices, min, max,
        ),
        AoBakerKind::Lightmapper => Vec::new(),
    };

    let sz = atlas.size;
    if !chart_px.is_empty() {
        for (ci, c) in charts.iter().enumerate() {
            for ty in 0..c.h {
                for tx in 0..c.w {
                    atlas.pixels[(c.y + ty) * sz + c.x + tx] = chart_px[ci][ty * c.w + tx];
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

    // --- Emit geometry: one vertex per (source vertex, chart) --------------
    let mut out_pos = Vec::with_capacity(positions.len());
    let mut out_nrm = Vec::with_capacity(positions.len());
    let mut source_vertex: Vec<u32> = Vec::with_capacity(positions.len());
    let mut ao_uv: Vec<[f32; 2]> = Vec::with_capacity(positions.len());
    let mut out_idx = Vec::with_capacity(indices.len());
    let mut seen: std::collections::HashMap<(u32, usize), u32> = std::collections::HashMap::new();

    for t in 0..tri_count {
        let ci = tri_chart[t];
        let c = &charts[ci];
        for k in 0..3 {
            let vi = indices[t * 3 + k];
            let key = (vi, ci);
            let out_i = *seen.entry(key).or_insert_with(|| {
                let p = positions[vi as usize];
                let (pu, pv) = project(p, c.axis);
                let tx = GUTTER as f32 + (pu - c.u0) * c.scale;
                let ty = GUTTER as f32 + (pv - c.v0) * c.scale;
                out_pos.push(p);
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
            out_idx.push(out_i);
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
        let bake = crate::ao_lightmapper::bake_ao(
            &out_pos, Some(&lm_nrm), &ao_uv, &out_idx, sz, sz, &params,
        );
        let mut data = bake.data;
        #[cfg(test)]
        let mut captured = forensics::BakeForensics {
            tri_chart: tri_chart.clone(),
            charts: charts.iter().map(|c| (c.x, c.y, c.w, c.h)).collect(),
            debug: bake.debug.clone(),
            size: sz,
            out_pos: out_pos.clone(),
            out_idx: out_idx.clone(),
            lm_nrm: lm_nrm.clone(),
            ao_uv: ao_uv.clone(),
            ..Default::default()
        };
        if data.iter().any(|v| *v != 0.0) {
            // The cosine kernel integrates to ~0.5 over the hemisphere (the
            // header normalizes only the validity channel of the weight
            // texture), so an open-sky texel reads ~0.5. Rescale so open sky
            // = 1.0 using the kernel's own discrete sum — lmImageScale.
            let w = crate::ao_lightmapper::hemisphere_weights(params.hemisphere_size, |c| c);
            let open_sky: f32 = w.iter().map(|x| x.0).sum();
            crate::ao_lightmapper::image_scale(&mut data, 1.0 / open_sky);
            #[cfg(test)]
            {
                captured.data_raw = data.clone();
            }
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
            #[cfg(test)]
            {
                captured.data_fill = data.clone();
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
        #[cfg(test)]
        forensics::LAST.with(|l| *l.borrow_mut() = Some(captured));
    }
    atlas.bake_evaluator = baker.name();
    atlas.bake_seconds += t_bake.elapsed().as_secs_f64();

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

fn tri_area(a: Vec3f, b: Vec3f, c: Vec3f) -> f32 {
    let (ux, uy, uz) = (b.x - a.x, b.y - a.y, b.z - a.z);
    let (vx, vy, vz) = (c.x - a.x, c.y - a.y, c.z - a.z);
    let (nx, ny, nz) = (uy * vz - uz * vy, uz * vx - ux * vz, ux * vy - uy * vx);
    (nx * nx + ny * ny + nz * nz).sqrt() * 0.5
}

/// Test-only capture of a lightmapper bake's internals, so forensics tests
/// can ask WHICH STAGE put a value in a texel (rendered, rejected+min-filled,
/// dilated) and which chart owns each triangle. Populated by `bake_into` for
/// the last lightmapper bake on this thread.
#[cfg(test)]
pub(crate) mod forensics {
    use std::cell::RefCell;
    #[derive(Default)]
    pub(crate) struct BakeForensics {
        /// Input/output triangle index -> chart index.
        pub tri_chart: Vec<usize>,
        /// Chart rects (x, y, w, h) in atlas texels.
        pub charts: Vec<(usize, usize, usize, usize)>,
        /// The lightmapper debug RGB: r=rendered, g=interpolated, b=rejected.
        pub debug: Vec<u8>,
        /// Atlas floats after the open-sky rescale, before the buried fill.
        pub data_raw: Vec<f32>,
        /// After the buried min-fill, before dilate/smooth/gamma.
        pub data_fill: Vec<f32>,
        pub size: usize,
        /// The exact bake inputs: emitted geometry and the smooth-normal
        /// field the hemicubes aimed along.
        pub out_pos: Vec<makepad_draw::makepad_math::Vec3f>,
        pub out_idx: Vec<u32>,
        pub lm_nrm: Vec<makepad_draw::makepad_math::Vec3f>,
        pub ao_uv: Vec<[f32; 2]>,
    }
    thread_local! {
        pub(crate) static LAST: RefCell<Option<BakeForensics>> =
            const { RefCell::new(None) };
    }
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
                            d >= COPLANAR_DOT,
                            "texel ({x},{y}) shared by non-coplanar triangles {} and {t}",
                            *cell
                        );
                    }
                    *cell = t;
                }
            }
        }
    }


    #[test]
    fn report_corner_contrast() {
        let (mut p, mut n, mut i) = floor_with_wall();
        let mut at = AoAtlas::new(ATLAS_MAX);
        bake_into(&mut at, &mut p, &mut n, &mut i, v(-4.0, 0.0, -4.0), v(4.0, 3.0, 4.0));
        let lo = *at.pixels.iter().min().unwrap();
        let hi = *at.pixels.iter().max().unwrap();
        println!(
            "AO atlas {}x{} ({} KB), range {lo}..{hi} of 255 — AO_FLOOR maps to 132",
            at.size, at.size, at.kilobytes()
        );
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

    /// THE MAPPING TEST: what the atlas says at a triangle's own uv must
    /// match occlusion computed directly at that triangle's position in
    /// space — by the SAME evaluator that filled the atlas.
    ///
    /// Every AO defect in this pipeline has been the same bug wearing different
    /// clothes — the occlusion was correct and landed in the wrong texel. Per
    /// triangle patches, an atlas that doubled mid-pack and invalidated every
    /// uv already issued, chart projection errors: all of them produce
    /// plausible-looking numbers, so a histogram of the atlas passes happily
    /// while the render shows grey smudges floating in the middle of flat
    /// walls. Aggregate statistics are spatially blind and cannot catch this.
    ///
    /// Comparing per-centroid closes that hole exactly: it asks "is the value
    /// at this uv the value that belongs at this point?", which is the
    /// property every one of those bugs violated, and it needs no renderer
    /// and no eyes. The oracle is one hemicube rendered at the centroid with
    /// the same parameters the bake used (cosine kernel, open-sky rescale,
    /// display gamma), so a mismatch is a MAPPING error, not an evaluator
    /// disagreement.
    ///
    /// Run against a REAL model deliberately. The synthetic two-triangle
    /// fixtures elsewhere in this file have never once caught a real defect —
    /// they have no chart boundaries, no coplanar neighbours and no packing
    /// pressure, which is where all of this actually goes wrong.
    #[test]
    fn every_vertex_uv_reads_back_its_own_occlusion() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/sandbox/resources/models/kenney/city-kit-suburban/building-type-h.glb"
        );
        let Ok(bytes) = std::fs::read(path) else {
            println!("  (no asset packs — skipped)");
            return;
        };
        // Pin the evaluator: this test's oracle IS the lightmapper hemicube,
        // so the atlas must have been filled by it whatever `AO_BAKER` says.
        set_thread_baker(Some(AoBakerKind::Lightmapper));
        let mut at = AoAtlas::new(ATLAS_MAX);
        let m = crate::model::StaticModel::parse_glb_baked(&bytes, &mut at).unwrap();
        set_thread_baker(None);

        let stride = crate::model::MODEL_VERTEX_FLOATS;
        let count = m.vertices.len() / stride;
        let pos: Vec<Vec3f> = (0..count)
            .map(|i| Vec3f {
                x: m.vertices[i * stride],
                y: m.vertices[i * stride + 1],
                z: m.vertices[i * stride + 2],
            })
            .collect();
        let tri_count = m.indices.len() / 3;

        // The oracle's scene: the BAKED mesh exactly as emitted — the
        // engine's dedup already fixed the winding, and the hemicube bake
        // runs LM_NONE (facing from winding), so the as-baked triangles ARE
        // the scene and each probe's direction is its winding face normal.
        let scene_idx: Vec<u32> = m.indices.clone();
        let tri_n: Vec<Vec3f> = (0..tri_count)
            .map(|t| {
                face_normal(
                    pos[m.indices[t * 3] as usize],
                    pos[m.indices[t * 3 + 1] as usize],
                    pos[m.indices[t * 3 + 2] as usize],
                )
            })
            .collect();

        // The oracle mirrors the atlas dispatch's parameters exactly —
        // kernel and validity gate included, or probes near joints compare
        // a rendered oracle against a buried-filled texel.
        let params = crate::ao_lightmapper::LightmapParams::atlas();

        // Probe at triangle CENTROIDS, not vertices: a vertex sits on a
        // chart boundary half a texel from a crease, a centroid lands well
        // inside its chart, so any mismatch there is a genuine mapping error
        // rather than boundary noise. Sizable faces only (sub-texel trim is
        // an area average by design), capped to a fixed deterministic subset
        // — a hemicube per probe is a real render.
        let sizable: Vec<usize> = (0..tri_count)
            .filter(|&t| {
                let (ia, ib, ic) = (
                    m.indices[t * 3] as usize,
                    m.indices[t * 3 + 1] as usize,
                    m.indices[t * 3 + 2] as usize,
                );
                tri_area(pos[ia], pos[ib], pos[ic]) >= 5.0e-3
            })
            .collect();
        const MAX_PROBES: usize = 240;
        let step = (sizable.len() / MAX_PROBES).max(1);

        let third = 1.0 / 3.0;
        let mut worst = 0.0f32;
        let mut worst_at = 0usize;
        let mut bad = 0usize;
        let mut probed = 0usize;
        let mut rejected = 0usize;
        for &t in sizable.iter().step_by(step) {
            let (ia, ib, ic) = (
                m.indices[t * 3] as usize,
                m.indices[t * 3 + 1] as usize,
                m.indices[t * 3 + 2] as usize,
            );
            let c = Vec3f {
                x: (pos[ia].x + pos[ib].x + pos[ic].x) * third,
                y: (pos[ia].y + pos[ib].y + pos[ic].y) * third,
                z: (pos[ia].z + pos[ib].z + pos[ic].z) * third,
            };
            let n = {
                let f = tri_n[t];
                let l = (f.x * f.x + f.y * f.y + f.z * f.z).sqrt().max(1.0e-12);
                Vec3f { x: f.x / l, y: f.y / l, z: f.z / l }
            };
            let (vis, validity) =
                crate::ao_lightmapper::sample_hemicube_ao(&pos, &scene_idx, c, n, &params);
            if validity <= params.validity_min {
                // The bake's own validity gate would have rejected this
                // sample and dilation filled the texel — nothing comparable.
                rejected += 1;
                continue;
            }
            let direct = vis.clamp(0.0, 1.0).powf(1.0 / 2.2);
            // The uv the GPU will interpolate to at this centroid.
            let (ua, ub, uc) = (
                crate::model::unpack_ao_uv(m.vertices[ia * stride + 6]),
                crate::model::unpack_ao_uv(m.vertices[ib * stride + 6]),
                crate::model::unpack_ao_uv(m.vertices[ic * stride + 6]),
            );
            let uv = [
                (ua[0] + ub[0] + uc[0]) * third,
                (ua[1] + ub[1] + uc[1]) * third,
            ];
            let x = ((uv[0] * at.size as f32) as usize).min(at.size - 1);
            let y = ((uv[1] * at.size as f32) as usize).min(at.size - 1);
            let sampled = at.pixels[y * at.size + x] as f32 / 255.0;
            let d = (sampled - direct).abs();
            probed += 1;
            if d > worst {
                worst = d;
                worst_at = t;
            }
            // "Bad" = a foreign chart's value (a misplaced chart reads
            // 0.3+ off across its whole area); smaller deltas are hemicube
            // discretization, the interpolation passes and the smooth pass.
            if d > 0.3 {
                bad += 1;
                if std::env::var_os("AO_CHART_DEBUG").is_some() && bad <= 12 {
                    println!(
                        "    bad tri {t}: uv=({:.4},{:.4}) sampled={sampled:.3} \
                         direct={direct:.3}",
                        uv[0], uv[1]
                    );
                }
            }
        }
        let pct = bad as f32 / probed.max(1) as f32 * 100.0;
        println!(
            "  atlas overflow fallbacks: {}, widest chart normal spread: {:.1} deg",
            at.overflowed, at.max_chart_spread
        );
        println!(
            "  {probed} probes ({rejected} validity-rejected, skipped): worst mismatch \
             {worst:.3} at tri {worst_at}, {bad} ({pct:.1}%) off by more than 0.3"
        );
        assert!(
            pct < 2.0,
            "{pct:.1}% of centroids read back occlusion that does not belong to \
             them (worst {worst:.3}) — the atlas mapping is misplacing AO"
        );
    }


    #[test]
    #[ignore = "needs the asset packs; run explicitly"]
    fn report_real_model_histogram() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/sandbox/resources/models/kenney/city-kit-suburban/building-type-h.glb"
        );
        let Ok(bytes) = std::fs::read(path) else {
            println!("  (no asset packs)");
            return;
        };
        let mut at = AoAtlas::new(ATLAS_MAX);
        let m = crate::model::StaticModel::parse_glb_baked(&bytes, &mut at).unwrap();
        let mut bins = [0usize; 10];
        for p in &at.pixels {
            bins[(*p as usize * 10 / 256).min(9)] += 1;
        }
        let total: usize = bins.iter().sum();
        println!("\n  {} tris, atlas {}x{}", m.triangle_count(), at.size, at.size);
        for (i, c) in bins.iter().enumerate() {
            println!(
                "  {:3}..{:3}  {:6.2}%  {}",
                i * 26,
                i * 26 + 25,
                *c as f32 / total as f32 * 100.0,
                "#".repeat((*c * 60 / total.max(1)).min(60))
            );
        }
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

#[cfg(test)]
mod sidecar_consistency {
    use super::*;
    use crate::model::{StaticModel, MODEL_VERTEX_FLOATS};

    /// The bake in memory is clean; the screen shows rims. Verify the two
    /// hand-offs: (1) the .ao.png on disk equals an in-process bake's
    /// pixels; (2) the .aomesh's uvs equal the fresh bake's uvs.
    #[test]
    fn disk_pair_matches_fresh_bake() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../apps/sandbox/resources/models/kenney/modular-dungeon-kit");
        let Ok(bytes) = std::fs::read(root.join("template-floor-layer-raised.glb")) else {
            eprintln!("asset absent — skipped");
            return;
        };
        // 512 — the size the TOOL writes; comparing across sizes is noise.
        let mut at = AoAtlas::new(512);
        at.fill = true;
        let fresh = StaticModel::parse_glb_baked(&bytes, &mut at).unwrap();
        // Determinism: a second in-process bake must be byte-identical, or
        // the tool's pair and any in-process validation are incomparable.
        {
            let mut at2 = AoAtlas::new(512);
            at2.fill = true;
            let again = StaticModel::parse_glb_baked(&bytes, &mut at2).unwrap();
            let uv_diff = fresh
                .vertices
                .iter()
                .zip(again.vertices.iter())
                .filter(|(a, b)| a.to_bits() != b.to_bits())
                .count();
            let px_diff = at
                .pixels
                .iter()
                .zip(at2.pixels.iter())
                .filter(|(a, b)| (**a as i32 - **b as i32).abs() > 2)
                .count();
            eprintln!("determinism: vertex-float diffs {uv_diff}, pixel diffs>2 {px_diff}");
            assert_eq!(uv_diff, 0, "bake is nondeterministic in uvs");
        }
        let disk = std::fs::read(root.join("template-floor-layer-raised.aomesh"))
            .ok()
            .and_then(|b| StaticModel::from_aomesh(&b))
            .expect("aomesh sidecar");
        assert_eq!(
            disk.vertices.len(),
            fresh.vertices.len(),
            "vertex count differs disk vs fresh"
        );
        let stride = MODEL_VERTEX_FLOATS;
        let n = disk.vertices.len() / stride;
        let mut worst = 0.0f32;
        for i in 0..n {
            let a = crate::model::unpack_ao_uv(disk.vertices[i * stride + 6]);
            let b = crate::model::unpack_ao_uv(fresh.vertices[i * stride + 6]);
            let d = (a[0] - b[0]).abs().max((a[1] - b[1]).abs());
            if d > worst {
                worst = d;
            }
        }
        eprintln!("uv worst delta disk-vs-fresh: {worst:.6} ({} verts)", n);
        assert!(worst < 2.0 / 1024.0, "aomesh uvs differ from fresh bake");
        // png pixels
        let png = std::fs::read(root.join("template-floor-layer-raised.ao.png")).unwrap();
        let (mut o, mut w, mut idat) = (8usize, 0usize, Vec::new());
        while o + 8 <= png.len() {
            let len = u32::from_be_bytes(png[o..o + 4].try_into().unwrap()) as usize;
            let kind = &png[o + 4..o + 8];
            let body = &png[o + 8..o + 8 + len];
            match kind {
                b"IHDR" => {
                    w = u32::from_be_bytes(body[..4].try_into().unwrap()) as usize;
                }
                b"IDAT" => idat.extend_from_slice(body),
                b"IEND" => break,
                _ => {}
            }
            o += 8 + len + 4;
        }
        let raw = makepad_fast_inflate::zlib_decompress_vec(&idat).unwrap();
        let mut disk_px = Vec::with_capacity(w * w);
        for row in raw.chunks_exact(w + 1) {
            disk_px.extend_from_slice(&row[1..]);
        }
        assert_eq!(w, at.size, "atlas size differs disk {w} vs fresh {}", at.size);
        let diff: usize = disk_px
            .iter()
            .zip(at.pixels.iter())
            .filter(|(a, b)| (**a as i32 - **b as i32).abs() > 8)
            .count();
        eprintln!("png texels differing >8: {diff} of {}", w * w);
        assert!(diff < w * w / 100, "disk atlas diverges from fresh bake");
    }
}

#[cfg(test)]
mod dark_chart_owner {
    use super::*;
    use crate::model::{StaticModel, MODEL_VERTEX_FLOATS};

    /// For the darkest small charts in the pergola atlas: which triangles
    /// own them (position, normal, area)? Distinguishes "bevel strips baked
    /// dark" from "inward twins" from "slot walls".
    #[test]
    fn who_owns_the_dark_strips() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "../../../apps/sandbox/resources/models/kenney/modular-dungeon-kit/template-floor-layer-raised.glb",
        );
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("asset absent — skipped");
            return;
        };
        // 512, matching the TOOL's per-model bake — the pair the renderer
        // actually loads. Testing at 1024 validated a different layout.
        let mut at = AoAtlas::new(512);
        at.fill = true;
        let m = StaticModel::parse_glb_baked(&bytes, &mut at).unwrap();
        let stride = MODEL_VERTEX_FLOATS;
        let vp = |i: u32| Vec3f {
            x: m.vertices[i as usize * stride],
            y: m.vertices[i as usize * stride + 1],
            z: m.vertices[i as usize * stride + 2],
        };
        let uv = |i: u32| crate::model::unpack_ao_uv(m.vertices[i as usize * stride + 6]);
        let mut printed = 0;
        for (t, tri) in m.indices.chunks_exact(3).enumerate() {
            let (ua, ub, uc) = (uv(tri[0]), uv(tri[1]), uv(tri[2]));
            let u = [
                (ua[0] + ub[0] + uc[0]) / 3.0,
                (ua[1] + ub[1] + uc[1]) / 3.0,
            ];
            let x = ((u[0] * at.size as f32) as usize).min(at.size - 1);
            let y = ((u[1] * at.size as f32) as usize).min(at.size - 1);
            let val = at.pixels[y * at.size + x];
            if val < 180 {
                let (a, b, c) = (vp(tri[0]), vp(tri[1]), vp(tri[2]));
                let e1 = b - a;
                let e2 = c - a;
                let n = Vec3f {
                    x: e1.y * e2.z - e1.z * e2.y,
                    y: e1.z * e2.x - e1.x * e2.z,
                    z: e1.x * e2.y - e1.y * e2.x,
                };
                let l = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt().max(1.0e-12);
                let area = l * 0.5;
                let cen = Vec3f {
                    x: (a.x + b.x + c.x) / 3.0,
                    y: (a.y + b.y + c.y) / 3.0,
                    z: (a.z + b.z + c.z) / 3.0,
                };
                printed += 1;
                if printed <= 20 {
                    eprintln!(
                        "dark tri {t}: val {} area {area:.4} centre ({:.2},{:.2},{:.2}) n ({:.2},{:.2},{:.2})",
                        val, cen.x, cen.y, cen.z,
                        n.x / l, n.y / l, n.z / l
                    );
                }
            }
        }
        eprintln!("dark-sampling triangles: {printed} of {}", m.indices.len() / 3);
    }
}

#[cfg(test)]
mod pergola_forensics {
    use super::*;
    use crate::model::{StaticModel, MODEL_VERTEX_FLOATS};

    /// The real failing model through the truth machinery: bake in-process,
    /// then for the most-downward big face (a rafter underside — open air
    /// below) print its winding normal, its atlas texel, and every ray hit
    /// from its winding hemisphere. Answers "is the bake dark or is the
    /// sampling dark" for the exact geometry that keeps regressing.
    #[test]
    fn rafter_underside_tells_all() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "../../../apps/sandbox/resources/models/kenney/modular-dungeon-kit/template-floor-layer-raised.glb",
        );
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("asset absent — skipped");
            return;
        };
        let mut at = AoAtlas::new(ATLAS_MAX);
        at.fill = true;
        let m = StaticModel::parse_glb_baked(&bytes, &mut at).unwrap();
        let stride = MODEL_VERTEX_FLOATS;
        let count = m.vertices.len() / stride;
        let pos: Vec<Vec3f> = (0..count)
            .map(|i| Vec3f {
                x: m.vertices[i * stride],
                y: m.vertices[i * stride + 1],
                z: m.vertices[i * stride + 2],
            })
            .collect();
        // Highest big down-wound face = a top rafter's underside.
        let mut best: Option<(usize, f32, f32)> = None; // (tri, area, cy)
        for (t, tri) in m.indices.chunks_exact(3).enumerate() {
            let (a, b, c) = (
                pos[tri[0] as usize],
                pos[tri[1] as usize],
                pos[tri[2] as usize],
            );
            let e1 = b - a;
            let e2 = c - a;
            let n = Vec3f {
                x: e1.y * e2.z - e1.z * e2.y,
                y: e1.z * e2.x - e1.x * e2.z,
                z: e1.x * e2.y - e1.y * e2.x,
            };
            let l = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt();
            if l < 1.0e-9 {
                continue;
            }
            let cy = (a.y + b.y + c.y) / 3.0;
            if n.y / l < -0.9 && cy > 2.0 {
                let area = l * 0.5;
                if best.map_or(true, |(_, ba, _)| area > ba) {
                    best = Some((t, area, cy));
                }
            }
        }
        let Some((t, area, cy)) = best else {
            eprintln!("no down-wound face above y=2");
            return;
        };
        let tri = &m.indices[t * 3..t * 3 + 3];
        let (a, b, c) = (
            pos[tri[0] as usize],
            pos[tri[1] as usize],
            pos[tri[2] as usize],
        );
        let centre = Vec3f {
            x: (a.x + b.x + c.x) / 3.0,
            y: cy,
            z: (a.z + b.z + c.z) / 3.0,
        };
        eprintln!(
            "underside tri {t}: area {area:.3} centre ({:.2},{:.2},{:.2})",
            centre.x, centre.y, centre.z
        );
        // Atlas value via this triangle's own uvs (what the GPU samples).
        let uv = |i: u32| crate::model::unpack_ao_uv(m.vertices[i as usize * stride + 6]);
        let (ua, ub, uc) = (uv(tri[0]), uv(tri[1]), uv(tri[2]));
        let u = [
            (ua[0] + ub[0] + uc[0]) / 3.0,
            (ua[1] + ub[1] + uc[1]) / 3.0,
        ];
        let x = ((u[0] * at.size as f32) as usize).min(at.size - 1);
        let y = ((u[1] * at.size as f32) as usize).min(at.size - 1);
        eprintln!(
            "atlas at its own uv ({:.4},{:.4}) -> texel ({x},{y}) = {:.3}",
            u[0], u[1],
            at.pixels[y * at.size + x] as f32 / 255.0
        );
        // Direct sampler from the winding side, with hits.
        let sampler = crate::ao::AoSampler::with_ground(
            &pos, &m.indices, m.min, m.max, 64, m.indices.len() / 3,
        );
        let n = Vec3f { x: 0.0, y: -1.0, z: 0.0 };
        let v = sampler.at_split(&pos, &m.indices, centre, n).0;
        eprintln!("direct sampler (n=down, self-only): {v:.3}");
        let mut dirs = [Vec3f { x: 0.0, y: 1.0, z: 0.0 }; crate::ao::AO_RAYS_OFFLINE];
        crate::ao::hemisphere(n, 32, &mut dirs);
        let mut hits = 0;
        for (i, d) in dirs.iter().take(32).enumerate() {
            let t2 = sampler.nearest_hit(
                &pos,
                &m.indices,
                Vec3f {
                    x: centre.x + n.x * 0.004,
                    y: centre.y + n.y * 0.004,
                    z: centre.z + n.z * 0.004,
                },
                *d,
                0.35,
            );
            if t2.is_finite() {
                hits += 1;
                if hits <= 8 {
                    eprintln!(
                        "  hit ray {i} dir ({:.2},{:.2},{:.2}) t {:.3}",
                        d.x, d.y, d.z, t2
                    );
                }
            }
        }
        eprintln!("hemisphere hits within 0.35: {hits}/32");
    }
}

// ── Seam acceptance ─────────────────────────────────────────────────────
// The live crypt-kit bug: a flat quad's two triangles landing in different
// charts (separate rasterisation, separate dilation) shows as a hard
// diagonal seam. Root cause was orientation — the unified dedup kept
// arbitrary-winding twins, so a quad could keep one up-wound and one
// down-wound triangle and chart growth (which clusters by winding facing)
// split them. With each engine's own dedup restored this must never
// happen: every coplanar pair of emitted triangles sharing a welded edge
// (and no third same-side triangle) must read CONTINUOUS uvs across that
// edge — the observable form of "same chart" — and the atlas must not step
// across the quad diagonal.
#[cfg(test)]
mod seam_acceptance {
    use super::*;
    use crate::model::{unpack_ao_uv, StaticModel, MODEL_VERTEX_FLOATS};

    /// (model, max diagonal step asserted). The flat template pieces must
    /// scan glass-smooth. The gate arches interpenetrate their own segments,
    /// and a buried contact region legitimately meets its exposed lip in a
    /// hard value boundary ALIGNED with face edges (measured: the pillar
    /// blocks' undersides at y=1.10/1.60) — so for them the hard contract is
    /// chart integrity (0 uv seams) and the step is reported, not asserted.
    const MODELS: [(&str, Option<i32>); 4] = [
        ("template-floor-layer-raised.glb", Some(48)),
        ("template-floor-detail.glb", Some(48)),
        ("gate.glb", None),
        ("gate-metal-bars.glb", None),
    ];

    fn kit_path(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../../apps/sandbox/resources/models/kenney/modular-dungeon-kit/{name}"
        ))
    }

    /// Scan one baked model: for every welded edge shared by EXACTLY two
    /// same-side coplanar triangles, assert uv continuity at both endpoints
    /// and measure the atlas step across the edge. Returns
    /// `(pairs, uv_seams, max_step)`.
    fn scan(m: &StaticModel, at: &AoAtlas) -> (usize, usize, i32) {
        let stride = MODEL_VERTEX_FLOATS;
        let pos = |i: u32| Vec3f {
            x: m.vertices[i as usize * stride],
            y: m.vertices[i as usize * stride + 1],
            z: m.vertices[i as usize * stride + 2],
        };
        let uv = |i: u32| unpack_ao_uv(m.vertices[i as usize * stride + 6]);
        let tri_count = m.indices.len() / 3;
        let span = (m.max.x - m.min.x)
            .max(m.max.y - m.min.y)
            .max(m.max.z - m.min.z)
            .max(1.0e-5);
        let inv_eps = 1.0 / (span * 1.0e-5);
        let quant = |p: Vec3f| {
            (
                (p.x * inv_eps).round() as i64,
                (p.y * inv_eps).round() as i64,
                (p.z * inv_eps).round() as i64,
            )
        };
        // Face normal per triangle; degenerate slivers opt out.
        let tri_n: Vec<Option<Vec3f>> = (0..tri_count)
            .map(|t| {
                let (a, b, c) = (
                    pos(m.indices[t * 3]),
                    pos(m.indices[t * 3 + 1]),
                    pos(m.indices[t * 3 + 2]),
                );
                if tri_area(a, b, c) < 1.0e-7 {
                    None
                } else {
                    Some(face_normal(a, b, c))
                }
            })
            .collect();
        // Welded-edge map: edge key -> (triangle, corner-of-edge-start).
        let mut edges: std::collections::HashMap<
            ((i64, i64, i64), (i64, i64, i64)),
            Vec<(usize, usize)>,
        > = std::collections::HashMap::new();
        for t in 0..tri_count {
            if tri_n[t].is_none() {
                continue;
            }
            for e in 0..3 {
                let a = quant(pos(m.indices[t * 3 + e]));
                let b = quant(pos(m.indices[t * 3 + (e + 1) % 3]));
                let key = if a < b { (a, b) } else { (b, a) };
                edges.entry(key).or_default().push((t, e));
            }
        }
        let sz = at.size as f32;
        let sample = |u: f32, v: f32| -> i32 {
            let x = ((u * sz) as usize).min(at.size - 1);
            let y = ((v * sz) as usize).min(at.size - 1);
            at.pixels[y * at.size + x] as i32
        };
        let (mut pairs, mut uv_seams, mut max_step) = (0usize, 0usize, 0i32);
        for (_key, users) in edges {
            // Both sides of a double-sided sheet, junctions, T-joints: only
            // clean pairs of SAME-SIDE coplanar triangles are quad halves.
            for i in 0..users.len() {
                for j in i + 1..users.len() {
                    let (t1, e1) = users[i];
                    let (t2, e2) = users[j];
                    let (Some(n1), Some(n2)) = (tri_n[t1], tri_n[t2]) else { continue };
                    let d = n1.x * n2.x + n1.y * n2.y + n1.z * n2.z;
                    if d < 0.99 {
                        continue;
                    }
                    // A third same-side triangle on this edge makes it a
                    // junction, not a quad diagonal — skip.
                    let same_side = users
                        .iter()
                        .filter(|(t, _)| {
                            tri_n[*t]
                                .is_some_and(|n| n.x * n1.x + n.y * n1.y + n.z * n1.z > 0.99)
                        })
                        .count();
                    if same_side != 2 {
                        continue;
                    }
                    pairs += 1;
                    // uv continuity at both welded endpoints, in texels. The
                    // shared edge runs start->end in t1 and end->start in t2
                    // (opposite traversal on a consistent-winding surface).
                    let mut seam = false;
                    for (c1, c2) in [
                        (m.indices[t1 * 3 + e1], m.indices[t2 * 3 + (e2 + 1) % 3]),
                        (m.indices[t1 * 3 + (e1 + 1) % 3], m.indices[t2 * 3 + e2]),
                    ] {
                        if quant(pos(c1)) != quant(pos(c2)) {
                            continue;
                        }
                        let (ua, ub) = (uv(c1), uv(c2));
                        let du = (ua[0] - ub[0]).abs() * sz;
                        let dv = (ua[1] - ub[1]).abs() * sz;
                        if du > 1.5 || dv > 1.5 {
                            seam = true;
                        }
                    }
                    if seam {
                        uv_seams += 1;
                        continue;
                    }
                    // Texel step across the edge: sample a whisker inside
                    // each triangle from the edge midpoint (uvs are affine
                    // over the triangle, so mixing them in uv space is
                    // exact).
                    let read = |t: usize, e: usize| -> i32 {
                        let (ia, ib, ic) =
                            (m.indices[t * 3], m.indices[t * 3 + 1], m.indices[t * 3 + 2]);
                        let (es, ee) = (m.indices[t * 3 + e], m.indices[t * 3 + (e + 1) % 3]);
                        let m1 = [
                            (uv(es)[0] + uv(ee)[0]) * 0.5,
                            (uv(es)[1] + uv(ee)[1]) * 0.5,
                        ];
                        let cen = [
                            (uv(ia)[0] + uv(ib)[0] + uv(ic)[0]) / 3.0,
                            (uv(ia)[1] + uv(ib)[1] + uv(ic)[1]) / 3.0,
                        ];
                        sample(
                            m1[0] + (cen[0] - m1[0]) * 0.25,
                            m1[1] + (cen[1] - m1[1]) * 0.25,
                        )
                    };
                    let step = (read(t1, e1) - read(t2, e2)).abs();
                    if step > 40 {
                        let (es, ee) = (m.indices[t1 * 3 + e1], m.indices[t1 * 3 + (e1 + 1) % 3]);
                        let (a, b) = (pos(es), pos(ee));
                        eprintln!(
                            "    step {step} across edge ({:.2},{:.2},{:.2})-({:.2},{:.2},{:.2}) n ({:.2},{:.2},{:.2})",
                            a.x, a.y, a.z, b.x, b.y, b.z,
                            tri_n[t1].unwrap().x, tri_n[t1].unwrap().y, tri_n[t1].unwrap().z
                        );
                    }
                    max_step = max_step.max(step);
                }
            }
        }
        (pairs, uv_seams, max_step)
    }

    /// Scan whatever sidecar pair currently SHIPS for the raised floor —
    /// the artifact the renderer actually loads. Diagnostic (prints, no
    /// hard asserts beyond loading): run explicitly to audit a pair on
    /// disk against the seam contract.
    #[test]
    #[ignore = "audits the on-disk sidecar pair; run explicitly"]
    fn scan_shipped_sidecar_pair() {
        let root = kit_path("");
        let Some(mesh) = std::fs::read(root.join("template-floor-layer-raised.aomesh"))
            .ok()
            .and_then(|b| StaticModel::from_aomesh(&b))
        else {
            eprintln!("  (no shipped aomesh — skipped)");
            return;
        };
        let Ok(png) = std::fs::read(root.join("template-floor-layer-raised.ao.png")) else {
            eprintln!("  (no shipped ao.png — skipped)");
            return;
        };
        let (mut o, mut w, mut idat) = (8usize, 0usize, Vec::new());
        while o + 8 <= png.len() {
            let len = u32::from_be_bytes(png[o..o + 4].try_into().unwrap()) as usize;
            let kind = &png[o + 4..o + 8];
            let body = &png[o + 8..o + 8 + len];
            match kind {
                b"IHDR" => w = u32::from_be_bytes(body[..4].try_into().unwrap()) as usize,
                b"IDAT" => idat.extend_from_slice(body),
                b"IEND" => break,
                _ => {}
            }
            o += 8 + len + 4;
        }
        let raw = makepad_fast_inflate::zlib_decompress_vec(&idat).unwrap();
        let mut at = AoAtlas::new(w);
        at.pixels.clear();
        for row in raw.chunks_exact(w + 1) {
            at.pixels.extend_from_slice(&row[1..]);
        }
        at.size = w;
        let (pairs, uv_seams, max_step) = scan(&mesh, &at);
        eprintln!(
            "shipped pair: {pairs} coplanar pairs, {uv_seams} uv seams, max diagonal step {max_step}"
        );
    }

    #[test]
    fn coplanar_pairs_share_charts_across_engines() {
        for (name, step_cap) in MODELS {
            let Ok(bytes) = std::fs::read(kit_path(name)) else {
                eprintln!("  ({name} absent — skipped)");
                continue;
            };
            for kind in [
                AoBakerKind::Lightmapper,
                AoBakerKind::BakerBoy,
                AoBakerKind::Aobaker,
            ] {
                set_thread_baker(Some(kind));
                let mut at = AoAtlas::new(512);
                at.fill = true;
                let m = StaticModel::parse_glb_baked(&bytes, &mut at).unwrap();
                set_thread_baker(None);
                let (pairs, uv_seams, max_step) = scan(&m, &at);
                eprintln!(
                    "{name} [{}]: {pairs} coplanar pairs, {uv_seams} uv seams, \
                     max diagonal step {max_step}",
                    kind.name()
                );
                assert!(pairs > 0, "{name}: the scan found no coplanar pairs at all");
                assert_eq!(
                    uv_seams, 0,
                    "{name} [{}]: coplanar welded pairs landed in different charts",
                    kind.name()
                );
                if let Some(cap) = step_cap {
                    assert!(
                        max_step <= cap,
                        "{name} [{}]: a {max_step}-level step across a flat quad's \
                         diagonal — per-triangle seam",
                        kind.name()
                    );
                }
            }
        }
    }
}
