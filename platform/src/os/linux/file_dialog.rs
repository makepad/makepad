//! Native file and folder dialogs on Linux, through the desktop's own
//! helper (`zenity`, `kdialog`, `qarma`, `matedialog`).
//!
//! There is no one Linux file dialog. GTK apps get GTK's, Qt apps get Qt's,
//! and a toolkit that draws its own windows — as this one does — has three
//! options: link GTK (a large dependency, and wrong on a KDE desktop), talk
//! to `xdg-desktop-portal` over DBus (correct, but this tree carries no
//! DBus client), or run the helper the user's desktop already ships. The
//! third is what we do: it needs nothing new, it gives the user the file
//! dialog the rest of their desktop uses, and when no helper is present it
//! fails loudly instead of pretending.
//!
//! Same contract as macOS and Windows: never inline (the helper is modal
//! and the platform-op drain holds the `Cx` borrow), always answered later
//! through [`FileDialogAction`].

use crate::cx::Cx;
use crate::cx_api::CxOsApi;
use crate::file_dialogs::{FileDialog, FileDialogAction};
use std::path::PathBuf;
use std::process::Command;

/// Which helper we found, in preference order. Zenity first: it is the most
/// widely installed and its `--separator` makes multi-select unambiguous.
#[derive(Clone, Copy, PartialEq)]
enum Helper {
    Zenity,
    Qarma,
    MateDialog,
    KDialog,
}

impl Helper {
    fn binary(self) -> &'static str {
        match self {
            Helper::Zenity => "zenity",
            Helper::Qarma => "qarma",
            Helper::MateDialog => "matedialog",
            Helper::KDialog => "kdialog",
        }
    }

    /// Zenity's command line, which qarma and matedialog clone exactly.
    fn is_zenity_like(self) -> bool {
        !matches!(self, Helper::KDialog)
    }
}

/// Look for a helper on PATH. KDE first when the session says KDE, so a
/// Plasma user gets Plasma's dialog even with zenity installed.
fn find_helper() -> Option<Helper> {
    let kde = std::env::var("XDG_CURRENT_DESKTOP")
        .map(|d| d.to_ascii_lowercase().contains("kde"))
        .unwrap_or(false);
    let order = if kde {
        [Helper::KDialog, Helper::Zenity, Helper::Qarma, Helper::MateDialog]
    } else {
        [Helper::Zenity, Helper::Qarma, Helper::MateDialog, Helper::KDialog]
    };
    order.into_iter().find(|helper| on_path(helper.binary()))
}

fn on_path(binary: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else { return false };
    std::env::split_paths(&path).any(|dir| dir.join(binary).is_file())
}

/// What the caller wants a path for.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    OpenFile,
    SaveFile,
    OpenFolder,
    SaveFolder,
}

pub fn open_select_file_dialog(settings: FileDialog) {
    run(settings, Mode::OpenFile);
}

pub fn open_save_file_dialog(settings: FileDialog) {
    run(settings, Mode::SaveFile);
}

pub fn open_select_folder_dialog(settings: FileDialog) {
    run(settings, Mode::OpenFolder);
}

pub fn open_save_folder_dialog(settings: FileDialog) {
    run(settings, Mode::SaveFolder);
}

fn run(settings: FileDialog, mode: Mode) {
    std::thread::Builder::new()
        .name("file-dialog".into())
        .spawn(move || {
            let paths = run_helper(&settings, mode);
            Cx::post_action(answer(&settings, mode, paths));
        })
        .ok();
}

/// Turn the helper's output into the action for this mode. An empty result
/// is a cancellation — including the "no helper installed" case, which has
/// already logged why.
fn answer(settings: &FileDialog, mode: Mode, paths: Vec<PathBuf>) -> FileDialogAction {
    let id = settings.id;
    match mode {
        Mode::OpenFile => match paths.is_empty() {
            true => FileDialogAction::FileCancelled { id },
            false => FileDialogAction::FileSelected { id, paths },
        },
        Mode::SaveFile => match paths.into_iter().next() {
            Some(path) => FileDialogAction::SaveFileSelected { id, path },
            None => FileDialogAction::SaveFileCancelled { id },
        },
        Mode::OpenFolder | Mode::SaveFolder => match paths.into_iter().next() {
            Some(path) => FileDialogAction::FolderSelected(path),
            None => FileDialogAction::FolderCancelled,
        },
    }
}

fn run_helper(settings: &FileDialog, mode: Mode) -> Vec<PathBuf> {
    let Some(helper) = find_helper() else {
        crate::error!(
            "file dialog: no zenity, qarma, matedialog or kdialog on PATH — \
             install one of them for native file dialogs"
        );
        return Vec::new();
    };
    let mut command = Command::new(helper.binary());
    if helper.is_zenity_like() {
        build_zenity(&mut command, settings, mode);
    } else {
        build_kdialog(&mut command, settings, mode);
    }
    let output = match command.output() {
        Ok(output) => output,
        Err(err) => {
            crate::error!("file dialog: {} failed to run: {err}", helper.binary());
            return Vec::new();
        }
    };
    // Exit code 1 is the user cancelling, which is not an error.
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect()
}

fn build_zenity(command: &mut Command, settings: &FileDialog, mode: Mode) {
    command.arg("--file-selection");
    if let Some(title) = &settings.title {
        command.arg(format!("--title={title}"));
    }
    match mode {
        Mode::OpenFile => {
            if settings.multiple {
                command.arg("--multiple");
                // Newline, so a path containing the default `|` cannot
                // split one answer into two.
                command.arg("--separator=\n");
            }
        }
        Mode::SaveFile => {
            command.arg("--save");
            command.arg("--confirm-overwrite");
        }
        Mode::OpenFolder => {
            command.arg("--directory");
        }
        Mode::SaveFolder => {
            command.arg("--directory");
            command.arg("--save");
        }
    }
    // Zenity takes the start location and the suggested name in one
    // argument; a trailing slash is what marks it as a directory.
    let mut start = settings
        .location
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !start.is_empty() && !start.ends_with('/') {
        start.push('/');
    }
    if let Some(filename) = &settings.filename {
        start.push_str(filename);
    }
    if !start.is_empty() {
        command.arg(format!("--filename={start}"));
    }
    if matches!(mode, Mode::OpenFile | Mode::SaveFile) {
        for filter in &settings.filters {
            let patterns = filter
                .extensions
                .iter()
                .map(|e| glob_for(e))
                .collect::<Vec<_>>()
                .join(" ");
            command.arg(format!("--file-filter={} | {}", filter.description, patterns));
        }
    }
}

fn build_kdialog(command: &mut Command, settings: &FileDialog, mode: Mode) {
    let start = settings
        .location
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".to_string());
    match mode {
        Mode::OpenFile => {
            command.arg("--getopenfilename").arg(&start).arg(kdialog_filter(settings));
            if settings.multiple {
                command.arg("--multiple").arg("--separate-output");
            }
        }
        Mode::SaveFile => {
            let mut start = start;
            if let Some(filename) = &settings.filename {
                if !start.ends_with('/') {
                    start.push('/');
                }
                start.push_str(filename);
            }
            command.arg("--getsavefilename").arg(&start).arg(kdialog_filter(settings));
        }
        Mode::OpenFolder | Mode::SaveFolder => {
            command.arg("--getexistingdirectory").arg(&start);
        }
    }
    if let Some(title) = &settings.title {
        command.arg("--title").arg(title);
    }
}

/// kdialog wants one filter string: `*.mp4 *.mkv|Video`, groups joined by
/// newlines. An empty filter means "everything", which is its default.
fn kdialog_filter(settings: &FileDialog) -> String {
    settings
        .filters
        .iter()
        .map(|filter| {
            let patterns = filter
                .extensions
                .iter()
                .map(|e| glob_for(e))
                .collect::<Vec<_>>()
                .join(" ");
            format!("{patterns}|{}", filter.description)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `mp4` / `.mp4` / `*.mp4` all mean the same glob; `*` means everything.
fn glob_for(extension: &str) -> String {
    let cleaned = extension.trim().trim_start_matches('*').trim_start_matches('.');
    if cleaned.is_empty() || extension.trim() == "*" {
        "*".to_string()
    } else {
        format!("*.{cleaned}")
    }
}
