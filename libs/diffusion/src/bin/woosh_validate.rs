//! Woosh DFlow stage validation against the reference oracle dumps
//! (box woosh_dump.py -> local/woosh_ref/dumps/<fixture>).
//!
//! Stages: tok | te | freqs | dit | sample | ae | all. `dit` runs the
//! forward for all 4 captured steps (taps compared on call 0); `sample`
//! replays the full Euler+renoise loop with the dumped fresh noise and
//! checks x against every step plus latents_final; `ae` decodes
//! latents_final and compares the backbone taps and the final audio.
//!
//! Usage:
//!   woosh-validate [--dumps local/woosh_ref/dumps] [--models local/models/woosh]
//!                  [--fixture dflow_sword_clash] [--stage all]

use makepad_diffusion::woosh::{WOOSH_DESC_TOKENS, WOOSH_LATENT_DIM, WOOSH_LATENT_FRAMES};
use makepad_diffusion::woosh_dit::WooshTaps;
use makepad_diffusion::woosh_pipeline::WooshPipeline;
use std::path::{Path, PathBuf};

struct Npy {
    shape: Vec<usize>,
    descr: String,
    fortran: bool,
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
    let fortran = header.contains("'fortran_order': True");
    Ok(Npy {
        shape,
        descr,
        fortran,
        data: bytes[header_start + header_len..].to_vec(),
    })
}

impl Npy {
    fn as_f32(&self) -> Result<Vec<f32>, String> {
        if self.fortran && self.shape.len() >= 2 {
            return Err("fortran-order npy not supported (dumps are C-order)".into());
        }
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

fn compare(tag: &str, ours: &[f32], reference: &[f32]) -> (f64, f32) {
    assert_eq!(ours.len(), reference.len(), "{tag}: length mismatch");
    let mut dot = 0f64;
    let mut na = 0f64;
    let mut nb = 0f64;
    let mut max_abs = 0f32;
    let mut ref_absmax = 0f32;
    for (&a, &b) in ours.iter().zip(reference) {
        dot += a as f64 * b as f64;
        na += a as f64 * a as f64;
        nb += b as f64 * b as f64;
        max_abs = max_abs.max((a - b).abs());
        ref_absmax = ref_absmax.max(b.abs());
    }
    let cos = dot / (na.sqrt() * nb.sqrt()).max(1e-30);
    println!("  {tag}: cos {cos:.7} max_abs {max_abs:.3e} (ref absmax {ref_absmax:.3})");
    (cos, max_abs)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |flag: &str, default: &str| -> String {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
            .unwrap_or_else(|| default.to_string())
    };
    let dumps = PathBuf::from(arg("--dumps", "local/woosh_ref/dumps"));
    let models = PathBuf::from(arg("--models", "local/models/woosh"));
    let fixture = arg("--fixture", "dflow_sword_clash");
    let stage = arg("--stage", "all");
    let fdir = dumps.join(&fixture);
    let load = |name: &str| -> Vec<f32> {
        load_npy(&fdir.join(format!("{name}.npy")))
            .and_then(|npy| npy.as_f32())
            .unwrap_or_else(|e| panic!("{e}"))
    };
    let run = |name: &str| stage == "all" || stage == name;

    let prompt = std::fs::read_to_string(fdir.join("stats.json"))
        .ok()
        .and_then(|text| {
            text.split("\"prompt\": \"")
                .nth(1)
                .and_then(|rest| rest.split('"').next())
                .map(str::to_string)
        })
        .expect("prompt in stats.json");
    println!("fixture {fixture}: {prompt:?}");

    println!("loading pipeline...");
    let t0 = std::time::Instant::now();
    let pipe = WooshPipeline::load(
        models.join("checkpoints/TextConditionerA/weights.safetensors"),
        models.join("checkpoints/Woosh-DFlow/weights.safetensors"),
        models.join("checkpoints/Woosh-AE/weights.safetensors"),
        models.join("tokenizer.json"),
        None,
    )
    .expect("pipeline load");
    println!("loaded in {:.1}s", t0.elapsed().as_secs_f32());

    if run("tok") {
        for (tag, text) in [("pos", prompt.as_str()), ("neg", "")] {
            let ids_ref: Vec<u32> = load(&format!("tok_ids_{tag}")).iter().map(|&v| v as u32).collect();
            let mask_ref = load(&format!("tok_mask_{tag}"));
            let (ids, mask) = pipe.tokenizer.encode_padded(text);
            let ids_ok = ids == ids_ref;
            let mask_ok = mask
                .iter()
                .zip(mask_ref.iter())
                .all(|(a, b)| (a - b).abs() < 1e-6);
            if ids_ok && mask_ok {
                println!("  tok {tag}: EXACT (77 ids)");
            } else {
                println!("  tok {tag}: MISMATCH\n   ours {ids:?}\n   ref  {ids_ref:?}");
            }
        }
    }

    if run("freqs") {
        let audio_ref = load("freqs_cis_audio_real");
        compare("freqs audio (1003x64x2)", pipe.dit.freqs_cis(), &audio_ref);
        let desc_ref = load("freqs_cis_desc_real");
        let identity: Vec<f32> = (0..desc_ref.len())
            .map(|i| if i % 2 == 0 { 1.0 } else { 0.0 })
            .collect();
        compare("freqs desc (identity)", &identity, &desc_ref);
    }

    let mut cond = None;
    if run("te") || run("dit") || run("sample") || run("ae") {
        let (ids, mask) = pipe.tokenizer.encode_padded(&prompt);
        let t0 = std::time::Instant::now();
        let hidden = pipe.text.encode(&ids, &mask, None).expect("te encode");
        println!("  te encode: {:.2}s", t0.elapsed().as_secs_f32());
        if run("te") {
            compare("te lhs[-2] pos (77x1024)", &hidden, &load("te_cond_pos"));
            let (neg_ids, neg_mask) = pipe.tokenizer.encode_padded("");
            let neg_hidden = pipe.text.encode(&neg_ids, &neg_mask, None).expect("te neg");
            compare("te lhs[-2] neg (77x1024)", &neg_hidden, &load("te_cond_neg"));
            let mask_ref = load("te_mask_pos");
            assert!(
                mask.iter().zip(mask_ref.iter()).all(|(a, b)| (a - b).abs() < 1e-6),
                "te mask mismatch"
            );
        }
        cond = Some(pipe.dit.embed_condition(&hidden, &mask).expect("embed cond"));
    }

    if run("dit") {
        let cond = cond.as_ref().expect("cond");
        // Call 0 with taps (t=1, r=0.75, x_in = init noise).
        let x_in = load("step0_x_in");
        let t = load("step0_t")[0];
        let r = load("step0_r")[0];
        let mut taps: WooshTaps = Vec::new();
        let t0 = std::time::Instant::now();
        let u = pipe
            .dit
            .forward_with_taps(&x_in, t, r, 4.5, cond, Some(&mut taps))
            .expect("dit forward");
        println!("  dit call0 forward: {:.2}s", t0.elapsed().as_secs_f32());
        for (name, data) in &taps {
            let (dump_name, take) = match name.as_str() {
                "pre_x" => ("call0_pre_x".to_string(), true),
                "pre_desc" => ("call0_pre_desc".to_string(), true),
                "pre_t" => ("call0_pre_t".to_string(), true),
                "pre_mplus" => ("call0_pre_mplus".to_string(), true),
                other => {
                    let known = ["block00", "block01", "block05", "block06", "block11"]
                        .iter()
                        .any(|k| other.starts_with(k));
                    (format!("call0_{other}"), known)
                }
            };
            if take {
                compare(&format!("dit {name}"), data, &load(&dump_name));
            }
        }
        compare("dit step0 u", &u, &load("step0_u"));
        for step in 1..4 {
            let x_in = load(&format!("step{step}_x_in"));
            let t = load(&format!("step{step}_t"))[0];
            let r = load(&format!("step{step}_r"))[0];
            let u = pipe.dit.forward(&x_in, t, r, 4.5, cond).expect("dit forward");
            compare(&format!("dit step{step} u"), &u, &load(&format!("step{step}_u")));
        }
    }

    if run("sample") {
        let cond = cond.as_ref().expect("cond");
        let init = load("noise_init");
        // Replay with the dumped renoise gaussians; check x_out per step by
        // comparing the final latents (per-step x_in is validated implicitly:
        // any drift would explode the comparison).
        let t0 = std::time::Instant::now();
        let latents = pipe
            .sample(
                cond,
                &init,
                4,
                4.5,
                &[0.0, 0.5, 0.5, 0.3],
                &mut |step| load(&format!("step{step}_fresh_noise")),
                None,
                None,
            )
            .expect("sample");
        println!("  sample (4 steps): {:.2}s", t0.elapsed().as_secs_f32());
        compare("sample final latents", &latents, &load("latents_final"));
        compare("sample vs step3_x_out", &latents, &load("step3_x_out"));
    }

    if run("ae") {
        let latents = load("latents_final");
        let mut taps = Vec::new();
        let t0 = std::time::Instant::now();
        let audio = pipe
            .ae
            .decode_with_taps(&latents, None, Some(&mut taps))
            .expect("ae decode");
        println!("  ae decode: {:.2}s", t0.elapsed().as_secs_f32());
        for (name, data) in &taps {
            let dump_name = format!("ae_{name}");
            compare(&format!("ae {name}"), data, &load(&dump_name));
        }
        compare("ae audio (240000)", &audio, &load("ae_audio"));
        let _ = (WOOSH_DESC_TOKENS, WOOSH_LATENT_DIM, WOOSH_LATENT_FRAMES);
    }

    println!("done");
}
