//! FLUX.2 text-encoder tap-parity validator against the frozen diffusers
//! oracle dumps (local/agent_state/flux2-port-files/ref_dump_flux2.py,
//! `--phase te`).
//!
//! Usage:
//!   flux2-check-te <text_encoder_dir> <dumps_dir> [--tokenizer <dir>] [--strict]
//!
//! Compares every operator boundary the oracle records, in compute order:
//! token ids (optionally re-derived from the tokenizer), rope inv_freq +
//! bf16 tables, embedding rows, the full layer-0 chain, every hidden state
//! the dump kept (1..=30 — drift bisects to one layer in a single run), and
//! the final `(512, 15360)` conditioning.
//!
//! Without `--strict` this is a survey run: all metrics print, only
//! shape/load errors fail. With `--strict` the BF16-exact contract is
//! enforced: max_abs == 0.0 for every pre-attention stage, and the
//! attention-dependent tail must stay within FLUX2_TE_ATTN_TOL (pinned from
//! the first survey; torch SDPA reduction order is the only sanctioned
//! divergence source).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use makepad_diffusion::flux2::{Flux2TextEncoderWeights, Mistral3TextConfig, FLUX2_SYSTEM_MESSAGE};
use makepad_diffusion::flux2_text::{
    flux2_text_encode_tapped, flux2_text_release, round_to_bf16, Flux2TextEncoderPrepared,
};
use makepad_diffusion::flux2_tokenizer::{
    Flux2Tokenizer, Flux2TokenizedPrompt, FLUX2_MAX_SEQUENCE_LENGTH,
};

/// Keep in sync with PROMPT in ref_dump_flux2.py.
const ORACLE_PROMPT: &str = "A photorealistic photo of a red fox standing on a mossy rock in a \
     misty forest at dawn, volumetric light, 85mm lens";

/// Provisional gate for attention-dependent stages under --strict; pin the
/// observed value from the first survey run before acceptance.
const FLUX2_TE_ATTN_TOL: f64 = 0.0;

struct Npy {
    shape: Vec<usize>,
    descr: String,
    data: Vec<u8>,
}

fn load_npy(path: &Path) -> Result<Npy, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() < 10 || &bytes[..6] != b"\x93NUMPY" {
        return Err(format!("{}: not an npy file", path.display()));
    }
    let header_len = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
    let data_start = 10 + header_len;
    let header = String::from_utf8_lossy(&bytes[10..data_start]).to_string();
    if header.contains("'fortran_order': True") {
        return Err(format!("{}: Fortran-order npy is unsupported", path.display()));
    }
    let descr = header
        .split("'descr':")
        .nth(1)
        .and_then(|rest| rest.split('\'').nth(1))
        .ok_or_else(|| format!("{}: npy has no descr", path.display()))?
        .to_string();
    let shape_text = header
        .split("'shape':")
        .nth(1)
        .and_then(|rest| rest.split('(').nth(1))
        .and_then(|rest| rest.split(')').next())
        .ok_or_else(|| format!("{}: npy has no shape", path.display()))?;
    let shape = shape_text
        .split(',')
        .filter_map(|part| part.trim().parse::<usize>().ok())
        .collect();
    Ok(Npy {
        shape,
        descr,
        data: bytes[data_start..].to_vec(),
    })
}

impl Npy {
    fn f32(self) -> Result<Vec<f32>, String> {
        match self.descr.as_str() {
            "<f4" => Ok(self
                .data
                .chunks_exact(4)
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect()),
            other => Err(format!("npy dtype {other} is not f32")),
        }
    }

    fn ids_u32(self) -> Result<Vec<u32>, String> {
        match self.descr.as_str() {
            "<i8" => self
                .data
                .chunks_exact(8)
                .map(|chunk| {
                    let value = i64::from_le_bytes(chunk.try_into().unwrap());
                    u32::try_from(value).map_err(|_| format!("invalid token id {value}"))
                })
                .collect(),
            "<i4" => self
                .data
                .chunks_exact(4)
                .map(|chunk| {
                    let value = i32::from_le_bytes(chunk.try_into().unwrap());
                    u32::try_from(value).map_err(|_| format!("invalid token id {value}"))
                })
                .collect(),
            other => Err(format!("npy dtype {other} is not a supported token id")),
        }
    }

    fn elements(&self) -> usize {
        self.shape.iter().product()
    }
}

/// Loads an f32 dump whose trailing dims must multiply to `elements`
/// (leading batch-1 dims from torch are tolerated).
fn load_f32_elements(path: &Path, elements: usize) -> Result<Vec<f32>, String> {
    let npy = load_npy(path)?;
    if npy.elements() != elements {
        return Err(format!(
            "{}: shape {:?} has {} elements, expected {elements}",
            path.display(),
            npy.shape,
            npy.elements(),
        ));
    }
    npy.f32()
}

#[derive(Clone, Copy)]
struct Metrics {
    max_abs: f64,
    mean_abs: f64,
    max_index: usize,
}

struct Gate {
    name: String,
    metrics: Metrics,
    /// true = pre-attention stage, exactness required under --strict.
    exact: bool,
}

fn compare(name: &str, native: &[f32], oracle: &[f32]) -> Result<Metrics, String> {
    if native.len() != oracle.len() {
        return Err(format!(
            "{name}: native/oracle lengths differ: {}/{}",
            native.len(),
            oracle.len(),
        ));
    }
    let mut max_abs = 0.0f64;
    let mut max_index = 0usize;
    let mut sum_abs = 0.0f64;
    let mut exact = 0usize;
    for (index, (&native_v, &oracle_v)) in native.iter().zip(oracle).enumerate() {
        let native_v = native_v as f64;
        let oracle_v = oracle_v as f64;
        if !native_v.is_finite() {
            return Err(format!("{name}: non-finite native value at {index}"));
        }
        let difference = (native_v - oracle_v).abs();
        sum_abs += difference;
        if difference == 0.0 {
            exact += 1;
        }
        if difference > max_abs {
            max_abs = difference;
            max_index = index;
        }
    }
    let count = native.len().max(1);
    println!(
        "[{name}] max_abs={:.9e} mean_abs={:.9e} exact={}/{} max_i={} native_at_max={:.9e} oracle_at_max={:.9e}",
        max_abs,
        sum_abs / count as f64,
        exact,
        count,
        max_index,
        native[max_index],
        oracle[max_index],
    );
    Ok(Metrics {
        max_abs,
        mean_abs: sum_abs / count as f64,
        max_index,
    })
}

fn run() -> Result<Vec<Gate>, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut positional = Vec::new();
    let mut tokenizer_dir: Option<PathBuf> = None;
    let mut strict = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--tokenizer" => {
                index += 1;
                tokenizer_dir = Some(PathBuf::from(args.get(index).ok_or("--tokenizer needs a dir")?));
            }
            "--strict" => strict = true,
            other => positional.push(other.to_string()),
        }
        index += 1;
    }
    if positional.len() != 2 {
        return Err("usage: flux2-check-te <text_encoder_dir> <dumps_dir> [--tokenizer <dir>] [--strict]".into());
    }
    let te_dir = PathBuf::from(&positional[0]);
    let dumps = PathBuf::from(&positional[1]);
    println!("strict={strict} te_dir={} dumps={}", te_dir.display(), dumps.display());

    let seq = FLUX2_MAX_SEQUENCE_LENGTH;
    let config = Mistral3TextConfig::flux2_dev();
    let hidden = config.hidden_size as usize;
    let heads = config.num_attention_heads as usize;
    let kv_heads = config.num_key_value_heads as usize;
    let head_dim = config.head_dim as usize;
    let half = head_dim / 2;
    let ffn = config.intermediate_size as usize;

    // ── Oracle inputs ────────────────────────────────────────────────────
    let oracle_ids = load_npy(&dumps.join("input_ids.npy"))?.ids_u32()?;
    if oracle_ids.len() != seq {
        return Err(format!("input_ids.npy has {} ids, expected {seq}", oracle_ids.len()));
    }
    let mask = load_npy(&dumps.join("attention_mask.npy"))?.ids_u32()?;
    let real_len = mask.iter().filter(|&&m| m != 0).count();
    let left_padded = mask.first().copied() == Some(0);
    if left_padded {
        return Err("attention_mask starts with 0: oracle is LEFT-padded, port assumes right".into());
    }
    println!("oracle real_len={real_len} (right-padded)");

    let mut gates = Vec::new();

    // ── Tokenizer cross-check (optional) ─────────────────────────────────
    if let Some(dir) = tokenizer_dir {
        let tokenizer = Flux2Tokenizer::load(&dir).map_err(|e| format!("tokenizer: {e}"))?;
        let tokenized = tokenizer.encode_t2i(FLUX2_SYSTEM_MESSAGE, ORACLE_PROMPT);
        if tokenized.token_ids == oracle_ids && tokenized.real_len == real_len {
            println!("[tokenizer] exact=true ids={seq} real_len={real_len}");
        } else {
            let first = tokenized
                .token_ids
                .iter()
                .zip(&oracle_ids)
                .position(|(a, b)| a != b);
            return Err(format!(
                "tokenizer mismatch: native real_len={} oracle real_len={real_len} first_id_mismatch={first:?}",
                tokenized.real_len,
            ));
        }
    }

    // ── Native tapped encode ─────────────────────────────────────────────
    let weights = Flux2TextEncoderWeights::load(&te_dir).map_err(|e| format!("weights: {e}"))?;
    let prepared = Flux2TextEncoderPrepared::prepare(&weights, config.clone())
        .map_err(|e| format!("prepare: {e}"))?;

    if let Ok(oracle_inv) = load_f32_elements(&dumps.join("rope_inv_freq.npy"), half) {
        gates.push(Gate {
            name: "rope_inv_freq".into(),
            metrics: compare("rope_inv_freq", &prepared.rope_inv_freq, &oracle_inv)?,
            exact: true,
        });
    } else {
        println!("[rope_inv_freq] dump missing, skipped");
    }

    let started = std::time::Instant::now();
    let prompt = Flux2TokenizedPrompt {
        token_ids: oracle_ids.clone(),
        real_len,
    };
    let (conditioning, taps) = flux2_text_encode_tapped(&weights, &prepared, &prompt)
        .map_err(|e| format!("encode: {e}"))?;
    println!("native tapped encode: {:.2}s", started.elapsed().as_secs_f64());

    // ── Embedding + rope tables ──────────────────────────────────────────
    let oracle_embed = load_f32_elements(&dumps.join("te_hidden_00.npy"), seq * hidden)?;
    gates.push(Gate {
        name: "embed".into(),
        metrics: compare("embed", &taps.embed, &oracle_embed)?,
        exact: true,
    });

    // HF rotary emits full-width tables `cat(freqs, freqs)`; ours are the
    // rotate-half width. Verify the duplication, compare against the left half.
    for (name, native) in [("rope_cos", &taps.rope_cos), ("rope_sin", &taps.rope_sin)] {
        let path = dumps.join(format!("{name}.npy"));
        let oracle = match load_f32_elements(&path, seq * head_dim) {
            Ok(values) => values,
            Err(error) => {
                println!("[{name}] {error}, skipped");
                continue;
            }
        };
        let mut left = Vec::with_capacity(seq * half);
        for row in 0..seq {
            let full = &oracle[row * head_dim..(row + 1) * head_dim];
            if full[..half] != full[half..] {
                return Err(format!("{name}: oracle halves differ at row {row}"));
            }
            left.extend_from_slice(&full[..half]);
        }
        gates.push(Gate {
            name: name.into(),
            metrics: compare(name, native, &left)?,
            exact: true,
        });
    }

    // ── Layer-0 operator chain ───────────────────────────────────────────
    let layer0 = &taps.layer0;
    let pre_attention: [(&str, &Vec<f32>, usize); 4] = [
        ("l0_input_norm", &layer0.input_norm, seq * hidden),
        ("l0_q_proj", &layer0.query_projected, seq * heads * head_dim),
        ("l0_k_proj", &layer0.key_projected, seq * kv_heads * head_dim),
        ("l0_v_proj", &layer0.value_projected, seq * kv_heads * head_dim),
    ];
    for (name, native, elements) in pre_attention {
        let oracle = load_f32_elements(&dumps.join(format!("{name}.npy")), elements)?;
        gates.push(Gate {
            name: name.into(),
            metrics: compare(name, native, &oracle)?,
            exact: true,
        });
    }

    let oracle_attn_raw =
        load_f32_elements(&dumps.join("l0_attn_raw.npy"), seq * heads * head_dim)?;
    gates.push(Gate {
        name: "l0_attn_raw".into(),
        metrics: compare("l0_attn_raw", &layer0.attention_raw, &oracle_attn_raw)?,
        exact: false,
    });
    let oracle_attn_proj = load_f32_elements(&dumps.join("l0_attn_proj.npy"), seq * hidden)?;
    gates.push(Gate {
        name: "l0_attn_proj".into(),
        metrics: compare("l0_attn_proj", &layer0.attention_projected, &oracle_attn_proj)?,
        exact: false,
    });

    // Residual has no oracle hook; derive it the way torch does (f32 add of
    // bf16 fixed points, rounded back) from dumps the hooks did capture.
    let derived_residual: Vec<f32> = oracle_embed
        .iter()
        .zip(&oracle_attn_proj)
        .map(|(&h, &a)| round_to_bf16(h + a))
        .collect();
    gates.push(Gate {
        name: "l0_attn_residual(derived)".into(),
        metrics: compare(
            "l0_attn_residual(derived)",
            &layer0.attention_residual,
            &derived_residual,
        )?,
        exact: false,
    });

    let oracle_post_norm = load_f32_elements(&dumps.join("l0_post_attn_norm.npy"), seq * hidden)?;
    gates.push(Gate {
        name: "l0_post_attn_norm".into(),
        metrics: compare("l0_post_attn_norm", &layer0.post_attention_norm, &oracle_post_norm)?,
        exact: false,
    });

    // Native fused [up | gate] vs the separate oracle projections.
    let oracle_up = load_f32_elements(&dumps.join("l0_up_proj.npy"), seq * ffn)?;
    let oracle_gate = load_f32_elements(&dumps.join("l0_gate_proj.npy"), seq * ffn)?;
    let mut oracle_up_gate = Vec::with_capacity(seq * 2 * ffn);
    for row in 0..seq {
        oracle_up_gate.extend_from_slice(&oracle_up[row * ffn..(row + 1) * ffn]);
        oracle_up_gate.extend_from_slice(&oracle_gate[row * ffn..(row + 1) * ffn]);
    }
    gates.push(Gate {
        name: "l0_up_gate".into(),
        metrics: compare("l0_up_gate", &layer0.up_gate, &oracle_up_gate)?,
        exact: false,
    });

    let oracle_activated = load_f32_elements(&dumps.join("l0_activated.npy"), seq * ffn)?;
    gates.push(Gate {
        name: "l0_activated".into(),
        metrics: compare("l0_activated", &layer0.activated, &oracle_activated)?,
        exact: false,
    });
    let oracle_down = load_f32_elements(&dumps.join("l0_down_proj.npy"), seq * hidden)?;
    gates.push(Gate {
        name: "l0_down_proj".into(),
        metrics: compare("l0_down_proj", &layer0.down_projected, &oracle_down)?,
        exact: false,
    });
    let oracle_h1 = load_f32_elements(&dumps.join("te_hidden_01.npy"), seq * hidden)?;
    gates.push(Gate {
        name: "l0_hidden".into(),
        metrics: compare("l0_hidden", &layer0.hidden, &oracle_h1)?,
        exact: false,
    });

    // ── Per-layer hidden states (bisect) + conditioning ──────────────────
    for (index, native) in &taps.hidden_states {
        let path = dumps.join(format!("te_hidden_{index:02}.npy"));
        let oracle = match load_f32_elements(&path, seq * hidden) {
            Ok(values) => values,
            Err(_) => continue, // dump set may keep fewer layers
        };
        let name = format!("te_hidden_{index:02}");
        gates.push(Gate {
            name: name.clone(),
            metrics: compare(&name, native, &oracle)?,
            exact: false,
        });
    }

    let oracle_embeds = load_f32_elements(
        &dumps.join("prompt_embeds.npy"),
        seq * config.conditioning_dim() as usize,
    )?;
    gates.push(Gate {
        name: "conditioning".into(),
        metrics: compare("conditioning", &conditioning, &oracle_embeds)?,
        exact: false,
    });

    let evicted = flux2_text_release().map_err(|e| format!("release: {e}"))?;
    println!("released {evicted} cached weights");

    if strict {
        let mut failures = Vec::new();
        for gate in &gates {
            let limit = if gate.exact { 0.0 } else { FLUX2_TE_ATTN_TOL };
            if gate.metrics.max_abs > limit {
                failures.push(format!(
                    "{}: max_abs {:.9e} > {:.9e} (mean {:.9e}, max_i {})",
                    gate.name, gate.metrics.max_abs, limit, gate.metrics.mean_abs, gate.metrics.max_index,
                ));
            }
        }
        if !failures.is_empty() {
            return Err(format!("STRICT gate failures:\n  {}", failures.join("\n  ")));
        }
        println!("STRICT: all {} gates passed", gates.len());
    }
    Ok(gates)
}

fn main() -> ExitCode {
    match run() {
        Ok(gates) => {
            println!("flux2-check-te: {} stages compared", gates.len());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("flux2-check-te: {error}");
            ExitCode::FAILURE
        }
    }
}
