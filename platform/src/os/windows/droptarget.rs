#![allow(dead_code)]
#![allow(non_snake_case)]
#![allow(non_camel_case_types)]
use {
    crate::{
        //implement_com,
        event::DragItem,
        log,
        os::windows::dropfiles::*,
        windows::{
            core::{self as wcore},
            Win32::{
                Foundation::{HWND, LPARAM, POINTL, WPARAM},
                System::{
                    Com::{IDataObject, DATADIR_GET, FORMATETC},
                    Ole::{IDropTarget, IDropTarget_Impl, CF_HDROP, DROPEFFECT},
                    SystemServices::MODIFIERKEYS_FLAGS,
                },
                UI::WindowsAndMessaging::{SendMessageW, WM_USER},
            },
        },
    },
    std::cell::RefCell,
};

#[derive(Clone)]
pub enum DropTargetMessage {
    Enter(MODIFIERKEYS_FLAGS, POINTL, DROPEFFECT, DragItem),
    Leave,
    Over(MODIFIERKEYS_FLAGS, POINTL, DROPEFFECT, DragItem),
    Drop(MODIFIERKEYS_FLAGS, POINTL, DROPEFFECT, DragItem),
}

// This uses WM_USER to send user messages back to the message queue of the window; careful when using WM_USER elsewhere
pub const WM_DROPTARGET: u32 = WM_USER + 0;

#[derive(Clone)]
pub(crate) struct DropTarget {
    pub drag_item: RefCell<Option<DragItem>>, // Windows only provides the data item for Enter and Drop, but makepad needs it for Over as well
    pub hwnd: HWND,                           // which window to send the messages to
}
crate::implement_com! {
    for_struct: DropTarget,
    identity: IDropTarget,
    wrapper_struct: DropTarget_Impl,
    interface_count: 1,
    interfaces: {
        0: IDropTarget
    }
}

fn create_dragitem_from_idataobject(data_object: &IDataObject) -> Option<DragItem> {
    // obtain enumerator for all DATADIR_GET formats of this object
    let enum_formats = unsafe { data_object.EnumFormatEtc(DATADIR_GET.0 as u32).unwrap() };

    //log!("available FORMATETCs:");

    // extract all formats from the enumerator
    let mut formats: [FORMATETC; 256] = [FORMATETC::default(); 256];
    let mut element_count: u32 = 0;
    unsafe {
        enum_formats
            .Next(&mut formats, Some(&mut element_count))
            .unwrap()
    };

    // find the one CF_HDROP format
    let mut format: Option<&FORMATETC> = None;
    for i in 0..element_count as usize {
        //log!("    cfFormat: {},dwAspect: {},lindex: {},tymed: {}",formats[i].cfFormat,formats[i].dwAspect,formats[i].lindex,formats[i].tymed);

        if formats[i].cfFormat == CF_HDROP.0 {
            format = Some(&formats[i]);
        }
    }

    // if found...
    if let Some(format) = format {
        // get data medium of the object
        let medium = unsafe { data_object.GetData(format).unwrap() };

        // convert to DragItem
        convert_medium_to_dragitem(medium)
    } else {
        log!("CF_HDROP format not found on data object");
        None
    }
}

// IDropTarget implementation for DropTarget, which sends WM_DROPTARGET messages to the window as they appear

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(
        &self,
        _p_data_obj: wcore::Ref<'_, IDataObject>,
        _grf_key_state: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        _pdweffect: *mut DROPEFFECT,
    ) -> wcore::Result<()> {
        // ignore null pointer
        let Some(p_data_obj) = _p_data_obj.as_ref() else {
            return Ok(());
        };

        // convert _p_data_obj to DragItem
        let drag_item_opt = create_dragitem_from_idataobject(p_data_obj);

        // ignore if conversion fails
        if let None = drag_item_opt {
            return Ok(());
        }

        // store locally for Over messages
        self.drag_item.replace(drag_item_opt.clone());

        // allocate message
        let effect = unsafe { *_pdweffect };
        let param = Box::new(DropTargetMessage::Enter(
            _grf_key_state,
            *_pt,
            effect,
            drag_item_opt.unwrap(),
        ));

        // send to window for further processing
        unsafe {
            SendMessageW(
                self.hwnd,
                WM_DROPTARGET,
                Some(WPARAM(0)),
                Some(LPARAM(Box::into_raw(param) as isize)),
            )
        };

        Ok(())
    }

    fn DragLeave(&self) -> wcore::Result<()> {
        // allocate message
        let param = Box::new(DropTargetMessage::Leave);

        // forget the locally stored data item
        self.drag_item.replace(None);

        // send to window for further processing
        unsafe {
            SendMessageW(
                self.hwnd,
                WM_DROPTARGET,
                Some(WPARAM(0)),
                Some(LPARAM(Box::into_raw(param) as isize)),
            )
        };

        Ok(())
    }

    fn DragOver(
        &self,
        _grf_key_state: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        _pdweffect: *mut DROPEFFECT,
    ) -> wcore::Result<()> {
        // if for some reason there is no current drag item, exit
        if let None = *self.drag_item.borrow() {
            return Ok(());
        }

        // allocate message
        let effect = unsafe { *_pdweffect };
        let param = Box::new(DropTargetMessage::Over(
            _grf_key_state,
            *_pt,
            effect,
            self.drag_item.borrow().clone().unwrap(),
        ));

        // send to window for further processing
        unsafe {
            SendMessageW(
                self.hwnd,
                WM_DROPTARGET,
                Some(WPARAM(0)),
                Some(LPARAM(Box::into_raw(param) as isize)),
            )
        };

        Ok(())
    }

    fn Drop(
        &self,
        _p_data_obj: wcore::Ref<'_, IDataObject>,
        _grf_key_state: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        _pdweffect: *mut DROPEFFECT,
    ) -> wcore::Result<()> {
        //log!("DropTarget::Drop");

        // ignore null pointer
        let Some(p_data_obj) = _p_data_obj.as_ref() else {
            return Ok(());
        };

        // convert _p_data_obj to DragItem
        let drag_item_opt = create_dragitem_from_idataobject(p_data_obj);

        // ignore if conversion fails
        if let None = drag_item_opt {
            return Ok(());
        }

        // forget the locally stored one, after Drop we don't need it anymore
        self.drag_item.replace(None);

        // allocate message
        let effect = unsafe { *_pdweffect };
        let param = Box::new(DropTargetMessage::Drop(
            _grf_key_state,
            *_pt,
            effect,
            drag_item_opt.unwrap().clone(),
        ));

        // send to window for further processing
        unsafe {
            SendMessageW(
                self.hwnd,
                WM_DROPTARGET,
                Some(WPARAM(0)),
                Some(LPARAM(Box::into_raw(param) as isize)),
            )
        };

        Ok(())
    }
}
