/*
 * Copyright (c) 2017, Alliance for Open Media. All rights reserved
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

// Note: TranHigh is the datatype used for intermediate transform stages.
typedef int64_t TranHigh;

#define AOM_QM_BITS 5

static INLINE void update_qp(__m256i* qp) {
    int32_t i;
    for (i = 0; i < 5; ++i) {
        qp[i] = _mm256_permute2x128_si256(qp[i], qp[i], 0x11);
    }
}

static INLINE void init_qp_add_shift(const int16_t* zbin_ptr, const int16_t* round_ptr, const int16_t* quant_ptr,
                                     const int16_t* dequant_ptr, const int16_t* quant_shift_ptr, __m256i* qp,
                                     const int add_shift) {
    __m128i       zbin        = _mm_loadu_si128((const __m128i*)zbin_ptr);
    __m128i       round       = _mm_loadu_si128((const __m128i*)round_ptr);
    const __m128i quant       = _mm_loadu_si128((const __m128i*)quant_ptr);
    const __m128i dequant     = _mm_loadu_si128((const __m128i*)dequant_ptr);
    const __m128i quant_shift = _mm_loadu_si128((const __m128i*)quant_shift_ptr);
    if (add_shift) {
        const __m128i add = _mm_set1_epi16((int16_t)add_shift);
        zbin              = _mm_add_epi16(zbin, add);
        round             = _mm_add_epi16(round, add);
        zbin              = _mm_srli_epi16(zbin, add_shift);
        round             = _mm_srli_epi16(round, add_shift);
    }
    qp[0] = _mm256_cvtepi16_epi32(zbin);
    qp[1] = _mm256_cvtepi16_epi32(round);
    qp[2] = _mm256_cvtepi16_epi32(quant);
    qp[3] = _mm256_cvtepi16_epi32(dequant);
    qp[4] = _mm256_cvtepi16_epi32(quant_shift);
}

// Note:
// *x is vector multiplied by *y which is 8 int32_t parallel multiplication
// and right shift 16.  The output, 8 int32_t is save in *p.
static INLINE void mm256_mul_shift_epi32(const __m256i* x, const __m256i* y, __m256i* p, int shift) {
    __m256i prod_lo       = _mm256_mul_epi32(*x, *y);
    prod_lo               = _mm256_srli_epi64(prod_lo, shift);
    __m256i       prod_hi = _mm256_srli_epi64(*x, 32);
    const __m256i mult_hi = _mm256_srli_epi64(*y, 32);
    prod_hi               = _mm256_mul_epi32(prod_hi, mult_hi);

    prod_hi = _mm256_srli_epi64(prod_hi, shift);

    prod_hi = _mm256_slli_epi64(prod_hi, 32);
    *p      = _mm256_blend_epi32(prod_lo, prod_hi, 0xAA); //interleave prod_lo, prod_hi
}

static INLINE void clamp_epi32(__m256i* x, __m256i min, __m256i max) {
    *x = _mm256_min_epi32(*x, max);
    *x = _mm256_max_epi32(*x, min);
}

static INLINE void quantize(const __m256i* qp, __m256i c, const int16_t* iscan_ptr, TranLow* qcoeff, TranLow* dqcoeff,
                            __m256i* eob, __m256i min, __m256i max, int shift_dq) {
    const __m256i zero   = _mm256_setzero_si256();
    const __m256i abs    = _mm256_abs_epi32(c);
    const __m256i flag1  = _mm256_cmpgt_epi32(qp[0], abs);
    const int32_t nzflag = _mm256_movemask_epi8(flag1);

    if (EB_LIKELY(~nzflag)) {
        __m256i q = _mm256_add_epi32(abs, qp[1]);
        clamp_epi32(&q, min, max);
        __m256i tmp;
        mm256_mul_shift_epi32(&q, &qp[2], &tmp, 16);
        q = _mm256_add_epi32(tmp, q);

        mm256_mul_shift_epi32(&q, &qp[4], &q, 16 - shift_dq);
        __m256i dq = _mm256_mullo_epi32(q, qp[3]);
        dq         = _mm256_srli_epi32(dq, shift_dq);

        q  = _mm256_sign_epi32(q, c);
        dq = _mm256_sign_epi32(dq, c);
        q  = _mm256_andnot_si256(flag1, q);
        dq = _mm256_andnot_si256(flag1, dq);

        _mm256_store_si256((__m256i*)qcoeff, q);
        _mm256_store_si256((__m256i*)dqcoeff, dq);

        const __m128i isc   = _mm_loadu_si128((const __m128i*)iscan_ptr);
        const __m256i iscan = _mm256_cvtepi16_epi32(isc);

        const __m256i zc      = _mm256_cmpeq_epi32(dq, zero);
        const __m256i nz      = _mm256_cmpeq_epi32(zc, zero);
        __m256i       cur_eob = _mm256_sub_epi32(iscan, nz);
        cur_eob               = _mm256_and_si256(cur_eob, nz);
        *eob                  = _mm256_max_epi32(cur_eob, *eob);
    } else {
        _mm256_store_si256((__m256i*)qcoeff, zero);
        _mm256_store_si256((__m256i*)dqcoeff, zero);
    }
}

static INLINE void quantize_qm(const __m256i* qp, __m256i c, const int16_t* iscan_ptr, TranLow* qcoeff,
                               TranLow* dqcoeff, __m256i* eob, __m256i min, __m256i max, __m256i qm, __m256i iqm,
                               int shift_dq, const __m256i shift16) {
    const __m256i zero   = _mm256_setzero_si256();
    const __m256i abs    = _mm256_abs_epi32(c);
    const __m256i abs_qm = _mm256_mullo_epi32(abs, qm); // abs * wt
    const __m256i flag1  = _mm256_cmpgt_epi32(qp[0], abs_qm);
    const int32_t nzflag = _mm256_movemask_epi8(flag1);

    if (EB_LIKELY(~nzflag)) {
        __m256i q = _mm256_add_epi32(abs, qp[1]);
        clamp_epi32(&q, min, max);

        __m256i q_wt = _mm256_mullo_epi32(q, qm); // q_wt = q * wt
        __m256i tmp;
        mm256_mul_shift_epi32(&q_wt, &qp[2], &tmp, 16); // tmp = (tmp * qp[2]) >> 16
        q = _mm256_add_epi32(tmp, q_wt); // q = tmp + q_wt
        mm256_mul_shift_epi32(&q, &qp[4], &q, 16 - shift_dq + AOM_QM_BITS); // q = (q * qp[4]) >> (16 - shift_dq + 5)

        __m256i dq = _mm256_mullo_epi32(qp[3], iqm); // dq = qp[3] * iqm
        dq         = _mm256_add_epi32(dq, shift16); // dq = dq + 16
        dq         = _mm256_srli_epi32(dq, AOM_QM_BITS); // dq = dq >> 5
        dq         = _mm256_mullo_epi32(dq, q);
        dq         = _mm256_srli_epi32(dq, shift_dq);

        q  = _mm256_sign_epi32(q, c);
        dq = _mm256_sign_epi32(dq, c);
        q  = _mm256_andnot_si256(flag1, q);
        dq = _mm256_andnot_si256(flag1, dq);

        _mm256_store_si256((__m256i*)qcoeff, q);
        _mm256_store_si256((__m256i*)dqcoeff, dq);

        const __m128i isc   = _mm_loadu_si128((const __m128i*)iscan_ptr);
        const __m256i iscan = _mm256_cvtepi16_epi32(isc);

        const __m256i zc      = _mm256_cmpeq_epi32(dq, zero);
        const __m256i nz      = _mm256_cmpeq_epi32(zc, zero);
        __m256i       cur_eob = _mm256_sub_epi32(iscan, nz);
        cur_eob               = _mm256_and_si256(cur_eob, nz);
        *eob                  = _mm256_max_epi32(cur_eob, *eob);
    } else {
        _mm256_store_si256((__m256i*)qcoeff, zero);
        _mm256_store_si256((__m256i*)dqcoeff, zero);
    }
}

static INLINE __m256i load_bytes_to_m256_avx2(const QmVal* p) {
    __m128i small_load = _mm_loadl_epi64((const __m128i*)p);
    return _mm256_cvtepu8_epi32(small_load);
}

void svt_av1_quantize_b_qm_avx2(const TranLow* coeff_ptr, intptr_t n_coeffs, const int16_t* zbin_ptr,
                                const int16_t* round_ptr, const int16_t* quant_ptr, const int16_t* quant_shift_ptr,
                                TranLow* qcoeff_ptr, TranLow* dqcoeff_ptr, const int16_t* dequant_ptr,
                                uint16_t* eob_ptr, const int16_t* scan, const int16_t* iscan, const QmVal* qm_ptr,
                                const QmVal* iqm_ptr, const int32_t log_scale) {
    (void)scan;
    const uint32_t step = 8;

    __m256i qp[5], coeff;
    init_qp_add_shift(zbin_ptr, round_ptr, quant_ptr, dequant_ptr, quant_shift_ptr, qp, log_scale);
    qp[0] = _mm256_slli_epi32(qp[0], AOM_QM_BITS);
    coeff = _mm256_load_si256((const __m256i*)coeff_ptr);

    __m256i qm, iqm;
    qm  = load_bytes_to_m256_avx2(qm_ptr);
    iqm = load_bytes_to_m256_avx2(iqm_ptr);

    __m256i       eob     = _mm256_setzero_si256();
    __m256i       min     = _mm256_set1_epi32(INT16_MIN);
    __m256i       max     = _mm256_set1_epi32(INT16_MAX);
    const __m256i shift16 = _mm256_set1_epi32(16);
    quantize_qm(qp, coeff, iscan, qcoeff_ptr, dqcoeff_ptr, &eob, min, max, qm, iqm, log_scale, shift16);
    update_qp(qp);

    while (n_coeffs > step) {
        coeff_ptr += step;
        qcoeff_ptr += step;
        dqcoeff_ptr += step;
        iscan += step;
        qm_ptr += step;
        iqm_ptr += step;
        n_coeffs -= step;

        coeff = _mm256_load_si256((const __m256i*)coeff_ptr);
        qm    = load_bytes_to_m256_avx2(qm_ptr);
        iqm   = load_bytes_to_m256_avx2(iqm_ptr);
        quantize_qm(qp, coeff, iscan, qcoeff_ptr, dqcoeff_ptr, &eob, min, max, qm, iqm, log_scale, shift16);
    }
    {
        __m256i eob_s;
        eob_s                   = _mm256_shuffle_epi32(eob, 0xe);
        eob                     = _mm256_max_epi16(eob, eob_s);
        eob_s                   = _mm256_shufflelo_epi16(eob, 0xe);
        eob                     = _mm256_max_epi16(eob, eob_s);
        eob_s                   = _mm256_shufflelo_epi16(eob, 1);
        eob                     = _mm256_max_epi16(eob, eob_s);
        const __m128i final_eob = _mm_max_epi16(_mm256_castsi256_si128(eob), _mm256_extractf128_si256(eob, 1));
        *eob_ptr                = _mm_extract_epi16(final_eob, 0);
    }
}

void svt_aom_quantize_b_avx2(const TranLow* coeff_ptr, intptr_t n_coeffs, const int16_t* zbin_ptr,
                             const int16_t* round_ptr, const int16_t* quant_ptr, const int16_t* quant_shift_ptr,
                             TranLow* qcoeff_ptr, TranLow* dqcoeff_ptr, const int16_t* dequant_ptr, uint16_t* eob_ptr,
                             const int16_t* scan, const int16_t* iscan, const QmVal* qm_ptr, const QmVal* iqm_ptr,
                             const int32_t log_scale) {
    (void)qm_ptr;
    (void)iqm_ptr;
    (void)scan;
    const uint32_t step = 8;

    __m256i qp[5], coeff;
    init_qp_add_shift(zbin_ptr, round_ptr, quant_ptr, dequant_ptr, quant_shift_ptr, qp, log_scale);
    coeff = _mm256_load_si256((const __m256i*)coeff_ptr);

    __m256i eob = _mm256_setzero_si256();
    __m256i min = _mm256_set1_epi32(INT16_MIN);
    __m256i max = _mm256_set1_epi32(INT16_MAX);
    quantize(qp, coeff, iscan, qcoeff_ptr, dqcoeff_ptr, &eob, min, max, log_scale);
    update_qp(qp);

    while (n_coeffs > step) {
        coeff_ptr += step;
        qcoeff_ptr += step;
        dqcoeff_ptr += step;
        iscan += step;
        n_coeffs -= step;

        coeff = _mm256_load_si256((const __m256i*)coeff_ptr);
        quantize(qp, coeff, iscan, qcoeff_ptr, dqcoeff_ptr, &eob, min, max, log_scale);
    }
    {
        __m256i eob_s;
        eob_s                   = _mm256_shuffle_epi32(eob, 0xe);
        eob                     = _mm256_max_epi16(eob, eob_s);
        eob_s                   = _mm256_shufflelo_epi16(eob, 0xe);
        eob                     = _mm256_max_epi16(eob, eob_s);
        eob_s                   = _mm256_shufflelo_epi16(eob, 1);
        eob                     = _mm256_max_epi16(eob, eob_s);
        const __m128i final_eob = _mm_max_epi16(_mm256_castsi256_si128(eob), _mm256_extractf128_si256(eob, 1));
        *eob_ptr                = _mm_extract_epi16(final_eob, 0);
    }
}

static INLINE void quantize_highbd_qm(const __m256i* qp, __m256i c, const int16_t* iscan_ptr, TranLow* qcoeff,
                                      TranLow* dqcoeff, __m256i* eob, int shift_dq, const __m256i wt, const __m256i iwt,
                                      const __m256i shift16) {
    const __m256i zero   = _mm256_setzero_si256();
    const __m256i abs    = _mm256_abs_epi32(c);
    const __m256i abs_wt = _mm256_mullo_epi32(abs, wt);
    const __m256i flag1  = _mm256_cmpgt_epi32(qp[0], abs_wt);
    const int32_t nzflag = _mm256_movemask_epi8(flag1);

    if (EB_LIKELY(~nzflag)) {
        __m256i q = _mm256_add_epi32(abs, qp[1]);
        q         = _mm256_mullo_epi32(q, wt);
        __m256i tmp;
        mm256_mul_shift_epi32(&q, &qp[2], &tmp, 16);
        q = _mm256_add_epi32(tmp, q);
        mm256_mul_shift_epi32(&q,
                              &qp[4],
                              &q,
                              16 - shift_dq + AOM_QM_BITS); // q = (q * quant_shift) >> (16 - shift_dq + 5)

        __m256i dq = _mm256_mullo_epi32(qp[3], iwt);
        dq         = _mm256_add_epi32(dq, shift16); // dq = dq + 16
        dq         = _mm256_srli_epi32(dq, AOM_QM_BITS);
        dq         = _mm256_mullo_epi32(dq, q);
        dq         = _mm256_srli_epi32(dq, shift_dq);

        q  = _mm256_sign_epi32(q, c);
        dq = _mm256_sign_epi32(dq, c);
        q  = _mm256_andnot_si256(flag1, q);
        dq = _mm256_andnot_si256(flag1, dq);

        _mm256_store_si256((__m256i*)qcoeff, q);
        _mm256_store_si256((__m256i*)dqcoeff, dq);

        const __m128i isc   = _mm_loadu_si128((const __m128i*)iscan_ptr);
        const __m256i iscan = _mm256_cvtepi16_epi32(isc);

        const __m256i zc      = _mm256_cmpeq_epi32(dq, zero);
        const __m256i nz      = _mm256_cmpeq_epi32(zc, zero);
        __m256i       cur_eob = _mm256_sub_epi32(iscan, nz);
        cur_eob               = _mm256_and_si256(cur_eob, nz);
        *eob                  = _mm256_max_epi32(cur_eob, *eob);
    } else {
        _mm256_store_si256((__m256i*)qcoeff, zero);
        _mm256_store_si256((__m256i*)dqcoeff, zero);
    }
}

void svt_av1_highbd_quantize_b_qm_avx2(const TranLow* coeff_ptr, intptr_t n_coeffs, const int16_t* zbin_ptr,
                                       const int16_t* round_ptr, const int16_t* quant_ptr,
                                       const int16_t* quant_shift_ptr, TranLow* qcoeff_ptr, TranLow* dqcoeff_ptr,
                                       const int16_t* dequant_ptr, uint16_t* eob_ptr, const int16_t* scan,
                                       const int16_t* iscan, const QmVal* qm_ptr, const QmVal* iqm_ptr,
                                       const int32_t log_scale) {
    (void)scan;
    const uint32_t step = 8;

    __m256i qp[5], coeff, qm, iqm;
    init_qp_add_shift(zbin_ptr, round_ptr, quant_ptr, dequant_ptr, quant_shift_ptr, qp, log_scale);
    qp[0] = _mm256_slli_epi32(qp[0], AOM_QM_BITS); // zbin = zbin << 5
    coeff = _mm256_load_si256((const __m256i*)coeff_ptr);
    qm    = load_bytes_to_m256_avx2(qm_ptr);
    iqm   = load_bytes_to_m256_avx2(iqm_ptr);

    __m256i       eob     = _mm256_setzero_si256();
    const __m256i shift16 = _mm256_set1_epi32(16);
    quantize_highbd_qm(qp, coeff, iscan, qcoeff_ptr, dqcoeff_ptr, &eob, log_scale, qm, iqm, shift16);
    update_qp(qp);

    while (n_coeffs > step) {
        coeff_ptr += step;
        qcoeff_ptr += step;
        dqcoeff_ptr += step;
        qm_ptr += step;
        iqm_ptr += step;
        iscan += step;
        n_coeffs -= step;
        coeff = _mm256_load_si256((const __m256i*)coeff_ptr);
        qm    = load_bytes_to_m256_avx2(qm_ptr);
        iqm   = load_bytes_to_m256_avx2(iqm_ptr);
        quantize_highbd_qm(qp, coeff, iscan, qcoeff_ptr, dqcoeff_ptr, &eob, log_scale, qm, iqm, shift16);
    }
    {
        __m256i eob_s;
        eob_s                   = _mm256_shuffle_epi32(eob, 0xe);
        eob                     = _mm256_max_epi16(eob, eob_s);
        eob_s                   = _mm256_shufflelo_epi16(eob, 0xe);
        eob                     = _mm256_max_epi16(eob, eob_s);
        eob_s                   = _mm256_shufflelo_epi16(eob, 1);
        eob                     = _mm256_max_epi16(eob, eob_s);
        const __m128i final_eob = _mm_max_epi16(_mm256_castsi256_si128(eob), _mm256_extractf128_si256(eob, 1));
        *eob_ptr                = _mm_extract_epi16(final_eob, 0);
    }
}

static INLINE void quantize_highbd(const __m256i* qp, __m256i c, const int16_t* iscan_ptr, TranLow* qcoeff,
                                   TranLow* dqcoeff, __m256i* eob, int shift_dq) {
    const __m256i zero   = _mm256_setzero_si256();
    const __m256i abs    = _mm256_abs_epi32(c);
    const __m256i flag1  = _mm256_cmpgt_epi32(qp[0], abs);
    const int32_t nzflag = _mm256_movemask_epi8(flag1);

    if (EB_LIKELY(~nzflag)) {
        __m256i q = _mm256_add_epi32(abs, qp[1]);
        __m256i tmp;
        mm256_mul_shift_epi32(&q, &qp[2], &tmp, 16);
        q = _mm256_add_epi32(tmp, q);

        mm256_mul_shift_epi32(&q, &qp[4], &q, 16 - shift_dq);
        __m256i dq = _mm256_mullo_epi32(q, qp[3]);
        dq         = _mm256_srli_epi32(dq, shift_dq);

        q  = _mm256_sign_epi32(q, c);
        dq = _mm256_sign_epi32(dq, c);
        q  = _mm256_andnot_si256(flag1, q);
        dq = _mm256_andnot_si256(flag1, dq);

        _mm256_store_si256((__m256i*)qcoeff, q);
        _mm256_store_si256((__m256i*)dqcoeff, dq);

        const __m128i isc   = _mm_loadu_si128((const __m128i*)iscan_ptr);
        const __m256i iscan = _mm256_cvtepi16_epi32(isc);

        const __m256i zc      = _mm256_cmpeq_epi32(dq, zero);
        const __m256i nz      = _mm256_cmpeq_epi32(zc, zero);
        __m256i       cur_eob = _mm256_sub_epi32(iscan, nz);
        cur_eob               = _mm256_and_si256(cur_eob, nz);
        *eob                  = _mm256_max_epi32(cur_eob, *eob);
    } else {
        _mm256_store_si256((__m256i*)qcoeff, zero);
        _mm256_store_si256((__m256i*)dqcoeff, zero);
    }
}

void svt_aom_highbd_quantize_b_avx2(const TranLow* coeff_ptr, intptr_t n_coeffs, const int16_t* zbin_ptr,
                                    const int16_t* round_ptr, const int16_t* quant_ptr, const int16_t* quant_shift_ptr,
                                    TranLow* qcoeff_ptr, TranLow* dqcoeff_ptr, const int16_t* dequant_ptr,
                                    uint16_t* eob_ptr, const int16_t* scan, const int16_t* iscan, const QmVal* qm_ptr,
                                    const QmVal* iqm_ptr, const int32_t log_scale) {
    (void)qm_ptr;
    (void)iqm_ptr;
    (void)scan;
    const uint32_t step = 8;

    __m256i qp[5], coeff;
    init_qp_add_shift(zbin_ptr, round_ptr, quant_ptr, dequant_ptr, quant_shift_ptr, qp, log_scale);
    coeff = _mm256_load_si256((const __m256i*)coeff_ptr);

    __m256i eob = _mm256_setzero_si256();
    quantize_highbd(qp, coeff, iscan, qcoeff_ptr, dqcoeff_ptr, &eob, log_scale);
    update_qp(qp);

    while (n_coeffs > step) {
        coeff_ptr += step;
        qcoeff_ptr += step;
        dqcoeff_ptr += step;
        iscan += step;
        n_coeffs -= step;
        coeff = _mm256_load_si256((const __m256i*)coeff_ptr);
        quantize_highbd(qp, coeff, iscan, qcoeff_ptr, dqcoeff_ptr, &eob, log_scale);
    }
    {
        __m256i eob_s;
        eob_s                   = _mm256_shuffle_epi32(eob, 0xe);
        eob                     = _mm256_max_epi16(eob, eob_s);
        eob_s                   = _mm256_shufflelo_epi16(eob, 0xe);
        eob                     = _mm256_max_epi16(eob, eob_s);
        eob_s                   = _mm256_shufflelo_epi16(eob, 1);
        eob                     = _mm256_max_epi16(eob, eob_s);
        const __m128i final_eob = _mm_max_epi16(_mm256_castsi256_si128(eob), _mm256_extractf128_si256(eob, 1));
        *eob_ptr                = _mm_extract_epi16(final_eob, 0);
    }
}

static INLINE void init_one_qp_fp(const __m128i* p, __m256i* qp) {
    const __m128i zero = _mm_setzero_si128();
    const __m128i dc   = _mm_unpacklo_epi16(*p, zero);
    const __m128i ac   = _mm_unpackhi_epi16(*p, zero);
    *qp                = _mm256_insertf128_si256(_mm256_castsi128_si256(dc), ac, 1);
}

static INLINE void init_qp_fp(const int16_t* round_ptr, const int16_t* quant_ptr, const int16_t* dequant_ptr,
                              int log_scale, __m256i* qp) {
    __m128i round = _mm_loadu_si128((const __m128i*)round_ptr);
    if (log_scale) {
        const __m128i round_scale = _mm_set1_epi16(1 << (15 - log_scale));
        round                     = _mm_mulhrs_epi16(round, round_scale);
    }
    const __m128i quant   = _mm_loadu_si128((const __m128i*)quant_ptr);
    const __m128i dequant = _mm_loadu_si128((const __m128i*)dequant_ptr);

    init_one_qp_fp(&round, &qp[0]);
    init_one_qp_fp(&quant, &qp[1]);
    init_one_qp_fp(&dequant, &qp[2]);
}

static INLINE void quantize_highbd_fp(const __m256i* qp, __m256i* c, const int16_t* iscan_ptr, int log_scale,
                                      TranLow* qcoeff, TranLow* dqcoeff, __m256i* eob) {
    const __m256i abs_coeff = _mm256_abs_epi32(*c);
    __m256i       q         = _mm256_add_epi32(abs_coeff, qp[0]);

    __m256i       q_lo  = _mm256_mul_epi32(q, qp[1]);
    __m256i       q_hi  = _mm256_srli_epi64(q, 32);
    const __m256i qp_hi = _mm256_srli_epi64(qp[1], 32);
    q_hi                = _mm256_mul_epi32(q_hi, qp_hi);
    q_lo                = _mm256_srli_epi64(q_lo, 16 - log_scale);
    q_hi                = _mm256_srli_epi64(q_hi, 16 - log_scale);
    q_hi                = _mm256_slli_epi64(q_hi, 32);
    q                   = _mm256_or_si256(q_lo, q_hi);
    const __m256i abs_s = _mm256_slli_epi32(abs_coeff, 1 + log_scale);
    const __m256i mask  = _mm256_cmpgt_epi32(qp[2], abs_s);
    q                   = _mm256_andnot_si256(mask, q);

    __m256i dq = _mm256_mullo_epi32(q, qp[2]);
    dq         = _mm256_srai_epi32(dq, log_scale);
    q          = _mm256_sign_epi32(q, *c);
    dq         = _mm256_sign_epi32(dq, *c);

    _mm256_storeu_si256((__m256i*)qcoeff, q);
    _mm256_storeu_si256((__m256i*)dqcoeff, dq);

    const __m128i isc   = _mm_loadu_si128((const __m128i*)iscan_ptr);
    const __m128i zr    = _mm_setzero_si128();
    const __m128i lo    = _mm_unpacklo_epi16(isc, zr);
    const __m128i hi    = _mm_unpackhi_epi16(isc, zr);
    const __m256i iscan = _mm256_insertf128_si256(_mm256_castsi128_si256(lo), hi, 1);

    const __m256i zero    = _mm256_setzero_si256();
    const __m256i zc      = _mm256_cmpeq_epi32(dq, zero);
    const __m256i nz      = _mm256_cmpeq_epi32(zc, zero);
    __m256i       cur_eob = _mm256_sub_epi32(iscan, nz);
    cur_eob               = _mm256_and_si256(cur_eob, nz);
    *eob                  = _mm256_max_epi32(cur_eob, *eob);
}

static INLINE void update_qp_fp(__m256i* qp) {
    qp[0] = _mm256_permute2x128_si256(qp[0], qp[0], 0x11);
    qp[1] = _mm256_permute2x128_si256(qp[1], qp[1], 0x11);
    qp[2] = _mm256_permute2x128_si256(qp[2], qp[2], 0x11);
}

void svt_av1_highbd_quantize_fp_avx2(const TranLow* coeff_ptr, intptr_t n_coeffs, const int16_t* zbin_ptr,
                                     const int16_t* round_ptr, const int16_t* quant_ptr, const int16_t* quant_shift_ptr,
                                     TranLow* qcoeff_ptr, TranLow* dqcoeff_ptr, const int16_t* dequant_ptr,
                                     uint16_t* eob_ptr, const int16_t* scan, const int16_t* iscan, int16_t log_scale) {
    (void)scan;
    (void)zbin_ptr;
    (void)quant_shift_ptr;
    const unsigned int step = 8;
    __m256i            qp[3], coeff;

    init_qp_fp(round_ptr, quant_ptr, dequant_ptr, log_scale, qp);
    coeff = _mm256_loadu_si256((const __m256i*)coeff_ptr);

    __m256i eob = _mm256_setzero_si256();
    quantize_highbd_fp(qp, &coeff, iscan, log_scale, qcoeff_ptr, dqcoeff_ptr, &eob);

    coeff_ptr += step;
    qcoeff_ptr += step;
    dqcoeff_ptr += step;
    iscan += step;
    n_coeffs -= step;

    update_qp_fp(qp);
    while (n_coeffs > 0) {
        coeff = _mm256_loadu_si256((const __m256i*)coeff_ptr);
        quantize_highbd_fp(qp, &coeff, iscan, log_scale, qcoeff_ptr, dqcoeff_ptr, &eob);

        coeff_ptr += step;
        qcoeff_ptr += step;
        dqcoeff_ptr += step;
        iscan += step;
        n_coeffs -= step;
    }

    {
        __m256i eob_s;
        eob_s                   = _mm256_shuffle_epi32(eob, 0xe);
        eob                     = _mm256_max_epi16(eob, eob_s);
        eob_s                   = _mm256_shufflelo_epi16(eob, 0xe);
        eob                     = _mm256_max_epi16(eob, eob_s);
        eob_s                   = _mm256_shufflelo_epi16(eob, 1);
        eob                     = _mm256_max_epi16(eob, eob_s);
        const __m128i final_eob = _mm_max_epi16(_mm256_castsi256_si128(eob), _mm256_extractf128_si256(eob, 1));
        *eob_ptr                = _mm_extract_epi16(final_eob, 0);
    }
}

// 64 bit multiply. return the low 64 bits of the intermediate integers
static inline __m256i mm256_mullo_epi64(const __m256i a, const __m256i b) {
    // if a 64bit integer 'a' can be represented by its low 32bit part a0 and high 32bit part a1 as: a1<<32+a0,
    // 64bit integer multiply a*b can expand to: (a1*b1)<<64 + (a1*b0 + a0*b1)<<32 + a0*b0.
    // since only the low 64bit part of the result 128bit integer is needed, the above expression can be simplified as: (a1*b0 + a0*b1)<<32 + a0*b0
    const __m256i bswap   = _mm256_shuffle_epi32(b, 0xB1); // b6 b7 b4 b5 b2 b3 b0 b1
    __m256i       prod_hi = _mm256_mullo_epi32(a,
                                         bswap); // a7*b6 a6*b7 a5*b4 a4*b5 a3*b2 a2*b3 a1*b0 a0*b1
    const __m256i zero    = _mm256_setzero_si256();
    prod_hi               = _mm256_hadd_epi32(prod_hi,
                                zero); // 0 0 a7*b6+a6*b7 a5*b4+a4*b5 0 0 a3*b2+a2*b3 a1*b0+a0*b1
    prod_hi               = _mm256_shuffle_epi32(prod_hi,
                                   0x73); // a7*b6+a6*b7 0 a5*b4+a4*b5 0 a3*b2+a2*b3 0 a1*b0+a0*b1 0
    const __m256i prod_lo = _mm256_mul_epu32(a, b); // 0 a6*b6 0 a4*b4 0 a2*b2 0 a0*b0
    const __m256i prod    = _mm256_add_epi64(prod_lo, prod_hi);
    return prod;
}

static INLINE void quantize_highbd_fp_qm(const __m256i* qp, __m256i* c, const int16_t* iscan_ptr, int log_scale,
                                         TranLow* qcoeff, TranLow* dqcoeff, __m256i* eob, const __m256i qm,
                                         const __m256i iqm) {
    const __m256i abs_coeff = _mm256_abs_epi32(*c);
    __m256i       q         = _mm256_add_epi32(abs_coeff, qp[0]);

    const __m256i wt_hi = _mm256_srli_epi64(qm, 32);
    const __m256i wt_lo = _mm256_srli_epi64(_mm256_slli_epi64(qm, 32), 32);

    __m256i q_lo = _mm256_mul_epi32(q, qp[1]);
    q_lo         = mm256_mullo_epi64(q_lo, wt_lo);

    __m256i       q_hi  = _mm256_srli_epi64(q, 32);
    const __m256i qp_hi = _mm256_srli_epi64(qp[1], 32);
    q_hi                = _mm256_mul_epi32(q_hi, qp_hi);
    q_hi                = mm256_mullo_epi64(q_hi, wt_hi);

    q_lo = _mm256_srli_epi64(q_lo, 16 - log_scale + AOM_QM_BITS);
    q_hi = _mm256_srli_epi64(q_hi, 16 - log_scale + AOM_QM_BITS);
    q_hi = _mm256_slli_epi64(q_hi, 32);
    q    = _mm256_or_si256(q_lo, q_hi);

    const __m256i abs_s = _mm256_mullo_epi32(abs_coeff, qm);
    __m256i       mask  = _mm256_slli_epi32(qp[2], AOM_QM_BITS - (1 + log_scale));
    mask                = _mm256_cmpgt_epi32(mask, abs_s);
    q                   = _mm256_andnot_si256(mask, q);

    __m256i       dq  = _mm256_mullo_epi32(qp[2], iqm);
    const __m256i a16 = _mm256_set1_epi32(16);
    dq                = _mm256_add_epi32(dq, a16);
    dq                = _mm256_srli_epi32(dq, AOM_QM_BITS);
    dq                = _mm256_mullo_epi32(q, dq);
    dq                = _mm256_srai_epi32(dq, log_scale);
    q                 = _mm256_sign_epi32(q, *c);
    dq                = _mm256_sign_epi32(dq, *c);

    _mm256_storeu_si256((__m256i*)qcoeff, q);
    _mm256_storeu_si256((__m256i*)dqcoeff, dq);

    const __m128i isc   = _mm_loadu_si128((const __m128i*)iscan_ptr);
    const __m128i zr    = _mm_setzero_si128();
    const __m128i lo    = _mm_unpacklo_epi16(isc, zr);
    const __m128i hi    = _mm_unpackhi_epi16(isc, zr);
    const __m256i iscan = _mm256_insertf128_si256(_mm256_castsi128_si256(lo), hi, 1);

    const __m256i zero    = _mm256_setzero_si256();
    const __m256i zc      = _mm256_cmpeq_epi32(dq, zero);
    const __m256i nz      = _mm256_cmpeq_epi32(zc, zero);
    __m256i       cur_eob = _mm256_sub_epi32(iscan, nz);
    cur_eob               = _mm256_and_si256(cur_eob, nz);
    *eob                  = _mm256_max_epi32(cur_eob, *eob);
}

void svt_av1_highbd_quantize_fp_qm_avx2(const TranLow* coeff_ptr, intptr_t n_coeffs, const int16_t* zbin_ptr,
                                        const int16_t* round_ptr, const int16_t* quant_ptr,
                                        const int16_t* quant_shift_ptr, TranLow* qcoeff_ptr, TranLow* dqcoeff_ptr,
                                        const int16_t* dequant_ptr, uint16_t* eob_ptr, const int16_t* scan,
                                        const int16_t* iscan, const QmVal* qm_ptr, const QmVal* iqm_ptr,
                                        int16_t log_scale) {
    (void)scan;
    (void)zbin_ptr;
    (void)quant_shift_ptr;
    const unsigned int step = 8;
    __m256i            qp[3];
    init_qp_fp(round_ptr, quant_ptr, dequant_ptr, log_scale, qp);
    __m256i eob = _mm256_setzero_si256();

    __m256i coeff = _mm256_loadu_si256((const __m256i*)coeff_ptr);
    __m256i qm    = load_bytes_to_m256_avx2(qm_ptr);
    __m256i iqm   = load_bytes_to_m256_avx2(iqm_ptr);
    quantize_highbd_fp_qm(qp, &coeff, iscan, log_scale, qcoeff_ptr, dqcoeff_ptr, &eob, qm, iqm);

    update_qp_fp(qp);
    while (n_coeffs > step) {
        coeff_ptr += step;
        qcoeff_ptr += step;
        dqcoeff_ptr += step;
        iscan += step;
        qm_ptr += step;
        iqm_ptr += step;
        n_coeffs -= step;

        coeff = _mm256_loadu_si256((const __m256i*)coeff_ptr);
        qm    = load_bytes_to_m256_avx2(qm_ptr);
        iqm   = load_bytes_to_m256_avx2(iqm_ptr);
        quantize_highbd_fp_qm(qp, &coeff, iscan, log_scale, qcoeff_ptr, dqcoeff_ptr, &eob, qm, iqm);
    }

    {
        __m256i eob_s;
        eob_s                   = _mm256_shuffle_epi32(eob, 0xe);
        eob                     = _mm256_max_epi16(eob, eob_s);
        eob_s                   = _mm256_shufflelo_epi16(eob, 0xe);
        eob                     = _mm256_max_epi16(eob, eob_s);
        eob_s                   = _mm256_shufflelo_epi16(eob, 1);
        eob                     = _mm256_max_epi16(eob, eob_s);
        const __m128i final_eob = _mm_max_epi16(_mm256_castsi256_si128(eob), _mm256_extractf128_si256(eob, 1));
        *eob_ptr                = _mm_extract_epi16(final_eob, 0);
    }
}
