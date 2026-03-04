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

#include "definitions.h"
#include <assert.h>
#include <math.h>
#include "svt_psnr.h"
#include "pic_buffer_desc.h"
#include "aom_dsp_rtcd.h"

static inline uint32_t variance(const uint8_t* a, int32_t a_stride, const uint8_t* b, int32_t b_stride, int32_t w,
                                int32_t h) {
    int      i, j;
    uint32_t sse = 0;

    for (i = 0; i < h; ++i) {
        for (j = 0; j < w; ++j) {
            sse += SQR(a[j] - b[j]);
        }

        a += a_stride;
        b += b_stride;
    }
    return sse;
}

uint32_t svt_aom_mse16x16_c(const uint8_t* src_ptr, int32_t source_stride, const uint8_t* ref_ptr,
                            int32_t recon_stride) {
    return variance(src_ptr, source_stride, ref_ptr, recon_stride, 16, 16);
}

int64_t svt_aom_get_sse(const uint8_t* a, int32_t a_stride, const uint8_t* b, int32_t b_stride, int32_t width,
                        int32_t height) {
    const int32_t dw        = width & 15;
    const int32_t dh        = height & 15;
    int64_t       total_sse = 0;
    uint32_t      sse       = 0;
    int32_t       x, y;

    if (dw > 0) {
        sse = variance(&a[width - dw], a_stride, &b[width - dw], b_stride, dw, height);
        total_sse += sse;
    }

    if (dh > 0) {
        sse = variance(&a[(height - dh) * a_stride], a_stride, &b[(height - dh) * b_stride], b_stride, width - dw, dh);
        total_sse += sse;
    }

    for (y = 0; y < height / 16; ++y) {
        const uint8_t* pa = a;
        const uint8_t* pb = b;
        for (x = 0; x < width / 16; ++x) {
            sse = svt_aom_mse16x16(pa, a_stride, pb, b_stride);

            total_sse += sse;

            pa += 16;
            pb += 16;
        }

        a += 16 * a_stride;
        b += 16 * b_stride;
    }

    return total_sse;
}

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
static inline int64_t highbd_variance(const uint8_t* a8, int32_t a_stride, const uint8_t* b8, int32_t b_stride,
                                      int32_t w, int32_t h) {
    const uint16_t* a   = CONVERT_TO_SHORTPTR(a8);
    const uint16_t* b   = CONVERT_TO_SHORTPTR(b8);
    int64_t         sse = 0;
    for (int32_t i = 0; i < h; ++i) {
        for (int32_t j = 0; j < w; ++j) {
            sse += (uint32_t)(SQR(a[j] - b[j]));
        }
        a += a_stride;
        b += b_stride;
    }
    return sse;
}

int64_t svt_aom_highbd_get_sse(const uint8_t* a, int32_t a_stride, const uint8_t* b, int32_t b_stride, int32_t width,
                               int32_t height) {
    int64_t       total_sse = 0;
    int32_t       x, y;
    const int32_t dw  = width % 16;
    const int32_t dh  = height % 16;
    uint32_t      sse = 0;
    if (dw > 0) {
        sse = (uint32_t)highbd_variance(&a[width - dw], a_stride, &b[width - dw], b_stride, dw, height);
        total_sse += sse;
    }
    if (dh > 0) {
        sse = (uint32_t)highbd_variance(
            &a[(height - dh) * a_stride], a_stride, &b[(height - dh) * b_stride], b_stride, width - dw, dh);
        total_sse += sse;
    }
    for (y = 0; y < height / 16; ++y) {
        const uint8_t* pa = a;
        const uint8_t* pb = b;
        for (x = 0; x < width / 16; ++x) {
            sse = svt_aom_highbd_mse16x16(pa, a_stride, pb, b_stride);

            total_sse += sse;
            pa += 16;
            pb += 16;
        }
        a += 16 * a_stride;
        b += 16 * b_stride;
    }
    return total_sse;
}

static void highbd_variance64(const uint8_t* a8, int a_stride, const uint8_t* b8, int b_stride, int w, int h,
                              uint64_t* sse, int64_t* sum) {
    const uint16_t* a    = CONVERT_TO_SHORTPTR(a8);
    const uint16_t* b    = CONVERT_TO_SHORTPTR(b8);
    int64_t         tsum = 0;
    uint64_t        tsse = 0;
    for (int i = 0; i < h; ++i) {
        int32_t lsum = 0;
        for (int j = 0; j < w; ++j) {
            const int diff = a[j] - b[j];
            lsum += diff;
            tsse += (uint32_t)(diff * diff);
        }
        tsum += lsum;
        a += a_stride;
        b += b_stride;
    }
    *sum = tsum;
    *sse = tsse;
}

static void highbd_10_variance(const uint8_t* a8, int a_stride, const uint8_t* b8, int b_stride, int w, int h,
                               uint32_t* sse, int* sum) {
    uint64_t sse_long = 0;
    int64_t  sum_long = 0;
    highbd_variance64(a8, a_stride, b8, b_stride, w, h, &sse_long, &sum_long);
    *sse = (uint32_t)ROUND_POWER_OF_TWO(sse_long, 4);
    *sum = (int)ROUND_POWER_OF_TWO(sum_long, 2);
}

#define HIGHBD_VAR(W, H)                                                                 \
    uint32_t svt_aom_highbd_10_variance##W##x##H##_c(                                    \
        const uint8_t* a, int a_stride, const uint8_t* b, int b_stride, uint32_t* sse) { \
        int     sum;                                                                     \
        int64_t var;                                                                     \
        highbd_10_variance(a, a_stride, b, b_stride, W, H, sse, &sum);                   \
        var = (int64_t)(*sse) - (((int64_t)sum * sum) / (W * H));                        \
        return (var >= 0) ? (uint32_t)var : 0;                                           \
    }

HIGHBD_VAR(128, 128)
HIGHBD_VAR(128, 64)
HIGHBD_VAR(64, 128)
HIGHBD_VAR(64, 64)
HIGHBD_VAR(64, 32)
HIGHBD_VAR(32, 64)
HIGHBD_VAR(32, 32)
HIGHBD_VAR(32, 16)
HIGHBD_VAR(16, 32)
HIGHBD_VAR(16, 16)
HIGHBD_VAR(16, 8)
HIGHBD_VAR(8, 16)
HIGHBD_VAR(8, 8)
HIGHBD_VAR(8, 4)
HIGHBD_VAR(4, 8)
HIGHBD_VAR(4, 4)
HIGHBD_VAR(4, 16)
HIGHBD_VAR(16, 4)
HIGHBD_VAR(8, 32)
HIGHBD_VAR(32, 8)
HIGHBD_VAR(16, 64)
HIGHBD_VAR(64, 16)
#endif

int64_t svt_aom_get_y_sse_part(const Yv12BufferConfig* a, const Yv12BufferConfig* b, int32_t hstart, int32_t width,
                               int32_t vstart, int32_t height) {
    return svt_aom_get_sse(a->y_buffer + vstart * a->y_stride + hstart,
                           a->y_stride,
                           b->y_buffer + vstart * b->y_stride + hstart,
                           b->y_stride,
                           width,
                           height);
}

int64_t svt_aom_get_u_sse_part(const Yv12BufferConfig* a, const Yv12BufferConfig* b, int32_t hstart, int32_t width,
                               int32_t vstart, int32_t height) {
    return svt_aom_get_sse(a->u_buffer + vstart * a->uv_stride + hstart,
                           a->uv_stride,
                           b->u_buffer + vstart * b->uv_stride + hstart,
                           b->uv_stride,
                           width,
                           height);
}

int64_t svt_aom_get_v_sse_part(const Yv12BufferConfig* a, const Yv12BufferConfig* b, int32_t hstart, int32_t width,
                               int32_t vstart, int32_t height) {
    return svt_aom_get_sse(a->v_buffer + vstart * a->uv_stride + hstart,
                           a->uv_stride,
                           b->v_buffer + vstart * b->uv_stride + hstart,
                           b->uv_stride,
                           width,
                           height);
}

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
int64_t svt_aom_highbd_get_y_sse_part(const Yv12BufferConfig* a, const Yv12BufferConfig* b, int32_t hstart,
                                      int32_t width, int32_t vstart, int32_t height) {
    return svt_aom_highbd_get_sse(a->y_buffer + vstart * a->y_stride + hstart,
                                  a->y_stride,
                                  b->y_buffer + vstart * b->y_stride + hstart,
                                  b->y_stride,
                                  width,
                                  height);
}

int64_t svt_aom_highbd_get_u_sse_part(const Yv12BufferConfig* a, const Yv12BufferConfig* b, int32_t hstart,
                                      int32_t width, int32_t vstart, int32_t height) {
    return svt_aom_highbd_get_sse(a->u_buffer + vstart * a->uv_stride + hstart,
                                  a->uv_stride,
                                  b->u_buffer + vstart * b->uv_stride + hstart,
                                  b->uv_stride,
                                  width,
                                  height);
}

int64_t svt_aom_highbd_get_v_sse_part(const Yv12BufferConfig* a, const Yv12BufferConfig* b, int32_t hstart,
                                      int32_t width, int32_t vstart, int32_t height) {
    return svt_aom_highbd_get_sse(a->v_buffer + vstart * a->uv_stride + hstart,
                                  a->uv_stride,
                                  b->v_buffer + vstart * b->uv_stride + hstart,
                                  b->uv_stride,
                                  width,
                                  height);
}
#endif
