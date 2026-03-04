/*
 * Copyright(c) 2019 Intel Corporation
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
#include "definitions.h"
#include "unit_test.h"
#include "unit_test_utility.h"
#include "highbd_intra_prediction_tests.h"
#include <immintrin.h>
#include "random.h"
#include "util.h"

#if EN_AVX512_SUPPORT
using namespace std;
int bitdepth[] = {8, 10, 12};

static void init_data(uint16_t **input, uint16_t **above, uint16_t **left,
                      ptrdiff_t input_stride) {
    TEST_ALLIGN_MALLOC(
        uint16_t *, *input, sizeof(uint16_t) * MAX_SB_SIZE * input_stride);
    memset(*input, 0, MAX_SB_SIZE * input_stride);
    svt_buf_random_u16(*input, (uint32_t)(MAX_SB_SIZE * input_stride));
    *above = *input + ((rand() % (MAX_SB_SIZE * input_stride / 2)) >>
                       4 << 4);  // align to 16
    *left = *input + ((rand() % (MAX_SB_SIZE * input_stride / 4)) >>
                      4 << 4);  // align to 16
}

static void uninit_data(uint16_t *input) {
    TEST_ALLIGN_FREE(input);
}

static void init_coeff(uint16_t **coeff, uint16_t **coeff_opt,
                       ptrdiff_t *stride) {
    *stride = svt_create_random_aligned_stride(MAX_SB_SIZE, 64);
    TEST_ALLIGN_MALLOC(
        uint16_t *, *coeff, sizeof(uint16_t) * MAX_SB_SIZE * *stride);
    TEST_ALLIGN_MALLOC(
        uint16_t *, *coeff_opt, sizeof(uint16_t) * MAX_SB_SIZE * *stride);
    memset(*coeff, 0, sizeof(uint16_t) * MAX_SB_SIZE * *stride);
    memset(*coeff_opt, 0, sizeof(uint16_t) * MAX_SB_SIZE * *stride);
}

static void uninit_coeff(uint16_t *coeff, uint16_t *coeff_opt) {
    TEST_ALLIGN_FREE(coeff);
    TEST_ALLIGN_FREE(coeff_opt);
}

void dc_compare_u16(uint16_t *output_base, uint16_t *output_opt,
                    ptrdiff_t stride, int height, int width) {
    for (int x = 0; x < height; x++) {
        for (int y = 0; y < width; y++) {
            EXPECT_EQ(output_base[y], output_opt[y]);
        }
        output_base += stride;
        output_opt += stride;
    }
}

TEST(AVX512_HighbdIntraPredictionTest, aom_dc_top_predictor_kernels) {
    uint16_t *input = NULL, *above = NULL, *left = NULL;
    uint16_t *dc_coeff = NULL, *dc_coeff_opt = NULL;
    ptrdiff_t stride = 0;

    int no_of_pred_funcs = sizeof(aom_highbd_dc_top_funcptr_array_opt) /
                           sizeof(aom_highbd_dc_top_predictor_func);
    int no_of_bitdepths = sizeof(bitdepth) / sizeof(int);

    for (int loop = 0; loop < no_of_pred_funcs; loop++) {  // Function Pairs
        for (int i = 0; i < EB_UNIT_TEST_NUM; i++) {     // Number of Test Runs
            for (int x = 0; x < no_of_bitdepths; x++) {  // Bit Depth
                switch (loop) {
                case 0:  // 32x8
                    for (int j = 0; j < 10; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_dc_top_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_dc_top_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 8, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 1:  // 32x16
                    for (int j = 0; j < 10; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_dc_top_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_dc_top_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 16, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 2:  // 32x32
                    for (int j = 0; j < 10; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_dc_top_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_dc_top_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 32, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 3:  // 32x64
                    for (int j = 0; j < 10; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_dc_top_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_dc_top_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 64, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 4:  // 64x16
                    for (int j = 0; j < 10; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_dc_top_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_dc_top_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 16, 64);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 5:  // 64x32
                    for (int j = 0; j < 10; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_dc_top_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_dc_top_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 32, 64);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 6:  // 64x64
                    for (int j = 0; j < 10; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_dc_top_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_dc_top_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 64, 64);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                default: GTEST_FAIL();
                }
            }
        }
    }
}

TEST(AVX512_HighbdIntraPredictionTest, aom_dc_left_predictor_kernels) {
    uint16_t *input = NULL, *above = NULL, *left = NULL;
    uint16_t *dc_coeff = NULL, *dc_coeff_opt = NULL;
    ptrdiff_t stride = 0;

    int no_of_pred_funcs = sizeof(aom_highbd_dc_left_funcptr_array_opt) /
                           sizeof(aom_highbd_dc_left_predictor_func);

    for (int loop = 0; loop < no_of_pred_funcs; loop++) {  // Function Pairs
        for (int i = 0; i < EB_UNIT_TEST_NUM; i++) {  // Number of Test Runs
            for (int x = 0; x < 3; x++) {             // Bit Depth
                switch (loop) {
                case 0:  // 32x8
                    for (int j = 0; j < 1; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_dc_left_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_dc_left_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 8, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 1:  // 32x16
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_dc_left_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_dc_left_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 16, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 2:  // 32x32
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_dc_left_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_dc_left_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 32, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 3:  // 32x64
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_dc_left_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_dc_left_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 64, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 4:  // 64x16
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_dc_left_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_dc_left_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 16, 64);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 5:  // 64x32
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_dc_left_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_dc_left_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 32, 64);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 6:  // 64x64
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_dc_left_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_dc_left_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 64, 64);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                default: GTEST_FAIL();
                }
            }
        }
    }
}

TEST(AVX512_HighbdIntraPredictionTest, aom_dc_predictor_kernels) {
    uint16_t *input = NULL, *above = NULL, *left = NULL;
    uint16_t *dc_coeff = NULL, *dc_coeff_opt = NULL;
    ptrdiff_t stride = 0;

    int no_of_pred_funcs = sizeof(aom_highbd_dc_pred_funcptr_array_opt) /
                           sizeof(aom_highbd_dc_predictor_func);

    for (int loop = 0; loop < no_of_pred_funcs; loop++) {  // Function Pairs
        for (int i = 0; i < EB_UNIT_TEST_NUM; i++) {  // Number of Test Runs
            for (int x = 0; x < 3; x++) {             // Bit Depth
                switch (loop) {
                case 0:  // 32x8
                    for (int j = 0; j < 1; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_dc_pred_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_dc_pred_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 8, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 1:  // 32x16
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_dc_pred_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_dc_pred_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 16, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 2:  // 32x32
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_dc_pred_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_dc_pred_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 32, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 3:  // 32x64
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_dc_pred_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_dc_pred_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 64, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 4:  // 64x16
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_dc_pred_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_dc_pred_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 16, 64);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 5:  // 64x32
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_dc_pred_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_dc_pred_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 32, 64);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 6:  // 64x64
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_dc_pred_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_dc_pred_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 64, 64);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                default: GTEST_FAIL();
                }
            }
        }
    }
}

TEST(AVX512_HighbdIntraPredictionTest, aom_h_predictor_kernels) {
    uint16_t *input = NULL, *above = NULL, *left = NULL;
    uint16_t *dc_coeff = NULL, *dc_coeff_opt = NULL;
    ptrdiff_t stride = 0;

    uint16_t no_of_pred_funcs = sizeof(aom_highbd_h_pred_funcptr_array_opt) /
                                sizeof(aom_highbd_h_predictor_func);

    for (uint16_t loop = 0; loop < no_of_pred_funcs; loop++) {  // Function
                                                                // Pairs
        for (uint16_t i = 0; i < EB_UNIT_TEST_NUM; i++) {  // Number of Test
                                                           // Runs
            for (uint16_t x = 0; x < 3; x++) {             // Bit Depth
                switch (loop) {
                case 0:  // 32x8
                    for (uint16_t j = 0; j < 1; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_h_pred_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_h_pred_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 8, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 1:  // 32x16
                    for (uint16_t j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_h_pred_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_h_pred_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 16, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 2:  // 32x32
                    for (uint16_t j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_h_pred_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_h_pred_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 32, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 3:  // 32x64
                    for (uint16_t j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_h_pred_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_h_pred_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 64, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 4:  // 64x16
                    for (uint16_t j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_h_pred_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_h_pred_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 16, 64);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 5:  // 64x32
                    for (uint16_t j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_h_pred_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_h_pred_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 32, 64);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 6:  // 64x64
                    for (uint16_t j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_h_pred_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_h_pred_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 64, 64);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                default: GTEST_FAIL();
                }
            }
        }
    }
}

TEST(AVX512_HighbdIntraPredictionTest, aom_v_predictor_kernels) {
    uint16_t *input = NULL, *above = NULL, *left = NULL;
    uint16_t *dc_coeff = NULL, *dc_coeff_opt = NULL;
    ptrdiff_t stride = 0;

    uint16_t no_of_pred_funcs = sizeof(aom_highbd_v_pred_funcptr_array_opt) /
                                sizeof(aom_highbd_v_predictor_func);

    for (uint16_t loop = 0; loop < no_of_pred_funcs; loop++) {  // Function
                                                                // Pairs
        for (uint16_t i = 0; i < EB_UNIT_TEST_NUM; i++) {  // Number of Test
                                                           // Runs
            for (uint16_t x = 0; x < 3; x++) {             // Bit Depth
                switch (loop) {
                case 0:  // 32x8
                    for (uint16_t j = 0; j < 1; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_v_pred_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_v_pred_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 8, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 1:  // 32x16
                    for (uint16_t j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_v_pred_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_v_pred_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 16, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 2:  // 32x32
                    for (uint16_t j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_v_pred_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_v_pred_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 32, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 3:  // 32x64
                    for (uint16_t j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_v_pred_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_v_pred_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 64, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 4:  // 64x16
                    for (uint16_t j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_v_pred_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_v_pred_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 16, 64);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 5:  // 64x32
                    for (uint16_t j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_v_pred_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_v_pred_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 32, 64);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 6:  // 64x64
                    for (uint16_t j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_v_pred_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_v_pred_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 64, 64);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                default: GTEST_FAIL();
                }
            }
        }
    }
}

TEST(AVX512_HighbdIntraPredictionTest, aom_highbd_smooth_predictor_kernels) {
    uint16_t *input = NULL, *above = NULL, *left = NULL;
    uint16_t *dc_coeff = NULL, *dc_coeff_opt = NULL;
    ptrdiff_t stride = 0;

    int no_of_pred_funcs = sizeof(aom_smooth_predictor_funcptr_array_opt) /
                           sizeof(aom_highbd_smooth_predictor_func);
    int no_of_bitdepths = sizeof(bitdepth) / sizeof(int);

    for (int loop = 0; loop < no_of_pred_funcs; loop++) {  // Function Pairs
        for (int i = 0; i < 1; i++) {                    // Number of Test Runs
            for (int x = 0; x < no_of_bitdepths; x++) {  // Bit Depth
                switch (loop) {
                case 0:  // 32x8
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_smooth_predictor_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_smooth_predictor_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 8, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 1:  // 32x16
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_smooth_predictor_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_smooth_predictor_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 16, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 2:  // 32x32
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_smooth_predictor_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_smooth_predictor_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 32, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 3:  // 32x64
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_smooth_predictor_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_smooth_predictor_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 64, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 4:  // 64x16
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_smooth_predictor_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_smooth_predictor_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 16, 64);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 5:  // 64x32
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_smooth_predictor_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_smooth_predictor_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 32, 64);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 6:  // 64x64
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_smooth_predictor_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_smooth_predictor_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 64, 64);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                default: GTEST_FAIL();
                }
            }
        }
    }
}

TEST(AVX512_HighbdIntraPredictionTest, aom_highbd_smooth_v_predictor_kernels) {
    uint16_t *input = NULL, *above = NULL, *left = NULL;
    uint16_t *dc_coeff = NULL, *dc_coeff_opt = NULL;
    ptrdiff_t stride = 0;

    int no_of_pred_funcs =
        sizeof(aom_highbd_smooth_v_predictor_funcptr_array_opt) /
        sizeof(aom_highbd_smooth_v_predictor_func);
    int no_of_bitdepths = sizeof(bitdepth) / sizeof(int);

    for (int loop = 0; loop < no_of_pred_funcs; loop++) {  // Function Pairs
        for (int i = 0; i < 1; i++) {                    // Number of Test Runs
            for (int x = 0; x < no_of_bitdepths; x++) {  // Bit Depth
                switch (loop) {
                case 0:  // 32x8
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_smooth_v_predictor_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_smooth_v_predictor_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 8, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 1:  // 32x16
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_smooth_v_predictor_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_smooth_v_predictor_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 16, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 2:  // 32x32
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_smooth_v_predictor_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_smooth_v_predictor_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 32, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 3:  // 32x64
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_smooth_v_predictor_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_smooth_v_predictor_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 64, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 4:  // 64x16
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_smooth_v_predictor_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_smooth_v_predictor_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 16, 64);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 5:  // 64x32
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_smooth_v_predictor_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_smooth_v_predictor_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 32, 64);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 6:  // 64x64
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_smooth_v_predictor_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_smooth_v_predictor_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 64, 64);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                default: GTEST_FAIL();
                }
            }
        }
    }
}

TEST(AVX512_HighbdIntraPredictionTest, aom_highbd_smooth_h_predictor_kernels) {
    uint16_t *input = NULL, *above = NULL, *left = NULL;
    uint16_t *dc_coeff = NULL, *dc_coeff_opt = NULL;
    ptrdiff_t stride = 0;

    int no_of_pred_funcs =
        sizeof(aom_highbd_smooth_h_predictor_funcptr_array_opt) /
        sizeof(aom_highbd_smooth_h_predictor_func);
    int no_of_bitdepths = sizeof(bitdepth) / sizeof(int);

    for (int loop = 0; loop < no_of_pred_funcs; loop++) {  // Function Pairs
        for (int i = 0; i < 1; i++) {                    // Number of Test Runs
            for (int x = 0; x < no_of_bitdepths; x++) {  // Bit Depth
                switch (loop) {
                case 0:  // 32x8
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_smooth_h_predictor_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_smooth_h_predictor_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 8, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 1:  // 32x16
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_smooth_h_predictor_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_smooth_h_predictor_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 16, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 2:  // 32x32
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_smooth_h_predictor_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_smooth_h_predictor_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 32, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 3:  // 32x64
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_smooth_h_predictor_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_smooth_h_predictor_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 64, 32);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 4:  // 64x16
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_smooth_h_predictor_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_smooth_h_predictor_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 16, 64);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 5:  // 64x32
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_smooth_h_predictor_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_smooth_h_predictor_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 32, 64);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                case 6:  // 64x64
                    for (int j = 0; j < 2; j++) {
                        init_coeff(&dc_coeff, &dc_coeff_opt, &stride);
                        GTEST_ASSERT_TRUE(svt_buf_compare_u16(
                            dc_coeff, dc_coeff_opt, MAX_SB_SIZE * stride));
                        init_data(&input, &above, &left, stride);
                        aom_highbd_smooth_h_predictor_funcptr_array_base[loop](
                            dc_coeff, stride, above, left, bitdepth[x]);
                        aom_highbd_smooth_h_predictor_funcptr_array_opt[loop](
                            dc_coeff_opt, stride, above, left, bitdepth[x]);
                        dc_compare_u16(dc_coeff, dc_coeff_opt, stride, 64, 64);
                        uninit_data(input);
                        uninit_coeff(dc_coeff, dc_coeff_opt);
                    }
                    break;
                default: GTEST_FAIL();
                }
            }
        }
    }
}
#endif
