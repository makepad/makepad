//! Woosh DFlow end-to-end pipeline (CPU f32): tokenize (RoBERTa BPE) ->
//! RoBERTa-large TE (hidden_states[-2]) -> 4-step Euler+renoise DFlow DiT
//! (CFG embedded, single forward per step) -> VOCOS AE decode -> fixed 5.0 s
//! mono 48 kHz, peak-normalized only when |peak| > 1.
//!
//! Determinism note (same stance as sa3/moss): seeded noise is our own
//! splitmix64 gaussian — deterministic per build, NOT torch-compatible.
//! Validation replays the reference's dumped init/renoise gaussians instead.

use std::path::Path;

use crate::error::{DiffusionError, Result};
use crate::moss_pipeline::MossSeededNoise;
use crate::woosh::{
    woosh_renoise, woosh_t_schedule, WOOSH_DEFAULT_CFG, WOOSH_DEFAULT_RENOISE,
    WOOSH_DEFAULT_STEPS, WOOSH_LATENT_DIM, WOOSH_LATENT_FRAMES,
};
use crate::woosh_ae::WooshAe;
use crate::woosh_dit::{WooshCond, WooshDit};
use crate::woosh_text::WooshTextEncoder;
use crate::woosh_tokenizer::WooshTokenizer;
use crate::{band_progress, emit_progress, hook_ref, ProgressHook};

pub struct WooshPipeline {
    pub tokenizer: WooshTokenizer,
    pub text: WooshTextEncoder,
    pub dit: WooshDit,
    pub ae: WooshAe,
    /// Empty-prompt condition (the CFG negative is baked into the DFlow
    /// forward via the cfg embedding, but the reference still encodes the
    /// empty string as the description for the unconditional stream — for
    /// DFlow the SINGLE forward uses the positive text; this cached negative
    /// exists for the optional Flow path and for parity tooling).
    pub neg_cond: WooshCond,
}

impl WooshPipeline {
    /// Loads the three single-file checkpoints + tokenizer.json.
    /// `progress` spans the whole load: TE and DiT safetensors dominate.
    pub fn load(
        te_path: impl AsRef<Path>,
        dflow_path: impl AsRef<Path>,
        ae_path: impl AsRef<Path>,
        tokenizer_path: impl AsRef<Path>,
        mut progress: Option<ProgressHook>,
    ) -> Result<Self> {
        emit_progress(&mut progress, "load woosh te", 0.0)?;
        let te_weights = crate::woosh::woosh_open(te_path)?;
        let text = WooshTextEncoder::load(&te_weights)?;
        emit_progress(&mut progress, "load woosh dflow", 0.35)?;
        let dflow_weights = crate::woosh::woosh_open(dflow_path)?;
        let dit = WooshDit::load(&dflow_weights)?;
        emit_progress(&mut progress, "load woosh ae", 0.65)?;
        let ae_weights = crate::woosh::woosh_open(ae_path)?;
        let ae = WooshAe::load(&ae_weights)?;
        emit_progress(&mut progress, "load woosh tokenizer", 0.85)?;
        let tokenizer = WooshTokenizer::load(tokenizer_path.as_ref())?;
        // Cache the empty-prompt condition (used by the optional Flow CFG
        // path; cheap relative to load).
        emit_progress(&mut progress, "encode empty prompt", 0.9)?;
        let (neg_ids, neg_mask) = tokenizer.encode_padded("");
        let neg_hidden = text.encode(&neg_ids, &neg_mask, None)?;
        let neg_cond = dit.embed_condition(&neg_hidden, &neg_mask)?;
        Ok(Self {
            tokenizer,
            text,
            dit,
            ae,
            neg_cond,
        })
    }

    /// Tokenize + TE + condition embed for one prompt.
    pub fn encode_prompt(
        &self,
        prompt: &str,
        progress: Option<ProgressHook>,
    ) -> Result<WooshCond> {
        let (ids, mask) = self.tokenizer.encode_padded(prompt);
        let hidden = self.text.encode(&ids, &mask, progress)?;
        self.dit.embed_condition(&hidden, &mask)
    }

    /// DFlow Euler + renoise sampling from `init_noise` (128 x 501,
    /// channel-major). `fresh_noise(step)` supplies the renoise gaussian for
    /// steps with renoise > 0 (generation: seeded splitmix64; validation:
    /// the dumped reference draws). Returns final latents (128 x 501).
    pub fn sample(
        &self,
        cond: &WooshCond,
        init_noise: &[f32],
        steps: usize,
        cfg: f32,
        renoise: &[f32],
        fresh_noise: &mut dyn FnMut(usize) -> Vec<f32>,
        mut progress: Option<ProgressHook>,
        cancel: Option<&dyn Fn() -> bool>,
    ) -> Result<Vec<f32>> {
        let len = WOOSH_LATENT_DIM * WOOSH_LATENT_FRAMES;
        if init_noise.len() != len {
            return Err(DiffusionError::model(format!(
                "woosh sample: init noise {} values, expected {len}",
                init_noise.len()
            )));
        }
        if renoise.len() != steps {
            return Err(DiffusionError::model(format!(
                "woosh sample: {} renoise values for {steps} steps",
                renoise.len()
            )));
        }
        let t_vals = woosh_t_schedule(steps);
        let mut x = init_noise.to_vec();
        for i in 0..steps {
            if cancel.is_some_and(|is_cancelled| is_cancelled()) {
                return Err(DiffusionError::Cancelled);
            }
            emit_progress(
                &mut progress,
                &format!("denoise {}/{steps}", i + 1),
                i as f64 / steps as f64,
            )?;
            let (mut t, r) = (t_vals[i], t_vals[i + 1]);
            if let Some((t_hat, scale, std)) = woosh_renoise(t, r, renoise[i]) {
                let fresh = fresh_noise(i);
                if fresh.len() != len {
                    return Err(DiffusionError::model(format!(
                        "woosh sample: fresh noise {} values at step {i}, expected {len}",
                        fresh.len()
                    )));
                }
                for (xv, nv) in x.iter_mut().zip(fresh.iter()) {
                    *xv = scale * *xv + std * *nv;
                }
                t = t_hat;
            }
            let u = self.dit.forward(&x, t, r, cfg, cond)?;
            let dt = t - r;
            for (xv, uv) in x.iter_mut().zip(u.iter()) {
                *xv -= dt * *uv;
            }
        }
        Ok(x)
    }

    /// End-to-end: prompt -> 240,000 mono 48 kHz f32 samples (fixed 5.0 s —
    /// the model has no duration conditioning). Progress bands: text-encode
    /// (0..0.12), denoise k/4 (0.12..0.88), ae-decode (0.88..1).
    pub fn generate(
        &self,
        prompt: &str,
        seed: u64,
        mut progress: Option<ProgressHook>,
        cancel: Option<&dyn Fn() -> bool>,
    ) -> Result<Vec<f32>> {
        let cond = {
            let mut sub = band_progress(&mut progress, 0.0, 0.12);
            self.encode_prompt(prompt, hook_ref(&mut sub))?
        };
        if cancel.is_some_and(|is_cancelled| is_cancelled()) {
            return Err(DiffusionError::Cancelled);
        }
        let len = WOOSH_LATENT_DIM * WOOSH_LATENT_FRAMES;
        let mut noise_src = MossSeededNoise::new(seed);
        let init_noise = noise_src.draw(len);
        let latents = {
            let mut sub = band_progress(&mut progress, 0.12, 0.76);
            self.sample(
                &cond,
                &init_noise,
                WOOSH_DEFAULT_STEPS,
                WOOSH_DEFAULT_CFG,
                &WOOSH_DEFAULT_RENOISE,
                &mut |_step| noise_src.draw(len),
                hook_ref(&mut sub),
                cancel,
            )?
        };
        if cancel.is_some_and(|is_cancelled| is_cancelled()) {
            return Err(DiffusionError::Cancelled);
        }
        let mut audio = {
            let mut sub = band_progress(&mut progress, 0.88, 0.12);
            self.ae.decode(&latents, hook_ref(&mut sub))?
        };
        // Reference: peak-normalize only when the peak exceeds 1.0.
        let peak = audio.iter().fold(0f32, |acc, v| acc.max(v.abs()));
        if peak > 1.0 {
            let inv = 1.0 / peak;
            for v in audio.iter_mut() {
                *v *= inv;
            }
        }
        Ok(audio)
    }
}
