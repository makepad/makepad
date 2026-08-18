use std::env;

fn main() {
    println!("cargo:rustc-check-cfg=cfg(makepad_ggml_cuda_kernels)");

    // The metallib build (shader sources + xcrun metal/metallib) moved to
    // makepad-ai-metal (libs/ai/metal, absorbed lane T5, /aiarch.md §1 +
    // §8b) — this crate depends on it directly so its
    // `links = "makepad_ai_metal"` metadata reaches us here as
    // DEP_MAKEPAD_AI_METAL_METALLIB / DEP_MAKEPAD_AI_METAL_SHADER_DIR.
    // Re-emit under the SAME env names ggml's Rust code has always read
    // (MAKEPAD_GGML_METALLIB, MAKEPAD_GGML_METAL_SHADER_DIR) so
    // src/backend/metal/{runtime,compat}.rs need no logic changes beyond
    // the shader_dir indirection (see step 4 of the T5 migration).
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "macos" {
        if let Ok(metallib) = env::var("DEP_MAKEPAD_AI_METAL_METALLIB") {
            println!("cargo:rustc-env=MAKEPAD_GGML_METALLIB={}", metallib);
        }
        if let Ok(shader_dir) = env::var("DEP_MAKEPAD_AI_METAL_SHADER_DIR") {
            println!(
                "cargo:rustc-env=MAKEPAD_GGML_METAL_SHADER_DIR={}",
                shader_dir
            );
        }
    }

    // The .cu kernels, nvcc build, and MAKEPAD_GGML_REQUIRE_CUDA validation
    // all moved to makepad-ai-cuda (libs/ai/cuda, absorbed lane T4,
    // /aiarch.md §1 + §4) — this crate depends on it directly (in addition
    // to the makepad-cuda facade, which only carries the driver/cuDNN FFI
    // symbols) so its `links = "makepad_ai_cuda"` metadata reaches us here
    // as DEP_MAKEPAD_AI_CUDA_KERNELS. Same cfg name as before the move, so
    // src/backend/cuda/mod.rs needs zero edits.
    if env::var("DEP_MAKEPAD_AI_CUDA_KERNELS").as_deref() == Ok("1") {
        println!("cargo:rustc-cfg=makepad_ggml_cuda_kernels");
    }
}
