use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt},
        },
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub use crate::wire::*;
#[derive(Clone, Debug)]
pub struct SessionLocation {
    pub state_dir: PathBuf,
    pub session_id: String,
    pub socket_path: PathBuf,
    pub claim_path: PathBuf,
    pub metadata_path: PathBuf,
}
impl SessionLocation {
    pub fn open(state_dir: &Path, session_id: &str, create: bool) -> Result<Self, String> {
        if !state_dir.is_absolute() || !valid_session(session_id) {
            return Err(
                "Screen requires an absolute state directory and a 1..48 character session ID"
                    .into(),
            );
        }
        private_directory(state_dir, create)?;
        let state_dir = state_dir.canonicalize().map_err(error)?;
        if state_dir.to_str().is_none() {
            return Err("Screen state directory must be UTF-8".into());
        }
        let uid = crate::unix::current_uid();
        let digest = crate::digest::hex(&crate::digest::sha256(state_dir.as_os_str().as_bytes()));
        let namespace = PathBuf::from(format!("/tmp/mp-screen-{uid}-{}", &digest[..16]));
        let namespace_exists = match fs::symlink_metadata(&namespace) {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.to_string()),
        };
        if create || namespace_exists {
            private_directory(&namespace, create)?;
            let marker = namespace.join("scope");
            if !marker.exists() && create {
                if let Err(error) = create_private(&marker, state_dir.as_os_str().as_bytes()) {
                    if !marker.exists() {
                        return Err(error);
                    }
                }
            }
            if read_private(&marker, 8192)? != state_dir.as_os_str().as_bytes() {
                return Err("Screen socket namespace belongs to another state directory".into());
            }
        }
        let socket_path = namespace.join(format!("{session_id}.sock"));
        if socket_path.as_os_str().as_bytes().len() >= 104 {
            return Err("Screen socket path exceeds Unix socket limit".into());
        }
        Ok(Self {
            claim_path: state_dir.join(format!("{session_id}.claim")),
            metadata_path: state_dir.join(format!("{session_id}.json")),
            state_dir,
            session_id: session_id.into(),
            socket_path,
        })
    }
    pub fn log_path(&self) -> PathBuf {
        self.state_dir.join(format!("{}.log", self.session_id))
    }
    pub fn lock_path(&self) -> PathBuf {
        self.state_dir.join(format!("{}.lock", self.session_id))
    }
    pub fn validate_socket(&self) -> Result<bool, String> {
        match fs::symlink_metadata(&self.socket_path) {
            Ok(metadata)
                if metadata.file_type().is_socket()
                    && metadata.uid() == crate::unix::current_uid()
                    && metadata.mode() & 0o077 == 0 =>
            {
                Ok(true)
            }
            Ok(_) => Err("Screen endpoint is not an owned private Unix socket".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(error(e)),
        }
    }
}
pub fn private_directory(path: &Path, create: bool) -> Result<(), String> {
    if create {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(path).map_err(error)?;
    }
    let metadata = fs::symlink_metadata(path).map_err(error)?;
    if !metadata.is_dir()
        || metadata.uid() != crate::unix::current_uid()
        || metadata.mode() & 0o077 != 0
    {
        return Err("Screen directories must be owned by this user with mode 0700".into());
    }
    Ok(())
}
pub fn read_private(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let before = fs::symlink_metadata(path).map_err(error)?;
    if !before.is_file()
        || before.uid() != crate::unix::current_uid()
        || before.mode() & 0o077 != 0
        || before.len() > limit as u64
    {
        return Err("Screen metadata must be an owned bounded mode-0600 regular file".into());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "macos")]
    options.custom_flags(0x100 | 0x4);
    #[cfg(target_os = "linux")]
    options.custom_flags(0x20000 | 0x800);
    let file = options.open(path).map_err(error)?;
    let after = file.metadata().map_err(error)?;
    if after.dev() != before.dev() || after.ino() != before.ino() {
        return Err("Screen metadata changed while opening".into());
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    if bytes.len() > limit {
        return Err("Screen metadata exceeds its size bound".into());
    }
    Ok(bytes)
}
pub fn create_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(error)?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(error)
}
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > 16 * 1024 {
        return Err("Screen metadata exceeds 16 KiB".into());
    }
    if path.exists() {
        read_private(path, 16 * 1024)?;
    }
    let parent = path.parent().ok_or("Screen metadata has no parent")?;
    let temporary = parent.join(format!(".screen-{}.tmp", random_token()?));
    let result = (|| {
        create_private(&temporary, bytes)?;
        fs::rename(&temporary, path).map_err(error)?;
        File::open(parent)
            .and_then(|file| file.sync_all())
            .map_err(error)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}
pub fn random_token() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(error)?;
    Ok(crate::digest::hex(&bytes))
}
pub fn error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// Lock is inherited only by the detached daemon, then made CLOEXEC before PTY spawn.
pub struct SessionLock {
    file: File,
}
impl SessionLock {
    pub fn acquire(location: &SessionLocation) -> Result<Option<Self>, String> {
        let path = location.lock_path();
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).mode(0o600);
        #[cfg(target_os = "macos")]
        options.custom_flags(0x100 | 0x4);
        #[cfg(target_os = "linux")]
        options.custom_flags(0x20000 | 0x800);
        let file = options.open(&path).map_err(error)?;
        let metadata = file.metadata().map_err(error)?;
        if !metadata.is_file()
            || metadata.uid() != crate::unix::current_uid()
            || metadata.mode() & 0o077 != 0
        {
            return Err("Invalid screen session lock".into());
        }
        unsafe extern "C" {
            fn flock(fd: i32, operation: i32) -> i32;
        }
        if unsafe { flock(file.as_raw_fd(), 2 | 4) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                return Ok(None);
            }
            return Err(error.to_string());
        }
        Ok(Some(Self { file }))
    }
    pub fn inherit(&self, inherit: bool) -> Result<i32, String> {
        unsafe extern "C" {
            fn fcntl(fd: i32, command: i32, ...) -> i32;
        }
        let fd = self.file.as_raw_fd();
        let flags = unsafe { fcntl(fd, 1) };
        if flags < 0 || unsafe { fcntl(fd, 2, if inherit { flags & !1 } else { flags | 1 }) } < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(fd)
    }
    /// `fd` is an internal CLI argument inherited from our own start command.
    pub unsafe fn from_inherited(location: &SessionLocation, fd: i32) -> Result<Self, String> {
        if fd < 3 {
            return Err("Invalid inherited session lock".into());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let actual = file.metadata().map_err(error)?;
        let expected = fs::symlink_metadata(location.lock_path()).map_err(error)?;
        if !actual.is_file()
            || actual.dev() != expected.dev()
            || actual.ino() != expected.ino()
            || actual.uid() != crate::unix::current_uid()
        {
            return Err("Inherited lock does not match the owned session".into());
        }
        unsafe extern "C" {
            fn flock(fd: i32, operation: i32) -> i32;
        }
        if unsafe { flock(file.as_raw_fd(), 2 | 4) } != 0 {
            return Err("Inherited session lock is unavailable".into());
        }
        let lock = Self { file };
        lock.inherit(false)?;
        Ok(lock)
    }
}
pub fn exchange(
    location: &SessionLocation,
    kind: u8,
    payload: &[u8],
    timeout: Duration,
) -> Result<Frame, String> {
    if !location.validate_socket()? {
        return Err("Screen session has no live endpoint".into());
    }
    let deadline = Instant::now() + timeout;
    let mut stream = crate::unix::connect(&location.socket_path, timeout).map_err(error)?;
    if crate::unix::peer_uid(&stream).map_err(error)? != crate::unix::current_uid() {
        return Err("Screen peer UID differs".into());
    }
    stream.set_nonblocking(true).map_err(error)?;
    let bytes = encode_frame(kind, payload)?;
    let mut sent = 0;
    let mut input = Vec::new();
    let mut buffer = [0u8; 16384];
    loop {
        if Instant::now() >= deadline {
            return Err("Screen control timed out".into());
        }
        if sent < bytes.len() {
            match stream.write(&bytes[sent..]) {
                Ok(0) => return Err("Screen control closed".into()),
                Ok(n) => sent += n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(error(e)),
            }
        }
        match stream.read(&mut buffer) {
            Ok(0) => return Err("Screen control closed before replying".into()),
            Ok(n) => input.extend_from_slice(&buffer[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(error(e)),
        }
        if input.len() > MAX_FRAME + 5 {
            return Err("Screen control response exceeds its bound".into());
        }
        if let Some(frame) = decode_frame(&mut input)? {
            return Ok(frame);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
