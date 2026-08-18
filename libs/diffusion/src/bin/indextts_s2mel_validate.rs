//! IndexTTS-2.5 semantic-codec + s2mel validation against the reference
//! oracle dumps (`local/indextts_ref/dumps`, see dump_oracle.py):
//!
//! - codec:     semantic_codes.npy -> s_infer.npy       (EnhancedCodec.decode)
//! - regulator: spk_cond_emb.npy   -> prompt_condition.npy (ylens = ref mel)
//!              s_infer.npy        -> s2mel_cond.npy       (ylens = 1.72x)
//! - dit:       one CFG-combined estimator pass at t=0  -> cfm_v_step1.npy
//! - solve:     full 25-step Euler CFG with replayed cfm_noise.npy
//!              -> cfm_v_step13/25.npy, cfm_mel_full.npy, vc_target.npy
//!
//! Gates: cosine >= 0.999 and max-abs <= 2e-3 for the single-pass stages;
//! the 25-step solve accumulates float error, so it gates on cosine only and
//! reports max-abs for judgement.
//!
//! Usage: indextts-s2mel-validate [--dumps <dir>] [--weights <dir>]
//!                                [--stage codec|regulator|dit|solve|all]
//! (defaults: the reference checkout dirs, stage all)
//!
//! There is also a hidden `--stage codec-debug` that bisects the decoder
//! layer by layer against the codecdbg_*.npy dumps written by
//! `local/indextts_ref/dump_s2mel_extra.py` (which also dumps DiT per-layer
//! hiddens `ditdbg_*` for future estimator bisection).
//!
//! NOTE: the npy reader honors `fortran_order: True` — several oracle dumps
//! (s_infer.npy among them) are np.save'd from transposed torch views and
//! are stored column-major.

use makepad_diffusion::indextts::{reference_checkpoints_dir, reference_dumps_dir};
use makepad_diffusion::indextts_codec::SemanticCodecDecoder;
use makepad_diffusion::indextts_s2mel::{S2mel, S2melNoiseSource};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

// --- minimal npy read (the crate-wide validate-bin pattern) -----------------

struct Npy {
    shape: Vec<usize>,
    data: Vec<f32>,
}

fn load_npy(path: &Path) -> Result<Npy, String> {
    let bytes = std::fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?;
    if bytes.len() < 10 || &bytes[..6] != b"\x93NUMPY" {
        return Err(format!("{}: not an npy file", path.display()));
    }
    let (header_len, header_start) = if bytes[6] == 1 {
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
    let raw = &bytes[header_start + header_len..];
    let data: Vec<f32> = match descr.as_str() {
        "<f4" => raw
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
        "<i8" => raw
            .chunks_exact(8)
            .map(|c| i64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]) as f32)
            .collect(),
        other => return Err(format!("{}: npy descr {other} unsupported", path.display())),
    };
    let numel: usize = shape.iter().product();
    if data.len() != numel.max(1) && !shape.is_empty() {
        return Err(format!(
            "{}: data len {} != shape {:?}",
            path.display(),
            data.len(),
            shape
        ));
    }
    // np.save stores transposed views column-major; rewrite to C order.
    let data = if fortran && shape.len() > 1 {
        let mut f_stride = vec![1usize; shape.len()];
        for d in 1..shape.len() {
            f_stride[d] = f_stride[d - 1] * shape[d - 1];
        }
        let mut out = vec![0f32; data.len()];
        let mut idx = vec![0usize; shape.len()];
        for value in out.iter_mut() {
            let mut off = 0usize;
            for d in 0..shape.len() {
                off += idx[d] * f_stride[d];
            }
            *value = data[off];
            for d in (0..shape.len()).rev() {
                idx[d] += 1;
                if idx[d] < shape[d] {
                    break;
                }
                idx[d] = 0;
            }
        }
        out
    } else {
        data
    };
    Ok(Npy { shape, data })
}

// --- comparison ---------------------------------------------------------------

struct Cmp {
    cosine: f64,
    max_abs: f64,
    rms_diff: f64,
}

fn compare(ours: &[f32], reference: &[f32]) -> Result<Cmp, String> {
    if ours.len() != reference.len() {
        return Err(format!(
            "length mismatch: ours {} ref {}",
            ours.len(),
            reference.len()
        ));
    }
    let mut dot = 0f64;
    let mut na = 0f64;
    let mut nb = 0f64;
    let mut max_abs = 0f64;
    let mut sq = 0f64;
    for (&a, &b) in ours.iter().zip(reference) {
        let a = a as f64;
        let b = b as f64;
        dot += a * b;
        na += a * a;
        nb += b * b;
        let d = (a - b).abs();
        max_abs = max_abs.max(d);
        sq += d * d;
    }
    Ok(Cmp {
        cosine: dot / (na.sqrt() * nb.sqrt()).max(1e-30),
        max_abs,
        rms_diff: (sq / ours.len() as f64).sqrt(),
    })
}

fn report(name: &str, cmp: &Cmp, secs: f64, gate_max_abs: Option<f64>) -> bool {
    let mut pass = cmp.cosine >= 0.999;
    if let Some(gate) = gate_max_abs {
        pass &= cmp.max_abs <= gate;
    }
    println!(
        "[{name}] cosine={:.8} max_abs={:.6e} rms_diff={:.6e} time={:.2}s -> {}",
        cmp.cosine,
        cmp.max_abs,
        cmp.rms_diff,
        secs,
        if pass { "PASS" } else { "FAIL" }
    );
    pass
}

// --- noise replay ---------------------------------------------------------------

struct ReplayNoise {
    data: Vec<f32>,
}

impl S2melNoiseSource for ReplayNoise {
    fn draw(&mut self, _index: usize, len: usize) -> Vec<f32> {
        assert_eq!(len, self.data.len(), "replay noise length mismatch");
        self.data.clone()
    }
}

// --- stages ---------------------------------------------------------------------

fn run_codec(dumps: &Path, weights: &Path) -> Result<bool, String> {
    let start = Instant::now();
    let decoder =
        SemanticCodecDecoder::load(weights.join("codec.pth")).map_err(|e| e.to_string())?;
    println!("codec load: {:.2}s", start.elapsed().as_secs_f64());
    let codes_npy = load_npy(&dumps.join("semantic_codes.npy"))?;
    let codes: Vec<u32> = codes_npy.data.iter().map(|&v| v.round() as u32).collect();
    let reference = load_npy(&dumps.join("s_infer.npy"))?;
    let start = Instant::now();
    let ours = decoder.decode(&codes).map_err(|e| e.to_string())?;
    let secs = start.elapsed().as_secs_f64();
    println!(
        "codec decode: {} codes -> ({}, 1024), ref {:?}",
        codes.len(),
        ours.len() / 1024,
        reference.shape
    );
    let cmp = compare(&ours, &reference.data)?;
    Ok(report("codec", &cmp, secs, Some(2e-3)))
}

fn run_regulator(dumps: &Path, s2mel: &S2mel) -> Result<bool, String> {
    let mut all = true;
    for (in_name, out_name) in [
        ("spk_cond_emb.npy", "prompt_condition.npy"),
        ("s_infer.npy", "s2mel_cond.npy"),
    ] {
        let input = load_npy(&dumps.join(in_name))?;
        let reference = load_npy(&dumps.join(out_name))?;
        let (in_len, out_len) = (input.shape[1], reference.shape[1]);
        let start = Instant::now();
        let ours = s2mel
            .length_regulator
            .forward(&input.data, in_len, out_len)
            .map_err(|e| e.to_string())?;
        let secs = start.elapsed().as_secs_f64();
        let cmp = compare(&ours, &reference.data)?;
        all &= report(&format!("regulator {in_len}->{out_len}"), &cmp, secs, Some(2e-3));
    }
    Ok(all)
}

/// Hidden bisection stage: compares decoder intermediates against the
/// codecdbg_*.npy dumps from dump_s2mel_extra.py.
fn run_codec_debug(dumps: &Path, weights: &Path) -> Result<bool, String> {
    let decoder =
        SemanticCodecDecoder::load(weights.join("codec.pth")).map_err(|e| e.to_string())?;
    let codes_npy = load_npy(&dumps.join("semantic_codes.npy"))?;
    let codes: Vec<u32> = codes_npy.data.iter().map(|&v| v.round() as u32).collect();
    let mut stages: Vec<(String, Vec<f32>)> = Vec::new();
    decoder
        .decode_traced(&codes, &mut |name, values| {
            stages.push((name.to_string(), values.to_vec()));
        })
        .map_err(|e| e.to_string())?;
    let mut all = true;
    for (name, ours) in &stages {
        let path = dumps.join(format!("codecdbg_{name}.npy"));
        if !path.is_file() {
            println!("[codec-debug {name}] no reference dump, skipped");
            continue;
        }
        let reference = load_npy(&path)?;
        let cmp = compare(ours, &reference.data)?;
        all &= report(&format!("codec-debug {name}"), &cmp, 0.0, Some(2e-3));
    }
    Ok(all)
}

struct CfmInputs {
    mu: Vec<f32>,
    t_len: usize,
    prompt_mel: Vec<f32>,
    prompt_len: usize,
    style: Vec<f32>,
    noise: Vec<f32>,
}

fn load_cfm_inputs(dumps: &Path) -> Result<CfmInputs, String> {
    let mu = load_npy(&dumps.join("cat_condition.npy"))?; // (1, T, 512)
    let prompt = load_npy(&dumps.join("ref_mel.npy"))?; // (1, 80, P)
    let style = load_npy(&dumps.join("campplus_style.npy"))?; // (1, 192)
    let noise = load_npy(&dumps.join("cfm_noise.npy"))?; // (1, 80, T)
    let t_len = mu.shape[1];
    let prompt_len = prompt.shape[2];
    if noise.shape != vec![1, 80, t_len] {
        return Err(format!("cfm_noise shape {:?} != (1,80,{t_len})", noise.shape));
    }
    Ok(CfmInputs {
        mu: mu.data,
        t_len,
        prompt_mel: prompt.data,
        prompt_len,
        style: style.data,
        noise: noise.data,
    })
}

fn run_dit(dumps: &Path, s2mel: &S2mel) -> Result<bool, String> {
    let inputs = load_cfm_inputs(dumps)?;
    let reference = load_npy(&dumps.join("cfm_v_step1.npy"))?;
    let (t_len, p) = (inputs.t_len, inputs.prompt_len);
    // Step-1 state: x = noise with the prompt region zeroed; prompt_x padded.
    let mut x = inputs.noise.clone();
    let mut prompt_x = vec![0f32; 80 * t_len];
    for c in 0..80 {
        x[c * t_len..c * t_len + p].fill(0.0);
        prompt_x[c * t_len..c * t_len + p]
            .copy_from_slice(&inputs.prompt_mel[c * p..(c + 1) * p]);
    }
    let zero_prompt = vec![0f32; 80 * t_len];
    let zero_style = vec![0f32; 192];
    let zero_mu = vec![0f32; t_len * 512];
    let cfg = 0.7f32;
    let start = Instant::now();
    let v_cond = s2mel
        .estimator
        .forward(&x, &prompt_x, 0.0, &inputs.style, &inputs.mu, t_len)
        .map_err(|e| e.to_string())?;
    let mut v = s2mel
        .estimator
        .forward(&x, &zero_prompt, 0.0, &zero_style, &zero_mu, t_len)
        .map_err(|e| e.to_string())?;
    let secs = start.elapsed().as_secs_f64();
    for (u, &c) in v.iter_mut().zip(&v_cond) {
        *u = (1.0 + cfg) * c - cfg * *u;
    }
    println!("dit: {:.2}s per estimator forward (T={t_len})", secs / 2.0);
    let cmp = compare(&v, &reference.data)?;
    Ok(report("dit (cfm_v_step1)", &cmp, secs, Some(2e-3)))
}

fn run_solve(dumps: &Path, s2mel: &S2mel) -> Result<bool, String> {
    let inputs = load_cfm_inputs(dumps)?;
    let v13_ref = load_npy(&dumps.join("cfm_v_step13.npy"))?;
    let v25_ref = load_npy(&dumps.join("cfm_v_step25.npy"))?;
    let mel_ref = load_npy(&dumps.join("cfm_mel_full.npy"))?;
    let vc_ref = load_npy(&dumps.join("vc_target.npy"))?;
    let mut noise = ReplayNoise { data: inputs.noise.clone() };
    let mut v13: Vec<f32> = Vec::new();
    let mut v25: Vec<f32> = Vec::new();
    let mut observe = |step: usize, v: &[f32]| match step {
        13 => v13 = v.to_vec(),
        25 => v25 = v.to_vec(),
        _ => {}
    };
    let start = Instant::now();
    let out = s2mel
        .estimator
        .solve_euler_observed(
            &inputs.mu,
            inputs.t_len,
            &inputs.prompt_mel,
            inputs.prompt_len,
            &inputs.style,
            25,
            0.7,
            &mut noise,
            Some(&mut observe),
            None,
        )
        .map_err(|e| e.to_string())?;
    let secs = start.elapsed().as_secs_f64();
    println!("solve: {secs:.2}s for 25 steps ({:.2}s/step, T={})", secs / 25.0, inputs.t_len);
    let mut all = true;
    all &= report("solve v_step13", &compare(&v13, &v13_ref.data)?, secs, None);
    all &= report("solve v_step25", &compare(&v25, &v25_ref.data)?, secs, None);
    all &= report("solve mel_full", &compare(&out.mel, &mel_ref.data)?, secs, None);
    all &= report("solve vc_target", &compare(&out.vc_target(), &vc_ref.data)?, secs, None);
    Ok(all)
}

// --- main -----------------------------------------------------------------------

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut opts: HashMap<String, String> = HashMap::new();
    let mut key: Option<String> = None;
    for arg in &args[1..] {
        if let Some(name) = arg.strip_prefix("--") {
            key = Some(name.to_string());
            opts.entry(name.to_string()).or_default();
        } else if let Some(name) = key.take() {
            opts.insert(name, arg.clone());
        }
    }
    let dumps = opts
        .get("dumps")
        .map(PathBuf::from)
        .unwrap_or_else(reference_dumps_dir);
    let weights = opts
        .get("weights")
        .map(PathBuf::from)
        .unwrap_or_else(reference_checkpoints_dir);
    let stage = opts.get("stage").map(String::as_str).unwrap_or("all");
    if !matches!(stage, "codec" | "codec-debug" | "regulator" | "dit" | "solve" | "all") {
        eprintln!(
            "usage: indextts-s2mel-validate [--dumps <dir>] [--weights <dir>] \
             [--stage codec|regulator|dit|solve|all]"
        );
        std::process::exit(2);
    }
    println!("dumps: {}  weights: {}  stage: {stage}", dumps.display(), weights.display());

    let mut all_pass = true;
    let mut ran_any = false;
    let fail = |name: &str, err: String, all_pass: &mut bool| {
        eprintln!("[{name}] FAILED: {err}");
        *all_pass = false;
    };

    if matches!(stage, "codec" | "all") {
        ran_any = true;
        match run_codec(&dumps, &weights) {
            Ok(pass) => all_pass &= pass,
            Err(err) => fail("codec", err, &mut all_pass),
        }
    }

    if stage == "codec-debug" {
        ran_any = true;
        match run_codec_debug(&dumps, &weights) {
            Ok(pass) => all_pass &= pass,
            Err(err) => fail("codec-debug", err, &mut all_pass),
        }
    }

    if matches!(stage, "regulator" | "dit" | "solve" | "all") {
        let start = Instant::now();
        match S2mel::load(weights.join("s2mel.pth")) {
            Ok(s2mel) => {
                println!("s2mel load: {:.2}s", start.elapsed().as_secs_f64());
                if matches!(stage, "regulator" | "all") {
                    ran_any = true;
                    match run_regulator(&dumps, &s2mel) {
                        Ok(pass) => all_pass &= pass,
                        Err(err) => fail("regulator", err, &mut all_pass),
                    }
                }
                if matches!(stage, "dit" | "all") {
                    ran_any = true;
                    match run_dit(&dumps, &s2mel) {
                        Ok(pass) => all_pass &= pass,
                        Err(err) => fail("dit", err, &mut all_pass),
                    }
                }
                if matches!(stage, "solve" | "all") {
                    ran_any = true;
                    match run_solve(&dumps, &s2mel) {
                        Ok(pass) => all_pass &= pass,
                        Err(err) => fail("solve", err, &mut all_pass),
                    }
                }
            }
            Err(err) => fail("s2mel-load", err.to_string(), &mut all_pass),
        }
    }

    if all_pass && ran_any {
        println!("S2MEL:OK");
    } else {
        eprintln!("S2MEL:FAIL");
        std::process::exit(1);
    }
}
