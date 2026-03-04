/*
 * Copyright(c) 2019 Netflix, Inc.
 * Copyright (c) 2016, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the
 * Alliance for Open Media Patent License 1.0 was not distributed with this
 * source code in the PATENTS file, you can obtain it at
 * https://www.aomedia.org/license/patent-license.
 */

/******************************************************************************
 * @file TxfmRef.cc
 *
 * @brief reference implementation for txfm, including :
 * - reference_dct_1d
 * - reference_adst_1d
 * - reference_idtx_1d
 * - reference_txfm_1d
 * - reference_txfm_2d
 * - fadst_ref
 *
 * @author Cidana-Edmond, Cidana-Wenyao
 *
 ******************************************************************************/
#include "TxfmRef.h"
namespace svt_av1_test_reference {

double get_scale_factor(const Txfm2dFlipCfg &cfg, const int tx_width,
                        const int tx_height) {
    const int8_t *shift = cfg.shift;
    const int amplify_bit = shift[0] + shift[1] + shift[2];
    double scale_factor =
        amplify_bit >= 0 ? (1 << amplify_bit) : (1.0 / (1 << -amplify_bit));

    // For rectangular transforms, we need to multiply by an extra factor.
    const int rect_type = get_rect_tx_log_ratio(tx_width, tx_height);
    if (abs(rect_type) == 1) {
        scale_factor *= pow(2, 0.5);
    }
    return scale_factor;
}

void reference_txfm_2d(const double *in, double *out, TxType tx_type,
                       TxSize tx_size, double scale_factor) {
    // Get transform type and size of each dimension.
    const TxType1D column_type = vtx_tab[tx_type];
    const TxType1D row_type = htx_tab[tx_type];
    const int tx_width = tx_size_wide[tx_size];
    const int tx_height = tx_size_high[tx_size];
    double *tmp_input = new double[tx_width * tx_height];
    double *tmp_output = new double[tx_width * tx_height];

    // second forward transform with row_type
    for (int r = 0; r < tx_height; ++r) {
        reference_txfm_1d(
            row_type, in + r * tx_width, out + r * tx_width, tx_width);
    }
    // matrix transposition
    for (int r = 0; r < tx_height; ++r) {
        for (int c = 0; c < tx_width; ++c) {
            tmp_input[c * tx_height + r] = out[r * tx_width + c];
        }
    }
    // first forward transform with column_type
    for (int c = 0; c < tx_width; ++c) {
        reference_txfm_1d(column_type,
                          tmp_input + c * tx_height,
                          tmp_output + c * tx_height,
                          tx_height);
    }
    // matrix transposition
    for (int r = 0; r < tx_width; ++r) {
        for (int c = 0; c < tx_height; ++c) {
            out[c * tx_width + r] = tmp_output[r * tx_height + c];
        }
    }

    // appropriate scale
    for (int r = 0; r < tx_height; ++r) {
        for (int c = 0; c < tx_width; ++c) {
            out[r * tx_width + c] *= scale_factor;
        }
    }

    if (tmp_input)
        delete[] tmp_input;
    if (tmp_output)
        delete[] tmp_output;
}

void fadst4_ref(const TranLow *input, TranLow *output) {
    //  16384 * sqrt(2) * sin(kPi/9) * 2 / 3
    const TranHigh sinpi_1_9 = 5283;
    const TranHigh sinpi_2_9 = 9929;
    const TranHigh sinpi_3_9 = 13377;
    const TranHigh sinpi_4_9 = 15212;

    TranHigh x0, x1, x2, x3;
    TranHigh s0, s1, s2, s3, s4, s5, s6, s7;
    x0 = input[0];
    x1 = input[1];
    x2 = input[2];
    x3 = input[3];

    if (!(x0 | x1 | x2 | x3)) {
        output[0] = output[1] = output[2] = output[3] = 0;
        return;
    }
    s0 = sinpi_1_9 * x0;
    s1 = sinpi_4_9 * x0;
    s2 = sinpi_2_9 * x1;
    s3 = sinpi_1_9 * x1;
    s4 = sinpi_3_9 * x2;
    s5 = sinpi_4_9 * x3;
    s6 = sinpi_2_9 * x3;
    s7 = x0 + x1 - x3;

    x0 = s0 + s2 + s5;
    x1 = sinpi_3_9 * s7;
    x2 = s1 - s3 + s6;
    x3 = s4;

    s0 = x0 + x3;
    s1 = x1;
    s2 = x2 - x3;
    s3 = x2 - x0 + x3;

    // 1-D transform scaling factor is sqrt(2).
    output[0] = (TranLow)svt_av1_test_tool::round_shift(s0, 14);
    output[1] = (TranLow)svt_av1_test_tool::round_shift(s1, 14);
    output[2] = (TranLow)svt_av1_test_tool::round_shift(s2, 14);
    output[3] = (TranLow)svt_av1_test_tool::round_shift(s3, 14);
}

void reference_idtx_1d(const double *in, double *out, int size) {
    const double Sqrt2 = 1.4142135623730950488016887242097f;
    double scale = 0;
    switch (size) {
    case 4: scale = Sqrt2; break;
    case 8: scale = 2; break;
    case 16: scale = 2 * Sqrt2; break;
    case 32: scale = 4; break;
    case 64: scale = 4 * Sqrt2; break;
    default: assert(0); break;
    }

    for (int k = 0; k < size; ++k) {
        out[k] = in[k] * scale;
    }
}

void reference_dct_1d(const double *in, double *out, int size) {
    const double kInvSqrt2 = 0.707106781186547524400844362104f;
    for (int k = 0; k < size; ++k) {
        out[k] = 0;
        for (int n = 0; n < size; ++n) {
            out[k] += in[n] * cos(PI * (2 * n + 1) * k / (2 * size));
        }
        if (k == 0)
            out[k] = out[k] * kInvSqrt2;
    }
}

void reference_adst_1d(const double *in, double *out, int size) {
    if (size == 4) {  // Special case.
        TranLow int_input[4];
        for (int i = 0; i < 4; ++i) {
            int_input[i] = static_cast<TranLow>(round(in[i]));
        }

        TranLow int_output[4];
        fadst4_ref(int_input, int_output);
        for (int i = 0; i < 4; ++i) {
            out[i] = int_output[i];
        }
        return;
    }
    for (int k = 0; k < size; ++k) {
        out[k] = 0;
        for (int n = 0; n < size; ++n) {
            out[k] += in[n] * sin(PI * (2 * n + 1) * (2 * k + 1) / (4 * size));
        }
    }
}

void reference_txfm_1d(TxType1D type, const double *in, double *out, int size) {
    switch (type) {
    case DCT_1D: reference_dct_1d(in, out, size); break;
    case ADST_1D:
    case FLIPADST_1D: reference_adst_1d(in, out, size); break;
    case IDTX_1D: reference_idtx_1d(in, out, size); break;
    default: assert(0); break;
    }
}

TxType1D get_txfm1d_types(TxfmType txfm_type) {
    switch (txfm_type) {
    case TXFM_TYPE_DCT4:
    case TXFM_TYPE_DCT8:
    case TXFM_TYPE_DCT16:
    case TXFM_TYPE_DCT32:
    case TXFM_TYPE_DCT64: return DCT_1D;
    case TXFM_TYPE_ADST4:
    case TXFM_TYPE_ADST8:
    case TXFM_TYPE_ADST16:
    case TXFM_TYPE_ADST32: return ADST_1D;
    case TXFM_TYPE_IDENTITY4:
    case TXFM_TYPE_IDENTITY8:
    case TXFM_TYPE_IDENTITY16:
    case TXFM_TYPE_IDENTITY32:
    case TXFM_TYPE_IDENTITY64: return IDTX_1D;
    default: assert(0); return TX_TYPES_1D;
    }
}

}  // namespace svt_av1_test_reference
