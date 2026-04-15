use std::env;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=CUDA_HOME");
    println!("cargo:rerun-if-env-changed=CUDA_PATH");

    if env::var("CARGO_CFG_TARGET_OS").ok().as_deref() != Some("linux") {
        return;
    }

    if let Some(cuda_root) = cuda_root() {
        let lib_dir = cuda_root.join("lib64");
        if lib_dir.join("libcudart.so").exists() {
            println!("cargo:rustc-link-search=native={}", lib_dir.display());
            println!("cargo:rustc-link-lib=dylib=cudart");
            if lib_dir.join("libcublas.so").exists() {
                println!("cargo:rustc-link-lib=dylib=cublas");
            }
        }
    }
}

fn cuda_root() -> Option<PathBuf> {
    env::var_os("CUDA_HOME")
        .or_else(|| env::var_os("CUDA_PATH"))
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .or_else(|| {
            let default = Path::new("/usr/local/cuda");
            default.exists().then(|| default.to_path_buf())
        })
}
