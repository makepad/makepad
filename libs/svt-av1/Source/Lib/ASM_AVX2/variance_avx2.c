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
#include <immintrin.h>
#include "aom_dsp_rtcd.h"

static INLINE __m128i mm256_add_hi_lo_epi32(const __m256i val) {
    return _mm_add_epi32(_mm256_castsi256_si128(val), _mm256_extractf128_si256(val, 1));
}

static INLINE void variance_kernel_no_sum_avx2(const __m256i src, const __m256i ref, __m256i* const sse) {
    const __m256i adj_sub = _mm256_set1_epi16((short)0xff01); // (1,-1)

    // unpack into pairs of source and reference values
    const __m256i src_ref0 = _mm256_unpacklo_epi8(src, ref);
    const __m256i src_ref1 = _mm256_unpackhi_epi8(src, ref);

    // subtract adjacent elements using src*1 + ref*-1
    const __m256i diff0 = _mm256_maddubs_epi16(src_ref0, adj_sub);
    const __m256i diff1 = _mm256_maddubs_epi16(src_ref1, adj_sub);
    const __m256i madd0 = _mm256_madd_epi16(diff0, diff0);
    const __m256i madd1 = _mm256_madd_epi16(diff1, diff1);

    // add to the running totals
    *sse = _mm256_add_epi32(*sse, _mm256_add_epi32(madd0, madd1));
}

static INLINE uint32_t variance_final_from_32bit_no_sum_avx2(__m256i vsse) {
    // extract the low lane and add it to the high lane
    const __m128i sse_reg_128 = mm256_add_hi_lo_epi32(vsse);
    const __m128i zero        = _mm_setzero_si128();

    // unpack sse and sum registers and add
    const __m128i sse_sum_lo = _mm_unpacklo_epi32(sse_reg_128, zero);
    const __m128i sse_sum_hi = _mm_unpackhi_epi32(sse_reg_128, zero);
    const __m128i sse_sum    = _mm_add_epi32(sse_sum_lo, sse_sum_hi);

    // perform the final summation and extract the results
    const __m128i res = _mm_add_epi32(sse_sum, _mm_srli_si128(sse_sum, 8));
    return _mm_cvtsi128_si32(res);
}

// Horizontal addition for computing the sum in variance calculations.
// Since the sum is squared at the end (see: Var(X) = E[X^2] - E[X]^2), it doesn't matter that this is negative.
static INLINE __m256i neg_sum_to_32bit_avx2(const __m256i sum) {
    // A -1 vector (all bits set) is easier to generate than a 1 vector. Simply cmpeq a register with itself.
    __m256i adj_neg_add = _mm256_set1_epi16(-1);
    return _mm256_madd_epi16(sum, adj_neg_add);
}

static INLINE void variance16_kernel_no_sum_avx2(const uint8_t* const src, const int32_t src_stride,
                                                 const uint8_t* const ref, const int32_t ref_stride,
                                                 __m256i* const sse) {
    const __m128i s0 = _mm_loadu_si128((__m128i const*)(src + 0 * src_stride));
    const __m128i s1 = _mm_loadu_si128((__m128i const*)(src + 1 * src_stride));
    const __m128i r0 = _mm_loadu_si128((__m128i const*)(ref + 0 * ref_stride));
    const __m128i r1 = _mm_loadu_si128((__m128i const*)(ref + 1 * ref_stride));
    const __m256i s  = _mm256_inserti128_si256(_mm256_castsi128_si256(s0), s1, 1);
    const __m256i r  = _mm256_inserti128_si256(_mm256_castsi128_si256(r0), r1, 1);
    variance_kernel_no_sum_avx2(s, r, sse);
}

static INLINE void variance16_no_sum_avx2(const uint8_t* src, const int32_t src_stride, const uint8_t* ref,
                                          const int32_t ref_stride, const int32_t h, __m256i* const vsse) {
    for (int32_t i = 0; i < h; i += 2) {
        variance16_kernel_no_sum_avx2(src, src_stride, ref, ref_stride, vsse);
        src += 2 * src_stride;
        ref += 2 * ref_stride;
    }
}

uint32_t svt_aom_mse16x16_avx2(const uint8_t* src, int32_t src_stride, const uint8_t* ref, int32_t ref_stride) {
    __m256i vsse = _mm256_setzero_si256();
    variance16_no_sum_avx2(src, src_stride, ref, ref_stride, 16, &vsse);
    return variance_final_from_32bit_no_sum_avx2(vsse);
}

static INLINE int variance_final_from_32bit_sum_avx2(__m256i vsse, __m256i vsum, unsigned int* const sse) {
    // unpack sse and sum registers and add
    const __m256i sse_sum_lo = _mm256_unpacklo_epi32(vsse, vsum);
    const __m256i sse_sum_hi = _mm256_unpackhi_epi32(vsse, vsum);
    const __m256i sse_sum    = _mm256_add_epi32(sse_sum_lo, sse_sum_hi);

    // extract the low lane and add it to the high lane
    const __m128i sse_sum_128 = mm256_add_hi_lo_epi32(sse_sum);

    // perform the final summation and extract the results
    const __m128i res = _mm_add_epi32(sse_sum_128, _mm_srli_si128(sse_sum_128, 8));
    *((int*)sse)      = _mm_cvtsi128_si32(res);
    return _mm_extract_epi32(res, 1);
}

static INLINE int variance_final_avx2(__m256i vsse, __m256i vsum, unsigned int* const sse) {
    vsum = neg_sum_to_32bit_avx2(vsum);
    return variance_final_from_32bit_sum_avx2(vsse, vsum, sse);
}

static INLINE void variance8_kernel_avx2(const uint8_t* const src, const int src_stride, const uint8_t* const ref,
                                         const int ref_stride, __m256i* const sse, __m256i* const sum) {
    const __m256i adj_sub = _mm256_set1_epi16(0xff01); // (1,-1)
    // Load operations. Note that the broadcasts are performed on the load port for no extra cost.
    const __m128i s0 = _mm_loadl_epi64((__m128i const*)(src + 0 * src_stride));
    const __m256i s1 = _mm256_broadcastq_epi64(_mm_loadl_epi64((__m128i const*)(src + 1 * src_stride)));
    const __m128i r0 = _mm_loadl_epi64((__m128i const*)(ref + 0 * ref_stride));
    const __m256i r1 = _mm256_broadcastq_epi64(_mm_loadl_epi64((__m128i const*)(ref + 1 * ref_stride)));

    const __m256i s = _mm256_blend_epi32(_mm256_castsi128_si256(s0), s1, 0xf0);
    const __m256i r = _mm256_blend_epi32(_mm256_castsi128_si256(r0), r1, 0xf0);

    // unpack into pairs of source and reference values
    const __m256i src_ref = _mm256_unpacklo_epi8(s, r);
    // subtract adjacent elements using src*1 + ref*-1
    const __m256i diff0 = _mm256_maddubs_epi16(src_ref, adj_sub);
    const __m256i madd0 = _mm256_madd_epi16(diff0, diff0);

    // add to the running totals
    *sum = _mm256_add_epi16(*sum, diff0);
    *sse = _mm256_add_epi32(*sse, madd0);
}

static INLINE void variance_kernel_avx2(const __m256i src, const __m256i ref, __m256i* const sse, __m256i* const sum) {
    const __m256i adj_sub = _mm256_set1_epi16(0xff01); // (1,-1)

    // unpack into pairs of source and reference values
    const __m256i src_ref0 = _mm256_unpacklo_epi8(src, ref);
    const __m256i src_ref1 = _mm256_unpackhi_epi8(src, ref);

    // subtract adjacent elements using src*1 + ref*-1
    const __m256i diff0 = _mm256_maddubs_epi16(src_ref0, adj_sub);
    const __m256i diff1 = _mm256_maddubs_epi16(src_ref1, adj_sub);
    const __m256i madd0 = _mm256_madd_epi16(diff0, diff0);
    const __m256i madd1 = _mm256_madd_epi16(diff1, diff1);

    // add to the running totals
    *sum = _mm256_add_epi16(*sum, _mm256_add_epi16(diff0, diff1));
    *sse = _mm256_add_epi32(*sse, _mm256_add_epi32(madd0, madd1));
}

static INLINE void variance16_kernel_avx2(const uint8_t* const src, const int src_stride, const uint8_t* const ref,
                                          const int ref_stride, __m256i* const sse, __m256i* const sum) {
    const __m128i s0 = _mm_loadu_si128((__m128i const*)(src + 0 * src_stride));
    const __m128i s1 = _mm_loadu_si128((__m128i const*)(src + 1 * src_stride));
    const __m128i r0 = _mm_loadu_si128((__m128i const*)(ref + 0 * ref_stride));
    const __m128i r1 = _mm_loadu_si128((__m128i const*)(ref + 1 * ref_stride));
    const __m256i s  = _mm256_inserti128_si256(_mm256_castsi128_si256(s0), s1, 1);
    const __m256i r  = _mm256_inserti128_si256(_mm256_castsi128_si256(r0), r1, 1);
    variance_kernel_avx2(s, r, sse, sum);
}

static INLINE void variance32_kernel_avx2(const uint8_t* const src, const uint8_t* const ref, __m256i* const sse,
                                          __m256i* const sum) {
    const __m256i s = _mm256_loadu_si256((__m256i const*)(src));
    const __m256i r = _mm256_loadu_si256((__m256i const*)(ref));
    variance_kernel_avx2(s, r, sse, sum);
}

static INLINE void variance8_avx2(const uint8_t* src, const int src_stride, const uint8_t* ref, const int ref_stride,
                                  const int h, __m256i* const vsse, __m256i* const vsum) {
    *vsum = _mm256_setzero_si256();

    for (int i = 0; i < h; i += 2) {
        variance8_kernel_avx2(src, src_stride, ref, ref_stride, vsse, vsum);
        src += 2 * src_stride;
        ref += 2 * ref_stride;
    }
}

static INLINE void variance16_avx2(const uint8_t* src, const int src_stride, const uint8_t* ref, const int ref_stride,
                                   const int h, __m256i* const vsse, __m256i* const vsum) {
    *vsum = _mm256_setzero_si256();

    for (int i = 0; i < h; i += 2) {
        variance16_kernel_avx2(src, src_stride, ref, ref_stride, vsse, vsum);
        src += 2 * src_stride;
        ref += 2 * ref_stride;
    }
}

static INLINE void variance32_avx2(const uint8_t* src, const int src_stride, const uint8_t* ref, const int ref_stride,
                                   const int h, __m256i* const vsse, __m256i* const vsum) {
    *vsum = _mm256_setzero_si256();

    for (int i = 0; i < h; i++) {
        variance32_kernel_avx2(src, ref, vsse, vsum);
        src += src_stride;
        ref += ref_stride;
    }
}

static INLINE void variance64_avx2(const uint8_t* src, const int src_stride, const uint8_t* ref, const int ref_stride,
                                   const int h, __m256i* const vsse, __m256i* const vsum) {
    *vsum = _mm256_setzero_si256();

    for (int i = 0; i < h; i++) {
        variance32_kernel_avx2(src + 0, ref + 0, vsse, vsum);
        variance32_kernel_avx2(src + 32, ref + 32, vsse, vsum);
        src += src_stride;
        ref += ref_stride;
    }
}

static INLINE void variance128_avx2(const uint8_t* src, const int src_stride, const uint8_t* ref, const int ref_stride,
                                    const int h, __m256i* const vsse, __m256i* const vsum) {
    *vsum = _mm256_setzero_si256();

    for (int i = 0; i < h; i++) {
        variance32_kernel_avx2(src + 0, ref + 0, vsse, vsum);
        variance32_kernel_avx2(src + 32, ref + 32, vsse, vsum);
        variance32_kernel_avx2(src + 64, ref + 64, vsse, vsum);
        variance32_kernel_avx2(src + 96, ref + 96, vsse, vsum);
        src += src_stride;
        ref += ref_stride;
    }
}

#define AOM_VAR_NO_LOOP_AVX2(bw, bh, bits)                                                           \
    unsigned int svt_aom_variance##bw##x##bh##_avx2(                                                 \
        const uint8_t* src, int src_stride, const uint8_t* ref, int ref_stride, unsigned int* sse) { \
        __m256i vsse = _mm256_setzero_si256();                                                       \
        __m256i vsum;                                                                                \
        variance##bw##_avx2(src, src_stride, ref, ref_stride, bh, &vsse, &vsum);                     \
        const int sum = variance_final_avx2(vsse, vsum, sse);                                        \
        return *sse - (uint32_t)(((int64_t)sum * sum) >> bits);                                      \
    }

AOM_VAR_NO_LOOP_AVX2(8, 4, 5);
AOM_VAR_NO_LOOP_AVX2(8, 8, 6);
AOM_VAR_NO_LOOP_AVX2(8, 16, 7);
AOM_VAR_NO_LOOP_AVX2(8, 32, 8);

AOM_VAR_NO_LOOP_AVX2(16, 4, 6);
AOM_VAR_NO_LOOP_AVX2(16, 8, 7);
AOM_VAR_NO_LOOP_AVX2(16, 16, 8);
AOM_VAR_NO_LOOP_AVX2(16, 32, 9);
AOM_VAR_NO_LOOP_AVX2(16, 64, 10);

AOM_VAR_NO_LOOP_AVX2(32, 8, 8);
AOM_VAR_NO_LOOP_AVX2(32, 16, 9);
AOM_VAR_NO_LOOP_AVX2(32, 32, 10);
AOM_VAR_NO_LOOP_AVX2(32, 64, 11);

AOM_VAR_NO_LOOP_AVX2(64, 16, 10);
AOM_VAR_NO_LOOP_AVX2(64, 32, 11);

#define AOM_VAR_LOOP_AVX2(bw, bh, bits, uh)                                                          \
    unsigned int svt_aom_variance##bw##x##bh##_avx2(                                                 \
        const uint8_t* src, int src_stride, const uint8_t* ref, int ref_stride, unsigned int* sse) { \
        __m256i vsse = _mm256_setzero_si256();                                                       \
        __m256i vsum = _mm256_setzero_si256();                                                       \
        for (int i = 0; i < (bh / uh); i++) {                                                        \
            __m256i vsum16;                                                                          \
            variance##bw##_avx2(src, src_stride, ref, ref_stride, uh, &vsse, &vsum16);               \
            vsum = _mm256_add_epi32(vsum, neg_sum_to_32bit_avx2(vsum16));                            \
            src += uh * src_stride;                                                                  \
            ref += uh * ref_stride;                                                                  \
        }                                                                                            \
        const int sum = variance_final_from_32bit_sum_avx2(vsse, vsum, sse);                         \
        return *sse - (unsigned int)(((int64_t)sum * sum) >> bits);                                  \
    }

AOM_VAR_LOOP_AVX2(64, 64, 12, 32); // 64x32 * ( 64/32)
AOM_VAR_LOOP_AVX2(64, 128, 13, 32); // 64x32 * (128/32)
AOM_VAR_LOOP_AVX2(128, 64, 13, 16); // 128x16 * ( 64/16)
AOM_VAR_LOOP_AVX2(128, 128, 14, 16); // 128x16 * (128/16)

unsigned int svt_aom_sub_pixel_variance32xh_avx2(const uint8_t* src, int src_stride, int x_offset, int y_offset,
                                                 const uint8_t* dst, int dst_stride, int height, unsigned int* sse);
unsigned int svt_aom_sub_pixel_variance16xh_avx2(const uint8_t* src, int src_stride, int x_offset, int y_offset,
                                                 const uint8_t* dst, int dst_stride, int height, unsigned int* sse);

#define AOM_SUB_PIXEL_VAR_AVX2(w, h, wf, wlog2, hlog2)                                           \
    unsigned int svt_aom_sub_pixel_variance##w##x##h##_avx2(const uint8_t* src,                  \
                                                            int            src_stride,           \
                                                            int            x_offset,             \
                                                            int            y_offset,             \
                                                            const uint8_t* dst,                  \
                                                            int            dst_stride,           \
                                                            unsigned int*  sse_ptr) {             \
        /*Avoid overflow in helper by capping height.*/                                          \
        const int    hf  = AOMMIN(h, 64);                                                        \
        const int    wf2 = AOMMIN(wf, 128);                                                      \
        unsigned int sse = 0;                                                                    \
        int          se  = 0;                                                                    \
        for (int i = 0; i < (w / wf2); ++i) {                                                    \
            const uint8_t* src_ptr = src;                                                        \
            const uint8_t* dst_ptr = dst;                                                        \
            for (int j = 0; j < (h / hf); ++j) {                                                 \
                unsigned int sse2;                                                               \
                const int    se2 = svt_aom_sub_pixel_variance##wf##xh_avx2(                      \
                    src_ptr, src_stride, x_offset, y_offset, dst_ptr, dst_stride, hf, &sse2); \
                dst_ptr += hf * dst_stride;                                                      \
                src_ptr += hf * src_stride;                                                      \
                se += se2;                                                                       \
                sse += sse2;                                                                     \
            }                                                                                    \
            src += wf;                                                                           \
            dst += wf;                                                                           \
        }                                                                                        \
        *sse_ptr = sse;                                                                          \
        return sse - (unsigned int)(((int64_t)se * se) >> (wlog2 + hlog2));                      \
    }

AOM_SUB_PIXEL_VAR_AVX2(128, 128, 32, 7, 7);
AOM_SUB_PIXEL_VAR_AVX2(128, 64, 32, 7, 6);
AOM_SUB_PIXEL_VAR_AVX2(64, 128, 32, 6, 7);
AOM_SUB_PIXEL_VAR_AVX2(64, 64, 32, 6, 6);
AOM_SUB_PIXEL_VAR_AVX2(64, 32, 32, 6, 5);
AOM_SUB_PIXEL_VAR_AVX2(32, 64, 32, 5, 6);
AOM_SUB_PIXEL_VAR_AVX2(32, 32, 32, 5, 5);
AOM_SUB_PIXEL_VAR_AVX2(32, 16, 32, 5, 4);
AOM_SUB_PIXEL_VAR_AVX2(16, 64, 16, 4, 6);
AOM_SUB_PIXEL_VAR_AVX2(16, 32, 16, 4, 5);
AOM_SUB_PIXEL_VAR_AVX2(16, 16, 16, 4, 4);
AOM_SUB_PIXEL_VAR_AVX2(16, 8, 16, 4, 3);
AOM_SUB_PIXEL_VAR_AVX2(16, 4, 16, 4, 2);
