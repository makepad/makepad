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

#ifndef EbComputeSAD_C_h
#define EbComputeSAD_C_h

#include "definitions.h"
#ifdef __cplusplus
extern "C" {
#endif

void svt_sad_loop_kernel_c(uint8_t*  src, // input parameter, source samples Ptr
                           uint32_t  src_stride, // input parameter, source stride
                           uint8_t*  ref, // input parameter, reference samples Ptr
                           uint32_t  ref_stride, // input parameter, reference stride
                           uint32_t  block_height, // input parameter, block height (M)
                           uint32_t  block_width, // input parameter, block width (N)
                           uint64_t* best_sad, int16_t* x_search_center, int16_t* y_search_center,
                           uint32_t src_stride_raw, // input parameter, source stride (no line skipping)
                           uint8_t skip_search_line, int16_t search_area_width, int16_t search_area_height);

uint32_t svt_nxm_sad_kernel_helper_c(const uint8_t* src, uint32_t src_stride, const uint8_t* ref, uint32_t ref_stride,
                                     uint32_t height, uint32_t width);

#ifdef __cplusplus
}
#endif
#endif // EbComputeSAD_C_h
