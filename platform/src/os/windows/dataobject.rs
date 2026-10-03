#![allow(dead_code)]
#![allow(non_snake_case)]
#![allow(non_camel_case_types)]
use {
    crate::{
        event::DragItem,
        os::windows::{dropfiles::*, enumformatetc::*},
        windows::{
            core::{self as wcore, BOOL},
            Win32::{
                Foundation::{
                    DATA_S_SAMEFORMATETC, DV_E_DVASPECT, DV_E_FORMATETC, DV_E_LINDEX, DV_E_TYMED,
                    E_NOTIMPL, E_UNEXPECTED, OLE_E_ADVISENOTSUPPORTED, S_OK,
                },
                System::{
                    Com::{
                        IAdviseSink, IDataObjectImpl, IEnumFORMATETC, IEnumSTATDATA,
                        DATADIR_GET, DVASPECT_CONTENT, FORMATETC, STGMEDIUM, STGMEDIUM_0,
                        TYMED_HGLOBAL,
                    },
                    Ole::CF_HDROP,
                },
            },
        },
    },
    std::cell::RefCell,
};


pub(crate) struct DragItemWindows(pub DragItem);
// IDataObject implementation for DragItem

#[allow(non_snake_case)]
impl IDataObjectImpl for DragItemWindows {
    fn GetData(&self, pformatetc: *const FORMATETC) -> Result<STGMEDIUM,wcore::HRESULT> {
        // if no format was supplied, return DV_E_FORMATETC
        if pformatetc == std::ptr::null_mut() {
            Err(DV_E_FORMATETC.into())
        } else {
            // if the format is not a CF_HDROP, deny
            if unsafe { (*pformatetc).cfFormat } != CF_HDROP.0 as u16 {
                Err(DV_E_FORMATETC.into())
            }
            // if the format's aspect is not DVASPECT_CONTENT, deny
            else if unsafe { (*pformatetc).dwAspect } != DVASPECT_CONTENT.0 as u32 {
                Err(DV_E_DVASPECT.into())
            }
            // if the index is not -1, deny
            else if unsafe { (*pformatetc).lindex } != -1 {
                Err(DV_E_LINDEX.into())
            }
            // if the medium is not a HGLOBAL, deny
            else if unsafe { (*pformatetc).tymed } != TYMED_HGLOBAL.0 as u32 {
                Err(DV_E_TYMED.into())
            } else {
                let hglobal_opt = create_hglobal_for_dragitem(&self.0);

                if let Some(hglobal) = hglobal_opt {
                    Ok(STGMEDIUM {
                        tymed: TYMED_HGLOBAL.0 as u32,
                        u: STGMEDIUM_0 { hGlobal: hglobal },
                        pUnkForRelease: std::ptr::null_mut(),
                    })
                } else {
                    Err(E_UNEXPECTED.into())
                }
            }
        }
    }

    fn GetDataHere(&self, _: *const FORMATETC, _: *mut STGMEDIUM) -> Result<(),wcore::HRESULT> {
        Err(E_NOTIMPL.into())
    }

    fn QueryGetData(&self, pformatetc: *const FORMATETC) -> wcore::HRESULT {
        // if no format was supplied, return DV_E_FORMATETC
        if pformatetc == std::ptr::null_mut() {
            DV_E_FORMATETC
        } else {
            // if the format is not a CF_HDROP, deny
            if unsafe { (*pformatetc).cfFormat } != CF_HDROP.0 as u16 {
                DV_E_FORMATETC
            }
            // if the format's aspect is not DVASPECT_CONTENT, deny
            else if unsafe { (*pformatetc).dwAspect } != DVASPECT_CONTENT.0 as u32 {
                DV_E_DVASPECT
            }
            // if the index is not -1, deny
            else if unsafe { (*pformatetc).lindex } != -1 {
                DV_E_LINDEX
            }
            // if the medium is not a HGLOBAL, deny
            else if unsafe { (*pformatetc).tymed } != TYMED_HGLOBAL.0 as u32 {
                DV_E_TYMED
            } else {
                S_OK
            }
        }
    }

    fn GetCanonicalFormatEtc(
        &self,
        pformatetcin: *const FORMATETC,
        pformatetcout: *mut FORMATETC,
    ) -> wcore::HRESULT {
        // if no format was supplied, return DV_E_FORMATETC
        if pformatetcin == std::ptr::null_mut() {
            return DV_E_FORMATETC;
        }

        // just copy the format and zero the device pointer
        unsafe {
            *pformatetcout = *pformatetcin;
            (*pformatetcout).ptd = std::ptr::null_mut();
        }

        DATA_S_SAMEFORMATETC
    }

    fn SetData(&self, _: *const FORMATETC, _: *const STGMEDIUM, _: BOOL) -> Result<(),wcore::HRESULT> {
        Err(E_NOTIMPL.into())
    }

    fn EnumFormatEtc(&self, dwdirection: u32) -> Result<IEnumFORMATETC,wcore::HRESULT> {
        if dwdirection != DATADIR_GET.0 as u32 {
            Err(E_NOTIMPL.into())
        } else {
            let formats = vec![FORMATETC {
                cfFormat: CF_HDROP.0,
                ptd: std::ptr::null_mut(),
                dwAspect: DVASPECT_CONTENT.0,
                lindex: -1,
                tymed: TYMED_HGLOBAL.0 as u32,
            }];
            let enum_format_etc: IEnumFORMATETC = IEnumFORMATETC::implement(Box::new(EnumFormatEtc {
                formats,
                index: RefCell::new(0),
            }));
            Ok(enum_format_etc)
        }
    }

    fn DAdvise(
        &self,
        _: *const FORMATETC,
        _: u32,
        _: Option<&IAdviseSink>,
    ) -> Result<u32,wcore::HRESULT> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn DUnadvise(&self, _: u32) -> Result<(),wcore::HRESULT> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn EnumDAdvise(&self) -> Result<IEnumSTATDATA,wcore::HRESULT> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }
}
