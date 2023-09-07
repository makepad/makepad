#![allow(dead_code)]
use {
    crate::{
        log,
        implement_com,
        windows::{
            Win32::System::Ole::{
                DROPEFFECT,
                IDropTarget,
                IDropTarget_Impl
            },
            Win32::System::Com::{
                IDataObject,
            },
            Win32::System::SystemServices::{
                MODIFIERKEYS_FLAGS,
            },
            Win32::Foundation::{
                POINTL,
            },
        },
    },
};
type DWORD = u64;

pub struct DropTarget { }

implement_com!{
    for_struct: DropTarget,
    identity: IDropTarget,
    wrapper_struct: DropTarget_Com,
    interface_count: 1,
    interfaces: {
        0: IDropTarget
    }
}

impl IDropTarget_Impl for DropTarget {

    fn DragEnter(&self,_p_data_obj: Option<&IDataObject>,_grf_key_state:MODIFIERKEYS_FLAGS,_pt: &POINTL,_pdweffect: *mut DROPEFFECT) -> crate::windows::core::Result<()> {
        log!("DropTarget::DragEnter");
        Ok(())
    }

    fn DragLeave(&self) -> crate::windows::core::Result<()> {
        log!("DropTarget::DragLeave");
        Ok(())
    }

    fn DragOver(&self,_grf_key_state:MODIFIERKEYS_FLAGS,_pt: &POINTL,_pdweffect: *mut DROPEFFECT) -> crate::windows::core::Result<()> {
        log!("DropTarget::DragOver");
        Ok(())
    }

    fn Drop(&self,_p_data_obj: Option<&IDataObject>, _grf_key_state: MODIFIERKEYS_FLAGS, _pt: &POINTL,_pdweffect: *mut DROPEFFECT) -> crate::windows::core::Result<()> {
        log!("DropTarget::Drop");
        Ok(())
    }
}
