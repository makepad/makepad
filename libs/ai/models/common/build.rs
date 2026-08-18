use std::env;

fn main() {
    println!("cargo:rustc-check-cfg=cfg(makepad_ai_cuda_kernels)");
    if env::var("DEP_MAKEPAD_AI_CUDA_KERNELS").as_deref() == Ok("1") {
        println!("cargo:rustc-cfg=makepad_ai_cuda_kernels");
    }
}
