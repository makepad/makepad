pub use crate::windows_security::{
    create_private, private_directory, random_token, read_private, write_private, SessionLock,
};
pub use crate::wire::*;
use makepad_strict_json::Value;
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
pub fn error(error: impl std::fmt::Display) -> String {
    error.to_string()
}
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
            return Err("Screen requires an absolute state path and valid session ID".into());
        }
        private_directory(state_dir, create)?;
        let state_dir = state_dir.canonicalize().map_err(error)?;
        if state_dir.to_str().is_none() {
            return Err("Screen state directory must be Unicode".into());
        }
        Ok(Self {
            session_id: session_id.into(),
            socket_path: state_dir.join(format!("{session_id}.endpoint")),
            claim_path: state_dir.join(format!("{session_id}.claim")),
            metadata_path: state_dir.join(format!("{session_id}.json")),
            state_dir,
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
            Ok(_) => {
                self.endpoint()?;
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.to_string()),
        }
    }
    pub fn endpoint(&self) -> Result<Value, String> {
        let value = parse_object(&read_private(&self.socket_path, 16384)?)?;
        if value.get("version").and_then(Value::as_u64) != Some(VERSION)
            || value.get("session_id").and_then(Value::as_str) != Some(&self.session_id)
            || value.get("state_dir").and_then(Value::as_str) != self.state_dir.to_str()
            || !value
                .get("token")
                .and_then(Value::as_str)
                .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            || !value
                .get("port")
                .and_then(Value::as_u64)
                .is_some_and(|p| (1..=65535).contains(&p))
        {
            return Err("Screen endpoint identity is corrupt or belongs to another session".into());
        }
        Ok(value)
    }
}
pub fn exchange(
    location: &SessionLocation,
    kind: u8,
    payload: &[u8],
    timeout: Duration,
) -> Result<Frame, String> {
    let deadline = Instant::now() + timeout;
    let mut stream = crate::windows::connect(location, timeout).map_err(error)?;
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
                Err(e) => return Err(e.to_string()),
            }
        }
        match stream.read(&mut buffer) {
            Ok(0) => return Err("Screen control closed before replying".into()),
            Ok(n) => input.extend_from_slice(&buffer[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.to_string()),
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
