//! The Hunyuan3D-Paint-2.1 job pipeline contract: native orchestration around
//! the "existing mesh + reference image -> PBR material set" flow.
//!
//! Everything except the neural forward passes runs natively here and is
//! deterministic: input validation, mesh normalization, candidate rendering
//! and bake-view selection, geometry-conditioning maps, back-projection
//! baking, inpainting, ORM packing, provenance, progress and cancellation.
//! The neural stages (DINO/CLIP-free conditioning, VAE encode, UNet2p5D
//! denoise, VAE decode, optional ESRGAN) sit behind [`PaintModelExec`]. This
//! crate currently ships no complete real-model implementation: a private
//! deterministic mock drives orchestration tests, while [`UnavailableExec`]
//! fails closed. Any future real executor also requires an explicit
//! [`hunyuan::LicenseAcknowledgement`] before the first unit of work.

use crate::bake::{
    bake_from_views, bake_tangent_normal_map, dilate_inpaint, nearest_fill, BakeView, BAKE_EXP,
    TRUST_EPS,
};
use crate::camera::{candidate_views, default_orthographic, model_view_matrix, Mat4, CAMERA_DISTANCE};
use crate::contract::{pack_orm, ChannelSlot, ColorSpace, PbrMap, PbrMaterialSet, PbrMeta, PixelFormat};
use crate::hunyuan;
use crate::mesh::TriMesh;
use crate::raster::{normal_map_rgb8_negated, position_map_rgb8, render_gbuffer};
use crate::test_backend::{PbrError, PbrProgress, PbrStage};
use crate::view_select::bake_view_selection;

/// Flip an interleaved `width x width x channels` u8 image vertically.
fn flip_rows_u8(data: &mut [u8], width: usize, channels: usize) {
    let row = width * channels;
    let (mut top, mut bottom) = (0usize, width.saturating_sub(1));
    while top < bottom {
        for k in 0..row {
            data.swap(top * row + k, bottom * row + k);
        }
        top += 1;
        bottom -= 1;
    }
}

/// Flip an interleaved `width x width x channels` f32 image vertically.
fn flip_rows_f32(data: &mut [f32], width: usize, channels: usize) {
    let row = width * channels;
    let (mut top, mut bottom) = (0usize, width.saturating_sub(1));
    while top < bottom {
        for k in 0..row {
            data.swap(top * row + k, bottom * row + k);
        }
        top += 1;
        bottom -= 1;
    }
}

/// Upstream `MeshRender` mesh-normalization scale factor: after centering,
/// the largest radial distance becomes `PAINT_SCALE_FACTOR / 2`, and the
/// position conditioning encodes `0.5 - p / PAINT_SCALE_FACTOR`.
pub const PAINT_SCALE_FACTOR: f32 = 1.15;
/// Background for normal conditioning maps: WHITE — verified against the
/// official renderer 2026-08-18 (`MeshRender.render_normal` default
/// `bg_color=[1, 1, 1]`).
pub const CONDITIONING_BG: [u8; 3] = [255, 255, 255];
/// Background for position conditioning maps: WHITE, pinned — the upstream
/// voxel-index masking treats a pixel as background exactly when all three
/// channels equal 1.0 (`compute_discrete_voxel_indice`'s `position != 1`).
pub const POSITION_BG: [u8; 3] = [255, 255, 255];
pub const MAX_VIEW_RESOLUTION: u32 = 2_048;
pub const MAX_TEXTURE_SIZE: u32 = 4_096;
pub const MAX_VISIBILITY_RESOLUTION: u32 = 4_096;
pub const MAX_REFERENCE_DIMENSION: u32 = 8_192;
pub const MAX_REFERENCE_BYTES: usize = 256 * 1024 * 1024;
/// Below this box size the normalized half-unit mesh produces needlessly
/// extreme screen coordinates and f32 projection math can cease to be safe.
pub const MIN_ORTHO_SCALE: f32 = 1.0e-4;

#[derive(Clone, Copy, Debug)]
pub struct PaintConfig {
    /// Upstream `max_num_view`, 6..=9; the first six canonical views are fixed.
    pub num_views_max: u32,
    /// Diffusion view size (official baseline 512).
    pub resolution: u32,
    /// Output atlas size.
    pub texture_size: u32,
    /// Resolution for candidate-view visibility renders (upstream 1024).
    pub view_select_res: u32,
    /// Per-view depth-buffer size for bake visibility (upstream render_size 2048).
    pub depth_size: u32,
    /// Orthographic box side (upstream `ortho_scale`; verify exact default at oracle).
    pub ortho_scale: f32,
    pub profile: MemoryProfile,
    pub seed: u64,
}

impl Default for PaintConfig {
    fn default() -> Self {
        Self {
            num_views_max: 6,
            resolution: 512,
            texture_size: 2048,
            view_select_res: 1024,
            depth_size: 2048,
            ortho_scale: 1.2,
            profile: MemoryProfile::Standard24g,
            seed: 0,
        }
    }
}

/// Service-side admission reserve added on top of a model's declared peak.
pub const SERVICE_VRAM_RESERVE_MIB: u32 = 2048;
/// Measured usable total of the RTX 4090 admission class.
pub const RTX4090_TOTAL_MIB: u32 = 24_564;
/// The frozen torch oracle logged "22.15GB" device peak (all components
/// resident, 6 views @ 512). The log line did not state GiB vs decimal GB, so
/// both readings are kept until the native canary measures exactly; admission
/// math uses the worst case.
pub const ORACLE_PEAK_MIB_UPPER: u32 = 22_681;
pub const ORACLE_PEAK_MIB_LOWER: u32 = 21_125;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryProfile {
    /// Staged residency for the 24GB service class: encoders released before
    /// denoise, VAE decode after UNet release, view-batched bake. Declared
    /// peak is a budget ceiling the canary must verify, not a measurement.
    Standard24g,
    /// All components resident (the oracle's shape). Worst-case unit reading
    /// of the measured peak; does NOT fit a 4090 once the service reserve is
    /// added — exposed separately for larger cards.
    HighVram,
}

/// Truthful, unit-exact admission data for schedulers.
#[derive(Clone, Copy, Debug)]
pub struct AdmissionEstimate {
    pub profile: MemoryProfile,
    pub declared_peak_mib: u32,
    /// Whether `declared_peak_mib` is a measurement (vs a budget ceiling).
    pub measured: bool,
    pub basis: &'static str,
}

pub fn admission_estimate(profile: MemoryProfile) -> AdmissionEstimate {
    match profile {
        MemoryProfile::Standard24g => AdmissionEstimate {
            profile,
            declared_peak_mib: 20_000,
            measured: false,
            basis: "staged-residency budget ceiling (encoders released before denoise, VAE decode after UNet release, view-batched bake); to be verified at native canary. Oracle all-resident peak bounds: 21125..22681 MiB.",
        },
        MemoryProfile::HighVram => AdmissionEstimate {
            profile,
            declared_peak_mib: ORACLE_PEAK_MIB_UPPER,
            measured: true,
            basis: "frozen torch oracle device peak, all components resident, 6 views @ 512 (RTX 4090, 2026-08-11); worst-case unit reading of the 22.15GB log line.",
        },
    }
}

/// Does this profile fit the 24GB service class after the admission reserve?
pub fn fits_24g_service(estimate: &AdmissionEstimate) -> bool {
    estimate
        .declared_peak_mib
        .checked_add(SERVICE_VRAM_RESERVE_MIB)
        .is_some_and(|required| required <= RTX4090_TOTAL_MIB)
}

#[derive(Clone, Debug)]
pub enum ExecStatus {
    Ready { device: String, vram_gb: f32 },
    MissingCheckpoints { detail: String },
    NoCuda { detail: String },
}

/// Truthful implementation state of the built-in Hunyuan path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeImplementationStatus {
    pub real_model_executor_available: bool,
    pub detail: &'static str,
}

pub fn native_implementation_status() -> NativeImplementationStatus {
    NativeImplementationStatus {
        real_model_executor_available: false,
        detail: "native run_multiview is wired (VAE + DINO ViT + dual write + 15-step DDIM); a 512-view service job has not been checked on the paint box, so real_model_executor_available stays false",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaintExecutionKind {
    NativeHunyuan,
    DeterministicMock,
    Unavailable,
}

/// One selected view's conditioning package.
#[derive(Clone, Debug)]
pub struct ViewConditioning {
    pub azim: f32,
    pub elev: f32,
    pub weight: f32,
    pub size: u32,
    /// World-space ("absolute") normal map, RGB8, `n*0.5+0.5`.
    pub normal_map_rgb: Vec<u8>,
    /// Position map, RGB8, `0.5 - p/scale` with the normalized-mesh scale.
    pub position_map_rgb: Vec<u8>,
}

pub struct PaintCondition<'a> {
    pub reference_rgb: &'a [u8],
    pub ref_width: u32,
    pub ref_height: u32,
    pub views: &'a [ViewConditioning],
    pub seed: u64,
    pub resolution: u32,
}

/// Model output: per-view albedo and metallic-roughness images in linear f32
/// RGB (MR semantics follow upstream: G = roughness, B = metallic).
pub struct MultiviewPbr {
    pub size: u32,
    pub albedo: Vec<Vec<f32>>,
    pub mr: Vec<Vec<f32>>,
}

/// The neural-execution seam. This crate supplies [`UnavailableExec`] for
/// fail-closed hosts and a private deterministic test implementation; a
/// complete native CUDA implementation remains future work.
pub trait PaintModelExec {
    fn execution_kind(&self) -> PaintExecutionKind;
    fn availability(&self) -> ExecStatus;
    fn is_resident(&self) -> bool;
    /// Load weights / acquire device residency.
    fn warm(&mut self) -> Result<(), PbrError>;
    /// Release device residency.
    fn release(&mut self);
    /// Run the multiview PBR diffusion. `progress(step, total)` returns false
    /// to cancel; implementations must honor it promptly.
    fn run_multiview(
        &mut self,
        cond: &PaintCondition,
        progress: &mut dyn FnMut(u32, u32) -> bool,
    ) -> Result<MultiviewPbr, PbrError>;
}

/// Fail-closed executor for hosts without native CUDA or pinned checkpoints.
pub struct UnavailableExec {
    pub reason: String,
}

impl PaintModelExec for UnavailableExec {
    fn execution_kind(&self) -> PaintExecutionKind {
        PaintExecutionKind::Unavailable
    }

    fn availability(&self) -> ExecStatus {
        ExecStatus::NoCuda {
            detail: self.reason.clone(),
        }
    }
    fn is_resident(&self) -> bool {
        false
    }
    fn warm(&mut self) -> Result<(), PbrError> {
        Err(PbrError::Unavailable(self.reason.clone()))
    }
    fn release(&mut self) {}
    fn run_multiview(
        &mut self,
        _cond: &PaintCondition,
        _progress: &mut dyn FnMut(u32, u32) -> bool,
    ) -> Result<MultiviewPbr, PbrError> {
        Err(PbrError::Unavailable(self.reason.clone()))
    }
}

/// Deterministic test executor: albedo := the view's normal map, MR := the
/// view's position map (both as f32). Geometry conditioning therefore flows
/// through the whole pipeline and is assertable in baked output.
/// Deterministic development executor for testing the orchestration and
/// artifact contract. It executes no model weights, identifies its outputs as
/// mock data, and must never be registered as the real Hunyuan backend.
pub struct MockPaintExec {
    pub resident: bool,
    pub warm_calls: u32,
    pub denoise_steps: u32,
}

impl Default for MockPaintExec {
    fn default() -> Self {
        Self {
            resident: false,
            warm_calls: 0,
            denoise_steps: 12,
        }
    }
}

impl PaintModelExec for MockPaintExec {
    fn execution_kind(&self) -> PaintExecutionKind {
        PaintExecutionKind::DeterministicMock
    }

    fn availability(&self) -> ExecStatus {
        ExecStatus::Ready {
            device: "mock".to_string(),
            vram_gb: 0.0,
        }
    }
    fn is_resident(&self) -> bool {
        self.resident
    }
    fn warm(&mut self) -> Result<(), PbrError> {
        self.resident = true;
        self.warm_calls += 1;
        Ok(())
    }
    fn release(&mut self) {
        self.resident = false;
    }
    fn run_multiview(
        &mut self,
        cond: &PaintCondition,
        progress: &mut dyn FnMut(u32, u32) -> bool,
    ) -> Result<MultiviewPbr, PbrError> {
        let total = self.denoise_steps;
        for step in 1..=total {
            if !progress(step, total) {
                return Err(PbrError::Cancelled);
            }
        }
        let to_f32 = |bytes: &[u8]| -> Vec<f32> { bytes.iter().map(|b| *b as f32 / 255.0).collect() };
        Ok(MultiviewPbr {
            size: cond.views.first().map(|v| v.size).unwrap_or(cond.resolution),
            albedo: cond.views.iter().map(|v| to_f32(&v.normal_map_rgb)).collect(),
            mr: cond.views.iter().map(|v| to_f32(&v.position_map_rgb)).collect(),
        })
    }
}

/// Inputs for one paint job. The service layer supplies exact input digests so
/// output provenance can bind the result to both inputs (dual-input contract).
pub struct PaintInputs<'a> {
    pub mesh: &'a TriMesh,
    pub reference_rgb: &'a [u8],
    pub ref_width: u32,
    pub ref_height: u32,
    pub mesh_sha256: Option<String>,
    pub reference_sha256: Option<String>,
    /// Optional engine-baked AO (Gray8 at texture_size²). When present it
    /// rides the packed ORM R channel as GeometryDerived; never model output.
    pub baked_ao: Option<&'a PbrMap>,
}

pub struct HunyuanPaintPipeline<E: PaintModelExec> {
    pub exec: E,
    pub config: PaintConfig,
    license_acknowledgement: Option<hunyuan::LicenseAcknowledgement>,
}

impl<E: PaintModelExec> HunyuanPaintPipeline<E> {
    pub fn new(exec: E, config: PaintConfig) -> Self {
        Self {
            exec,
            config,
            license_acknowledgement: None,
        }
    }

    /// Attach the explicit acknowledgement required by checkpoint
    /// provisioning and any executor claiming to run the real Hunyuan model.
    pub fn with_license_acknowledgement(
        mut self,
        acknowledgement: hunyuan::LicenseAcknowledgement,
    ) -> Self {
        self.license_acknowledgement = Some(acknowledgement);
        self
    }

    pub fn release(&mut self) {
        self.exec.release();
    }

    pub fn generate(
        &mut self,
        inputs: &PaintInputs,
        progress: &mut dyn FnMut(PbrProgress) -> bool,
    ) -> Result<PbrMaterialSet, PbrError> {
        let execution_kind = self.exec.execution_kind();
        let acknowledged_license_digest = match execution_kind {
            PaintExecutionKind::NativeHunyuan => Some(
                self.license_acknowledgement
                    .as_ref()
                    .ok_or_else(|| {
                        PbrError::Unavailable(format!(
                            "{} license acknowledgement missing; present {} and explicitly accept digest {} before provisioning or execution",
                            hunyuan::MODEL_ID,
                            hunyuan::LICENSE_URL,
                            hunyuan::LICENSE_TEXT_SHA256
                        ))
                    })?
                    .license_text_sha256()
                    .to_string(),
            ),
            PaintExecutionKind::DeterministicMock => None,
            PaintExecutionKind::Unavailable => {
                return Err(PbrError::Unavailable(
                    "executor declared itself unavailable; refusing an inconsistent ready status"
                        .to_string(),
                ));
            }
        };
        // Fail closed before any work if the neural runtime cannot run here.
        match self.exec.availability() {
            ExecStatus::Ready { .. } => {}
            ExecStatus::MissingCheckpoints { detail } | ExecStatus::NoCuda { detail } => {
                return Err(PbrError::Unavailable(detail));
            }
        }
        let cfg = self.config;
        if !(6..=9).contains(&cfg.num_views_max) {
            return Err(PbrError::InvalidParams(format!(
                "num_views_max {} outside 6..=9",
                cfg.num_views_max
            )));
        }
        let bounded_dimension = |name: &str, value: u32, maximum: u32| {
            if value == 0 || value > maximum {
                Err(PbrError::InvalidParams(format!(
                    "{name} {value} outside 1..={maximum}"
                )))
            } else {
                Ok(())
            }
        };
        bounded_dimension("resolution", cfg.resolution, MAX_VIEW_RESOLUTION)?;
        bounded_dimension("texture_size", cfg.texture_size, MAX_TEXTURE_SIZE)?;
        bounded_dimension(
            "view_select_res",
            cfg.view_select_res,
            MAX_VISIBILITY_RESOLUTION,
        )?;
        bounded_dimension("depth_size", cfg.depth_size, MAX_VISIBILITY_RESOLUTION)?;
        let checked_square = |name: &str, size: u32, channels: usize| {
            usize::try_from(size)
                .ok()
                .and_then(|size| size.checked_mul(size))
                .and_then(|pixels| pixels.checked_mul(channels))
                .ok_or_else(|| PbrError::InvalidParams(format!("{name} allocation size overflow")))
        };
        checked_square("diffusion view", cfg.resolution, 3)?;
        checked_square("texture atlas", cfg.texture_size, 3)?;
        checked_square("view-selection buffer", cfg.view_select_res, 1)?;
        checked_square("depth buffer", cfg.depth_size, 1)?;
        if !cfg.ortho_scale.is_finite()
            || cfg.ortho_scale < MIN_ORTHO_SCALE
            || cfg.ortho_scale > 1_000.0
        {
            return Err(PbrError::InvalidParams(format!(
                "ortho_scale {} must be finite and in [{MIN_ORTHO_SCALE},1000]",
                cfg.ortho_scale,
            )));
        }
        let proj = default_orthographic(cfg.ortho_scale);
        if proj.iter().flatten().any(|value| !value.is_finite()) {
            return Err(PbrError::InvalidParams(
                "ortho_scale produced a nonfinite projection matrix".to_string(),
            ));
        }
        inputs
            .mesh
            .validate(true)
            .map_err(|error| PbrError::InvalidParams(error.to_string()))?;
        let total_area = inputs.mesh.total_area();
        if !total_area.is_finite() || total_area <= 1e-20 {
            return Err(PbrError::InvalidParams(
                "mesh has no finite nonzero surface area".to_string(),
            ));
        }
        bounded_dimension(
            "reference width",
            inputs.ref_width,
            MAX_REFERENCE_DIMENSION,
        )?;
        bounded_dimension(
            "reference height",
            inputs.ref_height,
            MAX_REFERENCE_DIMENSION,
        )?;
        let expect_ref = usize::try_from(inputs.ref_width)
            .ok()
            .and_then(|width| {
                usize::try_from(inputs.ref_height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .and_then(|pixels| pixels.checked_mul(3))
            .ok_or_else(|| {
                PbrError::InvalidParams("reference image byte length overflow".to_string())
            })?;
        if expect_ref > MAX_REFERENCE_BYTES {
            return Err(PbrError::InvalidParams(format!(
                "reference image requires {expect_ref} bytes (limit {MAX_REFERENCE_BYTES})"
            )));
        }
        if inputs.reference_rgb.len() != expect_ref {
            return Err(PbrError::InvalidParams(format!(
                "reference image bytes {} do not match {}x{} RGB8",
                inputs.reference_rgb.len(),
                inputs.ref_width,
                inputs.ref_height
            )));
        }

        if let Some(ao) = inputs.baked_ao {
            if ao.width != cfg.texture_size
                || ao.height != cfg.texture_size
                || ao.format != PixelFormat::Gray8
                || ao.color_space != ColorSpace::Linear
                || ao.expected_len().ok() != Some(ao.data.len())
            {
                return Err(PbrError::InvalidParams(format!(
                    "baked AO must be Gray8 {0}x{0} with matching data",
                    cfg.texture_size
                )));
            }
        }

        let emit = |stage: PbrStage, current: u32, total: u32, progress: &mut dyn FnMut(PbrProgress) -> bool| {
            if progress(PbrProgress { stage, current, total }) {
                Ok(())
            } else {
                Err(PbrError::Cancelled)
            }
        };

        emit(PbrStage::Prepare, 1, 1, progress)?;
        // Normalized working copy in the upstream renderer's frame: glTF
        // Y-up mapped to the Z-up paint world, bbox-centered, max radial
        // distance PAINT_SCALE_FACTOR/2 — so the position-map encoding
        // `0.5 - p / PAINT_SCALE_FACTOR` lands in [0,1] and the Z-up camera
        // ring actually orbits the character's vertical axis.
        let mut mesh = inputs.mesh.clone();
        mesh.apply_paint_frame();
        // Outward normals from the (re-oriented) winding: bake facing tests,
        // the tangent-space normal map and the exported GLB all need them.
        mesh.compute_vertex_normals();
        mesh.normalize_paint_radial(PAINT_SCALE_FACTOR);
        mesh.validate(true).map_err(|error| {
            PbrError::InvalidParams(format!("normalized mesh invalid: {error}"))
        })?;

        // ---- View selection over the exact upstream candidate set.
        let candidates = candidate_views();
        let face_areas = mesh.face_areas();
        let total_area: f32 = face_areas.iter().sum();
        let area_ratios: Vec<f64> = face_areas
            .iter()
            .map(|a| (*a / total_area.max(1e-20)) as f64)
            .collect();
        let mut visible = Vec::with_capacity(candidates.len());
        for (i, cand) in candidates.iter().enumerate() {
            emit(PbrStage::ViewSelect, i as u32 + 1, candidates.len() as u32, progress)?;
            let mv = model_view_matrix(cand.elev, cand.azim, CAMERA_DISTANCE, [0.0; 3]);
            let gbuf = render_gbuffer(
                &mesh,
                &mv,
                &proj,
                cfg.view_select_res as usize,
                cfg.view_select_res as usize,
            );
            visible.push(gbuf.visible_faces());
        }
        let selection = bake_view_selection(&area_ratios, &visible, cfg.num_views_max as usize);

        // ---- Geometry conditioning maps for the selected views.
        let mut views = Vec::with_capacity(selection.selected.len());
        let mut view_mats: Vec<Mat4> = Vec::with_capacity(selection.selected.len());
        for (i, &cand_idx) in selection.selected.iter().enumerate() {
            emit(
                PbrStage::GeometryRender,
                i as u32 + 1,
                selection.selected.len() as u32,
                progress,
            )?;
            let cand = candidates[cand_idx];
            let mv = model_view_matrix(cand.elev, cand.azim, CAMERA_DISTANCE, [0.0; 3]);
            let gbuf = render_gbuffer(&mesh, &mv, &proj, cfg.resolution as usize, cfg.resolution as usize);
            // The official stack rasterizes GL-style (row 0 = NDC bottom) and
            // hands that buffer to PIL unflipped, so the model was trained on
            // vertically flipped renders relative to our top-down row order.
            // Flip the conditioning to the model's orientation; the model's
            // outputs are flipped back before the bake below.
            // Upstream shades with cross-product normals of the reflected
            // (still original-winding) triangles = the NEGATION of the
            // outward normals our re-oriented mesh carries. Measured against
            // the official render_normal on the same mesh: channel means
            // official (+2.7, -89.8, +22.1) vs ours-outward (-3.6, +89.7,
            // -23.3). Encode -n for the conditioning only.
            let mut normal_map_rgb = normal_map_rgb8_negated(&gbuf, CONDITIONING_BG);
            let mut position_map_rgb = position_map_rgb8(&gbuf, PAINT_SCALE_FACTOR, POSITION_BG);
            flip_rows_u8(&mut normal_map_rgb, cfg.resolution as usize, 3);
            flip_rows_u8(&mut position_map_rgb, cfg.resolution as usize, 3);
            views.push(ViewConditioning {
                azim: cand.azim,
                elev: cand.elev,
                weight: cand.weight,
                size: cfg.resolution,
                normal_map_rgb,
                position_map_rgb,
            });
            view_mats.push(mv);
        }

        // ---- Neural stage behind the exec seam (residency-aware).
        if !self.exec.is_resident() {
            emit(PbrStage::Encode, 1, 1, progress)?;
            self.exec.warm()?;
        }
        let cond = PaintCondition {
            reference_rgb: inputs.reference_rgb,
            ref_width: inputs.ref_width,
            ref_height: inputs.ref_height,
            views: &views,
            seed: cfg.seed,
            resolution: cfg.resolution,
        };
        let mut cancelled = false;
        let multiview = {
            let progress_ref: &mut dyn FnMut(PbrProgress) -> bool = progress;
            let mut map_progress = |step: u32, total: u32| -> bool {
                let go = progress_ref(PbrProgress {
                    stage: PbrStage::Denoise,
                    current: step,
                    total,
                });
                if !go {
                    cancelled = true;
                }
                go
            };
            self.exec.run_multiview(&cond, &mut map_progress)
        }?;
        if cancelled {
            return Err(PbrError::Cancelled);
        }
        if multiview.albedo.len() != views.len() || multiview.mr.len() != views.len() {
            return Err(PbrError::Internal(format!(
                "exec returned {} albedo / {} mr views for {} conditioned views",
                multiview.albedo.len(),
                multiview.mr.len(),
                views.len()
            )));
        }
        bounded_dimension("model output size", multiview.size, MAX_VIEW_RESOLUTION)
            .map_err(|error| PbrError::Internal(error.to_string()))?;
        let view_px = usize::try_from(multiview.size)
            .ok()
            .and_then(|size| size.checked_mul(size))
            .and_then(|pixels| pixels.checked_mul(3))
            .ok_or_else(|| PbrError::Internal("model output size overflow".to_string()))?;
        for img in multiview.albedo.iter().chain(multiview.mr.iter()) {
            if img.len() != view_px {
                return Err(PbrError::Internal("exec view image size mismatch".to_string()));
            }
            if img.iter().any(|value| !value.is_finite()) {
                return Err(PbrError::Internal(
                    "exec returned non-finite image values".to_string(),
                ));
            }
        }

        // The model works in the flipped (official) orientation; bring its
        // views back into our raster row order before the bake projections.
        let mut multiview = multiview;
        for img in multiview.albedo.iter_mut().chain(multiview.mr.iter_mut()) {
            flip_rows_f32(img, multiview.size as usize, 3);
        }

        // ---- Bake albedo and MR into the input mesh's UV atlas.
        fn make_bake_views<'v>(
            images: &'v [Vec<f32>],
            view_mats: &[Mat4],
            selected: &[usize],
            candidates: &[crate::camera::ViewCandidate],
            size: usize,
            proj: Mat4,
        ) -> Vec<BakeView<'v>> {
            images
                .iter()
                .zip(view_mats.iter())
                .zip(selected.iter())
                .map(|((img, mv), &cand_idx)| BakeView {
                    rgb: img,
                    width: size,
                    height: size,
                    mv: *mv,
                    proj,
                    weight: candidates[cand_idx].weight,
                })
                .collect()
        }
        let view_px_size = multiview.size as usize;
        emit(PbrStage::Bake, 1, 2, progress)?;
        let tex = cfg.texture_size as usize;
        let baked_albedo = bake_from_views(
            &mesh,
            &make_bake_views(&multiview.albedo, &view_mats, &selection.selected, &candidates, view_px_size, proj),
            tex,
            cfg.depth_size as usize,
            BAKE_EXP,
            1e-3,
        );
        emit(PbrStage::Bake, 2, 2, progress)?;
        let baked_mr = bake_from_views(
            &mesh,
            &make_bake_views(&multiview.mr, &view_mats, &selection.selected, &candidates, view_px_size, proj),
            tex,
            cfg.depth_size as usize,
            BAKE_EXP,
            1e-3,
        );

        emit(PbrStage::Inpaint, 1, 1, progress)?;
        let mut albedo_rgb = baked_albedo.rgb;
        let mut albedo_valid: Vec<bool> = baked_albedo.trust.iter().map(|t| *t > TRUST_EPS).collect();
        dilate_inpaint(&mut albedo_rgb, &mut albedo_valid, tex, 4);
        nearest_fill(&mut albedo_rgb, &mut albedo_valid, tex);
        let mut mr_rgb = baked_mr.rgb;
        let mut mr_valid: Vec<bool> = baked_mr.trust.iter().map(|t| *t > TRUST_EPS).collect();
        dilate_inpaint(&mut mr_rgb, &mut mr_valid, tex, 4);
        nearest_fill(&mut mr_rgb, &mut mr_valid, tex);

        // ---- Quantize + pack per the ORM contract.
        let q = |v: f32| -> u8 { (v.clamp(0.0, 1.0) * 255.0).round() as u8 };
        let count = tex
            .checked_mul(tex)
            .ok_or_else(|| PbrError::Internal("texture pixel count overflow".to_string()))?;
        let albedo_len = count
            .checked_mul(3)
            .ok_or_else(|| PbrError::Internal("albedo byte count overflow".to_string()))?;
        let mut albedo_bytes = Vec::new();
        albedo_bytes
            .try_reserve_exact(albedo_len)
            .map_err(|_| PbrError::Internal("albedo allocation failed".to_string()))?;
        for i in 0..count {
            albedo_bytes.push(q(albedo_rgb[i * 3]));
            albedo_bytes.push(q(albedo_rgb[i * 3 + 1]));
            albedo_bytes.push(q(albedo_rgb[i * 3 + 2]));
        }
        let mut rough_bytes = Vec::new();
        let mut metal_bytes = Vec::new();
        rough_bytes
            .try_reserve_exact(count)
            .map_err(|_| PbrError::Internal("roughness allocation failed".to_string()))?;
        metal_bytes
            .try_reserve_exact(count)
            .map_err(|_| PbrError::Internal("metallic allocation failed".to_string()))?;
        for i in 0..count {
            rough_bytes.push(q(mr_rgb[i * 3 + 1])); // MR G = roughness
            metal_bytes.push(q(mr_rgb[i * 3 + 2])); // MR B = metallic
        }
        let size = cfg.texture_size;
        let albedo = PbrMap {
            width: size,
            height: size,
            format: PixelFormat::Rgb8,
            color_space: ColorSpace::Srgb,
            data: albedo_bytes,
        };
        let rough = PbrMap {
            width: size,
            height: size,
            format: PixelFormat::Gray8,
            color_space: ColorSpace::Linear,
            data: rough_bytes,
        };
        let metal = PbrMap {
            width: size,
            height: size,
            format: PixelFormat::Gray8,
            color_space: ColorSpace::Linear,
            data: metal_bytes,
        };
        let normal_map = PbrMap {
            width: size,
            height: size,
            format: PixelFormat::Rgb8,
            color_space: ColorSpace::Linear,
            data: bake_tangent_normal_map(&mesh, tex),
        };
        let packed = pack_orm(inputs.baked_ao, &rough, &metal)
            .map_err(|e| PbrError::Internal(e.to_string()))?;
        let occlusion_slot = match inputs.baked_ao {
            Some(ao) => ChannelSlot::geometry_derived("engine per-asset AO baker", ao.clone()),
            None => ChannelSlot::absent(
                "not generated; per-asset AO baker available engine-side; packed ORM R is neutral 255",
            ),
        };

        let (output_model, checkpoint_revision) = match execution_kind {
            PaintExecutionKind::DeterministicMock => {
                ("hunyuan-pipeline-deterministic-mock-v1", "none")
            }
            PaintExecutionKind::NativeHunyuan => (hunyuan::MODEL_ID, hunyuan::WEIGHTS_REVISION),
            PaintExecutionKind::Unavailable => {
                return Err(PbrError::Unavailable(
                    "unavailable executor reached provenance assembly".to_string(),
                ));
            }
        };
        let mut meta = PbrMeta::new(
            output_model,
            checkpoint_revision,
            cfg.seed,
            selection.selected.len() as u32,
        );
        match execution_kind {
            PaintExecutionKind::DeterministicMock => meta.extra.push((
                "execution_backend".to_string(),
                "deterministic orchestration mock; no Hunyuan weights executed".to_string(),
            )),
            PaintExecutionKind::NativeHunyuan => {
                meta.extra = hunyuan::provenance();
                let digest = acknowledged_license_digest.ok_or_else(|| {
                    PbrError::Unavailable(
                        "native Hunyuan execution lost its license acknowledgement".to_string(),
                    )
                })?;
                meta.extra
                    .push(("license_acknowledged_sha256".to_string(), digest));
            }
            PaintExecutionKind::Unavailable => {
                return Err(PbrError::Unavailable(
                    "unavailable executor reached provenance assembly".to_string(),
                ));
            }
        }
        meta.extra.push(("resolution".to_string(), cfg.resolution.to_string()));
        meta.extra.push(("views_used".to_string(), selection.selected.len().to_string()));
        let adm = admission_estimate(cfg.profile);
        meta.extra.push(("memory_profile".to_string(), format!("{:?}", cfg.profile)));
        meta.extra.push(("declared_peak_mib".to_string(), adm.declared_peak_mib.to_string()));
        if let Some(h) = &inputs.mesh_sha256 {
            meta.extra.push(("input_mesh_sha256".to_string(), h.clone()));
        }
        if let Some(h) = &inputs.reference_sha256 {
            meta.extra.push(("input_reference_sha256".to_string(), h.clone()));
        }

        let set = PbrMaterialSet {
            albedo: ChannelSlot::generated(output_model, albedo),
            normal: ChannelSlot::geometry_derived(
                "uv-atlas tangent-space bake of mesh vertex normals (+Y up); the model does not generate normals",
                normal_map,
            ),
            roughness: ChannelSlot::generated(output_model, rough),
            metallic: ChannelSlot::generated(output_model, metal),
            occlusion: occlusion_slot,
            packed_orm: Some(packed),
            meta,
        };
        set.validate().map_err(|e| PbrError::Internal(e.to_string()))?;
        emit(PbrStage::Finalize, 1, 1, progress)?;
        Ok(set)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{ChannelOrigin, NEUTRAL_OCCLUSION};

    fn test_config() -> PaintConfig {
        PaintConfig {
            num_views_max: 6,
            resolution: 64,
            texture_size: 48,
            view_select_res: 96,
            depth_size: 96,
            ortho_scale: 1.2,
            profile: MemoryProfile::Standard24g,
            seed: 9,
        }
    }

    fn test_inputs(mesh: &TriMesh) -> PaintInputs<'_> {
        PaintInputs {
            mesh,
            reference_rgb: &REF_8X8,
            ref_width: 8,
            ref_height: 8,
            mesh_sha256: Some("aa".repeat(32)),
            reference_sha256: Some("bb".repeat(32)),
            baked_ao: None,
        }
    }

    static REF_8X8: [u8; 8 * 8 * 3] = [200; 8 * 8 * 3];

    fn run_ok() -> PbrMaterialSet {
        let mesh = TriMesh::unit_cube_atlas();
        let mut pipe = HunyuanPaintPipeline::new(MockPaintExec::default(), test_config());
        pipe.generate(&test_inputs(&mesh), &mut |_| true).unwrap()
    }

    fn face_cell_center(face: usize, size: usize) -> usize {
        let col = face % 3;
        let row = face / 3;
        let x = col * size / 3 + size / 6;
        let y = row * size / 2 + size / 4;
        y * size + x
    }

    #[test]
    fn e2e_mock_geometry_flows_to_baked_albedo() {
        let set = run_ok();
        set.validate().unwrap();
        let albedo = set.albedo.map.as_ref().unwrap();
        assert_eq!(albedo.width, 48);
        // Mock albedo := the world-normal conditioning map (paint frame,
        // model-orientation flip and all), so every baked face cell must
        // hold an axis-aligned encoded normal: exactly one channel saturated
        // (0 or 255) and the other two at 128. That is the "geometry flows
        // to the baked albedo" invariant independent of the frame mapping.
        // Faces 0/1 (+X/-X in glTF) land on the paint frame's side ring and
        // are always covered by the canonical azimuth views; the glTF +Y face
        // becomes the paint-frame bottom, which the 6-view selection may
        // legitimately skip (characters are not textured from below).
        let nonblack = albedo.data.chunks_exact(3).filter(|p| p.iter().any(|v| *v > 3)).count();
        assert!(nonblack > 0, "bake produced no texels");
        let mut covered = 0;
        for face in [0usize, 1] {
            let c = face_cell_center(face, 48);
            let px = &albedo.data[c * 3..c * 3 + 3];
            let saturated = px.iter().filter(|v| **v <= 3 || **v >= 252).count();
            let mid = px.iter().filter(|v| (**v as i32 - 128).abs() <= 3).count();
            if saturated == 1 && mid == 2 {
                covered += 1;
            } else {
                assert_eq!(px, &[0u8, 0, 0], "face {face} neither axis normal nor empty: {px:?}");
            }
        }
        assert!(covered >= 1, "no side face received a baked axis normal");
        // ORM present with neutral R (occlusion honestly absent).
        let orm = set.packed_orm.as_ref().unwrap();
        assert!(orm.data.chunks_exact(3).all(|p| p[0] == NEUTRAL_OCCLUSION));
        assert!(matches!(set.normal.origin, ChannelOrigin::GeometryDerived { .. }));
        assert_eq!(set.meta.views_used, 6);
    }

    #[test]
    fn channel_origins_exact_regression() {
        let set = run_ok();
        // Model-generated: albedo, roughness, metallic — and ONLY these.
        assert!(matches!(set.albedo.origin, ChannelOrigin::Generated { .. }));
        assert!(matches!(set.roughness.origin, ChannelOrigin::Generated { .. }));
        assert!(matches!(set.metallic.origin, ChannelOrigin::Generated { .. }));
        // Normal and AO must never be labeled Generated.
        assert!(matches!(set.normal.origin, ChannelOrigin::GeometryDerived { .. }));
        assert!(matches!(set.occlusion.origin, ChannelOrigin::Absent { .. }));
        for slot in [&set.albedo, &set.roughness, &set.metallic] {
            match &slot.origin {
                ChannelOrigin::Generated { model } => {
                    assert_eq!(model, "hunyuan-pipeline-deterministic-mock-v1");
                    assert_ne!(model, hunyuan::MODEL_ID);
                }
                other => panic!("expected generated mock channel, got {other:?}"),
            }
        }
        // The hard-faced cube bakes a flat tangent-space normal map on every
        // baked face cell (face 0/1 sit on the paint-frame side ring; the
        // glTF +Y face may be uncovered by the 6-view selection).
        let n = set.normal.map.as_ref().unwrap();
        let cells: Vec<[u8; 3]> = (0..6)
            .map(|f| { let c = face_cell_center(f, 48); [n.data[c * 3], n.data[c * 3 + 1], n.data[c * 3 + 2]] })
            .collect();
        eprintln!("tangent normal cells: {cells:?}");
        let flat = cells.iter().filter(|c| **c == [128, 128, 255]).count();
        assert!(flat >= 1, "no face carries a flat tangent normal: {cells:?}");
    }

    #[test]
    fn supplied_ao_rides_orm_r_as_geometry_derived() {
        let mesh = TriMesh::unit_cube_atlas();
        let ao = PbrMap {
            width: 48,
            height: 48,
            format: PixelFormat::Gray8,
            color_space: ColorSpace::Linear,
            data: vec![200; 48 * 48],
        };
        let mut pipe = HunyuanPaintPipeline::new(MockPaintExec::default(), test_config());
        let mut inputs = test_inputs(&mesh);
        inputs.baked_ao = Some(&ao);
        let set = pipe.generate(&inputs, &mut |_| true).unwrap();
        set.validate().unwrap();
        assert!(matches!(set.occlusion.origin, ChannelOrigin::GeometryDerived { .. }));
        let orm = set.packed_orm.as_ref().unwrap();
        assert!(orm.data.chunks_exact(3).all(|p| p[0] == 200));
    }

    #[test]
    fn mock_provenance_is_honest_and_binds_both_inputs() {
        let set = run_ok();
        let get = |k: &str| {
            set.meta
                .extra
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| panic!("missing provenance key {k}"))
        };
        assert!(get("execution_backend").contains("no Hunyuan weights executed"));
        assert_eq!(get("input_mesh_sha256"), "aa".repeat(32));
        assert_eq!(get("input_reference_sha256"), "bb".repeat(32));
        assert_eq!(get("views_used"), "6");
        assert_eq!(get("memory_profile"), "Standard24g");
        assert_eq!(get("declared_peak_mib"), "20000");
        let manifest = set.manifest_json();
        assert!(manifest.contains("\"provenance\""));
        assert!(!manifest.contains(hunyuan::WEIGHTS_REVISION));
        assert!(!manifest.contains(hunyuan::LICENSE_TEXT_SHA256));
    }

    #[test]
    fn fail_closed_without_native_runtime() {
        let mesh = TriMesh::unit_cube_atlas();
        let mut pipe = HunyuanPaintPipeline::new(
            UnavailableExec {
                reason: "no CUDA device; pinned checkpoints not provisioned".to_string(),
            },
            test_config(),
        );
        let mut calls = 0u32;
        let err = pipe
            .generate(&test_inputs(&mesh), &mut |_| {
                calls += 1;
                true
            })
            .unwrap_err();
        assert!(matches!(err, PbrError::Unavailable(_)));
        assert_eq!(calls, 0, "no progress before availability check");
    }

    struct InconsistentUnavailableExec;

    impl PaintModelExec for InconsistentUnavailableExec {
        fn execution_kind(&self) -> PaintExecutionKind {
            PaintExecutionKind::Unavailable
        }

        fn availability(&self) -> ExecStatus {
            ExecStatus::Ready {
                device: "malicious-ready-claim".to_string(),
                vram_gb: 96.0,
            }
        }

        fn is_resident(&self) -> bool {
            false
        }

        fn warm(&mut self) -> Result<(), PbrError> {
            panic!("unavailable kind must fail before warm")
        }

        fn release(&mut self) {}

        fn run_multiview(
            &mut self,
            _cond: &PaintCondition,
            _progress: &mut dyn FnMut(u32, u32) -> bool,
        ) -> Result<MultiviewPbr, PbrError> {
            panic!("unavailable kind must fail before execution")
        }
    }

    #[test]
    fn unavailable_kind_cannot_override_gate_with_ready_status() {
        let mesh = TriMesh::unit_cube_atlas();
        let mut pipe = HunyuanPaintPipeline::new(InconsistentUnavailableExec, test_config());
        let mut progress_calls = 0;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pipe.generate(&test_inputs(&mesh), &mut |_| {
                progress_calls += 1;
                true
            })
        }));
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), Err(PbrError::Unavailable(_))));
        assert_eq!(progress_calls, 0);
    }

    struct NativeClaimExec;

    impl PaintModelExec for NativeClaimExec {
        fn execution_kind(&self) -> PaintExecutionKind {
            PaintExecutionKind::NativeHunyuan
        }

        fn availability(&self) -> ExecStatus {
            ExecStatus::Ready {
                device: "claim-only test".to_string(),
                vram_gb: 96.0,
            }
        }

        fn is_resident(&self) -> bool {
            false
        }

        fn warm(&mut self) -> Result<(), PbrError> {
            panic!("license gate must run before warm")
        }

        fn release(&mut self) {}

        fn run_multiview(
            &mut self,
            _cond: &PaintCondition,
            _progress: &mut dyn FnMut(u32, u32) -> bool,
        ) -> Result<MultiviewPbr, PbrError> {
            panic!("license gate must run before execution")
        }
    }

    #[test]
    fn claimed_real_executor_fails_closed_without_license_acknowledgement() {
        let mesh = TriMesh::unit_cube_atlas();
        let mut pipe = HunyuanPaintPipeline::new(NativeClaimExec, test_config());
        let mut calls = 0;
        let error = pipe
            .generate(&test_inputs(&mesh), &mut |_| {
                calls += 1;
                true
            })
            .unwrap_err();
        assert!(matches!(error, PbrError::Unavailable(message) if message.contains("license acknowledgement missing")));
        assert_eq!(calls, 0);
        assert!(!native_implementation_status().real_model_executor_available);
    }

    #[test]
    fn cancellation_mid_denoise() {
        let mesh = TriMesh::unit_cube_atlas();
        let mut pipe = HunyuanPaintPipeline::new(MockPaintExec::default(), test_config());
        let result = pipe.generate(&test_inputs(&mesh), &mut |p| {
            !(p.stage == PbrStage::Denoise && p.current == 3)
        });
        assert_eq!(result.unwrap_err(), PbrError::Cancelled);
    }

    #[test]
    fn residency_warms_once_across_jobs() {
        let mesh = TriMesh::unit_cube_atlas();
        let mut pipe = HunyuanPaintPipeline::new(MockPaintExec::default(), test_config());
        pipe.generate(&test_inputs(&mesh), &mut |_| true).unwrap();
        pipe.generate(&test_inputs(&mesh), &mut |_| true).unwrap();
        assert_eq!(pipe.exec.warm_calls, 1, "second job reuses residency");
        assert!(pipe.exec.is_resident());
        pipe.release();
        assert!(!pipe.exec.is_resident());
    }

    #[test]
    fn deterministic_output() {
        let a = run_ok();
        let b = run_ok();
        assert_eq!(a.manifest_json(), b.manifest_json());
        assert_eq!(a.albedo.map.as_ref().unwrap().data, b.albedo.map.as_ref().unwrap().data);
        assert_eq!(a.packed_orm.as_ref().unwrap().data, b.packed_orm.as_ref().unwrap().data);
    }

    #[test]
    fn stage_order_is_coherent() {
        let mesh = TriMesh::unit_cube_atlas();
        let mut pipe = HunyuanPaintPipeline::new(MockPaintExec::default(), test_config());
        let mut stages = Vec::new();
        pipe.generate(&test_inputs(&mesh), &mut |p| {
            stages.push(p.stage);
            true
        })
        .unwrap();
        let order = [
            PbrStage::Prepare,
            PbrStage::ViewSelect,
            PbrStage::GeometryRender,
            PbrStage::Encode,
            PbrStage::Denoise,
            PbrStage::Bake,
            PbrStage::Inpaint,
            PbrStage::Finalize,
        ];
        let mut last = 0;
        for stage in stages {
            let idx = order.iter().position(|s| *s == stage).unwrap();
            assert!(idx >= last, "stage {stage:?} out of order");
            last = idx;
        }
    }

    #[test]
    fn rejects_bad_inputs() {
        let mut no_uv = TriMesh::unit_cube_atlas();
        no_uv.uvs.clear();
        let mut pipe = HunyuanPaintPipeline::new(MockPaintExec::default(), test_config());
        assert!(matches!(
            pipe.generate(&test_inputs(&no_uv), &mut |_| true),
            Err(PbrError::InvalidParams(_))
        ));
        let mesh = TriMesh::unit_cube_atlas();
        let bad_ref = PaintInputs {
            reference_rgb: &REF_8X8[..10],
            ..test_inputs(&mesh)
        };
        assert!(matches!(
            pipe.generate(&bad_ref, &mut |_| true),
            Err(PbrError::InvalidParams(_))
        ));
    }

    #[test]
    fn hostile_mesh_and_config_fail_before_progress_without_panicking() {
        let mut bad_index = TriMesh::unit_cube_atlas();
        bad_index.indices[0][0] = u32::MAX;
        let mut pipe = HunyuanPaintPipeline::new(MockPaintExec::default(), test_config());
        let mut calls = 0;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pipe.generate(&test_inputs(&bad_index), &mut |_| {
                calls += 1;
                true
            })
        }));
        assert!(matches!(result, Ok(Err(PbrError::InvalidParams(_)))));
        assert_eq!(calls, 0);

        let mesh = TriMesh::unit_cube_atlas();
        for bad_scale in [
            0.0,
            f32::MIN_POSITIVE,
            MIN_ORTHO_SCALE * 0.5,
            f32::NAN,
            f32::INFINITY,
        ] {
            let mut config = test_config();
            config.ortho_scale = bad_scale;
            let mut pipe = HunyuanPaintPipeline::new(MockPaintExec::default(), config);
            let mut calls = 0;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                pipe.generate(&test_inputs(&mesh), &mut |_| {
                    calls += 1;
                    true
                })
            }));
            assert!(matches!(result, Ok(Err(PbrError::InvalidParams(_)))));
            assert_eq!(calls, 0);
        }

        let mut config = test_config();
        config.texture_size = u32::MAX;
        let mut pipe = HunyuanPaintPipeline::new(MockPaintExec::default(), config);
        let mut calls = 0;
        assert!(matches!(
            pipe.generate(&test_inputs(&mesh), &mut |_| {
                calls += 1;
                true
            }),
            Err(PbrError::InvalidParams(_))
        ));
        assert_eq!(calls, 0);
    }

    #[test]
    fn hostile_reference_and_ao_lengths_fail_before_progress() {
        let mesh = TriMesh::unit_cube_atlas();
        let mut inputs = test_inputs(&mesh);
        inputs.ref_width = u32::MAX;
        let mut pipe = HunyuanPaintPipeline::new(MockPaintExec::default(), test_config());
        let mut calls = 0;
        assert!(matches!(
            pipe.generate(&inputs, &mut |_| {
                calls += 1;
                true
            }),
            Err(PbrError::InvalidParams(_))
        ));
        assert_eq!(calls, 0);

        let malformed_ao = PbrMap {
            width: 48,
            height: 48,
            format: PixelFormat::Gray8,
            color_space: ColorSpace::Linear,
            data: vec![0; 3],
        };
        let mut inputs = test_inputs(&mesh);
        inputs.baked_ao = Some(&malformed_ao);
        let mut pipe = HunyuanPaintPipeline::new(MockPaintExec::default(), test_config());
        let mut calls = 0;
        assert!(matches!(
            pipe.generate(&inputs, &mut |_| {
                calls += 1;
                true
            }),
            Err(PbrError::InvalidParams(_))
        ));
        assert_eq!(calls, 0);
    }

    #[test]
    fn admission_profiles_are_unit_exact() {
        let std24 = admission_estimate(MemoryProfile::Standard24g);
        assert!(fits_24g_service(&std24), "standard profile must fit 4090 minus reserve");
        assert!(
            std24
                .declared_peak_mib
                .checked_add(SERVICE_VRAM_RESERVE_MIB)
                .is_some_and(|required| required <= RTX4090_TOTAL_MIB)
        );
        assert!(!std24.measured, "budget ceiling stays honest: unmeasured until canary");
        let high = admission_estimate(MemoryProfile::HighVram);
        assert!(
            !fits_24g_service(&high),
            "all-resident oracle worst case + reserve exceeds 24,564 MiB and must not admit on a 4090"
        );
        assert_eq!(high.declared_peak_mib, ORACLE_PEAK_MIB_UPPER);
        assert!(high.basis.contains("22.15"));
        assert!(ORACLE_PEAK_MIB_LOWER < ORACLE_PEAK_MIB_UPPER);

        let overflow = AdmissionEstimate {
            profile: MemoryProfile::Standard24g,
            declared_peak_mib: u32::MAX,
            measured: false,
            basis: "hostile overflow regression",
        };
        assert!(!fits_24g_service(&overflow));
    }
}
