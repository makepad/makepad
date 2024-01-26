// mildly stripped down version of native_dialog_rs dialog interface.
use std::path::{PathBuf};


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
}

impl FileDialog {
    /// Creates a file dialog builder.
    pub fn new() -> Self {
        FileDialog {
            filename: None,
            location: None,
            filters: vec![],           
            title: None,
        }
    }

    /// Sets the window title for the dialog.
    pub fn set_title(mut self, title: String) -> Self {
        self.title = Some(title);
        self
    }

    /// Sets the default value of the filename text field in the dialog. For open dialogs of macOS
    /// and zenity, this is a no-op because there's no such text field on the dialog.
    pub fn set_filename(mut self, filename:  String) -> Self {
        self.filename = Some(filename);
        self
    }

    /// Resets the default value of the filename field in the dialog.
    pub fn reset_filename(mut self) -> Self {
        self.filename = None;
        self
    }

    /// Sets the default location that the dialog shows at open.
    pub fn set_location(mut self, path:  PathBuf) -> Self {
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
    pub fn add_filter(mut self, description: String, extensions:  Vec<String>) -> Self {
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



}



impl Default for FileDialog {
    fn default() -> Self {
        Self::new()
    }
}

