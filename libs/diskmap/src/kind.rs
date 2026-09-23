//! File-kind tags the map stores as a `u8`, plus the handful of path helpers
//! the scan and the widget share. Discriminant order is load-bearing: a cached
//! tree records `FileKind as u8`, and the view paints from that byte.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

/// What a file *is*, as far as the map's colours are concerned.
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
    /// The SVG basename under the files app's `resources/icons/`.
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

    /// The word shown in a Kind column.
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
///
/// The scan calls this once per file. Matching the extension in place, without
/// allocating a lowercased `String`, is what keeps that from becoming its own
/// tax once the directory read itself is no longer one stat per file.
pub fn kind_for(path: &Path, is_dir: bool) -> FileKind {
    if is_dir {
        return FileKind::Folder;
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        return match path.file_name() {
            Some(name) => classify_name(name.as_bytes()),
            None => FileKind::Generic,
        };
    }
    #[cfg(not(unix))]
    {
        let ext_owned = extension_of(path);
        let dotfile = path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with('.'));
        let ext = if ext_owned.is_empty() {
            None
        } else {
            Some(ext_owned.as_bytes())
        };
        classify_extension(ext, dotfile)
    }
}

#[cfg(unix)]
fn classify_name(name: &[u8]) -> FileKind {
    let dotfile = name.first() == Some(&b'.');
    // The same rule as `Path::extension`: a leading dot is not an extension
    // (`.zshrc`), and the extension is the bytes after the last dot.
    let ext = match name.iter().rposition(|byte| *byte == b'.') {
        Some(0) | None => None,
        Some(dot) => {
            let ext = &name[dot + 1..];
            if ext.is_empty() { None } else { Some(ext) }
        }
    };
    classify_extension(ext, dotfile)
}

fn ext_eq(ext: &[u8], known: &str) -> bool {
    ext.len() == known.len()
        && ext
            .iter()
            .zip(known.bytes())
            .all(|(byte, expected)| byte.to_ascii_lowercase() == expected)
}

fn ext_one_of(ext: &[u8], known: &[&str]) -> bool {
    known.iter().any(|name| ext_eq(ext, name))
}

fn classify_extension(ext: Option<&[u8]>, dotfile: bool) -> FileKind {
    if let Some(ext) = ext {
        if ext_eq(ext, "pdf") {
            return FileKind::Pdf;
        }
        if ext_one_of(ext, IMAGE_EXTS) {
            return FileKind::Image;
        }
        if ext_one_of(ext, CODE_EXTS) {
            return FileKind::Code;
        }
        if ext_one_of(ext, TEXT_EXTS) {
            return FileKind::Text;
        }
        if ext_one_of(ext, AUDIO_EXTS) {
            return FileKind::Audio;
        }
        if ext_one_of(ext, VIDEO_EXTS) {
            return FileKind::Video;
        }
        if ext_one_of(ext, ARCHIVE_EXTS) {
            return FileKind::Archive;
        }
        return FileKind::Generic;
    }
    // Dotfiles with no extension (.zshrc, .gitconfig) read as text.
    if dotfile {
        FileKind::Text
    } else {
        FileKind::Generic
    }
}

/// The kind class a file's colour comes from: the index into the map palette's
/// `kinds`. Kept here rather than on `FileKind` because it is a property of
/// *this picture*, not of the file.
pub fn kind_class(kind: FileKind) -> u8 {
    match kind {
        FileKind::Video => 0,
        FileKind::Image => 1,
        FileKind::Audio => 2,
        FileKind::Code => 3,
        // A PDF reads as a document, which is what the text hue means here.
        FileKind::Text | FileKind::Pdf => 4,
        FileKind::Archive => 5,
        FileKind::Folder | FileKind::Generic => 6,
    }
}

/// The last path component, falling back to the whole path for `/`.
pub fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.display().to_string())
}

/// The user's home, or the cwd, or `/`.
pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// The makepad home directory (`MAKEPAD_HOME`, else the user home; a temp dir as a
/// last resort).
pub fn makepad_home() -> PathBuf {
    if let Some(home) = std::env::var_os("MAKEPAD_HOME") {
        return PathBuf::from(home);
    }
    #[cfg(target_arch = "wasm32")]
    return PathBuf::from("/.makepad");
    #[cfg(not(target_arch = "wasm32"))]
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(".makepad")
}

/// Containers, Mail, Messages, Safari, CloudStorage and Mobile Documents live,
/// and every one of them is behind a separate TCC grant — walking it means a
/// permission dialog per protected folder, over and over, for bytes the user
/// cannot delete by hand anyway. `~/.Trash` is not the user's files either;
/// it is what they already threw away, and counting it would double every
/// number the moment they trashed something.
const HOME_SKIP: [&str; 2] = ["Library", ".Trash"];

static SCAN_ALL: AtomicBool = AtomicBool::new(false);
static SCAN_ALL_INIT: AtomicBool = AtomicBool::new(false);

fn ensure_scan_all_init() {
    if SCAN_ALL_INIT.swap(true, Ordering::Relaxed) {
        return;
    }
    if std::env::var_os("MAKEPAD_FILES_SCAN_ALL").is_some_and(|v| v != "0") {
        SCAN_ALL.store(true, Ordering::Relaxed);
    }
}

/// Whether the size map measures the system folders too.
pub fn scan_all() -> bool {
    ensure_scan_all_init();
    SCAN_ALL.load(Ordering::Relaxed)
}

/// Change the scope. The caller owns triggering the rescan and remembering
/// the choice.
pub fn set_scan_all(on: bool) {
    SCAN_ALL_INIT.store(true, Ordering::Relaxed);
    SCAN_ALL.store(on, Ordering::Relaxed);
}

/// True for a folder the size map must not enter.
///
/// Only ever consulted for directories, and only for the ones directly under
/// the user's home — a `Library` folder inside a project is a project's
/// library and gets measured like anything else.
pub fn skip_for_scan(path: &Path, home: &Path) -> bool {
    tool_dir_exclusion(path) || (!scan_all() && home_scan_exclusion(path, home))
}

/// Tool state folders the size map never enters, at any depth and in both
/// scan scopes (the user: "in the directory viz tool please ignore the
/// .cargo/.claude/.grok directories"): registries, agent worktrees and
/// session state, not the user's own files.
pub const TOOL_SKIP: [&str; 3] = [".cargo", ".claude", ".grok"];

pub fn tool_dir_exclusion(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| TOOL_SKIP.contains(&name))
}

pub fn home_scan_exclusion(path: &Path, home: &Path) -> bool {
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
        return Some("including system folders · never .cargo, .claude, .grok".to_string());
    }
    Some("excluding Library and Trash · never .cargo, .claude, .grok".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_state_folders_are_skipped_at_any_depth_in_both_scopes() {
        let home = Path::new("/Users/me");
        for on in [false, true] {
            set_scan_all(on);
            assert!(skip_for_scan(Path::new("/Users/me/.cargo"), home));
            assert!(skip_for_scan(Path::new("/Users/me/makepad/.claude"), home));
            assert!(skip_for_scan(Path::new("/Users/me/makepad/deep/er/.grok"), home));
            assert!(!skip_for_scan(Path::new("/Users/me/makepad/cargo"), home));
            assert!(!skip_for_scan(Path::new("/Users/me/makepad/src"), home));
        }
        set_scan_all(false);
    }
    #[test]
    fn kind_for_matches_the_files_app_table() {
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
    fn the_map_leaves_apples_folders_alone_and_touches_nothing_else() {
        let home = Path::new("/active-home");
        assert!(home_scan_exclusion(&home.join("Library"), home));
        assert!(home_scan_exclusion(&home.join(".Trash"), home));
        assert!(!home_scan_exclusion(&home.join("Documents"), home));
        assert!(!home_scan_exclusion(&home.join("Pictures"), home));
        assert!(!home_scan_exclusion(&home.join("Downloads"), home));
        assert!(!home_scan_exclusion(&home.join("code/thing/Library"), home));
        assert!(!home_scan_exclusion(Path::new("/tmp/Library"), home));
        assert!(scan_exclusions().is_some());
    }
}
