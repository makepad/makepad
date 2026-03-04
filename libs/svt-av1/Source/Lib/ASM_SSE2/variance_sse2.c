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
#include "definitions.h"
#include <assert.h>
#include <emmintrin.h> // SSE2
#include "aom_dsp_rtcd.h"
#include "variance_sse2.h"
#include "synonyms.h"
#include "inter_prediction.h"

#ifdef __cplusplus
extern "C" {
#endif

#ifdef __cplusplus
}
#endif

// Can handle 128 pixels' diff sum (such as 8x16 or 16x8)
// Slightly faster than variance_final_256_pel_sse2()
// diff sum of 128 pixels can still fit in 16bit integer
static INLINE void variance_final_128_pel_sse2(__m128i vsse, __m128i vsum, unsigned int* const sse, int* const sum) {
    *sse = add32x4_sse2(vsse);

    vsum = _mm_add_epi16(vsum, _mm_srli_si128(vsum, 8));
    vsum = _mm_add_epi16(vsum, _mm_srli_si128(vsum, 4));
    vsum = _mm_add_epi16(vsum, _mm_srli_si128(vsum, 2));
    *sum = (int16_t)_mm_extract_epi16(vsum, 0);
}

// Can handle 256 pixels' diff sum (such as 16x16)
static INLINE void variance_final_256_pel_sse2(__m128i vsse, __m128i vsum, unsigned int* const sse, int* const sum) {
    *sse = add32x4_sse2(vsse);

    vsum = _mm_add_epi16(vsum, _mm_srli_si128(vsum, 8));
    vsum = _mm_add_epi16(vsum, _mm_srli_si128(vsum, 4));
    *sum = (int16_t)_mm_extract_epi16(vsum, 0);
    *sum += (int16_t)_mm_extract_epi16(vsum, 1);
}

static INLINE void variance_kernel_sse2(const __m128i src, const __m128i ref, __m128i* const sse, __m128i* const sum) {
    const __m128i diff = _mm_sub_epi16(src, ref);
    *sse               = _mm_add_epi32(*sse, _mm_madd_epi16(diff, diff));
    *sum               = _mm_add_epi16(*sum, diff);
}

static INLINE void variance4_sse2(const uint8_t* src, const int src_stride, const uint8_t* ref, const int ref_stride,
                                  const int h, __m128i* const sse, __m128i* const sum) {
    assert(h <= 256); // May overflow for larger height.
    *sum = _mm_setzero_si128();

    for (int i = 0; i < h; i += 2) {
        const __m128i s = load4x2_sse2(src, src_stride);
        const __m128i r = load4x2_sse2(ref, ref_stride);

        variance_kernel_sse2(s, r, sse, sum);
        src += 2 * src_stride;
        ref += 2 * ref_stride;
    }
}

static INLINE void variance8_sse2(const uint8_t* src, const int src_stride, const uint8_t* ref, const int ref_stride,
                                  const int h, __m128i* const sse, __m128i* const sum) {
    assert(h <= 128); // May overflow for larger height.
    *sum = _mm_setzero_si128();
    for (int i = 0; i < h; i++) {
        const __m128i s = load8_8to16_sse2(src);
        const __m128i r = load8_8to16_sse2(ref);

        variance_kernel_sse2(s, r, sse, sum);
        src += src_stride;
        ref += ref_stride;
    }
}

static INLINE void variance16_kernel_sse2(const uint8_t* const src, const uint8_t* const ref, __m128i* const sse,
                                          __m128i* const sum) {
    const __m128i zero = _mm_setzero_si128();
    const __m128i s    = _mm_loadu_si128((const __m128i*)src);
    const __m128i r    = _mm_loadu_si128((const __m128i*)ref);
    const __m128i src0 = _mm_unpacklo_epi8(s, zero);
    const __m128i ref0 = _mm_unpacklo_epi8(r, zero);
    const __m128i src1 = _mm_unpackhi_epi8(s, zero);
    const __m128i ref1 = _mm_unpackhi_epi8(r, zero);

    variance_kernel_sse2(src0, ref0, sse, sum);
    variance_kernel_sse2(src1, ref1, sse, sum);
}

static INLINE void variance16_sse2(const uint8_t* src, const int src_stride, const uint8_t* ref, const int ref_stride,
                                   const int h, __m128i* const sse, __m128i* const sum) {
    assert(h <= 64); // May overflow for larger height.
    *sum = _mm_setzero_si128();

    for (int i = 0; i < h; ++i) {
        variance16_kernel_sse2(src, ref, sse, sum);
        src += src_stride;
        ref += ref_stride;
    }
}

static INLINE void variance32_sse2(const uint8_t* src, const int src_stride, const uint8_t* ref, const int ref_stride,
                                   const int h, __m128i* const sse, __m128i* const sum) {
    assert(h <= 32); // May overflow for larger height.
    // Don't initialize sse here since it's an accumulation.
    *sum = _mm_setzero_si128();

    for (int i = 0; i < h; ++i) {
        variance16_kernel_sse2(src + 0, ref + 0, sse, sum);
        variance16_kernel_sse2(src + 16, ref + 16, sse, sum);
        src += src_stride;
        ref += ref_stride;
    }
}

static INLINE void variance64_sse2(const uint8_t* src, const int src_stride, const uint8_t* ref, const int ref_stride,
                                   const int h, __m128i* const sse, __m128i* const sum) {
    assert(h <= 16); // May overflow for larger height.
    *sum = _mm_setzero_si128();

    for (int i = 0; i < h; ++i) {
        variance16_kernel_sse2(src + 0, ref + 0, sse, sum);
        variance16_kernel_sse2(src + 16, ref + 16, sse, sum);
        variance16_kernel_sse2(src + 32, ref + 32, sse, sum);
        variance16_kernel_sse2(src + 48, ref + 48, sse, sum);
        src += src_stride;
        ref += ref_stride;
    }
}

static INLINE void variance128_sse2(const uint8_t* src, const int src_stride, const uint8_t* ref, const int ref_stride,
                                    const int h, __m128i* const sse, __m128i* const sum) {
    assert(h <= 8); // May overflow for larger height.
    *sum = _mm_setzero_si128();

    for (int i = 0; i < h; ++i) {
        for (int j = 0; j < 4; ++j) {
            const int offset0 = j << 5;
            const int offset1 = offset0 + 16;
            variance16_kernel_sse2(src + offset0, ref + offset0, sse, sum);
            variance16_kernel_sse2(src + offset1, ref + offset1, sse, sum);
        }
        src += src_stride;
        ref += ref_stride;
    }
}

// Can handle 512 pixels' diff sum (such as 16x32 or 32x16)
static INLINE void variance_final_512_pel_sse2(__m128i vsse, __m128i vsum, unsigned int* const sse, int* const sum) {
    *sse = add32x4_sse2(vsse);

    vsum = _mm_add_epi16(vsum, _mm_srli_si128(vsum, 8));
    vsum = _mm_unpacklo_epi16(vsum, vsum);
    vsum = _mm_srai_epi32(vsum, 16);
    *sum = add32x4_sse2(vsum);
}

static INLINE __m128i sum_to_32bit_sse2(const __m128i sum) {
    const __m128i sum_lo = _mm_srai_epi32(_mm_unpacklo_epi16(sum, sum), 16);
    const __m128i sum_hi = _mm_srai_epi32(_mm_unpackhi_epi16(sum, sum), 16);
    return _mm_add_epi32(sum_lo, sum_hi);
}

// Can handle 1024 pixels' diff sum (such as 32x32)
static INLINE void variance_final_1024_pel_sse2(__m128i vsse, __m128i vsum, unsigned int* const sse, int* const sum) {
    *sse = add32x4_sse2(vsse);

    vsum = sum_to_32bit_sse2(vsum);
    *sum = add32x4_sse2(vsum);
}

#define AOM_VAR_NO_LOOP_SSE2(bw, bh, bits, max_pixels)                                               \
    unsigned int svt_aom_variance##bw##x##bh##_sse2(                                                 \
        const uint8_t* src, int src_stride, const uint8_t* ref, int ref_stride, unsigned int* sse) { \
        __m128i vsse = _mm_setzero_si128();                                                          \
        __m128i vsum;                                                                                \
        int     sum = 0;                                                                             \
        variance##bw##_sse2(src, src_stride, ref, ref_stride, bh, &vsse, &vsum);                     \
        variance_final_##max_pixels##_pel_sse2(vsse, vsum, sse, &sum);                               \
        assert(sum <= 255 * bw * bh);                                                                \
        assert(sum >= -255 * bw * bh);                                                               \
        return *sse - (uint32_t)(((int64_t)sum * sum) >> bits);                                      \
    }

AOM_VAR_NO_LOOP_SSE2(4, 4, 4, 128);
AOM_VAR_NO_LOOP_SSE2(4, 8, 5, 128);
AOM_VAR_NO_LOOP_SSE2(4, 16, 6, 128);

AOM_VAR_NO_LOOP_SSE2(8, 4, 5, 128);
AOM_VAR_NO_LOOP_SSE2(8, 8, 6, 128);
AOM_VAR_NO_LOOP_SSE2(8, 16, 7, 128);
AOM_VAR_NO_LOOP_SSE2(8, 32, 8, 256);

AOM_VAR_NO_LOOP_SSE2(16, 4, 6, 128);
AOM_VAR_NO_LOOP_SSE2(16, 8, 7, 128);
AOM_VAR_NO_LOOP_SSE2(16, 16, 8, 256);
AOM_VAR_NO_LOOP_SSE2(16, 32, 9, 512);
AOM_VAR_NO_LOOP_SSE2(16, 64, 10, 1024);

AOM_VAR_NO_LOOP_SSE2(32, 8, 8, 256);
AOM_VAR_NO_LOOP_SSE2(32, 16, 9, 512);
AOM_VAR_NO_LOOP_SSE2(32, 32, 10, 1024);

#define AOM_VAR_LOOP_SSE2(bw, bh, bits, uh)                                                          \
    unsigned int svt_aom_variance##bw##x##bh##_sse2(                                                 \
        const uint8_t* src, int src_stride, const uint8_t* ref, int ref_stride, unsigned int* sse) { \
        __m128i vsse = _mm_setzero_si128();                                                          \
        __m128i vsum = _mm_setzero_si128();                                                          \
        for (int i = 0; i < (bh / uh); ++i) {                                                        \
            __m128i vsum16;                                                                          \
            variance##bw##_sse2(src, src_stride, ref, ref_stride, uh, &vsse, &vsum16);               \
            vsum = _mm_add_epi32(vsum, sum_to_32bit_sse2(vsum16));                                   \
            src += (src_stride * uh);                                                                \
            ref += (ref_stride * uh);                                                                \
        }                                                                                            \
        *sse    = add32x4_sse2(vsse);                                                                \
        int sum = add32x4_sse2(vsum);                                                                \
        assert(sum <= 255 * bw * bh);                                                                \
        assert(sum >= -255 * bw * bh);                                                               \
        return *sse - (uint32_t)(((int64_t)sum * sum) >> bits);                                      \
    }

AOM_VAR_LOOP_SSE2(32, 64, 11, 32); // 32x32 * ( 64/32 )

AOM_VAR_LOOP_SSE2(64, 32, 11, 16); // 64x16 * ( 32/16 )
AOM_VAR_LOOP_SSE2(64, 64, 12, 16); // 64x16 * ( 64/16 )
AOM_VAR_LOOP_SSE2(64, 128, 13, 16); // 64x16 * ( 128/16 )

AOM_VAR_LOOP_SSE2(128, 64, 13, 8); // 128x8 * ( 64/8 )
AOM_VAR_LOOP_SSE2(128, 128, 14, 8); // 128x8 * ( 128/8 )

AOM_VAR_NO_LOOP_SSE2(64, 16, 10, 1024);

// The 2 unused parameters are place holders for PIC enabled build.
// These definitions are for functions defined in subpel_variance.asm
#define DECL(w)                                                           \
    int svt_aom_sub_pixel_variance##w##xh_sse2(const uint8_t* src,        \
                                               ptrdiff_t      src_stride, \
                                               int            x_offset,   \
                                               int            y_offset,   \
                                               const uint8_t* dst,        \
                                               ptrdiff_t      dst_stride, \
                                               int            height,     \
                                               unsigned int*  sse,        \
                                               void*          unused0,    \
                                               void*          unused)
DECL(4);
DECL(8);
DECL(16);

#undef DECL

#define FN(w, h, wf, wlog2, hlog2, cast_prod, cast)                                                          \
    unsigned int svt_aom_sub_pixel_variance##w##x##h##_sse2(const uint8_t* src,                              \
                                                            int            src_stride,                       \
                                                            int            x_offset,                         \
                                                            int            y_offset,                         \
                                                            const uint8_t* dst,                              \
                                                            int            dst_stride,                       \
                                                            unsigned int*  sse_ptr) {                         \
        /*Avoid overflow in helper by capping height.*/                                                      \
        const int    hf  = AOMMIN(h, 64);                                                                    \
        const int    wf2 = AOMMIN(wf, 128);                                                                  \
        unsigned int sse = 0;                                                                                \
        int          se  = 0;                                                                                \
        for (int i = 0; i < (w / wf2); ++i) {                                                                \
            const uint8_t* src_ptr = src;                                                                    \
            const uint8_t* dst_ptr = dst;                                                                    \
            for (int j = 0; j < (h / hf); ++j) {                                                             \
                unsigned int sse2;                                                                           \
                const int    se2 = svt_aom_sub_pixel_variance##wf##xh_sse2(                                  \
                    src_ptr, src_stride, x_offset, y_offset, dst_ptr, dst_stride, hf, &sse2, NULL, NULL); \
                dst_ptr += hf * dst_stride;                                                                  \
                src_ptr += hf * src_stride;                                                                  \
                se += se2;                                                                                   \
                sse += sse2;                                                                                 \
            }                                                                                                \
            src += wf;                                                                                       \
            dst += wf;                                                                                       \
        }                                                                                                    \
        *sse_ptr = sse;                                                                                      \
        return sse - (unsigned int)(cast_prod(cast se * se) >> (wlog2 + hlog2));                             \
    }

FN(128, 128, 16, 7, 7, (int64_t), (int64_t));
FN(128, 64, 16, 7, 6, (int64_t), (int64_t));
FN(64, 128, 16, 6, 7, (int64_t), (int64_t));
FN(64, 64, 16, 6, 6, (int64_t), (int64_t));
FN(64, 32, 16, 6, 5, (int64_t), (int64_t));
FN(32, 64, 16, 5, 6, (int64_t), (int64_t));
FN(32, 32, 16, 5, 5, (int64_t), (int64_t));
FN(32, 16, 16, 5, 4, (int64_t), (int64_t));
FN(16, 32, 16, 4, 5, (int64_t), (int64_t));
FN(16, 16, 16, 4, 4, (uint32_t), (int64_t));
FN(16, 8, 16, 4, 3, (int32_t), (int32_t));
FN(8, 16, 8, 3, 4, (int32_t), (int32_t));
FN(8, 8, 8, 3, 3, (int32_t), (int32_t));
FN(8, 4, 8, 3, 2, (int32_t), (int32_t));
FN(4, 8, 4, 2, 3, (int32_t), (int32_t));
FN(4, 4, 4, 2, 2, (int32_t), (int32_t));
FN(4, 16, 4, 2, 4, (int32_t), (int32_t));
FN(16, 4, 16, 4, 2, (int32_t), (int32_t));
FN(8, 32, 8, 3, 5, (uint32_t), (int64_t));
FN(32, 8, 16, 5, 3, (uint32_t), (int64_t));
FN(16, 64, 16, 4, 6, (int64_t), (int64_t));
FN(64, 16, 16, 6, 4, (int64_t), (int64_t))

#undef FN

static INLINE const InterpFilterParams* av1_get_filter(int subpel_search) {
    assert(subpel_search >= USE_2_TAPS);

    switch (subpel_search) {
    case USE_2_TAPS:
        return &av1_interp_filter_params_list[BILINEAR];
    case USE_4_TAPS:
        return &av1_interp_4tap[EIGHTTAP_REGULAR];
    case USE_8_TAPS:
        return &av1_interp_filter_params_list[EIGHTTAP_REGULAR];
    default:
        assert(0);
        return NULL;
    }
}

void svt_aom_upsampled_pred_sse2(MacroBlockD* xd, const struct AV1Common* const cm, int mi_row, int mi_col,
                                 const Mv* const mv, uint8_t* comp_pred, int width, int height, int subpel_x_q3,
                                 int subpel_y_q3, const uint8_t* ref, int ref_stride, int subpel_search) {
    (void)xd;
    (void)cm;
    (void)mi_row;
    (void)mi_col;
    (void)mv;
    const InterpFilterParams* filter = av1_get_filter(subpel_search);
    assert(filter != NULL);
    int filter_taps = (subpel_search <= USE_4_TAPS) ? 4 : SUBPEL_TAPS;

    if (!subpel_x_q3 && !subpel_y_q3) {
        if (width >= 16) {
            int i;
            assert(!(width & 15));
            /*Read 16 pixels one row at a time.*/
            for (i = 0; i < height; i++) {
                int j;
                for (j = 0; j < width; j += 16) {
                    xx_storeu_128(comp_pred, xx_loadu_128(ref));
                    comp_pred += 16;
                    ref += 16;
                }
                ref += ref_stride - width;
            }
        } else if (width >= 8) {
            int i;
            assert(!(width & 7));
            assert(!(height & 1));
            /*Read 8 pixels two rows at a time.*/
            for (i = 0; i < height; i += 2) {
                __m128i s0 = xx_loadl_64(ref + 0 * ref_stride);
                __m128i s1 = xx_loadl_64(ref + 1 * ref_stride);
                xx_storeu_128(comp_pred, _mm_unpacklo_epi64(s0, s1));
                comp_pred += 16;
                ref += 2 * ref_stride;
            }
        } else {
            int i;
            assert(!(width & 3));
            assert(!(height & 3));
            /*Read 4 pixels four rows at a time.*/
            for (i = 0; i < height; i += 4) {
                const __m128i row0 = xx_loadl_64(ref + 0 * ref_stride);
                const __m128i row1 = xx_loadl_64(ref + 1 * ref_stride);
                const __m128i row2 = xx_loadl_64(ref + 2 * ref_stride);
                const __m128i row3 = xx_loadl_64(ref + 3 * ref_stride);
                const __m128i reg  = _mm_unpacklo_epi64(_mm_unpacklo_epi32(row0, row1), _mm_unpacklo_epi32(row2, row3));
                xx_storeu_128(comp_pred, reg);
                comp_pred += 16;
                ref += 4 * ref_stride;
            }
        }
    } else if (!subpel_y_q3) {
        const int16_t* const kernel = av1_get_interp_filter_subpel_kernel(*filter, subpel_x_q3 << 1);
        svt_aom_convolve8_horiz(ref, ref_stride, comp_pred, width, kernel, 16, NULL, -1, width, height);
    } else if (!subpel_x_q3) {
        const int16_t* const kernel = av1_get_interp_filter_subpel_kernel(*filter, subpel_y_q3 << 1);
        svt_aom_convolve8_vert(ref, ref_stride, comp_pred, width, NULL, -1, kernel, 16, width, height);
    } else {
        DECLARE_ALIGNED(16, uint8_t, temp[((MAX_SB_SIZE * 2 + 16) + 16) * MAX_SB_SIZE]);
        const int16_t* const kernel_x  = av1_get_interp_filter_subpel_kernel(*filter, subpel_x_q3 << 1);
        const int16_t* const kernel_y  = av1_get_interp_filter_subpel_kernel(*filter, subpel_y_q3 << 1);
        const uint8_t*       ref_start = ref - ref_stride * ((filter_taps >> 1) - 1);
        uint8_t* temp_start_horiz      = (subpel_search <= USE_4_TAPS) ? temp + (filter_taps >> 1) * MAX_SB_SIZE : temp;
        uint8_t* temp_start_vert       = temp + MAX_SB_SIZE * ((filter->taps >> 1) - 1);
        int      intermediate_height   = (((height - 1) * 8 + subpel_y_q3) >> 3) + filter_taps;
        assert(intermediate_height <= (MAX_SB_SIZE * 2 + 16) + 16);
        svt_aom_convolve8_horiz(
            ref_start, ref_stride, temp_start_horiz, MAX_SB_SIZE, kernel_x, 16, NULL, -1, width, intermediate_height);
        svt_aom_convolve8_vert(temp_start_vert, MAX_SB_SIZE, comp_pred, width, NULL, -1, kernel_y, 16, width, height);
    }
}

unsigned int svt_aom_mse16x16_sse2(const uint8_t* src, int32_t src_stride, const uint8_t* ref, int32_t ref_stride) {
    uint32_t sse;
    svt_aom_variance16x16_sse2(src, src_stride, ref, ref_stride, &sse);
    return sse;
}
