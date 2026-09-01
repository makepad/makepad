//! Native file and folder dialogs.
//!
//! One shape for every OS: an app fills in a [`FileDialog`], hands it to
//! `Cx`, and the answer arrives later as a [`FileDialogAction`] — because
//! a modal is answered by a person, long after the call that opened it
//! returned. Every dialog carries an [`id`](FileDialog::id) which comes
//! back on the action, so an app with an "import statement" picker and an
//! "attach receipt" picker can tell the two answers apart.

use crate::makepad_live_id::LiveId;
use std::path::PathBuf;

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
}

impl Default for FileDialog {
    fn default() -> Self {
        Self::new()
    }
}

/// The outcome of a native file/folder dialog, delivered back to the app as a
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

impl FileDialogAction {
    /// The dialog this answers, for apps that route by id.
    pub fn id(&self) -> LiveId {
        match self {
            FileDialogAction::FileSelected { id, .. }
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
