//! IndexTTS-2.5 audio-feature front-end validation against the reference
//! oracle dumps (`local/indextts_ref/dumps`, see dump_oracle.py/dump_oracle2.py).
//!
//! Stages:
//!   fbank    — campplus kaldi fbank:      audio_16k -> campplus_fbank
//!   features — SeamlessM4T extractor:     audio_16k -> w2v_input_features
//!   w2v      — Wav2Vec2Bert encoder:      features -> every w2v_hidden_{i}
//!              dump present + spk_cond_emb (normalized hidden_states[17])
//!   campplus — DTDNN speaker embedder:    campplus_fbank -> campplus_style
//!   w2v-single — diagnostic (not in "all"): each conformer layer run alone
//!              from the ORACLE hidden state, isolating per-layer error from
//!              accumulated f32 noise (needs the dump_w2v_extra.py dumps).
//!
//! Gates per comparison: cosine >= 0.999 and max-abs <= 2e-3, except deep
//! w2v hidden states (>= layer 15) and spk_cond_emb which allow 2.5e-2 —
//! the torch f32 oracle itself sits 8.5e-3 from the f64 ground truth there
//! (see MAX_ABS_GATE_DEEP). Exits nonzero when any gate fails.
//!
//! Usage:
//!   indextts-w2v-validate [--dumps <dir>] [--weights <dir>]
//!                         [--stage all|fbank|features|w2v|campplus|w2v-single]

use makepad_diffusion::indextts::{reference_checkpoints_dir, reference_dumps_dir};
use makepad_diffusion::indextts_campplus::{campplus_fbank, CampPlus};
use makepad_diffusion::indextts_w2v::{extract_w2v_features, W2vBertEncoder};
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

const COS_GATE: f64 = 0.999;
const MAX_ABS_GATE: f32 = 2e-3;
/// Deep w2v hidden states (>= layer 15) and spk_cond_emb: the torch f32
/// oracle is itself 8.5e-3 max-abs from the f64 ground truth at hidden_17
/// (measured by dump_w2v_f64.py; outlier channels reach +-34 and 17 layers of
/// f32 accumulation are chaotic there), so 2e-3 is below the oracle's own
/// noise floor. Per-layer forwards from oracle inputs match to <= 2.3e-5
/// (`--stage w2v-single`); the chain gate allows oracle noise + our noise.
const MAX_ABS_GATE_DEEP: f32 = 2.5e-2;

/// Prints cosine / max-abs against the reference and returns whether the
/// stage gates pass. `row_len` locates the worst element as `[row, col]`.
fn compare_gated(
    tag: &str,
    ours: &[f32],
    reference: &[f32],
    row_len: usize,
    max_abs_gate: f32,
) -> bool {
    assert_eq!(ours.len(), reference.len(), "{tag}: length mismatch");
    let mut dot = 0f64;
    let mut na = 0f64;
    let mut nb = 0f64;
    let mut max_abs = 0f32;
    let mut max_at = 0usize;
    let mut ref_absmax = 0f32;
    for (i, (&a, &b)) in ours.iter().zip(reference).enumerate() {
        dot += a as f64 * b as f64;
        na += a as f64 * a as f64;
        nb += b as f64 * b as f64;
        let d = (a - b).abs();
        if d > max_abs {
            max_abs = d;
            max_at = i;
        }
        ref_absmax = ref_absmax.max(b.abs());
    }
    let cos = dot / (na.sqrt() * nb.sqrt()).max(1e-30);
    let pass = cos >= COS_GATE && max_abs <= max_abs_gate;
    println!(
        "  {tag}: cos {cos:.7} max_abs {max_abs:.3e} at [{}, {}] (ref there {:.4}, ref absmax {ref_absmax:.3}) {}",
        max_at / row_len,
        max_at % row_len,
        reference[max_at],
        if pass { "PASS" } else { "FAIL" }
    );
    pass
}

fn compare(tag: &str, ours: &[f32], reference: &[f32], row_len: usize) -> bool {
    compare_gated(tag, ours, reference, row_len, MAX_ABS_GATE)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |flag: &str| -> Option<String> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let dumps = arg("--dumps").map(PathBuf::from).unwrap_or_else(reference_dumps_dir);
    let weights = arg("--weights")
        .map(PathBuf::from)
        .unwrap_or_else(reference_checkpoints_dir);
    let stage = arg("--stage").unwrap_or_else(|| "all".to_string());
    let run = |name: &str| stage == "all" || stage == name;
    println!("dumps {} weights {}", dumps.display(), weights.display());

    let load = |name: &str| -> Vec<f32> {
        load_npy(&dumps.join(format!("{name}.npy")))
            .and_then(|npy| npy.as_f32())
            .unwrap_or_else(|e| panic!("{e}"))
    };

    let mut all_pass = true;

    if run("fbank") {
        println!("stage fbank (kaldi fbank for campplus)");
        let audio = load("audio_16k");
        let reference = load("campplus_fbank");
        let (ours, frames) = campplus_fbank(&audio).expect("campplus_fbank");
        println!("  {frames} frames");
        all_pass &= compare("campplus_fbank", &ours, &reference, 80);
    }

    if run("features") {
        println!("stage features (SeamlessM4T extractor)");
        let audio = load("audio_16k");
        let reference = load("w2v_input_features");
        let feats = extract_w2v_features(&audio).expect("extract_w2v_features");
        println!(
            "  {} stacked frames, {} valid (attention-mask ones)",
            feats.frames, feats.valid_frames
        );
        all_pass &= compare("w2v_input_features", &feats.data, &reference, 160);
    }

    if run("w2v") {
        println!("stage w2v (feature projection + 17 conformer layers)");
        let audio = load("audio_16k");
        let feats = extract_w2v_features(&audio).expect("extract_w2v_features");
        let t0 = std::time::Instant::now();
        let encoder = W2vBertEncoder::load(&weights).expect("W2vBertEncoder::load");
        println!("  loaded weights in {:.1}s", t0.elapsed().as_secs_f32());
        // Compare against every per-layer dump present (dump_oracle2.py writes
        // 0/1/9/17; dump_w2v_extra.py fills in the rest for bisection).
        let capture: Vec<usize> = (0..=encoder.num_layers())
            .filter(|i| dumps.join(format!("w2v_hidden_{i}.npy")).is_file())
            .collect();
        let t0 = std::time::Instant::now();
        let hidden = encoder.encode_layers(&feats, &capture);
        println!("  forward (17 layers) in {:.1}s", t0.elapsed().as_secs_f32());
        for (i, ours) in capture.iter().zip(&hidden) {
            let reference = load(&format!("w2v_hidden_{i}"));
            let gate = if *i >= 15 { MAX_ABS_GATE_DEEP } else { MAX_ABS_GATE };
            all_pass &= compare_gated(&format!("w2v_hidden_{i}"), ours, &reference, 1024, gate);
            // Informational: distance to the f64 ground truth when the
            // dump_w2v_f64.py probe dumps are present (not gated; shows we
            // sit as close to the truth as the f32 oracle itself does).
            if dumps.join(format!("w2v_hidden_{i}_f64.npy")).is_file() {
                compare_gated(
                    &format!("w2v_hidden_{i} vs f64 truth (info)"),
                    ours,
                    &load(&format!("w2v_hidden_{i}_f64")),
                    1024,
                    f32::INFINITY,
                );
            }
        }
        let normalized = encoder.normalize(hidden.last().expect("hidden_17"));
        let reference = load("spk_cond_emb");
        all_pass &= compare_gated("spk_cond_emb", &normalized, &reference, 1024, MAX_ABS_GATE_DEEP);
    }

    // Diagnostic (not part of "all"): single-layer error with ORACLE inputs —
    // for each consecutive pair of w2v_hidden dumps, run just that layer from
    // the oracle hidden state. Isolates per-layer porting errors from f32
    // noise amplified across the chain by the outlier rows.
    if stage == "w2v-single" {
        println!("stage w2v-single (per-layer forward from oracle hidden states)");
        let audio = load("audio_16k");
        let feats = extract_w2v_features(&audio).expect("extract_w2v_features");
        let encoder = W2vBertEncoder::load(&weights).expect("W2vBertEncoder::load");
        for i in 0..encoder.num_layers() {
            let a = dumps.join(format!("w2v_hidden_{i}.npy"));
            let b = dumps.join(format!("w2v_hidden_{}.npy", i + 1));
            if !a.is_file() || !b.is_file() {
                continue;
            }
            let mut hidden = load(&format!("w2v_hidden_{i}"));
            let reference = load(&format!("w2v_hidden_{}", i + 1));
            encoder.forward_hidden(&mut hidden, feats.frames, feats.valid_frames, i..i + 1);
            compare(&format!("layer {i:2} alone"), &hidden, &reference, 1024);
        }
    }

    if run("campplus") {
        println!("stage campplus (DTDNN speaker embedder)");
        let fbank = load("campplus_fbank");
        let reference = load("campplus_style");
        let t0 = std::time::Instant::now();
        let model = CampPlus::load(&weights).expect("CampPlus::load");
        println!("  loaded weights in {:.2}s", t0.elapsed().as_secs_f32());
        let frames = fbank.len() / 80;
        let t0 = std::time::Instant::now();
        let style = model.embed(&fbank, frames);
        println!("  forward ({frames} frames) in {:.2}s", t0.elapsed().as_secs_f32());
        all_pass &= compare("campplus_style", &style, &reference, 192);
    }

    if all_pass {
        println!("done: all gates passed");
    } else {
        println!("done: GATE FAILURES (cos >= {COS_GATE}, max_abs <= {MAX_ABS_GATE:e})");
        std::process::exit(1);
    }
}
