/*
 * Copyright(c) 2019 Netflix, Inc.
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
 * @file QuantAsmTest.c
 *
 * @brief Unit test for quantize avx2 functions:
 * - svt_aom_highbd_quantize_b_avx2
 * - svt_aom_quantize_b_avx2
 *
 * @author Cidana-Zhengwen
 *
 ******************************************************************************/

#include <random>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include "gtest/gtest.h"

#include "definitions.h"
#include "md_config_process.h"
#include "transforms.h"
#include "pcs.h"
#include "aom_dsp_rtcd.h"
#include "util.h"
#include "random.h"
#include "q_matrices.h"

namespace QuantizeAsmTest {

using QuantizeFunc = void (*)(const TranLow *coeff_ptr, intptr_t n_coeffs,
                              const int16_t *zbin_ptr, const int16_t *round_ptr,
                              const int16_t *quant_ptr,
                              const int16_t *quant_shift_ptr,
                              TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr,
                              const int16_t *dequant_ptr, uint16_t *eob_ptr,
                              const int16_t *scan, const int16_t *iscan,
                              const QmVal *qm_ptr, const QmVal *iqm_ptr,
                              const int32_t log_scale);

using QuantizeParam = std::tuple<int, int, QuantizeFunc>;

using svt_av1_test_tool::SVTRandom;  // to generate the random
/**
 * @brief Unit test for quantize avx2 functions:
 * - svt_aom_highbd_quantize_b_avx2
 * - svt_aom_quantize_b_avx2
 *
 * Test strategy:
 * These tests use quantize C function as reference, input the same data and
 * compare C function output with avx2 function output, so as to check
 * quant/dequant/eob output consistancy of quantize avx2 implementation.
 *
 * Expect result:
 * avx2 output quant/dequant/eob should be exactly same as C output.
 *
 * Test coverage:
 * All combinations of all tx_size with 8bit/10bit.
 *
 * Test cases:
 * - AVX2/QuantizeBTest.input_zero_all
 * - AVX2/QuantizeBTest.input_dcac_minmax_q_n
 * - AVX2/QuantizeBTest.input_random_dc_only
 * - AVX2/QuantizeBTest.input_random_all_q_all
 */
class QuantizeBTest : public ::testing::TestWithParam<QuantizeParam> {
  protected:
    QuantizeBTest()
        : tx_size_(static_cast<TxSize>(TEST_GET_PARAM(0))),
          bd_(static_cast<EbBitDepth>(TEST_GET_PARAM(1))),
          log_scale(0) {
        n_coeffs_ = av1_get_max_eob(tx_size_);
        coeff_min_ = -(1 << (7 + bd_)) + 1;
        coeff_max_ = (1 << (7 + bd_)) - 1;
        rnd_ = new SVTRandom(coeff_min_, coeff_max_);
        PictureParentControlSet pcs;
        pcs.scs = new SequenceControlSet;
        pcs.frm_hdr.quantization_params.base_q_idx = 0;
        pcs.scs->static_config.sharpness = 0;
        PictureParentControlSet *pcs_ptr = &pcs;

        svt_av1_build_quantizer(
            pcs_ptr, bd_, 0, 0, 0, 0, 0, &qtab_quants_, &qtab_deq_);
        setup_func_ptrs();
        delete pcs.scs;
    }

    virtual ~QuantizeBTest() {
        delete rnd_;
    }

    void SetUp() override {
        coeff_in_ = reinterpret_cast<TranLow *>(
            svt_aom_memalign(32, MAX_TX_SQUARE * sizeof(TranLow)));
        qcoeff_ref_ = reinterpret_cast<TranLow *>(
            svt_aom_memalign(32, MAX_TX_SQUARE * sizeof(TranLow)));
        dqcoeff_ref_ = reinterpret_cast<TranLow *>(
            svt_aom_memalign(32, MAX_TX_SQUARE * sizeof(TranLow)));
        qcoeff_test_ = reinterpret_cast<TranLow *>(
            svt_aom_memalign(32, MAX_TX_SQUARE * sizeof(TranLow)));
        dqcoeff_test_ = reinterpret_cast<TranLow *>(
            svt_aom_memalign(32, MAX_TX_SQUARE * sizeof(TranLow)));
    }

    void TearDown() override {
        svt_aom_free(coeff_in_);
        svt_aom_free(qcoeff_ref_);
        svt_aom_free(dqcoeff_ref_);
        svt_aom_free(qcoeff_test_);
        svt_aom_free(dqcoeff_test_);
    }

    /*
     * @brief setup reference and target function ptrs
     * @see svt_aom_setup_rtcd_internal() in aom_dsp_rtcd.h
     */
    void setup_func_ptrs() {
        if (bd_ == EB_EIGHT_BIT) {
            quant_ref_ = svt_aom_quantize_b_c;
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
        } else {
            quant_ref_ = svt_aom_highbd_quantize_b_c;
#endif  // CONFIG_ENABLE_HIGH_BIT_DEPTH
        }
        quant_test_ = TEST_GET_PARAM(2);
        if (tx_size_ == TX_32X32) {
            log_scale = 1;
        } else if (tx_size_ == TX_64X64) {
            log_scale = 2;
        } else {
            log_scale = 0;
        }
    }

    /*
     * @brief run quantize with the same input data, then check whether output
     * quant/dequant/eob are exactly same between C output and avx2 outptu
     */
    virtual void run_quantize(int q) {
        const ScanOrder *const sc = get_scan_order(tx_size_, DCT_DCT);
        const int16_t *zbin = qtab_quants_.y_zbin[q];
        const int16_t *round = qtab_quants_.y_round[q];
        const int16_t *quant = qtab_quants_.y_quant[q];
        const int16_t *quant_shift = qtab_quants_.y_quant_shift[q];
        const int16_t *dequant = qtab_deq_.y_dequant_qtx[q];

        memset(qcoeff_ref_, 0, MAX_TX_SQUARE * sizeof(TranLow));
        memset(dqcoeff_ref_, 0, MAX_TX_SQUARE * sizeof(TranLow));
        memset(qcoeff_test_, 0, MAX_TX_SQUARE * sizeof(TranLow));
        memset(dqcoeff_test_, 0, MAX_TX_SQUARE * sizeof(TranLow));

        quant_ref_(coeff_in_,
                   n_coeffs_,
                   zbin,
                   round,
                   quant,
                   quant_shift,
                   qcoeff_ref_,
                   dqcoeff_ref_,
                   dequant,
                   &eob_ref_,
                   sc->scan,
                   sc->iscan,
                   NULL,
                   NULL,
                   log_scale);

        quant_test_(coeff_in_,
                    n_coeffs_,
                    zbin,
                    round,
                    quant,
                    quant_shift,
                    qcoeff_test_,
                    dqcoeff_test_,
                    dequant,
                    &eob_test_,
                    sc->scan,
                    sc->iscan,
                    NULL,
                    NULL,
                    log_scale);

        for (int j = 0; j < n_coeffs_; ++j) {
            ASSERT_EQ(qcoeff_ref_[j], qcoeff_test_[j])
                << "Q mismatch at position: " << j << ", Q: " << q
                << " coeff: " << coeff_in_[j];
        }

        for (int j = 0; j < n_coeffs_; ++j) {
            ASSERT_EQ(dqcoeff_ref_[j], dqcoeff_test_[j])
                << "Dq mismatch at position: " << j << ", Q: " << q
                << " coeff: " << coeff_in_[j];
        }

        ASSERT_EQ(eob_ref_, eob_test_) << "eobs mismatch, Q: " << q;
    }

    void fill_coeff_const(int i_begin, int i_end, TranLow c) {
        for (int i = i_begin; i < i_end; ++i) {
            coeff_in_[i] = c;
        }
    }

    void fill_coeff_random(int i_begin, int i_end) {
        for (int i = i_begin; i < i_end; ++i) {
            coeff_in_[i] = rnd_->random();
        }
    }

    SVTRandom *rnd_;          /**< random int for 8bit and 10bit coeffs */
    Quants qtab_quants_;      /**< quant table */
    Dequants qtab_deq_;       /**< dequant table */
    TranLow coeff_min_;       /**< min input coeff value */
    TranLow coeff_max_;       /**< max input coeff value */
    QuantizeFunc quant_ref_;  /**< reference quantize function */
    QuantizeFunc quant_test_; /**< test target quantize function */
    const TxSize tx_size_;    /**< input param tx_size */
    const EbBitDepth bd_;     /**< input param 8bit or 10bit */
    int n_coeffs_;            /**< coeff number */
    int32_t log_scale;
    uint16_t eob_ref_;  /**< output ref eob */
    uint16_t eob_test_; /**< output test eob */

    TranLow *coeff_in_;
    TranLow *qcoeff_ref_;
    TranLow *dqcoeff_ref_;
    TranLow *qcoeff_test_;
    TranLow *dqcoeff_test_;
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(QuantizeBTest);

/**
 * @brief AVX2/QuantizeBTest.input_zero_all
 *
 * test output data consistency of quantize C and avx2 functions with
 * input coef: all 0
 * q_index: 0
 */
TEST_P(QuantizeBTest, input_zero_all) {
    fill_coeff_const(0, n_coeffs_, 0);
    run_quantize(0);
}

/**
 * @brief AVX2/QuantizeBTest.input_dcac_minmax_q_n
 *
 * test output data consistency of quantize C and avx2 functions with
 * input coef: combine dc min/max with 1st ac min/max, other ac all 0
 * q_index: 0 ~ QINDEX_RANGE, step 25
 */
TEST_P(QuantizeBTest, input_dcac_minmax_q_n) {
    fill_coeff_const(2, n_coeffs_, 0);
    for (int q = 0; q < QINDEX_RANGE; q += 25) {
        fill_coeff_const(0, 1, coeff_min_);
        fill_coeff_const(1, 2, coeff_min_);
        run_quantize(q);
        fill_coeff_const(0, 1, coeff_min_);
        fill_coeff_const(1, 2, coeff_max_);
        run_quantize(q);
        fill_coeff_const(0, 1, coeff_max_);
        fill_coeff_const(1, 2, coeff_min_);
        run_quantize(q);
        fill_coeff_const(0, 1, coeff_max_);
        fill_coeff_const(1, 2, coeff_max_);
        run_quantize(q);
    }
}

/**
 * @brief AVX2/QuantizeBTest.input_random_dc_only
 *
 * test output data consistency of quantize C and avx2 functions with
 * input coef: dc random and ac all 0
 * q_index: 0
 */
TEST_P(QuantizeBTest, input_random_dc_only) {
    fill_coeff_const(1, n_coeffs_, 0);
    fill_coeff_random(0, 1);
    run_quantize(0);
}

/**
 * @brief AVX2/QuantizeBTest.input_random_all_q_all
 *
 * loop test output data consistency of quantize C and avx2 functions with
 * input coef: dc random and ac all random
 * q_index: from 0 to QINDEX_RANGE-1
 */
TEST_P(QuantizeBTest, input_random_all_q_all) {
    const int num_tests = 10;
    for (int q = 0; q < QINDEX_RANGE; ++q) {
        for (int i = 0; i < num_tests; ++i) {
            fill_coeff_random(0, n_coeffs_);
            run_quantize(q);
        }
    }
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    LBD_SSE4_1, QuantizeBTest,
    ::testing::Combine(::testing::Values(static_cast<int>(TX_16X16),
                                         static_cast<int>(TX_32X32),
                                         static_cast<int>(TX_64X64)),
                       ::testing::Values(static_cast<int>(EB_EIGHT_BIT)),
                       ::testing::Values(svt_aom_quantize_b_sse4_1)));

INSTANTIATE_TEST_SUITE_P(
    LBD_AVX2, QuantizeBTest,
    ::testing::Combine(::testing::Values(static_cast<int>(TX_16X16),
                                         static_cast<int>(TX_32X32),
                                         static_cast<int>(TX_64X64)),
                       ::testing::Values(static_cast<int>(EB_EIGHT_BIT)),
                       ::testing::Values(svt_aom_quantize_b_avx2)));
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
INSTANTIATE_TEST_SUITE_P(
    HBD_SSE4_1, QuantizeBTest,
    ::testing::Combine(::testing::Values(static_cast<int>(TX_16X16),
                                         static_cast<int>(TX_32X32),
                                         static_cast<int>(TX_64X64)),
                       ::testing::Values(static_cast<int>(EB_TEN_BIT)),
                       ::testing::Values(svt_aom_highbd_quantize_b_sse4_1)));

INSTANTIATE_TEST_SUITE_P(
    HBD_AVX2, QuantizeBTest,
    ::testing::Combine(::testing::Values(static_cast<int>(TX_16X16),
                                         static_cast<int>(TX_32X32),
                                         static_cast<int>(TX_64X64)),
                       ::testing::Values(static_cast<int>(EB_TEN_BIT)),
                       ::testing::Values(svt_aom_highbd_quantize_b_avx2)));
#endif  // CONFIG_ENABLE_HIGH_BIT_DEPTH
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    LBD_NEON, QuantizeBTest,
    ::testing::Combine(::testing::Values(static_cast<int>(TX_16X16),
                                         static_cast<int>(TX_32X32),
                                         static_cast<int>(TX_64X64)),
                       ::testing::Values(static_cast<int>(EB_EIGHT_BIT)),
                       ::testing::Values(svt_aom_quantize_b_neon)));
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
INSTANTIATE_TEST_SUITE_P(
    HBD_NEON, QuantizeBTest,
    ::testing::Combine(::testing::Values(static_cast<int>(TX_16X16),
                                         static_cast<int>(TX_32X32),
                                         static_cast<int>(TX_64X64)),
                       ::testing::Values(static_cast<int>(EB_TEN_BIT)),
                       ::testing::Values(svt_aom_highbd_quantize_b_neon)));
#endif  // CONFIG_ENABLE_HIGH_BIT_DEPTH
#endif  // ARCH_AARCH64

#if CONFIG_ENABLE_QUANT_MATRIX
class QuantizeBQmTest : public QuantizeBTest {
  protected:
    QuantizeBQmTest() : QuantizeBTest() {
        qm_level_ = 8;
        init_qm();
    }

    virtual ~QuantizeBQmTest() = default;

    void run_quantize(int q) override {
        const ScanOrder *const sc = get_scan_order(tx_size_, DCT_DCT);
        const int16_t *zbin = qtab_quants_.y_zbin[q];
        const int16_t *round = qtab_quants_.y_round[q];
        const int16_t *quant = qtab_quants_.y_quant[q];
        const int16_t *quant_shift = qtab_quants_.y_quant_shift[q];
        const int16_t *dequant = qtab_deq_.y_dequant_qtx[q];

        const TxSize qm_tx_size = av1_get_adjusted_tx_size(tx_size_);
        const QmVal *qm_ptr = qmatrix_[qm_level_][0][qm_tx_size];
        const QmVal *iqm_ptr = iqmatrix_[qm_level_][0][qm_tx_size];

        memset(qcoeff_ref_, 0, MAX_TX_SQUARE * sizeof(TranLow));
        memset(dqcoeff_ref_, 0, MAX_TX_SQUARE * sizeof(TranLow));
        memset(qcoeff_test_, 0, MAX_TX_SQUARE * sizeof(TranLow));
        memset(dqcoeff_test_, 0, MAX_TX_SQUARE * sizeof(TranLow));

        quant_ref_(coeff_in_,
                   n_coeffs_,
                   zbin,
                   round,
                   quant,
                   quant_shift,
                   qcoeff_ref_,
                   dqcoeff_ref_,
                   dequant,
                   &eob_ref_,
                   sc->scan,
                   sc->iscan,
                   qm_ptr,
                   iqm_ptr,
                   log_scale);

        quant_test_(coeff_in_,
                    n_coeffs_,
                    zbin,
                    round,
                    quant,
                    quant_shift,
                    qcoeff_test_,
                    dqcoeff_test_,
                    dequant,
                    &eob_test_,
                    sc->scan,
                    sc->iscan,
                    qm_ptr,
                    iqm_ptr,
                    log_scale);

        for (int j = 0; j < n_coeffs_; ++j) {
            ASSERT_EQ(qcoeff_ref_[j], qcoeff_test_[j])
                << "Q mismatch at position: " << j << ", Q: " << q
                << " coeff: " << coeff_in_[j];
        }

        for (int j = 0; j < n_coeffs_; ++j) {
            ASSERT_EQ(dqcoeff_ref_[j], dqcoeff_test_[j])
                << "Dq mismatch at position: " << j << ", Q: " << q
                << " coeff: " << coeff_in_[j];
        }

        ASSERT_EQ(eob_ref_, eob_test_) << "eobs mismatch, Q: " << q;
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
                        qmatrix_[q][c][t] = &wt_matrix_ref[q][c >= 1][current];
                        iqmatrix_[q][c][t] =
                            &iwt_matrix_ref[q][c >= 1][current];
                        current += size;
                    }
                }
            }
        }
    }

    const QmVal *iqmatrix_[NUM_QM_LEVELS][3][TX_SIZES_ALL];
    const QmVal *qmatrix_[NUM_QM_LEVELS][3][TX_SIZES_ALL];
    int qm_level_;
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(QuantizeBQmTest);

/**
 * @brief AVX2/QuantizeBTest.input_zero_all
 *
 * test output data consistency of quantize C and avx2 functions with
 * input coef: all 0
 * q_index: 0
 */
TEST_P(QuantizeBQmTest, input_zero_all) {
    fill_coeff_const(0, n_coeffs_, 0);
    run_quantize(0);
}

/**
 * @brief AVX2/QuantizeBTest.input_dcac_minmax_q_n
 *
 * test output data consistency of quantize C and avx2 functions with
 * input coef: combine dc min/max with 1st ac min/max, other ac all 0
 * q_index: 0 ~ QINDEX_RANGE, step 25
 */
TEST_P(QuantizeBQmTest, input_dcac_minmax_q_n) {
    fill_coeff_const(2, n_coeffs_, 0);
    for (int q = 0; q < QINDEX_RANGE; q += 25) {
        fill_coeff_const(0, 1, coeff_min_);
        fill_coeff_const(1, 2, coeff_min_);
        run_quantize(q);
        fill_coeff_const(0, 1, coeff_min_);
        fill_coeff_const(1, 2, coeff_max_);
        run_quantize(q);
        fill_coeff_const(0, 1, coeff_max_);
        fill_coeff_const(1, 2, coeff_min_);
        run_quantize(q);
        fill_coeff_const(0, 1, coeff_max_);
        fill_coeff_const(1, 2, coeff_max_);
        run_quantize(q);
    }
}

/**
 * @brief AVX2/QuantizeBTest.input_random_dc_only
 *
 * test output data consistency of quantize C and avx2 functions with
 * input coef: dc random and ac all 0
 * q_index: 0
 */
TEST_P(QuantizeBQmTest, input_random_dc_only) {
    fill_coeff_const(1, n_coeffs_, 0);
    fill_coeff_random(0, 1);
    run_quantize(0);
}

/**
 * @brief AVX2/QuantizeBTest.input_random_all_q_all
 *
 * loop test output data consistency of quantize C and avx2 functions with
 * input coef: dc random and ac all random
 * q_index: from 0 to QINDEX_RANGE-1
 */
TEST_P(QuantizeBQmTest, input_random_all_q_all) {
    const int num_tests = 10;
    for (int q = 0; q < QINDEX_RANGE; ++q) {
        for (int i = 0; i < num_tests; ++i) {
            fill_coeff_random(0, n_coeffs_);
            run_quantize(q);
        }
    }
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    LBD_AVX2, QuantizeBQmTest,
    ::testing::Combine(::testing::Values(static_cast<int>(TX_16X16),
                                         static_cast<int>(TX_32X32),
                                         static_cast<int>(TX_64X64)),
                       ::testing::Values(static_cast<int>(EB_EIGHT_BIT)),
                       ::testing::Values(svt_av1_quantize_b_qm_avx2)));

INSTANTIATE_TEST_SUITE_P(
    HBD_AVX2, QuantizeBQmTest,
    ::testing::Combine(::testing::Values(static_cast<int>(TX_16X16),
                                         static_cast<int>(TX_32X32),
                                         static_cast<int>(TX_64X64)),
                       ::testing::Values(static_cast<int>(EB_TEN_BIT)),
                       ::testing::Values(svt_av1_highbd_quantize_b_qm_avx2)));
#endif  // ARCH_X86_64

#endif  // CONFIG_ENABLE_QUANT_MATRIX

}  // namespace QuantizeAsmTest
