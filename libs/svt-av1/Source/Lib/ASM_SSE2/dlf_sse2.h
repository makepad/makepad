/*
* Copyright(c) 2019 Intel Corporation
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#ifndef EbDeblockingFilter_SSE2_h
#define EbDeblockingFilter_SSE2_h

#include "definitions.h"

#ifdef __cplusplus
extern "C" {
#endif

void svt_aom_highbd_lpf_horizontal_14_sse2(uint16_t* s, int32_t pitch, const uint8_t* blimit, const uint8_t* limit,
                                           const uint8_t* thresh, int32_t bd);

void svt_aom_highbd_lpf_horizontal_4_sse2(uint16_t* s, int32_t pitch, const uint8_t* blimit, const uint8_t* limit,
                                          const uint8_t* thresh, int32_t bd);

void svt_aom_highbd_lpf_horizontal_6_sse2(uint16_t* s, int32_t pitch, const uint8_t* blimit, const uint8_t* limit,
                                          const uint8_t* thresh, int32_t bd);

void svt_aom_highbd_lpf_horizontal_8_sse2(uint16_t* s, int32_t pitch, const uint8_t* blimit, const uint8_t* limit,
                                          const uint8_t* thresh, int32_t bd);

void svt_aom_highbd_lpf_vertical_14_sse2(uint16_t* s, int32_t pitch, const uint8_t* blimit, const uint8_t* limit,
                                         const uint8_t* thresh, int32_t bd);

void svt_aom_highbd_lpf_vertical_4_sse2(uint16_t* s, int32_t pitch, const uint8_t* blimit, const uint8_t* limit,
                                        const uint8_t* thresh, int32_t bd);

void svt_aom_highbd_lpf_vertical_6_sse2(uint16_t* s, int32_t pitch, const uint8_t* blimit, const uint8_t* limit,
                                        const uint8_t* thresh, int32_t bd);

void svt_aom_highbd_lpf_vertical_8_sse2(uint16_t* s, int32_t pitch, const uint8_t* blimit, const uint8_t* limit,
                                        const uint8_t* thresh, int32_t bd);

void svt_aom_lpf_horizontal_14_sse2(uint8_t* s, int32_t pitch, const uint8_t* blimit, const uint8_t* limit,
                                    const uint8_t* thresh);

void svt_aom_lpf_horizontal_4_sse2(uint8_t* s, int32_t pitch, const uint8_t* blimit, const uint8_t* limit,
                                   const uint8_t* thresh);

void svt_aom_lpf_horizontal_6_sse2(uint8_t* s, int32_t pitch, const uint8_t* blimit, const uint8_t* limit,
                                   const uint8_t* thresh);

void svt_aom_lpf_horizontal_8_sse2(uint8_t* s, int32_t pitch, const uint8_t* blimit, const uint8_t* limit,
                                   const uint8_t* thresh);

void svt_aom_lpf_vertical_14_sse2(uint8_t* s, int32_t pitch, const uint8_t* blimit, const uint8_t* limit,
                                  const uint8_t* thresh);

void svt_aom_lpf_vertical_4_sse2(uint8_t* s, int32_t pitch, const uint8_t* blimit, const uint8_t* limit,
                                 const uint8_t* thresh);

void svt_aom_lpf_vertical_6_sse2(uint8_t* s, int32_t pitch, const uint8_t* blimit, const uint8_t* limit,
                                 const uint8_t* thresh);

void svt_aom_lpf_vertical_8_sse2(uint8_t* s, int32_t pitch, const uint8_t* blimit, const uint8_t* limit,
                                 const uint8_t* thresh);

#ifdef __cplusplus
}
#endif
#endif
