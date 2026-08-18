// Apple MLX / Metal execution is parked. Windows CUDA (qwen_runtime::cuda,
// text_runtime::cuda_exact) is the live path. Do not revive Metal from this
// crate until the dedicated Mac pass. Shared tensor/safetensors types below
// are still used by makepad-diffusion on CUDA.
mod core;
mod kv;
pub mod multimodal;

pub mod chat;
pub mod layer0_cached_case;
pub mod qwen_runtime;
pub mod text_runtime;

pub use core::*;
pub use kv::{
    GemmaAttentionKind, GemmaKvCache, GemmaKvCacheLayout, GemmaKvCacheSet, GemmaKvCacheSpec,
    GemmaKvError, GemmaKvStateView, KvTensor, KvTensorShape, KvTensorView,
};
pub use qwen_runtime::*;
pub type KvResult<T> = kv::Result<T>;
