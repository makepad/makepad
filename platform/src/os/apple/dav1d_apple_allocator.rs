//! Custom dav1d picture allocator that backs the Y plane with
//! CVPixelBuffer/IOSurface memory for zero-copy Metal texture wrapping.
//!
//! The allocator creates a biplanar NV12 CVPixelBuffer
//! (`kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange`) for each decoded
//! picture. Plane 0 is Y (R8), plane 1 is UV interleaved (RG8).
//!
//! dav1d writes separate U and V planes into temporary heap buffers.
//! After decoding, call `finalize_nv12` to interleave U/V into the
//! CVPixelBuffer UV plane, then wrap both planes as Metal textures via
//! `CVMetalTextureCacheCreateTextureFromImage` — zero memcpy for Y,
//! one interleave pass for UV.

use {
    super::apple_sys::*,
    crate::video_decode::dav1d_ffi::*,
    std::ffi::c_void,
    std::ptr,
};

/// NV12 biplanar 4:2:0 (video range).
const NV12_VIDEO_RANGE: u32 = 0x34323076; // '420v'

/// Per-picture allocation context stored in `Dav1dPicture.allocator_data`.
pub struct PicAlloc {
    pub pixel_buffer: CVPixelBufferRef,
    /// Whether U/V have been interleaved into the NV12 UV plane.
    pub finalized: bool,
}

/// Create a `Dav1dPicAllocator` for CVPixelBuffer-backed allocation.
pub fn create_cv_pic_allocator() -> Dav1dPicAllocator {
    Dav1dPicAllocator {
        cookie: ptr::null_mut(),
        alloc_picture_callback: Some(alloc_picture_callback),
        release_picture_callback: Some(release_picture_callback),
    }
}

/// Interleave the separate U/V heap buffers into the CVPixelBuffer NV12
/// UV plane. Must be called after decoding and before creating Metal
/// textures from the CVPixelBuffer.
///
/// Returns the CVPixelBufferRef (retained — caller must release) or null
/// if this picture was not allocated by our custom allocator.
pub unsafe fn finalize_nv12(pic: &mut Dav1dPicture) -> CVPixelBufferRef {
    if pic.allocator_data.is_null() {
        return ptr::null_mut();
    }
    let alloc = &mut *(pic.allocator_data as *mut PicAlloc);
    if alloc.finalized {
        CVPixelBufferRetain(alloc.pixel_buffer);
        return alloc.pixel_buffer;
    }

    let pixel_buffer = alloc.pixel_buffer;
    let w = pic.p.w as usize;
    let h = pic.p.h as usize;
    let chroma_w = (w + 1) / 2;
    let chroma_h = (h + 1) / 2;

    let uv_base = CVPixelBufferGetBaseAddressOfPlane(pixel_buffer, 1);
    let uv_stride = CVPixelBufferGetBytesPerRowOfPlane(pixel_buffer, 1);

    let u_ptr = pic.data[1] as *const u8;
    let v_ptr = pic.data[2] as *const u8;
    let u_stride = pic.stride[1] as usize;

    for row in 0..chroma_h {
        let uv_row = (uv_base as *mut u8).add(row * uv_stride);
        let u_row = u_ptr.add(row * u_stride);
        let v_row = v_ptr.add(row * u_stride);
        for col in 0..chroma_w {
            *uv_row.add(col * 2) = *u_row.add(col);
            *uv_row.add(col * 2 + 1) = *v_row.add(col);
        }
    }

    alloc.finalized = true;

    // Retain so the caller can hold a reference after picture unref
    CVPixelBufferRetain(pixel_buffer);
    pixel_buffer
}

unsafe extern "C" fn alloc_picture_callback(
    pic: *mut Dav1dPicture,
    _cookie: *mut c_void,
) -> i32 {
    let pic = &mut *pic;
    let w = pic.p.w as usize;
    let h = pic.p.h as usize;
    let layout = pic.p.layout;
    let bpc = pic.p.bpc;

    // Only 8-bit 4:2:0 for NV12 zero-copy path.
    if bpc != 8 || layout != Dav1dPixelLayout::I420 {
        return -1;
    }

    let chroma_w = (w + 1) / 2;
    let chroma_h = (h + 1) / 2;

    // Dictionary: Metal + IOSurface compatible
    let keys: [*const c_void; 2] = [
        kCVPixelBufferMetalCompatibilityKey as *const c_void,
        kCVPixelBufferIOSurfacePropertiesKey as *const c_void,
    ];
    let true_val: ObjcId = msg_send![class!(NSNumber), numberWithBool: true];
    let empty_dict: ObjcId = msg_send![class!(NSDictionary), dictionary];
    let values: [*const c_void; 2] = [
        true_val as *const c_void,
        empty_dict as *const c_void,
    ];
    let attrs: ObjcId = msg_send![class!(NSDictionary),
        dictionaryWithObjects: values.as_ptr()
        forKeys: keys.as_ptr()
        count: 2usize
    ];

    let mut pixel_buffer: CVPixelBufferRef = ptr::null_mut();
    let ret = CVPixelBufferCreate(
        ptr::null(),
        w,
        h,
        NV12_VIDEO_RANGE,
        attrs as *const c_void,
        &mut pixel_buffer,
    );

    if ret != 0 || pixel_buffer.is_null() {
        return -1;
    }

    // Lock for CPU write
    let lock_ret = CVPixelBufferLockBaseAddress(pixel_buffer, 0);
    if lock_ret != 0 {
        CVPixelBufferRelease(pixel_buffer);
        return -1;
    }

    // Y plane: direct into CVPixelBuffer plane 0
    let y_base = CVPixelBufferGetBaseAddressOfPlane(pixel_buffer, 0);
    let y_stride = CVPixelBufferGetBytesPerRowOfPlane(pixel_buffer, 0) as isize;

    // U and V: separate heap buffers (interleaved later by finalize_nv12)
    let u_buf = libc_alloc(chroma_w * chroma_h);
    let v_buf = libc_alloc(chroma_w * chroma_h);
    if u_buf.is_null() || v_buf.is_null() {
        if !u_buf.is_null() { libc_free(u_buf); }
        if !v_buf.is_null() { libc_free(v_buf); }
        CVPixelBufferUnlockBaseAddress(pixel_buffer, 0);
        CVPixelBufferRelease(pixel_buffer);
        return -1;
    }

    pic.data[0] = y_base;
    pic.data[1] = u_buf;
    pic.data[2] = v_buf;
    pic.stride[0] = y_stride;
    pic.stride[1] = chroma_w as isize;

    let alloc_ctx = Box::new(PicAlloc {
        pixel_buffer,
        finalized: false,
    });
    pic.allocator_data = Box::into_raw(alloc_ctx) as *mut c_void;

    0
}

unsafe extern "C" fn release_picture_callback(
    pic: *mut Dav1dPicture,
    _cookie: *mut c_void,
) {
    let pic = &mut *pic;
    if pic.allocator_data.is_null() {
        return;
    }

    let alloc = Box::from_raw(pic.allocator_data as *mut PicAlloc);

    // Free heap U/V buffers
    libc_free(pic.data[1]);
    libc_free(pic.data[2]);

    // Unlock and release CVPixelBuffer
    CVPixelBufferUnlockBaseAddress(alloc.pixel_buffer, 0);
    CVPixelBufferRelease(alloc.pixel_buffer);

    pic.allocator_data = ptr::null_mut();
}

unsafe fn libc_alloc(size: usize) -> *mut c_void {
    extern "C" {
        fn malloc(size: usize) -> *mut c_void;
    }
    malloc(size)
}

unsafe fn libc_free(ptr: *mut c_void) {
    extern "C" {
        fn free(ptr: *mut c_void);
    }
    free(ptr)
}
