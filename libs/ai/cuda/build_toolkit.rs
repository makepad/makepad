// Which CUDA toolkit a `cuda` build of makepad-ai-cuda compiles its kernels
// with, or that it compiles none. build.rs acts on the answer; the crate's
// tests include this file too and pin the answer down, because a build script
// has no tests of its own.
//
// A named toolkit is a request for CUDA. MAKEPAD_CUDA_ROOT that is set but
// does not hold a toolkit is an error, never a quiet stub build: a typo'd or
// mis-split root (`MAKEPAD_CUDA_ROOT="/opt/cuda MAKEPAD_GGML_REQUIRE_CUDA=1"`,
// one shell word for two variables) used to build zero kernels with nothing
// in the log, and every CUDA test and model behind it silently dropped out.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

/// A toolkit root holding both the compiler and the libs we link against.
pub struct Toolkit {
    pub root: PathBuf,
    pub nvcc: PathBuf,
    pub lib_dir: PathBuf,
}

/// The toolkit `root` (MAKEPAD_CUDA_ROOT) names. `None` when it names none
/// (unset or empty): the normal case of a machine without an NVIDIA GPU,
/// unless `require` (MAKEPAD_GGML_REQUIRE_CUDA) says this box must have CUDA.
/// A named root that is not a usable toolkit is an error.
pub fn named_toolkit(root: Option<&OsStr>, require: bool, target_os: &str) -> Result<Option<Toolkit>, String> {
    match root.filter(|root| !root.is_empty()) {
        None if require => Err("MAKEPAD_GGML_REQUIRE_CUDA=1, but MAKEPAD_CUDA_ROOT names no toolkit".to_string()),
        None => Ok(None),
        Some(root) => probe(Path::new(root), target_os)
            .map(Some)
            .map_err(|why| format!("MAKEPAD_CUDA_ROOT={root:?} is not a usable CUDA toolkit: {why}")),
    }
}

/// Both the compiler and the libs, or why not. A bare root left behind by an
/// uninstall, or a driver-only install, has one without the other.
fn probe(root: &Path, target_os: &str) -> Result<Toolkit, String> {
    let nvcc = nvcc_path(root, target_os);
    if !nvcc.is_file() {
        return Err(format!("no nvcc at {}", nvcc.display()));
    }
    let lib_dir = cuda_lib_dir(root, target_os).ok_or_else(|| {
        format!(
            "no CUDA link libraries (looked in {})",
            lib_dir_candidates(root, target_os)
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;
    Ok(Toolkit { root: root.to_path_buf(), nvcc, lib_dir })
}

fn nvcc_path(root: &Path, target_os: &str) -> PathBuf {
    root.join("bin").join(if target_os == "windows" { "nvcc.exe" } else { "nvcc" })
}

/// Where a toolkit of this version keeps the libs we link against. Ordered
/// most-specific first; every CUDA major version so far uses one of these,
/// and an unknown future layout reads as an unusable toolkit, a build error
/// naming where it looked, instead of a link failure.
fn lib_dir_candidates(root: &Path, target_os: &str) -> Vec<PathBuf> {
    if target_os == "windows" {
        vec![root.join("lib").join("x64"), root.join("lib")]
    } else {
        vec![
            root.join("lib64"),
            root.join("targets").join("x86_64-linux").join("lib"),
            root.join("lib"),
        ]
    }
}

fn cuda_lib_dir(root: &Path, target_os: &str) -> Option<PathBuf> {
    let prefix = if target_os == "windows" { "cudart.lib" } else { "libcudart.so" };
    lib_dir_candidates(root, target_os)
        .into_iter()
        .find(|dir| dir_has_file_starting_with(dir, prefix))
}

pub fn dir_has_file_starting_with(dir: &Path, prefix: &str) -> bool {
    fs::read_dir(dir).is_ok_and(|entries| {
        entries
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().starts_with(prefix))
    })
}

#[cfg(test)]
mod tests {
    use super::named_toolkit;
    use std::ffi::OsStr;
    use std::fs;
    use std::path::PathBuf;

    /// A directory laid out as a Linux toolkit (an nvcc and a libcudart),
    /// removed when dropped.
    struct FakeToolkit(PathBuf);

    impl FakeToolkit {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!("makepad-ai-cuda-toolkit-{tag}-{}", std::process::id()));
            fs::create_dir_all(root.join("bin")).unwrap();
            fs::create_dir_all(root.join("lib64")).unwrap();
            fs::write(root.join("bin").join("nvcc"), b"").unwrap();
            fs::write(root.join("lib64").join("libcudart.so.12"), b"").unwrap();
            Self(root)
        }
    }

    impl Drop for FakeToolkit {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_named_root_that_is_not_a_toolkit_fails_the_build() {
        let real = FakeToolkit::new("named");
        // Two variables passed as one shell word: the root is the whole string.
        let mis_split = format!("{} MAKEPAD_GGML_REQUIRE_CUDA=1", real.0.display());
        let missing = real.0.join("not-here");
        let without_libs = FakeToolkit::new("nolibs");
        fs::remove_dir_all(without_libs.0.join("lib64")).unwrap();
        for (what, root, why) in [
            ("mis-split", OsStr::new(&mis_split), "no nvcc"),
            ("missing", missing.as_os_str(), "no nvcc"),
            ("without libs", without_libs.0.as_os_str(), "no CUDA link libraries"),
        ] {
            for require in [false, true] {
                match named_toolkit(Some(root), require, "linux") {
                    Err(error) => assert!(
                        error.contains("is not a usable CUDA toolkit") && error.contains(why),
                        "{what}: {error}"
                    ),
                    Ok(found) => panic!("{what} (require {require}): accepted, toolkit {:?}", found.map(|t| t.root)),
                }
            }
        }
    }

    #[test]
    fn a_usable_root_is_the_toolkit_and_no_root_is_no_toolkit() {
        let real = FakeToolkit::new("usable");
        let toolkit = named_toolkit(Some(real.0.as_os_str()), false, "linux")
            .expect("a usable root")
            .expect("a toolkit");
        assert_eq!(toolkit.root, real.0);
        assert_eq!(toolkit.nvcc, real.0.join("bin").join("nvcc"));
        assert_eq!(toolkit.lib_dir, real.0.join("lib64"));
        // Unset and empty (the toolchain's CPU builds set it empty) name none.
        for root in [None, Some(OsStr::new(""))] {
            assert!(matches!(named_toolkit(root, false, "linux"), Ok(None)), "{root:?}");
            assert!(named_toolkit(root, true, "linux").is_err(), "{root:?} with require");
        }
    }
}
