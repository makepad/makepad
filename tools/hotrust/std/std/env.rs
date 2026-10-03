//! std::env: environment variables (behind a RwLock like real std), args, directories.

use alloc::ffi::CString;
use alloc::string::String;
use alloc::vec::Vec;
use core::error::Error;
use core::ffi::c_char;
use core::fmt;

use crate::ffi::{OsStr, OsString};
use crate::io;
use crate::path::{Path, PathBuf};
use crate::sync::RwLock;
use crate::sys;

static ENV_LOCK: RwLock<()> = RwLock::new(());

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

fn c_bytes(p: *const c_char) -> Vec<u8> {
    unsafe { core::slice::from_raw_parts(p as *const u8, sys::strlen(p)).to_vec() }
}

pub fn var_os<K: AsRef<OsStr>>(key: K) -> Option<OsString> {
    let key = match CString::new(key.as_ref().as_encoded_bytes()) {
        Ok(k) => k,
        Err(_) => return None,
    };
    let _guard = ENV_LOCK.read();
    let v = unsafe { sys::getenv(key.as_ptr()) };
    if v.is_null() {
        None
    } else {
        Some(OsString::from_bytes_vec(c_bytes(v)))
    }
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

/// Safe to call in edition 2021 code (real std marks it `unsafe` only from edition 2024).
pub fn set_var<K: AsRef<OsStr>, V: AsRef<OsStr>>(key: K, value: V) {
    let (key, value) = (key.as_ref(), value.as_ref());
    let k = CString::new(key.as_encoded_bytes());
    let v = CString::new(value.as_encoded_bytes());
    let r = match (k, v) {
        (Ok(k), Ok(v)) => {
            let _guard = ENV_LOCK.write();
            if unsafe { sys::setenv(k.as_ptr(), v.as_ptr(), 1) } == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        }
        _ => Err(io::Error::const_msg(io::ErrorKind::InvalidInput, "file name contained an unexpected NUL byte")),
    };
    if let Err(e) = r {
        panic!("failed to set environment variable `{:?}` to `{:?}`: {}", key, value, e);
    }
}

pub fn remove_var<K: AsRef<OsStr>>(key: K) {
    let key = key.as_ref();
    let r = match CString::new(key.as_encoded_bytes()) {
        Ok(k) => {
            let _guard = ENV_LOCK.write();
            if unsafe { sys::unsetenv(k.as_ptr()) } == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        }
        Err(_) => Err(io::Error::const_msg(io::ErrorKind::InvalidInput, "file name contained an unexpected NUL byte")),
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

pub fn vars_os() -> VarsOs {
    let _guard = ENV_LOCK.read();
    let mut out = Vec::new();
    unsafe {
        let mut p = sys::os::environ();
        if !p.is_null() {
            while !(*p).is_null() {
                let entry = c_bytes(*p);
                // the first '=' after position 0 splits key and value (a leading '=' is part
                // of the key, as real std does)
                if !entry.is_empty() {
                    let mut i = 1;
                    while i < entry.len() && entry[i] != b'=' {
                        i += 1;
                    }
                    if i < entry.len() {
                        let key = entry[..i].to_vec();
                        let value = entry[i + 1..].to_vec();
                        out.push((OsString::from_bytes_vec(key), OsString::from_bytes_vec(value)));
                    }
                }
                p = p.add(1);
            }
        }
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

// ---- args

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
    let mut buf: Vec<u8> = Vec::with_capacity(512);
    loop {
        let cap = buf.capacity();
        buf.resize(cap, 0);
        let r = unsafe { sys::getcwd(buf.as_mut_ptr() as *mut c_char, cap) };
        if !r.is_null() {
            let n = unsafe { sys::strlen(buf.as_ptr() as *const c_char) };
            buf.truncate(n);
            return Ok(PathBuf::from(OsString::from_bytes_vec(buf)));
        }
        let e = io::Error::last_os_error();
        if e.raw_os_error() != Some(sys::os::ERANGE) {
            return Err(e);
        }
        buf.reserve(cap);
    }
}

pub fn set_current_dir<P: AsRef<Path>>(path: P) -> io::Result<()> {
    let c = crate::fs::cstr(path.as_ref())?;
    if unsafe { sys::chdir(c.as_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn current_exe() -> io::Result<PathBuf> {
    let mut buf = [0u8; 4096];
    match sys::os::executable_path(&mut buf) {
        Some(n) => {
            let p = PathBuf::from(OsString::from_bytes_vec(buf[..n].to_vec()));
            if cfg!(target_os = "macos") {
                // real std canonicalizes _NSGetExecutablePath's result on macOS
                crate::fs::canonicalize(&p)
            } else {
                Ok(p)
            }
        }
        None => Err(io::Error::last_os_error()),
    }
}

pub fn temp_dir() -> PathBuf {
    match var_os("TMPDIR") {
        Some(p) => PathBuf::from(p),
        None => PathBuf::from("/tmp"),
    }
}

pub fn home_dir() -> Option<PathBuf> {
    match var_os("HOME") {
        Some(p) => Some(PathBuf::from(p)),
        None => None,
    }
}

// ---- PATH-style lists

pub struct SplitPaths<'a> {
    rest: Option<&'a [u8]>,
}

pub fn split_paths<T: AsRef<OsStr> + ?Sized>(unparsed: &T) -> SplitPaths<'_> {
    SplitPaths { rest: Some(unparsed.as_ref().as_encoded_bytes()) }
}

impl<'a> Iterator for SplitPaths<'a> {
    type Item = PathBuf;
    fn next(&mut self) -> Option<PathBuf> {
        let rest = self.rest?;
        let mut i = 0;
        while i < rest.len() && rest[i] != b':' {
            i += 1;
        }
        let part = &rest[..i];
        self.rest = if i < rest.len() { Some(&rest[i + 1..]) } else { None };
        Some(PathBuf::from(OsString::from_bytes_vec(part.to_vec())))
    }
}

#[derive(Debug)]
pub struct JoinPathsError;

impl fmt::Display for JoinPathsError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "path segment contains separator `{}`", ':')
    }
}

impl Error for JoinPathsError {}

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
            out.push(b':');
        }
        first = false;
        for c in b {
            if *c == b':' {
                return Err(JoinPathsError);
            }
        }
        out.extend_from_slice(b);
    }
    Ok(OsString::from_bytes_vec(out))
}

pub mod consts {
    pub const ARCH: &str = if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86_64" };
    pub const FAMILY: &str = "unix";
    pub const OS: &str = if cfg!(target_os = "macos") { "macos" } else { "linux" };
    pub const DLL_PREFIX: &str = "lib";
    pub const DLL_SUFFIX: &str = if cfg!(target_os = "macos") { ".dylib" } else { ".so" };
    pub const DLL_EXTENSION: &str = if cfg!(target_os = "macos") { "dylib" } else { "so" };
    pub const EXE_SUFFIX: &str = "";
    pub const EXE_EXTENSION: &str = "";
}

