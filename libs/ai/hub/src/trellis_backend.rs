//! The `trellis` backend: mesh domain — TRELLIS.2 image-to-3D through the
//! in-repo port in libs/diffusion (trellis_* on the makepad-ggml CUDA stack):
//! DINOv3 cond -> SS flow + conv3d decode -> cascade shape flows -> sparse
//! FDG decode -> dual-grid mesh -> tex SLAT flow + decode -> GLB (glTF Y-up).
//!
//! Texture path (default): decoded-surface cleanup -> narrow-band UDF dual
//! contouring at the oracle's unprojected offset shell -> QEM decimate
//! to the game-density target -> xatlas unwrap (same `parametrize` Hunyuan
//! uses) -> original-surface-snapped PBR atlas bake (base color +
//! metallic-roughness PNGs embedded in the GLB). Geometry-only jobs still
//! write TEXCOORD_0 so Hunyuan-Paint can retexture the mesh.
//! Texture lookup still snaps to the original decoded surface. This follows
//! TRELLIS.2's production postprocess contract. The raw mesh escape hatch
//! (`remesh_resolution: 0`) carries per-voxel base color as COLOR_0 instead.
//!
//! Layering (h3 pattern): request handling, PNG input decode and artifact
//! shaping compile and test EVERYWHERE — the generator is pluggable, so CI
//! exercises the whole mesh job path with a stub. The real generator
//! (feature `mesh`) replicates trellis-generate's run() stage for stage.
//!
//! Request: `{model: "trellis-2", input_b64: <png>, seed, remesh_resolution,
//! texture, decimation_target, texture_size}` -> one `model/gltf-binary`
//! artifact. `texture: false` skips the tex flow (untextured, faster).
//!
//! Warm residency (measured 2026-08-13 on the 4090 box): NOT applied here.
//! The flux-pattern candidate state (T2Dino + decoder prepares re-uploaded
//! per job) measured ~1.5s of a ~45s warm job — the big DiT matmul weights
//! already persist across jobs in the thread-local device weight cache
//! (namespaces t2ss/t2lr/t2hr/t2tex/t2sdec/t2tdec on the server worker
//! thread), and everything else in the job is real per-input compute. A
//! keep-alive worker owning !Send prepared models is not worth 3%.

use crate::backend::{CancelToken, ArtifactData, BackendCtx, ContentBackend, GenerateParams, ProgressSink};
use crate::error::AssetAiError;
use makepad_zune_core::options::DecoderOptions;
use makepad_zune_png::PngDecoder;

/// One generation request handed to the generator.
#[derive(Clone, Debug)]
pub struct MeshJob {
    /// Tightly packed RGBA8 input image.
    pub rgba: Vec<u8>,
    pub width: usize,
    pub height: usize,
    pub seed: u64,
    /// Narrow-band dual-contouring grid resolution for the output GLB,
    /// RESOLVED from the
    /// request in the shared path: request None -> the service default (256),
    /// request 0 -> `None` here (raw ~10M-face decode mesh, escape hatch),
    /// anything else clamped 16..=512.
    pub remesh_resolution: Option<u32>,
    /// True when the input carries a meaningful alpha matte (pre-segmented
    /// subject — the reference pipeline's rembg contract). False means the
    /// real generator must run the in-process native BiRefNet CUDA stage
    /// before conditioning. It is never permission to continue unsegmented.
    pub segmented: bool,
    /// Run the tex SLAT flow + decode and bake the per-voxel PBR attrs onto
    /// the mesh (UV atlas on retopo'd outputs, COLOR_0 vertex colors on the
    /// raw mesh; service default). False = untextured geometry only.
    pub texture: bool,
    /// Face target for the decimated textured output (game-asset density).
    pub decimation_target: usize,
    /// Baked atlas size in texels.
    pub texture_size: usize,
}

/// Default face target for textured mesh output: mid game-asset density
/// (the 40-100k band the sandbox props budget for).
pub const DECIMATION_DEFAULT: u32 = 80_000;
/// Default baked atlas size.
pub const TEXTURE_SIZE_DEFAULT: u32 = 1024;

/// Pluggable generation: the real path runs the trellis pipeline; tests plug
/// in a closure. Returns the finished GLB bytes.
pub type GenFn = Box<dyn FnMut(&MeshJob, ProgressSink) -> Result<Vec<u8>, AssetAiError> + Send>;

enum Gen {
    Stub(GenFn),
    #[cfg(feature = "mesh")]
    Trellis(trellis_gen::TrellisGen),
}

pub struct TrellisBackend {
    model_id: String,
    gen: Gen,
}

impl TrellisBackend {
    /// Test/CI constructor: generation is the given closure.
    pub fn with_stub(model_id: &str, gen: GenFn) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Stub(gen),
        }
    }

    /// Real constructor used by `create_backend`.
    #[cfg(feature = "mesh")]
    pub fn new_trellis(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Trellis(trellis_gen::TrellisGen::new()),
        }
    }
}

/// The service default narrow-band remesh resolution: raw TRELLIS meshes
/// are ~10M faces / ~180MB — hostile to artifact relay and viewers alike.
pub const REMESH_DEFAULT: u32 = 256;

/// Resolves the wire `remesh_resolution` into the job's:
/// None -> default 256, Some(0) -> None (raw escape hatch), else clamp.
pub fn resolve_remesh(requested: Option<u32>) -> Option<u32> {
    match requested {
        None => Some(REMESH_DEFAULT),
        Some(0) => None,
        Some(n) => Some(n.clamp(16, 512)),
    }
}

/// True when the alpha channel carries a real matte: more than 0.5% of
/// pixels meaningfully transparent (alpha < 250). Fully opaque photos (or
/// stray-noise alpha) return false — those are scene shots that require the
/// native matte stage.
pub fn alpha_is_segmented(rgba: &[u8]) -> bool {
    let total = rgba.len() / 4;
    if total == 0 {
        return false;
    }
    let transparent = rgba
        .chunks_exact(4)
        .filter(|px| px[3] < 250)
        .count();
    transparent * 200 > total // > 0.5%
}

/// Reject the characteristic TRELLIS reconstruction-volume sheet failure.
///
/// A failed O-Voxel sample can put one or more nearly planar surfaces on a
/// face of the normalized reconstruction cube.  They are not small floaters:
/// the sheets can be the largest connected components in the mesh, so the
/// ordinary `drop_small_components` cleanup deliberately cannot distinguish
/// them from the requested subject.  Passing one onward makes rigging appear
/// successful while skinning a floor-sized slab into the character.
///
/// This is a fail-closed validator, not a repair pass.  It never changes the
/// mesh.  The two checks are intentionally geometric and narrow:
///
/// * a dominant connected component must not be both near-zero-thickness and
///   span most of the reconstruction boundary in the other two axes;
/// * near-coplanar, full-span components must not collectively form a large
///   sheet anywhere inside the reconstruction volume;
/// * the union of faces in a thin boundary slab must not occupy a substantial
///   fraction of the entire mesh while spanning that boundary face;
/// * a connectivity-independent band of axis-facing triangles must not form
///   a large full-span sheet (the sheet can be welded into both legs);
/// * the final surface must not be dominated by thousands of tiny disconnected
///   fragments.
///
/// The second check catches a sheet split across several components.  A normal
/// foot sole can touch the minimum Y plane, but it neither consumes 12% of all
/// faces nor spans 80% of both other reconstruction axes.
/// Piecewise-linear map from the generator's internal progress timeline to
/// the fraction shown to clients. Knots are (internal, display); measured
/// on an RTX PRO 6000 for an ~80k-face result: forward 28s, weld+fill 9s,
/// BVH+field 3s, remesh/decimate 8s, xatlas unwrap 20-40s, bake 2s.
pub fn display_fraction(internal: f64) -> f64 {
    const KNOTS: [(f64, f64); 10] = [
        (0.000, 0.00),
        (0.880, 0.42), // native forward done
        (0.890, 0.52), // weld + fill holes
        (0.900, 0.56), // BVH + remesh field
        (0.937, 0.62), // remesh faces, weld/fill/drop
        (0.960, 0.70), // decimate
        (0.967, 0.73), // final weld/fill/drop, orient, sampler
        (0.973, 0.92), // xatlas unwrap
        (0.980, 0.97), // texel bake
        (1.000, 1.00), // encode + done
    ];
    let x = if internal.is_finite() { internal.clamp(0.0, 1.0) } else { 0.0 };
    for pair in KNOTS.windows(2) {
        let (x0, y0) = pair[0];
        let (x1, y1) = pair[1];
        if x <= x1 {
            return y0 + (y1 - y0) * ((x - x0) / (x1 - x0));
        }
    }
    1.0
}

/// Advisory only: the reconstruction is logged against these heuristics but
/// never rejected. Whatever TRELLIS produced is what the user gets.
pub fn check_trellis_mesh_quality(
    positions: &[[f32; 3]],
    indices: &[u32],
) -> Result<(), String> {
    const DOMINANT_COMPONENT_FACE_FRACTION: f32 = 0.10;
    const MIN_SHEET_FACES: usize = 128;
    const THIN_COMPONENT_FRACTION: f32 = 0.012;
    const PROJECTED_BOUNDARY_SPAN: f32 = 0.80;
    const BOUNDARY_SLAB_FRACTION: f32 = 0.015;
    const BOUNDARY_FACE_FRACTION: f32 = 0.12;
    const INTERIOR_SHEET_COMPONENT_FACE_FRACTION: f32 = 0.025;
    const INTERIOR_SHEET_THICKNESS_FRACTION: f32 = 0.04;
    const INTERIOR_SHEET_GROUP_GAP_FRACTION: f32 = 0.015;
    const INTERIOR_SHEET_FACE_FRACTION: f32 = 0.10;
    const PLANAR_BAND_THICKNESS_FRACTION: f32 = 0.02;
    const PLANAR_BAND_NORMAL_ALIGNMENT: f32 = 0.90;
    const PLANAR_BAND_FACE_FRACTION: f32 = 0.05;
    const FRAGMENT_COMPONENT_FLOOR: usize = 1024;
    const FRAGMENT_COMPONENT_FACE_DENSITY: f32 = 0.08;
    const TINY_COMPONENT_FACE_CEILING: usize = 4;
    const TINY_COMPONENT_FRACTION: f32 = 0.60;

    let face_count = indices.len() / 3;
    if positions.is_empty() || face_count == 0 {
        return Err(format!(
            "empty generated mesh (vertices={}, faces={face_count})",
            positions.len()
        ));
    }
    if indices.len() % 3 != 0 {
        return Err(format!(
            "triangle index count {} is not divisible by three",
            indices.len()
        ));
    }

    let mut referenced = vec![false; positions.len()];
    let mut bounds_min = [f32::INFINITY; 3];
    let mut bounds_max = [f32::NEG_INFINITY; 3];
    for &index in indices {
        let index = index as usize;
        let Some(&position) = positions.get(index) else {
            return Err(format!(
                "triangle index {index} is out of range for {} vertices",
                positions.len()
            ));
        };
        if !position.iter().all(|value| value.is_finite()) {
            return Err(format!("vertex {index} contains a non-finite coordinate"));
        }
        if referenced[index] {
            continue;
        }
        referenced[index] = true;
        for axis in 0..3 {
            bounds_min[axis] = bounds_min[axis].min(position[axis]);
            bounds_max[axis] = bounds_max[axis].max(position[axis]);
        }
    }
    let extent = [
        bounds_max[0] - bounds_min[0],
        bounds_max[1] - bounds_min[1],
        bounds_max[2] - bounds_min[2],
    ];
    if extent.iter().any(|value| !value.is_finite() || *value <= 1.0e-6) {
        return Err(format!(
            "degenerate generated-mesh bounds: min={bounds_min:?} max={bounds_max:?} extent={extent:?}"
        ));
    }

    // First catch one dominant sheet component. Union-find is over vertex
    // connectivity before UV chart splitting, at the final geometry boundary
    // that is handed to the GLB writer.
    let mut parent: Vec<u32> = (0..positions.len() as u32).collect();
    fn find(parent: &mut [u32], mut node: u32) -> u32 {
        while parent[node as usize] != node {
            parent[node as usize] = parent[parent[node as usize] as usize];
            node = parent[node as usize];
        }
        node
    }
    fn union(parent: &mut [u32], a: u32, b: u32) {
        let a = find(parent, a);
        let b = find(parent, b);
        if a != b {
            parent[a as usize] = b;
        }
    }
    for triangle in indices.chunks_exact(3) {
        union(&mut parent, triangle[0], triangle[1]);
        union(&mut parent, triangle[1], triangle[2]);
    }

    let mut component_faces = vec![0usize; positions.len()];
    for triangle in indices.chunks_exact(3) {
        let root = find(&mut parent, triangle[0]) as usize;
        component_faces[root] += 1;
    }
    let mut component_min = vec![[f32::INFINITY; 3]; positions.len()];
    let mut component_max = vec![[f32::NEG_INFINITY; 3]; positions.len()];
    for (index, &position) in positions.iter().enumerate() {
        if !referenced[index] {
            continue;
        }
        let root = find(&mut parent, index as u32) as usize;
        for axis in 0..3 {
            component_min[root][axis] = component_min[root][axis].min(position[axis]);
            component_max[root][axis] = component_max[root][axis].max(position[axis]);
        }
    }
    const AXIS_NAME: [&str; 3] = ["x", "y", "z"];
    for root in 0..component_faces.len() {
        let faces = component_faces[root];
        let face_fraction = faces as f32 / face_count as f32;
        if faces < MIN_SHEET_FACES || face_fraction < DOMINANT_COMPONENT_FACE_FRACTION {
            continue;
        }
        let component_extent = [
            component_max[root][0] - component_min[root][0],
            component_max[root][1] - component_min[root][1],
            component_max[root][2] - component_min[root][2],
        ];
        for thin_axis in 0..3 {
            if component_extent[thin_axis]
                > extent[thin_axis] * THIN_COMPONENT_FRACTION
            {
                continue;
            }
            let other_axes: Vec<usize> = (0..3).filter(|axis| *axis != thin_axis).collect();
            let span_a = component_extent[other_axes[0]] / extent[other_axes[0]];
            let span_b = component_extent[other_axes[1]] / extent[other_axes[1]];
            if span_a < PROJECTED_BOUNDARY_SPAN || span_b < PROJECTED_BOUNDARY_SPAN {
                continue;
            }
            let slab = extent[thin_axis] * BOUNDARY_SLAB_FRACTION;
            let min_gap = component_min[root][thin_axis] - bounds_min[thin_axis];
            let max_gap = bounds_max[thin_axis] - component_max[root][thin_axis];
            let (boundary, gap) = if min_gap <= max_gap {
                ("min", min_gap)
            } else {
                ("max", max_gap)
            };
            if gap <= slab {
                return Err(format!(
                    "dominant reconstruction-boundary sheet: component={root} faces={faces}/{face_count} ({:.1}%), boundary={boundary}-{} gap={gap:.6}, thickness={:.6} ({:.2}% of axis), projected_span=({:.1}%, {:.1}%), mesh_bounds={bounds_min:?}..{bounds_max:?}",
                    face_fraction * 100.0,
                    AXIS_NAME[thin_axis],
                    component_extent[thin_axis],
                    component_extent[thin_axis] / extent[thin_axis] * 100.0,
                    span_a * 100.0,
                    span_b * 100.0,
                ));
            }
        }
    }

    // A later failure mode produced two full X/Z sheets slightly above the
    // minimum Y bound. Each was below the single-component threshold and the
    // pair sat outside the boundary slab, but together they consumed almost
    // 12% of the mesh. Group full-span thin components by overlapping (or
    // immediately adjacent) intervals on their thin axis and reject their
    // aggregate anywhere in the reconstruction volume.
    for thin_axis in 0..3 {
        let other_axes: Vec<usize> = (0..3).filter(|axis| *axis != thin_axis).collect();
        let mut candidates: Vec<usize> = (0..component_faces.len())
            .filter(|&root| {
                let faces = component_faces[root];
                if faces < MIN_SHEET_FACES
                    || faces as f32 / (face_count as f32)
                        < INTERIOR_SHEET_COMPONENT_FACE_FRACTION
                {
                    return false;
                }
                let component_extent = [
                    component_max[root][0] - component_min[root][0],
                    component_max[root][1] - component_min[root][1],
                    component_max[root][2] - component_min[root][2],
                ];
                component_extent[thin_axis]
                    <= extent[thin_axis] * INTERIOR_SHEET_THICKNESS_FRACTION
                    && component_extent[other_axes[0]] / extent[other_axes[0]]
                        >= PROJECTED_BOUNDARY_SPAN
                    && component_extent[other_axes[1]] / extent[other_axes[1]]
                        >= PROJECTED_BOUNDARY_SPAN
            })
            .collect();
        candidates.sort_unstable_by(|&a, &b| {
            component_min[a][thin_axis]
                .total_cmp(&component_min[b][thin_axis])
        });

        let group_gap = extent[thin_axis] * INTERIOR_SHEET_GROUP_GAP_FRACTION;
        let mut at = 0usize;
        while at < candidates.len() {
            let first = candidates[at];
            let mut end = at + 1;
            let mut group_min = component_min[first][thin_axis];
            let mut group_max = component_max[first][thin_axis];
            let mut group_faces = component_faces[first];
            let mut projected_min = [f32::INFINITY; 3];
            let mut projected_max = [f32::NEG_INFINITY; 3];
            for &axis in &other_axes {
                projected_min[axis] = component_min[first][axis];
                projected_max[axis] = component_max[first][axis];
            }
            while end < candidates.len()
                && component_min[candidates[end]][thin_axis] <= group_max + group_gap
            {
                let root = candidates[end];
                group_min = group_min.min(component_min[root][thin_axis]);
                group_max = group_max.max(component_max[root][thin_axis]);
                group_faces += component_faces[root];
                for &axis in &other_axes {
                    projected_min[axis] = projected_min[axis].min(component_min[root][axis]);
                    projected_max[axis] = projected_max[axis].max(component_max[root][axis]);
                }
                end += 1;
            }
            let face_fraction = group_faces as f32 / face_count as f32;
            let span_a = (projected_max[other_axes[0]] - projected_min[other_axes[0]])
                / extent[other_axes[0]];
            let span_b = (projected_max[other_axes[1]] - projected_min[other_axes[1]])
                / extent[other_axes[1]];
            if face_fraction >= INTERIOR_SHEET_FACE_FRACTION
                && span_a >= PROJECTED_BOUNDARY_SPAN
                && span_b >= PROJECTED_BOUNDARY_SPAN
            {
                return Err(format!(
                    "aggregate near-coplanar reconstruction sheet: axis={} interval=[{group_min:.6}, {group_max:.6}] thickness={:.6} ({:.2}% of axis), components={} faces={group_faces}/{face_count} ({:.1}%), projected_span=({:.1}%, {:.1}%), mesh_bounds={bounds_min:?}..{bounds_max:?}",
                    AXIS_NAME[thin_axis],
                    group_max - group_min,
                    (group_max - group_min) / extent[thin_axis] * 100.0,
                    end - at,
                    face_fraction * 100.0,
                    span_a * 100.0,
                    span_b * 100.0,
                ));
            }
            at = end;
        }
    }

    // Then aggregate faces across components in each reconstruction-boundary
    // slab. This rejects several cooperating sheet fragments without treating
    // a local flat feature (a shoe sole, brim, or saddle) as a floor plane.
    for axis in 0..3 {
        let other_axes: Vec<usize> = (0..3).filter(|candidate| *candidate != axis).collect();
        let slab = extent[axis] * BOUNDARY_SLAB_FRACTION;
        for side in 0..2 {
            let mut slab_faces = 0usize;
            let mut projected_min = [f32::INFINITY; 3];
            let mut projected_max = [f32::NEG_INFINITY; 3];
            for triangle in indices.chunks_exact(3) {
                let inside = triangle.iter().all(|&index| {
                    let value = positions[index as usize][axis];
                    if side == 0 {
                        value <= bounds_min[axis] + slab
                    } else {
                        value >= bounds_max[axis] - slab
                    }
                });
                if !inside {
                    continue;
                }
                slab_faces += 1;
                for &index in triangle {
                    let position = positions[index as usize];
                    for &projected_axis in &other_axes {
                        projected_min[projected_axis] =
                            projected_min[projected_axis].min(position[projected_axis]);
                        projected_max[projected_axis] =
                            projected_max[projected_axis].max(position[projected_axis]);
                    }
                }
            }
            let face_fraction = slab_faces as f32 / face_count as f32;
            if slab_faces < MIN_SHEET_FACES || face_fraction < BOUNDARY_FACE_FRACTION {
                continue;
            }
            let span_a = (projected_max[other_axes[0]] - projected_min[other_axes[0]])
                / extent[other_axes[0]];
            let span_b = (projected_max[other_axes[1]] - projected_min[other_axes[1]])
                / extent[other_axes[1]];
            if span_a >= PROJECTED_BOUNDARY_SPAN && span_b >= PROJECTED_BOUNDARY_SPAN {
                return Err(format!(
                    "excessive reconstruction-boundary occupancy: boundary={}-{} slab={slab:.6}, faces={slab_faces}/{face_count} ({:.1}%), projected_span=({:.1}%, {:.1}%), mesh_bounds={bounds_min:?}..{bounds_max:?}",
                    if side == 0 { "min" } else { "max" },
                    AXIS_NAME[axis],
                    face_fraction * 100.0,
                    span_a * 100.0,
                    span_b * 100.0,
                ));
            }
        }
    }

    // Atlas generation splits vertices at chart seams, which made the saved
    // regression look like two thin components. At the live pre-atlas gate,
    // however, the same reconstruction sheet touches both shins and belongs
    // to the character's full-height component. Detect the geometry itself,
    // independent of connectivity: collect triangles that fit a 2%-thick
    // band and whose normals face that band's thin axis. Two half-band phases
    // keep a sheet centered on a bin edge from being divided below threshold.
    #[derive(Clone)]
    struct PlanarBand {
        faces: usize,
        min: [f32; 3],
        max: [f32; 3],
    }
    impl PlanarBand {
        fn empty() -> Self {
            Self {
                faces: 0,
                min: [f32::INFINITY; 3],
                max: [f32::NEG_INFINITY; 3],
            }
        }
    }
    for thin_axis in 0..3 {
        let other_axes = match thin_axis {
            0 => [1, 2],
            1 => [0, 2],
            _ => [0, 1],
        };
        let band_thickness = extent[thin_axis] * PLANAR_BAND_THICKNESS_FRACTION;
        let band_count = (extent[thin_axis] / band_thickness).ceil() as usize + 2;
        for phase in [0.0f32, 0.5] {
            let mut bands = vec![PlanarBand::empty(); band_count];
            for triangle in indices.chunks_exact(3) {
                let points = [
                    positions[triangle[0] as usize],
                    positions[triangle[1] as usize],
                    positions[triangle[2] as usize],
                ];
                let triangle_min = points
                    .iter()
                    .map(|point| point[thin_axis])
                    .fold(f32::INFINITY, f32::min);
                let triangle_max = points
                    .iter()
                    .map(|point| point[thin_axis])
                    .fold(f32::NEG_INFINITY, f32::max);
                if triangle_max - triangle_min > band_thickness {
                    continue;
                }
                let u = [
                    points[1][0] - points[0][0],
                    points[1][1] - points[0][1],
                    points[1][2] - points[0][2],
                ];
                let v = [
                    points[2][0] - points[0][0],
                    points[2][1] - points[0][1],
                    points[2][2] - points[0][2],
                ];
                let normal = [
                    u[1] * v[2] - u[2] * v[1],
                    u[2] * v[0] - u[0] * v[2],
                    u[0] * v[1] - u[1] * v[0],
                ];
                let normal_length_squared =
                    normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2];
                if normal_length_squared <= 1.0e-20
                    || normal[thin_axis] * normal[thin_axis]
                        < PLANAR_BAND_NORMAL_ALIGNMENT
                            * PLANAR_BAND_NORMAL_ALIGNMENT
                            * normal_length_squared
                {
                    continue;
                }
                let centroid = (points[0][thin_axis]
                    + points[1][thin_axis]
                    + points[2][thin_axis])
                    / 3.0;
                let bucket = (((centroid - bounds_min[thin_axis]) / band_thickness + phase)
                    .floor()
                    .max(0.0) as usize)
                    .min(band_count - 1);
                let band = &mut bands[bucket];
                band.faces += 1;
                for point in points {
                    for axis in 0..3 {
                        band.min[axis] = band.min[axis].min(point[axis]);
                        band.max[axis] = band.max[axis].max(point[axis]);
                    }
                }
            }
            for (bucket, band) in bands.into_iter().enumerate() {
                let face_fraction = band.faces as f32 / face_count as f32;
                if band.faces < MIN_SHEET_FACES || face_fraction < PLANAR_BAND_FACE_FRACTION {
                    continue;
                }
                let span_a =
                    (band.max[other_axes[0]] - band.min[other_axes[0]]) / extent[other_axes[0]];
                let span_b =
                    (band.max[other_axes[1]] - band.min[other_axes[1]]) / extent[other_axes[1]];
                if span_a < PROJECTED_BOUNDARY_SPAN || span_b < PROJECTED_BOUNDARY_SPAN {
                    continue;
                }
                let centroid_min = bounds_min[thin_axis]
                    + (bucket as f32 - phase) * band_thickness;
                let centroid_max = centroid_min + band_thickness;
                return Err(format!(
                    "excessive axis-aligned planar band: axis={} phase={phase:.1} centroid_interval=[{centroid_min:.6}, {centroid_max:.6}], aligned_faces={}/{face_count} ({:.1}%), projected_span=({:.1}%, {:.1}%), triangle_extent={:?}..{:?}, mesh_bounds={bounds_min:?}..{bounds_max:?}",
                    AXIS_NAME[thin_axis],
                    band.faces,
                    face_fraction * 100.0,
                    span_a * 100.0,
                    span_b * 100.0,
                    band.min,
                    band.max,
                ));
            }
        }
    }

    // The remeshed character surface may legitimately contain separate
    // accessories and nested shells. Thousands of components averaging only
    // a handful of faces are different: that is a collapsed/fragmented
    // reconstruction, and feeding it to SkinTokens creates a plausible-looking
    // skeleton over unusable geometry. Require all three signals so ordinary
    // multipart characters and the chart-split clean regression remain valid.
    let component_count = component_faces.iter().filter(|&&faces| faces > 0).count();
    let tiny_components = component_faces
        .iter()
        .filter(|&&faces| faces > 0 && faces <= TINY_COMPONENT_FACE_CEILING)
        .count();
    let component_density = component_count as f32 / face_count as f32;
    let tiny_fraction = tiny_components as f32 / component_count as f32;
    if component_count >= FRAGMENT_COMPONENT_FLOOR
        && component_density >= FRAGMENT_COMPONENT_FACE_DENSITY
        && tiny_fraction >= TINY_COMPONENT_FRACTION
    {
        let largest_component_faces = component_faces.iter().copied().max().unwrap_or(0);
        return Err(format!(
            "severe generated-mesh fragmentation: components={component_count}, faces={face_count}, component_density={:.1}%, tiny_components(<= {TINY_COMPONENT_FACE_CEILING} faces)={tiny_components}/{component_count} ({:.1}%), largest_component_faces={largest_component_faces}, mesh_bounds={bounds_min:?}..{bounds_max:?}",
            component_density * 100.0,
            tiny_fraction * 100.0,
        ));
    }
    Ok(())
}

/// Decodes a PNG into tightly packed RGBA8 (alpha filled for RGB inputs —
/// trellis preprocessing wants the alpha channel for object cropping).
pub fn decode_png_rgba8(bytes: &[u8]) -> Result<(Vec<u8>, usize, usize), AssetAiError> {
    let bad = |detail: String| AssetAiError::Params(format!("input_b64 png: {detail}"));
    let options = DecoderOptions::default().png_set_strip_to_8bit(true);
    let mut decoder = PngDecoder::new_with_options(std::io::Cursor::new(bytes), options);
    decoder
        .decode_headers()
        .map_err(|err| bad(format!("{err:?}")))?;
    let info = decoder.info().cloned().ok_or_else(|| bad("no info".into()))?;
    let colorspace = decoder
        .colorspace()
        .ok_or_else(|| bad("no colorspace".into()))?;
    let pixels = decoder.decode_raw().map_err(|err| bad(format!("{err:?}")))?;
    let components = colorspace.num_components();
    let (w, h) = (info.width as usize, info.height as usize);
    let mut rgba = vec![0u8; w * h * 4];
    for (i, chunk) in pixels.chunks_exact(components).enumerate() {
        match components {
            4 => rgba[i * 4..i * 4 + 4].copy_from_slice(chunk),
            3 => {
                rgba[i * 4..i * 4 + 3].copy_from_slice(chunk);
                rgba[i * 4 + 3] = 255;
            }
            // Grayscale / grayscale+alpha (common for hand-authored inpaint
            // masks): replicate luma across R/G/B.
            2 => {
                rgba[i * 4..i * 4 + 3].copy_from_slice(&[chunk[0]; 3]);
                rgba[i * 4 + 3] = chunk[1];
            }
            1 => {
                rgba[i * 4..i * 4 + 3].copy_from_slice(&[chunk[0]; 3]);
                rgba[i * 4 + 3] = 255;
            }
            _ => return Err(bad(format!("{components} components unsupported"))),
        }
    }
    Ok((rgba, w, h))
}

impl ContentBackend for TrellisBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, _ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        match &mut self.gen {
            Gen::Stub(_) => Ok(()),
            #[cfg(feature = "mesh")]
            Gen::Trellis(gen) => gen.ensure_loaded(_ctx),
        }
    }

    fn is_resident(&self) -> bool {
        match &self.gen {
            Gen::Stub(_) => false,
            #[cfg(feature = "mesh")]
            Gen::Trellis(gen) => gen.is_resident(),
        }
    }

    fn unload(&mut self) -> Result<(), AssetAiError> {
        match &mut self.gen {
            Gen::Stub(_) => Ok(()),
            #[cfg(feature = "mesh")]
            Gen::Trellis(gen) => gen.unload(),
        }
    }

    fn resident_is_healthy_after_error(&self, error: &AssetAiError) -> bool {
        // Parameter validation and cancellation happen without touching the
        // weights or the CUDA cache, so `/models` stays ready/loaded;
        // ordinary backend/CUDA errors stay conservative.
        matches!(error, AssetAiError::Cancelled | AssetAiError::Params(_))
    }

    fn generate(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        if params.input_bytes.is_empty() {
            return Err(AssetAiError::Params(
                "trellis-2 needs an input image (input_b64 png)".to_string(),
            ));
        }
        cancel.check()?;
        progress("preprocess", 0.02);
        let (rgba, width, height) = decode_png_rgba8(&params.input_bytes)?;
        // The real generator owns native matting so it runs on the same CUDA
        // worker and can explicitly release BiRefNet before TRELLIS streams
        // its much larger weights. Stubs retain the segmentation bit for
        // request-path tests without needing a GPU.
        let segmented = alpha_is_segmented(&rgba);

        let job = MeshJob {
            rgba,
            width,
            height,
            seed: params.seed,
            remesh_resolution: resolve_remesh(params.remesh_resolution),
            segmented,
            texture: params.texture.unwrap_or(true),
            decimation_target: params
                .decimation_target
                .unwrap_or(DECIMATION_DEFAULT)
                .clamp(1_000, 2_000_000) as usize,
            texture_size: params
                .texture_size
                .unwrap_or(TEXTURE_SIZE_DEFAULT)
                .clamp(256, 4096) as usize,
        };
        cancel.check()?;
        // The generator reports on an internal timeline (native forward
        // 0..0.88, then the CPU post-processing squeezed into 0.88..0.98).
        // Wall time is the other way round on a real mesh — the unwrap alone
        // outlasts the whole forward — so the client-visible fraction is
        // re-banded to roughly follow elapsed time.
        let mut display = |label: &str, internal: f64| progress(label, display_fraction(internal));
        let bytes = match &mut self.gen {
            Gen::Stub(gen) => gen(&job, &mut display)?,
            #[cfg(feature = "mesh")]
            Gen::Trellis(gen) => gen.generate(&job, &mut display, cancel)?,
        };
        cancel.check()?;
        Ok(vec![ArtifactData {
            content_type: "model/gltf-binary",
            ext: "glb",
            bytes,
        }])
    }
}

// ---------------------------------------------------------------------------
// Real generation through libs/diffusion (feature mesh)
// ---------------------------------------------------------------------------

#[cfg(feature = "mesh")]
mod trellis_gen {
    use super::MeshJob;
    use crate::backend::{BackendCtx, CancelToken, ProgressSink};
    use crate::error::AssetAiError;
    use makepad_ai_common::backend::gpu_pool_cap_override;
    use makepad_ai_vision::birefnet::{
        unload_birefnet, BiRefNet, BiRefNetImage, BiRefNetWeights,
    };
    use makepad_ai_h3::h3_pipeline::H3NoiseRng;
    use makepad_ai_trellis::trellis::{
        t2_quantize_unique_coords, t2_rope_tables, TrellisWeights, T2_SHAPE_SAMPLER,
        T2_SHAPE_SLAT_MEAN, T2_SHAPE_SLAT_STD, T2_SLAT_CHANNELS, T2_SS_CHANNELS, T2_SS_SAMPLER,
        T2_SS_TOKENS, T2_TEX_IN_CHANNELS, T2_TEX_SAMPLER, T2_TEX_SLAT_MEAN, T2_TEX_SLAT_STD,
    };
    use makepad_ai_trellis::trellis_dino::T2Dino;
    use makepad_ai_trellis::trellis_dit::{t2_upload_cond, t2_upload_rope, T2Dit};
    use makepad_ai_trellis::trellis_image::{
        t2_cond_input, t2_pad_black, t2_preprocess_rgba, t2_subject_border, T2Image,
    };
    use makepad_ai_trellis::trellis_mesh::{
        t2_dual_grid_to_mesh, t2_fdg_fields, t2_mesh_to_glb_colored, t2_yup, T2VoxelSampler,
    };
    use makepad_ai_trellis::trellis_pipeline::{
        t2_chw_to_tokens, t2_run_ss_cancel, t2_sample_flow_cancel, t2_sample_flow_concat_ctl,
    };
    use makepad_ai_trellis::trellis_slat::T2SparseDec;
    use makepad_ai_trellis::trellis_vae::T2SsDec;
    use makepad_ai_common::DiffusionError;
    use makepad_remesh::{remesh_narrow_band_dc_ctl, SurfaceBvh};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    /// Job-status writes are expensive. Forward a tick on phase change or
    /// every ~700ms — enough to show life, not enough to stall remesh.
    struct CoarseProgress<'a> {
        sink: ProgressSink<'a>,
        last: Instant,
        last_label: String,
    }

    impl<'a> CoarseProgress<'a> {
        fn new(sink: ProgressSink<'a>) -> Self {
            Self {
                sink,
                last: Instant::now() - Duration::from_secs(1),
                last_label: String::new(),
            }
        }

        fn emit(&mut self, label: &str, frac: f64) {
            let now = Instant::now();
            if label == self.last_label && now.duration_since(self.last) < Duration::from_millis(700)
            {
                return;
            }
            self.last = now;
            self.last_label.clear();
            self.last_label.push_str(label);
            (self.sink)(label, frac);
        }
    }

    /// The registry files this backend loads (tex flow + tex decoder only
    /// when the job asks for texture — the service default).
    struct Paths {
        /// `<cache_dir>/trellis`: where the last pre-unwrap mesh is kept so a
        /// hung or ugly unwrap can be replayed offline (single overwritten
        /// file, a few MB).
        scratch: PathBuf,
        ss_flow: PathBuf,
        lr_flow: PathBuf,
        hr_flow: PathBuf,
        shape_dec: PathBuf,
        dino: PathBuf,
        ss_dec: PathBuf,
        tex_flow: PathBuf,
        tex_dec: PathBuf,
        matte: PathBuf,
    }

    pub struct TrellisGen {
        paths: Option<Paths>,
        /// The large DiT/sparse-decoder matrices live in makepad-ggml's
        /// thread-local device cache after the first generation. Keep this
        /// explicit: reporting the backend as stateless while those matrices
        /// occupied VRAM made a following native rig job overcommit a 24 GB
        /// card even though the common lifecycle believed TRELLIS was gone.
        resident: bool,
    }

    fn trellis_err(err: impl std::fmt::Display) -> AssetAiError {
        AssetAiError::Backend(format!("trellis: {err}"))
    }

    /// Diffusion-error mapper that keeps cancellation its own kind (job
    /// state "cancelled", not an error).
    fn diffusion_err(err: DiffusionError) -> AssetAiError {
        match err {
            DiffusionError::Cancelled => AssetAiError::Cancelled,
            other => AssetAiError::Backend(format!("trellis: {other}")),
        }
    }

    impl TrellisGen {
        pub fn new() -> Self {
            Self {
                paths: None,
                resident: false,
            }
        }

        pub fn is_resident(&self) -> bool {
            self.resident
        }

        /// Evict every TRELLIS namespace populated by the generation path.
        /// DINO and the dense SS decoder are owned GPU tensors and drop at the
        /// end of a job; the six namespaces below are the persistent part.
        /// This must run on the service worker thread which performed the
        /// generation because both caches are thread-local by design.
        pub fn unload(&mut self) -> Result<(), AssetAiError> {
            use makepad_ai_common::backend::{
                gpu_pool_clear, gpu_weight_cache_evict_prefix,
            };

            unload_birefnet().map_err(diffusion_err)?;
            for namespace in ["t2ss", "t2lr", "t2hr", "t2tex", "t2sdec", "t2tdec"] {
                gpu_weight_cache_evict_prefix(&format!("{namespace}::"))
                    .map_err(trellis_err)?;
            }
            gpu_pool_clear();
            self.resident = false;
            Ok(())
        }

        pub fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
            ctx.ensure_files()?;
            self.paths = Some(Paths {
                scratch: ctx.cache_dir.join("trellis"),
                ss_flow: ctx.path_by_role("ss-flow")?,
                lr_flow: ctx.path_by_role("shape-flow-512")?,
                hr_flow: ctx.path_by_role("shape-flow-1024")?,
                shape_dec: ctx.path_by_role("shape-decoder")?,
                dino: ctx.path_by_role("dino-conditioner")?,
                ss_dec: ctx.path_by_role("ss-decoder")?,
                tex_flow: ctx.path_by_role("texture-flow-1024")?,
                tex_dec: ctx.path_by_role("texture-decoder")?,
                matte: ctx.path_by_role("native-matte")?,
            });
            Ok(())
        }

        pub fn generate(
            &mut self,
            job: &MeshJob,
            progress: ProgressSink,
            cancel: &CancelToken,
        ) -> Result<Vec<u8>, AssetAiError> {
            let paths = self
                .paths
                .as_ref()
                .ok_or_else(|| AssetAiError::Backend("trellis used before ensure_loaded".into()))?;
            // From this point on an error or cancellation may still leave a
            // partially populated device cache. Mark it resident before the
            // first upload so the service will call `unload` on every unwind.
            self.resident = true;
            let cancelled = || cancel.is_cancelled();

            // TRELLIS requires a segmented subject. Opaque inputs are matted
            // here, in process, on the same Rust/CUDA worker. BiRefNet is
            // deliberately dropped and its device namespace evicted before
            // any TRELLIS component loads, keeping the 24 GB path viable.
            let mut rgba = job.rgba.clone();
            if !job.segmented {
                progress("matte load", 0.0205);
                let matte_result = (|| -> Result<_, DiffusionError> {
                    let weights = BiRefNetWeights::load(&paths.matte)?;
                    let mut load_hook = |label: &str, fraction: f64| {
                        if cancelled() {
                            return Err(DiffusionError::Cancelled);
                        }
                        progress(label, 0.021 + 0.002 * fraction.clamp(0.0, 1.0));
                        Ok(())
                    };
                    let model = BiRefNet::prepare_controlled(
                        &weights,
                        Some(&cancelled),
                        Some(&mut load_hook),
                    )?;
                    let input = BiRefNetImage::rgba8(&rgba, job.width, job.height)?;
                    let mut matte_hook = |label: &str, fraction: f64| {
                        if cancelled() {
                            return Err(DiffusionError::Cancelled);
                        }
                        progress(label, 0.023 + 0.006 * fraction.clamp(0.0, 1.0));
                        Ok(())
                    };
                    model.matte_controlled(
                        input,
                        Some(&cancelled),
                        Some(&mut matte_hook),
                    )
                })();
                // Always release partially or fully populated BiRefNet caches,
                // including on cancellation or an operator error.
                let cleanup_result = unload_birefnet();
                cleanup_result.map_err(diffusion_err)?;
                let matte = matte_result.map_err(diffusion_err)?;
                for (pixel, alpha) in rgba
                    .chunks_exact_mut(4)
                    .zip(matte.alpha_u8().into_iter())
                {
                    pixel[3] = alpha;
                }
                progress("matte complete", 0.029);
                cancel.check()?;
            }

            // Mirrors trellis-generate's run(): preprocess -> prepare all
            // models (weight caches are process-global per namespace, so jobs
            // after the first are warm) -> cond -> ss -> lr -> upsample ->
            // hr -> decode -> mesh -> glb.
            let img = T2Image::from_rgba8(&rgba, job.width, job.height).map_err(trellis_err)?;
            let pre = t2_preprocess_rgba(&img).map_err(trellis_err)?;
            // VisualBruno's TRELLIS.2 workflows pad after the alpha crop so
            // the subject does not sit on the conditioner frame. A fixed
            // 10 px is not enough for a headshot or other tight matte: after
            // DINO resize the silhouette still kisses the O-Voxel wall and
            // decodes as a unit-cube floor sheet. Scale the border with the
            // cropped subject instead.
            let border = t2_subject_border(pre.width.min(pre.height));
            let pre = t2_pad_black(&pre, border).map_err(trellis_err)?;

            // Model prepare, one progress tick + cancel boundary per
            // component (headers/host tensors here; the GB-class weight
            // streams happen lazily inside the first forwards).
            let loads = if job.texture { 8 } else { 6 };
            progress(&format!("load dino 1/{loads}"), 0.03);
            let dino_weights = TrellisWeights::load(&paths.dino).map_err(trellis_err)?;
            // DINO is the one component whose weights are read from disk AND
            // uploaded at prepare time (the DiTs stream lazily inside their
            // first forwards, under the per-step ticks below) — its prepare
            // ticks "load dino block k/24" with a cancel boundary each.
            let dino = {
                let mut dino_hook = |label: &str, fraction: f64| -> Result<(), DiffusionError> {
                    if cancel.is_cancelled() {
                        return Err(DiffusionError::Cancelled);
                    }
                    progress(label, 0.03 + 0.01 * fraction.clamp(0.0, 1.0));
                    Ok(())
                };
                T2Dino::prepare_with_progress(&dino_weights, Some(&mut dino_hook))
                    .map_err(diffusion_err)?
            };
            cancel.check()?;
            progress(&format!("load ss-flow 2/{loads}"), 0.04);
            let ss_weights = TrellisWeights::load(&paths.ss_flow).map_err(trellis_err)?;
            let ss_dit = T2Dit::prepare(ss_weights, "t2ss", T2_SS_CHANNELS, T2_SS_CHANNELS)
                .map_err(trellis_err)?;
            cancel.check()?;
            progress(&format!("load ss-dec 3/{loads}"), 0.05);
            let dec_weights = TrellisWeights::load(&paths.ss_dec).map_err(trellis_err)?;
            let ss_dec = T2SsDec::prepare(&dec_weights).map_err(trellis_err)?;
            cancel.check()?;
            progress(&format!("load lr-flow 4/{loads}"), 0.06);
            let lr_weights = TrellisWeights::load(&paths.lr_flow).map_err(trellis_err)?;
            let lr_dit = T2Dit::prepare(lr_weights, "t2lr", T2_SLAT_CHANNELS, T2_SLAT_CHANNELS)
                .map_err(trellis_err)?;
            cancel.check()?;
            progress(&format!("load shape-dec 5/{loads}"), 0.07);
            let shape_dec_weights = TrellisWeights::load(&paths.shape_dec).map_err(trellis_err)?;
            let shape_dec =
                T2SparseDec::prepare(shape_dec_weights, "t2sdec", 7, true).map_err(trellis_err)?;
            cancel.check()?;
            progress(&format!("load hr-flow 6/{loads}"), 0.08);
            let hr_weights = TrellisWeights::load(&paths.hr_flow).map_err(trellis_err)?;
            let hr_dit = T2Dit::prepare(hr_weights, "t2hr", T2_SLAT_CHANNELS, T2_SLAT_CHANNELS)
                .map_err(trellis_err)?;
            cancel.check()?;
            // Tex flow + decoder only when the job wants texture: the extra
            // ~3.5GB of weights never streams for untextured jobs.
            let tex = if job.texture {
                progress(&format!("load tex-flow 7/{loads}"), 0.085);
                let tex_weights = TrellisWeights::load(&paths.tex_flow).map_err(trellis_err)?;
                let tex_dit = T2Dit::prepare(
                    tex_weights,
                    "t2tex",
                    T2_TEX_IN_CHANNELS,
                    T2_SLAT_CHANNELS,
                )
                .map_err(trellis_err)?;
                cancel.check()?;
                progress(&format!("load tex-dec 8/{loads}"), 0.09);
                let tex_dec_weights =
                    TrellisWeights::load(&paths.tex_dec).map_err(trellis_err)?;
                let tex_dec = T2SparseDec::prepare(tex_dec_weights, "t2tdec", 6, false)
                    .map_err(trellis_err)?;
                cancel.check()?;
                Some((tex_dit, tex_dec))
            } else {
                None
            };

            let mut rng = H3NoiseRng::new(job.seed);
            let mut draw = |count: usize| -> Vec<f32> {
                (0..count).map(|_| rng.next_normal()).collect()
            };

            // DINOv3 cond at 512 + 1024, positive + zero negative. Reaching
            // this point guarantees either input alpha or a native matte;
            // opaque/unsegmented fallback is intentionally impossible.
            progress("cond 1/2", 0.095);
            let input_512 = t2_cond_input(&pre, 512).map_err(trellis_err)?;
            let cond_512_host = dino.forward_rgb(&input_512, 512).map_err(trellis_err)?;
            cancel.check()?;
            progress("cond 2/2", 0.10);
            let input_1024 = t2_cond_input(&pre, 1024).map_err(trellis_err)?;
            let cond_1024_host = dino.forward_rgb(&input_1024, 1024).map_err(trellis_err)?;
            cancel.check()?;
            let cond_512 = t2_upload_cond(&cond_512_host).map_err(trellis_err)?;
            let neg_512 = t2_upload_cond(&vec![0.0; cond_512_host.len()]).map_err(trellis_err)?;
            let cond_1024 = t2_upload_cond(&cond_1024_host).map_err(trellis_err)?;
            let neg_1024 = t2_upload_cond(&vec![0.0; cond_1024_host.len()]).map_err(trellis_err)?;

            // Sparse structure stage (22 forwards + conv3d decode).
            progress("ss 0/22", 0.1);
            let noise_chw = draw(T2_SS_TOKENS * T2_SS_CHANNELS);
            let noise_tokens = t2_chw_to_tokens(&noise_chw, T2_SS_CHANNELS, T2_SS_TOKENS);
            let ss = t2_run_ss_cancel(
                &ss_dit,
                &ss_dec,
                &noise_tokens,
                &cond_512,
                &neg_512,
                &T2_SS_SAMPLER,
                32,
                &cancelled,
                |fwd, _, _, _| {
                    let done = (fwd + 1).min(22);
                    progress(
                        &format!("ss {done}/22"),
                        0.10 + 0.08 * (done as f64 / 22.0),
                    )
                },
            )
            .map_err(diffusion_err)?;
            if ss.coords.is_empty() {
                return Err(AssetAiError::Backend(
                    "trellis: ss stage produced no active voxels".to_string(),
                ));
            }

            let denorm = |latent: &mut [f32]| {
                for row in latent.chunks_exact_mut(T2_SLAT_CHANNELS) {
                    for (value, (mean, std)) in row
                        .iter_mut()
                        .zip(T2_SHAPE_SLAT_MEAN.iter().zip(T2_SHAPE_SLAT_STD.iter()))
                    {
                        *value = *value * std + mean;
                    }
                }
            };

            // LR shape flow at the SS coords.
            progress("shape_lr 0/21", 0.18);
            let lr_rope = t2_upload_rope(&t2_rope_tables(&ss.coords)).map_err(trellis_err)?;
            let mut lr = draw(ss.coords.len() * T2_SLAT_CHANNELS);
            t2_sample_flow_cancel(
                &lr_dit,
                &mut lr,
                ss.coords.len(),
                &cond_512,
                &neg_512,
                &lr_rope,
                &T2_SHAPE_SAMPLER,
                false,
                &cancelled,
                |fwd, _, _, _| {
                    let done = (fwd + 1).min(21);
                    progress(
                        &format!("shape_lr {done}/21"),
                        0.18 + 0.10 * (done as f64 / 21.0),
                    )
                },
            )
            .map_err(diffusion_err)?;
            denorm(&mut lr);

            // Cascade upsample -> HR token coords.
            cancel.check()?;
            progress("upsample", 0.29);
            let hr_grid_coords = shape_dec
                .upsample(&lr, ss.coords.clone(), 4)
                .map_err(diffusion_err)?;
            let hr_coords = t2_quantize_unique_coords(&hr_grid_coords, 512, 1024);
            if hr_coords.is_empty() {
                return Err(AssetAiError::Backend(
                    "trellis: cascade upsample produced no tokens".to_string(),
                ));
            }

            // HR shape flow at 1024.
            cancel.check()?;
            progress("shape_hr 0/21", 0.30);
            let hr_rope = t2_upload_rope(&t2_rope_tables(&hr_coords)).map_err(trellis_err)?;
            let mut hr = draw(hr_coords.len() * T2_SLAT_CHANNELS);
            t2_sample_flow_cancel(
                &hr_dit,
                &mut hr,
                hr_coords.len(),
                &cond_1024,
                &neg_1024,
                &hr_rope,
                &T2_SHAPE_SAMPLER,
                false,
                &cancelled,
                |fwd, _, _, _| {
                    let done = (fwd + 1).min(21);
                    progress(
                        &format!("shape_hr {done}/21"),
                        0.30 + 0.22 * (done as f64 / 21.0),
                    )
                },
            )
            .map_err(diffusion_err)?;
            // The tex flow conditions on the NORMALIZED shape slat (the
            // reference renormalizes with the shape stats — identical to the
            // pre-denorm samples). Snapshot before denorm.
            let hr_normalized = job.texture.then(|| hr.clone());
            denorm(&mut hr);

            // Sparse FDG decode. The decode phase rotates GB-class voxel
            // planes: a LOW pool cap wins here (shrink-on-lower keeps the
            // flow stages' small pooled buffers, decode runs far from the
            // VRAM ceiling) — same dance as trellis-generate. The cap is
            // restored on EVERY exit path (incl. cancel) before the error
            // maps out.
            cancel.check()?;
            progress("decode 0/5", 0.52);
            gpu_pool_cap_override(Some(3072 * 1024 * 1024));
            let decode_result = shape_dec.decode_ctl(
                &hr,
                hr_coords.clone(),
                None,
                &cancelled,
                &mut |stage, total| {
                    progress(
                        &format!("decode {}/{total}", stage + 1),
                        0.52 + 0.10 * (stage as f64 / total as f64),
                    )
                },
            );
            gpu_pool_cap_override(None);
            let (feats, voxel_coords, subs) = decode_result.map_err(diffusion_err)?;

            // Tex SLAT flow + decode: per-voxel PBR attrs (base RGB,
            // metallic, roughness, alpha — 6ch in 0..=1) at EXACTLY the
            // shape decode's voxel coords (the tex decoder replays the shape
            // subdivision masks instead of predicting its own).
            let voxel_pbr: Option<Vec<f32>> = match (&tex, hr_normalized) {
                (Some((tex_dit, tex_dec)), Some(concat_cond)) => {
                    cancel.check()?;
                    progress("tex 0/12", 0.63);
                    let mut x = draw(hr_coords.len() * T2_SLAT_CHANNELS);
                    t2_sample_flow_concat_ctl(
                        tex_dit,
                        &mut x,
                        hr_coords.len(),
                        Some(&concat_cond),
                        &cond_1024,
                        &neg_1024, // strength 1.0: never used
                        &hr_rope,
                        &T2_TEX_SAMPLER,
                        false,
                        &cancelled,
                        |fwd, _, _, _| {
                            let done = (fwd + 1).min(12);
                            progress(
                                &format!("tex {done}/12"),
                                0.63 + 0.12 * (done as f64 / 12.0),
                            )
                        },
                    )
                    .map_err(diffusion_err)?;
                    for row in x.chunks_exact_mut(T2_SLAT_CHANNELS) {
                        for (value, (mean, std)) in row
                            .iter_mut()
                            .zip(T2_TEX_SLAT_MEAN.iter().zip(T2_TEX_SLAT_STD.iter()))
                        {
                            *value = *value * std + mean;
                        }
                    }
                    cancel.check()?;
                    progress("tex decode 0/5", 0.75);
                    gpu_pool_cap_override(Some(3072 * 1024 * 1024));
                    let tex_result = tex_dec.decode_ctl(
                        &x,
                        hr_coords,
                        Some(&subs),
                        &cancelled,
                        &mut |stage, total| {
                            progress(
                                &format!("tex decode {}/{total}", stage + 1),
                                0.75 + 0.10 * (stage as f64 / total as f64),
                            )
                        },
                    );
                    gpu_pool_cap_override(None);
                    let (mut pbr, tex_coords, _) = tex_result.map_err(diffusion_err)?;
                    for value in &mut pbr {
                        *value = *value * 0.5 + 0.5;
                    }
                    if tex_coords.len() == voxel_coords.len() {
                        Some(pbr)
                    } else {
                        // Guided decode must reproduce the shape coords;
                        // degrade to untextured rather than mis-pair rows.
                        eprintln!(
                            "trellis tex decode voxel mismatch ({} vs {}) - untextured",
                            tex_coords.len(),
                            voxel_coords.len()
                        );
                        None
                    }
                }
                _ => None,
            };

            // Dual-grid mesh (one vertex per voxel — colors pair by row).
            cancel.check()?;
            progress("mesh", 0.87);
            let fields = t2_fdg_fields(&feats).map_err(trellis_err)?;
            let mesh = t2_dual_grid_to_mesh(&voxel_coords, &fields, 1024).map_err(trellis_err)?;
            // MeshJob None is the explicit raw escape hatch. Avoid building
            // the enormous raw GLB at all for normal jobs; the old path made
            // a ~100 MiB intermediate only to parse it back into FaithC.
            let Some(remesh_resolution) = job.remesh_resolution else {
                let voxel_rgb: Option<Vec<[f32; 3]>> = voxel_pbr.as_ref().map(|pbr| {
                    pbr.chunks_exact(6)
                        .map(|row| [row[0], row[1], row[2]])
                        .collect()
                });
                return Ok(t2_mesh_to_glb_colored(&mesh, voxel_rgb.as_deref()));
            };

            cancel.check()?;
            let mut coarse = CoarseProgress::new(progress);
            coarse.emit("weld surface", 0.88);
            // Keep this cleaned decoded surface alive through remesh AND
            // baking. Its BVH drives the UDF and snaps every atlas texel
            // after simplification back to the original attribute surface.
            let mut surface_positions = mesh.vertices;
            let mut surface_indices = Vec::with_capacity(mesh.faces.len() * 3);
            for face in mesh.faces {
                surface_indices.extend_from_slice(&face);
            }
            makepad_remesh::weld_vertices_ctl(
                &mut surface_positions,
                &mut surface_indices,
                1.0 / 8192.0,
                &mut |done, total| {
                    let frac = done as f64 / total.max(1) as f64;
                    coarse.emit("weld surface", 0.88 + 0.004 * frac.clamp(0.0, 1.0));
                    !cancel.is_cancelled()
                },
            );
            cancel.check()?;
            coarse.emit("fill holes", 0.884);
            makepad_remesh::fill_small_holes_ctl(&mut surface_indices, 64, &mut |done, total| {
                let frac = done as f64 / total.max(1) as f64;
                coarse.emit("fill holes", 0.884 + 0.006 * frac.clamp(0.0, 1.0));
                !cancel.is_cancelled()
            });
            cancel.check()?;
            coarse.emit("build BVH", 0.89);
            let surface_bvh = SurfaceBvh::build_ctl(
                &surface_positions,
                &surface_indices,
                &mut |done, total| {
                    let frac = done as f64 / total.max(1) as f64;
                    coarse.emit("build BVH", 0.89 + 0.008 * frac.clamp(0.0, 1.0));
                    !cancel.is_cancelled()
                },
            )
            .map_err(trellis_err)?;

            cancel.check()?;
            coarse.emit("remesh voxelize", 0.90);
            let remeshed = remesh_narrow_band_dc_ctl(
                &surface_positions,
                &surface_indices,
                &surface_bvh,
                remesh_resolution.clamp(16, 512) as usize,
                1,
                // Production callers (including VisualBruno's Ovoxel
                // exporter) explicitly use project=0. Projecting the UDF's
                // inner and outer shells back onto a noisy decoded surface
                // creates near-coincident geometry and dark interference.
                0.0,
                &mut |stage, frac| {
                    coarse.emit(
                        &format!("remesh {stage}"),
                        0.90 + 0.03 * frac.clamp(0.0, 1.0),
                    );
                    !cancel.is_cancelled()
                },
            )
            .map_err(trellis_err)?;
            let (mut positions, mut indices) = (remeshed.positions, remeshed.indices);
            cancel.check()?;
            makepad_remesh::weld_vertices_ctl(&mut positions, &mut indices, 1.0 / 8192.0, &mut |done, total| {
                let frac = done as f64 / total.max(1) as f64;
                coarse.emit("weld remesh", 0.930 + 0.002 * frac.clamp(0.0, 1.0));
                !cancel.is_cancelled()
            });
            cancel.check()?;
            makepad_remesh::fill_small_holes_ctl(&mut indices, 64, &mut |done, total| {
                let frac = done as f64 / total.max(1) as f64;
                coarse.emit("fill holes (remesh)", 0.932 + 0.003 * frac.clamp(0.0, 1.0));
                !cancel.is_cancelled()
            });
            cancel.check()?;
            makepad_remesh::drop_small_components_ctl(&mut positions, &mut indices, 0.02, &mut |done, total| {
                let frac = done as f64 / total.max(1) as f64;
                coarse.emit("drop islands", 0.935 + 0.002 * frac.clamp(0.0, 1.0));
                !cancel.is_cancelled()
            });

            // All remeshed outputs honor the requested game-density target,
            // including untextured jobs. Raw high-density output remains
            // available through remesh_resolution=0.
            cancel.check()?;
            let start_faces = (indices.len() / 3).max(1);
            let decimated = makepad_remesh::decimate_qem_ctl(
                &positions,
                &indices,
                job.decimation_target,
                &mut |_round, faces| {
                    let span = start_faces.saturating_sub(job.decimation_target).max(1);
                    let done = start_faces.saturating_sub(faces) as f64 / span as f64;
                    coarse.emit(
                        &format!("decimate {}k", faces / 1000),
                        0.937 + 0.023 * done.clamp(0.0, 1.0),
                    );
                    !cancel.is_cancelled()
                },
            );
            let Some((mut dp, mut di)) = decimated else {
                return Err(AssetAiError::Cancelled);
            };
            cancel.check()?;
            makepad_remesh::weld_vertices_ctl(&mut dp, &mut di, 1.0 / 8192.0, &mut |done, total| {
                let frac = done as f64 / total.max(1) as f64;
                coarse.emit("weld final", 0.960 + 0.001 * frac.clamp(0.0, 1.0));
                !cancel.is_cancelled()
            });
            makepad_remesh::fill_small_holes_ctl(&mut di, 64, &mut |done, total| {
                let frac = done as f64 / total.max(1) as f64;
                coarse.emit("fill holes (final)", 0.961 + 0.002 * frac.clamp(0.0, 1.0));
                !cancel.is_cancelled()
            });
            makepad_remesh::drop_small_components_ctl(&mut dp, &mut di, 0.03, &mut |done, total| {
                let frac = done as f64 / total.max(1) as f64;
                coarse.emit("drop islands (final)", 0.963 + 0.001 * frac.clamp(0.0, 1.0));
                !cancel.is_cancelled()
            });
            cancel.check()?;
            // The geometry heuristics (planar sheets, boundary occupancy,
            // floor components) stay as a logged AUDIT, not a gate: the user
            // gets whatever TRELLIS reconstructed. Rejecting + reseeding
            // never produced a better mesh in practice, and busts / figurines
            // / anything with a flat cut were refused outright.
            coarse.emit("quality audit", 0.964);
            if let Err(detail) = super::check_trellis_mesh_quality(&dp, &di) {
                eprintln!("trellis geometry audit (advisory): {detail}");
            }
            coarse.emit("orient faces", 0.965);
            let before_orient = makepad_remesh::audit_mesh_topology(&dp, &di);
            let reoriented = makepad_remesh::unify_face_orientations(&dp, &mut di);
            let after_orient = makepad_remesh::audit_mesh_topology(&dp, &di);
            eprintln!(
                "trellis topology: F={} boundary={} nonmanifold={} inconsistent {} -> {}; reoriented={}",
                after_orient.faces,
                after_orient.boundary_edges,
                after_orient.nonmanifold_edges,
                before_orient.inconsistent_edges,
                after_orient.inconsistent_edges,
                reoriented,
            );

            // Keep the exact xatlas input on disk: the unwrap is the one
            // stage that has hung in production, and the mesh is otherwise
            // unrecoverable (it only exists in this stack frame).
            dump_pre_unwrap_mesh(&paths.scratch, &dp, &di);
            // xatlas on a decimation-mangled mesh can take arbitrarily long
            // (chart merge is quadratic in chart count). Past this budget the
            // unwrap is abandoned and the projection fallbacks bake instead,
            // so the user always gets a textured mesh.
            let unwrap_started = Instant::now();
            let unwrap_budget = Duration::from_secs(
                std::env::var("MAKEPAD_TRELLIS_UNWRAP_BUDGET_S")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(150),
            );

            let glb = match &voxel_pbr {
                Some(pbr) => {
                    cancel.check()?;
                    coarse.emit("bake: sampler", 0.966);
                    let sampler = T2VoxelSampler::new(&voxel_coords, pbr, 6, 1024)
                        .map_err(trellis_err)?;
                    let sample = |p: [f32; 3]| -> Option<[f32; 6]> {
                        // Reference contract: simplified/remeshed texels are
                        // first mapped to the closest point on the ORIGINAL
                        // decoded surface, then sampled from the attr volume.
                        let source = surface_bvh
                            .closest(p, 2.0)
                            .map(|hit| hit.point)
                            .unwrap_or(p);
                        let mut out = [0f32; 6];
                        sampler.sample_into(source, 8, &mut out).then_some(out)
                    };
                    // Hunyuan-Paint's official unwrap is xatlas.parametrize.
                    // Chart/box projection stay as fallbacks if xatlas fails.
                    // xatlas unwrap 0.967..0.973 (chart groups), texel bake
                    // 0.973..0.98 (per triangle).
                    let mut baked = makepad_remesh::uv_xatlas_bake_ctl(
                        &dp,
                        &di,
                        job.texture_size,
                        &sample,
                        &mut |stage, frac| {
                            let frac = frac.clamp(0.0, 1.0);
                            let (label, lo, span) = match stage {
                                "unwrap" => ("unwrap (xatlas)", 0.967, 0.006),
                                _ => ("bake texels", 0.973, 0.007),
                            };
                            coarse.emit(label, lo + span * frac);
                            !cancel.is_cancelled()
                                && (stage != "unwrap" || unwrap_started.elapsed() < unwrap_budget)
                        },
                    );
                    if cancel.is_cancelled() {
                        return Err(AssetAiError::Cancelled);
                    }
                    if !baked.ok() {
                        eprintln!(
                            "trellis: xatlas unwrap abandoned after {:.1}s (budget {}s) - chart projection fallback",
                            unwrap_started.elapsed().as_secs_f64(),
                            unwrap_budget.as_secs()
                        );
                        coarse.emit("unwrap (chart projection)", 0.973);
                        baked = makepad_remesh::uv_chart_bake(
                            &dp,
                            &di,
                            job.texture_size,
                            &sample,
                        );
                    }
                    if !baked.ok() {
                        baked = makepad_remesh::uv_box_bake(
                            &dp,
                            &di,
                            job.texture_size,
                            &sample,
                        );
                    }
                    if baked.ok() {
                        cancel.check()?;
                        coarse.emit("encode png", 0.98);
                        let t = baked.size;
                        let base_png =
                            crate::testpattern::encode_png_rgba(&baked.base_rgba, t, t)?;
                        let mr_png =
                            crate::testpattern::encode_png_rgba(&baked.mr_rgba, t, t)?;
                        let pre_normals = makepad_gltf::compute_vertex_normals(&dp, &di);
                        let normals: Vec<[f32; 3]> = baked
                            .source_vertex
                            .iter()
                            .map(|&v| t2_yup(pre_normals[v as usize]))
                            .collect();
                        let exported_positions: Vec<[f32; 3]> =
                            baked.positions.iter().copied().map(t2_yup).collect();
                        makepad_gltf::write_glb_mesh_textured(
                            &makepad_gltf::GlbTexturedMesh {
                                positions: &exported_positions,
                                normals: Some(&normals),
                                uvs: &baked.uvs,
                                indices: &baked.indices,
                                base_color_png: &base_png,
                                metallic_roughness_png: Some(&mr_png),
                                double_sided: false,
                                colors: None,
                            },
                        )
                    } else {
                        eprintln!("trellis bake produced no charts - vertex colors");
                        let mut colors = Vec::with_capacity(dp.len());
                        for &p in &dp {
                            let source = surface_bvh
                                .closest(p, 2.0)
                                .map(|hit| hit.point)
                                .unwrap_or(p);
                            let mut out = [0.5f32; 6];
                            sampler.sample_into(source, 8, &mut out);
                            colors.push([out[0], out[1], out[2]]);
                        }
                        let exported_positions: Vec<[f32; 3]> =
                            dp.iter().copied().map(t2_yup).collect();
                        makepad_gltf::write_glb_mesh_colored(
                            &exported_positions,
                            &di,
                            Some(&colors),
                        )
                    }
                }
                None => {
                    // Geometry-only still carries Hunyuan-ready UV0 so a
                    // later paint stage can retexture without a second remesh.
                    coarse.emit("unwrap (xatlas)", 0.967);
                    match makepad_remesh::uv_xatlas_unwrap_ctl(&dp, &di, &mut |frac| {
                        coarse.emit("unwrap (xatlas)", 0.967 + 0.012 * frac.clamp(0.0, 1.0));
                        !cancel.is_cancelled() && unwrap_started.elapsed() < unwrap_budget
                    }) {
                        Ok((pos, uvs, idx, src)) => {
                            let pre_normals = makepad_gltf::compute_vertex_normals(&dp, &di);
                            let normals: Vec<[f32; 3]> = src
                                .iter()
                                .map(|&v| t2_yup(pre_normals[v as usize]))
                                .collect();
                            let exported_positions: Vec<[f32; 3]> =
                                pos.iter().copied().map(t2_yup).collect();
                            makepad_gltf::write_glb_mesh_unwrapped(
                                &exported_positions,
                                Some(&normals),
                                &uvs,
                                &idx,
                            )
                        }
                        Err(_) => {
                            cancel.check()?;
                            eprintln!(
                                "trellis: xatlas unwrap abandoned after {:.1}s (budget {}s) - exporting without UVs",
                                unwrap_started.elapsed().as_secs_f64(),
                                unwrap_budget.as_secs()
                            );
                            let exported_positions: Vec<[f32; 3]> =
                                dp.iter().copied().map(t2_yup).collect();
                            makepad_gltf::write_glb_mesh(&exported_positions, &di)
                        }
                    }
                }
            };
            Ok(glb)
        }
    }

    /// Best effort: `<scratch>/pre_unwrap.glb` (positions + indices in the
    /// TRELLIS frame). Failure only logs — this must never fail a job.
    fn dump_pre_unwrap_mesh(scratch: &std::path::Path, positions: &[[f32; 3]], indices: &[u32]) {
        let path = scratch.join("pre_unwrap.glb");
        let result = std::fs::create_dir_all(scratch)
            .and_then(|_| std::fs::write(&path, makepad_gltf::write_glb_mesh(positions, indices)));
        match result {
            Ok(()) => eprintln!(
                "trellis: pre-unwrap mesh saved to {} (V={} F={})",
                path.display(),
                positions.len(),
                indices.len() / 3
            ),
            Err(err) => eprintln!("trellis: pre-unwrap mesh dump failed: {err}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests (stubbed generation — this is what CI exercises)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::GenerateRequestJson;
    use crate::testpattern::encode_png_rgba;

    fn mesh_params(request: GenerateRequestJson) -> GenerateParams {
        GenerateParams::from_request(&request).unwrap()
    }

    fn b64(bytes: &[u8]) -> String {
        String::from_utf8(makepad_base64::base64_encode(
            bytes,
            &makepad_base64::BASE64_STANDARD,
        ))
        .unwrap()
    }

    fn tiny_png() -> Vec<u8> {
        // 8x4 opaque gradient.
        let (w, h) = (8usize, 4usize);
        let mut rgba = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(&[(x * 16) as u8, (y * 16) as u8, 128, 255]);
            }
        }
        encode_png_rgba(&rgba, w, h).unwrap()
    }

    #[test]
    fn stub_mesh_job_to_glb_artifact() {
        let mut backend = TrellisBackend::with_stub(
            "trellis-2",
            Box::new(|job: &MeshJob, progress: ProgressSink| {
                assert_eq!(job.width, 8);
                assert_eq!(job.height, 4);
                assert_eq!(job.rgba.len(), 8 * 4 * 4);
                assert_eq!(job.rgba[3], 255);
                assert_eq!(job.seed, 42);
                // 9999 clamped into the FaithC grid range.
                assert_eq!(job.remesh_resolution, Some(512));
                // Stubs observe the opaque input before the real generator's
                // native matte stage.
                assert!(!job.segmented);
                // Texture defaults ON when the request says nothing.
                assert!(job.texture);
                progress("ss", 0.2);
                Ok(b"GLBSTUB".to_vec())
            }),
        );
        let png = tiny_png();
        let params = mesh_params(GenerateRequestJson {
            model: "trellis-2".to_string(),
            seed: Some(42),
            input_b64: Some(b64(&png)),
            remesh_resolution: Some(9999),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        let artifacts = backend.generate(&params, &mut sink, &CancelToken::new()).unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].content_type, "model/gltf-binary");
        assert_eq!(artifacts[0].ext, "glb");
        assert_eq!(artifacts[0].bytes, b"GLBSTUB");
    }

    #[test]
    fn remesh_defaults_to_256_and_zero_means_raw() {
        // Request without remesh_resolution -> the service default.
        let mut backend = TrellisBackend::with_stub(
            "trellis-2",
            Box::new(|job: &MeshJob, _p: ProgressSink| {
                assert_eq!(job.remesh_resolution, Some(super::REMESH_DEFAULT));
                Ok(b"G".to_vec())
            }),
        );
        let png = tiny_png();
        let params = mesh_params(GenerateRequestJson {
            model: "trellis-2".to_string(),
            input_b64: Some(b64(&png)),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        backend.generate(&params, &mut sink, &CancelToken::new()).unwrap();

        // remesh_resolution 0 = the raw-mesh escape hatch (None in the job).
        // Built directly on GenerateParams: the wire clamp in from_request
        // must also preserve 0 (see resolve_remesh) for this to reach us.
        let mut raw_params = mesh_params(GenerateRequestJson {
            model: "trellis-2".to_string(),
            input_b64: Some(b64(&png)),
            ..GenerateRequestJson::default()
        });
        raw_params.remesh_resolution = Some(0);
        let mut backend = TrellisBackend::with_stub(
            "trellis-2",
            Box::new(|job: &MeshJob, _p: ProgressSink| {
                assert_eq!(job.remesh_resolution, None);
                Ok(b"G".to_vec())
            }),
        );
        backend.generate(&raw_params, &mut sink, &CancelToken::new()).unwrap();
    }

    #[test]
    fn pre_raised_cancel_token_short_circuits() {
        let mut backend = TrellisBackend::with_stub(
            "trellis-2",
            Box::new(|_: &MeshJob, _p: ProgressSink| {
                panic!("generator must not run on a cancelled job")
            }),
        );
        let png = tiny_png();
        let params = mesh_params(GenerateRequestJson {
            model: "trellis-2".to_string(),
            input_b64: Some(b64(&png)),
            ..GenerateRequestJson::default()
        });
        let token = CancelToken::new();
        token.cancel();
        let mut sink = |_: &str, _: f64| {};
        assert!(matches!(
            backend.generate(&params, &mut sink, &token),
            Err(AssetAiError::Cancelled)
        ));
    }

    #[test]
    fn resolve_remesh_semantics() {
        assert_eq!(resolve_remesh(None), Some(256));
        assert_eq!(resolve_remesh(Some(0)), None);
        assert_eq!(resolve_remesh(Some(4)), Some(16));
        assert_eq!(resolve_remesh(Some(256)), Some(256));
        assert_eq!(resolve_remesh(Some(9999)), Some(512));
    }

    #[test]
    fn alpha_segmentation_detection() {
        // Fully opaque -> scene photo, requires native matting.
        let opaque = vec![255u8; 64 * 4];
        assert!(!alpha_is_segmented(&opaque));
        // A real matte (half the pixels transparent) -> pre-segmented.
        let mut matte = vec![255u8; 64 * 4];
        for px in 0..32 {
            matte[px * 4 + 3] = 0;
        }
        assert!(alpha_is_segmented(&matte));
        // Stray noise (one dodgy pixel in 6400) is NOT a matte.
        let mut noise = vec![255u8; 6400 * 4];
        noise[3] = 200;
        assert!(!alpha_is_segmented(&noise));
    }

    fn append_grid(
        positions: &mut Vec<[f32; 3]>,
        indices: &mut Vec<u32>,
        x: (f32, f32),
        z: (f32, f32),
        y: f32,
        cells: usize,
    ) {
        let base = positions.len() as u32;
        for iz in 0..=cells {
            let tz = iz as f32 / cells as f32;
            for ix in 0..=cells {
                let tx = ix as f32 / cells as f32;
                positions.push([
                    x.0 + (x.1 - x.0) * tx,
                    y,
                    z.0 + (z.1 - z.0) * tz,
                ]);
            }
        }
        let row = cells + 1;
        for iz in 0..cells {
            for ix in 0..cells {
                let a = base + (iz * row + ix) as u32;
                let b = a + 1;
                let c = a + row as u32;
                let d = c + 1;
                indices.extend_from_slice(&[a, c, b, b, c, d]);
            }
        }
    }

    fn append_tetrahedron(
        positions: &mut Vec<[f32; 3]>,
        indices: &mut Vec<u32>,
        center: [f32; 3],
        radius: f32,
    ) {
        let base = positions.len() as u32;
        positions.extend_from_slice(&[
            [center[0] - radius, center[1] - radius, center[2] - radius],
            [center[0] + radius, center[1] - radius, center[2] + radius],
            [center[0] - radius, center[1] + radius, center[2] + radius],
            [center[0] + radius, center[1] + radius, center[2] - radius],
        ]);
        indices.extend_from_slice(&[
            base,
            base + 1,
            base + 2,
            base,
            base + 3,
            base + 1,
            base,
            base + 2,
            base + 3,
            base + 1,
            base + 3,
            base + 2,
        ]);
    }

    #[test]
    fn mesh_quality_accepts_normal_closed_mesh() {
        let positions = vec![
            [-1.0, -1.0, -1.0],
            [1.0, -1.0, -1.0],
            [1.0, 1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [-1.0, -1.0, 1.0],
            [1.0, -1.0, 1.0],
            [1.0, 1.0, 1.0],
            [-1.0, 1.0, 1.0],
        ];
        let indices = vec![
            0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 3, 7, 6, 3,
            6, 2, 0, 4, 7, 0, 7, 3, 1, 2, 6, 1, 6, 5,
        ];
        check_trellis_mesh_quality(&positions, &indices).unwrap();
    }

    #[test]
    fn mesh_quality_rejects_dominant_boundary_floor_sheet() {
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        append_grid(
            &mut positions,
            &mut indices,
            (-1.0, 1.0),
            (-1.0, 1.0),
            -1.0,
            20,
        );
        append_tetrahedron(&mut positions, &mut indices, [0.0, 0.0, 0.0], 0.5);
        let error = check_trellis_mesh_quality(&positions, &indices).unwrap_err();
        assert!(error.contains("dominant reconstruction-boundary sheet"), "{error}");
        assert!(error.contains("faces=800/804"), "{error}");
        assert!(error.contains("projected_span=(100.0%, 100.0%)"), "{error}");
        assert!(error.contains("mesh_bounds="), "{error}");
    }

    #[test]
    fn mesh_quality_rejects_excessive_boundary_occupancy_across_fragments() {
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        for &(x, z) in &[
            ((-1.0, -0.05), (-1.0, -0.05)),
            ((0.05, 1.0), (-1.0, -0.05)),
            ((-1.0, -0.05), (0.05, 1.0)),
            ((0.05, 1.0), (0.05, 1.0)),
        ] {
            append_grid(&mut positions, &mut indices, x, z, -1.0, 10);
        }
        // Keep every sheet fragment below the 10% dominant-component gate;
        // their aggregate still forms an unmistakable reconstruction floor.
        for _ in 0..400 {
            append_tetrahedron(&mut positions, &mut indices, [0.0, 0.0, 0.0], 0.5);
        }
        let error = check_trellis_mesh_quality(&positions, &indices).unwrap_err();
        assert!(
            error.contains("excessive reconstruction-boundary occupancy"),
            "{error}"
        );
        assert!(error.contains("faces=800/2400"), "{error}");
        assert!(error.contains("projected_span=(100.0%, 100.0%)"), "{error}");
    }

    #[test]
    fn mesh_quality_rejects_aggregate_near_coplanar_interior_sheets() {
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        append_grid(
            &mut positions,
            &mut indices,
            (-1.0, 1.0),
            (-1.0, 1.0),
            -0.20,
            10,
        );
        append_grid(
            &mut positions,
            &mut indices,
            (-1.0, 1.0),
            (-1.0, 1.0),
            -0.19,
            10,
        );
        // Establish a much taller subject volume and keep each sheet away
        // from both bounds. Neither component needs the old >=10% boundary
        // test; the near-coplanar aggregate is what makes this invalid.
        for _ in 0..300 {
            append_tetrahedron(&mut positions, &mut indices, [0.0, 0.0, 0.0], 0.8);
        }
        let error = check_trellis_mesh_quality(&positions, &indices).unwrap_err();
        assert!(
            error.contains("aggregate near-coplanar reconstruction sheet"),
            "{error}"
        );
        assert!(error.contains("axis=y"), "{error}");
        assert!(error.contains("components=2"), "{error}");
        assert!(error.contains("faces=400/1600"), "{error}");
        assert!(error.contains("projected_span=(100.0%, 100.0%)"), "{error}");
    }

    #[test]
    fn mesh_quality_rejects_planar_band_welded_into_tall_subject() {
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        append_grid(
            &mut positions,
            &mut indices,
            (-1.0, 1.0),
            (-1.0, 1.0),
            -0.20,
            10,
        );

        // Join the sheet's first vertex to geometry extending well above it.
        // Component-based thinness therefore cannot identify this as a sheet,
        // matching the live failure where the ring was welded into both shins.
        let a = positions.len() as u32;
        positions.extend_from_slice(&[
            [-0.9, 0.8, -0.9],
            [-0.8, 0.7, -0.9],
            [-0.9, 0.7, -0.8],
        ]);
        indices.extend_from_slice(&[0, a, a + 1, 0, a + 2, a, 0, a + 1, a + 2, a, a + 2, a + 1]);
        for _ in 0..300 {
            append_tetrahedron(&mut positions, &mut indices, [0.0, 0.0, 0.0], 0.8);
        }

        let error = check_trellis_mesh_quality(&positions, &indices).unwrap_err();
        assert!(error.contains("excessive axis-aligned planar band"), "{error}");
        assert!(error.contains("axis=y"), "{error}");
        assert!(error.contains("projected_span=(100.0%, 100.0%)"), "{error}");
    }

    #[test]
    fn mesh_quality_rejects_severe_fragmentation() {
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        for index in 0..1100 {
            let x = (index % 11) as f32 * 0.01;
            let z = ((index / 11) % 10) as f32 * 0.01;
            append_tetrahedron(&mut positions, &mut indices, [x, 0.0, z], 0.5);
        }
        let error = check_trellis_mesh_quality(&positions, &indices).unwrap_err();
        assert!(error.contains("severe generated-mesh fragmentation"), "{error}");
        assert!(error.contains("components=1100"), "{error}");
        assert!(error.contains("faces=4400"), "{error}");
        assert!(error.contains("tiny_components(<= 4 faces)=1100/1100"), "{error}");
    }

    #[cfg(feature = "mesh")]
    #[test]
    fn saved_yoshi_clean_and_floor_regression_match_quality_gate_when_present() {
        let library = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../local/ai_content_library");
        let verify = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../local/character_verify");
        let accepted = [
            library.join("lib-8.glb"),
            library.join("lib-12.glb"),
            library.join("lib-18.glb"),
        ];
        let boundary_rejected = [library.join("lib-23.glb"), library.join("lib-29.glb")];
        let attached_sheet_rejected = [
            library.join("lib-33.glb"),
            library.join("lib-38.glb"),
            verify.join("elf_trellis_quality_gate_seed_5351965101387779242.glb"),
        ];
        if accepted
            .iter()
            .chain(boundary_rejected.iter())
            .chain(attached_sheet_rejected.iter())
            .any(|path| !path.is_file())
        {
            // Developer regression corpus: synthetic tests above are the
            // hermetic CI contract; verify the originating artifacts whenever
            // the local character library is available.
            return;
        }

        fn load_welded(path: &std::path::Path) -> (Vec<[f32; 3]>, Vec<u32>) {
            let gltf = makepad_gltf::load_gltf_from_path(path).unwrap();
            let mesh = makepad_gltf::decode_mesh_primitive(&gltf, 0, 0).unwrap();
            let (mut positions, mut indices) = (mesh.positions, mesh.indices);
            // Recreate the pre-atlas topology seen by the production gate;
            // chart splitting duplicates positions along every UV seam.
            makepad_remesh::weld_vertices(&mut positions, &mut indices, 1.0 / 8192.0);
            (positions, indices)
        }

        for path in accepted {
            let (positions, indices) = load_welded(&path);
            check_trellis_mesh_quality(&positions, &indices)
                .unwrap_or_else(|error| panic!("clean corpus {} rejected: {error}", path.display()));
        }
        for path in boundary_rejected {
            let (positions, indices) = load_welded(&path);
            let error = check_trellis_mesh_quality(&positions, &indices).unwrap_err();
            assert!(
                error.contains("reconstruction-boundary")
                    || error.contains("axis-aligned planar band"),
                "unexpected boundary regression diagnostic for {}: {error}",
                path.display(),
            );
        }
        for path in attached_sheet_rejected {
            let (positions, indices) = load_welded(&path);
            let error = check_trellis_mesh_quality(&positions, &indices).unwrap_err();
            assert!(
                error.contains("excessive axis-aligned planar band"),
                "connectivity-independent gate missed {}: {error}",
                path.display(),
            );
        }
    }

    #[test]
    fn missing_input_image_is_a_params_error() {
        let mut backend = TrellisBackend::with_stub(
            "trellis-2",
            Box::new(|_: &MeshJob, _p: ProgressSink| unreachable!()),
        );
        let params = mesh_params(GenerateRequestJson {
            model: "trellis-2".to_string(),
            prompt: Some("a crown".to_string()),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        let err = backend
            .generate(&params, &mut sink, &CancelToken::new())
            .err()
            .expect("missing input must be an error");
        match err {
            AssetAiError::Params(msg) => assert!(msg.contains("input_b64")),
            other => panic!("expected Params error, got {other:?}"),
        }
    }

    #[test]
    fn display_fraction_is_monotone_and_time_weighted() {
        let mut last = -1.0;
        for i in 0..=1000 {
            let x = i as f64 / 1000.0;
            let y = display_fraction(x);
            assert!(y >= last, "not monotone at {x}: {y} < {last}");
            assert!((0.0..=1.0).contains(&y));
            last = y;
        }
        assert_eq!(display_fraction(0.0), 0.0);
        assert_eq!(display_fraction(1.0), 1.0);
        // The unwrap band (0.967..0.973 internally) must be a real stretch
        // of the visible bar, not the 0.6% it was.
        assert!(display_fraction(0.973) - display_fraction(0.967) > 0.15);
        assert!(display_fraction(0.88) < 0.5, "forward is under half the wall time");
        assert_eq!(display_fraction(f64::NAN), 0.0);
    }

    #[test]
    fn params_and_cancel_errors_keep_resident_backend_healthy() {
        let backend = TrellisBackend::with_stub(
            "trellis-2",
            Box::new(|_: &MeshJob, _p: ProgressSink| unreachable!()),
        );
        let params = AssetAiError::Params("trellis: needs an input image".to_string());
        assert!(backend.resident_is_healthy_after_error(&params));
        assert!(backend.resident_is_healthy_after_error(&AssetAiError::Cancelled));
        assert!(!backend.resident_is_healthy_after_error(&AssetAiError::Backend(
            "trellis: CUDA kernel launch failed".to_string()
        )));
    }

    #[test]
    fn garbage_input_png_rejected() {
        let mut backend = TrellisBackend::with_stub(
            "trellis-2",
            Box::new(|_: &MeshJob, _p: ProgressSink| unreachable!()),
        );
        let params = mesh_params(GenerateRequestJson {
            model: "trellis-2".to_string(),
            input_b64: Some(b64(b"not a png at all")),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        assert!(matches!(
            backend.generate(&params, &mut sink, &CancelToken::new()),
            Err(AssetAiError::Params(_))
        ));
    }

    #[test]
    fn remesh_resolution_threading() {
        // Absent stays absent; small values clamp up to the grid floor.
        let params = mesh_params(GenerateRequestJson {
            model: "trellis-2".to_string(),
            ..GenerateRequestJson::default()
        });
        assert_eq!(params.remesh_resolution, None);
        let params = mesh_params(GenerateRequestJson {
            model: "trellis-2".to_string(),
            remesh_resolution: Some(4),
            ..GenerateRequestJson::default()
        });
        assert_eq!(params.remesh_resolution, Some(16));
    }
}
