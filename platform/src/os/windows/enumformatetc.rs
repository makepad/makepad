#![allow(dead_code)]
use {
    std::cell::RefCell,
    crate::{
        log,
        implement_com,
        windows::{
            core,
            Win32::{
                System::{
                    Ole::CF_HDROP,
                    Com::{
                        //IDataObject,
                        //IDataObject_Impl,
                        FORMATETC,
                        STGMEDIUM,
                        STGMEDIUM_0,
                        IAdviseSink,
                        IEnumSTATDATA,
                        DATADIR,  // for windows-strip
                        DATADIR_GET,
                        TYMED,  // for windows-strip
                        TYMED_HGLOBAL,
                        DVASPECT,  // for windows-strip
                        DVASPECT_CONTENT,
                    },
                },
                Foundation::{
                    BOOL,
                    HGLOBAL,
                    S_OK,
                    S_FALSE,
                    E_NOTIMPL,
                    OLE_E_ADVISENOTSUPPORTED,
                    OLE_S_USEREG,
                    DATA_S_SAMEFORMATETC,
                    DV_E_FORMATETC,
                    DV_E_DVASPECT,
                    DV_E_LINDEX,
                    DV_E_TYMED,
                    E_UNEXPECTED,
                },
            },
        },
    },
};

// from Microsoft import, because Next method can have two different positive results that don't fit in their Result<>

#[repr(transparent)]pub struct IEnumFORMATETC(core::IUnknown);
impl IEnumFORMATETC {

    // define Next so it returns the original HRESULT
    pub unsafe fn Next(
        &self,
        rgelt: &mut [FORMATETC],
        pceltfetched: Option<*mut u32>
    ) -> core::HRESULT {
        (core::Interface::vtable(self).Next)(
            core::Interface::as_raw(self),
            rgelt.len() as _,
            std::mem::transmute(rgelt.as_ptr()),
            std::mem::transmute(pceltfetched.unwrap_or(::std::ptr::null_mut()))
        )
    }

    pub unsafe fn Skip(&self, celt: u32) -> core::Result<()> {
        (core::Interface::vtable(self).Skip)(
            core::Interface::as_raw(self),
            celt
        ).ok()
    }

    pub unsafe fn Reset(&self) -> core::Result<()> {
        (core::Interface::vtable(self).Reset)(
            core::Interface::as_raw(self)
        ).ok()
    }

    pub unsafe fn Clone(&self) -> core::Result<IEnumFORMATETC> {
        let mut result__ = std::mem::zeroed();
        (core::Interface::vtable(self).Clone)(
            core::Interface::as_raw(self),&mut result__
        ).from_abi(result__)
    }
}

impl std::cmp::Eq for IEnumFORMATETC { }

impl std::cmp::PartialEq for IEnumFORMATETC {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl std::clone::Clone for IEnumFORMATETC {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl std::fmt::Debug for IEnumFORMATETC {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("IEnumFORMATETC").field(&self.0).finish()
    }
}

unsafe impl core::Interface for IEnumFORMATETC {
    type Vtable = IEnumFORMATETC_Vtbl;
}

unsafe impl core::ComInterface for IEnumFORMATETC {
    const IID: core::GUID = core::GUID::from_u128(0x00000103_0000_0000_c000_000000000046);
}

impl core::CanInto<core::IUnknown> for IEnumFORMATETC { }

#[repr(C)]
pub struct IEnumFORMATETC_Vtbl {
    pub base__: core::IUnknown_Vtbl,
    pub Next: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, celt: u32, rgelt: *mut FORMATETC, pceltfetched: *mut u32) -> core::HRESULT,
    pub Skip: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, celt: u32) -> core::HRESULT,
    pub Reset: unsafe extern "system" fn(this: *mut ::core::ffi::c_void) -> core::HRESULT,
    pub Clone: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, ppenum: *mut *mut ::core::ffi::c_void) -> core::HRESULT,
}

pub trait IEnumFORMATETC_Impl: Sized {
    fn Next(&self, celt: u32, rgelt: *mut FORMATETC, pceltfetched: *mut u32) -> core::HRESULT;
    fn Skip(&self, celt: u32) -> core::Result<()>;
    fn Reset(&self) -> core::Result<()>;
    fn Clone(&self) -> core::Result<IEnumFORMATETC>;
}

impl core::RuntimeName for IEnumFORMATETC { }

impl IEnumFORMATETC_Vtbl {

    pub const fn new<Identity: core::IUnknownImpl<Impl = Impl>, Impl: IEnumFORMATETC_Impl, const OFFSET: isize>() -> IEnumFORMATETC_Vtbl {

        unsafe extern "system" fn Next<Identity: core::IUnknownImpl<Impl = Impl>, Impl: IEnumFORMATETC_Impl, const OFFSET: isize>(this: *mut std::ffi::c_void, celt: u32, rgelt: *mut FORMATETC, pceltfetched: *mut u32) -> core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.Next(std::mem::transmute_copy(&celt), std::mem::transmute_copy(&rgelt), std::mem::transmute_copy(&pceltfetched))
        }

        unsafe extern "system" fn Skip<Identity: core::IUnknownImpl<Impl = Impl>, Impl: IEnumFORMATETC_Impl, const OFFSET: isize>(this: *mut std::ffi::c_void, celt: u32) -> core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.Skip(std::mem::transmute_copy(&celt)).into()
        }

        unsafe extern "system" fn Reset<Identity: core::IUnknownImpl<Impl = Impl>, Impl: IEnumFORMATETC_Impl, const OFFSET: isize>(this: *mut std::ffi::c_void) -> core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.Reset().into()
        }

        unsafe extern "system" fn Clone<Identity: core::IUnknownImpl<Impl = Impl>, Impl: IEnumFORMATETC_Impl, const OFFSET: isize>(this: *mut std::ffi::c_void, ppenum: *mut *mut std::ffi::c_void) -> core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.Clone() {
                std::result::Result::Ok(ok__) => {
                    std::ptr::write(ppenum, std::mem::transmute(ok__));
                    core::HRESULT(0)
                }
                std::result::Result::Err(err) => err.into(),
            }
        }
        Self {
            base__: core::IUnknown_Vtbl::new::<Identity, OFFSET>(),
            Next: Next::<Identity, Impl, OFFSET>,
            Skip: Skip::<Identity, Impl, OFFSET>,
            Reset: Reset::<Identity, Impl, OFFSET>,
            Clone: Clone::<Identity, Impl, OFFSET>,
        }
    }
    pub fn matches(iid: &core::GUID) -> bool {
        iid == &<IEnumFORMATETC as core::ComInterface>::IID
    }
}

pub struct EnumFormatEtc {
    pub formats: Vec<FORMATETC>,
    pub index: RefCell<usize>,
}

implement_com!{
    for_struct: EnumFormatEtc,
    identity: IEnumFORMATETC,
    wrapper_struct: EnumFormatEtc_Com,
    interface_count: 1,
    interfaces: {
        0: IEnumFORMATETC
    }
}

#[allow(non_snake_case)]
impl IEnumFORMATETC_Impl for EnumFormatEtc {
    fn Next(&self, celt: u32, rgelt: *mut FORMATETC, pceltfetched: *mut u32) -> core::HRESULT {
        log!("EnumFormatEtc::Next (celt = {}, rgelt = {:?}, pceltfetched = {:?})",celt,rgelt,pceltfetched);
        let mut out_formats = unsafe { std::slice::from_raw_parts_mut(rgelt,256) };  // rgelt actually points to an array of FORMATETCs
        let n_avail = self.formats.len() - *self.index.borrow();
        log!("available formats: {}",n_avail);
        let n = if celt as usize > n_avail { n_avail } else { celt as usize };
        log!("formats to be copied: {}",n);
        if n > 0 {
            if pceltfetched != std::ptr::null_mut() {
                unsafe { *pceltfetched = n as u32 };
            }
            for i in 0..n {
                out_formats[i] = self.formats[*self.index.borrow() + i];
            }
            *self.index.borrow_mut() += n;
            S_OK
        }
        else {
            if pceltfetched != std::ptr::null_mut() {
                unsafe { *pceltfetched = 0 };
            }
            S_FALSE
        }
    }

    fn Skip(&self, celt: u32) -> core::Result<()> {
        log!("EnumFormatEtc::Skip (celt = {})",celt);
        let n_avail = self.formats.len() - *self.index.borrow();
        let n = if celt as usize > n_avail { n_avail } else { celt as usize };
        if n > 0 {
            *self.index.borrow_mut() += n;
        }
        Ok(())
    }

    fn Reset(&self) -> core::Result<()> {
        log!("EnumFormatEtc::Reset");
        self.index.replace(0);
        Ok(())
    }

    fn Clone(&self) -> core::Result<IEnumFORMATETC> {
        log!("EnumFormatEtc::Clone");
        Err(E_UNEXPECTED.into())
    }
}
