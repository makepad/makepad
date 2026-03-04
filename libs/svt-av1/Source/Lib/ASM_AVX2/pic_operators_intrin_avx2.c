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
#include <immintrin.h>
#include "pic_operators_inline_avx2.h"
#include "synonyms_avx2.h"

#include "pack_unpack_c.h"

#define _mm256_set_m128i(/* __m128i */ hi, /* __m128i */ lo) \
    _mm256_insertf128_si256(_mm256_castsi128_si256(lo), (hi), 0x1)

static INLINE void compressed_packmsb_32x2h(uint8_t* in8_bit_buffer, uint32_t in8_stride, uint8_t* inn_bit_buffer,
                                            uint32_t inn_stride, uint16_t* out16_bit_buffer, uint32_t out_stride,
                                            uint32_t height) {
    uint32_t y;
    __m256i  in_n_bit, in_8_bit, in_n_bit_stride, in_8bit_stride, concat0, concat1, concat2, concat3;
    __m256i  out0_15, out16_31, out_s0_s15, out_s16_s31;

    __m128i in_2_bit, ext0, ext1, ext2, ext3, ext01, ext23, ext01h, ext23h, ext0_15, ext16_31, ext32_47, ext48_63;
    __m128i msk0;

    msk0 = _mm_set1_epi8((int8_t)0xC0); //1100.000

    //processing 2 lines for chroma
    for (y = 0; y < height; y += 2) {
        //2 Lines Stored in 1D format-Could be replaced by 2 _mm_loadl_epi64
        in_2_bit = _mm_unpacklo_epi64(_mm_loadl_epi64((__m128i*)inn_bit_buffer),
                                      _mm_loadl_epi64((__m128i*)(inn_bit_buffer + inn_stride)));

        ext0 = _mm_and_si128(in_2_bit, msk0);
        ext1 = _mm_and_si128(_mm_slli_epi16(in_2_bit, 2), msk0);
        ext2 = _mm_and_si128(_mm_slli_epi16(in_2_bit, 4), msk0);
        ext3 = _mm_and_si128(_mm_slli_epi16(in_2_bit, 6), msk0);

        ext01    = _mm_unpacklo_epi8(ext0, ext1);
        ext23    = _mm_unpacklo_epi8(ext2, ext3);
        ext0_15  = _mm_unpacklo_epi16(ext01, ext23);
        ext16_31 = _mm_unpackhi_epi16(ext01, ext23);

        ext01h   = _mm_unpackhi_epi8(ext0, ext1);
        ext23h   = _mm_unpackhi_epi8(ext2, ext3);
        ext32_47 = _mm_unpacklo_epi16(ext01h, ext23h);
        ext48_63 = _mm_unpackhi_epi16(ext01h, ext23h);

        in_n_bit        = _mm256_set_m128i(ext16_31, ext0_15);
        in_n_bit_stride = _mm256_set_m128i(ext48_63, ext32_47);

        in_8_bit       = _mm256_loadu_si256((__m256i*)in8_bit_buffer);
        in_8bit_stride = _mm256_loadu_si256((__m256i*)(in8_bit_buffer + in8_stride));

        //(out_pixel | n_bit_pixel) concatenation is done with unpacklo_epi8 and unpackhi_epi8
        concat0 = _mm256_srli_epi16(_mm256_unpacklo_epi8(in_n_bit, in_8_bit), 6);
        concat1 = _mm256_srli_epi16(_mm256_unpackhi_epi8(in_n_bit, in_8_bit), 6);
        concat2 = _mm256_srli_epi16(_mm256_unpacklo_epi8(in_n_bit_stride, in_8bit_stride), 6);
        concat3 = _mm256_srli_epi16(_mm256_unpackhi_epi8(in_n_bit_stride, in_8bit_stride), 6);

        //Re-organize the packing for writing to the out buffer
        out0_15     = yy_unpacklo_epi128(concat0, concat1);
        out16_31    = yy_unpackhi_epi128(concat0, concat1);
        out_s0_s15  = yy_unpacklo_epi128(concat2, concat3);
        out_s16_s31 = yy_unpackhi_epi128(concat2, concat3);

        _mm256_storeu_si256((__m256i*)out16_bit_buffer, out0_15);
        _mm256_storeu_si256((__m256i*)(out16_bit_buffer + 16), out16_31);
        _mm256_storeu_si256((__m256i*)(out16_bit_buffer + out_stride), out_s0_s15);
        _mm256_storeu_si256((__m256i*)(out16_bit_buffer + out_stride + 16), out_s16_s31);

        in8_bit_buffer += in8_stride << 1;
        inn_bit_buffer += inn_stride << 1;
        out16_bit_buffer += out_stride << 1;
    }
}

static INLINE void compressed_packmsb_64xh(uint8_t* in8_bit_buffer, uint32_t in8_stride, uint8_t* inn_bit_buffer,
                                           uint32_t inn_stride, uint16_t* out16_bit_buffer, uint32_t out_stride,
                                           uint32_t height) {
    uint32_t y;
    __m256i  in_n_bit, in_8_bit, in_n_bit32, in_8_bit32;
    __m256i  concat0, concat1, concat2, concat3;
    __m256i  out_0_15, out16_31, out32_47, out_48_63;

    __m128i in_2_bit, ext0, ext1, ext2, ext3, ext01, ext23, ext01h, ext23h, ext0_15, ext16_31, ext32_47, ext48_63;
    __m128i msk;

    msk = _mm_set1_epi8((int8_t)0xC0); //1100.000

    //One row per iter
    for (y = 0; y < height; y++) {
        in_2_bit = _mm_loadu_si128((__m128i*)inn_bit_buffer);

        ext0 = _mm_and_si128(in_2_bit, msk);
        ext1 = _mm_and_si128(_mm_slli_epi16(in_2_bit, 2), msk);
        ext2 = _mm_and_si128(_mm_slli_epi16(in_2_bit, 4), msk);
        ext3 = _mm_and_si128(_mm_slli_epi16(in_2_bit, 6), msk);

        ext01    = _mm_unpacklo_epi8(ext0, ext1);
        ext23    = _mm_unpacklo_epi8(ext2, ext3);
        ext0_15  = _mm_unpacklo_epi16(ext01, ext23);
        ext16_31 = _mm_unpackhi_epi16(ext01, ext23);

        ext01h   = _mm_unpackhi_epi8(ext0, ext1);
        ext23h   = _mm_unpackhi_epi8(ext2, ext3);
        ext32_47 = _mm_unpacklo_epi16(ext01h, ext23h);
        ext48_63 = _mm_unpackhi_epi16(ext01h, ext23h);

        in_n_bit   = _mm256_set_m128i(ext16_31, ext0_15);
        in_n_bit32 = _mm256_set_m128i(ext48_63, ext32_47);

        in_8_bit   = _mm256_loadu_si256((__m256i*)in8_bit_buffer);
        in_8_bit32 = _mm256_loadu_si256((__m256i*)(in8_bit_buffer + 32));

        //(out_pixel | n_bit_pixel) concatenation
        concat0 = _mm256_srli_epi16(_mm256_unpacklo_epi8(in_n_bit, in_8_bit), 6);
        concat1 = _mm256_srli_epi16(_mm256_unpackhi_epi8(in_n_bit, in_8_bit), 6);
        concat2 = _mm256_srli_epi16(_mm256_unpacklo_epi8(in_n_bit32, in_8_bit32), 6);
        concat3 = _mm256_srli_epi16(_mm256_unpackhi_epi8(in_n_bit32, in_8_bit32), 6);

        //Re-organize the packing for writing to the out buffer
        out_0_15  = yy_unpacklo_epi128(concat0, concat1);
        out16_31  = yy_unpackhi_epi128(concat0, concat1);
        out32_47  = yy_unpacklo_epi128(concat2, concat3);
        out_48_63 = yy_unpackhi_epi128(concat2, concat3);

        _mm256_storeu_si256((__m256i*)out16_bit_buffer, out_0_15);
        _mm256_storeu_si256((__m256i*)(out16_bit_buffer + 16), out16_31);
        _mm256_storeu_si256((__m256i*)(out16_bit_buffer + 32), out32_47);
        _mm256_storeu_si256((__m256i*)(out16_bit_buffer + 48), out_48_63);

        in8_bit_buffer += in8_stride;
        inn_bit_buffer += inn_stride;
        out16_bit_buffer += out_stride;
    }
}

static INLINE void compressed_packmsb_128(uint8_t* in8_bit_buffer, uint8_t* inn_bit_buffer, uint16_t* out16_bit_buffer,
                                          uint32_t width_rep) {
    uint32_t w;
    __m256i  in_n_bit, in_8_bit, in_n_bit32, in_8_bit32, in_n_bit64, in_8_bit64, in_n_bit96, in_8_bit96;
    __m256i  concat0, concat1, concat2, concat3;
    __m256i  out_0_15, out16_31, out32_47, out_48_63;

    __m256i in_2_bit, ext0, ext1, ext2, ext3, ext01, ext23, ext01h, ext23h, ext0_15, ext16_31, ext32_47, ext48_63;
    __m256i msk;

    msk = _mm256_set1_epi8((int8_t)0xC0); //1100.000

    //One row per iter
    for (w = 0; w < width_rep; w++) {
        in_2_bit = _mm256_loadu_si256((__m256i*)(inn_bit_buffer + w * 32));

        ext0 = _mm256_and_si256(in_2_bit, msk);
        ext1 = _mm256_and_si256(_mm256_slli_epi16(in_2_bit, 2), msk);
        ext2 = _mm256_and_si256(_mm256_slli_epi16(in_2_bit, 4), msk);
        ext3 = _mm256_and_si256(_mm256_slli_epi16(in_2_bit, 6), msk);

        ext01    = _mm256_unpacklo_epi8(ext0, ext1);
        ext23    = _mm256_unpacklo_epi8(ext2, ext3);
        ext0_15  = _mm256_unpacklo_epi16(ext01, ext23);
        ext16_31 = _mm256_unpackhi_epi16(ext01, ext23);

        ext01h   = _mm256_unpackhi_epi8(ext0, ext1);
        ext23h   = _mm256_unpackhi_epi8(ext2, ext3);
        ext32_47 = _mm256_unpacklo_epi16(ext01h, ext23h);
        ext48_63 = _mm256_unpackhi_epi16(ext01h, ext23h);

        in_n_bit   = yy_unpacklo_epi128(ext0_15, ext16_31);
        in_n_bit32 = yy_unpacklo_epi128(ext32_47, ext48_63);
        in_n_bit64 = yy_unpackhi_epi128(ext0_15, ext16_31);
        in_n_bit96 = yy_unpackhi_epi128(ext32_47, ext48_63);

        in_8_bit   = _mm256_loadu_si256((__m256i*)(in8_bit_buffer + w * 128));
        in_8_bit32 = _mm256_loadu_si256((__m256i*)(in8_bit_buffer + w * 128 + 32));
        in_8_bit64 = _mm256_loadu_si256((__m256i*)(in8_bit_buffer + w * 128 + 64));
        in_8_bit96 = _mm256_loadu_si256((__m256i*)(in8_bit_buffer + w * 128 + 96));

        //(out_pixel | n_bit_pixel) concatenation
        concat0 = _mm256_srli_epi16(_mm256_unpacklo_epi8(in_n_bit, in_8_bit), 6);
        concat1 = _mm256_srli_epi16(_mm256_unpackhi_epi8(in_n_bit, in_8_bit), 6);
        concat2 = _mm256_srli_epi16(_mm256_unpacklo_epi8(in_n_bit32, in_8_bit32), 6);
        concat3 = _mm256_srli_epi16(_mm256_unpackhi_epi8(in_n_bit32, in_8_bit32), 6);

        //Re-organize the packing for writing to the out buffer
        out_0_15  = yy_unpacklo_epi128(concat0, concat1);
        out16_31  = yy_unpackhi_epi128(concat0, concat1);
        out32_47  = yy_unpacklo_epi128(concat2, concat3);
        out_48_63 = yy_unpackhi_epi128(concat2, concat3);

        _mm256_storeu_si256((__m256i*)(out16_bit_buffer + w * 128), out_0_15);
        _mm256_storeu_si256((__m256i*)(out16_bit_buffer + w * 128 + 16), out16_31);
        _mm256_storeu_si256((__m256i*)(out16_bit_buffer + w * 128 + 32), out32_47);
        _mm256_storeu_si256((__m256i*)(out16_bit_buffer + w * 128 + 48), out_48_63);

        //store data in range 64 .. 127
        concat0 = _mm256_srli_epi16(_mm256_unpacklo_epi8(in_n_bit64, in_8_bit64), 6);
        concat1 = _mm256_srli_epi16(_mm256_unpackhi_epi8(in_n_bit64, in_8_bit64), 6);
        concat2 = _mm256_srli_epi16(_mm256_unpacklo_epi8(in_n_bit96, in_8_bit96), 6);
        concat3 = _mm256_srli_epi16(_mm256_unpackhi_epi8(in_n_bit96, in_8_bit96), 6);

        //Re-organize the packing for writing to the out buffer
        out_0_15  = yy_unpacklo_epi128(concat0, concat1);
        out16_31  = yy_unpackhi_epi128(concat0, concat1);
        out32_47  = yy_unpacklo_epi128(concat2, concat3);
        out_48_63 = yy_unpackhi_epi128(concat2, concat3);

        _mm256_storeu_si256((__m256i*)(out16_bit_buffer + w * 128 + 64), out_0_15);
        _mm256_storeu_si256((__m256i*)(out16_bit_buffer + w * 128 + 80), out16_31);
        _mm256_storeu_si256((__m256i*)(out16_bit_buffer + w * 128 + 96), out32_47);
        _mm256_storeu_si256((__m256i*)(out16_bit_buffer + w * 128 + 112), out_48_63);
    }
}

void svt_compressed_packmsb_avx2_intrin(uint8_t* in8_bit_buffer, uint32_t in8_stride, uint8_t* inn_bit_buffer,
                                        uint32_t inn_stride, uint16_t* out16_bit_buffer, uint32_t out_stride,
                                        uint32_t width, uint32_t height) {
    if (width == 32) {
        compressed_packmsb_32x2h(
            in8_bit_buffer, in8_stride, inn_bit_buffer, inn_stride, out16_bit_buffer, out_stride, height);
    } else if (width == 64) {
        compressed_packmsb_64xh(
            in8_bit_buffer, in8_stride, inn_bit_buffer, inn_stride, out16_bit_buffer, out_stride, height);
    } else if (width == 128) {
        for (uint32_t y = 0; y < height; y++) {
            compressed_packmsb_128(
                in8_bit_buffer + y * in8_stride, inn_bit_buffer + y * inn_stride, out16_bit_buffer + y * out_stride, 1);
        }
    } else {
        int32_t  leftover     = width;
        uint32_t offset8b_16b = 0;
        uint32_t offset2b     = 0;
        if (leftover >= 128) {
            uint32_t offset = width & 0xffffff80;
            for (uint32_t y = 0; y < height; y++) {
                compressed_packmsb_128(in8_bit_buffer + y * in8_stride,
                                       inn_bit_buffer + y * inn_stride,
                                       out16_bit_buffer + y * out_stride,
                                       width >> 7);
            }
            offset8b_16b += offset;
            offset2b += offset >> 2;
            leftover -= offset;
        }
        if (leftover >= 64) {
            compressed_packmsb_64xh(in8_bit_buffer + offset8b_16b,
                                    in8_stride,
                                    inn_bit_buffer + offset2b,
                                    inn_stride,
                                    out16_bit_buffer + offset8b_16b,
                                    out_stride,
                                    height);
            offset8b_16b += 64;
            offset2b += 16;
            leftover -= 64;
        }
        if (leftover >= 32) {
            compressed_packmsb_32x2h(in8_bit_buffer + offset8b_16b,
                                     in8_stride,
                                     inn_bit_buffer + offset2b,
                                     inn_stride,
                                     out16_bit_buffer + offset8b_16b,
                                     out_stride,
                                     height);
            offset8b_16b += 32;
            offset2b += 8;
            leftover -= 32;
        }
        if (leftover) {
            svt_compressed_packmsb_c(in8_bit_buffer + offset8b_16b,
                                     in8_stride,
                                     inn_bit_buffer + offset2b,
                                     inn_stride,
                                     out16_bit_buffer + offset8b_16b,
                                     out_stride,
                                     leftover,
                                     height);
        }
    }
}

void svt_enc_msb_pack2d_avx2_intrin_al(uint8_t* in8_bit_buffer, uint32_t in8_stride, uint8_t* inn_bit_buffer,
                                       uint16_t* out16_bit_buffer, uint32_t inn_stride, uint32_t out_stride,
                                       uint32_t width, uint32_t height) {
    //(out_pixel | n_bit_pixel) concatenation is done with unpacklo_epi8 and unpackhi_epi8

    uint32_t y, x;

    __m128i out0, out1;

    if (width == 4) {
        for (y = 0; y < height; y += 2) {
            out0 = _mm_srli_epi16(_mm_unpacklo_epi8(_mm_cvtsi32_si128(*(uint32_t*)inn_bit_buffer),
                                                    _mm_cvtsi32_si128(*(uint32_t*)in8_bit_buffer)),
                                  6);
            out1 = _mm_srli_epi16(_mm_unpacklo_epi8(_mm_cvtsi32_si128(*(uint32_t*)(inn_bit_buffer + inn_stride)),
                                                    _mm_cvtsi32_si128(*(uint32_t*)(in8_bit_buffer + in8_stride))),
                                  6);

            _mm_storel_epi64((__m128i*)out16_bit_buffer, out0);
            _mm_storel_epi64((__m128i*)(out16_bit_buffer + out_stride), out1);

            in8_bit_buffer += in8_stride << 1;
            inn_bit_buffer += inn_stride << 1;
            out16_bit_buffer += out_stride << 1;
        }
    } else if (width == 8) {
        for (y = 0; y < height; y += 2) {
            out0 = _mm_srli_epi16(
                _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)inn_bit_buffer), _mm_loadl_epi64((__m128i*)in8_bit_buffer)),
                6);
            out1 = _mm_srli_epi16(_mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(inn_bit_buffer + inn_stride)),
                                                    _mm_loadl_epi64((__m128i*)(in8_bit_buffer + in8_stride))),
                                  6);

            _mm_storeu_si128((__m128i*)out16_bit_buffer, out0);
            _mm_storeu_si128((__m128i*)(out16_bit_buffer + out_stride), out1);

            in8_bit_buffer += in8_stride << 1;
            inn_bit_buffer += inn_stride << 1;
            out16_bit_buffer += out_stride << 1;
        }
    } else if (width == 16) {
        __m128i in_n_bit, in_8_bit, in_n_bit_stride, in_8bit_stride, out2, out3;

        for (y = 0; y < height; y += 2) {
            in_n_bit        = _mm_loadu_si128((__m128i*)inn_bit_buffer);
            in_8_bit        = _mm_loadu_si128((__m128i*)in8_bit_buffer);
            in_n_bit_stride = _mm_loadu_si128((__m128i*)(inn_bit_buffer + inn_stride));
            in_8bit_stride  = _mm_loadu_si128((__m128i*)(in8_bit_buffer + in8_stride));

            out0 = _mm_srli_epi16(_mm_unpacklo_epi8(in_n_bit, in_8_bit), 6);
            out1 = _mm_srli_epi16(_mm_unpackhi_epi8(in_n_bit, in_8_bit), 6);
            out2 = _mm_srli_epi16(_mm_unpacklo_epi8(in_n_bit_stride, in_8bit_stride), 6);
            out3 = _mm_srli_epi16(_mm_unpackhi_epi8(in_n_bit_stride, in_8bit_stride), 6);

            _mm_storeu_si128((__m128i*)out16_bit_buffer, out0);
            _mm_storeu_si128((__m128i*)(out16_bit_buffer + 8), out1);
            _mm_storeu_si128((__m128i*)(out16_bit_buffer + out_stride), out2);
            _mm_storeu_si128((__m128i*)(out16_bit_buffer + out_stride + 8), out3);

            in8_bit_buffer += in8_stride << 1;
            inn_bit_buffer += inn_stride << 1;
            out16_bit_buffer += out_stride << 1;
        }
    } else if (width == 24) {
        __m128i in_n_bit, in_8_bit, in_n_bit_stride, in_8bit_stride, out2, out3, out4, out5;

        for (y = 0; y < height; y += 2) {
            in_n_bit        = _mm_loadu_si128((__m128i*)inn_bit_buffer);
            in_8_bit        = _mm_loadu_si128((__m128i*)in8_bit_buffer);
            in_n_bit_stride = _mm_loadu_si128((__m128i*)(inn_bit_buffer + inn_stride));
            in_8bit_stride  = _mm_loadu_si128((__m128i*)(in8_bit_buffer + in8_stride));

            out0 = _mm_srli_epi16(_mm_unpacklo_epi8(in_n_bit, in_8_bit), 6);
            out1 = _mm_srli_epi16(_mm_unpackhi_epi8(in_n_bit, in_8_bit), 6);
            out2 = _mm_srli_epi16(_mm_unpacklo_epi8(in_n_bit_stride, in_8bit_stride), 6);
            out3 = _mm_srli_epi16(_mm_unpackhi_epi8(in_n_bit_stride, in_8bit_stride), 6);

            out4 = _mm_srli_epi16(_mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(inn_bit_buffer + 16)),
                                                    _mm_loadl_epi64((__m128i*)(in8_bit_buffer + 16))),
                                  6);
            out5 = _mm_srli_epi16(_mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(inn_bit_buffer + inn_stride + 16)),
                                                    _mm_loadl_epi64((__m128i*)(in8_bit_buffer + in8_stride + 16))),
                                  6);

            _mm_storeu_si128((__m128i*)out16_bit_buffer, out0);
            _mm_storeu_si128((__m128i*)(out16_bit_buffer + 8), out1);
            _mm_storeu_si128((__m128i*)(out16_bit_buffer + out_stride), out2);
            _mm_storeu_si128((__m128i*)(out16_bit_buffer + out_stride + 8), out3);
            _mm_storeu_si128((__m128i*)(out16_bit_buffer + 16), out4);
            _mm_storeu_si128((__m128i*)(out16_bit_buffer + out_stride + 16), out5);

            in8_bit_buffer += in8_stride << 1;
            inn_bit_buffer += inn_stride << 1;
            out16_bit_buffer += out_stride << 1;
        }
    } else if (width == 32) {
        __m256i in_n_bit, in_8_bit, in_n_bit_stride, in_8bit_stride, concat0, concat1, concat2, concat3;
        __m256i out0_15, out16_31, out_s0_s15, out_s16_s31;

        for (y = 0; y < height; y += 2) {
            in_n_bit        = _mm256_loadu_si256((__m256i*)inn_bit_buffer);
            in_8_bit        = _mm256_loadu_si256((__m256i*)in8_bit_buffer);
            in_n_bit_stride = _mm256_loadu_si256((__m256i*)(inn_bit_buffer + inn_stride));
            in_8bit_stride  = _mm256_loadu_si256((__m256i*)(in8_bit_buffer + in8_stride));

            //(out_pixel | n_bit_pixel) concatenation is done with unpacklo_epi8 and unpackhi_epi8
            concat0 = _mm256_srli_epi16(_mm256_unpacklo_epi8(in_n_bit, in_8_bit), 6);
            concat1 = _mm256_srli_epi16(_mm256_unpackhi_epi8(in_n_bit, in_8_bit), 6);
            concat2 = _mm256_srli_epi16(_mm256_unpacklo_epi8(in_n_bit_stride, in_8bit_stride), 6);
            concat3 = _mm256_srli_epi16(_mm256_unpackhi_epi8(in_n_bit_stride, in_8bit_stride), 6);

            //Re-organize the packing for writing to the out buffer
            out0_15     = yy_unpacklo_epi128(concat0, concat1);
            out16_31    = yy_unpackhi_epi128(concat0, concat1);
            out_s0_s15  = yy_unpacklo_epi128(concat2, concat3);
            out_s16_s31 = yy_unpackhi_epi128(concat2, concat3);

            _mm256_storeu_si256((__m256i*)out16_bit_buffer, out0_15);
            _mm256_storeu_si256((__m256i*)(out16_bit_buffer + 16), out16_31);
            _mm256_storeu_si256((__m256i*)(out16_bit_buffer + out_stride), out_s0_s15);
            _mm256_storeu_si256((__m256i*)(out16_bit_buffer + out_stride + 16), out_s16_s31);

            in8_bit_buffer += in8_stride << 1;
            inn_bit_buffer += inn_stride << 1;
            out16_bit_buffer += out_stride << 1;
        }
    } else if (width == 48) {
        __m128i xx_in_n_bit, xx_in_8_bit, xx_in_n_bit_stride, xx_in_8bit_stride, out2, out3;
        __m256i in_n_bit, in_8_bit, in_n_bit_stride, in_8bit_stride, concat0, concat1, concat2, concat3;
        __m256i out0_15, out16_31, out_s0_s15, out_s16_s31;

        for (y = 0; y < height; y += 2) {
            in_n_bit           = _mm256_loadu_si256((__m256i*)inn_bit_buffer);
            in_8_bit           = _mm256_loadu_si256((__m256i*)in8_bit_buffer);
            in_n_bit_stride    = _mm256_loadu_si256((__m256i*)(inn_bit_buffer + inn_stride));
            in_8bit_stride     = _mm256_loadu_si256((__m256i*)(in8_bit_buffer + in8_stride));
            xx_in_n_bit        = _mm_loadu_si128((__m128i*)(inn_bit_buffer + 32));
            xx_in_8_bit        = _mm_loadu_si128((__m128i*)(in8_bit_buffer + 32));
            xx_in_n_bit_stride = _mm_loadu_si128((__m128i*)(inn_bit_buffer + inn_stride + 32));
            xx_in_8bit_stride  = _mm_loadu_si128((__m128i*)(in8_bit_buffer + in8_stride + 32));

            //(out_pixel | n_bit_pixel) concatenation is done with unpacklo_epi8 and unpackhi_epi8
            concat0 = _mm256_srli_epi16(_mm256_unpacklo_epi8(in_n_bit, in_8_bit), 6);
            concat1 = _mm256_srli_epi16(_mm256_unpackhi_epi8(in_n_bit, in_8_bit), 6);
            concat2 = _mm256_srli_epi16(_mm256_unpacklo_epi8(in_n_bit_stride, in_8bit_stride), 6);
            concat3 = _mm256_srli_epi16(_mm256_unpackhi_epi8(in_n_bit_stride, in_8bit_stride), 6);

            out0 = _mm_srli_epi16(_mm_unpacklo_epi8(xx_in_n_bit, xx_in_8_bit), 6);
            out1 = _mm_srli_epi16(_mm_unpackhi_epi8(xx_in_n_bit, xx_in_8_bit), 6);
            out2 = _mm_srli_epi16(_mm_unpacklo_epi8(xx_in_n_bit_stride, xx_in_8bit_stride), 6);
            out3 = _mm_srli_epi16(_mm_unpackhi_epi8(xx_in_n_bit_stride, xx_in_8bit_stride), 6);

            //Re-organize the packing for writing to the out buffer
            out0_15     = yy_unpacklo_epi128(concat0, concat1);
            out16_31    = yy_unpackhi_epi128(concat0, concat1);
            out_s0_s15  = yy_unpacklo_epi128(concat2, concat3);
            out_s16_s31 = yy_unpackhi_epi128(concat2, concat3);

            _mm256_storeu_si256((__m256i*)out16_bit_buffer, out0_15);
            _mm256_storeu_si256((__m256i*)(out16_bit_buffer + 16), out16_31);
            _mm256_storeu_si256((__m256i*)(out16_bit_buffer + out_stride), out_s0_s15);
            _mm256_storeu_si256((__m256i*)(out16_bit_buffer + out_stride + 16), out_s16_s31);

            _mm_storeu_si128((__m128i*)(out16_bit_buffer + 32), out0);
            _mm_storeu_si128((__m128i*)(out16_bit_buffer + 40), out1);
            _mm_storeu_si128((__m128i*)(out16_bit_buffer + out_stride + 32), out2);
            _mm_storeu_si128((__m128i*)(out16_bit_buffer + out_stride + 40), out3);

            in8_bit_buffer += in8_stride << 1;
            inn_bit_buffer += inn_stride << 1;
            out16_bit_buffer += out_stride << 1;
        }
    } else if (width == 64) {
        __m256i in_n_bit, in_8_bit, in_n_bit_stride, in_8bit_stride, in_n_bit32, in_8_bit32, in_n_bitStride32,
            in_8bit_stride32;
        __m256i concat0, concat1, concat2, concat3, concat4, concat5, concat6, concat7;
        __m256i out_0_15, out16_31, out32_47, out_48_63, out_s0_s15, out_s16_s31, out_s32_s47, out_s48_s63;

        for (y = 0; y < height; y += 2) {
            in_n_bit         = _mm256_loadu_si256((__m256i*)inn_bit_buffer);
            in_8_bit         = _mm256_loadu_si256((__m256i*)in8_bit_buffer);
            in_n_bit32       = _mm256_loadu_si256((__m256i*)(inn_bit_buffer + 32));
            in_8_bit32       = _mm256_loadu_si256((__m256i*)(in8_bit_buffer + 32));
            in_n_bit_stride  = _mm256_loadu_si256((__m256i*)(inn_bit_buffer + inn_stride));
            in_8bit_stride   = _mm256_loadu_si256((__m256i*)(in8_bit_buffer + in8_stride));
            in_n_bitStride32 = _mm256_loadu_si256((__m256i*)(inn_bit_buffer + inn_stride + 32));
            in_8bit_stride32 = _mm256_loadu_si256((__m256i*)(in8_bit_buffer + in8_stride + 32));
            //(out_pixel | n_bit_pixel) concatenation is done with unpacklo_epi8 and unpackhi_epi8
            concat0 = _mm256_srli_epi16(_mm256_unpacklo_epi8(in_n_bit, in_8_bit), 6);
            concat1 = _mm256_srli_epi16(_mm256_unpackhi_epi8(in_n_bit, in_8_bit), 6);
            concat2 = _mm256_srli_epi16(_mm256_unpacklo_epi8(in_n_bit32, in_8_bit32), 6);
            concat3 = _mm256_srli_epi16(_mm256_unpackhi_epi8(in_n_bit32, in_8_bit32), 6);
            concat4 = _mm256_srli_epi16(_mm256_unpacklo_epi8(in_n_bit_stride, in_8bit_stride), 6);
            concat5 = _mm256_srli_epi16(_mm256_unpackhi_epi8(in_n_bit_stride, in_8bit_stride), 6);
            concat6 = _mm256_srli_epi16(_mm256_unpacklo_epi8(in_n_bitStride32, in_8bit_stride32), 6);
            concat7 = _mm256_srli_epi16(_mm256_unpackhi_epi8(in_n_bitStride32, in_8bit_stride32), 6);

            //Re-organize the packing for writing to the out buffer
            out_0_15    = yy_unpacklo_epi128(concat0, concat1);
            out16_31    = yy_unpackhi_epi128(concat0, concat1);
            out32_47    = yy_unpacklo_epi128(concat2, concat3);
            out_48_63   = yy_unpackhi_epi128(concat2, concat3);
            out_s0_s15  = yy_unpacklo_epi128(concat4, concat5);
            out_s16_s31 = yy_unpackhi_epi128(concat4, concat5);
            out_s32_s47 = yy_unpacklo_epi128(concat6, concat7);
            out_s48_s63 = yy_unpackhi_epi128(concat6, concat7);

            _mm256_storeu_si256((__m256i*)out16_bit_buffer, out_0_15);
            _mm256_storeu_si256((__m256i*)(out16_bit_buffer + 16), out16_31);
            _mm256_storeu_si256((__m256i*)(out16_bit_buffer + 32), out32_47);
            _mm256_storeu_si256((__m256i*)(out16_bit_buffer + 48), out_48_63);

            _mm256_storeu_si256((__m256i*)(out16_bit_buffer + out_stride), out_s0_s15);
            _mm256_storeu_si256((__m256i*)(out16_bit_buffer + out_stride + 16), out_s16_s31);
            _mm256_storeu_si256((__m256i*)(out16_bit_buffer + out_stride + 32), out_s32_s47);
            _mm256_storeu_si256((__m256i*)(out16_bit_buffer + out_stride + 48), out_s48_s63);

            in8_bit_buffer += in8_stride << 1;
            inn_bit_buffer += inn_stride << 1;
            out16_bit_buffer += out_stride << 1;
        }
    } else if (width == 80) {
        __m128i xx_in_n_bit, xx_in_8_bit, xx_in_n_bit_stride, xx_in_8bit_stride, out2, out3;
        __m256i in_n_bit, in_8_bit, in_n_bit_stride, in_8bit_stride, in_n_bit32, in_8_bit32, in_n_bitStride32,
            in_8bit_stride32;
        __m256i concat0, concat1, concat2, concat3, concat4, concat5, concat6, concat7;
        __m256i out_0_15, out16_31, out32_47, out_48_63, out_s0_s15, out_s16_s31, out_s32_s47, out_s48_s63;

        for (y = 0; y < height; y += 2) {
            in_n_bit           = _mm256_loadu_si256((__m256i*)inn_bit_buffer);
            in_8_bit           = _mm256_loadu_si256((__m256i*)in8_bit_buffer);
            in_n_bit32         = _mm256_loadu_si256((__m256i*)(inn_bit_buffer + 32));
            in_8_bit32         = _mm256_loadu_si256((__m256i*)(in8_bit_buffer + 32));
            in_n_bit_stride    = _mm256_loadu_si256((__m256i*)(inn_bit_buffer + inn_stride));
            in_8bit_stride     = _mm256_loadu_si256((__m256i*)(in8_bit_buffer + in8_stride));
            in_n_bitStride32   = _mm256_loadu_si256((__m256i*)(inn_bit_buffer + inn_stride + 32));
            in_8bit_stride32   = _mm256_loadu_si256((__m256i*)(in8_bit_buffer + in8_stride + 32));
            xx_in_n_bit        = _mm_loadu_si128((__m128i*)(inn_bit_buffer + 64));
            xx_in_8_bit        = _mm_loadu_si128((__m128i*)(in8_bit_buffer + 64));
            xx_in_n_bit_stride = _mm_loadu_si128((__m128i*)(inn_bit_buffer + inn_stride + 64));
            xx_in_8bit_stride  = _mm_loadu_si128((__m128i*)(in8_bit_buffer + in8_stride + 64));
            //(out_pixel | n_bit_pixel) concatenation is done with unpacklo_epi8 and unpackhi_epi8
            concat0 = _mm256_srli_epi16(_mm256_unpacklo_epi8(in_n_bit, in_8_bit), 6);
            concat1 = _mm256_srli_epi16(_mm256_unpackhi_epi8(in_n_bit, in_8_bit), 6);
            concat2 = _mm256_srli_epi16(_mm256_unpacklo_epi8(in_n_bit32, in_8_bit32), 6);
            concat3 = _mm256_srli_epi16(_mm256_unpackhi_epi8(in_n_bit32, in_8_bit32), 6);
            concat4 = _mm256_srli_epi16(_mm256_unpacklo_epi8(in_n_bit_stride, in_8bit_stride), 6);
            concat5 = _mm256_srli_epi16(_mm256_unpackhi_epi8(in_n_bit_stride, in_8bit_stride), 6);
            concat6 = _mm256_srli_epi16(_mm256_unpacklo_epi8(in_n_bitStride32, in_8bit_stride32), 6);
            concat7 = _mm256_srli_epi16(_mm256_unpackhi_epi8(in_n_bitStride32, in_8bit_stride32), 6);

            out0 = _mm_srli_epi16(_mm_unpacklo_epi8(xx_in_n_bit, xx_in_8_bit), 6);
            out1 = _mm_srli_epi16(_mm_unpackhi_epi8(xx_in_n_bit, xx_in_8_bit), 6);
            out2 = _mm_srli_epi16(_mm_unpacklo_epi8(xx_in_n_bit_stride, xx_in_8bit_stride), 6);
            out3 = _mm_srli_epi16(_mm_unpackhi_epi8(xx_in_n_bit_stride, xx_in_8bit_stride), 6);

            //Re-organize the packing for writing to the out buffer
            out_0_15    = yy_unpacklo_epi128(concat0, concat1);
            out16_31    = yy_unpackhi_epi128(concat0, concat1);
            out32_47    = yy_unpacklo_epi128(concat2, concat3);
            out_48_63   = yy_unpackhi_epi128(concat2, concat3);
            out_s0_s15  = yy_unpacklo_epi128(concat4, concat5);
            out_s16_s31 = yy_unpackhi_epi128(concat4, concat5);
            out_s32_s47 = yy_unpacklo_epi128(concat6, concat7);
            out_s48_s63 = yy_unpackhi_epi128(concat6, concat7);

            _mm256_storeu_si256((__m256i*)out16_bit_buffer, out_0_15);
            _mm256_storeu_si256((__m256i*)(out16_bit_buffer + 16), out16_31);
            _mm256_storeu_si256((__m256i*)(out16_bit_buffer + 32), out32_47);
            _mm256_storeu_si256((__m256i*)(out16_bit_buffer + 48), out_48_63);

            _mm256_storeu_si256((__m256i*)(out16_bit_buffer + out_stride), out_s0_s15);
            _mm256_storeu_si256((__m256i*)(out16_bit_buffer + out_stride + 16), out_s16_s31);
            _mm256_storeu_si256((__m256i*)(out16_bit_buffer + out_stride + 32), out_s32_s47);
            _mm256_storeu_si256((__m256i*)(out16_bit_buffer + out_stride + 48), out_s48_s63);

            _mm_storeu_si128((__m128i*)(out16_bit_buffer + 64), out0);
            _mm_storeu_si128((__m128i*)(out16_bit_buffer + 72), out1);
            _mm_storeu_si128((__m128i*)(out16_bit_buffer + out_stride + 64), out2);
            _mm_storeu_si128((__m128i*)(out16_bit_buffer + out_stride + 72), out3);

            in8_bit_buffer += in8_stride << 1;
            inn_bit_buffer += inn_stride << 1;
            out16_bit_buffer += out_stride << 1;
        }
    } else {
        uint32_t in_n_stride_diff = 2 * inn_stride;
        uint32_t in_8_stride_diff = 2 * in8_stride;
        uint32_t out_stride_diff  = 2 * out_stride;
        in_n_stride_diff -= width;
        in_8_stride_diff -= width;
        out_stride_diff -= width;

        if (!(width & 7)) {
            for (x = 0; x < height; x += 2) {
                for (y = 0; y < width; y += 8) {
                    out0 = _mm_srli_epi16(_mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)inn_bit_buffer),
                                                            _mm_loadl_epi64((__m128i*)in8_bit_buffer)),
                                          6);
                    out1 = _mm_srli_epi16(_mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(inn_bit_buffer + inn_stride)),
                                                            _mm_loadl_epi64((__m128i*)(in8_bit_buffer + in8_stride))),
                                          6);

                    _mm_storeu_si128((__m128i*)out16_bit_buffer, out0);
                    _mm_storeu_si128((__m128i*)(out16_bit_buffer + out_stride), out1);

                    in8_bit_buffer += 8;
                    inn_bit_buffer += 8;
                    out16_bit_buffer += 8;
                }
                in8_bit_buffer += in_8_stride_diff;
                inn_bit_buffer += in_n_stride_diff;
                out16_bit_buffer += out_stride_diff;
            }
        } else {
            for (x = 0; x < height; x += 2) {
                for (y = 0; y < width; y += 4) {
                    out0 = _mm_srli_epi16(_mm_unpacklo_epi8(_mm_cvtsi32_si128(*(uint32_t*)inn_bit_buffer),
                                                            _mm_cvtsi32_si128(*(uint32_t*)in8_bit_buffer)),
                                          6);
                    out1 = _mm_srli_epi16(
                        _mm_unpacklo_epi8(_mm_cvtsi32_si128(*(uint32_t*)(inn_bit_buffer + inn_stride)),
                                          _mm_cvtsi32_si128(*(uint32_t*)(in8_bit_buffer + in8_stride))),
                        6);

                    _mm_storel_epi64((__m128i*)out16_bit_buffer, out0);
                    _mm_storel_epi64((__m128i*)(out16_bit_buffer + out_stride), out1);

                    in8_bit_buffer += 4;
                    inn_bit_buffer += 4;
                    out16_bit_buffer += 4;
                }
                in8_bit_buffer += in_8_stride_diff;
                inn_bit_buffer += in_n_stride_diff;
                out16_bit_buffer += out_stride_diff;
            }
        }
    }
}

void svt_full_distortion_kernel32_bits_avx2(int32_t* coeff, uint32_t coeff_stride, int32_t* recon_coeff,
                                            uint32_t recon_coeff_stride, uint64_t distortion_result[DIST_CALC_TOTAL],
                                            uint32_t area_width, uint32_t area_height) {
    uint32_t row_count;
    __m256i  sum1 = _mm256_setzero_si256();
    __m256i  sum2 = _mm256_setzero_si256();
    __m128i  temp1, temp2, temp3;

    row_count = area_height;
    do {
        int32_t* coeff_temp       = coeff;
        int32_t* recon_coeff_temp = recon_coeff;

        uint32_t col_count = area_width / 4;
        do {
            __m128i x0, y0;
            __m256i x, y, z;
            x0   = _mm_loadu_si128((__m128i*)(coeff_temp));
            y0   = _mm_loadu_si128((__m128i*)(recon_coeff_temp));
            x    = _mm256_cvtepi32_epi64(x0);
            y    = _mm256_cvtepi32_epi64(y0);
            z    = _mm256_mul_epi32(x, x);
            sum2 = _mm256_add_epi64(sum2, z);
            x    = _mm256_sub_epi64(x, y);
            x    = _mm256_mul_epi32(x, x);
            sum1 = _mm256_add_epi64(sum1, x);
            coeff_temp += 4;
            recon_coeff_temp += 4;
        } while (--col_count);

        coeff += coeff_stride;
        recon_coeff += recon_coeff_stride;
        row_count -= 1;
    } while (row_count > 0);

    temp1 = _mm256_castsi256_si128(sum1);
    temp2 = _mm256_extracti128_si256(sum1, 1);
    temp1 = _mm_add_epi64(temp1, temp2);
    temp2 = _mm_shuffle_epi32(temp1, 0x4e);
    temp3 = _mm_add_epi64(temp1, temp2);
    temp1 = _mm256_castsi256_si128(sum2);
    temp2 = _mm256_extracti128_si256(sum2, 1);
    temp1 = _mm_add_epi64(temp1, temp2);
    temp2 = _mm_shuffle_epi32(temp1, 0x4e);
    temp1 = _mm_add_epi64(temp1, temp2);
    temp1 = _mm_unpacklo_epi64(temp3, temp1);

    _mm_storeu_si128((__m128i*)distortion_result, temp1);
}

void svt_full_distortion_kernel_cbf_zero32_bits_avx2(int32_t* coeff, uint32_t coeff_stride,
                                                     uint64_t distortion_result[DIST_CALC_TOTAL], uint32_t area_width,
                                                     uint32_t area_height) {
    uint32_t row_count;
    __m256i  sum = _mm256_setzero_si256();
    __m128i  temp1, temp2;

    row_count = area_height;
    do {
        int32_t* coeff_temp = coeff;

        uint32_t col_count = area_width / 4;
        do {
            __m128i x0;
            __m256i y0, z0;
            x0 = _mm_loadu_si128((__m128i*)(coeff_temp));
            coeff_temp += 4;
            y0  = _mm256_cvtepi32_epi64(x0);
            z0  = _mm256_mul_epi32(y0, y0);
            sum = _mm256_add_epi64(sum, z0);
        } while (--col_count);

        coeff += coeff_stride;
        row_count -= 1;
    } while (row_count > 0);

    temp1 = _mm256_castsi256_si128(sum);
    temp2 = _mm256_extracti128_si256(sum, 1);
    temp1 = _mm_add_epi64(temp1, temp2);
    temp2 = _mm_shuffle_epi32(temp1, 0x4e);
    temp1 = _mm_add_epi64(temp1, temp2);
    _mm_storeu_si128((__m128i*)distortion_result, temp1);
}

static INLINE void residual32_avx2(const uint8_t* const input, const uint8_t* const pred, int16_t* const residual) {
    const __m256i zero  = _mm256_setzero_si256();
    const __m256i in0   = _mm256_loadu_si256((__m256i*)input);
    const __m256i pr0   = _mm256_loadu_si256((__m256i*)pred);
    const __m256i in1   = _mm256_permute4x64_epi64(in0, 0xD8);
    const __m256i pr1   = _mm256_permute4x64_epi64(pr0, 0xD8);
    const __m256i in_lo = _mm256_unpacklo_epi8(in1, zero);
    const __m256i in_hi = _mm256_unpackhi_epi8(in1, zero);
    const __m256i pr_lo = _mm256_unpacklo_epi8(pr1, zero);
    const __m256i pr_hi = _mm256_unpackhi_epi8(pr1, zero);
    const __m256i re_lo = _mm256_sub_epi16(in_lo, pr_lo);
    const __m256i re_hi = _mm256_sub_epi16(in_hi, pr_hi);
    _mm256_storeu_si256((__m256i*)(residual + 0 * 16), re_lo);
    _mm256_storeu_si256((__m256i*)(residual + 1 * 16), re_hi);
}

SIMD_INLINE void residual_kernel32_avx2(const uint8_t* input, const uint32_t input_stride, const uint8_t* pred,
                                        const uint32_t pred_stride, int16_t* residual, const uint32_t residual_stride,
                                        const uint32_t area_height) {
    uint32_t y = area_height;

    do {
        residual32_avx2(input, pred, residual);
        input += input_stride;
        pred += pred_stride;
        residual += residual_stride;
    } while (--y);
}

SIMD_INLINE void residual_kernel64_avx2(const uint8_t* input, const uint32_t input_stride, const uint8_t* pred,
                                        const uint32_t pred_stride, int16_t* residual, const uint32_t residual_stride,
                                        const uint32_t area_height) {
    uint32_t y = area_height;

    do {
        residual32_avx2(input + 0 * 32, pred + 0 * 32, residual + 0 * 32);
        residual32_avx2(input + 1 * 32, pred + 1 * 32, residual + 1 * 32);
        input += input_stride;
        pred += pred_stride;
        residual += residual_stride;
    } while (--y);
}

SIMD_INLINE void residual_kernel128_avx2(const uint8_t* input, const uint32_t input_stride, const uint8_t* pred,
                                         const uint32_t pred_stride, int16_t* residual, const uint32_t residual_stride,
                                         const uint32_t area_height) {
    uint32_t y = area_height;

    do {
        residual32_avx2(input + 0 * 32, pred + 0 * 32, residual + 0 * 32);
        residual32_avx2(input + 1 * 32, pred + 1 * 32, residual + 1 * 32);
        residual32_avx2(input + 2 * 32, pred + 2 * 32, residual + 2 * 32);
        residual32_avx2(input + 3 * 32, pred + 3 * 32, residual + 3 * 32);
        input += input_stride;
        pred += pred_stride;
        residual += residual_stride;
    } while (--y);
}

void svt_residual_kernel8bit_avx2(uint8_t* input, uint32_t input_stride, uint8_t* pred, uint32_t pred_stride,
                                  int16_t* residual, uint32_t residual_stride, uint32_t area_width,
                                  uint32_t area_height) {
    switch (area_width) {
    case 4:
        residual_kernel4_avx2(input, input_stride, pred, pred_stride, residual, residual_stride, area_height);
        break;

    case 8:
        residual_kernel8_avx2(input, input_stride, pred, pred_stride, residual, residual_stride, area_height);
        break;

    case 16:
        residual_kernel16_avx2(input, input_stride, pred, pred_stride, residual, residual_stride, area_height);
        break;

    case 32:
        residual_kernel32_avx2(input, input_stride, pred, pred_stride, residual, residual_stride, area_height);
        break;

    case 64:
        residual_kernel64_avx2(input, input_stride, pred, pred_stride, residual, residual_stride, area_height);
        break;

    default: // 128
        residual_kernel128_avx2(input, input_stride, pred, pred_stride, residual, residual_stride, area_height);
        break;
    }
}

uint64_t svt_spatial_full_distortion_kernel_avx2(uint8_t* input, uint32_t input_offset, uint32_t input_stride,
                                                 uint8_t* recon, int32_t recon_offset, uint32_t recon_stride,
                                                 uint32_t area_width, uint32_t area_height) {
    const uint32_t leftover = area_width & 31;
    int32_t        h;
    __m256i        sum = _mm256_setzero_si256();
    __m128i        sum_l, sum_h, s;
    input += input_offset;
    recon += recon_offset;

    if (leftover) {
        const uint8_t* inp = input + area_width - leftover;
        const uint8_t* rec = recon + area_width - leftover;

        if (leftover == 4) {
            h = area_height;
            do {
                const __m128i in0 = _mm_cvtsi32_si128(*(uint32_t*)inp);
                const __m128i in1 = _mm_cvtsi32_si128(*(uint32_t*)(inp + input_stride));
                const __m128i re0 = _mm_cvtsi32_si128(*(uint32_t*)rec);
                const __m128i re1 = _mm_cvtsi32_si128(*(uint32_t*)(rec + recon_stride));
                const __m256i in  = _mm256_setr_m128i(in0, in1);
                const __m256i re  = _mm256_setr_m128i(re0, re1);
                distortion_avx2_intrin(in, re, &sum);
                inp += 2 * input_stride;
                rec += 2 * recon_stride;
                h -= 2;
            } while (h);

            if (area_width == 4) {
                sum_l = _mm256_castsi256_si128(sum);
                sum_h = _mm256_extracti128_si256(sum, 1);
                s     = _mm_add_epi32(sum_l, sum_h);
                s     = _mm_add_epi32(s, _mm_srli_si128(s, 4));
                return _mm_cvtsi128_si32(s);
            }
        } else if (leftover == 8) {
            h = area_height;
            do {
                const __m128i in0 = _mm_loadl_epi64((__m128i*)inp);
                const __m128i in1 = _mm_loadl_epi64((__m128i*)(inp + input_stride));
                const __m128i re0 = _mm_loadl_epi64((__m128i*)rec);
                const __m128i re1 = _mm_loadl_epi64((__m128i*)(rec + recon_stride));
                const __m256i in  = _mm256_setr_m128i(in0, in1);
                const __m256i re  = _mm256_setr_m128i(re0, re1);
                distortion_avx2_intrin(in, re, &sum);
                inp += 2 * input_stride;
                rec += 2 * recon_stride;
                h -= 2;
            } while (h);
        } else if (leftover <= 16) {
            h = area_height;
            do {
                spatial_full_distortion_kernel16_avx2_intrin(inp, rec, &sum);
                inp += input_stride;
                rec += recon_stride;
            } while (--h);

            if (leftover == 12) {
                const __m256i mask = _mm256_setr_epi32(-1, -1, -1, -1, -1, -1, 0, 0);
                sum                = _mm256_and_si256(sum, mask);
            }
        } else {
            __m256i sum1 = _mm256_setzero_si256();
            h            = area_height;
            do {
                spatial_full_distortion_kernel32_leftover_avx2_intrin(inp, rec, &sum, &sum1);
                inp += input_stride;
                rec += recon_stride;
            } while (--h);

            __m256i mask[2];
            if (leftover == 20) {
                mask[0] = _mm256_setr_epi32(-1, -1, -1, -1, -1, -1, 0, 0);
                mask[1] = _mm256_setr_epi32(-1, -1, -1, -1, 0, 0, 0, 0);
            } else if (leftover == 24) {
                mask[0] = _mm256_setr_epi32(-1, -1, -1, -1, -1, -1, -1, -1);
                mask[1] = _mm256_setr_epi32(-1, -1, -1, -1, 0, 0, 0, 0);
            } else { // leftover = 28
                mask[0] = _mm256_setr_epi32(-1, -1, -1, -1, -1, -1, -1, -1);
                mask[1] = _mm256_setr_epi32(-1, -1, -1, -1, -1, -1, 0, 0);
            }

            sum  = _mm256_and_si256(sum, mask[0]);
            sum1 = _mm256_and_si256(sum1, mask[1]);
            sum  = _mm256_add_epi32(sum, sum1);
        }
    }

    area_width -= leftover;

    if (area_width) {
        const uint8_t* inp = input;
        const uint8_t* rec = recon;
        h                  = area_height;

        if (area_width == 32) {
            do {
                spatial_full_distortion_kernel32_avx2_intrin(inp, rec, &sum);
                inp += input_stride;
                rec += recon_stride;
            } while (--h);
        } else if (area_width == 64) {
            do {
                spatial_full_distortion_kernel32_avx2_intrin(inp + 0 * 32, rec + 0 * 32, &sum);
                spatial_full_distortion_kernel32_avx2_intrin(inp + 1 * 32, rec + 1 * 32, &sum);
                inp += input_stride;
                rec += recon_stride;
            } while (--h);
        } else if (area_width == 96) {
            do {
                spatial_full_distortion_kernel32_avx2_intrin(inp + 0 * 32, rec + 0 * 32, &sum);
                spatial_full_distortion_kernel32_avx2_intrin(inp + 1 * 32, rec + 1 * 32, &sum);
                spatial_full_distortion_kernel32_avx2_intrin(inp + 2 * 32, rec + 2 * 32, &sum);
                inp += input_stride;
                rec += recon_stride;
            } while (--h);
        } else if (area_width == 128) {
            do {
                spatial_full_distortion_kernel32_avx2_intrin(inp + 0 * 32, rec + 0 * 32, &sum);
                spatial_full_distortion_kernel32_avx2_intrin(inp + 1 * 32, rec + 1 * 32, &sum);
                spatial_full_distortion_kernel32_avx2_intrin(inp + 2 * 32, rec + 2 * 32, &sum);
                spatial_full_distortion_kernel32_avx2_intrin(inp + 3 * 32, rec + 3 * 32, &sum);
                inp += input_stride;
                rec += recon_stride;
            } while (--h);
        } else {
            __m256i sum64 = _mm256_setzero_si256();
            do {
                for (uint32_t w = 0; w < area_width; w += 32) {
                    spatial_full_distortion_kernel32_avx2_intrin(inp + w, rec + w, &sum);
                }
                inp += input_stride;
                rec += recon_stride;

                sum32_to64(&sum, &sum64);
            } while (--h);
            s = _mm_add_epi64(_mm256_castsi256_si128(sum64), _mm256_extracti128_si256(sum64, 1));
            return _mm_extract_epi64(s, 0) + _mm_extract_epi64(s, 1);
        }
    }

    return hadd32_avx2_intrin(sum);
}

/************************************************
 * Support for params *input and *recon up to 15bit values
 * This assumption allow to use faster _mm256_madd_epi16() instruction
 ************************************************/
uint64_t svt_full_distortion_kernel16_bits_avx2(uint8_t* input, uint32_t input_offset, uint32_t input_stride,
                                                uint8_t* recon, int32_t recon_offset, uint32_t recon_stride,
                                                uint32_t area_width, uint32_t area_height) {
    const uint32_t leftover = area_width & 15;
    __m256i        sum32    = _mm256_setzero_si256();
    __m256i        sum64    = _mm256_setzero_si256();
    __m256i        in, re;
    uint16_t*      input_16bit = (uint16_t*)input;
    uint16_t*      recon_16bit = (uint16_t*)recon;
    input_16bit += input_offset;
    recon_16bit += recon_offset;

    if (leftover) {
        const uint16_t* inp = input_16bit + area_width - leftover;
        const uint16_t* rec = recon_16bit + area_width - leftover;
        uint32_t        h   = area_height;

        if (leftover == 4) {
            do {
                full_distortion_kernel4_avx2_intrin(inp, rec, &sum32);
                inp += input_stride;
                rec += recon_stride;
                sum32_to64(&sum32, &sum64);
            } while (--h);
        } else if (leftover == 8) {
            do {
                in = _mm256_set_m128i(_mm_loadu_si128((__m128i*)inp), _mm_loadu_si128((__m128i*)(inp + input_stride)));
                re = _mm256_set_m128i(_mm_loadu_si128((__m128i*)rec), _mm_loadu_si128((__m128i*)(rec + recon_stride)));
                full_distortion_kernel16_avx2_intrin(in, re, &sum32);
                inp += 2 * input_stride;
                rec += 2 * recon_stride;
                sum32_to64(&sum32, &sum64);
            } while (h -= 2);
        } else { //leftover == 12
            do {
                in = _mm256_set_m128i(_mm_loadu_si128((__m128i*)inp), _mm_loadl_epi64((__m128i*)(inp + 8)));
                re = _mm256_set_m128i(_mm_loadu_si128((__m128i*)rec), _mm_loadl_epi64((__m128i*)(rec + 8)));
                full_distortion_kernel16_avx2_intrin(in, re, &sum32);
                inp += input_stride;
                rec += recon_stride;
                sum32_to64(&sum32, &sum64);
            } while (--h);
        }
    }

    area_width -= leftover;

    if (area_width) {
        const uint16_t* inp = input_16bit;
        const uint16_t* rec = recon_16bit;

        if (area_width == 16) {
            for (uint32_t h = 0; h < area_height; h += 2) {
                full_distortion_kernel16_avx2_intrin(
                    _mm256_loadu_si256((__m256i*)inp), _mm256_loadu_si256((__m256i*)rec), &sum32);
                full_distortion_kernel16_avx2_intrin(_mm256_loadu_si256((__m256i*)(inp + input_stride)),
                                                     _mm256_loadu_si256((__m256i*)(rec + recon_stride)),
                                                     &sum32);
                inp += 2 * input_stride;
                rec += 2 * recon_stride;
                sum32_to64(&sum32, &sum64);
            }
        } else if (area_width == 32) {
            for (uint32_t h = 0; h < area_height; h++) {
                full_distortion_kernel16_avx2_intrin(
                    _mm256_loadu_si256((__m256i*)inp), _mm256_loadu_si256((__m256i*)rec), &sum32);
                full_distortion_kernel16_avx2_intrin(
                    _mm256_loadu_si256((__m256i*)(inp + 16)), _mm256_loadu_si256((__m256i*)(rec + 16)), &sum32);
                inp += input_stride;
                rec += recon_stride;
                sum32_to64(&sum32, &sum64);
            }
        } else {
            for (uint32_t h = 0; h < area_height; h++) {
                for (uint32_t w = 0; w < area_width; w += 16) {
                    full_distortion_kernel16_avx2_intrin(
                        _mm256_loadu_si256((__m256i*)(inp + w)), _mm256_loadu_si256((__m256i*)(rec + w)), &sum32);
                    sum32_to64(&sum32, &sum64);
                }
                inp += input_stride;
                rec += recon_stride;
            }
        }
    }

    __m128i s = _mm_add_epi64(_mm256_castsi256_si128(sum64), _mm256_extracti128_si256(sum64, 1));
    return _mm_extract_epi64(s, 0) + _mm_extract_epi64(s, 1);
}

void svt_convert_8bit_to_16bit_avx2(uint8_t* src, uint32_t src_stride, uint16_t* dst, uint32_t dst_stride,
                                    uint32_t width, uint32_t height) {
    __m128i tmp128, tmp128_2;
    __m256i tmp1, tmp2, tmp3, tmp4, tmp5, tmp6;

    uint8_t*  _src = src;
    uint16_t* _dst = dst;
    int32_t   k;

    switch (width) {
    case 2:
        for (uint32_t j = 0; j < height; j++) {
            dst[j * dst_stride]     = src[j * src_stride];
            dst[1 + j * dst_stride] = src[1 + j * src_stride];
        }
        break;
    case 4:
        for (uint32_t j = 0; j < height; j++) {
            dst[j * dst_stride]     = src[j * src_stride];
            dst[1 + j * dst_stride] = src[1 + j * src_stride];
            dst[2 + j * dst_stride] = src[2 + j * src_stride];
            dst[3 + j * dst_stride] = src[3 + j * src_stride];
        }
        break;
    case 8:
        for (uint32_t j = 0; j < height; j++) {
            tmp128   = _mm_loadl_epi64((__m128i*)_src);
            tmp128_2 = _mm_cvtepu8_epi16(tmp128);
            _mm_storeu_si128((__m128i*)_dst, tmp128_2);
            _src += src_stride;
            _dst += dst_stride;
        }
        break;
    case 16:
        for (uint32_t j = 0; j < height; j++) {
            tmp128 = _mm_loadu_si128((__m128i*)_src);
            tmp1   = _mm256_cvtepu8_epi16(tmp128);
            _mm256_storeu_si256((__m256i*)_dst, tmp1);
            _src += src_stride;
            _dst += dst_stride;
        }
        break;
    case 32:
        for (uint32_t j = 0; j < height; j++) {
            tmp1 = _mm256_loadu_si256((__m256i*)_src);
            tmp2 = _mm256_cvtepu8_epi16(_mm256_castsi256_si128(tmp1));
            tmp3 = _mm256_cvtepu8_epi16(_mm256_extracti128_si256(tmp1, 1));
            _mm256_storeu_si256((__m256i*)_dst, tmp2);
            _mm256_storeu_si256((__m256i*)(_dst + 16), tmp3);
            _src += src_stride;
            _dst += dst_stride;
        }
        break;
    case 64:
        for (uint32_t j = 0; j < height; j++) {
            tmp1 = _mm256_loadu_si256((__m256i*)_src);
            tmp4 = _mm256_loadu_si256((__m256i*)(_src + 32));
            tmp2 = _mm256_cvtepu8_epi16(_mm256_castsi256_si128(tmp1));
            tmp3 = _mm256_cvtepu8_epi16(_mm256_extracti128_si256(tmp1, 1));
            tmp5 = _mm256_cvtepu8_epi16(_mm256_castsi256_si128(tmp4));
            tmp6 = _mm256_cvtepu8_epi16(_mm256_extracti128_si256(tmp4, 1));
            _mm256_storeu_si256((__m256i*)_dst, tmp2);
            _mm256_storeu_si256((__m256i*)(_dst + 16), tmp3);
            _mm256_storeu_si256((__m256i*)(_dst + 32), tmp5);
            _mm256_storeu_si256((__m256i*)(_dst + 48), tmp6);
            _src += src_stride;
            _dst += dst_stride;
        }
        break;
    case 128:
        for (uint32_t j = 0; j < height; j++) {
            tmp1 = _mm256_loadu_si256((__m256i*)_src);
            tmp4 = _mm256_loadu_si256((__m256i*)(_src + 32));
            tmp2 = _mm256_cvtepu8_epi16(_mm256_castsi256_si128(tmp1));
            tmp3 = _mm256_cvtepu8_epi16(_mm256_extracti128_si256(tmp1, 1));
            tmp5 = _mm256_cvtepu8_epi16(_mm256_castsi256_si128(tmp4));
            tmp6 = _mm256_cvtepu8_epi16(_mm256_extracti128_si256(tmp4, 1));
            _mm256_storeu_si256((__m256i*)_dst, tmp2);
            _mm256_storeu_si256((__m256i*)(_dst + 16), tmp3);
            _mm256_storeu_si256((__m256i*)(_dst + 32), tmp5);
            _mm256_storeu_si256((__m256i*)(_dst + 48), tmp6);
            tmp1 = _mm256_loadu_si256((__m256i*)(_src + 64));
            tmp4 = _mm256_loadu_si256((__m256i*)(_src + 96));
            tmp2 = _mm256_cvtepu8_epi16(_mm256_castsi256_si128(tmp1));
            tmp3 = _mm256_cvtepu8_epi16(_mm256_extracti128_si256(tmp1, 1));
            tmp5 = _mm256_cvtepu8_epi16(_mm256_castsi256_si128(tmp4));
            tmp6 = _mm256_cvtepu8_epi16(_mm256_extracti128_si256(tmp4, 1));
            _mm256_storeu_si256((__m256i*)(_dst + 64), tmp2);
            _mm256_storeu_si256((__m256i*)(_dst + 80), tmp3);
            _mm256_storeu_si256((__m256i*)(_dst + 96), tmp5);
            _mm256_storeu_si256((__m256i*)(_dst + 112), tmp6);
            _src += src_stride;
            _dst += dst_stride;
        }
        break;
    default:
        for (uint32_t j = 0; j < height; j++) {
            for (k = 0; k <= (int32_t)width - 32; k += 32) {
                tmp1 = _mm256_loadu_si256((__m256i*)(_src + k));
                tmp2 = _mm256_cvtepu8_epi16(_mm256_castsi256_si128(tmp1));
                tmp3 = _mm256_cvtepu8_epi16(_mm256_extracti128_si256(tmp1, 1));
                _mm256_storeu_si256((__m256i*)(_dst + k), tmp2);
                _mm256_storeu_si256((__m256i*)(_dst + k + 16), tmp3);
            }
            for (; k < (int32_t)width; k++) {
                _dst[k] = (_src[k]);
            }
            _dst += dst_stride;
            _src += src_stride;
        }
        break;
    }
}

//Function is created with assumption that src buffer store values in range [0..255]
void svt_convert_16bit_to_8bit_avx2(uint16_t* src, uint32_t src_stride, uint8_t* dst, uint32_t dst_stride,
                                    uint32_t width, uint32_t height) {
    int32_t   k;
    __m256i   tmp1, tmp2, tmp3;
    uint8_t*  _dst = dst;
    uint16_t* _src = src;

    for (uint32_t j = 0; j < height; j++) {
        for (k = 0; k <= (int32_t)width - 32; k += 32) {
            tmp1 = _mm256_loadu_si256((__m256i*)(_src + k));
            tmp2 = _mm256_loadu_si256((__m256i*)(_src + k + 16));
            tmp3 = _mm256_packus_epi16(tmp1, tmp2);
            tmp3 = _mm256_permute4x64_epi64(tmp3, 0xd8);
            _mm256_storeu_si256((__m256i*)(_dst + k), tmp3);
        }
        for (; k < (int32_t)width; k++) {
            _dst[k] = (uint8_t)(_src[k]);
        }
        _dst += dst_stride;
        _src += src_stride;
    }
}

void svt_residual_kernel16bit_avx2(uint16_t* input, uint32_t input_stride, uint16_t* pred, uint32_t pred_stride,
                                   int16_t* residual, uint32_t residual_stride, uint32_t area_width,
                                   uint32_t area_height) {
    __m128i residual0_sse, residual1_sse;
    __m256i residual0, residual1, residual2, residual3;
    switch (area_width) {
    case 4:
        for (uint32_t y = 0; y < area_height; y += 2) {
            residual0_sse = _mm_sub_epi16(_mm_loadl_epi64((__m128i*)input), _mm_loadl_epi64((__m128i*)pred));
            residual1_sse = _mm_sub_epi16(_mm_loadl_epi64((__m128i*)(input + input_stride)),
                                          _mm_loadl_epi64((__m128i*)(pred + pred_stride)));

            _mm_storel_epi64((__m128i*)residual, residual0_sse);
            _mm_storel_epi64((__m128i*)(residual + residual_stride), residual1_sse);

            input += input_stride << 1;
            pred += pred_stride << 1;
            residual += residual_stride << 1;
        }
        break;
    case 8:
        for (uint32_t y = 0; y < area_height; y += 2) {
            residual0_sse = _mm_sub_epi16(_mm_loadu_si128((__m128i*)input), _mm_loadu_si128((__m128i*)pred));
            residual1_sse = _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + input_stride)),
                                          _mm_loadu_si128((__m128i*)(pred + pred_stride)));

            _mm_storeu_si128((__m128i*)residual, residual0_sse);
            _mm_storeu_si128((__m128i*)(residual + residual_stride), residual1_sse);

            input += input_stride << 1;
            pred += pred_stride << 1;
            residual += residual_stride << 1;
        }
        break;
    case 16:
        for (uint32_t y = 0; y < area_height; y += 2) {
            residual0 = _mm256_sub_epi16(_mm256_loadu_si256((__m256i*)input), _mm256_loadu_si256((__m256i*)pred));
            residual1 = _mm256_sub_epi16(_mm256_loadu_si256((__m256i*)(input + input_stride)),
                                         _mm256_loadu_si256((__m256i*)(pred + pred_stride)));
            _mm256_storeu_si256((__m256i*)residual, residual0);
            _mm256_storeu_si256((__m256i*)(residual + residual_stride), residual1);

            input += input_stride << 1;
            pred += pred_stride << 1;
            residual += residual_stride << 1;
        }
        break;
    case 32:
        for (uint32_t y = 0; y < area_height; y += 2) {
            residual0 = _mm256_sub_epi16(_mm256_loadu_si256((__m256i*)input), _mm256_loadu_si256((__m256i*)pred));
            residual1 = _mm256_sub_epi16(_mm256_loadu_si256((__m256i*)(input + 16)),
                                         _mm256_loadu_si256((__m256i*)(pred + 16)));
            residual2 = _mm256_sub_epi16(_mm256_loadu_si256((__m256i*)(input + input_stride)),
                                         _mm256_loadu_si256((__m256i*)(pred + pred_stride)));
            residual3 = _mm256_sub_epi16(_mm256_loadu_si256((__m256i*)(input + input_stride + 16)),
                                         _mm256_loadu_si256((__m256i*)(pred + pred_stride + 16)));

            _mm256_storeu_si256((__m256i*)residual, residual0);
            _mm256_storeu_si256((__m256i*)(residual + 16), residual1);
            _mm256_storeu_si256((__m256i*)(residual + residual_stride), residual2);
            _mm256_storeu_si256((__m256i*)(residual + residual_stride + 16), residual3);

            input += input_stride << 1;
            pred += pred_stride << 1;
            residual += residual_stride << 1;
        }
        break;
    case 64:
        for (uint32_t y = 0; y < area_height; y++) {
            residual0 = _mm256_sub_epi16(_mm256_loadu_si256((__m256i*)input), _mm256_loadu_si256((__m256i*)pred));
            residual1 = _mm256_sub_epi16(_mm256_loadu_si256((__m256i*)(input + 16)),
                                         _mm256_loadu_si256((__m256i*)(pred + 16)));
            residual2 = _mm256_sub_epi16(_mm256_loadu_si256((__m256i*)(input + 32)),
                                         _mm256_loadu_si256((__m256i*)(pred + 32)));
            residual3 = _mm256_sub_epi16(_mm256_loadu_si256((__m256i*)(input + 48)),
                                         _mm256_loadu_si256((__m256i*)(pred + 48)));

            _mm256_storeu_si256((__m256i*)residual, residual0);
            _mm256_storeu_si256((__m256i*)(residual + 16), residual1);
            _mm256_storeu_si256((__m256i*)(residual + 32), residual2);
            _mm256_storeu_si256((__m256i*)(residual + 48), residual3);

            input += input_stride;
            pred += pred_stride;
            residual += residual_stride;
        }
        break;
    default:
        svt_residual_kernel16bit_sse2_intrin(
            input, input_stride, pred, pred_stride, residual, residual_stride, area_width, area_height);
        break;
    }
}

static INLINE void unpack_and_2bcompress_32(uint16_t* in16b_buffer, uint8_t* out8b_buffer, uint8_t* out2b_buffer,
                                            uint32_t width_rep) {
    __m256i ymm_00ff = _mm256_set1_epi16(0x00FF);
    __m256i msk_2b   = _mm256_set1_epi16(0x0003); //0000.0000.0000.0011
    __m256i in1, in2, out8_u8;
    __m256i tmp_2b1, tmp_2b2, tmp_2b;
    __m256i ext0, ext1, ext2, ext3, ext0123, ext0123n, extp;
    __m256i msk0, msk1, msk2;

    msk0 = _mm256_set1_epi32(0x000000C0); //1100.0000
    msk1 = _mm256_set1_epi32(0x00000030); //0011.0000
    msk2 = _mm256_set1_epi32(0x0000000C); //0000.1100
    for (uint32_t w = 0; w < width_rep; w++) {
        in1 = _mm256_loadu_si256((__m256i*)(in16b_buffer + w * 32));
        in2 = _mm256_loadu_si256((__m256i*)(in16b_buffer + w * 32 + 16));

        tmp_2b1 = _mm256_and_si256(in1, msk_2b); //0000.0011.1111.1111 -> 0000.0000.0000.0011
        tmp_2b2 = _mm256_and_si256(in2, msk_2b);
        tmp_2b  = _mm256_permute4x64_epi64(_mm256_packus_epi16(tmp_2b1, tmp_2b2), 0xd8);

        ext0 = _mm256_srli_epi32(
            tmp_2b,
            3 * 8); //0000.0011.0000.0000.0000.0000.0000.0000 -> 0000.0000.0000.0000.0000.0000.0000.0011
        ext1 = _mm256_and_si256(
            _mm256_srli_epi32(tmp_2b, 1 * 8 + 6),
            msk2); //0000.0000.0000.0011.0000.0000.0000.0000 -> 0000.0000.0000.0000.0000.0000.0000.1100
        ext2 = _mm256_and_si256(
            _mm256_srli_epi32(tmp_2b, 4),
            msk1); //0000.0000.0000.0000.0000.0011.0000.0000 -> 0000.0000.0000.0000.0000.0000.0011.0000
        ext3 = _mm256_and_si256(
            _mm256_slli_epi32(tmp_2b, 6),
            msk0); //0000.0000.0000.0000.0000.0000.0000.0011 -> 0000.0000.0000.0000.0000.0000.1100.0000
        ext0123 = _mm256_or_si256(_mm256_or_si256(ext0, ext1),
                                  _mm256_or_si256(ext2, ext3)); //0000.0000.0000.0000.0000.0000.1111.1111

        ext0123n = _mm256_castsi128_si256(_mm256_extracti128_si256(ext0123, 1));

        extp = _mm256_packus_epi32(ext0123, ext0123n);
        extp = _mm256_packus_epi16(extp, extp);

        _mm_storel_epi64((__m128i*)(out2b_buffer + w * 8), _mm256_castsi256_si128(extp));

        out8_u8 = _mm256_packus_epi16(_mm256_and_si256(_mm256_srli_epi16(in1, 2), ymm_00ff),
                                      _mm256_and_si256(_mm256_srli_epi16(in2, 2), ymm_00ff));
        /*If we assume that in16b_buffer is 10bit max, then we can do:
        out8_u8 = _mm256_packus_epi16(_mm256_srli_epi16(in1, 2),
                                              _mm256_srli_epi16(in2, 2));
        */

        _mm256_storeu_si256((__m256i*)(out8b_buffer + w * 32), _mm256_permute4x64_epi64(out8_u8, 0xd8));
    }
}

static INLINE void svt_unpack_and_2bcompress_remainder(uint16_t* in16b_buffer, uint8_t* out8b_buffer,
                                                       uint8_t* out2b_buffer, uint32_t width) {
    uint32_t col;
    uint16_t in_pixel;
    uint8_t  tmp_pixel;

    uint32_t w_m4  = (width / 4) * 4;
    uint32_t w_rem = width - w_m4;

    for (col = 0; col < w_m4; col += 4) {
        uint8_t compressed_unpacked_pixel = 0;
        //+0
        in_pixel              = in16b_buffer[col + 0];
        out8b_buffer[col + 0] = (uint8_t)(in_pixel >> 2);
        tmp_pixel             = (uint8_t)(in_pixel << 6);
        compressed_unpacked_pixel |= ((tmp_pixel >> 0) & 0xC0); //1100.0000

        //+1
        in_pixel              = in16b_buffer[col + 1];
        out8b_buffer[col + 1] = (uint8_t)(in_pixel >> 2);
        tmp_pixel             = (uint8_t)(in_pixel << 6);
        compressed_unpacked_pixel |= ((tmp_pixel >> 2) & 0x30); //0011.0000

        //+2
        in_pixel              = in16b_buffer[col + 2];
        out8b_buffer[col + 2] = (uint8_t)(in_pixel >> 2);
        tmp_pixel             = (uint8_t)(in_pixel << 6);
        compressed_unpacked_pixel |= ((tmp_pixel >> 4) & 0x0C); //0000.1100

        //+3
        in_pixel              = in16b_buffer[col + 3];
        out8b_buffer[col + 3] = (uint8_t)(in_pixel >> 2);
        tmp_pixel             = (uint8_t)(in_pixel << 6);
        compressed_unpacked_pixel |= ((tmp_pixel >> 6) & 0x03); //0000.0011

        out2b_buffer[col / 4] = compressed_unpacked_pixel;
    }

    //we can have up to 3 pixels remaining
    if (w_rem > 0) {
        uint8_t compressed_unpacked_pixel = 0;
        //+0
        in_pixel              = in16b_buffer[col + 0];
        out8b_buffer[col + 0] = (uint8_t)(in_pixel >> 2);
        tmp_pixel             = (uint8_t)(in_pixel << 6);
        compressed_unpacked_pixel |= ((tmp_pixel >> 0) & 0xC0); //1100.0000

        if (w_rem > 1) {
            //+1
            in_pixel              = in16b_buffer[col + 1];
            out8b_buffer[col + 1] = (uint8_t)(in_pixel >> 2);
            tmp_pixel             = (uint8_t)(in_pixel << 6);
            compressed_unpacked_pixel |= ((tmp_pixel >> 2) & 0x30); //0011.0000
        }
        if (w_rem > 2) {
            //+2
            in_pixel              = in16b_buffer[col + 2];
            out8b_buffer[col + 2] = (uint8_t)(in_pixel >> 2);
            tmp_pixel             = (uint8_t)(in_pixel << 6);
            compressed_unpacked_pixel |= ((tmp_pixel >> 4) & 0x0C); //0000.1100
        }

        out2b_buffer[col / 4] = compressed_unpacked_pixel;
    }
}

static INLINE void transpose(__m256i out[4], __m256i in[4]) {
    const __m256i shufle_transpose_128 = _mm256_setr_epi8(
        0, 4, 8, 12, 1, 5, 9, 13, 2, 6, 10, 14, 3, 7, 11, 15, 0, 4, 8, 12, 1, 5, 9, 13, 2, 6, 10, 14, 3, 7, 11, 15);
    //in[0] = 00 01 02 03 04 05 06 07  08 09 0A 0B 0C 0D 0E 0F  10 11 12 13 14 15 16 17  18 19 1A 1B 1C 1D 1E 1F
    //in[1] = 20 21 22 23 24 25 26 27  28 29 2A 2B 2C 2D 2E 2F  30 31 32 33 34 35 36 37  38 39 3A 3B 3C 3D 3E 3F
    //in[2] = 40 41 42 43 44 45 46 47  48 49 4A 4B 4C 4D 4E 4F  50 51 52 53 54 55 56 57  58 59 5A 5B 5C 5D 5E 5F
    //in[3] = 60 61 62 63 64 65 66 67  68 69 6A 6B 6C 6D 6E 6F  70 71 72 73 74 75 76 77  78 79 7A 7B 7C 7D 7E 7F

    __m256i A = _mm256_shuffle_epi8(in[0], shufle_transpose_128);
    __m256i B = _mm256_shuffle_epi8(in[1], shufle_transpose_128);
    __m256i C = _mm256_shuffle_epi8(in[2], shufle_transpose_128);
    __m256i D = _mm256_shuffle_epi8(in[3], shufle_transpose_128);

    A = _mm256_permute4x64_epi64(A, 0xd8); //ACBD:ABCD
    B = _mm256_permute4x64_epi64(B, 0xd8); //ACBD:ABCD
    C = _mm256_permute4x64_epi64(C, 0xd8); //ACBD:ABCD
    D = _mm256_permute4x64_epi64(D, 0xd8); //ACBD:ABCD

    A = _mm256_shuffle_epi32(A, 0xd8); //ACBDEGFH:ABCDEFGH
    B = _mm256_shuffle_epi32(B, 0xd8); //ACBDEGFH:ABCDEFGH
    C = _mm256_shuffle_epi32(C, 0xd8); //ACBDEGFH:ABCDEFGH
    D = _mm256_shuffle_epi32(D, 0xd8); //ACBDEGFH:ABCDEFGH

    __m256i t0 = _mm256_unpacklo_epi64(A, B);
    __m256i t1 = _mm256_unpackhi_epi64(A, B);
    __m256i t2 = _mm256_unpacklo_epi64(C, D);
    __m256i t3 = _mm256_unpackhi_epi64(C, D);

    //out[0] = 00 04 08 0C 10 14 18 1C  20 24 28 2C 30 34 38 3C  40 44 48 4C 50 54 58 5C  60 64 68 6C 70 74 78 7C
    //out[1] = 01 05 09 0D 11 15 19 1D  21 25 29 2D 31 35 39 3D  41 45 49 4D 51 55 59 5D  61 65 69 6D 71 75 79 7D
    //out[2] = 02 06 0A 0E 12 16 1A 1E  22 26 2A 2E 32 36 3A 3E  42 46 4A 4E 52 56 5A 5E  62 66 6A 6E 72 76 7A 7E
    //out[3] = 03 07 0B 0F 13 17 1B 1F  23 27 2B 2F 33 37 3B 3F  43 47 4B 4F 53 57 5B 5F  63 67 6B 6F 73 77 7B 7F
    out[0] = yy_unpacklo_epi128(t0, t2); //[A0/2:B0/2]
    out[1] = yy_unpacklo_epi128(t1, t3); //[A0/2:B0/2]
    out[2] = yy_unpackhi_epi128(t0, t2); //[A1/2:B1/2]
    out[3] = yy_unpackhi_epi128(t1, t3); //[A1/2:B1/2]
}

static INLINE void unpack_and_2bcompress_32x4(uint16_t* in16b_buffer, uint8_t* out8b_buffer, uint8_t* out2b_buffer,
                                              uint32_t in16_stride, uint32_t out8_stride, uint32_t out2_stride) {
    __m256i ymm_00ff = _mm256_set1_epi16(0x00FF);
    __m256i msk_2b   = _mm256_set1_epi16(0x0003); //0000.0000.0000.0011
    __m256i in0, in1;
    __m256i in_buff[4];
    __m256i tmp0, tmp1;
    __m128i out0, out1;

    for (int i = 0; i < 4; i++) {
        //load 16b input
        in0 = _mm256_loadu_si256((__m256i*)(in16b_buffer + i * in16_stride));
        in1 = _mm256_loadu_si256((__m256i*)(in16b_buffer + i * in16_stride + 16));
        //extract 8 most significant bits
        tmp0 = _mm256_and_si256(_mm256_srli_epi16(in0, 2), ymm_00ff);
        tmp1 = _mm256_and_si256(_mm256_srli_epi16(in1, 2), ymm_00ff);
        //convert 16bit values to 8bit
        out0 = _mm_packus_epi16(_mm256_castsi256_si128(tmp0), _mm256_extracti128_si256(tmp0, 1));
        out1 = _mm_packus_epi16(_mm256_castsi256_si128(tmp1), _mm256_extracti128_si256(tmp1, 1));
        //store 8bit buffer
        _mm_storeu_si128((__m128i*)(out8b_buffer + i * out8_stride), out0);
        _mm_storeu_si128((__m128i*)(out8b_buffer + i * out8_stride + 16), out1);

        //extract 2 least significant bits
        in0 = _mm256_and_si256(in0, msk_2b);
        in1 = _mm256_and_si256(in1, msk_2b);

        in_buff[i] = _mm256_permute4x64_epi64(_mm256_packs_epi16(in0, in1), 0xd8);
    }

    transpose(in_buff, in_buff);

    in_buff[0] = _mm256_slli_epi16(in_buff[0], 6);
    in_buff[1] = _mm256_slli_epi16(in_buff[1], 4);
    in_buff[2] = _mm256_slli_epi16(in_buff[2], 2);

    tmp0 = _mm256_or_si256(_mm256_or_si256(in_buff[0], in_buff[1]), _mm256_or_si256(in_buff[2], in_buff[3]));

    _mm_storel_epi64((__m128i*)(out2b_buffer), _mm256_castsi256_si128(tmp0));
    _mm_storeh_epi64((__m128i*)(out2b_buffer + out2_stride), _mm256_castsi256_si128(tmp0));
    _mm_storel_epi64((__m128i*)(out2b_buffer + 2 * out2_stride), _mm256_extracti128_si256(tmp0, 1));
    _mm_storeh_epi64((__m128i*)(out2b_buffer + 3 * out2_stride), _mm256_extracti128_si256(tmp0, 1));
}

void svt_unpack_and_2bcompress_avx2(uint16_t* in16b_buffer, uint32_t in16b_stride, uint8_t* out8b_buffer,
                                    uint32_t out8b_stride, uint8_t* out2b_buffer, uint32_t out2b_stride, uint32_t width,
                                    uint32_t height) {
    uint32_t leftover_h4 = height & 3;
    uint32_t h           = 0;
    if (width == 32) {
        for (; h < height - leftover_h4; h += 4) {
            unpack_and_2bcompress_32x4(in16b_buffer + h * in16b_stride,
                                       out8b_buffer + h * out8b_stride,
                                       out2b_buffer + h * out2b_stride,
                                       in16b_stride,
                                       out8b_stride,
                                       out2b_stride);
        }
        for (; h < height; h++) {
            unpack_and_2bcompress_32(
                in16b_buffer + h * in16b_stride, out8b_buffer + h * out8b_stride, out2b_buffer + h * out2b_stride, 1);
        }
    } else if (width == 64) {
        for (; h < height - leftover_h4; h += 4) {
            unpack_and_2bcompress_32x4(in16b_buffer + h * in16b_stride,
                                       out8b_buffer + h * out8b_stride,
                                       out2b_buffer + h * out2b_stride,
                                       in16b_stride,
                                       out8b_stride,
                                       out2b_stride);
            unpack_and_2bcompress_32x4(in16b_buffer + h * in16b_stride + 32,
                                       out8b_buffer + h * out8b_stride + 32,
                                       out2b_buffer + h * out2b_stride + 8,
                                       in16b_stride,
                                       out8b_stride,
                                       out2b_stride);
        }
        for (; h < height; h++) {
            unpack_and_2bcompress_32(
                in16b_buffer + h * in16b_stride, out8b_buffer + h * out8b_stride, out2b_buffer + h * out2b_stride, 2);
        }
    } else {
        uint32_t offset_rem   = width & 0xffffffe0;
        uint32_t offset2b_rem = offset_rem >> 2;
        uint32_t remainder    = width & 0x1f;
        for (; h < height - leftover_h4; h += 4) {
            for (uint32_t w = 0; w < (width >> 5); w++) {
                unpack_and_2bcompress_32x4(in16b_buffer + h * in16b_stride + w * 32,
                                           out8b_buffer + h * out8b_stride + w * 32,
                                           out2b_buffer + h * out2b_stride + w * 8,
                                           in16b_stride,
                                           out8b_stride,
                                           out2b_stride);
            }
            if (remainder) {
                for (uint32_t hh = 0; hh < 4; hh++) {
                    svt_unpack_and_2bcompress_remainder(in16b_buffer + (h + hh) * in16b_stride + offset_rem,
                                                        out8b_buffer + (h + hh) * out8b_stride + offset_rem,
                                                        out2b_buffer + (h + hh) * out2b_stride + offset2b_rem,
                                                        remainder);
                }
            }
        }
        for (; h < height; h++) {
            unpack_and_2bcompress_32(in16b_buffer + h * in16b_stride,
                                     out8b_buffer + h * out8b_stride,
                                     out2b_buffer + h * out2b_stride,
                                     width >> 5);
            if (remainder) {
                svt_unpack_and_2bcompress_remainder(in16b_buffer + h * in16b_stride + offset_rem,
                                                    out8b_buffer + h * out8b_stride + offset_rem,
                                                    out2b_buffer + h * out2b_stride + offset2b_rem,
                                                    remainder);
            }
        }
    }
}

static INLINE void sign_extend_16bit_to_32bit_avx2(__m256i in, __m256i zero, __m256i* out_lo, __m256i* out_hi) {
    const __m256i sign_bits = _mm256_cmpgt_epi16(zero, in);
    *out_lo                 = _mm256_unpacklo_epi16(in, sign_bits);
    *out_hi                 = _mm256_unpackhi_epi16(in, sign_bits);
}

static INLINE void store_tran_low(__m256i a, int32_t* b) {
    const __m256i one  = _mm256_set1_epi16(1);
    const __m256i a_hi = _mm256_mulhi_epi16(a, one);
    const __m256i a_lo = _mm256_mullo_epi16(a, one);
    const __m256i a_1  = _mm256_unpacklo_epi16(a_lo, a_hi);
    const __m256i a_2  = _mm256_unpackhi_epi16(a_lo, a_hi);
    _mm256_storeu_si256((__m256i*)b, a_1);
    _mm256_storeu_si256((__m256i*)(b + 8), a_2);
}

static void hadamard_col8x2_avx2(__m256i* in, int iter) {
    __m256i a0 = in[0];
    __m256i a1 = in[1];
    __m256i a2 = in[2];
    __m256i a3 = in[3];
    __m256i a4 = in[4];
    __m256i a5 = in[5];
    __m256i a6 = in[6];
    __m256i a7 = in[7];

    __m256i b0 = _mm256_add_epi16(a0, a1);
    __m256i b1 = _mm256_sub_epi16(a0, a1);
    __m256i b2 = _mm256_add_epi16(a2, a3);
    __m256i b3 = _mm256_sub_epi16(a2, a3);
    __m256i b4 = _mm256_add_epi16(a4, a5);
    __m256i b5 = _mm256_sub_epi16(a4, a5);
    __m256i b6 = _mm256_add_epi16(a6, a7);
    __m256i b7 = _mm256_sub_epi16(a6, a7);

    a0 = _mm256_add_epi16(b0, b2);
    a1 = _mm256_add_epi16(b1, b3);
    a2 = _mm256_sub_epi16(b0, b2);
    a3 = _mm256_sub_epi16(b1, b3);
    a4 = _mm256_add_epi16(b4, b6);
    a5 = _mm256_add_epi16(b5, b7);
    a6 = _mm256_sub_epi16(b4, b6);
    a7 = _mm256_sub_epi16(b5, b7);

    if (iter == 0) {
        b0 = _mm256_add_epi16(a0, a4);
        b7 = _mm256_add_epi16(a1, a5);
        b3 = _mm256_add_epi16(a2, a6);
        b4 = _mm256_add_epi16(a3, a7);
        b2 = _mm256_sub_epi16(a0, a4);
        b6 = _mm256_sub_epi16(a1, a5);
        b1 = _mm256_sub_epi16(a2, a6);
        b5 = _mm256_sub_epi16(a3, a7);

        a0 = _mm256_unpacklo_epi16(b0, b1);
        a1 = _mm256_unpacklo_epi16(b2, b3);
        a2 = _mm256_unpackhi_epi16(b0, b1);
        a3 = _mm256_unpackhi_epi16(b2, b3);
        a4 = _mm256_unpacklo_epi16(b4, b5);
        a5 = _mm256_unpacklo_epi16(b6, b7);
        a6 = _mm256_unpackhi_epi16(b4, b5);
        a7 = _mm256_unpackhi_epi16(b6, b7);

        b0 = _mm256_unpacklo_epi32(a0, a1);
        b1 = _mm256_unpacklo_epi32(a4, a5);
        b2 = _mm256_unpackhi_epi32(a0, a1);
        b3 = _mm256_unpackhi_epi32(a4, a5);
        b4 = _mm256_unpacklo_epi32(a2, a3);
        b5 = _mm256_unpacklo_epi32(a6, a7);
        b6 = _mm256_unpackhi_epi32(a2, a3);
        b7 = _mm256_unpackhi_epi32(a6, a7);

        in[0] = _mm256_unpacklo_epi64(b0, b1);
        in[1] = _mm256_unpackhi_epi64(b0, b1);
        in[2] = _mm256_unpacklo_epi64(b2, b3);
        in[3] = _mm256_unpackhi_epi64(b2, b3);
        in[4] = _mm256_unpacklo_epi64(b4, b5);
        in[5] = _mm256_unpackhi_epi64(b4, b5);
        in[6] = _mm256_unpacklo_epi64(b6, b7);
        in[7] = _mm256_unpackhi_epi64(b6, b7);
    } else {
        in[0] = _mm256_add_epi16(a0, a4);
        in[7] = _mm256_add_epi16(a1, a5);
        in[3] = _mm256_add_epi16(a2, a6);
        in[4] = _mm256_add_epi16(a3, a7);
        in[2] = _mm256_sub_epi16(a0, a4);
        in[6] = _mm256_sub_epi16(a1, a5);
        in[1] = _mm256_sub_epi16(a2, a6);
        in[5] = _mm256_sub_epi16(a3, a7);
    }
}

static void hadamard_8x8x2_avx2(const int16_t* src_diff, ptrdiff_t src_stride, int16_t* coeff) {
    __m256i src[8];
    src[0] = _mm256_loadu_si256((const __m256i*)src_diff);
    src[1] = _mm256_loadu_si256((const __m256i*)(src_diff += src_stride));
    src[2] = _mm256_loadu_si256((const __m256i*)(src_diff += src_stride));
    src[3] = _mm256_loadu_si256((const __m256i*)(src_diff += src_stride));
    src[4] = _mm256_loadu_si256((const __m256i*)(src_diff += src_stride));
    src[5] = _mm256_loadu_si256((const __m256i*)(src_diff += src_stride));
    src[6] = _mm256_loadu_si256((const __m256i*)(src_diff += src_stride));
    src[7] = _mm256_loadu_si256((const __m256i*)(src_diff += src_stride));

    hadamard_col8x2_avx2(src, 0);
    hadamard_col8x2_avx2(src, 1);

    _mm256_storeu_si256((__m256i*)coeff, yy_unpacklo_epi128(src[0], src[1]));
    coeff += 16;
    _mm256_storeu_si256((__m256i*)coeff, yy_unpacklo_epi128(src[2], src[3]));
    coeff += 16;
    _mm256_storeu_si256((__m256i*)coeff, yy_unpacklo_epi128(src[4], src[5]));
    coeff += 16;
    _mm256_storeu_si256((__m256i*)coeff, yy_unpacklo_epi128(src[6], src[7]));
    coeff += 16;
    _mm256_storeu_si256((__m256i*)coeff, yy_unpackhi_epi128(src[0], src[1]));
    coeff += 16;
    _mm256_storeu_si256((__m256i*)coeff, yy_unpackhi_epi128(src[2], src[3]));
    coeff += 16;
    _mm256_storeu_si256((__m256i*)coeff, yy_unpackhi_epi128(src[4], src[5]));
    coeff += 16;
    _mm256_storeu_si256((__m256i*)coeff, yy_unpackhi_epi128(src[6], src[7]));
}

static INLINE void hadamard_16x16_avx2(const int16_t* src_diff, ptrdiff_t src_stride, int32_t* coeff, int is_final) {
    DECLARE_ALIGNED(32, int16_t, temp_coeff[16 * 16]);
    int16_t* t_coeff = temp_coeff;
    int16_t* coeff16 = (int16_t*)coeff;
    int      idx;
    for (idx = 0; idx < 2; ++idx) {
        const int16_t* src_ptr = src_diff + idx * 8 * src_stride;
        hadamard_8x8x2_avx2(src_ptr, src_stride, t_coeff + (idx * 64 * 2));
    }

    for (idx = 0; idx < 64; idx += 16) {
        const __m256i coeff0 = _mm256_loadu_si256((const __m256i*)t_coeff);
        const __m256i coeff1 = _mm256_loadu_si256((const __m256i*)(t_coeff + 64));
        const __m256i coeff2 = _mm256_loadu_si256((const __m256i*)(t_coeff + 128));
        const __m256i coeff3 = _mm256_loadu_si256((const __m256i*)(t_coeff + 192));

        __m256i b0 = _mm256_add_epi16(coeff0, coeff1);
        __m256i b1 = _mm256_sub_epi16(coeff0, coeff1);
        __m256i b2 = _mm256_add_epi16(coeff2, coeff3);
        __m256i b3 = _mm256_sub_epi16(coeff2, coeff3);

        b0 = _mm256_srai_epi16(b0, 1);
        b1 = _mm256_srai_epi16(b1, 1);
        b2 = _mm256_srai_epi16(b2, 1);
        b3 = _mm256_srai_epi16(b3, 1);
        if (is_final) {
            store_tran_low(_mm256_add_epi16(b0, b2), coeff);
            store_tran_low(_mm256_add_epi16(b1, b3), coeff + 64);
            store_tran_low(_mm256_sub_epi16(b0, b2), coeff + 128);
            store_tran_low(_mm256_sub_epi16(b1, b3), coeff + 192);
            coeff += 16;
        } else {
            _mm256_storeu_si256((__m256i*)coeff16, _mm256_add_epi16(b0, b2));
            _mm256_storeu_si256((__m256i*)(coeff16 + 64), _mm256_add_epi16(b1, b3));
            _mm256_storeu_si256((__m256i*)(coeff16 + 128), _mm256_sub_epi16(b0, b2));
            _mm256_storeu_si256((__m256i*)(coeff16 + 192), _mm256_sub_epi16(b1, b3));
            coeff16 += 16;
        }
        t_coeff += 16;
    }
}

void svt_aom_hadamard_16x16_avx2(const int16_t* src_diff, ptrdiff_t src_stride, int32_t* coeff) {
    hadamard_16x16_avx2(src_diff, src_stride, coeff, 1);
}

void svt_aom_hadamard_32x32_avx2(const int16_t* src_diff, ptrdiff_t src_stride, int32_t* coeff) {
    // For high bitdepths, it is unnecessary to store_tran_low
    // (mult/unpack/store), then load_tran_low (load/pack) the same memory in the
    // next stage.  Output to an intermediate buffer first, then store_tran_low()
    // in the final stage.
    DECLARE_ALIGNED(32, int16_t, temp_coeff[32 * 32]);
    int16_t*      t_coeff = temp_coeff;
    int           idx;
    __m256i       coeff0_lo, coeff1_lo, coeff2_lo, coeff3_lo, b0_lo, b1_lo, b2_lo, b3_lo;
    __m256i       coeff0_hi, coeff1_hi, coeff2_hi, coeff3_hi, b0_hi, b1_hi, b2_hi, b3_hi;
    __m256i       b0, b1, b2, b3;
    const __m256i zero = _mm256_setzero_si256();
    for (idx = 0; idx < 4; ++idx) {
        // src_diff: 9 bit, dynamic range [-255, 255]
        const int16_t* src_ptr = src_diff + (idx >> 1) * 16 * src_stride + (idx & 0x01) * 16;
        hadamard_16x16_avx2(src_ptr, src_stride, (int32_t*)(t_coeff + idx * 256), 0);
    }

    for (idx = 0; idx < 256; idx += 16) {
        const __m256i coeff0 = _mm256_loadu_si256((const __m256i*)t_coeff);
        const __m256i coeff1 = _mm256_loadu_si256((const __m256i*)(t_coeff + 256));
        const __m256i coeff2 = _mm256_loadu_si256((const __m256i*)(t_coeff + 512));
        const __m256i coeff3 = _mm256_loadu_si256((const __m256i*)(t_coeff + 768));

        // Sign extend 16 bit to 32 bit.
        sign_extend_16bit_to_32bit_avx2(coeff0, zero, &coeff0_lo, &coeff0_hi);
        sign_extend_16bit_to_32bit_avx2(coeff1, zero, &coeff1_lo, &coeff1_hi);
        sign_extend_16bit_to_32bit_avx2(coeff2, zero, &coeff2_lo, &coeff2_hi);
        sign_extend_16bit_to_32bit_avx2(coeff3, zero, &coeff3_lo, &coeff3_hi);

        b0_lo = _mm256_add_epi32(coeff0_lo, coeff1_lo);
        b0_hi = _mm256_add_epi32(coeff0_hi, coeff1_hi);

        b1_lo = _mm256_sub_epi32(coeff0_lo, coeff1_lo);
        b1_hi = _mm256_sub_epi32(coeff0_hi, coeff1_hi);

        b2_lo = _mm256_add_epi32(coeff2_lo, coeff3_lo);
        b2_hi = _mm256_add_epi32(coeff2_hi, coeff3_hi);

        b3_lo = _mm256_sub_epi32(coeff2_lo, coeff3_lo);
        b3_hi = _mm256_sub_epi32(coeff2_hi, coeff3_hi);

        b0_lo = _mm256_srai_epi32(b0_lo, 2);
        b1_lo = _mm256_srai_epi32(b1_lo, 2);
        b2_lo = _mm256_srai_epi32(b2_lo, 2);
        b3_lo = _mm256_srai_epi32(b3_lo, 2);

        b0_hi = _mm256_srai_epi32(b0_hi, 2);
        b1_hi = _mm256_srai_epi32(b1_hi, 2);
        b2_hi = _mm256_srai_epi32(b2_hi, 2);
        b3_hi = _mm256_srai_epi32(b3_hi, 2);

        b0 = _mm256_packs_epi32(b0_lo, b0_hi);
        b1 = _mm256_packs_epi32(b1_lo, b1_hi);
        b2 = _mm256_packs_epi32(b2_lo, b2_hi);
        b3 = _mm256_packs_epi32(b3_lo, b3_hi);

        store_tran_low(_mm256_add_epi16(b0, b2), coeff);
        store_tran_low(_mm256_add_epi16(b1, b3), coeff + 256);
        store_tran_low(_mm256_sub_epi16(b0, b2), coeff + 512);
        store_tran_low(_mm256_sub_epi16(b1, b3), coeff + 768);

        coeff += 16;
        t_coeff += 16;
    }
}
