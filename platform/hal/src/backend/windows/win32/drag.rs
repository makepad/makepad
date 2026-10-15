//! OLE drag and drop: incoming drags through an IDropTarget per window (`DragOver` / `DragDrop`
//! events, the drop effect answered from the app's last `DragReply` OVER), outgoing file drags
//! (`DragStart`) through IDataObject / IDropSource / IEnumFORMATETC. Moved from
//! os/windows/{droptarget, dataobject, dropsource, enumformatetc, dropfiles}.rs; every COM object is
//! built on the generated `*Impl` traits and vtables of makepad-windows-sys. In-window drags are
//! Cx's own pointer protocol and never come here.
//!
//! OLE calls the target on the main thread (its STA) from the modal loop of whoever drags: the
//! callbacks only queue events and answer from published state, never wait for the app.

use super::window::{win_of, Win};
use crate::buffers::pack_drag_items;
use crate::types::input;
use makepad_windows_sys::core::{HRESULT, BOOL};
use makepad_windows_sys::Win32::Foundation::*;
use makepad_windows_sys::Win32::Graphics::Gdi::ScreenToClient;
use makepad_windows_sys::Win32::System::Com::*;
use makepad_windows_sys::Win32::System::Memory::*;
use makepad_windows_sys::Win32::System::Ole::*;
use makepad_windows_sys::Win32::System::SystemServices::{MK_LBUTTON, MK_RBUTTON, MODIFIERKEYS_FLAGS};
use makepad_windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
use makepad_windows_sys::Win32::UI::Shell::HDROP;
use makepad_windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
use std::cell::{Cell, RefCell};

type Items = Vec<(u32, String)>;

fn format(cf: CLIPBOARD_FORMAT) -> FORMATETC {
    FORMATETC { cfFormat: cf.0, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 }
}

/// The dragged files (CF_HDROP), else the dragged text (CF_UNICODETEXT).
fn items_of(data: &IDataObject) -> Option<Items> {
    unsafe {
        let files = format(CF_HDROP);
        if data.QueryGetData(&files) == S_OK {
            if let Ok(mut medium) = data.GetData(&files) {
                let paths = super::clipboard::hdrop_paths(HDROP(medium.u.hGlobal.0));
                ReleaseStgMedium(&mut medium);
                if !paths.is_empty() {
                    return Some(paths.into_iter().map(|p| (input::drag_item::FILE, p)).collect());
                }
            }
        }
        let text = format(CF_UNICODETEXT);
        if data.QueryGetData(&text) == S_OK {
            if let Ok(mut medium) = data.GetData(&text) {
                let mem = medium.u.hGlobal;
                let p = GlobalLock(mem) as *const u16;
                let mut out = None;
                if !p.is_null() {
                    let s = std::slice::from_raw_parts(p, GlobalSize(mem) / 2);
                    let end = s.iter().position(|c| *c == 0).unwrap_or(s.len());
                    out = Some(vec![(input::drag_item::STRING, String::from_utf16_lossy(&s[..end]))]);
                    let _ = GlobalUnlock(mem);
                }
                ReleaseStgMedium(&mut medium);
                return out;
            }
        }
        None
    }
}

fn client_point(w: &Win, pt: POINTL) -> (f64, f64) {
    let mut p = POINT { x: pt.x, y: pt.y };
    unsafe {
        let _ = ScreenToClient(w.hwnd.get(), &mut p);
    }
    let dpi = w.dpi.get();
    (p.x as f64 / dpi, p.y as f64 / dpi)
}

/// The effect of the app's last answer, within what the source allows.
fn effect(w: &Win, allowed: DROPEFFECT) -> DROPEFFECT {
    let want = match w.drag_answer.get() {
        input::drag_response::COPY => DROPEFFECT_COPY,
        input::drag_response::LINK => DROPEFFECT_LINK,
        input::drag_response::MOVE => DROPEFFECT_MOVE,
        _ => return DROPEFFECT_NONE,
    };
    if allowed.0 & want.0 != 0 {
        want
    } else if allowed.0 & DROPEFFECT_COPY.0 != 0 {
        DROPEFFECT_COPY
    } else {
        DROPEFFECT_NONE
    }
}

fn push_over(w: &Win, phase: u32, pos: (f64, f64), items: &Items) {
    let list: Vec<(u32, &str)> = items.iter().map(|(k, s)| (*k, s.as_str())).collect();
    let payload = pack_drag_items(&list);
    w.push(|q, id| q.drag_over(id, pos.0, pos.1, phase, items.len() as u32, &payload));
}

fn push_drop(w: &Win, pos: (f64, f64), dropped: bool, items: &Items) {
    let list: Vec<(u32, &str)> = items.iter().map(|(k, s)| (*k, s.as_str())).collect();
    let payload = pack_drag_items(&list);
    w.push(|q, id| q.drag_drop(id, pos.0, pos.1, dropped, items.len() as u32, &payload));
}

struct DropTarget {
    hwnd: HWND,
    /// what entered (the source only hands the data over on enter and drop)
    items: RefCell<Option<Items>>,
    last: Cell<(i32, i32)>,
}

impl IDropTargetImpl for DropTarget {
    fn DragEnter(&self, data: Option<&IDataObject>, _keys: MODIFIERKEYS_FLAGS, pt: POINTL, effect_out: *mut DROPEFFECT) -> Result<(), HRESULT> {
        let Some(w) = win_of(self.hwnd) else { return Ok(()) };
        w.drag_answer.set(input::drag_response::NONE);
        let items = data.and_then(items_of);
        if let Some(items) = &items {
            push_over(&w, 0, client_point(&w, pt), items);
        }
        self.last.set((pt.x, pt.y));
        *self.items.borrow_mut() = items;
        if !effect_out.is_null() {
            unsafe { *effect_out = DROPEFFECT_NONE };
        }
        Ok(())
    }

    fn DragOver(&self, _keys: MODIFIERKEYS_FLAGS, pt: POINTL, effect_out: *mut DROPEFFECT) -> Result<(), HRESULT> {
        let Some(w) = win_of(self.hwnd) else { return Ok(()) };
        let items = self.items.borrow();
        let Some(items) = items.as_ref() else {
            if !effect_out.is_null() {
                unsafe { *effect_out = DROPEFFECT_NONE };
            }
            return Ok(());
        };
        // OLE also calls while the pointer rests: only motion is news to the app
        if self.last.replace((pt.x, pt.y)) != (pt.x, pt.y) {
            push_over(&w, 1, client_point(&w, pt), items);
        }
        if !effect_out.is_null() {
            unsafe { *effect_out = effect(&w, *effect_out) };
        }
        Ok(())
    }

    fn DragLeave(&self) -> Result<(), HRESULT> {
        let delivered = self.items.borrow_mut().take().is_some();
        if let (true, Some(w)) = (delivered, win_of(self.hwnd)) {
            w.drag_answer.set(input::drag_response::NONE);
            push_drop(&w, (-100000.0, -100000.0), false, &Vec::new());
        }
        Ok(())
    }

    fn Drop(&self, data: Option<&IDataObject>, _keys: MODIFIERKEYS_FLAGS, pt: POINTL, effect_out: *mut DROPEFFECT) -> Result<(), HRESULT> {
        let entered = self.items.borrow_mut().take();
        let Some(w) = win_of(self.hwnd) else { return Ok(()) };
        let items = data.and_then(items_of).or(entered);
        let answer = if effect_out.is_null() { DROPEFFECT_NONE } else { effect(&w, unsafe { *effect_out }) };
        match &items {
            Some(items) => push_drop(&w, client_point(&w, pt), answer != DROPEFFECT_NONE, items),
            None => push_drop(&w, (-100000.0, -100000.0), false, &Vec::new()),
        }
        w.drag_answer.set(input::drag_response::NONE);
        if !effect_out.is_null() {
            unsafe { *effect_out = answer };
        }
        Ok(())
    }
}

/// Makes the window a drop target (after OleInitialize on this thread).
pub(super) fn register(w: &Win) {
    let hwnd = w.hwnd.get();
    let target = IDropTarget::implement(Box::new(DropTarget { hwnd, items: RefCell::new(None), last: Cell::new((0, 0)) }));
    match unsafe { RegisterDragDrop(hwnd, &target) } {
        Ok(()) => *w.drop_target.borrow_mut() = Some(target),
        Err(e) => eprintln!("[hal-win32] RegisterDragDrop failed: {e:?}"),
    }
}

pub(super) fn revoke(w: &Win) {
    if w.drop_target.borrow_mut().take().is_some() {
        unsafe {
            let _ = RevokeDragDrop(w.hwnd.get());
        }
    }
}

// ---- outgoing

/// The CF_HDROP block of `paths` (DROPFILES with wide names, double-NUL terminated).
fn dropfiles(paths: &[String]) -> Option<HGLOBAL> {
    let mut names: Vec<u16> = Vec::new();
    for p in paths {
        names.extend(p.encode_utf16());
        names.push(0);
    }
    names.push(0);
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, 20 + names.len() * 2).ok()?;
        let p = GlobalLock(mem) as *mut u8;
        if p.is_null() {
            let _ = GlobalFree(Some(mem));
            return None;
        }
        // pFiles = 20 (the names follow the header), fWide = 1
        std::ptr::copy_nonoverlapping(20u32.to_le_bytes().as_ptr(), p, 4);
        std::ptr::copy_nonoverlapping(1u32.to_le_bytes().as_ptr(), p.add(16), 4);
        std::ptr::copy_nonoverlapping(names.as_ptr() as *const u8, p.add(20), names.len() * 2);
        let _ = GlobalUnlock(mem);
        Some(mem)
    }
}

fn check(f: *const FORMATETC) -> HRESULT {
    if f.is_null() {
        return DV_E_FORMATETC;
    }
    let f = unsafe { &*f };
    if f.cfFormat != CF_HDROP.0 {
        DV_E_FORMATETC
    } else if f.dwAspect != DVASPECT_CONTENT.0 {
        DV_E_DVASPECT
    } else if f.lindex != -1 {
        DV_E_LINDEX
    } else if f.tymed & TYMED_HGLOBAL.0 as u32 == 0 {
        DV_E_TYMED
    } else {
        S_OK
    }
}

struct Files(Vec<String>);

impl IDataObjectImpl for Files {
    fn GetData(&self, f: *const FORMATETC) -> Result<STGMEDIUM, HRESULT> {
        let ok = check(f);
        if ok != S_OK {
            return Err(ok);
        }
        // the receiver owns the block (ReleaseStgMedium frees it)
        let mem = dropfiles(&self.0).ok_or(E_UNEXPECTED)?;
        Ok(STGMEDIUM { tymed: TYMED_HGLOBAL.0 as u32, u: STGMEDIUM_0 { hGlobal: mem }, pUnkForRelease: std::ptr::null_mut() })
    }
    fn GetDataHere(&self, _: *const FORMATETC, _: *mut STGMEDIUM) -> Result<(), HRESULT> {
        Err(E_NOTIMPL)
    }
    fn QueryGetData(&self, f: *const FORMATETC) -> HRESULT {
        check(f)
    }
    fn GetCanonicalFormatEtc(&self, f_in: *const FORMATETC, f_out: *mut FORMATETC) -> HRESULT {
        if f_in.is_null() || f_out.is_null() {
            return E_POINTER;
        }
        unsafe {
            *f_out = *f_in;
            (*f_out).ptd = std::ptr::null_mut();
        }
        DATA_S_SAMEFORMATETC
    }
    fn SetData(&self, _: *const FORMATETC, _: *const STGMEDIUM, _: BOOL) -> Result<(), HRESULT> {
        Err(E_NOTIMPL)
    }
    fn EnumFormatEtc(&self, direction: u32) -> Result<IEnumFORMATETC, HRESULT> {
        if direction != DATADIR_GET.0 as u32 {
            return Err(E_NOTIMPL);
        }
        Ok(IEnumFORMATETC::implement(Box::new(Formats { formats: vec![format(CF_HDROP)], next: Cell::new(0) })))
    }
    fn DAdvise(&self, _: *const FORMATETC, _: u32, _: Option<&IAdviseSink>) -> Result<u32, HRESULT> {
        Err(OLE_E_ADVISENOTSUPPORTED)
    }
    fn DUnadvise(&self, _: u32) -> Result<(), HRESULT> {
        Err(OLE_E_ADVISENOTSUPPORTED)
    }
    fn EnumDAdvise(&self) -> Result<IEnumSTATDATA, HRESULT> {
        Err(OLE_E_ADVISENOTSUPPORTED)
    }
}

struct Formats {
    formats: Vec<FORMATETC>,
    next: Cell<usize>,
}

impl IEnumFORMATETCImpl for Formats {
    fn Next(&self, celt: u32, out: *mut FORMATETC, fetched: *mut u32) -> HRESULT {
        if out.is_null() && celt != 0 {
            return E_POINTER;
        }
        let at = self.next.get();
        let n = (celt as usize).min(self.formats.len() - at);
        for i in 0..n {
            unsafe { *out.add(i) = self.formats[at + i] };
        }
        self.next.set(at + n);
        if !fetched.is_null() {
            unsafe { *fetched = n as u32 };
        }
        if n == celt as usize { S_OK } else { S_FALSE }
    }
    fn Skip(&self, celt: u32) -> Result<(), HRESULT> {
        let at = self.next.get();
        let n = (celt as usize).min(self.formats.len() - at);
        self.next.set(at + n);
        if n == celt as usize { Ok(()) } else { Err(S_FALSE) }
    }
    fn Reset(&self) -> Result<(), HRESULT> {
        self.next.set(0);
        Ok(())
    }
    fn Clone(&self) -> Result<IEnumFORMATETC, HRESULT> {
        Ok(IEnumFORMATETC::implement(Box::new(Formats { formats: self.formats.clone(), next: Cell::new(self.next.get()) })))
    }
}

struct Source;

impl IDropSourceImpl for Source {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape.0 != 0 || keys.0 & MK_RBUTTON.0 != 0 {
            DRAGDROP_S_CANCEL
        } else if keys.0 & MK_LBUTTON.0 == 0 {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }
    fn GiveFeedback(&self, _: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// `DragStart`: drags existing files out of the window (always as a copy: a destination must
/// never move a managed file away). Runs OLE's modal drag loop; HAL ops keep arriving through it.
/// The drag ends, dropped or not, with a `DragDrop` without a drop (the app's gesture ends).
pub(super) fn start(w: &Win, items: &[(u32, String)]) {
    let paths: Option<Vec<String>> = items.iter().map(|(k, s)| (*k == input::drag_item::FILE).then(|| s.clone())).collect();
    let ok = match &paths {
        None => {
            eprintln!("[hal-win32] an external drag accepts only file paths");
            false
        }
        Some(p) if p.is_empty() || p.iter().any(|p| !std::path::Path::new(p).is_absolute() || !std::path::Path::new(p).is_file()) => {
            eprintln!("[hal-win32] an external drag requires absolute paths to existing files");
            false
        }
        // the press that started the gesture must still be held
        Some(_) if w.buttons.get() & 1 == 0 => {
            eprintln!("[hal-win32] an external drag needs the primary button still held");
            false
        }
        Some(_) => true,
    };
    if ok {
        let data = IDataObject::implement(Box::new(Files(paths.unwrap_or_default())));
        let source = IDropSource::implement(Box::new(Source));
        let mut done = DROPEFFECT_NONE;
        let hr = unsafe { DoDragDrop(&data, &source, DROPEFFECT_COPY, &mut done) };
        if super::log_enabled() {
            eprintln!("[hal-win32] external drag ended {hr:?} effect {}", done.0);
        }
        // OLE's loop took the button release: the window's press state ends here
        w.buttons.set(0);
        w.pending.set(None);
        unsafe {
            let _ = ReleaseCapture();
        }
    }
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
    }
    let pos = client_point(w, POINTL { x: pt.x, y: pt.y });
    push_drop(w, pos, false, &Vec::new());
}
