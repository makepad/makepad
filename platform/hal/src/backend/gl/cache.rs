//! Linked programs kept on disk: after a link the program's driver binary goes to the app's cache
//! dir, and the next launch loads it (`glProgramBinary`) instead of compiling the GLSL again. Binaries
//! are kept per driver and Android OS build, and a driver that won't take its own back stops getting them.

use super::sys::{self, GLenum, GLint, GLuint, Gl};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 4] = b"MPGL";
/// magic, binary format, the check hash, both source lengths, the binary's hash
const HEADER: usize = 4 + 4 + 8 + 4 + 4 + 8;
/// Marks a driver directory whose driver rejected a binary it wrote itself.
const REJECTED: &str = "rejected";

/// Which file a program's sources go to, and what that file must hold to be theirs.
#[derive(Clone, Copy)]
pub(super) struct Key {
    name: u64,
    check: u64,
    lens: [u32; 2],
}

impl Key {
    pub fn of(vertex: &str, fragment: &str) -> Self {
        Self { name: fxhash::hash64(&(vertex, fragment)), check: fxhash::hash64(&(fragment, vertex, MAGIC)), lens: [vertex.len() as u32, fragment.len() as u32] }
    }
}

pub(super) struct ProgramCache {
    /// `None` keeps nothing: there's no cache dir, or the driver has no binary formats or
    /// doesn't take its own binaries back
    dir: Option<PathBuf>,
}

impl ProgramCache {
    /// The directory of this driver's binaries, under `MAKEPAD_HAL_CACHE_DIR` or the app's cache dir.
    pub fn new(gl: &Gl) -> Self {
        if gl.integer(sys::NUM_PROGRAM_BINARY_FORMATS) <= 0 { return Self { dir: None }; }
        let driver = [sys::VENDOR, sys::RENDERER, sys::VERSION].map(|name| gl.string(name)).join("\n");
        #[cfg(target_os = "android")]
        let driver = driver + &crate::backend::android::xr::system_property("ro.build.fingerprint").unwrap_or_default();
        let Some(base) = base_dir() else { return Self { dir: None } };
        let parent = base.join("makepad-hal");
        let dir = parent.join(format!("gl-{:016x}", fxhash::hash64(&driver)));
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!("[hal-gl] program cache {}: {e}", dir.display());
            return Self { dir: None };
        }
        sweep(&parent, &dir);
        if dir.join(REJECTED).exists() { return Self { dir: None }; }
        Self { dir: Some(dir) }
    }

    /// Whether linked programs' binaries go to disk.
    pub fn keeps(&self) -> bool {
        self.dir.is_some()
    }

    fn path(&self, key: Key) -> Option<PathBuf> {
        self.dir.as_ref().map(|dir| dir.join(format!("{:016x}.bin", key.name)))
    }

    /// A new program made from the cached binary of `key`, if it's there and the driver takes it.
    pub fn load(&mut self, gl: &Gl, key: Key) -> Option<GLuint> {
        let path = self.path(key)?;
        let bytes = std::fs::read(&path).ok()?;
        let intact = bytes.len() > HEADER && &bytes[..4] == MAGIC && bytes[8..16] == key.check.to_le_bytes()
            && bytes[16..20] == key.lens[0].to_le_bytes() && bytes[20..24] == key.lens[1].to_le_bytes()
            && bytes[24..32] == fxhash::hash64(&bytes[HEADER..]).to_le_bytes();
        if !intact {
            let _ = std::fs::remove_file(&path);
            return None;
        }
        let program = unsafe {
            let format = GLenum::from_le_bytes(bytes[4..8].try_into().unwrap());
            let binary = &bytes[HEADER..];
            let program = (gl.glCreateProgram)();
            (gl.glProgramBinary)(program, format, binary.as_ptr() as *const _, binary.len() as i32);
            let mut ok = 0;
            (gl.glGetProgramiv)(program, sys::LINK_STATUS, &mut ok);
            if ok == 0 { (gl.glDeleteProgram)(program); }
            (ok != 0).then_some(program)
        };
        if program.is_none() {
            // This driver wrote the binary itself and won't take it back (the Android emulator's
            // doesn't), so we stop keeping its binaries instead of compiling and writing every launch.
            eprintln!("[hal-gl] this GL driver doesn't load its own program binaries, so they're no longer kept");
            if let Some(dir) = self.dir.take() { let _ = std::fs::write(dir.join(REJECTED), b""); }
        }
        program
    }

    /// Keeps the binary of a program linked from the sources of `key` (through a temp file, so
    /// a launch never reads half of one).
    pub fn store(&self, gl: &Gl, key: Key, program: GLuint) {
        let Some(path) = self.path(key) else { return };
        let mut len: GLint = 0;
        unsafe { (gl.glGetProgramiv)(program, sys::PROGRAM_BINARY_LENGTH, &mut len) };
        if len <= 0 { return; }
        let mut bytes = vec![0u8; HEADER + len as usize];
        let (mut written, mut format) = (0, 0);
        unsafe { (gl.glGetProgramBinary)(program, len, &mut written, &mut format, bytes[HEADER..].as_mut_ptr() as *mut _) };
        if written <= 0 { return; }
        bytes.truncate(HEADER + written as usize);
        let hash = fxhash::hash64(&bytes[HEADER..]);
        bytes[..4].copy_from_slice(MAGIC);
        bytes[4..8].copy_from_slice(&format.to_le_bytes());
        bytes[8..16].copy_from_slice(&key.check.to_le_bytes());
        bytes[16..20].copy_from_slice(&key.lens[0].to_le_bytes());
        bytes[20..24].copy_from_slice(&key.lens[1].to_le_bytes());
        bytes[24..32].copy_from_slice(&hash.to_le_bytes());
        let temp = path.with_extension(format!("{}.tmp", std::process::id()));
        if let Err(e) = std::fs::write(&temp, &bytes).and_then(|_| std::fs::rename(&temp, &path)) {
            let _ = std::fs::remove_file(&temp);
            eprintln!("[hal-gl] program cache {}: {e}", path.display());
        }
    }
}

/// `MAKEPAD_HAL_CACHE_DIR`, else the app's cache dir (Android), XDG_CACHE_HOME or ~/.cache.
fn base_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("MAKEPAD_HAL_CACHE_DIR") { return Some(dir.into()); }
    #[cfg(target_os = "android")]
    {
        let cache = crate::backend::android::AndroidBackend::params().cache_path;
        if !cache.is_empty() { return Some(cache.into()); }
        std::env::var_os("TMPDIR").map(Into::into)
    }
    #[cfg(not(target_os = "android"))]
    std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
}

/// Deletes the temp files of processes that died mid-write, and on Android (where the cache dir
/// is the app's alone) the binaries of every other driver or OS build.
fn sweep(parent: &Path, dir: &Path) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|n| n.strip_suffix(".tmp")).and_then(|n| n.rsplit('.').next()) else { continue };
        if !Path::new("/proc").join(pid).exists() { let _ = std::fs::remove_file(entry.path()); }
    }
    if cfg!(target_os = "android") {
        for entry in std::fs::read_dir(parent).into_iter().flatten().flatten() {
            if entry.path() != dir && entry.file_name().to_str().is_some_and(|n| n.starts_with("gl-")) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
}
