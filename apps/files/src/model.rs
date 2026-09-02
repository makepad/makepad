//! The entry model every view shares: what a directory holds, how it sorts,
//! how a file's kind is decided, and which app owns it.
//!
//! Nothing here touches the UI, so all of it is unit-testable.

use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// What a file *is*, as far as the browser is concerned: it picks the icon,
/// fills the Kind column, and decides whether Space can preview it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FileKind {
    Folder,
    Image,
    Text,
    Code,
    Audio,
    Video,
    Archive,
    Pdf,
    #[default]
    Generic,
}

impl FileKind {
    /// The SVG basename under `resources/icons/`.
    pub fn icon_name(self) -> &'static str {
        match self {
            FileKind::Folder => "folder",
            FileKind::Image => "image",
            FileKind::Text => "text",
            FileKind::Code => "code",
            FileKind::Audio => "audio",
            FileKind::Video => "video",
            FileKind::Archive => "archive",
            FileKind::Pdf => "pdf",
            FileKind::Generic => "file",
        }
    }

    /// The word shown in the Kind column.
    pub fn label(self) -> &'static str {
        match self {
            FileKind::Folder => "Folder",
            FileKind::Image => "Image",
            FileKind::Text => "Text",
            FileKind::Code => "Code",
            FileKind::Audio => "Audio",
            FileKind::Video => "Video",
            FileKind::Archive => "Archive",
            FileKind::Pdf => "PDF",
            FileKind::Generic => "File",
        }
    }

    /// Kinds the in-app quick look can render as text.
    pub fn is_textual(self) -> bool {
        matches!(self, FileKind::Text | FileKind::Code)
    }
}

/// Extensions the makepad image cache can decode (`detect_image_format` in
/// `draw/src/image_cache.rs`) — exactly the set that gets a real thumbnail.
pub const IMAGE_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "gif", "bmp", "qoi", "ico",
];

const CODE_EXTS: &[&str] = &[
    "rs", "c", "h", "cpp", "hpp", "cc", "js", "ts", "tsx", "jsx", "py", "go", "rb", "java", "kt",
    "swift", "sh", "zsh", "bash", "fish", "lua", "vim", "toml", "yaml", "yml", "json", "xml",
    "html", "css", "scss", "sql", "splash", "glsl", "wgsl", "metal", "m", "mm", "cs", "php",
    "pl", "r", "jl", "zig", "nim", "hs", "ml", "ex", "exs", "gradle", "cmake", "mk",
];

const TEXT_EXTS: &[&str] = &[
    "txt", "md", "markdown", "log", "csv", "tsv", "ini", "cfg", "conf", "rst", "org", "tex",
    "gitignore", "lock", "license", "readme",
];

const AUDIO_EXTS: &[&str] = &[
    "mp3", "wav", "flac", "ogg", "oga", "m4a", "aac", "aiff", "aif", "opus", "wma", "mid",
    "midi",
];

const VIDEO_EXTS: &[&str] = &[
    "mp4", "mov", "mkv", "webm", "avi", "m4v", "wmv", "flv", "mpg", "mpeg", "ts",
];

/// The videos the platform decoder demuxes, i.e. the ones that can get a real
/// first-frame thumbnail and an `video` association. The rest of
/// [`VIDEO_EXTS`] still reads as a video, it just gets the film-strip icon and
/// the desktop's own opener.
pub const PLAYABLE_VIDEO_EXTS: &[&str] = &["mp4", "mov", "m4v", "webm", "mkv", "avi"];

const ARCHIVE_EXTS: &[&str] = &[
    "zip", "tar", "gz", "tgz", "bz2", "xz", "zst", "7z", "rar", "dmg", "pkg", "iso", "jar",
    "whl", "deb", "rpm",
];

// There is no association table here: `makepad_wm_api::viewer_for` is the one the
// window manager and the browser share.

/// Lowercased extension of `path`, or "" when it has none.
pub fn extension_of(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// True when the makepad image cache can decode this file.
pub fn is_image_file(path: &Path) -> bool {
    IMAGE_EXTS.contains(&extension_of(path).as_str())
}

/// True when the platform video decoder can pull a first frame out of it.
pub fn is_playable_video(path: &Path) -> bool {
    PLAYABLE_VIDEO_EXTS.contains(&extension_of(path).as_str())
}

/// True when this file can get a real thumbnail instead of a type icon.
pub fn is_thumbnailable(path: &Path) -> bool {
    is_image_file(path) || is_playable_video(path)
}

/// Classify a directory entry.
pub fn kind_for(path: &Path, is_dir: bool) -> FileKind {
    if is_dir {
        return FileKind::Folder;
    }
    let ext = extension_of(path);
    if ext == "pdf" {
        return FileKind::Pdf;
    }
    if IMAGE_EXTS.contains(&ext.as_str()) {
        return FileKind::Image;
    }
    if CODE_EXTS.contains(&ext.as_str()) {
        return FileKind::Code;
    }
    if TEXT_EXTS.contains(&ext.as_str()) {
        return FileKind::Text;
    }
    if AUDIO_EXTS.contains(&ext.as_str()) {
        return FileKind::Audio;
    }
    if VIDEO_EXTS.contains(&ext.as_str()) {
        return FileKind::Video;
    }
    if ARCHIVE_EXTS.contains(&ext.as_str()) {
        return FileKind::Archive;
    }
    // Dotfiles with no extension (.zshrc, .gitconfig) read as text.
    if ext.is_empty() && path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
        return FileKind::Text;
    }
    FileKind::Generic
}

/// One row of a directory listing.
#[derive(Clone, Debug)]
pub struct FileEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    /// Seconds since the epoch; 0 when unknown. The sort key for Modified.
    pub modified_secs: u64,
    /// Creation (birth) time where the filesystem reports one, else 0.
    pub created_secs: u64,
    /// The mode as `rwxr-xr-x`, or a read-only/read-write word off unix.
    pub permissions: String,
    /// Entries inside a folder; `None` when it could not be read (and for
    /// files). Counting is bounded — see [`FOLDER_COUNT_CAP`].
    pub child_count: Option<u32>,
    pub kind: FileKind,
}

impl FileEntry {
    /// The Modified column: an absolute local timestamp, year always.
    pub fn modified_text(&self) -> String {
        format_stamp(self.modified_secs)
    }

    /// The Created column.
    pub fn created_text(&self) -> String {
        format_stamp(self.created_secs)
    }

    /// The Size column: bytes for a file, the item count for a folder.
    pub fn size_text(&self) -> String {
        match (self.is_dir, self.child_count) {
            (true, Some(1)) => "1 item".to_string(),
            (true, Some(n)) => format!("{} items", n),
            (true, None) => "—".to_string(),
            _ => format_size(self.size, false),
        }
    }

    /// The Kind column: the descriptive name of the file type.
    pub fn kind_text(&self) -> String {
        kind_label(&self.path, self.is_dir, self.kind)
    }
}

/// Never walk more than this many entries to count a folder — a listing must
/// not turn into a filesystem crawl.
pub const FOLDER_COUNT_CAP: u32 = 50_000;

/// The descriptive type name for the Kind column: the extension in words when
/// we know it, else the broad kind.
pub fn kind_label(path: &Path, is_dir: bool, kind: FileKind) -> String {
    if is_dir {
        return "Folder".to_string();
    }
    let ext = extension_of(path);
    let named = match ext.as_str() {
        "png" => "PNG image",
        "jpg" | "jpeg" => "JPEG image",
        "gif" => "GIF image",
        "webp" => "WebP image",
        "bmp" => "Bitmap image",
        "qoi" => "QOI image",
        "ico" => "Icon",
        "svg" => "SVG drawing",
        "mp4" | "m4v" => "MPEG-4 video",
        "mov" => "QuickTime video",
        "mkv" => "Matroska video",
        "webm" => "WebM video",
        "avi" => "AVI video",
        "mp3" => "MP3 audio",
        "wav" => "WAV audio",
        "flac" => "FLAC audio",
        "ogg" | "oga" | "opus" => "Ogg audio",
        "m4a" | "aac" => "AAC audio",
        "pdf" => "PDF document",
        "md" | "markdown" => "Markdown text",
        "txt" => "Plain text",
        "csv" => "CSV table",
        "tsv" => "TSV table",
        "json" => "JSON data",
        "toml" => "TOML data",
        "yaml" | "yml" => "YAML data",
        "xml" => "XML data",
        "html" | "htm" => "HTML page",
        "rs" => "Rust source",
        "splash" => "Splash source",
        "zip" => "ZIP archive",
        "tar" => "Tar archive",
        "gz" | "tgz" => "Gzip archive",
        "dmg" => "Disk image",
        "" => return kind.label().to_string(),
        _ => "",
    };
    if !named.is_empty() {
        return named.to_string();
    }
    // Unknown extension: name it after itself, with the broad kind behind it.
    match kind {
        FileKind::Generic => format!("{} file", ext.to_uppercase()),
        _ => format!("{} {}", ext.to_uppercase(), kind.label().to_lowercase()),
    }
}

/// Which column the listing is ordered by. Folders come first regardless —
/// that is what a file manager means by "sorted".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortKey {
    #[default]
    Name,
    Size,
    Kind,
    Modified,
    Created,
    Permissions,
}

impl SortKey {
    /// Every column the list view can show, in their natural order. Which of
    /// them are on screen is the view's business; this is the whole set.
    pub const ALL: [SortKey; 6] = [
        SortKey::Name,
        SortKey::Size,
        SortKey::Kind,
        SortKey::Modified,
        SortKey::Created,
        SortKey::Permissions,
    ];

    /// The column header.
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Name => "Name",
            SortKey::Size => "Size",
            SortKey::Kind => "Kind",
            SortKey::Modified => "Modified",
            SortKey::Created => "Created",
            SortKey::Permissions => "Permissions",
        }
    }

    /// A sensible starting width for the column, in points.
    pub fn default_width(self) -> f64 {
        match self {
            SortKey::Name => 320.0,
            SortKey::Size => 110.0,
            SortKey::Kind => 130.0,
            SortKey::Modified | SortKey::Created => 168.0,
            SortKey::Permissions => 116.0,
        }
    }

    /// Numbers and dates read right-aligned; words read left-aligned.
    pub fn align(self) -> f64 {
        match self {
            SortKey::Size => 1.0,
            _ => 0.0,
        }
    }

    /// This column's text for an entry.
    pub fn text(self, entry: &FileEntry) -> String {
        match self {
            SortKey::Name => entry.name.clone(),
            SortKey::Size => entry.size_text(),
            SortKey::Kind => entry.kind_text(),
            SortKey::Modified => entry.modified_text(),
            SortKey::Created => entry.created_text(),
            SortKey::Permissions => entry.permissions.clone(),
        }
    }
}

/// The active ordering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SortSpec {
    pub key: SortKey,
    pub ascending: bool,
}

impl Default for SortSpec {
    fn default() -> Self {
        Self {
            key: SortKey::Name,
            ascending: true,
        }
    }
}

/// Order `order` (indices into `entries`) by `sort`, folders always first.
pub fn sort_indices(entries: &[FileEntry], order: &mut [usize], sort: SortSpec) {
    order.sort_by(|a, b| {
        let (l, r) = (&entries[*a], &entries[*b]);
        // Folders first, always: a listing that scatters them is unreadable.
        let dirs = r.is_dir.cmp(&l.is_dir);
        if dirs != std::cmp::Ordering::Equal {
            return dirs;
        }
        let by_key = match sort.key {
            SortKey::Name => std::cmp::Ordering::Equal,
            // Folders sort by how much they hold, files by their bytes.
            SortKey::Size => l
                .child_count
                .cmp(&r.child_count)
                .then_with(|| l.size.cmp(&r.size)),
            SortKey::Kind => l.kind_text().cmp(&r.kind_text()),
            SortKey::Modified => l.modified_secs.cmp(&r.modified_secs),
            SortKey::Created => l.created_secs.cmp(&r.created_secs),
            SortKey::Permissions => l.permissions.cmp(&r.permissions),
        };
        let by_key = if sort.ascending {
            by_key
        } else {
            by_key.reverse()
        };
        if by_key != std::cmp::Ordering::Equal {
            return by_key;
        }
        // Name is the tiebreaker for every key, and reverses with the sort so
        // a descending listing is the exact mirror of the ascending one.
        let by_name = l
            .name
            .to_lowercase()
            .cmp(&r.name.to_lowercase())
            .then_with(|| l.name.cmp(&r.name));
        if sort.ascending || sort.key != SortKey::Name {
            by_name
        } else {
            by_name.reverse()
        }
    });
}

/// Folder names directly under the user's home that the size map never
/// enters.
///
/// This is not a taste decision, it is what makes the map usable on macOS at
/// all. `~/Library` is Apple's, not the user's: it is where Containers, Group
/// Containers, Mail, Messages, Safari, CloudStorage and Mobile Documents live,
/// and every one of them is behind a separate TCC grant — walking it means a
/// permission dialog per protected folder, over and over, for bytes the user
/// cannot delete by hand anyway. `~/.Trash` is not the user's files either;
/// it is what they already threw away, and counting it would double every
/// number the moment they trashed something.
///
/// `MAKEPAD_FILES_SCAN_ALL=1` turns the whole rule off for anyone who wants the
/// literal truth about their home directory and does not mind the dialogs.
const HOME_SKIP: [&str; 2] = ["Library", ".Trash"];

/// Whether the size map measures the system folders too. Off by default —
/// the map skips ~/Library and ~/.Trash so macOS never storms the user with
/// permission dialogs — and flipped by the "ignore system" checkbox on the
/// map's tool strip. `MAKEPAD_FILES_SCAN_ALL=1` or a saved preference turns it on
/// at startup; every change is written back so the choice survives launches.
pub fn scan_all() -> bool {
    *scan_all_flag().lock().unwrap_or_else(|e| e.into_inner())
}

/// Change the scope and remember it. The caller owns triggering the rescan.
pub fn set_scan_all(on: bool) {
    *scan_all_flag().lock().unwrap_or_else(|e| e.into_inner()) = on;
    pref_set("scan_all", if on { "1" } else { "0" });
}

fn scan_all_flag() -> &'static std::sync::Mutex<bool> {
    static FLAG: std::sync::OnceLock<std::sync::Mutex<bool>> = std::sync::OnceLock::new();
    FLAG.get_or_init(|| {
        if std::env::var_os("MAKEPAD_FILES_SCAN_ALL").is_some_and(|v| v != "0") {
            return std::sync::Mutex::new(true);
        }
        std::sync::Mutex::new(pref_get("scan_all").as_deref() == Some("1"))
    })
}

/// Where the little `key=value` preference file lives.
fn prefs_path() -> PathBuf {
    makepad_home().join("files/prefs")
}

fn memory_prefs() -> &'static std::sync::Mutex<String> {
    static PREFS: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
    PREFS.get_or_init(|| std::sync::Mutex::new(String::new()))
}

/// One saved preference, by key. The file is `key=value` lines, nothing
/// more; a missing file is simply no preferences.
pub fn pref_get(key: &str) -> Option<String> {
    if cfg!(test) || cfg!(feature = "demo") || crate::vfs::is_demo() {
        let text = memory_prefs().lock().unwrap_or_else(|e| e.into_inner());
        return pref_find(&text, key);
    }
    let text = std::fs::read_to_string(prefs_path()).ok()?;
    pref_find(&text, key)
}

/// Save one preference, leaving every other key exactly as it was — the
/// file is shared by whatever small choices the app remembers.
pub fn pref_set(key: &str, value: &str) {
    if cfg!(test) || cfg!(feature = "demo") || crate::vfs::is_demo() {
        let mut text = memory_prefs().lock().unwrap_or_else(|e| e.into_inner());
        *text = pref_replace(&text, key, value);
        return;
    }
    let path = prefs_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let _ = std::fs::write(&path, pref_replace(&old, key, value));
}

fn pref_find(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (k, v) = line.trim().split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

fn pref_replace(old: &str, key: &str, value: &str) -> String {
    let mut out = String::new();
    let mut written = false;
    for line in old.lines() {
        match line.trim().split_once('=') {
            Some((k, _)) if k == key => {
                if !written {
                    out.push_str(&format!("{key}={value}\n"));
                    written = true;
                }
            }
            _ if !line.trim().is_empty() => {
                out.push_str(line);
                out.push('\n');
            }
            _ => {}
        }
    }
    if !written {
        out.push_str(&format!("{key}={value}\n"));
    }
    out
}

/// True for a folder the size map must not enter.
///
/// Only ever consulted for directories, and only for the ones directly under
/// the user's home — a `Library` folder inside a project is a project's
/// library and gets measured like anything else.
pub fn skip_for_scan(path: &Path, home: &Path) -> bool {
    !scan_all() && home_scan_exclusion(path, home)
}

pub(crate) fn home_scan_exclusion(path: &Path, home: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    if parent != home {
        return false;
    }
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| HOME_SKIP.contains(&name))
}

/// What scope the map's numbers were measured under — always said, in both
/// states, so nobody misreads a total.
pub fn scan_exclusions() -> Option<String> {
    if scan_all() {
        return Some("including system folders".to_string());
    }
    Some("excluding Library and Trash".to_string())
}

/// One entry, read straight off the disk by path rather than found in a
/// listing. The treemap needs this: nearly everything it draws lives below the
/// folder the browser is listing, and a context menu on a file three folders
/// down has to describe that file, not fail to find a row for it.
///
/// `None` when the active filesystem has nothing there. Both real metadata
/// and virtual metadata arrive through `Vfs::stat`.
pub fn entry_at(path: &Path) -> Option<FileEntry> {
    crate::vfs::vfs().stat(path).ok()
}

pub(crate) fn real_entry_at(path: &Path) -> Option<FileEntry> {
    let metadata = fs::metadata(path).ok()?;
    let is_dir = metadata.is_dir();
    Some(FileEntry {
        name: display_name(path),
        kind: kind_for(path, is_dir),
        is_dir,
        size: if is_dir { 0 } else { metadata.len() },
        modified_secs: epoch_secs(metadata.modified().ok()),
        created_secs: epoch_secs(metadata.created().ok()),
        permissions: permissions_text(&metadata),
        child_count: is_dir.then(|| count_children(path)).flatten(),
        path: path.to_path_buf(),
    })
}

/// Read one directory. Runs on a worker thread — never the UI thread.
pub fn read_directory(path: &Path, show_hidden: bool) -> Result<Vec<FileEntry>, String> {
    let read_dir = fs::read_dir(path)
        .map_err(|error| format!("Could not read {}: {}", path.display(), error))?;
    let mut entries = Vec::new();
    for item in read_dir.flatten() {
        let name = item.file_name().to_string_lossy().into_owned();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        // `metadata` follows symlinks so a link to a folder browses like one;
        // a broken link has no metadata and is skipped.
        let Ok(metadata) = item.metadata() else {
            continue;
        };
        let path = item.path();
        let is_dir = metadata.is_dir();
        entries.push(FileEntry {
            kind: kind_for(&path, is_dir),
            name,
            is_dir,
            size: if is_dir { 0 } else { metadata.len() },
            modified_secs: epoch_secs(metadata.modified().ok()),
            created_secs: epoch_secs(metadata.created().ok()),
            permissions: permissions_text(&metadata),
            // One extra `read_dir` per folder, on this worker thread — never
            // on the UI thread, and never past the cap.
            child_count: is_dir.then(|| count_children(&path)).flatten(),
            path,
        });
    }
    let mut order: Vec<usize> = (0..entries.len()).collect();
    sort_indices(&entries, &mut order, SortSpec::default());
    Ok(order.into_iter().map(|i| entries[i].clone()).collect())
}

fn epoch_secs(time: Option<SystemTime>) -> u64 {
    time.and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The mode as `rwxr-xr-x` on unix; the writability elsewhere.
fn permissions_text(metadata: &fs::Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode();
        let bit = |shift: u32, chars: [char; 2]| {
            if mode >> shift & 1 == 1 {
                chars[0]
            } else {
                chars[1]
            }
        };
        (0..9)
            .rev()
            .map(|i| match i % 3 {
                2 => bit(i, ['r', '-']),
                1 => bit(i, ['w', '-']),
                _ => bit(i, ['x', '-']),
            })
            .collect()
    }
    #[cfg(not(unix))]
    {
        if metadata.permissions().readonly() {
            "read-only".to_string()
        } else {
            "read-write".to_string()
        }
    }
}

/// How many entries a folder holds, up to [`FOLDER_COUNT_CAP`]; `None` when
/// it cannot be read (permissions, a vanished directory).
fn count_children(path: &Path) -> Option<u32> {
    let read_dir = fs::read_dir(path).ok()?;
    Some(read_dir.take(FOLDER_COUNT_CAP as usize).count() as u32)
}

/// "1.5 KB" / "—" for folders.
pub fn format_size(bytes: u64, is_dir: bool) -> String {
    if is_dir {
        return "—".to_string();
    }
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else if value >= 10.0 {
        format!("{:.0} {}", value, UNITS[unit])
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

pub fn real_now_secs() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        return SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or(std::time::Duration::ZERO)
            .as_secs();
    }
    #[cfg(target_arch = "wasm32")]
    {
        makepad_widgets::Cx::time_now().max(0.0) as u64
    }
}

/// The machine's UTC offset in seconds, read once. The platform has no
/// timezone database, so we ask the system's own `date` — which knows about
/// DST — instead of guessing.
pub fn local_utc_offset_secs() -> i64 {
    static OFFSET: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *OFFSET.get_or_init(|| {
        #[cfg(all(not(target_arch = "wasm32"), any(target_os = "macos", target_os = "linux")))]
        {
            let Ok(out) = std::process::Command::new("date").arg("+%z").output() else {
                return 0;
            };
            return parse_utc_offset(String::from_utf8_lossy(&out.stdout).trim());
        }
        #[cfg(any(target_arch = "wasm32", not(any(target_os = "macos", target_os = "linux"))))]
        0
    })
}

/// `+0200` / `-0730` -> seconds east of UTC.
#[cfg(not(target_arch = "wasm32"))]
fn parse_utc_offset(text: &str) -> i64 {
    let bytes = text.as_bytes();
    if bytes.len() < 5 || (bytes[0] != b'+' && bytes[0] != b'-') {
        return 0;
    }
    let Ok(hours) = text[1..3].parse::<i64>() else {
        return 0;
    };
    let Ok(minutes) = text[3..5].parse::<i64>() else {
        return 0;
    };
    let magnitude = hours * 3600 + minutes * 60;
    if bytes[0] == b'-' {
        -magnitude
    } else {
        magnitude
    }
}

/// Days since the epoch to (year, month, day) — Howard Hinnant's
/// civil_from_days.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// A file timestamp as the file manager shows it: local time, the year
/// always, to the minute — "Aug 27, 2026 21:54".
pub fn format_stamp(secs: u64) -> String {
    format_stamp_at(secs, local_utc_offset_secs())
}

/// [`format_stamp`] with an explicit offset, so it can be tested.
pub fn format_stamp_at(secs: u64, offset_secs: i64) -> String {
    if secs == 0 {
        return "—".to_string();
    }
    let local = secs as i64 + offset_secs;
    let days = local.div_euclid(86_400);
    let time = local.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{} {}, {} {:02}:{:02}",
        MONTHS[(month as usize - 1).min(11)],
        day,
        year,
        time / 3600,
        (time % 3600) / 60
    )
}

/// "3 hr ago" for a timestamp `secs`, relative to `now`.
pub fn format_age(secs: u64, now: u64) -> String {
    if secs == 0 {
        return "Unknown".to_string();
    }
    let age = now.saturating_sub(secs);
    match age {
        a if a < 60 => "Just now".to_string(),
        a if a < 3600 => format!("{} min ago", a / 60),
        a if a < 86_400 => format!("{} hr ago", a / 3600),
        a if a < 604_800 => format!("{} days ago", a / 86_400),
        a if a < 31_536_000 => format!("{} weeks ago", a / 604_800),
        a => format!("{} years ago", a / 31_536_000),
    }
}

/// The first `lines` lines of a text file, for the in-app quick look.
pub fn read_head(path: &Path, lines: usize, max_bytes: usize) -> Result<String, String> {
    let data = crate::vfs::vfs().read_bytes(path, max_bytes)?;
    let cut = data.len();
    // Never split a UTF-8 sequence: back off to the last boundary in the cut.
    let text = match std::str::from_utf8(&data[..cut]) {
        Ok(text) => text.to_string(),
        Err(e) => String::from_utf8_lossy(&data[..e.valid_up_to()]).into_owned(),
    };
    let mut out = String::new();
    let mut truncated = false;
    for (i, line) in text.lines().enumerate() {
        if i >= lines {
            truncated = true;
            break;
        }
        if i > 0 {
            out.push('\n');
        }
        out.push_str(line);
    }
    if truncated {
        out.push_str("\n…");
    }
    Ok(out)
}

/// The user's home, or the cwd, or `/`.
pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Where deleted files go. The operations engine has to know this too and
/// cannot depend on this module, so it owns the definition and this is the
/// one name the rest of the app uses.
pub fn trash_dir(home: &Path) -> PathBuf {
    crate::ops::trash_dir(home)
}

/// The last path component, falling back to the whole path for `/`.
pub fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The preference file is shared by every small choice the app keeps, so
    // writing one key must never eat the others.
    #[test]
    fn setting_one_preference_keeps_the_rest() {
        let text = "scan_all=1\nprojection=ortho\n";
        assert_eq!(pref_find(text, "projection").as_deref(), Some("ortho"));
        assert_eq!(pref_find(text, "missing"), None);
        let replaced = pref_replace(text, "projection", "persp");
        assert_eq!(pref_find(&replaced, "projection").as_deref(), Some("persp"));
        assert_eq!(pref_find(&replaced, "scan_all").as_deref(), Some("1"));
        // A new key appends; nothing else moves.
        let grown = pref_replace(&replaced, "filter_side", "1");
        assert_eq!(pref_find(&grown, "filter_side").as_deref(), Some("1"));
        assert_eq!(pref_find(&grown, "scan_all").as_deref(), Some("1"));
        assert_eq!(grown.lines().count(), 3);
    }

    // The rule that keeps macOS from throwing a permission dialog per
    // protected folder: those folders are never entered at all.
    #[test]
    fn the_map_leaves_apples_folders_alone_and_touches_nothing_else() {
        let home = Path::new("/active-home");
        assert!(home_scan_exclusion(&home.join("Library"), home));
        assert!(home_scan_exclusion(&home.join(".Trash"), home));
        // The user's own files, which is the entire point.
        assert!(!home_scan_exclusion(&home.join("Documents"), home));
        assert!(!home_scan_exclusion(&home.join("Pictures"), home));
        assert!(!home_scan_exclusion(&home.join("Downloads"), home));
        // Only *directly* under home. A project's own `Library` folder is the
        // project's, and gets measured like anything else in it.
        assert!(!home_scan_exclusion(&home.join("code/thing/Library"), home));
        assert!(!home_scan_exclusion(Path::new("/tmp/Library"), home));
        // Whatever it leaves out, it says so.
        assert!(scan_exclusions().is_some());
    }

    fn entry(name: &str, is_dir: bool, size: u64, modified: u64) -> FileEntry {
        let path = PathBuf::from("/x").join(name);
        FileEntry {
            kind: kind_for(&path, is_dir),
            path,
            name: name.to_string(),
            is_dir,
            size,
            modified_secs: modified,
            created_secs: modified,
            permissions: "rw-r--r--".to_string(),
            child_count: None,
        }
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(format_size(0, true), "—");
        assert_eq!(format_size(999, false), "999 B");
        assert_eq!(format_size(1500, false), "1.5 KB");
        assert_eq!(format_size(15_000, false), "15 KB");
        assert_eq!(format_size(2_500_000_000, false), "2.5 GB");
    }

    #[test]
    fn formats_absolute_stamps() {
        // 2026-08-27 21:34 UTC, shown in UTC.
        let secs = 1_787_866_440;
        assert_eq!(format_stamp_at(secs, 0), "Aug 27, 2026 21:34");
        // Two hours east is two hours later on the same clock.
        assert_eq!(format_stamp_at(secs, 2 * 3600), "Aug 27, 2026 23:34");
        // And crossing midnight rolls the date.
        assert_eq!(format_stamp_at(secs, 3 * 3600), "Aug 28, 2026 00:34");
        assert_eq!(format_stamp_at(0, 0), "—");
        assert_eq!(parse_utc_offset("+0200"), 7200);
        assert_eq!(parse_utc_offset("-0730"), -27000);
        assert_eq!(parse_utc_offset("garbage"), 0);
    }

    #[test]
    fn names_the_kinds_the_columns_show() {
        assert_eq!(kind_label(Path::new("/a/b"), true, FileKind::Folder), "Folder");
        assert_eq!(kind_label(Path::new("/a/x.mp4"), false, FileKind::Video), "MPEG-4 video");
        assert_eq!(kind_label(Path::new("/a/x.png"), false, FileKind::Image), "PNG image");
        assert_eq!(kind_label(Path::new("/a/x.rs"), false, FileKind::Code), "Rust source");
        // An extension we do not name still reads as itself.
        assert_eq!(kind_label(Path::new("/a/x.glb"), false, FileKind::Generic), "GLB file");
        assert_eq!(kind_label(Path::new("/a/x"), false, FileKind::Generic), "File");
    }

    #[test]
    fn sizes_read_as_bytes_or_item_counts() {
        let mut dir = entry("d", true, 0, 1);
        dir.child_count = Some(12);
        assert_eq!(dir.size_text(), "12 items");
        dir.child_count = Some(1);
        assert_eq!(dir.size_text(), "1 item");
        dir.child_count = None;
        assert_eq!(dir.size_text(), "—");
        let file = entry("f.bin", false, 2_000_000, 1);
        assert_eq!(file.size_text(), "2.0 MB");
    }

    #[test]
    fn every_column_has_a_header_and_a_cell() {
        let e = entry("x.png", false, 1234, 1_787_866_440);
        for key in SortKey::ALL {
            assert!(!key.label().is_empty());
            assert!(key.default_width() > 0.0);
            assert!(!key.text(&e).is_empty(), "{:?}", key);
        }
    }

    #[test]
    fn formats_ages() {
        assert_eq!(format_age(0, 1000), "Unknown");
        assert_eq!(format_age(990, 1000), "Just now");
        assert_eq!(format_age(1000, 8200), "2 hr ago");
    }

    #[test]
    fn classifies_kinds() {
        assert_eq!(kind_for(Path::new("/a/b"), true), FileKind::Folder);
        assert_eq!(kind_for(Path::new("/a/p.PNG"), false), FileKind::Image);
        assert_eq!(kind_for(Path::new("/a/m.rs"), false), FileKind::Code);
        assert_eq!(kind_for(Path::new("/a/n.md"), false), FileKind::Text);
        assert_eq!(kind_for(Path::new("/a/s.flac"), false), FileKind::Audio);
        assert_eq!(kind_for(Path::new("/a/v.mkv"), false), FileKind::Video);
        assert_eq!(kind_for(Path::new("/a/z.tar.gz"), false), FileKind::Archive);
        assert_eq!(kind_for(Path::new("/a/d.pdf"), false), FileKind::Pdf);
        assert_eq!(kind_for(Path::new("/a/.zshrc"), false), FileKind::Text);
        assert_eq!(kind_for(Path::new("/a/blob"), false), FileKind::Generic);
    }

    #[test]
    fn thumbnails_follow_what_can_be_decoded() {
        // Pictures and playable video get a real thumbnail.
        for ext in IMAGE_EXTS.iter().chain(PLAYABLE_VIDEO_EXTS) {
            let path = PathBuf::from(format!("/a/x.{ext}"));
            assert!(is_thumbnailable(&path), "{ext}");
        }
        for ext in PLAYABLE_VIDEO_EXTS {
            assert_eq!(
                kind_for(&PathBuf::from(format!("/a/x.{ext}")), false),
                FileKind::Video,
                "{ext}"
            );
        }
        // A video we cannot decode still reads as one; it just gets the icon.
        assert_eq!(kind_for(Path::new("/a/x.flv"), false), FileKind::Video);
        assert!(!is_thumbnailable(Path::new("/a/x.flv")));
        assert!(!is_thumbnailable(Path::new("/a/m.rs")));
    }

    #[test]
    fn sorts_folders_first_then_name() {
        let entries = vec![
            entry("zeta.txt", false, 10, 100),
            entry("Alpha", true, 0, 900),
            entry("beta.txt", false, 500, 300),
            entry("Gamma", true, 0, 50),
        ];
        let mut order: Vec<usize> = (0..entries.len()).collect();
        sort_indices(&entries, &mut order, SortSpec::default());
        let names: Vec<&str> = order.iter().map(|i| entries[*i].name.as_str()).collect();
        assert_eq!(names, ["Alpha", "Gamma", "beta.txt", "zeta.txt"]);
    }

    #[test]
    fn sorts_by_size_and_reverses() {
        let entries = vec![
            entry("a.txt", false, 10, 100),
            entry("dir", true, 0, 900),
            entry("b.txt", false, 500, 300),
        ];
        let mut order: Vec<usize> = (0..entries.len()).collect();
        sort_indices(
            &entries,
            &mut order,
            SortSpec {
                key: SortKey::Size,
                ascending: false,
            },
        );
        let names: Vec<&str> = order.iter().map(|i| entries[*i].name.as_str()).collect();
        // The folder still leads; files run big -> small.
        assert_eq!(names, ["dir", "b.txt", "a.txt"]);
    }

    #[test]
    fn descending_name_is_the_mirror() {
        let entries = vec![
            entry("a.txt", false, 1, 1),
            entry("b.txt", false, 2, 2),
            entry("c.txt", false, 3, 3),
        ];
        let mut asc: Vec<usize> = (0..3).collect();
        sort_indices(&entries, &mut asc, SortSpec::default());
        let mut desc: Vec<usize> = (0..3).collect();
        sort_indices(
            &entries,
            &mut desc,
            SortSpec {
                key: SortKey::Name,
                ascending: false,
            },
        );
        desc.reverse();
        assert_eq!(asc, desc);
    }

    #[test]
    fn the_column_set_is_complete_and_unique() {
        let mut labels: Vec<&str> = SortKey::ALL.iter().map(|k| k.label()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), SortKey::ALL.len());
        assert_eq!(SortKey::ALL[0], SortKey::Name);
        assert_eq!(SortKey::default(), SortKey::Name);
    }

    #[test]
    fn reads_a_head_of_lines() {
        let dir = std::env::temp_dir().join("files-test-head");
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("head.txt");
        fs::write(&file, "one\ntwo\nthree\nfour\n").unwrap();
        let head = read_head(&file, 2, 4096).unwrap();
        assert_eq!(head, "one\ntwo\n…");
        fs::remove_file(&file).ok();
    }
}

/// The makepad home directory (`MAKEPAD_HOME`, else the user home; a temp dir as a
/// last resort) — the same rule the AI hub uses, kept local so demo builds without the
/// chat feature do not link the hub for a path.
// The shared per-user home for Makepad AI state.
pub fn makepad_home() -> PathBuf {
    if let Some(home) = std::env::var_os("MAKEPAD_HOME") {
        return PathBuf::from(home);
    }
    // USERPROFILE on Windows, HOME elsewhere; temp dir as a last resort.
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(".makepad")
}
