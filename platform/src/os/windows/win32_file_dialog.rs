//! Synchronous Win32 open/save file dialogs and folder picker for `CxOsOp::*FileDialog`.

use crate::{
    file_dialogs::FileDialog,
    windows::{
        core::{PCWSTR, PWSTR},
        Win32::{
            Foundation::HWND,
            System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL},
            UI::{
                Controls::Dialogs::{
                    GetOpenFileNameW, GetSaveFileNameW, OPENFILENAMEW, OPEN_FILENAME_FLAGS,
                    OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_HIDEREADONLY, OFN_NOCHANGEDIR,
                    OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST,
                },
                Shell::{
                    FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName,
                    FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS,
                    SIGDN_FILESYSPATH,
                },
            },
        },
    },
};
use std::path::PathBuf;

fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn build_filter(dialog: &FileDialog) -> Vec<u16> {
    let mut out: Vec<u16> = Vec::new();
    if dialog.filters.is_empty() {
        // "All Files\0*.*\0\0"
        out.extend("All Files".encode_utf16());
        out.push(0);
        out.extend("*.*".encode_utf16());
        out.push(0);
        out.push(0);
        return out;
    }
    for filter in &dialog.filters {
        out.extend(filter.description.encode_utf16());
        out.push(0);
        let pattern = filter
            .extensions
            .iter()
            .map(|ext| format!("*.{}", ext.trim_start_matches('.')))
            .collect::<Vec<_>>()
            .join(";");
        out.extend(pattern.encode_utf16());
        out.push(0);
    }
    out.push(0);
    out
}

fn default_extension(dialog: &FileDialog) -> Option<Vec<u16>> {
    let ext = dialog.filters.first()?.extensions.first()?;
    let trimmed = ext.trim_start_matches('.');
    if trimmed.is_empty() {
        None
    } else {
        Some(to_wide_null(trimmed))
    }
}

fn run_file_dialog(
    hwnd_owner: HWND,
    settings: &FileDialog,
    title_default: &str,
    flags: OPEN_FILENAME_FLAGS,
    save: bool,
) -> Option<Vec<String>> {
    let filter = build_filter(settings);
    let title = settings
        .title
        .as_deref()
        .map(to_wide_null)
        .unwrap_or_else(|| to_wide_null(title_default));
    let initial_dir = settings
        .location
        .as_ref()
        .map(|p| to_wide_null(&p.to_string_lossy()))
        .unwrap_or_default();
    let def_ext = default_extension(settings);
    let mut file_buf = vec![0u16; 32768];
    if let Some(name) = &settings.filename {
        let wide: Vec<u16> = name.encode_utf16().collect();
        let n = wide.len().min(file_buf.len() - 1);
        file_buf[..n].copy_from_slice(&wide[..n]);
    }

    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: hwnd_owner,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        nFilterIndex: 1,
        lpstrFile: PWSTR(file_buf.as_mut_ptr()),
        nMaxFile: file_buf.len() as u32,
        lpstrInitialDir: if initial_dir.is_empty() {
            PCWSTR::null()
        } else {
            PCWSTR(initial_dir.as_ptr())
        },
        lpstrTitle: PCWSTR(title.as_ptr()),
        lpstrDefExt: def_ext
            .as_ref()
            .map(|e| PCWSTR(e.as_ptr()))
            .unwrap_or_else(PCWSTR::null),
        Flags: flags,
        ..Default::default()
    };

    let ok = unsafe {
        if save {
            GetSaveFileNameW(&mut ofn)
        } else {
            GetOpenFileNameW(&mut ofn)
        }
    };
    if !ok.as_bool() {
        return None;
    }
    let end = file_buf
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(file_buf.len());
    let path = String::from_utf16_lossy(&file_buf[..end]);
    if path.is_empty() {
        return None;
    }
    // Normalize to a PathBuf then back so separators are consistent.
    let path = PathBuf::from(path).to_string_lossy().into_owned();
    Some(vec![path])
}

/// Shows a modal open-file dialog. Returns `None` if the user cancelled.
pub fn open_select_file_dialog(hwnd_owner: HWND, settings: &FileDialog) -> Option<Vec<String>> {
    run_file_dialog(
        hwnd_owner,
        settings,
        "Open File",
        OFN_EXPLORER
            | OFN_FILEMUSTEXIST
            | OFN_PATHMUSTEXIST
            | OFN_HIDEREADONLY
            | OFN_NOCHANGEDIR,
        false,
    )
}

/// Shows a modal save-file dialog. Returns `None` if the user cancelled.
pub fn open_save_file_dialog(hwnd_owner: HWND, settings: &FileDialog) -> Option<Vec<String>> {
    run_file_dialog(
        hwnd_owner,
        settings,
        "Save File",
        OFN_EXPLORER | OFN_PATHMUSTEXIST | OFN_OVERWRITEPROMPT | OFN_HIDEREADONLY | OFN_NOCHANGEDIR,
        true,
    )
}

/// Shows a modern folder picker (`IFileOpenDialog` + `FOS_PICKFOLDERS`).
///
/// Returns `None` if the user cancelled or the dialog could not be shown.
pub fn open_select_folder_dialog(hwnd_owner: HWND, settings: &FileDialog) -> Option<Vec<String>> {
    unsafe {
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_ALL).ok()?;

        let mut options = dialog.GetOptions().ok()?;
        options |= FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST | FOS_NOCHANGEDIR;
        dialog.SetOptions(options).ok()?;

        let title = settings
            .title
            .as_deref()
            .map(to_wide_null)
            .unwrap_or_else(|| to_wide_null("Select Folder"));
        let _ = dialog.SetTitle(PCWSTR(title.as_ptr()));

        if let Some(location) = &settings.location {
            let location_wide = to_wide_null(&location.to_string_lossy());
            if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(
                PCWSTR(location_wide.as_ptr()),
                None,
            ) {
                let _ = dialog.SetFolder(&item);
            }
        }

        // HRESULT_FROM_WIN32(ERROR_CANCELLED) — user dismissed the dialog.
        const HRESULT_CANCELLED: i32 = 0x800704C7u32 as i32;
        if let Err(err) = dialog.Show(Some(hwnd_owner)) {
            if err.code().0 == HRESULT_CANCELLED {
                return None;
            }
            crate::error!("IFileOpenDialog::Show failed: {err:?}");
            return None;
        }

        let item = dialog.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as *const _));
        let path = path?;
        if path.is_empty() {
            return None;
        }
        let path = PathBuf::from(path).to_string_lossy().into_owned();
        Some(vec![path])
    }
}

/// Windows has no distinct “save folder” UI; same picker as open-folder.
pub fn open_save_folder_dialog(hwnd_owner: HWND, settings: &FileDialog) -> Option<Vec<String>> {
    open_select_folder_dialog(hwnd_owner, settings)
}
