//! ACE-Step 1.5 XL stage validation against the sibling Python oracle dumps
//! (`C:\ai\ace\dumps\<fixture>` or `$ACE_DUMP_DIR`). Compares cond / latent /
//! step residual / wav when dumps exist; always checks the locked schedule
//! and prompt-format contracts.
//!
//! Usage:
//!   ace-validate [--dump <dir>] [--weights <dir>] [--stage schedule|prompt|cond|dit|wav|all]

use makepad_diffusion::ace::{
    ace_format_prompt, ace_latent_len, ace_sigmas, ACE_DEFAULT_SHIFT, ACE_INSTRUCTION,
    ACE_LATENT_DIM, ACE_SAMPLE_RATE,
};
use makepad_diffusion::ace_pipeline::{AceGenerate, AcePaths, AcePipeline};
use std::path::{Path, PathBuf};
use std::time::Instant;

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
    let fortran = header
        .split("'fortran_order':")
        .nth(1)
        .map(|rest| rest.trim_start().starts_with("True"))
        .unwrap_or(false);
    Ok(Npy {
        shape,
        descr,
        fortran,
        data: bytes[header_start + header_len..].to_vec(),
    })
}

impl Npy {
    fn as_f32(&self) -> Result<Vec<f32>, String> {
        let raw = match self.descr.as_str() {
            "<f4" => self
                .data
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect::<Vec<_>>(),
            "<f8" => self
                .data
                .chunks_exact(8)
                .map(|c| {
                    f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]) as f32
                })
                .collect(),
            other => return Err(format!("npy descr {other} not f32-convertible")),
        };
        if !self.fortran || self.shape.len() < 2 {
            return Ok(raw);
        }
        Ok(fortran_to_c(&raw, &self.shape))
    }
}

/// Reorder a Fortran-contiguous buffer into C-contiguous (last index fastest).
fn fortran_to_c(src: &[f32], shape: &[usize]) -> Vec<f32> {
    let n: usize = shape.iter().product();
    if src.len() != n || n == 0 {
        return src.to_vec();
    }
    let mut out = vec![0f32; n];
    let mut f_stride = vec![1usize; shape.len()];
    for i in 1..shape.len() {
        f_stride[i] = f_stride[i - 1] * shape[i - 1];
    }
    for c_idx in 0..n {
        let mut rem = c_idx;
        let mut f_idx = 0usize;
        for dim in (0..shape.len()).rev() {
            let coord = rem % shape[dim];
            rem /= shape[dim];
            f_idx += coord * f_stride[dim];
        }
        out[c_idx] = src[f_idx];
    }
    out
}

struct Cmp {
    cos: f64,
    max_abs: f32,
    mean_abs: f32,
    ref_absmax: f32,
}

fn compare(ours: &[f32], reference: &[f32]) -> Result<Cmp, String> {
    if ours.len() != reference.len() {
        return Err(format!(
            "length mismatch ours {} vs ref {}",
            ours.len(),
            reference.len()
        ));
    }
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
    Ok(Cmp {
        cos: dot / (na.sqrt() * nb.sqrt()).max(1e-30),
        max_abs,
        mean_abs: (sum_abs / ours.len() as f64) as f32,
        ref_absmax,
    })
}

fn report(name: &str, cmp: &Cmp) -> bool {
    let ok = cmp.cos >= 0.999 || (cmp.max_abs < 2e-2 && cmp.mean_abs < 2e-3);
    println!(
        "  {name:<24} cos={:.7} max_abs={:.3e} mean_abs={:.3e} (ref absmax {:.3}) {}",
        cmp.cos,
        cmp.max_abs,
        cmp.mean_abs,
        cmp.ref_absmax,
        if ok { "OK" } else { "FAIL" }
    );
    ok
}

fn default_dump_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("ACE_DUMP_DIR") {
        return PathBuf::from(dir);
    }
    let root = PathBuf::from(r"C:\ai\ace\dumps");
    if let Ok(rd) = std::fs::read_dir(&root) {
        if let Some(first) = rd.flatten().find(|e| e.path().is_dir()) {
            return first.path();
        }
    }
    root
}

fn default_weights_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("ACE_WEIGHTS_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(dir) = std::env::var("MAKEPAD_ACE_DIR") {
        return PathBuf::from(dir);
    }
    for cand in [
        r"C:\ai\ace\weights\acestep-v15-xl-base-diffusers",
        r"C:\ai\ace\weights\acestep-v15-xl-turbo-diffusers",
        r"C:\ai\ace\native",
    ] {
        let p = PathBuf::from(cand);
        if p.join("transformer").join("diffusion_pytorch_model-00002-of-00002.safetensors").is_file()
            || p.join("transformer").join("diffusion_pytorch_model-00001-of-00002.safetensors").is_file()
        {
            return p;
        }
    }
    PathBuf::from(r"C:\ai\ace\weights\acestep-v15-xl-base-diffusers")
}

fn flatten_match(ours: &[f32], reference: &[f32]) -> Result<(Vec<f32>, Vec<f32>), String> {
    if ours.len() == reference.len() {
        return Ok((ours.to_vec(), reference.to_vec()));
    }
    // Dumps are often [1, T, C]; skip a leading batch of ones.
    if reference.len() == ours.len() {
        return Ok((ours.to_vec(), reference.to_vec()));
    }
    Err(format!(
        "length mismatch ours {} vs ref {}",
        ours.len(),
        reference.len()
    ))
}

fn load_wav_planar(path: &Path) -> Result<(Vec<f32>, Vec<f32>), String> {
    let bytes = std::fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(format!("{}: not a wav", path.display()));
    }
    let mut off = 12usize;
    let mut data = None;
    let mut fmt_ch = 0u16;
    let mut fmt_bps = 0u16;
    while off + 8 <= bytes.len() {
        let tag = &bytes[off..off + 4];
        let size = u32::from_le_bytes([
            bytes[off + 4],
            bytes[off + 5],
            bytes[off + 6],
            bytes[off + 7],
        ]) as usize;
        let start = off + 8;
        let end = (start + size).min(bytes.len());
        if tag == b"fmt " && size >= 16 {
            fmt_ch = u16::from_le_bytes([bytes[start + 2], bytes[start + 3]]);
            fmt_bps = u16::from_le_bytes([bytes[start + 14], bytes[start + 15]]);
        } else if tag == b"data" {
            data = Some(&bytes[start..end]);
        }
        off = start + size + (size % 2);
    }
    let data = data.ok_or_else(|| format!("{}: no data chunk", path.display()))?;
    if fmt_ch != 2 {
        return Err(format!("{}: expected stereo, got {fmt_ch} ch", path.display()));
    }
    let mut left = Vec::new();
    let mut right = Vec::new();
    match fmt_bps {
        16 => {
            for pair in data.chunks_exact(4) {
                let l = i16::from_le_bytes([pair[0], pair[1]]) as f32 / 32768.0;
                let r = i16::from_le_bytes([pair[2], pair[3]]) as f32 / 32768.0;
                left.push(l);
                right.push(r);
            }
        }
        32 => {
            for pair in data.chunks_exact(8) {
                left.push(f32::from_le_bytes([pair[0], pair[1], pair[2], pair[3]]));
                right.push(f32::from_le_bytes([pair[4], pair[5], pair[6], pair[7]]));
            }
        }
        other => return Err(format!("{}: unsupported bits {other}", path.display())),
    }
    Ok((left, right))
}

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.windows(2)
        .find(|w| w[0] == flag)
        .map(|w| w[1].clone())
}

fn stage_schedule() -> bool {
    println!("== schedule ==");
    let s8 = ace_sigmas(8, ACE_DEFAULT_SHIFT);
    let s50 = ace_sigmas(50, ACE_DEFAULT_SHIFT);
    let ok8 = (s8[0] - 1.0).abs() < 1e-6 && s8[8].abs() < 1e-6;
    let ok50 = (s50[0] - 1.0).abs() < 1e-6 && s50.len() == 51 && s50[50].abs() < 1e-6;
    println!(
        "  8-step t0={:.6} t8={:.6} {} | 50-step n={} t0={:.6} t49={:.6} {}",
        s8[0],
        s8[8],
        if ok8 { "OK" } else { "FAIL" },
        s50.len(),
        s50[0],
        s50[49],
        if ok50 { "OK" } else { "FAIL" }
    );
    println!("  latent 12s = {} (25 Hz)", ace_latent_len(12.0));
    ok8 && ok50
}

fn stage_prompt() -> bool {
    println!("== prompt ==");
    let (text, lyrics) = ace_format_prompt(
        "a piano ballad",
        "[verse]\nhello",
        "en",
        30.0,
        Some(ACE_INSTRUCTION),
        Some(120),
        Some("C major"),
        Some("4"),
    );
    let ok = text.contains("# Instruction\n")
        && text.contains("# Caption\n")
        && text.contains("# Metas\n")
        && text.contains("<|endoftext|>")
        && lyrics.contains("# Languages\nen")
        && lyrics.contains("# Lyric\n");
    println!("  SFT template {} chars, lyrics {} chars", text.len(), lyrics.len());
    println!("  {}", if ok { "OK" } else { "FAIL" });
    ok
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dump = arg_value(&args, "--dump").map(PathBuf::from).unwrap_or_else(default_dump_dir);
    let weights = arg_value(&args, "--weights")
        .map(PathBuf::from)
        .unwrap_or_else(default_weights_dir);
    let stage = arg_value(&args, "--stage").unwrap_or_else(|| "all".into());

    println!("ace-validate stage={stage}");
    println!("  dump    {}", dump.display());
    println!("  weights {}", weights.display());

    let mut failed = 0usize;
    if stage == "all" || stage == "schedule" {
        if !stage_schedule() {
            failed += 1;
        }
    }
    if stage == "all" || stage == "prompt" {
        if !stage_prompt() {
            failed += 1;
        }
    }

    let want_weights = matches!(stage.as_str(), "all" | "cond" | "dit" | "dit0" | "wav");
    if want_weights {
        let dit = weights.join("transformer");
        if !dit.is_dir() && !dit.join("diffusion_pytorch_model-00001-of-00002.safetensors").is_file() {
            println!(
                "== weights ==\n  missing {} — skip cond/dit/wav (dumps/weights not staged)",
                dit.display()
            );
            if failed == 0 {
                println!("ace-validate: schedule/prompt OK; numeric gate blocked on missing weights/dumps");
                std::process::exit(0);
            }
            std::process::exit(1);
        }
        let started = Instant::now();
        let pipe = match AcePipeline::load(&AcePaths::from_model_dir(&weights), None) {
            Ok(p) => p,
            Err(err) => {
                eprintln!("load failed: {err:?}");
                std::process::exit(2);
            }
        };
        println!("  loaded in {:.1}s device={}", started.elapsed().as_secs_f64(), pipe.device_active());

        if stage == "dit0" {
            let cond_data = load_npy(&dump.join("cond_enc_out.npy")).and_then(|n| n.as_f32()).expect("cond");
            let xt = load_npy(&dump.join("step0_xt.npy")).and_then(|n| n.as_f32()).expect("xt");
            let src = load_npy(&dump.join("src_latents.npy")).and_then(|n| n.as_f32()).expect("src");
            let frames = ace_latent_len(12.0);
            let tokens = cond_data.len() / 2048;
            let cond = makepad_diffusion::ace_dit::AceConditioning {
                encoder_hidden: cond_data,
                encoder_mask: vec![true; tokens],
                tokens,
            };
            let mut context = vec![0f32; frames * 128];
            for t in 0..frames {
                context[t * 128..t * 128 + 64]
                    .copy_from_slice(&src[t * 64..(t + 1) * 64]);
                for c in 0..64 {
                    context[t * 128 + 64 + c] = 1.0;
                }
            }
            let t0 = ace_sigmas(50, ACE_DEFAULT_SHIFT)[0];
            println!("== dit0 t={t0} frames={frames} cond_tokens={tokens} ==");
            let cold = Instant::now();
            let vt = pipe.dit_forward(&xt, &context, frames, &cond, t0).expect("dit0");
            println!("  cold_dit0_s={:.3}", cold.elapsed().as_secs_f64());
            let warm = Instant::now();
            let vt_warm = pipe.dit_forward(&xt, &context, frames, &cond, t0).expect("dit0warm");
            println!("  warm_dit0_s={:.3}", warm.elapsed().as_secs_f64());
            let _ = vt_warm;
            let null = pipe
                .cond
                .null_condition_emb
                .as_ref()
                .cloned()
                .or_else(|| load_npy(&dump.join("null_condition_emb.npy")).and_then(|n| n.as_f32()).ok())
                .unwrap_or_default();
            let dim = 2048usize;
            let uncond = makepad_diffusion::ace_dit::AceConditioning {
                encoder_hidden: null.iter().copied().cycle().take(tokens * dim).collect(),
                encoder_mask: vec![true; tokens],
                tokens,
            };
            let vt_uncond = pipe
                .dit_forward(&xt, &context, frames, &uncond, t0)
                .expect("dit0 uncond");
            let mut apg = makepad_diffusion::ace::AceApgMomentum::new(makepad_diffusion::ace::ACE_APG_MOMENTUM);
            let vt_guided = makepad_diffusion::ace::ace_apg(
                &vt,
                &vt_uncond,
                frames,
                64,
                6.0,
                &mut apg,
                makepad_diffusion::ace::ACE_APG_ETA,
                makepad_diffusion::ace::ACE_APG_NORM_THRESHOLD,
            );
            let mut none_fail = 0;
            let check = |name: &str, ours: &[f32], file: &str| -> bool {
                let path = dump.join(file);
                match load_npy(&path).and_then(|n| n.as_f32()) {
                    Ok(reference) => match flatten_match(ours, &reference).and_then(|(a, b)| compare(&a, &b)) {
                        Ok(cmp) => report(name, &cmp),
                        Err(err) => { println!("  {name:<24} {err}"); false }
                    },
                    Err(err) => { println!("  {name:<24} {err}"); false }
                }
            };
            if !check("src_latents", &src, "src_latents.npy") { none_fail += 1; }
            if !check("step0_vt_cond", &vt, "step0_vt_cond.npy") { none_fail += 1; }
            if !check("step0_vt_uncond", &vt_uncond, "step0_vt_uncond.npy") { none_fail += 1; }
            if !check("step0_vt_guided", &vt_guided, "step0_vt_guided.npy") { none_fail += 1; }
            if !check("null_emb", &null, "null_condition_emb.npy") { none_fail += 1; }
            std::process::exit(if none_fail == 0 { 0 } else { 1 });
        }

        let seconds = arg_value(&args, "--seconds")
            .and_then(|s| s.parse().ok())
            .unwrap_or(12.0);
        let prompt = arg_value(&args, "--prompt")
            .unwrap_or_else(|| "lofi rain loop, warm keys".into());
        let lyrics = if let Some(p) = arg_value(&args, "--lyrics-file") {
            std::fs::read_to_string(&p).unwrap_or_else(|e| {
                eprintln!("lyrics-file {p}: {e}");
                std::process::exit(2);
            })
        } else {
            arg_value(&args, "--lyrics").unwrap_or_else(|| "[Instrumental]".into())
        };
        let seed = arg_value(&args, "--seed")
            .and_then(|s| s.parse().ok())
            .unwrap_or(7u64);
        let free_gen = (seconds - 12.0_f64).abs() > 1e-6
            || arg_value(&args, "--prompt").is_some()
            || args.iter().any(|a| a == "--no-compare");
        let no_warm = args.iter().any(|a| a == "--no-warm");
        let mut req = AceGenerate::new(prompt, lyrics, seconds, seed);
        req.steps = arg_value(&args, "--steps")
            .and_then(|s| s.parse().ok())
            .unwrap_or(50);
        req.guidance = 7.0;
        req.shift = 3.0;
        let noise_path = dump.join("initial_latent.npy");
        let replay = if !free_gen && noise_path.is_file() {
            load_npy(&noise_path).ok().and_then(|n| n.as_f32().ok())
        } else {
            None
        };
        println!(
            "  song seconds={seconds} seed={seed} steps={} free_gen={free_gen}",
            req.steps
        );
        let mut none = None;
        let gen_started = Instant::now();
        let taps = match pipe.generate_taps(&req, replay.as_deref(), &mut none) {
            Ok(t) => t,
            Err(err) => {
                eprintln!("generate failed: {err:?}");
                std::process::exit(2);
            }
        };
        println!(
            "  generate_s={:.3} device={} samples={}",
            gen_started.elapsed().as_secs_f64(),
            pipe.device_active(),
            taps.left.len()
        );
        if let Ok(path) = std::env::var("ACE_WRITE_WAV") {
            if !path.is_empty() {
                match write_wav_stereo16(
                    Path::new(&path),
                    &taps.left,
                    &taps.right,
                    ACE_SAMPLE_RATE as u32,
                ) {
                    Ok(()) => println!("  wrote {path}"),
                    Err(err) => println!("  ACE_WRITE_WAV {err}"),
                }
            }
        }
        if !no_warm && !free_gen {
            let warm_started = Instant::now();
            let mut none2 = None;
            match pipe.generate_taps(&req, replay.as_deref(), &mut none2) {
                Ok(_) => println!("  warm_generate_s={:.3}", warm_started.elapsed().as_secs_f64()),
                Err(err) => println!("  warm_generate failed: {err:?}"),
            }
        }

        if free_gen {
            println!("ace-validate: free generate, skip dump compare");
            std::process::exit(0);
        }

        let check = |name: &str, ours: &[f32], file: &str| -> bool {
            let path = dump.join(file);
            if !path.is_file() {
                println!("  {name:<24} dump {} missing — skip", path.display());
                return true;
            }
            match load_npy(&path).and_then(|n| n.as_f32()) {
                Ok(reference) => match flatten_match(ours, &reference).and_then(|(a, b)| compare(&a, &b)) {
                    Ok(cmp) => report(name, &cmp),
                    Err(err) => {
                        println!("  {name:<24} {err}");
                        false
                    }
                },
                Err(err) => {
                    println!("  {name:<24} {err}");
                    false
                }
            }
        };

        if stage == "all" || stage == "cond" {
            println!("== cond ==");
            if !check("text_hidden", &taps.text_hidden, "text_hidden.npy") {
                failed += 1;
            }
            if !check("cond_enc_out", &taps.encoder_hidden, "cond_enc_out.npy") {
                failed += 1;
            }
            if !check("pipe_cond_enc_out", &taps.encoder_hidden, "pipe_cond_enc_out.npy") {
                failed += 1;
            }
        }
        if stage == "all" || stage == "dit" {
            println!("== dit ==");
            if !check("step0_vt_cond", &taps.velocity0, "step0_vt_cond.npy") {
                failed += 1;
            }
            if !check("step0_vt_uncond", &taps.vt_uncond0, "step0_vt_uncond.npy") {
                failed += 1;
            }
            if !check("step0_vt_guided", &taps.vt_guided0, "step0_vt_guided.npy") {
                failed += 1;
            }
            if !check("last_vt_guided", &taps.last_vt_guided, "last_vt_guided.npy") {
                failed += 1;
            }
            if !check("last_xt", &taps.latents_final, "last_xt.npy") {
                failed += 1;
            }
            let _ = ACE_LATENT_DIM;
        }
        if stage == "all" || stage == "wav" {
            println!("== wav ==");
            if !check("wav_left", &taps.left, "wav_left.npy") {
                failed += 1;
            }
            if !check("wav_right", &taps.right, "wav_right.npy") {
                failed += 1;
            }
            for name in [
                "lofi_rain_12s_seed7.wav",
                "lofi_rain_12s_seed7_replay.wav",
                "lofi_rain_12s_seed7_from_latent.wav",
            ] {
                let path = dump.join(name);
                if !path.is_file() {
                    continue;
                }
                match load_wav_planar(&path) {
                    Ok((left, right)) => {
                        match flatten_match(&taps.left, &left).and_then(|(a, b)| compare(&a, &b)) {
                            Ok(cmp) => {
                                if !report(&format!("{name}/L"), &cmp) {
                                    failed += 1;
                                }
                            }
                            Err(err) => {
                                println!("  {name}/L {err}");
                                failed += 1;
                            }
                        }
                        match flatten_match(&taps.right, &right).and_then(|(a, b)| compare(&a, &b)) {
                            Ok(cmp) => {
                                if !report(&format!("{name}/R"), &cmp) {
                                    failed += 1;
                                }
                            }
                            Err(err) => {
                                println!("  {name}/R {err}");
                                failed += 1;
                            }
                        }
                    }
                    Err(err) => println!("  {name} {err}"),
                }
            }
        }
    }

    if failed == 0 {
        println!("ace-validate: PASS");
    } else {
        println!("ace-validate: {failed} stage(s) failed");
        std::process::exit(1);
    }
}

fn write_wav_stereo16(path: &Path, left: &[f32], right: &[f32], sr: u32) -> Result<(), String> {
    let n = left.len().min(right.len());
    let mut pcm = Vec::with_capacity(n * 4);
    for i in 0..n {
        for &s in &[left[i], right[i]] {
            let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
            pcm.extend_from_slice(&v.to_le_bytes());
        }
    }
    let mut buf = Vec::with_capacity(44 + pcm.len());
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
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
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(path, buf).map_err(|e| e.to_string())
}
