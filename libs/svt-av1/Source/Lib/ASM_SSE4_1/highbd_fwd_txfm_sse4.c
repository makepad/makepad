/*
* Copyright(c) 2019 Intel Corporation
* Copyright (c) 2016, Alliance for Open Media. All rights reserved
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#include <assert.h>
#include <smmintrin.h> /* SSE4.1 */

#include "definitions.h"
#include "aom_dsp_rtcd.h"
#include <emmintrin.h>
#include "transforms.h"
#include "av1_txfm1d_sse4.h"

static INLINE void load_buffer_4x4(const int16_t* input, __m128i* in, int32_t stride, int32_t flipud, int32_t fliplr,
                                   int32_t shift) {
    if (!flipud) {
        in[0] = _mm_loadl_epi64((const __m128i*)(input + 0 * stride));
        in[1] = _mm_loadl_epi64((const __m128i*)(input + 1 * stride));
        in[2] = _mm_loadl_epi64((const __m128i*)(input + 2 * stride));
        in[3] = _mm_loadl_epi64((const __m128i*)(input + 3 * stride));
    } else {
        in[0] = _mm_loadl_epi64((const __m128i*)(input + 3 * stride));
        in[1] = _mm_loadl_epi64((const __m128i*)(input + 2 * stride));
        in[2] = _mm_loadl_epi64((const __m128i*)(input + 1 * stride));
        in[3] = _mm_loadl_epi64((const __m128i*)(input + 0 * stride));
    }

    if (fliplr) {
        in[0] = _mm_shufflelo_epi16(in[0], 0x1b);
        in[1] = _mm_shufflelo_epi16(in[1], 0x1b);
        in[2] = _mm_shufflelo_epi16(in[2], 0x1b);
        in[3] = _mm_shufflelo_epi16(in[3], 0x1b);
    }

    in[0] = _mm_cvtepi16_epi32(in[0]);
    in[1] = _mm_cvtepi16_epi32(in[1]);
    in[2] = _mm_cvtepi16_epi32(in[2]);
    in[3] = _mm_cvtepi16_epi32(in[3]);

    in[0] = _mm_slli_epi32(in[0], shift);
    in[1] = _mm_slli_epi32(in[1], shift);
    in[2] = _mm_slli_epi32(in[2], shift);
    in[3] = _mm_slli_epi32(in[3], shift);
}

static void fidtx4x4_sse4_1(__m128i* in, __m128i* out, int32_t bit, int32_t col_num) {
    (void)bit;
    __m128i fact   = _mm_set1_epi32(new_sqrt2);
    __m128i offset = _mm_set1_epi32(1 << (new_sqrt2_bits - 1));
    __m128i a_low;
    __m128i v[4];

    for (int32_t i = 0; i < 4; i++) {
        a_low  = _mm_mullo_epi32(in[i * col_num], fact);
        a_low  = _mm_add_epi32(a_low, offset);
        out[i] = _mm_srai_epi32(a_low, new_sqrt2_bits);
    }

    // Transpose for 4x4
    v[0] = _mm_unpacklo_epi32(out[0], out[1]);
    v[1] = _mm_unpackhi_epi32(out[0], out[1]);
    v[2] = _mm_unpacklo_epi32(out[2], out[3]);
    v[3] = _mm_unpackhi_epi32(out[2], out[3]);

    out[0] = _mm_unpacklo_epi64(v[0], v[2]);
    out[1] = _mm_unpackhi_epi64(v[0], v[2]);
    out[2] = _mm_unpacklo_epi64(v[1], v[3]);
    out[3] = _mm_unpackhi_epi64(v[1], v[3]);
}

// We only use stage-2 bit;
// shift[0] is used in load_buffer_4x4()
// shift[1] is used in txfm_func_col()
// shift[2] is used in txfm_func_row()
static void fdct4x4_sse4_1(__m128i* in, __m128i* out, int32_t bit, const int32_t num_col) {
    const int32_t* cospi   = cospi_arr(bit);
    const __m128i  cospi32 = _mm_set1_epi32(cospi[32]);
    const __m128i  cospi48 = _mm_set1_epi32(cospi[48]);
    const __m128i  cospi16 = _mm_set1_epi32(cospi[16]);
    const __m128i  rnding  = _mm_set1_epi32(1 << (bit - 1));
    __m128i        s0, s1, s2, s3;
    __m128i        u0, u1, u2, u3;
    __m128i        v0, v1, v2, v3;

    int32_t endidx = 3 * num_col;
    s0             = _mm_add_epi32(in[0], in[endidx]);
    s3             = _mm_sub_epi32(in[0], in[endidx]);
    endidx -= num_col;
    s1 = _mm_add_epi32(in[num_col], in[endidx]);
    s2 = _mm_sub_epi32(in[num_col], in[endidx]);

    // btf_32_sse4_1_type0(cospi32, cospi32, s[01], u[02], bit);
    u0 = _mm_mullo_epi32(s0, cospi32);
    u1 = _mm_mullo_epi32(s1, cospi32);
    u2 = _mm_add_epi32(u0, u1);
    v0 = _mm_sub_epi32(u0, u1);

    u3 = _mm_add_epi32(u2, rnding);
    v1 = _mm_add_epi32(v0, rnding);

    u0 = _mm_srai_epi32(u3, bit);
    u2 = _mm_srai_epi32(v1, bit);

    // btf_32_sse4_1_type1(cospi48, cospi16, s[23], u[13], bit);
    v0 = _mm_mullo_epi32(s2, cospi48);
    v1 = _mm_mullo_epi32(s3, cospi16);
    v2 = _mm_add_epi32(v0, v1);

    v3 = _mm_add_epi32(v2, rnding);
    u1 = _mm_srai_epi32(v3, bit);

    v0 = _mm_mullo_epi32(s2, cospi16);
    v1 = _mm_mullo_epi32(s3, cospi48);
    v2 = _mm_sub_epi32(v1, v0);

    v3 = _mm_add_epi32(v2, rnding);
    u3 = _mm_srai_epi32(v3, bit);

    // Note: shift[1] and shift[2] are zeros

    // Transpose 4x4 32-bit
    v0 = _mm_unpacklo_epi32(u0, u1);
    v1 = _mm_unpackhi_epi32(u0, u1);
    v2 = _mm_unpacklo_epi32(u2, u3);
    v3 = _mm_unpackhi_epi32(u2, u3);

    out[0] = _mm_unpacklo_epi64(v0, v2);
    out[1] = _mm_unpackhi_epi64(v0, v2);
    out[2] = _mm_unpacklo_epi64(v1, v3);
    out[3] = _mm_unpackhi_epi64(v1, v3);
}

static INLINE void write_buffer_4x4(__m128i* res, int32_t* output) {
    _mm_storeu_si128((__m128i*)(output + 0 * 4), res[0]);
    _mm_storeu_si128((__m128i*)(output + 1 * 4), res[1]);
    _mm_storeu_si128((__m128i*)(output + 2 * 4), res[2]);
    _mm_storeu_si128((__m128i*)(output + 3 * 4), res[3]);
}

static void fadst4x4_sse4_1(__m128i* in, __m128i* out, int32_t bit, const int32_t num_col) {
    const int32_t* sinpi  = sinpi_arr(bit);
    const __m128i  rnding = _mm_set1_epi32(1 << (bit - 1));
    const __m128i  sinpi1 = _mm_set1_epi32((int32_t)sinpi[1]);
    const __m128i  sinpi2 = _mm_set1_epi32((int32_t)sinpi[2]);
    const __m128i  sinpi3 = _mm_set1_epi32((int32_t)sinpi[3]);
    const __m128i  sinpi4 = _mm_set1_epi32((int32_t)sinpi[4]);
    __m128i        t;
    __m128i        s0, s1, s2, s3, s4, s5, s6, s7;
    __m128i        x0, x1, x2, x3;
    __m128i        u0, u1, u2, u3;
    __m128i        v0, v1, v2, v3;

    int32_t idx = 0 * num_col;
    s0          = _mm_mullo_epi32(in[idx], sinpi1);
    s1          = _mm_mullo_epi32(in[idx], sinpi4);
    t           = _mm_add_epi32(in[idx], in[idx + num_col]);
    idx += num_col;
    s2 = _mm_mullo_epi32(in[idx], sinpi2);
    s3 = _mm_mullo_epi32(in[idx], sinpi1);
    idx += num_col;
    s4 = _mm_mullo_epi32(in[idx], sinpi3);
    idx += num_col;
    s5 = _mm_mullo_epi32(in[idx], sinpi4);
    s6 = _mm_mullo_epi32(in[idx], sinpi2);
    s7 = _mm_sub_epi32(t, in[idx]);

    t  = _mm_add_epi32(s0, s2);
    x0 = _mm_add_epi32(t, s5);
    x1 = _mm_mullo_epi32(s7, sinpi3);
    t  = _mm_sub_epi32(s1, s3);
    x2 = _mm_add_epi32(t, s6);
    x3 = s4;

    s0 = _mm_add_epi32(x0, x3);
    s1 = x1;
    s2 = _mm_sub_epi32(x2, x3);
    t  = _mm_sub_epi32(x2, x0);
    s3 = _mm_add_epi32(t, x3);

    u0 = _mm_add_epi32(s0, rnding);
    u0 = _mm_srai_epi32(u0, bit);

    u1 = _mm_add_epi32(s1, rnding);
    u1 = _mm_srai_epi32(u1, bit);

    u2 = _mm_add_epi32(s2, rnding);
    u2 = _mm_srai_epi32(u2, bit);

    u3 = _mm_add_epi32(s3, rnding);
    u3 = _mm_srai_epi32(u3, bit);

    v0 = _mm_unpacklo_epi32(u0, u1);
    v1 = _mm_unpackhi_epi32(u0, u1);
    v2 = _mm_unpacklo_epi32(u2, u3);
    v3 = _mm_unpackhi_epi32(u2, u3);

    out[0] = _mm_unpacklo_epi64(v0, v2);
    out[1] = _mm_unpackhi_epi64(v0, v2);
    out[2] = _mm_unpacklo_epi64(v1, v3);
    out[3] = _mm_unpackhi_epi64(v1, v3);
}

void svt_av1_fwd_txfm2d_4x4_sse4_1(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[4];
    const int8_t* shift   = fwd_txfm_shift_ls[TX_4X4];
    const int32_t txw_idx = get_txw_idx(TX_4X4);
    const int32_t txh_idx = get_txh_idx(TX_4X4);

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fdct4x4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fdct4x4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case ADST_DCT:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fadst4x4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fdct4x4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case DCT_ADST:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fdct4x4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fadst4x4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case ADST_ADST:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fadst4x4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fadst4x4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case FLIPADST_DCT:
        load_buffer_4x4(input, in, stride, 1, 0, shift[0]);
        fadst4x4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fdct4x4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case DCT_FLIPADST:
        load_buffer_4x4(input, in, stride, 0, 1, shift[0]);
        fdct4x4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fadst4x4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_4x4(input, in, stride, 1, 1, shift[0]);
        fadst4x4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fadst4x4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case ADST_FLIPADST:
        load_buffer_4x4(input, in, stride, 0, 1, shift[0]);
        fadst4x4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fadst4x4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case FLIPADST_ADST:
        load_buffer_4x4(input, in, stride, 1, 0, shift[0]);
        fadst4x4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fadst4x4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case IDTX:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fidtx4x4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fidtx4x4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case V_DCT:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fdct4x4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fidtx4x4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case H_DCT:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fidtx4x4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        fdct4x4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case V_ADST:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fadst4x4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fidtx4x4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case H_ADST:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fidtx4x4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        fadst4x4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case V_FLIPADST:
        load_buffer_4x4(input, in, stride, 1, 0, shift[0]);
        fadst4x4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        fidtx4x4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case H_FLIPADST:
        load_buffer_4x4(input, in, stride, 0, 1, shift[0]);
        fidtx4x4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        fadst4x4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

static void fdct4x4_N2_sse4_1(__m128i* in, __m128i* out, int32_t bit, const int32_t num_col) {
    const int32_t* cospi   = cospi_arr(bit);
    const __m128i  cospi32 = _mm_set1_epi32(cospi[32]);
    const __m128i  cospi48 = _mm_set1_epi32(cospi[48]);
    const __m128i  cospi16 = _mm_set1_epi32(cospi[16]);
    const __m128i  rnding  = _mm_set1_epi32(1 << (bit - 1));
    __m128i        s0, s1, s2, s3;
    __m128i        u0, u1, u2, u3;
    __m128i        v0, v1, v2, v3;

    int32_t endidx = 3 * num_col;
    s0             = _mm_add_epi32(in[0], in[endidx]);
    s3             = _mm_sub_epi32(in[0], in[endidx]);
    endidx -= num_col;
    s1 = _mm_add_epi32(in[num_col], in[endidx]);
    s2 = _mm_sub_epi32(in[num_col], in[endidx]);

    u0 = _mm_mullo_epi32(s0, cospi32);
    u1 = _mm_mullo_epi32(s1, cospi32);
    u2 = _mm_add_epi32(u0, u1);

    u3 = _mm_add_epi32(u2, rnding);

    u0 = _mm_srai_epi32(u3, bit);

    v0 = _mm_mullo_epi32(s2, cospi48);
    v1 = _mm_mullo_epi32(s3, cospi16);
    v2 = _mm_add_epi32(v0, v1);

    v3 = _mm_add_epi32(v2, rnding);
    u1 = _mm_srai_epi32(v3, bit);

    // Transpose 4x4 32-bit
    v0 = _mm_unpacklo_epi32(u0, u1);
    v1 = _mm_unpackhi_epi32(u0, u1);
    v2 = _mm_setzero_si128();
    v3 = _mm_setzero_si128();

    out[0] = _mm_unpacklo_epi64(v0, v2);
    out[1] = _mm_unpackhi_epi64(v0, v2);
    out[2] = _mm_unpacklo_epi64(v1, v3);
    out[3] = _mm_unpackhi_epi64(v1, v3);
}

static void fadst4x4_N2_sse4_1(__m128i* in, __m128i* out, int32_t bit, const int32_t num_col) {
    const int32_t* sinpi  = sinpi_arr(bit);
    const __m128i  rnding = _mm_set1_epi32(1 << (bit - 1));
    const __m128i  sinpi1 = _mm_set1_epi32((int32_t)sinpi[1]);
    const __m128i  sinpi2 = _mm_set1_epi32((int32_t)sinpi[2]);
    const __m128i  sinpi3 = _mm_set1_epi32((int32_t)sinpi[3]);
    const __m128i  sinpi4 = _mm_set1_epi32((int32_t)sinpi[4]);

    __m128i s0, s1, s2, s3, s4, s5;
    __m128i x0, x1;
    __m128i u0, u1;
    __m128i v0, v1, v2, v3;

    int32_t idx = 0 * num_col;
    s0          = _mm_mullo_epi32(in[idx], sinpi1);
    s5          = _mm_add_epi32(in[idx], in[idx + num_col]);
    idx += num_col;
    s1 = _mm_mullo_epi32(in[idx], sinpi2);
    idx += num_col;
    s2 = _mm_mullo_epi32(in[idx], sinpi3);
    idx += num_col;
    s3 = _mm_mullo_epi32(in[idx], sinpi4);
    s4 = _mm_sub_epi32(s5, in[idx]);

    s5 = _mm_add_epi32(s0, s1);
    x0 = _mm_add_epi32(s5, s3);
    x1 = _mm_mullo_epi32(s4, sinpi3);

    s0 = _mm_add_epi32(x0, s2);

    u0 = _mm_add_epi32(s0, rnding);
    u0 = _mm_srai_epi32(u0, bit);

    u1 = _mm_add_epi32(x1, rnding);
    u1 = _mm_srai_epi32(u1, bit);

    v0 = _mm_unpacklo_epi32(u0, u1);
    v1 = _mm_unpackhi_epi32(u0, u1);
    v2 = _mm_setzero_si128();
    v3 = _mm_setzero_si128();

    out[0] = _mm_unpacklo_epi64(v0, v2);
    out[1] = _mm_unpackhi_epi64(v0, v2);
    out[2] = _mm_unpacklo_epi64(v1, v3);
    out[3] = _mm_unpackhi_epi64(v1, v3);
}

static void fidtx4x4_N2_sse4_1(__m128i* in, __m128i* out, int32_t bit, int32_t col_num) {
    (void)bit;
    __m128i fact   = _mm_set1_epi32(new_sqrt2);
    __m128i offset = _mm_set1_epi32(1 << (new_sqrt2_bits - 1));
    __m128i a_low;
    __m128i v[4];

    for (int32_t i = 0; i < 2; i++) {
        a_low  = _mm_mullo_epi32(in[i * col_num], fact);
        a_low  = _mm_add_epi32(a_low, offset);
        out[i] = _mm_srai_epi32(a_low, new_sqrt2_bits);
    }

    // Transpose for 4x4
    v[0] = _mm_unpacklo_epi32(out[0], out[1]);
    v[1] = _mm_unpackhi_epi32(out[0], out[1]);
    v[2] = _mm_setzero_si128();
    v[3] = _mm_setzero_si128();

    out[0] = _mm_unpacklo_epi64(v[0], v[2]);
    out[1] = _mm_unpackhi_epi64(v[0], v[2]);
    out[2] = _mm_unpacklo_epi64(v[1], v[3]);
    out[3] = _mm_unpackhi_epi64(v[1], v[3]);
}

void svt_av1_fwd_txfm2d_4x4_N2_sse4_1(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[4];
    const int8_t* shift   = fwd_txfm_shift_ls[TX_4X4];
    const int32_t txw_idx = get_txw_idx(TX_4X4);
    const int32_t txh_idx = get_txh_idx(TX_4X4);

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fdct4x4_N2_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fdct4x4_N2_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case ADST_DCT:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fadst4x4_N2_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fdct4x4_N2_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case DCT_ADST:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fdct4x4_N2_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fadst4x4_N2_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case ADST_ADST:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fadst4x4_N2_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fadst4x4_N2_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case FLIPADST_DCT:
        load_buffer_4x4(input, in, stride, 1, 0, shift[0]);
        fadst4x4_N2_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fdct4x4_N2_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case DCT_FLIPADST:
        load_buffer_4x4(input, in, stride, 0, 1, shift[0]);
        fdct4x4_N2_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fadst4x4_N2_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_4x4(input, in, stride, 1, 1, shift[0]);
        fadst4x4_N2_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fadst4x4_N2_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case ADST_FLIPADST:
        load_buffer_4x4(input, in, stride, 0, 1, shift[0]);
        fadst4x4_N2_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fadst4x4_N2_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case FLIPADST_ADST:
        load_buffer_4x4(input, in, stride, 1, 0, shift[0]);
        fadst4x4_N2_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fadst4x4_N2_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case IDTX:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fidtx4x4_N2_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fidtx4x4_N2_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case V_DCT:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fdct4x4_N2_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fidtx4x4_N2_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case H_DCT:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fidtx4x4_N2_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        fdct4x4_N2_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case V_ADST:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fadst4x4_N2_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fidtx4x4_N2_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case H_ADST:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fidtx4x4_N2_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        fadst4x4_N2_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case V_FLIPADST:
        load_buffer_4x4(input, in, stride, 1, 0, shift[0]);
        fadst4x4_N2_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        fidtx4x4_N2_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case H_FLIPADST:
        load_buffer_4x4(input, in, stride, 0, 1, shift[0]);
        fidtx4x4_N2_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        fadst4x4_N2_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

static void fdct4x4_N4_sse4_1(__m128i* in, __m128i* out, int32_t bit, const int32_t num_col) {
    const int32_t* cospi   = cospi_arr(bit);
    const __m128i  cospi32 = _mm_set1_epi32(cospi[32]);
    const __m128i  rnding  = _mm_set1_epi32(1 << (bit - 1));
    const __m128i  zero    = _mm_setzero_si128();
    __m128i        s0, s1;
    __m128i        u0, u1, u2, u3;
    __m128i        v0, v1;

    int32_t endidx = 3 * num_col;
    s0             = _mm_add_epi32(in[0], in[endidx]);
    endidx -= num_col;
    s1 = _mm_add_epi32(in[num_col], in[endidx]);

    u0 = _mm_mullo_epi32(s0, cospi32);
    u1 = _mm_mullo_epi32(s1, cospi32);
    u2 = _mm_add_epi32(u0, u1);

    u3 = _mm_add_epi32(u2, rnding);

    u0 = _mm_srai_epi32(u3, bit);

    // Transpose 4x4 32-bit
    v0 = _mm_unpacklo_epi32(u0, zero);
    v1 = _mm_unpackhi_epi32(u0, zero);

    out[0] = _mm_unpacklo_epi64(v0, zero);
    out[1] = _mm_unpackhi_epi64(v0, zero);
    out[2] = _mm_unpacklo_epi64(v1, zero);
    out[3] = _mm_unpackhi_epi64(v1, zero);
}

static void fadst4x4_N4_sse4_1(__m128i* in, __m128i* out, int32_t bit, const int32_t num_col) {
    const int32_t* sinpi  = sinpi_arr(bit);
    const __m128i  rnding = _mm_set1_epi32(1 << (bit - 1));
    const __m128i  sinpi1 = _mm_set1_epi32((int32_t)sinpi[1]);
    const __m128i  sinpi2 = _mm_set1_epi32((int32_t)sinpi[2]);
    const __m128i  sinpi3 = _mm_set1_epi32((int32_t)sinpi[3]);
    const __m128i  sinpi4 = _mm_set1_epi32((int32_t)sinpi[4]);
    const __m128i  zero   = _mm_setzero_si128();
    __m128i        s0, s1, s2, s3, s4;
    __m128i        v0, v1;

    int32_t idx = 0 * num_col;
    s0          = _mm_mullo_epi32(in[idx], sinpi1);

    idx += num_col;
    s1 = _mm_mullo_epi32(in[idx], sinpi2);
    idx += num_col;
    s2 = _mm_mullo_epi32(in[idx], sinpi3);
    idx += num_col;
    s3 = _mm_mullo_epi32(in[idx], sinpi4);

    s4 = _mm_add_epi32(s0, s1);
    s1 = _mm_add_epi32(s4, s3);

    s0 = _mm_add_epi32(s1, s2);

    s3 = _mm_add_epi32(s0, rnding);
    s3 = _mm_srai_epi32(s3, bit);

    v0 = _mm_unpacklo_epi32(s3, zero);
    v1 = _mm_unpackhi_epi32(s3, zero);

    out[0] = _mm_unpacklo_epi64(v0, zero);
    out[1] = _mm_unpackhi_epi64(v0, zero);
    out[2] = _mm_unpacklo_epi64(v1, zero);
    out[3] = _mm_unpackhi_epi64(v1, zero);
}

static void fidtx4x4_N4_sse4_1(__m128i* in, __m128i* out, int32_t bit, int32_t col_num) {
    (void)bit;
    (void)col_num;
    __m128i       fact   = _mm_set1_epi32(new_sqrt2);
    __m128i       offset = _mm_set1_epi32(1 << (new_sqrt2_bits - 1));
    const __m128i zero   = _mm_setzero_si128();
    __m128i       a_low;
    __m128i       v[2];

    a_low = _mm_mullo_epi32(in[0], fact);
    a_low = _mm_add_epi32(a_low, offset);
    a_low = _mm_srai_epi32(a_low, new_sqrt2_bits);

    // Transpose for 4x4
    v[0] = _mm_unpacklo_epi32(a_low, zero);
    v[1] = _mm_unpackhi_epi32(a_low, zero);

    out[0] = _mm_unpacklo_epi64(v[0], zero);
    out[1] = _mm_unpackhi_epi64(v[0], zero);
    out[2] = _mm_unpacklo_epi64(v[1], zero);
    out[3] = _mm_unpackhi_epi64(v[1], zero);
}

void svt_av1_fwd_txfm2d_4x4_N4_sse4_1(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[4];
    const int8_t* shift   = fwd_txfm_shift_ls[TX_4X4];
    const int32_t txw_idx = get_txw_idx(TX_4X4);
    const int32_t txh_idx = get_txh_idx(TX_4X4);

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fdct4x4_N4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fdct4x4_N4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case ADST_DCT:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fadst4x4_N4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fdct4x4_N4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case DCT_ADST:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fdct4x4_N4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fadst4x4_N4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case ADST_ADST:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fadst4x4_N4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fadst4x4_N4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case FLIPADST_DCT:
        load_buffer_4x4(input, in, stride, 1, 0, shift[0]);
        fadst4x4_N4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fdct4x4_N4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case DCT_FLIPADST:
        load_buffer_4x4(input, in, stride, 0, 1, shift[0]);
        fdct4x4_N4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fadst4x4_N4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_4x4(input, in, stride, 1, 1, shift[0]);
        fadst4x4_N4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fadst4x4_N4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case ADST_FLIPADST:
        load_buffer_4x4(input, in, stride, 0, 1, shift[0]);
        fadst4x4_N4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fadst4x4_N4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case FLIPADST_ADST:
        load_buffer_4x4(input, in, stride, 1, 0, shift[0]);
        fadst4x4_N4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fadst4x4_N4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case IDTX:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fidtx4x4_N4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fidtx4x4_N4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case V_DCT:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fdct4x4_N4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fidtx4x4_N4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case H_DCT:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fidtx4x4_N4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        fdct4x4_N4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case V_ADST:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fadst4x4_N4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        fidtx4x4_N4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case H_ADST:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fidtx4x4_N4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        fadst4x4_N4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case V_FLIPADST:
        load_buffer_4x4(input, in, stride, 1, 0, shift[0]);
        fadst4x4_N4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        fidtx4x4_N4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    case H_FLIPADST:
        load_buffer_4x4(input, in, stride, 0, 1, shift[0]);
        fidtx4x4_N4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        fadst4x4_N4_sse4_1(in, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_4x4(in, coeff);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_aom_transform_config(TxType tx_type, TxSize tx_size, Txfm2dFlipCfg* cfg);

typedef void (*TxfmFuncSSE2)(__m128i* input, __m128i* output, const int8_t cos_bit, const int8_t* stage_range);

typedef void (*fwd_transform_1d_sse4_1)(__m128i* in, __m128i* out, int bit, const int num_cols);

static void fdct4x8_sse4_1(__m128i* in, __m128i* out, int bit, const int col_num) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospim32 = _mm_set1_epi32(-cospi[32]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospi16  = _mm_set1_epi32(cospi[16]);
    const __m128i  cospi56  = _mm_set1_epi32(cospi[56]);
    const __m128i  cospi8   = _mm_set1_epi32(cospi[8]);
    const __m128i  cospi24  = _mm_set1_epi32(cospi[24]);
    const __m128i  cospi40  = _mm_set1_epi32(cospi[40]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    __m128i        u[8], v[8];

    int startidx = 0 * col_num;
    int endidx   = 7 * col_num;
    // Even 8 points 0, 2, ..., 14
    // stage 0
    // stage 1
    u[0] = _mm_add_epi32(in[startidx], in[endidx]);
    v[7] = _mm_sub_epi32(in[startidx], in[endidx]); // v[7]
    startidx += col_num;
    endidx -= col_num;
    u[1] = _mm_add_epi32(in[startidx], in[endidx]);
    u[6] = _mm_sub_epi32(in[startidx], in[endidx]);
    startidx += col_num;
    endidx -= col_num;
    u[2] = _mm_add_epi32(in[startidx], in[endidx]);
    u[5] = _mm_sub_epi32(in[startidx], in[endidx]);
    startidx += col_num;
    endidx -= col_num;
    u[3] = _mm_add_epi32(in[startidx], in[endidx]);
    v[4] = _mm_sub_epi32(in[startidx], in[endidx]); // v[4]

    // stage 2
    v[0] = _mm_add_epi32(u[0], u[3]);
    v[3] = _mm_sub_epi32(u[0], u[3]);
    v[1] = _mm_add_epi32(u[1], u[2]);
    v[2] = _mm_sub_epi32(u[1], u[2]);

    v[5] = _mm_mullo_epi32(u[5], cospim32);
    v[6] = _mm_mullo_epi32(u[6], cospi32);
    v[5] = _mm_add_epi32(v[5], v[6]);
    v[5] = _mm_add_epi32(v[5], rnding);
    v[5] = _mm_srai_epi32(v[5], bit);

    u[0] = _mm_mullo_epi32(u[5], cospi32);
    v[6] = _mm_mullo_epi32(u[6], cospim32);
    v[6] = _mm_sub_epi32(u[0], v[6]);
    v[6] = _mm_add_epi32(v[6], rnding);
    v[6] = _mm_srai_epi32(v[6], bit);

    // stage 3
    // type 0
    v[0] = _mm_mullo_epi32(v[0], cospi32);
    v[1] = _mm_mullo_epi32(v[1], cospi32);
    u[0] = _mm_add_epi32(v[0], v[1]);
    u[0] = _mm_add_epi32(u[0], rnding);
    u[0] = _mm_srai_epi32(u[0], bit);

    u[1] = _mm_sub_epi32(v[0], v[1]);
    u[1] = _mm_add_epi32(u[1], rnding);
    u[1] = _mm_srai_epi32(u[1], bit);

    // type 1
    v[0] = _mm_mullo_epi32(v[2], cospi48);
    v[1] = _mm_mullo_epi32(v[3], cospi16);
    u[2] = _mm_add_epi32(v[0], v[1]);
    u[2] = _mm_add_epi32(u[2], rnding);
    u[2] = _mm_srai_epi32(u[2], bit);

    v[0] = _mm_mullo_epi32(v[2], cospi16);
    v[1] = _mm_mullo_epi32(v[3], cospi48);
    u[3] = _mm_sub_epi32(v[1], v[0]);
    u[3] = _mm_add_epi32(u[3], rnding);
    u[3] = _mm_srai_epi32(u[3], bit);

    u[4] = _mm_add_epi32(v[4], v[5]);
    u[5] = _mm_sub_epi32(v[4], v[5]);
    u[6] = _mm_sub_epi32(v[7], v[6]);
    u[7] = _mm_add_epi32(v[7], v[6]);

    // stage 4
    // stage 5
    v[0]             = _mm_mullo_epi32(u[4], cospi56);
    v[1]             = _mm_mullo_epi32(u[7], cospi8);
    v[0]             = _mm_add_epi32(v[0], v[1]);
    v[0]             = _mm_add_epi32(v[0], rnding);
    out[1 * col_num] = _mm_srai_epi32(v[0], bit); // buf0[4]

    v[0]             = _mm_mullo_epi32(u[4], cospi8);
    v[1]             = _mm_mullo_epi32(u[7], cospi56);
    v[0]             = _mm_sub_epi32(v[1], v[0]);
    v[0]             = _mm_add_epi32(v[0], rnding);
    out[7 * col_num] = _mm_srai_epi32(v[0], bit); // buf0[7]

    v[0]             = _mm_mullo_epi32(u[5], cospi24);
    v[1]             = _mm_mullo_epi32(u[6], cospi40);
    v[0]             = _mm_add_epi32(v[0], v[1]);
    v[0]             = _mm_add_epi32(v[0], rnding);
    out[5 * col_num] = _mm_srai_epi32(v[0], bit); // buf0[5]

    v[0]             = _mm_mullo_epi32(u[5], cospi40);
    v[1]             = _mm_mullo_epi32(u[6], cospi24);
    v[0]             = _mm_sub_epi32(v[1], v[0]);
    v[0]             = _mm_add_epi32(v[0], rnding);
    out[3 * col_num] = _mm_srai_epi32(v[0], bit); // buf0[6]

    out[0 * col_num] = u[0]; // buf0[0]
    out[4 * col_num] = u[1]; // buf0[1]
    out[2 * col_num] = u[2]; // buf0[2]
    out[6 * col_num] = u[3]; // buf0[3]
}

static void fdct8x8_sse4_1(__m128i* in, __m128i* out, int bit, const int col_num) {
    fdct4x8_sse4_1(in, out, bit, col_num);
    fdct4x8_sse4_1(in + 1, out + 1, bit, col_num);
}

void svt_av1_fdct32_sse4_1(__m128i* input, __m128i* output, int cos_bit, const int stride) {
    __m128i        buf0[32];
    __m128i        buf1[32];
    const int32_t* cospi;

    int startidx = 0 * stride;
    int endidx   = 31 * stride;
    // stage 0
    // stage 1
    buf1[0]  = _mm_add_epi32(input[startidx], input[endidx]);
    buf1[31] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += stride;
    endidx -= stride;
    buf1[1]  = _mm_add_epi32(input[startidx], input[endidx]);
    buf1[30] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += stride;
    endidx -= stride;
    buf1[2]  = _mm_add_epi32(input[startidx], input[endidx]);
    buf1[29] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += stride;
    endidx -= stride;
    buf1[3]  = _mm_add_epi32(input[startidx], input[endidx]);
    buf1[28] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += stride;
    endidx -= stride;
    buf1[4]  = _mm_add_epi32(input[startidx], input[endidx]);
    buf1[27] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += stride;
    endidx -= stride;
    buf1[5]  = _mm_add_epi32(input[startidx], input[endidx]);
    buf1[26] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += stride;
    endidx -= stride;
    buf1[6]  = _mm_add_epi32(input[startidx], input[endidx]);
    buf1[25] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += stride;
    endidx -= stride;
    buf1[7]  = _mm_add_epi32(input[startidx], input[endidx]);
    buf1[24] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += stride;
    endidx -= stride;
    buf1[8]  = _mm_add_epi32(input[startidx], input[endidx]);
    buf1[23] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += stride;
    endidx -= stride;
    buf1[9]  = _mm_add_epi32(input[startidx], input[endidx]);
    buf1[22] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += stride;
    endidx -= stride;
    buf1[10] = _mm_add_epi32(input[startidx], input[endidx]);
    buf1[21] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += stride;
    endidx -= stride;
    buf1[11] = _mm_add_epi32(input[startidx], input[endidx]);
    buf1[20] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += stride;
    endidx -= stride;
    buf1[12] = _mm_add_epi32(input[startidx], input[endidx]);
    buf1[19] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += stride;
    endidx -= stride;
    buf1[13] = _mm_add_epi32(input[startidx], input[endidx]);
    buf1[18] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += stride;
    endidx -= stride;
    buf1[14] = _mm_add_epi32(input[startidx], input[endidx]);
    buf1[17] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += stride;
    endidx -= stride;
    buf1[15] = _mm_add_epi32(input[startidx], input[endidx]);
    buf1[16] = _mm_sub_epi32(input[startidx], input[endidx]);

    // stage 2
    cospi    = cospi_arr(cos_bit);
    buf0[0]  = _mm_add_epi32(buf1[0], buf1[15]);
    buf0[15] = _mm_sub_epi32(buf1[0], buf1[15]);
    buf0[1]  = _mm_add_epi32(buf1[1], buf1[14]);
    buf0[14] = _mm_sub_epi32(buf1[1], buf1[14]);
    buf0[2]  = _mm_add_epi32(buf1[2], buf1[13]);
    buf0[13] = _mm_sub_epi32(buf1[2], buf1[13]);
    buf0[3]  = _mm_add_epi32(buf1[3], buf1[12]);
    buf0[12] = _mm_sub_epi32(buf1[3], buf1[12]);
    buf0[4]  = _mm_add_epi32(buf1[4], buf1[11]);
    buf0[11] = _mm_sub_epi32(buf1[4], buf1[11]);
    buf0[5]  = _mm_add_epi32(buf1[5], buf1[10]);
    buf0[10] = _mm_sub_epi32(buf1[5], buf1[10]);
    buf0[6]  = _mm_add_epi32(buf1[6], buf1[9]);
    buf0[9]  = _mm_sub_epi32(buf1[6], buf1[9]);
    buf0[7]  = _mm_add_epi32(buf1[7], buf1[8]);
    buf0[8]  = _mm_sub_epi32(buf1[7], buf1[8]);
    buf0[16] = buf1[16];
    buf0[17] = buf1[17];
    buf0[18] = buf1[18];
    buf0[19] = buf1[19];
    btf_32_sse4_1_type0(-cospi[32], cospi[32], buf1[20], buf1[27], buf0[20], buf0[27], cos_bit);
    btf_32_sse4_1_type0(-cospi[32], cospi[32], buf1[21], buf1[26], buf0[21], buf0[26], cos_bit);
    btf_32_sse4_1_type0(-cospi[32], cospi[32], buf1[22], buf1[25], buf0[22], buf0[25], cos_bit);
    btf_32_sse4_1_type0(-cospi[32], cospi[32], buf1[23], buf1[24], buf0[23], buf0[24], cos_bit);
    buf0[28] = buf1[28];
    buf0[29] = buf1[29];
    buf0[30] = buf1[30];
    buf0[31] = buf1[31];

    // stage 3
    cospi   = cospi_arr(cos_bit);
    buf1[0] = _mm_add_epi32(buf0[0], buf0[7]);
    buf1[7] = _mm_sub_epi32(buf0[0], buf0[7]);
    buf1[1] = _mm_add_epi32(buf0[1], buf0[6]);
    buf1[6] = _mm_sub_epi32(buf0[1], buf0[6]);
    buf1[2] = _mm_add_epi32(buf0[2], buf0[5]);
    buf1[5] = _mm_sub_epi32(buf0[2], buf0[5]);
    buf1[3] = _mm_add_epi32(buf0[3], buf0[4]);
    buf1[4] = _mm_sub_epi32(buf0[3], buf0[4]);
    buf1[8] = buf0[8];
    buf1[9] = buf0[9];
    btf_32_sse4_1_type0(-cospi[32], cospi[32], buf0[10], buf0[13], buf1[10], buf1[13], cos_bit);
    btf_32_sse4_1_type0(-cospi[32], cospi[32], buf0[11], buf0[12], buf1[11], buf1[12], cos_bit);
    buf1[14] = buf0[14];
    buf1[15] = buf0[15];
    buf1[16] = _mm_add_epi32(buf0[16], buf0[23]);
    buf1[23] = _mm_sub_epi32(buf0[16], buf0[23]);
    buf1[17] = _mm_add_epi32(buf0[17], buf0[22]);
    buf1[22] = _mm_sub_epi32(buf0[17], buf0[22]);
    buf1[18] = _mm_add_epi32(buf0[18], buf0[21]);
    buf1[21] = _mm_sub_epi32(buf0[18], buf0[21]);
    buf1[19] = _mm_add_epi32(buf0[19], buf0[20]);
    buf1[20] = _mm_sub_epi32(buf0[19], buf0[20]);
    buf1[24] = _mm_sub_epi32(buf0[31], buf0[24]);
    buf1[31] = _mm_add_epi32(buf0[31], buf0[24]);
    buf1[25] = _mm_sub_epi32(buf0[30], buf0[25]);
    buf1[30] = _mm_add_epi32(buf0[30], buf0[25]);
    buf1[26] = _mm_sub_epi32(buf0[29], buf0[26]);
    buf1[29] = _mm_add_epi32(buf0[29], buf0[26]);
    buf1[27] = _mm_sub_epi32(buf0[28], buf0[27]);
    buf1[28] = _mm_add_epi32(buf0[28], buf0[27]);

    // stage 4
    cospi   = cospi_arr(cos_bit);
    buf0[0] = _mm_add_epi32(buf1[0], buf1[3]);
    buf0[3] = _mm_sub_epi32(buf1[0], buf1[3]);
    buf0[1] = _mm_add_epi32(buf1[1], buf1[2]);
    buf0[2] = _mm_sub_epi32(buf1[1], buf1[2]);
    buf0[4] = buf1[4];
    btf_32_sse4_1_type0(-cospi[32], cospi[32], buf1[5], buf1[6], buf0[5], buf0[6], cos_bit);
    buf0[7]  = buf1[7];
    buf0[8]  = _mm_add_epi32(buf1[8], buf1[11]);
    buf0[11] = _mm_sub_epi32(buf1[8], buf1[11]);
    buf0[9]  = _mm_add_epi32(buf1[9], buf1[10]);
    buf0[10] = _mm_sub_epi32(buf1[9], buf1[10]);
    buf0[12] = _mm_sub_epi32(buf1[15], buf1[12]);
    buf0[15] = _mm_add_epi32(buf1[15], buf1[12]);
    buf0[13] = _mm_sub_epi32(buf1[14], buf1[13]);
    buf0[14] = _mm_add_epi32(buf1[14], buf1[13]);
    buf0[16] = buf1[16];
    buf0[17] = buf1[17];
    btf_32_sse4_1_type0(-cospi[16], cospi[48], buf1[18], buf1[29], buf0[18], buf0[29], cos_bit);
    btf_32_sse4_1_type0(-cospi[16], cospi[48], buf1[19], buf1[28], buf0[19], buf0[28], cos_bit);
    btf_32_sse4_1_type0(-cospi[48], -cospi[16], buf1[20], buf1[27], buf0[20], buf0[27], cos_bit);
    btf_32_sse4_1_type0(-cospi[48], -cospi[16], buf1[21], buf1[26], buf0[21], buf0[26], cos_bit);
    buf0[22] = buf1[22];
    buf0[23] = buf1[23];
    buf0[24] = buf1[24];
    buf0[25] = buf1[25];
    buf0[30] = buf1[30];
    buf0[31] = buf1[31];

    // stage 5
    cospi = cospi_arr(cos_bit);
    btf_32_sse4_1_type0(cospi[32], cospi[32], buf0[0], buf0[1], buf1[0], buf1[1], cos_bit);
    btf_32_sse4_1_type1(cospi[48], cospi[16], buf0[2], buf0[3], buf1[2], buf1[3], cos_bit);
    buf1[4] = _mm_add_epi32(buf0[4], buf0[5]);
    buf1[5] = _mm_sub_epi32(buf0[4], buf0[5]);
    buf1[6] = _mm_sub_epi32(buf0[7], buf0[6]);
    buf1[7] = _mm_add_epi32(buf0[7], buf0[6]);
    buf1[8] = buf0[8];
    btf_32_sse4_1_type0(-cospi[16], cospi[48], buf0[9], buf0[14], buf1[9], buf1[14], cos_bit);
    btf_32_sse4_1_type0(-cospi[48], -cospi[16], buf0[10], buf0[13], buf1[10], buf1[13], cos_bit);
    buf1[11] = buf0[11];
    buf1[12] = buf0[12];
    buf1[15] = buf0[15];
    buf1[16] = _mm_add_epi32(buf0[16], buf0[19]);
    buf1[19] = _mm_sub_epi32(buf0[16], buf0[19]);
    buf1[17] = _mm_add_epi32(buf0[17], buf0[18]);
    buf1[18] = _mm_sub_epi32(buf0[17], buf0[18]);
    buf1[20] = _mm_sub_epi32(buf0[23], buf0[20]);
    buf1[23] = _mm_add_epi32(buf0[23], buf0[20]);
    buf1[21] = _mm_sub_epi32(buf0[22], buf0[21]);
    buf1[22] = _mm_add_epi32(buf0[22], buf0[21]);
    buf1[24] = _mm_add_epi32(buf0[24], buf0[27]);
    buf1[27] = _mm_sub_epi32(buf0[24], buf0[27]);
    buf1[25] = _mm_add_epi32(buf0[25], buf0[26]);
    buf1[26] = _mm_sub_epi32(buf0[25], buf0[26]);
    buf1[28] = _mm_sub_epi32(buf0[31], buf0[28]);
    buf1[31] = _mm_add_epi32(buf0[31], buf0[28]);
    buf1[29] = _mm_sub_epi32(buf0[30], buf0[29]);
    buf1[30] = _mm_add_epi32(buf0[30], buf0[29]);

    // stage 6
    cospi   = cospi_arr(cos_bit);
    buf0[0] = buf1[0];
    buf0[1] = buf1[1];
    buf0[2] = buf1[2];
    buf0[3] = buf1[3];
    btf_32_sse4_1_type1(cospi[56], cospi[8], buf1[4], buf1[7], buf0[4], buf0[7], cos_bit);
    btf_32_sse4_1_type1(cospi[24], cospi[40], buf1[5], buf1[6], buf0[5], buf0[6], cos_bit);
    buf0[8]  = _mm_add_epi32(buf1[8], buf1[9]);
    buf0[9]  = _mm_sub_epi32(buf1[8], buf1[9]);
    buf0[10] = _mm_sub_epi32(buf1[11], buf1[10]);
    buf0[11] = _mm_add_epi32(buf1[11], buf1[10]);
    buf0[12] = _mm_add_epi32(buf1[12], buf1[13]);
    buf0[13] = _mm_sub_epi32(buf1[12], buf1[13]);
    buf0[14] = _mm_sub_epi32(buf1[15], buf1[14]);
    buf0[15] = _mm_add_epi32(buf1[15], buf1[14]);
    buf0[16] = buf1[16];
    btf_32_sse4_1_type0(-cospi[8], cospi[56], buf1[17], buf1[30], buf0[17], buf0[30], cos_bit);
    btf_32_sse4_1_type0(-cospi[56], -cospi[8], buf1[18], buf1[29], buf0[18], buf0[29], cos_bit);
    buf0[19] = buf1[19];
    buf0[20] = buf1[20];
    btf_32_sse4_1_type0(-cospi[40], cospi[24], buf1[21], buf1[26], buf0[21], buf0[26], cos_bit);
    btf_32_sse4_1_type0(-cospi[24], -cospi[40], buf1[22], buf1[25], buf0[22], buf0[25], cos_bit);
    buf0[23] = buf1[23];
    buf0[24] = buf1[24];
    buf0[27] = buf1[27];
    buf0[28] = buf1[28];
    buf0[31] = buf1[31];

    // stage 7
    cospi   = cospi_arr(cos_bit);
    buf1[0] = buf0[0];
    buf1[1] = buf0[1];
    buf1[2] = buf0[2];
    buf1[3] = buf0[3];
    buf1[4] = buf0[4];
    buf1[5] = buf0[5];
    buf1[6] = buf0[6];
    buf1[7] = buf0[7];
    btf_32_sse4_1_type1(cospi[60], cospi[4], buf0[8], buf0[15], buf1[8], buf1[15], cos_bit);
    btf_32_sse4_1_type1(cospi[28], cospi[36], buf0[9], buf0[14], buf1[9], buf1[14], cos_bit);
    btf_32_sse4_1_type1(cospi[44], cospi[20], buf0[10], buf0[13], buf1[10], buf1[13], cos_bit);
    btf_32_sse4_1_type1(cospi[12], cospi[52], buf0[11], buf0[12], buf1[11], buf1[12], cos_bit);
    buf1[16] = _mm_add_epi32(buf0[16], buf0[17]);
    buf1[17] = _mm_sub_epi32(buf0[16], buf0[17]);
    buf1[18] = _mm_sub_epi32(buf0[19], buf0[18]);
    buf1[19] = _mm_add_epi32(buf0[19], buf0[18]);
    buf1[20] = _mm_add_epi32(buf0[20], buf0[21]);
    buf1[21] = _mm_sub_epi32(buf0[20], buf0[21]);
    buf1[22] = _mm_sub_epi32(buf0[23], buf0[22]);
    buf1[23] = _mm_add_epi32(buf0[23], buf0[22]);
    buf1[24] = _mm_add_epi32(buf0[24], buf0[25]);
    buf1[25] = _mm_sub_epi32(buf0[24], buf0[25]);
    buf1[26] = _mm_sub_epi32(buf0[27], buf0[26]);
    buf1[27] = _mm_add_epi32(buf0[27], buf0[26]);
    buf1[28] = _mm_add_epi32(buf0[28], buf0[29]);
    buf1[29] = _mm_sub_epi32(buf0[28], buf0[29]);
    buf1[30] = _mm_sub_epi32(buf0[31], buf0[30]);
    buf1[31] = _mm_add_epi32(buf0[31], buf0[30]);

    // stage 8
    cospi    = cospi_arr(cos_bit);
    buf0[0]  = buf1[0];
    buf0[1]  = buf1[1];
    buf0[2]  = buf1[2];
    buf0[3]  = buf1[3];
    buf0[4]  = buf1[4];
    buf0[5]  = buf1[5];
    buf0[6]  = buf1[6];
    buf0[7]  = buf1[7];
    buf0[8]  = buf1[8];
    buf0[9]  = buf1[9];
    buf0[10] = buf1[10];
    buf0[11] = buf1[11];
    buf0[12] = buf1[12];
    buf0[13] = buf1[13];
    buf0[14] = buf1[14];
    buf0[15] = buf1[15];
    btf_32_sse4_1_type1(cospi[62], cospi[2], buf1[16], buf1[31], buf0[16], buf0[31], cos_bit);
    btf_32_sse4_1_type1(cospi[30], cospi[34], buf1[17], buf1[30], buf0[17], buf0[30], cos_bit);
    btf_32_sse4_1_type1(cospi[46], cospi[18], buf1[18], buf1[29], buf0[18], buf0[29], cos_bit);
    btf_32_sse4_1_type1(cospi[14], cospi[50], buf1[19], buf1[28], buf0[19], buf0[28], cos_bit);
    btf_32_sse4_1_type1(cospi[54], cospi[10], buf1[20], buf1[27], buf0[20], buf0[27], cos_bit);
    btf_32_sse4_1_type1(cospi[22], cospi[42], buf1[21], buf1[26], buf0[21], buf0[26], cos_bit);
    btf_32_sse4_1_type1(cospi[38], cospi[26], buf1[22], buf1[25], buf0[22], buf0[25], cos_bit);
    btf_32_sse4_1_type1(cospi[6], cospi[58], buf1[23], buf1[24], buf0[23], buf0[24], cos_bit);

    startidx = 0 * stride;
    endidx   = 31 * stride;
    // stage 9
    output[startidx] = buf0[0];
    output[endidx]   = buf0[31];
    startidx += stride;
    endidx -= stride;
    output[startidx] = buf0[16];
    output[endidx]   = buf0[15];
    startidx += stride;
    endidx -= stride;
    output[startidx] = buf0[8];
    output[endidx]   = buf0[23];
    startidx += stride;
    endidx -= stride;
    output[startidx] = buf0[24];
    output[endidx]   = buf0[7];
    startidx += stride;
    endidx -= stride;
    output[startidx] = buf0[4];
    output[endidx]   = buf0[27];
    startidx += stride;
    endidx -= stride;
    output[startidx] = buf0[20];
    output[endidx]   = buf0[11];
    startidx += stride;
    endidx -= stride;
    output[startidx] = buf0[12];
    output[endidx]   = buf0[19];
    startidx += stride;
    endidx -= stride;
    output[startidx] = buf0[28];
    output[endidx]   = buf0[3];
    startidx += stride;
    endidx -= stride;
    output[startidx] = buf0[2];
    output[endidx]   = buf0[29];
    startidx += stride;
    endidx -= stride;
    output[startidx] = buf0[18];
    output[endidx]   = buf0[13];
    startidx += stride;
    endidx -= stride;
    output[startidx] = buf0[10];
    output[endidx]   = buf0[21];
    startidx += stride;
    endidx -= stride;
    output[startidx] = buf0[26];
    output[endidx]   = buf0[5];
    startidx += stride;
    endidx -= stride;
    output[startidx] = buf0[6];
    output[endidx]   = buf0[25];
    startidx += stride;
    endidx -= stride;
    output[startidx] = buf0[22];
    output[endidx]   = buf0[9];
    startidx += stride;
    endidx -= stride;
    output[startidx] = buf0[14];
    output[endidx]   = buf0[17];
    startidx += stride;
    endidx -= stride;
    output[startidx] = buf0[30];
    output[endidx]   = buf0[1];
}

void svt_av1_fdct64_sse4_1(__m128i* input, __m128i* output, int8_t cos_bit, const int instride, const int outstride) {
    const int32_t* cospi      = cospi_arr(cos_bit);
    const __m128i  __rounding = _mm_set1_epi32(1 << (cos_bit - 1));

    __m128i cospi_m32 = _mm_set1_epi32(-cospi[32]);
    __m128i cospi_p32 = _mm_set1_epi32(cospi[32]);
    __m128i cospi_m16 = _mm_set1_epi32(-cospi[16]);
    __m128i cospi_p48 = _mm_set1_epi32(cospi[48]);
    __m128i cospi_m48 = _mm_set1_epi32(-cospi[48]);
    __m128i cospi_p16 = _mm_set1_epi32(cospi[16]);
    __m128i cospi_m08 = _mm_set1_epi32(-cospi[8]);
    __m128i cospi_p56 = _mm_set1_epi32(cospi[56]);
    __m128i cospi_m56 = _mm_set1_epi32(-cospi[56]);
    __m128i cospi_m40 = _mm_set1_epi32(-cospi[40]);
    __m128i cospi_p24 = _mm_set1_epi32(cospi[24]);
    __m128i cospi_m24 = _mm_set1_epi32(-cospi[24]);
    __m128i cospi_p08 = _mm_set1_epi32(cospi[8]);
    __m128i cospi_p40 = _mm_set1_epi32(cospi[40]);
    __m128i cospi_p60 = _mm_set1_epi32(cospi[60]);
    __m128i cospi_p04 = _mm_set1_epi32(cospi[4]);
    __m128i cospi_p28 = _mm_set1_epi32(cospi[28]);
    __m128i cospi_p36 = _mm_set1_epi32(cospi[36]);
    __m128i cospi_p44 = _mm_set1_epi32(cospi[44]);
    __m128i cospi_p20 = _mm_set1_epi32(cospi[20]);
    __m128i cospi_p12 = _mm_set1_epi32(cospi[12]);
    __m128i cospi_p52 = _mm_set1_epi32(cospi[52]);
    __m128i cospi_m04 = _mm_set1_epi32(-cospi[4]);
    __m128i cospi_m60 = _mm_set1_epi32(-cospi[60]);
    __m128i cospi_m36 = _mm_set1_epi32(-cospi[36]);
    __m128i cospi_m28 = _mm_set1_epi32(-cospi[28]);
    __m128i cospi_m20 = _mm_set1_epi32(-cospi[20]);
    __m128i cospi_m44 = _mm_set1_epi32(-cospi[44]);
    __m128i cospi_m52 = _mm_set1_epi32(-cospi[52]);
    __m128i cospi_m12 = _mm_set1_epi32(-cospi[12]);
    __m128i cospi_p62 = _mm_set1_epi32(cospi[62]);
    __m128i cospi_p02 = _mm_set1_epi32(cospi[2]);
    __m128i cospi_p30 = _mm_set1_epi32(cospi[30]);
    __m128i cospi_p34 = _mm_set1_epi32(cospi[34]);
    __m128i cospi_p46 = _mm_set1_epi32(cospi[46]);
    __m128i cospi_p18 = _mm_set1_epi32(cospi[18]);
    __m128i cospi_p14 = _mm_set1_epi32(cospi[14]);
    __m128i cospi_p50 = _mm_set1_epi32(cospi[50]);
    __m128i cospi_p54 = _mm_set1_epi32(cospi[54]);
    __m128i cospi_p10 = _mm_set1_epi32(cospi[10]);
    __m128i cospi_p22 = _mm_set1_epi32(cospi[22]);
    __m128i cospi_p42 = _mm_set1_epi32(cospi[42]);
    __m128i cospi_p38 = _mm_set1_epi32(cospi[38]);
    __m128i cospi_p26 = _mm_set1_epi32(cospi[26]);
    __m128i cospi_p06 = _mm_set1_epi32(cospi[6]);
    __m128i cospi_p58 = _mm_set1_epi32(cospi[58]);
    __m128i cospi_p63 = _mm_set1_epi32(cospi[63]);
    __m128i cospi_p01 = _mm_set1_epi32(cospi[1]);
    __m128i cospi_p31 = _mm_set1_epi32(cospi[31]);
    __m128i cospi_p33 = _mm_set1_epi32(cospi[33]);
    __m128i cospi_p47 = _mm_set1_epi32(cospi[47]);
    __m128i cospi_p17 = _mm_set1_epi32(cospi[17]);
    __m128i cospi_p15 = _mm_set1_epi32(cospi[15]);
    __m128i cospi_p49 = _mm_set1_epi32(cospi[49]);
    __m128i cospi_p55 = _mm_set1_epi32(cospi[55]);
    __m128i cospi_p09 = _mm_set1_epi32(cospi[9]);
    __m128i cospi_p23 = _mm_set1_epi32(cospi[23]);
    __m128i cospi_p41 = _mm_set1_epi32(cospi[41]);
    __m128i cospi_p39 = _mm_set1_epi32(cospi[39]);
    __m128i cospi_p25 = _mm_set1_epi32(cospi[25]);
    __m128i cospi_p07 = _mm_set1_epi32(cospi[7]);
    __m128i cospi_p57 = _mm_set1_epi32(cospi[57]);
    __m128i cospi_p59 = _mm_set1_epi32(cospi[59]);
    __m128i cospi_p05 = _mm_set1_epi32(cospi[5]);
    __m128i cospi_p27 = _mm_set1_epi32(cospi[27]);
    __m128i cospi_p37 = _mm_set1_epi32(cospi[37]);
    __m128i cospi_p43 = _mm_set1_epi32(cospi[43]);
    __m128i cospi_p21 = _mm_set1_epi32(cospi[21]);
    __m128i cospi_p11 = _mm_set1_epi32(cospi[11]);
    __m128i cospi_p53 = _mm_set1_epi32(cospi[53]);
    __m128i cospi_p51 = _mm_set1_epi32(cospi[51]);
    __m128i cospi_p13 = _mm_set1_epi32(cospi[13]);
    __m128i cospi_p19 = _mm_set1_epi32(cospi[19]);
    __m128i cospi_p45 = _mm_set1_epi32(cospi[45]);
    __m128i cospi_p35 = _mm_set1_epi32(cospi[35]);
    __m128i cospi_p29 = _mm_set1_epi32(cospi[29]);
    __m128i cospi_p03 = _mm_set1_epi32(cospi[3]);
    __m128i cospi_p61 = _mm_set1_epi32(cospi[61]);

    int startidx = 0 * instride;
    int endidx   = 63 * instride;
    // stage 1
    __m128i x1[64];
    x1[0]  = _mm_add_epi32(input[startidx], input[endidx]);
    x1[63] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[1]  = _mm_add_epi32(input[startidx], input[endidx]);
    x1[62] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[2]  = _mm_add_epi32(input[startidx], input[endidx]);
    x1[61] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[3]  = _mm_add_epi32(input[startidx], input[endidx]);
    x1[60] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[4]  = _mm_add_epi32(input[startidx], input[endidx]);
    x1[59] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[5]  = _mm_add_epi32(input[startidx], input[endidx]);
    x1[58] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[6]  = _mm_add_epi32(input[startidx], input[endidx]);
    x1[57] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[7]  = _mm_add_epi32(input[startidx], input[endidx]);
    x1[56] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[8]  = _mm_add_epi32(input[startidx], input[endidx]);
    x1[55] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[9]  = _mm_add_epi32(input[startidx], input[endidx]);
    x1[54] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[10] = _mm_add_epi32(input[startidx], input[endidx]);
    x1[53] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[11] = _mm_add_epi32(input[startidx], input[endidx]);
    x1[52] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[12] = _mm_add_epi32(input[startidx], input[endidx]);
    x1[51] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[13] = _mm_add_epi32(input[startidx], input[endidx]);
    x1[50] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[14] = _mm_add_epi32(input[startidx], input[endidx]);
    x1[49] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[15] = _mm_add_epi32(input[startidx], input[endidx]);
    x1[48] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[16] = _mm_add_epi32(input[startidx], input[endidx]);
    x1[47] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[17] = _mm_add_epi32(input[startidx], input[endidx]);
    x1[46] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[18] = _mm_add_epi32(input[startidx], input[endidx]);
    x1[45] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[19] = _mm_add_epi32(input[startidx], input[endidx]);
    x1[44] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[20] = _mm_add_epi32(input[startidx], input[endidx]);
    x1[43] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[21] = _mm_add_epi32(input[startidx], input[endidx]);
    x1[42] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[22] = _mm_add_epi32(input[startidx], input[endidx]);
    x1[41] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[23] = _mm_add_epi32(input[startidx], input[endidx]);
    x1[40] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[24] = _mm_add_epi32(input[startidx], input[endidx]);
    x1[39] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[25] = _mm_add_epi32(input[startidx], input[endidx]);
    x1[38] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[26] = _mm_add_epi32(input[startidx], input[endidx]);
    x1[37] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[27] = _mm_add_epi32(input[startidx], input[endidx]);
    x1[36] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[28] = _mm_add_epi32(input[startidx], input[endidx]);
    x1[35] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[29] = _mm_add_epi32(input[startidx], input[endidx]);
    x1[34] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[30] = _mm_add_epi32(input[startidx], input[endidx]);
    x1[33] = _mm_sub_epi32(input[startidx], input[endidx]);
    startidx += instride;
    endidx -= instride;
    x1[31] = _mm_add_epi32(input[startidx], input[endidx]);
    x1[32] = _mm_sub_epi32(input[startidx], input[endidx]);

    // stage 2
    __m128i x2[64];
    x2[0]  = _mm_add_epi32(x1[0], x1[31]);
    x2[31] = _mm_sub_epi32(x1[0], x1[31]);
    x2[1]  = _mm_add_epi32(x1[1], x1[30]);
    x2[30] = _mm_sub_epi32(x1[1], x1[30]);
    x2[2]  = _mm_add_epi32(x1[2], x1[29]);
    x2[29] = _mm_sub_epi32(x1[2], x1[29]);
    x2[3]  = _mm_add_epi32(x1[3], x1[28]);
    x2[28] = _mm_sub_epi32(x1[3], x1[28]);
    x2[4]  = _mm_add_epi32(x1[4], x1[27]);
    x2[27] = _mm_sub_epi32(x1[4], x1[27]);
    x2[5]  = _mm_add_epi32(x1[5], x1[26]);
    x2[26] = _mm_sub_epi32(x1[5], x1[26]);
    x2[6]  = _mm_add_epi32(x1[6], x1[25]);
    x2[25] = _mm_sub_epi32(x1[6], x1[25]);
    x2[7]  = _mm_add_epi32(x1[7], x1[24]);
    x2[24] = _mm_sub_epi32(x1[7], x1[24]);
    x2[8]  = _mm_add_epi32(x1[8], x1[23]);
    x2[23] = _mm_sub_epi32(x1[8], x1[23]);
    x2[9]  = _mm_add_epi32(x1[9], x1[22]);
    x2[22] = _mm_sub_epi32(x1[9], x1[22]);
    x2[10] = _mm_add_epi32(x1[10], x1[21]);
    x2[21] = _mm_sub_epi32(x1[10], x1[21]);
    x2[11] = _mm_add_epi32(x1[11], x1[20]);
    x2[20] = _mm_sub_epi32(x1[11], x1[20]);
    x2[12] = _mm_add_epi32(x1[12], x1[19]);
    x2[19] = _mm_sub_epi32(x1[12], x1[19]);
    x2[13] = _mm_add_epi32(x1[13], x1[18]);
    x2[18] = _mm_sub_epi32(x1[13], x1[18]);
    x2[14] = _mm_add_epi32(x1[14], x1[17]);
    x2[17] = _mm_sub_epi32(x1[14], x1[17]);
    x2[15] = _mm_add_epi32(x1[15], x1[16]);
    x2[16] = _mm_sub_epi32(x1[15], x1[16]);
    x2[32] = x1[32];
    x2[33] = x1[33];
    x2[34] = x1[34];
    x2[35] = x1[35];
    x2[36] = x1[36];
    x2[37] = x1[37];
    x2[38] = x1[38];
    x2[39] = x1[39];
    btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x1[40], x1[55], x2[40], x2[55], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x1[41], x1[54], x2[41], x2[54], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x1[42], x1[53], x2[42], x2[53], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x1[43], x1[52], x2[43], x2[52], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x1[44], x1[51], x2[44], x2[51], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x1[45], x1[50], x2[45], x2[50], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x1[46], x1[49], x2[46], x2[49], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x1[47], x1[48], x2[47], x2[48], __rounding, cos_bit);
    x2[56] = x1[56];
    x2[57] = x1[57];
    x2[58] = x1[58];
    x2[59] = x1[59];
    x2[60] = x1[60];
    x2[61] = x1[61];
    x2[62] = x1[62];
    x2[63] = x1[63];

    // stage 3
    __m128i x3[64];
    x3[0]  = _mm_add_epi32(x2[0], x2[15]);
    x3[15] = _mm_sub_epi32(x2[0], x2[15]);
    x3[1]  = _mm_add_epi32(x2[1], x2[14]);
    x3[14] = _mm_sub_epi32(x2[1], x2[14]);
    x3[2]  = _mm_add_epi32(x2[2], x2[13]);
    x3[13] = _mm_sub_epi32(x2[2], x2[13]);
    x3[3]  = _mm_add_epi32(x2[3], x2[12]);
    x3[12] = _mm_sub_epi32(x2[3], x2[12]);
    x3[4]  = _mm_add_epi32(x2[4], x2[11]);
    x3[11] = _mm_sub_epi32(x2[4], x2[11]);
    x3[5]  = _mm_add_epi32(x2[5], x2[10]);
    x3[10] = _mm_sub_epi32(x2[5], x2[10]);
    x3[6]  = _mm_add_epi32(x2[6], x2[9]);
    x3[9]  = _mm_sub_epi32(x2[6], x2[9]);
    x3[7]  = _mm_add_epi32(x2[7], x2[8]);
    x3[8]  = _mm_sub_epi32(x2[7], x2[8]);
    x3[16] = x2[16];
    x3[17] = x2[17];
    x3[18] = x2[18];
    x3[19] = x2[19];
    btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x2[20], x2[27], x3[20], x3[27], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x2[21], x2[26], x3[21], x3[26], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x2[22], x2[25], x3[22], x3[25], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x2[23], x2[24], x3[23], x3[24], __rounding, cos_bit);
    x3[28] = x2[28];
    x3[29] = x2[29];
    x3[30] = x2[30];
    x3[31] = x2[31];
    x3[32] = _mm_add_epi32(x2[32], x2[47]);
    x3[47] = _mm_sub_epi32(x2[32], x2[47]);
    x3[33] = _mm_add_epi32(x2[33], x2[46]);
    x3[46] = _mm_sub_epi32(x2[33], x2[46]);
    x3[34] = _mm_add_epi32(x2[34], x2[45]);
    x3[45] = _mm_sub_epi32(x2[34], x2[45]);
    x3[35] = _mm_add_epi32(x2[35], x2[44]);
    x3[44] = _mm_sub_epi32(x2[35], x2[44]);
    x3[36] = _mm_add_epi32(x2[36], x2[43]);
    x3[43] = _mm_sub_epi32(x2[36], x2[43]);
    x3[37] = _mm_add_epi32(x2[37], x2[42]);
    x3[42] = _mm_sub_epi32(x2[37], x2[42]);
    x3[38] = _mm_add_epi32(x2[38], x2[41]);
    x3[41] = _mm_sub_epi32(x2[38], x2[41]);
    x3[39] = _mm_add_epi32(x2[39], x2[40]);
    x3[40] = _mm_sub_epi32(x2[39], x2[40]);
    x3[48] = _mm_sub_epi32(x2[63], x2[48]);
    x3[63] = _mm_add_epi32(x2[63], x2[48]);
    x3[49] = _mm_sub_epi32(x2[62], x2[49]);
    x3[62] = _mm_add_epi32(x2[62], x2[49]);
    x3[50] = _mm_sub_epi32(x2[61], x2[50]);
    x3[61] = _mm_add_epi32(x2[61], x2[50]);
    x3[51] = _mm_sub_epi32(x2[60], x2[51]);
    x3[60] = _mm_add_epi32(x2[60], x2[51]);
    x3[52] = _mm_sub_epi32(x2[59], x2[52]);
    x3[59] = _mm_add_epi32(x2[59], x2[52]);
    x3[53] = _mm_sub_epi32(x2[58], x2[53]);
    x3[58] = _mm_add_epi32(x2[58], x2[53]);
    x3[54] = _mm_sub_epi32(x2[57], x2[54]);
    x3[57] = _mm_add_epi32(x2[57], x2[54]);
    x3[55] = _mm_sub_epi32(x2[56], x2[55]);
    x3[56] = _mm_add_epi32(x2[56], x2[55]);

    // stage 4
    __m128i x4[64];
    x4[0] = _mm_add_epi32(x3[0], x3[7]);
    x4[7] = _mm_sub_epi32(x3[0], x3[7]);
    x4[1] = _mm_add_epi32(x3[1], x3[6]);
    x4[6] = _mm_sub_epi32(x3[1], x3[6]);
    x4[2] = _mm_add_epi32(x3[2], x3[5]);
    x4[5] = _mm_sub_epi32(x3[2], x3[5]);
    x4[3] = _mm_add_epi32(x3[3], x3[4]);
    x4[4] = _mm_sub_epi32(x3[3], x3[4]);
    x4[8] = x3[8];
    x4[9] = x3[9];
    btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x3[10], x3[13], x4[10], x4[13], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x3[11], x3[12], x4[11], x4[12], __rounding, cos_bit);
    x4[14] = x3[14];
    x4[15] = x3[15];
    x4[16] = _mm_add_epi32(x3[16], x3[23]);
    x4[23] = _mm_sub_epi32(x3[16], x3[23]);
    x4[17] = _mm_add_epi32(x3[17], x3[22]);
    x4[22] = _mm_sub_epi32(x3[17], x3[22]);
    x4[18] = _mm_add_epi32(x3[18], x3[21]);
    x4[21] = _mm_sub_epi32(x3[18], x3[21]);
    x4[19] = _mm_add_epi32(x3[19], x3[20]);
    x4[20] = _mm_sub_epi32(x3[19], x3[20]);
    x4[24] = _mm_sub_epi32(x3[31], x3[24]);
    x4[31] = _mm_add_epi32(x3[31], x3[24]);
    x4[25] = _mm_sub_epi32(x3[30], x3[25]);
    x4[30] = _mm_add_epi32(x3[30], x3[25]);
    x4[26] = _mm_sub_epi32(x3[29], x3[26]);
    x4[29] = _mm_add_epi32(x3[29], x3[26]);
    x4[27] = _mm_sub_epi32(x3[28], x3[27]);
    x4[28] = _mm_add_epi32(x3[28], x3[27]);
    x4[32] = x3[32];
    x4[33] = x3[33];
    x4[34] = x3[34];
    x4[35] = x3[35];
    btf_32_type0_sse4_1_new(cospi_m16, cospi_p48, x3[36], x3[59], x4[36], x4[59], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m16, cospi_p48, x3[37], x3[58], x4[37], x4[58], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m16, cospi_p48, x3[38], x3[57], x4[38], x4[57], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m16, cospi_p48, x3[39], x3[56], x4[39], x4[56], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m48, cospi_m16, x3[40], x3[55], x4[40], x4[55], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m48, cospi_m16, x3[41], x3[54], x4[41], x4[54], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m48, cospi_m16, x3[42], x3[53], x4[42], x4[53], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m48, cospi_m16, x3[43], x3[52], x4[43], x4[52], __rounding, cos_bit);
    x4[44] = x3[44];
    x4[45] = x3[45];
    x4[46] = x3[46];
    x4[47] = x3[47];
    x4[48] = x3[48];
    x4[49] = x3[49];
    x4[50] = x3[50];
    x4[51] = x3[51];
    x4[60] = x3[60];
    x4[61] = x3[61];
    x4[62] = x3[62];
    x4[63] = x3[63];

    // stage 5
    __m128i x5[64];
    x5[0] = _mm_add_epi32(x4[0], x4[3]);
    x5[3] = _mm_sub_epi32(x4[0], x4[3]);
    x5[1] = _mm_add_epi32(x4[1], x4[2]);
    x5[2] = _mm_sub_epi32(x4[1], x4[2]);
    x5[4] = x4[4];
    btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x4[5], x4[6], x5[5], x5[6], __rounding, cos_bit);
    x5[7]  = x4[7];
    x5[8]  = _mm_add_epi32(x4[8], x4[11]);
    x5[11] = _mm_sub_epi32(x4[8], x4[11]);
    x5[9]  = _mm_add_epi32(x4[9], x4[10]);
    x5[10] = _mm_sub_epi32(x4[9], x4[10]);
    x5[12] = _mm_sub_epi32(x4[15], x4[12]);
    x5[15] = _mm_add_epi32(x4[15], x4[12]);
    x5[13] = _mm_sub_epi32(x4[14], x4[13]);
    x5[14] = _mm_add_epi32(x4[14], x4[13]);
    x5[16] = x4[16];
    x5[17] = x4[17];
    btf_32_type0_sse4_1_new(cospi_m16, cospi_p48, x4[18], x4[29], x5[18], x5[29], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m16, cospi_p48, x4[19], x4[28], x5[19], x5[28], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m48, cospi_m16, x4[20], x4[27], x5[20], x5[27], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m48, cospi_m16, x4[21], x4[26], x5[21], x5[26], __rounding, cos_bit);
    x5[22] = x4[22];
    x5[23] = x4[23];
    x5[24] = x4[24];
    x5[25] = x4[25];
    x5[30] = x4[30];
    x5[31] = x4[31];
    x5[32] = _mm_add_epi32(x4[32], x4[39]);
    x5[39] = _mm_sub_epi32(x4[32], x4[39]);
    x5[33] = _mm_add_epi32(x4[33], x4[38]);
    x5[38] = _mm_sub_epi32(x4[33], x4[38]);
    x5[34] = _mm_add_epi32(x4[34], x4[37]);
    x5[37] = _mm_sub_epi32(x4[34], x4[37]);
    x5[35] = _mm_add_epi32(x4[35], x4[36]);
    x5[36] = _mm_sub_epi32(x4[35], x4[36]);
    x5[40] = _mm_sub_epi32(x4[47], x4[40]);
    x5[47] = _mm_add_epi32(x4[47], x4[40]);
    x5[41] = _mm_sub_epi32(x4[46], x4[41]);
    x5[46] = _mm_add_epi32(x4[46], x4[41]);
    x5[42] = _mm_sub_epi32(x4[45], x4[42]);
    x5[45] = _mm_add_epi32(x4[45], x4[42]);
    x5[43] = _mm_sub_epi32(x4[44], x4[43]);
    x5[44] = _mm_add_epi32(x4[44], x4[43]);
    x5[48] = _mm_add_epi32(x4[48], x4[55]);
    x5[55] = _mm_sub_epi32(x4[48], x4[55]);
    x5[49] = _mm_add_epi32(x4[49], x4[54]);
    x5[54] = _mm_sub_epi32(x4[49], x4[54]);
    x5[50] = _mm_add_epi32(x4[50], x4[53]);
    x5[53] = _mm_sub_epi32(x4[50], x4[53]);
    x5[51] = _mm_add_epi32(x4[51], x4[52]);
    x5[52] = _mm_sub_epi32(x4[51], x4[52]);
    x5[56] = _mm_sub_epi32(x4[63], x4[56]);
    x5[63] = _mm_add_epi32(x4[63], x4[56]);
    x5[57] = _mm_sub_epi32(x4[62], x4[57]);
    x5[62] = _mm_add_epi32(x4[62], x4[57]);
    x5[58] = _mm_sub_epi32(x4[61], x4[58]);
    x5[61] = _mm_add_epi32(x4[61], x4[58]);
    x5[59] = _mm_sub_epi32(x4[60], x4[59]);
    x5[60] = _mm_add_epi32(x4[60], x4[59]);

    // stage 6
    __m128i x6[64];
    btf_32_type0_sse4_1_new(cospi_p32, cospi_p32, x5[0], x5[1], x6[0], x6[1], __rounding, cos_bit);
    btf_32_type1_sse4_1_new(cospi_p48, cospi_p16, x5[2], x5[3], x6[2], x6[3], __rounding, cos_bit);
    x6[4] = _mm_add_epi32(x5[4], x5[5]);
    x6[5] = _mm_sub_epi32(x5[4], x5[5]);
    x6[6] = _mm_sub_epi32(x5[7], x5[6]);
    x6[7] = _mm_add_epi32(x5[7], x5[6]);
    x6[8] = x5[8];
    btf_32_type0_sse4_1_new(cospi_m16, cospi_p48, x5[9], x5[14], x6[9], x6[14], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m48, cospi_m16, x5[10], x5[13], x6[10], x6[13], __rounding, cos_bit);
    x6[11] = x5[11];
    x6[12] = x5[12];
    x6[15] = x5[15];
    x6[16] = _mm_add_epi32(x5[16], x5[19]);
    x6[19] = _mm_sub_epi32(x5[16], x5[19]);
    x6[17] = _mm_add_epi32(x5[17], x5[18]);
    x6[18] = _mm_sub_epi32(x5[17], x5[18]);
    x6[20] = _mm_sub_epi32(x5[23], x5[20]);
    x6[23] = _mm_add_epi32(x5[23], x5[20]);
    x6[21] = _mm_sub_epi32(x5[22], x5[21]);
    x6[22] = _mm_add_epi32(x5[22], x5[21]);
    x6[24] = _mm_add_epi32(x5[24], x5[27]);
    x6[27] = _mm_sub_epi32(x5[24], x5[27]);
    x6[25] = _mm_add_epi32(x5[25], x5[26]);
    x6[26] = _mm_sub_epi32(x5[25], x5[26]);
    x6[28] = _mm_sub_epi32(x5[31], x5[28]);
    x6[31] = _mm_add_epi32(x5[31], x5[28]);
    x6[29] = _mm_sub_epi32(x5[30], x5[29]);
    x6[30] = _mm_add_epi32(x5[30], x5[29]);
    x6[32] = x5[32];
    x6[33] = x5[33];
    btf_32_type0_sse4_1_new(cospi_m08, cospi_p56, x5[34], x5[61], x6[34], x6[61], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m08, cospi_p56, x5[35], x5[60], x6[35], x6[60], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m56, cospi_m08, x5[36], x5[59], x6[36], x6[59], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m56, cospi_m08, x5[37], x5[58], x6[37], x6[58], __rounding, cos_bit);
    x6[38] = x5[38];
    x6[39] = x5[39];
    x6[40] = x5[40];
    x6[41] = x5[41];
    btf_32_type0_sse4_1_new(cospi_m40, cospi_p24, x5[42], x5[53], x6[42], x6[53], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m40, cospi_p24, x5[43], x5[52], x6[43], x6[52], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m24, cospi_m40, x5[44], x5[51], x6[44], x6[51], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m24, cospi_m40, x5[45], x5[50], x6[45], x6[50], __rounding, cos_bit);
    x6[46] = x5[46];
    x6[47] = x5[47];
    x6[48] = x5[48];
    x6[49] = x5[49];
    x6[54] = x5[54];
    x6[55] = x5[55];
    x6[56] = x5[56];
    x6[57] = x5[57];
    x6[62] = x5[62];
    x6[63] = x5[63];

    // stage 7
    __m128i x7[64];
    x7[0] = x6[0];
    x7[1] = x6[1];
    x7[2] = x6[2];
    x7[3] = x6[3];
    btf_32_type1_sse4_1_new(cospi_p56, cospi_p08, x6[4], x6[7], x7[4], x7[7], __rounding, cos_bit);
    btf_32_type1_sse4_1_new(cospi_p24, cospi_p40, x6[5], x6[6], x7[5], x7[6], __rounding, cos_bit);
    x7[8]  = _mm_add_epi32(x6[8], x6[9]);
    x7[9]  = _mm_sub_epi32(x6[8], x6[9]);
    x7[10] = _mm_sub_epi32(x6[11], x6[10]);
    x7[11] = _mm_add_epi32(x6[11], x6[10]);
    x7[12] = _mm_add_epi32(x6[12], x6[13]);
    x7[13] = _mm_sub_epi32(x6[12], x6[13]);
    x7[14] = _mm_sub_epi32(x6[15], x6[14]);
    x7[15] = _mm_add_epi32(x6[15], x6[14]);
    x7[16] = x6[16];
    btf_32_type0_sse4_1_new(cospi_m08, cospi_p56, x6[17], x6[30], x7[17], x7[30], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m56, cospi_m08, x6[18], x6[29], x7[18], x7[29], __rounding, cos_bit);
    x7[19] = x6[19];
    x7[20] = x6[20];
    btf_32_type0_sse4_1_new(cospi_m40, cospi_p24, x6[21], x6[26], x7[21], x7[26], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m24, cospi_m40, x6[22], x6[25], x7[22], x7[25], __rounding, cos_bit);
    x7[23] = x6[23];
    x7[24] = x6[24];
    x7[27] = x6[27];
    x7[28] = x6[28];
    x7[31] = x6[31];
    x7[32] = _mm_add_epi32(x6[32], x6[35]);
    x7[35] = _mm_sub_epi32(x6[32], x6[35]);
    x7[33] = _mm_add_epi32(x6[33], x6[34]);
    x7[34] = _mm_sub_epi32(x6[33], x6[34]);
    x7[36] = _mm_sub_epi32(x6[39], x6[36]);
    x7[39] = _mm_add_epi32(x6[39], x6[36]);
    x7[37] = _mm_sub_epi32(x6[38], x6[37]);
    x7[38] = _mm_add_epi32(x6[38], x6[37]);
    x7[40] = _mm_add_epi32(x6[40], x6[43]);
    x7[43] = _mm_sub_epi32(x6[40], x6[43]);
    x7[41] = _mm_add_epi32(x6[41], x6[42]);
    x7[42] = _mm_sub_epi32(x6[41], x6[42]);
    x7[44] = _mm_sub_epi32(x6[47], x6[44]);
    x7[47] = _mm_add_epi32(x6[47], x6[44]);
    x7[45] = _mm_sub_epi32(x6[46], x6[45]);
    x7[46] = _mm_add_epi32(x6[46], x6[45]);
    x7[48] = _mm_add_epi32(x6[48], x6[51]);
    x7[51] = _mm_sub_epi32(x6[48], x6[51]);
    x7[49] = _mm_add_epi32(x6[49], x6[50]);
    x7[50] = _mm_sub_epi32(x6[49], x6[50]);
    x7[52] = _mm_sub_epi32(x6[55], x6[52]);
    x7[55] = _mm_add_epi32(x6[55], x6[52]);
    x7[53] = _mm_sub_epi32(x6[54], x6[53]);
    x7[54] = _mm_add_epi32(x6[54], x6[53]);
    x7[56] = _mm_add_epi32(x6[56], x6[59]);
    x7[59] = _mm_sub_epi32(x6[56], x6[59]);
    x7[57] = _mm_add_epi32(x6[57], x6[58]);
    x7[58] = _mm_sub_epi32(x6[57], x6[58]);
    x7[60] = _mm_sub_epi32(x6[63], x6[60]);
    x7[63] = _mm_add_epi32(x6[63], x6[60]);
    x7[61] = _mm_sub_epi32(x6[62], x6[61]);
    x7[62] = _mm_add_epi32(x6[62], x6[61]);

    // stage 8
    __m128i x8[64];
    x8[0] = x7[0];
    x8[1] = x7[1];
    x8[2] = x7[2];
    x8[3] = x7[3];
    x8[4] = x7[4];
    x8[5] = x7[5];
    x8[6] = x7[6];
    x8[7] = x7[7];
    btf_32_type1_sse4_1_new(cospi_p60, cospi_p04, x7[8], x7[15], x8[8], x8[15], __rounding, cos_bit);
    btf_32_type1_sse4_1_new(cospi_p28, cospi_p36, x7[9], x7[14], x8[9], x8[14], __rounding, cos_bit);
    btf_32_type1_sse4_1_new(cospi_p44, cospi_p20, x7[10], x7[13], x8[10], x8[13], __rounding, cos_bit);
    btf_32_type1_sse4_1_new(cospi_p12, cospi_p52, x7[11], x7[12], x8[11], x8[12], __rounding, cos_bit);
    x8[16] = _mm_add_epi32(x7[16], x7[17]);
    x8[17] = _mm_sub_epi32(x7[16], x7[17]);
    x8[18] = _mm_sub_epi32(x7[19], x7[18]);
    x8[19] = _mm_add_epi32(x7[19], x7[18]);
    x8[20] = _mm_add_epi32(x7[20], x7[21]);
    x8[21] = _mm_sub_epi32(x7[20], x7[21]);
    x8[22] = _mm_sub_epi32(x7[23], x7[22]);
    x8[23] = _mm_add_epi32(x7[23], x7[22]);
    x8[24] = _mm_add_epi32(x7[24], x7[25]);
    x8[25] = _mm_sub_epi32(x7[24], x7[25]);
    x8[26] = _mm_sub_epi32(x7[27], x7[26]);
    x8[27] = _mm_add_epi32(x7[27], x7[26]);
    x8[28] = _mm_add_epi32(x7[28], x7[29]);
    x8[29] = _mm_sub_epi32(x7[28], x7[29]);
    x8[30] = _mm_sub_epi32(x7[31], x7[30]);
    x8[31] = _mm_add_epi32(x7[31], x7[30]);
    x8[32] = x7[32];
    btf_32_type0_sse4_1_new(cospi_m04, cospi_p60, x7[33], x7[62], x8[33], x8[62], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m60, cospi_m04, x7[34], x7[61], x8[34], x8[61], __rounding, cos_bit);
    x8[35] = x7[35];
    x8[36] = x7[36];
    btf_32_type0_sse4_1_new(cospi_m36, cospi_p28, x7[37], x7[58], x8[37], x8[58], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m28, cospi_m36, x7[38], x7[57], x8[38], x8[57], __rounding, cos_bit);
    x8[39] = x7[39];
    x8[40] = x7[40];
    btf_32_type0_sse4_1_new(cospi_m20, cospi_p44, x7[41], x7[54], x8[41], x8[54], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m44, cospi_m20, x7[42], x7[53], x8[42], x8[53], __rounding, cos_bit);
    x8[43] = x7[43];
    x8[44] = x7[44];
    btf_32_type0_sse4_1_new(cospi_m52, cospi_p12, x7[45], x7[50], x8[45], x8[50], __rounding, cos_bit);
    btf_32_type0_sse4_1_new(cospi_m12, cospi_m52, x7[46], x7[49], x8[46], x8[49], __rounding, cos_bit);
    x8[47] = x7[47];
    x8[48] = x7[48];
    x8[51] = x7[51];
    x8[52] = x7[52];
    x8[55] = x7[55];
    x8[56] = x7[56];
    x8[59] = x7[59];
    x8[60] = x7[60];
    x8[63] = x7[63];

    // stage 9
    __m128i x9[64];
    x9[0]  = x8[0];
    x9[1]  = x8[1];
    x9[2]  = x8[2];
    x9[3]  = x8[3];
    x9[4]  = x8[4];
    x9[5]  = x8[5];
    x9[6]  = x8[6];
    x9[7]  = x8[7];
    x9[8]  = x8[8];
    x9[9]  = x8[9];
    x9[10] = x8[10];
    x9[11] = x8[11];
    x9[12] = x8[12];
    x9[13] = x8[13];
    x9[14] = x8[14];
    x9[15] = x8[15];
    btf_32_type1_sse4_1_new(cospi_p62, cospi_p02, x8[16], x8[31], x9[16], x9[31], __rounding, cos_bit);
    btf_32_type1_sse4_1_new(cospi_p30, cospi_p34, x8[17], x8[30], x9[17], x9[30], __rounding, cos_bit);
    btf_32_type1_sse4_1_new(cospi_p46, cospi_p18, x8[18], x8[29], x9[18], x9[29], __rounding, cos_bit);
    btf_32_type1_sse4_1_new(cospi_p14, cospi_p50, x8[19], x8[28], x9[19], x9[28], __rounding, cos_bit);
    btf_32_type1_sse4_1_new(cospi_p54, cospi_p10, x8[20], x8[27], x9[20], x9[27], __rounding, cos_bit);
    btf_32_type1_sse4_1_new(cospi_p22, cospi_p42, x8[21], x8[26], x9[21], x9[26], __rounding, cos_bit);
    btf_32_type1_sse4_1_new(cospi_p38, cospi_p26, x8[22], x8[25], x9[22], x9[25], __rounding, cos_bit);
    btf_32_type1_sse4_1_new(cospi_p06, cospi_p58, x8[23], x8[24], x9[23], x9[24], __rounding, cos_bit);
    x9[32] = _mm_add_epi32(x8[32], x8[33]);
    x9[33] = _mm_sub_epi32(x8[32], x8[33]);
    x9[34] = _mm_sub_epi32(x8[35], x8[34]);
    x9[35] = _mm_add_epi32(x8[35], x8[34]);
    x9[36] = _mm_add_epi32(x8[36], x8[37]);
    x9[37] = _mm_sub_epi32(x8[36], x8[37]);
    x9[38] = _mm_sub_epi32(x8[39], x8[38]);
    x9[39] = _mm_add_epi32(x8[39], x8[38]);
    x9[40] = _mm_add_epi32(x8[40], x8[41]);
    x9[41] = _mm_sub_epi32(x8[40], x8[41]);
    x9[42] = _mm_sub_epi32(x8[43], x8[42]);
    x9[43] = _mm_add_epi32(x8[43], x8[42]);
    x9[44] = _mm_add_epi32(x8[44], x8[45]);
    x9[45] = _mm_sub_epi32(x8[44], x8[45]);
    x9[46] = _mm_sub_epi32(x8[47], x8[46]);
    x9[47] = _mm_add_epi32(x8[47], x8[46]);
    x9[48] = _mm_add_epi32(x8[48], x8[49]);
    x9[49] = _mm_sub_epi32(x8[48], x8[49]);
    x9[50] = _mm_sub_epi32(x8[51], x8[50]);
    x9[51] = _mm_add_epi32(x8[51], x8[50]);
    x9[52] = _mm_add_epi32(x8[52], x8[53]);
    x9[53] = _mm_sub_epi32(x8[52], x8[53]);
    x9[54] = _mm_sub_epi32(x8[55], x8[54]);
    x9[55] = _mm_add_epi32(x8[55], x8[54]);
    x9[56] = _mm_add_epi32(x8[56], x8[57]);
    x9[57] = _mm_sub_epi32(x8[56], x8[57]);
    x9[58] = _mm_sub_epi32(x8[59], x8[58]);
    x9[59] = _mm_add_epi32(x8[59], x8[58]);
    x9[60] = _mm_add_epi32(x8[60], x8[61]);
    x9[61] = _mm_sub_epi32(x8[60], x8[61]);
    x9[62] = _mm_sub_epi32(x8[63], x8[62]);
    x9[63] = _mm_add_epi32(x8[63], x8[62]);

    // stage 10
    __m128i x10[64];
    x10[0]  = x9[0];
    x10[1]  = x9[1];
    x10[2]  = x9[2];
    x10[3]  = x9[3];
    x10[4]  = x9[4];
    x10[5]  = x9[5];
    x10[6]  = x9[6];
    x10[7]  = x9[7];
    x10[8]  = x9[8];
    x10[9]  = x9[9];
    x10[10] = x9[10];
    x10[11] = x9[11];
    x10[12] = x9[12];
    x10[13] = x9[13];
    x10[14] = x9[14];
    x10[15] = x9[15];
    x10[16] = x9[16];
    x10[17] = x9[17];
    x10[18] = x9[18];
    x10[19] = x9[19];
    x10[20] = x9[20];
    x10[21] = x9[21];
    x10[22] = x9[22];
    x10[23] = x9[23];
    x10[24] = x9[24];
    x10[25] = x9[25];
    x10[26] = x9[26];
    x10[27] = x9[27];
    x10[28] = x9[28];
    x10[29] = x9[29];
    x10[30] = x9[30];
    x10[31] = x9[31];
    btf_32_type1_sse4_1_new(cospi_p63, cospi_p01, x9[32], x9[63], x10[32], x10[63], __rounding, cos_bit);
    btf_32_type1_sse4_1_new(cospi_p31, cospi_p33, x9[33], x9[62], x10[33], x10[62], __rounding, cos_bit);
    btf_32_type1_sse4_1_new(cospi_p47, cospi_p17, x9[34], x9[61], x10[34], x10[61], __rounding, cos_bit);
    btf_32_type1_sse4_1_new(cospi_p15, cospi_p49, x9[35], x9[60], x10[35], x10[60], __rounding, cos_bit);
    btf_32_type1_sse4_1_new(cospi_p55, cospi_p09, x9[36], x9[59], x10[36], x10[59], __rounding, cos_bit);
    btf_32_type1_sse4_1_new(cospi_p23, cospi_p41, x9[37], x9[58], x10[37], x10[58], __rounding, cos_bit);
    btf_32_type1_sse4_1_new(cospi_p39, cospi_p25, x9[38], x9[57], x10[38], x10[57], __rounding, cos_bit);
    btf_32_type1_sse4_1_new(cospi_p07, cospi_p57, x9[39], x9[56], x10[39], x10[56], __rounding, cos_bit);
    btf_32_type1_sse4_1_new(cospi_p59, cospi_p05, x9[40], x9[55], x10[40], x10[55], __rounding, cos_bit);
    btf_32_type1_sse4_1_new(cospi_p27, cospi_p37, x9[41], x9[54], x10[41], x10[54], __rounding, cos_bit);
    btf_32_type1_sse4_1_new(cospi_p43, cospi_p21, x9[42], x9[53], x10[42], x10[53], __rounding, cos_bit);
    btf_32_type1_sse4_1_new(cospi_p11, cospi_p53, x9[43], x9[52], x10[43], x10[52], __rounding, cos_bit);
    btf_32_type1_sse4_1_new(cospi_p51, cospi_p13, x9[44], x9[51], x10[44], x10[51], __rounding, cos_bit);
    btf_32_type1_sse4_1_new(cospi_p19, cospi_p45, x9[45], x9[50], x10[45], x10[50], __rounding, cos_bit);
    btf_32_type1_sse4_1_new(cospi_p35, cospi_p29, x9[46], x9[49], x10[46], x10[49], __rounding, cos_bit);
    btf_32_type1_sse4_1_new(cospi_p03, cospi_p61, x9[47], x9[48], x10[47], x10[48], __rounding, cos_bit);

    startidx = 0 * outstride;
    endidx   = 63 * outstride;
    // stage 11
    output[startidx] = x10[0];
    output[endidx]   = x10[63];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[32];
    output[endidx]   = x10[31];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[16];
    output[endidx]   = x10[47];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[48];
    output[endidx]   = x10[15];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[8];
    output[endidx]   = x10[55];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[40];
    output[endidx]   = x10[23];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[24];
    output[endidx]   = x10[39];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[56];
    output[endidx]   = x10[7];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[4];
    output[endidx]   = x10[59];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[36];
    output[endidx]   = x10[27];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[20];
    output[endidx]   = x10[43];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[52];
    output[endidx]   = x10[11];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[12];
    output[endidx]   = x10[51];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[44];
    output[endidx]   = x10[19];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[28];
    output[endidx]   = x10[35];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[60];
    output[endidx]   = x10[3];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[2];
    output[endidx]   = x10[61];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[34];
    output[endidx]   = x10[29];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[18];
    output[endidx]   = x10[45];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[50];
    output[endidx]   = x10[13];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[10];
    output[endidx]   = x10[53];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[42];
    output[endidx]   = x10[21];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[26];
    output[endidx]   = x10[37];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[58];
    output[endidx]   = x10[5];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[6];
    output[endidx]   = x10[57];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[38];
    output[endidx]   = x10[25];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[22];
    output[endidx]   = x10[41];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[54];
    output[endidx]   = x10[9];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[14];
    output[endidx]   = x10[49];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[46];
    output[endidx]   = x10[17];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[30];
    output[endidx]   = x10[33];
    startidx += outstride;
    endidx -= outstride;
    output[startidx] = x10[62];
    output[endidx]   = x10[1];
}

static void fdct32_sse4_1(__m128i* input, __m128i* output, const int8_t cos_bit, const int8_t* stage_range) {
    const int txfm_size   = 32;
    const int num_per_128 = 4;
    int       col_num     = txfm_size / num_per_128;
    int       col;
    (void)stage_range;
    for (col = 0; col < col_num; col++) {
        svt_av1_fdct32_sse4_1((input + col), (output + col), cos_bit, col_num);
    }
}

static void fdct16x16_sse4_1(__m128i* in, __m128i* out, int bit, const int col_num) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospim32 = _mm_set1_epi32(-cospi[32]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospi16  = _mm_set1_epi32(cospi[16]);
    const __m128i  cospim48 = _mm_set1_epi32(-cospi[48]);
    const __m128i  cospim16 = _mm_set1_epi32(-cospi[16]);
    const __m128i  cospi56  = _mm_set1_epi32(cospi[56]);
    const __m128i  cospi8   = _mm_set1_epi32(cospi[8]);
    const __m128i  cospi24  = _mm_set1_epi32(cospi[24]);
    const __m128i  cospi40  = _mm_set1_epi32(cospi[40]);
    const __m128i  cospi60  = _mm_set1_epi32(cospi[60]);
    const __m128i  cospi4   = _mm_set1_epi32(cospi[4]);
    const __m128i  cospi28  = _mm_set1_epi32(cospi[28]);
    const __m128i  cospi36  = _mm_set1_epi32(cospi[36]);
    const __m128i  cospi44  = _mm_set1_epi32(cospi[44]);
    const __m128i  cospi20  = _mm_set1_epi32(cospi[20]);
    const __m128i  cospi12  = _mm_set1_epi32(cospi[12]);
    const __m128i  cospi52  = _mm_set1_epi32(cospi[52]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    __m128i        u[16], v[16], x;
    int            col;

    // Calculate the column 0, 1, 2, 3
    for (col = 0; col < col_num; ++col) {
        // stage 0
        // stage 1
        u[0]  = _mm_add_epi32(in[0 * col_num + col], in[15 * col_num + col]);
        u[15] = _mm_sub_epi32(in[0 * col_num + col], in[15 * col_num + col]);
        u[1]  = _mm_add_epi32(in[1 * col_num + col], in[14 * col_num + col]);
        u[14] = _mm_sub_epi32(in[1 * col_num + col], in[14 * col_num + col]);
        u[2]  = _mm_add_epi32(in[2 * col_num + col], in[13 * col_num + col]);
        u[13] = _mm_sub_epi32(in[2 * col_num + col], in[13 * col_num + col]);
        u[3]  = _mm_add_epi32(in[3 * col_num + col], in[12 * col_num + col]);
        u[12] = _mm_sub_epi32(in[3 * col_num + col], in[12 * col_num + col]);
        u[4]  = _mm_add_epi32(in[4 * col_num + col], in[11 * col_num + col]);
        u[11] = _mm_sub_epi32(in[4 * col_num + col], in[11 * col_num + col]);
        u[5]  = _mm_add_epi32(in[5 * col_num + col], in[10 * col_num + col]);
        u[10] = _mm_sub_epi32(in[5 * col_num + col], in[10 * col_num + col]);
        u[6]  = _mm_add_epi32(in[6 * col_num + col], in[9 * col_num + col]);
        u[9]  = _mm_sub_epi32(in[6 * col_num + col], in[9 * col_num + col]);
        u[7]  = _mm_add_epi32(in[7 * col_num + col], in[8 * col_num + col]);
        u[8]  = _mm_sub_epi32(in[7 * col_num + col], in[8 * col_num + col]);

        // stage 2
        v[0] = _mm_add_epi32(u[0], u[7]);
        v[7] = _mm_sub_epi32(u[0], u[7]);
        v[1] = _mm_add_epi32(u[1], u[6]);
        v[6] = _mm_sub_epi32(u[1], u[6]);
        v[2] = _mm_add_epi32(u[2], u[5]);
        v[5] = _mm_sub_epi32(u[2], u[5]);
        v[3] = _mm_add_epi32(u[3], u[4]);
        v[4] = _mm_sub_epi32(u[3], u[4]);
        v[8] = u[8];
        v[9] = u[9];

        v[10] = _mm_mullo_epi32(u[10], cospim32);
        x     = _mm_mullo_epi32(u[13], cospi32);
        v[10] = _mm_add_epi32(v[10], x);
        v[10] = _mm_add_epi32(v[10], rnding);
        v[10] = _mm_srai_epi32(v[10], bit);

        v[13] = _mm_mullo_epi32(u[10], cospi32);
        x     = _mm_mullo_epi32(u[13], cospim32);
        v[13] = _mm_sub_epi32(v[13], x);
        v[13] = _mm_add_epi32(v[13], rnding);
        v[13] = _mm_srai_epi32(v[13], bit);

        v[11] = _mm_mullo_epi32(u[11], cospim32);
        x     = _mm_mullo_epi32(u[12], cospi32);
        v[11] = _mm_add_epi32(v[11], x);
        v[11] = _mm_add_epi32(v[11], rnding);
        v[11] = _mm_srai_epi32(v[11], bit);

        v[12] = _mm_mullo_epi32(u[11], cospi32);
        x     = _mm_mullo_epi32(u[12], cospim32);
        v[12] = _mm_sub_epi32(v[12], x);
        v[12] = _mm_add_epi32(v[12], rnding);
        v[12] = _mm_srai_epi32(v[12], bit);
        v[14] = u[14];
        v[15] = u[15];

        // stage 3
        u[0] = _mm_add_epi32(v[0], v[3]);
        u[3] = _mm_sub_epi32(v[0], v[3]);
        u[1] = _mm_add_epi32(v[1], v[2]);
        u[2] = _mm_sub_epi32(v[1], v[2]);
        u[4] = v[4];

        u[5] = _mm_mullo_epi32(v[5], cospim32);
        x    = _mm_mullo_epi32(v[6], cospi32);
        u[5] = _mm_add_epi32(u[5], x);
        u[5] = _mm_add_epi32(u[5], rnding);
        u[5] = _mm_srai_epi32(u[5], bit);

        u[6] = _mm_mullo_epi32(v[5], cospi32);
        x    = _mm_mullo_epi32(v[6], cospim32);
        u[6] = _mm_sub_epi32(u[6], x);
        u[6] = _mm_add_epi32(u[6], rnding);
        u[6] = _mm_srai_epi32(u[6], bit);

        u[7]  = v[7];
        u[8]  = _mm_add_epi32(v[8], v[11]);
        u[11] = _mm_sub_epi32(v[8], v[11]);
        u[9]  = _mm_add_epi32(v[9], v[10]);
        u[10] = _mm_sub_epi32(v[9], v[10]);
        u[12] = _mm_sub_epi32(v[15], v[12]);
        u[15] = _mm_add_epi32(v[15], v[12]);
        u[13] = _mm_sub_epi32(v[14], v[13]);
        u[14] = _mm_add_epi32(v[14], v[13]);

        // stage 4
        u[0] = _mm_mullo_epi32(u[0], cospi32);
        u[1] = _mm_mullo_epi32(u[1], cospi32);
        v[0] = _mm_add_epi32(u[0], u[1]);
        v[0] = _mm_add_epi32(v[0], rnding);
        v[0] = _mm_srai_epi32(v[0], bit);

        v[1] = _mm_sub_epi32(u[0], u[1]);
        v[1] = _mm_add_epi32(v[1], rnding);
        v[1] = _mm_srai_epi32(v[1], bit);

        v[2] = _mm_mullo_epi32(u[2], cospi48);
        x    = _mm_mullo_epi32(u[3], cospi16);
        v[2] = _mm_add_epi32(v[2], x);
        v[2] = _mm_add_epi32(v[2], rnding);
        v[2] = _mm_srai_epi32(v[2], bit);

        v[3] = _mm_mullo_epi32(u[2], cospi16);
        x    = _mm_mullo_epi32(u[3], cospi48);
        v[3] = _mm_sub_epi32(x, v[3]);
        v[3] = _mm_add_epi32(v[3], rnding);
        v[3] = _mm_srai_epi32(v[3], bit);

        v[4] = _mm_add_epi32(u[4], u[5]);
        v[5] = _mm_sub_epi32(u[4], u[5]);
        v[6] = _mm_sub_epi32(u[7], u[6]);
        v[7] = _mm_add_epi32(u[7], u[6]);
        v[8] = u[8];

        v[9] = _mm_mullo_epi32(u[9], cospim16);
        x    = _mm_mullo_epi32(u[14], cospi48);
        v[9] = _mm_add_epi32(v[9], x);
        v[9] = _mm_add_epi32(v[9], rnding);
        v[9] = _mm_srai_epi32(v[9], bit);

        v[14] = _mm_mullo_epi32(u[9], cospi48);
        x     = _mm_mullo_epi32(u[14], cospim16);
        v[14] = _mm_sub_epi32(v[14], x);
        v[14] = _mm_add_epi32(v[14], rnding);
        v[14] = _mm_srai_epi32(v[14], bit);

        v[10] = _mm_mullo_epi32(u[10], cospim48);
        x     = _mm_mullo_epi32(u[13], cospim16);
        v[10] = _mm_add_epi32(v[10], x);
        v[10] = _mm_add_epi32(v[10], rnding);
        v[10] = _mm_srai_epi32(v[10], bit);

        v[13] = _mm_mullo_epi32(u[10], cospim16);
        x     = _mm_mullo_epi32(u[13], cospim48);
        v[13] = _mm_sub_epi32(v[13], x);
        v[13] = _mm_add_epi32(v[13], rnding);
        v[13] = _mm_srai_epi32(v[13], bit);

        v[11] = u[11];
        v[12] = u[12];
        v[15] = u[15];

        // stage 5
        u[0] = v[0];
        u[1] = v[1];
        u[2] = v[2];
        u[3] = v[3];

        u[4] = _mm_mullo_epi32(v[4], cospi56);
        x    = _mm_mullo_epi32(v[7], cospi8);
        u[4] = _mm_add_epi32(u[4], x);
        u[4] = _mm_add_epi32(u[4], rnding);
        u[4] = _mm_srai_epi32(u[4], bit);

        u[7] = _mm_mullo_epi32(v[4], cospi8);
        x    = _mm_mullo_epi32(v[7], cospi56);
        u[7] = _mm_sub_epi32(x, u[7]);
        u[7] = _mm_add_epi32(u[7], rnding);
        u[7] = _mm_srai_epi32(u[7], bit);

        u[5] = _mm_mullo_epi32(v[5], cospi24);
        x    = _mm_mullo_epi32(v[6], cospi40);
        u[5] = _mm_add_epi32(u[5], x);
        u[5] = _mm_add_epi32(u[5], rnding);
        u[5] = _mm_srai_epi32(u[5], bit);

        u[6] = _mm_mullo_epi32(v[5], cospi40);
        x    = _mm_mullo_epi32(v[6], cospi24);
        u[6] = _mm_sub_epi32(x, u[6]);
        u[6] = _mm_add_epi32(u[6], rnding);
        u[6] = _mm_srai_epi32(u[6], bit);

        u[8]  = _mm_add_epi32(v[8], v[9]);
        u[9]  = _mm_sub_epi32(v[8], v[9]);
        u[10] = _mm_sub_epi32(v[11], v[10]);
        u[11] = _mm_add_epi32(v[11], v[10]);
        u[12] = _mm_add_epi32(v[12], v[13]);
        u[13] = _mm_sub_epi32(v[12], v[13]);
        u[14] = _mm_sub_epi32(v[15], v[14]);
        u[15] = _mm_add_epi32(v[15], v[14]);

        // stage 6
        v[0] = u[0];
        v[1] = u[1];
        v[2] = u[2];
        v[3] = u[3];
        v[4] = u[4];
        v[5] = u[5];
        v[6] = u[6];
        v[7] = u[7];

        v[8] = _mm_mullo_epi32(u[8], cospi60);
        x    = _mm_mullo_epi32(u[15], cospi4);
        v[8] = _mm_add_epi32(v[8], x);
        v[8] = _mm_add_epi32(v[8], rnding);
        v[8] = _mm_srai_epi32(v[8], bit);

        v[15] = _mm_mullo_epi32(u[8], cospi4);
        x     = _mm_mullo_epi32(u[15], cospi60);
        v[15] = _mm_sub_epi32(x, v[15]);
        v[15] = _mm_add_epi32(v[15], rnding);
        v[15] = _mm_srai_epi32(v[15], bit);

        v[9] = _mm_mullo_epi32(u[9], cospi28);
        x    = _mm_mullo_epi32(u[14], cospi36);
        v[9] = _mm_add_epi32(v[9], x);
        v[9] = _mm_add_epi32(v[9], rnding);
        v[9] = _mm_srai_epi32(v[9], bit);

        v[14] = _mm_mullo_epi32(u[9], cospi36);
        x     = _mm_mullo_epi32(u[14], cospi28);
        v[14] = _mm_sub_epi32(x, v[14]);
        v[14] = _mm_add_epi32(v[14], rnding);
        v[14] = _mm_srai_epi32(v[14], bit);

        v[10] = _mm_mullo_epi32(u[10], cospi44);
        x     = _mm_mullo_epi32(u[13], cospi20);
        v[10] = _mm_add_epi32(v[10], x);
        v[10] = _mm_add_epi32(v[10], rnding);
        v[10] = _mm_srai_epi32(v[10], bit);

        v[13] = _mm_mullo_epi32(u[10], cospi20);
        x     = _mm_mullo_epi32(u[13], cospi44);
        v[13] = _mm_sub_epi32(x, v[13]);
        v[13] = _mm_add_epi32(v[13], rnding);
        v[13] = _mm_srai_epi32(v[13], bit);

        v[11] = _mm_mullo_epi32(u[11], cospi12);
        x     = _mm_mullo_epi32(u[12], cospi52);
        v[11] = _mm_add_epi32(v[11], x);
        v[11] = _mm_add_epi32(v[11], rnding);
        v[11] = _mm_srai_epi32(v[11], bit);

        v[12] = _mm_mullo_epi32(u[11], cospi52);
        x     = _mm_mullo_epi32(u[12], cospi12);
        v[12] = _mm_sub_epi32(x, v[12]);
        v[12] = _mm_add_epi32(v[12], rnding);
        v[12] = _mm_srai_epi32(v[12], bit);

        out[0 * col_num + col]  = v[0];
        out[1 * col_num + col]  = v[8];
        out[2 * col_num + col]  = v[4];
        out[3 * col_num + col]  = v[12];
        out[4 * col_num + col]  = v[2];
        out[5 * col_num + col]  = v[10];
        out[6 * col_num + col]  = v[6];
        out[7 * col_num + col]  = v[14];
        out[8 * col_num + col]  = v[1];
        out[9 * col_num + col]  = v[9];
        out[10 * col_num + col] = v[5];
        out[11 * col_num + col] = v[13];
        out[12 * col_num + col] = v[3];
        out[13 * col_num + col] = v[11];
        out[14 * col_num + col] = v[7];
        out[15 * col_num + col] = v[15];
    }
}

static void fdct64_new_sse4_1(__m128i* input, __m128i* output, const int8_t cos_bit, const int8_t* stage_range) {
    const int txfm_size   = 64;
    const int num_per_128 = 4;
    int       col_num     = txfm_size / num_per_128;
    (void)stage_range;
    for (int col = 0; col < col_num; col++) {
        svt_av1_fdct64_sse4_1((input + col), (output + col), cos_bit, col_num, col_num);
    }
}

void svt_av1_idtx32_sse4_1(__m128i* input, __m128i* output, int cos_bit, const int col_num) {
    (void)cos_bit;
    for (int i = 0; i < 32; i++) {
        output[i * col_num] = _mm_slli_epi32(input[i * col_num], 2);
    }
}

static void idtx32x32_sse4_1(__m128i* input, __m128i* output, const int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;

    for (int i = 0; i < 8; i++) {
        svt_av1_idtx32_sse4_1(&input[i * 32], &output[i * 32], cos_bit, 1);
    }
}

static void idtx16x16_sse4_1(__m128i* in, __m128i* out, int bit, int col_num) {
    (void)bit;
    __m128i fact   = _mm_set1_epi32(2 * new_sqrt2);
    __m128i offset = _mm_set1_epi32(1 << (new_sqrt2_bits - 1));
    __m128i a_low;

    int num_iters = 16 * col_num;
    for (int i = 0; i < num_iters; i++) {
        a_low  = _mm_mullo_epi32(in[i], fact);
        a_low  = _mm_add_epi32(a_low, offset);
        out[i] = _mm_srai_epi32(a_low, new_sqrt2_bits);
    }
}

static void idtx32x8_sse4_1(__m128i* in, __m128i* out, int bit, int col_num) {
    (void)bit;
    (void)col_num;
    for (int j = 0; j < 2; j++) {
        out[j + 8 * 0] = _mm_add_epi32(in[j + 8 * 0], in[j + 8 * 0]);
        out[j + 8 * 1] = _mm_add_epi32(in[j + 8 * 1], in[j + 8 * 1]);
        out[j + 8 * 2] = _mm_add_epi32(in[j + 8 * 2], in[j + 8 * 2]);
        out[j + 8 * 3] = _mm_add_epi32(in[j + 8 * 3], in[j + 8 * 3]);
        out[j + 8 * 4] = _mm_add_epi32(in[j + 8 * 4], in[j + 8 * 4]);
        out[j + 8 * 5] = _mm_add_epi32(in[j + 8 * 5], in[j + 8 * 5]);
        out[j + 8 * 6] = _mm_add_epi32(in[j + 8 * 6], in[j + 8 * 6]);
        out[j + 8 * 7] = _mm_add_epi32(in[j + 8 * 7], in[j + 8 * 7]);
    }
}

static void idtx8x8_sse4_1(__m128i* in, __m128i* out, int bit, int col_num) {
    (void)bit;

    for (int i = 0; i < col_num; i += 1) {
        out[0 + 8 * i] = _mm_add_epi32(in[0 + 8 * i], in[0 + 8 * i]);
        out[1 + 8 * i] = _mm_add_epi32(in[1 + 8 * i], in[1 + 8 * i]);
        out[2 + 8 * i] = _mm_add_epi32(in[2 + 8 * i], in[2 + 8 * i]);
        out[3 + 8 * i] = _mm_add_epi32(in[3 + 8 * i], in[3 + 8 * i]);
        out[4 + 8 * i] = _mm_add_epi32(in[4 + 8 * i], in[4 + 8 * i]);
        out[5 + 8 * i] = _mm_add_epi32(in[5 + 8 * i], in[5 + 8 * i]);
        out[6 + 8 * i] = _mm_add_epi32(in[6 + 8 * i], in[6 + 8 * i]);
        out[7 + 8 * i] = _mm_add_epi32(in[7 + 8 * i], in[7 + 8 * i]);
    }
}

static void fidtx64x64_sse4_1(__m128i* input, __m128i* output, const int8_t cos_bit, const int8_t* stage_range) {
    (void)cos_bit;
    (void)stage_range;
    const int32_t bits     = 12; // new_sqrt2_bits = 12
    const int32_t sqrt     = 4 * 5793; // 4 * new_sqrt2
    const int32_t col_num  = 16;
    const __m128i newsqrt  = _mm_set1_epi32(sqrt);
    const __m128i rounding = _mm_set1_epi32(1 << (bits - 1));

    __m128i temp;
    int32_t num_iters = 64 * col_num;
    for (int32_t i = 0; i < num_iters; i++) {
        temp      = _mm_mullo_epi32(input[i], newsqrt);
        temp      = _mm_add_epi32(temp, rounding);
        output[i] = _mm_srai_epi32(temp, bits);
    }
}

static void fadst16x16_sse4_1(__m128i* in, __m128i* out, int bit, const int num_cols) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospi16  = _mm_set1_epi32(cospi[16]);
    const __m128i  cospim16 = _mm_set1_epi32(-cospi[16]);
    const __m128i  cospim48 = _mm_set1_epi32(-cospi[48]);
    const __m128i  cospi8   = _mm_set1_epi32(cospi[8]);
    const __m128i  cospi56  = _mm_set1_epi32(cospi[56]);
    const __m128i  cospim56 = _mm_set1_epi32(-cospi[56]);
    const __m128i  cospim8  = _mm_set1_epi32(-cospi[8]);
    const __m128i  cospi24  = _mm_set1_epi32(cospi[24]);
    const __m128i  cospim24 = _mm_set1_epi32(-cospi[24]);
    const __m128i  cospim40 = _mm_set1_epi32(-cospi[40]);
    const __m128i  cospi40  = _mm_set1_epi32(cospi[40]);
    const __m128i  cospi2   = _mm_set1_epi32(cospi[2]);
    const __m128i  cospi62  = _mm_set1_epi32(cospi[62]);
    const __m128i  cospim2  = _mm_set1_epi32(-cospi[2]);
    const __m128i  cospi10  = _mm_set1_epi32(cospi[10]);
    const __m128i  cospi54  = _mm_set1_epi32(cospi[54]);
    const __m128i  cospim10 = _mm_set1_epi32(-cospi[10]);
    const __m128i  cospi18  = _mm_set1_epi32(cospi[18]);
    const __m128i  cospi46  = _mm_set1_epi32(cospi[46]);
    const __m128i  cospim18 = _mm_set1_epi32(-cospi[18]);
    const __m128i  cospi26  = _mm_set1_epi32(cospi[26]);
    const __m128i  cospi38  = _mm_set1_epi32(cospi[38]);
    const __m128i  cospim26 = _mm_set1_epi32(-cospi[26]);
    const __m128i  cospi34  = _mm_set1_epi32(cospi[34]);
    const __m128i  cospi30  = _mm_set1_epi32(cospi[30]);
    const __m128i  cospim34 = _mm_set1_epi32(-cospi[34]);
    const __m128i  cospi42  = _mm_set1_epi32(cospi[42]);
    const __m128i  cospi22  = _mm_set1_epi32(cospi[22]);
    const __m128i  cospim42 = _mm_set1_epi32(-cospi[42]);
    const __m128i  cospi50  = _mm_set1_epi32(cospi[50]);
    const __m128i  cospi14  = _mm_set1_epi32(cospi[14]);
    const __m128i  cospim50 = _mm_set1_epi32(-cospi[50]);
    const __m128i  cospi58  = _mm_set1_epi32(cospi[58]);
    const __m128i  cospi6   = _mm_set1_epi32(cospi[6]);
    const __m128i  cospim58 = _mm_set1_epi32(-cospi[58]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    const __m128i  zero     = _mm_setzero_si128();

    __m128i u[16], v[16], x, y;
    int     col;

    for (col = 0; col < num_cols; ++col) {
        // stage 0
        // stage 1
        u[0]  = in[0 * num_cols + col];
        u[1]  = _mm_sub_epi32(zero, in[15 * num_cols + col]);
        u[2]  = _mm_sub_epi32(zero, in[7 * num_cols + col]);
        u[3]  = in[8 * num_cols + col];
        u[4]  = _mm_sub_epi32(zero, in[3 * num_cols + col]);
        u[5]  = in[12 * num_cols + col];
        u[6]  = in[4 * num_cols + col];
        u[7]  = _mm_sub_epi32(zero, in[11 * num_cols + col]);
        u[8]  = _mm_sub_epi32(zero, in[1 * num_cols + col]);
        u[9]  = in[14 * num_cols + col];
        u[10] = in[6 * num_cols + col];
        u[11] = _mm_sub_epi32(zero, in[9 * num_cols + col]);
        u[12] = in[2 * num_cols + col];
        u[13] = _mm_sub_epi32(zero, in[13 * num_cols + col]);
        u[14] = _mm_sub_epi32(zero, in[5 * num_cols + col]);
        u[15] = in[10 * num_cols + col];

        // stage 2
        v[0] = u[0];
        v[1] = u[1];

        x    = _mm_mullo_epi32(u[2], cospi32);
        y    = _mm_mullo_epi32(u[3], cospi32);
        v[2] = _mm_add_epi32(x, y);
        v[2] = _mm_add_epi32(v[2], rnding);
        v[2] = _mm_srai_epi32(v[2], bit);

        v[3] = _mm_sub_epi32(x, y);
        v[3] = _mm_add_epi32(v[3], rnding);
        v[3] = _mm_srai_epi32(v[3], bit);

        v[4] = u[4];
        v[5] = u[5];

        x    = _mm_mullo_epi32(u[6], cospi32);
        y    = _mm_mullo_epi32(u[7], cospi32);
        v[6] = _mm_add_epi32(x, y);
        v[6] = _mm_add_epi32(v[6], rnding);
        v[6] = _mm_srai_epi32(v[6], bit);

        v[7] = _mm_sub_epi32(x, y);
        v[7] = _mm_add_epi32(v[7], rnding);
        v[7] = _mm_srai_epi32(v[7], bit);

        v[8] = u[8];
        v[9] = u[9];

        x     = _mm_mullo_epi32(u[10], cospi32);
        y     = _mm_mullo_epi32(u[11], cospi32);
        v[10] = _mm_add_epi32(x, y);
        v[10] = _mm_add_epi32(v[10], rnding);
        v[10] = _mm_srai_epi32(v[10], bit);

        v[11] = _mm_sub_epi32(x, y);
        v[11] = _mm_add_epi32(v[11], rnding);
        v[11] = _mm_srai_epi32(v[11], bit);

        v[12] = u[12];
        v[13] = u[13];

        x     = _mm_mullo_epi32(u[14], cospi32);
        y     = _mm_mullo_epi32(u[15], cospi32);
        v[14] = _mm_add_epi32(x, y);
        v[14] = _mm_add_epi32(v[14], rnding);
        v[14] = _mm_srai_epi32(v[14], bit);

        v[15] = _mm_sub_epi32(x, y);
        v[15] = _mm_add_epi32(v[15], rnding);
        v[15] = _mm_srai_epi32(v[15], bit);

        // stage 3
        u[0]  = _mm_add_epi32(v[0], v[2]);
        u[1]  = _mm_add_epi32(v[1], v[3]);
        u[2]  = _mm_sub_epi32(v[0], v[2]);
        u[3]  = _mm_sub_epi32(v[1], v[3]);
        u[4]  = _mm_add_epi32(v[4], v[6]);
        u[5]  = _mm_add_epi32(v[5], v[7]);
        u[6]  = _mm_sub_epi32(v[4], v[6]);
        u[7]  = _mm_sub_epi32(v[5], v[7]);
        u[8]  = _mm_add_epi32(v[8], v[10]);
        u[9]  = _mm_add_epi32(v[9], v[11]);
        u[10] = _mm_sub_epi32(v[8], v[10]);
        u[11] = _mm_sub_epi32(v[9], v[11]);
        u[12] = _mm_add_epi32(v[12], v[14]);
        u[13] = _mm_add_epi32(v[13], v[15]);
        u[14] = _mm_sub_epi32(v[12], v[14]);
        u[15] = _mm_sub_epi32(v[13], v[15]);

        // stage 4
        v[0]  = u[0];
        v[1]  = u[1];
        v[2]  = u[2];
        v[3]  = u[3];
        v[4]  = half_btf_sse4_1(&cospi16, &u[4], &cospi48, &u[5], &rnding, bit);
        v[5]  = half_btf_sse4_1(&cospi48, &u[4], &cospim16, &u[5], &rnding, bit);
        v[6]  = half_btf_sse4_1(&cospim48, &u[6], &cospi16, &u[7], &rnding, bit);
        v[7]  = half_btf_sse4_1(&cospi16, &u[6], &cospi48, &u[7], &rnding, bit);
        v[8]  = u[8];
        v[9]  = u[9];
        v[10] = u[10];
        v[11] = u[11];
        v[12] = half_btf_sse4_1(&cospi16, &u[12], &cospi48, &u[13], &rnding, bit);
        v[13] = half_btf_sse4_1(&cospi48, &u[12], &cospim16, &u[13], &rnding, bit);
        v[14] = half_btf_sse4_1(&cospim48, &u[14], &cospi16, &u[15], &rnding, bit);
        v[15] = half_btf_sse4_1(&cospi16, &u[14], &cospi48, &u[15], &rnding, bit);

        // stage 5
        u[0]  = _mm_add_epi32(v[0], v[4]);
        u[1]  = _mm_add_epi32(v[1], v[5]);
        u[2]  = _mm_add_epi32(v[2], v[6]);
        u[3]  = _mm_add_epi32(v[3], v[7]);
        u[4]  = _mm_sub_epi32(v[0], v[4]);
        u[5]  = _mm_sub_epi32(v[1], v[5]);
        u[6]  = _mm_sub_epi32(v[2], v[6]);
        u[7]  = _mm_sub_epi32(v[3], v[7]);
        u[8]  = _mm_add_epi32(v[8], v[12]);
        u[9]  = _mm_add_epi32(v[9], v[13]);
        u[10] = _mm_add_epi32(v[10], v[14]);
        u[11] = _mm_add_epi32(v[11], v[15]);
        u[12] = _mm_sub_epi32(v[8], v[12]);
        u[13] = _mm_sub_epi32(v[9], v[13]);
        u[14] = _mm_sub_epi32(v[10], v[14]);
        u[15] = _mm_sub_epi32(v[11], v[15]);

        // stage 6
        v[0]  = u[0];
        v[1]  = u[1];
        v[2]  = u[2];
        v[3]  = u[3];
        v[4]  = u[4];
        v[5]  = u[5];
        v[6]  = u[6];
        v[7]  = u[7];
        v[8]  = half_btf_sse4_1(&cospi8, &u[8], &cospi56, &u[9], &rnding, bit);
        v[9]  = half_btf_sse4_1(&cospi56, &u[8], &cospim8, &u[9], &rnding, bit);
        v[10] = half_btf_sse4_1(&cospi40, &u[10], &cospi24, &u[11], &rnding, bit);
        v[11] = half_btf_sse4_1(&cospi24, &u[10], &cospim40, &u[11], &rnding, bit);
        v[12] = half_btf_sse4_1(&cospim56, &u[12], &cospi8, &u[13], &rnding, bit);
        v[13] = half_btf_sse4_1(&cospi8, &u[12], &cospi56, &u[13], &rnding, bit);
        v[14] = half_btf_sse4_1(&cospim24, &u[14], &cospi40, &u[15], &rnding, bit);
        v[15] = half_btf_sse4_1(&cospi40, &u[14], &cospi24, &u[15], &rnding, bit);

        // stage 7
        u[0]  = _mm_add_epi32(v[0], v[8]);
        u[1]  = _mm_add_epi32(v[1], v[9]);
        u[2]  = _mm_add_epi32(v[2], v[10]);
        u[3]  = _mm_add_epi32(v[3], v[11]);
        u[4]  = _mm_add_epi32(v[4], v[12]);
        u[5]  = _mm_add_epi32(v[5], v[13]);
        u[6]  = _mm_add_epi32(v[6], v[14]);
        u[7]  = _mm_add_epi32(v[7], v[15]);
        u[8]  = _mm_sub_epi32(v[0], v[8]);
        u[9]  = _mm_sub_epi32(v[1], v[9]);
        u[10] = _mm_sub_epi32(v[2], v[10]);
        u[11] = _mm_sub_epi32(v[3], v[11]);
        u[12] = _mm_sub_epi32(v[4], v[12]);
        u[13] = _mm_sub_epi32(v[5], v[13]);
        u[14] = _mm_sub_epi32(v[6], v[14]);
        u[15] = _mm_sub_epi32(v[7], v[15]);

        // stage 8
        v[0]  = half_btf_sse4_1(&cospi2, &u[0], &cospi62, &u[1], &rnding, bit);
        v[1]  = half_btf_sse4_1(&cospi62, &u[0], &cospim2, &u[1], &rnding, bit);
        v[2]  = half_btf_sse4_1(&cospi10, &u[2], &cospi54, &u[3], &rnding, bit);
        v[3]  = half_btf_sse4_1(&cospi54, &u[2], &cospim10, &u[3], &rnding, bit);
        v[4]  = half_btf_sse4_1(&cospi18, &u[4], &cospi46, &u[5], &rnding, bit);
        v[5]  = half_btf_sse4_1(&cospi46, &u[4], &cospim18, &u[5], &rnding, bit);
        v[6]  = half_btf_sse4_1(&cospi26, &u[6], &cospi38, &u[7], &rnding, bit);
        v[7]  = half_btf_sse4_1(&cospi38, &u[6], &cospim26, &u[7], &rnding, bit);
        v[8]  = half_btf_sse4_1(&cospi34, &u[8], &cospi30, &u[9], &rnding, bit);
        v[9]  = half_btf_sse4_1(&cospi30, &u[8], &cospim34, &u[9], &rnding, bit);
        v[10] = half_btf_sse4_1(&cospi42, &u[10], &cospi22, &u[11], &rnding, bit);
        v[11] = half_btf_sse4_1(&cospi22, &u[10], &cospim42, &u[11], &rnding, bit);
        v[12] = half_btf_sse4_1(&cospi50, &u[12], &cospi14, &u[13], &rnding, bit);
        v[13] = half_btf_sse4_1(&cospi14, &u[12], &cospim50, &u[13], &rnding, bit);
        v[14] = half_btf_sse4_1(&cospi58, &u[14], &cospi6, &u[15], &rnding, bit);
        v[15] = half_btf_sse4_1(&cospi6, &u[14], &cospim58, &u[15], &rnding, bit);

        // stage 9
        out[0 * num_cols + col]  = v[1];
        out[1 * num_cols + col]  = v[14];
        out[2 * num_cols + col]  = v[3];
        out[3 * num_cols + col]  = v[12];
        out[4 * num_cols + col]  = v[5];
        out[5 * num_cols + col]  = v[10];
        out[6 * num_cols + col]  = v[7];
        out[7 * num_cols + col]  = v[8];
        out[8 * num_cols + col]  = v[9];
        out[9 * num_cols + col]  = v[6];
        out[10 * num_cols + col] = v[11];
        out[11 * num_cols + col] = v[4];
        out[12 * num_cols + col] = v[13];
        out[13 * num_cols + col] = v[2];
        out[14 * num_cols + col] = v[15];
        out[15 * num_cols + col] = v[0];
    }
}

static void fadst8x8_sse4_1(__m128i* in, __m128i* out, int bit, const int col_num) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospi16  = _mm_set1_epi32(cospi[16]);
    const __m128i  cospim16 = _mm_set1_epi32(-cospi[16]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospim48 = _mm_set1_epi32(-cospi[48]);
    const __m128i  cospi4   = _mm_set1_epi32(cospi[4]);
    const __m128i  cospim4  = _mm_set1_epi32(-cospi[4]);
    const __m128i  cospi60  = _mm_set1_epi32(cospi[60]);
    const __m128i  cospi20  = _mm_set1_epi32(cospi[20]);
    const __m128i  cospim20 = _mm_set1_epi32(-cospi[20]);
    const __m128i  cospi44  = _mm_set1_epi32(cospi[44]);
    const __m128i  cospi28  = _mm_set1_epi32(cospi[28]);
    const __m128i  cospi36  = _mm_set1_epi32(cospi[36]);
    const __m128i  cospim36 = _mm_set1_epi32(-cospi[36]);
    const __m128i  cospi52  = _mm_set1_epi32(cospi[52]);
    const __m128i  cospim52 = _mm_set1_epi32(-cospi[52]);
    const __m128i  cospi12  = _mm_set1_epi32(cospi[12]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    const __m128i  zero     = _mm_setzero_si128();
    __m128i        u0, u1, u2, u3, u4, u5, u6, u7;
    __m128i        v0, v1, v2, v3, v4, v5, v6, v7;
    __m128i        x, y;
    int            col;

    // Note:
    //  Even column: 0, 2, ..., 14
    //  Odd column: 1, 3, ..., 15
    //  one even column plus one odd column constructs one row (8 coeffs)
    //  total we have 8 rows (8x8).
    for (col = 0; col < col_num; ++col) {
        // stage 0
        // stage 1
        u0 = in[col_num * 0 + col];
        u1 = _mm_sub_epi32(zero, in[col_num * 7 + col]);
        u2 = _mm_sub_epi32(zero, in[col_num * 3 + col]);
        u3 = in[col_num * 4 + col];
        u4 = _mm_sub_epi32(zero, in[col_num * 1 + col]);
        u5 = in[col_num * 6 + col];
        u6 = in[col_num * 2 + col];
        u7 = _mm_sub_epi32(zero, in[col_num * 5 + col]);

        // stage 2
        v0 = u0;
        v1 = u1;

        x  = _mm_mullo_epi32(u2, cospi32);
        y  = _mm_mullo_epi32(u3, cospi32);
        v2 = _mm_add_epi32(x, y);
        v2 = _mm_add_epi32(v2, rnding);
        v2 = _mm_srai_epi32(v2, bit);

        v3 = _mm_sub_epi32(x, y);
        v3 = _mm_add_epi32(v3, rnding);
        v3 = _mm_srai_epi32(v3, bit);

        v4 = u4;
        v5 = u5;

        x  = _mm_mullo_epi32(u6, cospi32);
        y  = _mm_mullo_epi32(u7, cospi32);
        v6 = _mm_add_epi32(x, y);
        v6 = _mm_add_epi32(v6, rnding);
        v6 = _mm_srai_epi32(v6, bit);

        v7 = _mm_sub_epi32(x, y);
        v7 = _mm_add_epi32(v7, rnding);
        v7 = _mm_srai_epi32(v7, bit);

        // stage 3
        u0 = _mm_add_epi32(v0, v2);
        u1 = _mm_add_epi32(v1, v3);
        u2 = _mm_sub_epi32(v0, v2);
        u3 = _mm_sub_epi32(v1, v3);
        u4 = _mm_add_epi32(v4, v6);
        u5 = _mm_add_epi32(v5, v7);
        u6 = _mm_sub_epi32(v4, v6);
        u7 = _mm_sub_epi32(v5, v7);

        // stage 4
        v0 = u0;
        v1 = u1;
        v2 = u2;
        v3 = u3;

        x  = _mm_mullo_epi32(u4, cospi16);
        y  = _mm_mullo_epi32(u5, cospi48);
        v4 = _mm_add_epi32(x, y);
        v4 = _mm_add_epi32(v4, rnding);
        v4 = _mm_srai_epi32(v4, bit);

        x  = _mm_mullo_epi32(u4, cospi48);
        y  = _mm_mullo_epi32(u5, cospim16);
        v5 = _mm_add_epi32(x, y);
        v5 = _mm_add_epi32(v5, rnding);
        v5 = _mm_srai_epi32(v5, bit);

        x  = _mm_mullo_epi32(u6, cospim48);
        y  = _mm_mullo_epi32(u7, cospi16);
        v6 = _mm_add_epi32(x, y);
        v6 = _mm_add_epi32(v6, rnding);
        v6 = _mm_srai_epi32(v6, bit);

        x  = _mm_mullo_epi32(u6, cospi16);
        y  = _mm_mullo_epi32(u7, cospi48);
        v7 = _mm_add_epi32(x, y);
        v7 = _mm_add_epi32(v7, rnding);
        v7 = _mm_srai_epi32(v7, bit);

        // stage 5
        u0 = _mm_add_epi32(v0, v4);
        u1 = _mm_add_epi32(v1, v5);
        u2 = _mm_add_epi32(v2, v6);
        u3 = _mm_add_epi32(v3, v7);
        u4 = _mm_sub_epi32(v0, v4);
        u5 = _mm_sub_epi32(v1, v5);
        u6 = _mm_sub_epi32(v2, v6);
        u7 = _mm_sub_epi32(v3, v7);

        // stage 6
        x  = _mm_mullo_epi32(u0, cospi4);
        y  = _mm_mullo_epi32(u1, cospi60);
        v0 = _mm_add_epi32(x, y);
        v0 = _mm_add_epi32(v0, rnding);
        v0 = _mm_srai_epi32(v0, bit);

        x  = _mm_mullo_epi32(u0, cospi60);
        y  = _mm_mullo_epi32(u1, cospim4);
        v1 = _mm_add_epi32(x, y);
        v1 = _mm_add_epi32(v1, rnding);
        v1 = _mm_srai_epi32(v1, bit);

        x  = _mm_mullo_epi32(u2, cospi20);
        y  = _mm_mullo_epi32(u3, cospi44);
        v2 = _mm_add_epi32(x, y);
        v2 = _mm_add_epi32(v2, rnding);
        v2 = _mm_srai_epi32(v2, bit);

        x  = _mm_mullo_epi32(u2, cospi44);
        y  = _mm_mullo_epi32(u3, cospim20);
        v3 = _mm_add_epi32(x, y);
        v3 = _mm_add_epi32(v3, rnding);
        v3 = _mm_srai_epi32(v3, bit);

        x  = _mm_mullo_epi32(u4, cospi36);
        y  = _mm_mullo_epi32(u5, cospi28);
        v4 = _mm_add_epi32(x, y);
        v4 = _mm_add_epi32(v4, rnding);
        v4 = _mm_srai_epi32(v4, bit);

        x  = _mm_mullo_epi32(u4, cospi28);
        y  = _mm_mullo_epi32(u5, cospim36);
        v5 = _mm_add_epi32(x, y);
        v5 = _mm_add_epi32(v5, rnding);
        v5 = _mm_srai_epi32(v5, bit);

        x  = _mm_mullo_epi32(u6, cospi52);
        y  = _mm_mullo_epi32(u7, cospi12);
        v6 = _mm_add_epi32(x, y);
        v6 = _mm_add_epi32(v6, rnding);
        v6 = _mm_srai_epi32(v6, bit);

        x  = _mm_mullo_epi32(u6, cospi12);
        y  = _mm_mullo_epi32(u7, cospim52);
        v7 = _mm_add_epi32(x, y);
        v7 = _mm_add_epi32(v7, rnding);
        v7 = _mm_srai_epi32(v7, bit);

        // stage 7
        out[col_num * 0 + col] = v1;
        out[col_num * 1 + col] = v6;
        out[col_num * 2 + col] = v3;
        out[col_num * 3 + col] = v4;
        out[col_num * 4 + col] = v5;
        out[col_num * 5 + col] = v2;
        out[col_num * 6 + col] = v7;
        out[col_num * 7 + col] = v0;
    }
}

static INLINE TxfmFuncSSE2 fwd_txfm_type_to_func_sse4(TxfmType txfm_type) {
    switch (txfm_type) {
    case TXFM_TYPE_DCT32:
        return fdct32_sse4_1;
        break;
    case TXFM_TYPE_DCT64:
        return fdct64_new_sse4_1;
        break;
    case TXFM_TYPE_IDENTITY32:
        return idtx32x32_sse4_1;
        break;
    case TXFM_TYPE_IDENTITY64:
        return fidtx64x64_sse4_1;
        break;
    default:
        assert(0);
    }
    return NULL;
}

//TODO: write sse code here
static INLINE void int16_array_with_stride_to_int32_array_without_stride(const int16_t* input, int stride,
                                                                         int32_t* output, int txfm1d_size) {
    int r, c;
    for (r = 0; r < txfm1d_size; r++) {
        for (c = 0; c < txfm1d_size; c++) {
            output[r * txfm1d_size + c] = (int32_t)input[r * stride + c];
        }
    }
}

static INLINE void load_buffer_64x64_sse4_1(const int16_t* input, int32_t stride, __m128i* output) {
    int32_t i;

    for (i = 0; i < 64; ++i) {
        output[0]  = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 0 * 4)));
        output[1]  = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 1 * 4)));
        output[2]  = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 2 * 4)));
        output[3]  = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 3 * 4)));
        output[4]  = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 4 * 4)));
        output[5]  = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 5 * 4)));
        output[6]  = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 6 * 4)));
        output[7]  = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 7 * 4)));
        output[8]  = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 8 * 4)));
        output[9]  = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 9 * 4)));
        output[10] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 10 * 4)));
        output[11] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 11 * 4)));
        output[12] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 12 * 4)));
        output[13] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 13 * 4)));
        output[14] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 14 * 4)));
        output[15] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 15 * 4)));

        input += stride;
        output += 16;
    }
}

static INLINE void flip_buf_sse4_1(__m128i* in, __m128i* out, int size) {
    for (int i = 0; i < size; i += 2) {
        in[30 - i] = out[i];
    }
    for (int i = 1; i < size; i += 2) {
        in[size - i] = out[i];
    }
}

static INLINE void convert_8x8_to_16x16(const __m128i* in, __m128i* out) {
    int row_index = 0;
    int dst_index = 0;
    int src_index = 0;

    // row 0, 1, .., 7
    do {
        out[dst_index]     = in[src_index];
        out[dst_index + 1] = in[src_index + 1];
        out[dst_index + 2] = in[src_index + 16];
        out[dst_index + 3] = in[src_index + 17];
        dst_index += 4;
        src_index += 2;
        row_index += 1;
    } while (row_index < 8);

    // row 8, 9, ..., 15
    src_index += 16;
    do {
        out[dst_index]     = in[src_index];
        out[dst_index + 1] = in[src_index + 1];
        out[dst_index + 2] = in[src_index + 16];
        out[dst_index + 3] = in[src_index + 17];
        dst_index += 4;
        src_index += 2;
        row_index += 1;
    } while (row_index < 16);
}

static INLINE void fwd_txfm2d_sse4_1(const int16_t* input, int32_t* output, const int stride, const Txfm2dFlipCfg* cfg,
                                     int32_t* txfm_buf) {
    // TODO(sarahparker) This does not currently support rectangular transforms
    // and will break without splitting txfm_size out into row and col size.
    // Rectangular transforms use c code only, so it should be ok for now.
    // It will be corrected when there are sse implementations for rectangular
    // transforms.
    assert(cfg->tx_size < TX_SIZES);
    const int          txfm_size       = tx_size_wide[cfg->tx_size];
    const int8_t*      shift           = cfg->shift;
    const int8_t*      stage_range_col = cfg->stage_range_col;
    const int8_t*      stage_range_row = cfg->stage_range_row;
    const int8_t       cos_bit_col     = cfg->cos_bit_col;
    const int8_t       cos_bit_row     = cfg->cos_bit_row;
    const TxfmFuncSSE2 txfm_func_col   = fwd_txfm_type_to_func_sse4(cfg->txfm_type_col);
    const TxfmFuncSSE2 txfm_func_row   = fwd_txfm_type_to_func_sse4(cfg->txfm_type_row);
    ASSERT(txfm_func_col);
    ASSERT(txfm_func_row);
    __m128i* buf_128         = (__m128i*)txfm_buf;
    __m128i* out_128         = (__m128i*)output;
    int      num_per_128     = 4;
    int      txfm2d_size_128 = txfm_size * txfm_size / num_per_128;

    int16_array_with_stride_to_int32_array_without_stride(input, stride, txfm_buf, txfm_size);
    av1_round_shift_array_32_sse4_1(buf_128, out_128, txfm2d_size_128, -shift[0]);
    txfm_func_col(out_128, buf_128, cos_bit_col, stage_range_col);
    av1_round_shift_array_32_sse4_1(buf_128, out_128, txfm2d_size_128, -shift[1]);
    transpose_32(txfm_size, out_128, buf_128);
    txfm_func_row(buf_128, out_128, cos_bit_row, stage_range_row);
    av1_round_shift_array_32_sse4_1(out_128, buf_128, txfm2d_size_128, -shift[2]);
    transpose_32(txfm_size, buf_128, out_128);
}

static INLINE void load_buffer_32x8n(const int16_t* input, __m128i* out, int stride, int flipud, int fliplr, int shift,
                                     const int height) {
    const int16_t* in     = input;
    __m128i*       output = out;
    for (int col = 0; col < height; col++) {
        in     = input + col * stride;
        output = out + col * 8;
        load_buffer_4x4(in, output, 4, flipud, fliplr, shift);
        load_buffer_4x4((in + 16), (output + 4), 4, flipud, fliplr, shift);
    }
}

static INLINE void load_buffer_8x8(const int16_t* input, __m128i* in, int stride, int flipud, int fliplr, int shift) {
    __m128i u;
    if (!flipud) {
        in[0] = _mm_loadu_si128((const __m128i*)(input + 0 * stride));
        in[1] = _mm_loadu_si128((const __m128i*)(input + 1 * stride));
        in[2] = _mm_loadu_si128((const __m128i*)(input + 2 * stride));
        in[3] = _mm_loadu_si128((const __m128i*)(input + 3 * stride));
        in[4] = _mm_loadu_si128((const __m128i*)(input + 4 * stride));
        in[5] = _mm_loadu_si128((const __m128i*)(input + 5 * stride));
        in[6] = _mm_loadu_si128((const __m128i*)(input + 6 * stride));
        in[7] = _mm_loadu_si128((const __m128i*)(input + 7 * stride));
    } else {
        in[0] = _mm_loadu_si128((const __m128i*)(input + 7 * stride));
        in[1] = _mm_loadu_si128((const __m128i*)(input + 6 * stride));
        in[2] = _mm_loadu_si128((const __m128i*)(input + 5 * stride));
        in[3] = _mm_loadu_si128((const __m128i*)(input + 4 * stride));
        in[4] = _mm_loadu_si128((const __m128i*)(input + 3 * stride));
        in[5] = _mm_loadu_si128((const __m128i*)(input + 2 * stride));
        in[6] = _mm_loadu_si128((const __m128i*)(input + 1 * stride));
        in[7] = _mm_loadu_si128((const __m128i*)(input + 0 * stride));
    }

    if (fliplr) {
        in[0] = mm_reverse_epi16(in[0]);
        in[1] = mm_reverse_epi16(in[1]);
        in[2] = mm_reverse_epi16(in[2]);
        in[3] = mm_reverse_epi16(in[3]);
        in[4] = mm_reverse_epi16(in[4]);
        in[5] = mm_reverse_epi16(in[5]);
        in[6] = mm_reverse_epi16(in[6]);
        in[7] = mm_reverse_epi16(in[7]);
    }

    u     = _mm_unpackhi_epi64(in[4], in[4]);
    in[8] = _mm_cvtepi16_epi32(in[4]);
    in[9] = _mm_cvtepi16_epi32(u);

    u      = _mm_unpackhi_epi64(in[5], in[5]);
    in[10] = _mm_cvtepi16_epi32(in[5]);
    in[11] = _mm_cvtepi16_epi32(u);

    u      = _mm_unpackhi_epi64(in[6], in[6]);
    in[12] = _mm_cvtepi16_epi32(in[6]);
    in[13] = _mm_cvtepi16_epi32(u);

    u      = _mm_unpackhi_epi64(in[7], in[7]);
    in[14] = _mm_cvtepi16_epi32(in[7]);
    in[15] = _mm_cvtepi16_epi32(u);

    u     = _mm_unpackhi_epi64(in[3], in[3]);
    in[6] = _mm_cvtepi16_epi32(in[3]);
    in[7] = _mm_cvtepi16_epi32(u);

    u     = _mm_unpackhi_epi64(in[2], in[2]);
    in[4] = _mm_cvtepi16_epi32(in[2]);
    in[5] = _mm_cvtepi16_epi32(u);

    u     = _mm_unpackhi_epi64(in[1], in[1]);
    in[2] = _mm_cvtepi16_epi32(in[1]);
    in[3] = _mm_cvtepi16_epi32(u);

    u     = _mm_unpackhi_epi64(in[0], in[0]);
    in[0] = _mm_cvtepi16_epi32(in[0]);
    in[1] = _mm_cvtepi16_epi32(u);

    in[0] = _mm_slli_epi32(in[0], shift);
    in[1] = _mm_slli_epi32(in[1], shift);
    in[2] = _mm_slli_epi32(in[2], shift);
    in[3] = _mm_slli_epi32(in[3], shift);
    in[4] = _mm_slli_epi32(in[4], shift);
    in[5] = _mm_slli_epi32(in[5], shift);
    in[6] = _mm_slli_epi32(in[6], shift);
    in[7] = _mm_slli_epi32(in[7], shift);

    in[8]  = _mm_slli_epi32(in[8], shift);
    in[9]  = _mm_slli_epi32(in[9], shift);
    in[10] = _mm_slli_epi32(in[10], shift);
    in[11] = _mm_slli_epi32(in[11], shift);
    in[12] = _mm_slli_epi32(in[12], shift);
    in[13] = _mm_slli_epi32(in[13], shift);
    in[14] = _mm_slli_epi32(in[14], shift);
    in[15] = _mm_slli_epi32(in[15], shift);
}

static INLINE void load_buffer_16x16(const int16_t* input, __m128i* out, int stride, int flipud, int fliplr,
                                     int shift) {
    __m128i in[64];
    // Load 4 8x8 blocks
    const int16_t* topL = input;
    const int16_t* topR = input + 8;
    const int16_t* botL = input + 8 * stride;
    const int16_t* botR = input + 8 * stride + 8;

    const int16_t* tmp;

    if (flipud) {
        // Swap left columns
        tmp  = topL;
        topL = botL;
        botL = tmp;
        // Swap right columns
        tmp  = topR;
        topR = botR;
        botR = tmp;
    }

    if (fliplr) {
        // Swap top rows
        tmp  = topL;
        topL = topR;
        topR = tmp;
        // Swap bottom rows
        tmp  = botL;
        botL = botR;
        botR = tmp;
    }

    // load first 8 columns
    load_buffer_8x8(topL, &in[0], stride, flipud, fliplr, shift);
    load_buffer_8x8(botL, &in[32], stride, flipud, fliplr, shift);

    // load second 8 columns
    load_buffer_8x8(topR, &in[16], stride, flipud, fliplr, shift);
    load_buffer_8x8(botR, &in[48], stride, flipud, fliplr, shift);

    convert_8x8_to_16x16(in, out);
}

static INLINE void load_buffer_8x4(const int16_t* input, __m128i* out, int stride, int flipud, int fliplr, int shift) {
    const int16_t* topL = input;
    const int16_t* topR = input + 4;

    const int16_t* tmp;

    if (fliplr) {
        tmp  = topL;
        topL = topR;
        topR = tmp;
    }

    load_buffer_4x4(topL, out, stride, flipud, fliplr, shift);
    load_buffer_4x4(topR, out + 4, stride, flipud, fliplr, shift);
}

static INLINE void load_buffer_16x4(const int16_t* input, __m128i* out, int stride, int flipud, int fliplr, int shift) {
    const int16_t* topL = input;
    const int16_t* topR = input + 8;

    const int16_t* tmp;

    if (fliplr) {
        tmp  = topL;
        topL = topR;
        topR = tmp;
    }

    load_buffer_8x4(topL, out, stride, flipud, fliplr, shift);
    load_buffer_8x4(topR, out + 8, stride, flipud, fliplr, shift);
}

static INLINE void load_buffer_8x16(const int16_t* input, __m128i* out, int stride, int flipud, int fliplr, int shift) {
    const int16_t* topL = input;
    const int16_t* botL = input + 8 * stride;

    const int16_t* tmp;

    if (flipud) {
        tmp  = topL;
        topL = botL;
        botL = tmp;
    }

    load_buffer_8x8(topL, out, stride, flipud, fliplr, shift);
    load_buffer_8x8(botL, out + 16, stride, flipud, fliplr, shift);
}

static INLINE void load_buffer_4x8(const int16_t* input, __m128i* out, int stride, int flipud, int fliplr, int shift) {
    const int16_t* topL = input;
    const int16_t* botL = input + 4 * stride;

    const int16_t* tmp;

    if (flipud) {
        tmp  = topL;
        topL = botL;
        botL = tmp;
    }

    load_buffer_4x4(topL, out, stride, flipud, fliplr, shift);
    load_buffer_4x4(botL, out + 4, stride, flipud, fliplr, shift);
}

static INLINE void load_buffer_4x16(const int16_t* input, __m128i* out, const int stride, const int flipud,
                                    const int fliplr, const int shift) {
    const int16_t* topL = input;
    const int16_t* botL = input + 8 * stride;

    const int16_t* tmp;

    if (flipud) {
        tmp  = topL;
        topL = botL;
        botL = tmp;
    }
    load_buffer_4x8(topL, out, stride, flipud, fliplr, shift);
    load_buffer_4x8(botL, out + 8, stride, flipud, fliplr, shift);
}

static INLINE void col_txfm_8x8_rounding(__m128i* in, int shift) {
    const __m128i rounding = _mm_set1_epi32(1 << (shift - 1));

    in[0]  = _mm_add_epi32(in[0], rounding);
    in[1]  = _mm_add_epi32(in[1], rounding);
    in[2]  = _mm_add_epi32(in[2], rounding);
    in[3]  = _mm_add_epi32(in[3], rounding);
    in[4]  = _mm_add_epi32(in[4], rounding);
    in[5]  = _mm_add_epi32(in[5], rounding);
    in[6]  = _mm_add_epi32(in[6], rounding);
    in[7]  = _mm_add_epi32(in[7], rounding);
    in[8]  = _mm_add_epi32(in[8], rounding);
    in[9]  = _mm_add_epi32(in[9], rounding);
    in[10] = _mm_add_epi32(in[10], rounding);
    in[11] = _mm_add_epi32(in[11], rounding);
    in[12] = _mm_add_epi32(in[12], rounding);
    in[13] = _mm_add_epi32(in[13], rounding);
    in[14] = _mm_add_epi32(in[14], rounding);
    in[15] = _mm_add_epi32(in[15], rounding);

    in[0]  = _mm_srai_epi32(in[0], shift);
    in[1]  = _mm_srai_epi32(in[1], shift);
    in[2]  = _mm_srai_epi32(in[2], shift);
    in[3]  = _mm_srai_epi32(in[3], shift);
    in[4]  = _mm_srai_epi32(in[4], shift);
    in[5]  = _mm_srai_epi32(in[5], shift);
    in[6]  = _mm_srai_epi32(in[6], shift);
    in[7]  = _mm_srai_epi32(in[7], shift);
    in[8]  = _mm_srai_epi32(in[8], shift);
    in[9]  = _mm_srai_epi32(in[9], shift);
    in[10] = _mm_srai_epi32(in[10], shift);
    in[11] = _mm_srai_epi32(in[11], shift);
    in[12] = _mm_srai_epi32(in[12], shift);
    in[13] = _mm_srai_epi32(in[13], shift);
    in[14] = _mm_srai_epi32(in[14], shift);
    in[15] = _mm_srai_epi32(in[15], shift);
}

static void col_txfm_16x16_rounding(__m128i* in, int shift) {
    // Note:
    //  We split 16x16 rounding into 4 sections of 8x8 rounding,
    //  instead of 4 columns
    col_txfm_8x8_rounding(&in[0], shift);
    col_txfm_8x8_rounding(&in[16], shift);
    col_txfm_8x8_rounding(&in[32], shift);
    col_txfm_8x8_rounding(&in[48], shift);
}

static void col_txfm_8x16_rounding(__m128i* in, int shift) {
    col_txfm_8x8_rounding(&in[0], shift);
    col_txfm_8x8_rounding(&in[16], shift);
}

static INLINE void col_txfm_4x8_rounding(__m128i* in, int shift) {
    const __m128i rounding = _mm_set1_epi32(1 << (shift - 1));

    in[0] = _mm_add_epi32(in[0], rounding);
    in[1] = _mm_add_epi32(in[1], rounding);
    in[2] = _mm_add_epi32(in[2], rounding);
    in[3] = _mm_add_epi32(in[3], rounding);
    in[4] = _mm_add_epi32(in[4], rounding);
    in[5] = _mm_add_epi32(in[5], rounding);
    in[6] = _mm_add_epi32(in[6], rounding);
    in[7] = _mm_add_epi32(in[7], rounding);

    in[0] = _mm_srai_epi32(in[0], shift);
    in[1] = _mm_srai_epi32(in[1], shift);
    in[2] = _mm_srai_epi32(in[2], shift);
    in[3] = _mm_srai_epi32(in[3], shift);
    in[4] = _mm_srai_epi32(in[4], shift);
    in[5] = _mm_srai_epi32(in[5], shift);
    in[6] = _mm_srai_epi32(in[6], shift);
    in[7] = _mm_srai_epi32(in[7], shift);
}

static INLINE void write_buffer_8x8(const __m128i* res, int32_t* output) {
    _mm_storeu_si128((__m128i*)(output + 0 * 4), res[0]);
    _mm_storeu_si128((__m128i*)(output + 1 * 4), res[1]);
    _mm_storeu_si128((__m128i*)(output + 2 * 4), res[2]);
    _mm_storeu_si128((__m128i*)(output + 3 * 4), res[3]);

    _mm_storeu_si128((__m128i*)(output + 4 * 4), res[4]);
    _mm_storeu_si128((__m128i*)(output + 5 * 4), res[5]);
    _mm_storeu_si128((__m128i*)(output + 6 * 4), res[6]);
    _mm_storeu_si128((__m128i*)(output + 7 * 4), res[7]);

    _mm_storeu_si128((__m128i*)(output + 8 * 4), res[8]);
    _mm_storeu_si128((__m128i*)(output + 9 * 4), res[9]);
    _mm_storeu_si128((__m128i*)(output + 10 * 4), res[10]);
    _mm_storeu_si128((__m128i*)(output + 11 * 4), res[11]);

    _mm_storeu_si128((__m128i*)(output + 12 * 4), res[12]);
    _mm_storeu_si128((__m128i*)(output + 13 * 4), res[13]);
    _mm_storeu_si128((__m128i*)(output + 14 * 4), res[14]);
    _mm_storeu_si128((__m128i*)(output + 15 * 4), res[15]);
}

static INLINE void write_buffer_16x8(const __m128i* res, int32_t* output, const int stride) {
    _mm_storeu_si128((__m128i*)(output), res[0]);
    _mm_storeu_si128((__m128i*)(output + 4), res[1]);
    _mm_storeu_si128((__m128i*)(output + stride), res[2]);
    _mm_storeu_si128((__m128i*)(output + stride + 4), res[3]);

    _mm_storeu_si128((__m128i*)(output + (stride * 2)), res[4]);
    _mm_storeu_si128((__m128i*)(output + (stride * 2) + 4), res[5]);
    _mm_storeu_si128((__m128i*)(output + (stride * 3)), res[6]);
    _mm_storeu_si128((__m128i*)(output + (stride * 3) + 4), res[7]);

    _mm_storeu_si128((__m128i*)(output + (stride * 4)), res[8]);
    _mm_storeu_si128((__m128i*)(output + (stride * 4) + 4), res[9]);
    _mm_storeu_si128((__m128i*)(output + (stride * 5)), res[10]);
    _mm_storeu_si128((__m128i*)(output + (stride * 5) + 4), res[11]);

    _mm_storeu_si128((__m128i*)(output + (stride * 6)), res[12]);
    _mm_storeu_si128((__m128i*)(output + (stride * 6) + 4), res[13]);
    _mm_storeu_si128((__m128i*)(output + (stride * 7)), res[14]);
    _mm_storeu_si128((__m128i*)(output + (stride * 7) + 4), res[15]);
}

static void write_buffer_16x16(const __m128i* in, int32_t* output) {
    const int size_8x8 = 16 * 4;
    write_buffer_8x8(&in[0], output);
    output += size_8x8;
    write_buffer_8x8(&in[16], output);
    output += size_8x8;
    write_buffer_8x8(&in[32], output);
    output += size_8x8;
    write_buffer_8x8(&in[48], output);
}

static const fwd_transform_1d_sse4_1 col_highbd_txfm8x32_arr[TX_TYPES] = {
    svt_av1_fdct32_sse4_1, // DCT_DCT
    NULL, // ADST_DCT
    NULL, // DCT_ADST
    NULL, // ADST_ADST
    NULL, // FLIPADST_DCT
    NULL, // DCT_FLIPADST
    NULL, // FLIPADST_FLIPADST
    NULL, // ADST_FLIPADST
    NULL, // FLIPADST_ADST
    svt_av1_idtx32_sse4_1, // IDTX
    NULL, // V_DCT
    NULL, // H_DCT
    NULL, // V_ADST
    NULL, // H_ADST
    NULL, // V_FLIPADST
    NULL // H_FLIPADST
};

static const fwd_transform_1d_sse4_1 col_highbd_txfm8x8_arr[TX_TYPES] = {
    fdct8x8_sse4_1, // DCT_DCT
    fadst8x8_sse4_1, // ADST_DCT
    fdct8x8_sse4_1, // DCT_ADST
    fadst8x8_sse4_1, // ADST_ADST
    fadst8x8_sse4_1, // FLIPADST_DCT
    fdct8x8_sse4_1, // DCT_FLIPADST
    fadst8x8_sse4_1, // FLIPADST_FLIPADST
    fadst8x8_sse4_1, // ADST_FLIPADST
    fadst8x8_sse4_1, // FLIPADST_ADST
    idtx8x8_sse4_1, // IDTX
    fdct8x8_sse4_1, // V_DCT
    idtx8x8_sse4_1, // H_DCT
    fadst8x8_sse4_1, // V_ADST
    idtx8x8_sse4_1, // H_ADST
    fadst8x8_sse4_1, // V_FLIPADST
    idtx8x8_sse4_1 // H_FLIPADST
};

static const fwd_transform_1d_sse4_1 col_highbd_txfm4x4_arr[TX_TYPES] = {
    fdct4x4_sse4_1, // DCT_DCT
    fadst4x4_sse4_1, // ADST_DCT
    fdct4x4_sse4_1, // DCT_ADST
    fadst4x4_sse4_1, // ADST_ADST
    fadst4x4_sse4_1, // FLIPADST_DCT
    fdct4x4_sse4_1, // DCT_FLIPADST
    fadst4x4_sse4_1, // FLIPADST_FLIPADST
    fadst4x4_sse4_1, // ADST_FLIPADST
    fadst4x4_sse4_1, // FLIPADST_ADST
    fidtx4x4_sse4_1, // IDTX
    fdct4x4_sse4_1, // V_DCT
    fidtx4x4_sse4_1, // H_DCT
    fadst4x4_sse4_1, // V_ADST
    fidtx4x4_sse4_1, // H_ADST
    fadst4x4_sse4_1, // V_FLIPADST
    fidtx4x4_sse4_1 // H_FLIPADST
};

static const fwd_transform_1d_sse4_1 col_highbd_txfm8x16_arr[TX_TYPES] = {
    fdct16x16_sse4_1, // DCT_DCT
    fadst16x16_sse4_1, // ADST_DCT
    fdct16x16_sse4_1, // DCT_ADST
    fadst16x16_sse4_1, // ADST_ADST
    fadst16x16_sse4_1, // FLIPADST_DCT
    fdct16x16_sse4_1, // DCT_FLIPADST
    fadst16x16_sse4_1, // FLIPADST_FLIPADST
    fadst16x16_sse4_1, // ADST_FLIPADST
    fadst16x16_sse4_1, // FLIPADST_ADST
    idtx16x16_sse4_1, // IDTX
    fdct16x16_sse4_1, // V_DCT
    idtx16x16_sse4_1, // H_DCT
    fadst16x16_sse4_1, // V_ADST
    idtx16x16_sse4_1, // H_ADST
    fadst16x16_sse4_1, // V_FLIPADST
    idtx16x16_sse4_1 // H_FLIPADST
};

static const fwd_transform_1d_sse4_1 col_highbd_txfm4x8_arr[TX_TYPES] = {
    fdct4x8_sse4_1, // DCT_DCT
    fadst8x8_sse4_1, // ADST_DCT
    fdct4x8_sse4_1, // DCT_ADST
    fadst8x8_sse4_1, // ADST_ADST
    fadst8x8_sse4_1, // FLIPADST_DCT
    fdct4x8_sse4_1, // DCT_FLIPADST
    fadst8x8_sse4_1, // FLIPADST_FLIPADST
    fadst8x8_sse4_1, // ADST_FLIPADST
    fadst8x8_sse4_1, // FLIPADST_ADST
    idtx8x8_sse4_1, // IDTX
    fdct4x8_sse4_1, // V_DCT
    idtx8x8_sse4_1, // H_DCT
    fadst8x8_sse4_1, // V_ADST
    idtx8x8_sse4_1, // H_ADST
    fadst8x8_sse4_1, // V_FLIPADST
    idtx8x8_sse4_1 // H_FLIPADST
};

static const fwd_transform_1d_sse4_1 row_highbd_txfm8x32_arr[TX_TYPES] = {
    fdct16x16_sse4_1, // DCT_DCT
    NULL, // ADST_DCT
    NULL, // DCT_ADST
    NULL, // ADST_ADST
    NULL, // FLIPADST_DCT
    NULL, // DCT_FLIPADST
    NULL, // FLIPADST_FLIPADST
    NULL, // ADST_FLIPADST
    NULL, // FLIPADST_ADST
    idtx16x16_sse4_1, // IDTX
    NULL, // V_DCT
    NULL, // H_DCT
    NULL, // V_ADST
    NULL, // H_ADST
    NULL, // V_FLIPADST
    NULL // H_FLIPADST
};

static const fwd_transform_1d_sse4_1 row_highbd_txfm32x8_arr[TX_TYPES] = {
    fdct8x8_sse4_1, // DCT_DCT
    NULL, // ADST_DCT
    NULL, // DCT_ADST
    NULL, // ADST_ADST
    NULL, // FLIPADST_DCT
    NULL, // DCT_FLIPADST
    NULL, // FLIPADST_FLIPADST
    NULL, // ADST_FLIPADST
    NULL, // FLIPADST-ADST
    idtx32x8_sse4_1, // IDTX
    NULL, // V_DCT
    NULL, // H_DCT
    NULL, // V_ADST
    NULL, // H_ADST
    NULL, // V_FLIPADST
    NULL, // H_FLIPADST
};

static const fwd_transform_1d_sse4_1 row_highbd_txfm8x16_arr[TX_TYPES] = {
    fdct16x16_sse4_1, // DCT_DCT
    fdct16x16_sse4_1, // ADST_DCT
    fadst16x16_sse4_1, // DCT_ADST
    fadst16x16_sse4_1, // ADST_ADST
    fdct16x16_sse4_1, // FLIPADST_DCT
    fadst16x16_sse4_1, // DCT_FLIPADST
    fadst16x16_sse4_1, // FLIPADST_FLIPADST
    fadst16x16_sse4_1, // ADST_FLIPADST
    fadst16x16_sse4_1, // FLIPADST_ADST
    idtx16x16_sse4_1, // IDTX
    idtx16x16_sse4_1, // V_DCT
    fdct16x16_sse4_1, // H_DCT
    idtx16x16_sse4_1, // V_ADST
    fadst16x16_sse4_1, // H_ADST
    idtx16x16_sse4_1, // V_FLIPADST
    fadst16x16_sse4_1 // H_FLIPADST
};

static const fwd_transform_1d_sse4_1 row_highbd_txfm8x8_arr[TX_TYPES] = {
    fdct8x8_sse4_1, // DCT_DCT
    fdct8x8_sse4_1, // ADST_DCT
    fadst8x8_sse4_1, // DCT_ADST
    fadst8x8_sse4_1, // ADST_ADST
    fdct8x8_sse4_1, // FLIPADST_DCT
    fadst8x8_sse4_1, // DCT_FLIPADST
    fadst8x8_sse4_1, // FLIPADST_FLIPADST
    fadst8x8_sse4_1, // ADST_FLIPADST
    fadst8x8_sse4_1, // FLIPADST_ADST
    idtx8x8_sse4_1, // IDTX
    idtx8x8_sse4_1, // V_DCT
    fdct8x8_sse4_1, // H_DCT
    idtx8x8_sse4_1, // V_ADST
    fadst8x8_sse4_1, // H_ADST
    idtx8x8_sse4_1, // V_FLIPADST
    fadst8x8_sse4_1 // H_FLIPADST
};

static const fwd_transform_1d_sse4_1 row_highbd_txfm4x8_arr[TX_TYPES] = {
    fdct4x8_sse4_1, // DCT_DCT
    fdct4x8_sse4_1, // ADST_DCT
    fadst8x8_sse4_1, // DCT_ADST
    fadst8x8_sse4_1, // ADST_ADST
    fdct4x8_sse4_1, // FLIPADST_DCT
    fadst8x8_sse4_1, // DCT_FLIPADST
    fadst8x8_sse4_1, // FLIPADST_FLIPADST
    fadst8x8_sse4_1, // ADST_FLIPADST
    fadst8x8_sse4_1, // FLIPADST_ADST
    idtx8x8_sse4_1, // IDTX
    idtx8x8_sse4_1, // V_DCT
    fdct4x8_sse4_1, // H_DCT
    idtx8x8_sse4_1, // V_ADST
    fadst8x8_sse4_1, // H_ADST
    idtx8x8_sse4_1, // V_FLIPADST
    fadst8x8_sse4_1 // H_FLIPADST
};

static const fwd_transform_1d_sse4_1 row_highbd_txfm4x4_arr[TX_TYPES] = {
    fdct4x4_sse4_1, // DCT_DCT
    fdct4x4_sse4_1, // ADST_DCT
    fadst4x4_sse4_1, // DCT_ADST
    fadst4x4_sse4_1, // ADST_ADST
    fdct4x4_sse4_1, // FLIPADST_DCT
    fadst4x4_sse4_1, // DCT_FLIPADST
    fadst4x4_sse4_1, // FLIPADST_FLIPADST
    fadst4x4_sse4_1, // ADST_FLIPADST
    fadst4x4_sse4_1, // FLIPADST_ADST
    fidtx4x4_sse4_1, // IDTX
    fidtx4x4_sse4_1, // V_DCT
    fdct4x4_sse4_1, // H_DCT
    fidtx4x4_sse4_1, // V_ADST
    fadst4x4_sse4_1, // H_ADST
    fidtx4x4_sse4_1, // V_FLIPADST
    fadst4x4_sse4_1 // H_FLIPADST
};

void svt_av1_fwd_txfm2d_64x64_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    DECLARE_ALIGNED(16, int32_t, txfm_buf[4096]);
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(tx_type, TX_64X64, &cfg);
    (void)bd;
    const int     txfm_size       = tx_size_wide[cfg.tx_size];
    const int8_t* shift           = cfg.shift;
    const int8_t* stage_range_col = cfg.stage_range_col;
    const int8_t  cos_bit_col     = cfg.cos_bit_col;
    const int8_t  cos_bit_row     = cfg.cos_bit_row;

    __m128i* buf_128 = (__m128i*)txfm_buf;
    __m128i* out_128 = (__m128i*)output;

    const int num_per_128     = 4;
    int       txfm2d_size_128 = txfm_size * txfm_size / num_per_128;
    int       col_num         = txfm_size / num_per_128;

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_64x64_sse4_1(input, stride, out_128);
        /*col wise transform*/
        fdct64_new_sse4_1(out_128, buf_128, cos_bit_col, stage_range_col);
        av1_round_shift_array_32_sse4_1(buf_128, out_128, txfm2d_size_128, -shift[1]);
        transpose_32(txfm_size, out_128, buf_128);

        /*row wise transform*/
        for (int col = 0; col < col_num; col++) {
            svt_av1_fdct64_sse4_1((buf_128 + col), (out_128 + col), cos_bit_row, col_num, col_num);
        }

        av1_round_shift_array_32_sse4_1(out_128, buf_128, txfm2d_size_128, -shift[2]);
        transpose_8nx8n(buf_128, out_128, 64, 64);
        break;
    case IDTX:
        load_buffer_64x64_sse4_1(input, stride, out_128);
        /*col wise transform*/
        fidtx64x64_sse4_1(out_128, buf_128, cos_bit_col, stage_range_col);
        av1_round_shift_array_32_sse4_1(buf_128, out_128, txfm2d_size_128, -shift[1]);
        /*row wise transform*/
        fidtx64x64_sse4_1(out_128, buf_128, cos_bit_col, stage_range_col);

        av1_round_shift_array_32_sse4_1(buf_128, out_128, txfm2d_size_128, -shift[2]);
        break;
    default:
        assert(0);
    }
}

void svt_av1_fwd_txfm2d_32x64_sse4_1(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)tx_type;
    __m128i       in[512];
    __m128i*      outcoef128    = (__m128i*)coeff;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_32X64];
    const int     txw_idx       = get_txw_idx(TX_32X64);
    const int     txh_idx       = get_txh_idx(TX_32X64);
    const int     txfm_size_col = tx_size_wide[TX_32X64];
    const int     txfm_size_row = tx_size_high[TX_32X64];
    int           bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int           bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int     num_row       = txfm_size_row >> 2;
    const int     num_col       = txfm_size_col >> 2;

    // column transform
    load_buffer_32x8n(input, in, stride, 0, 0, shift[0], txfm_size_row);
    for (int i = 0; i < num_col; i++) {
        svt_av1_fdct64_sse4_1((in + i), (in + i), bitcol, num_col, num_col);
    }
    for (int i = 0; i < num_col; i++) {
        col_txfm_16x16_rounding((in + i * txfm_size_row), -shift[1]);
    }
    transpose_8nx8n(in, outcoef128, txfm_size_col, txfm_size_row);

    // row transform
    for (int i = 0; i < num_row; i++) {
        svt_av1_fdct32_sse4_1((outcoef128 + i), (in + i), bitrow, num_row);
    }
    transpose_8nx8n(in, outcoef128, txfm_size_row, txfm_size_col);
    av1_round_shift_rect_array_32_sse4_1(outcoef128, outcoef128, 512, -shift[2]);
    (void)bd;
}

void svt_av1_fwd_txfm2d_64x32_sse4_1(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)tx_type;
    __m128i       in[512];
    __m128i*      outcoef128    = (__m128i*)coeff;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_64X32];
    const int     txw_idx       = get_txw_idx(TX_64X32);
    const int     txh_idx       = get_txh_idx(TX_64X32);
    const int     txfm_size_col = tx_size_wide[TX_64X32];
    const int     txfm_size_row = tx_size_high[TX_64X32];
    int           bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int           bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int     num_row       = txfm_size_row >> 2;
    const int     num_col       = txfm_size_col >> 2;

    // column transform
    for (int i = 0; i < 32; i++) {
        load_buffer_4x4(input + 0 + i * stride, in + 0 + i * 16, 4, 0, 0, shift[0]);
        load_buffer_4x4(input + 16 + i * stride, in + 4 + i * 16, 4, 0, 0, shift[0]);
        load_buffer_4x4(input + 32 + i * stride, in + 8 + i * 16, 4, 0, 0, shift[0]);
        load_buffer_4x4(input + 48 + i * stride, in + 12 + i * 16, 4, 0, 0, shift[0]);
    }

    for (int i = 0; i < num_col; i++) {
        svt_av1_fdct32_sse4_1((in + i), (in + i), bitcol, num_col);
    }

    for (int i = 0; i < num_row; i++) {
        col_txfm_16x16_rounding((in + i * txfm_size_col), -shift[1]);
    }
    transpose_8nx8n(in, outcoef128, txfm_size_col, txfm_size_row);

    // row transform
    for (int i = 0; i < num_row; i++) {
        svt_av1_fdct64_sse4_1((outcoef128 + i), (in + i), bitrow, num_row, num_row);
    }
    transpose_8nx8n(in, outcoef128, txfm_size_row, txfm_size_col);
    av1_round_shift_rect_array_32_sse4_1(outcoef128, outcoef128, 512, -shift[2]);
    (void)bd;
}

void svt_av1_fwd_txfm2d_64x16_sse4_1(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[256];
    __m128i*      outcoeff128   = (__m128i*)coeff;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_64X16];
    const int     txw_idx       = get_txw_idx(TX_64X16);
    const int     txh_idx       = get_txh_idx(TX_64X16);
    const int     txfm_size_col = tx_size_wide[TX_64X16];
    const int     txfm_size_row = tx_size_high[TX_64X16];
    int           bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int           bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    int           ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    // col tranform
    for (int i = 0; i < txfm_size_row; i++) {
        load_buffer_4x4(input + 0 + i * stride, in + 0 + i * txfm_size_row, 4, ud_flip, lr_flip, shift[0]);
        load_buffer_4x4(input + 16 + i * stride, in + 4 + i * txfm_size_row, 4, ud_flip, lr_flip, shift[0]);
        load_buffer_4x4(input + 32 + i * stride, in + 8 + i * txfm_size_row, 4, ud_flip, lr_flip, shift[0]);
        load_buffer_4x4(input + 48 + i * stride, in + 12 + i * txfm_size_row, 4, ud_flip, lr_flip, shift[0]);
    }

    fdct16x16_sse4_1(in, outcoeff128, bitcol, txfm_size_row);
    col_txfm_16x16_rounding(outcoeff128, -shift[1]);
    col_txfm_16x16_rounding(outcoeff128 + 64, -shift[1]);
    col_txfm_16x16_rounding(outcoeff128 + 128, -shift[1]);
    col_txfm_16x16_rounding(outcoeff128 + 192, -shift[1]);

    transpose_8nx8n(outcoeff128, in, txfm_size_col, txfm_size_row);
    for (int i = 0; i < 4; i++) {
        svt_av1_fdct64_sse4_1(in + i, in + i, bitrow, 4, 4);
    }
    transpose_8nx8n(in, outcoeff128, txfm_size_row, 64);
    (void)bd;
}

void svt_av1_fwd_txfm2d_32x32_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    DECLARE_ALIGNED(16, int32_t, txfm_buf[1024]);
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(tx_type, TX_32X32, &cfg);
    (void)bd;
    fwd_txfm2d_sse4_1(input, output, stride, &cfg, txfm_buf);
}

void svt_av1_fwd_txfm2d_32x16_sse4_1(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i                       in[128];
    __m128i*                      outcoef128 = (__m128i*)coeff;
    const int8_t*                 shift      = fwd_txfm_shift_ls[TX_32X16];
    const int                     txw_idx    = get_txw_idx(TX_32X16);
    const int                     txh_idx    = get_txh_idx(TX_32X16);
    const fwd_transform_1d_sse4_1 col_txfm   = row_highbd_txfm8x32_arr[tx_type];
    const fwd_transform_1d_sse4_1 row_txfm   = col_highbd_txfm8x32_arr[tx_type];
    int                           bitcol     = fwd_cos_bit_col[txw_idx][txh_idx];
    int                           bitrow     = fwd_cos_bit_row[txw_idx][txh_idx];

    // column transform
    load_buffer_32x8n(input, in, stride, 0, 0, shift[0], 16);
    col_txfm(in, in, bitcol, 8);
    col_txfm_16x16_rounding(&in[0], -shift[1]);
    col_txfm_16x16_rounding(&in[64], -shift[1]);
    transpose_8nx8n(in, outcoef128, 32, 16);

    // row transform
    for (int i = 0; i < 4; i++) {
        row_txfm((outcoef128 + i), (in + i), bitrow, 4);
    }
    transpose_8nx8n(in, outcoef128, 16, 32);
    av1_round_shift_rect_array_32_sse4_1(outcoef128, outcoef128, 128, -shift[2]);
    (void)bd;
}

void svt_av1_fwd_txfm2d_32x8_sse4_1(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i                       in[64];
    __m128i*                      outcoef128 = (__m128i*)coeff;
    const int8_t*                 shift      = fwd_txfm_shift_ls[TX_32X8];
    const int                     txw_idx    = get_txw_idx(TX_32X8);
    const int                     txh_idx    = get_txh_idx(TX_32X8);
    const fwd_transform_1d_sse4_1 col_txfm   = row_highbd_txfm32x8_arr[tx_type];
    const fwd_transform_1d_sse4_1 row_txfm   = col_highbd_txfm8x32_arr[tx_type];
    int                           bitcol     = fwd_cos_bit_col[txw_idx][txh_idx];
    int                           bitrow     = fwd_cos_bit_row[txw_idx][txh_idx];

    const int txfm_size_col = tx_size_wide[TX_32X8];
    const int txfm_size_row = tx_size_high[TX_32X8];
    const int num_col       = txfm_size_row >> 2;

    // column transform
    load_buffer_32x8n(input, in, stride, 0, 0, shift[0], 8);
    for (int i = 0; i < txfm_size_row; i += 2) {
        col_txfm((in + i), (in + i), bitcol, txfm_size_row);
    }

    col_txfm_16x16_rounding(&in[0], -shift[1]);
    transpose_8nx8n(in, outcoef128, txfm_size_col, txfm_size_row);

    // row transform
    for (int i = 0; i < num_col; i++) {
        row_txfm((outcoef128 + i), (in + i), bitrow, num_col);
    }
    transpose_8nx8n(in, outcoef128, txfm_size_row, txfm_size_col);
    (void)bd;
}

void svt_av1_fwd_txfm2d_16x64_sse4_1(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[256];
    __m128i*      outcoeff128   = (__m128i*)coeff;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_16X64];
    const int     txw_idx       = get_txw_idx(TX_16X64);
    const int     txh_idx       = get_txh_idx(TX_16X64);
    const int     txfm_size_col = tx_size_wide[TX_16X64];
    const int     txfm_size_row = tx_size_high[TX_16X64];
    int           bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int           bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    int           ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    const int num_col = txfm_size_col >> 2;
    // col tranform
    for (int i = 0; i < txfm_size_row; i += num_col) {
        load_buffer_4x4(input + (i + 0) * stride, in + (i + 0) * num_col, num_col, ud_flip, lr_flip, shift[0]);
        load_buffer_4x4(input + (i + 1) * stride, in + (i + 1) * num_col, num_col, ud_flip, lr_flip, shift[0]);
        load_buffer_4x4(input + (i + 2) * stride, in + (i + 2) * num_col, num_col, ud_flip, lr_flip, shift[0]);
        load_buffer_4x4(input + (i + 3) * stride, in + (i + 3) * num_col, num_col, ud_flip, lr_flip, shift[0]);
    }

    for (int i = 0; i < num_col; i++) {
        svt_av1_fdct64_sse4_1(in + i, outcoeff128 + i, bitcol, num_col, num_col);
    }

    col_txfm_16x16_rounding(outcoeff128, -shift[1]);
    col_txfm_16x16_rounding(outcoeff128 + 64, -shift[1]);
    col_txfm_16x16_rounding(outcoeff128 + 128, -shift[1]);
    col_txfm_16x16_rounding(outcoeff128 + 192, -shift[1]);

    transpose_8nx8n(outcoeff128, in, txfm_size_col, 64);
    fdct16x16_sse4_1(in, in, bitrow, 16);
    transpose_8nx8n(in, outcoeff128, 64, txfm_size_col);
    (void)bd;
}

void svt_av1_fwd_txfm2d_16x32_sse4_1(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i                       in[128];
    __m128i*                      outcoef128 = (__m128i*)coeff;
    const int8_t*                 shift      = fwd_txfm_shift_ls[TX_16X32];
    const int                     txw_idx    = get_txw_idx(TX_16X32);
    const int                     txh_idx    = get_txh_idx(TX_16X32);
    const fwd_transform_1d_sse4_1 col_txfm   = col_highbd_txfm8x32_arr[tx_type];
    const fwd_transform_1d_sse4_1 row_txfm   = row_highbd_txfm8x32_arr[tx_type];
    int                           bitcol     = fwd_cos_bit_col[txw_idx][txh_idx];
    int                           bitrow     = fwd_cos_bit_row[txw_idx][txh_idx];

    // column transform
    load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
    load_buffer_16x16(input + 16 * stride, in + 64, stride, 0, 0, shift[0]);

    for (int i = 0; i < 4; i++) {
        col_txfm((in + i), (in + i), bitcol, 4);
    }
    col_txfm_16x16_rounding(&in[0], -shift[1]);
    col_txfm_16x16_rounding(&in[64], -shift[1]);
    transpose_8nx8n(in, outcoef128, 16, 32);

    // row transform
    row_txfm(outcoef128, in, bitrow, 8);
    transpose_8nx8n(in, outcoef128, 32, 16);
    av1_round_shift_rect_array_32_sse4_1(outcoef128, outcoef128, 128, -shift[2]);
    (void)bd;
}

void svt_av1_fwd_txfm2d_16x16_sse4_1(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[64], out[64];
    const int8_t* shift   = fwd_txfm_shift_ls[TX_16X16];
    const int     txw_idx = get_txw_idx(TX_16X16);
    const int     txh_idx = get_txh_idx(TX_16X16);
    const int     col_num = 4;
    switch (tx_type) {
    case DCT_DCT:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fdct16x16_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        transpose_16x16(out, in);
        fdct16x16_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case ADST_DCT:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fadst16x16_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        transpose_16x16(out, in);
        fdct16x16_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case DCT_ADST:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fdct16x16_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        transpose_16x16(out, in);
        fadst16x16_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case ADST_ADST:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fadst16x16_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        transpose_16x16(out, in);
        fadst16x16_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case FLIPADST_DCT:
        load_buffer_16x16(input, in, stride, 1, 0, shift[0]);
        fadst16x16_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        transpose_16x16(out, in);
        fdct16x16_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case DCT_FLIPADST:
        load_buffer_16x16(input, in, stride, 0, 1, shift[0]);
        fdct16x16_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        transpose_16x16(out, in);
        fadst16x16_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_16x16(input, in, stride, 1, 1, shift[0]);
        fadst16x16_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        transpose_16x16(out, in);
        fadst16x16_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case ADST_FLIPADST:
        load_buffer_16x16(input, in, stride, 0, 1, shift[0]);
        fadst16x16_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        transpose_16x16(out, in);
        fadst16x16_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case FLIPADST_ADST:
        load_buffer_16x16(input, in, stride, 1, 0, shift[0]);
        fadst16x16_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        transpose_16x16(out, in);
        fadst16x16_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case IDTX:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        idtx16x16_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        transpose_16x16(out, in);
        idtx16x16_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case V_DCT:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fdct16x16_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        transpose_16x16(out, in);
        idtx16x16_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case H_DCT:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        idtx16x16_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        transpose_16x16(out, in);
        fdct16x16_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case V_ADST:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fadst16x16_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        transpose_16x16(out, in);
        idtx16x16_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case H_ADST:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        idtx16x16_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        transpose_16x16(out, in);
        fadst16x16_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case V_FLIPADST:
        load_buffer_16x16(input, in, stride, 1, 0, shift[0]);
        fadst16x16_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        transpose_16x16(out, in);
        idtx16x16_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case H_FLIPADST:
        load_buffer_16x16(input, in, stride, 0, 1, shift[0]);
        idtx16x16_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        transpose_16x16(out, in);
        fadst16x16_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16(out, in);
        write_buffer_16x16(in, coeff);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_16x8_sse4_1(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i                       in[32], out[32];
    const int8_t*                 shift    = fwd_txfm_shift_ls[TX_16X8];
    const int                     txw_idx  = get_txw_idx(TX_16X8);
    const int                     txh_idx  = get_txh_idx(TX_16X8);
    const fwd_transform_1d_sse4_1 col_txfm = col_highbd_txfm8x8_arr[tx_type];
    const fwd_transform_1d_sse4_1 row_txfm = row_highbd_txfm8x16_arr[tx_type];
    int                           bit      = fwd_cos_bit_col[txw_idx][txh_idx];
    int                           ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);

    for (int i = 0; i < 2; i++) {
        load_buffer_8x8(input + i * 8, in, stride, ud_flip, 0, shift[0]);
        col_txfm(in, in, bit, 2);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8(in, out + i * 16);
    }

    if (lr_flip) {
        flip_buf_sse4_1(in, out, 32);
        row_txfm(in, out, bit, 2);
    } else {
        row_txfm(out, out, bit, 2);
    }

    for (int i = 0; i < 2; i++) {
        transpose_8x8(out + i * 16, in);
        av1_round_shift_rect_array_32_sse4_1(in, in, 16, -shift[2]);
        write_buffer_16x8(in, coeff + i * 8, 16);
    }

    (void)bd;
}

void svt_av1_fwd_txfm2d_16x4_sse4_1(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i                       in[16];
    __m128i*                      outcoeff128   = (__m128i*)coeff;
    const int8_t*                 shift         = fwd_txfm_shift_ls[TX_16X4];
    const int                     txw_idx       = get_txw_idx(TX_16X4);
    const int                     txh_idx       = get_txh_idx(TX_16X4);
    const int                     txfm_size_col = tx_size_wide[TX_16X4];
    const int                     txfm_size_row = tx_size_high[TX_16X4];
    int                           bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int                           bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const fwd_transform_1d_sse4_1 col_txfm      = col_highbd_txfm4x4_arr[tx_type];
    const fwd_transform_1d_sse4_1 row_txfm      = row_highbd_txfm8x16_arr[tx_type];
    int                           ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);

    // col transform
    load_buffer_16x4(input, in, stride, ud_flip, lr_flip, shift[0]);

    for (int i = 0; i < txfm_size_row; i++) {
        col_txfm(in + i * txfm_size_row, outcoeff128 + i * txfm_size_row, bitcol, 1);
    }
    col_txfm_8x8_rounding(outcoeff128, -shift[1]);

    // row transform
    row_txfm(outcoeff128, in, bitrow, 1);
    transpose_8nx8n(in, outcoeff128, txfm_size_row, txfm_size_col);
    (void)bd;
}

void svt_av1_fwd_txfm2d_8x32_sse4_1(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i                       in[64];
    __m128i*                      outcoef128 = (__m128i*)coeff;
    const int8_t*                 shift      = fwd_txfm_shift_ls[TX_8X32];
    const int                     txw_idx    = get_txw_idx(TX_8X32);
    const int                     txh_idx    = get_txh_idx(TX_8X32);
    const fwd_transform_1d_sse4_1 col_txfm   = col_highbd_txfm8x32_arr[tx_type];
    const fwd_transform_1d_sse4_1 row_txfm   = row_highbd_txfm32x8_arr[tx_type];
    int                           bitcol     = fwd_cos_bit_col[txw_idx][txh_idx];
    int                           bitrow     = fwd_cos_bit_row[txw_idx][txh_idx];

    const int txfm_size_col = tx_size_wide[TX_8X32];
    const int txfm_size_row = tx_size_high[TX_8X32];
    const int num_col       = txfm_size_col >> 2;

    // column transform
    load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
    load_buffer_8x16(input + (txfm_size_row >> 1) * stride, in + txfm_size_row, stride, 0, 0, shift[0]);

    for (int i = 0; i < num_col; i++) {
        col_txfm((in + i), (in + i), bitcol, num_col);
    }
    col_txfm_16x16_rounding(in, -shift[1]);
    transpose_8nx8n(in, outcoef128, txfm_size_col, txfm_size_row);

    // row transform
    for (int i = 0; i < txfm_size_col; i += 2) {
        row_txfm((outcoef128 + i), (in + i), bitrow, txfm_size_col);
    }
    transpose_8nx8n(in, outcoef128, txfm_size_row, txfm_size_col);
    (void)bd;
}

void svt_av1_fwd_txfm2d_8x16_sse4_1(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i                       in[32], out[32];
    const int8_t*                 shift    = fwd_txfm_shift_ls[TX_8X16];
    const int                     txw_idx  = get_txw_idx(TX_8X16);
    const int                     txh_idx  = get_txh_idx(TX_8X16);
    const fwd_transform_1d_sse4_1 col_txfm = col_highbd_txfm8x16_arr[tx_type];
    const fwd_transform_1d_sse4_1 row_txfm = row_highbd_txfm8x8_arr[tx_type];
    int                           bit      = fwd_cos_bit_col[txw_idx][txh_idx];
    int                           ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);

    load_buffer_8x16(input, in, stride, ud_flip, lr_flip, shift[0]);
    col_txfm(in, in, bit, 2);
    col_txfm_8x16_rounding(in, -shift[1]);
    transpose_8x8(in, out);
    transpose_8x8(in + 16, out + 16);

    for (int i = 0; i < 2; i++) {
        row_txfm(out + i * 16, out, bit, 2);
        transpose_8x8(out, in);
        av1_round_shift_rect_array_32_sse4_1(in, in, 16, -shift[2]);
        write_buffer_8x8(in, coeff + i * 64);
    }

    (void)bd;
}

void svt_av1_fwd_txfm2d_8x8_sse4_1(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[16], out[16];
    const int8_t* shift   = fwd_txfm_shift_ls[TX_8X8];
    const int     txw_idx = get_txw_idx(TX_8X8);
    const int     txh_idx = get_txh_idx(TX_8X8);

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fdct8x8_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2);
        col_txfm_8x8_rounding(out, -shift[1]);
        transpose_8x8(out, in);
        fdct8x8_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 2);
        transpose_8x8(out, in);
        write_buffer_8x8(in, coeff);
        break;
    case ADST_DCT:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fadst8x8_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2);
        col_txfm_8x8_rounding(out, -shift[1]);
        transpose_8x8(out, in);
        fdct8x8_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 2);
        transpose_8x8(out, in);
        write_buffer_8x8(in, coeff);
        break;
    case DCT_ADST:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fdct8x8_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2);
        col_txfm_8x8_rounding(out, -shift[1]);
        transpose_8x8(out, in);
        fadst8x8_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 2);
        transpose_8x8(out, in);
        write_buffer_8x8(in, coeff);
        break;
    case ADST_ADST:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fadst8x8_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2);
        col_txfm_8x8_rounding(out, -shift[1]);
        transpose_8x8(out, in);
        fadst8x8_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 2);
        transpose_8x8(out, in);
        write_buffer_8x8(in, coeff);
        break;
    case FLIPADST_DCT:
        load_buffer_8x8(input, in, stride, 1, 0, shift[0]);
        fadst8x8_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2);
        col_txfm_8x8_rounding(out, -shift[1]);
        transpose_8x8(out, in);
        fdct8x8_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 2);
        transpose_8x8(out, in);
        write_buffer_8x8(in, coeff);
        break;
    case DCT_FLIPADST:
        load_buffer_8x8(input, in, stride, 0, 1, shift[0]);
        fdct8x8_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2);
        col_txfm_8x8_rounding(out, -shift[1]);
        transpose_8x8(out, in);
        fadst8x8_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 2);
        transpose_8x8(out, in);
        write_buffer_8x8(in, coeff);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_8x8(input, in, stride, 1, 1, shift[0]);
        fadst8x8_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2);
        col_txfm_8x8_rounding(out, -shift[1]);
        transpose_8x8(out, in);
        fadst8x8_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 2);
        transpose_8x8(out, in);
        write_buffer_8x8(in, coeff);
        break;
    case ADST_FLIPADST:
        load_buffer_8x8(input, in, stride, 0, 1, shift[0]);
        fadst8x8_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2);
        col_txfm_8x8_rounding(out, -shift[1]);
        transpose_8x8(out, in);
        fadst8x8_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 2);
        transpose_8x8(out, in);
        write_buffer_8x8(in, coeff);
        break;
    case FLIPADST_ADST:
        load_buffer_8x8(input, in, stride, 1, 0, shift[0]);
        fadst8x8_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2);
        col_txfm_8x8_rounding(out, -shift[1]);
        transpose_8x8(out, in);
        fadst8x8_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 2);
        transpose_8x8(out, in);
        write_buffer_8x8(in, coeff);
        break;
    case IDTX:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        idtx8x8_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2);
        col_txfm_8x8_rounding(out, -shift[1]);
        transpose_8x8(out, in);
        idtx8x8_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2);
        transpose_8x8(out, in);
        write_buffer_8x8(in, coeff);
        break;
    case V_DCT:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fdct8x8_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2);
        col_txfm_8x8_rounding(out, -shift[1]);
        transpose_8x8(out, in);
        idtx8x8_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2);
        transpose_8x8(out, in);
        write_buffer_8x8(in, coeff);
        break;
    case H_DCT:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        idtx8x8_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2);
        col_txfm_8x8_rounding(out, -shift[1]);
        transpose_8x8(out, in);
        fdct8x8_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2);
        transpose_8x8(out, in);
        write_buffer_8x8(in, coeff);
        break;
    case V_ADST:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fadst8x8_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2);
        col_txfm_8x8_rounding(out, -shift[1]);
        transpose_8x8(out, in);
        idtx8x8_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2);
        transpose_8x8(out, in);
        write_buffer_8x8(in, coeff);
        break;
    case H_ADST:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        idtx8x8_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2);
        col_txfm_8x8_rounding(out, -shift[1]);
        transpose_8x8(out, in);
        fadst8x8_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2);
        transpose_8x8(out, in);
        write_buffer_8x8(in, coeff);
        break;
    case V_FLIPADST:
        load_buffer_8x8(input, in, stride, 1, 0, shift[0]);
        fadst8x8_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2);
        col_txfm_8x8_rounding(out, -shift[1]);
        transpose_8x8(out, in);
        idtx8x8_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2);
        transpose_8x8(out, in);
        write_buffer_8x8(in, coeff);
        break;
    case H_FLIPADST:
        load_buffer_8x8(input, in, stride, 0, 1, shift[0]);
        idtx8x8_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2);
        col_txfm_8x8_rounding(out, -shift[1]);
        transpose_8x8(out, in);
        fadst8x8_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2);
        transpose_8x8(out, in);
        write_buffer_8x8(in, coeff);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_8x4_sse4_1(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i                       in[8];
    __m128i*                      outcoeff128   = (__m128i*)coeff;
    const int8_t*                 shift         = fwd_txfm_shift_ls[TX_8X4];
    const int                     txw_idx       = get_txw_idx(TX_8X4);
    const int                     txh_idx       = get_txh_idx(TX_8X4);
    const int                     txfm_size_col = tx_size_wide[TX_8X4];
    const int                     txfm_size_row = tx_size_high[TX_8X4];
    int                           bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int                           bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const fwd_transform_1d_sse4_1 col_txfm      = col_highbd_txfm4x4_arr[tx_type];
    const fwd_transform_1d_sse4_1 row_txfm      = row_highbd_txfm4x8_arr[tx_type];
    int                           ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    // col tranform
    load_buffer_8x4(input, in, stride, ud_flip, lr_flip, shift[0]);
    for (int i = 0; i < 2; i++) {
        col_txfm(in + i * txfm_size_row, in + i * txfm_size_row, bitcol, 1);
    }
    col_txfm_4x8_rounding(in, -shift[1]);

    // row tranform
    row_txfm(in, outcoeff128, bitrow, 1);
    av1_round_shift_rect_array_32_sse4_1(outcoeff128, in, txfm_size_col, -shift[2]);
    transpose_8nx8n(in, outcoeff128, txfm_size_row, txfm_size_col);
    (void)bd;
}

void svt_av1_fwd_txfm2d_4x16_sse4_1(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i                       in[16];
    __m128i*                      outcoeff128   = (__m128i*)coeff;
    const int8_t*                 shift         = fwd_txfm_shift_ls[TX_4X16];
    const int                     txw_idx       = get_txw_idx(TX_4X16);
    const int                     txh_idx       = get_txh_idx(TX_4X16);
    const int                     txfm_size_col = tx_size_wide[TX_4X16];
    const int                     txfm_size_row = tx_size_high[TX_4X16];
    int                           bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int                           bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const fwd_transform_1d_sse4_1 col_txfm      = col_highbd_txfm8x16_arr[tx_type];
    const fwd_transform_1d_sse4_1 row_txfm      = row_highbd_txfm4x4_arr[tx_type];

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    // col transform
    load_buffer_4x16(input, in, stride, ud_flip, lr_flip, shift[0]);
    col_txfm(in, outcoeff128, bitcol, 1);
    col_txfm_8x8_rounding(outcoeff128, -shift[1]);
    transpose_8nx8n(outcoeff128, in, txfm_size_col, txfm_size_row);

    // row transform
    for (int i = 0; i < txfm_size_col; i++) {
        row_txfm(in + i, outcoeff128 + i * txfm_size_col, bitrow, txfm_size_col);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_4x8_sse4_1(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i                       in[8];
    __m128i*                      outcoeff128   = (__m128i*)coeff;
    const int8_t*                 shift         = fwd_txfm_shift_ls[TX_4X8];
    const int                     txw_idx       = get_txw_idx(TX_4X8);
    const int                     txh_idx       = get_txh_idx(TX_4X8);
    const int                     txfm_size_col = tx_size_wide[TX_4X8];
    const int                     txfm_size_row = tx_size_high[TX_4X8];
    int                           bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int                           bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const fwd_transform_1d_sse4_1 col_txfm      = col_highbd_txfm4x8_arr[tx_type];
    const fwd_transform_1d_sse4_1 row_txfm      = row_highbd_txfm4x4_arr[tx_type];

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);

    load_buffer_4x8(input, in, stride, ud_flip, lr_flip, shift[0]);
    col_txfm(in, in, bitcol, 1);
    col_txfm_4x8_rounding(in, -shift[1]);
    transpose_8nx8n(in, outcoeff128, txfm_size_col, txfm_size_row);

    for (int i = 0; i < 2; i++) {
        row_txfm(outcoeff128 + i, in + i * txfm_size_col, bitrow, 2);
    }
    av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, txfm_size_row, -shift[2]);
    (void)bd;
}

#define TRANSPOSE_2X4X4_SSE4_1(in, x0, x2, x4, x6, out, y0, y1, y2, y3, y4, y5, y6, y7) \
    do {                                                                                \
        __m128i u0, u1, u2, u3, u4, u5, u6, u7;                                         \
        u0 = _mm_unpacklo_epi32(in[x0], in[x2]);                                        \
        u1 = _mm_unpackhi_epi32(in[x0], in[x2]);                                        \
        u2 = _mm_unpacklo_epi32(in[x0 + 1], in[x2 + 1]);                                \
        u3 = _mm_unpackhi_epi32(in[x0 + 1], in[x2 + 1]);                                \
        u4 = _mm_unpacklo_epi32(in[x4], in[x6]);                                        \
        u5 = _mm_unpackhi_epi32(in[x4], in[x6]);                                        \
        u6 = _mm_unpacklo_epi32(in[x4 + 1], in[x6 + 1]);                                \
        u7 = _mm_unpackhi_epi32(in[x4 + 1], in[x6 + 1]);                                \
                                                                                        \
        out[y0] = _mm_unpacklo_epi64(u0, u4);                                           \
        out[y1] = _mm_unpacklo_epi64(u2, u6);                                           \
        out[y2] = _mm_unpackhi_epi64(u0, u4);                                           \
        out[y3] = _mm_unpackhi_epi64(u2, u6);                                           \
        out[y4] = _mm_unpacklo_epi64(u1, u5);                                           \
        out[y5] = _mm_unpacklo_epi64(u3, u7);                                           \
        out[y6] = _mm_unpackhi_epi64(u1, u5);                                           \
        out[y7] = _mm_unpackhi_epi64(u3, u7);                                           \
    } while (0)

static INLINE void transpose_8x8_sse4_1(const __m128i* in, __m128i* out) {
    TRANSPOSE_2X4X4_SSE4_1(in, 0, 2, 4, 6, out, 0, 8, 2, 10, 4, 12, 6, 14);
    TRANSPOSE_2X4X4_SSE4_1(in, 8, 10, 12, 14, out, 1, 9, 3, 11, 5, 13, 7, 15);
}

static INLINE void transpose_8nx8n_N2_half_sse4_1(const __m128i* input, __m128i* output, const int32_t width,
                                                  const int32_t height) {
    const int32_t numcol      = height >> 3;
    const int32_t numrow      = width >> 3;
    int32_t       calc_numcol = numcol >> 1;
    if (!calc_numcol) {
        calc_numcol = 1;
    }

    for (int32_t j = 0; j < numrow; j++) {
        for (int32_t i = 0; i < calc_numcol; i++) {
            TRANSPOSE_2X4X4_SSE4_1(input,
                                   2 * (i * width + j + (numrow * 0)),
                                   2 * (i * width + j + (numrow * 1)),
                                   2 * (i * width + j + (numrow * 2)),
                                   2 * (i * width + j + (numrow * 3)),
                                   output,
                                   2 * (j * height + i + (numcol * 0)),
                                   2 * (j * height + i + (numcol * 4)),
                                   2 * (j * height + i + (numcol * 1)),
                                   2 * (j * height + i + (numcol * 5)),
                                   2 * (j * height + i + (numcol * 2)),
                                   2 * (j * height + i + (numcol * 6)),
                                   2 * (j * height + i + (numcol * 3)),
                                   2 * (j * height + i + (numcol * 7)));
            TRANSPOSE_2X4X4_SSE4_1(input,
                                   2 * (i * width + j + (numrow * 4)),
                                   2 * (i * width + j + (numrow * 5)),
                                   2 * (i * width + j + (numrow * 6)),
                                   2 * (i * width + j + (numrow * 7)),
                                   output,
                                   2 * (j * height + i + (numcol * 0)) + 1,
                                   2 * (j * height + i + (numcol * 4)) + 1,
                                   2 * (j * height + i + (numcol * 1)) + 1,
                                   2 * (j * height + i + (numcol * 5)) + 1,
                                   2 * (j * height + i + (numcol * 2)) + 1,
                                   2 * (j * height + i + (numcol * 6)) + 1,
                                   2 * (j * height + i + (numcol * 3)) + 1,
                                   2 * (j * height + i + (numcol * 7)) + 1);
        }
    }
}

static INLINE void transpose_8nx8n_N2_quad_sse4_1(const __m128i* input, __m128i* output, const int32_t width,
                                                  const int32_t height) {
    const int32_t numcol = height >> 3;
    const int32_t numrow = width >> 3;

    int32_t calc_numcol = numcol >> 1;
    int32_t calc_numrow = numrow >> 1;
    if (!calc_numcol) {
        calc_numcol = 1;
    }
    if (!calc_numrow) {
        calc_numrow = 1;
    }

    for (int32_t j = 0; j < calc_numrow; j++) {
        for (int32_t i = 0; i < calc_numcol; i++) {
            TRANSPOSE_2X4X4_SSE4_1(input,
                                   2 * (i * width + j + (numrow * 0)),
                                   2 * (i * width + j + (numrow * 1)),
                                   2 * (i * width + j + (numrow * 2)),
                                   2 * (i * width + j + (numrow * 3)),
                                   output,
                                   2 * (j * height + i + (numcol * 0)),
                                   2 * (j * height + i + (numcol * 4)),
                                   2 * (j * height + i + (numcol * 1)),
                                   2 * (j * height + i + (numcol * 5)),
                                   2 * (j * height + i + (numcol * 2)),
                                   2 * (j * height + i + (numcol * 6)),
                                   2 * (j * height + i + (numcol * 3)),
                                   2 * (j * height + i + (numcol * 7)));
            TRANSPOSE_2X4X4_SSE4_1(input,
                                   2 * (i * width + j + (numrow * 4)),
                                   2 * (i * width + j + (numrow * 5)),
                                   2 * (i * width + j + (numrow * 6)),
                                   2 * (i * width + j + (numrow * 7)),
                                   output,
                                   2 * (j * height + i + (numcol * 0)) + 1,
                                   2 * (j * height + i + (numcol * 4)) + 1,
                                   2 * (j * height + i + (numcol * 1)) + 1,
                                   2 * (j * height + i + (numcol * 5)) + 1,
                                   2 * (j * height + i + (numcol * 2)) + 1,
                                   2 * (j * height + i + (numcol * 6)) + 1,
                                   2 * (j * height + i + (numcol * 3)) + 1,
                                   2 * (j * height + i + (numcol * 7)) + 1);
        }
    }
}

static INLINE void transpose_4x8_sse4_1(const __m128i* in, __m128i* out) {
    __m128i out1[8];
    TRANSPOSE_2X4X4_SSE4_1(in, 0, 2, 4, 6, out1, 0, 1, 2, 3, 4, 5, 6, 7);

    out[0] = _mm_unpacklo_epi32(out1[0], out1[1]);
    out[1] = _mm_unpackhi_epi32(out1[0], out1[1]);
    out[2] = _mm_unpacklo_epi32(out1[2], out1[3]);
    out[3] = _mm_unpackhi_epi32(out1[2], out1[3]);
    out[4] = _mm_unpacklo_epi32(out1[4], out1[5]);
    out[5] = _mm_unpackhi_epi32(out1[4], out1[5]);
    out[6] = _mm_unpacklo_epi32(out1[6], out1[7]);
    out[7] = _mm_unpackhi_epi32(out1[6], out1[7]);
}

static INLINE __m128i half_btf_small(const __m128i* w0, const __m128i* n0, const __m128i* w1, const __m128i* n1,
                                     const __m128i* rounding, int32_t bit) {
    __m128i x, y;

    x = _mm_mullo_epi32(*w0, *n0);
    y = _mm_mullo_epi32(*w1, *n1);
    x = _mm_add_epi32(x, y);
    x = _mm_add_epi32(x, *rounding);
    x = _mm_srai_epi32(x, bit);
    return x;
}

// out0 = in0*w0 + in1*w1
// out1 = -in1*w0 + in0*w1
#define btf_32_type0_sse4_1_new(ww0, ww1, in0, in1, out0, out1, r, bit) \
    do {                                                                \
        const __m128i in0_w0 = _mm_mullo_epi32(in0, ww0);               \
        const __m128i in1_w1 = _mm_mullo_epi32(in1, ww1);               \
        out0                 = _mm_add_epi32(in0_w0, in1_w1);           \
        out0                 = _mm_add_epi32(out0, r);                  \
        out0                 = _mm_srai_epi32(out0, bit);               \
        const __m128i in0_w1 = _mm_mullo_epi32(in0, ww1);               \
        const __m128i in1_w0 = _mm_mullo_epi32(in1, ww0);               \
        out1                 = _mm_sub_epi32(in0_w1, in1_w0);           \
        out1                 = _mm_add_epi32(out1, r);                  \
        out1                 = _mm_srai_epi32(out1, bit);               \
    } while (0)

#define REVERSE_FLIP_LR_8_SSE4_1(temp, x, y)            \
    do {                                                \
        __m128i t = temp[x];                            \
        temp[x]   = _mm_shufflelo_epi16(temp[y], 0x1b); \
        temp[y]   = _mm_shufflelo_epi16(t, 0x1b);       \
    } while (0)

static INLINE void load_buffer_8x8_N2(const int16_t* input, __m128i* in, int32_t stride, int32_t flipud, int32_t fliplr,
                                      int32_t shift) {
    __m128i temp[16];
    if (!flipud) {
        temp[0]  = _mm_loadl_epi64((const __m128i*)(input + 0 * stride));
        temp[1]  = _mm_loadl_epi64((const __m128i*)(input + 0 * stride + 4));
        temp[2]  = _mm_loadl_epi64((const __m128i*)(input + 1 * stride));
        temp[3]  = _mm_loadl_epi64((const __m128i*)(input + 1 * stride + 4));
        temp[4]  = _mm_loadl_epi64((const __m128i*)(input + 2 * stride));
        temp[5]  = _mm_loadl_epi64((const __m128i*)(input + 2 * stride + 4));
        temp[6]  = _mm_loadl_epi64((const __m128i*)(input + 3 * stride));
        temp[7]  = _mm_loadl_epi64((const __m128i*)(input + 3 * stride + 4));
        temp[8]  = _mm_loadl_epi64((const __m128i*)(input + 4 * stride));
        temp[9]  = _mm_loadl_epi64((const __m128i*)(input + 4 * stride + 4));
        temp[10] = _mm_loadl_epi64((const __m128i*)(input + 5 * stride));
        temp[11] = _mm_loadl_epi64((const __m128i*)(input + 5 * stride + 4));
        temp[12] = _mm_loadl_epi64((const __m128i*)(input + 6 * stride));
        temp[13] = _mm_loadl_epi64((const __m128i*)(input + 6 * stride + 4));
        temp[14] = _mm_loadl_epi64((const __m128i*)(input + 7 * stride));
        temp[15] = _mm_loadl_epi64((const __m128i*)(input + 7 * stride + 4));
    } else {
        temp[0]  = _mm_loadl_epi64((const __m128i*)(input + 7 * stride));
        temp[1]  = _mm_loadl_epi64((const __m128i*)(input + 7 * stride + 4));
        temp[2]  = _mm_loadl_epi64((const __m128i*)(input + 6 * stride));
        temp[3]  = _mm_loadl_epi64((const __m128i*)(input + 6 * stride + 4));
        temp[4]  = _mm_loadl_epi64((const __m128i*)(input + 5 * stride));
        temp[5]  = _mm_loadl_epi64((const __m128i*)(input + 5 * stride + 4));
        temp[6]  = _mm_loadl_epi64((const __m128i*)(input + 4 * stride));
        temp[7]  = _mm_loadl_epi64((const __m128i*)(input + 4 * stride + 4));
        temp[8]  = _mm_loadl_epi64((const __m128i*)(input + 3 * stride));
        temp[9]  = _mm_loadl_epi64((const __m128i*)(input + 3 * stride + 4));
        temp[10] = _mm_loadl_epi64((const __m128i*)(input + 2 * stride));
        temp[11] = _mm_loadl_epi64((const __m128i*)(input + 2 * stride + 4));
        temp[12] = _mm_loadl_epi64((const __m128i*)(input + 1 * stride));
        temp[13] = _mm_loadl_epi64((const __m128i*)(input + 1 * stride + 4));
        temp[14] = _mm_loadl_epi64((const __m128i*)(input + 0 * stride));
        temp[15] = _mm_loadl_epi64((const __m128i*)(input + 0 * stride + 4));
    }

    if (fliplr) {
        REVERSE_FLIP_LR_8_SSE4_1(temp, 0, 1);
        REVERSE_FLIP_LR_8_SSE4_1(temp, 2, 3);
        REVERSE_FLIP_LR_8_SSE4_1(temp, 4, 5);
        REVERSE_FLIP_LR_8_SSE4_1(temp, 6, 7);
        REVERSE_FLIP_LR_8_SSE4_1(temp, 8, 9);
        REVERSE_FLIP_LR_8_SSE4_1(temp, 10, 11);
        REVERSE_FLIP_LR_8_SSE4_1(temp, 12, 13);
        REVERSE_FLIP_LR_8_SSE4_1(temp, 14, 15);
    }

    in[0]  = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[0]), shift);
    in[1]  = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[1]), shift);
    in[4]  = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[2]), shift);
    in[5]  = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[3]), shift);
    in[8]  = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[4]), shift);
    in[9]  = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[5]), shift);
    in[12] = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[6]), shift);
    in[13] = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[7]), shift);
    in[16] = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[8]), shift);
    in[17] = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[9]), shift);
    in[20] = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[10]), shift);
    in[21] = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[11]), shift);
    in[24] = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[12]), shift);
    in[25] = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[13]), shift);
    in[28] = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[14]), shift);
    in[29] = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[15]), shift);
}

static INLINE void load_buffer_4x8_sse4_1(const int16_t* input, __m128i* out, int32_t stride, int32_t flipud,
                                          int32_t fliplr, int32_t shift) {
    const int16_t* top_l = input;
    const int16_t* bot_l = input + 4 * stride;

    if (flipud) {
        load_buffer_4x4(bot_l, out, stride, flipud, fliplr, shift);
        load_buffer_4x4(top_l, out + 4, stride, flipud, fliplr, shift);
    } else {
        load_buffer_4x4(top_l, out, stride, flipud, fliplr, shift);
        load_buffer_4x4(bot_l, out + 4, stride, flipud, fliplr, shift);
    }
}

static INLINE void load_buffer_8x4_sse4_1(const int16_t* input, __m128i* out, int32_t stride, int32_t flipud,
                                          int32_t fliplr, int32_t shift) {
    const int16_t* top_l = input;
    const int16_t* top_r = input + 4;

    if (fliplr) {
        load_buffer_4x4(top_r, out, stride, flipud, fliplr, shift);
        load_buffer_4x4(top_l, out + 4, stride, flipud, fliplr, shift);
    } else {
        load_buffer_4x4(top_l, out, stride, flipud, fliplr, shift);
        load_buffer_4x4(top_r, out + 4, stride, flipud, fliplr, shift);
    }
}

static INLINE void load_buffer_4x16_sse4_1(const int16_t* input, __m128i* out, const int32_t stride,
                                           const int32_t flipud, const int32_t fliplr, const int32_t shift) {
    const int16_t* top_l = input;
    const int16_t* bot_l = input + 8 * stride;

    if (flipud) {
        load_buffer_4x8_sse4_1(bot_l, out, stride, flipud, fliplr, shift);
        load_buffer_4x8_sse4_1(top_l, out + 8, stride, flipud, fliplr, shift);
    } else {
        load_buffer_4x8_sse4_1(top_l, out, stride, flipud, fliplr, shift);
        load_buffer_4x8_sse4_1(bot_l, out + 8, stride, flipud, fliplr, shift);
    }
}

static INLINE void load_buffer_16x4_sse4_1(const int16_t* input, __m128i* out, int32_t stride, int32_t flipud,
                                           int32_t fliplr, int32_t shift) {
    const int16_t* top_l = input;
    const int16_t* top_r = input + 8;

    if (fliplr) {
        load_buffer_8x4_sse4_1(top_r, out, stride, flipud, fliplr, shift);
        load_buffer_8x4_sse4_1(top_l, out + 8, stride, flipud, fliplr, shift);
    } else {
        load_buffer_8x4_sse4_1(top_l, out, stride, flipud, fliplr, shift);
        load_buffer_8x4_sse4_1(top_r, out + 8, stride, flipud, fliplr, shift);
    }
}

static AOM_FORCE_INLINE void load_buffer_16x16_N2(const int16_t* input, __m128i* out, int32_t stride, int32_t flipud,
                                                  int32_t fliplr, int32_t shift) {
    // Load 4 8x8 blocks
    const int16_t* top_l = input;
    const int16_t* top_r = input + 8;
    const int16_t* bot_l = input + 8 * stride;
    const int16_t* bot_r = input + 8 * stride + 8;

    const int16_t* tmp;

    if (flipud) {
        // Swap left columns
        tmp   = top_l;
        top_l = bot_l;
        bot_l = tmp;
        // Swap right columns
        tmp   = top_r;
        top_r = bot_r;
        bot_r = tmp;
    }

    if (fliplr) {
        // Swap top rows
        tmp   = top_l;
        top_l = top_r;
        top_r = tmp;
        // Swap bottom rows
        tmp   = bot_l;
        bot_l = bot_r;
        bot_r = tmp;
    }

    // load first 8 columns
    load_buffer_8x8_N2(top_l, out, stride, flipud, fliplr, shift);
    load_buffer_8x8_N2(bot_l, &out[32], stride, flipud, fliplr, shift);
}

static AOM_FORCE_INLINE void load_buffer_16x16_N2_H(const int16_t* input, __m128i* out, int32_t stride, int32_t flipud,
                                                    int32_t fliplr, int32_t shift) {
    // Load 4 8x8 blocks
    const int16_t* top_l = input;
    const int16_t* top_r = input + 8;
    const int16_t* bot_l = input + 8 * stride;
    const int16_t* bot_r = input + 8 * stride + 8;

    const int16_t* tmp;

    if (flipud) {
        // Swap left columns
        tmp   = top_l;
        top_l = bot_l;
        bot_l = tmp;
        // Swap right columns
        tmp   = top_r;
        top_r = bot_r;
        bot_r = tmp;
    }

    if (fliplr) {
        // Swap top rows
        tmp   = top_l;
        top_l = top_r;
        top_r = tmp;
        // Swap bottom rows
        tmp   = bot_l;
        bot_l = bot_r;
        bot_r = tmp;
    }

    // load first 8 columns
    load_buffer_8x8_N2(top_l, out, stride, flipud, fliplr, shift);

    //// load second 8 columns
    load_buffer_8x8_N2(top_r, &out[2], stride, flipud, fliplr, shift);
}

static AOM_FORCE_INLINE void load_buffer_16x16_N2_half(const int16_t* input, __m128i* out, int32_t stride,
                                                       int32_t flipud, int32_t fliplr, int32_t shift) {
    // Load 4 8x8 blocks
    const int16_t* top_l = input;
    const int16_t* top_r = input + 8;
    const int16_t* bot_l = input + 8 * stride;
    const int16_t* bot_r = input + 8 * stride + 8;

    const int16_t* tmp;

    if (flipud) {
        // Swap left columns
        tmp   = top_l;
        top_l = bot_l;
        bot_l = tmp;
        // Swap right columns
        tmp   = top_r;
        top_r = bot_r;
        bot_r = tmp;
    }

    if (fliplr) {
        // Swap top rows
        tmp   = top_l;
        top_l = top_r;
        top_r = tmp;
        // Swap bottom rows
        tmp   = bot_l;
        bot_l = bot_r;
        bot_r = tmp;
    }

    // load first 8 columns
    load_buffer_8x8_N2(top_l, out /*& in[0]*/, stride, flipud, fliplr, shift);
}

static void av1_idtx32_new_sse4_1(const __m128i* input, __m128i* output, int8_t cos_bit, const int32_t col_num) {
    (void)cos_bit;
    for (int32_t i = 0; i < 32; i++) {
        output[2 * i * col_num]     = _mm_slli_epi32(input[2 * i * col_num], 2);
        output[2 * i * col_num + 1] = _mm_slli_epi32(input[2 * i * col_num + 1], 2);
    }
}

static INLINE void load_buffer_32x32_sse4_1(const int16_t* input, __m128i* output, int32_t stride) {
    int32_t i;

    for (i = 0; i < 32; ++i) {
        output[0] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 0 * 4)));
        output[1] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 1 * 4)));
        output[2] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 2 * 4)));
        output[3] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 3 * 4)));
        output[4] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 4 * 4)));
        output[5] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 5 * 4)));
        output[6] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 6 * 4)));
        output[7] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 7 * 4)));

        input += stride;
        output += 8;
    }
}

static INLINE void load_buffer_32x16_sse4_1(const int16_t* input, __m128i* output, int32_t stride) {
    int32_t i;

    for (i = 0; i < 16; ++i) {
        output[0] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 0 * 4)));
        output[1] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 1 * 4)));
        output[2] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 2 * 4)));
        output[3] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 3 * 4)));
        output[4] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 4 * 4)));
        output[5] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 5 * 4)));
        output[6] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 6 * 4)));
        output[7] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 7 * 4)));

        input += stride;
        output += 8;
    }
}

static INLINE void load_buffer_32x16_N2_sse4_1(const int16_t* input, __m128i* output, int32_t stride) {
    int32_t i;

    for (i = 0; i < 16; ++i) {
        output[0] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 0 * 4)));
        output[1] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 1 * 4)));
        output[2] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 2 * 4)));
        output[3] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 3 * 4)));

        input += stride;
        output += 8;
    }
}

static INLINE void load_buffer_32_sse4_1(const int16_t* input, __m128i* in, int32_t stride, int32_t flipud,
                                         int32_t fliplr, int32_t shift) {
    __m128i temp[8];
    if (!flipud) {
        temp[0] = _mm_loadl_epi64((const __m128i*)(input + 0 * stride));
        temp[1] = _mm_loadl_epi64((const __m128i*)(input + 0 * stride + 4));
        temp[2] = _mm_loadl_epi64((const __m128i*)(input + 1 * stride));
        temp[3] = _mm_loadl_epi64((const __m128i*)(input + 1 * stride + 4));
        temp[4] = _mm_loadl_epi64((const __m128i*)(input + 2 * stride));
        temp[5] = _mm_loadl_epi64((const __m128i*)(input + 2 * stride + 4));
        temp[6] = _mm_loadl_epi64((const __m128i*)(input + 3 * stride));
        temp[7] = _mm_loadl_epi64((const __m128i*)(input + 3 * stride + 4));
    } else {
        temp[0] = _mm_loadl_epi64((const __m128i*)(input + 3 * stride));
        temp[1] = _mm_loadl_epi64((const __m128i*)(input + 3 * stride + 4));
        temp[2] = _mm_loadl_epi64((const __m128i*)(input + 2 * stride));
        temp[3] = _mm_loadl_epi64((const __m128i*)(input + 2 * stride + 4));
        temp[4] = _mm_loadl_epi64((const __m128i*)(input + 1 * stride));
        temp[5] = _mm_loadl_epi64((const __m128i*)(input + 1 * stride + 4));
        temp[6] = _mm_loadl_epi64((const __m128i*)(input + 0 * stride));
        temp[7] = _mm_loadl_epi64((const __m128i*)(input + 0 * stride + 4));
    }

    if (fliplr) {
        REVERSE_FLIP_LR_8_SSE4_1(temp, 0, 1);
        REVERSE_FLIP_LR_8_SSE4_1(temp, 2, 3);
        REVERSE_FLIP_LR_8_SSE4_1(temp, 4, 5);
        REVERSE_FLIP_LR_8_SSE4_1(temp, 6, 7);
    }

    in[0] = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[0]), shift);
    in[1] = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[1]), shift);
    in[2] = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[2]), shift);
    in[3] = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[3]), shift);
    in[4] = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[4]), shift);
    in[5] = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[5]), shift);
    in[6] = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[6]), shift);
    in[7] = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[7]), shift);
}

static INLINE void load_buffer_16_sse4_1(const int16_t* input, __m128i* in, int32_t stride, int32_t flipud,
                                         int32_t fliplr, int32_t shift) {
    __m128i temp[4];
    if (!flipud) {
        temp[0] = _mm_loadl_epi64((const __m128i*)(input + 0 * stride));
        temp[1] = _mm_loadl_epi64((const __m128i*)(input + 0 * stride + 4));
        temp[2] = _mm_loadl_epi64((const __m128i*)(input + 1 * stride));
        temp[3] = _mm_loadl_epi64((const __m128i*)(input + 1 * stride + 4));
    } else {
        temp[0] = _mm_loadl_epi64((const __m128i*)(input + 1 * stride));
        temp[1] = _mm_loadl_epi64((const __m128i*)(input + 1 * stride + 4));
        temp[2] = _mm_loadl_epi64((const __m128i*)(input + 0 * stride));
        temp[3] = _mm_loadl_epi64((const __m128i*)(input + 0 * stride + 4));
    }

    if (fliplr) {
        REVERSE_FLIP_LR_8_SSE4_1(temp, 0, 1);
        REVERSE_FLIP_LR_8_SSE4_1(temp, 2, 3);
    }

    in[0] = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[0]), shift);
    in[1] = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[1]), shift);
    in[2] = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[2]), shift);
    in[3] = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[3]), shift);
}

static AOM_FORCE_INLINE void load_buffer_16x8n(const int16_t* input, __m128i* out, int32_t stride, int32_t flipud,
                                               int32_t fliplr, int32_t shift, const int32_t height) {
    for (int32_t col = 0; col < height; col++) {
        const int16_t* in     = input + col * stride;
        __m128i*       output = out + 2 * col * 4;
        load_buffer_16_sse4_1(in, output, 8, flipud, fliplr, shift);
    }
}

static INLINE void transpose_16x16_in_64x64_sse4_1(const __m128i* in, __m128i* out) {
    TRANSPOSE_2X4X4_SSE4_1(in, 0, 16, 32, 48, out, 0, 64, 16, 80, 32, 96, 48, 112);
    TRANSPOSE_2X4X4_SSE4_1(in, 64, 80, 96, 112, out, 1, 65, 17, 81, 33, 97, 49, 113);
    TRANSPOSE_2X4X4_SSE4_1(in, 2, 18, 34, 50, out, 128, 192, 144, 208, 160, 224, 176, 240);
    TRANSPOSE_2X4X4_SSE4_1(in, 66, 82, 98, 114, out, 129, 193, 145, 209, 161, 225, 177, 241);

    TRANSPOSE_2X4X4_SSE4_1(in, 128, 144, 160, 176, out, 2, 66, 18, 82, 34, 98, 50, 114);
    TRANSPOSE_2X4X4_SSE4_1(in, 192, 208, 224, 240, out, 3, 67, 19, 83, 35, 99, 51, 115);
    TRANSPOSE_2X4X4_SSE4_1(in, 130, 146, 162, 178, out, 130, 194, 146, 210, 162, 226, 178, 242);
    TRANSPOSE_2X4X4_SSE4_1(in, 194, 210, 226, 242, out, 131, 195, 147, 211, 163, 227, 179, 243);
}

static AOM_FORCE_INLINE void transpose_32x32_in_64x64_sse4_1(const __m128i* in, __m128i* out) {
    transpose_16x16_in_64x64_sse4_1(in, out); //top-left
    transpose_16x16_in_64x64_sse4_1(in + 4, out + 256); //top-right
    transpose_16x16_in_64x64_sse4_1(in + 256, out + 4); //bottom-left
    transpose_16x16_in_64x64_sse4_1(in + 260, out + 260); //bottom-right
}

/*
*    Transpose top left block of size 16x16 in 32x32 block
*/
static INLINE void transpose_16x16_in_32x32_sse4_1(const __m128i* in, __m128i* out) {
    TRANSPOSE_2X4X4_SSE4_1(in, 0, 8, 16, 24, out, 0, 32, 8, 40, 16, 48, 24, 56);
    TRANSPOSE_2X4X4_SSE4_1(in, 32, 40, 48, 56, out, 1, 33, 9, 41, 17, 49, 25, 57);
    TRANSPOSE_2X4X4_SSE4_1(in, 2, 10, 18, 26, out, 64, 96, 72, 104, 80, 112, 88, 120);
    TRANSPOSE_2X4X4_SSE4_1(in, 34, 42, 50, 58, out, 65, 97, 73, 105, 81, 113, 89, 121);

    TRANSPOSE_2X4X4_SSE4_1(in, 64, 72, 80, 88, out, 2, 34, 10, 42, 18, 50, 26, 58);
    TRANSPOSE_2X4X4_SSE4_1(in, 96, 104, 112, 120, out, 3, 35, 11, 43, 19, 51, 27, 59);
    TRANSPOSE_2X4X4_SSE4_1(in, 66, 74, 82, 90, out, 66, 98, 74, 106, 82, 114, 90, 122);
    TRANSPOSE_2X4X4_SSE4_1(in, 98, 106, 114, 122, out, 67, 99, 75, 107, 83, 115, 91, 123);
}

/*
*    Transpose top left block of size 8x8 in 16x16 block
*/
static INLINE void transpose_8x8_in_16x16_sse4_1(const __m128i* in, __m128i* out) {
    TRANSPOSE_2X4X4_SSE4_1(in, 0, 4, 8, 12, out, 0, 16, 4, 20, 8, 24, 12, 28);
    TRANSPOSE_2X4X4_SSE4_1(in, 16, 20, 24, 28, out, 1, 17, 5, 21, 9, 25, 13, 29);
}

static INLINE void transpose_8x8_half_sse4_1(const __m128i* in, __m128i* out) {
    TRANSPOSE_2X4X4_SSE4_1(in, 0, 2, 4, 6, out, 0, 8, 2, 10, 4, 12, 6, 14);
}

static AOM_FORCE_INLINE void transpose_8x8_N2_sse4_1(const __m128i* in, __m128i* out) {
    TRANSPOSE_2X4X4_SSE4_1(in, 0, 2, 4, 6, out, 0, 1, 2, 3, 4, 5, 6, 7);
}

static INLINE void transpose_4x8_in_4x16_sse4_1(const __m128i* in, __m128i* out) {
    __m128i out1[8];
    TRANSPOSE_2X4X4_SSE4_1(in, 0, 2, 4, 6, out1, 0, 1, 2, 3, 4, 5, 6, 7);
    out[0]  = _mm_unpacklo_epi32(out1[0], out1[1]);
    out[1]  = _mm_unpackhi_epi32(out1[0], out1[1]);
    out[4]  = _mm_unpacklo_epi32(out1[2], out1[3]);
    out[5]  = _mm_unpackhi_epi32(out1[2], out1[3]);
    out[8]  = _mm_unpacklo_epi32(out1[4], out1[5]);
    out[9]  = _mm_unpackhi_epi32(out1[4], out1[5]);
    out[12] = _mm_unpacklo_epi32(out1[6], out1[7]);
    out[13] = _mm_unpackhi_epi32(out1[6], out1[7]);
}

static AOM_FORCE_INLINE void transpose_4x8_in_4x16_half_sse4_1(const __m128i* in, __m128i* out) {
    __m128i u0, u1, u2, u3;
    __m128i v0, v1, v2, v3;

    u0 = _mm_unpacklo_epi32(in[0], in[2]);
    u1 = _mm_unpacklo_epi32(in[1], in[3]);
    u2 = _mm_unpacklo_epi32(in[4], in[6]);
    u3 = _mm_unpacklo_epi32(in[5], in[7]);

    v0 = _mm_unpacklo_epi64(u0, u2);
    v1 = _mm_unpacklo_epi64(u1, u3);
    v2 = _mm_unpackhi_epi64(u0, u2);
    v3 = _mm_unpackhi_epi64(u1, u3);

    out[0] = _mm_unpacklo_epi32(v0, v1);
    out[1] = _mm_unpackhi_epi32(v0, v1);
    out[4] = _mm_unpacklo_epi32(v2, v3);
    out[5] = _mm_unpackhi_epi32(v2, v3);
}

static INLINE void load_buffer_4x8_in_8x8(const int16_t* input, __m128i* in, int32_t stride, int32_t flipud,
                                          int32_t fliplr, int32_t shift, int32_t step) {
    __m128i temp[8];
    if (!flipud) {
        temp[0] = _mm_loadl_epi64((const __m128i*)(input + 0 * stride));
        temp[1] = _mm_loadl_epi64((const __m128i*)(input + 0 * stride + 4));
        temp[2] = _mm_loadl_epi64((const __m128i*)(input + 1 * stride));
        temp[3] = _mm_loadl_epi64((const __m128i*)(input + 1 * stride + 4));
        temp[4] = _mm_loadl_epi64((const __m128i*)(input + 2 * stride));
        temp[5] = _mm_loadl_epi64((const __m128i*)(input + 2 * stride + 4));
        temp[6] = _mm_loadl_epi64((const __m128i*)(input + 3 * stride));
        temp[7] = _mm_loadl_epi64((const __m128i*)(input + 3 * stride + 4));
    } else {
        temp[0] = _mm_loadl_epi64((const __m128i*)(input + 7 * stride));
        temp[1] = _mm_loadl_epi64((const __m128i*)(input + 7 * stride + 4));
        temp[2] = _mm_loadl_epi64((const __m128i*)(input + 6 * stride));
        temp[3] = _mm_loadl_epi64((const __m128i*)(input + 6 * stride + 4));
        temp[4] = _mm_loadl_epi64((const __m128i*)(input + 5 * stride));
        temp[5] = _mm_loadl_epi64((const __m128i*)(input + 5 * stride + 4));
        temp[6] = _mm_loadl_epi64((const __m128i*)(input + 4 * stride));
        temp[7] = _mm_loadl_epi64((const __m128i*)(input + 4 * stride + 4));
    }

    if (fliplr) {
        REVERSE_FLIP_LR_8_SSE4_1(temp, 0, 1);
        REVERSE_FLIP_LR_8_SSE4_1(temp, 2, 3);
        REVERSE_FLIP_LR_8_SSE4_1(temp, 4, 5);
        REVERSE_FLIP_LR_8_SSE4_1(temp, 6, 7);
    }

    in[0 * step]     = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[0]), shift);
    in[0 * step + 1] = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[1]), shift);
    in[2 * step]     = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[2]), shift);
    in[2 * step + 1] = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[3]), shift);
    in[4 * step]     = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[4]), shift);
    in[4 * step + 1] = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[5]), shift);
    in[6 * step]     = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[6]), shift);
    in[6 * step + 1] = _mm_slli_epi32(_mm_cvtepi16_epi32(temp[7]), shift);
}

static AOM_FORCE_INLINE void load_buffer_4x16_in_16x16(const int16_t* input, __m128i* out, int32_t stride,
                                                       int32_t flipud, int32_t fliplr, int32_t shift) {
    // Load 2 4x8 blocks
    const int16_t* top_l = input;
    const int16_t* top_r = input + 8;
    const int16_t* bot_l = input + 8 * stride;
    const int16_t* bot_r = input + 8 * stride + 8;

    const int16_t* tmp;

    if (flipud) {
        // Swap left columns
        top_l = bot_l;
        // Swap right columns
        top_r = bot_r;
    }

    if (fliplr) {
        // Swap top rows
        tmp   = top_l;
        top_l = top_r;
        top_r = tmp;
    }

    // load first 4 columns
    load_buffer_4x8_in_8x8(top_l, &out[0], stride, flipud, fliplr, shift, 2);

    // load second 4 columns
    load_buffer_4x8_in_8x8(top_r, &out[2], stride, flipud, fliplr, shift, 2);
}

static AOM_FORCE_INLINE void write_buffer_8x8_N2(const __m128i* res, int32_t* output) {
    const __m128i zero128 = _mm_setzero_si128();

    _mm_storeu_si128((__m128i*)(output + 0 * 4), res[0]);
    _mm_storeu_si128((__m128i*)(output + 2 * 4), res[2]);
    _mm_storeu_si128((__m128i*)(output + 4 * 4), res[4]);
    _mm_storeu_si128((__m128i*)(output + 6 * 4), res[6]);

    _mm_storeu_si128((__m128i*)(output + 1 * 4), zero128);
    _mm_storeu_si128((__m128i*)(output + 3 * 4), zero128);
    _mm_storeu_si128((__m128i*)(output + 5 * 4), zero128);
    _mm_storeu_si128((__m128i*)(output + 7 * 4), zero128);

    _mm_storeu_si128((__m128i*)(output + 8 * 4), zero128);
    _mm_storeu_si128((__m128i*)(output + 9 * 4), zero128);
    _mm_storeu_si128((__m128i*)(output + 10 * 4), zero128);
    _mm_storeu_si128((__m128i*)(output + 11 * 4), zero128);
    _mm_storeu_si128((__m128i*)(output + 12 * 4), zero128);
    _mm_storeu_si128((__m128i*)(output + 13 * 4), zero128);
    _mm_storeu_si128((__m128i*)(output + 14 * 4), zero128);
    _mm_storeu_si128((__m128i*)(output + 15 * 4), zero128);
}

static void fdct8x8_N2_sse4_1(const __m128i* in, __m128i* out, int8_t bit, const int32_t col_num) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospim32 = _mm_set1_epi32(-cospi[32]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospi16  = _mm_set1_epi32(cospi[16]);
    const __m128i  cospi56  = _mm_set1_epi32(cospi[56]);
    const __m128i  cospi8   = _mm_set1_epi32(cospi[8]);
    const __m128i  cospi24  = _mm_set1_epi32(cospi[24]);
    const __m128i  cospi40  = _mm_set1_epi32(cospi[40]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    __m128i        u[16], v[16];

    // stage 0
    // stage 1
    u[0] = _mm_add_epi32(in[2 * 0 * col_num], in[2 * 7 * col_num]);
    u[1] = _mm_add_epi32(in[2 * 0 * col_num + 1], in[2 * 7 * col_num + 1]);

    v[2 * 7]     = _mm_sub_epi32(in[2 * 0 * col_num], in[2 * 7 * col_num]);
    v[2 * 7 + 1] = _mm_sub_epi32(in[2 * 0 * col_num + 1], in[2 * 7 * col_num + 1]);

    u[2 * 1]     = _mm_add_epi32(in[2 * 1 * col_num], in[2 * 6 * col_num]);
    u[2 * 1 + 1] = _mm_add_epi32(in[2 * 1 * col_num + 1], in[2 * 6 * col_num + 1]);

    u[2 * 6]     = _mm_sub_epi32(in[2 * 1 * col_num], in[2 * 6 * col_num]);
    u[2 * 6 + 1] = _mm_sub_epi32(in[2 * 1 * col_num + 1], in[2 * 6 * col_num + 1]);
    u[2 * 2]     = _mm_add_epi32(in[2 * 2 * col_num], in[2 * 5 * col_num]);
    u[2 * 2 + 1] = _mm_add_epi32(in[2 * 2 * col_num + 1], in[2 * 5 * col_num + 1]);
    u[2 * 5]     = _mm_sub_epi32(in[2 * 2 * col_num], in[2 * 5 * col_num]);
    u[2 * 5 + 1] = _mm_sub_epi32(in[2 * 2 * col_num + 1], in[2 * 5 * col_num + 1]);
    u[2 * 3]     = _mm_add_epi32(in[2 * 3 * col_num], in[2 * 4 * col_num]);
    u[2 * 3 + 1] = _mm_add_epi32(in[2 * 3 * col_num + 1], in[2 * 4 * col_num + 1]);
    v[2 * 4]     = _mm_sub_epi32(in[2 * 3 * col_num], in[2 * 4 * col_num]);
    v[2 * 4 + 1] = _mm_sub_epi32(in[2 * 3 * col_num + 1], in[2 * 4 * col_num + 1]);

    // stage 2
    v[2 * 0]     = _mm_add_epi32(u[2 * 0], u[2 * 3]);
    v[2 * 0 + 1] = _mm_add_epi32(u[2 * 0 + 1], u[2 * 3 + 1]);
    v[2 * 3]     = _mm_sub_epi32(u[2 * 0], u[2 * 3]);
    v[2 * 3 + 1] = _mm_sub_epi32(u[2 * 0 + 1], u[2 * 3 + 1]);
    v[2 * 1]     = _mm_add_epi32(u[2 * 1], u[2 * 2]);
    v[2 * 1 + 1] = _mm_add_epi32(u[2 * 1 + 1], u[2 * 2 + 1]);
    v[2 * 2]     = _mm_sub_epi32(u[2 * 1], u[2 * 2]);
    v[2 * 2 + 1] = _mm_sub_epi32(u[2 * 1 + 1], u[2 * 2 + 1]);

    v[2 * 5]     = _mm_mullo_epi32(u[2 * 5], cospim32);
    v[2 * 5 + 1] = _mm_mullo_epi32(u[2 * 5 + 1], cospim32);
    v[2 * 6]     = _mm_mullo_epi32(u[2 * 6], cospi32);
    v[2 * 6 + 1] = _mm_mullo_epi32(u[2 * 6 + 1], cospi32);
    v[2 * 5]     = _mm_add_epi32(v[2 * 5], v[2 * 6]);
    v[2 * 5 + 1] = _mm_add_epi32(v[2 * 5 + 1], v[2 * 6 + 1]);
    v[2 * 5]     = _mm_add_epi32(v[2 * 5], rnding);
    v[2 * 5 + 1] = _mm_add_epi32(v[2 * 5 + 1], rnding);
    v[2 * 5]     = _mm_srai_epi32(v[2 * 5], bit);
    v[2 * 5 + 1] = _mm_srai_epi32(v[2 * 5 + 1], bit);

    u[2 * 0]     = _mm_mullo_epi32(u[2 * 5], cospi32);
    u[2 * 0 + 1] = _mm_mullo_epi32(u[2 * 5 + 1], cospi32);
    v[2 * 6]     = _mm_mullo_epi32(u[2 * 6], cospim32);
    v[2 * 6 + 1] = _mm_mullo_epi32(u[2 * 6 + 1], cospim32);
    v[2 * 6]     = _mm_sub_epi32(u[2 * 0], v[2 * 6]);
    v[2 * 6 + 1] = _mm_sub_epi32(u[2 * 0 + 1], v[2 * 6 + 1]);
    v[2 * 6]     = _mm_add_epi32(v[2 * 6], rnding);
    v[2 * 6 + 1] = _mm_add_epi32(v[2 * 6 + 1], rnding);
    v[2 * 6]     = _mm_srai_epi32(v[2 * 6], bit);
    v[2 * 6 + 1] = _mm_srai_epi32(v[2 * 6 + 1], bit);

    // stage 3
    // type 0
    v[2 * 0]     = _mm_mullo_epi32(v[2 * 0], cospi32);
    v[2 * 0 + 1] = _mm_mullo_epi32(v[2 * 0 + 1], cospi32);
    v[2 * 1]     = _mm_mullo_epi32(v[2 * 1], cospi32);
    v[2 * 1 + 1] = _mm_mullo_epi32(v[2 * 1 + 1], cospi32);
    u[2 * 0]     = _mm_add_epi32(v[2 * 0], v[2 * 1]);
    u[2 * 0 + 1] = _mm_add_epi32(v[2 * 0 + 1], v[2 * 1 + 1]);
    u[2 * 0]     = _mm_add_epi32(u[2 * 0], rnding);
    u[2 * 0 + 1] = _mm_add_epi32(u[2 * 0 + 1], rnding);
    u[2 * 0]     = _mm_srai_epi32(u[2 * 0], bit);
    u[2 * 0 + 1] = _mm_srai_epi32(u[2 * 0 + 1], bit);

    // type 1
    v[2 * 0]     = _mm_mullo_epi32(v[2 * 2], cospi48);
    v[2 * 0 + 1] = _mm_mullo_epi32(v[2 * 2 + 1], cospi48);
    v[2 * 1]     = _mm_mullo_epi32(v[2 * 3], cospi16);
    v[2 * 1 + 1] = _mm_mullo_epi32(v[2 * 3 + 1], cospi16);
    u[2 * 2]     = _mm_add_epi32(v[2 * 0], v[2 * 1]);
    u[2 * 2 + 1] = _mm_add_epi32(v[2 * 0 + 1], v[2 * 1 + 1]);
    u[2 * 2]     = _mm_add_epi32(u[2 * 2], rnding);
    u[2 * 2 + 1] = _mm_add_epi32(u[2 * 2 + 1], rnding);
    u[2 * 2]     = _mm_srai_epi32(u[2 * 2], bit);
    u[2 * 2 + 1] = _mm_srai_epi32(u[2 * 2 + 1], bit);

    u[2 * 4]     = _mm_add_epi32(v[2 * 4], v[2 * 5]);
    u[2 * 4 + 1] = _mm_add_epi32(v[2 * 4 + 1], v[2 * 5 + 1]);
    u[2 * 5]     = _mm_sub_epi32(v[2 * 4], v[2 * 5]);
    u[2 * 5 + 1] = _mm_sub_epi32(v[2 * 4 + 1], v[2 * 5 + 1]);
    u[2 * 6]     = _mm_sub_epi32(v[2 * 7], v[2 * 6]);
    u[2 * 6 + 1] = _mm_sub_epi32(v[2 * 7 + 1], v[2 * 6 + 1]);
    u[2 * 7]     = _mm_add_epi32(v[2 * 7], v[2 * 6]);
    u[2 * 7 + 1] = _mm_add_epi32(v[2 * 7 + 1], v[2 * 6 + 1]);

    // stage 4
    // stage 5
    v[2 * 0]                 = _mm_mullo_epi32(u[2 * 4], cospi56);
    v[2 * 0 + 1]             = _mm_mullo_epi32(u[2 * 4 + 1], cospi56);
    v[2 * 1]                 = _mm_mullo_epi32(u[2 * 7], cospi8);
    v[2 * 1 + 1]             = _mm_mullo_epi32(u[2 * 7 + 1], cospi8);
    v[2 * 0]                 = _mm_add_epi32(v[2 * 0], v[2 * 1]);
    v[2 * 0 + 1]             = _mm_add_epi32(v[2 * 0 + 1], v[2 * 1 + 1]);
    v[2 * 0]                 = _mm_add_epi32(v[2 * 0], rnding);
    v[2 * 0 + 1]             = _mm_add_epi32(v[2 * 0 + 1], rnding);
    out[2 * 1 * col_num]     = _mm_srai_epi32(v[2 * 0], bit);
    out[2 * 1 * col_num + 1] = _mm_srai_epi32(v[2 * 0 + 1], bit);

    v[2 * 0]                 = _mm_mullo_epi32(u[2 * 5], cospi40);
    v[2 * 0 + 1]             = _mm_mullo_epi32(u[2 * 5 + 1], cospi40);
    v[2 * 1]                 = _mm_mullo_epi32(u[2 * 6], cospi24);
    v[2 * 1 + 1]             = _mm_mullo_epi32(u[2 * 6 + 1], cospi24);
    v[2 * 0]                 = _mm_sub_epi32(v[2 * 1], v[2 * 0]);
    v[2 * 0 + 1]             = _mm_sub_epi32(v[2 * 1 + 1], v[2 * 0 + 1]);
    v[2 * 0]                 = _mm_add_epi32(v[2 * 0], rnding);
    v[2 * 0 + 1]             = _mm_add_epi32(v[2 * 0 + 1], rnding);
    out[2 * 3 * col_num]     = _mm_srai_epi32(v[2 * 0], bit);
    out[2 * 3 * col_num + 1] = _mm_srai_epi32(v[2 * 0 + 1], bit);

    out[2 * 0 * col_num]     = u[2 * 0];
    out[2 * 0 * col_num + 1] = u[2 * 0 + 1];
    out[2 * 2 * col_num]     = u[2 * 2];
    out[2 * 2 * col_num + 1] = u[2 * 2 + 1];
}

static void fadst8x8_N2_sse4_1(const __m128i* in, __m128i* out, int8_t bit, const int32_t col_num) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospi16  = _mm_set1_epi32(cospi[16]);
    const __m128i  cospim16 = _mm_set1_epi32(-cospi[16]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospim48 = _mm_set1_epi32(-cospi[48]);
    const __m128i  cospim4  = _mm_set1_epi32(-cospi[4]);
    const __m128i  cospi60  = _mm_set1_epi32(cospi[60]);
    const __m128i  cospim20 = _mm_set1_epi32(-cospi[20]);
    const __m128i  cospi44  = _mm_set1_epi32(cospi[44]);
    const __m128i  cospi28  = _mm_set1_epi32(cospi[28]);
    const __m128i  cospi36  = _mm_set1_epi32(cospi[36]);
    const __m128i  cospi52  = _mm_set1_epi32(cospi[52]);
    const __m128i  cospi12  = _mm_set1_epi32(cospi[12]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    const __m128i  zero     = _mm_setzero_si128();
    __m128i        u0, u1, u2, u3, u4, u5, u6, u7, u8, u9, u10, u11, u12, u13, u14, u15;
    __m128i        v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, v10, v11, v12, v13, v14, v15;
    __m128i        x0, x1, y0, y1;

    // stage 1
    v0  = in[2 * 0 * col_num];
    v1  = in[2 * 0 * col_num + 1];
    v2  = _mm_sub_epi32(zero, in[2 * 7 * col_num]);
    v3  = _mm_sub_epi32(zero, in[2 * 7 * col_num + 1]);
    u4  = _mm_sub_epi32(zero, in[2 * 3 * col_num]);
    u5  = _mm_sub_epi32(zero, in[2 * 3 * col_num + 1]);
    u6  = in[2 * 4 * col_num];
    u7  = in[2 * 4 * col_num + 1];
    v8  = _mm_sub_epi32(zero, in[2 * 1 * col_num]);
    v9  = _mm_sub_epi32(zero, in[2 * 1 * col_num + 1]);
    v10 = in[2 * 6 * col_num];
    v11 = in[2 * 6 * col_num + 1];
    u12 = in[2 * 2 * col_num];
    u13 = in[2 * 2 * col_num + 1];
    u14 = _mm_sub_epi32(zero, in[2 * 5 * col_num]);
    u15 = _mm_sub_epi32(zero, in[2 * 5 * col_num + 1]);

    // stage 2
    x0 = _mm_mullo_epi32(u4, cospi32);
    x1 = _mm_mullo_epi32(u5, cospi32);
    y0 = _mm_mullo_epi32(u6, cospi32);
    y1 = _mm_mullo_epi32(u7, cospi32);
    v4 = _mm_add_epi32(x0, y0);
    v5 = _mm_add_epi32(x1, y1);
    v4 = _mm_add_epi32(v4, rnding);
    v5 = _mm_add_epi32(v5, rnding);
    v4 = _mm_srai_epi32(v4, bit);
    v5 = _mm_srai_epi32(v5, bit);

    v6 = _mm_sub_epi32(x0, y0);
    v7 = _mm_sub_epi32(x1, y1);
    v6 = _mm_add_epi32(v6, rnding);
    v7 = _mm_add_epi32(v7, rnding);
    v6 = _mm_srai_epi32(v6, bit);
    v7 = _mm_srai_epi32(v7, bit);

    x0  = _mm_mullo_epi32(u12, cospi32);
    x1  = _mm_mullo_epi32(u13, cospi32);
    y0  = _mm_mullo_epi32(u14, cospi32);
    y1  = _mm_mullo_epi32(u15, cospi32);
    v12 = _mm_add_epi32(x0, y0);
    v13 = _mm_add_epi32(x1, y1);
    v12 = _mm_add_epi32(v12, rnding);
    v13 = _mm_add_epi32(v13, rnding);
    v12 = _mm_srai_epi32(v12, bit);
    v13 = _mm_srai_epi32(v13, bit);

    v14 = _mm_sub_epi32(x0, y0);
    v15 = _mm_sub_epi32(x1, y1);
    v14 = _mm_add_epi32(v14, rnding);
    v15 = _mm_add_epi32(v15, rnding);
    v14 = _mm_srai_epi32(v14, bit);
    v15 = _mm_srai_epi32(v15, bit);

    // stage 3
    u0  = _mm_add_epi32(v0, v4);
    u1  = _mm_add_epi32(v1, v5);
    u2  = _mm_add_epi32(v2, v6);
    u3  = _mm_add_epi32(v3, v7);
    u4  = _mm_sub_epi32(v0, v4);
    u5  = _mm_sub_epi32(v1, v5);
    u6  = _mm_sub_epi32(v2, v6);
    u7  = _mm_sub_epi32(v3, v7);
    u8  = _mm_add_epi32(v8, v12);
    u9  = _mm_add_epi32(v9, v13);
    u10 = _mm_add_epi32(v10, v14);
    u11 = _mm_add_epi32(v11, v15);
    u12 = _mm_sub_epi32(v8, v12);
    u13 = _mm_sub_epi32(v9, v13);
    u14 = _mm_sub_epi32(v10, v14);
    u15 = _mm_sub_epi32(v11, v15);

    // stage 4
    v0 = u0;
    v1 = u1;
    v2 = u2;
    v3 = u3;
    v4 = u4;
    v5 = u5;
    v6 = u6;
    v7 = u7;

    x0 = _mm_mullo_epi32(u8, cospi16);
    x1 = _mm_mullo_epi32(u9, cospi16);
    y0 = _mm_mullo_epi32(u10, cospi48);
    y1 = _mm_mullo_epi32(u11, cospi48);
    v8 = _mm_add_epi32(x0, y0);
    v9 = _mm_add_epi32(x1, y1);
    v8 = _mm_add_epi32(v8, rnding);
    v9 = _mm_add_epi32(v9, rnding);
    v8 = _mm_srai_epi32(v8, bit);
    v9 = _mm_srai_epi32(v9, bit);

    x0  = _mm_mullo_epi32(u8, cospi48);
    x1  = _mm_mullo_epi32(u9, cospi48);
    y0  = _mm_mullo_epi32(u10, cospim16);
    y1  = _mm_mullo_epi32(u11, cospim16);
    v10 = _mm_add_epi32(x0, y0);
    v11 = _mm_add_epi32(x1, y1);
    v10 = _mm_add_epi32(v10, rnding);
    v11 = _mm_add_epi32(v11, rnding);
    v10 = _mm_srai_epi32(v10, bit);
    v11 = _mm_srai_epi32(v11, bit);

    x0  = _mm_mullo_epi32(u12, cospim48);
    x1  = _mm_mullo_epi32(u13, cospim48);
    y0  = _mm_mullo_epi32(u14, cospi16);
    y1  = _mm_mullo_epi32(u15, cospi16);
    v12 = _mm_add_epi32(x0, y0);
    v13 = _mm_add_epi32(x1, y1);
    v12 = _mm_add_epi32(v12, rnding);
    v13 = _mm_add_epi32(v13, rnding);
    v12 = _mm_srai_epi32(v12, bit);
    v13 = _mm_srai_epi32(v13, bit);

    x0  = _mm_mullo_epi32(u12, cospi16);
    x1  = _mm_mullo_epi32(u13, cospi16);
    y0  = _mm_mullo_epi32(u14, cospi48);
    y1  = _mm_mullo_epi32(u15, cospi48);
    v14 = _mm_add_epi32(x0, y0);
    v15 = _mm_add_epi32(x1, y1);
    v14 = _mm_add_epi32(v14, rnding);
    v15 = _mm_add_epi32(v15, rnding);
    v14 = _mm_srai_epi32(v14, bit);
    v15 = _mm_srai_epi32(v15, bit);

    // stage 5
    u0  = _mm_add_epi32(v0, v8);
    u1  = _mm_add_epi32(v1, v9);
    u2  = _mm_add_epi32(v2, v10);
    u3  = _mm_add_epi32(v3, v11);
    u4  = _mm_add_epi32(v4, v12);
    u5  = _mm_add_epi32(v5, v13);
    u6  = _mm_add_epi32(v6, v14);
    u7  = _mm_add_epi32(v7, v15);
    u8  = _mm_sub_epi32(v0, v8);
    u9  = _mm_sub_epi32(v1, v9);
    u10 = _mm_sub_epi32(v2, v10);
    u11 = _mm_sub_epi32(v3, v11);
    u12 = _mm_sub_epi32(v4, v12);
    u13 = _mm_sub_epi32(v5, v13);
    u14 = _mm_sub_epi32(v6, v14);
    u15 = _mm_sub_epi32(v7, v15);

    // stage 6
    x0 = _mm_mullo_epi32(u0, cospi60);
    x1 = _mm_mullo_epi32(u1, cospi60);
    y0 = _mm_mullo_epi32(u2, cospim4);
    y1 = _mm_mullo_epi32(u3, cospim4);
    v2 = _mm_add_epi32(x0, y0);
    v3 = _mm_add_epi32(x1, y1);
    v2 = _mm_add_epi32(v2, rnding);
    v3 = _mm_add_epi32(v3, rnding);
    v2 = _mm_srai_epi32(v2, bit);
    v3 = _mm_srai_epi32(v3, bit);

    x0 = _mm_mullo_epi32(u4, cospi44);
    x1 = _mm_mullo_epi32(u5, cospi44);
    y0 = _mm_mullo_epi32(u6, cospim20);
    y1 = _mm_mullo_epi32(u7, cospim20);
    v6 = _mm_add_epi32(x0, y0);
    v7 = _mm_add_epi32(x1, y1);
    v6 = _mm_add_epi32(v6, rnding);
    v7 = _mm_add_epi32(v7, rnding);
    v6 = _mm_srai_epi32(v6, bit);
    v7 = _mm_srai_epi32(v7, bit);

    x0 = _mm_mullo_epi32(u8, cospi36);
    x1 = _mm_mullo_epi32(u9, cospi36);
    y0 = _mm_mullo_epi32(u10, cospi28);
    y1 = _mm_mullo_epi32(u11, cospi28);
    v8 = _mm_add_epi32(x0, y0);
    v9 = _mm_add_epi32(x1, y1);
    v8 = _mm_add_epi32(v8, rnding);
    v9 = _mm_add_epi32(v9, rnding);
    v8 = _mm_srai_epi32(v8, bit);
    v9 = _mm_srai_epi32(v9, bit);

    x0  = _mm_mullo_epi32(u12, cospi52);
    x1  = _mm_mullo_epi32(u13, cospi52);
    y0  = _mm_mullo_epi32(u14, cospi12);
    y1  = _mm_mullo_epi32(u15, cospi12);
    v12 = _mm_add_epi32(x0, y0);
    v13 = _mm_add_epi32(x1, y1);
    v12 = _mm_add_epi32(v12, rnding);
    v13 = _mm_add_epi32(v13, rnding);
    v12 = _mm_srai_epi32(v12, bit);
    v13 = _mm_srai_epi32(v13, bit);

    // stage 7
    out[2 * 0 * col_num]     = v2;
    out[2 * 0 * col_num + 1] = v3;
    out[2 * 1 * col_num]     = v12;
    out[2 * 1 * col_num + 1] = v13;
    out[2 * 2 * col_num]     = v6;
    out[2 * 2 * col_num + 1] = v7;
    out[2 * 3 * col_num]     = v8;
    out[2 * 3 * col_num + 1] = v9;
}

static void fidtx8x8_N2_sse4_1(const __m128i* in, __m128i* out, int8_t bit, int32_t col_num) {
    (void)bit;
    out[0] = _mm_slli_epi32(in[2 * 0 * col_num], 1);
    out[1] = _mm_slli_epi32(in[2 * 0 * col_num + 1], 1);
    out[2] = _mm_slli_epi32(in[2 * 1 * col_num], 1);
    out[3] = _mm_slli_epi32(in[2 * 1 * col_num + 1], 1);
    out[4] = _mm_slli_epi32(in[2 * 2 * col_num], 1);
    out[5] = _mm_slli_epi32(in[2 * 2 * col_num + 1], 1);
    out[6] = _mm_slli_epi32(in[2 * 3 * col_num], 1);
    out[7] = _mm_slli_epi32(in[2 * 3 * col_num + 1], 1);
}

static AOM_FORCE_INLINE void col_txfm_8x8_N2_rounding(__m128i* in, int32_t shift) {
    const __m128i rounding = _mm_set1_epi32(1 << (shift - 1));

    in[0] = _mm_add_epi32(in[0], rounding);
    in[1] = _mm_add_epi32(in[1], rounding);
    in[2] = _mm_add_epi32(in[2], rounding);
    in[3] = _mm_add_epi32(in[3], rounding);
    in[4] = _mm_add_epi32(in[4], rounding);
    in[5] = _mm_add_epi32(in[5], rounding);
    in[6] = _mm_add_epi32(in[6], rounding);
    in[7] = _mm_add_epi32(in[7], rounding);

    in[0] = _mm_srai_epi32(in[0], shift);
    in[1] = _mm_srai_epi32(in[1], shift);
    in[2] = _mm_srai_epi32(in[2], shift);
    in[3] = _mm_srai_epi32(in[3], shift);
    in[4] = _mm_srai_epi32(in[4], shift);
    in[5] = _mm_srai_epi32(in[5], shift);
    in[6] = _mm_srai_epi32(in[6], shift);
    in[7] = _mm_srai_epi32(in[7], shift);
}

static AOM_FORCE_INLINE void col_txfm_16x16_N2_rounding(__m128i* in, int32_t shift) {
    col_txfm_8x8_rounding(&in[0], shift);
    col_txfm_8x8_rounding(&in[16], shift);
}

static AOM_FORCE_INLINE void col_txfm_32x8_N2_half_rounding(__m128i* in, int32_t shift) {
    const __m128i rounding = _mm_set1_epi32(1 << (shift - 1));
    in[0]                  = _mm_add_epi32(in[0], rounding);
    in[1]                  = _mm_add_epi32(in[1], rounding);
    in[2]                  = _mm_add_epi32(in[2], rounding);
    in[3]                  = _mm_add_epi32(in[3], rounding);
    in[8]                  = _mm_add_epi32(in[8], rounding);
    in[9]                  = _mm_add_epi32(in[9], rounding);
    in[10]                 = _mm_add_epi32(in[10], rounding);
    in[11]                 = _mm_add_epi32(in[11], rounding);
    in[0]                  = _mm_srai_epi32(in[0], shift);
    in[1]                  = _mm_srai_epi32(in[1], shift);
    in[2]                  = _mm_srai_epi32(in[2], shift);
    in[3]                  = _mm_srai_epi32(in[3], shift);
    in[8]                  = _mm_srai_epi32(in[8], shift);
    in[9]                  = _mm_srai_epi32(in[9], shift);
    in[10]                 = _mm_srai_epi32(in[10], shift);
    in[11]                 = _mm_srai_epi32(in[11], shift);
    in[16]                 = _mm_add_epi32(in[16], rounding);
    in[17]                 = _mm_add_epi32(in[17], rounding);
    in[18]                 = _mm_add_epi32(in[18], rounding);
    in[19]                 = _mm_add_epi32(in[19], rounding);
    in[24]                 = _mm_add_epi32(in[24], rounding);
    in[25]                 = _mm_add_epi32(in[25], rounding);
    in[26]                 = _mm_add_epi32(in[26], rounding);
    in[27]                 = _mm_add_epi32(in[27], rounding);
    in[16]                 = _mm_srai_epi32(in[16], shift);
    in[17]                 = _mm_srai_epi32(in[17], shift);
    in[18]                 = _mm_srai_epi32(in[18], shift);
    in[19]                 = _mm_srai_epi32(in[19], shift);
    in[24]                 = _mm_srai_epi32(in[24], shift);
    in[25]                 = _mm_srai_epi32(in[25], shift);
    in[26]                 = _mm_srai_epi32(in[26], shift);
    in[27]                 = _mm_srai_epi32(in[27], shift);
}

static AOM_FORCE_INLINE void col_txfm_16x16_N2_half_rounding(__m128i* in, int32_t shift) {
    const __m128i rounding = _mm_set1_epi32(1 << (shift - 1));
    in[0]                  = _mm_add_epi32(in[0], rounding);
    in[1]                  = _mm_add_epi32(in[1], rounding);
    in[4]                  = _mm_add_epi32(in[4], rounding);
    in[5]                  = _mm_add_epi32(in[5], rounding);
    in[8]                  = _mm_add_epi32(in[8], rounding);
    in[9]                  = _mm_add_epi32(in[9], rounding);
    in[12]                 = _mm_add_epi32(in[12], rounding);
    in[13]                 = _mm_add_epi32(in[13], rounding);
    in[0]                  = _mm_srai_epi32(in[0], shift);
    in[1]                  = _mm_srai_epi32(in[1], shift);
    in[4]                  = _mm_srai_epi32(in[4], shift);
    in[5]                  = _mm_srai_epi32(in[5], shift);
    in[8]                  = _mm_srai_epi32(in[8], shift);
    in[9]                  = _mm_srai_epi32(in[9], shift);
    in[12]                 = _mm_srai_epi32(in[12], shift);
    in[13]                 = _mm_srai_epi32(in[13], shift);
    in[16]                 = _mm_add_epi32(in[16], rounding);
    in[17]                 = _mm_add_epi32(in[17], rounding);
    in[20]                 = _mm_add_epi32(in[20], rounding);
    in[21]                 = _mm_add_epi32(in[21], rounding);
    in[24]                 = _mm_add_epi32(in[24], rounding);
    in[25]                 = _mm_add_epi32(in[25], rounding);
    in[28]                 = _mm_add_epi32(in[28], rounding);
    in[29]                 = _mm_add_epi32(in[29], rounding);
    in[16]                 = _mm_srai_epi32(in[16], shift);
    in[17]                 = _mm_srai_epi32(in[17], shift);
    in[20]                 = _mm_srai_epi32(in[20], shift);
    in[21]                 = _mm_srai_epi32(in[21], shift);
    in[24]                 = _mm_srai_epi32(in[24], shift);
    in[25]                 = _mm_srai_epi32(in[25], shift);
    in[28]                 = _mm_srai_epi32(in[28], shift);
    in[29]                 = _mm_srai_epi32(in[29], shift);
}

static AOM_FORCE_INLINE void write_buffer_16x16_N2(const __m128i* res, int32_t* output) {
    const __m128i zero = _mm_setzero_si128();
    int32_t       i;
    for (i = 0; i < 4; i++) {
        _mm_storeu_si128((__m128i*)(output + i * 32 + 0 * 4), res[i * 8]);
        _mm_storeu_si128((__m128i*)(output + i * 32 + 1 * 4), res[i * 8 + 1]);
        _mm_storeu_si128((__m128i*)(output + i * 32 + 2 * 4), zero);
        _mm_storeu_si128((__m128i*)(output + i * 32 + 3 * 4), zero);
        _mm_storeu_si128((__m128i*)(output + i * 32 + 4 * 4), res[i * 8 + 4]);
        _mm_storeu_si128((__m128i*)(output + i * 32 + 5 * 4), res[i * 8 + 5]);
        _mm_storeu_si128((__m128i*)(output + i * 32 + 6 * 4), zero);
        _mm_storeu_si128((__m128i*)(output + i * 32 + 7 * 4), zero);
    }
    for (; i < 8; i++) {
        _mm_storeu_si128((__m128i*)(output + i * 32 + 0 * 4), zero);
        _mm_storeu_si128((__m128i*)(output + i * 32 + 1 * 4), zero);
        _mm_storeu_si128((__m128i*)(output + i * 32 + 2 * 4), zero);
        _mm_storeu_si128((__m128i*)(output + i * 32 + 3 * 4), zero);
        _mm_storeu_si128((__m128i*)(output + i * 32 + 4 * 4), zero);
        _mm_storeu_si128((__m128i*)(output + i * 32 + 5 * 4), zero);
        _mm_storeu_si128((__m128i*)(output + i * 32 + 6 * 4), zero);
        _mm_storeu_si128((__m128i*)(output + i * 32 + 7 * 4), zero);
    }
}

static void fdct16x16_N2_sse4_1(const __m128i* in, __m128i* out, int8_t bit, const int32_t col_num, int32_t size) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospim32 = _mm_set1_epi32(-cospi[32]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospi16  = _mm_set1_epi32(cospi[16]);
    const __m128i  cospim48 = _mm_set1_epi32(-cospi[48]);
    const __m128i  cospim16 = _mm_set1_epi32(-cospi[16]);
    const __m128i  cospi56  = _mm_set1_epi32(cospi[56]);
    const __m128i  cospi8   = _mm_set1_epi32(cospi[8]);
    const __m128i  cospi24  = _mm_set1_epi32(cospi[24]);
    const __m128i  cospi40  = _mm_set1_epi32(cospi[40]);
    const __m128i  cospi60  = _mm_set1_epi32(cospi[60]);
    const __m128i  cospi4   = _mm_set1_epi32(cospi[4]);
    const __m128i  cospi28  = _mm_set1_epi32(cospi[28]);
    const __m128i  cospi36  = _mm_set1_epi32(cospi[36]);
    const __m128i  cospi44  = _mm_set1_epi32(cospi[44]);
    const __m128i  cospi20  = _mm_set1_epi32(cospi[20]);
    const __m128i  cospi12  = _mm_set1_epi32(cospi[12]);
    const __m128i  cospi52  = _mm_set1_epi32(cospi[52]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    __m128i        u[32], v[32], x0, x1;
    int32_t        col;

    for (col = 0; col < size; ++col) {
        // stage 0
        // stage 1
        u[0]  = _mm_add_epi32(in[2 * (0 * col_num + col)], in[2 * (15 * col_num + col)]);
        u[1]  = _mm_add_epi32(in[2 * (0 * col_num + col) + 1], in[2 * (15 * col_num + col) + 1]);
        u[30] = _mm_sub_epi32(in[2 * (0 * col_num + col)], in[2 * (15 * col_num + col)]);
        u[31] = _mm_sub_epi32(in[2 * (0 * col_num + col) + 1], in[2 * (15 * col_num + col) + 1]);
        u[2]  = _mm_add_epi32(in[2 * (1 * col_num + col)], in[2 * (14 * col_num + col)]);
        u[3]  = _mm_add_epi32(in[2 * (1 * col_num + col) + 1], in[2 * (14 * col_num + col) + 1]);
        u[28] = _mm_sub_epi32(in[2 * (1 * col_num + col)], in[2 * (14 * col_num + col)]);
        u[29] = _mm_sub_epi32(in[2 * (1 * col_num + col) + 1], in[2 * (14 * col_num + col) + 1]);
        u[4]  = _mm_add_epi32(in[2 * (2 * col_num + col)], in[2 * (13 * col_num + col)]);
        u[5]  = _mm_add_epi32(in[2 * (2 * col_num + col) + 1], in[2 * (13 * col_num + col) + 1]);
        u[26] = _mm_sub_epi32(in[2 * (2 * col_num + col)], in[2 * (13 * col_num + col)]);
        u[27] = _mm_sub_epi32(in[2 * (2 * col_num + col) + 1], in[2 * (13 * col_num + col) + 1]);
        u[6]  = _mm_add_epi32(in[2 * (3 * col_num + col)], in[2 * (12 * col_num + col)]);
        u[7]  = _mm_add_epi32(in[2 * (3 * col_num + col) + 1], in[2 * (12 * col_num + col) + 1]);
        u[24] = _mm_sub_epi32(in[2 * (3 * col_num + col)], in[2 * (12 * col_num + col)]);
        u[25] = _mm_sub_epi32(in[2 * (3 * col_num + col) + 1], in[2 * (12 * col_num + col) + 1]);
        u[8]  = _mm_add_epi32(in[2 * (4 * col_num + col)], in[2 * (11 * col_num + col)]);
        u[9]  = _mm_add_epi32(in[2 * (4 * col_num + col) + 1], in[2 * (11 * col_num + col) + 1]);
        u[22] = _mm_sub_epi32(in[2 * (4 * col_num + col)], in[2 * (11 * col_num + col)]);
        u[23] = _mm_sub_epi32(in[2 * (4 * col_num + col) + 1], in[2 * (11 * col_num + col) + 1]);
        u[10] = _mm_add_epi32(in[2 * (5 * col_num + col)], in[2 * (10 * col_num + col)]);
        u[11] = _mm_add_epi32(in[2 * (5 * col_num + col) + 1], in[2 * (10 * col_num + col) + 1]);
        u[20] = _mm_sub_epi32(in[2 * (5 * col_num + col)], in[2 * (10 * col_num + col)]);
        u[21] = _mm_sub_epi32(in[2 * (5 * col_num + col) + 1], in[2 * (10 * col_num + col) + 1]);
        u[12] = _mm_add_epi32(in[2 * (6 * col_num + col)], in[2 * (9 * col_num + col)]);
        u[13] = _mm_add_epi32(in[2 * (6 * col_num + col) + 1], in[2 * (9 * col_num + col) + 1]);
        u[18] = _mm_sub_epi32(in[2 * (6 * col_num + col)], in[2 * (9 * col_num + col)]);
        u[19] = _mm_sub_epi32(in[2 * (6 * col_num + col) + 1], in[2 * (9 * col_num + col) + 1]);
        u[14] = _mm_add_epi32(in[2 * (7 * col_num + col)], in[2 * (8 * col_num + col)]);
        u[15] = _mm_add_epi32(in[2 * (7 * col_num + col) + 1], in[2 * (8 * col_num + col) + 1]);
        u[16] = _mm_sub_epi32(in[2 * (7 * col_num + col)], in[2 * (8 * col_num + col)]);
        u[17] = _mm_sub_epi32(in[2 * (7 * col_num + col) + 1], in[2 * (8 * col_num + col) + 1]);

        // stage 2
        v[0]  = _mm_add_epi32(u[0], u[14]);
        v[1]  = _mm_add_epi32(u[1], u[15]);
        v[14] = _mm_sub_epi32(u[0], u[14]);
        v[15] = _mm_sub_epi32(u[1], u[15]);
        v[2]  = _mm_add_epi32(u[2], u[12]);
        v[3]  = _mm_add_epi32(u[3], u[13]);
        v[12] = _mm_sub_epi32(u[2], u[12]);
        v[13] = _mm_sub_epi32(u[3], u[13]);
        v[4]  = _mm_add_epi32(u[4], u[10]);
        v[5]  = _mm_add_epi32(u[5], u[11]);
        v[10] = _mm_sub_epi32(u[4], u[10]);
        v[11] = _mm_sub_epi32(u[5], u[11]);
        v[6]  = _mm_add_epi32(u[6], u[8]);
        v[7]  = _mm_add_epi32(u[7], u[9]);
        v[8]  = _mm_sub_epi32(u[6], u[8]);
        v[9]  = _mm_sub_epi32(u[7], u[9]);
        v[16] = u[16];
        v[17] = u[17];
        v[18] = u[18];
        v[19] = u[19];

        v[20] = _mm_mullo_epi32(u[20], cospim32);
        v[21] = _mm_mullo_epi32(u[21], cospim32);
        x0    = _mm_mullo_epi32(u[26], cospi32);
        x1    = _mm_mullo_epi32(u[27], cospi32);
        v[20] = _mm_add_epi32(v[20], x0);
        v[21] = _mm_add_epi32(v[21], x1);
        v[20] = _mm_add_epi32(v[20], rnding);
        v[21] = _mm_add_epi32(v[21], rnding);
        v[20] = _mm_srai_epi32(v[20], bit);
        v[21] = _mm_srai_epi32(v[21], bit);

        v[26] = _mm_mullo_epi32(u[20], cospi32);
        v[27] = _mm_mullo_epi32(u[21], cospi32);
        x0    = _mm_mullo_epi32(u[26], cospim32);
        x1    = _mm_mullo_epi32(u[27], cospim32);
        v[26] = _mm_sub_epi32(v[26], x0);
        v[27] = _mm_sub_epi32(v[27], x1);
        v[26] = _mm_add_epi32(v[26], rnding);
        v[27] = _mm_add_epi32(v[27], rnding);
        v[26] = _mm_srai_epi32(v[26], bit);
        v[27] = _mm_srai_epi32(v[27], bit);

        v[22] = _mm_mullo_epi32(u[22], cospim32);
        v[23] = _mm_mullo_epi32(u[23], cospim32);
        x0    = _mm_mullo_epi32(u[24], cospi32);
        x1    = _mm_mullo_epi32(u[25], cospi32);
        v[22] = _mm_add_epi32(v[22], x0);
        v[23] = _mm_add_epi32(v[23], x1);
        v[22] = _mm_add_epi32(v[22], rnding);
        v[23] = _mm_add_epi32(v[23], rnding);
        v[22] = _mm_srai_epi32(v[22], bit);
        v[23] = _mm_srai_epi32(v[23], bit);

        v[24] = _mm_mullo_epi32(u[22], cospi32);
        v[25] = _mm_mullo_epi32(u[23], cospi32);
        x0    = _mm_mullo_epi32(u[24], cospim32);
        x1    = _mm_mullo_epi32(u[25], cospim32);
        v[24] = _mm_sub_epi32(v[24], x0);
        v[25] = _mm_sub_epi32(v[25], x1);
        v[24] = _mm_add_epi32(v[24], rnding);
        v[25] = _mm_add_epi32(v[25], rnding);
        v[24] = _mm_srai_epi32(v[24], bit);
        v[25] = _mm_srai_epi32(v[25], bit);
        v[28] = u[28];
        v[29] = u[29];
        v[30] = u[30];
        v[31] = u[31];

        // stage 3
        u[0] = _mm_add_epi32(v[0], v[6]);
        u[1] = _mm_add_epi32(v[1], v[7]);
        u[6] = _mm_sub_epi32(v[0], v[6]);
        u[7] = _mm_sub_epi32(v[1], v[7]);
        u[2] = _mm_add_epi32(v[2], v[4]);
        u[3] = _mm_add_epi32(v[3], v[5]);
        u[4] = _mm_sub_epi32(v[2], v[4]);
        u[5] = _mm_sub_epi32(v[3], v[5]);
        u[8] = v[8];
        u[9] = v[9];

        u[10] = _mm_mullo_epi32(v[10], cospim32);
        u[11] = _mm_mullo_epi32(v[11], cospim32);
        x0    = _mm_mullo_epi32(v[12], cospi32);
        x1    = _mm_mullo_epi32(v[13], cospi32);
        u[10] = _mm_add_epi32(u[10], x0);
        u[11] = _mm_add_epi32(u[11], x1);
        u[10] = _mm_add_epi32(u[10], rnding);
        u[11] = _mm_add_epi32(u[11], rnding);
        u[10] = _mm_srai_epi32(u[10], bit);
        u[11] = _mm_srai_epi32(u[11], bit);

        u[12] = _mm_mullo_epi32(v[10], cospi32);
        u[13] = _mm_mullo_epi32(v[11], cospi32);
        x0    = _mm_mullo_epi32(v[12], cospim32);
        x1    = _mm_mullo_epi32(v[13], cospim32);
        u[12] = _mm_sub_epi32(u[12], x0);
        u[13] = _mm_sub_epi32(u[13], x1);
        u[12] = _mm_add_epi32(u[12], rnding);
        u[13] = _mm_add_epi32(u[13], rnding);
        u[12] = _mm_srai_epi32(u[12], bit);
        u[13] = _mm_srai_epi32(u[13], bit);

        u[14] = v[14];
        u[15] = v[15];
        u[16] = _mm_add_epi32(v[16], v[22]);
        u[17] = _mm_add_epi32(v[17], v[23]);
        u[22] = _mm_sub_epi32(v[16], v[22]);
        u[23] = _mm_sub_epi32(v[17], v[23]);
        u[18] = _mm_add_epi32(v[18], v[20]);
        u[19] = _mm_add_epi32(v[19], v[21]);
        u[20] = _mm_sub_epi32(v[18], v[20]);
        u[21] = _mm_sub_epi32(v[19], v[21]);
        u[24] = _mm_sub_epi32(v[30], v[24]);
        u[25] = _mm_sub_epi32(v[31], v[25]);
        u[30] = _mm_add_epi32(v[30], v[24]);
        u[31] = _mm_add_epi32(v[31], v[25]);
        u[26] = _mm_sub_epi32(v[28], v[26]);
        u[27] = _mm_sub_epi32(v[29], v[27]);
        u[28] = _mm_add_epi32(v[28], v[26]);
        u[29] = _mm_add_epi32(v[29], v[27]);

        // stage 4
        u[0] = _mm_mullo_epi32(u[0], cospi32);
        u[1] = _mm_mullo_epi32(u[1], cospi32);
        u[2] = _mm_mullo_epi32(u[2], cospi32);
        u[3] = _mm_mullo_epi32(u[3], cospi32);
        v[0] = _mm_add_epi32(u[0], u[2]);
        v[1] = _mm_add_epi32(u[1], u[3]);
        v[0] = _mm_add_epi32(v[0], rnding);
        v[1] = _mm_add_epi32(v[1], rnding);
        v[0] = _mm_srai_epi32(v[0], bit);
        v[1] = _mm_srai_epi32(v[1], bit);

        v[4] = _mm_mullo_epi32(u[4], cospi48);
        v[5] = _mm_mullo_epi32(u[5], cospi48);
        x0   = _mm_mullo_epi32(u[6], cospi16);
        x1   = _mm_mullo_epi32(u[7], cospi16);
        v[4] = _mm_add_epi32(v[4], x0);
        v[5] = _mm_add_epi32(v[5], x1);
        v[4] = _mm_add_epi32(v[4], rnding);
        v[5] = _mm_add_epi32(v[5], rnding);
        v[4] = _mm_srai_epi32(v[4], bit);
        v[5] = _mm_srai_epi32(v[5], bit);

        v[8]  = _mm_add_epi32(u[8], u[10]);
        v[9]  = _mm_add_epi32(u[9], u[11]);
        v[10] = _mm_sub_epi32(u[8], u[10]);
        v[11] = _mm_sub_epi32(u[9], u[11]);
        v[12] = _mm_sub_epi32(u[14], u[12]);
        v[13] = _mm_sub_epi32(u[15], u[13]);
        v[14] = _mm_add_epi32(u[14], u[12]);
        v[15] = _mm_add_epi32(u[15], u[13]);
        v[16] = u[16];
        v[17] = u[17];

        v[18] = _mm_mullo_epi32(u[18], cospim16);
        v[19] = _mm_mullo_epi32(u[19], cospim16);
        x0    = _mm_mullo_epi32(u[28], cospi48);
        x1    = _mm_mullo_epi32(u[29], cospi48);
        v[18] = _mm_add_epi32(v[18], x0);
        v[19] = _mm_add_epi32(v[19], x1);
        v[18] = _mm_add_epi32(v[18], rnding);
        v[19] = _mm_add_epi32(v[19], rnding);
        v[18] = _mm_srai_epi32(v[18], bit);
        v[19] = _mm_srai_epi32(v[19], bit);

        v[28] = _mm_mullo_epi32(u[18], cospi48);
        v[29] = _mm_mullo_epi32(u[19], cospi48);
        x0    = _mm_mullo_epi32(u[28], cospim16);
        x1    = _mm_mullo_epi32(u[29], cospim16);
        v[28] = _mm_sub_epi32(v[28], x0);
        v[29] = _mm_sub_epi32(v[29], x1);
        v[28] = _mm_add_epi32(v[28], rnding);
        v[29] = _mm_add_epi32(v[29], rnding);
        v[28] = _mm_srai_epi32(v[28], bit);
        v[29] = _mm_srai_epi32(v[29], bit);

        v[20] = _mm_mullo_epi32(u[20], cospim48);
        v[21] = _mm_mullo_epi32(u[21], cospim48);
        x0    = _mm_mullo_epi32(u[26], cospim16);
        x1    = _mm_mullo_epi32(u[27], cospim16);
        v[20] = _mm_add_epi32(v[20], x0);
        v[21] = _mm_add_epi32(v[21], x1);
        v[20] = _mm_add_epi32(v[20], rnding);
        v[21] = _mm_add_epi32(v[21], rnding);
        v[20] = _mm_srai_epi32(v[20], bit);
        v[21] = _mm_srai_epi32(v[21], bit);

        v[26] = _mm_mullo_epi32(u[20], cospim16);
        v[27] = _mm_mullo_epi32(u[21], cospim16);
        x0    = _mm_mullo_epi32(u[26], cospim48);
        x1    = _mm_mullo_epi32(u[27], cospim48);
        v[26] = _mm_sub_epi32(v[26], x0);
        v[27] = _mm_sub_epi32(v[27], x1);
        v[26] = _mm_add_epi32(v[26], rnding);
        v[27] = _mm_add_epi32(v[27], rnding);
        v[26] = _mm_srai_epi32(v[26], bit);
        v[27] = _mm_srai_epi32(v[27], bit);

        v[22] = u[22];
        v[23] = u[23];
        v[24] = u[24];
        v[25] = u[25];
        v[30] = u[30];
        v[31] = u[31];

        // stage 5
        u[0] = v[0];
        u[1] = v[1];
        u[4] = v[4];
        u[5] = v[5];

        u[8] = _mm_mullo_epi32(v[8], cospi56);
        u[9] = _mm_mullo_epi32(v[9], cospi56);
        x0   = _mm_mullo_epi32(v[14], cospi8);
        x1   = _mm_mullo_epi32(v[15], cospi8);
        u[8] = _mm_add_epi32(u[8], x0);
        u[9] = _mm_add_epi32(u[9], x1);
        u[8] = _mm_add_epi32(u[8], rnding);
        u[9] = _mm_add_epi32(u[9], rnding);
        u[8] = _mm_srai_epi32(u[8], bit);
        u[9] = _mm_srai_epi32(u[9], bit);

        u[12] = _mm_mullo_epi32(v[10], cospi40);
        u[13] = _mm_mullo_epi32(v[11], cospi40);
        x0    = _mm_mullo_epi32(v[12], cospi24);
        x1    = _mm_mullo_epi32(v[13], cospi24);
        u[12] = _mm_sub_epi32(x0, u[12]);
        u[13] = _mm_sub_epi32(x1, u[13]);
        u[12] = _mm_add_epi32(u[12], rnding);
        u[13] = _mm_add_epi32(u[13], rnding);
        u[12] = _mm_srai_epi32(u[12], bit);
        u[13] = _mm_srai_epi32(u[13], bit);

        u[16] = _mm_add_epi32(v[16], v[18]);
        u[17] = _mm_add_epi32(v[17], v[19]);
        u[18] = _mm_sub_epi32(v[16], v[18]);
        u[19] = _mm_sub_epi32(v[17], v[19]);
        u[20] = _mm_sub_epi32(v[22], v[20]);
        u[21] = _mm_sub_epi32(v[23], v[21]);
        u[22] = _mm_add_epi32(v[22], v[20]);
        u[23] = _mm_add_epi32(v[23], v[21]);
        u[24] = _mm_add_epi32(v[24], v[26]);
        u[25] = _mm_add_epi32(v[25], v[27]);
        u[26] = _mm_sub_epi32(v[24], v[26]);
        u[27] = _mm_sub_epi32(v[25], v[27]);
        u[28] = _mm_sub_epi32(v[30], v[28]);
        u[29] = _mm_sub_epi32(v[31], v[29]);
        u[30] = _mm_add_epi32(v[30], v[28]);
        u[31] = _mm_add_epi32(v[31], v[29]);

        // stage 6
        v[0]  = u[0];
        v[1]  = u[1];
        v[4]  = u[4];
        v[5]  = u[5];
        v[8]  = u[8];
        v[9]  = u[9];
        v[12] = u[12];
        v[13] = u[13];

        v[16] = _mm_mullo_epi32(u[16], cospi60);
        v[17] = _mm_mullo_epi32(u[17], cospi60);
        x0    = _mm_mullo_epi32(u[30], cospi4);
        x1    = _mm_mullo_epi32(u[31], cospi4);
        v[16] = _mm_add_epi32(v[16], x0);
        v[17] = _mm_add_epi32(v[17], x1);
        v[16] = _mm_add_epi32(v[16], rnding);
        v[17] = _mm_add_epi32(v[17], rnding);
        v[16] = _mm_srai_epi32(v[16], bit);
        v[17] = _mm_srai_epi32(v[17], bit);

        v[28] = _mm_mullo_epi32(u[18], cospi36);
        v[29] = _mm_mullo_epi32(u[19], cospi36);
        x0    = _mm_mullo_epi32(u[28], cospi28);
        x1    = _mm_mullo_epi32(u[29], cospi28);
        v[28] = _mm_sub_epi32(x0, v[28]);
        v[29] = _mm_sub_epi32(x1, v[29]);
        v[28] = _mm_add_epi32(v[28], rnding);
        v[29] = _mm_add_epi32(v[29], rnding);
        v[28] = _mm_srai_epi32(v[28], bit);
        v[29] = _mm_srai_epi32(v[29], bit);

        v[20] = _mm_mullo_epi32(u[20], cospi44);
        v[21] = _mm_mullo_epi32(u[21], cospi44);
        x0    = _mm_mullo_epi32(u[26], cospi20);
        x1    = _mm_mullo_epi32(u[27], cospi20);
        v[20] = _mm_add_epi32(v[20], x0);
        v[21] = _mm_add_epi32(v[21], x1);
        v[20] = _mm_add_epi32(v[20], rnding);
        v[21] = _mm_add_epi32(v[21], rnding);
        v[20] = _mm_srai_epi32(v[20], bit);
        v[21] = _mm_srai_epi32(v[21], bit);

        v[24] = _mm_mullo_epi32(u[22], cospi52);
        v[25] = _mm_mullo_epi32(u[23], cospi52);
        x0    = _mm_mullo_epi32(u[24], cospi12);
        x1    = _mm_mullo_epi32(u[25], cospi12);
        v[24] = _mm_sub_epi32(x0, v[24]);
        v[25] = _mm_sub_epi32(x1, v[25]);
        v[24] = _mm_add_epi32(v[24], rnding);
        v[25] = _mm_add_epi32(v[25], rnding);
        v[24] = _mm_srai_epi32(v[24], bit);
        v[25] = _mm_srai_epi32(v[25], bit);

        out[2 * (0 * col_num + col)]     = v[0];
        out[2 * (0 * col_num + col) + 1] = v[1];
        out[2 * (1 * col_num + col)]     = v[16];
        out[2 * (1 * col_num + col) + 1] = v[17];
        out[2 * (2 * col_num + col)]     = v[8];
        out[2 * (2 * col_num + col) + 1] = v[9];
        out[2 * (3 * col_num + col)]     = v[24];
        out[2 * (3 * col_num + col) + 1] = v[25];
        out[2 * (4 * col_num + col)]     = v[4];
        out[2 * (4 * col_num + col) + 1] = v[5];
        out[2 * (5 * col_num + col)]     = v[20];
        out[2 * (5 * col_num + col) + 1] = v[21];
        out[2 * (6 * col_num + col)]     = v[12];
        out[2 * (6 * col_num + col) + 1] = v[13];
        out[2 * (7 * col_num + col)]     = v[28];
        out[2 * (7 * col_num + col) + 1] = v[29];
    }
}

static void fadst16x16_N2_sse4_1(const __m128i* in, __m128i* out, int8_t bit, const int32_t col_num, int32_t size) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospi16  = _mm_set1_epi32(cospi[16]);
    const __m128i  cospim16 = _mm_set1_epi32(-cospi[16]);
    const __m128i  cospim48 = _mm_set1_epi32(-cospi[48]);
    const __m128i  cospi8   = _mm_set1_epi32(cospi[8]);
    const __m128i  cospi56  = _mm_set1_epi32(cospi[56]);
    const __m128i  cospim56 = _mm_set1_epi32(-cospi[56]);
    const __m128i  cospim8  = _mm_set1_epi32(-cospi[8]);
    const __m128i  cospi24  = _mm_set1_epi32(cospi[24]);
    const __m128i  cospim24 = _mm_set1_epi32(-cospi[24]);
    const __m128i  cospim40 = _mm_set1_epi32(-cospi[40]);
    const __m128i  cospi40  = _mm_set1_epi32(cospi[40]);
    const __m128i  cospi62  = _mm_set1_epi32(cospi[62]);
    const __m128i  cospim2  = _mm_set1_epi32(-cospi[2]);
    const __m128i  cospi54  = _mm_set1_epi32(cospi[54]);
    const __m128i  cospim10 = _mm_set1_epi32(-cospi[10]);
    const __m128i  cospi46  = _mm_set1_epi32(cospi[46]);
    const __m128i  cospim18 = _mm_set1_epi32(-cospi[18]);
    const __m128i  cospi38  = _mm_set1_epi32(cospi[38]);
    const __m128i  cospim26 = _mm_set1_epi32(-cospi[26]);
    const __m128i  cospi34  = _mm_set1_epi32(cospi[34]);
    const __m128i  cospi30  = _mm_set1_epi32(cospi[30]);
    const __m128i  cospi42  = _mm_set1_epi32(cospi[42]);
    const __m128i  cospi22  = _mm_set1_epi32(cospi[22]);
    const __m128i  cospi50  = _mm_set1_epi32(cospi[50]);
    const __m128i  cospi14  = _mm_set1_epi32(cospi[14]);
    const __m128i  cospi58  = _mm_set1_epi32(cospi[58]);
    const __m128i  cospi6   = _mm_set1_epi32(cospi[6]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    const __m128i  zero     = _mm_setzero_si128();

    __m128i u[32], v[32], x0, x1, y0, y1;
    int32_t col;

    for (col = 0; col < size; ++col) {
        // stage 0
        // stage 1
        u[0]  = in[2 * (0 * col_num + col)];
        u[1]  = in[2 * (0 * col_num + col) + 1];
        u[2]  = _mm_sub_epi32(zero, in[2 * (15 * col_num + col)]);
        u[3]  = _mm_sub_epi32(zero, in[2 * (15 * col_num + col) + 1]);
        u[4]  = _mm_sub_epi32(zero, in[2 * (7 * col_num + col)]);
        u[5]  = _mm_sub_epi32(zero, in[2 * (7 * col_num + col) + 1]);
        u[6]  = in[2 * (8 * col_num + col)];
        u[7]  = in[2 * (8 * col_num + col) + 1];
        u[8]  = _mm_sub_epi32(zero, in[2 * (3 * col_num + col)]);
        u[9]  = _mm_sub_epi32(zero, in[2 * (3 * col_num + col) + 1]);
        u[10] = in[2 * (12 * col_num + col)];
        u[11] = in[2 * (12 * col_num + col) + 1];
        u[12] = in[2 * (4 * col_num + col)];
        u[13] = in[2 * (4 * col_num + col) + 1];
        u[14] = _mm_sub_epi32(zero, in[2 * (11 * col_num + col)]);
        u[15] = _mm_sub_epi32(zero, in[2 * (11 * col_num + col) + 1]);
        u[16] = _mm_sub_epi32(zero, in[2 * (1 * col_num + col)]);
        u[17] = _mm_sub_epi32(zero, in[2 * (1 * col_num + col) + 1]);
        u[18] = in[2 * (14 * col_num + col)];
        u[19] = in[2 * (14 * col_num + col) + 1];
        u[20] = in[2 * (6 * col_num + col)];
        u[21] = in[2 * (6 * col_num + col) + 1];
        u[22] = _mm_sub_epi32(zero, in[2 * (9 * col_num + col)]);
        u[23] = _mm_sub_epi32(zero, in[2 * (9 * col_num + col) + 1]);
        u[24] = in[2 * (2 * col_num + col)];
        u[25] = in[2 * (2 * col_num + col) + 1];
        u[26] = _mm_sub_epi32(zero, in[2 * (13 * col_num + col)]);
        u[27] = _mm_sub_epi32(zero, in[2 * (13 * col_num + col) + 1]);
        u[28] = _mm_sub_epi32(zero, in[2 * (5 * col_num + col)]);
        u[29] = _mm_sub_epi32(zero, in[2 * (5 * col_num + col) + 1]);
        u[30] = in[2 * (10 * col_num + col)];
        u[31] = in[2 * (10 * col_num + col) + 1];

        // stage 2
        v[0] = u[0];
        v[1] = u[1];
        v[2] = u[2];
        v[3] = u[3];

        x0   = _mm_mullo_epi32(u[4], cospi32);
        x1   = _mm_mullo_epi32(u[5], cospi32);
        y0   = _mm_mullo_epi32(u[6], cospi32);
        y1   = _mm_mullo_epi32(u[7], cospi32);
        v[4] = _mm_add_epi32(x0, y0);
        v[5] = _mm_add_epi32(x1, y1);
        v[4] = _mm_add_epi32(v[4], rnding);
        v[5] = _mm_add_epi32(v[5], rnding);
        v[4] = _mm_srai_epi32(v[4], bit);
        v[5] = _mm_srai_epi32(v[5], bit);

        v[6] = _mm_sub_epi32(x0, y0);
        v[7] = _mm_sub_epi32(x1, y1);
        v[6] = _mm_add_epi32(v[6], rnding);
        v[7] = _mm_add_epi32(v[7], rnding);
        v[6] = _mm_srai_epi32(v[6], bit);
        v[7] = _mm_srai_epi32(v[7], bit);

        v[8]  = u[8];
        v[9]  = u[9];
        v[10] = u[10];
        v[11] = u[11];

        x0    = _mm_mullo_epi32(u[12], cospi32);
        x1    = _mm_mullo_epi32(u[13], cospi32);
        y0    = _mm_mullo_epi32(u[14], cospi32);
        y1    = _mm_mullo_epi32(u[15], cospi32);
        v[12] = _mm_add_epi32(x0, y0);
        v[13] = _mm_add_epi32(x1, y1);
        v[12] = _mm_add_epi32(v[12], rnding);
        v[13] = _mm_add_epi32(v[13], rnding);
        v[12] = _mm_srai_epi32(v[12], bit);
        v[13] = _mm_srai_epi32(v[13], bit);

        v[14] = _mm_sub_epi32(x0, y0);
        v[15] = _mm_sub_epi32(x1, y1);
        v[14] = _mm_add_epi32(v[14], rnding);
        v[15] = _mm_add_epi32(v[15], rnding);
        v[14] = _mm_srai_epi32(v[14], bit);
        v[15] = _mm_srai_epi32(v[15], bit);

        v[16] = u[16];
        v[17] = u[17];
        v[18] = u[18];
        v[19] = u[19];

        x0    = _mm_mullo_epi32(u[20], cospi32);
        x1    = _mm_mullo_epi32(u[21], cospi32);
        y0    = _mm_mullo_epi32(u[22], cospi32);
        y1    = _mm_mullo_epi32(u[23], cospi32);
        v[20] = _mm_add_epi32(x0, y0);
        v[21] = _mm_add_epi32(x1, y1);
        v[20] = _mm_add_epi32(v[20], rnding);
        v[21] = _mm_add_epi32(v[21], rnding);
        v[20] = _mm_srai_epi32(v[20], bit);
        v[21] = _mm_srai_epi32(v[21], bit);

        v[22] = _mm_sub_epi32(x0, y0);
        v[23] = _mm_sub_epi32(x1, y1);
        v[22] = _mm_add_epi32(v[22], rnding);
        v[23] = _mm_add_epi32(v[23], rnding);
        v[22] = _mm_srai_epi32(v[22], bit);
        v[23] = _mm_srai_epi32(v[23], bit);

        v[24] = u[24];
        v[25] = u[25];
        v[26] = u[26];
        v[27] = u[27];

        x0    = _mm_mullo_epi32(u[28], cospi32);
        x1    = _mm_mullo_epi32(u[29], cospi32);
        y0    = _mm_mullo_epi32(u[30], cospi32);
        y1    = _mm_mullo_epi32(u[31], cospi32);
        v[28] = _mm_add_epi32(x0, y0);
        v[29] = _mm_add_epi32(x1, y1);
        v[28] = _mm_add_epi32(v[28], rnding);
        v[29] = _mm_add_epi32(v[29], rnding);
        v[28] = _mm_srai_epi32(v[28], bit);
        v[29] = _mm_srai_epi32(v[29], bit);

        v[30] = _mm_sub_epi32(x0, y0);
        v[31] = _mm_sub_epi32(x1, y1);
        v[30] = _mm_add_epi32(v[30], rnding);
        v[31] = _mm_add_epi32(v[31], rnding);
        v[30] = _mm_srai_epi32(v[30], bit);
        v[31] = _mm_srai_epi32(v[31], bit);

        // stage 3
        u[0]  = _mm_add_epi32(v[0], v[4]);
        u[1]  = _mm_add_epi32(v[1], v[5]);
        u[2]  = _mm_add_epi32(v[2], v[6]);
        u[3]  = _mm_add_epi32(v[3], v[7]);
        u[4]  = _mm_sub_epi32(v[0], v[4]);
        u[5]  = _mm_sub_epi32(v[1], v[5]);
        u[6]  = _mm_sub_epi32(v[2], v[6]);
        u[7]  = _mm_sub_epi32(v[3], v[7]);
        u[8]  = _mm_add_epi32(v[8], v[12]);
        u[9]  = _mm_add_epi32(v[9], v[13]);
        u[10] = _mm_add_epi32(v[10], v[14]);
        u[11] = _mm_add_epi32(v[11], v[15]);
        u[12] = _mm_sub_epi32(v[8], v[12]);
        u[13] = _mm_sub_epi32(v[9], v[13]);
        u[14] = _mm_sub_epi32(v[10], v[14]);
        u[15] = _mm_sub_epi32(v[11], v[15]);
        u[16] = _mm_add_epi32(v[16], v[20]);
        u[17] = _mm_add_epi32(v[17], v[21]);
        u[18] = _mm_add_epi32(v[18], v[22]);
        u[19] = _mm_add_epi32(v[19], v[23]);
        u[20] = _mm_sub_epi32(v[16], v[20]);
        u[21] = _mm_sub_epi32(v[17], v[21]);
        u[22] = _mm_sub_epi32(v[18], v[22]);
        u[23] = _mm_sub_epi32(v[19], v[23]);
        u[24] = _mm_add_epi32(v[24], v[28]);
        u[25] = _mm_add_epi32(v[25], v[29]);
        u[26] = _mm_add_epi32(v[26], v[30]);
        u[27] = _mm_add_epi32(v[27], v[31]);
        u[28] = _mm_sub_epi32(v[24], v[28]);
        u[29] = _mm_sub_epi32(v[25], v[29]);
        u[30] = _mm_sub_epi32(v[26], v[30]);
        u[31] = _mm_sub_epi32(v[27], v[31]);

        // stage 4
        v[0]  = u[0];
        v[1]  = u[1];
        v[2]  = u[2];
        v[3]  = u[3];
        v[4]  = u[4];
        v[5]  = u[5];
        v[6]  = u[6];
        v[7]  = u[7];
        v[8]  = half_btf_sse4_1(&cospi16, &u[8], &cospi48, &u[10], &rnding, bit);
        v[9]  = half_btf_sse4_1(&cospi16, &u[9], &cospi48, &u[11], &rnding, bit);
        v[10] = half_btf_sse4_1(&cospi48, &u[8], &cospim16, &u[10], &rnding, bit);
        v[11] = half_btf_sse4_1(&cospi48, &u[9], &cospim16, &u[11], &rnding, bit);
        v[12] = half_btf_sse4_1(&cospim48, &u[12], &cospi16, &u[14], &rnding, bit);
        v[13] = half_btf_sse4_1(&cospim48, &u[13], &cospi16, &u[15], &rnding, bit);
        v[14] = half_btf_sse4_1(&cospi16, &u[12], &cospi48, &u[14], &rnding, bit);
        v[15] = half_btf_sse4_1(&cospi16, &u[13], &cospi48, &u[15], &rnding, bit);
        v[16] = u[16];
        v[17] = u[17];
        v[18] = u[18];
        v[19] = u[19];
        v[20] = u[20];
        v[21] = u[21];
        v[22] = u[22];
        v[23] = u[23];
        v[24] = half_btf_sse4_1(&cospi16, &u[24], &cospi48, &u[26], &rnding, bit);
        v[25] = half_btf_sse4_1(&cospi16, &u[25], &cospi48, &u[27], &rnding, bit);
        v[26] = half_btf_sse4_1(&cospi48, &u[24], &cospim16, &u[26], &rnding, bit);
        v[27] = half_btf_sse4_1(&cospi48, &u[25], &cospim16, &u[27], &rnding, bit);
        v[28] = half_btf_sse4_1(&cospim48, &u[28], &cospi16, &u[30], &rnding, bit);
        v[29] = half_btf_sse4_1(&cospim48, &u[29], &cospi16, &u[31], &rnding, bit);
        v[30] = half_btf_sse4_1(&cospi16, &u[28], &cospi48, &u[30], &rnding, bit);
        v[31] = half_btf_sse4_1(&cospi16, &u[29], &cospi48, &u[31], &rnding, bit);

        // stage 5
        u[0]  = _mm_add_epi32(v[0], v[8]);
        u[1]  = _mm_add_epi32(v[1], v[9]);
        u[2]  = _mm_add_epi32(v[2], v[10]);
        u[3]  = _mm_add_epi32(v[3], v[11]);
        u[4]  = _mm_add_epi32(v[4], v[12]);
        u[5]  = _mm_add_epi32(v[5], v[13]);
        u[6]  = _mm_add_epi32(v[6], v[14]);
        u[7]  = _mm_add_epi32(v[7], v[15]);
        u[8]  = _mm_sub_epi32(v[0], v[8]);
        u[9]  = _mm_sub_epi32(v[1], v[9]);
        u[10] = _mm_sub_epi32(v[2], v[10]);
        u[11] = _mm_sub_epi32(v[3], v[11]);
        u[12] = _mm_sub_epi32(v[4], v[12]);
        u[13] = _mm_sub_epi32(v[5], v[13]);
        u[14] = _mm_sub_epi32(v[6], v[14]);
        u[15] = _mm_sub_epi32(v[7], v[15]);
        u[16] = _mm_add_epi32(v[16], v[24]);
        u[17] = _mm_add_epi32(v[17], v[25]);
        u[18] = _mm_add_epi32(v[18], v[26]);
        u[19] = _mm_add_epi32(v[19], v[27]);
        u[20] = _mm_add_epi32(v[20], v[28]);
        u[21] = _mm_add_epi32(v[21], v[29]);
        u[22] = _mm_add_epi32(v[22], v[30]);
        u[23] = _mm_add_epi32(v[23], v[31]);
        u[24] = _mm_sub_epi32(v[16], v[24]);
        u[25] = _mm_sub_epi32(v[17], v[25]);
        u[26] = _mm_sub_epi32(v[18], v[26]);
        u[27] = _mm_sub_epi32(v[19], v[27]);
        u[28] = _mm_sub_epi32(v[20], v[28]);
        u[29] = _mm_sub_epi32(v[21], v[29]);
        u[30] = _mm_sub_epi32(v[22], v[30]);
        u[31] = _mm_sub_epi32(v[23], v[31]);

        // stage 6
        v[0]  = u[0];
        v[1]  = u[1];
        v[2]  = u[2];
        v[3]  = u[3];
        v[4]  = u[4];
        v[5]  = u[5];
        v[6]  = u[6];
        v[7]  = u[7];
        v[8]  = u[8];
        v[9]  = u[9];
        v[10] = u[10];
        v[11] = u[11];
        v[12] = u[12];
        v[13] = u[13];
        v[14] = u[14];
        v[15] = u[15];
        v[16] = half_btf_sse4_1(&cospi8, &u[16], &cospi56, &u[18], &rnding, bit);
        v[17] = half_btf_sse4_1(&cospi8, &u[17], &cospi56, &u[19], &rnding, bit);
        v[18] = half_btf_sse4_1(&cospi56, &u[16], &cospim8, &u[18], &rnding, bit);
        v[19] = half_btf_sse4_1(&cospi56, &u[17], &cospim8, &u[19], &rnding, bit);
        v[20] = half_btf_sse4_1(&cospi40, &u[20], &cospi24, &u[22], &rnding, bit);
        v[21] = half_btf_sse4_1(&cospi40, &u[21], &cospi24, &u[23], &rnding, bit);
        v[22] = half_btf_sse4_1(&cospi24, &u[20], &cospim40, &u[22], &rnding, bit);
        v[23] = half_btf_sse4_1(&cospi24, &u[21], &cospim40, &u[23], &rnding, bit);
        v[24] = half_btf_sse4_1(&cospim56, &u[24], &cospi8, &u[26], &rnding, bit);
        v[25] = half_btf_sse4_1(&cospim56, &u[25], &cospi8, &u[27], &rnding, bit);
        v[26] = half_btf_sse4_1(&cospi8, &u[24], &cospi56, &u[26], &rnding, bit);
        v[27] = half_btf_sse4_1(&cospi8, &u[25], &cospi56, &u[27], &rnding, bit);
        v[28] = half_btf_sse4_1(&cospim24, &u[28], &cospi40, &u[30], &rnding, bit);
        v[29] = half_btf_sse4_1(&cospim24, &u[29], &cospi40, &u[31], &rnding, bit);
        v[30] = half_btf_sse4_1(&cospi40, &u[28], &cospi24, &u[30], &rnding, bit);
        v[31] = half_btf_sse4_1(&cospi40, &u[29], &cospi24, &u[31], &rnding, bit);

        // stage 7
        u[0]  = _mm_add_epi32(v[0], v[16]);
        u[1]  = _mm_add_epi32(v[1], v[17]);
        u[2]  = _mm_add_epi32(v[2], v[18]);
        u[3]  = _mm_add_epi32(v[3], v[19]);
        u[4]  = _mm_add_epi32(v[4], v[20]);
        u[5]  = _mm_add_epi32(v[5], v[21]);
        u[6]  = _mm_add_epi32(v[6], v[22]);
        u[7]  = _mm_add_epi32(v[7], v[23]);
        u[8]  = _mm_add_epi32(v[8], v[24]);
        u[9]  = _mm_add_epi32(v[9], v[25]);
        u[10] = _mm_add_epi32(v[10], v[26]);
        u[11] = _mm_add_epi32(v[11], v[27]);
        u[12] = _mm_add_epi32(v[12], v[28]);
        u[13] = _mm_add_epi32(v[13], v[29]);
        u[14] = _mm_add_epi32(v[14], v[30]);
        u[15] = _mm_add_epi32(v[15], v[31]);
        u[16] = _mm_sub_epi32(v[0], v[16]);
        u[17] = _mm_sub_epi32(v[1], v[17]);
        u[18] = _mm_sub_epi32(v[2], v[18]);
        u[19] = _mm_sub_epi32(v[3], v[19]);
        u[20] = _mm_sub_epi32(v[4], v[20]);
        u[21] = _mm_sub_epi32(v[5], v[21]);
        u[22] = _mm_sub_epi32(v[6], v[22]);
        u[23] = _mm_sub_epi32(v[7], v[23]);
        u[24] = _mm_sub_epi32(v[8], v[24]);
        u[25] = _mm_sub_epi32(v[9], v[25]);
        u[26] = _mm_sub_epi32(v[10], v[26]);
        u[27] = _mm_sub_epi32(v[11], v[27]);
        u[28] = _mm_sub_epi32(v[12], v[28]);
        u[29] = _mm_sub_epi32(v[13], v[29]);
        u[30] = _mm_sub_epi32(v[14], v[30]);
        u[31] = _mm_sub_epi32(v[15], v[31]);

        // stage 8
        v[2]  = half_btf_sse4_1(&cospi62, &u[0], &cospim2, &u[2], &rnding, bit);
        v[3]  = half_btf_sse4_1(&cospi62, &u[1], &cospim2, &u[3], &rnding, bit);
        v[6]  = half_btf_sse4_1(&cospi54, &u[4], &cospim10, &u[6], &rnding, bit);
        v[7]  = half_btf_sse4_1(&cospi54, &u[5], &cospim10, &u[7], &rnding, bit);
        v[10] = half_btf_sse4_1(&cospi46, &u[8], &cospim18, &u[10], &rnding, bit);
        v[11] = half_btf_sse4_1(&cospi46, &u[9], &cospim18, &u[11], &rnding, bit);
        v[14] = half_btf_sse4_1(&cospi38, &u[12], &cospim26, &u[14], &rnding, bit);
        v[15] = half_btf_sse4_1(&cospi38, &u[13], &cospim26, &u[15], &rnding, bit);
        v[16] = half_btf_sse4_1(&cospi34, &u[16], &cospi30, &u[18], &rnding, bit);
        v[17] = half_btf_sse4_1(&cospi34, &u[17], &cospi30, &u[19], &rnding, bit);
        v[20] = half_btf_sse4_1(&cospi42, &u[20], &cospi22, &u[22], &rnding, bit);
        v[21] = half_btf_sse4_1(&cospi42, &u[21], &cospi22, &u[23], &rnding, bit);
        v[24] = half_btf_sse4_1(&cospi50, &u[24], &cospi14, &u[26], &rnding, bit);
        v[25] = half_btf_sse4_1(&cospi50, &u[25], &cospi14, &u[27], &rnding, bit);
        v[28] = half_btf_sse4_1(&cospi58, &u[28], &cospi6, &u[30], &rnding, bit);
        v[29] = half_btf_sse4_1(&cospi58, &u[29], &cospi6, &u[31], &rnding, bit);

        // stage 9
        out[2 * (0 * col_num + col)]     = v[2];
        out[2 * (0 * col_num + col) + 1] = v[3];
        out[2 * (1 * col_num + col)]     = v[28];
        out[2 * (1 * col_num + col) + 1] = v[29];
        out[2 * (2 * col_num + col)]     = v[6];
        out[2 * (2 * col_num + col) + 1] = v[7];
        out[2 * (3 * col_num + col)]     = v[24];
        out[2 * (3 * col_num + col) + 1] = v[25];
        out[2 * (4 * col_num + col)]     = v[10];
        out[2 * (4 * col_num + col) + 1] = v[11];
        out[2 * (5 * col_num + col)]     = v[20];
        out[2 * (5 * col_num + col) + 1] = v[21];
        out[2 * (6 * col_num + col)]     = v[14];
        out[2 * (6 * col_num + col) + 1] = v[15];
        out[2 * (7 * col_num + col)]     = v[16];
        out[2 * (7 * col_num + col) + 1] = v[17];
    }
}

static void fidtx8xn_N2_col_sse4_1(const __m128i* in, __m128i* out, int8_t bit, int32_t col_num) {
    (void)bit;
    const int32_t bits     = 12; // new_sqrt2_bits = 12
    const int32_t sqrt     = 2 * 5793; // 2 * new_sqrt2
    const __m128i newsqrt  = _mm_set1_epi32(sqrt);
    const __m128i rounding = _mm_set1_epi32(1 << (bits - 1));
    __m128i       temp;
    for (int32_t i = 0; i < 2 * col_num; i++) {
        temp   = _mm_mullo_epi32(in[i], newsqrt);
        temp   = _mm_add_epi32(temp, rounding);
        out[i] = _mm_srai_epi32(temp, bits);
    }
}

static void fidtx16x16_N2_row_sse4_1(const __m128i* in, __m128i* out, int8_t bit, int32_t col_num) {
    (void)bit;
    const int32_t bits     = 12; // new_sqrt2_bits = 12
    const int32_t sqrt     = 2 * 5793; // 2 * new_sqrt2
    const __m128i newsqrt  = _mm_set1_epi32(sqrt);
    const __m128i rounding = _mm_set1_epi32(1 << (bits - 1));
    __m128i       temp;
    int32_t       num_iters = 16 * col_num;
    for (int32_t i = 0; i < num_iters /*/ 2*/; i += 1) {
        temp   = _mm_mullo_epi32(in[i], newsqrt);
        temp   = _mm_add_epi32(temp, rounding);
        out[i] = _mm_srai_epi32(temp, bits);
    }
}

static void av1_idtx32_new_N2_sse4_1(const __m128i* input, __m128i* output, int8_t cos_bit, const int32_t col_num,
                                     int32_t size) {
    (void)cos_bit;

    for (int32_t i = 0; i < size; i += 4) {
        output[2 * i * col_num]     = _mm_slli_epi32(input[2 * i * col_num], 2);
        output[2 * i * col_num + 1] = _mm_slli_epi32(input[2 * i * col_num + 1], 2);
        output[2 * i * col_num + 2] = _mm_slli_epi32(input[2 * i * col_num + 2], 2);
        output[2 * i * col_num + 3] = _mm_slli_epi32(input[2 * i * col_num + 3], 2);
    }
}

static void av1_idtx16x32_N2_sse4_1(const __m128i* input, __m128i* output) {
    for (int32_t i = 0; i < 32; i += 2) {
        output[2 * i]     = _mm_slli_epi32(input[2 * i], 2);
        output[2 * i + 1] = _mm_slli_epi32(input[2 * i + 1], 2);
    }
}

static AOM_FORCE_INLINE void clear_buffer_wxh_N2(__m128i* buff, int32_t num_col, int32_t num_row) {
    const __m128i zero = _mm_setzero_si128();
    assert(num_col > 0);
    assert(num_row > 1);

    //clear top-right quarter
    if (num_col == 1) { //clear top-right quarter
        for (int i = 0; i < num_row / 2; i++) {
            buff[i * 2 + 1] = zero;
        }
    } else {
        for (int i = 0; i < num_row / 2; i++) {
            for (int j = num_col / 2; j < num_col; j++) {
                buff[2 * (i * num_col + j)]     = zero;
                buff[2 * (i * num_col + j) + 1] = zero;
            }
        }
    }
    //clear bottom half
    for (int i = num_row / 2; i < num_row; i++) {
        for (int j = 0; j < num_col; j++) {
            buff[2 * (i * num_col + j)]     = zero;
            buff[2 * (i * num_col + j) + 1] = zero;
        }
    }
}

static void fidtx32x32_N2_col_sse4_1(const __m128i* input, __m128i* output, const int8_t cos_bit) {
    av1_idtx32_new_sse4_1(&input[0 * 32], &output[0 * 32], cos_bit, 1);
    av1_idtx32_new_sse4_1(&input[2 * 32], &output[2 * 32], cos_bit, 1);
}

static void fidtx32x32_N2_row_sse4_1(const __m128i* input, __m128i* output, const int8_t cos_bit) {
    av1_idtx32_new_N2_sse4_1(&input[0 * 32], &output[0 * 32], cos_bit, 1, 32);
    av1_idtx32_new_N2_sse4_1(&input[2 * 32], &output[2 * 32], cos_bit, 1, 32);
}

static void av1_fdct32_new_N2_sse4_1(const __m128i* input, __m128i* output, int8_t cos_bit, const int32_t col_num,
                                     const int32_t stride) {
    const int32_t* cospi      = cospi_arr(cos_bit);
    const __m128i  __rounding = _mm_set1_epi32(1 << (cos_bit - 1));
    const int32_t  columns    = col_num >> 2;

    __m128i cospi_m32 = _mm_set1_epi32(-cospi[32]);
    __m128i cospi_p32 = _mm_set1_epi32(cospi[32]);
    __m128i cospi_m16 = _mm_set1_epi32(-cospi[16]);
    __m128i cospi_p48 = _mm_set1_epi32(cospi[48]);
    __m128i cospi_m48 = _mm_set1_epi32(-cospi[48]);
    __m128i cospi_m08 = _mm_set1_epi32(-cospi[8]);
    __m128i cospi_p56 = _mm_set1_epi32(cospi[56]);
    __m128i cospi_m56 = _mm_set1_epi32(-cospi[56]);
    __m128i cospi_m40 = _mm_set1_epi32(-cospi[40]);
    __m128i cospi_p24 = _mm_set1_epi32(cospi[24]);
    __m128i cospi_m24 = _mm_set1_epi32(-cospi[24]);
    __m128i cospi_p16 = _mm_set1_epi32(cospi[16]);
    __m128i cospi_p08 = _mm_set1_epi32(cospi[8]);
    __m128i cospi_p04 = _mm_set1_epi32(cospi[4]);
    __m128i cospi_p60 = _mm_set1_epi32(cospi[60]);
    __m128i cospi_m36 = _mm_set1_epi32(-cospi[36]);
    __m128i cospi_p28 = _mm_set1_epi32(cospi[28]);
    __m128i cospi_p20 = _mm_set1_epi32(cospi[20]);
    __m128i cospi_p44 = _mm_set1_epi32(cospi[44]);
    __m128i cospi_m52 = _mm_set1_epi32(-cospi[52]);
    __m128i cospi_p12 = _mm_set1_epi32(cospi[12]);
    __m128i cospi_p02 = _mm_set1_epi32(cospi[2]);
    __m128i cospi_p06 = _mm_set1_epi32(cospi[6]);
    __m128i cospi_p62 = _mm_set1_epi32(cospi[62]);
    __m128i cospi_m34 = _mm_set1_epi32(-cospi[34]);
    __m128i cospi_p30 = _mm_set1_epi32(cospi[30]);
    __m128i cospi_p18 = _mm_set1_epi32(cospi[18]);
    __m128i cospi_p46 = _mm_set1_epi32(cospi[46]);
    __m128i cospi_m50 = _mm_set1_epi32(-cospi[50]);
    __m128i cospi_p14 = _mm_set1_epi32(cospi[14]);
    __m128i cospi_p10 = _mm_set1_epi32(cospi[10]);
    __m128i cospi_p54 = _mm_set1_epi32(cospi[54]);
    __m128i cospi_m42 = _mm_set1_epi32(-cospi[42]);
    __m128i cospi_p22 = _mm_set1_epi32(cospi[22]);
    __m128i cospi_p26 = _mm_set1_epi32(cospi[26]);
    __m128i cospi_p38 = _mm_set1_epi32(cospi[38]);
    __m128i cospi_m58 = _mm_set1_epi32(-cospi[58]);

    __m128i buf0[32];
    __m128i buf1[32];

    for (int32_t col = 0; col < columns; col++) {
        const __m128i* in  = &input[col];
        __m128i*       out = &output[col];

        // stage 0
        // stage 1
        buf1[0]  = _mm_add_epi32(in[2 * 0 * stride], in[2 * 31 * stride]);
        buf1[31] = _mm_sub_epi32(in[2 * 0 * stride], in[2 * 31 * stride]);
        buf1[1]  = _mm_add_epi32(in[2 * 1 * stride], in[2 * 30 * stride]);
        buf1[30] = _mm_sub_epi32(in[2 * 1 * stride], in[2 * 30 * stride]);
        buf1[2]  = _mm_add_epi32(in[2 * 2 * stride], in[2 * 29 * stride]);
        buf1[29] = _mm_sub_epi32(in[2 * 2 * stride], in[2 * 29 * stride]);
        buf1[3]  = _mm_add_epi32(in[2 * 3 * stride], in[2 * 28 * stride]);
        buf1[28] = _mm_sub_epi32(in[2 * 3 * stride], in[2 * 28 * stride]);
        buf1[4]  = _mm_add_epi32(in[2 * 4 * stride], in[2 * 27 * stride]);
        buf1[27] = _mm_sub_epi32(in[2 * 4 * stride], in[2 * 27 * stride]);
        buf1[5]  = _mm_add_epi32(in[2 * 5 * stride], in[2 * 26 * stride]);
        buf1[26] = _mm_sub_epi32(in[2 * 5 * stride], in[2 * 26 * stride]);
        buf1[6]  = _mm_add_epi32(in[2 * 6 * stride], in[2 * 25 * stride]);
        buf1[25] = _mm_sub_epi32(in[2 * 6 * stride], in[2 * 25 * stride]);
        buf1[7]  = _mm_add_epi32(in[2 * 7 * stride], in[2 * 24 * stride]);
        buf1[24] = _mm_sub_epi32(in[2 * 7 * stride], in[2 * 24 * stride]);
        buf1[8]  = _mm_add_epi32(in[2 * 8 * stride], in[2 * 23 * stride]);
        buf1[23] = _mm_sub_epi32(in[2 * 8 * stride], in[2 * 23 * stride]);
        buf1[9]  = _mm_add_epi32(in[2 * 9 * stride], in[2 * 22 * stride]);
        buf1[22] = _mm_sub_epi32(in[2 * 9 * stride], in[2 * 22 * stride]);
        buf1[10] = _mm_add_epi32(in[2 * 10 * stride], in[2 * 21 * stride]);
        buf1[21] = _mm_sub_epi32(in[2 * 10 * stride], in[2 * 21 * stride]);
        buf1[11] = _mm_add_epi32(in[2 * 11 * stride], in[2 * 20 * stride]);
        buf1[20] = _mm_sub_epi32(in[2 * 11 * stride], in[2 * 20 * stride]);
        buf1[12] = _mm_add_epi32(in[2 * 12 * stride], in[2 * 19 * stride]);
        buf1[19] = _mm_sub_epi32(in[2 * 12 * stride], in[2 * 19 * stride]);
        buf1[13] = _mm_add_epi32(in[2 * 13 * stride], in[2 * 18 * stride]);
        buf1[18] = _mm_sub_epi32(in[2 * 13 * stride], in[2 * 18 * stride]);
        buf1[14] = _mm_add_epi32(in[2 * 14 * stride], in[2 * 17 * stride]);
        buf1[17] = _mm_sub_epi32(in[2 * 14 * stride], in[2 * 17 * stride]);
        buf1[15] = _mm_add_epi32(in[2 * 15 * stride], in[2 * 16 * stride]);
        buf1[16] = _mm_sub_epi32(in[2 * 15 * stride], in[2 * 16 * stride]);

        // stage 2
        buf0[0]  = _mm_add_epi32(buf1[0], buf1[15]);
        buf0[15] = _mm_sub_epi32(buf1[0], buf1[15]);
        buf0[1]  = _mm_add_epi32(buf1[1], buf1[14]);
        buf0[14] = _mm_sub_epi32(buf1[1], buf1[14]);
        buf0[2]  = _mm_add_epi32(buf1[2], buf1[13]);
        buf0[13] = _mm_sub_epi32(buf1[2], buf1[13]);
        buf0[3]  = _mm_add_epi32(buf1[3], buf1[12]);
        buf0[12] = _mm_sub_epi32(buf1[3], buf1[12]);
        buf0[4]  = _mm_add_epi32(buf1[4], buf1[11]);
        buf0[11] = _mm_sub_epi32(buf1[4], buf1[11]);
        buf0[5]  = _mm_add_epi32(buf1[5], buf1[10]);
        buf0[10] = _mm_sub_epi32(buf1[5], buf1[10]);
        buf0[6]  = _mm_add_epi32(buf1[6], buf1[9]);
        buf0[9]  = _mm_sub_epi32(buf1[6], buf1[9]);
        buf0[7]  = _mm_add_epi32(buf1[7], buf1[8]);
        buf0[8]  = _mm_sub_epi32(buf1[7], buf1[8]);
        buf0[16] = buf1[16];
        buf0[17] = buf1[17];
        buf0[18] = buf1[18];
        buf0[19] = buf1[19];
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, buf1[20], buf1[27], buf0[20], buf0[27], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, buf1[21], buf1[26], buf0[21], buf0[26], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, buf1[22], buf1[25], buf0[22], buf0[25], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, buf1[23], buf1[24], buf0[23], buf0[24], __rounding, cos_bit);
        buf0[28] = buf1[28];
        buf0[29] = buf1[29];
        buf0[30] = buf1[30];
        buf0[31] = buf1[31];

        // stage 3
        buf1[0] = _mm_add_epi32(buf0[0], buf0[7]);
        buf1[7] = _mm_sub_epi32(buf0[0], buf0[7]);
        buf1[1] = _mm_add_epi32(buf0[1], buf0[6]);
        buf1[6] = _mm_sub_epi32(buf0[1], buf0[6]);
        buf1[2] = _mm_add_epi32(buf0[2], buf0[5]);
        buf1[5] = _mm_sub_epi32(buf0[2], buf0[5]);
        buf1[3] = _mm_add_epi32(buf0[3], buf0[4]);
        buf1[4] = _mm_sub_epi32(buf0[3], buf0[4]);
        buf1[8] = buf0[8];
        buf1[9] = buf0[9];
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, buf0[10], buf0[13], buf1[10], buf1[13], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, buf0[11], buf0[12], buf1[11], buf1[12], __rounding, cos_bit);
        buf1[14] = buf0[14];
        buf1[15] = buf0[15];
        buf1[16] = _mm_add_epi32(buf0[16], buf0[23]);
        buf1[23] = _mm_sub_epi32(buf0[16], buf0[23]);
        buf1[17] = _mm_add_epi32(buf0[17], buf0[22]);
        buf1[22] = _mm_sub_epi32(buf0[17], buf0[22]);
        buf1[18] = _mm_add_epi32(buf0[18], buf0[21]);
        buf1[21] = _mm_sub_epi32(buf0[18], buf0[21]);
        buf1[19] = _mm_add_epi32(buf0[19], buf0[20]);
        buf1[20] = _mm_sub_epi32(buf0[19], buf0[20]);
        buf1[24] = _mm_sub_epi32(buf0[31], buf0[24]);
        buf1[31] = _mm_add_epi32(buf0[31], buf0[24]);
        buf1[25] = _mm_sub_epi32(buf0[30], buf0[25]);
        buf1[30] = _mm_add_epi32(buf0[30], buf0[25]);
        buf1[26] = _mm_sub_epi32(buf0[29], buf0[26]);
        buf1[29] = _mm_add_epi32(buf0[29], buf0[26]);
        buf1[27] = _mm_sub_epi32(buf0[28], buf0[27]);
        buf1[28] = _mm_add_epi32(buf0[28], buf0[27]);

        // stage 4
        buf0[0] = _mm_add_epi32(buf1[0], buf1[3]);
        buf0[3] = _mm_sub_epi32(buf1[0], buf1[3]);
        buf0[1] = _mm_add_epi32(buf1[1], buf1[2]);
        buf0[2] = _mm_sub_epi32(buf1[1], buf1[2]);
        buf0[4] = buf1[4];
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, buf1[5], buf1[6], buf0[5], buf0[6], __rounding, cos_bit);
        buf0[7]  = buf1[7];
        buf0[8]  = _mm_add_epi32(buf1[8], buf1[11]);
        buf0[11] = _mm_sub_epi32(buf1[8], buf1[11]);
        buf0[9]  = _mm_add_epi32(buf1[9], buf1[10]);
        buf0[10] = _mm_sub_epi32(buf1[9], buf1[10]);
        buf0[12] = _mm_sub_epi32(buf1[15], buf1[12]);
        buf0[15] = _mm_add_epi32(buf1[15], buf1[12]);
        buf0[13] = _mm_sub_epi32(buf1[14], buf1[13]);
        buf0[14] = _mm_add_epi32(buf1[14], buf1[13]);
        buf0[16] = buf1[16];
        buf0[17] = buf1[17];
        btf_32_type0_sse4_1_new(cospi_m16, cospi_p48, buf1[18], buf1[29], buf0[18], buf0[29], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m16, cospi_p48, buf1[19], buf1[28], buf0[19], buf0[28], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m48, cospi_m16, buf1[20], buf1[27], buf0[20], buf0[27], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m48, cospi_m16, buf1[21], buf1[26], buf0[21], buf0[26], __rounding, cos_bit);
        buf0[22] = buf1[22];
        buf0[23] = buf1[23];
        buf0[24] = buf1[24];
        buf0[25] = buf1[25];
        buf0[30] = buf1[30];
        buf0[31] = buf1[31];

        // stage 5
        buf1[0] = half_btf_sse4_1(&cospi_p32, &buf0[0], &cospi_p32, &buf0[1], &__rounding, cos_bit);
        buf1[2] = half_btf_sse4_1(&cospi_p48, &buf0[2], &cospi_p16, &buf0[3], &__rounding, cos_bit);
        buf1[4] = _mm_add_epi32(buf0[4], buf0[5]);
        buf1[5] = _mm_sub_epi32(buf0[4], buf0[5]);
        buf1[6] = _mm_sub_epi32(buf0[7], buf0[6]);
        buf1[7] = _mm_add_epi32(buf0[7], buf0[6]);
        buf1[8] = buf0[8];
        btf_32_type0_sse4_1_new(cospi_m16, cospi_p48, buf0[9], buf0[14], buf1[9], buf1[14], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m48, cospi_m16, buf0[10], buf0[13], buf1[10], buf1[13], __rounding, cos_bit);
        buf1[11] = buf0[11];
        buf1[12] = buf0[12];
        buf1[15] = buf0[15];
        buf1[16] = _mm_add_epi32(buf0[16], buf0[19]);
        buf1[19] = _mm_sub_epi32(buf0[16], buf0[19]);
        buf1[17] = _mm_add_epi32(buf0[17], buf0[18]);
        buf1[18] = _mm_sub_epi32(buf0[17], buf0[18]);
        buf1[20] = _mm_sub_epi32(buf0[23], buf0[20]);
        buf1[23] = _mm_add_epi32(buf0[23], buf0[20]);
        buf1[21] = _mm_sub_epi32(buf0[22], buf0[21]);
        buf1[22] = _mm_add_epi32(buf0[22], buf0[21]);
        buf1[24] = _mm_add_epi32(buf0[24], buf0[27]);
        buf1[27] = _mm_sub_epi32(buf0[24], buf0[27]);
        buf1[25] = _mm_add_epi32(buf0[25], buf0[26]);
        buf1[26] = _mm_sub_epi32(buf0[25], buf0[26]);
        buf1[28] = _mm_sub_epi32(buf0[31], buf0[28]);
        buf1[31] = _mm_add_epi32(buf0[31], buf0[28]);
        buf1[29] = _mm_sub_epi32(buf0[30], buf0[29]);
        buf1[30] = _mm_add_epi32(buf0[30], buf0[29]);

        // stage 6
        buf0[0]  = buf1[0];
        buf0[2]  = buf1[2];
        buf0[4]  = half_btf_sse4_1(&cospi_p56, &buf1[4], &cospi_p08, &buf1[7], &__rounding, cos_bit);
        buf0[6]  = half_btf_sse4_1(&cospi_p24, &buf1[6], &cospi_m40, &buf1[5], &__rounding, cos_bit);
        buf0[8]  = _mm_add_epi32(buf1[8], buf1[9]);
        buf0[9]  = _mm_sub_epi32(buf1[8], buf1[9]);
        buf0[10] = _mm_sub_epi32(buf1[11], buf1[10]);
        buf0[11] = _mm_add_epi32(buf1[11], buf1[10]);
        buf0[12] = _mm_add_epi32(buf1[12], buf1[13]);
        buf0[13] = _mm_sub_epi32(buf1[12], buf1[13]);
        buf0[14] = _mm_sub_epi32(buf1[15], buf1[14]);
        buf0[15] = _mm_add_epi32(buf1[15], buf1[14]);
        buf0[16] = buf1[16];
        btf_32_type0_sse4_1_new(cospi_m08, cospi_p56, buf1[17], buf1[30], buf0[17], buf0[30], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m56, cospi_m08, buf1[18], buf1[29], buf0[18], buf0[29], __rounding, cos_bit);
        buf0[19] = buf1[19];
        buf0[20] = buf1[20];
        btf_32_type0_sse4_1_new(cospi_m40, cospi_p24, buf1[21], buf1[26], buf0[21], buf0[26], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m24, cospi_m40, buf1[22], buf1[25], buf0[22], buf0[25], __rounding, cos_bit);
        buf0[23] = buf1[23];
        buf0[24] = buf1[24];
        buf0[27] = buf1[27];
        buf0[28] = buf1[28];
        buf0[31] = buf1[31];

        // stage 7
        buf1[0]  = buf0[0];
        buf1[2]  = buf0[2];
        buf1[4]  = buf0[4];
        buf1[6]  = buf0[6];
        buf1[8]  = half_btf_sse4_1(&cospi_p60, &buf0[8], &cospi_p04, &buf0[15], &__rounding, cos_bit);
        buf1[14] = half_btf_sse4_1(&cospi_p28, &buf0[14], &cospi_m36, &buf0[9], &__rounding, cos_bit);
        buf1[10] = half_btf_sse4_1(&cospi_p44, &buf0[10], &cospi_p20, &buf0[13], &__rounding, cos_bit);
        buf1[12] = half_btf_sse4_1(&cospi_p12, &buf0[12], &cospi_m52, &buf0[11], &__rounding, cos_bit);

        buf1[16] = _mm_add_epi32(buf0[16], buf0[17]);
        buf1[17] = _mm_sub_epi32(buf0[16], buf0[17]);
        buf1[18] = _mm_sub_epi32(buf0[19], buf0[18]);
        buf1[19] = _mm_add_epi32(buf0[19], buf0[18]);
        buf1[20] = _mm_add_epi32(buf0[20], buf0[21]);
        buf1[21] = _mm_sub_epi32(buf0[20], buf0[21]);
        buf1[22] = _mm_sub_epi32(buf0[23], buf0[22]);
        buf1[23] = _mm_add_epi32(buf0[23], buf0[22]);
        buf1[24] = _mm_add_epi32(buf0[24], buf0[25]);
        buf1[25] = _mm_sub_epi32(buf0[24], buf0[25]);
        buf1[26] = _mm_sub_epi32(buf0[27], buf0[26]);
        buf1[27] = _mm_add_epi32(buf0[27], buf0[26]);
        buf1[28] = _mm_add_epi32(buf0[28], buf0[29]);
        buf1[29] = _mm_sub_epi32(buf0[28], buf0[29]);
        buf1[30] = _mm_sub_epi32(buf0[31], buf0[30]);
        buf1[31] = _mm_add_epi32(buf0[31], buf0[30]);

        // stage 8
        buf0[0]  = buf1[0];
        buf0[2]  = buf1[2];
        buf0[4]  = buf1[4];
        buf0[6]  = buf1[6];
        buf0[8]  = buf1[8];
        buf0[10] = buf1[10];
        buf0[12] = buf1[12];
        buf0[14] = buf1[14];
        buf0[16] = half_btf_sse4_1(&cospi_p62, &buf1[16], &cospi_p02, &buf1[31], &__rounding, cos_bit);
        buf0[30] = half_btf_sse4_1(&cospi_p30, &buf1[30], &cospi_m34, &buf1[17], &__rounding, cos_bit);
        buf0[18] = half_btf_sse4_1(&cospi_p46, &buf1[18], &cospi_p18, &buf1[29], &__rounding, cos_bit);
        buf0[28] = half_btf_sse4_1(&cospi_p14, &buf1[28], &cospi_m50, &buf1[19], &__rounding, cos_bit);
        buf0[20] = half_btf_sse4_1(&cospi_p54, &buf1[20], &cospi_p10, &buf1[27], &__rounding, cos_bit);
        buf0[26] = half_btf_sse4_1(&cospi_p22, &buf1[26], &cospi_m42, &buf1[21], &__rounding, cos_bit);
        buf0[22] = half_btf_sse4_1(&cospi_p38, &buf1[22], &cospi_p26, &buf1[25], &__rounding, cos_bit);
        buf0[24] = half_btf_sse4_1(&cospi_p06, &buf1[24], &cospi_m58, &buf1[23], &__rounding, cos_bit);

        // stage 9
        out[2 * 0 * stride]  = buf0[0];
        out[2 * 1 * stride]  = buf0[16];
        out[2 * 2 * stride]  = buf0[8];
        out[2 * 3 * stride]  = buf0[24];
        out[2 * 4 * stride]  = buf0[4];
        out[2 * 5 * stride]  = buf0[20];
        out[2 * 6 * stride]  = buf0[12];
        out[2 * 7 * stride]  = buf0[28];
        out[2 * 8 * stride]  = buf0[2];
        out[2 * 9 * stride]  = buf0[18];
        out[2 * 10 * stride] = buf0[10];
        out[2 * 11 * stride] = buf0[26];
        out[2 * 12 * stride] = buf0[6];
        out[2 * 13 * stride] = buf0[22];
        out[2 * 14 * stride] = buf0[14];
        out[2 * 15 * stride] = buf0[30];
    }
}

static AOM_FORCE_INLINE void fdct32x32_N2_row_sse4_1(const __m128i* input, __m128i* output, const int8_t cos_bit) {
    const int32_t txfm_size   = 32;
    const int32_t num_per_128 = 8;
    int32_t       col_num     = txfm_size / num_per_128;
    av1_fdct32_new_N2_sse4_1(input, output, cos_bit, txfm_size / 2, col_num);
}

static AOM_FORCE_INLINE void fdct32x32_N2_col_sse4_1(const __m128i* input, __m128i* output, const int8_t cos_bit) {
    const int32_t txfm_size   = 32;
    const int32_t num_per_128 = 8;
    int32_t       col_num     = txfm_size / num_per_128;
    av1_fdct32_new_N2_sse4_1(input, output, cos_bit, txfm_size, col_num);
}

static AOM_FORCE_INLINE void av1_round_shift_array_32_N2_sse4_1(__m128i* input, __m128i* output, const int32_t size,
                                                                const int32_t bit) {
    int32_t i;

    if (bit > 0) {
        const __m128i round = _mm_set1_epi32(1 << (bit - 1));
        __m128i       r0, r1, r2, r3;
        for (i = 0; i < size; i += 4) {
            r0                = _mm_add_epi32(input[2 * i + 0], round);
            output[2 * i + 0] = _mm_srai_epi32(r0, bit);
            r1                = _mm_add_epi32(input[2 * i + 1], round);
            output[2 * i + 1] = _mm_srai_epi32(r1, bit);
            r2                = _mm_add_epi32(input[2 * i + 2], round);
            output[2 * i + 2] = _mm_srai_epi32(r2, bit);
            r3                = _mm_add_epi32(input[2 * i + 3], round);
            output[2 * i + 3] = _mm_srai_epi32(r3, bit);
        }
    } else {
        for (i = 0; i < size; i += 4) {
            output[2 * i]     = _mm_slli_epi32(input[2 * i], -bit);
            output[2 * i + 1] = _mm_slli_epi32(input[2 * i + 1], -bit);
            output[2 * i + 2] = _mm_slli_epi32(input[2 * i + 2], -bit);
            output[2 * i + 3] = _mm_slli_epi32(input[2 * i + 3], -bit);
        }
    }
}

static INLINE void av1_round_shift_rect_array_wxh_sse4_1(__m128i* input, __m128i* output, const int32_t bit,
                                                         const int32_t val, int32_t num_col, int32_t num_row) {
    const __m128i sqrt2  = _mm_set1_epi32(val);
    const __m128i round2 = _mm_set1_epi32(1 << (new_sqrt2_bits - 1));
    int32_t       i;
    if (bit > 0) {
        const __m128i round1 = _mm_set1_epi32(1 << (bit - 1));
        __m128i       r0, r1, r2, r3, r4, r5, r6, r7;
        for (i = 0; i < num_row / 2; i++) {
            for (int j = 0; j < num_col / 2; j++) {
                r0                                = _mm_add_epi32(input[2 * (i * num_col + j)], round1);
                r1                                = _mm_srai_epi32(r0, bit);
                r2                                = _mm_mullo_epi32(sqrt2, r1);
                r3                                = _mm_add_epi32(r2, round2);
                output[2 * (i * num_col + j)]     = _mm_srai_epi32(r3, new_sqrt2_bits);
                r4                                = _mm_add_epi32(input[2 * (i * num_col + j) + 1], round1);
                r5                                = _mm_srai_epi32(r4, bit);
                r6                                = _mm_mullo_epi32(sqrt2, r5);
                r7                                = _mm_add_epi32(r6, round2);
                output[2 * (i * num_col + j) + 1] = _mm_srai_epi32(r7, new_sqrt2_bits);
            }
        }
    } else {
        __m128i r0, r1, r2, r3, r4, r5;
        for (i = 0; i < num_row / 2; i++) {
            for (int j = 0; j < num_col / 2; j++) {
                r0                                = _mm_slli_epi32(input[2 * (i * num_col + j)], -bit);
                r1                                = _mm_mullo_epi32(sqrt2, r0);
                r2                                = _mm_add_epi32(r1, round2);
                output[2 * (i * num_col + j)]     = _mm_srai_epi32(r2, new_sqrt2_bits);
                r3                                = _mm_slli_epi32(input[2 * (i * num_col + j) + 1], -bit);
                r4                                = _mm_mullo_epi32(sqrt2, r3);
                r5                                = _mm_add_epi32(r4, round2);
                output[2 * (i * num_col + j) + 1] = _mm_srai_epi32(r5, new_sqrt2_bits);
            }
        }
    }
}

static INLINE void load_buffer_32x32_in_64x64_sse4_1(const int16_t* input, int32_t stride, __m128i* output) {
    int32_t i;

    for (i = 0; i < 32; ++i) {
        output[0] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 0 * 4)));
        output[1] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 1 * 4)));
        output[2] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 2 * 4)));
        output[3] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 3 * 4)));
        output[4] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 4 * 4)));
        output[5] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 5 * 4)));
        output[6] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 6 * 4)));
        output[7] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 7 * 4)));

        input += stride;
        output += 16;
    }
}

static void av1_fdct64_new_N2_sse4_1(const __m128i* input, __m128i* output, int8_t cos_bit, const int32_t col_num,
                                     const int32_t stride) {
    const int32_t* cospi      = cospi_arr(cos_bit);
    const __m128i  __rounding = _mm_set1_epi32(1 << (cos_bit - 1));
    const int32_t  columns    = col_num >> 2;

    __m128i cospi_m32 = _mm_set1_epi32(-cospi[32]);
    __m128i cospi_p32 = _mm_set1_epi32(cospi[32]);
    __m128i cospi_m16 = _mm_set1_epi32(-cospi[16]);
    __m128i cospi_p48 = _mm_set1_epi32(cospi[48]);
    __m128i cospi_m48 = _mm_set1_epi32(-cospi[48]);
    __m128i cospi_p16 = _mm_set1_epi32(cospi[16]);
    __m128i cospi_m08 = _mm_set1_epi32(-cospi[8]);
    __m128i cospi_p56 = _mm_set1_epi32(cospi[56]);
    __m128i cospi_m56 = _mm_set1_epi32(-cospi[56]);
    __m128i cospi_m40 = _mm_set1_epi32(-cospi[40]);
    __m128i cospi_p24 = _mm_set1_epi32(cospi[24]);
    __m128i cospi_m24 = _mm_set1_epi32(-cospi[24]);
    __m128i cospi_p08 = _mm_set1_epi32(cospi[8]);
    __m128i cospi_p60 = _mm_set1_epi32(cospi[60]);
    __m128i cospi_p04 = _mm_set1_epi32(cospi[4]);
    __m128i cospi_p28 = _mm_set1_epi32(cospi[28]);
    __m128i cospi_p44 = _mm_set1_epi32(cospi[44]);
    __m128i cospi_p20 = _mm_set1_epi32(cospi[20]);
    __m128i cospi_p12 = _mm_set1_epi32(cospi[12]);
    __m128i cospi_m04 = _mm_set1_epi32(-cospi[4]);
    __m128i cospi_m60 = _mm_set1_epi32(-cospi[60]);
    __m128i cospi_m36 = _mm_set1_epi32(-cospi[36]);
    __m128i cospi_m28 = _mm_set1_epi32(-cospi[28]);
    __m128i cospi_m20 = _mm_set1_epi32(-cospi[20]);
    __m128i cospi_m44 = _mm_set1_epi32(-cospi[44]);
    __m128i cospi_m52 = _mm_set1_epi32(-cospi[52]);
    __m128i cospi_m12 = _mm_set1_epi32(-cospi[12]);
    __m128i cospi_p62 = _mm_set1_epi32(cospi[62]);
    __m128i cospi_p02 = _mm_set1_epi32(cospi[2]);
    __m128i cospi_p30 = _mm_set1_epi32(cospi[30]);
    __m128i cospi_m34 = _mm_set1_epi32(-cospi[34]);
    __m128i cospi_p46 = _mm_set1_epi32(cospi[46]);
    __m128i cospi_p18 = _mm_set1_epi32(cospi[18]);
    __m128i cospi_p14 = _mm_set1_epi32(cospi[14]);
    __m128i cospi_m50 = _mm_set1_epi32(-cospi[50]);
    __m128i cospi_p54 = _mm_set1_epi32(cospi[54]);
    __m128i cospi_p10 = _mm_set1_epi32(cospi[10]);
    __m128i cospi_p22 = _mm_set1_epi32(cospi[22]);
    __m128i cospi_m42 = _mm_set1_epi32(-cospi[42]);
    __m128i cospi_p38 = _mm_set1_epi32(cospi[38]);
    __m128i cospi_p26 = _mm_set1_epi32(cospi[26]);
    __m128i cospi_p06 = _mm_set1_epi32(cospi[6]);
    __m128i cospi_m58 = _mm_set1_epi32(-cospi[58]);
    __m128i cospi_p63 = _mm_set1_epi32(cospi[63]);
    __m128i cospi_p01 = _mm_set1_epi32(cospi[1]);
    __m128i cospi_p31 = _mm_set1_epi32(cospi[31]);
    __m128i cospi_m33 = _mm_set1_epi32(-cospi[33]);
    __m128i cospi_p47 = _mm_set1_epi32(cospi[47]);
    __m128i cospi_p17 = _mm_set1_epi32(cospi[17]);
    __m128i cospi_p15 = _mm_set1_epi32(cospi[15]);
    __m128i cospi_m49 = _mm_set1_epi32(-cospi[49]);
    __m128i cospi_p55 = _mm_set1_epi32(cospi[55]);
    __m128i cospi_p09 = _mm_set1_epi32(cospi[9]);
    __m128i cospi_p23 = _mm_set1_epi32(cospi[23]);
    __m128i cospi_m41 = _mm_set1_epi32(-cospi[41]);
    __m128i cospi_p39 = _mm_set1_epi32(cospi[39]);
    __m128i cospi_p25 = _mm_set1_epi32(cospi[25]);
    __m128i cospi_p07 = _mm_set1_epi32(cospi[7]);
    __m128i cospi_m57 = _mm_set1_epi32(-cospi[57]);
    __m128i cospi_p59 = _mm_set1_epi32(cospi[59]);
    __m128i cospi_p05 = _mm_set1_epi32(cospi[5]);
    __m128i cospi_p27 = _mm_set1_epi32(cospi[27]);
    __m128i cospi_m37 = _mm_set1_epi32(-cospi[37]);
    __m128i cospi_p43 = _mm_set1_epi32(cospi[43]);
    __m128i cospi_p21 = _mm_set1_epi32(cospi[21]);
    __m128i cospi_p11 = _mm_set1_epi32(cospi[11]);
    __m128i cospi_m53 = _mm_set1_epi32(-cospi[53]);
    __m128i cospi_p51 = _mm_set1_epi32(cospi[51]);
    __m128i cospi_p13 = _mm_set1_epi32(cospi[13]);
    __m128i cospi_p19 = _mm_set1_epi32(cospi[19]);
    __m128i cospi_m45 = _mm_set1_epi32(-cospi[45]);
    __m128i cospi_p35 = _mm_set1_epi32(cospi[35]);
    __m128i cospi_p29 = _mm_set1_epi32(cospi[29]);
    __m128i cospi_p03 = _mm_set1_epi32(cospi[3]);
    __m128i cospi_m61 = _mm_set1_epi32(-cospi[61]);

    for (int32_t col = 0; col < columns; col++) {
        const __m128i* in  = &input[col];
        __m128i*       out = &output[col];

        // stage 1
        __m128i x1[64];
        x1[0]  = _mm_add_epi32(in[2 * 0 * stride], in[2 * 63 * stride]);
        x1[63] = _mm_sub_epi32(in[2 * 0 * stride], in[2 * 63 * stride]);
        x1[1]  = _mm_add_epi32(in[2 * 1 * stride], in[2 * 62 * stride]);
        x1[62] = _mm_sub_epi32(in[2 * 1 * stride], in[2 * 62 * stride]);
        x1[2]  = _mm_add_epi32(in[2 * 2 * stride], in[2 * 61 * stride]);
        x1[61] = _mm_sub_epi32(in[2 * 2 * stride], in[2 * 61 * stride]);
        x1[3]  = _mm_add_epi32(in[2 * 3 * stride], in[2 * 60 * stride]);
        x1[60] = _mm_sub_epi32(in[2 * 3 * stride], in[2 * 60 * stride]);
        x1[4]  = _mm_add_epi32(in[2 * 4 * stride], in[2 * 59 * stride]);
        x1[59] = _mm_sub_epi32(in[2 * 4 * stride], in[2 * 59 * stride]);
        x1[5]  = _mm_add_epi32(in[2 * 5 * stride], in[2 * 58 * stride]);
        x1[58] = _mm_sub_epi32(in[2 * 5 * stride], in[2 * 58 * stride]);
        x1[6]  = _mm_add_epi32(in[2 * 6 * stride], in[2 * 57 * stride]);
        x1[57] = _mm_sub_epi32(in[2 * 6 * stride], in[2 * 57 * stride]);
        x1[7]  = _mm_add_epi32(in[2 * 7 * stride], in[2 * 56 * stride]);
        x1[56] = _mm_sub_epi32(in[2 * 7 * stride], in[2 * 56 * stride]);
        x1[8]  = _mm_add_epi32(in[2 * 8 * stride], in[2 * 55 * stride]);
        x1[55] = _mm_sub_epi32(in[2 * 8 * stride], in[2 * 55 * stride]);
        x1[9]  = _mm_add_epi32(in[2 * 9 * stride], in[2 * 54 * stride]);
        x1[54] = _mm_sub_epi32(in[2 * 9 * stride], in[2 * 54 * stride]);
        x1[10] = _mm_add_epi32(in[2 * 10 * stride], in[2 * 53 * stride]);
        x1[53] = _mm_sub_epi32(in[2 * 10 * stride], in[2 * 53 * stride]);
        x1[11] = _mm_add_epi32(in[2 * 11 * stride], in[2 * 52 * stride]);
        x1[52] = _mm_sub_epi32(in[2 * 11 * stride], in[2 * 52 * stride]);
        x1[12] = _mm_add_epi32(in[2 * 12 * stride], in[2 * 51 * stride]);
        x1[51] = _mm_sub_epi32(in[2 * 12 * stride], in[2 * 51 * stride]);
        x1[13] = _mm_add_epi32(in[2 * 13 * stride], in[2 * 50 * stride]);
        x1[50] = _mm_sub_epi32(in[2 * 13 * stride], in[2 * 50 * stride]);
        x1[14] = _mm_add_epi32(in[2 * 14 * stride], in[2 * 49 * stride]);
        x1[49] = _mm_sub_epi32(in[2 * 14 * stride], in[2 * 49 * stride]);
        x1[15] = _mm_add_epi32(in[2 * 15 * stride], in[2 * 48 * stride]);
        x1[48] = _mm_sub_epi32(in[2 * 15 * stride], in[2 * 48 * stride]);
        x1[16] = _mm_add_epi32(in[2 * 16 * stride], in[2 * 47 * stride]);
        x1[47] = _mm_sub_epi32(in[2 * 16 * stride], in[2 * 47 * stride]);
        x1[17] = _mm_add_epi32(in[2 * 17 * stride], in[2 * 46 * stride]);
        x1[46] = _mm_sub_epi32(in[2 * 17 * stride], in[2 * 46 * stride]);
        x1[18] = _mm_add_epi32(in[2 * 18 * stride], in[2 * 45 * stride]);
        x1[45] = _mm_sub_epi32(in[2 * 18 * stride], in[2 * 45 * stride]);
        x1[19] = _mm_add_epi32(in[2 * 19 * stride], in[2 * 44 * stride]);
        x1[44] = _mm_sub_epi32(in[2 * 19 * stride], in[2 * 44 * stride]);
        x1[20] = _mm_add_epi32(in[2 * 20 * stride], in[2 * 43 * stride]);
        x1[43] = _mm_sub_epi32(in[2 * 20 * stride], in[2 * 43 * stride]);
        x1[21] = _mm_add_epi32(in[2 * 21 * stride], in[2 * 42 * stride]);
        x1[42] = _mm_sub_epi32(in[2 * 21 * stride], in[2 * 42 * stride]);
        x1[22] = _mm_add_epi32(in[2 * 22 * stride], in[2 * 41 * stride]);
        x1[41] = _mm_sub_epi32(in[2 * 22 * stride], in[2 * 41 * stride]);
        x1[23] = _mm_add_epi32(in[2 * 23 * stride], in[2 * 40 * stride]);
        x1[40] = _mm_sub_epi32(in[2 * 23 * stride], in[2 * 40 * stride]);
        x1[24] = _mm_add_epi32(in[2 * 24 * stride], in[2 * 39 * stride]);
        x1[39] = _mm_sub_epi32(in[2 * 24 * stride], in[2 * 39 * stride]);
        x1[25] = _mm_add_epi32(in[2 * 25 * stride], in[2 * 38 * stride]);
        x1[38] = _mm_sub_epi32(in[2 * 25 * stride], in[2 * 38 * stride]);
        x1[26] = _mm_add_epi32(in[2 * 26 * stride], in[2 * 37 * stride]);
        x1[37] = _mm_sub_epi32(in[2 * 26 * stride], in[2 * 37 * stride]);
        x1[27] = _mm_add_epi32(in[2 * 27 * stride], in[2 * 36 * stride]);
        x1[36] = _mm_sub_epi32(in[2 * 27 * stride], in[2 * 36 * stride]);
        x1[28] = _mm_add_epi32(in[2 * 28 * stride], in[2 * 35 * stride]);
        x1[35] = _mm_sub_epi32(in[2 * 28 * stride], in[2 * 35 * stride]);
        x1[29] = _mm_add_epi32(in[2 * 29 * stride], in[2 * 34 * stride]);
        x1[34] = _mm_sub_epi32(in[2 * 29 * stride], in[2 * 34 * stride]);
        x1[30] = _mm_add_epi32(in[2 * 30 * stride], in[2 * 33 * stride]);
        x1[33] = _mm_sub_epi32(in[2 * 30 * stride], in[2 * 33 * stride]);
        x1[31] = _mm_add_epi32(in[2 * 31 * stride], in[2 * 32 * stride]);
        x1[32] = _mm_sub_epi32(in[2 * 31 * stride], in[2 * 32 * stride]);

        // stage 2
        __m128i x2[64];
        x2[0]  = _mm_add_epi32(x1[0], x1[31]);
        x2[31] = _mm_sub_epi32(x1[0], x1[31]);
        x2[1]  = _mm_add_epi32(x1[1], x1[30]);
        x2[30] = _mm_sub_epi32(x1[1], x1[30]);
        x2[2]  = _mm_add_epi32(x1[2], x1[29]);
        x2[29] = _mm_sub_epi32(x1[2], x1[29]);
        x2[3]  = _mm_add_epi32(x1[3], x1[28]);
        x2[28] = _mm_sub_epi32(x1[3], x1[28]);
        x2[4]  = _mm_add_epi32(x1[4], x1[27]);
        x2[27] = _mm_sub_epi32(x1[4], x1[27]);
        x2[5]  = _mm_add_epi32(x1[5], x1[26]);
        x2[26] = _mm_sub_epi32(x1[5], x1[26]);
        x2[6]  = _mm_add_epi32(x1[6], x1[25]);
        x2[25] = _mm_sub_epi32(x1[6], x1[25]);
        x2[7]  = _mm_add_epi32(x1[7], x1[24]);
        x2[24] = _mm_sub_epi32(x1[7], x1[24]);
        x2[8]  = _mm_add_epi32(x1[8], x1[23]);
        x2[23] = _mm_sub_epi32(x1[8], x1[23]);
        x2[9]  = _mm_add_epi32(x1[9], x1[22]);
        x2[22] = _mm_sub_epi32(x1[9], x1[22]);
        x2[10] = _mm_add_epi32(x1[10], x1[21]);
        x2[21] = _mm_sub_epi32(x1[10], x1[21]);
        x2[11] = _mm_add_epi32(x1[11], x1[20]);
        x2[20] = _mm_sub_epi32(x1[11], x1[20]);
        x2[12] = _mm_add_epi32(x1[12], x1[19]);
        x2[19] = _mm_sub_epi32(x1[12], x1[19]);
        x2[13] = _mm_add_epi32(x1[13], x1[18]);
        x2[18] = _mm_sub_epi32(x1[13], x1[18]);
        x2[14] = _mm_add_epi32(x1[14], x1[17]);
        x2[17] = _mm_sub_epi32(x1[14], x1[17]);
        x2[15] = _mm_add_epi32(x1[15], x1[16]);
        x2[16] = _mm_sub_epi32(x1[15], x1[16]);
        x2[32] = x1[32];
        x2[33] = x1[33];
        x2[34] = x1[34];
        x2[35] = x1[35];
        x2[36] = x1[36];
        x2[37] = x1[37];
        x2[38] = x1[38];
        x2[39] = x1[39];
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x1[40], x1[55], x2[40], x2[55], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x1[41], x1[54], x2[41], x2[54], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x1[42], x1[53], x2[42], x2[53], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x1[43], x1[52], x2[43], x2[52], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x1[44], x1[51], x2[44], x2[51], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x1[45], x1[50], x2[45], x2[50], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x1[46], x1[49], x2[46], x2[49], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x1[47], x1[48], x2[47], x2[48], __rounding, cos_bit);
        x2[56] = x1[56];
        x2[57] = x1[57];
        x2[58] = x1[58];
        x2[59] = x1[59];
        x2[60] = x1[60];
        x2[61] = x1[61];
        x2[62] = x1[62];
        x2[63] = x1[63];

        // stage 3
        __m128i x3[64];
        x3[0]  = _mm_add_epi32(x2[0], x2[15]);
        x3[15] = _mm_sub_epi32(x2[0], x2[15]);
        x3[1]  = _mm_add_epi32(x2[1], x2[14]);
        x3[14] = _mm_sub_epi32(x2[1], x2[14]);
        x3[2]  = _mm_add_epi32(x2[2], x2[13]);
        x3[13] = _mm_sub_epi32(x2[2], x2[13]);
        x3[3]  = _mm_add_epi32(x2[3], x2[12]);
        x3[12] = _mm_sub_epi32(x2[3], x2[12]);
        x3[4]  = _mm_add_epi32(x2[4], x2[11]);
        x3[11] = _mm_sub_epi32(x2[4], x2[11]);
        x3[5]  = _mm_add_epi32(x2[5], x2[10]);
        x3[10] = _mm_sub_epi32(x2[5], x2[10]);
        x3[6]  = _mm_add_epi32(x2[6], x2[9]);
        x3[9]  = _mm_sub_epi32(x2[6], x2[9]);
        x3[7]  = _mm_add_epi32(x2[7], x2[8]);
        x3[8]  = _mm_sub_epi32(x2[7], x2[8]);
        x3[16] = x2[16];
        x3[17] = x2[17];
        x3[18] = x2[18];
        x3[19] = x2[19];
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x2[20], x2[27], x3[20], x3[27], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x2[21], x2[26], x3[21], x3[26], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x2[22], x2[25], x3[22], x3[25], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x2[23], x2[24], x3[23], x3[24], __rounding, cos_bit);
        x3[28] = x2[28];
        x3[29] = x2[29];
        x3[30] = x2[30];
        x3[31] = x2[31];
        x3[32] = _mm_add_epi32(x2[32], x2[47]);
        x3[47] = _mm_sub_epi32(x2[32], x2[47]);
        x3[33] = _mm_add_epi32(x2[33], x2[46]);
        x3[46] = _mm_sub_epi32(x2[33], x2[46]);
        x3[34] = _mm_add_epi32(x2[34], x2[45]);
        x3[45] = _mm_sub_epi32(x2[34], x2[45]);
        x3[35] = _mm_add_epi32(x2[35], x2[44]);
        x3[44] = _mm_sub_epi32(x2[35], x2[44]);
        x3[36] = _mm_add_epi32(x2[36], x2[43]);
        x3[43] = _mm_sub_epi32(x2[36], x2[43]);
        x3[37] = _mm_add_epi32(x2[37], x2[42]);
        x3[42] = _mm_sub_epi32(x2[37], x2[42]);
        x3[38] = _mm_add_epi32(x2[38], x2[41]);
        x3[41] = _mm_sub_epi32(x2[38], x2[41]);
        x3[39] = _mm_add_epi32(x2[39], x2[40]);
        x3[40] = _mm_sub_epi32(x2[39], x2[40]);
        x3[48] = _mm_sub_epi32(x2[63], x2[48]);
        x3[63] = _mm_add_epi32(x2[63], x2[48]);
        x3[49] = _mm_sub_epi32(x2[62], x2[49]);
        x3[62] = _mm_add_epi32(x2[62], x2[49]);
        x3[50] = _mm_sub_epi32(x2[61], x2[50]);
        x3[61] = _mm_add_epi32(x2[61], x2[50]);
        x3[51] = _mm_sub_epi32(x2[60], x2[51]);
        x3[60] = _mm_add_epi32(x2[60], x2[51]);
        x3[52] = _mm_sub_epi32(x2[59], x2[52]);
        x3[59] = _mm_add_epi32(x2[59], x2[52]);
        x3[53] = _mm_sub_epi32(x2[58], x2[53]);
        x3[58] = _mm_add_epi32(x2[58], x2[53]);
        x3[54] = _mm_sub_epi32(x2[57], x2[54]);
        x3[57] = _mm_add_epi32(x2[57], x2[54]);
        x3[55] = _mm_sub_epi32(x2[56], x2[55]);
        x3[56] = _mm_add_epi32(x2[56], x2[55]);

        // stage 4
        __m128i x4[64];
        x4[0] = _mm_add_epi32(x3[0], x3[7]);
        x4[7] = _mm_sub_epi32(x3[0], x3[7]);
        x4[1] = _mm_add_epi32(x3[1], x3[6]);
        x4[6] = _mm_sub_epi32(x3[1], x3[6]);
        x4[2] = _mm_add_epi32(x3[2], x3[5]);
        x4[5] = _mm_sub_epi32(x3[2], x3[5]);
        x4[3] = _mm_add_epi32(x3[3], x3[4]);
        x4[4] = _mm_sub_epi32(x3[3], x3[4]);
        x4[8] = x3[8];
        x4[9] = x3[9];
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x3[10], x3[13], x4[10], x4[13], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x3[11], x3[12], x4[11], x4[12], __rounding, cos_bit);
        x4[14] = x3[14];
        x4[15] = x3[15];
        x4[16] = _mm_add_epi32(x3[16], x3[23]);
        x4[23] = _mm_sub_epi32(x3[16], x3[23]);
        x4[17] = _mm_add_epi32(x3[17], x3[22]);
        x4[22] = _mm_sub_epi32(x3[17], x3[22]);
        x4[18] = _mm_add_epi32(x3[18], x3[21]);
        x4[21] = _mm_sub_epi32(x3[18], x3[21]);
        x4[19] = _mm_add_epi32(x3[19], x3[20]);
        x4[20] = _mm_sub_epi32(x3[19], x3[20]);
        x4[24] = _mm_sub_epi32(x3[31], x3[24]);
        x4[31] = _mm_add_epi32(x3[31], x3[24]);
        x4[25] = _mm_sub_epi32(x3[30], x3[25]);
        x4[30] = _mm_add_epi32(x3[30], x3[25]);
        x4[26] = _mm_sub_epi32(x3[29], x3[26]);
        x4[29] = _mm_add_epi32(x3[29], x3[26]);
        x4[27] = _mm_sub_epi32(x3[28], x3[27]);
        x4[28] = _mm_add_epi32(x3[28], x3[27]);
        x4[32] = x3[32];
        x4[33] = x3[33];
        x4[34] = x3[34];
        x4[35] = x3[35];
        btf_32_type0_sse4_1_new(cospi_m16, cospi_p48, x3[36], x3[59], x4[36], x4[59], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m16, cospi_p48, x3[37], x3[58], x4[37], x4[58], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m16, cospi_p48, x3[38], x3[57], x4[38], x4[57], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m16, cospi_p48, x3[39], x3[56], x4[39], x4[56], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m48, cospi_m16, x3[40], x3[55], x4[40], x4[55], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m48, cospi_m16, x3[41], x3[54], x4[41], x4[54], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m48, cospi_m16, x3[42], x3[53], x4[42], x4[53], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m48, cospi_m16, x3[43], x3[52], x4[43], x4[52], __rounding, cos_bit);
        x4[44] = x3[44];
        x4[45] = x3[45];
        x4[46] = x3[46];
        x4[47] = x3[47];
        x4[48] = x3[48];
        x4[49] = x3[49];
        x4[50] = x3[50];
        x4[51] = x3[51];
        x4[60] = x3[60];
        x4[61] = x3[61];
        x4[62] = x3[62];
        x4[63] = x3[63];

        // stage 5
        __m128i x5[64];
        x5[0] = _mm_add_epi32(x4[0], x4[3]);
        x5[3] = _mm_sub_epi32(x4[0], x4[3]);
        x5[1] = _mm_add_epi32(x4[1], x4[2]);
        x5[2] = _mm_sub_epi32(x4[1], x4[2]);
        x5[4] = x4[4];
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x4[5], x4[6], x5[5], x5[6], __rounding, cos_bit);
        x5[7]  = x4[7];
        x5[8]  = _mm_add_epi32(x4[8], x4[11]);
        x5[11] = _mm_sub_epi32(x4[8], x4[11]);
        x5[9]  = _mm_add_epi32(x4[9], x4[10]);
        x5[10] = _mm_sub_epi32(x4[9], x4[10]);
        x5[12] = _mm_sub_epi32(x4[15], x4[12]);
        x5[15] = _mm_add_epi32(x4[15], x4[12]);
        x5[13] = _mm_sub_epi32(x4[14], x4[13]);
        x5[14] = _mm_add_epi32(x4[14], x4[13]);
        x5[16] = x4[16];
        x5[17] = x4[17];
        btf_32_type0_sse4_1_new(cospi_m16, cospi_p48, x4[18], x4[29], x5[18], x5[29], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m16, cospi_p48, x4[19], x4[28], x5[19], x5[28], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m48, cospi_m16, x4[20], x4[27], x5[20], x5[27], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m48, cospi_m16, x4[21], x4[26], x5[21], x5[26], __rounding, cos_bit);
        x5[22] = x4[22];
        x5[23] = x4[23];
        x5[24] = x4[24];
        x5[25] = x4[25];
        x5[30] = x4[30];
        x5[31] = x4[31];
        x5[32] = _mm_add_epi32(x4[32], x4[39]);
        x5[39] = _mm_sub_epi32(x4[32], x4[39]);
        x5[33] = _mm_add_epi32(x4[33], x4[38]);
        x5[38] = _mm_sub_epi32(x4[33], x4[38]);
        x5[34] = _mm_add_epi32(x4[34], x4[37]);
        x5[37] = _mm_sub_epi32(x4[34], x4[37]);
        x5[35] = _mm_add_epi32(x4[35], x4[36]);
        x5[36] = _mm_sub_epi32(x4[35], x4[36]);
        x5[40] = _mm_sub_epi32(x4[47], x4[40]);
        x5[47] = _mm_add_epi32(x4[47], x4[40]);
        x5[41] = _mm_sub_epi32(x4[46], x4[41]);
        x5[46] = _mm_add_epi32(x4[46], x4[41]);
        x5[42] = _mm_sub_epi32(x4[45], x4[42]);
        x5[45] = _mm_add_epi32(x4[45], x4[42]);
        x5[43] = _mm_sub_epi32(x4[44], x4[43]);
        x5[44] = _mm_add_epi32(x4[44], x4[43]);
        x5[48] = _mm_add_epi32(x4[48], x4[55]);
        x5[55] = _mm_sub_epi32(x4[48], x4[55]);
        x5[49] = _mm_add_epi32(x4[49], x4[54]);
        x5[54] = _mm_sub_epi32(x4[49], x4[54]);
        x5[50] = _mm_add_epi32(x4[50], x4[53]);
        x5[53] = _mm_sub_epi32(x4[50], x4[53]);
        x5[51] = _mm_add_epi32(x4[51], x4[52]);
        x5[52] = _mm_sub_epi32(x4[51], x4[52]);
        x5[56] = _mm_sub_epi32(x4[63], x4[56]);
        x5[63] = _mm_add_epi32(x4[63], x4[56]);
        x5[57] = _mm_sub_epi32(x4[62], x4[57]);
        x5[62] = _mm_add_epi32(x4[62], x4[57]);
        x5[58] = _mm_sub_epi32(x4[61], x4[58]);
        x5[61] = _mm_add_epi32(x4[61], x4[58]);
        x5[59] = _mm_sub_epi32(x4[60], x4[59]);
        x5[60] = _mm_add_epi32(x4[60], x4[59]);

        // stage 6
        __m128i x6[64];
        x6[0] = half_btf_sse4_1(&cospi_p32, &x5[0], &cospi_p32, &x5[1], &__rounding, cos_bit);
        x6[2] = half_btf_sse4_1(&cospi_p48, &x5[2], &cospi_p16, &x5[3], &__rounding, cos_bit);
        x6[4] = _mm_add_epi32(x5[4], x5[5]);
        x6[5] = _mm_sub_epi32(x5[4], x5[5]);
        x6[6] = _mm_sub_epi32(x5[7], x5[6]);
        x6[7] = _mm_add_epi32(x5[7], x5[6]);
        x6[8] = x5[8];
        btf_32_type0_sse4_1_new(cospi_m16, cospi_p48, x5[9], x5[14], x6[9], x6[14], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m48, cospi_m16, x5[10], x5[13], x6[10], x6[13], __rounding, cos_bit);
        x6[11] = x5[11];
        x6[12] = x5[12];
        x6[15] = x5[15];
        x6[16] = _mm_add_epi32(x5[16], x5[19]);
        x6[19] = _mm_sub_epi32(x5[16], x5[19]);
        x6[17] = _mm_add_epi32(x5[17], x5[18]);
        x6[18] = _mm_sub_epi32(x5[17], x5[18]);
        x6[20] = _mm_sub_epi32(x5[23], x5[20]);
        x6[23] = _mm_add_epi32(x5[23], x5[20]);
        x6[21] = _mm_sub_epi32(x5[22], x5[21]);
        x6[22] = _mm_add_epi32(x5[22], x5[21]);
        x6[24] = _mm_add_epi32(x5[24], x5[27]);
        x6[27] = _mm_sub_epi32(x5[24], x5[27]);
        x6[25] = _mm_add_epi32(x5[25], x5[26]);
        x6[26] = _mm_sub_epi32(x5[25], x5[26]);
        x6[28] = _mm_sub_epi32(x5[31], x5[28]);
        x6[31] = _mm_add_epi32(x5[31], x5[28]);
        x6[29] = _mm_sub_epi32(x5[30], x5[29]);
        x6[30] = _mm_add_epi32(x5[30], x5[29]);
        x6[32] = x5[32];
        x6[33] = x5[33];
        btf_32_type0_sse4_1_new(cospi_m08, cospi_p56, x5[34], x5[61], x6[34], x6[61], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m08, cospi_p56, x5[35], x5[60], x6[35], x6[60], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m56, cospi_m08, x5[36], x5[59], x6[36], x6[59], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m56, cospi_m08, x5[37], x5[58], x6[37], x6[58], __rounding, cos_bit);
        x6[38] = x5[38];
        x6[39] = x5[39];
        x6[40] = x5[40];
        x6[41] = x5[41];
        btf_32_type0_sse4_1_new(cospi_m40, cospi_p24, x5[42], x5[53], x6[42], x6[53], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m40, cospi_p24, x5[43], x5[52], x6[43], x6[52], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m24, cospi_m40, x5[44], x5[51], x6[44], x6[51], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m24, cospi_m40, x5[45], x5[50], x6[45], x6[50], __rounding, cos_bit);
        x6[46] = x5[46];
        x6[47] = x5[47];
        x6[48] = x5[48];
        x6[49] = x5[49];
        x6[54] = x5[54];
        x6[55] = x5[55];
        x6[56] = x5[56];
        x6[57] = x5[57];
        x6[62] = x5[62];
        x6[63] = x5[63];

        // stage 7
        __m128i x7[64];
        x7[0] = x6[0];
        //x7[1] = x6[1];
        x7[2] = x6[2];
        //x7[3] = x6[3];
        x7[4]  = half_btf_sse4_1(&cospi_p56, &x6[4], &cospi_p08, &x6[7], &__rounding, cos_bit);
        x7[6]  = half_btf_sse4_1(&cospi_p24, &x6[6], &cospi_m40, &x6[5], &__rounding, cos_bit);
        x7[8]  = _mm_add_epi32(x6[8], x6[9]);
        x7[9]  = _mm_sub_epi32(x6[8], x6[9]);
        x7[10] = _mm_sub_epi32(x6[11], x6[10]);
        x7[11] = _mm_add_epi32(x6[11], x6[10]);
        x7[12] = _mm_add_epi32(x6[12], x6[13]);
        x7[13] = _mm_sub_epi32(x6[12], x6[13]);
        x7[14] = _mm_sub_epi32(x6[15], x6[14]);
        x7[15] = _mm_add_epi32(x6[15], x6[14]);
        x7[16] = x6[16];
        btf_32_type0_sse4_1_new(cospi_m08, cospi_p56, x6[17], x6[30], x7[17], x7[30], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m56, cospi_m08, x6[18], x6[29], x7[18], x7[29], __rounding, cos_bit);
        x7[19] = x6[19];
        x7[20] = x6[20];
        btf_32_type0_sse4_1_new(cospi_m40, cospi_p24, x6[21], x6[26], x7[21], x7[26], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m24, cospi_m40, x6[22], x6[25], x7[22], x7[25], __rounding, cos_bit);
        x7[23] = x6[23];
        x7[24] = x6[24];
        x7[27] = x6[27];
        x7[28] = x6[28];
        x7[31] = x6[31];
        x7[32] = _mm_add_epi32(x6[32], x6[35]);
        x7[35] = _mm_sub_epi32(x6[32], x6[35]);
        x7[33] = _mm_add_epi32(x6[33], x6[34]);
        x7[34] = _mm_sub_epi32(x6[33], x6[34]);
        x7[36] = _mm_sub_epi32(x6[39], x6[36]);
        x7[39] = _mm_add_epi32(x6[39], x6[36]);
        x7[37] = _mm_sub_epi32(x6[38], x6[37]);
        x7[38] = _mm_add_epi32(x6[38], x6[37]);
        x7[40] = _mm_add_epi32(x6[40], x6[43]);
        x7[43] = _mm_sub_epi32(x6[40], x6[43]);
        x7[41] = _mm_add_epi32(x6[41], x6[42]);
        x7[42] = _mm_sub_epi32(x6[41], x6[42]);
        x7[44] = _mm_sub_epi32(x6[47], x6[44]);
        x7[47] = _mm_add_epi32(x6[47], x6[44]);
        x7[45] = _mm_sub_epi32(x6[46], x6[45]);
        x7[46] = _mm_add_epi32(x6[46], x6[45]);
        x7[48] = _mm_add_epi32(x6[48], x6[51]);
        x7[51] = _mm_sub_epi32(x6[48], x6[51]);
        x7[49] = _mm_add_epi32(x6[49], x6[50]);
        x7[50] = _mm_sub_epi32(x6[49], x6[50]);
        x7[52] = _mm_sub_epi32(x6[55], x6[52]);
        x7[55] = _mm_add_epi32(x6[55], x6[52]);
        x7[53] = _mm_sub_epi32(x6[54], x6[53]);
        x7[54] = _mm_add_epi32(x6[54], x6[53]);
        x7[56] = _mm_add_epi32(x6[56], x6[59]);
        x7[59] = _mm_sub_epi32(x6[56], x6[59]);
        x7[57] = _mm_add_epi32(x6[57], x6[58]);
        x7[58] = _mm_sub_epi32(x6[57], x6[58]);
        x7[60] = _mm_sub_epi32(x6[63], x6[60]);
        x7[63] = _mm_add_epi32(x6[63], x6[60]);
        x7[61] = _mm_sub_epi32(x6[62], x6[61]);
        x7[62] = _mm_add_epi32(x6[62], x6[61]);

        // stage 8
        __m128i x8[64];
        x8[0] = x7[0];
        x8[2] = x7[2];
        x8[4] = x7[4];
        x8[6] = x7[6];

        x8[8]  = half_btf_sse4_1(&cospi_p60, &x7[8], &cospi_p04, &x7[15], &__rounding, cos_bit);
        x8[14] = half_btf_sse4_1(&cospi_p28, &x7[14], &cospi_m36, &x7[9], &__rounding, cos_bit);
        x8[10] = half_btf_sse4_1(&cospi_p44, &x7[10], &cospi_p20, &x7[13], &__rounding, cos_bit);
        x8[12] = half_btf_sse4_1(&cospi_p12, &x7[12], &cospi_m52, &x7[11], &__rounding, cos_bit);
        x8[16] = _mm_add_epi32(x7[16], x7[17]);
        x8[17] = _mm_sub_epi32(x7[16], x7[17]);
        x8[18] = _mm_sub_epi32(x7[19], x7[18]);
        x8[19] = _mm_add_epi32(x7[19], x7[18]);
        x8[20] = _mm_add_epi32(x7[20], x7[21]);
        x8[21] = _mm_sub_epi32(x7[20], x7[21]);
        x8[22] = _mm_sub_epi32(x7[23], x7[22]);
        x8[23] = _mm_add_epi32(x7[23], x7[22]);
        x8[24] = _mm_add_epi32(x7[24], x7[25]);
        x8[25] = _mm_sub_epi32(x7[24], x7[25]);
        x8[26] = _mm_sub_epi32(x7[27], x7[26]);
        x8[27] = _mm_add_epi32(x7[27], x7[26]);
        x8[28] = _mm_add_epi32(x7[28], x7[29]);
        x8[29] = _mm_sub_epi32(x7[28], x7[29]);
        x8[30] = _mm_sub_epi32(x7[31], x7[30]);
        x8[31] = _mm_add_epi32(x7[31], x7[30]);
        x8[32] = x7[32];
        btf_32_type0_sse4_1_new(cospi_m04, cospi_p60, x7[33], x7[62], x8[33], x8[62], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m60, cospi_m04, x7[34], x7[61], x8[34], x8[61], __rounding, cos_bit);
        x8[35] = x7[35];
        x8[36] = x7[36];
        btf_32_type0_sse4_1_new(cospi_m36, cospi_p28, x7[37], x7[58], x8[37], x8[58], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m28, cospi_m36, x7[38], x7[57], x8[38], x8[57], __rounding, cos_bit);
        x8[39] = x7[39];
        x8[40] = x7[40];
        btf_32_type0_sse4_1_new(cospi_m20, cospi_p44, x7[41], x7[54], x8[41], x8[54], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m44, cospi_m20, x7[42], x7[53], x8[42], x8[53], __rounding, cos_bit);
        x8[43] = x7[43];
        x8[44] = x7[44];
        btf_32_type0_sse4_1_new(cospi_m52, cospi_p12, x7[45], x7[50], x8[45], x8[50], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m12, cospi_m52, x7[46], x7[49], x8[46], x8[49], __rounding, cos_bit);
        x8[47] = x7[47];
        x8[48] = x7[48];
        x8[51] = x7[51];
        x8[52] = x7[52];
        x8[55] = x7[55];
        x8[56] = x7[56];
        x8[59] = x7[59];
        x8[60] = x7[60];
        x8[63] = x7[63];

        // stage 9
        __m128i x9[64];
        x9[0]  = x8[0];
        x9[2]  = x8[2];
        x9[4]  = x8[4];
        x9[6]  = x8[6];
        x9[8]  = x8[8];
        x9[10] = x8[10];
        x9[12] = x8[12];
        x9[14] = x8[14];
        x9[16] = half_btf_sse4_1(&cospi_p62, &x8[16], &cospi_p02, &x8[31], &__rounding, cos_bit);
        x9[30] = half_btf_sse4_1(&cospi_p30, &x8[30], &cospi_m34, &x8[17], &__rounding, cos_bit);
        x9[18] = half_btf_sse4_1(&cospi_p46, &x8[18], &cospi_p18, &x8[29], &__rounding, cos_bit);
        x9[28] = half_btf_sse4_1(&cospi_p14, &x8[28], &cospi_m50, &x8[19], &__rounding, cos_bit);
        x9[20] = half_btf_sse4_1(&cospi_p54, &x8[20], &cospi_p10, &x8[27], &__rounding, cos_bit);
        x9[26] = half_btf_sse4_1(&cospi_p22, &x8[26], &cospi_m42, &x8[21], &__rounding, cos_bit);
        x9[22] = half_btf_sse4_1(&cospi_p38, &x8[22], &cospi_p26, &x8[25], &__rounding, cos_bit);
        x9[24] = half_btf_sse4_1(&cospi_p06, &x8[24], &cospi_m58, &x8[23], &__rounding, cos_bit);
        x9[32] = _mm_add_epi32(x8[32], x8[33]);
        x9[33] = _mm_sub_epi32(x8[32], x8[33]);
        x9[34] = _mm_sub_epi32(x8[35], x8[34]);
        x9[35] = _mm_add_epi32(x8[35], x8[34]);
        x9[36] = _mm_add_epi32(x8[36], x8[37]);
        x9[37] = _mm_sub_epi32(x8[36], x8[37]);
        x9[38] = _mm_sub_epi32(x8[39], x8[38]);
        x9[39] = _mm_add_epi32(x8[39], x8[38]);
        x9[40] = _mm_add_epi32(x8[40], x8[41]);
        x9[41] = _mm_sub_epi32(x8[40], x8[41]);
        x9[42] = _mm_sub_epi32(x8[43], x8[42]);
        x9[43] = _mm_add_epi32(x8[43], x8[42]);
        x9[44] = _mm_add_epi32(x8[44], x8[45]);
        x9[45] = _mm_sub_epi32(x8[44], x8[45]);
        x9[46] = _mm_sub_epi32(x8[47], x8[46]);
        x9[47] = _mm_add_epi32(x8[47], x8[46]);
        x9[48] = _mm_add_epi32(x8[48], x8[49]);
        x9[49] = _mm_sub_epi32(x8[48], x8[49]);
        x9[50] = _mm_sub_epi32(x8[51], x8[50]);
        x9[51] = _mm_add_epi32(x8[51], x8[50]);
        x9[52] = _mm_add_epi32(x8[52], x8[53]);
        x9[53] = _mm_sub_epi32(x8[52], x8[53]);
        x9[54] = _mm_sub_epi32(x8[55], x8[54]);
        x9[55] = _mm_add_epi32(x8[55], x8[54]);
        x9[56] = _mm_add_epi32(x8[56], x8[57]);
        x9[57] = _mm_sub_epi32(x8[56], x8[57]);
        x9[58] = _mm_sub_epi32(x8[59], x8[58]);
        x9[59] = _mm_add_epi32(x8[59], x8[58]);
        x9[60] = _mm_add_epi32(x8[60], x8[61]);
        x9[61] = _mm_sub_epi32(x8[60], x8[61]);
        x9[62] = _mm_sub_epi32(x8[63], x8[62]);
        x9[63] = _mm_add_epi32(x8[63], x8[62]);

        // stage 10
        __m128i x10[64];
        out[2 * 0 * stride]  = x9[0];
        out[2 * 16 * stride] = x9[2];
        out[2 * 8 * stride]  = x9[4];
        out[2 * 24 * stride] = x9[6];
        out[2 * 4 * stride]  = x9[8];
        out[2 * 20 * stride] = x9[10];
        out[2 * 12 * stride] = x9[12];
        out[2 * 28 * stride] = x9[14];
        out[2 * 2 * stride]  = x9[16];
        out[2 * 18 * stride] = x9[18];
        out[2 * 10 * stride] = x9[20];
        out[2 * 26 * stride] = x9[22];
        out[2 * 6 * stride]  = x9[24];
        out[2 * 22 * stride] = x9[26];
        out[2 * 14 * stride] = x9[28];
        out[2 * 30 * stride] = x9[30];
        x10[32]              = half_btf_sse4_1(&cospi_p63, &x9[32], &cospi_p01, &x9[63], &__rounding, cos_bit);
        x10[62]              = half_btf_sse4_1(&cospi_p31, &x9[62], &cospi_m33, &x9[33], &__rounding, cos_bit);
        x10[34]              = half_btf_sse4_1(&cospi_p47, &x9[34], &cospi_p17, &x9[61], &__rounding, cos_bit);
        x10[60]              = half_btf_sse4_1(&cospi_p15, &x9[60], &cospi_m49, &x9[35], &__rounding, cos_bit);
        x10[36]              = half_btf_sse4_1(&cospi_p55, &x9[36], &cospi_p09, &x9[59], &__rounding, cos_bit);
        x10[58]              = half_btf_sse4_1(&cospi_p23, &x9[58], &cospi_m41, &x9[37], &__rounding, cos_bit);
        x10[38]              = half_btf_sse4_1(&cospi_p39, &x9[38], &cospi_p25, &x9[57], &__rounding, cos_bit);
        x10[56]              = half_btf_sse4_1(&cospi_p07, &x9[56], &cospi_m57, &x9[39], &__rounding, cos_bit);
        x10[40]              = half_btf_sse4_1(&cospi_p59, &x9[40], &cospi_p05, &x9[55], &__rounding, cos_bit);
        x10[54]              = half_btf_sse4_1(&cospi_p27, &x9[54], &cospi_m37, &x9[41], &__rounding, cos_bit);
        x10[42]              = half_btf_sse4_1(&cospi_p43, &x9[42], &cospi_p21, &x9[53], &__rounding, cos_bit);
        x10[52]              = half_btf_sse4_1(&cospi_p11, &x9[52], &cospi_m53, &x9[43], &__rounding, cos_bit);
        x10[44]              = half_btf_sse4_1(&cospi_p51, &x9[44], &cospi_p13, &x9[51], &__rounding, cos_bit);
        x10[50]              = half_btf_sse4_1(&cospi_p19, &x9[50], &cospi_m45, &x9[45], &__rounding, cos_bit);
        x10[46]              = half_btf_sse4_1(&cospi_p35, &x9[46], &cospi_p29, &x9[49], &__rounding, cos_bit);
        x10[48]              = half_btf_sse4_1(&cospi_p03, &x9[48], &cospi_m61, &x9[47], &__rounding, cos_bit);

        // stage 11
        out[2 * 1 * stride]  = x10[32];
        out[2 * 3 * stride]  = x10[48];
        out[2 * 5 * stride]  = x10[40];
        out[2 * 7 * stride]  = x10[56];
        out[2 * 9 * stride]  = x10[36];
        out[2 * 11 * stride] = x10[52];
        out[2 * 13 * stride] = x10[44];
        out[2 * 15 * stride] = x10[60];
        out[2 * 17 * stride] = x10[34];
        out[2 * 19 * stride] = x10[50];
        out[2 * 21 * stride] = x10[42];
        out[2 * 23 * stride] = x10[58];
        out[2 * 25 * stride] = x10[38];
        out[2 * 27 * stride] = x10[54];
        out[2 * 29 * stride] = x10[46];
        out[2 * 31 * stride] = x10[62];
    }
}

static void fidtx64x64_N2_sse4_1(const __m128i* input, __m128i* output) {
    const int32_t bits     = 12; // new_sqrt2_bits = 12
    const int32_t sqrt     = 4 * 5793; // 4 * new_sqrt2
    const __m128i newsqrt  = _mm_set1_epi32(sqrt);
    const __m128i rounding = _mm_set1_epi32(1 << (bits - 1));

    __m128i temp0, temp1, temp2, temp3, temp4, temp5, temp6, temp7;
    for (int32_t i = 0; i < 512; i += 8) {
        temp0             = _mm_mullo_epi32(input[2 * i], newsqrt);
        temp0             = _mm_add_epi32(temp0, rounding);
        output[2 * i]     = _mm_srai_epi32(temp0, bits);
        temp1             = _mm_mullo_epi32(input[2 * i + 1], newsqrt);
        temp1             = _mm_add_epi32(temp1, rounding);
        output[2 * i + 1] = _mm_srai_epi32(temp1, bits);
        temp2             = _mm_mullo_epi32(input[2 * i + 2], newsqrt);
        temp2             = _mm_add_epi32(temp2, rounding);
        output[2 * i + 2] = _mm_srai_epi32(temp2, bits);
        temp3             = _mm_mullo_epi32(input[2 * i + 3], newsqrt);
        temp3             = _mm_add_epi32(temp3, rounding);
        output[2 * i + 3] = _mm_srai_epi32(temp3, bits);
        temp4             = _mm_mullo_epi32(input[2 * i + 4], newsqrt);
        temp4             = _mm_add_epi32(temp4, rounding);
        output[2 * i + 4] = _mm_srai_epi32(temp4, bits);
        temp5             = _mm_mullo_epi32(input[2 * i + 5], newsqrt);
        temp5             = _mm_add_epi32(temp5, rounding);
        output[2 * i + 5] = _mm_srai_epi32(temp5, bits);
        temp6             = _mm_mullo_epi32(input[2 * i + 6], newsqrt);
        temp6             = _mm_add_epi32(temp6, rounding);
        output[2 * i + 6] = _mm_srai_epi32(temp6, bits);
        temp7             = _mm_mullo_epi32(input[2 * i + 7], newsqrt);
        temp7             = _mm_add_epi32(temp7, rounding);
        output[2 * i + 7] = _mm_srai_epi32(temp7, bits);
    }
}

static INLINE void av1_round_shift_array_64_N2_sse4_1(__m128i* input, __m128i* output, const int32_t size,
                                                      const int32_t bit) {
    int32_t i;
    if (bit > 0) {
        const __m128i round = _mm_set1_epi32(1 << (bit - 1));
        __m128i       r0, r1, r2, r3, r4, r5, r6, r7;
        for (i = 0; i < size; i += 8) {
            r0                = _mm_add_epi32(input[2 * i], round);
            output[2 * i]     = _mm_srai_epi32(r0, bit);
            r1                = _mm_add_epi32(input[2 * i + 1], round);
            output[2 * i + 1] = _mm_srai_epi32(r1, bit);
            r2                = _mm_add_epi32(input[2 * i + 2], round);
            output[2 * i + 2] = _mm_srai_epi32(r2, bit);
            r3                = _mm_add_epi32(input[2 * i + 3], round);
            output[2 * i + 3] = _mm_srai_epi32(r3, bit);
            r4                = _mm_add_epi32(input[2 * i + 4], round);
            output[2 * i + 4] = _mm_srai_epi32(r4, bit);
            r5                = _mm_add_epi32(input[2 * i + 5], round);
            output[2 * i + 5] = _mm_srai_epi32(r5, bit);
            r6                = _mm_add_epi32(input[2 * i + 6], round);
            output[2 * i + 6] = _mm_srai_epi32(r6, bit);
            r7                = _mm_add_epi32(input[2 * i + 7], round);
            output[2 * i + 7] = _mm_srai_epi32(r7, bit);
        }
    } else {
        for (i = 0; i < size; i += 8) {
            output[2 * i]     = _mm_slli_epi32(input[2 * i], -bit);
            output[2 * i + 1] = _mm_slli_epi32(input[2 * i + 1], -bit);
            output[2 * i + 2] = _mm_slli_epi32(input[2 * i + 2], -bit);
            output[2 * i + 3] = _mm_slli_epi32(input[2 * i + 3], -bit);
            output[2 * i + 4] = _mm_slli_epi32(input[2 * i + 4], -bit);
            output[2 * i + 5] = _mm_slli_epi32(input[2 * i + 5], -bit);
            output[2 * i + 6] = _mm_slli_epi32(input[2 * i + 6], -bit);
            output[2 * i + 7] = _mm_slli_epi32(input[2 * i + 7], -bit);
        }
    }
}

static void av1_idtx16_new_sse4_1(const __m128i* input, __m128i* output, int8_t cos_bit, const int32_t col_num) {
    (void)cos_bit;
    for (int32_t i = 0; i < 16; i++) {
        output[2 * i * col_num]     = _mm_slli_epi32(input[2 * i * col_num], 2);
        output[2 * i * col_num + 1] = _mm_slli_epi32(input[2 * i * col_num], 2);
    }
}

static void fidtx32x8_N2_sse4_1(const __m128i* input, __m128i* output, int8_t cos_bit, const int32_t col_num,
                                int32_t row_num) {
    (void)cos_bit;
    for (int32_t i = 0; i < row_num; i++) {
        output[2 * i * col_num]     = _mm_slli_epi32(input[2 * i * col_num], 1);
        output[2 * i * col_num + 1] = _mm_slli_epi32(input[2 * i * col_num + 1], 1);
    }
}

static void fidtx32x16_N2_sse4_1(const __m128i* in, __m128i* out, int8_t bit, int32_t rows) {
    (void)bit;
    const int32_t bits     = 12; // new_sqrt2_bits = 12
    const int32_t sqrt     = 2 * 5793; // 2 * new_sqrt2
    const __m128i newsqrt  = _mm_set1_epi32(sqrt);
    const __m128i rounding = _mm_set1_epi32(1 << (bits - 1));
    __m128i       r0, r1, r2, r3;

    for (int32_t i = 0; i < rows; i++) {
        r0             = _mm_mullo_epi32(in[i * 8], newsqrt);
        r0             = _mm_add_epi32(r0, rounding);
        out[i * 8]     = _mm_srai_epi32(r0, bits);
        r1             = _mm_mullo_epi32(in[i * 8 + 1], newsqrt);
        r1             = _mm_add_epi32(r1, rounding);
        out[i * 8 + 1] = _mm_srai_epi32(r1, bits);
        r2             = _mm_mullo_epi32(in[i * 8 + 2], newsqrt);
        r2             = _mm_add_epi32(r2, rounding);
        out[i * 8 + 2] = _mm_srai_epi32(r2, bits);
        r3             = _mm_mullo_epi32(in[i * 8 + 3], newsqrt);
        r3             = _mm_add_epi32(r3, rounding);
        out[i * 8 + 3] = _mm_srai_epi32(r3, bits);
    }
}

static void av1_idtx32x16_N2_sse4_1(const __m128i* input, __m128i* output, const int32_t rows) {
    for (int32_t i = 0; i < rows; i++) {
        output[8 * i]     = _mm_slli_epi32(input[8 * i], 2);
        output[8 * i + 1] = _mm_slli_epi32(input[8 * i + 1], 2);
        output[8 * i + 2] = _mm_slli_epi32(input[8 * i + 2], 2);
        output[8 * i + 3] = _mm_slli_epi32(input[8 * i + 3], 2);
    }
}

static AOM_FORCE_INLINE void col_txfm_32x16_N2_rounding(__m128i* in, int32_t shift) {
    const __m128i rounding = _mm_set1_epi32(1 << (shift - 1));
    in[0]                  = _mm_add_epi32(in[0], rounding);
    in[1]                  = _mm_add_epi32(in[1], rounding);
    in[8]                  = _mm_add_epi32(in[8], rounding);
    in[9]                  = _mm_add_epi32(in[9], rounding);
    in[16]                 = _mm_add_epi32(in[16], rounding);
    in[17]                 = _mm_add_epi32(in[17], rounding);
    in[24]                 = _mm_add_epi32(in[24], rounding);
    in[25]                 = _mm_add_epi32(in[25], rounding);
    in[0]                  = _mm_srai_epi32(in[0], shift);
    in[1]                  = _mm_srai_epi32(in[1], shift);
    in[8]                  = _mm_srai_epi32(in[8], shift);
    in[9]                  = _mm_srai_epi32(in[9], shift);
    in[16]                 = _mm_srai_epi32(in[16], shift);
    in[17]                 = _mm_srai_epi32(in[17], shift);
    in[24]                 = _mm_srai_epi32(in[24], shift);
    in[25]                 = _mm_srai_epi32(in[25], shift);
    in[32]                 = _mm_add_epi32(in[32], rounding);
    in[33]                 = _mm_add_epi32(in[33], rounding);
    in[40]                 = _mm_add_epi32(in[40], rounding);
    in[41]                 = _mm_add_epi32(in[41], rounding);
    in[48]                 = _mm_add_epi32(in[48], rounding);
    in[49]                 = _mm_add_epi32(in[49], rounding);
    in[56]                 = _mm_add_epi32(in[56], rounding);
    in[57]                 = _mm_add_epi32(in[57], rounding);
    in[32]                 = _mm_srai_epi32(in[32], shift);
    in[33]                 = _mm_srai_epi32(in[33], shift);
    in[40]                 = _mm_srai_epi32(in[40], shift);
    in[41]                 = _mm_srai_epi32(in[41], shift);
    in[48]                 = _mm_srai_epi32(in[48], shift);
    in[49]                 = _mm_srai_epi32(in[49], shift);
    in[56]                 = _mm_srai_epi32(in[56], shift);
    in[57]                 = _mm_srai_epi32(in[57], shift);
}

static void fdct16x4_N2_sse4_1(__m128i* input, __m128i* output, int32_t bit) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospim32 = _mm_set1_epi32(-cospi[32]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospi16  = _mm_set1_epi32(cospi[16]);
    const __m128i  cospim48 = _mm_set1_epi32(-cospi[48]);
    const __m128i  cospim16 = _mm_set1_epi32(-cospi[16]);
    const __m128i  cospi56  = _mm_set1_epi32(cospi[56]);
    const __m128i  cospi8   = _mm_set1_epi32(cospi[8]);
    const __m128i  cospi24  = _mm_set1_epi32(cospi[24]);
    const __m128i  cospi40  = _mm_set1_epi32(cospi[40]);
    const __m128i  cospi60  = _mm_set1_epi32(cospi[60]);
    const __m128i  cospi4   = _mm_set1_epi32(cospi[4]);
    const __m128i  cospi28  = _mm_set1_epi32(cospi[28]);
    const __m128i  cospi36  = _mm_set1_epi32(cospi[36]);
    const __m128i  cospi44  = _mm_set1_epi32(cospi[44]);
    const __m128i  cospi20  = _mm_set1_epi32(cospi[20]);
    const __m128i  cospi12  = _mm_set1_epi32(cospi[12]);
    const __m128i  cospi52  = _mm_set1_epi32(cospi[52]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    __m128i        u[16], v[16], x;

    // stage 0
    // stage 1
    u[0]  = _mm_add_epi32(input[0], input[15]);
    v[15] = _mm_sub_epi32(input[0], input[15]);
    u[1]  = _mm_add_epi32(input[1], input[14]);
    v[14] = _mm_sub_epi32(input[1], input[14]);
    u[2]  = _mm_add_epi32(input[2], input[13]);
    u[13] = _mm_sub_epi32(input[2], input[13]);
    u[3]  = _mm_add_epi32(input[3], input[12]);
    u[12] = _mm_sub_epi32(input[3], input[12]);
    u[4]  = _mm_add_epi32(input[4], input[11]);
    u[11] = _mm_sub_epi32(input[4], input[11]);
    u[5]  = _mm_add_epi32(input[5], input[10]);
    u[10] = _mm_sub_epi32(input[5], input[10]);
    u[6]  = _mm_add_epi32(input[6], input[9]);
    v[9]  = _mm_sub_epi32(input[6], input[9]);
    u[7]  = _mm_add_epi32(input[7], input[8]);
    v[8]  = _mm_sub_epi32(input[7], input[8]);

    // stage 2
    v[0] = _mm_add_epi32(u[0], u[7]);
    u[7] = _mm_sub_epi32(u[0], u[7]);
    v[1] = _mm_add_epi32(u[1], u[6]);
    v[6] = _mm_sub_epi32(u[1], u[6]);
    v[2] = _mm_add_epi32(u[2], u[5]);
    v[5] = _mm_sub_epi32(u[2], u[5]);
    v[3] = _mm_add_epi32(u[3], u[4]);
    u[4] = _mm_sub_epi32(u[3], u[4]);

    v[10] = _mm_mullo_epi32(u[10], cospim32);
    x     = _mm_mullo_epi32(u[13], cospi32);
    v[10] = _mm_add_epi32(v[10], x);
    v[10] = _mm_add_epi32(v[10], rnding);
    v[10] = _mm_srai_epi32(v[10], bit);

    v[13] = _mm_mullo_epi32(u[10], cospi32);
    x     = _mm_mullo_epi32(u[13], cospim32);
    v[13] = _mm_sub_epi32(v[13], x);
    v[13] = _mm_add_epi32(v[13], rnding);
    v[13] = _mm_srai_epi32(v[13], bit);

    v[11] = _mm_mullo_epi32(u[11], cospim32);
    x     = _mm_mullo_epi32(u[12], cospi32);
    v[11] = _mm_add_epi32(v[11], x);
    v[11] = _mm_add_epi32(v[11], rnding);
    v[11] = _mm_srai_epi32(v[11], bit);

    v[12] = _mm_mullo_epi32(u[11], cospi32);
    x     = _mm_mullo_epi32(u[12], cospim32);
    v[12] = _mm_sub_epi32(v[12], x);
    v[12] = _mm_add_epi32(v[12], rnding);
    v[12] = _mm_srai_epi32(v[12], bit);

    // stage 3
    u[0] = _mm_add_epi32(v[0], v[3]);
    u[3] = _mm_sub_epi32(v[0], v[3]);
    u[1] = _mm_add_epi32(v[1], v[2]);
    u[2] = _mm_sub_epi32(v[1], v[2]);

    u[5] = _mm_mullo_epi32(v[5], cospim32);
    x    = _mm_mullo_epi32(v[6], cospi32);
    u[5] = _mm_add_epi32(u[5], x);
    u[5] = _mm_add_epi32(u[5], rnding);
    u[5] = _mm_srai_epi32(u[5], bit);

    u[6] = _mm_mullo_epi32(v[5], cospi32);
    x    = _mm_mullo_epi32(v[6], cospim32);
    u[6] = _mm_sub_epi32(u[6], x);
    u[6] = _mm_add_epi32(u[6], rnding);
    u[6] = _mm_srai_epi32(u[6], bit);

    u[8]  = _mm_add_epi32(v[8], v[11]);
    v[11] = _mm_sub_epi32(v[8], v[11]);
    u[9]  = _mm_add_epi32(v[9], v[10]);
    u[10] = _mm_sub_epi32(v[9], v[10]);
    u[12] = _mm_sub_epi32(v[15], v[12]);
    v[15] = _mm_add_epi32(v[15], v[12]);
    u[13] = _mm_sub_epi32(v[14], v[13]);
    u[14] = _mm_add_epi32(v[14], v[13]);

    // stage 4
    u[0]      = _mm_mullo_epi32(u[0], cospi32);
    u[1]      = _mm_mullo_epi32(u[1], cospi32);
    v[0]      = _mm_add_epi32(u[0], u[1]);
    v[0]      = _mm_add_epi32(v[0], rnding);
    output[0] = _mm_srai_epi32(v[0], bit);

    v[2]      = _mm_mullo_epi32(u[2], cospi48);
    x         = _mm_mullo_epi32(u[3], cospi16);
    v[2]      = _mm_add_epi32(v[2], x);
    v[2]      = _mm_add_epi32(v[2], rnding);
    output[4] = _mm_srai_epi32(v[2], bit);

    v[4] = _mm_add_epi32(u[4], u[5]);
    v[5] = _mm_sub_epi32(u[4], u[5]);
    v[6] = _mm_sub_epi32(u[7], u[6]);
    v[7] = _mm_add_epi32(u[7], u[6]);
    v[8] = u[8];

    v[9] = _mm_mullo_epi32(u[9], cospim16);
    x    = _mm_mullo_epi32(u[14], cospi48);
    v[9] = _mm_add_epi32(v[9], x);
    v[9] = _mm_add_epi32(v[9], rnding);
    v[9] = _mm_srai_epi32(v[9], bit);

    v[14] = _mm_mullo_epi32(u[9], cospi48);
    x     = _mm_mullo_epi32(u[14], cospim16);
    v[14] = _mm_sub_epi32(v[14], x);
    v[14] = _mm_add_epi32(v[14], rnding);
    v[14] = _mm_srai_epi32(v[14], bit);

    v[10] = _mm_mullo_epi32(u[10], cospim48);
    x     = _mm_mullo_epi32(u[13], cospim16);
    v[10] = _mm_add_epi32(v[10], x);
    v[10] = _mm_add_epi32(v[10], rnding);
    v[10] = _mm_srai_epi32(v[10], bit);

    v[13] = _mm_mullo_epi32(u[10], cospim16);
    x     = _mm_mullo_epi32(u[13], cospim48);
    v[13] = _mm_sub_epi32(v[13], x);
    v[13] = _mm_add_epi32(v[13], rnding);
    v[13] = _mm_srai_epi32(v[13], bit);

    v[12] = u[12];

    // stage 5
    u[4]      = _mm_mullo_epi32(v[4], cospi56);
    x         = _mm_mullo_epi32(v[7], cospi8);
    u[4]      = _mm_add_epi32(u[4], x);
    u[4]      = _mm_add_epi32(u[4], rnding);
    output[2] = _mm_srai_epi32(u[4], bit);

    u[6]      = _mm_mullo_epi32(v[5], cospi40);
    x         = _mm_mullo_epi32(v[6], cospi24);
    u[6]      = _mm_sub_epi32(x, u[6]);
    u[6]      = _mm_add_epi32(u[6], rnding);
    output[6] = _mm_srai_epi32(u[6], bit);

    u[8]  = _mm_add_epi32(v[8], v[9]);
    u[9]  = _mm_sub_epi32(v[8], v[9]);
    u[10] = _mm_sub_epi32(v[11], v[10]);
    u[11] = _mm_add_epi32(v[11], v[10]);
    u[12] = _mm_add_epi32(v[12], v[13]);
    u[13] = _mm_sub_epi32(v[12], v[13]);
    u[14] = _mm_sub_epi32(v[15], v[14]);
    u[15] = _mm_add_epi32(v[15], v[14]);

    // stage 6
    v[8]      = _mm_mullo_epi32(u[8], cospi60);
    x         = _mm_mullo_epi32(u[15], cospi4);
    v[8]      = _mm_add_epi32(v[8], x);
    v[8]      = _mm_add_epi32(v[8], rnding);
    output[1] = _mm_srai_epi32(v[8], bit);

    v[14]     = _mm_mullo_epi32(u[9], cospi36);
    x         = _mm_mullo_epi32(u[14], cospi28);
    v[14]     = _mm_sub_epi32(x, v[14]);
    v[14]     = _mm_add_epi32(v[14], rnding);
    output[7] = _mm_srai_epi32(v[14], bit);

    v[10]     = _mm_mullo_epi32(u[10], cospi44);
    x         = _mm_mullo_epi32(u[13], cospi20);
    v[10]     = _mm_add_epi32(v[10], x);
    v[10]     = _mm_add_epi32(v[10], rnding);
    output[5] = _mm_srai_epi32(v[10], bit);

    v[12]     = _mm_mullo_epi32(u[11], cospi52);
    x         = _mm_mullo_epi32(u[12], cospi12);
    v[12]     = _mm_sub_epi32(x, v[12]);
    v[12]     = _mm_add_epi32(v[12], rnding);
    output[3] = _mm_srai_epi32(v[12], bit);
}

static void fadst16x4_N2_sse4_1(__m128i* input, __m128i* output, int32_t bit) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospi16  = _mm_set1_epi32(cospi[16]);
    const __m128i  cospim16 = _mm_set1_epi32(-cospi[16]);
    const __m128i  cospim48 = _mm_set1_epi32(-cospi[48]);
    const __m128i  cospi8   = _mm_set1_epi32(cospi[8]);
    const __m128i  cospi56  = _mm_set1_epi32(cospi[56]);
    const __m128i  cospim56 = _mm_set1_epi32(-cospi[56]);
    const __m128i  cospim8  = _mm_set1_epi32(-cospi[8]);
    const __m128i  cospi24  = _mm_set1_epi32(cospi[24]);
    const __m128i  cospim24 = _mm_set1_epi32(-cospi[24]);
    const __m128i  cospim40 = _mm_set1_epi32(-cospi[40]);
    const __m128i  cospi40  = _mm_set1_epi32(cospi[40]);
    const __m128i  cospi62  = _mm_set1_epi32(cospi[62]);
    const __m128i  cospim2  = _mm_set1_epi32(-cospi[2]);
    const __m128i  cospi54  = _mm_set1_epi32(cospi[54]);
    const __m128i  cospim10 = _mm_set1_epi32(-cospi[10]);
    const __m128i  cospi46  = _mm_set1_epi32(cospi[46]);
    const __m128i  cospim18 = _mm_set1_epi32(-cospi[18]);
    const __m128i  cospi38  = _mm_set1_epi32(cospi[38]);
    const __m128i  cospim26 = _mm_set1_epi32(-cospi[26]);
    const __m128i  cospi34  = _mm_set1_epi32(cospi[34]);
    const __m128i  cospi30  = _mm_set1_epi32(cospi[30]);
    const __m128i  cospi42  = _mm_set1_epi32(cospi[42]);
    const __m128i  cospi22  = _mm_set1_epi32(cospi[22]);
    const __m128i  cospi50  = _mm_set1_epi32(cospi[50]);
    const __m128i  cospi14  = _mm_set1_epi32(cospi[14]);
    const __m128i  cospi58  = _mm_set1_epi32(cospi[58]);
    const __m128i  cospi6   = _mm_set1_epi32(cospi[6]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    const __m128i  zero     = _mm_setzero_si128();

    __m128i u[16], v[16], x, y;
    __m128i tmp[13];

    tmp[0] = _mm_sub_epi32(zero, input[15]);
    u[2]   = _mm_sub_epi32(zero, input[7]);
    tmp[1] = _mm_sub_epi32(zero, input[3]);
    u[7]   = _mm_sub_epi32(zero, input[11]);
    tmp[2] = _mm_sub_epi32(zero, input[1]);
    u[11]  = _mm_sub_epi32(zero, input[9]);
    tmp[3] = _mm_sub_epi32(zero, input[13]);
    u[14]  = _mm_sub_epi32(zero, input[5]);

    // stage 2

    x    = _mm_mullo_epi32(u[2], cospi32);
    y    = _mm_mullo_epi32(input[8], cospi32);
    v[2] = _mm_add_epi32(x, y);
    v[2] = _mm_add_epi32(v[2], rnding);
    v[2] = _mm_srai_epi32(v[2], bit);

    v[3] = _mm_sub_epi32(x, y);
    v[3] = _mm_add_epi32(v[3], rnding);
    v[3] = _mm_srai_epi32(v[3], bit);

    x    = _mm_mullo_epi32(input[4], cospi32);
    y    = _mm_mullo_epi32(u[7], cospi32);
    v[6] = _mm_add_epi32(x, y);
    v[6] = _mm_add_epi32(v[6], rnding);
    v[6] = _mm_srai_epi32(v[6], bit);

    v[7] = _mm_sub_epi32(x, y);
    v[7] = _mm_add_epi32(v[7], rnding);
    v[7] = _mm_srai_epi32(v[7], bit);

    x     = _mm_mullo_epi32(input[6], cospi32);
    y     = _mm_mullo_epi32(u[11], cospi32);
    v[10] = _mm_add_epi32(x, y);
    v[10] = _mm_add_epi32(v[10], rnding);
    v[10] = _mm_srai_epi32(v[10], bit);

    v[11] = _mm_sub_epi32(x, y);
    v[11] = _mm_add_epi32(v[11], rnding);
    v[11] = _mm_srai_epi32(v[11], bit);

    x     = _mm_mullo_epi32(u[14], cospi32);
    y     = _mm_mullo_epi32(input[10], cospi32);
    v[14] = _mm_add_epi32(x, y);
    v[14] = _mm_add_epi32(v[14], rnding);
    v[14] = _mm_srai_epi32(v[14], bit);

    v[15] = _mm_sub_epi32(x, y);
    v[15] = _mm_add_epi32(v[15], rnding);
    v[15] = _mm_srai_epi32(v[15], bit);

    // stage 3
    tmp[4] = _mm_add_epi32(input[0], v[2]);
    tmp[5] = _mm_add_epi32(tmp[0], v[3]);
    tmp[6] = _mm_sub_epi32(input[0], v[2]);
    tmp[0] = _mm_sub_epi32(tmp[0], v[3]);
    u[4]   = _mm_add_epi32(tmp[1], v[6]);
    u[5]   = _mm_add_epi32(input[12], v[7]);
    u[6]   = _mm_sub_epi32(tmp[1], v[6]);
    u[7]   = _mm_sub_epi32(input[12], v[7]);
    tmp[1] = _mm_add_epi32(tmp[2], v[10]);
    tmp[7] = _mm_add_epi32(input[14], v[11]);
    tmp[2] = _mm_sub_epi32(tmp[2], v[10]);
    tmp[8] = _mm_sub_epi32(input[14], v[11]);
    u[12]  = _mm_add_epi32(input[2], v[14]);
    u[13]  = _mm_add_epi32(tmp[3], v[15]);
    u[14]  = _mm_sub_epi32(input[2], v[14]);
    u[15]  = _mm_sub_epi32(tmp[3], v[15]);

    // stage 4
    v[4]  = half_btf_small(&cospi16, &u[4], &cospi48, &u[5], &rnding, bit);
    v[5]  = half_btf_small(&cospi48, &u[4], &cospim16, &u[5], &rnding, bit);
    v[6]  = half_btf_small(&cospim48, &u[6], &cospi16, &u[7], &rnding, bit);
    v[7]  = half_btf_small(&cospi16, &u[6], &cospi48, &u[7], &rnding, bit);
    v[12] = half_btf_small(&cospi16, &u[12], &cospi48, &u[13], &rnding, bit);
    v[13] = half_btf_small(&cospi48, &u[12], &cospim16, &u[13], &rnding, bit);
    v[14] = half_btf_small(&cospim48, &u[14], &cospi16, &u[15], &rnding, bit);
    v[15] = half_btf_small(&cospi16, &u[14], &cospi48, &u[15], &rnding, bit);

    // stage 5
    tmp[9]  = _mm_add_epi32(tmp[4], v[4]);
    tmp[10] = _mm_add_epi32(tmp[5], v[5]);
    tmp[11] = _mm_add_epi32(tmp[6], v[6]);
    tmp[12] = _mm_add_epi32(tmp[0], v[7]);
    tmp[4]  = _mm_sub_epi32(tmp[4], v[4]);
    tmp[5]  = _mm_sub_epi32(tmp[5], v[5]);
    tmp[6]  = _mm_sub_epi32(tmp[6], v[6]);
    tmp[0]  = _mm_sub_epi32(tmp[0], v[7]);
    u[8]    = _mm_add_epi32(tmp[1], v[12]);
    u[9]    = _mm_add_epi32(tmp[7], v[13]);
    u[10]   = _mm_add_epi32(tmp[2], v[14]);
    u[11]   = _mm_add_epi32(tmp[8], v[15]);
    u[12]   = _mm_sub_epi32(tmp[1], v[12]);
    u[13]   = _mm_sub_epi32(tmp[7], v[13]);
    u[14]   = _mm_sub_epi32(tmp[2], v[14]);
    u[15]   = _mm_sub_epi32(tmp[8], v[15]);

    // stage 6
    v[8]  = half_btf_small(&cospi8, &u[8], &cospi56, &u[9], &rnding, bit);
    v[9]  = half_btf_small(&cospi56, &u[8], &cospim8, &u[9], &rnding, bit);
    v[10] = half_btf_small(&cospi40, &u[10], &cospi24, &u[11], &rnding, bit);
    v[11] = half_btf_small(&cospi24, &u[10], &cospim40, &u[11], &rnding, bit);
    v[12] = half_btf_small(&cospim56, &u[12], &cospi8, &u[13], &rnding, bit);
    v[13] = half_btf_small(&cospi8, &u[12], &cospi56, &u[13], &rnding, bit);
    v[14] = half_btf_small(&cospim24, &u[14], &cospi40, &u[15], &rnding, bit);
    v[15] = half_btf_small(&cospi40, &u[14], &cospi24, &u[15], &rnding, bit);

    // stage 7
    u[0]  = _mm_add_epi32(tmp[9], v[8]);
    u[1]  = _mm_add_epi32(tmp[10], v[9]);
    u[2]  = _mm_add_epi32(tmp[11], v[10]);
    u[3]  = _mm_add_epi32(tmp[12], v[11]);
    u[4]  = _mm_add_epi32(tmp[4], v[12]);
    u[5]  = _mm_add_epi32(tmp[5], v[13]);
    u[6]  = _mm_add_epi32(tmp[6], v[14]);
    u[7]  = _mm_add_epi32(tmp[0], v[15]);
    u[8]  = _mm_sub_epi32(tmp[9], v[8]);
    u[9]  = _mm_sub_epi32(tmp[10], v[9]);
    u[10] = _mm_sub_epi32(tmp[11], v[10]);
    u[11] = _mm_sub_epi32(tmp[12], v[11]);
    u[12] = _mm_sub_epi32(tmp[4], v[12]);
    u[13] = _mm_sub_epi32(tmp[5], v[13]);
    u[14] = _mm_sub_epi32(tmp[6], v[14]);
    u[15] = _mm_sub_epi32(tmp[0], v[15]);

    // stage 8
    output[0] = half_btf_small(&cospi62, &u[0], &cospim2, &u[1], &rnding, bit);
    output[2] = half_btf_small(&cospi54, &u[2], &cospim10, &u[3], &rnding, bit);
    output[4] = half_btf_small(&cospi46, &u[4], &cospim18, &u[5], &rnding, bit);
    output[6] = half_btf_small(&cospi38, &u[6], &cospim26, &u[7], &rnding, bit);
    output[7] = half_btf_small(&cospi34, &u[8], &cospi30, &u[9], &rnding, bit);
    output[5] = half_btf_small(&cospi42, &u[10], &cospi22, &u[11], &rnding, bit);
    output[3] = half_btf_small(&cospi50, &u[12], &cospi14, &u[13], &rnding, bit);
    output[1] = half_btf_small(&cospi58, &u[14], &cospi6, &u[15], &rnding, bit);
}

static void fdct4x8_col_N2_sse4_1(__m128i* in, __m128i* output, int32_t bit, const int32_t num_col) {
    const int32_t* cospi   = cospi_arr(bit);
    const __m128i  zero    = _mm_setzero_si128();
    const __m128i  cospi32 = _mm_set1_epi32(cospi[32]);
    const __m128i  cospi48 = _mm_set1_epi32(cospi[48]);
    const __m128i  cospi16 = _mm_set1_epi32(cospi[16]);
    const __m128i  rnding  = _mm_set1_epi32(1 << (bit - 1));
    __m128i        s0, s1, s2, s3, s4, s5, s6, s7;
    __m128i        u0, u1, u2, u3, u4, u5, u6, u7;
    __m128i        v0, v1, v2, v3, v4, v5, v6, v7;

    int32_t endidx = 3 * num_col;
    s0             = _mm_add_epi32(in[0], in[2 * endidx]);
    s1             = _mm_add_epi32(in[1], in[2 * endidx + 1]);
    s6             = _mm_sub_epi32(in[0], in[2 * endidx]);
    s7             = _mm_sub_epi32(in[1], in[2 * endidx + 1]);
    endidx -= num_col;
    s2 = _mm_add_epi32(in[2 * num_col], in[2 * endidx]);
    s3 = _mm_add_epi32(in[2 * num_col + 1], in[2 * endidx + 1]);
    s4 = _mm_sub_epi32(in[2 * num_col], in[2 * endidx]);
    s5 = _mm_sub_epi32(in[2 * num_col + 1], in[2 * endidx + 1]);

    u0 = _mm_mullo_epi32(s0, cospi32);
    u1 = _mm_mullo_epi32(s1, cospi32);
    u2 = _mm_mullo_epi32(s2, cospi32);
    u3 = _mm_mullo_epi32(s3, cospi32);
    u4 = _mm_add_epi32(u0, u2);
    u5 = _mm_add_epi32(u1, u3);

    u6 = _mm_add_epi32(u4, rnding);
    u7 = _mm_add_epi32(u5, rnding);

    u0 = _mm_srai_epi32(u6, bit);
    u1 = _mm_srai_epi32(u7, bit);

    v0 = _mm_mullo_epi32(s4, cospi48);
    v1 = _mm_mullo_epi32(s5, cospi48);
    v2 = _mm_mullo_epi32(s6, cospi16);
    v3 = _mm_mullo_epi32(s7, cospi16);
    v4 = _mm_add_epi32(v0, v2);
    v5 = _mm_add_epi32(v1, v3);

    v6 = _mm_add_epi32(v4, rnding);
    v7 = _mm_add_epi32(v5, rnding);
    u2 = _mm_srai_epi32(v6, bit);
    u3 = _mm_srai_epi32(v7, bit);

    // Transpose 4x4 32-bit
    v0 = _mm_unpacklo_epi32(u0, u2);
    v1 = _mm_unpacklo_epi32(u1, u3);
    v2 = _mm_unpackhi_epi32(u0, u2);
    v3 = _mm_unpackhi_epi32(u1, u3);

    output[0] = _mm_unpacklo_epi64(v0, zero);
    output[1] = _mm_unpackhi_epi64(v0, zero);
    output[2] = _mm_unpacklo_epi64(v2, zero);
    output[3] = _mm_unpackhi_epi64(v2, zero);
    output[4] = _mm_unpacklo_epi64(v1, zero);
    output[5] = _mm_unpackhi_epi64(v1, zero);
    output[6] = _mm_unpacklo_epi64(v3, zero);
    output[7] = _mm_unpackhi_epi64(v3, zero);
}

static void fadst4x8_col_N2_sse4_1(__m128i* in, __m128i* output, int32_t bit, const int32_t num_col) {
    const int32_t* sinpi  = sinpi_arr(bit);
    const __m128i  zero   = _mm_setzero_si128();
    const __m128i  rnding = _mm_set1_epi32(1 << (bit - 1));
    const __m128i  sinpi1 = _mm_set1_epi32((int32_t)sinpi[1]);
    const __m128i  sinpi2 = _mm_set1_epi32((int32_t)sinpi[2]);
    const __m128i  sinpi3 = _mm_set1_epi32((int32_t)sinpi[3]);
    const __m128i  sinpi4 = _mm_set1_epi32((int32_t)sinpi[4]);
    __m128i        s0, s1, s2, s3, s4, s5, s6, s7, s8, s9;
    __m128i        x0, x1, x2, x3;
    __m128i        u0, u1, u2, u3;
    __m128i        v0, v1, v2, v3;

    int32_t idx = 0 * num_col;
    s0          = _mm_mullo_epi32(in[2 * idx], sinpi1);
    s1          = _mm_mullo_epi32(in[2 * idx + 1], sinpi1);
    u0          = _mm_add_epi32(in[2 * idx], in[2 * (idx + num_col)]);
    u1          = _mm_add_epi32(in[2 * idx + 1], in[2 * (idx + num_col) + 1]);
    idx += num_col;
    s4 = _mm_mullo_epi32(in[2 * idx], sinpi2);
    s5 = _mm_mullo_epi32(in[2 * idx + 1], sinpi2);
    idx += num_col;
    s8 = _mm_mullo_epi32(in[2 * idx], sinpi3);
    s9 = _mm_mullo_epi32(in[2 * idx + 1], sinpi3);
    idx += num_col;
    s6 = _mm_mullo_epi32(in[2 * idx], sinpi4);
    s7 = _mm_mullo_epi32(in[2 * idx + 1], sinpi4);
    s2 = _mm_sub_epi32(u0, in[2 * idx]);
    s3 = _mm_sub_epi32(u1, in[2 * idx + 1]);

    u0 = _mm_add_epi32(s0, s4);
    u1 = _mm_add_epi32(s1, s5);
    x0 = _mm_add_epi32(u0, s6);
    x1 = _mm_add_epi32(u1, s7);
    x2 = _mm_mullo_epi32(s2, sinpi3);
    x3 = _mm_mullo_epi32(s3, sinpi3);

    s0 = _mm_add_epi32(x0, s8);
    s1 = _mm_add_epi32(x1, s9);

    u0 = _mm_add_epi32(s0, rnding);
    u1 = _mm_add_epi32(s1, rnding);
    u0 = _mm_srai_epi32(u0, bit);
    u1 = _mm_srai_epi32(u1, bit);

    u2 = _mm_add_epi32(x2, rnding);
    u3 = _mm_add_epi32(x3, rnding);
    u2 = _mm_srai_epi32(u2, bit);
    u3 = _mm_srai_epi32(u3, bit);

    // Transpose 4x4 32-bit
    v0 = _mm_unpacklo_epi32(u0, u2);
    v1 = _mm_unpacklo_epi32(u1, u3);
    v2 = _mm_unpackhi_epi32(u0, u2);
    v3 = _mm_unpackhi_epi32(u1, u3);

    output[0] = _mm_unpacklo_epi64(v0, zero);
    output[1] = _mm_unpackhi_epi64(v0, zero);
    output[2] = _mm_unpacklo_epi64(v2, zero);
    output[3] = _mm_unpackhi_epi64(v2, zero);
    output[4] = _mm_unpacklo_epi64(v1, zero);
    output[5] = _mm_unpackhi_epi64(v1, zero);
    output[6] = _mm_unpacklo_epi64(v3, zero);
    output[7] = _mm_unpackhi_epi64(v3, zero);
}

static AOM_FORCE_INLINE void clear_buffer_4x16_N2(__m128i* buff) {
    const __m128i zero = _mm_setzero_si128();
    buff[8]            = zero;
    buff[9]            = zero;
    buff[10]           = zero;
    buff[11]           = zero;
    buff[12]           = zero;
    buff[13]           = zero;
    buff[14]           = zero;
    buff[15]           = zero;
}

static INLINE void fidtx4x8_col_N2_sse4_1(__m128i* in, __m128i* output, int32_t bit, int32_t col_num) {
    (void)bit;
    const __m128i zero   = _mm_setzero_si128();
    __m128i       fact   = _mm_set1_epi32(new_sqrt2);
    __m128i       offset = _mm_set1_epi32(1 << (new_sqrt2_bits - 1));
    __m128i       r0, r1;

    for (int32_t i = 0; i < col_num; i++) {
        r0            = _mm_mullo_epi32(in[2 * i], fact);
        r0            = _mm_add_epi32(r0, offset);
        r0            = _mm_srai_epi32(r0, new_sqrt2_bits);
        output[2 * i] = _mm_unpacklo_epi64(r0, zero);

        r1                = _mm_mullo_epi32(in[2 * i + 1], fact);
        r1                = _mm_add_epi32(r1, offset);
        r1                = _mm_srai_epi32(r1, new_sqrt2_bits);
        output[2 * i + 1] = _mm_unpacklo_epi64(r1, zero);
    }
}

static AOM_FORCE_INLINE void fidtx4x8_N2_perm_sse4_1(__m128i* in, __m128i* output, int32_t bit) {
    (void)bit;
    __m128i fact   = _mm_set1_epi32(2 * new_sqrt2);
    __m128i offset = _mm_set1_epi32(1 << (new_sqrt2_bits - 1));
    __m128i a_low0, a_low1;
    __m128i out[4];

    a_low0 = _mm_mullo_epi32(in[0], fact);
    a_low0 = _mm_add_epi32(a_low0, offset);
    out[0] = _mm_srai_epi32(a_low0, new_sqrt2_bits);
    a_low1 = _mm_mullo_epi32(in[1], fact);
    a_low1 = _mm_add_epi32(a_low1, offset);
    out[1] = _mm_srai_epi32(a_low1, new_sqrt2_bits);
    a_low0 = _mm_mullo_epi32(in[4], fact);
    a_low0 = _mm_add_epi32(a_low0, offset);
    out[2] = _mm_srai_epi32(a_low0, new_sqrt2_bits);
    a_low1 = _mm_mullo_epi32(in[5], fact);
    a_low1 = _mm_add_epi32(a_low1, offset);
    out[3] = _mm_srai_epi32(a_low1, new_sqrt2_bits);

    output[0] = out[0];
    output[1] = out[2];
    output[4] = out[1];
    output[5] = out[3];
}

static INLINE void fidtx4x8_row_N2_sse4_1(__m128i* in, __m128i* output, int32_t bit) {
    (void)bit;
    __m128i fact   = _mm_set1_epi32(new_sqrt2);
    __m128i offset = _mm_set1_epi32(1 << (new_sqrt2_bits - 1));
    __m128i a_low0, a_low1;

    for (int32_t i = 0; i < 4; i += 2) {
        a_low0            = _mm_mullo_epi32(in[2 * i], fact);
        a_low1            = _mm_mullo_epi32(in[2 * i + 1], fact);
        a_low0            = _mm_add_epi32(a_low0, offset);
        a_low1            = _mm_add_epi32(a_low1, offset);
        output[2 * i]     = _mm_srai_epi32(a_low0, new_sqrt2_bits);
        output[2 * i + 1] = _mm_srai_epi32(a_low1, new_sqrt2_bits);
    }
}

static void fdct4x8_row_N2_with_round_sse4_1(__m128i* input, __m128i* output, int32_t bit, const int32_t num_col,
                                             int32_t shift) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  zero     = _mm_setzero_si128();
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospi16  = _mm_set1_epi32(cospi[16]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    const __m128i  rounding = _mm_set1_epi32(1 << (shift - 1));
    __m128i        in[8];
    __m128i        s0, s1, s2, s3, s4, s5, s6, s7;
    __m128i        u0, u1, u2, u3, u4, u5, u6, u7;
    __m128i        v0, v1, v2, v3, v4, v5, v6, v7;
    int32_t        endidx = 3 * num_col;

    in[0] = input[0];
    in[1] = input[4];
    in[2] = input[1];
    in[3] = input[5];
    in[4] = input[2];
    in[5] = input[6];
    in[6] = input[3];
    in[7] = input[7];

    s0 = _mm_add_epi32(in[0], in[2 * endidx]);
    s1 = _mm_add_epi32(in[1], in[2 * endidx + 1]);
    s6 = _mm_sub_epi32(in[0], in[2 * endidx]);
    s7 = _mm_sub_epi32(in[1], in[2 * endidx + 1]);
    endidx -= num_col;
    s2 = _mm_add_epi32(in[2 * num_col], in[2 * endidx]);
    s3 = _mm_add_epi32(in[2 * num_col + 1], in[2 * endidx + 1]);
    s4 = _mm_sub_epi32(in[2 * num_col], in[2 * endidx]);
    s5 = _mm_sub_epi32(in[2 * num_col + 1], in[2 * endidx + 1]);

    u0 = _mm_mullo_epi32(s0, cospi32);
    u1 = _mm_mullo_epi32(s1, cospi32);
    u2 = _mm_mullo_epi32(s2, cospi32);
    u3 = _mm_mullo_epi32(s3, cospi32);
    u4 = _mm_add_epi32(u0, u2);
    u5 = _mm_add_epi32(u1, u3);

    u6 = _mm_add_epi32(u4, rnding);
    u7 = _mm_add_epi32(u5, rnding);

    u0 = _mm_srai_epi32(u6, bit);
    u1 = _mm_srai_epi32(u7, bit);

    v0 = _mm_mullo_epi32(s4, cospi48);
    v1 = _mm_mullo_epi32(s5, cospi48);
    v2 = _mm_mullo_epi32(s6, cospi16);
    v3 = _mm_mullo_epi32(s7, cospi16);
    v4 = _mm_add_epi32(v0, v2);
    v5 = _mm_add_epi32(v1, v3);

    v6 = _mm_add_epi32(v4, rnding);
    v7 = _mm_add_epi32(v5, rnding);
    u2 = _mm_srai_epi32(v6, bit);
    u3 = _mm_srai_epi32(v7, bit);

    //round
    u0 = _mm_add_epi32(u0, rounding);
    u1 = _mm_add_epi32(u1, rounding);
    u0 = _mm_srai_epi32(u0, shift);
    u1 = _mm_srai_epi32(u1, shift);
    u2 = _mm_add_epi32(u2, rounding);
    u3 = _mm_add_epi32(u3, rounding);
    u2 = _mm_srai_epi32(u2, shift);
    u3 = _mm_srai_epi32(u3, shift);

    // Transpose 4x4 32-bit
    v0 = _mm_unpacklo_epi32(u0, u2);
    v1 = _mm_unpacklo_epi32(u1, u3);
    v2 = _mm_unpackhi_epi32(u0, u2);
    v3 = _mm_unpackhi_epi32(u1, u3);

    output[0] = _mm_unpacklo_epi64(v0, zero);
    output[1] = _mm_unpackhi_epi64(v0, zero);
    output[2] = _mm_unpacklo_epi64(v2, zero);
    output[3] = _mm_unpackhi_epi64(v2, zero);
    output[4] = _mm_unpacklo_epi64(v1, zero);
    output[5] = _mm_unpackhi_epi64(v1, zero);
    output[6] = _mm_unpacklo_epi64(v3, zero);
    output[7] = _mm_unpackhi_epi64(v3, zero);
}

static void fadst4x8_row_N2_with_round_sse4_1(__m128i* input, __m128i* output, int32_t bit, const int32_t num_col,
                                              int32_t shift) {
    const int32_t* sinpi    = sinpi_arr(bit);
    const __m128i  zero     = _mm_setzero_si128();
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    const __m128i  rounding = _mm_set1_epi32(1 << (shift - 1));
    const __m128i  sinpi1   = _mm_set1_epi32((int32_t)sinpi[1]);
    const __m128i  sinpi2   = _mm_set1_epi32((int32_t)sinpi[2]);
    const __m128i  sinpi3   = _mm_set1_epi32((int32_t)sinpi[3]);
    const __m128i  sinpi4   = _mm_set1_epi32((int32_t)sinpi[4]);
    __m128i        s0, s1, s2, s3, s4, s5, s6, s7, s8, s9;
    __m128i        x0, x1, x2, x3;
    __m128i        u0, u1, u2, u3;
    __m128i        v0, v1, v2, v3;
    __m128i        in[8];

    int32_t idx = 0 * num_col;
    in[0]       = input[0];
    in[1]       = input[4];
    in[2]       = input[1];
    in[3]       = input[5];
    in[4]       = input[2];
    in[5]       = input[6];
    in[6]       = input[3];
    in[7]       = input[7];

    s0 = _mm_mullo_epi32(in[2 * idx], sinpi1);
    s1 = _mm_mullo_epi32(in[2 * idx + 1], sinpi1);
    u0 = _mm_add_epi32(in[2 * idx], in[2 * (idx + num_col)]);
    u1 = _mm_add_epi32(in[2 * idx + 1], in[2 * (idx + num_col) + 1]);
    idx += num_col;
    s4 = _mm_mullo_epi32(in[2 * idx], sinpi2);
    s5 = _mm_mullo_epi32(in[2 * idx + 1], sinpi2);
    idx += num_col;
    s8 = _mm_mullo_epi32(in[2 * idx], sinpi3);
    s9 = _mm_mullo_epi32(in[2 * idx + 1], sinpi3);
    idx += num_col;
    s6 = _mm_mullo_epi32(in[2 * idx], sinpi4);
    s7 = _mm_mullo_epi32(in[2 * idx + 1], sinpi4);
    s2 = _mm_sub_epi32(u0, in[2 * idx]);
    s3 = _mm_sub_epi32(u1, in[2 * idx + 1]);

    u0 = _mm_add_epi32(s0, s4);
    u1 = _mm_add_epi32(s1, s5);
    x0 = _mm_add_epi32(u0, s6);
    x1 = _mm_add_epi32(u1, s7);
    x2 = _mm_mullo_epi32(s2, sinpi3);
    x3 = _mm_mullo_epi32(s3, sinpi3);

    s0 = _mm_add_epi32(x0, s8);
    s1 = _mm_add_epi32(x1, s9);

    u0 = _mm_add_epi32(s0, rnding);
    u1 = _mm_add_epi32(s1, rnding);
    u0 = _mm_srai_epi32(u0, bit);
    u1 = _mm_srai_epi32(u1, bit);

    u2 = _mm_add_epi32(x2, rnding);
    u3 = _mm_add_epi32(x3, rnding);
    u2 = _mm_srai_epi32(u2, bit);
    u3 = _mm_srai_epi32(u3, bit);

    //round
    u0 = _mm_add_epi32(u0, rounding);
    u1 = _mm_add_epi32(u1, rounding);
    u0 = _mm_srai_epi32(u0, shift);
    u1 = _mm_srai_epi32(u1, shift);
    u2 = _mm_add_epi32(u2, rounding);
    u3 = _mm_add_epi32(u3, rounding);
    u2 = _mm_srai_epi32(u2, shift);
    u3 = _mm_srai_epi32(u3, shift);

    // Transpose 4x4 32-bit
    v0 = _mm_unpacklo_epi32(u0, u2);
    v1 = _mm_unpacklo_epi32(u1, u3);
    v2 = _mm_unpackhi_epi32(u0, u2);
    v3 = _mm_unpackhi_epi32(u1, u3);

    output[0] = _mm_unpacklo_epi64(v0, zero);
    output[1] = _mm_unpackhi_epi64(v0, zero);
    output[2] = _mm_unpacklo_epi64(v2, zero);
    output[3] = _mm_unpackhi_epi64(v2, zero);
    output[4] = _mm_unpacklo_epi64(v1, zero);
    output[5] = _mm_unpackhi_epi64(v1, zero);
    output[6] = _mm_unpacklo_epi64(v3, zero);
    output[7] = _mm_unpackhi_epi64(v3, zero);
}

static INLINE void fidtx4x8_row_N2_with_round_sse4_1(__m128i* input, __m128i* output, int32_t bit, int32_t shift) {
    (void)bit;
    __m128i       in[4];
    __m128i       out[4];
    __m128i       fact     = _mm_set1_epi32(new_sqrt2);
    __m128i       offset   = _mm_set1_epi32(1 << (new_sqrt2_bits - 1));
    const __m128i rounding = _mm_set1_epi32(1 << (shift - 1));
    const __m128i zero     = _mm_setzero_si128();
    __m128i       a_low;
    __m128i       v0, v1, v2, v3;

    in[0] = input[0];
    in[1] = input[4];
    in[2] = input[1];
    in[3] = input[5];

    for (int32_t i = 0; i < 4; i++) {
        a_low  = _mm_mullo_epi32(in[i], fact);
        a_low  = _mm_add_epi32(a_low, offset);
        a_low  = _mm_srai_epi32(a_low, new_sqrt2_bits);
        a_low  = _mm_add_epi32(a_low, rounding);
        out[i] = _mm_srai_epi32(a_low, shift);
    }

    // Transpose 4x4 32-bit
    v0 = _mm_unpacklo_epi32(out[0], out[2]);
    v1 = _mm_unpacklo_epi32(out[1], out[3]);
    v2 = _mm_unpackhi_epi32(out[0], out[2]);
    v3 = _mm_unpackhi_epi32(out[1], out[3]);

    output[0] = _mm_unpacklo_epi64(v0, zero);
    output[1] = _mm_unpackhi_epi64(v0, zero);
    output[2] = _mm_unpacklo_epi64(v2, zero);
    output[3] = _mm_unpackhi_epi64(v2, zero);
    output[4] = _mm_unpacklo_epi64(v1, zero);
    output[5] = _mm_unpackhi_epi64(v1, zero);
    output[6] = _mm_unpacklo_epi64(v3, zero);
    output[7] = _mm_unpackhi_epi64(v3, zero);
}

static void fdct4x8_N2_sse4_1(__m128i* input, __m128i* output, int32_t bit) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospim32 = _mm_set1_epi32(-cospi[32]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospi16  = _mm_set1_epi32(cospi[16]);
    const __m128i  cospi56  = _mm_set1_epi32(cospi[56]);
    const __m128i  cospi8   = _mm_set1_epi32(cospi[8]);
    const __m128i  cospi24  = _mm_set1_epi32(cospi[24]);
    const __m128i  cospi40  = _mm_set1_epi32(cospi[40]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    __m128i        u[8], v[8];

    // Even 8 points 0, 2, ..., 14
    // stage 0
    // stage 1
    u[0] = _mm_add_epi32(input[0], input[7]);
    v[7] = _mm_sub_epi32(input[0], input[7]); // v[7]
    u[1] = _mm_add_epi32(input[1], input[6]);
    u[6] = _mm_sub_epi32(input[1], input[6]);
    u[2] = _mm_add_epi32(input[2], input[5]);
    u[5] = _mm_sub_epi32(input[2], input[5]);
    u[3] = _mm_add_epi32(input[3], input[4]);
    v[4] = _mm_sub_epi32(input[3], input[4]); // v[4]

    // stage 2
    v[0] = _mm_add_epi32(u[0], u[3]);
    v[3] = _mm_sub_epi32(u[0], u[3]);
    v[1] = _mm_add_epi32(u[1], u[2]);
    v[2] = _mm_sub_epi32(u[1], u[2]);

    v[5] = _mm_mullo_epi32(u[5], cospim32);
    v[6] = _mm_mullo_epi32(u[6], cospi32);
    v[5] = _mm_add_epi32(v[5], v[6]);
    v[5] = _mm_add_epi32(v[5], rnding);
    v[5] = _mm_srai_epi32(v[5], bit);

    u[0] = _mm_mullo_epi32(u[5], cospi32);
    v[6] = _mm_mullo_epi32(u[6], cospim32);
    v[6] = _mm_sub_epi32(u[0], v[6]);
    v[6] = _mm_add_epi32(v[6], rnding);
    v[6] = _mm_srai_epi32(v[6], bit);

    // stage 3
    // type 0
    v[0]      = _mm_mullo_epi32(v[0], cospi32);
    v[1]      = _mm_mullo_epi32(v[1], cospi32);
    u[0]      = _mm_add_epi32(v[0], v[1]);
    u[0]      = _mm_add_epi32(u[0], rnding);
    output[0] = _mm_srai_epi32(u[0], bit);

    // type 1
    v[0]      = _mm_mullo_epi32(v[2], cospi48);
    v[1]      = _mm_mullo_epi32(v[3], cospi16);
    u[2]      = _mm_add_epi32(v[0], v[1]);
    u[2]      = _mm_add_epi32(u[2], rnding);
    output[2] = _mm_srai_epi32(u[2], bit);

    u[4] = _mm_add_epi32(v[4], v[5]);
    u[5] = _mm_sub_epi32(v[4], v[5]);
    u[6] = _mm_sub_epi32(v[7], v[6]);
    u[7] = _mm_add_epi32(v[7], v[6]);

    // stage 4
    // stage 5
    v[0]      = _mm_mullo_epi32(u[4], cospi56);
    v[1]      = _mm_mullo_epi32(u[7], cospi8);
    v[0]      = _mm_add_epi32(v[0], v[1]);
    v[0]      = _mm_add_epi32(v[0], rnding);
    output[1] = _mm_srai_epi32(v[0], bit); // buf0[4]

    v[0]      = _mm_mullo_epi32(u[5], cospi40);
    v[1]      = _mm_mullo_epi32(u[6], cospi24);
    v[0]      = _mm_sub_epi32(v[1], v[0]);
    v[0]      = _mm_add_epi32(v[0], rnding);
    output[3] = _mm_srai_epi32(v[0], bit); // buf0[6]
}

static AOM_FORCE_INLINE void col_txfm_8x4_N2_rounding(__m128i* in, int32_t shift) {
    const __m128i rounding = _mm_set1_epi32(1 << (shift - 1));

    in[0] = _mm_add_epi32(in[0], rounding);
    in[1] = _mm_add_epi32(in[1], rounding);
    in[2] = _mm_add_epi32(in[2], rounding);
    in[3] = _mm_add_epi32(in[3], rounding);

    in[0] = _mm_srai_epi32(in[0], shift);
    in[1] = _mm_srai_epi32(in[1], shift);
    in[2] = _mm_srai_epi32(in[2], shift);
    in[3] = _mm_srai_epi32(in[3], shift);
}

static AOM_FORCE_INLINE void write_buffer_4x8_N2(const __m128i* res, int32_t* output) {
    const __m128i zero = _mm_setzero_si128();
    _mm_storeu_si128((__m128i*)(output + 0 * 4), res[0]);
    _mm_storeu_si128((__m128i*)(output + 1 * 4), res[1]);
    _mm_storeu_si128((__m128i*)(output + 2 * 4), res[2]);
    _mm_storeu_si128((__m128i*)(output + 3 * 4), res[3]);
    _mm_storeu_si128((__m128i*)(output + 4 * 4), zero);
    _mm_storeu_si128((__m128i*)(output + 5 * 4), zero);
    _mm_storeu_si128((__m128i*)(output + 6 * 4), zero);
    _mm_storeu_si128((__m128i*)(output + 7 * 4), zero);
}

static void fadst8x4_N2_sse4_1(__m128i* input, __m128i* output, int32_t bit, const int32_t col_num) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospi16  = _mm_set1_epi32(cospi[16]);
    const __m128i  cospim16 = _mm_set1_epi32(-cospi[16]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospim48 = _mm_set1_epi32(-cospi[48]);
    const __m128i  cospim4  = _mm_set1_epi32(-cospi[4]);
    const __m128i  cospi60  = _mm_set1_epi32(cospi[60]);
    const __m128i  cospim20 = _mm_set1_epi32(-cospi[20]);
    const __m128i  cospi44  = _mm_set1_epi32(cospi[44]);
    const __m128i  cospi28  = _mm_set1_epi32(cospi[28]);
    const __m128i  cospi36  = _mm_set1_epi32(cospi[36]);
    const __m128i  cospi52  = _mm_set1_epi32(cospi[52]);
    const __m128i  cospi12  = _mm_set1_epi32(cospi[12]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    const __m128i  zero     = _mm_setzero_si128();
    __m128i        u0, u1, u2, u3, u4, u5, u6, u7;
    __m128i        v0, v1, v2, v3, v4, v5, v6, v7;
    __m128i        x, y;
    int32_t        col;

    // Note:
    //  Even column: 0, 2, ..., 14
    //  Odd column: 1, 3, ..., 15
    //  one even column plus one odd column constructs one row (8 coeffs)
    //  total we have 8 rows (8x8).
    for (col = 0; col < col_num; ++col) {
        // stage 0
        // stage 1
        u0 = input[col_num * 0 + col];
        u1 = _mm_sub_epi32(zero, input[col_num * 7 + col]);
        u2 = _mm_sub_epi32(zero, input[col_num * 3 + col]);
        u3 = input[col_num * 4 + col];
        u4 = _mm_sub_epi32(zero, input[col_num * 1 + col]);
        u5 = input[col_num * 6 + col];
        u6 = input[col_num * 2 + col];
        u7 = _mm_sub_epi32(zero, input[col_num * 5 + col]);

        // stage 2
        v0 = u0;
        v1 = u1;

        x  = _mm_mullo_epi32(u2, cospi32);
        y  = _mm_mullo_epi32(u3, cospi32);
        v2 = _mm_add_epi32(x, y);
        v2 = _mm_add_epi32(v2, rnding);
        v2 = _mm_srai_epi32(v2, bit);

        v3 = _mm_sub_epi32(x, y);
        v3 = _mm_add_epi32(v3, rnding);
        v3 = _mm_srai_epi32(v3, bit);

        v4 = u4;
        v5 = u5;

        x  = _mm_mullo_epi32(u6, cospi32);
        y  = _mm_mullo_epi32(u7, cospi32);
        v6 = _mm_add_epi32(x, y);
        v6 = _mm_add_epi32(v6, rnding);
        v6 = _mm_srai_epi32(v6, bit);

        v7 = _mm_sub_epi32(x, y);
        v7 = _mm_add_epi32(v7, rnding);
        v7 = _mm_srai_epi32(v7, bit);

        // stage 3
        u0 = _mm_add_epi32(v0, v2);
        u1 = _mm_add_epi32(v1, v3);
        u2 = _mm_sub_epi32(v0, v2);
        u3 = _mm_sub_epi32(v1, v3);
        u4 = _mm_add_epi32(v4, v6);
        u5 = _mm_add_epi32(v5, v7);
        u6 = _mm_sub_epi32(v4, v6);
        u7 = _mm_sub_epi32(v5, v7);

        // stage 4
        v0 = u0;
        v1 = u1;
        v2 = u2;
        v3 = u3;

        x  = _mm_mullo_epi32(u4, cospi16);
        y  = _mm_mullo_epi32(u5, cospi48);
        v4 = _mm_add_epi32(x, y);
        v4 = _mm_add_epi32(v4, rnding);
        v4 = _mm_srai_epi32(v4, bit);

        x  = _mm_mullo_epi32(u4, cospi48);
        y  = _mm_mullo_epi32(u5, cospim16);
        v5 = _mm_add_epi32(x, y);
        v5 = _mm_add_epi32(v5, rnding);
        v5 = _mm_srai_epi32(v5, bit);

        x  = _mm_mullo_epi32(u6, cospim48);
        y  = _mm_mullo_epi32(u7, cospi16);
        v6 = _mm_add_epi32(x, y);
        v6 = _mm_add_epi32(v6, rnding);
        v6 = _mm_srai_epi32(v6, bit);

        x  = _mm_mullo_epi32(u6, cospi16);
        y  = _mm_mullo_epi32(u7, cospi48);
        v7 = _mm_add_epi32(x, y);
        v7 = _mm_add_epi32(v7, rnding);
        v7 = _mm_srai_epi32(v7, bit);

        // stage 5
        u0 = _mm_add_epi32(v0, v4);
        u1 = _mm_add_epi32(v1, v5);
        u2 = _mm_add_epi32(v2, v6);
        u3 = _mm_add_epi32(v3, v7);
        u4 = _mm_sub_epi32(v0, v4);
        u5 = _mm_sub_epi32(v1, v5);
        u6 = _mm_sub_epi32(v2, v6);
        u7 = _mm_sub_epi32(v3, v7);

        // stage 6
        x                         = _mm_mullo_epi32(u0, cospi60);
        y                         = _mm_mullo_epi32(u1, cospim4);
        v1                        = _mm_add_epi32(x, y);
        v1                        = _mm_add_epi32(v1, rnding);
        output[col_num * 0 + col] = _mm_srai_epi32(v1, bit);

        x                         = _mm_mullo_epi32(u2, cospi44);
        y                         = _mm_mullo_epi32(u3, cospim20);
        v3                        = _mm_add_epi32(x, y);
        v3                        = _mm_add_epi32(v3, rnding);
        output[col_num * 2 + col] = _mm_srai_epi32(v3, bit);

        x                         = _mm_mullo_epi32(u4, cospi36);
        y                         = _mm_mullo_epi32(u5, cospi28);
        v4                        = _mm_add_epi32(x, y);
        v4                        = _mm_add_epi32(v4, rnding);
        output[col_num * 3 + col] = _mm_srai_epi32(v4, bit);

        x                         = _mm_mullo_epi32(u6, cospi52);
        y                         = _mm_mullo_epi32(u7, cospi12);
        v6                        = _mm_add_epi32(x, y);
        v6                        = _mm_add_epi32(v6, rnding);
        output[col_num * 1 + col] = _mm_srai_epi32(v6, bit);
    }
}

static AOM_FORCE_INLINE void fidtx8x4_N2_sse4_1(__m128i* in, __m128i* out, int32_t bit) {
    (void)bit;
    out[0] = _mm_add_epi32(in[0], in[0]);
    out[1] = _mm_add_epi32(in[1], in[1]);
    out[2] = _mm_add_epi32(in[2], in[2]);
    out[3] = _mm_add_epi32(in[3], in[3]);
}

static AOM_FORCE_INLINE void write_buffer_16x8_N2_sse4_1(const __m128i* res, __m128i* out) {
    out[0]  = res[0];
    out[1]  = res[1];
    out[4]  = res[2];
    out[5]  = res[3];
    out[8]  = res[4];
    out[9]  = res[5];
    out[12] = res[6];
    out[13] = res[7];
}

void svt_av1_fwd_txfm2d_8x8_N2_sse4_1(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[16], out[16];
    const int8_t* shift   = fwd_txfm_shift_ls[TX_8X8];
    const int32_t txw_idx = get_txw_idx(TX_8X8);
    const int32_t txh_idx = get_txh_idx(TX_8X8);

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fdct8x8_N2_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        transpose_8x8_half_sse4_1(out, in);
        fdct8x8_N2_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_sse4_1(out, in);
        write_buffer_8x8_N2(in, coeff);
        break;
    case ADST_DCT:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fadst8x8_N2_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        transpose_8x8_half_sse4_1(out, in);
        fdct8x8_N2_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_sse4_1(out, in);
        write_buffer_8x8_N2(in, coeff);
        break;
    case DCT_ADST:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fdct8x8_N2_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        transpose_8x8_half_sse4_1(out, in);
        fadst8x8_N2_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_sse4_1(out, in);
        write_buffer_8x8_N2(in, coeff);
        break;
    case ADST_ADST:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fadst8x8_N2_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        transpose_8x8_half_sse4_1(out, in);
        fadst8x8_N2_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_sse4_1(out, in);
        write_buffer_8x8_N2(in, coeff);
        break;
    case FLIPADST_DCT:
        load_buffer_8x8(input, in, stride, 1, 0, shift[0]);
        fadst8x8_N2_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        transpose_8x8_half_sse4_1(out, in);
        fdct8x8_N2_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_sse4_1(out, in);
        write_buffer_8x8_N2(in, coeff);
        break;
    case DCT_FLIPADST:
        load_buffer_8x8(input, in, stride, 0, 1, shift[0]);
        fdct8x8_N2_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        transpose_8x8_half_sse4_1(out, in);
        fadst8x8_N2_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_sse4_1(out, in);
        write_buffer_8x8_N2(in, coeff);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_8x8(input, in, stride, 1, 1, shift[0]);
        fadst8x8_N2_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        transpose_8x8_half_sse4_1(out, in);
        fadst8x8_N2_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_sse4_1(out, in);
        write_buffer_8x8_N2(in, coeff);
        break;
    case ADST_FLIPADST:
        load_buffer_8x8(input, in, stride, 0, 1, shift[0]);
        fadst8x8_N2_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        transpose_8x8_half_sse4_1(out, in);
        fadst8x8_N2_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_sse4_1(out, in);
        write_buffer_8x8_N2(in, coeff);
        break;
    case FLIPADST_ADST:
        load_buffer_8x8(input, in, stride, 1, 0, shift[0]);
        fadst8x8_N2_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        transpose_8x8_half_sse4_1(out, in);
        fadst8x8_N2_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_sse4_1(out, in);
        write_buffer_8x8_N2(in, coeff);
        break;
    case IDTX:
        load_buffer_4x8_in_8x8(input, in, stride, 0, 0, shift[0], 1);
        fidtx8x8_N2_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        fidtx8x8_N2_sse4_1(out, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_8x8_N2(out, coeff);
        break;
    case V_DCT:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fdct8x8_N2_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        fidtx8x8_N2_sse4_1(out, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_8x8_N2(out, coeff);
        break;
    case H_DCT:
        load_buffer_4x8_in_8x8(input, in, stride, 0, 0, shift[0], 1);
        fidtx8x8_N2_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_sse4_1(in, out);
        fdct8x8_N2_sse4_1(out, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_sse4_1(in, out);
        write_buffer_8x8_N2(out, coeff);
        break;
    case V_ADST:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fadst8x8_N2_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        fidtx8x8_N2_sse4_1(out, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_8x8_N2(out, coeff);
        break;
    case H_ADST:
        load_buffer_4x8_in_8x8(input, in, stride, 0, 0, shift[0], 1);
        fidtx8x8_N2_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_sse4_1(in, out);
        fadst8x8_N2_sse4_1(out, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_sse4_1(in, out);
        write_buffer_8x8_N2(out, coeff);
        break;
    case V_FLIPADST:
        load_buffer_8x8(input, in, stride, 1, 0, shift[0]);
        fadst8x8_N2_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        fidtx8x8_N2_sse4_1(out, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_8x8_N2(out, coeff);
        break;
    case H_FLIPADST:
        load_buffer_4x8_in_8x8(input, in, stride, 0, 1, shift[0], 1);
        fidtx8x8_N2_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_sse4_1(in, out);
        fadst8x8_N2_sse4_1(out, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_sse4_1(in, out);
        write_buffer_8x8_N2(out, coeff);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_16x16_N2_sse4_1(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[64], out[64];
    const int8_t* shift   = fwd_txfm_shift_ls[TX_16X16];
    const int32_t txw_idx = get_txw_idx(TX_16X16);
    const int32_t txh_idx = get_txh_idx(TX_16X16);
    const int32_t col_num = 2;
    switch (tx_type) {
    case IDTX:
        load_buffer_16x16_N2_half(input, in, stride, 0, 0, shift[0]);
        fidtx16x16_N2_row_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_N2_half_rounding(out, -shift[1]);
        fidtx16x16_N2_row_sse4_1(out, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        write_buffer_16x16_N2(out, coeff);
        break;
    case DCT_DCT:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fdct16x16_N2_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num);
        col_txfm_16x16_N2_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_sse4_1(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_sse4_1(out + 2, in + 32); //top-right ->bottom-left
        fdct16x16_N2_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, col_num / 2);
        transpose_8x8_in_16x16_sse4_1(out, in);
        write_buffer_16x16_N2(in, coeff);
        break;
    case ADST_DCT:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fadst16x16_N2_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num);
        col_txfm_16x16_N2_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_sse4_1(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_sse4_1(out + 2, in + 32); //top-right ->bottom-left
        fdct16x16_N2_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, col_num / 2);
        transpose_8x8_in_16x16_sse4_1(out, in);
        write_buffer_16x16_N2(in, coeff);
        break;
    case DCT_ADST:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fdct16x16_N2_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num);
        col_txfm_16x16_N2_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_sse4_1(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_sse4_1(out + 2, in + 32); //top-right ->bottom-left
        fadst16x16_N2_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, col_num);
        transpose_8x8_in_16x16_sse4_1(out, in);
        write_buffer_16x16_N2(in, coeff);
        break;
    case ADST_ADST:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fadst16x16_N2_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num);
        col_txfm_16x16_N2_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_sse4_1(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_sse4_1(out + 2, in + 32); //top-right ->bottom-left
        fadst16x16_N2_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, col_num / 2);
        transpose_8x8_in_16x16_sse4_1(out, in);
        write_buffer_16x16_N2(in, coeff);
        break;
    case DCT_FLIPADST:
        load_buffer_16x16(input, in, stride, 0, 1, shift[0]);
        fdct16x16_N2_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num);
        col_txfm_16x16_N2_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_sse4_1(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_sse4_1(out + 2, in + 32); //top-right ->bottom-left
        fadst16x16_N2_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, col_num / 2);
        transpose_8x8_in_16x16_sse4_1(out, in);
        write_buffer_16x16_N2(in, coeff);
        break;
    case FLIPADST_DCT:
        load_buffer_16x16(input, in, stride, 1, 0, shift[0]);
        fadst16x16_N2_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num);
        col_txfm_16x16_N2_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_sse4_1(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_sse4_1(out + 2, in + 32); //top-right ->bottom-left
        fdct16x16_N2_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, col_num / 2);
        transpose_8x8_in_16x16_sse4_1(out, in);
        write_buffer_16x16_N2(in, coeff);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_16x16(input, in, stride, 1, 1, shift[0]);
        fadst16x16_N2_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num);
        col_txfm_16x16_N2_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_sse4_1(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_sse4_1(out + 2, in + 32); //top-right ->bottom-left
        fadst16x16_N2_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, col_num / 2);
        transpose_8x8_in_16x16_sse4_1(out, in);
        write_buffer_16x16_N2(in, coeff);
        break;
    case ADST_FLIPADST:
        load_buffer_16x16(input, in, stride, 0, 1, shift[0]);
        fadst16x16_N2_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num);
        col_txfm_16x16_N2_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_sse4_1(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_sse4_1(out + 2, in + 32); //top-right ->bottom-left
        fadst16x16_N2_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, col_num / 2);
        transpose_8x8_in_16x16_sse4_1(out, in);
        write_buffer_16x16_N2(in, coeff);
        break;
    case FLIPADST_ADST:
        load_buffer_16x16(input, in, stride, 1, 0, shift[0]);
        fadst16x16_N2_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num);
        col_txfm_16x16_N2_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_sse4_1(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_sse4_1(out + 2, in + 32); //top-right ->bottom-left
        fadst16x16_N2_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, col_num / 2);
        transpose_8x8_in_16x16_sse4_1(out, in);
        write_buffer_16x16_N2(in, coeff);
        break;
    case V_DCT:
        load_buffer_16x16_N2(input, in, stride, 0, 0, shift[0]);
        fdct16x16_N2_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num / 2);
        col_txfm_16x16_N2_half_rounding(out, -shift[1]);
        fidtx16x16_N2_row_sse4_1(out, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        write_buffer_16x16_N2(out, coeff);
        break;
    case H_DCT:
        load_buffer_16x16_N2_H(input, in, stride, 0, 0, shift[0]);
        fidtx8xn_N2_col_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 16);
        col_txfm_16x16_N2_rounding(in, -shift[1]);
        transpose_8x8_in_16x16_sse4_1(in, out); //top-left -> top-left
        transpose_8x8_in_16x16_sse4_1(in + 2, out + 32); //top-right ->bottom-left
        fdct16x16_N2_sse4_1(out, in, fwd_cos_bit_row[txw_idx][txh_idx], col_num, col_num / 2);
        transpose_8x8_in_16x16_sse4_1(in, out);
        write_buffer_16x16_N2(out, coeff);
        break;
    case V_ADST:
        load_buffer_16x16_N2(input, in, stride, 0, 0, shift[0]);
        fadst16x16_N2_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num / 2);
        col_txfm_16x16_N2_half_rounding(out, -shift[1]);
        fidtx16x16_N2_row_sse4_1(out, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        write_buffer_16x16_N2(out, coeff);
        break;
    case H_ADST:
        load_buffer_16x16_N2_H(input, in, stride, 0, 0, shift[0]);
        fidtx8xn_N2_col_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 16);
        col_txfm_16x16_N2_rounding(in, -shift[1]);
        transpose_8x8_in_16x16_sse4_1(in, out); //top-left -> top-left
        transpose_8x8_in_16x16_sse4_1(in + 2, out + 32); //top-right ->bottom-left
        fadst16x16_N2_sse4_1(out, in, fwd_cos_bit_row[txw_idx][txh_idx], col_num, col_num / 2);
        transpose_8x8_in_16x16_sse4_1(in, out);
        write_buffer_16x16_N2(out, coeff);
        break;
    case V_FLIPADST:
        load_buffer_16x16_N2(input, in, stride, 1, 0, shift[0]);
        fadst16x16_N2_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num / 2);
        col_txfm_16x16_N2_half_rounding(out, -shift[1]);
        fidtx16x16_N2_row_sse4_1(out, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        write_buffer_16x16_N2(out, coeff);
        break;
    case H_FLIPADST:
        load_buffer_16x16_N2_H(input, in, stride, 0, 1, shift[0]);
        fidtx8xn_N2_col_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 16);
        col_txfm_16x16_N2_rounding(in, -shift[1]);
        transpose_8x8_in_16x16_sse4_1(in, out); //top-left -> top-left
        transpose_8x8_in_16x16_sse4_1(in + 2, out + 32); //top-right ->bottom-left
        fadst16x16_N2_sse4_1(out, in, fwd_cos_bit_row[txw_idx][txh_idx], col_num, col_num / 2);
        transpose_8x8_in_16x16_sse4_1(in, out);
        write_buffer_16x16_N2(out, coeff);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_64x64_N2_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    __m128i       in[1024];
    __m128i*      out     = (__m128i*)output;
    const int32_t txw_idx = tx_size_wide_log2[TX_64X64] - tx_size_wide_log2[0];
    const int32_t txh_idx = tx_size_high_log2[TX_64X64] - tx_size_high_log2[0];
    const int8_t* shift   = fwd_txfm_shift_ls[TX_64X64];

    switch (tx_type) {
    case IDTX:
        load_buffer_32x32_in_64x64_sse4_1(input, stride, out);
        fidtx64x64_N2_sse4_1(out, in);
        av1_round_shift_array_64_N2_sse4_1(in, in, 1024 / 2, -shift[1]);
        /*row wise transform*/
        fidtx64x64_N2_sse4_1(in, out);
        av1_round_shift_array_64_N2_sse4_1(out, out, 1024 / 2, -shift[2]);
        clear_buffer_wxh_N2(out, 8, 64);
        break;
    case DCT_DCT:
        load_buffer_64x64_sse4_1(input, stride, out);
        av1_fdct64_new_N2_sse4_1(out, in, fwd_cos_bit_col[txw_idx][txh_idx], 64, 8);
        av1_round_shift_array_32_sse4_1(in, out, 1024 / 2, -shift[1]);
        transpose_32x32_in_64x64_sse4_1(out, in); //top-left
        transpose_32x32_in_64x64_sse4_1(out + 8, in + 512); //top-right -> bottom left
        /*row wise transform*/
        av1_fdct64_new_N2_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 32, 8);
        av1_round_shift_array_64_N2_sse4_1(out, in, 1024 / 2, -shift[2]);
        transpose_32x32_in_64x64_sse4_1(in, out); //top-left
        clear_buffer_wxh_N2(out, 8, 64);
        break;
    default:
        assert(0);
    }
}

void svt_av1_fwd_txfm2d_32x32_N2_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    const int8_t* shift           = fwd_txfm_shift_ls[TX_32X32];
    const int32_t txw_idx         = tx_size_wide_log2[TX_32X32] - tx_size_wide_log2[0];
    const int32_t txh_idx         = tx_size_high_log2[TX_32X32] - tx_size_high_log2[0];
    const int8_t  cos_bit_col     = fwd_cos_bit_col[txw_idx][txh_idx];
    const int8_t  cos_bit_row     = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t txfm2d_size_128 = 32 * 32 / 4;
    __m128i       buf_128[256];
    __m128i*      out_128 = (__m128i*)output;
    (void)bd;

    switch (tx_type) {
    case IDTX:
        load_buffer_32x16_N2_sse4_1(input, buf_128, stride);
        av1_round_shift_array_32_sse4_1(buf_128, out_128, txfm2d_size_128 / 2, -shift[0]);
        fidtx32x32_N2_row_sse4_1(out_128, buf_128, cos_bit_col);
        av1_round_shift_array_32_sse4_1(buf_128, out_128, txfm2d_size_128 / 2, -shift[1]);
        fidtx32x32_N2_row_sse4_1(out_128, buf_128, cos_bit_row);
        av1_round_shift_array_32_N2_sse4_1(buf_128, out_128, txfm2d_size_128 / 4, -shift[2]);
        clear_buffer_wxh_N2(out_128, 4, 32);
        break;
    case DCT_DCT:
        load_buffer_32x32_sse4_1(input, buf_128, stride);
        av1_round_shift_array_32_sse4_1(buf_128, out_128, txfm2d_size_128, -shift[0]);
        fdct32x32_N2_col_sse4_1(out_128, buf_128, cos_bit_col);
        av1_round_shift_array_32_sse4_1(buf_128, out_128, txfm2d_size_128 / 2, -shift[1]);
        transpose_16x16_in_32x32_sse4_1(out_128, buf_128); //top-left -> top-left
        transpose_16x16_in_32x32_sse4_1(out_128 + 4, buf_128 + 128); //top-right ->bottom-left
        fdct32x32_N2_row_sse4_1(buf_128, out_128, cos_bit_row);
        av1_round_shift_array_32_N2_sse4_1(out_128, buf_128, txfm2d_size_128 / 4, -shift[2]);
        transpose_16x16_in_32x32_sse4_1(buf_128, out_128); //transpose nonzer output
        clear_buffer_wxh_N2(out_128, 4, 32);
        break;
    case V_DCT:
        load_buffer_32x32_sse4_1(input, buf_128, stride);
        av1_round_shift_array_32_sse4_1(buf_128, out_128, txfm2d_size_128, -shift[0]);
        fdct32x32_N2_row_sse4_1(out_128, buf_128, cos_bit_col); //col
        av1_round_shift_array_32_N2_sse4_1(buf_128, out_128, txfm2d_size_128 / 4, -shift[1]);
        fidtx32x32_N2_row_sse4_1(out_128, buf_128, cos_bit_row);
        av1_round_shift_array_32_N2_sse4_1(buf_128, out_128, txfm2d_size_128 / 4, -shift[2]);
        clear_buffer_wxh_N2(out_128, 4, 32);
        break;
    case H_DCT:
        load_buffer_32x16_sse4_1(input, buf_128, stride);
        av1_round_shift_array_32_sse4_1(buf_128, out_128, txfm2d_size_128 / 2, -shift[0]);
        fidtx32x32_N2_col_sse4_1(out_128, buf_128, cos_bit_col);
        av1_round_shift_array_32_sse4_1(buf_128, out_128, txfm2d_size_128 / 2, -shift[1]);
        transpose_16x16_in_32x32_sse4_1(out_128, buf_128); //top-left -> top-left
        transpose_16x16_in_32x32_sse4_1(out_128 + 4, buf_128 + 128); //top-right ->bottom-left
        fdct32x32_N2_row_sse4_1(buf_128, out_128, cos_bit_row);
        av1_round_shift_array_32_N2_sse4_1(out_128, buf_128, txfm2d_size_128 / 4, -shift[2]);
        transpose_16x16_in_32x32_sse4_1(buf_128, out_128); //transpose nonzer output
        clear_buffer_wxh_N2(out_128, 4, 32);
        break;
    default:
        assert(0);
    }
}

void svt_av1_fwd_txfm2d_16x32_N2_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[128];
    __m128i*      outcoef128    = (__m128i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_16X32];
    const int32_t txw_idx       = get_txw_idx(TX_16X32);
    const int32_t txh_idx       = get_txh_idx(TX_16X32);
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t txfm_size_col = tx_size_wide[TX_16X32];
    const int32_t txfm_size_row = tx_size_high[TX_16X32];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    switch (tx_type) {
    case IDTX:
        load_buffer_16x16_N2(input, in, stride, 0, 0, shift[0]);
        av1_idtx16x32_N2_sse4_1(in, in);
        col_txfm_16x16_N2_half_rounding(&in[0], -shift[1]);
        col_txfm_16x16_N2_half_rounding(&in[32], -shift[1]);
        fidtx16x16_N2_row_sse4_1(in, outcoef128, bitrow, num_row);
        av1_round_shift_rect_array_wxh_sse4_1(outcoef128, outcoef128, -shift[2], new_sqrt2, num_col, txfm_size_row);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case DCT_DCT:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        load_buffer_16x16(input + 16 * stride, in + 64, stride, 0, 0, shift[0]);
        for (int32_t i = 0; i < num_col; i++) {
            av1_fdct32_new_N2_sse4_1((in + 2 * i), (in + 2 * i), bitcol, 8, num_col);
        }
        col_txfm_16x16_rounding(&in[0], -shift[1]);
        transpose_8nx8n_N2_half_sse4_1(in, outcoef128, txfm_size_col, txfm_size_row);
        fdct16x16_N2_sse4_1(outcoef128, in, bitrow, num_row, num_row / 2);
        transpose_8nx8n_N2_quad_sse4_1(in, outcoef128, txfm_size_row, txfm_size_col);
        av1_round_shift_rect_array_wxh_sse4_1(outcoef128, outcoef128, -shift[2], new_sqrt2, num_col, txfm_size_row);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    default:
        assert(0);
    }

    (void)bd;
}

/* call this function only for IDTX */
void svt_av1_fwd_txfm2d_32x16_N2_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[128];
    __m128i*      outcoef128    = (__m128i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_32X16];
    const int32_t txw_idx       = get_txw_idx(TX_32X16);
    const int32_t txh_idx       = get_txh_idx(TX_32X16);
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t txfm_size_col = tx_size_wide[TX_32X16];
    const int32_t txfm_size_row = tx_size_high[TX_32X16];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    switch (tx_type) {
    case IDTX:
        load_buffer_16x8n(input, in, stride, 0, 0, shift[0], txfm_size_row / 2);
        fidtx32x16_N2_sse4_1(in, in, bitcol, txfm_size_row / 2);
        col_txfm_32x16_N2_rounding(&in[0], -shift[1]);
        col_txfm_32x16_N2_rounding(&in[2], -shift[1]);
        av1_idtx32x16_N2_sse4_1(in, outcoef128, txfm_size_row / 2);
        av1_round_shift_rect_array_wxh_sse4_1(outcoef128, outcoef128, -shift[2], new_sqrt2, num_col, txfm_size_row);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case DCT_DCT:
        load_buffer_32x8n(input, in, stride, 0, 0, shift[0], txfm_size_row);
        fdct16x16_N2_sse4_1(in, in, bitcol, num_col, num_col);
        col_txfm_16x16_rounding(&in[0], -shift[1]);
        transpose_8nx8n_N2_half_sse4_1(in, outcoef128, txfm_size_col, txfm_size_row);
        av1_fdct32_new_N2_sse4_1(outcoef128, in, bitrow, 8, num_row);
        transpose_8nx8n_N2_quad_sse4_1(in, outcoef128, txfm_size_row, txfm_size_col);
        av1_round_shift_rect_array_wxh_sse4_1(outcoef128, outcoef128, -shift[2], new_sqrt2, num_col, txfm_size_row);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    default:
        assert(0);
    }

    (void)bd;
}

/* call this function only for DCT_DCT, IDTX */
void svt_av1_fwd_txfm2d_8x32_N2_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[64];
    __m128i*      outcoef128 = (__m128i*)output;
    const int8_t* shift      = fwd_txfm_shift_ls[TX_8X32];
    const int32_t txw_idx    = get_txw_idx(TX_8X32);
    const int32_t txh_idx    = get_txh_idx(TX_8X32);
    int8_t        bitcol     = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow     = fwd_cos_bit_row[txw_idx][txh_idx];

    const int32_t txfm_size_col = tx_size_wide[TX_8X32];
    const int32_t txfm_size_row = tx_size_high[TX_8X32];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    switch (tx_type) {
    case IDTX:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        av1_idtx16_new_sse4_1(in, in, bitcol, num_col);
        col_txfm_16x16_N2_rounding(in, -shift[1]);
        // row transform
        fidtx32x8_N2_sse4_1(in, outcoef128, bitrow, num_col, 16);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case DCT_DCT:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        load_buffer_8x16(input + (txfm_size_row >> 1) * stride, in + 32, stride, 0, 0, shift[0]);
        av1_fdct32_new_N2_sse4_1(in, in, bitcol, 8, num_col);
        col_txfm_16x16_N2_rounding(in, -shift[1]);
        transpose_8nx8n_N2_half_sse4_1(in, outcoef128, txfm_size_col, txfm_size_row);
        // row transform
        fdct8x8_N2_sse4_1(outcoef128, in, bitrow, num_row);
        fdct8x8_N2_sse4_1(outcoef128 + 2, in + 2, bitrow, num_row);
        transpose_8nx8n_N2_quad_sse4_1(in, outcoef128, txfm_size_row, txfm_size_col);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    default:
        assert(0);
    }

    (void)bd;
}

/* call this function only for DCT_DCT, IDTX */
void svt_av1_fwd_txfm2d_32x8_N2_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[64];
    __m128i*      outcoef128 = (__m128i*)output;
    const int8_t* shift      = fwd_txfm_shift_ls[TX_32X8];
    const int32_t txw_idx    = get_txw_idx(TX_32X8);
    const int32_t txh_idx    = get_txh_idx(TX_32X8);
    int8_t        bitcol     = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow     = fwd_cos_bit_row[txw_idx][txh_idx];

    const int32_t txfm_size_col = tx_size_wide[TX_32X8];
    const int32_t txfm_size_row = tx_size_high[TX_32X8];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    (void)bd;

    switch (tx_type) {
    case IDTX:
        load_buffer_16x8n(input, in, stride, 0, 0, shift[0], txfm_size_row / 2);
        fidtx32x8_N2_sse4_1(in, in, bitcol, num_col, 4);
        fidtx32x8_N2_sse4_1(in + 2, in + 2, bitcol, num_col, 4);
        col_txfm_32x8_N2_half_rounding(&in[0], -shift[1]);
        // row transform
        av1_idtx32_new_N2_sse4_1(in, outcoef128, bitrow, num_row, 16);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case DCT_DCT:
        load_buffer_32x8n(input, in, stride, 0, 0, shift[0], txfm_size_row);
        for (int32_t i = 0; i < num_col; i++) {
            fdct8x8_N2_sse4_1((in + 2 * i), (in + 2 * i), bitcol, num_col);
        }
        col_txfm_16x16_N2_rounding(&in[0], -shift[1]);
        transpose_8nx8n_N2_half_sse4_1(in, outcoef128, txfm_size_col, txfm_size_row);
        // row transform
        av1_fdct32_new_N2_sse4_1(outcoef128, in, bitrow, 8, num_row);
        transpose_8nx8n_N2_quad_sse4_1(in, outcoef128, txfm_size_row, txfm_size_col);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    default:
        assert(0);
    }
}

/* call this function for all 16 transform types */
void svt_av1_fwd_txfm2d_8x16_N2_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[32], out[32];
    __m128i*      outcoef128    = (__m128i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_8X16];
    const int32_t txw_idx       = get_txw_idx(TX_8X16);
    const int32_t txh_idx       = get_txh_idx(TX_8X16);
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t txfm_size_col = tx_size_wide[TX_8X16];
    const int32_t txfm_size_row = tx_size_high[TX_8X16];
    const int32_t num_col       = txfm_size_col >> 3;

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        fdct16x16_N2_sse4_1(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_sse4_1(in, out);
        fdct8x8_N2_sse4_1(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, out, 8, -shift[2]);
        transpose_8x8_half_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case ADST_DCT:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        fadst16x16_N2_sse4_1(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_sse4_1(in, out);
        fdct8x8_N2_sse4_1(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, out, 8, -shift[2]);
        transpose_8x8_half_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case DCT_ADST:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        fdct16x16_N2_sse4_1(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_sse4_1(in, out);
        fadst8x8_N2_sse4_1(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, out, 8, -shift[2]);
        transpose_8x8_half_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case ADST_ADST:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        fadst16x16_N2_sse4_1(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_sse4_1(in, out);
        fadst8x8_N2_sse4_1(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, out, 8, -shift[2]);
        transpose_8x8_half_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case FLIPADST_DCT:
        load_buffer_8x16(input, in, stride, 1, 0, shift[0]);
        fadst16x16_N2_sse4_1(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_sse4_1(in, out);
        fdct8x8_N2_sse4_1(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, out, 8, -shift[2]);
        transpose_8x8_half_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case DCT_FLIPADST:
        load_buffer_8x16(input, in, stride, 0, 1, shift[0]);
        fdct16x16_N2_sse4_1(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_sse4_1(in, out);
        fadst8x8_N2_sse4_1(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, out, 8, -shift[2]);
        transpose_8x8_half_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_8x16(input, in, stride, 1, 1, shift[0]);
        fadst16x16_N2_sse4_1(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_sse4_1(in, out);
        fadst8x8_N2_sse4_1(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, out, 8, -shift[2]);
        transpose_8x8_half_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case ADST_FLIPADST:
        load_buffer_8x16(input, in, stride, 0, 1, shift[0]);
        fadst16x16_N2_sse4_1(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_sse4_1(in, out);
        fadst8x8_N2_sse4_1(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, out, 8, -shift[2]);
        transpose_8x8_half_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case FLIPADST_ADST:
        load_buffer_8x16(input, in, stride, 1, 0, shift[0]);
        fadst16x16_N2_sse4_1(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_sse4_1(in, out);
        fadst8x8_N2_sse4_1(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, out, 8, -shift[2]);
        transpose_8x8_half_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case IDTX:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fidtx8xn_N2_col_sse4_1(in, in, bitcol, 8);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_sse4_1(in, out);
        fidtx8x8_N2_sse4_1(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, out, 8, -shift[2]);
        transpose_8x8_half_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case V_DCT:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        fdct16x16_N2_sse4_1(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_sse4_1(in, out);
        fidtx8x8_N2_sse4_1(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, out, 8, -shift[2]);
        transpose_8x8_half_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case H_DCT:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fidtx8xn_N2_col_sse4_1(in, in, bitcol, 8);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_sse4_1(in, out);
        fdct8x8_N2_sse4_1(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, out, 8, -shift[2]);
        transpose_8x8_half_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case V_ADST:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        fadst16x16_N2_sse4_1(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_sse4_1(in, out);
        fidtx8x8_N2_sse4_1(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, out, 8, -shift[2]);
        transpose_8x8_half_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case H_ADST:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fidtx8xn_N2_col_sse4_1(in, in, bitcol, 8);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_sse4_1(in, out);
        fadst8x8_N2_sse4_1(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, out, 8, -shift[2]);
        transpose_8x8_half_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case V_FLIPADST:
        load_buffer_8x16(input, in, stride, 1, 0, shift[0]);
        fadst16x16_N2_sse4_1(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_sse4_1(in, out);
        fidtx8x8_N2_sse4_1(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, out, 8, -shift[2]);
        transpose_8x8_half_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case H_FLIPADST:
        load_buffer_8x8(input, in, stride, 0, 1, shift[0]);
        fidtx8xn_N2_col_sse4_1(in, in, bitcol, 8);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_sse4_1(in, out);
        fadst8x8_N2_sse4_1(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, out, 8, -shift[2]);
        transpose_8x8_half_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    default:
        assert(0);
    }

    (void)bd;
}

/* call this function for all 16 transform types */
void svt_av1_fwd_txfm2d_16x8_N2_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[32], out[32] = {_mm_setzero_si128()};
    __m128i*      outcoef128    = (__m128i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_16X8];
    const int32_t txw_idx       = get_txw_idx(TX_16X8);
    const int32_t txh_idx       = get_txh_idx(TX_16X8);
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t txfm_size_col = tx_size_wide[TX_16X8];
    const int32_t txfm_size_row = tx_size_high[TX_16X8];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;
    assert(num_col > 0);

    switch (tx_type) {
    case DCT_DCT:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fdct8x8_N2_sse4_1(in, in, bitcol, 1);
            col_txfm_8x8_N2_rounding(in, -shift[1]);
            transpose_8x8_half_sse4_1(in, out + i * 16);
        }
        fdct16x16_N2_sse4_1(out, out, bitrow, num_row, 1);
        transpose_8x8_sse4_1(out, in);
        av1_round_shift_rect_array_32_sse4_1(in, out, 8, -shift[2]);
        write_buffer_16x8_N2_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case ADST_DCT:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fadst8x8_N2_sse4_1(in, in, bitcol, 1);
            col_txfm_8x8_N2_rounding(in, -shift[1]);
            transpose_8x8_half_sse4_1(in, out + i * 16);
        }
        fdct16x16_N2_sse4_1(out, out, bitrow, num_row, 1);
        transpose_8x8_sse4_1(out, in);
        av1_round_shift_rect_array_32_sse4_1(in, out, 8, -shift[2]);
        write_buffer_16x8_N2_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case DCT_ADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fdct8x8_N2_sse4_1(in, in, bitcol, 1);
            col_txfm_8x8_N2_rounding(in, -shift[1]);
            transpose_8x8_half_sse4_1(in, out + i * 16);
        }
        fadst16x16_N2_sse4_1(out, out, bitrow, num_row, 1);
        transpose_8x8_sse4_1(out, in);
        av1_round_shift_rect_array_32_sse4_1(in, out, 8, -shift[2]);
        write_buffer_16x8_N2_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case ADST_ADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fadst8x8_N2_sse4_1(in, in, bitcol, 1);
            col_txfm_8x8_N2_rounding(in, -shift[1]);
            transpose_8x8_half_sse4_1(in, out + i * 16);
        }
        fadst16x16_N2_sse4_1(out, out, bitrow, num_row, 1);
        transpose_8x8_sse4_1(out, in);
        av1_round_shift_rect_array_32_sse4_1(in, out, 8, -shift[2]);
        write_buffer_16x8_N2_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case FLIPADST_DCT:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 1, 0, shift[0]);
            fadst8x8_N2_sse4_1(in, in, bitcol, 1);
            col_txfm_8x8_N2_rounding(in, -shift[1]);
            transpose_8x8_half_sse4_1(in, out + i * 16);
        }
        fdct16x16_N2_sse4_1(out, out, bitrow, num_row, 1);
        transpose_8x8_sse4_1(out, in);
        av1_round_shift_rect_array_32_sse4_1(in, out, 8, -shift[2]);
        write_buffer_16x8_N2_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case DCT_FLIPADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fdct8x8_N2_sse4_1(in, in, bitcol, 1);
            col_txfm_8x8_N2_rounding(in, -shift[1]);
            transpose_8x8_half_sse4_1(in, out + i * 16);
        }
        for (int32_t i = 0; i < 16; i++) {
            in[32 - 2 * i - 2] = out[2 * i];
            in[32 - 2 * i - 1] = out[2 * i + 1];
        }
        fadst16x16_N2_sse4_1(in, out, bitrow, num_row, 1);
        transpose_8x8_sse4_1(out, in);
        av1_round_shift_rect_array_32_sse4_1(in, out, 8, -shift[2]);
        write_buffer_16x8_N2_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case FLIPADST_FLIPADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 1, 0, shift[0]);
            fadst8x8_N2_sse4_1(in, in, bitcol, 1);
            col_txfm_8x8_N2_rounding(in, -shift[1]);
            transpose_8x8_half_sse4_1(in, out + i * 16);
        }
        for (int32_t i = 0; i < 16; i++) {
            in[32 - 2 * i - 2] = out[2 * i];
            in[32 - 2 * i - 1] = out[2 * i + 1];
        }
        fadst16x16_N2_sse4_1(in, out, bitrow, num_row, 1);
        transpose_8x8_sse4_1(out, in);
        av1_round_shift_rect_array_32_sse4_1(in, out, 8, -shift[2]);
        write_buffer_16x8_N2_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case ADST_FLIPADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fadst8x8_N2_sse4_1(in, in, bitcol, 1);
            col_txfm_8x8_N2_rounding(in, -shift[1]);
            transpose_8x8_half_sse4_1(in, out + i * 16);
        }
        for (int32_t i = 0; i < 16; i++) {
            in[32 - 2 * i - 2] = out[2 * i];
            in[32 - 2 * i - 1] = out[2 * i + 1];
        }
        fadst16x16_N2_sse4_1(in, out, bitrow, num_row, 1);
        transpose_8x8_sse4_1(out, in);
        av1_round_shift_rect_array_32_sse4_1(in, out, 8, -shift[2]);
        write_buffer_16x8_N2_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case FLIPADST_ADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 1, 0, shift[0]);
            fadst8x8_N2_sse4_1(in, in, bitcol, 1);
            col_txfm_8x8_N2_rounding(in, -shift[1]);
            transpose_8x8_half_sse4_1(in, out + i * 16);
        }
        fadst16x16_N2_sse4_1(out, out, bitrow, num_row, 1);
        transpose_8x8_sse4_1(out, in);
        av1_round_shift_rect_array_32_sse4_1(in, out, 8, -shift[2]);
        write_buffer_16x8_N2_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case IDTX:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fidtx8x8_N2_sse4_1(in, out, bitcol, 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        fidtx8xn_N2_col_sse4_1(out, out, bitrow, 4);
        av1_round_shift_rect_array_32_sse4_1(out, out, 8, -shift[2]);
        write_buffer_16x8_N2_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case V_DCT:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fdct8x8_N2_sse4_1(in, out, bitcol, 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        fidtx8xn_N2_col_sse4_1(out, out, bitrow, 4);
        av1_round_shift_rect_array_32_sse4_1(out, out, 8, -shift[2]);
        write_buffer_16x8_N2_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case H_DCT:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fidtx8x8_N2_sse4_1(in, in, bitcol, 1);
            col_txfm_8x8_N2_rounding(in, -shift[1]);
            transpose_8x8_half_sse4_1(in, out + i * 16);
        }
        fdct16x16_N2_sse4_1(out, out, bitrow, num_row, 1);
        transpose_8x8_sse4_1(out, in);
        av1_round_shift_rect_array_32_sse4_1(in, out, 8, -shift[2]);
        write_buffer_16x8_N2_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case V_ADST:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fadst8x8_N2_sse4_1(in, out, bitcol, 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        fidtx8xn_N2_col_sse4_1(out, out, bitrow, 4);
        av1_round_shift_rect_array_32_sse4_1(out, out, 8, -shift[2]);
        write_buffer_16x8_N2_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case H_ADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fidtx8x8_N2_sse4_1(in, in, bitcol, 1);
            col_txfm_8x8_N2_rounding(in, -shift[1]);
            transpose_8x8_half_sse4_1(in, out + i * 16);
        }
        fadst16x16_N2_sse4_1(out, out, bitrow, num_row, 1);
        transpose_8x8_sse4_1(out, in);
        av1_round_shift_rect_array_32_sse4_1(in, out, 8, -shift[2]);
        write_buffer_16x8_N2_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case V_FLIPADST:
        load_buffer_8x8(input, in, stride, 1, 0, shift[0]);
        fadst8x8_N2_sse4_1(in, out, bitcol, 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        fidtx8xn_N2_col_sse4_1(out, out, bitrow, 4);
        av1_round_shift_rect_array_32_sse4_1(out, out, 8, -shift[2]);
        write_buffer_16x8_N2_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    case H_FLIPADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fidtx8x8_N2_sse4_1(in, in, bitcol, 1);
            col_txfm_8x8_N2_rounding(in, -shift[1]);
            transpose_8x8_half_sse4_1(in, out + i * 16);
        }
        for (int32_t i = 0; i < 16; i++) {
            in[32 - 2 * i - 2] = out[2 * i];
            in[32 - 2 * i - 1] = out[2 * i + 1];
        }
        fadst16x16_N2_sse4_1(in, out, bitrow, num_row, 1);
        transpose_8x8_sse4_1(out, in);
        av1_round_shift_rect_array_32_sse4_1(in, out, 8, -shift[2]);
        write_buffer_16x8_N2_sse4_1(out, outcoef128);
        clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
        break;
    default:
        assert(0);
    }

    (void)bd;
}

void svt_av1_fwd_txfm2d_4x8_N2_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i in[8];
    __m128i outcoeff128[8];

    const int8_t* shift   = fwd_txfm_shift_ls[TX_4X8];
    const int32_t txw_idx = get_txw_idx(TX_4X8);
    const int32_t txh_idx = get_txh_idx(TX_4X8);
    int32_t       bitcol  = fwd_cos_bit_col[txw_idx][txh_idx];
    int32_t       bitrow  = fwd_cos_bit_row[txw_idx][txh_idx];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_4x8_sse4_1(input, in, stride, 0, 0, shift[0]);
        fdct4x8_N2_sse4_1(in, in, bitcol);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        transpose_4x8_sse4_1(in, outcoeff128);
        fdct4x8_col_N2_sse4_1(outcoeff128, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        write_buffer_4x8_N2(outcoeff128, output);
        break;
    case ADST_DCT:
        load_buffer_4x8_sse4_1(input, in, stride, 0, 0, shift[0]);
        fadst8x4_N2_sse4_1(in, in, bitcol, 1);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        transpose_4x8_sse4_1(in, outcoeff128);
        fdct4x8_col_N2_sse4_1(outcoeff128, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        write_buffer_4x8_N2(outcoeff128, output);
        break;
    case DCT_ADST:
        load_buffer_4x8_sse4_1(input, in, stride, 0, 0, shift[0]);
        fdct4x8_N2_sse4_1(in, in, bitcol);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        transpose_4x8_sse4_1(in, outcoeff128);
        fadst4x8_col_N2_sse4_1(outcoeff128, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        write_buffer_4x8_N2(outcoeff128, output);
        break;
    case ADST_ADST:
        load_buffer_4x8_sse4_1(input, in, stride, 0, 0, shift[0]);
        fadst8x4_N2_sse4_1(in, in, bitcol, 1);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        transpose_4x8_sse4_1(in, outcoeff128);
        fadst4x8_col_N2_sse4_1(outcoeff128, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        write_buffer_4x8_N2(outcoeff128, output);
        break;
    case FLIPADST_DCT:
        load_buffer_4x8_sse4_1(input, in, stride, 1, 0, shift[0]);
        fadst8x4_N2_sse4_1(in, in, bitcol, 1);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        transpose_4x8_sse4_1(in, outcoeff128);
        fdct4x8_col_N2_sse4_1(outcoeff128, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        write_buffer_4x8_N2(outcoeff128, output);
        break;
    case DCT_FLIPADST:
        load_buffer_4x8_sse4_1(input, in, stride, 0, 1, shift[0]);
        fdct4x8_N2_sse4_1(in, in, bitcol);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        transpose_4x8_sse4_1(in, outcoeff128);
        fadst4x8_col_N2_sse4_1(outcoeff128, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        write_buffer_4x8_N2(outcoeff128, output);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_4x8_sse4_1(input, in, stride, 1, 1, shift[0]);
        fadst8x4_N2_sse4_1(in, in, bitcol, 1);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        transpose_4x8_sse4_1(in, outcoeff128);
        fadst4x8_col_N2_sse4_1(outcoeff128, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        write_buffer_4x8_N2(outcoeff128, output);
        break;
    case ADST_FLIPADST:
        load_buffer_4x8_sse4_1(input, in, stride, 0, 1, shift[0]);
        fadst8x4_N2_sse4_1(in, in, bitcol, 1);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        transpose_4x8_sse4_1(in, outcoeff128);
        fadst4x8_col_N2_sse4_1(outcoeff128, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        write_buffer_4x8_N2(outcoeff128, output);
        break;
    case FLIPADST_ADST:
        load_buffer_4x8_sse4_1(input, in, stride, 1, 0, shift[0]);
        fadst8x4_N2_sse4_1(in, in, bitcol, 1);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        transpose_4x8_sse4_1(in, outcoeff128);
        fadst4x8_col_N2_sse4_1(outcoeff128, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        write_buffer_4x8_N2(outcoeff128, output);
        break;
    case IDTX:
        load_buffer_4x8_sse4_1(input, in, stride, 0, 0, shift[0]);
        fidtx8x4_N2_sse4_1(in, in, bitcol);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        fidtx4x8_col_N2_sse4_1(in, in, bitrow, 2);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        write_buffer_4x8_N2(outcoeff128, output);
        break;
    case V_DCT:
        load_buffer_4x8_sse4_1(input, in, stride, 0, 0, shift[0]);
        fdct4x8_N2_sse4_1(in, in, bitcol);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        fidtx4x8_col_N2_sse4_1(in, in, bitrow, 2);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        write_buffer_4x8_N2(outcoeff128, output);
        break;
    case H_DCT:
        load_buffer_4x8_sse4_1(input, in, stride, 0, 0, shift[0]);
        fidtx8x4_N2_sse4_1(in, in, bitcol);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        transpose_4x8_sse4_1(in, outcoeff128);
        fdct4x8_col_N2_sse4_1(outcoeff128, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        write_buffer_4x8_N2(outcoeff128, output);
        break;
    case V_ADST:
        load_buffer_4x8_sse4_1(input, in, stride, 0, 0, shift[0]);
        fadst8x4_N2_sse4_1(in, in, bitcol, 1);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        fidtx4x8_col_N2_sse4_1(in, in, bitrow, 2);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        write_buffer_4x8_N2(outcoeff128, output);
        break;
    case H_ADST:
        load_buffer_4x8_sse4_1(input, in, stride, 0, 0, shift[0]);
        fidtx8x4_N2_sse4_1(in, in, bitcol);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        transpose_4x8_sse4_1(in, outcoeff128);
        fadst4x8_col_N2_sse4_1(outcoeff128, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        write_buffer_4x8_N2(outcoeff128, output);
        break;
    case V_FLIPADST:
        load_buffer_4x8_sse4_1(input, in, stride, 1, 0, shift[0]);
        fadst8x4_N2_sse4_1(in, in, bitcol, 1);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        fidtx4x8_col_N2_sse4_1(in, in, bitrow, 2);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        write_buffer_4x8_N2(outcoeff128, output);
        break;
    case H_FLIPADST:
        load_buffer_4x8_sse4_1(input, in, stride, 0, 1, shift[0]);
        fidtx8x4_N2_sse4_1(in, in, bitcol);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        transpose_4x8_sse4_1(in, outcoeff128);
        fadst4x8_col_N2_sse4_1(outcoeff128, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        write_buffer_4x8_N2(outcoeff128, output);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_8x4_N2_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[8];
    __m128i*      outcoeff128 = (__m128i*)output;
    const int8_t* shift       = fwd_txfm_shift_ls[TX_8X4];
    const int32_t txw_idx     = get_txw_idx(TX_8X4);
    const int32_t txh_idx     = get_txh_idx(TX_8X4);
    int32_t       bitcol      = fwd_cos_bit_col[txw_idx][txh_idx];
    int32_t       bitrow      = fwd_cos_bit_row[txw_idx][txh_idx];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_8x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fdct4x8_row_N2_with_round_sse4_1(in, in, bitcol, 1, -shift[1]);
        fdct4x8_N2_sse4_1(in, outcoeff128, bitrow);
        transpose_4x8_sse4_1(outcoeff128, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        clear_buffer_wxh_N2(outcoeff128, 1, 4);
        break;
    case ADST_DCT:
        load_buffer_8x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fadst4x8_row_N2_with_round_sse4_1(in, in, bitcol, 1, -shift[1]);
        fdct4x8_N2_sse4_1(in, outcoeff128, bitrow);
        transpose_4x8_sse4_1(outcoeff128, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        clear_buffer_wxh_N2(outcoeff128, 1, 4);
        break;
    case DCT_ADST:
        load_buffer_8x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fdct4x8_row_N2_with_round_sse4_1(in, in, bitcol, 1, -shift[1]);
        fadst8x4_N2_sse4_1(in, outcoeff128, bitrow, 1);
        transpose_4x8_sse4_1(outcoeff128, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        clear_buffer_wxh_N2(outcoeff128, 1, 4);
        break;
    case ADST_ADST:
        load_buffer_8x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fadst4x8_row_N2_with_round_sse4_1(in, in, bitcol, 1, -shift[1]);
        fadst8x4_N2_sse4_1(in, outcoeff128, bitrow, 1);
        transpose_4x8_sse4_1(outcoeff128, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        clear_buffer_wxh_N2(outcoeff128, 1, 4);
        break;
    case FLIPADST_DCT:
        load_buffer_8x4_sse4_1(input, in, stride, 1, 0, shift[0]);
        fadst4x8_row_N2_with_round_sse4_1(in, in, bitcol, 1, -shift[1]);
        fdct4x8_N2_sse4_1(in, outcoeff128, bitrow);
        transpose_4x8_sse4_1(outcoeff128, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        clear_buffer_wxh_N2(outcoeff128, 1, 4);
        break;
    case DCT_FLIPADST:
        load_buffer_8x4_sse4_1(input, in, stride, 0, 1, shift[0]);
        fdct4x8_row_N2_with_round_sse4_1(in, in, bitcol, 1, -shift[1]);
        fadst8x4_N2_sse4_1(in, outcoeff128, bitrow, 1);
        transpose_4x8_sse4_1(outcoeff128, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        clear_buffer_wxh_N2(outcoeff128, 1, 4);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_8x4_sse4_1(input, in, stride, 1, 1, shift[0]);
        fadst4x8_row_N2_with_round_sse4_1(in, in, bitcol, 1, -shift[1]);
        fadst8x4_N2_sse4_1(in, outcoeff128, bitrow, 1);
        transpose_4x8_sse4_1(outcoeff128, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        clear_buffer_wxh_N2(outcoeff128, 1, 4);
        break;
    case ADST_FLIPADST:
        load_buffer_8x4_sse4_1(input, in, stride, 0, 1, shift[0]);
        fadst4x8_row_N2_with_round_sse4_1(in, in, bitcol, 1, -shift[1]);
        fadst8x4_N2_sse4_1(in, outcoeff128, bitrow, 1);
        transpose_4x8_sse4_1(outcoeff128, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        clear_buffer_wxh_N2(outcoeff128, 1, 4);
        break;
    case FLIPADST_ADST:
        load_buffer_8x4_sse4_1(input, in, stride, 1, 0, shift[0]);
        fadst4x8_row_N2_with_round_sse4_1(in, in, bitcol, 1, -shift[1]);
        fadst8x4_N2_sse4_1(in, outcoeff128, bitrow, 1);
        transpose_4x8_sse4_1(outcoeff128, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        clear_buffer_wxh_N2(outcoeff128, 1, 4);
        break;
    case IDTX:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fidtx4x8_row_N2_with_round_sse4_1(in, in, bitcol, -shift[1]);
        fidtx8x4_N2_sse4_1(in, outcoeff128, bitrow);
        transpose_4x8_sse4_1(outcoeff128, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        clear_buffer_wxh_N2(outcoeff128, 1, 4);
        break;
    case V_DCT:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fdct4x8_row_N2_with_round_sse4_1(in, in, bitcol, 1, -shift[1]);
        fidtx8x4_N2_sse4_1(in, outcoeff128, bitrow);
        transpose_4x8_sse4_1(outcoeff128, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        clear_buffer_wxh_N2(outcoeff128, 1, 4);
        break;
    case H_DCT:
        load_buffer_8x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fidtx4x8_row_N2_with_round_sse4_1(in, in, bitcol, -shift[1]);
        fdct4x8_N2_sse4_1(in, outcoeff128, bitrow);
        transpose_4x8_sse4_1(outcoeff128, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        clear_buffer_wxh_N2(outcoeff128, 1, 4);
        break;
    case V_ADST:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fadst4x8_row_N2_with_round_sse4_1(in, in, bitcol, 1, -shift[1]);
        fidtx8x4_N2_sse4_1(in, outcoeff128, bitrow);
        transpose_4x8_sse4_1(outcoeff128, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        clear_buffer_wxh_N2(outcoeff128, 1, 4);
        break;
    case H_ADST:
        load_buffer_8x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fidtx4x8_row_N2_with_round_sse4_1(in, in, bitcol, -shift[1]);
        fadst8x4_N2_sse4_1(in, outcoeff128, bitrow, 1);
        transpose_4x8_sse4_1(outcoeff128, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        clear_buffer_wxh_N2(outcoeff128, 1, 4);
        break;
    case V_FLIPADST:
        load_buffer_4x4(input, in, stride, 1, 0, shift[0]);
        fadst4x8_row_N2_with_round_sse4_1(in, in, bitcol, 1, -shift[1]);
        fidtx8x4_N2_sse4_1(in, outcoeff128, bitrow);
        transpose_4x8_sse4_1(outcoeff128, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        clear_buffer_wxh_N2(outcoeff128, 1, 4);
        break;
    case H_FLIPADST:
        load_buffer_8x4_sse4_1(input, in, stride, 0, 1, shift[0]);
        fidtx4x8_row_N2_with_round_sse4_1(in, in, bitcol, -shift[1]);
        fadst8x4_N2_sse4_1(in, outcoeff128, bitrow, 1);
        transpose_4x8_sse4_1(outcoeff128, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff128, 4, -shift[2]);
        clear_buffer_wxh_N2(outcoeff128, 1, 4);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_4x16_N2_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[16];
    __m128i*      outcoeff128 = (__m128i*)output;
    const int8_t* shift       = fwd_txfm_shift_ls[TX_4X16];
    const int32_t txw_idx     = get_txw_idx(TX_4X16);
    const int32_t txh_idx     = get_txh_idx(TX_4X16);
    int32_t       bitcol      = fwd_cos_bit_col[txw_idx][txh_idx];
    int32_t       bitrow      = fwd_cos_bit_row[txw_idx][txh_idx];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_4x16_sse4_1(input, in, stride, 0, 0, shift[0]);
        fdct16x4_N2_sse4_1(in, outcoeff128, bitcol);
        col_txfm_8x8_N2_rounding(outcoeff128, -shift[1]);
        transpose_4x8_in_4x16_sse4_1(outcoeff128, in);
        fdct4x8_col_N2_sse4_1(in, outcoeff128, bitrow, 2); //dct + transpose + clear right half of buffer
        clear_buffer_4x16_N2(outcoeff128);
        break;
    case ADST_DCT:
        load_buffer_4x16_sse4_1(input, in, stride, 0, 0, shift[0]);
        fadst16x4_N2_sse4_1(in, outcoeff128, bitcol);
        col_txfm_8x8_N2_rounding(outcoeff128, -shift[1]);
        transpose_4x8_in_4x16_sse4_1(outcoeff128, in);
        fdct4x8_col_N2_sse4_1(in, outcoeff128, bitrow, 2);
        clear_buffer_4x16_N2(outcoeff128);
        break;
    case DCT_ADST:
        load_buffer_4x16_sse4_1(input, in, stride, 0, 0, shift[0]);
        fdct16x4_N2_sse4_1(in, outcoeff128, bitcol);
        col_txfm_8x8_N2_rounding(outcoeff128, -shift[1]);
        transpose_4x8_in_4x16_sse4_1(outcoeff128, in);
        fadst4x8_col_N2_sse4_1(in, outcoeff128, bitrow, 2);
        clear_buffer_4x16_N2(outcoeff128);
        break;
    case ADST_ADST:
        load_buffer_4x16_sse4_1(input, in, stride, 0, 0, shift[0]);
        fadst16x4_N2_sse4_1(in, outcoeff128, bitcol);
        col_txfm_8x8_N2_rounding(outcoeff128, -shift[1]);
        transpose_4x8_in_4x16_sse4_1(outcoeff128, in);
        fadst4x8_col_N2_sse4_1(in, outcoeff128, bitrow, 2);
        clear_buffer_4x16_N2(outcoeff128);
        break;
    case FLIPADST_DCT:
        load_buffer_4x16_sse4_1(input, in, stride, 1, 0, shift[0]);
        fadst16x4_N2_sse4_1(in, outcoeff128, bitcol);
        col_txfm_8x8_N2_rounding(outcoeff128, -shift[1]);
        transpose_4x8_in_4x16_sse4_1(outcoeff128, in);
        fdct4x8_col_N2_sse4_1(in, outcoeff128, bitrow, 2);
        clear_buffer_4x16_N2(outcoeff128);
        break;
    case DCT_FLIPADST:
        load_buffer_4x16_sse4_1(input, in, stride, 0, 1, shift[0]);
        fdct16x4_N2_sse4_1(in, outcoeff128, bitcol);
        col_txfm_8x8_N2_rounding(outcoeff128, -shift[1]);
        transpose_4x8_in_4x16_sse4_1(outcoeff128, in);
        fadst4x8_col_N2_sse4_1(in, outcoeff128, bitrow, 2);
        clear_buffer_4x16_N2(outcoeff128);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_4x16_sse4_1(input, in, stride, 1, 1, shift[0]);
        fadst16x4_N2_sse4_1(in, outcoeff128, bitcol);
        col_txfm_8x8_N2_rounding(outcoeff128, -shift[1]);
        transpose_4x8_in_4x16_sse4_1(outcoeff128, in);
        fadst4x8_col_N2_sse4_1(in, outcoeff128, bitrow, 2);
        clear_buffer_4x16_N2(outcoeff128);
        break;
    case ADST_FLIPADST:
        load_buffer_4x16_sse4_1(input, in, stride, 0, 1, shift[0]);
        fadst16x4_N2_sse4_1(in, outcoeff128, bitcol);
        col_txfm_8x8_N2_rounding(outcoeff128, -shift[1]);
        transpose_4x8_in_4x16_sse4_1(outcoeff128, in);
        fadst4x8_col_N2_sse4_1(in, outcoeff128, bitrow, 2);
        clear_buffer_4x16_N2(outcoeff128);
        break;
    case FLIPADST_ADST:
        load_buffer_4x16_sse4_1(input, in, stride, 1, 0, shift[0]);
        fadst16x4_N2_sse4_1(in, outcoeff128, bitcol);
        col_txfm_8x8_N2_rounding(outcoeff128, -shift[1]);
        transpose_4x8_in_4x16_sse4_1(outcoeff128, in);
        fadst4x8_col_N2_sse4_1(in, outcoeff128, bitrow, 2);
        clear_buffer_4x16_N2(outcoeff128);
        break;
    case IDTX:
        load_buffer_4x8_sse4_1(input, in, stride, 0, 0, shift[0]);
        fidtx8xn_N2_col_sse4_1(in, outcoeff128, bitcol, 4);
        col_txfm_8x8_N2_rounding(outcoeff128, -shift[1]);
        fidtx4x8_col_N2_sse4_1(outcoeff128, outcoeff128, bitrow, 4);
        clear_buffer_4x16_N2(outcoeff128);
        break;
    case V_DCT:
        load_buffer_4x16_sse4_1(input, in, stride, 0, 0, shift[0]);
        fdct16x4_N2_sse4_1(in, outcoeff128, bitcol);
        col_txfm_8x8_N2_rounding(outcoeff128, -shift[1]);
        fidtx4x8_col_N2_sse4_1(outcoeff128, outcoeff128, bitrow, 4);
        clear_buffer_4x16_N2(outcoeff128);
        break;
    case H_DCT:
        load_buffer_4x8_sse4_1(input, in, stride, 0, 0, shift[0]);
        fidtx8xn_N2_col_sse4_1(in, outcoeff128, bitcol, 4);
        col_txfm_8x8_N2_rounding(outcoeff128, -shift[1]);
        transpose_4x8_in_4x16_sse4_1(outcoeff128, in);
        fdct4x8_col_N2_sse4_1(in, outcoeff128, bitrow, 2);
        clear_buffer_4x16_N2(outcoeff128);
        break;
    case V_ADST:
        load_buffer_4x16_sse4_1(input, in, stride, 0, 0, shift[0]);
        fadst16x4_N2_sse4_1(in, outcoeff128, bitcol);
        col_txfm_8x8_N2_rounding(outcoeff128, -shift[1]);
        fidtx4x8_col_N2_sse4_1(outcoeff128, outcoeff128, bitrow, 4);
        clear_buffer_4x16_N2(outcoeff128);
        break;
    case H_ADST:
        load_buffer_4x8_sse4_1(input, in, stride, 0, 0, shift[0]);
        fidtx8xn_N2_col_sse4_1(in, outcoeff128, bitcol, 4);
        col_txfm_8x8_N2_rounding(outcoeff128, -shift[1]);
        transpose_4x8_in_4x16_sse4_1(outcoeff128, in);
        fadst4x8_col_N2_sse4_1(in, outcoeff128, bitrow, 2);
        clear_buffer_4x16_N2(outcoeff128);
        break;
    case V_FLIPADST:
        load_buffer_4x16_sse4_1(input, in, stride, 1, 0, shift[0]);
        fadst16x4_N2_sse4_1(in, outcoeff128, bitcol);
        col_txfm_8x8_N2_rounding(outcoeff128, -shift[1]);
        fidtx4x8_col_N2_sse4_1(outcoeff128, outcoeff128, bitrow, 4);
        clear_buffer_4x16_N2(outcoeff128);
        break;
    case H_FLIPADST:
        load_buffer_4x8_sse4_1(input, in, stride, 0, 1, shift[0]);
        fidtx8xn_N2_col_sse4_1(in, outcoeff128, bitcol, 4);
        col_txfm_8x8_N2_rounding(outcoeff128, -shift[1]);
        transpose_4x8_in_4x16_sse4_1(outcoeff128, in);
        fadst4x8_col_N2_sse4_1(in, outcoeff128, bitrow, 2);
        clear_buffer_4x16_N2(outcoeff128);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_16x4_N2_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[16];
    __m128i*      outcoeff128 = (__m128i*)output;
    const int8_t* shift       = fwd_txfm_shift_ls[TX_16X4];
    const int32_t txw_idx     = get_txw_idx(TX_16X4);
    const int32_t txh_idx     = get_txh_idx(TX_16X4);
    int32_t       bitcol      = fwd_cos_bit_col[txw_idx][txh_idx];
    int32_t       bitrow      = fwd_cos_bit_row[txw_idx][txh_idx];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_16x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fdct4x8_row_N2_with_round_sse4_1(in, outcoeff128, bitcol, 1, -shift[1]);
        fdct4x8_row_N2_with_round_sse4_1(in + 8, outcoeff128 + 8, bitcol, 1, -shift[1]);
        fdct16x4_N2_sse4_1(outcoeff128, in, bitrow);
        transpose_4x8_in_4x16_half_sse4_1(in, outcoeff128);
        clear_buffer_wxh_N2(outcoeff128, 2, 4);
        break;
    case ADST_DCT:
        load_buffer_16x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fadst4x8_row_N2_with_round_sse4_1(in, outcoeff128, bitcol, 1, -shift[1]);
        fadst4x8_row_N2_with_round_sse4_1(in + 8, outcoeff128 + 8, bitcol, 1, -shift[1]);
        fdct16x4_N2_sse4_1(outcoeff128, in, bitrow);
        transpose_4x8_in_4x16_half_sse4_1(in, outcoeff128);
        clear_buffer_wxh_N2(outcoeff128, 2, 4);
        break;
    case DCT_ADST:
        load_buffer_16x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fdct4x8_row_N2_with_round_sse4_1(in, outcoeff128, bitcol, 1, -shift[1]);
        fdct4x8_row_N2_with_round_sse4_1(in + 8, outcoeff128 + 8, bitcol, 1, -shift[1]);
        fadst16x4_N2_sse4_1(outcoeff128, in, bitrow);
        transpose_4x8_in_4x16_half_sse4_1(in, outcoeff128);
        clear_buffer_wxh_N2(outcoeff128, 2, 4);
        break;
    case ADST_ADST:
        load_buffer_16x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fadst4x8_row_N2_with_round_sse4_1(in, outcoeff128, bitcol, 1, -shift[1]);
        fadst4x8_row_N2_with_round_sse4_1(in + 8, outcoeff128 + 8, bitcol, 1, -shift[1]);
        fadst16x4_N2_sse4_1(outcoeff128, in, bitrow);
        transpose_4x8_in_4x16_half_sse4_1(in, outcoeff128);
        clear_buffer_wxh_N2(outcoeff128, 2, 4);
        break;
    case FLIPADST_DCT:
        load_buffer_16x4_sse4_1(input, in, stride, 1, 0, shift[0]);
        fadst4x8_row_N2_with_round_sse4_1(in, outcoeff128, bitcol, 1, -shift[1]);
        fadst4x8_row_N2_with_round_sse4_1(in + 8, outcoeff128 + 8, bitcol, 1, -shift[1]);
        fdct16x4_N2_sse4_1(outcoeff128, in, bitrow);
        transpose_4x8_in_4x16_half_sse4_1(in, outcoeff128);
        clear_buffer_wxh_N2(outcoeff128, 2, 4);
        break;
    case DCT_FLIPADST:
        load_buffer_16x4_sse4_1(input, in, stride, 0, 1, shift[0]);
        fdct4x8_row_N2_with_round_sse4_1(in, outcoeff128, bitcol, 1, -shift[1]);
        fdct4x8_row_N2_with_round_sse4_1(in + 8, outcoeff128 + 8, bitcol, 1, -shift[1]);
        fadst16x4_N2_sse4_1(outcoeff128, in, bitrow);
        transpose_4x8_in_4x16_half_sse4_1(in, outcoeff128);
        clear_buffer_wxh_N2(outcoeff128, 2, 4);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_16x4_sse4_1(input, in, stride, 1, 1, shift[0]);
        fadst4x8_row_N2_with_round_sse4_1(in, outcoeff128, bitcol, 1, -shift[1]);
        fadst4x8_row_N2_with_round_sse4_1(in + 8, outcoeff128 + 8, bitcol, 1, -shift[1]);
        fadst16x4_N2_sse4_1(outcoeff128, in, bitrow);
        transpose_4x8_in_4x16_half_sse4_1(in, outcoeff128);
        clear_buffer_wxh_N2(outcoeff128, 2, 4);
        break;
    case ADST_FLIPADST:
        load_buffer_16x4_sse4_1(input, in, stride, 0, 1, shift[0]);
        fadst4x8_row_N2_with_round_sse4_1(in, outcoeff128, bitcol, 1, -shift[1]);
        fadst4x8_row_N2_with_round_sse4_1(in + 8, outcoeff128 + 8, bitcol, 1, -shift[1]);
        fadst16x4_N2_sse4_1(outcoeff128, in, bitrow);
        transpose_4x8_in_4x16_half_sse4_1(in, outcoeff128);
        clear_buffer_wxh_N2(outcoeff128, 2, 4);
        break;
    case FLIPADST_ADST:
        load_buffer_16x4_sse4_1(input, in, stride, 1, 0, shift[0]);
        fadst4x8_row_N2_with_round_sse4_1(in, outcoeff128, bitcol, 1, -shift[1]);
        fadst4x8_row_N2_with_round_sse4_1(in + 8, outcoeff128 + 8, bitcol, 1, -shift[1]);
        fadst16x4_N2_sse4_1(outcoeff128, in, bitrow);
        transpose_4x8_in_4x16_half_sse4_1(in, outcoeff128);
        clear_buffer_wxh_N2(outcoeff128, 2, 4);
        break;
    case IDTX:
        load_buffer_8x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fidtx4x8_row_N2_sse4_1(in, outcoeff128, bitcol);
        col_txfm_8x8_rounding(outcoeff128, -shift[1]);
        fidtx4x8_N2_perm_sse4_1(outcoeff128, outcoeff128, bitrow);
        clear_buffer_wxh_N2(outcoeff128, 2, 4);
        break;
    case V_DCT:
        load_buffer_8x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fdct4x8_row_N2_with_round_sse4_1(in, outcoeff128, bitcol, 1, -shift[1]);
        fidtx8xn_N2_col_sse4_1(outcoeff128, in, bitrow, 4);
        transpose_4x8_in_4x16_half_sse4_1(in, outcoeff128);
        clear_buffer_wxh_N2(outcoeff128, 2, 4);
        break;
    case H_DCT:
        load_buffer_16x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fidtx4x8_row_N2_with_round_sse4_1(in, outcoeff128, bitcol, -shift[1]);
        fidtx4x8_row_N2_with_round_sse4_1(in + 8, outcoeff128 + 8, bitcol, -shift[1]);
        fdct16x4_N2_sse4_1(outcoeff128, in, bitrow);
        transpose_4x8_in_4x16_half_sse4_1(in, outcoeff128);
        clear_buffer_wxh_N2(outcoeff128, 2, 4);
        break;
    case V_ADST:
        load_buffer_8x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fadst4x8_row_N2_with_round_sse4_1(in, outcoeff128, bitcol, 1, -shift[1]);
        fidtx8xn_N2_col_sse4_1(outcoeff128, in, bitrow, 4);
        transpose_4x8_in_4x16_half_sse4_1(in, outcoeff128);
        clear_buffer_wxh_N2(outcoeff128, 2, 4);
        break;
    case H_ADST:
        load_buffer_16x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fidtx4x8_row_N2_with_round_sse4_1(in, outcoeff128, bitcol, -shift[1]);
        fidtx4x8_row_N2_with_round_sse4_1(in + 8, outcoeff128 + 8, bitcol, -shift[1]);
        fadst16x4_N2_sse4_1(outcoeff128, in, bitrow);
        transpose_4x8_in_4x16_half_sse4_1(in, outcoeff128);
        clear_buffer_wxh_N2(outcoeff128, 2, 4);
        break;
    case V_FLIPADST:
        load_buffer_8x4_sse4_1(input, in, stride, 1, 0, shift[0]);
        fadst4x8_row_N2_with_round_sse4_1(in, outcoeff128, bitcol, 1, -shift[1]);
        fidtx8xn_N2_col_sse4_1(outcoeff128, in, bitrow, 4);
        transpose_4x8_in_4x16_half_sse4_1(in, outcoeff128);
        clear_buffer_wxh_N2(outcoeff128, 2, 4);
        break;
    case H_FLIPADST:
        load_buffer_16x4_sse4_1(input, in, stride, 0, 1, shift[0]);
        fidtx4x8_row_N2_with_round_sse4_1(in, outcoeff128, bitcol, -shift[1]);
        fidtx4x8_row_N2_with_round_sse4_1(in + 8, outcoeff128 + 8, bitcol, -shift[1]);
        fadst16x4_N2_sse4_1(outcoeff128, in, bitrow);
        transpose_4x8_in_4x16_half_sse4_1(in, outcoeff128);
        clear_buffer_wxh_N2(outcoeff128, 2, 4);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_32x64_N2_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)tx_type;
    __m128i       in[512];
    __m128i*      outcoef128    = (__m128i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_32X64];
    const int32_t txw_idx       = get_txw_idx(TX_32X64);
    const int32_t txh_idx       = get_txh_idx(TX_32X64);
    const int32_t txfm_size_col = tx_size_wide[TX_32X64];
    const int32_t txfm_size_row = tx_size_high[TX_32X64];
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    // column transform
    load_buffer_32x8n(input, in, stride, 0, 0, shift[0], txfm_size_row);
    av1_fdct64_new_N2_sse4_1(in, in, bitcol, txfm_size_col, num_col);

    for (int32_t i = 0; i < num_row / 2; i++) {
        col_txfm_16x16_rounding((in + 2 * i * txfm_size_col), -shift[1]);
    }
    transpose_8nx8n_N2_half_sse4_1(in, outcoef128, txfm_size_col, txfm_size_row);

    // row transform
    av1_fdct32_new_N2_sse4_1(outcoef128, in, bitrow, txfm_size_row / 2, num_row);
    transpose_8nx8n_N2_quad_sse4_1(in, outcoef128, txfm_size_row, txfm_size_col);
    av1_round_shift_rect_array_wxh_sse4_1(outcoef128, outcoef128, -shift[2], new_sqrt2, num_col, txfm_size_row);
    clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
    (void)bd;
}

void svt_av1_fwd_txfm2d_64x32_N2_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)tx_type;
    __m128i       in[512];
    __m128i*      outcoef128    = (__m128i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_64X32];
    const int32_t txw_idx       = get_txw_idx(TX_64X32);
    const int32_t txh_idx       = get_txh_idx(TX_64X32);
    const int32_t txfm_size_col = tx_size_wide[TX_64X32];
    const int32_t txfm_size_row = tx_size_high[TX_64X32];
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    // column transform
    for (int32_t i = 0; i < 32; i++) {
        load_buffer_32_sse4_1(input + 0 + i * stride, in + 2 * (0 + i * 8), 8, 0, 0, shift[0]);
        load_buffer_32_sse4_1(input + 32 + i * stride, in + 2 * (4 + i * 8), 8, 0, 0, shift[0]);
    }

    av1_fdct32_new_N2_sse4_1(in, in, bitcol, txfm_size_col, num_col);

    for (int32_t i = 0; i < num_col / 2; i++) {
        col_txfm_16x16_rounding(in + 2 * (i * txfm_size_row), -shift[1]);
    }
    transpose_8nx8n_N2_half_sse4_1(in, outcoef128, txfm_size_col, txfm_size_row);

    // row transform
    av1_fdct64_new_N2_sse4_1(outcoef128, in, bitrow, txfm_size_row / 2, num_row);
    transpose_8nx8n_N2_quad_sse4_1(in, outcoef128, txfm_size_row, txfm_size_col);
    av1_round_shift_rect_array_wxh_sse4_1(outcoef128, outcoef128, -shift[2], new_sqrt2, num_col, txfm_size_row);
    clear_buffer_wxh_N2(outcoef128, num_col, txfm_size_row);
    (void)bd;
}

void svt_av1_fwd_txfm2d_16x64_N2_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[256];
    __m128i*      outcoeff128   = (__m128i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_16X64];
    const int32_t txw_idx       = get_txw_idx(TX_16X64);
    const int32_t txh_idx       = get_txh_idx(TX_16X64);
    const int32_t txfm_size_col = tx_size_wide[TX_16X64];
    const int32_t txfm_size_row = tx_size_high[TX_16X64];
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    int32_t       ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    const int32_t num_row = txfm_size_row >> 3;
    const int32_t num_col = txfm_size_col >> 3;
    // col tranform
    for (int32_t i = 0; i < txfm_size_row; i += num_col) {
        load_buffer_16_sse4_1(input + (i + 0) * stride, in + 2 * (i + 0) * num_col, 8, ud_flip, lr_flip, shift[0]);
        load_buffer_16_sse4_1(input + (i + 1) * stride, in + 2 * (i + 1) * num_col, 8, ud_flip, lr_flip, shift[0]);
    }

    av1_fdct64_new_N2_sse4_1(in, outcoeff128, bitcol, txfm_size_col, num_col);

    col_txfm_16x16_rounding(outcoeff128, -shift[1]);
    col_txfm_16x16_rounding(outcoeff128 + 64, -shift[1]);
    transpose_8nx8n_N2_half_sse4_1(outcoeff128, in, txfm_size_col, txfm_size_row);
    // row tranform
    fdct16x16_N2_sse4_1(in, in, bitrow, num_row, num_row / 2);
    transpose_8nx8n_N2_quad_sse4_1(in, outcoeff128, txfm_size_row, txfm_size_col);
    clear_buffer_wxh_N2(outcoeff128, num_col, txfm_size_row);
    (void)bd;
}

void svt_av1_fwd_txfm2d_64x16_N2_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[256];
    __m128i*      outcoeff128   = (__m128i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_64X16];
    const int32_t txw_idx       = get_txw_idx(TX_64X16);
    const int32_t txh_idx       = get_txh_idx(TX_64X16);
    const int32_t txfm_size_col = tx_size_wide[TX_64X16];
    const int32_t txfm_size_row = tx_size_high[TX_64X16];
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    int32_t       ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    const int32_t num_row = txfm_size_row >> 3;
    const int32_t num_col = txfm_size_col >> 3;
    // col tranform
    for (int32_t i = 0; i < txfm_size_row; i++) {
        load_buffer_16_sse4_1(input + 0 + i * stride, in + 2 * (0 + i * 8), 8, ud_flip, lr_flip, shift[0]);
        load_buffer_16_sse4_1(input + 16 + i * stride, in + 2 * (2 + i * 8), 8, ud_flip, lr_flip, shift[0]);
        load_buffer_16_sse4_1(input + 32 + i * stride, in + 2 * (4 + i * 8), 8, ud_flip, lr_flip, shift[0]);
        load_buffer_16_sse4_1(input + 48 + i * stride, in + 2 * (6 + i * 8), 8, ud_flip, lr_flip, shift[0]);
    }

    fdct16x16_N2_sse4_1(in, outcoeff128, bitcol, num_col, num_col);
    col_txfm_16x16_rounding(outcoeff128, -shift[1]);
    col_txfm_16x16_rounding(outcoeff128 + 64, -shift[1]);
    transpose_8nx8n_N2_half_sse4_1(outcoeff128, in, txfm_size_col, txfm_size_row);
    // row tranform
    av1_fdct64_new_N2_sse4_1(in, in, bitrow, txfm_size_row / 2, num_row);
    transpose_8nx8n_N2_quad_sse4_1(in, outcoeff128, txfm_size_row, txfm_size_col);
    clear_buffer_wxh_N2(outcoeff128, num_col, txfm_size_row);
    (void)bd;
}

static INLINE void transpose_4x4_in_16x16_sse4_1(const __m128i* in, __m128i* out) {
    __m128i u0, u1, u2, u3;
    u0      = _mm_unpacklo_epi32(in[0], in[4]);
    u1      = _mm_unpackhi_epi32(in[0], in[4]);
    u2      = _mm_unpacklo_epi32(in[8], in[12]);
    u3      = _mm_unpackhi_epi32(in[8], in[12]);
    out[0]  = _mm_unpacklo_epi64(u0, u2);
    out[4]  = _mm_unpackhi_epi64(u0, u2);
    out[8]  = _mm_unpacklo_epi64(u1, u3);
    out[12] = _mm_unpackhi_epi64(u1, u3);
}

static INLINE void transpose_8x8_in_32x32_sse4_1(const __m128i* in, __m128i* out) {
    TRANSPOSE_2X4X4_SSE4_1(in, 0, 8, 16, 24, out, 0, 32, 8, 40, 16, 48, 24, 56);
    TRANSPOSE_2X4X4_SSE4_1(in, 32, 40, 48, 56, out, 1, 33, 9, 41, 17, 49, 25, 57);
}

static INLINE void transpose_8nx8n_N4_half_sse4_1(const __m128i* input, __m128i* output, const int32_t width,
                                                  const int32_t height) {
    const int32_t numcol      = height >> 3;
    const int32_t numrow      = width >> 3;
    int32_t       calc_numcol = numcol >> 2;
    if (!calc_numcol) {
        calc_numcol = 1;
    }

    for (int32_t j = 0; j < numrow; j++) {
        for (int32_t i = 0; i < calc_numcol; i++) {
            TRANSPOSE_2X4X4_SSE4_1(input,
                                   2 * (i * width + j + (numrow * 0)),
                                   2 * (i * width + j + (numrow * 1)),
                                   2 * (i * width + j + (numrow * 2)),
                                   2 * (i * width + j + (numrow * 3)),
                                   output,
                                   2 * (j * height + i + (numcol * 0)),
                                   2 * (j * height + i + (numcol * 4)),
                                   2 * (j * height + i + (numcol * 1)),
                                   2 * (j * height + i + (numcol * 5)),
                                   2 * (j * height + i + (numcol * 2)),
                                   2 * (j * height + i + (numcol * 6)),
                                   2 * (j * height + i + (numcol * 3)),
                                   2 * (j * height + i + (numcol * 7)));
            TRANSPOSE_2X4X4_SSE4_1(input,
                                   2 * (i * width + j + (numrow * 4)),
                                   2 * (i * width + j + (numrow * 5)),
                                   2 * (i * width + j + (numrow * 6)),
                                   2 * (i * width + j + (numrow * 7)),
                                   output,
                                   2 * (j * height + i + (numcol * 0)) + 1,
                                   2 * (j * height + i + (numcol * 4)) + 1,
                                   2 * (j * height + i + (numcol * 1)) + 1,
                                   2 * (j * height + i + (numcol * 5)) + 1,
                                   2 * (j * height + i + (numcol * 2)) + 1,
                                   2 * (j * height + i + (numcol * 6)) + 1,
                                   2 * (j * height + i + (numcol * 3)) + 1,
                                   2 * (j * height + i + (numcol * 7)) + 1);
        }
    }
}

static INLINE void transpose_8nx8n_N4_quad_sse4_1(const __m128i* input, __m128i* output, const int32_t width,
                                                  const int32_t height) {
    const int32_t numcol = height >> 3;
    const int32_t numrow = width >> 3;

    int32_t calc_numcol = numcol >> 2;
    int32_t calc_numrow = numrow >> 2;
    if (!calc_numcol) {
        calc_numcol = 1;
    }
    if (!calc_numrow) {
        calc_numrow = 1;
    }

    for (int32_t j = 0; j < calc_numrow; j++) {
        for (int32_t i = 0; i < calc_numcol; i++) {
            TRANSPOSE_2X4X4_SSE4_1(input,
                                   2 * (i * width + j + (numrow * 0)),
                                   2 * (i * width + j + (numrow * 1)),
                                   2 * (i * width + j + (numrow * 2)),
                                   2 * (i * width + j + (numrow * 3)),
                                   output,
                                   2 * (j * height + i + (numcol * 0)),
                                   2 * (j * height + i + (numcol * 4)),
                                   2 * (j * height + i + (numcol * 1)),
                                   2 * (j * height + i + (numcol * 5)),
                                   2 * (j * height + i + (numcol * 2)),
                                   2 * (j * height + i + (numcol * 6)),
                                   2 * (j * height + i + (numcol * 3)),
                                   2 * (j * height + i + (numcol * 7)));
            TRANSPOSE_2X4X4_SSE4_1(input,
                                   2 * (i * width + j + (numrow * 4)),
                                   2 * (i * width + j + (numrow * 5)),
                                   2 * (i * width + j + (numrow * 6)),
                                   2 * (i * width + j + (numrow * 7)),
                                   output,
                                   2 * (j * height + i + (numcol * 0)) + 1,
                                   2 * (j * height + i + (numcol * 4)) + 1,
                                   2 * (j * height + i + (numcol * 1)) + 1,
                                   2 * (j * height + i + (numcol * 5)) + 1,
                                   2 * (j * height + i + (numcol * 2)) + 1,
                                   2 * (j * height + i + (numcol * 6)) + 1,
                                   2 * (j * height + i + (numcol * 3)) + 1,
                                   2 * (j * height + i + (numcol * 7)) + 1);
        }
    }
}

static INLINE void transpose_4x8_in_4x16_quad_sse4_1(const __m128i* in, __m128i* out) {
    __m128i u0, u1, u2, u3;
    __m128i v0, v1;

    u0 = _mm_unpacklo_epi32(in[0], in[2]);
    u1 = _mm_unpacklo_epi32(in[1], in[3]);
    u2 = _mm_unpacklo_epi32(in[4], in[6]);
    u3 = _mm_unpacklo_epi32(in[5], in[7]);

    v0 = _mm_unpacklo_epi64(u0, u2);
    v1 = _mm_unpacklo_epi64(u1, u3);

    out[0] = _mm_unpacklo_epi32(v0, v1);
    out[1] = _mm_unpackhi_epi32(v0, v1);
}

static AOM_FORCE_INLINE void write_buffer_8x8_N4(const __m128i* res, int32_t* output) {
    const __m128i zero = _mm_setzero_si128();

    _mm_storeu_si128((__m128i*)(output + 0 * 4), zero);
    _mm_storeu_si128((__m128i*)(output + 1 * 4), zero);
    _mm_storeu_si128((__m128i*)(output + 2 * 4), zero);
    _mm_storeu_si128((__m128i*)(output + 3 * 4), zero);
    _mm_storel_epi64((__m128i*)(output + 0 * 4), res[0]);
    _mm_storel_epi64((__m128i*)(output + 2 * 4), res[2]);
    _mm_storeu_si128((__m128i*)(output + 4 * 4), zero);
    _mm_storeu_si128((__m128i*)(output + 5 * 4), zero);
    _mm_storeu_si128((__m128i*)(output + 6 * 4), zero);
    _mm_storeu_si128((__m128i*)(output + 7 * 4), zero);
    _mm_storeu_si128((__m128i*)(output + 8 * 4), zero);
    _mm_storeu_si128((__m128i*)(output + 9 * 4), zero);
    _mm_storeu_si128((__m128i*)(output + 10 * 4), zero);
    _mm_storeu_si128((__m128i*)(output + 11 * 4), zero);
    _mm_storeu_si128((__m128i*)(output + 12 * 4), zero);
    _mm_storeu_si128((__m128i*)(output + 13 * 4), zero);
    _mm_storeu_si128((__m128i*)(output + 14 * 4), zero);
    _mm_storeu_si128((__m128i*)(output + 15 * 4), zero);
}

static INLINE void write_buffer_16x16_N4(const __m128i* res, int32_t* output) {
    int32_t       fact = -1, index = 0;
    const __m128i zero = _mm_setzero_si128();
    int32_t       i;
    for (i = 0; i < 2; i++) {
        _mm_storeu_si128((__m128i*)(output + (++fact) * 16), res[2 * (index)]);
        _mm_storeu_si128((__m128i*)(output + fact * 16 + 4), zero);
        _mm_storeu_si128((__m128i*)(output + fact * 16 + 8), zero);
        _mm_storeu_si128((__m128i*)(output + fact * 16 + 12), zero);
        index += 2;
        _mm_storeu_si128((__m128i*)(output + (++fact) * 16), res[2 * (index)]);
        _mm_storeu_si128((__m128i*)(output + fact * 16 + 4), zero);
        _mm_storeu_si128((__m128i*)(output + fact * 16 + 8), zero);
        _mm_storeu_si128((__m128i*)(output + fact * 16 + 12), zero);
        index += 2;
    }
    for (; i < 8; i++) {
        _mm_storeu_si128((__m128i*)(output + (++fact) * 16), zero);
        _mm_storeu_si128((__m128i*)(output + (fact) * 16 + 4), zero);
        _mm_storeu_si128((__m128i*)(output + (fact) * 16 + 8), zero);
        _mm_storeu_si128((__m128i*)(output + (fact) * 16 + 12), zero);
        _mm_storeu_si128((__m128i*)(output + (++fact) * 16), zero);
        _mm_storeu_si128((__m128i*)(output + (fact) * 16 + 4), zero);
        _mm_storeu_si128((__m128i*)(output + (fact) * 16 + 8), zero);
        _mm_storeu_si128((__m128i*)(output + (fact) * 16 + 12), zero);
    }
}

static INLINE void write_buffer_32x32_N4(const __m128i* res, int32_t* output) {
    const __m128i zero = _mm_setzero_si128();
    uint32_t      i;

    for (i = 0; i < 8; i++) {
        _mm_storeu_si128((__m128i*)(output + i * 32 + 0), res[i * 8]);
        _mm_storeu_si128((__m128i*)(output + i * 32 + 4), res[i * 8 + 1]);
        _mm_storeu_si128((__m128i*)(output + i * 32 + 8), zero);
        _mm_storeu_si128((__m128i*)(output + i * 32 + 12), zero);
        _mm_storeu_si128((__m128i*)(output + i * 32 + 16), zero);
        _mm_storeu_si128((__m128i*)(output + i * 32 + 20), zero);
        _mm_storeu_si128((__m128i*)(output + i * 32 + 24), zero);
        _mm_storeu_si128((__m128i*)(output + i * 32 + 28), zero);
    }

    for (; i < 32; i++) {
        _mm_storeu_si128((__m128i*)(output + i * 32 + 0), zero);
        _mm_storeu_si128((__m128i*)(output + i * 32 + 4), zero);
        _mm_storeu_si128((__m128i*)(output + i * 32 + 8), zero);
        _mm_storeu_si128((__m128i*)(output + i * 32 + 12), zero);
        _mm_storeu_si128((__m128i*)(output + i * 32 + 16), zero);
        _mm_storeu_si128((__m128i*)(output + i * 32 + 20), zero);
        _mm_storeu_si128((__m128i*)(output + i * 32 + 24), zero);
        _mm_storeu_si128((__m128i*)(output + i * 32 + 28), zero);
    }
}

static AOM_FORCE_INLINE void write_buffer_16x8_N4_sse4_1(const __m128i* res, __m128i* out) {
    out[0] = res[0];
    out[1] = res[1];
    out[4] = res[2];
    out[5] = res[3];
}

static AOM_FORCE_INLINE void col_txfm_8x8_N4_rounding(__m128i* in, int32_t shift) {
    const __m128i rounding = _mm_set1_epi32(1 << (shift - 1));

    in[0] = _mm_add_epi32(in[0], rounding);
    in[1] = _mm_add_epi32(in[1], rounding);
    in[2] = _mm_add_epi32(in[2], rounding);
    in[3] = _mm_add_epi32(in[3], rounding);

    in[0] = _mm_srai_epi32(in[0], shift);
    in[1] = _mm_srai_epi32(in[1], shift);
    in[2] = _mm_srai_epi32(in[2], shift);
    in[3] = _mm_srai_epi32(in[3], shift);
}

static AOM_FORCE_INLINE void col_txfm_32x8_N4_rounding(__m128i* in, int32_t shift) {
    const __m128i rounding = _mm_set1_epi32(1 << (shift - 1));

    in[0] = _mm_add_epi32(in[0], rounding);
    in[1] = _mm_add_epi32(in[1], rounding);
    in[8] = _mm_add_epi32(in[8], rounding);
    in[9] = _mm_add_epi32(in[9], rounding);

    in[0] = _mm_srai_epi32(in[0], shift);
    in[1] = _mm_srai_epi32(in[1], shift);
    in[8] = _mm_srai_epi32(in[8], shift);
    in[9] = _mm_srai_epi32(in[9], shift);
}

static AOM_FORCE_INLINE void col_txfm_16x16_N4_rounding(__m128i* in, int32_t shift) {
    col_txfm_8x8_rounding(&in[0], shift);
}

static INLINE void av1_round_shift_array_32_N4_sse4_1(__m128i* input, __m128i* output, const int32_t size,
                                                      const int32_t bit) {
    int32_t i;
    if (bit > 0) {
        const __m128i round = _mm_set1_epi32(1 << (bit - 1));
        __m128i       r0, r1;
        for (i = 0; i < size; i += 4) {
            r0                = _mm_add_epi32(input[2 * i], round);
            r1                = _mm_add_epi32(input[2 * i + 1], round);
            output[2 * i]     = _mm_srai_epi32(r0, bit);
            output[2 * i + 1] = _mm_srai_epi32(r1, bit);
        }
    } else {
        for (i = 0; i < size; i += 4) {
            output[2 * i]     = _mm_slli_epi32(input[2 * i], -bit);
            output[2 * i + 1] = _mm_slli_epi32(input[2 * i + 1], -bit);
        }
    }
}

static INLINE void av1_round_shift_rect_wxh_N4(__m128i* input, __m128i* output, const int32_t bit, const int32_t val,
                                               int32_t num_col, int32_t num_row) {
    const __m128i sqrt2       = _mm_set1_epi32(val);
    const __m128i round2      = _mm_set1_epi32(1 << (new_sqrt2_bits - 1));
    int32_t       calc_numcol = num_col >> 2;
    if (!calc_numcol) {
        calc_numcol = 1;
    }
    int32_t i;

    if (bit > 0) {
        const __m128i round1 = _mm_set1_epi32(1 << (bit - 1));
        __m128i       r0, r1, r2, r3, r4, r5, r6, r7;
        for (i = 0; i < num_row / 4; i++) {
            for (int j = 0; j < calc_numcol; j++) {
                r0                                = _mm_add_epi32(input[2 * (i * num_col + j)], round1);
                r1                                = _mm_add_epi32(input[2 * (i * num_col + j) + 1], round1);
                r2                                = _mm_srai_epi32(r0, bit);
                r3                                = _mm_srai_epi32(r1, bit);
                r4                                = _mm_mullo_epi32(sqrt2, r2);
                r5                                = _mm_mullo_epi32(sqrt2, r3);
                r6                                = _mm_add_epi32(r4, round2);
                r7                                = _mm_add_epi32(r5, round2);
                output[2 * (i * num_col + j)]     = _mm_srai_epi32(r6, new_sqrt2_bits);
                output[2 * (i * num_col + j) + 1] = _mm_srai_epi32(r7, new_sqrt2_bits);
            }
        }
    } else {
        __m128i r0, r1, r2, r3, r4, r5;
        for (i = 0; i < num_row / 4; i++) {
            for (int j = 0; j < calc_numcol; j++) {
                r0                                = _mm_slli_epi32(input[2 * (i * num_col + j)], -bit);
                r1                                = _mm_slli_epi32(input[2 * (i * num_col + j) + 1], -bit);
                r2                                = _mm_mullo_epi32(sqrt2, r0);
                r3                                = _mm_mullo_epi32(sqrt2, r1);
                r4                                = _mm_add_epi32(r2, round2);
                r5                                = _mm_add_epi32(r3, round2);
                output[2 * (i * num_col + j)]     = _mm_srai_epi32(r4, new_sqrt2_bits);
                output[2 * (i * num_col + j) + 1] = _mm_srai_epi32(r5, new_sqrt2_bits);
            }
        }
    }
}

static AOM_FORCE_INLINE void col_txfm_32x16_N4_rounding(__m128i* in, int32_t shift) {
    const __m128i rounding = _mm_set1_epi32(1 << (shift - 1));
    in[0]                  = _mm_add_epi32(in[0], rounding);
    in[1]                  = _mm_add_epi32(in[1], rounding);
    in[8]                  = _mm_add_epi32(in[8], rounding);
    in[9]                  = _mm_add_epi32(in[9], rounding);
    in[16]                 = _mm_add_epi32(in[16], rounding);
    in[17]                 = _mm_add_epi32(in[17], rounding);
    in[24]                 = _mm_add_epi32(in[24], rounding);
    in[25]                 = _mm_add_epi32(in[25], rounding);
    in[0]                  = _mm_srai_epi32(in[0], shift);
    in[1]                  = _mm_srai_epi32(in[1], shift);
    in[8]                  = _mm_srai_epi32(in[8], shift);
    in[9]                  = _mm_srai_epi32(in[9], shift);
    in[16]                 = _mm_srai_epi32(in[16], shift);
    in[17]                 = _mm_srai_epi32(in[17], shift);
    in[24]                 = _mm_srai_epi32(in[24], shift);
    in[25]                 = _mm_srai_epi32(in[25], shift);
}

static INLINE void load_buffer_8x8_in_32x32_sse4_1(const int16_t* input, __m128i* output, int32_t stride) {
    int32_t i;

    for (i = 0; i < 8; ++i) {
        output[0] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input)));
        output[1] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 4)));

        input += stride;
        output += 8;
    }
}

static INLINE void load_buffer_8x32_in_32x32_sse4_1(const int16_t* input, __m128i* output, int32_t stride) {
    int32_t i;

    for (i = 0; i < 32; ++i) {
        output[0] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input)));
        output[1] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 4)));

        input += stride;
        output += 8;
    }
}

static INLINE void load_buffer_32x8_in_32x32_sse4_1(const int16_t* input, __m128i* output, int32_t stride) {
    int32_t i;

    for (i = 0; i < 8; ++i) {
        output[0] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 0 * 4)));
        output[1] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 1 * 4)));
        output[2] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 2 * 4)));
        output[3] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 3 * 4)));
        output[4] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 4 * 4)));
        output[5] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 5 * 4)));
        output[6] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 6 * 4)));
        output[7] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 7 * 4)));

        input += stride;
        output += 8;
    }
}

static INLINE void load_buffer_16x16_in_64x64_sse4_1(const int16_t* input, int32_t stride, __m128i* output) {
    int32_t i;

    for (i = 0; i < 16; ++i) {
        output[0] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 0 * 4)));
        output[1] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 1 * 4)));
        output[2] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 2 * 4)));
        output[3] = _mm_cvtepi16_epi32(_mm_loadl_epi64((const __m128i*)(input + 3 * 4)));

        input += stride;
        output += 16;
    }
}

static AOM_FORCE_INLINE void load_buffer_8x16_N4(const int16_t* input, __m128i* out, int32_t stride, int32_t flipud,
                                                 int32_t fliplr, int32_t shift) {
    if (flipud) {
        load_buffer_8x8(input + 8 * stride, out, stride, flipud, fliplr, shift);
    } else {
        load_buffer_8x8(input, out, stride, flipud, fliplr, shift);
    }
}

static INLINE void clear_buffer_wxh_N4(__m128i* buff, int32_t num_col, int32_t num_row) {
    const __m128i zero = _mm_setzero_si128();
    assert(num_col > 0);
    assert(num_row > 1);

    if (num_col == 1) {
        for (int i = 0; i < num_row / 4; i++) {
            buff[i * 2]     = _mm_unpacklo_epi64(buff[i * 2], zero);
            buff[i * 2 + 1] = zero;
        }
    } else if (num_col == 2) {
        for (int i = 0; i < num_row / 4; i++) {
            buff[i * 4 + 1] = zero;
            buff[i * 4 + 2] = zero;
            buff[i * 4 + 3] = zero;
        }
    } else {
        for (int i = 0; i < num_row / 4; i++) {
            for (int j = num_col / 4; j < num_col; j++) {
                buff[2 * (i * num_col + j)]     = zero;
                buff[2 * (i * num_col + j) + 1] = zero;
            }
        }
    }
    //clear bottom
    for (int i = num_row / 4; i < num_row; i++) {
        for (int j = 0; j < num_col; j++) {
            buff[2 * (i * num_col + j)]     = zero;
            buff[2 * (i * num_col + j) + 1] = zero;
        }
    }
}

static AOM_FORCE_INLINE void clear_buffer_4x16_N4(__m128i* buff) {
    const __m128i zero = _mm_setzero_si128();
    buff[4]            = zero;
    buff[5]            = zero;
    buff[6]            = zero;
    buff[7]            = zero;
    buff[8]            = zero;
    buff[9]            = zero;
    buff[10]           = zero;
    buff[11]           = zero;
    buff[12]           = zero;
    buff[13]           = zero;
    buff[14]           = zero;
    buff[15]           = zero;
}

static INLINE void av1_round_shift_array_64_N4_sse4_1(__m128i* input, __m128i* output, const int32_t size,
                                                      const int32_t bit) {
    int32_t i;
    if (bit > 0) {
        const __m128i round = _mm_set1_epi32(1 << (bit - 1));
        __m128i       r0, r1;
        for (i = 0; i < size; i += 8) {
            r0                = _mm_add_epi32(input[2 * i], round);
            r1                = _mm_add_epi32(input[2 * i + 1], round);
            output[2 * i]     = _mm_srai_epi32(r0, bit);
            output[2 * i + 1] = _mm_srai_epi32(r1, bit);
            r0                = _mm_add_epi32(input[2 * i + 2], round);
            r1                = _mm_add_epi32(input[2 * i + 3], round);
            output[2 * i + 2] = _mm_srai_epi32(r0, bit);
            output[2 * i + 3] = _mm_srai_epi32(r1, bit);
        }
    } else {
        for (i = 0; i < size; i += 8) {
            output[2 * i]     = _mm_slli_epi32(input[2 * i], -bit);
            output[2 * i + 1] = _mm_slli_epi32(input[2 * i + 1], -bit);
            output[2 * i + 2] = _mm_slli_epi32(input[2 * i + 2], -bit);
            output[2 * i + 3] = _mm_slli_epi32(input[2 * i + 3], -bit);
        }
    }
}

static void fdct8x8_N4_sse4_1(const __m128i* in, __m128i* out, int8_t bit, const int32_t col_num) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospim32 = _mm_set1_epi32(-cospi[32]);
    const __m128i  cospi56  = _mm_set1_epi32(cospi[56]);
    const __m128i  cospi8   = _mm_set1_epi32(cospi[8]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    __m128i        u[16], v[16];

    // stage 0
    // stage 1
    u[0]  = _mm_add_epi32(in[2 * 0 * col_num], in[2 * 7 * col_num]);
    u[1]  = _mm_add_epi32(in[2 * 0 * col_num + 1], in[2 * 7 * col_num + 1]);
    v[14] = _mm_sub_epi32(in[2 * 0 * col_num], in[2 * 7 * col_num]);
    v[15] = _mm_sub_epi32(in[2 * 0 * col_num + 1], in[2 * 7 * col_num + 1]);
    u[2]  = _mm_add_epi32(in[2 * 1 * col_num], in[2 * 6 * col_num]);
    u[3]  = _mm_add_epi32(in[2 * 1 * col_num + 1], in[2 * 6 * col_num + 1]);
    u[12] = _mm_sub_epi32(in[2 * 1 * col_num], in[2 * 6 * col_num]);
    u[13] = _mm_sub_epi32(in[2 * 1 * col_num + 1], in[2 * 6 * col_num + 1]);
    u[4]  = _mm_add_epi32(in[2 * 2 * col_num], in[2 * 5 * col_num]);
    u[5]  = _mm_add_epi32(in[2 * 2 * col_num + 1], in[2 * 5 * col_num + 1]);
    u[10] = _mm_sub_epi32(in[2 * 2 * col_num], in[2 * 5 * col_num]);
    u[11] = _mm_sub_epi32(in[2 * 2 * col_num + 1], in[2 * 5 * col_num + 1]);
    u[6]  = _mm_add_epi32(in[2 * 3 * col_num], in[2 * 4 * col_num]);
    u[7]  = _mm_add_epi32(in[2 * 3 * col_num + 1], in[2 * 4 * col_num + 1]);
    v[8]  = _mm_sub_epi32(in[2 * 3 * col_num], in[2 * 4 * col_num]);
    v[9]  = _mm_sub_epi32(in[2 * 3 * col_num + 1], in[2 * 4 * col_num + 1]);

    // stage 2
    v[0] = _mm_add_epi32(u[0], u[6]);
    v[1] = _mm_add_epi32(u[1], u[7]);
    v[2] = _mm_add_epi32(u[2], u[4]);
    v[3] = _mm_add_epi32(u[3], u[5]);

    v[10] = _mm_mullo_epi32(u[10], cospim32);
    v[11] = _mm_mullo_epi32(u[11], cospim32);
    v[12] = _mm_mullo_epi32(u[12], cospi32);
    v[13] = _mm_mullo_epi32(u[13], cospi32);
    v[10] = _mm_add_epi32(v[10], v[12]);
    v[11] = _mm_add_epi32(v[11], v[13]);
    v[10] = _mm_add_epi32(v[10], rnding);
    v[11] = _mm_add_epi32(v[11], rnding);
    v[10] = _mm_srai_epi32(v[10], bit);
    v[11] = _mm_srai_epi32(v[11], bit);

    u[0]  = _mm_mullo_epi32(u[10], cospi32);
    u[1]  = _mm_mullo_epi32(u[11], cospi32);
    v[12] = _mm_mullo_epi32(u[12], cospim32);
    v[13] = _mm_mullo_epi32(u[13], cospim32);
    v[12] = _mm_sub_epi32(u[0], v[12]);
    v[13] = _mm_sub_epi32(u[1], v[13]);
    v[12] = _mm_add_epi32(v[12], rnding);
    v[13] = _mm_add_epi32(v[13], rnding);
    v[12] = _mm_srai_epi32(v[12], bit);
    v[13] = _mm_srai_epi32(v[13], bit);

    // stage 3
    // type 0
    v[0] = _mm_mullo_epi32(v[0], cospi32);
    v[1] = _mm_mullo_epi32(v[1], cospi32);
    v[2] = _mm_mullo_epi32(v[2], cospi32);
    v[3] = _mm_mullo_epi32(v[3], cospi32);
    u[0] = _mm_add_epi32(v[0], v[2]);
    u[1] = _mm_add_epi32(v[1], v[3]);
    u[0] = _mm_add_epi32(u[0], rnding);
    u[1] = _mm_add_epi32(u[1], rnding);
    u[0] = _mm_srai_epi32(u[0], bit);
    u[1] = _mm_srai_epi32(u[1], bit);

    u[8]  = _mm_add_epi32(v[8], v[10]);
    u[9]  = _mm_add_epi32(v[9], v[11]);
    u[14] = _mm_add_epi32(v[14], v[12]);
    u[15] = _mm_add_epi32(v[15], v[13]);

    // stage 4
    // stage 5
    v[0]                     = _mm_mullo_epi32(u[8], cospi56);
    v[1]                     = _mm_mullo_epi32(u[9], cospi56);
    v[2]                     = _mm_mullo_epi32(u[14], cospi8);
    v[3]                     = _mm_mullo_epi32(u[15], cospi8);
    v[0]                     = _mm_add_epi32(v[0], v[2]);
    v[1]                     = _mm_add_epi32(v[1], v[3]);
    v[0]                     = _mm_add_epi32(v[0], rnding);
    v[1]                     = _mm_add_epi32(v[1], rnding);
    out[2 * 1 * col_num]     = _mm_srai_epi32(v[0], bit);
    out[2 * 1 * col_num + 1] = _mm_srai_epi32(v[1], bit);

    out[2 * 0 * col_num]     = u[0];
    out[2 * 0 * col_num + 1] = u[1];
}

static void fadst8x8_N4_sse4_1(const __m128i* in, __m128i* out, int8_t bit, const int32_t col_num) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospi16  = _mm_set1_epi32(cospi[16]);
    const __m128i  cospim16 = _mm_set1_epi32(-cospi[16]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospim48 = _mm_set1_epi32(-cospi[48]);
    const __m128i  cospim4  = _mm_set1_epi32(-cospi[4]);
    const __m128i  cospi60  = _mm_set1_epi32(cospi[60]);
    const __m128i  cospi52  = _mm_set1_epi32(cospi[52]);
    const __m128i  cospi12  = _mm_set1_epi32(cospi[12]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    const __m128i  zero     = _mm_setzero_si128();
    __m128i        u0, u1, u2, u3, u4, u5, u6, u7, u8, u9, u10, u11, u12, u13, u14, u15;
    __m128i        v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, v10, v11, v12, v13, v14, v15;
    __m128i        x0, x1, y0, y1;

    u0  = in[2 * 0 * col_num];
    u1  = in[2 * 0 * col_num + 1];
    u2  = _mm_sub_epi32(zero, in[2 * 7 * col_num]);
    u3  = _mm_sub_epi32(zero, in[2 * 7 * col_num + 1]);
    u4  = _mm_sub_epi32(zero, in[2 * 3 * col_num]);
    u5  = _mm_sub_epi32(zero, in[2 * 3 * col_num + 1]);
    u6  = in[2 * 4 * col_num];
    u7  = in[2 * 4 * col_num + 1];
    u8  = _mm_sub_epi32(zero, in[2 * 1 * col_num]);
    u9  = _mm_sub_epi32(zero, in[2 * 1 * col_num + 1]);
    u10 = in[2 * 6 * col_num];
    u11 = in[2 * 6 * col_num + 1];
    u12 = in[2 * 2 * col_num];
    u13 = in[2 * 2 * col_num + 1];
    u14 = _mm_sub_epi32(zero, in[2 * 5 * col_num]);
    u15 = _mm_sub_epi32(zero, in[2 * 5 * col_num + 1]);

    // stage 2
    v0 = u0;
    v1 = u1;
    v2 = u2;
    v3 = u3;

    x0 = _mm_mullo_epi32(u4, cospi32);
    x1 = _mm_mullo_epi32(u5, cospi32);
    y0 = _mm_mullo_epi32(u6, cospi32);
    y1 = _mm_mullo_epi32(u7, cospi32);
    v4 = _mm_add_epi32(x0, y0);
    v5 = _mm_add_epi32(x1, y1);
    v4 = _mm_add_epi32(v4, rnding);
    v5 = _mm_add_epi32(v5, rnding);
    v4 = _mm_srai_epi32(v4, bit);
    v5 = _mm_srai_epi32(v5, bit);

    v6 = _mm_sub_epi32(x0, y0);
    v7 = _mm_sub_epi32(x1, y1);
    v6 = _mm_add_epi32(v6, rnding);
    v7 = _mm_add_epi32(v7, rnding);
    v6 = _mm_srai_epi32(v6, bit);
    v7 = _mm_srai_epi32(v7, bit);

    v8  = u8;
    v9  = u9;
    v10 = u10;
    v11 = u11;

    x0  = _mm_mullo_epi32(u12, cospi32);
    x1  = _mm_mullo_epi32(u13, cospi32);
    y0  = _mm_mullo_epi32(u14, cospi32);
    y1  = _mm_mullo_epi32(u15, cospi32);
    v12 = _mm_add_epi32(x0, y0);
    v13 = _mm_add_epi32(x1, y1);
    v12 = _mm_add_epi32(v12, rnding);
    v13 = _mm_add_epi32(v13, rnding);
    v12 = _mm_srai_epi32(v12, bit);
    v13 = _mm_srai_epi32(v13, bit);

    v14 = _mm_sub_epi32(x0, y0);
    v15 = _mm_sub_epi32(x1, y1);
    v14 = _mm_add_epi32(v14, rnding);
    v15 = _mm_add_epi32(v15, rnding);
    v14 = _mm_srai_epi32(v14, bit);
    v15 = _mm_srai_epi32(v15, bit);

    // stage 3
    u0  = _mm_add_epi32(v0, v4);
    u1  = _mm_add_epi32(v1, v5);
    u2  = _mm_add_epi32(v2, v6);
    u3  = _mm_add_epi32(v3, v7);
    u4  = _mm_sub_epi32(v0, v4);
    u5  = _mm_sub_epi32(v1, v5);
    u6  = _mm_sub_epi32(v2, v6);
    u7  = _mm_sub_epi32(v3, v7);
    u8  = _mm_add_epi32(v8, v12);
    u9  = _mm_add_epi32(v9, v13);
    u10 = _mm_add_epi32(v10, v14);
    u11 = _mm_add_epi32(v11, v15);
    u12 = _mm_sub_epi32(v8, v12);
    u13 = _mm_sub_epi32(v9, v13);
    u14 = _mm_sub_epi32(v10, v14);
    u15 = _mm_sub_epi32(v11, v15);

    // stage 4
    v0 = u0;
    v1 = u1;
    v2 = u2;
    v3 = u3;
    v4 = u4;
    v5 = u5;
    v6 = u6;
    v7 = u7;

    x0 = _mm_mullo_epi32(u8, cospi16);
    x1 = _mm_mullo_epi32(u9, cospi16);
    y0 = _mm_mullo_epi32(u10, cospi48);
    y1 = _mm_mullo_epi32(u11, cospi48);
    v8 = _mm_add_epi32(x0, y0);
    v9 = _mm_add_epi32(x1, y1);
    v8 = _mm_add_epi32(v8, rnding);
    v9 = _mm_add_epi32(v9, rnding);
    v8 = _mm_srai_epi32(v8, bit);
    v9 = _mm_srai_epi32(v9, bit);

    x0  = _mm_mullo_epi32(u8, cospi48);
    x1  = _mm_mullo_epi32(u9, cospi48);
    y0  = _mm_mullo_epi32(u10, cospim16);
    y1  = _mm_mullo_epi32(u11, cospim16);
    v10 = _mm_add_epi32(x0, y0);
    v11 = _mm_add_epi32(x1, y1);
    v10 = _mm_add_epi32(v10, rnding);
    v11 = _mm_add_epi32(v11, rnding);
    v10 = _mm_srai_epi32(v10, bit);
    v11 = _mm_srai_epi32(v11, bit);

    x0  = _mm_mullo_epi32(u12, cospim48);
    x1  = _mm_mullo_epi32(u13, cospim48);
    y0  = _mm_mullo_epi32(u14, cospi16);
    y1  = _mm_mullo_epi32(u15, cospi16);
    v12 = _mm_add_epi32(x0, y0);
    v13 = _mm_add_epi32(x1, y1);
    v12 = _mm_add_epi32(v12, rnding);
    v13 = _mm_add_epi32(v13, rnding);
    v12 = _mm_srai_epi32(v12, bit);
    v13 = _mm_srai_epi32(v13, bit);

    x0  = _mm_mullo_epi32(u12, cospi16);
    x1  = _mm_mullo_epi32(u13, cospi16);
    y0  = _mm_mullo_epi32(u14, cospi48);
    y1  = _mm_mullo_epi32(u15, cospi48);
    v14 = _mm_add_epi32(x0, y0);
    v15 = _mm_add_epi32(x1, y1);
    v14 = _mm_add_epi32(v14, rnding);
    v15 = _mm_add_epi32(v15, rnding);
    v14 = _mm_srai_epi32(v14, bit);
    v15 = _mm_srai_epi32(v15, bit);

    // stage 5
    u0  = _mm_add_epi32(v0, v8);
    u1  = _mm_add_epi32(v1, v9);
    u2  = _mm_add_epi32(v2, v10);
    u3  = _mm_add_epi32(v3, v11);
    u12 = _mm_sub_epi32(v4, v12);
    u13 = _mm_sub_epi32(v5, v13);
    u14 = _mm_sub_epi32(v6, v14);
    u15 = _mm_sub_epi32(v7, v15);

    // stage 6
    x0 = _mm_mullo_epi32(u0, cospi60);
    x1 = _mm_mullo_epi32(u1, cospi60);
    y0 = _mm_mullo_epi32(u2, cospim4);
    y1 = _mm_mullo_epi32(u3, cospim4);
    v2 = _mm_add_epi32(x0, y0);
    v3 = _mm_add_epi32(x1, y1);
    v2 = _mm_add_epi32(v2, rnding);
    v3 = _mm_add_epi32(v3, rnding);
    v2 = _mm_srai_epi32(v2, bit);
    v3 = _mm_srai_epi32(v3, bit);

    x0  = _mm_mullo_epi32(u12, cospi52);
    x1  = _mm_mullo_epi32(u13, cospi52);
    y0  = _mm_mullo_epi32(u14, cospi12);
    y1  = _mm_mullo_epi32(u15, cospi12);
    v12 = _mm_add_epi32(x0, y0);
    v13 = _mm_add_epi32(x1, y1);
    v12 = _mm_add_epi32(v12, rnding);
    v13 = _mm_add_epi32(v13, rnding);
    v12 = _mm_srai_epi32(v12, bit);
    v13 = _mm_srai_epi32(v13, bit);

    // stage 7
    out[2 * 0 * col_num]     = v2;
    out[2 * 0 * col_num + 1] = v3;
    out[2 * 1 * col_num]     = v12;
    out[2 * 1 * col_num + 1] = v13;
}

static void fidtx8x8_N4_sse4_1(const __m128i* in, __m128i* out, int8_t bit, int32_t col_num) {
    (void)bit;
    out[0] = _mm_slli_epi32(in[0 * col_num], 1);
    out[1] = _mm_slli_epi32(in[0 * col_num + 1], 1);
    out[2] = _mm_slli_epi32(in[2 * col_num], 1);
    out[3] = _mm_slli_epi32(in[2 * col_num + 1], 1);
}

static void fdct16x16_N4_sse4_1(const __m128i* in, __m128i* out, int8_t bit, const int32_t col_num, int32_t size) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospim32 = _mm_set1_epi32(-cospi[32]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospim48 = _mm_set1_epi32(-cospi[48]);
    const __m128i  cospim16 = _mm_set1_epi32(-cospi[16]);
    const __m128i  cospi56  = _mm_set1_epi32(cospi[56]);
    const __m128i  cospi8   = _mm_set1_epi32(cospi[8]);
    const __m128i  cospi60  = _mm_set1_epi32(cospi[60]);
    const __m128i  cospi4   = _mm_set1_epi32(cospi[4]);
    const __m128i  cospi12  = _mm_set1_epi32(cospi[12]);
    const __m128i  cospi52  = _mm_set1_epi32(cospi[52]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    __m128i        u[16], v[16], x;
    int32_t        col;

    for (col = 0; col < size; ++col) {
        // stage 0
        // stage 1
        u[0]  = _mm_add_epi32(in[0 * col_num + col], in[15 * col_num + col]);
        u[15] = _mm_sub_epi32(in[0 * col_num + col], in[15 * col_num + col]);
        u[1]  = _mm_add_epi32(in[1 * col_num + col], in[14 * col_num + col]);
        u[14] = _mm_sub_epi32(in[1 * col_num + col], in[14 * col_num + col]);
        u[2]  = _mm_add_epi32(in[2 * col_num + col], in[13 * col_num + col]);
        u[13] = _mm_sub_epi32(in[2 * col_num + col], in[13 * col_num + col]);
        u[3]  = _mm_add_epi32(in[3 * col_num + col], in[12 * col_num + col]);
        u[12] = _mm_sub_epi32(in[3 * col_num + col], in[12 * col_num + col]);
        u[4]  = _mm_add_epi32(in[4 * col_num + col], in[11 * col_num + col]);
        u[11] = _mm_sub_epi32(in[4 * col_num + col], in[11 * col_num + col]);
        u[5]  = _mm_add_epi32(in[5 * col_num + col], in[10 * col_num + col]);
        u[10] = _mm_sub_epi32(in[5 * col_num + col], in[10 * col_num + col]);
        u[6]  = _mm_add_epi32(in[6 * col_num + col], in[9 * col_num + col]);
        u[9]  = _mm_sub_epi32(in[6 * col_num + col], in[9 * col_num + col]);
        u[7]  = _mm_add_epi32(in[7 * col_num + col], in[8 * col_num + col]);
        u[8]  = _mm_sub_epi32(in[7 * col_num + col], in[8 * col_num + col]);

        // stage 2
        v[0] = _mm_add_epi32(u[0], u[7]);
        v[7] = _mm_sub_epi32(u[0], u[7]);
        v[1] = _mm_add_epi32(u[1], u[6]);
        v[6] = _mm_sub_epi32(u[1], u[6]);
        v[2] = _mm_add_epi32(u[2], u[5]);
        v[5] = _mm_sub_epi32(u[2], u[5]);
        v[3] = _mm_add_epi32(u[3], u[4]);
        v[4] = _mm_sub_epi32(u[3], u[4]);
        v[8] = u[8];
        v[9] = u[9];

        v[10] = _mm_mullo_epi32(u[10], cospim32);
        x     = _mm_mullo_epi32(u[13], cospi32);
        v[10] = _mm_add_epi32(v[10], x);
        v[10] = _mm_add_epi32(v[10], rnding);
        v[10] = _mm_srai_epi32(v[10], bit);

        v[13] = _mm_mullo_epi32(u[10], cospi32);
        x     = _mm_mullo_epi32(u[13], cospim32);
        v[13] = _mm_sub_epi32(v[13], x);
        v[13] = _mm_add_epi32(v[13], rnding);
        v[13] = _mm_srai_epi32(v[13], bit);

        v[11] = _mm_mullo_epi32(u[11], cospim32);
        x     = _mm_mullo_epi32(u[12], cospi32);
        v[11] = _mm_add_epi32(v[11], x);
        v[11] = _mm_add_epi32(v[11], rnding);
        v[11] = _mm_srai_epi32(v[11], bit);

        v[12] = _mm_mullo_epi32(u[11], cospi32);
        x     = _mm_mullo_epi32(u[12], cospim32);
        v[12] = _mm_sub_epi32(v[12], x);
        v[12] = _mm_add_epi32(v[12], rnding);
        v[12] = _mm_srai_epi32(v[12], bit);
        v[14] = u[14];
        v[15] = u[15];

        // stage 3
        u[0] = _mm_add_epi32(v[0], v[3]);
        u[1] = _mm_add_epi32(v[1], v[2]);
        u[4] = v[4];

        u[5] = _mm_mullo_epi32(v[5], cospim32);
        x    = _mm_mullo_epi32(v[6], cospi32);
        u[5] = _mm_add_epi32(u[5], x);
        u[5] = _mm_add_epi32(u[5], rnding);
        u[5] = _mm_srai_epi32(u[5], bit);

        u[6] = _mm_mullo_epi32(v[5], cospi32);
        x    = _mm_mullo_epi32(v[6], cospim32);
        u[6] = _mm_sub_epi32(u[6], x);
        u[6] = _mm_add_epi32(u[6], rnding);
        u[6] = _mm_srai_epi32(u[6], bit);

        u[7]  = v[7];
        u[8]  = _mm_add_epi32(v[8], v[11]);
        u[11] = _mm_sub_epi32(v[8], v[11]);
        u[9]  = _mm_add_epi32(v[9], v[10]);
        u[10] = _mm_sub_epi32(v[9], v[10]);
        u[12] = _mm_sub_epi32(v[15], v[12]);
        u[15] = _mm_add_epi32(v[15], v[12]);
        u[13] = _mm_sub_epi32(v[14], v[13]);
        u[14] = _mm_add_epi32(v[14], v[13]);

        // stage 4
        u[0] = _mm_mullo_epi32(u[0], cospi32);
        u[1] = _mm_mullo_epi32(u[1], cospi32);
        v[0] = _mm_add_epi32(u[0], u[1]);
        v[0] = _mm_add_epi32(v[0], rnding);
        v[0] = _mm_srai_epi32(v[0], bit);

        v[4] = _mm_add_epi32(u[4], u[5]);
        v[7] = _mm_add_epi32(u[7], u[6]);
        v[8] = u[8];

        v[9] = _mm_mullo_epi32(u[9], cospim16);
        x    = _mm_mullo_epi32(u[14], cospi48);
        v[9] = _mm_add_epi32(v[9], x);
        v[9] = _mm_add_epi32(v[9], rnding);
        v[9] = _mm_srai_epi32(v[9], bit);

        v[14] = _mm_mullo_epi32(u[9], cospi48);
        x     = _mm_mullo_epi32(u[14], cospim16);
        v[14] = _mm_sub_epi32(v[14], x);
        v[14] = _mm_add_epi32(v[14], rnding);
        v[14] = _mm_srai_epi32(v[14], bit);

        v[10] = _mm_mullo_epi32(u[10], cospim48);
        x     = _mm_mullo_epi32(u[13], cospim16);
        v[10] = _mm_add_epi32(v[10], x);
        v[10] = _mm_add_epi32(v[10], rnding);
        v[10] = _mm_srai_epi32(v[10], bit);

        v[13] = _mm_mullo_epi32(u[10], cospim16);
        x     = _mm_mullo_epi32(u[13], cospim48);
        v[13] = _mm_sub_epi32(v[13], x);
        v[13] = _mm_add_epi32(v[13], rnding);
        v[13] = _mm_srai_epi32(v[13], bit);

        v[11] = u[11];
        v[12] = u[12];
        v[15] = u[15];

        // stage 5
        u[0] = v[0];

        u[4] = _mm_mullo_epi32(v[4], cospi56);
        x    = _mm_mullo_epi32(v[7], cospi8);
        u[4] = _mm_add_epi32(u[4], x);
        u[4] = _mm_add_epi32(u[4], rnding);
        u[4] = _mm_srai_epi32(u[4], bit);

        u[8]  = _mm_add_epi32(v[8], v[9]);
        u[11] = _mm_add_epi32(v[11], v[10]);
        u[12] = _mm_add_epi32(v[12], v[13]);
        u[15] = _mm_add_epi32(v[15], v[14]);

        // stage 6
        v[0] = u[0];
        v[4] = u[4];

        v[8] = _mm_mullo_epi32(u[8], cospi60);
        x    = _mm_mullo_epi32(u[15], cospi4);
        v[8] = _mm_add_epi32(v[8], x);
        v[8] = _mm_add_epi32(v[8], rnding);
        v[8] = _mm_srai_epi32(v[8], bit);

        v[12] = _mm_mullo_epi32(u[11], cospi52);
        x     = _mm_mullo_epi32(u[12], cospi12);
        v[12] = _mm_sub_epi32(x, v[12]);
        v[12] = _mm_add_epi32(v[12], rnding);
        v[12] = _mm_srai_epi32(v[12], bit);

        out[0 * col_num + col] = v[0];
        out[1 * col_num + col] = v[8];
        out[2 * col_num + col] = v[4];
        out[3 * col_num + col] = v[12];
    }
}

static void fadst16x16_N4_sse4_1(const __m128i* in, __m128i* out, int8_t bit, const int32_t col_num, int32_t size) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospi16  = _mm_set1_epi32(cospi[16]);
    const __m128i  cospim16 = _mm_set1_epi32(-cospi[16]);
    const __m128i  cospim48 = _mm_set1_epi32(-cospi[48]);
    const __m128i  cospi8   = _mm_set1_epi32(cospi[8]);
    const __m128i  cospi56  = _mm_set1_epi32(cospi[56]);
    const __m128i  cospim56 = _mm_set1_epi32(-cospi[56]);
    const __m128i  cospim8  = _mm_set1_epi32(-cospi[8]);
    const __m128i  cospi24  = _mm_set1_epi32(cospi[24]);
    const __m128i  cospim24 = _mm_set1_epi32(-cospi[24]);
    const __m128i  cospim40 = _mm_set1_epi32(-cospi[40]);
    const __m128i  cospi40  = _mm_set1_epi32(cospi[40]);
    const __m128i  cospi62  = _mm_set1_epi32(cospi[62]);
    const __m128i  cospim2  = _mm_set1_epi32(-cospi[2]);
    const __m128i  cospi54  = _mm_set1_epi32(cospi[54]);
    const __m128i  cospim10 = _mm_set1_epi32(-cospi[10]);
    const __m128i  cospi50  = _mm_set1_epi32(cospi[50]);
    const __m128i  cospi14  = _mm_set1_epi32(cospi[14]);
    const __m128i  cospi58  = _mm_set1_epi32(cospi[58]);
    const __m128i  cospi6   = _mm_set1_epi32(cospi[6]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    const __m128i  zero     = _mm_setzero_si128();

    __m128i u[16], v[16], x, y;
    int32_t col;

    for (col = 0; col < size; ++col) {
        // stage 0
        // stage 1
        u[0]  = in[0 * col_num + col];
        u[1]  = _mm_sub_epi32(zero, in[15 * col_num + col]);
        u[2]  = _mm_sub_epi32(zero, in[7 * col_num + col]);
        u[3]  = in[8 * col_num + col];
        u[4]  = _mm_sub_epi32(zero, in[3 * col_num + col]);
        u[5]  = in[12 * col_num + col];
        u[6]  = in[4 * col_num + col];
        u[7]  = _mm_sub_epi32(zero, in[11 * col_num + col]);
        u[8]  = _mm_sub_epi32(zero, in[1 * col_num + col]);
        u[9]  = in[14 * col_num + col];
        u[10] = in[6 * col_num + col];
        u[11] = _mm_sub_epi32(zero, in[9 * col_num + col]);
        u[12] = in[2 * col_num + col];
        u[13] = _mm_sub_epi32(zero, in[13 * col_num + col]);
        u[14] = _mm_sub_epi32(zero, in[5 * col_num + col]);
        u[15] = in[10 * col_num + col];

        // stage 2
        v[0] = u[0];
        v[1] = u[1];

        x    = _mm_mullo_epi32(u[2], cospi32);
        y    = _mm_mullo_epi32(u[3], cospi32);
        v[2] = _mm_add_epi32(x, y);
        v[2] = _mm_add_epi32(v[2], rnding);
        v[2] = _mm_srai_epi32(v[2], bit);

        v[3] = _mm_sub_epi32(x, y);
        v[3] = _mm_add_epi32(v[3], rnding);
        v[3] = _mm_srai_epi32(v[3], bit);

        v[4] = u[4];
        v[5] = u[5];

        x    = _mm_mullo_epi32(u[6], cospi32);
        y    = _mm_mullo_epi32(u[7], cospi32);
        v[6] = _mm_add_epi32(x, y);
        v[6] = _mm_add_epi32(v[6], rnding);
        v[6] = _mm_srai_epi32(v[6], bit);

        v[7] = _mm_sub_epi32(x, y);
        v[7] = _mm_add_epi32(v[7], rnding);
        v[7] = _mm_srai_epi32(v[7], bit);

        v[8] = u[8];
        v[9] = u[9];

        x     = _mm_mullo_epi32(u[10], cospi32);
        y     = _mm_mullo_epi32(u[11], cospi32);
        v[10] = _mm_add_epi32(x, y);
        v[10] = _mm_add_epi32(v[10], rnding);
        v[10] = _mm_srai_epi32(v[10], bit);

        v[11] = _mm_sub_epi32(x, y);
        v[11] = _mm_add_epi32(v[11], rnding);
        v[11] = _mm_srai_epi32(v[11], bit);

        v[12] = u[12];
        v[13] = u[13];

        x     = _mm_mullo_epi32(u[14], cospi32);
        y     = _mm_mullo_epi32(u[15], cospi32);
        v[14] = _mm_add_epi32(x, y);
        v[14] = _mm_add_epi32(v[14], rnding);
        v[14] = _mm_srai_epi32(v[14], bit);

        v[15] = _mm_sub_epi32(x, y);
        v[15] = _mm_add_epi32(v[15], rnding);
        v[15] = _mm_srai_epi32(v[15], bit);

        // stage 3
        u[0]  = _mm_add_epi32(v[0], v[2]);
        u[1]  = _mm_add_epi32(v[1], v[3]);
        u[2]  = _mm_sub_epi32(v[0], v[2]);
        u[3]  = _mm_sub_epi32(v[1], v[3]);
        u[4]  = _mm_add_epi32(v[4], v[6]);
        u[5]  = _mm_add_epi32(v[5], v[7]);
        u[6]  = _mm_sub_epi32(v[4], v[6]);
        u[7]  = _mm_sub_epi32(v[5], v[7]);
        u[8]  = _mm_add_epi32(v[8], v[10]);
        u[9]  = _mm_add_epi32(v[9], v[11]);
        u[10] = _mm_sub_epi32(v[8], v[10]);
        u[11] = _mm_sub_epi32(v[9], v[11]);
        u[12] = _mm_add_epi32(v[12], v[14]);
        u[13] = _mm_add_epi32(v[13], v[15]);
        u[14] = _mm_sub_epi32(v[12], v[14]);
        u[15] = _mm_sub_epi32(v[13], v[15]);

        // stage 4
        v[0]  = u[0];
        v[1]  = u[1];
        v[2]  = u[2];
        v[3]  = u[3];
        v[4]  = half_btf_small(&cospi16, &u[4], &cospi48, &u[5], &rnding, bit);
        v[5]  = half_btf_small(&cospi48, &u[4], &cospim16, &u[5], &rnding, bit);
        v[6]  = half_btf_small(&cospim48, &u[6], &cospi16, &u[7], &rnding, bit);
        v[7]  = half_btf_small(&cospi16, &u[6], &cospi48, &u[7], &rnding, bit);
        v[8]  = u[8];
        v[9]  = u[9];
        v[10] = u[10];
        v[11] = u[11];
        v[12] = half_btf_small(&cospi16, &u[12], &cospi48, &u[13], &rnding, bit);
        v[13] = half_btf_small(&cospi48, &u[12], &cospim16, &u[13], &rnding, bit);
        v[14] = half_btf_small(&cospim48, &u[14], &cospi16, &u[15], &rnding, bit);
        v[15] = half_btf_small(&cospi16, &u[14], &cospi48, &u[15], &rnding, bit);

        // stage 5
        u[0]  = _mm_add_epi32(v[0], v[4]);
        u[1]  = _mm_add_epi32(v[1], v[5]);
        u[2]  = _mm_add_epi32(v[2], v[6]);
        u[3]  = _mm_add_epi32(v[3], v[7]);
        u[4]  = _mm_sub_epi32(v[0], v[4]);
        u[5]  = _mm_sub_epi32(v[1], v[5]);
        u[6]  = _mm_sub_epi32(v[2], v[6]);
        u[7]  = _mm_sub_epi32(v[3], v[7]);
        u[8]  = _mm_add_epi32(v[8], v[12]);
        u[9]  = _mm_add_epi32(v[9], v[13]);
        u[10] = _mm_add_epi32(v[10], v[14]);
        u[11] = _mm_add_epi32(v[11], v[15]);
        u[12] = _mm_sub_epi32(v[8], v[12]);
        u[13] = _mm_sub_epi32(v[9], v[13]);
        u[14] = _mm_sub_epi32(v[10], v[14]);
        u[15] = _mm_sub_epi32(v[11], v[15]);

        // stage 6
        v[0]  = u[0];
        v[1]  = u[1];
        v[2]  = u[2];
        v[3]  = u[3];
        v[4]  = u[4];
        v[5]  = u[5];
        v[6]  = u[6];
        v[7]  = u[7];
        v[8]  = half_btf_small(&cospi8, &u[8], &cospi56, &u[9], &rnding, bit);
        v[9]  = half_btf_small(&cospi56, &u[8], &cospim8, &u[9], &rnding, bit);
        v[10] = half_btf_small(&cospi40, &u[10], &cospi24, &u[11], &rnding, bit);
        v[11] = half_btf_small(&cospi24, &u[10], &cospim40, &u[11], &rnding, bit);
        v[12] = half_btf_small(&cospim56, &u[12], &cospi8, &u[13], &rnding, bit);
        v[13] = half_btf_small(&cospi8, &u[12], &cospi56, &u[13], &rnding, bit);
        v[14] = half_btf_small(&cospim24, &u[14], &cospi40, &u[15], &rnding, bit);
        v[15] = half_btf_small(&cospi40, &u[14], &cospi24, &u[15], &rnding, bit);

        // stage 7
        u[0]  = _mm_add_epi32(v[0], v[8]);
        u[1]  = _mm_add_epi32(v[1], v[9]);
        u[2]  = _mm_add_epi32(v[2], v[10]);
        u[3]  = _mm_add_epi32(v[3], v[11]);
        u[12] = _mm_sub_epi32(v[4], v[12]);
        u[13] = _mm_sub_epi32(v[5], v[13]);
        u[14] = _mm_sub_epi32(v[6], v[14]);
        u[15] = _mm_sub_epi32(v[7], v[15]);

        // stage 8
        v[1]  = half_btf_small(&cospi62, &u[0], &cospim2, &u[1], &rnding, bit);
        v[3]  = half_btf_small(&cospi54, &u[2], &cospim10, &u[3], &rnding, bit);
        v[12] = half_btf_small(&cospi50, &u[12], &cospi14, &u[13], &rnding, bit);
        v[14] = half_btf_small(&cospi58, &u[14], &cospi6, &u[15], &rnding, bit);

        // stage 9
        out[0 * col_num + col] = v[1];
        out[1 * col_num + col] = v[14];
        out[2 * col_num + col] = v[3];
        out[3 * col_num + col] = v[12];
    }
}

static void fidtx16x16_N4_sse4_1(const __m128i* in, __m128i* out, int8_t bit, int32_t col_num, int32_t step) {
    (void)bit;
    const int32_t bits     = 12; // new_sqrt2_bits = 12
    const int32_t sqrt     = 2 * 5793; // 2 * new_sqrt2
    const __m128i newsqrt  = _mm_set1_epi32(sqrt);
    const __m128i rounding = _mm_set1_epi32(1 << (bits - 1));
    __m128i       temp0, temp1;
    int32_t       num_iters = 16 * col_num;
    for (int32_t i = 0; i < num_iters / 4; i += step) {
        temp0          = _mm_mullo_epi32(in[2 * i], newsqrt);
        temp1          = _mm_mullo_epi32(in[2 * i + 1], newsqrt);
        temp0          = _mm_add_epi32(temp0, rounding);
        temp1          = _mm_add_epi32(temp1, rounding);
        out[2 * i]     = _mm_srai_epi32(temp0, bits);
        out[2 * i + 1] = _mm_srai_epi32(temp1, bits);
    }
}

static void av1_fdct32_new_N4_sse4_1(const __m128i* input, __m128i* output, int8_t cos_bit, const int32_t col_num,
                                     const int32_t stride) {
    const int32_t* cospi      = cospi_arr(cos_bit);
    const __m128i  __rounding = _mm_set1_epi32(1 << (cos_bit - 1));
    const int32_t  columns    = col_num >> 2;

    __m128i cospi_m32 = _mm_set1_epi32(-cospi[32]);
    __m128i cospi_p32 = _mm_set1_epi32(cospi[32]);
    __m128i cospi_m16 = _mm_set1_epi32(-cospi[16]);
    __m128i cospi_p48 = _mm_set1_epi32(cospi[48]);
    __m128i cospi_m48 = _mm_set1_epi32(-cospi[48]);
    __m128i cospi_m08 = _mm_set1_epi32(-cospi[8]);
    __m128i cospi_p56 = _mm_set1_epi32(cospi[56]);
    __m128i cospi_m56 = _mm_set1_epi32(-cospi[56]);
    __m128i cospi_m40 = _mm_set1_epi32(-cospi[40]);
    __m128i cospi_p24 = _mm_set1_epi32(cospi[24]);
    __m128i cospi_m24 = _mm_set1_epi32(-cospi[24]);
    __m128i cospi_p08 = _mm_set1_epi32(cospi[8]);
    __m128i cospi_p04 = _mm_set1_epi32(cospi[4]);
    __m128i cospi_p60 = _mm_set1_epi32(cospi[60]);
    __m128i cospi_m52 = _mm_set1_epi32(-cospi[52]);
    __m128i cospi_p12 = _mm_set1_epi32(cospi[12]);
    __m128i cospi_p02 = _mm_set1_epi32(cospi[2]);
    __m128i cospi_p06 = _mm_set1_epi32(cospi[6]);
    __m128i cospi_p62 = _mm_set1_epi32(cospi[62]);
    __m128i cospi_m50 = _mm_set1_epi32(-cospi[50]);
    __m128i cospi_p14 = _mm_set1_epi32(cospi[14]);
    __m128i cospi_p10 = _mm_set1_epi32(cospi[10]);
    __m128i cospi_p54 = _mm_set1_epi32(cospi[54]);
    __m128i cospi_m58 = _mm_set1_epi32(-cospi[58]);

    __m128i buf0[32];
    __m128i buf1[32];

    for (int32_t col = 0; col < columns; col++) {
        const __m128i* in  = &input[col];
        __m128i*       out = &output[col];

        // stage 0
        // stage 1
        buf1[0]  = _mm_add_epi32(in[2 * 0 * stride], in[2 * 31 * stride]);
        buf1[31] = _mm_sub_epi32(in[2 * 0 * stride], in[2 * 31 * stride]);
        buf1[1]  = _mm_add_epi32(in[2 * 1 * stride], in[2 * 30 * stride]);
        buf1[30] = _mm_sub_epi32(in[2 * 1 * stride], in[2 * 30 * stride]);
        buf1[2]  = _mm_add_epi32(in[2 * 2 * stride], in[2 * 29 * stride]);
        buf1[29] = _mm_sub_epi32(in[2 * 2 * stride], in[2 * 29 * stride]);
        buf1[3]  = _mm_add_epi32(in[2 * 3 * stride], in[2 * 28 * stride]);
        buf1[28] = _mm_sub_epi32(in[2 * 3 * stride], in[2 * 28 * stride]);
        buf1[4]  = _mm_add_epi32(in[2 * 4 * stride], in[2 * 27 * stride]);
        buf1[27] = _mm_sub_epi32(in[2 * 4 * stride], in[2 * 27 * stride]);
        buf1[5]  = _mm_add_epi32(in[2 * 5 * stride], in[2 * 26 * stride]);
        buf1[26] = _mm_sub_epi32(in[2 * 5 * stride], in[2 * 26 * stride]);
        buf1[6]  = _mm_add_epi32(in[2 * 6 * stride], in[2 * 25 * stride]);
        buf1[25] = _mm_sub_epi32(in[2 * 6 * stride], in[2 * 25 * stride]);
        buf1[7]  = _mm_add_epi32(in[2 * 7 * stride], in[2 * 24 * stride]);
        buf1[24] = _mm_sub_epi32(in[2 * 7 * stride], in[2 * 24 * stride]);
        buf1[8]  = _mm_add_epi32(in[2 * 8 * stride], in[2 * 23 * stride]);
        buf1[23] = _mm_sub_epi32(in[2 * 8 * stride], in[2 * 23 * stride]);
        buf1[9]  = _mm_add_epi32(in[2 * 9 * stride], in[2 * 22 * stride]);
        buf1[22] = _mm_sub_epi32(in[2 * 9 * stride], in[2 * 22 * stride]);
        buf1[10] = _mm_add_epi32(in[2 * 10 * stride], in[2 * 21 * stride]);
        buf1[21] = _mm_sub_epi32(in[2 * 10 * stride], in[2 * 21 * stride]);
        buf1[11] = _mm_add_epi32(in[2 * 11 * stride], in[2 * 20 * stride]);
        buf1[20] = _mm_sub_epi32(in[2 * 11 * stride], in[2 * 20 * stride]);
        buf1[12] = _mm_add_epi32(in[2 * 12 * stride], in[2 * 19 * stride]);
        buf1[19] = _mm_sub_epi32(in[2 * 12 * stride], in[2 * 19 * stride]);
        buf1[13] = _mm_add_epi32(in[2 * 13 * stride], in[2 * 18 * stride]);
        buf1[18] = _mm_sub_epi32(in[2 * 13 * stride], in[2 * 18 * stride]);
        buf1[14] = _mm_add_epi32(in[2 * 14 * stride], in[2 * 17 * stride]);
        buf1[17] = _mm_sub_epi32(in[2 * 14 * stride], in[2 * 17 * stride]);
        buf1[15] = _mm_add_epi32(in[2 * 15 * stride], in[2 * 16 * stride]);
        buf1[16] = _mm_sub_epi32(in[2 * 15 * stride], in[2 * 16 * stride]);

        // stage 2
        buf0[0]  = _mm_add_epi32(buf1[0], buf1[15]);
        buf0[15] = _mm_sub_epi32(buf1[0], buf1[15]);
        buf0[1]  = _mm_add_epi32(buf1[1], buf1[14]);
        buf0[14] = _mm_sub_epi32(buf1[1], buf1[14]);
        buf0[2]  = _mm_add_epi32(buf1[2], buf1[13]);
        buf0[13] = _mm_sub_epi32(buf1[2], buf1[13]);
        buf0[3]  = _mm_add_epi32(buf1[3], buf1[12]);
        buf0[12] = _mm_sub_epi32(buf1[3], buf1[12]);
        buf0[4]  = _mm_add_epi32(buf1[4], buf1[11]);
        buf0[11] = _mm_sub_epi32(buf1[4], buf1[11]);
        buf0[5]  = _mm_add_epi32(buf1[5], buf1[10]);
        buf0[10] = _mm_sub_epi32(buf1[5], buf1[10]);
        buf0[6]  = _mm_add_epi32(buf1[6], buf1[9]);
        buf0[9]  = _mm_sub_epi32(buf1[6], buf1[9]);
        buf0[7]  = _mm_add_epi32(buf1[7], buf1[8]);
        buf0[8]  = _mm_sub_epi32(buf1[7], buf1[8]);
        buf0[16] = buf1[16];
        buf0[17] = buf1[17];
        buf0[18] = buf1[18];
        buf0[19] = buf1[19];
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, buf1[20], buf1[27], buf0[20], buf0[27], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, buf1[21], buf1[26], buf0[21], buf0[26], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, buf1[22], buf1[25], buf0[22], buf0[25], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, buf1[23], buf1[24], buf0[23], buf0[24], __rounding, cos_bit);
        buf0[28] = buf1[28];
        buf0[29] = buf1[29];
        buf0[30] = buf1[30];
        buf0[31] = buf1[31];

        // stage 3
        buf1[0] = _mm_add_epi32(buf0[0], buf0[7]);
        buf1[7] = _mm_sub_epi32(buf0[0], buf0[7]);
        buf1[1] = _mm_add_epi32(buf0[1], buf0[6]);
        buf1[6] = _mm_sub_epi32(buf0[1], buf0[6]);
        buf1[2] = _mm_add_epi32(buf0[2], buf0[5]);
        buf1[5] = _mm_sub_epi32(buf0[2], buf0[5]);
        buf1[3] = _mm_add_epi32(buf0[3], buf0[4]);
        buf1[4] = _mm_sub_epi32(buf0[3], buf0[4]);
        buf1[8] = buf0[8];
        buf1[9] = buf0[9];
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, buf0[10], buf0[13], buf1[10], buf1[13], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, buf0[11], buf0[12], buf1[11], buf1[12], __rounding, cos_bit);
        buf1[14] = buf0[14];
        buf1[15] = buf0[15];
        buf1[16] = _mm_add_epi32(buf0[16], buf0[23]);
        buf1[23] = _mm_sub_epi32(buf0[16], buf0[23]);
        buf1[17] = _mm_add_epi32(buf0[17], buf0[22]);
        buf1[22] = _mm_sub_epi32(buf0[17], buf0[22]);
        buf1[18] = _mm_add_epi32(buf0[18], buf0[21]);
        buf1[21] = _mm_sub_epi32(buf0[18], buf0[21]);
        buf1[19] = _mm_add_epi32(buf0[19], buf0[20]);
        buf1[20] = _mm_sub_epi32(buf0[19], buf0[20]);
        buf1[24] = _mm_sub_epi32(buf0[31], buf0[24]);
        buf1[31] = _mm_add_epi32(buf0[31], buf0[24]);
        buf1[25] = _mm_sub_epi32(buf0[30], buf0[25]);
        buf1[30] = _mm_add_epi32(buf0[30], buf0[25]);
        buf1[26] = _mm_sub_epi32(buf0[29], buf0[26]);
        buf1[29] = _mm_add_epi32(buf0[29], buf0[26]);
        buf1[27] = _mm_sub_epi32(buf0[28], buf0[27]);
        buf1[28] = _mm_add_epi32(buf0[28], buf0[27]);

        // stage 4
        buf0[0] = _mm_add_epi32(buf1[0], buf1[3]);
        buf0[1] = _mm_add_epi32(buf1[1], buf1[2]);
        buf0[4] = buf1[4];
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, buf1[5], buf1[6], buf0[5], buf0[6], __rounding, cos_bit);
        buf0[7]  = buf1[7];
        buf0[8]  = _mm_add_epi32(buf1[8], buf1[11]);
        buf0[11] = _mm_sub_epi32(buf1[8], buf1[11]);
        buf0[9]  = _mm_add_epi32(buf1[9], buf1[10]);
        buf0[10] = _mm_sub_epi32(buf1[9], buf1[10]);
        buf0[12] = _mm_sub_epi32(buf1[15], buf1[12]);
        buf0[15] = _mm_add_epi32(buf1[15], buf1[12]);
        buf0[13] = _mm_sub_epi32(buf1[14], buf1[13]);
        buf0[14] = _mm_add_epi32(buf1[14], buf1[13]);
        buf0[16] = buf1[16];
        buf0[17] = buf1[17];
        btf_32_type0_sse4_1_new(cospi_m16, cospi_p48, buf1[18], buf1[29], buf0[18], buf0[29], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m16, cospi_p48, buf1[19], buf1[28], buf0[19], buf0[28], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m48, cospi_m16, buf1[20], buf1[27], buf0[20], buf0[27], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m48, cospi_m16, buf1[21], buf1[26], buf0[21], buf0[26], __rounding, cos_bit);
        buf0[22] = buf1[22];
        buf0[23] = buf1[23];
        buf0[24] = buf1[24];
        buf0[25] = buf1[25];
        buf0[30] = buf1[30];
        buf0[31] = buf1[31];

        // stage 5
        buf1[0] = half_btf_small(&cospi_p32, &buf0[0], &cospi_p32, &buf0[1], &__rounding, cos_bit);
        buf1[4] = _mm_add_epi32(buf0[4], buf0[5]);
        buf1[7] = _mm_add_epi32(buf0[7], buf0[6]);
        buf1[8] = buf0[8];
        btf_32_type0_sse4_1_new(cospi_m16, cospi_p48, buf0[9], buf0[14], buf1[9], buf1[14], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m48, cospi_m16, buf0[10], buf0[13], buf1[10], buf1[13], __rounding, cos_bit);
        buf1[11] = buf0[11];
        buf1[12] = buf0[12];
        buf1[15] = buf0[15];
        buf1[16] = _mm_add_epi32(buf0[16], buf0[19]);
        buf1[19] = _mm_sub_epi32(buf0[16], buf0[19]);
        buf1[17] = _mm_add_epi32(buf0[17], buf0[18]);
        buf1[18] = _mm_sub_epi32(buf0[17], buf0[18]);
        buf1[20] = _mm_sub_epi32(buf0[23], buf0[20]);
        buf1[23] = _mm_add_epi32(buf0[23], buf0[20]);
        buf1[21] = _mm_sub_epi32(buf0[22], buf0[21]);
        buf1[22] = _mm_add_epi32(buf0[22], buf0[21]);
        buf1[24] = _mm_add_epi32(buf0[24], buf0[27]);
        buf1[27] = _mm_sub_epi32(buf0[24], buf0[27]);
        buf1[25] = _mm_add_epi32(buf0[25], buf0[26]);
        buf1[26] = _mm_sub_epi32(buf0[25], buf0[26]);
        buf1[28] = _mm_sub_epi32(buf0[31], buf0[28]);
        buf1[31] = _mm_add_epi32(buf0[31], buf0[28]);
        buf1[29] = _mm_sub_epi32(buf0[30], buf0[29]);
        buf1[30] = _mm_add_epi32(buf0[30], buf0[29]);

        // stage 6
        buf0[0]  = buf1[0];
        buf0[4]  = half_btf_small(&cospi_p56, &buf1[4], &cospi_p08, &buf1[7], &__rounding, cos_bit);
        buf0[8]  = _mm_add_epi32(buf1[8], buf1[9]);
        buf0[11] = _mm_add_epi32(buf1[11], buf1[10]);
        buf0[12] = _mm_add_epi32(buf1[12], buf1[13]);
        buf0[15] = _mm_add_epi32(buf1[15], buf1[14]);
        buf0[16] = buf1[16];
        btf_32_type0_sse4_1_new(cospi_m08, cospi_p56, buf1[17], buf1[30], buf0[17], buf0[30], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m56, cospi_m08, buf1[18], buf1[29], buf0[18], buf0[29], __rounding, cos_bit);
        buf0[19] = buf1[19];
        buf0[20] = buf1[20];
        btf_32_type0_sse4_1_new(cospi_m40, cospi_p24, buf1[21], buf1[26], buf0[21], buf0[26], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m24, cospi_m40, buf1[22], buf1[25], buf0[22], buf0[25], __rounding, cos_bit);
        buf0[23] = buf1[23];
        buf0[24] = buf1[24];
        buf0[27] = buf1[27];
        buf0[28] = buf1[28];
        buf0[31] = buf1[31];

        // stage 7
        buf1[0]  = buf0[0];
        buf1[4]  = buf0[4];
        buf1[8]  = half_btf_small(&cospi_p60, &buf0[8], &cospi_p04, &buf0[15], &__rounding, cos_bit);
        buf1[12] = half_btf_small(&cospi_p12, &buf0[12], &cospi_m52, &buf0[11], &__rounding, cos_bit);

        buf1[16] = _mm_add_epi32(buf0[16], buf0[17]);
        buf1[19] = _mm_add_epi32(buf0[19], buf0[18]);
        buf1[20] = _mm_add_epi32(buf0[20], buf0[21]);
        buf1[23] = _mm_add_epi32(buf0[23], buf0[22]);
        buf1[24] = _mm_add_epi32(buf0[24], buf0[25]);
        buf1[27] = _mm_add_epi32(buf0[27], buf0[26]);
        buf1[28] = _mm_add_epi32(buf0[28], buf0[29]);
        buf1[31] = _mm_add_epi32(buf0[31], buf0[30]);

        // stage 8
        buf0[0]  = buf1[0];
        buf0[4]  = buf1[4];
        buf0[8]  = buf1[8];
        buf0[12] = buf1[12];
        buf0[16] = half_btf_small(&cospi_p62, &buf1[16], &cospi_p02, &buf1[31], &__rounding, cos_bit);
        buf0[28] = half_btf_small(&cospi_p14, &buf1[28], &cospi_m50, &buf1[19], &__rounding, cos_bit);
        buf0[20] = half_btf_small(&cospi_p54, &buf1[20], &cospi_p10, &buf1[27], &__rounding, cos_bit);
        buf0[24] = half_btf_small(&cospi_p06, &buf1[24], &cospi_m58, &buf1[23], &__rounding, cos_bit);

        // stage 9
        out[2 * 0 * stride] = buf0[0];
        out[2 * 1 * stride] = buf0[16];
        out[2 * 2 * stride] = buf0[8];
        out[2 * 3 * stride] = buf0[24];
        out[2 * 4 * stride] = buf0[4];
        out[2 * 5 * stride] = buf0[20];
        out[2 * 6 * stride] = buf0[12];
        out[2 * 7 * stride] = buf0[28];
    }
}

static AOM_FORCE_INLINE void fdct32x32_N4_col_sse4_1(const __m128i* input, __m128i* output, const int8_t cos_bit) {
    const int32_t txfm_size   = 32;
    const int32_t num_per_256 = 8;
    int32_t       col_num     = txfm_size / num_per_256;
    av1_fdct32_new_N4_sse4_1(input, output, cos_bit, txfm_size, col_num);
}

static AOM_FORCE_INLINE void fdct32x32_N4_row_sse4_1(const __m128i* input, __m128i* output, const int8_t cos_bit) {
    const int32_t txfm_size   = 32;
    const int32_t num_per_256 = 8;
    int32_t       col_num     = txfm_size / num_per_256;
    av1_fdct32_new_N4_sse4_1(input, output, cos_bit, txfm_size / 4, col_num);
}

static void fidtx_wxh_N4_sse4_1(const __m128i* input, __m128i* output, int32_t size, int32_t step) {
    for (int32_t i = 0; i < size; i += step) {
        output[2 * i]     = _mm_slli_epi32(input[2 * i], 2);
        output[2 * i + 1] = _mm_slli_epi32(input[2 * i + 1], 2);
    }
}

static void av1_fdct64_new_N4_sse4_1(const __m128i* input, __m128i* output, int8_t cos_bit, const int32_t col_num,
                                     const int32_t stride) {
    const int32_t* cospi      = cospi_arr(cos_bit);
    const __m128i  __rounding = _mm_set1_epi32(1 << (cos_bit - 1));
    const int32_t  columns    = col_num >> 2;

    __m128i cospi_m32 = _mm_set1_epi32(-cospi[32]);
    __m128i cospi_p32 = _mm_set1_epi32(cospi[32]);
    __m128i cospi_m16 = _mm_set1_epi32(-cospi[16]);
    __m128i cospi_p48 = _mm_set1_epi32(cospi[48]);
    __m128i cospi_m48 = _mm_set1_epi32(-cospi[48]);
    __m128i cospi_m08 = _mm_set1_epi32(-cospi[8]);
    __m128i cospi_p56 = _mm_set1_epi32(cospi[56]);
    __m128i cospi_m56 = _mm_set1_epi32(-cospi[56]);
    __m128i cospi_m40 = _mm_set1_epi32(-cospi[40]);
    __m128i cospi_p24 = _mm_set1_epi32(cospi[24]);
    __m128i cospi_m24 = _mm_set1_epi32(-cospi[24]);
    __m128i cospi_p08 = _mm_set1_epi32(cospi[8]);
    __m128i cospi_p60 = _mm_set1_epi32(cospi[60]);
    __m128i cospi_p04 = _mm_set1_epi32(cospi[4]);
    __m128i cospi_p28 = _mm_set1_epi32(cospi[28]);
    __m128i cospi_p44 = _mm_set1_epi32(cospi[44]);
    __m128i cospi_p12 = _mm_set1_epi32(cospi[12]);
    __m128i cospi_m04 = _mm_set1_epi32(-cospi[4]);
    __m128i cospi_m60 = _mm_set1_epi32(-cospi[60]);
    __m128i cospi_m36 = _mm_set1_epi32(-cospi[36]);
    __m128i cospi_m28 = _mm_set1_epi32(-cospi[28]);
    __m128i cospi_m20 = _mm_set1_epi32(-cospi[20]);
    __m128i cospi_m44 = _mm_set1_epi32(-cospi[44]);
    __m128i cospi_m52 = _mm_set1_epi32(-cospi[52]);
    __m128i cospi_m12 = _mm_set1_epi32(-cospi[12]);
    __m128i cospi_p62 = _mm_set1_epi32(cospi[62]);
    __m128i cospi_p02 = _mm_set1_epi32(cospi[2]);
    __m128i cospi_p14 = _mm_set1_epi32(cospi[14]);
    __m128i cospi_m50 = _mm_set1_epi32(-cospi[50]);
    __m128i cospi_p54 = _mm_set1_epi32(cospi[54]);
    __m128i cospi_p10 = _mm_set1_epi32(cospi[10]);
    __m128i cospi_p06 = _mm_set1_epi32(cospi[6]);
    __m128i cospi_m58 = _mm_set1_epi32(-cospi[58]);
    __m128i cospi_p63 = _mm_set1_epi32(cospi[63]);
    __m128i cospi_p01 = _mm_set1_epi32(cospi[1]);
    __m128i cospi_p15 = _mm_set1_epi32(cospi[15]);
    __m128i cospi_m49 = _mm_set1_epi32(-cospi[49]);
    __m128i cospi_p55 = _mm_set1_epi32(cospi[55]);
    __m128i cospi_p09 = _mm_set1_epi32(cospi[9]);
    __m128i cospi_p07 = _mm_set1_epi32(cospi[7]);
    __m128i cospi_m57 = _mm_set1_epi32(-cospi[57]);
    __m128i cospi_p59 = _mm_set1_epi32(cospi[59]);
    __m128i cospi_p05 = _mm_set1_epi32(cospi[5]);
    __m128i cospi_p11 = _mm_set1_epi32(cospi[11]);
    __m128i cospi_m53 = _mm_set1_epi32(-cospi[53]);
    __m128i cospi_p51 = _mm_set1_epi32(cospi[51]);
    __m128i cospi_p13 = _mm_set1_epi32(cospi[13]);
    __m128i cospi_p03 = _mm_set1_epi32(cospi[3]);
    __m128i cospi_m61 = _mm_set1_epi32(-cospi[61]);

    for (int32_t col = 0; col < columns; col++) {
        const __m128i* in  = &input[col];
        __m128i*       out = &output[col];

        // stage 1
        __m128i x1[64];
        x1[0]  = _mm_add_epi32(in[2 * 0 * stride], in[2 * 63 * stride]);
        x1[63] = _mm_sub_epi32(in[2 * 0 * stride], in[2 * 63 * stride]);
        x1[1]  = _mm_add_epi32(in[2 * 1 * stride], in[2 * 62 * stride]);
        x1[62] = _mm_sub_epi32(in[2 * 1 * stride], in[2 * 62 * stride]);
        x1[2]  = _mm_add_epi32(in[2 * 2 * stride], in[2 * 61 * stride]);
        x1[61] = _mm_sub_epi32(in[2 * 2 * stride], in[2 * 61 * stride]);
        x1[3]  = _mm_add_epi32(in[2 * 3 * stride], in[2 * 60 * stride]);
        x1[60] = _mm_sub_epi32(in[2 * 3 * stride], in[2 * 60 * stride]);
        x1[4]  = _mm_add_epi32(in[2 * 4 * stride], in[2 * 59 * stride]);
        x1[59] = _mm_sub_epi32(in[2 * 4 * stride], in[2 * 59 * stride]);
        x1[5]  = _mm_add_epi32(in[2 * 5 * stride], in[2 * 58 * stride]);
        x1[58] = _mm_sub_epi32(in[2 * 5 * stride], in[2 * 58 * stride]);
        x1[6]  = _mm_add_epi32(in[2 * 6 * stride], in[2 * 57 * stride]);
        x1[57] = _mm_sub_epi32(in[2 * 6 * stride], in[2 * 57 * stride]);
        x1[7]  = _mm_add_epi32(in[2 * 7 * stride], in[2 * 56 * stride]);
        x1[56] = _mm_sub_epi32(in[2 * 7 * stride], in[2 * 56 * stride]);
        x1[8]  = _mm_add_epi32(in[2 * 8 * stride], in[2 * 55 * stride]);
        x1[55] = _mm_sub_epi32(in[2 * 8 * stride], in[2 * 55 * stride]);
        x1[9]  = _mm_add_epi32(in[2 * 9 * stride], in[2 * 54 * stride]);
        x1[54] = _mm_sub_epi32(in[2 * 9 * stride], in[2 * 54 * stride]);
        x1[10] = _mm_add_epi32(in[2 * 10 * stride], in[2 * 53 * stride]);
        x1[53] = _mm_sub_epi32(in[2 * 10 * stride], in[2 * 53 * stride]);
        x1[11] = _mm_add_epi32(in[2 * 11 * stride], in[2 * 52 * stride]);
        x1[52] = _mm_sub_epi32(in[2 * 11 * stride], in[2 * 52 * stride]);
        x1[12] = _mm_add_epi32(in[2 * 12 * stride], in[2 * 51 * stride]);
        x1[51] = _mm_sub_epi32(in[2 * 12 * stride], in[2 * 51 * stride]);
        x1[13] = _mm_add_epi32(in[2 * 13 * stride], in[2 * 50 * stride]);
        x1[50] = _mm_sub_epi32(in[2 * 13 * stride], in[2 * 50 * stride]);
        x1[14] = _mm_add_epi32(in[2 * 14 * stride], in[2 * 49 * stride]);
        x1[49] = _mm_sub_epi32(in[2 * 14 * stride], in[2 * 49 * stride]);
        x1[15] = _mm_add_epi32(in[2 * 15 * stride], in[2 * 48 * stride]);
        x1[48] = _mm_sub_epi32(in[2 * 15 * stride], in[2 * 48 * stride]);
        x1[16] = _mm_add_epi32(in[2 * 16 * stride], in[2 * 47 * stride]);
        x1[47] = _mm_sub_epi32(in[2 * 16 * stride], in[2 * 47 * stride]);
        x1[17] = _mm_add_epi32(in[2 * 17 * stride], in[2 * 46 * stride]);
        x1[46] = _mm_sub_epi32(in[2 * 17 * stride], in[2 * 46 * stride]);
        x1[18] = _mm_add_epi32(in[2 * 18 * stride], in[2 * 45 * stride]);
        x1[45] = _mm_sub_epi32(in[2 * 18 * stride], in[2 * 45 * stride]);
        x1[19] = _mm_add_epi32(in[2 * 19 * stride], in[2 * 44 * stride]);
        x1[44] = _mm_sub_epi32(in[2 * 19 * stride], in[2 * 44 * stride]);
        x1[20] = _mm_add_epi32(in[2 * 20 * stride], in[2 * 43 * stride]);
        x1[43] = _mm_sub_epi32(in[2 * 20 * stride], in[2 * 43 * stride]);
        x1[21] = _mm_add_epi32(in[2 * 21 * stride], in[2 * 42 * stride]);
        x1[42] = _mm_sub_epi32(in[2 * 21 * stride], in[2 * 42 * stride]);
        x1[22] = _mm_add_epi32(in[2 * 22 * stride], in[2 * 41 * stride]);
        x1[41] = _mm_sub_epi32(in[2 * 22 * stride], in[2 * 41 * stride]);
        x1[23] = _mm_add_epi32(in[2 * 23 * stride], in[2 * 40 * stride]);
        x1[40] = _mm_sub_epi32(in[2 * 23 * stride], in[2 * 40 * stride]);
        x1[24] = _mm_add_epi32(in[2 * 24 * stride], in[2 * 39 * stride]);
        x1[39] = _mm_sub_epi32(in[2 * 24 * stride], in[2 * 39 * stride]);
        x1[25] = _mm_add_epi32(in[2 * 25 * stride], in[2 * 38 * stride]);
        x1[38] = _mm_sub_epi32(in[2 * 25 * stride], in[2 * 38 * stride]);
        x1[26] = _mm_add_epi32(in[2 * 26 * stride], in[2 * 37 * stride]);
        x1[37] = _mm_sub_epi32(in[2 * 26 * stride], in[2 * 37 * stride]);
        x1[27] = _mm_add_epi32(in[2 * 27 * stride], in[2 * 36 * stride]);
        x1[36] = _mm_sub_epi32(in[2 * 27 * stride], in[2 * 36 * stride]);
        x1[28] = _mm_add_epi32(in[2 * 28 * stride], in[2 * 35 * stride]);
        x1[35] = _mm_sub_epi32(in[2 * 28 * stride], in[2 * 35 * stride]);
        x1[29] = _mm_add_epi32(in[2 * 29 * stride], in[2 * 34 * stride]);
        x1[34] = _mm_sub_epi32(in[2 * 29 * stride], in[2 * 34 * stride]);
        x1[30] = _mm_add_epi32(in[2 * 30 * stride], in[2 * 33 * stride]);
        x1[33] = _mm_sub_epi32(in[2 * 30 * stride], in[2 * 33 * stride]);
        x1[31] = _mm_add_epi32(in[2 * 31 * stride], in[2 * 32 * stride]);
        x1[32] = _mm_sub_epi32(in[2 * 31 * stride], in[2 * 32 * stride]);

        // stage 2
        __m128i x2[64];
        x2[0]  = _mm_add_epi32(x1[0], x1[31]);
        x2[31] = _mm_sub_epi32(x1[0], x1[31]);
        x2[1]  = _mm_add_epi32(x1[1], x1[30]);
        x2[30] = _mm_sub_epi32(x1[1], x1[30]);
        x2[2]  = _mm_add_epi32(x1[2], x1[29]);
        x2[29] = _mm_sub_epi32(x1[2], x1[29]);
        x2[3]  = _mm_add_epi32(x1[3], x1[28]);
        x2[28] = _mm_sub_epi32(x1[3], x1[28]);
        x2[4]  = _mm_add_epi32(x1[4], x1[27]);
        x2[27] = _mm_sub_epi32(x1[4], x1[27]);
        x2[5]  = _mm_add_epi32(x1[5], x1[26]);
        x2[26] = _mm_sub_epi32(x1[5], x1[26]);
        x2[6]  = _mm_add_epi32(x1[6], x1[25]);
        x2[25] = _mm_sub_epi32(x1[6], x1[25]);
        x2[7]  = _mm_add_epi32(x1[7], x1[24]);
        x2[24] = _mm_sub_epi32(x1[7], x1[24]);
        x2[8]  = _mm_add_epi32(x1[8], x1[23]);
        x2[23] = _mm_sub_epi32(x1[8], x1[23]);
        x2[9]  = _mm_add_epi32(x1[9], x1[22]);
        x2[22] = _mm_sub_epi32(x1[9], x1[22]);
        x2[10] = _mm_add_epi32(x1[10], x1[21]);
        x2[21] = _mm_sub_epi32(x1[10], x1[21]);
        x2[11] = _mm_add_epi32(x1[11], x1[20]);
        x2[20] = _mm_sub_epi32(x1[11], x1[20]);
        x2[12] = _mm_add_epi32(x1[12], x1[19]);
        x2[19] = _mm_sub_epi32(x1[12], x1[19]);
        x2[13] = _mm_add_epi32(x1[13], x1[18]);
        x2[18] = _mm_sub_epi32(x1[13], x1[18]);
        x2[14] = _mm_add_epi32(x1[14], x1[17]);
        x2[17] = _mm_sub_epi32(x1[14], x1[17]);
        x2[15] = _mm_add_epi32(x1[15], x1[16]);
        x2[16] = _mm_sub_epi32(x1[15], x1[16]);
        x2[32] = x1[32];
        x2[33] = x1[33];
        x2[34] = x1[34];
        x2[35] = x1[35];
        x2[36] = x1[36];
        x2[37] = x1[37];
        x2[38] = x1[38];
        x2[39] = x1[39];
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x1[40], x1[55], x2[40], x2[55], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x1[41], x1[54], x2[41], x2[54], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x1[42], x1[53], x2[42], x2[53], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x1[43], x1[52], x2[43], x2[52], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x1[44], x1[51], x2[44], x2[51], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x1[45], x1[50], x2[45], x2[50], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x1[46], x1[49], x2[46], x2[49], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x1[47], x1[48], x2[47], x2[48], __rounding, cos_bit);
        x2[56] = x1[56];
        x2[57] = x1[57];
        x2[58] = x1[58];
        x2[59] = x1[59];
        x2[60] = x1[60];
        x2[61] = x1[61];
        x2[62] = x1[62];
        x2[63] = x1[63];

        // stage 3
        __m128i x3[64];
        x3[0]  = _mm_add_epi32(x2[0], x2[15]);
        x3[15] = _mm_sub_epi32(x2[0], x2[15]);
        x3[1]  = _mm_add_epi32(x2[1], x2[14]);
        x3[14] = _mm_sub_epi32(x2[1], x2[14]);
        x3[2]  = _mm_add_epi32(x2[2], x2[13]);
        x3[13] = _mm_sub_epi32(x2[2], x2[13]);
        x3[3]  = _mm_add_epi32(x2[3], x2[12]);
        x3[12] = _mm_sub_epi32(x2[3], x2[12]);
        x3[4]  = _mm_add_epi32(x2[4], x2[11]);
        x3[11] = _mm_sub_epi32(x2[4], x2[11]);
        x3[5]  = _mm_add_epi32(x2[5], x2[10]);
        x3[10] = _mm_sub_epi32(x2[5], x2[10]);
        x3[6]  = _mm_add_epi32(x2[6], x2[9]);
        x3[9]  = _mm_sub_epi32(x2[6], x2[9]);
        x3[7]  = _mm_add_epi32(x2[7], x2[8]);
        x3[8]  = _mm_sub_epi32(x2[7], x2[8]);
        x3[16] = x2[16];
        x3[17] = x2[17];
        x3[18] = x2[18];
        x3[19] = x2[19];
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x2[20], x2[27], x3[20], x3[27], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x2[21], x2[26], x3[21], x3[26], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x2[22], x2[25], x3[22], x3[25], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x2[23], x2[24], x3[23], x3[24], __rounding, cos_bit);
        x3[28] = x2[28];
        x3[29] = x2[29];
        x3[30] = x2[30];
        x3[31] = x2[31];
        x3[32] = _mm_add_epi32(x2[32], x2[47]);
        x3[47] = _mm_sub_epi32(x2[32], x2[47]);
        x3[33] = _mm_add_epi32(x2[33], x2[46]);
        x3[46] = _mm_sub_epi32(x2[33], x2[46]);
        x3[34] = _mm_add_epi32(x2[34], x2[45]);
        x3[45] = _mm_sub_epi32(x2[34], x2[45]);
        x3[35] = _mm_add_epi32(x2[35], x2[44]);
        x3[44] = _mm_sub_epi32(x2[35], x2[44]);
        x3[36] = _mm_add_epi32(x2[36], x2[43]);
        x3[43] = _mm_sub_epi32(x2[36], x2[43]);
        x3[37] = _mm_add_epi32(x2[37], x2[42]);
        x3[42] = _mm_sub_epi32(x2[37], x2[42]);
        x3[38] = _mm_add_epi32(x2[38], x2[41]);
        x3[41] = _mm_sub_epi32(x2[38], x2[41]);
        x3[39] = _mm_add_epi32(x2[39], x2[40]);
        x3[40] = _mm_sub_epi32(x2[39], x2[40]);
        x3[48] = _mm_sub_epi32(x2[63], x2[48]);
        x3[63] = _mm_add_epi32(x2[63], x2[48]);
        x3[49] = _mm_sub_epi32(x2[62], x2[49]);
        x3[62] = _mm_add_epi32(x2[62], x2[49]);
        x3[50] = _mm_sub_epi32(x2[61], x2[50]);
        x3[61] = _mm_add_epi32(x2[61], x2[50]);
        x3[51] = _mm_sub_epi32(x2[60], x2[51]);
        x3[60] = _mm_add_epi32(x2[60], x2[51]);
        x3[52] = _mm_sub_epi32(x2[59], x2[52]);
        x3[59] = _mm_add_epi32(x2[59], x2[52]);
        x3[53] = _mm_sub_epi32(x2[58], x2[53]);
        x3[58] = _mm_add_epi32(x2[58], x2[53]);
        x3[54] = _mm_sub_epi32(x2[57], x2[54]);
        x3[57] = _mm_add_epi32(x2[57], x2[54]);
        x3[55] = _mm_sub_epi32(x2[56], x2[55]);
        x3[56] = _mm_add_epi32(x2[56], x2[55]);

        // stage 4
        __m128i x4[64];
        x4[0] = _mm_add_epi32(x3[0], x3[7]);
        x4[7] = _mm_sub_epi32(x3[0], x3[7]);
        x4[1] = _mm_add_epi32(x3[1], x3[6]);
        x4[6] = _mm_sub_epi32(x3[1], x3[6]);
        x4[2] = _mm_add_epi32(x3[2], x3[5]);
        x4[5] = _mm_sub_epi32(x3[2], x3[5]);
        x4[3] = _mm_add_epi32(x3[3], x3[4]);
        x4[4] = _mm_sub_epi32(x3[3], x3[4]);
        x4[8] = x3[8];
        x4[9] = x3[9];
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x3[10], x3[13], x4[10], x4[13], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x3[11], x3[12], x4[11], x4[12], __rounding, cos_bit);
        x4[14] = x3[14];
        x4[15] = x3[15];
        x4[16] = _mm_add_epi32(x3[16], x3[23]);
        x4[23] = _mm_sub_epi32(x3[16], x3[23]);
        x4[17] = _mm_add_epi32(x3[17], x3[22]);
        x4[22] = _mm_sub_epi32(x3[17], x3[22]);
        x4[18] = _mm_add_epi32(x3[18], x3[21]);
        x4[21] = _mm_sub_epi32(x3[18], x3[21]);
        x4[19] = _mm_add_epi32(x3[19], x3[20]);
        x4[20] = _mm_sub_epi32(x3[19], x3[20]);
        x4[24] = _mm_sub_epi32(x3[31], x3[24]);
        x4[31] = _mm_add_epi32(x3[31], x3[24]);
        x4[25] = _mm_sub_epi32(x3[30], x3[25]);
        x4[30] = _mm_add_epi32(x3[30], x3[25]);
        x4[26] = _mm_sub_epi32(x3[29], x3[26]);
        x4[29] = _mm_add_epi32(x3[29], x3[26]);
        x4[27] = _mm_sub_epi32(x3[28], x3[27]);
        x4[28] = _mm_add_epi32(x3[28], x3[27]);
        x4[32] = x3[32];
        x4[33] = x3[33];
        x4[34] = x3[34];
        x4[35] = x3[35];
        btf_32_type0_sse4_1_new(cospi_m16, cospi_p48, x3[36], x3[59], x4[36], x4[59], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m16, cospi_p48, x3[37], x3[58], x4[37], x4[58], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m16, cospi_p48, x3[38], x3[57], x4[38], x4[57], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m16, cospi_p48, x3[39], x3[56], x4[39], x4[56], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m48, cospi_m16, x3[40], x3[55], x4[40], x4[55], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m48, cospi_m16, x3[41], x3[54], x4[41], x4[54], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m48, cospi_m16, x3[42], x3[53], x4[42], x4[53], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m48, cospi_m16, x3[43], x3[52], x4[43], x4[52], __rounding, cos_bit);
        x4[44] = x3[44];
        x4[45] = x3[45];
        x4[46] = x3[46];
        x4[47] = x3[47];
        x4[48] = x3[48];
        x4[49] = x3[49];
        x4[50] = x3[50];
        x4[51] = x3[51];
        x4[60] = x3[60];
        x4[61] = x3[61];
        x4[62] = x3[62];
        x4[63] = x3[63];

        // stage 5
        __m128i x5[64];
        x5[0] = _mm_add_epi32(x4[0], x4[3]);
        x5[1] = _mm_add_epi32(x4[1], x4[2]);
        x5[4] = x4[4];
        btf_32_type0_sse4_1_new(cospi_m32, cospi_p32, x4[5], x4[6], x5[5], x5[6], __rounding, cos_bit);
        x5[7]  = x4[7];
        x5[8]  = _mm_add_epi32(x4[8], x4[11]);
        x5[11] = _mm_sub_epi32(x4[8], x4[11]);
        x5[9]  = _mm_add_epi32(x4[9], x4[10]);
        x5[10] = _mm_sub_epi32(x4[9], x4[10]);
        x5[12] = _mm_sub_epi32(x4[15], x4[12]);
        x5[15] = _mm_add_epi32(x4[15], x4[12]);
        x5[13] = _mm_sub_epi32(x4[14], x4[13]);
        x5[14] = _mm_add_epi32(x4[14], x4[13]);
        x5[16] = x4[16];
        x5[17] = x4[17];
        btf_32_type0_sse4_1_new(cospi_m16, cospi_p48, x4[18], x4[29], x5[18], x5[29], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m16, cospi_p48, x4[19], x4[28], x5[19], x5[28], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m48, cospi_m16, x4[20], x4[27], x5[20], x5[27], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m48, cospi_m16, x4[21], x4[26], x5[21], x5[26], __rounding, cos_bit);
        x5[22] = x4[22];
        x5[23] = x4[23];
        x5[24] = x4[24];
        x5[25] = x4[25];
        x5[30] = x4[30];
        x5[31] = x4[31];
        x5[32] = _mm_add_epi32(x4[32], x4[39]);
        x5[39] = _mm_sub_epi32(x4[32], x4[39]);
        x5[33] = _mm_add_epi32(x4[33], x4[38]);
        x5[38] = _mm_sub_epi32(x4[33], x4[38]);
        x5[34] = _mm_add_epi32(x4[34], x4[37]);
        x5[37] = _mm_sub_epi32(x4[34], x4[37]);
        x5[35] = _mm_add_epi32(x4[35], x4[36]);
        x5[36] = _mm_sub_epi32(x4[35], x4[36]);
        x5[40] = _mm_sub_epi32(x4[47], x4[40]);
        x5[47] = _mm_add_epi32(x4[47], x4[40]);
        x5[41] = _mm_sub_epi32(x4[46], x4[41]);
        x5[46] = _mm_add_epi32(x4[46], x4[41]);
        x5[42] = _mm_sub_epi32(x4[45], x4[42]);
        x5[45] = _mm_add_epi32(x4[45], x4[42]);
        x5[43] = _mm_sub_epi32(x4[44], x4[43]);
        x5[44] = _mm_add_epi32(x4[44], x4[43]);
        x5[48] = _mm_add_epi32(x4[48], x4[55]);
        x5[55] = _mm_sub_epi32(x4[48], x4[55]);
        x5[49] = _mm_add_epi32(x4[49], x4[54]);
        x5[54] = _mm_sub_epi32(x4[49], x4[54]);
        x5[50] = _mm_add_epi32(x4[50], x4[53]);
        x5[53] = _mm_sub_epi32(x4[50], x4[53]);
        x5[51] = _mm_add_epi32(x4[51], x4[52]);
        x5[52] = _mm_sub_epi32(x4[51], x4[52]);
        x5[56] = _mm_sub_epi32(x4[63], x4[56]);
        x5[63] = _mm_add_epi32(x4[63], x4[56]);
        x5[57] = _mm_sub_epi32(x4[62], x4[57]);
        x5[62] = _mm_add_epi32(x4[62], x4[57]);
        x5[58] = _mm_sub_epi32(x4[61], x4[58]);
        x5[61] = _mm_add_epi32(x4[61], x4[58]);
        x5[59] = _mm_sub_epi32(x4[60], x4[59]);
        x5[60] = _mm_add_epi32(x4[60], x4[59]);

        // stage 6
        __m128i x6[64];
        out[2 * 0 * stride] = half_btf_small(&cospi_p32, &x5[0], &cospi_p32, &x5[1], &__rounding, cos_bit);
        x6[4]               = _mm_add_epi32(x5[4], x5[5]);
        x6[7]               = _mm_add_epi32(x5[7], x5[6]);
        x6[8]               = x5[8];
        btf_32_type0_sse4_1_new(cospi_m16, cospi_p48, x5[9], x5[14], x6[9], x6[14], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m48, cospi_m16, x5[10], x5[13], x6[10], x6[13], __rounding, cos_bit);
        x6[11] = x5[11];
        x6[12] = x5[12];
        x6[15] = x5[15];
        x6[16] = _mm_add_epi32(x5[16], x5[19]);
        x6[19] = _mm_sub_epi32(x5[16], x5[19]);
        x6[17] = _mm_add_epi32(x5[17], x5[18]);
        x6[18] = _mm_sub_epi32(x5[17], x5[18]);
        x6[20] = _mm_sub_epi32(x5[23], x5[20]);
        x6[23] = _mm_add_epi32(x5[23], x5[20]);
        x6[21] = _mm_sub_epi32(x5[22], x5[21]);
        x6[22] = _mm_add_epi32(x5[22], x5[21]);
        x6[24] = _mm_add_epi32(x5[24], x5[27]);
        x6[27] = _mm_sub_epi32(x5[24], x5[27]);
        x6[25] = _mm_add_epi32(x5[25], x5[26]);
        x6[26] = _mm_sub_epi32(x5[25], x5[26]);
        x6[28] = _mm_sub_epi32(x5[31], x5[28]);
        x6[31] = _mm_add_epi32(x5[31], x5[28]);
        x6[29] = _mm_sub_epi32(x5[30], x5[29]);
        x6[30] = _mm_add_epi32(x5[30], x5[29]);
        x6[32] = x5[32];
        x6[33] = x5[33];
        btf_32_type0_sse4_1_new(cospi_m08, cospi_p56, x5[34], x5[61], x6[34], x6[61], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m08, cospi_p56, x5[35], x5[60], x6[35], x6[60], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m56, cospi_m08, x5[36], x5[59], x6[36], x6[59], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m56, cospi_m08, x5[37], x5[58], x6[37], x6[58], __rounding, cos_bit);
        x6[38] = x5[38];
        x6[39] = x5[39];
        x6[40] = x5[40];
        x6[41] = x5[41];
        btf_32_type0_sse4_1_new(cospi_m40, cospi_p24, x5[42], x5[53], x6[42], x6[53], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m40, cospi_p24, x5[43], x5[52], x6[43], x6[52], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m24, cospi_m40, x5[44], x5[51], x6[44], x6[51], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m24, cospi_m40, x5[45], x5[50], x6[45], x6[50], __rounding, cos_bit);
        x6[46] = x5[46];
        x6[47] = x5[47];
        x6[48] = x5[48];
        x6[49] = x5[49];
        x6[54] = x5[54];
        x6[55] = x5[55];
        x6[56] = x5[56];
        x6[57] = x5[57];
        x6[62] = x5[62];
        x6[63] = x5[63];

        // stage 7
        __m128i x7[64];
        out[2 * 8 * stride] = half_btf_small(&cospi_p56, &x6[4], &cospi_p08, &x6[7], &__rounding, cos_bit);
        x7[8]               = _mm_add_epi32(x6[8], x6[9]);
        x7[11]              = _mm_add_epi32(x6[11], x6[10]);
        x7[12]              = _mm_add_epi32(x6[12], x6[13]);
        x7[15]              = _mm_add_epi32(x6[15], x6[14]);
        x7[16]              = x6[16];
        btf_32_type0_sse4_1_new(cospi_m08, cospi_p56, x6[17], x6[30], x7[17], x7[30], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m56, cospi_m08, x6[18], x6[29], x7[18], x7[29], __rounding, cos_bit);
        x7[19] = x6[19];
        x7[20] = x6[20];
        btf_32_type0_sse4_1_new(cospi_m40, cospi_p24, x6[21], x6[26], x7[21], x7[26], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m24, cospi_m40, x6[22], x6[25], x7[22], x7[25], __rounding, cos_bit);
        x7[23] = x6[23];
        x7[24] = x6[24];
        x7[27] = x6[27];
        x7[28] = x6[28];
        x7[31] = x6[31];
        x7[32] = _mm_add_epi32(x6[32], x6[35]);
        x7[35] = _mm_sub_epi32(x6[32], x6[35]);
        x7[33] = _mm_add_epi32(x6[33], x6[34]);
        x7[34] = _mm_sub_epi32(x6[33], x6[34]);
        x7[36] = _mm_sub_epi32(x6[39], x6[36]);
        x7[39] = _mm_add_epi32(x6[39], x6[36]);
        x7[37] = _mm_sub_epi32(x6[38], x6[37]);
        x7[38] = _mm_add_epi32(x6[38], x6[37]);
        x7[40] = _mm_add_epi32(x6[40], x6[43]);
        x7[43] = _mm_sub_epi32(x6[40], x6[43]);
        x7[41] = _mm_add_epi32(x6[41], x6[42]);
        x7[42] = _mm_sub_epi32(x6[41], x6[42]);
        x7[44] = _mm_sub_epi32(x6[47], x6[44]);
        x7[47] = _mm_add_epi32(x6[47], x6[44]);
        x7[45] = _mm_sub_epi32(x6[46], x6[45]);
        x7[46] = _mm_add_epi32(x6[46], x6[45]);
        x7[48] = _mm_add_epi32(x6[48], x6[51]);
        x7[51] = _mm_sub_epi32(x6[48], x6[51]);
        x7[49] = _mm_add_epi32(x6[49], x6[50]);
        x7[50] = _mm_sub_epi32(x6[49], x6[50]);
        x7[52] = _mm_sub_epi32(x6[55], x6[52]);
        x7[55] = _mm_add_epi32(x6[55], x6[52]);
        x7[53] = _mm_sub_epi32(x6[54], x6[53]);
        x7[54] = _mm_add_epi32(x6[54], x6[53]);
        x7[56] = _mm_add_epi32(x6[56], x6[59]);
        x7[59] = _mm_sub_epi32(x6[56], x6[59]);
        x7[57] = _mm_add_epi32(x6[57], x6[58]);
        x7[58] = _mm_sub_epi32(x6[57], x6[58]);
        x7[60] = _mm_sub_epi32(x6[63], x6[60]);
        x7[63] = _mm_add_epi32(x6[63], x6[60]);
        x7[61] = _mm_sub_epi32(x6[62], x6[61]);
        x7[62] = _mm_add_epi32(x6[62], x6[61]);

        // stage 8
        __m128i x8[40];
        out[2 * 4 * stride]  = half_btf_small(&cospi_p60, &x7[8], &cospi_p04, &x7[15], &__rounding, cos_bit);
        out[2 * 12 * stride] = half_btf_small(&cospi_p12, &x7[12], &cospi_m52, &x7[11], &__rounding, cos_bit);
        x8[0]                = _mm_add_epi32(x7[16], x7[17]);
        x8[1]                = _mm_add_epi32(x7[19], x7[18]);
        x8[2]                = _mm_add_epi32(x7[20], x7[21]);
        x8[3]                = _mm_add_epi32(x7[23], x7[22]);
        x8[4]                = _mm_add_epi32(x7[24], x7[25]);
        x8[5]                = _mm_add_epi32(x7[27], x7[26]);
        x8[6]                = _mm_add_epi32(x7[28], x7[29]);
        x8[7]                = _mm_add_epi32(x7[31], x7[30]);
        x8[8]                = x7[32];
        btf_32_type0_sse4_1_new(cospi_m04, cospi_p60, x7[33], x7[62], x8[9], x8[32], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m60, cospi_m04, x7[34], x7[61], x8[10], x8[33], __rounding, cos_bit);
        x8[11] = x7[35];
        x8[12] = x7[36];
        btf_32_type0_sse4_1_new(cospi_m36, cospi_p28, x7[37], x7[58], x8[13], x8[34], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m28, cospi_m36, x7[38], x7[57], x8[14], x8[35], __rounding, cos_bit);
        x8[15] = x7[39];
        x8[16] = x7[40];
        btf_32_type0_sse4_1_new(cospi_m20, cospi_p44, x7[41], x7[54], x8[17], x8[36], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m44, cospi_m20, x7[42], x7[53], x8[18], x8[37], __rounding, cos_bit);
        x8[19] = x7[43];
        x8[20] = x7[44];
        btf_32_type0_sse4_1_new(cospi_m52, cospi_p12, x7[45], x7[50], x8[21], x8[38], __rounding, cos_bit);
        btf_32_type0_sse4_1_new(cospi_m12, cospi_m52, x7[46], x7[49], x8[22], x8[39], __rounding, cos_bit);
        x8[23] = x7[47];
        x8[24] = x7[48];
        x8[25] = x7[51];
        x8[26] = x7[52];
        x8[27] = x7[55];
        x8[28] = x7[56];
        x8[29] = x7[59];
        x8[30] = x7[60];
        x8[31] = x7[63];

        // stage 9
        __m128i x9[16];
        out[2 * 2 * stride]  = half_btf_small(&cospi_p62, &x8[0], &cospi_p02, &x8[7], &__rounding, cos_bit);
        out[2 * 14 * stride] = half_btf_small(&cospi_p14, &x8[6], &cospi_m50, &x8[1], &__rounding, cos_bit);
        out[2 * 10 * stride] = half_btf_small(&cospi_p54, &x8[2], &cospi_p10, &x8[5], &__rounding, cos_bit);
        out[2 * 6 * stride]  = half_btf_small(&cospi_p06, &x8[4], &cospi_m58, &x8[3], &__rounding, cos_bit);
        x9[0]                = _mm_add_epi32(x8[8], x8[9]);
        x9[1]                = _mm_add_epi32(x8[11], x8[10]);
        x9[2]                = _mm_add_epi32(x8[12], x8[13]);
        x9[3]                = _mm_add_epi32(x8[15], x8[14]);
        x9[4]                = _mm_add_epi32(x8[16], x8[17]);
        x9[5]                = _mm_add_epi32(x8[19], x8[18]);
        x9[6]                = _mm_add_epi32(x8[20], x8[21]);
        x9[7]                = _mm_add_epi32(x8[23], x8[22]);
        x9[8]                = _mm_add_epi32(x8[24], x8[39]);
        x9[9]                = _mm_add_epi32(x8[25], x8[38]);
        x9[10]               = _mm_add_epi32(x8[26], x8[37]);
        x9[11]               = _mm_add_epi32(x8[27], x8[36]);
        x9[12]               = _mm_add_epi32(x8[28], x8[35]);
        x9[13]               = _mm_add_epi32(x8[29], x8[34]);
        x9[14]               = _mm_add_epi32(x8[30], x8[33]);
        x9[15]               = _mm_add_epi32(x8[31], x8[32]);

        // stage 10
        out[2 * 1 * stride]  = half_btf_small(&cospi_p63, &x9[0], &cospi_p01, &x9[15], &__rounding, cos_bit);
        out[2 * 15 * stride] = half_btf_small(&cospi_p15, &x9[14], &cospi_m49, &x9[1], &__rounding, cos_bit);
        out[2 * 9 * stride]  = half_btf_small(&cospi_p55, &x9[2], &cospi_p09, &x9[13], &__rounding, cos_bit);
        out[2 * 7 * stride]  = half_btf_small(&cospi_p07, &x9[12], &cospi_m57, &x9[3], &__rounding, cos_bit);
        out[2 * 5 * stride]  = half_btf_small(&cospi_p59, &x9[4], &cospi_p05, &x9[11], &__rounding, cos_bit);
        out[2 * 11 * stride] = half_btf_small(&cospi_p11, &x9[10], &cospi_m53, &x9[5], &__rounding, cos_bit);
        out[2 * 13 * stride] = half_btf_small(&cospi_p51, &x9[6], &cospi_p13, &x9[9], &__rounding, cos_bit);
        out[2 * 3 * stride]  = half_btf_small(&cospi_p03, &x9[8], &cospi_m61, &x9[7], &__rounding, cos_bit);
    }
}

static void fidtx64x64_N4_sse4_1(const __m128i* input, __m128i* output) {
    const int32_t bits     = 12; // new_sqrt2_bits = 12
    const int32_t sqrt     = 4 * 5793; // 4 * new_sqrt2
    const __m128i newsqrt  = _mm_set1_epi32(sqrt);
    const __m128i rounding = _mm_set1_epi32(1 << (bits - 1));

    __m128i temp0, temp1;
    for (int32_t i = 0; i < 128; i += 8) {
        temp0             = _mm_mullo_epi32(input[2 * i], newsqrt);
        temp1             = _mm_mullo_epi32(input[2 * i + 1], newsqrt);
        temp0             = _mm_add_epi32(temp0, rounding);
        temp1             = _mm_add_epi32(temp1, rounding);
        output[2 * i]     = _mm_srai_epi32(temp0, bits);
        output[2 * i + 1] = _mm_srai_epi32(temp1, bits);
        temp0             = _mm_mullo_epi32(input[2 * i + 2], newsqrt);
        temp1             = _mm_mullo_epi32(input[2 * i + 3], newsqrt);
        temp0             = _mm_add_epi32(temp0, rounding);
        temp1             = _mm_add_epi32(temp1, rounding);
        output[2 * i + 2] = _mm_srai_epi32(temp0, bits);
        output[2 * i + 3] = _mm_srai_epi32(temp1, bits);
    }
}

static void fdct16x4_N4_sse4_1(__m128i* in, __m128i* out, int32_t bit) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospim32 = _mm_set1_epi32(-cospi[32]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospim48 = _mm_set1_epi32(-cospi[48]);
    const __m128i  cospim16 = _mm_set1_epi32(-cospi[16]);
    const __m128i  cospi56  = _mm_set1_epi32(cospi[56]);
    const __m128i  cospi8   = _mm_set1_epi32(cospi[8]);
    const __m128i  cospi60  = _mm_set1_epi32(cospi[60]);
    const __m128i  cospi4   = _mm_set1_epi32(cospi[4]);
    const __m128i  cospi12  = _mm_set1_epi32(cospi[12]);
    const __m128i  cospi52  = _mm_set1_epi32(cospi[52]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    __m128i        u[16], v[16], x;

    // stage 0
    // stage 1
    u[0]  = _mm_add_epi32(in[0], in[15]);
    v[15] = _mm_sub_epi32(in[0], in[15]);
    u[1]  = _mm_add_epi32(in[1], in[14]);
    v[14] = _mm_sub_epi32(in[1], in[14]);
    u[2]  = _mm_add_epi32(in[2], in[13]);
    u[13] = _mm_sub_epi32(in[2], in[13]);
    u[3]  = _mm_add_epi32(in[3], in[12]);
    u[12] = _mm_sub_epi32(in[3], in[12]);
    u[4]  = _mm_add_epi32(in[4], in[11]);
    u[11] = _mm_sub_epi32(in[4], in[11]);
    u[5]  = _mm_add_epi32(in[5], in[10]);
    u[10] = _mm_sub_epi32(in[5], in[10]);
    u[6]  = _mm_add_epi32(in[6], in[9]);
    v[9]  = _mm_sub_epi32(in[6], in[9]);
    u[7]  = _mm_add_epi32(in[7], in[8]);
    v[8]  = _mm_sub_epi32(in[7], in[8]);

    // stage 2
    v[0] = _mm_add_epi32(u[0], u[7]);
    u[7] = _mm_sub_epi32(u[0], u[7]);
    v[1] = _mm_add_epi32(u[1], u[6]);
    v[6] = _mm_sub_epi32(u[1], u[6]);
    v[2] = _mm_add_epi32(u[2], u[5]);
    v[5] = _mm_sub_epi32(u[2], u[5]);
    v[3] = _mm_add_epi32(u[3], u[4]);
    u[4] = _mm_sub_epi32(u[3], u[4]);

    v[10] = _mm_mullo_epi32(u[10], cospim32);
    x     = _mm_mullo_epi32(u[13], cospi32);
    v[10] = _mm_add_epi32(v[10], x);
    v[10] = _mm_add_epi32(v[10], rnding);
    v[10] = _mm_srai_epi32(v[10], bit);

    v[13] = _mm_mullo_epi32(u[10], cospi32);
    x     = _mm_mullo_epi32(u[13], cospim32);
    v[13] = _mm_sub_epi32(v[13], x);
    v[13] = _mm_add_epi32(v[13], rnding);
    v[13] = _mm_srai_epi32(v[13], bit);

    v[11] = _mm_mullo_epi32(u[11], cospim32);
    x     = _mm_mullo_epi32(u[12], cospi32);
    v[11] = _mm_add_epi32(v[11], x);
    v[11] = _mm_add_epi32(v[11], rnding);
    v[11] = _mm_srai_epi32(v[11], bit);

    v[12] = _mm_mullo_epi32(u[11], cospi32);
    x     = _mm_mullo_epi32(u[12], cospim32);
    v[12] = _mm_sub_epi32(v[12], x);
    v[12] = _mm_add_epi32(v[12], rnding);
    v[12] = _mm_srai_epi32(v[12], bit);

    // stage 3
    u[0] = _mm_add_epi32(v[0], v[3]);
    u[1] = _mm_add_epi32(v[1], v[2]);

    u[5] = _mm_mullo_epi32(v[5], cospim32);
    x    = _mm_mullo_epi32(v[6], cospi32);
    u[5] = _mm_add_epi32(u[5], x);
    u[5] = _mm_add_epi32(u[5], rnding);
    u[5] = _mm_srai_epi32(u[5], bit);

    u[6] = _mm_mullo_epi32(v[5], cospi32);
    x    = _mm_mullo_epi32(v[6], cospim32);
    u[6] = _mm_sub_epi32(u[6], x);
    u[6] = _mm_add_epi32(u[6], rnding);
    u[6] = _mm_srai_epi32(u[6], bit);

    u[8]  = _mm_add_epi32(v[8], v[11]);
    v[11] = _mm_sub_epi32(v[8], v[11]);
    u[9]  = _mm_add_epi32(v[9], v[10]);
    u[10] = _mm_sub_epi32(v[9], v[10]);
    u[12] = _mm_sub_epi32(v[15], v[12]);
    v[15] = _mm_add_epi32(v[15], v[12]);
    u[13] = _mm_sub_epi32(v[14], v[13]);
    u[14] = _mm_add_epi32(v[14], v[13]);

    // stage 4
    u[0]   = _mm_mullo_epi32(u[0], cospi32);
    u[1]   = _mm_mullo_epi32(u[1], cospi32);
    v[0]   = _mm_add_epi32(u[0], u[1]);
    v[0]   = _mm_add_epi32(v[0], rnding);
    out[0] = _mm_srai_epi32(v[0], bit);

    v[4] = _mm_add_epi32(u[4], u[5]);
    v[7] = _mm_add_epi32(u[7], u[6]);
    v[8] = u[8];

    v[9] = _mm_mullo_epi32(u[9], cospim16);
    x    = _mm_mullo_epi32(u[14], cospi48);
    v[9] = _mm_add_epi32(v[9], x);
    v[9] = _mm_add_epi32(v[9], rnding);
    v[9] = _mm_srai_epi32(v[9], bit);

    v[14] = _mm_mullo_epi32(u[9], cospi48);
    x     = _mm_mullo_epi32(u[14], cospim16);
    v[14] = _mm_sub_epi32(v[14], x);
    v[14] = _mm_add_epi32(v[14], rnding);
    v[14] = _mm_srai_epi32(v[14], bit);

    v[10] = _mm_mullo_epi32(u[10], cospim48);
    x     = _mm_mullo_epi32(u[13], cospim16);
    v[10] = _mm_add_epi32(v[10], x);
    v[10] = _mm_add_epi32(v[10], rnding);
    v[10] = _mm_srai_epi32(v[10], bit);

    v[13] = _mm_mullo_epi32(u[10], cospim16);
    x     = _mm_mullo_epi32(u[13], cospim48);
    v[13] = _mm_sub_epi32(v[13], x);
    v[13] = _mm_add_epi32(v[13], rnding);
    v[13] = _mm_srai_epi32(v[13], bit);

    v[12] = u[12];

    // stage 5
    u[4]   = _mm_mullo_epi32(v[4], cospi56);
    x      = _mm_mullo_epi32(v[7], cospi8);
    u[4]   = _mm_add_epi32(u[4], x);
    u[4]   = _mm_add_epi32(u[4], rnding);
    out[2] = _mm_srai_epi32(u[4], bit);

    u[8]  = _mm_add_epi32(v[8], v[9]);
    u[11] = _mm_add_epi32(v[11], v[10]);
    u[12] = _mm_add_epi32(v[12], v[13]);
    u[15] = _mm_add_epi32(v[15], v[14]);

    // stage 6
    v[8]   = _mm_mullo_epi32(u[8], cospi60);
    x      = _mm_mullo_epi32(u[15], cospi4);
    v[8]   = _mm_add_epi32(v[8], x);
    v[8]   = _mm_add_epi32(v[8], rnding);
    out[1] = _mm_srai_epi32(v[8], bit);

    v[12]  = _mm_mullo_epi32(u[11], cospi52);
    x      = _mm_mullo_epi32(u[12], cospi12);
    v[12]  = _mm_sub_epi32(x, v[12]);
    v[12]  = _mm_add_epi32(v[12], rnding);
    out[3] = _mm_srai_epi32(v[12], bit);
}

static void fdct4x8_col_N4_sse4_1(__m128i* in, __m128i* output, int32_t bit, const int32_t num_col) {
    const int32_t* cospi   = cospi_arr(bit);
    const __m128i  zero    = _mm_setzero_si128();
    const __m128i  cospi32 = _mm_set1_epi32(cospi[32]);
    const __m128i  rnding  = _mm_set1_epi32(1 << (bit - 1));
    __m128i        s0, s1;
    __m128i        u0, u1, u2, u3;
    __m128i        v0, v1;

    int32_t endidx = 3 * num_col;
    s0             = _mm_add_epi32(in[0], in[2 * endidx]);
    endidx -= num_col;
    s1 = _mm_add_epi32(in[2 * num_col], in[2 * endidx]);

    // btf_32_sse4_1_type0(cospi32, cospi32, s[01], u[02], bit);
    u0 = _mm_mullo_epi32(s0, cospi32);
    u1 = _mm_mullo_epi32(s1, cospi32);
    u2 = _mm_add_epi32(u0, u1);
    u3 = _mm_add_epi32(u2, rnding);
    u0 = _mm_srai_epi32(u3, bit);

    // Transpose 4x4 32-bit
    v0 = _mm_unpacklo_epi32(u0, zero);
    v1 = _mm_unpackhi_epi32(u0, zero);

    output[0] = _mm_unpacklo_epi64(v0, zero);
    output[1] = _mm_unpackhi_epi64(v0, zero);
    output[2] = _mm_unpacklo_epi64(v1, zero);
    output[3] = _mm_unpackhi_epi64(v1, zero);
}

static void fdct4x8_row_N4_with_round_sse4_1(__m128i* input, __m128i* output, int32_t bit, int32_t shift) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  zero     = _mm_setzero_si128();
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    const __m128i  rounding = _mm_set1_epi32(1 << (shift - 1));
    __m128i        s0, s1, s2, s3;
    __m128i        u0, u1, u2, u3, u4, u5, u6, u7;

    s0 = _mm_add_epi32(input[0], input[3]);
    s1 = _mm_add_epi32(input[4], input[7]);
    s2 = _mm_add_epi32(input[1], input[2]);
    s3 = _mm_add_epi32(input[5], input[6]);

    // btf_32_sse4_1_type0(cospi32, cospi32, s[01], u[02], bit);
    u0 = _mm_mullo_epi32(s0, cospi32);
    u1 = _mm_mullo_epi32(s1, cospi32);
    u2 = _mm_mullo_epi32(s2, cospi32);
    u3 = _mm_mullo_epi32(s3, cospi32);
    u4 = _mm_add_epi32(u0, u2);
    u5 = _mm_add_epi32(u1, u3);

    u6 = _mm_add_epi32(u4, rnding);
    u7 = _mm_add_epi32(u5, rnding);

    u0 = _mm_srai_epi32(u6, bit);
    u1 = _mm_srai_epi32(u7, bit);

    u0 = _mm_add_epi32(u0, rounding);
    u1 = _mm_add_epi32(u1, rounding);
    u0 = _mm_srai_epi32(u0, shift);
    u1 = _mm_srai_epi32(u1, shift);

    // Transpose 4x4 32-bit
    s0 = _mm_unpacklo_epi32(u0, zero);
    s1 = _mm_unpacklo_epi32(u1, zero);
    s2 = _mm_unpackhi_epi32(u0, zero);
    s3 = _mm_unpackhi_epi32(u1, zero);

    output[0] = _mm_unpacklo_epi64(s0, zero);
    output[1] = _mm_unpackhi_epi64(s0, zero);
    output[2] = _mm_unpacklo_epi64(s2, zero);
    output[3] = _mm_unpackhi_epi64(s2, zero);
    output[4] = _mm_unpacklo_epi64(s1, zero);
    output[5] = _mm_unpackhi_epi64(s1, zero);
    output[6] = _mm_unpacklo_epi64(s3, zero);
    output[7] = _mm_unpackhi_epi64(s3, zero);
}

static INLINE void fadst4x8_col_N4_sse4_1(__m128i* in, __m128i* output, int32_t bit, const int32_t num_col) {
    const int32_t* sinpi  = sinpi_arr(bit);
    const __m128i  zero   = _mm_setzero_si128();
    const __m128i  rnding = _mm_set1_epi32(1 << (bit - 1));
    const __m128i  sinpi1 = _mm_set1_epi32((int32_t)sinpi[1]);
    const __m128i  sinpi2 = _mm_set1_epi32((int32_t)sinpi[2]);
    const __m128i  sinpi3 = _mm_set1_epi32((int32_t)sinpi[3]);
    const __m128i  sinpi4 = _mm_set1_epi32((int32_t)sinpi[4]);
    __m128i        s0, s1, s2, s3;
    __m128i        u0, u1;
    __m128i        v0, v1;

    int32_t idx = 0 * num_col;
    s0          = _mm_mullo_epi32(in[2 * idx], sinpi1);
    idx += num_col;
    s1 = _mm_mullo_epi32(in[2 * idx], sinpi2);
    idx += num_col;
    s3 = _mm_mullo_epi32(in[2 * idx], sinpi3);
    idx += num_col;
    s2 = _mm_mullo_epi32(in[2 * idx], sinpi4);
    u0 = _mm_add_epi32(s0, s1);
    u1 = _mm_add_epi32(u0, s2);
    s0 = _mm_add_epi32(u1, s3);
    u0 = _mm_add_epi32(s0, rnding);
    u0 = _mm_srai_epi32(u0, bit);

    // Transpose 4x4 32-bit
    v0        = _mm_unpacklo_epi32(u0, zero);
    v1        = _mm_unpackhi_epi32(u0, zero);
    output[0] = _mm_unpacklo_epi64(v0, zero);
    output[1] = _mm_unpackhi_epi64(v0, zero);
    output[2] = _mm_unpacklo_epi64(v1, zero);
    output[3] = _mm_unpackhi_epi64(v1, zero);
}

static void fadst4x8_row_N4_with_round_sse4_1(__m128i* input, __m128i* output, int32_t bit, int32_t shift) {
    const int32_t* sinpi    = sinpi_arr(bit);
    const __m128i  zero     = _mm_setzero_si128();
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    const __m128i  rounding = _mm_set1_epi32(1 << (shift - 1));
    const __m128i  sinpi1   = _mm_set1_epi32((int32_t)sinpi[1]);
    const __m128i  sinpi2   = _mm_set1_epi32((int32_t)sinpi[2]);
    const __m128i  sinpi3   = _mm_set1_epi32((int32_t)sinpi[3]);
    const __m128i  sinpi4   = _mm_set1_epi32((int32_t)sinpi[4]);
    __m128i        s0, s1, s2, s3, s4, s5, s6, s7, s8, s9;
    __m128i        u0, u1, u2, u3;

    s0 = _mm_mullo_epi32(input[0], sinpi1);
    s1 = _mm_mullo_epi32(input[4], sinpi1);
    s4 = _mm_mullo_epi32(input[1], sinpi2);
    s5 = _mm_mullo_epi32(input[5], sinpi2);
    s8 = _mm_mullo_epi32(input[2], sinpi3);
    s9 = _mm_mullo_epi32(input[6], sinpi3);
    s6 = _mm_mullo_epi32(input[3], sinpi4);
    s7 = _mm_mullo_epi32(input[7], sinpi4);

    u0 = _mm_add_epi32(s0, s4);
    u1 = _mm_add_epi32(s1, s5);
    u2 = _mm_add_epi32(u0, s6);
    u3 = _mm_add_epi32(u1, s7);

    s0 = _mm_add_epi32(u2, s8);
    s1 = _mm_add_epi32(u3, s9);

    u0 = _mm_add_epi32(s0, rnding);
    u1 = _mm_add_epi32(s1, rnding);
    u0 = _mm_srai_epi32(u0, bit);
    u1 = _mm_srai_epi32(u1, bit);

    u0 = _mm_add_epi32(u0, rounding);
    u1 = _mm_add_epi32(u1, rounding);
    u0 = _mm_srai_epi32(u0, shift);
    u1 = _mm_srai_epi32(u1, shift);

    // Transpose 4x4 32-bit
    s0 = _mm_unpacklo_epi32(u0, zero);
    s1 = _mm_unpacklo_epi32(u1, zero);
    s2 = _mm_unpackhi_epi32(u0, zero);
    s3 = _mm_unpackhi_epi32(u1, zero);

    output[0] = _mm_unpacklo_epi64(s0, zero);
    output[1] = _mm_unpackhi_epi64(s0, zero);
    output[2] = _mm_unpacklo_epi64(s2, zero);
    output[3] = _mm_unpackhi_epi64(s2, zero);
    output[4] = _mm_unpacklo_epi64(s1, zero);
    output[5] = _mm_unpackhi_epi64(s1, zero);
    output[6] = _mm_unpacklo_epi64(s3, zero);
    output[7] = _mm_unpackhi_epi64(s3, zero);
}

static void fadst16x4_N4_sse4_1(__m128i* in, __m128i* out, int32_t bit) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospi16  = _mm_set1_epi32(cospi[16]);
    const __m128i  cospim16 = _mm_set1_epi32(-cospi[16]);
    const __m128i  cospim48 = _mm_set1_epi32(-cospi[48]);
    const __m128i  cospi8   = _mm_set1_epi32(cospi[8]);
    const __m128i  cospi56  = _mm_set1_epi32(cospi[56]);
    const __m128i  cospim56 = _mm_set1_epi32(-cospi[56]);
    const __m128i  cospim8  = _mm_set1_epi32(-cospi[8]);
    const __m128i  cospi24  = _mm_set1_epi32(cospi[24]);
    const __m128i  cospim24 = _mm_set1_epi32(-cospi[24]);
    const __m128i  cospim40 = _mm_set1_epi32(-cospi[40]);
    const __m128i  cospi40  = _mm_set1_epi32(cospi[40]);
    const __m128i  cospi62  = _mm_set1_epi32(cospi[62]);
    const __m128i  cospim2  = _mm_set1_epi32(-cospi[2]);
    const __m128i  cospi54  = _mm_set1_epi32(cospi[54]);
    const __m128i  cospim10 = _mm_set1_epi32(-cospi[10]);
    const __m128i  cospi50  = _mm_set1_epi32(cospi[50]);
    const __m128i  cospi14  = _mm_set1_epi32(cospi[14]);
    const __m128i  cospi58  = _mm_set1_epi32(cospi[58]);
    const __m128i  cospi6   = _mm_set1_epi32(cospi[6]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    const __m128i  zero     = _mm_setzero_si128();

    __m128i u[16], v[16], x, y;
    __m128i tmp[13];

    tmp[0] = _mm_sub_epi32(zero, in[15]);
    u[2]   = _mm_sub_epi32(zero, in[7]);
    tmp[1] = _mm_sub_epi32(zero, in[3]);
    u[7]   = _mm_sub_epi32(zero, in[11]);
    tmp[2] = _mm_sub_epi32(zero, in[1]);
    u[11]  = _mm_sub_epi32(zero, in[9]);
    tmp[3] = _mm_sub_epi32(zero, in[13]);
    u[14]  = _mm_sub_epi32(zero, in[5]);

    // stage 2

    x    = _mm_mullo_epi32(u[2], cospi32);
    y    = _mm_mullo_epi32(in[8], cospi32);
    v[2] = _mm_add_epi32(x, y);
    v[2] = _mm_add_epi32(v[2], rnding);
    v[2] = _mm_srai_epi32(v[2], bit);

    v[3] = _mm_sub_epi32(x, y);
    v[3] = _mm_add_epi32(v[3], rnding);
    v[3] = _mm_srai_epi32(v[3], bit);

    x    = _mm_mullo_epi32(in[4], cospi32);
    y    = _mm_mullo_epi32(u[7], cospi32);
    v[6] = _mm_add_epi32(x, y);
    v[6] = _mm_add_epi32(v[6], rnding);
    v[6] = _mm_srai_epi32(v[6], bit);

    v[7] = _mm_sub_epi32(x, y);
    v[7] = _mm_add_epi32(v[7], rnding);
    v[7] = _mm_srai_epi32(v[7], bit);

    x     = _mm_mullo_epi32(in[6], cospi32);
    y     = _mm_mullo_epi32(u[11], cospi32);
    v[10] = _mm_add_epi32(x, y);
    v[10] = _mm_add_epi32(v[10], rnding);
    v[10] = _mm_srai_epi32(v[10], bit);

    v[11] = _mm_sub_epi32(x, y);
    v[11] = _mm_add_epi32(v[11], rnding);
    v[11] = _mm_srai_epi32(v[11], bit);

    x     = _mm_mullo_epi32(u[14], cospi32);
    y     = _mm_mullo_epi32(in[10], cospi32);
    v[14] = _mm_add_epi32(x, y);
    v[14] = _mm_add_epi32(v[14], rnding);
    v[14] = _mm_srai_epi32(v[14], bit);

    v[15] = _mm_sub_epi32(x, y);
    v[15] = _mm_add_epi32(v[15], rnding);
    v[15] = _mm_srai_epi32(v[15], bit);

    // stage 3
    tmp[4] = _mm_add_epi32(in[0], v[2]);
    tmp[5] = _mm_add_epi32(tmp[0], v[3]);
    tmp[6] = _mm_sub_epi32(in[0], v[2]);
    tmp[0] = _mm_sub_epi32(tmp[0], v[3]);
    u[4]   = _mm_add_epi32(tmp[1], v[6]);
    u[5]   = _mm_add_epi32(in[12], v[7]);
    u[6]   = _mm_sub_epi32(tmp[1], v[6]);
    u[7]   = _mm_sub_epi32(in[12], v[7]);
    tmp[1] = _mm_add_epi32(tmp[2], v[10]);
    tmp[7] = _mm_add_epi32(in[14], v[11]);
    tmp[2] = _mm_sub_epi32(tmp[2], v[10]);
    tmp[8] = _mm_sub_epi32(in[14], v[11]);
    u[12]  = _mm_add_epi32(in[2], v[14]);
    u[13]  = _mm_add_epi32(tmp[3], v[15]);
    u[14]  = _mm_sub_epi32(in[2], v[14]);
    u[15]  = _mm_sub_epi32(tmp[3], v[15]);

    // stage 4
    v[4]  = half_btf_small(&cospi16, &u[4], &cospi48, &u[5], &rnding, bit);
    v[5]  = half_btf_small(&cospi48, &u[4], &cospim16, &u[5], &rnding, bit);
    v[6]  = half_btf_small(&cospim48, &u[6], &cospi16, &u[7], &rnding, bit);
    v[7]  = half_btf_small(&cospi16, &u[6], &cospi48, &u[7], &rnding, bit);
    v[12] = half_btf_small(&cospi16, &u[12], &cospi48, &u[13], &rnding, bit);
    v[13] = half_btf_small(&cospi48, &u[12], &cospim16, &u[13], &rnding, bit);
    v[14] = half_btf_small(&cospim48, &u[14], &cospi16, &u[15], &rnding, bit);
    v[15] = half_btf_small(&cospi16, &u[14], &cospi48, &u[15], &rnding, bit);

    // stage 5
    tmp[9]  = _mm_add_epi32(tmp[4], v[4]);
    tmp[10] = _mm_add_epi32(tmp[5], v[5]);
    tmp[11] = _mm_add_epi32(tmp[6], v[6]);
    tmp[12] = _mm_add_epi32(tmp[0], v[7]);
    tmp[4]  = _mm_sub_epi32(tmp[4], v[4]);
    tmp[5]  = _mm_sub_epi32(tmp[5], v[5]);
    tmp[6]  = _mm_sub_epi32(tmp[6], v[6]);
    tmp[0]  = _mm_sub_epi32(tmp[0], v[7]);
    u[8]    = _mm_add_epi32(tmp[1], v[12]);
    u[9]    = _mm_add_epi32(tmp[7], v[13]);
    u[10]   = _mm_add_epi32(tmp[2], v[14]);
    u[11]   = _mm_add_epi32(tmp[8], v[15]);
    u[12]   = _mm_sub_epi32(tmp[1], v[12]);
    u[13]   = _mm_sub_epi32(tmp[7], v[13]);
    u[14]   = _mm_sub_epi32(tmp[2], v[14]);
    u[15]   = _mm_sub_epi32(tmp[8], v[15]);

    // stage 6
    v[8]  = half_btf_small(&cospi8, &u[8], &cospi56, &u[9], &rnding, bit);
    v[9]  = half_btf_small(&cospi56, &u[8], &cospim8, &u[9], &rnding, bit);
    v[10] = half_btf_small(&cospi40, &u[10], &cospi24, &u[11], &rnding, bit);
    v[11] = half_btf_small(&cospi24, &u[10], &cospim40, &u[11], &rnding, bit);
    v[12] = half_btf_small(&cospim56, &u[12], &cospi8, &u[13], &rnding, bit);
    v[13] = half_btf_small(&cospi8, &u[12], &cospi56, &u[13], &rnding, bit);
    v[14] = half_btf_small(&cospim24, &u[14], &cospi40, &u[15], &rnding, bit);
    v[15] = half_btf_small(&cospi40, &u[14], &cospi24, &u[15], &rnding, bit);

    // stage 7
    u[0]  = _mm_add_epi32(tmp[9], v[8]);
    u[1]  = _mm_add_epi32(tmp[10], v[9]);
    u[2]  = _mm_add_epi32(tmp[11], v[10]);
    u[3]  = _mm_add_epi32(tmp[12], v[11]);
    u[12] = _mm_sub_epi32(tmp[4], v[12]);
    u[13] = _mm_sub_epi32(tmp[5], v[13]);
    u[14] = _mm_sub_epi32(tmp[6], v[14]);
    u[15] = _mm_sub_epi32(tmp[0], v[15]);

    // stage 8
    out[0] = half_btf_small(&cospi62, &u[0], &cospim2, &u[1], &rnding, bit);
    out[2] = half_btf_small(&cospi54, &u[2], &cospim10, &u[3], &rnding, bit);
    out[3] = half_btf_small(&cospi50, &u[12], &cospi14, &u[13], &rnding, bit);
    out[1] = half_btf_small(&cospi58, &u[14], &cospi6, &u[15], &rnding, bit);
}

static INLINE void fidtx4x8_col_N4_sse4_1(__m128i* in, __m128i* output, int32_t bit, int32_t col_num) {
    (void)bit;
    const __m128i zero   = _mm_setzero_si128();
    __m128i       fact   = _mm_set1_epi32(new_sqrt2);
    __m128i       offset = _mm_set1_epi32(1 << (new_sqrt2_bits - 1));
    __m128i       a_low;

    for (int32_t i = 0; i < col_num; i++) {
        a_low     = _mm_mullo_epi32(in[i], fact);
        a_low     = _mm_add_epi32(a_low, offset);
        a_low     = _mm_srai_epi32(a_low, new_sqrt2_bits);
        output[i] = _mm_blend_epi16(zero, a_low, 3);
    }
}

static INLINE void fidtx4x8_row_N4_with_round_sse4_1(__m128i* input, __m128i* output, int32_t bit, int32_t shift) {
    (void)bit;
    __m128i s0, s1, s2, s3;
    __m128i u0, u1;

    const __m128i fact     = _mm_set1_epi32(new_sqrt2);
    const __m128i offset   = _mm_set1_epi32(1 << (new_sqrt2_bits - 1));
    const __m128i rounding = _mm_set1_epi32(1 << (shift - 1));
    const __m128i zero     = _mm_setzero_si128();

    u0 = _mm_mullo_epi32(input[0], fact);
    u1 = _mm_mullo_epi32(input[4], fact);
    u0 = _mm_add_epi32(u0, offset);
    u1 = _mm_add_epi32(u1, offset);
    u0 = _mm_srai_epi32(u0, new_sqrt2_bits);
    u1 = _mm_srai_epi32(u1, new_sqrt2_bits);
    u0 = _mm_add_epi32(u0, rounding);
    u1 = _mm_add_epi32(u1, rounding);
    u0 = _mm_srai_epi32(u0, shift);
    u1 = _mm_srai_epi32(u1, shift);

    // Transpose 4x4 32-bit
    s0 = _mm_unpacklo_epi32(u0, zero);
    s1 = _mm_unpacklo_epi32(u1, zero);
    s2 = _mm_unpackhi_epi32(u0, zero);
    s3 = _mm_unpackhi_epi32(u1, zero);

    output[0] = _mm_unpacklo_epi64(s0, zero);
    output[1] = _mm_unpackhi_epi64(s0, zero);
    output[2] = _mm_unpacklo_epi64(s2, zero);
    output[3] = _mm_unpackhi_epi64(s2, zero);
    output[4] = _mm_unpacklo_epi64(s1, zero);
    output[5] = _mm_unpackhi_epi64(s1, zero);
    output[6] = _mm_unpacklo_epi64(s3, zero);
    output[7] = _mm_unpackhi_epi64(s3, zero);
}

static INLINE void fidtx4x8_row_N4_sse4_1(__m128i* in, __m128i* output, int32_t bit) {
    (void)bit;
    __m128i fact   = _mm_set1_epi32(new_sqrt2);
    __m128i offset = _mm_set1_epi32(1 << (new_sqrt2_bits - 1));
    __m128i a_low0, a_low1;

    a_low0    = _mm_mullo_epi32(in[0], fact);
    a_low1    = _mm_mullo_epi32(in[1], fact);
    a_low0    = _mm_add_epi32(a_low0, offset);
    a_low1    = _mm_add_epi32(a_low1, offset);
    output[0] = _mm_srai_epi32(a_low0, new_sqrt2_bits);
    output[1] = _mm_srai_epi32(a_low1, new_sqrt2_bits);
}

static AOM_FORCE_INLINE void fidtx8x4_N4_sse4_1(__m128i* in, __m128i* out, int32_t bit) {
    (void)bit;
    out[0] = _mm_add_epi32(in[0], in[0]);
    out[1] = _mm_add_epi32(in[1], in[1]);
}

static void fdct4x8_N4_sse4_1(__m128i* in, __m128i* out, int32_t bit) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospim32 = _mm_set1_epi32(-cospi[32]);
    const __m128i  cospi56  = _mm_set1_epi32(cospi[56]);
    const __m128i  cospi8   = _mm_set1_epi32(cospi[8]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    __m128i        u[8], v[8];

    // Even 8 points 0, 2, ..., 14
    // stage 0
    // stage 1
    u[0] = _mm_add_epi32(in[0], in[7]);
    v[7] = _mm_sub_epi32(in[0], in[7]); // v[7]
    u[1] = _mm_add_epi32(in[1], in[6]);
    u[6] = _mm_sub_epi32(in[1], in[6]);
    u[2] = _mm_add_epi32(in[2], in[5]);
    u[5] = _mm_sub_epi32(in[2], in[5]);
    u[3] = _mm_add_epi32(in[3], in[4]);
    v[4] = _mm_sub_epi32(in[3], in[4]); // v[4]

    // stage 2
    v[0] = _mm_add_epi32(u[0], u[3]);
    v[1] = _mm_add_epi32(u[1], u[2]);

    v[5] = _mm_mullo_epi32(u[5], cospim32);
    v[6] = _mm_mullo_epi32(u[6], cospi32);
    v[5] = _mm_add_epi32(v[5], v[6]);
    v[5] = _mm_add_epi32(v[5], rnding);
    v[5] = _mm_srai_epi32(v[5], bit);

    u[0] = _mm_mullo_epi32(u[5], cospi32);
    v[6] = _mm_mullo_epi32(u[6], cospim32);
    v[6] = _mm_sub_epi32(u[0], v[6]);
    v[6] = _mm_add_epi32(v[6], rnding);
    v[6] = _mm_srai_epi32(v[6], bit);

    // stage 3
    // type 0
    v[0]   = _mm_mullo_epi32(v[0], cospi32);
    v[1]   = _mm_mullo_epi32(v[1], cospi32);
    u[0]   = _mm_add_epi32(v[0], v[1]);
    u[0]   = _mm_add_epi32(u[0], rnding);
    out[0] = _mm_srai_epi32(u[0], bit);

    u[4] = _mm_add_epi32(v[4], v[5]);
    u[7] = _mm_add_epi32(v[7], v[6]);

    // stage 4
    // stage 5
    v[0]   = _mm_mullo_epi32(u[4], cospi56);
    v[1]   = _mm_mullo_epi32(u[7], cospi8);
    v[0]   = _mm_add_epi32(v[0], v[1]);
    v[0]   = _mm_add_epi32(v[0], rnding);
    out[1] = _mm_srai_epi32(v[0], bit); // buf0[4]
}

static void fadst8x4_N4_sse4_1(__m128i* in, __m128i* out, int32_t bit, const int32_t col_num) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospi16  = _mm_set1_epi32(cospi[16]);
    const __m128i  cospim16 = _mm_set1_epi32(-cospi[16]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospim48 = _mm_set1_epi32(-cospi[48]);
    const __m128i  cospim4  = _mm_set1_epi32(-cospi[4]);
    const __m128i  cospi60  = _mm_set1_epi32(cospi[60]);
    const __m128i  cospi52  = _mm_set1_epi32(cospi[52]);
    const __m128i  cospi12  = _mm_set1_epi32(cospi[12]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    const __m128i  zero     = _mm_setzero_si128();
    __m128i        u0, u1, u2, u3, u4, u5, u6, u7;
    __m128i        v0, v1, v2, v3, v4, v5, v6, v7;
    __m128i        x, y;
    int32_t        col;

    // Note:
    //  Even column: 0, 2, ..., 14
    //  Odd column: 1, 3, ..., 15
    //  one even column plus one odd column constructs one row (8 coeffs)
    //  total we have 8 rows (8x8).
    for (col = 0; col < col_num; ++col) {
        // stage 0
        // stage 1
        u0 = in[col_num * 0 + col];
        u1 = _mm_sub_epi32(zero, in[col_num * 7 + col]);
        u2 = _mm_sub_epi32(zero, in[col_num * 3 + col]);
        u3 = in[col_num * 4 + col];
        u4 = _mm_sub_epi32(zero, in[col_num * 1 + col]);
        u5 = in[col_num * 6 + col];
        u6 = in[col_num * 2 + col];
        u7 = _mm_sub_epi32(zero, in[col_num * 5 + col]);

        // stage 2
        v0 = u0;
        v1 = u1;

        x  = _mm_mullo_epi32(u2, cospi32);
        y  = _mm_mullo_epi32(u3, cospi32);
        v2 = _mm_add_epi32(x, y);
        v2 = _mm_add_epi32(v2, rnding);
        v2 = _mm_srai_epi32(v2, bit);

        v3 = _mm_sub_epi32(x, y);
        v3 = _mm_add_epi32(v3, rnding);
        v3 = _mm_srai_epi32(v3, bit);

        v4 = u4;
        v5 = u5;

        x  = _mm_mullo_epi32(u6, cospi32);
        y  = _mm_mullo_epi32(u7, cospi32);
        v6 = _mm_add_epi32(x, y);
        v6 = _mm_add_epi32(v6, rnding);
        v6 = _mm_srai_epi32(v6, bit);

        v7 = _mm_sub_epi32(x, y);
        v7 = _mm_add_epi32(v7, rnding);
        v7 = _mm_srai_epi32(v7, bit);

        // stage 3
        u0 = _mm_add_epi32(v0, v2);
        u1 = _mm_add_epi32(v1, v3);
        u2 = _mm_sub_epi32(v0, v2);
        u3 = _mm_sub_epi32(v1, v3);
        u4 = _mm_add_epi32(v4, v6);
        u5 = _mm_add_epi32(v5, v7);
        u6 = _mm_sub_epi32(v4, v6);
        u7 = _mm_sub_epi32(v5, v7);

        // stage 4
        v0 = u0;
        v1 = u1;
        v2 = u2;
        v3 = u3;

        x  = _mm_mullo_epi32(u4, cospi16);
        y  = _mm_mullo_epi32(u5, cospi48);
        v4 = _mm_add_epi32(x, y);
        v4 = _mm_add_epi32(v4, rnding);
        v4 = _mm_srai_epi32(v4, bit);

        x  = _mm_mullo_epi32(u4, cospi48);
        y  = _mm_mullo_epi32(u5, cospim16);
        v5 = _mm_add_epi32(x, y);
        v5 = _mm_add_epi32(v5, rnding);
        v5 = _mm_srai_epi32(v5, bit);

        x  = _mm_mullo_epi32(u6, cospim48);
        y  = _mm_mullo_epi32(u7, cospi16);
        v6 = _mm_add_epi32(x, y);
        v6 = _mm_add_epi32(v6, rnding);
        v6 = _mm_srai_epi32(v6, bit);

        x  = _mm_mullo_epi32(u6, cospi16);
        y  = _mm_mullo_epi32(u7, cospi48);
        v7 = _mm_add_epi32(x, y);
        v7 = _mm_add_epi32(v7, rnding);
        v7 = _mm_srai_epi32(v7, bit);

        // stage 5
        u0 = _mm_add_epi32(v0, v4);
        u1 = _mm_add_epi32(v1, v5);
        u6 = _mm_sub_epi32(v2, v6);
        u7 = _mm_sub_epi32(v3, v7);

        // stage 6
        x                      = _mm_mullo_epi32(u0, cospi60);
        y                      = _mm_mullo_epi32(u1, cospim4);
        v1                     = _mm_add_epi32(x, y);
        v1                     = _mm_add_epi32(v1, rnding);
        out[col_num * 0 + col] = _mm_srai_epi32(v1, bit);

        x                      = _mm_mullo_epi32(u6, cospi52);
        y                      = _mm_mullo_epi32(u7, cospi12);
        v6                     = _mm_add_epi32(x, y);
        v6                     = _mm_add_epi32(v6, rnding);
        out[col_num * 1 + col] = _mm_srai_epi32(v6, bit);
    }
}

static AOM_FORCE_INLINE void write_buffer_4x8_N4(const __m128i* res, int32_t* output) {
    const __m128i zero = _mm_setzero_si128();
    _mm_storeu_si128((__m128i*)(output + 0 * 4), res[0]);
    _mm_storeu_si128((__m128i*)(output + 1 * 4), res[1]);
    _mm_storeu_si128((__m128i*)(output + 2 * 4), zero);
    _mm_storeu_si128((__m128i*)(output + 3 * 4), zero);
    _mm_storeu_si128((__m128i*)(output + 4 * 4), zero);
    _mm_storeu_si128((__m128i*)(output + 5 * 4), zero);
    _mm_storeu_si128((__m128i*)(output + 6 * 4), zero);
    _mm_storeu_si128((__m128i*)(output + 7 * 4), zero);
}

static AOM_FORCE_INLINE void col_txfm_8x4_N4_rounding(__m128i* in, int32_t shift) {
    const __m128i rounding = _mm_set1_epi32(1 << (shift - 1));

    in[0] = _mm_add_epi32(in[0], rounding);
    in[1] = _mm_add_epi32(in[1], rounding);
    in[0] = _mm_srai_epi32(in[0], shift);
    in[1] = _mm_srai_epi32(in[1], shift);
}

void svt_av1_fwd_txfm2d_8x8_N4_sse4_1(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[16], out[16] = {_mm_setzero_si128()};
    const int8_t* shift   = fwd_txfm_shift_ls[TX_8X8];
    const int32_t txw_idx = get_txw_idx(TX_8X8);
    const int32_t txh_idx = get_txh_idx(TX_8X8);

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fdct8x8_N4_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        transpose_8x8_sse4_1(out, in);
        fdct8x8_N4_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_sse4_1(out, in);
        write_buffer_8x8_N4(in, coeff);
        break;
    case ADST_DCT:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fadst8x8_N4_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        transpose_8x8_sse4_1(out, in);
        fdct8x8_N4_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_sse4_1(out, in);
        write_buffer_8x8_N4(in, coeff);
        break;
    case DCT_ADST:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fdct8x8_N4_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        transpose_8x8_sse4_1(out, in);
        fadst8x8_N4_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_sse4_1(out, in);
        write_buffer_8x8_N4(in, coeff);
        break;
    case ADST_ADST:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fadst8x8_N4_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        transpose_8x8_sse4_1(out, in);
        fadst8x8_N4_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_sse4_1(out, in);
        write_buffer_8x8_N4(in, coeff);
        break;
    case FLIPADST_DCT:
        load_buffer_8x8(input, in, stride, 1, 0, shift[0]);
        fadst8x8_N4_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        transpose_8x8_sse4_1(out, in);
        fdct8x8_N4_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_sse4_1(out, in);
        write_buffer_8x8_N4(in, coeff);
        break;
    case DCT_FLIPADST:
        load_buffer_8x8(input, in, stride, 0, 1, shift[0]);
        fdct8x8_N4_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        transpose_8x8_sse4_1(out, in);
        fadst8x8_N4_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_sse4_1(out, in);
        write_buffer_8x8_N4(in, coeff);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_8x8(input, in, stride, 1, 1, shift[0]);
        fadst8x8_N4_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        transpose_8x8_sse4_1(out, in);
        fadst8x8_N4_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_sse4_1(out, in);
        write_buffer_8x8_N4(in, coeff);
        break;
    case ADST_FLIPADST:
        load_buffer_8x8(input, in, stride, 0, 1, shift[0]);
        fadst8x8_N4_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        transpose_8x8_sse4_1(out, in);
        fadst8x8_N4_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_sse4_1(out, in);
        write_buffer_8x8_N4(in, coeff);
        break;
    case FLIPADST_ADST:
        load_buffer_8x8(input, in, stride, 1, 0, shift[0]);
        fadst8x8_N4_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        transpose_8x8_sse4_1(out, in);
        fadst8x8_N4_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_sse4_1(out, in);
        write_buffer_8x8_N4(in, coeff);
        break;
    case IDTX:
        load_buffer_4x8_in_8x8(input, in, stride, 0, 0, shift[0], 1);
        clear_buffer_4x16_N2(in);
        fidtx8x8_N4_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        fidtx8x8_N4_sse4_1(out, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_8x8_N4(out, coeff);
        break;
    case V_DCT:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fdct8x8_N4_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        fidtx8x8_N4_sse4_1(out, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_8x8_N4(out, coeff);
        break;
    case H_DCT:
        load_buffer_4x8_in_8x8(input, in, stride, 0, 0, shift[0], 1);
        clear_buffer_4x16_N2(in);
        fidtx8x8_N4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(in, -shift[1]);
        transpose_8x8_sse4_1(in, out);
        fdct8x8_N4_sse4_1(out, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_sse4_1(in, out);
        write_buffer_8x8_N4(out, coeff);
        break;
    case V_ADST:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fadst8x8_N4_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        fidtx8x8_N4_sse4_1(out, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_8x8_N4(out, coeff);
        break;
    case H_ADST:
        load_buffer_4x8_in_8x8(input, in, stride, 0, 0, shift[0], 1);
        clear_buffer_4x16_N2(in);
        fidtx8x8_N4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(in, -shift[1]);
        transpose_8x8_sse4_1(in, out);
        fadst8x8_N4_sse4_1(out, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_sse4_1(in, out);
        write_buffer_8x8_N4(out, coeff);
        break;
    case V_FLIPADST:
        load_buffer_8x8(input, in, stride, 1, 0, shift[0]);
        fadst8x8_N4_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        fidtx8x8_N4_sse4_1(out, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_8x8_N4(out, coeff);
        break;
    case H_FLIPADST:
        load_buffer_4x8_in_8x8(input, in, stride, 0, 1, shift[0], 1);
        clear_buffer_4x16_N2(in);
        fidtx8x8_N4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(in, -shift[1]);
        transpose_8x8_sse4_1(in, out);
        fadst8x8_N4_sse4_1(out, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_sse4_1(in, out);
        write_buffer_8x8_N4(out, coeff);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_16x16_N4_sse4_1(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[64] = {_mm_setzero_si128()}, out[64] = {_mm_setzero_si128()};
    const int8_t* shift   = fwd_txfm_shift_ls[TX_16X16];
    const int32_t txw_idx = get_txw_idx(TX_16X16);
    const int32_t txh_idx = get_txh_idx(TX_16X16);
    const int32_t col_num = 2;
    switch (tx_type) {
    case IDTX:
        load_buffer_4x16_in_16x16(input, in, stride, 0, 0, shift[0]);
        fidtx16x16_N4_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, 2);
        col_txfm_16x16_N4_rounding(out, -shift[1]);
        fidtx16x16_N4_sse4_1(out, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, 2);
        write_buffer_16x16_N4(out, coeff);
        break;
    case DCT_DCT:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fdct16x16_N4_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2 * col_num, 2 * col_num);
        col_txfm_16x16_N4_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_sse4_1(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_sse4_1(out + 2, in + 32); //top-right ->bottom-left
        fdct16x16_N4_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 2 * col_num, 2);
        transpose_4x4_in_16x16_sse4_1(out, in);
        write_buffer_16x16_N4(in, coeff);
        break;
    case ADST_DCT:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fadst16x16_N4_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2 * col_num, 2 * col_num);
        col_txfm_16x16_N4_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_sse4_1(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_sse4_1(out + 2, in + 32); //top-right ->bottom-left
        fdct16x16_N4_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 2 * col_num, 2);
        transpose_4x4_in_16x16_sse4_1(out, in);
        write_buffer_16x16_N4(in, coeff);
        break;
    case DCT_ADST:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fdct16x16_N4_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2 * col_num, 2 * col_num);
        col_txfm_16x16_N4_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_sse4_1(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_sse4_1(out + 2, in + 32); //top-right ->bottom-left
        fadst16x16_N4_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 2 * col_num, 2);
        transpose_4x4_in_16x16_sse4_1(out, in);
        write_buffer_16x16_N4(in, coeff);
        break;
    case ADST_ADST:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fadst16x16_N4_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2 * col_num, 2 * col_num);
        col_txfm_16x16_N4_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_sse4_1(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_sse4_1(out + 2, in + 32); //top-right ->bottom-left
        fadst16x16_N4_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 2 * col_num, 2);
        transpose_4x4_in_16x16_sse4_1(out, in);
        write_buffer_16x16_N4(in, coeff);
        break;
    case DCT_FLIPADST:
        load_buffer_16x16(input, in, stride, 0, 1, shift[0]);
        fdct16x16_N4_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2 * col_num, 2 * col_num);
        col_txfm_16x16_N4_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_sse4_1(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_sse4_1(out + 2, in + 32); //top-right ->bottom-left
        fadst16x16_N4_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 2 * col_num, 2);
        transpose_4x4_in_16x16_sse4_1(out, in);
        write_buffer_16x16_N4(in, coeff);
        break;
    case FLIPADST_DCT:
        load_buffer_16x16(input, in, stride, 1, 0, shift[0]);
        fadst16x16_N4_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2 * col_num, 2 * col_num);
        col_txfm_16x16_N4_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_sse4_1(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_sse4_1(out + 2, in + 32); //top-right ->bottom-left
        fdct16x16_N4_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 2 * col_num, 2);
        transpose_4x4_in_16x16_sse4_1(out, in);
        write_buffer_16x16_N4(in, coeff);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_16x16(input, in, stride, 1, 1, shift[0]);
        fadst16x16_N4_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2 * col_num, 2 * col_num);
        col_txfm_16x16_N4_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_sse4_1(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_sse4_1(out + 2, in + 32); //top-right ->bottom-left
        fadst16x16_N4_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 2 * col_num, 2);
        transpose_4x4_in_16x16_sse4_1(out, in);
        write_buffer_16x16_N4(in, coeff);
        break;
    case ADST_FLIPADST:
        load_buffer_16x16(input, in, stride, 0, 1, shift[0]);
        fadst16x16_N4_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2 * col_num, 2 * col_num);
        col_txfm_16x16_N4_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_sse4_1(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_sse4_1(out + 2, in + 32); //top-right ->bottom-left
        fadst16x16_N4_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 2 * col_num, 2);
        transpose_4x4_in_16x16_sse4_1(out, in);
        write_buffer_16x16_N4(in, coeff);
        break;
    case FLIPADST_ADST:
        load_buffer_16x16(input, in, stride, 1, 0, shift[0]);
        fadst16x16_N4_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2 * col_num, 2 * col_num);
        col_txfm_16x16_N4_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_sse4_1(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_sse4_1(out + 2, in + 32); //top-right ->bottom-left
        fadst16x16_N4_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 2 * col_num, 2);
        transpose_4x4_in_16x16_sse4_1(out, in);
        write_buffer_16x16_N4(in, coeff);
        break;
    case V_DCT:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fdct16x16_N4_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2 * col_num, 2);
        col_txfm_16x16_N4_rounding(out, -shift[1]);
        fidtx16x16_N4_sse4_1(out, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, 2);
        write_buffer_16x16_N4(out, coeff);
        break;
    case H_DCT:
        load_buffer_4x16_in_16x16(input, in, stride, 0, 0, shift[0]);
        fidtx16x16_N4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], col_num, 1);
        col_txfm_16x16_N4_rounding(in, -shift[1]);
        transpose_8x8_in_16x16_sse4_1(in, out); //top-left -> top-left
        transpose_8x8_in_16x16_sse4_1(in + 2, out + 32); //top-right ->bottom-left
        fdct16x16_N4_sse4_1(out, in, fwd_cos_bit_row[txw_idx][txh_idx], 2 * col_num, 2);
        transpose_4x4_in_16x16_sse4_1(in, out);
        write_buffer_16x16_N4(out, coeff);
        break;
    case V_ADST:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fadst16x16_N4_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2 * col_num, 2);
        col_txfm_16x16_N4_rounding(out, -shift[1]);
        fidtx16x16_N4_sse4_1(out, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, 2);
        write_buffer_16x16_N4(out, coeff);
        break;
    case H_ADST:
        load_buffer_4x16_in_16x16(input, in, stride, 0, 0, shift[0]);
        fidtx16x16_N4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], col_num, 1);
        col_txfm_16x16_N4_rounding(in, -shift[1]);
        transpose_8x8_in_16x16_sse4_1(in, out); //top-left -> top-left
        transpose_8x8_in_16x16_sse4_1(in + 2, out + 32); //top-right ->bottom-left
        fadst16x16_N4_sse4_1(out, in, fwd_cos_bit_row[txw_idx][txh_idx], 2 * col_num, 2);
        transpose_4x4_in_16x16_sse4_1(in, out);
        write_buffer_16x16_N4(out, coeff);
        break;
    case V_FLIPADST:
        load_buffer_16x16(input, in, stride, 1, 0, shift[0]);
        fadst16x16_N4_sse4_1(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 2 * col_num, 2 * col_num);
        col_txfm_16x16_N4_rounding(out, -shift[1]);
        fidtx16x16_N4_sse4_1(out, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, 2);
        write_buffer_16x16_N4(out, coeff);
        break;
    case H_FLIPADST:
        load_buffer_4x16_in_16x16(input, in, stride, 0, 1, shift[0]);
        fidtx16x16_N4_sse4_1(in, in, fwd_cos_bit_col[txw_idx][txh_idx], col_num, 1);
        col_txfm_16x16_N4_rounding(in, -shift[1]);
        transpose_8x8_in_16x16_sse4_1(in, out); //top-left -> top-left
        transpose_8x8_in_16x16_sse4_1(in + 2, out + 32); //top-right ->bottom-left
        fadst16x16_N4_sse4_1(out, in, fwd_cos_bit_row[txw_idx][txh_idx], 2 * col_num, 2);
        transpose_4x4_in_16x16_sse4_1(in, out);
        write_buffer_16x16_N4(out, coeff);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_64x64_N4_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    __m128i       in[1024];
    __m128i*      out     = (__m128i*)output;
    const int32_t txw_idx = tx_size_wide_log2[TX_64X64] - tx_size_wide_log2[0];
    const int32_t txh_idx = tx_size_high_log2[TX_64X64] - tx_size_high_log2[0];
    const int8_t* shift   = fwd_txfm_shift_ls[TX_64X64];

    switch (tx_type) {
    case IDTX:
        load_buffer_16x16_in_64x64_sse4_1(input, stride, out);
        fidtx64x64_N4_sse4_1(out, in);
        av1_round_shift_array_64_N4_sse4_1(in, in, 512 / 4, -shift[1]);
        /*row wise transform*/
        fidtx64x64_N4_sse4_1(in, out);
        av1_round_shift_array_64_N4_sse4_1(out, out, 512 / 4, -shift[2]);
        clear_buffer_wxh_N4(out, 8, 64);
        break;
    case DCT_DCT:
        load_buffer_64x64_sse4_1(input, stride, out);
        av1_fdct64_new_N4_sse4_1(out, in, fwd_cos_bit_col[txw_idx][txh_idx], 64, 8);
        av1_round_shift_array_32_sse4_1(in, out, 1024 / 4, -shift[1]);
        transpose_16x16_in_64x64_sse4_1(out, in);
        transpose_16x16_in_64x64_sse4_1(out + 4, in + 256);
        transpose_16x16_in_64x64_sse4_1(out + 8, in + 512);
        transpose_16x16_in_64x64_sse4_1(out + 12, in + 768);
        /*row wise transform*/
        av1_fdct64_new_N4_sse4_1(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 16, 8);
        av1_round_shift_array_64_N4_sse4_1(out, in, 512 / 4, -shift[2]);
        transpose_16x16_in_64x64_sse4_1(in, out); //top-left
        clear_buffer_wxh_N4(out, 8, 64);
        break;
    default:
        assert(0);
    }
}

void svt_av1_fwd_txfm2d_32x32_N4_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    const int8_t* shift           = fwd_txfm_shift_ls[TX_32X32];
    const int32_t txw_idx         = tx_size_wide_log2[TX_32X32] - tx_size_wide_log2[0];
    const int32_t txh_idx         = tx_size_high_log2[TX_32X32] - tx_size_high_log2[0];
    const int8_t  cos_bit_col     = fwd_cos_bit_col[txw_idx][txh_idx];
    const int8_t  cos_bit_row     = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t txfm2d_size_128 = 32 * 32 / 4;
    __m128i       buf[256];
    __m128i*      out = (__m128i*)output;
    (void)bd;

    switch (tx_type) {
    case IDTX:
        load_buffer_8x8_in_32x32_sse4_1(input, buf, stride);
        av1_round_shift_array_32_N4_sse4_1(buf, out, 32, -shift[0]);
        fidtx_wxh_N4_sse4_1(out, buf, 32, 4);
        av1_round_shift_array_32_N4_sse4_1(buf, out, 32, -shift[1]);
        fidtx_wxh_N4_sse4_1(out, buf, 32, 4);
        av1_round_shift_array_32_N4_sse4_1(buf, buf, 32, -shift[2]);
        write_buffer_32x32_N4(buf, output);
        break;
    case DCT_DCT:
        load_buffer_32x32_sse4_1(input, buf, stride);
        av1_round_shift_array_32_sse4_1(buf, out, txfm2d_size_128, -shift[0]);
        fdct32x32_N4_col_sse4_1(out, buf, cos_bit_col);
        av1_round_shift_array_32_sse4_1(buf, out, txfm2d_size_128 / 4, -shift[1]);
        //transpose 32x8 to 8x32
        transpose_8x8_in_32x32_sse4_1(out, buf);
        transpose_8x8_in_32x32_sse4_1(out + 2, buf + 64);
        transpose_8x8_in_32x32_sse4_1(out + 4, buf + 128);
        transpose_8x8_in_32x32_sse4_1(out + 6, buf + 192);
        fdct32x32_N4_row_sse4_1(buf, out, cos_bit_row);
        av1_round_shift_array_32_N4_sse4_1(out, out, 32, -shift[2]);
        transpose_8x8_in_32x32_sse4_1(out, buf);
        write_buffer_32x32_N4(buf, output);
        break;
    case V_DCT:
        load_buffer_8x32_in_32x32_sse4_1(input, buf, stride);
        av1_round_shift_array_32_sse4_1(buf, out, txfm2d_size_128, -shift[0]);
        fdct32x32_N4_row_sse4_1(out, buf, cos_bit_col);
        av1_round_shift_array_32_N4_sse4_1(buf, out, 32, -shift[1]);
        fidtx_wxh_N4_sse4_1(out, buf, 32, 4);
        av1_round_shift_array_32_N4_sse4_1(buf, buf, 32, -shift[2]);
        write_buffer_32x32_N4(buf, output);
        break;
    case H_DCT:
        load_buffer_32x8_in_32x32_sse4_1(input, buf, stride);
        av1_round_shift_array_32_sse4_1(buf, out, txfm2d_size_128 / 4, -shift[0]);
        fidtx_wxh_N4_sse4_1(out, buf, 32, 1);
        av1_round_shift_array_32_sse4_1(buf, out, txfm2d_size_128 / 4, -shift[1]);
        //transpose 32x8 to 8x32
        transpose_8x8_in_32x32_sse4_1(out, buf);
        transpose_8x8_in_32x32_sse4_1(out + 2, buf + 64);
        transpose_8x8_in_32x32_sse4_1(out + 4, buf + 128);
        transpose_8x8_in_32x32_sse4_1(out + 6, buf + 192);
        fdct32x32_N4_row_sse4_1(buf, out, cos_bit_row);
        av1_round_shift_array_32_N4_sse4_1(out, out, 32, -shift[2]);
        transpose_8x8_in_32x32_sse4_1(out, buf);
        write_buffer_32x32_N4(buf, output);
        break;
    default:
        assert(0);
    }
}

void svt_av1_fwd_txfm2d_16x32_N4_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[128];
    __m128i*      outcoef       = (__m128i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_16X32];
    const int32_t txw_idx       = get_txw_idx(TX_16X32);
    const int32_t txh_idx       = get_txh_idx(TX_16X32);
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t txfm_size_col = tx_size_wide[TX_16X32];
    const int32_t txfm_size_row = tx_size_high[TX_16X32];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    switch (tx_type) {
    case IDTX:
        load_buffer_16x16_N2_half(input, in, stride, 0, 0, shift[0]);
        fidtx_wxh_N4_sse4_1(in, outcoef, 16, 2);
        col_txfm_16x16_N2_half_rounding(&outcoef[0], -shift[1]);
        fidtx16x16_N4_sse4_1(outcoef, in, bitrow, num_row, 2);
        av1_round_shift_rect_wxh_N4(in, outcoef, -shift[2], new_sqrt2, num_col, txfm_size_row);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case DCT_DCT:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        load_buffer_16x16(input + 16 * stride, in + 64, stride, 0, 0, shift[0]);
        for (int32_t i = 0; i < num_col; i++) {
            av1_fdct32_new_N4_sse4_1((in + 2 * i), (outcoef + 2 * i), bitcol, 8, num_col);
        }
        col_txfm_16x16_N2_rounding(&outcoef[0], -shift[1]);
        transpose_8nx8n_N4_half_sse4_1(outcoef, in, txfm_size_col, txfm_size_row);
        fdct16x16_N4_sse4_1(in, outcoef, bitrow, 2 * num_row, num_row);
        transpose_8nx8n_N4_quad_sse4_1(outcoef, in, txfm_size_row, txfm_size_col);
        av1_round_shift_rect_wxh_N4(in, outcoef, -shift[2], new_sqrt2, num_col, txfm_size_row);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

/* call this function only for IDTX */
void svt_av1_fwd_txfm2d_32x16_N4_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[128];
    __m128i*      outcoef       = (__m128i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_32X16];
    const int32_t txw_idx       = get_txw_idx(TX_32X16);
    const int32_t txh_idx       = get_txh_idx(TX_32X16);
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t txfm_size_col = tx_size_wide[TX_32X16];
    const int32_t txfm_size_row = tx_size_high[TX_32X16];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    switch (tx_type) {
    case IDTX:
        load_buffer_16x8n(input, in, stride, 0, 0, shift[0], txfm_size_row / 4);
        fidtx16x16_N4_sse4_1(in, outcoef, bitcol, 4, 4);
        col_txfm_32x16_N4_rounding(&outcoef[0], -shift[1]);
        fidtx_wxh_N4_sse4_1(outcoef, in, 16, 4);
        av1_round_shift_rect_wxh_N4(in, outcoef, -shift[2], new_sqrt2, num_col, txfm_size_row);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case DCT_DCT:
        load_buffer_32x8n(input, in, stride, 0, 0, shift[0], txfm_size_row);
        fdct16x16_N4_sse4_1(in, outcoef, bitcol, 2 * num_col, 2 * num_col);
        col_txfm_16x16_N4_rounding(&outcoef[0], -shift[1]);
        col_txfm_16x16_N4_rounding(&outcoef[16], -shift[1]);
        transpose_8nx8n_N4_half_sse4_1(outcoef, in, txfm_size_col, txfm_size_row);
        av1_fdct32_new_N4_sse4_1(in, outcoef, bitrow, 8, num_row);
        transpose_8nx8n_N4_quad_sse4_1(outcoef, in, txfm_size_row, txfm_size_col);
        av1_round_shift_rect_wxh_N4(in, outcoef, -shift[2], new_sqrt2, num_col, txfm_size_row);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

/* call this function only for DCT_DCT, IDTX */
void svt_av1_fwd_txfm2d_8x32_N4_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[64];
    __m128i*      outcoef = (__m128i*)output;
    const int8_t* shift   = fwd_txfm_shift_ls[TX_8X32];
    const int32_t txw_idx = get_txw_idx(TX_8X32);
    const int32_t txh_idx = get_txh_idx(TX_8X32);
    int8_t        bitcol  = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow  = fwd_cos_bit_row[txw_idx][txh_idx];

    const int32_t txfm_size_col = tx_size_wide[TX_8X32];
    const int32_t txfm_size_row = tx_size_high[TX_8X32];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    switch (tx_type) {
    case IDTX:
        load_buffer_8x16_N4(input, in, stride, 0, 0, shift[0]);
        fidtx_wxh_N4_sse4_1(in, in, 8, 1);
        col_txfm_16x16_N4_rounding(in, -shift[1]);
        // row transform
        fidtx32x8_N2_sse4_1(in, outcoef, bitrow, num_col, 8);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case DCT_DCT:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        load_buffer_8x16(input + (txfm_size_row >> 1) * stride, in + 32, stride, 0, 0, shift[0]);
        av1_fdct32_new_N4_sse4_1(in, in, bitcol, 8, num_col);
        col_txfm_16x16_N4_rounding(in, -shift[1]);
        transpose_8nx8n_N4_half_sse4_1(in, outcoef, txfm_size_col, txfm_size_row);
        // row transform
        fdct8x8_N4_sse4_1(outcoef, in, bitrow, num_row);
        transpose_8nx8n_N4_quad_sse4_1(in, outcoef, txfm_size_row, txfm_size_col);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

/* call this function only for DCT_DCT, IDTX */
void svt_av1_fwd_txfm2d_32x8_N4_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[64];
    __m128i*      outcoef = (__m128i*)output;
    const int8_t* shift   = fwd_txfm_shift_ls[TX_32X8];
    const int32_t txw_idx = get_txw_idx(TX_32X8);
    const int32_t txh_idx = get_txh_idx(TX_32X8);
    int8_t        bitcol  = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow  = fwd_cos_bit_row[txw_idx][txh_idx];

    const int32_t txfm_size_col = tx_size_wide[TX_32X8];
    const int32_t txfm_size_row = tx_size_high[TX_32X8];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    switch (tx_type) {
    case IDTX:
        load_buffer_16x8n(input, in, stride, 0, 0, shift[0], txfm_size_row / 4);
        fidtx32x8_N2_sse4_1(in, in, bitcol, num_col, 2);
        col_txfm_32x8_N4_rounding(&in[0], -shift[1]);
        // row transform
        fidtx_wxh_N4_sse4_1(in, outcoef, 8, 4);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case DCT_DCT:
        load_buffer_32x8n(input, in, stride, 0, 0, shift[0], txfm_size_row);
        for (int32_t i = 0; i < num_col; i++) {
            fdct8x8_N4_sse4_1((in + 2 * i), (in + 2 * i), bitcol, num_col);
        }
        col_txfm_16x16_N4_rounding(&in[0], -shift[1]);
        transpose_8nx8n_N4_half_sse4_1(in, outcoef, txfm_size_col, txfm_size_row);
        // row transform
        av1_fdct32_new_N4_sse4_1(outcoef, in, bitrow, 8, num_row);
        transpose_8nx8n_N4_quad_sse4_1(in, outcoef, txfm_size_row, txfm_size_col);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

/* call this function for all 16 transform types */
void svt_av1_fwd_txfm2d_8x16_N4_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[32], out[32] = {_mm_setzero_si128()};
    __m128i*      outcoef       = (__m128i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_8X16];
    const int32_t txw_idx       = get_txw_idx(TX_8X16);
    const int32_t txh_idx       = get_txh_idx(TX_8X16);
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t txfm_size_col = tx_size_wide[TX_8X16];
    const int32_t txfm_size_row = tx_size_high[TX_8X16];
    const int32_t num_col       = txfm_size_col >> 3;

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        fdct16x16_N4_sse4_1(in, in, bitcol, 2 * num_col, 2 * num_col);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_sse4_1(in, out);
        fdct8x8_N4_sse4_1(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, out, 4, -shift[2]);
        transpose_8x8_N2_sse4_1(out, outcoef);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case ADST_DCT:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        fadst16x16_N4_sse4_1(in, in, bitcol, 2 * num_col, 2 * num_col);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_sse4_1(in, out);
        fdct8x8_N4_sse4_1(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, out, 4, -shift[2]);
        transpose_8x8_N2_sse4_1(out, outcoef);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case DCT_ADST:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        fdct16x16_N4_sse4_1(in, in, bitcol, 2 * num_col, 2 * num_col);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_sse4_1(in, out);
        fadst8x8_N4_sse4_1(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, out, 4, -shift[2]);
        transpose_8x8_N2_sse4_1(out, outcoef);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case ADST_ADST:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        fadst16x16_N4_sse4_1(in, in, bitcol, 2 * num_col, 2 * num_col);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_sse4_1(in, out);
        fadst8x8_N4_sse4_1(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, out, 4, -shift[2]);
        transpose_8x8_N2_sse4_1(out, outcoef);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case FLIPADST_DCT:
        load_buffer_8x16(input, in, stride, 1, 0, shift[0]);
        fadst16x16_N4_sse4_1(in, in, bitcol, 2 * num_col, 2 * num_col);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_sse4_1(in, out);
        fdct8x8_N4_sse4_1(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, out, 4, -shift[2]);
        transpose_8x8_N2_sse4_1(out, outcoef);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case DCT_FLIPADST:
        load_buffer_8x16(input, in, stride, 0, 1, shift[0]);
        fdct16x16_N4_sse4_1(in, in, bitcol, 2 * num_col, 2 * num_col);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_sse4_1(in, out);
        fadst8x8_N4_sse4_1(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, out, 4, -shift[2]);
        transpose_8x8_N2_sse4_1(out, outcoef);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_8x16(input, in, stride, 1, 1, shift[0]);
        fadst16x16_N4_sse4_1(in, in, bitcol, 2 * num_col, 2 * num_col);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_sse4_1(in, out);
        fadst8x8_N4_sse4_1(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, out, 4, -shift[2]);
        transpose_8x8_N2_sse4_1(out, outcoef);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case ADST_FLIPADST:
        load_buffer_8x16(input, in, stride, 0, 1, shift[0]);
        fadst16x16_N4_sse4_1(in, in, bitcol, 2 * num_col, 2 * num_col);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_sse4_1(in, out);
        fadst8x8_N4_sse4_1(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, out, 4, -shift[2]);
        transpose_8x8_N2_sse4_1(out, outcoef);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case FLIPADST_ADST:
        load_buffer_8x16(input, in, stride, 1, 0, shift[0]);
        fadst16x16_N4_sse4_1(in, in, bitcol, 2 * num_col, 2 * num_col);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_sse4_1(in, out);
        fadst8x8_N4_sse4_1(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, out, 4, -shift[2]);
        transpose_8x8_N2_sse4_1(out, outcoef);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case IDTX:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fidtx8xn_N2_col_sse4_1(in, in, bitcol, 4);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        fidtx8x8_N2_sse4_1(in, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, outcoef, 8, -shift[2]);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case V_DCT:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        fdct16x16_N4_sse4_1(in, in, bitcol, 2 * num_col, 2 * num_col);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        fidtx8x8_N2_sse4_1(in, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, outcoef, 8, -shift[2]);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case H_DCT:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fidtx8xn_N2_col_sse4_1(in, in, bitcol, 4);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_sse4_1(in, out);
        fdct8x8_N4_sse4_1(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, out, 4, -shift[2]);
        transpose_8x8_N2_sse4_1(out, outcoef);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case V_ADST:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        fadst16x16_N4_sse4_1(in, in, bitcol, 2 * num_col, 2 * num_col);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        fidtx8x8_N2_sse4_1(in, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, outcoef, 8, -shift[2]);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case H_ADST:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fidtx8xn_N2_col_sse4_1(in, in, bitcol, 4);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_sse4_1(in, out);
        fadst8x8_N4_sse4_1(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, out, 4, -shift[2]);
        transpose_8x8_N2_sse4_1(out, outcoef);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case V_FLIPADST:
        load_buffer_8x16(input, in, stride, 1, 0, shift[0]);
        fadst16x16_N4_sse4_1(in, in, bitcol, 2 * num_col, 2 * num_col);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        fidtx8x8_N2_sse4_1(in, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, outcoef, 8, -shift[2]);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case H_FLIPADST:
        load_buffer_8x8(input, in, stride, 0, 1, shift[0]);
        fidtx8xn_N2_col_sse4_1(in, in, bitcol, 4);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_sse4_1(in, out);
        fadst8x8_N4_sse4_1(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, out, 4, -shift[2]);
        transpose_8x8_N2_sse4_1(out, outcoef);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

/* call this function for all 16 transform types */
void svt_av1_fwd_txfm2d_16x8_N4_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[32], out[32] = {_mm_setzero_si128()};
    __m128i*      outcoef       = (__m128i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_16X8];
    const int32_t txw_idx       = get_txw_idx(TX_16X8);
    const int32_t txh_idx       = get_txh_idx(TX_16X8);
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t txfm_size_col = tx_size_wide[TX_16X8];
    const int32_t txfm_size_row = tx_size_high[TX_16X8];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;
    assert(num_col > 0);
    // column transform
    switch (tx_type) {
    case DCT_DCT:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fdct8x8_N4_sse4_1(in, in, bitcol, 1);
            col_txfm_8x8_N4_rounding(in, -shift[1]);
            transpose_8x8_half_sse4_1(in, out + i * 16);
        }
        fdct16x16_N4_sse4_1(out, out, bitrow, 2 * num_row, 2);
        transpose_8x8_N2_sse4_1(out, in);
        av1_round_shift_rect_array_32_sse4_1(in, out, 4, -shift[2]);
        write_buffer_16x8_N4_sse4_1(out, outcoef);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case ADST_DCT:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fadst8x8_N4_sse4_1(in, in, bitcol, 1);
            col_txfm_8x8_N4_rounding(in, -shift[1]);
            transpose_8x8_half_sse4_1(in, out + i * 16);
        }
        fdct16x16_N4_sse4_1(out, out, bitrow, 2 * num_row, 2);
        transpose_8x8_N2_sse4_1(out, in);
        av1_round_shift_rect_array_32_sse4_1(in, out, 4, -shift[2]);
        write_buffer_16x8_N4_sse4_1(out, outcoef);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case DCT_ADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fdct8x8_N4_sse4_1(in, in, bitcol, 1);
            col_txfm_8x8_N4_rounding(in, -shift[1]);
            transpose_8x8_half_sse4_1(in, out + i * 16);
        }
        fadst16x16_N4_sse4_1(out, out, bitrow, 2 * num_row, 2);
        transpose_8x8_N2_sse4_1(out, in);
        av1_round_shift_rect_array_32_sse4_1(in, out, 4, -shift[2]);
        write_buffer_16x8_N4_sse4_1(out, outcoef);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case ADST_ADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fadst8x8_N4_sse4_1(in, in, bitcol, 1);
            col_txfm_8x8_N4_rounding(in, -shift[1]);
            transpose_8x8_half_sse4_1(in, out + i * 16);
        }
        fadst16x16_N4_sse4_1(out, out, bitrow, 2 * num_row, 2);
        transpose_8x8_N2_sse4_1(out, in);
        av1_round_shift_rect_array_32_sse4_1(in, out, 4, -shift[2]);
        write_buffer_16x8_N4_sse4_1(out, outcoef);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case FLIPADST_DCT:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 1, 0, shift[0]);
            fadst8x8_N4_sse4_1(in, in, bitcol, 1);
            col_txfm_8x8_N4_rounding(in, -shift[1]);
            transpose_8x8_half_sse4_1(in, out + i * 16);
        }
        fdct16x16_N4_sse4_1(out, out, bitrow, 2 * num_row, 2);
        transpose_8x8_N2_sse4_1(out, in);
        av1_round_shift_rect_array_32_sse4_1(in, out, 4, -shift[2]);
        write_buffer_16x8_N4_sse4_1(out, outcoef);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case DCT_FLIPADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fdct8x8_N4_sse4_1(in, in, bitcol, 1);
            col_txfm_8x8_N4_rounding(in, -shift[1]);
            transpose_8x8_half_sse4_1(in, out + i * 16);
        }
        for (int32_t i = 0; i < 16; i++) {
            in[30 - 2 * i] = out[2 * i];
            in[31 - 2 * i] = out[2 * i + 1];
        }
        fadst16x16_N4_sse4_1(in, out, bitrow, 2 * num_row, 2);
        transpose_8x8_N2_sse4_1(out, in);
        av1_round_shift_rect_array_32_sse4_1(in, out, 4, -shift[2]);
        write_buffer_16x8_N4_sse4_1(out, outcoef);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case FLIPADST_FLIPADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 1, 0, shift[0]);
            fadst8x8_N4_sse4_1(in, in, bitcol, 1);
            col_txfm_8x8_N4_rounding(in, -shift[1]);
            transpose_8x8_half_sse4_1(in, out + i * 16);
        }
        for (int32_t i = 0; i < 16; i++) {
            in[30 - 2 * i] = out[2 * i];
            in[31 - 2 * i] = out[2 * i + 1];
        }
        fadst16x16_N4_sse4_1(in, out, bitrow, 2 * num_row, 2);
        transpose_8x8_N2_sse4_1(out, in);
        av1_round_shift_rect_array_32_sse4_1(in, out, 4, -shift[2]);
        write_buffer_16x8_N4_sse4_1(out, outcoef);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case ADST_FLIPADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fadst8x8_N4_sse4_1(in, in, bitcol, 1);
            col_txfm_8x8_N4_rounding(in, -shift[1]);
            transpose_8x8_half_sse4_1(in, out + i * 16);
        }
        for (int32_t i = 0; i < 16; i++) {
            in[30 - 2 * i] = out[2 * i];
            in[31 - 2 * i] = out[2 * i + 1];
        }
        fadst16x16_N4_sse4_1(in, out, bitrow, 2 * num_row, 2);
        transpose_8x8_N2_sse4_1(out, in);
        av1_round_shift_rect_array_32_sse4_1(in, out, 4, -shift[2]);
        write_buffer_16x8_N4_sse4_1(out, outcoef);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case FLIPADST_ADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 1, 0, shift[0]);
            fadst8x8_N4_sse4_1(in, in, bitcol, 1);
            col_txfm_8x8_N4_rounding(in, -shift[1]);
            transpose_8x8_half_sse4_1(in, out + i * 16);
        }
        fadst16x16_N4_sse4_1(out, out, bitrow, 2 * num_row, 2);
        transpose_8x8_N2_sse4_1(out, in);
        av1_round_shift_rect_array_32_sse4_1(in, out, 4, -shift[2]);
        write_buffer_16x8_N4_sse4_1(out, outcoef);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case IDTX:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fidtx8x8_N4_sse4_1(in, out, bitcol, 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        fidtx8xn_N2_col_sse4_1(out, out, bitrow, 2);
        av1_round_shift_rect_array_32_sse4_1(out, out, 4, -shift[2]);
        write_buffer_16x8_N4_sse4_1(out, outcoef);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case V_DCT:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fdct8x8_N4_sse4_1(in, out, bitcol, 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        fidtx8xn_N2_col_sse4_1(out, out, bitrow, 2);
        av1_round_shift_rect_array_32_sse4_1(out, out, 4, -shift[2]);
        write_buffer_16x8_N4_sse4_1(out, outcoef);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case H_DCT:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fidtx8x8_N4_sse4_1(in, in, bitcol, 1);
            col_txfm_8x8_N4_rounding(in, -shift[1]);
            transpose_8x8_half_sse4_1(in, out + i * 16);
        }
        fdct16x16_N4_sse4_1(out, out, bitrow, 2 * num_row, 2);
        transpose_8x8_N2_sse4_1(out, in);
        av1_round_shift_rect_array_32_sse4_1(in, out, 4, -shift[2]);
        write_buffer_16x8_N4_sse4_1(out, outcoef);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case V_ADST:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fadst8x8_N4_sse4_1(in, out, bitcol, 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        fidtx8xn_N2_col_sse4_1(out, out, bitrow, 2);
        av1_round_shift_rect_array_32_sse4_1(out, out, 4, -shift[2]);
        write_buffer_16x8_N4_sse4_1(out, outcoef);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case H_ADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fidtx8x8_N4_sse4_1(in, in, bitcol, 1);
            col_txfm_8x8_N4_rounding(in, -shift[1]);
            transpose_8x8_half_sse4_1(in, out + i * 16);
        }
        fadst16x16_N4_sse4_1(out, out, bitrow, 2 * num_row, 2);
        transpose_8x8_N2_sse4_1(out, in);
        av1_round_shift_rect_array_32_sse4_1(in, out, 4, -shift[2]);
        write_buffer_16x8_N4_sse4_1(out, outcoef);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case V_FLIPADST:
        load_buffer_8x8(input, in, stride, 1, 0, shift[0]);
        fadst8x8_N4_sse4_1(in, out, bitcol, 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        fidtx8xn_N2_col_sse4_1(out, out, bitrow, 2);
        av1_round_shift_rect_array_32_sse4_1(out, out, 4, -shift[2]);
        write_buffer_16x8_N4_sse4_1(out, outcoef);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    case H_FLIPADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fidtx8x8_N4_sse4_1(in, in, bitcol, 1);
            col_txfm_8x8_N4_rounding(in, -shift[1]);
            transpose_8x8_half_sse4_1(in, out + i * 16);
        }
        for (int32_t i = 0; i < 16; i++) {
            in[30 - 2 * i] = out[2 * i];
            in[31 - 2 * i] = out[2 * i + 1];
        }
        fadst16x16_N2_sse4_1(in, out, bitrow, num_row, 1);
        transpose_8x8_N2_sse4_1(out, in);
        av1_round_shift_rect_array_32_sse4_1(in, out, 4, -shift[2]);
        write_buffer_16x8_N4_sse4_1(out, outcoef);
        clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_4x8_N4_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i in[8];
    __m128i outcoeff[8];

    const int8_t* shift   = fwd_txfm_shift_ls[TX_4X8];
    const int32_t txw_idx = get_txw_idx(TX_4X8);
    const int32_t txh_idx = get_txh_idx(TX_4X8);
    int32_t       bitcol  = fwd_cos_bit_col[txw_idx][txh_idx];
    int32_t       bitrow  = fwd_cos_bit_row[txw_idx][txh_idx];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_4x8_sse4_1(input, in, stride, 0, 0, shift[0]);
        fdct4x8_N4_sse4_1(in, in, bitcol);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        transpose_4x8_sse4_1(in, outcoeff);
        fdct4x8_col_N4_sse4_1(outcoeff, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        write_buffer_4x8_N4(outcoeff, output);
        break;
    case ADST_DCT:
        load_buffer_4x8_sse4_1(input, in, stride, 0, 0, shift[0]);
        fadst8x4_N4_sse4_1(in, in, bitcol, 1);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        transpose_4x8_sse4_1(in, outcoeff);
        fdct4x8_col_N4_sse4_1(outcoeff, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        write_buffer_4x8_N4(outcoeff, output);
        break;
    case DCT_ADST:
        load_buffer_4x8_sse4_1(input, in, stride, 0, 0, shift[0]);
        fdct4x8_N4_sse4_1(in, in, bitcol);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        transpose_4x8_sse4_1(in, outcoeff);
        fadst4x8_col_N4_sse4_1(outcoeff, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        write_buffer_4x8_N4(outcoeff, output);
        break;
    case ADST_ADST:
        load_buffer_4x8_sse4_1(input, in, stride, 0, 0, shift[0]);
        fadst8x4_N4_sse4_1(in, in, bitcol, 1);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        transpose_4x8_sse4_1(in, outcoeff);
        fadst4x8_col_N4_sse4_1(outcoeff, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        write_buffer_4x8_N4(outcoeff, output);
        break;
    case FLIPADST_DCT:
        load_buffer_4x8_sse4_1(input, in, stride, 1, 0, shift[0]);
        fadst8x4_N4_sse4_1(in, in, bitcol, 1);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        transpose_4x8_sse4_1(in, outcoeff);
        fdct4x8_col_N4_sse4_1(outcoeff, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        write_buffer_4x8_N4(outcoeff, output);
        break;
    case DCT_FLIPADST:
        load_buffer_4x8_sse4_1(input, in, stride, 0, 1, shift[0]);
        fdct4x8_N4_sse4_1(in, in, bitcol);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        transpose_4x8_sse4_1(in, outcoeff);
        fadst4x8_col_N4_sse4_1(outcoeff, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        write_buffer_4x8_N4(outcoeff, output);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_4x8_sse4_1(input, in, stride, 1, 1, shift[0]);
        fadst8x4_N4_sse4_1(in, in, bitcol, 1);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        transpose_4x8_sse4_1(in, outcoeff);
        fadst4x8_col_N4_sse4_1(outcoeff, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        write_buffer_4x8_N4(outcoeff, output);
        break;
    case ADST_FLIPADST:
        load_buffer_4x8_sse4_1(input, in, stride, 0, 1, shift[0]);
        fadst8x4_N4_sse4_1(in, in, bitcol, 1);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        transpose_4x8_sse4_1(in, outcoeff);
        fadst4x8_col_N4_sse4_1(outcoeff, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        write_buffer_4x8_N4(outcoeff, output);
        break;
    case FLIPADST_ADST:
        load_buffer_4x8_sse4_1(input, in, stride, 1, 0, shift[0]);
        fadst8x4_N4_sse4_1(in, in, bitcol, 1);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        transpose_4x8_sse4_1(in, outcoeff);
        fadst4x8_col_N4_sse4_1(outcoeff, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        write_buffer_4x8_N4(outcoeff, output);
        break;
    case IDTX:
        load_buffer_4x8_sse4_1(input, in, stride, 0, 0, shift[0]);
        fidtx8x4_N4_sse4_1(in, in, bitcol);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        fidtx4x8_col_N4_sse4_1(in, in, bitrow, 2);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        write_buffer_4x8_N4(outcoeff, output);
        break;
    case V_DCT:
        load_buffer_4x8_sse4_1(input, in, stride, 0, 0, shift[0]);
        fdct4x8_N4_sse4_1(in, in, bitcol);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        fidtx4x8_col_N4_sse4_1(in, in, bitrow, 2);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        write_buffer_4x8_N4(outcoeff, output);
        break;
    case H_DCT:
        load_buffer_4x8_sse4_1(input, in, stride, 0, 0, shift[0]);
        fidtx8x4_N4_sse4_1(in, in, bitcol);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        transpose_4x8_sse4_1(in, outcoeff);
        fdct4x8_col_N4_sse4_1(outcoeff, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        write_buffer_4x8_N4(outcoeff, output);
        break;
    case V_ADST:
        load_buffer_4x8_sse4_1(input, in, stride, 0, 0, shift[0]);
        fadst8x4_N4_sse4_1(in, in, bitcol, 1);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        fidtx4x8_col_N4_sse4_1(in, in, bitrow, 2);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        write_buffer_4x8_N4(outcoeff, output);
        break;
    case H_ADST:
        load_buffer_4x8_sse4_1(input, in, stride, 0, 0, shift[0]);
        fidtx8x4_N4_sse4_1(in, in, bitcol);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        transpose_4x8_sse4_1(in, outcoeff);
        fadst4x8_col_N4_sse4_1(outcoeff, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        write_buffer_4x8_N4(outcoeff, output);
        break;
    case V_FLIPADST:
        load_buffer_4x8_sse4_1(input, in, stride, 1, 0, shift[0]);
        fadst8x4_N4_sse4_1(in, in, bitcol, 1);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        fidtx4x8_col_N4_sse4_1(in, in, bitrow, 2);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        write_buffer_4x8_N4(outcoeff, output);
        break;
    case H_FLIPADST:
        load_buffer_4x8_sse4_1(input, in, stride, 0, 1, shift[0]);
        fidtx8x4_N4_sse4_1(in, in, bitcol);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        transpose_4x8_sse4_1(in, outcoeff);
        fadst4x8_col_N4_sse4_1(outcoeff, in, bitrow, 1);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        write_buffer_4x8_N4(outcoeff, output);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_8x4_N4_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[8];
    __m128i*      outcoeff = (__m128i*)output;
    const int8_t* shift    = fwd_txfm_shift_ls[TX_8X4];
    const int32_t txw_idx  = get_txw_idx(TX_8X4);
    const int32_t txh_idx  = get_txh_idx(TX_8X4);
    int32_t       bitcol   = fwd_cos_bit_col[txw_idx][txh_idx];
    int32_t       bitrow   = fwd_cos_bit_row[txw_idx][txh_idx];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_8x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fdct4x8_row_N4_with_round_sse4_1(in, in, bitcol, -shift[1]);
        fdct4x8_N4_sse4_1(in, outcoeff, bitrow);
        transpose_4x8_in_4x16_quad_sse4_1(outcoeff, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        clear_buffer_wxh_N4(outcoeff, 1, 4);
        break;
    case ADST_DCT:
        load_buffer_8x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fadst4x8_row_N4_with_round_sse4_1(in, in, bitcol, -shift[1]);
        fdct4x8_N4_sse4_1(in, outcoeff, bitrow);
        transpose_4x8_in_4x16_quad_sse4_1(outcoeff, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        clear_buffer_wxh_N4(outcoeff, 1, 4);
        break;
    case DCT_ADST:
        load_buffer_8x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fdct4x8_row_N4_with_round_sse4_1(in, in, bitcol, -shift[1]);
        fadst8x4_N4_sse4_1(in, outcoeff, bitrow, 1);
        transpose_4x8_in_4x16_quad_sse4_1(outcoeff, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        clear_buffer_wxh_N4(outcoeff, 1, 4);
        break;
    case ADST_ADST:
        load_buffer_8x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fadst4x8_row_N4_with_round_sse4_1(in, in, bitcol, -shift[1]);
        fadst8x4_N4_sse4_1(in, outcoeff, bitrow, 1);
        transpose_4x8_in_4x16_quad_sse4_1(outcoeff, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        clear_buffer_wxh_N4(outcoeff, 1, 4);
        break;
    case FLIPADST_DCT:
        load_buffer_8x4_sse4_1(input, in, stride, 1, 0, shift[0]);
        fadst4x8_row_N4_with_round_sse4_1(in, in, bitcol, -shift[1]);
        fdct4x8_N4_sse4_1(in, outcoeff, bitrow);
        transpose_4x8_in_4x16_quad_sse4_1(outcoeff, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        clear_buffer_wxh_N4(outcoeff, 1, 4);
        break;
    case DCT_FLIPADST:
        load_buffer_8x4_sse4_1(input, in, stride, 0, 1, shift[0]);
        fdct4x8_row_N4_with_round_sse4_1(in, in, bitcol, -shift[1]);
        fadst8x4_N4_sse4_1(in, outcoeff, bitrow, 1);
        transpose_4x8_in_4x16_quad_sse4_1(outcoeff, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        clear_buffer_wxh_N4(outcoeff, 1, 4);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_8x4_sse4_1(input, in, stride, 1, 1, shift[0]);
        fadst4x8_row_N4_with_round_sse4_1(in, in, bitcol, -shift[1]);
        fadst8x4_N4_sse4_1(in, outcoeff, bitrow, 1);
        transpose_4x8_in_4x16_quad_sse4_1(outcoeff, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        clear_buffer_wxh_N4(outcoeff, 1, 4);
        break;
    case ADST_FLIPADST:
        load_buffer_8x4_sse4_1(input, in, stride, 0, 1, shift[0]);
        fadst4x8_row_N4_with_round_sse4_1(in, in, bitcol, -shift[1]);
        fadst8x4_N4_sse4_1(in, outcoeff, bitrow, 1);
        transpose_4x8_in_4x16_quad_sse4_1(outcoeff, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        clear_buffer_wxh_N4(outcoeff, 1, 4);
        break;
    case FLIPADST_ADST:
        load_buffer_8x4_sse4_1(input, in, stride, 1, 0, shift[0]);
        fadst4x8_row_N4_with_round_sse4_1(in, in, bitcol, -shift[1]);
        fadst8x4_N4_sse4_1(in, outcoeff, bitrow, 1);
        transpose_4x8_in_4x16_quad_sse4_1(outcoeff, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        clear_buffer_wxh_N4(outcoeff, 1, 4);
        break;
    case IDTX:
        load_buffer_8x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fidtx4x8_row_N4_with_round_sse4_1(in, in, bitcol, -shift[1]);
        fidtx8x4_N2_sse4_1(in, outcoeff, bitrow);
        transpose_4x8_in_4x16_quad_sse4_1(outcoeff, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        clear_buffer_wxh_N4(outcoeff, 1, 4);
        break;
    case V_DCT:
        load_buffer_8x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fdct4x8_row_N4_with_round_sse4_1(in, in, bitcol, -shift[1]);
        fidtx8x4_N2_sse4_1(in, outcoeff, bitrow);
        transpose_4x8_in_4x16_quad_sse4_1(outcoeff, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        clear_buffer_wxh_N4(outcoeff, 1, 4);
        break;
    case H_DCT:
        load_buffer_8x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fidtx4x8_row_N4_with_round_sse4_1(in, in, bitcol, -shift[1]);
        fdct4x8_N4_sse4_1(in, outcoeff, bitrow);
        transpose_4x8_in_4x16_quad_sse4_1(outcoeff, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        clear_buffer_wxh_N4(outcoeff, 1, 4);
        break;
    case V_ADST:
        load_buffer_8x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fadst4x8_row_N4_with_round_sse4_1(in, in, bitcol, -shift[1]);
        fidtx8x4_N2_sse4_1(in, outcoeff, bitrow);
        transpose_4x8_in_4x16_quad_sse4_1(outcoeff, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        clear_buffer_wxh_N4(outcoeff, 1, 4);
        break;
    case H_ADST:
        load_buffer_8x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fidtx4x8_row_N4_with_round_sse4_1(in, in, bitcol, -shift[1]);
        fadst8x4_N4_sse4_1(in, outcoeff, bitrow, 1);
        transpose_4x8_in_4x16_quad_sse4_1(outcoeff, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        clear_buffer_wxh_N4(outcoeff, 1, 4);
        break;
    case V_FLIPADST:
        load_buffer_8x4_sse4_1(input, in, stride, 1, 0, shift[0]);
        fadst4x8_row_N4_with_round_sse4_1(in, in, bitcol, -shift[1]);
        fidtx8x4_N2_sse4_1(in, outcoeff, bitrow);
        transpose_4x8_in_4x16_quad_sse4_1(outcoeff, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        clear_buffer_wxh_N4(outcoeff, 1, 4);
        break;
    case H_FLIPADST:
        load_buffer_8x4_sse4_1(input, in, stride, 0, 1, shift[0]);
        fidtx4x8_row_N4_with_round_sse4_1(in, in, bitcol, -shift[1]);
        fadst8x4_N4_sse4_1(in, outcoeff, bitrow, 1);
        transpose_4x8_in_4x16_quad_sse4_1(outcoeff, in);
        av1_round_shift_rect_array_32_sse4_1(in, outcoeff, 2, -shift[2]);
        clear_buffer_wxh_N4(outcoeff, 1, 4);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_4x16_N4_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[16];
    __m128i*      outcoeff = (__m128i*)output;
    const int8_t* shift    = fwd_txfm_shift_ls[TX_4X16];
    const int32_t txw_idx  = get_txw_idx(TX_4X16);
    const int32_t txh_idx  = get_txh_idx(TX_4X16);
    int32_t       bitcol   = fwd_cos_bit_col[txw_idx][txh_idx];
    int32_t       bitrow   = fwd_cos_bit_row[txw_idx][txh_idx];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_4x16_sse4_1(input, in, stride, 0, 0, shift[0]);
        fdct16x4_N4_sse4_1(in, outcoeff, bitcol);
        col_txfm_8x8_N4_rounding(outcoeff, -shift[1]);
        transpose_4x8_in_4x16_sse4_1(outcoeff, in);
        fdct4x8_col_N4_sse4_1(in, outcoeff, bitrow, 2); //dct + transpose + clear right half of buffer
        clear_buffer_4x16_N4(outcoeff);
        break;
    case ADST_DCT:
        load_buffer_4x16_sse4_1(input, in, stride, 0, 0, shift[0]);
        fadst16x4_N4_sse4_1(in, outcoeff, bitcol);
        col_txfm_8x8_N4_rounding(outcoeff, -shift[1]);
        transpose_4x8_in_4x16_sse4_1(outcoeff, in);
        fdct4x8_col_N4_sse4_1(in, outcoeff, bitrow, 2);
        clear_buffer_4x16_N4(outcoeff);
        break;
    case DCT_ADST:
        load_buffer_4x16_sse4_1(input, in, stride, 0, 0, shift[0]);
        fdct16x4_N4_sse4_1(in, outcoeff, bitcol);
        col_txfm_8x8_N4_rounding(outcoeff, -shift[1]);
        transpose_4x8_in_4x16_sse4_1(outcoeff, in);
        fadst4x8_col_N4_sse4_1(in, outcoeff, bitrow, 2);
        clear_buffer_4x16_N4(outcoeff);
        break;
    case ADST_ADST:
        load_buffer_4x16_sse4_1(input, in, stride, 0, 0, shift[0]);
        fadst16x4_N4_sse4_1(in, outcoeff, bitcol);
        col_txfm_8x8_N4_rounding(outcoeff, -shift[1]);
        transpose_4x8_in_4x16_sse4_1(outcoeff, in);
        fadst4x8_col_N4_sse4_1(in, outcoeff, bitrow, 2);
        clear_buffer_4x16_N4(outcoeff);
        break;
    case FLIPADST_DCT:
        load_buffer_4x16_sse4_1(input, in, stride, 1, 0, shift[0]);
        fadst16x4_N4_sse4_1(in, outcoeff, bitcol);
        col_txfm_8x8_N4_rounding(outcoeff, -shift[1]);
        transpose_4x8_in_4x16_sse4_1(outcoeff, in);
        fdct4x8_col_N4_sse4_1(in, outcoeff, bitrow, 2);
        clear_buffer_4x16_N4(outcoeff);
        break;
    case DCT_FLIPADST:
        load_buffer_4x16_sse4_1(input, in, stride, 0, 1, shift[0]);
        fdct16x4_N4_sse4_1(in, outcoeff, bitcol);
        col_txfm_8x8_N4_rounding(outcoeff, -shift[1]);
        transpose_4x8_in_4x16_sse4_1(outcoeff, in);
        fadst4x8_col_N4_sse4_1(in, outcoeff, bitrow, 2);
        clear_buffer_4x16_N4(outcoeff);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_4x16_sse4_1(input, in, stride, 1, 1, shift[0]);
        fadst16x4_N4_sse4_1(in, outcoeff, bitcol);
        col_txfm_8x8_N4_rounding(outcoeff, -shift[1]);
        transpose_4x8_in_4x16_sse4_1(outcoeff, in);
        fadst4x8_col_N4_sse4_1(in, outcoeff, bitrow, 2);
        clear_buffer_4x16_N4(outcoeff);
        break;
    case ADST_FLIPADST:
        load_buffer_4x16_sse4_1(input, in, stride, 0, 1, shift[0]);
        fadst16x4_N4_sse4_1(in, outcoeff, bitcol);
        col_txfm_8x8_N4_rounding(outcoeff, -shift[1]);
        transpose_4x8_in_4x16_sse4_1(outcoeff, in);
        fadst4x8_col_N4_sse4_1(in, outcoeff, bitrow, 2);
        clear_buffer_4x16_N4(outcoeff);
        break;
    case FLIPADST_ADST:
        load_buffer_4x16_sse4_1(input, in, stride, 1, 0, shift[0]);
        fadst16x4_N4_sse4_1(in, outcoeff, bitcol);
        col_txfm_8x8_N4_rounding(outcoeff, -shift[1]);
        transpose_4x8_in_4x16_sse4_1(outcoeff, in);
        fadst4x8_col_N4_sse4_1(in, outcoeff, bitrow, 2);
        clear_buffer_4x16_N4(outcoeff);
        break;
    case IDTX:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fidtx8xn_N2_col_sse4_1(in, outcoeff, bitcol, 2);
        col_txfm_8x8_N4_rounding(outcoeff, -shift[1]);
        fidtx4x8_col_N4_sse4_1(outcoeff, outcoeff, bitrow, 4);
        clear_buffer_4x16_N4(outcoeff);
        break;
    case V_DCT:
        load_buffer_4x16_sse4_1(input, in, stride, 0, 0, shift[0]);
        fdct16x4_N4_sse4_1(in, outcoeff, bitcol);
        col_txfm_8x8_N4_rounding(outcoeff, -shift[1]);
        fidtx4x8_col_N4_sse4_1(outcoeff, outcoeff, bitrow, 4);
        clear_buffer_4x16_N4(outcoeff);
        break;
    case H_DCT:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fidtx8xn_N2_col_sse4_1(in, outcoeff, bitcol, 2);
        col_txfm_8x8_N4_rounding(outcoeff, -shift[1]);
        transpose_4x8_in_4x16_sse4_1(outcoeff, in);
        fdct4x8_col_N4_sse4_1(in, outcoeff, bitrow, 2);
        clear_buffer_4x16_N4(outcoeff);
        break;
    case V_ADST:
        load_buffer_4x16_sse4_1(input, in, stride, 0, 0, shift[0]);
        fadst16x4_N4_sse4_1(in, outcoeff, bitcol);
        col_txfm_8x8_N4_rounding(outcoeff, -shift[1]);
        fidtx4x8_col_N4_sse4_1(outcoeff, outcoeff, bitrow, 4);
        clear_buffer_4x16_N4(outcoeff);
        break;
    case H_ADST:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fidtx8xn_N2_col_sse4_1(in, outcoeff, bitcol, 2);
        col_txfm_8x8_N4_rounding(outcoeff, -shift[1]);
        transpose_4x8_in_4x16_sse4_1(outcoeff, in);
        fadst4x8_col_N4_sse4_1(in, outcoeff, bitrow, 2);
        clear_buffer_4x16_N4(outcoeff);
        break;
    case V_FLIPADST:
        load_buffer_4x16_sse4_1(input, in, stride, 1, 0, shift[0]);
        fadst16x4_N4_sse4_1(in, outcoeff, bitcol);
        col_txfm_8x8_N4_rounding(outcoeff, -shift[1]);
        fidtx4x8_col_N4_sse4_1(outcoeff, outcoeff, bitrow, 4);
        clear_buffer_4x16_N4(outcoeff);
        break;
    case H_FLIPADST:
        load_buffer_4x4(input, in, stride, 0, 1, shift[0]);
        fidtx8xn_N2_col_sse4_1(in, outcoeff, bitcol, 2);
        col_txfm_8x8_N4_rounding(outcoeff, -shift[1]);
        transpose_4x8_in_4x16_sse4_1(outcoeff, in);
        fadst4x8_col_N4_sse4_1(in, outcoeff, bitrow, 2);
        clear_buffer_4x16_N4(outcoeff);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_16x4_N4_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[16];
    __m128i*      outcoeff = (__m128i*)output;
    const int8_t* shift    = fwd_txfm_shift_ls[TX_16X4];
    const int32_t txw_idx  = get_txw_idx(TX_16X4);
    const int32_t txh_idx  = get_txh_idx(TX_16X4);
    int32_t       bitcol   = fwd_cos_bit_col[txw_idx][txh_idx];
    int32_t       bitrow   = fwd_cos_bit_row[txw_idx][txh_idx];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_16x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fdct4x8_row_N4_with_round_sse4_1(in, outcoeff, bitcol, -shift[1]);
        fdct4x8_row_N4_with_round_sse4_1(in + 8, outcoeff + 8, bitcol, -shift[1]);
        fdct16x4_N4_sse4_1(outcoeff, in, bitrow);
        transpose_4x8_in_4x16_quad_sse4_1(in, outcoeff);
        clear_buffer_wxh_N4(outcoeff, 2, 4);
        break;
    case ADST_DCT:
        load_buffer_16x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fadst4x8_row_N4_with_round_sse4_1(in, outcoeff, bitcol, -shift[1]);
        fadst4x8_row_N4_with_round_sse4_1(in + 8, outcoeff + 8, bitcol, -shift[1]);
        fdct16x4_N4_sse4_1(outcoeff, in, bitrow);
        transpose_4x8_in_4x16_quad_sse4_1(in, outcoeff);
        clear_buffer_wxh_N4(outcoeff, 2, 4);
        break;
    case DCT_ADST:
        load_buffer_16x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fdct4x8_row_N4_with_round_sse4_1(in, outcoeff, bitcol, -shift[1]);
        fdct4x8_row_N4_with_round_sse4_1(in + 8, outcoeff + 8, bitcol, -shift[1]);
        fadst16x4_N4_sse4_1(outcoeff, in, bitrow);
        transpose_4x8_in_4x16_quad_sse4_1(in, outcoeff);
        clear_buffer_wxh_N4(outcoeff, 2, 4);
        break;
    case ADST_ADST:
        load_buffer_16x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fadst4x8_row_N4_with_round_sse4_1(in, outcoeff, bitcol, -shift[1]);
        fadst4x8_row_N4_with_round_sse4_1(in + 8, outcoeff + 8, bitcol, -shift[1]);
        fadst16x4_N4_sse4_1(outcoeff, in, bitrow);
        transpose_4x8_in_4x16_quad_sse4_1(in, outcoeff);
        clear_buffer_wxh_N4(outcoeff, 2, 4);
        break;
    case FLIPADST_DCT:
        load_buffer_16x4_sse4_1(input, in, stride, 1, 0, shift[0]);
        fadst4x8_row_N4_with_round_sse4_1(in, outcoeff, bitcol, -shift[1]);
        fadst4x8_row_N4_with_round_sse4_1(in + 8, outcoeff + 8, bitcol, -shift[1]);
        fdct16x4_N4_sse4_1(outcoeff, in, bitrow);
        transpose_4x8_in_4x16_quad_sse4_1(in, outcoeff);
        clear_buffer_wxh_N4(outcoeff, 2, 4);
        break;
    case DCT_FLIPADST:
        load_buffer_16x4_sse4_1(input, in, stride, 0, 1, shift[0]);
        fdct4x8_row_N4_with_round_sse4_1(in, outcoeff, bitcol, -shift[1]);
        fdct4x8_row_N4_with_round_sse4_1(in + 8, outcoeff + 8, bitcol, -shift[1]);
        fadst16x4_N4_sse4_1(outcoeff, in, bitrow);
        transpose_4x8_in_4x16_quad_sse4_1(in, outcoeff);
        clear_buffer_wxh_N4(outcoeff, 2, 4);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_16x4_sse4_1(input, in, stride, 1, 1, shift[0]);
        fadst4x8_row_N4_with_round_sse4_1(in, outcoeff, bitcol, -shift[1]);
        fadst4x8_row_N4_with_round_sse4_1(in + 8, outcoeff + 8, bitcol, -shift[1]);
        fadst16x4_N4_sse4_1(outcoeff, in, bitrow);
        transpose_4x8_in_4x16_quad_sse4_1(in, outcoeff);
        clear_buffer_wxh_N4(outcoeff, 2, 4);
        break;
    case ADST_FLIPADST:
        load_buffer_16x4_sse4_1(input, in, stride, 0, 1, shift[0]);
        fadst4x8_row_N4_with_round_sse4_1(in, outcoeff, bitcol, -shift[1]);
        fadst4x8_row_N4_with_round_sse4_1(in + 8, outcoeff + 8, bitcol, -shift[1]);
        fadst16x4_N4_sse4_1(outcoeff, in, bitrow);
        transpose_4x8_in_4x16_quad_sse4_1(in, outcoeff);
        clear_buffer_wxh_N4(outcoeff, 2, 4);
        break;
    case FLIPADST_ADST:
        load_buffer_16x4_sse4_1(input, in, stride, 1, 0, shift[0]);
        fadst4x8_row_N4_with_round_sse4_1(in, outcoeff, bitcol, -shift[1]);
        fadst4x8_row_N4_with_round_sse4_1(in + 8, outcoeff + 8, bitcol, -shift[1]);
        fadst16x4_N4_sse4_1(outcoeff, in, bitrow);
        transpose_4x8_in_4x16_quad_sse4_1(in, outcoeff);
        clear_buffer_wxh_N4(outcoeff, 2, 4);
        break;
    case IDTX:
        load_buffer_4x4(input, in, stride, 0, 0, shift[0]);
        fidtx4x8_row_N4_sse4_1(in, outcoeff, bitcol);
        __m128i rounding = _mm_set1_epi32(1 << (-shift[1] - 1));
        outcoeff[0]      = _mm_add_epi32(outcoeff[0], rounding);
        outcoeff[1]      = _mm_add_epi32(outcoeff[1], rounding);
        outcoeff[0]      = _mm_srai_epi32(outcoeff[0], -shift[1]);
        outcoeff[1]      = _mm_srai_epi32(outcoeff[1], -shift[1]);
        fidtx4x8_N2_perm_sse4_1(outcoeff, outcoeff, bitrow);
        clear_buffer_wxh_N4(outcoeff, 2, 4);
        break;
    case V_DCT:
        load_buffer_8x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fdct4x8_row_N4_with_round_sse4_1(in, outcoeff, bitcol, -shift[1]);
        transpose_4x8_in_4x16_quad_sse4_1(outcoeff, in);
        fidtx8xn_N2_col_sse4_1(in, outcoeff, bitrow, 1);
        clear_buffer_wxh_N4(outcoeff, 2, 4);
        break;
    case H_DCT:
        load_buffer_16x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fidtx4x8_row_N4_with_round_sse4_1(in, outcoeff, bitcol, -shift[1]);
        fidtx4x8_row_N4_with_round_sse4_1(in + 8, outcoeff + 8, bitcol, -shift[1]);
        fdct16x4_N4_sse4_1(outcoeff, in, bitrow);
        transpose_4x8_in_4x16_quad_sse4_1(in, outcoeff);
        clear_buffer_wxh_N4(outcoeff, 2, 4);
        break;
    case V_ADST:
        load_buffer_8x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fadst4x8_row_N4_with_round_sse4_1(in, outcoeff, bitcol, -shift[1]);
        transpose_4x8_in_4x16_quad_sse4_1(outcoeff, in);
        fidtx8xn_N2_col_sse4_1(in, outcoeff, bitrow, 1);
        clear_buffer_wxh_N4(outcoeff, 2, 4);
        break;
    case H_ADST:
        load_buffer_16x4_sse4_1(input, in, stride, 0, 0, shift[0]);
        fidtx4x8_row_N4_with_round_sse4_1(in, outcoeff, bitcol, -shift[1]);
        fidtx4x8_row_N4_with_round_sse4_1(in + 8, outcoeff + 8, bitcol, -shift[1]);
        fadst16x4_N4_sse4_1(outcoeff, in, bitrow);
        transpose_4x8_in_4x16_quad_sse4_1(in, outcoeff);
        clear_buffer_wxh_N4(outcoeff, 2, 4);
        break;
    case V_FLIPADST:
        load_buffer_8x4_sse4_1(input, in, stride, 1, 0, shift[0]);
        fadst4x8_row_N4_with_round_sse4_1(in, outcoeff, bitcol, -shift[1]);
        transpose_4x8_in_4x16_quad_sse4_1(outcoeff, in);
        fidtx8xn_N2_col_sse4_1(in, outcoeff, bitrow, 1);
        clear_buffer_wxh_N4(outcoeff, 2, 4);
        break;
    case H_FLIPADST:
        load_buffer_16x4_sse4_1(input, in, stride, 0, 1, shift[0]);
        fidtx4x8_row_N4_with_round_sse4_1(in, outcoeff, bitcol, -shift[1]);
        fidtx4x8_row_N4_with_round_sse4_1(in + 8, outcoeff + 8, bitcol, -shift[1]);
        fadst16x4_N4_sse4_1(outcoeff, in, bitrow);
        transpose_4x8_in_4x16_quad_sse4_1(in, outcoeff);
        clear_buffer_wxh_N4(outcoeff, 2, 4);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_32x64_N4_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)tx_type;
    __m128i       in[512];
    __m128i*      outcoef       = (__m128i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_32X64];
    const int32_t txw_idx       = get_txw_idx(TX_32X64);
    const int32_t txh_idx       = get_txh_idx(TX_32X64);
    const int32_t txfm_size_col = tx_size_wide[TX_32X64];
    const int32_t txfm_size_row = tx_size_high[TX_32X64];
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    load_buffer_32x8n(input, in, stride, 0, 0, shift[0], txfm_size_row);
    av1_fdct64_new_N4_sse4_1(in, outcoef, bitcol, txfm_size_col, num_col);
    col_txfm_16x16_rounding(outcoef, -shift[1]);
    col_txfm_16x16_rounding(outcoef + 2 * txfm_size_col, -shift[1]);
    transpose_8nx8n_N4_half_sse4_1(outcoef, in, txfm_size_col, txfm_size_row);
    av1_fdct32_new_N4_sse4_1(in, outcoef, bitrow, txfm_size_row / 2, num_row);
    transpose_8nx8n_N4_quad_sse4_1(outcoef, in, txfm_size_row, txfm_size_col);
    av1_round_shift_rect_wxh_N4(in, outcoef, -shift[2], new_sqrt2, num_col, txfm_size_row);
    clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
    (void)bd;
}

void svt_av1_fwd_txfm2d_64x32_N4_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)tx_type;
    __m128i       in[512];
    __m128i*      outcoef       = (__m128i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_64X32];
    const int32_t txw_idx       = get_txw_idx(TX_64X32);
    const int32_t txh_idx       = get_txh_idx(TX_64X32);
    const int32_t txfm_size_col = tx_size_wide[TX_64X32];
    const int32_t txfm_size_row = tx_size_high[TX_64X32];
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    for (int32_t i = 0; i < 32; i++) {
        load_buffer_32_sse4_1(input + 0 + i * stride, in + 0 + i * 16, 8, 0, 0, shift[0]);
        load_buffer_32_sse4_1(input + 32 + i * stride, in + 8 + i * 16, 8, 0, 0, shift[0]);
    }
    av1_fdct32_new_N4_sse4_1(in, outcoef, bitcol, txfm_size_col, num_col);
    col_txfm_16x16_rounding(outcoef, -shift[1]);
    col_txfm_16x16_rounding(outcoef + 2 * txfm_size_row, -shift[1]);
    transpose_8nx8n_N4_half_sse4_1(outcoef, in, txfm_size_col, txfm_size_row);
    av1_fdct64_new_N4_sse4_1(in, outcoef, bitrow, txfm_size_row / 2, num_row);
    transpose_8nx8n_N4_quad_sse4_1(outcoef, in, txfm_size_row, txfm_size_col);
    av1_round_shift_rect_wxh_N4(in, outcoef, -shift[2], new_sqrt2, num_col, txfm_size_row);
    clear_buffer_wxh_N4(outcoef, num_col, txfm_size_row);
    (void)bd;
}

void svt_av1_fwd_txfm2d_16x64_N4_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[256];
    __m128i*      outcoeff      = (__m128i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_16X64];
    const int32_t txw_idx       = get_txw_idx(TX_16X64);
    const int32_t txh_idx       = get_txh_idx(TX_16X64);
    const int32_t txfm_size_col = tx_size_wide[TX_16X64];
    const int32_t txfm_size_row = tx_size_high[TX_16X64];
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    int32_t       ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    const int32_t num_row = txfm_size_row >> 3;
    const int32_t num_col = txfm_size_col >> 3;
    // col tranform
    for (int32_t i = 0; i < txfm_size_row; i += num_col) {
        load_buffer_16_sse4_1(input + (i + 0) * stride, in + 2 * (i + 0) * num_col, 8, ud_flip, lr_flip, shift[0]);
        load_buffer_16_sse4_1(input + (i + 1) * stride, in + 2 * (i + 1) * num_col, 8, ud_flip, lr_flip, shift[0]);
    }
    av1_fdct64_new_N4_sse4_1(in, outcoeff, bitcol, txfm_size_col, num_col);
    col_txfm_16x16_rounding(outcoeff, -shift[1]);
    transpose_8nx8n_N4_half_sse4_1(outcoeff, in, txfm_size_col, txfm_size_row);
    // row tranform
    fdct16x16_N4_sse4_1(in, in, bitrow, 2 * num_row, num_row / 2);
    transpose_8nx8n_N4_quad_sse4_1(in, outcoeff, txfm_size_row, txfm_size_col);
    clear_buffer_wxh_N4(outcoeff, num_col, txfm_size_row);
    (void)bd;
}

void svt_av1_fwd_txfm2d_64x16_N4_sse4_1(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m128i       in[256];
    __m128i*      outcoeff      = (__m128i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_64X16];
    const int32_t txw_idx       = get_txw_idx(TX_64X16);
    const int32_t txh_idx       = get_txh_idx(TX_64X16);
    const int32_t txfm_size_col = tx_size_wide[TX_64X16];
    const int32_t txfm_size_row = tx_size_high[TX_64X16];
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    int32_t       ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    const int32_t num_row = txfm_size_row >> 3;
    const int32_t num_col = txfm_size_col >> 3;
    // col tranform
    for (int32_t i = 0; i < txfm_size_row; i++) {
        load_buffer_16_sse4_1(input + 0 + i * stride, in + 0 + i * 16, 8, ud_flip, lr_flip, shift[0]);
        load_buffer_16_sse4_1(input + 16 + i * stride, in + 4 + i * 16, 8, ud_flip, lr_flip, shift[0]);
        load_buffer_16_sse4_1(input + 32 + i * stride, in + 8 + i * 16, 8, ud_flip, lr_flip, shift[0]);
        load_buffer_16_sse4_1(input + 48 + i * stride, in + 12 + i * 16, 8, ud_flip, lr_flip, shift[0]);
    }

    fdct16x16_N4_sse4_1(in, outcoeff, bitcol, 2 * num_col, 2 * num_col);
    col_txfm_16x16_rounding(outcoeff, -shift[1]);
    transpose_8nx8n_N4_half_sse4_1(outcoeff, in, txfm_size_col, txfm_size_row);
    // row tranform
    av1_fdct64_new_N4_sse4_1(in, in, bitrow, txfm_size_row / 2, num_row);
    transpose_8nx8n_N4_quad_sse4_1(in, outcoeff, txfm_size_row, txfm_size_col);
    clear_buffer_wxh_N4(outcoeff, num_col, txfm_size_row);
    (void)bd;
}

static inline void transpose_32bit_4x4(const __m128i* const in, __m128i* const out) {
    // Unpack 32 bit elements. Goes from:
    // in[0]: 00 01 02 03
    // in[1]: 10 11 12 13
    // in[2]: 20 21 22 23
    // in[3]: 30 31 32 33
    // to:
    // a0:    00 10 01 11
    // a1:    20 30 21 31
    // a2:    02 12 03 13
    // a3:    22 32 23 33

    const __m128i a0 = _mm_unpacklo_epi32(in[0], in[1]);
    const __m128i a1 = _mm_unpacklo_epi32(in[2], in[3]);
    const __m128i a2 = _mm_unpackhi_epi32(in[0], in[1]);
    const __m128i a3 = _mm_unpackhi_epi32(in[2], in[3]);

    // Unpack 64 bit elements resulting in:
    // out[0]: 00 10 20 30
    // out[1]: 01 11 21 31
    // out[2]: 02 12 22 32
    // out[3]: 03 13 23 33
    out[0] = _mm_unpacklo_epi64(a0, a1);
    out[1] = _mm_unpackhi_epi64(a0, a1);
    out[2] = _mm_unpacklo_epi64(a2, a3);
    out[3] = _mm_unpackhi_epi64(a2, a3);
}

void svt_av1_fwht4x4_sse4_1(int16_t* input, int32_t* output, uint32_t stride) {
    __m128i in[4];
    in[0] = _mm_loadl_epi64((const __m128i*)(input + 0 * stride));
    in[1] = _mm_loadl_epi64((const __m128i*)(input + 1 * stride));
    in[2] = _mm_loadl_epi64((const __m128i*)(input + 2 * stride));
    in[3] = _mm_loadl_epi64((const __m128i*)(input + 3 * stride));

    // Convert to int32_t.
    __m128i op[4];
    op[0] = _mm_cvtepi16_epi32(in[0]);
    op[1] = _mm_cvtepi16_epi32(in[1]);
    op[2] = _mm_cvtepi16_epi32(in[2]);
    op[3] = _mm_cvtepi16_epi32(in[3]);

    for (int i = 0; i < 2; ++i) {
        __m128i a1 = op[0];
        __m128i b1 = op[1];
        __m128i c1 = op[2];
        __m128i d1 = op[3];
        __m128i e1;

        a1 = _mm_add_epi32(a1, b1); // a1 += b1
        d1 = _mm_sub_epi32(d1, c1); // d1 = d1 - c1
        e1 = _mm_sub_epi32(a1, d1); // e1 = (a1 - d1) >> 1
        e1 = _mm_srai_epi32(e1, 1);
        b1 = _mm_sub_epi32(e1, b1); // b1 = e1 - b1
        c1 = _mm_sub_epi32(e1, c1); // c1 = e1 - c1
        a1 = _mm_sub_epi32(a1, c1); // a1 -= c1
        d1 = _mm_add_epi32(d1, b1); // d1 += b1

        op[0] = a1;
        op[1] = c1;
        op[2] = d1;
        op[3] = b1;

        if (i == 0) {
            transpose_32bit_4x4(op, op);
        }
    }

    op[0] = _mm_slli_epi32(op[0], UNIT_QUANT_SHIFT);
    op[1] = _mm_slli_epi32(op[1], UNIT_QUANT_SHIFT);
    op[2] = _mm_slli_epi32(op[2], UNIT_QUANT_SHIFT);
    op[3] = _mm_slli_epi32(op[3], UNIT_QUANT_SHIFT);

    _mm_storeu_si128((__m128i*)(output + 0), op[0]);
    _mm_storeu_si128((__m128i*)(output + 4), op[1]);
    _mm_storeu_si128((__m128i*)(output + 8), op[2]);
    _mm_storeu_si128((__m128i*)(output + 12), op[3]);
}
