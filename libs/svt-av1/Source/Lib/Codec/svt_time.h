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

#ifndef EbTime_h
#define EbTime_h

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

double svt_av1_compute_overall_elapsed_time_ms(const uint64_t start_seconds, const uint64_t start_useconds,
                                               const uint64_t finish_seconds, const uint64_t finish_useconds);
void   svt_av1_get_time(uint64_t* const seconds, uint64_t* const useconds);

#ifdef __cplusplus
}
#endif

#endif // EbTime_h
/* File EOF */
