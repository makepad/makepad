use super::{cuda, metal};
use crate::quant::bf16_to_f32;

#[derive(Clone, Copy, Debug)]
pub struct AffineQuantizedMatmulSpec<'a> {
    pub input_bf16_words: &'a [u16],
    pub out_rows: usize,
    pub weight_words_per_row: usize,
    pub qparams_per_row: usize,
    pub bits: u32,
    pub group_size: u64,
    pub cache_namespace: &'a str,
}

#[derive(Clone, Copy, Debug)]
pub struct AffineQuantizedMatmulRowsSpec<'a> {
    pub input_bf16_words: &'a [u16],
    pub input_rows: usize,
    pub out_rows: usize,
    pub weight_words_per_row: usize,
    pub qparams_per_row: usize,
    pub bits: u32,
    pub group_size: u64,
    pub cache_namespace: &'a str,
}

pub fn try_matmul_nt_ggml_bytes(
    a: &[f32],
    bt_bytes: &[u8],
    bt_ggml_type: u32,
    m: usize,
    k: usize,
    n: usize,
) -> Option<Vec<f32>> {
    metal::try_matmul_nt_ggml_bytes(a, bt_bytes, bt_ggml_type, m, k, n)
}

pub fn try_matmul_nt_ggml_bytes_cached<F>(
    a: &[f32],
    bt_ggml_type: u32,
    m: usize,
    k: usize,
    n: usize,
    cache_namespace: &str,
    bt_cache_key: &str,
    load_bt_bytes: F,
) -> Option<Result<Vec<f32>, String>>
where
    F: FnOnce() -> Result<Vec<u8>, String>,
{
    if metal::is_available() {
        let bt_bytes = match load_bt_bytes() {
            Ok(bytes) => bytes,
            Err(err) => return Some(Err(err)),
        };
        if let Some(out) = metal::try_matmul_nt_ggml_bytes(a, &bt_bytes, bt_ggml_type, m, k, n) {
            return Some(Ok(out));
        }
        return None;
    }
    if cuda::is_available() {
        return Some(cuda::try_matmul_nt_ggml_bytes_cached(
            a,
            bt_ggml_type,
            m,
            k,
            n,
            cache_namespace,
            bt_cache_key,
            load_bt_bytes,
        ));
    }
    None
}

pub fn try_matmul_nt_ggml_bytes_cached_bf16_words<F>(
    a_bf16_words: &[u16],
    bt_ggml_type: u32,
    m: usize,
    k: usize,
    n: usize,
    cache_namespace: &str,
    bt_cache_key: &str,
    load_bt_bytes: F,
) -> Option<Result<Vec<f32>, String>>
where
    F: FnOnce() -> Result<Vec<u8>, String>,
{
    if metal::is_available() {
        let a = a_bf16_words
            .iter()
            .copied()
            .map(bf16_to_f32)
            .collect::<Vec<_>>();
        let bt_bytes = match load_bt_bytes() {
            Ok(bytes) => bytes,
            Err(err) => return Some(Err(err)),
        };
        if let Some(out) = metal::try_matmul_nt_ggml_bytes(&a, &bt_bytes, bt_ggml_type, m, k, n) {
            return Some(Ok(out));
        }
        return None;
    }
    if cuda::is_available() {
        return Some(cuda::try_matmul_nt_ggml_bytes_cached_bf16_words(
            a_bf16_words,
            bt_ggml_type,
            m,
            k,
            n,
            cache_namespace,
            bt_cache_key,
            load_bt_bytes,
        ));
    }
    None
}

pub fn try_get_rows_ggml_bytes(
    src: &[u8],
    src_ggml_type: u32,
    n_cols: usize,
    n_rows: usize,
    row_indices: &[i32],
) -> Option<Vec<f32>> {
    metal::try_get_rows_ggml_bytes(src, src_ggml_type, n_cols, n_rows, row_indices)
}

pub fn try_get_rows_ggml_bytes_cached<F>(
    src_ggml_type: u32,
    n_cols: usize,
    n_rows: usize,
    row_indices: &[i32],
    cache_namespace: &str,
    src_cache_key: &str,
    load_src_bytes: F,
) -> Option<Result<Vec<f32>, String>>
where
    F: FnOnce() -> Result<Vec<u8>, String>,
{
    if metal::is_available() {
        let src_bytes = match load_src_bytes() {
            Ok(bytes) => bytes,
            Err(err) => return Some(Err(err)),
        };
        if let Some(out) =
            metal::try_get_rows_ggml_bytes(&src_bytes, src_ggml_type, n_cols, n_rows, row_indices)
        {
            return Some(Ok(out));
        }
        return None;
    }
    if cuda::is_available() {
        return Some(cuda::try_get_rows_ggml_bytes_cached(
            src_ggml_type,
            n_cols,
            n_rows,
            row_indices,
            cache_namespace,
            src_cache_key,
            load_src_bytes,
        ));
    }
    None
}

pub fn try_affine_quantized_matmul_bf16<FW, FS, FB>(
    spec: AffineQuantizedMatmulSpec<'_>,
    weight_cache_key: &str,
    scales_cache_key: &str,
    biases_cache_key: &str,
    load_weight_bytes: FW,
    load_scales_bytes: FS,
    load_biases_bytes: FB,
) -> Option<Result<Vec<f32>, String>>
where
    FW: FnOnce() -> Result<Vec<u8>, String>,
    FS: FnOnce() -> Result<Vec<u8>, String>,
    FB: FnOnce() -> Result<Vec<u8>, String>,
{
    if metal::supports_affine_quantized_matmul(spec.bits, spec.group_size) {
        return Some(metal::try_affine_quantized_matmul_bf16(
            spec,
            weight_cache_key,
            scales_cache_key,
            biases_cache_key,
            load_weight_bytes,
            load_scales_bytes,
            load_biases_bytes,
        ));
    }
    if cuda::supports_affine_quantized_matmul(spec.bits, spec.group_size) {
        return Some(cuda::try_affine_quantized_matmul_bf16(
            spec,
            weight_cache_key,
            scales_cache_key,
            biases_cache_key,
            load_weight_bytes,
            load_scales_bytes,
            load_biases_bytes,
        ));
    }
    None
}

pub fn try_affine_quantized_matmul_bf16_rows<FW, FS, FB>(
    spec: AffineQuantizedMatmulRowsSpec<'_>,
    weight_cache_key: &str,
    scales_cache_key: &str,
    biases_cache_key: &str,
    load_weight_bytes: FW,
    load_scales_bytes: FS,
    load_biases_bytes: FB,
) -> Option<Result<Vec<f32>, String>>
where
    FW: FnOnce() -> Result<Vec<u8>, String>,
    FS: FnOnce() -> Result<Vec<u8>, String>,
    FB: FnOnce() -> Result<Vec<u8>, String>,
{
    if metal::supports_affine_quantized_matmul(spec.bits, spec.group_size) {
        return Some(metal::try_affine_quantized_matmul_bf16_rows(
            spec,
            weight_cache_key,
            scales_cache_key,
            biases_cache_key,
            load_weight_bytes,
            load_scales_bytes,
            load_biases_bytes,
        ));
    }
    if cuda::supports_affine_quantized_matmul(spec.bits, spec.group_size) {
        return Some(cuda::try_affine_quantized_matmul_bf16_rows(
            spec,
            weight_cache_key,
            scales_cache_key,
            biases_cache_key,
            load_weight_bytes,
            load_scales_bytes,
            load_biases_bytes,
        ));
    }
    None
}
