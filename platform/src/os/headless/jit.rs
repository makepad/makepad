use std::ffi::{c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct HeadlessShaderJit {
    root_dir: PathBuf,
}

pub struct HeadlessJitOutput {
    pub dylib_path: PathBuf,
    pub module: Option<HeadlessLoadedModule>,
    pub shader_version: Option<u32>,
    pub load_error: Option<String>,
}

impl Default for HeadlessShaderJit {
    fn default() -> Self {
        let root_dir = std::env::var("MAKEPAD_HEADLESS_JIT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("local/headless/jit"));
        Self { root_dir }
    }
}

impl HeadlessShaderJit {
    pub fn compile_and_load(
        &self,
        source_hash: u64,
        source: &str,
    ) -> Result<HeadlessJitOutput, String> {
        let shader_dir = self.root_dir.join(format!("shader_{source_hash:016x}"));
        std::fs::create_dir_all(&shader_dir).map_err(|err| {
            format!(
                "failed to create headless shader output dir `{}`: {err}",
                shader_dir.display()
            )
        })?;

        let source_path = shader_dir.join("lib.rs");
        std::fs::write(&source_path, source).map_err(|err| {
            format!(
                "failed to write generated headless shader source `{}`: {err}",
                source_path.display()
            )
        })?;

        let dylib_path =
            shader_dir.join(format!("shader_{source_hash:016x}.{}", dylib_extension()));

        let crate_name = format!("makepad_headless_shader_{source_hash:016x}");
        let output = Command::new("rustc")
            .arg("--edition=2021")
            .arg("--crate-type")
            .arg("cdylib")
            .arg("--crate-name")
            .arg(&crate_name)
            .arg("-O")
            .arg(&source_path)
            .arg("-o")
            .arg(&dylib_path)
            .output()
            .map_err(|err| {
                format!("failed to run rustc for headless shader JIT `{crate_name}`: {err}")
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "headless shader JIT compile failed for `{}`:\n{}",
                dylib_path.display(),
                stderr.trim()
            ));
        }

        let mut load_error = None;
        let mut shader_version = None;
        let mut module = None;

        match HeadlessLoadedModule::load(&dylib_path) {
            Ok(loaded) => {
                shader_version = loaded.shader_version().ok();
                module = Some(loaded);
            }
            Err(err) => {
                load_error = Some(err);
            }
        }

        Ok(HeadlessJitOutput {
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
pub struct HeadlessLoadedModule {
    handle: std::ptr::NonNull<c_void>,
}

#[cfg(target_os = "macos")]
impl HeadlessLoadedModule {
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
        let version_fn: VersionFn = self.symbol("makepad_headless_shader_version")?;
        Ok(unsafe { version_fn() })
    }

    pub fn symbol<F: Sized>(&self, symbol: &str) -> Result<F, String> {
        let name = CString::new(symbol).map_err(|_| format!("invalid symbol name `{symbol}`"))?;
        let ptr = unsafe { dlsym(self.handle.as_ptr(), name.as_ptr()) };
        if ptr.is_null() {
            return Err(format!(
                "symbol `{symbol}` missing in headless shader module: {}",
                last_dlerror()
            ));
        }
        Ok(unsafe { std::mem::transmute_copy::<*mut c_void, F>(&ptr) })
    }
}

#[cfg(target_os = "macos")]
impl Drop for HeadlessLoadedModule {
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
pub struct HeadlessLoadedModule;

#[cfg(not(target_os = "macos"))]
impl HeadlessLoadedModule {
    pub fn load(path: &Path) -> Result<Self, String> {
        Err(format!(
            "headless shader dlopen is only implemented on macOS for now (`{}`)",
            path.display()
        ))
    }

    pub fn shader_version(&self) -> Result<u32, String> {
        Err("headless shader version lookup is only implemented on macOS for now".to_string())
    }
}
