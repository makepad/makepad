//! Backend-neutral accel vocabulary shared by every GPU store and the
//! cross-backend dispatchers in makepad-ai-common: the affine-quant matmul
//! specs and the per-model GEMM precision policy. Dispatch itself
//! (`try_matmul_nt_*`) stays in the caller that can see both stores.

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

/// Per-model precision policy for the generic dense-linear path.
///
/// The default is the validated Flux policy that the old unset environment
/// switches selected. Models with a stricter numerical contract pass an
/// explicit value to the `*_with_precision` launch entry points.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GemmPrecision {
    pub f16_accumulate: bool,
    pub f16_activations: bool,
}

impl Default for GemmPrecision {
    fn default() -> Self {
        Self {
            f16_accumulate: true,
            f16_activations: true,
        }
    }
}
