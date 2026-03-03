//! AV1 decoder wrapper built on the pure-Rust `rav1d` crate.

#![allow(non_camel_case_types, dead_code)]

use {
    rav1d::{
        include::dav1d::{
            data::Dav1dData,
            dav1d::{Dav1dContext, Dav1dSettings},
            headers::{
                Dav1dSequenceHeader, DAV1D_MC_BT2020_CL, DAV1D_MC_BT2020_NCL,
                DAV1D_MC_BT470BG, DAV1D_MC_BT601, DAV1D_MC_BT709, DAV1D_MC_FCC,
                DAV1D_PIXEL_LAYOUT_I400, DAV1D_PIXEL_LAYOUT_I420, DAV1D_PIXEL_LAYOUT_I422,
                DAV1D_PIXEL_LAYOUT_I444,
            },
            picture::Dav1dPicture,
        },
        src::lib::{
            dav1d_close, dav1d_data_create, dav1d_data_unref, dav1d_default_settings,
            dav1d_flush, dav1d_get_picture, dav1d_open, dav1d_picture_unref, dav1d_send_data,
        },
    },
    std::ptr::NonNull,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rav1dPixelLayout {
    I400 = 0,
    I420 = 1,
    I422 = 2,
    I444 = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rav1dMatrixCoefficients {
    Identity = 0,
    BT709 = 1,
    Unknown = 2,
    FCC = 4,
    BT470BG = 5,
    BT601 = 6,
    SMPTE240 = 7,
    SMPTE_YCGCO = 8,
    BT2020_NCL = 9,
    BT2020_CL = 10,
    SMPTE2085 = 11,
    ChromatNCL = 12,
    ChromatCL = 13,
    ICTCP = 14,
    Reserved = 255,
}

pub const DAV1D_ERR_EAGAIN: i32 = -libc_eagain();

const fn libc_eagain() -> i32 {
    // EAGAIN is 11 on Linux, macOS, Windows, and Android.
    11
}

pub struct Rav1dDecoder {
    ctx: Option<Dav1dContext>,
}

unsafe impl Send for Rav1dDecoder {}

impl Rav1dDecoder {
    pub fn new() -> Result<Self, String> {
        unsafe {
            let mut settings: Dav1dSettings = std::mem::zeroed();
            dav1d_default_settings(NonNull::from(&mut settings));
            settings.n_threads = 2;
            settings.max_frame_delay = 1;

            let mut ctx: Option<Dav1dContext> = None;
            let ret = dav1d_open(
                Some(NonNull::from(&mut ctx)),
                Some(NonNull::from(&mut settings)),
            );
            if ret.0 < 0 || ctx.is_none() {
                return Err(format!("rav1d open failed: {}", ret.0));
            }

            Ok(Rav1dDecoder { ctx })
        }
    }

    /// Send compressed AV1 sample bytes to the decoder.
    /// Returns Ok(true) if consumed, Ok(false) if EAGAIN.
    pub fn send_data(&mut self, data: &[u8], pts: i64) -> Result<bool, String> {
        unsafe {
            let mut packet = Dav1dData::default();
            let buf = dav1d_data_create(Some(NonNull::from(&mut packet)), data.len());
            if buf.is_null() {
                return Err("rav1d data_create failed".into());
            }

            std::ptr::copy_nonoverlapping(data.as_ptr(), buf, data.len());
            packet.m.timestamp = pts;

            let ret = dav1d_send_data(self.ctx, Some(NonNull::from(&mut packet)));
            if ret.0 == DAV1D_ERR_EAGAIN {
                dav1d_data_unref(Some(NonNull::from(&mut packet)));
                return Ok(false);
            }
            if ret.0 < 0 {
                dav1d_data_unref(Some(NonNull::from(&mut packet)));
                return Err(format!("rav1d send_data failed: {}", ret.0));
            }

            Ok(true)
        }
    }

    /// Try to get a decoded picture. Returns None if EAGAIN.
    pub fn get_picture(&mut self) -> Result<Option<DecodedPicture>, String> {
        unsafe {
            let mut pic: Dav1dPicture = std::mem::zeroed();
            let ret = dav1d_get_picture(self.ctx, Some(NonNull::from(&mut pic)));
            if ret.0 == DAV1D_ERR_EAGAIN {
                return Ok(None);
            }
            if ret.0 < 0 {
                return Err(format!("rav1d get_picture failed: {}", ret.0));
            }

            Ok(Some(DecodedPicture { pic }))
        }
    }

    pub fn flush(&mut self) {
        if let Some(ctx) = self.ctx {
            unsafe {
                dav1d_flush(ctx);
            }
        }
    }
}

impl Drop for Rav1dDecoder {
    fn drop(&mut self) {
        if self.ctx.is_some() {
            unsafe {
                dav1d_close(Some(NonNull::from(&mut self.ctx)));
            }
        }
    }
}

pub struct DecodedPicture {
    pub pic: Dav1dPicture,
}

impl DecodedPicture {
    pub fn width(&self) -> u32 {
        self.pic.p.w as u32
    }

    pub fn height(&self) -> u32 {
        self.pic.p.h as u32
    }

    pub fn layout(&self) -> Rav1dPixelLayout {
        match self.pic.p.layout {
            DAV1D_PIXEL_LAYOUT_I400 => Rav1dPixelLayout::I400,
            DAV1D_PIXEL_LAYOUT_I420 => Rav1dPixelLayout::I420,
            DAV1D_PIXEL_LAYOUT_I422 => Rav1dPixelLayout::I422,
            DAV1D_PIXEL_LAYOUT_I444 => Rav1dPixelLayout::I444,
            _ => Rav1dPixelLayout::I420,
        }
    }

    pub fn bpc(&self) -> i32 {
        self.pic.p.bpc
    }

    pub fn timestamp(&self) -> i64 {
        self.pic.m.timestamp
    }

    pub fn matrix_coefficients(&self) -> Rav1dMatrixCoefficients {
        let Some(seq_hdr_ptr) = self.pic.seq_hdr else {
            return Rav1dMatrixCoefficients::BT709;
        };

        let mtrx = unsafe { seq_hdr_ptr.as_ref().mtrx };
        match mtrx {
            DAV1D_MC_BT709 => Rav1dMatrixCoefficients::BT709,
            DAV1D_MC_FCC => Rav1dMatrixCoefficients::FCC,
            DAV1D_MC_BT470BG => Rav1dMatrixCoefficients::BT470BG,
            DAV1D_MC_BT601 => Rav1dMatrixCoefficients::BT601,
            DAV1D_MC_BT2020_NCL => Rav1dMatrixCoefficients::BT2020_NCL,
            DAV1D_MC_BT2020_CL => Rav1dMatrixCoefficients::BT2020_CL,
            _ => Rav1dMatrixCoefficients::Unknown,
        }
    }

    pub fn is_full_range(&self) -> bool {
        let Some(seq_hdr_ptr) = self.pic.seq_hdr else {
            return false;
        };

        unsafe { seq_hdr_ptr.as_ref().color_range != 0 }
    }

    pub fn plane_y(&self) -> (*const u8, isize) {
        let ptr = self.pic.data[0]
            .map(|p| p.as_ptr() as *const u8)
            .unwrap_or(std::ptr::null());
        (ptr, self.pic.stride[0])
    }

    pub fn plane_u(&self) -> (*const u8, isize) {
        let ptr = self.pic.data[1]
            .map(|p| p.as_ptr() as *const u8)
            .unwrap_or(std::ptr::null());
        (ptr, self.pic.stride[1])
    }

    pub fn plane_v(&self) -> (*const u8, isize) {
        let ptr = self.pic.data[2]
            .map(|p| p.as_ptr() as *const u8)
            .unwrap_or(std::ptr::null());
        (ptr, self.pic.stride[1])
    }

    #[allow(dead_code)]
    fn seq_hdr(&self) -> Option<&Dav1dSequenceHeader> {
        self.pic.seq_hdr.map(|p| unsafe { p.as_ref() })
    }
}

impl Drop for DecodedPicture {
    fn drop(&mut self) {
        unsafe {
            dav1d_picture_unref(Some(NonNull::from(&mut self.pic)));
        }
    }
}
