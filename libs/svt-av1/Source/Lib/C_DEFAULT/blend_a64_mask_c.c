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

//#include "utility.h"
#include "definitions.h"

void svt_aom_highbd_blend_a64_vmask_16bit_c(uint16_t* dst, uint32_t dst_stride, const uint16_t* src0,
                                            uint32_t src0_stride, const uint16_t* src1, uint32_t src1_stride,
                                            const uint8_t* mask, int w, int h, int bd) {
    (void)bd;
    int i, j;

    assert(IMPLIES(src0 == dst, src0_stride == dst_stride));
    assert(IMPLIES(src1 == dst, src1_stride == dst_stride));

    assert(h >= 1);
    assert(w >= 1);
    assert(IS_POWER_OF_TWO(h));
    assert(IS_POWER_OF_TWO(w));

    assert(bd == 8 || bd == 10 || bd == 12);

    for (i = 0; i < h; ++i) {
        const int m = mask[i];
        for (j = 0; j < w; ++j) {
            dst[i * dst_stride + j] = AOM_BLEND_A64(m, src0[i * src0_stride + j], src1[i * src1_stride + j]);
        }
    }
}
