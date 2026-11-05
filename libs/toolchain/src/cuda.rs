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

/// Recorded in the installation when the CUDA kernels failed to compile
/// here and the app was built without them: from then on every build in
/// this folder is CPU-only, so the flags (and the shared target) stay the
/// same. Deleting the file tries CUDA again.
pub const KERNELS_FAILED_FILE: &str = "cuda-kernels-failed";

/// Why the kernels were given up on, when they were.
pub fn kernels_failed(root: &Path) -> Option<String> {
    fs::read_to_string(root.join(KERNELS_FAILED_FILE)).ok().map(|text| text.lines().next().unwrap_or_default().to_owned())
}

/// Written once all of the toolkit is unpacked: a stopped install leaves the
/// files it got to, nvcc perhaps among them.
const READY: &str = ".ready";

pub fn ready(dest: &Path) -> bool {
    dest.join(READY).is_file()
}

/// A failed check must not make the next attempt reuse a toolkit it found broken.
pub(crate) fn invalidate(dest: &Path) -> Result<(), String> {
    crate::remove_marker(&dest.join(READY))
}

/// A component's archive in the manifest: its path, SHA-256 and size. One with no hash is refused:
/// nothing unverified is unpacked or run.
fn component(doc: &Value, name: &str) -> Result<(String, String, u64), String> {
    let win = doc
        .get(name)
        .ok_or_else(|| format!("cuda manifest missing {name}"))?
        .get("windows-x86_64")
        .ok_or_else(|| format!("{name} has no windows-x86_64"))?;
    let rel = win.get("relative_path").and_then(Value::as_str).ok_or("relative_path")?;
    let sha = win.get("sha256").and_then(Value::as_str).ok_or_else(|| format!("cuda manifest: {name} has no sha256"))?;
    // NVIDIA writes sizes as strings.
    let size = win.get("size").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).unwrap_or(0);
    Ok((rel.to_string(), sha.to_string(), size))
}

pub fn install(cache: &Path, dest: &Path) -> Result<(), String> {
    let _timing = crate::timing::component("CUDA");
    crate::progress::package("NVIDIA CUDA", "Read package manifest", 0, 0);
    if ready(dest) {
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
        let (rel, sha, size) = component(&doc, name)?;
        parts.push((*name, rel, sha, size));
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
            crate::fetch::file_then(cache, &url, &file, &sha, size, move |zip| {
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
    fs::write(dest.join(READY), "").map_err(|e| e.to_string())?;
    crate::progress::stage("Ready", "CUDA installed", 1.0);
    Ok(())
}

#[cfg(test)]
mod hash_tests {
    use super::*;

    #[test]
    fn a_component_without_a_hash_is_refused_by_name() {
        let doc = json::parse(br#"{"nvcc":{"windows-x86_64":{"relative_path":"a.zip","size":"1"}}}"#).unwrap();
        let error = component(&doc, "nvcc").unwrap_err();
        assert!(error.contains("nvcc") && error.contains("sha256"), "{error}");
        let doc = json::parse(br#"{"nvcc":{"windows-x86_64":{"relative_path":"a.zip","size":"1","sha256":"ab"}}}"#).unwrap();
        assert_eq!(component(&doc, "nvcc").unwrap(), ("a.zip".to_string(), "ab".to_string(), 1));
    }
}
