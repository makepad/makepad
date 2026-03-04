/*
 * Copyright (c) 2024, Alliance for Open Media. All rights reserved.
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at www.aomedia.org/license/software. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at www.aomedia.org/license/patent.
 */

#include "common_dsp_rtcd.h"

#include "third_party/googletest/include/gtest/gtest.h"

#if ARCH_AARCH64 || ARCH_X86_64
static void append_negative_gtest_filter(const char *str) {
    std::string flag_value = GTEST_FLAG_GET(filter);
    // Negative patterns begin with one '-' followed by a ':' separated list.
    if (flag_value.find('-') == std::string::npos)
        flag_value += '-';
    // OPT.* matches TEST() functions
    // OPT/* matches TEST_P() functions
    // OPT_* matches tests which have been manually sharded.
    // We do not match OPT* because of SSE/SSE2 collisions.
    const char *search_terminators = "./_";
    for (size_t pos = 0; pos < strlen(search_terminators); ++pos) {
        flag_value += ":";
        flag_value += "*";
        flag_value += str;
        flag_value += search_terminators[pos];
        flag_value += "*";
    }
    GTEST_FLAG_SET(filter, flag_value);
}
#endif  // ARCH_AARCH64 || ARCH_X86_64

int main(int argc, char **argv) {
    ::testing::InitGoogleTest(&argc, argv);

#if ARCH_AARCH64
    const EbCpuFlags caps = svt_aom_get_cpu_flags_to_use();
    if (!(caps & EB_CPU_FLAGS_ARM_CRC32))
        append_negative_gtest_filter("ARM_CRC32");
    if (!(caps & EB_CPU_FLAGS_NEON_DOTPROD))
        append_negative_gtest_filter("NEON_DOTPROD");
    if (!(caps & EB_CPU_FLAGS_NEON_I8MM))
        append_negative_gtest_filter("NEON_I8MM");
    if (!(caps & EB_CPU_FLAGS_SVE))
        append_negative_gtest_filter("SVE");
    if (!(caps & EB_CPU_FLAGS_SVE2))
        append_negative_gtest_filter("SVE2");
#endif  // ARCH_AARCH64

#if ARCH_X86_64
    const EbCpuFlags simd_caps = svt_aom_get_cpu_flags_to_use();
    if (!(simd_caps & EB_CPU_FLAGS_MMX))
        append_negative_gtest_filter("MMX");
    if (!(simd_caps & EB_CPU_FLAGS_SSE))
        append_negative_gtest_filter("SSE");
    if (!(simd_caps & EB_CPU_FLAGS_SSE2))
        append_negative_gtest_filter("SSE2");
    if (!(simd_caps & EB_CPU_FLAGS_SSE3))
        append_negative_gtest_filter("SSE3");
    if (!(simd_caps & EB_CPU_FLAGS_SSSE3))
        append_negative_gtest_filter("SSSE3");
    if (!(simd_caps & EB_CPU_FLAGS_SSE4_1))
        append_negative_gtest_filter("SSE4_1");
    if (!(simd_caps & EB_CPU_FLAGS_SSE4_2))
        append_negative_gtest_filter("SSE4_2");
    if (!(simd_caps & EB_CPU_FLAGS_AVX))
        append_negative_gtest_filter("AVX");
    if (!(simd_caps & EB_CPU_FLAGS_AVX2))
        append_negative_gtest_filter("AVX2");
    if (!(simd_caps & EB_CPU_FLAGS_AVX512F))
        append_negative_gtest_filter("AVX512");
    if (!(simd_caps & EB_CPU_FLAGS_AVX512ICL))
        append_negative_gtest_filter("AVX512ICL");
#endif  // ARCH_X86_64

    return RUN_ALL_TESTS();
}
