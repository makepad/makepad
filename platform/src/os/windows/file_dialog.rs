//! Native file and folder dialogs: `IFileOpenDialog` and `IFileSaveDialog`.
//!
//! Mirrors the macOS contract exactly: the dialog is
//! NOT run inline in the platform-op drain — a modal pumps messages, and
//! the drain holds the `Cx` borrow — so it runs on its own STA thread and
//! the answer comes back as a [`FileDialogAction`] through
//! [`Cx::post_action`], long after the call that asked has returned.
use {
    crate::{
        cx::Cx,
        file_dialogs::{FileDialog, FileDialogAction},
    },
    std::path::PathBuf,
    windows::{
        core::{HRESULT, PCWSTR},
        Win32::{
            System::Com::{
                CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
            },
            UI::Shell::{
                Common::COMDLG_FILTERSPEC, FileOpenDialog, FileSaveDialog, IFileOpenDialog,
                IFileSaveDialog, IShellItem, FOS_ALLOWMULTISELECT, FOS_FILEMUSTEXIST,
                FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT, FOS_PICKFOLDERS, SIGDN_FILESYSPATH,
            },
        },
    },
};

/// `HRESULT_FROM_WIN32(ERROR_CANCELLED)` — the user closed the dialog.
const HR_CANCELLED: i32 = 0x8007_04C7_u32 as i32;

/// Opens the native file picker on its own STA thread and posts the answer
/// as a [`FileDialogAction`].
pub fn open_select_file_dialog(settings: FileDialog) {
    std::thread::Builder::new()
        .name("file-dialog".into())
        .spawn(move || {
            let id = settings.id;
            let picked = unsafe { with_com(|| run_open_dialog(&settings, false)) };
            Cx::post_action(if picked.is_empty() {
                FileDialogAction::FileCancelled { id }
            } else {
                FileDialogAction::FileSelected { id, paths: picked }
            });
        })
        .ok();
}

/// Opens the native save panel on its own STA thread. The dialog itself
/// runs the "already exists, replace?" prompt.
pub fn open_save_file_dialog(settings: FileDialog) {
    std::thread::Builder::new()
        .name("save-dialog".into())
        .spawn(move || {
            let id = settings.id;
            let picked = unsafe { with_com(|| run_save_dialog(&settings)) };
            Cx::post_action(match picked {
                Some(path) => FileDialogAction::SaveFileSelected { id, path },
                None => FileDialogAction::SaveFileCancelled { id },
            });
        })
        .ok();
}

/// "Save into this folder": the folder picker with directory creation on.
pub fn open_save_folder_dialog(settings: FileDialog) {
    std::thread::Builder::new()
        .name("folder-dialog".into())
        .spawn(move || {
            let picked = unsafe { with_com(|| run_open_dialog(&settings, true)) };
            Cx::post_action(match picked.into_iter().next() {
                Some(path) => FileDialogAction::FolderSelected(path),
                None => FileDialogAction::FolderCancelled,
            });
        })
        .ok();
}

/// Opens the native folder picker on its own STA thread and posts the
/// answer as a [`FileDialogAction`].
pub fn open_select_folder_dialog(settings: FileDialog) {
    open_save_folder_dialog(settings);
}

/// Wrap a dialog run in this thread's COM apartment.
unsafe fn with_com<T>(body: impl FnOnce() -> T) -> T {
    let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
    let result = body();
    // S_OK / S_FALSE both mean this thread owes a matching uninitialize.
    if initialized {
        CoUninitialize();
    }
    result
}

/// Wide, NUL-terminated. The returned buffer must outlive the COM call
/// that borrows its pointer.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// Build the file-type dropdown rows. Returns the specs plus the string
/// buffers they point into — drop those and the pointers dangle.
fn build_filters(settings: &FileDialog) -> (Vec<COMDLG_FILTERSPEC>, Vec<Vec<u16>>) {
    let mut specs = Vec::new();
    let mut storage: Vec<Vec<u16>> = Vec::new();
    for filter in &settings.filters {
        // Windows wants "*.mp4;*.mkv" in one string.
        let pattern = filter
            .extensions
            .iter()
            .map(|e| {
                let cleaned = e.trim().trim_start_matches('*').trim_start_matches('.');
                if cleaned.is_empty() || e.trim() == "*" {
                    "*.*".to_string()
                } else {
                    format!("*.{cleaned}")
                }
            })
            .collect::<Vec<_>>()
            .join(";");
        storage.push(wide(&filter.description));
        storage.push(wide(&pattern));
        let name = storage[storage.len() - 2].as_ptr();
        let spec = storage[storage.len() - 1].as_ptr();
        specs.push(COMDLG_FILTERSPEC {
            pszName: PCWSTR(name),
            pszSpec: PCWSTR(spec),
        });
    }
    (specs, storage)
}

/// Read one `IShellItem`'s filesystem path; the item is released on drop.
unsafe fn shell_item_path(item: IShellItem) -> Option<PathBuf> {
    let raw = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
    if raw.0.is_null() {
        return None;
    }
    let mut len = 0;
    while *raw.0.add(len) != 0 {
        len += 1;
    }
    let text = String::from_utf16_lossy(std::slice::from_raw_parts(raw.0, len));
    CoTaskMemFree(Some(raw.0 as *const _));
    Some(PathBuf::from(text))
}

/// One `IFileOpenDialog` run. `folders` picks directories instead of files;
/// multi-select comes from the dialog settings.
unsafe fn run_open_dialog(settings: &FileDialog, folders: bool) -> Vec<PathBuf> {
    let result = (|| -> Result<Vec<PathBuf>, HRESULT> {
        let dialog = IFileOpenDialog::from_raw(CoCreateInstance(
            &FileOpenDialog,
            None,
            CLSCTX_INPROC_SERVER,
            &IFileOpenDialog::IID,
        )?)?;
        let mut options = dialog.GetOptions()? | FOS_FORCEFILESYSTEM;
        if folders {
            options = options | FOS_PICKFOLDERS;
        } else {
            options = options | FOS_FILEMUSTEXIST;
            if settings.multiple {
                options = options | FOS_ALLOWMULTISELECT;
            }
        }
        dialog.SetOptions(options)?;
        if let Some(title) = &settings.title {
            dialog.SetTitle(PCWSTR(wide(title).as_ptr()))?;
        }
        // Held until Show returns: the dialog borrows these pointers.
        let (specs, _storage) = build_filters(settings);
        if !folders && !specs.is_empty() {
            dialog.SetFileTypes(&specs)?;
        }
        dialog.Show(None)?;
        let mut picked = Vec::new();
        if !folders && settings.multiple {
            let items = dialog.GetResults()?;
            for index in 0..items.GetCount()? {
                if let Some(path) = shell_item_path(items.GetItemAt(index)?) {
                    picked.push(path);
                }
            }
        } else if let Some(path) = shell_item_path(dialog.GetResult()?) {
            picked.push(path);
        }
        Ok(picked)
    })();
    match result {
        Ok(paths) => paths,
        Err(error) => {
            if error.0 != HR_CANCELLED {
                crate::error!("file dialog failed: {error:?}");
            }
            Vec::new()
        }
    }
}

/// One `IFileSaveDialog` run.
unsafe fn run_save_dialog(settings: &FileDialog) -> Option<PathBuf> {
    let result = (|| -> Result<Option<PathBuf>, HRESULT> {
        let dialog = IFileSaveDialog::from_raw(CoCreateInstance(
            &FileSaveDialog,
            None,
            CLSCTX_INPROC_SERVER,
            &IFileSaveDialog::IID,
        )?)?;
        dialog.SetOptions(dialog.GetOptions()? | FOS_FORCEFILESYSTEM | FOS_OVERWRITEPROMPT)?;
        if let Some(title) = &settings.title {
            dialog.SetTitle(PCWSTR(wide(title).as_ptr()))?;
        }
        if let Some(filename) = &settings.filename {
            dialog.SetFileName(PCWSTR(wide(filename).as_ptr()))?;
        }
        let (specs, _storage) = build_filters(settings);
        if !specs.is_empty() {
            dialog.SetFileTypes(&specs)?;
            // Append the chosen type's extension when the user typed none.
            if let Some(first) = settings.filters.first().and_then(|f| f.extensions.first()) {
                let cleaned = first.trim().trim_start_matches('*').trim_start_matches('.');
                if !cleaned.is_empty() && cleaned != "*" {
                    dialog.SetDefaultExtension(PCWSTR(wide(cleaned).as_ptr()))?;
                }
            }
        }
        dialog.Show(None)?;
        Ok(shell_item_path(dialog.GetResult()?))
    })();
    match result {
        Ok(path) => path,
        Err(error) => {
            if error.0 != HR_CANCELLED {
                crate::error!("save dialog failed: {error:?}");
            }
            None
        }
    }
}
