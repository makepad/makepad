//! Contains manual bindings
#![allow(non_snake_case)]

/// Indicates the operation code in the function [`OH_NativeWindow_NativeWindowHandleOpt`].
///
/// @since 8
pub mod NativeWindowOperation {

    pub type Type = ::core::ffi::c_uint;
    /** set native window buffer geometry,
    variable parameter in function is
    [in] int32_t width, [in] int32_t height*/
    pub const SET_BUFFER_GEOMETRY: Type = 0;
    /** get native window buffer geometry,
    variable parameter in function is
    [out] int32_t *height, [out] int32_t *width*/
    pub const GET_BUFFER_GEOMETRY: Type = 1;
    /** get native window buffer format,
    variable parameter in function is
    [out] int32_t *format*/
    pub const GET_FORMAT: Type = 2;
    /** set native window buffer format,
    variable parameter in function is
    [in] int32_t format*/
    pub const SET_FORMAT: Type = 3;
    /** get native window buffer usage,
    variable parameter in function is
    [out] int32_t *usage.*/
    pub const GET_USAGE: Type = 4;
    /** set native window buffer usage,
    variable parameter in function is
    [in] int32_t usage.*/
    pub const SET_USAGE: Type = 5;
    /** set native window buffer stride,
    variable parameter in function is
    [in] int32_t stride.*/
    pub const SET_STRIDE: Type = 6;
    /** get native window buffer stride,
    variable parameter in function is
    [out] int32_t *stride.*/
    pub const GET_STRIDE: Type = 7;
    /** set native window buffer swap interval,
    variable parameter in function is
    [in] int32_t interval.*/
    pub const SET_SWAP_INTERVAL: Type = 8;
    /** get native window buffer swap interval,
    variable parameter in function is
    [out] int32_t *interval.*/
    pub const GET_SWAP_INTERVAL: Type = 9;
    /** set native window buffer timeout,
    variable parameter in function is
    [in] int32_t timeout.*/
    pub const SET_TIMEOUT: Type = 10;
    /** get native window buffer timeout,
    variable parameter in function is
    [out] int32_t *timeout.*/
    pub const GET_TIMEOUT: Type = 11;
    /** set native window buffer colorGamut,
    variable parameter in function is
    [in] int32_t colorGamut.*/
    pub const SET_COLOR_GAMUT: Type = 12;
    /** get native window buffer colorGamut,
    variable parameter in function is
    [out int32_t *colorGamut].*/
    pub const GET_COLOR_GAMUT: Type = 13;
    /** set native window buffer transform,
    variable parameter in function is
    [in] int32_t transform.*/
    pub const SET_TRANSFORM: Type = 14;
    /** get native window buffer transform,
    variable parameter in function is
    [out] int32_t *transform.*/
    pub const GET_TRANSFORM: Type = 15;
    /** set native window buffer uiTimestamp,
    variable parameter in function is
    [in] uint64_t uiTimestamp.*/
    pub const SET_UI_TIMESTAMP: Type = 16;
}
