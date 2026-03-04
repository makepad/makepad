/*
* Copyright(c) 2022 Intel Corporation
*
* This source code is subject to the terms of the BSD 3-Clause Clear License and
* the Alliance for Open Media Patent License 1.0. If the BSD 3-Clause Clear License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/
#ifndef AV1_COMMON_MC_H_
#define AV1_COMMON_MC_H_
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

//Functions defined in mc_avx2.asm and mc16_avx2.asm
void svt_dav1d_blend_v_8bpc_avx2(uint8_t* dst, ptrdiff_t dst_stride, const uint8_t* tmp, ptrdiff_t tmp_stride, int w,
                                 int h);
void svt_dav1d_blend_h_8bpc_avx2(uint8_t* dst, ptrdiff_t dst_stride, const uint8_t* tmp, ptrdiff_t tmp_stride, int w,
                                 int h);
void svt_dav1d_blend_v_16bpc_avx2(uint16_t* dst, ptrdiff_t dst_stride, const uint16_t* tmp, ptrdiff_t tmp_stride, int w,
                                  int h);
void svt_dav1d_blend_h_16bpc_avx2(uint16_t* dst, ptrdiff_t dst_stride, const uint16_t* tmp, ptrdiff_t tmp_stride, int w,
                                  int h);

#ifdef __cplusplus
}
#endif

#endif // AV1_COMMON_MC_H_
