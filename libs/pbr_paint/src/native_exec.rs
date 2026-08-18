//! Native Hunyuan3D-Paint-2.1 executor seam.
//!
//! VAE encode/decode is live and oracle-checked. The CPU DDIM/CFG loop is
//! assembled in [`crate::denoise`]. Isolated extras-on 2×2, extras-on
//! `down_blocks[i](...)`, `unet_dual` write-cache, and a 15-step DDIM
//! canary (step-0 v-pred 1.83e-4, final 1.35e-3) match official fp32.
//! `run_multiview` encodes views, writes the dual-stream cache, runs
//! native DINOv2-giant + DinoProj, and drives the 15-step extras-on DDIM
//! loop. CFG is one fused extras-on walk over `3 * n_pbr * n_views` packs.
//! A 512-view job has not been checked on the paint box yet, so
//! [`crate::pipeline::native_implementation_status`] stays false.

use crate::cond_assembly::{
    rope_levels_for_latent, voxel_xyz_for_views, LATENT_CHANNELS, PBR_MATERIALS,
};
use crate::denoise::{
    rgb8_interleaved_to_planar01, resize_rgb8_bilinear, DenoiseBatch, DEFAULT_STEPS,
};
use crate::dino_proj::DinoProj;
use crate::dino_vit::{self, DinoVit};
use crate::pipeline::{
    ExecStatus, MultiviewPbr, PaintCondition, PaintExecutionKind, PaintModelExec,
};
use crate::schedule::DdimVpredZsnr;
use crate::sd_vae::SdVae;
use crate::test_backend::PbrError;
use crate::unet_first::{pack_planar_host, unpack_planar_host, UnetFirst};
use crate::unet_forward::{
    cfg_ddim_gpu, pack_cfg_x12_gpu, walk_extras_on_resident, write_dual_cache, ExtrasJobCtx,
    VoxelLevel, VoxelPyramid,
};
use makepad_ggml::backend::cuda::{gpu_concat_cols, gpu_download, gpu_upload};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Default overlay used on the 4090 paint box. Fleet jobs must not rely on
/// this — they resolve `unet` / `vae` / `dino-conditioner` from the registry
/// cache via [`NativeHunyuanExec::at_bins`].
pub const DEFAULT_WEIGHTS_ROOT: &str =
    r"C:\ai\Hunyuan3D-2.1\weights\hunyuan3d-paintpbr-v2-1";

/// Explicit checkpoint files. The fleet cache layout is
/// `paint21/{unet,vae}/diffusion_pytorch_model.bin` plus
/// `paint21/dinov2-giant/model.safetensors` — not the Hunyuan git checkout.
#[derive(Clone, Debug)]
pub struct HunyuanBins {
    pub vae: PathBuf,
    pub unet: PathBuf,
    pub dino: PathBuf,
}

pub fn weights_root() -> PathBuf {
    if let Ok(p) = std::env::var("MAKEPAD_HUNYUAN_ROOT") {
        let p = PathBuf::from(p);
        let nested = p.join("weights").join("hunyuan3d-paintpbr-v2-1");
        if nested.is_dir() {
            return nested;
        }
        return p;
    }
    if let Ok(p) = std::env::var("MAKEPAD_HUNYUAN_WEIGHTS") {
        return PathBuf::from(p);
    }
    PathBuf::from(DEFAULT_WEIGHTS_ROOT)
}

pub fn vae_bin_path(root: &Path) -> PathBuf {
    root.join("vae").join("diffusion_pytorch_model.bin")
}

pub fn unet_bin_path(root: &Path) -> PathBuf {
    root.join("unet").join("diffusion_pytorch_model.bin")
}

pub fn dino_model_path() -> PathBuf {
    let p = dino_vit::default_snapshot_path();
    if p.is_file() {
        p
    } else {
        p.join("model.safetensors")
    }
}

pub fn bins_at_root(root: &Path) -> HunyuanBins {
    HunyuanBins {
        vae: vae_bin_path(root),
        unet: unet_bin_path(root),
        dino: dino_model_path(),
    }
}

/// Files required before we even attempt a warm.
pub fn required_bins_present(root: &Path) -> Result<(), String> {
    required_bins_present_at(&bins_at_root(root))
}

pub fn required_bins_present_at(bins: &HunyuanBins) -> Result<(), String> {
    for p in [&bins.vae, &bins.unet, &bins.dino] {
        if !p.is_file() {
            return Err(format!("missing {}", p.display()));
        }
    }
    Ok(())
}

/// Per-view VAE-encoded geometry latents plus the reference-image latent
/// (held for the dual-stream branch). All latents are scaled means.
pub struct EncodedViews {
    pub lat_w: usize,
    pub lat_h: usize,
    pub n_views: usize,
    pub normal_latents: Vec<Vec<f32>>,
    pub position_latents: Vec<Vec<f32>>,
    pub reference_latent: Vec<f32>,
    pub azims: Vec<f32>,
    /// ImageNet-normalized planar RGB the ViT will consume (224², 3-ch).
    pub dino_pixels: Vec<f32>,
}

pub struct NativeHunyuanExec {
    bins: HunyuanBins,
    vae: Option<SdVae>,
    unet: Option<UnetFirst>,
    dino: Option<DinoVit>,
    dino_proj: Option<DinoProj>,
}

impl NativeHunyuanExec {
    pub fn at_bins(bins: HunyuanBins) -> Result<Self, PbrError> {
        required_bins_present_at(&bins).map_err(PbrError::Unavailable)?;
        Ok(Self {
            bins,
            vae: None,
            unet: None,
            dino: None,
            dino_proj: None,
        })
    }

    pub fn at(root: PathBuf) -> Result<Self, PbrError> {
        Self::at_bins(bins_at_root(&root))
    }

    pub fn discover() -> Result<Self, PbrError> {
        Self::at(weights_root())
    }

    pub fn vae(&self) -> Option<&SdVae> {
        self.vae.as_ref()
    }

    pub fn unet(&self) -> Option<&UnetFirst> {
        self.unet.as_ref()
    }

    /// Encode planar RGB [0,1] to a scaled 4-ch latent mean.
    pub fn encode_view_rgb(
        &self,
        rgb01_planar: &[f32],
        width: usize,
        height: usize,
    ) -> Result<(Vec<f32>, usize, usize), PbrError> {
        let vae = self
            .vae
            .as_ref()
            .ok_or_else(|| PbrError::Unavailable("VAE not warm".into()))?;
        let p = vae
            .encode_mean(rgb01_planar, width, height)
            .map_err(PbrError::Internal)?;
        let data = makepad_ggml::backend::cuda::gpu_download(&p.t).map_err(PbrError::Internal)?;
        Ok((data, p.width, p.height))
    }

    fn encode_rgb8(
        &self,
        rgb: &[u8],
        width: u32,
        height: u32,
        target: u32,
    ) -> Result<(Vec<f32>, usize, usize), PbrError> {
        if target == 0 || target % 8 != 0 {
            return Err(PbrError::InvalidParams(format!(
                "VAE encode target {target} must be a positive multiple of 8"
            )));
        }
        let resized = resize_rgb8_bilinear(
            rgb,
            width as usize,
            height as usize,
            target as usize,
            target as usize,
        )?;
        let planar = rgb8_interleaved_to_planar01(&resized, target as usize, target as usize)?;
        self.encode_view_rgb(&planar, target as usize, target as usize)
    }

    /// Encode the reference image and every view's normal/position maps.
    pub fn encode_condition(&self, cond: &PaintCondition) -> Result<EncodedViews, PbrError> {
        if cond.views.is_empty() {
            return Err(PbrError::InvalidParams("no views to encode".into()));
        }
        let target = cond.resolution;
        let ref_resized = resize_rgb8_bilinear(
            cond.reference_rgb,
            cond.ref_width as usize,
            cond.ref_height as usize,
            target as usize,
            target as usize,
        )?;
        let dino_pixels = dino_vit::preprocess_official(&ref_resized, target as usize, target as usize)?;
        let (reference_latent, lat_w, lat_h) = self.encode_rgb8(
            &ref_resized,
            target,
            target,
            target,
        )?;
        let expect = LATENT_CHANNELS * lat_w * lat_h;
        if reference_latent.len() != expect {
            return Err(PbrError::Internal(format!(
                "reference latent {} != {expect}",
                reference_latent.len()
            )));
        }
        let mut normal_latents = Vec::with_capacity(cond.views.len());
        let mut position_latents = Vec::with_capacity(cond.views.len());
        let mut azims = Vec::with_capacity(cond.views.len());
        for (i, view) in cond.views.iter().enumerate() {
            let (n, nw, nh) = self.encode_rgb8(
                &view.normal_map_rgb,
                view.size,
                view.size,
                target,
            )?;
            let (p, pw, ph) = self.encode_rgb8(
                &view.position_map_rgb,
                view.size,
                view.size,
                target,
            )?;
            if nw != lat_w || nh != lat_h || pw != lat_w || ph != lat_h {
                return Err(PbrError::Internal(format!(
                    "view {i} latent {nw}x{nh}/{pw}x{ph} != {lat_w}x{lat_h}"
                )));
            }
            if n.len() != expect || p.len() != expect {
                return Err(PbrError::Internal(format!("view {i} latent length")));
            }
            normal_latents.push(n);
            position_latents.push(p);
            azims.push(view.azim);
        }
        Ok(EncodedViews {
            lat_w,
            lat_h,
            n_views: cond.views.len(),
            normal_latents,
            position_latents,
            reference_latent,
            azims,
            dino_pixels,
        })
    }

    /// Build the official 15-step batch from encoded views. The UNet callback
    /// is not attached; callers must not treat this as a complete generate.
    pub fn prepare_denoise(
        &self,
        cond: &PaintCondition,
        encoded: &EncodedViews,
    ) -> Result<(DdimVpredZsnr, DenoiseBatch), PbrError> {
        let sched = DdimVpredZsnr::hunyuan_paint();
        let batch = DenoiseBatch::from_defaults(
            cond.seed,
            &encoded.azims,
            encoded.lat_w,
            encoded.lat_h,
            &sched,
        )?;
        // Touch packing so a layout mistake fails here, not after the graph lands.
        let _packed = batch.pack_cfg_inputs(&encoded.normal_latents, &encoded.position_latents)?;
        if batch.timesteps.len() != DEFAULT_STEPS {
            return Err(PbrError::Internal(format!(
                "expected {DEFAULT_STEPS} DDIM steps, got {}",
                batch.timesteps.len()
            )));
        }
        Ok((sched, batch))
    }
}

impl PaintModelExec for NativeHunyuanExec {
    fn execution_kind(&self) -> PaintExecutionKind {
        PaintExecutionKind::NativeHunyuan
    }

    fn availability(&self) -> ExecStatus {
        if let Err(detail) = required_bins_present_at(&self.bins) {
            return ExecStatus::MissingCheckpoints { detail };
        }
        if !makepad_ggml::backend::cuda::gpu_device_available() {
            return ExecStatus::NoCuda {
                detail: "CUDA device/runtime unavailable".into(),
            };
        }
        ExecStatus::Ready {
            device: "cuda".into(),
            vram_gb: 0.0,
        }
    }

    fn is_resident(&self) -> bool {
        self.vae.is_some() && self.unet.is_some() && self.dino.is_some() && self.dino_proj.is_some()
    }

    fn warm(&mut self) -> Result<(), PbrError> {
        match self.availability() {
            ExecStatus::Ready { .. } => {}
            ExecStatus::MissingCheckpoints { detail } | ExecStatus::NoCuda { detail } => {
                return Err(PbrError::Unavailable(detail));
            }
        }
        if self.vae.is_none() {
            let vae = SdVae::load(&self.bins.vae).map_err(PbrError::Internal)?;
            self.vae = Some(vae);
        }
        if self.unet.is_none() {
            let unet = UnetFirst::load(&self.bins.unet).map_err(PbrError::Internal)?;
            self.unet = Some(unet);
        }
        if self.dino.is_none() {
            let dino = DinoVit::load(&self.bins.dino).map_err(PbrError::Internal)?;
            self.dino = Some(dino);
        }
        if self.dino_proj.is_none() {
            let proj = DinoProj::load_from_unet_bin(&self.bins.unet)
                .map_err(PbrError::Internal)?;
            self.dino_proj = Some(proj);
        }
        Ok(())
    }

    fn release(&mut self) {
        self.vae = None;
        self.unet = None;
        self.dino = None;
        self.dino_proj = None;
    }

    fn run_multiview(
        &mut self,
        cond: &PaintCondition,
        progress: &mut dyn FnMut(u32, u32) -> bool,
    ) -> Result<MultiviewPbr, PbrError> {
        self.warm()?;
        // Job-scoped K/V caches on a warm-kept UNet are keyed by layer name
        // only — clear them so this job cannot see the previous job's tokens.
        if let Some(unet) = self.unet.as_ref() {
            unet.begin_job();
        }
        let encoded = self.encode_condition(cond)?;
        let (sched, mut batch) = self.prepare_denoise(cond, &encoded)?;
        let unet = self
            .unet
            .as_ref()
            .ok_or_else(|| PbrError::Unavailable("UNet not warm".into()))?;
        let dino = self
            .dino
            .as_ref()
            .ok_or_else(|| PbrError::Unavailable("DINO not warm".into()))?;
        let dino_proj = self
            .dino_proj
            .as_ref()
            .ok_or_else(|| PbrError::Unavailable("DINO projector not warm".into()))?;
        let vae = self
            .vae
            .as_ref()
            .ok_or_else(|| PbrError::Unavailable("VAE not warm".into()))?;
        let cache = write_dual_cache(
            unet,
            &[encoded.reference_latent.as_slice()],
            encoded.lat_w,
            encoded.lat_h,
        )
        .map_err(PbrError::Internal)?;
        if cache.len() != crate::dual_stream::write_layer_names().len() {
            return Err(PbrError::Internal(format!(
                "dual write-cache has {} layers, expected 16",
                cache.len()
            )));
        }
        let size = cond.resolution as usize;
        let mut pos_maps = Vec::with_capacity(cond.views.len());
        for view in cond.views {
            pos_maps.push(resize_rgb8_bilinear(
                &view.position_map_rgb,
                view.size as usize,
                view.size as usize,
                size,
                size,
            )?);
        }
        let pos_refs: Vec<&[u8]> = pos_maps.iter().map(|m| m.as_slice()).collect();
        let levels = rope_levels_for_latent(encoded.lat_w);
        let full = voxel_xyz_for_views(&pos_refs, size, levels[0].0, levels[0].1)?;
        let half = voxel_xyz_for_views(&pos_refs, size, levels[1].0, levels[1].1)?;
        let quarter = voxel_xyz_for_views(&pos_refs, size, levels[2].0, levels[2].1)?;
        let eighth = voxel_xyz_for_views(&pos_refs, size, levels[3].0, levels[3].1)?;
        let voxels = VoxelPyramid {
            full: VoxelLevel {
                xyz: &full,
                res: levels[0].1,
            },
            half: VoxelLevel {
                xyz: &half,
                res: levels[1].1,
            },
            quarter: VoxelLevel {
                xyz: &quarter,
                res: levels[2].1,
            },
            eighth: VoxelLevel {
                xyz: &eighth,
                res: levels[3].1,
            },
        };
        let hidden = dino
            .forward(&encoded.dino_pixels)
            .map_err(PbrError::Internal)?;
        let rows = hidden.len() / crate::dino_proj::DINO_DIM;
        let dino_tok = dino_proj
            .forward(&hidden, rows)
            .map_err(PbrError::Internal)?;
        let enc_alb = unet.learned_text_clip_albedo().map_err(PbrError::Internal)?;
        let enc_mr = unet.learned_text_clip_mr().map_err(PbrError::Internal)?;
        let n_views = encoded.n_views;
        let row = batch.row_len();
        let hw = encoded.lat_w * encoded.lat_h;
        let ts = batch.timesteps.clone();
        let total = ts.len() as u32;
        let ctx = ExtrasJobCtx::upload(&enc_alb, &enc_mr, &dino_tok, &cache, &voxels, 3)
            .map_err(PbrError::Internal)?;
        let n_rows = PBR_MATERIALS * n_views;
        let sample_refs: Vec<&[f32]> = (0..n_rows)
            .map(|i| &batch.sample[i * row..(i + 1) * row])
            .collect();
        let mut sample = gpu_upload(
            &pack_planar_host(&sample_refs, 4, hw).map_err(PbrError::Internal)?,
            4,
            n_rows * hw,
        )
        .map_err(PbrError::Internal)?;
        let n_refs: Vec<&[f32]> = encoded.normal_latents.iter().map(|v| v.as_slice()).collect();
        let p_refs: Vec<&[f32]> = encoded.position_latents.iter().map(|v| v.as_slice()).collect();
        let normals = gpu_upload(
            &pack_planar_host(&n_refs, 4, hw).map_err(PbrError::Internal)?,
            4,
            n_views * hw,
        )
        .map_err(PbrError::Internal)?;
        let positions = gpu_upload(
            &pack_planar_host(&p_refs, 4, hw).map_err(PbrError::Internal)?,
            4,
            n_views * hw,
        )
        .map_err(PbrError::Internal)?;
        let normals_pbr = gpu_concat_cols(&[&normals, &normals]).map_err(PbrError::Internal)?;
        let positions_pbr = gpu_concat_cols(&[&positions, &positions]).map_err(PbrError::Internal)?;
        let mut scale_host = vec![0.0f32; 4 * n_rows * hw];
        for (i, vs) in batch.view_scales.iter().enumerate() {
            let a = batch.guidance * *vs;
            for c in 0..4 {
                let dst = c * n_rows * hw + i * hw;
                for k in 0..hw {
                    scale_host[dst + k] = a;
                }
            }
        }
        let scale_t = gpu_upload(&scale_host, 4, n_rows * hw).map_err(PbrError::Internal)?;
        let ref_scales = [0.0f32, 1.0, 1.0];
        // Official times denoise after one discarded UNet; keep the same split.
        if let Some(&t0) = ts.first() {
            let x12 = pack_cfg_x12_gpu(&sample, &normals_pbr, &positions_pbr)
                .map_err(PbrError::Internal)?;
            let xs = unet
                .conv_in_stacked(&x12, 3 * n_rows, encoded.lat_w, encoded.lat_h)
                .map_err(PbrError::Internal)?;
            let temb = unet
                .timestep_embedding(t0 as f32)
                .map_err(PbrError::Internal)?;
            let temb_act = unet.silu_temb(&temb).map_err(PbrError::Internal)?;
            let _ = walk_extras_on_resident(unet, xs, &temb_act, &ctx, n_views, &ref_scales)
                .map_err(PbrError::Internal)?;
        }
        let denoise_t0 = Instant::now();
        for (i, &t) in ts.iter().enumerate() {
            if !progress(i as u32 + 1, total) {
                return Err(PbrError::Cancelled);
            }
            let x12 = pack_cfg_x12_gpu(&sample, &normals_pbr, &positions_pbr)
                .map_err(PbrError::Internal)?;
            let xs = unet
                .conv_in_stacked(&x12, 3 * n_rows, encoded.lat_w, encoded.lat_h)
                .map_err(PbrError::Internal)?;
            let temb = unet
                .timestep_embedding(t as f32)
                .map_err(PbrError::Internal)?;
            let temb_act = unet.silu_temb(&temb).map_err(PbrError::Internal)?;
            let head = walk_extras_on_resident(unet, xs, &temb_act, &ctx, n_views, &ref_scales)
                .map_err(PbrError::Internal)?;
            let (c1, c2) = sched.ddim_linear_coeffs(t, batch.steps);
            sample = cfg_ddim_gpu(&sample, &head.t, &scale_t, c1, c2).map_err(PbrError::Internal)?;
        }
        // Issue time only — GPU work may still be queued on WDDM.
        println!("PBR_EXEC_DENOISE_ISSUE_S {:.3}", denoise_t0.elapsed().as_secs_f64());
        let dl_t0 = Instant::now();
        let sample_host = gpu_download(&sample).map_err(PbrError::Internal)?;
        println!("PBR_EXEC_DOWNLOAD_S {:.3}", dl_t0.elapsed().as_secs_f64());
        // Sync-closed: includes the drain that used to hide in gpu_download.
        println!("PBR_EXEC_DENOISE_S {:.3}", denoise_t0.elapsed().as_secs_f64());
        let parts = unpack_planar_host(&sample_host, 4, n_rows, hw)
            .map_err(PbrError::Internal)?;
        batch.sample = parts.into_iter().flatten().collect();
        if std::env::var_os("MAKEPAD_GPU_PROF").is_some() {
            let perf = makepad_ggml::backend::cuda::gpu_perf_stats(false);
            eprint!(
                "{}",
                makepad_ggml::backend::prof::report_and_reset("PBR_GPU_PROF ")
            );
            eprintln!(
                "PBR_GPU_PERF evict={} stream={} stream_mb={:.1} pool_fresh={} pool_fresh_mb={:.1} oom_clears={} free_mb={:.0}",
                perf.weight_evict_events,
                perf.weight_stream_count,
                perf.weight_stream_bytes as f64 / 1.0e6,
                perf.pool_fresh_alloc_count,
                perf.pool_fresh_alloc_bytes as f64 / 1.0e6,
                perf.pool_oom_clears,
                perf.mem_free_bytes as f64 / 1.0e6
            );
        }
        let (alb_lat, mr_lat) = batch.split_materials();
        let mut albedo = Vec::with_capacity(n_views);
        let mut mr = Vec::with_capacity(n_views);
        let mut out_w = 0usize;
        let mut out_h = 0usize;
        for lat in alb_lat {
            let (planar, w, h) = vae
                .decode_rgb01(&lat, encoded.lat_w, encoded.lat_h)
                .map_err(PbrError::Internal)?;
            out_w = w;
            out_h = h;
            albedo.push(planar_rgb_to_interleaved(&planar, w, h));
        }
        for lat in mr_lat {
            let (planar, w, h) = vae
                .decode_rgb01(&lat, encoded.lat_w, encoded.lat_h)
                .map_err(PbrError::Internal)?;
            mr.push(planar_rgb_to_interleaved(&planar, w, h));
        }
        if out_w != out_h {
            return Err(PbrError::Internal(format!(
                "VAE decode is not square ({out_w}x{out_h})"
            )));
        }
        Ok(MultiviewPbr {
            size: out_w as u32,
            albedo,
            mr,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_weights_are_a_precise_unavailable() {
        let err = NativeHunyuanExec::at(PathBuf::from("/no/such/hunyuan/weights")).unwrap_err();
        match err {
            PbrError::Unavailable(m) => assert!(m.contains("missing"), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn at_bins_reports_the_given_vae_path() {
        let missing = PathBuf::from("/cache/paint21/vae/diffusion_pytorch_model.bin");
        let err = NativeHunyuanExec::at_bins(HunyuanBins {
            vae: missing.clone(),
            unet: PathBuf::from("/cache/paint21/unet/diffusion_pytorch_model.bin"),
            dino: PathBuf::from("/cache/paint21/dinov2-giant/model.safetensors"),
        })
        .unwrap_err();
        match err {
            PbrError::Unavailable(m) => {
                assert!(m.contains("missing"), "{m}");
                assert!(m.contains(missing.to_string_lossy().as_ref()), "{m}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn dino_model_path_is_safetensors() {
        let p = dino_model_path();
        assert!(
            p.ends_with("model.safetensors") || p.extension().is_some_and(|e| e == "safetensors"),
            "{}",
            p.display()
        );
    }
}

fn planar_rgb_to_interleaved(planar: &[f32], width: usize, height: usize) -> Vec<f32> {
    let plane = width * height;
    let mut out = vec![0.0f32; 3 * plane];
    for i in 0..plane {
        out[i * 3] = planar[i];
        out[i * 3 + 1] = planar[plane + i];
        out[i * 3 + 2] = planar[2 * plane + i];
    }
    out
}
