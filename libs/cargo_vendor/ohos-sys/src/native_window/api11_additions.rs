#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use super::{OHNativeWindow, OHNativeWindowBuffer};
#[cfg(feature = "native_buffer")]
use crate::native_buffer::OH_NativeBuffer;

#[cfg(not(feature = "native_buffer"))]
#[repr(C)]
pub struct OH_NativeBuffer {
    _unused: [u8; 0],
}

extern "C" {
    /** @brief Creates a <b>OHNativeWindowBuffer</b> instance.
    A new <b>OHNativeWindowBuffer</b> instance is created each time this function is called.

     @syscap SystemCapability.Graphic.Graphic2D.NativeWindow
     @param nativeBuffer Indicates the pointer to a native buffer. The type is <b>OH_NativeBuffer*</b>.
     @return Returns the pointer to the <b>OHNativeWindowBuffer</b> instance created.
     @since 11
     @version 1.0*/
    pub fn OH_NativeWindow_CreateNativeWindowBufferFromNativeBuffer(
        nativeBuffer: *mut OH_NativeBuffer,
    ) -> *mut OHNativeWindowBuffer;
}

extern "C" {
    /** @brief Get the last flushed <b>OHNativeWindowBuffer</b> from a <b>OHNativeWindow</b> instance.

    @syscap SystemCapability.Graphic.Graphic2D.NativeWindow
    @param window Indicates the pointer to a <b>OHNativeWindow</b> instance.
    @param buffer Indicates the pointer to a <b>OHNativeWindowBuffer</b> pointer.
    @param fenceFd Indicates the pointer to a file descriptor handle.
    @param matrix Indicates the retrieved 4*4 transform matrix.
    @return Returns an error code, 0 is success, otherwise, failed.
    @since 11
    @version 1.0*/
    pub fn OH_NativeWindow_GetLastFlushedBuffer(
        window: *mut OHNativeWindow,
        buffer: *mut *mut OHNativeWindowBuffer,
        fenceFd: *mut ::core::ffi::c_int,
        matrix: *mut f32,
    ) -> i32;
}
