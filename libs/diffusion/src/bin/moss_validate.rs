//! MOSS-SoundEffect v2 stage validation against the reference oracle dumps
//! (scratchpad moss_dump.py -> local/moss_ref/dumps/<fixture>).
//!
//! Stages: sched | tok | te | dit | dac | sample (sample = full 100-step CPU
//! trajectory, ~hours on a laptop — off by default).
//!
//! Usage:
//!   moss-validate [--dumps <dir>] [--weights <dir>] [--fixture <name>]
//!                 [--stage all|sched|tok|te|dit|dac|sample]

use makepad_diffusion::moss::{
    moss_sigmas, moss_timesteps, MOSS_DEFAULT_SHIFT, MOSS_LATENT_DIM, MOSS_LATENT_FRAMES,
    MOSS_TEXT_TOKENS, MOSS_TE_HIDDEN,
};
use makepad_diffusion::moss_pipeline::MossPipeline;
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
        .map(|flat| self.c_order(flat))
    }

    /// numpy stores fortran_order arrays column-major; convert to C order.
    fn c_order(&self, flat: Vec<f32>) -> Vec<f32> {
        if !self.fortran || self.shape.len() < 2 {
            return flat;
        }
        let dims = self.shape.len();
        let numel: usize = self.shape.iter().product();
        if numel != flat.len() {
            return flat;
        }
        // F-order strides: stride[0]=1, stride[d]=stride[d-1]*shape[d-1]
        let mut f_stride = vec![1usize; dims];
        for d in 1..dims {
            f_stride[d] = f_stride[d - 1] * self.shape[d - 1];
        }
        let mut out = Vec::with_capacity(numel);
        let mut idx = vec![0usize; dims];
        'walk: loop {
            let mut off = 0usize;
            for d in 0..dims {
                off += idx[d] * f_stride[d];
            }
            out.push(flat[off]);
            for d in (0..dims).rev() {
                idx[d] += 1;
                if idx[d] < self.shape[d] {
                    continue 'walk;
                }
                idx[d] = 0;
            }
            break;
        }
        out
    }
}

fn compare(tag: &str, ours: &[f32], reference: &[f32]) {
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
    let dumps = PathBuf::from(arg("--dumps", "local/moss_ref/dumps"));
    let weights = PathBuf::from(arg("--weights", "local/moss_ref/weights"));
    let fixture = arg("--fixture", "sword_clash");
    let stage = arg("--stage", "all");
    let only_call0 = args.iter().any(|arg| arg == "--only-call0");
    let fdir = dumps.join(&fixture);
    let load = |name: &str| -> Npy {
        load_npy(&fdir.join(format!("{name}.npy"))).unwrap_or_else(|e| panic!("{e}"))
    };

    let info: String =
        std::fs::read_to_string(fdir.join("info.json")).expect("fixture info.json");
    let seconds: f64 = info
        .split("\"seconds\":")
        .nth(1)
        .and_then(|rest| rest.split(&[',', '}'][..]).next())
        .and_then(|v| v.trim().parse().ok())
        .expect("seconds in info.json");
    let prompt: String = info
        .split("\"prompt\": \"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .map(|s| s.to_string())
        .expect("prompt in info.json");
    println!("fixture {fixture}: {prompt:?} {seconds}s");

    let run = |name: &str| stage == "all" || stage == name;

    if run("sched") {
        let sig_ref = load("sched_sigmas").as_f32().unwrap();
        let ts_ref = load("sched_timesteps").as_f32().unwrap();
        let steps = ts_ref.len();
        let sig = moss_sigmas(steps, MOSS_DEFAULT_SHIFT);
        let ts = moss_timesteps(&sig);
        // reference sigmas tensor has `steps` entries (no terminal 0)
        compare("sched sigmas", &sig[..steps], &sig_ref[..steps]);
        compare("sched timesteps", &ts, &ts_ref);
    }

    println!("loading pipeline (te/dit/vae weights + tokenizer)...");
    let t0 = std::time::Instant::now();
    let pipe = MossPipeline::load(
        weights.join("text_encoder"),
        weights.join("transformer"),
        weights.join("vae/vae_128d_48k.pth"),
        weights.join("tokenizer"),
        None,
    )
    .expect("pipeline load");
    println!("loaded in {:.1}s", t0.elapsed().as_secs_f32());

    let ids_ref: Vec<u32> = load("te_pos_ids")
        .as_f32()
        .unwrap()
        .iter()
        .map(|&v| v as u32)
        .collect();
    let mask_ref: Vec<f32> = load("te_pos_mask").as_f32().unwrap();
    let valid = mask_ref.iter().filter(|&&m| m > 0.5).count();

    if run("tok") {
        let ids = pipe.tokenize(&prompt, seconds);
        let reference = &ids_ref[..valid];
        if ids.len() == reference.len()
            && ids.iter().zip(reference).all(|(a, b)| *a == *b)
        {
            println!("  tok: EXACT ({} ids)", ids.len());
        } else {
            println!("  tok: MISMATCH\n   ours {ids:?}\n   ref  {reference:?}");
        }
    }

    let mut context: Option<Vec<f32>> = None;
    if run("te") || run("dit") || run("sample") || run("dit-dev") || run("sample-dev") {
        let ids = &ids_ref[..valid];
        let t0 = std::time::Instant::now();
        let ctx = pipe.encode_context(ids).expect("te encode");
        println!("  te encode: {:.2}s", t0.elapsed().as_secs_f32());
        let hidden_ref = load("te_pos_hidden").as_f32().unwrap();
        compare(
            "te hidden (valid rows)",
            &ctx[..valid * MOSS_TE_HIDDEN],
            &hidden_ref[..valid * MOSS_TE_HIDDEN],
        );
        let ctx_ref = load("dit000_ctx").as_f32().unwrap();
        compare("te context (zero-padded)", &ctx, &ctx_ref);
        context = Some(ctx);
    }

    if run("dit") {
        let ctx = context.as_ref().expect("context");
        let pos = pipe.dit.embed_context(ctx).expect("embed ctx");
        let neg = pipe
            .dit
            .embed_context(&vec![0f32; MOSS_TEXT_TOKENS * MOSS_TE_HIDDEN])
            .expect("embed neg ctx");
        // timesteps per call from info.json
        let ts_all: Vec<f32> = info
            .split("\"dit_timesteps\": [")
            .nth(1)
            .and_then(|rest| rest.split(']').next())
            .map(|list| {
                list.split(',')
                    .filter_map(|v| v.trim().parse::<f32>().ok())
                    .collect()
            })
            .expect("dit_timesteps");
        for call in [0usize, 1, 196, 197] {
            if only_call0 && call != 0 { continue; }
            let x = load(&format!("dit{call:03}_x")).as_f32().unwrap();
            let out_ref = load(&format!("dit{call:03}_out")).as_f32().unwrap();
            let ctx_for_call = if call % 2 == 0 { &pos } else { &neg };
            let t = ts_all[call];
            let t0 = std::time::Instant::now();
            let out = pipe
                .dit
                .forward(&x, MOSS_LATENT_FRAMES, t, ctx_for_call)
                .expect("dit forward");
            println!(
                "  dit call {call} (t={t:.2}) forward {:.1}s",
                t0.elapsed().as_secs_f32()
            );
            compare(&format!("dit{call:03} out"), &out, &out_ref);
        }
    }

    if run("dit-dev") {
        // Device single-forward parity (CUDA box only).
        assert!(
            makepad_diffusion::moss::moss_device_enabled(),
            "dit-dev needs a CUDA device"
        );
        let ctx = context.as_ref().expect("context");
        let pos = pipe.dit.embed_context(ctx).expect("embed ctx");
        let state = pipe.prepare_device().expect("prepare device");
        let pos_run = pipe
            .dit
            .begin_device_run(&state.device, &pos)
            .expect("pos run");
        let neg = pipe
            .dit
            .embed_context(&vec![0f32; MOSS_TEXT_TOKENS * MOSS_TE_HIDDEN])
            .expect("neg ctx");
        let neg_run = pipe
            .dit
            .begin_device_run(&state.device, &neg)
            .expect("neg run");
        let rope = pipe.dit.device_rope(MOSS_LATENT_FRAMES).expect("rope");
        let ts_all: Vec<f32> = info
            .split("\"dit_timesteps\": [")
            .nth(1)
            .and_then(|rest| rest.split(']').next())
            .map(|list| {
                list.split(',')
                    .filter_map(|v| v.trim().parse::<f32>().ok())
                    .collect()
            })
            .expect("dit_timesteps");
        for call in [0usize, 1, 196, 197] {
            let x = load(&format!("dit{call:03}_x")).as_f32().unwrap();
            let out_ref = load(&format!("dit{call:03}_out")).as_f32().unwrap();
            // channel-major -> frame-major
            let frames = MOSS_LATENT_FRAMES;
            let ch = x.len() / frames;
            let mut xt = vec![0f32; x.len()];
            for c in 0..ch {
                for f in 0..frames {
                    xt[f * ch + c] = x[c * frames + f];
                }
            }
            let run_for_call = if call % 2 == 0 { &pos_run } else { &neg_run };
            let t = ts_all[call];
            let t0 = std::time::Instant::now();
            let out_t = pipe
                .dit
                .forward_device(&state.device, run_for_call, &rope, &xt, t)
                .expect("device forward");
            println!(
                "  dit-dev call {call} (t={t:.2}) forward {:.3}s",
                t0.elapsed().as_secs_f32()
            );
            // frame-major -> channel-major
            let mut out = vec![0f32; out_t.len()];
            for f in 0..frames {
                for c in 0..ch {
                    out[c * frames + f] = out_t[f * ch + c];
                }
            }
            compare(&format!("dit-dev{call:03} out"), &out, &out_ref);
        }
    }

    if run("sample-dev") {
        assert!(
            makepad_diffusion::moss::moss_device_enabled(),
            "sample-dev needs a CUDA device"
        );
        let ctx = context.as_ref().expect("context");
        let pos = pipe.dit.embed_context(ctx).expect("embed ctx");
        let state = pipe.prepare_device().expect("prepare device");
        let noise = load("dit000_x").as_f32().unwrap();
        let vae_in_ref = load("vae_in").as_f32().unwrap();
        let steps = 100;
        let t0 = std::time::Instant::now();
        let latents = pipe
            .sample_device(&state, &pos, &noise, steps, 4.0, MOSS_DEFAULT_SHIFT, None, None)
            .expect("sample device");
        println!("  sample-dev 100 steps: {:.2}s", t0.elapsed().as_secs_f32());
        compare("sample-dev final latents", &latents, &vae_in_ref);
        let vae_out_ref = load("vae_out").as_f32().unwrap();
        let t0 = std::time::Instant::now();
        let audio = pipe
            .dac
            .decode_device(&state.dac, &latents, MOSS_LATENT_FRAMES)
            .expect("dac decode device");
        println!("  dac-dev decode (full 30s): {:.2}s", t0.elapsed().as_secs_f32());
        compare("sample-dev audio e2e (device dac)", &audio, &vae_out_ref);
        // TE device parity
        let ids = &ids_ref[..valid];
        let t0 = std::time::Instant::now();
        let ctx_dev = pipe.text.encode_device(&state.te, ids).expect("te device");
        println!("  te-dev encode: {:.3}s", t0.elapsed().as_secs_f32());
        let ctx_ref2 = load("dit000_ctx").as_f32().unwrap();
        compare("te-dev context", &ctx_dev, &ctx_ref2);
    }

    if run("dac") {
        let vae_in = load("vae_in").as_f32().unwrap();
        let vae_out_ref = load("vae_out").as_f32().unwrap();
        let t0 = std::time::Instant::now();
        let audio = pipe.decode_full(&vae_in).expect("dac decode");
        println!("  dac decode: {:.1}s", t0.elapsed().as_secs_f32());
        compare("dac audio (full 30s)", &audio, &vae_out_ref);
    }

    if run("sample") {
        let ctx = context.as_ref().expect("context");
        let pos = pipe.dit.embed_context(ctx).expect("embed ctx");
        let noise = load("dit000_x").as_f32().unwrap();
        let vae_in_ref = load("vae_in").as_f32().unwrap();
        let steps = 100;
        let t0 = std::time::Instant::now();
        let mut last_print = std::time::Instant::now();
        let latents = pipe
            .sample(
                &pos,
                &noise,
                steps,
                4.0,
                MOSS_DEFAULT_SHIFT,
                Some(&mut |label: &str, _fraction: f64| {
                    if last_print.elapsed().as_secs() >= 60 {
                        println!("  sample {label}");
                        last_print = std::time::Instant::now();
                    }
                    Ok(())
                }),
                None,
            )
            .expect("sample");
        println!("  sample: {:.0}s", t0.elapsed().as_secs_f32());
        compare("final latents", &latents, &vae_in_ref);
        let _ = MOSS_LATENT_DIM;
    }

    println!("done");
}
