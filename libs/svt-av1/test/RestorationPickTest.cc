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
#include "common_utils.h"
#include "restoration.h"
#include "unit_test_utility.h"
#include "util.h"

#ifdef ARCH_X86_64

#include <immintrin.h>  // AVX2
#include "synonyms.h"
#include "synonyms_avx2.h"

#endif

#include "restoration_pick.h"

typedef void (*av1_compute_stats_func)(int32_t wiener_win, const uint8_t *dgd8,
                                       const uint8_t *src8, int32_t h_start,
                                       int32_t h_end, int32_t v_start,
                                       int32_t v_end, int32_t dgd_stride,
                                       int32_t src_stride, int64_t *M,
                                       int64_t *H);

typedef void (*av1_compute_stats_highbd_func)(
    int32_t wiener_win, const uint8_t *dgd8, const uint8_t *src8,
    int32_t h_start, int32_t h_end, int32_t v_start, int32_t v_end,
    int32_t dgd_stride, int32_t src_stride, int64_t *M, int64_t *H,
    EbBitDepth bit_depth);

typedef ::testing::tuple<BlockSize, av1_compute_stats_func, int, int>
    av1_compute_stats_params;

class av1_compute_stats_test
    : public ::testing::TestWithParam<av1_compute_stats_params> {
    uint8_t *dgd, *src;
    int32_t dgd_stride, src_stride;
    int64_t M_org[WIENER_WIN2], M_opt[WIENER_WIN2];
    int64_t H_org[WIENER_WIN2 * WIENER_WIN2], H_opt[WIENER_WIN2 * WIENER_WIN2];

    void init_data(const int idx) {
        if (!idx) {
            memset(
                dgd,
                0,
                sizeof(*dgd) * 2 * RESTORATION_UNITPELS_HORZ_MAX * dgd_stride);
            memset(
                src,
                0,
                sizeof(*src) * 2 * RESTORATION_UNITPELS_HORZ_MAX * src_stride);
        } else if (1 == idx) {
            svt_buf_random_u8_to_255(
                dgd, 2 * RESTORATION_UNITPELS_HORZ_MAX * dgd_stride);
            svt_buf_random_u8_to_255(
                src, 2 * RESTORATION_UNITPELS_HORZ_MAX * src_stride);
        } else if (2 == idx) {
            svt_buf_random_u8_to_255(
                dgd, 2 * RESTORATION_UNITPELS_HORZ_MAX * dgd_stride);
            memset(
                src,
                0,
                sizeof(*src) * 2 * RESTORATION_UNITPELS_HORZ_MAX * src_stride);
        } else if (3 == idx) {
            memset(
                dgd,
                0,
                sizeof(*dgd) * 2 * RESTORATION_UNITPELS_HORZ_MAX * dgd_stride);
            svt_buf_random_u8_to_255(
                src, 2 * RESTORATION_UNITPELS_HORZ_MAX * src_stride);
        } else if (4 == idx) {
            svt_buf_random_u8_to_0_or_255(
                dgd, 2 * RESTORATION_UNITPELS_HORZ_MAX * dgd_stride);
            svt_buf_random_u8_to_0_or_255(
                src, 2 * RESTORATION_UNITPELS_HORZ_MAX * src_stride);
        } else {
            svt_buf_random_u8(dgd,
                              2 * RESTORATION_UNITPELS_HORZ_MAX * dgd_stride);
            svt_buf_random_u8(src,
                              2 * RESTORATION_UNITPELS_HORZ_MAX * src_stride);
        }
    }

    void init_output() {
        svt_buf_random_s64(M_org, WIENER_WIN2);
        svt_buf_random_s64(H_org, WIENER_WIN2 * WIENER_WIN2);
        memcpy(M_opt, M_org, sizeof(*M_org) * WIENER_WIN2);
        memcpy(H_opt, H_org, sizeof(*H_org) * WIENER_WIN2 * WIENER_WIN2);
    }

  public:
    av1_compute_stats_test() = default;
    void match_test() {
        const int block_size = TEST_GET_PARAM(0);
        int width;
        int height;
        if (block_size < BLOCK_SIZES_ALL) {
            width = block_size_wide[block_size];
            height = block_size_high[block_size];
        } else {
            if (block_size == BLOCK_SIZES_ALL) {
                width = 308;
                height = 308;
            } else {
                width = 304;
                height = 304;
            }
        }
        av1_compute_stats_func func = TEST_GET_PARAM(1);
        int test_idx = TEST_GET_PARAM(2);
        int wiener_win = TEST_GET_PARAM(3);
        init_output();

        dgd_stride =
            svt_create_random_aligned_stride(2 * RESTORATION_UNITSIZE_MAX, 64);
        src_stride =
            svt_create_random_aligned_stride(2 * RESTORATION_UNITSIZE_MAX, 64);
        dgd = (uint8_t *)malloc(sizeof(*dgd) * 2 *
                                RESTORATION_UNITPELS_HORZ_MAX * dgd_stride);
        src = (uint8_t *)malloc(sizeof(*src) * 2 *
                                RESTORATION_UNITPELS_HORZ_MAX * src_stride);

        int test_times = test_idx == 0 ? 1 : 10;
        for (int i = 0; i < test_times; i++) {
            width += i % 2 == 0 ? 0 : 1;
            height += i % 2 == 0 ? 1 : 0;
            init_data(test_idx);
            uint8_t *const d = dgd + WIENER_WIN * dgd_stride;
            uint8_t *const s = src + WIENER_WIN * src_stride;

            svt_av1_compute_stats_c(wiener_win,
                                    d,
                                    s,
                                    0,
                                    width,
                                    0,
                                    height,
                                    dgd_stride,
                                    src_stride,
                                    M_org,
                                    H_org);

            func(wiener_win,
                 d,
                 s,
                 0,
                 width,
                 0,
                 height,
                 dgd_stride,
                 src_stride,
                 M_opt,
                 H_opt);

            ASSERT_EQ(0, memcmp(M_org, M_opt, sizeof(M_org)));
            ASSERT_EQ(0, memcmp(H_org, H_opt, sizeof(H_org)));
        }
        free(dgd);
        free(src);
    }

    void speed_test() {
        double time_c, time_o;
        uint64_t start_time_seconds, start_time_useconds;
        uint64_t middle_time_seconds, middle_time_useconds;
        uint64_t finish_time_seconds, finish_time_useconds;
        av1_compute_stats_func func = TEST_GET_PARAM(1);
        int wiener_win = TEST_GET_PARAM(3);

        init_output();
        dgd_stride =
            svt_create_random_aligned_stride(2 * RESTORATION_UNITSIZE_MAX, 64);
        src_stride =
            svt_create_random_aligned_stride(2 * RESTORATION_UNITSIZE_MAX, 64);
        dgd = (uint8_t *)malloc(sizeof(*dgd) * 2 *
                                RESTORATION_UNITPELS_HORZ_MAX * dgd_stride);
        src = (uint8_t *)malloc(sizeof(*src) * 2 *
                                RESTORATION_UNITPELS_HORZ_MAX * src_stride);
        init_data(5);
        uint8_t *const d = dgd + WIENER_WIN * dgd_stride;
        uint8_t *const s = src + WIENER_WIN * src_stride;
        const int32_t h_start = 0;
        const int32_t v_start = 0;
        const int32_t h_end = RESTORATION_UNITPELS_HORZ_MAX;
        const int32_t v_end = RESTORATION_UNITPELS_HORZ_MAX;

        const uint64_t num_loop = 100000 / (wiener_win * wiener_win);

        svt_av1_get_time(&start_time_seconds, &start_time_useconds);

        for (uint64_t i = 0; i < num_loop; i++) {
            svt_av1_compute_stats_c(wiener_win,
                                    d,
                                    s,
                                    h_start,
                                    h_end,
                                    v_start,
                                    v_end,
                                    dgd_stride,
                                    src_stride,
                                    M_org,
                                    H_org);
        }

        svt_av1_get_time(&middle_time_seconds, &middle_time_useconds);

        for (uint64_t i = 0; i < num_loop; i++) {
            func(wiener_win,
                 d,
                 s,
                 h_start,
                 h_end,
                 v_start,
                 v_end,
                 dgd_stride,
                 src_stride,
                 M_opt,
                 H_opt);
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

        ASSERT_EQ(0, memcmp(M_org, M_opt, sizeof(M_org)));
        ASSERT_EQ(0, memcmp(H_org, H_opt, sizeof(H_org)));

        printf("Average Nanoseconds per Function Call\n");
        printf("    svt_av1_compute_stats_c(%d)   : %6.2f\n",
               wiener_win,
               1000000 * time_c / num_loop);
        printf(
            "    svt_av1_compute_stats_opt(%d) : %6.2f   (Comparison: "
            "%5.2fx)\n",
            wiener_win,
            1000000 * time_o / num_loop,
            time_c / time_o);
    }
};

TEST_P(av1_compute_stats_test, match) {
    match_test();
}
TEST_P(av1_compute_stats_test, DISABLED_speed) {
    speed_test();
}

#ifdef ARCH_X86_64

INSTANTIATE_TEST_SUITE_P(
    AVX2, av1_compute_stats_test,
    ::testing::Combine(::testing::Range(BLOCK_4X4,
                                        (BlockSize)(BLOCK_SIZES_ALL + 2)),
                       ::testing::Values(svt_av1_compute_stats_sse4_1,
                                         svt_av1_compute_stats_avx2),
                       ::testing::Range(0, 6),
                       ::testing::Values(WIENER_WIN_CHROMA, WIENER_WIN)));

#if EN_AVX512_SUPPORT
INSTANTIATE_TEST_SUITE_P(
    AVX512, av1_compute_stats_test,
    ::testing::Combine(::testing::Range(BLOCK_4X4,
                                        (BlockSize)(BLOCK_SIZES_ALL + 2)),
                       ::testing::Values(svt_av1_compute_stats_avx512),
                       ::testing::Range(0, 6),
                       ::testing::Values(WIENER_WIN_CHROMA, WIENER_WIN)));
#endif

#endif  // ARCH_X86_64

#if ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, av1_compute_stats_test,
    ::testing::Combine(::testing::Range(BLOCK_4X4,
                                        (BlockSize)(BLOCK_SIZES_ALL + 2)),
                       ::testing::Values(svt_av1_compute_stats_neon),
                       ::testing::Range(0, 6),
                       ::testing::Values(WIENER_WIN_CHROMA, WIENER_WIN)));

#if HAVE_SVE
INSTANTIATE_TEST_SUITE_P(
    SVE, av1_compute_stats_test,
    ::testing::Combine(::testing::Range(BLOCK_4X4,
                                        (BlockSize)(BLOCK_SIZES_ALL + 2)),
                       ::testing::Values(svt_av1_compute_stats_sve),
                       ::testing::Range(0, 6),
                       ::testing::Values(WIENER_WIN_CHROMA, WIENER_WIN)));
#endif  // HAVE_SVE
#endif  // ARCH_AARCH64

#if CONFIG_ENABLE_HIGH_BIT_DEPTH

typedef ::testing::tuple<BlockSize, av1_compute_stats_highbd_func, int, int,
                         EbBitDepth>
    av1_compute_stats_hbd_params;

class av1_compute_stats_test_hbd
    : public ::testing::TestWithParam<av1_compute_stats_hbd_params> {
    uint16_t *dgd, *src;
    int32_t dgd_stride, src_stride;
    int64_t M_org[WIENER_WIN2], M_opt[WIENER_WIN2];
    int64_t H_org[WIENER_WIN2 * WIENER_WIN2], H_opt[WIENER_WIN2 * WIENER_WIN2];

    void init_data_highbd(EbBitDepth bd, const int idx) {
        if (!idx) {
            memset(
                dgd,
                0,
                sizeof(*dgd) * 2 * RESTORATION_UNITPELS_HORZ_MAX * dgd_stride);
            memset(
                src,
                0,
                sizeof(*src) * 2 * RESTORATION_UNITPELS_HORZ_MAX * src_stride);
        } else if (1 == idx) {
            svt_buf_random_u16_to_bd(
                dgd, 2 * RESTORATION_UNITPELS_HORZ_MAX * dgd_stride, bd);
            svt_buf_random_u16_to_bd(
                src, 2 * RESTORATION_UNITPELS_HORZ_MAX * src_stride, bd);
        } else if (2 == idx) {
            svt_buf_random_u16_to_bd(
                dgd, 2 * RESTORATION_UNITPELS_HORZ_MAX * dgd_stride, bd);
            memset(
                src,
                0,
                sizeof(*src) * 2 * RESTORATION_UNITPELS_HORZ_MAX * src_stride);
        } else if (3 == idx) {
            memset(
                dgd,
                0,
                sizeof(*dgd) * 2 * RESTORATION_UNITPELS_HORZ_MAX * dgd_stride);
            svt_buf_random_u16_to_bd(
                src, 2 * RESTORATION_UNITPELS_HORZ_MAX * src_stride, bd);
        } else if (4 == idx) {
            svt_buf_random_u16_to_0_or_bd(
                dgd, 2 * RESTORATION_UNITPELS_HORZ_MAX * dgd_stride, bd);
            svt_buf_random_u16_to_0_or_bd(
                src, 2 * RESTORATION_UNITPELS_HORZ_MAX * src_stride, bd);
        } else if (5 == idx) {
            // Trigger the 32-bit overflow in Step 3 and 4 for EB_TWELVE_BIT.
            svt_buf_random_u16_to_bd(
                dgd, 2 * RESTORATION_UNITPELS_HORZ_MAX * dgd_stride, bd);
            for (int i = 0; i < 2 * RESTORATION_UNITPELS_HORZ_MAX; i++) {
                memset(dgd + i * dgd_stride, 0, sizeof(*dgd) * 20);
            }
            memset(
                src,
                0,
                sizeof(*src) * 2 * RESTORATION_UNITPELS_HORZ_MAX * src_stride);
        } else if (6 == idx) {
            // Trigger the 32-bit overflow in Step 5 and 6 for EB_TWELVE_BIT.
            svt_buf_random_u16_to_bd(
                dgd, 2 * RESTORATION_UNITPELS_HORZ_MAX * dgd_stride, bd);
            memset(
                dgd, 0, sizeof(*dgd) * 2 * RESTORATION_UNITPELS_HORZ_MAX * 20);
            memset(
                src,
                0,
                sizeof(*src) * 2 * RESTORATION_UNITPELS_HORZ_MAX * src_stride);
        } else {
            svt_buf_random_u16_with_bd(
                dgd, 2 * RESTORATION_UNITPELS_HORZ_MAX * dgd_stride, bd);
            svt_buf_random_u16_with_bd(
                src, 2 * RESTORATION_UNITPELS_HORZ_MAX * src_stride, bd);
        }
    }

    void init_output() {
        svt_buf_random_s64(M_org, WIENER_WIN2);
        svt_buf_random_s64(H_org, WIENER_WIN2 * WIENER_WIN2);
        memcpy(M_opt, M_org, sizeof(*M_org) * WIENER_WIN2);
        memcpy(H_opt, H_org, sizeof(*H_org) * WIENER_WIN2 * WIENER_WIN2);
    }

  public:
    av1_compute_stats_test_hbd() = default;
    void highbd_match_test() {
        const int block_size = TEST_GET_PARAM(0);
        int width;
        int height;
        if (block_size < BLOCK_SIZES_ALL) {
            width = block_size_wide[block_size];
            height = block_size_high[block_size];
        } else {
            if (block_size == BLOCK_SIZES_ALL) {
                width = 308;
                height = 308;
            } else {
                width = 304;
                height = 304;
            }
        }
        av1_compute_stats_highbd_func func = TEST_GET_PARAM(1);
        int test_idx = TEST_GET_PARAM(2);
        int wiener_win = TEST_GET_PARAM(3);
        const EbBitDepth bit_depth = TEST_GET_PARAM(4);
        init_output();

        dgd_stride =
            svt_create_random_aligned_stride(2 * RESTORATION_UNITSIZE_MAX, 64);
        src_stride =
            svt_create_random_aligned_stride(2 * RESTORATION_UNITSIZE_MAX, 64);
        dgd = (uint16_t *)malloc(sizeof(*dgd) * 2 *
                                 RESTORATION_UNITPELS_HORZ_MAX * dgd_stride);
        src = (uint16_t *)malloc(sizeof(*src) * 2 *
                                 RESTORATION_UNITPELS_HORZ_MAX * src_stride);

        int test_times = test_idx == 0 ? 1 : 10;
        for (int i = 0; i < test_times; i++) {
            width += i % 2 == 0 ? 0 : 1;
            height += i % 2 == 0 ? 1 : 0;
            init_data_highbd(bit_depth, test_idx);

            const uint16_t *const d =
                dgd + WIENER_WIN * dgd_stride + WIENER_WIN;
            const uint16_t *const s =
                src + WIENER_WIN * src_stride + WIENER_WIN;
            const uint8_t *const dgd8 = CONVERT_TO_BYTEPTR(d);
            const uint8_t *const src8 = CONVERT_TO_BYTEPTR(s);

            svt_av1_compute_stats_highbd_c(wiener_win,
                                           dgd8,
                                           src8,
                                           0,
                                           width,
                                           0,
                                           height,
                                           dgd_stride,
                                           src_stride,
                                           M_org,
                                           H_org,
                                           bit_depth);

            func(wiener_win,
                 dgd8,
                 src8,
                 0,
                 width,
                 0,
                 height,
                 dgd_stride,
                 src_stride,
                 M_opt,
                 H_opt,
                 bit_depth);

            ASSERT_EQ(0, memcmp(M_org, M_opt, sizeof(M_org)));
            ASSERT_EQ(0, memcmp(H_org, H_opt, sizeof(H_org)));
        }
        free(dgd);
        free(src);
    }

    void highbd_speed_test() {
        double time_c, time_o;
        uint64_t start_time_seconds, start_time_useconds;
        uint64_t middle_time_seconds, middle_time_useconds;
        uint64_t finish_time_seconds, finish_time_useconds;

        av1_compute_stats_highbd_func func = TEST_GET_PARAM(1);
        int wiener_win = TEST_GET_PARAM(3);
        const EbBitDepth bit_depth = TEST_GET_PARAM(4);
        init_output();

        dgd_stride =
            svt_create_random_aligned_stride(2 * RESTORATION_UNITSIZE_MAX, 64);
        src_stride =
            svt_create_random_aligned_stride(2 * RESTORATION_UNITSIZE_MAX, 64);
        dgd = (uint16_t *)malloc(sizeof(*dgd) * 2 *
                                 RESTORATION_UNITPELS_HORZ_MAX * dgd_stride);
        src = (uint16_t *)malloc(sizeof(*src) * 2 *
                                 RESTORATION_UNITPELS_HORZ_MAX * src_stride);

        const int32_t h_start = 0;
        const int32_t v_start = 0;
        const int32_t h_end = RESTORATION_UNITPELS_HORZ_MAX;
        const int32_t v_end = RESTORATION_UNITPELS_HORZ_MAX;

        init_data_highbd(bit_depth, 7);
        uint16_t *const d = dgd + WIENER_WIN * dgd_stride;
        uint16_t *const s = src + WIENER_WIN * src_stride;
        const uint8_t *const dgd8 = CONVERT_TO_BYTEPTR(d);
        const uint8_t *const src8 = CONVERT_TO_BYTEPTR(s);

        const uint64_t num_loop = 1000000 / (wiener_win * wiener_win);

        svt_av1_get_time(&start_time_seconds, &start_time_useconds);

        for (uint64_t i = 0; i < num_loop; i++) {
            svt_av1_compute_stats_highbd_c(wiener_win,
                                           dgd8,
                                           src8,
                                           h_start,
                                           h_end,
                                           v_start,
                                           v_end,
                                           dgd_stride,
                                           src_stride,
                                           M_org,
                                           H_org,
                                           bit_depth);
        }

        svt_av1_get_time(&middle_time_seconds, &middle_time_useconds);

        for (uint64_t i = 0; i < num_loop; i++) {
            func(wiener_win,
                 dgd8,
                 src8,
                 h_start,
                 h_end,
                 v_start,
                 v_end,
                 dgd_stride,
                 src_stride,
                 M_opt,
                 H_opt,
                 bit_depth);
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

        ASSERT_EQ(0, memcmp(M_org, M_opt, sizeof(M_org)));
        ASSERT_EQ(0, memcmp(H_org, H_opt, sizeof(H_org)));

        printf("Average Nanoseconds per Function Call\n");
        printf("    svt_av1_compute_stats_highbd_c(%d)   : %6.2f\n",
               wiener_win,
               1000000 * time_c / num_loop);
        printf(
            "    av1_compute_stats_highbd_opt(%d) : %6.2f   "
            "(Comparison: "
            "%5.2fx)\n",
            wiener_win,
            1000000 * time_o / num_loop,
            time_c / time_o);
    }
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(av1_compute_stats_test);

TEST_P(av1_compute_stats_test_hbd, match) {
    highbd_match_test();
}
TEST_P(av1_compute_stats_test_hbd, DISABLED_speed) {
    highbd_speed_test();
}

#ifdef ARCH_X86_64

INSTANTIATE_TEST_SUITE_P(
    SSE4_1, av1_compute_stats_test_hbd,
    ::testing::Combine(
        ::testing::Range(BLOCK_4X4, (BlockSize)(BLOCK_SIZES_ALL + 2)),
        ::testing::Values(svt_av1_compute_stats_highbd_sse4_1),
        ::testing::Range(0, 8),
        ::testing::Values(WIENER_WIN_CHROMA, WIENER_WIN),
        ::testing::Values(EB_EIGHT_BIT, EB_TEN_BIT, EB_TWELVE_BIT)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, av1_compute_stats_test_hbd,
    ::testing::Combine(
        ::testing::Range(BLOCK_4X4, (BlockSize)(BLOCK_SIZES_ALL + 2)),
        ::testing::Values(svt_av1_compute_stats_highbd_avx2),
        ::testing::Range(0, 8),
        ::testing::Values(WIENER_WIN_CHROMA, WIENER_WIN),
        ::testing::Values(EB_EIGHT_BIT, EB_TEN_BIT, EB_TWELVE_BIT)));

#if EN_AVX512_SUPPORT
INSTANTIATE_TEST_SUITE_P(
    AVX512, av1_compute_stats_test_hbd,
    ::testing::Combine(
        ::testing::Range(BLOCK_4X4, (BlockSize)(BLOCK_SIZES_ALL + 2)),
        ::testing::Values(svt_av1_compute_stats_highbd_avx512),
        ::testing::Range(0, 8),
        ::testing::Values(WIENER_WIN_CHROMA, WIENER_WIN),
        ::testing::Values(EB_EIGHT_BIT, EB_TEN_BIT, EB_TWELVE_BIT)));
#endif

#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64

INSTANTIATE_TEST_SUITE_P(
    NEON, av1_compute_stats_test_hbd,
    ::testing::Combine(
        ::testing::Range(BLOCK_4X4, (BlockSize)(BLOCK_SIZES_ALL + 2)),
        ::testing::Values(svt_av1_compute_stats_highbd_neon),
        ::testing::Range(0, 8),
        ::testing::Values(WIENER_WIN_CHROMA, WIENER_WIN),
        ::testing::Values(EB_EIGHT_BIT, EB_TEN_BIT, EB_TWELVE_BIT)));

#if HAVE_SVE
INSTANTIATE_TEST_SUITE_P(
    SVE, av1_compute_stats_test_hbd,
    ::testing::Combine(
        ::testing::Range(BLOCK_4X4, (BlockSize)(BLOCK_SIZES_ALL + 2)),
        ::testing::Values(svt_av1_compute_stats_highbd_sve),
        ::testing::Range(0, 8),
        ::testing::Values(WIENER_WIN_CHROMA, WIENER_WIN),
        ::testing::Values(EB_EIGHT_BIT, EB_TEN_BIT, EB_TWELVE_BIT)));

#endif  // HAVE_SVE
#endif  // ARCH_AARCH64

#endif  // CONFIG_ENABLE_HIGH_BIT_DEPTH
