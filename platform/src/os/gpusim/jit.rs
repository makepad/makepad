use std::ffi::{c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct GpusimShaderJit {
    root_dir: PathBuf,
}

pub struct GpusimJitOutput {
    pub dylib_path: PathBuf,
    pub module: Option<GpusimLoadedModule>,
    pub shader_version: Option<u32>,
    pub load_error: Option<String>,
}

impl Default for GpusimShaderJit {
    fn default() -> Self {
        let root_dir = std::env::var("MAKEPAD_GPUSIM_JIT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| default_jit_root_dir());
        Self { root_dir }
    }
}

fn default_jit_root_dir() -> PathBuf {
    if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
        return PathBuf::from(target_dir).join("makepad-gpusim-jit");
    }
    if let Ok(cwd) = std::env::current_dir() {
        return cwd.join("target").join("makepad-gpusim-jit");
    }
    PathBuf::from("target/makepad-gpusim-jit")
}

impl GpusimShaderJit {
    pub fn compile_and_load(
        &self,
        source_hash: u64,
        source: &str,
    ) -> Result<GpusimJitOutput, String> {
        let shader_dir = self.root_dir.join(format!("shader_{source_hash:016x}"));
        let cached_path =
            shader_dir.join(format!("shader_{source_hash:016x}.{}", dylib_extension()));

        // The dylib is content-addressed by the hash of the source that made it,
        // so one left by an earlier run is exactly what rustc would produce now.
        // Reuse it: this compiles EVERY shader with `rustc -O` at startup, which
        // costs tens of seconds per process — and a gpusim test suite starts a
        // fresh process for every test. Anything unloadable (truncated by a killed
        // run, built by a different toolchain) just falls through and recompiles.
        if cached_path.is_file() {
            if let Ok(loaded) = GpusimLoadedModule::load(&cached_path) {
                if let Ok(version) = loaded.shader_version() {
                    return Ok(GpusimJitOutput {
                        dylib_path: cached_path,
                        module: Some(loaded),
                        shader_version: Some(version),
                        load_error: None,
                    });
                }
            }
        }

        std::fs::create_dir_all(&shader_dir).map_err(|err| {
            format!(
                "failed to create gpusim shader output dir `{}`: {err}",
                shader_dir.display()
            )
        })?;

        let source_path = shader_dir.join("lib.rs");
        std::fs::write(&source_path, source).map_err(|err| {
            format!(
                "failed to write generated gpusim shader source `{}`: {err}",
                source_path.display()
            )
        })?;

        let dylib_path = cached_path;
        // Compile to a private path and rename into place, so a crashed or
        // killed run can never leave a half-written dylib for the next one to
        // find (and so two processes building the same shader can't interleave).
        let staging_path = shader_dir.join(format!(
            "shader_{source_hash:016x}.{}.{}",
            std::process::id(),
            dylib_extension()
        ));

        let crate_name = format!("makepad_gpusim_shader_{source_hash:016x}");
        let output = Command::new("rustc")
            .arg("--edition=2021")
            .arg("--crate-type")
            .arg("cdylib")
            .arg("--crate-name")
            .arg(&crate_name)
            .arg("-O")
            .arg(&source_path)
            .arg("-o")
            .arg(&staging_path)
            .output()
            .map_err(|err| {
                format!("failed to run rustc for gpusim shader JIT `{crate_name}`: {err}")
            })?;

        if !output.status.success() {
            let _ = std::fs::remove_file(&staging_path);
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "gpusim shader JIT compile failed for `{}`:\n{}",
                dylib_path.display(),
                stderr.trim()
            ));
        }

        std::fs::rename(&staging_path, &dylib_path).map_err(|err| {
            format!(
                "failed to publish gpusim shader dylib `{}`: {err}",
                dylib_path.display()
            )
        })?;

        let mut load_error = None;
        let mut shader_version = None;
        let mut module = None;

        match GpusimLoadedModule::load(&dylib_path) {
            Ok(loaded) => {
                shader_version = loaded.shader_version().ok();
                module = Some(loaded);
            }
            Err(err) => {
                load_error = Some(err);
            }
        }

        Ok(GpusimJitOutput {
            dylib_path,
            module,
            shader_version,
            load_error,
        })
    }
}

fn dylib_extension() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        return "dll";
    }
    #[cfg(target_os = "macos")]
    {
        return "dylib";
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        return "so";
    }
    #[allow(unreachable_code)]
    "bin"
}

#[cfg(target_os = "macos")]
pub struct GpusimLoadedModule {
    handle: std::ptr::NonNull<c_void>,
}

#[cfg(target_os = "macos")]
impl GpusimLoadedModule {
    pub fn load(path: &Path) -> Result<Self, String> {
        const RTLD_NOW: i32 = 2;
        let c_path = CString::new(path.to_string_lossy().as_bytes())
            .map_err(|_| format!("invalid dylib path `{}`", path.display()))?;
        let handle = unsafe { dlopen(c_path.as_ptr(), RTLD_NOW) };
        let handle = std::ptr::NonNull::new(handle).ok_or_else(last_dlerror)?;
        Ok(Self { handle })
    }

    pub fn shader_version(&self) -> Result<u32, String> {
        type VersionFn = unsafe extern "C" fn() -> u32;
        let version_fn: VersionFn = self.symbol("makepad_gpusim_shader_version")?;
        Ok(unsafe { version_fn() })
    }

    pub fn symbol<F: Sized>(&self, symbol: &str) -> Result<F, String> {
        let name = CString::new(symbol).map_err(|_| format!("invalid symbol name `{symbol}`"))?;
        let ptr = unsafe { dlsym(self.handle.as_ptr(), name.as_ptr()) };
        if ptr.is_null() {
            return Err(format!(
                "symbol `{symbol}` missing in gpusim shader module: {}",
                last_dlerror()
            ));
        }
        Ok(unsafe { std::mem::transmute_copy::<*mut c_void, F>(&ptr) })
    }
}

#[cfg(target_os = "macos")]
impl Drop for GpusimLoadedModule {
    fn drop(&mut self) {
        unsafe {
            dlclose(self.handle.as_ptr());
        }
    }
}

#[cfg(target_os = "macos")]
fn last_dlerror() -> String {
    let err = unsafe { dlerror() };
    if err.is_null() {
        return "unknown dlopen/dlsym error".to_string();
    }
    unsafe { CStr::from_ptr(err) }
        .to_string_lossy()
        .into_owned()
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn dlopen(path: *const std::os::raw::c_char, mode: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const std::os::raw::c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> i32;
    fn dlerror() -> *const std::os::raw::c_char;
}

#[cfg(not(target_os = "macos"))]
pub struct GpusimLoadedModule;

#[cfg(not(target_os = "macos"))]
impl GpusimLoadedModule {
    pub fn load(path: &Path) -> Result<Self, String> {
        Err(format!(
            "gpusim shader dlopen is only implemented on macOS for now (`{}`)",
            path.display()
        ))
    }

    pub fn shader_version(&self) -> Result<u32, String> {
        Err("gpusim shader version lookup is only implemented on macOS for now".to_string())
    }
}
