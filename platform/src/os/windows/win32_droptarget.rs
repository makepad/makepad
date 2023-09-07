#![allow(dead_code)]
use {
    crate::{
        log,
        implement_com,
        windows::{
            Win32::System::Ole::{
                IDropTarget,
                IDataObject,
            },
            core::{
                DWORD,
                POINTL,
            },
        },
    },
};

pub struct DropTarget { }

implement_com!{
    for_struct: DropTarget,
    identity: IDropTarget,
    wrapper_struct: DropTarget_Com,
    interface_count: 1,
    interfaces: {
        0: IDropTarget,
    },
}

impl IDropTarget_Impl for DropTarget {

    fn DragEnter(&self,pDataObj: &IDataObject,grfKeyState: DWORD,pt: POINTL,pdwEffect: &mut DWORD) -> Result<()> {
        log!("DropTarget::DragEnter");
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        log!("DropTarget::DragLeave");
        Ok(())
    }

    fn DragOver(&self,grfKeyState: DWORD,pt: POINTL,pdwEffect: &mut DWORD) -> Result<()> {
        log!("DropTarget::DragOver");
        Ok(())
    }

    fn Drop(&self,pDataObj: &IDataObject,grfKeyState: DWORD,pt: POINTL,pdwEffect: &mut DWORD) -> Result<()> {
        log!("DropTarget::Drop");
        Ok(())
    }
}
