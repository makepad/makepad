//! std::env for the web targets (rustc's sys/env/unsupported.rs, sys/args/unsupported.rs and
//! sys/paths/unsupported.rs): no variables, no arguments, no directories. Same public API as
//! env_unix.rs / env_windows.rs.

use alloc::string::String;
use core::error::Error;
use core::fmt;
use core::marker::PhantomData;

use crate::ffi::{OsStr, OsString};
use crate::io;
use crate::path::{Path, PathBuf};

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

/// No variable is ever set.
pub fn var_os<K: AsRef<OsStr>>(_key: K) -> Option<OsString> {
    None
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

pub fn set_var<K: AsRef<OsStr>, V: AsRef<OsStr>>(key: K, value: V) {
    let (key, value) = (key.as_ref(), value.as_ref());
    let e = io::const_error!(io::ErrorKind::Unsupported, "cannot set env vars on this platform");
    panic!("failed to set environment variable `{:?}` to `{:?}`: {}", key, value, e);
}

pub fn remove_var<K: AsRef<OsStr>>(key: K) {
    let key = key.as_ref();
    let e = io::const_error!(io::ErrorKind::Unsupported, "cannot unset env vars on this platform");
    panic!("failed to remove environment variable `{:?}`: {}", key, e);
}

pub struct VarsOs {
    inner: Env,
}

pub struct Vars {
    inner: VarsOs,
}

/// rustc's `Env(!)`: the iterator cannot be made.
struct Env(!);

impl fmt::Debug for Env {
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0
    }
}

pub fn vars_os() -> VarsOs {
    panic!("not supported on this platform")
}

pub fn vars() -> Vars {
    Vars { inner: vars_os() }
}

impl Iterator for VarsOs {
    type Item = (OsString, OsString);
    fn next(&mut self) -> Option<(OsString, OsString)> {
        self.inner.0
    }
}

impl Iterator for Vars {
    type Item = (String, String);
    fn next(&mut self) -> Option<(String, String)> {
        self.inner.inner.0
    }
}

impl fmt::Debug for VarsOs {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("VarsOs").field("inner", &self.inner).finish()
    }
}

impl fmt::Debug for Vars {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Vars").field("inner", &self.inner.inner).finish()
    }
}

// ---- args: none

pub struct ArgsOs {
    inner: SysArgs,
}

pub struct Args {
    inner: ArgsOs,
}

/// rustc's sys `Args {}`: empty.
struct SysArgs {}

impl fmt::Debug for SysArgs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().finish()
    }
}

pub fn args_os() -> ArgsOs {
    ArgsOs { inner: SysArgs {} }
}

pub fn args() -> Args {
    Args { inner: args_os() }
}

impl Iterator for ArgsOs {
    type Item = OsString;
    fn next(&mut self) -> Option<OsString> {
        None
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(0))
    }
}

impl ExactSizeIterator for ArgsOs {
    fn len(&self) -> usize {
        0
    }
}

impl DoubleEndedIterator for ArgsOs {
    fn next_back(&mut self) -> Option<OsString> {
        None
    }
}

impl Iterator for Args {
    type Item = String;
    fn next(&mut self) -> Option<String> {
        None
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(0))
    }
}

impl ExactSizeIterator for Args {
    fn len(&self) -> usize {
        0
    }
}

impl DoubleEndedIterator for Args {
    fn next_back(&mut self) -> Option<String> {
        None
    }
}

impl fmt::Debug for ArgsOs {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ArgsOs").field("inner", &self.inner).finish()
    }
}

impl fmt::Debug for Args {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Args").field("inner", &self.inner.inner).finish()
    }
}

// ---- directories: none

pub fn current_dir() -> io::Result<PathBuf> {
    Err(io::Error::UNSUPPORTED_PLATFORM)
}

pub fn set_current_dir<P: AsRef<Path>>(_path: P) -> io::Result<()> {
    Err(io::Error::UNSUPPORTED_PLATFORM)
}

pub fn current_exe() -> io::Result<PathBuf> {
    Err(io::Error::UNSUPPORTED_PLATFORM)
}

pub fn temp_dir() -> PathBuf {
    panic!("no filesystem on this platform")
}

pub fn home_dir() -> Option<PathBuf> {
    None
}

// ---- PATH-style lists: unsupported

pub struct SplitPaths<'a>(!, PhantomData<&'a ()>);

pub fn split_paths<T: AsRef<OsStr> + ?Sized>(_unparsed: &T) -> SplitPaths<'_> {
    panic!("unsupported")
}

impl<'a> Iterator for SplitPaths<'a> {
    type Item = PathBuf;
    fn next(&mut self) -> Option<PathBuf> {
        self.0
    }
}

impl<'a> fmt::Debug for SplitPaths<'a> {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0
    }
}

#[derive(Debug)]
pub struct JoinPathsError {
    inner: imp::JoinPathsError,
}

/// rustc's platform `JoinPathsError` (sys/paths/unsupported.rs), the field of the public one.
mod imp {
    #[derive(Debug)]
    pub struct JoinPathsError;
}

impl fmt::Display for JoinPathsError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let _ = &self.inner;
        "not supported on this platform yet".fmt(f)
    }
}

impl Error for JoinPathsError {}

pub fn join_paths<I, T>(_paths: I) -> Result<OsString, JoinPathsError>
where
    I: IntoIterator<Item = T>,
    T: AsRef<OsStr>,
{
    Err(JoinPathsError { inner: imp::JoinPathsError })
}

pub mod consts {
    pub const ARCH: &str = if cfg!(target_arch = "wasm32") { "wasm32" } else { "wasm64" };
    pub const FAMILY: &str = "";
    pub const OS: &str = "";
    pub const DLL_PREFIX: &str = "";
    pub const DLL_SUFFIX: &str = ".wasm";
    pub const DLL_EXTENSION: &str = "wasm";
    pub const EXE_SUFFIX: &str = ".wasm";
    pub const EXE_EXTENSION: &str = "wasm";
}
