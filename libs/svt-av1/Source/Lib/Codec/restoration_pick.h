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
#ifndef AV1_ENCODER_PICKRST_H_
#define AV1_ENCODER_PICKRST_H_

#ifdef __cplusplus
extern "C" {
#endif

#include "definitions.h"
#include "pic_buffer_desc.h"

static INLINE uint8_t find_average(const uint8_t* src, int32_t h_start, int32_t h_end, int32_t v_start, int32_t v_end,
                                   int32_t stride) {
    uint64_t sum = 0;
    for (int32_t i = v_start; i < v_end; i++) {
        for (int32_t j = h_start; j < h_end; j++) {
            sum += src[i * stride + j];
        }
    }
    uint64_t avg = sum / ((v_end - v_start) * (h_end - h_start));
    return (uint8_t)avg;
}

static INLINE uint16_t find_average_highbd(const uint16_t* src, int32_t h_start, int32_t h_end, int32_t v_start,
                                           int32_t v_end, int32_t stride) {
    uint64_t sum = 0;
    for (int32_t i = v_start; i < v_end; i++) {
        for (int32_t j = h_start; j < h_end; j++) {
            sum += src[i * stride + j];
        }
    }
    uint64_t avg = sum / ((v_end - v_start) * (h_end - h_start));
    return (uint16_t)avg;
}

#ifdef __cplusplus
} // extern "C"
#endif

#endif // AV1_ENCODER_PICKRST_H_
