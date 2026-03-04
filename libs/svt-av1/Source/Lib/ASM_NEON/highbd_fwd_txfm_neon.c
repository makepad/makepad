/*
 * Copyright (c) 2024, Intel Corporation
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
#include "aom_dsp_rtcd.h"
#include "mem_neon.h"
#include "inv_transforms.h"
#include "transforms.h"
#include "transpose_neon.h"
#include "definitions.h"

// Constants are stored in pairs, where symmetrical constants in the
// cospi array are stored adjacent in memory, i.e.:
//   f(i,j) = (int)round(cos(PI*j/128) * (1<<(cos_bit_min+i)))
// and then in memory we store 4-tuples of constants together as:
//   f2(i,j) = [ f(i,j), f(i,64-j) ]
static const int32_t av1_cospi_arr_s32_data[4][66] = {
    {
        1024, 0,   1024, 25,  1023, 50,  1021, 75,  1019, 100, 1016, 125, 1013, 150, 1009, 175, 1004,
        200,  999, 224,  993, 249,  987, 273,  980, 297,  972, 321,  964, 345,  955, 369,  946, 392,
        936,  415, 926,  438, 915,  460, 903,  483, 891,  505, 878,  526, 865,  548, 851,  569, 837,
        590,  822, 610,  807, 630,  792, 650,  775, 669,  759, 688,  742, 706,  724, 724,
    },
    {
        2048, 0,    2047, 50,   2046, 100,  2042, 151,  2038, 201,  2033, 251,  2026, 301,  2018, 350,  2009,
        400,  1998, 449,  1987, 498,  1974, 546,  1960, 595,  1945, 642,  1928, 690,  1911, 737,  1892, 784,
        1872, 830,  1851, 876,  1829, 921,  1806, 965,  1782, 1009, 1757, 1053, 1730, 1096, 1703, 1138, 1674,
        1179, 1645, 1220, 1615, 1260, 1583, 1299, 1551, 1338, 1517, 1375, 1483, 1412, 1448, 1448,
    },
    {
        4096, 0,    4095, 101,  4091, 201,  4085, 301,  4076, 401,  4065, 501,  4052, 601,  4036, 700,  4017,
        799,  3996, 897,  3973, 995,  3948, 1092, 3920, 1189, 3889, 1285, 3857, 1380, 3822, 1474, 3784, 1567,
        3745, 1660, 3703, 1751, 3659, 1842, 3612, 1931, 3564, 2019, 3513, 2106, 3461, 2191, 3406, 2276, 3349,
        2359, 3290, 2440, 3229, 2520, 3166, 2598, 3102, 2675, 3035, 2751, 2967, 2824, 2896, 2896,
    },
    {
        8192, 0,    8190, 201,  8182, 402,  8170, 603,  8153, 803,  8130, 1003, 8103, 1202, 8071, 1401, 8035,
        1598, 7993, 1795, 7946, 1990, 7895, 2185, 7839, 2378, 7779, 2570, 7713, 2760, 7643, 2948, 7568, 3135,
        7489, 3320, 7405, 3503, 7317, 3683, 7225, 3862, 7128, 4038, 7027, 4212, 6921, 4383, 6811, 4551, 6698,
        4717, 6580, 4880, 6458, 5040, 6333, 5197, 6203, 5351, 6070, 5501, 5933, 5649, 5793, 5793,
    }};

static inline const int32_t* cospi_arr_s32(int n) {
    return av1_cospi_arr_s32_data[n - cos_bit_min];
}

static inline void ud_adjust_input_and_stride(int ud_flip, int16_t** input, uint32_t* stride, int out_size) {
    if (ud_flip) {
        *input  = *input + (out_size - 1) * *stride;
        *stride = -*stride;
    }
}

#define LOAD_BUFFER_4XH(h, shift)                                                                                 \
    static AOM_FORCE_INLINE void load_buffer_4x##h(const int16_t* input, int32x4_t* in, int stride, int fliplr) { \
        if (fliplr) {                                                                                             \
            for (int i = 0; i < (h); ++i) {                                                                       \
                int16x4_t a = vld1_s16(input + i * stride);                                                       \
                a           = vrev64_s16(a);                                                                      \
                in[i]       = vshll_n_s16(a, shift);                                                              \
            }                                                                                                     \
        } else {                                                                                                  \
            for (int i = 0; i < (h); ++i) {                                                                       \
                int16x4_t a = vld1_s16(input + i * stride);                                                       \
                in[i]       = vshll_n_s16(a, shift);                                                              \
            }                                                                                                     \
        }                                                                                                         \
    }

LOAD_BUFFER_4XH(4, 2)
LOAD_BUFFER_4XH(8, 2)
LOAD_BUFFER_4XH(16, 2)
LOAD_BUFFER_4XH(32, 2)
LOAD_BUFFER_4XH(64, 0)

#define LOAD_BUFFER_WXH(w, h, shift)                                                                                  \
    static AOM_FORCE_INLINE void load_buffer_##w##x##h(const int16_t* input, int32x4_t* in, int stride, int fliplr) { \
        assert(w >= 8);                                                                                               \
        if (fliplr) {                                                                                                 \
            for (int i = 0; i < (h); ++i) {                                                                           \
                for (int j = 0; j < (w) / 8; ++j) {                                                                   \
                    int16x8_t a                = vld1q_s16(input + i * stride + j * 8);                               \
                    a                          = vrev64q_s16(a);                                                      \
                    int j2                     = (w) / 8 - j - 1;                                                     \
                    in[i + (h) * (2 * j2 + 0)] = vshll_n_s16(vget_high_s16(a), shift);                                \
                    in[i + (h) * (2 * j2 + 1)] = vshll_n_s16(vget_low_s16(a), shift);                                 \
                }                                                                                                     \
            }                                                                                                         \
        } else {                                                                                                      \
            for (int i = 0; i < (h); ++i) {                                                                           \
                for (int j = 0; j < (w) / 8; ++j) {                                                                   \
                    int16x8_t a               = vld1q_s16(input + i * stride + j * 8);                                \
                    in[i + (h) * (2 * j + 0)] = vshll_n_s16(vget_low_s16(a), shift);                                  \
                    in[i + (h) * (2 * j + 1)] = vshll_n_s16(vget_high_s16(a), shift);                                 \
                }                                                                                                     \
            }                                                                                                         \
        }                                                                                                             \
    }

LOAD_BUFFER_WXH(8, 8, 2)
LOAD_BUFFER_WXH(8, 32, 2)
LOAD_BUFFER_WXH(16, 16, 2)
LOAD_BUFFER_WXH(16, 32, 2)
LOAD_BUFFER_WXH(16, 64, 0)
LOAD_BUFFER_WXH(32, 32, 2)
LOAD_BUFFER_WXH(32, 64, 0)
LOAD_BUFFER_WXH(64, 16, 2)
LOAD_BUFFER_WXH(64, 32, 2)
LOAD_BUFFER_WXH(64, 64, 0)

#define STORE_BUFFER_WXH(w, h)                                                                           \
    static AOM_FORCE_INLINE void store_buffer_##w##x##h(const int32x4_t* in, int32_t* out, int stride) { \
        for (int i = 0; i < (w); ++i) {                                                                  \
            for (int j = 0; j < (h) / 4; ++j) {                                                          \
                vst1q_s32(&out[i * stride + j * 4], in[i + j * (w)]);                                    \
            }                                                                                            \
        }                                                                                                \
    }

STORE_BUFFER_WXH(4, 4)
STORE_BUFFER_WXH(8, 8)
STORE_BUFFER_WXH(16, 16)

static AOM_FORCE_INLINE void highbd_fdct4_x4_neon(const int32x4_t* in, int32x4_t* out, int bit) {
    const int32_t* const cospi      = cospi_arr_s32(bit);
    const int32x4_t      cospi32    = vdupq_n_s32(cospi[2 * 32]);
    const int32x2_t      cospi16_48 = vld1_s32(&cospi[2 * 16]);

    const int32x4_t a0 = vaddq_s32(in[0], in[3]);
    const int32x4_t a1 = vsubq_s32(in[0], in[3]);
    const int32x4_t a2 = vaddq_s32(in[1], in[2]);
    const int32x4_t a3 = vsubq_s32(in[1], in[2]);

    const int32x4_t b0 = vmulq_s32(a0, cospi32);
    const int32x4_t b1 = vmulq_lane_s32(a1, cospi16_48, 1);
    const int32x4_t b2 = vmulq_s32(a2, cospi32);
    const int32x4_t b3 = vmulq_lane_s32(a3, cospi16_48, 1);

    const int32x4_t c0 = vaddq_s32(b0, b2);
    const int32x4_t c1 = vsubq_s32(b0, b2);
    const int32x4_t c2 = vmlaq_lane_s32(b3, a1, cospi16_48, 0);
    const int32x4_t c3 = vmlsq_lane_s32(b1, a3, cospi16_48, 0);

    const int32x4_t v_bit = vdupq_n_s32(-bit);
    const int32x4_t d0    = vrshlq_s32(c0, v_bit);
    const int32x4_t d1    = vrshlq_s32(c1, v_bit);
    const int32x4_t d2    = vrshlq_s32(c2, v_bit);
    const int32x4_t d3    = vrshlq_s32(c3, v_bit);

    out[0] = d0;
    out[1] = d2;
    out[2] = d1;
    out[3] = d3;
}

static AOM_FORCE_INLINE void highbd_fadst4_x4_neon(const int32x4_t* in, int32x4_t* out, int bit) {
    const int32x4_t sinpi = vld1q_s32(sinpi_arr(bit) + 1);

    const int32x4_t a0 = vaddq_s32(in[0], in[1]);
    const int32x4_t a1 = vmulq_lane_s32(in[0], vget_low_s32(sinpi), 0);
    const int32x4_t a2 = vmulq_lane_s32(in[0], vget_high_s32(sinpi), 1);
    const int32x4_t a3 = vmulq_lane_s32(in[2], vget_high_s32(sinpi), 0);

    const int32x4_t b0 = vmlaq_lane_s32(a1, in[1], vget_low_s32(sinpi), 1);
    const int32x4_t b1 = vmlsq_lane_s32(a2, in[1], vget_low_s32(sinpi), 0);
    const int32x4_t b2 = vsubq_s32(a0, in[3]);

    const int32x4_t c0 = vmlaq_lane_s32(b0, in[3], vget_high_s32(sinpi), 1);
    const int32x4_t c1 = vmlaq_lane_s32(b1, in[3], vget_low_s32(sinpi), 1);
    const int32x4_t c2 = vmulq_lane_s32(b2, vget_high_s32(sinpi), 0);

    const int32x4_t d0 = vaddq_s32(c0, a3);
    const int32x4_t d1 = vsubq_s32(c1, a3);
    const int32x4_t d2 = vsubq_s32(c1, c0);

    const int32x4_t e0 = vaddq_s32(d2, a3);

    const int32x4_t v_bit = vdupq_n_s32(-bit);
    out[0]                = vrshlq_s32(d0, v_bit);
    out[1]                = vrshlq_s32(c2, v_bit);
    out[2]                = vrshlq_s32(d1, v_bit);
    out[3]                = vrshlq_s32(e0, v_bit);
}

static AOM_FORCE_INLINE void highbd_fidentity4_x4_neon(const int32x4_t* in, int32x4_t* out, int bit) {
    (void)bit;
    int32x4_t fact = vdupq_n_s32(new_sqrt2);

    for (int i = 0; i < 4; i++) {
        const int32x4_t a_low = vmulq_s32(in[i], fact);
        out[i]                = vrshrq_n_s32(a_low, new_sqrt2_bits);
    }
}

void svt_av1_fwd_txfm2d_4x4_neon(int16_t* input, int32_t* output, uint32_t input_stride, TxType tx_type, uint8_t bd) {
    (void)bd;

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &input_stride, 4);

    // Workspace for column/row-wise transforms.
    int32x4_t buf[4];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fdct4_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fdct4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        store_buffer_4x4(buf, output, /*stride=*/4);
        break;
    case ADST_DCT:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fadst4_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fdct4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        store_buffer_4x4(buf, output, /*stride=*/4);
        break;
    case DCT_ADST:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fdct4_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fadst4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        store_buffer_4x4(buf, output, /*stride=*/4);
        break;
    case ADST_ADST:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fadst4_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fadst4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        store_buffer_4x4(buf, output, /*stride=*/4);
        break;
    case FLIPADST_DCT:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fadst4_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fdct4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        store_buffer_4x4(buf, output, /*stride=*/4);
        break;
    case DCT_FLIPADST:
        load_buffer_4x4(input, buf, input_stride, 1);
        highbd_fdct4_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fadst4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        store_buffer_4x4(buf, output, /*stride=*/4);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_4x4(input, buf, input_stride, 1);
        highbd_fadst4_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fadst4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        store_buffer_4x4(buf, output, /*stride=*/4);
        break;
    case ADST_FLIPADST:
        load_buffer_4x4(input, buf, input_stride, 1);
        highbd_fadst4_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fadst4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        store_buffer_4x4(buf, output, /*stride=*/4);
        break;
    case FLIPADST_ADST:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fadst4_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fadst4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        store_buffer_4x4(buf, output, /*stride=*/4);
        break;
    case IDTX:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fidentity4_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        highbd_fidentity4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        store_buffer_4x4(buf, output, /*stride=*/4);
        break;
    case V_DCT:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fdct4_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fidentity4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        store_buffer_4x4(buf, output, /*stride=*/4);
        break;
    case H_DCT:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fidentity4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fdct4_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        store_buffer_4x4(buf, output, /*stride=*/4);
        break;
    case V_ADST:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fadst4_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fidentity4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        store_buffer_4x4(buf, output, /*stride=*/4);
        break;
    case H_ADST:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fidentity4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fadst4_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        store_buffer_4x4(buf, output, /*stride=*/4);
        break;
    case V_FLIPADST:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fadst4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fidentity4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        store_buffer_4x4(buf, output, /*stride=*/4);
        break;
    case H_FLIPADST:
        load_buffer_4x4(input, buf, input_stride, 1);
        highbd_fidentity4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fadst4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        store_buffer_4x4(buf, output, /*stride=*/4);
        break;
    default:
        assert(0);
    }
}

#define SHIFT_LOOP_HELPER(name, type, intrinsic, arg)              \
    static inline void name(const type* in, type* out, int size) { \
        int i = 0;                                                 \
        do {                                                       \
            out[i] = intrinsic(in[i], arg);                        \
        } while (++i < size);                                      \
    }

SHIFT_LOOP_HELPER(shift_right_2_round_s32_x4, int32x4_t, vrshrq_n_s32, 2)
SHIFT_LOOP_HELPER(shift_right_4_round_s32_x4, int32x4_t, vrshrq_n_s32, 4)

// Addition instructions have slightly better performance compared to shift
// instructions on some micro-architectures, so use these for shifts by one.

SHIFT_LOOP_HELPER(shift_right_1_round_s32_x4, int32x4_t, vrhaddq_s32, vdupq_n_s32(0))

// A note on butterfly helper naming:
//
// butterfly_[weight_indices]_neon
// e.g. butterfly_0312_neon
//                ^ Weights are applied as indices 0, 3, 2, 1
//                  (see more detail below)
//
// Weight indices are treated as an index into the 4-tuple of the weight
// itself, plus related and negated constants: w=(w0, 1-w0, -w0, w0-1).
// This is then represented in the helper naming by referring to the lane index
// in the loaded tuple that each multiply is performed with:
//
//         in0   in1
//      /------------
// out0 |  w[0]  w[1]   ==>  out0 = in0 * w[0] + in1 * w[1]
// out1 |  w[2]  w[3]   ==>  out1 = in0 * w[2] + in1 * w[3]
//
// So for indices 0321 from the earlier example, we end up with:
//
//          in0       in1
//      /------------------
// out0 | (lane 0) (lane 3)   ==>  out0 = in0 *  w0 + in1 * (w0-1)
// out1 | (lane 2) (lane 1)   ==>  out1 = in0 * -w0 + in1 * (1-w0)

#define butterfly_half_neon(wvec, lane0, lane1, in0, in1, out, v_bit)                \
    do {                                                                             \
        int32x2x2_t wvecs = {{wvec, vneg_s32(wvec)}};                                \
        int32x4_t   x     = vmulq_lane_s32(in0, wvecs.val[lane0 / 2], lane0 % 2);    \
        x                 = vmlaq_lane_s32(x, in1, wvecs.val[lane1 / 2], lane1 % 2); \
        *out              = vrshlq_s32(x, v_bit);                                    \
    } while (false)

static AOM_FORCE_INLINE void butterfly_0112_neon(const int32_t* cospi, const int widx0, const int32x4_t n0,
                                                 const int32x4_t n1, int32x4_t* out0, int32x4_t* out1,
                                                 const int32x4_t v_bit) {
    int32x2_t w01 = vld1_s32(cospi + 2 * widx0);
    butterfly_half_neon(w01, 0, 1, n0, n1, out0, v_bit);
    butterfly_half_neon(w01, 1, 2, n0, n1, out1, v_bit);
}

static AOM_FORCE_INLINE void butterfly_2312_neon(const int32_t* cospi, const int widx0, const int32x4_t n0,
                                                 const int32x4_t n1, int32x4_t* out0, int32x4_t* out1,
                                                 const int32x4_t v_bit) {
    int32x2_t w01 = vld1_s32(cospi + 2 * widx0);
    butterfly_half_neon(w01, 2, 3, n0, n1, out0, v_bit);
    butterfly_half_neon(w01, 1, 2, n0, n1, out1, v_bit);
}

static AOM_FORCE_INLINE void butterfly_0332_neon(const int32_t* cospi, const int widx0, const int32x4_t n0,
                                                 const int32x4_t n1, int32x4_t* out0, int32x4_t* out1,
                                                 const int32x4_t v_bit) {
    int32x2_t w01 = vld1_s32(cospi + 2 * widx0);
    butterfly_half_neon(w01, 0, 3, n0, n1, out0, v_bit);
    butterfly_half_neon(w01, 3, 2, n0, n1, out1, v_bit);
}

static AOM_FORCE_INLINE void butterfly_0130_neon(const int32_t* cospi, const int widx0, const int32x4_t n0,
                                                 const int32x4_t n1, int32x4_t* out0, int32x4_t* out1,
                                                 const int32x4_t v_bit) {
    int32x2_t w01 = vld1_s32(cospi + 2 * widx0);
    butterfly_half_neon(w01, 0, 1, n0, n1, out0, v_bit);
    butterfly_half_neon(w01, 3, 0, n0, n1, out1, v_bit);
}

static AOM_FORCE_INLINE void butterfly_cospi32_0002_neon(const int32_t* cospi, const int32x4_t n0, const int32x4_t n1,
                                                         int32x4_t* out0, int32x4_t* out1, const int32x4_t v_bit) {
    int32x2_t w01 = vld1_s32(cospi + 2 * 32);
    butterfly_half_neon(w01, 0, 0, n0, n1, out0, v_bit);
    butterfly_half_neon(w01, 0, 2, n0, n1, out1, v_bit);
}

static AOM_FORCE_INLINE void butterfly_cospi32_0222_neon(const int32_t* cospi, const int32x4_t n0, const int32x4_t n1,
                                                         int32x4_t* out0, int32x4_t* out1, const int32x4_t v_bit) {
    int32x2_t w01 = vld1_s32(cospi + 2 * 32);
    butterfly_half_neon(w01, 0, 2, n0, n1, out0, v_bit);
    butterfly_half_neon(w01, 2, 2, n0, n1, out1, v_bit);
}

// Butterfly pre-processing:
// e.g. n=4:
//   out[0] = in[0] + in[3]
//   out[1] = in[1] + in[2]
//   out[2] = in[1] - in[2]
//   out[3] = in[0] - in[3]

static AOM_FORCE_INLINE void butterfly_dct_pre(const int32x4_t* input, int32x4_t* output, int n) {
    for (int i = 0; i < n / 2; ++i) {
        output[i] = vaddq_s32(input[i], input[n - i - 1]);
    }
    for (int i = 0; i < n / 2; ++i) {
        output[n / 2 + i] = vsubq_s32(input[n / 2 - i - 1], input[n / 2 + i]);
    }
}

// Butterfly post-processing:
// e.g. n=8:
//   out[0] = in0[0] + in1[3];
//   out[1] = in0[1] + in1[2];
//   out[2] = in0[1] - in1[2];
//   out[3] = in0[0] - in1[3];
//   out[4] = in0[7] - in1[4];
//   out[5] = in0[6] - in1[5];
//   out[6] = in0[6] + in1[5];
//   out[7] = in0[7] + in1[4];

static AOM_FORCE_INLINE void butterfly_dct_post(const int32x4_t* in0, const int32x4_t* in1, int32x4_t* output, int n) {
    for (int i = 0; i < n / 4; ++i) {
        output[i] = vaddq_s32(in0[i], in1[n / 2 - i - 1]);
    }
    for (int i = 0; i < n / 4; ++i) {
        output[n / 4 + i] = vsubq_s32(in0[n / 4 - i - 1], in1[n / 4 + i]);
    }
    for (int i = 0; i < n / 4; ++i) {
        output[n / 2 + i] = vsubq_s32(in0[n - i - 1], in1[n / 2 + i]);
    }
    for (int i = 0; i < n / 4; ++i) {
        output[(3 * n) / 4 + i] = vaddq_s32(in0[(3 * n) / 4 + i], in1[(3 * n) / 4 - i - 1]);
    }
}

static AOM_FORCE_INLINE void highbd_fdct8_x4_neon(const int32x4_t* in, int32x4_t* out, int bit) {
    const int32_t* const cospi = cospi_arr_s32(bit);
    const int32x4_t      v_bit = vdupq_n_s32(-bit);

    // stage 1
    int32x4_t a[8];
    butterfly_dct_pre(in, a, 8);

    // stage 2
    int32x4_t b[8];
    butterfly_dct_pre(a, b, 4);
    butterfly_0130_neon(cospi, 32, a[5], a[6], &b[6], &b[5], v_bit);

    // stage 3
    int32x4_t c[8];
    butterfly_0130_neon(cospi, 32, b[1], b[0], &c[0], &c[1], v_bit);
    butterfly_0112_neon(cospi, 16, b[3], b[2], &c[2], &c[3], v_bit);
    butterfly_dct_post(a + 4, b + 4, c + 4, 4);

    // stage 4-5
    butterfly_0112_neon(cospi, 8, c[7], c[4], &out[1], &out[7], v_bit);
    butterfly_0130_neon(cospi, 24, c[5], c[6], &out[5], &out[3], v_bit);

    out[0] = c[0];
    out[2] = c[2];
    out[4] = c[1];
    out[6] = c[3];
}

static AOM_FORCE_INLINE void highbd_fadst8_x4_neon(const int32x4_t* in, int32x4_t* out, int bit) {
    const int32_t* const cospi = cospi_arr_s32(bit);
    const int32x4_t      v_bit = vdupq_n_s32(-bit);

    int32x4_t u0, u1, u2, u3, u4, u5, u6, u7;
    int32x4_t v0, v1, v2, v3, v4, v5, v6, v7;

    // stage 0-1
    u0 = in[0];
    u1 = in[7];
    u2 = in[3];
    u3 = in[4];
    u4 = in[1];
    u5 = in[6];
    u6 = in[2];
    u7 = in[5];

    // stage 2
    v0 = u0;
    v1 = u1;
    butterfly_cospi32_0222_neon(cospi, u3, u2, &v2, &v3, v_bit);
    v4 = u4;
    v5 = u5;
    butterfly_cospi32_0002_neon(cospi, u6, u7, &v7, &v6, v_bit);

    // stage 3
    u0 = vaddq_s32(v0, v2);
    u1 = vsubq_s32(v3, v1);
    u2 = vsubq_s32(v0, v2);
    u3 = vaddq_s32(v1, v3);
    u4 = vsubq_s32(v6, v4);
    u5 = vaddq_s32(v5, v7);
    u6 = vaddq_s32(v4, v6);
    u7 = vsubq_s32(v5, v7);

    // stage 4
    v0 = u0;
    v1 = u1;
    v2 = u2;
    v3 = u3;

    butterfly_0112_neon(cospi, 16, u4, u5, &v4, &v5, v_bit);
    butterfly_0112_neon(cospi, 16, u7, u6, &v6, &v7, v_bit);

    // stage 5
    u0 = vaddq_s32(v0, v4);
    u1 = vaddq_s32(v1, v5);
    u2 = vaddq_s32(v2, v6);
    u3 = vsubq_s32(v7, v3);
    u4 = vsubq_s32(v0, v4);
    u5 = vsubq_s32(v1, v5);
    u6 = vsubq_s32(v2, v6);
    u7 = vaddq_s32(v3, v7);

    // stage 6
    butterfly_0112_neon(cospi, 4, u0, u1, &v0, &v1, v_bit);
    butterfly_0112_neon(cospi, 20, u2, u3, &v2, &v3, v_bit);
    butterfly_0130_neon(cospi, 28, u5, u4, &v4, &v5, v_bit);
    butterfly_0112_neon(cospi, 12, u6, u7, &v7, &v6, v_bit);

    // stage 7
    out[0] = v1;
    out[1] = v6;
    out[2] = v3;
    out[3] = v4;
    out[4] = v5;
    out[5] = v2;
    out[6] = v7;
    out[7] = v0;
}

static AOM_FORCE_INLINE void highbd_fidentity8_x4_neon(const int32x4_t* in, int32x4_t* out, int bit) {
    (void)bit;
    out[0] = vshlq_n_s32(in[0], 1);
    out[1] = vshlq_n_s32(in[1], 1);
    out[2] = vshlq_n_s32(in[2], 1);
    out[3] = vshlq_n_s32(in[3], 1);
    out[4] = vshlq_n_s32(in[4], 1);
    out[5] = vshlq_n_s32(in[5], 1);
    out[6] = vshlq_n_s32(in[6], 1);
    out[7] = vshlq_n_s32(in[7], 1);
}

static AOM_FORCE_INLINE void highbd_fdct8_xn_neon(const int32x4_t* in, int32x4_t* out, int bit, int howmany) {
    const int stride = 8;
    int       i      = 0;
    do {
        highbd_fdct8_x4_neon(in + i * stride, out + i * stride, bit);
    } while (++i < howmany);
}

static AOM_FORCE_INLINE void highbd_fadst8_xn_neon(const int32x4_t* in, int32x4_t* out, int bit, int howmany) {
    const int stride = 8;
    int       i      = 0;
    do {
        highbd_fadst8_x4_neon(in + i * stride, out + i * stride, bit);
    } while (++i < howmany);
}

static AOM_FORCE_INLINE void highbd_fidentity8_xn_neon(const int32x4_t* in, int32x4_t* out, int bit, int howmany) {
    (void)bit;
    const int stride = 8;
    int       i      = 0;
    do {
        highbd_fidentity8_x4_neon(in + i * stride, out + i * stride, bit);
    } while (++i < howmany);
}

void svt_av1_fwd_txfm2d_8x8_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 8);

    // Workspaces for column/row-wise transforms.
    int32x4_t buf0[16], buf1[16];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_8x8(input, buf0, stride, 0);
        highbd_fdct8_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_x4(buf0, buf0, 16);
        transpose_arrays_s32_8x8(buf0, buf1);
        highbd_fdct8_xn_neon(buf1, buf1, fwd_cos_bit_row[1][1], 2);
        transpose_arrays_s32_8x8(buf1, buf0);
        store_buffer_8x8(buf0, output, /*stride=*/8);
        break;
    case ADST_DCT:
        load_buffer_8x8(input, buf0, stride, 0);
        highbd_fadst8_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_x4(buf0, buf0, 16);
        transpose_arrays_s32_8x8(buf0, buf1);
        highbd_fdct8_xn_neon(buf1, buf1, fwd_cos_bit_row[1][1], 2);
        transpose_arrays_s32_8x8(buf1, buf0);
        store_buffer_8x8(buf0, output, /*stride=*/8);
        break;
    case DCT_ADST:
        load_buffer_8x8(input, buf0, stride, 0);
        highbd_fdct8_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_x4(buf0, buf0, 16);
        transpose_arrays_s32_8x8(buf0, buf1);
        highbd_fadst8_xn_neon(buf1, buf1, fwd_cos_bit_row[1][1], 2);
        transpose_arrays_s32_8x8(buf1, buf0);
        store_buffer_8x8(buf0, output, /*stride=*/8);
        break;
    case ADST_ADST:
        load_buffer_8x8(input, buf0, stride, 0);
        highbd_fadst8_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_x4(buf0, buf0, 16);
        transpose_arrays_s32_8x8(buf0, buf1);
        highbd_fadst8_xn_neon(buf1, buf1, fwd_cos_bit_row[1][1], 2);
        transpose_arrays_s32_8x8(buf1, buf0);
        store_buffer_8x8(buf0, output, /*stride=*/8);
        break;
    case FLIPADST_DCT:
        load_buffer_8x8(input, buf0, stride, 0);
        highbd_fadst8_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_x4(buf0, buf0, 16);
        transpose_arrays_s32_8x8(buf0, buf1);
        highbd_fdct8_xn_neon(buf1, buf1, fwd_cos_bit_row[1][1], 2);
        transpose_arrays_s32_8x8(buf1, buf0);
        store_buffer_8x8(buf0, output, /*stride=*/8);
        break;
    case DCT_FLIPADST:
        load_buffer_8x8(input, buf0, stride, 1);
        highbd_fdct8_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_x4(buf0, buf0, 16);
        transpose_arrays_s32_8x8(buf0, buf1);
        highbd_fadst8_xn_neon(buf1, buf1, fwd_cos_bit_row[1][1], 2);
        transpose_arrays_s32_8x8(buf1, buf0);
        store_buffer_8x8(buf0, output, /*stride=*/8);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_8x8(input, buf0, stride, 1);
        highbd_fadst8_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_x4(buf0, buf0, 16);
        transpose_arrays_s32_8x8(buf0, buf1);
        highbd_fadst8_xn_neon(buf1, buf1, fwd_cos_bit_row[1][1], 2);
        transpose_arrays_s32_8x8(buf1, buf0);
        store_buffer_8x8(buf0, output, /*stride=*/8);
        break;
    case ADST_FLIPADST:
        load_buffer_8x8(input, buf0, stride, 1);
        highbd_fadst8_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_x4(buf0, buf0, 16);
        transpose_arrays_s32_8x8(buf0, buf1);
        highbd_fadst8_xn_neon(buf1, buf1, fwd_cos_bit_row[1][1], 2);
        transpose_arrays_s32_8x8(buf1, buf0);
        store_buffer_8x8(buf0, output, /*stride=*/8);
        break;
    case FLIPADST_ADST:
        load_buffer_8x8(input, buf0, stride, 0);
        highbd_fadst8_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_x4(buf0, buf0, 16);
        transpose_arrays_s32_8x8(buf0, buf1);
        highbd_fadst8_xn_neon(buf1, buf1, fwd_cos_bit_row[1][1], 2);
        transpose_arrays_s32_8x8(buf1, buf0);
        store_buffer_8x8(buf0, output, /*stride=*/8);
        break;
    case IDTX:
        load_buffer_8x8(input, buf0, stride, 0);
        highbd_fidentity8_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_x4(buf0, buf0, 16);
        highbd_fidentity8_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        store_buffer_8x8(buf0, output, /*stride=*/8);
        break;
    case V_DCT:
        load_buffer_8x8(input, buf0, stride, 0);
        highbd_fdct8_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_x4(buf0, buf0, 16);
        transpose_arrays_s32_8x8(buf0, buf1);
        highbd_fidentity8_xn_neon(buf1, buf1, fwd_cos_bit_col[1][1], 2);
        transpose_arrays_s32_8x8(buf1, buf0);
        store_buffer_8x8(buf0, output, /*stride=*/8);
        break;
    case H_DCT:
        load_buffer_8x8(input, buf0, stride, 0);
        highbd_fidentity8_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_x4(buf0, buf0, 16);
        transpose_arrays_s32_8x8(buf0, buf1);
        highbd_fdct8_xn_neon(buf1, buf1, fwd_cos_bit_col[1][1], 2);
        transpose_arrays_s32_8x8(buf1, buf0);
        store_buffer_8x8(buf0, output, /*stride=*/8);
        break;
    case V_ADST:
        load_buffer_8x8(input, buf0, stride, 0);
        highbd_fadst8_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_x4(buf0, buf0, 16);
        transpose_arrays_s32_8x8(buf0, buf1);
        highbd_fidentity8_xn_neon(buf1, buf1, fwd_cos_bit_col[1][1], 2);
        transpose_arrays_s32_8x8(buf1, buf0);
        store_buffer_8x8(buf0, output, /*stride=*/8);
        break;
    case H_ADST:
        load_buffer_8x8(input, buf0, stride, 0);
        highbd_fidentity8_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_x4(buf0, buf0, 16);
        transpose_arrays_s32_8x8(buf0, buf1);
        highbd_fadst8_xn_neon(buf1, buf1, fwd_cos_bit_col[1][1], 2);
        transpose_arrays_s32_8x8(buf1, buf0);
        store_buffer_8x8(buf0, output, /*stride=*/8);
        break;
    case V_FLIPADST:
        load_buffer_8x8(input, buf0, stride, 0);
        highbd_fadst8_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_x4(buf0, buf0, 16);
        transpose_arrays_s32_8x8(buf0, buf1);
        highbd_fidentity8_xn_neon(buf1, buf1, fwd_cos_bit_col[1][1], 2);
        transpose_arrays_s32_8x8(buf1, buf0);
        store_buffer_8x8(buf0, output, /*stride=*/8);
        break;
    case H_FLIPADST:
        load_buffer_8x8(input, buf0, stride, 1);
        highbd_fidentity8_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_x4(buf0, buf0, 16);
        transpose_arrays_s32_8x8(buf0, buf1);
        highbd_fadst8_xn_neon(buf1, buf1, fwd_cos_bit_col[1][1], 2);
        transpose_arrays_s32_8x8(buf1, buf0);
        store_buffer_8x8(buf0, output, /*stride=*/8);
        break;
    default:
        assert(0);
    }
}

static void highbd_fdct16_x4_neon(const int32x4_t* in, int32x4_t* out, int bit) {
    const int32_t* const cospi = cospi_arr_s32(bit);
    const int32x4_t      v_bit = vdupq_n_s32(-bit);

    int32x4_t u[16], v[16];

    // stage 1
    butterfly_dct_pre(in, u, 16);

    // stage 2
    butterfly_dct_pre(u, v, 8);
    v[8] = u[8];
    v[9] = u[9];
    butterfly_cospi32_0002_neon(cospi, u[13], u[10], &v[13], &v[10], v_bit);
    butterfly_cospi32_0002_neon(cospi, u[12], u[11], &v[12], &v[11], v_bit);
    v[14] = u[14];
    v[15] = u[15];

    // stage 3
    butterfly_dct_pre(v, u, 4);
    u[4] = v[4];
    butterfly_cospi32_0002_neon(cospi, v[6], v[5], &u[6], &u[5], v_bit);
    u[7] = v[7];
    butterfly_dct_post(v + 8, v + 8, u + 8, 8);

    // stage 4
    butterfly_cospi32_0002_neon(cospi, u[0], u[1], &v[0], &v[1], v_bit);
    butterfly_0112_neon(cospi, 16, u[3], u[2], &v[2], &v[3], v_bit);
    butterfly_dct_post(u + 4, u + 4, v + 4, 4);
    v[8] = u[8];
    butterfly_0112_neon(cospi, 16, u[14], u[9], &v[14], &v[9], v_bit);
    butterfly_2312_neon(cospi, 16, u[13], u[10], &v[10], &v[13], v_bit);
    v[11] = u[11];
    v[12] = u[12];
    v[15] = u[15];

    // stage 5
    u[0] = v[0];
    u[1] = v[1];
    u[2] = v[2];
    u[3] = v[3];
    butterfly_0112_neon(cospi, 8, v[7], v[4], &u[4], &u[7], v_bit);
    butterfly_0130_neon(cospi, 24, v[5], v[6], &u[5], &u[6], v_bit);
    butterfly_dct_post(v + 8, v + 8, u + 8, 4);
    butterfly_dct_post(v + 12, v + 12, u + 12, 4);

    // stage 6
    v[0] = u[0];
    v[1] = u[1];
    v[2] = u[2];
    v[3] = u[3];
    v[4] = u[4];
    v[5] = u[5];
    v[6] = u[6];
    v[7] = u[7];
    butterfly_0112_neon(cospi, 4, u[15], u[8], &v[8], &v[15], v_bit);
    butterfly_0130_neon(cospi, 28, u[9], u[14], &v[9], &v[14], v_bit);
    butterfly_0112_neon(cospi, 20, u[13], u[10], &v[10], &v[13], v_bit);
    butterfly_0130_neon(cospi, 12, u[11], u[12], &v[11], &v[12], v_bit);

    out[0]  = v[0];
    out[1]  = v[8];
    out[2]  = v[4];
    out[3]  = v[12];
    out[4]  = v[2];
    out[5]  = v[10];
    out[6]  = v[6];
    out[7]  = v[14];
    out[8]  = v[1];
    out[9]  = v[9];
    out[10] = v[5];
    out[11] = v[13];
    out[12] = v[3];
    out[13] = v[11];
    out[14] = v[7];
    out[15] = v[15];
}

static void highbd_fadst16_x4_neon(const int32x4_t* in, int32x4_t* out, int bit) {
    const int32_t* const cospi = cospi_arr_s32(bit);
    const int32x4_t      v_bit = vdupq_n_s32(-bit);

    int32x4_t u[16], v[16];

    // stage 0-1
    u[0]  = in[0];
    u[1]  = in[15];
    u[2]  = in[7];
    u[3]  = in[8];
    u[4]  = in[3];
    u[5]  = in[12];
    u[6]  = in[4];
    u[7]  = in[11];
    u[8]  = in[1];
    u[9]  = in[14];
    u[10] = in[6];
    u[11] = in[9];
    u[12] = in[2];
    u[13] = in[13];
    u[14] = in[5];
    u[15] = in[10];

    // stage 2
    v[0] = u[0];
    v[1] = u[1];
    butterfly_cospi32_0222_neon(cospi, u[3], u[2], &v[2], &v[3], v_bit);
    v[4] = u[4];
    v[5] = u[5];
    butterfly_cospi32_0002_neon(cospi, u[6], u[7], &v[7], &v[6], v_bit);
    v[8] = u[8];
    v[9] = u[9];
    butterfly_cospi32_0002_neon(cospi, u[10], u[11], &v[11], &v[10], v_bit);
    v[12] = u[12];
    v[13] = u[13];
    butterfly_cospi32_0222_neon(cospi, u[15], u[14], &v[14], &v[15], v_bit);

    // stage 3
    u[0]  = vaddq_s32(v[0], v[2]);
    u[1]  = vsubq_s32(v[3], v[1]);
    u[2]  = vsubq_s32(v[0], v[2]);
    u[3]  = vaddq_s32(v[1], v[3]);
    u[4]  = vsubq_s32(v[6], v[4]);
    u[5]  = vaddq_s32(v[5], v[7]);
    u[6]  = vaddq_s32(v[4], v[6]);
    u[7]  = vsubq_s32(v[5], v[7]);
    u[8]  = vsubq_s32(v[10], v[8]);
    u[9]  = vaddq_s32(v[9], v[11]);
    u[10] = vaddq_s32(v[8], v[10]);
    u[11] = vsubq_s32(v[9], v[11]);
    u[12] = vaddq_s32(v[12], v[14]);
    u[13] = vsubq_s32(v[15], v[13]);
    u[14] = vsubq_s32(v[12], v[14]);
    u[15] = vaddq_s32(v[13], v[15]);

    // stage 4
    v[0] = u[0];
    v[1] = u[1];
    v[2] = u[2];
    v[3] = u[3];
    butterfly_0112_neon(cospi, 16, u[4], u[5], &v[4], &v[5], v_bit);
    butterfly_0112_neon(cospi, 16, u[7], u[6], &v[6], &v[7], v_bit);

    v[8]  = u[8];
    v[9]  = u[9];
    v[10] = u[10];
    v[11] = u[11];

    butterfly_0112_neon(cospi, 16, u[12], u[13], &v[12], &v[13], v_bit);
    butterfly_0332_neon(cospi, 16, u[14], u[15], &v[15], &v[14], v_bit);

    // stage 5
    u[0]  = vaddq_s32(v[0], v[4]);
    u[1]  = vaddq_s32(v[1], v[5]);
    u[2]  = vaddq_s32(v[2], v[6]);
    u[3]  = vsubq_s32(v[7], v[3]);
    u[4]  = vsubq_s32(v[0], v[4]);
    u[5]  = vsubq_s32(v[1], v[5]);
    u[6]  = vsubq_s32(v[2], v[6]);
    u[7]  = vaddq_s32(v[3], v[7]);
    u[8]  = vaddq_s32(v[8], v[12]);
    u[9]  = vaddq_s32(v[9], v[13]);
    u[10] = vsubq_s32(v[14], v[10]);
    u[11] = vaddq_s32(v[11], v[15]);
    u[12] = vsubq_s32(v[8], v[12]);
    u[13] = vsubq_s32(v[9], v[13]);
    u[14] = vaddq_s32(v[10], v[14]);
    u[15] = vsubq_s32(v[11], v[15]);

    // stage 6
    v[0] = u[0];
    v[1] = u[1];
    v[2] = u[2];
    v[3] = u[3];
    v[4] = u[4];
    v[5] = u[5];
    v[6] = u[6];
    v[7] = u[7];

    butterfly_0112_neon(cospi, 8, u[8], u[9], &v[8], &v[9], v_bit);
    butterfly_0130_neon(cospi, 8, u[12], u[13], &v[13], &v[12], v_bit);
    butterfly_0130_neon(cospi, 24, u[11], u[10], &v[10], &v[11], v_bit);
    butterfly_0130_neon(cospi, 24, u[14], u[15], &v[14], &v[15], v_bit);

    // stage 7
    u[0]  = vaddq_s32(v[0], v[8]);
    u[1]  = vaddq_s32(v[1], v[9]);
    u[2]  = vaddq_s32(v[2], v[10]);
    u[3]  = vaddq_s32(v[3], v[11]);
    u[4]  = vaddq_s32(v[4], v[12]);
    u[5]  = vaddq_s32(v[5], v[13]);
    u[6]  = vaddq_s32(v[6], v[14]);
    u[7]  = vsubq_s32(v[15], v[7]);
    u[8]  = vsubq_s32(v[0], v[8]);
    u[9]  = vsubq_s32(v[1], v[9]);
    u[10] = vsubq_s32(v[2], v[10]);
    u[11] = vsubq_s32(v[3], v[11]);
    u[12] = vsubq_s32(v[4], v[12]);
    u[13] = vsubq_s32(v[5], v[13]);
    u[14] = vsubq_s32(v[6], v[14]);
    u[15] = vaddq_s32(v[7], v[15]);

    // stage 8
    butterfly_0112_neon(cospi, 2, u[0], u[1], &v[0], &v[1], v_bit);
    butterfly_0112_neon(cospi, 10, u[2], u[3], &v[2], &v[3], v_bit);
    butterfly_0112_neon(cospi, 18, u[4], u[5], &v[4], &v[5], v_bit);
    butterfly_0112_neon(cospi, 26, u[6], u[7], &v[6], &v[7], v_bit);
    butterfly_0130_neon(cospi, 30, u[9], u[8], &v[8], &v[9], v_bit);
    butterfly_0130_neon(cospi, 22, u[11], u[10], &v[10], &v[11], v_bit);
    butterfly_0130_neon(cospi, 14, u[13], u[12], &v[12], &v[13], v_bit);
    butterfly_0112_neon(cospi, 6, u[14], u[15], &v[15], &v[14], v_bit);

    // stage 9
    out[0]  = v[1];
    out[1]  = v[14];
    out[2]  = v[3];
    out[3]  = v[12];
    out[4]  = v[5];
    out[5]  = v[10];
    out[6]  = v[7];
    out[7]  = v[8];
    out[8]  = v[9];
    out[9]  = v[6];
    out[10] = v[11];
    out[11] = v[4];
    out[12] = v[13];
    out[13] = v[2];
    out[14] = v[15];
    out[15] = v[0];
}

static void highbd_fidentity16_x4_neon(const int32x4_t* in, int32x4_t* out, int bit) {
    (void)bit;
    const int32x4_t fact = vdupq_n_s32(2 * new_sqrt2);

    for (int i = 0; i < 16; i++) {
        int32x4_t a = vmulq_s32(in[i], fact);
        out[i]      = vrshrq_n_s32(a, new_sqrt2_bits);
    }
}

static void highbd_fdct16_xn_neon(const int32x4_t* in, int32x4_t* out, int bit, const int howmany) {
    const int stride = 16;
    int       i      = 0;
    do {
        highbd_fdct16_x4_neon(in + i * stride, out + i * stride, bit);
    } while (++i < howmany);
}

static void highbd_fadst16_xn_neon(const int32x4_t* in, int32x4_t* out, int bit, int howmany) {
    const int stride = 16;
    int       i      = 0;
    do {
        highbd_fadst16_x4_neon(in + i * stride, out + i * stride, bit);
    } while (++i < howmany);
}

static void highbd_fidentity16_xn_neon(const int32x4_t* in, int32x4_t* out, int bit, int howmany) {
    const int stride = 16;
    int       i      = 0;
    do {
        highbd_fidentity16_x4_neon(in + i * stride, out + i * stride, bit);
    } while (++i < howmany);
}

void svt_av1_fwd_txfm2d_16x16_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 16);

    // Workspaces for column/row-wise transforms.
    int32x4_t buf0[64], buf1[64];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_16x16(input, buf0, stride, 0);
        highbd_fdct16_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_x4(buf0, buf0, 64);
        transpose_arrays_s32_16x16(buf0, buf1);
        highbd_fdct16_xn_neon(buf1, buf1, fwd_cos_bit_row[2][2], 4);
        transpose_arrays_s32_16x16(buf1, buf0);
        store_buffer_16x16(buf0, output, /*stride=*/16);
        break;
    case ADST_DCT:
        load_buffer_16x16(input, buf0, stride, 0);
        highbd_fadst16_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_x4(buf0, buf0, 64);
        transpose_arrays_s32_16x16(buf0, buf1);
        highbd_fdct16_xn_neon(buf1, buf1, fwd_cos_bit_row[2][2], 4);
        transpose_arrays_s32_16x16(buf1, buf0);
        store_buffer_16x16(buf0, output, /*stride=*/16);
        break;
    case DCT_ADST:
        load_buffer_16x16(input, buf0, stride, 0);
        highbd_fdct16_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_x4(buf0, buf0, 64);
        transpose_arrays_s32_16x16(buf0, buf1);
        highbd_fadst16_xn_neon(buf1, buf1, fwd_cos_bit_row[2][2], 4);
        transpose_arrays_s32_16x16(buf1, buf0);
        store_buffer_16x16(buf0, output, /*stride=*/16);
        break;
    case ADST_ADST:
        load_buffer_16x16(input, buf0, stride, 0);
        highbd_fadst16_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_x4(buf0, buf0, 64);
        transpose_arrays_s32_16x16(buf0, buf1);
        highbd_fadst16_xn_neon(buf1, buf1, fwd_cos_bit_row[2][2], 4);
        transpose_arrays_s32_16x16(buf1, buf0);
        store_buffer_16x16(buf0, output, /*stride=*/16);
        break;
    case FLIPADST_DCT:
        load_buffer_16x16(input, buf0, stride, 0);
        highbd_fadst16_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_x4(buf0, buf0, 64);
        transpose_arrays_s32_16x16(buf0, buf1);
        highbd_fdct16_xn_neon(buf1, buf1, fwd_cos_bit_row[2][2], 4);
        transpose_arrays_s32_16x16(buf1, buf0);
        store_buffer_16x16(buf0, output, /*stride=*/16);
        break;
    case DCT_FLIPADST:
        load_buffer_16x16(input, buf0, stride, 1);
        highbd_fdct16_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_x4(buf0, buf0, 64);
        transpose_arrays_s32_16x16(buf0, buf1);
        highbd_fadst16_xn_neon(buf1, buf1, fwd_cos_bit_row[2][2], 4);
        transpose_arrays_s32_16x16(buf1, buf0);
        store_buffer_16x16(buf0, output, /*stride=*/16);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_16x16(input, buf0, stride, 1);
        highbd_fadst16_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_x4(buf0, buf0, 64);
        transpose_arrays_s32_16x16(buf0, buf1);
        highbd_fadst16_xn_neon(buf1, buf1, fwd_cos_bit_row[2][2], 4);
        transpose_arrays_s32_16x16(buf1, buf0);
        store_buffer_16x16(buf0, output, /*stride=*/16);
        break;
    case ADST_FLIPADST:
        load_buffer_16x16(input, buf0, stride, 1);
        highbd_fadst16_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_x4(buf0, buf0, 64);
        transpose_arrays_s32_16x16(buf0, buf1);
        highbd_fadst16_xn_neon(buf1, buf1, fwd_cos_bit_row[2][2], 4);
        transpose_arrays_s32_16x16(buf1, buf0);
        store_buffer_16x16(buf0, output, /*stride=*/16);
        break;
    case FLIPADST_ADST:
        load_buffer_16x16(input, buf0, stride, 0);
        highbd_fadst16_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_x4(buf0, buf0, 64);
        transpose_arrays_s32_16x16(buf0, buf1);
        highbd_fadst16_xn_neon(buf1, buf1, fwd_cos_bit_row[2][2], 4);
        transpose_arrays_s32_16x16(buf1, buf0);
        store_buffer_16x16(buf0, output, /*stride=*/16);
        break;
    case IDTX:
        load_buffer_16x16(input, buf0, stride, 0);
        highbd_fidentity16_xn_neon(buf0, buf1, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_x4(buf1, buf1, 64);
        highbd_fidentity16_xn_neon(buf1, buf0, fwd_cos_bit_row[2][2], 4);
        store_buffer_16x16(buf0, output, /*stride=*/16);
        break;
    case V_DCT:
        load_buffer_16x16(input, buf0, stride, 0);
        highbd_fdct16_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_x4(buf0, buf0, 64);
        transpose_arrays_s32_16x16(buf0, buf1);
        highbd_fidentity16_xn_neon(buf1, buf1, fwd_cos_bit_row[2][2], 4);
        transpose_arrays_s32_16x16(buf1, buf0);
        store_buffer_16x16(buf0, output, /*stride=*/16);
        break;
    case H_DCT:
        load_buffer_16x16(input, buf0, stride, 0);
        highbd_fidentity16_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_x4(buf0, buf0, 64);
        transpose_arrays_s32_16x16(buf0, buf1);
        highbd_fdct16_xn_neon(buf1, buf1, fwd_cos_bit_row[2][2], 4);
        transpose_arrays_s32_16x16(buf1, buf0);
        store_buffer_16x16(buf0, output, /*stride=*/16);
        break;
    case V_ADST:
        load_buffer_16x16(input, buf0, stride, 0);
        highbd_fadst16_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_x4(buf0, buf0, 64);
        transpose_arrays_s32_16x16(buf0, buf1);
        highbd_fidentity16_xn_neon(buf1, buf1, fwd_cos_bit_row[2][2], 4);
        transpose_arrays_s32_16x16(buf1, buf0);
        store_buffer_16x16(buf0, output, /*stride=*/16);
        break;
    case H_ADST:
        load_buffer_16x16(input, buf0, stride, 0);
        highbd_fidentity16_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_x4(buf0, buf0, 64);
        transpose_arrays_s32_16x16(buf0, buf1);
        highbd_fadst16_xn_neon(buf1, buf1, fwd_cos_bit_row[2][2], 4);
        transpose_arrays_s32_16x16(buf1, buf0);
        store_buffer_16x16(buf0, output, /*stride=*/16);
        break;
    case V_FLIPADST:
        load_buffer_16x16(input, buf0, stride, 0);
        highbd_fadst16_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_x4(buf0, buf0, 64);
        transpose_arrays_s32_16x16(buf0, buf1);
        highbd_fidentity16_xn_neon(buf1, buf1, fwd_cos_bit_row[2][2], 4);
        transpose_arrays_s32_16x16(buf1, buf0);
        store_buffer_16x16(buf0, output, /*stride=*/16);
        break;
    case H_FLIPADST:
        load_buffer_16x16(input, buf0, stride, 1);
        highbd_fidentity16_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_x4(buf0, buf0, 64);
        transpose_arrays_s32_16x16(buf0, buf1);
        highbd_fadst16_xn_neon(buf1, buf1, fwd_cos_bit_row[2][2], 4);
        transpose_arrays_s32_16x16(buf1, buf0);
        store_buffer_16x16(buf0, output, /*stride=*/16);
        break;
    default:
        assert(0);
    }
}

static AOM_FORCE_INLINE void round_rect_array_s32_neon(const int32x4_t* input, int32x4_t* output, const int size) {
    const int32x4_t sqrt2 = vdupq_n_s32(new_sqrt2);
    int             i     = 0;
    do {
        const int32x4_t r1 = vmulq_s32(input[i], sqrt2);
        output[i]          = vrshrq_n_s32(r1, new_sqrt2_bits);
    } while (++i < size);
}

typedef void (*fwd_transform_1d_col_neon)(const int16_t* in, int32x4_t* out, int stride, int bit, int lr_flip);
typedef void (*fwd_transform_1d_col_many_neon)(const int16_t* in, int32x4_t* out, int stride, int bit, int lr_flip,
                                               int howmany, int hm_stride);

typedef void (*fwd_transform_1d_row_neon)(const int32x4_t* in, int32x4_t* out, int bit);
typedef void (*fwd_transform_1d_row_many_neon)(const int32x4_t* in, int32x4_t* out, int bit, int howmany,
                                               int hm_stride);

// Construct component kernels that include the load_buffer and store_buffer
// stages to avoid the need to spill loaded data to the stack between these and
// the txfm kernel calls.
// The TRANSFORM_*_ONE cases are only ever called in situations where the
// howmany parameter would be one, so no need for the loop at all in these
// cases.

#define TRANSFORM_COL_ONE(name, n)                                                       \
    static void highbd_##name##_col_neon(                                                \
        const int16_t* input, int32x4_t* output, int stride, int cos_bit, int lr_flip) { \
        int32x4_t buf0[n];                                                               \
        load_buffer_4x##n(input, buf0, stride, lr_flip);                                 \
        highbd_##name##_x4_neon(buf0, output, cos_bit);                                  \
    }

#define TRANSFORM_COL_MANY(name, n)                                                                                  \
    static void highbd_##name##_col_many_neon(                                                                       \
        const int16_t* input, int32x4_t* output, int stride, int cos_bit, int lr_flip, int howmany, int hm_stride) { \
        int i = 0;                                                                                                   \
        do {                                                                                                         \
            int32x4_t buf0[n];                                                                                       \
            load_buffer_4x##n(input + 4 * i, buf0, stride, lr_flip);                                                 \
            highbd_##name##_x4_neon(buf0, output + i * hm_stride, cos_bit);                                          \
        } while (++i < howmany);                                                                                     \
    }

#define TRANSFORM_ROW_ONE(name, n)                                                                 \
    static void highbd_##name##_row_neon(const int32x4_t* input, int32x4_t* output, int cos_bit) { \
        highbd_##name##_x4_neon(input, output, cos_bit);                                           \
    }

#define TRANSFORM_ROW_RECT_ONE(name, n)                                                                 \
    static void highbd_##name##_row_rect_neon(const int32x4_t* input, int32x4_t* output, int cos_bit) { \
        highbd_##name##_x4_neon(input, output, cos_bit);                                                \
        round_rect_array_s32_neon(output, output, (n));                                                 \
    }

#define TRANSFORM_ROW_MANY(name, n)                                                           \
    static void highbd_##name##_row_many_neon(                                                \
        const int32x4_t* input, int32x4_t* output, int cos_bit, int howmany, int hm_stride) { \
        int i = 0;                                                                            \
        do {                                                                                  \
            highbd_##name##_x4_neon(input + hm_stride * i, output + hm_stride * i, cos_bit);  \
        } while (++i < howmany);                                                              \
    }

#define TRANSFORM_ROW_RECT_MANY(name, n)                                                      \
    static void highbd_##name##_row_rect_many_neon(                                           \
        const int32x4_t* input, int32x4_t* output, int cos_bit, int howmany, int hm_stride) { \
        int i = 0;                                                                            \
        do {                                                                                  \
            highbd_##name##_x4_neon(input + hm_stride * i, output + hm_stride * i, cos_bit);  \
            round_rect_array_s32_neon(output + hm_stride * i, output + hm_stride * i, (n));   \
        } while (++i < howmany);                                                              \
    }

TRANSFORM_COL_ONE(fdct8, 8)
TRANSFORM_COL_ONE(fadst8, 8)
TRANSFORM_COL_ONE(fidentity8, 8)

TRANSFORM_COL_MANY(fdct4, 4)
TRANSFORM_COL_MANY(fadst4, 4)
TRANSFORM_COL_MANY(fidentity4, 4)
TRANSFORM_COL_MANY(fdct8, 8)
TRANSFORM_COL_MANY(fadst8, 8)
TRANSFORM_COL_MANY(fidentity8, 8)
TRANSFORM_COL_MANY(fdct16, 16)
TRANSFORM_COL_MANY(fadst16, 16)
TRANSFORM_COL_MANY(fidentity16, 16)

TRANSFORM_ROW_ONE(fdct16, 16)
TRANSFORM_ROW_ONE(fadst16, 16)
TRANSFORM_ROW_ONE(fidentity16, 16)

TRANSFORM_ROW_MANY(fdct4, 4)
TRANSFORM_ROW_MANY(fadst4, 4)
TRANSFORM_ROW_MANY(fidentity4, 4)
TRANSFORM_ROW_MANY(fdct8, 8)
TRANSFORM_ROW_MANY(fadst8, 8)
TRANSFORM_ROW_MANY(fidentity8, 8)

TRANSFORM_ROW_RECT_ONE(fdct8, 8)
TRANSFORM_ROW_RECT_ONE(fadst8, 8)
TRANSFORM_ROW_RECT_ONE(fidentity8, 8)

TRANSFORM_ROW_RECT_MANY(fdct4, 4)
TRANSFORM_ROW_RECT_MANY(fadst4, 4)
TRANSFORM_ROW_RECT_MANY(fidentity4, 4)
TRANSFORM_ROW_RECT_MANY(fdct8, 8)
TRANSFORM_ROW_RECT_MANY(fadst8, 8)
TRANSFORM_ROW_RECT_MANY(fidentity8, 8)
TRANSFORM_ROW_RECT_MANY(fdct16, 16)
TRANSFORM_ROW_RECT_MANY(fadst16, 16)
TRANSFORM_ROW_RECT_MANY(fidentity16, 16)

static const fwd_transform_1d_col_neon col_highbd_txfm8_x4_arr[TX_TYPES] = {
    highbd_fdct8_col_neon, // DCT_DCT
    highbd_fadst8_col_neon, // ADST_DCT
    highbd_fdct8_col_neon, // DCT_ADST
    highbd_fadst8_col_neon, // ADST_ADST
    highbd_fadst8_col_neon, // FLIPADST_DCT
    highbd_fdct8_col_neon, // DCT_FLIPADST
    highbd_fadst8_col_neon, // FLIPADST_FLIPADST
    highbd_fadst8_col_neon, // ADST_FLIPADST
    highbd_fadst8_col_neon, // FLIPADST_ADST
    highbd_fidentity8_col_neon, // IDTX
    highbd_fdct8_col_neon, // V_DCT
    highbd_fidentity8_col_neon, // H_DCT
    highbd_fadst8_col_neon, // V_ADST
    highbd_fidentity8_col_neon, // H_ADST
    highbd_fadst8_col_neon, // V_FLIPADST
    highbd_fidentity8_col_neon // H_FLIPADST
};

static const fwd_transform_1d_col_many_neon col_highbd_txfm16_xn_arr[TX_TYPES] = {
    highbd_fdct16_col_many_neon, // DCT_DCT
    highbd_fadst16_col_many_neon, // ADST_DCT
    highbd_fdct16_col_many_neon, // DCT_ADST
    highbd_fadst16_col_many_neon, // ADST_ADST
    highbd_fadst16_col_many_neon, // FLIPADST_DCT
    highbd_fdct16_col_many_neon, // DCT_FLIPADST
    highbd_fadst16_col_many_neon, // FLIPADST_FLIPADST
    highbd_fadst16_col_many_neon, // ADST_FLIPADST
    highbd_fadst16_col_many_neon, // FLIPADST_ADST
    highbd_fidentity16_col_many_neon, // IDTX
    highbd_fdct16_col_many_neon, // V_DCT
    highbd_fidentity16_col_many_neon, // H_DCT
    highbd_fadst16_col_many_neon, // V_ADST
    highbd_fidentity16_col_many_neon, // H_ADST
    highbd_fadst16_col_many_neon, // V_FLIPADST
    highbd_fidentity16_col_many_neon // H_FLIPADST
};

static const fwd_transform_1d_row_many_neon row_rect_highbd_txfm4_xn_arr[TX_TYPES] = {
    highbd_fdct4_row_rect_many_neon, // DCT_DCT
    highbd_fdct4_row_rect_many_neon, // ADST_DCT
    highbd_fadst4_row_rect_many_neon, // DCT_ADST
    highbd_fadst4_row_rect_many_neon, // ADST_ADST
    highbd_fdct4_row_rect_many_neon, // FLIPADST_DCT
    highbd_fadst4_row_rect_many_neon, // DCT_FLIPADST
    highbd_fadst4_row_rect_many_neon, // FLIPADST_FLIPADST
    highbd_fadst4_row_rect_many_neon, // ADST_FLIPADST
    highbd_fadst4_row_rect_many_neon, // FLIPADST_ADST
    highbd_fidentity4_row_rect_many_neon, // IDTX
    highbd_fidentity4_row_rect_many_neon, // V_DCT
    highbd_fdct4_row_rect_many_neon, // H_DCT
    highbd_fidentity4_row_rect_many_neon, // V_ADST
    highbd_fadst4_row_rect_many_neon, // H_ADST
    highbd_fidentity4_row_rect_many_neon, // V_FLIPADST
    highbd_fadst4_row_rect_many_neon // H_FLIPADST
};

void svt_av1_fwd_txfm2d_4x8_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    int                                  bitcol   = fwd_cos_bit_col[0][1];
    int                                  bitrow   = fwd_cos_bit_row[0][1];
    const fwd_transform_1d_col_neon      col_txfm = col_highbd_txfm8_x4_arr[tx_type];
    const fwd_transform_1d_row_many_neon row_txfm = row_rect_highbd_txfm4_xn_arr[tx_type];

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 8);

    // Column-wise transform.
    int32x4_t buf0[8];
    col_txfm(input, buf0, stride, bitcol, lr_flip);
    shift_right_1_round_s32_x4(buf0, buf0, 8);

    int32x4_t buf1[8];
    transpose_arrays_s32_4x8(buf0, buf1);

    // Row-wise transform.
    row_txfm(buf1, buf0, bitrow, /*howmany=*/2, /*hm_stride=*/4);
    transpose_arrays_s32_4x8(buf0, (int32x4_t*)output);
}

static const fwd_transform_1d_col_many_neon col_highbd_txfm4_xn_arr[TX_TYPES] = {
    highbd_fdct4_col_many_neon, // DCT_DCT
    highbd_fadst4_col_many_neon, // ADST_DCT
    highbd_fdct4_col_many_neon, // DCT_ADST
    highbd_fadst4_col_many_neon, // ADST_ADST
    highbd_fadst4_col_many_neon, // FLIPADST_DCT
    highbd_fdct4_col_many_neon, // DCT_FLIPADST
    highbd_fadst4_col_many_neon, // FLIPADST_FLIPADST
    highbd_fadst4_col_many_neon, // ADST_FLIPADST
    highbd_fadst4_col_many_neon, // FLIPADST_ADST
    highbd_fidentity4_col_many_neon, // IDTX
    highbd_fdct4_col_many_neon, // V_DCT
    highbd_fidentity4_col_many_neon, // H_DCT
    highbd_fadst4_col_many_neon, // V_ADST
    highbd_fidentity4_col_many_neon, // H_ADST
    highbd_fadst4_col_many_neon, // V_FLIPADST
    highbd_fidentity4_col_many_neon // H_FLIPADST
};

static const fwd_transform_1d_row_neon row_rect_highbd_txfm8_x4_arr[TX_TYPES] = {
    highbd_fdct8_row_rect_neon, // DCT_DCT
    highbd_fdct8_row_rect_neon, // ADST_DCT
    highbd_fadst8_row_rect_neon, // DCT_ADST
    highbd_fadst8_row_rect_neon, // ADST_ADST
    highbd_fdct8_row_rect_neon, // FLIPADST_DCT
    highbd_fadst8_row_rect_neon, // DCT_FLIPADST
    highbd_fadst8_row_rect_neon, // FLIPADST_FLIPADST
    highbd_fadst8_row_rect_neon, // ADST_FLIPADST
    highbd_fadst8_row_rect_neon, // FLIPADST_ADST
    highbd_fidentity8_row_rect_neon, // IDTX
    highbd_fidentity8_row_rect_neon, // V_DCT
    highbd_fdct8_row_rect_neon, // H_DCT
    highbd_fidentity8_row_rect_neon, // V_ADST
    highbd_fadst8_row_rect_neon, // H_ADST
    highbd_fidentity8_row_rect_neon, // V_FLIPADST
    highbd_fadst8_row_rect_neon // H_FLIPADST
};

static const fwd_transform_1d_row_many_neon row_rect_highbd_txfm8_xn_arr[TX_TYPES] = {
    highbd_fdct8_row_rect_many_neon, // DCT_DCT
    highbd_fdct8_row_rect_many_neon, // ADST_DCT
    highbd_fadst8_row_rect_many_neon, // DCT_ADST
    highbd_fadst8_row_rect_many_neon, // ADST_ADST
    highbd_fdct8_row_rect_many_neon, // FLIPADST_DCT
    highbd_fadst8_row_rect_many_neon, // DCT_FLIPADST
    highbd_fadst8_row_rect_many_neon, // FLIPADST_FLIPADST
    highbd_fadst8_row_rect_many_neon, // ADST_FLIPADST
    highbd_fadst8_row_rect_many_neon, // FLIPADST_ADST
    highbd_fidentity8_row_rect_many_neon, // IDTX
    highbd_fidentity8_row_rect_many_neon, // V_DCT
    highbd_fdct8_row_rect_many_neon, // H_DCT
    highbd_fidentity8_row_rect_many_neon, // V_ADST
    highbd_fadst8_row_rect_many_neon, // H_ADST
    highbd_fidentity8_row_rect_many_neon, // V_FLIPADST
    highbd_fadst8_row_rect_many_neon // H_FLIPADST
};

static const fwd_transform_1d_row_many_neon row_highbd_txfm4_xn_arr[TX_TYPES] = {
    highbd_fdct4_row_many_neon, // DCT_DCT
    highbd_fdct4_row_many_neon, // ADST_DCT
    highbd_fadst4_row_many_neon, // DCT_ADST
    highbd_fadst4_row_many_neon, // ADST_ADST
    highbd_fdct4_row_many_neon, // FLIPADST_DCT
    highbd_fadst4_row_many_neon, // DCT_FLIPADST
    highbd_fadst4_row_many_neon, // FLIPADST_FLIPADST
    highbd_fadst4_row_many_neon, // ADST_FLIPADST
    highbd_fadst4_row_many_neon, // FLIPADST_ADST
    highbd_fidentity4_row_many_neon, // IDTX
    highbd_fidentity4_row_many_neon, // V_DCT
    highbd_fdct4_row_many_neon, // H_DCT
    highbd_fidentity4_row_many_neon, // V_ADST
    highbd_fadst4_row_many_neon, // H_ADST
    highbd_fidentity4_row_many_neon, // V_FLIPADST
    highbd_fadst4_row_many_neon // H_FLIPADST
};

static const fwd_transform_1d_row_many_neon row_highbd_txfm8_xn_arr[TX_TYPES] = {
    highbd_fdct8_row_many_neon, // DCT_DCT
    highbd_fdct8_row_many_neon, // ADST_DCT
    highbd_fadst8_row_many_neon, // DCT_ADST
    highbd_fadst8_row_many_neon, // ADST_ADST
    highbd_fdct8_row_many_neon, // FLIPADST_DCT
    highbd_fadst8_row_many_neon, // DCT_FLIPADST
    highbd_fadst8_row_many_neon, // FLIPADST_FLIPADST
    highbd_fadst8_row_many_neon, // ADST_FLIPADST
    highbd_fadst8_row_many_neon, // FLIPADST_ADST
    highbd_fidentity8_row_many_neon, // IDTX
    highbd_fidentity8_row_many_neon, // V_DCT
    highbd_fdct8_row_many_neon, // H_DCT
    highbd_fidentity8_row_many_neon, // V_ADST
    highbd_fadst8_row_many_neon, // H_ADST
    highbd_fidentity8_row_many_neon, // V_FLIPADST
    highbd_fadst8_row_many_neon // H_FLIPADST
};

void svt_av1_fwd_txfm2d_4x16_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    int                                  bitcol   = fwd_cos_bit_col[0][2];
    int                                  bitrow   = fwd_cos_bit_row[0][2];
    const fwd_transform_1d_col_many_neon col_txfm = col_highbd_txfm16_xn_arr[tx_type];
    const fwd_transform_1d_row_many_neon row_txfm = row_highbd_txfm4_xn_arr[tx_type];

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 16);

    // Column-wise transform.
    int32x4_t buf0[16];
    if (lr_flip) {
        col_txfm(input,
                 buf0,
                 stride,
                 bitcol,
                 /*lr_flip=*/1,
                 /*howmany=*/1,
                 /*hm_stride=*/0);
    } else {
        col_txfm(input,
                 buf0,
                 stride,
                 bitcol,
                 /*lr_flip=*/0,
                 /*howmany=*/1,
                 /*hm_stride=*/0);
    }
    shift_right_1_round_s32_x4(buf0, buf0, 16);

    int32x4_t buf1[16];
    transpose_arrays_s32_4x16(buf0, buf1);

    // Row-wise transform.
    row_txfm(buf1, buf0, bitrow, /*howmany=*/4, /*hm_stride=*/4);
    transpose_arrays_s32_4x16(buf0, (int32x4_t*)output);
}

static inline void transpose_elems_s32_8x4(const int32x4_t* in, int32x4_t* out) {
    transpose_elems_s32_4x4(in[0], in[1], in[2], in[3], &out[0], &out[2], &out[4], &out[6]);
    transpose_elems_s32_4x4(in[4], in[5], in[6], in[7], &out[1], &out[3], &out[5], &out[7]);
}

static inline void transpose_8xh(const int32x4_t* in, int32x4_t* out, int n) {
    for (int i = 0; i < n; i += 8) {
        transpose_elems_s32_8x4(in + i, out + i);
    }
}

void svt_av1_fwd_txfm2d_8x4_neon(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    const int                            bitcol   = fwd_cos_bit_col[1][0];
    const int                            bitrow   = fwd_cos_bit_row[1][0];
    const fwd_transform_1d_col_many_neon col_txfm = col_highbd_txfm4_xn_arr[tx_type];
    const fwd_transform_1d_row_neon      row_txfm = row_rect_highbd_txfm8_x4_arr[tx_type];

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 4);

    // Column-wise transform.
    int32x4_t buf0[8];
    if (lr_flip) {
        col_txfm(input,
                 buf0 + 4,
                 stride,
                 bitcol,
                 /*lr_flip=*/1,
                 /*howmany=*/2,
                 /*hm_stride=*/-4);
    } else {
        col_txfm(input,
                 buf0,
                 stride,
                 bitcol,
                 /*lr_flip=*/0,
                 /*howmany=*/2,
                 /*hm_stride=*/4);
    }

    shift_right_1_round_s32_x4(buf0, buf0, 8);

    int32x4_t buf1[8];
    transpose_arrays_s32_8x4(buf0, buf1);

    // Row-wise transform.
    row_txfm(buf1, buf0, bitrow);
    transpose_8xh(buf0, (int32x4_t*)coeff, 8);
}

void svt_av1_fwd_txfm2d_8x16_neon(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    const fwd_transform_1d_col_many_neon col_txfm = col_highbd_txfm16_xn_arr[tx_type];
    const fwd_transform_1d_row_many_neon row_txfm = row_rect_highbd_txfm8_xn_arr[tx_type];
    int                                  bit      = fwd_cos_bit_col[1][2];

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 16);

    // Column-wise transform.
    int32x4_t buf0[32];
    if (lr_flip) {
        col_txfm(input,
                 buf0 + 16,
                 stride,
                 bit,
                 /*lr_flip=*/1,
                 /*howmany=*/2,
                 /*hm_stride=*/-16);
    } else {
        col_txfm(input,
                 buf0,
                 stride,
                 bit,
                 /*lr_flip=*/0,
                 /*howmany=*/2,
                 /*hm_stride=*/16);
    }
    shift_right_2_round_s32_x4(buf0, buf0, 32);

    int32x4_t buf1[32];
    transpose_arrays_s32_8x16(buf0, buf1);

    // Row-wise transform.
    row_txfm(buf1, buf0, bit, /*howmany=*/4, /*hm_stride=*/8);
    transpose_8xh(buf0, (int32x4_t*)coeff, 32);
}

static void highbd_fdct32_x4_neon(const int32x4_t* input, int32x4_t* output, int cos_bit) {
    const int32_t* const cospi     = cospi_arr_s32(cos_bit);
    const int32x4_t      v_cos_bit = vdupq_n_s32(-cos_bit);

    // Workspaces for intermediate transform steps.
    int32x4_t buf0[32];
    int32x4_t buf1[32];

    // stage 1
    butterfly_dct_pre(input, buf1, 32);

    // stage 2
    butterfly_dct_pre(buf1, buf0, 16);
    buf0[16] = buf1[16];
    buf0[17] = buf1[17];
    buf0[18] = buf1[18];
    buf0[19] = buf1[19];
    butterfly_0112_neon(cospi, 32, buf1[27], buf1[20], &buf0[27], &buf0[20], v_cos_bit);
    butterfly_0112_neon(cospi, 32, buf1[26], buf1[21], &buf0[26], &buf0[21], v_cos_bit);
    butterfly_0112_neon(cospi, 32, buf1[25], buf1[22], &buf0[25], &buf0[22], v_cos_bit);
    butterfly_0112_neon(cospi, 32, buf1[24], buf1[23], &buf0[24], &buf0[23], v_cos_bit);
    buf0[28] = buf1[28];
    buf0[29] = buf1[29];
    buf0[30] = buf1[30];
    buf0[31] = buf1[31];

    // stage 3
    butterfly_dct_pre(buf0, buf1, 8);
    buf1[8] = buf0[8];
    buf1[9] = buf0[9];
    butterfly_0112_neon(cospi, 32, buf0[13], buf0[10], &buf1[13], &buf1[10], v_cos_bit);
    butterfly_0112_neon(cospi, 32, buf0[12], buf0[11], &buf1[12], &buf1[11], v_cos_bit);
    buf1[14] = buf0[14];
    buf1[15] = buf0[15];
    butterfly_dct_post(buf0 + 16, buf0 + 16, buf1 + 16, 16);

    // stage 4
    butterfly_dct_pre(buf1, buf0, 4);
    buf0[4] = buf1[4];
    butterfly_0112_neon(cospi, 32, buf1[6], buf1[5], &buf0[6], &buf0[5], v_cos_bit);
    buf0[7] = buf1[7];
    butterfly_dct_post(buf1 + 8, buf1 + 8, buf0 + 8, 8);
    buf0[16] = buf1[16];
    buf0[17] = buf1[17];
    butterfly_0112_neon(cospi, 16, buf1[29], buf1[18], &buf0[29], &buf0[18], v_cos_bit);
    butterfly_0112_neon(cospi, 16, buf1[28], buf1[19], &buf0[28], &buf0[19], v_cos_bit);
    butterfly_2312_neon(cospi, 16, buf1[27], buf1[20], &buf0[20], &buf0[27], v_cos_bit);
    butterfly_2312_neon(cospi, 16, buf1[26], buf1[21], &buf0[21], &buf0[26], v_cos_bit);
    buf0[22] = buf1[22];
    buf0[23] = buf1[23];
    buf0[24] = buf1[24];
    buf0[25] = buf1[25];
    buf0[30] = buf1[30];
    buf0[31] = buf1[31];

    // stage 5
    butterfly_0112_neon(cospi, 32, buf0[0], buf0[1], &buf1[0], &buf1[1], v_cos_bit);
    butterfly_0112_neon(cospi, 16, buf0[3], buf0[2], &buf1[2], &buf1[3], v_cos_bit);
    butterfly_dct_post(buf0 + 4, buf0 + 4, buf1 + 4, 4);
    buf1[8] = buf0[8];
    butterfly_0112_neon(cospi, 16, buf0[14], buf0[9], &buf1[14], &buf1[9], v_cos_bit);
    butterfly_2312_neon(cospi, 16, buf0[13], buf0[10], &buf1[10], &buf1[13], v_cos_bit);
    buf1[11] = buf0[11];
    buf1[12] = buf0[12];
    buf1[15] = buf0[15];
    butterfly_dct_post(buf0 + 16, buf0 + 16, buf1 + 16, 8);
    butterfly_dct_post(buf0 + 24, buf0 + 24, buf1 + 24, 8);

    // stage 6
    buf0[0] = buf1[0];
    buf0[1] = buf1[1];
    buf0[2] = buf1[2];
    buf0[3] = buf1[3];

    butterfly_0112_neon(cospi, 8, buf1[7], buf1[4], &buf0[4], &buf0[7], v_cos_bit);
    butterfly_0112_neon(cospi, 8, buf1[30], buf1[17], &buf0[30], &buf0[17], v_cos_bit);
    butterfly_2312_neon(cospi, 8, buf1[29], buf1[18], &buf0[18], &buf0[29], v_cos_bit);
    butterfly_dct_post(buf1 + 8, buf1 + 8, buf0 + 8, 4);
    butterfly_dct_post(buf1 + 12, buf1 + 12, buf0 + 12, 4);
    buf0[16] = buf1[16];
    buf0[19] = buf1[19];
    buf0[20] = buf1[20];

    butterfly_0130_neon(cospi, 24, buf1[5], buf1[6], &buf0[5], &buf0[6], v_cos_bit);
    butterfly_0130_neon(cospi, 24, buf1[21], buf1[26], &buf0[26], &buf0[21], v_cos_bit);
    butterfly_0332_neon(cospi, 24, buf1[25], buf1[22], &buf0[25], &buf0[22], v_cos_bit);

    buf0[23] = buf1[23];
    buf0[24] = buf1[24];
    buf0[27] = buf1[27];
    buf0[28] = buf1[28];
    buf0[31] = buf1[31];

    // stage 7
    buf1[0] = buf0[0];
    buf1[1] = buf0[1];
    buf1[2] = buf0[2];
    buf1[3] = buf0[3];
    buf1[4] = buf0[4];
    buf1[5] = buf0[5];
    buf1[6] = buf0[6];
    buf1[7] = buf0[7];
    butterfly_0112_neon(cospi, 4, buf0[15], buf0[8], &buf1[8], &buf1[15], v_cos_bit);
    butterfly_0130_neon(cospi, 28, buf0[9], buf0[14], &buf1[9], &buf1[14], v_cos_bit);
    butterfly_0112_neon(cospi, 20, buf0[13], buf0[10], &buf1[10], &buf1[13], v_cos_bit);
    butterfly_0130_neon(cospi, 12, buf0[11], buf0[12], &buf1[11], &buf1[12], v_cos_bit);
    butterfly_dct_post(buf0 + 16, buf0 + 16, buf1 + 16, 4);
    butterfly_dct_post(buf0 + 20, buf0 + 20, buf1 + 20, 4);
    butterfly_dct_post(buf0 + 24, buf0 + 24, buf1 + 24, 4);
    butterfly_dct_post(buf0 + 28, buf0 + 28, buf1 + 28, 4);

    // stage 8
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
    butterfly_0112_neon(cospi, 2, buf1[31], buf1[16], &buf0[16], &buf0[31], v_cos_bit);
    butterfly_0130_neon(cospi, 30, buf1[17], buf1[30], &buf0[17], &buf0[30], v_cos_bit);
    butterfly_0112_neon(cospi, 18, buf1[29], buf1[18], &buf0[18], &buf0[29], v_cos_bit);
    butterfly_0130_neon(cospi, 14, buf1[19], buf1[28], &buf0[19], &buf0[28], v_cos_bit);
    butterfly_0112_neon(cospi, 10, buf1[27], buf1[20], &buf0[20], &buf0[27], v_cos_bit);
    butterfly_0130_neon(cospi, 22, buf1[21], buf1[26], &buf0[21], &buf0[26], v_cos_bit);
    butterfly_0112_neon(cospi, 26, buf1[25], buf1[22], &buf0[22], &buf0[25], v_cos_bit);
    butterfly_0130_neon(cospi, 6, buf1[23], buf1[24], &buf0[23], &buf0[24], v_cos_bit);

    // stage 9
    output[0]  = buf0[0];
    output[1]  = buf0[16];
    output[2]  = buf0[8];
    output[3]  = buf0[24];
    output[4]  = buf0[4];
    output[5]  = buf0[20];
    output[6]  = buf0[12];
    output[7]  = buf0[28];
    output[8]  = buf0[2];
    output[9]  = buf0[18];
    output[10] = buf0[10];
    output[11] = buf0[26];
    output[12] = buf0[6];
    output[13] = buf0[22];
    output[14] = buf0[14];
    output[15] = buf0[30];
    output[16] = buf0[1];
    output[17] = buf0[17];
    output[18] = buf0[9];
    output[19] = buf0[25];
    output[20] = buf0[5];
    output[21] = buf0[21];
    output[22] = buf0[13];
    output[23] = buf0[29];
    output[24] = buf0[3];
    output[25] = buf0[19];
    output[26] = buf0[11];
    output[27] = buf0[27];
    output[28] = buf0[7];
    output[29] = buf0[23];
    output[30] = buf0[15];
    output[31] = buf0[31];
}

static void highbd_fidentity32_x4_neon(const int32x4_t* input, int32x4_t* output, int cos_bit) {
    (void)cos_bit;
    for (int i = 0; i < 32; i++) {
        output[i] = vshlq_n_s32(input[i], 2);
    }
}

TRANSFORM_COL_MANY(fdct32, 32)
TRANSFORM_COL_MANY(fidentity32, 32)

static const fwd_transform_1d_col_many_neon col_highbd_txfm32_xn_arr[TX_TYPES] = {
    highbd_fdct32_col_many_neon, // DCT_DCT
    NULL, // ADST_DCT
    NULL, // DCT_ADST
    NULL, // ADST_ADST
    NULL, // FLIPADST_DCT
    NULL, // DCT_FLIPADST
    NULL, // FLIPADST_FLIPADST
    NULL, // ADST_FLIPADST
    NULL, // FLIPADST_ADST
    highbd_fidentity32_col_many_neon, // IDTX
    highbd_fdct32_col_many_neon, // V_DCT
    highbd_fidentity32_col_many_neon, // H_DCT
    NULL, // V_ADST
    NULL, // H_ADST
    NULL, // V_FLIPADST
    NULL // H_FLIPADST
};

void svt_av1_fwd_txfm2d_8x32_neon(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    const fwd_transform_1d_col_many_neon col_txfm = col_highbd_txfm32_xn_arr[tx_type];
    const fwd_transform_1d_row_many_neon row_txfm = row_highbd_txfm8_xn_arr[tx_type];
    int                                  bitcol   = fwd_cos_bit_col[1][3];
    int                                  bitrow   = fwd_cos_bit_row[1][3];

    // Column-wise transform.
    int32x4_t buf0[64];
    col_txfm(input,
             buf0,
             stride,
             bitcol,
             /*lr_flip=*/0,
             /*howmany=*/2,
             /*hm_stride=*/32);
    shift_right_2_round_s32_x4(buf0, buf0, 64);

    int32x4_t buf1[64];
    transpose_arrays_s32_8x32(buf0, buf1);

    // Row-wise transform.
    row_txfm(buf1, buf0, bitrow, /*howmany=*/8, /*hm_stride=*/8);
    transpose_8xh(buf0, (int32x4_t*)coeff, 64);
}

static const fwd_transform_1d_row_neon row_highbd_txfm16_xn_arr[TX_TYPES] = {
    highbd_fdct16_row_neon, // DCT_DCT
    highbd_fdct16_row_neon, // ADST_DCT
    highbd_fadst16_row_neon, // DCT_ADST
    highbd_fadst16_row_neon, // ADST_ADST
    highbd_fdct16_row_neon, // FLIPADST_DCT
    highbd_fadst16_row_neon, // DCT_FLIPADST
    highbd_fadst16_row_neon, // FLIPADST_FLIPADST
    highbd_fadst16_row_neon, // ADST_FLIPADST
    highbd_fadst16_row_neon, // FLIPADST_ADST
    highbd_fidentity16_row_neon, // IDTX
    highbd_fidentity16_row_neon, // V_DCT
    highbd_fdct16_row_neon, // H_DCT
    highbd_fidentity16_row_neon, // V_ADST
    highbd_fadst16_row_neon, // H_ADST
    highbd_fidentity16_row_neon, // V_FLIPADST
    highbd_fadst16_row_neon // H_FLIPADST
};

static inline void transpose_elems_s32_16x4(const int32x4_t* in, int32x4_t* out) {
    transpose_elems_s32_4x4(in[0], in[1], in[2], in[3], &out[0], &out[4], &out[8], &out[12]);
    transpose_elems_s32_4x4(in[4], in[5], in[6], in[7], &out[1], &out[5], &out[9], &out[13]);
    transpose_elems_s32_4x4(in[8], in[9], in[10], in[11], &out[2], &out[6], &out[10], &out[14]);
    transpose_elems_s32_4x4(in[12], in[13], in[14], in[15], &out[3], &out[7], &out[11], &out[15]);
}

static inline void transpose_16xh(const int32x4_t* in, int32x4_t* out, int n) {
    for (int i = 0; i < n; i += 16) {
        transpose_elems_s32_16x4(in + i, out + i);
    }
}

void svt_av1_fwd_txfm2d_16x4_neon(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    int                                  bitcol   = fwd_cos_bit_col[2][0];
    int                                  bitrow   = fwd_cos_bit_row[2][0];
    const fwd_transform_1d_col_many_neon col_txfm = col_highbd_txfm4_xn_arr[tx_type];
    const fwd_transform_1d_row_neon      row_txfm = row_highbd_txfm16_xn_arr[tx_type];

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 4);

    // Column-wise transform.
    int32x4_t buf0[16];
    if (lr_flip) {
        col_txfm(input,
                 buf0 + 3 * 4,
                 stride,
                 bitcol,
                 /*lr_flip=*/1,
                 /*howmany=*/4,
                 /*hm_stride=*/-4);
    } else {
        col_txfm(input,
                 buf0,
                 stride,
                 bitcol,
                 /*lr_flip=*/0,
                 /*howmany=*/4,
                 /*hm_stride=*/4);
    }

    shift_right_1_round_s32_x4(buf0, buf0, 16);
    transpose_arrays_s32_4x16(buf0, buf0);

    int32x4_t buf1[16];
    // Row-wise transform.
    row_txfm(buf0, buf1, bitrow);

    transpose_16xh(buf1, (int32x4_t*)coeff, 16);
}

static const fwd_transform_1d_col_many_neon col_highbd_txfm8_xn_arr[TX_TYPES] = {
    highbd_fdct8_col_many_neon, // DCT_DCT
    highbd_fadst8_col_many_neon, // ADST_DCT
    highbd_fdct8_col_many_neon, // DCT_ADST
    highbd_fadst8_col_many_neon, // ADST_ADST
    highbd_fadst8_col_many_neon, // FLIPADST_DCT
    highbd_fdct8_col_many_neon, // DCT_FLIPADST
    highbd_fadst8_col_many_neon, // FLIPADST_FLIPADST
    highbd_fadst8_col_many_neon, // ADST_FLIPADST
    highbd_fadst8_col_many_neon, // FLIPADST_ADST
    highbd_fidentity8_col_many_neon, // IDTX
    highbd_fdct8_col_many_neon, // V_DCT
    highbd_fidentity8_col_many_neon, // H_DCT
    highbd_fadst8_col_many_neon, // V_ADST
    highbd_fidentity8_col_many_neon, // H_ADST
    highbd_fadst8_col_many_neon, // V_FLIPADST
    highbd_fidentity8_col_many_neon // H_FLIPADST
};

static const fwd_transform_1d_row_many_neon row_rect_highbd_txfm16_xn_arr[TX_TYPES] = {
    highbd_fdct16_row_rect_many_neon, // DCT_DCT
    highbd_fdct16_row_rect_many_neon, // ADST_DCT
    highbd_fadst16_row_rect_many_neon, // DCT_ADST
    highbd_fadst16_row_rect_many_neon, // ADST_ADST
    highbd_fdct16_row_rect_many_neon, // FLIPADST_DCT
    highbd_fadst16_row_rect_many_neon, // DCT_FLIPADST
    highbd_fadst16_row_rect_many_neon, // FLIPADST_FLIPADST
    highbd_fadst16_row_rect_many_neon, // ADST_FLIPADST
    highbd_fadst16_row_rect_many_neon, // FLIPADST_ADST
    highbd_fidentity16_row_rect_many_neon, // IDTX
    highbd_fidentity16_row_rect_many_neon, // V_DCT
    highbd_fdct16_row_rect_many_neon, // H_DCT
    highbd_fidentity16_row_rect_many_neon, // V_ADST
    highbd_fadst16_row_rect_many_neon, // H_ADST
    highbd_fidentity16_row_rect_many_neon, // V_FLIPADST
    highbd_fadst16_row_rect_many_neon // H_FLIPADST
};

void svt_av1_fwd_txfm2d_16x8_neon(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    const fwd_transform_1d_col_many_neon col_txfm = col_highbd_txfm8_xn_arr[tx_type];
    const fwd_transform_1d_row_many_neon row_txfm = row_rect_highbd_txfm16_xn_arr[tx_type];
    int                                  bit      = fwd_cos_bit_col[2][1];

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 8);

    // Column-wise transform.
    int32x4_t buf0[32];
    if (lr_flip) {
        col_txfm(input,
                 buf0 + 3 * 8,
                 stride,
                 bit,
                 /*lr_flip=*/1,
                 /*howmany=*/4,
                 /*hm_stride=*/-8);
    } else {
        col_txfm(input,
                 buf0,
                 stride,
                 bit,
                 /*lr_flip=*/0,
                 /*howmany=*/4,
                 /*hm_stride=*/8);
    }
    shift_right_2_round_s32_x4(buf0, buf0, 32);

    int32x4_t buf1[32];
    transpose_arrays_s32_16x8(buf0, buf1);

    // Row-wise transform.
    row_txfm(buf1, buf0, bit, /*howmany=*/2, /*hm_stride=*/16);

    transpose_16xh(buf0, (int32x4_t*)coeff, 32);
}

void svt_av1_fwd_txfm2d_16x32_neon(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    const fwd_transform_1d_col_many_neon col_txfm = col_highbd_txfm32_xn_arr[tx_type];
    const fwd_transform_1d_row_many_neon row_txfm = row_rect_highbd_txfm16_xn_arr[tx_type];
    int                                  bitcol   = fwd_cos_bit_col[2][3];
    int                                  bitrow   = fwd_cos_bit_row[2][3];

    // Column-wise transform.
    int32x4_t buf0[128];
    col_txfm(input,
             buf0,
             stride,
             bitcol,
             /*lr_flip=*/0,
             /*howmany=*/4,
             /*hm_stride=*/32);
    shift_right_4_round_s32_x4(buf0, buf0, 128);

    int32x4_t buf1[128];
    transpose_arrays_s32_16x32(buf0, buf1);

    // Row-wise transform.
    row_txfm(buf1, buf0, bitrow, /*howmany=*/8, /*hm_stride=*/16);
    transpose_16xh(buf0, (int32x4_t*)coeff, 128);
}

static void highbd_fdct64_x4_neon(const int32x4_t* input, int32x4_t* output, int8_t cos_bit) {
    const int32_t* const cospi     = cospi_arr_s32(cos_bit);
    const int32x4_t      v_cos_bit = vdupq_n_s32(-cos_bit);

    // stage 1
    int32x4_t x1[64];
    butterfly_dct_pre(input, x1, 64);

    // stage 2
    int32x4_t x2[64];
    butterfly_dct_pre(x1, x2, 32);
    x2[32] = x1[32];
    x2[33] = x1[33];
    x2[34] = x1[34];
    x2[35] = x1[35];
    x2[36] = x1[36];
    x2[37] = x1[37];
    x2[38] = x1[38];
    x2[39] = x1[39];
    butterfly_0112_neon(cospi, 32, x1[55], x1[40], &x2[55], &x2[40], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x1[54], x1[41], &x2[54], &x2[41], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x1[53], x1[42], &x2[53], &x2[42], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x1[52], x1[43], &x2[52], &x2[43], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x1[51], x1[44], &x2[51], &x2[44], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x1[50], x1[45], &x2[50], &x2[45], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x1[49], x1[46], &x2[49], &x2[46], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x1[48], x1[47], &x2[48], &x2[47], v_cos_bit);
    x2[56] = x1[56];
    x2[57] = x1[57];
    x2[58] = x1[58];
    x2[59] = x1[59];
    x2[60] = x1[60];
    x2[61] = x1[61];
    x2[62] = x1[62];
    x2[63] = x1[63];

    // stage 3
    int32x4_t x3[64];
    butterfly_dct_pre(x2, x3, 16);
    x3[16] = x2[16];
    x3[17] = x2[17];
    x3[18] = x2[18];
    x3[19] = x2[19];
    butterfly_0112_neon(cospi, 32, x2[27], x2[20], &x3[27], &x3[20], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x2[26], x2[21], &x3[26], &x3[21], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x2[25], x2[22], &x3[25], &x3[22], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x2[24], x2[23], &x3[24], &x3[23], v_cos_bit);
    x3[28] = x2[28];
    x3[29] = x2[29];
    x3[30] = x2[30];
    x3[31] = x2[31];
    butterfly_dct_post(x2 + 32, x2 + 32, x3 + 32, 32);

    // stage 4
    int32x4_t x4[64];
    butterfly_dct_pre(x3, x4, 8);
    x4[8] = x3[8];
    x4[9] = x3[9];
    butterfly_0112_neon(cospi, 32, x3[13], x3[10], &x4[13], &x4[10], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x3[12], x3[11], &x4[12], &x4[11], v_cos_bit);
    x4[14] = x3[14];
    x4[15] = x3[15];
    butterfly_dct_post(x3 + 16, x3 + 16, x4 + 16, 16);
    x4[32] = x3[32];
    x4[33] = x3[33];
    x4[34] = x3[34];
    x4[35] = x3[35];
    butterfly_0112_neon(cospi, 16, x3[59], x3[36], &x4[59], &x4[36], v_cos_bit);
    butterfly_0112_neon(cospi, 16, x3[58], x3[37], &x4[58], &x4[37], v_cos_bit);
    butterfly_0112_neon(cospi, 16, x3[57], x3[38], &x4[57], &x4[38], v_cos_bit);
    butterfly_0112_neon(cospi, 16, x3[56], x3[39], &x4[56], &x4[39], v_cos_bit);
    butterfly_2312_neon(cospi, 16, x3[55], x3[40], &x4[40], &x4[55], v_cos_bit);
    butterfly_2312_neon(cospi, 16, x3[54], x3[41], &x4[41], &x4[54], v_cos_bit);
    butterfly_2312_neon(cospi, 16, x3[53], x3[42], &x4[42], &x4[53], v_cos_bit);
    butterfly_2312_neon(cospi, 16, x3[52], x3[43], &x4[43], &x4[52], v_cos_bit);
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
    int32x4_t x5[64];
    butterfly_dct_pre(x4, x5, 4);
    x5[4] = x4[4];
    butterfly_0112_neon(cospi, 32, x4[6], x4[5], &x5[6], &x5[5], v_cos_bit);
    x5[7] = x4[7];
    butterfly_dct_post(x4 + 8, x4 + 8, x5 + 8, 8);
    x5[16] = x4[16];
    x5[17] = x4[17];
    butterfly_0112_neon(cospi, 16, x4[29], x4[18], &x5[29], &x5[18], v_cos_bit);
    butterfly_0112_neon(cospi, 16, x4[28], x4[19], &x5[28], &x5[19], v_cos_bit);
    butterfly_2312_neon(cospi, 16, x4[27], x4[20], &x5[20], &x5[27], v_cos_bit);
    butterfly_2312_neon(cospi, 16, x4[26], x4[21], &x5[21], &x5[26], v_cos_bit);
    x5[22] = x4[22];
    x5[23] = x4[23];
    x5[24] = x4[24];
    x5[25] = x4[25];
    x5[30] = x4[30];
    x5[31] = x4[31];
    butterfly_dct_post(x4 + 32, x4 + 32, x5 + 32, 16);
    butterfly_dct_post(x4 + 48, x4 + 48, x5 + 48, 16);

    // stage 6
    int32x4_t x6[64];
    butterfly_0112_neon(cospi, 32, x5[0], x5[1], &x6[0], &x6[1], v_cos_bit);
    butterfly_0112_neon(cospi, 16, x5[3], x5[2], &x6[2], &x6[3], v_cos_bit);
    butterfly_dct_post(x5 + 4, x5 + 4, x6 + 4, 4);
    x6[8] = x5[8];
    butterfly_0112_neon(cospi, 16, x5[14], x5[9], &x6[14], &x6[9], v_cos_bit);
    butterfly_2312_neon(cospi, 16, x5[13], x5[10], &x6[10], &x6[13], v_cos_bit);
    x6[11] = x5[11];
    x6[12] = x5[12];
    x6[15] = x5[15];
    butterfly_dct_post(x5 + 16, x5 + 16, x6 + 16, 8);
    butterfly_dct_post(x5 + 24, x5 + 24, x6 + 24, 8);
    x6[32] = x5[32];
    x6[33] = x5[33];
    butterfly_0112_neon(cospi, 8, x5[61], x5[34], &x6[61], &x6[34], v_cos_bit);
    butterfly_0112_neon(cospi, 8, x5[60], x5[35], &x6[60], &x6[35], v_cos_bit);
    butterfly_2312_neon(cospi, 8, x5[59], x5[36], &x6[36], &x6[59], v_cos_bit);
    butterfly_2312_neon(cospi, 8, x5[58], x5[37], &x6[37], &x6[58], v_cos_bit);
    x6[38] = x5[38];
    x6[39] = x5[39];
    x6[40] = x5[40];
    x6[41] = x5[41];
    butterfly_0130_neon(cospi, 24, x5[42], x5[53], &x6[53], &x6[42], v_cos_bit);
    butterfly_0130_neon(cospi, 24, x5[43], x5[52], &x6[52], &x6[43], v_cos_bit);
    butterfly_0332_neon(cospi, 24, x5[51], x5[44], &x6[51], &x6[44], v_cos_bit);
    butterfly_0332_neon(cospi, 24, x5[50], x5[45], &x6[50], &x6[45], v_cos_bit);
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
    int32x4_t x7[64];
    x7[0] = x6[0];
    x7[1] = x6[1];
    x7[2] = x6[2];
    x7[3] = x6[3];
    butterfly_0112_neon(cospi, 8, x6[7], x6[4], &x7[4], &x7[7], v_cos_bit);
    butterfly_0130_neon(cospi, 24, x6[5], x6[6], &x7[5], &x7[6], v_cos_bit);
    butterfly_dct_post(x6 + 8, x6 + 8, x7 + 8, 4);
    butterfly_dct_post(x6 + 12, x6 + 12, x7 + 12, 4);
    x7[16] = x6[16];
    butterfly_0112_neon(cospi, 8, x6[30], x6[17], &x7[30], &x7[17], v_cos_bit);
    butterfly_2312_neon(cospi, 8, x6[29], x6[18], &x7[18], &x7[29], v_cos_bit);
    x7[19] = x6[19];
    x7[20] = x6[20];
    butterfly_0130_neon(cospi, 24, x6[21], x6[26], &x7[26], &x7[21], v_cos_bit);
    butterfly_0332_neon(cospi, 24, x6[25], x6[22], &x7[25], &x7[22], v_cos_bit);
    x7[23] = x6[23];
    x7[24] = x6[24];
    x7[27] = x6[27];
    x7[28] = x6[28];
    x7[31] = x6[31];
    butterfly_dct_post(x6 + 32, x6 + 32, x7 + 32, 8);
    butterfly_dct_post(x6 + 40, x6 + 40, x7 + 40, 8);
    butterfly_dct_post(x6 + 48, x6 + 48, x7 + 48, 8);
    butterfly_dct_post(x6 + 56, x6 + 56, x7 + 56, 8);

    // stage 8
    int32x4_t x8[64];
    x8[0] = x7[0];
    x8[1] = x7[1];
    x8[2] = x7[2];
    x8[3] = x7[3];
    x8[4] = x7[4];
    x8[5] = x7[5];
    x8[6] = x7[6];
    x8[7] = x7[7];

    butterfly_0112_neon(cospi, 4, x7[15], x7[8], &x8[8], &x8[15], v_cos_bit);
    butterfly_0130_neon(cospi, 28, x7[9], x7[14], &x8[9], &x8[14], v_cos_bit);
    butterfly_0112_neon(cospi, 20, x7[13], x7[10], &x8[10], &x8[13], v_cos_bit);
    butterfly_0130_neon(cospi, 12, x7[11], x7[12], &x8[11], &x8[12], v_cos_bit);
    butterfly_dct_post(x7 + 16, x7 + 16, x8 + 16, 4);
    butterfly_dct_post(x7 + 20, x7 + 20, x8 + 20, 4);
    butterfly_dct_post(x7 + 24, x7 + 24, x8 + 24, 4);
    butterfly_dct_post(x7 + 28, x7 + 28, x8 + 28, 4);
    x8[32] = x7[32];
    butterfly_0112_neon(cospi, 4, x7[62], x7[33], &x8[62], &x8[33], v_cos_bit);
    butterfly_2312_neon(cospi, 4, x7[61], x7[34], &x8[34], &x8[61], v_cos_bit);
    x8[35] = x7[35];
    x8[36] = x7[36];
    butterfly_0130_neon(cospi, 28, x7[37], x7[58], &x8[58], &x8[37], v_cos_bit);
    butterfly_0332_neon(cospi, 28, x7[57], x7[38], &x8[57], &x8[38], v_cos_bit);
    x8[39] = x7[39];
    x8[40] = x7[40];
    butterfly_0112_neon(cospi, 20, x7[54], x7[41], &x8[54], &x8[41], v_cos_bit);
    butterfly_2312_neon(cospi, 20, x7[53], x7[42], &x8[42], &x8[53], v_cos_bit);
    x8[43] = x7[43];
    x8[44] = x7[44];
    butterfly_0130_neon(cospi, 12, x7[45], x7[50], &x8[50], &x8[45], v_cos_bit);
    butterfly_0332_neon(cospi, 12, x7[49], x7[46], &x8[49], &x8[46], v_cos_bit);
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
    int32x4_t x9[64];
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
    butterfly_0112_neon(cospi, 2, x8[31], x8[16], &x9[16], &x9[31], v_cos_bit);
    butterfly_0130_neon(cospi, 30, x8[17], x8[30], &x9[17], &x9[30], v_cos_bit);
    butterfly_0112_neon(cospi, 18, x8[29], x8[18], &x9[18], &x9[29], v_cos_bit);
    butterfly_0130_neon(cospi, 14, x8[19], x8[28], &x9[19], &x9[28], v_cos_bit);
    butterfly_0112_neon(cospi, 10, x8[27], x8[20], &x9[20], &x9[27], v_cos_bit);
    butterfly_0130_neon(cospi, 22, x8[21], x8[26], &x9[21], &x9[26], v_cos_bit);
    butterfly_0112_neon(cospi, 26, x8[25], x8[22], &x9[22], &x9[25], v_cos_bit);
    butterfly_0130_neon(cospi, 6, x8[23], x8[24], &x9[23], &x9[24], v_cos_bit);
    butterfly_dct_post(x8 + 32, x8 + 32, x9 + 32, 4);
    butterfly_dct_post(x8 + 36, x8 + 36, x9 + 36, 4);
    butterfly_dct_post(x8 + 40, x8 + 40, x9 + 40, 4);
    butterfly_dct_post(x8 + 44, x8 + 44, x9 + 44, 4);
    butterfly_dct_post(x8 + 48, x8 + 48, x9 + 48, 4);
    butterfly_dct_post(x8 + 52, x8 + 52, x9 + 52, 4);
    butterfly_dct_post(x8 + 56, x8 + 56, x9 + 56, 4);
    butterfly_dct_post(x8 + 60, x8 + 60, x9 + 60, 4);

    // stage 10
    int32x4_t x10[64];
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
    butterfly_0112_neon(cospi, 1, x9[63], x9[32], &x10[32], &x10[63], v_cos_bit);
    butterfly_0130_neon(cospi, 31, x9[33], x9[62], &x10[33], &x10[62], v_cos_bit);
    butterfly_0112_neon(cospi, 17, x9[61], x9[34], &x10[34], &x10[61], v_cos_bit);
    butterfly_0130_neon(cospi, 15, x9[35], x9[60], &x10[35], &x10[60], v_cos_bit);
    butterfly_0112_neon(cospi, 9, x9[59], x9[36], &x10[36], &x10[59], v_cos_bit);
    butterfly_0130_neon(cospi, 23, x9[37], x9[58], &x10[37], &x10[58], v_cos_bit);
    butterfly_0112_neon(cospi, 25, x9[57], x9[38], &x10[38], &x10[57], v_cos_bit);
    butterfly_0130_neon(cospi, 7, x9[39], x9[56], &x10[39], &x10[56], v_cos_bit);
    butterfly_0112_neon(cospi, 5, x9[55], x9[40], &x10[40], &x10[55], v_cos_bit);
    butterfly_0130_neon(cospi, 27, x9[41], x9[54], &x10[41], &x10[54], v_cos_bit);
    butterfly_0112_neon(cospi, 21, x9[53], x9[42], &x10[42], &x10[53], v_cos_bit);
    butterfly_0130_neon(cospi, 11, x9[43], x9[52], &x10[43], &x10[52], v_cos_bit);
    butterfly_0112_neon(cospi, 13, x9[51], x9[44], &x10[44], &x10[51], v_cos_bit);
    butterfly_0130_neon(cospi, 19, x9[45], x9[50], &x10[45], &x10[50], v_cos_bit);
    butterfly_0112_neon(cospi, 29, x9[49], x9[46], &x10[46], &x10[49], v_cos_bit);
    butterfly_0130_neon(cospi, 3, x9[47], x9[48], &x10[47], &x10[48], v_cos_bit);

    // stage 11
    output[0]  = x10[0];
    output[1]  = x10[32];
    output[2]  = x10[16];
    output[3]  = x10[48];
    output[4]  = x10[8];
    output[5]  = x10[40];
    output[6]  = x10[24];
    output[7]  = x10[56];
    output[8]  = x10[4];
    output[9]  = x10[36];
    output[10] = x10[20];
    output[11] = x10[52];
    output[12] = x10[12];
    output[13] = x10[44];
    output[14] = x10[28];
    output[15] = x10[60];
    output[16] = x10[2];
    output[17] = x10[34];
    output[18] = x10[18];
    output[19] = x10[50];
    output[20] = x10[10];
    output[21] = x10[42];
    output[22] = x10[26];
    output[23] = x10[58];
    output[24] = x10[6];
    output[25] = x10[38];
    output[26] = x10[22];
    output[27] = x10[54];
    output[28] = x10[14];
    output[29] = x10[46];
    output[30] = x10[30];
    output[31] = x10[62];
    output[32] = x10[1];
    output[33] = x10[33];
    output[34] = x10[17];
    output[35] = x10[49];
    output[36] = x10[9];
    output[37] = x10[41];
    output[38] = x10[25];
    output[39] = x10[57];
    output[40] = x10[5];
    output[41] = x10[37];
    output[42] = x10[21];
    output[43] = x10[53];
    output[44] = x10[13];
    output[45] = x10[45];
    output[46] = x10[29];
    output[47] = x10[61];
    output[48] = x10[3];
    output[49] = x10[35];
    output[50] = x10[19];
    output[51] = x10[51];
    output[52] = x10[11];
    output[53] = x10[43];
    output[54] = x10[27];
    output[55] = x10[59];
    output[56] = x10[7];
    output[57] = x10[39];
    output[58] = x10[23];
    output[59] = x10[55];
    output[60] = x10[15];
    output[61] = x10[47];
    output[62] = x10[31];
    output[63] = x10[63];
}

void svt_av1_fwd_txfm2d_16x64_neon(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    const int bitcol = fwd_cos_bit_col[2][4];
    const int bitrow = fwd_cos_bit_row[2][4];

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 64);

    // Column-wise transform.
    int32x4_t buf0[256];
    load_buffer_16x64(input, buf0, stride, lr_flip);
    for (int i = 0; i < 4; i++) {
        highbd_fdct64_x4_neon(buf0 + i * 64, buf0 + i * 64, bitcol);
    }
    shift_right_2_round_s32_x4(buf0, buf0, 256);

    int32x4_t buf1[256];
    transpose_arrays_s32_16x64(buf0, buf1);

    // Row-wise transform.
    highbd_fdct16_xn_neon(buf1, buf1, bitrow, 16);
    transpose_16xh(buf1, (int32x4_t*)coeff, 256);
}

TRANSFORM_ROW_MANY(fdct32, 32)
TRANSFORM_ROW_MANY(fidentity32, 32)

static const fwd_transform_1d_row_many_neon row_highbd_txfm32_x4_arr[TX_TYPES] = {
    highbd_fdct32_row_many_neon, // DCT_DCT
    NULL, // ADST_DCT
    NULL, // DCT_ADST
    NULL, // ADST_ADST
    NULL, // FLIPADST_DCT
    NULL, // DCT_FLIPADST
    NULL, // FLIPADST_FLIPADST
    NULL, // ADST_FLIPADST
    NULL, // FLIPADST_ADST
    highbd_fidentity32_row_many_neon, // IDTX
    highbd_fidentity32_row_many_neon, // V_DCT
    highbd_fdct32_row_many_neon, // H_DCT
    NULL, // V_ADST
    NULL, // H_ADST
    NULL, // V_FLIPADST
    NULL // H_FLIPADST
};

static inline void transpose_elems_s32_32x4(const int32x4_t* in, int32x4_t* out) {
    transpose_elems_s32_4x4(in[0], in[1], in[2], in[3], &out[0], &out[8], &out[16], &out[24]);
    transpose_elems_s32_4x4(in[4], in[5], in[6], in[7], &out[1], &out[9], &out[17], &out[25]);
    transpose_elems_s32_4x4(in[8], in[9], in[10], in[11], &out[2], &out[10], &out[18], &out[26]);
    transpose_elems_s32_4x4(in[12], in[13], in[14], in[15], &out[3], &out[11], &out[19], &out[27]);
    transpose_elems_s32_4x4(in[16], in[17], in[18], in[19], &out[4], &out[12], &out[20], &out[28]);
    transpose_elems_s32_4x4(in[20], in[21], in[22], in[23], &out[5], &out[13], &out[21], &out[29]);
    transpose_elems_s32_4x4(in[24], in[25], in[26], in[27], &out[6], &out[14], &out[22], &out[30]);
    transpose_elems_s32_4x4(in[28], in[29], in[30], in[31], &out[7], &out[15], &out[23], &out[31]);
}

static inline void transpose_32xh(const int32x4_t* in, int32x4_t* out, int n) {
    for (int i = 0; i < n; i += 32) {
        transpose_elems_s32_32x4(in + i, out + i);
    }
}

void svt_av1_fwd_txfm2d_32x8_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    const fwd_transform_1d_col_many_neon col_txfm = col_highbd_txfm8_xn_arr[tx_type];
    const fwd_transform_1d_row_many_neon row_txfm = row_highbd_txfm32_x4_arr[tx_type];
    int                                  bitcol   = fwd_cos_bit_col[3][1];
    int                                  bitrow   = fwd_cos_bit_row[3][1];

    // Column-wise transform.
    int32x4_t buf0[64];
    col_txfm(input,
             buf0,
             stride,
             bitcol,
             /*lr_flip=*/0,
             /*howmany=*/8,
             /*hm_stride=*/8);
    shift_right_2_round_s32_x4(buf0, buf0, 64);

    int32x4_t buf1[64];
    transpose_arrays_s32_32x8(buf0, buf1);

    // Row-wise transform.
    row_txfm(buf1, buf0, bitrow, /*howmany=*/2, /*hm_stride=*/32);
    transpose_32xh(buf0, (int32x4_t*)output, 64);
}

TRANSFORM_ROW_RECT_MANY(fdct32, 32)
TRANSFORM_ROW_RECT_MANY(fidentity32, 32)

static const fwd_transform_1d_row_many_neon row_rect_highbd_txfm32_x4_arr[TX_TYPES] = {
    highbd_fdct32_row_rect_many_neon, // DCT_DCT
    NULL, // ADST_DCT
    NULL, // DCT_ADST
    NULL, // ADST_ADST
    NULL, // FLIPADST_DCT
    NULL, // DCT_FLIPADST
    NULL, // FLIPADST_FLIPADST
    NULL, // ADST_FLIPADST
    NULL, // FLIPADST_ADST
    highbd_fidentity32_row_rect_many_neon, // IDTX
    NULL, // V_DCT
    NULL, // H_DCT
    NULL, // V_ADST
    NULL, // H_ADST
    NULL, // V_FLIPADST
    NULL // H_FLIPADST
};

void svt_av1_fwd_txfm2d_32x16_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    const fwd_transform_1d_col_many_neon col_txfm = col_highbd_txfm16_xn_arr[tx_type];
    const fwd_transform_1d_row_many_neon row_txfm = row_rect_highbd_txfm32_x4_arr[tx_type];
    int                                  bitcol   = fwd_cos_bit_col[3][2];
    int                                  bitrow   = fwd_cos_bit_row[3][2];

    // Column-wise transform.
    int32x4_t buf0[128];
    col_txfm(input,
             buf0,
             stride,
             bitcol,
             /*lr_flip=*/0,
             /*howmany=*/8,
             /*hm_stride=*/16);
    shift_right_4_round_s32_x4(buf0, buf0, 128);

    int32x4_t buf1[128];
    transpose_arrays_s32_32x16(buf0, buf1);

    // Row-wise transform.
    row_txfm(buf1, buf0, bitrow, /*howmany=*/4, /*hm_stride=*/32);
    transpose_32xh(buf0, (int32x4_t*)output, 128);
}

void svt_av1_fwd_txfm2d_32x32_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    const fwd_transform_1d_col_many_neon col_txfm = col_highbd_txfm32_xn_arr[tx_type];
    const fwd_transform_1d_row_many_neon row_txfm = row_highbd_txfm32_x4_arr[tx_type];

    // Column-wise transform.
    int32x4_t buf0[256];
    col_txfm(input,
             buf0,
             stride,
             /*cos_bit=*/12,
             /*lr_flip=*/0,
             /*howmany=*/8,
             /*hm_stride=*/32);
    shift_right_4_round_s32_x4(buf0, buf0, 256);

    int32x4_t buf1[256];
    transpose_arrays_s32_32x32(buf0, buf1);

    // Row-wise transform.
    row_txfm(buf1, buf0, /*cos_bit=*/12, /*howmany=*/8, /*hm_stride=*/32);
    transpose_32xh(buf0, (int32x4_t*)output, 256);
}

static AOM_FORCE_INLINE void round_shift2_rect_array_s32_neon(const int32x4_t* input, int32x4_t* output,
                                                              const int size) {
    const int32x4_t sqrt2 = vdupq_n_s32(new_sqrt2);
    int             i     = 0;
    do {
        const int32x4_t r0 = vrshrq_n_s32(input[i], 2);
        const int32x4_t r1 = vmulq_s32(r0, sqrt2);
        output[i]          = vrshrq_n_s32(r1, new_sqrt2_bits);
    } while (++i < size);
}

void svt_av1_fwd_txfm2d_32x64_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    (void)tx_type;
    int bitcol = fwd_cos_bit_col[3][4];
    int bitrow = fwd_cos_bit_row[3][4];

    // Column-wise transform.
    int32x4_t buf0[512];
    load_buffer_32x64(input, buf0, stride, 0);
    for (int i = 0; i < 8; i++) {
        highbd_fdct64_x4_neon(buf0 + i * 64, buf0 + i * 64, bitcol);
    }
    shift_right_2_round_s32_x4(buf0, buf0, 512);

    int32x4_t buf1[512];
    transpose_arrays_s32_32x64(buf0, buf1);

    // Row-wise transform.
    for (int i = 0; i < 16; i++) {
        highbd_fdct32_x4_neon(buf1 + i * 32, buf1 + i * 32, bitrow);
    }
    round_shift2_rect_array_s32_neon(buf1, buf1, 512);
    transpose_32xh(buf1, (int32x4_t*)output, 512);
}

static inline void transpose_elems_s32_64x4(const int32x4_t* in, int32x4_t* out) {
    transpose_elems_s32_4x4(in[0], in[1], in[2], in[3], &out[0], &out[16], &out[32], &out[48]);
    transpose_elems_s32_4x4(in[4], in[5], in[6], in[7], &out[1], &out[17], &out[33], &out[49]);
    transpose_elems_s32_4x4(in[8], in[9], in[10], in[11], &out[2], &out[18], &out[34], &out[50]);
    transpose_elems_s32_4x4(in[12], in[13], in[14], in[15], &out[3], &out[19], &out[35], &out[51]);
    transpose_elems_s32_4x4(in[16], in[17], in[18], in[19], &out[4], &out[20], &out[36], &out[52]);
    transpose_elems_s32_4x4(in[20], in[21], in[22], in[23], &out[5], &out[21], &out[37], &out[53]);
    transpose_elems_s32_4x4(in[24], in[25], in[26], in[27], &out[6], &out[22], &out[38], &out[54]);
    transpose_elems_s32_4x4(in[28], in[29], in[30], in[31], &out[7], &out[23], &out[39], &out[55]);
    transpose_elems_s32_4x4(in[32], in[33], in[34], in[35], &out[8], &out[24], &out[40], &out[56]);
    transpose_elems_s32_4x4(in[36], in[37], in[38], in[39], &out[9], &out[25], &out[41], &out[57]);
    transpose_elems_s32_4x4(in[40], in[41], in[42], in[43], &out[10], &out[26], &out[42], &out[58]);
    transpose_elems_s32_4x4(in[44], in[45], in[46], in[47], &out[11], &out[27], &out[43], &out[59]);
    transpose_elems_s32_4x4(in[48], in[49], in[50], in[51], &out[12], &out[28], &out[44], &out[60]);
    transpose_elems_s32_4x4(in[52], in[53], in[54], in[55], &out[13], &out[29], &out[45], &out[61]);
    transpose_elems_s32_4x4(in[56], in[57], in[58], in[59], &out[14], &out[30], &out[46], &out[62]);
    transpose_elems_s32_4x4(in[60], in[61], in[62], in[63], &out[15], &out[31], &out[47], &out[63]);
}

static inline void transpose_64xh(const int32x4_t* in, int32x4_t* out, int n) {
    for (int i = 0; i < n; i += 64) {
        transpose_elems_s32_64x4(in + i, out + i);
    }
}

void svt_av1_fwd_txfm2d_64x16_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    const int bitcol = fwd_cos_bit_col[4][2];
    const int bitrow = fwd_cos_bit_row[4][2];

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 16);

    // Column-wise transform.
    int32x4_t buf0[256];
    load_buffer_64x16(input, buf0, stride, lr_flip);
    highbd_fdct16_xn_neon(buf0, buf0, bitcol, 16);
    shift_right_4_round_s32_x4(buf0, buf0, 256);

    int32x4_t buf1[256];
    transpose_arrays_s32_64x16(buf0, buf1);

    // Row-wise transform.
    for (int i = 0; i < 4; i++) {
        highbd_fdct64_x4_neon(buf1 + i * 64, buf1 + i * 64, bitrow);
    }
    transpose_64xh(buf1, (int32x4_t*)output, 256);
}

void svt_av1_fwd_txfm2d_64x32_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    (void)tx_type;
    int bitcol = fwd_cos_bit_col[4][3];
    int bitrow = fwd_cos_bit_row[4][3];

    // Column-wise transform.
    int32x4_t buf0[512];
    load_buffer_64x32(input, buf0, stride, 0);
    for (int i = 0; i < 16; i++) {
        highbd_fdct32_x4_neon(buf0 + i * 32, buf0 + i * 32, bitcol);
    }
    shift_right_4_round_s32_x4(buf0, buf0, 512);

    int32x4_t buf1[512];
    transpose_arrays_s32_64x32(buf0, buf1);

    // Row-wise transform.
    for (int i = 0; i < 8; i++) {
        highbd_fdct64_x4_neon(buf1 + i * 64, buf1 + i * 64, bitrow);
    }
    round_shift2_rect_array_s32_neon(buf1, buf1, 512);
    transpose_64xh(buf1, (int32x4_t*)output, 512);
}

TRANSFORM_COL_MANY(fdct64, 64)
TRANSFORM_ROW_MANY(fdct64, 64)

static inline void load_buffer_64x64_neon(const int16_t* input, int32_t stride, int32x4_t* output) {
    int32_t i;

    for (i = 0; i < 64; ++i) {
        output[0]  = vmovl_s16(vld1_s16(input + 0 * 4));
        output[1]  = vmovl_s16(vld1_s16(input + 1 * 4));
        output[2]  = vmovl_s16(vld1_s16(input + 2 * 4));
        output[3]  = vmovl_s16(vld1_s16(input + 3 * 4));
        output[4]  = vmovl_s16(vld1_s16(input + 4 * 4));
        output[5]  = vmovl_s16(vld1_s16(input + 5 * 4));
        output[6]  = vmovl_s16(vld1_s16(input + 6 * 4));
        output[7]  = vmovl_s16(vld1_s16(input + 7 * 4));
        output[8]  = vmovl_s16(vld1_s16(input + 8 * 4));
        output[9]  = vmovl_s16(vld1_s16(input + 9 * 4));
        output[10] = vmovl_s16(vld1_s16(input + 10 * 4));
        output[11] = vmovl_s16(vld1_s16(input + 11 * 4));
        output[12] = vmovl_s16(vld1_s16(input + 12 * 4));
        output[13] = vmovl_s16(vld1_s16(input + 13 * 4));
        output[14] = vmovl_s16(vld1_s16(input + 14 * 4));
        output[15] = vmovl_s16(vld1_s16(input + 15 * 4));

        input += stride;
        output += 16;
    }
}

static inline void fidtx64x64_neon(int32x4_t* input, int32x4_t* output, const int8_t cos_bit,
                                   const int8_t* stage_range) {
    (void)cos_bit;
    (void)stage_range;
    const int32_t   bits    = 12; // new_sqrt2_bits = 12
    const int32_t   sqrt    = 4 * 5793; // 4 * new_sqrt2
    const int32_t   col_num = 16;
    const int32x4_t newsqrt = vdupq_n_s32(sqrt);

    const int32_t num_iters = 64 * col_num;
    for (int32_t i = 0; i < num_iters; i++) {
        int32x4_t temp = vmulq_s32(input[i], newsqrt);
        output[i]      = vrshlq_s32(temp, vdupq_n_s32(-bits));
    }
}

void svt_av1_fwd_txfm2d_64x64_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;

    switch (tx_type) {
    case DCT_DCT: {
        // Column-wise transform.
        int32x4_t buf0[1024];
        highbd_fdct64_col_many_neon(input, buf0, stride, 13, /*lr_flip=*/0, /*howmany=*/16, /*hm_stride=*/64);
        shift_right_2_round_s32_x4(buf0, buf0, 1024);

        int32x4_t buf1[1024];
        transpose_arrays_s32_64x64(buf0, buf1);

        // Row-wise transform.
        highbd_fdct64_row_many_neon(buf1, buf0, 10, /*howmany=*/16, /*hm_stride=*/64);
        shift_right_2_round_s32_x4(buf0, buf0, 1024);
        transpose_64xh(buf0, (int32x4_t*)output, 1024);
        break;
    }
    case IDTX: {
        int32x4_t buf0[1024];
        load_buffer_64x64_neon(input, stride, buf0);

        // Column-wise transform.
        int32x4_t buf1[1024];
        fidtx64x64_neon(buf0, buf1, 13, NULL);
        shift_right_2_round_s32_x4(buf1, buf1, 1024);

        // Row-wise transform.
        fidtx64x64_neon(buf1, buf0, 10, NULL);
        shift_right_2_round_s32_x4(buf0, (int32x4_t*)output, 1024);

        break;
    }
    default:
        assert(0);
    }
}

static AOM_FORCE_INLINE void highbd_fdct4_N4_x4_neon(const int32x4_t* in, int32x4_t* out, int bit) {
    const int32_t* const cospi = cospi_arr_s32(bit);

    const int32x4_t a0 = vaddq_s32(in[0], in[3]);
    const int32x4_t a2 = vaddq_s32(in[1], in[2]);

    const int32x4_t b0 = vaddq_s32(a0, a2);
    const int32x4_t c0 = vmulq_n_s32(b0, cospi[2 * 32]);

    const int32x4_t v_bit = vdupq_n_s32(-bit);
    out[0]                = vrshlq_s32(c0, v_bit);
}

static AOM_FORCE_INLINE void highbd_fadst4_N4_x4_neon(const int32x4_t* in, int32x4_t* out, int bit) {
    const int32x4_t sinpi = vld1q_s32(sinpi_arr(bit) + 1);

    int32x4_t a = vmulq_lane_s32(in[0], vget_low_s32(sinpi), 0);
    a           = vmlaq_lane_s32(a, in[1], vget_low_s32(sinpi), 1);
    a           = vmlaq_lane_s32(a, in[3], vget_high_s32(sinpi), 1);

    a = vmlaq_lane_s32(a, in[2], vget_high_s32(sinpi), 0);

    const int32x4_t v_bit = vdupq_n_s32(-bit);
    out[0]                = vrshlq_s32(a, v_bit);
}

static AOM_FORCE_INLINE void highbd_fidentity4_N4_x4_neon(const int32x4_t* in, int32x4_t* out, int bit) {
    (void)bit;

    const int32x4_t a_low = vmulq_n_s32(in[0], new_sqrt2);
    out[0]                = vrshrq_n_s32(a_low, new_sqrt2_bits);
}

void svt_av1_fwd_txfm2d_4x4_N4_neon(int16_t* input, int32_t* output, uint32_t input_stride, TxType tx_type,
                                    uint8_t bd) {
    (void)bd;

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &input_stride, 4);

    // Workspace for column/row-wise transforms.
    int32x4_t buf[4];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fdct4_N4_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fdct4_N4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        output[0] = vgetq_lane_s32(buf[0], 0);
        break;
    case ADST_DCT:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fadst4_N4_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fdct4_N4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        output[0] = vgetq_lane_s32(buf[0], 0);
        break;
    case DCT_ADST:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fdct4_N4_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fadst4_N4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        output[0] = vgetq_lane_s32(buf[0], 0);
        break;
    case ADST_ADST:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fadst4_N4_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fadst4_N4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        output[0] = vgetq_lane_s32(buf[0], 0);
        break;
    case FLIPADST_DCT:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fadst4_N4_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fdct4_N4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        output[0] = vgetq_lane_s32(buf[0], 0);
        break;
    case DCT_FLIPADST:
        load_buffer_4x4(input, buf, input_stride, 1);
        highbd_fdct4_N4_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fadst4_N4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        output[0] = vgetq_lane_s32(buf[0], 0);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_4x4(input, buf, input_stride, 1);
        highbd_fadst4_N4_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fadst4_N4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        output[0] = vgetq_lane_s32(buf[0], 0);
        break;
    case ADST_FLIPADST:
        load_buffer_4x4(input, buf, input_stride, 1);
        highbd_fadst4_N4_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fadst4_N4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        output[0] = vgetq_lane_s32(buf[0], 0);
        break;
    case FLIPADST_ADST:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fadst4_N4_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fadst4_N4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        output[0] = vgetq_lane_s32(buf[0], 0);
        break;
    case IDTX:
        buf[0] = vshll_n_s16(vld1_s16(input), 2);
        highbd_fidentity4_N4_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        highbd_fidentity4_N4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        output[0] = vgetq_lane_s32(buf[0], 0);
        break;
    case V_DCT:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fdct4_N4_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        highbd_fidentity4_N4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        output[0] = vgetq_lane_s32(buf[0], 0);
        break;
    case H_DCT:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fidentity4_N4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fdct4_N4_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        output[0] = vgetq_lane_s32(buf[0], 0);
        break;
    case V_ADST:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fadst4_N4_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        highbd_fidentity4_N4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        output[0] = vgetq_lane_s32(buf[0], 0);
        break;
    case H_ADST:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fidentity4_N4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fadst4_N4_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        output[0] = vgetq_lane_s32(buf[0], 0);
        break;
    case V_FLIPADST:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fadst4_N4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        highbd_fidentity4_N4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        output[0] = vgetq_lane_s32(buf[0], 0);
        break;
    case H_FLIPADST:
        load_buffer_4x4(input, buf, input_stride, 1);
        highbd_fidentity4_N4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fadst4_N4_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        output[0] = vgetq_lane_s32(buf[0], 0);
        break;
    default:
        assert(0);
    }

    memset(output + 1, 0, 15 * 4);
}

static AOM_FORCE_INLINE void butterfly_dct_pre_half(const int32x4_t* input, int32x4_t* output, int n) {
    for (int i = 0; i < n / 2; ++i) {
        output[i] = vaddq_s32(input[i], input[n - i - 1]);
    }
}

static AOM_FORCE_INLINE void butterfly_dct_post_half(const int32x4_t* in0, const int32x4_t* in1, int32x4_t* output,
                                                     int n) {
    for (int i = 0; i < n / 4; ++i) {
        output[i] = vaddq_s32(in0[i], in1[n / 2 - i - 1]);
    }
    for (int i = 0; i < n / 4; ++i) {
        output[(3 * n) / 4 + i] = vaddq_s32(in0[(3 * n) / 4 + i], in1[(3 * n) / 4 - i - 1]);
    }
}

static AOM_FORCE_INLINE void highbd_fdct8_N4_x4_neon(const int32x4_t* in, int32x4_t* out, int bit) {
    const int32_t* const cospi = cospi_arr_s32(bit);
    const int32x4_t      v_bit = vdupq_n_s32(-bit);

    // stage 1
    int32x4_t a[8];
    butterfly_dct_pre(in, a, 8);

    // stage 2
    int32x4_t b[8];
    butterfly_dct_pre_half(a, b, 4);
    butterfly_0130_neon(cospi, 32, a[5], a[6], &b[6], &b[5], v_bit);

    // stage 3
    int32x4_t c[8];
    int32x2_t w01 = vld1_s32(cospi + 2 * 32);
    butterfly_half_neon(w01, 0, 1, b[1], b[0], &c[0], v_bit);
    butterfly_dct_post_half(a + 4, b + 4, c + 4, 4);

    // stage 4-5
    w01 = vld1_s32(cospi + 2 * 8);
    butterfly_half_neon(w01, 0, 1, c[7], c[4], &out[1], v_bit);

    out[0] = c[0];
}

static AOM_FORCE_INLINE void highbd_fadst8_N4_x4_neon(const int32x4_t* in, int32x4_t* out, int bit) {
    const int32_t* const cospi = cospi_arr_s32(bit);
    const int32x4_t      v_bit = vdupq_n_s32(-bit);

    int32x4_t u0, u1, u2, u3, u4, u5, u6, u7;
    int32x4_t v0, v1, v2, v3, v4, v5, v6, v7;

    // stage 0-1
    u0 = in[0];
    u1 = in[7];
    u2 = in[3];
    u3 = in[4];
    u4 = in[1];
    u5 = in[6];
    u6 = in[2];
    u7 = in[5];

    // stage 2
    v0 = u0;
    v1 = u1;
    butterfly_cospi32_0222_neon(cospi, u3, u2, &v2, &v3, v_bit);
    v4 = u4;
    v5 = u5;
    butterfly_cospi32_0002_neon(cospi, u6, u7, &v7, &v6, v_bit);

    // stage 3
    u0 = vaddq_s32(v0, v2);
    u1 = vsubq_s32(v3, v1);
    u2 = vsubq_s32(v0, v2);
    u3 = vaddq_s32(v1, v3);
    u4 = vsubq_s32(v6, v4);
    u5 = vaddq_s32(v5, v7);
    u6 = vaddq_s32(v4, v6);
    u7 = vsubq_s32(v5, v7);

    // stage 4
    v0 = u0;
    v1 = u1;
    v2 = u2;
    v3 = u3;

    butterfly_0112_neon(cospi, 16, u4, u5, &v4, &v5, v_bit);
    butterfly_0112_neon(cospi, 16, u7, u6, &v6, &v7, v_bit);

    // stage 5
    u0 = vaddq_s32(v0, v4);
    u1 = vaddq_s32(v1, v5);
    u6 = vsubq_s32(v2, v6);
    u7 = vaddq_s32(v3, v7);

    // stage 6
    int32x2_t w01 = vld1_s32(cospi + 2 * 4);
    butterfly_half_neon(w01, 1, 2, u0, u1, &v1, v_bit);
    w01 = vld1_s32(cospi + 2 * 12);
    butterfly_half_neon(w01, 1, 2, u6, u7, &v6, v_bit);

    // stage 7
    out[0] = v1;
    out[1] = v6;
}

static AOM_FORCE_INLINE void highbd_fidentity8_N4_x4_neon(const int32x4_t* in, int32x4_t* out, int bit) {
    (void)bit;
    out[0] = vshlq_n_s32(in[0], 1);
    out[1] = vshlq_n_s32(in[1], 1);
}

static AOM_FORCE_INLINE void highbd_fdct8_N4_xn_neon(const int32x4_t* in, int32x4_t* out, int bit, int howmany) {
    const int stride = 8;
    int       i      = 0;
    do {
        highbd_fdct8_N4_x4_neon(in + i * stride, out + i * stride, bit);
    } while (++i < howmany);
}

static AOM_FORCE_INLINE void highbd_fadst8_N4_xn_neon(const int32x4_t* in, int32x4_t* out, int bit, int howmany) {
    const int stride = 8;
    int       i      = 0;
    do {
        highbd_fadst8_N4_x4_neon(in + i * stride, out + i * stride, bit);
    } while (++i < howmany);
}

static AOM_FORCE_INLINE void highbd_fidentity8_N4_xn_neon(const int32x4_t* in, int32x4_t* out, int bit, int howmany) {
    (void)bit;
    const int stride = 8;
    int       i      = 0;
    do {
        highbd_fidentity8_N4_x4_neon(in + i * stride, out + i * stride, bit);
    } while (++i < howmany);
}

static inline void transpose_8x4_in_8x8(const int32x4_t* in, int32x4_t* out) {
    transpose_elems_s32_4x4(in[0], in[1], in[2], in[3], &out[0], &out[1], &out[2], &out[3]);
    transpose_elems_s32_4x4(in[8], in[9], in[10], in[11], &out[4], &out[5], &out[6], &out[7]);
}

static inline void write_buffer_8x8_N4(int32x2_t in0, int32x2_t in1, int32_t* out) {
    vst1_s32(out + 0, in0);
    vst1_s32(out + 2, vdup_n_s32(0));
    vst1q_s32(out + 4, vdupq_n_s32(0));

    vst1_s32(out + 8, in1);
    vst1_s32(out + 10, vdup_n_s32(0));
    vst1q_s32(out + 12, vdupq_n_s32(0));

    memset(out + 2 * 8, 0, 6 * 8 * sizeof(int32_t));
}

void svt_av1_fwd_txfm2d_8x8_N4_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 8);

    // Workspaces for column/row-wise transforms.
    int32x4_t   buf0[16], buf1[16];
    int32x2x2_t buf;

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_8x8(input, buf0, stride, 0);
        highbd_fdct8_N4_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_x4(buf0, buf0, 2);
        shift_right_1_round_s32_x4(buf0 + 8, buf0 + 8, 2);
        transpose_8x4_in_8x8(buf0, buf1);
        highbd_fdct8_N4_x4_neon(buf1, buf1, fwd_cos_bit_row[1][1]);
        buf = vtrn_s32(vget_low_s32(buf1[0]), vget_low_s32(buf1[1]));
        write_buffer_8x8_N4(buf.val[0], buf.val[1], output);
        break;
    case ADST_DCT:
        load_buffer_8x8(input, buf0, stride, 0);
        highbd_fadst8_N4_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_x4(buf0, buf0, 2);
        shift_right_1_round_s32_x4(buf0 + 8, buf0 + 8, 2);
        transpose_8x4_in_8x8(buf0, buf1);
        highbd_fdct8_N4_x4_neon(buf1, buf1, fwd_cos_bit_row[1][1]);
        buf = vtrn_s32(vget_low_s32(buf1[0]), vget_low_s32(buf1[1]));
        write_buffer_8x8_N4(buf.val[0], buf.val[1], output);
        break;
    case DCT_ADST:
        load_buffer_8x8(input, buf0, stride, 0);
        highbd_fdct8_N4_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_x4(buf0, buf0, 2);
        shift_right_1_round_s32_x4(buf0 + 8, buf0 + 8, 2);
        transpose_8x4_in_8x8(buf0, buf1);
        highbd_fadst8_N4_x4_neon(buf1, buf1, fwd_cos_bit_row[1][1]);
        buf = vtrn_s32(vget_low_s32(buf1[0]), vget_low_s32(buf1[1]));
        write_buffer_8x8_N4(buf.val[0], buf.val[1], output);
        break;
    case ADST_ADST:
        load_buffer_8x8(input, buf0, stride, 0);
        highbd_fadst8_N4_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_x4(buf0, buf0, 2);
        shift_right_1_round_s32_x4(buf0 + 8, buf0 + 8, 2);
        transpose_8x4_in_8x8(buf0, buf1);
        highbd_fadst8_N4_x4_neon(buf1, buf1, fwd_cos_bit_row[1][1]);
        buf = vtrn_s32(vget_low_s32(buf1[0]), vget_low_s32(buf1[1]));
        write_buffer_8x8_N4(buf.val[0], buf.val[1], output);
        break;
    case FLIPADST_DCT:
        load_buffer_8x8(input, buf0, stride, 0);
        highbd_fadst8_N4_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_x4(buf0, buf0, 2);
        shift_right_1_round_s32_x4(buf0 + 8, buf0 + 8, 2);
        transpose_8x4_in_8x8(buf0, buf1);
        highbd_fdct8_N4_x4_neon(buf1, buf1, fwd_cos_bit_row[1][1]);
        buf = vtrn_s32(vget_low_s32(buf1[0]), vget_low_s32(buf1[1]));
        write_buffer_8x8_N4(buf.val[0], buf.val[1], output);
        break;
    case DCT_FLIPADST:
        load_buffer_8x8(input, buf0, stride, 1);
        highbd_fdct8_N4_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_x4(buf0, buf0, 2);
        shift_right_1_round_s32_x4(buf0 + 8, buf0 + 8, 2);
        transpose_8x4_in_8x8(buf0, buf1);
        highbd_fadst8_N4_x4_neon(buf1, buf1, fwd_cos_bit_row[1][1]);
        buf = vtrn_s32(vget_low_s32(buf1[0]), vget_low_s32(buf1[1]));
        write_buffer_8x8_N4(buf.val[0], buf.val[1], output);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_8x8(input, buf0, stride, 1);
        highbd_fadst8_N4_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_x4(buf0, buf0, 2);
        shift_right_1_round_s32_x4(buf0 + 8, buf0 + 8, 2);
        transpose_8x4_in_8x8(buf0, buf1);
        highbd_fadst8_N4_x4_neon(buf1, buf1, fwd_cos_bit_row[1][1]);
        buf = vtrn_s32(vget_low_s32(buf1[0]), vget_low_s32(buf1[1]));
        write_buffer_8x8_N4(buf.val[0], buf.val[1], output);
        break;
    case ADST_FLIPADST:
        load_buffer_8x8(input, buf0, stride, 1);
        highbd_fadst8_N4_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_x4(buf0, buf0, 2);
        shift_right_1_round_s32_x4(buf0 + 8, buf0 + 8, 2);
        transpose_8x4_in_8x8(buf0, buf1);
        highbd_fadst8_N4_x4_neon(buf1, buf1, fwd_cos_bit_row[1][1]);
        buf = vtrn_s32(vget_low_s32(buf1[0]), vget_low_s32(buf1[1]));
        write_buffer_8x8_N4(buf.val[0], buf.val[1], output);
        break;
    case FLIPADST_ADST:
        load_buffer_8x8(input, buf0, stride, 0);
        highbd_fadst8_N4_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_x4(buf0, buf0, 2);
        shift_right_1_round_s32_x4(buf0 + 8, buf0 + 8, 2);
        transpose_8x4_in_8x8(buf0, buf1);
        highbd_fadst8_N4_x4_neon(buf1, buf1, fwd_cos_bit_row[1][1]);
        buf = vtrn_s32(vget_low_s32(buf1[0]), vget_low_s32(buf1[1]));
        write_buffer_8x8_N4(buf.val[0], buf.val[1], output);
        break;
    case IDTX:
        load_buffer_4x4(input, buf0, stride, 0);
        highbd_fidentity8_N4_x4_neon(buf0, buf0, fwd_cos_bit_col[1][1]);
        shift_right_1_round_s32_x4(buf0, buf0, 2);
        highbd_fidentity8_N4_x4_neon(buf0, buf0, fwd_cos_bit_col[1][1]);
        write_buffer_8x8_N4(vget_low_s32(buf0[0]), vget_low_s32(buf0[1]), output);
        break;
    case V_DCT:
        load_buffer_4x8(input, buf0, stride, 0);
        highbd_fdct8_N4_x4_neon(buf0, buf0, fwd_cos_bit_col[1][1]);
        shift_right_1_round_s32_x4(buf0, buf0, 2);
        highbd_fidentity8_N4_x4_neon(buf0, buf0, fwd_cos_bit_col[1][1]);
        write_buffer_8x8_N4(vget_low_s32(buf0[0]), vget_low_s32(buf0[1]), output);
        break;
    case H_DCT:
        load_buffer_4x4(input + 0 * 4, buf0 + 0 * 8, stride, 0);
        load_buffer_4x4(input + 1 * 4, buf0 + 1 * 8, stride, 0);
        highbd_fidentity8_N4_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_x4(buf0, buf0, 2);
        shift_right_1_round_s32_x4(buf0 + 8, buf0 + 8, 2);
        transpose_8x4_in_8x8(buf0, buf1);
        highbd_fdct8_N4_x4_neon(buf1, buf1, fwd_cos_bit_col[1][1]);
        buf = vtrn_s32(vget_low_s32(buf1[0]), vget_low_s32(buf1[1]));
        write_buffer_8x8_N4(buf.val[0], buf.val[1], output);
        break;
    case V_ADST:
        load_buffer_4x8(input, buf0, stride, 0);
        highbd_fadst8_N4_x4_neon(buf0, buf0, fwd_cos_bit_col[1][1]);
        shift_right_1_round_s32_x4(buf0, buf0, 2);
        highbd_fidentity8_N4_x4_neon(buf0, buf0, fwd_cos_bit_col[1][1]);
        write_buffer_8x8_N4(vget_low_s32(buf0[0]), vget_low_s32(buf0[1]), output);
        break;
    case H_ADST:
        load_buffer_4x4(input + 0 * 4, buf0 + 0 * 8, stride, 0);
        load_buffer_4x4(input + 1 * 4, buf0 + 1 * 8, stride, 0);
        highbd_fidentity8_N4_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_x4(buf0, buf0, 2);
        shift_right_1_round_s32_x4(buf0 + 8, buf0 + 8, 2);
        transpose_8x4_in_8x8(buf0, buf1);
        highbd_fadst8_N4_x4_neon(buf1, buf1, fwd_cos_bit_col[1][1]);
        buf = vtrn_s32(vget_low_s32(buf1[0]), vget_low_s32(buf1[1]));
        write_buffer_8x8_N4(buf.val[0], buf.val[1], output);
        break;
    case V_FLIPADST:
        load_buffer_4x8(input, buf0, stride, 0);
        highbd_fadst8_N4_x4_neon(buf0, buf0, fwd_cos_bit_col[1][1]);
        shift_right_1_round_s32_x4(buf0, buf0, 2);
        highbd_fidentity8_N4_x4_neon(buf0, buf0, fwd_cos_bit_col[1][1]);
        write_buffer_8x8_N4(vget_low_s32(buf0[0]), vget_low_s32(buf0[1]), output);
        break;
    case H_FLIPADST:
        load_buffer_4x4(input + 0 * 4, buf0 + 1 * 8, stride, 1);
        load_buffer_4x4(input + 1 * 4, buf0 + 0 * 8, stride, 1);
        highbd_fidentity8_N4_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_x4(buf0, buf0, 2);
        shift_right_1_round_s32_x4(buf0 + 8, buf0 + 8, 2);
        transpose_8x4_in_8x8(buf0, buf1);
        highbd_fadst8_N4_x4_neon(buf1, buf1, fwd_cos_bit_col[1][1]);
        buf = vtrn_s32(vget_low_s32(buf1[0]), vget_low_s32(buf1[1]));
        write_buffer_8x8_N4(buf.val[0], buf.val[1], output);
        break;
    default:
        assert(0);
    }
}

static inline void highbd_fdct16_N4_x4_neon(const int32x4_t* in, int32x4_t* out, int bit) {
    const int32_t* const cospi = cospi_arr_s32(bit);
    const int32x4_t      v_bit = vdupq_n_s32(-bit);

    int32x4_t u[16], v[16];

    // stage 1
    butterfly_dct_pre(in, u, 16);

    // stage 2
    butterfly_dct_pre(u, v, 8);
    v[8] = u[8];
    v[9] = u[9];
    butterfly_cospi32_0002_neon(cospi, u[13], u[10], &v[13], &v[10], v_bit);
    butterfly_cospi32_0002_neon(cospi, u[12], u[11], &v[12], &v[11], v_bit);
    v[14] = u[14];
    v[15] = u[15];

    // stage 3
    butterfly_dct_pre_half(v, u, 4);
    u[4] = v[4];
    butterfly_cospi32_0002_neon(cospi, v[6], v[5], &u[6], &u[5], v_bit);
    u[7] = v[7];
    butterfly_dct_post(v + 8, v + 8, u + 8, 8);

    // stage 4
    int32x2_t w01 = vld1_s32(cospi + 2 * 32);
    butterfly_half_neon(w01, 0, 0, u[0], u[1], &v[0], v_bit);
    butterfly_dct_post_half(u + 4, u + 4, v + 4, 4);
    v[8] = u[8];
    butterfly_0112_neon(cospi, 16, u[14], u[9], &v[14], &v[9], v_bit);
    butterfly_2312_neon(cospi, 16, u[13], u[10], &v[10], &v[13], v_bit);
    v[11] = u[11];
    v[12] = u[12];
    v[15] = u[15];

    // stage 5
    u[0] = v[0];
    w01  = vld1_s32(cospi + 2 * 8);
    butterfly_half_neon(w01, 0, 1, v[7], v[4], &u[4], v_bit);
    butterfly_dct_post_half(v + 8, v + 8, u + 8, 4);
    butterfly_dct_post_half(v + 12, v + 12, u + 12, 4);

    // stage 6
    v[0] = u[0];
    v[4] = u[4];
    w01  = vld1_s32(cospi + 2 * 4);
    butterfly_half_neon(w01, 0, 1, u[15], u[8], &v[8], v_bit);
    w01 = vld1_s32(cospi + 2 * 12);
    butterfly_half_neon(w01, 3, 0, u[11], u[12], &v[12], v_bit);

    out[0] = v[0];
    out[1] = v[8];
    out[2] = v[4];
    out[3] = v[12];
}

static inline void highbd_fadst16_N4_x4_neon(const int32x4_t* in, int32x4_t* out, int bit) {
    const int32_t* const cospi = cospi_arr_s32(bit);
    const int32x4_t      v_bit = vdupq_n_s32(-bit);

    int32x4_t u[16], v[16];

    // stage 0-1
    u[0]  = in[0];
    u[1]  = in[15];
    u[2]  = in[7];
    u[3]  = in[8];
    u[4]  = in[3];
    u[5]  = in[12];
    u[6]  = in[4];
    u[7]  = in[11];
    u[8]  = in[1];
    u[9]  = in[14];
    u[10] = in[6];
    u[11] = in[9];
    u[12] = in[2];
    u[13] = in[13];
    u[14] = in[5];
    u[15] = in[10];

    // stage 2
    v[0] = u[0];
    v[1] = u[1];
    butterfly_cospi32_0222_neon(cospi, u[3], u[2], &v[2], &v[3], v_bit);
    v[4] = u[4];
    v[5] = u[5];
    butterfly_cospi32_0002_neon(cospi, u[6], u[7], &v[7], &v[6], v_bit);
    v[8] = u[8];
    v[9] = u[9];
    butterfly_cospi32_0002_neon(cospi, u[10], u[11], &v[11], &v[10], v_bit);
    v[12] = u[12];
    v[13] = u[13];
    butterfly_cospi32_0222_neon(cospi, u[15], u[14], &v[14], &v[15], v_bit);

    // stage 3
    u[0]  = vaddq_s32(v[0], v[2]);
    u[1]  = vsubq_s32(v[3], v[1]);
    u[2]  = vsubq_s32(v[0], v[2]);
    u[3]  = vaddq_s32(v[1], v[3]);
    u[4]  = vsubq_s32(v[6], v[4]);
    u[5]  = vaddq_s32(v[5], v[7]);
    u[6]  = vaddq_s32(v[4], v[6]);
    u[7]  = vsubq_s32(v[5], v[7]);
    u[8]  = vsubq_s32(v[10], v[8]);
    u[9]  = vaddq_s32(v[9], v[11]);
    u[10] = vaddq_s32(v[8], v[10]);
    u[11] = vsubq_s32(v[9], v[11]);
    u[12] = vaddq_s32(v[12], v[14]);
    u[13] = vsubq_s32(v[15], v[13]);
    u[14] = vsubq_s32(v[12], v[14]);
    u[15] = vaddq_s32(v[13], v[15]);

    // stage 4
    v[0] = u[0];
    v[1] = u[1];
    v[2] = u[2];
    v[3] = u[3];
    butterfly_0112_neon(cospi, 16, u[4], u[5], &v[4], &v[5], v_bit);
    butterfly_0112_neon(cospi, 16, u[7], u[6], &v[6], &v[7], v_bit);

    v[8]  = u[8];
    v[9]  = u[9];
    v[10] = u[10];
    v[11] = u[11];

    butterfly_0112_neon(cospi, 16, u[12], u[13], &v[12], &v[13], v_bit);
    butterfly_0332_neon(cospi, 16, u[14], u[15], &v[15], &v[14], v_bit);

    // stage 5
    u[0]  = vaddq_s32(v[0], v[4]);
    u[1]  = vaddq_s32(v[1], v[5]);
    u[2]  = vaddq_s32(v[2], v[6]);
    u[3]  = vsubq_s32(v[7], v[3]);
    u[4]  = vsubq_s32(v[0], v[4]);
    u[5]  = vsubq_s32(v[1], v[5]);
    u[6]  = vsubq_s32(v[2], v[6]);
    u[7]  = vaddq_s32(v[3], v[7]);
    u[8]  = vaddq_s32(v[8], v[12]);
    u[9]  = vaddq_s32(v[9], v[13]);
    u[10] = vsubq_s32(v[14], v[10]);
    u[11] = vaddq_s32(v[11], v[15]);
    u[12] = vsubq_s32(v[8], v[12]);
    u[13] = vsubq_s32(v[9], v[13]);
    u[14] = vaddq_s32(v[10], v[14]);
    u[15] = vsubq_s32(v[11], v[15]);

    // stage 6
    v[0] = u[0];
    v[1] = u[1];
    v[2] = u[2];
    v[3] = u[3];
    v[4] = u[4];
    v[5] = u[5];
    v[6] = u[6];
    v[7] = u[7];

    butterfly_0112_neon(cospi, 8, u[8], u[9], &v[8], &v[9], v_bit);
    butterfly_0130_neon(cospi, 8, u[12], u[13], &v[13], &v[12], v_bit);
    butterfly_0130_neon(cospi, 24, u[11], u[10], &v[10], &v[11], v_bit);
    butterfly_0130_neon(cospi, 24, u[14], u[15], &v[14], &v[15], v_bit);

    // stage 7
    u[0]  = vaddq_s32(v[0], v[8]);
    u[1]  = vaddq_s32(v[1], v[9]);
    u[2]  = vaddq_s32(v[2], v[10]);
    u[3]  = vaddq_s32(v[3], v[11]);
    u[12] = vsubq_s32(v[4], v[12]);
    u[13] = vsubq_s32(v[5], v[13]);
    u[14] = vsubq_s32(v[6], v[14]);
    u[15] = vaddq_s32(v[7], v[15]);

    // stage 8
    int32x2_t w01 = vld1_s32(cospi + 2 * 2);
    butterfly_half_neon(w01, 1, 2, u[0], u[1], &v[1], v_bit);
    w01 = vld1_s32(cospi + 2 * 10);
    butterfly_half_neon(w01, 1, 2, u[2], u[3], &v[3], v_bit);
    w01 = vld1_s32(cospi + 2 * 14);
    butterfly_half_neon(w01, 0, 1, u[13], u[12], &v[12], v_bit);
    w01 = vld1_s32(cospi + 2 * 6);
    butterfly_half_neon(w01, 1, 2, u[14], u[15], &v[14], v_bit);

    // stage 9
    out[0] = v[1];
    out[1] = v[14];
    out[2] = v[3];
    out[3] = v[12];
}

static inline void highbd_fidentity16_N4_x4_neon(const int32x4_t* in, int32x4_t* out, int bit) {
    (void)bit;
    const int32x4_t fact = vdupq_n_s32(2 * new_sqrt2);

    for (int i = 0; i < 4; i++) {
        int32x4_t a = vmulq_s32(in[i], fact);
        out[i]      = vrshrq_n_s32(a, new_sqrt2_bits);
    }
}

static inline void highbd_fdct16_N4_xn_neon(const int32x4_t* in, int32x4_t* out, int bit, const int howmany) {
    const int stride = 16;
    int       i      = 0;
    do {
        highbd_fdct16_N4_x4_neon(in + i * stride, out + i * stride, bit);
    } while (++i < howmany);
}

static inline void highbd_fadst16_N4_xn_neon(const int32x4_t* in, int32x4_t* out, int bit, int howmany) {
    const int stride = 16;
    int       i      = 0;
    do {
        highbd_fadst16_N4_x4_neon(in + i * stride, out + i * stride, bit);
    } while (++i < howmany);
}

static inline void highbd_fidentity16_N4_xn_neon(const int32x4_t* in, int32x4_t* out, int bit, int howmany) {
    const int stride = 16;
    int       i      = 0;
    do {
        highbd_fidentity16_N4_x4_neon(in + i * stride, out + i * stride, bit);
    } while (++i < howmany);
}

static inline void transpose_16x4_in_16x16(const int32x4_t* in, int32x4_t* out) {
    transpose_elems_s32_4x4(in[0], in[1], in[2], in[3], &out[0], &out[1], &out[2], &out[3]);
    transpose_elems_s32_4x4(in[16], in[17], in[18], in[19], &out[4], &out[5], &out[6], &out[7]);
    transpose_elems_s32_4x4(in[32], in[33], in[34], in[35], &out[8], &out[9], &out[10], &out[11]);
    transpose_elems_s32_4x4(in[48], in[49], in[50], in[51], &out[12], &out[13], &out[14], &out[15]);
}

static inline void shift_right_2_round_s32_x4_N4(int32x4_t* in, int32x4_t* out) {
    shift_right_2_round_s32_x4(in + 0, out + 0, 4);
    shift_right_2_round_s32_x4(in + 16, out + 16, 4);
    shift_right_2_round_s32_x4(in + 32, out + 32, 4);
    shift_right_2_round_s32_x4(in + 48, out + 48, 4);
}

static inline void write_buffer_16x16_N4(const int32x4_t* buf, int32_t* output) {
    const int32x4_t zero = vdupq_n_s32(0);

    for (int i = 0; i < 4; i++) {
        vst1q_s32(output + i * 16 + 0, buf[i]);
        vst1q_s32(output + i * 16 + 4, zero);
        vst1q_s32(output + i * 16 + 8, zero);
        vst1q_s32(output + i * 16 + 12, zero);
    }

    memset(output + 4 * 16, 0, 12 * 16 * sizeof(int32_t));
}

static inline void load_buffer_16x4_in_16x16(const int16_t* input, int32x4_t* buf, int stride) {
    load_buffer_4x4(input + 0 * 4, buf + 0 * 16, stride, 0);
    load_buffer_4x4(input + 1 * 4, buf + 1 * 16, stride, 0);
    load_buffer_4x4(input + 2 * 4, buf + 2 * 16, stride, 0);
    load_buffer_4x4(input + 3 * 4, buf + 3 * 16, stride, 0);
}

static inline void load_buffer_16x4_in_16x16_flip(const int16_t* input, int32x4_t* buf, int stride) {
    load_buffer_4x4(input + 0 * 4, buf + 3 * 16, stride, 1);
    load_buffer_4x4(input + 1 * 4, buf + 2 * 16, stride, 1);
    load_buffer_4x4(input + 2 * 4, buf + 1 * 16, stride, 1);
    load_buffer_4x4(input + 3 * 4, buf + 0 * 16, stride, 1);
}

void svt_av1_fwd_txfm2d_16x16_N4_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 16);

    // Workspaces for column/row-wise transforms.
    int32x4_t buf0[64], buf1[64];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_16x16(input, buf0, stride, 0);
        highbd_fdct16_N4_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_x4_N4(buf0, buf0);
        transpose_16x4_in_16x16(buf0, buf1);
        highbd_fdct16_N4_x4_neon(buf1, buf1, fwd_cos_bit_row[2][2]);
        transpose_arrays_s32_4x4(buf1, buf0);
        write_buffer_16x16_N4(buf0, output);
        break;
    case ADST_DCT:
        load_buffer_16x16(input, buf0, stride, 0);
        highbd_fadst16_N4_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_x4_N4(buf0, buf0);
        transpose_16x4_in_16x16(buf0, buf1);
        highbd_fdct16_N4_x4_neon(buf1, buf1, fwd_cos_bit_row[2][2]);
        transpose_arrays_s32_4x4(buf1, buf0);
        write_buffer_16x16_N4(buf0, output);
        break;
    case DCT_ADST:
        load_buffer_16x16(input, buf0, stride, 0);
        highbd_fdct16_N4_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_x4_N4(buf0, buf0);
        transpose_16x4_in_16x16(buf0, buf1);
        highbd_fadst16_N4_x4_neon(buf1, buf1, fwd_cos_bit_row[2][2]);
        transpose_arrays_s32_4x4(buf1, buf0);
        write_buffer_16x16_N4(buf0, output);
        break;
    case ADST_ADST:
        load_buffer_16x16(input, buf0, stride, 0);
        highbd_fadst16_N4_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_x4_N4(buf0, buf0);
        transpose_16x4_in_16x16(buf0, buf1);
        highbd_fadst16_N4_x4_neon(buf1, buf1, fwd_cos_bit_row[2][2]);
        transpose_arrays_s32_4x4(buf1, buf0);
        write_buffer_16x16_N4(buf0, output);
        break;
    case FLIPADST_DCT:
        load_buffer_16x16(input, buf0, stride, 0);
        highbd_fadst16_N4_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_x4_N4(buf0, buf0);
        transpose_16x4_in_16x16(buf0, buf1);
        highbd_fdct16_N4_x4_neon(buf1, buf1, fwd_cos_bit_row[2][2]);
        transpose_arrays_s32_4x4(buf1, buf0);
        write_buffer_16x16_N4(buf0, output);
        break;
    case DCT_FLIPADST:
        load_buffer_16x16(input, buf0, stride, 1);
        highbd_fdct16_N4_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_x4_N4(buf0, buf0);
        transpose_16x4_in_16x16(buf0, buf1);
        highbd_fadst16_N4_x4_neon(buf1, buf1, fwd_cos_bit_row[2][2]);
        transpose_arrays_s32_4x4(buf1, buf0);
        write_buffer_16x16_N4(buf0, output);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_16x16(input, buf0, stride, 1);
        highbd_fadst16_N4_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_x4_N4(buf0, buf0);
        transpose_16x4_in_16x16(buf0, buf1);
        highbd_fadst16_N4_x4_neon(buf1, buf1, fwd_cos_bit_row[2][2]);
        transpose_arrays_s32_4x4(buf1, buf0);
        write_buffer_16x16_N4(buf0, output);
        break;
    case ADST_FLIPADST:
        load_buffer_16x16(input, buf0, stride, 1);
        highbd_fadst16_N4_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_x4_N4(buf0, buf0);
        transpose_16x4_in_16x16(buf0, buf1);
        highbd_fadst16_N4_x4_neon(buf1, buf1, fwd_cos_bit_row[2][2]);
        transpose_arrays_s32_4x4(buf1, buf0);
        write_buffer_16x16_N4(buf0, output);
        break;
    case FLIPADST_ADST:
        load_buffer_16x16(input, buf0, stride, 0);
        highbd_fadst16_N4_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_x4_N4(buf0, buf0);
        transpose_16x4_in_16x16(buf0, buf1);
        highbd_fadst16_N4_x4_neon(buf1, buf1, fwd_cos_bit_row[2][2]);
        transpose_arrays_s32_4x4(buf1, buf0);
        write_buffer_16x16_N4(buf0, output);
        break;
    case IDTX:
        load_buffer_4x4(input, buf0, stride, 0);
        highbd_fidentity16_N4_x4_neon(buf0, buf1, fwd_cos_bit_col[2][2]);
        shift_right_2_round_s32_x4(buf1, buf1, 4);
        highbd_fidentity16_N4_x4_neon(buf1, buf0, fwd_cos_bit_row[2][2]);
        write_buffer_16x16_N4(buf0, output);
        break;
    case V_DCT:
        load_buffer_4x16(input, buf0, stride, 0);
        highbd_fdct16_N4_x4_neon(buf0, buf0, fwd_cos_bit_col[2][2]);
        shift_right_2_round_s32_x4(buf0, buf0, 4);
        highbd_fidentity16_N4_x4_neon(buf0, buf0, fwd_cos_bit_row[2][2]);
        write_buffer_16x16_N4(buf0, output);
        break;
    case H_DCT:
        load_buffer_16x4_in_16x16(input, buf0, stride);
        highbd_fidentity16_N4_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_x4_N4(buf0, buf0);
        transpose_16x4_in_16x16(buf0, buf1);
        highbd_fdct16_N4_x4_neon(buf1, buf1, fwd_cos_bit_row[2][2]);
        transpose_arrays_s32_4x4(buf1, buf0);
        write_buffer_16x16_N4(buf0, output);
        break;
    case V_ADST:
        load_buffer_4x16(input, buf0, stride, 0);
        highbd_fadst16_N4_x4_neon(buf0, buf0, fwd_cos_bit_col[2][2]);
        shift_right_2_round_s32_x4(buf0, buf0, 4);
        highbd_fidentity16_N4_x4_neon(buf0, buf0, fwd_cos_bit_row[2][2]);
        write_buffer_16x16_N4(buf0, output);
        break;
    case H_ADST:
        load_buffer_16x4_in_16x16(input, buf0, stride);
        highbd_fidentity16_N4_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_x4_N4(buf0, buf0);
        transpose_16x4_in_16x16(buf0, buf1);
        highbd_fadst16_N4_x4_neon(buf1, buf1, fwd_cos_bit_row[2][2]);
        transpose_arrays_s32_4x4(buf1, buf0);
        write_buffer_16x16_N4(buf0, output);
        break;
    case V_FLIPADST:
        load_buffer_4x16(input, buf0, stride, 0);
        highbd_fadst16_N4_x4_neon(buf0, buf0, fwd_cos_bit_col[2][2]);
        shift_right_2_round_s32_x4(buf0, buf0, 4);
        highbd_fidentity16_N4_x4_neon(buf0, buf0, fwd_cos_bit_row[2][2]);
        write_buffer_16x16_N4(buf0, output);
        break;
    case H_FLIPADST:
        load_buffer_16x4_in_16x16_flip(input, buf0, stride);
        highbd_fidentity16_N4_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_x4_N4(buf0, buf0);
        transpose_16x4_in_16x16(buf0, buf1);
        highbd_fadst16_N4_x4_neon(buf1, buf1, fwd_cos_bit_row[2][2]);
        transpose_arrays_s32_4x4(buf1, buf0);
        write_buffer_16x16_N4(buf0, output);
        break;
    default:
        assert(0);
    }
}

static inline void highbd_fdct32_N4_x4_neon(const int32x4_t* input, int32x4_t* output, int cos_bit) {
    const int32_t* const cospi     = cospi_arr_s32(cos_bit);
    const int32x4_t      v_cos_bit = vdupq_n_s32(-cos_bit);

    // Workspaces for intermediate transform steps.
    int32x4_t buf0[32];
    int32x4_t buf1[32];

    // stage 1
    butterfly_dct_pre(input, buf1, 32);

    // stage 2
    butterfly_dct_pre(buf1, buf0, 16);
    buf0[16] = buf1[16];
    buf0[17] = buf1[17];
    buf0[18] = buf1[18];
    buf0[19] = buf1[19];
    butterfly_0112_neon(cospi, 32, buf1[27], buf1[20], &buf0[27], &buf0[20], v_cos_bit);
    butterfly_0112_neon(cospi, 32, buf1[26], buf1[21], &buf0[26], &buf0[21], v_cos_bit);
    butterfly_0112_neon(cospi, 32, buf1[25], buf1[22], &buf0[25], &buf0[22], v_cos_bit);
    butterfly_0112_neon(cospi, 32, buf1[24], buf1[23], &buf0[24], &buf0[23], v_cos_bit);
    buf0[28] = buf1[28];
    buf0[29] = buf1[29];
    buf0[30] = buf1[30];
    buf0[31] = buf1[31];

    // stage 3
    butterfly_dct_pre(buf0, buf1, 8);
    buf1[8] = buf0[8];
    buf1[9] = buf0[9];
    butterfly_0112_neon(cospi, 32, buf0[13], buf0[10], &buf1[13], &buf1[10], v_cos_bit);
    butterfly_0112_neon(cospi, 32, buf0[12], buf0[11], &buf1[12], &buf1[11], v_cos_bit);
    buf1[14] = buf0[14];
    buf1[15] = buf0[15];
    butterfly_dct_post(buf0 + 16, buf0 + 16, buf1 + 16, 16);

    // stage 4
    butterfly_dct_pre(buf1, buf0, 4);
    buf0[4] = buf1[4];
    butterfly_0112_neon(cospi, 32, buf1[6], buf1[5], &buf0[6], &buf0[5], v_cos_bit);
    buf0[7] = buf1[7];
    butterfly_dct_post(buf1 + 8, buf1 + 8, buf0 + 8, 8);
    buf0[16] = buf1[16];
    buf0[17] = buf1[17];
    butterfly_0112_neon(cospi, 16, buf1[29], buf1[18], &buf0[29], &buf0[18], v_cos_bit);
    butterfly_0112_neon(cospi, 16, buf1[28], buf1[19], &buf0[28], &buf0[19], v_cos_bit);
    butterfly_2312_neon(cospi, 16, buf1[27], buf1[20], &buf0[20], &buf0[27], v_cos_bit);
    butterfly_2312_neon(cospi, 16, buf1[26], buf1[21], &buf0[21], &buf0[26], v_cos_bit);
    buf0[22] = buf1[22];
    buf0[23] = buf1[23];
    buf0[24] = buf1[24];
    buf0[25] = buf1[25];
    buf0[30] = buf1[30];
    buf0[31] = buf1[31];

    // stage 5
    butterfly_0112_neon(cospi, 32, buf0[0], buf0[1], &buf1[0], &buf1[1], v_cos_bit);
    butterfly_dct_post_half(buf0 + 4, buf0 + 4, buf1 + 4, 4);
    buf1[8] = buf0[8];
    butterfly_0112_neon(cospi, 16, buf0[14], buf0[9], &buf1[14], &buf1[9], v_cos_bit);
    butterfly_2312_neon(cospi, 16, buf0[13], buf0[10], &buf1[10], &buf1[13], v_cos_bit);
    buf1[11] = buf0[11];
    buf1[12] = buf0[12];
    buf1[15] = buf0[15];
    butterfly_dct_post(buf0 + 16, buf0 + 16, buf1 + 16, 8);
    butterfly_dct_post(buf0 + 24, buf0 + 24, buf1 + 24, 8);

    // stage 6
    buf0[0] = buf1[0];

    int32x2_t w01 = vld1_s32(cospi + 2 * 8);
    butterfly_half_neon(w01, 0, 1, buf1[7], buf1[4], &buf0[4], v_cos_bit);
    butterfly_0112_neon(cospi, 8, buf1[30], buf1[17], &buf0[30], &buf0[17], v_cos_bit);
    butterfly_2312_neon(cospi, 8, buf1[29], buf1[18], &buf0[18], &buf0[29], v_cos_bit);
    butterfly_dct_post_half(buf1 + 8, buf1 + 8, buf0 + 8, 4);
    butterfly_dct_post_half(buf1 + 12, buf1 + 12, buf0 + 12, 4);
    buf0[16] = buf1[16];
    buf0[19] = buf1[19];
    buf0[20] = buf1[20];

    butterfly_0130_neon(cospi, 24, buf1[21], buf1[26], &buf0[26], &buf0[21], v_cos_bit);
    butterfly_0332_neon(cospi, 24, buf1[25], buf1[22], &buf0[25], &buf0[22], v_cos_bit);

    buf0[23] = buf1[23];
    buf0[24] = buf1[24];
    buf0[27] = buf1[27];
    buf0[28] = buf1[28];
    buf0[31] = buf1[31];

    // stage 7
    buf1[0] = buf0[0];
    buf1[2] = buf0[2];
    buf1[4] = buf0[4];
    w01     = vld1_s32(cospi + 2 * 4);
    butterfly_half_neon(w01, 0, 1, buf0[15], buf0[8], &buf1[8], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 12);
    butterfly_half_neon(w01, 3, 0, buf0[11], buf0[12], &buf1[12], v_cos_bit);
    butterfly_dct_post_half(buf0 + 16, buf0 + 16, buf1 + 16, 4);
    butterfly_dct_post_half(buf0 + 20, buf0 + 20, buf1 + 20, 4);
    butterfly_dct_post_half(buf0 + 24, buf0 + 24, buf1 + 24, 4);
    butterfly_dct_post_half(buf0 + 28, buf0 + 28, buf1 + 28, 4);

    // stage 8
    buf0[0]  = buf1[0];
    buf0[2]  = buf1[2];
    buf0[4]  = buf1[4];
    buf0[8]  = buf1[8];
    buf0[12] = buf1[12];
    w01      = vld1_s32(cospi + 2 * 2);
    butterfly_half_neon(w01, 0, 1, buf1[31], buf1[16], &buf0[16], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 14);
    butterfly_half_neon(w01, 3, 0, buf1[19], buf1[28], &buf0[28], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 10);
    butterfly_half_neon(w01, 0, 1, buf1[27], buf1[20], &buf0[20], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 6);
    butterfly_half_neon(w01, 3, 0, buf1[23], buf1[24], &buf0[24], v_cos_bit);

    // stage 9
    output[0] = buf0[0];
    output[1] = buf0[16];
    output[2] = buf0[8];
    output[3] = buf0[24];
    output[4] = buf0[4];
    output[5] = buf0[20];
    output[6] = buf0[12];
    output[7] = buf0[28];
    output[8] = buf0[2];
}

static inline void highbd_fidentity32_N4_x4_neon(const int32x4_t* input, int32x4_t* output, int cos_bit) {
    (void)cos_bit;
    for (int i = 0; i < 8; i++) {
        output[i] = vshlq_n_s32(input[i], 2);
    }
}

static inline void highbd_fdct32_N4_xn_neon(const int32x4_t* in, int32x4_t* out, int bit, const int howmany) {
    const int stride = 32;
    int       i      = 0;
    do {
        highbd_fdct32_N4_x4_neon(in + i * stride, out + i * stride, bit);
    } while (++i < howmany);
}

static inline void highbd_fidentity32_N4_xn_neon(const int32x4_t* in, int32x4_t* out, int bit, int howmany) {
    const int stride = 32;
    int       i      = 0;
    do {
        highbd_fidentity32_N4_x4_neon(in + i * stride, out + i * stride, bit);
    } while (++i < howmany);
}

static inline void write_buffer_32x32_N4(const int32x4_t* in, int32_t* output) {
    const int32x4_t zero = vdupq_n_s32(0);

    for (int i = 0; i < 8; i++) {
        vst1q_s32(output + i * 32 + 0, in[0 + i]);
        vst1q_s32(output + i * 32 + 4, in[32 + i]);
        for (int j = 2; j < 8; j++) {
            vst1q_s32(output + i * 32 + j * 4, zero);
        }
    }

    memset((output + 8 * 32), 0, (24 * 32) * sizeof(int32_t));
}

static inline void shift_right_4_round_s32_x4_N4(int32x4_t* in, int32x4_t* out) {
    shift_right_4_round_s32_x4(in + 0 * 32, out + 0 * 32, 8);
    shift_right_4_round_s32_x4(in + 1 * 32, out + 1 * 32, 8);
    shift_right_4_round_s32_x4(in + 2 * 32, out + 2 * 32, 8);
    shift_right_4_round_s32_x4(in + 3 * 32, out + 3 * 32, 8);
    shift_right_4_round_s32_x4(in + 4 * 32, out + 4 * 32, 8);
    shift_right_4_round_s32_x4(in + 5 * 32, out + 5 * 32, 8);
    shift_right_4_round_s32_x4(in + 6 * 32, out + 6 * 32, 8);
    shift_right_4_round_s32_x4(in + 7 * 32, out + 7 * 32, 8);
}

static inline void transpose_32x8_in_32x32(const int32x4_t* in, int32x4_t* out) {
    for (int i = 0; i < 8; i++) {
        transpose_elems_s32_4x4(in[0 + i * 32],
                                in[1 + i * 32],
                                in[2 + i * 32],
                                in[3 + i * 32],
                                &out[0 + i * 4],
                                &out[1 + i * 4],
                                &out[2 + i * 4],
                                &out[3 + i * 4]);
        transpose_elems_s32_4x4(in[4 + i * 32],
                                in[5 + i * 32],
                                in[6 + i * 32],
                                in[7 + i * 32],
                                &out[32 + i * 4],
                                &out[33 + i * 4],
                                &out[34 + i * 4],
                                &out[35 + i * 4]);
    }
}

static inline void transpose_8x8_in_32x32(const int32x4_t* in, int32x4_t* out) {
    transpose_elems_s32_4x4(in[0], in[1], in[2], in[3], &out[0], &out[1], &out[2], &out[3]);
    transpose_elems_s32_4x4(in[32], in[33], in[34], in[35], &out[4], &out[5], &out[6], &out[7]);
    transpose_elems_s32_4x4(in[4], in[5], in[6], in[7], &out[32], &out[33], &out[34], &out[35]);
    transpose_elems_s32_4x4(in[36], in[37], in[38], in[39], &out[36], &out[37], &out[38], &out[39]);
}

static inline void load_buffer_8x8_in_32x32(const int16_t* input, int32x4_t* buf, int stride) {
    load_buffer_4x4(input + 0 * stride + 0, buf, stride, 0);
    load_buffer_4x4(input + 0 * stride + 4, buf + 32, stride, 0);
    load_buffer_4x4(input + 4 * stride + 0, buf + 4, stride, 0);
    load_buffer_4x4(input + 4 * stride + 4, buf + 32 + 4, stride, 0);
}

static inline void load_buffer_32x8_in_32x32(const int16_t* input, int32x4_t* buf, int stride) {
    load_buffer_4x8(input + 0 * 4, buf + 0 * 32, stride, 0);
    load_buffer_4x8(input + 1 * 4, buf + 1 * 32, stride, 0);
    load_buffer_4x8(input + 2 * 4, buf + 2 * 32, stride, 0);
    load_buffer_4x8(input + 3 * 4, buf + 3 * 32, stride, 0);
    load_buffer_4x8(input + 4 * 4, buf + 4 * 32, stride, 0);
    load_buffer_4x8(input + 5 * 4, buf + 5 * 32, stride, 0);
    load_buffer_4x8(input + 6 * 4, buf + 6 * 32, stride, 0);
    load_buffer_4x8(input + 7 * 4, buf + 7 * 32, stride, 0);
}

void svt_av1_fwd_txfm2d_32x32_N4_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;

    // Workspaces for column/row-wise transforms.
    int32x4_t buf0[256], buf1[256];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_32x32(input, buf0, stride, 0);
        highbd_fdct32_N4_xn_neon(buf0, buf0, fwd_cos_bit_col[3][3], 8);
        shift_right_4_round_s32_x4_N4(buf0, buf0);
        transpose_32x8_in_32x32(buf0, buf1);
        highbd_fdct32_N4_xn_neon(buf1, buf1, fwd_cos_bit_row[3][3], 2);
        transpose_8x8_in_32x32(buf1, buf0);
        write_buffer_32x32_N4(buf0, output);
        break;
    case IDTX:
        load_buffer_8x8_in_32x32(input, buf0, stride);
        highbd_fidentity32_N4_xn_neon(buf0, buf1, fwd_cos_bit_col[3][3], 2);
        shift_right_4_round_s32_x4(buf1, buf1, 8);
        shift_right_4_round_s32_x4(buf1 + 32, buf1 + 32, 8);
        highbd_fidentity32_N4_xn_neon(buf1, buf0, fwd_cos_bit_row[3][3], 2);
        write_buffer_32x32_N4(buf0, output);
        break;
    case V_DCT:
        load_buffer_8x32(input, buf0, stride, 0);
        highbd_fdct32_N4_xn_neon(buf0, buf0, fwd_cos_bit_col[3][3], 2);
        shift_right_4_round_s32_x4_N4(buf0, buf0);
        highbd_fidentity32_N4_xn_neon(buf0, buf0, fwd_cos_bit_row[3][3], 2);
        write_buffer_32x32_N4(buf0, output);
        break;
    case H_DCT:
        load_buffer_32x8_in_32x32(input, buf0, stride);
        highbd_fidentity32_N4_xn_neon(buf0, buf0, fwd_cos_bit_col[3][3], 8);
        shift_right_4_round_s32_x4_N4(buf0, buf0);
        transpose_32x8_in_32x32(buf0, buf1);
        highbd_fdct32_N4_xn_neon(buf1, buf1, fwd_cos_bit_row[3][3], 2);
        transpose_8x8_in_32x32(buf1, buf0);
        write_buffer_32x32_N4(buf0, output);
        break;
    default:
        assert(0);
    }
}

static inline void highbd_fdct64_N4_x4_neon(const int32x4_t* input, int32x4_t* output, int8_t cos_bit) {
    const int32_t* const cospi     = cospi_arr_s32(cos_bit);
    const int32x4_t      v_cos_bit = vdupq_n_s32(-cos_bit);

    // stage 1
    int32x4_t x1[64];
    butterfly_dct_pre(input, x1, 64);

    // stage 2
    int32x4_t x2[64];
    butterfly_dct_pre(x1, x2, 32);
    x2[32] = x1[32];
    x2[33] = x1[33];
    x2[34] = x1[34];
    x2[35] = x1[35];
    x2[36] = x1[36];
    x2[37] = x1[37];
    x2[38] = x1[38];
    x2[39] = x1[39];
    butterfly_0112_neon(cospi, 32, x1[55], x1[40], &x2[55], &x2[40], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x1[54], x1[41], &x2[54], &x2[41], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x1[53], x1[42], &x2[53], &x2[42], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x1[52], x1[43], &x2[52], &x2[43], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x1[51], x1[44], &x2[51], &x2[44], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x1[50], x1[45], &x2[50], &x2[45], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x1[49], x1[46], &x2[49], &x2[46], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x1[48], x1[47], &x2[48], &x2[47], v_cos_bit);
    x2[56] = x1[56];
    x2[57] = x1[57];
    x2[58] = x1[58];
    x2[59] = x1[59];
    x2[60] = x1[60];
    x2[61] = x1[61];
    x2[62] = x1[62];
    x2[63] = x1[63];

    // stage 3
    int32x4_t x3[64];
    butterfly_dct_pre(x2, x3, 16);
    x3[16] = x2[16];
    x3[17] = x2[17];
    x3[18] = x2[18];
    x3[19] = x2[19];
    butterfly_0112_neon(cospi, 32, x2[27], x2[20], &x3[27], &x3[20], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x2[26], x2[21], &x3[26], &x3[21], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x2[25], x2[22], &x3[25], &x3[22], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x2[24], x2[23], &x3[24], &x3[23], v_cos_bit);
    x3[28] = x2[28];
    x3[29] = x2[29];
    x3[30] = x2[30];
    x3[31] = x2[31];
    butterfly_dct_post(x2 + 32, x2 + 32, x3 + 32, 32);

    // stage 4
    int32x4_t x4[64];
    butterfly_dct_pre(x3, x4, 8);
    x4[8] = x3[8];
    x4[9] = x3[9];
    butterfly_0112_neon(cospi, 32, x3[13], x3[10], &x4[13], &x4[10], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x3[12], x3[11], &x4[12], &x4[11], v_cos_bit);
    x4[14] = x3[14];
    x4[15] = x3[15];
    butterfly_dct_post(x3 + 16, x3 + 16, x4 + 16, 16);
    x4[32] = x3[32];
    x4[33] = x3[33];
    x4[34] = x3[34];
    x4[35] = x3[35];
    butterfly_0112_neon(cospi, 16, x3[59], x3[36], &x4[59], &x4[36], v_cos_bit);
    butterfly_0112_neon(cospi, 16, x3[58], x3[37], &x4[58], &x4[37], v_cos_bit);
    butterfly_0112_neon(cospi, 16, x3[57], x3[38], &x4[57], &x4[38], v_cos_bit);
    butterfly_0112_neon(cospi, 16, x3[56], x3[39], &x4[56], &x4[39], v_cos_bit);
    butterfly_2312_neon(cospi, 16, x3[55], x3[40], &x4[40], &x4[55], v_cos_bit);
    butterfly_2312_neon(cospi, 16, x3[54], x3[41], &x4[41], &x4[54], v_cos_bit);
    butterfly_2312_neon(cospi, 16, x3[53], x3[42], &x4[42], &x4[53], v_cos_bit);
    butterfly_2312_neon(cospi, 16, x3[52], x3[43], &x4[43], &x4[52], v_cos_bit);
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
    int32x4_t x5[64];
    butterfly_dct_pre(x4, x5, 4);
    x5[4] = x4[4];
    butterfly_0112_neon(cospi, 32, x4[6], x4[5], &x5[6], &x5[5], v_cos_bit);
    x5[7] = x4[7];
    butterfly_dct_post(x4 + 8, x4 + 8, x5 + 8, 8);
    x5[16] = x4[16];
    x5[17] = x4[17];
    butterfly_0112_neon(cospi, 16, x4[29], x4[18], &x5[29], &x5[18], v_cos_bit);
    butterfly_0112_neon(cospi, 16, x4[28], x4[19], &x5[28], &x5[19], v_cos_bit);
    butterfly_2312_neon(cospi, 16, x4[27], x4[20], &x5[20], &x5[27], v_cos_bit);
    butterfly_2312_neon(cospi, 16, x4[26], x4[21], &x5[21], &x5[26], v_cos_bit);
    x5[22] = x4[22];
    x5[23] = x4[23];
    x5[24] = x4[24];
    x5[25] = x4[25];
    x5[30] = x4[30];
    x5[31] = x4[31];
    butterfly_dct_post(x4 + 32, x4 + 32, x5 + 32, 16);
    butterfly_dct_post(x4 + 48, x4 + 48, x5 + 48, 16);

    // stage 6
    int32x4_t x6[64];
    int32x2_t w01 = vld1_s32(cospi + 2 * 32);
    butterfly_half_neon(w01, 0, 1, x5[0], x5[1], &x6[0], v_cos_bit);
    butterfly_dct_post_half(x5 + 4, x5 + 4, x6 + 4, 4);
    x6[8] = x5[8];
    butterfly_0112_neon(cospi, 16, x5[14], x5[9], &x6[14], &x6[9], v_cos_bit);
    butterfly_2312_neon(cospi, 16, x5[13], x5[10], &x6[10], &x6[13], v_cos_bit);
    x6[11] = x5[11];
    x6[12] = x5[12];
    x6[15] = x5[15];
    butterfly_dct_post(x5 + 16, x5 + 16, x6 + 16, 8);
    butterfly_dct_post(x5 + 24, x5 + 24, x6 + 24, 8);
    x6[32] = x5[32];
    x6[33] = x5[33];
    butterfly_0112_neon(cospi, 8, x5[61], x5[34], &x6[61], &x6[34], v_cos_bit);
    butterfly_0112_neon(cospi, 8, x5[60], x5[35], &x6[60], &x6[35], v_cos_bit);
    butterfly_2312_neon(cospi, 8, x5[59], x5[36], &x6[36], &x6[59], v_cos_bit);
    butterfly_2312_neon(cospi, 8, x5[58], x5[37], &x6[37], &x6[58], v_cos_bit);
    x6[38] = x5[38];
    x6[39] = x5[39];
    x6[40] = x5[40];
    x6[41] = x5[41];
    butterfly_0130_neon(cospi, 24, x5[42], x5[53], &x6[53], &x6[42], v_cos_bit);
    butterfly_0130_neon(cospi, 24, x5[43], x5[52], &x6[52], &x6[43], v_cos_bit);
    butterfly_0332_neon(cospi, 24, x5[51], x5[44], &x6[51], &x6[44], v_cos_bit);
    butterfly_0332_neon(cospi, 24, x5[50], x5[45], &x6[50], &x6[45], v_cos_bit);
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
    int32x4_t x7[64];
    x7[0] = x6[0];
    w01   = vld1_s32(cospi + 2 * 8);
    butterfly_half_neon(w01, 0, 1, x6[7], x6[4], &x7[4], v_cos_bit);
    butterfly_dct_post_half(x6 + 8, x6 + 8, x7 + 8, 4);
    butterfly_dct_post_half(x6 + 12, x6 + 12, x7 + 12, 4);
    x7[16] = x6[16];
    butterfly_0112_neon(cospi, 8, x6[30], x6[17], &x7[30], &x7[17], v_cos_bit);
    butterfly_2312_neon(cospi, 8, x6[29], x6[18], &x7[18], &x7[29], v_cos_bit);
    x7[19] = x6[19];
    x7[20] = x6[20];
    butterfly_0130_neon(cospi, 24, x6[21], x6[26], &x7[26], &x7[21], v_cos_bit);
    butterfly_0332_neon(cospi, 24, x6[25], x6[22], &x7[25], &x7[22], v_cos_bit);
    x7[23] = x6[23];
    x7[24] = x6[24];
    x7[27] = x6[27];
    x7[28] = x6[28];
    x7[31] = x6[31];
    butterfly_dct_post(x6 + 32, x6 + 32, x7 + 32, 8);
    butterfly_dct_post(x6 + 40, x6 + 40, x7 + 40, 8);
    butterfly_dct_post(x6 + 48, x6 + 48, x7 + 48, 8);
    butterfly_dct_post(x6 + 56, x6 + 56, x7 + 56, 8);

    // stage 8
    int32x4_t x8[64];
    x8[0] = x7[0];
    x8[4] = x7[4];

    w01 = vld1_s32(cospi + 2 * 4);
    butterfly_half_neon(w01, 0, 1, x7[15], x7[8], &x8[8], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 12);
    butterfly_half_neon(w01, 3, 0, x7[11], x7[12], &x8[12], v_cos_bit);
    butterfly_dct_post_half(x7 + 16, x7 + 16, x8 + 16, 4);
    butterfly_dct_post_half(x7 + 20, x7 + 20, x8 + 20, 4);
    butterfly_dct_post_half(x7 + 24, x7 + 24, x8 + 24, 4);
    butterfly_dct_post_half(x7 + 28, x7 + 28, x8 + 28, 4);
    x8[32] = x7[32];
    butterfly_0112_neon(cospi, 4, x7[62], x7[33], &x8[62], &x8[33], v_cos_bit);
    butterfly_2312_neon(cospi, 4, x7[61], x7[34], &x8[34], &x8[61], v_cos_bit);
    x8[35] = x7[35];
    x8[36] = x7[36];
    butterfly_0130_neon(cospi, 28, x7[37], x7[58], &x8[58], &x8[37], v_cos_bit);
    butterfly_0332_neon(cospi, 28, x7[57], x7[38], &x8[57], &x8[38], v_cos_bit);
    x8[39] = x7[39];
    x8[40] = x7[40];
    butterfly_0112_neon(cospi, 20, x7[54], x7[41], &x8[54], &x8[41], v_cos_bit);
    butterfly_2312_neon(cospi, 20, x7[53], x7[42], &x8[42], &x8[53], v_cos_bit);
    x8[43] = x7[43];
    x8[44] = x7[44];
    butterfly_0130_neon(cospi, 12, x7[45], x7[50], &x8[50], &x8[45], v_cos_bit);
    butterfly_0332_neon(cospi, 12, x7[49], x7[46], &x8[49], &x8[46], v_cos_bit);
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
    int32x4_t x9[64];
    x9[0]  = x8[0];
    x9[4]  = x8[4];
    x9[8]  = x8[8];
    x9[12] = x8[12];
    w01    = vld1_s32(cospi + 2 * 2);
    butterfly_half_neon(w01, 0, 1, x8[31], x8[16], &x9[16], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 14);
    butterfly_half_neon(w01, 3, 0, x8[19], x8[28], &x9[28], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 10);
    butterfly_half_neon(w01, 0, 1, x8[27], x8[20], &x9[20], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 6);
    butterfly_half_neon(w01, 3, 0, x8[23], x8[24], &x9[24], v_cos_bit);
    butterfly_dct_post_half(x8 + 32, x8 + 32, x9 + 32, 4);
    butterfly_dct_post_half(x8 + 36, x8 + 36, x9 + 36, 4);
    butterfly_dct_post_half(x8 + 40, x8 + 40, x9 + 40, 4);
    butterfly_dct_post_half(x8 + 44, x8 + 44, x9 + 44, 4);
    butterfly_dct_post_half(x8 + 48, x8 + 48, x9 + 48, 4);
    butterfly_dct_post_half(x8 + 52, x8 + 52, x9 + 52, 4);
    butterfly_dct_post_half(x8 + 56, x8 + 56, x9 + 56, 4);
    butterfly_dct_post_half(x8 + 60, x8 + 60, x9 + 60, 4);

    // stage 10
    int32x4_t x10[64];
    x10[0]  = x9[0];
    x10[4]  = x9[4];
    x10[8]  = x9[8];
    x10[12] = x9[12];
    x10[16] = x9[16];
    x10[20] = x9[20];
    x10[24] = x9[24];
    x10[28] = x9[28];
    w01     = vld1_s32(cospi + 2 * 1);
    butterfly_half_neon(w01, 0, 1, x9[63], x9[32], &x10[32], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 15);
    butterfly_half_neon(w01, 3, 0, x9[35], x9[60], &x10[60], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 9);
    butterfly_half_neon(w01, 0, 1, x9[59], x9[36], &x10[36], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 7);
    butterfly_half_neon(w01, 3, 0, x9[39], x9[56], &x10[56], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 5);
    butterfly_half_neon(w01, 0, 1, x9[55], x9[40], &x10[40], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 11);
    butterfly_half_neon(w01, 3, 0, x9[43], x9[52], &x10[52], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 13);
    butterfly_half_neon(w01, 0, 1, x9[51], x9[44], &x10[44], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 3);
    butterfly_half_neon(w01, 3, 0, x9[47], x9[48], &x10[48], v_cos_bit);

    // stage 11
    output[0]  = x10[0];
    output[1]  = x10[32];
    output[2]  = x10[16];
    output[3]  = x10[48];
    output[4]  = x10[8];
    output[5]  = x10[40];
    output[6]  = x10[24];
    output[7]  = x10[56];
    output[8]  = x10[4];
    output[9]  = x10[36];
    output[10] = x10[20];
    output[11] = x10[52];
    output[12] = x10[12];
    output[13] = x10[44];
    output[14] = x10[28];
    output[15] = x10[60];
}

static inline void highbd_fidentity64_N4_x4_neon(const int32x4_t* input, int32x4_t* output, const int8_t cos_bit) {
    const int32_t   sqrt    = 4 * 5793; // 4 * new_sqrt2
    const int32x4_t newsqrt = vdupq_n_s32(sqrt);

    for (int32_t i = 0; i < 16; i++) {
        int32x4_t temp = vmulq_s32(input[i], newsqrt);
        output[i]      = vrshlq_s32(temp, vdupq_n_s32(-cos_bit));
    }
}

static inline void highbd_fdct64_N4_xn_neon(const int32x4_t* in, int32x4_t* out, int bit, const int howmany) {
    const int stride = 64;
    int       i      = 0;
    do {
        highbd_fdct64_N4_x4_neon(in + i * stride, out + i * stride, bit);
    } while (++i < howmany);
}

static inline void highbd_fidentity64_N4_xn_neon(const int32x4_t* in, int32x4_t* out, int bit, int howmany) {
    const int stride = 64;
    int       i      = 0;
    do {
        highbd_fidentity64_N4_x4_neon(in + i * stride, out + i * stride, bit);
    } while (++i < howmany);
}

static inline void write_buffer_64x64_N4(const int32x4_t* in, int32_t* output) {
    const int32x4_t zero = vdupq_n_s32(0);

    for (int i = 0; i < 16; i++) {
        vst1q_s32(output + i * 64 + 0, in[0 + i]);
        vst1q_s32(output + i * 64 + 4, in[64 + i]);
        vst1q_s32(output + i * 64 + 8, in[2 * 64 + i]);
        vst1q_s32(output + i * 64 + 12, in[3 * 64 + i]);
        for (int j = 4; j < 16; j++) {
            vst1q_s32(output + i * 64 + j * 4, zero);
        }
    }

    memset((output + 16 * 64), 0, (48 * 64) * sizeof(int32_t));
}

static inline void transpose_64x16_in_64x64(const int32x4_t* in, int32x4_t* out) {
    for (int i = 0; i < 16; i++) {
        transpose_elems_s32_4x4(in[0 + i * 64],
                                in[1 + i * 64],
                                in[2 + i * 64],
                                in[3 + i * 64],
                                &out[0 + i * 4],
                                &out[1 + i * 4],
                                &out[2 + i * 4],
                                &out[3 + i * 4]);
        transpose_elems_s32_4x4(in[4 + i * 64],
                                in[5 + i * 64],
                                in[6 + i * 64],
                                in[7 + i * 64],
                                &out[64 + i * 4],
                                &out[65 + i * 4],
                                &out[66 + i * 4],
                                &out[67 + i * 4]);
        transpose_elems_s32_4x4(in[8 + i * 64],
                                in[9 + i * 64],
                                in[10 + i * 64],
                                in[11 + i * 64],
                                &out[128 + i * 4],
                                &out[129 + i * 4],
                                &out[130 + i * 4],
                                &out[131 + i * 4]);
        transpose_elems_s32_4x4(in[12 + i * 64],
                                in[13 + i * 64],
                                in[14 + i * 64],
                                in[15 + i * 64],
                                &out[192 + i * 4],
                                &out[193 + i * 4],
                                &out[194 + i * 4],
                                &out[195 + i * 4]);
    }
}

static inline void transpose_16x16_in_64x64(const int32x4_t* in, int32x4_t* out) {
    for (int i = 0; i < 4; i++) {
        transpose_elems_s32_4x4(in[0 + 0 * 64 + 4 * i],
                                in[1 + 0 * 64 + 4 * i],
                                in[2 + 0 * 64 + 4 * i],
                                in[3 + 0 * 64 + 4 * i],
                                &out[0 + 64 * i],
                                &out[1 + 64 * i],
                                &out[2 + 64 * i],
                                &out[3 + 64 * i]);
        transpose_elems_s32_4x4(in[0 + 1 * 64 + 4 * i],
                                in[1 + 1 * 64 + 4 * i],
                                in[2 + 1 * 64 + 4 * i],
                                in[3 + 1 * 64 + 4 * i],
                                &out[4 + 64 * i],
                                &out[5 + 64 * i],
                                &out[6 + 64 * i],
                                &out[7 + 64 * i]);
        transpose_elems_s32_4x4(in[0 + 2 * 64 + 4 * i],
                                in[1 + 2 * 64 + 4 * i],
                                in[2 + 2 * 64 + 4 * i],
                                in[3 + 2 * 64 + 4 * i],
                                &out[8 + 64 * i],
                                &out[9 + 64 * i],
                                &out[10 + 64 * i],
                                &out[11 + 64 * i]);
        transpose_elems_s32_4x4(in[0 + 3 * 64 + 4 * i],
                                in[1 + 3 * 64 + 4 * i],
                                in[2 + 3 * 64 + 4 * i],
                                in[3 + 3 * 64 + 4 * i],
                                &out[12 + 64 * i],
                                &out[13 + 64 * i],
                                &out[14 + 64 * i],
                                &out[15 + 64 * i]);
    }
}

static AOM_FORCE_INLINE void load_buffer_4x4_N4(const int16_t* input, int32x4_t* in, int stride, int fliplr) {
    if (fliplr) {
        for (int i = 0; i < 4; ++i) {
            int16x4_t a = vld1_s16(input + i * stride);
            a           = vrev64_s16(a);
            in[i]       = vmovl_s16(a);
        }
    } else {
        for (int i = 0; i < 4; ++i) {
            int16x4_t a = vld1_s16(input + i * stride);
            in[i]       = vmovl_s16(a);
        }
    }
}

static inline void shift_right_2_round_s32_64x16_N4(int32x4_t* in, int32x4_t* out) {
    shift_right_2_round_s32_x4(in + 0 * 64, out + 0 * 64, 16);
    shift_right_2_round_s32_x4(in + 1 * 64, out + 1 * 64, 16);
    shift_right_2_round_s32_x4(in + 2 * 64, out + 2 * 64, 16);
    shift_right_2_round_s32_x4(in + 3 * 64, out + 3 * 64, 16);
    shift_right_2_round_s32_x4(in + 4 * 64, out + 4 * 64, 16);
    shift_right_2_round_s32_x4(in + 5 * 64, out + 5 * 64, 16);
    shift_right_2_round_s32_x4(in + 6 * 64, out + 6 * 64, 16);
    shift_right_2_round_s32_x4(in + 7 * 64, out + 7 * 64, 16);
    shift_right_2_round_s32_x4(in + 8 * 64, out + 8 * 64, 16);
    shift_right_2_round_s32_x4(in + 9 * 64, out + 9 * 64, 16);
    shift_right_2_round_s32_x4(in + 10 * 64, out + 10 * 64, 16);
    shift_right_2_round_s32_x4(in + 11 * 64, out + 11 * 64, 16);
    shift_right_2_round_s32_x4(in + 12 * 64, out + 12 * 64, 16);
    shift_right_2_round_s32_x4(in + 13 * 64, out + 13 * 64, 16);
    shift_right_2_round_s32_x4(in + 14 * 64, out + 14 * 64, 16);
    shift_right_2_round_s32_x4(in + 15 * 64, out + 15 * 64, 16);
}

static inline void shift_right_2_round_s32_16x16_N4(int32x4_t* in, int32x4_t* out) {
    shift_right_2_round_s32_x4(in + 0 * 64, out + 0 * 64, 16);
    shift_right_2_round_s32_x4(in + 1 * 64, out + 1 * 64, 16);
    shift_right_2_round_s32_x4(in + 2 * 64, out + 2 * 64, 16);
    shift_right_2_round_s32_x4(in + 3 * 64, out + 3 * 64, 16);
}

static inline void load_buffer_16x16_in_64x64(const int16_t* input, int32x4_t* buf, int stride) {
    load_buffer_4x4_N4(input + 0, buf, stride, 0);
    load_buffer_4x4_N4(input + 4, buf + 64, stride, 0);
    load_buffer_4x4_N4(input + 8, buf + 2 * 64, stride, 0);
    load_buffer_4x4_N4(input + 12, buf + 3 * 64, stride, 0);
    load_buffer_4x4_N4(input + 4 * stride + 0, buf + 4, stride, 0);
    load_buffer_4x4_N4(input + 4 * stride + 4, buf + 4 + 64, stride, 0);
    load_buffer_4x4_N4(input + 4 * stride + 8, buf + 4 + 2 * 64, stride, 0);
    load_buffer_4x4_N4(input + 4 * stride + 12, buf + 4 + 3 * 64, stride, 0);
    load_buffer_4x4_N4(input + 8 * stride + 0, buf + 8, stride, 0);
    load_buffer_4x4_N4(input + 8 * stride + 4, buf + 8 + 64, stride, 0);
    load_buffer_4x4_N4(input + 8 * stride + 8, buf + 8 + 2 * 64, stride, 0);
    load_buffer_4x4_N4(input + 8 * stride + 12, buf + 8 + 3 * 64, stride, 0);
    load_buffer_4x4_N4(input + 12 * stride + 0, buf + 12, stride, 0);
    load_buffer_4x4_N4(input + 12 * stride + 4, buf + 12 + 64, stride, 0);
    load_buffer_4x4_N4(input + 12 * stride + 8, buf + 12 + 2 * 64, stride, 0);
    load_buffer_4x4_N4(input + 12 * stride + 12, buf + 12 + 3 * 64, stride, 0);
}

void svt_av1_fwd_txfm2d_64x64_N4_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;

    switch (tx_type) {
    case DCT_DCT: {
        // Column-wise transform.
        int32x4_t buf0[1024];
        load_buffer_64x64(input, buf0, stride, 0);
        highbd_fdct64_N4_xn_neon(buf0, buf0, 13, 16);
        shift_right_2_round_s32_64x16_N4(buf0, buf0);

        int32x4_t buf1[1024];
        transpose_64x16_in_64x64(buf0, buf1);

        // Row-wise transform.
        highbd_fdct64_N4_xn_neon(buf1, buf0, 10, 4);
        shift_right_2_round_s32_16x16_N4(buf0, buf0);
        transpose_16x16_in_64x64(buf0, buf1);
        write_buffer_64x64_N4(buf1, output);
        break;
    }
    case IDTX: {
        int32x4_t buf0[1024];
        load_buffer_16x16_in_64x64(input, buf0, stride);

        // Column-wise transform.
        int32x4_t buf1[1024];
        highbd_fidentity64_N4_xn_neon(buf0, buf1, 12, 4);
        shift_right_2_round_s32_16x16_N4(buf1, buf1);

        // Row-wise transform.
        highbd_fidentity64_N4_xn_neon(buf1, buf0, 12, 4);
        shift_right_2_round_s32_16x16_N4(buf0, buf0);
        write_buffer_64x64_N4(buf0, output);
        break;
    }
    default:
        assert(0);
    }
}

typedef void (*fwd_transform_1d_neon)(int32x4_t* in, int32x4_t* out, int bit, const int num_cols);

static AOM_FORCE_INLINE void highbd_fdct4_N2_x4_neon(const int32x4_t* in, int32x4_t* out, int bit) {
    const int32_t* const cospi      = cospi_arr_s32(bit);
    const int32x2_t      cospi16_48 = vld1_s32(&cospi[2 * 16]);

    const int32x4_t a0 = vaddq_s32(in[0], in[3]);
    const int32x4_t a1 = vsubq_s32(in[0], in[3]);
    const int32x4_t a2 = vaddq_s32(in[1], in[2]);
    const int32x4_t a3 = vsubq_s32(in[1], in[2]);

    const int32x4_t b0 = vaddq_s32(a0, a2);
    const int32x4_t b3 = vmulq_lane_s32(a3, cospi16_48, 1);

    const int32x4_t c0 = vmulq_n_s32(b0, cospi[2 * 32]);
    const int32x4_t c2 = vmlaq_lane_s32(b3, a1, cospi16_48, 0);

    const int32x4_t v_bit = vdupq_n_s32(-bit);
    const int32x4_t d0    = vrshlq_s32(c0, v_bit);
    const int32x4_t d2    = vrshlq_s32(c2, v_bit);

    out[0] = d0;
    out[1] = d2;
}

static AOM_FORCE_INLINE void highbd_fadst4_N2_x4_neon(const int32x4_t* in, int32x4_t* out, int bit) {
    const int32x4_t sinpi = vld1q_s32(sinpi_arr(bit) + 1);

    const int32x4_t a0 = vaddq_s32(in[0], in[1]);
    const int32x4_t a1 = vmulq_lane_s32(in[0], vget_low_s32(sinpi), 0);

    const int32x4_t b0 = vmlaq_lane_s32(a1, in[1], vget_low_s32(sinpi), 1);
    const int32x4_t b2 = vsubq_s32(a0, in[3]);

    const int32x4_t c0 = vmlaq_lane_s32(b0, in[3], vget_high_s32(sinpi), 1);
    const int32x4_t c2 = vmulq_lane_s32(b2, vget_high_s32(sinpi), 0);

    const int32x4_t d0 = vmlaq_lane_s32(c0, in[2], vget_high_s32(sinpi), 0);

    const int32x4_t v_bit = vdupq_n_s32(-bit);
    const int32x4_t e0    = vrshlq_s32(d0, v_bit);
    const int32x4_t e1    = vrshlq_s32(c2, v_bit);

    out[0] = e0;
    out[1] = e1;
}

static AOM_FORCE_INLINE void highbd_fidentity4_N2_x4_neon(const int32x4_t* in, int32x4_t* out, int bit) {
    (void)bit;
    int32x4_t fact = vdupq_n_s32(new_sqrt2);

    for (int i = 0; i < 2; i++) {
        const int32x4_t a_low = vmulq_s32(in[i], fact);
        out[i]                = vrshrq_n_s32(a_low, new_sqrt2_bits);
    }
}

static inline void write_buffer_4x4_N2(int32x2_t in0, int32x2_t in1, int32_t* out) {
    vst1_s32(out + 0, in0);
    vst1_s32(out + 2, vdup_n_s32(0));
    vst1_s32(out + 4, in1);
    vst1_s32(out + 6, vdup_n_s32(0));

    memset(out + 2 * 4, 0, 2 * 4 * sizeof(int32_t));
}

void svt_av1_fwd_txfm2d_4x4_N2_neon(int16_t* input, int32_t* output, uint32_t input_stride, TxType tx_type,
                                    uint8_t bd) {
    (void)bd;

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &input_stride, 4);

    // Workspace for column/row-wise transforms.
    int32x4_t   buf[4];
    int32x2x2_t buf0;

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fdct4_N2_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fdct4_N2_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        buf0 = vtrn_s32(vget_low_s32(buf[0]), vget_low_s32(buf[1]));
        write_buffer_4x4_N2(buf0.val[0], buf0.val[1], output);
        break;
    case ADST_DCT:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fadst4_N2_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fdct4_N2_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        buf0 = vtrn_s32(vget_low_s32(buf[0]), vget_low_s32(buf[1]));
        write_buffer_4x4_N2(buf0.val[0], buf0.val[1], output);
        break;
    case DCT_ADST:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fdct4_N2_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fadst4_N2_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        buf0 = vtrn_s32(vget_low_s32(buf[0]), vget_low_s32(buf[1]));
        write_buffer_4x4_N2(buf0.val[0], buf0.val[1], output);
        break;
    case ADST_ADST:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fadst4_N2_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fadst4_N2_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        buf0 = vtrn_s32(vget_low_s32(buf[0]), vget_low_s32(buf[1]));
        write_buffer_4x4_N2(buf0.val[0], buf0.val[1], output);
        break;
    case FLIPADST_DCT:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fadst4_N2_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fdct4_N2_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        buf0 = vtrn_s32(vget_low_s32(buf[0]), vget_low_s32(buf[1]));
        write_buffer_4x4_N2(buf0.val[0], buf0.val[1], output);
        break;
    case DCT_FLIPADST:
        load_buffer_4x4(input, buf, input_stride, 1);
        highbd_fdct4_N2_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fadst4_N2_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        buf0 = vtrn_s32(vget_low_s32(buf[0]), vget_low_s32(buf[1]));
        write_buffer_4x4_N2(buf0.val[0], buf0.val[1], output);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_4x4(input, buf, input_stride, 1);
        highbd_fadst4_N2_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fadst4_N2_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        buf0 = vtrn_s32(vget_low_s32(buf[0]), vget_low_s32(buf[1]));
        write_buffer_4x4_N2(buf0.val[0], buf0.val[1], output);
        break;
    case ADST_FLIPADST:
        load_buffer_4x4(input, buf, input_stride, 1);
        highbd_fadst4_N2_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fadst4_N2_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        buf0 = vtrn_s32(vget_low_s32(buf[0]), vget_low_s32(buf[1]));
        write_buffer_4x4_N2(buf0.val[0], buf0.val[1], output);
        break;
    case FLIPADST_ADST:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fadst4_N2_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fadst4_N2_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        buf0 = vtrn_s32(vget_low_s32(buf[0]), vget_low_s32(buf[1]));
        write_buffer_4x4_N2(buf0.val[0], buf0.val[1], output);
        break;
    case IDTX:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fidentity4_N2_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        highbd_fidentity4_N2_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        write_buffer_4x4_N2(vget_low_s32(buf[0]), vget_low_s32(buf[1]), output);
        break;
    case V_DCT:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fdct4_N2_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        highbd_fidentity4_N2_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        write_buffer_4x4_N2(vget_low_s32(buf[0]), vget_low_s32(buf[1]), output);
        break;
    case H_DCT:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fidentity4_N2_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fdct4_N2_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        buf0 = vtrn_s32(vget_low_s32(buf[0]), vget_low_s32(buf[1]));
        write_buffer_4x4_N2(buf0.val[0], buf0.val[1], output);
        break;
    case V_ADST:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fadst4_N2_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        highbd_fidentity4_N2_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        write_buffer_4x4_N2(vget_low_s32(buf[0]), vget_low_s32(buf[1]), output);
        break;
    case H_ADST:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fidentity4_N2_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fadst4_N2_x4_neon(buf, buf, fwd_cos_bit_col[0][0]);
        buf0 = vtrn_s32(vget_low_s32(buf[0]), vget_low_s32(buf[1]));
        write_buffer_4x4_N2(buf0.val[0], buf0.val[1], output);
        break;
    case V_FLIPADST:
        load_buffer_4x4(input, buf, input_stride, 0);
        highbd_fadst4_N2_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        highbd_fidentity4_N2_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        write_buffer_4x4_N2(vget_low_s32(buf[0]), vget_low_s32(buf[1]), output);
        break;
    case H_FLIPADST:
        load_buffer_4x4(input, buf, input_stride, 1);
        highbd_fidentity4_N2_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        transpose_arrays_s32_4x4(buf, buf);
        highbd_fadst4_N2_x4_neon(buf, buf, fwd_cos_bit_row[0][0]);
        buf0 = vtrn_s32(vget_low_s32(buf[0]), vget_low_s32(buf[1]));
        write_buffer_4x4_N2(buf0.val[0], buf0.val[1], output);
        break;
    default:
        assert(0);
    }
}

static AOM_FORCE_INLINE void highbd_fdct8_N2_x4_neon(const int32x4_t* in, int32x4_t* out, int bit) {
    const int32_t* const cospi = cospi_arr_s32(bit);
    const int32x4_t      v_bit = vdupq_n_s32(-bit);

    // stage 1
    int32x4_t a[8];
    butterfly_dct_pre(in, a, 8);

    // stage 2
    int32x4_t b[8];
    butterfly_dct_pre(a, b, 4);
    butterfly_0130_neon(cospi, 32, a[5], a[6], &b[6], &b[5], v_bit);

    // stage 3
    int32x4_t c[8];
    int32x2_t w01 = vld1_s32(cospi + 2 * 32);
    butterfly_half_neon(w01, 0, 1, b[1], b[0], &c[0], v_bit);
    w01 = vld1_s32(cospi + 2 * 16);
    butterfly_half_neon(w01, 0, 1, b[3], b[2], &c[2], v_bit);
    butterfly_dct_post(a + 4, b + 4, c + 4, 4);

    // stage 4-5
    w01 = vld1_s32(cospi + 2 * 8);
    butterfly_half_neon(w01, 0, 1, c[7], c[4], &out[1], v_bit);
    w01 = vld1_s32(cospi + 2 * 24);
    butterfly_half_neon(w01, 3, 0, c[5], c[6], &out[3], v_bit);

    out[0] = c[0];
    out[2] = c[2];
}

static AOM_FORCE_INLINE void highbd_fadst8_N2_x4_neon(const int32x4_t* in, int32x4_t* out, int bit) {
    const int32_t* const cospi = cospi_arr_s32(bit);
    const int32x4_t      v_bit = vdupq_n_s32(-bit);

    int32x4_t u0, u1, u2, u3, u4, u5, u6, u7;
    int32x4_t v0, v1, v2, v3, v4, v5, v6, v7;

    // stage 0-1
    u0 = in[0];
    u1 = in[7];
    u2 = in[3];
    u3 = in[4];
    u4 = in[1];
    u5 = in[6];
    u6 = in[2];
    u7 = in[5];

    // stage 2
    v0 = u0;
    v1 = u1;
    butterfly_cospi32_0222_neon(cospi, u3, u2, &v2, &v3, v_bit);
    v4 = u4;
    v5 = u5;
    butterfly_cospi32_0002_neon(cospi, u6, u7, &v7, &v6, v_bit);

    // stage 3
    u0 = vaddq_s32(v0, v2);
    u1 = vsubq_s32(v3, v1);
    u2 = vsubq_s32(v0, v2);
    u3 = vaddq_s32(v1, v3);
    u4 = vsubq_s32(v6, v4);
    u5 = vaddq_s32(v5, v7);
    u6 = vaddq_s32(v4, v6);
    u7 = vsubq_s32(v5, v7);

    // stage 4
    v0 = u0;
    v1 = u1;
    v2 = u2;
    v3 = u3;

    butterfly_0112_neon(cospi, 16, u4, u5, &v4, &v5, v_bit);
    butterfly_0112_neon(cospi, 16, u7, u6, &v6, &v7, v_bit);

    // stage 5
    u0 = vaddq_s32(v0, v4);
    u1 = vaddq_s32(v1, v5);
    u2 = vaddq_s32(v2, v6);
    u3 = vsubq_s32(v7, v3);
    u4 = vsubq_s32(v0, v4);
    u5 = vsubq_s32(v1, v5);
    u6 = vsubq_s32(v2, v6);
    u7 = vaddq_s32(v3, v7);

    // stage 6
    int32x2_t w01 = vld1_s32(cospi + 2 * 4);
    butterfly_half_neon(w01, 1, 2, u0, u1, &v1, v_bit);
    w01 = vld1_s32(cospi + 2 * 20);
    butterfly_half_neon(w01, 1, 2, u2, u3, &v3, v_bit);
    w01 = vld1_s32(cospi + 2 * 28);
    butterfly_half_neon(w01, 0, 1, u5, u4, &v4, v_bit);
    w01 = vld1_s32(cospi + 2 * 12);
    butterfly_half_neon(w01, 1, 2, u6, u7, &v6, v_bit);

    // stage 7
    out[0] = v1;
    out[1] = v6;
    out[2] = v3;
    out[3] = v4;
}

static AOM_FORCE_INLINE void highbd_fidentity8_N2_x4_neon(const int32x4_t* in, int32x4_t* out, int bit) {
    (void)bit;
    out[0] = vaddq_s32(in[0], in[0]);
    out[1] = vaddq_s32(in[1], in[1]);
    out[2] = vaddq_s32(in[2], in[2]);
    out[3] = vaddq_s32(in[3], in[3]);
}

static AOM_FORCE_INLINE void highbd_fdct8_N2_xn_neon(const int32x4_t* in, int32x4_t* out, int bit, int howmany) {
    const int stride = 8;
    int       i      = 0;
    do {
        highbd_fdct8_N2_x4_neon(in + i * stride, out + i * stride, bit);
    } while (++i < howmany);
}

static AOM_FORCE_INLINE void highbd_fadst8_N2_xn_neon(const int32x4_t* in, int32x4_t* out, int bit, int howmany) {
    const int stride = 8;
    int       i      = 0;
    do {
        highbd_fadst8_N2_x4_neon(in + i * stride, out + i * stride, bit);
    } while (++i < howmany);
}

static AOM_FORCE_INLINE void highbd_fidentity8_N2_xn_neon(const int32x4_t* in, int32x4_t* out, int bit, int howmany) {
    (void)bit;
    const int stride = 8;
    int       i      = 0;
    do {
        highbd_fidentity8_N2_x4_neon(in + i * stride, out + i * stride, bit);
    } while (++i < howmany);
}

static inline void shift_right_1_round_s32_8x4_N2(int32x4_t* in, int32x4_t* out) {
    shift_right_1_round_s32_x4(in + 0, out + 0, 4);
    shift_right_1_round_s32_x4(in + 8, out + 8, 4);
}

static inline void transpose_8x4_in_8x8_N2(int32x4_t* in, int32x4_t* out) {
    transpose_elems_s32_4x4(in[0], in[1], in[2], in[3], &out[0], &out[1], &out[2], &out[3]);
    transpose_elems_s32_4x4(in[8], in[9], in[10], in[11], &out[4], &out[5], &out[6], &out[7]);
}

static inline void load_buffer_8x4_in_8x8(const int16_t* input, int32x4_t* buf, int stride) {
    load_buffer_4x4(input + 0 * 4, buf + 0 * 8, stride, 0);
    load_buffer_4x4(input + 1 * 4, buf + 1 * 8, stride, 0);
}

static inline void load_buffer_8x4_in_8x8_flip(const int16_t* input, int32x4_t* buf, int stride) {
    load_buffer_4x4(input + 0 * 4, buf + 1 * 8, stride, 1);
    load_buffer_4x4(input + 1 * 4, buf + 0 * 8, stride, 1);
}

static inline void write_buffer_8x8_N2(const int32x4_t* buf, int32_t* output) {
    const int32x4_t zeros = vdupq_n_s32(0);

    for (int i = 0; i < 4; i++) {
        vst1q_s32(output + i * 8 + 0, buf[i]);
        vst1q_s32(output + i * 8 + 4, zeros);
    }

    memset(output + 4 * 8, 0, 4 * 8 * sizeof(int32_t));
}

void svt_av1_fwd_txfm2d_8x8_N2_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 8);

    // Workspaces for column/row-wise transforms.
    int32x4_t buf0[16], buf1[16];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_8x8(input, buf0, stride, 0);
        highbd_fdct8_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_8x4_N2(buf0, buf0);
        transpose_8x4_in_8x8_N2(buf0, buf1);
        highbd_fdct8_N2_x4_neon(buf1, buf1, fwd_cos_bit_row[1][1]);
        transpose_arrays_s32_4x4(buf1, buf0);
        write_buffer_8x8_N2(buf0, output);
        break;
    case ADST_DCT:
        load_buffer_8x8(input, buf0, stride, 0);
        highbd_fadst8_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_8x4_N2(buf0, buf0);
        transpose_8x4_in_8x8_N2(buf0, buf1);
        highbd_fdct8_N2_xn_neon(buf1, buf1, fwd_cos_bit_row[1][1], 2);
        transpose_arrays_s32_4x4(buf1, buf0);
        write_buffer_8x8_N2(buf0, output);
        break;
    case DCT_ADST:
        load_buffer_8x8(input, buf0, stride, 0);
        highbd_fdct8_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_8x4_N2(buf0, buf0);
        transpose_8x4_in_8x8_N2(buf0, buf1);
        highbd_fadst8_N2_xn_neon(buf1, buf1, fwd_cos_bit_row[1][1], 2);
        transpose_arrays_s32_4x4(buf1, buf0);
        write_buffer_8x8_N2(buf0, output);
        break;
    case ADST_ADST:
        load_buffer_8x8(input, buf0, stride, 0);
        highbd_fadst8_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_8x4_N2(buf0, buf0);
        transpose_8x4_in_8x8_N2(buf0, buf1);
        highbd_fadst8_N2_xn_neon(buf1, buf1, fwd_cos_bit_row[1][1], 2);
        transpose_arrays_s32_4x4(buf1, buf0);
        write_buffer_8x8_N2(buf0, output);
        break;
    case FLIPADST_DCT:
        load_buffer_8x8(input, buf0, stride, 0);
        highbd_fadst8_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_8x4_N2(buf0, buf0);
        transpose_8x4_in_8x8_N2(buf0, buf1);
        highbd_fdct8_N2_xn_neon(buf1, buf1, fwd_cos_bit_row[1][1], 2);
        transpose_arrays_s32_4x4(buf1, buf0);
        write_buffer_8x8_N2(buf0, output);
        break;
    case DCT_FLIPADST:
        load_buffer_8x8(input, buf0, stride, 1);
        highbd_fdct8_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_8x4_N2(buf0, buf0);
        transpose_8x4_in_8x8_N2(buf0, buf1);
        highbd_fadst8_N2_xn_neon(buf1, buf1, fwd_cos_bit_row[1][1], 2);
        transpose_arrays_s32_4x4(buf1, buf0);
        write_buffer_8x8_N2(buf0, output);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_8x8(input, buf0, stride, 1);
        highbd_fadst8_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_8x4_N2(buf0, buf0);
        transpose_8x4_in_8x8_N2(buf0, buf1);
        highbd_fadst8_N2_xn_neon(buf1, buf1, fwd_cos_bit_row[1][1], 2);
        transpose_arrays_s32_4x4(buf1, buf0);
        write_buffer_8x8_N2(buf0, output);
        break;
    case ADST_FLIPADST:
        load_buffer_8x8(input, buf0, stride, 1);
        highbd_fadst8_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_8x4_N2(buf0, buf0);
        transpose_8x4_in_8x8_N2(buf0, buf1);
        highbd_fadst8_N2_xn_neon(buf1, buf1, fwd_cos_bit_row[1][1], 2);
        transpose_arrays_s32_4x4(buf1, buf0);
        write_buffer_8x8_N2(buf0, output);
        break;
    case FLIPADST_ADST:
        load_buffer_8x8(input, buf0, stride, 0);
        highbd_fadst8_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_8x4_N2(buf0, buf0);
        transpose_8x4_in_8x8_N2(buf0, buf1);
        highbd_fadst8_N2_xn_neon(buf1, buf1, fwd_cos_bit_row[1][1], 2);
        transpose_arrays_s32_4x4(buf1, buf0);
        write_buffer_8x8_N2(buf0, output);
        break;
    case IDTX:
        load_buffer_4x4(input, buf0, stride, 0);
        highbd_fidentity8_N2_x4_neon(buf0, buf0, fwd_cos_bit_col[1][1]);
        shift_right_1_round_s32_x4(buf0, buf0, 4);
        highbd_fidentity8_N2_x4_neon(buf0, buf0, fwd_cos_bit_col[1][1]);
        write_buffer_8x8_N2(buf0, output);
        break;
    case V_DCT:
        load_buffer_4x8(input, buf0, stride, 0);
        highbd_fdct8_N2_x4_neon(buf0, buf0, fwd_cos_bit_col[1][1]);
        shift_right_1_round_s32_x4(buf0, buf0, 4);
        highbd_fidentity8_N2_x4_neon(buf0, buf0, fwd_cos_bit_col[1][1]);
        write_buffer_8x8_N2(buf0, output);
        break;
    case H_DCT:
        load_buffer_8x4_in_8x8(input, buf0, stride);
        highbd_fidentity8_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_8x4_N2(buf0, buf0);
        transpose_8x4_in_8x8_N2(buf0, buf1);
        highbd_fdct8_N2_x4_neon(buf1, buf1, fwd_cos_bit_col[1][1]);
        transpose_arrays_s32_4x4(buf1, buf0);
        write_buffer_8x8_N2(buf0, output);
        break;
    case V_ADST:
        load_buffer_4x8(input, buf0, stride, 0);
        highbd_fadst8_N2_x4_neon(buf0, buf0, fwd_cos_bit_col[1][1]);
        shift_right_1_round_s32_x4(buf0, buf0, 4);
        highbd_fidentity8_N2_x4_neon(buf0, buf0, fwd_cos_bit_col[1][1]);
        write_buffer_8x8_N2(buf0, output);
        break;
    case H_ADST:
        load_buffer_8x4_in_8x8(input, buf0, stride);
        highbd_fidentity8_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_8x4_N2(buf0, buf0);
        transpose_8x4_in_8x8_N2(buf0, buf1);
        highbd_fadst8_N2_x4_neon(buf1, buf1, fwd_cos_bit_col[1][1]);
        transpose_arrays_s32_4x4(buf1, buf0);
        write_buffer_8x8_N2(buf0, output);
        break;
    case V_FLIPADST:
        load_buffer_4x8(input, buf0, stride, 0);
        highbd_fadst8_N2_x4_neon(buf0, buf0, fwd_cos_bit_col[1][1]);
        shift_right_1_round_s32_x4(buf0, buf0, 4);
        highbd_fidentity8_N2_x4_neon(buf0, buf0, fwd_cos_bit_col[1][1]);
        write_buffer_8x8_N2(buf0, output);
        break;
    case H_FLIPADST:
        load_buffer_8x4_in_8x8_flip(input, buf0, stride);
        highbd_fidentity8_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[1][1], 2);
        shift_right_1_round_s32_8x4_N2(buf0, buf0);
        transpose_8x4_in_8x8_N2(buf0, buf1);
        highbd_fadst8_N2_x4_neon(buf1, buf1, fwd_cos_bit_col[1][1]);
        transpose_arrays_s32_4x4(buf1, buf0);
        write_buffer_8x8_N2(buf0, output);
        break;
    default:
        assert(0);
    }
}

static inline void highbd_fdct16_N2_x4_neon(const int32x4_t* in, int32x4_t* out, int bit) {
    const int32_t* const cospi = cospi_arr_s32(bit);
    const int32x4_t      v_bit = vdupq_n_s32(-bit);

    int32x4_t u[16], v[16];

    // stage 1
    butterfly_dct_pre(in, u, 16);

    // stage 2
    butterfly_dct_pre(u, v, 8);
    v[8] = u[8];
    v[9] = u[9];
    butterfly_cospi32_0002_neon(cospi, u[13], u[10], &v[13], &v[10], v_bit);
    butterfly_cospi32_0002_neon(cospi, u[12], u[11], &v[12], &v[11], v_bit);
    v[14] = u[14];
    v[15] = u[15];

    // stage 3
    butterfly_dct_pre(v, u, 4);
    u[4] = v[4];
    butterfly_cospi32_0002_neon(cospi, v[6], v[5], &u[6], &u[5], v_bit);
    u[7] = v[7];
    butterfly_dct_post(v + 8, v + 8, u + 8, 8);

    // stage 4
    int32x2_t w01 = vld1_s32(cospi + 2 * 32);
    butterfly_half_neon(w01, 0, 0, u[0], u[1], &v[0], v_bit);
    w01 = vld1_s32(cospi + 2 * 16);
    butterfly_half_neon(w01, 0, 1, u[3], u[2], &v[2], v_bit);
    butterfly_dct_post(u + 4, u + 4, v + 4, 4);
    v[8] = u[8];
    butterfly_0112_neon(cospi, 16, u[14], u[9], &v[14], &v[9], v_bit);
    butterfly_2312_neon(cospi, 16, u[13], u[10], &v[10], &v[13], v_bit);
    v[11] = u[11];
    v[12] = u[12];
    v[15] = u[15];

    // stage 5
    u[0] = v[0];
    u[2] = v[2];
    w01  = vld1_s32(cospi + 2 * 8);
    butterfly_half_neon(w01, 0, 1, v[7], v[4], &u[4], v_bit);
    w01 = vld1_s32(cospi + 2 * 24);
    butterfly_half_neon(w01, 3, 0, v[5], v[6], &u[6], v_bit);
    butterfly_dct_post(v + 8, v + 8, u + 8, 4);
    butterfly_dct_post(v + 12, v + 12, u + 12, 4);

    // stage 6
    v[0] = u[0];
    v[2] = u[2];
    v[4] = u[4];
    v[6] = u[6];
    w01  = vld1_s32(cospi + 2 * 4);
    butterfly_half_neon(w01, 0, 1, u[15], u[8], &v[8], v_bit);
    w01 = vld1_s32(cospi + 2 * 28);
    butterfly_half_neon(w01, 3, 0, u[9], u[14], &v[14], v_bit);
    w01 = vld1_s32(cospi + 2 * 20);
    butterfly_half_neon(w01, 0, 1, u[13], u[10], &v[10], v_bit);
    w01 = vld1_s32(cospi + 2 * 12);
    butterfly_half_neon(w01, 3, 0, u[11], u[12], &v[12], v_bit);

    out[0] = v[0];
    out[1] = v[8];
    out[2] = v[4];
    out[3] = v[12];
    out[4] = v[2];
    out[5] = v[10];
    out[6] = v[6];
    out[7] = v[14];
}

static inline void highbd_fadst16_N2_x4_neon(const int32x4_t* in, int32x4_t* out, int bit) {
    const int32_t* const cospi = cospi_arr_s32(bit);
    const int32x4_t      v_bit = vdupq_n_s32(-bit);

    int32x4_t u[16], v[16];

    // stage 0-1
    u[0]  = in[0];
    u[1]  = in[15];
    u[2]  = in[7];
    u[3]  = in[8];
    u[4]  = in[3];
    u[5]  = in[12];
    u[6]  = in[4];
    u[7]  = in[11];
    u[8]  = in[1];
    u[9]  = in[14];
    u[10] = in[6];
    u[11] = in[9];
    u[12] = in[2];
    u[13] = in[13];
    u[14] = in[5];
    u[15] = in[10];

    // stage 2
    v[0] = u[0];
    v[1] = u[1];
    butterfly_cospi32_0222_neon(cospi, u[3], u[2], &v[2], &v[3], v_bit);
    v[4] = u[4];
    v[5] = u[5];
    butterfly_cospi32_0002_neon(cospi, u[6], u[7], &v[7], &v[6], v_bit);
    v[8] = u[8];
    v[9] = u[9];
    butterfly_cospi32_0002_neon(cospi, u[10], u[11], &v[11], &v[10], v_bit);
    v[12] = u[12];
    v[13] = u[13];
    butterfly_cospi32_0222_neon(cospi, u[15], u[14], &v[14], &v[15], v_bit);

    // stage 3
    u[0]  = vaddq_s32(v[0], v[2]);
    u[1]  = vsubq_s32(v[3], v[1]);
    u[2]  = vsubq_s32(v[0], v[2]);
    u[3]  = vaddq_s32(v[1], v[3]);
    u[4]  = vsubq_s32(v[6], v[4]);
    u[5]  = vaddq_s32(v[5], v[7]);
    u[6]  = vaddq_s32(v[4], v[6]);
    u[7]  = vsubq_s32(v[5], v[7]);
    u[8]  = vsubq_s32(v[10], v[8]);
    u[9]  = vaddq_s32(v[9], v[11]);
    u[10] = vaddq_s32(v[8], v[10]);
    u[11] = vsubq_s32(v[9], v[11]);
    u[12] = vaddq_s32(v[12], v[14]);
    u[13] = vsubq_s32(v[15], v[13]);
    u[14] = vsubq_s32(v[12], v[14]);
    u[15] = vaddq_s32(v[13], v[15]);

    // stage 4
    v[0] = u[0];
    v[1] = u[1];
    v[2] = u[2];
    v[3] = u[3];
    butterfly_0112_neon(cospi, 16, u[4], u[5], &v[4], &v[5], v_bit);
    butterfly_0112_neon(cospi, 16, u[7], u[6], &v[6], &v[7], v_bit);

    v[8]  = u[8];
    v[9]  = u[9];
    v[10] = u[10];
    v[11] = u[11];

    butterfly_0112_neon(cospi, 16, u[12], u[13], &v[12], &v[13], v_bit);
    butterfly_0332_neon(cospi, 16, u[14], u[15], &v[15], &v[14], v_bit);

    // stage 5
    u[0]  = vaddq_s32(v[0], v[4]);
    u[1]  = vaddq_s32(v[1], v[5]);
    u[2]  = vaddq_s32(v[2], v[6]);
    u[3]  = vsubq_s32(v[7], v[3]);
    u[4]  = vsubq_s32(v[0], v[4]);
    u[5]  = vsubq_s32(v[1], v[5]);
    u[6]  = vsubq_s32(v[2], v[6]);
    u[7]  = vaddq_s32(v[3], v[7]);
    u[8]  = vaddq_s32(v[8], v[12]);
    u[9]  = vaddq_s32(v[9], v[13]);
    u[10] = vsubq_s32(v[14], v[10]);
    u[11] = vaddq_s32(v[11], v[15]);
    u[12] = vsubq_s32(v[8], v[12]);
    u[13] = vsubq_s32(v[9], v[13]);
    u[14] = vaddq_s32(v[10], v[14]);
    u[15] = vsubq_s32(v[11], v[15]);

    // stage 6
    v[0] = u[0];
    v[1] = u[1];
    v[2] = u[2];
    v[3] = u[3];
    v[4] = u[4];
    v[5] = u[5];
    v[6] = u[6];
    v[7] = u[7];

    butterfly_0112_neon(cospi, 8, u[8], u[9], &v[8], &v[9], v_bit);
    butterfly_0130_neon(cospi, 8, u[12], u[13], &v[13], &v[12], v_bit);
    butterfly_0130_neon(cospi, 24, u[11], u[10], &v[10], &v[11], v_bit);
    butterfly_0130_neon(cospi, 24, u[14], u[15], &v[14], &v[15], v_bit);

    // stage 7
    u[0]  = vaddq_s32(v[0], v[8]);
    u[1]  = vaddq_s32(v[1], v[9]);
    u[2]  = vaddq_s32(v[2], v[10]);
    u[3]  = vaddq_s32(v[3], v[11]);
    u[4]  = vaddq_s32(v[4], v[12]);
    u[5]  = vaddq_s32(v[5], v[13]);
    u[6]  = vaddq_s32(v[6], v[14]);
    u[7]  = vsubq_s32(v[15], v[7]);
    u[8]  = vsubq_s32(v[0], v[8]);
    u[9]  = vsubq_s32(v[1], v[9]);
    u[10] = vsubq_s32(v[2], v[10]);
    u[11] = vsubq_s32(v[3], v[11]);
    u[12] = vsubq_s32(v[4], v[12]);
    u[13] = vsubq_s32(v[5], v[13]);
    u[14] = vsubq_s32(v[6], v[14]);
    u[15] = vaddq_s32(v[7], v[15]);

    // stage 8
    int32x2_t w01 = vld1_s32(cospi + 2 * 2);
    butterfly_half_neon(w01, 1, 2, u[0], u[1], &v[1], v_bit);
    w01 = vld1_s32(cospi + 2 * 10);
    butterfly_half_neon(w01, 1, 2, u[2], u[3], &v[3], v_bit);
    w01 = vld1_s32(cospi + 2 * 18);
    butterfly_half_neon(w01, 1, 2, u[4], u[5], &v[5], v_bit);
    w01 = vld1_s32(cospi + 2 * 26);
    butterfly_half_neon(w01, 1, 2, u[6], u[7], &v[7], v_bit);
    w01 = vld1_s32(cospi + 2 * 30);
    butterfly_half_neon(w01, 0, 1, u[9], u[8], &v[8], v_bit);
    w01 = vld1_s32(cospi + 2 * 22);
    butterfly_half_neon(w01, 0, 1, u[11], u[10], &v[10], v_bit);
    w01 = vld1_s32(cospi + 2 * 14);
    butterfly_half_neon(w01, 0, 1, u[13], u[12], &v[12], v_bit);
    w01 = vld1_s32(cospi + 2 * 6);
    butterfly_half_neon(w01, 1, 2, u[14], u[15], &v[14], v_bit);

    // stage 9
    out[0] = v[1];
    out[1] = v[14];
    out[2] = v[3];
    out[3] = v[12];
    out[4] = v[5];
    out[5] = v[10];
    out[6] = v[7];
    out[7] = v[8];
}

static inline void highbd_fidentity16_N2_x4_neon(const int32x4_t* in, int32x4_t* out, int bit) {
    (void)bit;
    const int32x4_t fact = vdupq_n_s32(2 * new_sqrt2);

    for (int i = 0; i < 8; i++) {
        int32x4_t a = vmulq_s32(in[i], fact);
        out[i]      = vrshrq_n_s32(a, new_sqrt2_bits);
    }
}

static inline void highbd_fdct16_N2_xn_neon(const int32x4_t* in, int32x4_t* out, int bit, const int howmany) {
    const int stride = 16;
    int       i      = 0;
    do {
        highbd_fdct16_N2_x4_neon(in + i * stride, out + i * stride, bit);
    } while (++i < howmany);
}

static inline void highbd_fadst16_N2_xn_neon(const int32x4_t* in, int32x4_t* out, int bit, int howmany) {
    const int stride = 16;
    int       i      = 0;
    do {
        highbd_fadst16_N2_x4_neon(in + i * stride, out + i * stride, bit);
    } while (++i < howmany);
}

static inline void highbd_fidentity16_N2_xn_neon(const int32x4_t* in, int32x4_t* out, int bit, int howmany) {
    const int stride = 16;
    int       i      = 0;
    do {
        highbd_fidentity16_N2_x4_neon(in + i * stride, out + i * stride, bit);
    } while (++i < howmany);
}

static inline void write_buffer_16x16_N2(const int32x4_t* buf, int32_t* output) {
    const int32x4_t zeros = vdupq_n_s32(0);

    for (int i = 0; i < 8; i++) {
        vst1q_s32(output + i * 16 + 0, buf[i + 0]);
        vst1q_s32(output + i * 16 + 4, buf[i + 16]);

        vst1q_s32(output + i * 16 + 8, zeros);
        vst1q_s32(output + i * 16 + 12, zeros);
    }

    memset(output + 8 * 16, 0, 8 * 16 * sizeof(int32_t));
}

static inline void shift_right_2_round_s32_16x8_N2(int32x4_t* in, int32x4_t* out) {
    shift_right_2_round_s32_x4(in + 0 * 16, out + 0 * 16, 8);
    shift_right_2_round_s32_x4(in + 1 * 16, out + 1 * 16, 8);
    shift_right_2_round_s32_x4(in + 2 * 16, out + 2 * 16, 8);
    shift_right_2_round_s32_x4(in + 3 * 16, out + 3 * 16, 8);
}

static inline void transpose_wx8_in_16x16(int32x4_t* in, int32x4_t* out, int width) {
    for (int i = 0; i < width; i++) {
        transpose_elems_s32_4x4(in[i * 16 + 0],
                                in[i * 16 + 1],
                                in[i * 16 + 2],
                                in[i * 16 + 3],
                                &out[i * 4 + 0],
                                &out[i * 4 + 1],
                                &out[i * 4 + 2],
                                &out[i * 4 + 3]);
        transpose_elems_s32_4x4(in[i * 16 + 4],
                                in[i * 16 + 5],
                                in[i * 16 + 6],
                                in[i * 16 + 7],
                                &out[i * 4 + 16],
                                &out[i * 4 + 17],
                                &out[i * 4 + 18],
                                &out[i * 4 + 19]);
    }
}

static inline void load_buffer_8x8_in_16x16(const int16_t* input, int32x4_t* buf, int stride) {
    load_buffer_4x8(input, buf, stride, 0);
    load_buffer_4x8(input + 4, buf + 16, stride, 0);
}

static inline void load_buffer_8x16_in_16x16(const int16_t* input, int32x4_t* buf, int stride) {
    load_buffer_4x16(input, buf, stride, 0);
    load_buffer_4x16(input + 4, buf + 16, stride, 0);
}

static inline void load_buffer_16x8_in_16x16(const int16_t* input, int32x4_t* buf, int stride) {
    load_buffer_4x8(input + 0 * 4, buf + 0 * 16, stride, 0);
    load_buffer_4x8(input + 1 * 4, buf + 1 * 16, stride, 0);
    load_buffer_4x8(input + 2 * 4, buf + 2 * 16, stride, 0);
    load_buffer_4x8(input + 3 * 4, buf + 3 * 16, stride, 0);
}

static inline void load_buffer_16x8_in_16x16_flip(const int16_t* input, int32x4_t* buf, int stride) {
    load_buffer_4x8(input + 0 * 4, buf + 3 * 16, stride, 1);
    load_buffer_4x8(input + 1 * 4, buf + 2 * 16, stride, 1);
    load_buffer_4x8(input + 2 * 4, buf + 1 * 16, stride, 1);
    load_buffer_4x8(input + 3 * 4, buf + 0 * 16, stride, 1);
}

void svt_av1_fwd_txfm2d_16x16_N2_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 16);

    // Workspaces for column/row-wise transforms.
    int32x4_t buf0[64], buf1[64];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_16x16(input, buf0, stride, 0);
        highbd_fdct16_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_16x8_N2(buf0, buf0);
        transpose_wx8_in_16x16(buf0, buf1, 4);
        highbd_fdct16_N2_xn_neon(buf1, buf1, fwd_cos_bit_row[2][2], 2);
        transpose_wx8_in_16x16(buf1, buf0, 2);
        write_buffer_16x16_N2(buf0, output);
        break;
    case ADST_DCT:
        load_buffer_16x16(input, buf0, stride, 0);
        highbd_fadst16_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_16x8_N2(buf0, buf0);
        transpose_wx8_in_16x16(buf0, buf1, 4);
        highbd_fdct16_N2_xn_neon(buf1, buf1, fwd_cos_bit_row[2][2], 2);
        transpose_wx8_in_16x16(buf1, buf0, 2);
        write_buffer_16x16_N2(buf0, output);
        break;
    case DCT_ADST:
        load_buffer_16x16(input, buf0, stride, 0);
        highbd_fdct16_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_16x8_N2(buf0, buf0);
        transpose_wx8_in_16x16(buf0, buf1, 4);
        highbd_fadst16_N2_xn_neon(buf1, buf1, fwd_cos_bit_row[2][2], 2);
        transpose_wx8_in_16x16(buf1, buf0, 2);
        write_buffer_16x16_N2(buf0, output);
        break;
    case ADST_ADST:
        load_buffer_16x16(input, buf0, stride, 0);
        highbd_fadst16_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_16x8_N2(buf0, buf0);
        transpose_wx8_in_16x16(buf0, buf1, 4);
        highbd_fadst16_N2_xn_neon(buf1, buf1, fwd_cos_bit_row[2][2], 2);
        transpose_wx8_in_16x16(buf1, buf0, 2);
        write_buffer_16x16_N2(buf0, output);
        break;
    case FLIPADST_DCT:
        load_buffer_16x16(input, buf0, stride, 0);
        highbd_fadst16_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_16x8_N2(buf0, buf0);
        transpose_wx8_in_16x16(buf0, buf1, 4);
        highbd_fdct16_N2_xn_neon(buf1, buf1, fwd_cos_bit_row[2][2], 2);
        transpose_wx8_in_16x16(buf1, buf0, 2);
        write_buffer_16x16_N2(buf0, output);
        break;
    case DCT_FLIPADST:
        load_buffer_16x16(input, buf0, stride, 1);
        highbd_fdct16_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_16x8_N2(buf0, buf0);
        transpose_wx8_in_16x16(buf0, buf1, 4);
        highbd_fadst16_N2_xn_neon(buf1, buf1, fwd_cos_bit_row[2][2], 2);
        transpose_wx8_in_16x16(buf1, buf0, 2);
        write_buffer_16x16_N2(buf0, output);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_16x16(input, buf0, stride, 1);
        highbd_fadst16_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_16x8_N2(buf0, buf0);
        transpose_wx8_in_16x16(buf0, buf1, 4);
        highbd_fadst16_N2_xn_neon(buf1, buf1, fwd_cos_bit_row[2][2], 2);
        transpose_wx8_in_16x16(buf1, buf0, 2);
        write_buffer_16x16_N2(buf0, output);
        break;
    case ADST_FLIPADST:
        load_buffer_16x16(input, buf0, stride, 1);
        highbd_fadst16_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_16x8_N2(buf0, buf0);
        transpose_wx8_in_16x16(buf0, buf1, 4);
        highbd_fadst16_N2_xn_neon(buf1, buf1, fwd_cos_bit_row[2][2], 2);
        transpose_wx8_in_16x16(buf1, buf0, 2);
        write_buffer_16x16_N2(buf0, output);
        break;
    case FLIPADST_ADST:
        load_buffer_16x16(input, buf0, stride, 0);
        highbd_fadst16_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_16x8_N2(buf0, buf0);
        transpose_wx8_in_16x16(buf0, buf1, 4);
        highbd_fadst16_N2_xn_neon(buf1, buf1, fwd_cos_bit_row[2][2], 2);
        transpose_wx8_in_16x16(buf1, buf0, 2);
        write_buffer_16x16_N2(buf0, output);
        break;
    case IDTX:
        load_buffer_8x8_in_16x16(input, buf0, stride);
        highbd_fidentity16_N2_xn_neon(buf0, buf1, fwd_cos_bit_col[2][2], 2);
        shift_right_2_round_s32_x4(buf1, buf1, 8);
        shift_right_2_round_s32_x4(buf1 + 16, buf1 + 16, 8);
        highbd_fidentity16_N2_xn_neon(buf1, buf0, fwd_cos_bit_row[2][2], 2);
        write_buffer_16x16_N2(buf0, output);
        break;
    case V_DCT:
        load_buffer_8x16_in_16x16(input, buf0, stride);
        highbd_fdct16_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 2);
        shift_right_2_round_s32_x4(buf0, buf0, 8);
        shift_right_2_round_s32_x4(buf0 + 16, buf0 + 16, 8);
        highbd_fidentity16_N2_xn_neon(buf0, buf0, fwd_cos_bit_row[2][2], 2);
        write_buffer_16x16_N2(buf0, output);
        break;
    case H_DCT:
        load_buffer_16x8_in_16x16(input, buf0, stride);
        highbd_fidentity16_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_16x8_N2(buf0, buf0);
        transpose_wx8_in_16x16(buf0, buf1, 4);
        highbd_fdct16_N2_xn_neon(buf1, buf1, fwd_cos_bit_row[2][2], 2);
        transpose_wx8_in_16x16(buf1, buf0, 2);
        write_buffer_16x16_N2(buf0, output);
        break;
    case V_ADST:
        load_buffer_8x16_in_16x16(input, buf0, stride);
        highbd_fadst16_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 2);
        shift_right_2_round_s32_x4(buf0, buf0, 8);
        shift_right_2_round_s32_x4(buf0 + 16, buf0 + 16, 8);
        highbd_fidentity16_N2_xn_neon(buf0, buf0, fwd_cos_bit_row[2][2], 2);
        write_buffer_16x16_N2(buf0, output);
        break;
    case H_ADST:
        load_buffer_16x8_in_16x16(input, buf0, stride);
        highbd_fidentity16_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_16x8_N2(buf0, buf0);
        transpose_wx8_in_16x16(buf0, buf1, 4);
        highbd_fadst16_N2_xn_neon(buf1, buf1, fwd_cos_bit_row[2][2], 2);
        transpose_wx8_in_16x16(buf1, buf0, 2);
        write_buffer_16x16_N2(buf0, output);
        break;
    case V_FLIPADST:
        load_buffer_8x16_in_16x16(input, buf0, stride);
        highbd_fadst16_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 2);
        shift_right_2_round_s32_x4(buf0, buf0, 8);
        shift_right_2_round_s32_x4(buf0 + 16, buf0 + 16, 8);
        highbd_fidentity16_N2_xn_neon(buf0, buf0, fwd_cos_bit_row[2][2], 2);
        write_buffer_16x16_N2(buf0, output);
        break;
    case H_FLIPADST:
        load_buffer_16x8_in_16x16_flip(input, buf0, stride);
        highbd_fidentity16_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[2][2], 4);
        shift_right_2_round_s32_16x8_N2(buf0, buf0);
        transpose_wx8_in_16x16(buf0, buf1, 4);
        highbd_fadst16_N2_xn_neon(buf1, buf1, fwd_cos_bit_row[2][2], 2);
        transpose_wx8_in_16x16(buf1, buf0, 2);
        write_buffer_16x16_N2(buf0, output);
        break;
    default:
        assert(0);
    }
}

static inline void highbd_fdct32_N2_x4_neon(const int32x4_t* input, int32x4_t* output, int cos_bit) {
    const int32_t* const cospi     = cospi_arr_s32(cos_bit);
    const int32x4_t      v_cos_bit = vdupq_n_s32(-cos_bit);

    // Workspaces for intermediate transform steps.
    int32x4_t buf0[32];
    int32x4_t buf1[32];

    // stage 1
    butterfly_dct_pre(input, buf1, 32);

    // stage 2
    butterfly_dct_pre(buf1, buf0, 16);
    buf0[16] = buf1[16];
    buf0[17] = buf1[17];
    buf0[18] = buf1[18];
    buf0[19] = buf1[19];
    butterfly_0112_neon(cospi, 32, buf1[27], buf1[20], &buf0[27], &buf0[20], v_cos_bit);
    butterfly_0112_neon(cospi, 32, buf1[26], buf1[21], &buf0[26], &buf0[21], v_cos_bit);
    butterfly_0112_neon(cospi, 32, buf1[25], buf1[22], &buf0[25], &buf0[22], v_cos_bit);
    butterfly_0112_neon(cospi, 32, buf1[24], buf1[23], &buf0[24], &buf0[23], v_cos_bit);
    buf0[28] = buf1[28];
    buf0[29] = buf1[29];
    buf0[30] = buf1[30];
    buf0[31] = buf1[31];

    // stage 3
    butterfly_dct_pre(buf0, buf1, 8);
    buf1[8] = buf0[8];
    buf1[9] = buf0[9];
    butterfly_0112_neon(cospi, 32, buf0[13], buf0[10], &buf1[13], &buf1[10], v_cos_bit);
    butterfly_0112_neon(cospi, 32, buf0[12], buf0[11], &buf1[12], &buf1[11], v_cos_bit);
    buf1[14] = buf0[14];
    buf1[15] = buf0[15];
    butterfly_dct_post(buf0 + 16, buf0 + 16, buf1 + 16, 16);

    // stage 4
    butterfly_dct_pre(buf1, buf0, 4);
    buf0[4] = buf1[4];
    butterfly_0112_neon(cospi, 32, buf1[6], buf1[5], &buf0[6], &buf0[5], v_cos_bit);
    buf0[7] = buf1[7];
    butterfly_dct_post(buf1 + 8, buf1 + 8, buf0 + 8, 8);
    buf0[16] = buf1[16];
    buf0[17] = buf1[17];
    butterfly_0112_neon(cospi, 16, buf1[29], buf1[18], &buf0[29], &buf0[18], v_cos_bit);
    butterfly_0112_neon(cospi, 16, buf1[28], buf1[19], &buf0[28], &buf0[19], v_cos_bit);
    butterfly_2312_neon(cospi, 16, buf1[27], buf1[20], &buf0[20], &buf0[27], v_cos_bit);
    butterfly_2312_neon(cospi, 16, buf1[26], buf1[21], &buf0[21], &buf0[26], v_cos_bit);
    buf0[22] = buf1[22];
    buf0[23] = buf1[23];
    buf0[24] = buf1[24];
    buf0[25] = buf1[25];
    buf0[30] = buf1[30];
    buf0[31] = buf1[31];

    // stage 5
    int32x2_t w01 = vld1_s32(cospi + 2 * 32);
    butterfly_half_neon(w01, 0, 1, buf0[0], buf0[1], &buf1[0], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 16);
    butterfly_half_neon(w01, 0, 1, buf0[3], buf0[2], &buf1[2], v_cos_bit);
    butterfly_dct_post(buf0 + 4, buf0 + 4, buf1 + 4, 4);
    buf1[8] = buf0[8];
    butterfly_0112_neon(cospi, 16, buf0[14], buf0[9], &buf1[14], &buf1[9], v_cos_bit);
    butterfly_2312_neon(cospi, 16, buf0[13], buf0[10], &buf1[10], &buf1[13], v_cos_bit);
    buf1[11] = buf0[11];
    buf1[12] = buf0[12];
    buf1[15] = buf0[15];
    butterfly_dct_post(buf0 + 16, buf0 + 16, buf1 + 16, 8);
    butterfly_dct_post(buf0 + 24, buf0 + 24, buf1 + 24, 8);

    // stage 6
    buf0[0] = buf1[0];
    buf0[2] = buf1[2];

    w01 = vld1_s32(cospi + 2 * 8);
    butterfly_half_neon(w01, 0, 1, buf1[7], buf1[4], &buf0[4], v_cos_bit);
    butterfly_0112_neon(cospi, 8, buf1[30], buf1[17], &buf0[30], &buf0[17], v_cos_bit);
    butterfly_2312_neon(cospi, 8, buf1[29], buf1[18], &buf0[18], &buf0[29], v_cos_bit);
    butterfly_dct_post(buf1 + 8, buf1 + 8, buf0 + 8, 4);
    butterfly_dct_post(buf1 + 12, buf1 + 12, buf0 + 12, 4);
    buf0[16] = buf1[16];
    buf0[19] = buf1[19];
    buf0[20] = buf1[20];

    w01 = vld1_s32(cospi + 2 * 24);
    butterfly_half_neon(w01, 3, 0, buf1[5], buf1[6], &buf0[6], v_cos_bit);
    butterfly_0130_neon(cospi, 24, buf1[21], buf1[26], &buf0[26], &buf0[21], v_cos_bit);
    butterfly_0332_neon(cospi, 24, buf1[25], buf1[22], &buf0[25], &buf0[22], v_cos_bit);

    buf0[23] = buf1[23];
    buf0[24] = buf1[24];
    buf0[27] = buf1[27];
    buf0[28] = buf1[28];
    buf0[31] = buf1[31];

    // stage 7
    buf1[0] = buf0[0];
    buf1[2] = buf0[2];
    buf1[4] = buf0[4];
    buf1[6] = buf0[6];
    w01     = vld1_s32(cospi + 2 * 4);
    butterfly_half_neon(w01, 0, 1, buf0[15], buf0[8], &buf1[8], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 28);
    butterfly_half_neon(w01, 3, 0, buf0[9], buf0[14], &buf1[14], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 20);
    butterfly_half_neon(w01, 0, 1, buf0[13], buf0[10], &buf1[10], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 12);
    butterfly_half_neon(w01, 3, 0, buf0[11], buf0[12], &buf1[12], v_cos_bit);
    butterfly_dct_post(buf0 + 16, buf0 + 16, buf1 + 16, 4);
    butterfly_dct_post(buf0 + 20, buf0 + 20, buf1 + 20, 4);
    butterfly_dct_post(buf0 + 24, buf0 + 24, buf1 + 24, 4);
    butterfly_dct_post(buf0 + 28, buf0 + 28, buf1 + 28, 4);

    // stage 8
    buf0[0]  = buf1[0];
    buf0[2]  = buf1[2];
    buf0[4]  = buf1[4];
    buf0[6]  = buf1[6];
    buf0[8]  = buf1[8];
    buf0[10] = buf1[10];
    buf0[12] = buf1[12];
    buf0[14] = buf1[14];
    w01      = vld1_s32(cospi + 2 * 2);
    butterfly_half_neon(w01, 0, 1, buf1[31], buf1[16], &buf0[16], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 30);
    butterfly_half_neon(w01, 3, 0, buf1[17], buf1[30], &buf0[30], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 18);
    butterfly_half_neon(w01, 0, 1, buf1[29], buf1[18], &buf0[18], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 14);
    butterfly_half_neon(w01, 3, 0, buf1[19], buf1[28], &buf0[28], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 10);
    butterfly_half_neon(w01, 0, 1, buf1[27], buf1[20], &buf0[20], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 22);
    butterfly_half_neon(w01, 3, 0, buf1[21], buf1[26], &buf0[26], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 26);
    butterfly_half_neon(w01, 0, 1, buf1[25], buf1[22], &buf0[22], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 6);
    butterfly_half_neon(w01, 3, 0, buf1[23], buf1[24], &buf0[24], v_cos_bit);

    // stage 9
    output[0]  = buf0[0];
    output[1]  = buf0[16];
    output[2]  = buf0[8];
    output[3]  = buf0[24];
    output[4]  = buf0[4];
    output[5]  = buf0[20];
    output[6]  = buf0[12];
    output[7]  = buf0[28];
    output[8]  = buf0[2];
    output[9]  = buf0[18];
    output[10] = buf0[10];
    output[11] = buf0[26];
    output[12] = buf0[6];
    output[13] = buf0[22];
    output[14] = buf0[14];
    output[15] = buf0[30];
}

static inline void highbd_fidentity32_N2_x4_neon(const int32x4_t* input, int32x4_t* output, int cos_bit) {
    (void)cos_bit;
    for (int i = 0; i < 16; i++) {
        output[i] = vshlq_n_s32(input[i], 2);
    }
}

static inline void highbd_fdct32_N2_xn_neon(const int32x4_t* in, int32x4_t* out, int bit, const int howmany) {
    const int stride = 32;
    int       i      = 0;
    do {
        highbd_fdct32_N2_x4_neon(in + i * stride, out + i * stride, bit);
    } while (++i < howmany);
}

static inline void highbd_fidentity32_N2_xn_neon(const int32x4_t* in, int32x4_t* out, int bit, int howmany) {
    const int stride = 32;
    int       i      = 0;
    do {
        highbd_fidentity32_N2_x4_neon(in + i * stride, out + i * stride, bit);
    } while (++i < howmany);
}

static inline void shift_right_4_round_s32_x4_N2(int32x4_t* in, int32x4_t* out) {
    shift_right_4_round_s32_x4(in + 0 * 32, out + 0 * 32, 16);
    shift_right_4_round_s32_x4(in + 1 * 32, out + 1 * 32, 16);
    shift_right_4_round_s32_x4(in + 2 * 32, out + 2 * 32, 16);
    shift_right_4_round_s32_x4(in + 3 * 32, out + 3 * 32, 16);
    shift_right_4_round_s32_x4(in + 4 * 32, out + 4 * 32, 16);
    shift_right_4_round_s32_x4(in + 5 * 32, out + 5 * 32, 16);
    shift_right_4_round_s32_x4(in + 6 * 32, out + 6 * 32, 16);
    shift_right_4_round_s32_x4(in + 7 * 32, out + 7 * 32, 16);
}

static inline void transpose_wx16_in_32x32(const int32x4_t* in, int32x4_t* out, int width) {
    for (int i = 0; i < width; i++) {
        for (int j = 0; j < 4; j++) {
            transpose_elems_s32_4x4(in[0 + i * 32 + j * 4],
                                    in[1 + i * 32 + j * 4],
                                    in[2 + i * 32 + j * 4],
                                    in[3 + i * 32 + j * 4],
                                    &out[0 + 32 * j + 4 * i],
                                    &out[1 + 32 * j + 4 * i],
                                    &out[2 + 32 * j + 4 * i],
                                    &out[3 + 32 * j + 4 * i]);
        }
    }
}

static inline void write_buffer_32x32_N2(const int32x4_t* in, int32_t* output) {
    const int32x4_t zeros = vdupq_n_s32(0);

    for (int i = 0; i < 16; i++) {
        vst1q_s32(output + i * 32 + 0, in[0 * 32 + i]);
        vst1q_s32(output + i * 32 + 4, in[1 * 32 + i]);
        vst1q_s32(output + i * 32 + 8, in[2 * 32 + i]);
        vst1q_s32(output + i * 32 + 12, in[3 * 32 + i]);
        vst1q_s32(output + i * 32 + 16, zeros);
        vst1q_s32(output + i * 32 + 20, zeros);
        vst1q_s32(output + i * 32 + 24, zeros);
        vst1q_s32(output + i * 32 + 28, zeros);
    }

    memset(output + 16 * 32, 0, 16 * 32 * sizeof(int32_t));
}

static inline void load_buffer_16x16_in_32x32(const int16_t* input, int32x4_t* buf, int stride) {
    load_buffer_4x4(input + 0 * stride + 0, buf + 0 + 0 * 32, stride, 0);
    load_buffer_4x4(input + 0 * stride + 4, buf + 0 + 1 * 32, stride, 0);
    load_buffer_4x4(input + 0 * stride + 8, buf + 0 + 2 * 32, stride, 0);
    load_buffer_4x4(input + 0 * stride + 12, buf + 0 + 3 * 32, stride, 0);
    load_buffer_4x4(input + 4 * stride + 0, buf + 4 + 0 * 32, stride, 0);
    load_buffer_4x4(input + 4 * stride + 4, buf + 4 + 1 * 32, stride, 0);
    load_buffer_4x4(input + 4 * stride + 8, buf + 4 + 2 * 32, stride, 0);
    load_buffer_4x4(input + 4 * stride + 12, buf + 4 + 3 * 32, stride, 0);
    load_buffer_4x4(input + 8 * stride + 0, buf + 8 + 0 * 32, stride, 0);
    load_buffer_4x4(input + 8 * stride + 4, buf + 8 + 1 * 32, stride, 0);
    load_buffer_4x4(input + 8 * stride + 8, buf + 8 + 2 * 32, stride, 0);
    load_buffer_4x4(input + 8 * stride + 12, buf + 8 + 3 * 32, stride, 0);
    load_buffer_4x4(input + 12 * stride + 0, buf + 12 + 0 * 32, stride, 0);
    load_buffer_4x4(input + 12 * stride + 4, buf + 12 + 1 * 32, stride, 0);
    load_buffer_4x4(input + 12 * stride + 8, buf + 12 + 2 * 32, stride, 0);
    load_buffer_4x4(input + 12 * stride + 12, buf + 12 + 3 * 32, stride, 0);
}

static inline void load_buffer_32x16_in_32x32(const int16_t* input, int32x4_t* buf, int stride) {
    for (int i = 0; i < 16; i++) {
        load_buffer_4x4(input + i * stride + 0, buf + i + 0 * 32, stride, 0);
        load_buffer_4x4(input + i * stride + 4, buf + i + 1 * 32, stride, 0);
        load_buffer_4x4(input + i * stride + 8, buf + i + 2 * 32, stride, 0);
        load_buffer_4x4(input + i * stride + 12, buf + i + 3 * 32, stride, 0);
        load_buffer_4x4(input + i * stride + 16, buf + i + 4 * 32, stride, 0);
        load_buffer_4x4(input + i * stride + 20, buf + i + 5 * 32, stride, 0);
        load_buffer_4x4(input + i * stride + 24, buf + i + 6 * 32, stride, 0);
        load_buffer_4x4(input + i * stride + 28, buf + i + 7 * 32, stride, 0);
    }
}

void svt_av1_fwd_txfm2d_32x32_N2_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;

    // Workspaces for column/row-wise transforms.
    int32x4_t buf0[256], buf1[256];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_32x32(input, buf0, stride, 0);
        highbd_fdct32_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[3][3], 8);
        shift_right_4_round_s32_x4_N2(buf0, buf0);
        transpose_wx16_in_32x32(buf0, buf1, 8);
        highbd_fdct32_N2_xn_neon(buf1, buf1, fwd_cos_bit_row[3][3], 4);
        transpose_wx16_in_32x32(buf1, buf0, 4);
        write_buffer_32x32_N2(buf0, output);
        break;
    case IDTX:
        load_buffer_16x16_in_32x32(input, buf0, stride);
        highbd_fidentity32_N2_xn_neon(buf0, buf1, fwd_cos_bit_col[3][3], 4);
        shift_right_4_round_s32_x4(buf1 + 0 * 32, buf1 + 0 * 32, 16);
        shift_right_4_round_s32_x4(buf1 + 1 * 32, buf1 + 1 * 32, 16);
        shift_right_4_round_s32_x4(buf1 + 2 * 32, buf1 + 2 * 32, 16);
        shift_right_4_round_s32_x4(buf1 + 3 * 32, buf1 + 3 * 32, 16);
        highbd_fidentity32_N2_xn_neon(buf1, buf0, fwd_cos_bit_row[3][3], 4);
        write_buffer_32x32_N2(buf0, output);
        break;
    case V_DCT:
        load_buffer_16x32(input, buf0, stride, 0);
        highbd_fdct32_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[3][3], 4);
        shift_right_4_round_s32_x4(buf0 + 0 * 32, buf0 + 0 * 32, 16);
        shift_right_4_round_s32_x4(buf0 + 1 * 32, buf0 + 1 * 32, 16);
        shift_right_4_round_s32_x4(buf0 + 2 * 32, buf0 + 2 * 32, 16);
        shift_right_4_round_s32_x4(buf0 + 3 * 32, buf0 + 3 * 32, 16);
        highbd_fidentity32_N2_xn_neon(buf0, buf0, fwd_cos_bit_row[3][3], 4);
        write_buffer_32x32_N2(buf0, output);
        break;
    case H_DCT:
        load_buffer_32x16_in_32x32(input, buf0, stride);
        highbd_fidentity32_N2_xn_neon(buf0, buf0, fwd_cos_bit_col[3][3], 8);
        shift_right_4_round_s32_x4_N2(buf0, buf0);
        transpose_wx16_in_32x32(buf0, buf1, 8);
        highbd_fdct32_N2_xn_neon(buf1, buf1, fwd_cos_bit_row[3][3], 4);
        transpose_wx16_in_32x32(buf1, buf0, 4);
        write_buffer_32x32_N2(buf0, output);
        break;
    default:
        assert(0);
    }
}

static inline void highbd_fdct64_N2_x4_neon(const int32x4_t* input, int32x4_t* output, int8_t cos_bit) {
    const int32_t* const cospi     = cospi_arr_s32(cos_bit);
    const int32x4_t      v_cos_bit = vdupq_n_s32(-cos_bit);

    // stage 1
    int32x4_t x1[64];
    butterfly_dct_pre(input, x1, 64);

    // stage 2
    int32x4_t x2[64];
    butterfly_dct_pre(x1, x2, 32);
    x2[32] = x1[32];
    x2[33] = x1[33];
    x2[34] = x1[34];
    x2[35] = x1[35];
    x2[36] = x1[36];
    x2[37] = x1[37];
    x2[38] = x1[38];
    x2[39] = x1[39];
    butterfly_0112_neon(cospi, 32, x1[55], x1[40], &x2[55], &x2[40], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x1[54], x1[41], &x2[54], &x2[41], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x1[53], x1[42], &x2[53], &x2[42], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x1[52], x1[43], &x2[52], &x2[43], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x1[51], x1[44], &x2[51], &x2[44], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x1[50], x1[45], &x2[50], &x2[45], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x1[49], x1[46], &x2[49], &x2[46], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x1[48], x1[47], &x2[48], &x2[47], v_cos_bit);
    x2[56] = x1[56];
    x2[57] = x1[57];
    x2[58] = x1[58];
    x2[59] = x1[59];
    x2[60] = x1[60];
    x2[61] = x1[61];
    x2[62] = x1[62];
    x2[63] = x1[63];

    // stage 3
    int32x4_t x3[64];
    butterfly_dct_pre(x2, x3, 16);
    x3[16] = x2[16];
    x3[17] = x2[17];
    x3[18] = x2[18];
    x3[19] = x2[19];
    butterfly_0112_neon(cospi, 32, x2[27], x2[20], &x3[27], &x3[20], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x2[26], x2[21], &x3[26], &x3[21], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x2[25], x2[22], &x3[25], &x3[22], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x2[24], x2[23], &x3[24], &x3[23], v_cos_bit);
    x3[28] = x2[28];
    x3[29] = x2[29];
    x3[30] = x2[30];
    x3[31] = x2[31];
    butterfly_dct_post(x2 + 32, x2 + 32, x3 + 32, 32);

    // stage 4
    int32x4_t x4[64];
    butterfly_dct_pre(x3, x4, 8);
    x4[8] = x3[8];
    x4[9] = x3[9];
    butterfly_0112_neon(cospi, 32, x3[13], x3[10], &x4[13], &x4[10], v_cos_bit);
    butterfly_0112_neon(cospi, 32, x3[12], x3[11], &x4[12], &x4[11], v_cos_bit);
    x4[14] = x3[14];
    x4[15] = x3[15];
    butterfly_dct_post(x3 + 16, x3 + 16, x4 + 16, 16);
    x4[32] = x3[32];
    x4[33] = x3[33];
    x4[34] = x3[34];
    x4[35] = x3[35];
    butterfly_0112_neon(cospi, 16, x3[59], x3[36], &x4[59], &x4[36], v_cos_bit);
    butterfly_0112_neon(cospi, 16, x3[58], x3[37], &x4[58], &x4[37], v_cos_bit);
    butterfly_0112_neon(cospi, 16, x3[57], x3[38], &x4[57], &x4[38], v_cos_bit);
    butterfly_0112_neon(cospi, 16, x3[56], x3[39], &x4[56], &x4[39], v_cos_bit);
    butterfly_2312_neon(cospi, 16, x3[55], x3[40], &x4[40], &x4[55], v_cos_bit);
    butterfly_2312_neon(cospi, 16, x3[54], x3[41], &x4[41], &x4[54], v_cos_bit);
    butterfly_2312_neon(cospi, 16, x3[53], x3[42], &x4[42], &x4[53], v_cos_bit);
    butterfly_2312_neon(cospi, 16, x3[52], x3[43], &x4[43], &x4[52], v_cos_bit);
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
    int32x4_t x5[64];
    butterfly_dct_pre(x4, x5, 4);
    x5[4] = x4[4];
    butterfly_0112_neon(cospi, 32, x4[6], x4[5], &x5[6], &x5[5], v_cos_bit);
    x5[7] = x4[7];
    butterfly_dct_post(x4 + 8, x4 + 8, x5 + 8, 8);
    x5[16] = x4[16];
    x5[17] = x4[17];
    butterfly_0112_neon(cospi, 16, x4[29], x4[18], &x5[29], &x5[18], v_cos_bit);
    butterfly_0112_neon(cospi, 16, x4[28], x4[19], &x5[28], &x5[19], v_cos_bit);
    butterfly_2312_neon(cospi, 16, x4[27], x4[20], &x5[20], &x5[27], v_cos_bit);
    butterfly_2312_neon(cospi, 16, x4[26], x4[21], &x5[21], &x5[26], v_cos_bit);
    x5[22] = x4[22];
    x5[23] = x4[23];
    x5[24] = x4[24];
    x5[25] = x4[25];
    x5[30] = x4[30];
    x5[31] = x4[31];
    butterfly_dct_post(x4 + 32, x4 + 32, x5 + 32, 16);
    butterfly_dct_post(x4 + 48, x4 + 48, x5 + 48, 16);

    // stage 6
    int32x4_t x6[64];
    int32x2_t w01 = vld1_s32(cospi + 2 * 32);
    butterfly_half_neon(w01, 0, 1, x5[0], x5[1], &x6[0], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 16);
    butterfly_half_neon(w01, 0, 1, x5[3], x5[2], &x6[2], v_cos_bit);
    butterfly_dct_post(x5 + 4, x5 + 4, x6 + 4, 4);
    x6[8] = x5[8];
    butterfly_0112_neon(cospi, 16, x5[14], x5[9], &x6[14], &x6[9], v_cos_bit);
    butterfly_2312_neon(cospi, 16, x5[13], x5[10], &x6[10], &x6[13], v_cos_bit);
    x6[11] = x5[11];
    x6[12] = x5[12];
    x6[15] = x5[15];
    butterfly_dct_post(x5 + 16, x5 + 16, x6 + 16, 8);
    butterfly_dct_post(x5 + 24, x5 + 24, x6 + 24, 8);
    x6[32] = x5[32];
    x6[33] = x5[33];
    butterfly_0112_neon(cospi, 8, x5[61], x5[34], &x6[61], &x6[34], v_cos_bit);
    butterfly_0112_neon(cospi, 8, x5[60], x5[35], &x6[60], &x6[35], v_cos_bit);
    butterfly_2312_neon(cospi, 8, x5[59], x5[36], &x6[36], &x6[59], v_cos_bit);
    butterfly_2312_neon(cospi, 8, x5[58], x5[37], &x6[37], &x6[58], v_cos_bit);
    x6[38] = x5[38];
    x6[39] = x5[39];
    x6[40] = x5[40];
    x6[41] = x5[41];
    butterfly_0130_neon(cospi, 24, x5[42], x5[53], &x6[53], &x6[42], v_cos_bit);
    butterfly_0130_neon(cospi, 24, x5[43], x5[52], &x6[52], &x6[43], v_cos_bit);
    butterfly_0332_neon(cospi, 24, x5[51], x5[44], &x6[51], &x6[44], v_cos_bit);
    butterfly_0332_neon(cospi, 24, x5[50], x5[45], &x6[50], &x6[45], v_cos_bit);
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
    int32x4_t x7[64];
    x7[0] = x6[0];
    x7[2] = x6[2];
    w01   = vld1_s32(cospi + 2 * 8);
    butterfly_half_neon(w01, 0, 1, x6[7], x6[4], &x7[4], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 24);
    butterfly_half_neon(w01, 3, 0, x6[5], x6[6], &x7[6], v_cos_bit);
    butterfly_dct_post(x6 + 8, x6 + 8, x7 + 8, 4);
    butterfly_dct_post(x6 + 12, x6 + 12, x7 + 12, 4);
    x7[16] = x6[16];
    butterfly_0112_neon(cospi, 8, x6[30], x6[17], &x7[30], &x7[17], v_cos_bit);
    butterfly_2312_neon(cospi, 8, x6[29], x6[18], &x7[18], &x7[29], v_cos_bit);
    x7[19] = x6[19];
    x7[20] = x6[20];
    butterfly_0130_neon(cospi, 24, x6[21], x6[26], &x7[26], &x7[21], v_cos_bit);
    butterfly_0332_neon(cospi, 24, x6[25], x6[22], &x7[25], &x7[22], v_cos_bit);
    x7[23] = x6[23];
    x7[24] = x6[24];
    x7[27] = x6[27];
    x7[28] = x6[28];
    x7[31] = x6[31];
    butterfly_dct_post(x6 + 32, x6 + 32, x7 + 32, 8);
    butterfly_dct_post(x6 + 40, x6 + 40, x7 + 40, 8);
    butterfly_dct_post(x6 + 48, x6 + 48, x7 + 48, 8);
    butterfly_dct_post(x6 + 56, x6 + 56, x7 + 56, 8);

    // stage 8
    int32x4_t x8[64];
    x8[0] = x7[0];
    x8[2] = x7[2];
    x8[4] = x7[4];
    x8[6] = x7[6];

    w01 = vld1_s32(cospi + 2 * 4);
    butterfly_half_neon(w01, 0, 1, x7[15], x7[8], &x8[8], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 28);
    butterfly_half_neon(w01, 3, 0, x7[9], x7[14], &x8[14], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 20);
    butterfly_half_neon(w01, 0, 1, x7[13], x7[10], &x8[10], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 12);
    butterfly_half_neon(w01, 3, 0, x7[11], x7[12], &x8[12], v_cos_bit);
    butterfly_dct_post(x7 + 16, x7 + 16, x8 + 16, 4);
    butterfly_dct_post(x7 + 20, x7 + 20, x8 + 20, 4);
    butterfly_dct_post(x7 + 24, x7 + 24, x8 + 24, 4);
    butterfly_dct_post(x7 + 28, x7 + 28, x8 + 28, 4);
    x8[32] = x7[32];
    butterfly_0112_neon(cospi, 4, x7[62], x7[33], &x8[62], &x8[33], v_cos_bit);
    butterfly_2312_neon(cospi, 4, x7[61], x7[34], &x8[34], &x8[61], v_cos_bit);
    x8[35] = x7[35];
    x8[36] = x7[36];
    butterfly_0130_neon(cospi, 28, x7[37], x7[58], &x8[58], &x8[37], v_cos_bit);
    butterfly_0332_neon(cospi, 28, x7[57], x7[38], &x8[57], &x8[38], v_cos_bit);
    x8[39] = x7[39];
    x8[40] = x7[40];
    butterfly_0112_neon(cospi, 20, x7[54], x7[41], &x8[54], &x8[41], v_cos_bit);
    butterfly_2312_neon(cospi, 20, x7[53], x7[42], &x8[42], &x8[53], v_cos_bit);
    x8[43] = x7[43];
    x8[44] = x7[44];
    butterfly_0130_neon(cospi, 12, x7[45], x7[50], &x8[50], &x8[45], v_cos_bit);
    butterfly_0332_neon(cospi, 12, x7[49], x7[46], &x8[49], &x8[46], v_cos_bit);
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
    int32x4_t x9[64];
    x9[0]  = x8[0];
    x9[2]  = x8[2];
    x9[4]  = x8[4];
    x9[6]  = x8[6];
    x9[8]  = x8[8];
    x9[10] = x8[10];
    x9[12] = x8[12];
    x9[14] = x8[14];
    w01    = vld1_s32(cospi + 2 * 2);
    butterfly_half_neon(w01, 0, 1, x8[31], x8[16], &x9[16], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 30);
    butterfly_half_neon(w01, 3, 0, x8[17], x8[30], &x9[30], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 18);
    butterfly_half_neon(w01, 0, 1, x8[29], x8[18], &x9[18], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 14);
    butterfly_half_neon(w01, 3, 0, x8[19], x8[28], &x9[28], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 10);
    butterfly_half_neon(w01, 0, 1, x8[27], x8[20], &x9[20], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 22);
    butterfly_half_neon(w01, 3, 0, x8[21], x8[26], &x9[26], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 26);
    butterfly_half_neon(w01, 0, 1, x8[25], x8[22], &x9[22], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 6);
    butterfly_half_neon(w01, 3, 0, x8[23], x8[24], &x9[24], v_cos_bit);
    butterfly_dct_post(x8 + 32, x8 + 32, x9 + 32, 4);
    butterfly_dct_post(x8 + 36, x8 + 36, x9 + 36, 4);
    butterfly_dct_post(x8 + 40, x8 + 40, x9 + 40, 4);
    butterfly_dct_post(x8 + 44, x8 + 44, x9 + 44, 4);
    butterfly_dct_post(x8 + 48, x8 + 48, x9 + 48, 4);
    butterfly_dct_post(x8 + 52, x8 + 52, x9 + 52, 4);
    butterfly_dct_post(x8 + 56, x8 + 56, x9 + 56, 4);
    butterfly_dct_post(x8 + 60, x8 + 60, x9 + 60, 4);

    // stage 10
    int32x4_t x10[64];
    x10[0]  = x9[0];
    x10[2]  = x9[2];
    x10[4]  = x9[4];
    x10[6]  = x9[6];
    x10[8]  = x9[8];
    x10[10] = x9[10];
    x10[12] = x9[12];
    x10[14] = x9[14];
    x10[16] = x9[16];
    x10[18] = x9[18];
    x10[20] = x9[20];
    x10[22] = x9[22];
    x10[24] = x9[24];
    x10[26] = x9[26];
    x10[28] = x9[28];
    x10[30] = x9[30];
    w01     = vld1_s32(cospi + 2 * 1);
    butterfly_half_neon(w01, 0, 1, x9[63], x9[32], &x10[32], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 31);
    butterfly_half_neon(w01, 3, 0, x9[33], x9[62], &x10[62], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 17);
    butterfly_half_neon(w01, 0, 1, x9[61], x9[34], &x10[34], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 15);
    butterfly_half_neon(w01, 3, 0, x9[35], x9[60], &x10[60], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 9);
    butterfly_half_neon(w01, 0, 1, x9[59], x9[36], &x10[36], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 23);
    butterfly_half_neon(w01, 3, 0, x9[37], x9[58], &x10[58], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 25);
    butterfly_half_neon(w01, 0, 1, x9[57], x9[38], &x10[38], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 7);
    butterfly_half_neon(w01, 3, 0, x9[39], x9[56], &x10[56], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 5);
    butterfly_half_neon(w01, 0, 1, x9[55], x9[40], &x10[40], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 27);
    butterfly_half_neon(w01, 3, 0, x9[41], x9[54], &x10[54], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 21);
    butterfly_half_neon(w01, 0, 1, x9[53], x9[42], &x10[42], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 11);
    butterfly_half_neon(w01, 3, 0, x9[43], x9[52], &x10[52], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 13);
    butterfly_half_neon(w01, 0, 1, x9[51], x9[44], &x10[44], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 19);
    butterfly_half_neon(w01, 3, 0, x9[45], x9[50], &x10[50], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 29);
    butterfly_half_neon(w01, 0, 1, x9[49], x9[46], &x10[46], v_cos_bit);
    w01 = vld1_s32(cospi + 2 * 3);
    butterfly_half_neon(w01, 3, 0, x9[47], x9[48], &x10[48], v_cos_bit);

    // stage 11
    output[0]  = x10[0];
    output[1]  = x10[32];
    output[2]  = x10[16];
    output[3]  = x10[48];
    output[4]  = x10[8];
    output[5]  = x10[40];
    output[6]  = x10[24];
    output[7]  = x10[56];
    output[8]  = x10[4];
    output[9]  = x10[36];
    output[10] = x10[20];
    output[11] = x10[52];
    output[12] = x10[12];
    output[13] = x10[44];
    output[14] = x10[28];
    output[15] = x10[60];
    output[16] = x10[2];
    output[17] = x10[34];
    output[18] = x10[18];
    output[19] = x10[50];
    output[20] = x10[10];
    output[21] = x10[42];
    output[22] = x10[26];
    output[23] = x10[58];
    output[24] = x10[6];
    output[25] = x10[38];
    output[26] = x10[22];
    output[27] = x10[54];
    output[28] = x10[14];
    output[29] = x10[46];
    output[30] = x10[30];
    output[31] = x10[62];
}

static inline void highbd_fidentity64_N2_x4_neon(const int32x4_t* input, int32x4_t* output, const int8_t cos_bit) {
    const int32_t   sqrt    = 4 * 5793; // 4 * new_sqrt2
    const int32x4_t newsqrt = vdupq_n_s32(sqrt);

    for (int32_t i = 0; i < 32; i++) {
        int32x4_t temp = vmulq_s32(input[i], newsqrt);
        output[i]      = vrshlq_s32(temp, vdupq_n_s32(-cos_bit));
    }
}

static inline void highbd_fdct64_N2_xn_neon(const int32x4_t* in, int32x4_t* out, int bit, const int howmany) {
    const int stride = 64;
    int       i      = 0;
    do {
        highbd_fdct64_N2_x4_neon(in + i * stride, out + i * stride, bit);
    } while (++i < howmany);
}

static inline void highbd_fidentity64_N2_xn_neon(const int32x4_t* in, int32x4_t* out, int bit, int howmany) {
    const int stride = 64;
    int       i      = 0;
    do {
        highbd_fidentity64_N2_x4_neon(in + i * stride, out + i * stride, bit);
    } while (++i < howmany);
}

static inline void write_buffer_64x64_N2(const int32x4_t* in, int32_t* output) {
    const int32x4_t zero = vdupq_n_s32(0);

    for (int i = 0; i < 32; i++) {
        vst1q_s32(output + i * 64 + 0, in[0 * 64 + i]);
        vst1q_s32(output + i * 64 + 4, in[1 * 64 + i]);
        vst1q_s32(output + i * 64 + 8, in[2 * 64 + i]);
        vst1q_s32(output + i * 64 + 12, in[3 * 64 + i]);
        vst1q_s32(output + i * 64 + 16, in[4 * 64 + i]);
        vst1q_s32(output + i * 64 + 20, in[5 * 64 + i]);
        vst1q_s32(output + i * 64 + 24, in[6 * 64 + i]);
        vst1q_s32(output + i * 64 + 28, in[7 * 64 + i]);
        for (int j = 8; j < 16; j++) {
            vst1q_s32(output + i * 64 + j * 4, zero);
        }
    }

    memset(output + 32 * 64, 0, 32 * 64 * sizeof(int32_t));
}

static inline void transpose_wx32_in_64x64(const int32x4_t* in, int32x4_t* out, int width) {
    for (int i = 0; i < width; i++) {
        for (int j = 0; j < 8; j++) {
            transpose_elems_s32_4x4(in[j * 4 + 0 + i * 64],
                                    in[j * 4 + 1 + i * 64],
                                    in[j * 4 + 2 + i * 64],
                                    in[j * 4 + 3 + i * 64],
                                    &out[j * 64 + 0 + i * 4],
                                    &out[j * 64 + 1 + i * 4],
                                    &out[j * 64 + 2 + i * 4],
                                    &out[j * 64 + 3 + i * 4]);
        }
    }
}

static inline void shift_right_2_round_s32_wx32_N2(const int32x4_t* in, int32x4_t* out, int width) {
    for (int i = 0; i < width; i++) {
        shift_right_2_round_s32_x4(in + i * 64, out + i * 64, 32);
    }
}

void svt_av1_fwd_txfm2d_64x64_N2_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;

    switch (tx_type) {
    case DCT_DCT: {
        // Column-wise transform.
        int32x4_t buf0[1024];
        load_buffer_64x64(input, buf0, stride, 0);
        highbd_fdct64_N2_xn_neon(buf0, buf0, 13, 16);
        shift_right_2_round_s32_wx32_N2(buf0, buf0, 16);

        int32x4_t buf1[1024];
        transpose_wx32_in_64x64(buf0, buf1, 16);

        // Row-wise transform.
        highbd_fdct64_N2_xn_neon(buf1, buf0, 10, 16);
        shift_right_2_round_s32_wx32_N2(buf0, buf0, 8);
        transpose_wx32_in_64x64(buf0, buf1, 8);
        write_buffer_64x64_N2(buf1, output);
        break;
    }
    case IDTX: {
        int32x4_t buf0[1024];
        load_buffer_64x64(input, buf0, stride, 0);

        // Column-wise transform.
        int32x4_t buf1[1024];
        highbd_fidentity64_N2_xn_neon(buf0, buf1, 12, 16);
        shift_right_2_round_s32_wx32_N2(buf1, buf1, 8);

        // Row-wise transform.
        highbd_fidentity64_N2_xn_neon(buf1, buf0, 12, 16);
        shift_right_2_round_s32_wx32_N2(buf0, buf0, 8);
        write_buffer_64x64_N2(buf0, output);
        break;
    }
    default:
        assert(0);
    }
}

static inline void write_buffer_4xh_N2(int32x4_t* buf, int32_t* output, int height) {
    const int h = height >> 1;
    for (int i = 0; i < h; i++) {
        vst1_s32(output + i * 4, vget_low_s32(buf[i]));
        memset(output + i * 4 + 2, 0, 2 * sizeof(int32_t));
    }
    memset(output + 4 * h, 0, 4 * h * sizeof(int32_t));
}

TRANSFORM_COL_ONE(fdct8_N2, 8)
TRANSFORM_COL_ONE(fadst8_N2, 8)
TRANSFORM_COL_ONE(fidentity8_N2, 8)

TRANSFORM_ROW_RECT_ONE(fdct4_N2, 4)
TRANSFORM_ROW_RECT_ONE(fadst4_N2, 4)
TRANSFORM_ROW_RECT_ONE(fidentity4_N2, 4)

static const fwd_transform_1d_col_neon col_highbd_txfm8_x4_N2_arr[TX_TYPES] = {
    highbd_fdct8_N2_col_neon, // DCT_DCT
    highbd_fadst8_N2_col_neon, // ADST_DCT
    highbd_fdct8_N2_col_neon, // DCT_ADST
    highbd_fadst8_N2_col_neon, // ADST_ADST
    highbd_fadst8_N2_col_neon, // FLIPADST_DCT
    highbd_fdct8_N2_col_neon, // DCT_FLIPADST
    highbd_fadst8_N2_col_neon, // FLIPADST_FLIPADST
    highbd_fadst8_N2_col_neon, // ADST_FLIPADST
    highbd_fadst8_N2_col_neon, // FLIPADST_ADST
    highbd_fidentity8_N2_col_neon, // IDTX
    highbd_fdct8_N2_col_neon, // V_DCT
    highbd_fidentity8_N2_col_neon, // H_DCT
    highbd_fadst8_N2_col_neon, // V_ADST
    highbd_fidentity8_N2_col_neon, // H_ADST
    highbd_fadst8_N2_col_neon, // V_FLIPADST
    highbd_fidentity8_N2_col_neon // H_FLIPADST
};

static const fwd_transform_1d_row_neon row_rect_highbd_txfm4_x4_N2_arr[TX_TYPES] = {
    highbd_fdct4_N2_row_rect_neon, // DCT_DCT
    highbd_fdct4_N2_row_rect_neon, // ADST_DCT
    highbd_fadst4_N2_row_rect_neon, // DCT_ADST
    highbd_fadst4_N2_row_rect_neon, // ADST_ADST
    highbd_fdct4_N2_row_rect_neon, // FLIPADST_DCT
    highbd_fadst4_N2_row_rect_neon, // DCT_FLIPADST
    highbd_fadst4_N2_row_rect_neon, // FLIPADST_FLIPADST
    highbd_fadst4_N2_row_rect_neon, // ADST_FLIPADST
    highbd_fadst4_N2_row_rect_neon, // FLIPADST_ADST
    highbd_fidentity4_N2_row_rect_neon, // IDTX
    highbd_fidentity4_N2_row_rect_neon, // V_DCT
    highbd_fdct4_N2_row_rect_neon, // H_DCT
    highbd_fidentity4_N2_row_rect_neon, // V_ADST
    highbd_fadst4_N2_row_rect_neon, // H_ADST
    highbd_fidentity4_N2_row_rect_neon, // V_FLIPADST
    highbd_fadst4_N2_row_rect_neon // H_FLIPADST
};

void svt_av1_fwd_txfm2d_4x8_N2_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    int                             bitcol   = fwd_cos_bit_col[0][1];
    int                             bitrow   = fwd_cos_bit_row[0][1];
    const fwd_transform_1d_col_neon col_txfm = col_highbd_txfm8_x4_N2_arr[tx_type];
    const fwd_transform_1d_row_neon row_txfm = row_rect_highbd_txfm4_x4_N2_arr[tx_type];

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 8);

    // Column-wise transform.
    int32x4_t buf0[8];
    col_txfm(input, buf0, stride, bitcol, lr_flip);
    shift_right_1_round_s32_x4(buf0, buf0, 4);

    int32x4_t buf1[8];
    transpose_arrays_s32_4x4(buf0, buf1);

    // Row-wise transform.
    row_txfm(buf1, buf0, bitrow);
    transpose_arrays_s32_4x4(buf0, buf1);
    write_buffer_4xh_N2(buf1, output, 8);
}

TRANSFORM_COL_ONE(fdct16_N2, 16)
TRANSFORM_COL_ONE(fadst16_N2, 16)
TRANSFORM_COL_ONE(fidentity16_N2, 16)

static const fwd_transform_1d_col_neon col_highbd_txfm16_x4_N2_arr[TX_TYPES] = {
    highbd_fdct16_N2_col_neon, // DCT_DCT
    highbd_fadst16_N2_col_neon, // ADST_DCT
    highbd_fdct16_N2_col_neon, // DCT_ADST
    highbd_fadst16_N2_col_neon, // ADST_ADST
    highbd_fadst16_N2_col_neon, // FLIPADST_DCT
    highbd_fdct16_N2_col_neon, // DCT_FLIPADST
    highbd_fadst16_N2_col_neon, // FLIPADST_FLIPADST
    highbd_fadst16_N2_col_neon, // ADST_FLIPADST
    highbd_fadst16_N2_col_neon, // FLIPADST_ADST
    highbd_fidentity16_N2_col_neon, // IDTX
    highbd_fdct16_N2_col_neon, // V_DCT
    highbd_fidentity16_N2_col_neon, // H_DCT
    highbd_fadst16_N2_col_neon, // V_ADST
    highbd_fidentity16_N2_col_neon, // H_ADST
    highbd_fadst16_N2_col_neon, // V_FLIPADST
    highbd_fidentity16_N2_col_neon // H_FLIPADST
};

TRANSFORM_ROW_MANY(fdct4_N2, 4)
TRANSFORM_ROW_MANY(fadst4_N2, 4)
TRANSFORM_ROW_MANY(fidentity4_N2, 4)

static const fwd_transform_1d_row_many_neon row_highbd_txfm4_xn_N2_arr[TX_TYPES] = {
    highbd_fdct4_N2_row_many_neon, // DCT_DCT
    highbd_fdct4_N2_row_many_neon, // ADST_DCT
    highbd_fadst4_N2_row_many_neon, // DCT_ADST
    highbd_fadst4_N2_row_many_neon, // ADST_ADST
    highbd_fdct4_N2_row_many_neon, // FLIPADST_DCT
    highbd_fadst4_N2_row_many_neon, // DCT_FLIPADST
    highbd_fadst4_N2_row_many_neon, // FLIPADST_FLIPADST
    highbd_fadst4_N2_row_many_neon, // ADST_FLIPADST
    highbd_fadst4_N2_row_many_neon, // FLIPADST_ADST
    highbd_fidentity4_N2_row_many_neon, // IDTX
    highbd_fidentity4_N2_row_many_neon, // V_DCT
    highbd_fdct4_N2_row_many_neon, // H_DCT
    highbd_fidentity4_N2_row_many_neon, // V_ADST
    highbd_fadst4_N2_row_many_neon, // H_ADST
    highbd_fidentity4_N2_row_many_neon, // V_FLIPADST
    highbd_fadst4_N2_row_many_neon // H_FLIPADST
};

void svt_av1_fwd_txfm2d_4x16_N2_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    int                                  bitcol   = fwd_cos_bit_col[0][2];
    int                                  bitrow   = fwd_cos_bit_row[0][2];
    const fwd_transform_1d_col_neon      col_txfm = col_highbd_txfm16_x4_N2_arr[tx_type];
    const fwd_transform_1d_row_many_neon row_txfm = row_highbd_txfm4_xn_N2_arr[tx_type];

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 16);

    // Column-wise transform.
    int32x4_t buf0[16];
    if (lr_flip) {
        col_txfm(input,
                 buf0,
                 stride,
                 bitcol,
                 /*lr_flip=*/1);
    } else {
        col_txfm(input,
                 buf0,
                 stride,
                 bitcol,
                 /*lr_flip=*/0);
    }
    shift_right_1_round_s32_x4(buf0, buf0, 8);

    int32x4_t buf1[16];
    transpose_arrays_s32_4x8(buf0, buf1);

    // Row-wise transform.
    row_txfm(buf1, buf0, bitrow, /*howmany=*/2, /*hm_stride=*/4);
    transpose_arrays_s32_4x8(buf0, buf1);
    write_buffer_4xh_N2(buf1, output, 16);
}

static inline void write_buffer_8xh_N2(int32x4_t* buf, int32_t* output, int height) {
    const int h = height >> 1;
    for (int i = 0; i < h; i++) {
        vst1q_s32(output + i * 8, buf[i]);
        memset(output + i * 8 + 4, 0, 4 * sizeof(int32_t));
    }
    memset(output + 8 * h, 0, 8 * h * sizeof(int32_t));
}

TRANSFORM_COL_MANY(fdct4_N2, 4)
TRANSFORM_COL_MANY(fadst4_N2, 4)
TRANSFORM_COL_MANY(fidentity4_N2, 4)

static const fwd_transform_1d_col_many_neon col_highbd_txfm4_xn_N2_arr[TX_TYPES] = {
    highbd_fdct4_N2_col_many_neon, // DCT_DCT
    highbd_fadst4_N2_col_many_neon, // ADST_DCT
    highbd_fdct4_N2_col_many_neon, // DCT_ADST
    highbd_fadst4_N2_col_many_neon, // ADST_ADST
    highbd_fadst4_N2_col_many_neon, // FLIPADST_DCT
    highbd_fdct4_N2_col_many_neon, // DCT_FLIPADST
    highbd_fadst4_N2_col_many_neon, // FLIPADST_FLIPADST
    highbd_fadst4_N2_col_many_neon, // ADST_FLIPADST
    highbd_fadst4_N2_col_many_neon, // FLIPADST_ADST
    highbd_fidentity4_N2_col_many_neon, // IDTX
    highbd_fdct4_N2_col_many_neon, // V_DCT
    highbd_fidentity4_N2_col_many_neon, // H_DCT
    highbd_fadst4_N2_col_many_neon, // V_ADST
    highbd_fidentity4_N2_col_many_neon, // H_ADST
    highbd_fadst4_N2_col_many_neon, // V_FLIPADST
    highbd_fidentity4_N2_col_many_neon // H_FLIPADST
};

TRANSFORM_ROW_RECT_ONE(fdct8_N2, 8)
TRANSFORM_ROW_RECT_ONE(fadst8_N2, 8)
TRANSFORM_ROW_RECT_ONE(fidentity8_N2, 8)

static const fwd_transform_1d_row_neon row_rect_highbd_txfm8_x4_N2_arr[TX_TYPES] = {
    highbd_fdct8_N2_row_rect_neon, // DCT_DCT
    highbd_fdct8_N2_row_rect_neon, // ADST_DCT
    highbd_fadst8_N2_row_rect_neon, // DCT_ADST
    highbd_fadst8_N2_row_rect_neon, // ADST_ADST
    highbd_fdct8_N2_row_rect_neon, // FLIPADST_DCT
    highbd_fadst8_N2_row_rect_neon, // DCT_FLIPADST
    highbd_fadst8_N2_row_rect_neon, // FLIPADST_FLIPADST
    highbd_fadst8_N2_row_rect_neon, // ADST_FLIPADST
    highbd_fadst8_N2_row_rect_neon, // FLIPADST_ADST
    highbd_fidentity8_N2_row_rect_neon, // IDTX
    highbd_fidentity8_N2_row_rect_neon, // V_DCT
    highbd_fdct8_N2_row_rect_neon, // H_DCT
    highbd_fidentity8_N2_row_rect_neon, // V_ADST
    highbd_fadst8_N2_row_rect_neon, // H_ADST
    highbd_fidentity8_N2_row_rect_neon, // V_FLIPADST
    highbd_fadst8_N2_row_rect_neon // H_FLIPADST
};

void svt_av1_fwd_txfm2d_8x4_N2_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    const int                            bitcol   = fwd_cos_bit_col[1][0];
    const int                            bitrow   = fwd_cos_bit_row[1][0];
    const fwd_transform_1d_col_many_neon col_txfm = col_highbd_txfm4_xn_N2_arr[tx_type];
    const fwd_transform_1d_row_neon      row_txfm = row_rect_highbd_txfm8_x4_N2_arr[tx_type];

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 4);

    // Column-wise transform.
    int32x4_t buf0[8];
    if (lr_flip) {
        col_txfm(input,
                 buf0 + 4,
                 stride,
                 bitcol,
                 /*lr_flip=*/1,
                 /*howmany=*/2,
                 /*hm_stride=*/-4);
    } else {
        col_txfm(input,
                 buf0,
                 stride,
                 bitcol,
                 /*lr_flip=*/0,
                 /*howmany=*/2,
                 /*hm_stride=*/4);
    }

    shift_right_1_round_s32_x4(buf0, buf0, 2);
    shift_right_1_round_s32_x4(buf0 + 4, buf0 + 4, 2);

    int32x4_t buf1[8];
    transpose_arrays_s32_8x4(buf0, buf1);

    // Row-wise transform.
    row_txfm(buf1, buf0, bitrow);
    transpose_arrays_s32_4x4(buf0, buf1);
    write_buffer_8xh_N2(buf1, output, 4);
}

TRANSFORM_COL_MANY(fdct16_N2, 16)
TRANSFORM_COL_MANY(fadst16_N2, 16)
TRANSFORM_COL_MANY(fidentity16_N2, 16)

static const fwd_transform_1d_col_many_neon col_highbd_txfm16_xn_N2_arr[TX_TYPES] = {
    highbd_fdct16_N2_col_many_neon, // DCT_DCT
    highbd_fadst16_N2_col_many_neon, // ADST_DCT
    highbd_fdct16_N2_col_many_neon, // DCT_ADST
    highbd_fadst16_N2_col_many_neon, // ADST_ADST
    highbd_fadst16_N2_col_many_neon, // FLIPADST_DCT
    highbd_fdct16_N2_col_many_neon, // DCT_FLIPADST
    highbd_fadst16_N2_col_many_neon, // FLIPADST_FLIPADST
    highbd_fadst16_N2_col_many_neon, // ADST_FLIPADST
    highbd_fadst16_N2_col_many_neon, // FLIPADST_ADST
    highbd_fidentity16_N2_col_many_neon, // IDTX
    highbd_fdct16_N2_col_many_neon, // V_DCT
    highbd_fidentity16_N2_col_many_neon, // H_DCT
    highbd_fadst16_N2_col_many_neon, // V_ADST
    highbd_fidentity16_N2_col_many_neon, // H_ADST
    highbd_fadst16_N2_col_many_neon, // V_FLIPADST
    highbd_fidentity16_N2_col_many_neon // H_FLIPADST
};

TRANSFORM_ROW_RECT_MANY(fdct8_N2, 8)
TRANSFORM_ROW_RECT_MANY(fadst8_N2, 8)
TRANSFORM_ROW_RECT_MANY(fidentity8_N2, 8)

static const fwd_transform_1d_row_many_neon row_rect_highbd_txfm8_xn_N2_arr[TX_TYPES] = {
    highbd_fdct8_N2_row_rect_many_neon, // DCT_DCT
    highbd_fdct8_N2_row_rect_many_neon, // ADST_DCT
    highbd_fadst8_N2_row_rect_many_neon, // DCT_ADST
    highbd_fadst8_N2_row_rect_many_neon, // ADST_ADST
    highbd_fdct8_N2_row_rect_many_neon, // FLIPADST_DCT
    highbd_fadst8_N2_row_rect_many_neon, // DCT_FLIPADST
    highbd_fadst8_N2_row_rect_many_neon, // FLIPADST_FLIPADST
    highbd_fadst8_N2_row_rect_many_neon, // ADST_FLIPADST
    highbd_fadst8_N2_row_rect_many_neon, // FLIPADST_ADST
    highbd_fidentity8_N2_row_rect_many_neon, // IDTX
    highbd_fidentity8_N2_row_rect_many_neon, // V_DCT
    highbd_fdct8_N2_row_rect_many_neon, // H_DCT
    highbd_fidentity8_N2_row_rect_many_neon, // V_ADST
    highbd_fadst8_N2_row_rect_many_neon, // H_ADST
    highbd_fidentity8_N2_row_rect_many_neon, // V_FLIPADST
    highbd_fadst8_N2_row_rect_many_neon // H_FLIPADST
};

static AOM_FORCE_INLINE void transpose_arrays_s32_4nx4n_in_4mx4m(const int32x4_t* in, int32x4_t* out, const int width,
                                                                 const int height, const int tr_width,
                                                                 const int tr_height) {
    const int tr_h = tr_height >> 2;
    const int tr_w = tr_width >> 2;
    for (int j = 0; j < tr_w; j++) {
        for (int i = 0; i < tr_h; i++) {
            transpose_arrays_s32_4x4(in + j * height + i * 4, out + i * width + j * 4);
        }
    }
}

void svt_av1_fwd_txfm2d_8x16_N2_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    const int                            bitcol   = fwd_cos_bit_col[1][2];
    const int                            bitrow   = fwd_cos_bit_row[1][2];
    const fwd_transform_1d_col_many_neon col_txfm = col_highbd_txfm16_xn_N2_arr[tx_type];
    const fwd_transform_1d_row_many_neon row_txfm = row_rect_highbd_txfm8_xn_N2_arr[tx_type];

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 16);

    // Column-wise transform.
    int32x4_t buf0[32];
    if (lr_flip) {
        col_txfm(input,
                 buf0 + 16,
                 stride,
                 bitcol,
                 /*lr_flip=*/1,
                 /*howmany=*/2,
                 /*hm_stride=*/-16);
    } else {
        col_txfm(input,
                 buf0,
                 stride,
                 bitcol,
                 /*lr_flip=*/0,
                 /*howmany=*/2,
                 /*hm_stride=*/16);
    }

    shift_right_2_round_s32_x4(buf0, buf0, 8);
    shift_right_2_round_s32_x4(buf0 + 16, buf0 + 16, 8);

    int32x4_t buf1[32];
    transpose_arrays_s32_8x16(buf0, buf1);

    // Row-wise transform.
    row_txfm(buf1, buf0, bitrow, /*howmany=*/2, /*hm_stride=*/8);
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 16, 8, 8, 4);
    write_buffer_8xh_N2(buf1, output, 16);
}

TRANSFORM_COL_MANY(fdct32_N2, 32)
TRANSFORM_COL_MANY(fidentity32_N2, 32)

static const fwd_transform_1d_col_many_neon col_highbd_txfm32_xn_N2_arr[TX_TYPES] = {
    highbd_fdct32_N2_col_many_neon, // DCT_DCT
    NULL, // ADST_DCT
    NULL, // DCT_ADST
    NULL, // ADST_ADST
    NULL, // FLIPADST_DCT
    NULL, // DCT_FLIPADST
    NULL, // FLIPADST_FLIPADST
    NULL, // ADST_FLIPADST
    NULL, // FLIPADST_ADST
    highbd_fidentity32_N2_col_many_neon, // IDTX
    NULL, // V_DCT
    NULL, // H_DCT
    NULL, // V_ADST
    NULL, // H_ADST
    NULL, // V_FLIPADST
    NULL // H_FLIPADST
};

TRANSFORM_ROW_MANY(fdct8_N2, 8)
TRANSFORM_ROW_MANY(fadst8_N2, 8)
TRANSFORM_ROW_MANY(fidentity8_N2, 8)

static const fwd_transform_1d_row_many_neon row_highbd_txfm8_xn_N2_arr[TX_TYPES] = {
    highbd_fdct8_N2_row_many_neon, // DCT_DCT
    highbd_fdct8_N2_row_many_neon, // ADST_DCT
    highbd_fadst8_N2_row_many_neon, // DCT_ADST
    highbd_fadst8_N2_row_many_neon, // ADST_ADST
    highbd_fdct8_N2_row_many_neon, // FLIPADST_DCT
    highbd_fadst8_N2_row_many_neon, // DCT_FLIPADST
    highbd_fadst8_N2_row_many_neon, // FLIPADST_FLIPADST
    highbd_fadst8_N2_row_many_neon, // ADST_FLIPADST
    highbd_fadst8_N2_row_many_neon, // FLIPADST_ADST
    highbd_fidentity8_N2_row_many_neon, // IDTX
    highbd_fidentity8_N2_row_many_neon, // V_DCT
    highbd_fdct8_N2_row_many_neon, // H_DCT
    highbd_fidentity8_N2_row_many_neon, // V_ADST
    highbd_fadst8_N2_row_many_neon, // H_ADST
    highbd_fidentity8_N2_row_many_neon, // V_FLIPADST
    highbd_fadst8_N2_row_many_neon // H_FLIPADST
};

void svt_av1_fwd_txfm2d_8x32_N2_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    const int                            bitcol   = fwd_cos_bit_col[1][3];
    const int                            bitrow   = fwd_cos_bit_row[1][3];
    const fwd_transform_1d_col_many_neon col_txfm = col_highbd_txfm32_xn_N2_arr[tx_type];
    const fwd_transform_1d_row_many_neon row_txfm = row_highbd_txfm8_xn_N2_arr[tx_type];

    // Column-wise transform.
    int32x4_t buf0[64];
    col_txfm(input,
             buf0,
             stride,
             bitcol,
             /*lr_flip=*/0,
             /*howmany=*/2,
             /*hm_stride=*/32);

    shift_right_2_round_s32_x4(buf0, buf0, 16);
    shift_right_2_round_s32_x4(buf0 + 32, buf0 + 32, 16);

    int32x4_t buf1[64];
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 8, 32, 8, 16);

    // Row-wise transform.
    row_txfm(buf1, buf0, bitrow, /*howmany=*/4, /*hm_stride=*/8);
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 32, 8, 16, 4);
    write_buffer_8xh_N2(buf1, output, 32);
}

TRANSFORM_ROW_ONE(fdct16_N2, 16)
TRANSFORM_ROW_ONE(fadst16_N2, 16)
TRANSFORM_ROW_ONE(fidentity16_N2, 16)

static const fwd_transform_1d_row_neon row_highbd_txfm16_x4_N2_arr[TX_TYPES] = {
    highbd_fdct16_N2_row_neon, // DCT_DCT
    highbd_fdct16_N2_row_neon, // ADST_DCT
    highbd_fadst16_N2_row_neon, // DCT_ADST
    highbd_fadst16_N2_row_neon, // ADST_ADST
    highbd_fdct16_N2_row_neon, // FLIPADST_DCT
    highbd_fadst16_N2_row_neon, // DCT_FLIPADST
    highbd_fadst16_N2_row_neon, // FLIPADST_FLIPADST
    highbd_fadst16_N2_row_neon, // ADST_FLIPADST
    highbd_fadst16_N2_row_neon, // FLIPADST_ADST
    highbd_fidentity16_N2_row_neon, // IDTX
    highbd_fidentity16_N2_row_neon, // V_DCT
    highbd_fdct16_N2_row_neon, // H_DCT
    highbd_fidentity16_N2_row_neon, // V_ADST
    highbd_fadst16_N2_row_neon, // H_ADST
    highbd_fidentity16_N2_row_neon, // V_FLIPADST
    highbd_fadst16_N2_row_neon // H_FLIPADST

};

static inline void write_buffer_16xh_N2(int32x4_t* buf, int32_t* output, int height) {
    const int h = height >> 1;
    for (int i = 0; i < h; i++) {
        vst1q_s32(output + i * 16, buf[i]);
        vst1q_s32(output + i * 16 + 4, buf[i + height]);
        memset(output + i * 16 + 8, 0, 8 * sizeof(int32_t));
    }
    memset(output + 16 * h, 0, 16 * h * sizeof(int32_t));
}

void svt_av1_fwd_txfm2d_16x4_N2_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    int                                  bitcol   = fwd_cos_bit_col[2][0];
    int                                  bitrow   = fwd_cos_bit_row[2][0];
    const fwd_transform_1d_col_many_neon col_txfm = col_highbd_txfm4_xn_N2_arr[tx_type];
    const fwd_transform_1d_row_neon      row_txfm = row_highbd_txfm16_x4_N2_arr[tx_type];

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 4);

    // Column-wise transform.
    int32x4_t buf0[16];
    if (lr_flip) {
        col_txfm(input,
                 buf0 + 3 * 4,
                 stride,
                 bitcol,
                 /*lr_flip=*/1,
                 /*howmany=*/4,
                 /*hm_stride=*/-4);
    } else {
        col_txfm(input,
                 buf0,
                 stride,
                 bitcol,
                 /*lr_flip=*/0,
                 /*howmany=*/4,
                 /*hm_stride=*/4);
    }

    for (int i = 0; i < 4; i++) {
        shift_right_1_round_s32_x4(buf0 + i * 4, buf0 + i * 4, 2);
    }

    int32x4_t buf1[16];
    transpose_arrays_s32_16x4(buf0, buf1);

    // Row-wise transform.
    row_txfm(buf1, buf0, bitrow);
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 4, 16, 4, 8);
    write_buffer_16xh_N2(buf1, output, 4);
}

TRANSFORM_COL_MANY(fdct8_N2, 8)
TRANSFORM_COL_MANY(fadst8_N2, 8)
TRANSFORM_COL_MANY(fidentity8_N2, 8)

static const fwd_transform_1d_col_many_neon col_highbd_txfm8_xn_N2_arr[TX_TYPES] = {
    highbd_fdct8_N2_col_many_neon, // DCT_DCT
    highbd_fadst8_N2_col_many_neon, // ADST_DCT
    highbd_fdct8_N2_col_many_neon, // DCT_ADST
    highbd_fadst8_N2_col_many_neon, // ADST_ADST
    highbd_fadst8_N2_col_many_neon, // FLIPADST_DCT
    highbd_fdct8_N2_col_many_neon, // DCT_FLIPADST
    highbd_fadst8_N2_col_many_neon, // FLIPADST_FLIPADST
    highbd_fadst8_N2_col_many_neon, // ADST_FLIPADST
    highbd_fadst8_N2_col_many_neon, // FLIPADST_ADST
    highbd_fidentity8_N2_col_many_neon, // IDTX
    highbd_fdct8_N2_col_many_neon, // V_DCT
    highbd_fidentity8_N2_col_many_neon, // H_DCT
    highbd_fadst8_N2_col_many_neon, // V_ADST
    highbd_fidentity8_N2_col_many_neon, // H_ADST
    highbd_fadst8_N2_col_many_neon, // V_FLIPADST
    highbd_fidentity8_N2_col_many_neon // H_FLIPADST
};

TRANSFORM_ROW_RECT_ONE(fdct16_N2, 16)
TRANSFORM_ROW_RECT_ONE(fadst16_N2, 16)
TRANSFORM_ROW_RECT_ONE(fidentity16_N2, 16)

static const fwd_transform_1d_row_neon row_rect_highbd_txfm16_x4_N2_arr[TX_TYPES] = {
    highbd_fdct16_N2_row_rect_neon, // DCT_DCT
    highbd_fdct16_N2_row_rect_neon, // ADST_DCT
    highbd_fadst16_N2_row_rect_neon, // DCT_ADST
    highbd_fadst16_N2_row_rect_neon, // ADST_ADST
    highbd_fdct16_N2_row_rect_neon, // FLIPADST_DCT
    highbd_fadst16_N2_row_rect_neon, // DCT_FLIPADST
    highbd_fadst16_N2_row_rect_neon, // FLIPADST_FLIPADST
    highbd_fadst16_N2_row_rect_neon, // ADST_FLIPADST
    highbd_fadst16_N2_row_rect_neon, // FLIPADST_ADST
    highbd_fidentity16_N2_row_rect_neon, // IDTX
    highbd_fidentity16_N2_row_rect_neon, // V_DCT
    highbd_fdct16_N2_row_rect_neon, // H_DCT
    highbd_fidentity16_N2_row_rect_neon, // V_ADST
    highbd_fadst16_N2_row_rect_neon, // H_ADST
    highbd_fidentity16_N2_row_rect_neon, // V_FLIPADST
    highbd_fadst16_N2_row_rect_neon // H_FLIPADST
};

void svt_av1_fwd_txfm2d_16x8_N2_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    const fwd_transform_1d_col_many_neon col_txfm = col_highbd_txfm8_xn_N2_arr[tx_type];
    const fwd_transform_1d_row_neon      row_txfm = row_rect_highbd_txfm16_x4_N2_arr[tx_type];
    int                                  bit      = fwd_cos_bit_col[2][1];

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 8);

    // Column-wise transform.
    int32x4_t buf0[32];
    if (lr_flip) {
        col_txfm(input,
                 buf0 + 3 * 8,
                 stride,
                 bit,
                 /*lr_flip=*/1,
                 /*howmany=*/4,
                 /*hm_stride=*/-8);
    } else {
        col_txfm(input,
                 buf0,
                 stride,
                 bit,
                 /*lr_flip=*/0,
                 /*howmany=*/4,
                 /*hm_stride=*/8);
    }
    for (int i = 0; i < 4; i++) {
        shift_right_2_round_s32_x4(buf0 + i * 8, buf0 + i * 8, 4);
    }

    int32x4_t buf1[32];
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 16, 8, 16, 4);

    // Row-wise transform.
    row_txfm(buf1, buf0, bit);
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 8, 16, 4, 8);
    write_buffer_16xh_N2(buf1, output, 8);
}

TRANSFORM_ROW_RECT_MANY(fdct16_N2, 16)
TRANSFORM_ROW_RECT_MANY(fadst16_N2, 16)
TRANSFORM_ROW_RECT_MANY(fidentity16_N2, 16)

static const fwd_transform_1d_row_many_neon row_rect_highbd_txfm16_xn_N2_arr[TX_TYPES] = {
    highbd_fdct16_N2_row_rect_many_neon, // DCT_DCT
    highbd_fdct16_N2_row_rect_many_neon, // ADST_DCT
    highbd_fadst16_N2_row_rect_many_neon, // DCT_ADST
    highbd_fadst16_N2_row_rect_many_neon, // ADST_ADST
    highbd_fdct16_N2_row_rect_many_neon, // FLIPADST_DCT
    highbd_fadst16_N2_row_rect_many_neon, // DCT_FLIPADST
    highbd_fadst16_N2_row_rect_many_neon, // FLIPADST_FLIPADST
    highbd_fadst16_N2_row_rect_many_neon, // ADST_FLIPADST
    highbd_fadst16_N2_row_rect_many_neon, // FLIPADST_ADST
    highbd_fidentity16_N2_row_rect_many_neon, // IDTX
    highbd_fidentity16_N2_row_rect_many_neon, // V_DCT
    highbd_fdct16_N2_row_rect_many_neon, // H_DCT
    highbd_fidentity16_N2_row_rect_many_neon, // V_ADST
    highbd_fadst16_N2_row_rect_many_neon, // H_ADST
    highbd_fidentity16_N2_row_rect_many_neon, // V_FLIPADST
    highbd_fadst16_N2_row_rect_many_neon // H_FLIPADST
};

void svt_av1_fwd_txfm2d_16x32_N2_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    const fwd_transform_1d_col_many_neon col_txfm = col_highbd_txfm32_xn_N2_arr[tx_type];
    const fwd_transform_1d_row_many_neon row_txfm = row_rect_highbd_txfm16_xn_N2_arr[tx_type];
    int                                  bitcol   = fwd_cos_bit_col[2][3];
    int                                  bitrow   = fwd_cos_bit_row[2][3];

    // Column-wise transform.
    int32x4_t buf0[128];
    col_txfm(input,
             buf0,
             stride,
             bitcol,
             /*lr_flip=*/0,
             /*howmany=*/4,
             /*hm_stride=*/32);
    for (int i = 0; i < 4; i++) {
        shift_right_4_round_s32_x4(buf0 + i * 32, buf0 + i * 32, 16);
    }

    int32x4_t buf1[128];
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 16, 32, 16, 16);

    // Row-wise transform.
    row_txfm(buf1, buf0, bitrow, /*howmany=*/4, /*hm_stride=*/16);
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 32, 16, 16, 8);
    write_buffer_16xh_N2(buf1, output, 32);
}

void svt_av1_fwd_txfm2d_16x64_N2_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    const int bitcol = fwd_cos_bit_col[2][4];
    const int bitrow = fwd_cos_bit_row[2][4];

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 64);

    // Column-wise transform.
    int32x4_t buf0[256];
    load_buffer_16x64(input, buf0, stride, lr_flip);
    highbd_fdct64_N2_xn_neon(buf0, buf0, bitcol, 4);
    for (int i = 0; i < 4; i++) {
        shift_right_2_round_s32_x4(buf0 + i * 64, buf0 + i * 64, 32);
    }

    int32x4_t buf1[256];
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 16, 64, 16, 32);

    // Row-wise transform.
    highbd_fdct16_N2_xn_neon(buf1, buf0, bitrow, 8);
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 64, 16, 32, 8);
    write_buffer_16xh_N2(buf1, output, 64);
}

static inline void write_buffer_32xh_N2(int32x4_t* buf, int32_t* output, int height) {
    const int h = height >> 1;
    for (int i = 0; i < h; i++) {
        store_s32_4x4(output + i * 32, 4, buf[i], buf[i + height], buf[i + 2 * height], buf[i + 3 * height]);
        memset(output + i * 32 + 16, 0, 16 * sizeof(int32_t));
    }
    memset(output + 32 * h, 0, 32 * h * sizeof(int32_t));
}

TRANSFORM_ROW_ONE(fdct32_N2, 32)
TRANSFORM_ROW_ONE(fidentity32_N2, 32)

static const fwd_transform_1d_row_neon row_highbd_txfm32_x4_N2_arr[TX_TYPES] = {
    highbd_fdct32_N2_row_neon, // DCT_DCT
    NULL, // ADST_DCT
    NULL, // DCT_ADST
    NULL, // ADST_ADST
    NULL, // FLIPADST_DCT
    NULL, // DCT_FLIPADST
    NULL, // FLIPADST_FLIPADST
    NULL, // ADST_FLIPADST
    NULL, // FLIPADST_ADST
    highbd_fidentity32_N2_row_neon, // IDTX
    NULL, // V_DCT
    NULL, // H_DCT
    NULL, // V_ADST
    NULL, // H_ADST
    NULL, // V_FLIPADST
    NULL // H_FLIPADST

};

void svt_av1_fwd_txfm2d_32x8_N2_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    const fwd_transform_1d_col_many_neon col_txfm = col_highbd_txfm8_xn_N2_arr[tx_type];
    const fwd_transform_1d_row_neon      row_txfm = row_highbd_txfm32_x4_N2_arr[tx_type];
    int                                  bitcol   = fwd_cos_bit_col[3][1];
    int                                  bitrow   = fwd_cos_bit_row[3][1];

    // Column-wise transform.
    int32x4_t buf0[64];
    col_txfm(input,
             buf0,
             stride,
             bitcol,
             /*lr_flip=*/0,
             /*howmany=*/8,
             /*hm_stride=*/8);
    for (int i = 0; i < 8; i++) {
        shift_right_2_round_s32_x4(buf0 + i * 8, buf0 + i * 8, 4);
    }

    int32x4_t buf1[64];
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 32, 8, 32, 4);

    // Row-wise transform.
    row_txfm(buf1, buf0, bitrow);
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 8, 32, 4, 16);
    write_buffer_32xh_N2(buf1, output, 8);
}

TRANSFORM_ROW_RECT_MANY(fdct32_N2, 32)
TRANSFORM_ROW_RECT_MANY(fidentity32_N2, 32)

static const fwd_transform_1d_row_many_neon row_rect_highbd_txfm32_xn_N2_arr[TX_TYPES] = {
    highbd_fdct32_N2_row_rect_many_neon, // DCT_DCT
    NULL, // ADST_DCT
    NULL, // DCT_ADST
    NULL, // ADST_ADST
    NULL, // FLIPADST_DCT
    NULL, // DCT_FLIPADST
    NULL, // FLIPADST_FLIPADST
    NULL, // ADST_FLIPADST
    NULL, // FLIPADST_ADST
    highbd_fidentity32_N2_row_rect_many_neon, // IDTX
    NULL, // V_DCT
    NULL, // H_DCT
    NULL, // V_ADST
    NULL, // H_ADST
    NULL, // V_FLIPADST
    NULL // H_FLIPADST

};

void svt_av1_fwd_txfm2d_32x16_N2_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    const fwd_transform_1d_col_many_neon col_txfm = col_highbd_txfm16_xn_N2_arr[tx_type];
    const fwd_transform_1d_row_many_neon row_txfm = row_rect_highbd_txfm32_xn_N2_arr[tx_type];
    int                                  bitcol   = fwd_cos_bit_col[3][2];
    int                                  bitrow   = fwd_cos_bit_row[3][2];

    // Column-wise transform.
    int32x4_t buf0[128];
    col_txfm(input,
             buf0,
             stride,
             bitcol,
             /*lr_flip=*/0,
             /*howmany=*/8,
             /*hm_stride=*/16);
    for (int i = 0; i < 8; i++) {
        shift_right_4_round_s32_x4(buf0 + i * 16, buf0 + i * 16, 8);
    }

    int32x4_t buf1[128];
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 32, 16, 32, 8);

    // Row-wise transform.
    row_txfm(buf1, buf0, bitrow, /*howmany=*/2, /*hm_stride=*/32);
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 16, 32, 8, 16);
    write_buffer_32xh_N2(buf1, output, 16);
}

void svt_av1_fwd_txfm2d_32x64_N2_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    (void)tx_type;
    int bitcol = fwd_cos_bit_col[3][4];
    int bitrow = fwd_cos_bit_row[3][4];

    // Column-wise transform.
    int32x4_t buf0[512];
    load_buffer_32x64(input, buf0, stride, 0);
    highbd_fdct64_N2_xn_neon(buf0, buf0, bitcol, 8);
    for (int i = 0; i < 8; i++) {
        shift_right_2_round_s32_x4(buf0 + i * 64, buf0 + i * 64, 32);
    }

    int32x4_t buf1[512];
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 32, 64, 32, 32);

    // Row-wise transform.
    highbd_fdct32_N2_xn_neon(buf1, buf0, bitrow, 8);
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 64, 32, 32, 16);
    for (int i = 0; i < 4; i++) {
        round_shift2_rect_array_s32_neon(buf1 + i * 64, buf1 + i * 64, 32);
    }
    write_buffer_32xh_N2(buf1, output, 64);
}

static inline void write_buffer_64xh_N2(int32x4_t* buf, int32_t* output, int height) {
    const int h = height >> 1;
    for (int i = 0; i < h; i++) {
        store_s32_8x4(output + i * 64,
                      4,
                      buf[i + 0 * height],
                      buf[i + 1 * height],
                      buf[i + 2 * height],
                      buf[i + 3 * height],
                      buf[i + 4 * height],
                      buf[i + 5 * height],
                      buf[i + 6 * height],
                      buf[i + 7 * height]);
        memset(output + i * 64 + 32, 0, 32 * sizeof(int32_t));
    }
    memset(output + 64 * h, 0, 64 * h * sizeof(int32_t));
}

void svt_av1_fwd_txfm2d_64x16_N2_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    const int bitcol = fwd_cos_bit_col[4][2];
    const int bitrow = fwd_cos_bit_row[4][2];

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 16);

    // Column-wise transform.
    int32x4_t buf0[256];
    load_buffer_64x16(input, buf0, stride, lr_flip);
    highbd_fdct16_N2_xn_neon(buf0, buf0, bitcol, 16);
    for (int i = 0; i < 16; i++) {
        shift_right_4_round_s32_x4(buf0 + i * 16, buf0 + i * 16, 8);
    }

    int32x4_t buf1[256];
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 64, 16, 64, 8);

    // Row-wise transform.
    highbd_fdct64_N2_xn_neon(buf1, buf0, bitrow, 2);
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 16, 64, 8, 32);
    write_buffer_64xh_N2(buf1, output, 16);
}

void svt_av1_fwd_txfm2d_64x32_N2_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    (void)tx_type;
    int bitcol = fwd_cos_bit_col[4][3];
    int bitrow = fwd_cos_bit_row[4][3];

    // Column-wise transform.
    int32x4_t buf0[512];
    load_buffer_64x32(input, buf0, stride, 0);
    highbd_fdct32_N2_xn_neon(buf0, buf0, bitcol, 16);
    for (int i = 0; i < 16; i++) {
        shift_right_4_round_s32_x4(buf0 + i * 32, buf0 + i * 32, 16);
    }

    int32x4_t buf1[512];
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 64, 32, 64, 16);

    // Row-wise transform.
    highbd_fdct64_N2_xn_neon(buf1, buf0, bitrow, 4);
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 32, 64, 16, 32);
    for (int i = 0; i < 8; i++) {
        round_shift2_rect_array_s32_neon(buf1 + i * 32, buf1 + i * 32, 16);
    }
    write_buffer_64xh_N2(buf1, output, 32);
}

TRANSFORM_COL_ONE(fdct8_N4, 8)
TRANSFORM_COL_ONE(fadst8_N4, 8)
TRANSFORM_COL_ONE(fidentity8_N4, 8)

TRANSFORM_ROW_RECT_ONE(fdct4_N4, 4)
TRANSFORM_ROW_RECT_ONE(fadst4_N4, 4)
TRANSFORM_ROW_RECT_ONE(fidentity4_N4, 4)

static const fwd_transform_1d_col_neon col_highbd_txfm8_x4_N4_arr[TX_TYPES] = {
    highbd_fdct8_N4_col_neon, // DCT_DCT
    highbd_fadst8_N4_col_neon, // ADST_DCT
    highbd_fdct8_N4_col_neon, // DCT_ADST
    highbd_fadst8_N4_col_neon, // ADST_ADST
    highbd_fadst8_N4_col_neon, // FLIPADST_DCT
    highbd_fdct8_N4_col_neon, // DCT_FLIPADST
    highbd_fadst8_N4_col_neon, // FLIPADST_FLIPADST
    highbd_fadst8_N4_col_neon, // ADST_FLIPADST
    highbd_fadst8_N4_col_neon, // FLIPADST_ADST
    highbd_fidentity8_N4_col_neon, // IDTX
    highbd_fdct8_N4_col_neon, // V_DCT
    highbd_fidentity8_N4_col_neon, // H_DCT
    highbd_fadst8_N4_col_neon, // V_ADST
    highbd_fidentity8_N4_col_neon, // H_ADST
    highbd_fadst8_N4_col_neon, // V_FLIPADST
    highbd_fidentity8_N4_col_neon // H_FLIPADST
};

static const fwd_transform_1d_row_neon row_rect_highbd_txfm4_x4_N4_arr[TX_TYPES] = {
    highbd_fdct4_N4_row_rect_neon, // DCT_DCT
    highbd_fdct4_N4_row_rect_neon, // ADST_DCT
    highbd_fadst4_N4_row_rect_neon, // DCT_ADST
    highbd_fadst4_N4_row_rect_neon, // ADST_ADST
    highbd_fdct4_N4_row_rect_neon, // FLIPADST_DCT
    highbd_fadst4_N4_row_rect_neon, // DCT_FLIPADST
    highbd_fadst4_N4_row_rect_neon, // FLIPADST_FLIPADST
    highbd_fadst4_N4_row_rect_neon, // ADST_FLIPADST
    highbd_fadst4_N4_row_rect_neon, // FLIPADST_ADST
    highbd_fidentity4_N4_row_rect_neon, // IDTX
    highbd_fidentity4_N4_row_rect_neon, // V_DCT
    highbd_fdct4_N4_row_rect_neon, // H_DCT
    highbd_fidentity4_N4_row_rect_neon, // V_ADST
    highbd_fadst4_N4_row_rect_neon, // H_ADST
    highbd_fidentity4_N4_row_rect_neon, // V_FLIPADST
    highbd_fadst4_N4_row_rect_neon // H_FLIPADST
};

static inline void write_buffer_4xh_N4(int32x4_t* buf, int32_t* output, int height) {
    const int h = height >> 2;
    for (int i = 0; i < h; i++) {
        output[i * 4] = vgetq_lane_s32(buf[i], 0);
        memset(output + i * 4 + 1, 0, 3 * sizeof(int32_t));
    }
    memset(output + 4 * h, 0, 4 * (height - h) * sizeof(int32_t));
}

void svt_av1_fwd_txfm2d_4x8_N4_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    int                             bitcol   = fwd_cos_bit_col[0][1];
    int                             bitrow   = fwd_cos_bit_row[0][1];
    const fwd_transform_1d_col_neon col_txfm = col_highbd_txfm8_x4_N4_arr[tx_type];
    const fwd_transform_1d_row_neon row_txfm = row_rect_highbd_txfm4_x4_N4_arr[tx_type];

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 8);

    // Column-wise transform.
    int32x4_t buf0[8];
    col_txfm(input, buf0, stride, bitcol, lr_flip);
    shift_right_1_round_s32_x4(buf0, buf0, 2);

    int32x4_t buf1[8];
    transpose_arrays_s32_4x4(buf0, buf1);

    // Row-wise transform.
    row_txfm(buf1, buf0, bitrow);
    transpose_arrays_s32_4x4(buf0, buf1);
    write_buffer_4xh_N4(buf1, output, 8);
}

TRANSFORM_COL_ONE(fdct16_N4, 16)
TRANSFORM_COL_ONE(fadst16_N4, 16)
TRANSFORM_COL_ONE(fidentity16_N4, 16)

static const fwd_transform_1d_col_neon col_highbd_txfm16_x4_N4_arr[TX_TYPES] = {
    highbd_fdct16_N4_col_neon, // DCT_DCT
    highbd_fadst16_N4_col_neon, // ADST_DCT
    highbd_fdct16_N4_col_neon, // DCT_ADST
    highbd_fadst16_N4_col_neon, // ADST_ADST
    highbd_fadst16_N4_col_neon, // FLIPADST_DCT
    highbd_fdct16_N4_col_neon, // DCT_FLIPADST
    highbd_fadst16_N4_col_neon, // FLIPADST_FLIPADST
    highbd_fadst16_N4_col_neon, // ADST_FLIPADST
    highbd_fadst16_N4_col_neon, // FLIPADST_ADST
    highbd_fidentity16_N4_col_neon, // IDTX
    highbd_fdct16_N4_col_neon, // V_DCT
    highbd_fidentity16_N4_col_neon, // H_DCT
    highbd_fadst16_N4_col_neon, // V_ADST
    highbd_fidentity16_N4_col_neon, // H_ADST
    highbd_fadst16_N4_col_neon, // V_FLIPADST
    highbd_fidentity16_N4_col_neon // H_FLIPADST
};

TRANSFORM_ROW_MANY(fdct4_N4, 4)
TRANSFORM_ROW_MANY(fadst4_N4, 4)
TRANSFORM_ROW_MANY(fidentity4_N4, 4)

static const fwd_transform_1d_row_many_neon row_highbd_txfm4_xn_N4_arr[TX_TYPES] = {
    highbd_fdct4_N4_row_many_neon, // DCT_DCT
    highbd_fdct4_N4_row_many_neon, // ADST_DCT
    highbd_fadst4_N4_row_many_neon, // DCT_ADST
    highbd_fadst4_N4_row_many_neon, // ADST_ADST
    highbd_fdct4_N4_row_many_neon, // FLIPADST_DCT
    highbd_fadst4_N4_row_many_neon, // DCT_FLIPADST
    highbd_fadst4_N4_row_many_neon, // FLIPADST_FLIPADST
    highbd_fadst4_N4_row_many_neon, // ADST_FLIPADST
    highbd_fadst4_N4_row_many_neon, // FLIPADST_ADST
    highbd_fidentity4_N4_row_many_neon, // IDTX
    highbd_fidentity4_N4_row_many_neon, // V_DCT
    highbd_fdct4_N4_row_many_neon, // H_DCT
    highbd_fidentity4_N4_row_many_neon, // V_ADST
    highbd_fadst4_N4_row_many_neon, // H_ADST
    highbd_fidentity4_N4_row_many_neon, // V_FLIPADST
    highbd_fadst4_N4_row_many_neon // H_FLIPADST
};

void svt_av1_fwd_txfm2d_4x16_N4_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    int                                  bitcol   = fwd_cos_bit_col[0][2];
    int                                  bitrow   = fwd_cos_bit_row[0][2];
    const fwd_transform_1d_col_neon      col_txfm = col_highbd_txfm16_x4_N4_arr[tx_type];
    const fwd_transform_1d_row_many_neon row_txfm = row_highbd_txfm4_xn_N4_arr[tx_type];

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 16);

    // Column-wise transform.
    int32x4_t buf0[16];
    if (lr_flip) {
        col_txfm(input,
                 buf0,
                 stride,
                 bitcol,
                 /*lr_flip=*/1);
    } else {
        col_txfm(input,
                 buf0,
                 stride,
                 bitcol,
                 /*lr_flip=*/0);
    }
    shift_right_1_round_s32_x4(buf0, buf0, 8);

    int32x4_t buf1[16];
    transpose_arrays_s32_4x4(buf0, buf1);

    // Row-wise transform.
    row_txfm(buf1, buf0, bitrow, /*howmany=*/2, /*hm_stride=*/4);
    transpose_arrays_s32_4x4(buf0, buf1);
    write_buffer_4xh_N4(buf1, output, 16);
}

TRANSFORM_COL_MANY(fdct4_N4, 4)
TRANSFORM_COL_MANY(fadst4_N4, 4)
TRANSFORM_COL_MANY(fidentity4_N4, 4)

static const fwd_transform_1d_col_many_neon col_highbd_txfm4_xn_N4_arr[TX_TYPES] = {
    highbd_fdct4_N4_col_many_neon, // DCT_DCT
    highbd_fadst4_N4_col_many_neon, // ADST_DCT
    highbd_fdct4_N4_col_many_neon, // DCT_ADST
    highbd_fadst4_N4_col_many_neon, // ADST_ADST
    highbd_fadst4_N4_col_many_neon, // FLIPADST_DCT
    highbd_fdct4_N4_col_many_neon, // DCT_FLIPADST
    highbd_fadst4_N4_col_many_neon, // FLIPADST_FLIPADST
    highbd_fadst4_N4_col_many_neon, // ADST_FLIPADST
    highbd_fadst4_N4_col_many_neon, // FLIPADST_ADST
    highbd_fidentity4_N4_col_many_neon, // IDTX
    highbd_fdct4_N4_col_many_neon, // V_DCT
    highbd_fidentity4_N4_col_many_neon, // H_DCT
    highbd_fadst4_N4_col_many_neon, // V_ADST
    highbd_fidentity4_N4_col_many_neon, // H_ADST
    highbd_fadst4_N4_col_many_neon, // V_FLIPADST
    highbd_fidentity4_N4_col_many_neon // H_FLIPADST
};

TRANSFORM_ROW_RECT_ONE(fdct8_N4, 8)
TRANSFORM_ROW_RECT_ONE(fadst8_N4, 8)
TRANSFORM_ROW_RECT_ONE(fidentity8_N4, 8)

static const fwd_transform_1d_row_neon row_rect_highbd_txfm8_x4_N4_arr[TX_TYPES] = {
    highbd_fdct8_N4_row_rect_neon, // DCT_DCT
    highbd_fdct8_N4_row_rect_neon, // ADST_DCT
    highbd_fadst8_N4_row_rect_neon, // DCT_ADST
    highbd_fadst8_N4_row_rect_neon, // ADST_ADST
    highbd_fdct8_N4_row_rect_neon, // FLIPADST_DCT
    highbd_fadst8_N4_row_rect_neon, // DCT_FLIPADST
    highbd_fadst8_N4_row_rect_neon, // FLIPADST_FLIPADST
    highbd_fadst8_N4_row_rect_neon, // ADST_FLIPADST
    highbd_fadst8_N4_row_rect_neon, // FLIPADST_ADST
    highbd_fidentity8_N4_row_rect_neon, // IDTX
    highbd_fidentity8_N4_row_rect_neon, // V_DCT
    highbd_fdct8_N4_row_rect_neon, // H_DCT
    highbd_fidentity8_N4_row_rect_neon, // V_ADST
    highbd_fadst8_N4_row_rect_neon, // H_ADST
    highbd_fidentity8_N4_row_rect_neon, // V_FLIPADST
    highbd_fadst8_N4_row_rect_neon // H_FLIPADST
};

static inline void write_buffer_8xh_N4(int32x4_t* buf, int32_t* output, int height) {
    const int h = height >> 2;
    for (int i = 0; i < h; i++) {
        vst1_s32(output + i * 8, vget_low_s32(buf[i]));
        memset(output + i * 8 + 2, 0, 6 * sizeof(int32_t));
    }
    memset(output + 8 * h, 0, 8 * (height - h) * sizeof(int32_t));
}

void svt_av1_fwd_txfm2d_8x4_N4_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    const int                            bitcol   = fwd_cos_bit_col[1][0];
    const int                            bitrow   = fwd_cos_bit_row[1][0];
    const fwd_transform_1d_col_many_neon col_txfm = col_highbd_txfm4_xn_N4_arr[tx_type];
    const fwd_transform_1d_row_neon      row_txfm = row_rect_highbd_txfm8_x4_N4_arr[tx_type];

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 4);

    // Column-wise transform.
    int32x4_t buf0[8];
    if (lr_flip) {
        col_txfm(input,
                 buf0 + 4,
                 stride,
                 bitcol,
                 /*lr_flip=*/1,
                 /*howmany=*/2,
                 /*hm_stride=*/-4);
    } else {
        col_txfm(input,
                 buf0,
                 stride,
                 bitcol,
                 /*lr_flip=*/0,
                 /*howmany=*/2,
                 /*hm_stride=*/4);
    }

    buf0[0] = vrshrq_n_s32(buf0[0], 1);
    buf0[4] = vrshrq_n_s32(buf0[4], 1);

    int32x4_t buf1[8];
    transpose_arrays_s32_8x4(buf0, buf1);

    // Row-wise transform.
    row_txfm(buf1, buf0, bitrow);
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 4, 8, 4, 4);
    write_buffer_8xh_N4(buf1, output, 4);
}

TRANSFORM_COL_MANY(fdct16_N4, 16)
TRANSFORM_COL_MANY(fadst16_N4, 16)
TRANSFORM_COL_MANY(fidentity16_N4, 16)

static const fwd_transform_1d_col_many_neon col_highbd_txfm16_xn_N4_arr[TX_TYPES] = {
    highbd_fdct16_N4_col_many_neon, // DCT_DCT
    highbd_fadst16_N4_col_many_neon, // ADST_DCT
    highbd_fdct16_N4_col_many_neon, // DCT_ADST
    highbd_fadst16_N4_col_many_neon, // ADST_ADST
    highbd_fadst16_N4_col_many_neon, // FLIPADST_DCT
    highbd_fdct16_N4_col_many_neon, // DCT_FLIPADST
    highbd_fadst16_N4_col_many_neon, // FLIPADST_FLIPADST
    highbd_fadst16_N4_col_many_neon, // ADST_FLIPADST
    highbd_fadst16_N4_col_many_neon, // FLIPADST_ADST
    highbd_fidentity16_N4_col_many_neon, // IDTX
    highbd_fdct16_N4_col_many_neon, // V_DCT
    highbd_fidentity16_N4_col_many_neon, // H_DCT
    highbd_fadst16_N4_col_many_neon, // V_ADST
    highbd_fidentity16_N4_col_many_neon, // H_ADST
    highbd_fadst16_N4_col_many_neon, // V_FLIPADST
    highbd_fidentity16_N4_col_many_neon // H_FLIPADST
};

void svt_av1_fwd_txfm2d_8x16_N4_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    const int                            bitcol   = fwd_cos_bit_col[1][2];
    const int                            bitrow   = fwd_cos_bit_row[1][2];
    const fwd_transform_1d_col_many_neon col_txfm = col_highbd_txfm16_xn_N4_arr[tx_type];
    const fwd_transform_1d_row_neon      row_txfm = row_rect_highbd_txfm8_x4_N4_arr[tx_type];

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 16);

    // Column-wise transform.
    int32x4_t buf0[32];
    if (lr_flip) {
        col_txfm(input,
                 buf0 + 16,
                 stride,
                 bitcol,
                 /*lr_flip=*/1,
                 /*howmany=*/2,
                 /*hm_stride=*/-16);
    } else {
        col_txfm(input,
                 buf0,
                 stride,
                 bitcol,
                 /*lr_flip=*/0,
                 /*howmany=*/2,
                 /*hm_stride=*/16);
    }

    shift_right_2_round_s32_x4(buf0, buf0, 4);
    shift_right_2_round_s32_x4(buf0 + 16, buf0 + 16, 4);

    int32x4_t buf1[32];
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 8, 16, 8, 4);

    // Row-wise transform.
    row_txfm(buf1, buf0, bitrow);
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 16, 8, 4, 4);
    write_buffer_8xh_N4(buf1, output, 16);
}

TRANSFORM_COL_MANY(fdct32_N4, 32)
TRANSFORM_COL_MANY(fidentity32_N4, 32)

static const fwd_transform_1d_col_many_neon col_highbd_txfm32_xn_N4_arr[TX_TYPES] = {
    highbd_fdct32_N4_col_many_neon, // DCT_DCT
    NULL, // ADST_DCT
    NULL, // DCT_ADST
    NULL, // ADST_ADST
    NULL, // FLIPADST_DCT
    NULL, // DCT_FLIPADST
    NULL, // FLIPADST_FLIPADST
    NULL, // ADST_FLIPADST
    NULL, // FLIPADST_ADST
    highbd_fidentity32_N4_col_many_neon, // IDTX
    NULL, // V_DCT
    NULL, // H_DCT
    NULL, // V_ADST
    NULL, // H_ADST
    NULL, // V_FLIPADST
    NULL // H_FLIPADST
};

TRANSFORM_ROW_MANY(fdct8_N4, 8)
TRANSFORM_ROW_MANY(fadst8_N4, 8)
TRANSFORM_ROW_MANY(fidentity8_N4, 8)

static const fwd_transform_1d_row_many_neon row_highbd_txfm8_xn_N4_arr[TX_TYPES] = {
    highbd_fdct8_N4_row_many_neon, // DCT_DCT
    highbd_fdct8_N4_row_many_neon, // ADST_DCT
    highbd_fadst8_N4_row_many_neon, // DCT_ADST
    highbd_fadst8_N4_row_many_neon, // ADST_ADST
    highbd_fdct8_N4_row_many_neon, // FLIPADST_DCT
    highbd_fadst8_N4_row_many_neon, // DCT_FLIPADST
    highbd_fadst8_N4_row_many_neon, // FLIPADST_FLIPADST
    highbd_fadst8_N4_row_many_neon, // ADST_FLIPADST
    highbd_fadst8_N4_row_many_neon, // FLIPADST_ADST
    highbd_fidentity8_N4_row_many_neon, // IDTX
    highbd_fidentity8_N4_row_many_neon, // V_DCT
    highbd_fdct8_N4_row_many_neon, // H_DCT
    highbd_fidentity8_N4_row_many_neon, // V_ADST
    highbd_fadst8_N4_row_many_neon, // H_ADST
    highbd_fidentity8_N4_row_many_neon, // V_FLIPADST
    highbd_fadst8_N4_row_many_neon // H_FLIPADST
};

void svt_av1_fwd_txfm2d_8x32_N4_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    const int                            bitcol   = fwd_cos_bit_col[1][3];
    const int                            bitrow   = fwd_cos_bit_row[1][3];
    const fwd_transform_1d_col_many_neon col_txfm = col_highbd_txfm32_xn_N4_arr[tx_type];
    const fwd_transform_1d_row_many_neon row_txfm = row_highbd_txfm8_xn_N4_arr[tx_type];

    // Column-wise transform.
    int32x4_t buf0[64];
    col_txfm(input,
             buf0,
             stride,
             bitcol,
             /*lr_flip=*/0,
             /*howmany=*/2,
             /*hm_stride=*/32);

    shift_right_2_round_s32_x4(buf0, buf0, 8);
    shift_right_2_round_s32_x4(buf0 + 32, buf0 + 32, 8);

    int32x4_t buf1[64];
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 8, 32, 8, 8);

    // Row-wise transform.
    row_txfm(buf1, buf0, bitrow, /*howmany=*/2, /*hm_stride=*/8);
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 32, 8, 8, 4);
    write_buffer_8xh_N4(buf1, output, 32);
}

TRANSFORM_ROW_ONE(fdct16_N4, 16)
TRANSFORM_ROW_ONE(fadst16_N4, 16)
TRANSFORM_ROW_ONE(fidentity16_N4, 16)

static const fwd_transform_1d_row_neon row_highbd_txfm16_x4_N4_arr[TX_TYPES] = {
    highbd_fdct16_N4_row_neon, // DCT_DCT
    highbd_fdct16_N4_row_neon, // ADST_DCT
    highbd_fadst16_N4_row_neon, // DCT_ADST
    highbd_fadst16_N4_row_neon, // ADST_ADST
    highbd_fdct16_N4_row_neon, // FLIPADST_DCT
    highbd_fadst16_N4_row_neon, // DCT_FLIPADST
    highbd_fadst16_N4_row_neon, // FLIPADST_FLIPADST
    highbd_fadst16_N4_row_neon, // ADST_FLIPADST
    highbd_fadst16_N4_row_neon, // FLIPADST_ADST
    highbd_fidentity16_N4_row_neon, // IDTX
    highbd_fidentity16_N4_row_neon, // V_DCT
    highbd_fdct16_N4_row_neon, // H_DCT
    highbd_fidentity16_N4_row_neon, // V_ADST
    highbd_fadst16_N4_row_neon, // H_ADST
    highbd_fidentity16_N4_row_neon, // V_FLIPADST
    highbd_fadst16_N4_row_neon // H_FLIPADST
};

static inline void write_buffer_16xh_N4(int32x4_t* buf, int32_t* output, int height) {
    const int h = height >> 2;
    for (int i = 0; i < h; i++) {
        vst1q_s32(output + i * 16, buf[i]);
        memset(output + i * 16 + 4, 0, 12 * sizeof(int32_t));
    }
    memset(output + 16 * h, 0, 16 * (height - h) * sizeof(int32_t));
}

void svt_av1_fwd_txfm2d_16x4_N4_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    int                                  bitcol   = fwd_cos_bit_col[2][0];
    int                                  bitrow   = fwd_cos_bit_row[2][0];
    const fwd_transform_1d_col_many_neon col_txfm = col_highbd_txfm4_xn_N4_arr[tx_type];
    const fwd_transform_1d_row_neon      row_txfm = row_highbd_txfm16_x4_N4_arr[tx_type];

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 4);

    // Column-wise transform.
    int32x4_t buf0[16];
    if (lr_flip) {
        col_txfm(input,
                 buf0 + 3 * 4,
                 stride,
                 bitcol,
                 /*lr_flip=*/1,
                 /*howmany=*/4,
                 /*hm_stride=*/-4);
    } else {
        col_txfm(input,
                 buf0,
                 stride,
                 bitcol,
                 /*lr_flip=*/0,
                 /*howmany=*/4,
                 /*hm_stride=*/4);
    }

    for (int i = 0; i < 4; i++) {
        buf0[i * 4] = vrhaddq_s32(buf0[i * 4], vdupq_n_s32(0));
    }

    int32x4_t buf1[16];
    transpose_arrays_s32_16x4(buf0, buf1);

    // Row-wise transform.
    row_txfm(buf1, buf0, bitrow);
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 4, 16, 4, 4);
    write_buffer_16xh_N4(buf1, output, 4);
}

TRANSFORM_COL_MANY(fdct8_N4, 8)
TRANSFORM_COL_MANY(fadst8_N4, 8)
TRANSFORM_COL_MANY(fidentity8_N4, 8)

static const fwd_transform_1d_col_many_neon col_highbd_txfm8_xn_N4_arr[TX_TYPES] = {
    highbd_fdct8_N4_col_many_neon, // DCT_DCT
    highbd_fadst8_N4_col_many_neon, // ADST_DCT
    highbd_fdct8_N4_col_many_neon, // DCT_ADST
    highbd_fadst8_N4_col_many_neon, // ADST_ADST
    highbd_fadst8_N4_col_many_neon, // FLIPADST_DCT
    highbd_fdct8_N4_col_many_neon, // DCT_FLIPADST
    highbd_fadst8_N4_col_many_neon, // FLIPADST_FLIPADST
    highbd_fadst8_N4_col_many_neon, // ADST_FLIPADST
    highbd_fadst8_N4_col_many_neon, // FLIPADST_ADST
    highbd_fidentity8_N4_col_many_neon, // IDTX
    highbd_fdct8_N4_col_many_neon, // V_DCT
    highbd_fidentity8_N4_col_many_neon, // H_DCT
    highbd_fadst8_N4_col_many_neon, // V_ADST
    highbd_fidentity8_N4_col_many_neon, // H_ADST
    highbd_fadst8_N4_col_many_neon, // V_FLIPADST
    highbd_fidentity8_N4_col_many_neon // H_FLIPADST
};

TRANSFORM_ROW_RECT_ONE(fdct16_N4, 16)
TRANSFORM_ROW_RECT_ONE(fadst16_N4, 16)
TRANSFORM_ROW_RECT_ONE(fidentity16_N4, 16)

static const fwd_transform_1d_row_neon row_rect_highbd_txfm16_x4_N4_arr[TX_TYPES] = {
    highbd_fdct16_N4_row_rect_neon, // DCT_DCT
    highbd_fdct16_N4_row_rect_neon, // ADST_DCT
    highbd_fadst16_N4_row_rect_neon, // DCT_ADST
    highbd_fadst16_N4_row_rect_neon, // ADST_ADST
    highbd_fdct16_N4_row_rect_neon, // FLIPADST_DCT
    highbd_fadst16_N4_row_rect_neon, // DCT_FLIPADST
    highbd_fadst16_N4_row_rect_neon, // FLIPADST_FLIPADST
    highbd_fadst16_N4_row_rect_neon, // ADST_FLIPADST
    highbd_fadst16_N4_row_rect_neon, // FLIPADST_ADST
    highbd_fidentity16_N4_row_rect_neon, // IDTX
    highbd_fidentity16_N4_row_rect_neon, // V_DCT
    highbd_fdct16_N4_row_rect_neon, // H_DCT
    highbd_fidentity16_N4_row_rect_neon, // V_ADST
    highbd_fadst16_N4_row_rect_neon, // H_ADST
    highbd_fidentity16_N4_row_rect_neon, // V_FLIPADST
    highbd_fadst16_N4_row_rect_neon // H_FLIPADST
};

void svt_av1_fwd_txfm2d_16x8_N4_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    const fwd_transform_1d_col_many_neon col_txfm = col_highbd_txfm8_xn_N4_arr[tx_type];
    const fwd_transform_1d_row_neon      row_txfm = row_rect_highbd_txfm16_x4_N4_arr[tx_type];
    int                                  bit      = fwd_cos_bit_col[2][1];

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 8);

    // Column-wise transform.
    int32x4_t buf0[32];
    if (lr_flip) {
        col_txfm(input,
                 buf0 + 3 * 8,
                 stride,
                 bit,
                 /*lr_flip=*/1,
                 /*howmany=*/4,
                 /*hm_stride=*/-8);
    } else {
        col_txfm(input,
                 buf0,
                 stride,
                 bit,
                 /*lr_flip=*/0,
                 /*howmany=*/4,
                 /*hm_stride=*/8);
    }
    for (int i = 0; i < 4; i++) {
        shift_right_2_round_s32_x4(buf0 + i * 8, buf0 + i * 8, 2);
    }

    int32x4_t buf1[32];
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 16, 8, 16, 4);

    // Row-wise transform.
    row_txfm(buf1, buf0, bit);
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 8, 16, 4, 4);
    write_buffer_16xh_N4(buf1, output, 8);
}

TRANSFORM_ROW_RECT_MANY(fdct16_N4, 16)
TRANSFORM_ROW_RECT_MANY(fadst16_N4, 16)
TRANSFORM_ROW_RECT_MANY(fidentity16_N4, 16)

static const fwd_transform_1d_row_many_neon row_rect_highbd_txfm16_xn_N4_arr[TX_TYPES] = {
    highbd_fdct16_N4_row_rect_many_neon, // DCT_DCT
    highbd_fdct16_N4_row_rect_many_neon, // ADST_DCT
    highbd_fadst16_N4_row_rect_many_neon, // DCT_ADST
    highbd_fadst16_N4_row_rect_many_neon, // ADST_ADST
    highbd_fdct16_N4_row_rect_many_neon, // FLIPADST_DCT
    highbd_fadst16_N4_row_rect_many_neon, // DCT_FLIPADST
    highbd_fadst16_N4_row_rect_many_neon, // FLIPADST_FLIPADST
    highbd_fadst16_N4_row_rect_many_neon, // ADST_FLIPADST
    highbd_fadst16_N4_row_rect_many_neon, // FLIPADST_ADST
    highbd_fidentity16_N4_row_rect_many_neon, // IDTX
    highbd_fidentity16_N4_row_rect_many_neon, // V_DCT
    highbd_fdct16_N4_row_rect_many_neon, // H_DCT
    highbd_fidentity16_N4_row_rect_many_neon, // V_ADST
    highbd_fadst16_N4_row_rect_many_neon, // H_ADST
    highbd_fidentity16_N4_row_rect_many_neon, // V_FLIPADST
    highbd_fadst16_N4_row_rect_many_neon // H_FLIPADST
};

void svt_av1_fwd_txfm2d_16x32_N4_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    const fwd_transform_1d_col_many_neon col_txfm = col_highbd_txfm32_xn_N4_arr[tx_type];
    const fwd_transform_1d_row_many_neon row_txfm = row_rect_highbd_txfm16_xn_N4_arr[tx_type];
    int                                  bitcol   = fwd_cos_bit_col[2][3];
    int                                  bitrow   = fwd_cos_bit_row[2][3];

    // Column-wise transform.
    int32x4_t buf0[128];
    col_txfm(input,
             buf0,
             stride,
             bitcol,
             /*lr_flip=*/0,
             /*howmany=*/4,
             /*hm_stride=*/32);
    for (int i = 0; i < 4; i++) {
        shift_right_4_round_s32_x4(buf0 + i * 32, buf0 + i * 32, 8);
    }

    int32x4_t buf1[128];
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 16, 32, 16, 8);

    // Row-wise transform.
    row_txfm(buf1, buf0, bitrow, /*howmany=*/4, /*hm_stride=*/16);
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 32, 16, 8, 4);
    write_buffer_16xh_N4(buf1, output, 32);
}

void svt_av1_fwd_txfm2d_16x64_N4_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    const int bitcol = fwd_cos_bit_col[2][4];
    const int bitrow = fwd_cos_bit_row[2][4];

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 64);

    // Column-wise transform.
    int32x4_t buf0[256];
    load_buffer_16x64(input, buf0, stride, lr_flip);
    highbd_fdct64_N4_xn_neon(buf0, buf0, bitcol, 4);
    for (int i = 0; i < 4; i++) {
        shift_right_2_round_s32_x4(buf0 + i * 64, buf0 + i * 64, 16);
    }

    int32x4_t buf1[256];
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 16, 64, 16, 16);

    // Row-wise transform.
    highbd_fdct16_N4_xn_neon(buf1, buf0, bitrow, 4);
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 64, 16, 16, 4);
    write_buffer_16xh_N4(buf1, output, 64);
}

static inline void write_buffer_32xh_N4(int32x4_t* buf, int32_t* output, int height) {
    const int h = height >> 2;
    for (int i = 0; i < h; i++) {
        vst1q_s32(output + i * 32, buf[i]);
        vst1q_s32(output + i * 32 + 4, buf[i + height]);
        memset(output + i * 32 + 8, 0, 24 * sizeof(int32_t));
    }
    memset(output + 32 * h, 0, 32 * (height - h) * sizeof(int32_t));
}

TRANSFORM_ROW_ONE(fdct32_N4, 32)

void svt_av1_fwd_txfm2d_32x8_N4_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    int       bitcol = fwd_cos_bit_col[3][1];
    int       bitrow = fwd_cos_bit_row[3][1];
    int32x4_t buf0[64];

    switch (tx_type) {
    case DCT_DCT:
        // Column-wise transform.
        highbd_fdct8_N4_col_many_neon(input,
                                      buf0,
                                      stride,
                                      bitcol,
                                      /*lr_flip=*/0,
                                      /*howmany=*/8,
                                      /*hm_stride=*/8);
        for (int i = 0; i < 8; i++) {
            shift_right_2_round_s32_x4(buf0 + i * 8, buf0 + i * 8, 2);
        }

        int32x4_t buf1[64];
        transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 32, 8, 32, 4);

        // Row-wise transform.
        highbd_fdct32_N4_row_neon(buf1, buf0, bitrow);
        transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 8, 32, 4, 8);
        write_buffer_32xh_N4(buf1, output, 8);
        break;
    case IDTX:
        // Column-wise transform.
        highbd_fidentity8_N4_col_many_neon(input,
                                           buf0,
                                           stride,
                                           bitcol,
                                           /*lr_flip=*/0,
                                           /*howmany=*/2,
                                           /*hm_stride=*/8);

        // The second transform only performs a shift left by two, so it cancels
        // out the intermediate shift right by 2.
        write_buffer_32xh_N4(buf0, output, 8);
        break;
    default:
        assert(0);
    }
}

TRANSFORM_ROW_RECT_ONE(fdct32_N4, 32)
TRANSFORM_ROW_RECT_ONE(fidentity32_N4, 32)

static const fwd_transform_1d_row_neon row_rect_highbd_txfm32_x4_N4_arr[TX_TYPES] = {
    highbd_fdct32_N4_row_rect_neon, // DCT_DCT
    NULL, // ADST_DCT
    NULL, // DCT_ADST
    NULL, // ADST_ADST
    NULL, // FLIPADST_DCT
    NULL, // DCT_FLIPADST
    NULL, // FLIPADST_FLIPADST
    NULL, // ADST_FLIPADST
    NULL, // FLIPADST_ADST
    highbd_fidentity32_N4_row_rect_neon, // IDTX
    NULL, // V_DCT
    NULL, // H_DCT
    NULL, // V_ADST
    NULL, // H_ADST
    NULL, // V_FLIPADST
    NULL // H_FLIPADST

};

void svt_av1_fwd_txfm2d_32x16_N4_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    const fwd_transform_1d_col_many_neon col_txfm = col_highbd_txfm16_xn_N4_arr[tx_type];
    const fwd_transform_1d_row_neon      row_txfm = row_rect_highbd_txfm32_x4_N4_arr[tx_type];
    int                                  bitcol   = fwd_cos_bit_col[3][2];
    int                                  bitrow   = fwd_cos_bit_row[3][2];

    // Column-wise transform.
    int32x4_t buf0[128];
    col_txfm(input,
             buf0,
             stride,
             bitcol,
             /*lr_flip=*/0,
             /*howmany=*/8,
             /*hm_stride=*/16);
    for (int i = 0; i < 8; i++) {
        shift_right_4_round_s32_x4(buf0 + i * 16, buf0 + i * 16, 4);
    }

    int32x4_t buf1[128];
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 32, 16, 32, 4);

    // Row-wise transform.
    row_txfm(buf1, buf0, bitrow);
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 16, 32, 4, 8);
    write_buffer_32xh_N4(buf1, output, 16);
}

void svt_av1_fwd_txfm2d_32x64_N4_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    (void)tx_type;
    int bitcol = fwd_cos_bit_col[3][4];
    int bitrow = fwd_cos_bit_row[3][4];

    // Column-wise transform.
    int32x4_t buf0[512];
    load_buffer_32x64(input, buf0, stride, 0);
    highbd_fdct64_N4_xn_neon(buf0, buf0, bitcol, 8);
    for (int i = 0; i < 8; i++) {
        shift_right_2_round_s32_x4(buf0 + i * 64, buf0 + i * 64, 16);
    }

    int32x4_t buf1[512];
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 32, 64, 32, 16);

    // Row-wise transform.
    highbd_fdct32_N4_xn_neon(buf1, buf0, bitrow, 4);
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 64, 32, 16, 8);
    for (int i = 0; i < 2; i++) {
        round_shift2_rect_array_s32_neon(buf1 + i * 64, buf1 + i * 64, 16);
    }
    write_buffer_32xh_N4(buf1, output, 64);
}

static inline void write_buffer_64xh_N4(int32x4_t* buf, int32_t* output, int height) {
    const int h = height >> 2;
    for (int i = 0; i < h; i++) {
        store_s32_4x4(
            output + i * 64, 4, buf[i + 0 * height], buf[i + 1 * height], buf[i + 2 * height], buf[i + 3 * height]);
        memset(output + i * 64 + 16, 0, 48 * sizeof(int32_t));
    }
    memset(output + 64 * h, 0, 64 * (height - h) * sizeof(int32_t));
}

void svt_av1_fwd_txfm2d_64x16_N4_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    const int bitcol = fwd_cos_bit_col[4][2];
    const int bitrow = fwd_cos_bit_row[4][2];

    int ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    ud_adjust_input_and_stride(ud_flip, &input, &stride, 16);

    // Column-wise transform.
    int32x4_t buf0[256];
    load_buffer_64x16(input, buf0, stride, lr_flip);
    highbd_fdct16_N4_xn_neon(buf0, buf0, bitcol, 16);
    for (int i = 0; i < 16; i++) {
        shift_right_4_round_s32_x4(buf0 + i * 16, buf0 + i * 16, 4);
    }

    int32x4_t buf1[256];
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 64, 16, 64, 4);

    // Row-wise transform.
    highbd_fdct64_N4_xn_neon(buf1, buf0, bitrow, 2);
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 16, 64, 4, 16);
    write_buffer_64xh_N4(buf1, output, 16);
}

void svt_av1_fwd_txfm2d_64x32_N4_neon(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    (void)tx_type;
    int bitcol = fwd_cos_bit_col[4][3];
    int bitrow = fwd_cos_bit_row[4][3];

    // Column-wise transform.
    int32x4_t buf0[512];
    load_buffer_64x32(input, buf0, stride, 0);
    highbd_fdct32_N2_xn_neon(buf0, buf0, bitcol, 16);
    for (int i = 0; i < 16; i++) {
        shift_right_4_round_s32_x4(buf0 + i * 32, buf0 + i * 32, 8);
    }

    int32x4_t buf1[512];
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 64, 32, 64, 8);

    // Row-wise transform.
    highbd_fdct64_N4_xn_neon(buf1, buf0, bitrow, 2);
    transpose_arrays_s32_4nx4n_in_4mx4m(buf0, buf1, 32, 64, 8, 16);
    for (int i = 0; i < 4; i++) {
        round_shift2_rect_array_s32_neon(buf1 + i * 32, buf1 + i * 32, 8);
    }
    write_buffer_64xh_N4(buf1, output, 32);
}
