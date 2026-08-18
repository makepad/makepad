//! Official-oracle parity and warm benchmark for the native TokenRig Qwen.
//!
//! Usage:
//! `skin-tokens-qwen-validate <tokenrig.safetensors> <oracle-dir> [warm-runs]`

use makepad_diffusion::skin_tokens::{
    SkinTokensWeights, SKIN_TOKENS_QWEN_FFN, SKIN_TOKENS_QWEN_HEADS,
    SKIN_TOKENS_QWEN_HEAD_DIM, SKIN_TOKENS_QWEN_KV_HEADS, SKIN_TOKENS_QWEN_LAYERS,
    SKIN_TOKENS_QWEN_WIDTH, SKIN_TOKENS_VOCAB,
};
use makepad_diffusion::skin_tokens_qwen::{
    skin_tokens_qwen_decode_beams, skin_tokens_qwen_decode_beams_tapped,
    skin_tokens_qwen_decode_step, skin_tokens_qwen_decode_step_tapped, skin_tokens_qwen_generate,
    skin_tokens_qwen_generate_traced, skin_tokens_qwen_prefill,
    skin_tokens_qwen_prefill_tapped, skin_tokens_qwen_projection_tap,
    SkinTokensBeamSelectionTrace, SkinTokensGenerationGrammar, SkinTokensGenerationParams,
    SkinTokensQwenBeamDecodeTap, SkinTokensQwenPrepared,
};
use makepad_diffusion::skin_tokens_tokenizer::{
    SkinTokensGenerationPhase, SkinTokensGrammar, SKIN_TOKENS_TOKEN_BOS,
    SKIN_TOKENS_TOKEN_CLASS_ARTICULATION_XL,
};
use makepad_diffusion::backend::{
    gpu_download, gpu_linear_nt_cached_bf16_mm, gpu_slice_rows, gpu_upload,
    gpu_weight_cache_ensure, GpuLinearPart,
};
use makepad_ggml::quant::GGML_TYPE_BF16;
use std::path::Path;
use std::time::Instant;

struct Npy {
    shape: Vec<usize>,
    descr: String,
    data: Vec<u8>,
}

fn load_npy(path: &Path) -> Result<Npy, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() < 12 || &bytes[..6] != b"\x93NUMPY" {
        return Err(format!("{}: not an npy file", path.display()));
    }
    let (header_len, header_start) = match bytes[6] {
        1 => (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10usize),
        2 | 3 => (
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
            12usize,
        ),
        version => {
            return Err(format!(
                "{}: unsupported npy version {version}",
                path.display()
            ))
        }
    };
    let data_start = header_start
        .checked_add(header_len)
        .filter(|&end| end <= bytes.len())
        .ok_or_else(|| format!("{}: truncated npy header", path.display()))?;
    let header = String::from_utf8_lossy(&bytes[header_start..data_start]);
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
            "<u4" => Ok(self
                .data
                .chunks_exact(4)
                .map(|chunk| u32::from_le_bytes(chunk.try_into().unwrap()))
                .collect()),
            other => Err(format!("npy dtype {other} is not a supported token id")),
        }
    }
}

fn load_f32(path: &Path, expected: &[usize]) -> Result<Vec<f32>, String> {
    let npy = load_npy(path)?;
    if npy.shape != expected {
        return Err(format!(
            "{}: shape {:?}, expected {expected:?}",
            path.display(),
            npy.shape,
        ));
    }
    npy.f32()
}

fn load_ids_u32(path: &Path, expected: &[usize]) -> Result<Vec<u32>, String> {
    let npy = load_npy(path)?;
    if npy.shape != expected {
        return Err(format!(
            "{}: shape {:?}, expected {expected:?}",
            path.display(),
            npy.shape,
        ));
    }
    npy.ids_u32()
}

#[derive(Clone, Copy)]
struct Metrics {
    cosine: f64,
    max_abs: f64,
    mean_abs: f64,
    rms: f64,
    max_index: usize,
}

fn compare(name: &str, native: &[f32], oracle: &[f32]) -> Result<Metrics, String> {
    if native.len() != oracle.len() {
        return Err(format!(
            "{name}: native/oracle lengths differ: {}/{}",
            native.len(),
            oracle.len(),
        ));
    }
    let mut dot = 0.0f64;
    let mut norm_native = 0.0f64;
    let mut norm_oracle = 0.0f64;
    let mut max_abs = 0.0f64;
    let mut max_index = 0usize;
    let mut sum_abs = 0.0f64;
    let mut sum_sq = 0.0f64;
    for (index, (&native, &oracle)) in native.iter().zip(oracle).enumerate() {
        let native = native as f64;
        let oracle = oracle as f64;
        if !native.is_finite() {
            return Err(format!("{name}: non-finite native value at {index}"));
        }
        dot += native * oracle;
        norm_native += native * native;
        norm_oracle += oracle * oracle;
        let difference = (native - oracle).abs();
        sum_abs += difference;
        sum_sq += difference * difference;
        if difference > max_abs {
            max_abs = difference;
            max_index = index;
        }
    }
    let count = native.len().max(1) as f64;
    let metrics = Metrics {
        cosine: dot / (norm_native.sqrt() * norm_oracle.sqrt()).max(f64::MIN_POSITIVE),
        max_abs,
        mean_abs: sum_abs / count,
        rms: (sum_sq / count).sqrt(),
        max_index,
    };
    println!(
        "[{name}] cosine={:.10} max_abs={:.9e} mean_abs={:.9e} rms={:.9e} max_i={} native_at_max={:.9e} oracle_at_max={:.9e}",
        metrics.cosine,
        metrics.max_abs,
        metrics.mean_abs,
        metrics.rms,
        metrics.max_index,
        native[metrics.max_index],
        oracle[metrics.max_index],
    );
    Ok(metrics)
}

fn compare_ids(name: &str, native: &[u32], oracle: &[u32]) -> Result<(), String> {
    if native == oracle {
        println!("[{name}] exact=true ids={}", native.len());
        return Ok(());
    }
    let mismatch = native
        .iter()
        .zip(oracle)
        .position(|(native, oracle)| native != oracle)
        .unwrap_or(native.len().min(oracle.len()));
    Err(format!(
        "{name}: exact=false native_len={} oracle_len={} first_mismatch={} native={:?} oracle={:?}",
        native.len(),
        oracle.len(),
        mismatch,
        native.get(mismatch),
        oracle.get(mismatch),
    ))
}

fn compare_generation_trace(
    native: &[SkinTokensBeamSelectionTrace],
    oracle: &Path,
    num_beams: usize,
) -> Result<(), String> {
    let sampled_scores_npy = load_npy(&oracle.join("beam_sampled_scores.npy"))?;
    if sampled_scores_npy.shape.len() != 2 || sampled_scores_npy.shape[1] != num_beams * 2 {
        return Err(format!(
            "{}: shape {:?}, expected [steps, {}]",
            oracle.join("beam_sampled_scores.npy").display(),
            sampled_scores_npy.shape,
            num_beams * 2,
        ));
    }
    let oracle_steps = sampled_scores_npy.shape[0];
    let sampled_width = sampled_scores_npy.shape[1];
    let oracle_sampled_scores = sampled_scores_npy.f32()?;
    let oracle_sampled_ids = load_ids_u32(
        &oracle.join("beam_sampled_flat_indices.npy"),
        &[oracle_steps, sampled_width],
    )?;
    let oracle_running_parents = load_ids_u32(
        &oracle.join("beam_running_parent_ids.npy"),
        &[oracle_steps, num_beams],
    )?;
    let oracle_running_tokens = load_ids_u32(
        &oracle.join("beam_running_token_ids.npy"),
        &[oracle_steps, num_beams],
    )?;

    let mut sampled_failure = None;
    let mut running_failure = None;
    let mut aligned_native_scores = Vec::new();
    let mut aligned_oracle_scores = Vec::new();
    let shared_steps = native.len().min(oracle_steps);
    for (step, native_step) in native.iter().take(shared_steps).enumerate() {
        let sampled_start = step * sampled_width;
        let sampled_end = sampled_start + sampled_width;
        let oracle_score_row = &oracle_sampled_scores[sampled_start..sampled_end];
        let oracle_id_row = &oracle_sampled_ids[sampled_start..sampled_end];
        let oracle_finite_ids: Vec<u32> = oracle_id_row
            .iter()
            .zip(oracle_score_row)
            .filter_map(|(&id, &score)| score.is_finite().then_some(id))
            .collect();
        let oracle_finite_scores: Vec<f32> = oracle_score_row
            .iter()
            .copied()
            .filter(|score| score.is_finite())
            .collect();

        if sampled_failure.is_none() {
            if native_step.sampled_flat_indices != oracle_finite_ids {
                let position = native_step
                    .sampled_flat_indices
                    .iter()
                    .zip(&oracle_finite_ids)
                    .position(|(native, oracle)| native != oracle)
                    .unwrap_or(
                        native_step
                            .sampled_flat_indices
                            .len()
                            .min(oracle_finite_ids.len()),
                    );
                sampled_failure = Some(format!(
                    "beam sampled IDs diverge at step={step} position={position}: native_len={} oracle_finite_len={} native={:?} oracle={:?} native_score={:?} oracle_score={:?} native_row={:?} oracle_row={:?}",
                    native_step.sampled_flat_indices.len(),
                    oracle_finite_ids.len(),
                    native_step.sampled_flat_indices.get(position),
                    oracle_finite_ids.get(position),
                    native_step.sampled_scores.get(position),
                    oracle_finite_scores.get(position),
                    native_step.sampled_flat_indices,
                    oracle_finite_ids,
                ));
            } else if native_step.sampled_scores.len() != oracle_finite_scores.len() {
                sampled_failure = Some(format!(
                    "beam sampled scores differ in length at step={step}: native={} oracle_finite={}",
                    native_step.sampled_scores.len(),
                    oracle_finite_scores.len(),
                ));
            } else {
                aligned_native_scores.extend_from_slice(&native_step.sampled_scores);
                aligned_oracle_scores.extend_from_slice(&oracle_finite_scores);
            }
        }

        if running_failure.is_none() {
            let running_start = step * num_beams;
            let running_end = running_start + num_beams;
            let oracle_parent_row = &oracle_running_parents[running_start..running_end];
            let oracle_token_row = &oracle_running_tokens[running_start..running_end];
            let native_len = native_step.running_parent_ids.len();
            let meaningful_len = native_step.sampled_scores.len().min(num_beams);
            if native_step.running_token_ids.len() != native_len
                || native_len > num_beams
                || meaningful_len > native_len
            {
                running_failure = Some(format!(
                    "native running trace has invalid shape at step={step}: parents={} tokens={} meaningful={meaningful_len} beams={num_beams}",
                    native_len,
                    native_step.running_token_ids.len(),
                ));
            } else {
                let parent_mismatch = native_step
                    .running_parent_ids
                    .iter()
                    .take(meaningful_len)
                    .zip(oracle_parent_row)
                    .position(|(native, oracle)| native != oracle);
                let token_mismatch = native_step
                    .running_token_ids
                    .iter()
                    .take(meaningful_len)
                    .zip(oracle_token_row)
                    .position(|(native, oracle)| native != oracle);
                if let Some(position) = parent_mismatch.or(token_mismatch) {
                    running_failure = Some(format!(
                        "beam running selection diverges at step={step} position={position}: native_parent={:?} oracle_parent={:?} native_token={:?} oracle_token={:?} native_parent_row={:?} oracle_parent_row={:?} native_token_row={:?} oracle_token_row={:?}",
                        native_step.running_parent_ids.get(position),
                        oracle_parent_row.get(position),
                        native_step.running_token_ids.get(position),
                        oracle_token_row.get(position),
                        native_step.running_parent_ids,
                        oracle_parent_row,
                        native_step.running_token_ids,
                        oracle_token_row,
                    ));
                }
            }
        }
    }

    if !aligned_native_scores.is_empty() {
        compare(
            "generation_trace_aligned_sampled_scores",
            &aligned_native_scores,
            &aligned_oracle_scores,
        )?;
    }
    if sampled_failure.is_none() && native.len() != oracle_steps {
        sampled_failure = Some(format!(
            "beam trace lengths differ: native_steps={} oracle_steps={oracle_steps}",
            native.len(),
        ));
    }
    match (&sampled_failure, &running_failure) {
        (None, None) => {
            println!(
                "[generation_trace] exact=true steps={oracle_steps} sampled_ids_and_running_beams=true"
            );
            Ok(())
        }
        _ => {
            if let Some(error) = &sampled_failure {
                println!("[generation_trace] SAMPLED_MISMATCH {error}");
            }
            if let Some(error) = &running_failure {
                println!("[generation_trace] RUNNING_MISMATCH {error}");
            }
            Err([sampled_failure, running_failure]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("; "))
        }
    }
}

fn validate_qwen_mm(weights: &SkinTokensWeights, oracle: &Path) -> Result<(), String> {
    if !oracle.join("prefill_layer0_input_norm.npy").is_file() {
        println!("[qwen_mm] SKIP no detailed official operator taps");
        return Ok(());
    }
    const NAMESPACE: &str = "skin-tokens-qwen-mm-validate";
    let mut failures = Vec::new();
    for (stage, rows) in [("prefill", 514usize), ("first_decode", 10usize)] {
        for (name, input_suffix, output_suffix, weight_name, input_cols, output_cols) in [
            (
                "q_proj",
                "layer0_input_norm",
                "layer0_q_proj",
                "transformer.model.layers.0.self_attn.q_proj.weight",
                SKIN_TOKENS_QWEN_WIDTH,
                2_048usize,
            ),
            (
                "k_proj",
                "layer0_input_norm",
                "layer0_k_proj",
                "transformer.model.layers.0.self_attn.k_proj.weight",
                SKIN_TOKENS_QWEN_WIDTH,
                1_024usize,
            ),
            (
                "v_proj",
                "layer0_input_norm",
                "layer0_v_proj",
                "transformer.model.layers.0.self_attn.v_proj.weight",
                SKIN_TOKENS_QWEN_WIDTH,
                1_024usize,
            ),
            (
                "o_proj",
                "layer0_attention_raw",
                "layer0_o_proj",
                "transformer.model.layers.0.self_attn.o_proj.weight",
                2_048usize,
                SKIN_TOKENS_QWEN_WIDTH,
            ),
            (
                "gate_proj",
                "layer0_post_attention_norm",
                "layer0_gate_proj",
                "transformer.model.layers.0.mlp.gate_proj.weight",
                SKIN_TOKENS_QWEN_WIDTH,
                3_072usize,
            ),
            (
                "up_proj",
                "layer0_post_attention_norm",
                "layer0_up_proj",
                "transformer.model.layers.0.mlp.up_proj.weight",
                SKIN_TOKENS_QWEN_WIDTH,
                3_072usize,
            ),
            (
                "down_proj",
                "layer0_mlp_activated",
                "layer0_down_proj",
                "transformer.model.layers.0.mlp.down_proj.weight",
                3_072usize,
                SKIN_TOKENS_QWEN_WIDTH,
            ),
        ] {
            let input_name = format!("{stage}_{input_suffix}.npy");
            let output_name = format!("{stage}_{output_suffix}.npy");
            let input_shape = if stage == "prefill" {
                vec![1, rows, input_cols]
            } else {
                vec![rows, 1, input_cols]
            };
            let input = load_f32(&oracle.join(input_name), &input_shape)?;
            gpu_weight_cache_ensure(
                NAMESPACE,
                weight_name,
                GGML_TYPE_BF16,
                output_cols,
                input_cols,
                false,
                || weights.tensor_bytes(weight_name).map_err(|error| error.to_string()),
            )?;
            let part = GpuLinearPart {
                bt_ggml_type: GGML_TYPE_BF16,
                n: output_cols,
                cache_key: weight_name,
                bytes: &[],
            };
            let input = gpu_upload(&input, rows, input_cols)?;
            let native = gpu_download(&gpu_linear_nt_cached_bf16_mm(
                &input,
                NAMESPACE,
                std::slice::from_ref(&part),
            )?)?;
            let expected = if stage == "prefill" {
                vec![1, rows, output_cols]
            } else {
                vec![rows, 1, output_cols]
            };
            let reference = load_f32(&oracle.join(output_name), &expected)?;
            let metrics = compare(&format!("qwen_mm_{stage}_{name}"), &native, &reference)?;
            if metrics.max_abs != 0.0 {
                failures.push(format!(
                    "qwen_mm_{stage}_{name}: max_abs={:.9e}",
                    metrics.max_abs,
                ));
            }
        }

        let final_norm_shape = if stage == "prefill" {
            vec![1, rows, SKIN_TOKENS_QWEN_WIDTH]
        } else {
            vec![rows, 1, SKIN_TOKENS_QWEN_WIDTH]
        };
        let input = load_f32(
            &oracle.join(format!("{stage}_final_norm.npy")),
            &final_norm_shape,
        )?;
        let weight_name = "transformer.lm_head.weight";
        gpu_weight_cache_ensure(
            NAMESPACE,
            weight_name,
            GGML_TYPE_BF16,
            SKIN_TOKENS_VOCAB,
            SKIN_TOKENS_QWEN_WIDTH,
            false,
            || weights.tensor_bytes(weight_name).map_err(|error| error.to_string()),
        )?;
        let part = GpuLinearPart {
            bt_ggml_type: GGML_TYPE_BF16,
            n: SKIN_TOKENS_VOCAB,
            cache_key: weight_name,
            bytes: &[],
        };
        let input = gpu_upload(&input, rows, SKIN_TOKENS_QWEN_WIDTH)?;
        let native = gpu_linear_nt_cached_bf16_mm(
            &input,
            NAMESPACE,
            std::slice::from_ref(&part),
        )?;
        let (native, reference) = if stage == "prefill" {
            (
                gpu_download(&gpu_slice_rows(&native, rows - 1, 1)?)?,
                load_f32(
                    &oracle.join("prefill_logits_last.npy"),
                    &[1, SKIN_TOKENS_VOCAB],
                )?,
            )
        } else {
            (
                gpu_download(&native)?,
                load_f32(
                    &oracle.join("first_decode_logits.npy"),
                    &[rows, 1, SKIN_TOKENS_VOCAB],
                )?,
            )
        };
        let metrics = compare(&format!("qwen_mm_{stage}_lm_head"), &native, &reference)?;
        if metrics.max_abs != 0.0 {
            failures.push(format!(
                "qwen_mm_{stage}_lm_head: max_abs={:.9e}",
                metrics.max_abs,
            ));
        }
    }
    if failures.is_empty() {
        println!("[qwen_mm] PASS bit-exact Torch 2.7 bias-free BF16 linears");
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn compare_first_decode_operator_tap(
    tap: &SkinTokensQwenBeamDecodeTap,
    oracle: &Path,
    beams: usize,
) -> Result<(), String> {
    let q_width = SKIN_TOKENS_QWEN_HEADS * SKIN_TOKENS_QWEN_HEAD_DIM;
    let kv_width = SKIN_TOKENS_QWEN_KV_HEADS * SKIN_TOKENS_QWEN_HEAD_DIM;
    let reference = load_f32(
        &oracle.join("first_decode_hidden.npy"),
        &[beams, 1, SKIN_TOKENS_QWEN_WIDTH],
    )?;
    compare("first_decode_input_hidden", &tap.input_hidden, &reference)?;
    for (name, native) in [
        ("cos", tap.rope_cos.as_slice()),
        ("sin", tap.rope_sin.as_slice()),
    ] {
        let reference = load_f32(
            &oracle.join(format!("first_decode_{name}.npy")),
            &[beams, 1, SKIN_TOKENS_QWEN_HEAD_DIM],
        )?;
        let mut unique = Vec::with_capacity(beams * SKIN_TOKENS_QWEN_HEAD_DIM / 2);
        for row in reference.chunks_exact(SKIN_TOKENS_QWEN_HEAD_DIM) {
            unique.extend_from_slice(&row[..SKIN_TOKENS_QWEN_HEAD_DIM / 2]);
        }
        compare(&format!("first_decode_rope_{name}"), native, &unique)?;
    }
    for (name, native, shape) in [
        (
            "layer0_input_norm",
            tap.layer0.input_norm.as_slice(),
            vec![beams, 1, SKIN_TOKENS_QWEN_WIDTH],
        ),
        (
            "layer0_q_proj",
            tap.layer0.query_projected.as_slice(),
            vec![beams, 1, q_width],
        ),
        (
            "layer0_k_proj",
            tap.layer0.key_projected.as_slice(),
            vec![beams, 1, kv_width],
        ),
        (
            "layer0_v_proj",
            tap.layer0.value_projected.as_slice(),
            vec![beams, 1, kv_width],
        ),
        (
            "layer0_q_norm",
            tap.layer0.query_normalized.as_slice(),
            vec![beams, 1, SKIN_TOKENS_QWEN_HEADS, SKIN_TOKENS_QWEN_HEAD_DIM],
        ),
        (
            "layer0_k_norm",
            tap.layer0.key_normalized.as_slice(),
            vec![
                beams,
                1,
                SKIN_TOKENS_QWEN_KV_HEADS,
                SKIN_TOKENS_QWEN_HEAD_DIM,
            ],
        ),
        (
            "layer0_attention_raw",
            tap.layer0.attention_raw.as_slice(),
            vec![beams, 1, q_width],
        ),
        (
            "layer0_o_proj",
            tap.layer0.attention_projected.as_slice(),
            vec![beams, 1, SKIN_TOKENS_QWEN_WIDTH],
        ),
        (
            "layer0_attention_residual",
            tap.layer0.attention_residual.as_slice(),
            vec![beams, 1, SKIN_TOKENS_QWEN_WIDTH],
        ),
        (
            "layer0_post_attention_norm",
            tap.layer0.post_attention_norm.as_slice(),
            vec![beams, 1, SKIN_TOKENS_QWEN_WIDTH],
        ),
        (
            "layer0_gate_proj",
            tap.layer0.gate_projected.as_slice(),
            vec![beams, 1, SKIN_TOKENS_QWEN_FFN],
        ),
        (
            "layer0_up_proj",
            tap.layer0.up_projected.as_slice(),
            vec![beams, 1, SKIN_TOKENS_QWEN_FFN],
        ),
        (
            "layer0_mlp_activated",
            tap.layer0.mlp_activated.as_slice(),
            vec![beams, 1, SKIN_TOKENS_QWEN_FFN],
        ),
        (
            "layer0_down_proj",
            tap.layer0.down_projected.as_slice(),
            vec![beams, 1, SKIN_TOKENS_QWEN_WIDTH],
        ),
        (
            "layer0_hidden",
            tap.layer0.hidden.as_slice(),
            vec![beams, 1, SKIN_TOKENS_QWEN_WIDTH],
        ),
    ] {
        let reference = load_f32(&oracle.join(format!("first_decode_{name}.npy")), &shape)?;
        compare(&format!("first_decode_{name}"), native, &reference)?;
    }
    if tap.layer_hidden.len() != SKIN_TOKENS_QWEN_LAYERS {
        return Err(format!(
            "first decode tap returned {} layer rows, expected {SKIN_TOKENS_QWEN_LAYERS}",
            tap.layer_hidden.len()
        ));
    }
    for (layer, native) in tap.layer_hidden.iter().enumerate() {
        let reference = load_f32(
            &oracle.join(format!("first_decode_layer{layer}_hidden.npy")),
            &[beams, 1, SKIN_TOKENS_QWEN_WIDTH],
        )?;
        compare(
            &format!("first_decode_layer{layer}_hidden"),
            native,
            &reference,
        )?;
    }
    let reference = load_f32(
        &oracle.join("first_decode_final_norm.npy"),
        &[beams, 1, SKIN_TOKENS_QWEN_WIDTH],
    )?;
    compare("first_decode_final_norm", &tap.final_norm, &reference)?;
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GenerationMode {
    None,
    Compatibility,
    Strict,
    Both,
}

impl GenerationMode {
    fn parse(value: Option<String>) -> Result<Self, String> {
        match value.as_deref() {
            None | Some("none") => Ok(Self::None),
            Some("compatibility") | Some("compat") => Ok(Self::Compatibility),
            Some("strict") => Ok(Self::Strict),
            Some("both") => Ok(Self::Both),
            Some(other) => Err(format!(
                "invalid generation mode '{other}', expected none|compatibility|strict|both"
            )),
        }
    }

    fn compatibility(self) -> bool {
        matches!(self, Self::Compatibility | Self::Both)
    }

    fn strict(self) -> bool {
        matches!(self, Self::Strict | Self::Both)
    }
}

fn run_generation_gate(
    weights: &SkinTokensWeights,
    prepared: &SkinTokensQwenPrepared,
    oracle: &Path,
    mode: GenerationMode,
) -> Result<(), String> {
    validate_qwen_mm(weights, oracle)?;
    let inputs = load_f32(
        &oracle.join("inputs_embeds.npy"),
        &[1, 514, SKIN_TOKENS_QWEN_WIDTH],
    )?;
    if oracle.join("prefill_layer0_input_norm.npy").is_file() {
        let tap = skin_tokens_qwen_projection_tap(weights, prepared, &inputs)
            .map_err(|error| error.to_string())?;
        for (name, native, shape) in [
            (
                "layer0_input_norm",
                tap.input_norm.as_slice(),
                vec![1, 514, SKIN_TOKENS_QWEN_WIDTH],
            ),
            (
                "layer0_q_proj",
                tap.query_projected.as_slice(),
                vec![
                    1,
                    514,
                    SKIN_TOKENS_QWEN_HEAD_DIM
                        * makepad_diffusion::skin_tokens::SKIN_TOKENS_QWEN_HEADS,
                ],
            ),
            (
                "layer0_k_proj",
                tap.key_projected.as_slice(),
                vec![
                    1,
                    514,
                    SKIN_TOKENS_QWEN_HEAD_DIM * SKIN_TOKENS_QWEN_KV_HEADS,
                ],
            ),
            (
                "layer0_v_proj",
                tap.value_projected.as_slice(),
                vec![
                    1,
                    514,
                    SKIN_TOKENS_QWEN_HEAD_DIM * SKIN_TOKENS_QWEN_KV_HEADS,
                ],
            ),
            (
                "layer0_q_norm",
                tap.query_normalized.as_slice(),
                vec![
                    1,
                    514,
                    makepad_diffusion::skin_tokens::SKIN_TOKENS_QWEN_HEADS,
                    SKIN_TOKENS_QWEN_HEAD_DIM,
                ],
            ),
            (
                "layer0_k_norm",
                tap.key_normalized.as_slice(),
                vec![
                    1,
                    514,
                    SKIN_TOKENS_QWEN_KV_HEADS,
                    SKIN_TOKENS_QWEN_HEAD_DIM,
                ],
            ),
        ] {
            let reference = load_f32(&oracle.join(format!("prefill_{name}.npy")), &shape)?;
            compare(&format!("generation_prefill_{name}"), native, &reference)?;
        }

        let (prefill, layer_taps) =
            skin_tokens_qwen_prefill_tapped(weights, prepared, &inputs, &[0])
                .map_err(|error| error.to_string())?;
        let layer0 = layer_taps
            .first()
            .ok_or_else(|| "detailed prefill returned no layer-0 tap".to_string())?;
        let reference = load_f32(
            &oracle.join("prefill_layer0_hidden.npy"),
            &[1, 514, SKIN_TOKENS_QWEN_WIDTH],
        )?;
        compare("generation_prefill_layer0_hidden", &layer0.hidden, &reference)?;
        for (name, native) in [
            ("key", layer0.key_head_major.as_slice()),
            ("value", layer0.value_head_major.as_slice()),
        ] {
            let reference = load_f32(
                &oracle.join(format!("prefill_kv0_{name}.npy")),
                &[
                    1,
                    SKIN_TOKENS_QWEN_KV_HEADS,
                    514,
                    SKIN_TOKENS_QWEN_HEAD_DIM,
                ],
            )?;
            compare(&format!("generation_prefill_kv0_{name}"), native, &reference)?;
        }

        let beam_ids = load_npy(&oracle.join("first_decode_input_ids.npy"))?.ids_u32()?;
        let parents: Vec<u32> = (0..beam_ids.len() as u32).collect();
        let cache = prefill
            .cache
            .expand_beams(beam_ids.len())
            .map_err(|error| error.to_string())?;
        let (decoded, tap) = skin_tokens_qwen_decode_beams_tapped(
            weights, prepared, cache, &beam_ids, &parents,
        )
        .map_err(|error| error.to_string())?;
        let reference = load_f32(
            &oracle.join("first_decode_logits.npy"),
            &[beam_ids.len(), 1, SKIN_TOKENS_VOCAB],
        )?;
        compare(
            "generation_first_decode_batched_logits",
            &decoded.logits,
            &reference,
        )?;
        compare_first_decode_operator_tap(&tap, oracle, beam_ids.len())?;
    }
    let prefix_values = &inputs[..512 * SKIN_TOKENS_QWEN_WIDTH];
    let prefix = gpu_upload(prefix_values, 512, SKIN_TOKENS_QWEN_WIDTH)
        .map_err(|error| error.to_string())?;
    let oracle_raw = load_npy(&oracle.join("raw_generate_ids.npy"))?.ids_u32()?;
    let oracle_full = load_npy(&oracle.join("full_ids.npy"))?.ids_u32()?;
    let oracle_skeleton = load_npy(&oracle.join("skeleton_tokens.npy"))?.ids_u32()?;
    let mut failures = Vec::new();

    if mode.compatibility() {
        let params = SkinTokensGenerationParams {
            seed: 424_242,
            grammar: SkinTokensGenerationGrammar::OfficialOffByOneCompatibility,
            ..SkinTokensGenerationParams::default()
        };
        let started = Instant::now();
        let (output, trace) = skin_tokens_qwen_generate_traced(weights, prepared, &prefix, &params)
            .map_err(|error| error.to_string())?;
        let seconds = started.elapsed().as_secs_f64();
        println!(
            "[generation] policy=OfficialOffByOneCompatibility beams={} generated={} skeleton={} fsq_groups={} seconds={seconds:.6}",
            params.num_beams,
            output.generated_ids.len(),
            output.skeleton_ids.len(),
            output.fsq_indices.len(),
        );
        for result in [
            compare_generation_trace(&trace, oracle, params.num_beams),
            compare_ids(
                "generation_compatibility_raw_ids",
                &output.generated_ids,
                &oracle_raw,
            ),
            compare_ids(
                "generation_compatibility_full_ids",
                &output.full_ids,
                &oracle_full,
            ),
            compare_ids(
                "generation_compatibility_skeleton_ids",
                &output.skeleton_ids,
                &oracle_skeleton,
            ),
        ] {
            if let Err(error) = result {
                println!("[generation] COMPATIBILITY_MISMATCH {error}");
                failures.push(error);
            }
        }
        if seconds > 33.6 {
            failures.push(format!(
                "compatibility generation took {seconds:.6}s, exceeding 33.6s oracle"
            ));
        }
    }

    if mode.strict() {
        let params = SkinTokensGenerationParams {
            seed: 424_242,
            grammar: SkinTokensGenerationGrammar::Strict,
            ..SkinTokensGenerationParams::default()
        };
        let started = Instant::now();
        let output = skin_tokens_qwen_generate(weights, prepared, &prefix, &params, None)
            .map_err(|error| error.to_string())?;
        let seconds = started.elapsed().as_secs_f64();
        let grammar = SkinTokensGrammar::from_tokens(&output.full_ids)
            .map_err(|error| format!("strict result violates grammar: {error}"))?;
        let bones = match grammar.phase() {
            SkinTokensGenerationPhase::Complete { bones } => bones,
            phase => return Err(format!("strict result did not complete: {phase:?}")),
        };
        if output.full_ids.get(..2)
            != Some(
                [
                    SKIN_TOKENS_TOKEN_BOS,
                    SKIN_TOKENS_TOKEN_CLASS_ARTICULATION_XL,
                ]
                .as_slice(),
            )
            || output.fsq_indices.len() != bones
        {
            return Err(format!(
                "strict result contract failed: bones={bones} fsq_groups={} starts={:?}",
                output.fsq_indices.len(),
                output.full_ids.get(..2),
            ));
        }
        println!(
            "[generation] policy=Strict beams={} generated={} skeleton={} bones={} genuine_fsq={} seconds={seconds:.6} structural_valid=true",
            params.num_beams,
            output.generated_ids.len(),
            output.skeleton_ids.len(),
            bones,
            output.fsq_indices.len() * 4,
        );
        let artifact = format!(
            "{{\n  \"policy\": \"Strict\",\n  \"seed\": {},\n  \"score\": {:?},\n  \"generated_ids\": {:?},\n  \"full_ids\": {:?},\n  \"skeleton_ids\": {:?},\n  \"fsq_indices\": {:?}\n}}\n",
            params.seed,
            output.score,
            output.generated_ids,
            output.full_ids,
            output.skeleton_ids,
            output.fsq_indices,
        );
        let artifact_path = oracle.join("native_strict_generation.json");
        std::fs::write(&artifact_path, artifact).map_err(|error| {
            format!(
                "failed to write strict generation artifact {}: {error}",
                artifact_path.display()
            )
        })?;
        println!(
            "[generation] strict_artifact={} skeleton_ids={:?} fsq_indices={:?}",
            artifact_path.display(),
            output.skeleton_ids,
            output.fsq_indices,
        );
        if seconds > 33.6 {
            failures.push(format!(
                "strict generation took {seconds:.6}s, exceeding 33.6s oracle"
            ));
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let checkpoint = args.next().ok_or_else(|| {
        "usage: skin-tokens-qwen-validate <tokenrig.safetensors> <oracle-dir> [warm-runs] [generation-oracle-dir] [none|compatibility|strict|both]"
            .to_string()
    })?;
    let oracle = args.next().ok_or_else(|| {
        "usage: skin-tokens-qwen-validate <tokenrig.safetensors> <oracle-dir> [warm-runs] [generation-oracle-dir] [none|compatibility|strict|both]"
            .to_string()
    })?;
    let warm_runs = args
        .next()
        .map(|value| value.parse::<usize>())
        .transpose()
        .map_err(|error| format!("invalid warm-runs: {error}"))?
        .unwrap_or(2);
    let generation_oracle = args.next();
    let generation_mode = GenerationMode::parse(args.next())?;
    if generation_mode != GenerationMode::None && generation_oracle.is_none() {
        return Err("generation mode requires generation-oracle-dir".to_string());
    }
    if let Some(extra) = args.next() {
        return Err(format!("unexpected argument '{extra}'"));
    }
    let checkpoint = Path::new(&checkpoint);
    let oracle = Path::new(&oracle);
    let input = load_f32(
        &oracle.join("qwen_inputs_embeds.npy"),
        &[1, 514, SKIN_TOKENS_QWEN_WIDTH],
    )?;

    let started = Instant::now();
    let weights = SkinTokensWeights::load(checkpoint).map_err(|error| error.to_string())?;
    let load_seconds = started.elapsed().as_secs_f64();
    let started = Instant::now();
    let prepared = SkinTokensQwenPrepared::prepare(&weights).map_err(|error| error.to_string())?;
    let prepare_seconds = started.elapsed().as_secs_f64();
    println!(
        "[setup] header_load={load_seconds:.6}s prepare_vectors_and_embeddings={prepare_seconds:.6}s"
    );

    if oracle.join("input_norm.npy").is_file() {
        let tap = skin_tokens_qwen_projection_tap(&weights, &prepared, &input)
            .map_err(|error| error.to_string())?;
        for (name, native, shape) in [
            (
                "input_norm",
                tap.input_norm.as_slice(),
                vec![1, 514, SKIN_TOKENS_QWEN_WIDTH],
            ),
            (
                "q_proj",
                tap.query_projected.as_slice(),
                vec![1, 514, 2_048],
            ),
            (
                "k_proj",
                tap.key_projected.as_slice(),
                vec![1, 514, 1_024],
            ),
            (
                "v_proj",
                tap.value_projected.as_slice(),
                vec![1, 514, 1_024],
            ),
            (
                "q_norm",
                tap.query_normalized.as_slice(),
                vec![1, 514, 16, SKIN_TOKENS_QWEN_HEAD_DIM],
            ),
            (
                "k_norm",
                tap.key_normalized.as_slice(),
                vec![1, 514, 8, SKIN_TOKENS_QWEN_HEAD_DIM],
            ),
        ] {
            let reference = load_f32(&oracle.join(format!("{name}.npy")), &shape)?;
            compare(&format!("operator_{name}"), native, &reference)?;
        }
        for (name, native) in [
            ("rope_cos", tap.rope_cos.as_slice()),
            ("rope_sin", tap.rope_sin.as_slice()),
        ] {
            let path = oracle.join(format!("{name}.npy"));
            if path.is_file() {
                let reference = load_f32(&path, &[1, 514, SKIN_TOKENS_QWEN_HEAD_DIM])?;
                // HF duplicates the half-width rotary table for the two
                // rotate-half blocks; native stores just the unique half.
                let mut unique = Vec::with_capacity(514 * SKIN_TOKENS_QWEN_HEAD_DIM / 2);
                for row in reference.chunks_exact(SKIN_TOKENS_QWEN_HEAD_DIM) {
                    unique.extend_from_slice(&row[..SKIN_TOKENS_QWEN_HEAD_DIM / 2]);
                }
                compare(&format!("operator_{name}"), native, &unique)?;
            }
        }
        let key_path = oracle.join("key_cache.npy");
        if key_path.is_file() {
            let reference = load_f32(
                &key_path,
                &[
                    1,
                    SKIN_TOKENS_QWEN_KV_HEADS,
                    514,
                    SKIN_TOKENS_QWEN_HEAD_DIM,
                ],
            )?;
            let mut head_major = vec![0.0f32; tap.key_rope.len()];
            for head in 0..SKIN_TOKENS_QWEN_KV_HEADS {
                for row in 0..514 {
                    let source = (row * SKIN_TOKENS_QWEN_KV_HEADS + head)
                        * SKIN_TOKENS_QWEN_HEAD_DIM;
                    let destination = (head * 514 + row) * SKIN_TOKENS_QWEN_HEAD_DIM;
                    head_major[destination..destination + SKIN_TOKENS_QWEN_HEAD_DIM]
                        .copy_from_slice(&tap.key_rope[source..source + SKIN_TOKENS_QWEN_HEAD_DIM]);
                }
            }
            compare("operator_key_rope", &head_major, &reference)?;
        }
    }

    let started = Instant::now();
    let (prefill, taps) =
        skin_tokens_qwen_prefill_tapped(&weights, &prepared, &input, &[0, 13, 27])
            .map_err(|error| error.to_string())?;
    let parity_seconds = started.elapsed().as_secs_f64();
    for tap in &taps {
        let layer_name = format!("qwen_layer{}", tap.layer);
        let reference = load_f32(
            &oracle.join(format!("{layer_name}.npy")),
            &[1, 514, SKIN_TOKENS_QWEN_WIDTH],
        )?;
        compare(&layer_name, &tap.hidden, &reference)?;
        if tap.layer == 0 || tap.layer == 27 {
            for (kind, native) in [
                ("key", tap.key_head_major.as_slice()),
                ("value", tap.value_head_major.as_slice()),
            ] {
                let name = format!("qwen_kv{}_{}", tap.layer, kind);
                let reference = load_f32(
                    &oracle.join(format!("{name}.npy")),
                    &[
                        1,
                        SKIN_TOKENS_QWEN_KV_HEADS,
                        514,
                        SKIN_TOKENS_QWEN_HEAD_DIM,
                    ],
                )?;
                compare(&name, native, &reference)?;
            }
        }
    }
    let logits = load_f32(
        &oracle.join("qwen_prefill_logits_last.npy"),
        &[1, SKIN_TOKENS_VOCAB],
    )?;
    let logits_metrics = compare("qwen_prefill_logits_last", &prefill.logits_last, &logits)?;
    println!("[parity] tapped_prefill={parity_seconds:.6}s");
    let mut validation_cache = Some(prefill.cache);

    let decode_ids_path = oracle.join("qwen_decode_input_ids.npy");
    if decode_ids_path.is_file() {
        let decode_ids = load_npy(&decode_ids_path)?.ids_u32()?;
        let mut cache = validation_cache
            .take()
            .ok_or_else(|| "decode validator cache was already consumed".to_string())?;
        for (step, token) in decode_ids.into_iter().enumerate() {
            let started = Instant::now();
            let decoded = skin_tokens_qwen_decode_step(&weights, &prepared, cache, token)
                .map_err(|error| error.to_string())?;
            println!(
                "[decode] step={step} token={token} sequence={} seconds={:.6}",
                decoded.cache.sequence,
                started.elapsed().as_secs_f64(),
            );
            let logits_path = oracle.join(format!("qwen_decode{step}_logits.npy"));
            if logits_path.is_file() {
                let reference = load_f32(&logits_path, &[1, SKIN_TOKENS_VOCAB])?;
                compare(
                    &format!("qwen_decode{step}_logits"),
                    &decoded.logits_last,
                    &reference,
                )?;
            }
            cache = decoded.cache;
        }
    }

    let beam_ids_path = oracle.join("first_decode_input_ids.npy");
    if beam_ids_path.is_file() {
        let beam_ids = load_npy(&beam_ids_path)?.ids_u32()?;
        let beam_logits = load_f32(
            &oracle.join("first_decode_logits.npy"),
            &[beam_ids.len(), 1, SKIN_TOKENS_VOCAB],
        )?;
        let beam_key = load_f32(
            &oracle.join("first_decode_key.npy"),
            &[
                beam_ids.len(),
                SKIN_TOKENS_QWEN_KV_HEADS,
                515,
                SKIN_TOKENS_QWEN_HEAD_DIM,
            ],
        )?;
        let beam_value = load_f32(
            &oracle.join("first_decode_value.npy"),
            &[
                beam_ids.len(),
                SKIN_TOKENS_QWEN_KV_HEADS,
                515,
                SKIN_TOKENS_QWEN_HEAD_DIM,
            ],
        )?;
        let cache_copy_started = Instant::now();
        let base_cache = validation_cache
            .take()
            .ok_or_else(|| "beam validator cache was already consumed".to_string())?;
        let mut beam_caches = Vec::with_capacity(beam_ids.len());
        for _ in 0..beam_ids.len() {
            beam_caches.push(
                base_cache
                    .try_clone_device()
                    .map_err(|error| error.to_string())?,
            );
        }
        println!(
            "[decode] initial_beam_cache_copies={} seconds={:.6}",
            beam_caches.len(),
            cache_copy_started.elapsed().as_secs_f64(),
        );
        let kv_stride = SKIN_TOKENS_QWEN_KV_HEADS * 515 * SKIN_TOKENS_QWEN_HEAD_DIM;
        let logits_stride = SKIN_TOKENS_VOCAB;
        let started = Instant::now();
        for (beam, (token, cache)) in beam_ids
            .into_iter()
            .zip(beam_caches.into_iter())
            .enumerate()
        {
            let (decoded, tap) =
                skin_tokens_qwen_decode_step_tapped(&weights, &prepared, cache, token)
                    .map_err(|error| error.to_string())?;
            compare(
                &format!("first_decode_beam{beam}_logits"),
                &decoded.logits_last,
                &beam_logits[beam * logits_stride..(beam + 1) * logits_stride],
            )?;
            compare(
                &format!("first_decode_beam{beam}_key0"),
                &tap.key0_head_major,
                &beam_key[beam * kv_stride..(beam + 1) * kv_stride],
            )?;
            compare(
                &format!("first_decode_beam{beam}_value0"),
                &tap.value0_head_major,
                &beam_value[beam * kv_stride..(beam + 1) * kv_stride],
            )?;
        }
        println!(
            "[decode] sequential_first_step_beams={} seconds={:.6}",
            beam_logits.len() / logits_stride,
            started.elapsed().as_secs_f64(),
        );
        let batched_prefill = skin_tokens_qwen_prefill(&weights, &prepared, &input)
            .map_err(|error| error.to_string())?;
        let batched_cache = batched_prefill
            .cache
            .expand_beams(beam_logits.len() / logits_stride)
            .map_err(|error| error.to_string())?;
        let beam_ids = load_npy(&beam_ids_path)?.ids_u32()?;
        let parents: Vec<u32> = (0..beam_ids.len() as u32).collect();
        let started = Instant::now();
        let detailed = oracle.join("first_decode_layer0_input_norm.npy").is_file();
        let (batched, detailed_tap) = if detailed {
            let (decoded, tap) = skin_tokens_qwen_decode_beams_tapped(
                &weights,
                &prepared,
                batched_cache,
                &beam_ids,
                &parents,
            )
            .map_err(|error| error.to_string())?;
            (decoded, Some(tap))
        } else {
            (
                skin_tokens_qwen_decode_beams(
                    &weights,
                    &prepared,
                    batched_cache,
                    &beam_ids,
                    &parents,
                )
                .map_err(|error| error.to_string())?,
                None,
            )
        };
        let elapsed = started.elapsed().as_secs_f64();
        compare("first_decode_batched_logits", &batched.logits, &beam_logits)?;
        if let Some(tap) = detailed_tap {
            compare_first_decode_operator_tap(&tap, oracle, beam_ids.len())?;
        }
        println!(
            "[decode] batched_first_step_beams={} seconds={elapsed:.6}",
            beam_ids.len(),
        );
    }

    let mut times = Vec::with_capacity(warm_runs);
    for run in 0..warm_runs {
        let started = Instant::now();
        let output = skin_tokens_qwen_prefill(&weights, &prepared, &input)
            .map_err(|error| error.to_string())?;
        let seconds = started.elapsed().as_secs_f64();
        compare(&format!("warm{run}_logits"), &output.logits_last, &logits)?;
        println!("[bench] run={run} seconds={seconds:.6}");
        times.push(seconds);
    }
    times.sort_by(f64::total_cmp);
    if !times.is_empty() {
        println!(
            "[bench] runs={} min={:.6}s median={:.6}s max={:.6}s",
            times.len(),
            times[0],
            times[times.len() / 2],
            times[times.len() - 1],
        );
    }
    if logits_metrics.cosine < 0.999 || logits_metrics.max_abs > 2.0 {
        return Err(format!(
            "prefill logits parity gate failed: cosine {:.9}, max_abs {:.6}",
            logits_metrics.cosine, logits_metrics.max_abs,
        ));
    }
    if let Some(generation_oracle) = generation_oracle {
        run_generation_gate(
            &weights,
            &prepared,
            Path::new(&generation_oracle),
            generation_mode,
        )?;
    }
    println!("[qwen] PASS native TokenRig prefill parity gate");
    Ok(())
}
