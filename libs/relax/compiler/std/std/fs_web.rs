//! std::fs for the web targets (rustc's sys/fs/unsupported.rs): there is no file system. Every
//! operation that would touch one is `Err(Unsupported)`; a `File`, `Metadata`, `FileType`,
//! `Permissions`, `ReadDir` and `DirEntry` can never exist, so their methods only match on the
//! uninhabited field. Same public API as fs_unix.rs / fs_windows.rs.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use core::hash::{Hash, Hasher};

use crate::ffi::OsString;
use crate::io::{self, Read, Seek, SeekFrom, Write};
use crate::path::{Path, PathBuf};
use crate::time::SystemTime;

fn unsupported<T>() -> io::Result<T> {
    Err(io::Error::UNSUPPORTED_PLATFORM)
}

// ---- File

pub struct File(!);

#[derive(Clone, Debug)]
pub struct OpenOptions {}

impl OpenOptions {
    pub fn new() -> OpenOptions {
        OpenOptions {}
    }
    pub fn read(&mut self, _read: bool) -> &mut OpenOptions {
        self
    }
    pub fn write(&mut self, _write: bool) -> &mut OpenOptions {
        self
    }
    pub fn append(&mut self, _append: bool) -> &mut OpenOptions {
        self
    }
    pub fn truncate(&mut self, _truncate: bool) -> &mut OpenOptions {
        self
    }
    pub fn create(&mut self, _create: bool) -> &mut OpenOptions {
        self
    }
    pub fn create_new(&mut self, _create_new: bool) -> &mut OpenOptions {
        self
    }
    pub fn open<P: AsRef<Path>>(&self, _path: P) -> io::Result<File> {
        unsupported()
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
    pub fn sync_all(&self) -> io::Result<()> {
        self.0
    }
    pub fn sync_data(&self) -> io::Result<()> {
        self.0
    }
    pub fn set_len(&self, _size: u64) -> io::Result<()> {
        self.0
    }
    pub fn metadata(&self) -> io::Result<Metadata> {
        self.0
    }
    pub fn try_clone(&self) -> io::Result<File> {
        self.0
    }
    pub fn set_permissions(&self, _perm: Permissions) -> io::Result<()> {
        self.0
    }
    pub fn set_times(&self, _times: FileTimes) -> io::Result<()> {
        self.0
    }
    pub fn set_modified(&self, _time: SystemTime) -> io::Result<()> {
        self.0
    }
    pub fn lock(&self) -> io::Result<()> {
        self.0
    }
    pub fn lock_shared(&self) -> io::Result<()> {
        self.0
    }
    pub fn try_lock(&self) -> Result<(), TryLockError> {
        self.0
    }
    pub fn try_lock_shared(&self) -> Result<(), TryLockError> {
        self.0
    }
    pub fn unlock(&self) -> io::Result<()> {
        self.0
    }
}

impl Read for File {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        self.0
    }
}
impl Read for &File {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        self.0
    }
}
impl Write for File {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        self.0
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0
    }
}
impl Write for &File {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        self.0
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0
    }
}
impl Seek for File {
    fn seek(&mut self, _pos: SeekFrom) -> io::Result<u64> {
        self.0
    }
}
impl Seek for &File {
    fn seek(&mut self, _pos: SeekFrom) -> io::Result<u64> {
        self.0
    }
}

impl fmt::Debug for File {
    fn fmt(&self, _f: &mut fmt::Formatter) -> fmt::Result {
        self.0
    }
}

impl crate::io::IsTerminal for File {
    fn is_terminal(&self) -> bool {
        self.0
    }
}

// ---- Metadata

pub struct Metadata(!);

pub struct FileType(!);

pub struct Permissions(!);

impl Metadata {
    pub fn file_type(&self) -> FileType {
        self.0
    }
    pub fn is_dir(&self) -> bool {
        self.0
    }
    pub fn is_file(&self) -> bool {
        self.0
    }
    pub fn is_symlink(&self) -> bool {
        self.0
    }
    pub fn len(&self) -> u64 {
        self.0
    }
    pub fn permissions(&self) -> Permissions {
        self.0
    }
    pub fn modified(&self) -> io::Result<SystemTime> {
        self.0
    }
    pub fn accessed(&self) -> io::Result<SystemTime> {
        self.0
    }
    pub fn created(&self) -> io::Result<SystemTime> {
        self.0
    }
}

impl Clone for Metadata {
    fn clone(&self) -> Metadata {
        self.0
    }
}

impl fmt::Debug for Metadata {
    fn fmt(&self, _f: &mut fmt::Formatter) -> fmt::Result {
        self.0
    }
}

/// Times to set on a file (File::set_times).
#[derive(Copy, Clone, Debug, Default)]
pub struct FileTimes {}

impl FileTimes {
    pub fn new() -> Self {
        FileTimes {}
    }
    pub fn set_accessed(self, _t: SystemTime) -> Self {
        self
    }
    pub fn set_modified(self, _t: SystemTime) -> Self {
        self
    }
}

impl FileType {
    pub fn is_dir(&self) -> bool {
        self.0
    }
    pub fn is_file(&self) -> bool {
        self.0
    }
    pub fn is_symlink(&self) -> bool {
        self.0
    }
}

impl Clone for FileType {
    fn clone(&self) -> FileType {
        self.0
    }
}
impl Copy for FileType {}
impl PartialEq for FileType {
    fn eq(&self, _other: &FileType) -> bool {
        self.0
    }
}
impl Eq for FileType {}
impl Hash for FileType {
    fn hash<H: Hasher>(&self, _h: &mut H) {
        self.0
    }
}
impl fmt::Debug for FileType {
    fn fmt(&self, _f: &mut fmt::Formatter) -> fmt::Result {
        self.0
    }
}

impl Permissions {
    pub fn readonly(&self) -> bool {
        self.0
    }
    pub fn set_readonly(&mut self, _readonly: bool) {
        self.0
    }
}
impl Clone for Permissions {
    fn clone(&self) -> Permissions {
        self.0
    }
}
impl PartialEq for Permissions {
    fn eq(&self, _other: &Permissions) -> bool {
        self.0
    }
}
impl Eq for Permissions {}
impl fmt::Debug for Permissions {
    fn fmt(&self, _f: &mut fmt::Formatter) -> fmt::Result {
        self.0
    }
}

// ---- directories

pub struct ReadDir(!);

pub struct DirEntry(!);

impl fmt::Debug for ReadDir {
    fn fmt(&self, _f: &mut fmt::Formatter) -> fmt::Result {
        self.0
    }
}

impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;
    fn next(&mut self) -> Option<io::Result<DirEntry>> {
        self.0
    }
}

impl DirEntry {
    pub fn path(&self) -> PathBuf {
        self.0
    }
    pub fn file_name(&self) -> OsString {
        self.0
    }
    pub fn metadata(&self) -> io::Result<Metadata> {
        self.0
    }
    pub fn file_type(&self) -> io::Result<FileType> {
        self.0
    }
}

impl fmt::Debug for DirEntry {
    fn fmt(&self, _f: &mut fmt::Formatter) -> fmt::Result {
        self.0
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
    /// rustc's std::fs `DirBuilder::create`: the recursive form returns Ok for "" and for a root
    /// (no parent) before its first `mkdir`; every `mkdir` is unsupported.
    pub fn create<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        let path = path.as_ref();
        if self.recursive && (path == Path::new("") || path.parent() == None) {
            return Ok(());
        }
        unsupported()
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

pub fn metadata<P: AsRef<Path>>(_path: P) -> io::Result<Metadata> {
    unsupported()
}

pub fn symlink_metadata<P: AsRef<Path>>(_path: P) -> io::Result<Metadata> {
    unsupported()
}

pub fn exists<P: AsRef<Path>>(_path: P) -> io::Result<bool> {
    unsupported()
}

pub fn remove_file<P: AsRef<Path>>(_path: P) -> io::Result<()> {
    unsupported()
}

pub fn rename<P: AsRef<Path>, Q: AsRef<Path>>(_from: P, _to: Q) -> io::Result<()> {
    unsupported()
}

pub fn hard_link<P: AsRef<Path>, Q: AsRef<Path>>(_original: P, _link: Q) -> io::Result<()> {
    unsupported()
}

pub fn copy<P: AsRef<Path>, Q: AsRef<Path>>(_from: P, _to: Q) -> io::Result<u64> {
    unsupported()
}

pub fn create_dir<P: AsRef<Path>>(_path: P) -> io::Result<()> {
    unsupported()
}

pub fn create_dir_all<P: AsRef<Path>>(path: P) -> io::Result<()> {
    DirBuilder::new().recursive(true).create(path.as_ref())
}

pub fn remove_dir<P: AsRef<Path>>(_path: P) -> io::Result<()> {
    unsupported()
}

pub fn remove_dir_all<P: AsRef<Path>>(_path: P) -> io::Result<()> {
    unsupported()
}

pub fn read_dir<P: AsRef<Path>>(_path: P) -> io::Result<ReadDir> {
    unsupported()
}

pub fn canonicalize<P: AsRef<Path>>(_path: P) -> io::Result<PathBuf> {
    unsupported()
}

pub fn read_link<P: AsRef<Path>>(_path: P) -> io::Result<PathBuf> {
    unsupported()
}

pub fn set_permissions<P: AsRef<Path>>(_path: P, perm: Permissions) -> io::Result<()> {
    perm.0
}

pub fn soft_link<P: AsRef<Path>, Q: AsRef<Path>>(_original: P, _link: Q) -> io::Result<()> {
    unsupported()
}
