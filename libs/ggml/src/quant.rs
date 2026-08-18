//! Quant compute lives in the CUDA store (`makepad-ai-cuda::quant`).
//! Re-exported here so existing `makepad_ggml::quant` / `crate::quant` paths
//! keep compiling while callers move over.

pub use makepad_ai_cuda::quant::*;
