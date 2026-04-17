use std::env;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=CUDA_HOME");
    println!("cargo:rerun-if-env-changed=CUDA_PATH");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let Some(cuda_root) = cuda_root(&target_os) else {
        return;
    };

    if target_os == "linux" {
        let lib_dir = cuda_root.join("lib64");
        if lib_dir.join("libcudart.so").exists() {
            println!("cargo:rustc-link-search=native={}", lib_dir.display());
            println!("cargo:rustc-link-lib=dylib=cudart");
            if lib_dir.join("libcublas.so").exists() {
                println!("cargo:rustc-link-lib=dylib=cublas");
            }
        }
    } else if target_os == "windows" {
        let lib_dir = cuda_root.join("lib").join("x64");
        if lib_dir.join("cudart.lib").exists() {
            println!("cargo:rustc-link-search=native={}", lib_dir.display());
            println!("cargo:rustc-link-lib=dylib=cudart");
            if lib_dir.join("cublas.lib").exists() {
                println!("cargo:rustc-link-lib=dylib=cublas");
            }
        }
    }
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
