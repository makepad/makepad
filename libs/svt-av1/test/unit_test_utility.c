/*
 * Copyright(c) 2019 Intel Corporation
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the
 * Alliance for Open Media Patent License 1.0 was not distributed with this
 * source code in the PATENTS file, you can obtain it at
 * https://www.aomedia.org/license/patent-license.
 */

// EbUnitTestUtility.c
//  -Unit test utility
//      -Randomize Data
//      -Compare Data
//      -Log Data
//      -Compute Overall Elapsed Millisecond

/***************************************
 * Includes
 ***************************************/
#include <time.h>
#ifdef _WIN32
#include <windows.h>
#else
#include <sys/time.h>
#endif

#include <assert.h>
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include "unit_test.h"
#include "unit_test_utility.h"
// #include "svt_time.h"

/***************************************
 * Randomize Data
 ***************************************/
void svt_buf_random_void(void *const buf, const uint32_t sizeBuf) {
    uint8_t *const buffer = (uint8_t *)buf;

    for (uint32_t i = 0; i < sizeBuf; i++) {
        buffer[i] = (uint8_t)(rand() % 256);
    }
}

void svt_buf_random_u8(uint8_t *const buf, const uint32_t sizeBuf) {
    svt_buf_random_void(buf, sizeBuf);
}

void svt_buf_random_u8_to_0_or_255(uint8_t *const buf, const uint32_t sizeBuf) {
    for (uint32_t i = 0; i < sizeBuf; i++)
        buf[i] = (rand() > (RAND_MAX >> 1)) ? 255 : 0;
}

void svt_buf_random_u8_to_255(uint8_t *const buf, const uint32_t sizeBuf) {
    memset(buf, 255, sizeBuf);
}

void svt_buf_random_s16(int16_t *const buf, const uint32_t sizeBuf) {
    svt_buf_random_void(buf, sizeof(*buf) * sizeBuf);
}

void svt_buf_random_u16(uint16_t *const buf, const uint32_t sizeBuf) {
    svt_buf_random_void(buf, sizeof(*buf) * sizeBuf);
}

void svt_buf_random_u16_to_0_or_bd(uint16_t *const buf, const uint32_t sizeBuf,
                                   const uint32_t bd) {
    const uint16_t max = (uint16_t)((1 << bd) - 1);

    for (uint32_t i = 0; i < sizeBuf; i++)
        buf[i] = (rand() > (RAND_MAX >> 1)) ? max : 0;
}

void svt_buf_random_u16_to_bd(uint16_t *const buf, const uint32_t sizeBuf,
                              const uint32_t bd) {
    const uint16_t max = (uint16_t)((1 << bd) - 1);

    for (uint32_t i = 0; i < sizeBuf; i++)
        buf[i] = max;
}

void svt_buf_random_u16_with_bd(uint16_t *const buf, const uint32_t sizeBuf,
                                const uint32_t bd) {
    assert(bd >= 8);

    for (uint32_t i = 0; i < sizeBuf; i++)
        buf[i] = (uint16_t)((uint32_t)rand() % (1 << bd));
}

void svt_buf_random_s32(int32_t *const buf, const uint32_t sizeBuf) {
    svt_buf_random_void(buf, sizeof(*buf) * sizeBuf);
}

void svt_buf_random_s32_with_max(int32_t *const buf, const uint32_t sizeBuf,
                                 const int32_t max_abs) {
    svt_buf_random_void(buf, sizeof(*buf) * sizeBuf);

    for (uint32_t i = 0; i < sizeBuf; i++) {
        buf[i] %= (max_abs + 1);
    }
}

void svt_buf_random_u32(uint32_t *const buf, const uint32_t sizeBuf) {
    svt_buf_random_void(buf, sizeof(*buf) * sizeBuf);
}

void svt_buf_random_u32_with_max(uint32_t *const buf, const uint32_t sizeBuf,
                                 const uint32_t max) {
    svt_buf_random_void(buf, sizeof(*buf) * sizeBuf);

    for (uint32_t i = 0; i < sizeBuf; i++)
        buf[i] %= (max + 1);
}

void svt_buf_random_s64(int64_t *const buf, const uint32_t sizeBuf) {
    svt_buf_random_void(buf, sizeof(*buf) * sizeBuf);
}

uint32_t svt_create_random_aligned_stride(const uint32_t width,
                                          const uint32_t align) {
    uint32_t stride;

    svt_buf_random_u32_with_max(&stride, 1, 100);
    stride += width + align;
    stride -= stride % align;
    return stride;
}

/***************************************
 * Compare Data
 ***************************************/
#define BUF_COMPARE_FUNCTION(name, type)                         \
    bool name(const type *const buf1,                            \
              const type *const buf2,                            \
              const size_t bufSize) {                            \
        bool result = true;                                      \
                                                                 \
        for (uint32_t i = 0; i < bufSize; i++) {                 \
            if (buf1[i] != buf2[i]) {                            \
                printf("\nbuf1[%3d] = 0x%8x\tbuf2[%3d] = 0x%8x", \
                       i,                                        \
                       buf1[i],                                  \
                       i,                                        \
                       buf2[i]);                                 \
                result = false;                                  \
            }                                                    \
        }                                                        \
                                                                 \
        if (!result)                                             \
            printf("\n\n");                                      \
                                                                 \
        return result;                                           \
    }

BUF_COMPARE_FUNCTION(svt_buf_compare_u16, uint16_t)
BUF_COMPARE_FUNCTION(svt_buf_compare_s16, int16_t)
BUF_COMPARE_FUNCTION(svt_buf_compare_u32, uint32_t)
BUF_COMPARE_FUNCTION(svt_buf_compare_s32, int32_t)
