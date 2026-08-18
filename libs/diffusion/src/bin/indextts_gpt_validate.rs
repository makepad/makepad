//! IndexTTS-2.5 GPT stage validation against the reference oracle dumps
//! (`local/indextts_ref/dump_oracle.py` + `dump_oracle2.py` ->
//! `local/indextts_ref/dumps/`).
//!
//! Stages (bisection order):
//!   emo     — conformer / perceiver / emovec_layer / emo_layer taps
//!   emovec  — merge_emovec(spk, emo, alpha=1) + the 8-dim vector mixing
//!   prefill — spk_latent, conds_latent, prefill embeds, transformer hidden
//!   logits  — greedy steps 0/1/2 (pre-penalty logits, KV-cached decode)
//!   greedy  — full token parity vs gpt_greedy_codes_norp / _rp10 (EXACT)
//!   tf-dump — generator (not in `all`): teacher-force the greedy_norp
//!             fixture path and write per-step pre-penalty f32 logits to
//!             dumps/gpt_tf_logits_norp.npy [n_codes, 8194]; row i is the
//!             logits BEFORE token i is chosen. This is the whole-sequence
//!             reference the CUDA validator's `tf` stage compares against
//!             (bf16 argmax flips are gated by the f32 near-tie margin).
//!
//! Usage:
//!   indextts-gpt-validate [--dumps <dir>] [--weights <dir>]
//!                         [--stage all|emo|emovec|prefill|logits|greedy|tf-dump]
//!
//! Tensor gates: cosine >= 0.999 and max-abs <= 2e-3 (logits accumulate over
//! 24 layers; they get a relaxed 2e-2 max-abs gate and are reported).
//! Token stages must match EXACTLY.

use makepad_diffusion::indextts::{reference_checkpoints_dir, reference_dumps_dir, GPT_START_MEL};
use makepad_diffusion::indextts_gpt::{
    apply_repetition_penalty, argmax, EmotionMatrices, GptSamplingConfig, IndexTtsGpt,
    EMO_INPUT_DIM,
};
use makepad_micro_serde::{DeJson, JsonValue};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

// ---------------------------------------------------------------------------
// npy loader (same pattern as moss_validate.rs)
// ---------------------------------------------------------------------------

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
    if header.contains("'fortran_order': True") {
        return Err(format!("{}: fortran order unsupported", path.display()));
    }
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
            other => Err(format!("unsupported npy dtype {other}")),
        }
    }
}

fn load_f32(dir: &Path, name: &str) -> (Vec<f32>, Vec<usize>) {
    let path = dir.join(format!("{name}.npy"));
    let npy = load_npy(&path).unwrap_or_else(|e| die(&e));
    let data = npy.as_f32().unwrap_or_else(|e| die(&format!("{name}: {e}")));
    (data, npy.shape)
}

/// Minimal npy v1.0 writer (<f4, C order, 2-D shape) for the tf-dump stage.
fn write_npy_f32(path: &Path, rows: usize, cols: usize, data: &[f32]) {
    assert_eq!(data.len(), rows * cols);
    let mut header = format!("{{'descr': '<f4', 'fortran_order': False, 'shape': ({rows}, {cols}), }}");
    // Pad so magic(6) + version(2) + len(2) + header is 64-aligned, '\n'-terminated.
    let unpadded = 6 + 2 + 2 + header.len() + 1;
    header.push_str(&" ".repeat((64 - unpadded % 64) % 64));
    header.push('\n');
    let mut bytes = Vec::with_capacity(10 + header.len() + data.len() * 4);
    bytes.extend_from_slice(b"\x93NUMPY\x01\x00");
    bytes.extend_from_slice(&(header.len() as u16).to_le_bytes());
    bytes.extend_from_slice(header.as_bytes());
    for v in data {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap_or_else(|e| die(&format!("{}: {e}", path.display())));
}

// ---------------------------------------------------------------------------
// meta.json
// ---------------------------------------------------------------------------

struct Meta {
    lang_token: u32,
    text_tokens: Vec<u32>,
    emo_vector_normalized: [f32; 8],
    emo_random_index: Vec<usize>,
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
    let emo_vec = json_num_array(&root, "emo_vector_normalized");
    let mut emo_vector_normalized = [0f32; 8];
    for (i, v) in emo_vec.iter().take(8).enumerate() {
        emo_vector_normalized[i] = *v as f32;
    }
    Meta {
        lang_token: json_number(root.get("lang_token").unwrap_or(&JsonValue::Null)) as u32,
        text_tokens: json_num_array(&root, "text_tokens").iter().map(|v| *v as u32).collect(),
        emo_vector_normalized,
        emo_random_index: json_num_array(&root, "emo_random_index")
            .iter()
            .map(|v| *v as usize)
            .collect(),
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
    eprintln!("indextts-gpt-validate: {msg}");
    std::process::exit(1);
}

/// Prints max-abs + cosine, returns pass/fail against the given max-abs gate
/// (cosine gate is always 0.999).
fn compare(name: &str, ours: &[f32], want: &[f32], max_abs_gate: f32) -> bool {
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
    let pass = cosine >= 0.999 && max_abs <= max_abs_gate;
    println!(
        "  {name}: max-abs {max_abs:.3e} cosine {cosine:.6} [{}]",
        if pass { "PASS" } else { "FAIL" }
    );
    pass
}

fn compare_tokens(name: &str, ours: &[u32], want: &[u32]) -> bool {
    if ours == want {
        println!("  {name}: EXACT ({} tokens)", want.len());
        return true;
    }
    let mismatch = ours
        .iter()
        .zip(want)
        .position(|(a, b)| a != b)
        .unwrap_or(ours.len().min(want.len()));
    println!(
        "  {name}: FAIL len {} vs {}, first mismatch at {mismatch} (ours {:?} vs {:?})",
        ours.len(),
        want.len(),
        ours.get(mismatch),
        want.get(mismatch)
    );
    false
}

// ---------------------------------------------------------------------------
// stages
// ---------------------------------------------------------------------------

struct Ctx {
    dumps: PathBuf,
    weights: PathBuf,
    gpt: Option<IndexTtsGpt>,
}

impl Ctx {
    fn gpt(&mut self) -> &IndexTtsGpt {
        if self.gpt.is_none() {
            let t0 = Instant::now();
            eprintln!("loading gpt.pth from {} ...", self.weights.display());
            let gpt = IndexTtsGpt::load(&self.weights)
                .unwrap_or_else(|e| die(&format!("load gpt.pth: {e:?}")));
            eprintln!("loaded in {:.1}s", t0.elapsed().as_secs_f32());
            self.gpt = Some(gpt);
        }
        self.gpt.as_ref().unwrap()
    }
}

/// The emo-path dumps were produced from spk_cond_emb (dump_oracle2.py uses
/// the resampled speaker features; emo_cond_emb.npy is a differently-loaded
/// copy of the same audio and does NOT match these taps).
fn stage_emo(ctx: &mut Ctx) -> bool {
    println!("stage emo:");
    let dumps = ctx.dumps.clone();
    let ctx = &mut *ctx;
    let (cond, shape) = load_f32(&dumps, "spk_cond_emb");
    let frames = shape[1];
    assert_eq!(shape[2], EMO_INPUT_DIM);
    let gpt = ctx.gpt();
    let t0 = Instant::now();
    let (conf, tp) = gpt
        .emo_conformer(&cond, frames)
        .unwrap_or_else(|e| die(&format!("emo_conformer: {e:?}")));
    let dt = t0.elapsed().as_secs_f32();
    let (want_conf, want_shape) = load_f32(&dumps, "emo_conformer_out");
    assert_eq!(tp, want_shape[1], "conformer frames");
    let mut ok = compare("emo_conformer_out", &conf, &want_conf, 2e-3);
    let perc = gpt.emo_perceiver(&conf, tp);
    let (want_perc, _) = load_f32(&dumps, "emo_perceiver_out");
    ok &= compare("emo_perceiver_out", &perc, &want_perc, 2e-3);
    let ev = gpt.apply_emovec_layer(&perc);
    let (want_ev, _) = load_f32(&dumps, "emo_emovec_layer_out");
    ok &= compare("emo_emovec_layer_out", &ev, &want_ev, 2e-3);
    let el = gpt.apply_emo_layer(&ev);
    let (want_el, _) = load_f32(&dumps, "emo_emo_layer_out");
    ok &= compare("emo_emo_layer_out", &el, &want_el, 2e-3);
    println!("  (conformer forward: {dt:.2}s for {frames} frames)");
    ok
}

fn stage_emovec(ctx: &mut Ctx, meta: &Meta) -> bool {
    println!("stage emovec:");
    let dumps = ctx.dumps.clone();
    let weights = ctx.weights.clone();
    let (spk, spk_shape) = load_f32(&dumps, "spk_cond_emb");
    let (emo, emo_shape) = load_f32(&dumps, "emo_cond_emb");
    let gpt = ctx.gpt();
    let merged = gpt
        .merge_emovec(&spk, spk_shape[1], &emo, emo_shape[1], 1.0)
        .unwrap_or_else(|e| die(&format!("merge_emovec: {e:?}")));
    let (want, _) = load_f32(&dumps, "emovec_ref");
    let mut ok = compare("emovec_ref", &merged, &want, 2e-3);

    // 8-dim vector mixing (feat1/feat2), against the DUMPED emovec_ref so the
    // mixing arithmetic is isolated from the emo path above.
    let mats =
        EmotionMatrices::load(&weights).unwrap_or_else(|e| die(&format!("feat1/feat2: {e:?}")));
    let (style, _) = load_f32(&dumps, "campplus_style");
    let mix = mats.mix(&style, &meta.emo_vector_normalized, &want);
    if mix.picks.to_vec() == meta.emo_random_index {
        println!("  emo_random_index: EXACT {:?}", mix.picks);
    } else {
        println!(
            "  emo_random_index: FAIL ours {:?} vs meta {:?}",
            mix.picks, meta.emo_random_index
        );
        ok = false;
    }
    let (want_mat, _) = load_f32(&dumps, "emovec_mat_sad");
    ok &= compare("emovec_mat_sad", &mix.emovec_mat, &want_mat, 2e-3);
    let (want_sad, _) = load_f32(&dumps, "emovec_sad");
    ok &= compare("emovec_sad", &mix.emovec, &want_sad, 2e-3);
    ok
}

fn text_tokens_from_dump(dumps: &Path) -> Vec<u32> {
    let (toks, _) = load_f32(dumps, "text_tokens");
    toks.iter().map(|v| *v as u32).collect()
}

fn stage_prefill(ctx: &mut Ctx, meta: &Meta) -> bool {
    println!("stage prefill:");
    let dumps = ctx.dumps.clone();
    let (style, _) = load_f32(&dumps, "campplus_style");
    let (emovec_ref, _) = load_f32(&dumps, "emovec_ref");
    let text_tokens = text_tokens_from_dump(&dumps);
    let (conds_dump, _) = load_f32(&dumps, "conds_latent");
    let gpt = ctx.gpt();

    let spk = gpt.spk_latent(&style);
    let (want_spk, _) = load_f32(&dumps, "spk_latent");
    let mut ok = compare("spk_latent", &spk, &want_spk, 2e-3);

    let conds = gpt.conds_latent(&style, &emovec_ref);
    ok &= compare("conds_latent", &conds, &conds_dump, 2e-3);

    // Embeds + hidden from the DUMPED conds_latent (same chain as
    // dump_oracle2.py) to isolate this stage.
    let prefill = gpt
        .prefill_embeds(&conds_dump, &text_tokens, meta.lang_token)
        .unwrap_or_else(|e| die(&format!("prefill_embeds: {e:?}")));
    let (want_embeds, want_shape) = load_f32(&dumps, "gpt_prefill_embeds");
    assert_eq!(prefill.rows, want_shape[1], "prefill rows");
    println!("  (prefill rows {}, n_pad {})", prefill.rows, prefill.n_pad);
    ok &= compare("gpt_prefill_embeds", &prefill.embeds, &want_embeds, 2e-3);

    // gpt_prefill_hidden was dumped WITHOUT the pad attention mask
    // (dump_oracle2.py calls im.transformer directly), so masked_prefix = 0.
    let mut embeds = prefill.embeds.clone();
    embeds.extend_from_slice(&gpt.mel_input_row(GPT_START_MEL, 0));
    let t0 = Instant::now();
    let hidden = gpt.forward_hidden(&embeds, prefill.rows + 1, 0);
    let dt = t0.elapsed().as_secs_f32();
    let (want_hidden, _) = load_f32(&dumps, "gpt_prefill_hidden");
    ok &= compare("gpt_prefill_hidden", &hidden, &want_hidden, 2e-3);
    println!("  (transformer prefill forward: {dt:.2}s for {} rows)", prefill.rows + 1);
    ok
}

fn stage_logits(ctx: &mut Ctx, meta: &Meta) -> bool {
    println!("stage logits:");
    let dumps = ctx.dumps.clone();
    let (conds_dump, _) = load_f32(&dumps, "conds_latent");
    let text_tokens = text_tokens_from_dump(&dumps);
    let gpt = ctx.gpt();
    let t0 = Instant::now();
    let (mut session, logits0) = gpt
        .prefill(&conds_dump, &text_tokens, meta.lang_token)
        .unwrap_or_else(|e| die(&format!("prefill: {e:?}")));
    let prefill_dt = t0.elapsed().as_secs_f32();
    // The oracle dumped pre-penalty logits for the norp (penalty 1.0) run and
    // fed the argmax tokens; feed the meta tokens to stay aligned regardless.
    let (want0, _) = load_f32(&dumps, "gpt_logits_step0");
    let mut ok = compare("gpt_logits_step0", &logits0, &want0, 2e-2);
    println!(
        "  (argmax step0: ours {} vs oracle {})",
        argmax(&logits0),
        meta.greedy_norp[0]
    );
    ok &= argmax(&logits0) as u32 == meta.greedy_norp[0];
    let t0 = Instant::now();
    let logits1 = session.step(meta.greedy_norp[0]);
    let (want1, _) = load_f32(&dumps, "gpt_logits_step1");
    ok &= compare("gpt_logits_step1", &logits1, &want1, 2e-2);
    ok &= argmax(&logits1) as u32 == meta.greedy_norp[1];
    let logits2 = session.step(meta.greedy_norp[1]);
    let (want2, _) = load_f32(&dumps, "gpt_logits_step2");
    ok &= compare("gpt_logits_step2", &logits2, &want2, 2e-2);
    ok &= argmax(&logits2) as u32 == meta.greedy_norp[2];
    let step_dt = t0.elapsed().as_secs_f32() / 2.0;
    println!("  (prefill {prefill_dt:.2}s, decode {step_dt:.3}s/token)");
    ok
}

fn stage_greedy(ctx: &mut Ctx, meta: &Meta) -> bool {
    println!("stage greedy:");
    let dumps = ctx.dumps.clone();
    let (conds_dump, _) = load_f32(&dumps, "conds_latent");
    let text_tokens = text_tokens_from_dump(&dumps);
    let gpt = ctx.gpt();

    let t0 = Instant::now();
    let norp = gpt
        .generate(&conds_dump, &text_tokens, meta.lang_token, &GptSamplingConfig::greedy(1.0))
        .unwrap_or_else(|e| die(&format!("generate norp: {e:?}")));
    let dt = t0.elapsed().as_secs_f32();
    let mut ok = compare_tokens("gpt_greedy_codes_norp", &norp, &meta.greedy_norp);
    println!(
        "  (norp: {} tokens in {dt:.1}s = {:.3}s/token)",
        norp.len(),
        dt / norp.len().max(1) as f32
    );

    let t0 = Instant::now();
    let rp10 = gpt
        .generate(&conds_dump, &text_tokens, meta.lang_token, &GptSamplingConfig::greedy(10.0))
        .unwrap_or_else(|e| die(&format!("generate rp10: {e:?}")));
    let dt = t0.elapsed().as_secs_f32();
    ok &= compare_tokens("gpt_greedy_codes_rp10", &rp10, &meta.greedy_rp10);
    println!(
        "  (rp10: {} tokens in {dt:.1}s = {:.3}s/token)",
        rp10.len(),
        dt / rp10.len().max(1) as f32
    );
    // Sanity: the penalty helper reproduces the oracle's per-step transform.
    let mut probe = vec![1.0f32, -1.0, 0.5];
    apply_repetition_penalty(&mut probe, &[true, true, false], 10.0);
    assert_eq!(probe, vec![0.1, -10.0, 0.5]);
    ok
}

/// Generator: teacher-forces the greedy_norp fixture tokens through the f32
/// CPU path and writes every step's pre-penalty logits (row i = logits before
/// token i; the norp run has penalty 1.0, so argmax(row i) == norp\[i\] — this
/// alignment is asserted per step). Output: dumps/gpt_tf_logits_norp.npy.
fn stage_tf_dump(ctx: &mut Ctx, meta: &Meta) -> bool {
    println!("stage tf-dump:");
    let dumps = ctx.dumps.clone();
    let (conds_dump, _) = load_f32(&dumps, "conds_latent");
    let text_tokens = text_tokens_from_dump(&dumps);
    let gpt = ctx.gpt();
    let n = meta.greedy_norp.len();
    let cols = 8194usize;
    let mut rows = Vec::with_capacity(n * cols);

    let t0 = Instant::now();
    let (mut session, mut logits) = gpt
        .prefill(&conds_dump, &text_tokens, meta.lang_token)
        .unwrap_or_else(|e| die(&format!("prefill: {e:?}")));
    for (i, &tok) in meta.greedy_norp.iter().enumerate() {
        assert_eq!(logits.len(), cols, "logits width");
        let am = argmax(&logits) as u32;
        if am != tok {
            println!("  FAIL: argmax(row {i}) = {am} but fixture token is {tok} (misaligned)");
            return false;
        }
        rows.extend_from_slice(&logits);
        if i + 1 < n {
            logits = session.step(tok);
        }
        if i % 16 == 0 {
            eprintln!("  tf {i}/{n} ({:.1}s)", t0.elapsed().as_secs_f32());
        }
    }
    let out = dumps.join("gpt_tf_logits_norp.npy");
    write_npy_f32(&out, n, cols, &rows);
    println!(
        "  wrote {} [{} x {cols}] in {:.1}s (argmax parity held on every row)",
        out.display(),
        n,
        t0.elapsed().as_secs_f32()
    );
    true
}

// ---------------------------------------------------------------------------

fn main() {
    let mut dumps = reference_dumps_dir();
    let mut weights = reference_checkpoints_dir();
    let mut stage = "all".to_string();
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
            other => die(&format!("unknown argument {other}")),
        }
        i += 1;
    }
    if !dumps.join("meta.json").is_file() {
        die(&format!("dumps dir {} has no meta.json", dumps.display()));
    }
    let meta = load_meta(&dumps);
    // Sanity: the f32 text_tokens.npy must cast to the meta.json token list.
    let npy_tokens = text_tokens_from_dump(&dumps);
    if npy_tokens != meta.text_tokens {
        die(&format!(
            "text_tokens.npy {npy_tokens:?} != meta.json {:?}",
            meta.text_tokens
        ));
    }
    let mut ctx = Ctx { dumps, weights, gpt: None };

    let run_emo = stage == "all" || stage == "emo";
    let run_emovec = stage == "all" || stage == "emovec";
    let run_prefill = stage == "all" || stage == "prefill";
    let run_logits = stage == "all" || stage == "logits";
    let run_greedy = stage == "all" || stage == "greedy";
    // Generator stage: opt-in only (writes into the dumps dir).
    let run_tf_dump = stage == "tf-dump";
    if !(run_emo || run_emovec || run_prefill || run_logits || run_greedy || run_tf_dump) {
        die(&format!("unknown stage {stage}"));
    }

    let mut all_ok = true;
    if run_emo {
        all_ok &= stage_emo(&mut ctx);
    }
    if run_emovec {
        all_ok &= stage_emovec(&mut ctx, &meta);
    }
    if run_prefill {
        all_ok &= stage_prefill(&mut ctx, &meta);
    }
    if run_logits {
        all_ok &= stage_logits(&mut ctx, &meta);
    }
    if run_greedy {
        all_ok &= stage_greedy(&mut ctx, &meta);
    }
    if run_tf_dump {
        all_ok &= stage_tf_dump(&mut ctx, &meta);
    }
    if all_ok {
        println!("ALL STAGES PASS");
    } else {
        println!("STAGE FAILURES (see above)");
        std::process::exit(1);
    }
}
