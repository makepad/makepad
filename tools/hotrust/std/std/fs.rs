//! std::fs for unix (libSystem / glibc calls through crate::sys).

use alloc::ffi::CString;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ffi::{c_char, c_int, c_void};
use core::fmt;

use crate::ffi::{OsStr, OsString};
use crate::io::{self, Read, Seek, SeekFrom, Write};
use crate::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use crate::path::{Path, PathBuf};
use crate::sys;
use crate::sys::os::stat_t;
use crate::time::SystemTime;

/// Path -> nul-terminated C string; an interior NUL is InvalidInput (real std's message).
pub(crate) fn cstr(path: &Path) -> io::Result<CString> {
    match CString::new(path.as_os_str().as_encoded_bytes()) {
        Ok(c) => Ok(c),
        Err(_) => Err(io::Error::const_msg(io::ErrorKind::InvalidInput, "file name contained an unexpected NUL byte")),
    }
}

fn cvt(r: c_int) -> io::Result<c_int> {
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(r)
    }
}

fn cvt_r<F: FnMut() -> c_int>(mut f: F) -> io::Result<c_int> {
    loop {
        let r = f();
        if r >= 0 {
            return Ok(r);
        }
        let e = io::Error::last_os_error();
        if !e.is_interrupted() {
            return Err(e);
        }
    }
}

fn needs_write() -> io::Error {
    io::Error::from_string(io::ErrorKind::InvalidInput, String::from("creating or truncating a file requires write or append access"))
}

// ---- File

pub struct File {
    fd: OwnedFd,
}

#[derive(Clone, Debug)]
pub struct OpenOptions {
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
    create: bool,
    create_new: bool,
    custom_flags: i32,
    mode: u32,
}

impl OpenOptions {
    pub fn new() -> OpenOptions {
        OpenOptions { read: false, write: false, append: false, truncate: false, create: false, create_new: false, custom_flags: 0, mode: 0o666 }
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
    pub(crate) fn set_mode(&mut self, mode: u32) {
        self.mode = mode;
    }
    pub(crate) fn set_custom_flags(&mut self, flags: i32) {
        self.custom_flags = flags;
    }
    pub fn open<P: AsRef<Path>>(&self, path: P) -> io::Result<File> {
        let path = cstr(path.as_ref())?;
        self.open_c(&path)
    }

    fn access_mode(&self) -> io::Result<c_int> {
        match (self.read, self.write, self.append) {
            (true, false, false) => Ok(sys::O_RDONLY),
            (false, true, false) => Ok(sys::O_WRONLY),
            (true, true, false) => Ok(sys::O_RDWR),
            (false, _, true) => Ok(sys::O_WRONLY | sys::os::O_APPEND),
            (true, _, true) => Ok(sys::O_RDWR | sys::os::O_APPEND),
            (false, false, false) => {
                if self.create || self.create_new || self.truncate {
                    Err(needs_write())
                } else {
                    Err(io::Error::from_string(io::ErrorKind::InvalidInput, String::from("must specify at least one of read, write, or append access")))
                }
            }
        }
    }

    fn creation_mode(&self) -> io::Result<c_int> {
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
            (false, false, false) => 0,
            (true, false, false) => sys::os::O_CREAT,
            (false, true, false) => sys::os::O_TRUNC,
            (true, true, false) => sys::os::O_CREAT | sys::os::O_TRUNC,
            (_, _, true) => sys::os::O_CREAT | sys::os::O_EXCL,
        })
    }

    fn open_c(&self, path: &CString) -> io::Result<File> {
        let flags = sys::os::O_CLOEXEC | self.access_mode()? | self.creation_mode()? | (self.custom_flags & !3);
        let mode = self.mode;
        let fd = cvt_r(|| unsafe { sys::open(path.as_ptr(), flags, mode as core::ffi::c_uint) })?;
        Ok(File { fd: unsafe { OwnedFd::from_raw_fd(fd) } })
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
    fn raw(&self) -> c_int {
        self.fd.as_raw_fd()
    }
    pub fn sync_all(&self) -> io::Result<()> {
        cvt_r(|| unsafe { sys::fsync(self.raw()) })?;
        Ok(())
    }
    pub fn sync_data(&self) -> io::Result<()> {
        self.sync_all()
    }
    pub fn set_len(&self, size: u64) -> io::Result<()> {
        if size > i64::MAX as u64 {
            return Err(io::Error::from_raw_os_error(sys::os::EINVAL));
        }
        cvt_r(|| unsafe { sys::ftruncate(self.raw(), size as i64) })?;
        Ok(())
    }
    pub fn metadata(&self) -> io::Result<Metadata> {
        let mut st = stat_t::zeroed();
        cvt(unsafe { sys::os::fstat(self.raw(), &mut st as *mut stat_t) })?;
        Ok(Metadata { st })
    }
    pub fn try_clone(&self) -> io::Result<File> {
        Ok(File { fd: self.fd.try_clone()? })
    }
    pub fn set_permissions(&self, perm: Permissions) -> io::Result<()> {
        cvt_r(|| unsafe { sys::fchmod(self.raw(), perm.mode as sys::mode_t) })?;
        Ok(())
    }
    pub fn lock(&self) -> io::Result<()> {
        cvt(unsafe { sys::flock(self.raw(), sys::os::LOCK_EX) })?;
        Ok(())
    }
    pub fn lock_shared(&self) -> io::Result<()> {
        cvt(unsafe { sys::flock(self.raw(), sys::os::LOCK_SH) })?;
        Ok(())
    }
    pub fn try_lock(&self) -> Result<(), TryLockError> {
        self.try_flock(sys::os::LOCK_EX | sys::os::LOCK_NB)
    }
    pub fn try_lock_shared(&self) -> Result<(), TryLockError> {
        self.try_flock(sys::os::LOCK_SH | sys::os::LOCK_NB)
    }
    fn try_flock(&self, op: c_int) -> Result<(), TryLockError> {
        match cvt(unsafe { sys::flock(self.raw(), op) }) {
            Ok(_) => Ok(()),
            Err(e) => {
                if e.raw_os_error() == Some(sys::os::EWOULDBLOCK) {
                    Err(TryLockError::WouldBlock)
                } else {
                    Err(TryLockError::Error(e))
                }
            }
        }
    }
    pub fn unlock(&self) -> io::Result<()> {
        cvt(unsafe { sys::flock(self.raw(), sys::os::LOCK_UN) })?;
        Ok(())
    }
    fn read_raw(&self, buf: &mut [u8]) -> io::Result<usize> {
        let len = if buf.len() > isize::MAX as usize { isize::MAX as usize } else { buf.len() };
        let r = unsafe { sys::read(self.raw(), buf.as_mut_ptr() as *mut c_void, len) };
        if r < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(r as usize)
        }
    }
    fn write_raw(&self, buf: &[u8]) -> io::Result<usize> {
        let len = if buf.len() > isize::MAX as usize { isize::MAX as usize } else { buf.len() };
        let r = unsafe { sys::write(self.raw(), buf.as_ptr() as *const c_void, len) };
        if r < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(r as usize)
        }
    }
    fn seek_raw(&self, pos: SeekFrom) -> io::Result<u64> {
        let (whence, off) = match pos {
            SeekFrom::Start(o) => (sys::SEEK_SET, o as i64),
            SeekFrom::End(o) => (sys::SEEK_END, o),
            SeekFrom::Current(o) => (sys::SEEK_CUR, o),
        };
        let r = unsafe { sys::lseek(self.raw(), off, whence) };
        if r < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(r as u64)
        }
    }
    /// Buffer sized from the file length for read_to_end (as real std does).
    fn size_hint(&self) -> Option<usize> {
        let mut st = stat_t::zeroed();
        if unsafe { sys::os::fstat(self.raw(), &mut st as *mut stat_t) } != 0 {
            return None;
        }
        let pos = unsafe { sys::lseek(self.raw(), 0, sys::SEEK_CUR) };
        if pos < 0 || st.st_size < pos {
            return None;
        }
        Some((st.st_size - pos) as usize)
    }
}

impl Read for File {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_raw(buf)
    }
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        (&*self).read_to_end(buf)
    }
}

impl Read for &File {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_raw(buf)
    }
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        if let Some(n) = self.size_hint() {
            buf.reserve(n);
        }
        let start = buf.len();
        let mut chunk = [0u8; 8 * 1024];
        loop {
            match self.read_raw(&mut chunk) {
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
        self.write_raw(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Write for &File {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_raw(buf)
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
        f.debug_struct("File").field("fd", &self.raw()).finish_non_exhaustive()
    }
}

impl AsRawFd for File {
    fn as_raw_fd(&self) -> RawFd {
        self.raw()
    }
}
impl AsFd for File {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}
impl FromRawFd for File {
    unsafe fn from_raw_fd(fd: RawFd) -> File {
        File { fd: OwnedFd::from_raw_fd(fd) }
    }
}
impl IntoRawFd for File {
    fn into_raw_fd(self) -> RawFd {
        self.fd.into_raw_fd()
    }
}
impl From<File> for OwnedFd {
    fn from(f: File) -> OwnedFd {
        f.fd
    }
}
impl From<OwnedFd> for File {
    fn from(fd: OwnedFd) -> File {
        File { fd }
    }
}
impl crate::io::IsTerminal for File {
    fn is_terminal(&self) -> bool {
        unsafe { sys::isatty(self.raw()) == 1 }
    }
}

// ---- Metadata / FileType / Permissions

#[derive(Clone)]
pub struct Metadata {
    st: stat_t,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct FileType {
    mode: u32,
}

#[derive(Clone, PartialEq, Eq)]
pub struct Permissions {
    mode: u32,
}

fn time_or_unsupported(secs: i64, nanos: i64) -> io::Result<SystemTime> {
    Ok(SystemTime::from_parts(secs, nanos))
}

impl Metadata {
    pub fn file_type(&self) -> FileType {
        FileType { mode: self.st.mode() }
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
        self.st.st_size as u64
    }
    pub fn permissions(&self) -> Permissions {
        Permissions { mode: self.st.mode() }
    }
    pub fn modified(&self) -> io::Result<SystemTime> {
        time_or_unsupported(self.st.st_mtime, self.st.st_mtime_nsec)
    }
    pub fn accessed(&self) -> io::Result<SystemTime> {
        time_or_unsupported(self.st.st_atime, self.st.st_atime_nsec)
    }
    pub fn created(&self) -> io::Result<SystemTime> {
        match self.st.created() {
            Some((s, n)) => time_or_unsupported(s, n),
            None => Err(io::Error::const_msg(io::ErrorKind::Unsupported, "creation time is not available on this platform currently")),
        }
    }
    pub(crate) fn stat(&self) -> &stat_t {
        &self.st
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

impl FileType {
    fn is(&self, kind: u32) -> bool {
        self.mode & sys::S_IFMT == kind
    }
    pub fn is_dir(&self) -> bool {
        self.is(sys::S_IFDIR)
    }
    pub fn is_file(&self) -> bool {
        self.is(sys::S_IFREG)
    }
    pub fn is_symlink(&self) -> bool {
        self.is(sys::S_IFLNK)
    }
    pub(crate) fn kind_fifo(&self) -> bool {
        self.is(sys::S_IFIFO)
    }
    pub(crate) fn kind_socket(&self) -> bool {
        self.is(sys::S_IFSOCK)
    }
    pub(crate) fn kind_char_device(&self) -> bool {
        self.is(sys::S_IFCHR)
    }
    pub(crate) fn kind_block_device(&self) -> bool {
        self.is(sys::S_IFBLK)
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
        self.mode & 0o222 == 0
    }
    pub fn set_readonly(&mut self, readonly: bool) {
        if readonly {
            self.mode &= !0o222;
        } else {
            self.mode |= 0o222;
        }
    }
    pub(crate) fn mode_bits(&self) -> u32 {
        self.mode
    }
    pub(crate) fn set_mode_bits(&mut self, mode: u32) {
        self.mode = mode;
    }
    pub(crate) fn from_mode_bits(mode: u32) -> Permissions {
        Permissions { mode }
    }
}

impl fmt::Debug for Permissions {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Permissions").field("readonly", &self.readonly()).finish_non_exhaustive()
    }
}

// ---- directories

pub struct ReadDir {
    dir: *mut c_void, // DIR*
    root: Arc<PathBuf>,
    end: bool,
}

unsafe impl Send for ReadDir {}
unsafe impl Sync for ReadDir {}

pub struct DirEntry {
    root: Arc<PathBuf>,
    name: Vec<u8>,
    d_type: u8,
}

const DT_UNKNOWN: u8 = 0;
const DT_FIFO: u8 = 1;
const DT_CHR: u8 = 2;
const DT_DIR: u8 = 4;
const DT_BLK: u8 = 6;
const DT_REG: u8 = 8;
const DT_LNK: u8 = 10;
const DT_SOCK: u8 = 12;

impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;
    fn next(&mut self) -> Option<io::Result<DirEntry>> {
        if self.end {
            return None;
        }
        loop {
            sys::set_errno(0);
            let ent = unsafe { sys::os::readdir(self.dir) };
            if ent.is_null() {
                self.end = true;
                let e = sys::errno();
                if e != 0 {
                    return Some(Err(io::Error::from_raw_os_error(e)));
                }
                return None;
            }
            let (name, d_type) = unsafe {
                let p = (*ent).d_name.as_ptr();
                let n = sys::strlen(p as *const c_char);
                (core::slice::from_raw_parts(p as *const u8, n).to_vec(), (*ent).d_type)
            };
            if name == b"." || name == b".." {
                continue;
            }
            return Some(Ok(DirEntry { root: self.root.clone(), name, d_type }));
        }
    }
}

impl Drop for ReadDir {
    fn drop(&mut self) {
        unsafe {
            sys::closedir(self.dir);
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
        self.root.join(OsStr::from_raw_bytes(&self.name))
    }
    pub fn file_name(&self) -> OsString {
        OsStr::from_raw_bytes(&self.name).to_os_string()
    }
    pub fn metadata(&self) -> io::Result<Metadata> {
        symlink_metadata(self.path())
    }
    pub fn file_type(&self) -> io::Result<FileType> {
        let mode = match self.d_type {
            DT_FIFO => sys::S_IFIFO,
            DT_CHR => sys::S_IFCHR,
            DT_DIR => sys::S_IFDIR,
            DT_BLK => sys::S_IFBLK,
            DT_REG => sys::S_IFREG,
            DT_LNK => sys::S_IFLNK,
            DT_SOCK => sys::S_IFSOCK,
            _ => return Ok(self.metadata()?.file_type()),
        };
        Ok(FileType { mode })
    }
    pub(crate) fn name_bytes(&self) -> &[u8] {
        &self.name
    }
}

impl fmt::Debug for DirEntry {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_tuple("DirEntry").field(&self.path()).finish()
    }
}

#[derive(Debug)]
pub struct DirBuilder {
    mode: u32,
    recursive: bool,
}

impl DirBuilder {
    pub fn new() -> DirBuilder {
        DirBuilder { mode: 0o777, recursive: false }
    }
    pub fn recursive(&mut self, recursive: bool) -> &mut DirBuilder {
        self.recursive = recursive;
        self
    }
    pub(crate) fn set_mode(&mut self, mode: u32) {
        self.mode = mode;
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
        let c = cstr(path)?;
        cvt(unsafe { sys::mkdir(c.as_ptr(), self.mode as sys::mode_t) })?;
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
            None => return Err(io::Error::const_msg(io::ErrorKind::Uncategorized, "failed to create whole tree")),
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

pub fn metadata<P: AsRef<Path>>(path: P) -> io::Result<Metadata> {
    let c = cstr(path.as_ref())?;
    let mut st = stat_t::zeroed();
    cvt(unsafe { sys::os::stat(c.as_ptr(), &mut st as *mut stat_t) })?;
    Ok(Metadata { st })
}

pub fn symlink_metadata<P: AsRef<Path>>(path: P) -> io::Result<Metadata> {
    let c = cstr(path.as_ref())?;
    let mut st = stat_t::zeroed();
    cvt(unsafe { sys::os::lstat(c.as_ptr(), &mut st as *mut stat_t) })?;
    Ok(Metadata { st })
}

pub fn exists<P: AsRef<Path>>(path: P) -> io::Result<bool> {
    match metadata(path) {
        Ok(_) => Ok(true),
        Err(e) => {
            if e.kind() == io::ErrorKind::NotFound {
                Ok(false)
            } else {
                Err(e)
            }
        }
    }
}

pub fn remove_file<P: AsRef<Path>>(path: P) -> io::Result<()> {
    let c = cstr(path.as_ref())?;
    cvt(unsafe { sys::unlink(c.as_ptr()) })?;
    Ok(())
}

pub fn rename<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> io::Result<()> {
    let a = cstr(from.as_ref())?;
    let b = cstr(to.as_ref())?;
    cvt(unsafe { sys::rename(a.as_ptr(), b.as_ptr()) })?;
    Ok(())
}

pub fn hard_link<P: AsRef<Path>, Q: AsRef<Path>>(original: P, link: Q) -> io::Result<()> {
    let a = cstr(original.as_ref())?;
    let b = cstr(link.as_ref())?;
    cvt(unsafe { sys::link(a.as_ptr(), b.as_ptr()) })?;
    Ok(())
}

pub fn copy<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> io::Result<u64> {
    let from = from.as_ref();
    let mut reader = File::open(from)?;
    let meta = reader.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::const_msg(io::ErrorKind::InvalidInput, "the source path is neither a regular file nor a symlink to a regular file"));
    }
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    opts.set_mode(meta.permissions().mode_bits());
    let mut writer = opts.open(to.as_ref())?;
    writer.set_permissions(meta.permissions())?;
    io::copy(&mut reader, &mut writer)
}

pub fn create_dir<P: AsRef<Path>>(path: P) -> io::Result<()> {
    DirBuilder::new().create(path.as_ref())
}

pub fn create_dir_all<P: AsRef<Path>>(path: P) -> io::Result<()> {
    DirBuilder::new().recursive(true).create(path.as_ref())
}

pub fn remove_dir<P: AsRef<Path>>(path: P) -> io::Result<()> {
    let c = cstr(path.as_ref())?;
    cvt(unsafe { sys::rmdir(c.as_ptr()) })?;
    Ok(())
}

pub fn remove_dir_all<P: AsRef<Path>>(path: P) -> io::Result<()> {
    let path = path.as_ref();
    let meta = symlink_metadata(path)?;
    if meta.is_symlink() {
        remove_file(path)
    } else {
        remove_dir_all_recursive(path)
    }
}

fn remove_dir_all_recursive(path: &Path) -> io::Result<()> {
    for child in read_dir(path)? {
        let child = child?;
        let child_path = child.path();
        if child.file_type()?.is_dir() {
            remove_dir_all_recursive(&child_path)?;
        } else {
            match remove_file(&child_path) {
                Ok(()) => {}
                Err(e) => {
                    if e.kind() != io::ErrorKind::NotFound {
                        return Err(e);
                    }
                }
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
    let path = path.as_ref();
    let c = cstr(path)?;
    let dir = unsafe { sys::opendir(c.as_ptr()) };
    if dir.is_null() {
        return Err(io::Error::last_os_error());
    }
    Ok(ReadDir { dir, root: Arc::new(path.to_path_buf()), end: false })
}

pub fn canonicalize<P: AsRef<Path>>(path: P) -> io::Result<PathBuf> {
    let c = cstr(path.as_ref())?;
    let r = unsafe { sys::realpath(c.as_ptr(), core::ptr::null_mut()) };
    if r.is_null() {
        return Err(io::Error::last_os_error());
    }
    let bytes = unsafe { core::slice::from_raw_parts(r as *const u8, sys::strlen(r)) }.to_vec();
    unsafe { sys::free(r as *mut c_void) };
    Ok(PathBuf::from(OsString::from_bytes_vec(bytes)))
}

pub fn read_link<P: AsRef<Path>>(path: P) -> io::Result<PathBuf> {
    let c = cstr(path.as_ref())?;
    let mut buf: Vec<u8> = Vec::with_capacity(256);
    loop {
        let cap = buf.capacity();
        buf.resize(cap, 0);
        let n = unsafe { sys::readlink(c.as_ptr(), buf.as_mut_ptr() as *mut c_char, cap) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        let n = n as usize;
        if n < cap {
            buf.truncate(n);
            return Ok(PathBuf::from(OsString::from_bytes_vec(buf)));
        }
        buf.reserve(cap);
    }
}

pub fn set_permissions<P: AsRef<Path>>(path: P, perm: Permissions) -> io::Result<()> {
    let c = cstr(path.as_ref())?;
    cvt_r(|| unsafe { sys::chmod(c.as_ptr(), perm.mode as sys::mode_t) })?;
    Ok(())
}

pub(crate) fn symlink_raw(original: &Path, link: &Path) -> io::Result<()> {
    let a = cstr(original)?;
    let b = cstr(link)?;
    cvt(unsafe { sys::symlink(a.as_ptr(), b.as_ptr()) })?;
    Ok(())
}
