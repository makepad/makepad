#![allow(dead_code)]
use {
    std::sync::Arc,
    std::cell::RefCell,
    crate::{
        live_id::*,
        log,
        implement_com,
        event::DragItem,
        windows::Win32::{
            System::{
                Ole::{
                    DROPEFFECT,
                    DROPEFFECT_COPY,
                    DROPEFFECT_MOVE,
                    DROPEFFECT_LINK,
                    DROPEFFECT_SCROLL,
                    IDropTarget,
                    IDropTarget_Impl,
                    CF_TEXT,
                    CF_HDROP,
                    ReleaseStgMedium,
                },
                Com::{
                    IDataObject,
                    FORMATETC,
                    STGMEDIUM,
                    DVASPECT,  // this needs to be added for windows-strip to include the necessary things
                    DVASPECT_ICON,
                    DVASPECT_CONTENT,
                    TYMED,  // this needs to be added for windows-strip to include the necessary things
                    TYMED_FILE,
                    TYMED_HGLOBAL,
                    DATADIR,  // this needs to be added for windows-strip to include the necessary things
                    DATADIR_GET,
                },
                SystemServices::{
                    MODIFIERKEYS_FLAGS,
                    MK_CONTROL,
                    MK_SHIFT,
                    MK_LBUTTON,
                    MK_MBUTTON,
                    MK_RBUTTON,
                },
            },
            Foundation::{
                WPARAM,
                LPARAM,
                HWND,
                HGLOBAL,
            },
            UI::{
                WindowsAndMessaging::{
                    WM_USER,
                    PostMessageW,
                },
                Shell::{
                    DragQueryFileW,
                    HDROP,
                },
            },
            Foundation::POINTL,
        },
    },
};

#[derive(Clone)]
pub enum DropTargetMessage {
    DragEnter(MODIFIERKEYS_FLAGS,POINTL,*mut DROPEFFECT),
    DragLeave,
    DragOver(MODIFIERKEYS_FLAGS,POINTL,*mut DROPEFFECT),
    Drop(MODIFIERKEYS_FLAGS,POINTL,*mut DROPEFFECT),
}

// This uses WM_USER to send user messages back to the message queue of the window; careful when using WM_USER elsewhere
pub const WM_DROPTARGET_DRAGENTER: u32 = WM_USER + 0;
pub const WM_DROPTARGET_DRAGLEAVE: u32 = WM_USER + 1;
pub const WM_DROPTARGET_DRAGOVER: u32 = WM_USER + 2;
pub const WM_DROPTARGET_DROP: u32 = WM_USER + 3;

pub const DTF_LBUTTON: u32 = 0x0001;
pub const DTF_RBUTTON: u32 = 0x0002;
pub const DTF_SHIFT: u32 = 0x0004;
pub const DTF_CONTROL: u32 = 0x0008;
pub const DTF_MBUTTON: u32 = 0x0010;
pub const DTF_ALT: u32 = 0x0080;
pub const DTF_COPY: u32 = 0x0100;
pub const DTF_MOVE: u32 = 0x0200;
pub const DTF_LINK: u32 = 0x0400;
pub const DTF_SCROLL: u32 = 0x0800;

#[derive(Clone)]
pub struct DropTarget {
    pub drag_item: Arc<RefCell<Option<DragItem>>>,
    pub hwnd: HWND,  // which window to send the messages to
}

implement_com!{
    for_struct: DropTarget,
    identity: IDropTarget,
    wrapper_struct: DropTarget_Com,
    interface_count: 1,
    interfaces: {
        0: IDropTarget
    }
}

// convert POINTL struct to LPARAM value
fn pointl_to_lparam(pt: &POINTL) -> LPARAM {
    LPARAM(((pt.y << 16) | (pt.x & 0xFFFF)) as isize)
}

// transform MODIFIERKEYS_FLAGS and DROPEFFECT into single WPARAM (only 64-bit windows)
fn modifier_keys_and_dropeffect_to_wparam(key_state: MODIFIERKEYS_FLAGS,effect: DROPEFFECT) -> WPARAM {
    let mut dtf: u32 = 0;
    if (key_state & MK_LBUTTON) != MODIFIERKEYS_FLAGS(0) {
        dtf |= DTF_LBUTTON;
    }
    if (key_state & MK_RBUTTON) != MODIFIERKEYS_FLAGS(0) {
        dtf |= DTF_RBUTTON;
    }
    if (key_state & MK_SHIFT) != MODIFIERKEYS_FLAGS(0) {
        dtf |= DTF_SHIFT;
    }
    if (key_state & MK_CONTROL) != MODIFIERKEYS_FLAGS(0) {
        dtf |= DTF_CONTROL;
    }
    if (key_state & MK_MBUTTON) != MODIFIERKEYS_FLAGS(0) {
        dtf |= DTF_MBUTTON;
    }
    // TODO: ALT
    if (effect & DROPEFFECT_COPY) != DROPEFFECT(0) {
        dtf |= DTF_COPY;
    }
    if (effect & DROPEFFECT_MOVE) != DROPEFFECT(0) {
        dtf |= DTF_MOVE;
    }
    if (effect & DROPEFFECT_LINK) != DROPEFFECT(0) {
        dtf |= DTF_LINK;
    }
    if (effect & DROPEFFECT_SCROLL) != DROPEFFECT(0) {
        dtf |= DTF_SCROLL;
    }
    WPARAM(dtf as usize)
}

fn idataobject_to_dragitem(data_object: &IDataObject) -> Option<DragItem> {

    /*
    let enum_format_etc = unsafe { data_object.EnumFormatEtc(DATADIR_GET.0 as u32).unwrap() };
    log!("available FORMATETCs:");
    let mut format_etc: [FORMATETC; 256] = [FORMATETC::default(); 256];
    let mut element_count: u32 = 0;
    unsafe { enum_format_etc.Next(&mut format_etc,Some(&mut element_count)).unwrap() };
    for i in 0..element_count as usize {
        log!("- cfFormat: {},dwAspect: {},lindex: {},tymed: {}",format_etc[i].cfFormat,format_etc[i].dwAspect,format_etc[i].lindex,format_etc[i].tymed);
    }
    */

    let format_etc = FORMATETC {
        cfFormat: CF_HDROP.0,  // file drag/drop uses HDROP
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    let medium = unsafe { data_object.GetData(&format_etc).unwrap() };

    let mut buffer = [0u16; 1024];
    let hdrop = unsafe { std::mem::transmute::<HGLOBAL,HDROP>(medium.u.hGlobal) };  // Dear Microsoft, please help the poor developers that try to use your traits...
    let count = unsafe { DragQueryFileW(hdrop,0,Some(&mut buffer)) };
    let opt_drag_item = if count >= 1 {

        // buffer contains null-terminated WSTR, so manually find the end
        let mut zero_pos = 0usize;
        for i in 0..buffer.len() {
            if buffer[i] == 0 {
                zero_pos = i;
                break;
            }
        }

        // and turn only that part into a string
        let filename = String::from_utf16(&buffer[0..zero_pos]).unwrap();
        Some(DragItem::FilePath { path: filename,internal_id: None, })
    }
    else {
        None
    };
    unsafe { ReleaseStgMedium(&medium as *const STGMEDIUM as *mut STGMEDIUM) };
    opt_drag_item
}

// separate COM-able object that captures the drag/drop messages and sends them back to the window
#[allow(non_snake_case)]
impl IDropTarget_Impl for DropTarget {

    fn DragEnter(&self,_p_data_obj: Option<&IDataObject>,_grf_key_state:MODIFIERKEYS_FLAGS,_pt: &POINTL,_pdweffect: *mut DROPEFFECT) -> crate::windows::core::Result<()> {

        if let Some(data_object) = _p_data_obj {
            self.drag_item.replace(idataobject_to_dragitem(data_object));
        }
        else {
            return Ok(());
        }

        unsafe {
            PostMessageW(
                self.hwnd,
                WM_DROPTARGET_DRAGENTER,
                modifier_keys_and_dropeffect_to_wparam(_grf_key_state,*_pdweffect),
                pointl_to_lparam(&_pt)
            ).expect("cannot PostMessageW")
        };

        Ok(())
    }

    fn DragLeave(&self) -> crate::windows::core::Result<()> {

        unsafe {
            PostMessageW(
                self.hwnd,
                WM_DROPTARGET_DRAGLEAVE,
                WPARAM(0),
                LPARAM(0)
            ).expect("cannot PostMessageW")
        };

        Ok(())
    }

    fn DragOver(&self,_grf_key_state:MODIFIERKEYS_FLAGS,_pt: &POINTL,_pdweffect: *mut DROPEFFECT) -> crate::windows::core::Result<()> {

        unsafe {
            PostMessageW(
                self.hwnd,
                WM_DROPTARGET_DRAGOVER,
                modifier_keys_and_dropeffect_to_wparam(_grf_key_state,*_pdweffect),
                pointl_to_lparam(&_pt)
            ).expect("cannot PostMessageW")
        };

        Ok(())
    }

    fn Drop(&self,_p_data_obj: Option<&IDataObject>, _grf_key_state: MODIFIERKEYS_FLAGS, _pt: &POINTL,_pdweffect: *mut DROPEFFECT) -> crate::windows::core::Result<()> {

        unsafe {
            PostMessageW(
                self.hwnd,
                WM_DROPTARGET_DROP,
                modifier_keys_and_dropeffect_to_wparam(_grf_key_state,*_pdweffect),
                pointl_to_lparam(&_pt)
            ).expect("cannot PostMessageW")
        };

        Ok(())
    }
}
