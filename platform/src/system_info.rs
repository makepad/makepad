//! What the machine is, in words a person can read: the operating system
//! and its version, the physical memory, and the audio stack the process
//! plays through. For about boxes, bug reports and feedback forms; nothing
//! here is used to make decisions (the GPU is `Cx::gpu_info` /
//! `Cx::gpu_backend`).

/// The operating system's name and version, e.g. "macOS 15.5 (24F74)",
/// "Windows 11 (10.0.26100)" or "Ubuntu 24.04.2 LTS (Linux 6.8.0-60-generic)".
/// Falls back to the family name when the version cannot be read.
pub fn os_name_and_version() -> String {
    os_version().unwrap_or_else(|| std::env::consts::OS.to_string())
}

static GPU_ADAPTER: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Called once by the GPU backend with the adapter it rendered with; the
/// first name sticks (a device lost and recreated is the same adapter).
#[allow(dead_code)] // not every backend names its adapter (gpusim, web)
pub(crate) fn note_gpu_adapter(name: &str) {
    let name = name.trim();
    if !name.is_empty() {
        let _ = GPU_ADAPTER.set(name.to_string());
    }
}

/// The GPU adapter the process renders with, as its driver names it
/// ("Apple M3 Max", "NVIDIA GeForce RTX 4070", "Mesa Intel(R) UHD Graphics
/// 620 (KBL GT2)"); known once the backend has created its device.
/// The API is `Cx::gpu_backend`.
pub fn gpu_adapter_name() -> Option<String> {
    GPU_ADAPTER.get().cloned()
}

/// Total physical memory in bytes, when the platform reports it.
pub fn physical_memory_bytes() -> Option<u64> {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        crate::cx::apple_physical_memory_bytes()
    }
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    {
        crate::cx::linux_physical_memory_bytes()
    }
    #[cfg(target_os = "windows")]
    {
        crate::cx::windows_physical_memory_bytes()
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        all(target_os = "linux", not(target_env = "ohos")),
        target_os = "windows"
    )))]
    {
        None
    }
}

#[cfg(target_vendor = "apple")]
fn sysctl_string(name: &str) -> Option<String> {
    unsafe extern "C" {
        fn sysctlbyname(
            name: *const std::ffi::c_char,
            old: *mut std::ffi::c_void,
            size: *mut usize,
            new: *mut std::ffi::c_void,
            new_size: usize,
        ) -> i32;
    }
    let name = std::ffi::CString::new(name).ok()?;
    let mut buf = [0u8; 256];
    let mut size = buf.len();
    let result = unsafe {
        sysctlbyname(
            name.as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 || size == 0 {
        return None;
    }
    let text = &buf[..size.min(buf.len())];
    let end = text.iter().position(|b| *b == 0).unwrap_or(text.len());
    let text = String::from_utf8_lossy(&text[..end]).trim().to_string();
    (!text.is_empty()).then_some(text)
}

#[cfg(target_vendor = "apple")]
fn os_version() -> Option<String> {
    let name = if cfg!(target_os = "ios") {
        "iOS"
    } else if cfg!(target_os = "tvos") {
        "tvOS"
    } else {
        "macOS"
    };
    let version = sysctl_string("kern.osproductversion")?;
    Some(match sysctl_string("kern.osversion") {
        Some(build) => format!("{name} {version} ({build})"),
        None => format!("{name} {version}"),
    })
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
fn os_version() -> Option<String> {
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let pretty = ["/etc/os-release", "/usr/lib/os-release"]
        .iter()
        .find_map(|path| std::fs::read_to_string(path).ok())
        .and_then(|release| {
            release.lines().find_map(|line| {
                let value = line.strip_prefix("PRETTY_NAME=")?;
                let value = value.trim().trim_matches('"').trim_matches('\'').trim();
                (!value.is_empty()).then(|| value.to_string())
            })
        });
    match (pretty, kernel) {
        (Some(pretty), Some(kernel)) => Some(format!("{pretty} (Linux {kernel})")),
        (Some(pretty), None) => Some(pretty),
        (None, Some(kernel)) => Some(format!("Linux {kernel}")),
        (None, None) => None,
    }
}

#[cfg(target_os = "windows")]
fn os_version() -> Option<String> {
    #[allow(non_snake_case)]
    #[repr(C)]
    struct OsVersionInfoW {
        dwOSVersionInfoSize: u32,
        dwMajorVersion: u32,
        dwMinorVersion: u32,
        dwBuildNumber: u32,
        dwPlatformId: u32,
        szCSDVersion: [u16; 128],
    }
    // RtlGetVersion reports the real version; GetVersionEx is capped at
    // what the executable's manifest declares.
    windows_core::link!("ntdll.dll" "system" fn RtlGetVersion(info: *mut OsVersionInfoW) -> i32);
    let mut info = OsVersionInfoW {
        dwOSVersionInfoSize: std::mem::size_of::<OsVersionInfoW>() as u32,
        dwMajorVersion: 0,
        dwMinorVersion: 0,
        dwBuildNumber: 0,
        dwPlatformId: 0,
        szCSDVersion: [0; 128],
    };
    if unsafe { RtlGetVersion(&mut info) } != 0 || info.dwMajorVersion == 0 {
        return None;
    }
    // Windows 11 still reports 10.0; its builds start at 22000.
    let name = match (info.dwMajorVersion, info.dwBuildNumber) {
        (10, build) if build >= 22000 => "Windows 11",
        (10, _) => "Windows 10",
        _ => "Windows",
    };
    Some(format!(
        "{name} ({}.{}.{})",
        info.dwMajorVersion, info.dwMinorVersion, info.dwBuildNumber
    ))
}

#[cfg(not(any(
    target_vendor = "apple",
    all(target_os = "linux", not(target_env = "ohos")),
    target_os = "windows"
)))]
fn os_version() -> Option<String> {
    None
}

// Desktop Linux reports what its media layer actually connected to
// (os/linux/linux_media.rs); every other platform has one audio stack.
#[cfg(not(all(target_os = "linux", not(target_env = "ohos"), not(gpusim))))]
impl crate::cx::Cx {
    /// The audio stack this process plays through, e.g. "CoreAudio",
    /// "WASAPI", "PipeWire 1.0.5 (PulseAudio API)" or "ALSA".
    pub fn audio_stack(&self) -> String {
        let stack = if cfg!(target_vendor = "apple") {
            "CoreAudio"
        } else if cfg!(target_os = "windows") {
            "WASAPI"
        } else if cfg!(target_os = "android") {
            "AAudio"
        } else if cfg!(target_env = "ohos") {
            "OHAudio"
        } else if cfg!(target_arch = "wasm32") {
            "Web Audio"
        } else {
            "unknown"
        };
        stack.to_string()
    }
}
