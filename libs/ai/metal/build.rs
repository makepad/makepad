use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

// The metallib build for THE Metal shader store (aiarch.md §1 + §8b ring 1,
// lane T5). Absorbed verbatim from libs/ggml/build.rs::build_metallib —
// same env var names (MAKEPAD_GGML_METAL_PRECOMPILE, still read directly
// off the process environment; build scripts see it regardless of which
// crate's Cargo.toml declares it), same feature name (`metal-precompile`,
// forwarded here from makepad-ggml via
// `metal-precompile = ["makepad-ai-metal/metal-precompile"]` in
// libs/ggml/Cargo.toml so CARGO_FEATURE_METAL_PRECOMPILE lands on THIS
// build script), same three-mode behavior (precompile disabled /
// precompile attempted but failed / precompile succeeded), same output
// filename (`ggml-default.metallib`).
//
// This crate sets `links = "makepad_ai_metal"` in its Cargo.toml, so on
// success (in EVERY mode — the metallib file always exists, empty or not)
// it emits the links-metadata line `cargo:metallib=<path>`, which flows to
// immediate dependents (libs/ggml) as `DEP_MAKEPAD_AI_METAL_METALLIB`.
// libs/ggml/build.rs reads that env var and re-emits
// `cargo:rustc-env=MAKEPAD_GGML_METALLIB=<path>` (same env name as before
// the move), so `libs/ggml/src/backend/metal/{runtime,compat}.rs`'s
// `include_bytes!(env!("MAKEPAD_GGML_METALLIB"))` sites need zero edits.
//
// Two of the include_str! sites in those same files (ggml-metal.metal,
// ggml-common.h, ggml-metal-impl.h) used to reach the shader tree by a
// RELATIVE path (it used to sit right next to them). Now that the shader
// tree lives in this crate, we also emit `cargo:shader_dir=<dir>`, which
// flows to libs/ggml as `DEP_MAKEPAD_AI_METAL_SHADER_DIR`; libs/ggml's
// build.rs re-emits that as `cargo:rustc-env=MAKEPAD_GGML_METAL_SHADER_DIR=<dir>`
// and the include sites switch to
// `include_str!(concat!(env!("MAKEPAD_GGML_METAL_SHADER_DIR"), "/ggml-metal.metal"))`
// — file contents stay byte-identical, only how the path is found changes.
fn main() {
    println!("cargo:rerun-if-env-changed=MAKEPAD_GGML_METAL_PRECOMPILE");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "macos" {
        build_metallib();
    } else {
        // env! sites in macos-only modules still need a value if rustc
        // type-checks them. Point at this crate's shader tree.
        let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
        let metal_dir = format!("{manifest_dir}/shaders/ggml");
        println!("cargo:rustc-env=MAKEPAD_METAL_SHADER_DIR={metal_dir}");
        println!("cargo:rustc-env=MAKEPAD_GGML_METAL_SHADER_DIR={metal_dir}");
        println!("cargo:rustc-env=MAKEPAD_METALLIB=/dev/null");
        println!("cargo:rustc-env=MAKEPAD_GGML_METALLIB=/dev/null");
    }
    // non-macos targets: nothing to do, no metallib, no shader_dir line —
    // dependents see DEP_MAKEPAD_AI_METAL_METALLIB / _SHADER_DIR unset.
}

fn build_metallib() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let metal_dir = format!("{}/shaders/ggml", manifest_dir);
    let metal_src = format!("{}/ggml-metal.metal", metal_dir);
    let common_h = format!("{}/ggml-common.h", metal_dir);
    let impl_h = format!("{}/ggml-metal-impl.h", metal_dir);
    let mlx_dir = format!("{}/shaders/mlx_qmm", manifest_dir);
    let steel_src = format!("{}/steel_qmm.metal", mlx_dir);

    println!("cargo:rerun-if-changed={}", metal_src);
    println!("cargo:rerun-if-changed={}", common_h);
    println!("cargo:rerun-if-changed={}", impl_h);
    println!("cargo:rerun-if-changed={}", steel_src);
    rerun_if_changed_tree(Path::new(&mlx_dir));

    // links-metadata handshake: flows to immediate dependents (libs/ggml)
    // as DEP_MAKEPAD_AI_METAL_SHADER_DIR=<metal_dir>. Emitted unconditionally
    // (independent of precompile mode) because the include_str! source
    // fallback in libs/ggml needs it regardless of whether a real metallib
    // was baked.
    println!("cargo:shader_dir={}", metal_dir);
    println!("cargo:rustc-env=MAKEPAD_METAL_SHADER_DIR={metal_dir}");
    println!("cargo:rustc-env=MAKEPAD_GGML_METAL_SHADER_DIR={metal_dir}");

    let precompile_default = env::var_os("CARGO_FEATURE_METAL_PRECOMPILE").is_some();
    let precompile_enabled = env::var("MAKEPAD_GGML_METAL_PRECOMPILE")
        .ok()
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            !(v.is_empty() || v == "0" || v == "false" || v == "no" || v == "off")
        })
        .unwrap_or(precompile_default);

    let out_dir = env::var("OUT_DIR").unwrap();
    let _ = fs::create_dir_all(&out_dir);
    let air_path = format!("{}/ggml-metal.air", out_dir);
    let steel_air_path = format!("{}/steel_qmm.air", out_dir);
    let metallib_path = format!("{}/ggml-default.metallib", out_dir);

    if !precompile_enabled {
        let _ = fs::write(&metallib_path, []);
        emit_metallib_env(&metallib_path);
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
        emit_metallib_env(&metallib_path);
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

    emit_metallib_env(&metallib_path);
}

fn emit_metallib_env(metallib_path: &str) {
    // Own crate's rustc-env: harmless (this crate has no Rust code that
    // reads it today), kept for parity/future use.
    println!("cargo:rustc-env=MAKEPAD_METALLIB={metallib_path}");
    println!("cargo:rustc-env=MAKEPAD_GGML_METALLIB={metallib_path}");
    // links-metadata handshake: flows to immediate dependents (libs/ggml)
    // as DEP_MAKEPAD_AI_METAL_METALLIB=<metallib_path>.
    println!("cargo:metallib={}", metallib_path);
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
