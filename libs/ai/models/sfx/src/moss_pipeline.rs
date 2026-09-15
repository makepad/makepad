//! MOSS-SoundEffect v2 end-to-end pipeline (CPU f32):
//! tokenize (Qwen BPE) -> Qwen3-1.7B TE -> 100-step CFG flow matching over
//! the fixed 30 s latent (128 x 1500) -> DAC decode -> mono 48 kHz samples
//! cropped to the requested seconds.
//!
//! Determinism note (same stance as sa3_pipeline): seeded noise is our own
//! splitmix64 gaussian — deterministic per build, NOT torch-compatible.
//! Validation replays the reference's dumped init noise instead.

use std::path::Path;

use crate::error::{DiffusionError, Result};
use makepad_ai_h3::h3_tokenizer::H3Tokenizer;
use crate::{band_progress, emit_progress, hook_ref, ProgressHook};
use crate::moss::{
    moss_format_prompt, moss_sigmas, moss_timesteps, MOSS_DEFAULT_CFG, MOSS_DEFAULT_SHIFT,
    MOSS_HOP, MOSS_LATENT_DIM, MOSS_LATENT_FRAMES, MOSS_MAX_SECONDS, MOSS_SAMPLE_RATE,
    MOSS_TEXT_TOKENS, MOSS_TE_HIDDEN,
};
use crate::moss_dac::MossDac;
use crate::moss_dit::{MossDit, MossDitContext};
use crate::moss_text::MossTextEncoder;

/// Margin of extra latent frames decoded past the crop point so the decoder's
/// receptive field cannot bleed edge effects into kept samples (~10 frames of
/// true receptive field; 16 is comfortably past it).
const DECODE_MARGIN_FRAMES: usize = 16;

pub struct MossSeededNoise {
    state: u64,
    spare: Option<f32>,
}

impl MossSeededNoise {
    pub fn new(seed: u64) -> Self {
        Self {
            state: seed ^ 0xA5B3_5705_87F6_C3E1,
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

    pub fn draw(&mut self, len: usize) -> Vec<f32> {
        (0..len).map(|_| self.next_gaussian()).collect()
    }
}

pub struct MossPipeline {
    pub tokenizer: H3Tokenizer,
    pub text: MossTextEncoder,
    pub dit: MossDit,
    pub dac: MossDac,
    /// Cached zero-context (empty negative prompt) projection.
    neg_context: MossDitContext,
}

impl MossPipeline {
    /// `te_dir` = text_encoder safetensors dir, `dit_dir` = transformer dir,
    /// `vae_pth` = DAC .pth, `tokenizer_dir` = dir with tokenizer.json.
    ///
    /// `progress` (phase-local fraction 0..=1 across the whole load) makes
    /// the cold disk read move: "load moss te 1.2/3.4GB" and "load moss dit
    /// 1.4/2.6GB" tick every ~256MB; the DAC .pth parse gets one label.
    pub fn load(
        te_dir: impl AsRef<Path>,
        dit_dir: impl AsRef<Path>,
        vae_pth: impl AsRef<Path>,
        tokenizer_dir: impl AsRef<Path>,
        mut progress: Option<ProgressHook>,
    ) -> Result<Self> {
        let te_shards = crate::moss::moss_open_shards(te_dir)?;
        let dit_shards = crate::moss::moss_open_shards(dit_dir)?;
        let text = {
            let mut sub = band_progress(&mut progress, 0.0, 0.5);
            MossTextEncoder::load_with_progress(&te_shards, hook_ref(&mut sub))?
        };
        let dit = {
            let mut sub = band_progress(&mut progress, 0.5, 0.42);
            MossDit::load_with_progress(&dit_shards, hook_ref(&mut sub))?
        };
        emit_progress(&mut progress, "load moss dac", 0.92)?;
        let dac = MossDac::load(vae_pth)?;
        emit_progress(&mut progress, "load moss tokenizer", 0.98)?;
        let tokenizer = H3Tokenizer::load(tokenizer_dir.as_ref())?;
        // Empty negative prompt: zero valid tokens -> the reference zeroes
        // every context row, so the raw negative context is all zeros.
        let neg_context = dit.embed_context(&vec![0f32; MOSS_TEXT_TOKENS * MOSS_TE_HIDDEN])?;
        Ok(Self {
            tokenizer,
            text,
            dit,
            dac,
            neg_context,
        })
    }

    /// Tokenizes the whitespace-cleaned, duration-suffixed prompt (reference
    /// HuggingfaceTokenizer clean='whitespace' + max_length 512 truncation).
    pub fn tokenize(&self, prompt: &str, seconds: f64) -> Vec<u32> {
        let formatted = moss_format_prompt(prompt, seconds);
        let cleaned = whitespace_clean(&formatted);
        let mut ids = self.tokenizer.encode(&cleaned);
        ids.truncate(MOSS_TEXT_TOKENS);
        ids
    }

    /// Encodes a prompt into the zero-padded raw context (512 x 2048).
    pub fn encode_context(&self, ids: &[u32]) -> Result<Vec<f32>> {
        self.text.encode(ids)
    }

    /// Full CFG denoise from `init_noise` (128 x 1500 channel-major).
    /// Returns the final latents. `progress` ticks "denoise k/N" at the
    /// START of each step (fraction (k-1)/N, local to the sample call).
    #[allow(clippy::too_many_arguments)]
    pub fn sample(
        &self,
        pos_context: &MossDitContext,
        init_noise: &[f32],
        steps: usize,
        cfg_scale: f32,
        shift: f32,
        mut progress: Option<ProgressHook>,
        cancel: Option<&dyn Fn() -> bool>,
    ) -> Result<Vec<f32>> {
        let frames = MOSS_LATENT_FRAMES;
        if init_noise.len() != MOSS_LATENT_DIM * frames {
            return Err(DiffusionError::model(format!(
                "moss sample: init noise {} values, expected {}",
                init_noise.len(),
                MOSS_LATENT_DIM * frames
            )));
        }
        let sigmas = moss_sigmas(steps, shift);
        let timesteps = moss_timesteps(&sigmas);
        let mut latents = init_noise.to_vec();
        for (i, &t) in timesteps.iter().enumerate() {
            if cancel.is_some_and(|is_cancelled| is_cancelled()) {
                return Err(DiffusionError::Cancelled);
            }
            emit_progress(
                &mut progress,
                &format!("denoise {}/{}", i + 1, timesteps.len()),
                i as f64 / timesteps.len() as f64,
            )?;
            let v_pos = self.dit.forward(&latents, frames, t, pos_context)?;
            let v = if cfg_scale != 1.0 {
                let v_neg = self.dit.forward(&latents, frames, t, &self.neg_context)?;
                let mut v = v_neg;
                for (vv, pv) in v.iter_mut().zip(v_pos.iter()) {
                    *vv += cfg_scale * (*pv - *vv);
                }
                v
            } else {
                v_pos
            };
            let d_sigma = sigmas[i + 1] - sigmas[i];
            for (x, vv) in latents.iter_mut().zip(v.iter()) {
                *x += vv * d_sigma;
            }
        }
        Ok(latents)
    }

    /// End-to-end: prompt -> mono 48 kHz f32 samples for `seconds`.
    /// Only decodes the latent prefix needed for the crop (plus margin).
    ///
    /// `progress` fractions span the whole generate 0..=1: "text-encode
    /// k/28" (0..0.06), "denoise k/N" (0.06..0.92), "dac-decode k/5"
    /// (0.92..1). `cancel` is polled between phases and steps.
    #[allow(clippy::too_many_arguments)]
    pub fn generate(
        &self,
        prompt: &str,
        seconds: f64,
        steps: usize,
        cfg_scale: f32,
        seed: u64,
        mut progress: Option<ProgressHook>,
        cancel: Option<&dyn Fn() -> bool>,
    ) -> Result<Vec<f32>> {
        let seconds = seconds.clamp(0.5, MOSS_MAX_SECONDS as f64);
        let ids = self.tokenize(prompt, seconds);
        let raw_context = {
            let mut sub = band_progress(&mut progress, 0.0, 0.06);
            self.text.encode_with_progress(&ids, hook_ref(&mut sub))?
        };
        let pos_context = self.dit.embed_context(&raw_context)?;
        let mut noise_src = MossSeededNoise::new(seed);
        let noise = noise_src.draw(MOSS_LATENT_DIM * MOSS_LATENT_FRAMES);
        let latents = {
            let mut sub = band_progress(&mut progress, 0.06, 0.86);
            self.sample(
                &pos_context,
                &noise,
                steps,
                cfg_scale,
                MOSS_DEFAULT_SHIFT,
                hook_ref(&mut sub),
                cancel,
            )?
        };
        if cancel.is_some_and(|is_cancelled| is_cancelled()) {
            return Err(DiffusionError::Cancelled);
        }
        let keep_samples = (seconds * MOSS_SAMPLE_RATE as f64).round() as usize;
        let need_frames =
            (keep_samples.div_ceil(MOSS_HOP) + DECODE_MARGIN_FRAMES).min(MOSS_LATENT_FRAMES);
        // Slice the latent prefix (channel-major rows are full frames wide).
        let mut prefix = vec![0f32; MOSS_LATENT_DIM * need_frames];
        for ch in 0..MOSS_LATENT_DIM {
            prefix[ch * need_frames..(ch + 1) * need_frames].copy_from_slice(
                &latents[ch * MOSS_LATENT_FRAMES..ch * MOSS_LATENT_FRAMES + need_frames],
            );
        }
        let mut audio = {
            let mut sub = band_progress(&mut progress, 0.92, 0.08);
            self.dac
                .decode_with_progress(&prefix, need_frames, hook_ref(&mut sub))?
        };
        audio.truncate(keep_samples);
        for v in audio.iter_mut() {
            *v = v.clamp(-1.0, 1.0);
        }
        Ok(audio)
    }

    /// Decodes final latents fully (validation path — matches the reference's
    /// full-window decode).
    pub fn decode_full(&self, latents: &[f32]) -> Result<Vec<f32>> {
        self.dac.decode(latents, MOSS_LATENT_FRAMES)
    }
}

pub fn moss_default_cfg() -> f32 {
    MOSS_DEFAULT_CFG
}

/// `re.sub(r'\s+', ' ', text).strip()`
pub fn whitespace_clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            in_space = true;
            continue;
        }
        if in_space && !out.is_empty() {
            out.push(' ');
        }
        in_space = false;
        out.push(ch);
    }
    out
}

// ---------------------------------------------------------------------------
// Device sampling (CUDA): frame-major latents, host scheduler steps.
// ---------------------------------------------------------------------------

use crate::moss_dac::MossDacDevice;
use crate::moss_dit::MossDitDevice;
use crate::moss_text::MossTeDevice;

/// Host-side device preparation (f16 weight bytes + cache keys only — no
/// live GPU handles, so the state is Send and can live inside a backend
/// that hops threads; per-generation GPU residents are created inside
/// [`MossPipeline::sample_device`]).
pub struct MossDeviceState {
    pub device: MossDitDevice,
    pub te: MossTeDevice,
    pub dac: MossDacDevice,
}

impl MossPipeline {
    /// Prepares the device weight caches (host-side, Send).
    pub fn prepare_device(&self) -> Result<MossDeviceState> {
        Ok(MossDeviceState {
            device: self.dit.prepare_device(),
            te: self.text.prepare_device(),
            dac: self.dac.prepare_device(),
        })
    }

    /// Full CFG denoise on the device. `init_noise` and the returned latents
    /// are channel-major (128 x 1500) like the CPU path. `progress` ticks
    /// "denoise k/N" at the START of each step — step 1 also streams the
    /// DiT's f16 weights into the device cache.
    #[allow(clippy::too_many_arguments)]
    pub fn sample_device(
        &self,
        state: &MossDeviceState,
        pos_context: &crate::moss_dit::MossDitContext,
        init_noise: &[f32],
        steps: usize,
        cfg_scale: f32,
        shift: f32,
        mut progress: Option<ProgressHook>,
        cancel: Option<&dyn Fn() -> bool>,
    ) -> Result<Vec<f32>> {
        let frames = MOSS_LATENT_FRAMES;
        let c = MOSS_LATENT_DIM;
        if init_noise.len() != c * frames {
            return Err(DiffusionError::model(format!(
                "moss sample_device: init noise {} values, expected {}",
                init_noise.len(),
                c * frames
            )));
        }
        let pos_run = self.dit.begin_device_run(&state.device, pos_context)?;
        let neg_run = self.dit.begin_device_run(&state.device, &self.neg_context)?;
        let rope = self.dit.device_rope(frames)?;
        // channel-major -> frame-major
        let mut x = vec![0f32; frames * c];
        for ch in 0..c {
            for f in 0..frames {
                x[f * c + ch] = init_noise[ch * frames + f];
            }
        }
        let sigmas = moss_sigmas(steps, shift);
        let timesteps = moss_timesteps(&sigmas);
        for (i, &t) in timesteps.iter().enumerate() {
            if cancel.is_some_and(|is_cancelled| is_cancelled()) {
                return Err(DiffusionError::Cancelled);
            }
            emit_progress(
                &mut progress,
                &format!("denoise {}/{}", i + 1, timesteps.len()),
                i as f64 / timesteps.len() as f64,
            )?;
            let v_pos = self
                .dit
                .forward_device(&state.device, &pos_run, &rope, &x, t)?;
            let v = if cfg_scale != 1.0 {
                let v_neg = self
                    .dit
                    .forward_device(&state.device, &neg_run, &rope, &x, t)?;
                let mut v = v_neg;
                for (vv, pv) in v.iter_mut().zip(v_pos.iter()) {
                    *vv += cfg_scale * (*pv - *vv);
                }
                v
            } else {
                v_pos
            };
            let d_sigma = sigmas[i + 1] - sigmas[i];
            for (xv, vv) in x.iter_mut().zip(v.iter()) {
                *xv += vv * d_sigma;
            }
        }
        // frame-major -> channel-major
        let mut latents = vec![0f32; c * frames];
        for f in 0..frames {
            for ch in 0..c {
                latents[ch * frames + f] = x[f * c + ch];
            }
        }
        Ok(latents)
    }

    /// End-to-end with automatic device dispatch. Same contract as
    /// [`MossPipeline::generate`],
    /// including the progress bands.
    #[allow(clippy::too_many_arguments)]
    pub fn generate_auto(
        &self,
        device: Option<&MossDeviceState>,
        prompt: &str,
        seconds: f64,
        steps: usize,
        cfg_scale: f32,
        seed: u64,
        mut progress: Option<ProgressHook>,
        cancel: Option<&dyn Fn() -> bool>,
    ) -> Result<Vec<f32>> {
        let device = match device {
            Some(state) if crate::moss::moss_device_enabled() => state,
            _ => return self.generate(prompt, seconds, steps, cfg_scale, seed, progress, cancel),
        };
        let seconds = seconds.clamp(0.5, crate::moss::MOSS_MAX_SECONDS as f64);
        let ids = self.tokenize(prompt, seconds);
        let raw_context = {
            let mut sub = band_progress(&mut progress, 0.0, 0.06);
            self.text
                .encode_device_with_progress(&device.te, &ids, hook_ref(&mut sub))?
        };
        let pos_context = self.dit.embed_context(&raw_context)?;
        let mut noise_src = MossSeededNoise::new(seed);
        let noise = noise_src.draw(MOSS_LATENT_DIM * MOSS_LATENT_FRAMES);
        let latents = {
            let mut sub = band_progress(&mut progress, 0.06, 0.86);
            self.sample_device(
                device,
                &pos_context,
                &noise,
                steps,
                cfg_scale,
                crate::moss::MOSS_DEFAULT_SHIFT,
                hook_ref(&mut sub),
                cancel,
            )?
        };
        if cancel.is_some_and(|is_cancelled| is_cancelled()) {
            return Err(DiffusionError::Cancelled);
        }
        let keep_samples = (seconds * MOSS_SAMPLE_RATE as f64).round() as usize;
        let need_frames =
            (keep_samples.div_ceil(MOSS_HOP) + 16).min(MOSS_LATENT_FRAMES);
        let mut prefix = vec![0f32; MOSS_LATENT_DIM * need_frames];
        for ch in 0..MOSS_LATENT_DIM {
            prefix[ch * need_frames..(ch + 1) * need_frames].copy_from_slice(
                &latents[ch * MOSS_LATENT_FRAMES..ch * MOSS_LATENT_FRAMES + need_frames],
            );
        }
        let mut audio = {
            let mut sub = band_progress(&mut progress, 0.92, 0.08);
            self.dac.decode_device_with_progress(
                &device.dac,
                &prefix,
                need_frames,
                hook_ref(&mut sub),
            )?
        };
        audio.truncate(keep_samples);
        for v in audio.iter_mut() {
            *v = v.clamp(-1.0, 1.0);
        }
        Ok(audio)
    }
}
