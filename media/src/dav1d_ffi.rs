//! Minimal FFI bindings for vendored dav1d AV1 decoder.

#![allow(non_camel_case_types, non_snake_case, dead_code)]

use std::ffi::c_void;
use std::os::raw::c_char;

/// Opaque dav1d decoder context.
pub enum Dav1dContext {}

/// Opaque dav1d reference.
pub enum Dav1dRef {}

// --- Enums ---

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dav1dPixelLayout {
    I400 = 0,
    I420 = 1,
    I422 = 2,
    I444 = 3,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dav1dMatrixCoefficients {
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

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dav1dInloopFilterType {
    None = 0,
    Deblock = 1,
    CDEF = 2,
    Restoration = 4,
    All = 7,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dav1dDecodeFrameType {
    All = 0,
    Reference = 1,
    Intra = 2,
    Key = 3,
}

// --- Structures ---

#[repr(C)]
pub struct Dav1dUserData {
    pub data: *const u8,
    pub r#ref: *mut Dav1dRef,
}

#[repr(C)]
pub struct Dav1dDataProps {
    pub timestamp: i64,
    pub duration: i64,
    pub offset: i64,
    pub size: usize,
    pub user_data: Dav1dUserData,
}

#[repr(C)]
pub struct Dav1dData {
    pub data: *const u8,
    pub sz: usize,
    pub r#ref: *mut Dav1dRef,
    pub m: Dav1dDataProps,
}

#[repr(C)]
pub struct Dav1dPictureParameters {
    pub w: i32,
    pub h: i32,
    pub layout: Dav1dPixelLayout,
    pub bpc: i32,
}

#[repr(C)]
pub struct Dav1dLogger {
    pub cookie: *mut c_void,
    pub callback: Option<unsafe extern "C" fn(*mut c_void, *const c_char, ...)>,
}

#[repr(C)]
pub struct Dav1dPicAllocator {
    pub cookie: *mut c_void,
    pub alloc_picture_callback: Option<unsafe extern "C" fn(*mut Dav1dPicture, *mut c_void) -> i32>,
    pub release_picture_callback: Option<unsafe extern "C" fn(*mut Dav1dPicture, *mut c_void)>,
}

#[repr(C)]
pub struct Dav1dSettings {
    pub n_threads: i32,
    pub max_frame_delay: i32,
    pub apply_grain: i32,
    pub operating_point: i32,
    pub all_layers: i32,
    pub frame_size_limit: u32,
    pub allocator: Dav1dPicAllocator,
    pub logger: Dav1dLogger,
    pub strict_std_compliance: i32,
    pub output_invisible_frames: i32,
    pub inloop_filters: Dav1dInloopFilterType,
    pub decode_frame_type: Dav1dDecodeFrameType,
    pub reserved: [u8; 16],
}

/// We don't need the full SequenceHeader layout — we only read fields from
/// `Dav1dPicture.p` (Dav1dPictureParameters) and access `seq_hdr` for color
/// matrix info. Keep it as opaque + offset-based access.
#[repr(C)]
pub struct Dav1dSequenceHeader {
    pub profile: u8,
    _pad0: [u8; 3],
    pub max_width: i32,
    pub max_height: i32,
    pub layout: Dav1dPixelLayout,
    pub pri: i32, // Dav1dColorPrimaries
    pub trc: i32, // Dav1dTransferCharacteristics
    pub mtrx: Dav1dMatrixCoefficients,
    pub chr: i32, // Dav1dChromaSamplePosition
    pub hbd: u8,
    pub color_range: u8,
    // ... more fields follow but we don't need them
}

#[repr(C)]
pub struct Dav1dFrameHeader {
    _opaque: [u8; 0],
}

/// Content light level (opaque, we don't use it).
#[repr(C)]
pub struct Dav1dContentLightLevel {
    _opaque: [u8; 4],
}

/// Mastering display (opaque, we don't use it).
#[repr(C)]
pub struct Dav1dMasteringDisplay {
    _opaque: [u8; 24],
}

/// ITU-T T.35 metadata.
#[repr(C)]
pub struct Dav1dITUTT35 {
    _opaque: [u8; 0],
}

#[repr(C)]
pub struct Dav1dPicture {
    pub seq_hdr: *mut Dav1dSequenceHeader,
    pub frame_hdr: *mut Dav1dFrameHeader,
    pub data: [*mut c_void; 3],
    pub stride: [isize; 2],
    pub p: Dav1dPictureParameters,
    pub m: Dav1dDataProps,
    pub content_light: *mut Dav1dContentLightLevel,
    pub mastering_display: *mut Dav1dMasteringDisplay,
    pub itut_t35: *mut Dav1dITUTT35,
    pub n_itut_t35: usize,
    pub reserved: [usize; 4],
    pub frame_hdr_ref: *mut Dav1dRef,
    pub seq_hdr_ref: *mut Dav1dRef,
    pub content_light_ref: *mut Dav1dRef,
    pub mastering_display_ref: *mut Dav1dRef,
    pub itut_t35_ref: *mut Dav1dRef,
    pub reserved_ref: [usize; 4],
    pub r#ref: *mut Dav1dRef,
    pub allocator_data: *mut c_void,
}

/// Minimum alignment/padding for picture buffers (matches dav1d headers).
pub const DAV1D_PICTURE_ALIGNMENT: i32 = 64;

// EAGAIN error code (negated on POSIX)
pub const DAV1D_ERR_EAGAIN: i32 = -libc_eagain();

const fn libc_eagain() -> i32 {
    // EAGAIN is 11 on Linux, macOS, Windows, Android
    11
}

#[cfg(has_dav1d)]
extern "C" {
    pub fn dav1d_default_settings(s: *mut Dav1dSettings);
    pub fn dav1d_open(c_out: *mut *mut Dav1dContext, s: *const Dav1dSettings) -> i32;
    pub fn dav1d_send_data(c: *mut Dav1dContext, data: *mut Dav1dData) -> i32;
    pub fn dav1d_get_picture(c: *mut Dav1dContext, out: *mut Dav1dPicture) -> i32;
    pub fn dav1d_picture_unref(p: *mut Dav1dPicture);
    pub fn dav1d_data_create(data: *mut Dav1dData, sz: usize) -> *mut u8;
    pub fn dav1d_data_unref(data: *mut Dav1dData);
    pub fn dav1d_close(c_out: *mut *mut Dav1dContext);
    pub fn dav1d_flush(c: *mut Dav1dContext);
}

/// Safe wrapper around dav1d decoder.
pub struct Dav1dDecoder {
    ctx: *mut Dav1dContext,
}

unsafe impl Send for Dav1dDecoder {}

impl Dav1dDecoder {
    /// Create a new dav1d decoder with default settings.
    #[cfg(has_dav1d)]
    pub fn new() -> Result<Self, String> {
        unsafe {
            let mut settings: Dav1dSettings = std::mem::zeroed();
            dav1d_default_settings(&mut settings);
            settings.n_threads = 2;
            settings.max_frame_delay = 1;

            let mut ctx: *mut Dav1dContext = std::ptr::null_mut();
            let ret = dav1d_open(&mut ctx, &settings);
            if ret < 0 || ctx.is_null() {
                return Err(format!("dav1d_open failed: {}", ret));
            }
            Ok(Dav1dDecoder { ctx })
        }
    }

    #[cfg(not(has_dav1d))]
    pub fn new() -> Result<Self, String> {
        Err("dav1d support disabled: enable makepad-platform feature 'dav1d'".to_string())
    }

    /// Create a new dav1d decoder with a custom picture allocator.
    ///
    /// When the allocator's `alloc_picture_callback` returns -1, dav1d
    /// falls back to its built-in allocator for that picture.
    #[cfg(has_dav1d)]
    pub fn new_with_allocator(allocator: Dav1dPicAllocator) -> Result<Self, String> {
        unsafe {
            let mut settings: Dav1dSettings = std::mem::zeroed();
            dav1d_default_settings(&mut settings);
            settings.n_threads = 2;
            settings.max_frame_delay = 1;
            settings.allocator = allocator;

            let mut ctx: *mut Dav1dContext = std::ptr::null_mut();
            let ret = dav1d_open(&mut ctx, &settings);
            if ret < 0 || ctx.is_null() {
                return Err(format!("dav1d_open failed: {}", ret));
            }
            Ok(Dav1dDecoder { ctx })
        }
    }

    #[cfg(not(has_dav1d))]
    pub fn new_with_allocator(_allocator: Dav1dPicAllocator) -> Result<Self, String> {
        Err("dav1d support disabled: enable makepad-platform feature 'dav1d'".to_string())
    }

    /// Send compressed AV1 data to the decoder.
    /// Returns Ok(true) if consumed, Ok(false) if EAGAIN (need to drain pictures first).
    #[cfg(has_dav1d)]
    pub fn send_data(&mut self, data: &[u8], pts: i64) -> Result<bool, String> {
        unsafe {
            let mut dav1d_data: Dav1dData = std::mem::zeroed();
            let buf = dav1d_data_create(&mut dav1d_data, data.len());
            if buf.is_null() {
                return Err("dav1d_data_create failed".into());
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), buf, data.len());
            dav1d_data.m.timestamp = pts;

            let ret = dav1d_send_data(self.ctx, &mut dav1d_data);
            if ret == DAV1D_ERR_EAGAIN {
                // Data not consumed; caller should drain pictures and retry
                dav1d_data_unref(&mut dav1d_data);
                return Ok(false);
            }
            if ret < 0 {
                dav1d_data_unref(&mut dav1d_data);
                return Err(format!("dav1d_send_data failed: {}", ret));
            }
            Ok(true)
        }
    }

    #[cfg(not(has_dav1d))]
    pub fn send_data(&mut self, _data: &[u8], _pts: i64) -> Result<bool, String> {
        Err("dav1d support disabled: enable makepad-platform feature 'dav1d'".to_string())
    }

    /// Try to get a decoded picture. Returns None if EAGAIN.
    #[cfg(has_dav1d)]
    pub fn get_picture(&mut self) -> Result<Option<DecodedPicture>, String> {
        unsafe {
            let mut pic: Dav1dPicture = std::mem::zeroed();
            let ret = dav1d_get_picture(self.ctx, &mut pic);
            if ret == DAV1D_ERR_EAGAIN {
                return Ok(None);
            }
            if ret < 0 {
                return Err(format!("dav1d_get_picture failed: {}", ret));
            }
            Ok(Some(DecodedPicture { pic }))
        }
    }

    #[cfg(not(has_dav1d))]
    pub fn get_picture(&mut self) -> Result<Option<DecodedPicture>, String> {
        Err("dav1d support disabled: enable makepad-platform feature 'dav1d'".to_string())
    }

    /// Flush the decoder (for seeking).
    #[cfg(has_dav1d)]
    pub fn flush(&mut self) {
        unsafe {
            dav1d_flush(self.ctx);
        }
    }

    #[cfg(not(has_dav1d))]
    pub fn flush(&mut self) {}
}

impl Drop for Dav1dDecoder {
    #[cfg(has_dav1d)]
    fn drop(&mut self) {
        if !self.ctx.is_null() {
            unsafe {
                dav1d_close(&mut self.ctx);
            }
        }
    }

    #[cfg(not(has_dav1d))]
    fn drop(&mut self) {}
}

/// A decoded picture from dav1d. Automatically unrefs on drop.
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

    pub fn layout(&self) -> Dav1dPixelLayout {
        self.pic.p.layout
    }

    pub fn bpc(&self) -> i32 {
        self.pic.p.bpc
    }

    pub fn timestamp(&self) -> i64 {
        self.pic.m.timestamp
    }

    /// Get the color matrix coefficients from the sequence header.
    pub fn matrix_coefficients(&self) -> Dav1dMatrixCoefficients {
        if self.pic.seq_hdr.is_null() {
            return Dav1dMatrixCoefficients::BT709;
        }
        unsafe { (*self.pic.seq_hdr).mtrx }
    }

    /// Whether pixels use full range (JPEG) or limited range (MPEG).
    pub fn is_full_range(&self) -> bool {
        if self.pic.seq_hdr.is_null() {
            return false;
        }
        unsafe { (*self.pic.seq_hdr).color_range != 0 }
    }

    /// Get Y plane data and stride.
    pub fn plane_y(&self) -> (*const u8, isize) {
        (self.pic.data[0] as *const u8, self.pic.stride[0])
    }

    /// Get U plane data and stride.
    pub fn plane_u(&self) -> (*const u8, isize) {
        (self.pic.data[1] as *const u8, self.pic.stride[1])
    }

    /// Get V plane data and stride.
    pub fn plane_v(&self) -> (*const u8, isize) {
        (self.pic.data[2] as *const u8, self.pic.stride[1])
    }
}

impl Drop for DecodedPicture {
    #[cfg(has_dav1d)]
    fn drop(&mut self) {
        unsafe {
            dav1d_picture_unref(&mut self.pic);
        }
    }

    #[cfg(not(has_dav1d))]
    fn drop(&mut self) {}
}
