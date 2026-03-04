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
#include "aom_dsp_rtcd.h"
#include "definitions.h"
#include "compute_sad_avx2.h"
#include "compute_sad_c.h"
#include "me_sad_calculation.h"
#include "motion_estimation.h"
#include "unit_test.h"
#include "unit_test_utility.h"

static const int num_sad = 22;

struct DistInfo {
    uint32_t width;
    uint32_t height;
};

const struct DistInfo sad_size_info[num_sad] = {
    {4, 4},   {4, 8},    {4, 16},   {8, 4},    {8, 8},   {8, 16},
    {8, 32},  {16, 4},   {16, 8},   {16, 16},  {16, 32}, {16, 64},
    {32, 8},  {32, 16},  {32, 32},  {32, 64},  {64, 16}, {64, 32},
    {64, 64}, {64, 128}, {128, 64}, {128, 128}};

typedef uint32_t (*AomSadFn)(const uint8_t *a, int a_stride, const uint8_t *b,
                             int b_stride);

typedef void (*AomSadMultiDFn)(const uint8_t *a, int a_stride,
                               const uint8_t *const b_array[], int b_stride,
                               uint32_t *sad_array);

AomSadFn aom_sad_c_func_ptr_array[num_sad] = {
    svt_aom_sad4x4_c,    svt_aom_sad4x8_c,    svt_aom_sad4x16_c,
    svt_aom_sad8x4_c,    svt_aom_sad8x8_c,    svt_aom_sad8x16_c,
    svt_aom_sad8x32_c,   svt_aom_sad16x4_c,   svt_aom_sad16x8_c,
    svt_aom_sad16x16_c,  svt_aom_sad16x32_c,  svt_aom_sad16x64_c,
    svt_aom_sad32x8_c,   svt_aom_sad32x16_c,  svt_aom_sad32x32_c,
    svt_aom_sad32x64_c,  svt_aom_sad64x16_c,  svt_aom_sad64x32_c,
    svt_aom_sad64x64_c,  svt_aom_sad64x128_c, svt_aom_sad128x64_c,
    svt_aom_sad128x128_c};

AomSadMultiDFn aom_sad_4d_c_func_ptr_array[num_sad] = {
    svt_aom_sad4x4x4d_c,    svt_aom_sad4x8x4d_c,    svt_aom_sad4x16x4d_c,
    svt_aom_sad8x4x4d_c,    svt_aom_sad8x8x4d_c,    svt_aom_sad8x16x4d_c,
    svt_aom_sad8x32x4d_c,   svt_aom_sad16x4x4d_c,   svt_aom_sad16x8x4d_c,
    svt_aom_sad16x16x4d_c,  svt_aom_sad16x32x4d_c,  svt_aom_sad16x64x4d_c,
    svt_aom_sad32x8x4d_c,   svt_aom_sad32x16x4d_c,  svt_aom_sad32x32x4d_c,
    svt_aom_sad32x64x4d_c,  svt_aom_sad64x16x4d_c,  svt_aom_sad64x32x4d_c,
    svt_aom_sad64x64x4d_c,  svt_aom_sad64x128x4d_c, svt_aom_sad128x64x4d_c,
    svt_aom_sad128x128x4d_c};

AomSadFn aom_sad_avx2_func_ptr_array[num_sad] = {
    svt_aom_sad4x4_avx2,    svt_aom_sad4x8_avx2,    svt_aom_sad4x16_avx2,
    svt_aom_sad8x4_avx2,    svt_aom_sad8x8_avx2,    svt_aom_sad8x16_avx2,
    svt_aom_sad8x32_avx2,   svt_aom_sad16x4_avx2,   svt_aom_sad16x8_avx2,
    svt_aom_sad16x16_avx2,  svt_aom_sad16x32_avx2,  svt_aom_sad16x64_avx2,
    svt_aom_sad32x8_avx2,   svt_aom_sad32x16_avx2,  svt_aom_sad32x32_avx2,
    svt_aom_sad32x64_avx2,  svt_aom_sad64x16_avx2,  svt_aom_sad64x32_avx2,
    svt_aom_sad64x64_avx2,  svt_aom_sad64x128_avx2, svt_aom_sad128x64_avx2,
    svt_aom_sad128x128_avx2};

AomSadMultiDFn aom_sad_4d_avx2_func_ptr_array[num_sad] = {
    svt_aom_sad4x4x4d_avx2,    svt_aom_sad4x8x4d_avx2,
    svt_aom_sad4x16x4d_avx2,   svt_aom_sad8x4x4d_avx2,
    svt_aom_sad8x8x4d_avx2,    svt_aom_sad8x16x4d_avx2,
    svt_aom_sad8x32x4d_avx2,   svt_aom_sad16x4x4d_avx2,
    svt_aom_sad16x8x4d_avx2,   svt_aom_sad16x16x4d_avx2,
    svt_aom_sad16x32x4d_avx2,  svt_aom_sad16x64x4d_avx2,
    svt_aom_sad32x8x4d_avx2,   svt_aom_sad32x16x4d_avx2,
    svt_aom_sad32x32x4d_avx2,  svt_aom_sad32x64x4d_avx2,
    svt_aom_sad64x16x4d_avx2,  svt_aom_sad64x32x4d_avx2,
    svt_aom_sad64x64x4d_avx2,  svt_aom_sad64x128x4d_avx2,
    svt_aom_sad128x64x4d_avx2, svt_aom_sad128x128x4d_avx2};

static void init_data_sadMxN(uint8_t **src_ptr, uint32_t *src_stride,
                             uint8_t **ref_ptr, uint32_t *ref_stride) {
    *src_stride = svt_create_random_aligned_stride(MAX_SB_SIZE, 64);
    *ref_stride = svt_create_random_aligned_stride(MAX_SB_SIZE, 64);
    *src_ptr = (uint8_t *)malloc(sizeof(**src_ptr) * MAX_SB_SIZE * *src_stride);
    *ref_ptr = (uint8_t *)malloc(sizeof(**ref_ptr) * MAX_SB_SIZE * *ref_stride);
    svt_buf_random_u8(*src_ptr, MAX_SB_SIZE * *src_stride);
    svt_buf_random_u8(*ref_ptr, MAX_SB_SIZE * *ref_stride);
}

static void init_data_sadMxNx4d(uint8_t **src_ptr, uint32_t *src_stride,
                                uint8_t *ref_ptr[4], uint32_t *ref_stride) {
    *src_stride = svt_create_random_aligned_stride(MAX_SB_SIZE, 64);
    *ref_stride = svt_create_random_aligned_stride(MAX_SB_SIZE, 64);
    *src_ptr = (uint8_t *)malloc(sizeof(**src_ptr) * MAX_SB_SIZE * *src_stride);
    ref_ptr[0] =
        (uint8_t *)malloc(sizeof(**ref_ptr) * (MAX_SB_SIZE + 3) * *ref_stride);
    svt_buf_random_u8(*src_ptr, MAX_SB_SIZE * *src_stride);
    svt_buf_random_u8(ref_ptr[0], (MAX_SB_SIZE + 3) * *ref_stride);
    ref_ptr[1] = ref_ptr[0] + *ref_stride;
    ref_ptr[2] = ref_ptr[1] + *ref_stride;
    ref_ptr[3] = ref_ptr[2] + *ref_stride;
}

static void uninit_data(uint8_t *src_ptr, uint8_t *ref_ptr) {
    free(src_ptr);
    free(ref_ptr);
}

void sadMxN_match_test(const AomSadFn *const func_table) {
    uint8_t *src_ptr, *ref_ptr;
    uint32_t src_stride, ref_stride;

    for (int i = 0; i < 10; i++) {
        init_data_sadMxN(&src_ptr, &src_stride, &ref_ptr, &ref_stride);

        for (int j = 0; j < num_sad; j++) {
            if (func_table[j] == NULL)
                continue;
            const uint32_t sad_org = aom_sad_c_func_ptr_array[j](
                src_ptr, src_stride, ref_ptr, ref_stride);
            const uint32_t sad_opt =
                func_table[j](src_ptr, src_stride, ref_ptr, ref_stride);

            EXPECT_EQ(sad_org, sad_opt);
        }

        uninit_data(src_ptr, ref_ptr);
    }
}

void sadMxNx4d_match_test(const AomSadMultiDFn *const func_table) {
    uint8_t *src_ptr, *ref_ptr[4];
    uint32_t src_stride, ref_stride;
    uint32_t sad_array_org[4], sad_array_opt[4];

    for (int i = 0; i < 10; i++) {
        init_data_sadMxNx4d(&src_ptr, &src_stride, ref_ptr, &ref_stride);

        for (int j = 0; j < num_sad; j++) {
            if (func_table[j] == NULL)
                continue;
            svt_buf_random_u32(sad_array_opt, 4);
            aom_sad_4d_c_func_ptr_array[j](
                src_ptr, src_stride, ref_ptr, ref_stride, sad_array_org);
            func_table[j](
                src_ptr, src_stride, ref_ptr, ref_stride, sad_array_opt);

            for (int l = 0; l < 4; l++)
                EXPECT_EQ(sad_array_org[l], sad_array_opt[l]);
        }

        uninit_data(src_ptr, ref_ptr[0]);
    }
}

void sadMxN_speed_test(const AomSadFn *const func_table) {
    uint8_t *src_ptr, *ref_ptr;
    uint32_t src_stride, ref_stride;
    double time_c, time_o;
    uint64_t start_time_seconds, start_time_useconds;
    uint64_t middle_time_seconds, middle_time_useconds;
    uint64_t finish_time_seconds, finish_time_useconds;

    init_data_sadMxN(&src_ptr, &src_stride, &ref_ptr, &ref_stride);

    for (int j = 0; j < num_sad; j++) {
        if (func_table[j] == NULL)
            continue;
        const uint32_t width = sad_size_info[j].width;
        const uint32_t height = sad_size_info[j].height;
        const uint64_t num_loop = 100000000 / (width + height);
        uint32_t sad_org = 0, sad_opt = 0;

        svt_av1_get_time(&start_time_seconds, &start_time_useconds);

        for (uint64_t i = 0; i < num_loop; i++)
            sad_org = aom_sad_c_func_ptr_array[j](
                src_ptr, src_stride, ref_ptr, ref_stride);

        svt_av1_get_time(&middle_time_seconds, &middle_time_useconds);

        for (uint64_t i = 0; i < num_loop; i++)
            sad_opt = func_table[j](src_ptr, src_stride, ref_ptr, ref_stride);

        svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);
        time_c = svt_av1_compute_overall_elapsed_time_ms(start_time_seconds,
                                                         start_time_useconds,
                                                         middle_time_seconds,
                                                         middle_time_useconds);
        time_o = svt_av1_compute_overall_elapsed_time_ms(middle_time_seconds,
                                                         middle_time_useconds,
                                                         finish_time_seconds,
                                                         finish_time_useconds);

        EXPECT_EQ(sad_org, sad_opt);

        printf("Average Nanoseconds per Function Call\n");
        printf("    aom_sad%2ux%2u_c()   : %6.2f\n",
               width,
               height,
               1000000 * time_c / num_loop);
        printf(
            "    aom_sad%2ux%2u_opt() : %6.2f   (Comparison: "
            "%5.2fx)\n",
            width,
            height,
            1000000 * time_o / num_loop,
            time_c / time_o);
    }

    uninit_data(src_ptr, ref_ptr);
}

void sadMxNx4d_speed_test(const AomSadMultiDFn *const func_table) {
    uint8_t *src_ptr, *ref_ptr[4];
    uint32_t src_stride, ref_stride;
    uint32_t sad_array_org[4], sad_array_opt[4];
    double time_c, time_o;
    uint64_t start_time_seconds, start_time_useconds;
    uint64_t middle_time_seconds, middle_time_useconds;
    uint64_t finish_time_seconds, finish_time_useconds;

    init_data_sadMxNx4d(&src_ptr, &src_stride, ref_ptr, &ref_stride);
    svt_buf_random_u32(sad_array_opt, 4);

    for (int j = 0; j < num_sad; j++) {
        if (func_table[j] == NULL)
            continue;
        const uint32_t width = sad_size_info[j].width;
        const uint32_t height = sad_size_info[j].height;
        const uint64_t num_loop = 20000000 / (width + height);

        svt_av1_get_time(&start_time_seconds, &start_time_useconds);

        for (uint64_t i = 0; i < num_loop; i++)
            aom_sad_4d_c_func_ptr_array[j](
                src_ptr, src_stride, ref_ptr, ref_stride, sad_array_org);

        svt_av1_get_time(&middle_time_seconds, &middle_time_useconds);

        for (uint64_t i = 0; i < num_loop; i++)
            func_table[j](
                src_ptr, src_stride, ref_ptr, ref_stride, sad_array_opt);

        svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);
        time_c = svt_av1_compute_overall_elapsed_time_ms(start_time_seconds,
                                                         start_time_useconds,
                                                         middle_time_seconds,
                                                         middle_time_useconds);
        time_o = svt_av1_compute_overall_elapsed_time_ms(middle_time_seconds,
                                                         middle_time_useconds,
                                                         finish_time_seconds,
                                                         finish_time_useconds);

        for (int l = 0; l < 4; l++)
            EXPECT_EQ(sad_array_org[l], sad_array_opt[l]);

        printf("Average Nanoseconds per Function Call\n");
        printf("    aom_sad%2dx%2d_c()   : %6.2f\n",
               width,
               height,
               1000000 * time_c / num_loop);
        printf(
            "    aom_sad%2dx%2d_opt() : %6.2f   (Comparison: "
            "%5.2fx)\n",
            width,
            height,
            1000000 * time_o / num_loop,
            time_c / time_o);
    }

    uninit_data(src_ptr, ref_ptr[0]);
}

TEST(MotionEstimation_avx2, sadMxN_match) {
    sadMxN_match_test(aom_sad_avx2_func_ptr_array);
}

TEST(MotionEstimation_avx2, sadMxNx4d_match) {
    sadMxNx4d_match_test(aom_sad_4d_avx2_func_ptr_array);
}

TEST(MotionEstimation_avx2, DISABLED_sadMxN_speed) {
    sadMxN_speed_test(aom_sad_avx2_func_ptr_array);
}

TEST(MotionEstimation_avx2, DISABLED_sadMxNx4d_speed) {
    sadMxNx4d_speed_test(aom_sad_4d_avx2_func_ptr_array);
}

#if EN_AVX512_SUPPORT

// NULL means not implemented
AomSadFn aom_sad_avx512_func_ptr_array[num_sad] = {NULL,
                                                   NULL,
                                                   NULL,
                                                   NULL,
                                                   NULL,
                                                   NULL,
                                                   NULL,
                                                   NULL,
                                                   NULL,
                                                   NULL,
                                                   NULL,
                                                   NULL,
                                                   NULL,
                                                   NULL,
                                                   NULL,
                                                   NULL,
                                                   svt_aom_sad64x16_avx512,
                                                   svt_aom_sad64x32_avx512,
                                                   svt_aom_sad64x64_avx512,
                                                   svt_aom_sad64x128_avx512,
                                                   svt_aom_sad128x64_avx512,
                                                   svt_aom_sad128x128_avx512};

// NULL means not implemented
AomSadMultiDFn aom_sad_4d_avx512_func_ptr_array[num_sad] = {
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    svt_aom_sad64x16x4d_avx2,
    svt_aom_sad64x32x4d_avx2,
    svt_aom_sad64x64x4d_avx2,
    svt_aom_sad64x128x4d_avx2,
    svt_aom_sad128x64x4d_avx512,
    svt_aom_sad128x128x4d_avx512};

TEST(AVX512_MotionEstimation_avx512, sadMxN_match) {
    sadMxN_match_test(aom_sad_avx512_func_ptr_array);
}

TEST(AVX512_MotionEstimation_avx512, sadMxNx4d_match) {
    sadMxNx4d_match_test(aom_sad_4d_avx512_func_ptr_array);
}

TEST(AVX512_MotionEstimation_avx512, DISABLED_sadMxN_speed) {
    sadMxN_speed_test(aom_sad_avx512_func_ptr_array);
}

TEST(AVX512_MotionEstimation_avx512, DISABLED_sadMxNx4d_speed) {
    sadMxNx4d_speed_test(aom_sad_4d_avx512_func_ptr_array);
}

#endif  // EN_AVX512_SUPPORT
