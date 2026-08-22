use std::env;

// Whether `cuda_exec/real.rs` compiles at all.
//
// The dispatcher calls both the CUDA runtime (`cudaMalloc`, `cudaFree`, ...)
// and the `mkllm_*` kernels, and BOTH arrive from makepad-ai-cuda: its nvcc
// build produces the kernel objects, and its build script emits the
// `cargo:rustc-link-lib` lines for cudart/cuBLAS. So makepad-ai-cuda's answer
// is the only honest one, and cargo hands it to us — we are a direct
// dependent — through the `links = "makepad_ai_cuda"` handshake as
// DEP_MAKEPAD_AI_CUDA_KERNELS. libs/voice, libs/ai/metal and
// libs/ai/models/common already gate on exactly this.
//
// Probing for nvcc here instead, which is what this script used to do, is how
// a machine WITH the CUDA toolkit still failed to link: nvcc existed, so
// real.rs compiled and referenced `cudaFree`, while makepad-ai-cuda had
// dropped out of its kernel build (MAKEPAD_GGML_NO_CUDA, an nvcc failure, no
// MSVC lib.exe, a toolkit whose lib dir it could not find) and emitted no
// CUDA link directives at all. Two build scripts answering the same question
// separately can only ever agree by luck.
fn main() {
    println!("cargo:rustc-check-cfg=cfg(makepad_llama_cuda_kernels)");
    if env::var("DEP_MAKEPAD_AI_CUDA_KERNELS").as_deref() != Ok("1") {
        return;
    }
    // The arch makepad-ai-cuda actually compiled for, so real.rs's
    // SASS-compatibility check tests the kernels that exist rather than a
    // second guess at them.
    if let Ok(arch) = env::var("DEP_MAKEPAD_AI_CUDA_ARCH") {
        println!("cargo:rustc-env=MAKEPAD_LLAMA_CUDA_ARCH={arch}");
    }
    println!("cargo:rustc-cfg=makepad_llama_cuda_kernels");
}
