//! makepad-remesh: CPU port of FaithC v1.5 "Faithful Contouring"
//! (github.com/Luo-Yihao/FaithC, Apache 2.0) — a classical-geometry
//! mesh <-> sparse-voxel-token codec used as a geometry-only remesher
//! (e.g. between Trellis-2 mesh decode and texture re-bake; the token form
//! is the future Trellis integration seam).
//!
//! - encoder: octree broadphase + exact SAT clip -> per-voxel QEF anchor +
//!   normal + 12 edge-flux signs (18 dims per active voxel)
//! - decoder: dual-contouring-style quad extraction from flux edges
//!
//! Faithful port: known reference limitations are preserved on purpose
//! (clamp_anchors spikes/pits, occasional small holes with clamp off,
//! one anchor per voxel — FaithC issue #3). Output geometry lives in the
//! reference's normalized [-1,1] space unless `denormalize` is set.

pub mod bake;
pub mod decoder;
pub mod encoder;
pub mod grid;
pub mod math;
pub mod mesh;
pub mod parallel;
pub mod post;
pub mod qef;
pub mod sat;
pub mod spatial;
pub mod surface;

pub use bake::{
    uv_atlas_bake, uv_box_bake, uv_chart_bake, uv_xatlas_bake, uv_xatlas_unwrap, BakedMesh,
};
pub use decoder::{decode, DecodedMesh, TriangulationMode};
pub use encoder::{encode, EncodeOptions, FctTokens};
pub use math::V3;
pub use mesh::{load_glb_normalized, Mesh, NormalizeInfo};
pub use post::{
    audit_mesh_topology, decimate_qem, decimate_qem_ctl, drop_small_components,
    fill_small_holes, unify_face_orientations, weld_vertices, MeshTopologyAudit,
};
pub use surface::{remesh_narrow_band_dc, SurfaceBvh, SurfaceHit, SurfaceMesh};

#[derive(Clone, Copy)]
pub struct RemeshOptions {
    /// clamp anchors to their voxel + reproject to the surface (demo default
    /// true; known to cause occasional spikes/pits — reference issue #3;
    /// false trades those for occasional small holes)
    pub clamp_anchors: bool,
    /// QEF distance regularization (demo/ComfyUI use 1e-3; library default
    /// was 0.1 — smaller adheres tighter to surface planes = sharper)
    pub lambda_d: f32,
    /// QEF normal constraint weight
    pub lambda_n: f32,
    /// normalization margin: input is scaled into [-(1-margin), 1-margin]^3
    pub margin: f64,
    pub tri_mode: TriangulationMode,
    /// map the output back into the input's original coordinate frame
    /// (the reference demo exports normalized coordinates; default false)
    pub denormalize: bool,
}

impl Default for RemeshOptions {
    fn default() -> Self {
        RemeshOptions {
            clamp_anchors: true,
            lambda_d: 1e-3,
            lambda_n: 1.0,
            margin: 0.05,
            tri_mode: TriangulationMode::Auto,
            denormalize: false,
        }
    }
}

/// Full remesh pipeline: GLB in -> normalize -> FCT encode at `resolution`
/// -> FCT decode -> geometry-only GLB out.
pub fn remesh_glb(bytes: &[u8], resolution: u32, opts: &RemeshOptions) -> Result<Vec<u8>, String> {
    let (positions, indices) = remesh_glb_mesh(bytes, resolution, opts)?;
    Ok(makepad_gltf::write_glb_mesh(&positions, &indices))
}

/// [`remesh_glb`] returning the mesh instead of serialized bytes, for callers
/// that attach attributes (e.g. resampled vertex colors) before writing.
pub fn remesh_glb_mesh(
    bytes: &[u8],
    resolution: u32,
    opts: &RemeshOptions,
) -> Result<(Vec<[f32; 3]>, Vec<u32>), String> {
    let (mesh, norm) = load_glb_normalized(bytes, opts.margin)?;
    let tris = mesh.tri_soup();
    let tokens = encode(
        &tris,
        resolution,
        &EncodeOptions {
            lambda_n: opts.lambda_n,
            lambda_d: opts.lambda_d,
            clamp_anchors: opts.clamp_anchors,
            compute_flux: true,
            min_level: None,
        },
    );
    let dec = decode(
        resolution,
        &tokens.voxel_indices,
        &tokens.anchors,
        &tokens.flux,
        Some(&tokens.normals),
        opts.tri_mode,
    );
    let positions: Vec<[f32; 3]> = if opts.denormalize {
        dec.vertices
            .iter()
            .map(|p| {
                [
                    (p[0] as f64 / norm.scale + norm.center[0]) as f32,
                    (p[1] as f64 / norm.scale + norm.center[1]) as f32,
                    (p[2] as f64 / norm.scale + norm.center[2]) as f32,
                ]
            })
            .collect()
    } else {
        dec.vertices
    };
    let indices: Vec<u32> = dec.faces.iter().flatten().copied().collect();
    Ok((positions, indices))
}
