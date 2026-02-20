// Runtime settings for the turbo-focused path.
// Keep these as in-code constants (no runtime env toggles).

// Prefer Metal on Apple platforms, CPU elsewhere.
pub(crate) const USE_METAL_BACKEND: bool = cfg!(target_os = "macos");

// Preserve raw ggml weight types so Metal kernels can consume packed formats directly.
pub(crate) const PRESERVE_RAW_WEIGHT_TYPE: bool = USE_METAL_BACKEND;

// Encoder / matmul fast paths.
pub(crate) const ENABLE_METAL_QUANT: bool = true;
pub(crate) const ENABLE_ACT_Q8: bool = cfg!(target_arch = "aarch64");

// Debug logging gates.
pub(crate) const LOG_METAL_MUL_MAT: bool = false;
pub(crate) const LOG_METAL_PIPELINES: bool = false;

// Keep ggml-metal feature gates at default behavior.
pub(crate) const DISABLE_GGML_METAL_BF16: bool = false;
pub(crate) const DISABLE_GGML_METAL_TENSOR: bool = false;
pub(crate) const FORCE_ENABLE_GGML_METAL_TENSOR: bool = false;
