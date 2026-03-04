// clang-format off
/*
* Copyright(c) 2019 Intel Corporation
* Copyright (c) 2016, Alliance for Open Media. All rights reserved
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

// This file is generated. Do not edit.
#ifndef AOM_DSP_RTCD_H_
#define AOM_DSP_RTCD_H_

#include "common_dsp_rtcd.h"
#include "definitions.h"
#include "coding_unit.h"

#include "noise_model.h"

#undef RTCD_EXTERN
#ifdef AOM_RTCD_C
#define RTCD_EXTERN                //CHKN RTCD call in effect. declare the function pointers in  encHandle.
#else
#define RTCD_EXTERN extern         //CHKN run time externing the function pointers.
#endif

 /**************************************
 * Instruction Set Support
 **************************************/

#ifdef _WIN32
# include <intrin.h>
#endif


#ifdef __cplusplus
extern "C" {
#endif

// Helper Functions
void svt_aom_setup_rtcd_internal(EbCpuFlags flags);

int64_t svt_aom_sse_c(const uint8_t *a, int a_stride, const uint8_t *b, int b_stride, int width, int height);
RTCD_EXTERN int64_t(*svt_aom_sse)(const uint8_t *a, int a_stride, const uint8_t *b, int b_stride, int width, int height);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
int64_t svt_aom_highbd_sse_c(const uint8_t *a8, int a_stride, const uint8_t *b8, int b_stride, int width, int height);
RTCD_EXTERN int64_t(*svt_aom_highbd_sse)(const uint8_t *a8, int a_stride, const uint8_t *b8, int b_stride, int width, int height);
#endif
uint32_t svt_av1_get_crc32c_value_c(void *c, const uint8_t *buf, size_t len);
RTCD_EXTERN uint32_t(*svt_av1_get_crc32c_value)(void *c, const uint8_t *buf, size_t len);
void svt_av1_wedge_compute_delta_squares_c(int16_t *d, const int16_t *a, const int16_t *b, int N);
RTCD_EXTERN void(*svt_av1_wedge_compute_delta_squares)(int16_t *d, const int16_t *a, const int16_t *b, int N);
int8_t svt_av1_wedge_sign_from_residuals_c(const int16_t *ds, const uint8_t *m, int N, int64_t limit);
RTCD_EXTERN int8_t(*svt_av1_wedge_sign_from_residuals)(const int16_t *ds, const uint8_t *m, int N, int64_t limit);
uint64_t svt_aom_compute_cdef_dist_16bit_c(const uint16_t* dst, int32_t dstride, const uint16_t* src, const CdefList* dlist, int32_t cdef_count, BlockSize bsize, int32_t coeff_shift, uint8_t subsampling_factor);
RTCD_EXTERN uint64_t(*svt_compute_cdef_dist_16bit)(const uint16_t* dst, int32_t dstride, const uint16_t* src, const CdefList* dlist, int32_t cdef_count, BlockSize bsize, int32_t coeff_shift, uint8_t subsampling_factor);
uint64_t svt_aom_compute_cdef_dist_8bit_c(const uint8_t* dst8, int32_t dstride, const uint8_t* src8, const CdefList* dlist, int32_t cdef_count, BlockSize bsize, int32_t coeff_shift, uint8_t subsampling_factor);
RTCD_EXTERN uint64_t(*svt_compute_cdef_dist_8bit)(const uint8_t* dst8, int32_t dstride, const uint8_t* src8, const CdefList* dlist, int32_t cdef_count, BlockSize bsize, int32_t coeff_shift, uint8_t subsampling_factor);
void svt_av1_compute_stats_c(int32_t wiener_win, const uint8_t *dgd8, const uint8_t *src8, int32_t h_start, int32_t h_end, int32_t v_start, int32_t v_end, int32_t dgd_stride, int32_t src_stride, int64_t *M, int64_t *H);
RTCD_EXTERN void(*svt_av1_compute_stats)(int32_t wiener_win, const uint8_t *dgd8, const uint8_t *src8, int32_t h_start, int32_t h_end, int32_t v_start, int32_t v_end, int32_t dgd_stride, int32_t src_stride, int64_t *M, int64_t *H);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_av1_compute_stats_highbd_c(int32_t wiener_win, const uint8_t *dgd8, const uint8_t *src8, int32_t h_start, int32_t h_end, int32_t v_start, int32_t v_end, int32_t dgd_stride, int32_t src_stride, int64_t *M, int64_t *H, EbBitDepth bit_depth);
RTCD_EXTERN void(*svt_av1_compute_stats_highbd)(int32_t wiener_win, const uint8_t *dgd8, const uint8_t *src8, int32_t h_start, int32_t h_end, int32_t v_start, int32_t v_end, int32_t dgd_stride, int32_t src_stride, int64_t *M, int64_t *H, EbBitDepth bit_depth);
#endif
typedef uint64_t(*EbSpatialFullDistType)(
    uint8_t   *input,
    uint32_t   input_offset,
    uint32_t   input_stride,
    uint8_t   *recon,
    int32_t    recon_offset,
    uint32_t   recon_stride,
    uint32_t   area_width,
    uint32_t   area_height);
int64_t svt_av1_lowbd_pixel_proj_error_c(const uint8_t *src8, int32_t width, int32_t height, int32_t src_stride, const uint8_t *dat8, int32_t dat_stride, int32_t *flt0, int32_t flt0_stride, int32_t *flt1, int32_t flt1_stride, const int32_t xq[2], const SgrParamsType *params);
RTCD_EXTERN int64_t(*svt_av1_lowbd_pixel_proj_error)(const uint8_t *src8, int32_t width, int32_t height, int32_t src_stride, const uint8_t *dat8, int32_t dat_stride, int32_t *flt0, int32_t flt0_stride, int32_t *flt1, int32_t flt1_stride, const int32_t xq[2], const SgrParamsType *params);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
int64_t svt_av1_highbd_pixel_proj_error_c(const uint8_t *src8, int32_t width, int32_t height, int32_t src_stride, const uint8_t *dat8, int32_t dat_stride, int32_t *flt0, int32_t flt0_stride, int32_t *flt1, int32_t flt1_stride, const int32_t xq[2], const SgrParamsType *params);
RTCD_EXTERN int64_t(*svt_av1_highbd_pixel_proj_error)(const uint8_t *src8, int32_t width, int32_t height, int32_t src_stride, const uint8_t *dat8, int32_t dat_stride, int32_t *flt0, int32_t flt0_stride, int32_t *flt1, int32_t flt1_stride, const int32_t xq[2], const SgrParamsType *params);
#endif
void svt_subtract_average_c(int16_t *pred_buf_q3, int32_t width, int32_t height, int32_t round_offset, int32_t num_pel_log2);
RTCD_EXTERN void(*svt_subtract_average)(int16_t *pred_buf_q3, int32_t width, int32_t height, int32_t round_offset, int32_t num_pel_log2);
void svt_av1_fwd_txfm2d_4x16_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_4x16)(int16_t *input, int32_t *output, uint32_t inputStride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x4_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_16x4)(int16_t *input, int32_t *output, uint32_t inputStride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_4x8_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_4x8)(int16_t *input, int32_t *output, uint32_t inputStride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x4_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_8x4)(int16_t *input, int32_t *output, uint32_t inputStride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x16_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_8x16)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x8_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_16x8)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_4x16_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_4x16)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x4_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_16x4)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_4x8_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_4x8)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x4_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_8x4)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x16_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_32x16)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x8_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_32x8)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x32_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_8x32)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x32_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_16x32)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x64_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_32x64)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_64x32_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_64x32)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x64_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_16x64)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_64x16_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_64x16)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_transform_two_d_64x64_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_64x64)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_transform_two_d_32x32_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_32x32)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_transform_two_d_16x16_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_16x16)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_transform_two_d_8x8_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_8x8)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_transform_two_d_4x4_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_4x4)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x16_N2_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_8x16_N2)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x8_N2_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_16x8_N2)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_4x16_N2_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_4x16_N2)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x4_N2_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_16x4_N2)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_4x8_N2_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_4x8_N2)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x4_N2_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_8x4_N2)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x16_N2_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_32x16_N2)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x8_N2_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_32x8_N2)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x32_N2_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_8x32_N2)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x32_N2_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_16x32_N2)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x64_N2_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_32x64_N2)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_64x32_N2_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_64x32_N2)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x64_N2_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_16x64_N2)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_64x16_N2_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_64x16_N2)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_aom_transform_two_d_64x64_N2_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_64x64_N2)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_aom_transform_two_d_32x32_N2_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_32x32_N2)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_aom_transform_two_d_16x16_N2_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_16x16_N2)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_aom_transform_two_d_8x8_N2_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_8x8_N2)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_aom_transform_two_d_4x4_N2_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_4x4_N2)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x16_N4_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_8x16_N4)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x8_N4_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_16x8_N4)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_4x16_N4_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_4x16_N4)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x4_N4_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_16x4_N4)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_4x8_N4_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_4x8_N4)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x4_N4_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_8x4_N4)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x16_N4_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_32x16_N4)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x8_N4_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_32x8_N4)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x32_N4_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_8x32_N4)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x32_N4_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_16x32_N4)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x64_N4_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_32x64_N4)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_64x32_N4_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_64x32_N4)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x64_N4_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_16x64_N4)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_64x16_N4_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_64x16_N4)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_aom_transform_two_d_64x64_N4_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_64x64_N4)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_aom_transform_two_d_32x32_N4_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_32x32_N4)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_aom_transform_two_d_16x16_N4_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_16x16_N4)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_aom_transform_two_d_8x8_N4_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_8x8_N4)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_aom_transform_two_d_4x4_N4_c(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
RTCD_EXTERN void(*svt_av1_fwd_txfm2d_4x4_N4)(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwht4x4_c(int16_t* input, int32_t* output, uint32_t stride);
void svt_av1_fwht4x4_sse4_1(int16_t* input, int32_t* output, uint32_t stride);
RTCD_EXTERN void (*svt_av1_fwht4x4)(int16_t* input, int32_t* output, uint32_t stride);
int svt_aom_satd_c(const TranLow *coeff, int length);
RTCD_EXTERN int(*svt_aom_satd)(const TranLow *coeff, int length);
int64_t svt_av1_block_error_c(const TranLow *coeff, const TranLow *dqcoeff, intptr_t block_size, int64_t *ssz);
RTCD_EXTERN int64_t(*svt_av1_block_error)(const TranLow *coeff, const TranLow *dqcoeff, intptr_t block_size, int64_t *ssz);
void svt_get_proj_subspace_c(const uint8_t *src8, int width, int height, int src_stride, const uint8_t *dat8, int dat_stride, int use_highbitdepth, int32_t *flt0, int flt0_stride, int32_t *flt1, int flt1_stride, int *xq, const SgrParamsType *params);
RTCD_EXTERN void(*svt_get_proj_subspace)(const uint8_t *src8, int width, int height, int src_stride, const uint8_t *dat8, int dat_stride, int use_highbitdepth, int32_t *flt0, int flt0_stride, int32_t *flt1, int flt1_stride, int *xq, const SgrParamsType *params);
uint64_t svt_handle_transform16x64_c(int32_t *output);
RTCD_EXTERN uint64_t(*svt_handle_transform16x64)(int32_t *output);
uint64_t svt_handle_transform32x64_c(int32_t *output);
RTCD_EXTERN uint64_t(*svt_handle_transform32x64)(int32_t *output);
uint64_t svt_handle_transform64x16_c(int32_t *output);
RTCD_EXTERN uint64_t(*svt_handle_transform64x16)(int32_t *output);
uint64_t svt_handle_transform64x32_c(int32_t *output);
RTCD_EXTERN uint64_t(*svt_handle_transform64x32)(int32_t *output);
uint64_t svt_handle_transform64x64_c(int32_t *output);
RTCD_EXTERN uint64_t(*svt_handle_transform64x64)(int32_t *output);
uint64_t svt_handle_transform16x64_N2_N4_c(int32_t *output);
uint64_t svt_handle_transform16x64_N2_N4_avx2(int32_t *output);
RTCD_EXTERN uint64_t(*svt_handle_transform16x64_N2_N4)(int32_t *output);
uint64_t svt_handle_transform32x64_N2_N4_c(int32_t *output);
uint64_t svt_handle_transform32x64_N2_N4_avx2(int32_t *output);
RTCD_EXTERN uint64_t(*svt_handle_transform32x64_N2_N4)(int32_t *output);
uint64_t svt_handle_transform64x16_N2_N4_c(int32_t *output);
uint64_t svt_handle_transform64x16_N2_N4_avx2(int32_t *output);
RTCD_EXTERN uint64_t(*svt_handle_transform64x16_N2_N4)(int32_t *output);
uint64_t svt_handle_transform64x32_N2_N4_c(int32_t *output);
uint64_t svt_handle_transform64x32_N2_N4_avx2(int32_t *output);
RTCD_EXTERN uint64_t(*svt_handle_transform64x32_N2_N4)(int32_t *output);
uint64_t svt_handle_transform64x64_N2_N4_c(int32_t *output);
uint64_t svt_handle_transform64x64_N2_N4_avx2(int32_t *output);
RTCD_EXTERN uint64_t(*svt_handle_transform64x64_N2_N4)(int32_t *output);
uint64_t svt_search_one_dual_c(int *lev0, int *lev1, int nb_strengths, uint64_t** mse[2], int sb_count, int start_gi, int end_gi);
RTCD_EXTERN uint64_t(*svt_search_one_dual)(int *lev0, int *lev1, int nb_strengths, uint64_t** mse[2], int sb_count, int start_gi, int end_gi);
uint32_t svt_aom_mse16x16_c(const uint8_t *src_ptr, int32_t  source_stride, const uint8_t *ref_ptr, int32_t  recon_stride);
RTCD_EXTERN uint32_t(*svt_aom_mse16x16)(const uint8_t *src_ptr, int32_t  source_stride, const uint8_t *ref_ptr, int32_t  recon_stride);
void svt_aom_quantize_b_c(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan, const QmVal *qm_ptr, const QmVal *iqm_ptr, const int32_t log_scale);
RTCD_EXTERN void(*svt_aom_quantize_b)(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr,const int16_t *quant_ptr, const int16_t *quant_shift_ptr,TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr,uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan,const QmVal *qm_ptr, const QmVal *iqm_ptr, const int32_t log_scale);
RTCD_EXTERN void(*svt_av1_quantize_b_qm)(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr,const int16_t *quant_ptr, const int16_t *quant_shift_ptr,TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr,uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan,const QmVal *qm_ptr, const QmVal *iqm_ptr, const int32_t log_scale);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
RTCD_EXTERN void(*svt_av1_highbd_quantize_b_qm)(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan, const QmVal *qm_ptr, const QmVal *iqm_ptr, const int32_t log_scale);
void svt_aom_highbd_quantize_b_c(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan, const QmVal *qm_ptr, const QmVal *iqm_ptr, const int32_t log_scale);
RTCD_EXTERN void(*svt_aom_highbd_quantize_b)(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan, const QmVal *qm_ptr, const QmVal *iqm_ptr, const int32_t log_scale);
#endif
void svt_av1_quantize_fp_c(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan);
RTCD_EXTERN void(*svt_av1_quantize_fp)(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan);
void svt_av1_quantize_fp_qm_c(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan, const QmVal *qm_ptr, const QmVal *iqm_ptr, int16_t log_scale);
RTCD_EXTERN void(*svt_av1_quantize_fp_qm)(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan, const QmVal *qm_ptr, const QmVal *iqm_ptr, int16_t log_scale);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_av1_highbd_quantize_fp_c(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan, int16_t log_scale);
RTCD_EXTERN void(*svt_av1_highbd_quantize_fp)(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan, int16_t log_scale);
#endif
void svt_av1_quantize_fp_32x32_c(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan);
RTCD_EXTERN void(*svt_av1_quantize_fp_32x32)(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan);
void svt_av1_quantize_fp_64x64_c(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan);
RTCD_EXTERN void(*svt_av1_quantize_fp_64x64)(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_av1_highbd_quantize_fp_qm_c(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan, const QmVal *qm_ptr, const QmVal *iqm_ptr, int16_t log_scale);
RTCD_EXTERN void(*svt_av1_highbd_quantize_fp_qm)(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan, const QmVal *qm_ptr, const QmVal *iqm_ptr, int16_t log_scale);
uint32_t svt_aom_highbd_mse16x16_c(const uint8_t *src_ptr, int32_t source_stride, const uint8_t *ref_ptr, int32_t recon_stride);
RTCD_EXTERN uint32_t(*svt_aom_highbd_mse16x16)(const uint8_t *src_ptr, int32_t source_stride, const uint8_t *ref_ptr, int32_t recon_stride);
#endif
uint32_t svt_aom_sad128x128_c(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
RTCD_EXTERN uint32_t(*svt_aom_sad128x128)(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
void svt_aom_sad128x128x4d_c(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
RTCD_EXTERN void(*svt_aom_sad128x128x4d)(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
uint32_t svt_aom_sad128x64_c(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
RTCD_EXTERN uint32_t(*svt_aom_sad128x64)(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
void svt_aom_sad128x64x4d_c(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
RTCD_EXTERN void(*svt_aom_sad128x64x4d)(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
uint32_t svt_aom_sad16x16_c(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
RTCD_EXTERN uint32_t(*svt_aom_sad16x16)(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
void svt_aom_sad16x16x4d_c(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
RTCD_EXTERN void(*svt_aom_sad16x16x4d)(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
uint32_t svt_aom_sad16x32_c(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
RTCD_EXTERN uint32_t(*svt_aom_sad16x32)(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
void svt_aom_sad16x32x4d_c(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
RTCD_EXTERN void(*svt_aom_sad16x32x4d)(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
uint32_t svt_aom_sad16x4_c(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
RTCD_EXTERN uint32_t(*svt_aom_sad16x4)(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
void svt_aom_sad16x4x4d_c(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
RTCD_EXTERN void(*svt_aom_sad16x4x4d)(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
uint32_t svt_aom_sad16x64_c(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
RTCD_EXTERN uint32_t(*svt_aom_sad16x64)(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
void svt_aom_sad16x64x4d_c(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
RTCD_EXTERN void(*svt_aom_sad16x64x4d)(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
uint32_t svt_aom_sad16x8_c(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
RTCD_EXTERN uint32_t(*svt_aom_sad16x8)(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
void svt_aom_sad16x8x4d_c(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
RTCD_EXTERN void(*svt_aom_sad16x8x4d)(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
uint32_t svt_aom_sad32x16_c(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
RTCD_EXTERN uint32_t(*svt_aom_sad32x16)(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
void svt_aom_sad32x16x4d_c(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
RTCD_EXTERN void(*svt_aom_sad32x16x4d)(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
uint32_t svt_aom_sad32x32_c(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
RTCD_EXTERN uint32_t(*svt_aom_sad32x32)(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
void svt_aom_sad32x32x4d_c(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
RTCD_EXTERN void(*svt_aom_sad32x32x4d)(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
uint32_t svt_aom_sad32x64_c(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
RTCD_EXTERN uint32_t(*svt_aom_sad32x64)(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
void svt_aom_sad32x64x4d_c(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
RTCD_EXTERN void(*svt_aom_sad32x64x4d)(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
uint32_t svt_aom_sad32x8_c(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
RTCD_EXTERN uint32_t(*svt_aom_sad32x8)(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
void svt_aom_sad32x8x4d_c(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
RTCD_EXTERN void(*svt_aom_sad32x8x4d)(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
uint32_t svt_aom_sad4x16_c(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
RTCD_EXTERN uint32_t(*svt_aom_sad4x16)(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
void svt_aom_sad4x16x4d_c(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
RTCD_EXTERN void(*svt_aom_sad4x16x4d)(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
uint32_t svt_aom_sad4x4_c(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
RTCD_EXTERN uint32_t(*svt_aom_sad4x4)(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
void svt_aom_sad4x4x4d_c(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
RTCD_EXTERN void(*svt_aom_sad4x4x4d)(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
uint32_t svt_aom_sad4x8_c(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
RTCD_EXTERN uint32_t(*svt_aom_sad4x8)(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
void svt_aom_sad4x8x4d_c(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
RTCD_EXTERN void(*svt_aom_sad4x8x4d)(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
uint32_t svt_aom_sad64x128_c(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
RTCD_EXTERN uint32_t(*svt_aom_sad64x128)(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
void svt_aom_sad64x128x4d_c(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
RTCD_EXTERN void(*svt_aom_sad64x128x4d)(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
uint32_t svt_aom_sad64x16_c(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
RTCD_EXTERN uint32_t(*svt_aom_sad64x16)(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
void svt_aom_sad64x16x4d_c(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
RTCD_EXTERN void(*svt_aom_sad64x16x4d)(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
uint32_t svt_aom_sad64x32_c(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
RTCD_EXTERN uint32_t(*svt_aom_sad64x32)(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
void svt_aom_sad64x32x4d_c(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
RTCD_EXTERN void(*svt_aom_sad64x32x4d)(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
uint32_t svt_aom_sad64x64_c(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
RTCD_EXTERN uint32_t(*svt_aom_sad64x64)(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
void svt_aom_sad64x64x4d_c(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
RTCD_EXTERN void(*svt_aom_sad64x64x4d)(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
uint32_t svt_aom_sad8x16_c(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
RTCD_EXTERN uint32_t(*svt_aom_sad8x16)(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
void svt_aom_sad8x16x4d_c(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
RTCD_EXTERN void(*svt_aom_sad8x16x4d)(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
uint32_t svt_aom_sad8x32_c(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
RTCD_EXTERN uint32_t(*svt_aom_sad8x32)(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
void svt_aom_sad8x32x4d_c(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
RTCD_EXTERN void(*svt_aom_sad8x32x4d)(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
uint32_t svt_aom_sad8x4_c(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
RTCD_EXTERN uint32_t(*svt_aom_sad8x4)(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
void svt_aom_sad8x4x4d_c(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
RTCD_EXTERN void(*svt_aom_sad8x4x4d)(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
uint32_t svt_aom_sad8x8_c(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
RTCD_EXTERN uint32_t(*svt_aom_sad8x8)(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
void svt_aom_sad8x8x4d_c(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
RTCD_EXTERN void(*svt_aom_sad8x8x4d)(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);
void svt_aom_upsampled_pred_c(MacroBlockD* xd, const struct AV1Common* const cm, int mi_row, int mi_col, const Mv* const mv, uint8_t* comp_pred, int width, int height, int subpel_x_q3, int subpel_y_q3, const uint8_t* ref, int ref_stride, int subpel_search);
RTCD_EXTERN void(*svt_aom_upsampled_pred) (MacroBlockD* xd, const struct AV1Common* const cm, int mi_row, int mi_col, const Mv* const mv, uint8_t* comp_pred, int width, int height, int subpel_x_q3, int subpel_y_q3, const uint8_t* ref, int ref_stride, int subpel_search);
#if CONFIG_ENABLE_OBMC
unsigned int svt_aom_obmc_sad128x128_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sad128x128)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad128x64_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sad128x64)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad16x16_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sad16x16)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad16x32_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sad16x32)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad16x4_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sad16x4)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad16x64_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sad16x64)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad16x8_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sad16x8)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad32x16_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sad32x16)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad32x32_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sad32x32)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad32x64_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sad32x64)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad32x8_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sad32x8)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad4x16_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sad4x16)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad4x4_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sad4x4)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad4x8_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sad4x8)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad64x128_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sad64x128)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad64x16_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sad64x16)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad64x32_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sad64x32)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad64x64_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sad64x64)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad8x16_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sad8x16)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad8x32_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sad8x32)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad8x4_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sad8x4)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad8x8_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sad8x8)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sub_pixel_variance128x128_c(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sub_pixel_variance128x128)(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance128x64_c(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sub_pixel_variance128x64)(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance16x16_c(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sub_pixel_variance16x16)(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance16x32_c(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sub_pixel_variance16x32)(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance16x4_c(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sub_pixel_variance16x4)(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance16x64_c(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sub_pixel_variance16x64)(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance16x8_c(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sub_pixel_variance16x8)(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance32x16_c(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sub_pixel_variance32x16)(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance32x32_c(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sub_pixel_variance32x32)(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance32x64_c(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sub_pixel_variance32x64)(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance32x8_c(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sub_pixel_variance32x8)(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance4x16_c(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sub_pixel_variance4x16)(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance4x4_c(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sub_pixel_variance4x4)(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance4x8_c(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sub_pixel_variance4x8)(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance64x128_c(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sub_pixel_variance64x128)(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance64x16_c(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sub_pixel_variance64x16)(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance64x32_c(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sub_pixel_variance64x32)(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance64x64_c(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sub_pixel_variance64x64)(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance8x16_c(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sub_pixel_variance8x16)(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance8x32_c(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sub_pixel_variance8x32)(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance8x4_c(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sub_pixel_variance8x4)(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance8x8_c(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_sub_pixel_variance8x8)(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_variance128x128_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_variance128x128)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_variance128x64_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_variance128x64)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_variance16x16_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_variance16x16)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_variance16x32_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_variance16x32)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_variance16x4_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_variance16x4)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_variance16x64_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_variance16x64)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_variance16x8_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_variance16x8)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_variance32x16_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_variance32x16)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_variance32x32_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_variance32x32)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_variance32x64_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_variance32x64)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_variance32x8_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_variance32x8)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_variance4x16_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_variance4x16)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_variance4x4_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_variance4x4)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_variance4x8_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_variance4x8)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_variance64x128_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_variance64x128)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_variance64x16_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_variance64x16)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_variance64x32_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_variance64x32)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_variance64x64_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_variance64x64)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_variance8x16_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_variance8x16)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_variance8x32_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_variance8x32)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_variance8x4_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_variance8x4)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_variance8x8_c(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_obmc_variance8x8)(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
#endif // CONFIG_ENABLE_OBMC
unsigned int svt_aom_variance4x4_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_variance4x4)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance4x8_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_variance4x8)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance4x16_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_variance4x16)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance8x4_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_variance8x4)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance8x8_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_variance8x8)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance8x16_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_variance8x16)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance8x32_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_variance8x32)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance16x4_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_variance16x4)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance16x8_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_variance16x8)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance16x16_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_variance16x16)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance16x32_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_variance16x32)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance16x64_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_variance16x64)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance32x8_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_variance32x8)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance32x16_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_variance32x16)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance32x32_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_variance32x32)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance32x64_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_variance32x64)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance64x16_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_variance64x16)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance64x32_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_variance64x32)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance64x64_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_variance64x64)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance64x128_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_variance64x128)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance128x64_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_variance128x64)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance128x128_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_variance128x128)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
unsigned int svt_aom_highbd_10_variance4x4_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_highbd_10_variance4x4)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance4x8_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_highbd_10_variance4x8)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance4x16_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_highbd_10_variance4x16)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance8x4_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_highbd_10_variance8x4)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance8x8_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_highbd_10_variance8x8)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance8x16_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_highbd_10_variance8x16)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance8x32_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_highbd_10_variance8x32)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance16x4_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_highbd_10_variance16x4)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance16x8_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_highbd_10_variance16x8)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance16x16_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_highbd_10_variance16x16)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance16x32_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_highbd_10_variance16x32)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance16x64_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_highbd_10_variance16x64)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance32x8_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_highbd_10_variance32x8)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance32x16_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_highbd_10_variance32x16)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance32x32_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_highbd_10_variance32x32)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance32x64_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_highbd_10_variance32x64)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance64x16_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_highbd_10_variance64x16)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance64x32_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_highbd_10_variance64x32)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance64x64_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_highbd_10_variance64x64)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance64x128_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_highbd_10_variance64x128)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance128x64_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_highbd_10_variance128x64)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance128x128_c(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
RTCD_EXTERN unsigned int(*svt_aom_highbd_10_variance128x128)(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
#endif
uint32_t svt_aom_sub_pixel_variance128x128_c(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#if CONFIG_ENABLE_OBMC
RTCD_EXTERN void (*svt_av1_calc_target_weighted_pred_above)(uint8_t is16bit, MacroBlockD* xd, int rel_mi_col, uint8_t nb_mi_width, MbModeInfo* nb_mi, void* fun_ctxt);
void svt_av1_calc_target_weighted_pred_above_c(uint8_t is16bit, MacroBlockD* xd, int rel_mi_col, uint8_t nb_mi_width, MbModeInfo* nb_mi, void* fun_ctxt);
RTCD_EXTERN void (*svt_av1_calc_target_weighted_pred_left)(uint8_t is16bit, MacroBlockD* xd, int rel_mi_row, uint8_t nb_mi_height, MbModeInfo* nb_mi, void* fun_ctxt);
void svt_av1_calc_target_weighted_pred_left_c(uint8_t is16bit, MacroBlockD* xd, int rel_mi_row, uint8_t nb_mi_height, MbModeInfo* nb_mi, void* fun_ctxt);
#endif // CONFIG_ENABLE_OBMC
#ifdef ARCH_X86_64
uint32_t svt_aom_sub_pixel_variance128x128_sse2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance128x128_ssse3(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance128x128_avx2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance128x128_avx512(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#endif
RTCD_EXTERN uint32_t(*svt_aom_sub_pixel_variance128x128)(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_sub_pixel_variance128x64_c(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#ifdef ARCH_X86_64
uint32_t svt_aom_sub_pixel_variance128x64_sse2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance128x64_ssse3(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance128x64_avx2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance128x64_avx512(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#endif
RTCD_EXTERN uint32_t(*svt_aom_sub_pixel_variance128x64)(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_sub_pixel_variance16x16_c(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#ifdef ARCH_X86_64
uint32_t svt_aom_sub_pixel_variance16x16_sse2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance16x16_ssse3(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance16x16_avx2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#endif
RTCD_EXTERN uint32_t(*svt_aom_sub_pixel_variance16x16)(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_sub_pixel_variance16x32_c(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#ifdef ARCH_X86_64
uint32_t svt_aom_sub_pixel_variance16x32_sse2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance16x32_ssse3(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance16x32_avx2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#endif
RTCD_EXTERN uint32_t(*svt_aom_sub_pixel_variance16x32)(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_sub_pixel_variance16x4_c(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#ifdef ARCH_X86_64
uint32_t svt_aom_sub_pixel_variance16x4_sse2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance16x4_ssse3(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance16x4_avx2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#endif
RTCD_EXTERN uint32_t(*svt_aom_sub_pixel_variance16x4)(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_sub_pixel_variance16x64_c(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#ifdef ARCH_X86_64
uint32_t svt_aom_sub_pixel_variance16x64_sse2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance16x64_ssse3(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance16x64_avx2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#endif
RTCD_EXTERN uint32_t(*svt_aom_sub_pixel_variance16x64)(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_sub_pixel_variance16x8_c(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#ifdef ARCH_X86_64
uint32_t svt_aom_sub_pixel_variance16x8_sse2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance16x8_ssse3(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance16x8_avx2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#endif
RTCD_EXTERN uint32_t(*svt_aom_sub_pixel_variance16x8)(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_sub_pixel_variance32x16_c(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#ifdef ARCH_X86_64
uint32_t svt_aom_sub_pixel_variance32x16_sse2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance32x16_ssse3(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance32x16_avx2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance32x16_avx512(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#endif
RTCD_EXTERN uint32_t(*svt_aom_sub_pixel_variance32x16)(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_sub_pixel_variance32x32_c(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#ifdef ARCH_X86_64
uint32_t svt_aom_sub_pixel_variance32x32_sse2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance32x32_ssse3(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance32x32_avx2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance32x32_avx512(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#endif
RTCD_EXTERN uint32_t(*svt_aom_sub_pixel_variance32x32)(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_sub_pixel_variance32x64_c(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#ifdef ARCH_X86_64
uint32_t svt_aom_sub_pixel_variance32x64_sse2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance32x64_ssse3(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance32x64_avx2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance32x64_avx512(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#endif
RTCD_EXTERN uint32_t(*svt_aom_sub_pixel_variance32x64)(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_sub_pixel_variance32x8_c(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#ifdef ARCH_X86_64
uint32_t svt_aom_sub_pixel_variance32x8_sse2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance32x8_ssse3(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#endif
RTCD_EXTERN uint32_t(*svt_aom_sub_pixel_variance32x8)(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_sub_pixel_variance4x16_c(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#ifdef ARCH_X86_64
uint32_t svt_aom_sub_pixel_variance4x16_sse2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance4x16_ssse3(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#endif
RTCD_EXTERN uint32_t(*svt_aom_sub_pixel_variance4x16)(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_sub_pixel_variance4x4_c(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#ifdef ARCH_X86_64
uint32_t svt_aom_sub_pixel_variance4x4_sse2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance4x4_ssse3(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#endif
RTCD_EXTERN uint32_t(*svt_aom_sub_pixel_variance4x4)(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_sub_pixel_variance4x8_c(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#ifdef ARCH_X86_64
uint32_t svt_aom_sub_pixel_variance4x8_sse2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance4x8_ssse3(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#endif
RTCD_EXTERN uint32_t(*svt_aom_sub_pixel_variance4x8)(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_sub_pixel_variance64x128_c(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#ifdef ARCH_X86_64
uint32_t svt_aom_sub_pixel_variance64x128_sse2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance64x128_ssse3(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance64x128_avx2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance64x128_avx512(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#endif
RTCD_EXTERN uint32_t(*svt_aom_sub_pixel_variance64x128)(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_sub_pixel_variance64x16_c(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#ifdef ARCH_X86_64
uint32_t svt_aom_sub_pixel_variance64x16_sse2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance64x16_ssse3(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#endif
RTCD_EXTERN uint32_t(*svt_aom_sub_pixel_variance64x16)(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_sub_pixel_variance64x32_c(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#ifdef ARCH_X86_64
uint32_t svt_aom_sub_pixel_variance64x32_sse2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance64x32_ssse3(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance64x32_avx2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance64x32_avx512(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#endif
RTCD_EXTERN uint32_t(*svt_aom_sub_pixel_variance64x32)(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_sub_pixel_variance64x64_c(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#ifdef ARCH_X86_64
uint32_t svt_aom_sub_pixel_variance64x64_sse2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance64x64_ssse3(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance64x64_avx2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance64x64_avx512(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#endif
RTCD_EXTERN uint32_t(*svt_aom_sub_pixel_variance64x64)(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_sub_pixel_variance8x16_c(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#ifdef ARCH_X86_64
uint32_t svt_aom_sub_pixel_variance8x16_sse2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance8x16_ssse3(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#endif
RTCD_EXTERN uint32_t(*svt_aom_sub_pixel_variance8x16)(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_sub_pixel_variance8x32_c(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#ifdef ARCH_X86_64
uint32_t svt_aom_sub_pixel_variance8x32_sse2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance8x32_ssse3(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#endif
RTCD_EXTERN uint32_t(*svt_aom_sub_pixel_variance8x32)(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_sub_pixel_variance8x4_c(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#ifdef ARCH_X86_64
uint32_t svt_aom_sub_pixel_variance8x4_sse2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance8x4_ssse3(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#endif
RTCD_EXTERN uint32_t(*svt_aom_sub_pixel_variance8x4)(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_sub_pixel_variance8x8_c(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#ifdef ARCH_X86_64
uint32_t svt_aom_sub_pixel_variance8x8_sse2(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_sub_pixel_variance8x8_ssse3(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#endif
RTCD_EXTERN uint32_t(*svt_aom_sub_pixel_variance8x8)(const uint8_t *src_ptr, int source_stride, int xoffset, int  yoffset, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
void svt_aom_ifft16x16_float_c(const float *input, float *temp, float *output);
RTCD_EXTERN void(*svt_aom_ifft16x16_float)(const float *input, float *temp, float *output);
void svt_aom_ifft2x2_float_c(const float *input, float *temp, float *output);
RTCD_EXTERN void(*svt_aom_ifft2x2_float)(const float *input, float *temp, float *output);
void svt_aom_ifft32x32_float_c(const float *input, float *temp, float *output);
RTCD_EXTERN void(*svt_aom_ifft32x32_float)(const float *input, float *temp, float *output);
void svt_aom_ifft4x4_float_c(const float *input, float *temp, float *output);
RTCD_EXTERN void(*svt_aom_ifft4x4_float)(const float *input, float *temp, float *output);
void svt_aom_ifft8x8_float_c(const float *input, float *temp, float *output);
RTCD_EXTERN void(*svt_aom_ifft8x8_float)(const float *input, float *temp, float *output);
void svt_aom_fft16x16_float_c(const float *input, float *temp, float *output);
RTCD_EXTERN void(*svt_aom_fft16x16_float)(const float *input, float *temp, float *output);
void svt_aom_fft2x2_float_c(const float *input, float *temp, float *output);
RTCD_EXTERN void(*svt_aom_fft2x2_float)(const float *input, float *temp, float *output);
void svt_aom_fft32x32_float_c(const float *input, float *temp, float *output);
RTCD_EXTERN void(*svt_aom_fft32x32_float)(const float *input, float *temp, float *output);
void svt_aom_fft4x4_float_c(const float *input, float *temp, float *output);
RTCD_EXTERN void(*svt_aom_fft4x4_float)(const float *input, float *temp, float *output);
void svt_aom_fft8x8_float_c(const float *input, float *temp, float *output);
RTCD_EXTERN void(*svt_aom_fft8x8_float)(const float *input, float *temp, float *output);
void svt_av1_get_nz_map_contexts_c(const uint8_t *const levels, const int16_t *const scan, const uint16_t eob, const TxSize tx_size, const TxClass tx_class, int8_t *const coeff_contexts);
RTCD_EXTERN void(*svt_av1_get_nz_map_contexts)(const uint8_t *const levels, const int16_t *const scan, const uint16_t eob, const TxSize tx_size, const TxClass tx_class, int8_t *const coeff_contexts);
RTCD_EXTERN void(*svt_sad_loop_kernel)(uint8_t *src, uint32_t src_stride, uint8_t *ref, uint32_t ref_stride, uint32_t block_height, uint32_t block_width, uint64_t *best_sad, int16_t *x_search_center, int16_t *y_search_center, uint32_t src_stride_raw, uint8_t skip_search_line, int16_t search_area_width, int16_t search_area_height);
void svt_av1_txb_init_levels_c(const TranLow *const coeff, const int32_t width, const int32_t height, uint8_t *const levels);
RTCD_EXTERN void(*svt_av1_txb_init_levels)(const TranLow *const coeff, const int32_t width, const int32_t height, uint8_t *const levels);
double svt_av1_compute_cross_correlation_c(unsigned char *im1, int stride1, int x1, int y1, unsigned char *im2, int stride2, int x2, int y2, uint8_t match_sz);
RTCD_EXTERN double(*svt_av1_compute_cross_correlation)(unsigned char *im1, int stride1, int x1, int y1, unsigned char *im2, int stride2, int x2, int y2, uint8_t match_sz);
void svt_av1_k_means_dim1_c(const int* data, int* centroids, uint8_t* indices, int n, int k, int max_itr);
RTCD_EXTERN void(*svt_av1_k_means_dim1)(const int* data, int* centroids, uint8_t* indices, int n, int k, int max_itr);
void svt_av1_k_means_dim2_c(const int* data, int* centroids, uint8_t* indices, int n, int k, int max_itr);
RTCD_EXTERN void(*svt_av1_k_means_dim2)(const int* data, int* centroids, uint8_t* indices, int n, int k, int max_itr);
void svt_av1_calc_indices_dim1_c(const int* data, const int* centroids, uint8_t* indices, int n, int k);
RTCD_EXTERN void(*svt_av1_calc_indices_dim1)(const int* data, const int* centroids, uint8_t* indices, int n, int k);
void svt_av1_calc_indices_dim2_c(const int* data, const int* centroids, uint8_t* indices, int n, int k);
RTCD_EXTERN void(*svt_av1_calc_indices_dim2)(const int* data, const int* centroids, uint8_t* indices, int n, int k);

struct MeContext;
RTCD_EXTERN void(*svt_av1_apply_zz_based_temporal_filter_planewise_medium)(
    struct MeContext *me_ctx, const uint8_t *y_pre,
    int y_pre_stride,
    const uint8_t *u_pre, const uint8_t *v_pre, int uv_pre_stride, unsigned int block_width,
    unsigned int block_height, int ss_x, int ss_y, uint32_t *y_accum, uint16_t *y_count, uint32_t *u_accum,
    uint16_t *u_count, uint32_t *v_accum, uint16_t *v_count);

RTCD_EXTERN void(*svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd)(
    struct MeContext *me_ctx, const uint16_t *y_pre,
    int y_pre_stride,
    const uint16_t *u_pre, const uint16_t *v_pre, int uv_pre_stride, unsigned int block_width,
    unsigned int block_height, int ss_x, int ss_y, uint32_t *y_accum, uint16_t *y_count, uint32_t *u_accum,
    uint16_t *u_count, uint32_t *v_accum, uint16_t *v_count, uint32_t encoder_bit_depth);


RTCD_EXTERN void(*svt_av1_apply_temporal_filter_planewise_medium)(
    struct MeContext *me_ctx, const uint8_t *y_src, int y_src_stride, const uint8_t *y_pre,
    int y_pre_stride, const uint8_t *u_src, const uint8_t *v_src, int uv_src_stride,
    const uint8_t *u_pre, const uint8_t *v_pre, int uv_pre_stride, unsigned int block_width,
    unsigned int block_height, int ss_x, int ss_y, uint32_t *y_accum, uint16_t *y_count, uint32_t *u_accum,
    uint16_t *u_count, uint32_t *v_accum, uint16_t *v_count);

RTCD_EXTERN void(*svt_av1_apply_temporal_filter_planewise_medium_hbd)(
    struct MeContext *me_ctx, const uint16_t *y_src, int y_src_stride, const uint16_t *y_pre,
    int y_pre_stride, const uint16_t *u_src, const uint16_t *v_src, int uv_src_stride,
    const uint16_t *u_pre, const uint16_t *v_pre, int uv_pre_stride, unsigned int block_width,
    unsigned int block_height, int ss_x, int ss_y, uint32_t *y_accum, uint16_t *y_count, uint32_t *u_accum,
    uint16_t *u_count, uint32_t *v_accum, uint16_t *v_count, uint32_t encoder_bit_depth);
struct MeContext;
void svt_aom_get_final_filtered_pixels_c(struct MeContext *me_ctx, EbByte *src_center_ptr_start, uint16_t **altref_buffer_highbd_start, uint32_t **accum, uint16_t **count, const uint32_t *stride, int blk_y_src_offset, int blk_ch_src_offset, uint16_t blk_width_ch, uint16_t blk_height_ch, bool is_highbd);
void svt_aom_get_final_filtered_pixels_sse4_1(struct MeContext *me_ctx, EbByte *src_center_ptr_start, uint16_t **altref_buffer_highbd_start, uint32_t **accum, uint16_t **count, const uint32_t *stride, int blk_y_src_offset, int blk_ch_src_offset, uint16_t blk_width_ch, uint16_t blk_height_ch, bool is_highbd);
void svt_aom_get_final_filtered_pixels_avx2(struct MeContext *me_ctx, EbByte *src_center_ptr_start, uint16_t **altref_buffer_highbd_start, uint32_t **accum, uint16_t **count, const uint32_t *stride, int blk_y_src_offset, int blk_ch_src_offset, uint16_t blk_width_ch, uint16_t blk_height_ch, bool is_highbd);
RTCD_EXTERN void (*get_final_filtered_pixels)(struct MeContext *me_ctx, EbByte *src_center_ptr_start, uint16_t **altref_buffer_highbd_start, uint32_t **accum, uint16_t **count, const uint32_t *stride, int blk_y_src_offset, int blk_ch_src_offset, uint16_t blk_width_ch, uint16_t blk_height_ch, bool is_highbd);
void svt_aom_apply_filtering_central_sse4_1(struct MeContext *me_ctx, EbPictureBufferDesc *input_picture_ptr_central, EbByte *src, uint32_t **accum, uint16_t **count, uint16_t blk_width, uint16_t blk_height, uint32_t ss_x, uint32_t ss_y);
void svt_aom_apply_filtering_central_avx2(struct MeContext *me_ctx, EbPictureBufferDesc *input_picture_ptr_central, EbByte *src, uint32_t **accum, uint16_t **count, uint16_t blk_width, uint16_t blk_height, uint32_t ss_x, uint32_t ss_y);
void svt_aom_apply_filtering_central_c(struct MeContext *me_ctx, EbPictureBufferDesc *input_picture_ptr_central, EbByte *src, uint32_t **accum, uint16_t **count, uint16_t blk_width, uint16_t blk_height, uint32_t ss_x, uint32_t ss_y);
RTCD_EXTERN void (*apply_filtering_central)(struct MeContext *me_ctx, EbPictureBufferDesc *input_picture_ptr_central, EbByte *src, uint32_t **accum, uint16_t **count, uint16_t blk_width, uint16_t blk_height, uint32_t ss_x, uint32_t ss_y);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_aom_apply_filtering_central_highbd_sse4_1(struct MeContext *me_ctx, EbPictureBufferDesc *input_picture_ptr_central, uint16_t **src_16bit, uint32_t **accum, uint16_t **count, uint16_t blk_width, uint16_t blk_height, uint32_t ss_x, uint32_t ss_y);
void svt_aom_apply_filtering_central_highbd_avx2(struct MeContext *me_ctx, EbPictureBufferDesc *input_picture_ptr_central, uint16_t **src_16bit, uint32_t **accum, uint16_t **count, uint16_t blk_width, uint16_t blk_height, uint32_t ss_x, uint32_t ss_y);
void svt_aom_apply_filtering_central_highbd_c(struct MeContext *me_ctx, EbPictureBufferDesc *input_picture_ptr_central, uint16_t **src_16bit, uint32_t **accum, uint16_t **count, uint16_t blk_width, uint16_t blk_height, uint32_t ss_x, uint32_t ss_y);
RTCD_EXTERN void (*apply_filtering_central_highbd)(struct MeContext *me_ctx, EbPictureBufferDesc *input_picture_ptr_central, uint16_t **src_16bit, uint32_t **accum, uint16_t **count, uint16_t blk_width, uint16_t blk_height, uint32_t ss_x, uint32_t ss_y);
#endif
void svt_aom_downsample_2d_sse4_1(uint8_t *input_samples, uint32_t input_stride, uint32_t input_area_width, uint32_t input_area_height, uint8_t *decim_samples, uint32_t decim_stride, uint32_t decim_step);
void svt_aom_downsample_2d_avx2(uint8_t *input_samples, uint32_t input_stride, uint32_t input_area_width, uint32_t input_area_height, uint8_t *decim_samples, uint32_t decim_stride, uint32_t decim_step);
RTCD_EXTERN void (*downsample_2d)(uint8_t *input_samples, uint32_t input_stride, uint32_t input_area_width, uint32_t input_area_height, uint8_t *decim_samples, uint32_t decim_stride, uint32_t decim_step);
RTCD_EXTERN void(*svt_ext_sad_calculation_8x8_16x16)(uint8_t *src, uint32_t src_stride, uint8_t *ref, uint32_t ref_stride, uint32_t *p_best_sad_8x8, uint32_t *p_best_sad_16x16, uint32_t *p_best_mv8x8, uint32_t *p_best_mv16x16, uint32_t mv, uint32_t *p_sad16x16, uint32_t *p_sad8x8, bool sub_sad);
void svt_ext_sad_calculation_8x8_16x16_c(uint8_t *src, uint32_t src_stride, uint8_t *ref,
    uint32_t ref_stride, uint32_t *p_best_sad_8x8,
    uint32_t *p_best_sad_16x16, uint32_t *p_best_mv8x8,
    uint32_t *p_best_mv16x16, uint32_t mv, uint32_t *p_sad16x16,
    uint32_t *p_sad8x8, bool sub_sad);
RTCD_EXTERN void(*svt_ext_sad_calculation_32x32_64x64)(uint32_t *p_sad16x16, uint32_t *p_best_sad_32x32, uint32_t *p_best_sad_64x64, uint32_t *p_best_mv32x32, uint32_t *p_best_mv64x64, uint32_t mv, uint32_t *p_sad32x32);
void svt_ext_sad_calculation_32x32_64x64_c(uint32_t *p_sad16x16, uint32_t *p_best_sad_32x32,
    uint32_t *p_best_sad_64x64, uint32_t *p_best_mv32x32,
    uint32_t *p_best_mv64x64, uint32_t mv, uint32_t *p_sad32x32);

RTCD_EXTERN void(*svt_ext_all_sad_calculation_8x8_16x16)(uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride, uint32_t mv, uint32_t* p_best_sad_8x8, uint32_t* p_best_sad_16x16, uint32_t* p_best_mv8x8, uint32_t* p_best_mv16x16, uint32_t p_eight_sad16x16[16][8], uint32_t p_eight_sad8x8[64][8], bool sub_sad);
RTCD_EXTERN void(*svt_ext_eight_sad_calculation_32x32_64x64)(const uint32_t p_sad16x16[16][8], uint32_t *p_best_sad_32x32, uint32_t *p_best_sad_64x64, uint32_t *p_best_mv32x32, uint32_t *p_best_mv64x64, uint32_t mv, uint32_t p_sad32x32[4][8]);
RTCD_EXTERN void(*svt_initialize_buffer_32bits)(uint32_t* pointer, uint32_t count128, uint32_t count32, uint32_t value);
RTCD_EXTERN uint32_t(*svt_nxm_sad_kernel)(const uint8_t *src, uint32_t src_stride, const uint8_t *ref, uint32_t ref_stride, uint32_t height, uint32_t width);
RTCD_EXTERN uint32_t(*nxm_sad_avg_kernel)(uint8_t *src, uint32_t src_stride, uint8_t *ref1, uint32_t ref1_stride, uint8_t *ref2, uint32_t ref2_stride, uint32_t height, uint32_t width);
RTCD_EXTERN uint64_t(*svt_compute_mean_8x8)(uint8_t *input_samples, uint32_t input_stride, uint32_t input_area_width, uint32_t input_area_height);
RTCD_EXTERN uint64_t(*svt_compute_mean_square_values_8x8)(uint8_t *input_samples, uint32_t input_stride, uint32_t input_area_width, uint32_t input_area_height);
uint64_t svt_compute_sub_mean_8x8_c(uint8_t* input_samples, uint16_t input_stride);
RTCD_EXTERN void(*svt_compute_interm_var_four8x8)(uint8_t *input_samples, uint16_t input_stride, uint64_t *mean_of8x8_blocks, uint64_t *mean_of_squared8x8_blocks);
RTCD_EXTERN uint32_t(*sad_16b_kernel)(uint16_t *src, uint32_t src_stride, uint16_t *ref, uint32_t ref_stride, uint32_t height, uint32_t width);
struct svt_mv_cost_param;
void svt_pme_sad_loop_kernel_c(const struct svt_mv_cost_param *mv_cost_params, uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride, uint32_t block_height, uint32_t block_width, uint32_t *best_cost, int16_t *best_mvx, int16_t *best_mvy, int16_t search_position_start_x, int16_t search_position_start_y, int16_t search_area_width, int16_t search_area_height, int16_t search_step, int16_t mvx, int16_t mvy);
void svt_pme_sad_loop_kernel_sse4_1(const struct svt_mv_cost_param *mv_cost_params, uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride, uint32_t block_height, uint32_t block_width, uint32_t *best_cost, int16_t *best_mvx, int16_t *best_mvy, int16_t search_position_start_x, int16_t search_position_start_y, int16_t search_area_width, int16_t search_area_height, int16_t search_step, int16_t mvx, int16_t mvy);
void svt_pme_sad_loop_kernel_avx2(const struct svt_mv_cost_param *mv_cost_params, uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride, uint32_t block_height, uint32_t block_width, uint32_t *best_cost, int16_t *best_mvx, int16_t *best_mvy, int16_t search_position_start_x, int16_t search_position_start_y, int16_t search_area_width, int16_t search_area_height, int16_t search_step, int16_t mvx, int16_t mvy);
RTCD_EXTERN void(*svt_pme_sad_loop_kernel)(const struct svt_mv_cost_param *mv_cost_params, uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride, uint32_t block_height, uint32_t block_width, uint32_t *best_cost, int16_t *best_mvx, int16_t *best_mvy, int16_t search_position_start_x, int16_t search_position_start_y, int16_t search_area_width, int16_t search_area_height, int16_t search_step, int16_t mvx, int16_t mvy);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
RTCD_EXTERN uint32_t(*variance_highbd)(const uint16_t *a, int a_stride, const uint16_t *b, int b_stride, int w, int h, uint32_t *sse);
uint32_t svt_aom_variance_highbd_c(const uint16_t *a, int a_stride, const uint16_t *b, int b_stride, int w, int h, uint32_t *sse);
#endif
void svt_unpack_and_2bcompress_sse4_1(uint16_t *in16b_buffer, uint32_t in16b_stride, uint8_t *out8b_buffer, uint32_t out8b_stride, uint8_t *out2b_buffer, uint32_t out2b_stride, uint32_t width, uint32_t height);
void svt_unpack_and_2bcompress_avx2(uint16_t *in16b_buffer, uint32_t in16b_stride, uint8_t *out8b_buffer, uint32_t out8b_stride, uint8_t *out2b_buffer, uint32_t out2b_stride, uint32_t width, uint32_t height);
RTCD_EXTERN void (*svt_unpack_and_2bcompress)(uint16_t *in16b_buffer, uint32_t in16b_stride, uint8_t *out8b_buffer, uint32_t out8b_stride,uint8_t *out2b_buffer, uint32_t out2b_stride, uint32_t width, uint32_t height);
RTCD_EXTERN int32_t(*svt_estimate_noise_fp16)(const uint8_t *src, uint16_t width, uint16_t height, uint16_t stride_y);
int32_t svt_estimate_noise_fp16_c(const uint8_t *src, uint16_t width, uint16_t height, uint16_t stride_y);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
RTCD_EXTERN int32_t (*svt_estimate_noise_highbd_fp16)(const uint16_t *src, int width, int height, int stride, int bd);
int32_t svt_estimate_noise_highbd_fp16_c(const uint16_t *src, int width, int height, int stride, int bd);
#endif
RTCD_EXTERN void(*svt_copy_mi_map_grid)(MbModeInfo** mi_grid_ptr, uint32_t mi_stride, uint8_t num_rows, uint8_t num_cols);
void svt_copy_mi_map_grid_c(MbModeInfo** mi_grid_ptr, uint32_t mi_stride, uint8_t num_rows, uint8_t num_cols);
void svt_copy_mi_map_grid_avx2(MbModeInfo** mi_grid_ptr, uint32_t mi_stride, uint8_t num_rows, uint8_t num_cols);
RTCD_EXTERN void (*svt_av1_add_block_observations_internal)(uint32_t n, const double val, const double recp_sqr_norm, double *buffer, double *buffer_norm, double *b, double *A);
void svt_av1_add_block_observations_internal_c(uint32_t n, const double val, const double recp_sqr_norm, double *buffer, double *buffer_norm, double *b, double *A);
RTCD_EXTERN void (*svt_av1_pointwise_multiply)(const float *a, float *b, float *c, double *b_d, double *c_d, int32_t n);
void svt_av1_pointwise_multiply_c(const float *a, float *b, float *c, double *b_d, double *c_d, int32_t n);
RTCD_EXTERN void (*svt_av1_apply_window_function_to_plane)(int32_t y_size, int32_t x_size, float *result_ptr, uint32_t result_stride, float *block, float *plane, const float *window_function);
void svt_av1_apply_window_function_to_plane_c(int32_t y_size, int32_t x_size, float *result_ptr, uint32_t result_stride, float *block, float *plane, const float *window_function);
RTCD_EXTERN void (*svt_aom_noise_tx_filter)(int32_t block_size, float *block_ptr, const float psd);
void svt_aom_noise_tx_filter_c(int32_t block_size, float *block_ptr, const float psd);
#if CONFIG_ENABLE_FILM_GRAIN
RTCD_EXTERN void (*svt_aom_flat_block_finder_extract_block)(const AomFlatBlockFinder *block_finder, const uint8_t *const data, int32_t w, int32_t h, int32_t stride, int32_t offsx, int32_t offsy, double *plane, double *block);
void svt_aom_flat_block_finder_extract_block_c(const AomFlatBlockFinder *block_finder, const uint8_t *const data, int32_t w, int32_t h, int32_t stride, int32_t offsx, int32_t offsy, double *plane, double *block);
#endif
RTCD_EXTERN void(*svt_av1_interpolate_core)(const uint8_t *const input, int in_length, uint8_t *output, int out_length, const int16_t *interp_filters);
void svt_av1_interpolate_core_c(const uint8_t *const input, int in_length, uint8_t *output, int out_length, const int16_t *interp_filters);
RTCD_EXTERN void(*svt_av1_down2_symeven)(const uint8_t *const input, int length, uint8_t *output);
void svt_av1_down2_symeven_c(const uint8_t *const input, int length, uint8_t *output);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
RTCD_EXTERN void(*svt_av1_highbd_interpolate_core)(const uint16_t *const input, int in_length, uint16_t *output, int out_length, int bd, const int16_t *interp_filters);
void svt_av1_highbd_interpolate_core_c(const uint16_t *const input, int in_length, uint16_t *output, int out_length, int bd, const int16_t *interp_filters);
RTCD_EXTERN void(*svt_av1_highbd_down2_symeven)(const uint16_t *const input, int length, uint16_t *output, int bd);
void svt_av1_highbd_down2_symeven_c(const uint16_t *const input, int length, uint16_t *output, int bd);
RTCD_EXTERN EbErrorType(*svt_av1_highbd_resize_plane)(const uint16_t *const input, int height, int width, int in_stride, uint16_t *output, int height2, int width2, int out_stride, int bd);
EbErrorType svt_av1_highbd_resize_plane_c(const uint16_t *const input, int height, int width, int in_stride, uint16_t *output, int height2, int width2, int out_stride, int bd);
#endif
RTCD_EXTERN EbErrorType(*svt_av1_resize_plane)(const uint8_t *const input, int height, int width, int in_stride, uint8_t *output, int height2, int width2, int out_stride);
EbErrorType svt_av1_resize_plane_c(const uint8_t *const input, int height, int width, int in_stride, uint8_t *output, int height2, int width2, int out_stride);
RTCD_EXTERN uint8_t(*svt_av1_compute_cul_level)(const int16_t* const scan, const int32_t* const quant_coeff, uint16_t* eob);
uint8_t svt_av1_compute_cul_level_c(const int16_t* const scan, const int32_t* const quant_coeff, uint16_t* eob);
RTCD_EXTERN double (*svt_ssim_8x8)(const uint8_t* s, uint32_t sp, const uint8_t* r, uint32_t rp);
double svt_ssim_8x8_c(const uint8_t* s, uint32_t sp, const uint8_t* r, uint32_t rp);
RTCD_EXTERN double (*svt_ssim_4x4)(const uint8_t* s, uint32_t sp, const uint8_t* r, uint32_t rp);
double svt_ssim_4x4_c(const uint8_t* s, uint32_t sp, const uint8_t* r, uint32_t rp);
RTCD_EXTERN double (*svt_ssim_8x8_hbd)(const uint16_t* s, uint32_t sp, const uint16_t* r, uint32_t rp);
double svt_ssim_8x8_hbd_c(const uint16_t* s, uint32_t sp, const uint16_t* r, uint32_t rp);
RTCD_EXTERN double (*svt_ssim_4x4_hbd)(const uint16_t* s, uint32_t sp, const uint16_t* r, uint32_t rp);
double svt_ssim_4x4_hbd_c(const uint16_t* s, uint32_t sp, const uint16_t* r, uint32_t rp);

#ifdef ARCH_AARCH64
void svt_av1_calc_indices_dim1_neon(const int* data, const int* centroids, uint8_t* indices, int n, int k);
void svt_av1_calc_indices_dim2_neon(const int* data, const int* centroids, uint8_t* indices, int n, int k);
void svt_av1_compute_stats_neon(int32_t wiener_win, const uint8_t *dgd8, const uint8_t *src8, int32_t h_start, int32_t h_end, int32_t v_start, int32_t v_end, int32_t dgd_stride, int32_t src_stride, int64_t *M, int64_t *H);
void svt_av1_compute_stats_sve(int32_t wiener_win, const uint8_t *dgd8, const uint8_t *src8, int32_t h_start, int32_t h_end, int32_t v_start, int32_t v_end, int32_t dgd_stride, int32_t src_stride, int64_t *M, int64_t *H);
void svt_compute_interm_var_four8x8_neon(uint8_t *input_samples, uint16_t input_stride, uint64_t *mean_of8x8_blocks, uint64_t *mean_of_squared8x8_blocks);
void svt_compute_interm_var_four8x8_neon_dotprod(uint8_t *input_samples, uint16_t input_stride, uint64_t *mean_of8x8_blocks, uint64_t *mean_of_squared8x8_blocks);
void svt_ext_sad_calculation_8x8_16x16_neon(uint8_t *src, uint32_t src_stride, uint8_t *ref,
    uint32_t ref_stride, uint32_t *p_best_sad_8x8,
    uint32_t *p_best_sad_16x16, uint32_t *p_best_mv8x8,
    uint32_t *p_best_mv16x16, uint32_t mv,
    uint32_t *p_sad16x16, uint32_t *p_sad8x8,
    bool sub_sad);
void svt_ext_sad_calculation_8x8_16x16_neon_dotprod(uint8_t *src, uint32_t src_stride, uint8_t *ref,
    uint32_t ref_stride, uint32_t *p_best_sad_8x8,
    uint32_t *p_best_sad_16x16, uint32_t *p_best_mv8x8,
    uint32_t *p_best_mv16x16, uint32_t mv,
    uint32_t *p_sad16x16, uint32_t *p_sad8x8,
    bool sub_sad);


void svt_ext_all_sad_calculation_8x8_16x16_neon(uint8_t *src, uint32_t src_stride, uint8_t *ref,
    uint32_t ref_stride, uint32_t mv,
    uint32_t *p_best_sad_8x8, uint32_t *p_best_sad_16x16,
    uint32_t *p_best_mv8x8, uint32_t *p_best_mv16x16,
    uint32_t p_eight_sad16x16[16][8],
    uint32_t p_eight_sad8x8[64][8], bool sub_sad);
void svt_ext_all_sad_calculation_8x8_16x16_neon_dotprod(uint8_t *src, uint32_t src_stride, uint8_t *ref,
    uint32_t ref_stride, uint32_t mv,
    uint32_t *p_best_sad_8x8, uint32_t *p_best_sad_16x16,
    uint32_t *p_best_mv8x8, uint32_t *p_best_mv16x16,
    uint32_t p_eight_sad16x16[16][8],
    uint32_t p_eight_sad8x8[64][8], bool sub_sad);
void svt_ext_all_sad_calculation_8x8_16x16_sve(uint8_t *src, uint32_t src_stride, uint8_t *ref,
    uint32_t ref_stride, uint32_t mv,
    uint32_t *p_best_sad_8x8, uint32_t *p_best_sad_16x16,
    uint32_t *p_best_mv8x8, uint32_t *p_best_mv16x16,
    uint32_t p_eight_sad16x16[16][8],
    uint32_t p_eight_sad8x8[64][8], bool sub_sad);

void svt_aom_upsampled_pred_neon(MacroBlockD* xd, const struct AV1Common* const cm, int mi_row, int mi_col, const Mv* const mv, uint8_t* comp_pred, int width, int height, int subpel_x_q3, int subpel_y_q3, const uint8_t* ref, int ref_stride, int subpel_search);

void svt_sad_loop_kernel_neon(uint8_t *src, uint32_t src_stride, uint8_t *ref, uint32_t ref_stride,
                            uint32_t block_height, uint32_t block_width, uint64_t *best_sad,
                            int16_t *x_search_center, int16_t *y_search_center,
                            uint32_t src_stride_raw, uint8_t skip_search_line,
                            int16_t search_area_width, int16_t search_area_height);
void svt_sad_loop_kernel_neon_dotprod(uint8_t *src, uint32_t src_stride, uint8_t *ref, uint32_t ref_stride,
                            uint32_t block_height, uint32_t block_width, uint64_t *best_sad,
                            int16_t *x_search_center, int16_t *y_search_center,
                            uint32_t src_stride_raw, uint8_t skip_search_line,
                            int16_t search_area_width, int16_t search_area_height);
void svt_sad_loop_kernel_sve(uint8_t *src, uint32_t src_stride, uint8_t *ref, uint32_t ref_stride,
                            uint32_t block_height, uint32_t block_width, uint64_t *best_sad,
                            int16_t *x_search_center, int16_t *y_search_center,
                            uint32_t src_stride_raw, uint8_t skip_search_line,
                            int16_t search_area_width, int16_t search_area_height);
void svt_sad_loop_kernel_neoverse_v2(uint8_t *src, uint32_t src_stride, uint8_t *ref, uint32_t ref_stride,
                            uint32_t block_height, uint32_t block_width, uint64_t *best_sad,
                            int16_t *x_search_center, int16_t *y_search_center,
                            uint32_t src_stride_raw, uint8_t skip_search_line,
                            int16_t search_area_width, int16_t search_area_height);

void svt_pme_sad_loop_kernel_neon(const struct svt_mv_cost_param *mv_cost_params, uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride, uint32_t block_height, uint32_t block_width, uint32_t *best_cost, int16_t *best_mvx, int16_t *best_mvy, int16_t search_position_start_x, int16_t search_position_start_y, int16_t search_area_width, int16_t search_area_height, int16_t search_step, int16_t mvx, int16_t mvy);

unsigned int svt_aom_mse16x16_neon(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride);
unsigned int svt_aom_mse16x16_neon_dotprod(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
uint32_t svt_aom_highbd_mse16x16_neon(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
#endif

uint32_t svt_aom_sad_16b_kernel_neon(uint16_t *src, uint32_t  src_stride, uint16_t *ref, uint32_t  ref_stride, uint32_t  height, uint32_t  width);
unsigned int svt_aom_variance4x4_neon(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance4x8_neon(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance4x16_neon(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance8x4_neon(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance8x8_neon(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance8x16_neon(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance8x32_neon(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance16x4_neon(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance16x8_neon(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance16x16_neon(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance16x32_neon(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance16x64_neon(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance32x8_neon(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance32x16_neon(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance32x32_neon(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance32x64_neon(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance64x16_neon(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance64x32_neon(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance64x64_neon(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance64x128_neon(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance128x64_neon(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance128x128_neon(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance4x8_neon_dotprod(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance4x16_neon_dotprod(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance8x4_neon_dotprod(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance8x8_neon_dotprod(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance8x16_neon_dotprod(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance8x32_neon_dotprod(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance16x4_neon_dotprod(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance16x8_neon_dotprod(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance16x16_neon_dotprod(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance16x32_neon_dotprod(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance16x64_neon_dotprod(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance32x8_neon_dotprod(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance32x16_neon_dotprod(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance32x32_neon_dotprod(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance32x64_neon_dotprod(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance64x16_neon_dotprod(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance64x32_neon_dotprod(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance64x64_neon_dotprod(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance64x128_neon_dotprod(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance128x64_neon_dotprod(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance128x128_neon_dotprod(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, unsigned int *sse);

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
uint32_t svt_aom_highbd_10_variance4x4_neon(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance4x8_neon(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance4x16_neon(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_highbd_10_variance8x4_neon(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance8x8_neon(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance8x16_neon(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance8x32_neon(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_highbd_10_variance16x4_neon(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance16x8_neon(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance16x16_neon(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance16x32_neon(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance16x64_neon(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_highbd_10_variance32x8_neon(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance32x16_neon(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance32x32_neon(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance32x64_neon(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_highbd_10_variance64x16_neon(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance64x32_neon(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance64x64_neon(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance64x128_neon(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_highbd_10_variance128x64_neon(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance128x128_neon(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_highbd_10_variance4x4_sve(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance4x8_sve(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance4x16_sve(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_highbd_10_variance8x4_sve(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance8x8_sve(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance8x16_sve(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance8x32_sve(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_highbd_10_variance16x4_sve(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance16x8_sve(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance16x16_sve(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance16x32_sve(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance16x64_sve(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_highbd_10_variance32x8_sve(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance32x16_sve(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance32x32_sve(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance32x64_sve(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_highbd_10_variance64x16_sve(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance64x32_sve(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance64x64_sve(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance64x128_sve(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);

uint32_t svt_aom_highbd_10_variance128x64_sve(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
uint32_t svt_aom_highbd_10_variance128x128_sve(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride, uint32_t *sse);
#endif

unsigned int svt_aom_sub_pixel_variance4x4_neon(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance4x8_neon(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance4x16_neon(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);

unsigned int svt_aom_sub_pixel_variance8x4_neon(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance8x8_neon(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance8x16_neon(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance8x32_neon(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);

unsigned int svt_aom_sub_pixel_variance16x4_neon(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance16x8_neon(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance16x16_neon(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance16x32_neon(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance16x64_neon(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);

unsigned int svt_aom_sub_pixel_variance32x8_neon(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance32x16_neon(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance32x32_neon(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance32x64_neon(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);

unsigned int svt_aom_sub_pixel_variance64x16_neon(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance64x32_neon(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance64x64_neon(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance64x128_neon(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);

unsigned int svt_aom_sub_pixel_variance128x64_neon(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance128x128_neon(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);

unsigned int svt_aom_sub_pixel_variance4x8_neon_dotprod(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance4x16_neon_dotprod(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);

unsigned int svt_aom_sub_pixel_variance8x4_neon_dotprod(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance8x8_neon_dotprod(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance8x16_neon_dotprod(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance8x32_neon_dotprod(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);

unsigned int svt_aom_sub_pixel_variance16x4_neon_dotprod(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance16x8_neon_dotprod(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance16x16_neon_dotprod(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance16x32_neon_dotprod(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance16x64_neon_dotprod(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);

unsigned int svt_aom_sub_pixel_variance32x8_neon_dotprod(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance32x16_neon_dotprod(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance32x32_neon_dotprod(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance32x64_neon_dotprod(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);

unsigned int svt_aom_sub_pixel_variance64x16_neon_dotprod(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance64x32_neon_dotprod(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance64x64_neon_dotprod(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance64x128_neon_dotprod(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);

unsigned int svt_aom_sub_pixel_variance128x64_neon_dotprod(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);
unsigned int svt_aom_sub_pixel_variance128x128_neon_dotprod(const uint8_t *src,int src_stride,int xoffset, int yoffset,const uint8_t *ref, int ref_stride, unsigned int  *sse);

#if CONFIG_ENABLE_OBMC
unsigned int svt_aom_obmc_sad4x4_neon(const uint8_t *ref, int ref_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad4x8_neon(const uint8_t *ref, int ref_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad4x16_neon(const uint8_t *ref, int ref_stride, const int32_t *wsrc, const int32_t *mask);

unsigned int svt_aom_obmc_sad8x4_neon(const uint8_t *ref, int ref_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad8x8_neon(const uint8_t *ref, int ref_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad8x16_neon(const uint8_t *ref, int ref_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad8x32_neon(const uint8_t *ref, int ref_stride, const int32_t *wsrc, const int32_t *mask);

unsigned int svt_aom_obmc_sad16x4_neon(const uint8_t *ref, int ref_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad16x8_neon(const uint8_t *ref, int ref_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad16x16_neon(const uint8_t *ref, int ref_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad16x32_neon(const uint8_t *ref, int ref_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad16x64_neon(const uint8_t *ref, int ref_stride, const int32_t *wsrc, const int32_t *mask);

unsigned int svt_aom_obmc_sad32x8_neon(const uint8_t *ref, int ref_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad32x16_neon(const uint8_t *ref, int ref_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad32x32_neon(const uint8_t *ref, int ref_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad32x64_neon(const uint8_t *ref, int ref_stride, const int32_t *wsrc, const int32_t *mask);

unsigned int svt_aom_obmc_sad64x16_neon(const uint8_t *ref, int ref_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad64x32_neon(const uint8_t *ref, int ref_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad64x64_neon(const uint8_t *ref, int ref_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad64x128_neon(const uint8_t *ref, int ref_stride, const int32_t *wsrc, const int32_t *mask);

unsigned int svt_aom_obmc_sad128x64_neon(const uint8_t *ref, int ref_stride, const int32_t *wsrc, const int32_t *mask);
unsigned int svt_aom_obmc_sad128x128_neon(const uint8_t *ref, int ref_stride, const int32_t *wsrc, const int32_t *mask);

unsigned int svt_aom_obmc_variance4x4_neon(const uint8_t *pre, int pre_stride, const int32_t *wsrc,const int32_t *mask, unsigned *sse);
unsigned int svt_aom_obmc_variance4x8_neon(const uint8_t *pre, int pre_stride, const int32_t *wsrc,const int32_t *mask, unsigned *sse);
unsigned int svt_aom_obmc_variance4x16_neon(const uint8_t *pre, int pre_stride, const int32_t *wsrc,const int32_t *mask, unsigned *sse);

unsigned int svt_aom_obmc_variance8x4_neon(const uint8_t *pre, int pre_stride, const int32_t *wsrc,const int32_t *mask, unsigned *sse);
unsigned int svt_aom_obmc_variance8x8_neon(const uint8_t *pre, int pre_stride, const int32_t *wsrc,const int32_t *mask, unsigned *sse);
unsigned int svt_aom_obmc_variance8x16_neon(const uint8_t *pre, int pre_stride, const int32_t *wsrc,const int32_t *mask, unsigned *sse);
unsigned int svt_aom_obmc_variance8x32_neon(const uint8_t *pre, int pre_stride, const int32_t *wsrc,const int32_t *mask, unsigned *sse);

unsigned int svt_aom_obmc_variance16x4_neon(const uint8_t *pre, int pre_stride, const int32_t *wsrc,const int32_t *mask, unsigned *sse);
unsigned int svt_aom_obmc_variance16x8_neon(const uint8_t *pre, int pre_stride, const int32_t *wsrc,const int32_t *mask, unsigned *sse);
unsigned int svt_aom_obmc_variance16x16_neon(const uint8_t *pre, int pre_stride, const int32_t *wsrc,const int32_t *mask, unsigned *sse);
unsigned int svt_aom_obmc_variance16x32_neon(const uint8_t *pre, int pre_stride, const int32_t *wsrc,const int32_t *mask, unsigned *sse);
unsigned int svt_aom_obmc_variance16x64_neon(const uint8_t *pre, int pre_stride, const int32_t *wsrc,const int32_t *mask, unsigned *sse);

unsigned int svt_aom_obmc_variance32x8_neon(const uint8_t *pre, int pre_stride, const int32_t *wsrc,const int32_t *mask, unsigned *sse);
unsigned int svt_aom_obmc_variance32x16_neon(const uint8_t *pre, int pre_stride, const int32_t *wsrc,const int32_t *mask, unsigned *sse);
unsigned int svt_aom_obmc_variance32x32_neon(const uint8_t *pre, int pre_stride, const int32_t *wsrc,const int32_t *mask, unsigned *sse);
unsigned int svt_aom_obmc_variance32x64_neon(const uint8_t *pre, int pre_stride, const int32_t *wsrc,const int32_t *mask, unsigned *sse);

unsigned int svt_aom_obmc_variance64x16_neon(const uint8_t *pre, int pre_stride, const int32_t *wsrc,const int32_t *mask, unsigned *sse);
unsigned int svt_aom_obmc_variance64x32_neon(const uint8_t *pre, int pre_stride, const int32_t *wsrc,const int32_t *mask, unsigned *sse);
unsigned int svt_aom_obmc_variance64x64_neon(const uint8_t *pre, int pre_stride, const int32_t *wsrc,const int32_t *mask, unsigned *sse);
unsigned int svt_aom_obmc_variance64x128_neon(const uint8_t *pre, int pre_stride, const int32_t *wsrc,const int32_t *mask, unsigned *sse);

unsigned int svt_aom_obmc_variance128x64_neon(const uint8_t *pre, int pre_stride, const int32_t *wsrc,const int32_t *mask, unsigned *sse);
unsigned int svt_aom_obmc_variance128x128_neon(const uint8_t *pre, int pre_stride, const int32_t *wsrc,const int32_t *mask, unsigned *sse);

unsigned int svt_aom_obmc_sub_pixel_variance4x4_neon(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance4x8_neon(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance4x16_neon(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_sub_pixel_variance8x4_neon(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance8x8_neon(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance8x16_neon(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance8x32_neon(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_sub_pixel_variance16x4_neon(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance16x8_neon(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance16x16_neon(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance16x32_neon(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance16x64_neon(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_sub_pixel_variance32x8_neon(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance32x16_neon(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance32x32_neon(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance32x64_neon(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_sub_pixel_variance64x16_neon(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance64x32_neon(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance64x64_neon(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance64x128_neon(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_sub_pixel_variance128x64_neon(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
unsigned int svt_aom_obmc_sub_pixel_variance128x128_neon(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
#endif // CONFIG_ENABLE_OBMC

uint32_t svt_nxm_sad_kernel_helper_neon(const uint8_t *src, uint32_t src_stride, const uint8_t *ref, uint32_t ref_stride, uint32_t height, uint32_t width);
void svt_get_proj_subspace_neon(const uint8_t *src8, int32_t width, int32_t height, int32_t src_stride, const uint8_t *dat8, int32_t dat_stride, int32_t use_highbitdepth, int32_t *flt0, int32_t flt0_stride, int32_t *flt1, int32_t flt1_stride, int32_t *xq, const SgrParamsType *params);
uint64_t svt_handle_transform16x64_neon(int32_t *output);
uint64_t svt_handle_transform32x64_neon(int32_t *output);
uint64_t svt_handle_transform64x16_neon(int32_t *output);
uint64_t svt_handle_transform64x32_neon(int32_t *output);
uint64_t svt_handle_transform64x64_neon(int32_t *output);
uint64_t svt_handle_transform16x64_N2_N4_neon(int32_t *output);
uint64_t svt_handle_transform32x64_N2_N4_neon(int32_t *output);
uint64_t svt_handle_transform64x16_N2_N4_neon(int32_t *output);
uint64_t svt_handle_transform64x32_N2_N4_neon(int32_t *output);
uint64_t svt_handle_transform64x64_N2_N4_neon(int32_t *output);

void svt_av1_fwd_txfm2d_4x4_neon(int16_t *input, int32_t *coeff, uint32_t stride, TxType tx_type, uint8_t bd);
void svt_av1_fwd_txfm2d_4x8_neon(int16_t *input, int32_t *coeff, uint32_t stride, TxType tx_type, uint8_t bd);
void svt_av1_fwd_txfm2d_4x16_neon(int16_t *input, int32_t *coeff, uint32_t stride, TxType tx_type, uint8_t bd);

void svt_av1_fwd_txfm2d_8x4_neon(int16_t *input, int32_t *coeff, uint32_t stride, TxType tx_type, uint8_t bd);
void svt_av1_fwd_txfm2d_8x8_neon(int16_t *input, int32_t *coeff, uint32_t stride, TxType tx_type, uint8_t bd);
void svt_av1_fwd_txfm2d_8x16_neon(int16_t *input, int32_t *coeff, uint32_t stride, TxType tx_type, uint8_t bd);
void svt_av1_fwd_txfm2d_8x32_neon(int16_t *input, int32_t *coeff, uint32_t stride, TxType tx_type, uint8_t bd);

void svt_av1_fwd_txfm2d_16x4_neon(int16_t *input, int32_t *coeff, uint32_t stride, TxType tx_type, uint8_t bd);
void svt_av1_fwd_txfm2d_16x8_neon(int16_t *input, int32_t *coeff, uint32_t stride, TxType tx_type, uint8_t bd);
void svt_av1_fwd_txfm2d_16x16_neon(int16_t *input, int32_t *coeff, uint32_t stride, TxType tx_type, uint8_t bd);
void svt_av1_fwd_txfm2d_16x32_neon(int16_t *input, int32_t *coeff, uint32_t stride, TxType tx_type, uint8_t bd);
void svt_av1_fwd_txfm2d_16x64_neon(int16_t *input, int32_t *coeff, uint32_t stride, TxType tx_type, uint8_t bd);

void svt_av1_fwd_txfm2d_32x8_neon(int16_t *input, int32_t *coeff, uint32_t stride, TxType tx_type, uint8_t bd);
void svt_av1_fwd_txfm2d_32x16_neon(int16_t *input, int32_t *coeff, uint32_t stride, TxType tx_type, uint8_t bd);
void svt_av1_fwd_txfm2d_32x32_neon(int16_t *input, int32_t *coeff, uint32_t stride, TxType tx_type, uint8_t bd);
void svt_av1_fwd_txfm2d_32x64_neon(int16_t *input, int32_t *coeff, uint32_t stride, TxType tx_type, uint8_t bd);

void svt_av1_fwd_txfm2d_64x16_neon(int16_t *input, int32_t *coeff, uint32_t stride, TxType tx_type, uint8_t bd);
void svt_av1_fwd_txfm2d_64x32_neon(int16_t *input, int32_t *coeff, uint32_t stride, TxType tx_type, uint8_t bd);
void svt_av1_fwd_txfm2d_64x64_neon(int16_t *input, int32_t *coeff, uint32_t stride, TxType tx_type, uint8_t bd);

void svt_av1_quantize_fp_neon(const TranLow *coeff_ptr, intptr_t count, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan);
void svt_av1_quantize_fp_32x32_neon(const TranLow *coeff_ptr, intptr_t count, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan);
void svt_av1_quantize_fp_64x64_neon(const TranLow *coeff_ptr, intptr_t count, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan);
void svt_aom_quantize_b_neon(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan, const QmVal *qm_ptr, const QmVal *iqm_ptr, const int log_scale);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_aom_highbd_quantize_b_neon(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan, const QmVal *qm_ptr, const QmVal *iqm_ptr, const int32_t log_scale);
void svt_av1_highbd_quantize_fp_neon(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan, int16_t log_scale);
int64_t svt_av1_highbd_pixel_proj_error_neon(const uint8_t *src8, int32_t width, int32_t height, int32_t src_stride, const uint8_t *dat8, int32_t dat_stride, int32_t *flt0, int32_t flt0_stride, int32_t *flt1, int32_t flt1_stride, const int32_t xq[2], const SgrParamsType *params);
int64_t svt_av1_highbd_pixel_proj_error_sve(const uint8_t *src8, int32_t width, int32_t height, int32_t src_stride, const uint8_t *dat8, int32_t dat_stride, int32_t *flt0, int32_t flt0_stride, int32_t *flt1, int32_t flt1_stride, const int32_t xq[2], const SgrParamsType *params);
#endif
int64_t svt_av1_lowbd_pixel_proj_error_neon(const uint8_t *src8, int32_t width, int32_t height, int32_t src_stride, const uint8_t *dat8, int32_t dat_stride, int32_t *flt0, int32_t flt0_stride, int32_t *flt1, int32_t flt1_stride, const int32_t xq[2], const SgrParamsType *params);
int64_t svt_av1_lowbd_pixel_proj_error_sve(const uint8_t *src8, int32_t width, int32_t height, int32_t src_stride, const uint8_t *dat8, int32_t dat_stride, int32_t *flt0, int32_t flt0_stride, int32_t *flt1, int32_t flt1_stride, const int32_t xq[2], const SgrParamsType *params);

void svt_subtract_average_neon(int16_t *pred_buf_q3, int32_t width, int32_t height, int32_t round_offset, int32_t num_pel_log2);

void svt_aom_downsample_2d_neon(uint8_t *input_samples, uint32_t input_stride, uint32_t input_area_width, uint32_t input_area_height, uint8_t *decim_samples, uint32_t decim_stride, uint32_t decim_step);

void svt_av1_txb_init_levels_neon(const TranLow *const coeff, const int32_t width, const int32_t height, uint8_t *const levels);

void svt_av1_get_nz_map_contexts_neon(const uint8_t *const levels, const int16_t *const scan, const uint16_t eob, TxSize tx_size, const TxClass tx_class, int8_t *const coeff_contexts);

uint64_t svt_search_one_dual_neon(int *lev0, int *lev1, int nb_strengths, uint64_t** mse[2], int sb_count, int start_gi, int end_gi);

int32_t svt_estimate_noise_fp16_neon(const uint8_t *src, uint16_t width, uint16_t height, uint16_t stride_y);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
int32_t svt_estimate_noise_highbd_fp16_neon(const uint16_t *src, int width, int height, int stride, int bd);
#endif
uint64_t svt_aom_compute_cdef_dist_8bit_neon(const uint8_t *dst8, int32_t dstride, const uint8_t *src8,
                                                const CdefList *dlist, int32_t cdef_count, BlockSize bsize,
                                                int32_t coeff_shift, uint8_t subsampling_factor);
uint64_t svt_aom_compute_cdef_dist_8bit_neon_dotprod(const uint8_t *dst8, int32_t dstride, const uint8_t *src8,
                                                const CdefList *dlist, int32_t cdef_count, BlockSize bsize,
                                                int32_t coeff_shift, uint8_t subsampling_factor);

void svt_av1_fwd_txfm2d_4x4_N2_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_4x8_N2_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_4x16_N2_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x4_N2_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x8_N2_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x16_N2_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x32_N2_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x4_N2_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x8_N2_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x16_N2_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x32_N2_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x64_N2_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x8_N2_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x16_N2_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x32_N2_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x64_N2_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_64x16_N2_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_64x32_N2_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_64x64_N2_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);

void svt_av1_fwd_txfm2d_4x4_N4_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_4x8_N4_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_4x16_N4_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x4_N4_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x8_N4_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x16_N4_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x32_N4_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x4_N4_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x8_N4_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x16_N4_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x32_N4_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x64_N4_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x8_N4_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x16_N4_neon(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x32_N4_neon(int16_t *input, int32_t *output, uint32_t stride, TxType tx_type, uint8_t bd);
void svt_av1_fwd_txfm2d_32x64_N4_neon(int16_t *input, int32_t *output, uint32_t stride, TxType tx_type, uint8_t bd);
void svt_av1_fwd_txfm2d_64x16_N4_neon(int16_t *input, int32_t *output, uint32_t stride, TxType tx_type, uint8_t bd);
void svt_av1_fwd_txfm2d_64x32_N4_neon(int16_t *input, int32_t *output, uint32_t stride, TxType tx_type, uint8_t bd);
void svt_av1_fwd_txfm2d_64x64_N4_neon(int16_t *input, int32_t *output, uint32_t stride, TxType tx_type, uint8_t bd);

void svt_av1_apply_temporal_filter_planewise_medium_neon(struct MeContext *me_ctx, const uint8_t *y_src, int y_src_stride, const uint8_t *y_pre, int y_pre_stride, const uint8_t *u_src, const uint8_t *v_src, int uv_src_stride, const uint8_t *u_pre, const uint8_t *v_pre, int uv_pre_stride, unsigned int block_width, unsigned int block_height, int ss_x, int ss_y, uint32_t *y_accum, uint16_t *y_count, uint32_t *u_accum, uint16_t *u_count, uint32_t *v_accum, uint16_t *v_count);

void svt_av1_apply_zz_based_temporal_filter_planewise_medium_neon(
    struct MeContext *me_ctx, const uint8_t *y_pre,
    int y_pre_stride,
    const uint8_t *u_pre, const uint8_t *v_pre, int uv_pre_stride, unsigned int block_width,
    unsigned int block_height, int ss_x, int ss_y, uint32_t *y_accum, uint16_t *y_count, uint32_t *u_accum,
    uint16_t *u_count, uint32_t *v_accum, uint16_t *v_count);
void svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_neon(
    struct MeContext *me_ctx, const uint16_t *y_pre,
    int y_pre_stride,
    const uint16_t *u_pre, const uint16_t *v_pre, int uv_pre_stride, unsigned int block_width,
    unsigned int block_height, int ss_x, int ss_y, uint32_t *y_accum, uint16_t *y_count, uint32_t *u_accum,
    uint16_t *u_count, uint32_t *v_accum, uint16_t *v_count, uint32_t encoder_bit_depth);

void svt_ext_sad_calculation_32x32_64x64_neon(uint32_t *p_sad16x16, uint32_t *p_best_sad_32x32,
                                                uint32_t *p_best_sad_64x64, uint32_t *p_best_mv32x32,
                                                uint32_t *p_best_mv64x64, uint32_t mv, uint32_t *p_sad32x32);
void svt_ext_eight_sad_calculation_32x32_64x64_neon(const uint32_t p_sad16x16[16][8], uint32_t *p_best_sad_32x32,
                                                    uint32_t *p_best_sad_64x64, uint32_t *p_best_mv32x32,
                                                    uint32_t *p_best_mv64x64, uint32_t mv, uint32_t p_sad32x32[4][8]);

uint8_t svt_av1_compute_cul_level_neon(const int16_t *const scan, const int32_t *const quant_coeff, uint16_t *eob);
uint8_t svt_av1_compute_cul_level_sve(const int16_t *const scan, const int32_t *const quant_coeff, uint16_t *eob);

void svt_aom_apply_filtering_central_neon(struct MeContext *me_ctx, EbPictureBufferDesc *input_picture_ptr_central,
                                            EbByte *src, uint32_t **accum, uint16_t **count, uint16_t blk_width,
                                            uint16_t blk_height, uint32_t ss_x, uint32_t ss_y);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_aom_apply_filtering_central_highbd_neon(struct MeContext *me_ctx, EbPictureBufferDesc *input_picture_ptr_central,
                                                    uint16_t **src_16bit, uint32_t **accum, uint16_t **count,
                                                    uint16_t blk_width, uint16_t blk_height, uint32_t ss_x,
                                                    uint32_t ss_y);
#endif

int svt_aom_satd_neon(const TranLow *coeff, int length);
void svt_copy_mi_map_grid_neon(MbModeInfo** mi_grid_ptr, uint32_t mi_stride, uint8_t num_rows, uint8_t num_cols);

int64_t svt_aom_sse_neon(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, int width, int height);
int64_t svt_aom_sse_neon_dotprod(const uint8_t *src, int src_stride, const uint8_t *ref, int ref_stride, int width, int height);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
int64_t svt_aom_highbd_sse_neon(const uint8_t *a8, int a_stride, const uint8_t *b8, int b_stride, int width, int height);
int64_t svt_aom_highbd_sse_sve(const uint8_t *a8, int a_stride, const uint8_t *b8, int b_stride, int width, int height);
#endif
int64_t svt_av1_block_error_neon(const TranLow *coeff, const TranLow *dqcoeff, intptr_t block_size, int64_t *ssz);
int64_t svt_av1_block_error_sve(const TranLow *coeff, const TranLow *dqcoeff, intptr_t block_size, int64_t *ssz);
int8_t svt_av1_wedge_sign_from_residuals_neon(const int16_t *ds, const uint8_t *m, int N, int64_t limit);
int8_t svt_av1_wedge_sign_from_residuals_sve(const int16_t *ds, const uint8_t *m, int N, int64_t limit);
void svt_av1_wedge_compute_delta_squares_neon(int16_t *d, const int16_t *a, const int16_t *b, int N);
void svt_aom_get_final_filtered_pixels_neon(struct MeContext *me_ctx, EbByte *src_center_ptr_start, uint16_t **altref_buffer_highbd_start, uint32_t **accum, uint16_t **count, const uint32_t *stride, int blk_y_src_offset, int blk_ch_src_offset, uint16_t blk_width_ch, uint16_t blk_height_ch, bool is_highbd);
void svt_aom_get_final_filtered_pixels_sve(struct MeContext *me_ctx, EbByte *src_center_ptr_start, uint16_t **altref_buffer_highbd_start, uint32_t **accum, uint16_t **count, const uint32_t *stride, int blk_y_src_offset, int blk_ch_src_offset, uint16_t blk_width_ch, uint16_t blk_height_ch, bool is_highbd);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_av1_compute_stats_highbd_neon(int32_t wiener_win, const uint8_t *dgd8, const uint8_t *src8, int32_t h_start, int32_t h_end, int32_t v_start, int32_t v_end, int32_t dgd_stride, int32_t src_stride, int64_t *M, int64_t *H, EbBitDepth bit_depth);
void svt_av1_compute_stats_highbd_sve(int32_t wiener_win, const uint8_t *dgd8, const uint8_t *src8, int32_t h_start, int32_t h_end, int32_t v_start, int32_t v_end, int32_t dgd_stride, int32_t src_stride, int64_t *M, int64_t *H, EbBitDepth bit_depth);
#endif
uint64_t svt_aom_compute_cdef_dist_16bit_neon(const uint16_t* dst, int32_t dstride, const uint16_t* src, const CdefList* dlist, int32_t cdef_count, BlockSize bsize, int32_t coeff_shift, uint8_t subsampling_factor);
uint64_t svt_aom_compute_cdef_dist_16bit_sve(const uint16_t* dst, int32_t dstride, const uint16_t* src, const CdefList* dlist, int32_t cdef_count, BlockSize bsize, int32_t coeff_shift, uint8_t subsampling_factor);

void svt_unpack_and_2bcompress_neon(uint16_t *in16b_buffer, uint32_t in16b_stride, uint8_t *out8b_buffer,
                                uint32_t out8b_stride, uint8_t *out2b_buffer, uint32_t out2b_stride, uint32_t width,
                                uint32_t height);
void svt_av1_apply_temporal_filter_planewise_medium_hbd_neon(
    struct MeContext *me_ctx, const uint16_t *y_src, int y_src_stride, const uint16_t *y_pre,
    int y_pre_stride, const uint16_t *u_src, const uint16_t *v_src, int uv_src_stride,
    const uint16_t *u_pre, const uint16_t *v_pre, int uv_pre_stride, unsigned int block_width,
    unsigned int block_height, int ss_x, int ss_y, uint32_t *y_accum, uint16_t *y_count, uint32_t *u_accum,
    uint16_t *u_count, uint32_t *v_accum, uint16_t *v_count, uint32_t encoder_bit_depth);
double svt_av1_compute_cross_correlation_neon(unsigned char *im1, int stride1, int x1, int y1, unsigned char *im2, int stride2, int x2, int y2, uint8_t match_sz);
double svt_av1_compute_cross_correlation_neon_dotprod(unsigned char *im1, int stride1, int x1, int y1, unsigned char *im2, int stride2, int x2, int y2, uint8_t match_sz);
double svt_av1_compute_cross_correlation_sve(unsigned char *im1, int stride1, int x1, int y1, unsigned char *im2, int stride2, int x2, int y2, uint8_t match_sz);

#if CONFIG_ENABLE_OBMC
void svt_av1_calc_target_weighted_pred_left_neon(uint8_t is16bit, MacroBlockD* xd, int rel_mi_row, uint8_t nb_mi_height, MbModeInfo* nb_mi, void* fun_ctxt);
#endif // CONFIG_ENABLE_OBMC
#endif

#ifdef ARCH_X86_64
int64_t svt_aom_sse_avx2(const uint8_t *a, int a_stride, const uint8_t *b, int b_stride, int width, int height);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
int64_t svt_aom_highbd_sse_avx2(const uint8_t *a8, int a_stride, const uint8_t *b8, int b_stride, int width, int height);
#endif

void svt_av1_wedge_compute_delta_squares_avx2(int16_t *d, const int16_t *a, const int16_t *b, int N);


int8_t svt_av1_wedge_sign_from_residuals_sse2(const int16_t *ds, const uint8_t *m, int N, int64_t limit);
int8_t svt_av1_wedge_sign_from_residuals_avx2(const int16_t *ds, const uint8_t *m, int N, int64_t limit);
uint64_t svt_aom_compute_cdef_dist_16bit_sse4_1(const uint16_t* dst, int32_t dstride, const uint16_t* src, const CdefList* dlist, int32_t cdef_count, BlockSize bsize, int32_t coeff_shift, uint8_t subsampling_factor);
uint64_t svt_aom_compute_cdef_dist_16bit_avx2(const uint16_t* dst, int32_t dstride, const uint16_t* src, const CdefList* dlist, int32_t cdef_count, BlockSize bsize, int32_t coeff_shift, uint8_t subsampling_factor);
uint64_t svt_aom_compute_cdef_dist_8bit_sse4_1(const uint8_t* dst8, int32_t dstride, const uint8_t* src8, const CdefList* dlist, int32_t cdef_count, BlockSize bsize, int32_t coeff_shift, uint8_t subsampling_factor);
uint64_t svt_aom_compute_cdef_dist_8bit_avx2(const uint8_t* dst8, int32_t dstride, const uint8_t* src8, const CdefList* dlist, int32_t cdef_count, BlockSize bsize, int32_t coeff_shift, uint8_t subsampling_factor);
void svt_av1_compute_stats_sse4_1(int32_t wiener_win, const uint8_t *dgd8, const uint8_t *src8, int32_t h_start, int32_t h_end, int32_t v_start, int32_t v_end, int32_t dgd_stride, int32_t src_stride, int64_t *M, int64_t *H);
void svt_av1_compute_stats_avx2(int32_t wiener_win, const uint8_t *dgd8, const uint8_t *src8, int32_t h_start, int32_t h_end, int32_t v_start, int32_t v_end, int32_t dgd_stride, int32_t src_stride, int64_t *M, int64_t *H);
void svt_av1_compute_stats_avx512(int32_t wiener_win, const uint8_t *dgd8, const uint8_t *src8, int32_t h_start, int32_t h_end, int32_t v_start, int32_t v_end, int32_t dgd_stride, int32_t src_stride, int64_t *M, int64_t *H);

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_av1_compute_stats_highbd_sse4_1(int32_t wiener_win, const uint8_t *dgd8, const uint8_t *src8, int32_t h_start, int32_t h_end, int32_t v_start, int32_t v_end, int32_t dgd_stride, int32_t src_stride, int64_t *M, int64_t *H, EbBitDepth bit_depth);
void svt_av1_compute_stats_highbd_avx2(int32_t wiener_win, const uint8_t *dgd8, const uint8_t *src8, int32_t h_start, int32_t h_end, int32_t v_start, int32_t v_end, int32_t dgd_stride, int32_t src_stride, int64_t *M, int64_t *H, EbBitDepth bit_depth);
void svt_av1_compute_stats_highbd_avx512(int32_t wiener_win, const uint8_t *dgd8, const uint8_t *src8, int32_t h_start, int32_t h_end, int32_t v_start, int32_t v_end, int32_t dgd_stride, int32_t src_stride, int64_t *M, int64_t *H, EbBitDepth bit_depth);
#endif

int64_t svt_av1_lowbd_pixel_proj_error_sse4_1(const uint8_t *src8, int32_t width, int32_t height, int32_t src_stride, const uint8_t *dat8, int32_t dat_stride, int32_t *flt0, int32_t flt0_stride, int32_t *flt1, int32_t flt1_stride, const int32_t xq[2], const SgrParamsType *params);
int64_t svt_av1_lowbd_pixel_proj_error_avx2(const uint8_t *src8, int32_t width, int32_t height, int32_t src_stride, const uint8_t *dat8, int32_t dat_stride, int32_t *flt0, int32_t flt0_stride, int32_t *flt1, int32_t flt1_stride, const int32_t xq[2], const SgrParamsType *params);
int64_t svt_av1_lowbd_pixel_proj_error_avx512(const uint8_t *src8, int32_t width, int32_t height, int32_t src_stride, const uint8_t *dat8, int32_t dat_stride, int32_t *flt0, int32_t flt0_stride, int32_t *flt1, int32_t flt1_stride, const int32_t xq[2], const SgrParamsType *params);

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
int64_t svt_av1_highbd_pixel_proj_error_sse4_1(const uint8_t *src8, int32_t width, int32_t height, int32_t src_stride, const uint8_t *dat8, int32_t dat_stride, int32_t *flt0, int32_t flt0_stride, int32_t *flt1, int32_t flt1_stride, const int32_t xq[2], const SgrParamsType *params);
int64_t svt_av1_highbd_pixel_proj_error_avx2(const uint8_t *src8, int32_t width, int32_t height, int32_t src_stride, const uint8_t *dat8, int32_t dat_stride, int32_t *flt0, int32_t flt0_stride, int32_t *flt1, int32_t flt1_stride, const int32_t xq[2], const SgrParamsType *params);
#endif

void svt_subtract_average_avx2(int16_t *pred_buf_q3, int32_t width, int32_t height, int32_t round_offset, int32_t num_pel_log2);

void svt_av1_fwd_txfm2d_4x16_avx2(int16_t *input, int32_t *output, uint32_t inputStride, TxType transform_type, uint8_t  bit_depth);

void svt_av1_fwd_txfm2d_16x4_avx2(int16_t *input, int32_t *output, uint32_t inputStride, TxType transform_type, uint8_t  bit_depth);

void svt_av1_fwd_txfm2d_4x8_avx2(int16_t *input, int32_t *output, uint32_t inputStride, TxType transform_type, uint8_t  bit_depth);

void svt_av1_fwd_txfm2d_8x4_avx2(int16_t *input, int32_t *output, uint32_t inputStride, TxType transform_type, uint8_t  bit_depth);

void svt_av1_fwd_txfm2d_8x16_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);

void svt_av1_fwd_txfm2d_16x8_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);





void svt_av1_fwd_txfm2d_32x16_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x16_avx512(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);

void svt_av1_fwd_txfm2d_32x8_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);

void svt_av1_fwd_txfm2d_8x32_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);

void svt_av1_fwd_txfm2d_16x32_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x32_avx512(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);

void svt_av1_fwd_txfm2d_32x64_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x64_avx512(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);

void svt_av1_fwd_txfm2d_64x32_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_64x32_avx512(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);

void svt_av1_fwd_txfm2d_16x64_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x64_avx512(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);

void svt_av1_fwd_txfm2d_64x16_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_64x16_avx512(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);

void svt_av1_fwd_txfm2d_64x64_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_64x64_avx512(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);

void svt_av1_fwd_txfm2d_32x32_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x32_avx512(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);

void svt_av1_fwd_txfm2d_16x16_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x16_avx512(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);

void svt_av1_fwd_txfm2d_8x8_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);

void svt_av1_fwd_txfm2d_4x4_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);

void svt_av1_fwd_txfm2d_4x8_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_4x16_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);

void svt_av1_fwd_txfm2d_8x4_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x8_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x16_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x32_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);

void svt_av1_fwd_txfm2d_16x4_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x8_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x16_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x32_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x64_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);

void svt_av1_fwd_txfm2d_32x8_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x16_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x32_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x64_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);

void svt_av1_fwd_txfm2d_64x16_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_64x32_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_64x64_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);

void svt_av1_fwd_txfm2d_4x16_N2_avx2(int16_t *input, int32_t *output, uint32_t inputStride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x4_N2_avx2(int16_t *input, int32_t *output, uint32_t inputStride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_4x8_N2_avx2(int16_t *input, int32_t *output, uint32_t inputStride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x4_N2_avx2(int16_t *input, int32_t *output, uint32_t inputStride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x16_N2_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x8_N2_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x16_N2_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x8_N2_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x32_N2_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x32_N2_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x64_N2_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_64x32_N2_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x64_N2_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_64x16_N2_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_64x64_N2_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x32_N2_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x16_N2_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x8_N2_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_4x4_N2_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_4x16_N4_avx2(int16_t *input, int32_t *output, uint32_t inputStride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x4_N4_avx2(int16_t *input, int32_t *output, uint32_t inputStride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_4x8_N4_avx2(int16_t *input, int32_t *output, uint32_t inputStride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x4_N4_avx2(int16_t *input, int32_t *output, uint32_t inputStride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x16_N4_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x8_N4_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x16_N4_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x8_N4_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x32_N4_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x32_N4_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x64_N4_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_64x32_N4_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x64_N4_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_64x16_N4_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_64x64_N4_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x32_N4_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x16_N4_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x8_N4_avx2(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_4x4_N4_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void av1_fwd_txfm2d_32x32_N2_avx512(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void av1_fwd_txfm2d_32x64_N2_avx512(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void av1_fwd_txfm2d_64x32_N2_avx512(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void av1_fwd_txfm2d_64x64_N2_avx512(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void av1_fwd_txfm2d_64x64_N4_avx512(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_4x16_N2_sse4_1(int16_t *input, int32_t *output, uint32_t inputStride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x4_N2_sse4_1(int16_t *input, int32_t *output, uint32_t inputStride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_4x8_N2_sse4_1(int16_t *input, int32_t *output, uint32_t inputStride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x4_N2_sse4_1(int16_t *input, int32_t *output, uint32_t inputStride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x16_N2_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x8_N2_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x16_N2_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x8_N2_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x32_N2_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x32_N2_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x64_N2_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_64x32_N2_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x64_N2_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_64x16_N2_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_64x64_N2_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x32_N2_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x16_N2_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x8_N2_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_4x16_N4_sse4_1(int16_t *input, int32_t *output, uint32_t inputStride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x4_N4_sse4_1(int16_t *input, int32_t *output, uint32_t inputStride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_4x8_N4_sse4_1(int16_t *input, int32_t *output, uint32_t inputStride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x4_N4_sse4_1(int16_t *input, int32_t *output, uint32_t inputStride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x16_N4_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x8_N4_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x16_N4_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x8_N4_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x32_N4_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x32_N4_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x64_N4_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_64x32_N4_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x64_N4_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_64x16_N4_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_64x64_N4_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_32x32_N4_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_16x16_N4_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);
void svt_av1_fwd_txfm2d_8x8_N4_sse4_1(int16_t *input, int32_t *output, uint32_t input_stride, TxType transform_type, uint8_t  bit_depth);

void svt_get_proj_subspace_avx2(const uint8_t *src8, int width, int height, int src_stride, const uint8_t *dat8, int dat_stride, int use_highbitdepth, int32_t *flt0, int flt0_stride, int32_t *flt1, int flt1_stride, int *xq, const SgrParamsType *params);

uint64_t svt_handle_transform16x64_avx2(int32_t *output);

uint64_t svt_handle_transform32x64_avx2(int32_t *output);

uint64_t svt_handle_transform64x16_avx2(int32_t *output);

uint64_t svt_handle_transform64x32_avx2(int32_t *output);

uint64_t svt_handle_transform64x64_avx2(int32_t *output);
uint64_t svt_search_one_dual_avx2(int *lev0, int *lev1, int nb_strengths, uint64_t** mse[2], int sb_count, int start_gi, int end_gi);
uint64_t svt_search_one_dual_avx512(int *lev0, int *lev1, int nb_strengths, uint64_t** mse[2], int sb_count, int start_gi, int end_gi);
uint32_t svt_aom_mse16x16_sse2(const uint8_t *src_ptr, int32_t  source_stride, const uint8_t *ref_ptr, int32_t  recon_stride);
uint32_t svt_aom_mse16x16_avx2(const uint8_t *src_ptr, int32_t  source_stride, const uint8_t *ref_ptr, int32_t  recon_stride);

void svt_aom_quantize_b_sse4_1(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan, const QmVal *qm_ptr, const QmVal *iqm_ptr, const int32_t log_scale);
void svt_aom_quantize_b_avx2(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan, const QmVal *qm_ptr, const QmVal *iqm_ptr, const int32_t log_scale);

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_aom_highbd_quantize_b_sse4_1(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan, const QmVal *qm_ptr, const QmVal *iqm_ptr, const int32_t log_scale);
void svt_aom_highbd_quantize_b_avx2(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan, const QmVal *qm_ptr, const QmVal *iqm_ptr, const int32_t log_scale);
#endif

void svt_av1_quantize_b_qm_avx2(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan, const QmVal *qm_ptr, const QmVal *iqm_ptr, const int32_t log_scale);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_av1_highbd_quantize_b_qm_avx2(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan, const QmVal *qm_ptr, const QmVal *iqm_ptr, const int32_t log_scale);
#endif

void svt_av1_quantize_fp_sse4_1(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan);
void svt_av1_quantize_fp_avx2(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan);

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_av1_highbd_quantize_fp_sse4_1(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan, int16_t log_scale);
void svt_av1_highbd_quantize_fp_avx2(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan, int16_t log_scale);
#endif

void svt_av1_quantize_fp_32x32_sse4_1(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan);
void svt_av1_quantize_fp_32x32_avx2(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan);

void svt_av1_quantize_fp_64x64_sse4_1(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan);
void svt_av1_quantize_fp_64x64_avx2(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan);

void svt_av1_quantize_fp_qm_avx2(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan, const QmVal *qm_ptr, const QmVal *iqm_ptr, int16_t log_scale);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_av1_highbd_quantize_fp_qm_avx2(const TranLow *coeff_ptr, intptr_t n_coeffs, const int16_t *zbin_ptr, const int16_t *round_ptr, const int16_t *quant_ptr, const int16_t *quant_shift_ptr, TranLow *qcoeff_ptr, TranLow *dqcoeff_ptr, const int16_t *dequant_ptr, uint16_t *eob_ptr, const int16_t *scan, const int16_t *iscan, const QmVal *qm_ptr, const QmVal *iqm_ptr, int16_t log_scale);

uint32_t svt_aom_highbd_mse16x16_sse2(const uint8_t *src_ptr, int32_t source_stride, const uint8_t *ref_ptr, int32_t recon_stride);
#endif

/* DC_PRED top */

uint32_t svt_aom_sad128x128_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
uint32_t svt_aom_sad128x128_avx512(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);

void svt_aom_sad128x128x4d_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[4], int ref_stride, uint32_t sad_array[4]);
void svt_aom_sad128x128x4d_avx512(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[], int ref_stride, uint32_t *sad_array);

uint32_t svt_aom_sad128x64_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
uint32_t svt_aom_sad128x64_avx512(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);

void svt_aom_sad128x64x4d_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[4], int ref_stride, uint32_t sad_array[4]);
void svt_aom_sad128x64x4d_avx512(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[4], int ref_stride, uint32_t sad_array[4]);

uint32_t svt_aom_sad16x16_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);

void svt_aom_sad16x16x4d_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[4], int ref_stride, uint32_t sad_array[4]);

uint32_t svt_aom_sad16x32_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);

void svt_aom_sad16x32x4d_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[4], int ref_stride, uint32_t sad_array[4]);

uint32_t svt_aom_sad16x4_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);

void svt_aom_sad16x4x4d_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[4], int ref_stride, uint32_t sad_array[4]);

uint32_t svt_aom_sad16x64_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);

void svt_aom_sad16x64x4d_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[4], int ref_stride, uint32_t sad_array[4]);

uint32_t svt_aom_sad16x8_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);

void svt_aom_sad16x8x4d_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[4], int ref_stride, uint32_t sad_array[4]);

uint32_t svt_aom_sad32x16_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);

void svt_aom_sad32x16x4d_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[4], int ref_stride, uint32_t sad_array[4]);

uint32_t svt_aom_sad32x32_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);

void svt_aom_sad32x32x4d_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[4], int ref_stride, uint32_t sad_array[4]);

uint32_t svt_aom_sad32x64_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);

void svt_aom_sad32x64x4d_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[4], int ref_stride, uint32_t sad_array[4]);

uint32_t svt_aom_sad32x8_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);

void svt_aom_sad32x8x4d_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[4], int ref_stride, uint32_t sad_array[4]);

uint32_t svt_aom_sad4x16_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);

void svt_aom_sad4x16x4d_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[4], int ref_stride, uint32_t sad_array[4]);

uint32_t svt_aom_sad4x4_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);

void svt_aom_sad4x4x4d_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[4], int ref_stride, uint32_t sad_array[4]);

uint32_t svt_aom_sad4x8_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);

void svt_aom_sad4x8x4d_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[4], int ref_stride, uint32_t sad_array[4]);

uint32_t svt_aom_sad64x128_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
uint32_t svt_aom_sad64x128_avx512(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);

void svt_aom_sad64x128x4d_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[4], int ref_stride, uint32_t sad_array[4]);

uint32_t svt_aom_sad64x16_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
uint32_t svt_aom_sad64x16_avx512(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);

void svt_aom_sad64x16x4d_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[4], int ref_stride, uint32_t sad_array[4]);

uint32_t svt_aom_sad64x32_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
uint32_t svt_aom_sad64x32_avx512(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);

void svt_aom_sad64x32x4d_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[4], int ref_stride, uint32_t sad_array[4]);

uint32_t svt_aom_sad64x64_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);
uint32_t svt_aom_sad64x64_avx512(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);

void svt_aom_sad64x64x4d_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[4], int ref_stride, uint32_t sad_array[4]);

uint32_t svt_aom_sad8x16_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);

void svt_aom_sad8x16x4d_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[4], int ref_stride, uint32_t sad_array[4]);

uint32_t svt_aom_sad8x32_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);

void svt_aom_sad8x32x4d_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[4], int ref_stride, uint32_t sad_array[4]);

uint32_t svt_aom_sad8x4_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);

void svt_aom_sad8x4x4d_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[4], int ref_stride, uint32_t sad_array[4]);

uint32_t svt_aom_sad8x8_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t *ref_ptr, int ref_stride);

void svt_aom_sad8x8x4d_avx2(const uint8_t *src_ptr, int src_stride, const uint8_t * const ref_ptr[4], int ref_stride, uint32_t sad_array[4]);

void svt_aom_upsampled_pred_sse2(MacroBlockD* xd, const struct AV1Common* const cm, int mi_row, int mi_col, const Mv* const mv, uint8_t* comp_pred, int width, int height, int subpel_x_q3, int subpel_y_q3, const uint8_t* ref, int ref_stride, int subpel_search);

#if CONFIG_ENABLE_OBMC
unsigned int svt_aom_obmc_sad128x128_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);

unsigned int svt_aom_obmc_sad128x64_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);

unsigned int svt_aom_obmc_sad16x16_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);

unsigned int svt_aom_obmc_sad16x32_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);

unsigned int svt_aom_obmc_sad16x4_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);

unsigned int svt_aom_obmc_sad16x64_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);

unsigned int svt_aom_obmc_sad16x8_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);

unsigned int svt_aom_obmc_sad32x16_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);

unsigned int svt_aom_obmc_sad32x32_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);

unsigned int svt_aom_obmc_sad32x64_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);

unsigned int svt_aom_obmc_sad32x8_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);

unsigned int svt_aom_obmc_sad4x16_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);

unsigned int svt_aom_obmc_sad4x4_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);

unsigned int svt_aom_obmc_sad4x8_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);

unsigned int svt_aom_obmc_sad64x128_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);

unsigned int svt_aom_obmc_sad64x16_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);

unsigned int svt_aom_obmc_sad64x32_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);

unsigned int svt_aom_obmc_sad64x64_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);

unsigned int svt_aom_obmc_sad8x16_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);

unsigned int svt_aom_obmc_sad8x32_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);

unsigned int svt_aom_obmc_sad8x4_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);

unsigned int svt_aom_obmc_sad8x8_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask);

unsigned int svt_aom_obmc_sub_pixel_variance128x128_sse4_1(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_sub_pixel_variance128x64_sse4_1(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_sub_pixel_variance16x16_sse4_1(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_sub_pixel_variance16x32_sse4_1(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_sub_pixel_variance16x4_sse4_1(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_sub_pixel_variance16x64_sse4_1(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_sub_pixel_variance16x8_sse4_1(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_sub_pixel_variance32x16_sse4_1(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_sub_pixel_variance32x32_sse4_1(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_sub_pixel_variance32x64_sse4_1(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_sub_pixel_variance32x8_sse4_1(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_sub_pixel_variance4x16_sse4_1(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_sub_pixel_variance4x4_sse4_1(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_sub_pixel_variance4x8_sse4_1(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_sub_pixel_variance64x128_sse4_1(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_sub_pixel_variance64x16_sse4_1(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_sub_pixel_variance64x32_sse4_1(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_sub_pixel_variance64x64_sse4_1(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_sub_pixel_variance8x16_sse4_1(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_sub_pixel_variance8x32_sse4_1(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_sub_pixel_variance8x4_sse4_1(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_sub_pixel_variance8x8_sse4_1(const uint8_t *pre, int pre_stride, int xoffset, int yoffset, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance128x128_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance128x64_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance16x16_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance16x32_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance16x4_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance16x64_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance16x8_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance32x16_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance32x32_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance32x64_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance32x8_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance4x16_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance4x4_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance4x8_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance64x128_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance64x16_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance64x32_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance64x64_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance8x16_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance8x32_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance8x4_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance8x8_avx2(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance128x128_sse4_1(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance128x64_sse4_1(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance16x16_sse4_1(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance16x32_sse4_1(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance16x4_sse4_1(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance16x64_sse4_1(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance16x8_sse4_1(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance32x16_sse4_1(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance32x32_sse4_1(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance32x64_sse4_1(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance32x8_sse4_1(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance4x16_sse4_1(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance4x4_sse4_1(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance4x8_sse4_1(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance64x128_sse4_1(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance64x16_sse4_1(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance64x32_sse4_1(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance64x64_sse4_1(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance8x16_sse4_1(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance8x32_sse4_1(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance8x4_sse4_1(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);

unsigned int svt_aom_obmc_variance8x8_sse4_1(const uint8_t *pre, int pre_stride, const int32_t *wsrc, const int32_t *mask, unsigned int *sse);
#endif // CONFIG_ENABLE_OBMC

unsigned int svt_aom_variance4x4_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance4x8_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance4x16_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance8x4_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance8x8_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance8x16_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance8x32_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance8x4_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance8x8_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance8x16_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance8x32_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance16x4_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance16x8_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance16x16_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance16x32_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance16x64_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance32x8_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance32x16_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance32x32_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance32x64_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance64x16_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance64x32_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance64x64_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance64x128_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance128x64_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance128x128_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance32x8_avx512(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance32x16_avx512(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance32x32_avx512(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance32x64_avx512(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance64x16_avx512(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance64x32_avx512(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance64x64_avx512(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance64x128_avx512(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance128x64_avx512(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_variance128x128_avx512(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance16x4_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance16x8_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance16x16_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance16x32_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance16x64_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance32x8_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance32x16_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance32x32_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance32x64_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance64x16_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance64x32_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance64x64_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance64x128_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance128x64_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_variance128x128_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
unsigned int svt_aom_highbd_10_variance8x8_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance8x16_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance8x32_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance16x4_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance16x8_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance16x16_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance16x32_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance16x64_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance32x8_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance32x16_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance32x32_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance32x64_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance64x16_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance64x32_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance64x64_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance64x128_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance128x64_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance128x128_sse2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);

unsigned int svt_aom_highbd_10_variance8x8_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance8x16_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance8x32_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance16x8_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance16x16_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance16x32_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance16x64_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance32x8_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance32x16_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance32x32_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance32x64_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance64x16_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance64x32_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance64x64_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance64x128_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance128x64_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
unsigned int svt_aom_highbd_10_variance128x128_avx2(const uint8_t *src_ptr, int source_stride, const uint8_t *ref_ptr, int ref_stride, unsigned int *sse);
#endif

void svt_aom_ifft16x16_float_avx2(const float *input, float *temp, float *output);


void svt_aom_ifft32x32_float_avx2(const float *input, float *temp, float *output);

void svt_aom_ifft4x4_float_sse2(const float *input, float *temp, float *output);

void svt_aom_ifft8x8_float_avx2(const float *input, float *temp, float *output);

void svt_aom_fft16x16_float_avx2(const float *input, float *temp, float *output);


void svt_aom_fft32x32_float_avx2(const float *input, float *temp, float *output);

void svt_aom_fft4x4_float_sse2(const float *input, float *temp, float *output);

void svt_aom_fft8x8_float_avx2(const float *input, float *temp, float *output);

void svt_av1_get_nz_map_contexts_sse2(const uint8_t *const levels, const int16_t *const scan, const uint16_t eob, const TxSize tx_size, const TxClass tx_class, int8_t *const coeff_contexts);
void svt_av1_get_nz_map_contexts_avx2(const uint8_t *const levels, const int16_t *const scan, const uint16_t eob, const TxSize tx_size, const TxClass tx_class, int8_t *const coeff_contexts);

void svt_residual_kernel8bit_avx2(uint8_t *input, uint32_t input_stride, uint8_t *pred, uint32_t pred_stride, int16_t *residual, uint32_t residual_stride, uint32_t area_width, uint32_t area_height);
void svt_residual_kernel8bit_avx512(uint8_t *input, uint32_t input_stride, uint8_t *pred, uint32_t pred_stride, int16_t *residual, uint32_t residual_stride, uint32_t area_width, uint32_t area_height);

void svt_sad_loop_kernel_sse4_1_intrin(uint8_t *src, uint32_t src_stride, uint8_t *ref, uint32_t ref_stride, uint32_t block_height, uint32_t block_width, uint64_t *best_sad, int16_t *x_search_center, int16_t *y_search_center, uint32_t src_stride_raw, uint8_t skip_search_line, int16_t search_area_width, int16_t search_area_height);
void svt_sad_loop_kernel_avx2_intrin(uint8_t *src, uint32_t src_stride, uint8_t *ref, uint32_t ref_stride, uint32_t block_height, uint32_t block_width, uint64_t *best_sad, int16_t *x_search_center, int16_t *y_search_center, uint32_t src_stride_raw, uint8_t skip_search_line, int16_t search_area_width, int16_t search_area_height);
void svt_sad_loop_kernel_avx512_intrin(uint8_t *src, uint32_t src_stride, uint8_t *ref, uint32_t ref_stride, uint32_t block_height, uint32_t block_width, uint64_t *best_sad, int16_t *x_search_center, int16_t *y_search_center, uint32_t src_stride_raw, uint8_t skip_search_line, int16_t search_area_width, int16_t search_area_height);

void svt_av1_txb_init_levels_sse4_1(const TranLow *const coeff, const int32_t width, const int32_t height, uint8_t *const levels);
void svt_av1_txb_init_levels_avx2(const TranLow *const coeff, const int32_t width, const int32_t height, uint8_t *const levels);
void svt_av1_txb_init_levels_avx512(const TranLow *const coeff, const int32_t width, const int32_t height, uint8_t *const levels);
int svt_aom_satd_avx2(const TranLow *coeff, int length);
int64_t svt_av1_block_error_avx2(const TranLow *coeff, const TranLow *dqcoeff, intptr_t block_size, int64_t *ssz);
double svt_av1_compute_cross_correlation_sse4_1(unsigned char *im1, int stride1, int x1, int y1, unsigned char *im2, int stride2, int x2, int y2, uint8_t match_sz);
double svt_av1_compute_cross_correlation_avx2(unsigned char *im1, int stride1, int x1, int y1, unsigned char *im2, int stride2, int x2, int y2, uint8_t match_sz);
void svt_av1_k_means_dim1_avx2(const int* data, int* centroids, uint8_t* indices, int n, int k, int max_itr);

void svt_av1_k_means_dim2_avx2(const int* data, int* centroids, uint8_t* indices, int n, int k, int max_itr);

void svt_av1_calc_indices_dim1_avx2(const int* data, const int* centroids, uint8_t* indices, int n, int k);

void svt_av1_calc_indices_dim2_avx2(const int* data, const int* centroids, uint8_t* indices, int n, int k);

void svt_ext_sad_calculation_8x8_16x16_avx2_intrin(uint8_t *src, uint32_t src_stride, uint8_t *ref,
    uint32_t ref_stride, uint32_t *p_best_sad_8x8,
    uint32_t *p_best_sad_16x16, uint32_t *p_best_mv8x8,
    uint32_t *p_best_mv16x16, uint32_t mv,
    uint32_t *p_sad16x16, uint32_t *p_sad8x8,
    bool sub_sad);

void svt_ext_sad_calculation_8x8_16x16_sse4_1_intrin(uint8_t *src, uint32_t src_stride, uint8_t *ref,
    uint32_t ref_stride, uint32_t *p_best_sad_8x8,
    uint32_t *p_best_sad_16x16, uint32_t *p_best_mv8x8,
    uint32_t *p_best_mv16x16, uint32_t mv,
    uint32_t *p_sad16x16, uint32_t *p_sad8x8,
    bool sub_sad);

void svt_ext_sad_calculation_32x32_64x64_sse4_intrin(uint32_t *p_sad16x16, uint32_t *p_best_sad_32x32,
    uint32_t *p_best_sad_64x64,
    uint32_t *p_best_mv32x32, uint32_t *p_best_mv64x64,
    uint32_t mv, uint32_t *p_sad32x32);
void svt_ext_all_sad_calculation_8x8_16x16_avx2(uint8_t* src, uint32_t src_stride, uint8_t* ref,
    uint32_t ref_stride, uint32_t mv,
    uint32_t* p_best_sad_8x8, uint32_t* p_best_sad_16x16,
    uint32_t* p_best_mv8x8, uint32_t* p_best_mv16x16,
    uint32_t p_eight_sad16x16[16][8],
    uint32_t p_eight_sad8x8[64][8], bool sub_sad);
void svt_ext_all_sad_calculation_8x8_16x16_sse4_1(uint8_t* src, uint32_t src_stride, uint8_t* ref,
    uint32_t ref_stride, uint32_t mv,
    uint32_t* p_best_sad_8x8, uint32_t* p_best_sad_16x16,
    uint32_t* p_best_mv8x8, uint32_t* p_best_mv16x16,
    uint32_t p_eight_sad16x16[16][8],
    uint32_t p_eight_sad8x8[64][8], bool sub_sad);
void svt_ext_eight_sad_calculation_32x32_64x64_sse4_1(const uint32_t  p_sad16x16[16][8],
    uint32_t *p_best_sad_32x32,
    uint32_t *p_best_sad_64x64,
    uint32_t *p_best_mv32x32, uint32_t *p_best_mv64x64,
    uint32_t mv, uint32_t p_sad32x32[4][8]);
void svt_ext_eight_sad_calculation_32x32_64x64_avx2(const uint32_t  p_sad16x16[16][8],
    uint32_t *p_best_sad_32x32,
    uint32_t *p_best_sad_64x64,
    uint32_t *p_best_mv32x32, uint32_t *p_best_mv64x64,
    uint32_t mv, uint32_t p_sad32x32[4][8]);
uint32_t svt_compute4x_m_sad_avx2_intrin(
    const uint8_t *src, // input parameter, source samples Ptr
    uint32_t       src_stride, // input parameter, source stride
    const uint8_t *ref, // input parameter, reference samples Ptr
    uint32_t       ref_stride, // input parameter, reference stride
    uint32_t       height, // input parameter, block height (M)
    uint32_t       width); // input parameter, block width (N)
void svt_initialize_buffer_32bits_sse2_intrin(uint32_t *pointer, uint32_t count128,
    uint32_t count32, uint32_t value);
uint32_t svt_nxm_sad_kernel_helper_avx2(const uint8_t *src, uint32_t src_stride, const uint8_t *ref,
    uint32_t ref_stride, uint32_t height, uint32_t width);
uint32_t svt_nxm_sad_kernel_helper_sse4_1(const uint8_t *src, uint32_t src_stride, const uint8_t *ref,
    uint32_t ref_stride, uint32_t height, uint32_t width);
uint32_t nxm_sad_avg_kernel_helper_avx2(uint8_t *src, uint32_t src_stride, uint8_t *ref1,
    uint32_t ref1_stride, uint8_t *ref2, uint32_t ref2_stride,
    uint32_t height, uint32_t width);
uint32_t svt_nxm_sad_kernel_helper_avx512(const uint8_t *src, uint32_t src_stride, const uint8_t *ref,
    uint32_t ref_stride, uint32_t height, uint32_t width);
uint64_t svt_compute_mean_of_squared_values8x8_sse2_intrin(
    uint8_t* input_samples, // input parameter, input samples Ptr
    uint32_t input_stride, // input parameter, input stride
    uint32_t input_area_width, // input parameter, input area width
    uint32_t input_area_height); // input parameter, input area height
uint64_t svt_aom_compute_subd_mean_of_squared_values8x8_sse2_intrin(
    uint8_t* input_samples, // input parameter, input samples Ptr
    uint16_t input_stride);
uint64_t svt_compute_mean8x8_sse2_intrin(
    uint8_t* input_samples, // input parameter, input samples Ptr
    uint32_t input_stride, // input parameter, input stride
    uint32_t input_area_width, // input parameter, input area width
    uint32_t input_area_height); // input parameter, input area height
uint64_t svt_compute_mean8x8_avx2_intrin(
    uint8_t *input_samples, // input parameter, input samples Ptr
    uint32_t input_stride, // input parameter, input stride
    uint32_t input_area_width, // input parameter, input area width
    uint32_t input_area_height);

uint64_t svt_compute_sub_mean8x8_sse2_intrin(uint8_t* input_samples, uint16_t input_stride);

void svt_compute_interm_var_four8x8_helper_sse2(uint8_t* input_samples, uint16_t input_stride,
    uint64_t* mean_of8x8_blocks, // mean of four  8x8
    uint64_t* mean_of_squared8x8_blocks); // meanSquared
void svt_compute_interm_var_four8x8_avx2_intrin(uint8_t *input_samples, uint16_t input_stride,
    uint64_t *mean_of8x8_blocks, // mean of four  8x8
    uint64_t *mean_of_squared8x8_blocks);
uint32_t svt_aom_sad_16bit_kernel_avx2(uint16_t *src, uint32_t src_stride, uint16_t *ref,
    uint32_t ref_stride, uint32_t height, uint32_t width);

void svt_av1_apply_zz_based_temporal_filter_planewise_medium_sse4_1(
    struct MeContext *me_ctx, const uint8_t *y_pre,
    int y_pre_stride,
    const uint8_t *u_pre, const uint8_t *v_pre, int uv_pre_stride, unsigned int block_width,
    unsigned int block_height, int ss_x, int ss_y, uint32_t *y_accum, uint16_t *y_count, uint32_t *u_accum,
    uint16_t *u_count, uint32_t *v_accum, uint16_t *v_count);
void svt_av1_apply_zz_based_temporal_filter_planewise_medium_avx2(
    struct MeContext *me_ctx, const uint8_t *y_pre,
    int y_pre_stride,
    const uint8_t *u_pre, const uint8_t *v_pre, int uv_pre_stride, unsigned int block_width,
    unsigned int block_height, int ss_x, int ss_y, uint32_t *y_accum, uint16_t *y_count, uint32_t *u_accum,
    uint16_t *u_count, uint32_t *v_accum, uint16_t *v_count);
void svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_sse4_1(
    struct MeContext *me_ctx, const uint16_t *y_pre,
    int y_pre_stride,
    const uint16_t *u_pre, const uint16_t *v_pre, int uv_pre_stride, unsigned int block_width,
    unsigned int block_height, int ss_x, int ss_y, uint32_t *y_accum, uint16_t *y_count, uint32_t *u_accum,
    uint16_t *u_count, uint32_t *v_accum, uint16_t *v_count, uint32_t encoder_bit_depth);
void svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_avx2(
    struct MeContext *me_ctx, const uint16_t *y_pre,
    int y_pre_stride,
    const uint16_t *u_pre, const uint16_t *v_pre, int uv_pre_stride, unsigned int block_width,
    unsigned int block_height, int ss_x, int ss_y, uint32_t *y_accum, uint16_t *y_count, uint32_t *u_accum,
    uint16_t *u_count, uint32_t *v_accum, uint16_t *v_count, uint32_t encoder_bit_depth);
void svt_av1_apply_temporal_filter_planewise_medium_sse4_1(
    struct MeContext *me_ctx, const uint8_t *y_src, int y_src_stride, const uint8_t *y_pre,
    int y_pre_stride, const uint8_t *u_src, const uint8_t *v_src, int uv_src_stride,
    const uint8_t *u_pre, const uint8_t *v_pre, int uv_pre_stride, unsigned int block_width,
    unsigned int block_height, int ss_x, int ss_y, uint32_t *y_accum, uint16_t *y_count, uint32_t *u_accum,
    uint16_t *u_count, uint32_t *v_accum, uint16_t *v_count);
void svt_av1_apply_temporal_filter_planewise_medium_avx2(
    struct MeContext *me_ctx, const uint8_t *y_src, int y_src_stride, const uint8_t *y_pre,
    int y_pre_stride, const uint8_t *u_src, const uint8_t *v_src, int uv_src_stride,
    const uint8_t *u_pre, const uint8_t *v_pre, int uv_pre_stride, unsigned int block_width,
    unsigned int block_height, int ss_x, int ss_y, uint32_t *y_accum, uint16_t *y_count, uint32_t *u_accum,
    uint16_t *u_count, uint32_t *v_accum, uint16_t *v_count);

void svt_av1_apply_temporal_filter_planewise_medium_hbd_sse4_1(
    struct MeContext *me_ctx, const uint16_t *y_src, int y_src_stride, const uint16_t *y_pre,
    int y_pre_stride, const uint16_t *u_src, const uint16_t *v_src, int uv_src_stride,
    const uint16_t *u_pre, const uint16_t *v_pre, int uv_pre_stride, unsigned int block_width,
    unsigned int block_height, int ss_x, int ss_y, uint32_t *y_accum, uint16_t *y_count, uint32_t *u_accum,
    uint16_t *u_count, uint32_t *v_accum, uint16_t *v_count, uint32_t encoder_bit_depth);
void svt_av1_apply_temporal_filter_planewise_medium_hbd_avx2(
    struct MeContext *me_ctx, const uint16_t *y_src, int y_src_stride, const uint16_t *y_pre,
    int y_pre_stride, const uint16_t *u_src, const uint16_t *v_src, int uv_src_stride,
    const uint16_t *u_pre, const uint16_t *v_pre, int uv_pre_stride, unsigned int block_width,
    unsigned int block_height, int ss_x, int ss_y, uint32_t *y_accum, uint16_t *y_count, uint32_t *u_accum,
    uint16_t *u_count, uint32_t *v_accum, uint16_t *v_count, uint32_t encoder_bit_depth);

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
uint32_t svt_aom_variance_highbd_sse4_1(const uint16_t *a, int a_stride, const uint16_t *b, int b_stride,
                            int w, int h, uint32_t *sse);
uint32_t svt_aom_variance_highbd_avx2(const uint16_t *a, int a_stride, const uint16_t *b, int b_stride,
                            int w, int h, uint32_t *sse);
#endif
#if CONFIG_ENABLE_OBMC
void svt_av1_calc_target_weighted_pred_above_avx2(uint8_t is16bit, MacroBlockD* xd, int rel_mi_col, uint8_t nb_mi_width, MbModeInfo* nb_mi, void* fun_ctxt);
void svt_av1_calc_target_weighted_pred_left_avx2(uint8_t is16bit, MacroBlockD* xd, int rel_mi_row, uint8_t nb_mi_height, MbModeInfo* nb_mi, void* fun_ctxt);
#endif // CONFIG_ENABLE_OBMC
int32_t svt_estimate_noise_fp16_avx2(const uint8_t *src, uint16_t width, uint16_t height, uint16_t stride_y);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
int32_t svt_estimate_noise_highbd_fp16_avx2(const uint16_t *src, int width, int height, int stride, int bd);
#endif
void svt_av1_add_block_observations_internal_avx2(uint32_t n, const double val, const double recp_sqr_norm, double *buffer, double *buffer_norm, double *b, double *A);
void svt_av1_pointwise_multiply_avx2(const float *a, float *b, float *c, double *b_d, double *c_d, int32_t n);
void svt_av1_apply_window_function_to_plane_avx2(int32_t y_size, int32_t x_size, float *result_ptr, uint32_t result_stride, float *block, float *plane, const float *window_function);
void svt_aom_noise_tx_filter_avx2(int32_t block_size, float *block_ptr, const float psd);
#if CONFIG_ENABLE_FILM_GRAIN
void svt_aom_flat_block_finder_extract_block_avx2(const AomFlatBlockFinder *block_finder, const uint8_t *const data, int32_t w, int32_t h, int32_t stride, int32_t offsx, int32_t offsy, double *plane, double *block);
#endif
void svt_av1_interpolate_core_avx2(const uint8_t *const input, int in_length, uint8_t *output, int out_length, const int16_t *interp_filters);
void svt_av1_down2_symeven_avx2(const uint8_t *const input, int length, uint8_t *output);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_av1_highbd_interpolate_core_avx2(const uint16_t *const input, int in_length, uint16_t *output, int out_length, int bd, const int16_t *interp_filters);
void svt_av1_highbd_down2_symeven_avx2(const uint16_t *const input, int length, uint16_t *output, int bd);
EbErrorType svt_av1_highbd_resize_plane_avx2(const uint16_t *const input, int height, int width, int in_stride, uint16_t *output, int height2, int width2, int out_stride, int bd);
#endif
EbErrorType svt_av1_resize_plane_avx2(const uint8_t *const input, int height, int width, int in_stride, uint8_t *output, int height2, int width2, int out_stride);
uint8_t svt_av1_compute_cul_level_avx2(const int16_t* const scan, const int32_t* const quant_coeff, uint16_t* eob);
double svt_ssim_8x8_avx2(const uint8_t* s, uint32_t sp, const uint8_t* r, uint32_t rp);
double svt_ssim_4x4_avx2(const uint8_t* s, uint32_t sp, const uint8_t* r, uint32_t rp);
double svt_ssim_8x8_hbd_avx2(const uint16_t* s, uint32_t sp, const uint16_t* r, uint32_t rp);
double svt_ssim_4x4_hbd_avx2(const uint16_t* s, uint32_t sp, const uint16_t* r, uint32_t rp);
#endif

#ifdef __cplusplus
}  // extern "C"
#endif

#endif

// clang-format on
