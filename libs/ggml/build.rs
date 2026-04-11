use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=MAKEPAD_GGML_METAL_PRECOMPILE");
    println!("cargo:rerun-if-env-changed=MAKEPAD_GGML_CUDA_ARCH");
    println!("cargo:rustc-check-cfg=cfg(makepad_ggml_cuda_kernels)");

    if env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "macos" {
        build_metallib();
    }
    if env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "linux" {
        build_cuda_backends();
    }
}

fn build_metallib() {
    let precompile_default = env::var_os("CARGO_FEATURE_METAL_PRECOMPILE").is_some();
    let precompile_enabled = env::var("MAKEPAD_GGML_METAL_PRECOMPILE")
        .ok()
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            !(v.is_empty() || v == "0" || v == "false" || v == "no" || v == "off")
        })
        .unwrap_or(precompile_default);

    let out_dir = env::var("OUT_DIR").unwrap();
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let metal_dir = format!("{}/src/backend/metal/ggml", manifest_dir);
    let metal_src = format!("{}/ggml-metal.metal", metal_dir);
    let common_h = format!("{}/ggml-common.h", metal_dir);
    let impl_h = format!("{}/ggml-metal-impl.h", metal_dir);

    println!("cargo:rerun-if-changed={}", metal_src);
    println!("cargo:rerun-if-changed={}", common_h);
    println!("cargo:rerun-if-changed={}", impl_h);

    let _ = fs::create_dir_all(&out_dir);
    let air_path = format!("{}/ggml-metal.air", out_dir);
    let metallib_path = format!("{}/ggml-default.metallib", out_dir);

    if !precompile_enabled {
        let _ = fs::write(&metallib_path, []);
        println!("cargo:rustc-env=MAKEPAD_GGML_METALLIB={}", metallib_path);
        return;
    }

    let metal_status = Command::new("xcrun")
        .args([
            "--sdk",
            "macosx",
            "metal",
            "-O3",
            "-fno-fast-math",
            "-c",
            &metal_src,
            "-I",
            &metal_dir,
            "-o",
            &air_path,
        ])
        .status();

    let ok = metal_status.as_ref().is_ok_and(|s| s.success());
    if !ok {
        println!(
            "cargo:warning=failed to compile ggml-metal.metal to AIR; runtime source compile will be used"
        );
        let _ = fs::write(&metallib_path, []);
        println!("cargo:rustc-env=MAKEPAD_GGML_METALLIB={}", metallib_path);
        return;
    }

    let metallib_status = Command::new("xcrun")
        .args([
            "--sdk",
            "macosx",
            "metallib",
            &air_path,
            "-o",
            &metallib_path,
        ])
        .status();

    let ok = metallib_status.as_ref().is_ok_and(|s| s.success());
    if !ok {
        println!(
            "cargo:warning=failed to build ggml default metallib; runtime source compile will be used"
        );
        let _ = fs::write(&metallib_path, []);
    }

    println!("cargo:rustc-env=MAKEPAD_GGML_METALLIB={}", metallib_path);
}

fn build_cuda_backends() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src_paths = [
        manifest_dir.join("src/backend/cuda/affine.cu"),
        manifest_dir.join("src/backend/cuda/nvfp4.cu"),
        manifest_dir.join("src/backend/cuda/ops.cu"),
    ];
    for src_path in &src_paths {
        println!("cargo:rerun-if-changed={}", src_path.display());
    }

    let cuda_root = cuda_root().unwrap_or_else(|| PathBuf::from("/usr/local/cuda"));
    let nvcc = cuda_root.join("bin/nvcc");
    if !nvcc.exists() {
        println!(
            "cargo:warning=CUDA nvcc not found at {}; CUDA backends disabled",
            nvcc.display()
        );
        return;
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let lib_path = out_dir.join("libggml_cuda_affine.a");
    let arch = env::var("MAKEPAD_GGML_CUDA_ARCH").unwrap_or_else(|_| "120".to_string());

    let mut obj_paths = Vec::new();
    for src_path in &src_paths {
        let stem = src_path.file_stem().unwrap().to_string_lossy();
        let obj_path = out_dir.join(format!("ggml_cuda_{stem}.o"));
        let status = Command::new(&nvcc)
            .args([
                "-std=c++17",
                "-O3",
                "-c",
                "-Xcompiler",
                "-fPIC",
                "-I",
                cuda_root.join("include").to_string_lossy().as_ref(),
                "-gencode",
                format!("arch=compute_{arch},code=sm_{arch}").as_str(),
                "-o",
                obj_path.to_string_lossy().as_ref(),
                src_path.to_string_lossy().as_ref(),
            ])
            .status();

        let ok = status.as_ref().is_ok_and(|s| s.success());
        if !ok {
            println!(
                "cargo:warning=failed to compile CUDA backend source {}; CUDA path disabled",
                src_path.display()
            );
            return;
        }
        obj_paths.push(obj_path);
    }

    let mut ar = Command::new("ar");
    ar.arg("crus").arg(lib_path.to_string_lossy().as_ref());
    for obj_path in &obj_paths {
        ar.arg(obj_path.to_string_lossy().as_ref());
    }
    let ar_status = ar.status();
    let ok = ar_status.as_ref().is_ok_and(|s| s.success());
    if !ok {
        println!("cargo:warning=failed to archive CUDA backends; CUDA path disabled");
        return;
    }

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=ggml_cuda_affine");
    println!("cargo:rustc-link-lib=dylib=stdc++");
    println!("cargo:rustc-cfg=makepad_ggml_cuda_kernels");
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
