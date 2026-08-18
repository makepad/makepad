use std::env;
use std::path::{Path, PathBuf};

// CUDA kernels live in makepad-ggml (`src/backend/cuda/llm/`) and compile
// once via libs/ggml/build.rs. This script only enables the llama dispatcher
// (`cuda_exec/real.rs`) when the same toolkit/arch ggml uses is present, so
// `mkllm_*` FFI resolves against ggml's static lib instead of a second nvcc.
fn main() {
    println!("cargo:rerun-if-env-changed=MAKEPAD_GGML_CUDA_ARCH");
    println!("cargo:rerun-if-env-changed=MAKEPAD_GGML_REQUIRE_CUDA");
    println!("cargo:rerun-if-env-changed=CUDA_HOME");
    println!("cargo:rerun-if-env-changed=CUDA_PATH");
    println!("cargo:rustc-check-cfg=cfg(makepad_llama_cuda_kernels)");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "linux" && target_os != "windows" {
        return;
    }

    let Some(cuda_root) = cuda_root(&target_os) else {
        return;
    };
    let nvcc = if target_os == "windows" {
        cuda_root.join("bin").join("nvcc.exe")
    } else {
        cuda_root.join("bin").join("nvcc")
    };
    if !nvcc.exists() {
        return;
    }

    let arch = env::var("MAKEPAD_GGML_CUDA_ARCH").unwrap_or_else(|_| "120a".to_string());
    println!("cargo:rustc-env=MAKEPAD_LLAMA_CUDA_ARCH={arch}");
    println!("cargo:rustc-cfg=makepad_llama_cuda_kernels");
}

fn cuda_root(target_os: &str) -> Option<PathBuf> {
    env::var_os("CUDA_HOME")
        .or_else(|| env::var_os("CUDA_PATH"))
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .or_else(|| {
            if target_os == "windows" {
                latest_windows_cuda_root()
            } else {
                let default = Path::new("/usr/local/cuda");
                default.exists().then(|| default.to_path_buf())
            }
        })
}

fn latest_windows_cuda_root() -> Option<PathBuf> {
    let cuda_root = env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .map(|program_files| {
            program_files
                .join("NVIDIA GPU Computing Toolkit")
                .join("CUDA")
        })?;
    let mut entries = std::fs::read_dir(cuda_root)
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().ok().is_some_and(|ty| ty.is_dir()))
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    entries.pop().map(|entry| entry.path())
}
