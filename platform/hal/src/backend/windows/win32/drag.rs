//! OLE drag and drop: incoming drags through an IDropTarget per window (`DragOver` / `DragDrop`
//! events, the drop effect answered from the app's last `DragReply` OVER), outgoing file drags
//! (`DragStart`) through IDataObject / IDropSource / IEnumFORMATETC. Moved from
//! os/windows/{droptarget, dataobject, dropsource, enumformatetc, dropfiles}.rs; every COM object is
//! built on the generated `*Impl` traits and vtables of makepad-windows-sys. In-window drags are
//! Cx's own pointer protocol and never come here.
//!
//! OLE calls the target on the main thread (its STA) from the modal loop of whoever drags: the
//! callbacks only queue events and answer from published state, never wait for the app.
//!
//! A receiver's GetData of a drag out must answer in its call, on this thread. A promised file
//! still being made is waited for on the promise waiter while this thread goes on dispatching its
//! messages (`wait_pumping`), and the data object offers IDataObjectAsyncCapability, so a receiver
//! that takes it (Explorer) takes the data on a thread of its own and its UI does not wait either.

use super::window::{win_of, Win};
use crate::buffers::pack_drag_items;
use crate::types::input;
use makepad_windows_sys::core::{ObjectBuilder, HRESULT, BOOL};
use makepad_windows_sys::Win32::Foundation::*;
use makepad_windows_sys::Win32::Graphics::Gdi::ScreenToClient;
use makepad_windows_sys::Win32::System::Com::*;
use makepad_windows_sys::Win32::System::Memory::*;
use makepad_windows_sys::Win32::System::Ole::*;
use makepad_windows_sys::Win32::System::SystemServices::{MK_LBUTTON, MK_RBUTTON, MODIFIERKEYS_FLAGS};
use makepad_windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
use makepad_windows_sys::Win32::UI::Shell::HDROP;
use makepad_windows_sys::Win32::System::Threading::{CreateEventW, SetEvent};
use makepad_windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetCursorPos, MsgWaitForMultipleObjectsEx, PeekMessageW, PostQuitMessage, TranslateMessage, MSG, MWMO_INPUTAVAILABLE, PM_REMOVE,
    QS_ALLINPUT, WM_QUIT,
};
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
        // promised files are handed over once they are written
        let items: Vec<(u32, String)> = self.0.iter().map(|p| (input::drag_item::FILE, p.clone())).collect();
        if !wait_pumping(items) {
            return Err(E_UNEXPECTED);
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

thread_local! {
    /// This thread's drag loops under way: DoDragDrop's (`start`) and a receiver's GetData waiting
    /// for a promised file (`wait_pumping`). Both dispatch the thread's messages, so a HAL op can
    /// arrive inside one; a DragStart then ends at once, its gesture over without a drop, said.
    static LOOPS: Cell<u32> = const { Cell::new(0) };
}

/// One of this thread's drag loops, counted in [`LOOPS`] while it lives.
struct InLoop;
impl InLoop {
    fn enter() -> Self {
        LOOPS.with(|n| n.set(n.get() + 1));
        InLoop
    }
}
impl Drop for InLoop {
    fn drop(&mut self) {
        LOOPS.with(|n| n.set(n.get() - 1));
    }
}

/// [`crate::drag_promise::wait_promised`] for a receiver's GetData, which must answer in its call
/// on this thread: the promise waiter waits (`drag_promise::when_made`) while this thread goes on
/// dispatching its messages (its windows paint, HAL ops and COM calls arrive), until the waiter
/// sets an event. A WM_QUIT taken meanwhile is posted again after. Nothing promised still being
/// made: no wait. Without an event (it could not be made) it waits here, as before, and says so.
fn wait_pumping(items: Vec<(u32, String)>) -> bool {
    if !crate::drag_promise::pending(&items) {
        return crate::drag_promise::wait_promised(&items);
    }
    let event = match unsafe { CreateEventW(None, true, false, None) } {
        Ok(event) => event,
        Err(e) => {
            eprintln!("[hal-win32] drag out: no event to wait on ({e:?}): the drop waits without dispatching");
            return crate::drag_promise::wait_promised(&items);
        }
    };
    let (answer, answered) = std::sync::mpsc::channel();
    let raw = event.0 as usize;
    crate::drag_promise::when_made(items, move |made| {
        // the event first: the wait closes it once it has the answer
        unsafe {
            let _ = SetEvent(HANDLE(raw as _));
        }
        let _ = answer.send(made);
    });
    let _loop = InLoop::enter();
    let mut quit = None;
    let made = loop {
        match answered.try_recv() {
            Ok(made) => break made,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => break false,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
        if unsafe { MsgWaitForMultipleObjectsEx(Some(&[event]), u32::MAX, QS_ALLINPUT, MWMO_INPUTAVAILABLE) } == WAIT_FAILED {
            eprintln!("[hal-win32] drag out: the drop's wait could not dispatch: it waits without");
            break answered.recv().unwrap_or(false);
        }
        let mut msg = MSG::default();
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            if msg.message == WM_QUIT {
                quit = Some(msg.wParam.0 as i32);
                continue;
            }
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    };
    unsafe {
        let _ = CloseHandle(event);
        if let Some(code) = quit {
            PostQuitMessage(code);
        }
    }
    made
}

/// IDataObjectAsyncCapability (IAsyncOperation before its rename), which the generated bindings
/// do not carry, beside the drag's IDataObject on one object. A receiver that takes it calls
/// StartOperation in its drop, returns, and takes the data on a thread of its own: its UI does
/// not wait on a promised file, and DoDragDrop returns here.
mod async_op {
    use makepad_windows_sys::core::{object_add_ref, object_query, object_release, ObjectBuilder, ObjectSlot, BOOL, GUID, HRESULT};
    use makepad_windows_sys::Win32::Foundation::{E_POINTER, S_OK};
    use makepad_windows_sys::Win32::System::Com::IUnknown_Vtbl;
    use std::cell::Cell;
    use std::ffi::c_void;

    pub(super) const IID: GUID = GUID::from_u128(0x3D8B0590_F691_11D2_8EA9_006097DF5BD4);

    #[repr(C)]
    pub(super) struct Vtbl {
        pub base: IUnknown_Vtbl,
        pub set_async_mode: unsafe extern "system" fn(*mut c_void, BOOL) -> HRESULT,
        pub get_async_mode: unsafe extern "system" fn(*mut c_void, *mut BOOL) -> HRESULT,
        pub start_operation: unsafe extern "system" fn(*mut c_void, *mut c_void) -> HRESULT,
        pub in_operation: unsafe extern "system" fn(*mut c_void, *mut BOOL) -> HRESULT,
        pub end_operation: unsafe extern "system" fn(*mut c_void, HRESULT, *mut c_void, u32) -> HRESULT,
    }

    static VTABLE: Vtbl = Vtbl {
        base: IUnknown_Vtbl { QueryInterface: object_query, AddRef: object_add_ref, Release: object_release },
        set_async_mode,
        get_async_mode,
        start_operation,
        in_operation,
        end_operation,
    };

    struct State {
        asynchronous: Cell<bool>,
        running: Cell<bool>,
    }

    unsafe fn state<'a>(this: *mut c_void) -> &'a State {
        &*((*(this as *const ObjectSlot)).implementation as *const State)
    }
    unsafe fn destroy(raw: *mut c_void) {
        drop(Box::from_raw(raw as *mut State));
    }
    unsafe extern "system" fn set_async_mode(this: *mut c_void, on: BOOL) -> HRESULT {
        state(this).asynchronous.set(on.as_bool());
        S_OK
    }
    unsafe extern "system" fn get_async_mode(this: *mut c_void, out: *mut BOOL) -> HRESULT {
        if out.is_null() {
            return E_POINTER;
        }
        *out = BOOL(state(this).asynchronous.get() as i32);
        S_OK
    }
    unsafe extern "system" fn start_operation(this: *mut c_void, _: *mut c_void) -> HRESULT {
        state(this).running.set(true);
        S_OK
    }
    unsafe extern "system" fn in_operation(this: *mut c_void, out: *mut BOOL) -> HRESULT {
        if out.is_null() {
            return E_POINTER;
        }
        *out = BOOL(state(this).running.get() as i32);
        S_OK
    }
    unsafe extern "system" fn end_operation(this: *mut c_void, result: HRESULT, _: *mut c_void, effect: u32) -> HRESULT {
        state(this).running.set(false);
        if super::super::log_enabled() {
            eprintln!("[hal-win32] the receiver took the drag's data on its own thread: {result:?}, effect {effect}");
        }
        S_OK
    }

    /// Adds the capability, asynchronous from the start, to the object `builder` makes.
    pub(super) fn add(mut builder: ObjectBuilder) -> ObjectBuilder {
        let state = Box::new(State { asynchronous: Cell::new(true), running: Cell::new(false) });
        unsafe {
            builder.add(&VTABLE as *const Vtbl as *const c_void, Box::into_raw(state) as *mut c_void, destroy, vec![IID]);
        }
        builder
    }
}

/// The drag's data object: its files, and IDataObjectAsyncCapability ([`async_op`]).
fn data_object(paths: Vec<String>) -> IDataObject {
    let builder = async_op::add(ObjectBuilder::new().add_IDataObject(Box::new(Files(paths))));
    unsafe { IDataObject::from_raw(builder.finish().expect("COM object")).expect("allocated COM interface") }
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
        // inside a drag's loop (a HAL op dispatched by DoDragDrop or by a drop's wait): the
        // pointer is that drag's
        _ if LOOPS.with(Cell::get) > 0 => {
            eprintln!("[hal-win32] a drag asked while another drag's loop runs ends at once");
            false
        }
        None => {
            eprintln!("[hal-win32] an external drag accepts only file paths");
            false
        }
        Some(p) if p.is_empty() || p.iter().any(|p| !std::path::Path::new(p).is_absolute() || !(std::path::Path::new(p).is_file() || crate::drag_promise::is_promised(p))) => {
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
        let data = data_object(paths.unwrap_or_default());
        let source = IDropSource::implement(Box::new(Source));
        let mut done = DROPEFFECT_NONE;
        let hr = {
            let _loop = InLoop::enter();
            unsafe { DoDragDrop(&data, &source, DROPEFFECT_COPY, &mut done) }
        };
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

/// The drag promise test's Win32 part (`drag_promise::tests`): a receiver's GetData while the
/// producer holds its promise answers once the file is made, and this thread's messages (a timer
/// standing for the window's) are dispatched meanwhile; the third lets the producer finish. The
/// data object takes asynchronous extraction.
#[cfg(test)]
pub(crate) fn check_get_data_pumps(dir: &std::path::Path) {
    use makepad_windows_sys::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};
    thread_local! {
        static TICKS: Cell<u32> = const { Cell::new(0) };
        static PRODUCER: RefCell<Option<(std::path::PathBuf, std::fs::File)>> = const { RefCell::new(None) };
    }
    unsafe extern "system" fn tick(_: HWND, _: u32, _: usize, _: u32) {
        let n = TICKS.with(|t| {
            t.set(t.get() + 1);
            t.get()
        });
        if n == 3 {
            if let Some((path, held)) = PRODUCER.with_borrow_mut(Option::take) {
                std::fs::write(&path, b"made while the receiver waited").unwrap();
                drop(held);
            }
        }
    }
    let path = dir.join("pumped.mp3");
    let name = path.to_string_lossy().into_owned();
    PRODUCER.with_borrow_mut(|p| *p = Some((path.clone(), crate::drag_promise::hold(&path).unwrap())));
    let data = data_object(vec![name.clone()]);
    unsafe {
        let timer = SetTimer(None, 0, 1, Some(tick));
        let mut medium = data.GetData(&format(CF_HDROP)).expect("the file list once the file is made");
        let _ = KillTimer(None, timer);
        assert!(TICKS.with(Cell::get) >= 3, "the thread's messages were dispatched while GetData waited");
        assert_eq!(LOOPS.with(Cell::get), 0, "the wait's loop ended with it");
        assert_eq!(super::clipboard::hdrop_paths(HDROP(medium.u.hGlobal.0)), vec![name]);
        ReleaseStgMedium(&mut medium);
        let cap = makepad_windows_sys::core::query(data.as_raw(), &async_op::IID).expect("the data object takes asynchronous extraction");
        let v = &**(cap as *const *const async_op::Vtbl);
        let mut on = BOOL(0);
        assert_eq!((v.get_async_mode)(cap, &mut on), S_OK);
        assert!(on.as_bool(), "asynchronous from the start");
        assert_eq!((v.start_operation)(cap, std::ptr::null_mut()), S_OK);
        let _ = (v.in_operation)(cap, &mut on);
        assert!(on.as_bool(), "in operation once started");
        assert_eq!((v.end_operation)(cap, S_OK, std::ptr::null_mut(), DROPEFFECT_COPY.0), S_OK);
        let _ = (v.in_operation)(cap, &mut on);
        assert!(!on.as_bool(), "not after it ended");
        makepad_windows_sys::core::release(cap);
    }
}
