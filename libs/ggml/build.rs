use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=MAKEPAD_GGML_METAL_PRECOMPILE");
    println!("cargo:rustc-check-cfg=cfg(makepad_ggml_cuda_kernels)");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "macos" {
        build_metallib();
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
    let mlx_dir = format!("{}/src/backend/metal/mlx_qmm", manifest_dir);
    let steel_src = format!("{}/steel_qmm.metal", mlx_dir);

    println!("cargo:rerun-if-changed={}", metal_src);
    println!("cargo:rerun-if-changed={}", common_h);
    println!("cargo:rerun-if-changed={}", impl_h);
    println!("cargo:rerun-if-changed={}", steel_src);
    rerun_if_changed_tree(Path::new(&mlx_dir));

    let _ = fs::create_dir_all(&out_dir);
    let air_path = format!("{}/ggml-metal.air", out_dir);
    let steel_air_path = format!("{}/steel_qmm.air", out_dir);
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

    let steel_output = Command::new("xcrun")
        .args([
            "--sdk",
            "macosx",
            "metal",
            "-O3",
            "-fno-fast-math",
            "-c",
            &steel_src,
            "-I",
            &mlx_dir,
            "-o",
            &steel_air_path,
        ])
        .output();

    let steel_ok = match &steel_output {
        Ok(output) if output.status.success() => true,
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.trim().is_empty() {
                println!(
                    "cargo:warning=failed to compile steel_qmm.metal (status {:?})",
                    output.status
                );
            } else {
                for line in stderr.lines() {
                    println!("cargo:warning=steel qmm: {line}");
                }
            }
            false
        }
        Err(err) => {
            println!("cargo:warning=failed to invoke metal for steel_qmm.metal: {err}");
            false
        }
    };

    let mut metallib = Command::new("xcrun");
    metallib.args(["--sdk", "macosx", "metallib", &air_path]);
    if steel_ok {
        metallib.arg(&steel_air_path);
    } else {
        println!(
            "cargo:warning=steel affine_qmm_t not in metallib; MAKEPAD_METAL_AFFINE_QMM will fall back"
        );
    }
    let metallib_status = metallib.arg("-o").arg(&metallib_path).status();

    let ok = metallib_status.as_ref().is_ok_and(|s| s.success());
    if !ok {
        println!(
            "cargo:warning=failed to build ggml default metallib; runtime source compile will be used"
        );
        let _ = fs::write(&metallib_path, []);
    }

    println!("cargo:rustc-env=MAKEPAD_GGML_METALLIB={}", metallib_path);
}

fn rerun_if_changed_tree(path: &Path) {
    println!("cargo:rerun-if-changed={}", path.display());
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let child = entry.path();
        if child.is_dir() {
            rerun_if_changed_tree(&child);
        } else {
            println!("cargo:rerun-if-changed={}", child.display());
        }
    }
}
