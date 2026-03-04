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
 * @file AdaptiveScanTest.cc
 *
 * @brief Unit test for adaptive scan:
 *
 * @author Cidana-Wenyao
 *
 ******************************************************************************/
#include <stdlib.h>

#include "gtest/gtest.h"
#include "definitions.h"
#include "transforms.h"
#include "TxfmCommon.h"
#include "cabac_context_model.h"  // use tx_type_to_class
#include "util.h"
#include "random.h"

static bool valid_scan(const int16_t *scan, const int16_t *iscan, int si,
                       int expected_pos) {
    if (scan[si] != expected_pos || iscan[expected_pos] != si)
        return false;
    else
        return true;
}

/**
 * @brief Unit test for scan tables:
 *
 * Test strategy:
 * Iterate the positions according to the scan type and verify the pos from
 * scan table and si from iscan table.
 *
 * Expect result:
 * Position from the scan table should be identical with the pos calculated
 * from x and y.
 *
 * Test coverage:
 * Test cases:
 * All valid scan, including zig-zag scan, vertical scan, horizontal scan
 * and rectangle scan
 *
 */
TEST(AdaptiveScanTest, scan_tables_test) {
    for (int tx_size = TX_4X4; tx_size < TX_SIZES_ALL; ++tx_size) {
        const int txb_height = get_txb_high((TxSize)tx_size);
        const int txb_width = get_txb_wide((TxSize)tx_size);
        const int bwl = get_txb_bwl((TxSize)tx_size);

        for (int tx_type = DCT_DCT; tx_type < TX_TYPES; ++tx_type) {
            if (!is_txfm_allowed((TxType)tx_type, (TxSize)tx_size))
                continue;

            TxClass tx_class = tx_type_to_class[tx_type];
            const ScanOrder *scan_order = get_scan_order(tx_size, tx_type);
            const int16_t *scan = scan_order->scan;
            const int16_t *iscan = scan_order->iscan;
            const int sum_xy = txb_width + txb_height - 1;
            int si = 0;
            if (tx_class == TX_CLASS_VERT) {
                // vertical transform will scan the rows horizontally,
                // so si is the same with pos
                // process horizontal scan first
                for (int y = 0; y < txb_height; ++y) {
                    for (int x = 0; x < txb_width; ++x) {
                        EXPECT_TRUE(valid_scan(scan, iscan, si, x + (y << bwl)))
                            << "tx_size " << tx_size << " tx_type " << tx_type
                            << " scan table test fail";
                        ++si;
                    }
                }
            } else if (tx_class == TX_CLASS_HORIZ) {
                // horizontal transform will scan vertically.
                for (int x = 0; x < txb_width; ++x) {
                    for (int y = 0; y < txb_height; ++y) {
                        EXPECT_TRUE(valid_scan(scan, iscan, si, x + (y << bwl)))
                            << "tx_size " << tx_size << " tx_type " << tx_type
                            << " scan table test fail";
                        ++si;
                    }
                }
            } else if (txb_width < txb_height) {
                // cols are smaller than rows
                for (int i = 0; i < sum_xy; ++i) {
                    for (int y = 0; y < txb_height; ++y) {
                        int x = i - y;
                        if (x >= 0 && x < txb_width) {
                            EXPECT_TRUE(
                                valid_scan(scan, iscan, si, x + (y << bwl)))
                                << "tx_size " << tx_size << " tx_type "
                                << tx_type << " scan table test fail";
                            ++si;
                        }
                    }
                }
            } else if (txb_width > txb_height) {
                // col diag
                for (int i = 0; i < sum_xy; ++i) {
                    for (int x = 0; x < txb_width; ++x) {
                        int y = i - x;
                        if (y >= 0 && y < txb_height) {
                            EXPECT_TRUE(
                                valid_scan(scan, iscan, si, x + (y << bwl)))
                                << "tx_size " << tx_size << " tx_type "
                                << tx_type << " scan table test fail";
                            ++si;
                        }
                    }
                }
            } else {
                // zig-zag scan
                int swap = 1;
                for (int i = 0; i < sum_xy; ++i) {
                    swap ^= 1;
                    for (int j = 0; j < txb_width; ++j) {
                        const int x = swap ? i - j : j;
                        const int y = swap ? j : i - j;
                        if (i - j >= 0 && i - j < txb_width) {
                            EXPECT_TRUE(
                                valid_scan(scan, iscan, si, x + (y << bwl)))
                                << "tx_size " << tx_size << " tx_type "
                                << tx_type << " scan table test fail";
                            ++si;
                        }
                    }
                }
            }
        }
    }
}

using svt_av1_test_tool::SVTRandom;
using CopyMiMapGridFunc = void (*)(MbModeInfo **mi_grid_ptr, uint32_t mi_stride,
                                   uint8_t num_rows, uint8_t num_cols);

using CopyMiMapGridParam = std::tuple<int, CopyMiMapGridFunc>;

class CopyMiMapGridTest : public ::testing::TestWithParam<CopyMiMapGridParam> {
  public:
    CopyMiMapGridTest()
        : tx_size_(TEST_GET_PARAM(0)), test_func_(TEST_GET_PARAM(1)) {
    }

    void test_match() {
        SVTRandom rnd(0, (1 << 10) - 1);
        const int max_size = 100;
        MbModeInfo *mi_grid_ref[max_size * max_size];
        MbModeInfo *mi_grid_tst[max_size * max_size];

        const int txb_height = get_txb_high((TxSize)tx_size_);
        const int txb_width = get_txb_wide((TxSize)tx_size_);
        uint32_t stride = txb_width + rnd.random() % 10;

        memset(mi_grid_ref, 0xcd, sizeof(mi_grid_ref));
        memset(mi_grid_tst, 0xcd, sizeof(mi_grid_ref));

        mi_grid_ref[0] = mi_grid_tst[0] =
            (MbModeInfo *)((uint64_t)rnd.random());

        svt_copy_mi_map_grid_c(mi_grid_ref, stride, txb_height, txb_width);

        test_func_(mi_grid_tst, stride, txb_height, txb_width);

        EXPECT_TRUE(memcmp(mi_grid_ref, mi_grid_tst, sizeof(mi_grid_ref)) == 0);

        // special case non power of 2 cols
        for (int cols = 0; cols < 15; ++cols) {
            stride = cols + rnd.random() % 10;

            memset(mi_grid_ref, 0xcd, sizeof(mi_grid_ref));
            memset(mi_grid_tst, 0xcd, sizeof(mi_grid_ref));

            mi_grid_ref[0] = mi_grid_tst[0] =
                (MbModeInfo *)((uint64_t)rnd.random());

            svt_copy_mi_map_grid_c(mi_grid_ref, stride, cols, cols);

            test_func_(mi_grid_tst, stride, cols, cols);

            EXPECT_TRUE(memcmp(mi_grid_ref, mi_grid_tst, sizeof(mi_grid_ref)) ==
                        0);
        }
    }

  private:
    int tx_size_;
    CopyMiMapGridFunc test_func_;
};

TEST_P(CopyMiMapGridTest, test_match) {
    test_match();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    AVX2, CopyMiMapGridTest,
    ::testing::Combine(::testing::Range(static_cast<int>(TX_4X4),
                                        static_cast<int>(TX_SIZES_ALL)),
                       ::testing::Values(svt_copy_mi_map_grid_avx2)));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, CopyMiMapGridTest,
    ::testing::Combine(::testing::Range(static_cast<int>(TX_4X4),
                                        static_cast<int>(TX_SIZES_ALL)),
                       ::testing::Values(svt_copy_mi_map_grid_neon)));
#endif  // ARCH_AARCH64
