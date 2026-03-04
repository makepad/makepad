/*
 * Copyright (c) 2016, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
 */

#include <immintrin.h>

#include "common_dsp_rtcd.h"

#include "convolve.h"
#include "convolve_avx2.h"
// #include "aom_ports/mem.h"

#if defined(__clang__)
#if (__clang_major__ > 0 && __clang_major__ < 3) || (__clang_major__ == 3 && __clang_minor__ <= 3) || \
    (defined(__APPLE__) && defined(__apple_build_version__) &&                                        \
     ((__clang_major__ == 4 && __clang_minor__ <= 2) || (__clang_major__ == 5 && __clang_minor__ == 0)))
#define MM256_BROADCASTSI128_SI256(x) _mm_broadcastsi128_si256((__m128i const*)&(x))
#else // clang > 3.3, and not 5.0 on macosx.
#define MM256_BROADCASTSI128_SI256(x) _mm256_broadcastsi128_si256(x)
#endif // clang <= 3.3
#elif defined(__GNUC__)
#if __GNUC__ < 4 || (__GNUC__ == 4 && __GNUC_MINOR__ <= 6)
#define MM256_BROADCASTSI128_SI256(x) _mm_broadcastsi128_si256((__m128i const*)&(x))
#elif __GNUC__ == 4 && __GNUC_MINOR__ == 7
#define MM256_BROADCASTSI128_SI256(x) _mm_broadcastsi128_si256(x)
#else // gcc > 4.7
#define MM256_BROADCASTSI128_SI256(x) _mm256_broadcastsi128_si256(x)
#endif // gcc <= 4.6
#else // !(gcc || clang)
#define MM256_BROADCASTSI128_SI256(x) _mm256_broadcastsi128_si256(x)
#endif // __clang__

typedef void Filter81dFunction(const uint8_t* src_ptr, ptrdiff_t src_pitch, uint8_t* output_ptr, ptrdiff_t out_pitch,
                               uint32_t output_height, const int16_t* filter);
void         svt_aom_filter_block1d4_v8_sse2(const uint8_t* src_ptr, ptrdiff_t src_pitch, uint8_t* output_ptr,
                                             ptrdiff_t out_pitch, uint32_t output_height, const int16_t* filter);
void         svt_aom_filter_block1d16_v2_ssse3(const uint8_t* src_ptr, ptrdiff_t src_pitch, uint8_t* output_ptr,
                                               ptrdiff_t out_pitch, uint32_t output_height, const int16_t* filter);
void         svt_aom_filter_block1d16_h2_ssse3(const uint8_t* src_ptr, ptrdiff_t src_pitch, uint8_t* output_ptr,
                                               ptrdiff_t out_pitch, uint32_t output_height, const int16_t* filter);
void         svt_aom_filter_block1d8_v2_ssse3(const uint8_t* src_ptr, ptrdiff_t src_pitch, uint8_t* output_ptr,
                                              ptrdiff_t out_pitch, uint32_t output_height, const int16_t* filter);
void         svt_aom_filter_block1d8_h2_ssse3(const uint8_t* src_ptr, ptrdiff_t src_pitch, uint8_t* output_ptr,
                                              ptrdiff_t out_pitch, uint32_t output_height, const int16_t* filter);
void         svt_aom_filter_block1d4_v2_ssse3(const uint8_t* src_ptr, ptrdiff_t src_pitch, uint8_t* output_ptr,
                                              ptrdiff_t out_pitch, uint32_t output_height, const int16_t* filter);
void         svt_aom_filter_block1d4_h2_ssse3(const uint8_t* src_ptr, ptrdiff_t src_pitch, uint8_t* output_ptr,
                                              ptrdiff_t out_pitch, uint32_t output_height, const int16_t* filter);

Filter81dFunction svt_aom_filter_block1d4_v8_ssse3;
Filter81dFunction svt_aom_filter_block1d16_v2_ssse3;
Filter81dFunction svt_aom_filter_block1d16_h2_ssse3;
Filter81dFunction svt_aom_filter_block1d8_v2_ssse3;
Filter81dFunction svt_aom_filter_block1d8_h2_ssse3;
Filter81dFunction svt_aom_filter_block1d4_v2_ssse3;
Filter81dFunction svt_aom_filter_block1d4_h2_ssse3;
#define svt_aom_filter_block1d4_v8_avx2 svt_aom_filter_block1d4_v8_sse2
#define svt_aom_filter_block1d16_v2_avx2 svt_aom_filter_block1d16_v2_ssse3
#define svt_aom_filter_block1d16_h2_avx2 svt_aom_filter_block1d16_h2_ssse3
#define svt_aom_filter_block1d8_v2_avx2 svt_aom_filter_block1d8_v2_ssse3
#define svt_aom_filter_block1d8_h2_avx2 svt_aom_filter_block1d8_h2_ssse3
#define svt_aom_filter_block1d4_v2_avx2 svt_aom_filter_block1d4_v2_ssse3
#define svt_aom_filter_block1d4_h2_avx2 svt_aom_filter_block1d4_h2_ssse3

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

// filters for 16
DECLARE_ALIGNED(32, static const uint8_t, filt_global_avx2[]) = {
    0, 1, 1, 2, 2, 3, 3, 4,  4,  5,  5,  6,  6,  7,  7,  8,  0, 1, 1, 2, 2, 3, 3, 4,  4,  5,  5,  6,  6,  7,  7,  8,
    2, 3, 3, 4, 4, 5, 5, 6,  6,  7,  7,  8,  8,  9,  9,  10, 2, 3, 3, 4, 4, 5, 5, 6,  6,  7,  7,  8,  8,  9,  9,  10,
    4, 5, 5, 6, 6, 7, 7, 8,  8,  9,  9,  10, 10, 11, 11, 12, 4, 5, 5, 6, 6, 7, 7, 8,  8,  9,  9,  10, 10, 11, 11, 12,
    6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13, 14, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13, 14};

DECLARE_ALIGNED(32, static const uint8_t, filt_d4_global_avx2[]) = {
    0, 1, 2, 3, 1, 2, 3, 4, 2, 3, 4, 5, 3, 4, 5, 6,  0, 1, 2, 3, 1, 2, 3, 4, 2, 3, 4, 5, 3, 4, 5, 6,
    4, 5, 6, 7, 5, 6, 7, 8, 6, 7, 8, 9, 7, 8, 9, 10, 4, 5, 6, 7, 5, 6, 7, 8, 6, 7, 8, 9, 7, 8, 9, 10,
};

DECLARE_ALIGNED(32, static const uint8_t, filt4_d4_global_avx2[]) = {
    2, 3, 4, 5, 3, 4, 5, 6, 4, 5, 6, 7, 5, 6, 7, 8, 2, 3, 4, 5, 3, 4, 5, 6, 4, 5, 6, 7, 5, 6, 7, 8,
};

static INLINE void xx_storeu2_epi32(const uint8_t* output_ptr, const ptrdiff_t stride, const __m256i* a) {
    *((uint32_t*)(output_ptr))          = _mm_cvtsi128_si32(_mm256_castsi256_si128(*a));
    *((uint32_t*)(output_ptr + stride)) = _mm_cvtsi128_si32(_mm256_extracti128_si256(*a, 1));
}

static INLINE __m256i xx_loadu2_epi64(const void* hi, const void* lo) {
    __m256i a = _mm256_castsi128_si256(_mm_loadl_epi64((const __m128i*)(lo)));
    a         = _mm256_inserti128_si256(a, _mm_loadl_epi64((const __m128i*)(hi)), 1);
    return a;
}

static INLINE void xx_storeu2_epi64(const uint8_t* output_ptr, const ptrdiff_t stride, const __m256i* a) {
    _mm_storel_epi64((__m128i*)output_ptr, _mm256_castsi256_si128(*a));
    _mm_storel_epi64((__m128i*)(output_ptr + stride), _mm256_extractf128_si256(*a, 1));
}

static INLINE __m256i xx_loadu2_mi128(const void* hi, const void* lo) {
    __m256i a = _mm256_castsi128_si256(_mm_loadu_si128((const __m128i*)(lo)));
    a         = _mm256_inserti128_si256(a, _mm_loadu_si128((const __m128i*)(hi)), 1);
    return a;
}

static INLINE void xx_store2_mi128(const uint8_t* output_ptr, const ptrdiff_t stride, const __m256i* a) {
    _mm_storeu_si128((__m128i*)output_ptr, _mm256_castsi256_si128(*a));
    _mm_storeu_si128((__m128i*)(output_ptr + stride), _mm256_extractf128_si256(*a, 1));
}

static void svt_aom_filter_block1d4_h4_avx2(const uint8_t* src_ptr, ptrdiff_t src_pixels_per_line, uint8_t* output_ptr,
                                            ptrdiff_t output_pitch, uint32_t output_height, const int16_t* filter) {
    __m128i      filters_reg;
    __m256i      add_filter_reg32, filt1_reg, first_filters, src_reg32b1, src_reg_filt32b1_1;
    unsigned int i;
    ptrdiff_t    src_stride, dst_stride;
    src_ptr -= 3;
    add_filter_reg32 = _mm256_set1_epi16(32);
    filters_reg      = _mm_loadu_si128((const __m128i*)filter);
    filters_reg      = _mm_srai_epi16(filters_reg, 1);
    // converting the 16 bit (short) to 8 bit (byte) and have the same data
    // in both lanes of 128 bit register.
    filters_reg = _mm_packs_epi16(filters_reg, filters_reg);
    // have the same data in both lanes of a 256 bit register
    const __m256i filters_reg32 = MM256_BROADCASTSI128_SI256(filters_reg);

    first_filters = _mm256_shuffle_epi8(filters_reg32, _mm256_set1_epi32(0x5040302u));
    filt1_reg     = _mm256_loadu_si256((__m256i const*)(filt4_d4_global_avx2));

    // multiple the size of the source and destination stride by two
    src_stride = src_pixels_per_line << 1;
    dst_stride = output_pitch << 1;
    for (i = output_height; i > 1; i -= 2) {
        // load the 2 strides of source
        src_reg32b1 = xx_loadu2_mi128(src_ptr + src_pixels_per_line, src_ptr);

        // filter the source buffer
        src_reg_filt32b1_1 = _mm256_shuffle_epi8(src_reg32b1, filt1_reg);

        // multiply 4 adjacent elements with the filter and add the result
        src_reg_filt32b1_1 = _mm256_maddubs_epi16(src_reg_filt32b1_1, first_filters);

        src_reg_filt32b1_1 = _mm256_hadds_epi16(src_reg_filt32b1_1, _mm256_setzero_si256());

        // shift by 6 bit each 16 bit
        src_reg_filt32b1_1 = _mm256_adds_epi16(src_reg_filt32b1_1, add_filter_reg32);
        src_reg_filt32b1_1 = _mm256_srai_epi16(src_reg_filt32b1_1, 6);

        // shrink to 8 bit each 16 bits, the first lane contain the first
        // convolve result and the second lane contain the second convolve result
        src_reg_filt32b1_1 = _mm256_packus_epi16(src_reg_filt32b1_1, _mm256_setzero_si256());

        src_ptr += src_stride;

        xx_storeu2_epi32(output_ptr, output_pitch, &src_reg_filt32b1_1);
        output_ptr += dst_stride;
    }

    // if the number of strides is odd.
    // process only 4 bytes
    if (i > 0) {
        __m128i src_reg1, src_reg_filt1_1;

        src_reg1 = _mm_loadu_si128((const __m128i*)(src_ptr));

        // filter the source buffer
        src_reg_filt1_1 = _mm_shuffle_epi8(src_reg1, _mm256_castsi256_si128(filt1_reg));

        // multiply 4 adjacent elements with the filter and add the result
        src_reg_filt1_1 = _mm_maddubs_epi16(src_reg_filt1_1, _mm256_castsi256_si128(first_filters));

        src_reg_filt1_1 = _mm_hadds_epi16(src_reg_filt1_1, _mm_setzero_si128());
        // shift by 6 bit each 16 bit
        src_reg_filt1_1 = _mm_adds_epi16(src_reg_filt1_1, _mm256_castsi256_si128(add_filter_reg32));
        src_reg_filt1_1 = _mm_srai_epi16(src_reg_filt1_1, 6);

        // shrink to 8 bit each 16 bits, the first lane contain the first
        // convolve result and the second lane contain the second convolve result
        src_reg_filt1_1 = _mm_packus_epi16(src_reg_filt1_1, _mm_setzero_si128());

        // save 4 bytes
        *((uint32_t*)(output_ptr)) = _mm_cvtsi128_si32(src_reg_filt1_1);
    }
}

static void svt_aom_filter_block1d4_h8_avx2(const uint8_t* src_ptr, ptrdiff_t src_pixels_per_line, uint8_t* output_ptr,
                                            ptrdiff_t output_pitch, uint32_t output_height, const int16_t* filter) {
    __m128i      filters_reg;
    __m256i      add_filter_reg32, filt1_reg, filt2_reg;
    __m256i      first_filters, second_filters;
    __m256i      src_reg_filt32b1_1, src_reg_Filt32b2;
    __m256i      src_reg32b1;
    unsigned int i;
    ptrdiff_t    src_stride, dst_stride;
    src_ptr -= 3;
    add_filter_reg32 = _mm256_set1_epi16(32);
    filters_reg      = _mm_loadu_si128((const __m128i*)filter);
    filters_reg      = _mm_srai_epi16(filters_reg, 1);
    // converting the 16 bit (short) to 8 bit (byte) and have the same data
    // in both lanes of 128 bit register.
    filters_reg = _mm_packs_epi16(filters_reg, filters_reg);
    // have the same data in both lanes of a 256 bit register
    const __m256i filters_reg32 = MM256_BROADCASTSI128_SI256(filters_reg);

    // duplicate only the first 32 bits
    first_filters = _mm256_shuffle_epi32(filters_reg32, 0);
    // duplicate only the second 32 bits
    second_filters = _mm256_shuffle_epi32(filters_reg32, 0x55);

    filt1_reg = _mm256_loadu_si256((__m256i const*)filt_d4_global_avx2);
    filt2_reg = _mm256_loadu_si256((__m256i const*)(filt_d4_global_avx2 + 32));

    // multiple the size of the source and destination stride by two
    src_stride = src_pixels_per_line << 1;
    dst_stride = output_pitch << 1;
    for (i = output_height; i > 1; i -= 2) {
        // load the 2 strides of source
        src_reg32b1 = xx_loadu2_mi128(src_ptr + src_pixels_per_line, src_ptr);

        // filter the source buffer
        src_reg_filt32b1_1 = _mm256_shuffle_epi8(src_reg32b1, filt1_reg);

        // multiply 4 adjacent elements with the filter and add the result
        src_reg_filt32b1_1 = _mm256_maddubs_epi16(src_reg_filt32b1_1, first_filters);

        // filter the source buffer
        src_reg_Filt32b2 = _mm256_shuffle_epi8(src_reg32b1, filt2_reg);

        // multiply 4 adjacent elements with the filter and add the result
        src_reg_Filt32b2 = _mm256_maddubs_epi16(src_reg_Filt32b2, second_filters);

        src_reg_filt32b1_1 = _mm256_adds_epi16(src_reg_filt32b1_1, src_reg_Filt32b2);

        src_reg_filt32b1_1 = _mm256_hadds_epi16(src_reg_filt32b1_1, _mm256_setzero_si256());

        // shift by 6 bit each 16 bit
        src_reg_filt32b1_1 = _mm256_adds_epi16(src_reg_filt32b1_1, add_filter_reg32);
        src_reg_filt32b1_1 = _mm256_srai_epi16(src_reg_filt32b1_1, 6);

        // shrink to 8 bit each 16 bits, the first lane contain the first
        // convolve result and the second lane contain the second convolve result
        src_reg_filt32b1_1 = _mm256_packus_epi16(src_reg_filt32b1_1, _mm256_setzero_si256());

        src_ptr += src_stride;

        xx_storeu2_epi32(output_ptr, output_pitch, &src_reg_filt32b1_1);
        output_ptr += dst_stride;
    }

    // if the number of strides is odd.
    // process only 4 bytes
    if (i > 0) {
        __m128i src_reg1, src_reg_filt1_1;
        __m128i src_reg_filt2;

        src_reg1 = _mm_loadu_si128((const __m128i*)(src_ptr));

        // filter the source buffer
        src_reg_filt1_1 = _mm_shuffle_epi8(src_reg1, _mm256_castsi256_si128(filt1_reg));

        // multiply 4 adjacent elements with the filter and add the result
        src_reg_filt1_1 = _mm_maddubs_epi16(src_reg_filt1_1, _mm256_castsi256_si128(first_filters));

        // filter the source buffer
        src_reg_filt2 = _mm_shuffle_epi8(src_reg1, _mm256_castsi256_si128(filt2_reg));

        // multiply 4 adjacent elements with the filter and add the result
        src_reg_filt2 = _mm_maddubs_epi16(src_reg_filt2, _mm256_castsi256_si128(second_filters));

        src_reg_filt1_1 = _mm_adds_epi16(src_reg_filt1_1, src_reg_filt2);
        src_reg_filt1_1 = _mm_hadds_epi16(src_reg_filt1_1, _mm_setzero_si128());
        // shift by 6 bit each 16 bit
        src_reg_filt1_1 = _mm_adds_epi16(src_reg_filt1_1, _mm256_castsi256_si128(add_filter_reg32));
        src_reg_filt1_1 = _mm_srai_epi16(src_reg_filt1_1, 6);

        // shrink to 8 bit each 16 bits, the first lane contain the first
        // convolve result and the second lane contain the second convolve result
        src_reg_filt1_1 = _mm_packus_epi16(src_reg_filt1_1, _mm_setzero_si128());

        // save 4 bytes
        *((uint32_t*)(output_ptr)) = _mm_cvtsi128_si32(src_reg_filt1_1);
    }
}

static void svt_aom_filter_block1d8_h4_avx2(const uint8_t* src_ptr, ptrdiff_t src_pixels_per_line, uint8_t* output_ptr,
                                            ptrdiff_t output_pitch, uint32_t output_height, const int16_t* filter) {
    __m128i      filters_reg;
    __m256i      add_filter_reg32, filt2_reg, filt3_reg;
    __m256i      second_filters, third_filters;
    __m256i      src_reg_filt32b1_1, src_reg_Filt32b2, src_reg_Filt32b3;
    __m256i      src_reg32b1, filters_reg32;
    unsigned int i;
    ptrdiff_t    src_stride, dst_stride;
    src_ptr -= 3;
    add_filter_reg32 = _mm256_set1_epi16(32);
    filters_reg      = _mm_loadu_si128((const __m128i*)filter);
    filters_reg      = _mm_srai_epi16(filters_reg, 1);
    // converting the 16 bit (short) to 8 bit (byte) and have the same data
    // in both lanes of 128 bit register.
    filters_reg = _mm_packs_epi16(filters_reg, filters_reg);
    // have the same data in both lanes of a 256 bit register
    filters_reg32 = MM256_BROADCASTSI128_SI256(filters_reg);

    // duplicate only the second 16 bits (third and forth byte)
    // across 256 bit register
    second_filters = _mm256_shuffle_epi8(filters_reg32, _mm256_set1_epi16(0x302u));
    // duplicate only the third 16 bits (fifth and sixth byte)
    // across 256 bit register
    third_filters = _mm256_shuffle_epi8(filters_reg32, _mm256_set1_epi16(0x504u));

    filt2_reg = _mm256_loadu_si256((__m256i const*)(filt_global_avx2 + 32));
    filt3_reg = _mm256_loadu_si256((__m256i const*)(filt_global_avx2 + 32 * 2));

    // multiply the size of the source and destination stride by two
    src_stride = src_pixels_per_line << 1;
    dst_stride = output_pitch << 1;
    for (i = output_height; i > 1; i -= 2) {
        // load the 2 strides of source
        src_reg32b1 = xx_loadu2_mi128(src_ptr + src_pixels_per_line, src_ptr);

        // filter the source buffer
        src_reg_Filt32b3 = _mm256_shuffle_epi8(src_reg32b1, filt2_reg);
        src_reg_Filt32b2 = _mm256_shuffle_epi8(src_reg32b1, filt3_reg);

        // multiply 2 adjacent elements with the filter and add the result
        src_reg_Filt32b3 = _mm256_maddubs_epi16(src_reg_Filt32b3, second_filters);
        src_reg_Filt32b2 = _mm256_maddubs_epi16(src_reg_Filt32b2, third_filters);

        src_reg_filt32b1_1 = _mm256_adds_epi16(src_reg_Filt32b3, src_reg_Filt32b2);

        // shift by 6 bit each 16 bit
        src_reg_filt32b1_1 = _mm256_adds_epi16(src_reg_filt32b1_1, add_filter_reg32);
        src_reg_filt32b1_1 = _mm256_srai_epi16(src_reg_filt32b1_1, 6);

        // shrink to 8 bit each 16 bits
        src_reg_filt32b1_1 = _mm256_packus_epi16(src_reg_filt32b1_1, src_reg_filt32b1_1);

        src_ptr += src_stride;

        xx_storeu2_epi64(output_ptr, output_pitch, &src_reg_filt32b1_1);
        output_ptr += dst_stride;
    }

    // if the number of strides is odd.
    // process only 8 bytes
    if (i > 0) {
        __m128i src_reg1, src_reg_filt1_1;
        __m128i src_reg_filt2, src_reg_filt3;

        src_reg1 = _mm_loadu_si128((const __m128i*)(src_ptr));

        // filter the source buffer
        src_reg_filt2 = _mm_shuffle_epi8(src_reg1, _mm256_castsi256_si128(filt2_reg));
        src_reg_filt3 = _mm_shuffle_epi8(src_reg1, _mm256_castsi256_si128(filt3_reg));

        // multiply 2 adjacent elements with the filter and add the result
        src_reg_filt2 = _mm_maddubs_epi16(src_reg_filt2, _mm256_castsi256_si128(second_filters));
        src_reg_filt3 = _mm_maddubs_epi16(src_reg_filt3, _mm256_castsi256_si128(third_filters));

        // add and saturate the results together
        src_reg_filt1_1 = _mm_adds_epi16(src_reg_filt2, src_reg_filt3);

        // shift by 6 bit each 16 bit
        src_reg_filt1_1 = _mm_adds_epi16(src_reg_filt1_1, _mm256_castsi256_si128(add_filter_reg32));
        src_reg_filt1_1 = _mm_srai_epi16(src_reg_filt1_1, 6);

        // shrink to 8 bit each 16 bits
        src_reg_filt1_1 = _mm_packus_epi16(src_reg_filt1_1, _mm_setzero_si128());

        // save 8 bytes
        _mm_storel_epi64((__m128i*)output_ptr, src_reg_filt1_1);
    }
}

static void svt_aom_filter_block1d8_h8_avx2(const uint8_t* src_ptr, ptrdiff_t src_pixels_per_line, uint8_t* output_ptr,
                                            ptrdiff_t output_pitch, uint32_t output_height, const int16_t* filter) {
    __m128i      filters_reg;
    __m256i      add_filter_reg32, filt1_reg, filt2_reg, filt3_reg, filt4_reg;
    __m256i      first_filters, second_filters, third_filters, forth_filters;
    __m256i      src_reg_filt32b1_1, src_reg_Filt32b2, src_reg_Filt32b3;
    __m256i      src_reg32b1;
    unsigned int i;
    ptrdiff_t    src_stride, dst_stride;
    src_ptr -= 3;
    add_filter_reg32 = _mm256_set1_epi16(32);
    filters_reg      = _mm_loadu_si128((const __m128i*)filter);
    filters_reg      = _mm_srai_epi16(filters_reg, 1);
    // converting the 16 bit (short) to 8 bit (byte) and have the same data
    // in both lanes of 128 bit register.
    filters_reg = _mm_packs_epi16(filters_reg, filters_reg);
    // have the same data in both lanes of a 256 bit register
    const __m256i filters_reg32 = MM256_BROADCASTSI128_SI256(filters_reg);

    // duplicate only the first 16 bits (first and second byte)
    // across 256 bit register
    first_filters = _mm256_shuffle_epi8(filters_reg32, _mm256_set1_epi16(0x100u));
    // duplicate only the second 16 bits (third and forth byte)
    // across 256 bit register
    second_filters = _mm256_shuffle_epi8(filters_reg32, _mm256_set1_epi16(0x302u));
    // duplicate only the third 16 bits (fifth and sixth byte)
    // across 256 bit register
    third_filters = _mm256_shuffle_epi8(filters_reg32, _mm256_set1_epi16(0x504u));
    // duplicate only the forth 16 bits (seventh and eighth byte)
    // across 256 bit register
    forth_filters = _mm256_shuffle_epi8(filters_reg32, _mm256_set1_epi16(0x706u));

    filt1_reg = _mm256_loadu_si256((__m256i const*)filt_global_avx2);
    filt2_reg = _mm256_loadu_si256((__m256i const*)(filt_global_avx2 + 32));
    filt3_reg = _mm256_loadu_si256((__m256i const*)(filt_global_avx2 + 32 * 2));
    filt4_reg = _mm256_loadu_si256((__m256i const*)(filt_global_avx2 + 32 * 3));

    // multiple the size of the source and destination stride by two
    src_stride = src_pixels_per_line << 1;
    dst_stride = output_pitch << 1;
    for (i = output_height; i > 1; i -= 2) {
        // load the 2 strides of source
        src_reg32b1 = xx_loadu2_mi128(src_ptr + src_pixels_per_line, src_ptr);

        // filter the source buffer
        src_reg_filt32b1_1 = _mm256_shuffle_epi8(src_reg32b1, filt1_reg);
        src_reg_Filt32b2   = _mm256_shuffle_epi8(src_reg32b1, filt4_reg);

        // multiply 2 adjacent elements with the filter and add the result
        src_reg_filt32b1_1 = _mm256_maddubs_epi16(src_reg_filt32b1_1, first_filters);
        src_reg_Filt32b2   = _mm256_maddubs_epi16(src_reg_Filt32b2, forth_filters);

        // add and saturate the results together
        src_reg_filt32b1_1 = _mm256_adds_epi16(src_reg_filt32b1_1, src_reg_Filt32b2);

        // filter the source buffer
        src_reg_Filt32b3 = _mm256_shuffle_epi8(src_reg32b1, filt2_reg);
        src_reg_Filt32b2 = _mm256_shuffle_epi8(src_reg32b1, filt3_reg);

        // multiply 2 adjacent elements with the filter and add the result
        src_reg_Filt32b3 = _mm256_maddubs_epi16(src_reg_Filt32b3, second_filters);
        src_reg_Filt32b2 = _mm256_maddubs_epi16(src_reg_Filt32b2, third_filters);

        __m256i sum23      = _mm256_adds_epi16(src_reg_Filt32b3, src_reg_Filt32b2);
        src_reg_filt32b1_1 = _mm256_adds_epi16(src_reg_filt32b1_1, sum23);

        // shift by 6 bit each 16 bit
        src_reg_filt32b1_1 = _mm256_adds_epi16(src_reg_filt32b1_1, add_filter_reg32);
        src_reg_filt32b1_1 = _mm256_srai_epi16(src_reg_filt32b1_1, 6);

        // shrink to 8 bit each 16 bits, the first lane contain the first
        // convolve result and the second lane contain the second convolve result
        src_reg_filt32b1_1 = _mm256_packus_epi16(src_reg_filt32b1_1, _mm256_setzero_si256());

        src_ptr += src_stride;

        xx_storeu2_epi64(output_ptr, output_pitch, &src_reg_filt32b1_1);
        output_ptr += dst_stride;
    }

    // if the number of strides is odd.
    // process only 8 bytes
    if (i > 0) {
        __m128i src_reg1, src_reg_filt1_1;
        __m128i src_reg_filt2, src_reg_filt3;

        src_reg1 = _mm_loadu_si128((const __m128i*)(src_ptr));

        // filter the source buffer
        src_reg_filt1_1 = _mm_shuffle_epi8(src_reg1, _mm256_castsi256_si128(filt1_reg));
        src_reg_filt2   = _mm_shuffle_epi8(src_reg1, _mm256_castsi256_si128(filt4_reg));

        // multiply 2 adjacent elements with the filter and add the result
        src_reg_filt1_1 = _mm_maddubs_epi16(src_reg_filt1_1, _mm256_castsi256_si128(first_filters));
        src_reg_filt2   = _mm_maddubs_epi16(src_reg_filt2, _mm256_castsi256_si128(forth_filters));

        // add and saturate the results together
        src_reg_filt1_1 = _mm_adds_epi16(src_reg_filt1_1, src_reg_filt2);

        // filter the source buffer
        src_reg_filt3 = _mm_shuffle_epi8(src_reg1, _mm256_castsi256_si128(filt2_reg));
        src_reg_filt2 = _mm_shuffle_epi8(src_reg1, _mm256_castsi256_si128(filt3_reg));

        // multiply 2 adjacent elements with the filter and add the result
        src_reg_filt3 = _mm_maddubs_epi16(src_reg_filt3, _mm256_castsi256_si128(second_filters));
        src_reg_filt2 = _mm_maddubs_epi16(src_reg_filt2, _mm256_castsi256_si128(third_filters));

        // add and saturate the results together
        src_reg_filt1_1 = _mm_adds_epi16(src_reg_filt1_1, _mm_adds_epi16(src_reg_filt3, src_reg_filt2));

        // shift by 6 bit each 16 bit
        src_reg_filt1_1 = _mm_adds_epi16(src_reg_filt1_1, _mm256_castsi256_si128(add_filter_reg32));
        src_reg_filt1_1 = _mm_srai_epi16(src_reg_filt1_1, 6);

        // shrink to 8 bit each 16 bits, the first lane contain the first
        // convolve result and the second lane contain the second convolve
        // result
        src_reg_filt1_1 = _mm_packus_epi16(src_reg_filt1_1, _mm_setzero_si128());

        // save 8 bytes
        _mm_storel_epi64((__m128i*)output_ptr, src_reg_filt1_1);
    }
}

static void svt_aom_filter_block1d16_h4_avx2(const uint8_t* src_ptr, ptrdiff_t src_pixels_per_line, uint8_t* output_ptr,
                                             ptrdiff_t output_pitch, uint32_t output_height, const int16_t* filter) {
    __m128i      filters_reg;
    __m256i      add_filter_reg32, filt2_reg, filt3_reg;
    __m256i      second_filters, third_filters;
    __m256i      src_reg_filt32b1_1, src_reg_Filt32b2_1, src_reg_Filt32b2, src_reg_Filt32b3;
    __m256i      src_reg32b1, src_reg_32b2, filters_reg32;
    unsigned int i;
    ptrdiff_t    src_stride, dst_stride;
    src_ptr -= 3;
    add_filter_reg32 = _mm256_set1_epi16(32);
    filters_reg      = _mm_loadu_si128((const __m128i*)filter);
    filters_reg      = _mm_srai_epi16(filters_reg, 1);
    // converting the 16 bit (short) to 8 bit (byte) and have the same data
    // in both lanes of 128 bit register.
    filters_reg = _mm_packs_epi16(filters_reg, filters_reg);
    // have the same data in both lanes of a 256 bit register
    filters_reg32 = MM256_BROADCASTSI128_SI256(filters_reg);

    // duplicate only the second 16 bits (third and forth byte)
    // across 256 bit register
    second_filters = _mm256_shuffle_epi8(filters_reg32, _mm256_set1_epi16(0x302u));
    // duplicate only the third 16 bits (fifth and sixth byte)
    // across 256 bit register
    third_filters = _mm256_shuffle_epi8(filters_reg32, _mm256_set1_epi16(0x504u));

    filt2_reg = _mm256_loadu_si256((__m256i const*)(filt_global_avx2 + 32));
    filt3_reg = _mm256_loadu_si256((__m256i const*)(filt_global_avx2 + 32 * 2));

    // multiply the size of the source and destination stride by two
    src_stride = src_pixels_per_line << 1;
    dst_stride = output_pitch << 1;
    for (i = output_height; i > 1; i -= 2) {
        // load the 2 strides of source
        src_reg32b1 = xx_loadu2_mi128(src_ptr + src_pixels_per_line, src_ptr);

        // filter the source buffer
        src_reg_Filt32b3 = _mm256_shuffle_epi8(src_reg32b1, filt2_reg);
        src_reg_Filt32b2 = _mm256_shuffle_epi8(src_reg32b1, filt3_reg);

        // multiply 2 adjacent elements with the filter and add the result
        src_reg_Filt32b3 = _mm256_maddubs_epi16(src_reg_Filt32b3, second_filters);
        src_reg_Filt32b2 = _mm256_maddubs_epi16(src_reg_Filt32b2, third_filters);

        src_reg_filt32b1_1 = _mm256_adds_epi16(src_reg_Filt32b3, src_reg_Filt32b2);

        // reading 2 strides of the next 16 bytes
        // (part of it was being read by earlier read)
        src_reg_32b2 = xx_loadu2_mi128(src_ptr + src_pixels_per_line + 8, src_ptr + 8);

        // filter the source buffer
        src_reg_Filt32b3 = _mm256_shuffle_epi8(src_reg_32b2, filt2_reg);
        src_reg_Filt32b2 = _mm256_shuffle_epi8(src_reg_32b2, filt3_reg);

        // multiply 2 adjacent elements with the filter and add the result
        src_reg_Filt32b3 = _mm256_maddubs_epi16(src_reg_Filt32b3, second_filters);
        src_reg_Filt32b2 = _mm256_maddubs_epi16(src_reg_Filt32b2, third_filters);

        // add and saturate the results together
        src_reg_Filt32b2_1 = _mm256_adds_epi16(src_reg_Filt32b3, src_reg_Filt32b2);

        // shift by 6 bit each 16 bit
        src_reg_filt32b1_1 = _mm256_adds_epi16(src_reg_filt32b1_1, add_filter_reg32);
        src_reg_Filt32b2_1 = _mm256_adds_epi16(src_reg_Filt32b2_1, add_filter_reg32);
        src_reg_filt32b1_1 = _mm256_srai_epi16(src_reg_filt32b1_1, 6);
        src_reg_Filt32b2_1 = _mm256_srai_epi16(src_reg_Filt32b2_1, 6);

        // shrink to 8 bit each 16 bits, the first lane contain the first
        // convolve result and the second lane contain the second convolve result
        src_reg_filt32b1_1 = _mm256_packus_epi16(src_reg_filt32b1_1, src_reg_Filt32b2_1);

        src_ptr += src_stride;

        xx_store2_mi128(output_ptr, output_pitch, &src_reg_filt32b1_1);
        output_ptr += dst_stride;
    }

    // if the number of strides is odd.
    // process only 16 bytes
    if (i > 0) {
        __m256i src_reg1, src_reg12;
        __m256i src_reg_filt2, src_reg_filt3, src_reg_filt1_1;

        src_reg1  = _mm256_loadu_si256((const __m256i*)(src_ptr));
        src_reg12 = _mm256_permute4x64_epi64(src_reg1, 0x94);

        // filter the source buffer
        src_reg_filt2 = _mm256_shuffle_epi8(src_reg12, filt2_reg);
        src_reg_filt3 = _mm256_shuffle_epi8(src_reg12, filt3_reg);

        // multiply 2 adjacent elements with the filter and add the result
        src_reg_filt2 = _mm256_maddubs_epi16(src_reg_filt2, second_filters);
        src_reg_filt3 = _mm256_maddubs_epi16(src_reg_filt3, third_filters);

        // add and saturate the results together
        src_reg_filt1_1 = _mm256_adds_epi16(src_reg_filt2, src_reg_filt3);

        // shift by 6 bit each 16 bit
        src_reg_filt1_1 = _mm256_adds_epi16(src_reg_filt1_1, add_filter_reg32);
        src_reg_filt1_1 = _mm256_srai_epi16(src_reg_filt1_1, 6);

        // shrink to 8 bit each 16 bits, the first lane contain the first
        // convolve result and the second lane contain the second convolve
        // result
        src_reg_filt1_1 = _mm256_packus_epi16(src_reg_filt1_1, src_reg_filt1_1);
        src_reg_filt1_1 = _mm256_permute4x64_epi64(src_reg_filt1_1, 0x8);

        // save 16 bytes
        _mm_storeu_si128((__m128i*)output_ptr, _mm256_castsi256_si128(src_reg_filt1_1));
    }
}

static void svt_aom_filter_block1d16_h8_avx2(const uint8_t* src_ptr, ptrdiff_t src_pixels_per_line, uint8_t* output_ptr,
                                             ptrdiff_t output_pitch, uint32_t output_height, const int16_t* filter) {
    __m128i      filters_reg;
    __m256i      add_filter_reg32, filt1_reg, filt2_reg, filt3_reg, filt4_reg;
    __m256i      first_filters, second_filters, third_filters, forth_filters;
    __m256i      src_reg_filt32b1_1, src_reg_Filt32b2_1, src_reg_Filt32b2, src_reg_Filt32b3;
    __m256i      src_reg32b1, src_reg_32b2, filters_reg32;
    unsigned int i;
    ptrdiff_t    src_stride, dst_stride;
    src_ptr -= 3;
    add_filter_reg32 = _mm256_set1_epi16(32);
    filters_reg      = _mm_loadu_si128((const __m128i*)filter);
    filters_reg      = _mm_srai_epi16(filters_reg, 1);
    // converting the 16 bit (short) to 8 bit (byte) and have the same data
    // in both lanes of 128 bit register.
    filters_reg = _mm_packs_epi16(filters_reg, filters_reg);
    // have the same data in both lanes of a 256 bit register
    filters_reg32 = MM256_BROADCASTSI128_SI256(filters_reg);

    // duplicate only the first 16 bits (first and second byte)
    // across 256 bit register
    first_filters = _mm256_shuffle_epi8(filters_reg32, _mm256_set1_epi16(0x100u));
    // duplicate only the second 16 bits (third and forth byte)
    // across 256 bit register
    second_filters = _mm256_shuffle_epi8(filters_reg32, _mm256_set1_epi16(0x302u));
    // duplicate only the third 16 bits (fifth and sixth byte)
    // across 256 bit register
    third_filters = _mm256_shuffle_epi8(filters_reg32, _mm256_set1_epi16(0x504u));
    // duplicate only the forth 16 bits (seventh and eighth byte)
    // across 256 bit register
    forth_filters = _mm256_shuffle_epi8(filters_reg32, _mm256_set1_epi16(0x706u));

    filt1_reg = _mm256_loadu_si256((__m256i const*)filt_global_avx2);
    filt2_reg = _mm256_loadu_si256((__m256i const*)(filt_global_avx2 + 32));
    filt3_reg = _mm256_loadu_si256((__m256i const*)(filt_global_avx2 + 32 * 2));
    filt4_reg = _mm256_loadu_si256((__m256i const*)(filt_global_avx2 + 32 * 3));

    // multiple the size of the source and destination stride by two
    src_stride = src_pixels_per_line << 1;
    dst_stride = output_pitch << 1;
    for (i = output_height; i > 1; i -= 2) {
        // load the 2 strides of source
        src_reg32b1 = xx_loadu2_mi128(src_ptr + src_pixels_per_line, src_ptr);

        // filter the source buffer
        src_reg_filt32b1_1 = _mm256_shuffle_epi8(src_reg32b1, filt1_reg);
        src_reg_Filt32b2   = _mm256_shuffle_epi8(src_reg32b1, filt4_reg);

        // multiply 2 adjacent elements with the filter and add the result
        src_reg_filt32b1_1 = _mm256_maddubs_epi16(src_reg_filt32b1_1, first_filters);
        src_reg_Filt32b2   = _mm256_maddubs_epi16(src_reg_Filt32b2, forth_filters);

        // add and saturate the results together
        src_reg_filt32b1_1 = _mm256_adds_epi16(src_reg_filt32b1_1, src_reg_Filt32b2);

        // filter the source buffer
        src_reg_Filt32b3 = _mm256_shuffle_epi8(src_reg32b1, filt2_reg);
        src_reg_Filt32b2 = _mm256_shuffle_epi8(src_reg32b1, filt3_reg);

        // multiply 2 adjacent elements with the filter and add the result
        src_reg_Filt32b3 = _mm256_maddubs_epi16(src_reg_Filt32b3, second_filters);
        src_reg_Filt32b2 = _mm256_maddubs_epi16(src_reg_Filt32b2, third_filters);

        __m256i sum23      = _mm256_adds_epi16(src_reg_Filt32b3, src_reg_Filt32b2);
        src_reg_filt32b1_1 = _mm256_adds_epi16(src_reg_filt32b1_1, sum23);

        // reading 2 strides of the next 16 bytes
        // (part of it was being read by earlier read)
        src_reg_32b2 = xx_loadu2_mi128(src_ptr + src_pixels_per_line + 8, src_ptr + 8);

        // filter the source buffer
        src_reg_Filt32b2_1 = _mm256_shuffle_epi8(src_reg_32b2, filt1_reg);
        src_reg_Filt32b2   = _mm256_shuffle_epi8(src_reg_32b2, filt4_reg);

        // multiply 2 adjacent elements with the filter and add the result
        src_reg_Filt32b2_1 = _mm256_maddubs_epi16(src_reg_Filt32b2_1, first_filters);
        src_reg_Filt32b2   = _mm256_maddubs_epi16(src_reg_Filt32b2, forth_filters);

        // add and saturate the results together
        src_reg_Filt32b2_1 = _mm256_adds_epi16(src_reg_Filt32b2_1, src_reg_Filt32b2);

        // filter the source buffer
        src_reg_Filt32b3 = _mm256_shuffle_epi8(src_reg_32b2, filt2_reg);
        src_reg_Filt32b2 = _mm256_shuffle_epi8(src_reg_32b2, filt3_reg);

        // multiply 2 adjacent elements with the filter and add the result
        src_reg_Filt32b3 = _mm256_maddubs_epi16(src_reg_Filt32b3, second_filters);
        src_reg_Filt32b2 = _mm256_maddubs_epi16(src_reg_Filt32b2, third_filters);

        // add and saturate the results together
        src_reg_Filt32b2_1 = _mm256_adds_epi16(src_reg_Filt32b2_1,
                                               _mm256_adds_epi16(src_reg_Filt32b3, src_reg_Filt32b2));

        // shift by 6 bit each 16 bit
        src_reg_filt32b1_1 = _mm256_adds_epi16(src_reg_filt32b1_1, add_filter_reg32);
        src_reg_Filt32b2_1 = _mm256_adds_epi16(src_reg_Filt32b2_1, add_filter_reg32);
        src_reg_filt32b1_1 = _mm256_srai_epi16(src_reg_filt32b1_1, 6);
        src_reg_Filt32b2_1 = _mm256_srai_epi16(src_reg_Filt32b2_1, 6);

        // shrink to 8 bit each 16 bits, the first lane contain the first
        // convolve result and the second lane contain the second convolve result
        src_reg_filt32b1_1 = _mm256_packus_epi16(src_reg_filt32b1_1, src_reg_Filt32b2_1);

        src_ptr += src_stride;

        xx_store2_mi128(output_ptr, output_pitch, &src_reg_filt32b1_1);
        output_ptr += dst_stride;
    }

    // if the number of strides is odd.
    // process only 16 bytes
    if (i > 0) {
        __m128i src_reg1, src_reg2, src_reg_filt1_1, src_reg_filt2_1;
        __m128i src_reg_filt2, src_reg_filt3;

        src_reg1 = _mm_loadu_si128((const __m128i*)(src_ptr));

        // filter the source buffer
        src_reg_filt1_1 = _mm_shuffle_epi8(src_reg1, _mm256_castsi256_si128(filt1_reg));
        src_reg_filt2   = _mm_shuffle_epi8(src_reg1, _mm256_castsi256_si128(filt4_reg));

        // multiply 2 adjacent elements with the filter and add the result
        src_reg_filt1_1 = _mm_maddubs_epi16(src_reg_filt1_1, _mm256_castsi256_si128(first_filters));
        src_reg_filt2   = _mm_maddubs_epi16(src_reg_filt2, _mm256_castsi256_si128(forth_filters));

        // add and saturate the results together
        src_reg_filt1_1 = _mm_adds_epi16(src_reg_filt1_1, src_reg_filt2);

        // filter the source buffer
        src_reg_filt3 = _mm_shuffle_epi8(src_reg1, _mm256_castsi256_si128(filt2_reg));
        src_reg_filt2 = _mm_shuffle_epi8(src_reg1, _mm256_castsi256_si128(filt3_reg));

        // multiply 2 adjacent elements with the filter and add the result
        src_reg_filt3 = _mm_maddubs_epi16(src_reg_filt3, _mm256_castsi256_si128(second_filters));
        src_reg_filt2 = _mm_maddubs_epi16(src_reg_filt2, _mm256_castsi256_si128(third_filters));

        // add and saturate the results together
        src_reg_filt1_1 = _mm_adds_epi16(src_reg_filt1_1, _mm_adds_epi16(src_reg_filt3, src_reg_filt2));

        // reading the next 16 bytes
        // (part of it was being read by earlier read)
        src_reg2 = _mm_loadu_si128((const __m128i*)(src_ptr + 8));

        // filter the source buffer
        src_reg_filt2_1 = _mm_shuffle_epi8(src_reg2, _mm256_castsi256_si128(filt1_reg));
        src_reg_filt2   = _mm_shuffle_epi8(src_reg2, _mm256_castsi256_si128(filt4_reg));

        // multiply 2 adjacent elements with the filter and add the result
        src_reg_filt2_1 = _mm_maddubs_epi16(src_reg_filt2_1, _mm256_castsi256_si128(first_filters));
        src_reg_filt2   = _mm_maddubs_epi16(src_reg_filt2, _mm256_castsi256_si128(forth_filters));

        // add and saturate the results together
        src_reg_filt2_1 = _mm_adds_epi16(src_reg_filt2_1, src_reg_filt2);

        // filter the source buffer
        src_reg_filt3 = _mm_shuffle_epi8(src_reg2, _mm256_castsi256_si128(filt2_reg));
        src_reg_filt2 = _mm_shuffle_epi8(src_reg2, _mm256_castsi256_si128(filt3_reg));

        // multiply 2 adjacent elements with the filter and add the result
        src_reg_filt3 = _mm_maddubs_epi16(src_reg_filt3, _mm256_castsi256_si128(second_filters));
        src_reg_filt2 = _mm_maddubs_epi16(src_reg_filt2, _mm256_castsi256_si128(third_filters));

        // add and saturate the results together
        src_reg_filt2_1 = _mm_adds_epi16(src_reg_filt2_1, _mm_adds_epi16(src_reg_filt3, src_reg_filt2));

        // shift by 6 bit each 16 bit
        src_reg_filt1_1 = _mm_adds_epi16(src_reg_filt1_1, _mm256_castsi256_si128(add_filter_reg32));
        src_reg_filt1_1 = _mm_srai_epi16(src_reg_filt1_1, 6);

        src_reg_filt2_1 = _mm_adds_epi16(src_reg_filt2_1, _mm256_castsi256_si128(add_filter_reg32));
        src_reg_filt2_1 = _mm_srai_epi16(src_reg_filt2_1, 6);

        // shrink to 8 bit each 16 bits, the first lane contain the first
        // convolve result and the second lane contain the second convolve
        // result
        src_reg_filt1_1 = _mm_packus_epi16(src_reg_filt1_1, src_reg_filt2_1);

        // save 16 bytes
        _mm_storeu_si128((__m128i*)output_ptr, src_reg_filt1_1);
    }
}

static void svt_aom_filter_block1d8_v4_avx2(const uint8_t* src_ptr, ptrdiff_t src_pitch, uint8_t* output_ptr,
                                            ptrdiff_t out_pitch, uint32_t output_height, const int16_t* filter) {
    __m128i      filters_reg;
    __m256i      filters_reg32, add_filter_reg32;
    __m256i      src_reg23, src_reg_4x, src_reg_34, src_reg_5x, src_reg_45, src_reg_6x, src_reg_56;
    __m256i      src_reg_23_34_lo, src_reg_45_56_lo;
    __m256i      res_reg23_34_lo, res_reg45_56_lo;
    __m256i      res_reg_lo, res_reg;
    __m256i      second_filters, third_filters;
    unsigned int i;
    ptrdiff_t    src_stride, dst_stride;

    add_filter_reg32 = _mm256_set1_epi16(32);
    filters_reg      = _mm_loadu_si128((const __m128i*)filter);
    // converting the 16 bit (short) to  8 bit (byte) and have the
    // same data in both lanes of 128 bit register.
    filters_reg = _mm_srai_epi16(filters_reg, 1);
    filters_reg = _mm_packs_epi16(filters_reg, filters_reg);
    // have the same data in both lanes of a 256 bit register
    filters_reg32 = MM256_BROADCASTSI128_SI256(filters_reg);

    // duplicate only the second 16 bits (third and forth byte)
    // across 256 bit register
    second_filters = _mm256_shuffle_epi8(filters_reg32, _mm256_set1_epi16(0x302u));
    // duplicate only the third 16 bits (fifth and sixth byte)
    // across 256 bit register
    third_filters = _mm256_shuffle_epi8(filters_reg32, _mm256_set1_epi16(0x504u));

    // multiple the size of the source and destination stride by two
    src_stride = src_pitch << 1;
    dst_stride = out_pitch << 1;

    src_reg23  = xx_loadu2_epi64(src_ptr + src_pitch * 3, src_ptr + src_pitch * 2);
    src_reg_4x = _mm256_castsi128_si256(_mm_loadl_epi64((const __m128i*)(src_ptr + src_pitch * 4)));

    // have consecutive loads on the same 256 register
    src_reg_34 = _mm256_permute2x128_si256(src_reg23, src_reg_4x, 0x21);

    src_reg_23_34_lo = _mm256_unpacklo_epi8(src_reg23, src_reg_34);

    for (i = output_height; i > 1; i -= 2) {
        // load the last 2 loads of 16 bytes and have every two
        // consecutive loads in the same 256 bit register
        src_reg_5x = _mm256_castsi128_si256(_mm_loadl_epi64((const __m128i*)(src_ptr + src_pitch * 5)));
        src_reg_45 = _mm256_inserti128_si256(src_reg_4x, _mm256_castsi256_si128(src_reg_5x), 1);

        src_reg_6x = _mm256_castsi128_si256(_mm_loadl_epi64((const __m128i*)(src_ptr + src_pitch * 6)));
        src_reg_56 = _mm256_inserti128_si256(src_reg_5x, _mm256_castsi256_si128(src_reg_6x), 1);

        // merge every two consecutive registers
        src_reg_45_56_lo = _mm256_unpacklo_epi8(src_reg_45, src_reg_56);

        // multiply 2 adjacent elements with the filter and add the result
        res_reg23_34_lo = _mm256_maddubs_epi16(src_reg_23_34_lo, second_filters);
        res_reg45_56_lo = _mm256_maddubs_epi16(src_reg_45_56_lo, third_filters);

        // add and saturate the results together
        res_reg_lo = _mm256_adds_epi16(res_reg23_34_lo, res_reg45_56_lo);

        // shift by 6 bit each 16 bit
        res_reg_lo = _mm256_adds_epi16(res_reg_lo, add_filter_reg32);
        res_reg_lo = _mm256_srai_epi16(res_reg_lo, 6);

        // shrink to 8 bit each 16 bits, the first lane contain the first
        // convolve result and the second lane contain the second convolve
        // result
        res_reg = _mm256_packus_epi16(res_reg_lo, res_reg_lo);

        src_ptr += src_stride;

        xx_storeu2_epi64(output_ptr, out_pitch, &res_reg);

        output_ptr += dst_stride;

        // save part of the registers for next strides
        src_reg_23_34_lo = src_reg_45_56_lo;
        src_reg_4x       = src_reg_6x;
    }
}

static void svt_aom_filter_block1d8_v8_avx2(const uint8_t* src_ptr, ptrdiff_t src_pitch, uint8_t* output_ptr,
                                            ptrdiff_t out_pitch, uint32_t output_height, const int16_t* filter) {
    __m128i      filters_reg;
    __m256i      add_filter_reg32;
    __m256i      src_reg32b1, src_reg_32b2, src_reg_32b3, src_reg_32b4, src_reg_32b5;
    __m256i      src_reg_32b6, src_reg_32b7, src_reg_32b8, src_reg_32b9, src_reg_32b10;
    __m256i      src_reg_32b11, src_reg_32b12, filters_reg32;
    __m256i      first_filters, second_filters, third_filters, forth_filters;
    unsigned int i;
    ptrdiff_t    src_stride, dst_stride;

    add_filter_reg32 = _mm256_set1_epi16(32);
    filters_reg      = _mm_loadu_si128((const __m128i*)filter);
    // converting the 16 bit (short) to  8 bit (byte) and have the
    // same data in both lanes of 128 bit register.
    filters_reg = _mm_srai_epi16(filters_reg, 1);
    filters_reg = _mm_packs_epi16(filters_reg, filters_reg);
    // have the same data in both lanes of a 256 bit register
    filters_reg32 = MM256_BROADCASTSI128_SI256(filters_reg);

    // duplicate only the first 16 bits (first and second byte)
    // across 256 bit register
    first_filters = _mm256_shuffle_epi8(filters_reg32, _mm256_set1_epi16(0x100u));
    // duplicate only the second 16 bits (third and forth byte)
    // across 256 bit register
    second_filters = _mm256_shuffle_epi8(filters_reg32, _mm256_set1_epi16(0x302u));
    // duplicate only the third 16 bits (fifth and sixth byte)
    // across 256 bit register
    third_filters = _mm256_shuffle_epi8(filters_reg32, _mm256_set1_epi16(0x504u));
    // duplicate only the forth 16 bits (seventh and eighth byte)
    // across 256 bit register
    forth_filters = _mm256_shuffle_epi8(filters_reg32, _mm256_set1_epi16(0x706u));

    // multiple the size of the source and destination stride by two
    src_stride = src_pitch << 1;
    dst_stride = out_pitch << 1;

    // load 16 bytes 7 times in stride of src_pitch
    src_reg32b1  = xx_loadu2_epi64(src_ptr + src_pitch, src_ptr);
    src_reg_32b3 = xx_loadu2_epi64(src_ptr + src_pitch * 3, src_ptr + src_pitch * 2);
    src_reg_32b5 = xx_loadu2_epi64(src_ptr + src_pitch * 5, src_ptr + src_pitch * 4);
    src_reg_32b7 = _mm256_castsi128_si256(_mm_loadl_epi64((const __m128i*)(src_ptr + src_pitch * 6)));

    // have each consecutive loads on the same 256 register
    src_reg_32b2 = _mm256_permute2x128_si256(src_reg32b1, src_reg_32b3, 0x21);
    src_reg_32b4 = _mm256_permute2x128_si256(src_reg_32b3, src_reg_32b5, 0x21);
    src_reg_32b6 = _mm256_permute2x128_si256(src_reg_32b5, src_reg_32b7, 0x21);
    // merge every two consecutive registers except the last one
    src_reg_32b10 = _mm256_unpacklo_epi8(src_reg32b1, src_reg_32b2);
    src_reg_32b11 = _mm256_unpacklo_epi8(src_reg_32b3, src_reg_32b4);
    src_reg_32b2  = _mm256_unpacklo_epi8(src_reg_32b5, src_reg_32b6);

    for (i = output_height; i > 1; i -= 2) {
        // load the last 2 loads of 16 bytes and have every two
        // consecutive loads in the same 256 bit register
        src_reg_32b8 = _mm256_castsi128_si256(_mm_loadl_epi64((const __m128i*)(src_ptr + src_pitch * 7)));
        src_reg_32b7 = _mm256_inserti128_si256(src_reg_32b7, _mm256_castsi256_si128(src_reg_32b8), 1);
        src_reg_32b9 = _mm256_castsi128_si256(_mm_loadl_epi64((const __m128i*)(src_ptr + src_pitch * 8)));
        src_reg_32b8 = _mm256_inserti128_si256(src_reg_32b8, _mm256_castsi256_si128(src_reg_32b9), 1);

        // merge every two consecutive registers
        // save
        src_reg_32b4 = _mm256_unpacklo_epi8(src_reg_32b7, src_reg_32b8);

        // multiply 2 adjacent elements with the filter and add the result
        src_reg_32b10 = _mm256_maddubs_epi16(src_reg_32b10, first_filters);
        src_reg_32b6  = _mm256_maddubs_epi16(src_reg_32b4, forth_filters);

        // add and saturate the results together
        src_reg_32b10 = _mm256_adds_epi16(src_reg_32b10, src_reg_32b6);

        // multiply 2 adjacent elements with the filter and add the result
        src_reg_32b8  = _mm256_maddubs_epi16(src_reg_32b11, second_filters);
        src_reg_32b12 = _mm256_maddubs_epi16(src_reg_32b2, third_filters);

        // add and saturate the results together
        src_reg_32b10 = _mm256_adds_epi16(src_reg_32b10, _mm256_adds_epi16(src_reg_32b8, src_reg_32b12));

        // shift by 6 bit each 16 bit
        src_reg_32b10 = _mm256_adds_epi16(src_reg_32b10, add_filter_reg32);
        src_reg_32b10 = _mm256_srai_epi16(src_reg_32b10, 6);

        // shrink to 8 bit each 16 bits, the first lane contain the first
        // convolve result and the second lane contain the second convolve
        // result
        src_reg32b1 = _mm256_packus_epi16(src_reg_32b10, _mm256_setzero_si256());

        src_ptr += src_stride;

        xx_storeu2_epi64(output_ptr, out_pitch, &src_reg32b1);

        output_ptr += dst_stride;

        // save part of the registers for next strides
        src_reg_32b10 = src_reg_32b11;
        src_reg_32b11 = src_reg_32b2;
        src_reg_32b2  = src_reg_32b4;
        src_reg_32b7  = src_reg_32b9;
    }
    if (i > 0) {
        __m128i src_reg_filt1, src_reg_filt4, src_reg_filt6, src_reg_filt8;
        // load the last 16 bytes
        src_reg_filt8 = _mm_loadl_epi64((const __m128i*)(src_ptr + src_pitch * 7));

        // merge the last 2 results together
        src_reg_filt4 = _mm_unpacklo_epi8(_mm256_castsi256_si128(src_reg_32b7), src_reg_filt8);

        // multiply 2 adjacent elements with the filter and add the result
        src_reg_filt1 = _mm_maddubs_epi16(_mm256_castsi256_si128(src_reg_32b10), _mm256_castsi256_si128(first_filters));
        src_reg_filt4 = _mm_maddubs_epi16(src_reg_filt4, _mm256_castsi256_si128(forth_filters));

        // add and saturate the results together
        src_reg_filt1 = _mm_adds_epi16(src_reg_filt1, src_reg_filt4);

        // multiply 2 adjacent elements with the filter and add the result
        src_reg_filt4 = _mm_maddubs_epi16(_mm256_castsi256_si128(src_reg_32b11),
                                          _mm256_castsi256_si128(second_filters));

        // multiply 2 adjacent elements with the filter and add the result
        src_reg_filt6 = _mm_maddubs_epi16(_mm256_castsi256_si128(src_reg_32b2), _mm256_castsi256_si128(third_filters));

        // add and saturate the results together
        src_reg_filt1 = _mm_adds_epi16(src_reg_filt1, _mm_adds_epi16(src_reg_filt4, src_reg_filt6));

        // shift by 6 bit each 16 bit
        src_reg_filt1 = _mm_adds_epi16(src_reg_filt1, _mm256_castsi256_si128(add_filter_reg32));
        src_reg_filt1 = _mm_srai_epi16(src_reg_filt1, 6);

        // shrink to 8 bit each 16 bits, the first lane contain the first
        // convolve result and the second lane contain the second convolve result
        src_reg_filt1 = _mm_packus_epi16(src_reg_filt1, _mm_setzero_si128());

        // save 8 bytes
        _mm_storel_epi64((__m128i*)output_ptr, src_reg_filt1);
    }
}

static void svt_aom_filter_block1d16_v4_avx2(const uint8_t* src_ptr, ptrdiff_t src_pitch, uint8_t* output_ptr,
                                             ptrdiff_t out_pitch, uint32_t output_height, const int16_t* filter) {
    __m128i      filters_reg;
    __m256i      filters_reg32, add_filter_reg32;
    __m256i      src_reg23, src_reg_4x, src_reg_34, src_reg_5x, src_reg_45, src_reg_6x, src_reg_56;
    __m256i      src_reg_23_34_lo, src_reg_23_34_hi, src_reg_45_56_lo, src_reg_45_56_hi;
    __m256i      res_reg23_34_lo, res_reg23_34_hi, res_reg45_56_lo, res_reg45_56_hi;
    __m256i      res_reg_lo, res_reg_hi, res_reg;
    __m256i      second_filters, third_filters;
    unsigned int i;
    ptrdiff_t    src_stride, dst_stride;

    add_filter_reg32 = _mm256_set1_epi16(32);
    filters_reg      = _mm_loadu_si128((const __m128i*)filter);
    // converting the 16 bit (short) to  8 bit (byte) and have the
    // same data in both lanes of 128 bit register.
    filters_reg = _mm_srai_epi16(filters_reg, 1);
    filters_reg = _mm_packs_epi16(filters_reg, filters_reg);
    // have the same data in both lanes of a 256 bit register
    filters_reg32 = MM256_BROADCASTSI128_SI256(filters_reg);

    // duplicate only the second 16 bits (third and forth byte)
    // across 256 bit register
    second_filters = _mm256_shuffle_epi8(filters_reg32, _mm256_set1_epi16(0x302u));
    // duplicate only the third 16 bits (fifth and sixth byte)
    // across 256 bit register
    third_filters = _mm256_shuffle_epi8(filters_reg32, _mm256_set1_epi16(0x504u));

    // multiple the size of the source and destination stride by two
    src_stride = src_pitch << 1;
    dst_stride = out_pitch << 1;

    src_reg23  = xx_loadu2_mi128(src_ptr + src_pitch * 3, src_ptr + src_pitch * 2);
    src_reg_4x = _mm256_castsi128_si256(_mm_loadu_si128((const __m128i*)(src_ptr + src_pitch * 4)));

    // have consecutive loads on the same 256 register
    src_reg_34 = _mm256_permute2x128_si256(src_reg23, src_reg_4x, 0x21);

    src_reg_23_34_lo = _mm256_unpacklo_epi8(src_reg23, src_reg_34);
    src_reg_23_34_hi = _mm256_unpackhi_epi8(src_reg23, src_reg_34);

    for (i = output_height; i > 1; i -= 2) {
        // load the last 2 loads of 16 bytes and have every two
        // consecutive loads in the same 256 bit register
        src_reg_5x = _mm256_castsi128_si256(_mm_loadu_si128((const __m128i*)(src_ptr + src_pitch * 5)));
        src_reg_45 = _mm256_inserti128_si256(src_reg_4x, _mm256_castsi256_si128(src_reg_5x), 1);

        src_reg_6x = _mm256_castsi128_si256(_mm_loadu_si128((const __m128i*)(src_ptr + src_pitch * 6)));
        src_reg_56 = _mm256_inserti128_si256(src_reg_5x, _mm256_castsi256_si128(src_reg_6x), 1);

        // merge every two consecutive registers
        src_reg_45_56_lo = _mm256_unpacklo_epi8(src_reg_45, src_reg_56);
        src_reg_45_56_hi = _mm256_unpackhi_epi8(src_reg_45, src_reg_56);

        // multiply 2 adjacent elements with the filter and add the result
        res_reg23_34_lo = _mm256_maddubs_epi16(src_reg_23_34_lo, second_filters);
        res_reg45_56_lo = _mm256_maddubs_epi16(src_reg_45_56_lo, third_filters);

        // add and saturate the results together
        res_reg_lo = _mm256_adds_epi16(res_reg23_34_lo, res_reg45_56_lo);

        // multiply 2 adjacent elements with the filter and add the result
        res_reg23_34_hi = _mm256_maddubs_epi16(src_reg_23_34_hi, second_filters);
        res_reg45_56_hi = _mm256_maddubs_epi16(src_reg_45_56_hi, third_filters);

        // add and saturate the results together
        res_reg_hi = _mm256_adds_epi16(res_reg23_34_hi, res_reg45_56_hi);

        // shift by 6 bit each 16 bit
        res_reg_lo = _mm256_adds_epi16(res_reg_lo, add_filter_reg32);
        res_reg_hi = _mm256_adds_epi16(res_reg_hi, add_filter_reg32);
        res_reg_lo = _mm256_srai_epi16(res_reg_lo, 6);
        res_reg_hi = _mm256_srai_epi16(res_reg_hi, 6);

        // shrink to 8 bit each 16 bits, the first lane contain the first
        // convolve result and the second lane contain the second convolve
        // result
        res_reg = _mm256_packus_epi16(res_reg_lo, res_reg_hi);

        src_ptr += src_stride;

        xx_store2_mi128(output_ptr, out_pitch, &res_reg);

        output_ptr += dst_stride;

        // save part of the registers for next strides
        src_reg_23_34_lo = src_reg_45_56_lo;
        src_reg_23_34_hi = src_reg_45_56_hi;
        src_reg_4x       = src_reg_6x;
    }
}

static void svt_aom_filter_block1d16_v8_avx2(const uint8_t* src_ptr, ptrdiff_t src_pitch, uint8_t* output_ptr,
                                             ptrdiff_t out_pitch, uint32_t output_height, const int16_t* filter) {
    __m128i      filters_reg;
    __m256i      add_filter_reg32;
    __m256i      src_reg32b1, src_reg_32b2, src_reg_32b3, src_reg_32b4, src_reg_32b5;
    __m256i      src_reg_32b6, src_reg_32b7, src_reg_32b8, src_reg_32b9, src_reg_32b10;
    __m256i      src_reg_32b11, src_reg_32b12, filters_reg32;
    __m256i      first_filters, second_filters, third_filters, forth_filters;
    unsigned int i;
    ptrdiff_t    src_stride, dst_stride;

    add_filter_reg32 = _mm256_set1_epi16(32);
    filters_reg      = _mm_loadu_si128((const __m128i*)filter);
    // converting the 16 bit (short) to  8 bit (byte) and have the
    // same data in both lanes of 128 bit register.
    filters_reg = _mm_srai_epi16(filters_reg, 1);
    filters_reg = _mm_packs_epi16(filters_reg, filters_reg);
    // have the same data in both lanes of a 256 bit register
    filters_reg32 = MM256_BROADCASTSI128_SI256(filters_reg);

    // duplicate only the first 16 bits (first and second byte)
    // across 256 bit register
    first_filters = _mm256_shuffle_epi8(filters_reg32, _mm256_set1_epi16(0x100u));
    // duplicate only the second 16 bits (third and forth byte)
    // across 256 bit register
    second_filters = _mm256_shuffle_epi8(filters_reg32, _mm256_set1_epi16(0x302u));
    // duplicate only the third 16 bits (fifth and sixth byte)
    // across 256 bit register
    third_filters = _mm256_shuffle_epi8(filters_reg32, _mm256_set1_epi16(0x504u));
    // duplicate only the forth 16 bits (seventh and eighth byte)
    // across 256 bit register
    forth_filters = _mm256_shuffle_epi8(filters_reg32, _mm256_set1_epi16(0x706u));

    // multiple the size of the source and destination stride by two
    src_stride = src_pitch << 1;
    dst_stride = out_pitch << 1;

    // load 16 bytes 7 times in stride of src_pitch
    src_reg32b1  = xx_loadu2_mi128(src_ptr + src_pitch, src_ptr);
    src_reg_32b3 = xx_loadu2_mi128(src_ptr + src_pitch * 3, src_ptr + src_pitch * 2);
    src_reg_32b5 = xx_loadu2_mi128(src_ptr + src_pitch * 5, src_ptr + src_pitch * 4);
    src_reg_32b7 = _mm256_castsi128_si256(_mm_loadu_si128((const __m128i*)(src_ptr + src_pitch * 6)));

    // have each consecutive loads on the same 256 register
    src_reg_32b2 = _mm256_permute2x128_si256(src_reg32b1, src_reg_32b3, 0x21);
    src_reg_32b4 = _mm256_permute2x128_si256(src_reg_32b3, src_reg_32b5, 0x21);
    src_reg_32b6 = _mm256_permute2x128_si256(src_reg_32b5, src_reg_32b7, 0x21);
    // merge every two consecutive registers except the last one
    src_reg_32b10 = _mm256_unpacklo_epi8(src_reg32b1, src_reg_32b2);
    src_reg32b1   = _mm256_unpackhi_epi8(src_reg32b1, src_reg_32b2);

    // save
    src_reg_32b11 = _mm256_unpacklo_epi8(src_reg_32b3, src_reg_32b4);
    src_reg_32b3  = _mm256_unpackhi_epi8(src_reg_32b3, src_reg_32b4);
    src_reg_32b2  = _mm256_unpacklo_epi8(src_reg_32b5, src_reg_32b6);
    src_reg_32b5  = _mm256_unpackhi_epi8(src_reg_32b5, src_reg_32b6);

    for (i = output_height; i > 1; i -= 2) {
        // load the last 2 loads of 16 bytes and have every two
        // consecutive loads in the same 256 bit register
        src_reg_32b8 = _mm256_castsi128_si256(_mm_loadu_si128((const __m128i*)(src_ptr + src_pitch * 7)));
        src_reg_32b7 = _mm256_inserti128_si256(src_reg_32b7, _mm256_castsi256_si128(src_reg_32b8), 1);
        src_reg_32b9 = _mm256_castsi128_si256(_mm_loadu_si128((const __m128i*)(src_ptr + src_pitch * 8)));
        src_reg_32b8 = _mm256_inserti128_si256(src_reg_32b8, _mm256_castsi256_si128(src_reg_32b9), 1);

        // merge every two consecutive registers
        // save
        src_reg_32b4 = _mm256_unpacklo_epi8(src_reg_32b7, src_reg_32b8);
        src_reg_32b7 = _mm256_unpackhi_epi8(src_reg_32b7, src_reg_32b8);

        // multiply 2 adjacent elements with the filter and add the result
        src_reg_32b10 = _mm256_maddubs_epi16(src_reg_32b10, first_filters);
        src_reg_32b6  = _mm256_maddubs_epi16(src_reg_32b4, forth_filters);

        // add and saturate the results together
        src_reg_32b10 = _mm256_adds_epi16(src_reg_32b10, src_reg_32b6);

        // multiply 2 adjacent elements with the filter and add the result
        src_reg_32b8  = _mm256_maddubs_epi16(src_reg_32b11, second_filters);
        src_reg_32b12 = _mm256_maddubs_epi16(src_reg_32b2, third_filters);

        // add and saturate the results together
        src_reg_32b10 = _mm256_adds_epi16(src_reg_32b10, _mm256_adds_epi16(src_reg_32b8, src_reg_32b12));

        // multiply 2 adjacent elements with the filter and add the result
        src_reg32b1  = _mm256_maddubs_epi16(src_reg32b1, first_filters);
        src_reg_32b6 = _mm256_maddubs_epi16(src_reg_32b7, forth_filters);

        src_reg32b1 = _mm256_adds_epi16(src_reg32b1, src_reg_32b6);

        // multiply 2 adjacent elements with the filter and add the result
        src_reg_32b8  = _mm256_maddubs_epi16(src_reg_32b3, second_filters);
        src_reg_32b12 = _mm256_maddubs_epi16(src_reg_32b5, third_filters);

        // add and saturate the results together
        src_reg32b1 = _mm256_adds_epi16(src_reg32b1, _mm256_adds_epi16(src_reg_32b8, src_reg_32b12));

        // shift by 6 bit each 16 bit
        src_reg_32b10 = _mm256_adds_epi16(src_reg_32b10, add_filter_reg32);
        src_reg32b1   = _mm256_adds_epi16(src_reg32b1, add_filter_reg32);
        src_reg_32b10 = _mm256_srai_epi16(src_reg_32b10, 6);
        src_reg32b1   = _mm256_srai_epi16(src_reg32b1, 6);

        // shrink to 8 bit each 16 bits, the first lane contain the first
        // convolve result and the second lane contain the second convolve
        // result
        src_reg32b1 = _mm256_packus_epi16(src_reg_32b10, src_reg32b1);

        src_ptr += src_stride;

        xx_store2_mi128(output_ptr, out_pitch, &src_reg32b1);

        output_ptr += dst_stride;

        // save part of the registers for next strides
        src_reg_32b10 = src_reg_32b11;
        src_reg32b1   = src_reg_32b3;
        src_reg_32b11 = src_reg_32b2;
        src_reg_32b3  = src_reg_32b5;
        src_reg_32b2  = src_reg_32b4;
        src_reg_32b5  = src_reg_32b7;
        src_reg_32b7  = src_reg_32b9;
    }
    if (i > 0) {
        __m128i src_reg_filt1, src_reg_filt3, src_reg_filt4, src_reg_filt5;
        __m128i src_reg_filt6, src_reg_filt7, src_reg_filt8;
        // load the last 16 bytes
        src_reg_filt8 = _mm_loadu_si128((const __m128i*)(src_ptr + src_pitch * 7));

        // merge the last 2 results together
        src_reg_filt4 = _mm_unpacklo_epi8(_mm256_castsi256_si128(src_reg_32b7), src_reg_filt8);
        src_reg_filt7 = _mm_unpackhi_epi8(_mm256_castsi256_si128(src_reg_32b7), src_reg_filt8);

        // multiply 2 adjacent elements with the filter and add the result
        src_reg_filt1 = _mm_maddubs_epi16(_mm256_castsi256_si128(src_reg_32b10), _mm256_castsi256_si128(first_filters));
        src_reg_filt4 = _mm_maddubs_epi16(src_reg_filt4, _mm256_castsi256_si128(forth_filters));
        src_reg_filt3 = _mm_maddubs_epi16(_mm256_castsi256_si128(src_reg32b1), _mm256_castsi256_si128(first_filters));
        src_reg_filt7 = _mm_maddubs_epi16(src_reg_filt7, _mm256_castsi256_si128(forth_filters));

        // add and saturate the results together
        src_reg_filt1 = _mm_adds_epi16(src_reg_filt1, src_reg_filt4);
        src_reg_filt3 = _mm_adds_epi16(src_reg_filt3, src_reg_filt7);

        // multiply 2 adjacent elements with the filter and add the result
        src_reg_filt4 = _mm_maddubs_epi16(_mm256_castsi256_si128(src_reg_32b11),
                                          _mm256_castsi256_si128(second_filters));
        src_reg_filt5 = _mm_maddubs_epi16(_mm256_castsi256_si128(src_reg_32b3), _mm256_castsi256_si128(second_filters));

        // multiply 2 adjacent elements with the filter and add the result
        src_reg_filt6 = _mm_maddubs_epi16(_mm256_castsi256_si128(src_reg_32b2), _mm256_castsi256_si128(third_filters));
        src_reg_filt7 = _mm_maddubs_epi16(_mm256_castsi256_si128(src_reg_32b5), _mm256_castsi256_si128(third_filters));

        // add and saturate the results together
        src_reg_filt1 = _mm_adds_epi16(src_reg_filt1, _mm_adds_epi16(src_reg_filt4, src_reg_filt6));
        src_reg_filt3 = _mm_adds_epi16(src_reg_filt3, _mm_adds_epi16(src_reg_filt5, src_reg_filt7));

        // shift by 6 bit each 16 bit
        src_reg_filt1 = _mm_adds_epi16(src_reg_filt1, _mm256_castsi256_si128(add_filter_reg32));
        src_reg_filt3 = _mm_adds_epi16(src_reg_filt3, _mm256_castsi256_si128(add_filter_reg32));
        src_reg_filt1 = _mm_srai_epi16(src_reg_filt1, 6);
        src_reg_filt3 = _mm_srai_epi16(src_reg_filt3, 6);

        // shrink to 8 bit each 16 bits, the first lane contain the first
        // convolve result and the second lane contain the second convolve
        // result
        src_reg_filt1 = _mm_packus_epi16(src_reg_filt1, src_reg_filt3);

        // save 16 bytes
        _mm_storeu_si128((__m128i*)output_ptr, src_reg_filt1);
    }
}

static void svt_aom_filter_block1d4_v4_avx2(const uint8_t* src_ptr, ptrdiff_t src_pitch, uint8_t* output_ptr,
                                            ptrdiff_t out_pitch, uint32_t output_height, const int16_t* filter) {
    __m128i      filters_reg;
    __m256i      filters_reg32, add_filter_reg32;
    __m256i      src_reg23, src_reg_4x, src_reg_34, src_reg_5x, src_reg_45, src_reg_6x, src_reg_56;
    __m256i      src_reg_23_34_lo, src_reg_45_56_lo;
    __m256i      src_reg_2345_3456_lo;
    __m256i      res_reg_lo, res_reg;
    __m256i      first_filters;
    unsigned int i;
    ptrdiff_t    src_stride, dst_stride;

    add_filter_reg32 = _mm256_set1_epi16(32);
    filters_reg      = _mm_loadu_si128((const __m128i*)filter);
    // converting the 16 bit (short) to  8 bit (byte) and have the
    // same data in both lanes of 128 bit register.
    filters_reg = _mm_srai_epi16(filters_reg, 1);
    filters_reg = _mm_packs_epi16(filters_reg, filters_reg);
    // have the same data in both lanes of a 256 bit register
    filters_reg32 = MM256_BROADCASTSI128_SI256(filters_reg);

    first_filters = _mm256_shuffle_epi8(filters_reg32, _mm256_set1_epi32(0x5040302u));

    // multiple the size of the source and destination stride by two
    src_stride = src_pitch << 1;
    dst_stride = out_pitch << 1;

    src_reg23  = xx_loadu2_epi64(src_ptr + src_pitch * 3, src_ptr + src_pitch * 2);
    src_reg_4x = _mm256_castsi128_si256(_mm_loadl_epi64((const __m128i*)(src_ptr + src_pitch * 4)));

    // have consecutive loads on the same 256 register
    src_reg_34 = _mm256_permute2x128_si256(src_reg23, src_reg_4x, 0x21);

    src_reg_23_34_lo = _mm256_unpacklo_epi8(src_reg23, src_reg_34);

    for (i = output_height; i > 1; i -= 2) {
        // load the last 2 loads of 16 bytes and have every two
        // consecutive loads in the same 256 bit register
        src_reg_5x = _mm256_castsi128_si256(_mm_loadl_epi64((const __m128i*)(src_ptr + src_pitch * 5)));
        src_reg_45 = _mm256_inserti128_si256(src_reg_4x, _mm256_castsi256_si128(src_reg_5x), 1);

        src_reg_6x = _mm256_castsi128_si256(_mm_loadl_epi64((const __m128i*)(src_ptr + src_pitch * 6)));
        src_reg_56 = _mm256_inserti128_si256(src_reg_5x, _mm256_castsi256_si128(src_reg_6x), 1);

        // merge every two consecutive registers
        src_reg_45_56_lo = _mm256_unpacklo_epi8(src_reg_45, src_reg_56);

        src_reg_2345_3456_lo = _mm256_unpacklo_epi16(src_reg_23_34_lo, src_reg_45_56_lo);

        // multiply 2 adjacent elements with the filter and add the result
        res_reg_lo = _mm256_maddubs_epi16(src_reg_2345_3456_lo, first_filters);

        res_reg_lo = _mm256_hadds_epi16(res_reg_lo, _mm256_setzero_si256());

        // shift by 6 bit each 16 bit
        res_reg_lo = _mm256_adds_epi16(res_reg_lo, add_filter_reg32);
        res_reg_lo = _mm256_srai_epi16(res_reg_lo, 6);

        // shrink to 8 bit each 16 bits, the first lane contain the first
        // convolve result and the second lane contain the second convolve
        // result
        res_reg = _mm256_packus_epi16(res_reg_lo, res_reg_lo);

        src_ptr += src_stride;

        xx_storeu2_epi32(output_ptr, out_pitch, &res_reg);

        output_ptr += dst_stride;

        // save part of the registers for next strides
        src_reg_23_34_lo = src_reg_45_56_lo;
        src_reg_4x       = src_reg_6x;
    }
}

FUN_CONV_1D(horiz, x_step_q4, filter_x, h, src, , avx2);
FUN_CONV_1D(vert, y_step_q4, filter_y, v, src - src_stride * 3, , avx2);
