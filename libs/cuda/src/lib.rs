//! makepad-cuda is now a thin facade over makepad-ai-cuda (the CUDA
//! warehouse at libs/ai/cuda; see /aiarch.md §1 + §4, lane T4). It exists so
//! `makepad_cuda::` paths in libs/ggml and libs/llama keep working
//! unedited; all real driver/cuDNN code lives in makepad-ai-cuda.

// `pub use ...::*` also re-exports public modules, so `makepad_cuda::cudnn::*`
// (used throughout libs/ggml's CUDA backend) keeps resolving without a
// separate `pub mod cudnn` shim — declaring one here would conflict with
// this glob's re-export of the same name.
pub use makepad_ai_cuda::*;
