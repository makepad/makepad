//! Small transport-side primitives: real clock, OS randomness (fail closed),
//! hex, and the stderr logger. The core never touches any of these; the
//! transport is the only place time and randomness enter the system.

use crate::{ServerError, ServerResult};

/// Wall-clock milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Fill `out` with OS randomness. Fails closed on every platform: no fallback
/// generator exists, so token and ID minting refuse rather than degrade.
///
/// Unix reads `/dev/urandom`. Windows has no such device — asking for it there
/// returned `NotFound`, which is what stopped the embedded store dead the
/// first time it got far enough to mint a server id — so it calls
/// `BCryptGenRandom` with the system-preferred RNG, which is the same
/// CNG generator Rust's own `std` randomness sits on.
#[cfg(not(windows))]
pub fn fill_random(out: &mut [u8]) -> ServerResult<()> {
    use std::io::Read;
    let mut f = std::fs::File::open("/dev/urandom")
        .map_err(|e| ServerError::Io { op: "open urandom", kind: e.kind() })?;
    f.read_exact(out)
        .map_err(|e| ServerError::Io { op: "read urandom", kind: e.kind() })
}

#[cfg(windows)]
pub fn fill_random(out: &mut [u8]) -> ServerResult<()> {
    /// `BCRYPT_USE_SYSTEM_PREFERRED_RNG`: no algorithm handle needed.
    const USE_SYSTEM_PREFERRED_RNG: u32 = 0x0000_0002;

    #[link(name = "bcrypt")]
    extern "system" {
        fn BCryptGenRandom(
            algorithm: *mut std::ffi::c_void,
            buffer: *mut u8,
            count: u32,
            flags: u32,
        ) -> i32;
    }

    if out.is_empty() {
        return Ok(());
    }
    // A single call is capped at u32 bytes; every caller here asks for 16 or
    // 32, but chunk anyway so the contract does not depend on that.
    for chunk in out.chunks_mut(u32::MAX as usize) {
        // Safety: `chunk` is a live, writable slice of exactly `len` bytes,
        // and CNG writes no more than that.
        let status = unsafe {
            BCryptGenRandom(
                std::ptr::null_mut(),
                chunk.as_mut_ptr(),
                chunk.len() as u32,
                USE_SYSTEM_PREFERRED_RNG,
            )
        };
        // NTSTATUS: anything but STATUS_SUCCESS is a refusal, and randomness
        // is the one thing this must never guess at.
        if status != 0 {
            return Err(ServerError::Io {
                op: "BCryptGenRandom",
                kind: std::io::ErrorKind::Other,
            });
        }
    }
    Ok(())
}

pub fn rand16() -> ServerResult<[u8; 16]> {
    let mut b = [0u8; 16];
    fill_random(&mut b)?;
    Ok(b)
}

pub fn rand32() -> ServerResult<[u8; 32]> {
    let mut b = [0u8; 32];
    fill_random(&mut b)?;
    Ok(b)
}

pub fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Strict lowercase-hex decode of exactly `N` bytes; anything else is None.
pub fn from_hex_exact<const N: usize>(s: &str) -> Option<[u8; N]> {
    let b = s.as_bytes();
    if b.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    for i in 0..N {
        let hi = hex_val(b[i * 2])?;
        let lo = hex_val(b[i * 2 + 1])?;
        out[i] = (hi << 4) | lo;
    }
    Some(out)
}

/// Strict lowercase-hex decode of a variable-length value, bounded by
/// `max_bytes` decoded bytes; empty, odd-length, over-bound or non-hex
/// input is None.
pub fn from_hex_bounded(s: &str, max_bytes: usize) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if b.is_empty() || b.len() % 2 != 0 || b.len() / 2 > max_bytes {
        return None;
    }
    let mut out = Vec::with_capacity(b.len() / 2);
    for pair in b.chunks_exact(2) {
        out.push((hex_val(pair[0])? << 4) | hex_val(pair[1])?);
    }
    Some(out)
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

/// Log one line to stderr with a millisecond timestamp. Callers must never
/// pass secrets, request headers, or body content.
pub fn log(enabled: bool, msg: &str) {
    if enabled {
        eprintln!("[asset-server {}] {}", now_ms(), msg);
    }
}
