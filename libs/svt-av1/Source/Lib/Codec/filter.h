/*
 * Copyright (c) 2016, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
 */

#ifndef AV1_COMMON_FILTER_H_
#define AV1_COMMON_FILTER_H_

#include <assert.h>

#ifdef __cplusplus
extern "C" {
#endif

//---aom_filter.h
#define FILTER_BITS 7

#define SUBPEL_BITS 4
#define SUBPEL_MASK ((1 << SUBPEL_BITS) - 1)
#define SUBPEL_SHIFTS (1 << SUBPEL_BITS)
#define SUBPEL_TAPS 8

#define SCALE_SUBPEL_BITS 10
#define SCALE_SUBPEL_SHIFTS (1 << SCALE_SUBPEL_BITS)
#define SCALE_SUBPEL_MASK (SCALE_SUBPEL_SHIFTS - 1)
#define SCALE_EXTRA_BITS (SCALE_SUBPEL_BITS - SUBPEL_BITS)
#define SCALE_EXTRA_OFF ((1 << SCALE_EXTRA_BITS) / 2)

#define BIL_SUBPEL_BITS 3
#define BIL_SUBPEL_SHIFTS (1 << BIL_SUBPEL_BITS)

// 2 tap bilinear filters
static const uint8_t bilinear_filters_2t[BIL_SUBPEL_SHIFTS][2] = {
    {128, 0},
    {112, 16},
    {96, 32},
    {80, 48},
    {64, 64},
    {48, 80},
    {32, 96},
    {16, 112},
};
//----
#define MAX_FILTER_TAP 8

// With CONFIG_DUAL_FILTER, pack two InterpFilter's into a uint32_t: since
// there are at most 10 filters, we can use 16 bits for each and have more than
// enough space. This reduces argument passing and unifies the operation of
// setting a (pair of) filters.
//
// Without CONFIG_DUAL_FILTER,
typedef uint32_t InterpFilters;

static INLINE InterpFilter av1_extract_interp_filter(InterpFilters filters, int32_t x_filter) {
    return (InterpFilter)((filters >> (x_filter ? 16 : 0)) & 0xffff);
}

static INLINE InterpFilters av1_make_interp_filters(InterpFilter y_filter, InterpFilter x_filter) {
    uint16_t y16 = y_filter & 0xffff;
    uint16_t x16 = x_filter & 0xffff;
    return y16 | ((uint32_t)x16 << 16);
}

static INLINE InterpFilters av1_broadcast_interp_filter(InterpFilter filter) {
    return av1_make_interp_filters(filter, filter);
}

#define INTER_FILTER_COMP_OFFSET (SWITCHABLE_FILTERS + 1)
#define INTER_FILTER_DIR_OFFSET ((SWITCHABLE_FILTERS + 1) * 2)

static INLINE const int16_t* av1_get_interp_filter_subpel_kernel(const InterpFilterParams filter_params,
                                                                 const int32_t            subpel) {
    return filter_params.filter_ptr + filter_params.taps * subpel;
}

#ifdef __cplusplus
} // extern "C"
#endif

#endif // AV1_COMMON_FILTER_H_
