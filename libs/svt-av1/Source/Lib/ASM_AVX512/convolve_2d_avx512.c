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

#include "definitions.h"

#if EN_AVX512_SUPPORT

#include "memory_sse4_1.h"
#include "common_dsp_rtcd.h"
#include "convolve.h"
#include "convolve_avx2.h"
#include "convolve_avx512.h"
#include "synonyms.h"
#include <immintrin.h>

static void convolve_2d_sr_hor_2tap_avx512(const uint8_t* const src, const int32_t src_stride, const int32_t w,
                                           const int32_t h, const InterpFilterParams* const filter_params_x,
                                           const int32_t subpel_x_q4, int16_t* const im_block) {
    const uint8_t* src_ptr = src;
    int32_t        y       = h;
    int16_t*       im      = im_block;

    if (w <= 8) {
        __m128i coeffs_128;

        prepare_half_coeffs_2tap_ssse3(filter_params_x, subpel_x_q4, &coeffs_128);

        if (w == 2) {
            do {
                const __m128i r = x_convolve_2tap_2x2_sse4_1(src_ptr, src_stride, &coeffs_128);
                xy_x_round_store_2x2_sse2(r, im);
                src_ptr += 2 * src_stride;
                im += 2 * 2;
                y -= 2;
            } while (y);
        } else if (w == 4) {
            do {
                const __m128i r = x_convolve_2tap_4x2_ssse3(src_ptr, src_stride, &coeffs_128);
                xy_x_round_store_4x2_sse2(r, im);
                src_ptr += 2 * src_stride;
                im += 2 * 4;
                y -= 2;
            } while (y);
        } else {
            assert(w == 8);

            do {
                __m128i r[2];

                x_convolve_2tap_8x2_ssse3(src_ptr, src_stride, &coeffs_128, r);
                xy_x_round_store_8x2_sse2(r, im);
                src_ptr += 2 * src_stride;
                im += 2 * 8;
                y -= 2;
            } while (y);
        }
    } else if (w == 16) {
        __m256i coeffs_256;

        prepare_half_coeffs_2tap_avx2(filter_params_x, subpel_x_q4, &coeffs_256);

        do {
            __m256i r[2];

            x_convolve_2tap_16x2_avx2(src_ptr, src_stride, &coeffs_256, r);
            xy_x_round_store_32_avx2(r, im);
            src_ptr += 2 * src_stride;
            im += 2 * 16;
            y -= 2;
        } while (y);
    } else {
        __m512i coeffs_512;

        prepare_half_coeffs_2tap_avx512(filter_params_x, subpel_x_q4, &coeffs_512);

        if (w == 32) {
            do {
                xy_x_2tap_32x2_avx512(src_ptr, src_stride, &coeffs_512, im);
                src_ptr += 2 * src_stride;
                im += 2 * 32;
                y -= 2;
            } while (y);
        } else if (w == 64) {
            do {
                xy_x_2tap_64_avx512(src_ptr, &coeffs_512, im);
                src_ptr += src_stride;
                im += 64;
            } while (--y);
        } else {
            assert(w == 128);

            do {
                xy_x_2tap_64_avx512(src_ptr + 0 * 64, &coeffs_512, im + 0 * 64);
                xy_x_2tap_64_avx512(src_ptr + 1 * 64, &coeffs_512, im + 1 * 64);
                src_ptr += src_stride;
                im += 128;
            } while (--y);
        }
    }
}

static void convolve_2d_sr_hor_6tap_avx512(const uint8_t* const src, const int32_t src_stride, const int32_t w,
                                           const int32_t h, const InterpFilterParams* const filter_params_x,
                                           const int32_t subpel_x_q4, int16_t* const im_block) {
    const uint8_t* src_ptr = src - 2;
    int32_t        y       = h;
    int16_t*       im      = im_block;

    if (w <= 16) {
        __m256i coeffs_256[3], filt_256[3];

        filt_256[0] = _mm256_loadu_si256((__m256i const*)filt1_global_avx);
        filt_256[1] = _mm256_loadu_si256((__m256i const*)filt2_global_avx);
        filt_256[2] = _mm256_loadu_si256((__m256i const*)filt3_global_avx);
        prepare_half_coeffs_6tap_avx2(filter_params_x, subpel_x_q4, coeffs_256);

        if (w == 8) {
            do {
                const __m256i res = x_convolve_6tap_8x2_avx2(src_ptr, src_stride, coeffs_256, filt_256);
                xy_x_round_store_8x2_avx2(res, im);
                src_ptr += 2 * src_stride;
                im += 2 * 8;
                y -= 2;
            } while (y);
        } else {
            assert(w == 16);

            do {
                __m256i r[2];

                x_convolve_6tap_16x2_avx2(src_ptr, src_stride, coeffs_256, filt_256, r);
                xy_x_round_store_32_avx2(r, im);
                src_ptr += 2 * src_stride;
                im += 2 * 16;
                y -= 2;
            } while (y);
        }
    } else {
        __m512i coeffs_512[3], filt_512[3];

        filt_512[0] = zz_load_512(filt1_global_avx);
        filt_512[1] = zz_load_512(filt2_global_avx);
        filt_512[2] = zz_load_512(filt3_global_avx);
        prepare_half_coeffs_6tap_avx512(filter_params_x, subpel_x_q4, coeffs_512);

        if (w == 32) {
            do {
                xy_x_6tap_32x2_avx512(src_ptr, src_stride, coeffs_512, filt_512, im);
                src_ptr += 2 * src_stride;
                im += 2 * 32;
                y -= 2;
            } while (y);
        } else if (w == 64) {
            do {
                xy_x_6tap_64_avx512(src_ptr, coeffs_512, filt_512, im);
                src_ptr += src_stride;
                im += 64;
            } while (--y);
        } else {
            assert(w == 128);

            do {
                xy_x_6tap_64_avx512(src_ptr, coeffs_512, filt_512, im);
                xy_x_6tap_64_avx512(src_ptr + 64, coeffs_512, filt_512, im + 64);
                src_ptr += src_stride;
                im += 128;
            } while (--y);
        }
    }
}

static void convolve_2d_sr_hor_8tap_avx512(const uint8_t* const src, const int32_t src_stride, const int32_t w,
                                           const int32_t h, const InterpFilterParams* const filter_params_x,
                                           const int32_t subpel_x_q4, int16_t* const im_block) {
    const uint8_t* src_ptr = src - 3;
    int32_t        y       = h;
    int16_t*       im      = im_block;

    if (w <= 16) {
        __m256i coeffs_256[4], filt_256[4];

        filt_256[0] = _mm256_loadu_si256((__m256i const*)filt1_global_avx);
        filt_256[1] = _mm256_loadu_si256((__m256i const*)filt2_global_avx);
        filt_256[2] = _mm256_loadu_si256((__m256i const*)filt3_global_avx);
        filt_256[3] = _mm256_loadu_si256((__m256i const*)filt4_global_avx);

        prepare_half_coeffs_8tap_avx2(filter_params_x, subpel_x_q4, coeffs_256);

        if (w == 8) {
            do {
                const __m256i res = x_convolve_8tap_8x2_avx2(src_ptr, src_stride, coeffs_256, filt_256);
                xy_x_round_store_8x2_avx2(res, im);
                src_ptr += 2 * src_stride;
                im += 2 * 8;
                y -= 2;
            } while (y);
        } else {
            assert(w == 16);

            do {
                __m256i r[2];

                x_convolve_8tap_16x2_avx2(src_ptr, src_stride, coeffs_256, filt_256, r);
                xy_x_round_store_32_avx2(r, im);
                src_ptr += 2 * src_stride;
                im += 2 * 16;
                y -= 2;
            } while (y);
        }
    } else {
        __m512i coeffs_512[4], filt_512[4];

        filt_512[0] = zz_load_512(filt1_global_avx);
        filt_512[1] = zz_load_512(filt2_global_avx);
        filt_512[2] = zz_load_512(filt3_global_avx);
        filt_512[3] = zz_load_512(filt4_global_avx);
        prepare_half_coeffs_8tap_avx512(filter_params_x, subpel_x_q4, coeffs_512);

        if (w == 32) {
            do {
                xy_x_8tap_32x2_avx512(src_ptr, src_stride, coeffs_512, filt_512, im);
                src_ptr += 2 * src_stride;
                im += 2 * 32;
                y -= 2;
            } while (y);
        } else if (w == 64) {
            do {
                xy_x_8tap_64_avx512(src_ptr, coeffs_512, filt_512, im);
                src_ptr += src_stride;
                im += 64;
            } while (--y);
        } else {
            assert(w == 128);

            do {
                xy_x_8tap_64_avx512(src_ptr, coeffs_512, filt_512, im);
                xy_x_8tap_64_avx512(src_ptr + 64, coeffs_512, filt_512, im + 64);
                src_ptr += src_stride;
                im += 128;
            } while (--y);
        }
    }
}

static void convolve_2d_sr_ver_2tap_avx512(const int16_t* const im_block, const int32_t w, const int32_t h,
                                           const InterpFilterParams* const filter_params_y, const int32_t subpel_y_q4,
                                           uint8_t* dst, const int32_t dst_stride) {
    const int16_t* im = im_block;
    int32_t        y  = h;

    if (w <= 4) {
        __m128i coeffs_128;

        prepare_coeffs_2tap_sse2(filter_params_y, subpel_y_q4, &coeffs_128);

        if (w == 2) {
            __m128i s_32[2];

            s_32[0] = _mm_cvtsi32_si128(*(int32_t*)im);

            do {
                const __m128i res = xy_y_convolve_2tap_2x2_sse2(im, s_32, &coeffs_128);
                xy_y_round_store_2x2_sse2(res, dst, dst_stride);
                im += 2 * 2;
                dst += 2 * dst_stride;
                y -= 2;
            } while (y);
        } else {
            __m128i s_64[2], r[2];

            assert(w == 4);

            s_64[0] = _mm_loadl_epi64((__m128i*)im);

            do {
                xy_y_convolve_2tap_4x2_sse2(im, s_64, &coeffs_128, r);
                r[0]             = xy_y_round_sse2(r[0]);
                r[1]             = xy_y_round_sse2(r[1]);
                const __m128i rr = _mm_packs_epi32(r[0], r[1]);
                pack_store_4x2_sse2(rr, dst, dst_stride);
                im += 2 * 4;
                dst += 2 * dst_stride;
                y -= 2;
            } while (y);
        }
    } else if (w <= 16) {
        __m256i coeffs_256;

        prepare_coeffs_2tap_avx2(filter_params_y, subpel_y_q4, &coeffs_256);

        if (w == 8) {
            __m128i s_128[2];
            __m256i r[2];

            s_128[0] = _mm_loadu_si128((__m128i*)im);

            do {
                xy_y_convolve_2tap_8x2_avx2(im, s_128, &coeffs_256, r);
                xy_y_round_store_8x2_avx2(r, dst, dst_stride);
                im += 2 * 8;
                dst += 2 * dst_stride;
                y -= 2;
            } while (y);
        } else {
            __m256i s_256[2], r[4];

            assert(w == 16);

            s_256[0] = _mm256_loadu_si256((__m256i*)im);

            do {
                xy_y_convolve_2tap_16x2_avx2(im, s_256, &coeffs_256, r);
                xy_y_round_store_16x2_avx2(r, dst, dst_stride);
                im += 2 * 16;
                dst += 2 * dst_stride;
                y -= 2;
            } while (y);
        }
    } else {
        __m512i coeffs_512;

        prepare_coeffs_2tap_avx512(filter_params_y, subpel_y_q4, &coeffs_512);

        if (w == 32) {
            __m512i s_512[2], r[4];

            s_512[0] = loadu_s16_16x2_avx512(im, 32);

            do {
                xy_y_convolve_2tap_32x2_avx512(im, s_512, &coeffs_512, r);
                xy_y_round_store_32x2_avx512(r + 0, r + 2, dst, dst_stride);
                im += 2 * 32;
                dst += 2 * dst_stride;
                y -= 2;
            } while (y);
        } else if (w == 64) {
            __m512i s_512[2][2];

            s_512[0][0] = zz_load_512(im + 0 * 32);
            s_512[0][1] = zz_load_512(im + 1 * 32);

            do {
                xy_y_convolve_2tap_64_all_avx512(im + 64, s_512[0], s_512[1], &coeffs_512, dst);
                im += 2 * 64;
                xy_y_convolve_2tap_64_all_avx512(im, s_512[1], s_512[0], &coeffs_512, dst + dst_stride);
                dst += 2 * dst_stride;
                y -= 2;
            } while (y);
        } else {
            __m512i s_512[2][4];

            assert(w == 128);

            load_16bit_4rows_avx512(im, 32, s_512[0]);

            do {
                xy_y_convolve_2tap_64_all_avx512(im + 128, s_512[0] + 0, s_512[1] + 0, &coeffs_512, dst);
                xy_y_convolve_2tap_64_all_avx512(im + 192, s_512[0] + 2, s_512[1] + 2, &coeffs_512, dst + 64);
                im += 2 * 128;
                xy_y_convolve_2tap_64_all_avx512(im, s_512[1] + 0, s_512[0] + 0, &coeffs_512, dst + dst_stride);
                xy_y_convolve_2tap_64_all_avx512(
                    im + 64, s_512[1] + 2, s_512[0] + 2, &coeffs_512, dst + dst_stride + 64);
                dst += 2 * dst_stride;
                y -= 2;
            } while (y);
        }
    }
}

static void convolve_2d_sr_ver_2tap_half_avx512(const int16_t* const im_block, const int32_t w, const int32_t h,
                                                const InterpFilterParams* const filter_params_y,
                                                const int32_t subpel_y_q4, uint8_t* dst, const int32_t dst_stride) {
    const int16_t* im = im_block;
    int32_t        y  = h;

    (void)filter_params_y;
    (void)subpel_y_q4;

    if (w == 2) {
        __m128i s_32[2];

        s_32[0] = _mm_cvtsi32_si128(*(int32_t*)im);

        do {
            const __m128i res = xy_y_convolve_2tap_2x2_half_pel_sse2(im, s_32);
            const __m128i r   = xy_y_round_half_pel_sse2(res);
            pack_store_2x2_sse2(r, dst, dst_stride);
            im += 2 * 2;
            dst += 2 * dst_stride;
            y -= 2;
        } while (y);
    } else if (w == 4) {
        __m128i s_64[2];

        s_64[0] = _mm_loadl_epi64((__m128i*)im);

        do {
            const __m128i res = xy_y_convolve_2tap_4x2_half_pel_sse2(im, s_64);
            const __m128i r   = xy_y_round_half_pel_sse2(res);
            pack_store_4x2_sse2(r, dst, dst_stride);
            im += 2 * 4;
            dst += 2 * dst_stride;
            y -= 2;
        } while (y);
    } else if (w == 8) {
        __m128i s_128[2];

        s_128[0] = _mm_loadu_si128((__m128i*)im);

        do {
            const __m256i res = xy_y_convolve_2tap_8x2_half_pel_avx2(im, s_128);
            const __m256i r   = xy_y_round_half_pel_avx2(res);
            pack_store_8x2_avx2(r, dst, dst_stride);
            im += 2 * 8;
            dst += 2 * dst_stride;
            y -= 2;
        } while (y);
    } else if (w == 16) {
        __m256i s_256[2], r[2];

        s_256[0] = _mm256_loadu_si256((__m256i*)im);

        do {
            xy_y_convolve_2tap_16x2_half_pel_avx2(im, s_256, r);
            r[0] = xy_y_round_half_pel_avx2(r[0]);
            r[1] = xy_y_round_half_pel_avx2(r[1]);
            xy_y_pack_store_16x2_avx2(r[0], r[1], dst, dst_stride);
            im += 2 * 16;
            dst += 2 * dst_stride;
            y -= 2;
        } while (y);
    } else if (w == 32) {
        __m512i s_512[2], r[2];

        s_512[0] = loadu_s16_16x2_avx512(im, 32);

        do {
            xy_y_convolve_2tap_half_pel_32x2_avx512(im + 16, s_512, r);
            r[0] = xy_y_round_half_pel_avx512(r[0]);
            r[1] = xy_y_round_half_pel_avx512(r[1]);
            xy_y_pack_store_32x2_avx512(r[0], r[1], dst, dst_stride);
            im += 2 * 32;
            dst += 2 * dst_stride;
            y -= 2;
        } while (y);
    } else if (w == 64) {
        __m512i s_512[2][2];

        s_512[0][0] = zz_load_512(im + 0 * 32);
        s_512[0][1] = zz_load_512(im + 1 * 32);

        do {
            xy_y_convolve_2tap_half_pel_64_all_avx512(im + 64, s_512[0], s_512[1], dst);
            im += 2 * 64;
            xy_y_convolve_2tap_half_pel_64_all_avx512(im, s_512[1], s_512[0], dst + dst_stride);
            dst += 2 * dst_stride;
            y -= 2;
        } while (y);
    } else {
        __m512i s_512[2][4];

        assert(w == 128);

        load_16bit_4rows_avx512(im, 32, s_512[0]);

        do {
            xy_y_convolve_2tap_half_pel_64_all_avx512(im + 128, s_512[0] + 0, s_512[1] + 0, dst);
            xy_y_convolve_2tap_half_pel_64_all_avx512(im + 192, s_512[0] + 2, s_512[1] + 2, dst + 64);
            im += 2 * 128;
            xy_y_convolve_2tap_half_pel_64_all_avx512(im, s_512[1] + 0, s_512[0] + 0, dst + dst_stride);
            xy_y_convolve_2tap_half_pel_64_all_avx512(im + 64, s_512[1] + 2, s_512[0] + 2, dst + dst_stride + 64);
            dst += 2 * dst_stride;
            y -= 2;
        } while (y);
    }
}

void convolve_2d_sr_ver_4tap_avx512(const int16_t* const im_block, const int32_t w, const int32_t h,
                                    const InterpFilterParams* const filter_params_y, const int32_t subpel_y_q4,
                                    uint8_t* dst, const int32_t dst_stride) {
    const int16_t* im = im_block;
    int32_t        y  = h;

    if (w == 2) {
        __m128i coeffs_128[2], s_32[4], ss_128[2];

        prepare_coeffs_4tap_sse2(filter_params_y, subpel_y_q4, coeffs_128);

        s_32[0] = _mm_cvtsi32_si128(*(int32_t*)(im + 0 * 2));
        s_32[1] = _mm_cvtsi32_si128(*(int32_t*)(im + 1 * 2));
        s_32[2] = _mm_cvtsi32_si128(*(int32_t*)(im + 2 * 2));

        const __m128i src01 = _mm_unpacklo_epi32(s_32[0], s_32[1]);
        const __m128i src12 = _mm_unpacklo_epi32(s_32[1], s_32[2]);

        ss_128[0] = _mm_unpacklo_epi16(src01, src12);

        do {
            const __m128i res = xy_y_convolve_4tap_2x2_sse2(im, s_32, ss_128, coeffs_128);
            xy_y_round_store_2x2_sse2(res, dst, dst_stride);
            im += 2 * 2;
            dst += 2 * dst_stride;
            y -= 2;
        } while (y);
    } else {
        __m256i coeffs_256[2];

        prepare_coeffs_4tap_avx2(filter_params_y, subpel_y_q4, coeffs_256);

        if (w == 4) {
            __m128i s_64[4];
            __m256i s_256[2], ss_256[2];

            s_64[0] = _mm_loadl_epi64((__m128i*)(im + 0 * 4));
            s_64[1] = _mm_loadl_epi64((__m128i*)(im + 1 * 4));
            s_64[2] = _mm_loadl_epi64((__m128i*)(im + 2 * 4));

            // Load lines a and b. Line a to lower 128, line b to upper 128
            s_256[0] = _mm256_setr_m128i(s_64[0], s_64[1]);
            s_256[1] = _mm256_setr_m128i(s_64[1], s_64[2]);

            ss_256[0] = _mm256_unpacklo_epi16(s_256[0], s_256[1]);

            do {
                const __m256i res = xy_y_convolve_4tap_4x2_avx2(im, s_64, ss_256, coeffs_256);
                xy_y_round_store_4x2_avx2(res, dst, dst_stride);
                im += 2 * 4;
                dst += 2 * dst_stride;
                y -= 2;
            } while (y);
        } else if (w == 8) {
            __m256i s_256[4], r[2];

            s_256[0] = _mm256_loadu_si256((__m256i*)(im + 0 * 8));
            s_256[1] = _mm256_loadu_si256((__m256i*)(im + 1 * 8));

            if (subpel_y_q4 != 8) {
                __m256i ss_256[4];

                ss_256[0] = _mm256_unpacklo_epi16(s_256[0], s_256[1]);
                ss_256[2] = _mm256_unpackhi_epi16(s_256[0], s_256[1]);

                do {
                    xy_y_convolve_4tap_8x2_avx2(im, ss_256, coeffs_256, r);
                    xy_y_round_store_8x2_avx2(r, dst, dst_stride);
                    im += 2 * 8;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else {
                do {
                    xy_y_convolve_4tap_8x2_half_pel_avx2(im, coeffs_256, s_256, r);
                    xy_y_round_store_8x2_avx2(r, dst, dst_stride);
                    im += 2 * 8;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else if (w == 16) {
            __m256i s_256[5];

            s_256[0] = _mm256_loadu_si256((__m256i*)(im + 0 * 16));
            s_256[1] = _mm256_loadu_si256((__m256i*)(im + 1 * 16));
            s_256[2] = _mm256_loadu_si256((__m256i*)(im + 2 * 16));

            if (subpel_y_q4 != 8) {
                __m256i ss_256[4], tt_256[4], r[4];

                ss_256[0] = _mm256_unpacklo_epi16(s_256[0], s_256[1]);
                ss_256[2] = _mm256_unpackhi_epi16(s_256[0], s_256[1]);

                tt_256[0] = _mm256_unpacklo_epi16(s_256[1], s_256[2]);
                tt_256[2] = _mm256_unpackhi_epi16(s_256[1], s_256[2]);

                do {
                    xy_y_convolve_4tap_16x2_avx2(im, s_256, ss_256, tt_256, coeffs_256, r);
                    xy_y_round_store_16x2_avx2(r, dst, dst_stride);
                    im += 2 * 16;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else {
                __m256i r[4];

                do {
                    xy_y_convolve_4tap_16x2_half_pelavx2(im, s_256, coeffs_256, r);
                    xy_y_round_store_16x2_avx2(r, dst, dst_stride);
                    im += 2 * 16;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else {
            /*It's a special condition for OBMC. A/c  to Av1 spec 4-tap won't
            support for width(w)>16, but for OBMC while predicting above block
            it reduces size block to Wx(h/2), for example, if above block size
            is 32x8, we get block size as 32x4 for OBMC.*/
            __m512i coeffs_512[2], s_512[4], ss_512[4], tt_512[4], r[4];

            assert(w == 32);

            prepare_coeffs_4tap_avx512(filter_params_y, subpel_y_q4, coeffs_512);

            loadu_unpack_16bit_32x3_avx512(im, s_512, ss_512, tt_512);

            y = h;
            do {
                xy_y_convolve_4tap_width32x2_avx512(im, s_512, ss_512, tt_512, coeffs_512, r);
                xy_y_round_store_32x2_avx512(r + 0, r + 2, dst, dst_stride);

                im += 2 * 32;
                dst += 2 * dst_stride;
                y -= 2;
            } while (y);
        }
    }
}

static void convolve_2d_sr_ver_6tap_avx512(const int16_t* const im_block, const int32_t w, const int32_t h,
                                           const InterpFilterParams* const filter_params_y, const int32_t subpel_y_q4,
                                           uint8_t* dst, const int32_t dst_stride) {
    const int16_t* im = im_block;
    int32_t        y;

    if (w == 2) {
        __m128i coeffs_128[3], s_32[6], ss_128[3];

        prepare_coeffs_6tap_ssse3(filter_params_y, subpel_y_q4, coeffs_128);

        s_32[0] = _mm_cvtsi32_si128(*(int32_t*)(im + 0 * 2));
        s_32[1] = _mm_cvtsi32_si128(*(int32_t*)(im + 1 * 2));
        s_32[2] = _mm_cvtsi32_si128(*(int32_t*)(im + 2 * 2));
        s_32[3] = _mm_cvtsi32_si128(*(int32_t*)(im + 3 * 2));
        s_32[4] = _mm_cvtsi32_si128(*(int32_t*)(im + 4 * 2));

        const __m128i src01 = _mm_unpacklo_epi32(s_32[0], s_32[1]);
        const __m128i src12 = _mm_unpacklo_epi32(s_32[1], s_32[2]);
        const __m128i src23 = _mm_unpacklo_epi32(s_32[2], s_32[3]);
        const __m128i src34 = _mm_unpacklo_epi32(s_32[3], s_32[4]);

        ss_128[0] = _mm_unpacklo_epi16(src01, src12);
        ss_128[1] = _mm_unpacklo_epi16(src23, src34);

        y = h;
        do {
            const __m128i res = xy_y_convolve_6tap_2x2_sse2(im, s_32, ss_128, coeffs_128);
            xy_y_round_store_2x2_sse2(res, dst, dst_stride);
            im += 2 * 2;
            dst += 2 * dst_stride;
            y -= 2;
        } while (y);
    } else if (w <= 16) {
        __m256i coeffs_256[3];

        prepare_coeffs_6tap_avx2(filter_params_y, subpel_y_q4, coeffs_256);

        if (w == 4) {
            __m128i s_64[6];
            __m256i s_256[6], ss_256[3];

            s_64[0] = _mm_loadl_epi64((__m128i*)(im + 0 * 4));
            s_64[1] = _mm_loadl_epi64((__m128i*)(im + 1 * 4));
            s_64[2] = _mm_loadl_epi64((__m128i*)(im + 2 * 4));
            s_64[3] = _mm_loadl_epi64((__m128i*)(im + 3 * 4));
            s_64[4] = _mm_loadl_epi64((__m128i*)(im + 4 * 4));

            // Load lines a and b. Line a to lower 128, line b to upper 128
            s_256[0] = _mm256_setr_m128i(s_64[0], s_64[1]);
            s_256[1] = _mm256_setr_m128i(s_64[1], s_64[2]);
            s_256[2] = _mm256_setr_m128i(s_64[2], s_64[3]);
            s_256[3] = _mm256_setr_m128i(s_64[3], s_64[4]);

            ss_256[0] = _mm256_unpacklo_epi16(s_256[0], s_256[1]);
            ss_256[1] = _mm256_unpacklo_epi16(s_256[2], s_256[3]);

            y = h;
            do {
                const __m256i res = xy_y_convolve_6tap_4x2_avx2(im, s_64, ss_256, coeffs_256);
                xy_y_round_store_4x2_avx2(res, dst, dst_stride);
                im += 2 * 4;
                dst += 2 * dst_stride;
                y -= 2;
            } while (y);
        } else if (w == 8) {
            __m256i s_256[6], r[2];

            s_256[0] = _mm256_loadu_si256((__m256i*)(im + 0 * 8));
            s_256[1] = _mm256_loadu_si256((__m256i*)(im + 1 * 8));
            s_256[2] = _mm256_loadu_si256((__m256i*)(im + 2 * 8));
            s_256[3] = _mm256_loadu_si256((__m256i*)(im + 3 * 8));
            y        = h;

            if (subpel_y_q4 != 8) {
                __m256i ss_256[6];

                ss_256[0] = _mm256_unpacklo_epi16(s_256[0], s_256[1]);
                ss_256[1] = _mm256_unpacklo_epi16(s_256[2], s_256[3]);

                ss_256[3] = _mm256_unpackhi_epi16(s_256[0], s_256[1]);
                ss_256[4] = _mm256_unpackhi_epi16(s_256[2], s_256[3]);

                do {
                    xy_y_convolve_6tap_8x2_avx2(im, ss_256, coeffs_256, r);
                    xy_y_round_store_8x2_avx2(r, dst, dst_stride);
                    im += 2 * 8;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else {
                do {
                    xy_y_convolve_6tap_8x2_half_pel_avx2(im, coeffs_256, s_256, r);
                    xy_y_round_store_8x2_avx2(r, dst, dst_stride);
                    im += 2 * 8;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else {
            __m256i s_256[6];

            assert(w == 16);

            s_256[0] = _mm256_loadu_si256((__m256i*)(im + 0 * 16));
            s_256[1] = _mm256_loadu_si256((__m256i*)(im + 1 * 16));
            s_256[2] = _mm256_loadu_si256((__m256i*)(im + 2 * 16));
            s_256[3] = _mm256_loadu_si256((__m256i*)(im + 3 * 16));
            s_256[4] = _mm256_loadu_si256((__m256i*)(im + 4 * 16));
            y        = h;

            if (subpel_y_q4 != 8) {
                __m256i ss_256[6], tt_256[6], r[4];

                ss_256[0] = _mm256_unpacklo_epi16(s_256[0], s_256[1]);
                ss_256[1] = _mm256_unpacklo_epi16(s_256[2], s_256[3]);
                ss_256[3] = _mm256_unpackhi_epi16(s_256[0], s_256[1]);
                ss_256[4] = _mm256_unpackhi_epi16(s_256[2], s_256[3]);

                tt_256[0] = _mm256_unpacklo_epi16(s_256[1], s_256[2]);
                tt_256[1] = _mm256_unpacklo_epi16(s_256[3], s_256[4]);
                tt_256[3] = _mm256_unpackhi_epi16(s_256[1], s_256[2]);
                tt_256[4] = _mm256_unpackhi_epi16(s_256[3], s_256[4]);

                do {
                    xy_y_convolve_6tap_16x2_avx2(im, 16, s_256, ss_256, tt_256, coeffs_256, r);
                    xy_y_round_store_16x2_avx2(r, dst, dst_stride);
                    im += 2 * 16;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else {
                __m256i ss_256[4], r[4];

                do {
                    xy_y_convolve_6tap_16x2_half_pel_avx2(im, 16, s_256, ss_256, coeffs_256, r);
                    xy_y_round_store_16x2_avx2(r, dst, dst_stride);

                    im += 2 * 16;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        }
    } else {
        __m512i coeffs_512[3];

        prepare_coeffs_6tap_avx512(filter_params_y, subpel_y_q4, coeffs_512);

        if (w == 32) {
            __m512i s_512[6], ss_512[6], tt_512[6], r[4];

            loadu_unpack_16bit_32x5_avx512(im, s_512, ss_512, tt_512);

            y = h;
            do {
                xy_y_convolve_6tap_width32x2_avx512(im, coeffs_512, s_512, ss_512, tt_512, r);
                xy_y_round_store_32x2_avx512(r + 0, r + 2, dst, dst_stride);

                im += 2 * 32;
                dst += 2 * dst_stride;
                y -= 2;
            } while (y);
        } else {
            int32_t x = 0;
            __m512i s_512[2][6], ss_512[2][6], tt_512[2][6], r0[4], r1[4];

            assert(!(w % 64));

            do {
                const int16_t* s = im + x;
                uint8_t*       d = dst + x;

                loadu_unpack_16bit_5rows_avx512(s, w, s_512[0], ss_512[0], tt_512[0]);
                loadu_unpack_16bit_5rows_avx512(s + 32, w, s_512[1], ss_512[1], tt_512[1]);

                y = h;
                do {
                    xy_y_convolve_6tap_32x2_avx512(s, w, s_512[0], ss_512[0], tt_512[0], coeffs_512, r0);
                    xy_y_convolve_6tap_32x2_avx512(s + 32, w, s_512[1], ss_512[1], tt_512[1], coeffs_512, r1);

                    xy_y_round_store_64_avx512(r0 + 0, r1 + 0, d);
                    xy_y_round_store_64_avx512(r0 + 2, r1 + 2, d + dst_stride);

                    s += 2 * w;
                    d += 2 * dst_stride;
                    y -= 2;
                } while (y);

                x += 64;
            } while (x < w);
        }
    }
}

static void convolve_2d_sr_ver_8tap_avx512(const int16_t* const im_block, const int32_t w, const int32_t h,
                                           const InterpFilterParams* const filter_params_y, const int32_t subpel_y_q4,
                                           uint8_t* dst, const int32_t dst_stride) {
    const int16_t* im = im_block;
    int32_t        y;

    if (w == 2) {
        __m128i coeffs_128[4], s_32[8], ss_128[4];

        prepare_coeffs_8tap_sse2(filter_params_y, subpel_y_q4, coeffs_128);

        s_32[0] = _mm_cvtsi32_si128(*(int32_t*)(im + 0 * 2));
        s_32[1] = _mm_cvtsi32_si128(*(int32_t*)(im + 1 * 2));
        s_32[2] = _mm_cvtsi32_si128(*(int32_t*)(im + 2 * 2));
        s_32[3] = _mm_cvtsi32_si128(*(int32_t*)(im + 3 * 2));
        s_32[4] = _mm_cvtsi32_si128(*(int32_t*)(im + 4 * 2));
        s_32[5] = _mm_cvtsi32_si128(*(int32_t*)(im + 5 * 2));
        s_32[6] = _mm_cvtsi32_si128(*(int32_t*)(im + 6 * 2));

        const __m128i src01 = _mm_unpacklo_epi32(s_32[0], s_32[1]);
        const __m128i src12 = _mm_unpacklo_epi32(s_32[1], s_32[2]);
        const __m128i src23 = _mm_unpacklo_epi32(s_32[2], s_32[3]);
        const __m128i src34 = _mm_unpacklo_epi32(s_32[3], s_32[4]);
        const __m128i src45 = _mm_unpacklo_epi32(s_32[4], s_32[5]);
        const __m128i src56 = _mm_unpacklo_epi32(s_32[5], s_32[6]);

        ss_128[0] = _mm_unpacklo_epi16(src01, src12);
        ss_128[1] = _mm_unpacklo_epi16(src23, src34);
        ss_128[2] = _mm_unpacklo_epi16(src45, src56);

        y = h;
        do {
            const __m128i res = xy_y_convolve_8tap_2x2_sse2(im, s_32, ss_128, coeffs_128);
            xy_y_round_store_2x2_sse2(res, dst, dst_stride);
            im += 2 * 2;
            dst += 2 * dst_stride;
            y -= 2;
        } while (y);
    } else if (w <= 16) {
        __m256i coeffs_256[4];

        prepare_coeffs_8tap_avx2(filter_params_y, subpel_y_q4, coeffs_256);

        if (w == 4) {
            __m128i s_64[8];
            __m256i s_256[8], ss_256[4];

            s_64[0] = _mm_loadl_epi64((__m128i*)(im + 0 * 4));
            s_64[1] = _mm_loadl_epi64((__m128i*)(im + 1 * 4));
            s_64[2] = _mm_loadl_epi64((__m128i*)(im + 2 * 4));
            s_64[3] = _mm_loadl_epi64((__m128i*)(im + 3 * 4));
            s_64[4] = _mm_loadl_epi64((__m128i*)(im + 4 * 4));
            s_64[5] = _mm_loadl_epi64((__m128i*)(im + 5 * 4));
            s_64[6] = _mm_loadl_epi64((__m128i*)(im + 6 * 4));

            // Load lines a and b. Line a to lower 128, line b to upper 128
            s_256[0] = _mm256_setr_m128i(s_64[0], s_64[1]);
            s_256[1] = _mm256_setr_m128i(s_64[1], s_64[2]);
            s_256[2] = _mm256_setr_m128i(s_64[2], s_64[3]);
            s_256[3] = _mm256_setr_m128i(s_64[3], s_64[4]);
            s_256[4] = _mm256_setr_m128i(s_64[4], s_64[5]);
            s_256[5] = _mm256_setr_m128i(s_64[5], s_64[6]);

            ss_256[0] = _mm256_unpacklo_epi16(s_256[0], s_256[1]);
            ss_256[1] = _mm256_unpacklo_epi16(s_256[2], s_256[3]);
            ss_256[2] = _mm256_unpacklo_epi16(s_256[4], s_256[5]);

            y = h;
            do {
                const __m256i res = xy_y_convolve_8tap_4x2_avx2(im, s_64, ss_256, coeffs_256);
                xy_y_round_store_4x2_avx2(res, dst, dst_stride);
                im += 2 * 4;
                dst += 2 * dst_stride;
                y -= 2;
            } while (y);
        } else if (w == 8) {
            __m256i s_256[8], r[2];

            s_256[0] = _mm256_loadu_si256((__m256i*)(im + 0 * 8));
            s_256[1] = _mm256_loadu_si256((__m256i*)(im + 1 * 8));
            s_256[2] = _mm256_loadu_si256((__m256i*)(im + 2 * 8));
            s_256[3] = _mm256_loadu_si256((__m256i*)(im + 3 * 8));
            s_256[4] = _mm256_loadu_si256((__m256i*)(im + 4 * 8));
            s_256[5] = _mm256_loadu_si256((__m256i*)(im + 5 * 8));
            y        = h;

            if (subpel_y_q4 != 8) {
                __m256i ss_256[8];

                convolve_8tap_unapck_avx2(s_256, ss_256);

                do {
                    xy_y_convolve_8tap_8x2_avx2(im, ss_256, coeffs_256, r);
                    xy_y_round_store_8x2_avx2(r, dst, dst_stride);
                    im += 2 * 8;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else {
                do {
                    xy_y_convolve_8tap_8x2_half_pel_avx2(im, coeffs_256, s_256, r);
                    xy_y_round_store_8x2_avx2(r, dst, dst_stride);
                    im += 2 * 8;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else {
            __m256i s_256[8], r[4];

            assert(w == 16);

            load_16bit_7rows_avx2(im, 16, s_256);
            y = h;

            if (subpel_y_q4 != 8) {
                __m256i ss_256[8], tt_256[8];

                convolve_8tap_unapck_avx2(s_256, ss_256);
                convolve_8tap_unapck_avx2(s_256 + 1, tt_256);

                do {
                    xy_y_convolve_8tap_16x2_avx2(im, 16, coeffs_256, s_256, ss_256, tt_256, r);
                    xy_y_round_store_16x2_avx2(r, dst, dst_stride);

                    im += 2 * 16;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else {
                do {
                    xy_y_convolve_8tap_16x2_half_pel_avx2(im, 16, coeffs_256, s_256, r);
                    xy_y_round_store_16x2_avx2(r, dst, dst_stride);

                    im += 2 * 16;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        }
    } else {
        __m512i coeffs_512[4];

        prepare_coeffs_8tap_avx512(filter_params_y, subpel_y_q4, coeffs_512);

        if (w == 32) {
            __m512i s_512[8], ss_512[8], tt_512[8], r[4];

            load_16bit_32x7_avx512(im, s_512);
            convolve_8tap_unapck_avx512(s_512, ss_512);
            convolve_8tap_unapck_avx512(s_512 + 1, tt_512);

            y = h;
            do {
                xy_y_convolve_8tap_width32x2_avx512(im, coeffs_512, s_512, ss_512, tt_512, r);
                xy_y_round_store_32x2_avx512(r + 0, r + 2, dst, dst_stride);

                im += 2 * 32;
                dst += 2 * dst_stride;
                y -= 2;
            } while (y);
        } else {
            int32_t x = 0;
            __m512i s_512[2][8], ss_512[2][8], tt_512[2][8], r0[4], r1[4];

            assert(!(w % 64));

            do {
                const int16_t* s = im + x;
                uint8_t*       d = dst + x;

                load_16bit_7rows_avx512(s, w, s_512[0]);
                convolve_8tap_unapck_avx512(s_512[0], ss_512[0]);
                convolve_8tap_unapck_avx512(s_512[0] + 1, tt_512[0]);

                load_16bit_7rows_avx512(s + 32, w, s_512[1]);
                convolve_8tap_unapck_avx512(s_512[1], ss_512[1]);
                convolve_8tap_unapck_avx512(s_512[1] + 1, tt_512[1]);

                y = h;
                do {
                    xy_y_convolve_8tap_32x2_avx512(s, w, coeffs_512, s_512[0], ss_512[0], tt_512[0], r0);
                    xy_y_convolve_8tap_32x2_avx512(s + 32, w, coeffs_512, s_512[1], ss_512[1], tt_512[1], r1);
                    xy_y_round_store_64_avx512(r0 + 0, r1 + 0, d);
                    xy_y_round_store_64_avx512(r0 + 2, r1 + 2, d + dst_stride);

                    s += 2 * w;
                    d += 2 * dst_stride;
                    y -= 2;
                } while (y);

                x += 64;
            } while (x < w);
        }
    }
}

typedef void (*Convolve2dSrHorTapFunc)(const uint8_t* const src, const int32_t src_stride, const int32_t w,
                                       const int32_t h, const InterpFilterParams* const filter_params_x,
                                       const int32_t subpel_x_q4, int16_t* const im_block);

typedef void (*Convolve2dSrVerTapFunc)(const int16_t* const im_block, const int32_t w, const int32_t h,
                                       const InterpFilterParams* const filter_params_y, const int32_t subpel_y_q4,
                                       uint8_t* dst, const int32_t dst_stride);

void svt_av1_convolve_2d_sr_avx512(const uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride, int32_t w,
                                   int32_t h, const InterpFilterParams* filter_params_x,
                                   const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                   const int32_t subpel_y_q4, ConvolveParams* conv_params) {
    static const Convolve2dSrHorTapFunc convolve_2d_sr_hor_tap_func_table[MAX_FILTER_TAP + 1] = {
        NULL,
        NULL,
        convolve_2d_sr_hor_2tap_avx512,
        NULL,
        convolve_2d_sr_hor_4tap_ssse3,
        NULL,
        convolve_2d_sr_hor_6tap_avx512,
        NULL,
        convolve_2d_sr_hor_8tap_avx512};
    static const Convolve2dSrVerTapFunc convolve_2d_sr_ver_tap_func_table[MAX_FILTER_TAP + 1] = {
        NULL,
        convolve_2d_sr_ver_2tap_half_avx512,
        convolve_2d_sr_ver_2tap_avx512,
        convolve_2d_sr_ver_4tap_avx512,
        convolve_2d_sr_ver_4tap_avx512,
        convolve_2d_sr_ver_6tap_avx512,
        convolve_2d_sr_ver_6tap_avx512,
        convolve_2d_sr_ver_8tap_avx512,
        convolve_2d_sr_ver_8tap_avx512};
    const int32_t  tap_x   = get_convolve_tap(filter_params_x->filter_ptr);
    const int32_t  tap_y   = get_convolve_tap(filter_params_y->filter_ptr);
    const uint8_t* src_ptr = src + ((MAX_FILTER_TAP - tap_y) / 2 - 3) * src_stride;
    // Note: im_block is 8-pixel interlaced for width 32 and up, to avoid data
    //       permutation.
    DECLARE_ALIGNED(64, int16_t, im_block[(MAX_SB_SIZE + MAX_FILTER_TAP) * MAX_SB_SIZE]);

    (void)conv_params;

    assert(conv_params->round_0 == 3);
    assert(conv_params->round_1 == 11);

    // horizontal filter

    // Have to calculate 1 more row for small widths, since 2 lines are
    // calculated in each loop for them.
    const int32_t hh = h + tap_y - (w >= 64);

    convolve_2d_sr_hor_tap_func_table[tap_x](src_ptr, src_stride, w, hh, filter_params_x, subpel_x_q4, im_block);

    // vertical filter
    convolve_2d_sr_ver_tap_func_table[tap_y - (subpel_y_q4 == 8)](
        im_block, w, h, filter_params_y, subpel_y_q4, dst, dst_stride);
}

static INLINE void copy_64(const uint8_t* const src, uint8_t* const dst) {
    const __m512i s = _mm512_loadu_si512((__m512i*)src);
    _mm512_storeu_si512((__m512i*)dst, s);
}

static INLINE void copy_128(const uint8_t* const src, uint8_t* const dst) {
    copy_64(src + 0 * 64, dst + 0 * 64);
    copy_64(src + 1 * 64, dst + 1 * 64);
}

void svt_av1_convolve_2d_copy_sr_avx512(const uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride,
                                        int32_t w, int32_t h, const InterpFilterParams* filter_params_x,
                                        const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                        const int32_t subpel_y_q4, ConvolveParams* conv_params) {
    (void)filter_params_x;
    (void)filter_params_y;
    (void)subpel_x_q4;
    (void)subpel_y_q4;
    (void)conv_params;

    if (w == 2) {
        do {
            svt_memcpy(dst, src, 2 * sizeof(*src));
            src += src_stride;
            dst += dst_stride;
            svt_memcpy(dst, src, 2 * sizeof(*src));
            src += src_stride;
            dst += dst_stride;
            h -= 2;
        } while (h);
    } else if (w == 4) {
        do {
            svt_memcpy(dst, src, 4 * sizeof(*src));
            src += src_stride;
            dst += dst_stride;
            svt_memcpy(dst, src, 4 * sizeof(*src));
            src += src_stride;
            dst += dst_stride;
            h -= 2;
        } while (h);
    } else if (w == 8) {
        do {
            __m128i s[2];
            s[0] = _mm_loadl_epi64((__m128i*)src);
            src += src_stride;
            s[1] = _mm_loadl_epi64((__m128i*)src);
            src += src_stride;
            _mm_storel_epi64((__m128i*)dst, s[0]);
            dst += dst_stride;
            _mm_storel_epi64((__m128i*)dst, s[1]);
            dst += dst_stride;
            h -= 2;
        } while (h);
    } else if (w == 16) {
        do {
            __m128i s[2];
            s[0] = _mm_loadu_si128((__m128i*)src);
            src += src_stride;
            s[1] = _mm_loadu_si128((__m128i*)src);
            src += src_stride;
            _mm_storeu_si128((__m128i*)dst, s[0]);
            dst += dst_stride;
            _mm_storeu_si128((__m128i*)dst, s[1]);
            dst += dst_stride;
            h -= 2;
        } while (h);
    } else if (w == 32) {
        do {
            __m256i s[2];
            s[0] = _mm256_loadu_si256((__m256i*)src);
            src += src_stride;
            s[1] = _mm256_loadu_si256((__m256i*)src);
            src += src_stride;
            _mm256_storeu_si256((__m256i*)dst, s[0]);
            dst += dst_stride;
            _mm256_storeu_si256((__m256i*)dst, s[1]);
            dst += dst_stride;
            h -= 2;
        } while (h);
    } else if (w == 64) {
        do {
            copy_64(src, dst);
            src += src_stride;
            dst += dst_stride;
            copy_64(src, dst);
            src += src_stride;
            dst += dst_stride;
            h -= 2;
        } while (h);
    } else {
        do {
            copy_128(src, dst);
            src += src_stride;
            dst += dst_stride;
            copy_128(src, dst);
            src += src_stride;
            dst += dst_stride;
            h -= 2;
        } while (h);
    }
}

#endif // EN_AVX512_SUPPORT
