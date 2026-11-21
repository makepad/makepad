//! ACE-Step 1.5 XL turbo end-to-end pipeline: Qwen3 TE → condition encoder
//! → 8-step shifted flow-matching DiT → Oobleck VAE decode.
//!
//! Turbo checkpoints have guidance distilled in; CFG is not applied.

use crate::ace::{
    ace_apg, ace_clamp_seconds, ace_device_enabled, ace_format_prompt, ace_latent_len, ace_sigmas,
    AceApgMomentum, AceSeededNoise, ACE_APG_ETA, ACE_APG_MOMENTUM, ACE_APG_NORM_THRESHOLD,
    ACE_BASE_CFG, ACE_CONTEXT_DIM, ACE_DEFAULT_SHIFT, ACE_DEFAULT_STEPS, ACE_HOP,
    ACE_AUDIO_CHANNELS, ACE_INSTRUCTION, ACE_LATENT_DIM, ACE_MAX_LYRIC_TOKENS, ACE_MAX_TEXT_TOKENS,
    ACE_SAMPLE_RATE, ACE_TIMBRE_SECONDS,
};
use crate::ace_dit::{
    AceCondDevice, AceConditionEncoder, AceConditioning, AceCrossKvCache, AceDit, AceDitDevice,
};
use crate::ace_text::{ace_tokenize, AceTeDevice, AceTextEncoder};
use crate::oobleck::{OobleckDecoder, ACE_OOBLECK};
use makepad_ai_h3::h3_tokenizer::H3Tokenizer;
use crate::{emit_progress, ProgressHook, Result};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The VAE's tiles when the whole song doesn't fit on the GPU: a core of latents with a halo of
/// context each side, whose core is kept as it is (no blending). The decoder reaches more than 8
/// latents either side and fewer than 16: tiled with a halo of 16, the decode equals the whole
/// one bit for bit (8 differs by 5e-5, 4 by 2.5e-2; measured on the CPU decode). 64 latents
/// (2.56 s) bound the largest working slab, the last block's im2col (128 channels by 7 taps at
/// 48 kHz), to about 0.33 GB, where a whole song of 30 s takes about 2.6 GB.
const ACE_VAE_TILE_CORE: usize = 64;
const ACE_VAE_TILE_HALO: usize = 16;

pub struct AcePaths {
    pub text_encoder: PathBuf,
    pub tokenizer: PathBuf,
    pub condition_encoder: PathBuf,
    pub transformer: PathBuf,
    pub vae: PathBuf,
}

impl AcePaths {
    /// Resolve a diffusers-layout model root
    /// (`text_encoder/`, `tokenizer/`, `condition_encoder/`, `transformer/`, `vae/`).
    pub fn from_model_dir(dir: impl AsRef<Path>) -> Self {
        let dir = dir.as_ref();
        Self {
            text_encoder: dir.join("text_encoder"),
            tokenizer: dir.join("tokenizer"),
            condition_encoder: dir.join("condition_encoder"),
            transformer: dir.join("transformer"),
            vae: dir.join("vae"),
        }
    }
}

pub struct AceGenerate {
    pub prompt: String,
    pub lyrics: String,
    pub seconds: f64,
    pub seed: u64,
    pub steps: usize,
    pub shift: f32,
    pub guidance: f32,
    pub vocal_language: String,
}

impl AceGenerate {
    pub fn new(prompt: impl Into<String>, lyrics: impl Into<String>, seconds: f64, seed: u64) -> Self {
        Self {
            prompt: prompt.into(),
            lyrics: lyrics.into(),
            seconds,
            seed,
            steps: ACE_DEFAULT_STEPS,
            shift: ACE_DEFAULT_SHIFT,
            guidance: ACE_BASE_CFG,
            vocal_language: "en".into(),
        }
    }
}

pub struct AcePipeline {
    pub text: AceTextEncoder,
    pub tokenizer: H3Tokenizer,
    pub cond: AceConditionEncoder,
    pub dit: AceDit,
    pub vae: OobleckDecoder,
    te_device: Option<AceTeDevice>,
    cond_device: Option<AceCondDevice>,
    dit_device: Option<AceDitDevice>,
}

pub struct AceStageTaps {
    pub text_hidden: Vec<f32>,
    pub lyric_embed: Vec<f32>,
    pub encoder_hidden: Vec<f32>,
    pub latents0: Vec<f32>,
    pub velocity0: Vec<f32>,
    pub vt_uncond0: Vec<f32>,
    pub vt_guided0: Vec<f32>,
    pub last_vt_guided: Vec<f32>,
    pub latents_final: Vec<f32>,
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    pub sample_rate: u32,
}

impl AcePipeline {
    /// Loads the model. With a GPU each part's matrices go to their device copy as the part is
    /// read and their f32 host copies are freed, so the host holds the bf16 copies (about 11 GB)
    /// and not the f32 ones beside them as well (32 GB resident, which a 30 GB node ran out of).
    pub fn load(paths: &AcePaths, mut progress: Option<ProgressHook>) -> Result<Self> {
        let device = ace_device_enabled();
        emit_progress(&mut progress, "load ace te", 0.0)?;
        let mut text = AceTextEncoder::load(&paths.text_encoder)?;
        let te_device = device.then(|| text.prepare_device());
        emit_progress(&mut progress, "load ace tokenizer", 0.15)?;
        let tokenizer = H3Tokenizer::load(&paths.tokenizer)?;
        emit_progress(&mut progress, "load ace cond", 0.2)?;
        let mut cond = AceConditionEncoder::load(&paths.condition_encoder)?;
        let cond_device = device.then(|| cond.prepare_device());
        emit_progress(&mut progress, "load ace dit", 0.4)?;
        let (dit, dit_device) = if device {
            let (dit, dev) = AceDit::load_device(&paths.transformer, None)?;
            (dit, Some(dev))
        } else {
            (AceDit::load(&paths.transformer)?, None)
        };
        emit_progress(&mut progress, "load ace vae", 0.85)?;
        let vae = OobleckDecoder::load(&paths.vae, ACE_OOBLECK)?;
        emit_progress(&mut progress, "load ace", 1.0)?;
        Ok(Self {
            text,
            tokenizer,
            cond,
            dit,
            vae,
            te_device,
            cond_device,
            dit_device,
        })
    }

    /// The VAE decode: on the GPU when there is one, the whole song at once; when that fails
    /// (its working set doesn't fit beside what else the GPU holds, such as a chat model and its
    /// speech models), in tiles on the GPU; only when the GPU can't at all, on the CPU, about a
    /// hundred times slower. Each step down is logged with its reason.
    pub fn decode_audio(&self, latents: &[f32], frames: usize) -> Result<(Vec<f32>, Vec<f32>)> {
        if !ace_device_enabled() {
            return self.vae.decode_with_progress(latents, frames, None);
        }
        let whole = match self.vae.decode_device(latents, frames) {
            Ok(pair) => return Ok(pair),
            Err(e) => e,
        };
        eprintln!(
            "ace vae: the whole song didn't decode on the GPU ({whole}): decoding it in tiles of \
             {ACE_VAE_TILE_CORE} latents"
        );
        // What the failed attempt left in the GPU's pool goes back before the tiles.
        if let Err(e) = crate::backend::gpu_runtime_trim() {
            eprintln!("ace vae: the GPU's idle buffers weren't released: {e}");
        }
        let started = Instant::now();
        let tiled = self.vae.decode_tiled_device(
            latents,
            frames,
            frames,
            ACE_VAE_TILE_CORE,
            ACE_VAE_TILE_HALO,
            &mut |_, _| Ok(()),
        );
        match tiled {
            Ok(audio) => {
                eprintln!(
                    "ace vae: {frames} latents decoded in {} tiles on the GPU in {:.3}s",
                    frames.div_ceil(ACE_VAE_TILE_CORE),
                    started.elapsed().as_secs_f64()
                );
                Ok(audio.chunks_exact(ACE_AUDIO_CHANNELS).map(|s| (s[0], s[1])).unzip())
            }
            Err(e) => {
                eprintln!("ace vae: the tiles didn't decode on the GPU either ({e}): decoding on the CPU, much slower");
                self.vae.decode_with_progress(latents, frames, None)
            }
        }
    }

    pub fn device_active(&self) -> bool {
        self.dit_device.is_some()
    }

    pub fn dit_forward(
        &self,
        x: &[f32],
        context: &[f32],
        frames: usize,
        encoder: &AceConditioning,
        t: f32,
    ) -> Result<Vec<f32>> {
        self.dit_forward_kv(x, context, frames, encoder, t, None)
    }

    fn dit_forward_kv(
        &self,
        x: &[f32],
        context: &[f32],
        frames: usize,
        encoder: &AceConditioning,
        t: f32,
        cache: Option<&AceCrossKvCache>,
    ) -> Result<Vec<f32>> {
        if let Some(device) = &self.dit_device {
            if let Some(cache) = cache {
                self.dit
                    .forward_device_kv(device, x, context, frames, encoder, t, t, cache)
            } else {
                self.dit
                    .forward_device(device, x, context, frames, encoder, t, t)
            }
        } else {
            self.dit.forward(x, context, frames, encoder, t, t, None)
        }
    }

    pub fn encode_conditions(
        &self,
        req: &AceGenerate,
        seconds: f64,
        mut progress: Option<ProgressHook>,
    ) -> Result<AceConditioning> {
        let (text_str, lyric_str) = ace_format_prompt(
            &req.prompt,
            &req.lyrics,
            &req.vocal_language,
            seconds,
            Some(ACE_INSTRUCTION),
            None,
            None,
            None,
        );
        let text_ids = ace_tokenize(&self.tokenizer, &text_str, ACE_MAX_TEXT_TOKENS);
        let lyric_ids = ace_tokenize(&self.tokenizer, &lyric_str, ACE_MAX_LYRIC_TOKENS);
        emit_progress(&mut progress, "text-encode", 0.0)?;
        let text_hidden = if let Some(device) = &self.te_device {
            self.text.encode_device(device, &text_ids, None)?
        } else {
            self.text.encode_with_progress(&text_ids, None)?
        };
        let lyric_embed = self.text.embed_tokens(&lyric_ids)?;
        let timbre_frames = ace_latent_len(ACE_TIMBRE_SECONDS);
        let refer = self.cond.silence_slice(timbre_frames);
        match &self.cond_device {
            Some(cdev) => self.cond.encode_device(
                cdev,
                &text_hidden,
                text_ids.len(),
                &lyric_embed,
                lyric_ids.len(),
                &refer,
                timbre_frames,
            ),
            None => self.cond.encode(
                &text_hidden,
                text_ids.len(),
                &lyric_embed,
                lyric_ids.len(),
                &refer,
                timbre_frames,
            ),
        }
    }

    pub fn generate(
        &self,
        req: &AceGenerate,
        mut progress: Option<ProgressHook>,
    ) -> Result<(Vec<f32>, Vec<f32>)> {
        let taps = self.generate_taps(req, None, &mut progress)?;
        Ok((taps.left, taps.right))
    }

    /// Full generate with optional replay noise (validation) and stage taps.
    pub fn generate_taps(
        &self,
        req: &AceGenerate,
        replay_noise: Option<&[f32]>,
        progress: &mut Option<ProgressHook>,
    ) -> Result<AceStageTaps> {
        let t_gen = Instant::now();
        let seconds = ace_clamp_seconds(req.seconds);
        let frames = ace_latent_len(seconds);
        let steps = req.steps.max(1);
        let shift = if req.shift > 0.0 {
            req.shift
        } else {
            ACE_DEFAULT_SHIFT
        };

        let (text_str, lyric_str) = ace_format_prompt(
            &req.prompt,
            &req.lyrics,
            &req.vocal_language,
            seconds,
            Some(ACE_INSTRUCTION),
            None,
            None,
            None,
        );
        let text_ids = ace_tokenize(&self.tokenizer, &text_str, ACE_MAX_TEXT_TOKENS);
        let lyric_ids = ace_tokenize(&self.tokenizer, &lyric_str, ACE_MAX_LYRIC_TOKENS);
        emit_progress(progress, "text-encode", 0.0)?;
        let text_hidden = if let Some(device) = &self.te_device {
            self.text.encode_device(device, &text_ids, None)?
        } else {
            self.text.encode_with_progress(&text_ids, None)?
        };
        let lyric_embed = self.text.embed_tokens(&lyric_ids)?;
        let timbre_frames = ace_latent_len(ACE_TIMBRE_SECONDS);
        let refer = self.cond.silence_slice(timbre_frames);
        let cond = if let Some(cdev) = &self.cond_device {
            self.cond.encode_device(
                cdev,
                &text_hidden,
                text_ids.len(),
                &lyric_embed,
                lyric_ids.len(),
                &refer,
                timbre_frames,
            )?
        } else {
            self.cond.encode(
                &text_hidden,
                text_ids.len(),
                &lyric_embed,
                lyric_ids.len(),
                &refer,
                timbre_frames,
            )?
        };
        let cond_s = t_gen.elapsed().as_secs_f64();
        let t_after_cond = Instant::now();
        if makepad_ai_common::backend::prof::enabled() {
            eprint!(
                "{}",
                makepad_ai_common::backend::prof::report_and_reset("ace prof cond ")
            );
        }
        emit_progress(progress, "condition", 0.15)?;

        let n = frames * ACE_LATENT_DIM;
        let mut xt = if let Some(noise) = replay_noise {
            if noise.len() != n {
                return Err(crate::DiffusionError::model(format!(
                    "ace replay noise {} vs {n}",
                    noise.len()
                )));
            }
            noise.to_vec()
        } else {
            AceSeededNoise::new(req.seed).draw(n)
        };
        let latents0 = xt.clone();
        let src = self.cond.silence_slice(frames);
        let mut context = vec![0f32; frames * ACE_CONTEXT_DIM];
        for t in 0..frames {
            context[t * ACE_CONTEXT_DIM..t * ACE_CONTEXT_DIM + ACE_LATENT_DIM]
                .copy_from_slice(&src[t * ACE_LATENT_DIM..(t + 1) * ACE_LATENT_DIM]);
            for c in 0..ACE_LATENT_DIM {
                context[t * ACE_CONTEXT_DIM + ACE_LATENT_DIM + c] = 1.0;
            }
        }

        let sigmas = ace_sigmas(steps, shift);
        let mut velocity0 = Vec::new();
        let mut vt_uncond0 = Vec::new();
        let mut vt_guided0 = Vec::new();
        let mut last_vt_guided = Vec::new();
        let mut apg = AceApgMomentum::new(ACE_APG_MOMENTUM);
        let mut dit_total_s = 0.0f64;
        let do_apg = req.guidance > 1.0 && self.cond.null_condition_emb.is_some();
        let uncond = if do_apg {
            let null = self.cond.null_condition_emb.as_ref().unwrap();
            let dim = crate::ace::ACE_COND_DIM;
            let tokens = cond.tokens.max(1);
            Some(AceConditioning {
                encoder_hidden: null.iter().cycle().take(tokens * dim).copied().collect(),
                encoder_mask: vec![true; tokens],
                tokens,
            })
        } else {
            None
        };
        let cond_kv = if let Some(device) = self.dit_device.as_ref() {
            Some(self.dit.prepare_cross_kv(device, &cond)?)
        } else {
            None
        };
        let uncond_kv = match (self.dit_device.as_ref(), uncond.as_ref()) {
            (Some(device), Some(u)) => Some(self.dit.prepare_cross_kv(device, u)?),
            _ => None,
        };
        for step in 0..steps {
            emit_progress(
                progress,
                &format!("denoise {}/{steps}", step + 1),
                0.2 + 0.65 * (step as f64 / steps as f64),
            )?;
            let t = sigmas[step];
            let t_dit = Instant::now();
            let (vt_cond, vt_uncond_step) = if let Some(uncond) = uncond.as_ref() {
                let vt_cond = self.dit_forward_kv(
                    &xt, &context, frames, &cond, t, cond_kv.as_ref(),
                )?;
                let vt_uncond = self.dit_forward_kv(
                    &xt, &context, frames, uncond, t, uncond_kv.as_ref(),
                )?;
                (vt_cond, vt_uncond)
            } else {
                (
                    self.dit_forward_kv(&xt, &context, frames, &cond, t, cond_kv.as_ref())?,
                    Vec::new(),
                )
            };
            let step_s = t_dit.elapsed().as_secs_f64();
            dit_total_s += step_s;
            let forwards = if uncond.is_some() { 2 } else { 1 };
            if step < 2 {
                eprintln!(
                    "ace dit step={step} batch_cfg=false forwards={forwards} {step_s:.3}s"
                );
            }
            let (vt, vt_uncond_step) = if uncond.is_some() {
                let guided = ace_apg(
                    &vt_cond,
                    &vt_uncond_step,
                    frames,
                    ACE_LATENT_DIM,
                    req.guidance - 1.0,
                    &mut apg,
                    ACE_APG_ETA,
                    ACE_APG_NORM_THRESHOLD,
                );
                (guided, vt_uncond_step)
            } else {
                (vt_cond.clone(), vt_uncond_step)
            };
            if step == 0 {
                velocity0 = vt_cond;
                vt_uncond0 = vt_uncond_step;
                vt_guided0 = vt.clone();
            }
            last_vt_guided = vt.clone();
            let dt = sigmas[step + 1] - sigmas[step];
            for (x, v) in xt.iter_mut().zip(vt.iter()) {
                *x += dt * *v;
            }
        }

        let denoise_s = t_after_cond.elapsed().as_secs_f64();
        let forwards_per_step = if uncond.is_some() { 2 } else { 1 };
        eprintln!(
            "ace dit summary steps={steps} batch_cfg=false forwards_per_step={forwards_per_step} official_shape=50x1_batched dit_total={dit_total_s:.3}s mean={:.3}s",
            if steps > 0 { dit_total_s / steps as f64 } else { 0.0 }
        );
        emit_progress(progress, "vae-decode", 0.88)?;
        let t_vae = Instant::now();
        let (left, right) = self.decode_audio(&xt, frames)?;
        eprintln!(
            "ace timings cond={cond_s:.3}s denoise={denoise_s:.3}s vae={:.3}s",
            t_vae.elapsed().as_secs_f64()
        );
        if makepad_ai_common::backend::prof::enabled() {
            eprint!(
                "{}",
                makepad_ai_common::backend::prof::report_and_reset("ace prof vae ")
            );
        }
        let target = (seconds * ACE_SAMPLE_RATE as f64) as usize;
        let left = crop(&left, target);
        let right = crop(&right, target);
        emit_progress(progress, "done", 1.0)?;

        Ok(AceStageTaps {
            text_hidden,
            lyric_embed,
            encoder_hidden: cond.encoder_hidden,
            latents0,
            velocity0,
            vt_uncond0,
            vt_guided0,
            last_vt_guided,
            latents_final: xt,
            left,
            right,
            sample_rate: ACE_SAMPLE_RATE as u32,
        })
    }
}

fn crop(samples: &[f32], target: usize) -> Vec<f32> {
    if samples.len() >= target {
        samples[..target].to_vec()
    } else {
        let mut out = samples.to_vec();
        out.resize(target, 0.0);
        out
    }
}

pub fn ace_generate_with_progress(
    model_dir: impl AsRef<Path>,
    req: &AceGenerate,
    progress: &mut dyn FnMut(&str, f64),
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<f32>> {
    if cancel() {
        return Err(crate::DiffusionError::Cancelled);
    }
    let started = Instant::now();
    let paths = AcePaths::from_model_dir(model_dir);
    let mut load_hook = |label: &str, frac: f64| {
        if cancel() {
            return Err(crate::DiffusionError::Cancelled);
        }
        progress(label, 0.02 + 0.18 * frac);
        Ok(())
    };
    let pipe = AcePipeline::load(&paths, Some(&mut load_hook))?;
    if cancel() {
        return Err(crate::DiffusionError::Cancelled);
    }
    let mut gen_hook = |label: &str, frac: f64| {
        if cancel() {
            return Err(crate::DiffusionError::Cancelled);
        }
        progress(label, 0.20 + 0.75 * frac);
        Ok(())
    };
    let (left, right) = pipe.generate(req, Some(&mut gen_hook))?;
    let _ = (started, ACE_HOP);
    let mut interleaved = Vec::with_capacity(left.len() * 2);
    for i in 0..left.len() {
        interleaved.push(left[i]);
        interleaved.push(right.get(i).copied().unwrap_or(left[i]));
    }
    progress("done", 1.0);
    Ok(interleaved)
}

pub fn ace_planar_stereo(interleaved: &[f32]) -> Result<(Vec<f32>, Vec<f32>)> {
    if interleaved.len() % 2 != 0 {
        return Err(crate::DiffusionError::model(format!(
            "ace stereo: {} samples not even",
            interleaved.len()
        )));
    }
    let mut left = Vec::with_capacity(interleaved.len() / 2);
    let mut right = Vec::with_capacity(interleaved.len() / 2);
    for pair in interleaved.chunks_exact(2) {
        left.push(pair[0]);
        right.push(pair[1]);
    }
    Ok((left, right))
}
