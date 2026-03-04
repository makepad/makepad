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
#ifndef EbCdef_h
#define EbCdef_h

#include <stdint.h>
#include "definitions.h"

#ifdef __cplusplus
extern "C" {
#endif

#define CDEF_STRENGTH_BITS 6
#define CDEF_PRI_STRENGTHS 16
#define CDEF_SEC_STRENGTHS 4

#define CDEF_BLOCKSIZE 64
#define CDEF_BLOCKSIZE_LOG2 6
#define CDEF_NBLOCKS ((1 << MAX_SB_SIZE_LOG2) / 8)

/* We need to buffer three vertical lines. */
#define CDEF_VBORDER (3)
/* We only need to buffer three horizontal pixels too, but let's align to
16 bytes (8 x 16 bits) to make vectorization easier. */
#define CDEF_HBORDER (8)
#define CDEF_BSTRIDE ALIGN_POWER_OF_TWO((1 << MAX_SB_SIZE_LOG2) + 2 * CDEF_HBORDER, 3)
// Value is chosen so that memset can be used in cdef_seg_search().  Must be a large
// int16_t value.
#define CDEF_VERY_LARGE ((uint8_t)~0 >> 1 | ((uint8_t)~0 >> 1) << 8)
#define CDEF_INBUF_SIZE (CDEF_BSTRIDE * ((1 << MAX_SB_SIZE_LOG2) + 2 * CDEF_VBORDER))

extern const int32_t svt_aom_eb_cdef_pri_taps[2][2];
extern const int32_t svt_aom_eb_cdef_sec_taps[2][2];
extern const int (*const svt_aom_eb_cdef_directions)[2];

#define TOTAL_STRENGTHS (CDEF_PRI_STRENGTHS * CDEF_SEC_STRENGTHS)

void svt_aom_copy_sb8_16(uint16_t* dst, int32_t dstride, const uint8_t* src, int32_t src_voffset, int32_t src_hoffset,
                         int32_t sstride, int32_t vsize, int32_t hsize, bool is_16bit);

void svt_cdef_filter_fb(uint8_t* dst8, uint16_t* dst16, int32_t dstride, uint16_t* in, int32_t xdec, int32_t ydec,
                        uint8_t dir[CDEF_NBLOCKS][CDEF_NBLOCKS], int32_t* dirinit,
                        int32_t var[CDEF_NBLOCKS][CDEF_NBLOCKS], int32_t pli, CdefList* dlist, int32_t cdef_count,
                        int32_t level, int32_t sec_strength, int32_t pri_damping, int32_t sec_damping,
                        int32_t coeff_shift, uint8_t subsampling_factor);

#ifdef __cplusplus
}
#endif
#endif // EbCdef_h
