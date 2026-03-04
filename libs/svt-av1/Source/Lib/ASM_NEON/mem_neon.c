/*
* Copyright(c) 2025 Meta Platforms, Inc. and affiliates.
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#include <arm_neon.h>

#include "common_dsp_rtcd.h"

static inline void copy_4xh(uint8_t* src, uint32_t src_stride, uint8_t* dst, uint32_t dst_stride, uint32_t height) {
    assert(height % 2 == 0);
    height >>= 1;

    do {
        *(uint32_t*)dst                = *(uint32_t*)src;
        *(uint32_t*)(dst + dst_stride) = *(uint32_t*)(src + src_stride);

        src += 2 * src_stride;
        dst += 2 * dst_stride;
    } while (--height != 0);
}

static inline void copy_8xh(uint8_t* src, uint32_t src_stride, uint8_t* dst, uint32_t dst_stride, uint32_t height) {
    assert(height % 2 == 0);
    height >>= 1;

    do {
        vst1_u8(dst, vld1_u8(src));
        vst1_u8(dst + dst_stride, vld1_u8(src + src_stride));

        src += src_stride * 2;
        dst += dst_stride * 2;
    } while (--height != 0);
}

static inline void copy_16xh(uint8_t* src, uint32_t src_stride, uint8_t* dst, uint32_t dst_stride, uint32_t height) {
    assert(height % 2 == 0);
    height >>= 1;

    do {
        vst1q_u8(dst, vld1q_u8(src));
        vst1q_u8(dst + dst_stride, vld1q_u8(src + src_stride));

        src += src_stride * 2;
        dst += dst_stride * 2;
    } while (--height != 0);
}

static inline void copy_16Xxh(uint8_t* src, uint32_t src_stride, uint8_t* dst, uint32_t dst_stride, uint32_t height,
                              uint32_t width) {
    assert(width % 16 == 0);

    do {
        for (uint32_t i = 0; i < width; i += 16) {
            vst1q_u8(dst + i, vld1q_u8(src + i));
        }

        src += src_stride;
        dst += dst_stride;
    } while (--height != 0);
}

void svt_av1_copy_wxh_8bit_neon(uint8_t* src, uint32_t src_stride, uint8_t* dst, uint32_t dst_stride, uint32_t height,
                                uint32_t width) {
    switch (width) {
    case 4:
        copy_4xh(src, src_stride, dst, dst_stride, height);
        break;
    case 8:
        copy_8xh(src, src_stride, dst, dst_stride, height);
        break;
    case 16:
        copy_16xh(src, src_stride, dst, dst_stride, height);
        break;
    case 32:
        copy_16Xxh(src, src_stride, dst, dst_stride, height, 32);
        break;
    case 64:
        copy_16Xxh(src, src_stride, dst, dst_stride, height, 64);
        break;
    default:
        for (uint32_t j = 0; j < height; j++) {
            svt_memcpy_c(dst + j * dst_stride, src + j * src_stride, width);
        }
    }
}

void svt_av1_copy_wxh_16bit_neon(uint16_t* src, uint32_t src_stride, uint16_t* dst, uint32_t dst_stride,
                                 uint32_t height, uint32_t width) {
    // 2x pixel size, so all byte functions are 2x too
    uint8_t* src8        = (uint8_t*)src;
    uint8_t* dst8        = (uint8_t*)dst;
    uint32_t src_stride8 = src_stride * 2;
    uint32_t dst_stride8 = dst_stride * 2;
    uint32_t width8      = width * 2;

    switch (width) {
    case 4:
        copy_8xh(src8, src_stride8, dst8, dst_stride8, height);
        break;
    case 8:
        copy_16xh(src8, src_stride8, dst8, dst_stride8, height);
        break;
    case 16:
        copy_16Xxh(src8, src_stride8, dst8, dst_stride8, height, 16 * 2);
        break;
    case 32:
        copy_16Xxh(src8, src_stride8, dst8, dst_stride8, height, 32 * 2);
        break;
    // 64 pixels (ie 128 bytes) copy has ~same perf as system
    default:
        for (uint32_t j = 0; j < height; j++) {
            svt_memcpy_c(dst8 + j * dst_stride8, src8 + j * src_stride8, width8);
        }
    }
}

void svt_memcpy_neon(void* dst_ptr, void const* src_ptr, size_t size) {
    const uint8_t* src = src_ptr;
    uint8_t*       dst = dst_ptr;
    size_t         i   = 0;

    while (i + 32 <= size) {
        vst1q_u8(dst + i, vld1q_u8(src + i));
        vst1q_u8(dst + i + 16, vld1q_u8(src + i + 16));
        i += 32;
    }

    if (i + 16 <= size) {
        vst1q_u8(dst + i, vld1q_u8(src + i));
        i += 16;
    }

    if (i + 8 <= size) {
        vst1_u8(dst + i, vld1_u8(src + i));
        i += 8;
    }

    for (; i < size; ++i) {
        dst[i] = src[i];
    }
}

void svt_memset_neon(void* dst_ptr, int c, size_t size) {
    uint8_t* dst = dst_ptr;
    size_t   i   = 0;

    uint8x16_t vec = vdupq_n_u8(c);

    while (i + 32 <= size) {
        vst1q_u8(dst + i, vec);
        vst1q_u8(dst + i + 16, vec);
        i += 32;
    }

    if (i + 16 <= size) {
        vst1q_u8(dst + i, vec);
        i += 16;
    }

    if (i + 8 <= size) {
        vst1_u8(dst + i, vget_low_u8(vec));
        i += 8;
    }

    for (; i < size; ++i) {
        dst[i] = c;
    }
}
