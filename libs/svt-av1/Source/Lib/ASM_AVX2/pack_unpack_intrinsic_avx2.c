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
#include <immintrin.h>
#include <stdint.h>

void svt_enc_msb_un_pack2d_avx2_intrin(uint16_t* in16_bit_buffer, uint32_t in_stride, uint8_t* out8_bit_buffer,
                                       uint8_t* outn_bit_buffer, uint32_t out8_stride, uint32_t outn_stride,
                                       uint32_t width, uint32_t height) {
    uint32_t x, y;

    __m128i in_pixel0, in_pixel1, temp_pixel0, temp_pixel1, in_pixel1_shft_r_2_u8, in_pixel0_shft_r_2_u8,
        in_pixel0_shft_r_2, in_pixel1_shft_r_2, temp_pixel0_u8, temp_pixel1_u8;

    __m128i xmm_3    = _mm_set1_epi16(0x0003);
    __m128i xmm_00ff = _mm_set1_epi16(0x00FF);
    __m256i ymm_3    = _mm256_set1_epi16(0x0003);
    __m256i ymm_00ff = _mm256_set1_epi16(0x00FF);

    if (width == 4) {
        for (y = 0; y < height; y += 2) {
            in_pixel0 = _mm_loadl_epi64((__m128i*)in16_bit_buffer);
            in_pixel1 = _mm_loadl_epi64((__m128i*)(in16_bit_buffer + in_stride));

            if (outn_bit_buffer) {
                temp_pixel0                                 = _mm_slli_epi16(_mm_and_si128(in_pixel0, xmm_3), 6);
                temp_pixel1                                 = _mm_slli_epi16(_mm_and_si128(in_pixel1, xmm_3), 6);
                temp_pixel0_u8                              = _mm_packus_epi16(temp_pixel0, temp_pixel0);
                temp_pixel1_u8                              = _mm_packus_epi16(temp_pixel1, temp_pixel1);
                *(uint32_t*)outn_bit_buffer                 = _mm_cvtsi128_si32(temp_pixel0_u8);
                *(uint32_t*)(outn_bit_buffer + outn_stride) = _mm_cvtsi128_si32(temp_pixel1_u8);
                outn_bit_buffer += 2 * outn_stride;
            }

            in_pixel0_shft_r_2                          = _mm_and_si128(_mm_srli_epi16(in_pixel0, 2), xmm_00ff);
            in_pixel1_shft_r_2                          = _mm_and_si128(_mm_srli_epi16(in_pixel1, 2), xmm_00ff);
            in_pixel0_shft_r_2_u8                       = _mm_packus_epi16(in_pixel0_shft_r_2, in_pixel0_shft_r_2);
            in_pixel1_shft_r_2_u8                       = _mm_packus_epi16(in_pixel1_shft_r_2, in_pixel1_shft_r_2);
            *(uint32_t*)out8_bit_buffer                 = _mm_cvtsi128_si32(in_pixel0_shft_r_2_u8);
            *(uint32_t*)(out8_bit_buffer + out8_stride) = _mm_cvtsi128_si32(in_pixel1_shft_r_2_u8);

            out8_bit_buffer += 2 * out8_stride;
            in16_bit_buffer += 2 * in_stride;
        }
    } else if (width == 8) {
        for (y = 0; y < height; y += 2) {
            in_pixel0 = _mm_loadu_si128((__m128i*)in16_bit_buffer);
            in_pixel1 = _mm_loadu_si128((__m128i*)(in16_bit_buffer + in_stride));

            if (outn_bit_buffer) {
                temp_pixel0    = _mm_slli_epi16(_mm_and_si128(in_pixel0, xmm_3), 6);
                temp_pixel1    = _mm_slli_epi16(_mm_and_si128(in_pixel1, xmm_3), 6);
                temp_pixel0_u8 = _mm_packus_epi16(temp_pixel0, temp_pixel0);
                temp_pixel1_u8 = _mm_packus_epi16(temp_pixel1, temp_pixel1);
                _mm_storel_epi64((__m128i*)outn_bit_buffer, temp_pixel0_u8);
                _mm_storel_epi64((__m128i*)(outn_bit_buffer + outn_stride), temp_pixel1_u8);
                outn_bit_buffer += 2 * outn_stride;
            }

            in_pixel0_shft_r_2    = _mm_and_si128(_mm_srli_epi16(in_pixel0, 2), xmm_00ff);
            in_pixel1_shft_r_2    = _mm_and_si128(_mm_srli_epi16(in_pixel1, 2), xmm_00ff);
            in_pixel0_shft_r_2_u8 = _mm_packus_epi16(in_pixel0_shft_r_2, in_pixel0_shft_r_2);
            in_pixel1_shft_r_2_u8 = _mm_packus_epi16(in_pixel1_shft_r_2, in_pixel1_shft_r_2);
            _mm_storel_epi64((__m128i*)out8_bit_buffer, in_pixel0_shft_r_2_u8);
            _mm_storel_epi64((__m128i*)(out8_bit_buffer + out8_stride), in_pixel1_shft_r_2_u8);

            out8_bit_buffer += 2 * out8_stride;
            in16_bit_buffer += 2 * in_stride;
        }
    } else if (width == 16) {
        __m256i in_pixel_0, in_pixel_1, in_pixel_0_shft_r_2_u8, temp_pixel_0_u8;

        for (y = 0; y < height; y += 2) {
            in_pixel_0 = _mm256_loadu_si256((__m256i*)in16_bit_buffer);
            in_pixel_1 = _mm256_loadu_si256((__m256i*)(in16_bit_buffer + in_stride));

            if (outn_bit_buffer) {
                temp_pixel_0_u8 = _mm256_packus_epi16(_mm256_and_si256(in_pixel_0, ymm_3),
                                                      _mm256_and_si256(in_pixel_1, ymm_3));
                temp_pixel_0_u8 = _mm256_slli_epi16(_mm256_permute4x64_epi64(temp_pixel_0_u8, 0xd8), 6);
                _mm_storeu_si128((__m128i*)outn_bit_buffer, _mm256_castsi256_si128(temp_pixel_0_u8));
                _mm_storeu_si128((__m128i*)(outn_bit_buffer + outn_stride),
                                 _mm256_extracti128_si256(temp_pixel_0_u8, 1));
                outn_bit_buffer += 2 * outn_stride;
            }

            in_pixel_0_shft_r_2_u8 = _mm256_packus_epi16(_mm256_and_si256(_mm256_srli_epi16(in_pixel_0, 2), ymm_00ff),
                                                         _mm256_and_si256(_mm256_srli_epi16(in_pixel_1, 2), ymm_00ff));
            in_pixel_0_shft_r_2_u8 = _mm256_permute4x64_epi64(in_pixel_0_shft_r_2_u8, 0xd8);
            _mm_storeu_si128((__m128i*)out8_bit_buffer, _mm256_castsi256_si128(in_pixel_0_shft_r_2_u8));
            _mm_storeu_si128((__m128i*)(out8_bit_buffer + out8_stride),
                             _mm256_extracti128_si256(in_pixel_0_shft_r_2_u8, 1));

            out8_bit_buffer += 2 * out8_stride;
            in16_bit_buffer += 2 * in_stride;
        }
    } else if (width == 32) {
        __m256i in_pixel_0, in_pixel_1, in_pixel_2, in_pixel_3;
        __m256i outn0_u8, outn1_u8;
        __m256i out8_0_u8, out8_1_u8;

        for (y = 0; y < height; y += 2) {
            in_pixel_0 = _mm256_loadu_si256((__m256i*)in16_bit_buffer);
            in_pixel_1 = _mm256_loadu_si256((__m256i*)(in16_bit_buffer + 16));
            in_pixel_2 = _mm256_loadu_si256((__m256i*)(in16_bit_buffer + in_stride));
            in_pixel_3 = _mm256_loadu_si256((__m256i*)(in16_bit_buffer + in_stride + 16));

            if (outn_bit_buffer) {
                outn0_u8 = _mm256_packus_epi16(_mm256_and_si256(in_pixel_0, ymm_3),
                                               _mm256_and_si256(in_pixel_1, ymm_3));
                outn1_u8 = _mm256_packus_epi16(_mm256_and_si256(in_pixel_2, ymm_3),
                                               _mm256_and_si256(in_pixel_3, ymm_3));
                outn0_u8 = _mm256_slli_epi16(_mm256_permute4x64_epi64(outn0_u8, 0xd8), 6);
                outn1_u8 = _mm256_slli_epi16(_mm256_permute4x64_epi64(outn1_u8, 0xd8), 6);
                _mm256_storeu_si256((__m256i*)outn_bit_buffer, outn0_u8);
                _mm256_storeu_si256((__m256i*)(outn_bit_buffer + outn_stride), outn1_u8);
                outn_bit_buffer += 2 * outn_stride;
            }

            out8_0_u8 = _mm256_packus_epi16(_mm256_and_si256(_mm256_srli_epi16(in_pixel_0, 2), ymm_00ff),
                                            _mm256_and_si256(_mm256_srli_epi16(in_pixel_1, 2), ymm_00ff));
            out8_1_u8 = _mm256_packus_epi16(_mm256_and_si256(_mm256_srli_epi16(in_pixel_2, 2), ymm_00ff),
                                            _mm256_and_si256(_mm256_srli_epi16(in_pixel_3, 2), ymm_00ff));
            out8_0_u8 = _mm256_permute4x64_epi64(out8_0_u8, 0xd8);
            out8_1_u8 = _mm256_permute4x64_epi64(out8_1_u8, 0xd8);
            _mm256_storeu_si256((__m256i*)out8_bit_buffer, out8_0_u8);
            _mm256_storeu_si256((__m256i*)(out8_bit_buffer + out8_stride), out8_1_u8);

            out8_bit_buffer += 2 * out8_stride;
            in16_bit_buffer += 2 * in_stride;
        }
    } else if (width == 64) {
        __m256i in_pixel_0, in_pixel_1, in_pixel_2, in_pixel_3;
        __m256i outn0_u8, outn1_u8;
        __m256i out8_0_u8, out8_1_u8;

        for (y = 0; y < height; ++y) {
            in_pixel_0 = _mm256_loadu_si256((__m256i*)(in16_bit_buffer + y * in_stride));
            in_pixel_1 = _mm256_loadu_si256((__m256i*)(in16_bit_buffer + y * in_stride + 16));
            in_pixel_2 = _mm256_loadu_si256((__m256i*)(in16_bit_buffer + y * in_stride + 32));
            in_pixel_3 = _mm256_loadu_si256((__m256i*)(in16_bit_buffer + y * in_stride + 48));

            if (outn_bit_buffer) {
                outn0_u8 = _mm256_packus_epi16(_mm256_and_si256(in_pixel_0, ymm_3),
                                               _mm256_and_si256(in_pixel_1, ymm_3));
                outn1_u8 = _mm256_packus_epi16(_mm256_and_si256(in_pixel_2, ymm_3),
                                               _mm256_and_si256(in_pixel_3, ymm_3));
                outn0_u8 = _mm256_slli_epi16(_mm256_permute4x64_epi64(outn0_u8, 0xd8), 6);
                outn1_u8 = _mm256_slli_epi16(_mm256_permute4x64_epi64(outn1_u8, 0xd8), 6);
                _mm256_storeu_si256((__m256i*)(outn_bit_buffer + y * outn_stride), outn0_u8);
                _mm256_storeu_si256((__m256i*)(outn_bit_buffer + y * outn_stride + 32), outn1_u8);
            }

            out8_0_u8 = _mm256_packus_epi16(_mm256_and_si256(_mm256_srli_epi16(in_pixel_0, 2), ymm_00ff),
                                            _mm256_and_si256(_mm256_srli_epi16(in_pixel_1, 2), ymm_00ff));
            out8_1_u8 = _mm256_packus_epi16(_mm256_and_si256(_mm256_srli_epi16(in_pixel_2, 2), ymm_00ff),
                                            _mm256_and_si256(_mm256_srli_epi16(in_pixel_3, 2), ymm_00ff));
            out8_0_u8 = _mm256_permute4x64_epi64(out8_0_u8, 0xd8);
            out8_1_u8 = _mm256_permute4x64_epi64(out8_1_u8, 0xd8);
            _mm256_storeu_si256((__m256i*)(out8_bit_buffer + y * out8_stride), out8_0_u8);
            _mm256_storeu_si256((__m256i*)(out8_bit_buffer + y * out8_stride + 32), out8_1_u8);
        }

    } else {
        uint32_t in_stride_diff    = (2 * in_stride) - width;
        uint32_t out8_stride_diff  = (2 * out8_stride) - width;
        uint32_t out_n_stride_diff = (2 * outn_stride) - width;

        uint32_t in_stride_diff64    = in_stride - width;
        uint32_t out8_stride_diff64  = out8_stride - width;
        uint32_t out_n_stride_diff64 = outn_stride - width;

        if (!(width & 63)) {
            __m256i in_pixel_0, in_pixel_1, in_pixel_2, in_pixel_3;
            __m256i outn0_u8, outn1_u8;
            __m256i out8_0_u8, out8_1_u8;

            for (x = 0; x < height; x += 1) {
                for (y = 0; y < width; y += 64) {
                    in_pixel_0 = _mm256_loadu_si256((__m256i*)in16_bit_buffer);
                    in_pixel_1 = _mm256_loadu_si256((__m256i*)(in16_bit_buffer + 16));
                    in_pixel_2 = _mm256_loadu_si256((__m256i*)(in16_bit_buffer + 32));
                    in_pixel_3 = _mm256_loadu_si256((__m256i*)(in16_bit_buffer + 48));

                    if (outn_bit_buffer) {
                        outn0_u8 = _mm256_packus_epi16(_mm256_and_si256(in_pixel_0, ymm_3),
                                                       _mm256_and_si256(in_pixel_1, ymm_3));
                        outn1_u8 = _mm256_packus_epi16(_mm256_and_si256(in_pixel_2, ymm_3),
                                                       _mm256_and_si256(in_pixel_3, ymm_3));
                        outn0_u8 = _mm256_slli_epi16(_mm256_permute4x64_epi64(outn0_u8, 0xd8), 6);
                        outn1_u8 = _mm256_slli_epi16(_mm256_permute4x64_epi64(outn1_u8, 0xd8), 6);
                        _mm256_storeu_si256((__m256i*)outn_bit_buffer, outn0_u8);
                        _mm256_storeu_si256((__m256i*)(outn_bit_buffer + 32), outn1_u8);
                        outn_bit_buffer += 64;
                    }

                    out8_0_u8 = _mm256_packus_epi16(_mm256_and_si256(_mm256_srli_epi16(in_pixel_0, 2), ymm_00ff),
                                                    _mm256_and_si256(_mm256_srli_epi16(in_pixel_1, 2), ymm_00ff));
                    out8_1_u8 = _mm256_packus_epi16(_mm256_and_si256(_mm256_srli_epi16(in_pixel_2, 2), ymm_00ff),
                                                    _mm256_and_si256(_mm256_srli_epi16(in_pixel_3, 2), ymm_00ff));
                    out8_0_u8 = _mm256_permute4x64_epi64(out8_0_u8, 0xd8);
                    out8_1_u8 = _mm256_permute4x64_epi64(out8_1_u8, 0xd8);
                    _mm256_storeu_si256((__m256i*)out8_bit_buffer, out8_0_u8);
                    _mm256_storeu_si256((__m256i*)(out8_bit_buffer + 32), out8_1_u8);

                    out8_bit_buffer += 64;
                    in16_bit_buffer += 64;
                }
                in16_bit_buffer += in_stride_diff64;
                if (outn_bit_buffer) {
                    outn_bit_buffer += out_n_stride_diff64;
                }
                out8_bit_buffer += out8_stride_diff64;
            }
        } else if (!(width & 31)) {
            __m256i in_pixel_0, in_pixel_1, in_pixel_2, in_pixel_3;
            __m256i outn0_u8, outn1_u8;
            __m256i out8_0_u8, out8_1_u8;

            for (x = 0; x < height; x += 2) {
                for (y = 0; y < width; y += 32) {
                    in_pixel_0 = _mm256_loadu_si256((__m256i*)in16_bit_buffer);
                    in_pixel_1 = _mm256_loadu_si256((__m256i*)(in16_bit_buffer + 16));
                    in_pixel_2 = _mm256_loadu_si256((__m256i*)(in16_bit_buffer + in_stride));
                    in_pixel_3 = _mm256_loadu_si256((__m256i*)(in16_bit_buffer + in_stride + 16));

                    if (outn_bit_buffer) {
                        outn0_u8 = _mm256_packus_epi16(_mm256_and_si256(in_pixel_0, ymm_3),
                                                       _mm256_and_si256(in_pixel_1, ymm_3));
                        outn1_u8 = _mm256_packus_epi16(_mm256_and_si256(in_pixel_2, ymm_3),
                                                       _mm256_and_si256(in_pixel_3, ymm_3));
                        outn0_u8 = _mm256_slli_epi16(_mm256_permute4x64_epi64(outn0_u8, 0xd8), 6);
                        outn1_u8 = _mm256_slli_epi16(_mm256_permute4x64_epi64(outn1_u8, 0xd8), 6);
                        _mm256_storeu_si256((__m256i*)outn_bit_buffer, outn0_u8);
                        _mm256_storeu_si256((__m256i*)(outn_bit_buffer + outn_stride), outn1_u8);
                        outn_bit_buffer += 32;
                    }

                    out8_0_u8 = _mm256_packus_epi16(_mm256_and_si256(_mm256_srli_epi16(in_pixel_0, 2), ymm_00ff),
                                                    _mm256_and_si256(_mm256_srli_epi16(in_pixel_1, 2), ymm_00ff));
                    out8_1_u8 = _mm256_packus_epi16(_mm256_and_si256(_mm256_srli_epi16(in_pixel_2, 2), ymm_00ff),
                                                    _mm256_and_si256(_mm256_srli_epi16(in_pixel_3, 2), ymm_00ff));
                    out8_0_u8 = _mm256_permute4x64_epi64(out8_0_u8, 0xd8);
                    out8_1_u8 = _mm256_permute4x64_epi64(out8_1_u8, 0xd8);
                    _mm256_storeu_si256((__m256i*)out8_bit_buffer, out8_0_u8);
                    _mm256_storeu_si256((__m256i*)(out8_bit_buffer + out8_stride), out8_1_u8);

                    out8_bit_buffer += 32;
                    in16_bit_buffer += 32;
                }
                in16_bit_buffer += in_stride_diff;
                if (outn_bit_buffer) {
                    outn_bit_buffer += out_n_stride_diff;
                }
                out8_bit_buffer += out8_stride_diff;
            }
        } else if (!(width & 15)) {
            __m256i in_pixel_0, in_pixel_1, in_pixel_0_shft_r_2_u8, temp_pixel_0_u8;

            for (x = 0; x < height; x += 2) {
                for (y = 0; y < width; y += 16) {
                    in_pixel_0 = _mm256_loadu_si256((__m256i*)in16_bit_buffer);
                    in_pixel_1 = _mm256_loadu_si256((__m256i*)(in16_bit_buffer + in_stride));

                    if (outn_bit_buffer) {
                        temp_pixel_0_u8 = _mm256_packus_epi16(_mm256_and_si256(in_pixel_0, ymm_3),
                                                              _mm256_and_si256(in_pixel_1, ymm_3));
                        temp_pixel_0_u8 = _mm256_slli_epi16(_mm256_permute4x64_epi64(temp_pixel_0_u8, 0xd8), 6);
                        _mm_storeu_si128((__m128i*)outn_bit_buffer, _mm256_castsi256_si128(temp_pixel_0_u8));
                        _mm_storeu_si128((__m128i*)(outn_bit_buffer + outn_stride),
                                         _mm256_extracti128_si256(temp_pixel_0_u8, 1));
                        outn_bit_buffer += 16;
                    }

                    in_pixel_0_shft_r_2_u8 = _mm256_packus_epi16(
                        _mm256_and_si256(_mm256_srli_epi16(in_pixel_0, 2), ymm_00ff),
                        _mm256_and_si256(_mm256_srli_epi16(in_pixel_1, 2), ymm_00ff));
                    in_pixel_0_shft_r_2_u8 = _mm256_permute4x64_epi64(in_pixel_0_shft_r_2_u8, 0xd8);
                    _mm_storeu_si128((__m128i*)out8_bit_buffer, _mm256_castsi256_si128(in_pixel_0_shft_r_2_u8));
                    _mm_storeu_si128((__m128i*)(out8_bit_buffer + out8_stride),
                                     _mm256_extracti128_si256(in_pixel_0_shft_r_2_u8, 1));

                    out8_bit_buffer += 16;
                    in16_bit_buffer += 16;
                }
                in16_bit_buffer += in_stride_diff;
                if (outn_bit_buffer) {
                    outn_bit_buffer += out_n_stride_diff;
                }
                out8_bit_buffer += out8_stride_diff;
            }
        } else if (!(width & 7)) {
            for (x = 0; x < height; x += 2) {
                for (y = 0; y < width; y += 8) {
                    in_pixel0 = _mm_loadu_si128((__m128i*)in16_bit_buffer);
                    in_pixel1 = _mm_loadu_si128((__m128i*)(in16_bit_buffer + in_stride));

                    if (outn_bit_buffer) {
                        temp_pixel0    = _mm_slli_epi16(_mm_and_si128(in_pixel0, xmm_3), 6);
                        temp_pixel1    = _mm_slli_epi16(_mm_and_si128(in_pixel1, xmm_3), 6);
                        temp_pixel0_u8 = _mm_packus_epi16(temp_pixel0, temp_pixel0);
                        temp_pixel1_u8 = _mm_packus_epi16(temp_pixel1, temp_pixel1);
                        _mm_storel_epi64((__m128i*)outn_bit_buffer, temp_pixel0_u8);
                        _mm_storel_epi64((__m128i*)(outn_bit_buffer + outn_stride), temp_pixel1_u8);
                        outn_bit_buffer += 8;
                    }

                    in_pixel0_shft_r_2 = _mm_and_si128(_mm_srli_epi16(in_pixel0, 2), xmm_00ff);
                    in_pixel1_shft_r_2 = _mm_and_si128(_mm_srli_epi16(in_pixel1, 2), xmm_00ff);

                    in_pixel0_shft_r_2_u8 = _mm_packus_epi16(in_pixel0_shft_r_2, in_pixel0_shft_r_2);
                    in_pixel1_shft_r_2_u8 = _mm_packus_epi16(in_pixel1_shft_r_2, in_pixel1_shft_r_2);
                    _mm_storel_epi64((__m128i*)out8_bit_buffer, in_pixel0_shft_r_2_u8);
                    _mm_storel_epi64((__m128i*)(out8_bit_buffer + out8_stride), in_pixel1_shft_r_2_u8);

                    out8_bit_buffer += 8;
                    in16_bit_buffer += 8;
                }
                in16_bit_buffer += in_stride_diff;
                if (outn_bit_buffer) {
                    outn_bit_buffer += out_n_stride_diff;
                }
                out8_bit_buffer += out8_stride_diff;
            }
        } else {
            for (x = 0; x < height; x += 2) {
                for (y = 0; y < width; y += 4) {
                    in_pixel0 = _mm_loadl_epi64((__m128i*)in16_bit_buffer);
                    in_pixel1 = _mm_loadl_epi64((__m128i*)(in16_bit_buffer + in_stride));

                    if (outn_bit_buffer) {
                        temp_pixel0                 = _mm_slli_epi16(_mm_and_si128(in_pixel0, xmm_3), 6);
                        temp_pixel1                 = _mm_slli_epi16(_mm_and_si128(in_pixel1, xmm_3), 6);
                        temp_pixel0_u8              = _mm_packus_epi16(temp_pixel0, temp_pixel0);
                        temp_pixel1_u8              = _mm_packus_epi16(temp_pixel1, temp_pixel1);
                        *(uint32_t*)outn_bit_buffer = _mm_cvtsi128_si32(temp_pixel0_u8);
                        *(uint32_t*)(outn_bit_buffer + outn_stride) = _mm_cvtsi128_si32(temp_pixel1_u8);
                        outn_bit_buffer += 4;
                    }

                    in_pixel0_shft_r_2          = _mm_and_si128(_mm_srli_epi16(in_pixel0, 2), xmm_00ff);
                    in_pixel1_shft_r_2          = _mm_and_si128(_mm_srli_epi16(in_pixel1, 2), xmm_00ff);
                    in_pixel0_shft_r_2_u8       = _mm_packus_epi16(in_pixel0_shft_r_2, in_pixel0_shft_r_2);
                    in_pixel1_shft_r_2_u8       = _mm_packus_epi16(in_pixel1_shft_r_2, in_pixel1_shft_r_2);
                    *(uint32_t*)out8_bit_buffer = _mm_cvtsi128_si32(in_pixel0_shft_r_2_u8);
                    *(uint32_t*)(out8_bit_buffer + out8_stride) = _mm_cvtsi128_si32(in_pixel1_shft_r_2_u8);

                    out8_bit_buffer += 4;
                    in16_bit_buffer += 4;
                }
                in16_bit_buffer += in_stride_diff;
                if (outn_bit_buffer) {
                    outn_bit_buffer += out_n_stride_diff;
                }
                out8_bit_buffer += out8_stride_diff;
            }
        }
    }
    return;
}
