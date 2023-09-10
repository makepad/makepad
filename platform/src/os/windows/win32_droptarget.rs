#![allow(dead_code)]
use {
    std::sync::Arc,
    std::cell::Cell,
    crate::{
        implement_com,
        windows::Win32::{
            System::{
                Ole::{
                    DROPEFFECT,
                    DROPEFFECT_COPY,
                    DROPEFFECT_MOVE,
                    DROPEFFECT_LINK,
                    DROPEFFECT_SCROLL,
                    IDropTarget,
                    IDropTarget_Impl
                },
                Com::IDataObject,
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
            },
            UI::WindowsAndMessaging::{
                WM_USER,
                PostMessageW,
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
    pub data_object: Arc<Cell<Option<IDataObject>>>,
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

// separate COM-able object that captures the drag/drop messages and sends them back to the window
#[allow(non_snake_case)]
impl IDropTarget_Impl for DropTarget {

    fn DragEnter(&self,_p_data_obj: Option<&IDataObject>,_grf_key_state:MODIFIERKEYS_FLAGS,_pt: &POINTL,_pdweffect: *mut DROPEFFECT) -> crate::windows::core::Result<()> {

        if let Some(data_object) = _p_data_obj {
            self.data_object.set(Some(data_object.clone()));
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

        if let Some(data_object) = _p_data_obj {
            self.data_object.set(Some(data_object.clone()));
        }
        else {
            return Ok(());
        }

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
