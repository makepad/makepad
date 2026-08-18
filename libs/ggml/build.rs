use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=MAKEPAD_GGML_METAL_PRECOMPILE");
    println!("cargo:rerun-if-env-changed=MAKEPAD_GGML_CUDA_ARCH");
    println!("cargo:rerun-if-env-changed=MAKEPAD_GGML_REQUIRE_CUDA");
    println!("cargo:rerun-if-env-changed=CUDA_HOME");
    println!("cargo:rerun-if-env-changed=CUDA_PATH");
    println!("cargo:rustc-check-cfg=cfg(makepad_ggml_cuda_kernels)");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let require_cuda = env_flag("MAKEPAD_GGML_REQUIRE_CUDA");
    if require_cuda && target_os != "linux" && target_os != "windows" {
        panic!(
            "MAKEPAD_GGML_REQUIRE_CUDA=1, but CUDA kernels are unsupported for target OS {target_os:?}"
        );
    }
    if target_os == "macos" {
        build_metallib();
    }
    if target_os == "linux" || target_os == "windows" {
        build_cuda_backends(&target_os, require_cuda);
    }
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

fn build_cuda_backends(target_os: &str, require_cuda: bool) {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src_paths = [
        manifest_dir.join("src/backend/cuda/affine.cu"),
        manifest_dir.join("src/backend/cuda/diffusion_ops.cu"),
        manifest_dir.join("src/backend/cuda/gated_delta_net.cu"),
        manifest_dir.join("src/backend/cuda/kquants.cu"),
        manifest_dir.join("src/backend/cuda/llm/kernels.cu"),
        manifest_dir.join("src/backend/cuda/nvfp4.cu"),
        manifest_dir.join("src/backend/cuda/nvfp4_mmq.cu"),
        manifest_dir.join("src/backend/cuda/ops.cu"),
        manifest_dir.join("src/backend/cuda/paint_extras.cu"),
        manifest_dir.join("src/backend/cuda/ssm_conv.cu"),
    ];
    for src_path in &src_paths {
        println!("cargo:rerun-if-changed={}", src_path.display());
    }
    rerun_if_changed_tree(&manifest_dir.join("src/backend/cuda/llm"));

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
    }
    println!("cargo:rustc-cfg=makepad_ggml_cuda_kernels");
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
