//! Lasting access to folders the user picked.
//!
//! A sandboxed macOS app may read a folder only because the user picked it in
//! an open panel, and that grant ends with the process. To reopen the folder
//! after a restart the app keeps a security-scoped bookmark of it and resolves
//! that at launch. Everywhere else (and for an unsandboxed macOS app, where the
//! scoped form is simply not required) a folder's bookmark is its path, so
//! callers store and resolve bookmarks the same way on every platform.

use std::path::{Path, PathBuf};

const PATH_TAG: &[u8] = b"path:";
#[cfg(target_os = "macos")]
const MACOS_TAG: &[u8] = b"macos-bookmark:";

/// A folder a bookmark resolved to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedFolder {
    /// Where the folder is now (it may have been moved or renamed).
    pub path: PathBuf,
    /// A replacement bookmark when the stored one went stale: store this one
    /// instead, or the next resolve may fail.
    pub refreshed: Option<Vec<u8>>,
}

/// A bookmark for `path`, to store and later pass to
/// [`resolve_folder_bookmark`]. Call it while the app has access to the
/// folder: right after the user picked it.
pub fn folder_bookmark(path: &Path) -> Vec<u8> {
    #[cfg(target_os = "macos")]
    if let Some(bookmark) = path.to_str().and_then(macos::bookmark) {
        return [MACOS_TAG, &bookmark].concat();
    }
    [PATH_TAG, path.to_string_lossy().as_bytes()].concat()
}

/// The folder `bookmark` names, with access granted for the rest of this
/// process. `None` when the folder is gone or access was withdrawn.
pub fn resolve_folder_bookmark(bookmark: &[u8]) -> Option<ResolvedFolder> {
    if let Some(path) = bookmark.strip_prefix(PATH_TAG) {
        let path = PathBuf::from(std::str::from_utf8(path).ok()?);
        return path.is_dir().then_some(ResolvedFolder {
            path,
            refreshed: None,
        });
    }
    #[cfg(target_os = "macos")]
    if let Some(data) = bookmark.strip_prefix(MACOS_TAG) {
        let (path, refreshed) = macos::resolve(data)?;
        return Some(ResolvedFolder {
            path: PathBuf::from(path),
            refreshed: refreshed.map(|bookmark| [MACOS_TAG, &bookmark].concat()),
        });
    }
    None
}

#[cfg(target_os = "macos")]
mod macos {
    use crate::os::apple::apple_sys::*;
    use std::ffi::c_void;

    const CREATION_WITH_SECURITY_SCOPE: u64 = 1 << 11;
    const RESOLUTION_WITHOUT_UI: u64 = 1 << 8;
    const RESOLUTION_WITH_SECURITY_SCOPE: u64 = 1 << 10;

    unsafe fn data_bytes(data: ObjcId) -> Vec<u8> {
        let len: u64 = msg_send![data, length];
        let bytes: *const u8 = msg_send![data, bytes];
        if bytes.is_null() || len == 0 {
            return Vec::new();
        }
        std::slice::from_raw_parts(bytes, len as usize).to_vec()
    }

    /// Security-scoped when the process may create one (sandboxed, with the
    /// bookmarks entitlement), a plain bookmark otherwise.
    pub fn bookmark(path: &str) -> Option<Vec<u8>> {
        unsafe {
            let url: ObjcId =
                msg_send![class!(NSURL), fileURLWithPath: str_to_nsstring(path) isDirectory: YES];
            if url == nil {
                return None;
            }
            for options in [CREATION_WITH_SECURITY_SCOPE, 0] {
                let mut error: ObjcId = nil;
                let data: ObjcId = msg_send![
                    url,
                    bookmarkDataWithOptions: options
                    includingResourceValuesForKeys: nil
                    relativeToURL: nil
                    error: &mut error as *mut ObjcId
                ];
                if data != nil {
                    return Some(data_bytes(data));
                }
            }
            None
        }
    }

    /// The bookmarked folder's current path, and a fresh bookmark when this
    /// one is stale. A security-scoped bookmark also starts access, which is
    /// kept for the life of the process (the app works in the folder until
    /// it quits or opens another).
    pub fn resolve(stored: &[u8]) -> Option<(String, Option<Vec<u8>>)> {
        unsafe {
            let data: ObjcId = msg_send![
                class!(NSData),
                dataWithBytes: stored.as_ptr() as *const c_void
                length: stored.len() as u64
            ];
            if data == nil {
                return None;
            }
            for options in [
                RESOLUTION_WITH_SECURITY_SCOPE | RESOLUTION_WITHOUT_UI,
                RESOLUTION_WITHOUT_UI,
            ] {
                let mut stale: BOOL = NO;
                let mut error: ObjcId = nil;
                let url: ObjcId = msg_send![
                    class!(NSURL),
                    URLByResolvingBookmarkData: data
                    options: options
                    relativeToURL: nil
                    bookmarkDataIsStale: &mut stale as *mut BOOL
                    error: &mut error as *mut ObjcId
                ];
                if url == nil {
                    continue;
                }
                if options & RESOLUTION_WITH_SECURITY_SCOPE != 0 {
                    let _: BOOL = msg_send![url, startAccessingSecurityScopedResource];
                }
                let path: ObjcId = msg_send![url, path];
                if path == nil {
                    return None;
                }
                let path = nsstring_to_string(path);
                let refreshed = if stale != NO { bookmark(&path) } else { None };
                return Some((path, refreshed));
            }
            None
        }
    }
}
