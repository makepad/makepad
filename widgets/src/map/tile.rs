use super::geometry::*;
use super::icons::*;
use super::label::*;
use super::style::*;
use crate::makepad_draw::vector::{
    append_expanded_stroke_geometry, append_tessellated_geometry,
    append_tessellated_geometry_decked, compute_clip_radii, map_fill_variant_code,
    pack_fill_vertices, pack_road_vertices, pack_vector_vertices,
    VECTOR_PACKED_FLOATS_PER_VERTEX,
    tessellate_path_fill, LineCap, LineJoin, Tessellator, VVertex,
    VectorPath, VectorRenderParams, VECTOR_ANALYTIC_FRINGE_STROKE_MULT,
    VECTOR_FLOATS_PER_VERTEX, VECTOR_ZBIAS_STEP,
};
use crate::makepad_draw::*;
use crate::makepad_platform::makepad_micro_serde::*;
use makepad_fast_inflate::{gzip_decompress_vec, zlib_decompress_vec};
use makepad_mbtile_reader::{MbtilesReader, TileArchiveReader};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const OVERPASS_ENDPOINTS: &[&str] = &["https://overpass.kumi.systems/api/interpreter"];
pub const MAX_PENDING_REQUESTS: usize = 2;
pub const MAX_TILE_RETRIES: u8 = 6;
pub const RETRY_BASE_FRAMES: u64 = 30;
pub const RETRY_MAX_FRAMES: u64 = 300;
pub const TILE_CACHE_DIR: &str = "local/tilecache_v4";
pub const TILE_QUERY_PAD: f64 = 0.05;
// Default archive: the curated Europe Shortbread base produced by
// `./tools/download_map.sh convert`. Apps can override per-widget via the
// MapView `mbtiles_path` property (examples/map pins Noord-Holland).
pub const LOCAL_MBTILES_PATH: &str = "local/maps/europe-shortbread.mbtiles";
pub const LOCAL_MBTILES_MIN_ZOOM: u32 = 0;
pub const LOCAL_MBTILES_MAX_ZOOM: u32 = 14;
// Fills are clipped to their own tile square (+ tiny overlap against AA
// hairlines) so a tile's buffer fragments never overpaint the neighbor.
pub const FILL_CLIP_OVERLAP: f32 = 0.25;
pub const ROAD_SMOOTH_FACTOR: f32 = 0.0;
pub const BUILDING_OUTLINE_MIN_ZOOM: u32 = 15;
pub const BUILDING_OUTLINE_WIDTH_PX: f32 = 0.9;
pub const EARCUT_MAX_RINGS: usize = 500;

const MVT_INTERNAL_FEATURE_KEY: &str = "__mp_feature";
const MVT_INTERNAL_RING_INDEX_KEY: &str = "__mp_ring";
/// Stable tilt-mode road depth bands. Unlike the old per-tile face ladder,
/// these values depend only on road semantics, so padded copies and adjacent
/// tiles render a shared surface at exactly the same depth.
const ROAD_SURFACE_PLAZA_DEPTH: f32 = 0.06;
const ROAD_SURFACE_CASING_DEPTH: f32 = 0.08;
pub const ROAD_UNION_CENTER_DEPTH: f32 = 0.24;
const ROAD_DEPTH_MAX_MICRO: f32 = 730.0 * DEPTH_MICRO_PER_RANK;
const ROAD_SUNK_DEPTH_OFFSET: f32 = 0.40;
const ROAD_TUNNEL_CASING_DEPTH: f32 = 0.0498;
const ROAD_TUNNEL_CENTER_DEPTH: f32 = 0.0500;
const ROAD_FASCIA_DEPTH_EPSILON: f32 = 0.00005;
const ROAD_FRINGE_DEPTH_EPSILON: f32 = 0.00005;
const ROAD_STROKE_PASS_DEPTH_OFFSET: f32 = 0.02;
// The icon draw call is +0.04 above the casing call in MapView. Subtract it
// from arrow param5, then restore only this tiny own-surface decal epsilon.
const ARROW_ICON_PASS_DEPTH_OFFSET: f32 = 0.04;
/// Baked shadow decals (T3): above the entire grounded road micro-depth
/// ladder (strokes reach 0.22 + 0.146 rank micro) so shadows darken the
/// streets they fall across, but below lifted bridge decks (param5 + 0.30
/// bumps) so a deck still draws over the shadow pooling under it.
const SHADOW_DECAL_DEPTH: f32 = 0.40;
/// Extruded building surfaces (walls, roofs, canopy balls): above every
/// ground DECAL including shadows. A wall pixel N px up its quad carries
/// the depth of ground N px behind it, so any decal with a bigger param5
/// than the wall would cut a band across the building base whenever the
/// camera rotation puts that decal behind the building on screen.
const BUILDING_SURFACE_DEPTH: f32 = 0.50;
const ARROW_DECAL_DEPTH_EPSILON: f32 = 0.0001;
const MVT_INTERNAL_FIDX_KEY: &str = "__mp_fidx";
const MVT_INTERNAL_PIDX_KEY: &str = "__mp_pidx";

// --- Tile state types ---

#[derive(Debug)]
pub enum TileLoadState {
    LoadingNetwork,
    LoadingLocal,
    Ready {
        fill_geometry: Option<Geometry>,
        /// Non-fill records formerly interleaved with ground fills (building
        /// outline strokes), retained on the generic vector layout.
        fill_misc_geometry: Option<Geometry>,
        casing_geometry: Option<Geometry>,
        stroke_geometry: Option<Geometry>,
        icon_geometry: Option<Geometry>,
        /// Street-band icons (zoom floor > ICON_HIGH_BAND_FLOOR) — drawn
        /// only when the view can actually reveal them.
        icon_high_geometry: Option<Geometry>,
        /// Instanced POI symbols (records only; the meshes are shared per
        /// slot on the view), same band split as the vertex streams.
        icon_instances: Vec<IconInstances>,
        icon_high_instances: Vec<IconInstances>,
        /// Tree/signal contact-shadow discs, drawn into the shadow mask.
        shadow_disc_geometry: Option<Geometry>,
        /// Analytic AA fringes — skipped at strong tilt where blur and
        /// density hide 1px edge AA.
        fringe_geometry: Option<Geometry>,
        /// 3D volume geometry, distance-faded from the view focus.
        fill_3d_geometry: Option<Geometry>,
        wall_geometry: Option<Geometry>,
        /// Building walls as instance records (see `WALL_INSTANCE_FLOATS`).
        wall_instances: Vec<f32>,
        tree_geometry: Option<Geometry>,
        tree_cross_geometry: Option<Geometry>,
        /// The tile's street-tree templates (near ring / mid ring) and the
        /// per-tree records placing them.
        tree_template_geometry: Option<Geometry>,
        tree_cross_template_geometry: Option<Geometry>,
        tree_instances: Vec<f32>,
        feature_count: usize,
        labels: Vec<TileLabel>,
        pin_hits: Vec<PinHit>,
    },
    Failed {
        retry_after: u64,
    },
}

#[derive(Debug)]
pub struct TileEntry {
    pub state: TileLoadState,
    pub last_used: u64,
    pub attempts: u8,
    /// Earliest frame a REBUILD of a still-drawable stale entry may be
    /// re-requested (backoff after a failed rebuild). A failed rebuild must
    /// never replace live stale geometry with a gray placeholder; the
    /// backoff lives here instead of in TileLoadState::Failed.
    pub retry_after: u64,
    /// Geometry buffer footprint (CPU-side floats at bake time; the GPU
    /// copy is the same order of magnitude). Drives byte-budget eviction —
    /// 3D building tiles reach 60-90 MB each, so a tile-count cap alone
    /// lets the cache take the machine out.
    pub bytes: usize,
    /// View-zoom bucket the geometry was styled for; stale buckets stay
    /// drawable while a rebuild is in flight.
    pub bucket: u32,
    /// This bake carries 3D extrusions (buildings/trees/signals).
    pub baked_3d: bool,
    /// Whether casing/stroke GPU geometry and the CPU road-arrow subset are
    /// available for a same-bucket 2D/3D overlay-only rebake.
    pub road_core_cached: bool,
    pub road_icon_indices: Vec<u32>,
    pub road_icon_vertices: Vec<f32>,
    /// Cross-fade state: the replaced generation's geometry stays drawable
    /// underneath while the new one fades in.
    pub fade: Option<TileFade>,
}

#[derive(Debug)]
pub struct TileFade {
    pub started: f64,
    /// Render bucket the outgoing geometry was styled for, so its stroke
    /// widths can be corrected while it fades out.
    pub bucket: u32,
    /// This fade is the flat->3D transition: the incoming bake grows its
    /// heights with the fade. 3D->3D rebakes keep full height (alpha-only
    /// crossfade) so zoom regens never replay the animation.
    pub grow_heights: bool,
    /// The incoming casing/stroke handles are the unchanged resident road
    /// core. They stay fully opaque and at full height while only the
    /// mode-dependent fill/icon overlay cross-fades.
    pub reuse_road_core: bool,
    pub fill_geometry: Option<Geometry>,
    pub fill_misc_geometry: Option<Geometry>,
    pub casing_geometry: Option<Geometry>,
    pub stroke_geometry: Option<Geometry>,
    pub icon_geometry: Option<Geometry>,
    /// The outgoing generation's instanced symbols (low band only: the
    /// street band is gated by zoom, not faded).
    pub icon_instances: Vec<IconInstances>,
}

#[derive(Debug)]
pub struct PendingTileRequest {
    pub tile_key: TileKey,
    pub endpoint: &'static str,
}

#[derive(Debug)]
pub enum TileWorkerMessage {
    LocalBatchLoaded {
        style_epoch: u64,
        requested: Vec<TileKey>,
        loaded: Vec<LoadedLocalTile>,
        /// Keys whose tile data exists but failed to decode — retryable,
        /// unlike keys absent from the archive.
        failed: Vec<TileKey>,
    },
    LocalBatchFailed {
        style_epoch: u64,
        requested: Vec<TileKey>,
        error: String,
    },
    NetworkTileParsed {
        style_epoch: u64,
        tile_key: TileKey,
        buffers: TileBuffers,
    },
    NetworkTileParseFailed {
        style_epoch: u64,
        tile_key: TileKey,
        error: String,
    },
}

#[derive(Debug)]
pub struct LoadedLocalTile {
    pub tile_key: TileKey,
    pub buffers: TileBuffers,
}

// --- Internal data types ---

#[derive(Debug)]
struct WayData {
    nodes: Vec<i64>,
    tags: HashMap<String, String>,
    closed: bool,
}

/// A tappable pin baked into a tile: normalized world position + the
/// attributes the info bubble shows.
#[derive(Debug, Clone, PartialEq)]
pub struct PinHit {
    pub norm: (f64, f64),
    pub info: Vec<(String, String)>,
    /// 3D stalk height of this pin's marker (0 = grounded).
    pub lift_m: f32,
}

/// Zoom floor above which icons go to the high (street) band; the draw
/// skips that band entirely below view ~16.25.
pub const ICON_HIGH_BAND_FLOOR: f32 = 16.01;

/// Partition icon geometry by per-vertex zoom floor (slot 15): triangles
/// whose vertices carry a floor above the threshold move to the high band.
/// One O(n) pass on the builder thread; vertex records are remapped
/// per-band so both halves are self-contained (verts, indices) pairs.
fn split_icon_band(
    vertices: &mut Vec<f32>,
    indices: &mut Vec<u32>,
) -> (Vec<f32>, Vec<u32>) {
    split_band_by(vertices, indices, |record| record[15] > ICON_HIGH_BAND_FLOOR)
}

/// Partition the analytic AA fringes (stroke_mult sentinel 2e6, slot 8)
/// out of the casing buffer: at strong tilt the tilt-shift blur and
/// geometry density hide 1px edge AA, and the fringes are ~2/3 of the
/// casing vertex mass on street tiles.
fn split_fringe_band(
    vertices: &mut Vec<f32>,
    indices: &mut Vec<u32>,
) -> (Vec<f32>, Vec<u32>) {
    split_band_by(vertices, indices, |record| record[8] > 1.5e6)
}

fn split_band_by(
    vertices: &mut Vec<f32>,
    indices: &mut Vec<u32>,
    predicate: impl Fn(&[f32]) -> bool,
) -> (Vec<f32>, Vec<u32>) {
    let vert_count = vertices.len() / VECTOR_FLOATS_PER_VERTEX;
    let mut is_high = vec![false; vert_count];
    let mut any_high = false;
    for (vi, record) in vertices.chunks_exact(VECTOR_FLOATS_PER_VERTEX).enumerate() {
        if predicate(record) {
            is_high[vi] = true;
            any_high = true;
        }
    }
    if !any_high {
        return (Vec::new(), Vec::new());
    }
    let mut low_vertices = Vec::new();
    let mut low_indices = Vec::new();
    let mut high_vertices = Vec::new();
    let mut high_indices = Vec::new();
    // Old vertex index -> new index in its band (u32::MAX = unmapped).
    let mut remap = vec![u32::MAX; vert_count];
    for tri in indices.chunks_exact(3) {
        let high = is_high[tri[0] as usize];
        let (band_verts, band_indices) = if high {
            (&mut high_vertices, &mut high_indices)
        } else {
            (&mut low_vertices, &mut low_indices)
        };
        for &old in tri {
            let slot = &mut remap[old as usize];
            if *slot == u32::MAX || (is_high[old as usize] != high) {
                // (mixed triangles duplicate the odd vertex into this band)
            }
            let mapped = if *slot != u32::MAX && is_high[old as usize] == high {
                *slot
            } else {
                let new_index = (band_verts.len() / VECTOR_FLOATS_PER_VERTEX) as u32;
                let start = old as usize * VECTOR_FLOATS_PER_VERTEX;
                band_verts.extend_from_slice(
                    &vertices[start..start + VECTOR_FLOATS_PER_VERTEX],
                );
                if is_high[old as usize] == high {
                    *slot = new_index;
                }
                new_index
            };
            band_indices.push(mapped);
        }
    }
    *vertices = low_vertices;
    *indices = low_indices;
    (high_vertices, high_indices)
}

/// Floats per icon instance: anchor xy, screen-px offset xy, scale, the
/// param4 composite (zoom floor + pin lift), zbias, unorm8x4 colour.
pub const ICON_INSTANCE_FLOATS: usize = 8;

/// Floats per building-wall instance: edge a xy, edge b xy, base m, top m,
/// outward normal xy, bottom AO, unorm8x4 colour, zbias. The wall quad is
/// extruded from this record in the vertex shader (`DrawMapWall`) — the
/// bake no longer writes four 48-byte vertices per footprint edge.
pub const WALL_INSTANCE_FLOATS: usize = 11;

/// Floats per street-tree instance: anchor xy and the zbias shift. Every
/// tree in a tile is the same mesh (`tree_template` near, `tree_cross_template`
/// at the mid LOD ring), drawn once per record with the anchor added in the
/// vertex shader.
pub const TREE_INSTANCE_FLOATS: usize = 3;

/// One symbol mesh drawn N times: the mesh lives once on the GPU (per
/// registry slot), every placement is an 8-float instance record instead of
/// a copy of the tessellated SVG at 48 bytes a vertex.
#[derive(Debug, PartialEq, Clone, Default)]
pub struct IconInstances {
    pub mesh_slot: u16,
    pub data: Vec<f32>,
}

impl IconInstances {
    pub fn count(&self) -> usize {
        self.data.len() / ICON_INSTANCE_FLOATS
    }
}

#[derive(Debug, PartialEq)]
pub struct TileBuffers {
    pub pin_hits: Vec<PinHit>,
    pub fill_indices: Vec<u32>,
    pub fill_vertices: Vec<f32>,
    pub fill_misc_indices: Vec<u32>,
    pub fill_misc_vertices: Vec<f32>,
    /// Compact eight-float `RoadVertexPacked` records: GPU-expandable strokes
    /// (shape >= 100) and shape-0 Boolean union faces.
    pub casing_indices: Vec<u32>,
    pub casing_vertices: Vec<f32>,
    /// Compact eight-float `RoadVertexPacked` records. Rails, dashed tunnels
    /// and other patterned lines go through `append_expanded_stroke_geometry`
    /// (shape 11/12 become 111/112); plaza fills are shape-0. Oneway arrows
    /// stay in `icon_*` / `road_icon_*`. No leftover non-road shapes, so
    /// there is no `stroke_misc` stream.
    pub stroke_indices: Vec<u32>,
    pub stroke_vertices: Vec<f32>,
    /// Vertex-baked symbols that must ride the map plane: road-surface
    /// decals (oneway arrows). Free-standing POI symbols are instances.
    pub icon_indices: Vec<u32>,
    pub icon_vertices: Vec<f32>,
    /// Street-band icons (per-vertex zoom floor > ICON_HIGH_BAND_FLOOR):
    /// the z18 horizon carries millions of shader-collapsed icon verts
    /// that mid zooms never show — split out so the draw skips the whole
    /// band below the floor instead of vertex-processing it every frame.
    pub icon_high_indices: Vec<u32>,
    pub icon_high_vertices: Vec<f32>,
    /// Instanced POI symbols, grouped per mesh slot; the same band split as
    /// the vertex streams (floor <= ICON_HIGH_BAND_FLOOR here).
    pub icon_instances: Vec<IconInstances>,
    pub icon_high_instances: Vec<IconInstances>,
    /// Tree/signal contact-shadow discs (material 6), drawn into the
    /// shadow mask pass as coverage rather than as ground decals.
    pub shadow_disc_indices: Vec<u32>,
    pub shadow_disc_vertices: Vec<f32>,
    /// Analytic AA fringes split from `casing_*` (see split_fringe_band),
    /// stored as compact eight-float `RoadVertexPacked` records and drawn
    /// with `DrawMapRoad` (same shader as casing/stroke; 25° tilt gate).
    pub fringe_indices: Vec<u32>,
    pub fringe_vertices: Vec<f32>,
    /// 3D volume geometry (walls/roofs/trees/skirts): distance-faded under
    /// tilt so the far field skips its vertex mass.
    pub fill_3d_indices: Vec<u32>,
    pub fill_3d_vertices: Vec<f32>,
    /// Building walls (MAT_WALL) — skipped at the mid LOD ring.
    pub wall_indices: Vec<u32>,
    pub wall_vertices: Vec<f32>,
    /// Building walls as instanced edge records (`WALL_INSTANCE_FLOATS` each);
    /// drawn with the wall band's LOD gate.
    pub wall_instances: Vec<f32>,
    /// Full canopy balls (MAT_CANOPY) — near ring only.
    pub tree_indices: Vec<u32>,
    pub tree_vertices: Vec<f32>,
    /// Crossed-quad tree stand-ins for the mid/far rings.
    pub tree_cross_indices: Vec<u32>,
    pub tree_cross_vertices: Vec<f32>,
    /// One street tree at the origin (trunk + canopy ball, GPU-packed) and
    /// its crossed-quad stand-in; `tree_instances` places them.
    pub tree_template_indices: Vec<u32>,
    pub tree_template_vertices: Vec<f32>,
    pub tree_cross_template_indices: Vec<u32>,
    pub tree_cross_template_vertices: Vec<f32>,
    pub tree_instances: Vec<f32>,
    /// Stable oneway-arrow subset of `icon_*`. The UI keeps this small CPU
    /// copy beside the resident GPU road meshes, then appends it to a
    /// mode-only 2D/3D icon rebake without regenerating the road Boolean.
    pub road_icon_indices: Vec<u32>,
    pub road_icon_vertices: Vec<f32>,
    /// This bake intentionally omitted all tilt-invariant road geometry and
    /// must reuse the resident tile's casing/stroke/road-icon core.
    pub mode_overlay_only: bool,
    pub feature_count: usize,
    pub labels: Vec<TileLabel>,
    /// View-zoom bucket this tile's styling was built for.
    pub render_zoom: u32,
    /// Compact per-stage build timing ("stage:ms stage:ms ..."), filled for
    /// builds over ~100ms — carried into the SLOW-tile log so a slow build
    /// is replayable headlessly without re-hitting it in-app.
    pub stage_summary: String,
}

impl TileBuffers {
    /// Geometry byte footprint (vertex + index data).
    pub fn byte_size(&self) -> usize {
        (self.fill_indices.len()
            + self.fill_vertices.len()
            + self.fill_misc_indices.len()
            + self.fill_misc_vertices.len()
            + self.casing_indices.len()
            + self.casing_vertices.len()
            + self.stroke_indices.len()
            + self.stroke_vertices.len()
            + self.icon_indices.len()
            + self.icon_high_indices.len()
            + self.icon_high_vertices.len()
            + self.fringe_indices.len()
            + self.fringe_vertices.len()
            + self.fill_3d_indices.len()
            + self.fill_3d_vertices.len()
            + self.wall_indices.len()
            + self.wall_vertices.len()
            + self.tree_indices.len()
            + self.tree_vertices.len()
            + self.tree_cross_indices.len()
            + self.tree_cross_vertices.len()
            + self.shadow_disc_indices.len()
            + self.shadow_disc_vertices.len()
            + self.icon_vertices.len()
            + self.road_icon_indices.len()
            + self.road_icon_vertices.len()
            + self.tree_template_indices.len()
            + self.tree_template_vertices.len()
            + self.tree_cross_template_indices.len()
            + self.tree_cross_template_vertices.len()
            + self.tree_instances.len()
            + self.wall_instances.len()
            + self.icon_instance_floats())
            * 4
    }

    /// Instance floats across both icon bands.
    pub fn icon_instance_floats(&self) -> usize {
        self.icon_instances
            .iter()
            .chain(self.icon_high_instances.iter())
            .map(|group| group.data.len())
            .sum()
    }

    /// Restore the cached road decals after a mode-only overlay bake. Road
    /// arrows share the icon draw call so their surface-depth compensation
    /// remains identical to a full bake.
    pub fn append_cached_road_icons(
        &mut self,
        road_indices: &[u32],
        road_vertices: &[f32],
    ) {
        if road_indices.is_empty() || road_vertices.is_empty() {
            return;
        }
        // icon_vertices is GPU-PACKED at this point (12-slot stride); the
        // cached road decals are kept as logical 19-float records — pack
        // them on the way in and base indices on the packed stride.
        let vertex_base =
            (self.icon_vertices.len() / VECTOR_PACKED_FLOATS_PER_VERTEX) as u32;
        self.icon_indices
            .extend(road_indices.iter().map(|index| index + vertex_base));
        self.icon_vertices
            .extend_from_slice(&pack_vector_vertices(road_vertices));
        self.road_icon_indices.clear();
        self.road_icon_indices.extend_from_slice(road_indices);
        self.road_icon_vertices.clear();
        self.road_icon_vertices.extend_from_slice(road_vertices);
    }
}

#[derive(Clone, Debug)]
struct StrokeDrawJob {
    sort_rank: i16,
    style: StrokeStyle,
    points: Vec<(f32, f32)>,
    /// Physical solid-road geometry joins its tier's boolean surface mesh.
    /// Patterned lines and non-road strokes stay in the vector stroke path.
    solid_road_surface: bool,
    /// Per-point deck heights (base_dz join), aligned with `points`.
    dz: Option<Vec<f32>>,
    /// Solid-road paint and physical vertical identity. Patterned/non-road
    /// strokes leave this empty and stay on the regular stroke path.
    surface_key: Option<RoadSurfaceKey>,
    /// Road semantics retained for safe 3D split/merge reconciliation.
    join_meta: RoadJoinMeta,
}

/// The surface depth slot an oneway arrow must decal onto. Solid-road
/// surfaces acquire their final slot after boolean painting; patterned
/// strokes already carry a stable param5.
#[derive(Clone, Copy, Debug)]
enum ArrowSurfaceDepth {
    Union(RoadSurfaceKey),
    Stroke { level: i8, depth_micro: f32 },
    Unknown,
}

#[derive(Clone, Debug)]
struct ArrowDrawJob {
    points: Vec<(f32, f32)>,
    reverse: bool,
    /// Dense, signed deck profile aligned with `points`. Keeping this on the
    /// source way prevents a nearby parallel/crossing road from lending its
    /// height to the arrow.
    dz: Option<Vec<f32>>,
    surface_depth: ArrowSurfaceDepth,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
struct StrokePassKey {
    color: u32,
    width_bits: u32,
    shape_id_bits: u32,
    depth_micro_bits: u32,
}

impl From<StrokePassStyle> for StrokePassKey {
    fn from(value: StrokePassStyle) -> Self {
        Self {
            color: value.color,
            width_bits: value.width.to_bits(),
            shape_id_bits: value.shape_id.to_bits(),
            depth_micro_bits: value.depth_micro.to_bits(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
struct StrokeStyleKey {
    sort_rank: i16,
    casing: Option<StrokePassKey>,
    center: StrokePassKey,
}

impl From<StrokeStyle> for StrokeStyleKey {
    fn from(value: StrokeStyle) -> Self {
        Self {
            sort_rank: value.sort_rank,
            casing: value.casing.map(StrokePassKey::from),
            center: StrokePassKey::from(value.center),
        }
    }
}

/// Physical sheet identity for solid-road boolean geometry. Paint style
/// alone is insufficient: a same-colored tunnel, surface street, and deck
/// may cross in XY while occupying different 2.5D planes. Mixing them into
/// one nearest-centerline DzField folds the resulting mesh between their
/// signed elevations when the camera tilts.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
enum RoadVerticalClass {
    Sunk,
    Surface,
    Elevated,
}

/// Sentinel for "no casing pass" in `RoadSurfaceKey::casing_width_bits`
/// (real width bit patterns never collide with it).
const NO_CASING_BITS: u32 = u32::MAX;

/// Expand-class band offset selecting the CLAMPED face correction in the
/// shader (faces only widen; strokes keep the exact curve).
pub const FACE_MORPH_CLASS_OFFSET: f32 = 4.0;

/// Cascade-input dump lever.
fn cascade_dump_armed() -> bool {
    crate::makepad_platform::makepad_error_log::trace_enabled("map.cascade")
}

/// Theme-stable identity of one road-surface union tier. NO resolved
/// colors: grouping, paint-order tiebreaks and the faces-bake signature
/// key on the styling rule's class id plus the geometric fields, so one
/// bake serves the light/dark/circuit themes (which by contract only
/// recolor). Field order is the derived Ord = the tier paint order.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub(crate) struct RoadSurfaceKey {
    sort_rank: i16,
    class_id: u32,
    center_width_bits: u32,
    center_depth_micro_bits: u32,
    casing_width_bits: u32,
    casing_depth_micro_bits: u32,
    vertical: RoadVerticalClass,
    /// Normalized OSM stack level. Untagged signed profiles use -1/+1.
    layer: i8,
}

impl RoadSurfaceKey {
    fn from_way(
        style: StrokeStyle,
        tags: &HashMap<String, String>,
        dz: Option<&[f32]>,
    ) -> Self {
        let bridge = tag_is_truthy(tags, "bridge");
        let tunnel = tag_is_truthy(tags, "tunnel");
        let raw_layer = tags
            .get("osm_layer")
            .and_then(|value| value.parse::<f32>().ok())
            .filter(|value| value.is_finite())
            .map(|value| value.round() as i32);
        // Mirror the importer/baker's normalization so renderer-side sheet
        // identity cannot disagree with the profile solver.
        let osm_layer = match raw_layer {
            Some(layer) if bridge && layer < 1 => 1,
            Some(layer) if tunnel && layer > -1 => -1,
            Some(layer) => layer,
            None if bridge => 1,
            None if tunnel => -1,
            None => 0,
        }
            .clamp(i8::MIN as i32, i8::MAX as i32) as i8;
        let min_dz = dz
            .into_iter()
            .flatten()
            .copied()
            .fold(0.0f32, f32::min);
        let max_dz = dz
            .into_iter()
            .flatten()
            .copied()
            .fold(0.0f32, f32::max);
        let (vertical, layer) = if tunnel || osm_layer < 0 {
            (RoadVerticalClass::Sunk, osm_layer.min(-1))
        } else if bridge || osm_layer > 0 {
            (RoadVerticalClass::Elevated, osm_layer.max(1))
        } else if min_dz <= -LIFT_COVER_M {
            (RoadVerticalClass::Sunk, -1)
        } else if max_dz >= LIFT_COVER_M {
            (RoadVerticalClass::Elevated, 1)
        } else {
            (RoadVerticalClass::Surface, 0)
        };
        Self {
            sort_rank: style.sort_rank,
            class_id: style.class_id,
            center_width_bits: style.center.width.to_bits(),
            center_depth_micro_bits: style.center.depth_micro.to_bits(),
            casing_width_bits: style
                .casing
                .map_or(NO_CASING_BITS, |pass| pass.width.to_bits()),
            casing_depth_micro_bits: style
                .casing
                .map_or(NO_CASING_BITS, |pass| pass.depth_micro.to_bits()),
            vertical,
            layer,
        }
    }

    /// Surface approaches may reconcile with their elevated continuation.
    /// A tunnel must never inherit a nearby surface/deck profile, and two
    /// explicitly different elevated stack levels are separate structures.
    fn grade_compatible(self, other: Self) -> bool {
        match (self.vertical, other.vertical) {
            (RoadVerticalClass::Sunk, RoadVerticalClass::Sunk) => self.layer == other.layer,
            (RoadVerticalClass::Sunk, _) | (_, RoadVerticalClass::Sunk) => false,
            (RoadVerticalClass::Elevated, RoadVerticalClass::Elevated) => {
                self.layer == other.layer
            }
            _ => true,
        }
    }

    fn depth_level(self) -> i8 {
        if self.vertical == RoadVerticalClass::Sunk {
            -1
        } else {
            0
        }
    }
}

/// Effective within-pass depth for road paint. Phase bands preserve the
/// global plaza -> all casings -> all centers painter order; the style's
/// exact depth micro preserves ordering inside a band. Solid sunk sheets
/// retain those separations in one parallel band below surface content.
fn road_semantic_param5(level: i8, phase: u8, depth_micro: f32) -> f32 {
    let depth_micro = depth_micro.clamp(0.0, ROAD_DEPTH_MAX_MICRO);
    let surface = match phase {
        0 => ROAD_SURFACE_PLAZA_DEPTH,
        1 => ROAD_SURFACE_CASING_DEPTH + depth_micro,
        _ => ROAD_UNION_CENTER_DEPTH + depth_micro,
    };
    if level < 0 {
        surface - ROAD_SUNK_DEPTH_OFFSET
    } else {
        surface
    }
}

fn road_surface_param5(key: RoadSurfaceKey, phase: u8) -> f32 {
    let depth_micro = if phase == 1 {
        if key.casing_depth_micro_bits == NO_CASING_BITS {
            0.0
        } else {
            f32::from_bits(key.casing_depth_micro_bits)
        }
    } else {
        f32::from_bits(key.center_depth_micro_bits)
    };
    road_semantic_param5(key.depth_level(), phase, depth_micro)
}

/// Whether a styled way is a physical solid road surface handled by the
/// road-paint boolean mesh. This deliberately ignores color and rank: those
/// select paint groups inside the shared pipeline, not a different renderer.
/// Patterned/dashed passes remain vector overlays.
fn is_solid_road_surface(
    tags: &HashMap<String, String>,
    style: &StrokeStyle,
) -> bool {
    let layer = tags.get("layer").map(String::as_str).unwrap_or("");
    tags.contains_key("highway")
        // Road-area polygons already enter the union as their complete
        // plaza contour. Treating their styled outline as a second physical
        // ribbon adds a redundant offset loop around every polygon edge and
        // can change the union silhouette. Keep that outline in the ordinary
        // vector-stroke path instead.
        && !is_road_polygon_layer(layer)
        && !tag_is_truthy(tags, "rail")
        && !tag_is_truthy(tags, "tunnel")
        && style.center.shape_id == 0.0
        && style
            .casing
            .is_none_or(|casing| casing.shape_id == 0.0)
}

type RoadTierEnd = (RoadSurfaceKey, usize, bool);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum RoadJoinFamily {
    #[default]
    Unknown,
    Motorway,
    Trunk,
    Primary,
    Secondary,
    Tertiary,
    Unclassified,
    Residential,
    Service,
    LivingStreet,
    Pedestrian,
    Footway,
    Cycleway,
    Path,
    Track,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct RoadJoinMeta {
    family: RoadJoinFamily,
    is_link: bool,
    is_bridge: bool,
}

impl RoadJoinMeta {
    fn from_tags(tags: &HashMap<String, String>) -> Self {
        let highway = tags.get("highway").map(String::as_str);
        let family = match highway {
            Some("motorway" | "motorway_link") => RoadJoinFamily::Motorway,
            Some("trunk" | "trunk_link") => RoadJoinFamily::Trunk,
            Some("primary" | "primary_link") => RoadJoinFamily::Primary,
            Some("secondary" | "secondary_link") => RoadJoinFamily::Secondary,
            Some("tertiary" | "tertiary_link") => RoadJoinFamily::Tertiary,
            Some("unclassified") => RoadJoinFamily::Unclassified,
            Some("residential") => RoadJoinFamily::Residential,
            Some("service") => RoadJoinFamily::Service,
            Some("living_street") => RoadJoinFamily::LivingStreet,
            Some("pedestrian") => RoadJoinFamily::Pedestrian,
            Some("footway" | "steps") => RoadJoinFamily::Footway,
            Some("cycleway") => RoadJoinFamily::Cycleway,
            Some("path") => RoadJoinFamily::Path,
            Some("track") => RoadJoinFamily::Track,
            _ => RoadJoinFamily::Unknown,
        };
        Self {
            family,
            is_link: tag_is_truthy(tags, "link")
                || highway.is_some_and(|kind| kind.ends_with("_link")),
            is_bridge: tag_is_truthy(tags, "bridge"),
        }
    }

    fn same_known_family(self, other: Self) -> bool {
        self.family != RoadJoinFamily::Unknown && self.family == other.family
    }
}

#[derive(Clone, Debug)]
struct RoadTierJoinWay {
    key: RoadSurfaceKey,
    way_index: usize,
    points: Vec<(f32, f32)>,
    dz: Vec<f32>,
    half_width: f32,
    meta: RoadJoinMeta,
}

#[derive(Clone, Copy, Debug)]
struct RoadTierGradeCorrection {
    end: RoadTierEnd,
    target_dz: f32,
}

/// Find lower-rank road ends which geometrically merge into the INTERIOR
/// of a wider/higher-rank through road. Vector-tile feature splitting does
/// not guarantee that such a merge shares an exact node: the link can end
/// against the middle of the mainline's segment. In that case independent
/// dz profiles can leave the link deck above or below the mainline and
/// expose its round fascia as a ledge.
///
/// This deliberately requires an acute, overlapping endpoint-to-through
/// relationship. Perpendicular crossings, target endpoints, and same-tier
/// carriageways are rejected. Large height differences are accepted only
/// for a typed link joining a non-link road of the same family. That typed
/// relationship is also the only one allowed to lower an endpoint: the
/// wider through road is authoritative in either direction.
/// Way-bbox cell grid for the endpoint join passes: an endpoint query
/// returns every way whose bbox expanded by its own half-width plus the
/// caller's worst-case source reach can contain the point. Candidate lists
/// stay in ascending way order (insertion order), so first-match iteration
/// semantics survive the prefilter exactly.
struct WayBboxGrid {
    cell: f32,
    cells: HashMap<(i32, i32), Vec<u32>>,
}

impl WayBboxGrid {
    fn build(
        way_bounds: &[(f32, f32, f32, f32, f32)],
        ways: &[RoadTierJoinWay],
        extra_reach: f32,
    ) -> WayBboxGrid {
        let cell = 16.0f32;
        let mut cells: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for (way_index, (&(bx0, by0, bx1, by1, _), way)) in
            way_bounds.iter().zip(ways).enumerate()
        {
            if bx0 > bx1 {
                continue;
            }
            let reach = way.half_width + extra_reach + 0.01;
            let clamp_cell = |v: f32| ((v / cell).floor() as i32).clamp(-64, 64);
            let x0 = clamp_cell(bx0 - reach);
            let x1 = clamp_cell(bx1 + reach);
            let y0 = clamp_cell(by0 - reach);
            let y1 = clamp_cell(by1 + reach);
            for cy in y0..=y1 {
                for cx in x0..=x1 {
                    cells.entry((cx, cy)).or_default().push(way_index as u32);
                }
            }
        }
        WayBboxGrid { cell, cells }
    }

    fn candidates(&self, point: (f32, f32)) -> &[u32] {
        let key = (
            ((point.0 / self.cell).floor() as i32).clamp(-64, 64),
            ((point.1 / self.cell).floor() as i32).clamp(-64, 64),
        );
        self.cells.get(&key).map_or(&[], |cell| cell.as_slice())
    }
}

fn endpoint_to_through_grade_corrections(
    ways: &[RoadTierJoinWay],
) -> Vec<RoadTierGradeCorrection> {
    const MIN_TANGENT_DOT: f32 = 0.72;
    const LARGE_JOIN_TANGENT_DOT: f32 = 0.90;
    const MAX_RAISE_M: f32 = 3.0;
    const MAX_TYPED_CORRECTION_M: f32 = 40.0;
    const CORRECTION_EPSILON_M: f32 = 0.001;

    // Per-way bbox + polyline length, computed once: every distance gate
    // below is bounded by source.half_width + target.half_width, so an
    // endpoint outside the target's expanded bbox can never contribute —
    // the pair scan was O(ways^2 x points) on mid-zoom city tiles.
    let way_bounds: Vec<(f32, f32, f32, f32, f32)> = ways
        .iter()
        .map(|way| {
            let mut min_x = f32::MAX;
            let mut min_y = f32::MAX;
            let mut max_x = f32::MIN;
            let mut max_y = f32::MIN;
            for &(x, y) in &way.points {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
            let total_len: f32 = way
                .points
                .windows(2)
                .map(|pair| {
                    let dx = pair[1].0 - pair[0].0;
                    let dy = pair[1].1 - pair[0].1;
                    (dx * dx + dy * dy).sqrt()
                })
                .sum();
            (min_x, min_y, max_x, max_y, total_len)
        })
        .collect();
    let max_source_hw = ways.iter().map(|way| way.half_width).fold(0.0f32, f32::max);
    let bbox_grid = WayBboxGrid::build(&way_bounds, ways, max_source_hw);
    let mut corrections = Vec::new();
    for source in ways {
        if source.points.len() < 2 || source.dz.len() != source.points.len() {
            continue;
        }
        for is_start in [true, false] {
            let (end_index, inner_index) = if is_start {
                (0, 1)
            } else {
                (source.points.len() - 1, source.points.len() - 2)
            };
            let point = source.points[end_index];
            // Buffered MVT copies can end outside their owning tile. Let
            // the tile that owns the real endpoint repair the merge; doing
            // it again on a padded copy can create a new cross-tile grade.
            if point.0 < 0.0
                || point.0 > TILE_SIZE as f32
                || point.1 < 0.0
                || point.1 > TILE_SIZE as f32
            {
                continue;
            }
            let inner = source.points[inner_index];
            let (out_x, out_y) = (point.0 - inner.0, point.1 - inner.1);
            let out_len = (out_x * out_x + out_y * out_y).sqrt();
            if out_len <= 1e-5 {
                continue;
            }
            let outward = (out_x / out_len, out_y / out_len);
            let source_dz = source.dz[end_index];
            let mut best: Option<(f32, f32)> = None;

            for &target_index in bbox_grid.candidates(point) {
                let target_index = target_index as usize;
                let target = &ways[target_index];
                let (bx0, by0, bx1, by1, total_len) = way_bounds[target_index];
                let reach = source.half_width + target.half_width;
                if point.0 < bx0 - reach
                    || point.0 > bx1 + reach
                    || point.1 < by0 - reach
                    || point.1 > by1 + reach
                {
                    continue;
                }
                if target.key == source.key
                    || !source.key.grade_compatible(target.key)
                    || target.points.len() < 2
                    || target.dz.len() != target.points.len()
                {
                    continue;
                }
                // A link inherits from its mainline, never the reverse.
                // Width handles styles whose painter ranks happen to tie.
                if target.key.sort_rank <= source.key.sort_rank
                    && target.half_width <= source.half_width * 1.1
                {
                    continue;
                }

                if total_len <= 1e-4 {
                    continue;
                }
                // The projection must have usable mainline on BOTH sides.
                // Scaling the margin by ribbon width rejects a target cap
                // while still accepting a projection onto an interior
                // polyline vertex.
                let through_margin = (target.half_width * 0.5)
                    .max(0.25)
                    .min(total_len * 0.25);
                let overlap_distance =
                    (source.half_width + target.half_width - 0.05).max(0.05);
                let overlap_sq = overlap_distance * overlap_distance;
                let mut along_before = 0.0;

                for segment_index in 0..target.points.len() - 1 {
                    let a = target.points[segment_index];
                    let b = target.points[segment_index + 1];
                    let (seg_x, seg_y) = (b.0 - a.0, b.1 - a.1);
                    let seg_sq = seg_x * seg_x + seg_y * seg_y;
                    if seg_sq <= 1e-8 {
                        continue;
                    }
                    let seg_len = seg_sq.sqrt();
                    let t = (((point.0 - a.0) * seg_x + (point.1 - a.1) * seg_y)
                        / seg_sq)
                        .clamp(0.0, 1.0);
                    let projection = (a.0 + seg_x * t, a.1 + seg_y * t);
                    let (off_x, off_y) = (point.0 - projection.0, point.1 - projection.1);
                    let distance_sq = off_x * off_x + off_y * off_y;
                    let along = along_before + seg_len * t;
                    along_before += seg_len;
                    if distance_sq >= overlap_sq
                        || along <= through_margin
                        || total_len - along <= through_margin
                    {
                        continue;
                    }
                    let tangent_dot =
                        ((outward.0 * seg_x + outward.1 * seg_y) / seg_len).abs();
                    if tangent_dot < MIN_TANGENT_DOT {
                        continue;
                    }
                    let target_dz = target.dz[segment_index]
                        + (target.dz[segment_index + 1] - target.dz[segment_index]) * t;
                    let delta = target_dz - source_dz;
                    let typed_link_join = source.meta.is_link
                        && !target.meta.is_link
                        && source.meta.same_known_family(target.meta);
                    if delta < -CORRECTION_EPSILON_M && !typed_link_join {
                        continue;
                    }
                    let max_delta = if typed_link_join {
                        MAX_TYPED_CORRECTION_M
                    } else {
                        MAX_RAISE_M
                    };
                    if delta.abs() > max_delta {
                        continue;
                    }
                    // An 8-40 m correction is safe only at the actual nose
                    // of a typed gore. A downward correction likewise needs
                    // this stronger authority: merely overlapping wide
                    // ribbons or running parallel nearby must not move
                    // another deck.
                    let large_join_gap = (source.half_width * 0.25).max(0.35);
                    if (delta < -CORRECTION_EPSILON_M || delta > MAX_RAISE_M)
                        && (tangent_dot < LARGE_JOIN_TANGENT_DOT
                            || target.key.sort_rank <= source.key.sort_rank
                            || target.half_width <= source.half_width * 1.1
                            || distance_sq > large_join_gap * large_join_gap)
                    {
                        continue;
                    }
                    // Prefer the deepest overlap and the most collinear
                    // through segment. A tiny height term makes an already
                    // matching continuation win over an unrelated upper
                    // parallel road when the geometry is otherwise tied.
                    let score = distance_sq / overlap_sq
                        + (1.0 - tangent_dot) * 0.5
                        + delta.abs() * 0.01;
                    if best.is_none_or(|(best_score, _)| score < best_score) {
                        best = Some((score, target_dz));
                    }
                }
            }
            if let Some((_, target_dz)) = best {
                corrections.push(RoadTierGradeCorrection {
                    end: (source.key, source.way_index, is_start),
                    target_dz,
                });
            }
        }
    }
    corrections
}

/// Find same-height road ends which land on a physical continuation in
/// another paint tier. Most joins land on the target's interior. A
/// generalized link can instead stop beside the target's cap; that is still
/// a safe butt joint when the target cap has its own exact, opposite
/// continuation. Exact shared-node topology also permits a typed link to
/// fork from a proven through road of another class. Generalized/nearby
/// matches remain deliberately one-way and same-family so parallel decks
/// cannot suppress each other's exposed ends.
///
fn endpoint_to_through_flush_ends(
    ways: &[RoadTierJoinWay],
) -> std::collections::HashSet<RoadTierEnd> {
    const MIN_TANGENT_DOT: f32 = 0.90;
    const MAX_DZ_GAP_M: f32 = 0.30;
    const MIN_CENTERLINE_GAP: f32 = 0.35;
    const MAX_NODE_DISTANCE: f32 = 0.20;
    const MAX_CONTINUATION_DOT: f32 = -0.90;

    #[derive(Clone, Copy)]
    struct Endpoint {
        point: (f32, f32),
        outward: (f32, f32),
        dz: f32,
    }

    let endpoints: Vec<[Option<Endpoint>; 2]> = ways
        .iter()
        .map(|way| {
            if way.points.len() < 2 || way.dz.len() != way.points.len() {
                return [None, None];
            }
            let endpoint = |is_start: bool| {
                let (end_index, inner_index) = if is_start {
                    (0, 1)
                } else {
                    (way.points.len() - 1, way.points.len() - 2)
                };
                let point = way.points[end_index];
                let inner = way.points[inner_index];
                let (out_x, out_y) = (point.0 - inner.0, point.1 - inner.1);
                let out_len = (out_x * out_x + out_y * out_y).sqrt();
                (out_len > 1e-5).then_some(Endpoint {
                    point,
                    outward: (out_x / out_len, out_y / out_len),
                    dz: way.dz[end_index],
                })
            };
            [endpoint(true), endpoint(false)]
        })
        .collect();

    // Endpoint hash grid (1-unit cells): continuation proofs join endpoints
    // within MAX_NODE_DISTANCE (0.2), so candidates always share the 3x3
    // neighborhood — the previous all-ways rescan per endpoint was the
    // larger quadratic half of this pass on mid-zoom city tiles.
    let mut endpoint_cells: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (way_index, slots) in endpoints.iter().enumerate() {
        for endpoint in slots.iter().flatten() {
            let cell = (endpoint.point.0.floor() as i32, endpoint.point.1.floor() as i32);
            let entry = endpoint_cells.entry(cell).or_default();
            if entry.last() != Some(&way_index) {
                entry.push(way_index);
            }
        }
    }
    // Precompute the exact through proof once. Looking it up inside every
    // source/target pair keeps this pass linear-ish rather than rescanning
    // all ways a third time for every candidate.
    let continuations: Vec<[Vec<usize>; 2]> = ways
        .iter()
        .enumerate()
        .map(|(target_index, target)| {
            std::array::from_fn(|target_end_slot| {
                let Some(target_end) = endpoints[target_index][target_end_slot] else {
                    return Vec::new();
                };
                let cell_x = target_end.point.0.floor() as i32;
                let cell_y = target_end.point.1.floor() as i32;
                let mut candidates: Vec<usize> = Vec::new();
                for cy in (cell_y - 1)..=(cell_y + 1) {
                    for cx in (cell_x - 1)..=(cell_x + 1) {
                        if let Some(cell) = endpoint_cells.get(&(cx, cy)) {
                            candidates.extend_from_slice(cell);
                        }
                    }
                }
                candidates.sort_unstable();
                candidates.dedup();
                candidates
                    .into_iter()
                    .filter(|&continuation_index| {
                        let continuation = &ways[continuation_index];
                        let min_width = target.half_width.min(continuation.half_width);
                        let max_width = target.half_width.max(continuation.half_width);
                        continuation_index != target_index
                            && target.meta.same_known_family(continuation.meta)
                            && target.key.grade_compatible(continuation.key)
                            && target.meta.is_link == continuation.meta.is_link
                            && min_width > 1e-5
                            && max_width / min_width <= 1.25
                            && endpoints[continuation_index]
                                .iter()
                                .flatten()
                                .any(|continuation_end| {
                                    let dx = continuation_end.point.0 - target_end.point.0;
                                    let dy = continuation_end.point.1 - target_end.point.1;
                                    dx * dx + dy * dy
                                        <= MAX_NODE_DISTANCE * MAX_NODE_DISTANCE
                                        && target_end.outward.0 * continuation_end.outward.0
                                            + target_end.outward.1
                                                * continuation_end.outward.1
                                            <= MAX_CONTINUATION_DOT
                                        && target_end.dz > 0.2
                                        && continuation_end.dz > 0.2
                                        && (target_end.dz - continuation_end.dz).abs()
                                            < MAX_DZ_GAP_M
                                })
                    })
                    .collect()
            })
        })
        .collect();

    // Same bbox prefilter as the grade pass: every proximity gate in the
    // target loop is bounded by half-width sums (plus the 0.35 centerline
    // floor), so distant targets can never classify an end as flush.
    let way_bounds: Vec<(f32, f32, f32, f32, f32)> = ways
        .iter()
        .map(|way| {
            let mut min_x = f32::MAX;
            let mut min_y = f32::MAX;
            let mut max_x = f32::MIN;
            let mut max_y = f32::MIN;
            for &(x, y) in &way.points {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
            let total_len: f32 = way
                .points
                .windows(2)
                .map(|pair| {
                    let dx = pair[1].0 - pair[0].0;
                    let dy = pair[1].1 - pair[0].1;
                    (dx * dx + dy * dy).sqrt()
                })
                .sum();
            (min_x, min_y, max_x, max_y, total_len)
        })
        .collect();
    let max_source_hw = ways.iter().map(|way| way.half_width).fold(0.0f32, f32::max);
    let bbox_grid =
        WayBboxGrid::build(&way_bounds, ways, max_source_hw + MIN_CENTERLINE_GAP);
    let mut flush = std::collections::HashSet::new();
    for (source_index, source) in ways.iter().enumerate() {
        if source.points.len() < 2 || source.dz.len() != source.points.len() {
            continue;
        }
        for is_start in [true, false] {
            let (end_index, inner_index) = if is_start {
                (0, 1)
            } else {
                (source.points.len() - 1, source.points.len() - 2)
            };
            let point = source.points[end_index];
            let clip_min = -ROAD_PAINT_CLIP_PADDING;
            let clip_max = TILE_SIZE as f32 + ROAD_PAINT_CLIP_PADDING;
            if point.0 < clip_min
                || point.0 > clip_max
                || point.1 < clip_min
                || point.1 > clip_max
                || source.dz[end_index] <= 0.2
            {
                continue;
            }
            let inner = source.points[inner_index];
            let (out_x, out_y) = (point.0 - inner.0, point.1 - inner.1);
            let out_len = (out_x * out_x + out_y * out_y).sqrt();
            if out_len <= 1e-5 {
                continue;
            }
            let outward = (out_x / out_len, out_y / out_len);

            'targets: for &target_index in bbox_grid.candidates(point) {
                let target_index = target_index as usize;
                let target = &ways[target_index];
                let (bx0, by0, bx1, by1, precomputed_len) = way_bounds[target_index];
                let reach = source.half_width + target.half_width + MIN_CENTERLINE_GAP;
                if point.0 < bx0 - reach
                    || point.0 > bx1 + reach
                    || point.1 < by0 - reach
                    || point.1 > by1 + reach
                {
                    continue;
                }
                if target.key == source.key
                    || !source.key.grade_compatible(target.key)
                    || target.points.len() < 2
                    || target.dz.len() != target.points.len()
                {
                    continue;
                }

                // An exact source node is stronger topology than class or
                // tangent similarity. A typed link may fork from a through
                // road of another class (for example motorway_link from a
                // trunk); Shortbread keeps the shared node, but styling puts
                // the two surfaces in different union tiers. Prove that the
                // target really continues on both sides of that node, and
                // require the final deck profiles to agree. This is exact
                // topology only: the conservative family/tangent gates below
                // still govern generalized or merely nearby geometry.
                let exact_link_fork = source.meta.is_link
                    && !target.meta.is_link
                    && source.meta.family != RoadJoinFamily::Unknown
                    && target.meta.family != RoadJoinFamily::Unknown
                    && target.points[1..target.points.len() - 1]
                        .iter()
                        .enumerate()
                        .any(|(relative_index, &target_point)| {
                            let vertex = relative_index + 1;
                            let dx = point.0 - target_point.0;
                            let dy = point.1 - target_point.1;
                            if dx * dx + dy * dy
                                > MAX_NODE_DISTANCE * MAX_NODE_DISTANCE
                                || target.dz[vertex] <= 0.2
                                || (source.dz[end_index] - target.dz[vertex]).abs()
                                    >= MAX_DZ_GAP_M
                            {
                                return false;
                            }
                            let previous = target.points[vertex - 1];
                            let next = target.points[vertex + 1];
                            let into_previous = (
                                previous.0 - target_point.0,
                                previous.1 - target_point.1,
                            );
                            let into_next = (
                                next.0 - target_point.0,
                                next.1 - target_point.1,
                            );
                            let previous_len = (into_previous.0 * into_previous.0
                                + into_previous.1 * into_previous.1)
                                .sqrt();
                            let next_len =
                                (into_next.0 * into_next.0 + into_next.1 * into_next.1)
                                    .sqrt();
                            previous_len > 1e-5
                                && next_len > 1e-5
                                && (into_previous.0 * into_next.0
                                    + into_previous.1 * into_next.1)
                                    / (previous_len * next_len)
                                    <= MAX_CONTINUATION_DOT
                        });
                if exact_link_fork {
                    flush.insert((source.key, source.way_index, is_start));
                    break 'targets;
                }

                if !source.meta.same_known_family(target.meta) {
                    continue;
                }

                // A link may terminate beside a mainline CAP rather than on
                // its centerline. Accept that generalized gore only when the
                // cap is independently proven to be a through node by a
                // third, exact opposite continuation at the same deck height.
                // Only the link end becomes flush; the through-road pair
                // already owns its ordinary exact-node joint.
                if source.meta.is_link && !target.meta.is_link {
                    let topology_reach =
                        (source.half_width + target.half_width - 0.05)
                        .max(0.05);
                    let topology_reach_sq = topology_reach * topology_reach;
                    for (target_end_slot, target_end) in
                        endpoints[target_index].iter().enumerate()
                    {
                        let Some(target_end) = target_end else {
                            continue;
                        };
                        let dx = point.0 - target_end.point.0;
                        let dy = point.1 - target_end.point.1;
                        let tangent_dot =
                            (outward.0 * target_end.outward.0
                                + outward.1 * target_end.outward.1)
                                .abs();
                        if dx * dx + dy * dy <= topology_reach_sq
                            && tangent_dot >= MIN_TANGENT_DOT
                            && target_end.dz > 0.2
                            && (source.dz[end_index] - target_end.dz).abs()
                                < MAX_DZ_GAP_M
                            && continuations[target_index][target_end_slot]
                                .iter()
                                .any(|&continuation_index| {
                                    continuation_index != source_index
                                })
                        {
                            flush.insert((source.key, source.way_index, is_start));
                            break 'targets;
                        }
                    }
                }

                let total_len = precomputed_len;
                if total_len <= 1e-4 {
                    continue;
                }
                let through_margin = (target.half_width * 0.5)
                    .max(0.25)
                    .min(total_len * 0.25);
                let centerline_gap =
                    MIN_CENTERLINE_GAP.max(source.half_width.min(target.half_width) * 0.25);
                let centerline_gap_sq = centerline_gap * centerline_gap;
                let mut along_before = 0.0;

                for segment_index in 0..target.points.len() - 1 {
                    let a = target.points[segment_index];
                    let b = target.points[segment_index + 1];
                    let (seg_x, seg_y) = (b.0 - a.0, b.1 - a.1);
                    let seg_sq = seg_x * seg_x + seg_y * seg_y;
                    if seg_sq <= 1e-8 {
                        continue;
                    }
                    let seg_len = seg_sq.sqrt();
                    let t = (((point.0 - a.0) * seg_x + (point.1 - a.1) * seg_y)
                        / seg_sq)
                        .clamp(0.0, 1.0);
                    let projection = (a.0 + seg_x * t, a.1 + seg_y * t);
                    let (off_x, off_y) = (point.0 - projection.0, point.1 - projection.1);
                    let distance_sq = off_x * off_x + off_y * off_y;
                    let along = along_before + seg_len * t;
                    along_before += seg_len;
                    if distance_sq > centerline_gap_sq
                        || along <= through_margin
                        || total_len - along <= through_margin
                    {
                        continue;
                    }
                    let tangent_dot =
                        ((outward.0 * seg_x + outward.1 * seg_y) / seg_len).abs();
                    if tangent_dot < MIN_TANGENT_DOT {
                        continue;
                    }
                    let target_dz = target.dz[segment_index]
                        + (target.dz[segment_index + 1] - target.dz[segment_index]) * t;
                    if target_dz <= 0.2
                        || (target_dz - source.dz[end_index]).abs() >= MAX_DZ_GAP_M
                    {
                        continue;
                    }
                    flush.insert((source.key, source.way_index, is_start));
                    break 'targets;
                }
            }
        }
    }
    flush
}

/// Repair an exact style split whose two centerlines are one collinear road
/// but whose baked endpoint heights disagree. Same-family bridge splits may
/// inherit a large deck correction in either direction. Other road-class
/// transitions are limited to a small correction and must already be lifted,
/// preventing a chance ground/deck coincidence from joining stacked roads.
fn endpoint_continuation_grade_corrections(
    ways: &[RoadTierJoinWay],
) -> Vec<RoadTierGradeCorrection> {
    const MAX_CROSS_STYLE_RAISE_M: f32 = 3.0;
    const MAX_BRIDGE_RAISE_M: f32 = 40.0;
    const MAX_NODE_DISTANCE: f32 = 0.20;
    const MAX_WIDTH_RATIO: f32 = 1.25;
    const MAX_DIRECTION_DOT: f32 = -0.90;
    const DZ_EPSILON_M: f32 = 0.05;

    #[derive(Clone, Copy)]
    struct Endpoint {
        end: RoadTierEnd,
        point: (f32, f32),
        outward: (f32, f32),
        dz: f32,
        half_width: f32,
        meta: RoadJoinMeta,
    }

    let mut nodes: HashMap<(i32, i32), Vec<Endpoint>> = HashMap::new();
    for way in ways {
        if way.points.len() < 2 || way.dz.len() != way.points.len() {
            continue;
        }
        for is_start in [true, false] {
            let (end_index, inner_index) = if is_start {
                (0, 1)
            } else {
                (way.points.len() - 1, way.points.len() - 2)
            };
            let point = way.points[end_index];
            if point.0 < 0.0
                || point.0 > TILE_SIZE as f32
                || point.1 < 0.0
                || point.1 > TILE_SIZE as f32
            {
                continue;
            }
            let inner = way.points[inner_index];
            let (dx, dy) = (point.0 - inner.0, point.1 - inner.1);
            let len = (dx * dx + dy * dy).sqrt();
            if len <= 1e-5 {
                continue;
            }
            nodes
                .entry(((point.0 * 4.0).round() as i32, (point.1 * 4.0).round() as i32))
                .or_default()
                .push(Endpoint {
                    end: (way.key, way.way_index, is_start),
                    point,
                    outward: (dx / len, dy / len),
                    dz: way.dz[end_index],
                    half_width: way.half_width,
                    meta: way.meta,
                });
        }
    }

    let mut by_end = HashMap::<RoadTierEnd, f32>::new();
    for entries in nodes.values() {
        for (index, a) in entries.iter().enumerate() {
            for b in entries.iter().skip(index + 1) {
                if a.end.0 == b.end.0
                    || !a.end.0.grade_compatible(b.end.0)
                    || a.meta.is_link != b.meta.is_link
                {
                    continue;
                }
                let (node_dx, node_dy) = (a.point.0 - b.point.0, a.point.1 - b.point.1);
                if node_dx * node_dx + node_dy * node_dy
                    > MAX_NODE_DISTANCE * MAX_NODE_DISTANCE
                {
                    continue;
                }
                let width_ratio = a.half_width.max(b.half_width)
                    / a.half_width.min(b.half_width).max(0.05);
                if width_ratio > MAX_WIDTH_RATIO
                    || a.outward.0 * b.outward.0 + a.outward.1 * b.outward.1
                        > MAX_DIRECTION_DOT
                {
                    continue;
                }
                let (lower, higher) = if a.dz <= b.dz { (a, b) } else { (b, a) };
                let raise = higher.dz - lower.dz;
                let bridge_continuation = a.meta.is_bridge != b.meta.is_bridge
                    && a.meta.same_known_family(b.meta);
                let max_raise = if bridge_continuation {
                    MAX_BRIDGE_RAISE_M
                } else {
                    MAX_CROSS_STYLE_RAISE_M
                };
                if raise <= DZ_EPSILON_M
                    || raise > max_raise
                    || (!bridge_continuation
                        && (a.meta.family == RoadJoinFamily::Unknown
                            || b.meta.family == RoadJoinFamily::Unknown
                            || a.dz <= 0.2
                            || b.dz <= 0.2))
                {
                    continue;
                }
                by_end
                    .entry(lower.end)
                    .and_modify(|target| *target = target.max(higher.dz))
                    .or_insert(higher.dz);
            }
        }
    }
    by_end
        .into_iter()
        .map(|(end, target_dz)| RoadTierGradeCorrection { end, target_dz })
        .collect()
}

/// Move one endpoint to a through-road deck and taper that correction
/// smoothly back into the source profile. Both ends of the blend have zero
/// derivative from the correction term, avoiding a new grade kink.
fn apply_endpoint_grade_correction(
    points: &[(f32, f32)],
    dz: &mut [f32],
    is_start: bool,
    target_dz: f32,
    half_width: f32,
) {
    if points.len() < 2 || dz.len() != points.len() {
        return;
    }
    let endpoint_index = if is_start { 0 } else { points.len() - 1 };
    let delta = target_dz - dz[endpoint_index];
    if delta.abs() <= 0.001 {
        return;
    }
    let total_len: f32 = points
        .windows(2)
        .map(|pair| {
            let dx = pair[1].0 - pair[0].0;
            let dy = pair[1].1 - pair[0].1;
            (dx * dx + dy * dy).sqrt()
        })
        .sum();
    if total_len <= 1e-5 {
        dz[endpoint_index] = target_dz;
        return;
    }
    // At z14 one tile unit is several metres. Keep ordinary 0.5-3 m join
    // corrections local while still bounding the longer taper needed by a
    // trusted typed bridge/link correction, without dragging either through
    // a whole ramp.
    let blend_len = (delta.abs() * 3.0 + half_width * 2.0)
        .clamp(3.0, 96.0)
        .min(total_len);
    let mut distances = vec![0.0f32; points.len()];
    if is_start {
        for index in 1..points.len() {
            let dx = points[index].0 - points[index - 1].0;
            let dy = points[index].1 - points[index - 1].1;
            distances[index] = distances[index - 1] + (dx * dx + dy * dy).sqrt();
        }
    } else {
        for index in (0..points.len() - 1).rev() {
            let dx = points[index + 1].0 - points[index].0;
            let dy = points[index + 1].1 - points[index].1;
            distances[index] = distances[index + 1] + (dx * dx + dy * dy).sqrt();
        }
    }
    for (value, distance) in dz.iter_mut().zip(distances) {
        if distance > blend_len {
            continue;
        }
        let t = (distance / blend_len).clamp(0.0, 1.0);
        let weight = 1.0 - t * t * (3.0 - 2.0 * t);
        let original = *value;
        // Existing interior samples may already be on the far side of the
        // mainline height. Never turn a corrected endpoint into a hump or
        // a trough.
        let corrected = original + delta * weight;
        *value = if delta > 0.0 {
            corrected.min(target_dz.max(original))
        } else {
            corrected.max(target_dz.min(original))
        };
    }
    dz[endpoint_index] = target_dz;
}

#[derive(Clone, Debug)]
struct PreparedWay {
    way_index: usize,
    points: Vec<(f32, f32)>,
}

#[derive(Debug)]
struct FillFeatureGroup {
    color: u32,
    alpha: f32,
    layer_rank: u8,
    is_building: bool,
    pattern: f32,
    /// shiny.md material id (param3): water/green fills get per-pixel
    /// effects behind uniform gates; 0 = legacy path.
    material: f32,
    /// Bake into the ICON buffer (pass 3, after road strokes): district
    /// tints must colorize the roads too, and fills draw before strokes.
    late: bool,
    /// 3D bridge deck height (m): road polygons and bridge-area slabs at
    /// close zoom lift with the stroke decks instead of lying flat under
    /// the crossing.
    deck_m: f32,
    /// Index into the tile's decoded baked-fill list when this feature's
    /// triangulation was pre-baked (payload v2-fills-1). Flat mode then
    /// skips runtime ring classification + tessellation for the body.
    baked: Option<usize>,
    /// Road-surface polygon: eligible for per-vertex corridor decks when a
    /// baked bridge-dz overlay covers the tile.
    deckable: bool,
    /// This feature's own baked outline profiles (base_dz join): fill
    /// vertices lift by projecting onto these only.
    profiles: Vec<BridgeCorridor>,
    rings: Vec<FillRing>,
}

/// Shortbread's `bridges` polygons describe the physical bridge structure,
/// not the cartographic road surface. In a flat map the road stroke already
/// communicates the crossing; painting the structure as an opaque ground
/// fill hides water and roads beneath it. Tilted mode keeps the polygon so
/// the bridge still has a visible physical footprint below its deck.
fn structural_bridge_area_visible(layer: &str, tilted_3d: bool) -> bool {
    layer != "bridges" || tilted_3d
}

#[derive(DeJson)]
struct OverpassResponse {
    elements: Vec<OverpassElement>,
}

#[derive(DeJson)]
struct OverpassElement {
    #[rename(type)]
    kind: String,
    id: i64,
    lat: Option<f64>,
    lon: Option<f64>,
    nodes: Option<Vec<i64>>,
    tags: Option<HashMap<String, String>>,
}

// --- Public API ---

pub fn retry_delay_frames(attempts: u8) -> u64 {
    let shift = attempts.saturating_sub(1).min(6) as u32;
    let delay = RETRY_BASE_FRAMES.saturating_mul(1_u64 << shift);
    delay.min(RETRY_MAX_FRAMES)
}

pub fn overpass_endpoint(attempts: u8) -> &'static str {
    let index = attempts as usize % OVERPASS_ENDPOINTS.len();
    OVERPASS_ENDPOINTS[index]
}

pub fn overpass_query(tile: TileKey) -> String {
    let (south, west, north, east) = tile_bounds_padded(tile, TILE_QUERY_PAD);
    let mut ways = String::new();

    ways.push_str(&format!(
        "way[\"highway\"]({south:.6},{west:.6},{north:.6},{east:.6});\
         way[\"waterway\"]({south:.6},{west:.6},{north:.6},{east:.6});\
         way[\"natural\"=\"water\"]({south:.6},{west:.6},{north:.6},{east:.6});"
    ));

    if tile.z >= 15 {
        ways.push_str(&format!(
            "way[\"building\"][\"building\"!=\"no\"]({south:.6},{west:.6},{north:.6},{east:.6});"
        ));
    }

    if tile.z >= 14 {
        ways.push_str(&format!(
            "way[\"landuse\"]({south:.6},{west:.6},{north:.6},{east:.6});\
             way[\"leisure\"]({south:.6},{west:.6},{north:.6},{east:.6});"
        ));
    }

    format!(
        "[out:json][timeout:20];\
         ({ways});\
         (._;>;);\
         out body;"
    )
}

pub fn ensure_cache_dir() {
    let _ = fs::create_dir_all(TILE_CACHE_DIR);
}

pub fn tile_data_cache_path_for(tile_key: TileKey) -> PathBuf {
    Path::new(TILE_CACHE_DIR).join(format!(
        "z{}_x{}_y{}.json",
        tile_key.z, tile_key.x, tile_key.y
    ))
}

pub fn store_tile_data_cache_on_disk(tile_key: TileKey, body: &str) {
    ensure_cache_dir();
    let path = tile_data_cache_path_for(tile_key);
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, body).is_err() {
        let _ = fs::remove_file(&tmp);
        return;
    }
    let _ = fs::rename(&tmp, &path);
}

pub fn format_tile_key_sample(keys: &[TileKey], limit: usize) -> String {
    if keys.is_empty() {
        return "[]".to_string();
    }
    let mut out = String::from("[");
    for (index, key) in keys.iter().take(limit).enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(&format!("z{}x{}y{}", key.z, key.x, key.y));
    }
    if keys.len() > limit {
        out.push_str(", ...");
    }
    out.push(']');
    out
}

// --- Tile buffer building ---

/// Network/Overpass path: parse the JSON body, project lon/lat to tile-local
/// coordinates, then hand off to the shared feature builder.
pub fn build_tile_buffers_from_body(
    tile_key: TileKey,
    body: &str,
    theme: &CompiledMapTheme,
    render_zoom: u32,
) -> Result<TileBuffers, String> {
    let parsed = OverpassResponse::deserialize_json_lenient(body)
        .map_err(|e| format!("json error at line {} col {}: {}", e.line, e.col, e.msg))?;

    let tile_origin = dvec2(
        tile_key.x as f64 * TILE_SIZE,
        tile_key.y as f64 * TILE_SIZE,
    );
    let render_scale = 2.0_f64
        .powi(render_zoom as i32 - tile_key.z as i32)
        .max(1e-3) as f32;

    let mut nodes = HashMap::<i64, (f64, f64)>::new();
    let mut ways = Vec::<WayData>::new();
    let mut tagged_points = Vec::<((f32, f32), HashMap<String, String>)>::new();

    for element in parsed.elements {
        match element.kind.as_str() {
            "node" => {
                if let (Some(lat), Some(lon)) = (element.lat, element.lon) {
                    nodes.insert(element.id, (lon, lat));
                    if let Some(tags) = element.tags {
                        let world = lon_lat_to_world(lon, lat, tile_key.z) - tile_origin;
                        tagged_points.push(((world.x as f32, world.y as f32), tags));
                    }
                }
            }
            "way" => {
                if let Some(node_ids) = element.nodes {
                    let closed =
                        node_ids.len() > 2 && node_ids.first().copied() == node_ids.last().copied();
                    ways.push(WayData {
                        nodes: node_ids,
                        tags: element.tags.unwrap_or_default(),
                        closed,
                    });
                }
            }
            _ => {}
        }
    }

    let mut tile_ways = Vec::<TileWay>::with_capacity(ways.len());
    for way in ways {
        let projected =
            project_way_points_with_nodes(&way.nodes, &nodes, tile_key, tile_origin, render_scale);
        if projected.len() < 2 {
            continue;
        }
        let points = projected.into_iter().map(|(_, point)| point).collect();
        tile_ways.push(TileWay {
            points,
            tags: way.tags,
            closed: way.closed,
            dz: None,
            fidx: None,
        });
    }

    Ok(build_tile_buffers_from_features(
        tile_key,
        tile_ways,
        tagged_points,
        theme,
        render_zoom,
        false,
        true,
        Vec::new(),
        false,
        false,
        Vec::new(),
        None,
    ))
}

/// Local mbtiles path: decode the MVT protobuf STRAIGHT into tile-local
/// coordinates — no lon/lat round trip, no generated-JSON detour.
/// Render buckets from which 2.5D buildings are baked.
pub const BUILDING_3D_MIN_ZOOM: u32 = 15;

/// Icon INCLUSION horizon: from the high keyframe bucket (16) tiles carry
/// every icon through z18 with its real zoom floor encoded per icon; the
/// shader's live `icon_zoom` uniform reveals them per frame. Inclusion
/// stays bucket-gated below the keyframe so mid-zoom buffers stay lean.
fn icon_inclusion_zoom(render_zoom: u32) -> f32 {
    if render_zoom >= 16 { 18.0 } else { render_zoom as f32 }
}

pub fn build_tile_buffers_from_mvt(
    tile_key: TileKey,
    raw_tile_data: &[u8],
    detail_tile_data: Option<&[u8]>,
    bridge_dz_tile_data: Option<&[u8]>,
    bridge_dz_covered: bool,
    overlay_tiles: &[OverlayTileData],
    theme: &CompiledMapTheme,
    render_zoom: u32,
    buildings_3d: bool,
    build_road_core: bool,
) -> Result<TileBuffers, String> {
    let have_charger_overlay = overlay_tiles.iter().any(|overlay| overlay.has_chargers);
    let mut profiler = TileProfiler::new();
    let pbf_data = decode_vector_tile_payload(raw_tile_data)?;
    profiler.lap("payload-decode", "");
    // Baked fill triangulations (payload v2-fills-1, field 100): flat mode
    // substitutes them for the runtime tessellation of the big polygon
    // features. 3D/terrain ignores the stream — drape and extrusion re-grid
    // from the rings. MAKEPAD_NO_BAKED_FILLS=1 is the kill switch (also the
    // A/B lever for benchmarks).
    #[cfg(not(target_arch = "wasm32"))]
    static NO_BAKED_FILLS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    #[cfg(not(target_arch = "wasm32"))]
    let no_baked =
        *NO_BAKED_FILLS.get_or_init(|| std::env::var("MAKEPAD_NO_BAKED_FILLS").is_ok());
    #[cfg(target_arch = "wasm32")]
    let no_baked = false;
    let baked_fills: Vec<BakedFillFeature> = if buildings_3d || no_baked {
        Vec::new()
    } else {
        parse_baked_fills(&pbf_data).unwrap_or_default()
    };
    // Baked painter-cascade faces (v2-faces-1, field 101): z14 tiles carry
    // buckets 14/16, mid-zoom tiles their native bucket (the runtime
    // cascade at z11-13 city tiles measured 117-340ms — worse than z14).
    // A bucket missing from the stream or any signature mismatch falls
    // back to the runtime cascade. MAKEPAD_NO_BAKED_FACES=1 is the kill
    // switch / A/B lever.
    #[cfg(not(target_arch = "wasm32"))]
    static NO_BAKED_FACES: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    #[cfg(not(target_arch = "wasm32"))]
    let no_baked_faces =
        *NO_BAKED_FACES.get_or_init(|| std::env::var("MAKEPAD_NO_BAKED_FACES").is_ok());
    #[cfg(target_arch = "wasm32")]
    let no_baked_faces = false;
    let baked_faces = if !no_baked_faces
        && (10..=18).contains(&render_zoom)
        && !faces_bake_sink_armed()
    {
        parse_baked_faces(&pbf_data, render_zoom)
    } else {
        None
    };
    profiler.lap("baked-parse", "");
    let render_scale = 2.0_f64
        .powi(render_zoom as i32 - tile_key.z as i32)
        .max(1e-3) as f32;
    let mut collector = MvtLocalCollector::new(render_scale);
    collector.layer_filter = LayerParseFilter::BaseNoDetailLayers;
    // Baked base_dz overlay: dense solved height profiles keyed to this
    // tile's exact base features. The profile geometry replaces sparse base
    // edges during collection after an endpoint identity check.
    if let Some(dz_data) = bridge_dz_tile_data {
        match parse_base_dz_map(dz_data, tile_key) {
            Ok(map) => collector.base_dz = map,
            Err(err) => log!(
                "MapView: base dz tile z{} x{} y{} decode failed: {}",
                tile_key.z,
                tile_key.x,
                tile_key.y,
                err
            ),
        }
    }
    profiler.lap("dz-parse", "");
    parse_mvt_tile(&pbf_data, tile_key, &mut collector)?;
    profiler.lap("mvt-parse", "");
    // Compose micro-POIs (trees, benches, bins…) and, in 2.5D mode, building
    // footprints with real heights from the all-tag detail archive over the
    // shortbread base — skip the extra decode below the zooms that use them.
    let want_buildings = buildings_3d && render_zoom >= BUILDING_3D_MIN_ZOOM;
    let mut bridge_corridors = Vec::<BridgeCorridor>::new();
    // Bridge corridors want the detail archive from bucket 14 in 3D.
    let want_detail_points = !faces_bake_sink_armed()
        && icon_inclusion_zoom(render_zoom) >= ICON_MIN_ZOOM as f32;
    let want_detail_platforms = render_zoom >= 16;
    // Road elevation is camera-independent. Outside solved bridge-dz
    // coverage collect the heuristic corridors in flat mode too, making
    // the resulting road core reusable when the camera tilts.
    let collect_detail_corridors =
        build_road_core && render_zoom >= 14 && !bridge_dz_covered;
    if want_detail_points
        || want_detail_platforms
        || want_buildings
        || collect_detail_corridors
    {
        if let Some(detail_data) = detail_tile_data {
            if let Err(err) = merge_detail_features(
                detail_data,
                tile_key,
                render_scale,
                want_detail_points,
                want_buildings,
                collect_detail_corridors,
                &mut collector.points,
                &mut collector.ways,
                &mut bridge_corridors,
            ) {
                log!(
                    "MapView: detail tile z{} x{} y{} decode failed: {}",
                    tile_key.z,
                    tile_key.x,
                    tile_key.y,
                    err
                );
            }
        }
    }

    profiler.lap("detail-merge", "");
    for overlay in overlay_tiles {
        if let Err(err) = merge_overlay_features(
            overlay,
            tile_key,
            render_scale,
            &mut collector.points,
            &mut collector.ways,
        ) {
            log!(
                "MapView: overlay tile z{} x{} y{} decode failed: {}",
                tile_key.z,
                tile_key.x,
                tile_key.y,
                err
            );
        }
    }
    profiler.lap("overlay-merge", "");
    if faces_bake_sink_armed() {
        // Field 101 contains road regions and dissolved building groups,
        // never labels, POIs, trees, signals, or their contact shadows.
        collector.points.clear();
    }
    Ok(build_tile_buffers_from_features_profiled(
        profiler,
        tile_key,
        collector.ways,
        collector.points,
        theme,
        render_zoom,
        buildings_3d,
        build_road_core,
        bridge_corridors,
        bridge_dz_covered,
        have_charger_overlay,
        baked_fills,
        baked_faces,
    ))
}

/// Verbatim-point sink for the bridge-bake overlay: the dz array is
/// per-vertex, so the min-distance simplification of MvtLocalCollector
/// would break the alignment.
struct BridgeDzCollector {
    next_feature_id: u64,
    ways: Vec<TileWay>,
}

impl MvtSink for BridgeDzCollector {
    fn alloc_feature_id(&mut self) -> u64 {
        let id = self.next_feature_id;
        self.next_feature_id = self.next_feature_id.wrapping_add(1).max(1);
        id
    }

    fn add_path(
        &mut self,
        _tile_key: TileKey,
        extent: u32,
        points: &[(i32, i32)],
        tags: HashMap<String, String>,
        close: bool,
    ) {
        if points.len() < 2 {
            return;
        }
        let scale = TILE_SIZE as f32 / extent.max(1) as f32;
        self.ways.push(TileWay {
            points: points
                .iter()
                .map(|&(x, y)| (x as f32 * scale, y as f32 * scale))
                .collect(),
            tags,
            closed: close,
            dz: None,
            fidx: None,
        });
    }

    fn add_point(
        &mut self,
        _tile_key: TileKey,
        _extent: u32,
        _point: (i32, i32),
        _tags: HashMap<String, String>,
    ) {
    }
}

#[derive(Clone, Debug)]
struct BaseDzProfile {
    points: Vec<(f32, f32)>,
    decks: Vec<f32>,
}

fn base_dz_profile_from_way(
    way: TileWay,
) -> Option<((String, u32, u32), BaseDzProfile)> {
    if way.tags.get("layer").map(|v| v.as_str()) != Some("base_dz") {
        return None;
    }
    let (Some(layer), Some(fidx), Some(pidx), Some(dz)) = (
        way.tags.get("L"),
        way.tags.get("F"),
        way.tags.get("P"),
        way.tags.get("dz"),
    ) else {
        return None;
    };
    let (Ok(fidx), Ok(pidx)) = (fidx.parse::<u32>(), pidx.parse::<u32>()) else {
        return None;
    };
    let Ok(decks): Result<Vec<f32>, _> = dz
        .split(',')
        .map(|value| value.parse::<f32>().map(|dm| dm * 0.1))
        .collect()
    else {
        return None;
    };
    if decks.iter().any(|value| !value.is_finite()) {
        return None;
    }
    if way.points.len() < 2 || decks.len() != way.points.len() {
        return None;
    }
    Some((
        (layer.clone(), fidx, pidx),
        BaseDzProfile { points: way.points, decks },
    ))
}

/// Decode the base_dz layer of a bake overlay tile into the join map:
/// (source layer, feature index, path index) -> dense geometry + deck meters.
fn parse_base_dz_map(
    dz_tile_data: &[u8],
    tile_key: TileKey,
) -> Result<HashMap<(String, u32, u32), BaseDzProfile>, String> {
    let pbf_data = decode_vector_tile_payload(dz_tile_data)?;
    let mut collector = BridgeDzCollector { next_feature_id: 1, ways: Vec::new() };
    parse_mvt_tile(&pbf_data, tile_key, &mut collector)?;
    let mut map = HashMap::new();
    for way in collector.ways {
        if let Some((key, profile)) = base_dz_profile_from_way(way) {
            map.insert(key, profile);
        }
    }
    Ok(map)
}

fn base_dz_profile_projected_points(
    profile: &BaseDzProfile,
    raw_points: &[(i32, i32)],
    raw_scale: f32,
    close: bool,
) -> Option<Vec<(f32, f32)>> {
    if profile.points.len() < 2
        || profile.points.len() != profile.decks.len()
        || raw_points.len() < 2
    {
        return None;
    }
    const POINT_EPSILON: f32 = 0.05;
    let near = |a: (f32, f32), b: (f32, f32)| {
        let dx = a.0 - b.0;
        let dy = a.1 - b.1;
        dx * dx + dy * dy <= POINT_EPSILON * POINT_EPSILON
    };
    let mut raw_scaled: Vec<(f32, f32)> = raw_points
        .iter()
        .map(|&(x, y)| (x as f32 * raw_scale, y as f32 * raw_scale))
        .collect();
    if close && !near(raw_scaled[0], raw_scaled[raw_scaled.len() - 1]) {
        raw_scaled.push(raw_scaled[0]);
    }
    let raw_first = raw_scaled[0];
    let raw_last = raw_scaled[raw_scaled.len() - 1];
    if !near(profile.points[0], raw_first)
        || !near(profile.points[profile.points.len() - 1], raw_last)
        || (close && !near(profile.points[0], profile.points[profile.points.len() - 1]))
    {
        return None;
    }

    // Dense profiles preserve every raw vertex in order. Checking the full
    // subsequence (rather than endpoints alone) makes a stale overlay fail
    // closed when the source path acquires or moves an interior bend.
    let mut profile_index = 0usize;
    let mut previous_raw: Option<(f32, f32)> = None;
    let mut anchors = Vec::with_capacity(raw_scaled.len());
    for &raw in &raw_scaled {
        if previous_raw.is_some_and(|previous| near(previous, raw))
            && profile_index > 0
            && near(profile.points[profile_index - 1], raw)
        {
            anchors.push(profile_index - 1);
            previous_raw = Some(raw);
            continue;
        }
        while profile_index < profile.points.len()
            && !near(profile.points[profile_index], raw)
        {
            profile_index += 1;
        }
        if profile_index == profile.points.len() {
            return None;
        }
        anchors.push(profile_index);
        profile_index += 1;
        previous_raw = Some(raw);
    }

    // Overlay MVT quantization must not redefine the base road's XY shape:
    // at high overzoom, a half-extent-unit error would become a many-pixel
    // zig-zag. Snap every dense knot onto its containing validated raw
    // segment while retaining the profile's deck-value cardinality.
    let mut projected = profile.points.clone();
    for raw_segment in 0..raw_scaled.len() - 1 {
        let start = anchors[raw_segment];
        let end = anchors[raw_segment + 1];
        if start > end {
            return None;
        }
        let a = raw_scaled[raw_segment];
        let b = raw_scaled[raw_segment + 1];
        let direction = (b.0 - a.0, b.1 - a.1);
        let length_squared = direction.0 * direction.0 + direction.1 * direction.1;
        for index in start..=end {
            projected[index] = if length_squared <= 1e-9 {
                a
            } else {
                let source = profile.points[index];
                let t = (((source.0 - a.0) * direction.0
                    + (source.1 - a.1) * direction.1)
                    / length_squared)
                    .clamp(0.0, 1.0);
                (a.0 + direction.0 * t, a.1 + direction.1 * t)
            };
        }
    }
    Some(projected)
}

/// Decode a bridge-bake overlay tile into per-point-deck corridors. Tags:
/// dz = comma-joined decimeters per vertex, hw = corridor half-width meters.
/// Runtime consumer not wired yet; exercised by the probe_bridge_dz_load test.
#[cfg_attr(not(test), allow(dead_code))]
fn parse_bridge_dz_corridors(
    dz_tile_data: &[u8],
    tile_key: TileKey,
) -> Result<Vec<BridgeCorridor>, String> {
    let pbf_data = decode_vector_tile_payload(dz_tile_data)?;
    let mut collector = BridgeDzCollector { next_feature_id: 1, ways: Vec::new() };
    parse_mvt_tile(&pbf_data, tile_key, &mut collector)?;
    // Tile-local units per meter at this tile's latitude.
    let tile_span_m = {
        let n = (1u64 << tile_key.z.min(30)) as f64;
        let merc_y = 1.0 - 2.0 * (tile_key.y as f64 + 0.5) / n;
        let lat = (std::f64::consts::PI * merc_y).sinh().atan();
        40_075_016.686 * lat.cos() / n
    };
    let units_per_m = (TILE_SIZE / tile_span_m.max(1.0)) as f32;
    let mut corridors = Vec::new();
    for way in collector.ways {
        if way.tags.get("layer").map(|v| v.as_str()) != Some("bridge_dz") {
            continue;
        }
        let Some(dz_tag) = way.tags.get("dz") else {
            continue;
        };
        let decks: Vec<f32> = dz_tag
            .split(',')
            .filter_map(|v| v.parse::<f32>().ok())
            .map(|dm| (dm * 0.1).max(0.0))
            .collect();
        if decks.len() != way.points.len() {
            log!(
                "MapView: bridge dz way point/deck mismatch ({} vs {})",
                way.points.len(),
                decks.len()
            );
            continue;
        }
        let half_width_m = way
            .tags
            .get("hw")
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(5.0);
        corridors.push(BridgeCorridor {
            points: way.points,
            decks,
            half_width: (half_width_m * units_per_m).max(2.0),
            solved: true,
        });
    }
    Ok(corridors)
}

/// Merge features from a geodata overlay tile (layers.md track: chargers,
/// transit, nature, districts…). The MVT layer name arrives as the "layer"
/// tag and drives styling. Ancestor tiles (overlay maxzoom below the
/// requested zoom) are scaled into this tile's local space and rely on the
/// existing fill/stroke clipping; points get a bounds filter here.
fn merge_overlay_features(
    overlay: &OverlayTileData,
    tile_key: TileKey,
    render_scale: f32,
    points: &mut Vec<((f32, f32), HashMap<String, String>)>,
    ways: &mut Vec<TileWay>,
) -> Result<(), String> {
    let pbf_data = decode_vector_tile_payload(&overlay.raw)?;
    let mut collector = MvtLocalCollector::new(render_scale);
    parse_mvt_tile(&pbf_data, tile_key, &mut collector)?;
    let scale = (1u32 << overlay.shift) as f32;
    let offset_x = overlay.quadrant_x as f32 * TILE_SIZE as f32;
    let offset_y = overlay.quadrant_y as f32 * TILE_SIZE as f32;
    let transform = |p: (f32, f32)| (p.0 * scale - offset_x, p.1 * scale - offset_y);
    for (point, tags) in collector.points {
        let point = transform(point);
        if point.0 < -32.0
            || point.1 < -32.0
            || point.0 > TILE_SIZE as f32 + 32.0
            || point.1 > TILE_SIZE as f32 + 32.0
        {
            continue;
        }
        if overlay.filter != 0
            && tags.get("layer").map(|v| v.as_str()) == Some("chargers")
        {
            let kw = tags
                .get("max_kw")
                .and_then(|value| value.parse::<f64>().ok())
                .unwrap_or(0.0);
            let is_fast = kw >= 50.0;
            if (overlay.filter == 1) != is_fast {
                continue;
            }
        }
        points.push((point, tags));
    }
    for mut way in collector.ways {
        for point in way.points.iter_mut() {
            *point = transform(*point);
        }
        ways.push(way);
    }
    Ok(())
}

/// Merge whitelisted features from a detail-archive tile: micro-POI points
/// retagged into the synthetic `micro_pois` layer (icons only — the label
/// extractor ignores that layer, so base-poi labels are never duplicated),
/// and in 2.5D mode building polygons retagged `detail_buildings`.
#[allow(clippy::too_many_arguments)]
fn merge_detail_features(
    detail_data: &[u8],
    tile_key: TileKey,
    render_scale: f32,
    want_points: bool,
    want_buildings: bool,
    collect_corridors: bool,
    points: &mut Vec<((f32, f32), HashMap<String, String>)>,
    ways: &mut Vec<TileWay>,
    corridors: &mut Vec<BridgeCorridor>,
) -> Result<(), String> {
    let census_start = ways.len();
    let pbf_data = decode_vector_tile_payload(detail_data)?;
    let mut collector = MvtLocalCollector::new(render_scale);
    let render_zoom = tile_key.z as f32 + render_scale.max(1e-6).log2();
    // Keyframe icon horizon (see icon_inclusion_zoom): from bucket 16 the
    // buffers carry all street icons; the shader reveals by live zoom.
    let icon_horizon = if render_zoom >= 16.0 { 18.0 } else { render_zoom };
    // Combined archives carry base AND detail layers in one tile; this
    // pass only consumes the raw osm_* layers, and of those only the
    // geometry classes the flags below reach:
    // - points: micro-POI icons (want_points only)
    // - lines: heuristic bridge corridors, barrier/pedestrian/attraction
    //   rings (platform zooms)
    // - polygons: buildings, platforms, and polygon-anchored POI icons
    let want_platform_zoom = render_zoom >= 15.5;
    collector.layer_filter = LayerParseFilter::DetailLayers {
        points: want_points,
        lines: collect_corridors || want_platform_zoom,
        polygons: want_buildings || want_platform_zoom || want_points,
    };
    parse_mvt_tile(&pbf_data, tile_key, &mut collector)?;
    for way in &collector.ways {
        if !collect_corridors {
            break;
        }
        if way.closed || way.points.len() < 2 {
            continue;
        }
        let tags = &way.tags;
        if tags.get("layer").map(|v| v.as_str()) != Some("osm_lines") {
            continue;
        }
        let bridge = tags.get("bridge").map(|v| v.as_str()).unwrap_or("");
        if !(bridge == "yes" || bridge == "viaduct") {
            continue;
        }
        if !(tags.contains_key("highway") || tags.contains_key("railway")) {
            continue;
        }
        // Real OSM layer survives as osm_layer (plain `layer` is shadowed
        // by the MVT layer name). No layer = low crossing (canal bridge).
        let osm_layer = tags
            .get("osm_layer")
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(0.0);
        let deck_m = if osm_layer >= 1.0 {
            5.5 * osm_layer.min(3.0)
        } else {
            2.5
        };
        // Corridor width from the way's own width tag when present. Tiles
        // are TILE_SIZE (256) units across, whatever the source extent.
        let tile_span_m = {
            let n = (1u64 << tile_key.z.min(30)) as f64;
            let merc_y = 1.0 - 2.0 * (tile_key.y as f64 + 0.5) / n;
            let lat = (std::f64::consts::PI * merc_y).sinh().atan();
            40_075_016.686 * lat.cos() / n
        };
        let units_per_m = (TILE_SIZE / tile_span_m.max(1.0)) as f32;
        let half_width = tags
            .get("width")
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| v.is_finite())
            .map(|w| (w * 0.75).clamp(4.0, 14.0))
            .unwrap_or(7.0)
            * units_per_m;
        corridors.push(BridgeCorridor {
            decks: vec![deck_m; way.points.len()],
            points: way.points.clone(),
            half_width: half_width.max(0.5),
            solved: false,
        });
    }
    if want_points {
        for (point, mut tags) in collector.points {
            if tags.get("layer").map(|value| value.as_str()) != Some("osm_points") {
                continue;
            }
            // Attraction nodes (zoo animals) carry a label with no icon.
            let is_attraction_node = tags.contains_key("name")
                && (tags.contains_key("attraction") || tags.contains_key("zoo"));
            match micro_icon_for_tags(&tags) {
                Some((icon, _)) => {
                    if icon_horizon < micro_icon_min_zoom(icon) {
                        continue;
                    }
                }
                None => {
                    if !is_attraction_node || render_zoom < 15.5 {
                        continue;
                    }
                }
            }
            tags.insert("layer".to_string(), "micro_pois".to_string());
            points.push((point, tags));
        }
    }
    // Station/stop platforms render as gray polygons from z15.5 in both
    // 2D and 3D modes; buildings only when the 3D pass wants them.
    let want_platforms = render_zoom >= 15.5;
    if want_buildings || want_platforms || want_points {
        for mut way in collector.ways {
            // Polygon-anchored POIs (parking lots and garages, shops and
            // offices mapped on their building) icon at the centroid like
            // carto; the icon-collision pass dedups against any base node.
            if want_points && way.closed {
                // Underground garages span whole blocks; carto shows their
                // entrance node, not a centroid P in the middle of nowhere.
                let underground =
                    way.tags.get("parking").map(|v| v.as_str()) == Some("underground");
                if let Some((icon, _)) = micro_icon_for_tags(&way.tags).filter(|_| !underground) {
                    if icon_horizon >= micro_icon_min_zoom(icon) && way.points.len() >= 3 {
                        let mut tags = way.tags.clone();
                        tags.insert("layer".to_string(), "micro_pois".to_string());
                        points.push((ring_centroid(&way.points), tags));
                    }
                }
            }
            // Plain building ways AND assembled multipolygon relations
            // (palaces, courtyarded blocks) both carry building geometry.
            let from_polygons = matches!(
                way.tags.get("layer").map(|value| value.as_str()),
                Some("osm_polygons") | Some("osm_relation_polygons")
            );
            // Pedestrian squares mapped as highway=pedestrian + area=yes
            // stay in osm_lines (highway ways don't classify as polygons
            // at conversion). area=yes MEANS polygon, so close the ring
            // unconditionally — tile clipping can leave it open.
            if !from_polygons {
                // Walls, fences and hedges draw as thin barrier lines
                // (the dark perimeter around Artis is its wall).
                if let Some(barrier) = way.tags.get("barrier") {
                    if want_platforms
                        && matches!(
                            barrier.as_str(),
                            "wall" | "fence" | "retaining_wall" | "city_wall" | "hedge"
                        )
                    {
                        way.tags
                            .insert("layer".to_string(), "barrier_line".to_string());
                        ways.push(way);
                    }
                    continue;
                }
                let is_ped_area = tag_is_truthy(&way.tags, "area")
                    && matches!(
                        way.tags.get("highway").map(|v| v.as_str()),
                        Some("pedestrian" | "footway")
                    );
                // Attractions are areas by convention; clipping may have
                // opened the ring, so no first==last requirement.
                let is_attraction_ring = way.tags.contains_key("name")
                    && (way.tags.contains_key("attraction")
                        || way.tags.contains_key("zoo")
                        || way.tags.get("tourism").map(|v| v.as_str()) == Some("attraction"));
                let target_layer = if is_ped_area {
                    Some("street_polygons")
                } else if is_attraction_ring {
                    Some("attraction_area")
                } else {
                    None
                };
                if let Some(layer) = target_layer {
                    if want_platforms && way.points.len() >= 3 {
                        if way.points.first() != way.points.last() {
                            let first = way.points[0];
                            way.points.push(first);
                        }
                        way.closed = true;
                        way.tags.insert("layer".to_string(), layer.to_string());
                        ways.push(way);
                    }
                }
                continue;
            }
            // osm_lines rings arrive as LineStrings, so `closed` is only
            // set for real Polygon geometry — detect implicit closure.
            let ring_closed = way.closed
                || (way.points.len() >= 4 && way.points.first() == way.points.last());
            if !ring_closed {
                continue;
            }
            let is_platform = way.tags.get("railway").map(|v| v.as_str()) == Some("platform")
                || way.tags.get("public_transport").map(|v| v.as_str()) == Some("platform");
            if is_platform {
                if want_platforms {
                    way.tags.insert("layer".to_string(), "platforms".to_string());
                    ways.push(way);
                }
                continue;
            }
            // Small green patches (verges, lawns) are generalized away in
            // the z14 base tiles; at street zoom the detail archive fills
            // them back in. Bigger landuse stays with the base tile.
            let is_green_patch = matches!(
                way.tags.get("landuse").map(|v| v.as_str()),
                Some("grass" | "village_green" | "flowerbed" | "meadow")
            ) || matches!(
                way.tags.get("leisure").map(|v| v.as_str()),
                Some("garden")
            ) || matches!(
                way.tags.get("natural").map(|v| v.as_str()),
                Some("scrub" | "heath" | "shrubbery" | "sand" | "beach" | "shingle")
            );
            // Zoo perimeter draws carto's purple boundary line.
            if matches!(
                way.tags.get("tourism").map(|v| v.as_str()),
                Some("zoo" | "theme_park")
            ) {
                if want_platforms {
                    way.tags
                        .insert("layer".to_string(), "tourism_boundary".to_string());
                    ways.push(way);
                }
                continue;
            }
            let is_building = way
                .tags
                .get("building")
                .is_some_and(|value| value != "no");
            let is_building_part = way
                .tags
                .get("building:part")
                .is_some_and(|value| value != "no");
            // Named zoo enclosures / attractions label at their centroid
            // (and fill if they carry a surface like sand). Famous BUILDINGS
            // also carry tourism=attraction (Westerkerk, Munttoren…) — in 3D
            // mode they must fall through to the extrusion path, not get
            // swallowed as a flat attraction fill.
            let is_attraction = way.tags.contains_key("name")
                && (way.tags.contains_key("attraction")
                    || way.tags.contains_key("zoo")
                    || way.tags.get("tourism").map(|v| v.as_str()) == Some("attraction"))
                && !(want_buildings && (is_building || is_building_part));
            if is_attraction {
                if want_platforms {
                    way.tags
                        .insert("layer".to_string(), "attraction_area".to_string());
                    ways.push(way);
                }
                continue;
            }
            if is_green_patch {
                if want_platforms {
                    way.tags.insert("layer".to_string(), "detail_land".to_string());
                    ways.push(way);
                }
                continue;
            }
            // Pedestrian squares (Hella Haasseplein) are polygons the z14
            // base generalizes away; route them into the existing street-
            // area pipeline so fill, rank and labels all apply.
            let is_pedestrian_area = matches!(
                way.tags.get("highway").map(|v| v.as_str()),
                Some("pedestrian" | "footway")
            ) || way.tags.get("place").map(|v| v.as_str()) == Some("square");
            if is_pedestrian_area {
                if want_platforms {
                    way.tags
                        .insert("layer".to_string(), "street_polygons".to_string());
                    ways.push(way);
                }
                continue;
            }
            if !want_buildings {
                continue;
            }
            if !is_building && !is_building_part {
                continue;
            }
            // Underground volumes (metro halls mapped as building:part,
            // parking cellars) must never extrude above ground.
            if way.tags.get("location").map(|v| v.as_str()) == Some("underground")
                || way
                    .tags
                    .get("osm_layer")
                    .is_some_and(|value| value.starts_with('-'))
            {
                continue;
            }
            way.tags
                .insert("layer".to_string(), "detail_buildings".to_string());
            ways.push(way);
        }
    }
    // Distinct tag keys on forwarded detail
    // ways — the ground truth for the parse whitelist above.
    if crate::makepad_platform::makepad_error_log::trace_enabled("map.census") {
        let mut census = std::collections::BTreeMap::<String, usize>::new();
        for way in ways.iter().skip(census_start) {
            for key in way.tags.keys() {
                *census.entry(key.clone()).or_default() += 1;
            }
        }
        trace!("map.census", "z{}/{}/{}: {:?}", tile_key.z, tile_key.x, tile_key.y, census);
    }
    Ok(())
}

/// Per-icon zoom gates, carto-style: doors only when you could walk
/// through one, signals/chargers at street level.
fn micro_icon_min_zoom(icon: &str) -> f32 {
    match icon {
        "entrance" => 18.0,
        "traffic_signals" | "charger" | "dot" => 16.5,
        "parking" => 15.5,
        _ => 0.0,
    }
}

/// ADAPTIVE Chaikin corner-cutting: only vertices whose adjacent segments
/// are both short (dense curve sampling from tile quantization) get cut;
/// sparse vertices are real corners — street grids must stay sharp or
/// roads round through buildings and detach from their bridges.
fn chaikin_smooth(points: &[(f32, f32)], rounds: usize, cut_below: f32) -> Vec<(f32, f32)> {
    if rounds == 0 || points.len() < 3 || points.len() > 2000 {
        return points.to_vec();
    }
    let closed = points.first() == points.last();
    let mut pts = if closed {
        points[..points.len() - 1].to_vec()
    } else {
        points.to_vec()
    };
    let cut_below_sq = cut_below * cut_below;
    let seg_sq = |a: (f32, f32), b: (f32, f32)| {
        let dx = b.0 - a.0;
        let dy = b.1 - a.1;
        dx * dx + dy * dy
    };
    let lerp =
        |a: (f32, f32), b: (f32, f32), t: f32| (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
    for _ in 0..rounds {
        if pts.len() < 3 {
            break;
        }
        let n = pts.len();
        let mut out = Vec::with_capacity(n * 2 + 2);
        let range = if closed { 0..n } else { 1..n - 1 };
        if !closed {
            out.push(pts[0]);
        }
        for i in range {
            let prev = pts[(i + n - 1) % n];
            let v = pts[i];
            let next = pts[(i + 1) % n];
            // Only gentle bends get cut (turn < ~30 degrees): densely
            // sampled quay curves still carry SHARP corners at bridge
            // junctions between short segments — rounding those pulls
            // the road through the corner buildings.
            let a = (v.0 - prev.0, v.1 - prev.1);
            let b = (next.0 - v.0, next.1 - v.1);
            let dot = (a.0 * b.0 + a.1 * b.1) as f64;
            let len = ((a.0 as f64 * a.0 as f64 + a.1 as f64 * a.1 as f64)
                * (b.0 as f64 * b.0 as f64 + b.1 as f64 * b.1 as f64))
                .sqrt();
            let gentle = len > 1e-12 && dot / len > 0.866;
            if gentle && seg_sq(prev, v) < cut_below_sq && seg_sq(v, next) < cut_below_sq {
                out.push(lerp(v, prev, 0.25));
                out.push(lerp(v, next, 0.25));
            } else {
                out.push(v);
            }
        }
        if !closed {
            out.push(*pts.last().unwrap());
        }
        pts = out;
    }
    if closed {
        if let Some(&first) = pts.first() {
            pts.push(first);
        }
    }
    pts
}

/// `chaikin_smooth` with the bridge-dz channel riding along: dz lerps with
/// the same 0.25 corner cuts so lifted geometry keeps its ramp profile
/// through the smoothing.
fn chaikin_smooth_dz(
    points: &[(f32, f32)],
    dz: Option<&[f32]>,
    rounds: usize,
    cut_below: f32,
) -> (Vec<(f32, f32)>, Option<Vec<f32>>) {
    let Some(dz) = dz else {
        return (chaikin_smooth(points, rounds, cut_below), None);
    };
    if rounds == 0 || points.len() < 3 || points.len() > 2000 || dz.len() != points.len() {
        return (points.to_vec(), Some(dz.to_vec()));
    }
    let closed = points.first() == points.last();
    let mut pts: Vec<(f32, f32, f32)> = points
        .iter()
        .zip(dz)
        .map(|(&(x, y), &d)| (x, y, d))
        .collect();
    if closed {
        pts.pop();
    }
    let cut_below_sq = cut_below * cut_below;
    let seg_sq = |a: (f32, f32, f32), b: (f32, f32, f32)| {
        let dx = b.0 - a.0;
        let dy = b.1 - a.1;
        dx * dx + dy * dy
    };
    let lerp = |a: (f32, f32, f32), b: (f32, f32, f32), t: f32| {
        (
            a.0 + (b.0 - a.0) * t,
            a.1 + (b.1 - a.1) * t,
            a.2 + (b.2 - a.2) * t,
        )
    };
    for _ in 0..rounds {
        if pts.len() < 3 {
            break;
        }
        let n = pts.len();
        let mut out = Vec::with_capacity(n * 2 + 2);
        let range = if closed { 0..n } else { 1..n - 1 };
        if !closed {
            out.push(pts[0]);
        }
        for i in range {
            let prev = pts[(i + n - 1) % n];
            let v = pts[i];
            let next = pts[(i + 1) % n];
            let a = (v.0 - prev.0, v.1 - prev.1);
            let b = (next.0 - v.0, next.1 - v.1);
            let dot = (a.0 * b.0 + a.1 * b.1) as f64;
            let len = ((a.0 as f64 * a.0 as f64 + a.1 as f64 * a.1 as f64)
                * (b.0 as f64 * b.0 as f64 + b.1 as f64 * b.1 as f64))
                .sqrt();
            let gentle = len > 1e-12 && dot / len > 0.866;
            if gentle && seg_sq(prev, v) < cut_below_sq && seg_sq(v, next) < cut_below_sq {
                out.push(lerp(v, prev, 0.25));
                out.push(lerp(v, next, 0.25));
            } else {
                out.push(v);
            }
        }
        if !closed {
            out.push(*pts.last().unwrap());
        }
        pts = out;
    }
    if closed {
        if let Some(&first) = pts.first() {
            pts.push(first);
        }
    }
    (
        pts.iter().map(|&(x, y, _)| (x, y)).collect(),
        Some(pts.iter().map(|&(_, _, d)| d).collect()),
    )
}

/// Building height in meters from OSM tags: explicit `height`, else
/// `building:levels` × 3m + roof allowance, else a modest default.
fn building_height_m(tags: &HashMap<String, String>) -> f32 {
    if let Some(height) = tags.get("height") {
        let digits: String = height
            .trim()
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if let Ok(h) = digits.parse::<f32>() {
            return h.clamp(2.0, 220.0);
        }
    }
    if let Some(levels) = tags.get("building:levels") {
        if let Ok(n) = levels.trim().parse::<f32>() {
            // "nan"/"inf" parse as valid f32s; three buildings on the
            // planet carry such tags and a NaN height panics the bake.
            if n.is_finite() {
                return (n * 3.0 + 2.0).clamp(3.0, 220.0);
            }
        }
    }
    8.0
}

/// Base height (bottom of the volume) for building:part features:
/// `min_height` meters, else `building:min_level` x 3m.
fn building_min_height_m(tags: &HashMap<String, String>) -> f32 {
    if let Some(min_height) = tags.get("min_height") {
        let digits: String = min_height
            .trim()
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if let Ok(h) = digits.parse::<f32>() {
            return h.clamp(0.0, 220.0);
        }
    }
    if let Some(levels) = tags.get("building:min_level") {
        if let Ok(n) = levels.trim().parse::<f32>() {
            if n.is_finite() {
                return (n * 3.0).clamp(0.0, 220.0);
            }
        }
    }
    0.0
}

/// Ray-cast point-in-polygon on a tile-local ring.
fn point_in_ring(point: (f32, f32), ring: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let n = ring.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = ring[i];
        let (xj, yj) = ring[j];
        if (yi > point.1) != (yj > point.1) {
            let x_cross = xi + (point.1 - yi) / (yj - yi) * (xj - xi);
            if point.0 < x_cross {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// A low-poly SPHERE: horizontal rings in map units, per-vertex height in
/// param4 — the tilt shader's per-meter lift renders a true ball silhouette
/// (stacked flat discs read as separate pancakes).
#[allow(clippy::too_many_arguments)]
fn append_ball(
    center: (f32, f32),
    radius_units: f32,
    radius_m: f32,
    center_h_m: f32,
    color: [f32; 4],
    segs: u32,
    rings: u32,
    sun: &SceneSun,
    material: f32,
    out_vertices: &mut Vec<f32>,
    out_indices: &mut Vec<u32>,
    zbias: &mut f32,
) {
    let (segs, rings) = (segs.max(3), rings.max(2));
    // Phong-ish per-vertex lighting (Gouraud across the triangles): the
    // one SceneSun shared with the building walls plus a tight glossy
    // highlight, so canopies and lights read as lit volumes instead of
    // flat blobs. Map coords: x east, y SOUTH (screen down), z up.
    let light = (sun.dir.x, sun.dir.y, sun.dir.z);
    let view = {
        let (vx, vy, vz) = (0.0f32, 0.62, 0.79);
        let len = (vx * vx + vy * vy + vz * vz).sqrt();
        (vx / len, vy / len, vz / len)
    };
    let half = {
        let (hx, hy, hz) = (light.0 + view.0, light.1 + view.1, light.2 + view.2);
        let len = (hx * hx + hy * hy + hz * hz).sqrt();
        (hx / len, hy / len, hz / len)
    };
    let lit = |nx: f32, ny: f32, nz: f32| -> [f32; 4] {
        let ndl = (nx * light.0 + ny * light.1 + nz * light.2).max(0.0);
        let ndh = (nx * half.0 + ny * half.1 + nz * half.2).max(0.0);
        let diffuse = 0.45 + 0.55 * ndl;
        let spec = ndh.powi(32) * 0.85;
        [
            (color[0] * diffuse + spec).min(1.0),
            (color[1] * diffuse + spec).min(1.0),
            (color[2] * diffuse + spec).min(1.0),
            color[3],
        ]
    };
    let base = (out_vertices.len() / VECTOR_FLOATS_PER_VERTEX) as u32;
    let mut push_vertex = |x: f32, y: f32, h: f32, shade: [f32; 4], nx: f32, ny: f32| {
        out_vertices.extend_from_slice(&[
            x, y, 0.5, 1.0, shade[0], shade[1], shade[2], shade[3], 1e6, 0.0, 0.0, 0.0,
            nx, ny, material, h, BUILDING_SURFACE_DEPTH, 24.0, *zbias,
        ]);
    };
    // rings from south pole (phi -90) to north pole (phi +90)
    for ring in 0..=rings {
        let phi = (ring as f32 / rings as f32 - 0.5) * std::f32::consts::PI;
        let ring_r = radius_units * phi.cos();
        let h = center_h_m + radius_m * phi.sin();
        for seg in 0..segs {
            let a = seg as f32 / segs as f32 * std::f32::consts::TAU;
            let (nx, ny, nz) = (phi.cos() * a.cos(), phi.cos() * a.sin(), phi.sin());
            let shade = lit(nx, ny, nz);
            push_vertex(
                center.0 + a.cos() * ring_r,
                center.1 + a.sin() * ring_r,
                h,
                shade,
                nx,
                ny,
            );
        }
    }
    for ring in 0..rings {
        for seg in 0..segs {
            let next = (seg + 1) % segs;
            let a = base + ring * segs + seg;
            let b = base + ring * segs + next;
            let c = base + (ring + 1) * segs + seg;
            let d = base + (ring + 1) * segs + next;
            out_indices.extend_from_slice(&[a, b, c, b, d, c]);
        }
    }
    *zbias += VECTOR_ZBIAS_STEP;
}

/// One flat-shaded wall quad: two ground vertices and two roof vertices
/// whose height rides in param4 for the tilt shader to lift. The outward
/// normal + material id ride in param1..3 (T1 channels); `ao_bottom`
/// darkens the two ground vertices (T2 vertical AO gradient, 1.0 = off).
#[allow(clippy::too_many_arguments)]
fn append_wall_quad(
    a: (f32, f32),
    b: (f32, f32),
    base_m: f32,
    height_m: f32,
    color: [f32; 4],
    ao_bottom: f32,
    normal: (f32, f32),
    material: f32,
    out_vertices: &mut Vec<f32>,
    out_indices: &mut Vec<u32>,
    zbias: &mut f32,
) {
    let base = (out_vertices.len() / VECTOR_FLOATS_PER_VERTEX) as u32;
    for (p, h, ao) in [
        (a, base_m, ao_bottom),
        (b, base_m, ao_bottom),
        (b, height_m, 1.0),
        (a, height_m, 1.0),
    ] {
        out_vertices.extend_from_slice(&[
            p.0,
            p.1,
            0.5,
            1.0,
            color[0] * ao,
            color[1] * ao,
            color[2] * ao,
            color[3],
            1e6,
            0.0,
            0.0,
            0.0,
            normal.0,
            normal.1,
            material,
            h,
            BUILDING_SURFACE_DEPTH,
            90.0,
            *zbias,
        ]);
    }
    out_indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    *zbias += VECTOR_ZBIAS_STEP;
}

/// One building wall edge as an instance record (`WALL_INSTANCE_FLOATS`):
/// the shader builds the quad, lifts the top by the height and shades the
/// bottom by the AO term — the same vertices `append_wall_quad` used to
/// write. Takes the same zbias step so sibling geometry keeps its order.
#[allow(clippy::too_many_arguments)]
fn push_wall_instance(
    out: &mut Vec<f32>,
    a: (f32, f32),
    b: (f32, f32),
    base_m: f32,
    height_m: f32,
    color: [f32; 4],
    ao_bottom: f32,
    normal: (f32, f32),
    zbias: &mut f32,
) {
    out.extend_from_slice(&[
        a.0,
        a.1,
        b.0,
        b.1,
        base_m,
        height_m,
        normal.0,
        normal.1,
        ao_bottom,
        crate::makepad_draw::vector::pack_unorm8x4(color[0], color[1], color[2], color[3]),
        *zbias,
    ]);
    *zbias += VECTOR_ZBIAS_STEP;
}

/// T3 contact-shadow decal: a radial-gradient disc on the ground
/// (alpha `strength` at center, 0 at the rim). Split into the shadow-disc
/// stream and drawn only into MapView's screen-space shadow mask.
fn append_ground_shadow_disc(
    center: (f32, f32),
    radius_units: f32,
    strength: f32,
    depth_micro: f32,
    out_vertices: &mut Vec<f32>,
    out_indices: &mut Vec<u32>,
    zbias: &mut f32,
) {
    const SEGS: u32 = 10;
    let base = (out_vertices.len() / VECTOR_FLOATS_PER_VERTEX) as u32;
    let mut push_vertex = |x: f32, y: f32, alpha: f32| {
        out_vertices.extend_from_slice(&[
            x,
            y,
            0.5,
            1.0,
            0.0,
            0.0,
            0.0,
            alpha,
            1e6,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            MAT_SHADOW,
            0.0,
            depth_micro,
            radius_units * 2.0,
            *zbias,
        ]);
    };
    push_vertex(center.0, center.1, strength);
    for seg in 0..SEGS {
        let a = seg as f32 / SEGS as f32 * std::f32::consts::TAU;
        push_vertex(center.0 + a.cos() * radius_units, center.1 + a.sin() * radius_units, 0.0);
    }
    for seg in 0..SEGS {
        let next = (seg + 1) % SEGS;
        out_indices.extend_from_slice(&[base, base + 1 + seg, base + 1 + next]);
    }
    *zbias += VECTOR_ZBIAS_STEP;
}

/// Clip, winding-normalize and tessellate boolean shadow shapes into the
/// icon buffer as material-6 decals. The boolean Difference can hand back
/// hole rings wound like outers; un-normalized they invert the fill and
/// paint self-overlapping wedges that z-fight (sharp corner lines +
/// striping seen in review). Outer ring positive, holes negative.
#[allow(clippy::too_many_arguments)]
fn emit_shadow_shapes(
    shapes: Vec<Vec<Vec<[f64; 2]>>>,
    clip_bounds: GeoBounds,
    aa: f32,
    tolerance: f32,
    path: &mut VectorPath,
    tess: &mut Tessellator,
    tess_verts: &mut Vec<VVertex>,
    tess_indices: &mut Vec<u32>,
    out_vertices: &mut Vec<f32>,
    out_indices: &mut Vec<u32>,
    zbias: &mut f32,
) {
    for shape in shapes {
        let mut any_ring = false;
        for (ring_index, ring) in shape.iter().enumerate() {
            let pts: Vec<(f32, f32)> = ring
                .iter()
                .map(|p| (p[0] as f32, p[1] as f32))
                .collect();
            let mut clipped = clip_ring_to_rect(&pts, clip_bounds);
            if clipped.len() < 3 {
                continue;
            }
            let area = polygon_signed_area(&clipped);
            // Needle filter: the boolean leaves hair-thin slivers where a
            // projected edge grazes a footprint. Average width below ~a
            // decimeter of tile space reads as a dark pin — drop it.
            let mut perimeter = 0.0f64;
            for i in 0..clipped.len() {
                let a = clipped[i];
                let b = clipped[(i + 1) % clipped.len()];
                perimeter +=
                    (((b.0 - a.0) * (b.0 - a.0) + (b.1 - a.1) * (b.1 - a.1)) as f64).sqrt();
            }
            let min_width = (aa as f64 * 0.8).max(0.05);
            if area.abs() < 0.02 || area.abs() / perimeter.max(1e-6) < min_width {
                if ring_index == 0 {
                    break;
                }
                continue;
            }
            if (ring_index == 0 && area < 0.0) || (ring_index > 0 && area > 0.0) {
                clipped.reverse();
            }
            emit_path(path, &clipped, true);
            any_ring = true;
        }
        if !any_ring {
            continue;
        }
        // Bevel joins: after the footprint subtraction the shadow boundary
        // meets building corners at acute angles, and a miter fringe
        // extrudes long dark spikes past the silhouette.
        tessellate_path_fill(
            path,
            tess,
            tess_verts,
            tess_indices,
            LineJoin::Bevel,
            1.0,
            aa,
            false,
            tolerance,
        );
        // ICON buffer (pass 3, after the road strokes), like the district
        // tints: in a city almost all ground between buildings is road
        // surface, and a shadow in the fill pass would be painted over by
        // every street. Full-dark premultiplied black; the material-6
        // shader branch scales it by the live shadow_alpha uniform.
        append_tessellated_geometry(
            tess_verts,
            tess_indices,
            out_vertices,
            out_indices,
            VectorRenderParams {
                color: [0.0, 0.0, 0.0, 1.0],
                stroke_mult: 1e6,
                shape_id: 0.0,
                params: [0.0, 0.0, 0.0, MAT_SHADOW, 0.0, SHADOW_DECAL_DEPTH],
                zbias: *zbias,
            },
        );
        *zbias += VECTOR_ZBIAS_STEP;
    }
}

/// Miter-offset a positively wound ring outward by `amount` (tile units).
/// Used to dilate the footprints subtracted from the shadow union: real
/// sub-meter slits between abutting building sections otherwise collect
/// shadow and read as dark hairline spikes between the walls.
fn dilate_ring(ring: &[(f32, f32)], amount: f32) -> Vec<(f32, f32)> {
    let n = ring.len();
    if n < 3 || amount <= 0.0 {
        return ring.to_vec();
    }
    let edge_normal = |i: usize| -> Option<(f32, f32)> {
        let a = ring[i % n];
        let b = ring[(i + 1) % n];
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-4 {
            return None;
        }
        // Outward normal of a positively wound (y-down clockwise) ring.
        Some((dy / len, -dx / len))
    };
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let prev = (0..n)
            .map(|k| (i + n - 1 - k) % n)
            .find_map(edge_normal)
            .unwrap_or((0.0, 0.0));
        let next = (0..n).map(|k| (i + k) % n).find_map(edge_normal).unwrap_or(prev);
        let (mx, my) = (prev.0 + next.0, prev.1 + next.1);
        let len = (mx * mx + my * my).sqrt();
        if len < 1e-4 {
            out.push(ring[i]);
            continue;
        }
        let scale = (2.0 / len).min(2.0);
        out.push((
            ring[i].0 + mx / len * amount * scale,
            ring[i].1 + my / len * amount * scale,
        ));
    }
    out
}

/// Even-odd point-in-shapes over i_overlay output (outer rings + holes):
/// used to drop contact-shadow discs for trees already standing inside a
/// building's cast shadow (stacked decals double-darken and z-fight).
fn point_in_shadow_shapes(p: (f32, f32), shapes: &[Vec<Vec<[f64; 2]>>]) -> bool {
    let (px, py) = (p.0 as f64, p.1 as f64);
    for shape in shapes {
        let mut inside = false;
        for ring in shape {
            let n = ring.len();
            if n < 3 {
                continue;
            }
            let mut j = n - 1;
            for i in 0..n {
                let (xi, yi) = (ring[i][0], ring[i][1]);
                let (xj, yj) = (ring[j][0], ring[j][1]);
                if (yi > py) != (yj > py) && px < xi + (py - yi) / (yj - yi) * (xj - xi) {
                    inside = !inside;
                }
                j = i;
            }
        }
        if inside {
            return true;
        }
    }
    false
}

/// T2 roof-edge/parapet AO: a gradient quad strip hugging the roof outline,
/// dark at the edge fading to the plain roof color ~1.5 m inward. Drawn on
/// top of the roof fill (micro-depth one rank above), so the roof reads as
/// a slab with a lip instead of a flat sticker.
fn append_roof_edge_ao(
    ring: &[(f32, f32)],
    height_m: f32,
    roof_color: [f32; 4],
    inset_units: f32,
    out_vertices: &mut Vec<f32>,
    out_indices: &mut Vec<u32>,
    zbias: &mut f32,
) {
    let n = ring.len();
    if n < 3 || inset_units <= 0.0 {
        return;
    }
    // Skip slivers the strip couldn't fit into without self-crossing.
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for &(x, y) in ring {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    if (max_x - min_x).min(max_y - min_y) < inset_units * 3.0 {
        return;
    }
    // Interior side: exterior rings wind positive (clockwise in y-down
    // space), holes negative — the roof interior flips accordingly.
    let interior_sign = if polygon_signed_area(ring) > 0.0 { 1.0 } else { -1.0 };
    // Drop duplicate closing point if present.
    let n = if ring[0] == ring[n - 1] { n - 1 } else { n };
    if n < 3 {
        return;
    }
    let edge_normal = |i: usize| -> Option<(f32, f32)> {
        let a = ring[i % n];
        let b = ring[(i + 1) % n];
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-4 {
            return None;
        }
        // Inward normal (toward the roof interior).
        Some((-dy / len * interior_sign, dx / len * interior_sign))
    };
    // Per-vertex miter offset from the two adjacent edges.
    let mut inner: Vec<(f32, f32)> = Vec::with_capacity(n);
    for i in 0..n {
        let prev = (0..n)
            .map(|k| (i + n - 1 - k) % n)
            .find_map(edge_normal)
            .unwrap_or((0.0, 0.0));
        let next = (0..n).map(|k| (i + k) % n).find_map(edge_normal).unwrap_or(prev);
        let (mx, my) = (prev.0 + next.0, prev.1 + next.1);
        let len = (mx * mx + my * my).sqrt();
        if len < 1e-4 {
            inner.push(ring[i]);
            continue;
        }
        // Miter scale = 1/cos(half-angle), clamped so spikes stay short.
        let scale = (2.0 / len).min(2.5);
        inner.push((
            ring[i].0 + mx / len * inset_units * scale,
            ring[i].1 + my / len * inset_units * scale,
        ));
    }
    const PARAPET_SHADE: f32 = 0.86;
    let outer_color = [
        roof_color[0] * PARAPET_SHADE,
        roof_color[1] * PARAPET_SHADE,
        roof_color[2] * PARAPET_SHADE,
        roof_color[3],
    ];
    let base = (out_vertices.len() / VECTOR_FLOATS_PER_VERTEX) as u32;
    let mut push_vertex = |p: (f32, f32), color: [f32; 4]| {
        out_vertices.extend_from_slice(&[
            p.0,
            p.1,
            0.5,
            1.0,
            color[0],
            color[1],
            color[2],
            color[3],
            1e6,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            MAT_ROOF,
            height_m,
            BUILDING_SURFACE_DEPTH + DEPTH_MICRO_PER_RANK,
            90.0,
            *zbias,
        ]);
    };
    for i in 0..n {
        push_vertex(ring[i], outer_color);
        push_vertex(inner[i], roof_color);
    }
    for i in 0..n as u32 {
        let j = (i + 1) % n as u32;
        out_indices.extend_from_slice(&[
            base + i * 2,
            base + j * 2,
            base + j * 2 + 1,
            base + i * 2,
            base + j * 2 + 1,
            base + i * 2 + 1,
        ]);
    }
    *zbias += VECTOR_ZBIAS_STEP;
}

/// A way in tile-local coordinates ready for styling/tessellation.
pub struct TileWay {
    pub points: Vec<(f32, f32)>,
    pub tags: HashMap<String, String>,
    pub closed: bool,
    /// Baked per-vertex deck height (m), aligned with `points` — from the
    /// base_dz overlay join. The way lifts off its own profile.
    pub dz: Option<Vec<f32>>,
    /// Source-layer feature index in MVT decode order: the join key into
    /// the baked fill stream (protobuf field 100, payload v2-fills-1).
    /// None for ways that did not come from a plain base-tile decode.
    pub fidx: Option<u32>,
}

/// Stage clock for `map.tile_profile`: per-stage wall time, so the
/// generator's cost distribution is measurable headless and in-app alike.
struct ProfileClock(f64);

impl ProfileClock {
    fn now() -> Self {
        Self(Cx::monotonic_now())
    }

    fn elapsed_seconds(&self) -> f64 {
        Cx::monotonic_now() - self.0
    }
}

#[cfg(test)]
#[test]
fn profile_clock_elapsed_is_non_negative() {
    let clock = ProfileClock::now();
    assert!(clock.elapsed_seconds() >= 0.0);
}

#[cfg(test)]
#[test]
fn compact_fill_record_roundtrips_within_packed_precision() {
    use crate::makepad_draw::vector::{
        pack_fill_record, unpack_fill_depths, unpack_pair_f16,
        FILL_PACKED_FLOATS_PER_VERTEX,
    };

    let mut record = [0.0f32; VECTOR_FLOATS_PER_VERTEX];
    record[0] = 123.25;
    record[1] = -45.5;
    record[2] = 0.37;
    record[4..8].copy_from_slice(&[0.13, 0.47, 0.81, 0.62]);
    record[8] = 1e6;
    record[10] = 30.0;
    record[14] = 5.0;
    record[16] = 0.00873;
    record[18] = 0.004321;

    let packed = pack_fill_record(&record).unwrap();
    assert_eq!(packed.len(), FILL_PACKED_FLOATS_PER_VERTEX);
    assert_eq!(
        std::mem::size_of::<crate::makepad_draw::geometry::geometry_gen::FillVertexPacked>(),
        20
    );
    assert_eq!((packed[0], packed[1]), (record[0], record[1]));
    let (code, coverage) = unpack_pair_f16(packed[3]);
    assert_eq!(code, 30.0);
    assert!((coverage - record[2]).abs() <= 0.00025);
    let rgba = packed[2].to_bits();
    for (channel, shift) in record[4..8].iter().zip([0, 8, 16, 24]) {
        let unpacked = ((rgba >> shift) & 0xff) as f32 / 255.0;
        assert!((unpacked - channel).abs() <= 0.5 / 255.0 + f32::EPSILON);
    }
    let (zbias, param5) = unpack_fill_depths(packed[4]);
    assert!((zbias - record[18]).abs() <= VECTOR_ZBIAS_STEP * 0.5 + f32::EPSILON);
    assert!((param5 - record[16]).abs() <= 0.000005 + f32::EPSILON);
}

#[cfg(test)]
#[test]
fn road_vertex_pack_round_trips_deck_depth_ticks_uv_and_pixel_fields() {
    use crate::makepad_draw::geometry::geometry_gen::RoadVertexPacked;
    use crate::makepad_draw::vector::{
        unpack_pair_f16, ROAD_PACKED_FLOATS_PER_VERTEX, ROAD_PARAM_KIND_SCALE,
    };

    assert_eq!(std::mem::size_of::<RoadVertexPacked>(), 32);
    let mut record = [0.0f32; VECTOR_FLOATS_PER_VERTEX];
    record[0] = 14.0;
    record[1] = 27.0;
    record[2] = 0.25;
    record[3] = 0.625;
    record[4..8].copy_from_slice(&[0.1, 0.3, 0.7, 1.0]);
    record[9] = 511.5;
    record[10] = 112.0;
    record[12] = -1.75;
    record[13] = 2.5;
    record[14] = 1.0;
    record[15] = 100.25;
    record[16] = 0.384;
    record[18] = 321.0 * VECTOR_ZBIAS_STEP;
    let packed = pack_road_vertices(&record);
    assert_eq!(packed.len(), ROAD_PACKED_FLOATS_PER_VERTEX);
    let (ox, oy) = unpack_pair_f16(packed[2]);
    assert!((ox + 1.75).abs() < 0.002 && (oy - 2.5).abs() < 0.002);
    let rgba = packed[3].to_bits().to_le_bytes();
    for (actual, expected) in rgba
        .into_iter()
        .zip([0.1f32, 0.3, 0.7, 1.0])
    {
        assert!((actual as f32 / 255.0 - expected).abs() <= 0.5 / 255.0 + f32::EPSILON);
    }
    let (meta, stroke_dist) = unpack_pair_f16(packed[4]);
    assert_eq!(meta, 1.0 + 64.0 * 3.0 + 1024.0);
    assert_eq!(stroke_dist, record[9]);
    assert_eq!(packed[5], 100.25);
    let (param5, zbias_ticks) = unpack_pair_f16(packed[6]);
    assert!((param5 - record[16]).abs() < 0.0002);
    assert_eq!(zbias_ticks, 321.0);
    let (u, v) = unpack_pair_f16(packed[7]);
    assert!((u - record[2]).abs() < 0.001);
    assert!((v - record[3]).abs() < 0.001);

    record[2] = -1.0;
    record[3] = 0.375;
    record[8] = VECTOR_ANALYTIC_FRINGE_STROKE_MULT;
    record[10] = 0.0;
    record[14] = 3.0;
    let packed = pack_road_vertices(&record);
    let (meta, coverage) = unpack_pair_f16(packed[4]);
    assert_eq!(meta, 24.0 + ROAD_PARAM_KIND_SCALE * 2.0);
    assert_eq!(coverage, 0.0);
    let (u, v) = unpack_pair_f16(packed[7]);
    assert_eq!(u, -1.0);
    assert!((v - 0.375).abs() < 0.001);

    record[2] = 0.5;
    record[8] = 1e6;
    record[12] = 0.35;
    record[14] = 7.0;
    let packed = pack_road_vertices(&record);
    let (meta, emissive) = unpack_pair_f16(packed[4]);
    assert_eq!(meta, 8.0 * 7.0 + ROAD_PARAM_KIND_SCALE);
    assert!((emissive - 0.35).abs() < 0.001);
}

#[cfg(test)]
#[test]
fn split_fringe_band_then_road_pack_round_trips() {
    use crate::makepad_draw::vector::{
        unpack_pair_f16, ROAD_PACKED_FLOATS_PER_VERTEX, ROAD_PARAM_KIND_SCALE,
    };

    fn push_record(buf: &mut Vec<f32>, x: f32, u: f32, stroke_mult: f32) {
        let mut record = [0.0f32; VECTOR_FLOATS_PER_VERTEX];
        record[0] = x;
        record[1] = 20.0;
        record[2] = u;
        record[4..8].copy_from_slice(&[0.2, 0.4, 0.6, 1.0]);
        record[8] = stroke_mult;
        buf.extend_from_slice(&record);
    }

    let mut vertices = Vec::new();
    push_record(&mut vertices, 10.0, 0.5, 1e6);
    push_record(&mut vertices, 12.0, 0.5, 1e6);
    push_record(&mut vertices, 11.0, 0.5, 1e6);
    push_record(&mut vertices, 11.0, 0.0, VECTOR_ANALYTIC_FRINGE_STROKE_MULT);
    push_record(&mut vertices, 15.0, -1.0, VECTOR_ANALYTIC_FRINGE_STROKE_MULT);
    push_record(&mut vertices, 13.0, -0.5, VECTOR_ANALYTIC_FRINGE_STROKE_MULT);
    let mut indices = vec![0, 1, 2, 3, 4, 5];
    let (fringe_vertices, fringe_indices) = split_fringe_band(&mut vertices, &mut indices);

    assert_eq!(vertices.len(), VECTOR_FLOATS_PER_VERTEX * 3);
    assert_eq!(indices, vec![0, 1, 2]);
    assert_eq!(fringe_vertices.len(), VECTOR_FLOATS_PER_VERTEX * 3);
    assert_eq!(fringe_indices, vec![0, 1, 2]);

    let packed_body = pack_road_vertices(&vertices);
    assert_eq!(packed_body.len(), ROAD_PACKED_FLOATS_PER_VERTEX * 3);
    let (meta, coverage) = unpack_pair_f16(packed_body[4]);
    assert_eq!(meta, ROAD_PARAM_KIND_SCALE);
    assert!((coverage - 0.5).abs() < 0.001);

    let packed_fringe = pack_road_vertices(&fringe_vertices);
    assert_eq!(packed_fringe.len(), ROAD_PACKED_FLOATS_PER_VERTEX * 3);
    let (meta0, cov0) = unpack_pair_f16(packed_fringe[4]);
    let (meta1, cov1) = unpack_pair_f16(packed_fringe[4 + ROAD_PACKED_FLOATS_PER_VERTEX]);
    assert_eq!(meta0, ROAD_PARAM_KIND_SCALE * 2.0);
    assert_eq!(meta1, ROAD_PARAM_KIND_SCALE * 2.0);
    assert!((cov0 - 1.0).abs() < 0.001);
    assert_eq!(cov1, 0.0);
}

struct TileProfiler {
    on: bool,
    last: ProfileClock,
    start: ProfileClock,
    /// Always recorded (cheap): fuels the SLOW-tile replay log even when
    /// stage printing is off.
    laps: Vec<(&'static str, f64)>,
}

impl TileProfiler {
    fn new() -> TileProfiler {
        TileProfiler {
            on: crate::makepad_platform::makepad_error_log::trace_enabled("map.tile_profile"),
            last: ProfileClock::now(),
            start: ProfileClock::now(),
            laps: Vec::new(),
        }
    }
    fn lap(&mut self, name: &'static str, extra: &str) {
        let now = ProfileClock::now();
        let ms = self.last.elapsed_seconds() * 1000.0;
        self.laps.push((name, ms));
        if self.on {
            trace!("map.tile_profile", "{name} {ms:.1}ms {extra}");
        }
        self.last = now;
    }

    /// Compact "stage:ms" summary for builds worth logging.
    fn summary(&self) -> String {
        let mut out = String::new();
        for (name, ms) in &self.laps {
            if *ms < 0.5 {
                continue;
            }
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&format!("{name}:{ms:.0}"));
        }
        out
    }
    fn total(&self, tile_key: TileKey, extra: &str) {
        if !self.on {
            return;
        }
        trace!(
            "map.tile_profile",
            "TOTAL z{}/{}/{} {:.1}ms {extra}",
            tile_key.z,
            tile_key.x,
            tile_key.y,
            self.start.elapsed_seconds() * 1000.0
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn build_tile_buffers_from_features(
    tile_key: TileKey,
    tile_ways: Vec<TileWay>,
    tagged_points: Vec<((f32, f32), HashMap<String, String>)>,
    theme: &CompiledMapTheme,
    render_zoom: u32,
    buildings_3d: bool,
    build_road_core: bool,
    bridge_corridors: Vec<BridgeCorridor>,
    bridge_dz_covered: bool,
    have_charger_overlay: bool,
    baked_fills: Vec<BakedFillFeature>,
    baked_faces: Option<BakedFacesBucket>,
) -> TileBuffers {
    build_tile_buffers_from_features_profiled(
        TileProfiler::new(),
        tile_key,
        tile_ways,
        tagged_points,
        theme,
        render_zoom,
        buildings_3d,
        build_road_core,
        bridge_corridors,
        bridge_dz_covered,
        have_charger_overlay,
        baked_fills,
        baked_faces,
    )
}

/// The shared feature builder with the caller's stage clock: the mbtiles
/// path laps its parse/merge stages on the same profiler so the SLOW-tile
/// log accounts for the WHOLE build, not just post-parse.
#[allow(clippy::too_many_arguments)]
fn build_tile_buffers_from_features_profiled(
    mut profiler: TileProfiler,
    tile_key: TileKey,
    tile_ways: Vec<TileWay>,
    tagged_points: Vec<((f32, f32), HashMap<String, String>)>,
    theme: &CompiledMapTheme,
    render_zoom: u32,
    buildings_3d: bool,
    build_road_core: bool,
    bridge_corridors: Vec<BridgeCorridor>,
    bridge_dz_covered: bool,
    have_charger_overlay: bool,
    baked_fills: Vec<BakedFillFeature>,
    baked_faces: Option<BakedFacesBucket>,
) -> TileBuffers {
    // How much this tile gets magnified on screen at the styled view zoom.
    let render_scale = 2.0_f64
        .powi(render_zoom as i32 - tile_key.z as i32)
        .max(1e-3) as f32;
    // Every open way with baked dz becomes its own lift profile: strokes
    // and arrows match against these (exact same geometry, tight reach) —
    // never against other ways.
    let own_profiles: Vec<BridgeCorridor> = tile_ways
        .iter()
        .filter(|way| !way.closed)
        .filter_map(|way| {
            // Sunk (tunnel) ways stay OUT of the stroke/arrow corridors: a
            // surface stroke directly above a tunnel line would otherwise
            // match it at distance ~0 and sink with it. Tunnel union faces
            // get their dz from the per-tier field instead.
            way.dz
                .as_ref()
                // The solved overlay annotates grounded ways with all-zero
                // profiles too. They cannot lift anything and turning each
                // one into a corridor makes every patterned stroke compare
                // against thousands of irrelevant segments.
                .filter(|dz| {
                    !dz.iter().any(|&d| d < -0.05)
                        && dz.iter().any(|&d| d > 0.05)
                })
                .map(|dz| BridgeCorridor {
                    points: way.points.clone(),
                    decks: dz.clone(),
                    half_width: 2.0,
                    solved: true,
                })
        })
        .collect();
    // Inside baked coverage: only own profiles lift strokes. Outside:
    // the tag-heuristic corridor soup.
    let stroke_corridors_available = if bridge_dz_covered {
        !own_profiles.is_empty()
    } else {
        !bridge_corridors.is_empty()
    };
    // Converts "screen px at render_zoom" into tile-local units.
    let zoom_mult = zoom_width_mult(render_zoom);
    let px_to_units = 1.0 / render_scale;
    let aa_units = 1.0 / render_scale;
    // A one-device-pixel carrier can collapse below a fragment at stale
    // overzoom and steep pitch before the pixel shader ever gets to run.
    // Four bucket pixels still project to at least ~0.59 px at the maximum
    // 78-degree pitch (including half-bucket underscale); signed-u/fwidth
    // keeps only the final one-pixel coverage ramp visible.
    let analytic_fringe_units = ANALYTIC_FRINGE_CARRIER_PX / render_scale;
    let tolerance = DEFAULT_FLATTEN_TOLERANCE / render_scale;

    let mut labels = Vec::<TileLabel>::new();
    let mut pin_hits = Vec::<PinHit>::new();
    let mut icon_jobs =
        Vec::<((f32, f32), &'static IconMesh, u8, u8, f32, u8, f32, f32, f32, f32)>::new();
    let mut tree_points_3d = Vec::<(f32, f32)>::new();
    let mut signal_points_3d = Vec::<(f32, f32)>::new();
    for (point, tags) in &tagged_points {
        let mut label_point = *point;
        let layer = tags.get("layer").map(|value| value.as_str()).unwrap_or("");
        // Overlay points (chargers, transit stops) show earlier than the
        // dense base-POI iconography. Chargers tier by power: an ultra-fast
        // site matters at road-trip zoom, a street post doesn't.
        let icon_zoom_floor = match layer {
            "chargers" => {
                let kw = tags
                    .get("max_kw")
                    .and_then(|value| value.parse::<f64>().ok())
                    .unwrap_or(0.0);
                if kw >= 150.0 {
                    8
                } else if kw >= 50.0 {
                    10
                } else {
                    12
                }
            }
            "stops" => 13,
            _ => ICON_MIN_ZOOM,
        };
        if icon_inclusion_zoom(render_zoom) >= icon_zoom_floor as f32 {
            if let Some((icon_name, color_class)) = icon_for_tags(tags) {
                if let Some(mesh) = icon_mesh(icon_name) {
                    // Doors and generic dots yield to real symbols in the
                    // collision pass (a recycling point must not lose to
                    // the building entrance next to it).
                    // Chargers place before everything (EV navigator) and
                    // are never collided away by shop/POI symbols.
                    let priority = match icon_name {
                        // Overlay charger pins are never collided away —
                        // base-map charging_station icons yield to them.
                        "charger" if layer == "chargers" => 0,
                        "charger" => 2,
                        "entrance" => 3,
                        "dot" => 2,
                        _ => 1,
                    };
                    // Micro street furniture packs tighter than shop/POI
                    // symbols — a bench must not knock out the tree row.
                    let dist_factor = match icon_name {
                        "tree" | "bench" | "waste_basket" | "recycling" | "dot"
                        | "bicycle" => 0.45f32,
                        _ => 1.0,
                    };
                    let charger_kw = (layer == "chargers")
                        .then(|| {
                            tags.get("max_kw")
                                .and_then(|value| value.parse::<f64>().ok())
                                .unwrap_or(0.0)
                        })
                        .unwrap_or(0.0);
                    // Stall count (OCPI EVSEs) rides along for the in-pin
                    // "kW/stalls" text at close zooms.
                    let charger_stalls = (layer == "chargers")
                        .then(|| {
                            tags.get("evses")
                                .and_then(|value| value.parse::<f64>().ok())
                                .unwrap_or(0.0)
                        })
                        .unwrap_or(0.0);
                    // Chargers render as Tesla-style pin badges: wide badge
                    // (bolt + kW text inside) for fast sites, small badge
                    // for street AC.
                    let two_tone = match icon_name {
                        "tree" => 1u8,
                        "charger" if charger_kw >= 50.0 => 2,
                        "charger" => 3,
                        _ => 0,
                    };
                    let mesh = match two_tone {
                        2 => icon_mesh("charger_pin_fast").unwrap_or(mesh),
                        3 => icon_mesh("charger_pin_ac").unwrap_or(mesh),
                        _ => mesh,
                    };
                    // The icon's own zoom floor rides into the vertex data
                    // (param4): the shader hides the icon the instant the
                    // LIVE view zoom drops below it, so stale deeper-bucket
                    // tiles never flash markers while zooming out.
                    // Overlay layers (chargers, stops) use their TIER floor;
                    // micro_icon_min_zoom("charger") is the 16.5 street-level
                    // gate for BASE-map charging posts and must not apply to
                    // overlay pins (it hid every pin below z16 — the
                    // "chargers disappeared" bug).
                    let zoom_floor = if layer == "chargers" || layer == "stops" {
                        icon_zoom_floor as f32
                    } else {
                        micro_icon_min_zoom(icon_name).max(icon_zoom_floor as f32)
                    };
                    // 3D mode: markers fly on stalks above the skyline —
                    // chargers highest, then shops/cafés (base pois), then
                    // transit stops. Street furniture (benches, entrances,
                    // micro POIs) stays on the ground where it belongs.
                    let pin_lift_m = if buildings_3d {
                        if layer == "chargers" {
                            if charger_kw >= 50.0 { 26.0f32 } else { 20.0 }
                        } else if layer == "stops" {
                            12.0
                        } else if tags.get("layer").map(|v| v.as_str()) == Some("pois") {
                            18.0
                        } else {
                            0.0
                        }
                    } else {
                        0.0
                    };
                    // In 3D mode trees become little REAL 3D trees (trunk +
                    // canopy blob lifted by the building height mechanism)
                    // instead of flat billboard discs.
                    // With the charger overlay active the base-map
                    // charging_station icons are duplicates of overlay
                    // pins — drop them instead of letting them collide.
                    if icon_name == "charger" && layer != "chargers" && have_charger_overlay {
                        continue;
                    }
                    if buildings_3d && icon_name == "tree" {
                        tree_points_3d.push(*point);
                        continue;
                    }
                    if buildings_3d && icon_name == "traffic_signals" {
                        signal_points_3d.push(*point);
                        continue;
                    }
                    icon_jobs.push((
                        *point,
                        mesh,
                        color_class,
                        priority,
                        dist_factor,
                        two_tone,
                        charger_kw as f32,
                        charger_stalls as f32,
                        zoom_floor,
                        pin_lift_m,
                    ));
                    if two_tone == 2 || two_tone == 3 {
                        // Tappable: record position + info for the bubble.
                        let world = (1u32 << tile_key.z) as f64;
                        let norm = (
                            (tile_key.x as f64 + point.0 as f64 / TILE_SIZE) / world,
                            (tile_key.y as f64 + point.1 as f64 / TILE_SIZE) / world,
                        );
                        let mut info: Vec<(String, String)> = Vec::new();
                        for key in ["name", "operator", "city", "max_kw", "evses", "connectors"] {
                            if let Some(value) = tags.get(key) {
                                if !value.trim().is_empty() {
                                    info.push((key.to_string(), value.clone()));
                                }
                            }
                        }
                        pin_hits.push(PinHit { norm, info, lift_m: 0.0 });
                    }
                    if two_tone == 2 {
                        // In-pin text via the NORMAL text renderer (drawn in
                        // the post-icon pin phase, billboard-anchored):
                        // Tesla pins show the stall count (the kW is implied
                        // by the brand, like the Tesla app), other brands
                        // show the peak kW.
                        let is_tesla = tags
                            .get("operator")
                            .or_else(|| tags.get("brand"))
                            .is_some_and(|v| v.to_lowercase().contains("tesla"));
                        let pin_text = if is_tesla && charger_stalls >= 1.0 {
                            format!("{:.0}", charger_stalls.min(99.0))
                        } else if charger_kw >= 1.0 {
                            format!("{:.0}", charger_kw.min(999.0))
                        } else {
                            String::new()
                        };
                        if !pin_text.is_empty() {
                            labels.push(TileLabel {
                                text: pin_text,
                                priority: 1,
                                source_layer: "chargers".to_string(),
                                road_kind: format!(
                                    "chp{}_{:.0}x{:.0}",
                                    icon_zoom_floor,
                                    point.0 * 4.0,
                                    point.1 * 4.0
                                ),
                                color_class: crate::map::label::LABEL_CLASS_PIN,
                                path_points: crate::map::label::point_label_path_pub((
                                    point.0, point.1,
                                )),
                                name_key: String::new(),
                                bbox: (0.0, 0.0, 0.0, 0.0),
                                        lift_m: 0.0,
                            });
                        }
                        // brand reads below the pin from z13; the kW digits
                        // are part of the icon composite itself.
                        if render_zoom >= 13 {
                            if let Some(operator) = tags.get("operator") {
                                let brand = operator
                                    .split([' ', '/'])
                                    .next()
                                    .unwrap_or("")
                                    .to_string();
                                if brand.len() >= 2 {
                                    labels.push(TileLabel {
                                        text: brand,
                                        priority: 3,
                                        source_layer: "charger_brand".to_string(),
                                        road_kind: format!(
                                            "chb{:.0}x{:.0}",
                                            point.0 * 4.0,
                                            point.1 * 4.0
                                        ),
                                        color_class: if operator.to_lowercase().contains("tesla")
                                        {
                                            crate::map::label::LABEL_CLASS_HEALTH
                                        } else {
                                            crate::map::label::LABEL_CLASS_AMENITY
                                        },
                                        // Anchor AT the charger point; the
                                        // below-the-pin offset is applied in
                                        // SCREEN space at candidate time so it
                                        // doesn't tilt-compress or orbit the
                                        // billboard pin when the camera moves.
                                        path_points: crate::map::label::point_label_path_pub((
                                            point.0, point.1,
                                        )),
                                        name_key: String::new(),
                                        bbox: (0.0, 0.0, 0.0, 0.0),
                                        lift_m: 0.0,
                                    });
                                }
                            }
                        }
                    } else {
                        // text sits below the symbol, carto-style
                        label_point.1 += 11.0 / render_scale;
                    }
                }
            }
        }
        if let Some(label) = extract_point_label(tags, label_point) {
            labels.push(label);
        }
    }
    icon_jobs.sort_by_key(|job| job.3);

    // Icon-vs-icon collision: keep the first symbol in any ~icon-sized
    // neighborhood (dense shopping streets otherwise stack into a carpet).
    let icon_min_dist = (ICON_SIZE_PX + 3.0) / render_scale;
    let icon_min_dist_sq = icon_min_dist * icon_min_dist;
    let mut accepted_icons = Vec::<(f32, f32)>::new();
    icon_jobs.retain(|(point, _, _, _, dist_factor, _, _, _, _, _)| {
        let collides = accepted_icons.iter().any(|other| {
            let dx = other.0 - point.0;
            let dy = other.1 - point.1;
            dx * dx + dy * dy < icon_min_dist_sq * dist_factor * dist_factor
        });
        if collides {
            false
        } else {
            accepted_icons.push(*point);
            true
        }
    });

    let mut path = VectorPath::new();
    let mut tess = Tessellator::default();
    // Map polygon rings have trustworthy winding everywhere fills are
    // emitted from here: MVT rings are orientation-normalized by
    // classify_polygon_rings, boolean overlay/shadow outputs are
    // winding-consistent by construction. This lets fill() derive the AA
    // fill-side sign from ring orientation instead of O(V^2) probing.
    tess.set_trust_fill_winding(true);
    let mut tess_verts = Vec::<VVertex>::new();
    let mut tess_indices = Vec::<u32>::new();

    // NOTE: do NOT pre-reserve these to "final" sizes. A generous
    // reservation (tried at up to 24M floats) made 12 concurrent builders
    // first-touch ~240MB of fresh zero pages each and serialized the whole
    // pool on the kernel fault path — the buildings stage went 340ms ->
    // 3000ms in-app while staying at 11ms in the serial harness.
    let mut fill_indices = Vec::<u32>::new();
    let mut fill_vertices = Vec::<f32>::new();
    let mut casing_indices = Vec::<u32>::new();
    let mut casing_vertices = Vec::<f32>::new();
    let mut stroke_indices = Vec::<u32>::new();
    let mut stroke_vertices = Vec::<f32>::new();
    let mut icon_indices = Vec::<u32>::new();
    let mut icon_vertices = Vec::<f32>::new();
    // Building walls as instance records; see WALL_INSTANCE_FLOATS.
    let mut wall_instances = Vec::<f32>::new();
    // Street trees: one template mesh per LOD ring + TREE_INSTANCE_FLOATS per tree.
    let mut tree_template_vertices = Vec::<f32>::new();
    let mut tree_template_indices = Vec::<u32>::new();
    let mut tree_cross_template_vertices = Vec::<f32>::new();
    let mut tree_cross_template_indices = Vec::<u32>::new();
    let mut tree_instances = Vec::<f32>::new();
    let mut road_icon_indices = Vec::<u32>::new();
    let mut road_icon_vertices = Vec::<f32>::new();
    let mut fill_zbias = 0.0_f32;
    let mut casing_zbias = 0.0_f32;
    let mut stroke_zbias = 0.0_f32;
    let mut icon_zbias = 0.0_f32;
    let mut feature_count = 0usize;

    let mut prepared = Vec::<PreparedWay>::with_capacity(tile_ways.len());
    for (way_index, way) in tile_ways.iter().enumerate() {
        if way.points.len() < 2 {
            continue;
        }
        prepared.push(PreparedWay {
            way_index,
            points: way.points.clone(),
        });
    }

    // 2.5D: when the detail archive supplied building footprints with real
    // heights, they replace the base building fills entirely. Rings group
    // per source feature so multipolygon buildings (palaces, courtyarded
    // blocks) keep their holes.
    let has_detail_buildings = tile_ways
        .iter()
        .any(|way| way.tags.get("layer").map(|v| v.as_str()) == Some("detail_buildings"));
    struct BuildingGroup {
        rings: Vec<FillRing>,
        height_m: f32,
        min_height_m: f32,
        is_part: bool,
    }
    let mut building_groups = Vec::<BuildingGroup>::new();
    let mut building_group_lookup = HashMap::<String, usize>::new();
    // Building-age layer active: index BAG polygons by quantized centroid
    // so extruded buildings can pick up their bouwjaar tint (BAG footprints
    // match OSM buildings nearly 1:1).
    let mut bag_centroid_colors = HashMap::<(i32, i32), u32>::new();
    for way in tile_ways.iter() {
        if way.tags.get("layer").map(|v| v.as_str()) == Some("bag")
            && way.closed
            && way.points.len() >= 3
        {
            if let Some(color) = crate::map::style::bag_year_color(&way.tags) {
                let c = ring_centroid(&way.points);
                bag_centroid_colors
                    .insert(((c.0 / 6.0).round() as i32, (c.1 / 6.0).round() as i32), color);
            }
        }
    }

    // Fill pass
    let mut fill_groups = Vec::<FillFeatureGroup>::new();
    let mut plaza_rings: Vec<(u32, f32, Vec<(f32, f32)>, Option<Vec<f32>>)> = Vec::new();
    let mut fill_group_lookup = HashMap::<(String, u32, u32), usize>::new();
    // Baked fill join: (baker Layer discriminant, per-layer feature index).
    let baked_fill_lookup: HashMap<(u8, u32), usize> = baked_fills
        .iter()
        .enumerate()
        .map(|(index, bake)| ((bake.layer_id, bake.feature_index), index))
        .collect();
    for (order, prepared_way) in prepared.iter().enumerate() {
        let way = &tile_ways[prepared_way.way_index];
        if way.tags.get("layer").map(|v| v.as_str()) == Some("detail_buildings") {
            let Some(mut ring_points) = normalize_polygon_ring(&prepared_way.points) else {
                continue;
            };
            let clip = tile_clip_bounds((1.0 / render_scale).min(FILL_CLIP_OVERLAP));
            if !ring_inside_bounds(&ring_points, clip) {
                ring_points = clip_ring_to_rect(&ring_points, clip);
                if ring_points.len() < 3 {
                    continue;
                }
            }
            let signed_area = polygon_signed_area(&ring_points);
            if signed_area.abs() <= POLYGON_AREA_EPSILON {
                continue;
            }
            let feature_key = way
                .tags
                .get(MVT_INTERNAL_FEATURE_KEY)
                .cloned()
                .unwrap_or_else(|| format!("bldg:{}", prepared_way.way_index));
            let group_index =
                if let Some(index) = building_group_lookup.get(&feature_key).copied() {
                    index
                } else {
                    let index = building_groups.len();
                    building_group_lookup.insert(feature_key, index);
                    building_groups.push(BuildingGroup {
                        rings: Vec::new(),
                        height_m: building_height_m(&way.tags),
                        min_height_m: building_min_height_m(&way.tags),
                        is_part: way
                            .tags
                            .get("building:part")
                            .is_some_and(|value| value != "no"),
                    });
                    index
                };
            let ring_order = way
                .tags
                .get(MVT_INTERNAL_RING_INDEX_KEY)
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(order);
            building_groups[group_index].rings.push(FillRing {
                order: ring_order,
                points: ring_points,
                signed_area,
            });
            continue;
        }
        if has_detail_buildings && way.tags.contains_key("building") {
            // Base buildings are replaced by the extruded detail set.
            continue;
        }
        // Labels are independent of fills: a named zoo enclosure with no
        // distinctive surface still gets its name at the centroid.
        let fill_color = fill_color_for_tags(theme, &way.tags, way.closed, render_zoom);
        let Some(mut ring_points) = normalize_polygon_ring(&prepared_way.points) else {
            continue;
        };
        // Overlap only needs to cover the AA fringe (~1 screen px). Any wider
        // and the double-drawn strip shows the later tile's LAND painting over
        // the earlier tile's BUILDINGS (per-tile rank order doesn't hold
        // across tiles), visible as a pale band at high zoom.
        let fill_clip_bounds = tile_clip_bounds((1.0 / render_scale).min(FILL_CLIP_OVERLAP));
        if !ring_inside_bounds(&ring_points, fill_clip_bounds) {
            ring_points = clip_ring_to_rect(&ring_points, fill_clip_bounds);
            if ring_points.len() < 3 {
                continue;
            }
        }

        let area_label_ok = render_zoom >= 15
            || matches!(
                way.tags.get("layer").map(|value| value.as_str()),
                Some("natura2000" | "wetlands")
            );
        if area_label_ok {
            if let Some(label) = extract_area_label(&way.tags, ring_centroid(&ring_points)) {
                labels.push(label);
            }
        }
        let source_layer = way.tags.get("layer").map(String::as_str).unwrap_or("");
        if !structural_bridge_area_visible(source_layer, buildings_3d) {
            continue;
        }
        let Some(color) = fill_color else {
            continue;
        };
        let feature_key = way
            .tags
            .get(MVT_INTERNAL_FEATURE_KEY)
            .cloned()
            .unwrap_or_else(|| format!("way:{}", prepared_way.way_index));
        let pattern = fill_pattern_shape(&way.tags);
        let alpha = fill_alpha_for_tags(&way.tags);
        let group_key = (feature_key, color, pattern.to_bits() ^ alpha.to_bits());
        let group_index = if let Some(index) = fill_group_lookup.get(&group_key).copied() {
            index
        } else {
            let index = fill_groups.len();
            fill_group_lookup.insert(group_key, index);
            let mvt_layer = way.tags.get("layer").map(|v| v.as_str()).unwrap_or("");
            let deckable =
                matches!(mvt_layer, "street_polygons" | "streets_med" | "streets_low")
                    && !tag_is_truthy(&way.tags, "tunnel");
            // Attribute decks are the shortbread-tag fallback; solved
            // bridge-dz coverage replaces them with corridor matching.
            let deck_m = if deckable
                && !bridge_dz_covered
                && tag_is_truthy(&way.tags, "bridge")
            {
                9.0
            } else {
                0.0
            };
            let baked = way.fidx.and_then(|fidx| {
                let layer_id = baked_layer_discriminant(mvt_layer)?;
                baked_fill_lookup.get(&(layer_id, fidx)).copied()
            });
            fill_groups.push(FillFeatureGroup {
                color,
                layer_rank: fill_layer_rank(&way.tags),
                is_building: way.tags.contains_key("building"),
                alpha,
                pattern,
                baked,
                material: fill_material_for_tags(&way.tags),
                late: matches!(
                    way.tags.get("layer").map(|v| v.as_str()),
                    Some("gemeenten" | "wijken" | "buurten")
                ),
                deck_m,
                deckable,
                profiles: Vec::new(),
                rings: Vec::new(),
            });
            index
        };

        // Road-surface polygons join the road tier unions instead of the
        // fill pipeline: the junction plaza and its road class must be ONE
        // surface (2D reference: plazas paint over minor-road centers).
        let plaza_layer = way.tags.get("layer").map(|v| v.as_str()).unwrap_or("");
        if build_road_core
            && matches!(plaza_layer, "street_polygons" | "streets_med" | "streets_low")
            && !tag_is_truthy(&way.tags, "tunnel")
            && way.closed
        {
            plaza_rings.push((
                color,
                fill_alpha_for_tags(&way.tags),
                way.points.clone(),
                way.dz.clone(),
            ));
            continue;
        }
        let ring_order = way
            .tags
            .get(MVT_INTERNAL_RING_INDEX_KEY)
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(order);
        let signed_area = polygon_signed_area(&ring_points);
        if signed_area.abs() <= POLYGON_AREA_EPSILON {
            continue;
        }
        if let Some(dz) = &way.dz {
            fill_groups[group_index].profiles.push(BridgeCorridor {
                points: way.points.clone(),
                decks: dz.clone(),
                half_width: 3.0,
                solved: true,
            });
        }
        fill_groups[group_index].rings.push(FillRing {
            order: ring_order,
            points: ring_points,
            signed_area,
        });
    }

    // Road-surface plaza rings diverted to the road tier unions.
    let plaza_rings_ref = &plaza_rings;
    let _ = plaza_rings_ref;
    // Paint fills in semantic order (land -> sites -> water -> buildings ->
    // street areas), not raw MVT layer order which puts land/sites on top of
    // the buildings. Stable within each rank to preserve source order.
    let mut fill_order = (0..fill_groups.len()).collect::<Vec<_>>();
    fill_order.sort_by_key(|&index| fill_groups[index].layer_rank);


    let building_outline = if render_zoom >= BUILDING_OUTLINE_MIN_ZOOM {
        theme.building_outline
    } else {
        None
    };

    for (order_pos, group_index) in fill_order.into_iter().enumerate() {
        let group = &fill_groups[group_index];
        if faces_bake_sink_armed() {
            // Plaza rings were captured above; all other fill meshes are
            // runtime output and are not part of the face stream.
            continue;
        }
        // A same-bucket 2D/3D switch reuses the resident road core. Its
        // deckable street-area fills already live in the stable stroke
        // geometry and must not be emitted a second time.
        if !build_road_core && group.deckable {
            continue;
        }
        // Baked fast path: the feature's body triangulation was pre-baked
        // into the tile (v2-fills-1). Emit the clipped strip triangles
        // directly and add ONLY the AA fringe from the (clipped) runtime
        // rings — the fringe strips are self-contained, so edge AA is the
        // exact geometry the runtime fill would have produced, while the
        // body skips ring classification + sweep tessellation entirely.
        // Empty rings mean the whole feature was diverted (plaza tier) or
        // clipped away — never double-paint from the bake then.
        let baked_body = group
            .baked
            .and_then(|index| baked_fills.get(index))
            .filter(|_| !group.rings.is_empty());
        let mut baked_ready = false;
        if let Some(baked) = baked_body {
            let clip = tile_clip_rect((1.0 / render_scale).min(FILL_CLIP_OVERLAP));
            let baked_area =
                emit_baked_fill_body(baked, clip, &mut tess_verts, &mut tess_indices);
            // Sanity guard: the clipped baked partition must cover the
            // feature's net (clipped) ring area. A bake that disagrees —
            // e.g. an inverted-winding source feature whose exterior the
            // baker mistook for a hole — would erase whole surfaces, so it
            // falls back to runtime tessellation instead.
            let net_ring_area: f64 = group.rings.iter().map(|r| r.signed_area).sum();
            let net_ring_area = net_ring_area.abs();
            if net_ring_area > 1e-6 && (baked_area - net_ring_area).abs() <= net_ring_area * 0.05
            {
                if aa_units > 0.0 {
                    for ring in &group.rings {
                        emit_path(&mut path, &ring.points, true);
                    }
                    tess.flatten(&path, tolerance);
                    tess.fill_fringe_into(
                        aa_units,
                        LineJoin::Miter,
                        4.0,
                        false,
                        &mut tess_verts,
                        &mut tess_indices,
                    );
                    path.clear();
                }
                compute_clip_radii(&mut tess_verts, &tess_indices);
                baked_ready = true;
            } else {
                tess_verts.clear();
                tess_indices.clear();
            }
        }
        let polygons = if baked_ready {
            // One synthetic piece drives the shared emission tail below.
            vec![Vec::new()]
        } else {
            classify_polygon_rings(&group.rings, EARCUT_MAX_RINGS)
        };
        for polygon in polygons {
            if baked_ready {
                // tess_verts / tess_indices already hold the baked body +
                // fringe for this single piece.
            } else {
            if polygon.is_empty() {
                continue;
            }
            for ring in &polygon {
                emit_path(&mut path, ring, true);
            }
            tessellate_path_fill(
                &mut path,
                &mut tess,
                &mut tess_verts,
                &mut tess_indices,
                LineJoin::Miter,
                4.0,
                aa_units,
                false,
                tolerance,
            );
            }
            // Road-surface polygons join the STROKE pass: in tilt mode
            // passes 1-3 carry the relief depth boost, and a junction
            // plaza left in the unboosted fill domain gets sliced by every
            // casing rim crossing it — flat and tilted views must layer
            // roads identically.
            let (target_verts, target_indices, target_zbias) = if group.late {
                (&mut icon_vertices, &mut icon_indices, &mut icon_zbias)
            } else if group.deckable {
                (&mut stroke_vertices, &mut stroke_indices, &mut stroke_zbias)
            } else {
                (&mut fill_vertices, &mut fill_indices, &mut fill_zbias)
            };
            let road_surface_micro = if group.deckable {
                // Under the union surfaces: the tier meshes ARE the road
                // now; plazas are backdrop.
                0.05
            } else {
                0.0
            };
            // Baked coverage: the polygon rides its OWN annotated outline
            // profile (base_dz join). Outside coverage there is no fill
            // profile source, so fills stay on the constant attribute deck.
            let fill_decks: Option<Vec<f32>> =
                if group.deckable && !group.profiles.is_empty() {
                    Some(
                        tess_verts
                            .iter()
                            .map(|v| corridor_deck_at_point(v.x, v.y, &group.profiles))
                            .collect(),
                    )
                } else {
                    None
                };
            // Same-rank micro ladder keyed on the group's PAINT-ORDER
            // position: overlapping same-rank fills are adjacent in
            // fill_order, so adjacent indices can never tie — the old
            // feature_count % 16 collided every 16 features and shimmered
            // (green vs gray z-fight in tilt mode).
            let fill_micro = road_surface_micro
                + group.layer_rank as f32 * DEPTH_MICRO_PER_RANK
                + (order_pos % 19) as f32 * DEPTH_MICRO_PER_FEATURE;
            append_tessellated_geometry_decked(
                &tess_verts,
                &tess_indices,
                target_verts,
                target_indices,
                VectorRenderParams {
                    color: hex_to_premul_rgba(group.color, group.alpha),
                    stroke_mult: 1e6,
                    shape_id: group.pattern,
                    params: [0.0, 0.0, 0.0, group.material, group.deck_m, fill_micro],
                    zbias: *target_zbias,
                },
                fill_decks.as_deref(),
            );
            *target_zbias += VECTOR_ZBIAS_STEP;
            feature_count += 1;

            if let (true, Some(outline)) = (group.is_building, building_outline) {
                // Outline the ring but drop segments that run along the tile
                // cut, so clipped buildings don't get a fake wall at the seam.
                let outline_bounds =
                    tile_clip_bounds((1.0 / render_scale).min(FILL_CLIP_OVERLAP) * 0.2);
                let outline_style = StrokePassStyle { deck_m: 0.0,
                    color: outline,
                    width: BUILDING_OUTLINE_WIDTH_PX / render_scale,
                    shape_id: 0.0,
                    expand_class: EXPAND_CLASS_CONST_PX,
                    depth_micro: 46.0 * DEPTH_MICRO_PER_RANK,
                    emissive: 0.0,
                };
                for ring in &polygon {
                    let mut closed_points = ring.clone();
                    if closed_points.first() != closed_points.last() {
                        if let Some(first) = closed_points.first().copied() {
                            closed_points.push(first);
                        }
                    }
                    for part in clip_polyline_parts(&closed_points, outline_bounds, false) {
                        if part.len() < 2 {
                            continue;
                        }
                        let full_loop = part.len() == closed_points.len()
                            && part.first() == part.last();
                        let points = if full_loop { &part[..part.len() - 1] } else { &part[..] };
                        append_stroke_pass(
                            &mut path,
                            points,
                            full_loop,
                            None,
                            &mut tess,
                            &mut tess_verts,
                            &mut tess_indices,
                            &mut fill_vertices,
                            &mut fill_indices,
                            outline_style,
                            LineCap::Butt,
                            LineCap::Butt,
                            LineJoin::Miter,
                            aa_units,
                            tolerance,
                            &mut fill_zbias,
                            stroke_pass_param5(&outline_style),
                        );
                    }
                }
            }
        }
    }

    profiler.lap("fills", &format!("fill={}KB", fill_vertices.len() * 4 / 1024));
    // Everything appended to the fill buffers from here through the
    // buildings lap is 3D volume (walls, roofs, trees, skirts): split off
    // as fill_3d so distant tiles under tilt can skip/fade it.
    let fill_3d_vert_start = fill_vertices.len();
    let fill_3d_index_start = fill_indices.len();
    let tree_cross_vertices: Vec<f32> = Vec::new();
    let tree_cross_indices: Vec<u32> = Vec::new();

    // v4 building-dissolve capture: filled in the buildings block (jobs
    // are local there), consumed by the bake sink.
    let mut captured_building_sig = 0u64;
    let mut captured_building_groups: Vec<BakedBuildingGroup> = Vec::new();
    // 2.5D building extrusion: per-edge flat-shaded walls (exterior rings
    // AND courtyard holes), then the roof with holes preserved, lifted by
    // height (the tilt shader does the lifting per frame, so tilt animates
    // without rebuilding tiles). North-first paint order is the painter's
    // approximation of occlusion under the screen-top extrusion.
    if !building_groups.is_empty() {
        struct BuildingJob {
            polygon: Vec<Vec<(f32, f32)>>,
            height_m: f32,
            base_m: f32,
            tint: Option<u32>,
            min_y: f32,
        }
        // Simple 3D Buildings: an outline whose interior holds
        // building:parts must NOT extrude — the parts carry the true
        // volumes (Westerkerk's nave + 85m Westertoren); the outline
        // keeps only a flat footprint fill beneath them.
        let part_centroids: Vec<(f32, f32)> = building_groups
            .iter()
            .filter(|group| group.is_part)
            .filter_map(|group| {
                group
                    .rings
                    .iter()
                    .max_by(|a, b| {
                        a.signed_area
                            .abs()
                            .partial_cmp(&b.signed_area.abs())
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|ring| ring_centroid(&ring.points))
            })
            .collect();
        if !part_centroids.is_empty() {
            // Cell grid over part centroids: point_in_ring can only hit a
            // centroid inside the ring's bbox, so the cover test visits one
            // bbox worth of cells instead of every part in the tile —
            // dense part cities (Paris) ran groups x rings x parts x verts.
            const CENTROID_CELL: f32 = 16.0;
            let mut centroid_cells: HashMap<(i32, i32), Vec<(f32, f32)>> = HashMap::new();
            for &c in &part_centroids {
                centroid_cells
                    .entry((
                        (c.0 / CENTROID_CELL).floor() as i32,
                        (c.1 / CENTROID_CELL).floor() as i32,
                    ))
                    .or_default()
                    .push(c);
            }
            for group in building_groups.iter_mut() {
                if group.is_part {
                    continue;
                }
                let covers = group.rings.iter().any(|ring| {
                    let mut min_x = f32::MAX;
                    let mut min_y = f32::MAX;
                    let mut max_x = f32::MIN;
                    let mut max_y = f32::MIN;
                    for &(x, y) in &ring.points {
                        min_x = min_x.min(x);
                        min_y = min_y.min(y);
                        max_x = max_x.max(x);
                        max_y = max_y.max(y);
                    }
                    let cx0 = (min_x / CENTROID_CELL).floor() as i32;
                    let cy0 = (min_y / CENTROID_CELL).floor() as i32;
                    let cx1 = (max_x / CENTROID_CELL).floor() as i32;
                    let cy1 = (max_y / CENTROID_CELL).floor() as i32;
                    (cy0..=cy1).any(|cy| {
                        (cx0..=cx1).any(|cx| {
                            centroid_cells.get(&(cx, cy)).is_some_and(|cell| {
                                cell.iter().any(|&c| {
                                    c.0 >= min_x
                                        && c.0 <= max_x
                                        && c.1 >= min_y
                                        && c.1 <= max_y
                                        && point_in_ring(c, &ring.points)
                                })
                            })
                        })
                    })
                });
                if covers {
                    group.height_m = 0.0;
                    group.min_height_m = 0.0;
                }
            }
        }
        let mut building_jobs = Vec::<BuildingJob>::new();
        for group in &building_groups {
            for polygon in classify_polygon_rings(&group.rings, EARCUT_MAX_RINGS) {
                if polygon.is_empty() {
                    continue;
                }
                let min_y = polygon
                    .iter()
                    .flat_map(|ring| ring.iter())
                    .fold(f32::MAX, |acc, p| acc.min(p.1));
                let tint = if bag_centroid_colors.is_empty() {
                    None
                } else {
                    polygon.first().and_then(|ring| {
                        let c = ring_centroid(ring);
                        let (qx, qy) = ((c.0 / 6.0).round() as i32, (c.1 / 6.0).round() as i32);
                        let mut found = None;
                        'search: for dy in -1..=1 {
                            for dx in -1..=1 {
                                if let Some(color) =
                                    bag_centroid_colors.get(&(qx + dx, qy + dy))
                                {
                                    found = Some(*color);
                                    break 'search;
                                }
                            }
                        }
                        found
                    })
                };
                let height_m = if group.height_m.is_finite() {
                    group.height_m.max(0.0)
                } else {
                    8.0
                };
                let base_m = if group.min_height_m.is_finite() {
                    group.min_height_m.clamp(0.0, height_m)
                } else {
                    0.0
                };
                building_jobs.push(BuildingJob {
                    polygon,
                    height_m,
                    base_m,
                    tint,
                    min_y,
                });
            }
        }
        building_jobs.sort_by(|a, b| {
            a.min_y
                .partial_cmp(&b.min_y)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        profiler.lap("b-jobs", &format!("jobs={}", building_jobs.len()));
        let building_units_per_m = {
            let n = (1u32 << tile_key.z) as f64;
            let lat = (std::f64::consts::PI * (1.0 - 2.0 * (tile_key.y as f64 + 0.5) / n))
                .sinh()
                .atan();
            (crate::map::geometry::TILE_SIZE * n / (40_075_016.686 * lat.cos())) as f32
        };
        // The one SceneSun also drives the legacy baked-shadow projection.
        let sun_2d = theme.shiny.sun.dir_2d();
        let (light_x, light_y) = (sun_2d.x, sun_2d.y);
        // v4 block dissolve: same-height touching buildings union at BAKE
        // time so shared interior walls never reach the extruder. Runtime
        // pays ZERO booleans — a signature HIT swaps the eligible jobs for
        // the baked union groups; a MISS keeps the per-building path.
        {
            // Deterministic dissolve-eligibility, identical at bake and
            // runtime (both sides hash only eligible jobs): grounded, and
            // NOT part of a monster same-height set — the Westland
            // greenhouse belt puts thousands of identical-height rings in
            // one tile and the union blowup got the bake jetsam-killed.
            let mut key_jobs: std::collections::HashMap<(i32, u32), (u32, u64)> =
                std::collections::HashMap::new();
            for job in building_jobs.iter().filter(|job| job.base_m.abs() < 0.01) {
                let key = (
                    (job.height_m * 2.0).round() as i32,
                    job.tint.map_or(0, |t| t | 0x8000_0000),
                );
                let points: u64 = job.polygon.iter().map(|r| r.len() as u64).sum();
                let entry = key_jobs.entry(key).or_default();
                entry.0 += 1;
                entry.1 += points;
            }
            let group_ok = |job: &BuildingJob| {
                let key = (
                    (job.height_m * 2.0).round() as i32,
                    job.tint.map_or(0, |t| t | 0x8000_0000),
                );
                key_jobs
                    .get(&key)
                    .is_some_and(|&(count, points)| count <= 800 && points <= 120_000)
            };
            let eligible = |job: &BuildingJob| job.base_m.abs() < 0.01 && group_ok(job);
            let building_sig = {
                use std::hash::Hasher;
                let mut h = FnvStdHasher(0x9e37_79b9_97f4_a7c5);
                for job in building_jobs.iter().filter(|job| eligible(job)) {
                    h.write(&job.height_m.to_bits().to_le_bytes());
                    h.write(&job.tint.unwrap_or(0).to_le_bytes());
                    h.write(&(job.polygon.len() as u32).to_le_bytes());
                    for ring in &job.polygon {
                        h.write(&(ring.len() as u32).to_le_bytes());
                        for &(x, y) in ring {
                            h.write(&x.to_bits().to_le_bytes());
                            h.write(&y.to_bits().to_le_bytes());
                        }
                    }
                }
                h.0
            };
            if faces_bake_sink_armed() {
                use i_overlay::core::fill_rule::FillRule as IoFillRule;
                use i_overlay::core::overlay_rule::OverlayRule;
                use i_overlay::float::simplify::SimplifyShape;
                use i_overlay::float::single::SingleFloatOverlay;
                use std::collections::BTreeMap;
                let mut by_key: BTreeMap<(i32, u32), Vec<Vec<[f64; 2]>>> = BTreeMap::new();
                for job in building_jobs.iter().filter(|job| eligible(job)) {
                    let key = (
                        (job.height_m * 2.0).round() as i32,
                        job.tint.map_or(0, |t| t | 0x8000_0000),
                    );
                    by_key.entry(key).or_default().extend(
                        job.polygon.iter().map(|ring| {
                            ring.iter()
                                .map(|&(x, y)| [x as f64, y as f64])
                                .collect::<Vec<_>>()
                        }),
                    );
                }
                captured_building_sig = building_sig;
                for ((height_q, tint), rings) in by_key {
                    const CHUNK: usize = 3000;
                    let mut acc: Vec<Vec<Vec<[f64; 2]>>> = Vec::new();
                    for chunk in rings.chunks(CHUNK) {
                        let part = chunk.to_vec().simplify_shape(IoFillRule::NonZero);
                        if acc.is_empty() {
                            acc = part;
                        } else {
                            let part_paths: Vec<Vec<[f64; 2]>> = part
                                .iter()
                                .flat_map(|shape| shape.iter().cloned())
                                .collect();
                            acc = part_paths.overlay(
                                &acc,
                                OverlayRule::Union,
                                IoFillRule::NonZero,
                            );
                        }
                    }
                    // ONE group per connected shape: a group's ring set is
                    // one polygon-with-holes for the roof earcut — merging
                    // separate blocks into one ring list turns disjoint
                    // outers into phantom holes. Winding normalizes to the
                    // extruder's y-down convention (outer signed area > 0,
                    // holes < 0) — i_overlay's output orientation differs.
                    for shape in acc {
                        let mut rings: Vec<Vec<[f64; 2]>> = Vec::with_capacity(shape.len());
                        for (ring_index, mut ring) in shape.into_iter().enumerate() {
                            // Same shoelace form as polygon_signed_area so
                            // the outer test matches the extruder exactly.
                            let mut area = 0.0f64;
                            for i in 0..ring.len() {
                                let j = (i + 1) % ring.len();
                                area += ring[i][0] * ring[j][1] - ring[j][0] * ring[i][1];
                            }
                            let outer = ring_index == 0;
                            if (outer && area < 0.0) || (!outer && area > 0.0) {
                                ring.reverse();
                            }
                            rings.push(ring);
                        }
                        if !rings.is_empty() {
                            captured_building_groups.push(BakedBuildingGroup {
                                height_m: height_q as f32 / 2.0,
                                tint,
                                rings,
                            });
                        }
                    }
                }
            } else if let Some(bake) = baked_faces
                .as_ref()
                .filter(|bake| {
                    bake.bucket == render_zoom
                        && bake.building_signature == building_sig
                        && !bake.buildings.is_empty()
                })
            {
                let dissolved: Vec<BuildingJob> = bake
                    .buildings
                    .iter()
                    .map(|group| {
                        let polygon: Vec<Vec<(f32, f32)>> = group
                            .rings
                            .iter()
                            .map(|ring| {
                                ring.iter().map(|p| (p[0] as f32, p[1] as f32)).collect()
                            })
                            .collect();
                        let min_y = polygon
                            .iter()
                            .flat_map(|ring| ring.iter().map(|p| p.1))
                            .fold(f32::MAX, f32::min);
                        BuildingJob {
                            polygon,
                            height_m: group.height_m,
                            base_m: 0.0,
                            tint: (group.tint != 0).then(|| group.tint & 0x7fff_ffff),
                            min_y,
                        }
                    })
                    .collect();
                building_jobs.retain(|job| !eligible(job));
                building_jobs.extend(dissolved);
                // Preserve the north->south paint order the pre-dissolve
                // sort established (walls paint over northern neighbors).
                building_jobs.sort_by(|a, b| {
                    a.min_y
                        .partial_cmp(&b.min_y)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                profiler.lap("b-dissolved", &format!("groups={}", bake.buildings.len()));
            }
        }
        // The offline sink consumes only the dissolved building groups
        // above. Walls, roofs and their AO are derived again by the renderer
        // and must not be tessellated into throwaway TileBuffers here.
        let derived_building_jobs: &[BuildingJob] = if faces_bake_sink_armed() {
            &[]
        } else {
            &building_jobs
        };
        let base_color = theme.building_fill_color().unwrap_or(0xd9d0c9);
        // The one SceneSun: walls shade by their outward normal against its
        // horizontal direction (defaults reproduce the legacy NW sun).
        let sun_2d = theme.shiny.sun.dir_2d();
        let (light_x, light_y) = (sun_2d.x, sun_2d.y);
        // T2 vertical AO: ground-contact vertices darken so buildings sit
        // in the scene instead of floating. Sections starting above ground
        // (bridge decks, tower setbacks) fade the effect out.
        let wall_ao = |base_m: f32| -> f32 {
            if theme.shiny.bake_ao {
                0.75 + 0.25 * (base_m / 8.0).clamp(0.0, 1.0)
            } else {
                1.0
            }
        };
        // Wall LOD: per-edge quads dominate 3D tile size (30-56 MB observed
        // on dense tiles) while sub-pixel footprint detail cannot show in a
        // wall silhouette. Collapse wall edges under ~1.2 screen px and drop
        // wall rings for courtyards under ~5 px; roofs keep full detail.
        let wall_min_edge = 1.2 / render_scale;
        let wall_min_hole_extent = 5.0 / render_scale;
        for job in derived_building_jobs {
            // Building-age layer tints the 3D model itself (walls shade
            // from the same hue via the normal lighting math).
            let roof_color = hex_to_premul_rgba(job.tint.unwrap_or(base_color), 1.0);
            if job.height_m <= 0.05 {
                // Flattened outline: footprint fill only, no walls.
            } else {
            for source_ring in &job.polygon {
                // Outward normal needs ring orientation; positive shoelace
                // in y-down tile space = exterior winding, holes come
                // opposite so their normals flip into the courtyard.
                let clockwise = polygon_signed_area(source_ring) > 0.0;
                if !clockwise {
                    let mut min_x = f32::MAX;
                    let mut min_y = f32::MAX;
                    let mut max_x = f32::MIN;
                    let mut max_y = f32::MIN;
                    for &(x, y) in source_ring {
                        min_x = min_x.min(x);
                        min_y = min_y.min(y);
                        max_x = max_x.max(x);
                        max_y = max_y.max(y);
                    }
                    if (max_x - min_x).max(max_y - min_y) < wall_min_hole_extent {
                        continue;
                    }
                }
                let wall_ring = simplify_wall_ring(source_ring, wall_min_edge);
                if wall_ring.len() < 3 {
                    continue;
                }
                let ring = &wall_ring;
                let n = ring.len();
                // South-most edges last so they paint over northern walls.
                let mut edge_order: Vec<usize> = (0..n).collect();
                edge_order.sort_by(|&i, &j| {
                    let yi = ring[i].1 + ring[(i + 1) % n].1;
                    let yj = ring[j].1 + ring[(j + 1) % n].1;
                    yi.partial_cmp(&yj).unwrap_or(std::cmp::Ordering::Equal)
                });
                for &i in &edge_order {
                    let a = ring[i];
                    let b = ring[(i + 1) % n];
                    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                    let len = (dx * dx + dy * dy).sqrt();
                    if len < 1e-4 {
                        continue;
                    }
                    let (mut nx, mut ny) = (dy / len, -dx / len);
                    if !clockwise {
                        nx = -nx;
                        ny = -ny;
                    }
                    let facing = (nx * light_x + ny * light_y).clamp(-1.0, 1.0);
                    let shade = 0.62 + 0.20 * (facing + 1.0);
                    let wall_color = [
                        roof_color[0] * shade,
                        roof_color[1] * shade,
                        roof_color[2] * shade,
                        1.0,
                    ];
                    push_wall_instance(
                        &mut wall_instances,
                        a,
                        b,
                        job.base_m,
                        job.height_m,
                        wall_color,
                        wall_ao(job.base_m),
                        (nx, ny),
                        &mut fill_zbias,
                    );
                }
            }
            }
            for ring in &job.polygon {
                emit_path(&mut path, ring, true);
            }
            // No AA fringe on roofs: the fringe doubles the vertex count
            // across thousands of buildings, and a lifted roof edge meets
            // its own wall, not the background it would blend against.
            tessellate_path_fill(
                &mut path,
                &mut tess,
                &mut tess_verts,
                &mut tess_indices,
                LineJoin::Miter,
                4.0,
                0.0,
                false,
                tolerance,
            );
            append_tessellated_geometry(
                &tess_verts,
                &tess_indices,
                &mut fill_vertices,
                &mut fill_indices,
                VectorRenderParams {
                    color: roof_color,
                    stroke_mult: 1e6,
                    shape_id: 0.0,
                    params: [0.0, 0.0, 0.0, MAT_ROOF, job.height_m, BUILDING_SURFACE_DEPTH],
                    zbias: fill_zbias,
                },
            );
            fill_zbias += VECTOR_ZBIAS_STEP;
            // T2 roof-edge AO: parapet gradient strip along the outline.
            if theme.shiny.bake_ao && job.height_m > 0.05 {
                for ring in &job.polygon {
                    append_roof_edge_ao(
                        ring,
                        job.height_m,
                        roof_color,
                        1.5 * building_units_per_m,
                        &mut fill_vertices,
                        &mut fill_indices,
                        &mut fill_zbias,
                    );
                }
            }
            feature_count += 1;
        }
    }

    // Little 3D trees (tilt mode): two crossed trunk quads (visible from
    // any camera heading) + two stacked canopy discs lifted by the same
    // per-meter height mechanism as building roofs — the tilt compression
    // turns them into oval blobs.
    if !tree_points_3d.is_empty() {
        let n = (1u32 << tile_key.z) as f64;
        let lat = (std::f64::consts::PI * (1.0 - 2.0 * (tile_key.y as f64 + 0.5) / n))
            .sinh()
            .atan();
        // tile-local units per meter at this latitude
        let units_per_m =
            (crate::map::geometry::TILE_SIZE * n / (40_075_016.686 * lat.cos())) as f32;
        let trunk_color = hex_to_premul_rgba(theme.tree_trunk.unwrap_or(0x8a6b4a), 1.0);
        let canopy_color = hex_to_premul_rgba(theme.tree_canopy.unwrap_or(0x4a7d44), 1.0);
        let arm = 0.7 * units_per_m;
        // Canopy LOD by screen size: park tiles carry thousands of trees
        // and a 16x8 ball per ~14 px canopy was the single largest buffer
        // in dense tiles.
        let canopy_px = 2.9 * units_per_m * render_scale;
        let (canopy_segs_u, canopy_segs_v) = if canopy_px >= 24.0 {
            (12, 6)
        } else if canopy_px >= 12.0 {
            (8, 4)
        } else {
            (6, 3)
        };

        let trunk_ao = if theme.shiny.bake_ao { 0.78 } else { 1.0 };
        // T3 tree contact shadows: a soft dark disc under each canopy,
        // nudged along the shadow direction — "the tree stands on the
        // ground" for a dozen vertices per tree.
        if theme.shiny.bake_shadows {
            let sun_2d = theme.shiny.sun.dir_2d();
            for (index, (x, y)) in tree_points_3d.iter().enumerate() {
                let center = (
                    *x - sun_2d.x * 2.2 * units_per_m,
                    *y - sun_2d.y * 2.2 * units_per_m,
                );
                // Full-strength center (the live shadow uniform is the
                // brightness knob), canopy-sized, offset like a canopy
                // hanging 7.5-11 m up would cast. The per-disc depth step
                // keeps overlapping discs (tree rows) from z-fighting each
                // other or the building shadow union underneath.
                append_ground_shadow_disc(
                    center,
                    3.4 * units_per_m,
                    1.0,
                    SHADOW_DECAL_DEPTH + 0.005 + (index % 8) as f32 * 5e-4,
                    &mut icon_vertices,
                    &mut icon_indices,
                    &mut icon_zbias,
                );
            }
        }
        // Every street tree is the same mesh (shading depends on normals
        // and the sun, not the anchor): build ONE tree at the origin and
        // instance it by memcpy + anchor/zbias patch. Dense park tiles at
        // the icon horizon carry thousands — the per-tree sin/cos rebuild
        // was most of the buildings stage there.
        if !tree_points_3d.is_empty() && !faces_bake_sink_armed() {
            let mut template_verts = Vec::<f32>::new();
            let mut template_indices = Vec::<u32>::new();
            let mut template_zbias = 0.0f32;
            append_wall_quad(
                (-arm, 0.0),
                (arm, 0.0),
                0.0,
                7.5,
                trunk_color,
                trunk_ao,
                (0.0, 0.0),
                MAT_NONE,
                &mut template_verts,
                &mut template_indices,
                &mut template_zbias,
            );
            append_wall_quad(
                (0.0, -arm),
                (0.0, arm),
                0.0,
                7.5,
                trunk_color,
                trunk_ao,
                (0.0, 0.0),
                MAT_NONE,
                &mut template_verts,
                &mut template_indices,
                &mut template_zbias,
            );
            // Street-tree proportions vs buildings: ~11.5m total. The
            // canopy is a PROLATE ellipsoid (taller than wide) on a tall
            // trunk — scaling the ball uniformly reads as a bush.
            append_ball(
                (0.0, 0.0),
                2.9 * units_per_m,
                4.0,
                7.5,
                canopy_color,
                canopy_segs_u,
                canopy_segs_v,
                &theme.shiny.sun,
                MAT_CANOPY,
                &mut template_verts,
                &mut template_indices,
                &mut template_zbias,
            );
            // Mid/far-ring stand-in: the trunk plus two crossed vertical
            // quads (canopy color, canopy material) — 8 verts vs ~70. Both
            // templates sit at the origin; the GPU adds the anchor per
            // instance (TREE_INSTANCE_FLOATS), so a park tile carries one
            // tree mesh, not thousands.
            let cross_r = 2.6 * units_per_m;
            let mut cross_zbias = 0.0f32;
            for (a, b, normal) in [
                ((-arm, 0.0), (arm, 0.0), (0.0, 0.0)),
                ((0.0, -arm), (0.0, arm), (0.0, 0.0)),
            ] {
                append_wall_quad(
                    a,
                    b,
                    0.0,
                    7.5,
                    trunk_color,
                    trunk_ao,
                    normal,
                    MAT_NONE,
                    &mut tree_cross_template_vertices,
                    &mut tree_cross_template_indices,
                    &mut cross_zbias,
                );
            }
            for (a, b, normal) in [
                ((-cross_r, 0.0), (cross_r, 0.0), (0.0, 1.0)),
                ((0.0, -cross_r), (0.0, cross_r), (1.0, 0.0)),
            ] {
                append_wall_quad(
                    a,
                    b,
                    1.5,
                    11.0,
                    canopy_color,
                    trunk_ao,
                    normal,
                    MAT_CANOPY,
                    &mut tree_cross_template_vertices,
                    &mut tree_cross_template_indices,
                    &mut cross_zbias,
                );
            }
            let template_step = template_zbias.max(cross_zbias);
            tree_instances.reserve(tree_points_3d.len() * TREE_INSTANCE_FLOATS);
            for (instance, (x, y)) in tree_points_3d.iter().enumerate() {
                tree_instances.extend_from_slice(&[
                    *x,
                    *y,
                    fill_zbias + instance as f32 * template_step,
                ]);
                feature_count += 1;
            }
            fill_zbias += tree_points_3d.len() as f32 * template_step;
            tree_template_vertices = template_verts;
            tree_template_indices = template_indices;
        }
    }

    // Dynamic stalk heights: every flying marker clears the building under
    // it by ~8 m (a 100 m tower gets a 108 m pin), plus a small
    // deterministic stagger so clustered pins don't form one flat plane.
    // Cell grid over building rings: the stalk-clearance scan was
    // icons x groups x rings, and the icon horizon multiplied the icon
    // side by ~10 (75ms on center tiles). A point query now touches one
    // cell's candidates.
    let lift_grid: CellMap<Vec<u32>> = if faces_bake_sink_armed() {
        CellMap::default()
    } else {
        const LIFT_CELL: f32 = 24.0;
        let mut grid: CellMap<Vec<u32>> = CellMap::default();
        for (group_index, group) in building_groups.iter().enumerate() {
            if group.height_m <= 0.0 {
                continue;
            }
            for ring in &group.rings {
                if ring.signed_area <= 0.0 {
                    continue;
                }
                let mut min_x = f32::MAX;
                let mut min_y = f32::MAX;
                let mut max_x = f32::MIN;
                let mut max_y = f32::MIN;
                for &(x, y) in &ring.points {
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
                for cy in (min_y / LIFT_CELL).floor() as i32..=(max_y / LIFT_CELL).floor() as i32
                {
                    for cx in
                        (min_x / LIFT_CELL).floor() as i32..=(max_x / LIFT_CELL).floor() as i32
                    {
                        let cell = grid.entry((cx, cy)).or_default();
                        if cell.last() != Some(&(group_index as u32)) {
                            cell.push(group_index as u32);
                        }
                    }
                }
            }
        }
        grid
    };
    let job_lifts: Vec<f32> = icon_jobs
        .iter()
        .map(|job| {
            let base = job.9;
            if base <= 0.0 {
                return 0.0;
            }
            let (px, py) = job.0;
            let mut clearance = 0.0f32;
            const LIFT_CELL: f32 = 24.0;
            let key = ((px / LIFT_CELL).floor() as i32, (py / LIFT_CELL).floor() as i32);
            if let Some(candidates) = lift_grid.get(&key) {
                for &group_index in candidates {
                    let group = &building_groups[group_index as usize];
                    if group.height_m <= clearance {
                        continue;
                    }
                    for ring in &group.rings {
                        if ring.signed_area <= 0.0 {
                            continue;
                        }
                        if point_in_ring((px, py), &ring.points) {
                            clearance = clearance.max(group.height_m);
                            break;
                        }
                    }
                }
            }
            base.max(clearance + 8.0)
        })
        .collect();
    // Propagate the FINAL lifts into the labels and tap zones that belong
    // to these markers, so text and hit-testing ride the same stalk.
    for (job, lift) in icon_jobs.iter().zip(job_lifts.iter()) {
        if *lift <= 0.0 {
            continue;
        }
        let (jx, jy) = job.0;
        for label in labels.iter_mut() {
            let eligible = label.color_class == crate::map::label::LABEL_CLASS_PIN
                || label.road_kind.starts_with("chb")
                || label.road_kind.starts_with("poi")
                || label.road_kind.starts_with("stS")
                || label.road_kind.starts_with("stp");
            if !eligible || label.path_points.is_empty() {
                continue;
            }
            let (lx, ly) = label.path_points[0];
            let (mx, my) = label
                .path_points
                .last()
                .map(|p| ((lx + p.0) * 0.5, (ly + p.1) * 0.5))
                .unwrap_or((lx, ly));
            if (mx - jx).abs() < 2.5 && (my - jy).abs() < 2.5 {
                label.lift_m = *lift;
            }
        }
        let world = (1u32 << tile_key.z) as f64;
        let jnorm = (
            (tile_key.x as f64 + jx as f64 / crate::map::geometry::TILE_SIZE) / world,
            (tile_key.y as f64 + jy as f64 / crate::map::geometry::TILE_SIZE) / world,
        );
        for hit in pin_hits.iter_mut() {
            if (hit.norm.0 - jnorm.0).abs() < 1e-7 && (hit.norm.1 - jnorm.1).abs() < 1e-7 {
                hit.lift_m = *lift;
            }
        }
    }
    // Marker stalks (3D mode): thin dark lines from the ground point up to
    // every floating marker.
    if buildings_3d {
        let has_pins = icon_jobs.iter().any(|job| job.9 > 0.0);
        if has_pins {
            let n = (1u32 << tile_key.z) as f64;
            let lat = (std::f64::consts::PI * (1.0 - 2.0 * (tile_key.y as f64 + 0.5) / n))
                .sinh()
                .atan();
            let units_per_m =
                (crate::map::geometry::TILE_SIZE * n / (40_075_016.686 * lat.cos())) as f32;
            let stalk_color = hex_to_premul_rgba(0x4a5058, 1.0);
            for (job_index, job) in icon_jobs.iter().enumerate() {
                let lift = job_lifts[job_index];
                if lift <= 0.0 {
                    continue;
                }
                // Chargers get a slightly heavier stalk than POI markers.
                let arm = if job.5 == 2 || job.5 == 3 { 0.22 } else { 0.14 } * units_per_m;
                let (x, y) = job.0;
                append_wall_quad(
                    (x - arm, y),
                    (x + arm, y),
                    0.0,
                    lift,
                    stalk_color,
                    1.0,
                    (0.0, 0.0),
                    MAT_NONE,
                    &mut fill_vertices,
                    &mut fill_indices,
                    &mut fill_zbias,
                );
                append_wall_quad(
                    (x, y - arm),
                    (x, y + arm),
                    0.0,
                    lift,
                    stalk_color,
                    1.0,
                    (0.0, 0.0),
                    MAT_NONE,
                    &mut fill_vertices,
                    &mut fill_indices,
                    &mut fill_zbias,
                );
            }
        }
    }

    // Little 3D stoplights (tilt mode): a slim dark pole with the classic
    // three lights stacked on top — red above amber above green.
    if !signal_points_3d.is_empty() {
        let n = (1u32 << tile_key.z) as f64;
        let lat = (std::f64::consts::PI * (1.0 - 2.0 * (tile_key.y as f64 + 0.5) / n))
            .sinh()
            .atan();
        let units_per_m =
            (crate::map::geometry::TILE_SIZE * n / (40_075_016.686 * lat.cos())) as f32;
        let pole_color = hex_to_premul_rgba(0x3c4046, 1.0);
        let lights = [
            (hex_to_premul_rgba(0x2ecc40, 1.0), 3.5f32),
            (hex_to_premul_rgba(0xf5a623, 1.0), 4.35),
            (hex_to_premul_rgba(0xd7263d, 1.0), 5.2),
        ];
        let arm = 0.32 * units_per_m;
        // T3: soft contact shadow under each stoplight pole.
        if theme.shiny.bake_shadows {
            let sun_2d = theme.shiny.sun.dir_2d();
            for (index, (x, y)) in signal_points_3d.iter().enumerate() {
                let center = (
                    *x - sun_2d.x * 0.9 * units_per_m,
                    *y - sun_2d.y * 0.9 * units_per_m,
                );
                append_ground_shadow_disc(
                    center,
                    1.3 * units_per_m,
                    0.9,
                    SHADOW_DECAL_DEPTH + 0.005 + (index % 8) as f32 * 5e-4,
                    &mut icon_vertices,
                    &mut icon_indices,
                    &mut icon_zbias,
                );
            }
        }
        for (x, y) in &signal_points_3d {
            append_wall_quad(
                (*x - arm, *y),
                (*x + arm, *y),
                0.0,
                3.2,
                pole_color,
                1.0,
                (0.0, 0.0),
                MAT_NONE,
                &mut fill_vertices,
                &mut fill_indices,
                &mut fill_zbias,
            );
            append_wall_quad(
                (*x, *y - arm),
                (*x, *y + arm),
                0.0,
                3.2,
                pole_color,
                1.0,
                (0.0, 0.0),
                MAT_NONE,
                &mut fill_vertices,
                &mut fill_indices,
                &mut fill_zbias,
            );
            for (color, height_m) in lights {
                append_ball(
                    (*x, *y),
                    0.5 * units_per_m,
                    0.5,
                    height_m,
                    color,
                    8,
                    4,
                    &theme.shiny.sun,
                    MAT_NONE,
                    &mut fill_vertices,
                    &mut fill_indices,
                    &mut fill_zbias,
                );
            }
            feature_count += 1;
        }
    }

    // Stroke pass
    let mut stroke_jobs = Vec::<StrokeDrawJob>::new();
    let mut arrow_jobs = Vec::<ArrowDrawJob>::new();
    for prepared_way in &prepared {
        let way = &tile_ways[prepared_way.way_index];
        if !faces_bake_sink_armed() {
            if let Some(label) = extract_way_label(&way.tags, &prepared_way.points) {
                labels.push(label);
            }
        }
        // Detail building footprints are a mode-specific fill overlay. A
        // handful inherit highway-like OSM tags; letting those fall through
        // stroke styling made the supposedly stable road core differ between
        // flat and tilted bakes and could add spurious road slivers.
        if way.tags.get("layer").map(String::as_str) == Some("detail_buildings") {
            continue;
        }
        // Road labels are mode-independent but are cheap to extract. Keep
        // them in the replacement label set, then reuse the resident GPU
        // road core instead of rebuilding stroke jobs and Boolean surfaces.
        if !build_road_core {
            continue;
        }
        // Oneway arrows read from mid zoom, not just street level —
        // direction matters while route-planning zoomed out. NOT tied to
        // stroke style presence: at close zooms wide roads render as
        // street polygons and their centerline stroke style is None, but
        // the direction arrows must survive.
        let implicit_oneway = matches!(
            way.tags.get("junction").map(|v| v.as_str()),
            Some("roundabout") | Some("circular")
        );
        let arrow_reverse = (render_zoom >= 15
            && (tag_is_truthy(&way.tags, "oneway") || implicit_oneway)
            && way.tags.contains_key("highway")
            && !tag_is_truthy(&way.tags, "rail"))
        .then(|| tag_is_truthy(&way.tags, "oneway_reverse"));
        let mut arrow_surface_depth = ArrowSurfaceDepth::Unknown;
        // A tunnel stays a cartographic dashed/ghosted stroke in both 2D
        // and 3D. Promoting its solved negative profile to an exposed solid
        // mesh made the portal a literal cutaway: independently clipped
        // sunk/surface faces opened triangular holes when pitched. The
        // fixed under-surface depth still gives the tunnel correct
        // occlusion beneath streets and plazas without pretending that its
        // underground deck is visible terrain.
        let style_tags = &way.tags;
        if let Some(mut style) =
            stroke_style_for_tags(theme, style_tags, tile_key.z, render_zoom, zoom_mult, px_to_units)
        {
            // Inside baked bridge-dz coverage the solved corridor profile
            // is the only lift source — shortbread bridge tags are the
            // coarse signal that lifted whole merged runs.
            if bridge_dz_covered {
                style.center.deck_m = 0.0;
                if let Some(casing) = style.casing.as_mut() {
                    casing.deck_m = 0.0;
                }
            }
            // Tunnels must never ride a corridor deck: a tunnel tube
            // running parallel to a bridge (IJtunnel next to Zouthavenbrug)
            // passes the direction gate and would hoist above ground.
            // deck_m < 0 is the "never deck" sentinel for the stroke pass.
            if tag_is_truthy(&way.tags, "tunnel") {
                style.center.deck_m = -1.0;
                if let Some(casing) = style.casing.as_mut() {
                    casing.deck_m = -1.0;
                }
            }
            if let Some(dots) = thin_bridge_dots_for_tags(
                theme,
                &way.tags,
                render_zoom,
                zoom_mult,
                px_to_units,
            ) {
                stroke_jobs.push(StrokeDrawJob {
                    sort_rank: dots.sort_rank,
                    style: dots,
                    points: prepared_way.points.clone(),
                    solid_road_surface: false,
                    dz: None,
                    surface_key: None,
                    join_meta: RoadJoinMeta::default(),
                });
            }
            // Solid road geometry joins the per-tier union mesh: one
            // seamless surface per class, identical flat and tilted. Dashed
            // shapes (rails, tunnels' dash patterns) keep the stroke path.
            let solid_road_surface = is_solid_road_surface(&way.tags, &style);
            let surface_key = solid_road_surface.then(|| {
                RoadSurfaceKey::from_way(style, &way.tags, way.dz.as_deref())
            });
            if arrow_reverse.is_some() {
                arrow_surface_depth = if let Some(surface_key) = surface_key {
                    ArrowSurfaceDepth::Union(surface_key)
                } else {
                    ArrowSurfaceDepth::Stroke {
                        level: if style.center.deck_m < 0.0 { -1 } else { 0 },
                        depth_micro: style.center.depth_micro,
                    }
                };
            }
            stroke_jobs.push(StrokeDrawJob {
                sort_rank: style.sort_rank,
                style,
                points: prepared_way.points.clone(),
                solid_road_surface,
                dz: if solid_road_surface {
                    way.dz.clone()
                } else {
                    None
                },
                surface_key,
                join_meta: RoadJoinMeta::from_tags(&way.tags),
            });
        }
        if let Some(reverse) = arrow_reverse {
            arrow_jobs.push(ArrowDrawJob {
                points: prepared_way.points.clone(),
                reverse,
                dz: way
                    .dz
                    .as_ref()
                    .filter(|dz| dz.len() == prepared_way.points.len())
                    .cloned(),
                surface_depth: arrow_surface_depth,
            });
        }
    }

    let fill_3d_vertices = fill_vertices.split_off(fill_3d_vert_start);
    let mut fill_3d_indices = fill_indices.split_off(fill_3d_index_start);
    let fill_3d_base = (fill_3d_vert_start / VECTOR_FLOATS_PER_VERTEX) as u32;
    for index in fill_3d_indices.iter_mut() {
        *index -= fill_3d_base;
    }
    profiler.lap("buildings", &format!("fill={}KB", fill_3d_vertices.len() * 4 / 1024));

    let mut union_tiers =
        HashMap::<RoadSurfaceKey, (StrokeStyle, Vec<(Vec<(f32, f32)>, Option<Vec<f32>>)>)>::new();
    let mut union_way_meta = HashMap::<RoadSurfaceKey, Vec<RoadJoinMeta>>::new();
    let mut grouped_strokes = HashMap::<StrokeStyleKey, (StrokeStyle, Vec<Vec<(f32, f32)>>)>::new();
    for job in stroke_jobs {
        if job.solid_road_surface {
            let key = job
                .surface_key
                .expect("solid-road job must carry physical surface identity");
            let entry = union_tiers.entry(key).or_insert((job.style, Vec::new()));
            entry.1.push((job.points, job.dz));
            union_way_meta.entry(key).or_default().push(job.join_meta);
            continue;
        }
        let key = StrokeStyleKey::from(job.style);
        let entry = grouped_strokes.entry(key).or_insert((job.style, Vec::new()));
        entry.1.push(job.points);
    }

    let mut merged_stroke_jobs = Vec::<StrokeDrawJob>::new();
    for (_key, (style, polylines)) in grouped_strokes {
        for points in merge_stroke_polylines(&polylines) {
            merged_stroke_jobs.push(StrokeDrawJob {
                sort_rank: style.sort_rank,
                style,
                points,
                solid_road_surface: false,
                dz: None,
                surface_key: None,
                join_meta: RoadJoinMeta::default(),
            });
        }
    }

    // Deterministic paint order: rank, then style bits (HashMap iteration
    // order must not leak into the render).
    merged_stroke_jobs.sort_unstable_by_key(|job| {
        (
            job.sort_rank,
            job.style.center.color,
            job.style.center.width.to_bits(),
        )
    });
    let clip_bounds = tile_clip_bounds(ROAD_PAINT_CLIP_PADDING);
    // Overzoomed tiles magnify the source tile's coordinate quantization
    // into visibly angular curves (ovals read as polygons at 8-16x). A
    // round or two of Chaikin corner-cutting restores the curvature.
    let chaikin_rounds = if render_scale >= 8.0 {
        2
    } else if render_scale >= 3.0 {
        1
    } else {
        0
    };
    // Only cut where segments are shorter than ~10 screen px — dense
    // quantized curves qualify, real street corners never do.
    let chaikin_cut_below = 10.0 / render_scale;
    let mut merged_stroke_parts = Vec::<(StrokeStyle, Vec<Vec<(f32, f32)>>)>::new();
    for job in merged_stroke_jobs {
        let smooth = chaikin_smooth(&job.points, chaikin_rounds, chaikin_cut_below);
        let parts = build_polyline_parts(&smooth, clip_bounds, false, ROAD_SMOOTH_FACTOR);
        merged_stroke_parts.push((job.style, parts));
    }
    profiler.lap("sp-merge", "");

    // Painter interleave vs the road faces: patterned/non-surface strokes
    // whose rank is
    // below the topmost road tier paint UNDER the faces (cycleway dashes,
    // park paths — covered by roads at crossings in the reference); only
    // higher ranks (trams, rails) stay above.
    let max_tier_rank = union_tiers
        .values()
        .map(|(style, _)| style.sort_rank)
        .max()
        .unwrap_or(i16::MIN);

    // Repair endpoint-to-interior merges BEFORE smoothing, unioning, and
    // DzField construction so every later consumer sees one continuous
    // deck profile. MVT feature boundaries often put a slip-road endpoint
    // against the middle of its mainline's segment rather than at a shared
    // node, so the exact-node joint pass below cannot discover this case.
    let mut join_ways: Vec<RoadTierJoinWay> = union_tiers
        .iter()
        .flat_map(|(key, (style, ways))| {
            let half_width = style
                .casing
                .map_or(style.center.width, |casing| {
                    casing.width.max(style.center.width)
                })
                * 0.5;
            let metas = union_way_meta.get(key);
            ways
                .iter()
                .enumerate()
                .map(move |(way_index, (points, dz))| {
                    let dz = dz
                        .as_ref()
                        .filter(|values| values.len() == points.len())
                        .cloned()
                        .unwrap_or_else(|| vec![0.0; points.len()]);
                    RoadTierJoinWay {
                        key: *key,
                        way_index,
                        points: points.clone(),
                        dz,
                        half_width,
                        meta: metas
                            .and_then(|metas| metas.get(way_index))
                            .copied()
                            .unwrap_or_default(),
                    }
                })
        })
        .collect();
    profiler.lap("sp-joinways", "");
    let mut endpoint_grade_corrections = endpoint_to_through_grade_corrections(&join_ways);
    profiler.lap("sp-through", &format!("ways={}", join_ways.len()));
    let interior_fascia_ends: std::collections::HashSet<RoadTierEnd> =
        endpoint_grade_corrections
            .iter()
            .map(|correction| correction.end)
            .collect();
    let continuation_corrections = endpoint_continuation_grade_corrections(&join_ways);
    profiler.lap("sp-continuation", "");
    for correction in continuation_corrections {
        if let Some(existing) = endpoint_grade_corrections
            .iter_mut()
            .find(|existing| existing.end == correction.end)
        {
            existing.target_dz = existing.target_dz.max(correction.target_dz);
        } else {
            endpoint_grade_corrections.push(correction);
        }
    }
    for correction in endpoint_grade_corrections {
        let Some((style, ways)) = union_tiers.get_mut(&correction.end.0) else {
            continue;
        };
        let Some((points, dz)) = ways.get_mut(correction.end.1) else {
            continue;
        };
        if dz.as_ref().is_none_or(|values| values.len() != points.len()) {
            *dz = Some(vec![0.0; points.len()]);
        }
        let half_width = style
            .casing
            .map_or(style.center.width, |casing| {
                casing.width.max(style.center.width)
            })
            * 0.5;
        apply_endpoint_grade_correction(
            points,
            dz.as_mut().unwrap(),
            correction.end.2,
            correction.target_dz,
            half_width,
        );
        // Keep the already-built join snapshot in sync so the flush-joint
        // classifier can consume the corrected profiles without cloning
        // every road geometry a second time.
        if let Some(join_way) = join_ways.iter_mut().find(|way| {
            way.key == correction.end.0 && way.way_index == correction.end.1
        }) {
            apply_endpoint_grade_correction(
                &join_way.points,
                &mut join_way.dz,
                correction.end.2,
                correction.target_dz,
                join_way.half_width,
            );
        }
    }
    profiler.lap("sp-apply", "");
    // Classify flush endpoint-to-interior joins only after all dz
    // corrections have landed; their safety gate requires the final two
    // deck profiles to agree at the contact point.
    let endpoint_through_flush_ends = endpoint_to_through_flush_ends(&join_ways);

    profiler.lap("stroke-prep", "");

    // Road paint ladder: solid-road faces and patterned strokes merge into ONE
    // ordered sequence — plazas, then all casings by rank, then all centers
    // by rank — exactly the reference painter. Patterned strokes no longer sit
    // wholesale under the faces: a park path draws above the plaza it
    // crosses, a cycle lane above the road it rides, and higher road faces
    // still cover both. Flat mode is buffer order; tilt follows the same
    // order through the param5 ladder. Only centers ranked above the top
    // road tier (trams, rails) stay in the stroke buffer above everything.
    let union_clip = clip_bounds;
    let mut tier_list: Vec<(
        RoadSurfaceKey,
        &(StrokeStyle, Vec<(Vec<(f32, f32)>, Option<Vec<f32>>)>),
    )> = union_tiers.iter().map(|(key, entry)| (*key, entry)).collect();
    // Theme-stable paint order: the key's derived Ord leads with sort_rank
    // and never reads resolved colors, so tier order (and with it the baked
    // regions' group indices) is identical across recolor-only themes.
    tier_list.sort_by_key(|(key, _)| *key);
    let mut groups: Vec<PaintGroup> = Vec::new();
    {
        // Road-polygon fills all resolve to ONE theme constant
        // (street_area_fill), so alpha is the only structural discriminant
        // — grouping by it alone is identical to the old (color, alpha)
        // grouping within any single theme AND theme-stable across
        // recolors. The paint color comes from the group's first member.
        let mut plaza_keys: Vec<u32> = plaza_rings
            .iter()
            .map(|(_, alpha, _, _)| alpha.to_bits())
            .collect();
        plaza_keys.sort_unstable();
        plaza_keys.dedup();
        for alpha_bits in plaza_keys {
            let color = plaza_rings
                .iter()
                .find(|(_, a, _, _)| a.to_bits() == alpha_bits)
                .map(|(c, _, _, _)| *c)
                .unwrap_or(0);
            let ribbons: Vec<RoadRibbon> = plaza_rings
                .iter()
                .filter(|(_, a, _, _)| a.to_bits() == alpha_bits)
                .map(|(_, _, points, dz)| RoadRibbon {
                    points,
                    dz: dz.as_deref(),
                    closed_ring: true,
                    start_disc: true,
                    end_disc: true,
                })
                .collect();
            let rings = road_ribbon_rings(&ribbons, 1.0, union_clip);
            groups.push(PaintGroup {
                color: hex_to_premul_rgba(color, f32::from_bits(alpha_bits)),
                emissive: 0.0,
                phase: 0,
                rank: i16::MIN,
                depth_micro: 0.0,
                field: 0,
                skirt_joints: Vec::new(),
                half_width: 1.0,
                rings: rings
                    .into_iter()
                    .map(|(ring, dz)| {
                        let min_dz = dz.iter().copied().fold(f32::MAX, f32::min);
                        let max_dz = dz.iter().copied().fold(0.0f32, f32::max);
                        (ring, if min_dz == f32::MAX { 0.0 } else { min_dz }, max_dz)
                    })
                    .collect(),
            });
        }
    }
    // Tier-transition joints: endpoints where ways from DIFFERENT tier
    // keys end on the same node (bridge/approach style splits — the layer
    // rank bias puts a deck in another tier than its own road). These get
    // flush butt joints: no cap discs, no walls across.
    // (tier key + exact way end, unit direction OUT of the way into the
    // node, endpoint dz) per node. Identity matters at forks: one valid
    // continuation must not turn every branch at that node into a butt end.
    let mut endpoint_tiers: HashMap<(i32, i32), Vec<(RoadTierEnd, (f32, f32), f32)>> =
        HashMap::new();
    // Iterate the SORTED tier list, not the HashMap: per-node candidate
    // order feeds flush-joint pairing and the groups' skirt-joint lists,
    // and raw map order made builds nondeterministic run-to-run (which the
    // per-bucket face bake verification caught).
    for &(key, entry) in tier_list.iter() {
        let ways = &entry.1;
        let key = &key;
        for (way_index, (points, dz)) in ways.iter().enumerate() {
            if points.len() < 2 {
                continue;
            }
            let ends = [
                (true, 0usize, points[0], points[1]),
                (
                    false,
                    points.len() - 1,
                    points[points.len() - 1],
                    points[points.len() - 2],
                ),
            ];
            for (is_start, index, end, inner) in ends {
                let node = ((end.0 * 4.0).round() as i32, (end.1 * 4.0).round() as i32);
                let (dx, dy) = (end.0 - inner.0, end.1 - inner.1);
                let len = (dx * dx + dy * dy).sqrt().max(1e-6);
                let end_dz = dz
                    .as_ref()
                    .and_then(|dz| dz.get(index).copied())
                    .unwrap_or(0.0);
                endpoint_tiers
                    .entry(node)
                    .or_default()
                    .push(((*key, way_index, is_start), (dx / len, dy / len), end_dz));
            }
        }
    }
    // A FLUSH joint is a CONTINUATION AT ONE HEIGHT: two ways of different
    // tiers ending at one node with (near-)opposite directions AND
    // agreeing endpoint dz — a bridge continuing into its approach at the
    // shared level. Where the heights genuinely differ (a deck rising off
    // its grounded continuation) a flush butt would TEAR open; those
    // joints keep their round caps so the overlap hides the step. Forks
    // and T-junctions keep caps as well.
    let mut tier_joint_ends = endpoint_through_flush_ends;
    // Same-height cross-tier endpoints also suppress a round-cap FASCIA at
    // acute slip-road merges, while retaining the top cap itself. Flush,
    // near-opposite continuations additionally become true butt ends.
    let mut fascia_joint_ends = interior_fascia_ends;
    for entries in endpoint_tiers.values() {
        for (index, (end_a, dir_a, dz_a)) in entries.iter().enumerate() {
            for (end_b, dir_b, dz_b) in entries.iter().skip(index + 1) {
                if end_a.0 == end_b.0
                    || !end_a.0.grade_compatible(end_b.0)
                    || (dz_a - dz_b).abs() >= 0.3
                {
                    continue;
                }
                fascia_joint_ends.insert(*end_a);
                fascia_joint_ends.insert(*end_b);
                if dir_a.0 * dir_b.0 + dir_a.1 * dir_b.1 < -0.7 {
                    tier_joint_ends.insert(*end_a);
                    tier_joint_ends.insert(*end_b);
                }
            }
        }
    }

    // Endpoints of tunnel ways: a surface way ending on one of these nodes
    // stops with a BUTT end (its continuation dives into the tunnel).
    let tunnel_portals: std::collections::HashSet<(i32, i32)> = tile_ways
        .iter()
        .filter(|way| !way.closed && tag_is_truthy(&way.tags, "tunnel"))
        .flat_map(|way| {
            [way.points.first(), way.points.last()]
                .into_iter()
                .flatten()
                .map(|&(x, y)| ((x * 4.0).round() as i32, (y * 4.0).round() as i32))
                .collect::<Vec<_>>()
        })
        .collect();

    // Solid-road ways get the same corner-cut smoothing as other strokes so
    // curve shapes stay identical between the two pipelines; dz rides
    // along through the cuts.
    let smoothed_tiers: Vec<(
        RoadSurfaceKey,
        StrokeStyle,
        Vec<(Vec<(f32, f32)>, Option<Vec<f32>>)>,
    )> = tier_list
        .iter()
        .map(|(key, entry)| {
            (
                *key,
                entry.0,
                entry
                    .1
                    .iter()
                    .map(|(points, dz)| {
                        chaikin_smooth_dz(
                            points,
                            dz.as_deref(),
                            chaikin_rounds,
                            chaikin_cut_below,
                        )
                    })
                    .collect(),
            )
        })
        .collect();
    profiler.lap("rf-smooth", "");
    // Input signature over everything ring construction consumes. When it
    // matches the baked bucket, the i_overlay tier ring construction (and
    // its per-ring-vertex field classification) is skipped entirely — the
    // baked regions carry the geometry, and every other group ingredient
    // (styling, fields, skirt joints, shadows) derives from the ways.
    // Bake capture and the dump diagnostic need real rings, so both force
    // the full path.
    let input_sig = paint_input_signature(
        &smoothed_tiers,
        &plaza_rings,
        &tier_joint_ends,
        &tunnel_portals,
        union_clip,
    );
    let baked_input_hit = !faces_bake_sink_armed()
        && !cascade_dump_armed()
        && baked_faces
            .as_ref()
            .is_some_and(|bake| bake.bucket == render_zoom && bake.signature == input_sig);
    // Per-tier deck fields: index 0 is the plaza field, tier i lives at
    // 1 + i. Casing and center faces of one tier share one field, so both
    // displace identically in tilt — no more detached outlines on ramps.
    let mut dz_fields: Vec<Option<DzField>> = Vec::new();
    {
        // Plazas ride their own ring dz AND any road lifting through them
        // (a bridge deck crossing a quay), so the road ways join the field.
        // PER-WAY reach: ring sources span the whole quay slab, but a road
        // only lifts the plaza across its own deck width — one shared wide
        // radius let a 1 m bridge hump raise half of Weesperplein and
        // shear the square over its grounded surroundings.
        let mut plaza_ways: Vec<(&[(f32, f32)], Option<&[f32]>, f32)> = plaza_rings
            .iter()
            .map(|(_, _, points, dz)| (points.as_slice(), dz.as_deref(), 6.0f32))
            .collect();
        for (_, style, ways) in &smoothed_tiers {
            let reach = (style
                .casing
                .map_or(style.center.width, |casing| casing.width.max(style.center.width))
                * 0.5
                + 1.0)
                .max(1.5);
            plaza_ways.extend(
                ways.iter()
                    .map(|(points, dz)| (points.as_slice(), dz.as_deref(), reach)),
            );
        }
        dz_fields.push(DzField::build_with_radii(&plaza_ways, union_clip));
    }
    for (_, style, ways) in &smoothed_tiers {
        let half_width = style
            .casing
            .map_or(style.center.width, |casing| casing.width.max(style.center.width))
            * 0.5;
        let ways_ref: Vec<(&[(f32, f32)], Option<&[f32]>)> = ways
            .iter()
            .map(|(points, dz)| (points.as_slice(), dz.as_deref()))
            .collect();
        dz_fields.push(DzField::build(&ways_ref, half_width + 2.0, union_clip));
    }
    profiler.lap("rf-fields", "");
    for pass in 0..2u8 {
        for (tier_index, (tier_key, style, ways)) in smoothed_tiers.iter().enumerate() {
            let (color, width, depth_micro) = if pass == 0 {
                let Some(casing) = style.casing else { continue };
                (casing.color, casing.width, casing.depth_micro)
            } else {
                (
                    style.center.color,
                    style.center.width,
                    style.center.depth_micro,
                )
            };
            let ribbons: Vec<RoadRibbon> = ways
                .iter()
                .enumerate()
                .map(|(way_index, (points, dz))| {
                    let butt = |point: Option<&(f32, f32)>, is_start: bool| {
                        point.is_some_and(|&(x, y)| {
                            let node = ((x * 4.0).round() as i32, (y * 4.0).round() as i32);
                            road_endpoint_is_clip_cut((x, y), union_clip)
                                || tunnel_portals.contains(&node)
                                || tier_joint_ends.contains(&(*tier_key, way_index, is_start))
                        })
                    };
                    RoadRibbon {
                        points,
                        dz: dz.as_deref(),
                        closed_ring: false,
                        start_disc: !butt(points.first(), true),
                        end_disc: !butt(points.last(), false),
                    }
                })
                .collect();
            let rings = if baked_input_hit {
                Vec::new()
            } else {
                road_ribbon_rings(&ribbons, (width * 0.5).max(0.05), union_clip)
            };
            let skirt_joints: Vec<RoadSkirtJoint> = ways
                .iter()
                .enumerate()
                .flat_map(|(way_index, (points, _))| {
                    if points.len() < 2 {
                        return Vec::new();
                    }
                    [
                        (true, points[0], points[1]),
                        (
                            false,
                            points[points.len() - 1],
                            points[points.len() - 2],
                        ),
                    ]
                    .into_iter()
                    .filter_map(|(is_start, point, inner)| {
                        let end = (*tier_key, way_index, is_start);
                        let round_cap = !tier_joint_ends.contains(&end)
                            && fascia_joint_ends.contains(&end);
                        if !round_cap && !tier_joint_ends.contains(&end) {
                            return None;
                        }
                        let (dx, dy) = (point.0 - inner.0, point.1 - inner.1);
                        let len = (dx * dx + dy * dy).sqrt().max(1e-6);
                        Some(RoadSkirtJoint {
                            point,
                            outward: (dx / len, dy / len),
                            round_cap,
                        })
                    })
                    .collect::<Vec<_>>()
                })
                .collect();
            let field = dz_fields
                .get(1 + tier_index)
                .and_then(|field| field.as_ref());
            groups.push(PaintGroup {
                color: hex_to_premul_rgba(color, 1.0),
                // Only center faces glow: the emissive class draws as a
                // filament core, casings stay plain.
                emissive: if pass == 1 { style.center.emissive } else { 0.0 },
                phase: 1 + pass,
                rank: style.sort_rank,
                depth_micro,
                field: (1 + tier_index) as u16,
                skirt_joints,
                half_width: (width * 0.5).max(0.05),
                rings: rings
                    .into_iter()
                    .map(|(ring, dz)| {
                        // Classify with the SAME nearest-way field that will
                        // displace the final face. Copied ribbon dz can
                        // disagree inside overlapping twins/gores, causing a
                        // nominally grounded cover to punch a hole and then
                        // lift away from it at draw time.
                        let mut min_dz = f32::MAX;
                        let mut max_dz = f32::MIN;
                        if let Some(field) = field {
                            for &(x, y) in &ring {
                                let value = field.sample(x, y);
                                min_dz = min_dz.min(value);
                                max_dz = max_dz.max(value);
                            }
                        } else {
                            for value in dz {
                                min_dz = min_dz.min(value);
                                max_dz = max_dz.max(value);
                            }
                        }
                        (ring, if min_dz == f32::MAX { 0.0 } else { min_dz }, max_dz)
                    })
                    .collect(),
            });
        }
    }
    if profiler.on {
        let ring_count: usize = groups.iter().map(|group| group.rings.len()).sum();
        let ring_verts: usize = groups
            .iter()
            .flat_map(|group| group.rings.iter().map(|(ring, _, _)| ring.len()))
            .sum();
        profiler.lap(
            "rings+fields",
            &format!("groups={} rings={} ring_verts={}", groups.len(), ring_count, ring_verts),
        );
    }
    // Bake mode (offline tool): capture the cascade output + signature and
    // stop — nothing after the faces is needed for the bake.
    if faces_bake_sink_armed() {
        let regions = if groups.is_empty() {
            Vec::new()
        } else {
            compute_visible_regions(&groups)
        };
        let bucket = BakedFacesBucket {
            bucket: render_zoom,
            signature: input_sig,
            regions,
            shadow_signature: 0,
            shadow_shapes: Vec::new(),
            shadow_footprints: Vec::new(),
            building_signature: captured_building_sig,
            buildings: std::mem::take(&mut captured_building_groups),
        };
        FACES_BAKE_SINK.with(|sink| *sink.borrow_mut() = Some(Some(bucket)));
        return TileBuffers {
            pin_hits: Vec::new(),
            fill_indices: Vec::new(),
            fill_vertices: Vec::new(),
            fill_misc_indices: Vec::new(),
            fill_misc_vertices: Vec::new(),
            casing_indices: Vec::new(),
            casing_vertices: Vec::new(),
            stroke_indices: Vec::new(),
            stroke_vertices: Vec::new(),
            icon_indices: Vec::new(),
            icon_vertices: Vec::new(),
            icon_high_indices: Vec::new(),
            icon_high_vertices: Vec::new(),
            shadow_disc_indices: Vec::new(),
            shadow_disc_vertices: Vec::new(),
            icon_instances: Vec::new(),
            icon_high_instances: Vec::new(),
            fringe_indices: Vec::new(),
            fringe_vertices: Vec::new(),
            fill_3d_indices: Vec::new(),
            fill_3d_vertices: Vec::new(),
            wall_indices: Vec::new(),
            wall_vertices: Vec::new(),
            wall_instances: Vec::new(),
            tree_indices: Vec::new(),
            tree_vertices: Vec::new(),
            tree_cross_indices: Vec::new(),
            tree_cross_vertices: Vec::new(),
            tree_template_indices: Vec::new(),
            tree_template_vertices: Vec::new(),
            tree_cross_template_indices: Vec::new(),
            tree_cross_template_vertices: Vec::new(),
            tree_instances: Vec::new(),
            road_icon_indices: Vec::new(),
            road_icon_vertices: Vec::new(),
            mode_overlay_only: false,
            feature_count: 0,
            labels: Vec::new(),
            render_zoom,
            stage_summary: String::new(),
        };
    }
    let faces = if groups.is_empty() {
        Vec::new()
    } else {
        // Baked cascade fast path: regions from the archive, tessellation +
        // styling at runtime. Guarded by the group-structure signature (and
        // the stream's coordinate checksum at parse time); any mismatch
        // falls back to the runtime cascade.
        // Structural dump of the cascade input —
        // diff two runs (e.g. light vs night) to find what diverges.
        if cascade_dump_armed() {
            for (key, _, ways) in &smoothed_tiers {
                let pts: usize = ways.iter().map(|(points, _)| points.len()).sum();
                let dzs: usize = ways
                    .iter()
                    .filter(|(_, dz)| dz.is_some())
                    .count();
                trace!(
                    "map.cascade",
                    "CASCADE-TIER class={:08x} rank={} cw={:08x} caw={:08x} v={:?} l={} ways={} pts={} dz={}",
                    key.class_id,
                    key.sort_rank,
                    key.center_width_bits,
                    key.casing_width_bits,
                    key.vertical,
                    key.layer,
                    ways.len(),
                    pts,
                    dzs
                );
            }
            let plaza_pts: usize = plaza_rings.iter().map(|(_, _, p, _)| p.len()).sum();
            trace!(
                "map.cascade",
                "CASCADE-PLAZA count={} pts={} joints={} portals={}",
                plaza_rings.len(),
                plaza_pts,
                tier_joint_ends.len(),
                tunnel_portals.len()
            );
            for (gi, group) in groups.iter().enumerate() {
                let ring_lens: Vec<usize> =
                    group.rings.iter().map(|(ring, _, _)| ring.len()).collect();
                let lift_bits: Vec<u8> = group
                    .rings
                    .iter()
                    .map(|(_, min_dz, max_dz)| {
                        (*max_dz >= LIFT_COVER_M) as u8 | (((*min_dz <= -LIFT_COVER_M) as u8) << 1)
                    })
                    .collect();
                trace!(
                    "map.cascade",
                    "CASCADE-GROUP {gi}: phase={} rank={} field={} hw={} rings={:?} lift={:?}",
                    group.phase, group.rank, group.field, group.half_width, ring_lens, lift_bits
                );
            }
        }
        let baked = baked_faces.as_ref().filter(|bake| {
            let ok = bake.bucket == render_zoom && bake.signature == input_sig;
            if !ok && profiler.on {
                trace!(
                    "map.tile_profile",
                    "cascade-baked-MISS bucket {} vs rz {} sig {:016x} vs runtime {:016x} regions {} groups {}",
                    bake.bucket,
                    render_zoom,
                    bake.signature,
                    input_sig,
                    bake.regions.len(),
                    groups.len(),
                );
            }
            ok
        });
        match baked {
            Some(bake) => {
                if profiler.on {
                    trace!("map.tile_profile", "cascade-baked regions={}", bake.regions.len());
                }
                // Group count is pinned by the input signature; an index
                // past it means a corrupt stream (already checksum-guarded
                // at parse). Drop such regions instead of indexing OOB —
                // the runtime cascade is NOT a fallback here, the rings
                // were skipped on the signature's authority.
                let in_range: Vec<VisibleRegions>;
                let regions: &[VisibleRegions] = if bake
                    .regions
                    .iter()
                    .all(|region| region.group_index < groups.len())
                {
                    &bake.regions
                } else {
                    log!(
                        "MapView: baked faces region/group mismatch on z{} x{} y{} — dropping out-of-range regions",
                        tile_key.z,
                        tile_key.x,
                        tile_key.y
                    );
                    in_range = bake
                        .regions
                        .iter()
                        .filter(|region| region.group_index < groups.len())
                        .cloned()
                        .collect();
                    &in_range
                };
                build_paint_faces(&groups, regions, &mut tess, tolerance, analytic_fringe_units)
            }
            None => overlay_paint_groups(&groups, &mut tess, tolerance, analytic_fringe_units),
        }
    };
    if profiler.on {
        let face_verts: usize = faces.iter().map(|face| face.verts.len()).sum();
        profiler.lap(
            "boolean",
            &format!("faces={} face_verts={}", faces.len(), face_verts),
        );
    }

    enum RoadPaintEvent<'a> {
        Face(usize),
        Stroke {
            pass: StrokePassStyle,
            part: &'a [(f32, f32)],
            start_cap: LineCap,
            end_cap: LineCap,
        },
    }
    // Round caps blend same-color segments at junctions and give dead ends
    // the carto nub — but ends produced by the tile clip must stay butt, or
    // the cap disc overpaints the neighbor tile's content.
    let cap_eps = 0.05_f32;
    // Sort key gains a level-class prefix: sunk faces (tunnels) paint
    // before ALL surface content — under plazas, casings, everything.
    let events_build_clock = ProfileClock::now();
    let mut events: Vec<((u8, u8, i16, u8, u32), RoadPaintEvent<'_>)> = Vec::new();
    for (face_index, face) in faces.iter().enumerate() {
        let level_class = if face.level < 0 { 0u8 } else { 1 };
        events.push((
            (level_class, face.phase, face.rank, 1, face_index as u32),
            RoadPaintEvent::Face(face_index),
        ));
    }
    let mut stroke_seq = 0u32;
    for (style, parts) in &merged_stroke_parts {
        for part in parts {
            if part.len() < 2 {
                continue;
            }
            if let Some(casing) = style.casing {
                events.push((
                    (1, 1, style.sort_rank, 0, stroke_seq),
                    RoadPaintEvent::Stroke {
                        pass: casing,
                        part,
                        start_cap: LineCap::Butt,
                        end_cap: LineCap::Butt,
                    },
                ));
                stroke_seq += 1;
            }
            if style.sort_rank <= max_tier_rank {
                let start_cap = if point_on_bounds(part[0], clip_bounds, cap_eps) {
                    LineCap::Butt
                } else {
                    LineCap::Round
                };
                let end_cap = if point_on_bounds(part[part.len() - 1], clip_bounds, cap_eps) {
                    LineCap::Butt
                } else {
                    LineCap::Round
                };
                events.push((
                    (1, 2, style.sort_rank, 0, stroke_seq),
                    RoadPaintEvent::Stroke {
                        pass: style.center,
                        part,
                        start_cap,
                        end_cap,
                    },
                ));
                stroke_seq += 1;
            }
        }
    }
    events.sort_by_key(|(key, _)| *key);

    // Corridor bbox prefilter: deck matching is O(verts x corridors) per
    // stroke, and most strokes are nowhere near a bridge. One cheap bbox
    // pass decides which strokes pay for corridor matching at all.
    let corridor_set: &[BridgeCorridor] =
        if bridge_dz_covered { &own_profiles } else { &bridge_corridors };
    let corridor_boxes: Vec<(f32, f32, f32, f32)> = corridor_set
        .iter()
        .map(|corridor| {
            // Match the exact support of corridor_deck_overrides. Solved
            // profiles are identically zero at SOLVED_REACH_ZERO; heuristic
            // corridors are identically zero outside half_width + feather.
            // The previous extra two units admitted many strokes that could
            // never receive a non-zero deck and made them pay the full
            // vertex x corridor-segment matcher anyway.
            let reach = if corridor.solved {
                SOLVED_REACH_ZERO
            } else {
                corridor.half_width + CORRIDOR_FEATHER
            };
            let mut min_x = f32::MAX;
            let mut min_y = f32::MAX;
            let mut max_x = f32::MIN;
            let mut max_y = f32::MIN;
            for &(x, y) in &corridor.points {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
            (min_x - reach, min_y - reach, max_x + reach, max_y + reach)
        })
        .collect();
    let part_near_corridor = |part: &[(f32, f32)]| -> bool {
        if corridor_boxes.is_empty() {
            return false;
        }
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        for &(x, y) in part {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
        corridor_boxes
            .iter()
            .any(|&(bx0, by0, bx1, by1)| min_x <= bx1 && max_x >= bx0 && min_y <= by1 && max_y >= by0)
    };
    // Segment grid for the matcher itself: the bbox gate above only decides
    // WHETHER a part pays for deck matching, the grid keeps that matching
    // from scanning every corridor segment per vertex.
    let corridor_grid = CorridorGrid::build(corridor_set);
    let corridor_query = CorridorGridQuery {
        corridors: corridor_set,
        grid: &corridor_grid,
    };

    // Tilt depth is semantic rather than face-ordinal. Adjacent tiles contain
    // different padded face soups, so an ordinal ladder assigned two copies
    // of the same road different depths and exposed the overlap at pitch.
    // Event sorting remains authoritative for flat painter order.
    let mut prof_subdiv_ms = 0.0f64;
    let mut prof_sample_ms = 0.0f64;
    let mut prof_face_verts_out = 0usize;
    let mut prof_skirt_ms = 0.0f64;
    let mut prof_body_ms = 0.0f64;
    let mut prof_fringe_ms = 0.0f64;
    let mut prof_stroke_arm_ms = 0.0f64;
    let mut prof_face_arm_ms = 0.0f64;
    let prof_events_build_ms = events_build_clock.elapsed_seconds() * 1e3;
    let events_loop_clock = ProfileClock::now();
    for ((_, phase, _, _, _), event) in &events {
        match event {
            RoadPaintEvent::Face(face_index) => {
                let whole_face_clock = ProfileClock::now();
                let face = &faces[*face_index];
                let face_param5 =
                    road_semantic_param5(face.level, face.phase, face.depth_micro);
                // 3D: faces re-acquire deck height from their tier's dz
                // field — height never needs to survive the boolean. The
                // mesh is refined near lifted geometry first, so ramps
                // interpolate as smoothly as densely sampled stroke meshes.
                let field = dz_fields
                    .get(face.field as usize)
                    .and_then(|field| field.as_ref());
                let mut sub_verts;
                let mut sub_indices;
                let mut sub_offsets: Vec<[f32; 2]> = Vec::new();
                let (verts, indices, deck): (&[VVertex], &[u32], Option<Vec<f32>>) =
                    match field {
                        // Only faces whose bbox touches lifted cells pay the
                        // clone + refine + per-vertex sample; a tier with one
                        // bridge must not tax its every face tile-wide.
                        Some(field)
                            if !face.verts.is_empty() && {
                                let mut min_x = f32::MAX;
                                let mut min_y = f32::MAX;
                                let mut max_x = f32::MIN;
                                let mut max_y = f32::MIN;
                                for v in &face.verts {
                                    min_x = min_x.min(v.x);
                                    min_y = min_y.min(v.y);
                                    max_x = max_x.max(v.x);
                                    max_y = max_y.max(v.y);
                                }
                                field.active_near(min_x, min_y, max_x, max_y)
                            } =>
                        {
                            let clock = ProfileClock::now();
                            sub_verts = face.verts.clone();
                            sub_indices = face.indices.clone();
                            if face.morph_offsets.len() == face.verts.len()
                                && face.emissive <= 0.001
                            {
                                sub_offsets = face.morph_offsets.clone();
                                subdivide_face_mesh_morph(
                                    &mut sub_verts,
                                    &mut sub_indices,
                                    &mut sub_offsets,
                                    3.0,
                                    field,
                                );
                            } else {
                                sub_offsets = Vec::new();
                                subdivide_face_mesh(&mut sub_verts, &mut sub_indices, 3.0, field);
                            }
                            prof_subdiv_ms += clock.elapsed_seconds() * 1000.0;
                            let clock = ProfileClock::now();
                            let deck: Vec<f32> = sub_verts
                                .iter()
                                .map(|v| field.sample(v.x, v.y))
                                .collect();
                            prof_sample_ms += clock.elapsed_seconds() * 1000.0;
                            prof_face_verts_out += sub_verts.len();
                            let displaced = deck.iter().any(|&d| d.abs() > 0.05);
                            (&sub_verts, &sub_indices, displaced.then_some(deck))
                        }
                        _ => (&face.verts, &face.indices, None),
                    };
                let face_clock = ProfileClock::now();
                // Deck side walls first (under the face): top verts (v=0)
                // ride the deck field, bottom verts (v=1) stay grounded —
                // flat mode collapses them, tilt reveals the wall. Closes
                // the crescents a displaced ramp leaves over its footprint.
                if !face.skirt_verts.is_empty() {
                    if let Some(field) = field {
                        let mut sk_verts = face.skirt_verts.clone();
                        let mut sk_indices = face.skirt_indices.clone();
                        subdivide_face_mesh(&mut sk_verts, &mut sk_indices, 3.0, field);
                        // Fascia, not full wall: the road reads ~1.4 m
                        // thick — the band hangs below the deck edge
                        // (clamped at ground on low ramps) and the space
                        // underneath stays OPEN for underpasses. Internal
                        // seams are dropped by the probe below, so the
                        // band can run the full elevated length.
                        const DECK_FASCIA_M: f32 = 1.4;
                        let sk_deck: Vec<f32> = sk_verts
                            .iter()
                            .map(|v| {
                                let deck = field.sample(v.x, v.y);
                                // Subdivision creates intermediate v values
                                // on the top-to-bottom diagonal. Interpolate
                                // the fascia height continuously; snapping
                                // v<=0.5 to the top folded each quad into
                                // opposing triangles ("scissor" holes).
                                (deck - DECK_FASCIA_M * v.v.clamp(0.0, 1.0)).max(0.0)
                            })
                            .collect();
                        // Only decks that read as real structures hang a
                        // fascia: quay humps (solved profiles peak at
                        // exactly 1.0 m here) rendered 1 px dark hairs at
                        // their stub ends ("shadow spikes" in review —
                        // they weren't shadows at all).
                        if sk_deck.iter().any(|&d| d > 1.0) {
                            let wall = [
                                face.color[0] * 0.72,
                                face.color[1] * 0.72,
                                face.color[2] * 0.72,
                                face.color[3],
                            ];
                            append_tessellated_geometry_decked(
                                &sk_verts,
                                &sk_indices,
                                &mut casing_vertices,
                                &mut casing_indices,
                                VectorRenderParams {
                                    color: wall,
                                    stroke_mult: 1e6,
                                    shape_id: 0.0,
                                    params: [
                                        0.0,
                                        0.0,
                                        0.0,
                                        0.0,
                                        0.0,
                                        (face_param5 - ROAD_FASCIA_DEPTH_EPSILON).max(0.0),
                                    ],
                                    zbias: casing_zbias,
                                },
                                Some(&sk_deck),
                            );
                            casing_zbias += VECTOR_ZBIAS_STEP;
                        }
                    }
                }
                prof_skirt_ms += face_clock.elapsed_seconds() * 1e3;
                let face_clock = ProfileClock::now();
                // Morphable body: non-emissive faces whose offsets are
                // 1:1 with the emitted verts — the dz-subdivided path
                // carries them through midpoint averaging, so decked city
                // centers morph too (pinned kf16 geometry there was the
                // magnified-wide-roads glitch). Deck heights ride the
                // expanded layout's per-vertex override.
                let body_offsets: &[[f32; 2]] = if deck.is_some() {
                    &sub_offsets
                } else {
                    &face.morph_offsets
                };
                #[cfg(not(target_arch = "wasm32"))]
                static FACE_MORPH: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
                #[cfg(not(target_arch = "wasm32"))]
                let face_morph_on = *FACE_MORPH.get_or_init(|| {
                    std::env::var_os("MAKEPAD_FACE_MORPH").is_some()
                        || std::path::Path::new("/tmp/mp_face_morph").exists()
                });
                #[cfg(target_arch = "wasm32")]
                let face_morph_on = false;
                let body_morph = face_morph_on
                    && face.emissive <= 0.001
                    && body_offsets.len() == verts.len()
                    && body_offsets.iter().any(|o| o[0] != 0.0 || o[1] != 0.0);
                if body_morph {
                    let anchors: Vec<[f32; 2]> = verts
                        .iter()
                        .zip(body_offsets)
                        .map(|(v, o)| [v.x - o[0], v.y - o[1]])
                        .collect();
                    append_expanded_stroke_geometry(
                        verts,
                        &anchors,
                        indices,
                        &mut casing_vertices,
                        &mut casing_indices,
                        VectorRenderParams {
                            color: face.color,
                            stroke_mult: 1e6,
                            shape_id: 0.0,
                            params: [0.0, 0.0, 0.0, 0.0, 0.0, face_param5],
                            zbias: casing_zbias,
                        },
                        EXPAND_CLASS_ROAD + FACE_MORPH_CLASS_OFFSET,
                        0.0,
                        deck.as_deref(),
                    );
                } else {
                    append_tessellated_geometry_decked(
                        verts,
                        indices,
                        &mut casing_vertices,
                        &mut casing_indices,
                        VectorRenderParams {
                            color: face.color,
                            stroke_mult: 1e6,
                            shape_id: 0.0,
                            params: [
                                0.0,
                                face.emissive,
                                0.0,
                                if face.emissive > 0.001 { MAT_ROUTE_GLOW } else { 0.0 },
                                0.0,
                                face_param5,
                            ],
                            zbias: casing_zbias,
                        },
                        deck.as_deref(),
                    );
                }
                casing_zbias += VECTOR_ZBIAS_STEP;
                prof_body_ms += face_clock.elapsed_seconds() * 1e3;
                let face_clock = ProfileClock::now();
                // AA skirt: same slot, next zbias step — blends this face's
                // boundary over whatever the ladder painted below it.
                if !face.fringe_verts.is_empty() {
                    let mut fringe_verts;
                    let mut fringe_indices;
                    let (fr_verts, fr_indices, fr_deck): (&[VVertex], &[u32], Option<Vec<f32>>) =
                        match field {
                            Some(field) if deck.is_some() => {
                                fringe_verts = face.fringe_verts.clone();
                                fringe_indices = face.fringe_indices.clone();
                                // Every boundary/outer pair must remain on
                                // one vertical plane. Sampling the moved
                                // outer XY lets DzField's corridor feather
                                // pull a wide AA carrier toward the ground,
                                // producing a visible sloped curtain.
                                let mut fr_deck = Vec::with_capacity(fringe_verts.len());
                                for pair in fringe_verts.chunks_exact(2) {
                                    let height = field.sample(pair[0].x, pair[0].y);
                                    fr_deck.extend_from_slice(&[height, height]);
                                }
                                debug_assert_eq!(fr_deck.len(), fringe_verts.len());
                                subdivide_face_mesh_decked(
                                    &mut fringe_verts,
                                    &mut fringe_indices,
                                    &mut fr_deck,
                                    3.0,
                                    field,
                                );
                                (&fringe_verts, &fringe_indices, Some(fr_deck))
                            }
                            _ => (&face.fringe_verts, &face.fringe_indices, None),
                        };
                    // Signed u runs 0 -> -1 from the exact road edge through
                    // a wide, outside-only carrier. DrawVector divides by
                    // its screen derivative to recover device-pixel distance.
                    // Morphable fringes ride the expandable band with their
                    // boundary vertices' offsets so the AA edge tracks the
                    // morphed face edge; carrier outer verts pin (u-ramp
                    // stretches, coverage stays one pixel by fwidth).
                    let fringe_morph = face_morph_on
                        && fr_deck.is_none()
                        && face.emissive <= 0.001
                        && face.morph_fringe_offsets.len() == fr_verts.len()
                        && face
                            .morph_fringe_offsets
                            .iter()
                            .any(|o| o[0] != 0.0 || o[1] != 0.0);
                    if fringe_morph {
                        let anchors: Vec<[f32; 2]> = fr_verts
                            .iter()
                            .zip(&face.morph_fringe_offsets)
                            .map(|(v, o)| [v.x - o[0], v.y - o[1]])
                            .collect();
                        append_expanded_stroke_geometry(
                            fr_verts,
                            &anchors,
                            fr_indices,
                            &mut casing_vertices,
                            &mut casing_indices,
                            VectorRenderParams {
                                color: face.color,
                                stroke_mult: VECTOR_ANALYTIC_FRINGE_STROKE_MULT,
                                shape_id: 0.0,
                                params: [
                                    0.0,
                                    0.0,
                                    0.0,
                                    0.0,
                                    0.0,
                                    face_param5 + ROAD_FRINGE_DEPTH_EPSILON,
                                ],
                                zbias: casing_zbias,
                            },
                            EXPAND_CLASS_ROAD + FACE_MORPH_CLASS_OFFSET,
                            0.0,
                            None,
                        );
                    } else {
                        append_tessellated_geometry_decked(
                            fr_verts,
                            fr_indices,
                            &mut casing_vertices,
                            &mut casing_indices,
                            VectorRenderParams {
                                color: face.color,
                                stroke_mult: VECTOR_ANALYTIC_FRINGE_STROKE_MULT,
                                shape_id: 0.0,
                                // A fixed sub-rank epsilon keeps the carrier on
                                // its own face without making depth tile-local.
                                params: [
                                    0.0,
                                    face.emissive,
                                    0.0,
                                    if face.emissive > 0.001 { MAT_ROUTE_GLOW } else { 0.0 },
                                    0.0,
                                    face_param5 + ROAD_FRINGE_DEPTH_EPSILON,
                                ],
                                zbias: casing_zbias,
                            },
                            fr_deck.as_deref(),
                        );
                    }
                    casing_zbias += VECTOR_ZBIAS_STEP;
                }
                prof_fringe_ms += face_clock.elapsed_seconds() * 1e3;
                prof_face_arm_ms += whole_face_clock.elapsed_seconds() * 1e3;
                feature_count += 1;
            }
            RoadPaintEvent::Stroke {
                pass,
                part,
                start_cap,
                end_cap,
            } => {
                let arm_clock = ProfileClock::now();
                let near_corridor = stroke_corridors_available && part_near_corridor(part);
                let param5 = if pass.deck_m < 0.0 {
                    // Patterned tunnels have no physical sunk mesh; keep
                    // their visible cartographic casing and dashed center
                    // in two stable slots just under surface plazas.
                    if *phase == 1 {
                        ROAD_TUNNEL_CASING_DEPTH
                    } else {
                        ROAD_TUNNEL_CENTER_DEPTH
                    }
                } else {
                    road_semantic_param5(0, *phase, pass.depth_micro)
                };
                append_stroke_pass(
                    &mut path,
                    part,
                    false,
                    near_corridor.then_some(corridor_query),
                    &mut tess,
                    &mut tess_verts,
                    &mut tess_indices,
                    &mut casing_vertices,
                    &mut casing_indices,
                    *pass,
                    *start_cap,
                    *end_cap,
                    LineJoin::Round,
                    aa_units,
                    tolerance,
                    &mut casing_zbias,
                    param5,
                );
                prof_stroke_arm_ms += arm_clock.elapsed_seconds() * 1e3;
                feature_count += 1;
            }
        }
    }

    let prof_events_loop_ms = events_loop_clock.elapsed_seconds() * 1e3;
    let sp = crate::map::geometry::stroke_prof_take();
    profiler.lap(
        "emit",
        &format!(
            "events={} subdiv={:.1}ms sample={:.1}ms sub_verts={} skirt={:.1} body={:.1} fringe={:.1} arm={:.1} facearm={:.1} ebuild={:.1} eloop={:.1} | strokes: calls={} verts={} densify={:.1}ms tess={:.1}ms deck={:.1}ms expand={:.1}ms",
            events.len(),
            prof_subdiv_ms,
            prof_sample_ms,
            prof_face_verts_out,
            prof_skirt_ms,
            prof_body_ms,
            prof_fringe_ms,
            prof_stroke_arm_ms,
            prof_face_arm_ms,
            prof_events_build_ms,
            prof_events_loop_ms,
            sp.calls,
            sp.verts,
            sp.densify_ms,
            sp.tess_ms,
            sp.deck_ms,
            sp.expand_ms
        ),
    );

    // Centers ranked above the topmost road tier (trams, rails): stroke
    // buffer, above every face and interleaved stroke.
    for (style, parts) in &merged_stroke_parts {
        if style.sort_rank <= max_tier_rank {
            continue;
        }
        for part in parts {
            if part.len() < 2 {
                continue;
            }
            let near_corridor = stroke_corridors_available && part_near_corridor(part);
            let start_cap = if point_on_bounds(part[0], clip_bounds, cap_eps) {
                LineCap::Butt
            } else {
                LineCap::Round
            };
            let end_cap = if point_on_bounds(part[part.len() - 1], clip_bounds, cap_eps) {
                LineCap::Butt
            } else {
                LineCap::Round
            };
            append_stroke_pass(
                &mut path,
                part,
                false,
                near_corridor.then_some(corridor_query),
                &mut tess,
                &mut tess_verts,
                &mut tess_indices,
                &mut stroke_vertices,
                &mut stroke_indices,
                style.center,
                start_cap,
                end_cap,
                LineJoin::Round,
                aa_units,
                tolerance,
                &mut stroke_zbias,
                road_semantic_param5(0, 2, style.center.depth_micro)
                    - ROAD_STROKE_PASS_DEPTH_OFFSET,
            );
            feature_count += 1;
        }
    }

    // Pass 3: POI symbols — zoom-constant vector icons, drawn above strokes.
    // Instanced: the mesh lives once on the GPU, each placement is a record.
    let mut icon_groups = Vec::<IconInstances>::new();
    for (job_index, (anchor, mesh, color_class, _, _, two_tone, kw, stalls, zoom_floor, _)) in
        icon_jobs.iter().enumerate()
    {
        let pin_lift_m = job_lifts[job_index];
        // The lift rides in param4's hundreds (0.25 m quanta) so the zoom
        // floor keeps its low digits.
        let param4_encoded = zoom_floor + (pin_lift_m * 4.0).round() * 100.0;
        push_icon_instance(
            &mut icon_groups,
            mesh,
            *anchor,
            (0.0, 0.0),
            1.0,
            hex_to_premul_rgba(poi_class_hex(*color_class), 1.0),
            param4_encoded,
            &mut icon_zbias,
        );
        // carto trees: light canopy disc with a dark center dot.
        if *two_tone == 1 {
            if let Some(core) = icon_mesh("tree_core") {
                push_icon_instance(
                    &mut icon_groups,
                    core,
                    *anchor,
                    (0.0, 0.0),
                    1.0,
                    hex_to_premul_rgba(0x4c7a4c, 1.0),
                    param4_encoded,
                    &mut icon_zbias,
                );
            }
        }
        // Charger pins are one COMPOSITE at a single anchor: badge, white
        // bolt (offset baked in the mesh) and, for fast sites, the kW
        // digits as vector glyphs — all billboard together, so nothing
        // detaches, doubles or re-lays-out while zooming or rotating.
        if *two_tone == 2 || *two_tone == 3 {
            let bolt_name = if *two_tone == 2 { "charger_bolt_fast" } else { "charger_bolt_ac" };
            if let Some(bolt) = icon_mesh(bolt_name) {
                push_icon_instance(
                    &mut icon_groups,
                    bolt,
                    *anchor,
                    (0.0, 0.0),
                    1.0,
                    hex_to_premul_rgba(0xffffff, 1.0),
                    param4_encoded,
                    &mut icon_zbias,
                );
            }
        }
        let _ = (kw, stalls);
        feature_count += 1;
    }

    // Oneway arrows: zoom-constant glyphs spaced along the way, offsets
    // pre-rotated into the travel direction (carto-style). Each physical
    // surface key resolves to the same semantic center depth on every tile.
    // The icon pass itself contributes +0.04, so append_oneway_arrow
    // subtracts that global lift and leaves only an own-surface decal bias.
    let arrow_color = hex_to_premul_rgba(0x8a8a8a, 1.0);
    let arrow_interval = 170.0 / render_scale;
    let arrow_union_fields: HashMap<RoadSurfaceKey, u16> = smoothed_tiers
        .iter()
        .enumerate()
        .map(|(index, (key, _, _))| (*key, (index + 1) as u16))
        .collect();
    let arrow_surface_depth_at = |surface: ArrowSurfaceDepth| -> f32 {
        match surface {
            ArrowSurfaceDepth::Union(key) => road_surface_param5(key, 2),
            ArrowSurfaceDepth::Stroke { level, depth_micro } => {
                if level < 0 {
                    ROAD_TUNNEL_CENTER_DEPTH
                } else {
                    road_semantic_param5(0, 2, depth_micro)
                }
            }
            ArrowSurfaceDepth::Unknown => ROAD_UNION_CENTER_DEPTH,
        }
    };
    for job in &arrow_jobs {
        let final_surface_field = match job.surface_depth {
            ArrowSurfaceDepth::Union(key) => arrow_union_fields
                .get(&key)
                .and_then(|field| dz_fields.get(*field as usize))
                .and_then(|field| field.as_ref()),
            ArrowSurfaceDepth::Stroke { .. } | ArrowSurfaceDepth::Unknown => None,
        };
        for part in build_polyline_parts(&job.points, clip_bounds, false, 0.0) {
            let mut cumulative = Vec::<f32>::with_capacity(part.len());
            let mut total = 0.0_f32;
            cumulative.push(0.0);
            for pair in part.windows(2) {
                let dx = pair[1].0 - pair[0].0;
                let dy = pair[1].1 - pair[0].1;
                total += (dx * dx + dy * dy).sqrt();
                cumulative.push(total);
            }
            if total < arrow_interval * 0.6 {
                continue;
            }
            let mut distance = arrow_interval * 0.5;
            while distance < total {
                // find segment containing this distance
                let mut segment = 0;
                while segment + 2 < cumulative.len() && cumulative[segment + 1] < distance {
                    segment += 1;
                }
                let seg_len = (cumulative[segment + 1] - cumulative[segment]).max(1e-6);
                let t = ((distance - cumulative[segment]) / seg_len).clamp(0.0, 1.0);
                let a = part[segment];
                let b = part[segment + 1];
                let anchor = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
                let mut dir_x = (b.0 - a.0) / seg_len;
                let mut dir_y = (b.1 - a.1) / seg_len;
                if job.reverse {
                    dir_x = -dir_x;
                    dir_y = -dir_y;
                }
                // Patterned tunnels are a cartographic ground-plane overlay:
                // keep their arrows on that same plane instead of sinking
                // or hoisting them with the physical corridor profile.
                let patterned_tunnel = matches!(
                    job.surface_depth,
                    ArrowSurfaceDepth::Stroke { level, .. } if level < 0
                );
                let profile_dz = if patterned_tunnel {
                    None
                } else {
                    job.dz.as_deref()
                };
                // The baked profile on THIS source way is authoritative for
                // physical surfaces. Outside baked coverage, direction-gated
                // heuristic corridors remain the fallback.
                let fallback_corridors: &[BridgeCorridor] =
                    if patterned_tunnel || bridge_dz_covered {
                    &[]
                } else {
                    &bridge_corridors
                };
                append_oneway_arrow(
                    anchor,
                    dir_x,
                    dir_y,
                    render_scale,
                    &job.points,
                    profile_dz,
                    final_surface_field,
                    fallback_corridors,
                    arrow_surface_depth_at(job.surface_depth),
                    arrow_color,
                    &mut road_icon_vertices,
                    &mut road_icon_indices,
                    &mut icon_zbias,
                );
                distance += arrow_interval;
            }
        }
    }

    // Road decals intentionally remain in the ordinary icon pass. Keep a
    // compact source copy so a mode-only bake can append the exact same
    // vertices without rerunning any road work.
    if build_road_core {
        let vertex_base = (icon_vertices.len() / VECTOR_FLOATS_PER_VERTEX) as u32;
        icon_indices.extend(
            road_icon_indices
                .iter()
                .map(|index| index + vertex_base),
        );
        icon_vertices.extend_from_slice(&road_icon_vertices);
    }

    compact_tile_labels(&mut labels);

    profiler.lap("tail", "");
    // FNV over every emitted buffer, printed on the
    // TOTAL line — the bit-identity oracle for geometry-path refactors.
    let buffer_hash = if crate::makepad_platform::makepad_error_log::trace_enabled("map.tile_hash") {
        let mut h = 0xcbf29ce484222325u64;
        let mut eat = |bytes: &[u8]| {
            for &b in bytes {
                h = (h ^ b as u64).wrapping_mul(0x100000001b3);
            }
        };
        for floats in [&fill_vertices, &casing_vertices, &stroke_vertices, &icon_vertices] {
            for f in floats.iter() {
                eat(&f.to_bits().to_le_bytes());
            }
        }
        for indices in [&fill_indices, &casing_indices, &stroke_indices, &icon_indices] {
            for i in indices.iter() {
                eat(&i.to_le_bytes());
            }
        }
        trace!("map.tile_hash", "z{}/{}/{} hash={:016x}", tile_key.z, tile_key.x, tile_key.y, h);
        format!(" hash={h:016x}")
    } else {
        String::new()
    };
    profiler.total(
        tile_key,
        &format!(
            "rz{} fill={}KB casing={}KB stroke={}KB icon={}KB{}",
            render_zoom,
            (fill_vertices.len() + fill_indices.len()) * 4 / 1024,
            (casing_vertices.len() + casing_indices.len()) * 4 / 1024,
            (stroke_vertices.len() + stroke_indices.len()) * 4 / 1024,
            (icon_vertices.len() + icon_indices.len()) * 4 / 1024,
            buffer_hash,
        ),
    );

    let stage_summary = if profiler.start.elapsed_seconds() * 1e3 > 100.0 {
        profiler.summary()
    } else {
        String::new()
    };
    let mut icon_vertices = icon_vertices;
    let mut icon_indices = icon_indices;
    let (icon_high_vertices, icon_high_indices) =
        split_icon_band(&mut icon_vertices, &mut icon_indices);
    let (shadow_disc_vertices, shadow_disc_indices) = split_band_by(
        &mut icon_vertices,
        &mut icon_indices,
        |record| record[14] > 5.5 && record[14] < 6.5,
    );
    let (icon_instances, icon_high_instances) = split_icon_instance_band(icon_groups);
    let mut casing_vertices = casing_vertices;
    let mut casing_indices = casing_indices;
    let (fringe_vertices, fringe_indices) =
        split_fringe_band(&mut casing_vertices, &mut casing_indices);
    // 3D band sub-splits by material (param3, slot 14): walls skip at the
    // mid LOD ring ("roofs only"), canopy balls swap to crossed quads far
    // out. Materials were authored per-append so the predicate is exact.
    let mut fill_3d_vertices = fill_3d_vertices;
    let mut fill_3d_indices = fill_3d_indices;
    let (wall_vertices, wall_indices) = split_band_by(
        &mut fill_3d_vertices,
        &mut fill_3d_indices,
        |record| record[14] > 0.5 && record[14] < 1.5,
    );
    let (tree_vertices, tree_indices) = split_band_by(
        &mut fill_3d_vertices,
        &mut fill_3d_indices,
        |record| record[14] > 3.5 && record[14] < 4.5,
    );
    // The ground stream is compact only for polygon-fill variants. Building
    // outline strokes are the sole generic records emitted into this pass;
    // keep them in a sibling stream so their stroke expansion stays intact.
    let mut fill_vertices = fill_vertices;
    let mut fill_indices = fill_indices;
    let (fill_misc_vertices, fill_misc_indices) = split_band_by(
        &mut fill_vertices,
        &mut fill_indices,
        |record| map_fill_variant_code(record).is_none(),
    );
    // GPU-pack on the builder thread: uploads ship pre-packed bytes (the
    // main-thread pack was 10-15ms per street tile and throttled the
    // upload drain to one tile per frame).
    let fill_vertices = pack_fill_vertices(&fill_vertices);
    let fill_misc_vertices = pack_vector_vertices(&fill_misc_vertices);
    let fill_3d_vertices = pack_vector_vertices(&fill_3d_vertices);
    debug_assert!(casing_vertices
        .chunks_exact(VECTOR_FLOATS_PER_VERTEX)
        .all(|record| record[10].abs() < 0.5 || record[10] >= 99.5));
    debug_assert!(stroke_vertices
        .chunks_exact(VECTOR_FLOATS_PER_VERTEX)
        .all(|record| record[10].abs() < 0.5 || record[10] >= 99.5));
    debug_assert!(fringe_vertices
        .chunks_exact(VECTOR_FLOATS_PER_VERTEX)
        .all(|record| record[10].abs() < 0.5 || record[10] >= 99.5));
    let casing_vertices = pack_road_vertices(&casing_vertices);
    let stroke_vertices = pack_road_vertices(&stroke_vertices);
    let icon_vertices = pack_vector_vertices(&icon_vertices);
    let icon_high_vertices = pack_vector_vertices(&icon_high_vertices);
    let shadow_disc_vertices = pack_vector_vertices(&shadow_disc_vertices);
    let fringe_vertices = pack_road_vertices(&fringe_vertices);
    let wall_vertices = pack_vector_vertices(&wall_vertices);
    let tree_vertices = pack_vector_vertices(&tree_vertices);
    let tree_cross_vertices = pack_vector_vertices(&tree_cross_vertices);
    TileBuffers {
        pin_hits,
        fill_indices,
        fill_vertices,
        fill_misc_indices,
        fill_misc_vertices,
        casing_indices,
        casing_vertices,
        stroke_indices,
        stroke_vertices,
        icon_indices,
        icon_vertices,
        icon_high_indices,
        icon_high_vertices,
        shadow_disc_indices,
        shadow_disc_vertices,
        icon_instances,
        icon_high_instances,
        fringe_indices,
        fringe_vertices,
        fill_3d_indices,
        fill_3d_vertices,
        wall_indices,
        wall_vertices,
        wall_instances,
        tree_indices,
        tree_vertices,
        tree_cross_indices,
        tree_cross_vertices,
        tree_template_indices,
        tree_template_vertices: pack_vector_vertices(&tree_template_vertices),
        tree_cross_template_indices,
        tree_cross_template_vertices: pack_vector_vertices(&tree_cross_template_vertices),
        tree_instances,
        road_icon_indices,
        road_icon_vertices,
        mode_overlay_only: !build_road_core,
        feature_count,
        labels,
        render_zoom,
        stage_summary,
    }
}

/// Shape id telling the map vertex shader to treat (param1, param2) as a
/// screen-px offset (zoom-constant symbols). Regular icons add it after the
/// map transform; surface decals (param3 = 2) project it with the road plane.
pub const ICON_SHAPE_ID: f32 = 20.0;

/// Wall-LOD ring simplification: drop vertices closer than `min_edge` to
/// the last kept one. The roof keeps the detailed ring; a wall silhouette
/// offset by under a pixel is invisible.
fn simplify_wall_ring(ring: &[(f32, f32)], min_edge: f32) -> Vec<(f32, f32)> {
    if ring.len() <= 4 {
        return ring.to_vec();
    }
    let min_sq = min_edge * min_edge;
    let mut out = Vec::with_capacity(ring.len());
    out.push(ring[0]);
    for &point in &ring[1..] {
        let last = *out.last().unwrap();
        let d2 = (point.0 - last.0).powi(2) + (point.1 - last.1).powi(2);
        if d2 >= min_sq {
            out.push(point);
        }
    }
    if out.len() >= 2 {
        let first = out[0];
        let last = *out.last().unwrap();
        if (first.0 - last.0).powi(2) + (first.1 - last.1).powi(2) < min_sq {
            out.pop();
        }
    }
    // Collinear-run merge: drop vertices whose turn is under ~2 degrees —
    // the adjacent wall quads fuse into one (same plane, same shade, same
    // silhouette). Footprint digitization noise makes these runs common.
    if out.len() > 4 {
        let mut merged = Vec::with_capacity(out.len());
        let n = out.len();
        for i in 0..n {
            let prev = out[(i + n - 1) % n];
            let cur = out[i];
            let next = out[(i + 1) % n];
            let (ax, ay) = (cur.0 - prev.0, cur.1 - prev.1);
            let (bx, by) = (next.0 - cur.0, next.1 - cur.1);
            let cross = ax * by - ay * bx;
            let dot = ax * bx + ay * by;
            let len2 = ((ax * ax + ay * ay) * (bx * bx + by * by)).sqrt();
            let straight = len2 > 1e-9 && dot > 0.0 && cross.abs() / len2 < 0.035;
            if !straight {
                merged.push(cur);
            }
        }
        if merged.len() >= 3 {
            out = merged;
        }
    }
    out
}

fn ring_centroid(ring: &[(f32, f32)]) -> (f32, f32) {
    if ring.is_empty() {
        return (0.0, 0.0);
    }
    let mut sum = (0.0_f32, 0.0_f32);
    for point in ring {
        sum.0 += point.0;
        sum.1 += point.1;
    }
    (sum.0 / ring.len() as f32, sum.1 / ring.len() as f32)
}

fn deck_profile_at_point_dir(
    point: (f32, f32),
    dir: (f32, f32),
    points: &[(f32, f32)],
    dz: &[f32],
) -> Option<f32> {
    if points.len() < 2 || points.len() != dz.len() {
        return None;
    }
    let dir_len = (dir.0 * dir.0 + dir.1 * dir.1).sqrt().max(1e-6);
    let mut nearest: Option<(f32, f32)> = None;
    for (index, segment) in points.windows(2).enumerate() {
        let a = segment[0];
        let b = segment[1];
        let edge = (b.0 - a.0, b.1 - a.1);
        let edge_len_sq = edge.0 * edge.0 + edge.1 * edge.1;
        if edge_len_sq <= 1e-9
            || (dir.0 * edge.0 + dir.1 * edge.1).abs()
                < 0.82 * dir_len * edge_len_sq.sqrt()
        {
            continue;
        }
        let t = (((point.0 - a.0) * edge.0 + (point.1 - a.1) * edge.1)
            / edge_len_sq)
            .clamp(0.0, 1.0);
        let nearest_point = (a.0 + edge.0 * t, a.1 + edge.1 * t);
        let dx = point.0 - nearest_point.0;
        let dy = point.1 - nearest_point.1;
        let dist_sq = dx * dx + dy * dy;
        if nearest.is_none_or(|(best_dist_sq, _)| dist_sq < best_dist_sq) {
            nearest = Some((dist_sq, dz[index] * (1.0 - t) + dz[index + 1] * t));
        }
    }
    nearest.map(|(_, deck)| deck)
}

const ONEWAY_ARROW_SHAPE: [(f32, f32); 7] = [
    (-6.0, -0.9),
    (0.5, -0.9),
    (0.5, 0.9),
    (-6.0, 0.9),
    (0.5, -3.0),
    (6.0, 0.0),
    (0.5, 3.0),
];
const ONEWAY_ARROW_INDICES: [u32; 9] = [0, 1, 2, 0, 2, 3, 4, 5, 6];

/// Screen-px arrow glyph (shaft + head, +x = travel direction) as
/// zoom-constant anchor+offset vertices. Each vertex samples the source
/// road profile under its own map-plane position so a ramp arrow shares the
/// road's slope rather than hovering on a quantized horizontal card.
#[allow(clippy::too_many_arguments)]
fn append_oneway_arrow(
    anchor: (f32, f32),
    dir_x: f32,
    dir_y: f32,
    render_scale: f32,
    profile_points: &[(f32, f32)],
    profile_dz: Option<&[f32]>,
    final_surface_field: Option<&DzField>,
    fallback_corridors: &[BridgeCorridor],
    surface_param5: f32,
    color: [f32; 4],
    out_vertices: &mut Vec<f32>,
    out_indices: &mut Vec<u32>,
    zbias: &mut f32,
) {
    let base = (out_vertices.len() / VECTOR_FLOATS_PER_VERTEX) as u32;
    for (x, y) in ONEWAY_ARROW_SHAPE {
        let ox = x * dir_x - y * dir_y;
        let oy = x * dir_y + y * dir_x;
        // Tile geometry is baked at render_scale while the shader keeps the
        // glyph screen-constant through fractional zoom. Buckets are at
        // most half a zoom apart; over this six-pixel glyph, the baker's
        // grade limit bounds the profile discrepancy to only a few cm.
        // Terrain itself is sampled at the exact live offset in the shader.
        let sample = (
            anchor.0 + ox / render_scale.max(1e-3),
            anchor.1 + oy / render_scale.max(1e-3),
        );
        let lift_m = if let Some(field) = final_surface_field {
            // Same post-junction-correction, post-smoothing field used by
            // the emitted union face.
            field.sample(sample.0, sample.1)
        } else {
            profile_dz
                .and_then(|dz| {
                    deck_profile_at_point_dir(
                        sample,
                        (dir_x, dir_y),
                        profile_points,
                        dz,
                    )
                })
                .unwrap_or_else(|| {
                    corridor_deck_at_point_dir(
                        sample.0,
                        sample.1,
                        (dir_x, dir_y),
                        fallback_corridors,
                    )
                })
        };
        // param3 = 2.0 identifies a road-surface decal: the shader projects
        // this offset through map rotation/tilt, terrain-samples at the
        // offset point, and interprets param4 as exact signed meters.
        // Match the road's own depth bump, then cancel the icon pass's
        // global +0.04 so only ARROW_DECAL_DEPTH_EPSILON remains.
        let deck_depth = if lift_m > 0.0 {
            0.30 * (lift_m / 2.0).min(1.0)
        } else {
            0.0
        };
        let arrow_param5 = surface_param5 + deck_depth
            - ARROW_ICON_PASS_DEPTH_OFFSET
            + ARROW_DECAL_DEPTH_EPSILON;
        out_vertices.extend_from_slice(&[
            anchor.0, anchor.1, 0.5, 1.0, color[0], color[1], color[2], color[3], 1e6, 0.0,
            ICON_SHAPE_ID, 0.0, ox, oy, 2.0, lift_m, arrow_param5, 16.0, *zbias,
        ]);
    }
    for index in ONEWAY_ARROW_INDICES {
        out_indices.push(base + index);
    }
    *zbias += VECTOR_ZBIAS_STEP;
}

/// Tilt depth of a free-standing symbol: a SMALL camera-ward bias, enough to
/// clear the marker's own ground pixel, small enough that buildings
/// meaningfully in front still occlude. The instanced icon shader carries it
/// as a constant; keep the two in lockstep.
pub const ICON_INSTANCE_DEPTH_BIAS: f32 = 0.35;
/// Clip radius (screen px) of a free-standing symbol: generous, avoids
/// view-edge pop-in. Shader twin in `DrawMapIcon`.
pub const ICON_INSTANCE_CLIP_RADIUS: f32 = 24.0;

/// One symbol placement: the mesh stays on the GPU, this is the 8-float
/// instance record (see `ICON_INSTANCE_FLOATS`). Every placement still takes
/// its own zbias step so draw order matches the vertex-baked path.
fn push_icon_instance(
    groups: &mut Vec<IconInstances>,
    mesh: &IconMesh,
    anchor: (f32, f32),
    screen_offset: (f32, f32),
    scale: f32,
    color: [f32; 4],
    min_zoom: f32,
    zbias: &mut f32,
) {
    let mesh_slot = icon_mesh_slot(mesh);
    let group = match groups.iter_mut().position(|group| group.mesh_slot == mesh_slot) {
        Some(index) => &mut groups[index],
        None => {
            groups.push(IconInstances {
                mesh_slot,
                data: Vec::new(),
            });
            groups.last_mut().unwrap()
        }
    };
    group.data.extend_from_slice(&[
        anchor.0,
        anchor.1,
        screen_offset.0,
        screen_offset.1,
        scale,
        min_zoom,
        *zbias,
        crate::makepad_draw::vector::pack_unorm8x4(color[0], color[1], color[2], color[3]),
    ]);
    *zbias += VECTOR_ZBIAS_STEP;
}

/// Split instance groups into the two icon bands by the record's zoom floor
/// (param4 composite), mirroring `split_icon_band` for vertex streams.
fn split_icon_instance_band(groups: Vec<IconInstances>) -> (Vec<IconInstances>, Vec<IconInstances>) {
    let mut low = Vec::new();
    let mut high = Vec::new();
    for group in groups {
        let mut low_data = Vec::new();
        let mut high_data = Vec::new();
        for record in group.data.chunks_exact(ICON_INSTANCE_FLOATS) {
            if record[5] > ICON_HIGH_BAND_FLOOR {
                high_data.extend_from_slice(record);
            } else {
                low_data.extend_from_slice(record);
            }
        }
        if !low_data.is_empty() {
            low.push(IconInstances {
                mesh_slot: group.mesh_slot,
                data: low_data,
            });
        }
        if !high_data.is_empty() {
            high.push(IconInstances {
                mesh_slot: group.mesh_slot,
                data: high_data,
            });
        }
    }
    (low, high)
}

fn project_way_points_with_nodes(
    node_ids: &[i64],
    nodes: &HashMap<i64, (f64, f64)>,
    tile_key: TileKey,
    tile_origin: Vec2d,
    render_scale: f32,
) -> Vec<(i64, (f32, f32))> {
    let mut out = Vec::with_capacity(node_ids.len());
    let mut last: Option<(f32, f32)> = None;
    // Drop detail below ~a third of a screen pixel AT THE STYLED ZOOM —
    // invisible, but it dominates vertex volume at low buckets (a z14 tile
    // holds ~60K building ring points). Scale-aware, unlike the old fixed
    // source-zoom filter that ate visible corners when overzoomed.
    let min_dist = 0.35 / render_scale.max(0.001);
    let min_dist_sq = min_dist * min_dist;

    for node_id in node_ids {
        let Some((lon, lat)) = nodes.get(node_id).copied() else {
            continue;
        };
        let world = lon_lat_to_world(lon, lat, tile_key.z) - tile_origin;
        let point = (world.x as f32, world.y as f32);

        if let Some(prev) = last {
            let dx = point.0 - prev.0;
            let dy = point.1 - prev.1;
            if dx * dx + dy * dy < min_dist_sq {
                continue;
            }
        }

        out.push((*node_id, point));
        last = Some(point);
    }

    out
}

// --- Local mbtiles loading ---

/// One decoded overlay tile handed to the tile builder: raw MVT bytes plus
/// the ancestor shift (0 = exact zoom) and the quadrant offsets that map the
/// ancestor's local space into this tile's.
pub struct OverlayTileData {
    pub raw: Vec<u8>,
    pub shift: u32,
    pub quadrant_x: u32,
    pub quadrant_y: u32,
    /// 0 = all features, 1 = fast chargers (>=50 kW), 2 = slow chargers.
    pub filter: u8,
    /// Source is a charger overlay: base-map charging_station icons are
    /// suppressed as duplicates while one is active.
    pub has_chargers: bool,
}

fn overlay_zoom_range(reader: &mut MbtilesReader) -> (u32, u32) {
    let metadata = reader.get_metadata().unwrap_or_default();
    let parse = |key: &str, fallback: u32| {
        metadata
            .get(key)
            .and_then(|value| value.trim().parse::<u32>().ok())
            .unwrap_or(fallback)
    };
    (parse("minzoom", 0), parse("maxzoom", 30))
}

/// Build one tile from bytes already supplied by the asynchronous `.mkmap`
/// archive. Local MBTiles bridge/overlay sidecars remain worker-only inputs.
pub fn build_local_tile_from_archive_bytes(
    tile_key: TileKey,
    base: Option<std::sync::Arc<[u8]>>,
    detail: Option<std::sync::Arc<[u8]>>,
    detail_mbtiles_path: Option<&Path>,
    bridge_dz_mbtiles_path: Option<&Path>,
    overlay_paths: &[String],
    theme: &CompiledMapTheme,
    render_zoom: u32,
    buildings_3d: bool,
    build_road_core: bool,
) -> Result<Option<LoadedLocalTile>, String> {
    let mut overlay_tiles = Vec::new();
    for path in overlay_paths.iter().filter(|path| !path.is_empty()) {
        let (file, filter) = match path.split_once('?') {
            Some((file, "fast")) => (file, 1_u8),
            Some((file, "slow")) => (file, 2),
            Some((file, _)) => (file, 0),
            None => (path.as_str(), 0),
        };
        let Ok(mut reader) = MbtilesReader::open(Path::new(file)) else {
            continue;
        };
        let (min_zoom, max_zoom) = overlay_zoom_range(&mut reader);
        if tile_key.z < min_zoom {
            continue;
        }
        let shift = tile_key.z.saturating_sub(max_zoom);
        let fetch_z = tile_key.z - shift;
        let fetch_x = (tile_key.x as u32 >> shift) as i64;
        let fetch_y = (tile_key.y as u32 >> shift) as i64;
        let tms_row = (1_i64 << fetch_z) - 1 - fetch_y;
        if let Ok(Some(raw)) = reader.get_tile_decoded(fetch_z as i64, fetch_x, tms_row) {
            overlay_tiles.push(OverlayTileData {
                raw,
                shift,
                quadrant_x: tile_key.x as u32 - ((fetch_x as u32) << shift),
                quadrant_y: tile_key.y as u32 - ((fetch_y as u32) << shift),
                filter,
                has_chargers: file.contains("chargers"),
            });
        }
    }

    let Some(base) = base else {
        if overlay_tiles.is_empty() {
            return Ok(None);
        }
        let buffers = build_tile_buffers_from_mvt(
            tile_key,
            &[],
            None,
            None,
            false,
            &overlay_tiles,
            theme,
            render_zoom,
            buildings_3d,
            build_road_core,
        )?;
        return Ok(Some(LoadedLocalTile { tile_key, buffers }));
    };

    let mut bridge_dz = bridge_dz_mbtiles_path
        .filter(|path| path.is_file())
        .and_then(|path| MbtilesReader::open(path).ok())
        .and_then(|mut reader| {
            let meta = reader.get_metadata().unwrap_or_default();
            let zoom = meta.get("minzoom").and_then(|z| z.parse::<u32>().ok())?;
            let bounds: Vec<f64> = meta
                .get("bounds")?
                .split(',')
                .filter_map(|value| value.trim().parse().ok())
                .collect();
            (bounds.len() == 4)
                .then_some((reader, zoom, [bounds[0], bounds[1], bounds[2], bounds[3]]))
        });
    let (bridge_dz_raw, bridge_dz_covered) = if let Some((reader, zoom, bounds)) = bridge_dz.as_mut()
    {
        if tile_key.z == *zoom {
            let n = (1_u64 << tile_key.z) as f64;
            let west = tile_key.x as f64 / n * 360.0 - 180.0;
            let east = (tile_key.x as f64 + 1.0) / n * 360.0 - 180.0;
            let lat = |y: f64| {
                (std::f64::consts::PI * (1.0 - 2.0 * y / n))
                    .sinh()
                    .atan()
                    .to_degrees()
            };
            let north = lat(tile_key.y as f64);
            let south = lat(tile_key.y as f64 + 1.0);
            let covered = west >= bounds[0]
                && east <= bounds[2]
                && south >= bounds[1]
                && north <= bounds[3];
            if covered {
                let tms_row = (1_i64 << tile_key.z) - 1 - tile_key.y as i64;
                (
                    reader
                        .get_tile_decoded(tile_key.z as i64, tile_key.x as i64, tms_row)
                        .ok()
                        .flatten(),
                    true,
                )
            } else {
                (None, false)
            }
        } else {
            (None, false)
        }
    } else {
        (None, false)
    };
    let detail_needed = render_zoom >= ICON_MIN_ZOOM
        || render_zoom >= 16
        || (buildings_3d && render_zoom >= BUILDING_3D_MIN_ZOOM)
        || !bridge_dz_covered;
    let detail = if detail.is_none() && detail_needed {
        detail_mbtiles_path
            .filter(|path| path.is_file())
            .and_then(|path| MbtilesReader::open(path).ok())
            .and_then(|mut reader| {
                let tms_row = (1_i64 << tile_key.z) - 1 - tile_key.y as i64;
                reader
                    .get_tile_decoded(tile_key.z as i64, tile_key.x as i64, tms_row)
                    .ok()
                    .flatten()
                    .map(std::sync::Arc::from)
            })
    } else {
        detail
    };
    let buffers = build_tile_buffers_from_mvt(
        tile_key,
        &base,
        detail_needed.then_some(detail.as_deref()).flatten(),
        bridge_dz_raw.as_deref(),
        bridge_dz_covered,
        &overlay_tiles,
        theme,
        render_zoom,
        buildings_3d,
        build_road_core,
    )?;
    Ok(Some(LoadedLocalTile { tile_key, buffers }))
}

pub fn load_local_tile_batch(
    mbtiles_path: &Path,
    detail_mbtiles_path: Option<&Path>,
    bridge_dz_mbtiles_path: Option<&Path>,
    overlay_paths: &[String],
    requested: &[TileKey],
    theme: &CompiledMapTheme,
    render_zoom: u32,
    buildings_3d: bool,
    build_road_core: bool,
) -> Result<(Vec<LoadedLocalTile>, Vec<TileKey>), String> {
    if requested.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }

    // Baked bridge-dz overlay (bridge.md M1/M2): solved per-vertex road
    // elevation. Coverage is the archive's metadata bounds — inside them the
    // solved profile replaces every tag-based deck heuristic, including for
    // tiles that simply have no elevated roads.
    let mut bridge_dz = bridge_dz_mbtiles_path
        .filter(|path| path.is_file())
        .and_then(|path| MbtilesReader::open(path).ok())
        .and_then(|mut reader| {
            let meta = reader.get_metadata().unwrap_or_default();
            let zoom = meta.get("minzoom").and_then(|z| z.parse::<u32>().ok())?;
            let bounds: Vec<f64> = meta
                .get("bounds")?
                .split(',')
                .filter_map(|v| v.trim().parse().ok())
                .collect();
            if bounds.len() != 4 {
                return None;
            }
            Some((reader, zoom, [bounds[0], bounds[1], bounds[2], bounds[3]]))
        });
    let mut fetch_bridge_dz = |tile_key: TileKey| -> (Option<Vec<u8>>, bool) {
        let Some((reader, zoom, bounds)) = bridge_dz.as_mut() else {
            return (None, false);
        };
        if tile_key.z != *zoom {
            return (None, false);
        }
        let n = (1u64 << tile_key.z) as f64;
        let west = tile_key.x as f64 / n * 360.0 - 180.0;
        let east = (tile_key.x as f64 + 1.0) / n * 360.0 - 180.0;
        let lat = |y: f64| {
            (std::f64::consts::PI * (1.0 - 2.0 * y / n)).sinh().atan().to_degrees()
        };
        let north = lat(tile_key.y as f64);
        let south = lat(tile_key.y as f64 + 1.0);
        let covered =
            west >= bounds[0] && east <= bounds[2] && south >= bounds[1] && north <= bounds[3];
        if !covered {
            return (None, false);
        }
        let tms_row = (1_i64 << tile_key.z) - 1 - tile_key.y as i64;
        let raw = reader
            .get_tile_decoded(tile_key.z as i64, tile_key.x as i64, tms_row)
            .ok()
            .flatten();
        (raw, true)
    };

    // Path entries may carry a "?fast" / "?slow" charger-power filter.
    let mut overlay_readers: Vec<(MbtilesReader, u32, u32, u8, bool)> = overlay_paths
        .iter()
        .filter(|path| !path.is_empty())
        .filter_map(|path| {
            let (file, filter) = match path.split_once('?') {
                Some((file, "fast")) => (file, 1u8),
                Some((file, "slow")) => (file, 2),
                Some((file, _)) => (file, 0),
                None => (path.as_str(), 0),
            };
            let has_chargers = file.contains("chargers");
            MbtilesReader::open(Path::new(file))
                .ok()
                .map(|reader| (reader, filter, has_chargers))
        })
        .map(|(mut reader, filter, has_chargers)| {
            let (min_zoom, max_zoom) = overlay_zoom_range(&mut reader);
            (reader, min_zoom, max_zoom, filter, has_chargers)
        })
        .collect();

    let mut fetch_overlays = |tile_key: TileKey| -> Vec<OverlayTileData> {
        let mut out = Vec::new();
        for (reader, min_zoom, max_zoom, filter, has_chargers) in overlay_readers.iter_mut() {
            if tile_key.z < *min_zoom {
                continue;
            }
            let shift = tile_key.z.saturating_sub(*max_zoom);
            let fetch_z = tile_key.z - shift;
            let fetch_x = (tile_key.x as u32 >> shift) as i64;
            let fetch_y = (tile_key.y as u32 >> shift) as i64;
            let tms_row = (1_i64 << fetch_z) - 1 - fetch_y;
            if let Ok(Some(raw)) = reader.get_tile_decoded(fetch_z as i64, fetch_x, tms_row) {
                out.push(OverlayTileData {
                    raw,
                    shift,
                    quadrant_x: (tile_key.x as u32) - ((fetch_x as u32) << shift),
                    quadrant_y: (tile_key.y as u32) - ((fetch_y as u32) << shift),
                    filter: *filter,
                    has_chargers: *has_chargers,
                });
            }
        }
        out
    };

    // The MBTiles archive is already the local, seekable tile cache. Do not
    // duplicate it into millions of generated JSON files.
    let mut loaded = Vec::<LoadedLocalTile>::new();
    let mut decode_failed = Vec::<TileKey>::new();
    let missing = requested;

    let mut reader = TileArchiveReader::open(mbtiles_path)
        .map_err(|err| format!("open {}: {}", mbtiles_path.display(), err))?;

    // Optional all-tag detail overlay. At z14 it may supply fallback bridge
    // corridors outside solved bridge-dz coverage; points, platforms and
    // buildings join at their higher style gates. Per tile below we avoid
    // reading the large blob when solved coverage makes every one of those
    // outputs unnecessary.
    let detail_may_be_needed = render_zoom >= 14;
    // Single-archive mode: base and detail layers live in the SAME file
    // (the pbf-base combined build). Re-reading it through a second reader
    // brotli-decoded every z14 blob twice — reuse the base bytes instead.
    let combined_archive = detail_mbtiles_path == Some(mbtiles_path);
    let mut detail_reader = if detail_may_be_needed && !combined_archive {
        detail_mbtiles_path
            .filter(|path| path.is_file() || TileArchiveReader::is_mkmap_path(path))
            .and_then(|path| TileArchiveReader::open(path).ok())
    } else {
        None
    };

    let mut by_zoom = HashMap::<u32, Vec<TileKey>>::new();
    for key in missing {
        by_zoom.entry(key.z).or_default().push(*key);
    }

    let mut logged_xyz_row_scheme = false;

    for (zoom, mut keys) in by_zoom {
        let tile_count = 1_i64 << zoom;

        if reader.supports_direct_tile_lookup() {
            // Match the writer's block-major rowid order to keep visible-tile
            // reads close together on disk.
            keys.sort_unstable_by_key(|key| {
                (key.y >> 8, key.x >> 8, key.y & 255, key.x & 255)
            });
            let mut unavailable = Vec::new();
            for tile_key in keys {
                let tms_row = tile_count - 1 - tile_key.y as i64;
                let raw = reader
                    .get_tile_decoded(zoom as i64, tile_key.x as i64, tms_row)
                    .map_err(|err| {
                        format!(
                            "read tile z{} x{} y{} from {}: {}",
                            tile_key.z,
                            tile_key.x,
                            tile_key.y,
                            mbtiles_path.display(),
                            err
                        )
                    })?;
                let Some(raw) = raw else {
                    // The base pyramid omits contentless tiles entirely (open
                    // sea has no OSM features), but an overlay may still cover
                    // the spot — the ocean sidecars cover every sea tile. Build
                    // from an empty base so water renders there; a tile no
                    // overlay covers stays a true miss.
                    let overlay_tiles = fetch_overlays(tile_key);
                    if overlay_tiles.is_empty() {
                        unavailable.push(tile_key);
                        continue;
                    }
                    match build_tile_buffers_from_mvt(
                        tile_key,
                        &[],
                        None,
                        None,
                        false,
                        &overlay_tiles,
                        theme,
                        render_zoom,
                        buildings_3d,
                        build_road_core,
                    ) {
                        Ok(buffers) => loaded.push(LoadedLocalTile { tile_key, buffers }),
                        Err(_) => unavailable.push(tile_key),
                    }
                    continue;
                };
                let t_build = ProfileClock::now();
                let (bridge_dz_raw, bridge_dz_covered) = fetch_bridge_dz(tile_key);
                let detail_needed = render_zoom >= ICON_MIN_ZOOM
                    || render_zoom >= 16
                    || (buildings_3d && render_zoom >= BUILDING_3D_MIN_ZOOM)
                    || !bridge_dz_covered;
                let detail_raw = if combined_archive {
                    None // base bytes double as detail below
                } else {
                    detail_needed
                        .then(|| {
                            detail_reader.as_mut().and_then(|reader| {
                                reader
                                    .get_tile_decoded(zoom as i64, tile_key.x as i64, tms_row)
                                    .ok()
                            })
                        })
                        .flatten()
                        .flatten()
                };
                let detail_slice = if combined_archive && detail_needed && tile_key.z >= 14 {
                    Some(raw.as_slice())
                } else {
                    detail_raw.as_deref()
                };
                let overlay_tiles = fetch_overlays(tile_key);

                match build_tile_buffers_from_mvt(
                    tile_key,
                    &raw,
                    detail_slice,
                    bridge_dz_raw.as_deref(),
                    bridge_dz_covered,
                    &overlay_tiles,
                    theme,
                    render_zoom,
                    buildings_3d,
                    build_road_core,
                ) {
                    Ok(buffers) => {
                        // Slow-tile forensics: anything over 150ms is worth a
                        // line — which tile, how many bytes, what it holds.
                        let build_ms = t_build.elapsed_seconds() * 1e3;
                        if build_ms > 150.0 {
                            // Everything needed to replay this exact build
                            // headlessly, plus the ready-to-paste command.
                            let detail_env = if combined_archive {
                                String::new()
                            } else {
                                detail_mbtiles_path
                                    .map(|p| {
                                        format!(
                                            " TILE_PROFILE_DETAIL_ARCHIVE={}",
                                            p.display()
                                        )
                                    })
                                    .unwrap_or_default()
                            };
                            let dz_env = bridge_dz_mbtiles_path
                                .map(|p| format!(" TILE_PROFILE_BRIDGE_DZ={}", p.display()))
                                .unwrap_or_default();
                            let overlays_env = if overlay_paths.is_empty() {
                                String::new()
                            } else {
                                format!(" TILE_PROFILE_OVERLAYS=\"{}\"", overlay_paths.join(";"))
                            };
                            log!(
                                "MapView: SLOW tile z{} x{} y{}: {:.0}ms build rz{} {} raw {} detail {} | stages: {} | repro: MAKEPAD_TRACE=map.tile_profile TILE_PROFILE_ARCHIVE={}{}{}{} TILE_PROFILE_KEYS=\"{},{},{}\" TILE_PROFILE_RENDER_ZOOM={} TILE_PROFILE_3D={} cargo test -p makepad-widgets --features maps --release profile_tile_build -- --ignored --nocapture",
                                tile_key.z,
                                tile_key.x,
                                tile_key.y,
                                build_ms,
                                render_zoom,
                                if buildings_3d { "3D" } else { "flat" },
                                raw.len(),
                                detail_slice.map_or(0, |d| d.len()),
                                buffers.stage_summary,
                                mbtiles_path.display(),
                                detail_env,
                                dz_env,
                                overlays_env,
                                tile_key.z,
                                tile_key.x,
                                tile_key.y,
                                render_zoom,
                                if buildings_3d { "1" } else { "0" },
                            );
                        }
                        loaded.push(LoadedLocalTile { tile_key, buffers });
                    }
                    Err(err) => {
                        decode_failed.push(tile_key);
                        log!(
                            "MapView: failed to decode local mbtile z{} x{} y{}: {}",
                            tile_key.z,
                            tile_key.x,
                            tile_key.y,
                            err
                        );
                    }
                }
            }
            if !unavailable.is_empty() {
                unavailable.sort_unstable();
                log!(
                    "MapView: local mbtiles missing {} tile(s) at z{} sample:{}",
                    unavailable.len(),
                    zoom,
                    format_tile_key_sample(&unavailable, 8)
                );
            }
            continue;
        }

        let mut needed_tms = HashMap::<(i64, i64), TileKey>::new();
        let mut needed_xyz = HashMap::<(i64, i64), TileKey>::new();
        for key in keys {
            let x = key.x as i64;
            let xyz_row = key.y as i64;
            let tms_row = tile_count - 1 - key.y as i64;
            needed_tms.insert((x, tms_row), key);
            needed_xyz.insert((x, xyz_row), key);
        }

        let tiles = reader.get_tiles_at_zoom(zoom as i64).map_err(|err| {
            format!(
                "read zoom {} from {}: {}",
                zoom,
                mbtiles_path.display(),
                err
            )
        })?;

        for tile in tiles {
            let lookup = (tile.tile_column, tile.tile_row);

            let matched = if let Some(tile_key) = needed_tms.remove(&lookup) {
                let xyz_lookup = (tile_key.x as i64, tile_key.y as i64);
                needed_xyz.remove(&xyz_lookup);
                Some((tile_key, false))
            } else if let Some(tile_key) = needed_xyz.remove(&lookup) {
                let tms_lookup = (tile_key.x as i64, tile_count - 1 - tile_key.y as i64);
                needed_tms.remove(&tms_lookup);
                Some((tile_key, true))
            } else {
                None
            };

            let Some((tile_key, used_xyz_row)) = matched else {
                continue;
            };

            if used_xyz_row && !logged_xyz_row_scheme {
                log!("MapView: local mbtiles rows appear XYZ-oriented (matched without TMS row flip)");
                logged_xyz_row_scheme = true;
            }

            let (bridge_dz_raw, bridge_dz_covered) = fetch_bridge_dz(tile_key);
            let detail_needed = render_zoom >= ICON_MIN_ZOOM
                || render_zoom >= 16
                || (buildings_3d && render_zoom >= BUILDING_3D_MIN_ZOOM)
                || !bridge_dz_covered;
            let detail_raw = if combined_archive {
                None // base bytes double as detail below
            } else {
                detail_needed
                    .then(|| {
                        detail_reader.as_mut().and_then(|reader| {
                            let tms_row = tile_count - 1 - tile_key.y as i64;
                            reader
                                .get_tile_decoded(zoom as i64, tile_key.x as i64, tms_row)
                                .ok()
                        })
                    })
                    .flatten()
                    .flatten()
            };
            let overlay_tiles = fetch_overlays(tile_key);
            // Codec-aware decode; a failure falls back to the raw payload,
            // which the magic-byte sniff downstream still handles for the
            // legacy gzip/zlib/raw archives this scan path serves.
            let tile_data = reader
                .decode_tile(&tile.tile_data)
                .unwrap_or_else(|_| tile.tile_data.clone());
            let detail_slice = if combined_archive && detail_needed && tile_key.z >= 14 {
                Some(tile_data.as_slice())
            } else {
                detail_raw.as_deref()
            };
            match build_tile_buffers_from_mvt(
                tile_key,
                &tile_data,
                detail_slice,
                bridge_dz_raw.as_deref(),
                bridge_dz_covered,
                &overlay_tiles,
                theme,
                render_zoom,
                buildings_3d,
                build_road_core,
            ) {
                Ok(buffers) => {
                    loaded.push(LoadedLocalTile { tile_key, buffers });
                }
                Err(err) => {
                    decode_failed.push(tile_key);
                    log!(
                        "MapView: failed to decode local mbtile z{} x{} y{}: {}",
                        tile_key.z,
                        tile_key.x,
                        tile_key.y,
                        err
                    );
                }
            }
        }

        if !needed_tms.is_empty() {
            let mut missing = needed_tms.values().copied().collect::<Vec<_>>();
            missing.sort_unstable();
            log!(
                "MapView: local mbtiles missing {} tile(s) at z{} sample:{}",
                missing.len(),
                zoom,
                format_tile_key_sample(&missing, 8)
            );
        }
    }

    Ok((loaded, decode_failed))
}

#[cfg(test)]
mod local_archive_regression_tests {
    use super::*;
    use makepad_mbtile_reader::MbtilesWriter;

    fn test_mbtiles(name: &str, with_tile: bool) -> std::path::PathBuf {
        test_mbtiles_with_payload(name, with_tile.then_some(&[][..]))
    }

    fn test_mbtiles_with_payload(
        name: &str,
        tile: Option<&[u8]>,
    ) -> std::path::PathBuf {
        // Unique per process AND per call: tests run in parallel, and a
        // wall-clock nonce collides within the same tick.
        static NONCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = format!(
            "{}-{}",
            std::process::id(),
            NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let path = std::path::PathBuf::from(format!("target/{name}-{id}.mbtiles"));
        let mut writer = MbtilesWriter::create(&path).unwrap();
        writer.set_metadata("minzoom", "0");
        writer.set_metadata("maxzoom", "0");
        if let Some(tile) = tile {
            writer.write_tile_encoded(0, 0, 0, tile).unwrap();
        }
        writer.finish().unwrap();
        path
    }

    fn protobuf_varint(mut value: u64, out: &mut Vec<u8>) {
        while value >= 0x80 {
            out.push((value as u8) | 0x80);
            value >>= 7;
        }
        out.push(value as u8);
    }

    fn protobuf_varint_field(field: u64, value: u64, out: &mut Vec<u8>) {
        protobuf_varint(field << 3, out);
        protobuf_varint(value, out);
    }

    fn protobuf_bytes_field(field: u64, value: &[u8], out: &mut Vec<u8>) {
        protobuf_varint((field << 3) | 2, out);
        protobuf_varint(value.len() as u64, out);
        out.extend_from_slice(value);
    }

    fn nonempty_polygon_mvt() -> Vec<u8> {
        let mut geometry = Vec::new();
        for value in [9, 20, 20, 26, 200, 0, 0, 200, 199, 0, 15] {
            protobuf_varint(value, &mut geometry);
        }
        let mut feature = Vec::new();
        protobuf_varint_field(3, 3, &mut feature);
        protobuf_bytes_field(4, &geometry, &mut feature);
        let mut layer = Vec::new();
        protobuf_bytes_field(1, b"natura2000", &mut layer);
        protobuf_bytes_field(2, &feature, &mut layer);
        protobuf_varint_field(5, 4096, &mut layer);
        protobuf_varint_field(15, 2, &mut layer);
        let mut tile = Vec::new();
        protobuf_bytes_field(3, &layer, &mut tile);
        tile
    }

    #[test]
    fn archive_overlay_only_build_ignores_detail_like_legacy_branch() {
        let base = test_mbtiles("archive-missing-base", false);
        let overlay_payload = nonempty_polygon_mvt();
        let overlay = test_mbtiles_with_payload("archive-overlay", Some(&overlay_payload));
        let overlay_paths = vec![overlay.to_string_lossy().into_owned()];
        let key = TileKey { z: 0, x: 0, y: 0 };
        let build = |detail| {
            build_local_tile_from_archive_bytes(
                key,
                None,
                detail,
                None,
                None,
                &overlay_paths,
                &CompiledMapTheme::default(),
                0,
                false,
                false,
            )
            .unwrap()
            .unwrap()
            .buffers
        };
        let (mut legacy, failed) = load_local_tile_batch(
            &base,
            None,
            None,
            &overlay_paths,
            &[key],
            &CompiledMapTheme::default(),
            0,
            false,
            false,
        )
        .unwrap();
        assert!(failed.is_empty());
        let mut legacy = legacy.pop().unwrap().buffers;
        let mut archive = build(None);
        let mut with_unusable_detail = build(Some(vec![0xff, 0xff, 0xff].into()));
        assert!(legacy.feature_count > 0);
        assert!(!legacy.fill_vertices.is_empty());
        legacy.stage_summary.clear();
        archive.stage_summary.clear();
        with_unusable_detail.stage_summary.clear();
        assert_eq!(legacy, archive);
        assert_eq!(archive, with_unusable_detail);
        std::fs::remove_file(base).unwrap();
        std::fs::remove_file(overlay).unwrap();
    }
}

// --- MVT (Mapbox Vector Tile) parsing ---

/// Receives decoded MVT features (tile-local integer geometry + tags).
pub trait MvtSink {
    fn alloc_feature_id(&mut self) -> u64;
    /// Layer-level lazy skip: consulted by `parse_mvt_layer` BEFORE decoding
    /// a layer's features. MVT layers are length-prefixed, so returning
    /// false skips the whole layer's geometry/tag decode. Default: consume
    /// everything.
    fn wants_layer(&self, _layer_name: &str) -> bool {
        true
    }
    /// Tag-key whitelist for a layer, or None for all keys. Resolved once
    /// per layer against the MVT key table, so the string compares are paid
    /// per distinct key — the per-feature loop then skips unwanted pairs
    /// before any String materializes. All-tag detail layers carry dozens
    /// of keys per feature (multilingual names, addr:*) that no consumer
    /// below the icon zooms ever reads.
    fn tag_key_whitelist(&self, _layer_name: &str) -> Option<&'static [&'static str]> {
        None
    }
    fn add_path(
        &mut self,
        tile_key: TileKey,
        extent: u32,
        points: &[(i32, i32)],
        tags: HashMap<String, String>,
        close: bool,
    );
    fn add_point(
        &mut self,
        tile_key: TileKey,
        extent: u32,
        point: (i32, i32),
        tags: HashMap<String, String>,
    );
}

/// Collects MVT features directly in tile-local f32 coordinates with
/// scale-aware vertex thinning — the typed replacement for the old
/// MVT -> Overpass-JSON -> parse round trip.
/// Which MVT layers a collector pass consumes (layer-level lazy skip).
#[derive(Clone, Copy, PartialEq, Eq)]
enum LayerParseFilter {
    /// Everything (legacy behavior; standalone probes).
    All,
    /// Base-tile pass of a combined base+detail archive: the six raw
    /// `osm_*` detail layers are owned by the detail pass (bridge
    /// corridors, micro-POIs, detail buildings/platforms) — wholesale
    /// base ingestion double-styles them (roads exist in `streets` AND
    /// `osm_lines`) and was measured tripling road-union input.
    BaseNoDetailLayers,
    /// Detail pass: ONLY the raw `osm_*` layers, and only the geometry
    /// classes the current render bucket consumes. Point layers are the
    /// bulk of a city-center detail blob (every tree/bench/POI with full
    /// tags) and icons don't render below z17 — parsing them there was
    /// most of a ~110ms/tile constant.
    DetailLayers { points: bool, lines: bool, polygons: bool },
}

const DETAIL_WAY_KEYS: &[&str] = &[
    "layer", "bridge", "tunnel", "highway", "railway", "width", "barrier", "area",
    "name", "attraction", "zoo", "tourism", "public_transport", "landuse", "leisure",
    "natural", "building", "building:part", "height", "building:levels", "min_height",
    "building:min_level", "location", "place", "parking", "surface", "access", "service",
    "link", "rail", "waterway", "ref",
];

const DETAIL_POINT_EXTRA_KEYS: &[&str] = &[
    "amenity", "brand", "craft", "entrance", "historic", "max_kw", "office", "operator",
    "shop", "osm_layer", "kerb", "bus", "shelter",
];

static POINT_KEYS: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();

fn point_keys() -> &'static [&'static str] {
    POINT_KEYS
        .get_or_init(|| {
            let mut keys = DETAIL_WAY_KEYS.to_vec();
            keys.extend_from_slice(DETAIL_POINT_EXTRA_KEYS);
            keys
        })
        .as_slice()
}

pub(super) fn warm_tile_registries() {
    let _ = point_keys();
}

struct MvtLocalCollector {
    layer_filter: LayerParseFilter,
    min_dist_sq: f32,
    next_feature_id: u64,
    ways: Vec<TileWay>,
    points: Vec<((f32, f32), HashMap<String, String>)>,
    /// Baked dense deck profiles keyed (source layer, feature index, path
    /// index) — validated and substituted during collection.
    base_dz: HashMap<(String, u32, u32), BaseDzProfile>,
}

impl MvtLocalCollector {
    fn new(render_scale: f32) -> Self {
        let min_dist = 0.35 / render_scale.max(0.001);
        Self {
            layer_filter: LayerParseFilter::All,
            min_dist_sq: min_dist * min_dist,
            next_feature_id: 1,
            ways: Vec::new(),
            points: Vec::new(),
            base_dz: HashMap::new(),
        }
    }
}

impl MvtSink for MvtLocalCollector {
    fn alloc_feature_id(&mut self) -> u64 {
        let id = self.next_feature_id;
        self.next_feature_id = self.next_feature_id.wrapping_add(1).max(1);
        id
    }

    fn wants_layer(&self, layer_name: &str) -> bool {
        let is_detail = layer_name.starts_with("osm_");
        match self.layer_filter {
            LayerParseFilter::All => true,
            LayerParseFilter::BaseNoDetailLayers => !is_detail,
            LayerParseFilter::DetailLayers { points, lines, polygons } => {
                is_detail
                    && match layer_name {
                        "osm_points" | "osm_relation_points" => points,
                        "osm_lines" | "osm_relation_lines" => lines,
                        "osm_polygons" | "osm_relation_polygons" => polygons,
                        _ => true,
                    }
            }
        }
    }

    fn tag_key_whitelist(&self, _layer_name: &str) -> Option<&'static [&'static str]> {
        // Every key the detail-merge way consumers (corridors, barriers,
        // platforms, attraction/pedestrian/green rings, building extrusion)
        // or downstream styling of their rewritten layers can read. Point
        // features are the one consumer with an open-ended key set
        // (micro_icon_for_tags), so the whitelist only arms when the point
        // layers are off — below the icon zooms, exactly where the tag mass
        // hurts.
        // Point layers add the (bounded) icon-matcher key set: every key
        // icons.rs or the point/attraction routing in merge_detail_features
        // reads. micro POIs carry dozens of address/name-translation tags
        // that nothing consumes — at the kf16 icon horizon this parse was
        // ~140ms/tile with the whitelist forced off.
        #[cfg(not(target_arch = "wasm32"))]
        static NO_WHITELIST: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        #[cfg(not(target_arch = "wasm32"))]
        let no_whitelist =
            *NO_WHITELIST.get_or_init(|| std::env::var_os("MAKEPAD_NO_TAG_WHITELIST").is_some());
        #[cfg(target_arch = "wasm32")]
        let no_whitelist = false;
        if no_whitelist {
            return None;
        }
        match self.layer_filter {
            LayerParseFilter::DetailLayers { points: false, .. } => Some(DETAIL_WAY_KEYS),
            LayerParseFilter::DetailLayers { points: true, .. } => Some(point_keys()),
            _ => None,
        }
    }

    fn add_path(
        &mut self,
        _tile_key: TileKey,
        extent: u32,
        points: &[(i32, i32)],
        mut tags: HashMap<String, String>,
        close: bool,
    ) {
        if points.len() < 2 {
            return;
        }
        // Baked dz joins on (source layer, feature idx, path idx). Its dense
        // geometry replaces the sparse base path only when both raw
        // endpoints still match, so a stale bake fails closed.
        let feature_index = tags.remove(MVT_INTERNAL_FIDX_KEY);
        let path_index = tags.remove(MVT_INTERNAL_PIDX_KEY);
        let scale = TILE_SIZE as f32 / extent.max(1) as f32;
        let profile = if self.base_dz.is_empty() {
            None
        } else {
            match (tags.get("layer"), feature_index.as_deref(), path_index) {
                (Some(layer), Some(fidx), Some(pidx)) => {
                    match (fidx.parse::<u32>(), pidx.parse::<u32>()) {
                        (Ok(fidx), Ok(pidx)) => self
                            .base_dz
                            .get(&(layer.clone(), fidx, pidx))
                            .and_then(|profile| {
                                base_dz_profile_projected_points(
                                    profile, points, scale, close,
                                )
                                .map(|projected| BaseDzProfile {
                                    points: projected,
                                    decks: profile.decks.clone(),
                                })
                            }),
                        _ => None,
                    }
                }
                _ => None,
            }
        };
        let source: Vec<((f32, f32), Option<f32>)> = if let Some(profile) = profile {
            profile
                .points
                .into_iter()
                .zip(profile.decks.into_iter().map(Some))
                .collect()
        } else {
            points
                .iter()
                .map(|&(x, y)| ((x as f32 * scale, y as f32 * scale), None))
                .collect()
        };
        let mut out = Vec::<(f32, f32)>::with_capacity(source.len() + 1);
        let mut out_dz = Vec::<f32>::new();
        let mut last: Option<(f32, f32)> = None;
        for (point, deck) in source {
            if let Some(prev) = last {
                let dx = point.0 - prev.0;
                let dy = point.1 - prev.1;
                if dx * dx + dy * dy < self.min_dist_sq {
                    continue;
                }
            }
            out.push(point);
            if let Some(deck) = deck {
                out_dz.push(deck);
            }
            last = Some(point);
        }
        if out.len() < 2 {
            return;
        }
        if close {
            if out.first() != out.last() {
                out.push(out[0]);
                if !out_dz.is_empty() {
                    out_dz.push(out_dz[0]);
                }
            }
            if out.len() < 4 {
                return;
            }
        }
        let dz = (!out_dz.is_empty() && out_dz.iter().any(|&v| v.abs() > 0.05))
            .then_some(out_dz);
        self.ways.push(TileWay {
            points: out,
            tags,
            closed: close,
            dz,
            fidx: feature_index.as_deref().and_then(|v| v.parse::<u32>().ok()),
        });
    }

    fn add_point(
        &mut self,
        _tile_key: TileKey,
        extent: u32,
        point: (i32, i32),
        tags: HashMap<String, String>,
    ) {
        let scale = TILE_SIZE as f32 / extent.max(1) as f32;
        self.points
            .push(((point.0 as f32 * scale, point.1 as f32 * scale), tags));
    }
}

pub fn decode_vector_tile_payload(raw: &[u8]) -> Result<Vec<u8>, String> {
    if raw.len() >= 2 && raw[0] == 0x1f && raw[1] == 0x8b {
        return gzip_decompress_vec(raw).map_err(|e| format!("gzip decode failed: {}", e));
    }
    if raw.len() >= 2 && raw[0] == 0x78 {
        if let Ok(out) = zlib_decompress_vec(raw) {
            return Ok(out);
        }
    }
    Ok(raw.to_vec())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MvtGeomType {
    Unknown,
    Point,
    LineString,
    Polygon,
}

impl MvtGeomType {
    fn from_u64(value: u64) -> Self {
        match value {
            1 => Self::Point,
            2 => Self::LineString,
            3 => Self::Polygon,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Debug)]
enum MvtValue {
    String(String),
    Float(f32),
    Double(f64),
    Int(i64),
    UInt(u64),
    SInt(i64),
    Bool(bool),
}

impl MvtValue {
    fn to_tag_string(&self) -> String {
        match self {
            Self::String(value) => value.clone(),
            Self::Float(value) => format!("{}", value),
            Self::Double(value) => format!("{}", value),
            Self::Int(value) => format!("{}", value),
            Self::UInt(value) => format!("{}", value),
            Self::SInt(value) => format!("{}", value),
            Self::Bool(value) => {
                if *value {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
        }
    }
}

// --- Baked painter-cascade faces (payload v2-faces-1) ----------------------
// The road painter-order union cascade (compute_visible_regions) is fully
// deterministic per (tile bytes, bridge-dz bytes, style structure, render
// bucket). Field 101 stores its output rings per bucket 14/15/16 so the
// renderer can skip the boolean cascade — the dominant cost of heavy urban
// tiles. Group STYLING (colors/ranks/fields) stays runtime: baked regions
// join the runtime-built PaintGroups by index, guarded by a structural
// signature plus a coordinate checksum. Any mismatch falls back to the
// runtime cascade, so the stream is strictly an accelerator.

thread_local! {
    /// Bake-tool sink: when armed (Some), build_tile_buffers_from_features
    /// stops at the painter cascade and hands its regions back through
    /// here. Thread-local so the offline baker needs no public signature
    /// churn on the tile-build entry points; never armed in the app.
    static FACES_BAKE_SINK: std::cell::RefCell<Option<Option<BakedFacesBucket>>> =
        const { std::cell::RefCell::new(None) };
}

fn faces_bake_sink_armed() -> bool {
    FACES_BAKE_SINK.with(|sink| sink.borrow().is_some())
}

/// Offline bake entry: run the real tile build up to the painter cascade
/// for one bucket and return its captured regions + signature.
pub fn bake_tile_paint_faces(
    tile_key: TileKey,
    raw_tile_data: &[u8],
    detail_tile_data: Option<&[u8]>,
    bridge_dz_tile_data: Option<&[u8]>,
    bridge_dz_covered: bool,
    theme: &CompiledMapTheme,
    bucket: u32,
) -> Option<BakedFacesBucket> {
    try_bake_tile_paint_faces(
        tile_key,
        raw_tile_data,
        detail_tile_data,
        bridge_dz_tile_data,
        bridge_dz_covered,
        theme,
        bucket,
    )
    .ok()
    .flatten()
}

/// Fallible face-bake entry used by the offline worker. The Option wrapper
/// above remains convenient for probes; production baking must retain the
/// tile-build error text so it can skip and report that tile.
pub fn try_bake_tile_paint_faces(
    tile_key: TileKey,
    raw_tile_data: &[u8],
    detail_tile_data: Option<&[u8]>,
    bridge_dz_tile_data: Option<&[u8]>,
    bridge_dz_covered: bool,
    theme: &CompiledMapTheme,
    bucket: u32,
) -> Result<Option<BakedFacesBucket>, String> {
    FACES_BAKE_SINK.with(|sink| *sink.borrow_mut() = Some(None));
    let result = build_tile_buffers_from_mvt(
        tile_key,
        raw_tile_data,
        detail_tile_data,
        bridge_dz_tile_data,
        bridge_dz_covered,
        &[],
        theme,
        bucket,
        // 3D build: the shadow pass (and the detail buildings feeding it)
        // only runs there, and v3 buckets carry its dissolved output. Road
        // tier inputs — the faces signature — are building-independent.
        true,
        true,
    );
    let captured = FACES_BAKE_SINK.with(|sink| sink.borrow_mut().take());
    result?;
    Ok(captured.flatten())
}

const BAKED_FACES_FIELD: u32 = 101;
// v2: `signature` is paint_input_signature (hash of the ring-construction
// INPUT), letting the consumer skip tier ring building on a hit. v1
// streams carried the group-structure hash — semantically incompatible,
// so they are rejected wholesale and fall back to the runtime cascade
// until the archive is rebaked.
// v3: bucket body gains shadow_signature + dissolved shadow shapes +
// grounded footprints after the regions (same running checksum).
const BAKED_FACES_VERSION: u8 = 4;
/// Cascade coordinates are snapped to 1/64 unit (geometry.rs SNAP), so a
/// x64 fixed-point roundtrip is EXACT.
const BAKED_FACES_COORD_SCALE: f64 = 64.0;

fn fnv1a64_step(hash: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *hash ^= b as u64;
        *hash = hash.wrapping_mul(0x100_0000_01b3);
    }
}

/// Deterministic FNV-1a as a std Hasher: the baker and the app must agree
/// across processes, which rules out SipHash's per-process keys. Feeds the
/// derived Hash impls of the tier/style key types.
struct FnvStdHasher(u64);

impl std::hash::Hasher for FnvStdHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x100000001b3);
        }
    }
}

/// Signature of the painter-cascade INPUT, taken BEFORE ring construction:
/// tier order and style identity, every smoothed way's points and dz,
/// plaza rings, and the end sets that decide ribbon caps. If this matches
/// a baked bucket, the bake's regions are valid for the current input and
/// the expensive i_overlay ring construction can be skipped wholesale —
/// the regions carry all geometry, groups only contribute styling, fields
/// and skirt joints, all of which derive from the ways directly.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_input_signature(
    smoothed_tiers: &[(
        RoadSurfaceKey,
        StrokeStyle,
        Vec<(Vec<(f32, f32)>, Option<Vec<f32>>)>,
    )],
    plaza_rings: &[(u32, f32, Vec<(f32, f32)>, Option<Vec<f32>>)],
    tier_joint_ends: &std::collections::HashSet<RoadTierEnd>,
    tunnel_portals: &std::collections::HashSet<(i32, i32)>,
    union_clip: GeoBounds,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = FnvStdHasher(0xcbf2_9ce4_8422_2325);
    let eat_points = |h: &mut FnvStdHasher, points: &[(f32, f32)]| {
        h.write(&(points.len() as u32).to_le_bytes());
        for &(x, y) in points {
            h.write(&x.to_bits().to_le_bytes());
            h.write(&y.to_bits().to_le_bytes());
        }
    };
    let eat_dz = |h: &mut FnvStdHasher, dz: &Option<Vec<f32>>| match dz {
        Some(values) => {
            h.write(&(values.len() as u32).to_le_bytes());
            for value in values {
                h.write(&value.to_bits().to_le_bytes());
            }
        }
        None => h.write(&u32::MAX.to_le_bytes()),
    };
    // NO resolved colors anywhere in this hash: the key carries the
    // theme-stable class/rank/width identity, so recolor-only themes
    // (light/dark/circuit) produce the same signature and share one bake.
    h.write(&(plaza_rings.len() as u32).to_le_bytes());
    for (_, alpha, points, dz) in plaza_rings {
        h.write(&alpha.to_bits().to_le_bytes());
        eat_points(&mut h, points);
        eat_dz(&mut h, dz);
    }
    h.write(&(smoothed_tiers.len() as u32).to_le_bytes());
    for (key, _, ways) in smoothed_tiers {
        key.hash(&mut h);
        h.write(&(ways.len() as u32).to_le_bytes());
        for (points, dz) in ways {
            eat_points(&mut h, points);
            eat_dz(&mut h, dz);
        }
    }
    // HashSet iteration is nondeterministic: sort before hashing.
    let mut joint_ends: Vec<&RoadTierEnd> = tier_joint_ends.iter().collect();
    joint_ends.sort_unstable();
    h.write(&(joint_ends.len() as u32).to_le_bytes());
    for end in joint_ends {
        end.hash(&mut h);
    }
    let mut portals: Vec<&(i32, i32)> = tunnel_portals.iter().collect();
    portals.sort_unstable();
    h.write(&(portals.len() as u32).to_le_bytes());
    for portal in portals {
        portal.hash(&mut h);
    }
    h.write(&union_clip.min.x.to_bits().to_le_bytes());
    h.write(&union_clip.min.y.to_bits().to_le_bytes());
    h.write(&union_clip.max.x.to_bits().to_le_bytes());
    h.write(&union_clip.max.y.to_bits().to_le_bytes());
    h.0
}

/// Structural signature of the cascade INPUT: group order, phases, ranks,
/// fields, ring counts/sizes and per-ring dz classification. Identical
/// between bake and runtime iff the styling structure and tier input match.
pub fn paint_groups_signature(groups: &[PaintGroup]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    fnv1a64_step(&mut hash, &(groups.len() as u32).to_le_bytes());
    for group in groups {
        fnv1a64_step(&mut hash, &[group.phase]);
        fnv1a64_step(&mut hash, &group.rank.to_le_bytes());
        fnv1a64_step(&mut hash, &group.field.to_le_bytes());
        fnv1a64_step(&mut hash, &group.half_width.to_bits().to_le_bytes());
        fnv1a64_step(&mut hash, &(group.rings.len() as u32).to_le_bytes());
        for (ring, min_dz, max_dz) in &group.rings {
            fnv1a64_step(&mut hash, &(ring.len() as u32).to_le_bytes());
            let lifted = (*max_dz >= LIFT_COVER_M) as u8;
            let sunk = (*min_dz <= -LIFT_COVER_M) as u8;
            fnv1a64_step(&mut hash, &[lifted | (sunk << 1)]);
        }
    }
    hash
}

fn write_faces_varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn faces_zigzag(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

fn write_shapes(
    shapes: &[Vec<Vec<[f64; 2]>>],
    out: &mut Vec<u8>,
    checksum: &mut u64,
) {
    write_faces_varint(shapes.len() as u64, out);
    for shape in shapes {
        write_faces_varint(shape.len() as u64, out);
        for ring in shape {
            write_faces_varint(ring.len() as u64, out);
            let mut px = 0i64;
            let mut py = 0i64;
            for p in ring {
                let x = (p[0] * BAKED_FACES_COORD_SCALE).round() as i64;
                let y = (p[1] * BAKED_FACES_COORD_SCALE).round() as i64;
                write_faces_varint(faces_zigzag(x - px), out);
                write_faces_varint(faces_zigzag(y - py), out);
                fnv1a64_step(checksum, &x.to_le_bytes());
                fnv1a64_step(checksum, &y.to_le_bytes());
                px = x;
                py = y;
            }
        }
    }
}

/// One bucket's baked cascade: the group-structure signature it was built
/// against plus each group's visible regions.
pub struct BakedFacesBucket {
    pub bucket: u32,
    pub signature: u64,
    pub regions: Vec<VisibleRegions>,
    /// Reserved v3 compatibility slots. Building and deck shadows are now
    /// derived by the draw-time shadow mask, so v4 writers leave these
    /// fields empty and the parser accepts them only for old archives.
    pub shadow_signature: u64,
    pub shadow_shapes: Vec<Vec<Vec<[f64; 2]>>>,
    pub shadow_footprints: Vec<Vec<[f64; 2]>>,
    /// v4: same-height building blocks pre-dissolved at bake time —
    /// shared interior walls vanish before the extruder ever sees them.
    /// Guarded by its own input signature; empty on v3 streams.
    pub building_signature: u64,
    pub buildings: Vec<BakedBuildingGroup>,
}

#[derive(Clone, Debug)]
pub struct BakedBuildingGroup {
    pub height_m: f32,
    /// 0 = untinted; otherwise the tint color with bit 31 set.
    pub tint: u32,
    pub rings: Vec<Vec<[f64; 2]>>,
}

/// Encode buckets into the complete field-101 bytes to append to a tile.
pub fn encode_baked_faces_field(buckets: &[BakedFacesBucket]) -> Vec<u8> {
    let mut blob = Vec::new();
    blob.push(BAKED_FACES_VERSION);
    write_faces_varint(buckets.len() as u64, &mut blob);
    for bucket in buckets {
        write_faces_varint(bucket.bucket as u64, &mut blob);
        blob.extend_from_slice(&bucket.signature.to_le_bytes());
        let mut body = Vec::new();
        let mut checksum = 0xcbf2_9ce4_8422_2325u64;
        write_faces_varint(bucket.regions.len() as u64, &mut body);
        for region in &bucket.regions {
            write_faces_varint(region.group_index as u64, &mut body);
            write_shapes(&region.main, &mut body, &mut checksum);
            write_shapes(&region.sunk, &mut body, &mut checksum);
            write_shapes(&region.lifted_outlines, &mut body, &mut checksum);
        }
        // v3 shadow section, checksummed with the same running fnv.
        body.extend_from_slice(&bucket.shadow_signature.to_le_bytes());
        write_shapes(&bucket.shadow_shapes, &mut body, &mut checksum);
        // Footprints are a flat ring list: wrap as single-ring shapes.
        let footprint_shapes: Vec<Vec<Vec<[f64; 2]>>> = bucket
            .shadow_footprints
            .iter()
            .map(|ring| vec![ring.clone()])
            .collect();
        write_shapes(&footprint_shapes, &mut body, &mut checksum);
        // v4 building section: pre-dissolved same-height blocks.
        body.extend_from_slice(&bucket.building_signature.to_le_bytes());
        write_faces_varint(bucket.buildings.len() as u64, &mut body);
        for group in &bucket.buildings {
            write_faces_varint(
                zigzag_encode((group.height_m * 16.0).round() as i64),
                &mut body,
            );
            write_faces_varint(group.tint as u64, &mut body);
            write_shapes(
                &[group.rings.clone()],
                &mut body,
                &mut checksum,
            );
        }
        blob.extend_from_slice(&checksum.to_le_bytes());
        write_faces_varint(body.len() as u64, &mut blob);
        blob.extend_from_slice(&body);
    }
    let mut field = Vec::with_capacity(blob.len() + 8);
    write_faces_varint(u64::from(BAKED_FACES_FIELD) << 3 | 2, &mut field);
    write_faces_varint(blob.len() as u64, &mut field);
    field.extend_from_slice(&blob);
    field
}

#[cfg(test)]
#[test]
fn trimmed_v4_and_legacy_v3_face_streams_parse_with_empty_shadow_sections() {
    let bucket = BakedFacesBucket {
        bucket: 16,
        signature: 7,
        regions: Vec::new(),
        shadow_signature: 0,
        shadow_shapes: Vec::new(),
        shadow_footprints: Vec::new(),
        building_signature: 11,
        buildings: Vec::new(),
    };
    let v4 = encode_baked_faces_field(&[bucket]);
    let parsed = parse_baked_faces(&v4, 16).expect("trimmed v4 stream");
    assert!(parsed.shadow_shapes.is_empty());
    assert!(parsed.shadow_footprints.is_empty());
    assert_eq!(parsed.building_signature, 11);

    // A v3 body ends after the same empty shadow sections. Reuse the v4
    // encoder with an empty v4 extension; v3 ignores that zero-valued tail
    // and validates the same coordinate checksum.
    let mut v3 = v4;
    let mut pos = 0;
    let _field_key = read_pb_varint(&v3, &mut pos).unwrap();
    let _blob_len = read_pb_varint(&v3, &mut pos).unwrap();
    v3[pos] = 3;
    let parsed = parse_baked_faces(&v3, 16).expect("legacy v3 stream");
    assert!(parsed.shadow_shapes.is_empty());
    assert_eq!(parsed.building_signature, 0);
    assert!(parsed.buildings.is_empty());
}

fn read_shapes(
    blob: &[u8],
    pos: &mut usize,
    checksum: &mut u64,
) -> Option<Vec<Vec<Vec<[f64; 2]>>>> {
    let shape_count = read_pb_varint(blob, pos).ok()? as usize;
    if shape_count > 1_000_000 {
        return None;
    }
    let mut shapes = Vec::with_capacity(shape_count);
    for _ in 0..shape_count {
        let ring_count = read_pb_varint(blob, pos).ok()? as usize;
        if ring_count > 1_000_000 {
            return None;
        }
        let mut rings = Vec::with_capacity(ring_count);
        for _ in 0..ring_count {
            let pt_count = read_pb_varint(blob, pos).ok()? as usize;
            if pt_count > 4_000_000 {
                return None;
            }
            let mut ring = Vec::with_capacity(pt_count);
            let mut px = 0i64;
            let mut py = 0i64;
            for _ in 0..pt_count {
                px += zigzag_decode(read_pb_varint(blob, pos).ok()?);
                py += zigzag_decode(read_pb_varint(blob, pos).ok()?);
                fnv1a64_step(checksum, &px.to_le_bytes());
                fnv1a64_step(checksum, &py.to_le_bytes());
                ring.push([
                    px as f64 / BAKED_FACES_COORD_SCALE,
                    py as f64 / BAKED_FACES_COORD_SCALE,
                ]);
            }
            rings.push(ring);
        }
        shapes.push(rings);
    }
    Some(shapes)
}

/// Scan a decoded tile for field 101 and decode ONE bucket's regions.
/// None = absent/malformed/checksum-mismatch: caller runs the cascade.
pub fn parse_baked_faces(tile_data: &[u8], want_bucket: u32) -> Option<BakedFacesBucket> {
    let mut pos = 0usize;
    let mut blob: Option<&[u8]> = None;
    while pos < tile_data.len() {
        let key = read_pb_varint(tile_data, &mut pos).ok()?;
        let field = (key >> 3) as u32;
        let wire = (key & 0x7) as u8;
        if field == BAKED_FACES_FIELD && wire == 2 {
            blob = Some(read_pb_len_slice(tile_data, &mut pos).ok()?);
            break;
        }
        skip_pb_field(tile_data, &mut pos, wire).ok()?;
    }
    let blob = blob?;
    let stream_version = *blob.first()?;
    if stream_version != 3 && stream_version != BAKED_FACES_VERSION {
        return None;
    }
    let mut pos = 1usize;
    let bucket_count = read_pb_varint(blob, &mut pos).ok()? as usize;
    for _ in 0..bucket_count.min(16) {
        let bucket = read_pb_varint(blob, &mut pos).ok()? as u32;
        if pos + 16 > blob.len() {
            return None;
        }
        let signature = u64::from_le_bytes(blob[pos..pos + 8].try_into().ok()?);
        pos += 8;
        let stored_checksum = u64::from_le_bytes(blob[pos..pos + 8].try_into().ok()?);
        pos += 8;
        let body_len = read_pb_varint(blob, &mut pos).ok()? as usize;
        if pos + body_len > blob.len() {
            return None;
        }
        if bucket != want_bucket {
            pos += body_len;
            continue;
        }
        let body = &blob[pos..pos + body_len];
        let mut bpos = 0usize;
        let mut checksum = 0xcbf2_9ce4_8422_2325u64;
        let region_count = read_pb_varint(body, &mut bpos).ok()? as usize;
        if region_count > 100_000 {
            return None;
        }
        let mut regions = Vec::with_capacity(region_count);
        for _ in 0..region_count {
            let group_index = read_pb_varint(body, &mut bpos).ok()? as usize;
            let main = read_shapes(body, &mut bpos, &mut checksum)?;
            let sunk = read_shapes(body, &mut bpos, &mut checksum)?;
            let lifted_outlines = read_shapes(body, &mut bpos, &mut checksum)?;
            regions.push(VisibleRegions {
                group_index,
                main,
                sunk,
                lifted_outlines,
            });
        }
        // v3 shadow section.
        if bpos + 8 > body.len() {
            return None;
        }
        let shadow_signature = u64::from_le_bytes(body[bpos..bpos + 8].try_into().ok()?);
        bpos += 8;
        let shadow_shapes = read_shapes(body, &mut bpos, &mut checksum)?;
        let footprint_shapes = read_shapes(body, &mut bpos, &mut checksum)?;
        let shadow_footprints: Vec<Vec<[f64; 2]>> = footprint_shapes
            .into_iter()
            .filter_map(|mut shape| (!shape.is_empty()).then(|| shape.swap_remove(0)))
            .collect();
        let mut building_signature = 0u64;
        let mut buildings = Vec::new();
        if stream_version >= 4 {
            if bpos + 8 > body.len() {
                return None;
            }
            building_signature =
                u64::from_le_bytes(body[bpos..bpos + 8].try_into().ok()?);
            bpos += 8;
            let group_count = read_pb_varint(body, &mut bpos).ok()? as usize;
            if group_count > 100_000 {
                return None;
            }
            for _ in 0..group_count {
                let height_q = zigzag_decode(read_pb_varint(body, &mut bpos).ok()?);
                let tint = read_pb_varint(body, &mut bpos).ok()? as u32;
                let mut shape = read_shapes(body, &mut bpos, &mut checksum)?;
                let rings = if shape.is_empty() {
                    Vec::new()
                } else {
                    shape.swap_remove(0)
                };
                buildings.push(BakedBuildingGroup {
                    height_m: height_q as f32 / 16.0,
                    tint,
                    rings,
                });
            }
        }
        if checksum != stored_checksum {
            return None;
        }
        return Some(BakedFacesBucket {
            bucket,
            signature,
            regions,
            shadow_signature,
            shadow_shapes,
            shadow_footprints,
            building_signature,
            buildings,
        });
    }
    None
}

// --- Baked fill triangulations (payload v2-fills-1) -----------------------
// The pyramid baker (tools/map_tiles/src/native/bake.rs) appends top-level
// protobuf field 100 to qualifying tiles: pre-earcut triangle strips for the
// big polygon features (water_polygons / land / street_polygons, >=96 ring
// vertices). Flat mode substitutes these for the runtime fill tessellation;
// 3D/terrain ignores them (drape re-grids from the rings).

/// One decoded baked-fill feature.
pub struct BakedFillFeature {
    /// tools/map_tiles Layer discriminant of the source polygon layer.
    pub layer_id: u8,
    /// Feature index within that MVT layer, decode order (joins __mp_fidx).
    pub feature_index: u32,
    /// Tile-local units (TILE_SIZE space), full MVT precision, UNCLIPPED:
    /// geometry carries the emitter's 64-MVT-unit tile buffer, so consumers
    /// must clip triangles to their own tile bounds.
    pub verts: Vec<(f32, f32)>,
    /// Decoded triangle list (positive y-down winding).
    pub tris: Vec<[u32; 3]>,
}

/// Maps a renderer-side MVT layer NAME to the baker's Layer discriminant.
/// Mirrors tools/map_tiles/src/native/mvt.rs `Layer`; only the layers the
/// baker emits (bake.rs `baked_fills_field`) are listed.
fn baked_layer_discriminant(layer_name: &str) -> Option<u8> {
    match layer_name {
        "water_polygons" => Some(9),
        "land" => Some(11),
        "street_polygons" => Some(13),
        _ => None,
    }
}

fn zigzag_encode(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

fn zigzag_decode(value: u64) -> i64 {
    ((value >> 1) as i64) ^ -((value & 1) as i64)
}

/// Decode the strip stream exactly like the baker's `strip_to_triangles`:
/// sliding window of 3, even/odd winding alternation by absolute window
/// index, degenerate windows (repeated index) restart the strip.
fn baked_strip_to_triangles(strip: &[u32]) -> Vec<[u32; 3]> {
    let mut out = Vec::with_capacity(strip.len().saturating_sub(2));
    let mut odd = false;
    for window in strip.windows(3) {
        let [a, b, c] = [window[0], window[1], window[2]];
        if a != b && b != c && a != c {
            out.push(if odd { [b, a, c] } else { [a, b, c] });
        }
        odd = !odd;
    }
    out
}

/// Scan a decoded (uncompressed) tile for field 100 and decode the baked
/// fill stream. Returns None when absent or malformed (renderer falls back
/// to runtime tessellation — the stream is strictly an accelerator).
pub fn parse_baked_fills(tile_data: &[u8]) -> Option<Vec<BakedFillFeature>> {
    let mut pos = 0usize;
    let mut blob: Option<&[u8]> = None;
    while pos < tile_data.len() {
        let key = read_pb_varint(tile_data, &mut pos).ok()?;
        let field = (key >> 3) as u32;
        let wire = (key & 0x7) as u8;
        if field == 100 && wire == 2 {
            blob = Some(read_pb_len_slice(tile_data, &mut pos).ok()?);
            break;
        }
        skip_pb_field(tile_data, &mut pos, wire).ok()?;
    }
    let blob = blob?;
    if blob.first() != Some(&1u8) {
        return None;
    }
    // The baker writes MVT extent 4096 for every layer (mvt.rs MVT_EXTENT);
    // baked coordinates are in the same local tile space.
    const BAKED_MVT_EXTENT: f32 = 4096.0;
    let scale = TILE_SIZE as f32 / BAKED_MVT_EXTENT;
    let mut pos = 1usize;
    let count = read_pb_varint(blob, &mut pos).ok()? as usize;
    // Cap against garbage: a tile never has millions of baked features.
    if count > 100_000 {
        return None;
    }
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let layer_id = read_pb_varint(blob, &mut pos).ok()?;
        let feature_index = read_pb_varint(blob, &mut pos).ok()?;
        let vertex_count = read_pb_varint(blob, &mut pos).ok()? as usize;
        let index_count = read_pb_varint(blob, &mut pos).ok()? as usize;
        if vertex_count > 4_000_000 || index_count > 12_000_000 {
            return None;
        }
        let mut verts = vec![(0.0f32, 0.0f32); vertex_count];
        let mut previous = 0i64;
        for vert in verts.iter_mut() {
            previous += zigzag_decode(read_pb_varint(blob, &mut pos).ok()?);
            vert.0 = previous as f32 * scale;
        }
        previous = 0;
        for vert in verts.iter_mut() {
            previous += zigzag_decode(read_pb_varint(blob, &mut pos).ok()?);
            vert.1 = previous as f32 * scale;
        }
        let mut strip = Vec::with_capacity(index_count);
        previous = 0;
        for _ in 0..index_count {
            previous += zigzag_decode(read_pb_varint(blob, &mut pos).ok()?);
            if previous < 0 || previous as usize >= vertex_count {
                return None;
            }
            strip.push(previous as u32);
        }
        let tris = baked_strip_to_triangles(&strip);
        out.push(BakedFillFeature {
            layer_id: layer_id as u8,
            feature_index: feature_index as u32,
            verts,
            tris,
        });
    }
    Some(out)
}

/// Clip a convex polygon against one half-plane (Sutherland–Hodgman step).
/// `inside`/`intersect` in f64 for edge-crossing robustness.
fn clip_poly_axis(
    input: &[(f64, f64)],
    output: &mut Vec<(f64, f64)>,
    axis_x: bool,
    bound: f64,
    keep_less: bool,
) {
    output.clear();
    let inside = |p: (f64, f64)| {
        let v = if axis_x { p.0 } else { p.1 };
        if keep_less {
            v <= bound
        } else {
            v >= bound
        }
    };
    let n = input.len();
    for i in 0..n {
        let cur = input[i];
        let prev = input[(i + n - 1) % n];
        let cur_in = inside(cur);
        let prev_in = inside(prev);
        if cur_in != prev_in {
            // Edge crosses the boundary: emit intersection.
            let (num, den) = if axis_x {
                (bound - prev.0, cur.0 - prev.0)
            } else {
                (bound - prev.1, cur.1 - prev.1)
            };
            let t = if den.abs() > 1e-12 { num / den } else { 0.0 };
            output.push((prev.0 + (cur.0 - prev.0) * t, prev.1 + (cur.1 - prev.1) * t));
        }
        if cur_in {
            output.push(cur);
        }
    }
}

/// Emit a baked fill body into `verts`/`indices` (clears both): shared
/// vertices for fully-inside triangles, per-triangle Sutherland–Hodgman
/// clipping + fan retriangulation at the tile clip rect. Vertex semantics
/// match `Tessellator::fill`'s EvenOdd body: position on the ring,
/// u=0.5 / v=1.0 (opaque interior). Returns the emitted (clipped)
/// triangle area — callers sanity-check it against the feature's net ring
/// area and fall back to runtime tessellation on disagreement.
fn emit_baked_fill_body(
    baked: &BakedFillFeature,
    clip: (f32, f32, f32, f32),
    verts: &mut Vec<VVertex>,
    indices: &mut Vec<u32>,
) -> f64 {
    verts.clear();
    indices.clear();
    let (min_x, min_y, max_x, max_y) = clip;
    for &(x, y) in &baked.verts {
        verts.push(VVertex {
            x,
            y,
            u: 0.5,
            v: 1.0,
            stroke_dist: 0.0,
            clip_radius: 0.0,
        });
    }
    let inside = |x: f32, y: f32| x >= min_x && x <= max_x && y >= min_y && y <= max_y;
    let tri_area = |a: (f64, f64), b: (f64, f64), c: (f64, f64)| -> f64 {
        0.5 * ((b.0 - a.0) * (c.1 - a.1) - (c.0 - a.0) * (b.1 - a.1)).abs()
    };
    let mut area = 0.0f64;
    let mut poly_a: Vec<(f64, f64)> = Vec::with_capacity(8);
    let mut poly_b: Vec<(f64, f64)> = Vec::with_capacity(8);
    for tri in &baked.tris {
        let a = &baked.verts[tri[0] as usize];
        let b = &baked.verts[tri[1] as usize];
        let c = &baked.verts[tri[2] as usize];
        if inside(a.0, a.1) && inside(b.0, b.1) && inside(c.0, c.1) {
            indices.extend_from_slice(&[tri[0], tri[1], tri[2]]);
            area += tri_area(
                (a.0 as f64, a.1 as f64),
                (b.0 as f64, b.1 as f64),
                (c.0 as f64, c.1 as f64),
            );
            continue;
        }
        // Clip against the four rect edges; the result is convex, so a
        // fan preserves the (positive) winding.
        poly_a.clear();
        poly_a.extend([
            (a.0 as f64, a.1 as f64),
            (b.0 as f64, b.1 as f64),
            (c.0 as f64, c.1 as f64),
        ]);
        clip_poly_axis(&poly_a, &mut poly_b, true, min_x as f64, false);
        clip_poly_axis(&poly_b, &mut poly_a, true, max_x as f64, true);
        clip_poly_axis(&poly_a, &mut poly_b, false, min_y as f64, false);
        clip_poly_axis(&poly_b, &mut poly_a, false, max_y as f64, true);
        if poly_a.len() < 3 {
            continue;
        }
        let start = verts.len() as u32;
        for &(x, y) in poly_a.iter() {
            verts.push(VVertex {
                x: x as f32,
                y: y as f32,
                u: 0.5,
                v: 1.0,
                stroke_dist: 0.0,
                clip_radius: 0.0,
            });
        }
        for k in 1..poly_a.len() as u32 - 1 {
            indices.extend_from_slice(&[start, start + k, start + k + 1]);
            area += tri_area(
                poly_a[0],
                poly_a[k as usize],
                poly_a[k as usize + 1],
            );
        }
    }
    area
}

pub fn parse_mvt_tile(
    tile_data: &[u8],
    tile_key: TileKey,
    builder: &mut impl MvtSink,
) -> Result<(), String> {
    let mut pos = 0_usize;
    while pos < tile_data.len() {
        let key = read_pb_varint(tile_data, &mut pos)?;
        let field = (key >> 3) as u32;
        let wire = (key & 0x7) as u8;
        match (field, wire) {
            (3, 2) => {
                let layer = read_pb_len_slice(tile_data, &mut pos)?;
                parse_mvt_layer(layer, tile_key, builder)?;
            }
            _ => skip_pb_field(tile_data, &mut pos, wire)?,
        }
    }
    Ok(())
}

fn parse_mvt_layer(
    layer_data: &[u8],
    tile_key: TileKey,
    builder: &mut impl MvtSink,
) -> Result<(), String> {
    let mut pos = 0_usize;
    let mut layer_name = String::new();
    let mut extent = 4096_u32;
    let mut features = Vec::<&[u8]>::new();
    let mut keys = Vec::<String>::new();
    let mut values = Vec::<MvtValue>::new();

    while pos < layer_data.len() {
        let key = read_pb_varint(layer_data, &mut pos)?;
        let field = (key >> 3) as u32;
        let wire = (key & 0x7) as u8;
        match (field, wire) {
            (1, 2) => {
                let slice = read_pb_len_slice(layer_data, &mut pos)?;
                layer_name = String::from_utf8_lossy(slice).into_owned();
            }
            (2, 2) => features.push(read_pb_len_slice(layer_data, &mut pos)?),
            (3, 2) => {
                let slice = read_pb_len_slice(layer_data, &mut pos)?;
                keys.push(String::from_utf8_lossy(slice).into_owned());
            }
            (4, 2) => {
                let value = parse_mvt_value(read_pb_len_slice(layer_data, &mut pos)?)?;
                values.push(value);
            }
            (5, 0) => extent = read_pb_varint(layer_data, &mut pos)? as u32,
            _ => skip_pb_field(layer_data, &mut pos, wire)?,
        }
    }

    let extent = extent.max(1);
    // Layer-level lazy skip: drop the whole layer before any feature decode
    // when the sink does not consume it in this pass.
    if !builder.wants_layer(&layer_name) {
        return Ok(());
    }
    // Key-level lazy skip: one bool per key-table entry.
    let key_wanted: Option<Vec<bool>> = builder.tag_key_whitelist(&layer_name).map(|whitelist| {
        keys.iter()
            .map(|key| whitelist.contains(&key.as_str()))
            .collect()
    });
    for (feature_index, feature_data) in features.into_iter().enumerate() {
        parse_mvt_feature(
            feature_index as u32,
            feature_data,
            &layer_name,
            &keys,
            &values,
            key_wanted.as_deref(),
            extent,
            tile_key,
            builder,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn parse_mvt_feature(
    feature_index: u32,
    feature_data: &[u8],
    layer_name: &str,
    keys: &[String],
    values: &[MvtValue],
    key_wanted: Option<&[bool]>,
    extent: u32,
    tile_key: TileKey,
    builder: &mut impl MvtSink,
) -> Result<(), String> {
    let mut pos = 0_usize;
    let mut feature_id: Option<u64> = None;
    let mut tag_indexes = Vec::<u32>::new();
    let mut geom_type = MvtGeomType::Unknown;
    let mut geometry_cmds = Vec::<u32>::new();

    while pos < feature_data.len() {
        let key = read_pb_varint(feature_data, &mut pos)?;
        let field = (key >> 3) as u32;
        let wire = (key & 0x7) as u8;
        match (field, wire) {
            (1, 0) => feature_id = Some(read_pb_varint(feature_data, &mut pos)?),
            (2, 2) => {
                let packed = read_pb_len_slice(feature_data, &mut pos)?;
                tag_indexes = read_packed_u32(packed)?;
            }
            (3, 0) => geom_type = MvtGeomType::from_u64(read_pb_varint(feature_data, &mut pos)?),
            (4, 2) => {
                let packed = read_pb_len_slice(feature_data, &mut pos)?;
                geometry_cmds = read_packed_u32(packed)?;
            }
            _ => skip_pb_field(feature_data, &mut pos, wire)?,
        }
    }

    if geom_type == MvtGeomType::Unknown {
        return Ok(());
    }

    let mut tags = HashMap::<String, String>::new();
    for pair in tag_indexes.chunks_exact(2) {
        let key_index = pair[0] as usize;
        let value_index = pair[1] as usize;
        if let Some(wanted) = key_wanted {
            if !wanted.get(key_index).copied().unwrap_or(false) {
                continue;
            }
        }
        let Some(key) = keys.get(key_index) else {
            continue;
        };
        let Some(value) = values.get(value_index) else {
            continue;
        };
        tags.insert(key.clone(), value.to_tag_string());
    }
    normalize_mvt_tags(layer_name, geom_type, &mut tags);

    let paths = decode_mvt_geometry(&geometry_cmds, geom_type)?;
    if geom_type == MvtGeomType::Point {
        if !should_emit_mvt_point_label_feature(&tags) {
            return Ok(());
        }
        for path in paths {
            let Some(point) = path.first().copied() else {
                continue;
            };
            builder.add_point(tile_key, extent, point, tags.clone());
        }
        return Ok(());
    }

    let polygon_feature_key = if geom_type == MvtGeomType::Polygon {
        let raw_id = feature_id.unwrap_or_else(|| builder.alloc_feature_id());
        Some(format!("{}:{}", layer_name, raw_id))
    } else {
        None
    };

    for (ring_index, mut path) in paths.into_iter().enumerate() {
        if path.len() < 2 {
            continue;
        }
        let close = geom_type == MvtGeomType::Polygon;
        if close && path.first().copied() != path.last().copied() {
            if let Some(first) = path.first().copied() {
                path.push(first);
            }
        }
        if close && path.len() < 4 {
            continue;
        }
        let mut path_tags = tags.clone();
        if let Some(feature_key) = &polygon_feature_key {
            path_tags.insert(MVT_INTERNAL_FEATURE_KEY.to_string(), feature_key.clone());
            path_tags.insert(
                MVT_INTERNAL_RING_INDEX_KEY.to_string(),
                ring_index.to_string(),
            );
        }
        // Join keys for the baked base_dz overlay: feature index within
        // the source layer + path index within the feature, in decode
        // order (the bake tool enumerates identically).
        path_tags.insert(MVT_INTERNAL_FIDX_KEY.to_string(), feature_index.to_string());
        path_tags.insert(MVT_INTERNAL_PIDX_KEY.to_string(), ring_index.to_string());
        builder.add_path(tile_key, extent, &path, path_tags, close);
    }

    Ok(())
}

fn normalize_mvt_tags(
    layer_name: &str,
    geom_type: MvtGeomType,
    tags: &mut HashMap<String, String>,
) {
    // The source-layer name OWNS the "layer" key. OSM's own layer=-1/1
    // stacking tag collides with it and silently broke recognition of any
    // layer-tagged feature (the Artis zoo way, bridges, tunnels) — keep
    // the OSM value under "osm_layer" instead.
    if let Some(previous) = tags.insert("layer".to_string(), layer_name.to_string()) {
        if previous != layer_name {
            tags.insert("osm_layer".to_string(), previous);
        }
    }

    match layer_name {
        "building" | "buildings" => {
            tags.entry("building".to_string())
                .or_insert_with(|| "yes".to_string());
        }
        "water" | "water_polygons" | "water_polygons_labels" | "ocean" => {
            if geom_type == MvtGeomType::Polygon {
                tags.entry("natural".to_string())
                    .or_insert_with(|| "water".to_string());
            } else {
                tags.entry("waterway".to_string())
                    .or_insert_with(|| "river".to_string());
            }
        }
        "waterway" | "water_lines" | "water_lines_labels" | "dam_lines" | "pier_lines" => {
            let value = tags
                .get("kind")
                .cloned()
                .or_else(|| tags.get("subclass").cloned())
                .or_else(|| tags.get("class").cloned())
                .unwrap_or_else(|| "river".to_string());
            tags.entry("waterway".to_string()).or_insert(value);
        }
        "transportation"
        | "transportation_name"
        | "road"
        | "streets"
        | "street_polygons"
        | "street_labels"
        | "street_labels_points"
        | "streets_polygons_labels"
        | "bridges"
        | "aerialways"
        | "ferries"
        | "public_transport" => {
            let value = tags
                .get("kind")
                .cloned()
                .or_else(|| tags.get("subclass").cloned())
                .or_else(|| tags.get("class").cloned())
                .unwrap_or_else(|| "residential".to_string());
            tags.entry("highway".to_string())
                .or_insert_with(|| normalize_highway_kind(&value));
        }
        "railway" => {
            tags.entry("railway".to_string())
                .or_insert_with(|| "rail".to_string());
        }
        "park" => {
            tags.entry("leisure".to_string())
                .or_insert_with(|| "park".to_string());
        }
        "landuse" | "landcover" | "land" | "sites" | "pois" => {
            let value = tags
                .get("kind")
                .cloned()
                .or_else(|| tags.get("class").cloned())
                .or_else(|| tags.get("subclass").cloned())
                .unwrap_or_else(|| "residential".to_string());
            if is_leisure_kind(&value) {
                tags.entry("leisure".to_string())
                    .or_insert_with(|| "park".to_string());
            } else {
                tags.entry("landuse".to_string()).or_insert(value);
            }
        }
        _ => {}
    }
}

fn should_emit_mvt_point_label_feature(tags: &HashMap<String, String>) -> bool {
    let Some(layer) = tags.get("layer") else {
        return false;
    };
    match layer.as_str() {
        "addresses" => tags
            .get("housenumber")
            .or_else(|| tags.get("housename"))
            .is_some_and(|value| !value.trim().is_empty()),
        "pois" => select_label_text(tags).is_some(),
        // All-tag detail archive points pass through; the micro-POI
        // whitelist decides downstream what actually draws.
        "osm_points" => true,
        "water_polygons_labels" => select_label_text(tags).is_some(),
        // Geodata overlay point layers (layers.md).
        "chargers" | "stops" => true,
        // Settlement names (city/town/suburb…).
        "place_labels" => select_label_text(tags).is_some(),
        _ => {
            is_road_point_label_layer(layer)
                && tags.contains_key("highway")
                && select_label_text(tags).is_some()
        }
    }
}

fn normalize_highway_kind(kind: &str) -> String {
    match kind {
        "motorway_link" => "motorway".to_string(),
        "trunk_link" => "trunk".to_string(),
        "primary_link" => "primary".to_string(),
        "secondary_link" => "secondary".to_string(),
        "tertiary_link" => "tertiary".to_string(),
        "major_road" => "primary".to_string(),
        "minor_road" => "residential".to_string(),
        "path" => "path".to_string(),
        other => other.to_string(),
    }
}

fn is_leisure_kind(kind: &str) -> bool {
    matches!(
        kind,
        "park" | "garden" | "playground" | "golf_course" | "pitch" | "sports_centre"
    )
}

fn parse_mvt_value(bytes: &[u8]) -> Result<MvtValue, String> {
    let mut pos = 0_usize;
    let mut value = MvtValue::String(String::new());
    while pos < bytes.len() {
        let key = read_pb_varint(bytes, &mut pos)?;
        let field = (key >> 3) as u32;
        let wire = (key & 0x7) as u8;
        match (field, wire) {
            (1, 2) => {
                let slice = read_pb_len_slice(bytes, &mut pos)?;
                value = MvtValue::String(String::from_utf8_lossy(slice).into_owned());
            }
            (2, 5) => value = MvtValue::Float(f32::from_bits(read_pb_fixed32(bytes, &mut pos)?)),
            (3, 1) => value = MvtValue::Double(f64::from_bits(read_pb_fixed64(bytes, &mut pos)?)),
            (4, 0) => value = MvtValue::Int(read_pb_varint(bytes, &mut pos)? as i64),
            (5, 0) => value = MvtValue::UInt(read_pb_varint(bytes, &mut pos)?),
            (6, 0) => value = MvtValue::SInt(zigzag_decode_u64(read_pb_varint(bytes, &mut pos)?)),
            (7, 0) => value = MvtValue::Bool(read_pb_varint(bytes, &mut pos)? != 0),
            _ => skip_pb_field(bytes, &mut pos, wire)?,
        }
    }
    Ok(value)
}

fn decode_mvt_geometry(
    commands: &[u32],
    geom_type: MvtGeomType,
) -> Result<Vec<Vec<(i32, i32)>>, String> {
    let mut parts = Vec::<Vec<(i32, i32)>>::new();
    let mut current = Vec::<(i32, i32)>::new();
    let mut x = 0_i32;
    let mut y = 0_i32;
    let mut index = 0_usize;

    while index < commands.len() {
        let header = commands[index];
        index += 1;
        let command_id = header & 0x7;
        let count = header >> 3;

        match command_id {
            1 => {
                for _ in 0..count {
                    if index + 1 >= commands.len() {
                        return Err("mvt geometry move_to missing arguments".to_string());
                    }
                    x = x.wrapping_add(zigzag_decode_u32(commands[index]));
                    y = y.wrapping_add(zigzag_decode_u32(commands[index + 1]));
                    index += 2;
                    if !current.is_empty() {
                        parts.push(current);
                        current = Vec::new();
                    }
                    current.push((x, y));
                }
            }
            2 => {
                for _ in 0..count {
                    if index + 1 >= commands.len() {
                        return Err("mvt geometry line_to missing arguments".to_string());
                    }
                    x = x.wrapping_add(zigzag_decode_u32(commands[index]));
                    y = y.wrapping_add(zigzag_decode_u32(commands[index + 1]));
                    index += 2;
                    current.push((x, y));
                }
            }
            7 => {
                if geom_type == MvtGeomType::Polygon && !current.is_empty() {
                    let first = current[0];
                    if current.last().copied() != Some(first) {
                        current.push(first);
                    }
                }
            }
            _ => return Err(format!("mvt geometry unknown command {}", command_id)),
        }
    }

    if !current.is_empty() {
        parts.push(current);
    }
    Ok(parts)
}

// --- Protobuf primitives ---

fn zigzag_decode_u32(value: u32) -> i32 {
    ((value >> 1) as i32) ^ (-((value & 1) as i32))
}

fn zigzag_decode_u64(value: u64) -> i64 {
    ((value >> 1) as i64) ^ (-((value & 1) as i64))
}

fn read_packed_u32(bytes: &[u8]) -> Result<Vec<u32>, String> {
    let mut pos = 0_usize;
    let mut out = Vec::new();
    while pos < bytes.len() {
        out.push(read_pb_varint(bytes, &mut pos)? as u32);
    }
    Ok(out)
}

fn read_pb_fixed32(bytes: &[u8], pos: &mut usize) -> Result<u32, String> {
    if *pos + 4 > bytes.len() {
        return Err("unexpected eof reading fixed32".to_string());
    }
    let value = u32::from_le_bytes([
        bytes[*pos],
        bytes[*pos + 1],
        bytes[*pos + 2],
        bytes[*pos + 3],
    ]);
    *pos += 4;
    Ok(value)
}

fn read_pb_fixed64(bytes: &[u8], pos: &mut usize) -> Result<u64, String> {
    if *pos + 8 > bytes.len() {
        return Err("unexpected eof reading fixed64".to_string());
    }
    let value = u64::from_le_bytes([
        bytes[*pos],
        bytes[*pos + 1],
        bytes[*pos + 2],
        bytes[*pos + 3],
        bytes[*pos + 4],
        bytes[*pos + 5],
        bytes[*pos + 6],
        bytes[*pos + 7],
    ]);
    *pos += 8;
    Ok(value)
}

fn read_pb_varint(bytes: &[u8], pos: &mut usize) -> Result<u64, String> {
    let mut value = 0_u64;
    let mut shift = 0_u32;
    while *pos < bytes.len() {
        let byte = bytes[*pos];
        *pos += 1;
        value |= ((byte & 0x7f) as u64) << shift;
        if (byte & 0x80) == 0 {
            return Ok(value);
        }
        shift += 7;
        if shift > 63 {
            return Err("varint too long".to_string());
        }
    }
    Err("unexpected eof reading varint".to_string())
}

fn read_pb_len_slice<'a>(bytes: &'a [u8], pos: &mut usize) -> Result<&'a [u8], String> {
    let len = read_pb_varint(bytes, pos)? as usize;
    if *pos + len > bytes.len() {
        return Err("unexpected eof reading length-delimited field".to_string());
    }
    let slice = &bytes[*pos..*pos + len];
    *pos += len;
    Ok(slice)
}

fn skip_pb_field(bytes: &[u8], pos: &mut usize, wire: u8) -> Result<(), String> {
    match wire {
        0 => {
            let _ = read_pb_varint(bytes, pos)?;
            Ok(())
        }
        1 => {
            if *pos + 8 > bytes.len() {
                return Err("unexpected eof skipping 64-bit field".to_string());
            }
            *pos += 8;
            Ok(())
        }
        2 => {
            let len = read_pb_varint(bytes, pos)? as usize;
            if *pos + len > bytes.len() {
                return Err("unexpected eof skipping length-delimited field".to_string());
            }
            *pos += len;
            Ok(())
        }
        5 => {
            if *pos + 4 > bytes.len() {
                return Err("unexpected eof skipping 32-bit field".to_string());
            }
            *pos += 4;
            Ok(())
        }
        _ => Err(format!("unsupported protobuf wire type {}", wire)),
    }
}

#[cfg(test)]
mod bridge_probe_tests {
    use super::*;

    /// Baked-fill audit (payload v2-fills-1): per baked feature, compare the
    /// clipped baked body triangle area against the runtime tessellation of
    /// the SAME feature's (clipped, min-dist-deduped) rings. NaN scan on the
    /// baked vertices included. Env:
    ///   BAKED_AUDIT_ARCHIVE (default ../local/maps/nl-base-br.mbtiles)
    ///   BAKED_AUDIT_KEYS "z,x,y;..." (default a z10-13 spread)
    /// Run:
    ///   cargo test -p makepad-widgets --features maps --release \
    ///     baked_fill_audit -- --ignored --nocapture
    #[test]
    #[ignore]
    fn baked_fill_audit() {
        let archive = std::env::var("BAKED_AUDIT_ARCHIVE")
            .unwrap_or_else(|_| "../local/maps/nl-base-br.mbtiles".to_string());
        let path = std::path::Path::new(&archive);
        if !path.exists() {
            println!("no archive at {archive}");
            return;
        }
        let keys_spec = std::env::var("BAKED_AUDIT_KEYS").unwrap_or_else(|_| {
            "10,528,340;11,1057,678;12,2103,1346;13,4207,2692;13,4211,2691".into()
        });
        let mut reader = makepad_mbtile_reader::TileArchiveReader::open(path).unwrap();
        let mut total_features = 0usize;
        let mut total_diverged = 0usize;
        for spec in keys_spec.split(';') {
            let mut it = spec.split(',').map(|v| v.trim().parse::<i64>().unwrap());
            let (z, x, y) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());
            let Some(blob) = reader.get_tile(z, x, (1 << z) - 1 - y).ok().flatten() else {
                println!("z{z} {x}/{y}: missing");
                continue;
            };
            let raw = reader.decode_tile(&blob).unwrap();
            let pbf = decode_vector_tile_payload(&raw).unwrap();
            let Some(baked) = parse_baked_fills(&pbf) else {
                println!("z{z} {x}/{y}: no baked stream");
                continue;
            };
            let key = TileKey { z: z as u32, x: x as i32, y: y as i32 };
            // Collect ways exactly like the flat build (render_zoom = z).
            let mut collector = MvtLocalCollector::new(1.0);
            parse_mvt_tile(&pbf, key, &mut collector).unwrap();
            // Group rings per (layer discriminant, fidx).
            let mut rings_of: HashMap<(u8, u32), Vec<FillRing>> = HashMap::new();
            let clip_bounds = tile_clip_bounds(FILL_CLIP_OVERLAP);
            for (order, way) in collector.ways.iter().enumerate() {
                let Some(fidx) = way.fidx else { continue };
                let layer = way.tags.get("layer").map(String::as_str).unwrap_or("");
                let Some(layer_id) = baked_layer_discriminant(layer) else {
                    continue;
                };
                let Some(mut ring_points) = normalize_polygon_ring(&way.points) else {
                    continue;
                };
                if !ring_inside_bounds(&ring_points, clip_bounds) {
                    ring_points = clip_ring_to_rect(&ring_points, clip_bounds);
                    if ring_points.len() < 3 {
                        continue;
                    }
                }
                let signed_area = polygon_signed_area(&ring_points);
                if signed_area.abs() <= POLYGON_AREA_EPSILON {
                    continue;
                }
                let ring_order = way
                    .tags
                    .get(MVT_INTERNAL_RING_INDEX_KEY)
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(order);
                rings_of.entry((layer_id, fidx)).or_default().push(FillRing {
                    order: ring_order,
                    points: ring_points,
                    signed_area,
                });
            }
            if std::env::var("BAKED_AUDIT_DEBUG").is_ok() {
                let mut runtime_list: Vec<((u8, u32), usize, f64)> = rings_of
                    .iter()
                    .map(|(&k, rings)| {
                        (
                            k,
                            rings.iter().map(|r| r.points.len()).sum::<usize>(),
                            rings.iter().map(|r| r.signed_area).sum::<f64>(),
                        )
                    })
                    .collect();
                runtime_list.sort_by_key(|entry| entry.0);
                for (k, nv, area) in runtime_list.iter().take(30) {
                    println!("  runtime layer {} fidx {}: {} ring verts, net area {:.2}", k.0, k.1, nv, area);
                }
                for bake in baked.iter().take(30) {
                    let (mut minx, mut miny, mut maxx, mut maxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                    for &(x, y) in &bake.verts {
                        minx = minx.min(x); miny = miny.min(y); maxx = maxx.max(x); maxy = maxy.max(y);
                    }
                    println!(
                        "  baked layer {} fidx {}: {} verts {} tris bbox ({:.1},{:.1})-({:.1},{:.1})",
                        bake.layer_id, bake.feature_index, bake.verts.len(), bake.tris.len(), minx, miny, maxx, maxy
                    );
                }
            }
            // Sum of per-triangle |area|: both tessellations partition their
            // region (no overlaps), so this equals covered area regardless
            // of triangle winding (the sweep emits mixed orientations).
            let tri_area = |verts: &[VVertex], indices: &[u32]| -> f64 {
                let mut area = 0.0f64;
                for tri in indices.chunks_exact(3) {
                    let a = &verts[tri[0] as usize];
                    let b = &verts[tri[1] as usize];
                    let c = &verts[tri[2] as usize];
                    area += 0.5
                        * ((b.x as f64 - a.x as f64) * (c.y as f64 - a.y as f64)
                            - (c.x as f64 - a.x as f64) * (b.y as f64 - a.y as f64))
                            .abs();
                }
                area
            };
            let mut tess = Tessellator::default();
            tess.set_trust_fill_winding(true);
            let mut path = VectorPath::new();
            let mut verts = Vec::<VVertex>::new();
            let mut indices = Vec::<u32>::new();
            let mut diverged = 0usize;
            let mut nan_verts = 0usize;
            let mut max_rel = 0.0f64;
            let mut guard_rejected = 0usize;
            let mut max_rel_accepted = 0.0f64;
            for bake in &baked {
                total_features += 1;
                // Baked body, clipped like the fill pass at render_zoom = z.
                let clip = tile_clip_rect(FILL_CLIP_OVERLAP);
                emit_baked_fill_body(bake, clip, &mut verts, &mut indices);
                nan_verts += verts
                    .iter()
                    .filter(|v| !v.x.is_finite() || !v.y.is_finite())
                    .count();
                let baked_area = tri_area(&verts, &indices);
                // Runtime tessellation of the same feature's rings
                // (aa = 0: body only, no fringe — the fringe is shared
                // construction in both paths).
                let mut runtime_area = 0.0f64;
                if let Some(rings) = rings_of.get(&(bake.layer_id, bake.feature_index)) {
                    for polygon in classify_polygon_rings(rings, EARCUT_MAX_RINGS) {
                        if polygon.is_empty() {
                            continue;
                        }
                        for ring in &polygon {
                            emit_path(&mut path, ring, true);
                        }
                        tessellate_path_fill(
                            &mut path,
                            &mut tess,
                            &mut verts,
                            &mut indices,
                            LineJoin::Miter,
                            4.0,
                            0.0,
                            false,
                            DEFAULT_FLATTEN_TOLERANCE,
                        );
                        runtime_area += tri_area(&verts, &indices);
                    }
                }
                let denom = runtime_area.max(1e-6);
                let rel = (baked_area - runtime_area).abs() / denom;
                max_rel = max_rel.max(rel);
                // Mirror the shipping fast path's sanity guard: baked
                // partition area vs the feature's net clipped ring area.
                let net_ring_area: f64 = rings_of
                    .get(&(bake.layer_id, bake.feature_index))
                    .map(|rings| rings.iter().map(|r| r.signed_area).sum::<f64>().abs())
                    .unwrap_or(0.0);
                let guard_accepts = net_ring_area > 1e-6
                    && (baked_area - net_ring_area).abs() <= net_ring_area * 0.05;
                if guard_accepts {
                    max_rel_accepted = max_rel_accepted.max(rel);
                } else {
                    guard_rejected += 1;
                }
                if rel > 1e-3 {
                    diverged += 1;
                    if diverged <= 5 {
                        let rings = rings_of
                            .get(&(bake.layer_id, bake.feature_index))
                            .map(Vec::as_slice)
                            .unwrap_or(&[]);
                        let net: f64 = rings.iter().map(|r| r.signed_area).sum();
                        println!(
                            "  DIVERGED layer {} fidx {}: baked {:.3} runtime {:.3} rel {:.2e} ({} verts {} tris, {} rings net {:.3})",
                            bake.layer_id,
                            bake.feature_index,
                            baked_area,
                            runtime_area,
                            rel,
                            bake.verts.len(),
                            bake.tris.len(),
                            rings.len(),
                            net,
                        );
                        if rel > 3e-2 {
                            for (ri, ring) in rings.iter().enumerate().take(12) {
                                println!(
                                    "    ring {ri}: {} pts area {:.3}",
                                    ring.points.len(),
                                    ring.signed_area
                                );
                            }
                        }
                    }
                }
            }
            total_diverged += diverged;
            println!(
                "z{z} {x}/{y}: {} baked features, {} diverged (>1e-3 rel), max rel {:.2e}, {} NaN verts | guard rejects {}, max rel among accepted {:.2e}",
                baked.len(),
                diverged,
                max_rel,
                nan_verts,
                guard_rejected,
                max_rel_accepted,
            );
            assert_eq!(nan_verts, 0, "NaN in baked vertices");
        }
        println!("audit total: {total_features} baked features, {total_diverged} diverged");
    }

    /// Per-layer way census for one tile of TILE_PROFILE_ARCHIVE — used to
    /// attribute build-time regressions to emission changes. Prints layer,
    /// way count, total verts, and per-key street-class breakdown.
    #[test]
    #[ignore]
    fn layer_census() {
        let archive = std::env::var("TILE_PROFILE_ARCHIVE")
            .unwrap_or_else(|_| "../local/maps/nl-base-br.mbtiles".to_string());
        let keys_spec = std::env::var("TILE_PROFILE_KEYS").unwrap_or_else(|_| "14,8414,5386".into());
        let mut reader =
            makepad_mbtile_reader::TileArchiveReader::open(std::path::Path::new(&archive)).unwrap();
        for spec in keys_spec.split(';') {
            let mut it = spec.split(',').map(|v| v.trim().parse::<i64>().unwrap());
            let (z, x, y) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());
            let Some(blob) = reader.get_tile(z, x, (1 << z) - 1 - y).ok().flatten() else {
                println!("missing");
                continue;
            };
            let raw = reader.decode_tile(&blob).unwrap();
            let pbf = decode_vector_tile_payload(&raw).unwrap();
            let key = TileKey { z: z as u32, x: x as i32, y: y as i32 };
            let mut collector = MvtLocalCollector::new(1.0);
            parse_mvt_tile(&pbf, key, &mut collector).unwrap();
            let mut per_layer: std::collections::BTreeMap<String, (usize, usize)> =
                Default::default();
            let mut point_layers: std::collections::BTreeMap<String, usize> = Default::default();
            for (_pos, tags) in &collector.points {
                let layer = tags.get("layer").cloned().unwrap_or_default();
                *point_layers.entry(layer).or_default() += 1;
            }
            for (layer, count) in &point_layers {
                println!("  POINTS {:<24} {:>6}", layer, count);
            }
            // The real micro-POI path: merge_detail_features with the
            // detail whitelist (trees/benches/bins enter here, not via
            // plain parsing).
            {
                let mut points = Vec::new();
                let mut ways = Vec::new();
                let mut corridors = Vec::new();
                merge_detail_features(
                    &raw,
                    key,
                    4.0,
                    true,
                    true,
                    false,
                    &mut points,
                    &mut ways,
                    &mut corridors,
                )
                .unwrap();
                let mut kinds: std::collections::BTreeMap<String, usize> = Default::default();
                for (_pos, tags) in &points {
                    let kind = tags
                        .get("natural")
                        .or_else(|| tags.get("amenity"))
                        .or_else(|| tags.get("highway"))
                        .cloned()
                        .unwrap_or_else(|| "other".into());
                    *kinds.entry(kind).or_default() += 1;
                }
                println!("  DETAIL-MERGE points={} ways={}", points.len(), ways.len());
                for (kind, count) in kinds.iter().take(8) {
                    println!("    micro {:<16} {:>6}", kind, count);
                }
            }
            let mut street_kind: std::collections::BTreeMap<String, usize> = Default::default();
            for way in &collector.ways {
                let layer = way.tags.get("layer").cloned().unwrap_or_default();
                let e = per_layer.entry(layer.clone()).or_default();
                e.0 += 1;
                e.1 += way.points.len();
                if layer.starts_with("street") {
                    let kind = way
                        .tags
                        .get("kind")
                        .or_else(|| way.tags.get("highway"))
                        .cloned()
                        .unwrap_or_default();
                    *street_kind.entry(kind).or_default() += 1;
                }
            }
            println!("z{z} {x}/{y} ({} bytes blob): {} ways", blob.len(), collector.ways.len());
            for (layer, (n, verts)) in &per_layer {
                println!("  {layer:<24} {n:>6} ways {verts:>8} verts");
            }
            for (kind, n) in &street_kind {
                println!("    street kind {kind:<20} {n:>6}");
            }
        }
    }

    /// Baked painter-cascade roundtrip: bake one tile's faces per bucket,
    /// append the field-101 stream to the payload, rebuild through the
    /// normal path, and require BIT-IDENTICAL buffers vs the runtime
    /// cascade (same code builds the faces either way; the stream's 1/64
    /// fixed-point roundtrip is exact).
    #[test]
    #[ignore]
    fn baked_faces_roundtrip() {
        let archive = std::env::var("TILE_PROFILE_ARCHIVE")
            .unwrap_or_else(|_| "../local/maps/nl-base-br.mbtiles".to_string());
        let path = std::path::Path::new(&archive);
        if !path.exists() {
            println!("no archive");
            return;
        }
        let keys = std::env::var("TILE_PROFILE_KEYS")
            .unwrap_or_else(|_| "14,8414,5386;14,8415,5387".into());
        let mut reader = makepad_mbtile_reader::TileArchiveReader::open(path).unwrap();
        let mut dz_reader = std::env::var("TILE_PROFILE_BRIDGE_DZ")
            .ok()
            .map(|p| makepad_mbtile_reader::TileArchiveReader::open(std::path::Path::new(&p)).unwrap());
        let theme = crate::map::style::probe_compiled_theme();
        for spec in keys.split(';') {
            let mut it = spec.split(',').map(|v| v.trim().parse::<i64>().unwrap());
            let (z, x, y) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());
            let raw = reader
                .get_tile_decoded(z, x, (1 << z) - 1 - y)
                .unwrap()
                .unwrap();
            let dz_raw = dz_reader
                .as_mut()
                .and_then(|r| r.get_tile_decoded(z, x, (1 << z) - 1 - y).ok().flatten());
            let dz_covered = dz_reader.is_some();
            let pbf = decode_vector_tile_payload(&raw).unwrap();
            let key = TileKey { z: z as u32, x: x as i32, y: y as i32 };
            for bucket in [14u32, 15, 16] {
                let Some(baked) =
                    bake_tile_paint_faces(key, &pbf, Some(&pbf), dz_raw.as_deref(), dz_covered, &theme, bucket)
                else {
                    println!("z{z} {x}/{y} b{bucket}: no bake");
                    continue;
                };
                let baked2 =
                    bake_tile_paint_faces(key, &pbf, Some(&pbf), dz_raw.as_deref(), dz_covered, &theme, bucket)
                        .unwrap();
                println!(
                    "bake determinism: sig {} regions {} vs sig {} regions {} | encode equal {}",
                    baked.signature,
                    baked.regions.len(),
                    baked2.signature,
                    baked2.regions.len(),
                    encode_baked_faces_field(std::slice::from_ref(&baked))
                        == encode_baked_faces_field(std::slice::from_ref(&baked2)),
                );
                let region_count = baked.regions.len();
                let field = encode_baked_faces_field(&[baked]);
                let mut with_field = pbf.clone();
                with_field.extend_from_slice(&field);
                // Decode sanity through the parse path.
                let parsed = parse_baked_faces(&with_field, bucket).expect("parse baked");
                assert_eq!(parsed.regions.len(), region_count);
                let runtime = build_tile_buffers_from_mvt(
                    key, &pbf, Some(&pbf), dz_raw.as_deref(), dz_covered, &[], &theme, bucket, false, true,
                )
                .unwrap();
                let runtime2 = build_tile_buffers_from_mvt(
                    key, &pbf, Some(&pbf), dz_raw.as_deref(), dz_covered, &[], &theme, bucket, false, true,
                )
                .unwrap();
                let baked_build = build_tile_buffers_from_mvt(
                    key,
                    &with_field,
                    Some(&with_field),
                    dz_raw.as_deref(),
                    dz_covered,
                    &[],
                    &theme,
                    bucket,
                    false,
                    true,
                )
                .unwrap();
                // Emission ORDER is not deterministic run-to-run in
                // production (HashMap-fed passes downstream of the faces;
                // order-derived floats zbias/micro-depth ride along), so
                // equivalence is order-independent: identical vertex
                // MULTISETS with the two order-derived fields masked, and
                // runtime-vs-runtime must show the same equivalence as
                // baked-vs-runtime (proving the bake adds no divergence
                // beyond the pre-existing jitter).
                let vert_multiset = |verts: &[f32]| -> Vec<Vec<u32>> {
                    let mut rows: Vec<Vec<u32>> = verts
                        .chunks_exact(VECTOR_FLOATS_PER_VERTEX)
                        .map(|chunk| {
                            chunk
                                .iter()
                                .enumerate()
                                .filter(|(i, _)| *i != 16 && *i != 18)
                                .map(|(_, v)| v.to_bits())
                                .collect()
                        })
                        .collect();
                    rows.sort_unstable();
                    rows
                };
                for (name, a, b, c) in [
                    (
                        "casing",
                        &runtime.casing_vertices,
                        &runtime2.casing_vertices,
                        &baked_build.casing_vertices,
                    ),
                    (
                        "stroke",
                        &runtime.stroke_vertices,
                        &runtime2.stroke_vertices,
                        &baked_build.stroke_vertices,
                    ),
                    (
                        "fill",
                        &runtime.fill_vertices,
                        &runtime2.fill_vertices,
                        &baked_build.fill_vertices,
                    ),
                ] {
                    let ma = vert_multiset(a);
                    assert_eq!(
                        ma,
                        vert_multiset(b),
                        "{name} runtime-vs-runtime multiset diverged z{z} {x}/{y} b{bucket}"
                    );
                    let mc = vert_multiset(c);
                    if ma != mc {
                        let only_a: Vec<_> = ma.iter().filter(|r| !mc.contains(r)).take(3).collect();
                        let only_c: Vec<_> = mc.iter().filter(|r| !ma.contains(r)).take(3).collect();
                        println!("{name}: rows {} vs {}", ma.len(), mc.len());
                        for r in &only_a {
                            let f: Vec<f32> = r.iter().map(|&b| f32::from_bits(b)).collect();
                            println!("  only-runtime: {:?}", &f[..8.min(f.len())]);
                        }
                        for r in &only_c {
                            let f: Vec<f32> = r.iter().map(|&b| f32::from_bits(b)).collect();
                            println!("  only-baked:   {:?}", &f[..8.min(f.len())]);
                        }
                        panic!("{name} baked-vs-runtime multiset diverged z{z} {x}/{y} b{bucket}");
                    }
                }
                println!(
                    "z{z} {x}/{y} b{bucket}: OK bit-identical, {} regions, field {} bytes",
                    region_count,
                    field.len()
                );
            }
        }
    }

    #[test]
    fn structural_bridge_area_is_3d_only() {
        assert!(!structural_bridge_area_visible("bridges", false));
        assert!(structural_bridge_area_visible("bridges", true));
        assert!(structural_bridge_area_visible("pier_polygons", false));
        assert!(structural_bridge_area_visible("dam_polygons", false));
    }

    #[test]
    fn mode_overlay_appends_cached_road_icons_with_rebased_indices() {
        let mut buffers = TileBuffers {
            pin_hits: Vec::new(),
            fill_indices: Vec::new(),
            fill_vertices: Vec::new(),
            fill_misc_indices: Vec::new(),
            fill_misc_vertices: Vec::new(),
            casing_indices: Vec::new(),
            casing_vertices: Vec::new(),
            stroke_indices: Vec::new(),
            stroke_vertices: Vec::new(),
            // icon_vertices holds GPU-PACKED records post-finalize; the
            // cached road decals stay logical 19-float and pack on append.
            icon_indices: vec![0],
            icon_vertices: vec![1.0; VECTOR_PACKED_FLOATS_PER_VERTEX],
            icon_high_indices: Vec::new(),
            icon_high_vertices: Vec::new(),
            shadow_disc_indices: Vec::new(),
            shadow_disc_vertices: Vec::new(),
            icon_instances: Vec::new(),
            icon_high_instances: Vec::new(),
            fringe_indices: Vec::new(),
            fringe_vertices: Vec::new(),
            fill_3d_indices: Vec::new(),
            fill_3d_vertices: Vec::new(),
            wall_indices: Vec::new(),
            wall_vertices: Vec::new(),
            wall_instances: Vec::new(),
            tree_indices: Vec::new(),
            tree_vertices: Vec::new(),
            tree_cross_indices: Vec::new(),
            tree_cross_vertices: Vec::new(),
            tree_template_indices: Vec::new(),
            tree_template_vertices: Vec::new(),
            tree_cross_template_indices: Vec::new(),
            tree_cross_template_vertices: Vec::new(),
            tree_instances: Vec::new(),
            stage_summary: String::new(),
            road_icon_indices: Vec::new(),
            road_icon_vertices: Vec::new(),
            mode_overlay_only: true,
            feature_count: 0,
            labels: Vec::new(),
            render_zoom: 17,
        };
        let road_vertices = vec![2.0; VECTOR_FLOATS_PER_VERTEX * 2];
        buffers.append_cached_road_icons(&[0, 1], &road_vertices);
        assert_eq!(buffers.icon_indices, vec![0, 1, 2]);
        assert_eq!(
            buffers.icon_vertices.len(),
            VECTOR_PACKED_FLOATS_PER_VERTEX * 3
        );
        assert_eq!(buffers.road_icon_indices, vec![0, 1]);
        assert_eq!(buffers.road_icon_vertices, road_vertices);
    }

    #[test]
    fn built_in_solid_road_colors_share_union_mesh_pipeline() {
        let theme = probe_compiled_theme();
        let zoom_mult = zoom_width_mult(18);
        let ordinary = [
            "motorway",
            "trunk",
            "primary",
            "secondary",
            "busway",
            "tertiary",
            "residential",
            "unclassified",
            "living_street",
            "service",
            "pedestrian",
            // Exercises the compiled wildcard road rule too.
            "other_ordinary_road",
        ];

        for highway in ordinary {
            let tags = HashMap::from([
                ("layer".to_string(), "streets".to_string()),
                ("highway".to_string(), highway.to_string()),
            ]);
            let mut style =
                stroke_style_for_tags(&theme, &tags, 14, 18, zoom_mult, 1.0).unwrap();
            assert!(
                is_solid_road_surface(&tags, &style),
                "{highway} bypassed the solid-road union"
            );

            // Paint identity must not select a different renderer.
            style.center.color ^= 0x00ff_ffff;
            if let Some(casing) = style.casing.as_mut() {
                casing.color ^= 0x005a_a55a;
            }
            assert!(
                is_solid_road_surface(&tags, &style),
                "{highway} union eligibility depended on color"
            );
        }
    }

    #[test]
    fn patterned_and_nonroad_strokes_stay_out_of_road_union() {
        let theme = probe_compiled_theme();
        let zoom_mult = zoom_width_mult(18);
        let tags_for = |highway: &str| {
            HashMap::from([
                ("layer".to_string(), "streets".to_string()),
                ("highway".to_string(), highway.to_string()),
            ])
        };

        let cycleway = tags_for("cycleway");
        let cycleway_style =
            stroke_style_for_tags(&theme, &cycleway, 14, 18, zoom_mult, 1.0).unwrap();
        assert!(!is_solid_road_surface(&cycleway, &cycleway_style));

        let mut rail = tags_for("tram");
        rail.insert("rail".to_string(), "true".to_string());
        let rail_style =
            stroke_style_for_tags(&theme, &rail, 14, 18, zoom_mult, 1.0).unwrap();
        assert!(!is_solid_road_surface(&rail, &rail_style));

        let mut tunnel = tags_for("primary");
        tunnel.insert("tunnel".to_string(), "true".to_string());
        let tunnel_style =
            stroke_style_for_tags(&theme, &tunnel, 14, 18, zoom_mult, 1.0).unwrap();
        assert!(!is_solid_road_surface(&tunnel, &tunnel_style));
        assert_eq!(tunnel_style.center.shape_id, 11.0);
        assert!(
            tunnel_style
                .casing
                .is_none_or(|casing| casing.shape_id == 11.0)
        );
    }

    #[test]
    fn patterned_tunnel_depths_preserve_casing_center_plaza_order() {
        assert!(ROAD_TUNNEL_CASING_DEPTH < ROAD_TUNNEL_CENTER_DEPTH);
        assert!(ROAD_TUNNEL_CENTER_DEPTH < ROAD_SURFACE_PLAZA_DEPTH);
    }

    #[test]
    fn arrow_profile_sampling_is_signed_and_direction_gated() {
        let points = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)];
        let dz = [-2.0, 3.0, 9.0];
        let deck = deck_profile_at_point_dir((5.0, 0.2), (1.0, 0.0), &points, &dz)
            .expect("horizontal source segment");
        assert!((deck - 0.5).abs() < 1e-6);
        let reverse = deck_profile_at_point_dir((5.0, 0.2), (-1.0, 0.0), &points, &dz)
            .expect("reverse travel follows the same surface");
        assert!((reverse - deck).abs() < 1e-6);
    }

    #[test]
    fn road_depth_is_semantic_and_stable_across_face_splits() {
        let micro = 500.0 * DEPTH_MICRO_PER_RANK;
        let casing = road_semantic_param5(0, 1, micro);
        let center = road_semantic_param5(0, 2, micro);
        let repeated_center = (0..32)
            .map(|_| road_semantic_param5(0, 2, micro))
            .collect::<Vec<_>>();

        assert!(repeated_center.iter().all(|depth| *depth == center));
        assert!(casing < center);
        assert!((center - (ROAD_UNION_CENTER_DEPTH + micro)).abs() < 1e-6);
        assert!(
            road_semantic_param5(-1, 2, micro) < ROAD_SURFACE_PLAZA_DEPTH,
            "a solid sunk sheet must remain below all surface paint"
        );
        assert!(ROAD_FRINGE_DEPTH_EPSILON < DEPTH_MICRO_PER_RANK);
    }

    #[test]
    fn oneway_arrow_encodes_exact_per_vertex_surface_and_decal_depth() {
        let points = [(0.0, 0.0), (20.0, 0.0)];
        let dz = [0.0, 1.0];
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut zbias = 0.0;
        let surface_param5 = 0.25;
        append_oneway_arrow(
            (10.0, 0.0),
            1.0,
            0.0,
            1.0,
            &points,
            Some(&dz),
            None,
            &[],
            surface_param5,
            [0.5, 0.5, 0.5, 1.0],
            &mut vertices,
            &mut indices,
            &mut zbias,
        );
        assert_eq!(indices, ONEWAY_ARROW_INDICES);
        assert_eq!(vertices.len(), ONEWAY_ARROW_SHAPE.len() * VECTOR_FLOATS_PER_VERTEX);
        for (vertex, &(x, _)) in vertices
            .chunks_exact(VECTOR_FLOATS_PER_VERTEX)
            .zip(ONEWAY_ARROW_SHAPE.iter())
        {
            let expected_lift = (10.0 + x) / 20.0;
            assert_eq!(vertex[10], ICON_SHAPE_ID);
            assert_eq!(vertex[14], 2.0);
            assert!(
                (vertex[15] - expected_lift).abs() < 1e-6,
                "lift {} != {expected_lift}",
                vertex[15]
            );
            let expected_depth = surface_param5
                + 0.30 * (expected_lift / 2.0).min(1.0)
                - ARROW_ICON_PASS_DEPTH_OFFSET
                + ARROW_DECAL_DEPTH_EPSILON;
            assert!(
                (vertex[16] - expected_depth).abs() < 1e-6,
                "depth {} != {expected_depth}",
                vertex[16]
            );
            let total_arrow_depth = vertex[16] + ARROW_ICON_PASS_DEPTH_OFFSET;
            let own_surface_depth =
                surface_param5 + 0.30 * (expected_lift / 2.0).min(1.0);
            assert!(
                (total_arrow_depth - own_surface_depth - ARROW_DECAL_DEPTH_EPSILON).abs()
                    < 1e-6,
                "icon-pass compensation left a global depth lift"
            );
        }
    }

    fn test_mvt_varint(mut value: u64, out: &mut Vec<u8>) {
        while value >= 0x80 {
            out.push((value as u8) | 0x80);
            value >>= 7;
        }
        out.push(value as u8);
    }

    fn test_mvt_field_varint(field: u64, value: u64, out: &mut Vec<u8>) {
        test_mvt_varint(field << 3, out);
        test_mvt_varint(value, out);
    }

    fn test_mvt_field_bytes(field: u64, value: &[u8], out: &mut Vec<u8>) {
        test_mvt_varint((field << 3) | 2, out);
        test_mvt_varint(value.len() as u64, out);
        out.extend_from_slice(value);
    }

    fn test_base_dz_mvt(points: &[(i32, i32)], dz: &str) -> Vec<u8> {
        let keys = ["L", "F", "P", "dz"];
        let values = ["streets", "7", "2", dz];
        let mut feature = Vec::new();
        let mut tags = Vec::new();
        for index in 0..keys.len() {
            test_mvt_varint(index as u64, &mut tags);
            test_mvt_varint(index as u64, &mut tags);
        }
        test_mvt_field_bytes(2, &tags, &mut feature);
        test_mvt_field_varint(3, 2, &mut feature);
        let mut geometry = Vec::new();
        test_mvt_varint(9, &mut geometry);
        let zigzag = |value: i32| ((value << 1) ^ (value >> 31)) as u64;
        test_mvt_varint(zigzag(points[0].0), &mut geometry);
        test_mvt_varint(zigzag(points[0].1), &mut geometry);
        test_mvt_varint((((points.len() - 1) as u64) << 3) | 2, &mut geometry);
        for pair in points.windows(2) {
            test_mvt_varint(zigzag(pair[1].0 - pair[0].0), &mut geometry);
            test_mvt_varint(zigzag(pair[1].1 - pair[0].1), &mut geometry);
        }
        test_mvt_field_bytes(4, &geometry, &mut feature);

        let mut layer = Vec::new();
        test_mvt_field_bytes(1, b"base_dz", &mut layer);
        test_mvt_field_bytes(2, &feature, &mut layer);
        for key in keys {
            test_mvt_field_bytes(3, key.as_bytes(), &mut layer);
        }
        for value in values {
            let mut message = Vec::new();
            test_mvt_field_bytes(1, value.as_bytes(), &mut message);
            test_mvt_field_bytes(4, &message, &mut layer);
        }
        test_mvt_field_varint(5, 4096, &mut layer);
        test_mvt_field_varint(15, 2, &mut layer);
        let mut tile = Vec::new();
        test_mvt_field_bytes(3, &layer, &mut tile);
        tile
    }

    #[test]
    fn base_dz_codec_keeps_dense_profile_geometry_aligned() {
        let encoded =
            test_base_dz_mvt(&[(-64, 0), (0, 8), (2048, 16), (4096, 0)], "-55,-31,0,55");
        let mut profiles =
            parse_base_dz_map(&encoded, TileKey { z: 14, x: 0, y: 0 }).unwrap();
        let key = ("streets".to_string(), 7, 2);
        let profile = profiles.remove(&key).unwrap();
        assert_eq!(key, ("streets".to_string(), 7, 2));
        assert_eq!(
            profile.points,
            [(-4.0, 0.0), (0.0, 0.5), (128.0, 1.0), (256.0, 0.0)]
        );
        assert_eq!(profile.decks, [-5.5, -3.1000001, 0.0, 5.5]);
    }

    #[test]
    fn base_dz_codec_rejects_malformed_or_non_finite_decks() {
        for dz in ["55,bad,0", "55,NaN,0"] {
            let encoded = test_base_dz_mvt(&[(0, 0), (2048, 0), (4096, 0)], dz);
            let profiles =
                parse_base_dz_map(&encoded, TileKey { z: 14, x: 0, y: 0 }).unwrap();
            assert!(profiles.is_empty(), "accepted malformed dz={dz}");
        }
    }

    #[test]
    fn collector_substitutes_valid_dense_base_dz_and_rejects_stale_endpoints() {
        let key = ("streets".to_string(), 0, 0);
        let profile = BaseDzProfile {
            points: vec![(0.0, 0.0), (64.0, 0.0), (128.0, 0.0)],
            decks: vec![5.5, 0.0, 5.5],
        };
        let tags = || {
            HashMap::from([
                ("layer".to_string(), "streets".to_string()),
                (MVT_INTERNAL_FIDX_KEY.to_string(), "0".to_string()),
                (MVT_INTERNAL_PIDX_KEY.to_string(), "0".to_string()),
            ])
        };
        let raw = [(0, 0), (2048, 0)];

        let mut collector = MvtLocalCollector::new(1.0);
        collector.base_dz.insert(key.clone(), profile.clone());
        collector.add_path(TileKey { z: 14, x: 0, y: 0 }, 4096, &raw, tags(), false);
        assert_eq!(collector.ways[0].points, profile.points);
        assert_eq!(collector.ways[0].dz.as_deref(), Some(profile.decks.as_slice()));

        let mut stale_endpoint = MvtLocalCollector::new(1.0);
        stale_endpoint.base_dz.insert(
            key.clone(),
            BaseDzProfile {
                points: vec![(0.0, 0.0), (64.0, 0.0), (120.0, 0.0)],
                decks: vec![5.5, 0.0, 5.5],
            },
        );
        stale_endpoint.add_path(
            TileKey { z: 14, x: 0, y: 0 },
            4096,
            &raw,
            tags(),
            false,
        );
        assert_eq!(stale_endpoint.ways[0].points, [(0.0, 0.0), (128.0, 0.0)]);
        assert!(stale_endpoint.ways[0].dz.is_none());

        let bent_raw = [(0, 0), (2048, 256), (4096, 0)];
        let mut stale_bend = MvtLocalCollector::new(1.0);
        stale_bend.base_dz.insert(
            key,
            BaseDzProfile {
                points: vec![(0.0, 0.0), (64.0, 0.0), (128.0, 0.0)],
                decks: vec![5.5, 0.0, 5.5],
            },
        );
        stale_bend.add_path(
            TileKey { z: 14, x: 0, y: 0 },
            4096,
            &bent_raw,
            tags(),
            false,
        );
        assert_eq!(
            stale_bend.ways[0].points,
            [(0.0, 0.0), (128.0, 16.0), (256.0, 0.0)]
        );
        assert!(stale_bend.ways[0].dz.is_none());

        let diagonal_raw = [(0, 0), (4096, 4096)];
        let diagonal_profile = BaseDzProfile {
            points: vec![
                (0.0, 0.0),
                (64.0, 64.04),
                (128.0, 127.97),
                (256.0, 256.0),
            ],
            decks: vec![5.5, 3.0, 1.0, 0.0],
        };
        let mut diagonal = MvtLocalCollector::new(1.0);
        diagonal.base_dz.insert(
            ("streets".to_string(), 0, 0),
            diagonal_profile.clone(),
        );
        diagonal.add_path(
            TileKey { z: 14, x: 0, y: 0 },
            4096,
            &diagonal_raw,
            tags(),
            false,
        );
        assert_eq!(
            diagonal.ways[0].dz.as_deref(),
            Some(diagonal_profile.decks.as_slice())
        );
        assert!(
            diagonal.ways[0]
                .points
                .iter()
                .all(|point| (point.0 - point.1).abs() < 1e-4),
            "quantized dense knots were not snapped back to the raw line"
        );

        let closed_profile = BaseDzProfile {
            points: vec![
                (0.0, 0.0),
                (256.0, 0.0),
                (256.0, 256.0),
                (0.0, 256.0),
                (0.0, 0.0),
            ],
            decks: vec![5.5; 5],
        };
        let closed_raw = [(0, 0), (4096, 0), (4096, 4096), (0, 4096)];
        let projected =
            base_dz_profile_projected_points(&closed_profile, &closed_raw, 1.0 / 16.0, true)
                .unwrap();
        assert_eq!(projected.first(), projected.last());
    }

    fn join_test_vertical_key(
        rank: i16,
        width: f32,
        color: u32,
        vertical: RoadVerticalClass,
        layer: i8,
    ) -> RoadSurfaceKey {
        RoadSurfaceKey {
            sort_rank: rank,
            // Tests previously discriminated tiers by color; the key is
            // color-free now, so the color argument doubles as class id.
            class_id: color,
            center_width_bits: width.to_bits(),
            center_depth_micro_bits: (rank as f32 * DEPTH_MICRO_PER_RANK).to_bits(),
            casing_width_bits: NO_CASING_BITS,
            casing_depth_micro_bits: NO_CASING_BITS,
            vertical,
            layer,
        }
    }

    fn join_test_key(rank: i16, width: f32, color: u32) -> RoadSurfaceKey {
        join_test_vertical_key(rank, width, color, RoadVerticalClass::Surface, 0)
    }

    #[test]
    fn physical_surface_key_separates_tunnel_surface_and_deck() {
        let theme = probe_compiled_theme();
        let surface_tags = HashMap::from([
            ("layer".to_string(), "streets".to_string()),
            ("highway".to_string(), "primary".to_string()),
        ]);
        let style = stroke_style_for_tags(
            &theme,
            &surface_tags,
            14,
            18,
            zoom_width_mult(18),
            1.0,
        )
        .unwrap();
        let surface = RoadSurfaceKey::from_way(style, &surface_tags, Some(&[0.0, 0.0]));

        let mut tunnel_tags = surface_tags.clone();
        tunnel_tags.insert("tunnel".to_string(), "true".to_string());
        // Positive contradictory layer is normalized exactly like the baker.
        tunnel_tags.insert("osm_layer".to_string(), "2".to_string());
        let tunnel = RoadSurfaceKey::from_way(style, &tunnel_tags, Some(&[-0.8, 0.0]));

        let mut bridge_tags = surface_tags.clone();
        bridge_tags.insert("bridge".to_string(), "true".to_string());
        // Negative contradictory layer is normalized to the bridge band.
        bridge_tags.insert("osm_layer".to_string(), "-1".to_string());
        let bridge = RoadSurfaceKey::from_way(style, &bridge_tags, Some(&[0.0, 5.5]));

        assert_eq!(surface.vertical, RoadVerticalClass::Surface);
        assert_eq!((tunnel.vertical, tunnel.layer), (RoadVerticalClass::Sunk, -1));
        assert_eq!(
            (bridge.vertical, bridge.layer),
            (RoadVerticalClass::Elevated, 1)
        );
        assert!(!surface.grade_compatible(tunnel));
        assert!(surface.grade_compatible(bridge));

        let upper_bridge = RoadSurfaceKey { layer: 2, ..bridge };
        assert!(!bridge.grade_compatible(upper_bridge));

        let sheets = std::collections::HashSet::from([surface, tunnel, bridge, upper_bridge]);
        assert_eq!(sheets.len(), 4);
    }

    fn join_test_meta(
        family: RoadJoinFamily,
        is_link: bool,
        is_bridge: bool,
    ) -> RoadJoinMeta {
        RoadJoinMeta {
            family,
            is_link,
            is_bridge,
        }
    }

    #[test]
    fn steps_and_footways_share_a_join_family() {
        let tags = |highway: &str| {
            HashMap::from([("highway".to_string(), highway.to_string())])
        };
        assert_eq!(
            RoadJoinMeta::from_tags(&tags("steps")).family,
            RoadJoinMeta::from_tags(&tags("footway")).family
        );
        assert_eq!(
            RoadJoinMeta::from_tags(&tags("steps")).family,
            RoadJoinFamily::Footway
        );
    }

    #[test]
    fn endpoint_to_through_merge_inherits_deck_with_smooth_grade() {
        let link_key = join_test_key(10, 2.0, 1);
        let main_key = join_test_key(20, 4.0, 2);
        let crossing_key = join_test_key(11, 2.0, 3);
        let link_meta = join_test_meta(RoadJoinFamily::Motorway, true, false);
        let main_meta = join_test_meta(RoadJoinFamily::Motorway, false, false);
        let ways = vec![
            RoadTierJoinWay {
                key: link_key,
                way_index: 0,
                // Start endpoint approaches the through road almost
                // collinearly and its cap overlaps the mainline ribbon.
                points: vec![(0.0, 0.0), (-4.0, -0.2), (-10.0, -0.2)],
                dz: vec![2.0, 2.0, 2.0],
                half_width: 1.0,
                meta: link_meta,
            },
            RoadTierJoinWay {
                key: crossing_key,
                way_index: 0,
                // A perpendicular fork at the same place must retain its
                // independent profile.
                points: vec![(0.0, 0.0), (0.0, -4.0), (0.0, -8.0)],
                dz: vec![2.0, 2.0, 2.0],
                half_width: 1.0,
                meta: link_meta,
            },
            RoadTierJoinWay {
                key: main_key,
                way_index: 0,
                points: vec![(-8.0, 0.05), (8.0, 0.05)],
                dz: vec![4.0, 4.0],
                half_width: 2.0,
                meta: main_meta,
            },
        ];

        let corrections = endpoint_to_through_grade_corrections(&ways);
        assert_eq!(corrections.len(), 1);
        assert_eq!(corrections[0].end, (link_key, 0, true));
        assert!((corrections[0].target_dz - 4.0).abs() < 1e-5);

        let mut corrected = ways[0].dz.clone();
        apply_endpoint_grade_correction(
            &ways[0].points,
            &mut corrected,
            true,
            corrections[0].target_dz,
            ways[0].half_width,
        );
        assert!((corrected[0] - 4.0).abs() < 1e-5);
        assert!(corrected[1] > 2.0 && corrected[1] < 4.0);
        assert!((corrected[2] - 2.0).abs() < 1e-5);

        let mut already_on_grade = vec![2.0, 4.0, 4.0];
        apply_endpoint_grade_correction(
            &ways[0].points,
            &mut already_on_grade,
            true,
            4.0,
            ways[0].half_width,
        );
        assert_eq!(already_on_grade, vec![4.0, 4.0, 4.0]);

        // Typed link-to-mainline semantics admit a larger correction at the
        // same tightly aligned, interior merge.
        let mut high_mainline = ways.clone();
        high_mainline[0].dz.fill(0.0);
        high_mainline[2].dz.fill(8.0);
        let high_corrections = endpoint_to_through_grade_corrections(&high_mainline);
        assert_eq!(high_corrections.len(), 1);
        assert_eq!(high_corrections[0].end, (link_key, 0, true));
        assert!((high_corrections[0].target_dz - 8.0).abs() < 1e-5);

        // Without trusted link semantics the conservative 3 m cap remains:
        // a merely aligned upper/lower pair must not be joined.
        let mut grade_separated = ways.clone();
        grade_separated[0].dz.fill(0.0);
        grade_separated[2].dz.fill(8.0);
        grade_separated[0].meta.is_link = false;
        assert!(endpoint_to_through_grade_corrections(&grade_separated).is_empty());
    }

    #[test]
    fn high_typed_link_lowers_to_bridge_mainline_and_becomes_flush() {
        let link_key = join_test_key(716, 0.65625, 1);
        let main_key = join_test_key(726, 0.9375, 2);
        let link_meta = join_test_meta(RoadJoinFamily::Motorway, true, true);
        let main_meta = join_test_meta(RoadJoinFamily::Motorway, false, true);
        let source = RoadTierJoinWay {
            key: link_key,
            way_index: 0,
            points: vec![
                (119.8125, 4.9375),
                (119.7, 2.5),
                (119.5, 0.0),
                (119.25, -5.0),
            ],
            dz: vec![5.5; 4],
            half_width: 0.39375,
            meta: link_meta,
        };
        let target = RoadTierJoinWay {
            key: main_key,
            way_index: 0,
            points: vec![(119.3125, -4.0), (120.0, 9.0)],
            dz: vec![5.2, 4.7],
            half_width: 0.5625,
            meta: main_meta,
        };

        let corrections =
            endpoint_to_through_grade_corrections(&[source.clone(), target.clone()]);
        assert_eq!(corrections.len(), 1);
        assert_eq!(corrections[0].end, (link_key, 0, true));
        assert!((corrections[0].target_dz - 4.85625).abs() < 0.001);

        let mut corrected_dz = source.dz.clone();
        apply_endpoint_grade_correction(
            &source.points,
            &mut corrected_dz,
            true,
            corrections[0].target_dz,
            source.half_width,
        );
        assert!((corrected_dz[0] - corrections[0].target_dz).abs() < 1e-5);
        assert!(corrected_dz[1] > corrections[0].target_dz && corrected_dz[1] < 5.5);
        assert!((corrected_dz[2] - 5.5).abs() < 1e-5);

        let mut corrected_source = source.clone();
        corrected_source.dz = corrected_dz;
        let flush =
            endpoint_to_through_flush_ends(&[corrected_source.clone(), target.clone()]);
        assert!(flush.contains(&(link_key, 0, true)));

        // Geometry alone is not enough to lower one independent deck onto
        // another; the link-to-mainline relationship is the authority.
        corrected_source.meta.is_link = false;
        corrected_source.dz = source.dz;
        assert!(endpoint_to_through_grade_corrections(&[
            corrected_source,
            target
        ])
        .is_empty());
    }

    #[test]
    fn same_height_endpoint_to_interior_is_a_flush_joint() {
        let link_key = join_test_key(10, 2.0, 1);
        let main_key = join_test_key(20, 4.0, 2);
        let motorway_link = join_test_meta(RoadJoinFamily::Motorway, true, false);
        let motorway = join_test_meta(RoadJoinFamily::Motorway, false, false);
        let source = RoadTierJoinWay {
            key: main_key,
            way_index: 0,
            points: vec![(0.0, 0.0), (0.0, -8.0)],
            dz: vec![8.5, 8.5],
            half_width: 2.0,
            meta: motorway,
        };
        let target = RoadTierJoinWay {
            key: link_key,
            way_index: 0,
            points: vec![(-1.0, 8.0), (0.0, 0.0), (1.0, 8.0)],
            dz: vec![8.5, 8.5, 8.5],
            half_width: 1.0,
            meta: motorway_link,
        };

        let flush = endpoint_to_through_flush_ends(&[source.clone(), target.clone()]);
        assert_eq!(flush.len(), 1);
        assert!(flush.contains(&(main_key, 0, true)));
        // This is topology-only: rank/width direction must not invent a
        // grade correction when both profiles already agree.
        assert!(endpoint_to_through_grade_corrections(&[
            source.clone(),
            target.clone()
        ])
        .is_empty());

        let mut mismatch = target.clone();
        mismatch.dz.fill(8.0);
        assert!(endpoint_to_through_flush_ends(&[source.clone(), mismatch]).is_empty());

        let mut perpendicular = target.clone();
        perpendicular.points = vec![(-8.0, 0.0), (0.0, 0.0), (8.0, 0.0)];
        assert!(
            endpoint_to_through_flush_ends(&[source.clone(), perpendicular]).is_empty()
        );

        let mut offset = target.clone();
        offset.points = vec![(0.0, 8.0), (1.0, 0.0), (2.0, 8.0)];
        assert!(endpoint_to_through_flush_ends(&[source.clone(), offset]).is_empty());

        let mut other_family = target.clone();
        other_family.meta.family = RoadJoinFamily::Primary;
        assert!(
            endpoint_to_through_flush_ends(&[source.clone(), other_family]).is_empty()
        );

        let mut terminal = target;
        terminal.points = vec![(0.0, 0.0), (0.0, 8.0)];
        terminal.dz = vec![8.5, 8.5];
        assert!(endpoint_to_through_flush_ends(&[source, terminal]).is_empty());
    }

    #[test]
    fn exact_cross_family_link_fork_is_flush_only_with_through_and_height_proof() {
        let link_key = join_test_key(10, 2.0, 1);
        let trunk_key = join_test_key(20, 4.0, 2);
        let source = RoadTierJoinWay {
            key: link_key,
            way_index: 0,
            // An angled motorway link starts at the exact interior node of
            // a trunk. Its tangent is deliberately outside the generalized
            // >=0.90 gate: exact shared-node topology is the authority.
            points: vec![(0.0, 0.0), (1.0, 5.0), (2.0, 10.0)],
            dz: vec![5.5, 5.5, 5.5],
            half_width: 1.0,
            meta: join_test_meta(RoadJoinFamily::Motorway, true, true),
        };
        let through = RoadTierJoinWay {
            key: trunk_key,
            way_index: 0,
            points: vec![(-8.0, -7.0), (0.0, 0.0), (8.0, 6.0)],
            dz: vec![5.5, 5.5, 5.5],
            half_width: 2.0,
            meta: join_test_meta(RoadJoinFamily::Trunk, false, true),
        };

        let flush = endpoint_to_through_flush_ends(&[source.clone(), through.clone()]);
        assert_eq!(flush.len(), 1);
        assert!(flush.contains(&(link_key, 0, true)));

        // MVT buffer geometry just outside the nominal tile is still
        // rendered inside the road clip padding. Its copy must classify
        // the same flush join as the owning neighbor tile, or the padded
        // copy leaves a cap/fascia seam on top.
        let translate_y = |way: &RoadTierJoinWay, dy: f32| {
            let mut shifted = way.clone();
            for point in &mut shifted.points {
                point.1 += dy;
            }
            shifted
        };
        let inside_padding = -ROAD_PAINT_CLIP_PADDING + 0.1;
        let padded_source = translate_y(&source, inside_padding);
        let padded_through = translate_y(&through, inside_padding);
        let padded_flush =
            endpoint_to_through_flush_ends(&[padded_source, padded_through]);
        assert!(padded_flush.contains(&(link_key, 0, true)));

        let outside_padding = -ROAD_PAINT_CLIP_PADDING - 0.1;
        let clipped_source = translate_y(&source, outside_padding);
        let clipped_through = translate_y(&through, outside_padding);
        assert!(
            endpoint_to_through_flush_ends(&[clipped_source, clipped_through])
                .is_empty()
        );

        // A generalized near miss is not exact topology; cross-family
        // geometry remains under the conservative path below.
        let mut nearby = source.clone();
        nearby.points[0] = (0.21, 0.0);
        assert!(endpoint_to_through_flush_ends(&[nearby, through.clone()]).is_empty());

        // A target ending at the shared node is not a through road.
        let mut terminal = through.clone();
        terminal.points = vec![(-8.0, -7.0), (0.0, 0.0)];
        terminal.dz = vec![5.5, 5.5];
        assert!(
            endpoint_to_through_flush_ends(&[source.clone(), terminal]).is_empty()
        );

        // Nor is an interior cusp whose two arms do not continue through
        // the node in opposite directions.
        let mut cusp = through.clone();
        cusp.points = vec![(-8.0, -7.0), (0.0, 0.0), (-7.0, -8.0)];
        assert!(endpoint_to_through_flush_ends(&[source.clone(), cusp]).is_empty());

        let mut height_mismatch = through;
        height_mismatch.dz[1] = 5.2;
        assert!(
            endpoint_to_through_flush_ends(&[source, height_mismatch]).is_empty()
        );
    }

    fn a9_near_cap_join_fixture(
    ) -> (RoadTierJoinWay, RoadTierJoinWay, RoadTierJoinWay, RoadSurfaceKey) {
        let link_key = join_test_key(690, 0.7875, 1);
        let main_key = join_test_key(700, 1.125, 2);
        let continuation_key = join_test_key(726, 1.125, 3);
        let motorway_link = join_test_meta(RoadJoinFamily::Motorway, true, false);
        let motorway = join_test_meta(RoadJoinFamily::Motorway, false, false);
        let bridge_motorway = join_test_meta(RoadJoinFamily::Motorway, false, true);

        // z14/8417/5389 source geometry, scaled from the MVT's 4096 extent
        // into the renderer's 256-unit tile.
        let source = RoadTierJoinWay {
            key: link_key,
            way_index: 0,
            points: vec![
                (41.6875, 167.4375),
                (52.5625, 163.6875),
                (58.25, 161.0625),
                (62.1875, 159.8125),
            ],
            dz: vec![5.5, 1.1, 0.0, 0.0],
            half_width: 0.39375,
            meta: motorway_link,
        };
        let target = RoadTierJoinWay {
            key: main_key,
            way_index: 0,
            points: vec![(41.6875, 167.4375), (89.125, 140.875)],
            dz: vec![5.5, 5.9],
            half_width: 0.5625,
            meta: motorway,
        };
        let continuation = RoadTierJoinWay {
            key: continuation_key,
            way_index: 0,
            points: vec![(34.8125, 171.25), (41.6875, 167.4375)],
            dz: vec![5.5, 5.5],
            half_width: 0.5625,
            meta: bridge_motorway,
        };
        (source, target, continuation, link_key)
    }

    #[test]
    fn near_cap_link_to_proven_mainline_is_flush() {
        let (source, target, continuation, link_key) = a9_near_cap_join_fixture();
        let ways = [source.clone(), target.clone(), continuation.clone()];
        let flush = endpoint_to_through_flush_ends(&ways);

        assert_eq!(flush.len(), 1);
        assert!(flush.contains(&(link_key, 0, true)));

        // A small MVT-generalization offset still lies inside the two
        // ribbons and uses the same independently proven mainline cap.
        let mut near_source = source.clone();
        near_source.points[0] = (41.5875, 167.6375);
        let near_ways = [near_source, target.clone(), continuation.clone()];
        let near_flush = endpoint_to_through_flush_ends(&near_ways);
        assert_eq!(near_flush.len(), 1);
        assert!(near_flush.contains(&(link_key, 0, true)));

        // The decks already agree. The near-cap rule removes only the
        // internal top/fascia cap; it does not invent a grade correction.
        assert!(
            !endpoint_to_through_grade_corrections(&near_ways)
                .iter()
                .any(|correction| correction.end == (link_key, 0, true))
        );
    }

    #[test]
    fn near_cap_link_requires_semantics_height_overlap_and_through_proof() {
        let (source, target, continuation, _) = a9_near_cap_join_fixture();

        // A target cap without a third road proving continuation is a real
        // terminal and must retain its cap.
        assert!(endpoint_to_through_flush_ends(&[source.clone(), target.clone()]).is_empty());

        let mut perpendicular_continuation = continuation.clone();
        perpendicular_continuation.points =
            vec![(41.6875, 150.0), (41.6875, 167.4375)];
        assert!(endpoint_to_through_flush_ends(&[
            source.clone(),
            target.clone(),
            perpendicular_continuation,
        ])
        .is_empty());

        let mut other_family = continuation.clone();
        other_family.meta.family = RoadJoinFamily::Primary;
        assert!(endpoint_to_through_flush_ends(&[
            source.clone(),
            target.clone(),
            other_family,
        ])
        .is_empty());

        let mut height_mismatch = source.clone();
        height_mismatch.dz[0] = 4.9;
        assert!(endpoint_to_through_flush_ends(&[
            height_mismatch,
            target.clone(),
            continuation.clone(),
        ])
        .is_empty());

        let mut untyped_source = source.clone();
        untyped_source.meta.is_link = false;
        assert!(endpoint_to_through_flush_ends(&[
            untyped_source,
            target.clone(),
            continuation.clone(),
        ])
        .is_empty());

        let mut link_target = target.clone();
        link_target.meta.is_link = true;
        let mut link_continuation = continuation.clone();
        link_continuation.meta.is_link = true;
        assert!(endpoint_to_through_flush_ends(&[
            source.clone(),
            link_target,
            link_continuation,
        ])
        .is_empty());

        // The neighboring F87/P3 nose has the same semantics and height but
        // is about three tile units from this cap. Its styled ribbons do not
        // overlap, so the topology proof must not bridge that real gore gap.
        let mut distant_sibling = source;
        distant_sibling.points = vec![
            (43.125, 170.0625),
            (52.5625, 164.875),
            (58.625, 161.9375),
            (60.3125, 161.3125),
            (62.1875, 159.8125),
        ];
        distant_sibling.dz = vec![5.5, 1.4, 0.0, 0.0, 0.0];
        assert!(
            endpoint_to_through_flush_ends(&[distant_sibling, target, continuation]).is_empty()
        );
    }

    #[test]
    fn exact_bridge_continuation_inherits_deck_height() {
        let approach_key = join_test_key(20, 4.0, 1);
        let bridge_key =
            join_test_vertical_key(46, 4.0, 2, RoadVerticalClass::Elevated, 1);
        let approach_meta = join_test_meta(RoadJoinFamily::Motorway, false, false);
        let bridge_meta = join_test_meta(RoadJoinFamily::Motorway, false, true);
        let ways = vec![
            RoadTierJoinWay {
                key: approach_key,
                way_index: 0,
                points: vec![(0.0, 0.0), (-5.0, 0.0), (-12.0, 0.0)],
                dz: vec![0.0, 0.0, 0.0],
                half_width: 2.0,
                meta: approach_meta,
            },
            RoadTierJoinWay {
                key: bridge_key,
                way_index: 0,
                points: vec![(0.0, 0.0), (5.0, 0.0)],
                dz: vec![5.5, 5.5],
                half_width: 2.0,
                meta: bridge_meta,
            },
        ];

        let corrections = endpoint_continuation_grade_corrections(&ways);
        assert_eq!(corrections.len(), 1);
        assert_eq!(corrections[0].end, (approach_key, 0, true));
        assert!((corrections[0].target_dz - 5.5).abs() < 1e-5);

        let mut corrected = ways[0].dz.clone();
        apply_endpoint_grade_correction(
            &ways[0].points,
            &mut corrected,
            true,
            corrections[0].target_dz,
            ways[0].half_width,
        );
        assert!((corrected[0] - 5.5).abs() < 1e-5);
        assert!(corrected[1] > 0.0 && corrected[1] < 5.5);

        // The physical continuation wins regardless of which side carries
        // the bridge tag: a lower bridge segment must inherit the higher
        // ordinary approach just as the reverse case does.
        let mut reversed = ways.clone();
        reversed[0].dz.fill(5.5);
        reversed[1].dz.fill(4.3);
        let reversed_corrections = endpoint_continuation_grade_corrections(&reversed);
        assert_eq!(reversed_corrections.len(), 1);
        assert_eq!(reversed_corrections[0].end, (bridge_key, 0, true));
        assert!((reversed_corrections[0].target_dz - 5.5).abs() < 1e-5);

        // A strict, already-elevated continuation may change road class and
        // paint style at its exact node. Small bake disagreement is repaired
        // so the white/yellow pieces form one deck.
        let tertiary_key = join_test_key(10, 4.0, 3);
        let secondary_key = join_test_key(20, 4.1, 4);
        let class_transition = vec![
            RoadTierJoinWay {
                key: tertiary_key,
                way_index: 0,
                points: vec![(0.0, 0.0), (-5.0, 0.0)],
                dz: vec![16.6, 16.6],
                half_width: 2.0,
                meta: join_test_meta(RoadJoinFamily::Tertiary, false, false),
            },
            RoadTierJoinWay {
                key: secondary_key,
                way_index: 0,
                points: vec![(0.0, 0.0), (5.0, 0.0)],
                dz: vec![18.0, 18.0],
                half_width: 2.05,
                meta: join_test_meta(RoadJoinFamily::Secondary, false, false),
            },
        ];
        let class_corrections =
            endpoint_continuation_grade_corrections(&class_transition);
        assert_eq!(class_corrections.len(), 1);
        assert_eq!(class_corrections[0].end, (tertiary_key, 0, true));
        assert!((class_corrections[0].target_dz - 18.0).abs() < 1e-5);

        let mut excessive_class_step = class_transition.clone();
        excessive_class_step[0].dz.fill(14.0);
        assert!(
            endpoint_continuation_grade_corrections(&excessive_class_step).is_empty()
        );
        let mut grounded_class_step = class_transition.clone();
        grounded_class_step[0].dz.fill(0.0);
        assert!(endpoint_continuation_grade_corrections(&grounded_class_step).is_empty());

        let mut crossing = ways.clone();
        crossing[1].points = vec![(0.0, 0.0), (0.0, 5.0)];
        assert!(endpoint_continuation_grade_corrections(&crossing).is_empty());
    }

    /// Headless generator probe: build real tiles with the mirrored live
    /// theme, print per-stage timings (`MAKEPAD_TRACE=map.tile_profile`) and buffer sizes.
    /// Run: MAKEPAD_TRACE=map.tile_profile cargo test -p makepad-widgets --features maps \
    ///   --release union_perf_probe -- --ignored --nocapture
    #[test]
    #[ignore]
    fn union_perf_probe() {
        let maps = Path::new("../examples/map/local/maps");
        let mut base = MbtilesReader::open(&maps.join("europe-shortbread.mbtiles")).unwrap();
        let mut detail = MbtilesReader::open(&maps.join("europe-osm-detail.mbtiles")).unwrap();
        let mut dz = MbtilesReader::open(&maps.join("nl-bridge-dz.mbtiles")).unwrap();
        let theme = crate::map::style::probe_compiled_theme();
        let spots = [
            ("raampoort", 4.8785f64, 52.3798f64),
            ("europaboulevard", 4.8895f64, 52.3405f64),
            ("watergraafsmeer", 4.96521f64, 52.35456f64),
        ];
        for (name, lon, lat) in spots {
            let z = 14u32;
            let n = (1u64 << z) as f64;
            let nx = (lon + 180.0) / 360.0;
            let r = lat.to_radians();
            let ny = (1.0 - (r.tan() + 1.0 / r.cos()).ln() / std::f64::consts::PI) / 2.0;
            let (tx, ty) = ((nx * n) as i64, (ny * n) as i64);
            let tms = (1i64 << z) - 1 - ty;
            let raw = base.get_tile(z as i64, tx, tms).unwrap().unwrap();
            let det = detail.get_tile(z as i64, tx, tms).ok().flatten();
            let dzt = dz.get_tile(z as i64, tx, tms).ok().flatten();
            for render_zoom in [14u32, 17] {
                let key = TileKey { z, x: tx as i32, y: ty as i32 };
                let clock = Cx::monotonic_now();
                let buffers = build_tile_buffers_from_mvt(
                    key,
                    &raw,
                    det.as_deref(),
                    dzt.as_deref(),
                    dzt.is_some(),
                    &[],
                    &theme,
                    render_zoom,
                    true,
                    true,
                )
                .unwrap();
                let full_ms = (Cx::monotonic_now() - clock) * 1000.0;
                let overlay_clock = Cx::monotonic_now();
                let overlay = build_tile_buffers_from_mvt(
                    key,
                    &raw,
                    det.as_deref(),
                    dzt.as_deref(),
                    dzt.is_some(),
                    &[],
                    &theme,
                    render_zoom,
                    false,
                    false,
                )
                .unwrap();
                let overlay_ms = (Cx::monotonic_now() - overlay_clock) * 1000.0;
                assert!(overlay.mode_overlay_only);
                assert!(overlay.casing_vertices.is_empty());
                assert!(overlay.stroke_vertices.is_empty());
                assert!(overlay.road_icon_vertices.is_empty());
                println!(
                    "PROBE {name} z14/{tx}/{ty} rz{render_zoom}: full={full_ms:.0}ms mode-overlay={overlay_ms:.0}ms fill={}KB casing={}KB stroke={}KB icon={}KB",
                    (buffers.fill_vertices.len() + buffers.fill_indices.len()) * 4 / 1024,
                    (buffers.casing_vertices.len() + buffers.casing_indices.len()) * 4 / 1024,
                    (buffers.stroke_vertices.len() + buffers.stroke_indices.len()) * 4 / 1024,
                    (buffers.icon_vertices.len() + buffers.icon_indices.len()) * 4 / 1024,
                );
            }
        }
    }

    /// Forensic: dump baked base_dz ways near a lon/lat — endpoint coords
    /// and full dz profiles, to identify detached/outlier pieces.
    #[test]
    #[ignore]
    fn dump_base_dz_at() {
        use super::*;
        let (lon, lat) = (4.9445f64, 52.3382f64);
        let maps = Path::new("../examples/map/local/maps");
        let mut dz = MbtilesReader::open(&maps.join("nl-bridge-dz.mbtiles")).unwrap();
        let z = 14u32;
        let n = (1u64 << z) as f64;
        let nx = (lon + 180.0) / 360.0 * n;
        let r = lat.to_radians();
        let ny = (1.0 - (r.tan() + 1.0 / r.cos()).ln() / std::f64::consts::PI) / 2.0 * n;
        for (tx, ty) in [(nx.floor() as i64, ny.floor() as i64), (nx.floor() as i64 + 1, ny.floor() as i64)] {
            let lx = ((nx - tx as f64) * 256.0) as f32;
            let ly = ((ny - ty as f64) * 256.0) as f32;
            let tms = (1i64 << z) - 1 - ty;
            let Some(raw) = dz.get_tile(z as i64, tx, tms).ok().flatten() else {
                println!("tile {tx}/{ty}: none");
                continue;
            };
            let pbf = decode_vector_tile_payload(&raw).unwrap();
            let key = TileKey { z, x: tx as i32, y: ty as i32 };
            let mut collector = MvtLocalCollector::new(1.0);
            parse_mvt_tile(&pbf, key, &mut collector).unwrap();
            println!("=== tile {tx}/{ty} target local ({lx:.0},{ly:.0})");
            for way in &collector.ways {
                if way.tags.get("layer").map(|v| v.as_str()) != Some("base_dz") {
                    continue;
                }
                let near = way.points.iter().any(|&(px, py)| {
                    (px - lx).abs() < 18.0 && (py - ly).abs() < 18.0
                });
                if !near {
                    continue;
                }
                let decks: Vec<f32> = way
                    .tags
                    .get("dz")
                    .map(|s| s.split(',').filter_map(|v| v.parse::<f32>().ok()).map(|d| d * 0.1).collect())
                    .unwrap_or_default();
                let max = decks.iter().copied().fold(0.0f32, f32::max);
                if max < 0.3 {
                    continue;
                }
                println!(
                    "L={} F={} P={} closed={} pts={} start=({:.0},{:.0}) end=({:.0},{:.0}) dz={:?}",
                    way.tags.get("L").map(|v| v.as_str()).unwrap_or(""),
                    way.tags.get("F").map(|v| v.as_str()).unwrap_or(""),
                    way.tags.get("P").map(|v| v.as_str()).unwrap_or(""),
                    way.closed,
                    way.points.len(),
                    way.points.first().map(|p| p.0).unwrap_or(0.0),
                    way.points.first().map(|p| p.1).unwrap_or(0.0),
                    way.points.last().map(|p| p.0).unwrap_or(0.0),
                    way.points.last().map(|p| p.1).unwrap_or(0.0),
                    decks.iter().map(|d| (d * 10.0).round() / 10.0).collect::<Vec<_>>()
                );
            }
        }
    }

    /// Forensic: dump lifted casing-buffer vertices in the doubled-slab
    /// region of the Gooiseknoop seam sliver. p5 (ladder slot) identifies
    /// which face each copy belongs to.
    #[test]
    #[ignore]
    fn seam_probe() {
        use super::*;
        let maps = Path::new("../examples/map/local/maps");
        let mut base = MbtilesReader::open(&maps.join("europe-shortbread.mbtiles")).unwrap();
        let mut detail = MbtilesReader::open(&maps.join("europe-osm-detail.mbtiles")).unwrap();
        let mut dz = MbtilesReader::open(&maps.join("nl-bridge-dz.mbtiles")).unwrap();
        let theme = crate::map::style::probe_compiled_theme();
        let (z, tx, ty) = (14u32, 8414i64, 5386i64);
        let tms = (1i64 << z) - 1 - ty;
        let raw = base.get_tile(z as i64, tx, tms).unwrap().unwrap();
        let det = detail.get_tile(z as i64, tx, tms).ok().flatten();
        let dzt = dz.get_tile(z as i64, tx, tms).ok().flatten();
        let buffers = build_tile_buffers_from_mvt(
            TileKey { z, x: tx as i32, y: ty as i32 },
            &raw,
            det.as_deref(),
            dzt.as_deref(),
            dzt.is_some(),
            &[],
            &theme,
            18,
            true,
            true,
        )
        .unwrap();
        let mut rows: Vec<(i32, i32, f32, i32, u32)> = Vec::new();
        for chunk in buffers.casing_vertices.chunks_exact(19) {
            let (x, y, deck, p5) = (chunk[0], chunk[1], chunk[15], chunk[16]);
            let color = ((chunk[4] * 255.0) as u32) << 16
                | ((chunk[5] * 255.0) as u32) << 8
                | (chunk[6] * 255.0) as u32;
            if deck > 0.3 && (120.0..262.0).contains(&x) && (170.0..255.0).contains(&y) {
                rows.push((
                    x.round() as i32,
                    y.round() as i32,
                    deck,
                    (p5 * 1000.0).round() as i32,
                    color,
                ));
            }
        }
        rows.sort_by(|a, b| (a.1, a.0).cmp(&(b.1, b.0)));
        println!("rows: {}", rows.len());
        // Histogram: (color, p5 bucket) -> count + deck range
        let mut hist: HashMap<(u32, i32), (usize, f32, f32)> = HashMap::new();
        for &(_, _, deck, p5, color) in &rows {
            let entry = hist
                .entry((color, p5))
                .or_insert((0, f32::MAX, f32::MIN));
            entry.0 += 1;
            entry.1 = entry.1.min(deck);
            entry.2 = entry.2.max(deck);
        }
        let mut hist: Vec<_> = hist.into_iter().collect();
        hist.sort_by_key(|&((color, p5), _)| (color, p5));
        for ((color, p5), (count, lo, hi)) in hist {
            println!(
                "face color={color:06x} p5={:.3} verts={count} deck {lo:.2}..{hi:.2}",
                p5 as f32 / 1000.0
            );
        }
        // Coincident-location double coverage: same (x,y) cell, different p5
        let mut cells: HashMap<(i32, i32), Vec<(f32, i32, u32)>> = HashMap::new();
        for &(x, y, deck, p5, color) in &rows {
            cells.entry((x / 2, y / 2)).or_default().push((deck, p5, color));
        }
        let mut doubles = 0;
        for ((cx, cy), entries) in cells.iter() {
            let mut decks: Vec<f32> = entries.iter().map(|e| e.0).collect();
            decks.sort_by(|a, b| a.partial_cmp(b).unwrap());
            if decks.last().unwrap() - decks.first().unwrap() > 0.6 {
                doubles += 1;
                if doubles <= 12 {
                    println!("DOUBLE at ({},{}) -> {:?}", cx * 2, cy * 2, entries);
                }
            }
        }
        println!("double-height cells: {doubles}");

        // Cross-tile: build the EAST neighbor and compare road-surface
        // heights inside the shared clip-overlap band (global coords).
        let raw_b = base.get_tile(z as i64, tx + 1, tms).unwrap().unwrap();
        let det_b = detail.get_tile(z as i64, tx + 1, tms).ok().flatten();
        let dzt_b = dz.get_tile(z as i64, tx + 1, tms).ok().flatten();
        let buffers_b = build_tile_buffers_from_mvt(
            TileKey { z, x: (tx + 1) as i32, y: ty as i32 },
            &raw_b,
            det_b.as_deref(),
            dzt_b.as_deref(),
            dzt_b.is_some(),
            &[],
            &theme,
            18,
            true,
            true,
        )
        .unwrap();
        let mut band: Vec<(char, f32, f32, f32, u32)> = Vec::new();
        for (tag, buffers, x_off) in
            [('A', &buffers, 0.0f32), ('B', &buffers_b, 256.0)]
        {
            for chunk in buffers.casing_vertices.chunks_exact(19) {
                let (x, y, deck) = (chunk[0], chunk[1], chunk[15]);
                let gx = x + x_off;
                let color = ((chunk[4] * 255.0) as u32) << 16
                    | ((chunk[5] * 255.0) as u32) << 8
                    | (chunk[6] * 255.0) as u32;
                // Road surface colors only (motorway center + casing).
                if deck > 0.3
                    && (252.0..261.0).contains(&gx)
                    && (color == 0x00e892a2
                        || color == 0x00dc2a67
                        || color == 0x00f9b29c
                        || color == 0x00c84e2f)
                {
                    band.push((tag, gx, y, deck, color));
                }
            }
        }
        band.sort_by(|a, b| {
            (a.2 as i32, a.1 as i32, a.0).cmp(&(b.2 as i32, b.1 as i32, b.0))
        });
        println!("band rows: {}", band.len());
        for (tag, gx, y, deck, color) in band.iter().take(120) {
            println!("{tag} gx={gx:.1} y={y:.1} deck={deck:.2} c={color:06x}");
        }
    }

    #[test]
    #[ignore]
    fn probe_rai_detail_tags() {
        use super::*;
        let (lon, lat) = (4.8895f64, 52.3405f64);
        let z = 14u32;
        let n = (1u64 << z) as f64;
        let nx = (lon + 180.0) / 360.0;
        let r = lat.to_radians();
        let ny = (1.0 - (r.tan() + 1.0 / r.cos()).ln() / std::f64::consts::PI) / 2.0;
        let (tx, ty) = ((nx * n) as i64, (ny * n) as i64);
        let mut reader =
            MbtilesReader::open(Path::new("../local/maps/europe-osm-detail.mbtiles")).unwrap();
        let tms = (1i64 << z) - 1 - ty;
        let raw = reader.get_tile(z as i64, tx, tms).unwrap().unwrap();
        let data = decode_vector_tile_payload(&raw).unwrap();
        struct Dump;
        impl MvtSink for Dump {
            fn alloc_feature_id(&mut self) -> u64 {
                0
            }
            fn add_point(
                &mut self,
                _k: TileKey,
                _e: u32,
                _p: (i32, i32),
                _t: HashMap<String, String>,
            ) {
            }
            fn add_path(
                &mut self,
                _k: TileKey,
                _e: u32,
                _pts: &[(i32, i32)],
                tags: HashMap<String, String>,
                _close: bool,
            ) {
                let bridge = tags.get("bridge").map(|v| v.as_str()).unwrap_or("");
                if bridge == "yes" || bridge == "viaduct" {
                    let mut kv: Vec<String> = tags
                        .iter()
                        .filter(|(k, _)| !k.starts_with("__"))
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect();
                    kv.sort();
                    println!("DET {}", kv.join(" "));
                }
            }
        }
        let key = TileKey { z, x: tx as i32, y: ty as i32 };
        parse_mvt_tile(&data, key, &mut Dump).unwrap();
    }

    #[test]
    #[ignore] // needs local/maps/ocean-*.mbtiles (ocean-tiles output)
    fn probe_ocean_overlay() {
        use super::*;
        // 1. Exact-zoom coastal tile from the high archive parses into ways
        //    that carry the injected natural=water (the styling contract).
        let high = std::path::Path::new("../local/maps/ocean-high.mbtiles");
        assert!(high.is_file(), "no ocean archive at {}", high.display());
        let mut reader = MbtilesReader::open(high).unwrap();
        let (z, x, y) = (14_i64, 8385_i64, 5402_i64); // Scheveningen coast
        let raw = reader
            .get_tile_decoded(z, x, (1 << z) - 1 - y)
            .unwrap()
            .expect("coastal ocean tile missing");
        let key = TileKey { z: z as u32, x: x as i32, y: y as i32 };
        let mut collector = MvtLocalCollector::new(2.0);
        parse_mvt_tile(&raw, key, &mut collector).unwrap();
        let ocean_ways: Vec<_> = collector
            .ways
            .iter()
            .filter(|way| way.closed && way.tags.get("natural").map(String::as_str) == Some("water"))
            .collect();
        println!("coastal z14: {} ocean ways", ocean_ways.len());
        assert!(!ocean_ways.is_empty());

        // 2. Ancestor-shift path: a z14 view over open sea fetches the z9
        //    low-archive tile; merge_overlay_features must scale it into
        //    local space and still yield water polygons covering the tile.
        let low = std::path::Path::new("../local/maps/ocean-low.mbtiles");
        assert!(low.is_file(), "no ocean archive at {}", low.display());
        let mut low_reader = MbtilesReader::open(low).unwrap();
        // Mid-North-Sea z14 tile (no high-archive coverage), z9 ancestor.
        let (vz, vx, vy) = (14_u32, 8340_u32, 5390_u32);
        let shift = 5_u32; // 14 - maxzoom 9
        let (fx, fy) = (vx >> shift, vy >> shift);
        let araw = low_reader
            .get_tile_decoded(9, fx as i64, (1 << 9) - 1 - fy as i64)
            .unwrap()
            .expect("z9 ancestor ocean tile missing");
        let overlay = OverlayTileData {
            raw: araw,
            shift,
            quadrant_x: vx - (fx << shift),
            quadrant_y: vy - (fy << shift),
            filter: 0,
            has_chargers: false,
        };
        let mut points = Vec::new();
        let mut ways = Vec::new();
        merge_overlay_features(
            &overlay,
            TileKey { z: vz, x: vx as i32, y: vy as i32 },
            2.0,
            &mut points,
            &mut ways,
        )
        .unwrap();
        let water: Vec<_> = ways
            .iter()
            .filter(|way| way.tags.get("natural").map(String::as_str) == Some("water"))
            .collect();
        println!("shifted z9->z14: {} water ways, {} pts first", water.len(),
            water.first().map(|w| w.points.len()).unwrap_or(0));
        assert!(!water.is_empty());

        // 3. A pure-ocean tile the base pyramid omits entirely (central
        //    North Sea) must still build via the empty-base path when the
        //    ocean overlays cover it — the "dead beige rectangles" bug.
        let world = std::path::Path::new("../local/maps/world.mkmap");
        if world.exists() {
            let theme = CompiledMapTheme::default();
            let keys = vec![TileKey { z: 8, x: 130, y: 82 }];
            let overlays = vec![
                "../local/maps/ocean-low.mbtiles".to_string(),
                "../local/maps/ocean-high.mbtiles".to_string(),
            ];
            let (loaded, failed) = load_local_tile_batch(
                world,
                Some(world),
                None,
                &overlays,
                &keys,
                &theme,
                8,
                false,
                false,
            )
            .unwrap();
            println!("ocean-only tile: loaded {} failed {}", loaded.len(), failed.len());
            assert_eq!(loaded.len(), 1, "pure-ocean tile must build from empty base");
        } else {
            println!("world.mkmap absent — skipping empty-base build check");
        }
    }

    #[test]
    #[ignore] // needs local bake output
    fn probe_bridge_dz_load() {
        use super::*;
        let path = std::path::Path::new("../local/maps/nl-bridge-dz.mbtiles");
        assert!(path.is_file(), "no bake output at {}", path.display());
        let mut reader = MbtilesReader::open(path).unwrap();
        let meta = reader.get_metadata().unwrap_or_default();
        println!("meta minzoom={:?} bounds={:?}", meta.get("minzoom"), meta.get("bounds"));
        let (x, y) = (8414i64, 5387i64);
        let tms = (1i64 << 14) - 1 - y;
        let raw = reader.get_tile(14, x, tms).unwrap().expect("dz tile missing");
        println!("raw {} bytes", raw.len());
        let key = TileKey { z: 14, x: x as i32, y: y as i32 };
        let corridors = parse_bridge_dz_corridors(&raw, key).unwrap();
        println!("corridors: {}", corridors.len());
        for corridor in corridors.iter().take(5) {
            let max = corridor.decks.iter().fold(0.0f32, |a, &b| a.max(b));
            println!(
                "  pts {} decks max {:.1} hw {:.2} first ({:.1},{:.1})",
                corridor.points.len(),
                max,
                corridor.half_width,
                corridor.points[0].0,
                corridor.points[0].1
            );
        }
        assert!(!corridors.is_empty());

        // Full pipeline: fetch + parse + corridor match into GPU buffers.
        let theme = CompiledMapTheme::default();
        let keys = vec![TileKey { z: 14, x: 8414, y: 5387 }];
        let (loaded, failed) = load_local_tile_batch(
            std::path::Path::new("../local/maps/europe-shortbread.mbtiles"),
            Some(std::path::Path::new("../local/maps/europe-osm-detail.mbtiles")),
            Some(path),
            &[],
            &keys,
            &theme,
            17,
            true,
            true,
        )
        .unwrap();
        println!("loaded {} failed {}", loaded.len(), failed.len());
        let buffers = &loaded[0].buffers;
        let floats_per_vertex = 19;
        let mut decked = 0usize;
        let mut max_deck = 0.0f32;
        for chunk in buffers.stroke_vertices.chunks_exact(floats_per_vertex) {
            let deck = chunk[15];
            if deck > 0.3 {
                decked += 1;
                max_deck = max_deck.max(deck);
            }
        }
        println!(
            "stroke verts {} decked {} max {:.1}",
            buffers.stroke_vertices.len() / floats_per_vertex,
            decked,
            max_deck
        );
        assert!(decked > 0, "no decked stroke vertices — dz not reaching geometry");

        // Join diagnostics: how many base paths find their dz, and how many
        // fail the length check.
        let raw2 = reader.get_tile(14, x, tms).unwrap().unwrap();
        let map = parse_base_dz_map(&raw2, key).unwrap();
        println!("base_dz map entries: {}", map.len());
        struct JoinProbe {
            map: HashMap<(String, u32, u32), BaseDzProfile>,
            hit: usize,
            invalid_profile: usize,
            miss: usize,
            oneway: usize,
            oneway_values: Vec<String>,
        }
        impl MvtSink for JoinProbe {
            fn alloc_feature_id(&mut self) -> u64 {
                1
            }
            fn add_path(
                &mut self,
                _tile_key: TileKey,
                _extent: u32,
                points: &[(i32, i32)],
                tags: HashMap<String, String>,
                _close: bool,
            ) {
                let (Some(layer), Some(fidx), Some(pidx)) = (
                    tags.get("layer"),
                    tags.get(MVT_INTERNAL_FIDX_KEY),
                    tags.get(MVT_INTERNAL_PIDX_KEY),
                ) else {
                    return;
                };
                let key = (
                    layer.clone(),
                    fidx.parse::<u32>().unwrap_or(9999),
                    pidx.parse::<u32>().unwrap_or(9999),
                );
                if tags.get("layer").map(|v| v.as_str()) == Some("streets") {
                    if let Some(value) = tags.get("oneway") {
                        self.oneway += 1;
                        if !self.oneway_values.contains(value) {
                            self.oneway_values.push(value.clone());
                        }
                    }
                }
                match self.map.get(&key) {
                    Some(profile)
                        if base_dz_profile_projected_points(
                            profile,
                            points,
                            TILE_SIZE as f32 / 4096.0,
                            false,
                        )
                        .is_some() =>
                    {
                        self.hit += 1;
                    }
                    Some(profile) => {
                        self.invalid_profile += 1;
                        if self.invalid_profile <= 5 {
                            println!(
                                "  invalid profile {:?}: profile {} vs raw {}",
                                key,
                                profile.points.len(),
                                points.len()
                            );
                        }
                    }
                    None => self.miss += 1,
                }
            }
            fn add_point(
                &mut self,
                _tile_key: TileKey,
                _extent: u32,
                _point: (i32, i32),
                _tags: HashMap<String, String>,
            ) {
            }
        }
        let mut probe = JoinProbe {
            map,
            hit: 0,
            invalid_profile: 0,
            miss: 0,
            oneway: 0,
            oneway_values: Vec::new(),
        };
        let base_raw = MbtilesReader::open(std::path::Path::new(
            "../local/maps/europe-shortbread.mbtiles",
        ))
        .unwrap()
        .get_tile(14, x, tms)
        .unwrap()
        .unwrap();
        let base_pbf = decode_vector_tile_payload(&base_raw).unwrap();
        parse_mvt_tile(&base_pbf, key, &mut probe).unwrap();
        println!(
            "join: hit {} invalid_profile {} miss {} oneway {} values {:?}",
            probe.hit, probe.invalid_profile, probe.miss, probe.oneway, probe.oneway_values
        );

        // Oneway arrows: count map-aligned icon glyphs and their lifts.
        let mut arrows = 0;
        let mut lifted_arrows = 0;
        for chunk in buffers.icon_vertices.chunks_exact(floats_per_vertex) {
            let shape = chunk[10];
            let param3 = chunk[14];
            let param4 = chunk[15];
            if (shape - 20.0).abs() < 0.1 && (param3 - 2.0).abs() < 0.1 {
                arrows += 1;
                if param4.abs() > 0.05 {
                    lifted_arrows += 1;
                }
            }
        }
        println!("arrow verts {} lifted {}", arrows, lifted_arrows);
    }

    #[test]
    #[ignore]
    fn probe_rai_bridge_tags() {
        use super::*;
        let (lon, lat) = (4.8895f64, 52.3405f64);
        let z = 14u32;
        let n = (1u64 << z) as f64;
        let nx = (lon + 180.0) / 360.0;
        let r = lat.to_radians();
        let ny = (1.0 - (r.tan() + 1.0 / r.cos()).ln() / std::f64::consts::PI) / 2.0;
        let (tx, ty) = ((nx * n) as i64, (ny * n) as i64);
        let mut reader = MbtilesReader::open(Path::new("../local/maps/europe-shortbread.mbtiles"))
            .or_else(|_| {
                MbtilesReader::open(Path::new("local/maps/europe-shortbread.mbtiles"))
            })
            .unwrap();
        let tms = (1i64 << z) - 1 - ty;
        let raw = reader.get_tile(z as i64, tx, tms).unwrap().unwrap();
        let data = decode_vector_tile_payload(&raw).unwrap();
        struct Dump;
        impl MvtSink for Dump {
            fn alloc_feature_id(&mut self) -> u64 {
                0
            }
            fn add_point(
                &mut self,
                _k: TileKey,
                _e: u32,
                _p: (i32, i32),
                _t: HashMap<String, String>,
            ) {
            }
            fn add_path(
                &mut self,
                _k: TileKey,
                _e: u32,
                _pts: &[(i32, i32)],
                tags: HashMap<String, String>,
                _close: bool,
            ) {
                let layer = tags.get("layer").cloned().unwrap_or_default();
                let name = tags.get("name").cloned().unwrap_or_default();
                let interesting = name.contains("brug")
                    || name.contains("Europaboulevard")
                    || tags.contains_key("bridge");
                if interesting && matches!(layer.as_str(), "streets" | "street_polygons" | "bridges") {
                    let mut kv: Vec<String> = tags
                        .iter()
                        .filter(|(k, _)| !k.starts_with("__"))
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect();
                    kv.sort();
                    println!("[{layer}] {}", kv.join(" "));
                }
            }
        }
        let key = TileKey { z, x: tx as i32, y: ty as i32 };
        parse_mvt_tile(&data, key, &mut Dump).unwrap();
    }

    #[test]
    #[ignore]
    fn westerkerk_probe() {
        let detail = std::path::Path::new("../local/maps/europe-osm-detail.mbtiles");
        if !detail.exists() {
            return;
        }
        let mut reader = makepad_mbtile_reader::MbtilesReader::open(detail).unwrap();
        let (z, x, y) = (14i64, 8414i64, 5384i64);
        let raw = reader.get_tile(z, x, (1 << z) - 1 - y).unwrap().unwrap();
        let key = TileKey { z: z as u32, x: x as i32, y: y as i32 };
        let pbf = decode_vector_tile_payload(&raw).unwrap();
        let mut collector = MvtLocalCollector::new(4.0);
        parse_mvt_tile(&pbf, key, &mut collector).unwrap();
        let mut by_layer = std::collections::HashMap::<String, usize>::new();
        for way in &collector.ways {
            let layer = way.tags.get("layer").cloned().unwrap_or_default();
            *by_layer.entry(layer).or_default() += 1;
            if way.tags.contains_key("building:part") {
                println!(
                    "PART layer={} closed={} pts={} id={:?} h={:?} min={:?}",
                    way.tags.get("layer").cloned().unwrap_or_default(),
                    way.closed,
                    way.points.len(),
                    way.tags.get("__makepad_osm_id"),
                    way.tags.get("height"),
                    way.tags.get("min_height"),
                );
            }
        }
        let mut stats: Vec<_> = by_layer.into_iter().collect();
        stats.sort();
        println!("LAYER STATS {:?}", stats);
    }

    #[test]
    #[ignore]
    fn place_labels_probe() {
        let base = std::path::Path::new("../local/maps/europe-shortbread.mbtiles");
        if !base.exists() {
            return;
        }
        let mut reader = makepad_mbtile_reader::MbtilesReader::open(base).unwrap();
        // Amsterdam's own tile: dump raw place kinds.
        {
            let (z, x, y) = (10i64, 525i64, 336i64);
            if let Some(raw) = reader.get_tile(z, x, (1 << z) - 1 - y).unwrap() {
                let key = TileKey { z: z as u32, x: x as i32, y: y as i32 };
                let pbf = decode_vector_tile_payload(&raw).unwrap();
                let mut collector = MvtLocalCollector::new(1.0);
                parse_mvt_tile(&pbf, key, &mut collector).unwrap();
                for (_, tags) in &collector.points {
                    if tags.get("layer").map(|v| v.as_str()) == Some("place_labels") {
                        let name = tags.get("name").cloned().unwrap_or_default();
                        if name.contains("Amsterdam") || name.contains("Haarlem") {
                            let mut t: Vec<_> = tags.iter().collect();
                            t.sort();
                            println!("PLACE {:?}", t);
                        }
                    }
                }
            }
        }
        for (z, x, y) in [(11i64, 1052i64, 674i64), (10, 526, 337), (8, 131, 84)] {
            let raw = reader.get_tile(z, x, (1 << z) - 1 - y).unwrap().unwrap();
            let key = TileKey { z: z as u32, x: x as i32, y: y as i32 };
            let theme = CompiledMapTheme::default();
            let buffers = build_tile_buffers_from_mvt(
                key,
                &raw,
                None,
                None,
                false,
                &[],
                &theme,
                z as u32,
                false,
                true,
            )
            .unwrap();
            let places: Vec<&TileLabel> = buffers
                .labels
                .iter()
                .filter(|l| l.source_layer == "place_labels")
                .collect();
            let streets = buffers
                .labels
                .iter()
                .filter(|l| l.source_layer.starts_with("street"))
                .count();
            println!(
                "z{} labels total {} places {} streets {}",
                z,
                buffers.labels.len(),
                places.len(),
                streets
            );
            for label in places.iter().take(5) {
                println!("  {:?} kind={} prio={}", label.text, label.road_kind, label.priority);
            }
        }
    }

    #[test]
    #[ignore]
    fn overlay_batch_probe() {
        let base = std::path::Path::new("../local/maps/europe-shortbread.mbtiles");
        let overlay = "../local/overlays/nl-chargers.mbtiles".to_string();
        let transit = "../local/overlays/nl-transit.mbtiles".to_string();
        if !base.exists() {
            return;
        }
        let theme = CompiledMapTheme::default();
        let keys = vec![TileKey { z: 12, x: 2103, y: 1346 }];
        let loaded = load_local_tile_batch(
            base,
            None,
            None,
            &[overlay, transit],
            &keys,
            &theme,
            12,
            false,
            true,
        )
        .unwrap();
        for tile in &loaded.0 {
            println!(
                "tile z{} icons {} strokes {} labels {}",
                tile.tile_key.z,
                tile.buffers.icon_vertices.len() / VECTOR_FLOATS_PER_VERTEX,
                tile.buffers.stroke_vertices.len() / VECTOR_FLOATS_PER_VERTEX,
                tile.buffers.labels.len()
            );
        }
    }

    #[test]
    #[ignore]
    fn overlay_chargers_probe() {
        let base = std::path::Path::new("../local/maps/europe-shortbread.mbtiles");
        let overlay = std::path::Path::new("../local/overlays/nl-chargers.mbtiles");
        if !base.exists() || !overlay.exists() {
            return;
        }
        let mut base_reader = makepad_mbtile_reader::MbtilesReader::open(base).unwrap();
        let mut overlay_reader = makepad_mbtile_reader::MbtilesReader::open(overlay).unwrap();
        let (z, x, y) = (12i64, 2103i64, 1346i64);
        let raw = base_reader.get_tile(z, x, (1 << z) - 1 - y).unwrap().unwrap();
        let ov = overlay_reader
            .get_tile(z, x, (1 << z) - 1 - y)
            .unwrap()
            .unwrap();
        let key = TileKey { z: z as u32, x: x as i32, y: y as i32 };
        let overlay_tiles = vec![OverlayTileData {
            raw: ov,
            shift: 0,
            quadrant_x: 0,
            quadrant_y: 0,
            filter: 0,
            has_chargers: true,
        }];
        let theme = CompiledMapTheme::default();
        let buffers = build_tile_buffers_from_mvt(
            key,
            &raw,
            None,
            None,
            false,
            &overlay_tiles,
            &theme,
            12,
            false,
            true,
        )
        .unwrap();
        println!(
            "icon verts {} labels {} features {}",
            buffers.icon_vertices.len() / VECTOR_FLOATS_PER_VERTEX,
            buffers.labels.len(),
            buffers.feature_count
        );
    }

    /// Headless tile-build profiler: the app's exact hot path (decode +
    /// parse + style + tessellation) over real archive tiles, no window.
    /// TILE_PROFILE_KEYS="z,x,y;z,x,y;..." overrides the default slow set;
    /// The theme-independence contract: baking a tile under the light
    /// theme and under a full recolor (the dark/circuit stand-in) must
    /// produce identical signatures and regions — proving one bake serves
    /// every recolor-only theme. Uses TILE_PROFILE_ARCHIVE/KEYS.
    #[test]
    #[ignore]
    fn baked_faces_theme_independent() {
        let archive = std::env::var("TILE_PROFILE_ARCHIVE")
            .unwrap_or_else(|_| "../local/maps/nl-base-br3.mbtiles".to_string());
        let path = std::path::Path::new(&archive);
        if !path.exists() {
            println!("no archive at {archive}");
            return;
        }
        let keys_spec = std::env::var("TILE_PROFILE_KEYS")
            .unwrap_or_else(|_| "14,8414,5384;13,4207,2690;12,2103,1346".into());
        let mut reader = makepad_mbtile_reader::TileArchiveReader::open(path).unwrap();
        let light = crate::map::style::probe_compiled_theme();
        let recolored = crate::map::style::probe_compiled_theme_recolored();
        for spec in keys_spec.split(';') {
            let mut it = spec.split(',').map(|v| v.trim().parse::<i64>().unwrap());
            let (z, x, y) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());
            let tile_count = 1_i64 << z;
            let Some(blob) = reader.get_tile(z, x, tile_count - 1 - y).ok().flatten() else {
                panic!("tile z{z} {x}/{y} missing from {archive}");
            };
            let raw = reader.decode_tile(&blob).unwrap();
            let pbf = decode_vector_tile_payload(&raw).unwrap();
            let key = TileKey { z: z as u32, x: x as i32, y: y as i32 };
            let bucket = if z >= 14 { 16 } else { z as u32 };
            let a = bake_tile_paint_faces(key, &pbf, Some(&pbf), None, false, &light, bucket)
                .unwrap_or_else(|| panic!("light bake produced nothing for z{z} {x}/{y}"));
            let b = bake_tile_paint_faces(key, &pbf, Some(&pbf), None, false, &recolored, bucket)
                .unwrap_or_else(|| panic!("recolored bake produced nothing for z{z} {x}/{y}"));
            assert_eq!(
                a.signature, b.signature,
                "signature diverged across recolor on z{z} {x}/{y}"
            );
            assert_eq!(
                a.regions.len(),
                b.regions.len(),
                "region count diverged on z{z} {x}/{y}"
            );
            for (ra, rb) in a.regions.iter().zip(&b.regions) {
                assert_eq!(ra.group_index, rb.group_index, "group order diverged");
                assert_eq!(
                    ra.main.len(),
                    rb.main.len(),
                    "main region shapes diverged on z{z} {x}/{y}"
                );
                assert_eq!(ra.sunk.len(), rb.sunk.len(), "sunk shapes diverged");
            }
            println!(
                "z{z} {x}/{y} bucket {bucket}: sig {:016x} identical across recolor, {} regions",
                a.signature,
                a.regions.len()
            );
        }
    }

    /// TILE_PROFILE_REPS=N (default 3). Run:
    ///   cargo test -p makepad-widgets --features maps --release \
    ///     profile_tile_build -- --ignored --nocapture
    #[test]
    #[ignore]
    fn profile_tile_build() {
        let archive = std::env::var("TILE_PROFILE_ARCHIVE")
            .unwrap_or_else(|_| "../local/maps/nl-base-br.mbtiles".to_string());
        let path = std::path::Path::new(&archive);
        if !path.exists() {
            println!("no archive at {archive}");
            return;
        }
        // Defaults: the worst offenders from the in-app slow-tile log plus
        // one mid and one deep zoom for contrast.
        let keys_spec = std::env::var("TILE_PROFILE_KEYS").unwrap_or_else(|_| {
            "9,265,170;9,265,171;9,264,171;9,266,170;11,1057,678;13,4207,2692;14,8414,5387".into()
        });
        let reps: usize = std::env::var("TILE_PROFILE_REPS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3);
        let mut reader = makepad_mbtile_reader::TileArchiveReader::open(path).unwrap();
        // TILE_PROFILE_DETAIL_ARCHIVE: two-reader mode replicating the old
        // base+detail archive pair (europe-shortbread + europe-osm-detail);
        // without it, deep-zoom detail comes from the combined archive's own
        // tile bytes (the nl-base-br pattern).
        let mut detail_reader = std::env::var("TILE_PROFILE_DETAIL_ARCHIVE")
            .ok()
            .map(|p| makepad_mbtile_reader::TileArchiveReader::open(std::path::Path::new(&p)).unwrap());
        // The real compiled day theme: a default CompiledMapTheme styles
        // nothing and skips the entire tessellation path being profiled.
        let theme = crate::map::style::probe_compiled_theme();
        println!(
            "{:<16} {:>9} {:>9} {:>9} {:>9} {:>8} {:>9}",
            "tile", "bytes", "decode", "build", "best", "feats", "verts"
        );
        let mut total_best = 0.0f64;
        for spec in keys_spec.split(';') {
            let mut it = spec.split(',').map(|v| v.trim().parse::<i64>().unwrap());
            let (z, x, y) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());
            let tile_count = 1_i64 << z;
            let Some(blob) = reader.get_tile(z, x, tile_count - 1 - y).ok().flatten() else {
                println!("{:<16} missing", format!("z{z} {x}/{y}"));
                continue;
            };
            let t0 = Cx::monotonic_now();
            let raw = reader.decode_tile(&blob).unwrap();
            let decode_ms = (Cx::monotonic_now() - t0) * 1e3;
            let key = TileKey { z: z as u32, x: x as i32, y: y as i32 };
            let mut best = f64::MAX;
            let mut last = None;
            // Simulate overzoom/3D: TILE_PROFILE_RENDER_ZOOM overrides the
            // render zoom (e.g. 16 for z14 tiles at building-3D zooms) and
            // TILE_PROFILE_3D=0/1 forces the buildings mode.
            let render_zoom: u32 = std::env::var("TILE_PROFILE_RENDER_ZOOM")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(z as u32);
            let force_3d = std::env::var("TILE_PROFILE_3D")
                .ok()
                .map(|v| v == "1")
                .unwrap_or(z >= 14);
            // TILE_PROFILE_BRIDGE_DZ / TILE_PROFILE_OVERLAYS: reproduce the
            // in-app worker path exactly (the SLOW-log replay contract) by
            // delegating to load_local_tile_batch, which owns dz-coverage
            // bounds and overlay quadrant/filter handling.
            let bridge_dz_env = std::env::var("TILE_PROFILE_BRIDGE_DZ").ok();
            let overlays_env: Vec<String> = std::env::var("TILE_PROFILE_OVERLAYS")
                .map(|v| {
                    v.split(';')
                        .filter(|s| !s.trim().is_empty())
                        .map(|s| s.trim().to_string())
                        .collect()
                })
                .unwrap_or_default();
            if bridge_dz_env.is_some() || !overlays_env.is_empty() {
                let mut best = f64::MAX;
                let mut last = None;
                for _ in 0..reps {
                    let t1 = Cx::monotonic_now();
                    // Combined archives are their own detail source (the
                    // app passes the same path for both).
                    let detail_env = std::env::var("TILE_PROFILE_DETAIL_ARCHIVE").ok();
                    let (loaded, _failed) = load_local_tile_batch(
                        path,
                        Some(
                            detail_env
                                .as_deref()
                                .map(std::path::Path::new)
                                .unwrap_or(path),
                        ),
                        bridge_dz_env.as_deref().map(std::path::Path::new),
                        &overlays_env,
                        &[TileKey { z: z as u32, x: x as i32, y: y as i32 }],
                        &theme,
                        render_zoom,
                        force_3d,
                        true,
                    )
                    .unwrap();
                    best = best.min((Cx::monotonic_now() - t1) * 1e3);
                    last = loaded.into_iter().next().map(|t| t.buffers);
                }
                let Some(buffers) = last else {
                    println!("{:<16} missing (batch)", format!("z{z} {x}/{y}"));
                    continue;
                };
                total_best += best;
                println!(
                    "{:<16} {:>9} {:>8.1}m {:>8.1}m {:>8.1}m {:>8} {:>9}  (full batch path)",
                    format!("z{z} {x}/{y}"),
                    blob.len(),
                    decode_ms,
                    best,
                    best,
                    buffers.feature_count,
                    buffers.fill_vertices.len() / VECTOR_FLOATS_PER_VERTEX,
                );
                continue;
            }
            let detail_raw: Option<Vec<u8>> = detail_reader.as_mut().and_then(|dr| {
                let blob = dr.get_tile(z, x, tile_count - 1 - y).ok().flatten()?;
                dr.decode_tile(&blob).ok()
            });
            for _ in 0..reps {
                let t1 = Cx::monotonic_now();
                let detail = if detail_reader.is_some() {
                    // Old two-archive pattern: detail strictly from its own
                    // archive (may be absent for a tile).
                    detail_raw.as_deref().filter(|_| z >= 14)
                } else {
                    (z >= 14).then_some(raw.as_slice())
                };
                let buffers = build_tile_buffers_from_mvt(
                    key,
                    &raw,
                    detail,
                    None,
                    false,
                    &[],
                    &theme,
                    render_zoom,
                    force_3d,
                    true,
                )
                .unwrap();
                best = best.min((Cx::monotonic_now() - t1) * 1e3);
                last = Some(buffers);
            }
            let buffers = last.unwrap();
            total_best += best;
            println!(
                "{:<16} {:>9} {:>8.1}m {:>8.1}m {:>8.1}m {:>8} {:>9}",
                format!("z{z} {x}/{y}"),
                blob.len(),
                decode_ms,
                best,
                best,
                buffers.feature_count,
                buffers.fill_vertices.len() / VECTOR_FLOATS_PER_VERTEX,
            );
            println!(
                "                  icons {} labels {}",
                buffers.icon_vertices.len() / VECTOR_FLOATS_PER_VERTEX,
                buffers.labels.len()
            );
        }
        println!("total best-of-{reps}: {total_best:.1}ms");
    }

    #[test]
    #[ignore]
    fn weesperplein_tear_probe() {
        let base = std::path::Path::new("../local/maps/europe-shortbread.mbtiles");
        let detail = std::path::Path::new("../local/maps/europe-osm-detail.mbtiles");
        let dz_name = std::env::var("TEAR_DZ")
            .unwrap_or_else(|_| "../local/maps/nl-bridge-dz.mbtiles".to_string());
        let dz = std::path::Path::new(&dz_name);
        if !base.exists() || !detail.exists() || !dz.exists() {
            println!("no archives");
            return;
        }
        let mut base_reader = makepad_mbtile_reader::MbtilesReader::open(base).unwrap();
        let mut detail_reader = makepad_mbtile_reader::MbtilesReader::open(detail).unwrap();
        let mut dz_reader = makepad_mbtile_reader::MbtilesReader::open(dz).unwrap();
        let tile_spec = std::env::var("TEAR_TILE").unwrap_or_else(|_| "8415,5384".to_string());
        let mut ts = tile_spec.split(',').map(|v| v.parse::<i64>().unwrap());
        let (x, y) = (ts.next().unwrap(), ts.next().unwrap());
        let key = TileKey { z: 14, x: x as i32, y: y as i32 };
        let raw = base_reader.get_tile(14, x, 16383 - y).unwrap().unwrap();
        let det = detail_reader.get_tile(14, x, 16383 - y).unwrap();
        let dzt = dz_reader.get_tile(14, x, 16383 - y).unwrap();
        let mut theme = probe_compiled_theme();
        theme.shiny.bake_shadows = true;
        theme.shiny.bake_ao = true;
        let buffers = build_tile_buffers_from_mvt(
            key,
            &raw,
            det.as_deref(),
            dzt.as_deref(),
            dzt.is_some(),
            &[],
            &theme,
            std::env::var("TEAR_ZOOM")
                .ok()
                .and_then(|z| z.parse().ok())
                .unwrap_or(17),
            true,
            true,
        )
        .unwrap();
        // Group decked vertices (param4 > 0.3) per buffer by quantized
        // color + param5, print bbox + deck range — who lifts where.
        for (name, verts) in [
            ("casing", &buffers.casing_vertices),
            ("stroke", &buffers.stroke_vertices),
        ] {
            use std::collections::HashMap;
            let mut groups: HashMap<(u32, u32, u32), (f32, f32, f32, f32, f32, f32, usize)> =
                HashMap::new();
            for v in verts.chunks_exact(VECTOR_FLOATS_PER_VERTEX) {
                let deck = v[15];
                // Weesperplein plaza window; report every vertex incl.
                // grounded so the full layer stack is visible.
                let win = std::env::var("TEAR_WIN").unwrap_or_else(|_| "88,118,86,120".to_string());
                let mut wv = win.split(',').map(|v| v.parse::<f32>().unwrap());
                let (wx0, wx1, wy0, wy1) = (wv.next().unwrap(), wv.next().unwrap(), wv.next().unwrap(), wv.next().unwrap());
                if v[0] < wx0 || v[0] > wx1 || v[1] < wy0 || v[1] > wy1 {
                    continue;
                }
                let _ = deck;
                let color_key = ((v[4] * 15.0) as u32) << 8
                    | ((v[5] * 15.0) as u32) << 4
                    | (v[6] * 15.0) as u32;
                let p5_key = (v[16] * 1000.0) as u32;
                let shape_key = v[10] as u32;
                let entry = groups
                    .entry((color_key, p5_key, shape_key))
                    .or_insert((f32::MAX, f32::MAX, f32::MIN, f32::MIN, f32::MAX, f32::MIN, 0));
                entry.0 = entry.0.min(v[0]);
                entry.1 = entry.1.min(v[1]);
                entry.2 = entry.2.max(v[0]);
                entry.3 = entry.3.max(v[1]);
                entry.4 = entry.4.min(deck);
                entry.5 = entry.5.max(deck);
                entry.6 += 1;
            }
            let mut rows: Vec<_> = groups.into_iter().collect();
            rows.sort_by_key(|(_, v)| std::cmp::Reverse(v.6));
            for ((color, p5, shape), (x0, y0, x1, y1, d0, d1, count)) in rows.iter().take(14) {
                println!(
                    "{name}: color {color:03x} p5 {:.3} shape {shape} verts {count} bbox ({x0:.0},{y0:.0})-({x1:.0},{y1:.0}) deck {d0:.2}..{d1:.2}",
                    *p5 as f32 / 1000.0
                );
            }
            println!("-- {name} total verts {}", verts.len() / VECTOR_FLOATS_PER_VERTEX);
        }
        // SVG dump of the window's triangles in draw order (casing pass):
        // the tear must show as literal holes/overdraw in here.
        {
            use std::fmt::Write as _;
            let (verts, indices) = (&buffers.casing_vertices, &buffers.casing_indices);
            let vb_env = std::env::var("TEAR_VB").unwrap_or_else(|_| "86 84 36 40".to_string());
            let mut svg = format!(
                "<svg xmlns='http://www.w3.org/2000/svg' viewBox='{vb_env}' width='1440' height='1600'>\n",
            );
            let mut tris = 0usize;
            for tri in indices.chunks_exact(3) {
                let v0 = &verts[tri[0] as usize * VECTOR_FLOATS_PER_VERTEX..];
                let v1 = &verts[tri[1] as usize * VECTOR_FLOATS_PER_VERTEX..];
                let v2 = &verts[tri[2] as usize * VECTOR_FLOATS_PER_VERTEX..];
                let vb = std::env::var("TEAR_VB").unwrap_or_else(|_| "86 84 36 40".to_string());
                let mut vbv = vb.split(' ').map(|v| v.parse::<f32>().unwrap());
                let (bx, by, bw, bh) = (vbv.next().unwrap(), vbv.next().unwrap(), vbv.next().unwrap(), vbv.next().unwrap());
                let inside = |v: &[f32]| {
                    v[0] > bx && v[0] < bx + bw && v[1] > by && v[1] < by + bh
                };
                if !(inside(v0) || inside(v1) || inside(v2)) {
                    continue;
                }
                let a = v0[7].max(0.001);
                let rgb = (
                    (v0[4] / a * 255.0).min(255.0) as u8,
                    (v0[5] / a * 255.0).min(255.0) as u8,
                    (v0[6] / a * 255.0).min(255.0) as u8,
                );
                let _ = write!(
                    svg,
                    "<polygon points='{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}' fill='rgb({},{},{})' fill-opacity='{:.2}'/>\n",
                    v0[0], v0[1], v1[0], v1[1], v2[0], v2[1], rgb.0, rgb.1, rgb.2, a
                );
                tris += 1;
            }
            svg.push_str("</svg>\n");
            let out = format!(
                "/private/tmp/claude-501/-Users-admin-makepad-makepad/dc97c21e-85e9-41f6-a8d1-03d180e6bf12/scratchpad/weesperplein_{}.svg",
                std::env::var("TEAR_TAG").unwrap_or_else(|_| "casing".to_string())
            );
            let out = out.as_str();
            std::fs::write(out, svg).unwrap();
            println!("svg: {tris} triangles -> {out}");
        }
    }

    #[test]
    #[ignore]
    fn shadow_bake_probe() {
        let base = std::path::Path::new("../local/maps/europe-shortbread.mbtiles");
        let detail = std::path::Path::new("../local/maps/europe-osm-detail.mbtiles");
        if !base.exists() || !detail.exists() {
            println!("no archives");
            return;
        }
        let mut base_reader = makepad_mbtile_reader::MbtilesReader::open(base).unwrap();
        let mut detail_reader = makepad_mbtile_reader::MbtilesReader::open(detail).unwrap();
        let (x, y) = (8412i64, 5380i64);
        let key = TileKey { z: 14, x: x as i32, y: y as i32 };
        let raw = base_reader.get_tile(14, x, 16383 - y).unwrap().unwrap();
        let det = detail_reader.get_tile(14, x, 16383 - y).unwrap();
        let mut theme = CompiledMapTheme::default();
        theme.shiny.bake_shadows = true;
        theme.shiny.bake_ao = true;
        let buffers = build_tile_buffers_from_mvt(
            key,
            &raw,
            det.as_deref(),
            None,
            false,
            &[],
            &theme,
            17,
            true,
            true,
        )
        .unwrap();
        let shadow_verts = buffers.shadow_disc_vertices.len() / VECTOR_PACKED_FLOATS_PER_VERTEX;
        println!(
            "fill verts {} icon verts {} shadow_disc verts {}",
            buffers.fill_vertices.len() / VECTOR_PACKED_FLOATS_PER_VERTEX,
            buffers.icon_vertices.len() / VECTOR_PACKED_FLOATS_PER_VERTEX,
            shadow_verts
        );
    }

    #[test]
    #[ignore]
    fn artis_full_build_probe() {
        let base = std::path::Path::new("../local/maps/europe-shortbread.mbtiles");
        let detail = std::path::Path::new("../local/maps/noord-holland-detail.mbtiles");
        if !base.exists() || !detail.exists() {
            return;
        }
        let mut base_reader = makepad_mbtile_reader::MbtilesReader::open(base).unwrap();
        let mut detail_reader = makepad_mbtile_reader::MbtilesReader::open(detail).unwrap();
        let y = 5384i64;
        let key = TileKey { z: 14, x: 8415, y: y as i32 };
        let raw = base_reader.get_tile(14, 8415, 16383 - y).unwrap().unwrap();
        let det = detail_reader.get_tile(14, 8415, 16383 - y).unwrap();
        let theme = CompiledMapTheme::default();
        let buffers = build_tile_buffers_from_mvt(
            key,
            &raw,
            det.as_deref(),
            None,
            false,
            &[],
            &theme,
            17,
            false,
            true,
        )
        .unwrap();
        let attraction: Vec<&TileLabel> = buffers
            .labels
            .iter()
            .filter(|label| label.source_layer == "green_area")
            .collect();
        println!("green_area labels: {}", attraction.len());
        for label in attraction.iter() {
            if label.color_class == 3 {
                println!("  ATTRACTION {:?}", label.text);
            }
        }
        println!("total labels: {}", buffers.labels.len());
    }

    #[test]
    #[ignore]
    fn artis_attraction_probe() {
        let path = std::path::Path::new("../local/maps/noord-holland-detail.mbtiles");
        if !path.exists() {
            return;
        }
        let mut reader = makepad_mbtile_reader::TileArchiveReader::open(path).unwrap();
        for y in 5378..=5392 {
            let Some(raw) = reader.get_tile(14, 8415, 16383 - y).unwrap() else {
                continue;
            };
            let key = TileKey { z: 14, x: 8415, y: y as i32 };
            let mut points = Vec::new();
            let mut ways = Vec::new();
            {
                let pbf_data = decode_vector_tile_payload(&raw).unwrap();
                let mut collector = MvtLocalCollector::new(4.0);
                parse_mvt_tile(&pbf_data, key, &mut collector).unwrap();
                let mut max_id = 0i64;
                for way in &collector.ways {
                    if let Some(id) = way
                        .tags
                        .get("__makepad_osm_id")
                        .and_then(|v| v.parse::<i64>().ok())
                    {
                        if way.tags.get("__makepad_osm_type").map(|v| v.as_str()) == Some("way") {
                            max_id = max_id.max(id);
                        }
                        if id == 1391036659 {
                            println!("tile y={} FOUND flamingo way!", y);
                        }
                    }
                }
                println!("tile y={} max way id {}", y, max_id);
            }
            merge_detail_features(
                &raw,
                key,
                4.0,
                true,
                false,
                true,
                &mut points,
                &mut ways,
                &mut Vec::new(),
            )
            .unwrap();
            let mut admitted = 0;
            let mut labeled = 0;
            for way in &ways {
                if way.tags.get("layer").map(|v| v.as_str()) == Some("attraction_area") {
                    admitted += 1;
                    let ring = normalize_polygon_ring(&way.points);
                    let label = ring.as_ref().and_then(|ring| {
                        crate::map::label::extract_area_label(&way.tags, ring_centroid(ring))
                    });
                    if label.is_some() {
                        labeled += 1;
                    }
                    println!(
                        "tile y={} ADMIT {:?} attraction={:?} ring={:?}",
                        y,
                        way.tags.get("name"),
                        way.tags.get("attraction"),
                        ring.as_ref().map(|r| r.len())
                    );
                }
            }
            println!(
                "tile y={} attraction_area ways: {} labeled: {}",
                y, admitted, labeled
            );
        }
    }

    // Diagnostic: print line features near the Reguliersgracht x
    // Keizersgracht bridge to identify the "black dashed fragments".
    // Run: cargo test -p makepad-widgets --features maps bridge_probe -- --nocapture --ignored
    #[test]
    #[ignore]
    fn bridge_probe() {
        let path = std::path::Path::new("../local/maps/europe-shortbread.mbtiles");
        if !path.exists() {
            return;
        }
        let mut reader = makepad_mbtile_reader::TileArchiveReader::open(path).unwrap();
        let raw = reader.get_tile(14, 8414, 16383 - 5386).unwrap().unwrap();
        let data = decode_vector_tile_payload(&raw).unwrap();
        let mut collector = MvtLocalCollector::new(1.0);
        parse_mvt_tile(&data, TileKey { z: 14, x: 8414, y: 5386 }, &mut collector).unwrap();
        let target = (3219.0f32 / 16.0, 3973.0f32 / 16.0);
        for way in &collector.ways {
            let near = way.points.iter().any(|p| {
                let dx = p.0 - target.0;
                let dy = p.1 - target.1;
                dx * dx + dy * dy < (12.0f32) * 12.0
            });
            if near && !way.closed {
                let mut tags: Vec<_> = way.tags.iter().collect();
                tags.sort();
                println!("LINE pts={} {:?}", way.points.len(), tags);
            }
        }
    }

}
