//! The link gate: how this crate declares its C FFI so that a machine with
//! no usable CUDA toolkit still **links**.
//!
//! `makepad-ai-cuda` is an unconditional dependency of every AI family crate
//! (`ai-common`, `ai-llm`, `ai-metal`, `voice`, and through them `ai-stems`),
//! and therefore of standalone apps — the VJ — that must build and run on
//! machines that have no CUDA at all. `build.rs` is the single authority on
//! whether this machine actually compiled the `.cu` kernels; it emits
//! `cargo:rustc-cfg=makepad_ai_cuda_kernels` (and the matching
//! `cargo:rustc-link-lib=dylib=cudart` / `cublas` / `cublasLt` lines) only
//! then.
//!
//! When it does not, nothing in the final binary may *reference* a `cuda*`,
//! `cublas*` or `mkllm_*` symbol. A declaration alone is not the problem — a
//! declaration that reachable code CALLS is: MSVC's linker then fails the
//! whole build with `unresolved external symbol cudaFree`, which is exactly
//! how a toolkit-less (or failed-kernel-build) Windows machine lost the VJ.
//! Gating only on `target_os` is not enough, because "Windows" and "has a
//! working CUDA toolkit" are different questions.
//!
//! So every extern block in this crate is written through [`cuda_ffi!`],
//! which emits either the real `unsafe extern "C"` declarations, or
//! same-signature Rust functions that fail closed. The public API, the
//! wrapper types and every call site are identical in both modes — only the
//! linker can tell the difference. Runtime behaviour in stub mode is an
//! error return, which the existing `CudaError` paths already handle: no
//! device is found, so callers fall through to Metal or the CPU.

/// The value a stubbed FFI function returns when this build has no CUDA.
///
/// Implemented only for the return types that actually appear in this
/// crate's extern blocks, deliberately: a blanket pointer impl would
/// silently hand a null to some future string getter.
#[cfg(not(makepad_ai_cuda_kernels))]
pub(crate) trait NoCudaValue {
    fn no_cuda_value() -> Self;
}

/// `cudaError_t` and `cublasStatus_t` are both `c_int`, so they share this
/// impl. 100 is `cudaErrorNoDevice`, which is the literal truth here and is
/// non-zero — i.e. an error — for the cuBLAS checks as well.
#[cfg(not(makepad_ai_cuda_kernels))]
impl NoCudaValue for std::ffi::c_int {
    fn no_cuda_value() -> Self {
        100
    }
}

/// Scratch-size queries (`mkllm_fattn_*_bytes`). Zero: nothing to allocate
/// for a launch that cannot happen.
#[cfg(not(makepad_ai_cuda_kernels))]
impl NoCudaValue for usize {
    fn no_cuda_value() -> Self {
        0
    }
}

/// `cudaGetErrorString`. Callers null-check, but a real message beats a
/// null: `CudaError::message()` then says why there is no CUDA rather than
/// printing a bare code.
#[cfg(not(makepad_ai_cuda_kernels))]
impl NoCudaValue for *const std::ffi::c_char {
    fn no_cuda_value() -> Self {
        c"CUDA unavailable: makepad-ai-cuda was built without kernels".as_ptr()
    }
}

/// Declare a C FFI block that disappears cleanly when this build has no CUDA.
///
/// Takes exactly what an `unsafe extern "C"` block takes, except that every
/// function must have an explicit return type (there is no useful stub for a
/// `void` CUDA call — add an arm here if one ever appears).
macro_rules! cuda_ffi {
    ($(
        $(#[$attr:meta])*
        pub fn $name:ident($($arg:ident: $ty:ty),* $(,)?) -> $ret:ty;
    )*) => {
        #[cfg(makepad_ai_cuda_kernels)]
        unsafe extern "C" {
            $(
                $(#[$attr])*
                pub fn $name($($arg: $ty),*) -> $ret;
            )*
        }

        $(
            $(#[$attr])*
            #[cfg(not(makepad_ai_cuda_kernels))]
            #[allow(non_snake_case, unused_variables, clippy::missing_safety_doc)]
            #[inline]
            pub unsafe fn $name($($arg: $ty),*) -> $ret {
                <$ret as $crate::link_gate::NoCudaValue>::no_cuda_value()
            }
        )*
    };
}
