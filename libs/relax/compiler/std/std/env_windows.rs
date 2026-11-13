//! std::env for Windows: the process environment through kernel32's wide-string API (no lock:
//! the Win32 environment calls are thread-safe), args as the runtime recorded them, directories
//! and `;`-separated PATH lists. Semantics follow real std's sys/env/windows.rs and
//! sys/paths/windows.rs.

#![allow(non_snake_case)]

use alloc::string::String;
use alloc::vec::Vec;
use core::error::Error;
use core::ffi::{c_char, c_void};
use core::fmt;

use crate::ffi::{OsStr, OsString};
use crate::io;
use crate::os::windows::ffi::OsStringExt;
use crate::path::{Path, PathBuf};
use crate::sys;
use crate::sys::os::{cvt, fill_utf16_buf, to_wide, BOOL, DWORD, HANDLE};
use crate::sys::os::{GetLastError, GetModuleFileNameW, GetModuleHandleW};

#[link(name = "kernel32")]
extern "system" {
    fn GetEnvironmentVariableW(name: *const u16, buf: *mut u16, n: DWORD) -> DWORD;
    fn SetEnvironmentVariableW(name: *const u16, value: *const u16) -> BOOL;
    fn GetCurrentDirectoryW(n: DWORD, buf: *mut u16) -> DWORD;
    fn SetCurrentDirectoryW(path: *const u16) -> BOOL;
    fn GetTempPathW(n: DWORD, buf: *mut u16) -> DWORD;
    fn GetProcAddress(module: HANDLE, name: *const c_char) -> *mut c_void;
}

#[link(name = "userenv")]
extern "system" {
    fn GetUserProfileDirectoryW(token: HANDLE, dir: *mut u16, size: *mut DWORD) -> BOOL;
}

const ERROR_INSUFFICIENT_BUFFER: DWORD = 122;

fn os2path(s: &[u16]) -> PathBuf {
    PathBuf::from(OsString::from_wide(s))
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum VarError {
    NotPresent,
    NotUnicode(OsString),
}

impl fmt::Display for VarError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            VarError::NotPresent => f.write_str("environment variable not found"),
            VarError::NotUnicode(s) => write!(f, "environment variable was not valid unicode: {:?}", s),
        }
    }
}

impl Error for VarError {}

extern "C" {
    fn relax_pm_getenv(key: *const u8, len: usize, out: *mut *const u8, out_len: *mut usize) -> i32;
    // the program's snapshot and std's host-only plumbing (relax-base progenv.rs)
    fn __relax_env_snapshot(out: *mut *const u8, out_len: *mut usize);
    fn __relax_host_getenv_w(name: *const u16, buf: *mut u16, size: u32) -> u32;
}

pub fn var_os<K: AsRef<OsStr>>(key: K) -> Option<OsString> {
    if let Some(ks) = key.as_ref().to_str() {
        let kb = ks.as_bytes();
        let mut pm_out: *const u8 = core::ptr::null();
        let mut pm_len: usize = 0;
        if unsafe { relax_pm_getenv(kb.as_ptr(), kb.len(), &mut pm_out, &mut pm_len) } == 0 && !pm_out.is_null() {
            if let Ok(s) = core::str::from_utf8(unsafe { core::slice::from_raw_parts(pm_out, pm_len) }) {
                return Some(OsString::from(s));
            }
        }
    }
    let k = match to_wide(key.as_ref()) {
        Ok(k) => k,
        Err(_) => return None,
    };
    match fill_utf16_buf(|buf, n| unsafe { GetEnvironmentVariableW(k.as_ptr(), buf, n) }) {
        Ok(v) => Some(OsString::from_wide(&v)),
        Err(_) => None,
    }
}

/// The host's value of `key`, for std's plumbing, apart from the program's own values.
pub(crate) fn host_var_os(key: &str) -> Option<OsString> {
    let k = to_wide(OsStr::new(key)).ok()?;
    let v = fill_utf16_buf(|buf, n| unsafe { __relax_host_getenv_w(k.as_ptr(), buf, n) }).ok()?;
    Some(OsString::from_wide(&v))
}

pub fn var<K: AsRef<OsStr>>(key: K) -> Result<String, VarError> {
    match var_os(key) {
        Some(s) => match s.into_string() {
            Ok(s) => Ok(s),
            Err(s) => Err(VarError::NotUnicode(s)),
        },
        None => Err(VarError::NotPresent),
    }
}

/// Unsafe as real std's: a call needs no unsafe block before edition 2024 (`deprecated_safe_2024`).
#[rustc_deprecated_safe_2024(audit_that = "the environment access only happens in single-threaded code")]
pub unsafe fn set_var<K: AsRef<OsStr>, V: AsRef<OsStr>>(key: K, value: V) {
    let (key, value) = (key.as_ref(), value.as_ref());
    let r = match (to_wide(key), to_wide(value)) {
        (Ok(k), Ok(v)) => match cvt(unsafe { SetEnvironmentVariableW(k.as_ptr(), v.as_ptr()) }) {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        },
        (Err(e), _) => Err(e),
        (_, Err(e)) => Err(e),
    };
    if let Err(e) = r {
        panic!("failed to set environment variable `{:?}` to `{:?}`: {}", key, value, e);
    }
}

/// Unsafe as real std's: a call needs no unsafe block before edition 2024 (`deprecated_safe_2024`).
#[rustc_deprecated_safe_2024(audit_that = "the environment access only happens in single-threaded code")]
pub unsafe fn remove_var<K: AsRef<OsStr>>(key: K) {
    let key = key.as_ref();
    let r = match to_wide(key) {
        Ok(k) => match cvt(unsafe { SetEnvironmentVariableW(k.as_ptr(), core::ptr::null()) }) {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        },
        Err(e) => Err(e),
    };
    if let Err(e) = r {
        panic!("failed to remove environment variable `{:?}`: {}", key, e);
    }
}

pub struct VarsOs {
    inner: alloc::vec::IntoIter<(OsString, OsString)>,
}

pub struct Vars {
    inner: VarsOs,
}

/// The program's snapshot (relax-base progenv.rs): byte length, `:`, `NAME=value`, as WTF-8.
pub fn vars_os() -> VarsOs {
    let mut p: *const u8 = core::ptr::null();
    let mut n: usize = 0;
    unsafe { __relax_env_snapshot(&mut p, &mut n) };
    let mut s: &[u8] = if p.is_null() { &[] } else { unsafe { core::slice::from_raw_parts(p, n) } };
    let mut out = Vec::new();
    while let Some(colon) = s.iter().position(|&b| b == b':') {
        let mut len: usize = 0;
        for &d in &s[..colon] {
            len = len * 10 + (d - b'0') as usize;
        }
        let e = &s[colon + 1..colon + 1 + len];
        // a leading '=' is part of the key, as in `=C:=C:\dir`
        let mut i = 1;
        while i < e.len() && e[i] != b'=' {
            i += 1;
        }
        if i < e.len() {
            out.push((OsString::from_bytes_vec(e[..i].to_vec()), OsString::from_bytes_vec(e[i + 1..].to_vec())));
        }
        s = &s[colon + 1 + len..];
    }
    VarsOs { inner: out.into_iter() }
}

pub fn vars() -> Vars {
    Vars { inner: vars_os() }
}

impl Iterator for VarsOs {
    type Item = (OsString, OsString);
    fn next(&mut self) -> Option<(OsString, OsString)> {
        self.inner.next()
    }
}

impl Iterator for Vars {
    type Item = (String, String);
    fn next(&mut self) -> Option<(String, String)> {
        match self.inner.next() {
            Some((a, b)) => Some((a.into_string().unwrap(), b.into_string().unwrap())),
            None => None,
        }
    }
}

impl fmt::Debug for VarsOs {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("VarsOs").finish_non_exhaustive()
    }
}

impl fmt::Debug for Vars {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Vars").finish_non_exhaustive()
    }
}

// ---- args (UTF-8 C strings recorded by the runtime at boot)

fn c_bytes(p: *const c_char) -> Vec<u8> {
    let mut n = 0;
    unsafe {
        while *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p as *const u8, n).to_vec()
    }
}

pub struct ArgsOs {
    inner: alloc::vec::IntoIter<OsString>,
}

pub struct Args {
    inner: ArgsOs,
}

pub fn args_os() -> ArgsOs {
    let (argc, argv) = sys::os::args();
    let mut out = Vec::with_capacity(argc);
    let mut i = 0;
    while i < argc {
        unsafe {
            let p = *argv.add(i);
            if p.is_null() {
                break;
            }
            out.push(OsString::from_bytes_vec(c_bytes(p)));
        }
        i += 1;
    }
    ArgsOs { inner: out.into_iter() }
}

pub fn args() -> Args {
    Args { inner: args_os() }
}

impl Iterator for ArgsOs {
    type Item = OsString;
    fn next(&mut self) -> Option<OsString> {
        self.inner.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for ArgsOs {}

impl DoubleEndedIterator for ArgsOs {
    fn next_back(&mut self) -> Option<OsString> {
        self.inner.next_back()
    }
}

impl Iterator for Args {
    type Item = String;
    fn next(&mut self) -> Option<String> {
        match self.inner.next() {
            Some(s) => Some(s.into_string().unwrap()),
            None => None,
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for Args {}

impl DoubleEndedIterator for Args {
    fn next_back(&mut self) -> Option<String> {
        match self.inner.next_back() {
            Some(s) => Some(s.into_string().unwrap()),
            None => None,
        }
    }
}

impl fmt::Debug for ArgsOs {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ArgsOs").field("args", &self.inner.as_slice()).finish()
    }
}

impl fmt::Debug for Args {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Args").field("args", &self.inner.inner.as_slice()).finish()
    }
}

// ---- directories

pub fn current_dir() -> io::Result<PathBuf> {
    // a git repository run (Relax's VFS): the virtual workspace folder
    if let Some(d) = crate::fs::vfs::cwd() {
        return Ok(d);
    }
    let w = fill_utf16_buf(|buf, n| unsafe { GetCurrentDirectoryW(n, buf) })?;
    Ok(os2path(&w))
}

pub fn set_current_dir<P: AsRef<Path>>(path: P) -> io::Result<()> {
    let target = match crate::fs::vfs::set_cwd(path.as_ref()) {
        Ok(virtual_result) => return virtual_result,
        Err(target) => target,
    };
    let w = to_wide(target.as_os_str())?;
    cvt(unsafe { SetCurrentDirectoryW(w.as_ptr()) })?;
    crate::fs::vfs::chose_real();
    Ok(())
}

/// Under `makepad run`, the program's own executable: Relax's hard link for this run (Relax
/// selfexe.rs), the first word of RELAX_SELF_EXEC. The process itself runs the loader's file.
fn run_exe() -> Option<PathBuf> {
    let pre = host_var_os("RELAX_SELF_EXEC")?;
    let first = pre.as_encoded_bytes().split(|&b| b == 0x1f).next()?;
    (!first.is_empty()).then(|| PathBuf::from(unsafe { OsString::from_encoded_bytes_unchecked(first.to_vec()) }))
}

pub fn current_exe() -> io::Result<PathBuf> {
    if let Some(p) = run_exe() {
        return Ok(p);
    }
    let w = fill_utf16_buf(|buf, n| unsafe { GetModuleFileNameW(core::ptr::null_mut(), buf, n) })?;
    Ok(os2path(&w))
}

/// GetTempPath2W (Windows 11 / Server 2022: a per-user directory also for SYSTEM processes),
/// GetTempPathW where kernel32 lacks it (real std's compat fallback).
pub fn temp_dir() -> PathBuf {
    let kernel32: [u16; 13] = [b'k' as u16, b'e' as u16, b'r' as u16, b'n' as u16, b'e' as u16, b'l' as u16, b'3' as u16, b'2' as u16, b'.' as u16, b'd' as u16, b'l' as u16, b'l' as u16, 0];
    let name = b"GetTempPath2W\0";
    let f = unsafe {
        let m = GetModuleHandleW(kernel32.as_ptr());
        if m.is_null() {
            core::ptr::null_mut()
        } else {
            GetProcAddress(m, name.as_ptr() as *const c_char)
        }
    };
    let w = if f.is_null() {
        fill_utf16_buf(|buf, n| unsafe { GetTempPathW(n, buf) })
    } else {
        let get: unsafe extern "system" fn(DWORD, *mut u16) -> DWORD = unsafe { core::mem::transmute(f) };
        fill_utf16_buf(|buf, n| unsafe { get(n, buf) })
    };
    os2path(&w.unwrap())
}

/// The profile directory of the process token (GetUserProfileDirectoryW speaks its own buffer
/// protocol: the size includes the NUL).
fn home_dir_crt() -> Option<PathBuf> {
    const CURRENT_PROCESS_TOKEN: isize = -4;
    let r = fill_utf16_buf(|buf, n| {
        let mut sz = n;
        if unsafe { GetUserProfileDirectoryW(CURRENT_PROCESS_TOKEN as HANDLE, buf, &mut sz) } == 0 {
            if unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER {
                0
            } else {
                sz
            }
        } else {
            sz - 1
        }
    });
    match r {
        Ok(w) => Some(os2path(&w)),
        Err(_) => None,
    }
}

/// USERPROFILE when set and non-empty, else the token's profile directory (real std).
pub fn home_dir() -> Option<PathBuf> {
    match var_os("USERPROFILE") {
        Some(p) => {
            if !p.is_empty() {
                return Some(PathBuf::from(p));
            }
        }
        None => {}
    }
    home_dir_crt()
}

// ---- PATH-style lists: `;`-separated, a `"..."` region may hold `;` (the quotes are dropped)

pub struct SplitPaths<'a> {
    rest: &'a [u8],
    must_yield: bool,
}

pub fn split_paths<T: AsRef<OsStr> + ?Sized>(unparsed: &T) -> SplitPaths<'_> {
    SplitPaths { rest: unparsed.as_ref().as_encoded_bytes(), must_yield: true }
}

impl<'a> Iterator for SplitPaths<'a> {
    type Item = PathBuf;
    fn next(&mut self) -> Option<PathBuf> {
        let must_yield = self.must_yield;
        self.must_yield = false;
        let mut part: Vec<u8> = Vec::new();
        let mut in_quote = false;
        let rest = self.rest;
        let mut i = 0;
        while i < rest.len() {
            let b = rest[i];
            i += 1;
            if b == b'"' {
                in_quote = !in_quote;
            } else if b == b';' && !in_quote {
                self.must_yield = true;
                break;
            } else {
                part.push(b);
            }
        }
        self.rest = &rest[i..];
        if !must_yield && part.is_empty() {
            None
        } else {
            Some(PathBuf::from(OsString::from_bytes_vec(part)))
        }
    }
}

#[derive(Debug)]
pub struct JoinPathsError;

impl fmt::Display for JoinPathsError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("path segment contains `\"`")
    }
}

impl Error for JoinPathsError {}

/// Joins with `;`; a path holding `;` is quoted, one holding `"` is an error (real std).
pub fn join_paths<I, T>(paths: I) -> Result<OsString, JoinPathsError>
where
    I: IntoIterator<Item = T>,
    T: AsRef<OsStr>,
{
    let mut out: Vec<u8> = Vec::new();
    let mut first = true;
    for p in paths {
        let b = p.as_ref().as_encoded_bytes();
        if !first {
            out.push(b';');
        }
        first = false;
        let mut has_sep = false;
        for c in b {
            if *c == b'"' {
                return Err(JoinPathsError);
            }
            if *c == b';' {
                has_sep = true;
            }
        }
        if has_sep {
            out.push(b'"');
            out.extend_from_slice(b);
            out.push(b'"');
        } else {
            out.extend_from_slice(b);
        }
    }
    Ok(OsString::from_bytes_vec(out))
}

pub mod consts {
    pub const ARCH: &str = if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86_64" };
    pub const FAMILY: &str = "windows";
    pub const OS: &str = "windows";
    pub const DLL_PREFIX: &str = "";
    pub const DLL_SUFFIX: &str = ".dll";
    pub const DLL_EXTENSION: &str = "dll";
    pub const EXE_SUFFIX: &str = ".exe";
    pub const EXE_EXTENSION: &str = "exe";
}
