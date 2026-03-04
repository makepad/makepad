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

#ifndef EbComputeMean_SS2_h
#define EbComputeMean_SS2_h
#ifdef __cplusplus
extern "C" {
#endif

#include "definitions.h"

uint64_t svt_aom_compute_subd_mean_of_squared_values8x8_sse2_intrin(
    uint8_t* input_samples, // input parameter, input samples Ptr
    uint16_t input_stride);

#ifdef __cplusplus
}
#endif

#endif
