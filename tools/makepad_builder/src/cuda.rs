use std::fs;
use std::path::{Path, PathBuf};

use makepad_strict_json::{self as json, Value};

use crate::extract;

pub const CUDA_VERSION: &str = "13.2.2";
const REDIST: &str = "https://developer.download.nvidia.com/compute/cuda/redist";

const COMPONENTS: &[&str] = &[
    "cuda_nvcc",
    "libnvvm",
    "cuda_cccl",
    "cuda_crt",
    "cuda_cudart",
    "libcublas",
];

/// Availability of the private toolkit, independent of the app release defaults.
pub fn supported() -> bool {
    cfg!(all(windows, target_arch = "x86_64"))
}

/// An NVIDIA display driver is installed: it places nvcuda.dll in System32.
/// Only then is the optional toolkit offered.
pub fn gpu_present() -> bool {
    supported()
        && std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
            .join("System32/nvcuda.dll")
            .is_file()
}

/// Apps build and run with CUDA exactly when this machine has an NVIDIA
/// driver and the installation has its private toolkit: no switch. Never
/// with Rust's GNU toolchain: CUDA's compiler needs Microsoft's.
pub fn build_with(root: &Path) -> bool {
    crate::runtime::windows_chain(root) != crate::runtime::WindowsChain::Gnu
        && gpu_present() && root.join("toolchain/cuda/bin").join(crate::runtime::exe("nvcc")).is_file() && kernels_failed(root).is_none()
}

/// Recorded in the installation when the CUDA kernels failed to compile
/// here and the app was built without them: from then on every build in
/// this folder is CPU-only, so the flags (and the shared target) stay the
/// same. Deleting the file tries CUDA again.
pub const KERNELS_FAILED_FILE: &str = "cuda-kernels-failed";

/// Why the kernels were given up on, when they were.
pub fn kernels_failed(root: &Path) -> Option<String> {
    fs::read_to_string(root.join(KERNELS_FAILED_FILE)).ok().map(|text| text.lines().next().unwrap_or_default().to_owned())
}

pub fn install(cache: &Path, dest: &Path) -> Result<(), String> {
    let _timing = crate::timing::component("CUDA");
    crate::progress::package("NVIDIA CUDA", "Read package manifest", 0, 0);
    if dest.join("bin").join("nvcc.exe").is_file() {
        crate::progress::stage("Ready", "CUDA already installed", 1.0);
        return Ok(());
    }
    let manifest_url = format!("{REDIST}/redistrib_{CUDA_VERSION}.json");
    crate::setup_note!("cuda: {manifest_url}");
    let bytes = crate::fetch::bytes(&manifest_url)?;
    let doc = json::parse(&bytes).map_err(|e| format!("cuda json: {e}"))?;
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    // One bar for CUDA: every component's archive size is in the manifest.
    let mut parts = Vec::new();
    for name in COMPONENTS {
        let win = doc
            .get(name)
            .ok_or_else(|| format!("cuda manifest missing {name}"))?
            .get("windows-x86_64")
            .ok_or_else(|| format!("{name} has no windows-x86_64"))?;
        let rel = win.get("relative_path").and_then(Value::as_str).ok_or("relative_path")?;
        // NVIDIA writes sizes as strings.
        let size = win.get("size").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).unwrap_or(0);
        parts.push((*name, rel.to_string(), win.get("sha256").and_then(Value::as_str).map(str::to_owned), size));
    }
    let _total = crate::progress::total("CUDA", parts.iter().map(|p| p.3).sum());
    // Side by side, each straight into the toolkit without its archive's
    // top directory; a file in two archives ends as the later one's.
    let pending: Vec<_> = parts
        .into_iter()
        .enumerate()
        .map(|(order, (name, rel, sha, size))| {
            let url = format!("{REDIST}/{rel}");
            let file = rel.rsplit('/').next().unwrap_or(name).to_string();
            let dest = dest.to_path_buf();
            crate::fetch::file_then(cache, &url, &file, sha.as_deref(), size, move |zip| {
                crate::jobs::check_cancelled()?;
                crate::progress::package("CUDA", name, 0, 0);
                extract::unzip_parallel(zip, &dest, extract::Strip::TopDir, order as u64, size)?;
                Ok(())
            })
        })
        .collect();
    crate::jobs::wait_all(pending)?;
    if !dest.join("bin").join("nvcc.exe").is_file() {
        return Err("nvcc.exe missing after cuda extract".into());
    }
    crate::progress::stage("Ready", "CUDA installed", 1.0);
    Ok(())
}

/// Copy the CUDA runtime DLLs VJ links (`cudart` / `cublas` / `cublasLt` plus
/// the nvJitLink/nvrtc helpers they load) from a local NVIDIA toolkit into
/// our private `cuda\bin`. Used when the person already has CUDA on the
/// machine — we still do not put Program Files on PATH.
pub fn harvest_runtime(dest: &Path) -> Result<(), String> {
    let bin = dest.join("bin");
    if bin.join("cudart64_13.dll").is_file() || bin.join("cudart64_12.dll").is_file() {
        crate::setup_note!("cuda: runtime already at {}", bin.display());
        return Ok(());
    }
    let src_bin = system_cuda_bin().ok_or_else(|| {
        "no local NVIDIA CUDA toolkit (and no private nvcc). Accept NVIDIA and retry, or install CUDA."
            .to_string()
    })?;
    fs::create_dir_all(&bin).map_err(|e| e.to_string())?;
    let mut copied = 0usize;
    let rd = fs::read_dir(&src_bin).map_err(|e| e.to_string())?;
    for ent in rd.flatten() {
        let name = ent.file_name();
        let name = name.to_string_lossy();
        let lower = name.to_ascii_lowercase();
        if !(lower.ends_with(".dll")
            && (lower.starts_with("cudart")
                || lower.starts_with("cublas")
                || lower.starts_with("nvjitlink")
                || lower.starts_with("nvrtc")))
        {
            continue;
        }
        let to = bin.join(ent.file_name());
        fs::copy(ent.path(), &to).map_err(|e| format!("copy {name}: {e}"))?;
        copied += 1;
    }
    if copied == 0 {
        return Err(format!("no CUDA runtime DLLs in {}", src_bin.display()));
    }
    crate::setup_note!(
        "cuda: harvested {copied} runtime DLLs from {} into {}",
        src_bin.display(),
        bin.display()
    );
    Ok(())
}

fn system_cuda_bin() -> Option<PathBuf> {
    let pf = std::env::var_os("ProgramFiles").map(PathBuf::from)?;
    let root = pf.join("NVIDIA GPU Computing Toolkit").join("CUDA");
    let mut best: Option<(u32, u32, PathBuf)> = None;
    let rd = fs::read_dir(&root).ok()?;
    for ent in rd.flatten() {
        let name = ent.file_name();
        let name = name.to_string_lossy();
        let digits = name.trim_start_matches(|c: char| !c.is_ascii_digit());
        let mut parts = digits.split('.');
        let major: u32 = parts.next().unwrap_or("0").parse().unwrap_or(0);
        let minor: u32 = parts
            .next()
            .unwrap_or("0")
            .trim_end_matches(|c: char| !c.is_ascii_digit())
            .parse()
            .unwrap_or(0);
        let bin = ent.path().join("bin");
        let bin = if bin.join("x64").is_dir() {
            bin.join("x64")
        } else {
            bin
        };
        if bin.is_dir() {
            match &best {
                Some(cur) if (major, minor) <= (cur.0, cur.1) => {}
                _ => best = Some((major, minor, bin)),
            }
        }
    }
    best.map(|(_, _, p)| p)
}
