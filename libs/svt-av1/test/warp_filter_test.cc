/*
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
#include "gtest/gtest.h"
#include "warp_filter_test_util.h"

using libaom_test::AV1HighbdWarpFilter::AV1HighbdWarpFilterTest;
using libaom_test::AV1WarpFilter::AV1WarpFilterTest;
using std::make_tuple;
using std::tuple;
using svt_av1_test_tool::SVTRandom;

namespace {

TEST_P(AV1WarpFilterTest, CheckOutput) {
    RunCheckOutput(std::get<3>(TEST_GET_PARAM(0)));
}
TEST_P(AV1WarpFilterTest, DISABLED_Speed) {
    RunSpeedTest(std::get<3>(TEST_GET_PARAM(0)));
}

INSTANTIATE_TEST_SUITE_P(
    C, AV1WarpFilterTest,
    libaom_test::AV1WarpFilter::BuildParams(svt_av1_warp_affine_c));

TEST_P(AV1HighbdWarpFilterTest, CheckOutput) {
    RunCheckOutput(std::get<4>(TEST_GET_PARAM(0)));
}

TEST_P(AV1HighbdWarpFilterTest, DISABLED_Speed) {
    RunSpeedTest(std::get<4>(TEST_GET_PARAM(0)));
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    AVX2, AV1WarpFilterTest,
    libaom_test::AV1WarpFilter::BuildParams(svt_av1_warp_affine_avx2));

INSTANTIATE_TEST_SUITE_P(
    SSE4_1, AV1WarpFilterTest,
    libaom_test::AV1WarpFilter::BuildParams(svt_av1_warp_affine_sse4_1));

INSTANTIATE_TEST_SUITE_P(AVX2, AV1HighbdWarpFilterTest,
                         libaom_test::AV1HighbdWarpFilter::BuildParams(
                             svt_av1_highbd_warp_affine_avx2));

INSTANTIATE_TEST_SUITE_P(SSE4_1, AV1HighbdWarpFilterTest,
                         libaom_test::AV1HighbdWarpFilter::BuildParams(
                             svt_av1_highbd_warp_affine_sse4_1));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, AV1WarpFilterTest,
    libaom_test::AV1WarpFilter::BuildParams(svt_av1_warp_affine_neon));

INSTANTIATE_TEST_SUITE_P(NEON, AV1HighbdWarpFilterTest,
                         libaom_test::AV1HighbdWarpFilter::BuildParams(
                             svt_av1_highbd_warp_affine_neon));

#if HAVE_NEON_I8MM
INSTANTIATE_TEST_SUITE_P(
    NEON_I8MM, AV1WarpFilterTest,
    libaom_test::AV1WarpFilter::BuildParams(svt_av1_warp_affine_neon_i8mm));
#endif  // HAVE_NEON_I8MM

#if HAVE_SVE
INSTANTIATE_TEST_SUITE_P(
    SVE, AV1WarpFilterTest,
    libaom_test::AV1WarpFilter::BuildParams(svt_av1_warp_affine_sve));

INSTANTIATE_TEST_SUITE_P(SVE, AV1HighbdWarpFilterTest,
                         libaom_test::AV1HighbdWarpFilter::BuildParams(
                             svt_av1_highbd_warp_affine_sve));
#endif  // HAVE_SVE
#endif  // ARCH_AARCH64

}  // namespace
