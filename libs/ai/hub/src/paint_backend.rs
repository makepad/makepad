//! The generative-PBR "paint" domain backend: **existing mesh GLB + reference
//! image -> PBR-textured GLB + semantic channel maps + provenance manifest**.
//! This is retexturing, not shape generation; the image->shape->paint chain
//! composes it behind an upstream mesh backend.
//!
//! Dual-input contract (strict): the request must carry BOTH named inputs
//! `"mesh"` (model/gltf-binary) and `"reference_image"` (image/png) via the
//! `inputs` field. A missing or mismatched input is refused with an explicit
//! error — never inferred, never silently substituted from `input_b64`.
//!
//! Artifact contract (fixed order; roles also named in the manifest artifact):
//! 0 textured GLB (model/gltf-binary) — input mesh rewritten with the baked
//!   maps (baseColor + metallicRoughness pointing at the ORM image);
//! 1 albedo atlas (image/png, sRGB);
//! 2 tangent-space normal atlas (image/png, linear, +Y up, geometry-derived);
//! 3 packed ORM atlas (image/png, linear: R=occlusion — neutral 255 when AO
//!   is absent — G=roughness, B=metallic);
//! 4 material manifest (application/json): artifact roles + per-channel
//!   origins/digests + exact model/license/input provenance.
//!
//! Backends: `paint-test` is the deterministic mock executor used by crate
//! tests (no GPU). It is not a fleet-advertised model.
//! `paint` (registry `hunyuan3d-paint-2.1`) downloads the pinned unet/vae/dino
//! blobs through the fleet cache (`paint21/…`) like any other model, then
//! hands those resolved paths to the native CUDA executor. A missing CUDA
//! build still fails closed; absent weights are `absent` until the first pull.

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, NamedInput,
    ProgressSink,
};
use crate::error::AssetAiError;
use crate::registry::ModelSpec;
use std::path::PathBuf;
use crate::trellis_backend::decode_png_rgba8;
use makepad_gltf::{
    load_gltf_from_bytes, read_accessor_f32x2, read_accessor_f32x3, read_accessor_indices_u32,
    write_glb_mesh_textured, GlbTexturedMesh,
};
use makepad_ai_paint::contract::PbrMaterialSet;
use makepad_ai_paint::digest;
use makepad_ai_paint::mesh::TriMesh;
use makepad_ai_paint::pipeline::{
    HunyuanPaintPipeline, MemoryProfile, MockPaintExec, PaintConfig, PaintInputs,
};
use makepad_ai_paint::png::{encode_png, PngColor};
use makepad_ai_paint::test_backend::{PbrError, PbrProgress, PbrStage};

pub const INPUT_MESH: &str = "mesh";
pub const INPUT_REFERENCE: &str = "reference_image";
pub const MESH_CONTENT_TYPE: &str = "model/gltf-binary";
pub const REFERENCE_CONTENT_TYPE: &str = "image/png";

/// Fail-closed only when this binary was not built with the CUDA paint
/// executor. Weight files may still be absent (model state `absent` until
/// downloaded); that is not a provisioning refusal.
pub const HUNYUAN_NATIVE_BLOCKER: &str =
    "native CUDA Hunyuan Paint is not in this build \
     (need the paint-cuda cargo feature on Windows/Linux)";

/// True when this service can serve Hunyuan Paint: a CUDA `paint-cuda`
/// Windows/Linux build. Weights download like any other registry model.
/// The deterministic `paint-test` tier does not consult this.
pub fn hunyuan_native_provisioned() -> bool {
    cfg!(all(
        feature = "paint-cuda",
        any(target_os = "linux", target_os = "windows")
    ))
}

pub const ROLE_UNET: &str = "unet";
pub const ROLE_VAE: &str = "vae";
pub const ROLE_DINO: &str = "dino-conditioner";

#[cfg(all(feature = "paint-cuda", any(target_os = "linux", target_os = "windows")))]
fn hunyuan_license() -> Result<makepad_ai_paint::hunyuan::LicenseAcknowledgement, AssetAiError> {
    // Fleet default: accept the pinned Hunyuan 3D 2.1 community license.
    // Opt out with MAKEPAD_HUNYUAN_LICENSE_ACCEPT=0.
    let accept = std::env::var("MAKEPAD_HUNYUAN_LICENSE_ACCEPT")
        .map(|v| {
            let v = v.trim();
            !(v == "0" || v.eq_ignore_ascii_case("false") || v.eq_ignore_ascii_case("no"))
        })
        .unwrap_or(true);
    let digest = std::env::var("MAKEPAD_HUNYUAN_LICENSE_SHA256")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| makepad_ai_paint::hunyuan::LICENSE_TEXT_SHA256.to_string());
    makepad_ai_paint::hunyuan::acknowledge_license(accept, &digest).map_err(|e| {
        AssetAiError::Backend(format!(
            "Hunyuan license acknowledgement required ({e}). Set \
             MAKEPAD_HUNYUAN_LICENSE_ACCEPT=1 and MAKEPAD_HUNYUAN_LICENSE_SHA256 to the \
             pinned digest, or omit both to use the fleet default."
        ))
    })
}

pub struct PaintBackend {
    model_id: String,
    /// True for the deterministic `paint-test` backend string.
    deterministic: bool,
    loaded: bool,
    /// Fleet-cache files resolved by role in `ensure_loaded`. Native generate
    /// refuses to fall back to the Hunyuan git checkout overlay.
    vae_path: Option<PathBuf>,
    unet_path: Option<PathBuf>,
    dino_path: Option<PathBuf>,
}

impl PaintBackend {
    pub fn new(spec: &ModelSpec) -> Self {
        Self {
            model_id: spec.id.clone(),
            deterministic: spec.backend == "paint-test",
            loaded: false,
            vae_path: None,
            unet_path: None,
            dino_path: None,
        }
    }
}

fn named_input<'p>(
    params: &'p GenerateParams,
    name: &str,
    content_type: &str,
) -> Result<&'p NamedInput, AssetAiError> {
    let input = params.inputs.iter().find(|i| i.name == name).ok_or_else(|| {
        let got: Vec<&str> = params.inputs.iter().map(|i| i.name.as_str()).collect();
        AssetAiError::Params(format!(
            "paint requires named input {name:?} ({content_type}); request carried [{}]. \
             Both \"mesh\" and \"reference_image\" are mandatory — no fallback, no inference.",
            got.join(", ")
        ))
    })?;
    if input.content_type != content_type {
        return Err(AssetAiError::Params(format!(
            "named input {name:?} content_type must be {content_type:?}, got {:?}",
            input.content_type
        )));
    }
    Ok(input)
}

/// Strict single-mesh single-primitive GLB -> TriMesh (+ flat index list for
/// the writer). Requires POSITION and TEXCOORD_0 (the retexture atlas).
fn glb_to_tri_mesh(bytes: &[u8]) -> Result<(TriMesh, Vec<u32>), AssetAiError> {
    let loaded = load_gltf_from_bytes(bytes, None)
        .map_err(|e| AssetAiError::Params(format!("mesh input is not a parseable GLB: {e:?}")))?;
    let meshes = loaded.document.meshes.as_deref().unwrap_or(&[]);
    if meshes.len() != 1 || meshes[0].primitives.len() != 1 {
        return Err(AssetAiError::Params(format!(
            "paint requires exactly one mesh with one primitive, got {} mesh(es) / {} primitive(s)",
            meshes.len(),
            meshes.first().map(|m| m.primitives.len()).unwrap_or(0)
        )));
    }
    let prim = &meshes[0].primitives[0];
    let pos_acc = *prim
        .attributes
        .get("POSITION")
        .ok_or_else(|| AssetAiError::Params("mesh GLB has no POSITION attribute".to_string()))?;
    let idx_acc = prim
        .indices
        .ok_or_else(|| AssetAiError::Params("mesh GLB primitive has no indices".to_string()))?;
    let positions = read_accessor_f32x3(&loaded, pos_acc)
        .map_err(|e| AssetAiError::Params(format!("mesh POSITION accessor: {e:?}")))?;
    let flat = read_accessor_indices_u32(&loaded, idx_acc)
        .map_err(|e| AssetAiError::Params(format!("mesh index accessor: {e:?}")))?;
    if flat.len() % 3 != 0 || flat.is_empty() {
        return Err(AssetAiError::Params(format!(
            "mesh index count {} is not a non-empty multiple of 3",
            flat.len()
        )));
    }
    let normals = prim
        .attributes
        .get("NORMAL")
        .and_then(|acc| read_accessor_f32x3(&loaded, *acc).ok())
        .unwrap_or_default();
    let uvs = prim
        .attributes
        .get("TEXCOORD_0")
        .and_then(|acc| read_accessor_f32x2(&loaded, *acc).ok())
        .unwrap_or_default();
    let (positions, uvs, flat, normals) = if uvs.len() == positions.len() {
        (positions, uvs, flat, normals)
    } else {
        unwrap_paint_uvs(positions, flat, normals)?
    };
    let mut mesh = TriMesh {
        positions,
        normals,
        uvs,
        indices: flat.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect(),
    };
    if mesh.normals.len() != mesh.positions.len() {
        mesh.compute_vertex_normals();
    }
    Ok((mesh, flat))
}

fn unwrap_paint_uvs(
    positions: Vec<[f32; 3]>,
    flat: Vec<u32>,
    normals: Vec<[f32; 3]>,
) -> Result<(Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>, Vec<[f32; 3]>), AssetAiError> {
    #[cfg(feature = "paint")]
    {
        let (pos, uvs, idx, src) = makepad_remesh::uv_xatlas_unwrap(&positions, &flat)
            .map_err(|e| AssetAiError::Params(format!("xatlas unwrap for paint: {e}")))?;
        let nrm = if normals.len() == positions.len() {
            src.iter().map(|&v| normals[v as usize]).collect()
        } else {
            Vec::new()
        };
        Ok((pos, uvs, idx, nrm))
    }
    #[cfg(not(feature = "paint"))]
    {
        let _ = (positions, flat, normals);
        Err(AssetAiError::Params(
            "retexture requires a TEXCOORD_0 UV atlas on the input mesh".to_string(),
        ))
    }
}

/// Overall-fraction mapping for the pipeline stages (ProgressSink convention).
fn stage_fraction(progress: &PbrProgress) -> (String, f64) {
    let frac = |base: f64, span: f64, current: u32, total: u32| {
        base + span * (current as f64 / total.max(1) as f64)
    };
    match progress.stage {
        PbrStage::Prepare => ("prepare".to_string(), 0.02),
        PbrStage::ViewSelect => (
            format!("view-select {}/{}", progress.current, progress.total),
            frac(0.02, 0.08, progress.current, progress.total),
        ),
        PbrStage::GeometryRender => (
            format!("geometry {}/{}", progress.current, progress.total),
            frac(0.10, 0.08, progress.current, progress.total),
        ),
        PbrStage::Encode => ("encode".to_string(), 0.20),
        PbrStage::Denoise => (
            format!("denoise {}/{}", progress.current, progress.total),
            frac(0.20, 0.60, progress.current, progress.total),
        ),
        PbrStage::Decode => ("decode".to_string(), 0.82),
        PbrStage::Upscale => ("upscale".to_string(), 0.86),
        PbrStage::Bake => (
            format!("bake {}/{}", progress.current, progress.total),
            frac(0.86, 0.06, progress.current, progress.total),
        ),
        PbrStage::Inpaint => ("inpaint".to_string(), 0.96),
        PbrStage::Finalize => ("finalize".to_string(), 1.0),
    }
}

impl PaintBackend {
    #[cfg(all(feature = "paint-cuda", any(target_os = "linux", target_os = "windows")))]
    fn generate_native(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        use makepad_ai_paint::native_exec::{HunyuanBins, NativeHunyuanExec};
        let mesh_input = named_input(params, INPUT_MESH, MESH_CONTENT_TYPE)?;
        let reference_input = named_input(params, INPUT_REFERENCE, REFERENCE_CONTENT_TYPE)?;
        let mesh_sha = digest::sha256_hex(&mesh_input.bytes);
        let reference_sha = digest::sha256_hex(&reference_input.bytes);
        let (mesh, flat_indices) = glb_to_tri_mesh(&mesh_input.bytes)?;
        let (rgba, ref_w, ref_h) = decode_png_rgba8(&reference_input.bytes)?;
        let mut reference_rgb = Vec::with_capacity(ref_w * ref_h * 3);
        for px in rgba.chunks_exact(4) {
            reference_rgb.extend_from_slice(&px[..3]);
        }
        let (Some(vae), Some(unet), Some(dino)) = (
            self.vae_path.clone(),
            self.unet_path.clone(),
            self.dino_path.clone(),
        ) else {
            return Err(AssetAiError::Backend(
                "hunyuan3d-paint-2.1: cache paths were not resolved — call ensure_loaded first"
                    .to_string(),
            ));
        };
        let exec = NativeHunyuanExec::at_bins(HunyuanBins { vae, unet, dino })
            .map_err(map_pbr_error)?;
        let config = PaintConfig {
            num_views_max: 6,
            resolution: 512,
            texture_size: params.texture_size.unwrap_or(2048).clamp(64, 4096),
            view_select_res: 1024,
            depth_size: 2048,
            ortho_scale: 1.2,
            profile: MemoryProfile::Standard24g,
            seed: params.seed,
        };
        let mut pipeline = HunyuanPaintPipeline::new(exec, config)
            .with_license_acknowledgement(hunyuan_license()?);
        let inputs = PaintInputs {
            mesh: &mesh,
            reference_rgb: &reference_rgb,
            ref_width: ref_w as u32,
            ref_height: ref_h as u32,
            mesh_sha256: Some(mesh_sha),
            reference_sha256: Some(reference_sha),
            baked_ao: None,
        };
        let mut sink = |p: PbrProgress| -> bool {
            let (stage, frac) = stage_fraction(&p);
            progress(&stage, frac);
            !cancel.is_cancelled()
        };
        let set = pipeline.generate(&inputs, &mut sink).map_err(map_pbr_error)?;
        artifacts_from_material(&set, &mesh, &flat_indices)
    }
}

fn map_pbr_error(err: PbrError) -> AssetAiError {
    match err {
        PbrError::Cancelled => AssetAiError::Cancelled,
        PbrError::InvalidParams(m) => AssetAiError::Params(m),
        PbrError::Unavailable(m) => AssetAiError::Backend(format!("unavailable (fail-closed): {m}")),
        PbrError::Internal(m) => AssetAiError::Backend(m),
    }
}

fn artifacts_from_material(
    set: &PbrMaterialSet,
    mesh: &TriMesh,
    flat_indices: &[u32],
) -> Result<Vec<ArtifactData>, AssetAiError> {
    let albedo = set.albedo.map.as_ref().ok_or_else(|| {
        AssetAiError::Backend("material set missing albedo map".to_string())
    })?;
    let normal = set.normal.map.as_ref().ok_or_else(|| {
        AssetAiError::Backend("material set missing geometry-derived normal map".to_string())
    })?;
    let orm = set.packed_orm.as_ref().ok_or_else(|| {
        AssetAiError::Backend("material set missing packed ORM map".to_string())
    })?;

    // Albedo PNG is written RGBA (writer contract), alpha 255.
    let mut albedo_rgba = Vec::with_capacity(albedo.data.len() / 3 * 4);
    for px in albedo.data.chunks_exact(3) {
        albedo_rgba.extend_from_slice(px);
        albedo_rgba.push(255);
    }
    let albedo_png = encode_png(albedo.width, albedo.height, PngColor::Rgba, &albedo_rgba);
    let normal_png = encode_png(normal.width, normal.height, PngColor::Rgb, &normal.data);
    let orm_png = encode_png(orm.width, orm.height, PngColor::Rgb, &orm.data);

    // Textured GLB: the INPUT mesh geometry with the baked maps. The ORM
    // image doubles as the glTF metallicRoughness texture (G/B semantics
    // identical; R carries occlusion for consumers that bind it).
    let glb = write_glb_mesh_textured(&GlbTexturedMesh {
        positions: &mesh.positions,
        normals: Some(&mesh.normals),
        uvs: &mesh.uvs,
        indices: flat_indices,
        base_color_png: &albedo_png,
        metallic_roughness_png: Some(&orm_png),
        double_sided: false,
        colors: None,
    });

    let manifest = format!(
        "{{\n \"artifact_roles\": {{ \"textured_glb\": 0, \"albedo\": 1, \"normal\": 2, \"orm\": 3, \"material_manifest\": 4 }},\n \"orm_semantics\": \"R=occlusion (neutral 255 when absent), G=roughness, B=metallic; glTF metallicRoughnessTexture binds G/B, occlusionTexture binds R of the same image\",\n \"normal_semantics\": \"tangent-space, +Y up (OpenGL/glTF), geometry-derived\",\n \"material\": {}}}\n",
        set.manifest_json()
    );

    Ok(vec![
        ArtifactData {
            content_type: "model/gltf-binary",
            ext: "glb",
            bytes: glb,
        },
        ArtifactData {
            content_type: "image/png",
            ext: "albedo.png",
            bytes: albedo_png,
        },
        ArtifactData {
            content_type: "image/png",
            ext: "normal.png",
            bytes: normal_png,
        },
        ArtifactData {
            content_type: "image/png",
            ext: "orm.png",
            bytes: orm_png,
        },
        ArtifactData {
            content_type: "application/json",
            ext: "material.json",
            bytes: manifest.into_bytes(),
        },
    ])
}

impl ContentBackend for PaintBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        if self.deterministic {
            self.loaded = true;
            return Ok(());
        }
        if !hunyuan_native_provisioned() {
            return Err(AssetAiError::Backend(format!(
                "{}: {}",
                self.model_id, HUNYUAN_NATIVE_BLOCKER
            )));
        }
        ctx.ensure_files()?;
        self.unet_path = Some(ctx.path_by_role(ROLE_UNET)?);
        self.vae_path = Some(ctx.path_by_role(ROLE_VAE)?);
        self.dino_path = Some(ctx.path_by_role(ROLE_DINO)?);
        self.loaded = true;
        Ok(())
    }

    /// Honest residency. This used to answer a flat `false` while a native
    /// run had a full Hunyuan working set on the card, and every eviction path
    /// there is — the LRU candidate list, the idle sweep, `evict_resident` —
    /// gates on this answer. A model that says it holds nothing is a model
    /// nothing will ever retire, which is how the card ended up 43 GB down
    /// with `models_loaded: []`.
    ///
    /// The deterministic (test) mode really does hold no device memory, so it
    /// keeps saying so.
    fn is_resident(&self) -> bool {
        self.loaded && !self.deterministic
    }

    /// Drops the paths and the resident flag, and releases the device weight
    /// cache namespaces the native pipeline filled. The heavy allocations live
    /// in the worker thread's cache rather than in this struct, so clearing
    /// fields alone frees nothing — `server::evict_resident` follows this with
    /// a thread-cache release once nothing is resident, which is what actually
    /// returns the bytes.
    fn unload(&mut self) -> Result<(), AssetAiError> {
        self.loaded = false;
        self.unet_path = None;
        self.vae_path = None;
        self.dino_path = None;
        Ok(())
    }

    fn generate(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        if !self.deterministic {
            #[cfg(all(
                feature = "paint-cuda",
                any(target_os = "linux", target_os = "windows")
            ))]
            {
                return self.generate_native(params, progress, cancel);
            }
            #[cfg(not(all(
                feature = "paint-cuda",
                any(target_os = "linux", target_os = "windows")
            )))]
            {
                return Err(AssetAiError::Backend(format!(
                    "{}: {}",
                    self.model_id, HUNYUAN_NATIVE_BLOCKER
                )));
            }
        }
        let mesh_input = named_input(params, INPUT_MESH, MESH_CONTENT_TYPE)?;
        let reference_input = named_input(params, INPUT_REFERENCE, REFERENCE_CONTENT_TYPE)?;
        let mesh_sha = digest::sha256_hex(&mesh_input.bytes);
        let reference_sha = digest::sha256_hex(&reference_input.bytes);

        let (mesh, flat_indices) = glb_to_tri_mesh(&mesh_input.bytes)?;
        let (rgba, ref_w, ref_h) = decode_png_rgba8(&reference_input.bytes)?;
        let mut reference_rgb = Vec::with_capacity(ref_w * ref_h * 3);
        for px in rgba.chunks_exact(4) {
            reference_rgb.extend_from_slice(&px[..3]);
        }

        // Deterministic tier: small views, full artifact contract.
        let config = PaintConfig {
            num_views_max: 6,
            resolution: 64,
            texture_size: params.texture_size.unwrap_or(512).clamp(64, 4096),
            view_select_res: 128,
            depth_size: 128,
            ortho_scale: 1.2,
            profile: MemoryProfile::Standard24g,
            seed: params.seed,
        };
        let mut pipeline = HunyuanPaintPipeline::new(MockPaintExec::default(), config);
        let inputs = PaintInputs {
            mesh: &mesh,
            reference_rgb: &reference_rgb,
            ref_width: ref_w as u32,
            ref_height: ref_h as u32,
            mesh_sha256: Some(mesh_sha),
            reference_sha256: Some(reference_sha),
            baked_ao: None,
        };
        let mut sink = |p: PbrProgress| -> bool {
            let (stage, frac) = stage_fraction(&p);
            progress(&stage, frac);
            !cancel.is_cancelled()
        };
        let mut set = pipeline.generate(&inputs, &mut sink).map_err(map_pbr_error)?;

        // Honest provenance for the deterministic tier: this run used the
        // mock executor, not the Hunyuan weights — say so, and drop the
        // Hunyuan license/checkpoint identity lines.
        set.meta.generator = self.model_id.clone();
        set.meta.checkpoint_revision = "none".to_string();
        set.meta.extra.retain(|(k, _)| {
            !matches!(
                k.as_str(),
                "model"
                    | "weights_repo"
                    | "weights_revision"
                    | "code_repo"
                    | "code_revision"
                    | "conditioner"
                    | "license"
                    | "license_sha256"
                    | "license_url"
                    | "license_note"
            )
        });
        set.meta
            .extra
            .insert(0, ("model".to_string(), self.model_id.clone()));
        set.meta.extra.insert(
            1,
            (
                "execution".to_string(),
                "deterministic mock executor (no model weights)".to_string(),
            ),
        );
        set.validate()
            .map_err(|e| AssetAiError::Backend(e.to_string()))?;

        artifacts_from_material(&set, &mesh, &flat_indices)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{GenerateRequestJson, NamedInputJson};
    use makepad_base64::base64_encode;
    use makepad_ai_paint::hunyuan;
    use makepad_gltf::write_glb_mesh;

    fn cube_glb_no_uv() -> Vec<u8> {
        let cube = TriMesh::unit_cube();
        let flat: Vec<u32> = cube.indices.iter().flat_map(|t| t.iter().copied()).collect();
        write_glb_mesh(&cube.positions, &flat)
    }

    fn cube_glb() -> Vec<u8> {
        let cube = TriMesh::unit_cube_atlas();
        let flat: Vec<u32> = cube.indices.iter().flat_map(|t| t.iter().copied()).collect();
        let tiny_png = encode_png(1, 1, PngColor::Rgba, &[255, 255, 255, 255]);
        write_glb_mesh_textured(&GlbTexturedMesh {
            positions: &cube.positions,
            normals: Some(&cube.normals),
            uvs: &cube.uvs,
            indices: &flat,
            base_color_png: &tiny_png,
            metallic_roughness_png: None,
            double_sided: false,
        colors: None,
        })
    }

    fn reference_png() -> Vec<u8> {
        encode_png(8, 8, PngColor::Rgb, &[200u8; 8 * 8 * 3])
    }

    fn request_with(inputs: Vec<NamedInputJson>) -> GenerateRequestJson {
        GenerateRequestJson {
            model: "pbr-testpattern".to_string(),
            seed: Some(7),
            texture_size: Some(64),
            inputs: Some(inputs),
            ..Default::default()
        }
    }

    fn named(name: &str, content_type: &str, bytes: &[u8]) -> NamedInputJson {
        NamedInputJson {
            name: name.to_string(),
            content_type: content_type.to_string(),
            data_b64: String::from_utf8(base64_encode(bytes, &makepad_base64::BASE64_STANDARD))
                .unwrap(),
        }
    }

    fn spec() -> ModelSpec {
        crate::registry::ModelSpec {
            id: "pbr-testpattern".to_string(),
            domain: crate::registry::Domain::Paint,
            backend: "paint-test".to_string(),
            available: true,
            gated: false,
            vram_gb: Some(0.0),
            min_vram_gb: None,
            min_compute_cap: None,
            note: None,
            license: None,
            files: Vec::new(),
        }
    }

    fn run(request: &GenerateRequestJson) -> Result<Vec<ArtifactData>, AssetAiError> {
        let params = GenerateParams::from_request(request)?;
        let mut backend = PaintBackend::new(&spec());
        let cancel = CancelToken::new();
        let mut last = (String::new(), 0.0f64);
        let mut progress = |stage: &str, frac: f64| {
            last = (stage.to_string(), frac);
        };
        backend.generate(&params, &mut progress, &cancel)
    }

    #[test]
    fn e2e_dual_input_to_full_artifact_contract() {
        let request = request_with(vec![
            named(INPUT_MESH, MESH_CONTENT_TYPE, &cube_glb()),
            named(INPUT_REFERENCE, REFERENCE_CONTENT_TYPE, &reference_png()),
        ]);
        let artifacts = run(&request).unwrap();
        assert_eq!(artifacts.len(), 5);
        assert_eq!(artifacts[0].content_type, "model/gltf-binary");
        assert_eq!(artifacts[1].ext, "albedo.png");
        assert_eq!(artifacts[2].ext, "normal.png");
        assert_eq!(artifacts[3].ext, "orm.png");
        assert_eq!(artifacts[4].content_type, "application/json");
        // The output GLB must parse and carry the input topology.
        let reparsed = load_gltf_from_bytes(&artifacts[0].bytes, None).unwrap();
        assert_eq!(reparsed.document.meshes.as_deref().unwrap().len(), 1);
        let manifest = String::from_utf8(artifacts[4].bytes.clone()).unwrap();
        assert!(manifest.contains("\"artifact_roles\""));
        assert!(manifest.contains("\"orm\": 3"));
        assert!(manifest.contains("R=occlusion"));
        assert!(manifest.contains("input_mesh_sha256"));
        assert!(manifest.contains("input_reference_sha256"));
        // Deterministic-tier honesty: no Hunyuan checkpoint identity claimed.
        assert!(manifest.contains("pbr-testpattern"));
        assert!(manifest.contains("deterministic mock executor"));
        assert!(!manifest.contains(hunyuan::WEIGHTS_REVISION));
    }

    #[test]
    fn mesh_without_uvs_is_xatlas_unwrapped() {
        let request = request_with(vec![
            named(INPUT_MESH, MESH_CONTENT_TYPE, &cube_glb_no_uv()),
            named(INPUT_REFERENCE, REFERENCE_CONTENT_TYPE, &reference_png()),
        ]);
        let artifacts = run(&request).unwrap();
        assert_eq!(artifacts.len(), 5);
        let reparsed = load_gltf_from_bytes(&artifacts[0].bytes, None).unwrap();
        let prim = makepad_gltf::decode_mesh_primitive(&reparsed, 0, 0).unwrap();
        let uvs = prim.texcoords0.expect("paint output must carry TEXCOORD_0");
        assert_eq!(uvs.len(), prim.positions.len());
        assert!(uvs.iter().all(|uv| (0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[1])));
    }

    #[test]
    fn deterministic_artifacts_across_runs() {
        let request = request_with(vec![
            named(INPUT_MESH, MESH_CONTENT_TYPE, &cube_glb()),
            named(INPUT_REFERENCE, REFERENCE_CONTENT_TYPE, &reference_png()),
        ]);
        let a = run(&request).unwrap();
        let b = run(&request).unwrap();
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.bytes, y.bytes);
        }
    }

    #[test]
    fn missing_mesh_is_visibly_refused() {
        let request = request_with(vec![named(
            INPUT_REFERENCE,
            REFERENCE_CONTENT_TYPE,
            &reference_png(),
        )]);
        let err = match run(&request) {
            Err(err) => err,
            Ok(_) => panic!("expected refusal"),
        };
        let msg = format!("{err:?}");
        assert!(msg.contains("named input") && msg.contains("mesh"), "{msg}");
        assert!(msg.contains("no fallback"), "{msg}");
    }

    #[test]
    fn wrong_content_type_is_refused() {
        let request = request_with(vec![
            named(INPUT_MESH, "image/png", &cube_glb()),
            named(INPUT_REFERENCE, REFERENCE_CONTENT_TYPE, &reference_png()),
        ]);
        let err = match run(&request) {
            Err(err) => err,
            Ok(_) => panic!("expected refusal"),
        };
        assert!(format!("{err:?}").contains("content_type"));
    }

    #[test]
    fn input_b64_is_not_a_fallback() {
        let mut request = request_with(Vec::new());
        request.input_b64 = Some(
            String::from_utf8(base64_encode(&cube_glb(), &makepad_base64::BASE64_STANDARD))
                .unwrap(),
        );
        let err = match run(&request) {
            Err(err) => err,
            Ok(_) => panic!("expected refusal"),
        };
        assert!(format!("{err:?}").contains("mandatory"));
    }

    #[test]
    fn hostile_mesh_bytes_are_refused() {
        let request = request_with(vec![
            named(INPUT_MESH, MESH_CONTENT_TYPE, b"not a glb at all"),
            named(INPUT_REFERENCE, REFERENCE_CONTENT_TYPE, &reference_png()),
        ]);
        let err = match run(&request) {
            Err(err) => err,
            Ok(_) => panic!("expected refusal"),
        };
        assert!(format!("{err:?}").contains("GLB"));
    }

    #[test]
    fn hostile_reference_bytes_are_refused() {
        let request = request_with(vec![
            named(INPUT_MESH, MESH_CONTENT_TYPE, &cube_glb()),
            named(INPUT_REFERENCE, REFERENCE_CONTENT_TYPE, b"not a png"),
        ]);
        assert!(run(&request).is_err());
    }

    #[test]
    fn wire_parser_rejects_hostile_named_inputs() {
        // Duplicate names.
        let mut request = request_with(vec![
            named(INPUT_MESH, MESH_CONTENT_TYPE, b"a"),
            named(INPUT_MESH, MESH_CONTENT_TYPE, b"b"),
        ]);
        assert!(GenerateParams::from_request(&request).is_err());
        // Too many inputs.
        request = request_with(vec![
            named("a", "x/y", b"1"),
            named("b", "x/y", b"1"),
            named("c", "x/y", b"1"),
            named("d", "x/y", b"1"),
            named("e", "x/y", b"1"),
        ]);
        assert!(GenerateParams::from_request(&request).is_err());
        // Bad base64: invalid characters (4-char group) and a length that is
        // not a multiple of 4 — both must refuse, never panic.
        request = request_with(vec![NamedInputJson {
            name: INPUT_MESH.to_string(),
            content_type: MESH_CONTENT_TYPE.to_string(),
            data_b64: "@@@@".to_string(),
        }]);
        assert!(GenerateParams::from_request(&request).is_err());
        request = request_with(vec![NamedInputJson {
            name: INPUT_MESH.to_string(),
            content_type: MESH_CONTENT_TYPE.to_string(),
            data_b64: "abc".to_string(),
        }]);
        assert!(GenerateParams::from_request(&request).is_err());
        // Empty payload.
        request = request_with(vec![named(INPUT_MESH, MESH_CONTENT_TYPE, b"")]);
        assert!(GenerateParams::from_request(&request).is_err());
    }

    #[test]
    fn hunyuan_backend_fails_closed_without_native_runtime() {
        let registry =
            crate::registry::Registry::parse(crate::registry::EMBEDDED_REGISTRY).unwrap();
        let spec = registry.find("hunyuan3d-paint-2.1").expect("registry entry");
        let mut backend = PaintBackend::new(spec);
        let params = GenerateParams::from_request(&request_with(vec![
            named(INPUT_MESH, MESH_CONTENT_TYPE, &cube_glb()),
            named(INPUT_REFERENCE, REFERENCE_CONTENT_TYPE, &reference_png()),
        ]))
        .unwrap();
        let cancel = CancelToken::new();
        let mut progress = |_: &str, _: f64| {};
        let err = match backend.generate(&params, &mut progress, &cancel) {
            Err(err) => err,
            Ok(_) => panic!("expected fail-closed error"),
        };
        let text = format!("{err:?}");
        assert!(
            text.contains("not provisioned")
                || text.contains("unavailable")
                || text.contains("not in this build"),
            "fail-closed message: {text}"
        );
        assert!(!hunyuan_native_provisioned());
    }
}
