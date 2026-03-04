/*
 * Copyright(c) 2022 Intel Corporation
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
#include "random.h"
#include "definitions.h"
#include "aom_dsp_rtcd.h"
#include "noise_util.h"
#include "noise_model.h"

namespace {
using svt_av1_test_tool::SVTRandom;
#if CONFIG_ENABLE_FILM_GRAIN
const int32_t MAX_SIZE = 300;
const int32_t test_time = 20;

void rand_double_array(double *buf, int32_t size, SVTRandom *rnd_float) {
    for (int32_t i = 0; i < size; i++) {
        buf[i] = (double)rnd_float->random_float();
    }
}

void rand_float_array(float *buf, int32_t size, SVTRandom *rnd_float) {
    for (int32_t i = 0; i < size; i++) {
        buf[i] = rnd_float->random_float();
    }
}

void rand_u8_array(uint8_t *buf, int32_t size, SVTRandom *rnd) {
    for (int32_t i = 0; i < size; i++) {
        buf[i] = rnd->random();
    }
}

void rand_u16_array(uint16_t *buf, int32_t size, SVTRandom *rnd) {
    for (int32_t i = 0; i < size; i++) {
        buf[i] = rnd->random();
    }
}

TEST(fg_add_block_observations_internal, AVX2) {
    SVTRandom rnd_float(-10.0f, 10.0f);

    // only input
    double *buffer = new double[MAX_SIZE];
    // only output
    double *buffer_norm_ref = new double[MAX_SIZE];
    double *buffer_norm_mod = new double[MAX_SIZE];
    // input and output
    double *b_buff_ref = new double[MAX_SIZE];
    double *b_buff_mod = new double[MAX_SIZE];
    double *a_buff_ref = new double[MAX_SIZE * MAX_SIZE];
    double *a_buff_mod = new double[MAX_SIZE * MAX_SIZE];

    memset(buffer_norm_ref, 0, sizeof(double) * MAX_SIZE);
    memset(buffer_norm_mod, 0, sizeof(double) * MAX_SIZE);
    // only input
    rand_double_array(buffer, MAX_SIZE, &rnd_float);

    for (int i = 0; i < test_time; i++) {
        int32_t n = (200 + test_time) % MAX_SIZE;
        double val = (double)rnd_float.random_float();
        double recp_sqr = (double)rnd_float.random_float();

        // input and output
        rand_double_array(b_buff_ref, MAX_SIZE, &rnd_float);
        memcpy(b_buff_mod, b_buff_ref, sizeof(double) * MAX_SIZE);
        rand_double_array(a_buff_ref, MAX_SIZE * MAX_SIZE, &rnd_float);
        memcpy(a_buff_mod, a_buff_ref, sizeof(double) * MAX_SIZE * MAX_SIZE);

        svt_av1_add_block_observations_internal_c(
            n, val, recp_sqr, buffer, buffer_norm_ref, b_buff_ref, a_buff_ref);
        svt_av1_add_block_observations_internal_avx2(
            n, val, recp_sqr, buffer, buffer_norm_mod, b_buff_mod, a_buff_mod);

        // compare results
        ASSERT_EQ(0, memcmp(b_buff_ref, b_buff_mod, sizeof(double) * MAX_SIZE))
            << "test"
            << "[" << i << "] "
            << "add_block_observations b_buff_ mismatch!\n";
        ASSERT_EQ(
            0,
            memcmp(
                a_buff_ref, a_buff_mod, sizeof(double) * MAX_SIZE * MAX_SIZE))
            << "test"
            << "[" << i << "] "
            << "add_block_observations a_buff_ mismatch!\n";
        ASSERT_EQ(
            0,
            memcmp(buffer_norm_ref, buffer_norm_mod, sizeof(double) * MAX_SIZE))
            << "test"
            << "[" << i << "] "
            << "add_block_observations buffer_norm_ mismatch!\n";
    }

    delete[] buffer;
    delete[] buffer_norm_ref;
    delete[] buffer_norm_mod;
    delete[] b_buff_ref;
    delete[] b_buff_mod;
    delete[] a_buff_ref;
    delete[] a_buff_mod;
}

TEST(fg_pointwise_multiply, AVX2) {
    SVTRandom rnd_float(-10.0f, 10.0f);

    // only input
    float *a_buff = new float[MAX_SIZE];
    double *b_d_buff = new double[MAX_SIZE];
    double *c_d_buff = new double[MAX_SIZE];
    // only output
    float *b_buff_ref = new float[MAX_SIZE];
    float *b_buff_mod = new float[MAX_SIZE];
    float *c_buff_ref = new float[MAX_SIZE];
    float *c_buff_mod = new float[MAX_SIZE];

    memset(b_buff_ref, 0, sizeof(float) * MAX_SIZE);
    memset(b_buff_mod, 0, sizeof(float) * MAX_SIZE);
    memset(c_buff_ref, 0, sizeof(float) * MAX_SIZE);
    memset(c_buff_mod, 0, sizeof(float) * MAX_SIZE);

    for (int i = 0; i < test_time; i++) {
        int32_t n = (200 + test_time) % MAX_SIZE;

        // input
        rand_float_array(a_buff, MAX_SIZE, &rnd_float);
        rand_double_array(b_d_buff, MAX_SIZE, &rnd_float);
        rand_double_array(c_d_buff, MAX_SIZE, &rnd_float);

        svt_av1_pointwise_multiply_c(
            a_buff, b_buff_ref, c_buff_ref, b_d_buff, c_d_buff, n);
        svt_av1_pointwise_multiply_avx2(
            a_buff, b_buff_mod, c_buff_mod, b_d_buff, c_d_buff, n);

        // compare results
        ASSERT_EQ(0, memcmp(b_buff_ref, b_buff_mod, sizeof(float) * MAX_SIZE))
            << "test"
            << "[" << i << "] "
            << "pointwise_multiply b_buff_ mismatch!\n";
        ASSERT_EQ(0, memcmp(c_buff_ref, c_buff_mod, sizeof(float) * MAX_SIZE))
            << "test"
            << "[" << i << "] "
            << "pointwise_multiply c_buff_ mismatch!\n";
    }

    delete[] a_buff;
    delete[] b_d_buff;
    delete[] c_d_buff;
    delete[] b_buff_ref;
    delete[] b_buff_mod;
    delete[] c_buff_ref;
    delete[] c_buff_mod;
}

TEST(fg_apply_window_to_plane, AVX2) {
    SVTRandom rnd_float(-10.0f, 10.0f);

    // only input
    float *block = new float[MAX_SIZE * MAX_SIZE];
    float *plane = new float[MAX_SIZE * MAX_SIZE];
    float *window = new float[MAX_SIZE * MAX_SIZE];
    // input and output
    float *out_ref = new float[MAX_SIZE * MAX_SIZE];
    float *out_mod = new float[MAX_SIZE * MAX_SIZE];

    for (int i = 0; i < test_time; i++) {
        int32_t n = (200 + test_time) % MAX_SIZE;

        // input
        rand_float_array(block, MAX_SIZE * MAX_SIZE, &rnd_float);
        rand_float_array(plane, MAX_SIZE * MAX_SIZE, &rnd_float);
        rand_float_array(window, MAX_SIZE * MAX_SIZE, &rnd_float);
        // input and output
        rand_float_array(out_ref, MAX_SIZE * MAX_SIZE, &rnd_float);
        memcpy(out_mod, out_ref, sizeof(float) * MAX_SIZE * MAX_SIZE);

        svt_av1_apply_window_function_to_plane_c(
            MAX_SIZE, n, out_ref, MAX_SIZE, block, plane, window);
        svt_av1_apply_window_function_to_plane_avx2(
            MAX_SIZE, n, out_mod, MAX_SIZE, block, plane, window);

        // compare results
        ASSERT_EQ(0,
                  memcmp(out_ref, out_mod, sizeof(float) * MAX_SIZE * MAX_SIZE))
            << "test"
            << "[" << i << "] "
            << "apply_window_to_plane mismatch!\n";
    }

    delete[] block;
    delete[] plane;
    delete[] window;
    delete[] out_ref;
    delete[] out_mod;
}

TEST(fg_noise_tx_filter, AVX2) {
    SVTRandom rnd_float(-10.0f, 10.0f);

    // input and output
    float *out_ref = new float[2 * MAX_SIZE * MAX_SIZE];
    float *out_mod = new float[2 * MAX_SIZE * MAX_SIZE];

    for (int i = 0; i < test_time; i++) {
        int32_t n = (200 + test_time) % MAX_SIZE;
        float psd = rnd_float.random_float();

        // input and output
        rand_float_array(out_ref, 2 * MAX_SIZE * MAX_SIZE, &rnd_float);
        memcpy(out_mod, out_ref, sizeof(float) * 2 * MAX_SIZE * MAX_SIZE);

        svt_aom_noise_tx_filter_c(n, out_ref, psd);
        svt_aom_noise_tx_filter_avx2(n, out_mod, psd);

        // compare results
        ASSERT_EQ(0,
                  memcmp(out_ref, out_mod, sizeof(float) * MAX_SIZE * MAX_SIZE))
            << "test"
            << "[" << i << "] "
            << "apply_window_to_plane mismatch!\n";
    }

    delete[] out_ref;
    delete[] out_mod;
}

TEST(fg_flat_block_finder_extract_block, lbd_AVX2) {
    SVTRandom rnd_float(-10.0f, 10.0f);
    SVTRandom rnd_uint8(0, 255);

    // input
    uint8_t *data = new uint8_t[MAX_SIZE * MAX_SIZE];
    AomFlatBlockFinder *block_finder = new AomFlatBlockFinder;
    block_finder->at_a_inv = new double[MAX_SIZE * MAX_SIZE];
    block_finder->A = new double[MAX_SIZE * MAX_SIZE];
    block_finder->num_params = 10; /*unused*/
    block_finder->normalization = 255;
    block_finder->use_highbd = 0;

    // output
    double *block_ref = new double[MAX_SIZE * MAX_SIZE];
    double *block_mod = new double[MAX_SIZE * MAX_SIZE];
    double *plane_ref = new double[MAX_SIZE * MAX_SIZE];
    double *plane_mod = new double[MAX_SIZE * MAX_SIZE];
    for (int i = 0; i < MAX_SIZE * MAX_SIZE; i++) {
        block_ref[i] = 0.0;
        block_mod[i] = 0.0;
        plane_ref[i] = 0.0;
        plane_mod[i] = 0.0;
    }

    for (int i = 0; i < test_time; i++) {
        block_finder->block_size = (50 + test_time) % MAX_SIZE;
        int32_t offsx = i == 0   ? -1
                        : i == 1 ? (MAX_SIZE + 100)
                                 : rnd_uint8.random() % 10;
        int32_t offsy = rnd_uint8.random() % 10;

        // input
        rand_u8_array(data, MAX_SIZE * MAX_SIZE, &rnd_uint8);
        rand_double_array(
            block_finder->at_a_inv, MAX_SIZE * MAX_SIZE, &rnd_float);
        rand_double_array(block_finder->A, MAX_SIZE * MAX_SIZE, &rnd_float);

        // input and output
        svt_aom_flat_block_finder_extract_block_c(block_finder,
                                                  data,
                                                  MAX_SIZE,
                                                  MAX_SIZE,
                                                  MAX_SIZE,
                                                  offsx,
                                                  offsy,
                                                  plane_ref,
                                                  block_ref);
        svt_aom_flat_block_finder_extract_block_avx2(block_finder,
                                                     data,
                                                     MAX_SIZE,
                                                     MAX_SIZE,
                                                     MAX_SIZE,
                                                     offsx,
                                                     offsy,
                                                     plane_mod,
                                                     block_mod);

        // compare results
        ASSERT_EQ(
            0,
            memcmp(block_ref, block_mod, sizeof(double) * MAX_SIZE * MAX_SIZE))
            << "test"
            << "[" << i << "] "
            << "flat_block_finder_extract_block block_ mismatch !\n ";
        ASSERT_EQ(
            0,
            memcmp(plane_ref, plane_mod, sizeof(double) * MAX_SIZE * MAX_SIZE))
            << "test"
            << "[" << i << "] "
            << "flat_block_finder_extract_block plane_ mismatch !\n ";
    }

    delete[] data;
    delete[] block_ref;
    delete[] block_mod;
    delete[] plane_ref;
    delete[] plane_mod;
    delete[] block_finder->at_a_inv;
    delete[] block_finder->A;
    delete block_finder;
}

TEST(fg_flat_block_finder_extract_block, hbd_AVX2) {
    SVTRandom rnd_float(-10.0f, 10.0f);
    SVTRandom rnd_uint16(0, 1023);

    // input
    uint16_t *data = new uint16_t[MAX_SIZE * MAX_SIZE];
    AomFlatBlockFinder *block_finder = new AomFlatBlockFinder;
    block_finder->at_a_inv = new double[MAX_SIZE * MAX_SIZE];
    block_finder->A = new double[MAX_SIZE * MAX_SIZE];
    block_finder->num_params = 10; /*unused*/
    block_finder->normalization = 1023;
    block_finder->use_highbd = 1;

    // output
    double *block_ref = new double[MAX_SIZE * MAX_SIZE];
    double *block_mod = new double[MAX_SIZE * MAX_SIZE];
    double *plane_ref = new double[MAX_SIZE * MAX_SIZE];
    double *plane_mod = new double[MAX_SIZE * MAX_SIZE];
    for (int i = 0; i < MAX_SIZE * MAX_SIZE; i++) {
        block_ref[i] = 0.0;
        block_mod[i] = 0.0;
        plane_ref[i] = 0.0;
        plane_mod[i] = 0.0;
    }

    for (int i = 0; i < test_time; i++) {
        block_finder->block_size = (50 + test_time) % MAX_SIZE;
        int32_t offsx = i == 0   ? -1
                        : i == 1 ? (MAX_SIZE + 100)
                                 : rnd_uint16.random() % 10;
        int32_t offsy = rnd_uint16.random() % 10;

        // input
        rand_u16_array(data, MAX_SIZE * MAX_SIZE, &rnd_uint16);
        rand_double_array(
            block_finder->at_a_inv, MAX_SIZE * MAX_SIZE, &rnd_float);
        rand_double_array(block_finder->A, MAX_SIZE * MAX_SIZE, &rnd_float);

        // input and output
        svt_aom_flat_block_finder_extract_block_c(block_finder,
                                                  (uint8_t *)data,
                                                  MAX_SIZE,
                                                  MAX_SIZE,
                                                  MAX_SIZE,
                                                  offsx,
                                                  offsy,
                                                  plane_ref,
                                                  block_ref);
        svt_aom_flat_block_finder_extract_block_avx2(block_finder,
                                                     (uint8_t *)data,
                                                     MAX_SIZE,
                                                     MAX_SIZE,
                                                     MAX_SIZE,
                                                     offsx,
                                                     offsy,
                                                     plane_mod,
                                                     block_mod);

        // compare results
        ASSERT_EQ(
            0,
            memcmp(block_ref, block_mod, sizeof(double) * MAX_SIZE * MAX_SIZE))
            << "test"
            << "[" << i << "] "
            << "flat_block_finder_extract_block block_ mismatch !\n ";
        ASSERT_EQ(
            0,
            memcmp(plane_ref, plane_mod, sizeof(double) * MAX_SIZE * MAX_SIZE))
            << "test"
            << "[" << i << "] "
            << "flat_block_finder_extract_block plane_ mismatch !\n ";
    }

    delete[] data;
    delete[] block_ref;
    delete[] block_mod;
    delete[] plane_ref;
    delete[] plane_mod;
    delete[] block_finder->at_a_inv;
    delete[] block_finder->A;
    delete block_finder;
}
#endif  // CONFIG_ENABLE_FILM_GRAIN
}  // namespace
