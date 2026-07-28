//! Synchronous Win32 open-file dialog (`GetOpenFileNameW`) for `CxOsOp::SelectFileDialog`.

use crate::{
    file_dialogs::FileDialog,
    windows::{
        core::{PCWSTR, PWSTR},
        Win32::UI::Controls::Dialogs::{
            GetOpenFileNameW, OPENFILENAMEW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_HIDEREADONLY,
            OFN_PATHMUSTEXIST,
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

/// Shows a modal open-file dialog. Returns `None` if the user cancelled.
pub fn open_select_file_dialog(settings: &FileDialog) -> Option<Vec<String>> {
    let filter = build_filter(settings);
    let title = settings
        .title
        .as_deref()
        .map(to_wide_null)
        .unwrap_or_else(|| to_wide_null("Open File"));
    let initial_dir = settings
        .location
        .as_ref()
        .map(|p| to_wide_null(&p.to_string_lossy()))
        .unwrap_or_default();
    let mut file_buf = vec![0u16; 32768];
    if let Some(name) = &settings.filename {
        let wide: Vec<u16> = name.encode_utf16().collect();
        let n = wide.len().min(file_buf.len() - 1);
        file_buf[..n].copy_from_slice(&wide[..n]);
    }

    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
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
        Flags: OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_HIDEREADONLY,
        ..Default::default()
    };

    let ok = unsafe { GetOpenFileNameW(&mut ofn) };
    if !ok.as_bool() {
        return None;
    }
    let end = file_buf.iter().position(|&c| c == 0).unwrap_or(file_buf.len());
    let path = String::from_utf16_lossy(&file_buf[..end]);
    if path.is_empty() {
        return None;
    }
    // Normalize to a PathBuf then back so separators are consistent.
    let path = PathBuf::from(path).to_string_lossy().into_owned();
    Some(vec![path])
}
