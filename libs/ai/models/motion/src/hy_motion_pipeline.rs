//! Full native prompt-to-motion runtime for HY-Motion 1.0.
//!
//! The API is split deliberately:
//! - [`HyMotionPromptEncoder`] owns Qwen3-8B + CLIP-L conditioning;
//! - [`HyMotionSampler`] owns the full-f32 1.04B DiT + exact wooden decoder;
//! - [`HyMotionPipeline`] keeps both resident for the lowest repeat latency.
//!
//! Memory-constrained callers can run the two stages separately, call
//! [`HyMotionPromptEncoder::evict_device_weights`], drop the encoder, and
//! only then load the sampler. High-throughput workers should keep the
//! combined pipeline alive: changing prompts then reuse Qwen's CUDA cache,
//! CLIP's host checkpoint, and the sampler weights without compromising the
//! accepted precision contracts.
//!
//! # Service handoff
//!
//! 1. Construct [`HyMotionModelPaths`] and call [`HyMotionPipeline::load`]
//!    once on the dedicated GPU worker thread.
//! 2. For each request, fill [`HyMotionGenerateParams`] and call
//!    [`HyMotionPipeline::generate_with_control`]. Keep the pipeline alive;
//!    rebuilding it throws away the accepted 1.59-second warm path.
//! 3. A native retarget/export consumer reads
//!    [`HyMotionPipeline::skeleton`],
//!    [`HyMotionDecoded::local_rotation_matrices`] and
//!    [`HyMotionDecoded::translations`]. Rotations are local and already
//!    include the Pelvis/root orientation at joint 0. Translation is a
//!    separate mesh track: apply each exactly once.
//! 4. [`HyMotionDecoded::keypoints_3d`] reproduces the reference diagnostic
//!    WoodenMesh keypoints. It is not a replacement for the local-matrix +
//!    translation animation tracks.

use std::path::{Path, PathBuf};
use std::time::Instant;

use makepad_ai_h3::h3::H3ShardedWeights;
use crate::hy_motion::{
    HY_MOTION_CFG, HY_MOTION_CONTEXT_DIM, HY_MOTION_INPUT_DIM,
    HY_MOTION_MAX_FRAMES, HY_MOTION_MIN_FRAMES, HY_MOTION_STEPS,
    HY_MOTION_VECTOR_DIM,
};
use crate::hy_motion_clip::HyMotionClipConditioner;
use crate::hy_motion_decode::{
    hy_motion_denormalize, HyMotionDecoded, HyMotionSkeleton,
    HyMotionWoodenModel,
};
use crate::hy_motion_text::{
    hy_motion_qwen_encode_controlled, hy_motion_qwen_evict,
    HyMotionQwenPrepared, HyMotionQwenTokenizer,
};
use crate::hy_motion_transformer::HyMotionDeviceWeights;
use crate::hy_motion_weights::HyMotionCheckpoint;
use crate::{DiffusionError, Result};

/// All official assets required by the full prompt-to-motion runtime.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HyMotionModelPaths {
    pub qwen_dir: PathBuf,
    pub clip: PathBuf,
    pub checkpoint: PathBuf,
    pub wooden_dir: PathBuf,
}

impl HyMotionModelPaths {
    pub fn new(
        qwen_dir: impl Into<PathBuf>,
        clip: impl Into<PathBuf>,
        checkpoint: impl Into<PathBuf>,
        wooden_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            qwen_dir: qwen_dir.into(),
            clip: clip.into(),
            checkpoint: checkpoint.into(),
            wooden_dir: wooden_dir.into(),
        }
    }
}

/// Structured progress plus cooperative cancellation. A DiT forward is the
/// granularity floor; Qwen checks between its 36 decoder layers and sampling
/// checks between each of the official Euler steps.
#[derive(Default)]
pub struct HyMotionRunControl<'a> {
    pub on_phase: Option<&'a mut dyn FnMut(&str, usize, usize)>,
    pub cancel: Option<&'a (dyn Fn() -> bool + 'a)>,
}

impl HyMotionRunControl<'_> {
    fn phase(&mut self, name: &str, done: usize, total: usize) {
        if let Some(callback) = self.on_phase.as_deref_mut() {
            callback(name, done, total);
        }
    }

    fn check(&self) -> Result<()> {
        if self.cancel.map_or(false, |cancelled| cancelled()) {
            Err(DiffusionError::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct HyMotionPromptEncoderLoadTiming {
    pub qwen_contract_s: f64,
    pub clip_load_s: f64,
    pub total_s: f64,
}

#[derive(Clone, Debug, Default)]
pub struct HyMotionSamplerLoadTiming {
    pub checkpoint_upload_s: f64,
    pub wooden_load_s: f64,
    pub total_s: f64,
}

#[derive(Clone, Debug, Default)]
pub struct HyMotionPipelineLoadTiming {
    pub prompt_encoder: HyMotionPromptEncoderLoadTiming,
    pub sampler: HyMotionSamplerLoadTiming,
    pub total_s: f64,
}

#[derive(Clone, Debug)]
pub struct HyMotionConditioning {
    /// Cropped real Qwen rows (`text_tokens x 4096`), with no padding.
    pub context: Vec<f32>,
    /// CLIP-L/14 text-model pooler output (`768`).
    pub vector: Vec<f32>,
    pub qwen_input_ids: Vec<u32>,
    pub qwen_crop_start: usize,
    pub clip_input_ids: Vec<i32>,
    pub clip_eos_index: usize,
}

impl HyMotionConditioning {
    pub fn text_tokens(&self) -> usize {
        self.context.len() / HY_MOTION_CONTEXT_DIM
    }

    pub fn validate(&self) -> Result<()> {
        if self.context.is_empty() || self.context.len() % HY_MOTION_CONTEXT_DIM != 0 {
            return Err(DiffusionError::workflow(
                "HY-Motion conditioning context is not complete 4096-wide rows",
            ));
        }
        if self.vector.len() != HY_MOTION_VECTOR_DIM {
            return Err(DiffusionError::workflow(format!(
                "HY-Motion conditioning vector has {} values, expected {HY_MOTION_VECTOR_DIM}",
                self.vector.len()
            )));
        }
        if self.qwen_crop_start + self.text_tokens() > self.qwen_input_ids.len() {
            return Err(DiffusionError::workflow(
                "HY-Motion conditioning Qwen crop exceeds its token presentation",
            ));
        }
        if self.clip_input_ids.is_empty() || self.clip_eos_index >= self.clip_input_ids.len() {
            return Err(DiffusionError::workflow(
                "HY-Motion conditioning CLIP EOS is out of range",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub struct HyMotionPromptTiming {
    pub tokenize_s: f64,
    pub qwen_encode_s: f64,
    pub clip_encode_s: f64,
    pub total_s: f64,
}

#[derive(Clone, Debug)]
pub struct HyMotionPromptRun {
    pub conditioning: HyMotionConditioning,
    pub timing: HyMotionPromptTiming,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HyMotionEvictionReport {
    pub qwen_tensors: usize,
    pub clip_tensors: usize,
}

/// Native Qwen3-8B final-hidden-state + CLIP-L pooler conditioner.
pub struct HyMotionPromptEncoder {
    qwen_weights: H3ShardedWeights,
    qwen_prepared: HyMotionQwenPrepared,
    qwen_tokenizer: HyMotionQwenTokenizer,
    clip: HyMotionClipConditioner,
}

impl HyMotionPromptEncoder {
    pub fn load(
        qwen_dir: impl AsRef<Path>,
        clip: impl AsRef<Path>,
    ) -> Result<(Self, HyMotionPromptEncoderLoadTiming)> {
        Self::load_with_control(
            qwen_dir,
            clip,
            &mut HyMotionRunControl::default(),
        )
    }

    /// Load the two prompt conditioners with observable component boundaries.
    /// A single safetensors header scan or model upload remains the
    /// cancellation granularity floor, but a cancelled service load never
    /// proceeds into the next heavyweight component.
    pub fn load_with_control(
        qwen_dir: impl AsRef<Path>,
        clip: impl AsRef<Path>,
        control: &mut HyMotionRunControl,
    ) -> Result<(Self, HyMotionPromptEncoderLoadTiming)> {
        let total_started = Instant::now();
        control.check()?;
        control.phase("load-qwen", 0, 1);
        let qwen_started = Instant::now();
        let qwen_weights = H3ShardedWeights::load(qwen_dir.as_ref())?;
        let qwen_tokenizer = HyMotionQwenTokenizer::load(qwen_dir.as_ref())?;
        let qwen_prepared = HyMotionQwenPrepared::prepare(&qwen_weights)?;
        let qwen_contract_s = qwen_started.elapsed().as_secs_f64();
        control.phase("load-qwen", 1, 1);

        control.check()?;
        control.phase("load-clip", 0, 1);
        let clip_started = Instant::now();
        let clip = HyMotionClipConditioner::load(clip)?;
        let clip_load_s = clip_started.elapsed().as_secs_f64();
        control.phase("load-clip", 1, 1);
        control.check()?;
        Ok((
            Self {
                qwen_weights,
                qwen_prepared,
                qwen_tokenizer,
                clip,
            },
            HyMotionPromptEncoderLoadTiming {
                qwen_contract_s,
                clip_load_s,
                total_s: total_started.elapsed().as_secs_f64(),
            },
        ))
    }

    pub fn encode(&mut self, prompt: &str) -> Result<HyMotionPromptRun> {
        self.encode_with_control(prompt, &mut HyMotionRunControl::default())
    }

    pub fn encode_with_control(
        &mut self,
        prompt: &str,
        control: &mut HyMotionRunControl,
    ) -> Result<HyMotionPromptRun> {
        if prompt.trim().is_empty() {
            return Err(DiffusionError::workflow(
                "HY-Motion prompt must contain non-whitespace text",
            ));
        }
        control.check()?;
        let total_started = Instant::now();
        control.phase("qwen-tokenize", 0, 0);
        let tokenize_started = Instant::now();
        let tokens = self.qwen_tokenizer.tokenize(prompt)?;
        let tokenize_s = tokenize_started.elapsed().as_secs_f64();

        control.phase("qwen-encode", 0, 36);
        let qwen_started = Instant::now();
        let qwen = {
            let cancel = control.cancel;
            let on_phase = &mut control.on_phase;
            let mut on_layer = |done: usize, total: usize| -> Result<()> {
                if cancel.map_or(false, |cancelled| cancelled()) {
                    return Err(DiffusionError::Cancelled);
                }
                if let Some(callback) = on_phase.as_deref_mut() {
                    callback("qwen-encode", done, total);
                }
                Ok(())
            };
            hy_motion_qwen_encode_controlled(
                &self.qwen_weights,
                &self.qwen_prepared,
                &tokens,
                Some(&mut on_layer),
            )?
        };
        let qwen_encode_s = qwen_started.elapsed().as_secs_f64();

        control.check()?;
        control.phase("clip-encode", 0, 0);
        let clip_started = Instant::now();
        let clip = self.clip.encode(prompt)?;
        let clip_encode_s = clip_started.elapsed().as_secs_f64();
        control.check()?;

        let conditioning = HyMotionConditioning {
            context: qwen.context,
            vector: clip.vector,
            qwen_input_ids: qwen.input_ids,
            qwen_crop_start: qwen.crop_start,
            clip_input_ids: clip.input_ids,
            clip_eos_index: clip.eos_index,
        };
        conditioning.validate()?;
        Ok(HyMotionPromptRun {
            conditioning,
            timing: HyMotionPromptTiming {
                tokenize_s,
                qwen_encode_s,
                clip_encode_s,
                total_s: total_started.elapsed().as_secs_f64(),
            },
        })
    }

    /// Drop conditioner CUDA caches without discarding tokenizers, shard
    /// metadata, Qwen constants, or CLIP's host checkpoint.
    pub fn evict_device_weights(&self) -> Result<HyMotionEvictionReport> {
        Ok(HyMotionEvictionReport {
            qwen_tensors: hy_motion_qwen_evict()?,
            clip_tensors: self.clip.evict_device_weights()?,
        })
    }
}

#[derive(Clone, Debug)]
pub struct HyMotionGenerateParams {
    pub frames: usize,
    pub steps: usize,
    pub guidance: f32,
    pub seed: u64,
    /// Use f16 operands for the HY-Motion attention GEMMs.
    pub f16_attention_operands: bool,
    /// Optional normalized starting noise (`frames x 201`). Supplying the
    /// oracle noise is the fixed-seed parity path. `None` uses the native,
    /// deterministic distribution-compatible generator below.
    pub initial_latent: Option<Vec<f32>>,
    pub smooth: bool,
}

impl Default for HyMotionGenerateParams {
    fn default() -> Self {
        Self {
            frames: 120,
            steps: HY_MOTION_STEPS,
            guidance: HY_MOTION_CFG,
            seed: 0,
            f16_attention_operands: true,
            initial_latent: None,
            smooth: true,
        }
    }
}

impl HyMotionGenerateParams {
    fn validate(&self) -> Result<()> {
        if !(HY_MOTION_MIN_FRAMES..=HY_MOTION_MAX_FRAMES).contains(&self.frames) {
            return Err(DiffusionError::workflow(format!(
                "HY-Motion frame count must be {HY_MOTION_MIN_FRAMES}..={HY_MOTION_MAX_FRAMES}, got {}",
                self.frames
            )));
        }
        if self.steps == 0 {
            return Err(DiffusionError::workflow(
                "HY-Motion Euler step count must be non-zero",
            ));
        }
        if !self.guidance.is_finite() {
            return Err(DiffusionError::workflow(
                "HY-Motion guidance must be finite",
            ));
        }
        if let Some(initial) = &self.initial_latent {
            let expected = self.frames * HY_MOTION_INPUT_DIM;
            if initial.len() != expected {
                return Err(DiffusionError::workflow(format!(
                    "HY-Motion initial latent has {} values, expected {expected}",
                    initial.len()
                )));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub struct HyMotionSampleTiming {
    pub noise_s: f64,
    pub prepare_s: f64,
    pub denoise_s: f64,
    pub decode_s: f64,
    pub total_s: f64,
}

#[derive(Clone, Debug)]
pub struct HyMotionSampleRun {
    /// Final normalized model latent (`frames x 201`).
    pub normalized_latent: Vec<f32>,
    pub decoded: HyMotionDecoded,
    pub timing: HyMotionSampleTiming,
}

/// Full-f32 DiT sampler and exact 22-active/52-wooden-joint decoder.
pub struct HyMotionSampler {
    weights: HyMotionDeviceWeights,
    wooden: HyMotionWoodenModel,
}

impl HyMotionSampler {
    pub fn load(
        checkpoint: impl AsRef<Path>,
        wooden_dir: impl AsRef<Path>,
    ) -> Result<(Self, HyMotionSamplerLoadTiming)> {
        Self::load_with_control(
            checkpoint,
            wooden_dir,
            &mut HyMotionRunControl::default(),
        )
    }

    /// Load the DiT and the exact WoodenMesh decoder with service-facing
    /// progress/cancellation boundaries.
    pub fn load_with_control(
        checkpoint: impl AsRef<Path>,
        wooden_dir: impl AsRef<Path>,
        control: &mut HyMotionRunControl,
    ) -> Result<(Self, HyMotionSamplerLoadTiming)> {
        let total_started = Instant::now();
        control.check()?;
        control.phase("load-dit", 0, 1);
        let checkpoint_started = Instant::now();
        let mut checkpoint = HyMotionCheckpoint::open(checkpoint)?;
        let weights = HyMotionDeviceWeights::load_full(&mut checkpoint)?;
        let checkpoint_upload_s = checkpoint_started.elapsed().as_secs_f64();
        control.phase("load-dit", 1, 1);

        control.check()?;
        control.phase("load-wooden", 0, 1);
        let wooden_started = Instant::now();
        let wooden = HyMotionWoodenModel::load(wooden_dir)?;
        let wooden_load_s = wooden_started.elapsed().as_secs_f64();
        control.phase("load-wooden", 1, 1);
        control.check()?;
        Ok((
            Self { weights, wooden },
            HyMotionSamplerLoadTiming {
                checkpoint_upload_s,
                wooden_load_s,
                total_s: total_started.elapsed().as_secs_f64(),
            },
        ))
    }

    pub fn parameter_count(&self) -> usize {
        self.weights.parameter_count()
    }

    /// Immutable rest skeleton/joint-order contract for native retarget and
    /// animation exporters.
    pub fn skeleton(&self) -> &HyMotionSkeleton {
        self.wooden.skeleton()
    }

    pub fn sample(
        &self,
        conditioning: &HyMotionConditioning,
        params: &HyMotionGenerateParams,
    ) -> Result<HyMotionSampleRun> {
        self.sample_with_control(
            conditioning,
            params,
            &mut HyMotionRunControl::default(),
        )
    }

    pub fn sample_with_control(
        &self,
        conditioning: &HyMotionConditioning,
        params: &HyMotionGenerateParams,
        control: &mut HyMotionRunControl,
    ) -> Result<HyMotionSampleRun> {
        conditioning.validate()?;
        params.validate()?;
        control.check()?;
        let total_started = Instant::now();

        control.phase("noise", 0, 0);
        let noise_started = Instant::now();
        let initial_latent = match &params.initial_latent {
            Some(initial) => initial.clone(),
            None => HyMotionNoiseRng::new(params.seed)
                .fill_normal(params.frames * HY_MOTION_INPUT_DIM),
        };
        let noise_s = noise_started.elapsed().as_secs_f64();

        control.check()?;
        control.phase("prepare", 0, 0);
        let prepare_started = Instant::now();
        let prepared = self.weights.prepare_cfg(
            &conditioning.context,
            &conditioning.vector,
            params.frames,
            params.f16_attention_operands,
        )?;
        let prepare_s = prepare_started.elapsed().as_secs_f64();

        control.check()?;
        control.phase("denoise", 0, params.steps);
        let denoise_started = Instant::now();
        let normalized_latent = {
            let cancel = control.cancel;
            let on_phase = &mut control.on_phase;
            let mut on_step = |done: usize, total: usize| {
                if let Some(callback) = on_phase.as_deref_mut() {
                    callback("denoise", done, total);
                }
            };
            self.weights.sample_cfg_euler_controlled(
                &initial_latent,
                &prepared,
                params.steps,
                params.guidance,
                Some(&mut on_step),
                cancel,
            )?
        };
        let denoise_s = denoise_started.elapsed().as_secs_f64();

        control.check()?;
        control.phase("decode", 0, 0);
        let decode_started = Instant::now();
        let (mean, std) = self.weights.normalization_stats()?;
        let latent_denorm =
            hy_motion_denormalize(&normalized_latent, params.frames, mean, std)?;
        let decoded = self
            .wooden
            .decode_denormalized(&latent_denorm, params.frames, params.smooth)?;
        let decode_s = decode_started.elapsed().as_secs_f64();
        control.phase("done", 1, 1);

        Ok(HyMotionSampleRun {
            normalized_latent,
            decoded,
            timing: HyMotionSampleTiming {
                noise_s,
                prepare_s,
                denoise_s,
                decode_s,
                total_s: total_started.elapsed().as_secs_f64(),
            },
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct HyMotionGenerateTiming {
    pub prompt: HyMotionPromptTiming,
    pub sample: HyMotionSampleTiming,
    pub total_s: f64,
}

#[derive(Clone, Debug)]
pub struct HyMotionGenerateRun {
    pub conditioning: HyMotionConditioning,
    pub normalized_latent: Vec<f32>,
    pub decoded: HyMotionDecoded,
    pub conditioning_cache_hit: bool,
    pub timing: HyMotionGenerateTiming,
}

/// Persistent high-throughput prompt-to-motion runtime. It caches the most
/// recent host conditioning exactly, retains Qwen and DiT device weights,
/// and retains CLIP's host checkpoint. This is the accepted fastest repeat-
/// job mode on the reference box.
pub struct HyMotionPipeline {
    encoder: HyMotionPromptEncoder,
    sampler: HyMotionSampler,
    cached_conditioning: Option<(String, HyMotionConditioning)>,
}

impl HyMotionPipeline {
    pub fn load(
        paths: &HyMotionModelPaths,
    ) -> Result<(Self, HyMotionPipelineLoadTiming)> {
        Self::load_with_control(paths, &mut HyMotionRunControl::default())
    }

    /// Load a resident full pipeline while reporting each heavyweight model
    /// component. This is the canonical service entry point; callers should
    /// retain the resulting pipeline instead of reloading it per request.
    pub fn load_with_control(
        paths: &HyMotionModelPaths,
        control: &mut HyMotionRunControl,
    ) -> Result<(Self, HyMotionPipelineLoadTiming)> {
        let total_started = Instant::now();
        let (encoder, prompt_encoder) =
            HyMotionPromptEncoder::load_with_control(&paths.qwen_dir, &paths.clip, control)?;
        let (sampler, sampler_timing) =
            HyMotionSampler::load_with_control(&paths.checkpoint, &paths.wooden_dir, control)?;
        Ok((
            Self {
                encoder,
                sampler,
                cached_conditioning: None,
            },
            HyMotionPipelineLoadTiming {
                prompt_encoder,
                sampler: sampler_timing,
                total_s: total_started.elapsed().as_secs_f64(),
            },
        ))
    }

    pub fn clear_conditioning_cache(&mut self) {
        self.cached_conditioning = None;
    }

    /// Rest skeleton paired with every [`HyMotionGenerateRun`]. The run's
    /// first 22 local matrices map to these first 22 entries; the remaining
    /// finger joints use identity rotations.
    pub fn skeleton(&self) -> &HyMotionSkeleton {
        self.sampler.skeleton()
    }

    pub fn encode_prompt_uncached(
        &mut self,
        prompt: &str,
        control: &mut HyMotionRunControl,
    ) -> Result<HyMotionPromptRun> {
        self.encoder.encode_with_control(prompt, control)
    }

    pub fn evict_conditioner_device_weights(&self) -> Result<HyMotionEvictionReport> {
        self.encoder.evict_device_weights()
    }

    pub fn generate(
        &mut self,
        prompt: &str,
        params: &HyMotionGenerateParams,
    ) -> Result<HyMotionGenerateRun> {
        self.generate_with_control(
            prompt,
            params,
            &mut HyMotionRunControl::default(),
        )
    }

    pub fn generate_with_control(
        &mut self,
        prompt: &str,
        params: &HyMotionGenerateParams,
        control: &mut HyMotionRunControl,
    ) -> Result<HyMotionGenerateRun> {
        let total_started = Instant::now();
        let (conditioning, prompt_timing, conditioning_cache_hit) = match
            self.cached_conditioning
                .as_ref()
                .filter(|(cached_prompt, _)| cached_prompt == prompt)
        {
            Some((_, conditioning)) => {
                control.phase("conditioning-cached", 1, 1);
                (conditioning.clone(), HyMotionPromptTiming::default(), true)
            }
            None => {
                let run = self.encoder.encode_with_control(prompt, control)?;
                self.cached_conditioning =
                    Some((prompt.to_string(), run.conditioning.clone()));
                (run.conditioning, run.timing, false)
            }
        };
        let sample = self
            .sampler
            .sample_with_control(&conditioning, params, control)?;
        Ok(HyMotionGenerateRun {
            conditioning,
            normalized_latent: sample.normalized_latent,
            decoded: sample.decoded,
            conditioning_cache_hit,
            timing: HyMotionGenerateTiming {
                prompt: prompt_timing,
                sample: sample.timing,
                total_s: total_started.elapsed().as_secs_f64(),
            },
        })
    }
}

/// Deterministic xorshift64*/Box-Muller standard normal generator. This has
/// the same distribution as `torch.randn`, but not its seed-to-sample bit
/// mapping. Pass `initial_latent` for oracle/torch-seed composition parity.
pub struct HyMotionNoiseRng {
    state: u64,
    spare: Option<f32>,
}

impl HyMotionNoiseRng {
    pub fn new(seed: u64) -> Self {
        Self {
            state: seed.wrapping_mul(0x9e37_79b9_7f4a_7c15).max(1) | 1,
            spare: None,
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut value = self.state;
        value ^= value >> 12;
        value ^= value << 25;
        value ^= value >> 27;
        self.state = value;
        value.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn next_uniform(&mut self) -> f32 {
        (((self.next_u64() >> 40) as f32) + 1.0) / (1u32 << 24) as f32
    }

    pub fn next_normal(&mut self) -> f32 {
        if let Some(value) = self.spare.take() {
            return value;
        }
        let u1 = self.next_uniform();
        let u2 = self.next_uniform();
        let radius = (-2.0 * u1.ln()).sqrt();
        let angle = 2.0 * std::f32::consts::PI * u2;
        self.spare = Some(radius * angle.sin());
        radius * angle.cos()
    }

    pub fn fill_normal(&mut self, len: usize) -> Vec<f32> {
        (0..len).map(|_| self.next_normal()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_accepted_full_runtime_contract() {
        let params = HyMotionGenerateParams::default();
        assert_eq!(params.frames, 120);
        assert_eq!(params.steps, 50);
        assert_eq!(params.guidance, 5.0);
        assert!(params.f16_attention_operands);
        assert!(params.smooth);
        params.validate().unwrap();
    }

    #[test]
    fn native_noise_is_deterministic_and_standard_normal() {
        let left = HyMotionNoiseRng::new(123).fill_normal(20_000);
        let right = HyMotionNoiseRng::new(123).fill_normal(20_000);
        assert_eq!(left, right);
        let mean = left.iter().map(|&value| value as f64).sum::<f64>() / left.len() as f64;
        let variance = left
            .iter()
            .map(|&value| {
                let delta = value as f64 - mean;
                delta * delta
            })
            .sum::<f64>()
            / left.len() as f64;
        assert!(mean.abs() < 0.03, "mean={mean}");
        assert!((variance - 1.0).abs() < 0.04, "variance={variance}");
    }

    #[test]
    fn initial_latent_contract_rejects_wrong_shape() {
        let params = HyMotionGenerateParams {
            initial_latent: Some(vec![0.0; 119 * HY_MOTION_INPUT_DIM]),
            ..HyMotionGenerateParams::default()
        };
        assert!(params.validate().is_err());
    }

    #[test]
    fn conditioning_contract_rejects_misaligned_context() {
        let conditioning = HyMotionConditioning {
            context: vec![0.0; HY_MOTION_CONTEXT_DIM - 1],
            vector: vec![0.0; HY_MOTION_VECTOR_DIM],
            qwen_input_ids: vec![1],
            qwen_crop_start: 0,
            clip_input_ids: vec![1],
            clip_eos_index: 0,
        };
        assert!(conditioning.validate().is_err());
    }
}
