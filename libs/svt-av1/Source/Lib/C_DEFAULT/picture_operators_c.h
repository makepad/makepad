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

#ifndef EbPictureOperators_C_h
#define EbPictureOperators_C_h

#include "definitions.h"
#ifdef __cplusplus
extern "C" {
#endif

void svt_aom_picture_copy_kernel(EbByte src, uint32_t src_stride, EbByte dst, uint32_t dst_stride, uint32_t area_width,
                                 uint32_t area_height, uint32_t bytes_per_sample);

#ifdef __cplusplus
}
#endif
#endif // EbPictureOperators_C_h
