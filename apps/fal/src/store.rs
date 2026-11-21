//! What the app keeps on disk, all under `~/.makepad/fal`: the API key, the
//! pictures it made (with their prompt and request beside them, since Fal's
//! result URLs expire) and the sweeps.
//!
//! The key file is read once at startup and written when the person presses
//! Save: a few bytes, on the UI thread. Everything bigger (pictures, sidecars,
//! sweep tables) is written on the app's task pool, and the outcome comes back
//! as a `Saved` action.

use crate::fal;
use makepad_widgets::makepad_platform::thread::Class;
use makepad_widgets::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// `~/.makepad/fal` (Makepad's per-user state, `MAKEPAD_HOME` when set). A
/// test run's own home (an absolute `MAKEPAD_TEST_HOME`) stands in for the
/// person's, so a test never reads or writes their key or pictures.
pub fn fal_dir() -> Option<PathBuf> {
    if let Some(test_home) = std::env::var_os("MAKEPAD_TEST_HOME").map(PathBuf::from).filter(|home| home.is_absolute()) {
        return Some(test_home.join(".makepad").join("fal"));
    }
    Some(makepad_widgets::makepad_platform::home::makepad_home().join("fal"))
}

pub fn key_path() -> Option<PathBuf> {
    Some(fal_dir()?.join("key"))
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum KeySource {
    File,
    Env,
}

impl KeySource {
    pub fn describe(self) -> &'static str {
        match self {
            KeySource::File => "saved in ~/.makepad/fal/key",
            KeySource::Env => "from FAL_KEY",
        }
    }
}

/// The key the app should use: the saved file first (the person's own choice
/// in this window), else the `FAL_KEY` variable. The second value is a
/// problem worth showing (an unreadable or empty file), never the key.
pub fn load_key() -> (Option<(String, KeySource)>, Option<String>) {
    let mut problem = None;
    if let Some(path) = key_path() {
        match std::fs::read_to_string(&path) {
            Ok(text) => match fal::parse_key_text(&text) {
                Some(key) => return (Some((key, KeySource::File)), None),
                None => problem = Some("The key file is empty or not a key.".to_string()),
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => problem = Some(format!("The key file can't be read ({}).", err.kind())),
        }
    }
    if let Ok(text) = std::env::var("FAL_KEY") {
        if let Some(key) = fal::parse_key_text(&text) {
            return (Some((key, KeySource::Env)), problem);
        }
    }
    (None, problem)
}

/// Write the key: the folder 0700, the file 0600, created that way (never
/// readable by others, not even for a moment).
pub fn save_key(key: &str) -> Result<(), String> {
    let path = key_path().ok_or("The home folder is unknown.")?;
    let dir = path.parent().ok_or("No folder for the key.")?;
    create_private_dir(dir)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path).map_err(|err| format!("The key can't be saved ({}).", err.kind()))?;
    // A file that existed before keeps its old mode through `open`: set it.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    use std::io::Write;
    file.write_all(key.as_bytes())
        .and_then(|_| file.write_all(b"\n"))
        .map_err(|err| format!("The key can't be saved ({}).", err.kind()))
}

pub fn forget_key() -> Result<(), String> {
    let path = key_path().ok_or("The home folder is unknown.")?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("The key file can't be removed ({}).", err.kind())),
    }
}

fn create_private_dir(dir: &Path) -> Result<(), String> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir).map_err(|err| format!("{} can't be created ({}).", dir.display(), err.kind()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

/// Now as (unix seconds, "YYYY-MM-DD", "YYYY-MM-DD_HH-MM-SS"), in UTC.
pub fn now_stamps() -> (u64, String, String) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, mo, d, h, mi, s) = fal::civil_utc(secs);
    (secs, format!("{y:04}-{mo:02}-{d:02}"), format!("{y:04}-{mo:02}-{d:02}_{h:02}-{mi:02}-{s:02}"))
}

/// The folder for one day's pictures.
pub fn images_dir(date: &str) -> Option<PathBuf> {
    Some(fal_dir()?.join("images").join(date))
}

pub fn sweeps_dir() -> Option<PathBuf> {
    Some(fal_dir()?.join("sweeps"))
}

/// One file to write: its path and its bytes.
pub struct FileWrite {
    pub path: PathBuf,
    pub bytes: Arc<[u8]>,
}

/// A batch of writes finished: per path, None on success or the error.
#[derive(Debug)]
pub struct Saved {
    pub results: Vec<(PathBuf, Option<String>)>,
}

/// Write files on the task pool; a `Saved` action reports back.
pub fn write_files(cx: &mut Cx, files: Vec<FileWrite>) {
    let job = move || {
        let mut results = Vec::with_capacity(files.len());
        for file in files {
            let outcome = match file.path.parent() {
                Some(dir) => create_private_dir(dir).and_then(|_| {
                    std::fs::write(&file.path, &file.bytes[..]).map_err(|err| format!("{} ({})", file.path.display(), err.kind()))
                }),
                None => Err("no folder".to_string()),
            };
            results.push((file.path, outcome.err()));
        }
        Cx::post_action(Saved { results });
    };
    match cx.task_pool().submit(Class::Background, job) {
        Ok(handle) => handle.detach(),
        Err(err) => log!("fal: the files could not be queued for writing ({err:?})"),
    }
}

/// The extension a saved picture gets, from its content type or its URL.
pub fn image_extension(content_type: Option<&str>, url: &str) -> &'static str {
    let from_type = match content_type.unwrap_or("") {
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/webp" => Some("webp"),
        "image/gif" => Some("gif"),
        _ => None,
    };
    if let Some(ext) = from_type {
        return ext;
    }
    let path = url.split(['?', '#']).next().unwrap_or("");
    match path.rsplit('.').next().map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("png") => "png",
        Some("jpg") | Some("jpeg") => "jpg",
        Some("webp") => "webp",
        Some("gif") => "gif",
        _ => "img",
    }
}
