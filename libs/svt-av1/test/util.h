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
 * @file util.h
 *
 * @brief common utils
 *
 * @author Cidana-Edmond
 *
 ******************************************************************************/
#ifndef _TEST_UTIL_H_
#define _TEST_UTIL_H_

#include <math.h>
#include <stdio.h>
#include "definitions.h"
#include "gtest/gtest.h"

// Macros
#ifndef PI
#define PI 3.141592653589793238462643383279502884f
#endif

#ifndef TEST_GET_PARAM
#define TEST_GET_PARAM(k) std::get<k>(this->GetParam())
#endif

#define ALIGNED_ADDR(T, alignment, buffer) \
    (T *)(((uintptr_t)buffer + (alignment - 1)) & ~(alignment - 1))

#define ALIGNMENT (64)

namespace svt_av1_test_tool {
INLINE int32_t round_shift(int64_t value, int32_t bit) {
    assert(bit >= 1);
    return (int32_t)((value + (1ll << (bit - 1))) >> bit);
}
}  // namespace svt_av1_test_tool

#endif  // _TEST_UTIL_H_
