//! Cross-platform file and folder dialogs.
//!
//! One shape for every OS: an app fills in a [`FileDialog`], hands it to
//! `Cx`, and the answer arrives later as a [`FileDialogAction`] — because
//! a modal is answered by a person, long after the call that opened it
//! returned. Every dialog carries an [`id`](FileDialog::id) which comes
//! back on the action, so an app with an "import statement" picker and an
//! "attach receipt" picker can tell the two answers apart.

use crate::makepad_live_id::LiveId;
use std::{path::PathBuf, sync::Arc};

pub const DEFAULT_VIRTUAL_FILE_SIZE_LIMIT: u64 = 512 * 1024 * 1024;

/// Bytes accepted from a file picker or browser drop before they enter the
/// application. Both limits default to 512 MiB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirtualFileLimits {
    pub max_file_size: u64,
    pub max_total_size: u64,
}

impl Default for VirtualFileLimits {
    fn default() -> Self {
        Self {
            max_file_size: DEFAULT_VIRTUAL_FILE_SIZE_LIMIT,
            max_total_size: DEFAULT_VIRTUAL_FILE_SIZE_LIMIT,
        }
    }
}

/// A user-selected file whose contents are already resident in the app.
/// Browser files never leave the page unless application code sends them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VirtualFile {
    pub name: String,
    pub mime: String,
    pub bytes: Arc<[u8]>,
    pub size: u64,
}

#[derive(Debug)]
pub(crate) struct VirtualFileData {
    pub name: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

pub(crate) fn assemble_virtual_files(
    files: Vec<VirtualFileData>,
    limits: VirtualFileLimits,
) -> Result<Vec<VirtualFile>, String> {
    let mut total = 0u64;
    let mut out = Vec::with_capacity(files.len());
    for file in files {
        let size = u64::try_from(file.bytes.len()).unwrap_or(u64::MAX);
        if size > limits.max_file_size {
            return Err(format!(
                "file '{}' is {} bytes, exceeding the per-file limit of {} bytes",
                file.name, size, limits.max_file_size
            ));
        }
        total = total
            .checked_add(size)
            .ok_or_else(|| "combined file size overflowed u64".to_string())?;
        if total > limits.max_total_size {
            return Err(format!(
                "selected files total {} bytes, exceeding the per-drop limit of {} bytes",
                total, limits.max_total_size
            ));
        }
        out.push(VirtualFile {
            name: file.name,
            mime: file.mime,
            bytes: Arc::from(file.bytes),
            size,
        });
    }
    Ok(out)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn load_virtual_files(
    paths: Vec<PathBuf>,
    limits: VirtualFileLimits,
) -> Result<Vec<VirtualFile>, String> {
    let mut total = 0u64;
    let mut data = Vec::with_capacity(paths.len());
    for path in paths {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let size = std::fs::metadata(&path)
            .map_err(|error| format!("cannot inspect '{}': {error}", path.display()))?
            .len();
        if size > limits.max_file_size {
            return Err(format!(
                "file '{}' is {} bytes, exceeding the per-file limit of {} bytes",
                name, size, limits.max_file_size
            ));
        }
        total = total
            .checked_add(size)
            .ok_or_else(|| "combined file size overflowed u64".to_string())?;
        if total > limits.max_total_size {
            return Err(format!(
                "selected files total {} bytes, exceeding the per-drop limit of {} bytes",
                total, limits.max_total_size
            ));
        }
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("cannot read '{}': {error}", path.display()))?;
        data.push(VirtualFileData {
            mime: mime_from_path(&path).to_string(),
            name,
            bytes,
        });
    }
    assemble_virtual_files(data, limits)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn load_virtual_files_action(
    id: LiveId,
    paths: Vec<PathBuf>,
    limits: VirtualFileLimits,
) -> FileDialogAction {
    match load_virtual_files(paths, limits) {
        Ok(files) => FileDialogAction::FileLoaded { id, files },
        Err(error) => {
            crate::error!("file dialog byte load failed: {error}");
            FileDialogAction::FileCancelled { id }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn mime_from_path(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "txt" | "log" | "rs" | "toml" | "yaml" | "yml" | "ini" | "cfg" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "zip" => "application/zip",
        _ => "",
    }
}

/// Represents a set of file extensions and their description.
#[derive(Debug, PartialEq)]
pub struct Filter {
    pub description: String,
    pub extensions: Vec<String>,
}

/// Builds and shows file dialogs.

#[derive(Debug, PartialEq)]
pub struct FileDialog {
    pub filename: Option<String>,
    pub location: Option<PathBuf>,
    pub filters: Vec<Filter>,
    pub title: Option<String>,
    /// Let the user choose more than one file. Open dialogs only.
    pub multiple: bool,
    /// Echoed back on the action, so an app can tell which of its dialogs
    /// answered. `LiveId(0)` when the app never set one.
    pub id: LiveId,
    /// Load selected files into [`VirtualFile`] values on a worker instead
    /// of returning filesystem paths. Web always behaves as if this is true.
    pub want_bytes: bool,
}

impl FileDialog {
    /// Creates a file dialog builder.
    pub fn new() -> Self {
        FileDialog {
            filename: None,
            location: None,
            filters: vec![],
            title: None,
            multiple: false,
            id: LiveId(0),
            want_bytes: false,
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

    /// Adds a file type filter. The filter must contains at least one extension, otherwise this
    /// method will panic. For dialogs that open directories, this is a no-op.
    pub fn add_filter(mut self, description: String, extensions: Vec<String>) -> Self {
        if extensions.is_empty() {
            panic!("The file extensions of a filter must be specified.")
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

    /// Allow selecting several files at once (open dialogs only).
    pub fn set_multiple(mut self, multiple: bool) -> Self {
        self.multiple = multiple;
        self
    }

    /// Tag this dialog, so its answer can be told from another's.
    pub fn set_id(mut self, id: LiveId) -> Self {
        self.id = id;
        self
    }

    /// Choose whether an open dialog returns file contents instead of paths.
    pub fn want_bytes(mut self, want_bytes: bool) -> Self {
        self.want_bytes = want_bytes;
        self
    }
}

impl Default for FileDialog {
    fn default() -> Self {
        Self::new()
    }
}

/// The outcome of a file/folder dialog, delivered back to the app as a
/// plain [`crate::action::Action`] (via `Cx::post_action`) on the next actions
/// pass — a dialog is answered by the user long after the call that opened it,
/// so there is no return value to wait on.
///
/// Cancelling is a first-class outcome, not an error: a UI that armed an
/// import needs to disarm it again.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum FileDialogAction {
    /// The user chose these files. One path unless the dialog asked for
    /// [`FileDialog::multiple`]; never empty.
    FileSelected { id: LiveId, paths: Vec<PathBuf> },
    /// The selected files, loaded away from the UI thread. This is the only
    /// successful file-open result on web.
    FileLoaded { id: LiveId, files: Vec<VirtualFile> },
    /// The user dismissed a file-open dialog without choosing.
    FileCancelled { id: LiveId },
    /// The user named this file to save to. The file may or may not exist
    /// — the OS has already asked about overwriting.
    SaveFileSelected { id: LiveId, path: PathBuf },
    SaveFileCancelled { id: LiveId },
    /// The user chose this folder in a folder-select dialog.
    ///
    /// The folder variants predate the id and keep their shape: an app has
    /// one folder picker, and three shipped apps match on them.
    FolderSelected(PathBuf),
    /// The user dismissed a folder-select dialog without choosing.
    FolderCancelled,
    #[default]
    None,
}

/// Marks a worker completion so it cannot be mistaken for a fresh native
/// dialog result when applications reuse the default dialog id.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Debug)]
pub(crate) struct FileDialogLoadAction(pub FileDialogAction);

impl FileDialogAction {
    /// The dialog this answers, for apps that route by id.
    pub fn id(&self) -> LiveId {
        match self {
            FileDialogAction::FileSelected { id, .. }
            | FileDialogAction::FileLoaded { id, .. }
            | FileDialogAction::FileCancelled { id }
            | FileDialogAction::SaveFileSelected { id, .. }
            | FileDialogAction::SaveFileCancelled { id } => *id,
            _ => LiveId(0),
        }
    }

    /// The single chosen path, for the common one-file case.
    pub fn path(&self) -> Option<&PathBuf> {
        match self {
            FileDialogAction::FileSelected { paths, .. } => paths.first(),
            FileDialogAction::SaveFileSelected { path, .. } => Some(path),
            FileDialogAction::FolderSelected(path) => Some(path),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PendingFileDialog {
    pub id: LiveId,
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub want_bytes: bool,
    pub limits: VirtualFileLimits,
}

#[derive(Default)]
pub(crate) struct FileDialogState {
    pending: Vec<PendingFileDialog>,
    limits: VirtualFileLimits,
}

impl FileDialogState {
    pub fn limits(&self) -> VirtualFileLimits {
        self.limits
    }

    pub fn set_limits(&mut self, limits: VirtualFileLimits) {
        self.limits = limits;
    }

    pub fn begin(&mut self, dialog: &FileDialog) {
        self.pending.push(PendingFileDialog {
            id: dialog.id,
            want_bytes: dialog.want_bytes,
            limits: self.limits,
        });
    }

    pub fn finish(&mut self, id: LiveId) -> Option<PendingFileDialog> {
        let index = self.pending.iter().position(|pending| pending.id == id)?;
        Some(self.pending.remove(index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(name: &str, bytes: &[u8]) -> VirtualFileData {
        VirtualFileData {
            name: name.to_string(),
            mime: "application/octet-stream".to_string(),
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn pending_dialog_ids_are_finished_in_request_order() {
        let mut state = FileDialogState::default();
        state.begin(&FileDialog::new().set_id(LiveId(7)).want_bytes(true));
        state.begin(&FileDialog::new().set_id(LiveId(9)));
        state.begin(&FileDialog::new().set_id(LiveId(7)));

        assert!(state.finish(LiveId(9)).is_some());
        assert!(state.finish(LiveId(7)).unwrap().want_bytes);
        assert!(!state.finish(LiveId(7)).unwrap().want_bytes);
        assert!(state.finish(LiveId(7)).is_none());
    }

    #[test]
    fn assembles_multiple_files_in_order() {
        let files = assemble_virtual_files(
            vec![data("one", &[1, 2]), data("two", &[3, 4, 5])],
            VirtualFileLimits::default(),
        )
        .unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].name, "one");
        assert_eq!(&*files[1].bytes, &[3, 4, 5]);
        assert_eq!(files[1].size, 3);
    }

    #[test]
    fn enforces_per_file_and_combined_caps() {
        let per_file = VirtualFileLimits {
            max_file_size: 2,
            max_total_size: 10,
        };
        assert!(assemble_virtual_files(vec![data("large", &[0; 3])], per_file)
            .unwrap_err()
            .contains("per-file limit"));

        let combined = VirtualFileLimits {
            max_file_size: 10,
            max_total_size: 4,
        };
        assert!(assemble_virtual_files(
            vec![data("one", &[0; 2]), data("two", &[0; 3])],
            combined,
        )
        .unwrap_err()
        .contains("per-drop limit"));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    // Native dialog test uses wall time only to make its temporary path unique.
    #[allow(clippy::disallowed_types, clippy::disallowed_methods)]
    fn native_file_loading_runs_on_a_worker_and_preserves_bytes() {
        use std::{
            sync::mpsc::channel,
            thread,
            time::{SystemTime, UNIX_EPOCH},
        };

        let ui_thread = thread::current().id();
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("makepad-virtual-file-{unique}.txt"));
        std::fs::write(&path, b"local bytes").unwrap();
        let dialog = FileDialog::new().set_id(LiveId(11)).want_bytes(true);
        let mut state = FileDialogState::default();
        state.begin(&dialog);
        let pending = state.finish(dialog.id).unwrap();
        assert!(pending.want_bytes);
        let (send, recv) = channel();
        let worker_path = path.clone();
        thread::spawn(move || {
            let action = load_virtual_files_action(
                pending.id,
                vec![worker_path],
                pending.limits,
            );
            send.send((thread::current().id(), action)).unwrap();
        });

        let (worker_thread, action) = recv.recv().unwrap();
        std::fs::remove_file(path).unwrap();
        assert_ne!(ui_thread, worker_thread);
        let FileDialogAction::FileLoaded { files, .. } = action else {
            unreachable!()
        };
        assert_eq!(&*files[0].bytes, b"local bytes");
    }
}
