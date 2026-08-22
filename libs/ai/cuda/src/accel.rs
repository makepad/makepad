//! Affine-quant matmul specs shared by the CUDA launch surface.
//! Cross-backend dispatch (`try_matmul_nt_*`) stays in the caller that
//! can see both stores — these are just the typed specs.

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
