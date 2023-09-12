#![allow(dead_code)]
use {
    std::sync::Arc,
    std::cell::RefCell,
    crate::{
        log,
        error,
        implement_com,
        event::DragItem,
        live_id::LiveId,
        windows::{
            core,
            Win32::{
                System::{
                    Ole::{
                        DROPEFFECT,
                        DROPEFFECT_COPY,
                        DROPEFFECT_MOVE,
                        DROPEFFECT_LINK,
                        DROPEFFECT_SCROLL,
                        //IDropTarget,
                        //IDropTarget_Impl,
                        ReleaseStgMedium,
                        CF_HDROP,
                    },
                    Com::{
                        //IDataObject,
                        FORMATETC,
                        STGMEDIUM,
                        DVASPECT,  // for windows-strip
                        DVASPECT_CONTENT,
                        TYMED,  // for windows-strip
                        TYMED_HGLOBAL,
                        DATADIR,
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
                    Memory::{
                        GlobalAlloc,
                        GlobalLock,
                        GlobalUnlock,
                        GlobalSize,
                    },
                },
                Foundation::{
                    WPARAM,
                    LPARAM,
                    HWND,
                    HGLOBAL,
                    POINTL,
                    FALSE,
                },
                UI::{
                    WindowsAndMessaging::{
                        WM_USER,
                        PostMessageW,
                    },
                    Shell::{
                        DragQueryFileW,
                        DROPFILES,
                        HDROP,
                    },
                },
            },
        },
        os::windows::{
            dropsource::*,
            dataobject::*,
        },
    },
};

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

        let i32_slice = unsafe { std::slice::from_raw_parts_mut(hglobal_raw_ptr as *mut i32,5) };
        let names_offset = i32_slice[0];
        let has_wide_strings = i32_slice[4];

        // guard against non-wide strings or unknown objects
        if has_wide_strings == 0 {
            log!("CF_HDROP drag object should have wide strings");
            unsafe { GlobalUnlock(medium.u.hGlobal) };
            unsafe { ReleaseStgMedium(&medium as *const STGMEDIUM as *mut STGMEDIUM) };
            return None;
        }

        if (names_offset != 20) && (names_offset != 28) {
            log!("unknown CF_HDROP object");
            unsafe { GlobalUnlock(medium.u.hGlobal) };
            unsafe { ReleaseStgMedium(&medium as *const STGMEDIUM as *mut STGMEDIUM) };
            return None;
        }

        // get internal_id (if any) and u16_slice
        let mut internal_id: Option<LiveId> = None;
        
        // for external Windows shell objects
        let u16_slice = if names_offset == 20 {
            log!("external object from Windows shell");

            // get u16 slice with names
            unsafe { std::slice::from_raw_parts_mut((hglobal_raw_ptr as *mut u8).offset(20) as *mut u16,(hglobal_size - 20) / 2) }
        }

        // or internal objects with LiveId
        else {
            log!("internal object with optional LiveId");

            // extract internal_id
            let u64_slice = unsafe { std::slice::from_raw_parts_mut((hglobal_raw_ptr as *mut u8).offset(20) as *mut u64,1) };
            if u64_slice[0] != 0 {
                internal_id = Some(LiveId(u64_slice[0]));
                log!("    internal_id = {:?}",internal_id);
            }

            // get u16 slice with names
            unsafe { std::slice::from_raw_parts_mut((hglobal_raw_ptr as *mut u8).offset(28) as *mut u16,(hglobal_size - 28) / 2) }
        };

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

        // we don't need Microsoft objects here anymore
        unsafe { GlobalUnlock(medium.u.hGlobal) };
        unsafe { ReleaseStgMedium(&medium as *const STGMEDIUM as *mut STGMEDIUM) };

        if filenames.len() != 1 {
            log!("dropping multiple files is not supported");
            None
        }
        else {
            let opt_drag_item = Some(DragItem::FilePath { path: filenames[0].clone(),internal_id, });
            opt_drag_item
        }
    }
    else {
        log!("unknown drag object format");
        None
    }
}

// separate COM-able object that captures the drag/drop messages and sends them back to the window
#[allow(non_snake_case)]
impl IDropTarget_Impl for DropTarget {

    fn DragEnter(&self,_p_data_obj: Option<&IDataObject>,_grf_key_state: MODIFIERKEYS_FLAGS,_pt: &POINTL,_pdweffect: *mut DROPEFFECT) -> core::Result<()> {

        log!("dragenter");

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

    fn DragLeave(&self) -> core::Result<()> {

        log!("dragleave");

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

    fn DragOver(&self,_grf_key_state: MODIFIERKEYS_FLAGS,_pt: &POINTL,_pdweffect: *mut DROPEFFECT) -> core::Result<()> {

        log!("dragover");

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

    fn Drop(&self,_p_data_obj: Option<&IDataObject>, _grf_key_state: MODIFIERKEYS_FLAGS, _pt: &POINTL,_pdweffect: *mut DROPEFFECT) -> core::Result<()> {

        log!("drop");

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
