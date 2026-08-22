//! Native folder picker: `IFileOpenDialog` with `FOS_PICKFOLDERS`.
//!
//! The one dialog the apps actually use (`CxOsOp::SelectFolderDialog` — the
//! VJ's IMPORT CONTENT). Mirrors the macOS contract exactly: the dialog is
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

const FOS_PICKFOLDERS: u32 = 0x20;
const FOS_FORCEFILESYSTEM: u32 = 0x40;
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

/// Opens the native folder picker on its own STA thread and posts the
/// answer as a [`FileDialogAction`].
pub fn open_select_folder_dialog(settings: FileDialog) {
    std::thread::Builder::new()
        .name("folder-dialog".into())
        .spawn(move || {
            let picked = unsafe { run_folder_dialog(&settings) };
            Cx::post_action(match picked {
                Some(path) => FileDialogAction::FolderSelected(path),
                None => FileDialogAction::FolderCancelled,
            });
        })
        .ok();
}

unsafe fn run_folder_dialog(settings: &FileDialog) -> Option<PathBuf> {
    let com = CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED);
    let result = run_folder_dialog_inner(settings);
    // S_OK / S_FALSE both mean this thread owes a matching uninitialize.
    if com >= 0 {
        CoUninitialize();
    }
    result
}

unsafe fn run_folder_dialog_inner(settings: &FileDialog) -> Option<PathBuf> {
    let mut raw: *mut c_void = std::ptr::null_mut();
    let hr = CoCreateInstance(
        &CLSID_FILE_OPEN_DIALOG,
        std::ptr::null_mut(),
        CLSCTX_INPROC_SERVER,
        &IID_IFILE_OPEN_DIALOG,
        &mut raw,
    );
    if hr < 0 || raw.is_null() {
        crate::error!("folder dialog: CoCreateInstance failed: {hr:#x}");
        return None;
    }
    let vtbl = *(raw as *mut *mut FileOpenDialogVtbl);
    let release = (*vtbl).release;

    let picked = (|| {
        let mut options = 0u32;
        if ((*vtbl).get_options)(raw, &mut options) < 0 {
            return None;
        }
        ((*vtbl).set_options)(raw, options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM);
        if let Some(title) = &settings.title {
            let wide: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
            ((*vtbl).set_title)(raw, wide.as_ptr());
        }
        let hr = ((*vtbl).show)(raw, std::ptr::null_mut());
        if hr == HR_CANCELLED {
            return None;
        }
        if hr < 0 {
            crate::error!("folder dialog: Show failed: {hr:#x}");
            return None;
        }
        let mut item: *mut c_void = std::ptr::null_mut();
        if ((*vtbl).get_result)(raw, &mut item) < 0 || item.is_null() {
            return None;
        }
        let item_vtbl = *(item as *mut *mut ShellItemVtbl);
        let mut wide: *mut u16 = std::ptr::null_mut();
        let path = if ((*item_vtbl).get_display_name)(item, SIGDN_FILESYSPATH, &mut wide) >= 0
            && !wide.is_null()
        {
            let mut len = 0usize;
            while *wide.add(len) != 0 {
                len += 1;
            }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(wide, len));
            CoTaskMemFree(wide as *mut c_void);
            Some(PathBuf::from(text))
        } else {
            None
        };
        ((*item_vtbl).release)(item);
        path
    })();

    release(raw);
    picked
}
