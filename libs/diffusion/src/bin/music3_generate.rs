//! Replay-generate MiniMax-Music3 from a frozen oracle dump (cond hiddens +
//! step-0 noise) through native CUDA DiT + vocoder. Compares the wav to the
//! dump when present.
//!
//! Usage:
//!   music3-generate --weights <MiniMax-Music3> --dump <oracle dir> [--out song.wav] [--cpu-vocoder]

use makepad_diffusion::music3::{
    music3_latent_len, Music3ConditionEncoder, MUSIC3_DIT_IN_CHANNELS, MUSIC3_FLOW_CFG,
    MUSIC3_FLOW_STEPS, MUSIC3_NUM_CODEBOOKS, MUSIC3_SAMPLE_RATE,
};
use makepad_diffusion::music3_ar::music3_ar_replay;
use makepad_diffusion::music3_dit::{music3_dit_evict, music3_dit_sample, Music3DitPrepared};
use makepad_diffusion::music3_lm::{music3_lm_evict, Music3LmPrepared};
use makepad_diffusion::music3_rvq::{music3_rvq_evict, Music3RvqPrepared};
use makepad_diffusion::music3_vocoder::Music3Vocoder;
use makepad_diffusion::music3_weights::Music3Shards;
use std::path::{Path, PathBuf};
use std::time::Instant;

struct Npy {
    shape: Vec<usize>,
    descr: String,
    fortran_order: bool,
    data: Vec<u8>,
}

fn load_npy(path: &Path) -> Result<Npy, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() < 10 || &bytes[..6] != b"\x93NUMPY" {
        return Err(format!("{}: not npy", path.display()));
    }
    let major = bytes[6];
    let (header_len, header_start) = if major == 1 {
        (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10)
    } else {
        (
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
            12,
        )
    };
    let header = String::from_utf8_lossy(&bytes[header_start..header_start + header_len]).to_string();
    let descr = header
        .split("'descr':")
        .nth(1)
        .and_then(|r| r.split('\'').nth(1))
        .ok_or("no descr")?
        .to_string();
    let shape_text = header
        .split("'shape':")
        .nth(1)
        .and_then(|r| r.split('(').nth(1))
        .and_then(|r| r.split(')').next())
        .ok_or("no shape")?;
    let shape: Vec<usize> = shape_text
        .split(',')
        .filter_map(|p| p.trim().parse().ok())
        .collect();
    let fortran_order =
        header.contains("'fortran_order': True") || header.contains("'fortran_order':True");
    Ok(Npy {
        shape,
        descr,
        fortran_order,
        data: bytes[header_start + header_len..].to_vec(),
    })
}

impl Npy {
    fn as_i64(&self) -> Result<Vec<i64>, String> {
        let raw: Vec<i64> = match self.descr.as_str() {
            "<i8" => self
                .data
                .chunks_exact(8)
                .map(|c| i64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]))
                .collect(),
            "<i4" => self
                .data
                .chunks_exact(4)
                .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as i64)
                .collect(),
            _ => return Err(format!("descr {} not int", self.descr)),
        };
        if !self.fortran_order {
            return Ok(raw);
        }
        let n = raw.len();
        let mut out = vec![0i64; n];
        for c_idx in 0..n {
            let mut rest = c_idx;
            let mut coords = vec![0usize; self.shape.len()];
            for d in (0..self.shape.len()).rev() {
                coords[d] = rest % self.shape[d];
                rest /= self.shape[d];
            }
            let mut f_idx = 0usize;
            let mut stride = 1usize;
            for d in 0..self.shape.len() {
                f_idx += coords[d] * stride;
                stride *= self.shape[d];
            }
            out[c_idx] = raw[f_idx];
        }
        Ok(out)
    }

    fn as_f32(&self) -> Result<Vec<f32>, String> {
        let raw: Vec<f32> = match self.descr.as_str() {
            "<f4" => self
                .data
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
            _ => return Err(format!("descr {}", self.descr)),
        };
        if !self.fortran_order {
            return Ok(raw);
        }
        let n = raw.len();
        let mut out = vec![0f32; n];
        for c_idx in 0..n {
            let mut rest = c_idx;
            let mut coords = vec![0usize; self.shape.len()];
            for d in (0..self.shape.len()).rev() {
                coords[d] = rest % self.shape[d];
                rest /= self.shape[d];
            }
            let mut f_idx = 0usize;
            let mut stride = 1usize;
            for d in 0..self.shape.len() {
                f_idx += coords[d] * stride;
                stride *= self.shape[d];
            }
            out[c_idx] = raw[f_idx];
        }
        Ok(out)
    }
}

fn write_wav_i16(path: &Path, stereo: &[f32], frames: usize, sr: u32) -> Result<(), String> {
    let samples = stereo.len() / 2;
    let mut pcm = Vec::with_capacity(samples * 4);
    for i in 0..samples {
        for ch in 0..2 {
            let v = stereo[ch * frames.max(1) * (samples / frames.max(1)) + i];
            let s = (v.clamp(-1.0, 1.0) * 32767.0) as i16;
            pcm.extend_from_slice(&s.to_le_bytes());
        }
    }
    // stereo is [2, N] channel-major; interleave
    let n = stereo.len() / 2;
    pcm.clear();
    for i in 0..n {
        for ch in 0..2 {
            let s = (stereo[ch * n + i].clamp(-1.0, 1.0) * 32767.0) as i16;
            pcm.extend_from_slice(&s.to_le_bytes());
        }
    }
    let mut buf = Vec::new();
    buf.extend_from_slice(b"RIFF");
    let size = 36 + pcm.len() as u32;
    buf.extend_from_slice(&size.to_le_bytes());
    buf.extend_from_slice(b"WAVEfmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes());
    buf.extend_from_slice(&2u16.to_le_bytes());
    buf.extend_from_slice(&sr.to_le_bytes());
    buf.extend_from_slice(&(sr * 4).to_le_bytes());
    buf.extend_from_slice(&4u16.to_le_bytes());
    buf.extend_from_slice(&16u16.to_le_bytes());
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    buf.extend_from_slice(&pcm);
    std::fs::write(path, buf).map_err(|e| e.to_string())
}

fn snr_db(ours: &[f32], reference: &[f32]) -> f64 {
    let n = ours.len().min(reference.len());
    let mut num = 0f64;
    let mut den = 0f64;
    for i in 0..n {
        num += (reference[i] as f64).powi(2);
        den += (ours[i] as f64 - reference[i] as f64).powi(2);
    }
    10.0 * (num / den.max(1e-30)).log10()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut opts = std::collections::HashMap::new();
    let mut i = 1;
    while i < args.len() {
        if let Some(k) = args[i].strip_prefix("--") {
            if i + 1 < args.len() && !args[i + 1].starts_with("--") {
                opts.insert(k.to_string(), args[i + 1].clone());
                i += 2;
                continue;
            }
            opts.insert(k.to_string(), String::new());
        }
        i += 1;
    }
    let weights = PathBuf::from(opts.get("weights").cloned().unwrap_or_else(|| {
        r"C:\ai\asset_node_cache\music\MiniMax-Music3".into()
    }));
    let dump = PathBuf::from(
        opts.get("dump")
            .cloned()
            .unwrap_or_else(|| r"C:\ai\music3_oracle\pine_5s_seed7".into()),
    );
    let out = PathBuf::from(opts.get("out").cloned().unwrap_or_else(|| {
        dump.join("native_song.wav").to_string_lossy().into()
    }));
    let cpu_voc = opts.contains_key("cpu-vocoder");
    let ar = opts.contains_key("ar");
    if let Err(err) = run(&weights, &dump, &out, cpu_voc, ar) {
        eprintln!("music3-generate FAILED: {err}");
        std::process::exit(1);
    }
}

fn run(weights: &Path, dump: &Path, out: &Path, cpu_voc: bool, ar: bool) -> Result<(), String> {
    println!(
        "music3-generate dump={} path={}",
        dump.display(),
        if ar { "ar-replay" } else { "replay-hiddens" }
    );
    let total = Instant::now();

    let t0 = Instant::now();
    let (cond, frames, ar_s) = if ar {
        let ids = load_npy(&dump.join("text_ids.npy"))?;
        let t = ids.shape[1];
        let vals = ids.as_i64()?;
        let cond_ids: Vec<u32> = vals[..t].iter().map(|&v| v as u32).collect();
        let semantic: Vec<u32> = load_npy(&dump.join("semantic_codes.npy"))?
            .as_i64()?
            .iter()
            .map(|&v| v as u32)
            .collect();
        let resid: Vec<u32> = load_npy(&dump.join("rvq_codes.npy"))?
            .as_i64()?
            .iter()
            .map(|&v| v as u32)
            .collect();
        if resid.len() != semantic.len() * (MUSIC3_NUM_CODEBOOKS - 1) {
            return Err("ar dump residual/semantic mismatch".into());
        }
        let lm = Music3Shards::load(weights.join("language_model")).map_err(|e| e.to_string())?;
        let lm_prep = Music3LmPrepared::prepare(&lm).map_err(|e| e.to_string())?;
        let rvq = Music3Shards::load(weights.join("rvq_depth_decoder")).map_err(|e| e.to_string())?;
        let rvq_prep = Music3RvqPrepared::prepare(&rvq).map_err(|e| e.to_string())?;
        let ta = Instant::now();
        let hiddens = music3_ar_replay(&lm, &lm_prep, &rvq, &rvq_prep, &cond_ids, &semantic, &resid)
            .map_err(|e| e.to_string())?;
        let ar_s = ta.elapsed().as_secs_f64();
        let frames = hiddens.len() / (makepad_diffusion::music3::MUSIC3_COND_LAYERS
            * makepad_diffusion::music3::MUSIC3_COND_HIDDEN);
        println!(
            "  ar replay {frames} frames codes={} {:.3}s",
            semantic.len(),
            ar_s
        );
        let enc = Music3ConditionEncoder::load(weights).map_err(|e| e.to_string())?;
        let cond = enc.forward(&hiddens, frames).map_err(|e| e.to_string())?;
        (cond, frames, ar_s)
    } else {
        let hin = load_npy(&dump.join("cond_enc_in.npy"))?;
        let frames = hin.shape[1];
        (Vec::new(), frames, 0.0)
    };
    let (cond, frames) = if ar {
        (cond, frames)
    } else {
        let hin = load_npy(&dump.join("cond_enc_in.npy"))?;
        let frames = hin.shape[1];
        let enc = Music3ConditionEncoder::load(weights).map_err(|e| e.to_string())?;
        let cond = enc
            .forward(&hin.as_f32()?, frames)
            .map_err(|e| e.to_string())?;
        (cond, frames)
    };
    let tokens = music3_latent_len(frames);
    println!(
        "  cond {frames} frames -> {tokens} latents {:.3}s (ar={ar_s:.3}s)",
        t0.elapsed().as_secs_f64()
    );

    let noise = load_npy(&dump.join("dit_step0_x.npy"))?.as_f32()?;
    if noise.len() != MUSIC3_DIT_IN_CHANNELS * tokens {
        return Err(format!(
            "noise {} expected {}",
            noise.len(),
            MUSIC3_DIT_IN_CHANNELS * tokens
        ));
    }
    let shards = Music3Shards::load(weights.join("transformer")).map_err(|e| e.to_string())?;
    let prepared = Music3DitPrepared::prepare(&shards).map_err(|e| e.to_string())?;
    let t1 = Instant::now();
    let latents = music3_dit_sample(
        &shards,
        &prepared,
        &noise,
        &cond,
        tokens,
        MUSIC3_FLOW_STEPS,
        MUSIC3_FLOW_CFG,
    )
    .map_err(|e| e.to_string())?;
    let dit_cold = t1.elapsed().as_secs_f64();
    println!("  dit {MUSIC3_FLOW_STEPS} steps cfg={MUSIC3_FLOW_CFG} cold {dit_cold:.3}s");

    let voc = Music3Vocoder::load(weights).map_err(|e| e.to_string())?;
    let t2 = Instant::now();
    let audio = if cpu_voc {
        voc.decode(&latents, tokens).map_err(|e| e.to_string())?
    } else {
        voc.decode_cuda(&latents, tokens).map_err(|e| e.to_string())?
    };
    let voc_s = t2.elapsed().as_secs_f64();
    println!(
        "  vocoder {} {voc_s:.3}s",
        if cpu_voc { "cpu" } else { "cuda" }
    );

    let t3 = Instant::now();
    if ar {
        let ids = load_npy(&dump.join("text_ids.npy"))?;
        let t = ids.shape[1];
        let vals = ids.as_i64()?;
        let cond_ids: Vec<u32> = vals[..t].iter().map(|&v| v as u32).collect();
        let semantic: Vec<u32> = load_npy(&dump.join("semantic_codes.npy"))?
            .as_i64()?
            .iter()
            .map(|&v| v as u32)
            .collect();
        let resid: Vec<u32> = load_npy(&dump.join("rvq_codes.npy"))?
            .as_i64()?
            .iter()
            .map(|&v| v as u32)
            .collect();
        let lm = Music3Shards::load(weights.join("language_model")).map_err(|e| e.to_string())?;
        let lm_prep = Music3LmPrepared::prepare(&lm).map_err(|e| e.to_string())?;
        let rvq = Music3Shards::load(weights.join("rvq_depth_decoder")).map_err(|e| e.to_string())?;
        let rvq_prep = Music3RvqPrepared::prepare(&rvq).map_err(|e| e.to_string())?;
        let tw = Instant::now();
        let hiddens = music3_ar_replay(&lm, &lm_prep, &rvq, &rvq_prep, &cond_ids, &semantic, &resid)
            .map_err(|e| e.to_string())?;
        let ar_warm = tw.elapsed().as_secs_f64();
        let enc = Music3ConditionEncoder::load(weights).map_err(|e| e.to_string())?;
        let cond_w = enc
            .forward(
                &hiddens,
                hiddens.len()
                    / (makepad_diffusion::music3::MUSIC3_COND_LAYERS
                        * makepad_diffusion::music3::MUSIC3_COND_HIDDEN),
            )
            .map_err(|e| e.to_string())?;
        let _ = music3_dit_sample(
            &shards,
            &prepared,
            &noise,
            &cond_w,
            tokens,
            MUSIC3_FLOW_STEPS,
            MUSIC3_FLOW_CFG,
        )
        .map_err(|e| e.to_string())?;
        let audio2 = if cpu_voc {
            voc.decode(&latents, tokens).map_err(|e| e.to_string())?
        } else {
            voc.decode_cuda(&latents, tokens).map_err(|e| e.to_string())?
        };
        let _ = audio2;
        println!(
            "  warm ar {:.3}s + dit+vocoder full {:.3}s (weights resident)",
            ar_warm,
            t3.elapsed().as_secs_f64()
        );
        let _ = music3_lm_evict();
        let _ = music3_rvq_evict();
    } else {
        let _ = music3_dit_sample(
            &shards,
            &prepared,
            &noise,
            &cond,
            tokens,
            MUSIC3_FLOW_STEPS,
            MUSIC3_FLOW_CFG,
        )
        .map_err(|e| e.to_string())?;
        let audio2 = if cpu_voc {
            voc.decode(&latents, tokens).map_err(|e| e.to_string())?
        } else {
            voc.decode_cuda(&latents, tokens).map_err(|e| e.to_string())?
        };
        let _ = audio2;
        println!(
            "  warm dit+vocoder {:.3}s (weights resident)",
            t3.elapsed().as_secs_f64()
        );
    }
    let _ = music3_dit_evict();
    write_wav_i16(out, &audio, tokens, MUSIC3_SAMPLE_RATE as u32)?;
    println!("  wrote {} samples={}", out.display(), audio.len() / 2);

    if dump.join("audio.npy").is_file() {
        let reference = load_npy(&dump.join("audio.npy"))?.as_f32()?;
        let n = audio.len().min(reference.len());
        let mut max_abs = 0f32;
        for i in 0..n {
            max_abs = max_abs.max((audio[i] - reference[i]).abs());
        }
        println!(
            "  vs oracle wav snr={:.2} dB max_abs={:.3e}",
            snr_db(&audio, &reference),
            max_abs
        );
    }
    println!("  total {:.3}s", total.elapsed().as_secs_f64());
    Ok(())
}
