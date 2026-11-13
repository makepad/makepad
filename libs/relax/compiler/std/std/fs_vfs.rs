//! Relax's VFS (both OS halves of std::fs, env and process): a program run from a git
//! repository (`makepad run <git url>`) reads its files from the repository's store; the loader
//! answers paths under its virtual folders (compiler vfs.rs). Every other path is the disk, as
//! before. Virtual files are read-only, unless the run has an edits folder (RELAX_EDITS, compiler
//! vfs.rs): then a write into a virtual folder goes to that folder on disk (`edit_target`).

use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec::Vec;

use crate::io;
use crate::path::{Path, PathBuf};

extern "C" {
    fn relax_vfs_stat(p: *const u8, n: usize, size: *mut u64, mtime_ns: *mut u64) -> i32;
    fn relax_vfs_read(p: *const u8, n: usize, out: *mut *const u8, out_len: *mut usize) -> u64;
    fn relax_vfs_list(p: *const u8, n: usize, out: *mut *const u8, out_len: *mut usize) -> u64;
    fn relax_vfs_mounts(out: *mut *const u8, out_len: *mut usize) -> u64;
    fn relax_vfs_release(id: u64);
    fn relax_vfs_edit(p: *const u8, n: usize, mode: u32, out: *mut *const u8, out_len: *mut usize) -> u64;
    fn relax_vfs_note(p: *const u8, n: usize, kind: u32);
}

/// What the program looked at, opened or not: on the disk the path as given (relative to its
/// current folder), in a virtual folder its absolute path. `b'f'` a file it opened for reading,
/// `b'd'` a folder it listed, `b'p'` a path it probed (metadata, exists). A live session keeps it (compiler reload.rs `relax_vfs_note`): a data file a
/// test reads is part of its verdict wherever it lies.
pub(crate) fn note(path: &Path, kind: u8) {
    let b = key(path);
    unsafe { relax_vfs_note(b.as_ptr(), b.len(), kind as u32) };
}

/// What a write into a virtual path does (relax_vfs_edit's mode).
#[derive(Clone, Copy)]
pub(crate) enum Edit {
    /// the edited file or folder, if there is one (remove, rename's source)
    Existing = 0,
    /// a write that keeps the content (append, read-write without truncate)
    Keep = 1,
    /// a write that replaces the content (create, truncate, rename's target)
    Replace = 2,
    /// a folder
    Dir = 3,
}

/// The disk path a write into the virtual `path` goes to (Some(Err) when it cannot be
/// written); None when `path` is not virtual (the disk, as given).
pub(crate) fn edit_target(path: &Path, how: Edit) -> Option<io::Result<PathBuf>> {
    let abs = match absolute(path) {
        Some(abs) => Cow::Owned(abs),
        None => Cow::Borrowed(path),
    };
    kind_abs(&*abs)?;
    let b = key(&*abs);
    let (mut p, mut n) = (core::ptr::null(), 0usize);
    let id = unsafe { relax_vfs_edit(b.as_ptr(), b.len(), how as u32, &mut p, &mut n) };
    Some(match take(id, p, n) {
        Some(v) => Ok(PathBuf::from(String::from_utf8_lossy(&v).into_owned())),
        None => Err(match how {
            Edit::Existing => io::Error::from(io::ErrorKind::NotFound),
            _ => read_only(),
        }),
    })
}

/// A virtual file's stat: its length, and when it last changed in ns since the Unix epoch (an
/// edited file's disk time; a store file's, derived from its blob, moves when the blob does).
#[derive(Clone, Copy)]
pub(crate) struct Stat {
    pub len: u64,
    pub mtime_ns: u64,
}

pub(crate) enum Kind {
    File(Stat),
    Dir,
    Missing,
}

/// The current folder the program changed to (set_current_dir): Some(Some(a virtual folder)), or
/// Some(None) for a real one (the OS's). None: the one it started in.
static CHOSEN: crate::sync::Mutex<Option<Option<PathBuf>>> = crate::sync::Mutex::new(None);

/// The program's virtual current folder: the one it changed to, else the workspace of a git run
/// (the host's RELAX_WORKSPACE, when it is a virtual folder).
pub(crate) fn cwd() -> Option<PathBuf> {
    if let Some(c) = chosen() {
        return c;
    }
    let w = crate::env::host_var_os("RELAX_WORKSPACE")?;
    let p = PathBuf::from(w);
    matches!(kind_abs(&p), Some(Kind::Dir)).then_some(p)
}

/// The folder set_current_dir changed to, if the program changed it: Some(None) for a real one.
pub(crate) fn chosen() -> Option<Option<PathBuf>> {
    CHOSEN.lock().unwrap().clone()
}

/// set_current_dir: a virtual folder becomes the program's current folder (Ok: the result). A
/// real one is Err(the folder): the caller changes the OS's folder to it, then calls `chose_real`.
pub(crate) fn set_cwd(path: &Path) -> Result<io::Result<()>, PathBuf> {
    let Some((abs, k)) = kind(path) else {
        // (a relative path is the virtual folder's while the program is in one)
        return Err(absolute(path).unwrap_or_else(|| path.to_path_buf()));
    };
    Ok(match k {
        Kind::Dir => {
            let abs = abs.into_owned();
            *CHOSEN.lock().unwrap() = Some(Some(abs));
            Ok(())
        }
        Kind::File(_) => Err(io::Error::from(io::ErrorKind::NotADirectory)),
        Kind::Missing => Err(io::Error::from(io::ErrorKind::NotFound)),
    })
}

/// The program changed to a real folder: it is in no virtual one any more.
pub(crate) fn chose_real() {
    *CHOSEN.lock().unwrap() = Some(None);
}

/// `path` made absolute against the virtual current folder (a relative path of a git run).
pub(crate) fn absolute(path: &Path) -> Option<PathBuf> {
    if path.is_absolute() {
        return None;
    }
    Some(cwd()?.join(path))
}

/// The path as the loader reads it: on Windows without the verbatim `\\?\` prefix (the
/// loader splits on both separators itself).
fn key(path: &Path) -> &[u8] {
    let b = path.as_os_str().as_encoded_bytes();
    #[cfg(windows)]
    if let Some(r) = b.strip_prefix(br"\\?\") {
        return r;
    }
    b
}

fn kind_abs(path: &Path) -> Option<Kind> {
    let b = key(path);
    let (mut len, mut mtime_ns) = (0u64, 0u64);
    match unsafe { relax_vfs_stat(b.as_ptr(), b.len(), &mut len, &mut mtime_ns) } {
        1 => Some(Kind::File(Stat { len, mtime_ns })),
        2 => Some(Kind::Dir),
        -1 => Some(Kind::Missing),
        _ => None,
    }
}

/// None: the disk; Some((absolute path, kind)): a virtual path.
pub(crate) fn kind(path: &Path) -> Option<(Cow<'_, Path>, Kind)> {
    let abs = match absolute(path) {
        Some(abs) => Cow::Owned(abs),
        None => Cow::Borrowed(path),
    };
    let k = kind_abs(&*abs)?;
    Some((abs, k))
}

fn take(id: u64, ptr: *const u8, len: usize) -> Option<Vec<u8>> {
    if id == 0 {
        return None;
    }
    let v = unsafe { core::slice::from_raw_parts(ptr, len) }.to_vec();
    unsafe { relax_vfs_release(id) };
    Some(v)
}

pub(crate) fn read(path: &Path) -> io::Result<Vec<u8>> {
    let b = key(path);
    let (mut p, mut n) = (core::ptr::null(), 0usize);
    let id = unsafe { relax_vfs_read(b.as_ptr(), b.len(), &mut p, &mut n) };
    take(id, p, n).ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
}

/// (name, is folder) per entry.
pub(crate) fn list(path: &Path) -> io::Result<Vec<(Vec<u8>, bool)>> {
    let b = key(path);
    let (mut p, mut n) = (core::ptr::null(), 0usize);
    let id = unsafe { relax_vfs_list(b.as_ptr(), b.len(), &mut p, &mut n) };
    let v = take(id, p, n).ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
    Ok(v.split(|c| *c == b'\n').filter(|l| l.len() > 2).map(|l| (l[2..].to_vec(), l[0] == b'd')).collect())
}

/// The mount table, for a child started in a virtual folder (process.rs).
pub(crate) fn mounts() -> String {
    let (mut p, mut n) = (core::ptr::null(), 0usize);
    let id = unsafe { relax_vfs_mounts(&mut p, &mut n) };
    take(id, p, n).map(|v| String::from_utf8(v).unwrap_or_default()).unwrap_or_default()
}

pub(crate) fn read_only() -> io::Error {
    io::const_error!(io::ErrorKind::PermissionDenied, "a file of a git repository run (Relax's VFS) is read-only")
}
