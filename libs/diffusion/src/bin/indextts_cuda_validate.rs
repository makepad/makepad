//! IndexTTS-2.5 CUDA-path validation + bench on the 5090 box.
//!
//! Gates the device transformer against the CPU f32 implementation (itself
//! oracle-parity-validated by `indextts_gpt_validate`) and against the
//! oracle dumps directly:
//!
//!   gpt   — step-0/1/2 logits vs dumps (cosine; bf16-vs-f32 gets a relaxed
//!           max-abs), argmax parity per step, then full greedy norp/rp10
//!           sequences vs the meta.json fixtures. The fixtures are f32; the
//!           CUDA path is torch-autocast-bf16 semantics (the deployed
//!           reference precision), so an argmax flip at an f32 near-tie is
//!           expected and accepted ONLY when the f32 teacher-forced logits
//!           (gpt_tf_logits_norp.npy) prove the flipped pair sat within
//!           BF16_TIE_MARGIN at the divergence step. Anything else FAILS.
//!   tf    — whole-sequence teacher-forced sweep: feed the greedy_norp
//!           fixture tokens, compare EVERY step's logits vs the f32 dump
//!           (cosine >= 0.999 per step) and margin-check every argmax
//!           disagreement. This is the definitive transformer-parity gate
//!           (covers KV cache + positions at all depths, not just steps 0-2).
//!   bench — GPT-stage timing: prefill + per-token decode over the fixture
//!           text with production sampling, cold + N warm runs.
//!   dit   — one CUDA CFG-combined estimator pass at t=0 vs cfm_v_step1
//!           (cosine gate; the F16 GEMM spine moves values by ~1e-3 absolute,
//!           so max-abs is reported, not gated).
//!   solve — full 25-step CUDA Euler CFG with replayed cfm_noise. Hard gates:
//!           cfm_mel_full + vc_target (cosine >= 0.999) and the final wav in
//!           the spectrogram domain (CUDA mel through the oracle-validated
//!           CPU BigVGAN, re-analysis mel vs the oracle wav's, cosine >=
//!           0.99; raw sample cosine is informational — the GAN vocoder
//!           turns tiny mel diffs into phase shifts). cfm_v_step13/25 are reported
//!           informationally: mid-trajectory velocities diverge from the f16
//!           GEMM operand spine compounding through the ODE (proven identical
//!           under f16 and f32 accumulate while step-1 velocity at the oracle
//!           input is exact), so they carry no verdict.
//!
//! Usage:
//!   indextts-cuda-validate [--dumps <dir>] [--weights <dir>]
//!                          [--stage all|gpt|tf|bench|codec|dit|solve[,..]] [--warm <n>]

use makepad_diffusion::indextts::{
    reference_checkpoints_dir, reference_dumps_dir, S2MEL_CFG_RATE, S2MEL_CFM_STEPS,
};
use makepad_diffusion::indextts_bigvgan::IndexTtsBigVgan;
use makepad_diffusion::indextts_codec::SemanticCodecDecoder;
use makepad_diffusion::indextts_mel::mel_spectrogram_22k;
use makepad_diffusion::indextts_gpt::{
    apply_repetition_penalty, argmax, gpt_cuda_available, GptSamplingConfig, IndexTtsGpt,
};
use makepad_diffusion::indextts_s2mel::cuda::{S2melCudaRun, S2melCudaSession};
use makepad_diffusion::indextts_s2mel::{S2mel, S2melNoiseSource};
use makepad_micro_serde::{DeJson, JsonValue};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

// ---------------------------------------------------------------------------
// npy loader (same pattern as indextts_gpt_validate.rs)
// ---------------------------------------------------------------------------

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
    Ok(Npy {
        shape,
        descr,
        fortran: header.contains("'fortran_order': True"),
        data: bytes[header_start + header_len..].to_vec(),
    })
}

impl Npy {
    fn as_f32(&self) -> Result<Vec<f32>, String> {
        let data: Vec<f32> = match self.descr.as_str() {
            "<f4" => self
                .data
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
            "<f8" => self
                .data
                .chunks_exact(8)
                .map(|c| {
                    f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]) as f32
                })
                .collect(),
            other => return Err(format!("unsupported npy dtype {other}")),
        };
        // np.save stores transposed torch views column-major; rewrite to C
        // order (same conversion as indextts_s2mel_validate).
        if !self.fortran || self.shape.len() < 2 {
            return Ok(data);
        }
        let mut f_stride = vec![1usize; self.shape.len()];
        for d in 1..self.shape.len() {
            f_stride[d] = f_stride[d - 1] * self.shape[d - 1];
        }
        let mut out = vec![0f32; data.len()];
        let mut idx = vec![0usize; self.shape.len()];
        for value in out.iter_mut() {
            let mut off = 0usize;
            for d in 0..self.shape.len() {
                off += idx[d] * f_stride[d];
            }
            *value = data[off];
            for d in (0..self.shape.len()).rev() {
                idx[d] += 1;
                if idx[d] < self.shape[d] {
                    break;
                }
                idx[d] = 0;
            }
        }
        Ok(out)
    }
}

fn load_f32(dir: &Path, name: &str) -> Vec<f32> {
    let path = dir.join(format!("{name}.npy"));
    let npy = load_npy(&path).unwrap_or_else(|e| die(&e));
    npy.as_f32().unwrap_or_else(|e| die(&format!("{name}: {e}")))
}

// ---------------------------------------------------------------------------
// meta.json
// ---------------------------------------------------------------------------

struct Meta {
    lang_token: u32,
    greedy_norp: Vec<u32>,
    greedy_rp10: Vec<u32>,
}

fn json_number(v: &JsonValue) -> f64 {
    match v {
        JsonValue::F64(f) => *f,
        JsonValue::U64(u) => *u as f64,
        JsonValue::I64(i) => *i as f64,
        _ => die(&format!("meta.json: expected number, got {v:?}")),
    }
}

fn json_num_array(root: &HashMap<String, JsonValue>, key: &str) -> Vec<f64> {
    match root.get(key) {
        Some(JsonValue::Array(items)) => items.iter().map(json_number).collect(),
        other => die(&format!("meta.json: {key} missing or not an array ({other:?})")),
    }
}

fn load_meta(dir: &Path) -> Meta {
    let path = dir.join("meta.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| die(&format!("{}: {e}", path.display())));
    let root = HashMap::<String, JsonValue>::deserialize_json(&text)
        .unwrap_or_else(|e| die(&format!("meta.json parse: {e:?}")));
    Meta {
        lang_token: json_number(root.get("lang_token").unwrap_or(&JsonValue::Null)) as u32,
        greedy_norp: json_num_array(&root, "gpt_greedy_codes_norp")
            .iter()
            .map(|v| *v as u32)
            .collect(),
        greedy_rp10: json_num_array(&root, "gpt_greedy_codes_rp10")
            .iter()
            .map(|v| *v as u32)
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// comparisons
// ---------------------------------------------------------------------------

fn die(msg: &str) -> ! {
    eprintln!("indextts-cuda-validate: {msg}");
    std::process::exit(1);
}

/// Prints max-abs + cosine; passes on cosine >= gate (bf16 vs f32 logits
/// accumulate real absolute error, so the cosine carries the decision and
/// max-abs is reported for the record).
fn compare_cos(name: &str, ours: &[f32], want: &[f32], cos_gate: f64) -> bool {
    if ours.len() != want.len() {
        println!("  {name}: FAIL length {} vs {}", ours.len(), want.len());
        return false;
    }
    let mut max_abs = 0f32;
    let mut dot = 0f64;
    let mut na = 0f64;
    let mut nb = 0f64;
    for (a, b) in ours.iter().zip(want) {
        max_abs = max_abs.max((a - b).abs());
        dot += *a as f64 * *b as f64;
        na += *a as f64 * *a as f64;
        nb += *b as f64 * *b as f64;
    }
    let cosine = dot / (na.sqrt() * nb.sqrt()).max(1e-30);
    let pass = cosine >= cos_gate;
    println!(
        "  {name}: max-abs {max_abs:.3e} cosine {cosine:.6} [{}]",
        if pass { "PASS" } else { "FAIL" }
    );
    pass
}

/// Prints max-abs + cosine with no verdict: mid-trajectory ODE probes carry
/// inherent f16-operand divergence (see the solve stage doc above) and are
/// recorded for drift tracking without gating the run.
fn info_cos(name: &str, ours: &[f32], want: &[f32]) {
    if ours.len() != want.len() {
        println!("  {name}: length {} vs {} [info]", ours.len(), want.len());
        return;
    }
    let mut max_abs = 0f32;
    let mut dot = 0f64;
    let mut na = 0f64;
    let mut nb = 0f64;
    for (a, b) in ours.iter().zip(want) {
        max_abs = max_abs.max((a - b).abs());
        dot += *a as f64 * *b as f64;
        na += *a as f64 * *a as f64;
        nb += *b as f64 * *b as f64;
    }
    let cosine = dot / (na.sqrt() * nb.sqrt()).max(1e-30);
    println!("  {name}: max-abs {max_abs:.3e} cosine {cosine:.6} [info]");
}

fn text_tokens_from_dump(dumps: &Path) -> Vec<u32> {
    load_f32(dumps, "text_tokens").iter().map(|v| *v as u32).collect()
}

/// Max f32 top-2 gap that a bf16 argmax flip is allowed to cross. Observed
/// bf16-vs-f32 per-logit error is ~1.5e-1 max-abs (24 bf16 GEMM layers); two
/// independent logits give gap noise up to ~3e-1. 0.5 adds headroom while
/// still catching real bugs — genuine (non-tie) top-2 gaps are O(1)..O(10).
const BF16_TIE_MARGIN: f32 = 0.5;

const GPT_LOGITS_COLS: usize = 8194;

/// The teacher-forced f32 per-step logits fixture (row i = pre-penalty
/// logits before fixture token i; produced by indextts-gpt-validate
/// --stage tf-dump on the same dumps). None when absent.
struct TfLogits {
    rows: usize,
    data: Vec<f32>,
}

fn load_tf_logits(dumps: &Path) -> Option<TfLogits> {
    let path = dumps.join("gpt_tf_logits_norp.npy");
    if !path.is_file() {
        return None;
    }
    let npy = load_npy(&path).unwrap_or_else(|e| die(&e));
    if npy.shape.len() != 2 || npy.shape[1] != GPT_LOGITS_COLS {
        die(&format!("gpt_tf_logits_norp.npy: bad shape {:?}", npy.shape));
    }
    let data = npy
        .as_f32()
        .unwrap_or_else(|e| die(&format!("gpt_tf_logits_norp: {e}")));
    Some(TfLogits { rows: npy.shape[0], data })
}

impl TfLogits {
    fn row(&self, i: usize) -> &[f32] {
        &self.data[i * GPT_LOGITS_COLS..(i + 1) * GPT_LOGITS_COLS]
    }
}

/// Greedy-sequence gate for the bf16 CUDA path against the f32 fixture:
/// exact match passes; a divergence passes ONLY when the f32 teacher-forced
/// logits prove the flipped pair was a near-tie (penalized gap <=
/// BF16_TIE_MARGIN) at the first divergence step. The tf dump follows the
/// norp path, so the margin check requires the fixture prefix to match the
/// norp prefix up to the divergence (holds for norp trivially; checked for
/// rp10). Whole-sequence transformer parity is the `tf` stage's job.
fn compare_greedy_bf16(
    name: &str,
    ours: &[u32],
    want: &[u32],
    penalty: f32,
    tf: Option<&TfLogits>,
    norp_path: &[u32],
) -> bool {
    if ours == want {
        println!("  {name}: EXACT ({} tokens)", want.len());
        return true;
    }
    let k = ours
        .iter()
        .zip(want)
        .position(|(a, b)| a != b)
        .unwrap_or(ours.len().min(want.len()));
    let (our_tok, want_tok) = match (ours.get(k), want.get(k)) {
        (Some(&a), Some(&b)) => (a, b),
        _ => {
            println!(
                "  {name}: FAIL length {} vs {} (prefix agrees, one ran past the other's stop)",
                ours.len(),
                want.len()
            );
            return false;
        }
    };
    let Some(tf) = tf else {
        println!(
            "  {name}: FAIL diverged at {k} (ours {our_tok} vs {want_tok}) and no \
             gpt_tf_logits_norp.npy to margin-check (run indextts-gpt-validate --stage tf-dump)"
        );
        return false;
    };
    if k >= tf.rows || want[..k] != norp_path[..k] {
        println!(
            "  {name}: FAIL diverged at {k} (ours {our_tok} vs {want_tok}); tf dump follows the \
             norp path and cannot margin-check this step"
        );
        return false;
    }
    // Reconstruct the exact decision input at step k: penalize the f32
    // teacher-forced logits with the fixture's seen-set (prefixes match
    // ours up to k, so both implementations saw the same set).
    let mut row = tf.row(k).to_vec();
    let mut seen = vec![false; GPT_LOGITS_COLS];
    for &t in &want[..k] {
        seen[t as usize] = true;
    }
    apply_repetition_penalty(&mut row, &seen, penalty);
    let gap = (row[want_tok as usize] - row[our_tok as usize]).abs();
    let ok = gap <= BF16_TIE_MARGIN;
    println!(
        "  {name}: {} bf16 argmax flip at step {k}: fixture {want_tok} vs ours {our_tok}, \
         f32 penalized gap {gap:.3} ({} {BF16_TIE_MARGIN}); lens {} vs {}",
        if ok { "PASS" } else { "FAIL" },
        if ok { "<=" } else { ">" },
        ours.len(),
        want.len()
    );
    ok
}

// ---------------------------------------------------------------------------
// stages
// ---------------------------------------------------------------------------

fn stage_gpt(gpt: &IndexTtsGpt, dumps: &Path, meta: &Meta) -> bool {
    println!("stage gpt (cuda):");
    let conds_dump = load_f32(dumps, "conds_latent");
    let text_tokens = text_tokens_from_dump(dumps);

    // Step logits vs the f32 oracle dumps. bf16 GEMMs move logits by ~1e-1
    // absolute over 24 layers; cosine is the meaningful gate here, argmax
    // parity per step is the hard functional gate.
    let t0 = Instant::now();
    let (mut session, logits0) = gpt
        .prefill_cuda(&conds_dump, &text_tokens, meta.lang_token)
        .unwrap_or_else(|e| die(&format!("prefill_cuda: {e:?}")));
    let prefill_dt = t0.elapsed().as_secs_f32();
    let want0 = load_f32(dumps, "gpt_logits_step0");
    let mut ok = compare_cos("gpt_logits_step0", &logits0, &want0, 0.999);
    println!(
        "  (argmax step0: ours {} vs oracle {})",
        argmax(&logits0),
        meta.greedy_norp[0]
    );
    ok &= argmax(&logits0) as u32 == meta.greedy_norp[0];
    let t0 = Instant::now();
    let logits1 = session
        .step(meta.greedy_norp[0])
        .unwrap_or_else(|e| die(&format!("step: {e:?}")));
    let want1 = load_f32(dumps, "gpt_logits_step1");
    ok &= compare_cos("gpt_logits_step1", &logits1, &want1, 0.999);
    ok &= argmax(&logits1) as u32 == meta.greedy_norp[1];
    let logits2 = session
        .step(meta.greedy_norp[1])
        .unwrap_or_else(|e| die(&format!("step: {e:?}")));
    let want2 = load_f32(dumps, "gpt_logits_step2");
    ok &= compare_cos("gpt_logits_step2", &logits2, &want2, 0.999);
    ok &= argmax(&logits2) as u32 == meta.greedy_norp[2];
    let step_dt = t0.elapsed().as_secs_f32() / 2.0;
    println!("  (prefill {prefill_dt:.3}s, decode {:.1}ms/token)", step_dt * 1e3);

    // Full greedy sequences vs the oracle fixtures (bf16 near-tie flips
    // accepted only when the f32 tf dump proves the margin).
    let tf = load_tf_logits(dumps);
    for (name, penalty, want) in [
        ("gpt_greedy_codes_norp", 1.0f32, &meta.greedy_norp),
        ("gpt_greedy_codes_rp10", 10.0f32, &meta.greedy_rp10),
    ] {
        let t0 = Instant::now();
        let codes = gpt
            .generate_cuda_observed(
                &conds_dump,
                &text_tokens,
                meta.lang_token,
                &GptSamplingConfig::greedy(penalty),
                None,
            )
            .unwrap_or_else(|e| die(&format!("generate cuda: {e:?}")));
        let dt = t0.elapsed().as_secs_f32();
        ok &= compare_greedy_bf16(name, &codes, want, penalty, tf.as_ref(), &meta.greedy_norp);
        println!(
            "  ({name}: {} tokens in {dt:.2}s = {:.1}ms/token)",
            codes.len(),
            dt * 1e3 / codes.len().max(1) as f32
        );
    }
    ok
}

/// Whole-sequence teacher-forced parity: feed the greedy_norp fixture tokens
/// and gate EVERY step's CUDA logits against the f32 dump. Catches KV-cache,
/// position and mask bugs at all sequence depths. Argmax disagreements are
/// allowed only at f32 near-ties (penalty 1.0 on this path, so raw logits
/// are the decision values).
fn stage_tf(gpt: &IndexTtsGpt, dumps: &Path, meta: &Meta) -> bool {
    println!("stage tf (cuda, teacher-forced whole sequence):");
    let Some(tf) = load_tf_logits(dumps) else {
        println!("  FAIL: gpt_tf_logits_norp.npy missing (run indextts-gpt-validate --stage tf-dump)");
        return false;
    };
    let n = meta.greedy_norp.len();
    if tf.rows != n {
        println!("  FAIL: tf dump rows {} != fixture len {n}", tf.rows);
        return false;
    }
    let conds_dump = load_f32(dumps, "conds_latent");
    let text_tokens = text_tokens_from_dump(dumps);

    let t0 = Instant::now();
    let (mut session, mut logits) = gpt
        .prefill_cuda(&conds_dump, &text_tokens, meta.lang_token)
        .unwrap_or_else(|e| die(&format!("prefill_cuda: {e:?}")));
    let mut min_cos = 1f64;
    let mut min_cos_step = 0usize;
    let mut worst_abs = 0f32;
    let mut flips = 0usize;
    let mut ok = true;
    for i in 0..n {
        let want = tf.row(i);
        let mut dot = 0f64;
        let mut na = 0f64;
        let mut nb = 0f64;
        let mut max_abs = 0f32;
        for (a, b) in logits.iter().zip(want) {
            max_abs = max_abs.max((a - b).abs());
            dot += *a as f64 * *b as f64;
            na += *a as f64 * *a as f64;
            nb += *b as f64 * *b as f64;
        }
        let cos = dot / (na.sqrt() * nb.sqrt()).max(1e-30);
        if cos < min_cos {
            min_cos = cos;
            min_cos_step = i;
        }
        worst_abs = worst_abs.max(max_abs);
        if cos < 0.999 {
            println!("  step {i}: FAIL cosine {cos:.6} (max-abs {max_abs:.3e})");
            ok = false;
        }
        let ours_am = argmax(&logits);
        let want_am = meta.greedy_norp[i] as usize;
        if ours_am != want_am {
            flips += 1;
            let gap = (want[want_am] - want[ours_am]).abs();
            let tie = gap <= BF16_TIE_MARGIN;
            println!(
                "  step {i}: argmax flip fixture {want_am} vs ours {ours_am}, f32 gap {gap:.3} \
                 [{}]",
                if tie { "near-tie OK" } else { "FAIL" }
            );
            ok &= tie;
        }
        if i + 1 < n {
            logits = session
                .step(meta.greedy_norp[i])
                .unwrap_or_else(|e| die(&format!("step: {e:?}")));
        }
    }
    let dt = t0.elapsed().as_secs_f32();
    println!(
        "  {n} steps: min cosine {min_cos:.6} (step {min_cos_step}), worst max-abs \
         {worst_abs:.3e}, {flips} argmax flips (all near-tie: {}) [{}] ({dt:.2}s)",
        if ok { "yes" } else { "NO" },
        if ok { "PASS" } else { "FAIL" }
    );
    ok
}

fn stage_bench(gpt: &IndexTtsGpt, dumps: &Path, meta: &Meta, warm: usize) -> bool {
    println!("stage bench (gpt cuda, production sampling):");
    let conds_dump = load_f32(dumps, "conds_latent");
    let text_tokens = text_tokens_from_dump(dumps);
    let mut cfg = GptSamplingConfig::default();
    let mut times = Vec::new();
    for run in 0..warm + 1 {
        cfg.seed = 42 + run as u64;
        let t0 = Instant::now();
        let codes = gpt
            .generate_cuda_observed(&conds_dump, &text_tokens, meta.lang_token, &cfg, None)
            .unwrap_or_else(|e| die(&format!("generate cuda: {e:?}")));
        let dt = t0.elapsed().as_secs_f32();
        let label = if run == 0 { "cold" } else { "warm" };
        println!(
            "  {label} run {run}: {dt:.3}s for {} tokens ({:.1}ms/token)",
            codes.len(),
            dt * 1e3 / codes.len().max(1) as f32
        );
        if run > 0 {
            times.push(dt);
        }
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if !times.is_empty() {
        let median = times[times.len() / 2];
        let p95 = times[((times.len() as f32 * 0.95) as usize).min(times.len() - 1)];
        println!("  gpt_gen warm median {median:.3}s p95 {p95:.3}s over {} runs", times.len());
    }
    true
}

// ---------------------------------------------------------------------------
// s2mel stages (CUDA estimator vs the oracle CFM fixtures; mirrors the
// fixture wiring of indextts_s2mel_validate.rs)
// ---------------------------------------------------------------------------

struct ReplayNoise {
    data: Vec<f32>,
}

impl S2melNoiseSource for ReplayNoise {
    fn draw(&mut self, _index: usize, len: usize) -> Vec<f32> {
        assert_eq!(len, self.data.len(), "replay noise length mismatch");
        self.data.clone()
    }
}

struct CfmInputs {
    mu: Vec<f32>,
    t_len: usize,
    prompt_mel: Vec<f32>,
    prompt_len: usize,
    style: Vec<f32>,
    noise: Vec<f32>,
}

fn load_cfm_inputs(dumps: &Path) -> CfmInputs {
    let mu = load_npy(&dumps.join("cat_condition.npy")).unwrap_or_else(|e| die(&e)); // (1, T, 512)
    let prompt = load_npy(&dumps.join("ref_mel.npy")).unwrap_or_else(|e| die(&e)); // (1, 80, P)
    let style = load_npy(&dumps.join("campplus_style.npy")).unwrap_or_else(|e| die(&e)); // (1, 192)
    let noise = load_npy(&dumps.join("cfm_noise.npy")).unwrap_or_else(|e| die(&e)); // (1, 80, T)
    let t_len = mu.shape[1];
    let prompt_len = prompt.shape[2];
    if noise.shape != vec![1, 80, t_len] {
        die(&format!("cfm_noise shape {:?} != (1,80,{t_len})", noise.shape));
    }
    CfmInputs {
        mu: mu.as_f32().unwrap_or_else(|e| die(&e)),
        t_len,
        prompt_mel: prompt.as_f32().unwrap_or_else(|e| die(&e)),
        prompt_len,
        style: style.as_f32().unwrap_or_else(|e| die(&e)),
        noise: noise.as_f32().unwrap_or_else(|e| die(&e)),
    }
}

/// One CUDA CFG-combined estimator pass at t=0 (step 1) vs cfm_v_step1.
fn stage_dit(s2mel: &S2mel, session: &S2melCudaSession, dumps: &Path) -> bool {
    println!("stage dit (cuda):");
    let inputs = load_cfm_inputs(dumps);
    let want = load_f32(dumps, "cfm_v_step1");
    let (t_len, p) = (inputs.t_len, inputs.prompt_len);
    // Step-1 state: x = noise with the prompt region zeroed.
    let mut x = inputs.noise.clone();
    for c in 0..80 {
        x[c * t_len..c * t_len + p].fill(0.0);
    }
    let run = S2melCudaRun::new(
        session,
        &s2mel.estimator,
        &inputs.mu,
        t_len,
        &inputs.prompt_mel,
        p,
        &inputs.style,
    )
    .unwrap_or_else(|e| die(&format!("s2mel run: {e:?}")));
    let t0 = Instant::now();
    let v = run
        .velocity_cfg(1, &x, S2MEL_CFG_RATE)
        .unwrap_or_else(|e| die(&format!("velocity_cfg: {e:?}")));
    let dt = t0.elapsed().as_secs_f32();
    println!("  (T={t_len}, {:.1}ms per batched CFG forward)", dt * 1e3);
    compare_cos("cfm_v_step1", &v, &want, 0.999)
}

/// Full 25-step CUDA Euler CFG solve with replayed noise. Hard-gates the
/// final mel artifacts and the waveform (CUDA mel through the CPU BigVGAN vs
/// the oracle wav); reports mid-trajectory velocities informationally; also
/// times a warm second solve (the production path's s2mel cost).
fn stage_solve(s2mel: &S2mel, session: &S2melCudaSession, dumps: &Path, weights: &Path) -> bool {
    println!("stage solve (cuda, {} steps):", S2MEL_CFM_STEPS);
    let inputs = load_cfm_inputs(dumps);
    let v13_want = load_f32(dumps, "cfm_v_step13");
    let v25_want = load_f32(dumps, "cfm_v_step25");
    let mel_want = load_f32(dumps, "cfm_mel_full");
    let vc_want = load_f32(dumps, "vc_target");
    let mut v13: Vec<f32> = Vec::new();
    let mut v25: Vec<f32> = Vec::new();
    let mut observe = |step: usize, v: &[f32]| match step {
        13 => v13 = v.to_vec(),
        25 => v25 = v.to_vec(),
        _ => {}
    };
    let mut noise = ReplayNoise { data: inputs.noise.clone() };
    let t0 = Instant::now();
    let out = session
        .solve_euler_observed(
            &s2mel.estimator,
            &inputs.mu,
            inputs.t_len,
            &inputs.prompt_mel,
            inputs.prompt_len,
            &inputs.style,
            S2MEL_CFG_RATE,
            &mut noise,
            Some(&mut observe),
            None,
        )
        .unwrap_or_else(|e| die(&format!("solve cuda: {e:?}")));
    let dt = t0.elapsed().as_secs_f32();
    println!(
        "  (cold solve {dt:.3}s = {:.1}ms/step, T={})",
        dt * 1e3 / S2MEL_CFM_STEPS as f32,
        inputs.t_len
    );
    let mut ok = true;
    info_cos("cfm_v_step13", &v13, &v13_want);
    info_cos("cfm_v_step25", &v25, &v25_want);
    ok &= compare_cos("cfm_mel_full", &out.mel, &mel_want, 0.999);
    ok &= compare_cos("vc_target", &out.vc_target(), &vc_want, 0.999);
    // Wav-level gate: the CUDA mel through the oracle-validated CPU BigVGAN
    // vs the reference waveform, compared in the spectrogram domain
    // (re-analysis mel of both wavs). Raw waveform cosine is informational
    // only — the GAN vocoder turns sub-1e-3 mel input differences into
    // carrier phase shifts that collapse sample-wise correlation while the
    // audio content stays matched; the re-analysis mel is phase-robust and
    // still catches any real assembly error (wrong slice, layout, scaling).
    let wav_want = load_f32(dumps, "bigvgan_wav");
    let frames = inputs.t_len - inputs.prompt_len;
    let t0 = Instant::now();
    let bigvgan = IndexTtsBigVgan::load(&weights.join("hf_cache/bigvgan/bigvgan_generator.pt"))
        .unwrap_or_else(|e| die(&format!("load bigvgan: {e:?}")));
    let load_s = t0.elapsed().as_secs_f32();
    let t0 = Instant::now();
    let wav = bigvgan
        .synthesize_cpu(&out.vc_target(), frames)
        .unwrap_or_else(|e| die(&format!("bigvgan synthesize cpu: {e:?}")));
    println!(
        "  (bigvgan load {load_s:.2}s, cpu synth {:.2}s, {frames} frames)",
        t0.elapsed().as_secs_f32()
    );
    info_cos("bigvgan_wav", &wav, &wav_want);
    let (mel_ours, _) = mel_spectrogram_22k(&wav);
    let (mel_ref, _) = mel_spectrogram_22k(&wav_want);
    ok &= compare_cos("bigvgan_wav_mel", &mel_ours, &mel_ref, 0.99);
    if bigvgan.cuda_active() {
        // Device path gates: sample-domain vs the CPU reference (same weights,
        // same input — phase fragility does not apply here, both are the same
        // deterministic network, so the bar is tight) and spectrogram-domain
        // vs the reference dump (same bar as the CPU gate above).
        let t0 = Instant::now();
        let _ = bigvgan
            .synthesize(&out.vc_target(), frames)
            .unwrap_or_else(|e| die(&format!("bigvgan synthesize cuda: {e:?}")));
        let cold_s = t0.elapsed().as_secs_f32();
        let t0 = Instant::now();
        let wav_cuda = bigvgan
            .synthesize(&out.vc_target(), frames)
            .unwrap_or_else(|e| {
                die(&format!("bigvgan synthesize cuda warm: {e:?}"));
            });
        println!(
            "  (bigvgan cuda synth cold {cold_s:.2}s, warm {:.2}s)",
            t0.elapsed().as_secs_f32()
        );
        ok &= compare_cos("bigvgan_cuda_vs_cpu", &wav_cuda, &wav, 0.999);
        let (mel_cuda, _) = mel_spectrogram_22k(&wav_cuda);
        ok &= compare_cos("bigvgan_cuda_wav_mel", &mel_cuda, &mel_ref, 0.99);
    } else {
        println!("  (bigvgan cuda session absent — device gates skipped)");
    }
    // Warm timing (weights + tables resident, fresh run state).
    let mut noise = ReplayNoise { data: inputs.noise };
    let t0 = Instant::now();
    let _ = session
        .solve_euler_observed(
            &s2mel.estimator,
            &inputs.mu,
            inputs.t_len,
            &inputs.prompt_mel,
            inputs.prompt_len,
            &inputs.style,
            S2MEL_CFG_RATE,
            &mut noise,
            None,
            None,
        )
        .unwrap_or_else(|e| die(&format!("solve cuda warm: {e:?}")));
    println!("  (warm solve {:.3}s)", t0.elapsed().as_secs_f32());
    ok
}

// ---------------------------------------------------------------------------

fn stage_codec(dumps: &Path, weights: &Path) -> bool {
    println!("stage codec (cuda):");
    let mut ok = true;
    let codes: Vec<u32> = load_f32(dumps, "semantic_codes")
        .iter()
        .map(|&v| v.round() as u32)
        .collect();
    let want = load_f32(dumps, "s_infer");
    let t0 = Instant::now();
    let decoder = SemanticCodecDecoder::load(weights.join("codec.pth"))
        .unwrap_or_else(|e| die(&format!("load codec.pth: {e:?}")));
    let load_s = t0.elapsed().as_secs_f32();
    let t0 = Instant::now();
    let cpu = decoder
        .decode_cpu(&codes)
        .unwrap_or_else(|e| die(&format!("codec decode cpu: {e:?}")));
    println!(
        "  (codec load {load_s:.2}s, cpu decode {:.3}s, {} codes)",
        t0.elapsed().as_secs_f32(),
        codes.len()
    );
    ok &= compare_cos("codec_cpu_vs_oracle", &cpu, &want, 0.999);
    if decoder.cuda_active() {
        let t0 = Instant::now();
        let _ = decoder
            .decode(&codes)
            .unwrap_or_else(|e| die(&format!("codec decode cuda: {e:?}")));
        let cold_s = t0.elapsed().as_secs_f32();
        let t0 = Instant::now();
        let cuda = decoder
            .decode(&codes)
            .unwrap_or_else(|e| die(&format!("codec decode cuda warm: {e:?}")));
        println!(
            "  (codec cuda decode cold {cold_s:.3}s, warm {:.3}s)",
            t0.elapsed().as_secs_f32()
        );
        ok &= compare_cos("codec_cuda_vs_cpu", &cuda, &cpu, 0.999);
        ok &= compare_cos("codec_cuda_vs_oracle", &cuda, &want, 0.999);
    } else {
        println!("  (codec cuda session absent — device gates skipped)");
    }
    ok
}

fn main() {
    let mut dumps = reference_dumps_dir();
    let mut weights = reference_checkpoints_dir();
    let mut stage = "all".to_string();
    let mut warm = 12usize;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dumps" => {
                i += 1;
                dumps = PathBuf::from(args.get(i).unwrap_or_else(|| die("--dumps needs a value")));
            }
            "--weights" => {
                i += 1;
                weights =
                    PathBuf::from(args.get(i).unwrap_or_else(|| die("--weights needs a value")));
            }
            "--stage" => {
                i += 1;
                stage = args.get(i).unwrap_or_else(|| die("--stage needs a value")).clone();
            }
            "--warm" => {
                i += 1;
                warm = args
                    .get(i)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| die("--warm needs a number"));
            }
            other => die(&format!("unknown argument {other}")),
        }
        i += 1;
    }
    if !gpt_cuda_available() {
        die("no CUDA device available (run on the 5090 box)");
    }
    if !dumps.join("meta.json").is_file() {
        die(&format!("dumps dir {} has no meta.json", dumps.display()));
    }
    let meta = load_meta(&dumps);
    let t0 = Instant::now();
    eprintln!("loading gpt.pth from {} ...", weights.display());
    let gpt =
        IndexTtsGpt::load(&weights).unwrap_or_else(|e| die(&format!("load gpt.pth: {e:?}")));
    eprintln!("loaded in {:.1}s", t0.elapsed().as_secs_f32());

    let stages: Vec<&str> = stage.split(',').collect();
    let want_stage = |name: &str| stages.iter().any(|s| *s == "all" || *s == name);
    if !stages
        .iter()
        .all(|s| matches!(*s, "all" | "gpt" | "tf" | "bench" | "codec" | "dit" | "solve"))
    {
        die(&format!("unknown stage in {stage}"));
    }
    let mut ok = true;
    if want_stage("gpt") {
        ok &= stage_gpt(&gpt, &dumps, &meta);
    }
    if want_stage("tf") {
        ok &= stage_tf(&gpt, &dumps, &meta);
    }
    if want_stage("bench") {
        ok &= stage_bench(&gpt, &dumps, &meta, warm);
    }
    if want_stage("codec") {
        ok &= stage_codec(&dumps, &weights);
    }
    if want_stage("dit") || want_stage("solve") {
        let t0 = Instant::now();
        eprintln!("loading s2mel.pth ...");
        let s2mel = S2mel::load(weights.join("s2mel.pth"))
            .unwrap_or_else(|e| die(&format!("load s2mel.pth: {e:?}")));
        let session = S2melCudaSession::new(&s2mel.estimator, S2MEL_CFM_STEPS)
            .unwrap_or_else(|e| die(&format!("s2mel session: {e:?}")));
        eprintln!("loaded in {:.1}s", t0.elapsed().as_secs_f32());
        if want_stage("dit") {
            ok &= stage_dit(&s2mel, &session, &dumps);
        }
        if want_stage("solve") {
            ok &= stage_solve(&s2mel, &session, &dumps, &weights);
        }
    }
    if ok {
        println!("indextts-cuda-validate: ALL PASS");
    } else {
        println!("indextts-cuda-validate: FAIL");
        std::process::exit(1);
    }
}
