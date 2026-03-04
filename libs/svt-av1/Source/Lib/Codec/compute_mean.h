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

#ifndef EbComputeMean_h
#define EbComputeMean_h

#include "definitions.h"

#ifdef __cplusplus
extern "C" {
#endif

uint64_t svt_compute_mean_c(uint8_t* input_samples, /**< input parameter, input samples Ptr */
                            uint32_t input_stride, /**< input parameter, input stride */
                            uint32_t input_area_width, /**< input parameter, input area width */
                            uint32_t input_area_height); /**< input parameter, input area height */

uint64_t svt_compute_mean_squared_values_c(uint8_t* input_samples, /**< input parameter, input samples Ptr */
                                           uint32_t input_stride, /**< input parameter, input stride */
                                           uint32_t input_area_width, /**< input parameter, input area width */
                                           uint32_t input_area_height); /**< input parameter, input area height */

uint64_t svt_compute_sub_mean_8x8_c(uint8_t* input_samples, /**< input parameter, input samples Ptr */
                                    uint16_t input_stride); /**< input parameter, input stride */

uint64_t svt_aom_compute_sub_mean_squared_values_c(
    uint8_t* input_samples, /**< input parameter, input samples Ptr */
    uint32_t input_stride, /**< input parameter, input stride */
    uint32_t input_area_width, /**< input parameter, input area width */
    uint32_t input_area_height); /**< input parameter, input area height */

void svt_compute_interm_var_four8x8_c(uint8_t* input_samples, uint16_t input_stride,
                                      uint64_t* mean_of8x8_blocks, // mean of four  8x8
                                      uint64_t* mean_of_squared8x8_blocks); // meanSquared

#ifdef __cplusplus
}
#endif

#endif
