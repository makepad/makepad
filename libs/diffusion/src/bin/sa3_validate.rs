//! SA3 Small SFX stage validation against the stable-audio-3 oracle dumps
//! (local/sa3_ref/sa3_dump.py -> local/sa3_ref/dumps/<fixture>). Compares,
//! per stage, the CPU port against the reference tensors: schedule, text
//! encoder, conditioning embeds, DiT forward-0 velocity, the full 8-step
//! noise-replayed sampling trajectory, and the AE decode.
//!
//! Usage:
//!   sa3-validate [--dump <dir>] [--weights <dir>] [--stage sigmas|te|cond|dit|sample|ae|all]

use makepad_diffusion::sa3::{sa3_sigmas, SA3_LATENT_DIM};
use makepad_diffusion::sa3_pipeline::{Sa3NoiseSource, Sa3Pipeline};
use makepad_diffusion::sa3_text::apply_learned_padding;
use makepad_diffusion::sa3_transformer::Sa3PadMode;
use std::path::{Path, PathBuf};

// --- minimal .npy reader (same pattern as h3_validate) ---------------------

struct Npy {
    shape: Vec<usize>,
    descr: String,
    data: Vec<u8>,
}

fn load_npy(path: &Path) -> Result<Npy, String> {
    let bytes = std::fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?;
    if bytes.len() < 10 || &bytes[..6] != b"\x93NUMPY" {
        return Err(format!("{}: not an npy file", path.display()));
    }
    let major = bytes[6];
    let (header_len, header_start) = if major == 1 {
        (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10usize)
    } else {
        (
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
            12usize,
        )
    };
    let header =
        String::from_utf8_lossy(&bytes[header_start..header_start + header_len]).to_string();
    let descr = header
        .split("'descr':")
        .nth(1)
        .and_then(|rest| rest.split('\'').nth(1))
        .ok_or_else(|| format!("{}: no descr", path.display()))?
        .to_string();
    let shape_text = header
        .split("'shape':")
        .nth(1)
        .and_then(|rest| rest.split('(').nth(1))
        .and_then(|rest| rest.split(')').next())
        .ok_or_else(|| format!("{}: no shape", path.display()))?;
    let shape: Vec<usize> = shape_text
        .split(',')
        .filter_map(|part| part.trim().parse::<usize>().ok())
        .collect();
    Ok(Npy {
        shape,
        descr,
        data: bytes[header_start + header_len..].to_vec(),
    })
}

impl Npy {
    fn as_f32(&self) -> Result<Vec<f32>, String> {
        match self.descr.as_str() {
            "<f4" => Ok(self
                .data
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect()),
            "<f8" => Ok(self
                .data
                .chunks_exact(8)
                .map(|c| {
                    f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]) as f32
                })
                .collect()),
            "<i8" => Ok(self
                .data
                .chunks_exact(8)
                .map(|c| {
                    i64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]) as f32
                })
                .collect()),
            other => Err(format!("npy descr {other} not f32-convertible")),
        }
    }
}

// --- comparison helpers -----------------------------------------------------

struct Cmp {
    cos: f64,
    max_abs: f32,
    mean_abs: f32,
    ref_absmax: f32,
}

fn compare(ours: &[f32], reference: &[f32]) -> Cmp {
    assert_eq!(ours.len(), reference.len(), "length mismatch");
    let mut dot = 0f64;
    let mut na = 0f64;
    let mut nb = 0f64;
    let mut max_abs = 0f32;
    let mut sum_abs = 0f64;
    let mut ref_absmax = 0f32;
    for (&a, &b) in ours.iter().zip(reference) {
        dot += a as f64 * b as f64;
        na += a as f64 * a as f64;
        nb += b as f64 * b as f64;
        let d = (a - b).abs();
        if d > max_abs {
            max_abs = d;
        }
        sum_abs += d as f64;
        if b.abs() > ref_absmax {
            ref_absmax = b.abs();
        }
    }
    Cmp {
        cos: dot / (na.sqrt() * nb.sqrt()).max(1e-30),
        max_abs,
        mean_abs: (sum_abs / ours.len() as f64) as f32,
        ref_absmax,
    }
}

fn report(name: &str, cmp: &Cmp) {
    println!(
        "  {name:<24} cos={:.7} max_abs={:.3e} mean_abs={:.3e} (ref absmax {:.3})",
        cmp.cos, cmp.max_abs, cmp.mean_abs, cmp.ref_absmax
    );
}

/// Dump tensors are (1, C, T) channel-major; the port uses [T, C] token-major.
fn to_tokens_major(data: &[f32], channels: usize, tokens: usize) -> Vec<f32> {
    let mut out = vec![0f32; data.len()];
    for c in 0..channels {
        for t in 0..tokens {
            out[t * channels + c] = data[c * tokens + t];
        }
    }
    out
}

struct DumpNoise {
    draws: Vec<Vec<f32>>,
}

impl Sa3NoiseSource for DumpNoise {
    fn draw(&mut self, index: usize, len: usize) -> Vec<f32> {
        let v = self.draws[index].clone();
        assert_eq!(v.len(), len, "dump noise draw {index} length");
        v
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut opts = std::collections::HashMap::new();
    let mut i = 1;
    while i + 1 < args.len() {
        if let Some(key) = args[i].strip_prefix("--") {
            opts.insert(key.to_string(), args[i + 1].clone());
        }
        i += 2;
    }
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dump = PathBuf::from(
        opts.get("dump")
            .cloned()
            .unwrap_or_else(|| repo.join("local/sa3_ref/dumps/sword_clash").to_string_lossy().into_owned()),
    );
    let weights = PathBuf::from(
        opts.get("weights")
            .cloned()
            .unwrap_or_else(|| {
                repo.join("local/sa3_ref/weights/stable-audio-3-small-sfx")
                    .to_string_lossy()
                    .into_owned()
            }),
    );
    let stage = opts.get("stage").cloned().unwrap_or_else(|| "all".to_string());
    if let Err(err) = run(&dump, &weights, &stage) {
        eprintln!("sa3-validate FAILED: {err}");
        std::process::exit(1);
    }
}

fn run(dump: &Path, weights: &Path, stage: &str) -> Result<(), String> {
    let load = |name: &str| load_npy(&dump.join(format!("{name}.npy")));
    let all = stage == "all";

    // Fixture geometry from the dump itself.
    let noise_init = load("noise_init")?;
    let latent_len = noise_init.shape[2];
    let sigmas_ref = load("sigmas")?.as_f32()?;
    let steps = sigmas_ref.len() - 1;
    let padding_mask = load("padding_mask")?.as_f32()?;
    let valid_len = padding_mask.iter().filter(|&&v| v > 0.5).count();
    println!(
        "fixture: {} latents={latent_len} valid={valid_len} steps={steps}",
        dump.display()
    );

    if all || stage == "sigmas" {
        println!("[sigmas]");
        let ours = sa3_sigmas(steps);
        report("sigmas", &compare(&ours, &sigmas_ref));
    }

    let t0 = std::time::Instant::now();
    let pipeline = Sa3Pipeline::load(
        weights.join("model.safetensors"),
        weights.join("t5gemma-b-b-ul2/model.safetensors"),
        None,
    )
    .map_err(|err| format!("load: {err:?}"))?;
    println!("weights loaded in {:.1}s", t0.elapsed().as_secs_f32());

    let tok_ids: Vec<u32> = load("tok_ids")?
        .as_f32()?
        .iter()
        .map(|&v| v as u32)
        .collect();
    let tok_mask: Vec<bool> = load("tok_mask")?
        .as_f32()?
        .iter()
        .map(|&v| v > 0.5)
        .collect();

    // seconds is recoverable from valid/effective lengths: not needed —
    // conditioning uses the dumped seconds embedding path via known duration.
    // The fixtures encode it in their names; both dumps carry it implicitly.
    // We reconstruct: effective = valid - 64 headroom, seconds ~ effective.
    // Instead, read the exact seconds from the fixture geometry:
    let seconds = match dump.file_name().and_then(|n| n.to_str()) {
        Some("sword_clash") => 4.0,
        Some("coin_pickup") => 2.0,
        other => {
            return Err(format!(
                "unknown fixture {other:?}: add its duration mapping"
            ))
        }
    };

    if all || stage == "te" {
        println!("[te] T5Gemma encoder");
        let t0 = std::time::Instant::now();
        let hidden = pipeline
            .text
            .encode(&tok_ids, &tok_mask)
            .map_err(|err| format!("te encode: {err:?}"))?;
        println!("  encode {:.2}s", t0.elapsed().as_secs_f32());
        let te_raw = load("te_hidden_raw")?.as_f32()?;
        report("te_hidden_raw", &compare(&hidden, &te_raw));
        let mut padded = hidden.clone();
        let pad_emb = load("padding_embedding")?.as_f32()?;
        apply_learned_padding(&mut padded, &tok_mask, &pad_emb);
        let te_padded = load("te_hidden")?.as_f32()?;
        report("te_hidden(padded)", &compare(&padded, &te_padded));
    }

    // Our full-pipeline conditioning (own TE, f32 — reference TE ran bf16).
    let cond_own = pipeline
        .conditioning(&tok_ids, &tok_mask, seconds)
        .map_err(|err| format!("conditioning: {err:?}"))?;

    // Dump-based conditioning: isolates the DiT stages from TE bf16 noise.
    let dump_cross = load("cross_attn_cond")?.as_f32()?;
    let dump_seconds = load("seconds_emb")?.as_f32()?;
    let cond = makepad_diffusion::sa3_pipeline::Sa3Conditioning {
        cond_embed: pipeline.dit.embed_conditioning(&dump_cross),
        global_embed: pipeline.dit.embed_global(&dump_seconds),
        cross_attn_cond: dump_cross.clone(),
        seconds_emb: dump_seconds.clone(),
    };

    if all || stage == "cond" {
        println!("[cond] conditioning embeds");
        report(
            "seconds_emb",
            &compare(&cond_own.seconds_emb, &dump_seconds),
        );
        report(
            "cross_attn_cond(own TE)",
            &compare(&cond_own.cross_attn_cond, &dump_cross),
        );
        report(
            "cond_embed(dump cross)",
            &compare(&cond.cond_embed, &load("cond_embed")?.as_f32()?),
        );
        report(
            "global_embed_pre_t",
            &compare(&cond.global_embed, &load("global_embed_pre_t")?.as_f32()?),
        );
        let temb = pipeline.dit.embed_timestep(sigmas_ref[0]);
        report(
            "timestep_embed(t=1)",
            &compare(&temb, &load("timestep_embed")?.as_f32()?),
        );
    }

    if all || stage == "dit" {
        println!("[dit] forward step 0");
        let x0 = to_tokens_major(&noise_init.as_f32()?, SA3_LATENT_DIM, latent_len);
        let t0 = std::time::Instant::now();
        let v = pipeline
            .dit
            .forward(
                &x0,
                sigmas_ref[0],
                &cond.cond_embed,
                &cond.global_embed,
                latent_len,
                valid_len,
                Sa3PadMode::VZero,
            )
            .map_err(|err| format!("dit forward: {err:?}"))?;
        println!("  forward {:.2}s", t0.elapsed().as_secs_f32());
        let v_ref = to_tokens_major(&load("step0_v")?.as_f32()?, SA3_LATENT_DIM, latent_len);
        report("step0_v", &compare(&v, &v_ref));
    }

    if all || stage == "sample" {
        println!("[sample] {steps}-step noise-replayed trajectory");
        let mut draws = vec![to_tokens_major(
            &noise_init.as_f32()?,
            SA3_LATENT_DIM,
            latent_len,
        )];
        for i in 0..steps - 1 {
            draws.push(to_tokens_major(
                &load(&format!("step{i}_noise"))?.as_f32()?,
                SA3_LATENT_DIM,
                latent_len,
            ));
        }
        let mut noise = DumpNoise { draws };
        let t0 = std::time::Instant::now();
        let latents = pipeline
            .sample(
                &cond,
                latent_len,
                valid_len,
                steps,
                Sa3PadMode::VZero,
                &mut noise,
                None,
                None,
            )
            .map_err(|err| format!("sample: {err:?}"))?;
        println!("  {steps} steps in {:.2}s", t0.elapsed().as_secs_f32());
        let latents_ref =
            to_tokens_major(&load("latents_final")?.as_f32()?, SA3_LATENT_DIM, latent_len);
        report("latents_final", &compare(&latents, &latents_ref));

        println!("[ae] decode of our sampled latents vs reference audio");
        let t0 = std::time::Instant::now();
        let audio = pipeline
            .decode(&latents, latent_len)
            .map_err(|err| format!("ae decode: {err:?}"))?;
        println!("  decode {:.2}s", t0.elapsed().as_secs_f32());
        let audio_ref = load("audio")?.as_f32()?;
        let samples = audio_ref.len() / 2;
        let mut ours = Vec::with_capacity(audio_ref.len());
        ours.extend_from_slice(&audio[0]);
        ours.extend_from_slice(&audio[1]);
        assert_eq!(ours.len(), samples * 2);
        report("audio(e2e)", &compare(&ours, &audio_ref));
    }

    if (all || stage == "ae") && stage != "sample" {
        println!("[ae] decode of REFERENCE latents (isolates the AE)");
        let latents_ref =
            to_tokens_major(&load("latents_final")?.as_f32()?, SA3_LATENT_DIM, latent_len);
        let t0 = std::time::Instant::now();
        let audio = pipeline
            .decode(&latents_ref, latent_len)
            .map_err(|err| format!("ae decode: {err:?}"))?;
        println!("  decode {:.2}s", t0.elapsed().as_secs_f32());
        let audio_ref = load("audio")?.as_f32()?;
        let mut ours = Vec::with_capacity(audio_ref.len());
        ours.extend_from_slice(&audio[0]);
        ours.extend_from_slice(&audio[1]);
        report("audio(ae-only)", &compare(&ours, &audio_ref));
        let bn_ref = load("bottleneck_out")?.as_f32()?;
        let _ = bn_ref; // bottleneck is a scalar multiply; covered by audio.
    }

    println!("sa3-validate done");
    Ok(())
}
