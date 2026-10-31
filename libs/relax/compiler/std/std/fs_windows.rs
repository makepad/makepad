//! std::fs for Windows (x86_64-pc-windows-gnu): handle-based kernel32 calls. Semantics follow
//! real std's sys/fs/windows.rs, sys/pal/windows/handle.rs and sys/path/windows.rs.

#![allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::fmt;

use crate::ffi::OsString;
use crate::io::{self, Read, Seek, SeekFrom, Write};
use crate::os::windows::ffi::OsStringExt;
use crate::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, IntoRawHandle, OwnedHandle, RawHandle};
use crate::path::{Path, PathBuf};
use crate::sys::os::{cvt, fill_utf16_buf, to_wide, BOOL, DWORD, HANDLE, INVALID_HANDLE_VALUE};
// declared once in sys/windows.rs (ReadFile / WriteFile take the OVERLAPPED as *mut c_void)
use crate::sys::os::{CloseHandle, GetConsoleMode, GetFileType, GetLastError, ReadFile, WriteFile};
use crate::time::SystemTime;

// ---- Win32 declarations (kernel32)

#[repr(C)]
#[derive(Copy, Clone)]
struct FILETIME {
    dwLowDateTime: u32,
    dwHighDateTime: u32,
}

#[repr(C)]
struct OVERLAPPED {
    Internal: usize,
    InternalHigh: usize,
    Offset: u32,
    OffsetHigh: u32,
    hEvent: HANDLE,
}

#[repr(C)]
struct BY_HANDLE_FILE_INFORMATION {
    dwFileAttributes: u32,
    ftCreationTime: FILETIME,
    ftLastAccessTime: FILETIME,
    ftLastWriteTime: FILETIME,
    dwVolumeSerialNumber: u32,
    nFileSizeHigh: u32,
    nFileSizeLow: u32,
    nNumberOfLinks: u32,
    nFileIndexHigh: u32,
    nFileIndexLow: u32,
}

#[repr(C)]
struct WIN32_FIND_DATAW {
    dwFileAttributes: u32,
    ftCreationTime: FILETIME,
    ftLastAccessTime: FILETIME,
    ftLastWriteTime: FILETIME,
    nFileSizeHigh: u32,
    nFileSizeLow: u32,
    dwReserved0: u32,
    dwReserved1: u32,
    cFileName: [u16; 260],
    cAlternateFileName: [u16; 14],
}

#[repr(C)]
struct FILE_ATTRIBUTE_TAG_INFO {
    FileAttributes: u32,
    ReparseTag: u32,
}

#[repr(C)]
struct FILE_BASIC_INFO {
    CreationTime: i64,
    LastAccessTime: i64,
    LastWriteTime: i64,
    ChangeTime: i64,
    FileAttributes: u32,
}

#[repr(C)]
struct FILE_END_OF_FILE_INFO {
    EndOfFile: i64,
}

#[repr(C)]
struct FILE_ALLOCATION_INFO {
    AllocationSize: i64,
}

#[repr(C)]
struct FILE_DISPOSITION_INFO_EX {
    Flags: u32,
}

#[repr(C)]
struct FILE_NAME_INFO_MAX {
    FileNameLength: u32,
    FileName: [u16; 260],
}

#[repr(C)]
struct SECURITY_ATTRIBUTES {
    nLength: u32,
    lpSecurityDescriptor: *mut c_void,
    bInheritHandle: BOOL,
}

type LPPROGRESS_ROUTINE = Option<unsafe extern "system" fn(i64, i64, i64, i64, u32, u32, HANDLE, HANDLE, *const c_void) -> u32>;

#[link(name = "kernel32")]
extern "system" {
    fn CreateFileW(name: *const u16, access: DWORD, share: DWORD, sa: *const SECURITY_ATTRIBUTES, creation: DWORD, flags: DWORD, template: HANDLE) -> HANDLE;
    fn SetFilePointerEx(h: HANDLE, distance: i64, new_pos: *mut i64, method: DWORD) -> BOOL;
    fn GetFileSizeEx(h: HANDLE, size: *mut i64) -> BOOL;
    fn FlushFileBuffers(h: HANDLE) -> BOOL;
    fn SetFileInformationByHandle(h: HANDLE, class: i32, info: *const c_void, size: DWORD) -> BOOL;
    fn GetFileInformationByHandle(h: HANDLE, info: *mut BY_HANDLE_FILE_INFORMATION) -> BOOL;
    fn GetFileInformationByHandleEx(h: HANDLE, class: i32, info: *mut c_void, size: DWORD) -> BOOL;
    fn SetFileTime(h: HANDLE, created: *const FILETIME, accessed: *const FILETIME, written: *const FILETIME) -> BOOL;
    fn FindFirstFileExW(name: *const u16, level: i32, data: *mut c_void, op: i32, filter: *const c_void, flags: DWORD) -> HANDLE;
    fn FindNextFileW(h: HANDLE, data: *mut WIN32_FIND_DATAW) -> BOOL;
    fn FindClose(h: HANDLE) -> BOOL;
    fn CreateDirectoryW(name: *const u16, sa: *const SECURITY_ATTRIBUTES) -> BOOL;
    fn RemoveDirectoryW(name: *const u16) -> BOOL;
    fn DeleteFileW(name: *const u16) -> BOOL;
    fn MoveFileExW(from: *const u16, to: *const u16, flags: DWORD) -> BOOL;
    fn CreateHardLinkW(link: *const u16, original: *const u16, sa: *const SECURITY_ATTRIBUTES) -> BOOL;
    fn CreateSymbolicLinkW(link: *const u16, target: *const u16, flags: DWORD) -> u8;
    fn CopyFileExW(from: *const u16, to: *const u16, progress: LPPROGRESS_ROUTINE, data: *const c_void, cancel: *mut BOOL, flags: DWORD) -> BOOL;
    fn GetFinalPathNameByHandleW(h: HANDLE, buf: *mut u16, n: DWORD, flags: DWORD) -> DWORD;
    fn GetFullPathNameW(name: *const u16, n: DWORD, buf: *mut u16, file_part: *mut *mut u16) -> DWORD;
    fn SetFileAttributesW(name: *const u16, attrs: DWORD) -> BOOL;
    fn DeviceIoControl(h: HANDLE, code: DWORD, inbuf: *const c_void, insize: DWORD, outbuf: *mut c_void, outsize: DWORD, returned: *mut DWORD, overlapped: *mut OVERLAPPED) -> BOOL;
    fn LockFileEx(h: HANDLE, flags: DWORD, reserved: DWORD, low: DWORD, high: DWORD, overlapped: *mut OVERLAPPED) -> BOOL;
    fn UnlockFile(h: HANDLE, off_low: DWORD, off_high: DWORD, low: DWORD, high: DWORD) -> BOOL;
    fn CreateEventW(sa: *const SECURITY_ATTRIBUTES, manual: BOOL, initial: BOOL, name: *const u16) -> HANDLE;
    fn GetOverlappedResult(h: HANDLE, overlapped: *mut OVERLAPPED, transferred: *mut DWORD, wait: BOOL) -> BOOL;
}

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_WRITE_DATA: u32 = 0x0002;
const FILE_GENERIC_WRITE: u32 = 0x0012_0116;
const FILE_LIST_DIRECTORY: u32 = 0x0001;
const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
const DELETE: u32 = 0x0001_0000;

const FILE_SHARE_READ: u32 = 0x1;
const FILE_SHARE_WRITE: u32 = 0x2;
const FILE_SHARE_DELETE: u32 = 0x4;

const CREATE_NEW: u32 = 1;
const OPEN_EXISTING: u32 = 3;
const OPEN_ALWAYS: u32 = 4;
const TRUNCATE_EXISTING: u32 = 5;

const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const SECURITY_SQOS_PRESENT: u32 = 0x0010_0000;

pub(crate) const FILE_ATTRIBUTE_READONLY: u32 = 0x1;
pub(crate) const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
pub(crate) const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

const FILE_BEGIN: u32 = 0;
const FILE_CURRENT: u32 = 1;
const FILE_END: u32 = 2;

const LOCKFILE_FAIL_IMMEDIATELY: u32 = 0x1;
const LOCKFILE_EXCLUSIVE_LOCK: u32 = 0x2;

const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_PATH_NOT_FOUND: u32 = 3;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_NO_MORE_FILES: u32 = 18;
const ERROR_SHARING_VIOLATION: u32 = 32;
const ERROR_LOCK_VIOLATION: u32 = 33;
const ERROR_HANDLE_EOF: u32 = 38;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_BROKEN_PIPE: u32 = 109;
const ERROR_NOT_LOCKED: u32 = 158;
const ERROR_ALREADY_EXISTS: u32 = 183;
const ERROR_DIRECTORY: u32 = 267;
const ERROR_NEGATIVE_SEEK: u32 = 131;
const ERROR_IO_PENDING: u32 = 997;
const ERROR_CANT_ACCESS_FILE: u32 = 1920;

const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
const VOLUME_NAME_DOS: u32 = 0x0;
const PROGRESS_CONTINUE: u32 = 0;

const FSCTL_GET_REPARSE_POINT: u32 = 0x0009_00a8;
const MAXIMUM_REPARSE_DATA_BUFFER_SIZE: usize = 16 * 1024;
const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xa000_0003;
const IO_REPARSE_TAG_SYMLINK: u32 = 0xa000_000c;
const SYMLINK_FLAG_RELATIVE: u32 = 0x1;

const SYMBOLIC_LINK_FLAG_DIRECTORY: u32 = 0x1;
const SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE: u32 = 0x2;

const FILE_TYPE_PIPE: u32 = 3;

// FILE_INFO_BY_HANDLE_CLASS
const FileBasicInfo: i32 = 0;
const FileNameInfo: i32 = 2;
const FileAllocationInfo: i32 = 5;
const FileEndOfFileInfo: i32 = 6;
const FileAttributeTagInfo: i32 = 9;
const FileDispositionInfoEx: i32 = 21;

const FILE_DISPOSITION_FLAG_DELETE: u32 = 0x1;
const FILE_DISPOSITION_FLAG_POSIX_SEMANTICS: u32 = 0x2;
const FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE: u32 = 0x10;

// FINDEX_INFO_LEVELS / FINDEX_SEARCH_OPS
const FindExInfoBasic: i32 = 1;
const FindExSearchNameMatch: i32 = 0;

fn last_error() -> u32 {
    unsafe { GetLastError() }
}

fn os_error(code: u32) -> io::Error {
    io::Error::from_raw_os_error(code as i32)
}

fn set_info<T>(h: HANDLE, class: i32, info: &T) -> io::Result<()> {
    cvt(unsafe { SetFileInformationByHandle(h, class, info as *const T as *const c_void, core::mem::size_of::<T>() as DWORD) })?;
    Ok(())
}

// ---- paths

const SEP: u16 = b'\\' as u16;
const ALT_SEP: u16 = b'/' as u16;
const QUERY: u16 = b'?' as u16;
const COLON: u16 = b':' as u16;
const DOT: u16 = b'.' as u16;

fn starts_with_u16(s: &[u16], p: &[u16]) -> bool {
    s.len() >= p.len() && &s[..p.len()] == p
}

/// The path as a NUL-terminated wide string, made absolute and verbatim (`\\?\`) so long paths
/// work (real std's maybe_verbatim / get_long_path with prefer_verbatim).
fn maybe_verbatim(path: &Path) -> io::Result<Vec<u16>> {
    let path = to_wide(path.as_os_str())?;
    get_long_path(path, true)
}

fn get_long_path(path: Vec<u16>, prefer_verbatim: bool) -> io::Result<Vec<u16>> {
    const LEGACY_MAX_PATH: usize = 248;
    let verbatim: [u16; 4] = [SEP, SEP, QUERY, SEP];
    let nt: [u16; 4] = [SEP, QUERY, QUERY, SEP];
    let unc: [u16; 8] = [SEP, SEP, QUERY, SEP, b'U' as u16, b'N' as u16, b'C' as u16, SEP];

    if starts_with_u16(&path, &verbatim) || starts_with_u16(&path, &nt) || (path.len() == 1 && path[0] == 0) {
        return Ok(path);
    } else if path.len() < LEGACY_MAX_PATH {
        // `D:` alone or `D:\...` / `D:/...` (the drive not a separator)
        if path.len() >= 3 && path[1] == COLON && path[0] != SEP && path[0] != ALT_SEP {
            if path[2] == 0 && path.len() == 3 {
                return Ok(path);
            }
            if path[2] == SEP || path[2] == ALT_SEP {
                return Ok(path);
            }
        }
        // `\\...` / `//...`
        if path.len() >= 2 && (path[0] == SEP || path[0] == ALT_SEP) && (path[1] == SEP || path[1] == ALT_SEP) {
            return Ok(path);
        }
    }

    let absolute = fill_utf16_buf(|buf, n| unsafe { GetFullPathNameW(path.as_ptr(), n, buf, core::ptr::null_mut()) })?;
    let mut out: Vec<u16> = Vec::new();
    let mut rest: &[u16] = &absolute;
    if prefer_verbatim || absolute.len() + 1 >= LEGACY_MAX_PATH {
        if absolute.len() >= 3 && absolute[1] == COLON && absolute[2] == SEP {
            out.extend_from_slice(&verbatim);
        } else if absolute.len() >= 4 && absolute[0] == SEP && absolute[1] == SEP && absolute[2] == DOT && absolute[3] == SEP {
            rest = &absolute[4..];
            out.extend_from_slice(&verbatim);
        } else if starts_with_u16(&absolute, &verbatim) || starts_with_u16(&absolute, &nt) {
        } else if absolute.len() >= 2 && absolute[0] == SEP && absolute[1] == SEP {
            rest = &absolute[2..];
            out.extend_from_slice(&unc);
        }
    }
    out.extend_from_slice(rest);
    out.push(0);
    Ok(out)
}

/// A `\\?\` path back in its user form when that names the same file (real std's
/// sys/args/windows.rs from_wide_to_user_path). `path` and the result are NUL-terminated.
fn from_wide_to_user_path(mut path: Vec<u16>) -> io::Result<Vec<u16>> {
    const LEGACY_MAX_PATH: usize = 260;
    if path.len() > LEGACY_MAX_PATH {
        return Ok(path);
    }
    let n = path.len();
    if n >= 7 && path[0] == SEP && path[1] == SEP && path[2] == QUERY && path[3] == SEP && path[5] == COLON && path[6] == SEP {
        let full = fill_utf16_buf(|buf, sz| unsafe { GetFullPathNameW(path[4..].as_ptr(), sz, buf, core::ptr::null_mut()) })?;
        if full[..] == path[4..n - 1] {
            let mut p = full;
            p.push(0);
            return Ok(p);
        }
        return Ok(path);
    }
    if n >= 8 && path[0] == SEP && path[1] == SEP && path[2] == QUERY && path[3] == SEP && path[4] == b'U' as u16 && path[5] == b'N' as u16 && path[6] == b'C' as u16 && path[7] == SEP {
        path[6] = SEP;
        let full = fill_utf16_buf(|buf, sz| unsafe { GetFullPathNameW(path[6..].as_ptr(), sz, buf, core::ptr::null_mut()) })?;
        if full[..] == path[6..n - 1] {
            let mut p = full;
            p.push(0);
            return Ok(p);
        }
        path[6] = b'C' as u16;
        return Ok(path);
    }
    Ok(path)
}

fn os2path(s: &[u16]) -> PathBuf {
    PathBuf::from(OsString::from_wide(s))
}

fn get_path(h: HANDLE) -> io::Result<PathBuf> {
    let w = fill_utf16_buf(|buf, n| unsafe { GetFinalPathNameByHandleW(h, buf, n, VOLUME_NAME_DOS) })?;
    Ok(os2path(&w))
}

// ---- time conversion: FILETIME (100 ns since 1601-01-01) <-> SystemTime (unix epoch)

const INTERVALS_PER_SEC: i128 = 10_000_000;
const INTERVALS_TO_UNIX_EPOCH: i128 = 11_644_473_600 * INTERVALS_PER_SEC;

fn filetime_u64(ft: &FILETIME) -> u64 {
    (ft.dwLowDateTime as u64) | ((ft.dwHighDateTime as u64) << 32)
}

fn system_time_from_intervals(t: u64) -> SystemTime {
    let t = t as i128 - INTERVALS_TO_UNIX_EPOCH;
    let secs = t.div_euclid(INTERVALS_PER_SEC) as i64;
    let nanos = (t.rem_euclid(INTERVALS_PER_SEC) * 100) as i64;
    SystemTime::from_parts(secs, nanos)
}

fn intervals_from_system_time(t: SystemTime) -> io::Result<FILETIME> {
    let (secs, nanos) = t.unix_parts();
    let v = secs as i128 * INTERVALS_PER_SEC + (nanos / 100) as i128 + INTERVALS_TO_UNIX_EPOCH;
    if v < 0 || v > u64::MAX as i128 {
        return Err(io::const_error!(io::ErrorKind::InvalidInput, "file time is out of range"));
    }
    let v = v as u64;
    Ok(FILETIME { dwLowDateTime: v as u32, dwHighDateTime: (v >> 32) as u32 })
}

// ---- File

pub struct File {
    handle: OwnedHandle,
    /// a virtual file of a git run (Relax's VFS): its bytes and cursor; the handle is NUL's
    mem: Option<Box<Mem>>,
}

struct Mem {
    data: Arc<Vec<u8>>,
    pos: core::sync::atomic::AtomicU64,
}

use super::vfs;

#[derive(Clone, Debug)]
pub struct OpenOptions {
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
    create: bool,
    create_new: bool,
    custom_flags: u32,
    access_mode: Option<u32>,
    attributes: u32,
    share_mode: u32,
    security_qos_flags: u32,
}

fn needs_write() -> io::Error {
    io::Error::from_string(io::ErrorKind::InvalidInput, String::from("creating or truncating a file requires write or append access"))
}

impl OpenOptions {
    pub fn new() -> OpenOptions {
        OpenOptions {
            read: false,
            write: false,
            append: false,
            truncate: false,
            create: false,
            create_new: false,
            custom_flags: 0,
            access_mode: None,
            attributes: 0,
            share_mode: FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            security_qos_flags: 0,
        }
    }
    pub fn read(&mut self, read: bool) -> &mut OpenOptions {
        self.read = read;
        self
    }
    pub fn write(&mut self, write: bool) -> &mut OpenOptions {
        self.write = write;
        self
    }
    pub fn append(&mut self, append: bool) -> &mut OpenOptions {
        self.append = append;
        self
    }
    pub fn truncate(&mut self, truncate: bool) -> &mut OpenOptions {
        self.truncate = truncate;
        self
    }
    pub fn create(&mut self, create: bool) -> &mut OpenOptions {
        self.create = create;
        self
    }
    pub fn create_new(&mut self, create_new: bool) -> &mut OpenOptions {
        self.create_new = create_new;
        self
    }
    // std::os::windows::fs::OpenOptionsExt
    pub(crate) fn set_custom_flags(&mut self, flags: u32) {
        self.custom_flags = flags;
    }
    pub(crate) fn set_access_mode(&mut self, access: u32) {
        self.access_mode = Some(access);
    }
    pub(crate) fn set_share_mode(&mut self, share: u32) {
        self.share_mode = share;
    }
    pub(crate) fn set_attributes(&mut self, attrs: u32) {
        self.attributes = attrs;
    }
    pub(crate) fn set_security_qos_flags(&mut self, flags: u32) {
        // SECURITY_ANONYMOUS is 0, so the presence bit is set here (as real std)
        self.security_qos_flags = flags | SECURITY_SQOS_PRESENT;
    }
    pub fn open<P: AsRef<Path>>(&self, path: P) -> io::Result<File> {
        if let Some((abs, k)) = vfs::kind(path.as_ref()) {
            if self.write || self.append || self.truncate || self.create || self.create_new {
                // the run's edits folder, if it has one (else read-only)
                let how = if self.truncate || (self.create_new && matches!(k, vfs::Kind::Missing)) || (!self.append && matches!(k, vfs::Kind::Missing)) { vfs::Edit::Replace } else { vfs::Edit::Keep };
                let disk = vfs::edit_target(&abs, how).unwrap_or_else(|| Err(vfs::read_only()))?;
                return self.open_native(&maybe_verbatim(&disk)?);
            }
            return match k {
                vfs::Kind::File(_) => {
                    let data = vfs::read(&abs)?;
                    let null: Vec<u16> = "NUL".encode_utf16().chain(core::iter::once(0)).collect();
                    let mut f = OpenOptions::new().read(true).open_native(&null)?;
                    f.mem = Some(Box::new(Mem { data: Arc::new(data), pos: core::sync::atomic::AtomicU64::new(0) }));
                    Ok(f)
                }
                vfs::Kind::Dir => Err(os_error(ERROR_ACCESS_DENIED)),
                vfs::Kind::Missing => Err(io::Error::from(io::ErrorKind::NotFound)),
            };
        }
        let path = maybe_verbatim(path.as_ref())?;
        self.open_native(&path)
    }

    fn get_access_mode(&self) -> io::Result<u32> {
        if let Some(mode) = self.access_mode {
            return Ok(mode);
        }
        match (self.read, self.write, self.append) {
            (true, false, false) => Ok(GENERIC_READ),
            (false, true, false) => Ok(GENERIC_WRITE),
            (true, true, false) => Ok(GENERIC_READ | GENERIC_WRITE),
            (false, _, true) => Ok(FILE_GENERIC_WRITE & !FILE_WRITE_DATA),
            (true, _, true) => Ok(GENERIC_READ | (FILE_GENERIC_WRITE & !FILE_WRITE_DATA)),
            (false, false, false) => {
                if self.create || self.create_new || self.truncate {
                    Err(needs_write())
                } else {
                    Err(io::Error::from_string(io::ErrorKind::InvalidInput, String::from("must specify at least one of read, write, or append access")))
                }
            }
        }
    }

    fn get_creation_mode(&self) -> io::Result<u32> {
        match (self.write, self.append) {
            (true, false) => {}
            (false, false) => {
                if self.truncate || self.create || self.create_new {
                    return Err(needs_write());
                }
            }
            (_, true) => {
                if self.truncate && !self.create_new {
                    return Err(needs_write());
                }
            }
        }
        Ok(match (self.create, self.truncate, self.create_new) {
            (false, false, false) => OPEN_EXISTING,
            (true, false, false) => OPEN_ALWAYS,
            (false, true, false) => TRUNCATE_EXISTING,
            // CREATE_ALWAYS replaces attributes / streams: OPEN_ALWAYS + manual truncation (#115745)
            (true, true, false) => OPEN_ALWAYS,
            (_, _, true) => CREATE_NEW,
        })
    }

    fn get_flags_and_attributes(&self) -> u32 {
        self.custom_flags | self.attributes | self.security_qos_flags | if self.create_new { FILE_FLAG_OPEN_REPARSE_POINT } else { 0 }
    }

    /// `path` is NUL-terminated.
    fn open_native(&self, path: &[u16]) -> io::Result<File> {
        let creation = self.get_creation_mode()?;
        let access = self.get_access_mode()?;
        let h = unsafe { CreateFileW(path.as_ptr(), access, self.share_mode, core::ptr::null(), creation, self.get_flags_and_attributes(), core::ptr::null_mut()) };
        if h == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let file = File { handle: unsafe { OwnedHandle::from_raw_handle(h as RawHandle) }, mem: None };
        if self.truncate && creation == OPEN_ALWAYS && last_error() == ERROR_ALREADY_EXISTS {
            // FileAllocationInfo first, FileEndOfFileInfo for filesystems without it (WINE)
            let alloc_info = FILE_ALLOCATION_INFO { AllocationSize: 0 };
            if set_info(file.raw(), FileAllocationInfo, &alloc_info).is_err() {
                let eof = FILE_END_OF_FILE_INFO { EndOfFile: 0 };
                set_info(file.raw(), FileEndOfFileInfo, &eof)?;
            }
        }
        Ok(file)
    }
}

impl Default for OpenOptions {
    fn default() -> OpenOptions {
        OpenOptions::new()
    }
}

#[derive(Debug)]
pub enum TryLockError {
    Error(io::Error),
    WouldBlock,
}

impl fmt::Display for TryLockError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            TryLockError::Error(_) => f.pad("lock acquisition failed due to I/O error"),
            TryLockError::WouldBlock => f.pad("lock acquisition failed because the operation would block"),
        }
    }
}

impl core::error::Error for TryLockError {}

impl From<TryLockError> for io::Error {
    fn from(err: TryLockError) -> io::Error {
        match err {
            TryLockError::Error(e) => e,
            TryLockError::WouldBlock => io::ErrorKind::WouldBlock.into(),
        }
    }
}

fn zeroed_overlapped() -> OVERLAPPED {
    OVERLAPPED { Internal: 0, InternalHigh: 0, Offset: 0, OffsetHigh: 0, hEvent: core::ptr::null_mut() }
}

impl File {
    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<File> {
        OpenOptions::new().read(true).open(path.as_ref())
    }
    pub fn create<P: AsRef<Path>>(path: P) -> io::Result<File> {
        OpenOptions::new().write(true).create(true).truncate(true).open(path.as_ref())
    }
    pub fn create_new<P: AsRef<Path>>(path: P) -> io::Result<File> {
        OpenOptions::new().read(true).write(true).create_new(true).open(path.as_ref())
    }
    pub fn options() -> OpenOptions {
        OpenOptions::new()
    }
    fn raw(&self) -> HANDLE {
        self.handle.as_raw_handle() as HANDLE
    }
    pub fn sync_all(&self) -> io::Result<()> {
        cvt(unsafe { FlushFileBuffers(self.raw()) })?;
        Ok(())
    }
    pub fn sync_data(&self) -> io::Result<()> {
        self.sync_all()
    }
    pub fn set_len(&self, size: u64) -> io::Result<()> {
        let info = FILE_END_OF_FILE_INFO { EndOfFile: size as i64 };
        set_info(self.raw(), FileEndOfFileInfo, &info)
    }
    pub fn metadata(&self) -> io::Result<Metadata> {
        if let Some(m) = &self.mem {
            return Ok(Metadata::virtual_entry(&vfs::Kind::File(m.data.len() as u64)));
        }
        unsafe {
            let mut info: BY_HANDLE_FILE_INFORMATION = core::mem::zeroed();
            cvt(GetFileInformationByHandle(self.raw(), &mut info))?;
            let mut reparse_tag = 0;
            if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                let mut tag = FILE_ATTRIBUTE_TAG_INFO { FileAttributes: 0, ReparseTag: 0 };
                cvt(GetFileInformationByHandleEx(self.raw(), FileAttributeTagInfo, &mut tag as *mut FILE_ATTRIBUTE_TAG_INFO as *mut c_void, core::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as DWORD))?;
                if tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                    reparse_tag = tag.ReparseTag;
                }
            }
            Ok(Metadata {
                attributes: info.dwFileAttributes,
                creation_time: filetime_u64(&info.ftCreationTime),
                last_access_time: filetime_u64(&info.ftLastAccessTime),
                last_write_time: filetime_u64(&info.ftLastWriteTime),
                change_time: None,
                file_size: (info.nFileSizeLow as u64) | ((info.nFileSizeHigh as u64) << 32),
                reparse_tag,
                volume_serial_number: Some(info.dwVolumeSerialNumber),
                number_of_links: Some(info.nNumberOfLinks),
                file_index: Some((info.nFileIndexLow as u64) | ((info.nFileIndexHigh as u64) << 32)),
            })
        }
    }
    fn basic_info(&self) -> io::Result<FILE_BASIC_INFO> {
        let mut info = FILE_BASIC_INFO { CreationTime: 0, LastAccessTime: 0, LastWriteTime: 0, ChangeTime: 0, FileAttributes: 0 };
        cvt(unsafe { GetFileInformationByHandleEx(self.raw(), FileBasicInfo, &mut info as *mut FILE_BASIC_INFO as *mut c_void, core::mem::size_of::<FILE_BASIC_INFO>() as DWORD) })?;
        Ok(info)
    }
    pub fn try_clone(&self) -> io::Result<File> {
        let mem = self.mem.as_ref().map(|m| Box::new(Mem { data: m.data.clone(), pos: core::sync::atomic::AtomicU64::new(m.pos.load(core::sync::atomic::Ordering::Relaxed)) }));
        Ok(File { handle: self.handle.try_clone()?, mem })
    }
    pub fn set_permissions(&self, perm: Permissions) -> io::Result<()> {
        let info = FILE_BASIC_INFO { CreationTime: 0, LastAccessTime: 0, LastWriteTime: 0, ChangeTime: 0, FileAttributes: perm.attrs };
        set_info(self.raw(), FileBasicInfo, &info)
    }
    pub fn set_times(&self, times: FileTimes) -> io::Result<()> {
        let accessed = match times.accessed {
            Some(t) => Some(intervals_from_system_time(t)?),
            None => None,
        };
        let modified = match times.modified {
            Some(t) => Some(intervals_from_system_time(t)?),
            None => None,
        };
        let created = match times.created {
            Some(t) => Some(intervals_from_system_time(t)?),
            None => None,
        };
        let mut zero = false;
        let mut max = false;
        for t in [accessed, modified, created].iter() {
            if let Some(t) = t {
                let v = filetime_u64(t);
                if v == 0 {
                    zero = true;
                }
                if v == u64::MAX {
                    max = true;
                }
            }
        }
        if zero {
            return Err(io::const_error!(io::ErrorKind::InvalidInput, "cannot set file timestamp to 0"));
        }
        if max {
            return Err(io::const_error!(io::ErrorKind::InvalidInput, "cannot set file timestamp to 0xFFFF_FFFF_FFFF_FFFF"));
        }
        let p = |t: &Option<FILETIME>| -> *const FILETIME {
            match t {
                Some(ft) => ft as *const FILETIME,
                None => core::ptr::null(),
            }
        };
        cvt(unsafe { SetFileTime(self.raw(), p(&created), p(&accessed), p(&modified)) })?;
        Ok(())
    }
    pub fn set_modified(&self, time: SystemTime) -> io::Result<()> {
        self.set_times(FileTimes::new().set_modified(time))
    }
    fn acquire_lock(&self, flags: u32) -> io::Result<()> {
        unsafe {
            let mut overlapped = zeroed_overlapped();
            let event = CreateEventW(core::ptr::null(), 0, 0, core::ptr::null());
            if event.is_null() {
                return Err(io::Error::last_os_error());
            }
            overlapped.hEvent = event;
            let r = match cvt(LockFileEx(self.raw(), flags, 0, u32::MAX, u32::MAX, &mut overlapped)) {
                Ok(_) => Ok(()),
                Err(e) => {
                    if e.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
                        // an async handle: wait for the lock
                        let mut transferred: DWORD = 0;
                        match cvt(GetOverlappedResult(self.raw(), &mut overlapped, &mut transferred, 1)) {
                            Ok(_) => Ok(()),
                            Err(e) => Err(e),
                        }
                    } else {
                        Err(e)
                    }
                }
            };
            CloseHandle(overlapped.hEvent);
            r
        }
    }
    pub fn lock(&self) -> io::Result<()> {
        self.acquire_lock(LOCKFILE_EXCLUSIVE_LOCK)
    }
    pub fn lock_shared(&self) -> io::Result<()> {
        self.acquire_lock(0)
    }
    fn try_lock_flags(&self, flags: u32) -> Result<(), TryLockError> {
        let mut overlapped = zeroed_overlapped();
        match cvt(unsafe { LockFileEx(self.raw(), flags | LOCKFILE_FAIL_IMMEDIATELY, 0, u32::MAX, u32::MAX, &mut overlapped) }) {
            Ok(_) => Ok(()),
            Err(e) => {
                if e.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32) {
                    Err(TryLockError::WouldBlock)
                } else {
                    Err(TryLockError::Error(e))
                }
            }
        }
    }
    pub fn try_lock(&self) -> Result<(), TryLockError> {
        self.try_lock_flags(LOCKFILE_EXCLUSIVE_LOCK)
    }
    pub fn try_lock_shared(&self) -> Result<(), TryLockError> {
        self.try_lock_flags(0)
    }
    pub fn unlock(&self) -> io::Result<()> {
        // twice: a handle may hold both an exclusive and a shared lock (real std)
        cvt(unsafe { UnlockFile(self.raw(), 0, 0, u32::MAX, u32::MAX) })?;
        match cvt(unsafe { UnlockFile(self.raw(), 0, 0, u32::MAX, u32::MAX) }) {
            Ok(_) => Ok(()),
            Err(e) => {
                if e.raw_os_error() == Some(ERROR_NOT_LOCKED as i32) {
                    Ok(())
                } else {
                    Err(e)
                }
            }
        }
    }

    fn read_raw(&self, buf: &mut [u8], offset: Option<u64>) -> io::Result<usize> {
        if let Some(m) = &self.mem {
            // a positional read moves the cursor too, as ReadFile with an OVERLAPPED offset does
            let at = offset.unwrap_or_else(|| m.pos.load(core::sync::atomic::Ordering::Relaxed)) as usize;
            let n = if at >= m.data.len() { 0 } else { buf.len().min(m.data.len() - at) };
            buf[..n].copy_from_slice(&m.data[at..at + n]);
            m.pos.store((at + n) as u64, core::sync::atomic::Ordering::Relaxed);
            return Ok(n);
        }
        let len = if buf.len() > u32::MAX as usize { u32::MAX } else { buf.len() as u32 };
        let mut read: DWORD = 0;
        let mut overlapped = zeroed_overlapped();
        let op = match offset {
            Some(o) => {
                overlapped.Offset = o as u32;
                overlapped.OffsetHigh = (o >> 32) as u32;
                &mut overlapped as *mut OVERLAPPED as *mut c_void
            }
            None => core::ptr::null_mut(),
        };
        if unsafe { ReadFile(self.raw(), buf.as_mut_ptr(), len, &mut read, op) } == 0 {
            let e = last_error();
            // the write end of a pipe closed / reading at or past the end: end of file
            if e == ERROR_BROKEN_PIPE || (offset.is_some() && e == ERROR_HANDLE_EOF) {
                return Ok(0);
            }
            return Err(os_error(e));
        }
        Ok(read as usize)
    }
    fn write_raw(&self, buf: &[u8], offset: Option<u64>) -> io::Result<usize> {
        if self.mem.is_some() {
            return Err(vfs::read_only());
        }
        let len = if buf.len() > u32::MAX as usize { u32::MAX } else { buf.len() as u32 };
        let mut written: DWORD = 0;
        let mut overlapped = zeroed_overlapped();
        let op = match offset {
            Some(o) => {
                overlapped.Offset = o as u32;
                overlapped.OffsetHigh = (o >> 32) as u32;
                &mut overlapped as *mut OVERLAPPED as *mut c_void
            }
            None => core::ptr::null_mut(),
        };
        if unsafe { WriteFile(self.raw(), buf.as_ptr(), len, &mut written, op) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(written as usize)
    }
    /// Positional read (std::os::windows::fs::FileExt::seek_read; moves the file cursor).
    pub(crate) fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        self.read_raw(buf, Some(offset))
    }
    /// Positional write (FileExt::seek_write; moves the file cursor).
    pub(crate) fn write_at(&self, buf: &[u8], offset: u64) -> io::Result<usize> {
        self.write_raw(buf, Some(offset))
    }
    fn seek_raw(&self, pos: SeekFrom) -> io::Result<u64> {
        if let Some(m) = &self.mem {
            let cur = m.pos.load(core::sync::atomic::Ordering::Relaxed) as i64;
            let to = match pos {
                SeekFrom::Start(o) => o as i64,
                SeekFrom::End(o) => m.data.len() as i64 + o,
                SeekFrom::Current(o) => cur + o,
            };
            if to < 0 {
                return Err(os_error(ERROR_NEGATIVE_SEEK));
            }
            m.pos.store(to as u64, core::sync::atomic::Ordering::Relaxed);
            return Ok(to as u64);
        }
        let (whence, off) = match pos {
            SeekFrom::Start(o) => (FILE_BEGIN, o as i64),
            SeekFrom::End(o) => (FILE_END, o),
            SeekFrom::Current(o) => (FILE_CURRENT, o),
        };
        let mut newpos: i64 = 0;
        cvt(unsafe { SetFilePointerEx(self.raw(), off, &mut newpos, whence) })?;
        Ok(newpos as u64)
    }
    /// Buffer sized from the file length for read_to_end (as real std does).
    fn size_hint(&self) -> Option<usize> {
        if let Some(m) = &self.mem {
            let pos = m.pos.load(core::sync::atomic::Ordering::Relaxed) as usize;
            return Some(m.data.len().saturating_sub(pos));
        }
        let mut size: i64 = 0;
        if unsafe { GetFileSizeEx(self.raw(), &mut size) } == 0 {
            return None;
        }
        let pos = self.seek_raw(SeekFrom::Current(0)).ok()?;
        let size = size as u64;
        Some(if size > pos { (size - pos) as usize } else { 0 })
    }
}

impl Read for File {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_raw(buf, None)
    }
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        (&*self).read_to_end(buf)
    }
}

impl Read for &File {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_raw(buf, None)
    }
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        if let Some(n) = File::size_hint(*self) {
            buf.reserve(n);
        }
        let start = buf.len();
        let mut chunk = [0u8; 8 * 1024];
        loop {
            match self.read_raw(&mut chunk, None) {
                Ok(0) => return Ok(buf.len() - start),
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e) => {
                    if !e.is_interrupted() {
                        return Err(e);
                    }
                }
            }
        }
    }
}

impl Write for File {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_raw(buf, None)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Write for &File {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_raw(buf, None)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for File {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.seek_raw(pos)
    }
}

impl Seek for &File {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.seek_raw(pos)
    }
}

impl fmt::Debug for File {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut b = f.debug_struct("File");
        b.field("handle", &self.raw());
        if let Ok(path) = get_path(self.raw()) {
            b.field("path", &path);
        }
        b.finish()
    }
}

impl AsRawHandle for File {
    fn as_raw_handle(&self) -> RawHandle {
        self.handle.as_raw_handle()
    }
}
impl AsHandle for File {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.handle.as_handle()
    }
}
impl FromRawHandle for File {
    unsafe fn from_raw_handle(handle: RawHandle) -> File {
        File { handle: OwnedHandle::from_raw_handle(handle), mem: None }
    }
}
impl IntoRawHandle for File {
    fn into_raw_handle(self) -> RawHandle {
        self.handle.into_raw_handle()
    }
}
impl From<File> for OwnedHandle {
    fn from(f: File) -> OwnedHandle {
        f.handle
    }
}
impl From<OwnedHandle> for File {
    fn from(handle: OwnedHandle) -> File {
        File { handle, mem: None }
    }
}

/// A console, or an msys / cygwin pty pipe (real std's sys/io/is_terminal/windows.rs).
fn handle_is_console(h: HANDLE) -> bool {
    if h.is_null() {
        return false;
    }
    let mut mode: DWORD = 0;
    if unsafe { GetConsoleMode(h, &mut mode) } != 0 {
        return true;
    }
    if unsafe { GetFileType(h) } != FILE_TYPE_PIPE {
        return false;
    }
    let mut info = FILE_NAME_INFO_MAX { FileNameLength: 0, FileName: [0; 260] };
    let r = unsafe { GetFileInformationByHandleEx(h, FileNameInfo, &mut info as *mut FILE_NAME_INFO_MAX as *mut c_void, core::mem::size_of::<FILE_NAME_INFO_MAX>() as DWORD) };
    if r == 0 {
        return false;
    }
    let n = info.FileNameLength as usize / 2;
    if n > info.FileName.len() {
        return false;
    }
    let name = String::from_utf16_lossy(&info.FileName[..n]);
    let base = match name.rsplit('\\').next() {
        Some(b) => b,
        None => &name,
    };
    (base.starts_with("msys-") || base.starts_with("cygwin-")) && base.contains("-pty")
}

impl crate::io::IsTerminal for File {
    fn is_terminal(&self) -> bool {
        handle_is_console(self.raw())
    }
}

// ---- Metadata / FileType / Permissions

#[derive(Clone)]
pub struct Metadata {
    attributes: u32,
    creation_time: u64,
    last_access_time: u64,
    last_write_time: u64,
    change_time: Option<u64>,
    file_size: u64,
    reparse_tag: u32,
    volume_serial_number: Option<u32>,
    number_of_links: Option<u32>,
    file_index: Option<u64>,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct FileType {
    is_directory: bool,
    is_symlink: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct Permissions {
    attrs: u32,
}

impl Metadata {
    pub fn file_type(&self) -> FileType {
        FileType::new(self.attributes, self.reparse_tag)
    }
    pub fn is_dir(&self) -> bool {
        self.file_type().is_dir()
    }
    pub fn is_file(&self) -> bool {
        self.file_type().is_file()
    }
    pub fn is_symlink(&self) -> bool {
        self.file_type().is_symlink()
    }
    pub fn len(&self) -> u64 {
        self.file_size
    }
    pub fn permissions(&self) -> Permissions {
        Permissions { attrs: self.attributes }
    }
    pub fn modified(&self) -> io::Result<SystemTime> {
        Ok(system_time_from_intervals(self.last_write_time))
    }
    pub fn accessed(&self) -> io::Result<SystemTime> {
        Ok(system_time_from_intervals(self.last_access_time))
    }
    pub fn created(&self) -> io::Result<SystemTime> {
        Ok(system_time_from_intervals(self.creation_time))
    }
    // std::os::windows::fs::MetadataExt
    pub(crate) fn attrs(&self) -> u32 {
        self.attributes
    }
    pub(crate) fn created_u64(&self) -> u64 {
        self.creation_time
    }
    pub(crate) fn accessed_u64(&self) -> u64 {
        self.last_access_time
    }
    pub(crate) fn modified_u64(&self) -> u64 {
        self.last_write_time
    }
    pub(crate) fn changed_u64(&self) -> Option<u64> {
        self.change_time
    }
    pub(crate) fn volume_serial_number(&self) -> Option<u32> {
        self.volume_serial_number
    }
    pub(crate) fn number_of_links(&self) -> Option<u32> {
        self.number_of_links
    }
    pub(crate) fn file_index(&self) -> Option<u64> {
        self.file_index
    }

    /// A virtual entry of a git run (Relax's VFS): a read-only file of its size, or a folder.
    fn virtual_entry(k: &vfs::Kind) -> Metadata {
        let (attributes, file_size) = match k {
            vfs::Kind::File(n) => (FILE_ATTRIBUTE_READONLY, *n),
            _ => (FILE_ATTRIBUTE_DIRECTORY, 0),
        };
        Metadata {
            attributes,
            creation_time: 0,
            last_access_time: 0,
            last_write_time: 0,
            change_time: None,
            file_size,
            reparse_tag: 0,
            volume_serial_number: None,
            number_of_links: None,
            file_index: None,
        }
    }
    fn from_find_data(d: &FindData) -> Metadata {
        Metadata {
            attributes: d.attributes,
            creation_time: d.creation_time,
            last_access_time: d.last_access_time,
            last_write_time: d.last_write_time,
            change_time: None,
            file_size: d.file_size,
            // dwReserved0 holds the tag only on reparse points
            reparse_tag: if d.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 { d.reserved0 } else { 0 },
            volume_serial_number: None,
            number_of_links: None,
            file_index: None,
        }
    }
}

impl fmt::Debug for Metadata {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Metadata")
            .field("file_type", &self.file_type())
            .field("permissions", &self.permissions())
            .field("len", &self.len())
            .finish_non_exhaustive()
    }
}

/// Times to set on a file (File::set_times).
#[derive(Copy, Clone, Debug, Default)]
pub struct FileTimes {
    accessed: Option<SystemTime>,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
}

impl FileTimes {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn set_accessed(mut self, t: SystemTime) -> Self {
        self.accessed = Some(t);
        self
    }
    pub fn set_modified(mut self, t: SystemTime) -> Self {
        self.modified = Some(t);
        self
    }
    // std::os::windows::fs::FileTimesExt
    pub(crate) fn with_created(mut self, t: SystemTime) -> Self {
        self.created = Some(t);
        self
    }
}

impl FileType {
    fn new(attributes: u32, reparse_tag: u32) -> FileType {
        let is_directory = attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
        // a name-surrogate reparse point (symlink, junction) counts as a symlink
        let is_symlink = attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 && reparse_tag & 0x2000_0000 != 0;
        FileType { is_directory, is_symlink }
    }
    pub fn is_dir(&self) -> bool {
        !self.is_symlink && self.is_directory
    }
    pub fn is_file(&self) -> bool {
        !self.is_symlink && !self.is_directory
    }
    pub fn is_symlink(&self) -> bool {
        self.is_symlink
    }
    // std::os::windows::fs::FileTypeExt
    pub(crate) fn is_symlink_dir(&self) -> bool {
        self.is_symlink && self.is_directory
    }
    pub(crate) fn is_symlink_file(&self) -> bool {
        self.is_symlink && !self.is_directory
    }
}

impl fmt::Debug for FileType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("FileType")
            .field("is_file", &self.is_file())
            .field("is_dir", &self.is_dir())
            .field("is_symlink", &self.is_symlink())
            .finish_non_exhaustive()
    }
}

impl Permissions {
    pub fn readonly(&self) -> bool {
        self.attrs & FILE_ATTRIBUTE_READONLY != 0
    }
    pub fn set_readonly(&mut self, readonly: bool) {
        if readonly {
            self.attrs |= FILE_ATTRIBUTE_READONLY;
        } else {
            self.attrs &= !FILE_ATTRIBUTE_READONLY;
        }
    }
    pub(crate) fn file_attributes(&self) -> u32 {
        self.attrs
    }
    pub(crate) fn set_file_attributes(&mut self, attrs: u32) {
        self.attrs = attrs;
    }
    pub(crate) fn from_file_attributes(attrs: u32) -> Permissions {
        Permissions { attrs }
    }
}

struct FilePermissionsDebug(u32);

impl fmt::Debug for FilePermissionsDebug {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("FilePermissions").field("attrs", &self.0).finish()
    }
}

impl fmt::Debug for Permissions {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // real std: Permissions(FilePermissions { attrs }) (both derived)
        f.debug_tuple("Permissions").field(&FilePermissionsDebug(self.attrs)).finish()
    }
}

// ---- directories

/// The fields of a WIN32_FIND_DATAW that std uses.
#[derive(Clone)]
struct FindData {
    attributes: u32,
    creation_time: u64,
    last_access_time: u64,
    last_write_time: u64,
    file_size: u64,
    reserved0: u32,
    name: Vec<u16>,
}

impl FindData {
    fn from_raw(wfd: &WIN32_FIND_DATAW) -> FindData {
        let mut n = 0;
        while n < wfd.cFileName.len() && wfd.cFileName[n] != 0 {
            n += 1;
        }
        FindData {
            attributes: wfd.dwFileAttributes,
            creation_time: filetime_u64(&wfd.ftCreationTime),
            last_access_time: filetime_u64(&wfd.ftLastAccessTime),
            last_write_time: filetime_u64(&wfd.ftLastWriteTime),
            file_size: ((wfd.nFileSizeHigh as u64) << 32) | (wfd.nFileSizeLow as u64),
            reserved0: wfd.dwReserved0,
            name: wfd.cFileName[..n].to_vec(),
        }
    }
    fn is_dot_or_dotdot(&self) -> bool {
        let n = &self.name;
        (n.len() == 1 && n[0] == DOT) || (n.len() == 2 && n[0] == DOT && n[1] == DOT)
    }
}

fn zeroed_find_data() -> WIN32_FIND_DATAW {
    unsafe { core::mem::zeroed() }
}

struct FindHandle(HANDLE);

unsafe impl Send for FindHandle {}
unsafe impl Sync for FindHandle {}

impl Drop for FindHandle {
    fn drop(&mut self) {
        unsafe {
            FindClose(self.0);
        }
    }
}

pub struct ReadDir {
    handle: Option<FindHandle>,
    root: Arc<PathBuf>,
    first: Option<FindData>,
    /// a virtual folder's entries (Relax's VFS), last first
    virt: Vec<FindData>,
}

pub struct DirEntry {
    root: Arc<PathBuf>,
    data: FindData,
}

impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;
    fn next(&mut self) -> Option<io::Result<DirEntry>> {
        let h = match &self.handle {
            Some(h) => h.0,
            // FindFirstFileExW found nothing in an existing directory; or a virtual folder
            None => return self.virt.pop().map(|data| Ok(DirEntry { root: self.root.clone(), data })),
        };
        if let Some(first) = self.first.take() {
            if !first.is_dot_or_dotdot() {
                return Some(Ok(DirEntry { root: self.root.clone(), data: first }));
            }
        }
        let mut wfd = zeroed_find_data();
        loop {
            if unsafe { FindNextFileW(h, &mut wfd) } == 0 {
                self.handle = None;
                let e = last_error();
                if e == ERROR_NO_MORE_FILES {
                    return None;
                }
                return Some(Err(os_error(e)));
            }
            let d = FindData::from_raw(&wfd);
            if !d.is_dot_or_dotdot() {
                return Some(Ok(DirEntry { root: self.root.clone(), data: d }));
            }
        }
    }
}

impl fmt::Debug for ReadDir {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_tuple("ReadDir").field(&*self.root).finish()
    }
}

impl DirEntry {
    pub fn path(&self) -> PathBuf {
        self.root.join(self.file_name())
    }
    pub fn file_name(&self) -> OsString {
        OsString::from_wide(&self.data.name)
    }
    /// From the directory listing (no extra call, as real std on Windows).
    pub fn metadata(&self) -> io::Result<Metadata> {
        Ok(Metadata::from_find_data(&self.data))
    }
    pub fn file_type(&self) -> io::Result<FileType> {
        Ok(FileType::new(self.data.attributes, self.data.reserved0))
    }
}

impl fmt::Debug for DirEntry {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_tuple("DirEntry").field(&self.path()).finish()
    }
}

#[derive(Debug)]
pub struct DirBuilder {
    recursive: bool,
}

impl DirBuilder {
    pub fn new() -> DirBuilder {
        DirBuilder { recursive: false }
    }
    pub fn recursive(&mut self, recursive: bool) -> &mut DirBuilder {
        self.recursive = recursive;
        self
    }
    pub fn create<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        let path = path.as_ref();
        if self.recursive {
            self.create_dir_all(path)
        } else {
            self.mkdir(path)
        }
    }
    fn mkdir(&self, path: &Path) -> io::Result<()> {
        let p = maybe_verbatim(path)?;
        cvt(unsafe { CreateDirectoryW(p.as_ptr(), core::ptr::null()) })?;
        Ok(())
    }
    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        if path == Path::new("") {
            return Ok(());
        }
        match self.mkdir(path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                if e.kind() != io::ErrorKind::NotFound {
                    if path.is_dir() {
                        return Ok(());
                    }
                    return Err(e);
                }
            }
        }
        match path.parent() {
            Some(p) => self.create_dir_all(p)?,
            None => return Err(io::const_error!(io::ErrorKind::Uncategorized, "failed to create whole tree")),
        }
        match self.mkdir(path) {
            Ok(()) => Ok(()),
            Err(e) => {
                if path.is_dir() {
                    Ok(())
                } else {
                    Err(e)
                }
            }
        }
    }
}

impl Default for DirBuilder {
    fn default() -> DirBuilder {
        DirBuilder::new()
    }
}

// ---- free functions

pub fn read<P: AsRef<Path>>(path: P) -> io::Result<Vec<u8>> {
    let mut file = File::open(path.as_ref())?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub fn read_to_string<P: AsRef<Path>>(path: P) -> io::Result<String> {
    let mut file = File::open(path.as_ref())?;
    let mut s = String::new();
    file.read_to_string(&mut s)?;
    Ok(s)
}

pub fn write<P: AsRef<Path>, C: AsRef<[u8]>>(path: P, contents: C) -> io::Result<()> {
    File::create(path.as_ref())?.write_all(contents.as_ref())
}

/// Opens `path` (NUL-terminated) for metadata only: no access rights, directories allowed.
fn open_for_meta(path: &[u16], extra_flags: u32) -> io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.set_access_mode(0);
    opts.set_custom_flags(FILE_FLAG_BACKUP_SEMANTICS | extra_flags);
    opts.open_native(path)
}

/// real std's sys/fs/windows.rs metadata(): through a handle; a locked or protected file falls
/// back to its (possibly stale) directory entry.
fn stat_native(path: &[u16], reparse_flag: u32) -> io::Result<Metadata> {
    match open_for_meta(path, reparse_flag) {
        Ok(f) => f.metadata(),
        Err(e) => {
            let code = e.raw_os_error();
            if code != Some(ERROR_SHARING_VIOLATION as i32) && code != Some(ERROR_ACCESS_DENIED as i32) {
                return Err(e);
            }
            let mut wfd = zeroed_find_data();
            let h = unsafe { FindFirstFileExW(path.as_ptr(), FindExInfoBasic, &mut wfd as *mut WIN32_FIND_DATAW as *mut c_void, FindExSearchNameMatch, core::ptr::null(), 0) };
            if h == INVALID_HANDLE_VALUE {
                return Err(e);
            }
            unsafe {
                FindClose(h);
            }
            let m = Metadata::from_find_data(&FindData::from_raw(&wfd));
            if reparse_flag == 0 && m.file_type().is_symlink() {
                Err(e)
            } else {
                Ok(m)
            }
        }
    }
}

pub fn metadata<P: AsRef<Path>>(path: P) -> io::Result<Metadata> {
    if let Some((_, k)) = vfs::kind(path.as_ref()) {
        return match k {
            vfs::Kind::Missing => Err(io::Error::from(io::ErrorKind::NotFound)),
            k => Ok(Metadata::virtual_entry(&k)),
        };
    }
    let p = maybe_verbatim(path.as_ref())?;
    match stat_native(&p, 0) {
        Err(e) => {
            if e.raw_os_error() == Some(ERROR_CANT_ACCESS_FILE as i32) {
                // a reparse point CreateFileW cannot follow (an app exec link, a unix socket)
                if let Ok(m) = stat_native(&p, FILE_FLAG_OPEN_REPARSE_POINT) {
                    if !m.file_type().is_symlink() {
                        return Ok(m);
                    }
                }
            }
            Err(e)
        }
        Ok(m) => Ok(m),
    }
}

pub fn symlink_metadata<P: AsRef<Path>>(path: P) -> io::Result<Metadata> {
    if vfs::kind(path.as_ref()).is_some() {
        return metadata(path);
    }
    let p = maybe_verbatim(path.as_ref())?;
    stat_native(&p, FILE_FLAG_OPEN_REPARSE_POINT)
}

pub fn exists<P: AsRef<Path>>(path: P) -> io::Result<bool> {
    if let Some((_, k)) = vfs::kind(path.as_ref()) {
        return Ok(!matches!(k, vfs::Kind::Missing));
    }
    let p = maybe_verbatim(path.as_ref())?;
    match open_for_meta(&p, 0) {
        Ok(_) => Ok(true),
        Err(e) => {
            if e.kind() == io::ErrorKind::NotFound {
                Ok(false)
            } else if e.raw_os_error() == Some(ERROR_SHARING_VIOLATION as i32) || e.raw_os_error() == Some(ERROR_CANT_ACCESS_FILE as i32) {
                Ok(true)
            } else {
                Err(e)
            }
        }
    }
}

impl File {
    /// Delete on close with POSIX semantics, ignoring the readonly attribute.
    fn posix_delete(&self) -> io::Result<()> {
        let info = FILE_DISPOSITION_INFO_EX { Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE };
        set_info(self.raw(), FileDispositionInfoEx, &info)
    }
}

fn unlink_native(p: &[u16]) -> io::Result<()> {
    if unsafe { DeleteFileW(p.as_ptr()) } != 0 {
        return Ok(());
    }
    let err = last_error();
    // a readonly file: delete through a handle, ignoring the attribute (real std)
    if err == ERROR_ACCESS_DENIED {
        let mut opts = OpenOptions::new();
        opts.set_access_mode(DELETE);
        opts.set_custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        if let Ok(f) = opts.open_native(p) {
            if f.posix_delete().is_ok() {
                return Ok(());
            }
        }
    }
    Err(os_error(err))
}

pub fn remove_file<P: AsRef<Path>>(path: P) -> io::Result<()> {
    if let Some(disk) = vfs::edit_target(path.as_ref(), vfs::Edit::Existing) {
        return unlink_native(&maybe_verbatim(&disk?)?);
    }
    let p = maybe_verbatim(path.as_ref())?;
    unlink_native(&p)
}

/// MoveFileExW(MOVEFILE_REPLACE_EXISTING). Real std additionally retries an ACCESS_DENIED rename
/// of a readonly file through SetFileInformationByHandle(FileRenameInfoEx); not done here.
pub fn rename<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> io::Result<()> {
    // within a virtual folder: between the edited files on disk
    let from = match vfs::edit_target(from.as_ref(), vfs::Edit::Existing) {
        Some(d) => d?,
        None => from.as_ref().to_path_buf(),
    };
    let to = match vfs::edit_target(to.as_ref(), vfs::Edit::Replace) {
        Some(d) => d?,
        None => to.as_ref().to_path_buf(),
    };
    let a = maybe_verbatim(&from)?;
    let b = maybe_verbatim(&to)?;
    cvt(unsafe { MoveFileExW(a.as_ptr(), b.as_ptr(), MOVEFILE_REPLACE_EXISTING) })?;
    Ok(())
}

pub fn hard_link<P: AsRef<Path>, Q: AsRef<Path>>(original: P, link: Q) -> io::Result<()> {
    let a = maybe_verbatim(original.as_ref())?;
    let b = maybe_verbatim(link.as_ref())?;
    cvt(unsafe { CreateHardLinkW(b.as_ptr(), a.as_ptr(), core::ptr::null()) })?;
    Ok(())
}

unsafe extern "system" fn copy_progress(_total: i64, _transferred: i64, _stream_size: i64, stream_transferred: i64, stream_number: u32, _reason: u32, _src: HANDLE, _dst: HANDLE, data: *const c_void) -> u32 {
    if stream_number == 1 {
        *(data as *mut i64) = stream_transferred;
    }
    PROGRESS_CONTINUE
}

/// CopyFileExW (attributes, streams and times copied by the OS); the result is the size of the
/// main stream (real std).
pub fn copy<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> io::Result<u64> {
    let a = maybe_verbatim(from.as_ref())?;
    let b = maybe_verbatim(to.as_ref())?;
    let mut size: i64 = 0;
    cvt(unsafe { CopyFileExW(a.as_ptr(), b.as_ptr(), Some(copy_progress), &mut size as *mut i64 as *const c_void, core::ptr::null_mut(), 0) })?;
    Ok(size as u64)
}

pub fn create_dir<P: AsRef<Path>>(path: P) -> io::Result<()> {
    // in a virtual folder: in the edits folder (without one, the disk as before)
    if let Some(Ok(disk)) = vfs::edit_target(path.as_ref(), vfs::Edit::Dir) {
        return DirBuilder::new().recursive(true).create(&disk);
    }
    DirBuilder::new().create(path.as_ref())
}

pub fn create_dir_all<P: AsRef<Path>>(path: P) -> io::Result<()> {
    if let Some(Ok(disk)) = vfs::edit_target(path.as_ref(), vfs::Edit::Dir) {
        return DirBuilder::new().recursive(true).create(&disk);
    }
    DirBuilder::new().recursive(true).create(path.as_ref())
}

pub fn remove_dir<P: AsRef<Path>>(path: P) -> io::Result<()> {
    let p = maybe_verbatim(path.as_ref())?;
    cvt(unsafe { RemoveDirectoryW(p.as_ptr()) })?;
    Ok(())
}

/// Removes a directory tree without following links. The path must be a directory or a
/// directory link (else ERROR_DIRECTORY, as real std); a directory link is removed itself.
/// Real std walks the tree through handles (remove_dir_all_iterative); this walks it by path.
pub fn remove_dir_all<P: AsRef<Path>>(path: P) -> io::Result<()> {
    let path = path.as_ref();
    let p = maybe_verbatim(path)?;
    let mut opts = OpenOptions::new();
    opts.set_access_mode(FILE_LIST_DIRECTORY);
    opts.set_custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    let file = opts.open_native(&p)?;
    let attrs = file.basic_info()?.FileAttributes;
    if attrs & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(os_error(ERROR_DIRECTORY));
    }
    let is_link = file.metadata()?.file_type().is_symlink();
    drop(file);
    if is_link {
        return remove_dir(path);
    }
    remove_dir_all_recursive(path)
}

fn remove_dir_all_recursive(path: &Path) -> io::Result<()> {
    for child in read_dir(path)? {
        let child = child?;
        let child_path = child.path();
        let ft = child.file_type()?;
        let r = if ft.is_dir() {
            remove_dir_all_recursive(&child_path)
        } else if ft.is_symlink_dir() {
            remove_dir(&child_path)
        } else {
            remove_file(&child_path)
        };
        if let Err(e) = r {
            if e.kind() != io::ErrorKind::NotFound {
                return Err(e);
            }
        }
    }
    match remove_dir(path) {
        Ok(()) => Ok(()),
        Err(e) => {
            if e.kind() == io::ErrorKind::NotFound {
                Ok(())
            } else {
                Err(e)
            }
        }
    }
}

pub fn read_dir<P: AsRef<Path>>(path: P) -> io::Result<ReadDir> {
    let p = path.as_ref();
    if let Some((abs, k)) = vfs::kind(p) {
        return match k {
            vfs::Kind::Dir => {
                let mut virt = Vec::new();
                for (name, is_dir) in vfs::list(&abs)? {
                    let name_s = String::from_utf8_lossy(&name).into_owned();
                    let file_size = if is_dir {
                        0
                    } else {
                        match vfs::kind(&abs.join(&name_s)) {
                            Some((_, vfs::Kind::File(n))) => n,
                            _ => 0,
                        }
                    };
                    virt.push(FindData {
                        attributes: if is_dir { FILE_ATTRIBUTE_DIRECTORY } else { FILE_ATTRIBUTE_READONLY },
                        creation_time: 0,
                        last_access_time: 0,
                        last_write_time: 0,
                        file_size,
                        reserved0: 0,
                        name: name_s.encode_utf16().collect(),
                    });
                }
                virt.reverse();
                // entries name the path as given (a relative path stays relative, as with the disk)
                Ok(ReadDir { handle: None, root: Arc::new(p.to_path_buf()), first: None, virt })
            }
            vfs::Kind::File(_) => Err(os_error(ERROR_DIRECTORY)),
            vfs::Kind::Missing => Err(io::Error::from(io::ErrorKind::NotFound)),
        };
    }
    // `*` appended to "" would list the current directory: an error like other opens
    if p.as_os_str().is_empty() {
        return Err(os_error(ERROR_PATH_NOT_FOUND));
    }
    let root = p.to_path_buf();
    let star = p.join("*");
    let w = maybe_verbatim(&star)?;
    let mut wfd = zeroed_find_data();
    let h = unsafe { FindFirstFileExW(w.as_ptr(), FindExInfoBasic, &mut wfd as *mut WIN32_FIND_DATAW as *mut c_void, FindExSearchNameMatch, core::ptr::null(), 0) };
    if h != INVALID_HANDLE_VALUE {
        return Ok(ReadDir { handle: Some(FindHandle(h)), root: Arc::new(root), first: Some(FindData::from_raw(&wfd)), virt: Vec::new() });
    }
    let e = last_error();
    // no match in an existing directory: an empty listing (real std, #120040)
    if e == ERROR_FILE_NOT_FOUND {
        return Ok(ReadDir { handle: None, root: Arc::new(root), first: None, virt: Vec::new() });
    }
    Err(os_error(e))
}

/// The final path of the opened file, in its `\\?\` form (real std).
pub fn canonicalize<P: AsRef<Path>>(path: P) -> io::Result<PathBuf> {
    if let Some((abs, k)) = vfs::kind(path.as_ref()) {
        if matches!(k, vfs::Kind::Missing) {
            return Err(io::Error::from(io::ErrorKind::NotFound));
        }
        // lexically: a virtual folder has no links; no verbatim prefix (the loader's own spelling)
        let mut out = PathBuf::new();
        for c in abs.components() {
            match c {
                crate::path::Component::ParentDir => {
                    out.pop();
                }
                crate::path::Component::CurDir => {}
                c => out.push(c.as_os_str()),
            }
        }
        return Ok(out);
    }
    let p = maybe_verbatim(path.as_ref())?;
    let f = open_for_meta(&p, 0)?;
    get_path(f.raw())
}

pub fn read_link<P: AsRef<Path>>(path: P) -> io::Result<PathBuf> {
    let p = maybe_verbatim(path.as_ref())?;
    // no access rights: FILE_LIST_DIRECTORY is denied on some junctions
    let f = open_for_meta(&p, FILE_FLAG_OPEN_REPARSE_POINT)?;
    // u64 storage: REPARSE_DATA_BUFFER needs 4-byte alignment
    let mut space: Vec<u64> = alloc::vec![0u64; MAXIMUM_REPARSE_DATA_BUFFER_SIZE / 8];
    let mut bytes: DWORD = 0;
    cvt(unsafe { DeviceIoControl(f.raw(), FSCTL_GET_REPARSE_POINT, core::ptr::null(), 0, space.as_mut_ptr() as *mut c_void, MAXIMUM_REPARSE_DATA_BUFFER_SIZE as DWORD, &mut bytes, core::ptr::null_mut()) })?;
    let buf = space.as_ptr() as *const u8;
    let rd16 = |off: usize| -> u16 { unsafe { core::ptr::read_unaligned(buf.add(off) as *const u16) } };
    let rd32 = |off: usize| -> u32 { unsafe { core::ptr::read_unaligned(buf.add(off) as *const u32) } };
    // REPARSE_DATA_BUFFER: ReparseTag u32, ReparseDataLength u16, Reserved u16, then
    // SubstituteNameOffset u16, SubstituteNameLength u16, PrintNameOffset u16, PrintNameLength u16,
    // (symlinks: Flags u32), PathBuffer
    let tag = rd32(0);
    let (path_buffer, relative) = if tag == IO_REPARSE_TAG_SYMLINK {
        (20usize, rd32(16) & SYMLINK_FLAG_RELATIVE != 0)
    } else if tag == IO_REPARSE_TAG_MOUNT_POINT {
        (16usize, false)
    } else {
        return Err(io::const_error!(io::ErrorKind::Uncategorized, "Unsupported reparse point type"));
    };
    let subst_off = rd16(8) as usize / 2;
    let subst_len = rd16(10) as usize / 2;
    let mut subst: Vec<u16> = Vec::with_capacity(subst_len + 1);
    let mut i = 0;
    while i < subst_len {
        subst.push(rd16(path_buffer + (subst_off + i) * 2));
        i += 1;
    }
    // an absolute target starts with the NT prefix `\??\`: shown as `\\?\`, then in user form
    if !relative && subst.len() >= 4 && subst[0] == SEP && subst[1] == QUERY && subst[2] == QUERY && subst[3] == SEP {
        subst[1] = SEP;
        subst.push(0);
        let user = from_wide_to_user_path(subst)?;
        let n = if user.last() == Some(&0) { user.len() - 1 } else { user.len() };
        Ok(os2path(&user[..n]))
    } else {
        Ok(os2path(&subst))
    }
}

pub fn set_permissions<P: AsRef<Path>>(path: P, perm: Permissions) -> io::Result<()> {
    let p = maybe_verbatim(path.as_ref())?;
    cvt(unsafe { SetFileAttributesW(p.as_ptr(), perm.attrs) })?;
    Ok(())
}

/// CreateSymbolicLinkW for std::os::windows::fs::symlink_file / symlink_dir: unprivileged
/// creation (developer mode) requested, retried without the flag on older Windows (real std).
pub(crate) fn symlink_inner(original: &Path, link: &Path, dir: bool) -> io::Result<()> {
    let original = to_wide(original.as_os_str())?;
    let link = maybe_verbatim(link)?;
    let flags = if dir { SYMBOLIC_LINK_FLAG_DIRECTORY } else { 0 };
    if unsafe { CreateSymbolicLinkW(link.as_ptr(), original.as_ptr(), flags | SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE) } != 0 {
        return Ok(());
    }
    let e = last_error();
    if e != ERROR_INVALID_PARAMETER {
        return Err(os_error(e));
    }
    if unsafe { CreateSymbolicLinkW(link.as_ptr(), original.as_ptr(), flags) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
