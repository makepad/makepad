//! Cross-family internals shared by the AI model family crates split out of
//! `libs/diffusion` (lanes T6a–T6, /aiarch.md §1).
//!
//! Shared because every family needs them, not because they are a runtime
//! abstraction: `DiffusionError`, the sharded-safetensors reader, the
//! `backend` exec surface (`gpu_*` / `try_*` / compiled-graph helpers),
//! host `.pth` / Metal-GEMM helpers, and the progress-hook types.
//! Family-private internals stay in the family crates.

pub mod accel;
pub mod backend;
pub mod dtype;
pub mod error;
pub mod gpu;
pub mod json;
pub mod metal_accel;
pub mod progress;
pub mod raw_st;
pub mod sharded;
pub mod torch_pth;

pub use dtype::f16_word_to_f32;
pub use error::{DiffusionError, Result};
pub use progress::{
    band_progress, emit_byte_progress, emit_progress, hook_ref, BoxedProgressHook, ProgressHook,
    BYTE_PROGRESS_STEP,
};

// Graph IR + quant helpers that used to live at `makepad_ggml::`.
pub use makepad_ai_cuda::llm_ops;
pub use makepad_ai_cuda::quant;
pub use makepad_ai_cuda::quant::*;
pub use makepad_ai_llm::context;
pub use makepad_ai_llm::core;
pub use makepad_ai_llm::graph;
pub use makepad_ai_llm::mmap;
pub use makepad_ai_llm::op;
pub use makepad_ai_llm::tensor;
pub use makepad_ai_llm::{
    ggml_blck_size_for_type, ggml_ftype_to_tensor_type, ggml_glu_op_name, ggml_op_name,
    ggml_op_symbol, ggml_pad, ggml_row_size_for_type, ggml_type_size_for_type, ggml_unary_op_name,
    BufferUsage, Context, Ftype, GluOp, Graph, GraphEvalOrder, InitParams, LogLevel, MappedRegion,
    NodeId, ObjectType, Op, PoolOp, Prec, ScaleMode, SortOrder, Status, Tensor, TensorDesc,
    TensorFlag, TensorFlags, TensorId, TensorLayout, TensorType, TriType, UnaryOp,
    GGML_DEFAULT_GRAPH_SIZE, GGML_DEFAULT_N_THREADS, GGML_FILE_MAGIC, GGML_FILE_VERSION,
    GGML_MAX_DIMS, GGML_MAX_NAME, GGML_MAX_N_THREADS, GGML_MAX_OP_PARAMS, GGML_MAX_PARAMS,
    GGML_MAX_SRC, GGML_MEM_ALIGN, GGML_MROPE_SECTIONS, GGML_QNT_VERSION, GGML_QNT_VERSION_FACTOR,
    GGML_ROPE_TYPE_IMROPE, GGML_ROPE_TYPE_MROPE, GGML_ROPE_TYPE_NEOX, GGML_ROPE_TYPE_NORMAL,
    GGML_ROPE_TYPE_VISION, GGML_SCALE_FLAG_ALIGN_CORNERS, GGML_SCALE_FLAG_ANTIALIAS,
};
pub use makepad_ai_metal::{BackendCapabilities, BackendInfo, BackendKind};

/// Re-export of the macOS RIFE op profiler dump (no-op elsewhere).
#[cfg(target_os = "macos")]
pub fn metal_rife_prof_dump() {
    makepad_ai_metal::rife::prof_dump();
}
