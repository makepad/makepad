use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// The nvcc build for THE CUDA store (aiarch.md §1 + §4, lane T4). Absorbed
// verbatim from libs/ggml/build.rs::build_cuda_backends — same env var
// names (fleet boxes set these), same static-lib name (`ggml_cuda_affine`,
// kept unchanged so nothing downstream needs to know it moved), same
// graceful-skip-unless-required semantics.
//
// On success this crate (which sets `links = "makepad_ai_cuda"` in its
// Cargo.toml) emits BOTH its own cfg (`makepad_ai_cuda_kernels`) and the
// links-metadata line `cargo:kernels=1`, which flows to immediate
// dependents (libs/ggml, and libs/cuda's facade) as
// `DEP_MAKEPAD_AI_CUDA_KERNELS`. libs/ggml/build.rs reads that env var and
// re-emits `cargo:rustc-cfg=makepad_cuda_kernels` (same cfg name as
// before the move), so `libs/ggml/src/backend/cuda/mod.rs` needs zero
// edits.
fn main() {
    println!("cargo:rerun-if-env-changed=MAKEPAD_GGML_CUDA_ARCH");
    println!("cargo:rerun-if-env-changed=MAKEPAD_GGML_REQUIRE_CUDA");
    println!("cargo:rerun-if-env-changed=CUDA_HOME");
    println!("cargo:rerun-if-env-changed=CUDA_PATH");
    println!("cargo:rustc-check-cfg=cfg(makepad_ai_cuda_kernels)");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let require_cuda = env_flag("MAKEPAD_GGML_REQUIRE_CUDA");
    if require_cuda && target_os != "linux" && target_os != "windows" {
        panic!(
            "MAKEPAD_GGML_REQUIRE_CUDA=1, but CUDA kernels are unsupported for target OS {target_os:?}"
        );
    }
    if target_os == "linux" || target_os == "windows" {
        build_cuda_backends(&target_os, require_cuda);
    }
    // macos/other targets: nothing to do, no kernels, no cfg, no
    // cargo:kernels line — dependents see DEP_MAKEPAD_AI_CUDA_KERNELS unset.
}

fn env_flag(name: &str) -> bool {
    env::var(name)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn build_cuda_backends(target_os: &str, require_cuda: bool) {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src_paths = [
        manifest_dir.join("kernels/affine.cu"),
        manifest_dir.join("kernels/diffusion_ops.cu"),
        manifest_dir.join("kernels/gated_delta_net.cu"),
        manifest_dir.join("kernels/kquants.cu"),
        manifest_dir.join("kernels/llm/kernels.cu"),
        manifest_dir.join("kernels/nvfp4.cu"),
        manifest_dir.join("kernels/nvfp4_mmq.cu"),
        manifest_dir.join("kernels/ops.cu"),
        manifest_dir.join("kernels/paint_extras.cu"),
        manifest_dir.join("kernels/rife.cu"),
        manifest_dir.join("kernels/roformer.cu"),
        manifest_dir.join("kernels/splat.cu"),
        manifest_dir.join("kernels/ssm_conv.cu"),
    ];
    for src_path in &src_paths {
        println!("cargo:rerun-if-changed={}", src_path.display());
    }
    rerun_if_changed_tree(&manifest_dir.join("kernels/llm"));

    let Some(cuda_root) = cuda_root(target_os) else {
        cuda_unavailable(require_cuda, "CUDA toolkit root not found");
        return;
    };

    let nvcc = if target_os == "windows" {
        cuda_root.join("bin").join("nvcc.exe")
    } else {
        cuda_root.join("bin").join("nvcc")
    };
    if !nvcc.exists() {
        cuda_unavailable(
            require_cuda,
            &format!("CUDA nvcc not found at {}", nvcc.display()),
        );
        return;
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let lib_path = if target_os == "windows" {
        out_dir.join("ggml_cuda_affine.lib")
    } else {
        out_dir.join("libggml_cuda_affine.a")
    };
    let obj_ext = if target_os == "windows" { "obj" } else { "o" };
    let arch = env::var("MAKEPAD_GGML_CUDA_ARCH").unwrap_or_else(|_| "120a".to_string());
    let include_dir = cuda_root.join("include");
    let msvc_bin_dir = if target_os == "windows" {
        find_msvc_tool("cl.exe").and_then(|path| path.parent().map(Path::to_path_buf))
    } else {
        None
    };
    let lib_exe = if target_os == "windows" {
        match find_msvc_tool("lib.exe") {
            Some(path) => Some(path),
            None => {
                cuda_unavailable(require_cuda, "MSVC lib.exe not found");
                return;
            }
        }
    } else {
        None
    };

    let mut obj_paths = Vec::new();
    for src_path in &src_paths {
        let stem = src_path.file_stem().unwrap().to_string_lossy();
        let obj_path = out_dir.join(format!("ggml_cuda_{stem}.{obj_ext}"));
        let arch_flag = format!("arch=compute_{arch},code=sm_{arch}");
        let mut command = Command::new(&nvcc);
        command.args(["-std=c++17", "-O3"]);
        if target_os == "windows" {
            if let Some(msvc_bin_dir) = &msvc_bin_dir {
                command.arg("-ccbin").arg(msvc_bin_dir);
            }
            command.args(["-Xcompiler", "/EHsc"]);
            command.args(["-Xcompiler", "/MD"]);
        } else {
            command.args(["-Xcompiler", "-fPIC"]);
        }
        if let Some(src_dir) = src_path.parent() {
            command.arg("-I").arg(src_dir);
        }
        let status = command
            .args([
                "-c",
                "-I",
                include_dir.to_string_lossy().as_ref(),
                "-gencode",
                arch_flag.as_str(),
                "-o",
                obj_path.to_string_lossy().as_ref(),
                src_path.to_string_lossy().as_ref(),
            ])
            .status();

        let ok = status.as_ref().is_ok_and(|s| s.success());
        if !ok {
            cuda_unavailable(
                require_cuda,
                &format!(
                    "failed to compile CUDA backend source {} ({status:?})",
                    src_path.display()
                ),
            );
            return;
        }
        obj_paths.push(obj_path);
    }

    let archive_ok = if target_os == "windows" {
        let mut lib = Command::new(lib_exe.unwrap());
        lib.arg("/NOLOGO")
            .arg(format!("/OUT:{}", lib_path.to_string_lossy()));
        for obj_path in &obj_paths {
            lib.arg(obj_path);
        }
        lib.status().as_ref().is_ok_and(|s| s.success())
    } else {
        let mut ar = Command::new("ar");
        ar.arg("crus").arg(lib_path.to_string_lossy().as_ref());
        for obj_path in &obj_paths {
            ar.arg(obj_path.to_string_lossy().as_ref());
        }
        ar.status().as_ref().is_ok_and(|s| s.success())
    };
    if !archive_ok {
        cuda_unavailable(require_cuda, "failed to archive CUDA backends");
        return;
    }

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=ggml_cuda_affine");
    if target_os == "linux" {
        println!("cargo:rustc-link-lib=dylib=stdc++");
        let lib_dir = cuda_root.join("lib64");
        println!("cargo:rustc-link-search=native={}", lib_dir.display());
        println!("cargo:rustc-link-lib=dylib=cudart");
        println!("cargo:rustc-link-lib=dylib=cublas");
        println!("cargo:rustc-link-lib=dylib=cublasLt");
        if lib_dir.join("libcudnn.so").exists() {
            println!("cargo:rustc-link-lib=dylib=cudnn");
        }
    } else if target_os == "windows" {
        let lib_dir = cuda_root.join("lib").join("x64");
        println!("cargo:rustc-link-search=native={}", lib_dir.display());
        println!("cargo:rustc-link-lib=dylib=cudart");
        println!("cargo:rustc-link-lib=dylib=cublas");
        println!("cargo:rustc-link-lib=dylib=cublasLt");
        if lib_dir.join("cudnn.lib").exists() {
            println!("cargo:rustc-link-lib=dylib=cudnn");
        }
    }
    println!("cargo:rustc-cfg=makepad_ai_cuda_kernels");
    // links-metadata handshake: flows to immediate dependents (libs/ggml,
    // libs/cuda facade) as DEP_MAKEPAD_AI_CUDA_KERNELS=1.
    println!("cargo:kernels=1");
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

fn cuda_unavailable(required: bool, reason: &str) {
    if required {
        panic!("MAKEPAD_GGML_REQUIRE_CUDA=1, but the CUDA backend build failed: {reason}");
    }
    println!("cargo:warning={reason}; CUDA backends disabled");
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
    let mut entries = fs::read_dir(cuda_root)
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().ok().is_some_and(|ty| ty.is_dir()))
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    entries.pop().map(|entry| entry.path())
}

fn find_msvc_tool(tool_name: &str) -> Option<PathBuf> {
    if let Some(paths) = env::var_os("PATH") {
        if let Some(path) = env::split_paths(&paths)
            .map(|path| path.join(tool_name))
            .find(|candidate| candidate.exists())
        {
            return Some(path);
        }
    }

    let find_pattern = format!(r"VC\Tools\MSVC\**\bin\Hostx64\x64\{tool_name}");
    for installer_root in [
        r"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe",
        r"C:\Program Files\Microsoft Visual Studio\Installer\vswhere.exe",
    ] {
        let vswhere = Path::new(installer_root);
        if !vswhere.exists() {
            continue;
        }
        let output = match Command::new(vswhere)
            .args([
                "-latest",
                "-products",
                "*",
                "-requires",
                "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                "-find",
                find_pattern.as_str(),
            ])
            .output()
        {
            Ok(output) => output,
            Err(_) => continue,
        };
        if !output.status.success() {
            continue;
        }
        if let Some(path) = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(PathBuf::from)
            .filter(|path| path.exists())
        {
            return Some(path);
        }
    }
    None
}
