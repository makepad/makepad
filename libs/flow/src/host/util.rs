use super::ServerError;
use std::io::Write;
use std::path::Path;

#[cfg(any(target_vendor = "apple", target_os = "freebsd", target_os = "openbsd"))]
fn fill_random(out: &mut [u8]) -> Result<(), ServerError> {
    unsafe extern "C" {
        fn arc4random_buf(buf: *mut std::ffi::c_void, len: usize);
    }
    // SAFETY: the pointer is valid and writable for exactly `out.len()` bytes.
    unsafe { arc4random_buf(out.as_mut_ptr().cast(), out.len()) };
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn fill_random(out: &mut [u8]) -> Result<(), ServerError> {
    unsafe extern "C" {
        fn getrandom(buf: *mut std::ffi::c_void, buflen: usize, flags: u32) -> isize;
    }
    let mut done = 0;
    while done < out.len() {
        // SAFETY: the remaining slice is live and writable for the supplied length.
        let n = unsafe { getrandom(out[done..].as_mut_ptr().cast(), out.len() - done, 0) };
        if n > 0 {
            done += n as usize;
        } else if n == -1 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        } else {
            return Err(ServerError::Io {
                op: "getrandom",
                kind: std::io::Error::last_os_error().kind(),
            });
        }
    }
    Ok(())
}

#[cfg(windows)]
fn fill_random(out: &mut [u8]) -> Result<(), ServerError> {
    const USE_SYSTEM_PREFERRED_RNG: u32 = 2;
    #[link(name = "bcrypt")]
    unsafe extern "system" {
        fn BCryptGenRandom(
            algorithm: *mut std::ffi::c_void,
            buffer: *mut u8,
            count: u32,
            flags: u32,
        ) -> i32;
    }
    for chunk in out.chunks_mut(u32::MAX as usize) {
        // SAFETY: CNG writes at most `count` bytes into this live slice.
        let status = unsafe {
            BCryptGenRandom(
                std::ptr::null_mut(),
                chunk.as_mut_ptr(),
                chunk.len() as u32,
                USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if status != 0 {
            return Err(ServerError::Io {
                op: "BCryptGenRandom",
                kind: std::io::ErrorKind::Other,
            });
        }
    }
    Ok(())
}

#[cfg(not(any(
    target_vendor = "apple",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "linux",
    target_os = "android",
    windows
)))]
compile_error!("flow-server needs a supported operating-system random source");

pub(crate) fn random_16() -> Result<[u8; 16], ServerError> {
    let mut bytes = [0; 16];
    fill_random(&mut bytes)?;
    Ok(bytes)
}

pub(crate) fn random_32() -> Result<[u8; 32], ServerError> {
    let mut bytes = [0; 32];
    fill_random(&mut bytes)?;
    Ok(bytes)
}

pub(crate) fn random_u64() -> Result<u64, ServerError> {
    let mut bytes = [0; 8];
    fill_random(&mut bytes)?;
    // Keep the JSON representation inside the signed-64 range accepted by
    // the project's strict response parser while retaining 63 random bits.
    Ok(u64::from_le_bytes(bytes) & i64::MAX as u64)
}

pub(crate) fn to_hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(text, "{byte:02x}");
    }
    text
}

pub(crate) fn from_hex_16(text: &str) -> Option<[u8; 16]> {
    if text.len() != 32 || !text.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()) {
        return None;
    }
    let mut out = [0; 16];
    for (index, pair) in text.as_bytes().chunks_exact(2).enumerate() {
        out[index] = (hex_digit(pair[0])? << 4) | hex_digit(pair[1])?;
    }
    Some(out)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

pub(crate) fn write_secret_file(path: &Path, secret: &str) -> Result<(), ServerError> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| ServerError::io("create token", error))?;
    file.write_all(format!("{secret}\n").as_bytes())
        .map_err(|error| ServerError::io("write token", error))?;
    file.sync_all().map_err(|error| ServerError::io("sync token", error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| ServerError::io("chmod token", error))?;
    }
    Ok(())
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8], serial: u64) -> Result<(), ServerError> {
    let file_name = path.file_name().and_then(|name| name.to_str()).unwrap_or("flow");
    let mut last_collision = None;
    for attempt in 0..16 {
        let temp = path.with_file_name(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            serial.wrapping_add(attempt)
        ));
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                last_collision = Some(error);
                continue;
            }
            Err(error) => return Err(ServerError::io("create temporary file", error)),
        };
        let result = (|| {
            file.write_all(bytes)
                .map_err(|error| ServerError::io("write temporary file", error))?;
            file.sync_all()
                .map_err(|error| ServerError::io("sync temporary file", error))?;
            std::fs::rename(&temp, path)
                .map_err(|error| ServerError::io("rename temporary file", error))
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        return result;
    }
    Err(ServerError::io(
        "create temporary file",
        last_collision.unwrap_or_else(|| std::io::Error::from(std::io::ErrorKind::AlreadyExists)),
    ))
}

pub(crate) fn log(config: &super::config::FlowServerConfig, message: &str) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (config.log)(message)));
}
