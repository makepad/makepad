/*
 * Copyright (c) 2024, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at www.aomedia.org/license/software. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at www.aomedia.org/license/patent.
 */

#include <arm_neon.h>
#include <math.h>
#include "aom_dsp_rtcd.h"
#include "cdef.h"
#include "definitions.h"
#include "mem_neon.h"

static inline void mse_4xn_8bit_neon(const uint8_t* src, const uint8_t* dst, const int32_t dstride, uint32x4_t* sse,
                                     uint8_t height, uint8_t subsampling_factor) {
    do {
        const uint8x8_t s = load_u8_4x2(src, 4 * subsampling_factor);
        const uint8x8_t d = load_u8_4x2(dst, dstride * subsampling_factor);

        const uint16x8_t abs = vabdl_u8(d, s);

        *sse = vmlal_u16(*sse, vget_low_u16(abs), vget_low_u16(abs));
        *sse = vmlal_u16(*sse, vget_high_u16(abs), vget_high_u16(abs));

        src += 2 * 4 * subsampling_factor; // with * 2 rows per iter * subsampling
        dst += 2 * dstride * subsampling_factor;
        height -= 2 * subsampling_factor;
    } while (height != 0);
}

static inline void mse_8xn_8bit_neon(const uint8_t* src, const uint8_t* dst, const int32_t dstride, uint32x4_t* sse,
                                     uint8_t height, uint8_t subsampling_factor) {
    uint32x4_t mse0 = vdupq_n_u32(0);
    uint32x4_t mse1 = vdupq_n_u32(0);

    do {
        const uint8x8_t s0 = vld1_u8(src);
        const uint8x8_t s1 = vld1_u8(src + subsampling_factor * 8);
        const uint8x8_t d0 = vld1_u8(dst);
        const uint8x8_t d1 = vld1_u8(dst + subsampling_factor * dstride);

        const uint16x8_t abs0 = vabdl_u8(d0, s0);
        const uint16x8_t abs1 = vabdl_u8(d1, s1);

        mse0 = vmlal_u16(mse0, vget_low_u16(abs0), vget_low_u16(abs0));
        mse0 = vmlal_u16(mse0, vget_high_u16(abs0), vget_high_u16(abs0));
        mse1 = vmlal_u16(mse1, vget_low_u16(abs1), vget_low_u16(abs1));
        mse1 = vmlal_u16(mse1, vget_high_u16(abs1), vget_high_u16(abs1));

        src += 8 * 2 * subsampling_factor;
        dst += 2 * subsampling_factor * dstride;
        height -= 2 * subsampling_factor;
    } while (height != 0);
    *sse = vaddq_u32(*sse, mse0);
    *sse = vaddq_u32(*sse, mse1);
}

uint64_t svt_aom_compute_cdef_dist_8bit_neon(const uint8_t* dst8, int32_t dstride, const uint8_t* src8,
                                             const CdefList* dlist, int32_t cdef_count, BlockSize bsize,
                                             int32_t coeff_shift, uint8_t subsampling_factor) {
    uint64_t sum;
    int32_t  bi, bx, by;

    uint32x4_t mse = vdupq_n_u32(0);

    if (bsize == BLOCK_8X8) {
        for (bi = 0; bi < cdef_count; bi++) {
            by = dlist[bi].by;
            bx = dlist[bi].bx;
            mse_8xn_8bit_neon(src8, dst8 + 8 * by * dstride + 8 * bx, dstride, &mse, 8, subsampling_factor);
            src8 += 8 * 8;
        }
    } else if (bsize == BLOCK_4X8) {
        for (bi = 0; bi < cdef_count; bi++) {
            by = dlist[bi].by;
            bx = dlist[bi].bx;
            mse_4xn_8bit_neon(src8, dst8 + 8 * by * dstride + 4 * bx, dstride, &mse, 8, subsampling_factor);
            src8 += 4 * 8;
        }
    } else if (bsize == BLOCK_8X4) {
        for (bi = 0; bi < cdef_count; bi++) {
            by = dlist[bi].by;
            bx = dlist[bi].bx;
            mse_8xn_8bit_neon(src8, dst8 + 4 * by * dstride + 8 * bx, dstride, &mse, 4, subsampling_factor);
            src8 += 8 * 4;
        }
    } else {
        assert(bsize == BLOCK_4X4);
        for (bi = 0; bi < cdef_count; bi++) {
            by = dlist[bi].by;
            bx = dlist[bi].bx;
            mse_4xn_8bit_neon(src8, dst8 + 4 * by * dstride + 4 * bx, dstride, &mse, 4, subsampling_factor);
            src8 += 4 * 4;
        }
    }

    sum = vaddlvq_u32(mse);
    return sum >> 2 * coeff_shift;
}

static inline uint32x4_t mse_8xn_16bit_neon(const uint16_t* src, const uint16_t* dst, const int32_t dstride,
                                            uint8_t height, uint8_t subsampling_factor) {
    uint32x4_t sse0 = vdupq_n_u32(0);
    uint32x4_t sse1 = vdupq_n_u32(0);

    do {
        const uint16x8_t s0 = vld1q_u16(src);
        const uint16x8_t s1 = vld1q_u16(src + subsampling_factor * 8);
        const uint16x8_t d0 = vld1q_u16(dst);
        const uint16x8_t d1 = vld1q_u16(dst + subsampling_factor * dstride);

        const uint16x8_t abs0 = vabdq_u16(d0, s0);
        const uint16x8_t abs1 = vabdq_u16(d1, s1);

        sse0 = vmlal_u16(sse0, vget_low_u16(abs0), vget_low_u16(abs0));
        sse0 = vmlal_u16(sse0, vget_high_u16(abs0), vget_high_u16(abs0));
        sse1 = vmlal_u16(sse1, vget_low_u16(abs1), vget_low_u16(abs1));
        sse1 = vmlal_u16(sse1, vget_high_u16(abs1), vget_high_u16(abs1));

        src += 8 * 2 * subsampling_factor;
        dst += 2 * subsampling_factor * dstride;
        height -= 2 * subsampling_factor;
    } while (height != 0);

    return vaddq_u32(sse0, sse1);
}

static inline uint32x4_t mse_4xn_16bit_neon(const uint16_t* src, const uint16_t* dst, const int32_t dstride,
                                            uint8_t height, uint8_t subsampling_factor) {
    uint32x4_t sse = vdupq_n_u32(0);

    do {
        const uint16x8_t s0 = load_u16_4x2(src, 4 * subsampling_factor);
        const uint16x8_t d0 = load_u16_4x2(dst, dstride * subsampling_factor);

        const uint16x8_t diff_0 = vabdq_u16(d0, s0);

        sse = vmlal_u16(sse, vget_low_u16(diff_0), vget_low_u16(diff_0));
        sse = vmlal_u16(sse, vget_high_u16(diff_0), vget_high_u16(diff_0));

        src += 2 * 4 * subsampling_factor; // with * 4 rows per iter * subsampling
        dst += 2 * subsampling_factor * dstride;
        height -= 2 * subsampling_factor;
    } while (height != 0);

    return sse;
}

uint64_t svt_aom_compute_cdef_dist_16bit_neon(const uint16_t* dst, int32_t dstride, const uint16_t* src,
                                              const CdefList* dlist, int32_t cdef_count, BlockSize bsize,
                                              int32_t coeff_shift, uint8_t subsampling_factor) {
    uint64_t sum;
    int32_t  bi, bx, by;

    uint64x2_t mse64 = vdupq_n_u64(0);

    if (bsize == BLOCK_8X8) {
        for (bi = 0; bi < cdef_count; bi++) {
            by               = dlist[bi].by;
            bx               = dlist[bi].bx;
            uint32x4_t mse32 = mse_8xn_16bit_neon(src, dst + 8 * by * dstride + 8 * bx, dstride, 8, subsampling_factor);
            mse64            = vpadalq_u32(mse64, mse32);
            src += 8 * 8;
        }
    } else if (bsize == BLOCK_4X8) {
        for (bi = 0; bi < cdef_count; bi++) {
            by               = dlist[bi].by;
            bx               = dlist[bi].bx;
            uint32x4_t mse32 = mse_4xn_16bit_neon(src, dst + 8 * by * dstride + 4 * bx, dstride, 8, subsampling_factor);
            mse64            = vpadalq_u32(mse64, mse32);
            src += 4 * 8;
        }
    } else if (bsize == BLOCK_8X4) {
        for (bi = 0; bi < cdef_count; bi++) {
            by               = dlist[bi].by;
            bx               = dlist[bi].bx;
            uint32x4_t mse32 = mse_8xn_16bit_neon(src, dst + 4 * by * dstride + 8 * bx, dstride, 4, subsampling_factor);
            mse64            = vpadalq_u32(mse64, mse32);
            src += 8 * 4;
        }
    } else {
        assert(bsize == BLOCK_4X4);
        for (bi = 0; bi < cdef_count; bi++) {
            by               = dlist[bi].by;
            bx               = dlist[bi].bx;
            uint32x4_t mse32 = mse_4xn_16bit_neon(src, dst + 4 * by * dstride + 4 * bx, dstride, 4, subsampling_factor);
            mse64            = vpadalq_u32(mse64, mse32);
            src += 4 * 4;
        }
    }

    sum = vaddvq_u64(mse64);

    return sum >> 2 * coeff_shift;
}

/* Loop over the already selected nb_strengths (Luma_strength,
   Chroma_strength) pairs, and find the pair that has the smallest mse
   (best_mse) for the current filter block. */
static inline uint64_t find_best_mse(const uint64_t* mse0, const uint64_t* mse1, const int* lev0, const int* lev1,
                                     int n) {
    uint64_t best_mse = (uint64_t)1 << 63;
    for (int i = 0; i < n; i++) {
        uint64_t curr = mse0[(uint32_t)lev0[i]] + mse1[(uint32_t)lev1[i]];
        if (curr < best_mse) {
            best_mse = curr;
        }
    }
    return best_mse;
}

static inline uint64x2_t add_select_best(uint64x2_t a, uint64x2_t b, uint64x2_t best) {
    uint64x2_t sum  = vaddq_u64(a, b);
    uint64x2_t comp = vcltq_u64(sum, best);
    best            = vbslq_u64(comp, sum, best);

    return best;
}

uint64_t svt_search_one_dual_neon(int* lev0, int* lev1, int nb_strengths, uint64_t** mse[2], int sb_count, int start_gi,
                                  int end_gi) {
    if (start_gi >= end_gi) {
        lev0[nb_strengths] = 0;
        lev1[nb_strengths] = 0;
        return (uint64_t)1 << 63;
    }

    if (sb_count == 0) {
        lev0[nb_strengths] = 0;
        lev1[nb_strengths] = 0;
        return 0;
    }

    size_t   stride = ((end_gi + 3) & ~3);
    uint64_t tot_mse[end_gi * stride];
    size_t   start_idx = start_gi;
    size_t   end_idx   = end_gi;

    const uint64_t* mse_0_ptr = mse[0][0];
    const uint64_t* mse_1_ptr = mse[1][0];

    uint64x2_t best_mse0 = vdupq_n_u64(find_best_mse(mse_0_ptr, mse_1_ptr, lev0, lev1, nb_strengths));

    size_t j = start_idx;
    do {
        uint64_t*  tot_mse_ptr = tot_mse + j * stride;
        uint64x2_t mse0        = vld1q_dup_u64(&mse_0_ptr[j]);

        size_t k = start_idx;
        do {
            uint64x2_t mse1_0 = vld1q_u64(&mse_1_ptr[k + 0]);
            uint64x2_t mse1_1 = vld1q_u64(&mse_1_ptr[k + 2]);

            uint64x2_t best_0 = add_select_best(mse0, mse1_0, best_mse0);
            uint64x2_t best_1 = add_select_best(mse0, mse1_1, best_mse0);

            vst1q_u64(tot_mse_ptr + k + 0, best_0);
            vst1q_u64(tot_mse_ptr + k + 2, best_1);
            k += 4;
        } while (k < end_idx);
    } while (++j < end_idx);

    /* Loop over the filter blocks in the frame */
    for (size_t i = 1; i + 2 <= (size_t)sb_count; i += 2) {
        const uint64_t* mse_0_0_ptr = mse[0][i + 0];
        const uint64_t* mse_0_1_ptr = mse[0][i + 1];
        const uint64_t* mse_1_0_ptr = mse[1][i + 0];
        const uint64_t* mse_1_1_ptr = mse[1][i + 1];

        best_mse0            = vdupq_n_u64(find_best_mse(mse_0_0_ptr, mse_1_0_ptr, lev0, lev1, nb_strengths));
        uint64x2_t best_mse1 = vdupq_n_u64(find_best_mse(mse_0_1_ptr, mse_1_1_ptr, lev0, lev1, nb_strengths));

        /* Loop over the set of available (Luma_strength, Chroma_strength)
           pairs, identify any that provide an mse better than best_mse from the
           step above for the current filter block, and update any corresponding
           total mse (tot_mse[j * stride + k]). */
        /* Find best mse when adding each possible new option. */
        j = start_idx;
        do {
            uint64_t*  tot_mse_ptr = tot_mse + j * stride;
            uint64x2_t mse0_0      = vld1q_dup_u64(mse_0_0_ptr + j);
            uint64x2_t mse0_1      = vld1q_dup_u64(mse_0_1_ptr + j);

            size_t k = start_idx;
            do {
                uint64x2_t mse1_0_0 = vld1q_u64(mse_1_0_ptr + k + 0);
                uint64x2_t mse1_0_1 = vld1q_u64(mse_1_0_ptr + k + 2);

                uint64x2_t mse1_1_0 = vld1q_u64(mse_1_1_ptr + k + 0);
                uint64x2_t mse1_1_1 = vld1q_u64(mse_1_1_ptr + k + 2);

                uint64x2_t tot_mse_0 = vld1q_u64(tot_mse_ptr + k + 0);
                uint64x2_t tot_mse_1 = vld1q_u64(tot_mse_ptr + k + 2);

                uint64x2_t best0_0 = add_select_best(mse0_0, mse1_0_0, best_mse0);
                uint64x2_t best0_1 = add_select_best(mse0_0, mse1_0_1, best_mse0);
                uint64x2_t best1_0 = add_select_best(mse0_1, mse1_1_0, best_mse1);
                uint64x2_t best1_1 = add_select_best(mse0_1, mse1_1_1, best_mse1);

                best0_0 = vaddq_u64(best0_0, best1_0);
                best0_1 = vaddq_u64(best0_1, best1_1);

                vst1q_u64(tot_mse_ptr + k + 0, vaddq_u64(tot_mse_0, best0_0));
                vst1q_u64(tot_mse_ptr + k + 2, vaddq_u64(tot_mse_1, best0_1));

                k += 4;
            } while (k < end_idx);
        } while (++j < end_idx);
    }

    if (sb_count % 2 == 0) {
        mse_0_ptr = mse[0][sb_count - 1];
        mse_1_ptr = mse[1][sb_count - 1];

        best_mse0 = vdupq_n_u64(find_best_mse(mse_0_ptr, mse_1_ptr, lev0, lev1, nb_strengths));

        j = start_idx;
        do {
            uint64_t*  tot_mse_ptr = tot_mse + j * stride;
            uint64x2_t mse0        = vld1q_dup_u64(&mse_0_ptr[j]);

            size_t k = start_idx;
            do {
                uint64x2_t mse1_0 = vld1q_u64(mse_1_ptr + k + 0);
                uint64x2_t mse1_1 = vld1q_u64(mse_1_ptr + k + 2);

                uint64x2_t tot_mse_0 = vld1q_u64(tot_mse_ptr + k + 0);
                uint64x2_t tot_mse_1 = vld1q_u64(tot_mse_ptr + k + 2);

                uint64x2_t best_0 = add_select_best(mse0, mse1_0, best_mse0);
                uint64x2_t best_1 = add_select_best(mse0, mse1_1, best_mse0);

                vst1q_u64(tot_mse_ptr + k + 0, vaddq_u64(tot_mse_0, best_0));
                vst1q_u64(tot_mse_ptr + k + 2, vaddq_u64(tot_mse_1, best_1));
                k += 4;
            } while (k < end_idx);
        } while (++j < end_idx);
    }

    /* Loop over the additionally searched (Luma_strength, Chroma_strength) pairs
       from the step above, and identify any such pair that provided the best mse for
       the whole frame. The identified pair would be added to the set of already selected pairs. */
    uint64_t best_tot_mse = (uint64_t)1 << 63;
    int32_t  best_id0     = 0;
    int32_t  best_id1     = 0;

    j = start_gi;
    do {
        // Loop over the additionally searched luma strengths
        size_t k = start_gi;
        do {
            // Loop over the additionally searched chroma strengths
            if (tot_mse[j * stride + k] < best_tot_mse) {
                best_tot_mse = tot_mse[j * stride + k];
                best_id0     = j; // index for the best luma strength
                best_id1     = k; // index for the best chroma strength
            }
        } while (++k < end_idx);
    } while (++j < end_idx);

    lev0[nb_strengths] = best_id0; // Add the identified luma strength to the list of selected luma strengths
    lev1[nb_strengths] = best_id1; // Add the identified chroma strength to the list of selected chroma strengths
    return best_tot_mse;
}
