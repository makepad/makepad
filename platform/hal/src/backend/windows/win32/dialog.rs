//! File and folder dialogs (`FileDialog` -> `DialogResult`): IFileOpenDialog / IFileSaveDialog,
//! moved from os/windows/file_dialog.rs. The dialogs run modal, so they run on one persistent COM
//! STA worker (not the main thread, which keeps serving windows, and not a thread per request);
//! requests queue there in order. A cancelled or failed dialog answers with no paths.

use crate::types::dialog_kind;
use crate::Id;
use makepad_windows_sys::core::{HRESULT, PCWSTR};
use makepad_windows_sys::Win32::Foundation::HWND;
use makepad_windows_sys::Win32::System::Com::*;
use makepad_windows_sys::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use makepad_windows_sys::Win32::UI::Shell::*;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Mutex, OnceLock};

/// `HRESULT_FROM_WIN32(ERROR_CANCELLED)`: the user closed the dialog.
const HR_CANCELLED: i32 = 0x8007_04C7_u32 as i32;

pub(super) struct Request {
    pub sid: u32,
    pub req: u32,
    pub kind: u32,
    pub title: String,
    pub location: String,
    pub filename: String,
    /// (description, "ext,ext") pairs, flattened
    pub filters: Vec<String>,
    /// the window the dialog is modal to (0: none)
    pub owner: usize,
}

/// Queues a dialog on the dialog worker (started on first use).
pub(super) fn open(r: Request) {
    static WORKER: OnceLock<Mutex<Sender<Request>>> = OnceLock::new();
    let tx = WORKER.get_or_init(|| {
        let (tx, rx) = channel::<Request>();
        let spawned = std::thread::Builder::new().name("makepad-win32-dialogs".into()).spawn(move || {
            unsafe {
                let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                if hr.0 < 0 {
                    eprintln!("[hal-win32] dialog worker: CoInitializeEx failed {hr:?}");
                }
            }
            while let Ok(r) = rx.recv() {
                let paths = run(&r);
                answer(r.sid, r.req, &paths);
            }
        });
        if let Err(e) = spawned {
            eprintln!("[hal-win32] dialog worker: {e}");
        }
        Mutex::new(tx)
    });
    let (sid, req) = (r.sid, r.req);
    if super::lock(tx).send(r).is_err() {
        // the worker is gone: the request is answered as cancelled
        answer(sid, req, &[]);
    }
}

fn answer(sid: u32, req: u32, paths: &[String]) {
    let mut bytes = Vec::new();
    for p in paths {
        bytes.extend_from_slice(&(p.len() as u32).to_le_bytes());
        bytes.extend_from_slice(p.as_bytes());
    }
    super::Shared::global().push((sid, Id::NONE), |q, _| q.dialog_result(req, paths.len() as u32, &bytes));
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// "*.mp4;*.mkv" per filter. Returns the specs and the buffers they point into.
fn build_filters(filters: &[String]) -> (Vec<COMDLG_FILTERSPEC>, Vec<Vec<u16>>, Option<String>) {
    let mut storage: Vec<Vec<u16>> = Vec::new();
    let mut first_ext = None;
    let mut rows = Vec::new();
    for pair in filters.chunks(2) {
        let (desc, list) = (&pair[0], pair.get(1).map(String::as_str).unwrap_or(""));
        let pattern = list
            .split(',')
            .map(|e| {
                let cleaned = e.trim().trim_start_matches('*').trim_start_matches('.');
                if cleaned.is_empty() || e.trim() == "*" {
                    "*.*".to_string()
                } else {
                    if first_ext.is_none() {
                        first_ext = Some(cleaned.to_string());
                    }
                    format!("*.{cleaned}")
                }
            })
            .collect::<Vec<_>>()
            .join(";");
        storage.push(wide(if desc.is_empty() { &pattern } else { desc }));
        storage.push(wide(&pattern));
        rows.push(storage.len() - 2);
    }
    let specs = rows.iter().map(|i| COMDLG_FILTERSPEC { pszName: PCWSTR(storage[*i].as_ptr()), pszSpec: PCWSTR(storage[*i + 1].as_ptr()) }).collect();
    (specs, storage, first_ext)
}

unsafe fn item_path(item: IShellItem) -> Option<String> {
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
    (!text.is_empty()).then_some(text)
}

unsafe fn shell_item(path: &str) -> Option<IShellItem> {
    let w = wide(path);
    IShellItem::from_raw(SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None, &IShellItem::IID).ok()?).ok()
}

/// What the dialog shows first: the folder of `location` (a folder, or a file in one) and,
/// for a save, the offered name.
unsafe fn start_at(dialog: &IFileDialog, location: &str) {
    if location.is_empty() {
        return;
    }
    let path = std::path::Path::new(location);
    let folder = if path.is_dir() { Some(path) } else { path.parent() };
    if let Some(item) = folder.and_then(|f| f.to_str()).and_then(|f| shell_item(f)) {
        let _ = dialog.SetFolder(&item);
    }
}

fn run(r: &Request) -> Vec<String> {
    let owner = (r.owner != 0).then(|| HWND(r.owner as _));
    let result = unsafe {
        match r.kind & 0xff {
            dialog_kind::SAVE_FILE => run_save(r, owner).map(|p| p.into_iter().collect()),
            dialog_kind::SELECT_FILE => run_open(r, owner, false),
            _ => run_open(r, owner, true),
        }
    };
    match result {
        Ok(paths) => paths,
        Err(e) => {
            if e.0 != HR_CANCELLED {
                eprintln!("[hal-win32] file dialog failed: {e:?}");
            }
            Vec::new()
        }
    }
}

unsafe fn run_open(r: &Request, owner: Option<HWND>, folders: bool) -> Result<Vec<String>, HRESULT> {
    let dialog = IFileOpenDialog::from_raw(CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER, &IFileOpenDialog::IID)?)?;
    let d = dialog.as_IFileDialog();
    let multiple = r.kind & dialog_kind::MULTIPLE != 0;
    let mut options = d.GetOptions()? | FOS_FORCEFILESYSTEM;
    if folders {
        options = options | FOS_PICKFOLDERS;
    } else {
        options = options | FOS_FILEMUSTEXIST;
    }
    if multiple {
        options = options | FOS_ALLOWMULTISELECT;
    }
    d.SetOptions(options)?;
    let title = wide(&r.title);
    if !r.title.is_empty() {
        d.SetTitle(PCWSTR(title.as_ptr()))?;
    }
    // held until Show returns: the dialog borrows these
    let (specs, _storage, _) = build_filters(&r.filters);
    if !folders && !specs.is_empty() {
        d.SetFileTypes(&specs)?;
    }
    start_at(d, &r.location);
    d.Show(owner)?;
    let mut picked = Vec::new();
    if multiple {
        let items = dialog.GetResults()?;
        for index in 0..items.GetCount()? {
            picked.extend(item_path(items.GetItemAt(index)?));
        }
    } else {
        picked.extend(item_path(d.GetResult()?));
    }
    Ok(picked)
}

unsafe fn run_save(r: &Request, owner: Option<HWND>) -> Result<Option<String>, HRESULT> {
    let dialog = IFileSaveDialog::from_raw(CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER, &IFileSaveDialog::IID)?)?;
    let d = dialog.as_IFileDialog();
    d.SetOptions(d.GetOptions()? | FOS_FORCEFILESYSTEM | FOS_OVERWRITEPROMPT)?;
    let title = wide(&r.title);
    if !r.title.is_empty() {
        d.SetTitle(PCWSTR(title.as_ptr()))?;
    }
    let filename = wide(&r.filename);
    if !r.filename.is_empty() {
        d.SetFileName(PCWSTR(filename.as_ptr()))?;
    }
    let (specs, _storage, first_ext) = build_filters(&r.filters);
    let ext = first_ext.map(|e| wide(&e));
    if !specs.is_empty() {
        d.SetFileTypes(&specs)?;
        // the chosen type's extension is appended when the user typed none
        if let Some(ext) = &ext {
            d.SetDefaultExtension(PCWSTR(ext.as_ptr()))?;
        }
    }
    start_at(d, &r.location);
    d.Show(owner)?;
    Ok(item_path(d.GetResult()?))
}
