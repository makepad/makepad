//! SA3 Small SFX end-to-end pipeline: conditioning assembly, the ping-pong
//! rectified-flow sampler and the AE decode, on the CPU f32 modules.
//!
//! Noise injection is pluggable: the ping-pong sampler draws a fresh gaussian
//! at every step (reference semantics), so `Sa3NoiseSource` either replays
//! oracle-dump noise (validation) or generates seeded gaussians (service).
//! The built-in RNG is deterministic per seed but NOT torch-compatible;
//! same-seed outputs match the reference distributionally, not bit-for-bit.

use crate::sa3::{
    expo_fourier, linear, sa3_latent_len, sa3_sigmas, sa3_valid_len, Sa3Tensors, SA3_COND_DIM,
    SA3_COND_TOKENS, SA3_DOWNSAMPLE, SA3_LATENT_DIM, SA3_SECONDS_MAX, SA3_TEXT_TOKENS,
    SA3_TIMESTEP_FEATURES,
};
use crate::sa3_ae::Sa3AeDecoder;
use crate::sa3_text::{apply_learned_padding, T5GemmaEncoder};
use crate::sa3_transformer::{Sa3Dit, Sa3PadMode};
use crate::{band_progress, emit_progress, hook_ref, DiffusionError, ProgressHook, Result};
use std::path::Path;

/// Per-draw noise provider. `draw` is called with the draw index:
/// 0 = initial latent noise, then one per ping-pong step injection.
pub trait Sa3NoiseSource {
    fn draw(&mut self, index: usize, len: usize) -> Vec<f32>;
}

/// Deterministic gaussian source (splitmix64 + Box-Muller).
pub struct Sa3SeededNoise {
    state: u64,
    spare: Option<f32>,
}

impl Sa3SeededNoise {
    pub fn new(seed: u64) -> Self {
        Self {
            state: seed ^ 0x9E37_79B9_7F4A_7C15,
            spare: None,
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn next_f32(&mut self) -> f32 {
        // (0, 1): never exactly 0 so ln() stays finite.
        ((self.next_u64() >> 40) as f32 + 0.5) / 16_777_216.0
    }

    fn next_gaussian(&mut self) -> f32 {
        if let Some(v) = self.spare.take() {
            return v;
        }
        let u1 = self.next_f32();
        let u2 = self.next_f32();
        let r = (-2.0 * u1.ln()).sqrt();
        let theta = 2.0 * std::f32::consts::PI * u2;
        self.spare = Some(r * theta.sin());
        r * theta.cos()
    }
}

impl Sa3NoiseSource for Sa3SeededNoise {
    fn draw(&mut self, _index: usize, len: usize) -> Vec<f32> {
        (0..len).map(|_| self.next_gaussian()).collect()
    }
}

/// Precomputed conditioning for one prompt + duration.
pub struct Sa3Conditioning {
    /// to_cond_embed output, `[257, 1024]`.
    pub cond_embed: Vec<f32>,
    /// to_global_embed output (pre-timestep), `[1024]`.
    pub global_embed: Vec<f32>,
    /// Raw cross-attention conditioning `[257, 768]` (validation taps).
    pub cross_attn_cond: Vec<f32>,
    /// Raw seconds embedding `[768]` (validation taps).
    pub seconds_emb: Vec<f32>,
}

pub struct Sa3Pipeline {
    pub dit: Sa3Dit,
    pub ae: Sa3AeDecoder,
    pub text: T5GemmaEncoder,
    padding_embedding: Vec<f32>,
    seconds_w: Vec<f32>,
    seconds_b: Vec<f32>,
    /// Prepared CUDA weights (f16), present when the device path is active.
    device: Option<Sa3DeviceState>,
}

struct Sa3DeviceState {
    dit: crate::sa3_transformer::Sa3DitDevice,
    ae: crate::sa3_ae::Sa3AeDevice,
    text: crate::sa3_text::T5GemmaDevice,
}

impl Sa3Pipeline {
    /// `ckpt` = the combined SA3 model.safetensors, `t5gemma` = the
    /// t5gemma-b-b-ul2 encoder model.safetensors.
    ///
    /// `progress` (fractions span the whole load 0..=1) labels each
    /// component read so the cold disk read moves: "load sa3 dit/ae/te",
    /// then "prepare sa3 device" for the f16 conversion pass.
    pub fn load(
        ckpt: impl AsRef<Path>,
        t5gemma: impl AsRef<Path>,
        mut progress: Option<ProgressHook>,
    ) -> Result<Self> {
        let tensors = Sa3Tensors::load(ckpt)?;
        emit_progress(&mut progress, "load sa3 dit", 0.0)?;
        let dit = Sa3Dit::load(&tensors)?;
        emit_progress(&mut progress, "load sa3 ae", 0.25)?;
        let ae = Sa3AeDecoder::load(&tensors)?;
        let padding_embedding = tensors.f32_shaped(
            "conditioner.conditioners.prompt.padding_embedding",
            &[SA3_COND_DIM],
        )?;
        let seconds_w = tensors.f32_shaped(
            "conditioner.conditioners.seconds_total.embedder.embedding.1.weight",
            &[SA3_COND_DIM, SA3_TIMESTEP_FEATURES],
        )?;
        let seconds_b = tensors.f32_shaped(
            "conditioner.conditioners.seconds_total.embedder.embedding.1.bias",
            &[SA3_COND_DIM],
        )?;
        emit_progress(&mut progress, "load sa3 te", 0.4)?;
        let text = T5GemmaEncoder::load(t5gemma)?;
        let device = if crate::sa3::sa3_device_enabled() {
            emit_progress(&mut progress, "prepare sa3 device", 0.8)?;
            Some(Sa3DeviceState {
                dit: dit.prepare_device(),
                ae: ae.prepare_device(),
                text: text.prepare_device(),
            })
        } else {
            None
        };
        Ok(Self {
            dit,
            ae,
            text,
            padding_embedding,
            seconds_w,
            seconds_b,
            device,
        })
    }

    /// True when generation runs on the CUDA device path.
    pub fn device_active(&self) -> bool {
        self.device.is_some()
    }

    /// AE decode on the active path (device when available).
    pub fn decode(&self, latents: &[f32], latent_len: usize) -> Result<Vec<Vec<f32>>> {
        match &self.device {
            Some(device) => self.ae.decode_device(&device.ae, latents, latent_len),
            None => self.ae.decode(latents, latent_len),
        }
    }

    /// NumberConditioner: clamp(seconds,0,384)/384 -> ExpoFourier(256) ->
    /// Linear(256->768). Returns the 768 seconds embedding.
    pub fn seconds_embedding(&self, seconds: f64) -> Vec<f32> {
        let normalized = (seconds.clamp(0.0, SA3_SECONDS_MAX) / SA3_SECONDS_MAX) as f32;
        let features = expo_fourier(normalized, SA3_TIMESTEP_FEATURES);
        linear(
            &features,
            &self.seconds_w,
            Some(&self.seconds_b),
            1,
            SA3_TIMESTEP_FEATURES,
            SA3_COND_DIM,
        )
    }

    /// Builds conditioning from padded token ids (+ mask) and the duration.
    pub fn conditioning(
        &self,
        token_ids: &[u32],
        token_mask: &[bool],
        seconds: f64,
    ) -> Result<Sa3Conditioning> {
        self.conditioning_with_progress(token_ids, token_mask, seconds, None)
    }

    /// [`Self::conditioning`] with the TE's per-layer "text-encode k/12".
    pub fn conditioning_with_progress(
        &self,
        token_ids: &[u32],
        token_mask: &[bool],
        seconds: f64,
        progress: Option<ProgressHook>,
    ) -> Result<Sa3Conditioning> {
        if token_ids.len() != SA3_TEXT_TOKENS {
            return Err(DiffusionError::model(format!(
                "sa3 expects {SA3_TEXT_TOKENS} padded tokens, got {}",
                token_ids.len()
            )));
        }
        let mut hidden = match &self.device {
            Some(device) => self.text.encode_device_with_progress(
                &device.text,
                token_ids,
                token_mask,
                progress,
            )?,
            None => self.text.encode_with_progress(token_ids, token_mask, progress)?,
        };
        apply_learned_padding(&mut hidden, token_mask, &self.padding_embedding);
        let seconds_emb = self.seconds_embedding(seconds);

        let mut cross = vec![0f32; SA3_COND_TOKENS * SA3_COND_DIM];
        cross[..SA3_TEXT_TOKENS * SA3_COND_DIM].copy_from_slice(&hidden);
        cross[SA3_TEXT_TOKENS * SA3_COND_DIM..].copy_from_slice(&seconds_emb);

        let cond_embed = self.dit.embed_conditioning(&cross);
        let global_embed = self.dit.embed_global(&seconds_emb);
        Ok(Sa3Conditioning {
            cond_embed,
            global_embed,
            cross_attn_cond: cross,
            seconds_emb,
        })
    }

    /// Ping-pong sampler: denoised = x - t*v(x, t); x' = (1-t_next)*denoised
    /// + t_next*fresh_noise. Returns final latents `[latent_len, 256]`
    /// token-major. `progress` ticks "denoise k/N" at the START of each step
    /// (fraction (k-1)/N, local to the sample call).
    #[allow(clippy::too_many_arguments)]
    pub fn sample(
        &self,
        cond: &Sa3Conditioning,
        latent_len: usize,
        valid_len: usize,
        steps: usize,
        pad_mode: Sa3PadMode,
        noise: &mut dyn Sa3NoiseSource,
        mut progress: Option<ProgressHook>,
        cancel: Option<&dyn Fn() -> bool>,
    ) -> Result<Vec<f32>> {
        let sigmas = sa3_sigmas(steps);
        let n = latent_len * SA3_LATENT_DIM;
        let mut x = noise.draw(0, n);
        if x.len() != n {
            return Err(DiffusionError::model("sa3 noise source length mismatch"));
        }
        // Device runs use the reference's V-zeroing padding semantics; the
        // pad mode toggle is a CPU-path (validation) concern.
        let device_run = match &self.device {
            Some(device) => Some((
                device,
                self.dit.begin_device_run(
                    &device.dit,
                    &cond.cond_embed,
                    &cond.global_embed,
                    latent_len,
                    valid_len,
                )?,
            )),
            None => None,
        };
        for i in 0..steps {
            if cancel.is_some_and(|is_cancelled| is_cancelled()) {
                return Err(DiffusionError::Cancelled);
            }
            emit_progress(
                &mut progress,
                &format!("denoise {}/{steps}", i + 1),
                i as f64 / steps as f64,
            )?;
            let t_curr = sigmas[i];
            let t_next = sigmas[i + 1];
            let v = match &device_run {
                Some((device, run)) => self.dit.forward_device(&device.dit, run, &x, t_curr)?,
                None => self.dit.forward(
                    &x,
                    t_curr,
                    &cond.cond_embed,
                    &cond.global_embed,
                    latent_len,
                    valid_len,
                    pad_mode,
                )?,
            };
            if t_next > 0.0 {
                let fresh = noise.draw(i + 1, n);
                for j in 0..n {
                    let denoised = x[j] - t_curr * v[j];
                    x[j] = (1.0 - t_next) * denoised + t_next * fresh[j];
                }
            } else {
                for j in 0..n {
                    x[j] -= t_curr * v[j];
                }
            }
        }
        Ok(x)
    }

    /// Full text-to-audio: returns planar stereo f32 samples, truncated to
    /// the requested duration, with padding-region audio zeroed first
    /// (reference masks decoded audio beyond the valid latent region).
    ///
    /// `progress` fractions span the whole generate 0..=1: "text-encode
    /// k/12" (0..0.08), "denoise k/N" (0.08..0.85), "ae-decode k/6"
    /// (0.85..1). `cancel` is checked between phases and between steps; when
    /// it returns true the run unwinds with [`DiffusionError::Cancelled`].
    #[allow(clippy::too_many_arguments)]
    pub fn generate(
        &self,
        token_ids: &[u32],
        token_mask: &[bool],
        seconds: f64,
        steps: usize,
        pad_mode: Sa3PadMode,
        noise: &mut dyn Sa3NoiseSource,
        mut progress: Option<ProgressHook>,
        cancel: Option<&dyn Fn() -> bool>,
    ) -> Result<Vec<Vec<f32>>> {
        let latent_len = sa3_latent_len(seconds);
        let valid_len = sa3_valid_len(seconds);
        if cancel.is_some_and(|is_cancelled| is_cancelled()) {
            return Err(DiffusionError::Cancelled);
        }
        let cond = {
            let mut sub = band_progress(&mut progress, 0.0, 0.08);
            self.conditioning_with_progress(token_ids, token_mask, seconds, hook_ref(&mut sub))?
        };
        let latents = {
            let mut sub = band_progress(&mut progress, 0.08, 0.77);
            self.sample(
                &cond,
                latent_len,
                valid_len,
                steps,
                pad_mode,
                noise,
                hook_ref(&mut sub),
                cancel,
            )?
        };
        if cancel.is_some_and(|is_cancelled| is_cancelled()) {
            return Err(DiffusionError::Cancelled);
        }
        let mut audio = {
            let mut sub = band_progress(&mut progress, 0.85, 0.15);
            match &self.device {
                Some(device) => self.ae.decode_device_with_progress(
                    &device.ae,
                    &latents,
                    latent_len,
                    hook_ref(&mut sub),
                )?,
                None => self.ae.decode_with_progress(&latents, latent_len, hook_ref(&mut sub))?,
            }
        };
        let valid_samples = valid_len * SA3_DOWNSAMPLE;
        let keep = ((seconds * crate::sa3::SA3_SAMPLE_RATE as f64) as usize).min(valid_samples);
        for channel in audio.iter_mut() {
            let zero_from = valid_samples.min(channel.len());
            for v in channel[zero_from..].iter_mut() {
                *v = 0.0;
            }
            channel.truncate(keep);
            // clamp(-1, 1) as the reference does before writing wavs
            for v in channel.iter_mut() {
                *v = v.clamp(-1.0, 1.0);
            }
        }
        Ok(audio)
    }
}
