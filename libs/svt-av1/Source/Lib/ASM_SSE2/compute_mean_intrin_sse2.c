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

#include <emmintrin.h>
#include "compute_mean_sse2.h"

uint64_t svt_aom_compute_subd_mean_of_squared_values8x8_sse2_intrin(
    uint8_t* input_samples, // input parameter, input samples Ptr
    uint16_t input_stride) // input parameter, input stride

{
    __m128i xmm0, xmm_block_mean, xmm_input;

    xmm0           = _mm_setzero_si128();
    xmm_input      = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)input_samples), xmm0);
    xmm_block_mean = _mm_madd_epi16(xmm_input, xmm_input);

    /*xmm_input = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i *)(input_samples + input_stride)), xmm0);
    xmm_block_mean = _mm_add_epi32(xmm_block_mean, _mm_madd_epi16(xmm_input, xmm_input));*/

    xmm_input      = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(input_samples + 2 * input_stride)), xmm0);
    xmm_block_mean = _mm_add_epi32(xmm_block_mean, _mm_madd_epi16(xmm_input, xmm_input));

    /*xmm_input = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i *)(input_samples+3*input_stride)), xmm0);
    xmm_block_mean = _mm_add_epi32(xmm_block_mean, _mm_madd_epi16(xmm_input, xmm_input));*/

    xmm_input      = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(input_samples + 4 * input_stride)), xmm0);
    xmm_block_mean = _mm_add_epi32(xmm_block_mean, _mm_madd_epi16(xmm_input, xmm_input));

    //xmm_input = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i *)(input_samples+5*input_stride)), xmm0);
    //xmm_block_mean = _mm_add_epi32(xmm_block_mean, _mm_madd_epi16(xmm_input, xmm_input));

    xmm_input      = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(input_samples + 6 * input_stride)), xmm0);
    xmm_block_mean = _mm_add_epi32(xmm_block_mean, _mm_madd_epi16(xmm_input, xmm_input));

    /*xmm_input = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i *)(input_samples+7*input_stride)), xmm0);
    xmm_block_mean = _mm_add_epi32(xmm_block_mean, _mm_madd_epi16(xmm_input, xmm_input));*/

    xmm_block_mean = _mm_add_epi32(xmm_block_mean, _mm_srli_si128(xmm_block_mean, 8));
    xmm_block_mean = _mm_add_epi32(xmm_block_mean, _mm_srli_si128(xmm_block_mean, 4));

    return (uint64_t)_mm_cvtsi128_si32(xmm_block_mean) << 11;
}

uint64_t svt_compute_sub_mean8x8_sse2_intrin(uint8_t* input_samples, // input parameter, input samples Ptr
                                             uint16_t input_stride) // input parameter, input stride

{
    __m128i xmm0 = _mm_setzero_si128(), xmm1, xmm3, xmm_sum1, xmm_sum2;

    xmm1 = _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(input_samples)), xmm0);
    //xmm2 = _mm_sad_epu8(_mm_loadl_epi64((__m128i *)(input_samples + input_stride)), xmm0);
    xmm3 = _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(input_samples + 2 * input_stride)), xmm0);
    //xmm4 = _mm_sad_epu8(_mm_loadl_epi64((__m128i *)(input_samples + 3 * input_stride)), xmm0);
    xmm_sum1 = _mm_add_epi16(xmm1, xmm3);

    input_samples += 4 * input_stride;
    xmm1 = _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(input_samples)), xmm0);
    //xmm2 = _mm_sad_epu8(_mm_loadl_epi64((__m128i *)(input_samples + input_stride)), xmm0);
    xmm3 = _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(input_samples + 2 * input_stride)), xmm0);
    //xmm4 = _mm_sad_epu8(_mm_loadl_epi64((__m128i *)(input_samples + 3 * input_stride)), xmm0);
    xmm_sum2 = _mm_add_epi16(xmm1, xmm3);
    xmm_sum2 = _mm_add_epi16(xmm_sum1, xmm_sum2);

    return (uint64_t)_mm_cvtsi128_si32(xmm_sum2) << 3;
}

uint64_t svt_compute_mean_of_squared_values8x8_sse2_intrin(
    uint8_t* input_samples, // input parameter, input samples Ptr
    uint32_t input_stride, // input parameter, input stride
    uint32_t input_area_width, // input parameter, input area width
    uint32_t input_area_height) // input parameter, input area height
{
    __m128i xmm0, xmm_block_mean, xmm_input;
    (void)input_area_width;
    (void)input_area_height;
    xmm0           = _mm_setzero_si128();
    xmm_input      = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)input_samples), xmm0);
    xmm_block_mean = _mm_madd_epi16(xmm_input, xmm_input);

    xmm_input      = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(input_samples + input_stride)), xmm0);
    xmm_block_mean = _mm_add_epi32(xmm_block_mean, _mm_madd_epi16(xmm_input, xmm_input));

    xmm_input      = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(input_samples + 2 * input_stride)), xmm0);
    xmm_block_mean = _mm_add_epi32(xmm_block_mean, _mm_madd_epi16(xmm_input, xmm_input));

    xmm_input      = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(input_samples + 3 * input_stride)), xmm0);
    xmm_block_mean = _mm_add_epi32(xmm_block_mean, _mm_madd_epi16(xmm_input, xmm_input));

    xmm_input      = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(input_samples + 4 * input_stride)), xmm0);
    xmm_block_mean = _mm_add_epi32(xmm_block_mean, _mm_madd_epi16(xmm_input, xmm_input));

    xmm_input      = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(input_samples + 5 * input_stride)), xmm0);
    xmm_block_mean = _mm_add_epi32(xmm_block_mean, _mm_madd_epi16(xmm_input, xmm_input));

    xmm_input      = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(input_samples + 6 * input_stride)), xmm0);
    xmm_block_mean = _mm_add_epi32(xmm_block_mean, _mm_madd_epi16(xmm_input, xmm_input));

    xmm_input      = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(input_samples + 7 * input_stride)), xmm0);
    xmm_block_mean = _mm_add_epi32(xmm_block_mean, _mm_madd_epi16(xmm_input, xmm_input));

    xmm_block_mean = _mm_add_epi32(xmm_block_mean, _mm_srli_si128(xmm_block_mean, 8));
    xmm_block_mean = _mm_add_epi32(xmm_block_mean, _mm_srli_si128(xmm_block_mean, 4));

    return (uint64_t)_mm_cvtsi128_si32(xmm_block_mean) << 10;
}

uint64_t svt_compute_mean8x8_sse2_intrin(uint8_t* input_samples, // input parameter, input samples Ptr
                                         uint32_t input_stride, // input parameter, input stride
                                         uint32_t input_area_width, // input parameter, input area width
                                         uint32_t input_area_height) // input parameter, input area height
{
    __m128i xmm0 = _mm_setzero_si128(), xmm1, xmm2, xmm3, xmm4, xmm_sum1, xmm_sum2;

    xmm1     = _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(input_samples)), xmm0);
    xmm2     = _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(input_samples + input_stride)), xmm0);
    xmm3     = _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(input_samples + 2 * input_stride)), xmm0);
    xmm4     = _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(input_samples + 3 * input_stride)), xmm0);
    xmm_sum1 = _mm_add_epi16(_mm_add_epi16(xmm1, xmm2), _mm_add_epi16(xmm3, xmm4));

    input_samples += 4 * input_stride;
    xmm1     = _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(input_samples)), xmm0);
    xmm2     = _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(input_samples + input_stride)), xmm0);
    xmm3     = _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(input_samples + 2 * input_stride)), xmm0);
    xmm4     = _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(input_samples + 3 * input_stride)), xmm0);
    xmm_sum2 = _mm_add_epi16(_mm_add_epi16(xmm1, xmm2), _mm_add_epi16(xmm3, xmm4));
    xmm_sum2 = _mm_add_epi16(xmm_sum1, xmm_sum2);

    (void)input_area_width;
    (void)input_area_height;

    return (uint64_t)_mm_cvtsi128_si32(xmm_sum2) << 2;
}

void svt_compute_interm_var_four8x8_helper_sse2(uint8_t* input_samples, uint16_t input_stride,
                                                uint64_t* mean_of8x8_blocks, // mean of four  8x8
                                                uint64_t* mean_of_squared8x8_blocks) // meanSquared
{
    uint32_t block_index = 0;
    // (0,1)
    mean_of8x8_blocks[0]         = svt_compute_sub_mean8x8_sse2_intrin(input_samples + block_index, input_stride);
    mean_of_squared8x8_blocks[0] = svt_aom_compute_subd_mean_of_squared_values8x8_sse2_intrin(
        input_samples + block_index, input_stride);

    // (0,2)
    block_index                  = block_index + 8;
    mean_of8x8_blocks[1]         = svt_compute_sub_mean8x8_sse2_intrin(input_samples + block_index, input_stride);
    mean_of_squared8x8_blocks[1] = svt_aom_compute_subd_mean_of_squared_values8x8_sse2_intrin(
        input_samples + block_index, input_stride);

    // (0,3)
    block_index                  = block_index + 8;
    mean_of8x8_blocks[2]         = svt_compute_sub_mean8x8_sse2_intrin(input_samples + block_index, input_stride);
    mean_of_squared8x8_blocks[2] = svt_aom_compute_subd_mean_of_squared_values8x8_sse2_intrin(
        input_samples + block_index, input_stride);

    // (0,4)
    block_index                  = block_index + 8;
    mean_of8x8_blocks[3]         = svt_compute_sub_mean8x8_sse2_intrin(input_samples + block_index, input_stride);
    mean_of_squared8x8_blocks[3] = svt_aom_compute_subd_mean_of_squared_values8x8_sse2_intrin(
        input_samples + block_index, input_stride);
}
