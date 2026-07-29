//! Linux desktop file dialogs via `zenity` (preferred) or `kdialog` fallback.
//! Used by X11 and Wayland backends; requires a desktop helper binary.

use crate::file_dialogs::FileDialog;
use std::io::ErrorKind;
use std::path::Path;
use std::process::Command;

#[derive(Clone, Copy)]
enum DialogKind {
    OpenFile,
    SaveFile,
    OpenFolder,
}

/// Why a Linux helper dialog could not produce a normal cancel/ok outcome.
#[derive(Debug)]
pub enum HelperError {
    /// Neither zenity nor kdialog is installed.
    Missing,
    /// A helper binary was found but failed to run.
    Failed(String),
}

fn initial_path(settings: &FileDialog) -> Option<String> {
    match (&settings.location, &settings.filename) {
        (Some(dir), Some(name)) => Some(dir.join(name).to_string_lossy().into_owned()),
        (Some(dir), None) => {
            let mut p = dir.to_string_lossy().into_owned();
            if !p.ends_with('/') {
                p.push('/');
            }
            Some(p)
        }
        (None, Some(name)) => Some(name.clone()),
        (None, None) => None,
    }
}

fn zenity_filters(settings: &FileDialog) -> Vec<String> {
    let mut args = Vec::new();
    for filter in &settings.filters {
        let patterns = filter
            .extensions
            .iter()
            .map(|ext| format!("*.{}", ext.trim_start_matches('.')))
            .collect::<Vec<_>>()
            .join(" ");
        if patterns.is_empty() {
            continue;
        }
        // zenity: --file-filter=Description | *.ext *.ext2
        let desc = filter.description.replace('|', "/");
        args.push(format!("--file-filter={} | {}", desc, patterns));
    }
    if !args.is_empty() {
        args.push("--file-filter=All files | *".to_string());
    }
    args
}

fn run_zenity(
    settings: &FileDialog,
    kind: DialogKind,
) -> Option<Result<Option<Vec<String>>, String>> {
    let mut cmd = Command::new("zenity");
    cmd.arg("--file-selection");
    if let Some(title) = &settings.title {
        cmd.arg(format!("--title={title}"));
    }
    if let Some(path) = initial_path(settings) {
        cmd.arg(format!("--filename={path}"));
    }
    match kind {
        DialogKind::OpenFile => {
            for filter in zenity_filters(settings) {
                cmd.arg(filter);
            }
        }
        DialogKind::SaveFile => {
            cmd.arg("--save");
            cmd.arg("--confirm-overwrite");
            for filter in zenity_filters(settings) {
                cmd.arg(filter);
            }
        }
        DialogKind::OpenFolder => {
            cmd.arg("--directory");
        }
    }
    match cmd.output() {
        Ok(output) => {
            if !output.status.success() {
                // Cancel or error — zenity returns 1 on cancel.
                return Some(Ok(None));
            }
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if path.is_empty() {
                return Some(Ok(None));
            }
            Some(Ok(Some(vec![path])))
        }
        Err(err) if err.kind() == ErrorKind::NotFound => None,
        Err(err) => Some(Err(format!("zenity failed: {err}"))),
    }
}

fn kdialog_filter(settings: &FileDialog) -> String {
    if settings.filters.is_empty() {
        return "All files (*)".to_string();
    }
    let mut parts = Vec::new();
    for filter in &settings.filters {
        let patterns = filter
            .extensions
            .iter()
            .map(|ext| format!("*.{}", ext.trim_start_matches('.')))
            .collect::<Vec<_>>()
            .join(" ");
        parts.push(format!("{} ({})", filter.description, patterns));
    }
    parts.push("All files (*)".to_string());
    parts.join("\n")
}

fn run_kdialog(
    settings: &FileDialog,
    kind: DialogKind,
) -> Option<Result<Option<Vec<String>>, String>> {
    let mut cmd = Command::new("kdialog");
    if let Some(title) = &settings.title {
        cmd.arg(format!("--title={title}"));
    }
    let start = initial_path(settings).unwrap_or_default();
    match kind {
        DialogKind::OpenFile => {
            cmd.arg("--getopenfilename");
            cmd.arg(&start);
            cmd.arg(kdialog_filter(settings));
        }
        DialogKind::SaveFile => {
            cmd.arg("--getsavefilename");
            cmd.arg(&start);
            cmd.arg(kdialog_filter(settings));
        }
        DialogKind::OpenFolder => {
            cmd.arg("--getexistingdirectory");
            cmd.arg(if start.is_empty() { "." } else { &start });
        }
    }
    match cmd.output() {
        Ok(output) => {
            if !output.status.success() {
                return Some(Ok(None));
            }
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if path.is_empty() {
                return Some(Ok(None));
            }
            let path = Path::new(&path).to_string_lossy().into_owned();
            Some(Ok(Some(vec![path])))
        }
        Err(err) if err.kind() == ErrorKind::NotFound => None,
        Err(err) => Some(Err(format!("kdialog failed: {err}"))),
    }
}

fn run_dialog(
    settings: &FileDialog,
    kind: DialogKind,
) -> Result<Option<Vec<String>>, HelperError> {
    match run_zenity(settings, kind) {
        Some(Ok(result)) => return Ok(result),
        Some(Err(msg)) => return Err(HelperError::Failed(msg)),
        None => {}
    }
    match run_kdialog(settings, kind) {
        Some(Ok(result)) => return Ok(result),
        Some(Err(msg)) => return Err(HelperError::Failed(msg)),
        None => {}
    }
    Err(HelperError::Missing)
}

/// Shows an open-file dialog. `Err(Missing)` means no dialog helper is available.
pub fn open_select_file_dialog(
    settings: &FileDialog,
) -> Result<Option<Vec<String>>, HelperError> {
    run_dialog(settings, DialogKind::OpenFile)
}

pub fn open_save_file_dialog(settings: &FileDialog) -> Result<Option<Vec<String>>, HelperError> {
    run_dialog(settings, DialogKind::SaveFile)
}

pub fn open_select_folder_dialog(
    settings: &FileDialog,
) -> Result<Option<Vec<String>>, HelperError> {
    run_dialog(settings, DialogKind::OpenFolder)
}

pub fn open_save_folder_dialog(
    settings: &FileDialog,
) -> Result<Option<Vec<String>>, HelperError> {
    // Zenity/kdialog have no distinct “save folder”; same as open-folder.
    open_select_folder_dialog(settings)
}

/// Maps a helper result into a [`crate::file_dialogs::FileDialogResultEvent`].
pub fn result_from_helper(
    settings: &crate::file_dialogs::FileDialog,
    result: Result<Option<Vec<String>>, HelperError>,
    unsupported_op: &str,
) -> crate::file_dialogs::FileDialogResultEvent {
    match result {
        Ok(paths) => crate::file_dialogs::FileDialogResultEvent::from_option(
            settings,
            paths,
            crate::file_dialogs::FileDialogPathKind::Filesystem,
        ),
        Err(HelperError::Missing) => {
            crate::file_dialogs::FileDialogResultEvent::unsupported_from(settings, unsupported_op)
        }
        Err(HelperError::Failed(msg)) => {
            crate::file_dialogs::FileDialogResultEvent::error_from(settings, msg)
        }
    }
}
