use std::env;
use std::fs;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=MAKEPAD_GGML_METAL_PRECOMPILE");

    if env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "macos" {
        build_metallib();
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
