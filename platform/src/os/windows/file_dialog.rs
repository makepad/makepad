//! Native file and folder dialogs: `IFileOpenDialog` and `IFileSaveDialog`.
//!
//! Mirrors the macOS contract exactly: the dialog is
//! NOT run inline in the platform-op drain — a modal pumps messages, and
//! the drain holds the `Cx` borrow — so it runs on its own STA thread and
//! the answer comes back as a [`FileDialogAction`] through
//! [`Cx::post_action`], long after the call that asked has returned.
//!
//! The vendored windows crate carries no shell-dialog surface, so the two
//! interfaces are declared here raw, full vtable order, the way this
//! module family already hand-rolls COM (droptarget et al). Slot order is
//! the ABI: every method before the ones used must be present and in
//! sequence.

use {
    crate::cx::Cx,
    crate::cx_api::CxOsApi,
    crate::file_dialogs::{FileDialog, FileDialogAction},
    std::ffi::c_void,
    std::path::PathBuf,
};

#[repr(C)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

// {DC1C5A9C-E88A-4DDE-A5A1-60F82A20AEF7}
const CLSID_FILE_OPEN_DIALOG: Guid = Guid {
    data1: 0xDC1C5A9C,
    data2: 0xE88A,
    data3: 0x4DDE,
    data4: [0xA5, 0xA1, 0x60, 0xF8, 0x2A, 0x20, 0xAE, 0xF7],
};
// {D57C7288-D4AD-4768-BE02-9D969532D960}
const IID_IFILE_OPEN_DIALOG: Guid = Guid {
    data1: 0xD57C7288,
    data2: 0xD4AD,
    data3: 0x4768,
    data4: [0xBE, 0x02, 0x9D, 0x96, 0x95, 0x32, 0xD9, 0x60],
};
// {C0B4E2F3-BA21-4773-8DBA-335EC946EB8B}
const CLSID_FILE_SAVE_DIALOG: Guid = Guid {
    data1: 0xC0B4E2F3,
    data2: 0xBA21,
    data3: 0x4773,
    data4: [0x8D, 0xBA, 0x33, 0x5E, 0xC9, 0x46, 0xEB, 0x8B],
};
// {84BCCD23-5FDE-4CDB-AEA4-AF64B83D78AB}
const IID_IFILE_SAVE_DIALOG: Guid = Guid {
    data1: 0x84BCCD23,
    data2: 0x5FDE,
    data3: 0x4CDB,
    data4: [0xAE, 0xA4, 0xAF, 0x64, 0xB8, 0x3D, 0x78, 0xAB],
};

const FOS_OVERWRITEPROMPT: u32 = 0x2;
const FOS_PICKFOLDERS: u32 = 0x20;
const FOS_FORCEFILESYSTEM: u32 = 0x40;
const FOS_ALLOWMULTISELECT: u32 = 0x200;
const FOS_FILEMUSTEXIST: u32 = 0x1000;
/// `SIGDN_FILESYSPATH`.
const SIGDN_FILESYSPATH: u32 = 0x8005_8000;
/// `HRESULT_FROM_WIN32(ERROR_CANCELLED)` — the user closed the dialog.
const HR_CANCELLED: i32 = 0x8007_04C7_u32 as i32;
const COINIT_APARTMENTTHREADED: u32 = 0x2;

/// `IFileOpenDialog` vtable, complete through the methods used. The tail
/// past `GetResult` is present only to keep the struct honest about size —
/// never called.
#[repr(C)]
struct FileOpenDialogVtbl {
    // IUnknown
    query_interface: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    // IModalWindow
    show: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32,
    // IFileDialog
    set_file_types: unsafe extern "system" fn(*mut c_void, u32, *const c_void) -> i32,
    set_file_type_index: unsafe extern "system" fn(*mut c_void, u32) -> i32,
    get_file_type_index: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32,
    advise: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut u32) -> i32,
    unadvise: unsafe extern "system" fn(*mut c_void, u32) -> i32,
    set_options: unsafe extern "system" fn(*mut c_void, u32) -> i32,
    get_options: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32,
    set_default_folder: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32,
    set_folder: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32,
    get_folder: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32,
    get_current_selection: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32,
    set_file_name: unsafe extern "system" fn(*mut c_void, *const u16) -> i32,
    get_file_name: unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> i32,
    set_title: unsafe extern "system" fn(*mut c_void, *const u16) -> i32,
    set_ok_button_label: unsafe extern "system" fn(*mut c_void, *const u16) -> i32,
    set_file_name_label: unsafe extern "system" fn(*mut c_void, *const u16) -> i32,
    get_result: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32,
    add_place: unsafe extern "system" fn(*mut c_void, *mut c_void, u32) -> i32,
    set_default_extension: unsafe extern "system" fn(*mut c_void, *const u16) -> i32,
    close: unsafe extern "system" fn(*mut c_void, i32) -> i32,
    set_client_guid: unsafe extern "system" fn(*mut c_void, *const Guid) -> i32,
    clear_client_data: unsafe extern "system" fn(*mut c_void) -> i32,
    set_filter: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32,
    // IFileOpenDialog
    get_results: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32,
    get_selected_items: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32,
}

/// `IShellItem`, complete through `GetDisplayName`.
#[repr(C)]
struct ShellItemVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    bind_to_handler: unsafe extern "system" fn(
        *mut c_void,
        *mut c_void,
        *const Guid,
        *const Guid,
        *mut *mut c_void,
    ) -> i32,
    get_parent: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32,
    get_display_name: unsafe extern "system" fn(*mut c_void, u32, *mut *mut u16) -> i32,
    get_attributes: unsafe extern "system" fn(*mut c_void, u32, *mut u32) -> i32,
    compare: unsafe extern "system" fn(*mut c_void, *mut c_void, u32, *mut i32) -> i32,
}

/// `IShellItemArray`, complete through `GetItemAt`.
#[repr(C)]
struct ShellItemArrayVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    bind_to_handler: unsafe extern "system" fn(
        *mut c_void,
        *mut c_void,
        *const Guid,
        *const Guid,
        *mut *mut c_void,
    ) -> i32,
    get_property_store:
        unsafe extern "system" fn(*mut c_void, u32, *const Guid, *mut *mut c_void) -> i32,
    get_property_description_list:
        unsafe extern "system" fn(*mut c_void, *const c_void, *const Guid, *mut *mut c_void) -> i32,
    get_attributes: unsafe extern "system" fn(*mut c_void, u32, u32, *mut u32) -> i32,
    get_count: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32,
    get_item_at: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32,
    enum_items: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32,
}

/// `COMDLG_FILTERSPEC`: one row of the dialog's file-type dropdown.
#[repr(C)]
struct FilterSpec {
    name: *const u16,
    spec: *const u16,
}

#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, coinit: u32) -> i32;
    fn CoUninitialize();
    fn CoCreateInstance(
        clsid: *const Guid,
        outer: *mut c_void,
        clsctx: u32,
        iid: *const Guid,
        out: *mut *mut c_void,
    ) -> i32;
    fn CoTaskMemFree(pv: *mut c_void);
}

const CLSCTX_INPROC_SERVER: u32 = 0x1;

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

/// Wrap a dialog run in this thread's COM apartment.
unsafe fn with_com<T>(body: impl FnOnce() -> T) -> T {
    let com = CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED);
    let result = body();
    // S_OK / S_FALSE both mean this thread owes a matching uninitialize.
    if com >= 0 {
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
fn build_filters(settings: &FileDialog) -> (Vec<FilterSpec>, Vec<Vec<u16>>) {
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
        specs.push(FilterSpec { name, spec });
    }
    (specs, storage)
}

/// Read one `IShellItem`'s filesystem path, releasing the item.
unsafe fn shell_item_path(item: *mut c_void) -> Option<PathBuf> {
    if item.is_null() {
        return None;
    }
    let item_vtbl = *(item as *mut *mut ShellItemVtbl);
    let mut raw: *mut u16 = std::ptr::null_mut();
    let path = if ((*item_vtbl).get_display_name)(item, SIGDN_FILESYSPATH, &mut raw) >= 0
        && !raw.is_null()
    {
        let mut len = 0usize;
        while *raw.add(len) != 0 {
            len += 1;
        }
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(raw, len));
        CoTaskMemFree(raw as *mut c_void);
        Some(PathBuf::from(text))
    } else {
        None
    };
    ((*item_vtbl).release)(item);
    path
}

/// One `IFileOpenDialog` run. `folders` picks directories instead of files;
/// multi-select comes from the dialog settings.
unsafe fn run_open_dialog(settings: &FileDialog, folders: bool) -> Vec<PathBuf> {
    let mut raw: *mut c_void = std::ptr::null_mut();
    let hr = CoCreateInstance(
        &CLSID_FILE_OPEN_DIALOG,
        std::ptr::null_mut(),
        CLSCTX_INPROC_SERVER,
        &IID_IFILE_OPEN_DIALOG,
        &mut raw,
    );
    if hr < 0 || raw.is_null() {
        crate::error!("file dialog: CoCreateInstance failed: {hr:#x}");
        return Vec::new();
    }
    let vtbl = *(raw as *mut *mut FileOpenDialogVtbl);
    let release = (*vtbl).release;

    let picked = (|| {
        let mut options = 0u32;
        if ((*vtbl).get_options)(raw, &mut options) < 0 {
            return Vec::new();
        }
        options |= FOS_FORCEFILESYSTEM;
        if folders {
            options |= FOS_PICKFOLDERS;
        } else {
            options |= FOS_FILEMUSTEXIST;
            if settings.multiple {
                options |= FOS_ALLOWMULTISELECT;
            }
        }
        ((*vtbl).set_options)(raw, options);
        if let Some(title) = &settings.title {
            ((*vtbl).set_title)(raw, wide(title).as_ptr());
        }
        // Held until Show returns: the dialog borrows these pointers.
        let (specs, _storage) = build_filters(settings);
        if !folders && !specs.is_empty() {
            ((*vtbl).set_file_types)(
                raw,
                specs.len() as u32,
                specs.as_ptr() as *const c_void,
            );
        }
        let hr = ((*vtbl).show)(raw, std::ptr::null_mut());
        if hr == HR_CANCELLED {
            return Vec::new();
        }
        if hr < 0 {
            crate::error!("file dialog: Show failed: {hr:#x}");
            return Vec::new();
        }
        if !folders && settings.multiple {
            let mut array: *mut c_void = std::ptr::null_mut();
            if ((*vtbl).get_results)(raw, &mut array) < 0 || array.is_null() {
                return Vec::new();
            }
            let array_vtbl = *(array as *mut *mut ShellItemArrayVtbl);
            let mut count = 0u32;
            let mut out = Vec::new();
            if ((*array_vtbl).get_count)(array, &mut count) >= 0 {
                for index in 0..count {
                    let mut item: *mut c_void = std::ptr::null_mut();
                    if ((*array_vtbl).get_item_at)(array, index, &mut item) >= 0 {
                        if let Some(path) = shell_item_path(item) {
                            out.push(path);
                        }
                    }
                }
            }
            ((*array_vtbl).release)(array);
            return out;
        }
        let mut item: *mut c_void = std::ptr::null_mut();
        if ((*vtbl).get_result)(raw, &mut item) < 0 {
            return Vec::new();
        }
        shell_item_path(item).into_iter().collect()
    })();

    release(raw);
    picked
}

/// One `IFileSaveDialog` run. Only `IFileDialog` methods are called, and
/// those occupy the same leading vtable slots as the open dialog's — the
/// two interfaces share that prefix by definition. The `IFileOpenDialog`
/// tail (`get_results`, `get_selected_items`) is NEVER touched here.
unsafe fn run_save_dialog(settings: &FileDialog) -> Option<PathBuf> {
    let mut raw: *mut c_void = std::ptr::null_mut();
    let hr = CoCreateInstance(
        &CLSID_FILE_SAVE_DIALOG,
        std::ptr::null_mut(),
        CLSCTX_INPROC_SERVER,
        &IID_IFILE_SAVE_DIALOG,
        &mut raw,
    );
    if hr < 0 || raw.is_null() {
        crate::error!("save dialog: CoCreateInstance failed: {hr:#x}");
        return None;
    }
    let vtbl = *(raw as *mut *mut FileOpenDialogVtbl);
    let release = (*vtbl).release;

    let picked = (|| {
        let mut options = 0u32;
        if ((*vtbl).get_options)(raw, &mut options) < 0 {
            return None;
        }
        ((*vtbl).set_options)(raw, options | FOS_FORCEFILESYSTEM | FOS_OVERWRITEPROMPT);
        if let Some(title) = &settings.title {
            ((*vtbl).set_title)(raw, wide(title).as_ptr());
        }
        if let Some(filename) = &settings.filename {
            ((*vtbl).set_file_name)(raw, wide(filename).as_ptr());
        }
        let (specs, _storage) = build_filters(settings);
        if !specs.is_empty() {
            ((*vtbl).set_file_types)(
                raw,
                specs.len() as u32,
                specs.as_ptr() as *const c_void,
            );
            // Append the chosen type's extension when the user typed none.
            if let Some(first) = settings.filters.first().and_then(|f| f.extensions.first()) {
                let cleaned = first.trim().trim_start_matches('*').trim_start_matches('.');
                if !cleaned.is_empty() && cleaned != "*" {
                    ((*vtbl).set_default_extension)(raw, wide(cleaned).as_ptr());
                }
            }
        }
        let hr = ((*vtbl).show)(raw, std::ptr::null_mut());
        if hr == HR_CANCELLED {
            return None;
        }
        if hr < 0 {
            crate::error!("save dialog: Show failed: {hr:#x}");
            return None;
        }
        let mut item: *mut c_void = std::ptr::null_mut();
        if ((*vtbl).get_result)(raw, &mut item) < 0 {
            return None;
        }
        shell_item_path(item)
    })();

    release(raw);
    picked
}

/// Opens the native folder picker on its own STA thread and posts the
/// answer as a [`FileDialogAction`].
pub fn open_select_folder_dialog(settings: FileDialog) {
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
