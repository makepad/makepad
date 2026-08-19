//! `TripoSplatPipeline.run` — conditioning, flow sampling, decode, PLY.
//!
//! Background removal is the caller's job (the service runs the native
//! BiRefNet stage so it shares the CUDA worker); everything after "an RGBA
//! image with a meaningful matte" lives here.

use crate::splat::{
    check_cancel, resolve_num_gaussians, splat_cfg_combine, splat_cfg_enabled, splat_euler_step,
    splat_t_sequence, SplatCancel, SplatWeights, DEFAULT_ERODE_RADIUS, DEFAULT_GUIDANCE,
    DEFAULT_SEED, DEFAULT_SHIFT, DEFAULT_STEPS, FLOW_CAM_CHANNELS, FLOW_COND2_CHANNELS,
    FLOW_COND_CHANNELS, FLOW_IN_CHANNELS, FLOW_Q_TOKENS, GAUSSIANS_DEFAULT, GS_OUT_CHANNELS,
    GS_PER_POINT, SPLAT_CANVAS, VAE_BN_EPS, VAE_PACKED_CHANNELS, VAE_ZERO_PREFIX,
};
use crate::splat_decoder::{SplatGaussianDecoder, SplatOctree};
use crate::splat_dino::SplatDino;
use crate::splat_flow::SplatFlow;
use crate::splat_image::{preprocess, to_planar_f32, SplatImage};
use crate::splat_ops::{Device, Ten};
use crate::splat_ply::{build_anchor_splats, write_ply, PlySplat, DEFAULT_TRANSFORM};
use crate::splat_rand::SplatRng;
use crate::{band_progress, emit_progress, hook_ref, DiffusionError, ProgressHook, Result};
use makepad_ai_flux::flux2::{flux2_pack_latents, Flux2PackedLatents};
use makepad_ai_flux::flux2_vae::{flux2_vae_encode_moments, Flux2VaeImage, Flux2VaeWeights};

/// One generation request, already resolved against the model's limits.
#[derive(Clone, Copy, Debug)]
pub struct SplatParams {
    pub seed: u64,
    pub steps: usize,
    pub guidance_scale: f32,
    pub shift: f32,
    pub num_gaussians: usize,
    pub erode_radius: usize,
}

impl Default for SplatParams {
    fn default() -> Self {
        Self {
            seed: DEFAULT_SEED,
            steps: DEFAULT_STEPS,
            guidance_scale: DEFAULT_GUIDANCE,
            shift: DEFAULT_SHIFT,
            num_gaussians: GAUSSIANS_DEFAULT,
            erode_radius: DEFAULT_ERODE_RADIUS,
        }
    }
}

impl SplatParams {
    /// Clamp every knob into the band the reference documents.
    pub fn resolved(mut self) -> Self {
        self.steps = self.steps.clamp(1, 200);
        self.guidance_scale = self.guidance_scale.clamp(0.0, 20.0);
        self.shift = if self.shift.is_finite() && self.shift > 0.0 {
            self.shift.clamp(0.1, 10.0)
        } else {
            DEFAULT_SHIFT
        };
        self.num_gaussians = resolve_num_gaussians(self.num_gaussians);
        self.erode_radius = self.erode_radius.min(8);
        self
    }
}

/// The prepared model set. Not `Send` in practice — the device tensors are
/// thread-local by design, so this lives on whichever worker thread built it.
pub struct SplatPipeline {
    device: Device,
    pub dino: SplatDino,
    pub flow: SplatFlow,
    pub octree: SplatOctree,
    pub gaussians: SplatGaussianDecoder,
    pub vae: Flux2VaeWeights,
}

impl SplatPipeline {
    pub fn prepare(
        device: Device,
        dino_path: &std::path::Path,
        flow_path: &std::path::Path,
        decoder_path: &std::path::Path,
        vae_path: &std::path::Path,
        mut progress: Option<ProgressHook>,
    ) -> Result<Self> {
        let mut sub = band_progress(&mut progress, 0.0, 0.45);
        let dino = SplatDino::prepare(device, &SplatWeights::load(dino_path)?, hook_ref(&mut sub))?;
        drop(sub);
        let mut sub = band_progress(&mut progress, 0.45, 0.4);
        let flow = SplatFlow::prepare(device, &SplatWeights::load(flow_path)?, hook_ref(&mut sub))?;
        drop(sub);
        emit_progress(&mut progress, "load splat decoder", 0.85)?;
        let decoder_weights = SplatWeights::load(decoder_path)?;
        let octree = SplatOctree::prepare(device, &decoder_weights)?;
        let gaussians = SplatGaussianDecoder::prepare(device, &decoder_weights)?;
        emit_progress(&mut progress, "load flux2 vae", 0.95)?;
        let vae = Flux2VaeWeights::load(vae_path)?;
        emit_progress(&mut progress, "models ready", 1.0)?;
        Ok(Self {
            device,
            dino,
            flow,
            octree,
            gaussians,
            vae,
        })
    }

    pub fn device(&self) -> Device {
        self.device
    }

    /// `encode_image`: DINOv3 tokens (`feature1`) and the stochastic FLUX.2
    /// VAE latent tokens (`feature2`), both 4101 rows long.
    pub fn encode_image(&self, canvas: &SplatImage, rng: &mut SplatRng) -> Result<SplatCondition> {
        if canvas.channels != 3 || canvas.width != SPLAT_CANVAS || canvas.height != SPLAT_CANVAS {
            return Err(DiffusionError::workflow(
                "splat conditioning needs the 1024x1024 RGB canvas",
            ));
        }
        let planar = to_planar_f32(canvas)?;
        let feature1 = self.dino.forward_rgb(&planar, SPLAT_CANVAS)?;
        let feature2 = self.encode_vae(&planar, rng)?;
        Ok(SplatCondition { feature1, feature2 })
    }

    /// `Flux2VAEEncoder.encode(x*2 - 1, deterministic=False)` followed by the
    /// 5 zero rows that align `feature2` with `feature1`.
    ///
    /// The BatchNorm here is TripoSplat's own `nn.BatchNorm1d(128, eps=1e-5)`
    /// loaded with the released running stats, NOT the FLUX.2 pipeline's
    /// `eps=1e-4`, so the normalization is done locally instead of through
    /// `flux2_vae_bn_normalize`.
    fn encode_vae(&self, planar: &[f32], rng: &mut SplatRng) -> Result<Vec<f32>> {
        let image = Flux2VaeImage {
            width: SPLAT_CANVAS,
            height: SPLAT_CANVAS,
            data: planar.iter().map(|v| v * 2.0 - 1.0).collect(),
        };
        let (moments, width, height) = flux2_vae_encode_moments(&self.vae, &image)?;
        let plane = width * height;
        let z = moments.len() / (2 * plane);
        let mut latents = vec![0.0f32; z * plane];
        for i in 0..z * plane {
            let logvar = moments[z * plane + i];
            latents[i] = moments[i] + (0.5 * logvar).exp() * rng.normal();
        }
        let mut packed = flux2_pack_latents(&latents, width, height, z)?;
        bn_normalize(&mut packed, self.vae.bn_mean(), self.vae.bn_var());
        let tokens = packed.to_tokens();
        let mut out = vec![0.0f32; VAE_ZERO_PREFIX * VAE_PACKED_CHANNELS];
        out.extend_from_slice(&tokens);
        Ok(out)
    }

    /// `sample_latent`: Euler flow matching with the shift schedule and
    /// diffusers-convention CFG. Returns the `(8192, 16)` splat latent.
    pub fn sample_latent(
        &self,
        condition: &SplatCondition,
        params: &SplatParams,
        rng: &mut SplatRng,
        mut progress: Option<ProgressHook>,
        cancel: Option<SplatCancel<'_>>,
    ) -> Result<Vec<f32>> {
        let tokens = FLOW_Q_TOKENS;
        let mut latent = rng.normal_vec(tokens * FLOW_IN_CHANNELS);
        let mut camera = rng.normal_vec(FLOW_CAM_CHANNELS);

        let positive = self
            .flow
            .encode_context(&condition.feature1, &condition.feature2)?;
        let cfg = splat_cfg_enabled(params.guidance_scale);
        let negative = if cfg {
            Some(self.flow.encode_context(
                &vec![0.0f32; condition.feature1.len()],
                &vec![0.0f32; condition.feature2.len()],
            )?)
        } else {
            None
        };

        let schedule = splat_t_sequence(params.steps, params.shift as f64);
        for step in 0..params.steps {
            check_cancel(cancel)?;
            emit_progress(
                &mut progress,
                &format!("denoise {}/{}", step + 1, params.steps),
                step as f64 / params.steps as f64,
            )?;
            let (t, t_prev) = (schedule[step], schedule[step + 1]);
            let t1000 = (1000.0 * t) as f32;
            let mut velocity = self.flow.forward(&latent, &camera, t1000, &positive)?;
            if let Some(negative) = &negative {
                let uncond = self.flow.forward(&latent, &camera, t1000, negative)?;
                splat_cfg_combine(&mut velocity.latent, &uncond.latent, params.guidance_scale);
                splat_cfg_combine(&mut velocity.camera, &uncond.camera, params.guidance_scale);
            }
            splat_euler_step(&mut latent, &velocity.latent, t, t_prev)?;
            splat_euler_step(&mut camera, &velocity.camera, t, t_prev)?;
        }
        emit_progress(&mut progress, "denoise complete", 1.0)?;
        Ok(latent)
    }

    /// `OctreeGaussianDecoder.decode` + `_build_gaussians` + `save_ply`.
    pub fn decode_to_ply(
        &self,
        latent: &[f32],
        num_gaussians: usize,
        rng: &mut SplatRng,
        mut progress: Option<ProgressHook>,
        cancel: Option<SplatCancel<'_>>,
    ) -> Result<Vec<u8>> {
        let anchors_wanted = (num_gaussians / GS_PER_POINT).max(1);
        let tokens = latent.len() / crate::splat::OCT_COND_CHANNELS;
        let latent = Ten::upload(
            self.device,
            latent,
            tokens,
            crate::splat::OCT_COND_CHANNELS,
        )?;

        let mut sub = band_progress(&mut progress, 0.0, 0.35);
        let anchors = self
            .octree
            .sample(&latent, anchors_wanted, rng, hook_ref(&mut sub), cancel)?;
        drop(sub);
        check_cancel(cancel)?;

        let mut sub = band_progress(&mut progress, 0.35, 0.6);
        let features = self
            .gaussians
            .forward(&anchors, &latent, hook_ref(&mut sub), cancel)?;
        drop(sub);
        drop(latent);
        check_cancel(cancel)?;

        emit_progress(&mut progress, "write ply", 0.96)?;
        let mut splats: Vec<PlySplat> = Vec::with_capacity(anchors_wanted * GS_PER_POINT);
        for anchor in 0..anchors_wanted {
            build_anchor_splats(
                &features[anchor * GS_OUT_CHANNELS..(anchor + 1) * GS_OUT_CHANNELS],
                [
                    anchors[anchor * 3],
                    anchors[anchor * 3 + 1],
                    anchors[anchor * 3 + 2],
                ],
                self.gaussians.perturbation(),
                self.gaussians.base_offset_scale(),
                &DEFAULT_TRANSFORM,
                &mut splats,
            )?;
        }
        emit_progress(&mut progress, "decode complete", 1.0)?;
        Ok(write_ply(&splats))
    }

    /// The whole run: preprocessed image in, PLY bytes out.
    pub fn run(
        &self,
        image: &SplatImage,
        params: &SplatParams,
        mut progress: Option<ProgressHook>,
        cancel: Option<SplatCancel<'_>>,
    ) -> Result<Vec<u8>> {
        let params = params.resolved();
        let mut rng = SplatRng::new(params.seed);
        emit_progress(&mut progress, "preprocess", 0.0)?;
        let canvas = preprocess(image, params.erode_radius)?;
        check_cancel(cancel)?;
        emit_progress(&mut progress, "condition", 0.05)?;
        let condition = self.encode_image(&canvas, &mut rng)?;
        check_cancel(cancel)?;

        let mut sub = band_progress(&mut progress, 0.15, 0.65);
        let latent = self.sample_latent(&condition, &params, &mut rng, hook_ref(&mut sub), cancel)?;
        drop(sub);

        let mut sub = band_progress(&mut progress, 0.8, 0.2);
        let ply = self.decode_to_ply(
            &latent,
            params.num_gaussians,
            &mut rng,
            hook_ref(&mut sub),
            cancel,
        )?;
        drop(sub);
        emit_progress(&mut progress, "done", 1.0)?;
        Ok(ply)
    }
}

/// `encode_image`'s output.
pub struct SplatCondition {
    /// `(4101, 1280)`.
    pub feature1: Vec<f32>,
    /// `(4101, 128)`.
    pub feature2: Vec<f32>,
}

impl SplatCondition {
    pub fn token_count(&self) -> usize {
        self.feature1.len() / FLOW_COND_CHANNELS
    }

    pub fn is_aligned(&self) -> bool {
        self.feature1.len() % FLOW_COND_CHANNELS == 0
            && self.feature2.len() % FLOW_COND2_CHANNELS == 0
            && self.feature1.len() / FLOW_COND_CHANNELS
                == self.feature2.len() / FLOW_COND2_CHANNELS
    }
}

/// `(x - running_mean) / sqrt(running_var + eps)` per packed channel.
fn bn_normalize(packed: &mut Flux2PackedLatents, mean: &[f32], var: &[f32]) {
    let plane = packed.token_count();
    for channel in 0..packed.channels {
        let m = mean[channel];
        let std = (var[channel] + VAE_BN_EPS).sqrt();
        let start = channel * plane;
        for value in &mut packed.data[start..start + plane] {
            *value = (*value - m) / std;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_resolve_into_the_documented_bands() {
        let params = SplatParams {
            steps: 0,
            guidance_scale: -3.0,
            shift: 0.0,
            num_gaussians: 1,
            erode_radius: 99,
            seed: 7,
        }
        .resolved();
        assert_eq!(params.steps, 1);
        assert_eq!(params.guidance_scale, 0.0);
        assert_eq!(params.shift, DEFAULT_SHIFT);
        assert_eq!(params.num_gaussians, crate::splat::GAUSSIANS_MIN);
        assert_eq!(params.erode_radius, 8);
        assert_eq!(params.seed, 7);
        // Defaults are the reference's recommended run.
        let default = SplatParams::default().resolved();
        assert_eq!(default.steps, 20);
        assert_eq!(default.guidance_scale, 3.0);
        assert_eq!(default.shift, 3.0);
        assert_eq!(default.num_gaussians, 262_144);
    }

    #[test]
    fn bn_normalize_uses_the_reference_epsilon() {
        let mut packed = Flux2PackedLatents {
            width: 1,
            height: 1,
            channels: 2,
            data: vec![1.0, 3.0],
        };
        bn_normalize(&mut packed, &[1.0, 1.0], &[0.0, 3.0]);
        // channel 0: (1-1)/sqrt(0 + 1e-5) = 0
        assert_eq!(packed.data[0], 0.0);
        // channel 1: (3-1)/sqrt(3 + 1e-5)
        assert!((packed.data[1] - 2.0 / (3.0f32 + VAE_BN_EPS).sqrt()).abs() < 1e-6);
    }

    #[test]
    fn condition_alignment_contract() {
        let condition = SplatCondition {
            feature1: vec![0.0; 4101 * FLOW_COND_CHANNELS],
            feature2: vec![0.0; 4101 * FLOW_COND2_CHANNELS],
        };
        assert!(condition.is_aligned());
        assert_eq!(condition.token_count(), 4101);
        let skewed = SplatCondition {
            feature1: vec![0.0; 4101 * FLOW_COND_CHANNELS],
            feature2: vec![0.0; 4096 * FLOW_COND2_CHANNELS],
        };
        assert!(!skewed.is_aligned());
    }

    #[test]
    fn anchor_count_follows_the_gaussian_budget() {
        for wanted in [32_768usize, 65_536, 131_072, 262_144] {
            assert_eq!(wanted / GS_PER_POINT * GS_PER_POINT, wanted);
            assert_eq!((wanted / GS_PER_POINT) * GS_PER_POINT, wanted);
        }
        assert_eq!(262_144 / GS_PER_POINT, 8192);
    }
}
