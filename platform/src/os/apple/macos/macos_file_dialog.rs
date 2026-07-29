//! Synchronous AppKit open/save panels for `CxOsOp::*FileDialog` / `*FolderDialog`.

use crate::{
    file_dialogs::FileDialog,
    os::apple::{
        apple_sys::*,
        apple_util::{nsstring_to_string, str_to_nsstring},
    },
};

/// `NSModalResponseOK` / legacy `NSFileHandlingPanelOKButton`.
const NS_MODAL_RESPONSE_OK: i64 = 1;

fn apply_common_settings(panel: ObjcId, settings: &FileDialog) {
    unsafe {
        if let Some(title) = &settings.title {
            let () = msg_send![panel, setTitle: str_to_nsstring(title)];
        }
        if let Some(location) = &settings.location {
            let path = str_to_nsstring(&location.to_string_lossy());
            let url: ObjcId = msg_send![class!(NSURL), fileURLWithPath: path];
            let () = msg_send![panel, setDirectoryURL: url];
        }
        if let Some(name) = &settings.filename {
            let () = msg_send![panel, setNameFieldStringValue: str_to_nsstring(name)];
        }
        if !settings.filters.is_empty() {
            let mut types: Vec<ObjcId> = Vec::new();
            for filter in &settings.filters {
                for ext in &filter.extensions {
                    let trimmed = ext.trim_start_matches('.');
                    if !trimmed.is_empty() {
                        types.push(str_to_nsstring(trimmed));
                    }
                }
            }
            if !types.is_empty() {
                let array: ObjcId = msg_send![
                    class!(NSArray),
                    arrayWithObjects: types.as_ptr()
                    count: types.len()
                ];
                // Deprecated but widely available; maps extensions to content types.
                let () = msg_send![panel, setAllowedFileTypes: array];
            }
        }
    }
}

fn urls_to_paths(urls: ObjcId) -> Vec<String> {
    unsafe {
        if urls == nil {
            return Vec::new();
        }
        let count: usize = msg_send![urls, count];
        let mut paths = Vec::with_capacity(count);
        for index in 0..count {
            let url: ObjcId = msg_send![urls, objectAtIndex: index];
            if url == nil {
                continue;
            }
            let path: ObjcId = msg_send![url, path];
            if path == nil {
                continue;
            }
            let path = nsstring_to_string(path);
            if !path.is_empty() {
                paths.push(path);
            }
        }
        paths
    }
}

fn run_panel(panel: ObjcId) -> Option<Vec<String>> {
    unsafe {
        let response: i64 = msg_send![panel, runModal];
        if response != NS_MODAL_RESPONSE_OK {
            return None;
        }
        let urls: ObjcId = msg_send![panel, URLs];
        let paths = urls_to_paths(urls);
        if paths.is_empty() {
            // NSSavePanel exposes a single URL via `URL` rather than `URLs`.
            let url: ObjcId = msg_send![panel, URL];
            if url != nil {
                let path: ObjcId = msg_send![url, path];
                if path != nil {
                    let path = nsstring_to_string(path);
                    if !path.is_empty() {
                        return Some(vec![path]);
                    }
                }
            }
            return None;
        }
        Some(paths)
    }
}

/// Shows a modal open-file dialog. Returns `None` if the user cancelled.
pub fn open_select_file_dialog(settings: &FileDialog) -> Option<Vec<String>> {
    unsafe {
        let panel: ObjcId = msg_send![class!(NSOpenPanel), openPanel];
        let () = msg_send![panel, setCanChooseFiles: YES];
        let () = msg_send![panel, setCanChooseDirectories: NO];
        let () = msg_send![panel, setAllowsMultipleSelection: NO];
        let () = msg_send![panel, setResolvesAliases: YES];
        apply_common_settings(panel, settings);
        run_panel(panel)
    }
}

/// Shows a modal save-file dialog. Returns `None` if the user cancelled.
pub fn open_save_file_dialog(settings: &FileDialog) -> Option<Vec<String>> {
    unsafe {
        let panel: ObjcId = msg_send![class!(NSSavePanel), savePanel];
        let () = msg_send![panel, setCanCreateDirectories: YES];
        apply_common_settings(panel, settings);
        run_panel(panel)
    }
}

/// Shows a modal open-folder dialog. Returns `None` if the user cancelled.
pub fn open_select_folder_dialog(settings: &FileDialog) -> Option<Vec<String>> {
    unsafe {
        let panel: ObjcId = msg_send![class!(NSOpenPanel), openPanel];
        let () = msg_send![panel, setCanChooseFiles: NO];
        let () = msg_send![panel, setCanChooseDirectories: YES];
        let () = msg_send![panel, setAllowsMultipleSelection: NO];
        let () = msg_send![panel, setCanCreateDirectories: YES];
        apply_common_settings(panel, settings);
        run_panel(panel)
    }
}

/// Shows a modal “save folder” dialog (directory picker that can create folders).
pub fn open_save_folder_dialog(settings: &FileDialog) -> Option<Vec<String>> {
    open_select_folder_dialog(settings)
}
