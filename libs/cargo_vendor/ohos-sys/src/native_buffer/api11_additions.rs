#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use super::OH_NativeBuffer;

pub mod OH_NativeBuffer_ColorSpace {
    /** @brief Indicates the color space of a native buffer.

    @syscap SystemCapability.Graphic.Graphic2D.NativeBuffer
    @since 11
    @version 1.0*/
    pub type Type = ::core::ffi::c_uint;
    /// None color space
    pub const OH_COLORSPACE_NONE: Type = 0;
    /// COLORPRIMARIES_BT601_P | (TRANSFUNC_BT709 << 8) | (MATRIX_BT601_P << 16) | (RANGE_FULL << 21)
    pub const OH_COLORSPACE_BT601_EBU_FULL: Type = 1;
    /// COLORPRIMARIES_BT601_N | (TRANSFUNC_BT709 << 8) | (MATRIX_BT601_N << 16) | (RANGE_FULL << 21)
    pub const OH_COLORSPACE_BT601_SMPTE_C_FULL: Type = 2;
    /// COLORPRIMARIES_BT709 | (TRANSFUNC_BT709 << 8) | (MATRIX_BT709 << 16) | (RANGE_FULL << 21)
    pub const OH_COLORSPACE_BT709_FULL: Type = 3;
    /// COLORPRIMARIES_BT2020 | (TRANSFUNC_HLG << 8) | (MATRIX_BT2020 << 16) | (RANGE_FULL << 21)
    pub const OH_COLORSPACE_BT2020_HLG_FULL: Type = 4;
    /// COLORPRIMARIES_BT2020 | (TRANSFUNC_PQ << 8) | (MATRIX_BT2020 << 16) | (RANGE_FULL << 21)
    pub const OH_COLORSPACE_BT2020_PQ_FULL: Type = 5;
    /// COLORPRIMARIES_BT601_P | (TRANSFUNC_BT709 << 8) | (MATRIX_BT601_P << 16) | (RANGE_LIMITED << 21)
    pub const OH_COLORSPACE_BT601_EBU_LIMIT: Type = 6;
    /// COLORPRIMARIES_BT601_N | (TRANSFUNC_BT709 << 8) | (MATRIX_BT601_N << 16) | (RANGE_LIMITED << 21)
    pub const OH_COLORSPACE_BT601_SMPTE_C_LIMIT: Type = 7;
    /// COLORPRIMARIES_BT709 | (TRANSFUNC_BT709 << 8) | (MATRIX_BT709 << 16) | (RANGE_LIMITED << 21)
    pub const OH_COLORSPACE_BT709_LIMIT: Type = 8;
    /// COLORPRIMARIES_BT2020 | (TRANSFUNC_HLG << 8) | (MATRIX_BT2020 << 16) | (RANGE_LIMITED << 21)
    pub const OH_COLORSPACE_BT2020_HLG_LIMIT: Type = 9;
    /// COLORPRIMARIES_BT2020 | (TRANSFUNC_PQ << 8) | (MATRIX_BT2020 << 16) | (RANGE_LIMITED << 21)
    pub const OH_COLORSPACE_BT2020_PQ_LIMIT: Type = 10;
    /// COLORPRIMARIES_SRGB | (TRANSFUNC_SRGB << 8) | (MATRIX_BT601_N << 16) | (RANGE_FULL << 21)
    pub const OH_COLORSPACE_SRGB_FULL: Type = 11;
    /// COLORPRIMARIES_P3_D65 | (TRANSFUNC_SRGB << 8) | (MATRIX_P3 << 16) | (RANGE_FULL << 21)
    pub const OH_COLORSPACE_P3_FULL: Type = 12;
    /// COLORPRIMARIES_P3_D65 | (TRANSFUNC_HLG << 8) | (MATRIX_P3 << 16) | (RANGE_FULL << 21)
    pub const OH_COLORSPACE_P3_HLG_FULL: Type = 13;
    /// COLORPRIMARIES_P3_D65 | (TRANSFUNC_PQ << 8) | (MATRIX_P3 << 16) | (RANGE_FULL << 21)
    pub const OH_COLORSPACE_P3_PQ_FULL: Type = 14;
    /// COLORPRIMARIES_ADOBERGB | (TRANSFUNC_ADOBERGB << 8) | (MATRIX_ADOBERGB << 16) | (RANGE_FULL << 21)
    pub const OH_COLORSPACE_ADOBERGB_FULL: Type = 15;
    /// COLORPRIMARIES_SRGB | (TRANSFUNC_SRGB << 8) | (MATRIX_BT601_N << 16) | (RANGE_LIMITED << 21)
    pub const OH_COLORSPACE_SRGB_LIMIT: Type = 16;
    /// COLORPRIMARIES_P3_D65 | (TRANSFUNC_SRGB << 8) | (MATRIX_P3 << 16) | (RANGE_LIMITED << 21)
    pub const OH_COLORSPACE_P3_LIMIT: Type = 17;
    /// COLORPRIMARIES_P3_D65 | (TRANSFUNC_HLG << 8) | (MATRIX_P3 << 16) | (RANGE_LIMITED << 21)
    pub const OH_COLORSPACE_P3_HLG_LIMIT: Type = 18;
    /// COLORPRIMARIES_P3_D65 | (TRANSFUNC_PQ << 8) | (MATRIX_P3 << 16) | (RANGE_LIMITED << 21)
    pub const OH_COLORSPACE_P3_PQ_LIMIT: Type = 19;
    /// COLORPRIMARIES_ADOBERGB | (TRANSFUNC_ADOBERGB << 8) | (MATRIX_ADOBERGB << 16) | (RANGE_LIMITED << 21)
    pub const OH_COLORSPACE_ADOBERGB_LIMIT: Type = 20;
    /// COLORPRIMARIES_SRGB | (TRANSFUNC_LINEAR << 8)
    pub const OH_COLORSPACE_LINEAR_SRGB: Type = 21;
    /// equal to OH_COLORSPACE_LINEAR_SRGB
    pub const OH_COLORSPACE_LINEAR_BT709: Type = 22;
    /// COLORPRIMARIES_P3_D65 | (TRANSFUNC_LINEAR << 8)
    pub const OH_COLORSPACE_LINEAR_P3: Type = 23;
    /// COLORPRIMARIES_BT2020 | (TRANSFUNC_LINEAR << 8)
    pub const OH_COLORSPACE_LINEAR_BT2020: Type = 24;
    /// equal to OH_COLORSPACE_SRGB_FULL
    pub const OH_COLORSPACE_DISPLAY_SRGB: Type = 25;
    /// equal to OH_COLORSPACE_P3_FULL
    pub const OH_COLORSPACE_DISPLAY_P3_SRGB: Type = 26;
    /// equal to OH_COLORSPACE_P3_HLG_FULL
    pub const OH_COLORSPACE_DISPLAY_P3_HLG: Type = 27;
    /// equal to OH_COLORSPACE_P3_PQ_FULL
    pub const OH_COLORSPACE_DISPLAY_P3_PQ: Type = 28;
    /// COLORPRIMARIES_BT2020 | (TRANSFUNC_SRGB << 8) | (MATRIX_BT2020 << 16) | (RANGE_FULL << 21)
    pub const OH_COLORSPACE_DISPLAY_BT2020_SRGB: Type = 29;
    /// equal to OH_COLORSPACE_BT2020_HLG_FULL
    pub const OH_COLORSPACE_DISPLAY_BT2020_HLG: Type = 30;
    /// equal to OH_COLORSPACE_BT2020_PQ_FULL
    pub const OH_COLORSPACE_DISPLAY_BT2020_PQ: Type = 31;
}

extern "C" {
    /** @brief Set the color space of the OH_NativeBuffer.

    @syscap SystemCapability.Graphic.Graphic2D.NativeBuffer
    @param buffer Indicates the pointer to a <b>OH_NativeBuffer</b> instance.
    @param colorSpace Indicates the color space of native buffer, see <b>OH_NativeBuffer_ColorSpace</b>.
    @return Returns an error code, 0 is success, otherwise, failed.
    @since 11
    @version 1.0*/
    pub fn OH_NativeBuffer_SetColorSpace(
        buffer: *mut OH_NativeBuffer,
        colorSpace: OH_NativeBuffer_ColorSpace::Type,
    ) -> i32;
}
