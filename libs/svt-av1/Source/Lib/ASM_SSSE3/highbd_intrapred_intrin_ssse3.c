/*
* Copyright(c) 2019 Intel Corporation
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#include <tmmintrin.h>
#include "definitions.h"
#include "intra_prediction.h"
#include "common_dsp_rtcd.h"

// =============================================================================

// SMOOTH_PRED

// bs = 4
EB_ALIGN(16) static const uint16_t sm_weights_4[8] = {255, 1, 149, 107, 85, 171, 64, 192};

// bs = 8
EB_ALIGN(32)
static const uint16_t sm_weights_8[16] = {255, 1, 197, 59, 146, 110, 105, 151, 73, 183, 50, 206, 37, 219, 32, 224};

// bs = 16
EB_ALIGN(32)
static const uint16_t sm_weights_16[32] = {
    255, 1,   225, 31,  196, 60,  170, 86,  145, 111, 123, 133, 102, 154, 84, 172,
    68,  188, 54,  202, 43,  213, 33,  223, 26,  230, 20,  236, 17,  239, 16, 240,
};

// 4xN

static INLINE void load_right_weights_4(const uint16_t* const above, __m128i* const r, __m128i* const weights) {
    *r       = _mm_set1_epi16((uint16_t)above[3]);
    *weights = _mm_loadu_si128((const __m128i*)sm_weights_4);
}

static INLINE void init_4(const uint16_t* const above, const uint16_t* const left, const int32_t h, __m128i* const ab,
                          __m128i* const r, __m128i* const weights_w, __m128i* const rep) {
    const __m128i a = _mm_loadl_epi64((const __m128i*)above);
    const __m128i b = _mm_set1_epi16((uint16_t)left[h - 1]);
    *ab             = _mm_unpacklo_epi16(a, b);
    load_right_weights_4(above, r, weights_w);

    rep[0] = _mm_set1_epi32(0x03020100);
    rep[1] = _mm_set1_epi32(0x07060504);
    rep[2] = _mm_set1_epi32(0x0B0A0908);
    rep[3] = _mm_set1_epi32(0x0F0E0D0C);
}

static INLINE void load_left_8(const uint16_t* const left, const __m128i r, __m128i* const lr) {
    const __m128i l = _mm_loadu_si128((const __m128i*)left);
    lr[0]           = _mm_unpacklo_epi16(l, r); // 0 1 2 3
    lr[1]           = _mm_unpackhi_epi16(l, r); // 4 5 6 7
}

static INLINE __m128i smooth_pred_4(const __m128i weights_w, const __m128i weights_h, const __m128i rep,
                                    const __m128i ab, const __m128i lr) {
    const __m128i round = _mm_set1_epi32((1 << sm_weight_log2_scale));
    const __m128i w     = _mm_shuffle_epi8(weights_h, rep);
    const __m128i t     = _mm_shuffle_epi8(lr, rep);
    const __m128i s0    = _mm_madd_epi16(ab, w);
    const __m128i s1    = _mm_madd_epi16(t, weights_w);
    __m128i       sum;

    sum = _mm_add_epi32(s0, s1);
    sum = _mm_add_epi32(sum, round);
    sum = _mm_srai_epi32(sum, 1 + sm_weight_log2_scale);
    return sum;
}

static INLINE void smooth_pred_4x2(const __m128i weights_w, const __m128i weights_h, const __m128i* const rep,
                                   const __m128i ab, const __m128i lr, uint16_t** const dst, const ptrdiff_t stride) {
    const __m128i sum0 = smooth_pred_4(weights_w, weights_h, rep[0], ab, lr);
    const __m128i sum1 = smooth_pred_4(weights_w, weights_h, rep[1], ab, lr);
    const __m128i sum  = _mm_packs_epi32(sum0, sum1);
    _mm_storel_epi64((__m128i*)*dst, sum);
    *dst += stride;
    _mm_storeh_pd((double*)(void*)*dst, _mm_castsi128_pd(sum));
    *dst += stride;
}

static INLINE void smooth_pred_4x4(const __m128i weights_w, const __m128i weights_h, const __m128i* const rep,
                                   const __m128i ab, const __m128i lr, uint16_t** const dst, const ptrdiff_t stride) {
    smooth_pred_4x2(weights_w, weights_h, rep + 0, ab, lr, dst, stride);
    smooth_pred_4x2(weights_w, weights_h, rep + 2, ab, lr, dst, stride);
}

// 4x4

void svt_aom_highbd_smooth_predictor_4x4_ssse3(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                               const uint16_t* left, int32_t bd) {
    const __m128i l = _mm_loadl_epi64((const __m128i*)left);
    __m128i       ab, r, lr, weights_w, rep[4];
    (void)bd;

    init_4(above, left, 4, &ab, &r, &weights_w, rep);
    lr = _mm_unpacklo_epi16(l, r);
    smooth_pred_4x4(weights_w, weights_w, rep, ab, lr, &dst, stride);
}

// 4x8

void svt_aom_highbd_smooth_predictor_4x8_ssse3(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                               const uint16_t* left, int32_t bd) {
    __m128i ab, r, lr[2], weights_w, weights_h, rep[4];
    (void)bd;

    init_4(above, left, 8, &ab, &r, &weights_w, rep);
    load_left_8(left, r, lr);
    weights_h = _mm_loadu_si128((const __m128i*)(sm_weights_8 + 0));
    smooth_pred_4x4(weights_w, weights_h, rep, ab, lr[0], &dst, stride);
    weights_h = _mm_loadu_si128((const __m128i*)(sm_weights_8 + 8));
    smooth_pred_4x4(weights_w, weights_h, rep, ab, lr[1], &dst, stride);
}

// 4x16

void svt_aom_highbd_smooth_predictor_4x16_ssse3(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    __m128i ab, r, lr[2], weights_w, weights_h, rep[4];
    (void)bd;

    init_4(above, left, 16, &ab, &r, &weights_w, rep);

    load_left_8(left + 0, r, lr);
    weights_h = _mm_loadu_si128((const __m128i*)(sm_weights_16 + 0));
    smooth_pred_4x4(weights_w, weights_h, rep, ab, lr[0], &dst, stride);
    weights_h = _mm_loadu_si128((const __m128i*)(sm_weights_16 + 8));
    smooth_pred_4x4(weights_w, weights_h, rep, ab, lr[1], &dst, stride);

    load_left_8(left + 8, r, lr);
    weights_h = _mm_loadu_si128((const __m128i*)(sm_weights_16 + 16));
    smooth_pred_4x4(weights_w, weights_h, rep, ab, lr[0], &dst, stride);
    weights_h = _mm_loadu_si128((const __m128i*)(sm_weights_16 + 24));
    smooth_pred_4x4(weights_w, weights_h, rep, ab, lr[1], &dst, stride);
}

// =============================================================================

// SMOOTH_H_PRED

// 4xN

static INLINE __m128i smooth_h_pred_4(const __m128i weights, __m128i* const lr) {
    const __m128i round = _mm_set1_epi32((1 << (sm_weight_log2_scale - 1)));
    const __m128i rep   = _mm_set1_epi32(0x03020100);
    const __m128i t     = _mm_shuffle_epi8(*lr, rep);
    const __m128i sum0  = _mm_madd_epi16(t, weights);
    const __m128i sum1  = _mm_add_epi32(sum0, round);
    const __m128i sum2  = _mm_srai_epi32(sum1, sm_weight_log2_scale);
    *lr                 = _mm_srli_si128(*lr, 4);
    return sum2;
}

static INLINE void smooth_h_pred_4x2(const __m128i weights, __m128i* const lr, uint16_t** const dst,
                                     const ptrdiff_t stride) {
    const __m128i sum0 = smooth_h_pred_4(weights, lr);
    const __m128i sum1 = smooth_h_pred_4(weights, lr);
    const __m128i sum  = _mm_packs_epi32(sum0, sum1);
    _mm_storel_epi64((__m128i*)*dst, sum);
    *dst += stride;
    _mm_storeh_pd((double*)(void*)*dst, _mm_castsi128_pd(sum));
    *dst += stride;
}

static INLINE void smooth_h_pred_4x4(const __m128i weights, __m128i* const lr, uint16_t** const dst,
                                     const ptrdiff_t stride) {
    smooth_h_pred_4x2(weights, lr, dst, stride);
    smooth_h_pred_4x2(weights, lr, dst, stride);
}

// 4x4

void svt_aom_highbd_smooth_h_predictor_4x4_ssse3(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                 const uint16_t* left, int32_t bd) {
    const __m128i l = _mm_loadl_epi64((const __m128i*)left);
    __m128i       r, weights;
    (void)bd;

    load_right_weights_4(above, &r, &weights);
    __m128i lr = _mm_unpacklo_epi16(l, r);
    smooth_h_pred_4x4(weights, &lr, &dst, stride);
}

// 4x8

void svt_aom_highbd_smooth_h_predictor_4x8_ssse3(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                 const uint16_t* left, int32_t bd) {
    __m128i r, lr[2], weights;
    (void)bd;

    load_right_weights_4(above, &r, &weights);
    load_left_8(left, r, lr);
    smooth_h_pred_4x4(weights, &lr[0], &dst, stride);
    smooth_h_pred_4x4(weights, &lr[1], &dst, stride);
}

// 4x16

void svt_aom_highbd_smooth_h_predictor_4x16_ssse3(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                  const uint16_t* left, int32_t bd) {
    __m128i r, lr[2], weights;
    (void)bd;

    load_right_weights_4(above, &r, &weights);
    load_left_8(left + 0, r, lr);
    smooth_h_pred_4x4(weights, &lr[0], &dst, stride);
    smooth_h_pred_4x4(weights, &lr[1], &dst, stride);
    load_left_8(left + 8, r, lr);
    smooth_h_pred_4x4(weights, &lr[0], &dst, stride);
    smooth_h_pred_4x4(weights, &lr[1], &dst, stride);
}

// =============================================================================

// SMOOTH_V_PRED

// 4xN

static INLINE void smooth_v_init_4(const uint16_t* const above, const uint16_t* const left, const int32_t h,
                                   __m128i* const ab, __m128i* const rep) {
    const __m128i a = _mm_loadl_epi64((const __m128i*)above);
    const __m128i b = _mm_set1_epi16((uint16_t)left[h - 1]);
    *ab             = _mm_unpacklo_epi16(a, b);

    rep[0] = _mm_set1_epi32(0x03020100);
    rep[1] = _mm_set1_epi32(0x07060504);
    rep[2] = _mm_set1_epi32(0x0B0A0908);
    rep[3] = _mm_set1_epi32(0x0F0E0D0C);
}

static INLINE __m128i smooth_v_pred_4(const __m128i weights, const __m128i rep, const __m128i ab) {
    const __m128i round = _mm_set1_epi32((1 << (sm_weight_log2_scale - 1)));
    const __m128i w     = _mm_shuffle_epi8(weights, rep);
    const __m128i sum0  = _mm_madd_epi16(ab, w);
    __m128i       sum;

    sum = _mm_add_epi32(sum0, round);
    sum = _mm_srai_epi32(sum, sm_weight_log2_scale);
    return sum;
}

static INLINE void smooth_v_pred_4x2(const __m128i weights, const __m128i* const rep, const __m128i ab,
                                     uint16_t** const dst, const ptrdiff_t stride) {
    const __m128i sum0 = smooth_v_pred_4(weights, rep[0], ab);
    const __m128i sum1 = smooth_v_pred_4(weights, rep[1], ab);
    const __m128i sum  = _mm_packs_epi32(sum0, sum1);
    _mm_storel_epi64((__m128i*)*dst, sum);
    *dst += stride;
    _mm_storeh_pd((double*)(void*)*dst, _mm_castsi128_pd(sum));
    *dst += stride;
}

static INLINE void smooth_v_pred_4x4(const __m128i weights, const __m128i* const rep, const __m128i ab,
                                     uint16_t** const dst, const ptrdiff_t stride) {
    smooth_v_pred_4x2(weights, rep + 0, ab, dst, stride);
    smooth_v_pred_4x2(weights, rep + 2, ab, dst, stride);
}

// 4x4

void svt_aom_highbd_smooth_v_predictor_4x4_ssse3(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                 const uint16_t* left, int32_t bd) {
    __m128i ab, rep[4];
    (void)bd;

    smooth_v_init_4(above, left, 4, &ab, rep);
    const __m128i weights = _mm_loadu_si128((const __m128i*)sm_weights_4);
    smooth_v_pred_4x4(weights, rep, ab, &dst, stride);
}

// 4x8

void svt_aom_highbd_smooth_v_predictor_4x8_ssse3(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                 const uint16_t* left, int32_t bd) {
    __m128i ab, weights, rep[4];
    (void)bd;

    smooth_v_init_4(above, left, 8, &ab, rep);
    weights = _mm_loadu_si128((const __m128i*)(sm_weights_8 + 0));
    smooth_v_pred_4x4(weights, rep, ab, &dst, stride);
    weights = _mm_loadu_si128((const __m128i*)(sm_weights_8 + 8));
    smooth_v_pred_4x4(weights, rep, ab, &dst, stride);
}

// 4x16

void svt_aom_highbd_smooth_v_predictor_4x16_ssse3(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                  const uint16_t* left, int32_t bd) {
    __m128i ab, weights, rep[4];
    (void)bd;

    smooth_v_init_4(above, left, 16, &ab, rep);

    weights = _mm_loadu_si128((const __m128i*)(sm_weights_16 + 0));
    smooth_v_pred_4x4(weights, rep, ab, &dst, stride);
    weights = _mm_loadu_si128((const __m128i*)(sm_weights_16 + 8));
    smooth_v_pred_4x4(weights, rep, ab, &dst, stride);

    weights = _mm_loadu_si128((const __m128i*)(sm_weights_16 + 16));
    smooth_v_pred_4x4(weights, rep, ab, &dst, stride);
    weights = _mm_loadu_si128((const __m128i*)(sm_weights_16 + 24));
    smooth_v_pred_4x4(weights, rep, ab, &dst, stride);
}

// Return 8 16-bit pixels in one row
static INLINE __m128i paeth_8x1_pred(const __m128i* left, const __m128i* top, const __m128i* topleft) {
    const __m128i base = _mm_sub_epi16(_mm_add_epi16(*top, *left), *topleft);

    __m128i pl  = _mm_abs_epi16(_mm_sub_epi16(base, *left));
    __m128i pt  = _mm_abs_epi16(_mm_sub_epi16(base, *top));
    __m128i ptl = _mm_abs_epi16(_mm_sub_epi16(base, *topleft));

    __m128i mask1 = _mm_cmpgt_epi16(pl, pt);
    mask1         = _mm_or_si128(mask1, _mm_cmpgt_epi16(pl, ptl));
    __m128i mask2 = _mm_cmpgt_epi16(pt, ptl);

    pl = _mm_andnot_si128(mask1, *left);

    ptl = _mm_and_si128(mask2, *topleft);
    pt  = _mm_andnot_si128(mask2, *top);
    pt  = _mm_or_si128(pt, ptl);
    pt  = _mm_and_si128(mask1, pt);

    return _mm_or_si128(pl, pt);
}

void svt_aom_paeth_predictor_4x4_ssse3(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    __m128i       l    = _mm_loadl_epi64((const __m128i*)left);
    const __m128i t    = _mm_loadl_epi64((const __m128i*)above);
    const __m128i zero = _mm_setzero_si128();
    const __m128i t16  = _mm_unpacklo_epi8(t, zero);
    const __m128i tl16 = _mm_set1_epi16((uint16_t)above[-1]);
    __m128i       rep  = _mm_set1_epi16(0x8000);
    const __m128i one  = _mm_set1_epi16(1);

    int i;
    for (i = 0; i < 4; ++i) {
        const __m128i l16 = _mm_shuffle_epi8(l, rep);
        const __m128i row = paeth_8x1_pred(&l16, &t16, &tl16);

        *(uint32_t*)dst = _mm_cvtsi128_si32(_mm_packus_epi16(row, row));
        dst += stride;
        rep = _mm_add_epi16(rep, one);
    }
}

void svt_aom_paeth_predictor_4x8_ssse3(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    __m128i       l    = _mm_loadl_epi64((const __m128i*)left);
    const __m128i t    = _mm_loadl_epi64((const __m128i*)above);
    const __m128i zero = _mm_setzero_si128();
    const __m128i t16  = _mm_unpacklo_epi8(t, zero);
    const __m128i tl16 = _mm_set1_epi16((uint16_t)above[-1]);
    __m128i       rep  = _mm_set1_epi16(0x8000);
    const __m128i one  = _mm_set1_epi16(1);

    int i;
    for (i = 0; i < 8; ++i) {
        const __m128i l16 = _mm_shuffle_epi8(l, rep);
        const __m128i row = paeth_8x1_pred(&l16, &t16, &tl16);

        *(uint32_t*)dst = _mm_cvtsi128_si32(_mm_packus_epi16(row, row));
        dst += stride;
        rep = _mm_add_epi16(rep, one);
    }
}

void svt_aom_paeth_predictor_4x16_ssse3(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    __m128i       l    = _mm_loadu_si128((const __m128i*)left);
    const __m128i t    = _mm_cvtsi32_si128(((const uint32_t*)above)[0]);
    const __m128i zero = _mm_setzero_si128();
    const __m128i t16  = _mm_unpacklo_epi8(t, zero);
    const __m128i tl16 = _mm_set1_epi16((uint16_t)above[-1]);
    __m128i       rep  = _mm_set1_epi16(0x8000);
    const __m128i one  = _mm_set1_epi16(1);

    for (int i = 0; i < 16; ++i) {
        const __m128i l16 = _mm_shuffle_epi8(l, rep);
        const __m128i row = paeth_8x1_pred(&l16, &t16, &tl16);

        *(uint32_t*)dst = _mm_cvtsi128_si32(_mm_packus_epi16(row, row));
        dst += stride;
        rep = _mm_add_epi16(rep, one);
    }
}

void svt_aom_paeth_predictor_8x4_ssse3(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    __m128i       l    = _mm_loadl_epi64((const __m128i*)left);
    const __m128i t    = _mm_loadl_epi64((const __m128i*)above);
    const __m128i zero = _mm_setzero_si128();
    const __m128i t16  = _mm_unpacklo_epi8(t, zero);
    const __m128i tl16 = _mm_set1_epi16((uint16_t)above[-1]);
    __m128i       rep  = _mm_set1_epi16(0x8000);
    const __m128i one  = _mm_set1_epi16(1);

    int i;
    for (i = 0; i < 4; ++i) {
        const __m128i l16 = _mm_shuffle_epi8(l, rep);
        const __m128i row = paeth_8x1_pred(&l16, &t16, &tl16);

        _mm_storel_epi64((__m128i*)dst, _mm_packus_epi16(row, row));
        dst += stride;
        rep = _mm_add_epi16(rep, one);
    }
}

void svt_aom_paeth_predictor_8x8_ssse3(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    __m128i       l    = _mm_loadl_epi64((const __m128i*)left);
    const __m128i t    = _mm_loadl_epi64((const __m128i*)above);
    const __m128i zero = _mm_setzero_si128();
    const __m128i t16  = _mm_unpacklo_epi8(t, zero);
    const __m128i tl16 = _mm_set1_epi16((uint16_t)above[-1]);
    __m128i       rep  = _mm_set1_epi16(0x8000);
    const __m128i one  = _mm_set1_epi16(1);

    int i;
    for (i = 0; i < 8; ++i) {
        const __m128i l16 = _mm_shuffle_epi8(l, rep);
        const __m128i row = paeth_8x1_pred(&l16, &t16, &tl16);

        _mm_storel_epi64((__m128i*)dst, _mm_packus_epi16(row, row));
        dst += stride;
        rep = _mm_add_epi16(rep, one);
    }
}

void svt_aom_paeth_predictor_8x16_ssse3(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    __m128i       l    = _mm_loadu_si128((const __m128i*)left);
    const __m128i t    = _mm_loadl_epi64((const __m128i*)above);
    const __m128i zero = _mm_setzero_si128();
    const __m128i t16  = _mm_unpacklo_epi8(t, zero);
    const __m128i tl16 = _mm_set1_epi16((uint16_t)above[-1]);
    __m128i       rep  = _mm_set1_epi16(0x8000);
    const __m128i one  = _mm_set1_epi16(1);

    int i;
    for (i = 0; i < 16; ++i) {
        const __m128i l16 = _mm_shuffle_epi8(l, rep);
        const __m128i row = paeth_8x1_pred(&l16, &t16, &tl16);

        _mm_storel_epi64((__m128i*)dst, _mm_packus_epi16(row, row));
        dst += stride;
        rep = _mm_add_epi16(rep, one);
    }
}

void svt_aom_paeth_predictor_8x32_ssse3(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const __m128i t    = _mm_loadl_epi64((const __m128i*)above);
    const __m128i zero = _mm_setzero_si128();
    const __m128i t16  = _mm_unpacklo_epi8(t, zero);
    const __m128i tl16 = _mm_set1_epi16((uint16_t)above[-1]);
    const __m128i one  = _mm_set1_epi16(1);

    for (int j = 0; j < 2; ++j) {
        const __m128i l   = _mm_loadu_si128((const __m128i*)(left + j * 16));
        __m128i       rep = _mm_set1_epi16(0x8000);
        for (int i = 0; i < 16; ++i) {
            const __m128i l16 = _mm_shuffle_epi8(l, rep);
            const __m128i row = paeth_8x1_pred(&l16, &t16, &tl16);

            _mm_storel_epi64((__m128i*)dst, _mm_packus_epi16(row, row));
            dst += stride;
            rep = _mm_add_epi16(rep, one);
        }
    }
}

// Return 16 8-bit pixels in one row
static INLINE __m128i paeth_16x1_pred(const __m128i* left, const __m128i* top0, const __m128i* top1,
                                      const __m128i* topleft) {
    const __m128i p0 = paeth_8x1_pred(left, top0, topleft);
    const __m128i p1 = paeth_8x1_pred(left, top1, topleft);
    return _mm_packus_epi16(p0, p1);
}

void svt_aom_paeth_predictor_16x4_ssse3(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    __m128i       l    = _mm_cvtsi32_si128(((const uint32_t*)left)[0]);
    const __m128i t    = _mm_loadu_si128((const __m128i*)above);
    const __m128i zero = _mm_setzero_si128();
    const __m128i top0 = _mm_unpacklo_epi8(t, zero);
    const __m128i top1 = _mm_unpackhi_epi8(t, zero);
    const __m128i tl16 = _mm_set1_epi16((uint16_t)above[-1]);
    __m128i       rep  = _mm_set1_epi16(0x8000);
    const __m128i one  = _mm_set1_epi16(1);

    for (int i = 0; i < 4; ++i) {
        const __m128i l16 = _mm_shuffle_epi8(l, rep);
        const __m128i row = paeth_16x1_pred(&l16, &top0, &top1, &tl16);

        _mm_storeu_si128((__m128i*)dst, row);
        dst += stride;
        rep = _mm_add_epi16(rep, one);
    }
}

void svt_aom_paeth_predictor_16x8_ssse3(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    __m128i       l    = _mm_loadl_epi64((const __m128i*)left);
    const __m128i t    = _mm_loadu_si128((const __m128i*)above);
    const __m128i zero = _mm_setzero_si128();
    const __m128i top0 = _mm_unpacklo_epi8(t, zero);
    const __m128i top1 = _mm_unpackhi_epi8(t, zero);
    const __m128i tl16 = _mm_set1_epi16((uint16_t)above[-1]);
    __m128i       rep  = _mm_set1_epi16(0x8000);
    const __m128i one  = _mm_set1_epi16(1);

    int i;
    for (i = 0; i < 8; ++i) {
        const __m128i l16 = _mm_shuffle_epi8(l, rep);
        const __m128i row = paeth_16x1_pred(&l16, &top0, &top1, &tl16);

        _mm_storeu_si128((__m128i*)dst, row);
        dst += stride;
        rep = _mm_add_epi16(rep, one);
    }
}

void svt_aom_paeth_predictor_16x16_ssse3(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    __m128i       l    = _mm_loadu_si128((const __m128i*)left);
    const __m128i t    = _mm_loadu_si128((const __m128i*)above);
    const __m128i zero = _mm_setzero_si128();
    const __m128i top0 = _mm_unpacklo_epi8(t, zero);
    const __m128i top1 = _mm_unpackhi_epi8(t, zero);
    const __m128i tl16 = _mm_set1_epi16((uint16_t)above[-1]);
    __m128i       rep  = _mm_set1_epi16(0x8000);
    const __m128i one  = _mm_set1_epi16(1);

    int i;
    for (i = 0; i < 16; ++i) {
        const __m128i l16 = _mm_shuffle_epi8(l, rep);
        const __m128i row = paeth_16x1_pred(&l16, &top0, &top1, &tl16);

        _mm_storeu_si128((__m128i*)dst, row);
        dst += stride;
        rep = _mm_add_epi16(rep, one);
    }
}

void svt_aom_paeth_predictor_16x32_ssse3(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    __m128i       l    = _mm_loadu_si128((const __m128i*)left);
    const __m128i t    = _mm_loadu_si128((const __m128i*)above);
    const __m128i zero = _mm_setzero_si128();
    const __m128i top0 = _mm_unpacklo_epi8(t, zero);
    const __m128i top1 = _mm_unpackhi_epi8(t, zero);
    const __m128i tl16 = _mm_set1_epi16((uint16_t)above[-1]);
    __m128i       rep  = _mm_set1_epi16(0x8000);
    const __m128i one  = _mm_set1_epi16(1);
    __m128i       l16;

    int i;
    for (i = 0; i < 16; ++i) {
        l16               = _mm_shuffle_epi8(l, rep);
        const __m128i row = paeth_16x1_pred(&l16, &top0, &top1, &tl16);

        _mm_storeu_si128((__m128i*)dst, row);
        dst += stride;
        rep = _mm_add_epi16(rep, one);
    }

    l   = _mm_loadu_si128((const __m128i*)(left + 16));
    rep = _mm_set1_epi16(0x8000);
    for (i = 0; i < 16; ++i) {
        l16               = _mm_shuffle_epi8(l, rep);
        const __m128i row = paeth_16x1_pred(&l16, &top0, &top1, &tl16);

        _mm_storeu_si128((__m128i*)dst, row);
        dst += stride;
        rep = _mm_add_epi16(rep, one);
    }
}

void svt_aom_paeth_predictor_16x64_ssse3(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const __m128i t    = _mm_loadu_si128((const __m128i*)above);
    const __m128i zero = _mm_setzero_si128();
    const __m128i top0 = _mm_unpacklo_epi8(t, zero);
    const __m128i top1 = _mm_unpackhi_epi8(t, zero);
    const __m128i tl16 = _mm_set1_epi16((uint16_t)above[-1]);
    const __m128i one  = _mm_set1_epi16(1);

    for (int j = 0; j < 4; ++j) {
        const __m128i l   = _mm_loadu_si128((const __m128i*)(left + j * 16));
        __m128i       rep = _mm_set1_epi16(0x8000);
        for (int i = 0; i < 16; ++i) {
            const __m128i l16 = _mm_shuffle_epi8(l, rep);
            const __m128i row = paeth_16x1_pred(&l16, &top0, &top1, &tl16);
            _mm_storeu_si128((__m128i*)dst, row);
            dst += stride;
            rep = _mm_add_epi16(rep, one);
        }
    }
}

void svt_aom_paeth_predictor_32x8_ssse3(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const __m128i a    = _mm_loadu_si128((const __m128i*)above);
    const __m128i b    = _mm_loadu_si128((const __m128i*)(above + 16));
    const __m128i zero = _mm_setzero_si128();
    const __m128i al   = _mm_unpacklo_epi8(a, zero);
    const __m128i ah   = _mm_unpackhi_epi8(a, zero);
    const __m128i bl   = _mm_unpacklo_epi8(b, zero);
    const __m128i bh   = _mm_unpackhi_epi8(b, zero);

    const __m128i tl16 = _mm_set1_epi16((uint16_t)above[-1]);
    __m128i       rep  = _mm_set1_epi16(0x8000);
    const __m128i one  = _mm_set1_epi16(1);
    const __m128i l    = _mm_loadl_epi64((const __m128i*)left);
    __m128i       l16;

    for (int i = 0; i < 8; ++i) {
        l16                = _mm_shuffle_epi8(l, rep);
        const __m128i r32l = paeth_16x1_pred(&l16, &al, &ah, &tl16);
        const __m128i r32h = paeth_16x1_pred(&l16, &bl, &bh, &tl16);

        _mm_storeu_si128((__m128i*)dst, r32l);
        _mm_storeu_si128((__m128i*)(dst + 16), r32h);
        dst += stride;
        rep = _mm_add_epi16(rep, one);
    }
}

void svt_aom_paeth_predictor_32x16_ssse3(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const __m128i a    = _mm_loadu_si128((const __m128i*)above);
    const __m128i b    = _mm_loadu_si128((const __m128i*)(above + 16));
    const __m128i zero = _mm_setzero_si128();
    const __m128i al   = _mm_unpacklo_epi8(a, zero);
    const __m128i ah   = _mm_unpackhi_epi8(a, zero);
    const __m128i bl   = _mm_unpacklo_epi8(b, zero);
    const __m128i bh   = _mm_unpackhi_epi8(b, zero);

    const __m128i tl16 = _mm_set1_epi16((uint16_t)above[-1]);
    __m128i       rep  = _mm_set1_epi16(0x8000);
    const __m128i one  = _mm_set1_epi16(1);
    __m128i       l    = _mm_loadu_si128((const __m128i*)left);
    __m128i       l16;

    int i;
    for (i = 0; i < 16; ++i) {
        l16                = _mm_shuffle_epi8(l, rep);
        const __m128i r32l = paeth_16x1_pred(&l16, &al, &ah, &tl16);
        const __m128i r32h = paeth_16x1_pred(&l16, &bl, &bh, &tl16);

        _mm_storeu_si128((__m128i*)dst, r32l);
        _mm_storeu_si128((__m128i*)(dst + 16), r32h);
        dst += stride;
        rep = _mm_add_epi16(rep, one);
    }
}

void svt_aom_paeth_predictor_32x32_ssse3(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const __m128i a    = _mm_loadu_si128((const __m128i*)above);
    const __m128i b    = _mm_loadu_si128((const __m128i*)(above + 16));
    const __m128i zero = _mm_setzero_si128();
    const __m128i al   = _mm_unpacklo_epi8(a, zero);
    const __m128i ah   = _mm_unpackhi_epi8(a, zero);
    const __m128i bl   = _mm_unpacklo_epi8(b, zero);
    const __m128i bh   = _mm_unpackhi_epi8(b, zero);

    const __m128i tl16 = _mm_set1_epi16((uint16_t)above[-1]);
    __m128i       rep  = _mm_set1_epi16(0x8000);
    const __m128i one  = _mm_set1_epi16(1);
    __m128i       l    = _mm_loadu_si128((const __m128i*)left);
    __m128i       l16;

    int i;
    for (i = 0; i < 16; ++i) {
        l16                = _mm_shuffle_epi8(l, rep);
        const __m128i r32l = paeth_16x1_pred(&l16, &al, &ah, &tl16);
        const __m128i r32h = paeth_16x1_pred(&l16, &bl, &bh, &tl16);

        _mm_storeu_si128((__m128i*)dst, r32l);
        _mm_storeu_si128((__m128i*)(dst + 16), r32h);
        dst += stride;
        rep = _mm_add_epi16(rep, one);
    }

    rep = _mm_set1_epi16(0x8000);
    l   = _mm_loadu_si128((const __m128i*)(left + 16));
    for (i = 0; i < 16; ++i) {
        l16                = _mm_shuffle_epi8(l, rep);
        const __m128i r32l = paeth_16x1_pred(&l16, &al, &ah, &tl16);
        const __m128i r32h = paeth_16x1_pred(&l16, &bl, &bh, &tl16);

        _mm_storeu_si128((__m128i*)dst, r32l);
        _mm_storeu_si128((__m128i*)(dst + 16), r32h);
        dst += stride;
        rep = _mm_add_epi16(rep, one);
    }
}

void svt_aom_paeth_predictor_32x64_ssse3(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const __m128i a    = _mm_loadu_si128((const __m128i*)above);
    const __m128i b    = _mm_loadu_si128((const __m128i*)(above + 16));
    const __m128i zero = _mm_setzero_si128();
    const __m128i al   = _mm_unpacklo_epi8(a, zero);
    const __m128i ah   = _mm_unpackhi_epi8(a, zero);
    const __m128i bl   = _mm_unpacklo_epi8(b, zero);
    const __m128i bh   = _mm_unpackhi_epi8(b, zero);

    const __m128i tl16 = _mm_set1_epi16((uint16_t)above[-1]);
    const __m128i one  = _mm_set1_epi16(1);
    __m128i       l16;

    int i, j;
    for (j = 0; j < 4; ++j) {
        const __m128i l   = _mm_loadu_si128((const __m128i*)(left + j * 16));
        __m128i       rep = _mm_set1_epi16(0x8000);
        for (i = 0; i < 16; ++i) {
            l16                = _mm_shuffle_epi8(l, rep);
            const __m128i r32l = paeth_16x1_pred(&l16, &al, &ah, &tl16);
            const __m128i r32h = paeth_16x1_pred(&l16, &bl, &bh, &tl16);

            _mm_storeu_si128((__m128i*)dst, r32l);
            _mm_storeu_si128((__m128i*)(dst + 16), r32h);
            dst += stride;
            rep = _mm_add_epi16(rep, one);
        }
    }
}

void svt_aom_paeth_predictor_64x32_ssse3(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const __m128i a    = _mm_loadu_si128((const __m128i*)above);
    const __m128i b    = _mm_loadu_si128((const __m128i*)(above + 16));
    const __m128i c    = _mm_loadu_si128((const __m128i*)(above + 32));
    const __m128i d    = _mm_loadu_si128((const __m128i*)(above + 48));
    const __m128i zero = _mm_setzero_si128();
    const __m128i al   = _mm_unpacklo_epi8(a, zero);
    const __m128i ah   = _mm_unpackhi_epi8(a, zero);
    const __m128i bl   = _mm_unpacklo_epi8(b, zero);
    const __m128i bh   = _mm_unpackhi_epi8(b, zero);
    const __m128i cl   = _mm_unpacklo_epi8(c, zero);
    const __m128i ch   = _mm_unpackhi_epi8(c, zero);
    const __m128i dl   = _mm_unpacklo_epi8(d, zero);
    const __m128i dh   = _mm_unpackhi_epi8(d, zero);

    const __m128i tl16 = _mm_set1_epi16((uint16_t)above[-1]);
    const __m128i one  = _mm_set1_epi16(1);
    __m128i       l16;

    int i, j;
    for (j = 0; j < 2; ++j) {
        const __m128i l   = _mm_loadu_si128((const __m128i*)(left + j * 16));
        __m128i       rep = _mm_set1_epi16(0x8000);
        for (i = 0; i < 16; ++i) {
            l16              = _mm_shuffle_epi8(l, rep);
            const __m128i r0 = paeth_16x1_pred(&l16, &al, &ah, &tl16);
            const __m128i r1 = paeth_16x1_pred(&l16, &bl, &bh, &tl16);
            const __m128i r2 = paeth_16x1_pred(&l16, &cl, &ch, &tl16);
            const __m128i r3 = paeth_16x1_pred(&l16, &dl, &dh, &tl16);

            _mm_storeu_si128((__m128i*)dst, r0);
            _mm_storeu_si128((__m128i*)(dst + 16), r1);
            _mm_storeu_si128((__m128i*)(dst + 32), r2);
            _mm_storeu_si128((__m128i*)(dst + 48), r3);
            dst += stride;
            rep = _mm_add_epi16(rep, one);
        }
    }
}

void svt_aom_paeth_predictor_64x64_ssse3(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const __m128i a    = _mm_loadu_si128((const __m128i*)above);
    const __m128i b    = _mm_loadu_si128((const __m128i*)(above + 16));
    const __m128i c    = _mm_loadu_si128((const __m128i*)(above + 32));
    const __m128i d    = _mm_loadu_si128((const __m128i*)(above + 48));
    const __m128i zero = _mm_setzero_si128();
    const __m128i al   = _mm_unpacklo_epi8(a, zero);
    const __m128i ah   = _mm_unpackhi_epi8(a, zero);
    const __m128i bl   = _mm_unpacklo_epi8(b, zero);
    const __m128i bh   = _mm_unpackhi_epi8(b, zero);
    const __m128i cl   = _mm_unpacklo_epi8(c, zero);
    const __m128i ch   = _mm_unpackhi_epi8(c, zero);
    const __m128i dl   = _mm_unpacklo_epi8(d, zero);
    const __m128i dh   = _mm_unpackhi_epi8(d, zero);

    const __m128i tl16 = _mm_set1_epi16((uint16_t)above[-1]);
    const __m128i one  = _mm_set1_epi16(1);
    __m128i       l16;

    int i, j;
    for (j = 0; j < 4; ++j) {
        const __m128i l   = _mm_loadu_si128((const __m128i*)(left + j * 16));
        __m128i       rep = _mm_set1_epi16(0x8000);
        for (i = 0; i < 16; ++i) {
            l16              = _mm_shuffle_epi8(l, rep);
            const __m128i r0 = paeth_16x1_pred(&l16, &al, &ah, &tl16);
            const __m128i r1 = paeth_16x1_pred(&l16, &bl, &bh, &tl16);
            const __m128i r2 = paeth_16x1_pred(&l16, &cl, &ch, &tl16);
            const __m128i r3 = paeth_16x1_pred(&l16, &dl, &dh, &tl16);

            _mm_storeu_si128((__m128i*)dst, r0);
            _mm_storeu_si128((__m128i*)(dst + 16), r1);
            _mm_storeu_si128((__m128i*)(dst + 32), r2);
            _mm_storeu_si128((__m128i*)(dst + 48), r3);
            dst += stride;
            rep = _mm_add_epi16(rep, one);
        }
    }
}

void svt_aom_paeth_predictor_64x16_ssse3(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const __m128i a    = _mm_loadu_si128((const __m128i*)above);
    const __m128i b    = _mm_loadu_si128((const __m128i*)(above + 16));
    const __m128i c    = _mm_loadu_si128((const __m128i*)(above + 32));
    const __m128i d    = _mm_loadu_si128((const __m128i*)(above + 48));
    const __m128i zero = _mm_setzero_si128();
    const __m128i al   = _mm_unpacklo_epi8(a, zero);
    const __m128i ah   = _mm_unpackhi_epi8(a, zero);
    const __m128i bl   = _mm_unpacklo_epi8(b, zero);
    const __m128i bh   = _mm_unpackhi_epi8(b, zero);
    const __m128i cl   = _mm_unpacklo_epi8(c, zero);
    const __m128i ch   = _mm_unpackhi_epi8(c, zero);
    const __m128i dl   = _mm_unpacklo_epi8(d, zero);
    const __m128i dh   = _mm_unpackhi_epi8(d, zero);

    const __m128i tl16 = _mm_set1_epi16((uint16_t)above[-1]);
    const __m128i one  = _mm_set1_epi16(1);
    __m128i       l16;

    int           i;
    const __m128i l   = _mm_loadu_si128((const __m128i*)left);
    __m128i       rep = _mm_set1_epi16(0x8000);
    for (i = 0; i < 16; ++i) {
        l16              = _mm_shuffle_epi8(l, rep);
        const __m128i r0 = paeth_16x1_pred(&l16, &al, &ah, &tl16);
        const __m128i r1 = paeth_16x1_pred(&l16, &bl, &bh, &tl16);
        const __m128i r2 = paeth_16x1_pred(&l16, &cl, &ch, &tl16);
        const __m128i r3 = paeth_16x1_pred(&l16, &dl, &dh, &tl16);

        _mm_storeu_si128((__m128i*)dst, r0);
        _mm_storeu_si128((__m128i*)(dst + 16), r1);
        _mm_storeu_si128((__m128i*)(dst + 32), r2);
        _mm_storeu_si128((__m128i*)(dst + 48), r3);
        dst += stride;
        rep = _mm_add_epi16(rep, one);
    }
}
