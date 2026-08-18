//! Prompt+lyrics → stereo wav through the native Music3 stack.
//!
//! Sampling RNG is a product xorshift, not torch. The clip is a valid
//! Music3 song; it is not bit-identical to the Python ModularPipeline.

use crate::music3::{
    load_tokenizer, music3_max_frames, tokenize_cfg_pair, Music3ConditionEncoder,
    MUSIC3_AR_CFG, MUSIC3_AUDIO_CHANNELS, MUSIC3_CHUNK_FRAMES, MUSIC3_CHUNK_HOP, MUSIC3_COND_HIDDEN,
    MUSIC3_COND_LAYERS, MUSIC3_COND_OUT, MUSIC3_CROP_LEFT_LATENT, MUSIC3_CROP_RIGHT_LATENT,
    MUSIC3_DIT_IN_CHANNELS, MUSIC3_FLOW_CFG, MUSIC3_FLOW_STEPS, MUSIC3_FRAME_RATE,
    MUSIC3_MIN_SECONDS, MUSIC3_OVERLAP_LATENT, MUSIC3_SAMPLE_RATE, MUSIC3_VAE_UPSAMPLE,
};
use crate::music3_ar::music3_ar_sample_with_progress;
use crate::music3_dit::{music3_dit_sample_window, Music3DitPrepared};
use crate::music3_lm::Music3LmPrepared;
use crate::music3_rvq::Music3RvqPrepared;
use crate::music3_vocoder::Music3Vocoder;
use crate::music3_weights::Music3Shards;
use crate::{DiffusionError, Result};
use std::path::Path;

pub struct Music3Generate {
    pub caption: String,
    pub lyrics: String,
    pub seconds: f64,
    pub seed: u64,
}

/// Planar stereo `[2, samples]` at 44.1 kHz.
pub fn music3_generate(model_dir: &Path, req: &Music3Generate) -> Result<Vec<f32>> {
    music3_generate_with_progress(model_dir, req, &mut |_, _| {}, &|| false)
}

pub fn music3_generate_with_progress(
    model_dir: &Path,
    req: &Music3Generate,
    progress: &mut dyn FnMut(&str, f64),
    should_cancel: &dyn Fn() -> bool,
) -> Result<Vec<f32>> {
    let seconds = req.seconds.max(MUSIC3_MIN_SECONDS);
    let max_frames = music3_max_frames(seconds);
    let min_frames = music3_max_frames(MUSIC3_MIN_SECONDS);
    progress("tokenize", 0.02);
    let tokenizer = load_tokenizer(&model_dir.join("tokenizer"))?;
    let pairs = tokenize_cfg_pair(&tokenizer, &req.caption, &req.lyrics)?;
    let cond_ids: Vec<u32> = pairs.iter().map(|p| p[0]).collect();
    let uncond_ids: Vec<u32> = pairs.iter().map(|p| p[1]).collect();
    progress(
        &format!(
            "prompt {} tok cap={} lyr={}",
            pairs.len(),
            req.caption.chars().count(),
            req.lyrics.chars().count()
        ),
        0.03,
    );

    progress("load lm", 0.04);
    let bench = std::env::var_os("MAKEPAD_MUSIC3_BENCH").is_some();
    let t_load = std::time::Instant::now();
    let lm = Music3Shards::load(model_dir.join("language_model"))?;
    let lm_prep = Music3LmPrepared::prepare(&lm)?;
    let rvq = Music3Shards::load(model_dir.join("rvq_depth_decoder"))?;
    let rvq_prep = Music3RvqPrepared::prepare(&rvq)?;
    if bench {
        eprintln!("BENCH lm_load_wall={:.3}", t_load.elapsed().as_secs_f64());
    }
    progress("prefill", 0.06);
    let t_ar = std::time::Instant::now();
    let hiddens = music3_ar_sample_with_progress(
        &lm,
        &lm_prep,
        &rvq,
        &rvq_prep,
        &cond_ids,
        &uncond_ids,
        max_frames,
        min_frames,
        req.seed,
        should_cancel,
        &mut |stage, k, n| {
            let n = n.max(1) as f64;
            let u = (k as f64 / n).clamp(0.0, 1.0);
            match stage {
                // One bar for the whole song: prefill is a few percent, AR is
                // the long smear (6% → 88%). Phase-local 0–100 reset made
                // prefill look finished before the song started.
                "prefill-cond" | "prefill-uncond" => {
                    progress(&format!("prefill {k}/{}", n as usize), 0.04 + 0.06 * u)
                }
                _ => progress(&format!("AR {k}/{} frames", n as usize), 0.10 + 0.78 * u),
            }
        },
    )?;
    let frames = hiddens.len() / (MUSIC3_COND_LAYERS * MUSIC3_COND_HIDDEN);
    if bench {
        eprintln!(
            "BENCH ar_wall={:.3} frames={frames}",
            t_ar.elapsed().as_secs_f64()
        );
    }
    if frames == 0 {
        return Err(DiffusionError::workflow("music3 generate: no AR frames"));
    }
    // After early <|audio_end|> the AR bar is still k/requested_max (e.g.
    // 260/1500 ≈ 20%). Do not snap to a hardcoded 88% — smear the tail
    // from wherever AR actually stopped.
    let after_ar = (0.06 + 0.82 * (frames as f64 / max_frames.max(1) as f64)).clamp(0.06, 0.88);
    progress("cond", after_ar);
    let t_render = std::time::Instant::now();
    let enc = Music3ConditionEncoder::load(model_dir)?;
    let dit0 = after_ar + 0.40 * (0.96 - after_ar).max(0.02);
    progress("dit", dit0);
    let dit = Music3Shards::load(model_dir.join("transformer"))?;
    let dit_prep = Music3DitPrepared::prepare(&dit)?;
    let voc = Music3Vocoder::load(model_dir)?;
    let audio = official_chunk_denoise_decode(
        &enc,
        &dit,
        &dit_prep,
        &voc,
        &hiddens,
        frames,
        req.seed,
        after_ar,
        dit0,
        should_cancel,
        progress,
    )?;
    let _ = (MUSIC3_AR_CFG, MUSIC3_FRAME_RATE, MUSIC3_SAMPLE_RATE);
    if bench {
        eprintln!(
            "BENCH render_wall={:.3} samples={}",
            t_render.elapsed().as_secs_f64(),
            audio.len() / 2
        );
    }
    progress("done", 1.0);
    Ok(audio)
}

/// Render fused AR hiddens `[frames, 8*4096]` through the exact
/// `music3_generate` tail (cond encoder → chunked DiT → vocoder), so a code
/// replay shares the same handoff as the sampled path. Planar `[2, samples]`.
pub fn music3_render_hiddens(
    model_dir: &Path,
    hiddens: &[f32],
    seed: u64,
    progress: &mut dyn FnMut(&str, f64),
) -> Result<Vec<f32>> {
    let width = MUSIC3_COND_LAYERS * MUSIC3_COND_HIDDEN;
    let frames = if width == 0 { 0 } else { hiddens.len() / width };
    if frames == 0 || hiddens.len() != frames * width {
        return Err(DiffusionError::model("music3 render hiddens width"));
    }
    let enc = Music3ConditionEncoder::load(model_dir)?;
    let dit = Music3Shards::load(model_dir.join("transformer"))?;
    let dit_prep = Music3DitPrepared::prepare(&dit)?;
    let voc = Music3Vocoder::load(model_dir)?;
    official_chunk_denoise_decode(
        &enc,
        &dit,
        &dit_prep,
        &voc,
        hiddens,
        frames,
        seed,
        0.06,
        0.5,
        &|| false,
        progress,
    )
}

/// Official PrepareChunks + ChunkDenoise + VocoderDecode.
/// `chunk_starts = [0]` if `frames <= 200` else `range(0, frames-100, 100)`.
/// Each window is re-encoded, overlap-carried from `[L-344, L-172)`, then
/// vocoded and crop-stitched (`left=86`, `right=258` latents).
fn official_chunk_starts(frames: usize) -> Vec<usize> {
    if frames <= MUSIC3_CHUNK_FRAMES {
        vec![0]
    } else {
        (0..frames.saturating_sub(MUSIC3_CHUNK_HOP))
            .step_by(MUSIC3_CHUNK_HOP)
            .collect()
    }
}

fn official_chunk_denoise_decode(
    enc: &Music3ConditionEncoder,
    dit: &Music3Shards,
    prep: &Music3DitPrepared,
    voc: &Music3Vocoder,
    hiddens: &[f32],
    frames: usize,
    seed: u64,
    after_ar: f64,
    dit0: f64,
    should_cancel: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(&str, f64),
) -> Result<Vec<f32>> {
    let width = MUSIC3_COND_LAYERS * MUSIC3_COND_HIDDEN;
    let starts = official_chunk_starts(frames);
    let n_chunks = starts.len().max(1);
    let hop: usize = MUSIC3_VAE_UPSAMPLE.iter().product();
    let mut left_ch = Vec::new();
    let mut right_ch = Vec::new();
    let mut prev_lat: Option<Vec<f32>> = None;
    let mut prev_cond: Option<Vec<f32>> = None;
    for (k, &chunk_start) in starts.iter().enumerate() {
        if should_cancel() {
            return Err(DiffusionError::Cancelled);
        }
        let chunk_end = (chunk_start + MUSIC3_CHUNK_FRAMES).min(frames);
        let n_frames = chunk_end - chunk_start;
        if n_frames == 0 {
            continue;
        }
        let hid = &hiddens[chunk_start * width..chunk_end * width];
        let w0 = after_ar + (0.99 - after_ar).max(0.01) * (k as f64 / n_chunks as f64);
        let w1 = after_ar + (0.99 - after_ar).max(0.01) * ((k + 1) as f64 / n_chunks as f64);
        let band = (w1 - w0).max(0.01);
        let cond = enc.forward_with_progress(hid, n_frames, &mut |phase, i, n| {
            let n = n.max(1) as f64;
            let u = (i as f64 / n).clamp(0.0, 1.0);
            let frac = match phase {
                "mix" => w0 + 0.15 * band * u,
                _ => w0 + 0.15 * band + 0.15 * band * u,
            };
            progress(
                &format!("cond {phase} {i}/{} w{}/{}", n as usize, k + 1, n_chunks),
                frac,
            );
        })?;
        let tokens = cond.len() / MUSIC3_COND_OUT;
        if tokens == 0 || cond.len() != tokens * MUSIC3_COND_OUT {
            return Err(DiffusionError::model("official chunk cond width"));
        }
        let mut cond = cond;
        let mut overlap = 0usize;
        if let (Some(pl), Some(pc)) = (prev_lat.as_ref(), prev_cond.as_ref()) {
            overlap = (pl.len() / MUSIC3_DIT_IN_CHANNELS).min(tokens);
            overlap = overlap.min(pc.len() / MUSIC3_COND_OUT);
            for t in 0..overlap {
                cond[t * MUSIC3_COND_OUT..(t + 1) * MUSIC3_COND_OUT]
                    .copy_from_slice(&pc[t * MUSIC3_COND_OUT..(t + 1) * MUSIC3_COND_OUT]);
            }
        }
        let noise = gaussian_noise(
            MUSIC3_DIT_IN_CHANNELS * tokens,
            seed ^ (0x9E37_79B9_7F4A_7C15 + k as u64),
        );
        let mut lat = music3_dit_sample_window(
            dit,
            prep,
            &noise,
            &cond,
            tokens,
            MUSIC3_FLOW_STEPS,
            MUSIC3_FLOW_CFG,
            overlap,
            prev_lat.as_deref(),
            &mut |step, steps| {
                if should_cancel() {
                    return;
                }
                let steps = steps.max(1);
                let chunk_frac = step as f64 / steps as f64;
                progress(
                    &format!("DiT {step}/{steps} w{}/{}", k + 1, n_chunks),
                    w0 + 0.30 * band + 0.65 * band * chunk_frac,
                );
            },
        )?;
        if overlap > 0 {
            if let Some(pl) = prev_lat.as_ref() {
                for c in 0..MUSIC3_DIT_IN_CHANNELS {
                    lat[c * tokens..c * tokens + overlap]
                        .copy_from_slice(&pl[c * overlap..c * overlap + overlap]);
                }
            }
        }
        let carry_start = tokens.saturating_sub(2 * MUSIC3_OVERLAP_LATENT);
        let carry_end = tokens.saturating_sub(MUSIC3_OVERLAP_LATENT).max(carry_start);
        let carry_n = carry_end - carry_start;
        if carry_n > 0 {
            let mut next_lat = vec![0f32; MUSIC3_DIT_IN_CHANNELS * carry_n];
            let mut next_cond = vec![0f32; carry_n * MUSIC3_COND_OUT];
            for c in 0..MUSIC3_DIT_IN_CHANNELS {
                next_lat[c * carry_n..c * carry_n + carry_n]
                    .copy_from_slice(&lat[c * tokens + carry_start..c * tokens + carry_end]);
            }
            next_cond.copy_from_slice(
                &cond[carry_start * MUSIC3_COND_OUT..carry_end * MUSIC3_COND_OUT],
            );
            prev_lat = Some(next_lat);
            prev_cond = Some(next_cond);
        } else {
            prev_lat = None;
            prev_cond = None;
        }
        progress(&format!("vocoder w{}/{}", k + 1, n_chunks), w1.min(0.99));
        let wave = voc.decode_cuda(&lat, tokens)?;
        let samples = tokens * hop;
        let left_crop = if k == 0 { 0 } else { MUSIC3_CROP_LEFT_LATENT * hop };
        let right_crop = if k + 1 == n_chunks {
            0
        } else {
            MUSIC3_CROP_RIGHT_LATENT * hop
        };
        if left_crop + right_crop >= samples {
            return Err(DiffusionError::model("official vocoder crop emptied window"));
        }
        let keep = samples - left_crop - right_crop;
        left_ch.extend_from_slice(&wave[left_crop..left_crop + keep]);
        right_ch.extend_from_slice(&wave[samples + left_crop..samples + left_crop + keep]);
    }
    if left_ch.is_empty() || left_ch.len() != right_ch.len() {
        return Err(DiffusionError::workflow("music3 official stitch empty"));
    }
    let mut out = Vec::with_capacity(left_ch.len() + right_ch.len());
    out.extend_from_slice(&left_ch);
    out.extend_from_slice(&right_ch);
    Ok(out)
}

fn gaussian_noise(n: usize, seed: u64) -> Vec<f32> {
    let mut s = seed | 1;
    let mut out = vec![0f32; n];
    let mut i = 0;
    while i < n {
        let u1 = xorshift_f32(&mut s).max(1e-7);
        let u2 = xorshift_f32(&mut s);
        let r = (-2.0 * u1.ln()).sqrt();
        let theta = 2.0 * std::f32::consts::PI * u2;
        out[i] = r * theta.cos();
        if i + 1 < n {
            out[i + 1] = r * theta.sin();
        }
        i += 2;
    }
    out
}

fn xorshift_f32(state: &mut u64) -> f32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    (x as f32) / (u64::MAX as f32)
}

/// Split planar `[2, N]` vocoder output into left/right.
pub fn music3_planar_stereo(audio: &[f32]) -> Result<(Vec<f32>, Vec<f32>)> {
    if audio.len() < 2 || audio.len() % MUSIC3_AUDIO_CHANNELS != 0 {
        return Err(DiffusionError::model("music3 stereo layout"));
    }
    let n = audio.len() / MUSIC3_AUDIO_CHANNELS;
    let hop: usize = MUSIC3_VAE_UPSAMPLE.iter().product();
    let _ = hop;
    Ok((audio[..n].to_vec(), audio[n..].to_vec()))
}
