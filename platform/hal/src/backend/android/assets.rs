//! APK resources stay with the native host, independently of the compiled app's lifetime.
use std::ffi::{c_char, c_int, c_void, CString};
use std::sync::atomic::{AtomicPtr, Ordering};

static MANAGER: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

/// # Safety
/// The host must retain the Java AssetManager for the lifetime of the process.
pub unsafe fn set_manager(manager: *mut c_void) {
    MANAGER.store(manager, Ordering::Release);
}

#[link(name = "android")]
extern "C" {
    fn AAssetManager_open(manager: *mut c_void, name: *const c_char, mode: c_int) -> *mut c_void;
    fn AAsset_getLength64(asset: *mut c_void) -> i64;
    fn AAsset_read(asset: *mut c_void, out: *mut c_void, count: usize) -> c_int;
    fn AAsset_close(asset: *mut c_void);
    fn AAsset_openFileDescriptor64(asset: *mut c_void, start: *mut i64, length: *mut i64) -> c_int;
}

pub(crate) fn read(path: &str, out: &mut [u8]) -> i64 {
    let manager = MANAGER.load(Ordering::Acquire);
    let Ok(path) = CString::new(path) else { return -1 };
    if manager.is_null() { return -1; }
    unsafe {
        let asset = AAssetManager_open(manager, path.as_ptr(), 2);
        if asset.is_null() { return -1; }
        let len = AAsset_getLength64(asset);
        let mut result = len;
        if len >= 0 && out.len() as u64 >= len as u64 {
            let mut offset = 0;
            while offset < len as usize {
                let n = AAsset_read(asset, out[offset..].as_mut_ptr().cast(), len as usize - offset);
                if n <= 0 { result = -1; break; }
                offset += n as usize;
            }
        }
        AAsset_close(asset);
        result
    }
}

/// A stored (uncompressed) asset as a descriptor range: the open descriptor (the caller owns and
/// closes it) and `out = [start, length]`; -1 for a missing or compressed asset.
pub(crate) fn range(path: &str, out: &mut [i64]) -> i64 {
    let manager = MANAGER.load(Ordering::Acquire);
    let Ok(path) = CString::new(path) else { return -1 };
    if manager.is_null() || out.len() < 2 { return -1; }
    unsafe {
        let asset = AAssetManager_open(manager, path.as_ptr(), 0);
        if asset.is_null() { return -1; }
        let (mut start, mut len) = (0i64, 0i64);
        let fd = AAsset_openFileDescriptor64(asset, &mut start, &mut len);
        AAsset_close(asset);
        if fd >= 0 { out[0] = start; out[1] = len; }
        fd as i64
    }
}

/// A stored asset mapped read-only (see [`range`]).
pub(crate) fn map(path: &str) -> std::io::Result<std::sync::Arc<makepad_font_core::MappedBytes>> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let mut span = [0i64; 2];
    let fd = range(path, &mut span);
    if fd < 0 { return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no stored APK asset")); }
    let fd = unsafe { OwnedFd::from_raw_fd(fd as i32) };
    let bytes = makepad_font_core::SharedBytes::map_fd_range_named(fd.as_raw_fd(), span[0] as u64, span[1] as usize, path)?;
    bytes.mapped().cloned().ok_or_else(|| std::io::Error::other("asset not mapped"))
}
