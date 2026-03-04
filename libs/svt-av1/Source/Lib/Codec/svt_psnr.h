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

#ifndef AOM_DSP_PSNR_H_
#define AOM_DSP_PSNR_H_

#include "pic_buffer_desc.h"
#include "definitions.h"
#include "utility.h"

#ifdef __cplusplus
extern "C" {
#endif

int64_t svt_aom_get_sse(const uint8_t* a, int32_t a_stride, const uint8_t* b, int32_t b_stride, int32_t width,
                        int32_t height);

int64_t svt_aom_get_y_sse_part(const Yv12BufferConfig* a, const Yv12BufferConfig* b, int32_t hstart, int32_t width,
                               int32_t vstart, int32_t height);

int64_t svt_aom_get_u_sse_part(const Yv12BufferConfig* a, const Yv12BufferConfig* b, int32_t hstart, int32_t width,
                               int32_t vstart, int32_t height);

int64_t svt_aom_get_v_sse_part(const Yv12BufferConfig* a, const Yv12BufferConfig* b, int32_t hstart, int32_t width,
                               int32_t vstart, int32_t height);

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
int64_t svt_aom_highbd_get_sse(const uint8_t* a, int32_t a_stride, const uint8_t* b, int32_t b_stride, int32_t width,
                               int32_t height);

int64_t svt_aom_highbd_get_y_sse_part(const Yv12BufferConfig* a, const Yv12BufferConfig* b, int32_t hstart,
                                      int32_t width, int32_t vstart, int32_t height);

int64_t svt_aom_highbd_get_u_sse_part(const Yv12BufferConfig* a, const Yv12BufferConfig* b, int32_t hstart,
                                      int32_t width, int32_t vstart, int32_t height);

int64_t svt_aom_highbd_get_v_sse_part(const Yv12BufferConfig* a, const Yv12BufferConfig* b, int32_t hstart,
                                      int32_t width, int32_t vstart, int32_t height);
#endif

#ifdef __cplusplus
} // extern "C"
#endif
#endif // AOM_DSP_PSNR_H_
