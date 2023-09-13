#![allow(dead_code)]
#![allow(non_snake_case)]
#![allow(non_camel_case_types)]
use {
    std::cell::RefCell,
    std::boxed::Box,
    crate::{
        log,
        implement_com,
        event::DragItem,
        windows::{
            core,
            Win32::{
                System::{
                    Ole::{
                        DROPEFFECT,
                        CF_HDROP,
                    },
                    Com::{
                        FORMATETC,
                        DATADIR_GET,
                    },
                    SystemServices::MODIFIERKEYS_FLAGS,
                    Memory::{
                        GlobalLock,
                        GlobalUnlock,
                        GlobalSize,
                    },
                },
                Foundation::{
                    WPARAM,
                    LPARAM,
                    HWND,
                    POINTL,
                },
                UI::WindowsAndMessaging::{
                    WM_USER,
                    SendMessageW,
                },
            },
        },
        os::windows::dataobject::*,
    },
};

// This is a reimplementation of windows-rs IDropTarget that refers to the reimplemented IDataObject interface from dataobject.rs

#[repr(transparent)]pub struct IDropTarget(core::IUnknown);
impl IDropTarget {
    pub unsafe fn DragEnter<P0>(&self, pdataobj: P0, grfkeystate: MODIFIERKEYS_FLAGS, pt: POINTL, pdweffect: *mut DROPEFFECT) -> core::Result<()>
    where
        P0: core::IntoParam<IDataObject>,
    {
        (core::Interface::vtable(self).DragEnter)(core::Interface::as_raw(self), pdataobj.into_param().abi(), grfkeystate, ::core::mem::transmute(pt), pdweffect).ok()
    }
    pub unsafe fn DragOver(&self, grfkeystate: MODIFIERKEYS_FLAGS, pt: POINTL, pdweffect: *mut DROPEFFECT) -> core::Result<()> {
        (core::Interface::vtable(self).DragOver)(core::Interface::as_raw(self), grfkeystate, ::core::mem::transmute(pt), pdweffect).ok()
    }
    pub unsafe fn DragLeave(&self) -> core::Result<()> {
        (core::Interface::vtable(self).DragLeave)(core::Interface::as_raw(self)).ok()
    }
    pub unsafe fn Drop<P0>(&self, pdataobj: P0, grfkeystate: MODIFIERKEYS_FLAGS, pt: POINTL, pdweffect: *mut DROPEFFECT) -> core::Result<()>
    where
        P0: core::IntoParam<IDataObject>,
    {
        (core::Interface::vtable(self).Drop)(core::Interface::as_raw(self), pdataobj.into_param().abi(), grfkeystate, ::core::mem::transmute(pt), pdweffect).ok()
    }
}
impl ::core::cmp::Eq for IDropTarget {}
impl ::core::cmp::PartialEq for IDropTarget {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for IDropTarget {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for IDropTarget {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("IDropTarget").field(&self.0).finish()
    }
}
unsafe impl core::Interface for IDropTarget {
    type Vtable = IDropTarget_Vtbl;
}
unsafe impl core::ComInterface for IDropTarget {
    const IID: core::GUID = core::GUID::from_u128(0x00000122_0000_0000_c000_000000000046);
}

impl core::CanInto<core::IUnknown> for IDropTarget { }

#[repr(C)]
pub struct IDropTarget_Vtbl {
    pub base__: core::IUnknown_Vtbl,
    pub DragEnter: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdataobj: *mut ::core::ffi::c_void, grfkeystate: MODIFIERKEYS_FLAGS, pt: POINTL, pdweffect: *mut DROPEFFECT) -> core::HRESULT,
    pub DragOver: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, grfkeystate: MODIFIERKEYS_FLAGS, pt: POINTL, pdweffect: *mut DROPEFFECT) -> core::HRESULT,
    pub DragLeave: unsafe extern "system" fn(this: *mut ::core::ffi::c_void) -> core::HRESULT,
    pub Drop: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdataobj: *mut ::core::ffi::c_void, grfkeystate: MODIFIERKEYS_FLAGS, pt: POINTL, pdweffect: *mut DROPEFFECT) -> core::HRESULT,
}

pub trait IDropTarget_Impl: Sized {
    fn DragEnter(&self, pdataobj: ::core::option::Option<&IDataObject>, grfkeystate: MODIFIERKEYS_FLAGS, pt: &POINTL, pdweffect: *mut DROPEFFECT) -> core::Result<()>;
    fn DragOver(&self, grfkeystate: MODIFIERKEYS_FLAGS, pt: &POINTL, pdweffect: *mut DROPEFFECT) -> core::Result<()>;
    fn DragLeave(&self) -> core::Result<()>;
    fn Drop(&self, pdataobj: ::core::option::Option<&IDataObject>, grfkeystate: MODIFIERKEYS_FLAGS, pt: &POINTL, pdweffect: *mut DROPEFFECT) -> core::Result<()>;
}

impl core::RuntimeName for IDropTarget {}

impl IDropTarget_Vtbl {
    pub const fn new<Identity: core::IUnknownImpl<Impl = Impl>, Impl: IDropTarget_Impl, const OFFSET: isize>() -> IDropTarget_Vtbl {
        unsafe extern "system" fn DragEnter<Identity: core::IUnknownImpl<Impl = Impl>, Impl: IDropTarget_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdataobj: *mut ::core::ffi::c_void, grfkeystate: MODIFIERKEYS_FLAGS, pt: POINTL, pdweffect: *mut DROPEFFECT) -> core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.DragEnter(core::from_raw_borrowed(&pdataobj), ::core::mem::transmute_copy(&grfkeystate), ::core::mem::transmute(&pt), ::core::mem::transmute_copy(&pdweffect)).into()
        }
        unsafe extern "system" fn DragOver<Identity: core::IUnknownImpl<Impl = Impl>, Impl: IDropTarget_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, grfkeystate: MODIFIERKEYS_FLAGS, pt: POINTL, pdweffect: *mut DROPEFFECT) -> core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.DragOver(::core::mem::transmute_copy(&grfkeystate), ::core::mem::transmute(&pt), ::core::mem::transmute_copy(&pdweffect)).into()
        }
        unsafe extern "system" fn DragLeave<Identity: core::IUnknownImpl<Impl = Impl>, Impl: IDropTarget_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void) -> core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.DragLeave().into()
        }
        unsafe extern "system" fn Drop<Identity: core::IUnknownImpl<Impl = Impl>, Impl: IDropTarget_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdataobj: *mut ::core::ffi::c_void, grfkeystate: MODIFIERKEYS_FLAGS, pt: POINTL, pdweffect: *mut DROPEFFECT) -> core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.Drop(core::from_raw_borrowed(&pdataobj), ::core::mem::transmute_copy(&grfkeystate), ::core::mem::transmute(&pt), ::core::mem::transmute_copy(&pdweffect)).into()
        }
        Self {
            base__: core::IUnknown_Vtbl::new::<Identity, OFFSET>(),
            DragEnter: DragEnter::<Identity, Impl, OFFSET>,
            DragOver: DragOver::<Identity, Impl, OFFSET>,
            DragLeave: DragLeave::<Identity, Impl, OFFSET>,
            Drop: Drop::<Identity, Impl, OFFSET>,
        }
    }
    pub fn matches(iid: &core::GUID) -> bool {
        iid == &<IDropTarget as core::ComInterface>::IID
    }
}

#[derive(Clone)]
pub enum DropTargetMessage {
    Enter(MODIFIERKEYS_FLAGS,POINTL,DROPEFFECT,DragItem),
    Leave,
    Over(MODIFIERKEYS_FLAGS,POINTL,DROPEFFECT,DragItem),
    Drop(MODIFIERKEYS_FLAGS,POINTL,DROPEFFECT,DragItem),
}

// This uses WM_USER to send user messages back to the message queue of the window; careful when using WM_USER elsewhere
pub const WM_DROPTARGET: u32 = WM_USER + 0;

#[derive(Clone)]
pub struct DropTarget {
    pub drag_item: RefCell<Option<DragItem>>,  // Windows only provides the data item for Enter and Drop, but makepad needs it for Over as well
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


// convert foreign IDataObject from drag operations to makepad DragItem

fn idataobject_to_dragitem(data_object: &IDataObject) -> Option<DragItem> {

    // obtain enumerator for all DATADIR_GET formats of this object
    let enum_formats = unsafe { data_object.EnumFormatEtc(DATADIR_GET.0 as u32).unwrap() };

    //log!("available FORMATETCs:");

    // extract all formats from the enumerator
    let mut formats: [FORMATETC; 256] = [FORMATETC::default(); 256];
    let mut element_count: u32 = 0;
    unsafe { enum_formats.Next(&mut formats,Some(&mut element_count)).unwrap() };

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
        
        // get data medium of the object in CF_HDROP format
        let medium = unsafe { data_object.GetData(format).unwrap() };

        // examine the HDROP structure
        let hglobal_size = unsafe { GlobalSize(medium.u.hGlobal) };
        let hglobal_raw_ptr = unsafe { GlobalLock(medium.u.hGlobal) };

        // debug dump
        /*
        let u8_slice = unsafe { std::slice::from_raw_parts_mut(hglobal_raw_ptr as *mut u8,hglobal_size) };
        for i in 0..(hglobal_size >> 4) {
            let mut line = String::new();
            for k in 0..16 {
                line.push_str(&format!(" {:02X}",u8_slice[(i << 4) + k]));
            }
            log!("{:04X}: {}",i << 4,line);
        }
        if (hglobal_size & 15) != 0 {
            let mut line = String::new();
            for k in 0..(hglobal_size & 15) {
                line.push_str(&format!(" {:02X}",u8_slice[((hglobal_size >> 4) << 4) + k]));
            }
            log!("{:04X}: {}",(hglobal_size >> 4) << 4,line);
        }
        */

        let i32_slice = unsafe { std::slice::from_raw_parts_mut(hglobal_raw_ptr as *mut i32,5) };
        let names_offset = i32_slice[0];
        let has_wide_strings = i32_slice[4];

        // guard against non-wide strings or unknown objects
        if has_wide_strings == 0 {
            log!("CF_HDROP drag object should have wide strings");
            let _ = unsafe { GlobalUnlock(medium.u.hGlobal) };
            //unsafe { ReleaseStgMedium(&medium as *const STGMEDIUM as *mut STGMEDIUM) };
            return None;
        }

        if names_offset != 20 {
            log!("unknown CF_HDROP object");
            let _ = unsafe { GlobalUnlock(medium.u.hGlobal) };
            //unsafe { ReleaseStgMedium(&medium as *const STGMEDIUM as *mut STGMEDIUM) };
            return None;
        }

        // get u16 slice with names
        let u16_slice = unsafe { std::slice::from_raw_parts_mut((hglobal_raw_ptr as *mut u8).offset(20) as *mut u16,(hglobal_size - 20) / 2) };

        // extract/decode filenames
        let mut filenames = Vec::<String>::new();
        let mut filename = String::new();
        for w in u16_slice {
            if *w != 0 {
                let c = match char::from_u32(*w as u32) {
                    Some(c) => c,
                    None => char::REPLACEMENT_CHARACTER,
                };
                filename.push(c);
            }
            else {
                if (filename.len() == 0) && (filenames.len() > 0) { 
                    break;
                }
                filenames.push(filename);
                filename = String::new();
            }
        }

        for filename in filenames.iter() {
            log!("    \"{}\"",filename);
        }

        // we don't need the Microsoft objects here anymore
        let _ = unsafe { GlobalUnlock(medium.u.hGlobal) };
        //unsafe { ReleaseStgMedium(&medium as *const STGMEDIUM as *mut STGMEDIUM) };

        if filenames.len() != 1 {
            log!("dropping multiple files is not supported");
            None
        }
        else {
            let opt_drag_item = Some(DragItem::FilePath { path: filenames[0].clone(),internal_id: None, });
            opt_drag_item
        }
    }
    else {
        log!("unknown drag object format");
        None
    }
}


// IDropTarget implementation for DropTarget, which sends WM_DROPTARGET messages to the window as they appear

impl IDropTarget_Impl for DropTarget {

    fn DragEnter(&self,_p_data_obj: Option<&IDataObject>,_grf_key_state: MODIFIERKEYS_FLAGS,_pt: &POINTL,_pdweffect: *mut DROPEFFECT) -> core::Result<()> {

        // ignore null pointer
        if let None = _p_data_obj {
            return Ok(());
        }

        // convert _p_data_obj to DragItem
        let drag_item = idataobject_to_dragitem(_p_data_obj.unwrap());

        // ignore if conversion fails
        if let None = drag_item {
            return Ok(());
        }

        // store locally for Over messages
        self.drag_item.replace(drag_item.clone());

        // allocate message
        let effect = unsafe { *_pdweffect };
        let param = Box::new(DropTargetMessage::Enter(_grf_key_state,*_pt,effect,drag_item.unwrap()));

        // send to window for further processing
        unsafe { SendMessageW(
            self.hwnd,
            WM_DROPTARGET,
            WPARAM(0),
            LPARAM(Box::into_raw(param) as isize)
        ) };

        Ok(())
    }

    fn DragLeave(&self) -> core::Result<()> {

        // allocate message
        let param = Box::new(DropTargetMessage::Leave);

        // forget the locally stored data item
        self.drag_item.replace(None);

        // send to window for further processing
        unsafe { SendMessageW(
            self.hwnd,
            WM_DROPTARGET,
            WPARAM(0),
            LPARAM(Box::into_raw(param) as isize),
        ) };

        Ok(())
    }

    fn DragOver(&self,_grf_key_state: MODIFIERKEYS_FLAGS,_pt: &POINTL,_pdweffect: *mut DROPEFFECT) -> core::Result<()> {

        // if for some reason there is no current drag item, exit
        if let None = *self.drag_item.borrow() {
            return Ok(());
        }

        // allocate message
        let effect = unsafe { *_pdweffect };
        let param = Box::new(DropTargetMessage::Over(_grf_key_state,*_pt,effect,self.drag_item.borrow().clone().unwrap()));

        // send to window for further processing
        unsafe { SendMessageW(
            self.hwnd,
            WM_DROPTARGET,
            WPARAM(0),
            LPARAM(Box::into_raw(param) as isize),                
        ) };

        Ok(())
    }

    fn Drop(&self,_p_data_obj: Option<&IDataObject>, _grf_key_state: MODIFIERKEYS_FLAGS, _pt: &POINTL,_pdweffect: *mut DROPEFFECT) -> core::Result<()> {

        log!("DropTarget::Drop");

        // ignore null pointer
        if let None = _p_data_obj {
            return Ok(());
        }

        // convert _p_data_obj to DragItem
        let drag_item = idataobject_to_dragitem(_p_data_obj.unwrap());

        // ignore if conversion fails
        if let None = drag_item {
            return Ok(());
        }

        // forget the locally stored one, after Drop we don't need it anymore
        self.drag_item.replace(None);

        // allocate message
        let effect = unsafe { *_pdweffect };
        let param = Box::new(DropTargetMessage::Drop(_grf_key_state,*_pt,effect,drag_item.unwrap().clone()));

        // send to window for further processing
        unsafe { SendMessageW(
            self.hwnd,
            WM_DROPTARGET,
            WPARAM(0),
            LPARAM(Box::into_raw(param) as isize),                
        ) };

        Ok(())
    }
}
