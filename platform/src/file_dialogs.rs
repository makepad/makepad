// mildly stripped down version of native_dialog_rs dialog interface.
use std::path::PathBuf;

/// Represents a set of file extensions and their description.
#[derive(Clone, Debug, PartialEq)]
pub struct Filter {
    pub description: String,
    pub extensions: Vec<String>,
}

/// Which system dialog was requested / produced a result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FileDialogKind {
    #[default]
    OpenFile,
    SaveFile,
    OpenFolder,
    SaveFolder,
}

/// Builds and shows file dialogs.

#[derive(Clone, Debug, PartialEq)]
pub struct FileDialog {
    pub filename: Option<String>,
    pub location: Option<PathBuf>,
    pub filters: Vec<Filter>,
    pub title: Option<String>,
    /// Assigned by [`Cx`](crate::Cx) when the dialog is enqueued; echoed in the result.
    pub request_id: u64,
    /// Assigned by [`Cx`](crate::Cx) when the dialog is enqueued; echoed in the result.
    pub kind: FileDialogKind,
}

impl FileDialog {
    /// Creates a file dialog builder.
    pub fn new() -> Self {
        FileDialog {
            filename: None,
            location: None,
            filters: vec![],
            title: None,
            request_id: 0,
            kind: FileDialogKind::OpenFile,
        }
    }

    /// Minimal settings used when correlating an async platform result.
    pub fn from_meta(request_id: u64, kind: FileDialogKind) -> Self {
        Self {
            request_id,
            kind,
            ..Self::new()
        }
    }

    /// Sets the window title for the dialog.
    pub fn set_title(mut self, title: String) -> Self {
        self.title = Some(title);
        self
    }

    /// Sets the default value of the filename text field in the dialog. For open dialogs of macOS
    /// and zenity, this is a no-op because there's no such text field on the dialog.
    pub fn set_filename(mut self, filename: String) -> Self {
        self.filename = Some(filename);
        self
    }

    /// Resets the default value of the filename field in the dialog.
    pub fn reset_filename(mut self) -> Self {
        self.filename = None;
        self
    }

    /// Sets the default location that the dialog shows at open.
    pub fn set_location(mut self, path: PathBuf) -> Self {
        self.location = Some(path);
        self
    }

    /// Resets the default location that the dialog shows at open. Without a default location set,
    /// the dialog will probably use the current working directory as default location.
    pub fn reset_location(mut self) -> Self {
        self.location = None;
        self
    }

    /// Adds a file type filter. Empty extension lists are ignored (no panic).
    pub fn add_filter(mut self, description: String, extensions: Vec<String>) -> Self {
        if extensions.is_empty() {
            return self;
        }
        self.filters.push(Filter {
            description,
            extensions,
        });
        self
    }

    /// Removes all file type filters.
    pub fn remove_all_filters(mut self) -> Self {
        self.filters = vec![];
        self
    }
}

impl Default for FileDialog {
    fn default() -> Self {
        Self::new()
    }
}

/// Outcome of a system open/save/folder dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FileDialogStatus {
    /// User confirmed a selection. See [`FileDialogResultEvent::paths`].
    Ok,
    /// User dismissed the dialog without selecting.
    #[default]
    Cancelled,
    /// This dialog kind is not implemented on the current platform / browser.
    Unsupported,
    /// The platform attempted the dialog but failed (permissions, missing UI, I/O, …).
    Error,
}

/// How [`FileDialogResultEvent::paths`] should be interpreted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FileDialogPathKind {
    /// Native filesystem path usable with `std::fs` (desktop + iOS import copies).
    #[default]
    Filesystem,
    /// Android SAF / OpenHarmony document URI. Needs platform content APIs, not `std::fs`.
    ContentUri,
    /// Web open: [`paths`](FileDialogResultEvent::paths) are display names and
    /// [`contents`](FileDialogResultEvent::contents) holds the file bytes (parallel arrays).
    Inline,
}

/// Result of a system open/save file dialog.
#[derive(Clone, Debug, Default)]
pub struct FileDialogResultEvent {
    /// Selected paths / URIs / display names. Empty unless [`status`](Self::status) is [`FileDialogStatus::Ok`].
    pub paths: Vec<String>,
    pub status: FileDialogStatus,
    pub path_kind: FileDialogPathKind,
    /// File bytes when `path_kind` is [`FileDialogPathKind::Inline`] (Web). Otherwise empty.
    pub contents: Vec<Vec<u8>>,
    /// Matches the `request_id` on the originating [`FileDialog`].
    pub request_id: u64,
    pub kind: FileDialogKind,
    /// Optional detail for [`FileDialogStatus::Error`] / [`FileDialogStatus::Unsupported`].
    pub message: Option<String>,
}

impl FileDialogResultEvent {
    fn base(settings: &FileDialog) -> Self {
        Self {
            paths: Vec::new(),
            status: FileDialogStatus::Cancelled,
            path_kind: FileDialogPathKind::Filesystem,
            contents: Vec::new(),
            request_id: settings.request_id,
            kind: settings.kind,
            message: None,
        }
    }

    pub fn ok(settings: &FileDialog, paths: Vec<String>, path_kind: FileDialogPathKind) -> Self {
        Self {
            paths,
            status: FileDialogStatus::Ok,
            path_kind,
            contents: Vec::new(),
            request_id: settings.request_id,
            kind: settings.kind,
            message: None,
        }
    }

    pub fn ok_filesystem(settings: &FileDialog, paths: Vec<String>) -> Self {
        Self::ok(settings, paths, FileDialogPathKind::Filesystem)
    }

    pub fn ok_content_uris(settings: &FileDialog, paths: Vec<String>) -> Self {
        Self::ok(settings, paths, FileDialogPathKind::ContentUri)
    }

    /// Web-style result: display names plus inlined file bytes.
    pub fn ok_inline(settings: &FileDialog, paths: Vec<String>, contents: Vec<Vec<u8>>) -> Self {
        Self {
            paths,
            status: FileDialogStatus::Ok,
            path_kind: FileDialogPathKind::Inline,
            contents,
            request_id: settings.request_id,
            kind: settings.kind,
            message: None,
        }
    }

    pub fn cancelled() -> Self {
        Self::default()
    }

    pub fn cancelled_from(settings: &FileDialog) -> Self {
        Self::base(settings)
    }

    /// Emits an error log and returns an unsupported result (not a user cancel).
    pub fn unsupported(op: &str) -> Self {
        crate::error!("File dialog not implemented on this platform: {op}");
        Self {
            paths: Vec::new(),
            status: FileDialogStatus::Unsupported,
            path_kind: FileDialogPathKind::Filesystem,
            contents: Vec::new(),
            request_id: 0,
            kind: FileDialogKind::OpenFile,
            message: Some(op.to_string()),
        }
    }

    pub fn unsupported_from(settings: &FileDialog, message: impl Into<String>) -> Self {
        let message = message.into();
        crate::error!(
            "File dialog unsupported (id={}, {:?}): {message}",
            settings.request_id,
            settings.kind
        );
        Self {
            message: Some(message),
            status: FileDialogStatus::Unsupported,
            ..Self::base(settings)
        }
    }

    pub fn error_from(settings: &FileDialog, message: impl Into<String>) -> Self {
        let message = message.into();
        crate::error!(
            "File dialog error (id={}, {:?}): {message}",
            settings.request_id,
            settings.kind
        );
        Self {
            message: Some(message),
            status: FileDialogStatus::Error,
            ..Self::base(settings)
        }
    }

    /// Maps a modal dialog return value (`None` = user cancelled).
    pub fn from_option(
        settings: &FileDialog,
        paths: Option<Vec<String>>,
        path_kind: FileDialogPathKind,
    ) -> Self {
        match paths {
            Some(paths) => Self::ok(settings, paths, path_kind),
            None => Self::cancelled_from(settings),
        }
    }

    pub fn with_meta(mut self, request_id: u64, kind: FileDialogKind) -> Self {
        self.request_id = request_id;
        self.kind = kind;
        self
    }

    pub fn is_ok(&self) -> bool {
        self.status == FileDialogStatus::Ok
    }

    pub fn is_cancelled(&self) -> bool {
        self.status == FileDialogStatus::Cancelled
    }

    pub fn is_unsupported(&self) -> bool {
        self.status == FileDialogStatus::Unsupported
    }

    pub fn is_error(&self) -> bool {
        self.status == FileDialogStatus::Error
    }

    /// Read bytes for a successful selection.
    ///
    /// - [`FileDialogPathKind::Inline`]: returns inlined `contents`
    /// - [`FileDialogPathKind::Filesystem`]: `std::fs::read`
    /// - [`FileDialogPathKind::ContentUri`]: returns [`FileDialogIoError::NeedsPlatformApi`]
    pub fn read_bytes(&self, index: usize) -> Result<Vec<u8>, FileDialogIoError> {
        if !self.is_ok() {
            return Err(FileDialogIoError::NotOk);
        }
        match self.path_kind {
            FileDialogPathKind::Inline => self
                .contents
                .get(index)
                .cloned()
                .ok_or(FileDialogIoError::IndexOutOfRange),
            FileDialogPathKind::Filesystem => {
                let path = self
                    .paths
                    .get(index)
                    .ok_or(FileDialogIoError::IndexOutOfRange)?;
                std::fs::read(path).map_err(FileDialogIoError::Io)
            }
            FileDialogPathKind::ContentUri => Err(FileDialogIoError::NeedsPlatformApi),
        }
    }
}

/// Errors from [`FileDialogResultEvent::read_bytes`].
#[derive(Debug)]
pub enum FileDialogIoError {
    NotOk,
    IndexOutOfRange,
    /// Android / OpenHarmony URIs need a platform content reader (not yet unified).
    NeedsPlatformApi,
    Io(std::io::Error),
}

impl std::fmt::Display for FileDialogIoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotOk => write!(f, "file dialog result is not Ok"),
            Self::IndexOutOfRange => write!(f, "file dialog index out of range"),
            Self::NeedsPlatformApi => write!(
                f,
                "content URI requires a platform-specific reader (not std::fs)"
            ),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for FileDialogIoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

/// Best-effort MIME type for Android `Intent` filters from dialog extensions.
pub fn mime_type_for_filters(filters: &[Filter]) -> &'static str {
    let mimes = mime_types_for_filters(filters);
    mimes.first().copied().unwrap_or("*/*")
}

/// One or more MIME types for Android `Intent.EXTRA_MIME_TYPES`.
pub fn mime_types_for_filters(filters: &[Filter]) -> Vec<&'static str> {
    if filters.is_empty() {
        return vec!["*/*"];
    }
    let mut out: Vec<&'static str> = Vec::new();
    for filter in filters {
        for ext in &filter.extensions {
            let e = ext.trim_start_matches('.').to_ascii_lowercase();
            let mime = match e.as_str() {
                "mp4" | "m4v" => "video/mp4",
                "mkv" => "video/x-matroska",
                "webm" => "video/webm",
                "mov" => "video/quicktime",
                "3gp" => "video/3gpp",
                "avi" => "video/x-msvideo",
                "flv" | "ts" | "wmv" | "mpg" | "mpeg" => "video/*",
                "jpg" | "jpeg" => "image/jpeg",
                "png" => "image/png",
                "gif" => "image/gif",
                "webp" => "image/webp",
                "bmp" => "image/bmp",
                "heic" | "heif" => "image/*",
                "mp3" => "audio/mpeg",
                "wav" => "audio/wav",
                "ogg" => "audio/ogg",
                "m4a" => "audio/mp4",
                "aac" => "audio/aac",
                "flac" => "audio/flac",
                "opus" => "audio/opus",
                "pdf" => "application/pdf",
                "txt" => "text/plain",
                "json" => "application/json",
                "html" | "htm" => "text/html",
                "zip" => "application/zip",
                _ => continue,
            };
            if !out.contains(&mime) {
                out.push(mime);
            }
        }
    }
    if out.is_empty() {
        vec!["*/*"]
    } else {
        out
    }
}

/// HTML `<input accept>` / File System Access filter string from dialog extensions.
pub fn accept_for_filters(filters: &[Filter]) -> String {
    if filters.is_empty() {
        return String::new();
    }
    let mut parts = Vec::new();
    for filter in filters {
        for ext in &filter.extensions {
            let trimmed = ext.trim_start_matches('.');
            if trimmed.is_empty() {
                continue;
            }
            let part = format!(".{trimmed}");
            if !parts.iter().any(|p| p == &part) {
                parts.push(part);
            }
        }
    }
    parts.join(",")
}
