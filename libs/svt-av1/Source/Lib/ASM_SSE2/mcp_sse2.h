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

#ifndef EBMCP_SSE2_H
#define EBMCP_SSE2_H

#include "definitions.h"

#ifdef __cplusplus
extern "C" {
#endif

/**************************************************
    * Assembly Declarations
    **************************************************/
void svt_picture_average_kernel_sse2(EbByte src0, uint32_t src0_stride, EbByte src1, uint32_t src1_stride, EbByte dst,
                                     uint32_t dst_stride, uint32_t area_width, uint32_t area_height);

#ifdef __cplusplus
}
#endif
#endif //EBMCP_SSE2_H
