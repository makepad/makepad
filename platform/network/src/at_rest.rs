//! Keeping a small secret (a key file's bytes) safe on disk, and saying how safe.
//!
//! [`protect_at_rest`] returns the bytes to write and the [`Protection`] they have. Windows binds
//! them to the signed-in user with DPAPI. Other systems have no OS facility that needs no prompt
//! and no signed build, so the bytes stay as they are and the level says [`Protection::FileOnly`]:
//! a caller never claims more than the level it is given. [`exclude_from_backups`] keeps a
//! directory out of the OS's backups where the OS has such a mark.

use std::io;
use std::path::Path;

/// How well the written bytes are protected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protection {
    /// Only this user on this machine can unprotect them (Windows DPAPI, user scope).
    OsBound,
    /// The bytes are as they are: only file permissions and the disk's own encryption protect them.
    FileOnly,
}

#[cfg(windows)]
mod sys {
    use std::io;

    #[repr(C)]
    struct DataBlob {
        len: u32,
        data: *mut u8,
    }

    #[link(name = "crypt32", kind = "raw-dylib")]
    extern "system" {
        fn CryptProtectData(
            data_in: *const DataBlob,
            description: *const u16,
            entropy: *const DataBlob,
            reserved: *const core::ffi::c_void,
            prompt: *const core::ffi::c_void,
            flags: u32,
            data_out: *mut DataBlob,
        ) -> i32;
        fn CryptUnprotectData(
            data_in: *const DataBlob,
            description: *mut *mut u16,
            entropy: *const DataBlob,
            reserved: *const core::ffi::c_void,
            prompt: *const core::ffi::c_void,
            flags: u32,
            data_out: *mut DataBlob,
        ) -> i32;
    }
    #[link(name = "kernel32", kind = "raw-dylib")]
    extern "system" {
        fn LocalFree(mem: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
    }

    const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x1;

    fn blob(bytes: &[u8]) -> io::Result<DataBlob> {
        let len = u32::try_from(bytes.len()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "too large to protect"))?;
        Ok(DataBlob { len, data: bytes.as_ptr() as *mut u8 })
    }

    fn run(protect: bool, purpose: &str, bytes: &[u8]) -> io::Result<Vec<u8>> {
        let input = blob(bytes)?;
        let entropy = blob(purpose.as_bytes())?;
        let mut out = DataBlob { len: 0, data: std::ptr::null_mut() };
        // SAFETY: both input blobs borrow slices that outlive the call and are only read; `out`
        // is filled by the system and released with `LocalFree` after its bytes are copied.
        let ok = unsafe {
            if protect {
                CryptProtectData(&input, std::ptr::null(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
            } else {
                CryptUnprotectData(&input, std::ptr::null_mut(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
            }
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: on success `out` holds `len` bytes the system allocated.
        let result = unsafe { std::slice::from_raw_parts(out.data, out.len as usize).to_vec() };
        unsafe { LocalFree(out.data as *mut core::ffi::c_void) };
        Ok(result)
    }

    pub fn protect(purpose: &str, bytes: &[u8]) -> io::Result<Vec<u8>> {
        run(true, purpose, bytes)
    }
    pub fn unprotect(purpose: &str, bytes: &[u8]) -> io::Result<Vec<u8>> {
        run(false, purpose, bytes)
    }
}

/// The level [`protect_at_rest`] gives on this OS, for showing it without protecting anything.
pub fn at_rest_protection() -> Protection {
    if cfg!(windows) {
        Protection::OsBound
    } else {
        Protection::FileOnly
    }
}

/// `bytes` made ready to write to disk, and how well that protects them. `purpose` is bound in
/// where the OS can: bytes protected for one purpose do not unprotect under another.
pub fn protect_at_rest(purpose: &str, bytes: &[u8]) -> io::Result<(Vec<u8>, Protection)> {
    #[cfg(windows)]
    {
        Ok((sys::protect(purpose, bytes)?, at_rest_protection()))
    }
    #[cfg(not(windows))]
    {
        let _ = purpose;
        Ok((bytes.to_vec(), at_rest_protection()))
    }
}

/// The bytes [`protect_at_rest`] was given, from what it returned. An error when they can't be
/// unprotected; never a reason to make new ones.
pub fn unprotect_at_rest(purpose: &str, bytes: &[u8]) -> io::Result<Vec<u8>> {
    #[cfg(windows)]
    {
        sys::unprotect(purpose, bytes)
    }
    #[cfg(not(windows))]
    {
        let _ = purpose;
        Ok(bytes.to_vec())
    }
}

/// The attribute that keeps a directory out of Time Machine and other backups of the volume.
const BACKUP_EXCLUDE_ATTR: &str = "com.apple.metadata:com_apple_backup_excludeItem";
/// The binary plist of the string `com.apple.backupd`, the value `tmutil addexclusion` writes.
/// Made with `python3 -I -c 'import plistlib; plistlib.dumps("com.apple.backupd", fmt=plistlib.FMT_BINARY)'`.
#[cfg(target_vendor = "apple")]
const BACKUP_EXCLUDE_VALUE: [u8; 61] = [
    0x62, 0x70, 0x6c, 0x69, 0x73, 0x74, 0x30, 0x30, 0x5f, 0x10, 0x11, 0x63, 0x6f, 0x6d, 0x2e, 0x61, 0x70, 0x70, 0x6c, 0x65,
    0x2e, 0x62, 0x61, 0x63, 0x6b, 0x75, 0x70, 0x64, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x1c,
];

#[cfg(target_vendor = "apple")]
extern "C" {
    fn setxattr(path: *const core::ffi::c_char, name: *const core::ffi::c_char, value: *const core::ffi::c_void, size: usize, position: u32, options: i32) -> i32;
    #[cfg(test)]
    fn getxattr(path: *const core::ffi::c_char, name: *const core::ffi::c_char, value: *mut core::ffi::c_void, size: usize, position: u32, options: i32) -> isize;
}

/// Keeps `dir` out of the OS's backups where it has such a mark (Apple systems); elsewhere it
/// does nothing and succeeds.
pub fn exclude_from_backups(dir: &Path) -> io::Result<()> {
    #[cfg(target_vendor = "apple")]
    {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a path with a NUL"))?;
        let name = std::ffi::CString::new(BACKUP_EXCLUDE_ATTR).unwrap();
        // SAFETY: both names are NUL-ended and the value is a live constant of the given size.
        let rc = unsafe { setxattr(path.as_ptr(), name.as_ptr(), BACKUP_EXCLUDE_VALUE.as_ptr() as *const _, BACKUP_EXCLUDE_VALUE.len(), 0, 0) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        let _ = (dir, BACKUP_EXCLUDE_ATTR);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_bytes_round_trip_and_stay_with_their_purpose() {
        let secret = [7u8; 97];
        let (stored, level) = protect_at_rest("test purpose", &secret).unwrap();
        assert_eq!(unprotect_at_rest("test purpose", &stored).unwrap(), secret, "the bytes round trip");
        if cfg!(windows) {
            assert_eq!(level, Protection::OsBound, "Windows binds the bytes to the user");
            assert_ne!(stored, secret, "the stored bytes are not the secret");
            assert!(unprotect_at_rest("another purpose", &stored).is_err(), "another purpose can't unprotect them");
        } else {
            assert_eq!(level, Protection::FileOnly, "no OS facility is claimed");
            assert_eq!(stored, secret, "the bytes are as they were");
        }
        #[cfg(windows)]
        assert!(unprotect_at_rest("test purpose", b"not protected").is_err(), "bytes never protected fail to unprotect");

        let dir = std::env::temp_dir().join(format!("makepad-at-rest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        exclude_from_backups(&dir).unwrap();
        #[cfg(target_vendor = "apple")]
        {
            let path = std::ffi::CString::new(std::os::unix::ffi::OsStrExt::as_bytes(dir.as_os_str())).unwrap();
            let name = std::ffi::CString::new(BACKUP_EXCLUDE_ATTR).unwrap();
            let mut buf = [0u8; 128];
            let n = unsafe { getxattr(path.as_ptr(), name.as_ptr(), buf.as_mut_ptr() as *mut _, buf.len(), 0, 0) };
            assert_eq!(&buf[..n.max(0) as usize], &BACKUP_EXCLUDE_VALUE[..], "the backup exclusion is set");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
