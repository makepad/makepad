/*
 * Copyright (c) 2022, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at www.aomedia.org/license/software. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at www.aomedia.org/license/patent.
 */

#include <arm_neon.h>

#include "aom_dsp_rtcd.h"
#include "full_loop.h"
#include "mem_neon.h"
#include "sum_neon.h"

static inline uint16x4_t quantize_4_b(const TranLow* coeff_ptr, TranLow* qcoeff_ptr, TranLow* dqcoeff_ptr,
                                      int32x4_t v_quant_s32, int32x4_t v_dequant_s32, int32x4_t v_round_s32,
                                      int32x4_t v_zbin_s32, int32x4_t v_quant_shift_s32, int log_scale) {
    const int32x4_t v_coeff      = vld1q_s32(coeff_ptr);
    const int32x4_t v_coeff_sign = vreinterpretq_s32_u32(vcltq_s32(v_coeff, vdupq_n_s32(0)));
    const int32x4_t v_abs_coeff  = vabsq_s32(v_coeff);
    // if (abs_coeff < zbins[rc != 0]),
    const uint32x4_t v_zbin_mask = vcgeq_s32(v_abs_coeff, v_zbin_s32);
    const int32x4_t  v_log_scale = vdupq_n_s32(log_scale);
    // const int64_t tmp = (int64_t)abs_coeff + log_scaled_round;
    const int32x4_t v_tmp = vaddq_s32(v_abs_coeff, v_round_s32);
    // const int32_t tmpw32 = tmp * wt;
    const int32x4_t v_tmpw32 = vmulq_s32(v_tmp, vdupq_n_s32((1 << AOM_QM_BITS)));
    // const int32_t tmp2 = (int32_t)((tmpw32 * quant64) >> 16);
    const int32x4_t v_tmp2 = vqdmulhq_s32(v_tmpw32, v_quant_s32);
    // const int32_t tmp3 =
    //    ((((tmp2 + tmpw32)<< log_scale) * (int64_t)(quant_shift << 15)) >> 32);
    const int32x4_t v_tmp3 = vqdmulhq_s32(vshlq_s32(vaddq_s32(v_tmp2, v_tmpw32), v_log_scale), v_quant_shift_s32);
    // const int abs_qcoeff = vmask ? (int)tmp3 >> AOM_QM_BITS : 0;
    const int32x4_t v_abs_qcoeff = vandq_s32(vreinterpretq_s32_u32(v_zbin_mask), vshrq_n_s32(v_tmp3, AOM_QM_BITS));
    // const TranLow abs_dqcoeff = (abs_qcoeff * dequant_iwt) >> log_scale;
    // vshlq_s32 will shift right if shift value is negative.
    const int32x4_t v_abs_dqcoeff = vshlq_s32(vmulq_s32(v_abs_qcoeff, v_dequant_s32), vnegq_s32(v_log_scale));
    // qcoeff_ptr[rc] = (TranLow)((abs_qcoeff ^ coeff_sign) - coeff_sign);
    const int32x4_t v_qcoeff = vsubq_s32(veorq_s32(v_abs_qcoeff, v_coeff_sign), v_coeff_sign);
    // dqcoeff_ptr[rc] = (TranLow)((abs_dqcoeff ^ coeff_sign) - coeff_sign);
    const int32x4_t v_dqcoeff = vsubq_s32(veorq_s32(v_abs_dqcoeff, v_coeff_sign), v_coeff_sign);

    vst1q_s32(qcoeff_ptr, v_qcoeff);
    vst1q_s32(dqcoeff_ptr, v_dqcoeff);

    // Used to find eob.
    const uint32x4_t nz_qcoeff_mask = vcgtq_s32(v_abs_qcoeff, vdupq_n_s32(0));
    return vmovn_u32(nz_qcoeff_mask);
}

static inline uint16_t get_max_eob(int16x8_t v_eobmax) {
    int16_t max_val = vmaxvq_s16(v_eobmax);
    return (uint16_t)max_val + 1;
}

static inline int16x8_t get_max_lane_eob(const int16_t* iscan, int16x8_t v_eobmax, uint16x8_t v_mask) {
    const int16x8_t v_iscan    = vld1q_s16(iscan);
    const int16x8_t v_nz_iscan = vbslq_s16(v_mask, v_iscan, vdupq_n_s16(-1));
    return vmaxq_s16(v_eobmax, v_nz_iscan);
}

void svt_aom_highbd_quantize_b_neon(const TranLow* coeff_ptr, intptr_t n_coeffs, const int16_t* zbin_ptr,
                                    const int16_t* round_ptr, const int16_t* quant_ptr, const int16_t* quant_shift_ptr,
                                    TranLow* qcoeff_ptr, TranLow* dqcoeff_ptr, const int16_t* dequant_ptr,
                                    uint16_t* eob_ptr, const int16_t* scan, const int16_t* iscan, const QmVal* qm_ptr,
                                    const QmVal* iqm_ptr, const int32_t log_scale) {
    (void)qm_ptr;
    (void)iqm_ptr;
    (void)scan;
    const int16x4_t  v_quant           = vld1_s16(quant_ptr);
    const int16x4_t  v_dequant         = vld1_s16(dequant_ptr);
    const int16x4_t  v_zero            = vdup_n_s16(0);
    const uint16x4_t v_round_select    = vcgt_s16(vdup_n_s16(log_scale), v_zero);
    const int16x4_t  v_round_no_scale  = vld1_s16(round_ptr);
    const int16x4_t  v_round_log_scale = vqrdmulh_n_s16(v_round_no_scale, (int16_t)(1 << (15 - log_scale)));
    const int16x4_t  v_round           = vbsl_s16(v_round_select, v_round_log_scale, v_round_no_scale);
    const int16x4_t  v_quant_shift     = vld1_s16(quant_shift_ptr);
    const int16x4_t  v_zbin_no_scale   = vld1_s16(zbin_ptr);
    const int16x4_t  v_zbin_log_scale  = vqrdmulh_n_s16(v_zbin_no_scale, (int16_t)(1 << (15 - log_scale)));
    const int16x4_t  v_zbin            = vbsl_s16(v_round_select, v_zbin_log_scale, v_zbin_no_scale);
    int32x4_t        v_round_s32       = vmovl_s16(v_round);
    int32x4_t        v_quant_s32       = vshlq_n_s32(vmovl_s16(v_quant), 15);
    int32x4_t        v_dequant_s32     = vmovl_s16(v_dequant);
    int32x4_t        v_quant_shift_s32 = vshlq_n_s32(vmovl_s16(v_quant_shift), 15);
    int32x4_t        v_zbin_s32        = vmovl_s16(v_zbin);
    uint16x4_t       v_mask_lo, v_mask_hi;
    int16x8_t        v_eobmax = vdupq_n_s16(-1);

    intptr_t non_zero_count = n_coeffs;

    assert(n_coeffs > 8);
    // Pre-scan pass
    const int32x4_t v_zbin_s32x = vdupq_lane_s32(vget_low_s32(v_zbin_s32), 1);
    intptr_t        i           = n_coeffs;
    do {
        const int32x4_t  v_coeff_a     = vld1q_s32(coeff_ptr + i - 4);
        const int32x4_t  v_coeff_b     = vld1q_s32(coeff_ptr + i - 8);
        const int32x4_t  v_abs_coeff_a = vabsq_s32(v_coeff_a);
        const int32x4_t  v_abs_coeff_b = vabsq_s32(v_coeff_b);
        const uint32x4_t v_mask_a      = vcgeq_s32(v_abs_coeff_a, v_zbin_s32x);
        const uint32x4_t v_mask_b      = vcgeq_s32(v_abs_coeff_b, v_zbin_s32x);
        // If the coefficient is in the base ZBIN range, then discard.
        if (vaddvq_u32(v_mask_a) + vaddvq_u32(v_mask_b) == 0) {
            non_zero_count -= 8;
        } else {
            break;
        }
        i -= 8;
    } while (i > 0);

    const intptr_t remaining_zcoeffs = n_coeffs - non_zero_count;
    memset(qcoeff_ptr + non_zero_count, 0, remaining_zcoeffs * sizeof(*qcoeff_ptr));
    memset(dqcoeff_ptr + non_zero_count, 0, remaining_zcoeffs * sizeof(*dqcoeff_ptr));

    // DC and first 3 AC
    v_mask_lo = quantize_4_b(coeff_ptr,
                             qcoeff_ptr,
                             dqcoeff_ptr,
                             v_quant_s32,
                             v_dequant_s32,
                             v_round_s32,
                             v_zbin_s32,
                             v_quant_shift_s32,
                             log_scale);

    // overwrite the DC constants with AC constants
    v_round_s32       = vdupq_lane_s32(vget_low_s32(v_round_s32), 1);
    v_quant_s32       = vdupq_lane_s32(vget_low_s32(v_quant_s32), 1);
    v_dequant_s32     = vdupq_lane_s32(vget_low_s32(v_dequant_s32), 1);
    v_quant_shift_s32 = vdupq_lane_s32(vget_low_s32(v_quant_shift_s32), 1);
    v_zbin_s32        = vdupq_lane_s32(vget_low_s32(v_zbin_s32), 1);

    // 4 more AC
    v_mask_hi = quantize_4_b(coeff_ptr + 4,
                             qcoeff_ptr + 4,
                             dqcoeff_ptr + 4,
                             v_quant_s32,
                             v_dequant_s32,
                             v_round_s32,
                             v_zbin_s32,
                             v_quant_shift_s32,
                             log_scale);

    v_eobmax = get_max_lane_eob(iscan, v_eobmax, vcombine_u16(v_mask_lo, v_mask_hi));

    intptr_t count = non_zero_count - 8;
    for (; count > 0; count -= 8) {
        coeff_ptr += 8;
        qcoeff_ptr += 8;
        dqcoeff_ptr += 8;
        iscan += 8;
        v_mask_lo = quantize_4_b(coeff_ptr,
                                 qcoeff_ptr,
                                 dqcoeff_ptr,
                                 v_quant_s32,
                                 v_dequant_s32,
                                 v_round_s32,
                                 v_zbin_s32,
                                 v_quant_shift_s32,
                                 log_scale);
        v_mask_hi = quantize_4_b(coeff_ptr + 4,
                                 qcoeff_ptr + 4,
                                 dqcoeff_ptr + 4,
                                 v_quant_s32,
                                 v_dequant_s32,
                                 v_round_s32,
                                 v_zbin_s32,
                                 v_quant_shift_s32,
                                 log_scale);
        // Find the max lane eob for 8 coeffs.
        v_eobmax = get_max_lane_eob(iscan, v_eobmax, vcombine_u16(v_mask_lo, v_mask_hi));
    }

    *eob_ptr = get_max_eob(v_eobmax);
}

static inline uint16x4_t quantize_4_fp(const tran_low_t* coeff_ptr, tran_low_t* qcoeff_ptr, tran_low_t* dqcoeff_ptr,
                                       int32x4_t v_quant_s32, int32x4_t v_dequant_s32, int32x4_t v_round_s32,
                                       int log_scale) {
    const int32x4_t v_coeff      = vld1q_s32(coeff_ptr);
    const int32x4_t v_coeff_sign = vreinterpretq_s32_u32(vcltq_s32(v_coeff, vdupq_n_s32(0)));
    const int32x4_t v_log_scale  = vdupq_n_s32(log_scale);
    const int32x4_t v_abs_coeff  = vabsq_s32(v_coeff);
    // ((abs_coeff << (1 + log_scale)) >= dequant_ptr[rc01])
    const int32x4_t  v_abs_coeff_scaled = vshlq_s32(v_abs_coeff, vdupq_n_s32(1 + log_scale));
    const uint32x4_t v_mask             = vcgeq_s32(v_abs_coeff_scaled, v_dequant_s32);
    // const int64_t tmp = vmask ? (int64_t)abs_coeff + log_scaled_round : 0
    const int32x4_t v_tmp = vandq_s32(vaddq_s32(v_abs_coeff, v_round_s32), vreinterpretq_s32_u32(v_mask));
    // const int abs_qcoeff = (int)((tmp * quant) >> (16 - log_scale));
    const int32x4_t v_abs_qcoeff = vqdmulhq_s32(vshlq_s32(v_tmp, v_log_scale), v_quant_s32);
    // qcoeff_ptr[rc] = (tran_low_t)((abs_qcoeff ^ coeff_sign) - coeff_sign);
    const int32x4_t v_qcoeff = vsubq_s32(veorq_s32(v_abs_qcoeff, v_coeff_sign), v_coeff_sign);
    // vshlq_s32 will shift right if shift value is negative.
    const int32x4_t v_abs_dqcoeff = vshlq_s32(vmulq_s32(v_abs_qcoeff, v_dequant_s32), vnegq_s32(v_log_scale));
    // dqcoeff_ptr[rc] = (tran_low_t)((abs_dqcoeff ^ coeff_sign) - coeff_sign);
    const int32x4_t v_dqcoeff = vsubq_s32(veorq_s32(v_abs_dqcoeff, v_coeff_sign), v_coeff_sign);

    vst1q_s32(qcoeff_ptr, v_qcoeff);
    vst1q_s32(dqcoeff_ptr, v_dqcoeff);

    // Used to find eob.
    const uint32x4_t nz_qcoeff_mask = vcgtq_s32(v_abs_qcoeff, vdupq_n_s32(0));
    return vmovn_u32(nz_qcoeff_mask);
}

void svt_av1_highbd_quantize_fp_neon(const TranLow* coeff_ptr, intptr_t count, const int16_t* zbin_ptr,
                                     const int16_t* round_ptr, const int16_t* quant_ptr, const int16_t* quant_shift_ptr,
                                     TranLow* qcoeff_ptr, TranLow* dqcoeff_ptr, const int16_t* dequant_ptr,
                                     uint16_t* eob_ptr, const int16_t* scan, const int16_t* iscan, int16_t log_scale) {
    (void)scan;
    (void)zbin_ptr;
    (void)quant_shift_ptr;

    const int16x4_t  v_quant           = vld1_s16(quant_ptr);
    const int16x4_t  v_dequant         = vld1_s16(dequant_ptr);
    const int16x4_t  v_zero            = vdup_n_s16(0);
    const uint16x4_t v_round_select    = vcgt_s16(vdup_n_s16(log_scale), v_zero);
    const int16x4_t  v_round_no_scale  = vld1_s16(round_ptr);
    const int16x4_t  v_round_log_scale = vqrdmulh_n_s16(v_round_no_scale, (int16_t)(1 << (15 - log_scale)));
    const int16x4_t  v_round           = vbsl_s16(v_round_select, v_round_log_scale, v_round_no_scale);
    int32x4_t        v_round_s32       = vaddl_s16(v_round, v_zero);
    int32x4_t        v_quant_s32       = vshlq_n_s32(vaddl_s16(v_quant, v_zero), 15);
    int32x4_t        v_dequant_s32     = vaddl_s16(v_dequant, v_zero);
    uint16x4_t       v_mask_lo, v_mask_hi;
    int16x8_t        v_eobmax = vdupq_n_s16(-1);

    // DC and first 3 AC
    v_mask_lo = quantize_4_fp(coeff_ptr, qcoeff_ptr, dqcoeff_ptr, v_quant_s32, v_dequant_s32, v_round_s32, log_scale);

    // overwrite the DC constants with AC constants
    v_round_s32   = vdupq_lane_s32(vget_low_s32(v_round_s32), 1);
    v_quant_s32   = vdupq_lane_s32(vget_low_s32(v_quant_s32), 1);
    v_dequant_s32 = vdupq_lane_s32(vget_low_s32(v_dequant_s32), 1);

    // 4 more AC
    v_mask_hi = quantize_4_fp(
        coeff_ptr + 4, qcoeff_ptr + 4, dqcoeff_ptr + 4, v_quant_s32, v_dequant_s32, v_round_s32, log_scale);

    // Find the max lane eob for the first 8 coeffs.
    v_eobmax = get_max_lane_eob(iscan, v_eobmax, vcombine_u16(v_mask_lo, v_mask_hi));

    count -= 8;
    do {
        coeff_ptr += 8;
        qcoeff_ptr += 8;
        dqcoeff_ptr += 8;
        iscan += 8;
        v_mask_lo = quantize_4_fp(
            coeff_ptr, qcoeff_ptr, dqcoeff_ptr, v_quant_s32, v_dequant_s32, v_round_s32, log_scale);
        v_mask_hi = quantize_4_fp(
            coeff_ptr + 4, qcoeff_ptr + 4, dqcoeff_ptr + 4, v_quant_s32, v_dequant_s32, v_round_s32, log_scale);
        // Find the max lane eob for 8 coeffs.
        v_eobmax = get_max_lane_eob(iscan, v_eobmax, vcombine_u16(v_mask_lo, v_mask_hi));
        count -= 8;
    } while (count);

    *eob_ptr = get_max_eob(v_eobmax);
}
