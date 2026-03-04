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

#ifndef EbComputeSAD_h
#define EbComputeSAD_h

#include "definitions.h"
#include "aom_dsp_rtcd.h"
#include "compute_sad_c.h"
#include "utility.h"
#ifdef __cplusplus
extern "C" {
#endif

uint32_t svt_aom_sad_16b_kernel_c(uint16_t* src, // input parameter, source samples Ptr
                                  uint32_t  src_stride, // input parameter, source stride
                                  uint16_t* ref, // input parameter, reference samples Ptr
                                  uint32_t  ref_stride, // input parameter, reference stride
                                  uint32_t  height, // input parameter, block height (M)
                                  uint32_t  width); // input parameter, block width (N)

#ifdef __cplusplus
}
#endif
#endif // EbComputeSAD_h
