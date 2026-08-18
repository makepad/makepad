//! Numerical CUDA canary for the quantized MiniMax-H3 dense-linear path.
//!
//! This is intentionally synthetic: it exercises the exact upload, cached
//! quant payload, CUDA dequantization, BF16 activation, cuBLAS GEMM, and
//! download path without requiring a multi-gigabyte checkpoint.  A node must
//! pass this before a quant tier is advertised as runnable.

use makepad_diffusion::backend::{
    gpu_device_available, gpu_download, gpu_linear_nt_cached, gpu_runtime_trim, gpu_upload,
    gpu_weight_cache_ensure_quant, gpu_weight_cache_evict_prefix_if_loaded, GpuLinearPart,
};
use makepad_ai_common::quant::{
    dequantize_nvfp4_pairs_row, dequantize_q4_k, f32_to_f16, h3_nvfp4_pairs_pack,
    GGML_TYPE_H3_NVFP4_PAIRS,
    GGML_TYPE_H3_NVFP4_PAIRS_PRESCALE, GGML_TYPE_Q4_K,
};

const ROWS: usize = 128;
const COLS: usize = 256;
const NAMESPACE: &str = "h3-quant-cuda-canary";

fn bf16_trunc(value: f32) -> f32 {
    // gpu_linear_nt_cached uses the backend's deliberately truncating f32 to
    // BF16 conversion for both activations and dequantized weight scratch.
    f32::from_bits(value.to_bits() & 0xffff_0000)
}

fn bf16_bytes(values: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 2);
    for &value in values {
        out.extend_from_slice(&((value.to_bits() >> 16) as u16).to_le_bytes());
    }
    out
}

fn input_values() -> Vec<f32> {
    (0..COLS)
        .map(|col| ((col as i32 % 17) - 8) as f32 * 0.125)
        .collect()
}

fn reference_linear(input: &[f32], rows: &[Vec<f32>]) -> Vec<f32> {
    let input: Vec<f32> = input.iter().copied().map(bf16_trunc).collect();
    rows.iter()
        .map(|row| {
            row.iter()
                .copied()
                .map(bf16_trunc)
                .zip(&input)
                .fold(0.0f32, |sum, (weight, &x)| sum + weight * x)
        })
        .collect()
}

fn check_result(label: &str, expected: &[f32], actual: &[f32]) -> Result<(), String> {
    if actual.len() != expected.len() {
        return Err(format!(
            "{label}: output length mismatch: got {}, expected {}",
            actual.len(),
            expected.len()
        ));
    }
    if actual.iter().any(|value| !value.is_finite()) {
        return Err(format!("{label}: CUDA output contains NaN or infinity"));
    }

    let mut dot = 0.0f64;
    let mut aa = 0.0f64;
    let mut ee = 0.0f64;
    let mut squared_error = 0.0f64;
    let mut max_abs_error = 0.0f32;
    let mut max_expected = 0.0f32;
    for (&actual, &expected) in actual.iter().zip(expected) {
        let error = actual - expected;
        dot += actual as f64 * expected as f64;
        aa += (actual as f64).powi(2);
        ee += (expected as f64).powi(2);
        squared_error += (error as f64).powi(2);
        max_abs_error = max_abs_error.max(error.abs());
        max_expected = max_expected.max(expected.abs());
    }
    let cosine = if aa > 0.0 && ee > 0.0 {
        dot / (aa.sqrt() * ee.sqrt())
    } else {
        f64::NAN
    };
    let rmse = (squared_error / expected.len() as f64).sqrt();
    let rms_expected = (ee / expected.len() as f64).sqrt();
    let max_allowed = 0.05f32.max(max_expected * 5e-4);
    let rmse_allowed = 0.01f64.max(rms_expected * 1e-4);
    println!(
        "CANARY {label} cosine={cosine:.9} rmse={rmse:.9} max_abs_error={max_abs_error:.9} max_allowed={max_allowed:.9}"
    );
    if !cosine.is_finite()
        || cosine < 0.999_999
        || rmse > rmse_allowed
        || max_abs_error > max_allowed
    {
        return Err(format!(
            "{label}: numerical mismatch (cosine={cosine:.9}, rmse={rmse:.9}/{rmse_allowed:.9}, max={max_abs_error:.9}/{max_allowed:.9})"
        ));
    }
    Ok(())
}

fn run_cached_linear(
    label: &str,
    ggml_type: u32,
    payload: Vec<u8>,
    expected: &[f32],
) -> Result<(), String> {
    let cache_key = format!("{label}.weight");
    gpu_weight_cache_ensure_quant(NAMESPACE, &cache_key, ggml_type, ROWS, COLS, || {
        Ok(payload)
    })?;
    let input = input_values();
    let x = gpu_upload(&input, 1, COLS)?;
    let part = GpuLinearPart {
        bt_ggml_type: ggml_type,
        n: ROWS,
        cache_key: &cache_key,
        // Match the real H3 path: ensure the payload first, then hand the
        // linear a cache-only descriptor rather than a duplicate host copy.
        bytes: &[],
    };
    let output = gpu_linear_nt_cached(&x, NAMESPACE, &[part], &[])?;
    let actual = gpu_download(&output)?;
    check_result(label, expected, &actual)
}

fn q4_k_case() -> (Vec<u8>, Vec<Vec<f32>>) {
    const BLOCK_BYTES: usize = 144;
    let mut payload = Vec::with_capacity(ROWS * BLOCK_BYTES);
    let mut decoded = Vec::with_capacity(ROWS);
    for row in 0..ROWS {
        let mut block = vec![0u8; BLOCK_BYTES];
        let d = [0.125, 0.25, 0.5, 1.0][row % 4];
        let dmin = [0.0, 0.0625, 0.125, 0.25][(row / 4) % 4];
        block[0..2].copy_from_slice(&f32_to_f16(d).to_le_bytes());
        block[2..4].copy_from_slice(&f32_to_f16(dmin).to_le_bytes());
        for (index, byte) in block[4..16].iter_mut().enumerate() {
            // All 6-bit packed scale fields remain finite; variation across
            // rows catches scale-pack lane and row-stride mistakes.
            *byte = ((row * 11 + index * 7 + 3) & 0x3f) as u8;
        }
        for (index, byte) in block[16..].iter_mut().enumerate() {
            let lo = ((row + index * 3) & 0x0f) as u8;
            let hi = ((row * 5 + index + 1) & 0x0f) as u8;
            *byte = lo | (hi << 4);
        }
        let mut values = vec![0.0f32; COLS];
        dequantize_q4_k(&block, &mut values);
        decoded.push(values);
        payload.extend_from_slice(&block);
    }
    (payload, decoded)
}

fn nvfp4_case(with_pre_scale: bool) -> Result<(Vec<u8>, Vec<Vec<f32>>), String> {
    let scales_per_row = COLS / 16;
    let pairs_per_row = COLS / 2;
    let mut scales = vec![0u8; ROWS * scales_per_row];
    let mut weights = vec![0u8; ROWS * pairs_per_row];
    let scale_codes = [0x30u8, 0x34, 0x38, 0x3c, 0x40]; // .5, .75, 1, 1.5, 2
    for row in 0..ROWS {
        for group in 0..scales_per_row {
            scales[row * scales_per_row + group] = scale_codes[(row + group) % scale_codes.len()];
        }
        for pair in 0..pairs_per_row {
            let lo = ((row + 2 * pair) & 0x0f) as u8;
            let hi = ((3 * row + pair + 1) & 0x0f) as u8;
            weights[row * pairs_per_row + pair] = lo | (hi << 4);
        }
    }

    let pre_values: Option<Vec<f32>> = with_pre_scale.then(|| {
        let values = [0.5f32, 0.75, 1.0, 1.5, 2.0];
        (0..COLS)
            .map(|col| bf16_trunc(values[col % values.len()]))
            .collect()
    });
    let pre_bytes = pre_values.as_deref().map(bf16_bytes);
    let scale2 = 0.25f32;
    let payload = h3_nvfp4_pairs_pack(
        ROWS,
        COLS,
        scale2,
        &scales,
        &weights,
        pre_bytes.as_deref(),
    )
    .ok_or_else(|| "cannot construct synthetic NVFP4 pairs payload".to_string())?;

    let mut decoded = Vec::with_capacity(ROWS);
    for row in 0..ROWS {
        let mut values = vec![0.0f32; COLS];
        dequantize_nvfp4_pairs_row(
            &weights[row * pairs_per_row..(row + 1) * pairs_per_row],
            &scales[row * scales_per_row..(row + 1) * scales_per_row],
            scale2,
            pre_values.as_deref(),
            &mut values,
        );
        decoded.push(values);
    }
    Ok((payload, decoded))
}

fn run() -> Result<(), String> {
    if !gpu_device_available() {
        return Err("CUDA device tensor backend is unavailable".to_string());
    }
    let input = input_values();

    let (q4_payload, q4_rows) = q4_k_case();
    let q4_expected = reference_linear(&input, &q4_rows);
    run_cached_linear("q4_k", GGML_TYPE_Q4_K, q4_payload, &q4_expected)?;

    let (nv_payload, nv_rows) = nvfp4_case(false)?;
    let nv_expected = reference_linear(&input, &nv_rows);
    run_cached_linear(
        "nvfp4_pairs",
        GGML_TYPE_H3_NVFP4_PAIRS,
        nv_payload,
        &nv_expected,
    )?;

    let (awq_payload, awq_rows) = nvfp4_case(true)?;
    let awq_expected = reference_linear(&input, &awq_rows);
    run_cached_linear(
        "nvfp4_pairs_prescale",
        GGML_TYPE_H3_NVFP4_PAIRS_PRESCALE,
        awq_payload,
        &awq_expected,
    )?;
    Ok(())
}

fn main() {
    let result = run();
    let evict_result = gpu_weight_cache_evict_prefix_if_loaded(NAMESPACE);
    let trim_result = gpu_runtime_trim();
    if let Err(error) = result {
        eprintln!("H3_QUANT_CUDA_CANARY_FAILED: {error}");
        std::process::exit(1);
    }
    if let Err(error) = evict_result {
        eprintln!("H3_QUANT_CUDA_CANARY_FAILED during cache eviction: {error}");
        std::process::exit(1);
    }
    if let Err(error) = trim_result {
        eprintln!("H3_QUANT_CUDA_CANARY_FAILED during runtime trim: {error}");
        std::process::exit(1);
    }
    println!("H3_QUANT_CUDA_CANARY_OK");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_payloads_match_declared_shapes() {
        let (q4, q4_rows) = q4_k_case();
        assert_eq!(q4.len(), ROWS * 144);
        assert_eq!(q4_rows.len(), ROWS);
        assert!(q4_rows.iter().all(|row| row.len() == COLS));

        for with_pre in [false, true] {
            let (payload, rows) = nvfp4_case(with_pre).unwrap();
            let expected = 32 + ROWS * (COLS / 16) + ROWS * (COLS / 2)
                + if with_pre { COLS * 2 } else { 0 };
            assert_eq!(payload.len(), expected);
            assert!(rows.iter().all(|row| row.len() == COLS));
        }
    }

    #[test]
    fn bf16_reference_is_finite_and_nontrivial() {
        let input = input_values();
        let (_, rows) = nvfp4_case(true).unwrap();
        let expected = reference_linear(&input, &rows);
        assert!(expected.iter().all(|value| value.is_finite()));
        assert!(expected.iter().any(|value| value.abs() > 1.0));
        // Make sure this tests row addressing, not 128 copies of one dot.
        assert!(expected.windows(2).any(|pair| pair[0] != pair[1]));
    }

    #[test]
    fn bf16_bytes_round_trip_expected_words() {
        let values = [0.5f32, 0.75, 1.0, 1.5, 2.0];
        let bytes = bf16_bytes(&values);
        for (index, chunk) in bytes.chunks_exact(2).enumerate() {
            let word = u16::from_le_bytes([chunk[0], chunk[1]]);
            assert_eq!(makepad_ai_common::quant::bf16_to_f32(word), values[index]);
        }
    }
}
