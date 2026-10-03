#![allow(dead_code)]
#![allow(non_snake_case)]
#![allow(non_camel_case_types)]
use {
    crate::windows::{
        core::{self as wcore},
        Win32::{
            Foundation::{E_UNEXPECTED, S_FALSE, S_OK},
            System::Com::{IEnumFORMATETC, IEnumFORMATETCImpl, FORMATETC},
        },
    },
    std::cell::RefCell,
};


pub(crate) struct EnumFormatEtc {
    pub formats: Vec<FORMATETC>,
    pub index: RefCell<usize>,
}

// IEnumFORMATETC implementation for EnumFormatEtc, which hosts a list of FORMATETCs that can be queried by COM and DoDragDrop

impl IEnumFORMATETCImpl for EnumFormatEtc {
    fn Next(&self, celt: u32, rgelt: *mut FORMATETC, pceltfetched: *mut u32) -> wcore::HRESULT {
        // get reference to slice from rgelt pointer
        if rgelt.is_null() && celt != 0 { return crate::windows::Win32::Foundation::E_POINTER; }
        let out_formats = if celt == 0 { &mut [] } else { unsafe { std::slice::from_raw_parts_mut(rgelt, celt as usize) } };

        // figure out how many formats are still remaining and need to be copied
        let n_avail = self.formats.len() - *self.index.borrow();
        let n = if celt as usize > n_avail {
            n_avail
        } else {
            celt as usize
        };

        // if anything needs to be copied
        if n > 0 {
            // return number of formats that were copied in pceltfetched
            if pceltfetched != std::ptr::null_mut() {
                unsafe { *pceltfetched = n as u32 };
            }

            // actually copy the formats
            for i in 0..n {
                out_formats[i] = self.formats[*self.index.borrow() + i];
            }

            // and move the iterator forward
            *self.index.borrow_mut() += n;

            if n == celt as usize { S_OK } else { S_FALSE }
        } else {
            // return zero in pceltfetched
            if pceltfetched != std::ptr::null_mut() {
                unsafe { *pceltfetched = 0 };
            }

            if celt == 0 { S_OK } else { S_FALSE }
        }
    }

    fn Skip(&self, celt: u32) -> Result<(),wcore::HRESULT> {
        // figure out how many formats are still remaining and need to be skipped
        let n_avail = self.formats.len() - *self.index.borrow();
        let n = if celt as usize > n_avail {
            n_avail
        } else {
            celt as usize
        };

        // skip the formats
        if n > 0 {
            *self.index.borrow_mut() += n;
        }

        Ok(())
    }

    fn Reset(&self) -> Result<(),wcore::HRESULT> {
        // reset the iterator
        self.index.replace(0);

        Ok(())
    }

    fn Clone(&self) -> Result<IEnumFORMATETC,wcore::HRESULT> {
        // nope.
        Err(E_UNEXPECTED.into())
    }
}
