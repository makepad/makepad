/*
 * Copyright (c) 2016, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at www.aomedia.org/license/software. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at www.aomedia.org/license/patent.
 */

#include <tmmintrin.h>
#include "common_dsp_rtcd.h"

#include "convolve.h"
#include "transpose_sse2.h"

// filters for 8_h8 and 16_h8
DECLARE_ALIGNED(32, static const uint8_t, filt_h4[]) = {
    0, 1, 1, 2, 2, 3, 3, 4,  4,  5,  5,  6,  6,  7,  7,  8,  0, 1, 1, 2, 2, 3, 3, 4,  4,  5,  5,  6,  6,  7,  7,  8,
    2, 3, 3, 4, 4, 5, 5, 6,  6,  7,  7,  8,  8,  9,  9,  10, 2, 3, 3, 4, 4, 5, 5, 6,  6,  7,  7,  8,  8,  9,  9,  10,
    4, 5, 5, 6, 6, 7, 7, 8,  8,  9,  9,  10, 10, 11, 11, 12, 4, 5, 5, 6, 6, 7, 7, 8,  8,  9,  9,  10, 10, 11, 11, 12,
    6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13, 14, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13, 14};

DECLARE_ALIGNED(32, static const uint8_t, filtd4[]) = {
    2, 3, 4, 5, 3, 4, 5, 6, 4, 5, 6, 7, 5, 6, 7, 8, 2, 3, 4, 5, 3, 4, 5, 6, 4, 5, 6, 7, 5, 6, 7, 8,
};

typedef void filter8_1dfunction(const uint8_t* src_ptr, ptrdiff_t src_pitch, uint8_t* output_ptr, ptrdiff_t out_pitch,
                                uint32_t output_height, const int16_t* filter);

filter8_1dfunction svt_aom_filter_block1d16_v8_ssse3;
filter8_1dfunction svt_aom_filter_block1d16_h8_ssse3;
filter8_1dfunction svt_aom_filter_block1d8_v8_ssse3;
filter8_1dfunction svt_aom_filter_block1d8_h8_ssse3;
filter8_1dfunction svt_aom_filter_block1d4_v8_ssse3;
filter8_1dfunction svt_aom_filter_block1d4_h8_ssse3;

filter8_1dfunction svt_aom_filter_block1d16_v2_ssse3;
filter8_1dfunction svt_aom_filter_block1d16_h2_ssse3;
filter8_1dfunction svt_aom_filter_block1d8_v2_ssse3;
filter8_1dfunction svt_aom_filter_block1d8_h2_ssse3;
filter8_1dfunction svt_aom_filter_block1d4_v2_ssse3;
filter8_1dfunction svt_aom_filter_block1d4_h2_ssse3;

#define FUN_CONV_1D(name, step_q4, filter, dir, src_start, avg, opt)                                             \
    void svt_aom_convolve8_##name##_##opt(const uint8_t* src,                                                    \
                                          ptrdiff_t      src_stride,                                             \
                                          uint8_t*       dst,                                                    \
                                          ptrdiff_t      dst_stride,                                             \
                                          const int16_t* filter_x,                                               \
                                          int            x_step_q4,                                              \
                                          const int16_t* filter_y,                                               \
                                          int            y_step_q4,                                              \
                                          int            w,                                                      \
                                          int            h) {                                                               \
        (void)filter_x;                                                                                          \
        (void)x_step_q4;                                                                                         \
        (void)filter_y;                                                                                          \
        (void)y_step_q4;                                                                                         \
        assert((-128 <= filter[3]) && (filter[3] <= 127));                                                       \
        assert(step_q4 == 16);                                                                                   \
        if (((filter[0] | filter[1] | filter[6] | filter[7]) == 0) && (filter[2] | filter[5])) {                 \
            while (w >= 16) {                                                                                    \
                svt_aom_filter_block1d16_##dir##4_##avg##opt(src_start, src_stride, dst, dst_stride, h, filter); \
                src += 16;                                                                                       \
                dst += 16;                                                                                       \
                w -= 16;                                                                                         \
            }                                                                                                    \
            while (w >= 8) {                                                                                     \
                svt_aom_filter_block1d8_##dir##4_##avg##opt(src_start, src_stride, dst, dst_stride, h, filter);  \
                src += 8;                                                                                        \
                dst += 8;                                                                                        \
                w -= 8;                                                                                          \
            }                                                                                                    \
            while (w >= 4) {                                                                                     \
                svt_aom_filter_block1d4_##dir##4_##avg##opt(src_start, src_stride, dst, dst_stride, h, filter);  \
                src += 4;                                                                                        \
                dst += 4;                                                                                        \
                w -= 4;                                                                                          \
            }                                                                                                    \
        } else if (filter[0] | filter[1] | filter[2]) {                                                          \
            while (w >= 16) {                                                                                    \
                svt_aom_filter_block1d16_##dir##8_##avg##opt(src_start, src_stride, dst, dst_stride, h, filter); \
                src += 16;                                                                                       \
                dst += 16;                                                                                       \
                w -= 16;                                                                                         \
            }                                                                                                    \
            while (w >= 8) {                                                                                     \
                svt_aom_filter_block1d8_##dir##8_##avg##opt(src_start, src_stride, dst, dst_stride, h, filter);  \
                src += 8;                                                                                        \
                dst += 8;                                                                                        \
                w -= 8;                                                                                          \
            }                                                                                                    \
            while (w >= 4) {                                                                                     \
                svt_aom_filter_block1d4_##dir##8_##avg##opt(src_start, src_stride, dst, dst_stride, h, filter);  \
                src += 4;                                                                                        \
                dst += 4;                                                                                        \
                w -= 4;                                                                                          \
            }                                                                                                    \
        } else {                                                                                                 \
            while (w >= 16) {                                                                                    \
                svt_aom_filter_block1d16_##dir##2_##avg##opt(src, src_stride, dst, dst_stride, h, filter);       \
                src += 16;                                                                                       \
                dst += 16;                                                                                       \
                w -= 16;                                                                                         \
            }                                                                                                    \
            while (w >= 8) {                                                                                     \
                svt_aom_filter_block1d8_##dir##2_##avg##opt(src, src_stride, dst, dst_stride, h, filter);        \
                src += 8;                                                                                        \
                dst += 8;                                                                                        \
                w -= 8;                                                                                          \
            }                                                                                                    \
            while (w >= 4) {                                                                                     \
                svt_aom_filter_block1d4_##dir##2_##avg##opt(src, src_stride, dst, dst_stride, h, filter);        \
                src += 4;                                                                                        \
                dst += 4;                                                                                        \
                w -= 4;                                                                                          \
            }                                                                                                    \
        }                                                                                                        \
        if (w) {                                                                                                 \
            svt_aom_convolve8_##name##_c(                                                                        \
                src, src_stride, dst, dst_stride, filter_x, x_step_q4, filter_y, y_step_q4, w, h);               \
        }                                                                                                        \
    }

static void svt_aom_filter_block1d4_h4_ssse3(const uint8_t* src_ptr, ptrdiff_t src_pixels_per_line, uint8_t* output_ptr,
                                             ptrdiff_t output_pitch, uint32_t output_height, const int16_t* filter) {
    __m128i      filtersReg;
    __m128i      addFilterReg32, filt1Reg, firstFilters, srcReg32b1, srcRegFilt32b1_1;
    unsigned int i;
    src_ptr -= 3;
    addFilterReg32 = _mm_set1_epi16(32);
    filtersReg     = _mm_loadu_si128((const __m128i*)filter);
    filtersReg     = _mm_srai_epi16(filtersReg, 1);
    // converting the 16 bit (short) to 8 bit (byte) and have the same data
    // in both lanes of 128 bit register.
    filtersReg = _mm_packs_epi16(filtersReg, filtersReg);

    firstFilters = _mm_shuffle_epi8(filtersReg, _mm_set1_epi32(0x5040302u));
    filt1Reg     = _mm_loadu_si128((__m128i const*)(filtd4));

    for (i = output_height; i > 0; i -= 1) {
        // load the 2 strides of source
        srcReg32b1 = _mm_loadu_si128((const __m128i*)src_ptr);

        // filter the source buffer
        srcRegFilt32b1_1 = _mm_shuffle_epi8(srcReg32b1, filt1Reg);

        // multiply 4 adjacent elements with the filter and add the result
        srcRegFilt32b1_1 = _mm_maddubs_epi16(srcRegFilt32b1_1, firstFilters);

        srcRegFilt32b1_1 = _mm_hadds_epi16(srcRegFilt32b1_1, _mm_setzero_si128());

        // shift by 6 bit each 16 bit
        srcRegFilt32b1_1 = _mm_adds_epi16(srcRegFilt32b1_1, addFilterReg32);
        srcRegFilt32b1_1 = _mm_srai_epi16(srcRegFilt32b1_1, 6);

        // shrink to 8 bit each 16 bits, the first lane contain the first
        // convolve result and the second lane contain the second convolve result
        srcRegFilt32b1_1 = _mm_packus_epi16(srcRegFilt32b1_1, _mm_setzero_si128());

        src_ptr += src_pixels_per_line;

        *((uint32_t*)(output_ptr)) = _mm_cvtsi128_si32(srcRegFilt32b1_1);
        output_ptr += output_pitch;
    }
}

static void svt_aom_filter_block1d4_v4_ssse3(const uint8_t* src_ptr, ptrdiff_t src_pitch, uint8_t* output_ptr,
                                             ptrdiff_t out_pitch, uint32_t output_height, const int16_t* filter) {
    __m128i      filtersReg;
    __m128i      addFilterReg32;
    __m128i      srcReg2, srcReg3, srcReg23, srcReg4, srcReg34, srcReg5, srcReg45, srcReg6, srcReg56;
    __m128i      srcReg23_34_lo, srcReg45_56_lo;
    __m128i      srcReg2345_3456_lo, srcReg2345_3456_hi;
    __m128i      resReglo, resReghi;
    __m128i      firstFilters;
    unsigned int i;
    ptrdiff_t    src_stride, dst_stride;

    addFilterReg32 = _mm_set1_epi16(32);
    filtersReg     = _mm_loadu_si128((const __m128i*)filter);
    // converting the 16 bit (short) to  8 bit (byte) and have the
    // same data in both lanes of 128 bit register.
    filtersReg = _mm_srai_epi16(filtersReg, 1);
    filtersReg = _mm_packs_epi16(filtersReg, filtersReg);

    firstFilters = _mm_shuffle_epi8(filtersReg, _mm_set1_epi32(0x5040302u));

    // multiple the size of the source and destination stride by two
    src_stride = src_pitch << 1;
    dst_stride = out_pitch << 1;

    srcReg2  = _mm_loadl_epi64((const __m128i*)(src_ptr + src_pitch * 2));
    srcReg3  = _mm_loadl_epi64((const __m128i*)(src_ptr + src_pitch * 3));
    srcReg23 = _mm_unpacklo_epi32(srcReg2, srcReg3);

    srcReg4 = _mm_loadl_epi64((const __m128i*)(src_ptr + src_pitch * 4));

    // have consecutive loads on the same 256 register
    srcReg34 = _mm_unpacklo_epi32(srcReg3, srcReg4);

    srcReg23_34_lo = _mm_unpacklo_epi8(srcReg23, srcReg34);

    for (i = output_height; i > 1; i -= 2) {
        srcReg5  = _mm_loadl_epi64((const __m128i*)(src_ptr + src_pitch * 5));
        srcReg45 = _mm_unpacklo_epi32(srcReg4, srcReg5);

        srcReg6  = _mm_loadl_epi64((const __m128i*)(src_ptr + src_pitch * 6));
        srcReg56 = _mm_unpacklo_epi32(srcReg5, srcReg6);

        // merge every two consecutive registers
        srcReg45_56_lo = _mm_unpacklo_epi8(srcReg45, srcReg56);

        srcReg2345_3456_lo = _mm_unpacklo_epi16(srcReg23_34_lo, srcReg45_56_lo);
        srcReg2345_3456_hi = _mm_unpackhi_epi16(srcReg23_34_lo, srcReg45_56_lo);

        // multiply 2 adjacent elements with the filter and add the result
        resReglo = _mm_maddubs_epi16(srcReg2345_3456_lo, firstFilters);
        resReghi = _mm_maddubs_epi16(srcReg2345_3456_hi, firstFilters);

        resReglo = _mm_hadds_epi16(resReglo, _mm_setzero_si128());
        resReghi = _mm_hadds_epi16(resReghi, _mm_setzero_si128());

        // shift by 6 bit each 16 bit
        resReglo = _mm_adds_epi16(resReglo, addFilterReg32);
        resReghi = _mm_adds_epi16(resReghi, addFilterReg32);
        resReglo = _mm_srai_epi16(resReglo, 6);
        resReghi = _mm_srai_epi16(resReghi, 6);

        // shrink to 8 bit each 16 bits, the first lane contain the first
        // convolve result and the second lane contain the second convolve
        // result
        resReglo = _mm_packus_epi16(resReglo, resReglo);
        resReghi = _mm_packus_epi16(resReghi, resReghi);

        src_ptr += src_stride;

        *((uint32_t*)(output_ptr))             = _mm_cvtsi128_si32(resReglo);
        *((uint32_t*)(output_ptr + out_pitch)) = _mm_cvtsi128_si32(resReghi);

        output_ptr += dst_stride;

        // save part of the registers for next strides
        srcReg23_34_lo = srcReg45_56_lo;
        srcReg4        = srcReg6;
    }
}

static void svt_aom_filter_block1d8_h4_ssse3(const uint8_t* src_ptr, ptrdiff_t src_pixels_per_line, uint8_t* output_ptr,
                                             ptrdiff_t output_pitch, uint32_t output_height, const int16_t* filter) {
    __m128i      filtersReg;
    __m128i      addFilterReg32, filt2Reg, filt3Reg;
    __m128i      secondFilters, thirdFilters;
    __m128i      srcRegFilt32b1_1, srcRegFilt32b2, srcRegFilt32b3;
    __m128i      srcReg32b1;
    unsigned int i;
    src_ptr -= 3;
    addFilterReg32 = _mm_set1_epi16(32);
    filtersReg     = _mm_loadu_si128((const __m128i*)filter);
    filtersReg     = _mm_srai_epi16(filtersReg, 1);
    // converting the 16 bit (short) to 8 bit (byte) and have the same data
    // in both lanes of 128 bit register.
    filtersReg = _mm_packs_epi16(filtersReg, filtersReg);

    // duplicate only the second 16 bits (third and forth byte)
    // across 256 bit register
    secondFilters = _mm_shuffle_epi8(filtersReg, _mm_set1_epi16(0x302u));
    // duplicate only the third 16 bits (fifth and sixth byte)
    // across 256 bit register
    thirdFilters = _mm_shuffle_epi8(filtersReg, _mm_set1_epi16(0x504u));

    filt2Reg = _mm_loadu_si128((__m128i const*)(filt_h4 + 32));
    filt3Reg = _mm_loadu_si128((__m128i const*)(filt_h4 + 32 * 2));

    for (i = output_height; i > 0; i -= 1) {
        srcReg32b1 = _mm_loadu_si128((const __m128i*)src_ptr);

        // filter the source buffer
        srcRegFilt32b3 = _mm_shuffle_epi8(srcReg32b1, filt2Reg);
        srcRegFilt32b2 = _mm_shuffle_epi8(srcReg32b1, filt3Reg);

        // multiply 2 adjacent elements with the filter and add the result
        srcRegFilt32b3 = _mm_maddubs_epi16(srcRegFilt32b3, secondFilters);
        srcRegFilt32b2 = _mm_maddubs_epi16(srcRegFilt32b2, thirdFilters);

        srcRegFilt32b1_1 = _mm_adds_epi16(srcRegFilt32b3, srcRegFilt32b2);

        // shift by 6 bit each 16 bit
        srcRegFilt32b1_1 = _mm_adds_epi16(srcRegFilt32b1_1, addFilterReg32);
        srcRegFilt32b1_1 = _mm_srai_epi16(srcRegFilt32b1_1, 6);

        // shrink to 8 bit each 16 bits
        srcRegFilt32b1_1 = _mm_packus_epi16(srcRegFilt32b1_1, _mm_setzero_si128());

        src_ptr += src_pixels_per_line;

        _mm_storel_epi64((__m128i*)output_ptr, srcRegFilt32b1_1);

        output_ptr += output_pitch;
    }
}

static void svt_aom_filter_block1d8_v4_ssse3(const uint8_t* src_ptr, ptrdiff_t src_pitch, uint8_t* output_ptr,
                                             ptrdiff_t out_pitch, uint32_t output_height, const int16_t* filter) {
    __m128i      filtersReg;
    __m128i      srcReg2, srcReg3, srcReg4, srcReg5, srcReg6;
    __m128i      srcReg23, srcReg34, srcReg45, srcReg56;
    __m128i      resReg23, resReg34, resReg45, resReg56;
    __m128i      resReg23_45, resReg34_56;
    __m128i      addFilterReg32, secondFilters, thirdFilters;
    unsigned int i;
    ptrdiff_t    src_stride, dst_stride;

    addFilterReg32 = _mm_set1_epi16(32);
    filtersReg     = _mm_loadu_si128((const __m128i*)filter);
    // converting the 16 bit (short) to  8 bit (byte) and have the
    // same data in both lanes of 128 bit register.
    filtersReg = _mm_srai_epi16(filtersReg, 1);
    filtersReg = _mm_packs_epi16(filtersReg, filtersReg);

    // duplicate only the second 16 bits (third and forth byte)
    // across 128 bit register
    secondFilters = _mm_shuffle_epi8(filtersReg, _mm_set1_epi16(0x302u));
    // duplicate only the third 16 bits (fifth and sixth byte)
    // across 128 bit register
    thirdFilters = _mm_shuffle_epi8(filtersReg, _mm_set1_epi16(0x504u));

    // multiple the size of the source and destination stride by two
    src_stride = src_pitch << 1;
    dst_stride = out_pitch << 1;

    srcReg2  = _mm_loadl_epi64((const __m128i*)(src_ptr + src_pitch * 2));
    srcReg3  = _mm_loadl_epi64((const __m128i*)(src_ptr + src_pitch * 3));
    srcReg23 = _mm_unpacklo_epi8(srcReg2, srcReg3);

    srcReg4 = _mm_loadl_epi64((const __m128i*)(src_ptr + src_pitch * 4));

    // have consecutive loads on the same 256 register
    srcReg34 = _mm_unpacklo_epi8(srcReg3, srcReg4);

    for (i = output_height; i > 1; i -= 2) {
        srcReg5 = _mm_loadl_epi64((const __m128i*)(src_ptr + src_pitch * 5));

        srcReg45 = _mm_unpacklo_epi8(srcReg4, srcReg5);

        srcReg6 = _mm_loadl_epi64((const __m128i*)(src_ptr + src_pitch * 6));

        srcReg56 = _mm_unpacklo_epi8(srcReg5, srcReg6);

        // multiply 2 adjacent elements with the filter and add the result
        resReg23 = _mm_maddubs_epi16(srcReg23, secondFilters);
        resReg34 = _mm_maddubs_epi16(srcReg34, secondFilters);
        resReg45 = _mm_maddubs_epi16(srcReg45, thirdFilters);
        resReg56 = _mm_maddubs_epi16(srcReg56, thirdFilters);

        // add and saturate the results together
        resReg23_45 = _mm_adds_epi16(resReg23, resReg45);
        resReg34_56 = _mm_adds_epi16(resReg34, resReg56);

        // shift by 6 bit each 16 bit
        resReg23_45 = _mm_adds_epi16(resReg23_45, addFilterReg32);
        resReg34_56 = _mm_adds_epi16(resReg34_56, addFilterReg32);
        resReg23_45 = _mm_srai_epi16(resReg23_45, 6);
        resReg34_56 = _mm_srai_epi16(resReg34_56, 6);

        // shrink to 8 bit each 16 bits, the first lane contain the first
        // convolve result and the second lane contain the second convolve
        // result
        resReg23_45 = _mm_packus_epi16(resReg23_45, _mm_setzero_si128());
        resReg34_56 = _mm_packus_epi16(resReg34_56, _mm_setzero_si128());

        src_ptr += src_stride;

        _mm_storel_epi64((__m128i*)output_ptr, (resReg23_45));
        _mm_storel_epi64((__m128i*)(output_ptr + out_pitch), (resReg34_56));

        output_ptr += dst_stride;

        // save part of the registers for next strides
        srcReg23 = srcReg45;
        srcReg34 = srcReg56;
        srcReg4  = srcReg6;
    }
}

static void svt_aom_filter_block1d16_h4_ssse3(const uint8_t* src_ptr, ptrdiff_t src_pixels_per_line,
                                              uint8_t* output_ptr, ptrdiff_t output_pitch, uint32_t output_height,
                                              const int16_t* filter) {
    __m128i      filtersReg;
    __m128i      addFilterReg32, filt2Reg, filt3Reg;
    __m128i      secondFilters, thirdFilters;
    __m128i      srcRegFilt32b1_1, srcRegFilt32b2_1, srcRegFilt32b2, srcRegFilt32b3;
    __m128i      srcReg32b1, srcReg32b2;
    unsigned int i;
    src_ptr -= 3;
    addFilterReg32 = _mm_set1_epi16(32);
    filtersReg     = _mm_loadu_si128((const __m128i*)filter);
    filtersReg     = _mm_srai_epi16(filtersReg, 1);
    // converting the 16 bit (short) to 8 bit (byte) and have the same data
    // in both lanes of 128 bit register.
    filtersReg = _mm_packs_epi16(filtersReg, filtersReg);

    // duplicate only the second 16 bits (third and forth byte)
    // across 256 bit register
    secondFilters = _mm_shuffle_epi8(filtersReg, _mm_set1_epi16(0x302u));
    // duplicate only the third 16 bits (fifth and sixth byte)
    // across 256 bit register
    thirdFilters = _mm_shuffle_epi8(filtersReg, _mm_set1_epi16(0x504u));

    filt2Reg = _mm_loadu_si128((__m128i const*)(filt_h4 + 32));
    filt3Reg = _mm_loadu_si128((__m128i const*)(filt_h4 + 32 * 2));

    for (i = output_height; i > 0; i -= 1) {
        srcReg32b1 = _mm_loadu_si128((const __m128i*)src_ptr);

        // filter the source buffer
        srcRegFilt32b3 = _mm_shuffle_epi8(srcReg32b1, filt2Reg);
        srcRegFilt32b2 = _mm_shuffle_epi8(srcReg32b1, filt3Reg);

        // multiply 2 adjacent elements with the filter and add the result
        srcRegFilt32b3 = _mm_maddubs_epi16(srcRegFilt32b3, secondFilters);
        srcRegFilt32b2 = _mm_maddubs_epi16(srcRegFilt32b2, thirdFilters);

        srcRegFilt32b1_1 = _mm_adds_epi16(srcRegFilt32b3, srcRegFilt32b2);

        // reading stride of the next 16 bytes
        // (part of it was being read by earlier read)
        srcReg32b2 = _mm_loadu_si128((const __m128i*)(src_ptr + 8));

        // filter the source buffer
        srcRegFilt32b3 = _mm_shuffle_epi8(srcReg32b2, filt2Reg);
        srcRegFilt32b2 = _mm_shuffle_epi8(srcReg32b2, filt3Reg);

        // multiply 2 adjacent elements with the filter and add the result
        srcRegFilt32b3 = _mm_maddubs_epi16(srcRegFilt32b3, secondFilters);
        srcRegFilt32b2 = _mm_maddubs_epi16(srcRegFilt32b2, thirdFilters);

        // add and saturate the results together
        srcRegFilt32b2_1 = _mm_adds_epi16(srcRegFilt32b3, srcRegFilt32b2);

        // shift by 6 bit each 16 bit
        srcRegFilt32b1_1 = _mm_adds_epi16(srcRegFilt32b1_1, addFilterReg32);
        srcRegFilt32b2_1 = _mm_adds_epi16(srcRegFilt32b2_1, addFilterReg32);
        srcRegFilt32b1_1 = _mm_srai_epi16(srcRegFilt32b1_1, 6);
        srcRegFilt32b2_1 = _mm_srai_epi16(srcRegFilt32b2_1, 6);

        // shrink to 8 bit each 16 bits, the first lane contain the first
        // convolve result and the second lane contain the second convolve result
        srcRegFilt32b1_1 = _mm_packus_epi16(srcRegFilt32b1_1, srcRegFilt32b2_1);

        src_ptr += src_pixels_per_line;

        _mm_storeu_si128((__m128i*)output_ptr, srcRegFilt32b1_1);

        output_ptr += output_pitch;
    }
}

static void svt_aom_filter_block1d16_v4_ssse3(const uint8_t* src_ptr, ptrdiff_t src_pitch, uint8_t* output_ptr,
                                              ptrdiff_t out_pitch, uint32_t output_height, const int16_t* filter) {
    __m128i      filtersReg;
    __m128i      srcReg2, srcReg3, srcReg4, srcReg5, srcReg6;
    __m128i      srcReg23_lo, srcReg23_hi, srcReg34_lo, srcReg34_hi;
    __m128i      srcReg45_lo, srcReg45_hi, srcReg56_lo, srcReg56_hi;
    __m128i      resReg23_lo, resReg34_lo, resReg45_lo, resReg56_lo;
    __m128i      resReg23_hi, resReg34_hi, resReg45_hi, resReg56_hi;
    __m128i      resReg23_45_lo, resReg34_56_lo, resReg23_45_hi, resReg34_56_hi;
    __m128i      resReg23_45, resReg34_56;
    __m128i      addFilterReg32, secondFilters, thirdFilters;
    unsigned int i;
    ptrdiff_t    src_stride, dst_stride;

    addFilterReg32 = _mm_set1_epi16(32);
    filtersReg     = _mm_loadu_si128((const __m128i*)filter);
    // converting the 16 bit (short) to  8 bit (byte) and have the
    // same data in both lanes of 128 bit register.
    filtersReg = _mm_srai_epi16(filtersReg, 1);
    filtersReg = _mm_packs_epi16(filtersReg, filtersReg);

    // duplicate only the second 16 bits (third and forth byte)
    // across 128 bit register
    secondFilters = _mm_shuffle_epi8(filtersReg, _mm_set1_epi16(0x302u));
    // duplicate only the third 16 bits (fifth and sixth byte)
    // across 128 bit register
    thirdFilters = _mm_shuffle_epi8(filtersReg, _mm_set1_epi16(0x504u));

    // multiple the size of the source and destination stride by two
    src_stride = src_pitch << 1;
    dst_stride = out_pitch << 1;

    srcReg2     = _mm_loadu_si128((const __m128i*)(src_ptr + src_pitch * 2));
    srcReg3     = _mm_loadu_si128((const __m128i*)(src_ptr + src_pitch * 3));
    srcReg23_lo = _mm_unpacklo_epi8(srcReg2, srcReg3);
    srcReg23_hi = _mm_unpackhi_epi8(srcReg2, srcReg3);

    srcReg4 = _mm_loadu_si128((const __m128i*)(src_ptr + src_pitch * 4));

    // have consecutive loads on the same 256 register
    srcReg34_lo = _mm_unpacklo_epi8(srcReg3, srcReg4);
    srcReg34_hi = _mm_unpackhi_epi8(srcReg3, srcReg4);

    for (i = output_height; i > 1; i -= 2) {
        srcReg5 = _mm_loadu_si128((const __m128i*)(src_ptr + src_pitch * 5));

        srcReg45_lo = _mm_unpacklo_epi8(srcReg4, srcReg5);
        srcReg45_hi = _mm_unpackhi_epi8(srcReg4, srcReg5);

        srcReg6 = _mm_loadu_si128((const __m128i*)(src_ptr + src_pitch * 6));

        srcReg56_lo = _mm_unpacklo_epi8(srcReg5, srcReg6);
        srcReg56_hi = _mm_unpackhi_epi8(srcReg5, srcReg6);

        // multiply 2 adjacent elements with the filter and add the result
        resReg23_lo = _mm_maddubs_epi16(srcReg23_lo, secondFilters);
        resReg34_lo = _mm_maddubs_epi16(srcReg34_lo, secondFilters);
        resReg45_lo = _mm_maddubs_epi16(srcReg45_lo, thirdFilters);
        resReg56_lo = _mm_maddubs_epi16(srcReg56_lo, thirdFilters);

        // add and saturate the results together
        resReg23_45_lo = _mm_adds_epi16(resReg23_lo, resReg45_lo);
        resReg34_56_lo = _mm_adds_epi16(resReg34_lo, resReg56_lo);

        // multiply 2 adjacent elements with the filter and add the result

        resReg23_hi = _mm_maddubs_epi16(srcReg23_hi, secondFilters);
        resReg34_hi = _mm_maddubs_epi16(srcReg34_hi, secondFilters);
        resReg45_hi = _mm_maddubs_epi16(srcReg45_hi, thirdFilters);
        resReg56_hi = _mm_maddubs_epi16(srcReg56_hi, thirdFilters);

        // add and saturate the results together
        resReg23_45_hi = _mm_adds_epi16(resReg23_hi, resReg45_hi);
        resReg34_56_hi = _mm_adds_epi16(resReg34_hi, resReg56_hi);

        // shift by 6 bit each 16 bit
        resReg23_45_lo = _mm_adds_epi16(resReg23_45_lo, addFilterReg32);
        resReg34_56_lo = _mm_adds_epi16(resReg34_56_lo, addFilterReg32);
        resReg23_45_hi = _mm_adds_epi16(resReg23_45_hi, addFilterReg32);
        resReg34_56_hi = _mm_adds_epi16(resReg34_56_hi, addFilterReg32);
        resReg23_45_lo = _mm_srai_epi16(resReg23_45_lo, 6);
        resReg34_56_lo = _mm_srai_epi16(resReg34_56_lo, 6);
        resReg23_45_hi = _mm_srai_epi16(resReg23_45_hi, 6);
        resReg34_56_hi = _mm_srai_epi16(resReg34_56_hi, 6);

        // shrink to 8 bit each 16 bits, the first lane contain the first
        // convolve result and the second lane contain the second convolve
        // result
        resReg23_45 = _mm_packus_epi16(resReg23_45_lo, resReg23_45_hi);
        resReg34_56 = _mm_packus_epi16(resReg34_56_lo, resReg34_56_hi);

        src_ptr += src_stride;

        _mm_storeu_si128((__m128i*)output_ptr, (resReg23_45));
        _mm_storeu_si128((__m128i*)(output_ptr + out_pitch), (resReg34_56));

        output_ptr += dst_stride;

        // save part of the registers for next strides
        srcReg23_lo = srcReg45_lo;
        srcReg34_lo = srcReg56_lo;
        srcReg23_hi = srcReg45_hi;
        srcReg34_hi = srcReg56_hi;
        srcReg4     = srcReg6;
    }
}

FUN_CONV_1D(horiz, x_step_q4, filter_x, h, src, , ssse3);
FUN_CONV_1D(vert, y_step_q4, filter_y, v, src - src_stride * 3, , ssse3);
