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
 * @file TestEnv.c
 *
 * @brief Environment setup for unit test
 *
 * @author Cidana-Edmond
 *
 ******************************************************************************/

#include "aom_dsp_rtcd.h"

/** setup_test_env is a util for unit test setup environment without create a
 * encoder */
void setup_test_env() {
#if defined(ARCH_X86_64) || defined(ARCH_AARCH64)
    EbCpuFlags cpu_flags = svt_aom_get_cpu_flags_to_use();
#else
    EbCpuFlags cpu_flags = 0;
#endif
    svt_aom_setup_common_rtcd_internal(cpu_flags);

    svt_aom_setup_rtcd_internal(cpu_flags);
}

void reset_test_env() {
    EbCpuFlags cpu_flags = 0;
    svt_aom_setup_common_rtcd_internal(cpu_flags);
    svt_aom_setup_rtcd_internal(cpu_flags);
}
