/*
 * Copyright(c) 2019 Netflix, Inc.
 * Copyright (c) 2017, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the
 * Alliance for Open Media Patent License 1.0 was not distributed with this
 * source code in the PATENTS file, you can obtain it at
 * https://www.aomedia.org/license/patent-license.
 */

#include "gtest/gtest.h"

#include "aom_dsp_rtcd.h"
#include "md_config_process.h"
#include "random.h"
#include "util.h"

#include "definitions.h"
#include "pcs.h"
#include "transforms.h"
#include "unit_test_utility.h"
#include "q_matrices.h"

namespace {
using svt_av1_test_tool::SVTRandom;

#define QUAN_PARAM_LIST                                                      \
    const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr,    \
        const int16_t *round_ptr, const int16_t *quant_ptr,                  \
        const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr,                 \
        TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, \
        const int16_t *scan, const int16_t *iscan
#define QUAN_HBD_PARAM int16_t log_scale
#define QUAN_QM_PARAM_LIST                                                   \
    const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr,    \
        const int16_t *round_ptr, const int16_t *quant_ptr,                  \
        const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr,                 \
        TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, \
        const int16_t *scan, const int16_t *iscan, const QmVal *qm_ptr,      \
        const QmVal *iqm_ptr

typedef void (*QuantizeFunc)(QUAN_PARAM_LIST);
typedef void (*QuantizeHbdFunc)(QUAN_PARAM_LIST, QUAN_HBD_PARAM);
typedef void (*QuantizeQmFunc)(QUAN_QM_PARAM_LIST, QUAN_HBD_PARAM);

enum QuantType { TYPE_B, TYPE_DC, TYPE_FP };

using std::tuple;
typedef tuple<QuantizeFunc, QuantizeFunc, TxSize, QuantType, EbBitDepth>
    QuantizeParam;
typedef tuple<QuantizeHbdFunc, QuantizeHbdFunc, TxSize, QuantType, EbBitDepth>
    QuantizeHbdParam;
typedef tuple<QuantizeQmFunc, QuantizeQmFunc, TxSize, QuantType, EbBitDepth>
    QuantizeQmParam;

typedef struct {
    Quants quant;
    Dequants dequant;
} QuanTable;

const int kTestNum = 1000;

template <typename ParamType, typename FuncType>
class QuantizeTest : public ::testing::TestWithParam<ParamType> {
  protected:
    QuantizeTest()
        : rnd_(32, true),
          quant_ref_(nullptr),
          quant_(nullptr),
          tx_size_(TX_4X4),
          type_(TYPE_FP),
          bd_(EB_EIGHT_BIT) {
    }

    virtual ~QuantizeTest() {
    }

    virtual void SetUp() {
        qtab_ =
            reinterpret_cast<QuanTable *>(svt_aom_memalign(32, sizeof(*qtab_)));
        const int n_coeffs = coeff_num();
        coeff_ = reinterpret_cast<TranLow *>(
            svt_aom_memalign(32, 6 * n_coeffs * sizeof(TranLow)));
        InitQuantizer();
    }

    virtual void TearDown() {
        svt_aom_free(qtab_);
        qtab_ = NULL;
        svt_aom_free(coeff_);
        coeff_ = NULL;
    }

    void InitQuantizer() {
        PictureParentControlSet pcs;
        pcs.scs = new SequenceControlSet;
        pcs.frm_hdr.quantization_params.base_q_idx = 0;
        pcs.scs->static_config.sharpness = 0;
        PictureParentControlSet *pcs_ptr = &pcs;
        svt_av1_build_quantizer(
            pcs_ptr, bd_, 0, 0, 0, 0, 0, &qtab_->quant, &qtab_->dequant);
        delete pcs.scs;
    }

    virtual void QuantizeRun(bool is_loop, int q = 0, int test_num = 1) = 0;

    int coeff_num() const {
        return av1_get_max_eob(tx_size_);
    }

    void FillCoeff(TranLow c) {
        const int n_coeffs = coeff_num();
        for (int i = 0; i < n_coeffs; ++i) {
            coeff_[i] = c;
        }
    }

    void FillCoeffRandom() {
        const int n_coeffs = coeff_num();
        FillCoeffZero();
        int num = rnd_.random() % n_coeffs;
        for (int i = 0; i < num; ++i) {
            coeff_[i] = GetRandomCoeff();
        }
    }

    void FillCoeffRandomRows(int num) {
        FillCoeffZero();
        for (int i = 0; i < num; ++i) {
            coeff_[i] = GetRandomCoeff();
        }
    }

    void FillCoeffZero() {
        FillCoeff(0);
    }

    void FillDcOnly() {
        FillCoeffZero();
        coeff_[0] = GetRandomCoeff();
    }

    void FillDcLargeNegative() {
        FillCoeffZero();
        // Generate a qcoeff which contains 512/-512 (0x0100/0xFE00) to catch
        // issues like BUG=883 where the constant being compared was incorrectly
        // initialized.
        coeff_[0] = -8191;
    }

    TranLow GetRandomCoeff() {
        TranLow coeff;
        if (bd_ == EB_EIGHT_BIT) {
            coeff = clamp(
                static_cast<int16_t>(rnd_.random()), INT16_MIN + 1, INT16_MAX);
        } else {
            TranLow min = -(1 << (7 + bd_));
            TranLow max = -min - 1;
            coeff = clamp(static_cast<TranLow>(rnd_.random()), min, max);
        }
        return coeff;
    }

    SVTRandom rnd_;
    QuanTable *qtab_;
    TranLow *coeff_;
    FuncType quant_ref_;
    FuncType quant_;
    TxSize tx_size_;
    QuantType type_;
    EbBitDepth bd_;
};

class QuantizeLbdTest : public QuantizeTest<QuantizeParam, QuantizeFunc> {
  protected:
    QuantizeLbdTest() {
        quant_ref_ = TEST_GET_PARAM(0);
        quant_ = TEST_GET_PARAM(1);
        tx_size_ = TEST_GET_PARAM(2);
        type_ = TEST_GET_PARAM(3);
        bd_ = TEST_GET_PARAM(4);
    }

    void QuantizeRun(bool is_loop, int q = 0, int test_num = 1) override {
        TranLow *coeff_ptr = coeff_;
        const intptr_t n_coeffs = coeff_num();

        TranLow *qcoeff_ref = coeff_ptr + n_coeffs;
        TranLow *dqcoeff_ref = qcoeff_ref + n_coeffs;

        TranLow *qcoeff = dqcoeff_ref + n_coeffs;
        TranLow *dqcoeff = qcoeff + n_coeffs;
        uint16_t *eob = (uint16_t *)(dqcoeff + n_coeffs);

        // Testing uses 2-D DCT scan order table
        const ScanOrder *const sc = get_scan_order(tx_size_, DCT_DCT);

        // Testing uses luminance quantization table
        const int16_t *zbin = qtab_->quant.y_zbin[q];

        const int16_t *round = 0;
        const int16_t *quant = 0;
        if (type_ == TYPE_B) {
            round = qtab_->quant.y_round[q];
            quant = qtab_->quant.y_quant[q];
        } else if (type_ == TYPE_FP) {
            round = qtab_->quant.y_round_fp[q];
            quant = qtab_->quant.y_quant_fp[q];
        }

        const int16_t *quant_shift = qtab_->quant.y_quant_shift[q];
        const int16_t *dequant = qtab_->dequant.y_dequant_qtx[q];

        for (int i = 0; i < test_num; ++i) {
            if (is_loop)
                FillCoeffRandom();

            memset(qcoeff_ref, 0, 5 * n_coeffs * sizeof(*qcoeff_ref));

            quant_ref_(coeff_ptr,
                       n_coeffs,
                       zbin,
                       round,
                       quant,
                       quant_shift,
                       qcoeff_ref,
                       dqcoeff_ref,
                       dequant,
                       &eob[0],
                       sc->scan,
                       sc->iscan);

            quant_(coeff_ptr,
                   n_coeffs,
                   zbin,
                   round,
                   quant,
                   quant_shift,
                   qcoeff,
                   dqcoeff,
                   dequant,
                   &eob[1],
                   sc->scan,
                   sc->iscan);

            for (int j = 0; j < n_coeffs; ++j) {
                ASSERT_EQ(qcoeff_ref[j], qcoeff[j])
                    << "Q mismatch on test: " << i << " at position: " << j
                    << " Q: " << q << " coeff: " << coeff_ptr[j];
            }

            for (int j = 0; j < n_coeffs; ++j) {
                ASSERT_EQ(dqcoeff_ref[j], dqcoeff[j])
                    << "Dq mismatch on test: " << i << " at position: " << j
                    << " Q: " << q << " coeff: " << coeff_ptr[j];
            }

            ASSERT_EQ(eob[0], eob[1])
                << "eobs mismatch on test: " << i << " Q: " << q;
        }
    }
};

TEST_P(QuantizeLbdTest, ZeroInput) {
    FillCoeffZero();
    QuantizeRun(false);
}

TEST_P(QuantizeLbdTest, LargeNegativeInput) {
    FillDcLargeNegative();
    QuantizeRun(false, 0, 1);
}

TEST_P(QuantizeLbdTest, DcOnlyInput) {
    FillDcOnly();
    QuantizeRun(false, 0, 1);
}

TEST_P(QuantizeLbdTest, RandomInput) {
    QuantizeRun(true, 0, kTestNum);
}

TEST_P(QuantizeLbdTest, MultipleQ) {
    for (int q = 0; q < QINDEX_RANGE; ++q) {
        QuantizeRun(true, q, kTestNum);
    }
}

// Force the coeff to be half the value of the dequant.  This exposes a
// mismatch found in av1_quantize_fp_sse2().
TEST_P(QuantizeLbdTest, CoeffHalfDequant) {
    FillCoeff(16);
    QuantizeRun(false, 25, 1);
}

TEST_P(QuantizeLbdTest, DISABLED_Speed) {
    TranLow *coeff_ptr = coeff_;
    const intptr_t n_coeffs = coeff_num();

    TranLow *qcoeff_ref = coeff_ptr + n_coeffs;
    TranLow *dqcoeff_ref = qcoeff_ref + n_coeffs;

    TranLow *qcoeff = dqcoeff_ref + n_coeffs;
    TranLow *dqcoeff = qcoeff + n_coeffs;
    uint16_t *eob = (uint16_t *)(dqcoeff + n_coeffs);

    // Testing uses 2-D DCT scan order table
    const ScanOrder *const sc = get_scan_order(tx_size_, DCT_DCT);

    // Testing uses luminance quantization table
    const int q = 22;
    const int16_t *zbin = qtab_->quant.y_zbin[q];
    const int16_t *round_fp = qtab_->quant.y_round_fp[q];
    const int16_t *quant_fp = qtab_->quant.y_quant_fp[q];
    const int16_t *quant_shift = qtab_->quant.y_quant_shift[q];
    const int16_t *dequant = qtab_->dequant.y_dequant_qtx[q];
    const int kNumTests = 5000000;
    double time_c, time_o;
    uint64_t start_time_seconds, start_time_useconds;
    uint64_t middle_time_seconds, middle_time_useconds;
    uint64_t finish_time_seconds, finish_time_useconds;
    int rows = tx_size_high[tx_size_];
    int cols = tx_size_wide[tx_size_];
    rows = AOMMIN(32, rows);
    cols = AOMMIN(32, cols);
    for (int cnt = 0; cnt <= rows; cnt++) {
        FillCoeffRandomRows(cnt * cols);

        svt_av1_get_time(&start_time_seconds, &start_time_useconds);
        for (int n = 0; n < kNumTests; ++n) {
            quant_ref_(coeff_ptr,
                       n_coeffs,
                       zbin,
                       round_fp,
                       quant_fp,
                       quant_shift,
                       qcoeff,
                       dqcoeff,
                       dequant,
                       eob,
                       sc->scan,
                       sc->iscan);
        }

        svt_av1_get_time(&middle_time_seconds, &middle_time_useconds);

        for (int n = 0; n < kNumTests; ++n) {
            quant_(coeff_ptr,
                   n_coeffs,
                   zbin,
                   round_fp,
                   quant_fp,
                   quant_shift,
                   qcoeff,
                   dqcoeff,
                   dequant,
                   eob,
                   sc->scan,
                   sc->iscan);
        }
        svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);
        time_c = svt_av1_compute_overall_elapsed_time_ms(start_time_seconds,
                                                         start_time_useconds,
                                                         middle_time_seconds,
                                                         middle_time_useconds);
        time_o = svt_av1_compute_overall_elapsed_time_ms(middle_time_seconds,
                                                         middle_time_useconds,
                                                         finish_time_seconds,
                                                         finish_time_useconds);

        printf("c_time = %f \t simd_time = %f \t Gain = %f \n",
               time_c,
               time_o,
               (time_c / time_o));
    }
}
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
class QuantizeHbdTest : public QuantizeTest<QuantizeHbdParam, QuantizeHbdFunc> {
  protected:
    QuantizeHbdTest() {
        quant_ref_ = TEST_GET_PARAM(0);
        quant_ = TEST_GET_PARAM(1);
        tx_size_ = TEST_GET_PARAM(2);
        type_ = TEST_GET_PARAM(3);
        bd_ = TEST_GET_PARAM(4);
    }

    void QuantizeRun(bool is_loop, int q = 0, int test_num = 1) override {
        TranLow *coeff_ptr = coeff_;
        const intptr_t n_coeffs = coeff_num();

        TranLow *qcoeff_ref = coeff_ptr + n_coeffs;
        TranLow *dqcoeff_ref = qcoeff_ref + n_coeffs;

        TranLow *qcoeff = dqcoeff_ref + n_coeffs;
        TranLow *dqcoeff = qcoeff + n_coeffs;
        uint16_t *eob = (uint16_t *)(dqcoeff + n_coeffs);

        // Testing uses 2-D DCT scan order table
        const ScanOrder *const sc = get_scan_order(tx_size_, DCT_DCT);

        // Testing uses luminance quantization table
        const int16_t *zbin = qtab_->quant.y_zbin[q];

        const int16_t *round = 0;
        const int16_t *quant = 0;
        if (type_ == TYPE_B) {
            round = qtab_->quant.y_round[q];
            quant = qtab_->quant.y_quant[q];
        } else if (type_ == TYPE_FP) {
            round = qtab_->quant.y_round_fp[q];
            quant = qtab_->quant.y_quant_fp[q];
        }

        const int16_t *quant_shift = qtab_->quant.y_quant_shift[q];
        const int16_t *dequant = qtab_->dequant.y_dequant_qtx[q];

        for (int i = 0; i < test_num; ++i) {
            if (is_loop)
                FillCoeffRandom();

            memset(qcoeff_ref, 0, 5 * n_coeffs * sizeof(*qcoeff_ref));

            quant_ref_(coeff_ptr,
                       n_coeffs,
                       zbin,
                       round,
                       quant,
                       quant_shift,
                       qcoeff_ref,
                       dqcoeff_ref,
                       dequant,
                       &eob[0],
                       sc->scan,
                       sc->iscan,
                       av1_get_tx_scale(tx_size_));

            quant_(coeff_ptr,
                   n_coeffs,
                   zbin,
                   round,
                   quant,
                   quant_shift,
                   qcoeff,
                   dqcoeff,
                   dequant,
                   &eob[1],
                   sc->scan,
                   sc->iscan,
                   av1_get_tx_scale(tx_size_));

            for (int j = 0; j < n_coeffs; ++j) {
                ASSERT_EQ(qcoeff_ref[j], qcoeff[j])
                    << "Q mismatch on test: " << i << " at position: " << j
                    << " Q: " << q << " coeff: " << coeff_ptr[j];
            }

            for (int j = 0; j < n_coeffs; ++j) {
                ASSERT_EQ(dqcoeff_ref[j], dqcoeff[j])
                    << "Dq mismatch on test: " << i << " at position: " << j
                    << " Q: " << q << " coeff: " << coeff_ptr[j];
            }

            ASSERT_EQ(eob[0], eob[1])
                << "eobs mismatch on test: " << i << " Q: " << q;
        }
    }
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(QuantizeHbdTest);

TEST_P(QuantizeHbdTest, ZeroInput) {
    FillCoeffZero();
    QuantizeRun(false);
}

TEST_P(QuantizeHbdTest, LargeNegativeInput) {
    FillDcLargeNegative();
    QuantizeRun(false, 0, 1);
}

TEST_P(QuantizeHbdTest, DcOnlyInput) {
    FillDcOnly();
    QuantizeRun(false, 0, 1);
}

TEST_P(QuantizeHbdTest, RandomInput) {
    QuantizeRun(true, 0, kTestNum);
}

TEST_P(QuantizeHbdTest, MultipleQ) {
    for (int q = 0; q < QINDEX_RANGE; ++q) {
        QuantizeRun(true, q, kTestNum);
    }
}

// Force the coeff to be half the value of the dequant.  This exposes a
// mismatch found in av1_quantize_fp_sse2().
TEST_P(QuantizeHbdTest, CoeffHalfDequant) {
    FillCoeff(16);
    QuantizeRun(false, 25, 1);
}

TEST_P(QuantizeHbdTest, DISABLED_Speed) {
    TranLow *coeff_ptr = coeff_;
    const intptr_t n_coeffs = coeff_num();

    TranLow *qcoeff_ref = coeff_ptr + n_coeffs;
    TranLow *dqcoeff_ref = qcoeff_ref + n_coeffs;

    TranLow *qcoeff = dqcoeff_ref + n_coeffs;
    TranLow *dqcoeff = qcoeff + n_coeffs;
    uint16_t *eob = (uint16_t *)(dqcoeff + n_coeffs);

    // Testing uses 2-D DCT scan order table
    const ScanOrder *const sc = get_scan_order(tx_size_, DCT_DCT);

    // Testing uses luminance quantization table
    const int q = 22;
    const int16_t *zbin = qtab_->quant.y_zbin[q];
    const int16_t *round_fp = qtab_->quant.y_round_fp[q];
    const int16_t *quant_fp = qtab_->quant.y_quant_fp[q];
    const int16_t *quant_shift = qtab_->quant.y_quant_shift[q];
    const int16_t *dequant = qtab_->dequant.y_dequant_qtx[q];
    const int kNumTests = 5000000;
    double time_c, time_o;
    uint64_t start_time_seconds, start_time_useconds;
    uint64_t middle_time_seconds, middle_time_useconds;
    uint64_t finish_time_seconds, finish_time_useconds;
    int rows = tx_size_high[tx_size_];
    int cols = tx_size_wide[tx_size_];
    rows = AOMMIN(32, rows);
    cols = AOMMIN(32, cols);
    for (int cnt = 0; cnt <= rows; cnt++) {
        FillCoeffRandomRows(cnt * cols);

        svt_av1_get_time(&start_time_seconds, &start_time_useconds);
        for (int n = 0; n < kNumTests; ++n) {
            quant_ref_(coeff_ptr,
                       n_coeffs,
                       zbin,
                       round_fp,
                       quant_fp,
                       quant_shift,
                       qcoeff,
                       dqcoeff,
                       dequant,
                       eob,
                       sc->scan,
                       sc->iscan,
                       av1_get_tx_scale(tx_size_));
        }

        svt_av1_get_time(&middle_time_seconds, &middle_time_useconds);

        for (int n = 0; n < kNumTests; ++n) {
            quant_(coeff_ptr,
                   n_coeffs,
                   zbin,
                   round_fp,
                   quant_fp,
                   quant_shift,
                   qcoeff,
                   dqcoeff,
                   dequant,
                   eob,
                   sc->scan,
                   sc->iscan,
                   av1_get_tx_scale(tx_size_));
        }
        svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);
        time_c = svt_av1_compute_overall_elapsed_time_ms(start_time_seconds,
                                                         start_time_useconds,
                                                         middle_time_seconds,
                                                         middle_time_useconds);
        time_o = svt_av1_compute_overall_elapsed_time_ms(middle_time_seconds,
                                                         middle_time_useconds,
                                                         finish_time_seconds,
                                                         finish_time_useconds);

        printf("c_time = %f \t simd_time = %f \t Gain = %f \n",
               time_c,
               time_o,
               (time_c / time_o));
    }
}

#endif  // CONFIG_ENABLE_HIGH_BIT_DEPTH

class QuantizeQmTest : public QuantizeTest<QuantizeQmParam, QuantizeQmFunc> {
  protected:
    QuantizeQmTest() {
        quant_ref_ = TEST_GET_PARAM(0);
        quant_ = TEST_GET_PARAM(1);
        tx_size_ = TEST_GET_PARAM(2);
        type_ = TEST_GET_PARAM(3);
        bd_ = TEST_GET_PARAM(4);

        qm_level_ = 8;
        init_qm();
    }

    void QuantizeRun(bool is_loop, int q = 0, int test_num = 1) override {
        TranLow *coeff_ptr = coeff_;
        const intptr_t n_coeffs = coeff_num();

        TranLow *qcoeff_ref = coeff_ptr + n_coeffs;
        TranLow *dqcoeff_ref = qcoeff_ref + n_coeffs;

        TranLow *qcoeff = dqcoeff_ref + n_coeffs;
        TranLow *dqcoeff = qcoeff + n_coeffs;
        uint16_t *eob = (uint16_t *)(dqcoeff + n_coeffs);

        // Testing uses 2-D DCT scan order table
        const ScanOrder *const sc = get_scan_order(tx_size_, DCT_DCT);

        // Testing uses luminance quantization table
        const int16_t *zbin = qtab_->quant.y_zbin[q];

        // ASSERT_EQ(type_ == TYPE_FP);
        const int16_t *round = qtab_->quant.y_round_fp[q];
        const int16_t *quant = qtab_->quant.y_quant_fp[q];

        const int16_t *quant_shift = qtab_->quant.y_quant_shift[q];
        const int16_t *dequant = qtab_->dequant.y_dequant_qtx[q];

        const TxSize qm_tx_size = av1_get_adjusted_tx_size(tx_size_);
        const QmVal *qm_ptr = qmatrix_[qm_level_][0][qm_tx_size];
        const QmVal *iqm_ptr = iqmatrix_[qm_level_][0][qm_tx_size];

        for (int i = 0; i < test_num; ++i) {
            if (is_loop)
                FillCoeffRandom();

            memset(qcoeff_ref, 0, 5 * n_coeffs * sizeof(*qcoeff_ref));
            int log_scale = av1_get_tx_scale(tx_size_);

            quant_ref_(coeff_ptr,
                       n_coeffs,
                       zbin,
                       round,
                       quant,
                       quant_shift,
                       qcoeff_ref,
                       dqcoeff_ref,
                       dequant,
                       &eob[0],
                       sc->scan,
                       sc->iscan,
                       qm_ptr,
                       iqm_ptr,
                       log_scale);

            quant_(coeff_ptr,
                   n_coeffs,
                   zbin,
                   round,
                   quant,
                   quant_shift,
                   qcoeff,
                   dqcoeff,
                   dequant,
                   &eob[1],
                   sc->scan,
                   sc->iscan,
                   qm_ptr,
                   iqm_ptr,
                   log_scale);

            for (int j = 0; j < n_coeffs; ++j) {
                ASSERT_EQ(qcoeff_ref[j], qcoeff[j])
                    << "Q mismatch on test: " << i << " at position: " << j
                    << " Q: " << q << " coeff: " << coeff_ptr[j];
            }

            for (int j = 0; j < n_coeffs; ++j) {
                ASSERT_EQ(dqcoeff_ref[j], dqcoeff[j])
                    << "Dq mismatch on test: " << i << " at position: " << j
                    << " Q: " << q << " coeff: " << coeff_ptr[j];
            }

            ASSERT_EQ(eob[0], eob[1])
                << "eobs mismatch on test: " << i << " Q: " << q;
        }
    }

  private:
    TxSize av1_get_adjusted_tx_size(TxSize tx_size) {
        switch (tx_size) {
        case TX_64X64:
        case TX_64X32:
        case TX_32X64: return TX_32X32;
        case TX_64X16: return TX_32X16;
        case TX_16X64: return TX_16X32;
        default: return tx_size;
        }
    }

    void init_qm() {
        const uint8_t num_planes = 1;
        uint8_t q, c, t;
#if CONFIG_ENABLE_QUANT_MATRIX
        int32_t current;
        for (q = 0; q < NUM_QM_LEVELS; ++q) {
            for (c = 0; c < num_planes; ++c) {
                current = 0;
                for (t = 0; t < TX_SIZES_ALL; ++t) {
                    const int32_t size = tx_size_2d[t];
                    const TxSize qm_tx_size =
                        av1_get_adjusted_tx_size(static_cast<TxSize>(t));
                    if (q == NUM_QM_LEVELS - 1) {
                        qmatrix_[q][c][t] = NULL;
                        iqmatrix_[q][c][t] = NULL;
                    } else if (t !=
                               qm_tx_size) {  // Reuse matrices for 'qm_tx_size'
                        qmatrix_[q][c][t] = qmatrix_[q][c][qm_tx_size];
                        iqmatrix_[q][c][t] = iqmatrix_[q][c][qm_tx_size];
                    } else {
                        assert(current + size <= QM_TOTAL_SIZE);
                        // The following lines are suppressed since it could
                        // break if we set num_planes to 3 cppcheck-suppress
                        // knownConditionTrueFalse
                        qmatrix_[q][c][t] = &wt_matrix_ref[q][c >= 1][current];
                        // cppcheck-suppress knownConditionTrueFalse
                        iqmatrix_[q][c][t] =
                            &iwt_matrix_ref[q][c >= 1][current];

                        current += size;
                    }
                }
            }
        }
#else
        for (q = 0; q < NUM_QM_LEVELS; ++q) {
            for (c = 0; c < num_planes; ++c) {
                for (t = 0; t < TX_SIZES_ALL; ++t) {
                    qmatrix_[q][c][t] = NULL;
                    iqmatrix_[q][c][t] = NULL;
                }
            }
        }
#endif  // CONFIG_ENABLE_QUANT_MATRIX
    }

    const QmVal *iqmatrix_[NUM_QM_LEVELS][3][TX_SIZES_ALL];
    const QmVal *qmatrix_[NUM_QM_LEVELS][3][TX_SIZES_ALL];
    int qm_level_;
};

using std::make_tuple;

#ifdef ARCH_X86_64
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(QuantizeQmTest);

TEST_P(QuantizeQmTest, ZeroInput) {
    FillCoeffZero();
    QuantizeRun(false);
}

TEST_P(QuantizeQmTest, LargeNegativeInput) {
    FillDcLargeNegative();
    QuantizeRun(false, 0, 1);
}

TEST_P(QuantizeQmTest, DcOnlyInput) {
    FillDcOnly();
    QuantizeRun(false, 0, 1);
}

TEST_P(QuantizeQmTest, RandomInput) {
    QuantizeRun(true, 0, kTestNum);
}

TEST_P(QuantizeQmTest, MultipleQ) {
    for (int q = 0; q < QINDEX_RANGE; ++q) {
        QuantizeRun(true, q, kTestNum);
    }
}

// Force the coeff to be half the value of the dequant.  This exposes a
// mismatch found in av1_quantize_fp_sse2().
TEST_P(QuantizeQmTest, CoeffHalfDequant) {
    FillCoeff(16);
    QuantizeRun(false, 25, 1);
}

using QuantizeQmHbdTest = QuantizeQmTest;
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(QuantizeQmHbdTest);

TEST_P(QuantizeQmHbdTest, ZeroInput) {
    FillCoeffZero();
    QuantizeRun(false);
}

TEST_P(QuantizeQmHbdTest, LargeNegativeInput) {
    FillDcLargeNegative();
    QuantizeRun(false, 0, 1);
}

TEST_P(QuantizeQmHbdTest, DcOnlyInput) {
    FillDcOnly();
    QuantizeRun(false, 0, 1);
}

TEST_P(QuantizeQmHbdTest, RandomInput) {
    QuantizeRun(true, 0, kTestNum);
}

TEST_P(QuantizeQmHbdTest, MultipleQ) {
    for (int q = 0; q < QINDEX_RANGE; ++q) {
        QuantizeRun(true, q, kTestNum);
    }
}

// Force the coeff to be half the value of the dequant.  This exposes a
// mismatch found in av1_quantize_fp_sse2().
TEST_P(QuantizeQmHbdTest, CoeffHalfDequant) {
    FillCoeff(16);
    QuantizeRun(false, 25, 1);
}

const QuantizeParam kQParamArrayAvx2[] = {
    make_tuple(&svt_av1_quantize_fp_c, &svt_av1_quantize_fp_sse4_1,
               static_cast<TxSize>(TX_16X16), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_c, &svt_av1_quantize_fp_sse4_1,
               static_cast<TxSize>(TX_4X16), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_c, &svt_av1_quantize_fp_sse4_1,
               static_cast<TxSize>(TX_16X4), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_c, &svt_av1_quantize_fp_sse4_1,
               static_cast<TxSize>(TX_32X8), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_c, &svt_av1_quantize_fp_sse4_1,
               static_cast<TxSize>(TX_8X32), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_c, &svt_av1_quantize_fp_avx2,
               static_cast<TxSize>(TX_16X16), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_c, &svt_av1_quantize_fp_avx2,
               static_cast<TxSize>(TX_4X16), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_c, &svt_av1_quantize_fp_avx2,
               static_cast<TxSize>(TX_16X4), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_c, &svt_av1_quantize_fp_avx2,
               static_cast<TxSize>(TX_32X8), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_c, &svt_av1_quantize_fp_avx2,
               static_cast<TxSize>(TX_8X32), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_32x32_c, &svt_av1_quantize_fp_32x32_avx2,
               static_cast<TxSize>(TX_32X32), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_32x32_c, &svt_av1_quantize_fp_32x32_avx2,
               static_cast<TxSize>(TX_16X64), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_32x32_c, &svt_av1_quantize_fp_32x32_avx2,
               static_cast<TxSize>(TX_64X16), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_32x32_c, &svt_av1_quantize_fp_32x32_sse4_1,
               static_cast<TxSize>(TX_32X32), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_32x32_c, &svt_av1_quantize_fp_32x32_sse4_1,
               static_cast<TxSize>(TX_16X64), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_32x32_c, &svt_av1_quantize_fp_32x32_sse4_1,
               static_cast<TxSize>(TX_64X16), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_64x64_c, &svt_av1_quantize_fp_64x64_avx2,
               static_cast<TxSize>(TX_64X64), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_64x64_c, &svt_av1_quantize_fp_64x64_sse4_1,
               static_cast<TxSize>(TX_64X64), TYPE_FP, EB_EIGHT_BIT)};

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
const QuantizeHbdParam kQHbdParamArraySse41[] = {
    make_tuple(&svt_av1_highbd_quantize_fp_c,
               &svt_av1_highbd_quantize_fp_sse4_1,
               static_cast<TxSize>(TX_16X16), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c,
               &svt_av1_highbd_quantize_fp_sse4_1, static_cast<TxSize>(TX_4X16),
               TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c,
               &svt_av1_highbd_quantize_fp_sse4_1, static_cast<TxSize>(TX_16X4),
               TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c,
               &svt_av1_highbd_quantize_fp_sse4_1, static_cast<TxSize>(TX_32X8),
               TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c,
               &svt_av1_highbd_quantize_fp_sse4_1, static_cast<TxSize>(TX_8X32),
               TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c,
               &svt_av1_highbd_quantize_fp_sse4_1,
               static_cast<TxSize>(TX_64X64), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c,
               &svt_av1_highbd_quantize_fp_sse4_1,
               static_cast<TxSize>(TX_16X16), TYPE_FP, EB_TEN_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c,
               &svt_av1_highbd_quantize_fp_sse4_1, static_cast<TxSize>(TX_4X16),
               TYPE_FP, EB_TEN_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c,
               &svt_av1_highbd_quantize_fp_sse4_1, static_cast<TxSize>(TX_16X4),
               TYPE_FP, EB_TEN_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c,
               &svt_av1_highbd_quantize_fp_sse4_1, static_cast<TxSize>(TX_32X8),
               TYPE_FP, EB_TEN_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c,
               &svt_av1_highbd_quantize_fp_sse4_1, static_cast<TxSize>(TX_8X32),
               TYPE_FP, EB_TEN_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c,
               &svt_av1_highbd_quantize_fp_sse4_1,
               static_cast<TxSize>(TX_64X64), TYPE_FP, EB_TEN_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c,
               &svt_av1_highbd_quantize_fp_sse4_1,
               static_cast<TxSize>(TX_16X16), TYPE_FP, EB_TWELVE_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c,
               &svt_av1_highbd_quantize_fp_sse4_1, static_cast<TxSize>(TX_4X16),
               TYPE_FP, EB_TWELVE_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c,
               &svt_av1_highbd_quantize_fp_sse4_1, static_cast<TxSize>(TX_16X4),
               TYPE_FP, EB_TWELVE_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c,
               &svt_av1_highbd_quantize_fp_sse4_1, static_cast<TxSize>(TX_32X8),
               TYPE_FP, EB_TWELVE_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c,
               &svt_av1_highbd_quantize_fp_sse4_1, static_cast<TxSize>(TX_8X32),
               TYPE_FP, EB_TWELVE_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c,
               &svt_av1_highbd_quantize_fp_sse4_1,
               static_cast<TxSize>(TX_64X64), TYPE_FP, EB_TWELVE_BIT)};

const QuantizeHbdParam kQHbdParamArrayAvx2[] = {
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_avx2,
               static_cast<TxSize>(TX_16X16), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_avx2,
               static_cast<TxSize>(TX_4X16), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_avx2,
               static_cast<TxSize>(TX_16X4), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_avx2,
               static_cast<TxSize>(TX_32X8), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_avx2,
               static_cast<TxSize>(TX_8X32), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_avx2,
               static_cast<TxSize>(TX_64X64), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_avx2,
               static_cast<TxSize>(TX_16X16), TYPE_FP, EB_TEN_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_avx2,
               static_cast<TxSize>(TX_4X16), TYPE_FP, EB_TEN_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_avx2,
               static_cast<TxSize>(TX_16X4), TYPE_FP, EB_TEN_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_avx2,
               static_cast<TxSize>(TX_32X8), TYPE_FP, EB_TEN_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_avx2,
               static_cast<TxSize>(TX_8X32), TYPE_FP, EB_TEN_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_avx2,
               static_cast<TxSize>(TX_64X64), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_avx2,
               static_cast<TxSize>(TX_16X16), TYPE_FP, EB_TWELVE_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_avx2,
               static_cast<TxSize>(TX_4X16), TYPE_FP, EB_TWELVE_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_avx2,
               static_cast<TxSize>(TX_16X4), TYPE_FP, EB_TWELVE_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_avx2,
               static_cast<TxSize>(TX_32X8), TYPE_FP, EB_TWELVE_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_avx2,
               static_cast<TxSize>(TX_8X32), TYPE_FP, EB_TWELVE_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_avx2,
               static_cast<TxSize>(TX_64X64), TYPE_FP, EB_TWELVE_BIT)};
#endif  // CONFIG_ENABLE_HIGH_BIT_DEPTH

const QuantizeQmParam kQmParamArrayAvx2[] = {
    make_tuple(&svt_av1_quantize_fp_qm_c, &svt_av1_quantize_fp_qm_avx2,
               static_cast<TxSize>(TX_16X16), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_qm_c, &svt_av1_quantize_fp_qm_avx2,
               static_cast<TxSize>(TX_4X16), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_qm_c, &svt_av1_quantize_fp_qm_avx2,
               static_cast<TxSize>(TX_16X4), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_qm_c, &svt_av1_quantize_fp_qm_avx2,
               static_cast<TxSize>(TX_32X8), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_qm_c, &svt_av1_quantize_fp_qm_avx2,
               static_cast<TxSize>(TX_8X32), TYPE_FP, EB_EIGHT_BIT)};

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
const QuantizeQmParam kQmParamHbdArrayAvx2[] = {
    make_tuple(&svt_av1_highbd_quantize_fp_qm_c,
               &svt_av1_highbd_quantize_fp_qm_avx2,
               static_cast<TxSize>(TX_16X16), TYPE_FP, EB_TEN_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_qm_c,
               &svt_av1_highbd_quantize_fp_qm_avx2,
               static_cast<TxSize>(TX_4X16), TYPE_FP, EB_TEN_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_qm_c,
               &svt_av1_highbd_quantize_fp_qm_avx2,
               static_cast<TxSize>(TX_16X4), TYPE_FP, EB_TEN_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_qm_c,
               &svt_av1_highbd_quantize_fp_qm_avx2,
               static_cast<TxSize>(TX_32X8), TYPE_FP, EB_TEN_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_qm_c,
               &svt_av1_highbd_quantize_fp_qm_avx2,
               static_cast<TxSize>(TX_8X32), TYPE_FP, EB_TEN_BIT)};
#endif  // CONFIG_ENABLE_HIGH_BIT_DEPTH

INSTANTIATE_TEST_SUITE_P(AVX2, QuantizeLbdTest,
                         ::testing::ValuesIn(kQParamArrayAvx2));
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
INSTANTIATE_TEST_SUITE_P(SSE4_1, QuantizeHbdTest,
                         ::testing::ValuesIn(kQHbdParamArraySse41));
INSTANTIATE_TEST_SUITE_P(AVX2, QuantizeHbdTest,
                         ::testing::ValuesIn(kQHbdParamArrayAvx2));
#endif  // CONFIG_ENABLE_HIGH_BIT_DEPTH
INSTANTIATE_TEST_SUITE_P(AVX2, QuantizeQmTest,
                         ::testing::ValuesIn(kQmParamArrayAvx2));
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
INSTANTIATE_TEST_SUITE_P(AVX2, QuantizeQmHbdTest,
                         ::testing::ValuesIn(kQmParamHbdArrayAvx2));
#endif  // CONFIG_ENABLE_HIGH_BIT_DEPTH
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
const QuantizeParam kQParamArrayNeon[] = {
    make_tuple(&svt_av1_quantize_fp_c, &svt_av1_quantize_fp_neon,
               static_cast<TxSize>(TX_16X16), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_c, &svt_av1_quantize_fp_neon,
               static_cast<TxSize>(TX_4X16), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_c, &svt_av1_quantize_fp_neon,
               static_cast<TxSize>(TX_16X4), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_c, &svt_av1_quantize_fp_neon,
               static_cast<TxSize>(TX_32X8), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_c, &svt_av1_quantize_fp_neon,
               static_cast<TxSize>(TX_8X32), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_32x32_c, &svt_av1_quantize_fp_32x32_neon,
               static_cast<TxSize>(TX_32X32), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_32x32_c, &svt_av1_quantize_fp_32x32_neon,
               static_cast<TxSize>(TX_16X64), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_32x32_c, &svt_av1_quantize_fp_32x32_neon,
               static_cast<TxSize>(TX_64X16), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_quantize_fp_64x64_c, &svt_av1_quantize_fp_64x64_neon,
               static_cast<TxSize>(TX_64X64), TYPE_FP, EB_EIGHT_BIT)};

INSTANTIATE_TEST_SUITE_P(NEON, QuantizeLbdTest,
                         ::testing::ValuesIn(kQParamArrayNeon));

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
const QuantizeHbdParam kQHbdParamArrayNeon[] = {
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_neon,
               static_cast<TxSize>(TX_16X16), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_neon,
               static_cast<TxSize>(TX_4X16), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_neon,
               static_cast<TxSize>(TX_16X4), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_neon,
               static_cast<TxSize>(TX_32X8), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_neon,
               static_cast<TxSize>(TX_8X32), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_neon,
               static_cast<TxSize>(TX_64X64), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_neon,
               static_cast<TxSize>(TX_16X16), TYPE_FP, EB_TEN_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_neon,
               static_cast<TxSize>(TX_4X16), TYPE_FP, EB_TEN_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_neon,
               static_cast<TxSize>(TX_16X4), TYPE_FP, EB_TEN_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_neon,
               static_cast<TxSize>(TX_32X8), TYPE_FP, EB_TEN_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_neon,
               static_cast<TxSize>(TX_8X32), TYPE_FP, EB_TEN_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_neon,
               static_cast<TxSize>(TX_64X64), TYPE_FP, EB_EIGHT_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_neon,
               static_cast<TxSize>(TX_16X16), TYPE_FP, EB_TWELVE_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_neon,
               static_cast<TxSize>(TX_4X16), TYPE_FP, EB_TWELVE_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_neon,
               static_cast<TxSize>(TX_16X4), TYPE_FP, EB_TWELVE_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_neon,
               static_cast<TxSize>(TX_32X8), TYPE_FP, EB_TWELVE_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_neon,
               static_cast<TxSize>(TX_8X32), TYPE_FP, EB_TWELVE_BIT),
    make_tuple(&svt_av1_highbd_quantize_fp_c, &svt_av1_highbd_quantize_fp_neon,
               static_cast<TxSize>(TX_64X64), TYPE_FP, EB_TWELVE_BIT)};

INSTANTIATE_TEST_SUITE_P(NEON, QuantizeHbdTest,
                         ::testing::ValuesIn(kQHbdParamArrayNeon));
#endif  // CONFIG_ENABLE_HIGH_BIT_DEPTH
#endif  // ARCH_AARCH64

using ComputeCulLevelFunc = uint8_t (*)(const int16_t *const scan,
                                        const int32_t *const quant_coeff,
                                        uint16_t *eob);

class ComputeCulLevelTest
    : public ::testing::TestWithParam<ComputeCulLevelFunc> {
  public:
    ComputeCulLevelTest() : test_func_(GetParam()) {
    }

    void test_match() {
        SVTRandom rnd(0, (1 << 10) - 1);
        SVTRandom quant_rnd(-10, 10);
        const int max_size = 50;
        // scan[] is a set of indexes for quant_coeff[]
        int16_t scan[max_size];
        int32_t quant_coeff[max_size];
        uint16_t eob_ref, eob_test;

        scan[0] = 0;

        for (int test = 0; test < 1000; test++) {
            eob_ref = eob_test = rnd.random() % max_size;

            for (uint32_t i = 0; i < max_size; i++) {
                quant_coeff[i] = eob_ref == 0 ? 0 : quant_rnd.random();
                if (i != 0)
                    scan[i] = rnd.random() % max_size;
            }

            int32_t ref_res =
                svt_av1_compute_cul_level_c(scan, quant_coeff, &eob_ref);

            int32_t test_res = test_func_(scan, quant_coeff, &eob_test);

            EXPECT_EQ(ref_res, test_res);
            EXPECT_EQ(eob_ref, eob_test);
        }
    }

  private:
    ComputeCulLevelFunc test_func_;
};

TEST_P(ComputeCulLevelTest, test_match) {
    test_match();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(AVX2, ComputeCulLevelTest,
                         ::testing::Values(svt_av1_compute_cul_level_avx2));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(NEON, ComputeCulLevelTest,
                         ::testing::Values(svt_av1_compute_cul_level_neon));

#if HAVE_SVE
INSTANTIATE_TEST_SUITE_P(SVE, ComputeCulLevelTest,
                         ::testing::Values(svt_av1_compute_cul_level_sve));
#endif  // HAVE_SVE
#endif  // ARCH_AARCH64
}  // namespace
