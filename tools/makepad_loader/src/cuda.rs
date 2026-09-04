use std::fs;
use std::path::{Path, PathBuf};

use makepad_strict_json::{self as json, Value};

use crate::extract;
use crate::http;

const CUDA_VERSION: &str = "13.2.2";
const REDIST: &str = "https://developer.download.nvidia.com/compute/cuda/redist";

const COMPONENTS: &[&str] = &[
    "cuda_nvcc",
    "libnvvm",
    "cuda_cccl",
    "cuda_crt",
    "cuda_cudart",
    "libcublas",
];

pub fn install(cache: &Path, dest: &Path) -> Result<(), String> {
    if dest.join("bin").join("nvcc.exe").is_file() {
        println!("cuda: already extracted at {}", dest.display());
        return Ok(());
    }
    let manifest_url = format!("{REDIST}/redistrib_{CUDA_VERSION}.json");
    println!("cuda: {manifest_url}");
    let bytes = http::fetch_bytes(&manifest_url)?;
    let doc = json::parse(&bytes).map_err(|e| format!("cuda json: {e}"))?;
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    for name in COMPONENTS {
        let comp = doc
            .get(name)
            .ok_or_else(|| format!("cuda manifest missing {name}"))?;
        let win = comp
            .get("windows-x86_64")
            .ok_or_else(|| format!("{name} has no windows-x86_64"))?;
        let rel = win
            .get("relative_path")
            .and_then(Value::as_str)
            .ok_or("relative_path")?;
        let sha = win.get("sha256").and_then(Value::as_str);
        let url = format!("{REDIST}/{rel}");
        let file = rel.rsplit('/').next().unwrap_or(name);
        println!("cuda: {name}");
        let zip = http::cached_file(cache, &url, file, sha)?;
        let tmp = dest.join(format!(".unpack-{name}"));
        let _ = fs::remove_dir_all(&tmp);
        extract::unzip_file(&zip, &tmp, None)?;
        let inner = extract::single_child_dir(&tmp).unwrap_or(tmp.clone());
        extract::merge_dir(&inner, dest)?;
        let _ = fs::remove_dir_all(&tmp);
    }
    if !dest.join("bin").join("nvcc.exe").is_file() {
        return Err("nvcc.exe missing after cuda extract".into());
    }
    println!("cuda: ready at {}", dest.display());
    Ok(())
}

/// Copy the CUDA runtime DLLs VJ links (`cudart` / `cublas` / `cublasLt` plus
/// the nvJitLink/nvrtc helpers they load) from a local NVIDIA toolkit into
/// our private `cuda\bin`. Used when the person already has CUDA on the
/// machine — we still do not put Program Files on PATH.
pub fn harvest_runtime(dest: &Path) -> Result<(), String> {
    let bin = dest.join("bin");
    if bin.join("cudart64_13.dll").is_file() || bin.join("cudart64_12.dll").is_file() {
        println!("cuda: runtime already at {}", bin.display());
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
    println!(
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
