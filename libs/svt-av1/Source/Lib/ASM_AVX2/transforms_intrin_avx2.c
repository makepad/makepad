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

#include <emmintrin.h>
#include <immintrin.h>
#include "motion_estimation.h"

static INLINE void energy_computation_kernel_avx2(const int32_t* const in, __m256i* const sum256) {
    const __m256i zero      = _mm256_setzero_si256();
    const __m256i input     = _mm256_loadu_si256((__m256i*)in);
    const __m256i in_lo     = _mm256_unpacklo_epi32(input, zero);
    const __m256i in_hi     = _mm256_unpackhi_epi32(input, zero);
    const __m256i energy_lo = _mm256_mul_epi32(in_lo, in_lo);
    const __m256i energy_hi = _mm256_mul_epi32(in_hi, in_hi);
    *sum256                 = _mm256_add_epi64(*sum256, energy_lo);
    *sum256                 = _mm256_add_epi64(*sum256, energy_hi);
}

static INLINE uint64_t hadd64_avx2(const __m256i sum256) {
    const __m128i sum256_lo = _mm256_castsi256_si128(sum256);
    const __m128i sum256_hi = _mm256_extracti128_si256(sum256, 1);
    const __m128i sum128    = _mm_add_epi64(sum256_lo, sum256_hi);
    const __m128i sum128_hi = _mm_srli_si128(sum128, 8);
    const __m128i sum       = _mm_add_epi64(sum128, sum128_hi);

    return _mm_extract_epi64(sum, 0);
}

static INLINE uint64_t energy_computation_avx2(const int32_t* const in, const uint32_t size) {
    const __m256i zero = _mm256_setzero_si256();
    uint32_t      i    = 0;
    __m256i       sum  = zero;

    do {
        energy_computation_kernel_avx2(in + i, &sum);
        i += 8;
    } while (i < size);

    return hadd64_avx2(sum);
}

static INLINE uint64_t energy_computation_64_avx2(const int32_t* in, const uint32_t height) {
    const __m256i zero = _mm256_setzero_si256();
    uint32_t      i    = height;
    __m256i       sum  = zero;

    do {
        energy_computation_kernel_avx2(in + 0 * 8, &sum);
        energy_computation_kernel_avx2(in + 1 * 8, &sum);
        energy_computation_kernel_avx2(in + 2 * 8, &sum);
        energy_computation_kernel_avx2(in + 3 * 8, &sum);
        in += 64;
    } while (--i);

    return hadd64_avx2(sum);
}

static INLINE void copy_32_bytes_avx2(const int32_t* src, int32_t* dst) {
    const __m256i val = _mm256_loadu_si256((__m256i*)(src + 0 * 8));
    _mm256_storeu_si256((__m256i*)(dst + 0 * 8), val);
}

static INLINE void copy_256x_bytes_avx2(const int32_t* src, int32_t* dst, const uint32_t height) {
    uint32_t h = height;

    do {
        copy_32_bytes_avx2(src + 0 * 8, dst + 0 * 8);
        copy_32_bytes_avx2(src + 1 * 8, dst + 1 * 8);
        copy_32_bytes_avx2(src + 2 * 8, dst + 2 * 8);
        copy_32_bytes_avx2(src + 3 * 8, dst + 3 * 8);
        src += 64;
        dst += 32;
    } while (--h);
}

uint64_t svt_handle_transform16x64_avx2(int32_t* output) {
    //bottom 16x32 area.
    const uint64_t three_quad_energy = energy_computation_avx2(output + 16 * 32, 16 * 32);
    return three_quad_energy;
}

uint64_t svt_handle_transform32x64_avx2(int32_t* output) {
    //bottom 32x32 area.
    const uint64_t three_quad_energy = energy_computation_avx2(output + 32 * 32, 32 * 32);
    return three_quad_energy;
}

uint64_t svt_handle_transform64x16_avx2(int32_t* output) {
    // top - right 32x16 area.
    const uint64_t three_quad_energy = energy_computation_64_avx2(output + 32, 16);
    // Re-pack non-zero coeffs in the first 32x16 indices.
    copy_256x_bytes_avx2(output + 64, output + 32, 15);

    return three_quad_energy;
}

uint64_t svt_handle_transform64x32_avx2(int32_t* output) {
    // top - right 32x32 area.
    const uint64_t three_quad_energy = energy_computation_64_avx2(output + 32, 32);
    // Re-pack non-zero coeffs in the first 32x32 indices.
    copy_256x_bytes_avx2(output + 64, output + 32, 31);

    return three_quad_energy;
}

uint64_t svt_handle_transform64x64_avx2(int32_t* output) {
    uint64_t three_quad_energy;

    // top - right 32x32 area.
    three_quad_energy = energy_computation_64_avx2(output + 32, 32);
    //bottom 64x32 area.
    three_quad_energy += energy_computation_avx2(output + 32 * 64, 64 * 32);
    // Re-pack non-zero coeffs in the first 32x32 indices.
    copy_256x_bytes_avx2(output + 64, output + 32, 31);

    return three_quad_energy;
}

uint64_t svt_handle_transform16x64_N2_N4_avx2(int32_t* output) {
    (void)output;
    return 0;
}

uint64_t svt_handle_transform32x64_N2_N4_avx2(int32_t* output) {
    (void)output;
    return 0;
}

uint64_t svt_handle_transform64x16_N2_N4_avx2(int32_t* output) {
    // Re-pack non-zero coeffs in the first 32x16 indices.
    copy_256x_bytes_avx2(output + 64, output + 32, 15);
    return 0;
}

uint64_t svt_handle_transform64x32_N2_N4_avx2(int32_t* output) {
    // Re-pack non-zero coeffs in the first 32x32 indices.
    copy_256x_bytes_avx2(output + 64, output + 32, 31);
    return 0;
}

uint64_t svt_handle_transform64x64_N2_N4_avx2(int32_t* output) {
    // Re-pack non-zero coeffs in the first 32x32 indices.
    copy_256x_bytes_avx2(output + 64, output + 32, 31);
    return 0;
}

static INLINE __m128i compute_sum(__m256i* in, __m256i* prev_in) {
    const __m256i zero    = _mm256_setzero_si256();
    const __m256i round_2 = _mm256_set1_epi16(2);
    __m256i       prev_in_lo, in_lo, sum_;

    prev_in_lo = _mm256_unpacklo_epi8(*prev_in, zero);
    *prev_in   = _mm256_unpackhi_epi8(*prev_in, zero);
    in_lo      = _mm256_unpacklo_epi8(*in, zero);
    *in        = _mm256_unpackhi_epi8(*in, zero);

    sum_ = _mm256_add_epi16(_mm256_hadd_epi16(prev_in_lo, *prev_in), _mm256_hadd_epi16(in_lo, *in));
    sum_ = _mm256_add_epi16(sum_, round_2);
    sum_ = _mm256_srli_epi16(sum_, 2);

    return _mm_packus_epi16(_mm256_castsi256_si128(sum_), _mm256_extracti128_si256(sum_, 0x1));
}

void svt_aom_downsample_2d_avx2(uint8_t* input_samples, // input parameter, input samples Ptr
                                uint32_t input_stride, // input parameter, input stride
                                uint32_t input_area_width, // input parameter, input area width
                                uint32_t input_area_height, // input parameter, input area height
                                uint8_t* decim_samples, // output parameter, decimated samples Ptr
                                uint32_t decim_stride, // input parameter, output stride
                                uint32_t decim_step) // input parameter, decimation amount in pixels
{
    uint32_t input_stripe_stride = input_stride * decim_step;
    uint32_t decim_horizontal_index;
    uint8_t* in_ptr        = input_samples;
    uint8_t* out_ptr       = decim_samples;
    uint32_t width_align32 = input_area_width - (input_area_width % 32);

    __m256i in, prev_in;
    __m128i sum_epu8;
    DECLARE_ALIGNED(16, uint8_t, tmp_buf[16]);

    if (decim_step == 2) {
        in_ptr += input_stride;
        for (uint32_t vert_idx = 1; vert_idx < input_area_height; vert_idx += 2) {
            uint8_t* prev_in_line  = in_ptr - input_stride;
            decim_horizontal_index = 0;
            for (uint32_t horiz_idx = 1; horiz_idx < width_align32; horiz_idx += 32) {
                prev_in  = _mm256_loadu_si256((__m256i*)(prev_in_line + horiz_idx - 1));
                in       = _mm256_loadu_si256((__m256i*)(in_ptr + horiz_idx - 1));
                sum_epu8 = compute_sum(&in, &prev_in);
                _mm_storeu_si128((__m128i*)(out_ptr + decim_horizontal_index), sum_epu8);
                decim_horizontal_index += 16;
            }
            //complement when input_area_width is not multiple of 32
            if (width_align32 < input_area_width) {
                prev_in   = _mm256_loadu_si256((__m256i*)(prev_in_line + width_align32));
                in        = _mm256_loadu_si256((__m256i*)(in_ptr + width_align32));
                sum_epu8  = compute_sum(&in, &prev_in);
                int count = (input_area_width - width_align32) >> 1;
                _mm_storeu_si128((__m128i*)(tmp_buf), sum_epu8);
                memcpy(out_ptr + decim_horizontal_index, tmp_buf, count * sizeof(uint8_t));
            }
            in_ptr += input_stripe_stride;
            out_ptr += decim_stride;
        }
    } else if (decim_step == 4) {
        const __m128i mask = _mm_set_epi64x(0x0F0D0B0907050301, 0x0E0C0A0806040200);
        in_ptr += 2 * input_stride;
        for (uint32_t vertical_index = 2; vertical_index < input_area_height; vertical_index += 4) {
            uint8_t* prev_in_line  = in_ptr - input_stride;
            decim_horizontal_index = 0;
            for (uint32_t horiz_idx = 2; horiz_idx < width_align32; horiz_idx += 32) {
                prev_in  = _mm256_loadu_si256((__m256i*)(prev_in_line + horiz_idx - 1));
                in       = _mm256_loadu_si256((__m256i*)(in_ptr + horiz_idx - 1));
                sum_epu8 = compute_sum(&in, &prev_in);
                sum_epu8 = _mm_shuffle_epi8(sum_epu8, mask);
                _mm_storel_epi64((__m128i*)(out_ptr + decim_horizontal_index), sum_epu8);
                decim_horizontal_index += 8;
            }
            //complement when input_area_width is not multiple of 32
            if (width_align32 < input_area_width) {
                prev_in   = _mm256_loadu_si256((__m256i*)(prev_in_line + width_align32 + 1));
                in        = _mm256_loadu_si256((__m256i*)(in_ptr + width_align32 + 1));
                sum_epu8  = compute_sum(&in, &prev_in);
                sum_epu8  = _mm_shuffle_epi8(sum_epu8, mask);
                int count = (input_area_width - width_align32) >> 2;
                _mm_storeu_si128((__m128i*)(tmp_buf), sum_epu8);
                memcpy(out_ptr + decim_horizontal_index, tmp_buf, count * sizeof(uint8_t));
            }
            in_ptr += input_stripe_stride;
            out_ptr += decim_stride;
        }
    } else {
        svt_aom_downsample_2d_c(
            input_samples, input_stride, input_area_width, input_area_height, decim_samples, decim_stride, decim_step);
    }
}
