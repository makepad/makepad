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
#ifndef COMMON_DSP_RTCD_H_
#define COMMON_DSP_RTCD_H_

#include "definitions.h"

#ifdef RTCD_C
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
#if defined ARCH_X86_64 || defined ARCH_AARCH64
EbCpuFlags svt_aom_get_cpu_flags();
EbCpuFlags svt_aom_get_cpu_flags_to_use();
#endif
void svt_aom_setup_common_rtcd_internal(EbCpuFlags flags);

void svt_av1_copy_wxh_8bit_c(uint8_t *src, uint32_t src_stride, uint8_t *dst, uint32_t dst_stride, uint32_t height, uint32_t width);
RTCD_EXTERN void(*svt_av1_copy_wxh_8bit)(uint8_t *src, uint32_t src_stride, uint8_t *dst, uint32_t dst_stride, uint32_t height, uint32_t width);
void svt_av1_copy_wxh_16bit_c(uint16_t *src, uint32_t src_stride, uint16_t *dst, uint32_t dst_stride, uint32_t height, uint32_t width);
RTCD_EXTERN void(*svt_av1_copy_wxh_16bit)(uint16_t *src, uint32_t src_stride, uint16_t *dst, uint32_t dst_stride, uint32_t height, uint32_t width);

void svt_aom_blend_a64_vmask_c(uint8_t *dst, uint32_t dst_stride, const uint8_t *src0, uint32_t src0_stride, const uint8_t *src1, uint32_t src1_stride, const uint8_t *mask, int w, int h);
RTCD_EXTERN void(*svt_aom_blend_a64_vmask)(uint8_t *dst, uint32_t dst_stride, const uint8_t *src0, uint32_t src0_stride, const uint8_t *src1, uint32_t src1_stride, const uint8_t *mask, int w, int h);
void svt_aom_blend_a64_hmask_c(uint8_t *dst, uint32_t dst_stride, const uint8_t *src0, uint32_t src0_stride, const uint8_t *src1, uint32_t src1_stride, const uint8_t *mask, int w, int h);
RTCD_EXTERN void(*svt_aom_blend_a64_hmask)(uint8_t *dst, uint32_t dst_stride, const uint8_t *src0, uint32_t src0_stride, const uint8_t *src1, uint32_t src1_stride, const uint8_t *mask, int w, int h);
void svt_aom_blend_a64_mask_c(uint8_t *dst, uint32_t dst_stride, const uint8_t *src0, uint32_t src0_stride, const uint8_t *src1, uint32_t src1_stride, const uint8_t *mask, uint32_t mask_stride, int w, int h, int subx, int suby);
RTCD_EXTERN void(*svt_aom_blend_a64_mask)(uint8_t *dst, uint32_t dst_stride, const uint8_t *src0, uint32_t src0_stride, const uint8_t *src1, uint32_t src1_stride, const uint8_t *mask, uint32_t mask_stride, int w, int h, int subx, int suby);
void svt_aom_highbd_blend_a64_mask_c(uint8_t *dst, uint32_t dst_stride, const uint8_t *src0, uint32_t src0_stride, const uint8_t *src1, uint32_t src1_stride, const uint8_t *mask, uint32_t mask_stride, int w, int h, int subx, int suby, int bd);
RTCD_EXTERN void(*svt_aom_highbd_blend_a64_mask)(uint8_t *dst, uint32_t dst_stride, const uint8_t *src0, uint32_t src0_stride, const uint8_t *src1, uint32_t src1_stride, const uint8_t *mask, uint32_t mask_stride, int w, int h, int subx, int suby, int bd);
void svt_aom_highbd_blend_a64_vmask_16bit_c(uint16_t *dst, uint32_t dst_stride, const uint16_t *src0, uint32_t src0_stride, const uint16_t *src1, uint32_t src1_stride, const uint8_t *mask, int w, int h, int bd);
RTCD_EXTERN void(*svt_aom_highbd_blend_a64_vmask_16bit)(uint16_t *dst, uint32_t dst_stride, const uint16_t *src0, uint32_t src0_stride, const uint16_t *src1, uint32_t src1_stride, const uint8_t *mask, int w, int h, int bd);
void svt_aom_highbd_blend_a64_hmask_16bit_c(uint16_t *dst, uint32_t dst_stride, const uint16_t *src0, uint32_t src0_stride, const uint16_t *src1, uint32_t src1_stride, const uint8_t *mask, int w, int h, int bd);
RTCD_EXTERN void(*svt_aom_highbd_blend_a64_hmask_16bit)(uint16_t *dst, uint32_t dst_stride, const uint16_t *src0, uint32_t src0_stride, const uint16_t *src1, uint32_t src1_stride, const uint8_t *mask, int w, int h, int bd);
void svt_cfl_predict_lbd_c(const int16_t *pred_buf_q3, uint8_t *pred, int32_t pred_stride, uint8_t *dst, int32_t dst_stride, int32_t alpha_q3, int32_t bit_depth, int32_t width, int32_t height);
RTCD_EXTERN void(*svt_cfl_predict_lbd)(const int16_t *pred_buf_q3, uint8_t *pred, int32_t pred_stride, uint8_t *dst, int32_t dst_stride, int32_t alpha_q3, int32_t bit_depth, int32_t width, int32_t height);
void svt_cfl_predict_hbd_c(const int16_t *pred_buf_q3, uint16_t *pred, int32_t pred_stride, uint16_t *dst, int32_t dst_stride, int32_t alpha_q3, int32_t bit_depth, int32_t width, int32_t height);
RTCD_EXTERN void(*svt_cfl_predict_hbd)(const int16_t *pred_buf_q3, uint16_t *pred, int32_t pred_stride, uint16_t *dst, int32_t dst_stride, int32_t alpha_q3, int32_t bit_depth, int32_t width, int32_t height);
void svt_cfl_luma_subsampling_420_lbd_c(const uint8_t *input, int32_t input_stride, int16_t *output_q3, int32_t width, int32_t height);
void svt_cfl_luma_subsampling_420_lbd_avx2(const uint8_t *input, int32_t input_stride, int16_t *output_q3, int32_t width, int32_t height);
RTCD_EXTERN void(*svt_cfl_luma_subsampling_420_lbd)(const uint8_t *input, int32_t input_stride, int16_t *output_q3, int32_t width, int32_t height);

void svt_cfl_luma_subsampling_420_hbd_c(const uint16_t *input, int32_t input_stride, int16_t *output_q3, int32_t width, int32_t height);
void svt_cfl_luma_subsampling_420_hbd_avx2(const uint16_t *input, int32_t input_stride, int16_t *output_q3, int32_t width, int32_t height);
RTCD_EXTERN void(*svt_cfl_luma_subsampling_420_hbd)(const uint16_t *input, int32_t input_stride, int16_t *output_q3, int32_t width, int32_t height);

void svt_av1_filter_intra_predictor_c(uint8_t *dst, ptrdiff_t stride, TxSize tx_size, const uint8_t *above, const uint8_t *left, int32_t mode);
RTCD_EXTERN void(*svt_av1_filter_intra_predictor) (uint8_t *dst, ptrdiff_t stride, TxSize tx_size, const uint8_t *above, const uint8_t *left, int32_t mode);
void svt_av1_filter_intra_edge_c(uint8_t *p, int32_t sz, int32_t strength);
RTCD_EXTERN void(*svt_av1_filter_intra_edge)(uint8_t *p, int32_t sz, int32_t strength);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_av1_filter_intra_edge_high_c(uint16_t *p, int32_t sz, int32_t strength);
RTCD_EXTERN void(*svt_av1_filter_intra_edge_high)(uint16_t *p, int32_t sz, int32_t strength);
#endif
void svt_av1_upsample_intra_edge_c(uint8_t *p, int32_t sz);
RTCD_EXTERN void(*svt_av1_upsample_intra_edge)(uint8_t *p, int32_t sz);
void svt_av1_upsample_intra_edge_high_c(uint16_t *p, int32_t sz, int32_t bd);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_av1_highbd_dr_prediction_z2_c(uint16_t *dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint16_t *above, const uint16_t *left, int32_t upsample_above, int32_t upsample_left, int32_t dx, int32_t dy, int32_t bd);
RTCD_EXTERN void(*svt_av1_highbd_dr_prediction_z2)(uint16_t *dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint16_t *above, const uint16_t *left, int32_t upsample_above, int32_t upsample_left, int32_t dx, int32_t dy, int32_t bd);
#endif
void svt_av1_build_compound_diffwtd_mask_d16_c(uint8_t *mask, DIFFWTD_MASK_TYPE mask_type, const CONV_BUF_TYPE *src0, int src0_stride, const CONV_BUF_TYPE *src1, int src1_stride, int h, int w, ConvolveParams *conv_params, int bd);
RTCD_EXTERN void(*svt_av1_build_compound_diffwtd_mask_d16)(uint8_t *mask, DIFFWTD_MASK_TYPE mask_type, const CONV_BUF_TYPE *src0, int src0_stride, const CONV_BUF_TYPE *src1, int src1_stride, int h, int w, ConvolveParams *conv_params, int bd);
void svt_av1_inv_txfm2d_add_4x4_c(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);
RTCD_EXTERN void(*svt_av1_inv_txfm2d_add_4x4)(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);
void svt_av1_inv_txfm2d_add_8x8_c(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);
RTCD_EXTERN void(*svt_av1_inv_txfm2d_add_8x8)(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);
void svt_av1_inv_txfm2d_add_16x16_c(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);
RTCD_EXTERN void(*svt_av1_inv_txfm2d_add_16x16)(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);
void svt_av1_inv_txfm2d_add_32x32_c(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);
RTCD_EXTERN void(*svt_av1_inv_txfm2d_add_32x32)(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);
void svt_av1_inv_txfm2d_add_64x64_c(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);
RTCD_EXTERN void(*svt_av1_inv_txfm2d_add_64x64)(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);
void svt_av1_inv_txfm2d_add_8x16_c(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
RTCD_EXTERN void(*svt_av1_inv_txfm2d_add_8x16)(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
void svt_av1_inv_txfm2d_add_16x8_c(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
RTCD_EXTERN void(*svt_av1_inv_txfm2d_add_16x8)(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_we, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
void svt_av1_inv_txfm2d_add_16x32_c(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
RTCD_EXTERN void(*svt_av1_inv_txfm2d_add_16x32)(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
void svt_av1_inv_txfm2d_add_32x16_c(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
RTCD_EXTERN void(*svt_av1_inv_txfm2d_add_32x16)(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
void svt_av1_inv_txfm2d_add_32x8_c(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
RTCD_EXTERN void(*svt_av1_inv_txfm2d_add_32x8)(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
void svt_av1_inv_txfm2d_add_8x32_c(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
RTCD_EXTERN void(*svt_av1_inv_txfm2d_add_8x32)(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
void svt_av1_inv_txfm2d_add_32x64_c(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
RTCD_EXTERN void(*svt_av1_inv_txfm2d_add_32x64)(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
void svt_av1_inv_txfm2d_add_64x32_c(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
RTCD_EXTERN void(*svt_av1_inv_txfm2d_add_64x32)(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
void svt_av1_inv_txfm2d_add_16x64_c(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
RTCD_EXTERN void(*svt_av1_inv_txfm2d_add_16x64)(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
void svt_av1_inv_txfm2d_add_64x16_c(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
RTCD_EXTERN void(*svt_av1_inv_txfm2d_add_64x16)(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
void svt_av1_inv_txfm2d_add_4x8_c(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t bd);
RTCD_EXTERN void(*svt_av1_inv_txfm2d_add_4x8)(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t bd);
void svt_av1_inv_txfm2d_add_8x4_c(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t bd);
RTCD_EXTERN void(*svt_av1_inv_txfm2d_add_8x4)(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t bd);
void svt_av1_inv_txfm2d_add_4x16_c(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t bd);
RTCD_EXTERN void(*svt_av1_inv_txfm2d_add_4x16)(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t bd);
void svt_av1_inv_txfm2d_add_16x4_c(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t bd);
RTCD_EXTERN void(*svt_av1_inv_txfm2d_add_16x4)(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t bd);
void svt_av1_inv_txfm_add_c(const TranLow *dqcoeff, uint8_t *dst_r, int32_t stride_r, uint8_t *dst_w, int32_t stride_w, const TxfmParam *txfm_param);
RTCD_EXTERN void(*svt_av1_inv_txfm_add)(const TranLow *dqcoeff, uint8_t *dst_r, int32_t stride_r, uint8_t *dst_w, int32_t stride_w, const TxfmParam *txfm_param);
RTCD_EXTERN void(*svt_compressed_packmsb)(uint8_t *in8_bit_buffer, uint32_t in8_stride, uint8_t *inn_bit_buffer, uint32_t inn_stride, uint16_t *out16_bit_buffer, uint32_t out_stride, uint32_t width, uint32_t height);
RTCD_EXTERN void(*svt_convert_8bit_to_16bit)(uint8_t *src, uint32_t src_stride, uint16_t *dst, uint32_t dst_stride, uint32_t width, uint32_t height);
void svt_convert_8bit_to_16bit_avx2(uint8_t* src, uint32_t src_stride, uint16_t* dst,uint32_t dst_stride, uint32_t width, uint32_t height);
RTCD_EXTERN void(*svt_convert_16bit_to_8bit)(uint16_t *src, uint32_t src_stride, uint8_t *dst, uint32_t dst_stride, uint32_t width, uint32_t height);
void svt_convert_16bit_to_8bit_avx2(uint16_t *src, uint32_t src_stride, uint8_t *dst, uint32_t dst_stride, uint32_t width, uint32_t height);
RTCD_EXTERN void(*svt_pack2d_16_bit_src_mul4)(uint8_t *in8_bit_buffer, uint32_t in8_stride, uint8_t *inn_bit_buffer, uint16_t *out16_bit_buffer, uint32_t inn_stride, uint32_t out_stride, uint32_t width, uint32_t height);
RTCD_EXTERN void(*svt_aom_un_pack2d_16_bit_src_mul4)(uint16_t *in16_bit_buffer, uint32_t in_stride, uint8_t *out8_bit_buffer, uint8_t *outn_bit_buffer, uint32_t out8_stride, uint32_t outn_stride, uint32_t width, uint32_t height);
void svt_residual_kernel8bit_c(uint8_t *input, uint32_t input_stride, uint8_t *pred, uint32_t pred_stride, int16_t *residual, uint32_t residual_stride, uint32_t area_width, uint32_t area_height);
RTCD_EXTERN void(*svt_residual_kernel8bit)(uint8_t *input, uint32_t input_stride, uint8_t *pred, uint32_t pred_stride, int16_t *residual, uint32_t residual_stride, uint32_t area_width, uint32_t area_height);
RTCD_EXTERN uint64_t(*compute8x8_satd_u8)(uint8_t *diff, uint64_t *dc_value, uint32_t src_stride);
RTCD_EXTERN int32_t(*sum_residual8bit)(int16_t *in_ptr, uint32_t size, uint32_t stride_in);
RTCD_EXTERN void(*svt_full_distortion_kernel_cbf_zero32_bits)(int32_t *coeff, uint32_t coeff_stride, uint64_t distortion_result[DIST_CALC_TOTAL], uint32_t area_width, uint32_t area_height);
RTCD_EXTERN void(*svt_full_distortion_kernel32_bits)(int32_t *coeff, uint32_t coeff_stride, int32_t *recon_coeff, uint32_t recon_coeff_stride, uint64_t distortion_result[DIST_CALC_TOTAL], uint32_t area_width, uint32_t area_height);
uint64_t svt_spatial_full_distortion_kernel_c(uint8_t *input, uint32_t input_offset, uint32_t input_stride, uint8_t *recon, int32_t recon_offset, uint32_t recon_stride, uint32_t area_width, uint32_t area_height);
RTCD_EXTERN uint64_t(*svt_spatial_full_distortion_kernel)(uint8_t *input, uint32_t input_offset, uint32_t input_stride, uint8_t *recon, int32_t recon_offset, uint32_t recon_stride, uint32_t area_width, uint32_t area_height);
uint64_t svt_full_distortion_kernel16_bits_c(uint8_t* input, uint32_t input_offset, uint32_t input_stride, uint8_t* recon, int32_t recon_offset, uint32_t recon_stride, uint32_t area_width, uint32_t area_height);
RTCD_EXTERN uint64_t(*svt_full_distortion_kernel16_bits)(uint8_t* input, uint32_t input_offset, uint32_t input_stride, uint8_t* recon, int32_t recon_offset, uint32_t recon_stride, uint32_t area_width, uint32_t area_height);
RTCD_EXTERN void(*svt_residual_kernel16bit)(uint16_t *input, uint32_t input_stride, uint16_t *pred, uint32_t pred_stride, int16_t *residual, uint32_t residual_stride, uint32_t area_width, uint32_t area_height);
RTCD_EXTERN void(*avc_style_luma_interpolation_filter)(EbByte ref_pic, uint32_t src_stride, EbByte dst, uint32_t dst_stride, uint32_t pu_width, uint32_t pu_height, EbByte temp_buf, uint32_t frac_pos, uint8_t choice);
void svt_av1_wiener_convolve_add_src_c(const uint8_t *const src, const ptrdiff_t src_stride, uint8_t *const dst, const ptrdiff_t dst_stride, const int16_t *const filter_x, const int16_t *const filter_y, const int32_t w, const int32_t h, const ConvolveParams *const conv_params);
RTCD_EXTERN void(*svt_av1_wiener_convolve_add_src)(const uint8_t *const src, const ptrdiff_t src_stride, uint8_t *const dst, const ptrdiff_t dst_stride, const int16_t *const filter_x, const int16_t *const filter_y, const int32_t w, const int32_t h, const ConvolveParams *const conv_params);
void svt_av1_highbd_wiener_convolve_add_src_c(const uint8_t *const src, const ptrdiff_t src_stride, uint8_t *const dst, const ptrdiff_t dst_stride, const int16_t *const filter_x, const int16_t *const filter_y, const int32_t w, const int32_t h, const ConvolveParams *const conv_params, const int32_t bd);
RTCD_EXTERN void(*svt_av1_highbd_wiener_convolve_add_src)(const uint8_t *const src, const ptrdiff_t src_stride, uint8_t *const dst, const ptrdiff_t dst_stride, const int16_t *const filter_x, const int16_t *const filter_y, const int32_t w, const int32_t h, const ConvolveParams *const conv_params, const int32_t bd);
void svt_apply_selfguided_restoration_c(const uint8_t *dat, int32_t width, int32_t height, int32_t stride, int32_t eps, const int32_t *xqd, uint8_t *dst, int32_t dst_stride, int32_t *tmpbuf, int32_t bit_depth, int32_t highbd);
RTCD_EXTERN void(*svt_apply_selfguided_restoration)(const uint8_t *dat, int32_t width, int32_t height, int32_t stride, int32_t eps, const int32_t *xqd, uint8_t *dst, int32_t dst_stride, int32_t *tmpbuf, int32_t bit_depth, int32_t highbd);
void svt_av1_selfguided_restoration_c(const uint8_t *dgd8, int32_t width, int32_t height,
    int32_t dgd_stride, int32_t *flt0, int32_t *flt1, int32_t flt_stride,
    int32_t sgr_params_idx, int32_t bit_depth, int32_t highbd);
RTCD_EXTERN void(*svt_av1_selfguided_restoration)(const uint8_t *dgd8, int32_t width, int32_t height,
    int32_t dgd_stride, int32_t *flt0, int32_t *flt1, int32_t flt_stride,
    int32_t sgr_params_idx, int32_t bit_depth, int32_t highbd);
void svt_av1_convolve_2d_copy_sr_c(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
RTCD_EXTERN void(*svt_av1_convolve_2d_copy_sr)(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
void svt_av1_convolve_2d_sr_c(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
RTCD_EXTERN void(*svt_av1_convolve_2d_sr)(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
void svt_av1_jnt_convolve_2d_copy_c(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
RTCD_EXTERN void(*svt_av1_jnt_convolve_2d_copy)(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
void svt_av1_convolve_x_sr_c(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
RTCD_EXTERN void(*svt_av1_convolve_x_sr)(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
void svt_av1_convolve_y_sr_c(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
RTCD_EXTERN void(*svt_av1_convolve_y_sr)(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
void svt_av1_convolve_2d_scale_c(const uint8_t *src, int src_stride, uint8_t *dst, int dst_stride, int w, int h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int subpel_x_qn, const int x_step_qn, const int subpel_y_q4, const int y_step_qn, ConvolveParams *conv_params);
RTCD_EXTERN void(*svt_av1_convolve_2d_scale)(const uint8_t *src, int src_stride, uint8_t *dst, int dst_stride, int w, int h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int subpel_x_qn, const int x_step_qn, const int subpel_y_q4, const int y_step_qn, ConvolveParams *conv_params);
void svt_av1_jnt_convolve_x_c(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
RTCD_EXTERN void(*svt_av1_jnt_convolve_x)(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
void svt_av1_jnt_convolve_y_c(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
RTCD_EXTERN void(*svt_av1_jnt_convolve_y)(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
void svt_av1_jnt_convolve_2d_c(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
RTCD_EXTERN void(*svt_av1_jnt_convolve_2d)(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
void svt_av1_highbd_convolve_2d_copy_sr_c(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
RTCD_EXTERN void(*svt_av1_highbd_convolve_2d_copy_sr)(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
void svt_av1_highbd_jnt_convolve_2d_copy_c(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
RTCD_EXTERN void(*svt_av1_highbd_jnt_convolve_2d_copy)(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
void svt_av1_highbd_convolve_y_sr_c(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
RTCD_EXTERN void(*svt_av1_highbd_convolve_y_sr)(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
void svt_av1_highbd_convolve_2d_sr_c(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
RTCD_EXTERN void(*svt_av1_highbd_convolve_2d_sr)(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
void svt_av1_highbd_convolve_2d_scale_c(const uint16_t *src, int src_stride, uint16_t *dst, int dst_stride, int w, int h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int subpel_x_q4, const int x_step_qn, const int subpel_y_q4, const int y_step_qn, ConvolveParams *conv_params, int bd);
RTCD_EXTERN void(*svt_av1_highbd_convolve_2d_scale)(const uint16_t *src, int src_stride, uint16_t *dst, int dst_stride, int w, int h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int subpel_x_q4, const int x_step_qn, const int subpel_y_q4, const int y_step_qn, ConvolveParams *conv_params, int bd);
void svt_av1_highbd_jnt_convolve_2d_c(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
RTCD_EXTERN void(*svt_av1_highbd_jnt_convolve_2d)(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
void svt_av1_highbd_jnt_convolve_x_c(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
RTCD_EXTERN void(*svt_av1_highbd_jnt_convolve_x)(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
void svt_av1_highbd_jnt_convolve_y_c(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
RTCD_EXTERN void(*svt_av1_highbd_jnt_convolve_y)(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
void svt_av1_highbd_convolve_x_sr_c(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
RTCD_EXTERN void(*svt_av1_highbd_convolve_x_sr)(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
void svt_aom_convolve8_horiz_c(const uint8_t *src, ptrdiff_t src_stride, uint8_t *dst, ptrdiff_t dst_stride, const int16_t *filter_x, int x_step_q4, const int16_t *filter_y, int y_step_q4, int w, int h);
RTCD_EXTERN void(*svt_aom_convolve8_horiz)(const uint8_t *src, ptrdiff_t src_stride, uint8_t *dst, ptrdiff_t dst_stride, const int16_t *filter_x, int x_step_q4, const int16_t *filter_y, int y_step_q4, int w, int h);
void svt_aom_convolve8_vert_c(const uint8_t *src, ptrdiff_t src_stride, uint8_t *dst, ptrdiff_t dst_stride, const int16_t *filter_x, int x_step_q4, const int16_t *filter_y, int y_step_q4, int w, int h);
RTCD_EXTERN void(*svt_aom_convolve8_vert)(const uint8_t *src, ptrdiff_t src_stride, uint8_t *dst, ptrdiff_t dst_stride, const int16_t *filter_x, int x_step_q4, const int16_t *filter_y, int y_step_q4, int w, int h);
void svt_av1_build_compound_diffwtd_mask_c(uint8_t *mask, DIFFWTD_MASK_TYPE mask_type, const uint8_t *src0, int src0_stride, const uint8_t *src1, int src1_stride, int h, int w);
RTCD_EXTERN void(*svt_av1_build_compound_diffwtd_mask)(uint8_t *mask, DIFFWTD_MASK_TYPE mask_type, const uint8_t *src0, int src0_stride, const uint8_t *src1, int src1_stride, int h, int w);
void svt_av1_build_compound_diffwtd_mask_highbd_c(uint8_t *mask, DIFFWTD_MASK_TYPE mask_type, const uint8_t *src0, int src0_stride, const uint8_t *src1, int src1_stride, int h, int w, int bd);
RTCD_EXTERN void(*svt_av1_build_compound_diffwtd_mask_highbd)(uint8_t *mask, DIFFWTD_MASK_TYPE mask_type, const uint8_t *src0, int src0_stride, const uint8_t *src1, int src1_stride, int h, int w, int bd);
uint64_t svt_av1_wedge_sse_from_residuals_c(const int16_t *r1, const int16_t *d, const uint8_t *m, int N);
RTCD_EXTERN uint64_t(*svt_av1_wedge_sse_from_residuals)(const int16_t *r1, const int16_t *d, const uint8_t *m, int N);
void svt_aom_subtract_block_c(int rows, int cols, int16_t *diff_ptr, ptrdiff_t diff_stride, const uint8_t *src_ptr, ptrdiff_t src_stride, const uint8_t *pred_ptr, ptrdiff_t pred_stride);
RTCD_EXTERN void(*svt_aom_subtract_block)(int rows, int cols, int16_t *diff_ptr, ptrdiff_t diff_stride, const uint8_t *src_ptr, ptrdiff_t src_stride, const uint8_t *pred_ptr, ptrdiff_t pred_stride);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_aom_highbd_subtract_block_c(int rows, int cols, int16_t *diff_ptr, ptrdiff_t diff_stride, const uint8_t *src_ptr, ptrdiff_t src_stride, const uint8_t *pred_ptr, ptrdiff_t pred_stride, int bd);
RTCD_EXTERN void(*svt_aom_highbd_subtract_block) (int rows, int cols, int16_t *diff_ptr, ptrdiff_t diff_stride, const uint8_t *src_ptr, ptrdiff_t src_stride, const uint8_t *pred_ptr, ptrdiff_t pred_stride, int bd);
#endif
void svt_aom_lowbd_blend_a64_d16_mask_c(uint8_t *dst, uint32_t dst_stride, const CONV_BUF_TYPE *src0, uint32_t src0_stride, const CONV_BUF_TYPE *src1, uint32_t src1_stride, const uint8_t *mask, uint32_t mask_stride, int w, int h, int subw, int subh, ConvolveParams *conv_params);
RTCD_EXTERN void(*svt_aom_lowbd_blend_a64_d16_mask)(uint8_t *dst, uint32_t dst_stride, const CONV_BUF_TYPE *src0, uint32_t src0_stride, const CONV_BUF_TYPE *src1, uint32_t src1_stride, const uint8_t *mask, uint32_t mask_stride, int w, int h, int subw, int subh, ConvolveParams *conv_params);
void svt_aom_highbd_blend_a64_d16_mask_c(uint8_t *dst, uint32_t dst_stride, const CONV_BUF_TYPE *src0, uint32_t src0_stride, const CONV_BUF_TYPE *src1, uint32_t src1_stride, const uint8_t *mask, uint32_t mask_stride, int w, int h, int subx, int suby, ConvolveParams *conv_params, const int bd);
RTCD_EXTERN void(*svt_aom_highbd_blend_a64_d16_mask)(uint8_t *dst, uint32_t dst_stride, const CONV_BUF_TYPE *src0, uint32_t src0_stride, const CONV_BUF_TYPE *src1, uint32_t src1_stride, const uint8_t *mask, uint32_t mask_stride, int w, int h, int subx, int suby, ConvolveParams *conv_params, const int bd);
void svt_aom_highbd_dc_128_predictor_16x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_128_predictor_16x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_16x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_128_predictor_16x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_16x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_128_predictor_16x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_16x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_128_predictor_16x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_16x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_128_predictor_16x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_32x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_128_predictor_32x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_32x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_128_predictor_32x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_32x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_128_predictor_32x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_32x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_128_predictor_32x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_4x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_128_predictor_4x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_4x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_128_predictor_4x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_4x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_128_predictor_4x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_64x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_128_predictor_64x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_64x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_128_predictor_64x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_64x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_128_predictor_64x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_8x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_128_predictor_8x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_8x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_128_predictor_8x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_8x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_128_predictor_8x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_8x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_128_predictor_8x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_16x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_left_predictor_16x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_16x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_left_predictor_16x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_16x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_left_predictor_16x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_16x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_left_predictor_16x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_16x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_left_predictor_16x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_32x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_left_predictor_32x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_32x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_left_predictor_32x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_32x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_left_predictor_32x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_32x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_left_predictor_32x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_4x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_left_predictor_4x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_4x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_left_predictor_4x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_4x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_left_predictor_4x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_64x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_left_predictor_64x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_64x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_left_predictor_64x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_64x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_left_predictor_64x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_8x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_left_predictor_8x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_8x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_left_predictor_8x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_8x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_left_predictor_8x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_8x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_left_predictor_8x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_16x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_predictor_16x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_16x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_predictor_16x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_16x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_predictor_16x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_16x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_predictor_16x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_16x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_predictor_16x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_32x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_predictor_32x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_32x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_predictor_32x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_32x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_predictor_32x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_32x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_predictor_32x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_4x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_predictor_4x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_4x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_predictor_4x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_4x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_predictor_4x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_64x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_predictor_64x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_64x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_predictor_64x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_64x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_predictor_64x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_8x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_predictor_8x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_8x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_predictor_8x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_8x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_predictor_8x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_8x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_predictor_8x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_16x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_top_predictor_16x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_16x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_top_predictor_16x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_16x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_top_predictor_16x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_16x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_top_predictor_16x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_16x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_top_predictor_16x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_32x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_top_predictor_32x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_32x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_top_predictor_32x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_32x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_top_predictor_32x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_32x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_top_predictor_32x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_4x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_top_predictor_4x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_4x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_top_predictor_4x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_4x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_top_predictor_4x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_64x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_top_predictor_64x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_64x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_top_predictor_64x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_64x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_top_predictor_64x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_8x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_top_predictor_8x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_8x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_top_predictor_8x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_8x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_top_predictor_8x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_8x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_dc_top_predictor_8x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_16x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_h_predictor_16x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_16x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_h_predictor_16x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_16x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_h_predictor_16x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_16x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_h_predictor_16x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_16x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_h_predictor_16x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_32x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_h_predictor_32x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_32x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_h_predictor_32x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_32x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_h_predictor_32x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_32x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_h_predictor_32x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_4x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_h_predictor_4x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_4x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_h_predictor_4x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_4x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_h_predictor_4x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_64x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_h_predictor_64x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_64x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_h_predictor_64x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_64x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_h_predictor_64x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_8x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_h_predictor_8x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_8x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_h_predictor_8x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_8x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_h_predictor_8x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_8x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_h_predictor_8x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_16x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_h_predictor_16x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_16x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_h_predictor_16x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_16x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_h_predictor_16x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_16x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_h_predictor_16x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_16x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_h_predictor_16x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_32x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_h_predictor_32x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_32x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_h_predictor_32x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_32x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_h_predictor_32x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_32x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_h_predictor_32x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_4x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_h_predictor_4x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_4x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_h_predictor_4x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_4x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_h_predictor_4x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_64x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_h_predictor_64x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_64x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_h_predictor_64x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_64x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_h_predictor_64x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_8x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_h_predictor_8x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_8x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_h_predictor_8x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_8x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_h_predictor_8x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_8x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_h_predictor_8x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_16x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_predictor_16x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_16x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_predictor_16x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_16x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_predictor_16x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_16x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_predictor_16x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_16x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_predictor_16x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_32x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_predictor_32x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_32x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_predictor_32x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_32x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_predictor_32x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_32x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_predictor_32x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_4x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_predictor_4x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_4x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_predictor_4x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_4x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_predictor_4x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_64x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_predictor_64x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_64x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_predictor_64x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_64x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_predictor_64x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_8x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_predictor_8x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_8x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_predictor_8x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_8x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_predictor_8x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_8x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_predictor_8x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_16x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_v_predictor_16x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_16x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_v_predictor_16x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_16x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_v_predictor_16x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_16x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_v_predictor_16x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_16x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_v_predictor_16x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_32x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_v_predictor_32x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_32x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_v_predictor_32x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_32x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_v_predictor_32x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_32x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_v_predictor_32x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_4x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_v_predictor_4x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_aom_highbd_smooth_v_predictor_4x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_v_predictor_4x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_4x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_v_predictor_4x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_64x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_v_predictor_64x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_64x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_v_predictor_64x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_64x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_v_predictor_64x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_8x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_v_predictor_8x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_8x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_v_predictor_8x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_8x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_v_predictor_8x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_8x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_smooth_v_predictor_8x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_16x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_v_predictor_16x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_16x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_v_predictor_16x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_16x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_v_predictor_16x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_16x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_v_predictor_16x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_16x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_v_predictor_16x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_32x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_v_predictor_32x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_32x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_v_predictor_32x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_32x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_v_predictor_32x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_32x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_v_predictor_32x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_4x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_v_predictor_4x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_4x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_v_predictor_4x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_4x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_v_predictor_4x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_64x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_v_predictor_64x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_64x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_v_predictor_64x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_64x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_v_predictor_64x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_8x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_v_predictor_8x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_8x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_v_predictor_8x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_8x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_v_predictor_8x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_8x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_v_predictor_8x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
#endif
void svt_av1_dr_prediction_z1_c(uint8_t *dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t *above, const uint8_t *left, int32_t upsample_above, int32_t dx, int32_t dy);
RTCD_EXTERN void(*svt_av1_dr_prediction_z1)(uint8_t *dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t *above, const uint8_t *left, int32_t upsample_above, int32_t dx, int32_t dy);
void svt_av1_dr_prediction_z2_c(uint8_t *dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t *above, const uint8_t *left, int32_t upsample_above, int32_t upsample_left, int32_t dx, int32_t dy);
RTCD_EXTERN void(*svt_av1_dr_prediction_z2)(uint8_t *dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t *above, const uint8_t *left, int32_t upsample_above, int32_t upsample_left, int32_t dx, int32_t dy);
void svt_av1_dr_prediction_z3_c(uint8_t *dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t *above, const uint8_t *left, int32_t upsample_left, int32_t dx, int32_t dy);
RTCD_EXTERN void(*svt_av1_dr_prediction_z3)(uint8_t *dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t *above, const uint8_t *left, int32_t upsample_left, int32_t dx, int32_t dy);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_av1_highbd_dr_prediction_z1_c(uint16_t *dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint16_t *above, const uint16_t *left, int32_t upsample_above, int32_t dx, int32_t dy, int32_t bd);
RTCD_EXTERN void(*svt_av1_highbd_dr_prediction_z1)(uint16_t *dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint16_t *above, const uint16_t *left, int32_t upsample_above, int32_t dx, int32_t dy, int32_t bd);
void svt_av1_highbd_dr_prediction_z3_c(uint16_t *dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint16_t *above, const uint16_t *left, int32_t upsample_left, int32_t dx, int32_t dy, int32_t bd);
RTCD_EXTERN void(*svt_av1_highbd_dr_prediction_z3)(uint16_t *dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint16_t *above, const uint16_t *left, int32_t upsample_left, int32_t dx, int32_t dy, int32_t bd);
#endif
void svt_aom_dc_predictor_4x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_predictor_4x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_8x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_predictor_8x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_16x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_predictor_16x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_32x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_predictor_32x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_64x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_predictor_64x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_16x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_predictor_16x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_16x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_predictor_16x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_16x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_predictor_16x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_16x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_predictor_16x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_32x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_predictor_32x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_32x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_predictor_32x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_32x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_predictor_32x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_4x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_predictor_4x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_4x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_predictor_4x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_64x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_predictor_64x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_64x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_predictor_64x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_8x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_predictor_8x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_8x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_predictor_8x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_8x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_predictor_8x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_4x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_left_predictor_4x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_8x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_left_predictor_8x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_16x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_left_predictor_16x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_32x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_left_predictor_32x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_64x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_left_predictor_64x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_16x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_left_predictor_16x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_16x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_left_predictor_16x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_16x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_left_predictor_16x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_16x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_left_predictor_16x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_32x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_left_predictor_32x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_32x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_left_predictor_32x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_32x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_left_predictor_32x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_4x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_left_predictor_4x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_4x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_left_predictor_4x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_64x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_left_predictor_64x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_64x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_left_predictor_64x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_8x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_left_predictor_8x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_8x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_left_predictor_8x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_8x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_left_predictor_8x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_4x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_top_predictor_4x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_8x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_top_predictor_8x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_16x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_top_predictor_16x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_32x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_top_predictor_32x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_64x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_top_predictor_64x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_16x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_top_predictor_16x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_16x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_top_predictor_16x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_16x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_top_predictor_16x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_16x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_top_predictor_16x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_32x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_top_predictor_32x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_32x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_top_predictor_32x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_32x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_top_predictor_32x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_4x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_top_predictor_4x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_4x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_top_predictor_4x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_64x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_top_predictor_64x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_64x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_top_predictor_64x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_8x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_top_predictor_8x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_8x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_top_predictor_8x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_8x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_top_predictor_8x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_4x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_128_predictor_4x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_8x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_128_predictor_8x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_16x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_128_predictor_16x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_32x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_128_predictor_32x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_64x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_128_predictor_64x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_16x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_128_predictor_16x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_16x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_128_predictor_16x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_16x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_128_predictor_16x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_16x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_128_predictor_16x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_32x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_128_predictor_32x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_32x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_128_predictor_32x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_32x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_128_predictor_32x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_4x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_128_predictor_4x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_4x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_128_predictor_4x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_64x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_128_predictor_64x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_64x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_128_predictor_64x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_8x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_128_predictor_8x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_8x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_128_predictor_8x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_8x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_dc_128_predictor_8x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_h_predictor_64x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_h_predictor_64x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_h_predictor_32x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_h_predictor_32x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_h_predictor_16x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_h_predictor_16x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_h_predictor_8x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_h_predictor_8x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_h_predictor_4x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_h_predictor_4x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_h_predictor_16x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_h_predictor_16x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_h_predictor_16x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_h_predictor_16x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_h_predictor_16x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_h_predictor_16x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_h_predictor_16x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_h_predictor_16x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_h_predictor_32x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_h_predictor_32x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_h_predictor_32x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_h_predictor_32x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_h_predictor_32x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_h_predictor_32x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_h_predictor_4x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_h_predictor_4x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_h_predictor_4x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_h_predictor_4x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_h_predictor_64x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_h_predictor_64x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_h_predictor_64x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_h_predictor_64x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_h_predictor_8x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_h_predictor_8x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_h_predictor_8x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_h_predictor_8x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_h_predictor_8x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_h_predictor_8x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_v_predictor_64x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_v_predictor_64x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_v_predictor_32x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_v_predictor_32x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_v_predictor_16x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_v_predictor_16x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_v_predictor_8x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_v_predictor_8x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_v_predictor_4x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_v_predictor_4x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_v_predictor_16x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_v_predictor_16x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_v_predictor_16x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_v_predictor_16x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_v_predictor_16x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_v_predictor_16x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_v_predictor_16x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_v_predictor_16x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_v_predictor_32x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_v_predictor_32x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_v_predictor_32x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_v_predictor_32x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_v_predictor_32x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_v_predictor_32x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_v_predictor_4x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_v_predictor_4x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_v_predictor_4x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_v_predictor_4x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_v_predictor_64x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_v_predictor_64x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_v_predictor_64x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_v_predictor_64x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_v_predictor_8x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_v_predictor_8x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_v_predictor_8x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_v_predictor_8x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_v_predictor_8x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_v_predictor_8x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_predictor_64x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_predictor_64x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_predictor_32x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_predictor_32x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_predictor_16x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_predictor_16x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_predictor_8x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_predictor_8x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_predictor_4x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_predictor_4x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_predictor_16x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_predictor_16x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_predictor_16x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_predictor_16x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_predictor_16x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_predictor_16x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_predictor_16x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_predictor_16x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_predictor_32x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_predictor_32x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_predictor_32x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_predictor_32x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_predictor_32x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_predictor_32x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_predictor_4x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_predictor_4x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_predictor_4x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_predictor_4x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_predictor_64x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_predictor_64x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_predictor_64x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_predictor_64x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_predictor_8x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_predictor_8x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_predictor_8x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_predictor_8x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_smooth_predictor_8x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_smooth_predictor_8x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_v_predictor_4x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_v_predictor_4x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_v_predictor_8x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_v_predictor_8x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_v_predictor_16x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_v_predictor_16x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_v_predictor_32x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_v_predictor_32x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_v_predictor_64x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_v_predictor_64x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_v_predictor_16x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_v_predictor_16x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_v_predictor_16x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_v_predictor_16x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_v_predictor_16x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_v_predictor_16x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_v_predictor_16x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_v_predictor_16x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_v_predictor_32x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_v_predictor_32x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_v_predictor_32x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_v_predictor_32x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_v_predictor_32x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_v_predictor_32x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_v_predictor_4x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_v_predictor_4x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_v_predictor_4x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_v_predictor_4x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_v_predictor_64x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_v_predictor_64x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_v_predictor_64x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_v_predictor_64x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_v_predictor_8x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_v_predictor_8x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_v_predictor_8x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_v_predictor_8x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_v_predictor_8x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_v_predictor_8x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_h_predictor_4x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_h_predictor_4x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_h_predictor_8x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_h_predictor_8x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_h_predictor_16x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_h_predictor_16x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_h_predictor_32x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_h_predictor_32x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_h_predictor_64x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_h_predictor_64x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_h_predictor_16x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_h_predictor_16x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_h_predictor_16x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_h_predictor_16x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_h_predictor_16x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_h_predictor_16x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_h_predictor_16x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_h_predictor_16x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_h_predictor_32x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_h_predictor_32x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_h_predictor_32x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_h_predictor_32x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_h_predictor_32x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_h_predictor_32x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_h_predictor_4x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_h_predictor_4x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_h_predictor_4x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_h_predictor_4x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_h_predictor_64x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_h_predictor_64x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_h_predictor_64x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_h_predictor_64x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_h_predictor_8x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_h_predictor_8x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_h_predictor_8x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_h_predictor_8x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_h_predictor_8x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_h_predictor_8x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_16x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_paeth_predictor_16x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_16x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_paeth_predictor_16x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_16x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_paeth_predictor_16x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_16x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_paeth_predictor_16x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_16x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_paeth_predictor_16x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_32x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_paeth_predictor_32x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_32x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_paeth_predictor_32x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_32x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_paeth_predictor_32x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_32x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_paeth_predictor_32x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_4x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_paeth_predictor_4x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_4x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_paeth_predictor_4x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_4x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_paeth_predictor_4x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_64x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_paeth_predictor_64x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_64x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_paeth_predictor_64x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_64x64_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_paeth_predictor_64x64)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_8x16_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_paeth_predictor_8x16)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_8x32_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_paeth_predictor_8x32)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_8x4_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_paeth_predictor_8x4)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_8x8_c(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
RTCD_EXTERN void(*svt_aom_paeth_predictor_8x8)(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_highbd_paeth_predictor_16x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
RTCD_EXTERN void(*svt_aom_highbd_paeth_predictor_16x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
void svt_aom_highbd_paeth_predictor_16x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
RTCD_EXTERN void(*svt_aom_highbd_paeth_predictor_16x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
void svt_aom_highbd_paeth_predictor_16x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
RTCD_EXTERN void(*svt_aom_highbd_paeth_predictor_16x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
void svt_aom_highbd_paeth_predictor_16x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
RTCD_EXTERN void(*svt_aom_highbd_paeth_predictor_16x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
void svt_aom_highbd_paeth_predictor_16x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
RTCD_EXTERN void(*svt_aom_highbd_paeth_predictor_16x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
void svt_aom_highbd_paeth_predictor_32x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
RTCD_EXTERN void(*svt_aom_highbd_paeth_predictor_32x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
void svt_aom_highbd_paeth_predictor_32x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
RTCD_EXTERN void(*svt_aom_highbd_paeth_predictor_32x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
void svt_aom_highbd_paeth_predictor_32x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
RTCD_EXTERN void(*svt_aom_highbd_paeth_predictor_32x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
void svt_aom_highbd_paeth_predictor_32x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
RTCD_EXTERN void(*svt_aom_highbd_paeth_predictor_32x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
void svt_aom_highbd_paeth_predictor_4x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
RTCD_EXTERN void(*svt_aom_highbd_paeth_predictor_4x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
void svt_aom_highbd_paeth_predictor_4x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
RTCD_EXTERN void(*svt_aom_highbd_paeth_predictor_4x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
void svt_aom_highbd_paeth_predictor_4x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
RTCD_EXTERN void(*svt_aom_highbd_paeth_predictor_4x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
void svt_aom_highbd_paeth_predictor_64x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
RTCD_EXTERN void(*svt_aom_highbd_paeth_predictor_64x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
void svt_aom_highbd_paeth_predictor_64x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
RTCD_EXTERN void(*svt_aom_highbd_paeth_predictor_64x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
void svt_aom_highbd_paeth_predictor_64x64_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
RTCD_EXTERN void(*svt_aom_highbd_paeth_predictor_64x64)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
void svt_aom_highbd_paeth_predictor_8x16_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
RTCD_EXTERN void(*svt_aom_highbd_paeth_predictor_8x16)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
void svt_aom_highbd_paeth_predictor_8x32_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
RTCD_EXTERN void(*svt_aom_highbd_paeth_predictor_8x32)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
void svt_aom_highbd_paeth_predictor_8x4_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
RTCD_EXTERN void(*svt_aom_highbd_paeth_predictor_8x4)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
void svt_aom_highbd_paeth_predictor_8x8_c(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
RTCD_EXTERN void(*svt_aom_highbd_paeth_predictor_8x8)(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);
uint64_t svt_aom_sum_squares_i16_c(const int16_t *src, uint32_t N);
RTCD_EXTERN uint64_t(*svt_aom_sum_squares_i16)(const int16_t *src, uint32_t N);

uint8_t svt_aom_cdef_find_dir_c(const uint16_t *img, int32_t stride, int32_t *var, int32_t coeff_shift);
uint8_t svt_aom_cdef_find_dir_sse4_1(const uint16_t *img, int32_t stride, int32_t *var, int32_t coeff_shift);
uint8_t svt_aom_cdef_find_dir_avx2(const uint16_t *img, int32_t stride, int32_t *var, int32_t coeff_shift);
RTCD_EXTERN uint8_t(*svt_aom_cdef_find_dir)(const uint16_t *img, int32_t stride, int32_t *var, int32_t coeff_shift);

void svt_aom_cdef_find_dir_dual_c(const uint16_t *img1, const uint16_t *img2, int stride, int32_t *var1, int32_t *var2, int32_t coeff_shift, uint8_t *out1, uint8_t *out2);
void svt_aom_cdef_find_dir_dual_sse4_1(const uint16_t *img1, const uint16_t *img2, int stride, int32_t *var1, int32_t *var2, int32_t coeff_shift, uint8_t *out1, uint8_t *out2);
void svt_aom_cdef_find_dir_dual_avx2(const uint16_t *img1, const uint16_t *img2, int stride, int32_t *var1, int32_t *var2, int32_t coeff_shift, uint8_t *out1, uint8_t *out2);
RTCD_EXTERN void(*svt_aom_cdef_find_dir_dual)(const uint16_t *img1, const uint16_t *img2, int stride, int32_t *var1, int32_t *var2, int32_t coeff_shift, uint8_t *out1, uint8_t *out2);

void svt_cdef_filter_block_c(uint8_t *dst8, uint16_t *dst16, int32_t dstride, const uint16_t *in, int32_t pri_strength, int32_t sec_strength, int32_t dir, int32_t pri_damping, int32_t sec_damping, int32_t bsize, int32_t coeff_shift, uint8_t subsampling_factor);
RTCD_EXTERN void(*svt_cdef_filter_block)(uint8_t *dst8, uint16_t *dst16, int32_t dstride, const uint16_t *in, int32_t pri_strength, int32_t sec_strength, int32_t dir, int32_t pri_damping, int32_t sec_damping, int32_t bsize, int32_t coeff_shift, uint8_t subsampling_factor);
RTCD_EXTERN void(*svt_cdef_filter_block_8xn_16)(const uint16_t *const in, const int32_t pri_strength, const int32_t sec_strength, const int32_t dir, int32_t pri_damping, int32_t sec_damping, const int32_t coeff_shift, uint16_t *const dst, const int32_t dstride, uint8_t height, uint8_t subsampling_factor);

void svt_aom_copy_rect8_8bit_to_16bit_c(uint16_t *dst, int32_t dstride, const uint8_t *src, int32_t sstride, int32_t v, int32_t h);
void svt_aom_copy_rect8_8bit_to_16bit_sse4_1(uint16_t *dst, int32_t dstride, const uint8_t *src, int32_t sstride, int32_t v, int32_t h);
void svt_aom_copy_rect8_8bit_to_16bit_avx2(uint16_t *dst, int32_t dstride, const uint8_t *src, int32_t sstride, int32_t v, int32_t h);
RTCD_EXTERN void(*svt_aom_copy_rect8_8bit_to_16bit)(uint16_t *dst, int32_t dstride, const uint8_t *src, int32_t sstride, int32_t v, int32_t h);

void svt_av1_highbd_warp_affine_c(const int32_t *mat, const uint8_t *ref8b, const uint8_t *ref2b, int width, int height, int stride8b, int stride2b, uint16_t *pred, int p_col, int p_row, int p_width, int p_height, int p_stride, int subsampling_x, int subsampling_y, int bd, ConvolveParams *conv_params, int16_t alpha, int16_t beta, int16_t gamma, int16_t delta);
RTCD_EXTERN void(*svt_av1_highbd_warp_affine)(const int32_t *mat, const uint8_t *ref8b, const uint8_t *ref2b, int width, int height, int stride8b, int stride2b, uint16_t *pred, int p_col, int p_row, int p_width, int p_height, int p_stride, int subsampling_x, int subsampling_y, int bd, ConvolveParams *conv_params, int16_t alpha, int16_t beta, int16_t gamma, int16_t delta);

void svt_av1_warp_affine_c(const int32_t *mat, const uint8_t *ref, int width, int height, int stride, uint8_t *pred, int p_col, int p_row, int p_width, int p_height, int p_stride, int subsampling_x, int subsampling_y, ConvolveParams *conv_params, int16_t alpha, int16_t beta, int16_t gamma, int16_t delta);
RTCD_EXTERN void(*svt_av1_warp_affine)(const int32_t *mat, const uint8_t *ref, int width, int height, int stride, uint8_t *pred, int p_col, int p_row, int p_width, int p_height, int p_stride, int subsampling_x, int subsampling_y, ConvolveParams *conv_params, int16_t alpha, int16_t beta, int16_t gamma, int16_t delta);
void svt_aom_highbd_lpf_horizontal_14_c(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_lpf_horizontal_14)(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);
void svt_aom_highbd_lpf_horizontal_4_c(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_lpf_horizontal_4)(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);
void svt_aom_highbd_lpf_horizontal_6_c(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_lpf_horizontal_6)(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);
void svt_aom_highbd_lpf_horizontal_8_c(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_lpf_horizontal_8)(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);
void svt_aom_highbd_lpf_vertical_14_c(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_lpf_vertical_14)(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);
void svt_aom_highbd_lpf_vertical_4_c(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_lpf_vertical_4)(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);
void svt_aom_highbd_lpf_vertical_6_c(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_lpf_vertical_6)(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);
void svt_aom_highbd_lpf_vertical_8_c(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);
RTCD_EXTERN void(*svt_aom_highbd_lpf_vertical_8)(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);
void svt_aom_lpf_horizontal_14_c(uint8_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);
RTCD_EXTERN void(*svt_aom_lpf_horizontal_14)(uint8_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);
void svt_aom_lpf_horizontal_4_c(uint8_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);
RTCD_EXTERN void(*svt_aom_lpf_horizontal_4)(uint8_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);
void svt_aom_lpf_horizontal_6_c(uint8_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);
RTCD_EXTERN void(*svt_aom_lpf_horizontal_6)(uint8_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);
void svt_aom_lpf_horizontal_8_c(uint8_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);
RTCD_EXTERN void(*svt_aom_lpf_horizontal_8)(uint8_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);
void svt_aom_lpf_vertical_14_c(uint8_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);
RTCD_EXTERN void(*svt_aom_lpf_vertical_14)(uint8_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);
void svt_aom_lpf_vertical_4_c(uint8_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);
RTCD_EXTERN void(*svt_aom_lpf_vertical_4)(uint8_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);
void svt_aom_lpf_vertical_6_c(uint8_t* s, int32_t pitch, const uint8_t* blimit, const uint8_t* limit, const uint8_t* thresh);
RTCD_EXTERN void(*svt_aom_lpf_vertical_6)(uint8_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);
void svt_aom_lpf_vertical_8_c(uint8_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);
RTCD_EXTERN void(*svt_aom_lpf_vertical_8)(uint8_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);
uint32_t svt_aom_log2f_32(uint32_t x);
RTCD_EXTERN uint32_t(*svt_log2f)(uint32_t x);
void svt_memcpy_c(void  *dst_ptr, void  const*src_ptr, size_t size);
RTCD_EXTERN void (*svt_memcpy)(void  *dst_ptr, void  const*src_ptr, size_t size);
void svt_memset_c(void *dst_ptr, int c, size_t size);
RTCD_EXTERN void (*svt_memset)(void *dst_ptr, int c, size_t size);

void svt_aom_hadamard_16x16_c(const int16_t* src_diff, ptrdiff_t src_stride, int32_t* coeff);
void svt_aom_hadamard_16x16_avx2(const int16_t* src_diff, ptrdiff_t src_stride, int32_t* coeff);
RTCD_EXTERN void (*svt_aom_hadamard_16x16)(const int16_t* src_diff, ptrdiff_t src_stride, int32_t* coeff);

void svt_aom_hadamard_32x32_c(const int16_t* src_diff, ptrdiff_t src_stride, int32_t* coeff);
void svt_aom_hadamard_32x32_avx2(const int16_t* src_diff, ptrdiff_t src_stride, int32_t* coeff);
RTCD_EXTERN void (*svt_aom_hadamard_32x32)(const int16_t* src_diff, ptrdiff_t src_stride, int32_t* coeff);

void svt_aom_hadamard_8x8_c(const int16_t* src_diff, ptrdiff_t src_stride, int32_t* coeff);
void svt_aom_hadamard_8x8_sse2(const int16_t* src_diff, ptrdiff_t src_stride, int32_t* coeff);
RTCD_EXTERN void (*svt_aom_hadamard_8x8)(const int16_t* src_diff, ptrdiff_t src_stride, int32_t* coeff);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_aom_highbd_hadamard_8x8_c(const int16_t *src_diff, ptrdiff_t src_stride, int32_t *coeff);
RTCD_EXTERN void (*svt_aom_highbd_hadamard_8x8)(const int16_t *src_diff, ptrdiff_t src_stride, int32_t *coeff);
#endif
void svt_aom_hadamard_4x4_c(const int16_t* src_diff, ptrdiff_t src_stride, int32_t* coeff);
RTCD_EXTERN void (*svt_aom_hadamard_4x4)(const int16_t *src_diff, ptrdiff_t src_stride, int32_t *coeff);

#ifdef ARCH_AARCH64
void svt_av1_copy_wxh_8bit_neon(uint8_t *src, uint32_t src_stride, uint8_t *dst, uint32_t dst_stride, uint32_t height, uint32_t width);
void svt_av1_copy_wxh_16bit_neon(uint16_t *src, uint32_t src_stride, uint16_t *dst, uint32_t dst_stride, uint32_t height, uint32_t width);
void svt_memcpy_neon(void *dst_ptr, void const *src_ptr, size_t size);
void svt_memset_neon(void *dst_ptr, int c, size_t size);

void svt_av1_build_compound_diffwtd_mask_neon(uint8_t *mask, DIFFWTD_MASK_TYPE mask_type, const uint8_t *src0, int src0_stride, const uint8_t *src1, int src1_stride, int h, int w);
void svt_av1_build_compound_diffwtd_mask_d16_neon(uint8_t *mask, DIFFWTD_MASK_TYPE mask_type, const CONV_BUF_TYPE *src0, int src0_stride, const CONV_BUF_TYPE *src1, int src1_stride, int h, int w, ConvolveParams *conv_params, int bd);
void svt_av1_build_compound_diffwtd_mask_highbd_neon(uint8_t *mask, DIFFWTD_MASK_TYPE mask_type, const uint8_t *src0, int src0_stride, const uint8_t *src1, int src1_stride, int h, int w, int bd);

void svt_av1_convolve_2d_sr_neon(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);

void svt_av1_convolve_2d_sr_neon_dotprod(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);

void svt_av1_convolve_2d_sr_neon_i8mm(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);

void svt_av1_jnt_convolve_2d_neon(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);

void svt_av1_jnt_convolve_2d_neon_dotprod(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);

void svt_av1_jnt_convolve_2d_neon_i8mm(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);

void svt_aom_convolve8_horiz_neon(const uint8_t *src, ptrdiff_t src_stride, uint8_t *dst, ptrdiff_t dst_stride, const int16_t *filter_x, int x_step_q4, const int16_t *filter_y, int y_step_q4, int w, int h);

void svt_aom_convolve8_horiz_neon_dotprod(const uint8_t *src, ptrdiff_t src_stride, uint8_t *dst, ptrdiff_t dst_stride, const int16_t *filter_x, int x_step_q4, const int16_t *filter_y, int y_step_q4, int w, int h);

void svt_aom_convolve8_horiz_neon_i8mm(const uint8_t *src, ptrdiff_t src_stride, uint8_t *dst, ptrdiff_t dst_stride, const int16_t *filter_x, int x_step_q4, const int16_t *filter_y, int y_step_q4, int w, int h);

void svt_aom_convolve8_vert_neon(const uint8_t *src, ptrdiff_t src_stride, uint8_t *dst, ptrdiff_t dst_stride, const int16_t *filter_x, int x_step_q4, const int16_t *filter_y, int y_step_q4, int w, int h);

void svt_aom_convolve8_vert_neon_dotprod(const uint8_t *src, ptrdiff_t src_stride, uint8_t *dst, ptrdiff_t dst_stride, const int16_t *filter_x, int x_step_q4, const int16_t *filter_y, int y_step_q4, int w, int h);

void svt_aom_convolve8_vert_neon_i8mm(const uint8_t *src, ptrdiff_t src_stride, uint8_t *dst, ptrdiff_t dst_stride, const int16_t *filter_x, int x_step_q4, const int16_t *filter_y, int y_step_q4, int w, int h);

void svt_av1_convolve_x_sr_neon(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);

void svt_av1_convolve_x_sr_neon_dotprod(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);

void svt_av1_convolve_x_sr_neon_i8mm(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);

void svt_av1_convolve_y_sr_neon(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);

void svt_av1_convolve_y_sr_neon_dotprod(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);

void svt_av1_convolve_y_sr_neon_i8mm(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);

void svt_av1_jnt_convolve_x_neon(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);

void svt_av1_jnt_convolve_x_neon_dotprod(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);

void svt_av1_jnt_convolve_x_neon_i8mm(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);

void svt_av1_jnt_convolve_y_neon(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);

void svt_av1_jnt_convolve_y_neon_dotprod(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);

void svt_av1_jnt_convolve_y_neon_i8mm(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);

void svt_av1_jnt_convolve_2d_copy_neon(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);

void svt_av1_convolve_2d_scale_neon(const uint8_t *src, int src_stride, uint8_t *dst, int dst_stride, int w, int h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int subpel_x_qn, const int x_step_qn, const int subpel_y_q4, const int y_step_qn, ConvolveParams *conv_params);

void svt_av1_convolve_2d_scale_neon_dotprod(const uint8_t *src, int src_stride, uint8_t *dst, int dst_stride, int w, int h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int subpel_x_qn, const int x_step_qn, const int subpel_y_q4, const int y_step_qn, ConvolveParams *conv_params);

void svt_av1_convolve_2d_scale_neon_i8mm(const uint8_t *src, int src_stride, uint8_t *dst, int dst_stride, int w, int h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int subpel_x_qn, const int x_step_qn, const int subpel_y_q4, const int y_step_qn, ConvolveParams *conv_params);

void svt_av1_highbd_convolve_2d_scale_neon(const uint16_t *src, int src_stride, uint16_t *dst, int dst_stride, int w, int h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int subpel_x_q4, const int x_step_qn, const int subpel_y_q4, const int y_step_qn, ConvolveParams *conv_params, int bd);

void svt_av1_warp_affine_neon(const int32_t *mat, const uint8_t *ref, int width, int height, int stride, uint8_t *pred, int p_col, int p_row, int p_width, int p_height, int p_stride, int subsampling_x, int subsampling_y, ConvolveParams *conv_params, int16_t alpha, int16_t beta, int16_t gamma, int16_t delta);

void svt_av1_warp_affine_neon_i8mm(const int32_t *mat, const uint8_t *ref, int width, int height, int stride, uint8_t *pred, int p_col, int p_row, int p_width, int p_height, int p_stride, int subsampling_x, int subsampling_y, ConvolveParams *conv_params, int16_t alpha, int16_t beta, int16_t gamma, int16_t delta);

void svt_av1_warp_affine_sve(const int32_t *mat, const uint8_t *ref, int width, int height, int stride, uint8_t *pred, int p_col, int p_row, int p_width, int p_height, int p_stride, int subsampling_x, int subsampling_y, ConvolveParams *conv_params, int16_t alpha, int16_t beta, int16_t gamma, int16_t delta);

void svt_av1_highbd_warp_affine_neon(const int32_t *mat, const uint8_t *ref8b, const uint8_t *ref2b,  int width, int height, int stride8b, int stride2b, uint16_t *pred, int p_col, int p_row, int p_width, int p_height, int p_stride, int subsampling_x, int subsampling_y, int bd, ConvolveParams *conv_params, int16_t alpha, int16_t beta, int16_t gamma, int16_t delta);

void svt_av1_highbd_warp_affine_sve(const int32_t *mat, const uint8_t *ref8b, const uint8_t *ref2b,  int width, int height, int stride8b, int stride2b, uint16_t *pred, int p_col, int p_row, int p_width, int p_height, int p_stride, int subsampling_x, int subsampling_y, int bd, ConvolveParams *conv_params, int16_t alpha, int16_t beta, int16_t gamma, int16_t delta);

uint64_t svt_spatial_full_distortion_kernel_neon(uint8_t *input, uint32_t input_offset, uint32_t input_stride, uint8_t *recon, int32_t recon_offset, uint32_t recon_stride, uint32_t area_width, uint32_t area_height);
uint64_t svt_spatial_full_distortion_kernel_neon_dotprod(uint8_t *input, uint32_t input_offset, uint32_t input_stride, uint8_t *recon, int32_t recon_offset, uint32_t recon_stride, uint32_t area_width, uint32_t area_height);

void svt_av1_wiener_convolve_add_src_neon(const uint8_t *const src, const ptrdiff_t src_stride, uint8_t *const dst, const ptrdiff_t dst_stride, const int16_t *const filter_x, const int16_t *const filter_y, const int32_t w, const int32_t h, const ConvolveParams *const conv_params);

void svt_av1_convolve_2d_copy_sr_neon(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);

void svt_aom_blend_a64_hmask_neon(uint8_t *dst, uint32_t dst_stride, const uint8_t *src0, uint32_t src0_stride, const uint8_t *src1, uint32_t src1_stride, const uint8_t *mask, int w, int h);
void svt_aom_blend_a64_vmask_neon(uint8_t *dst, uint32_t dst_stride, const uint8_t *src0, uint32_t src0_stride, const uint8_t *src1, uint32_t src1_stride, const uint8_t *mask, int w, int h);
void svt_aom_lowbd_blend_a64_d16_mask_neon(uint8_t *dst, uint32_t dst_stride, const CONV_BUF_TYPE *src0, uint32_t src0_stride, const CONV_BUF_TYPE *src1, uint32_t src1_stride, const uint8_t *mask, uint32_t mask_stride, int w, int h, int subw, int subh, ConvolveParams *conv_params);
void svt_aom_blend_a64_mask_neon(uint8_t *dst, uint32_t dst_stride, const uint8_t *src0, uint32_t src0_stride, const uint8_t *src1, uint32_t src1_stride, const uint8_t *mask, uint32_t mask_stride, int w, int h, int subw, int subh);

void svt_aom_highbd_blend_a64_mask_neon(uint8_t *dst, uint32_t dst_stride, const uint8_t *src0, uint32_t src0_stride, const uint8_t *src1, uint32_t src1_stride, const uint8_t *mask, uint32_t mask_stride, int w, int h, int subx, int suby, int bd);
void svt_aom_highbd_blend_a64_d16_mask_neon(uint8_t *dst, uint32_t dst_stride, const CONV_BUF_TYPE *src0, uint32_t src0_stride, const CONV_BUF_TYPE *src1, uint32_t src1_stride, const uint8_t *mask, uint32_t mask_stride, int w, int h, int subx, int suby, ConvolveParams *conv_params, const int bd);
void svt_aom_highbd_blend_a64_hmask_16bit_neon(uint16_t *dst, uint32_t dst_stride, const uint16_t *src0, uint32_t src0_stride, const uint16_t *src1, uint32_t src1_stride, const uint8_t *mask, int w, int h, int bd);
void svt_aom_highbd_blend_a64_vmask_16bit_neon(uint16_t *dst, uint32_t dst_stride, const uint16_t *src0, uint32_t src0_stride, const uint16_t *src1, uint32_t src1_stride, const uint8_t *mask, int w, int h, int bd);

uint64_t svt_aom_sum_squares_i16_neon(const int16_t *src, uint32_t n);
uint64_t svt_aom_sum_squares_i16_sve(const int16_t *src, uint32_t n);

void svt_av1_selfguided_restoration_neon(const uint8_t *dat8, int32_t width, int32_t height, int32_t stride, int32_t *flt0, int32_t *flt1, int32_t flt_stride, int32_t sgr_params_idx, int32_t bit_depth, int32_t highbd);

void svt_av1_filter_intra_edge_neon(uint8_t *p, int32_t sz, int32_t strength);
void svt_av1_filter_intra_edge_high_neon(uint16_t *p, int32_t sz, int32_t strength);
void svt_av1_upsample_intra_edge_neon(uint8_t *p, int32_t sz);

void svt_av1_dr_prediction_z1_neon(uint8_t *dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t *above, const uint8_t *left,int32_t upsample_above, int32_t dx, int32_t dy);
void svt_av1_dr_prediction_z2_neon(uint8_t *dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t *above, const uint8_t *left, int32_t upsample_above, int32_t upsample_left, int32_t dx,int32_t dy);
void svt_av1_dr_prediction_z3_neon(uint8_t *dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t *above, const uint8_t *left, int32_t upsample_left, int32_t dx, int32_t dy);

void svt_av1_highbd_dr_prediction_z1_neon(uint16_t *dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint16_t *above, const uint16_t *left, int32_t upsample_above, int32_t dx, int32_t dy, int32_t bd);
void svt_av1_highbd_dr_prediction_z2_neon(uint16_t *dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint16_t *above, const uint16_t *left, int32_t upsample_above, int32_t upsample_left, int32_t dx,int32_t dy, int32_t bd);
void svt_av1_highbd_dr_prediction_z3_neon(uint16_t *dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint16_t *above, const uint16_t *left, int32_t upsample_left, int32_t dx, int32_t dy, int32_t bd);

uint64_t svt_av1_wedge_sse_from_residuals_neon(const int16_t *r1, const int16_t *d, const uint8_t *m, int N);
uint64_t svt_av1_wedge_sse_from_residuals_sve(const int16_t *r1, const int16_t *d, const uint8_t *m, int N);

void svt_av1_selfguided_restoration_neon(const uint8_t *dat8, int32_t width, int32_t height, int32_t stride, int32_t *flt0, int32_t *flt1, int32_t flt_stride, int32_t sgr_params_idx, int32_t bit_depth, int32_t highbd);
void svt_aom_apply_selfguided_restoration_neon(const uint8_t *dat, int32_t width, int32_t height, int32_t stride, int32_t eps, const int32_t *xqd, uint8_t *dst, int32_t dst_stride, int32_t *tmpbuf, int32_t bit_depth, int32_t highbd);

void svt_av1_dr_prediction_z1_neon(uint8_t *dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t *above, const uint8_t *left,int32_t upsample_above, int32_t dx, int32_t dy);
void svt_av1_dr_prediction_z2_neon(uint8_t *dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t *above, const uint8_t *left, int32_t upsample_above, int32_t upsample_left, int32_t dx,int32_t dy);
void svt_av1_dr_prediction_z3_neon(uint8_t *dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t *above, const uint8_t *left, int32_t upsample_left, int32_t dx, int32_t dy);

void svt_dav1d_inv_txfm_add_neon(const TranLow *dqcoeff, uint8_t *dst_r, int32_t stride_r, uint8_t *dst_w, int32_t stride_w, const TxfmParam *txfm_param);

void svt_residual_kernel8bit_neon(uint8_t *input, uint32_t input_stride, uint8_t *pred, uint32_t pred_stride, int16_t *residual, uint32_t residual_stride, uint32_t area_width, uint32_t area_height);
void svt_residual_kernel16bit_neon(uint16_t *input, uint32_t input_stride, uint16_t *pred, uint32_t pred_stride, int16_t *residual, uint32_t residual_stride, uint32_t area_width, uint32_t area_height);

void svt_cdef_filter_block_neon(uint8_t *dst8, uint16_t *dst16, int32_t dstride, const uint16_t *in, int32_t pri_strength, int32_t sec_strength, int32_t dir, int32_t pri_damping, int32_t sec_damping, int32_t bsize, int32_t coeff_shift, uint8_t subsampling_factor);
uint8_t svt_aom_cdef_find_dir_neon(const uint16_t *img, int32_t stride, int32_t *var, int32_t coeff_shift);
void svt_aom_cdef_find_dir_dual_neon(const uint16_t *img1, const uint16_t *img2, int stride, int32_t *var1, int32_t *var2, int32_t coeff_shift, uint8_t *out1, uint8_t *out2);

void svt_aom_cfl_predict_lbd_neon(const int16_t *pred_buf_q3, uint8_t *pred, int32_t pred_stride, uint8_t *dst, int32_t dst_stride, int32_t alpha_q3, int32_t bit_depth, int32_t width, int32_t height);
void svt_cfl_predict_hbd_neon(const int16_t *pred_buf_q3, uint16_t *pred, int32_t pred_stride, uint16_t *dst, int32_t dst_stride, int32_t alpha_q3, int32_t bit_depth, int32_t width, int32_t height);

void svt_cfl_luma_subsampling_420_lbd_neon(const uint8_t *input, int32_t input_stride, int16_t *output_q3, int32_t width, int32_t height);
void svt_cfl_luma_subsampling_420_hbd_neon(const uint16_t *input, int32_t input_stride, int16_t *output_q3, int32_t width, int32_t height);

void svt_aom_hadamard_4x4_neon(const int16_t *src_diff, ptrdiff_t src_stride, int32_t *coeff);
void svt_aom_hadamard_8x8_neon(const int16_t *src_diff, ptrdiff_t src_stride, int32_t *coeff);
void svt_aom_hadamard_16x16_neon(const int16_t *src_diff, ptrdiff_t src_stride, int32_t *coeff);
void svt_aom_hadamard_32x32_neon(const int16_t *src_diff, ptrdiff_t src_stride, int32_t *coeff);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_aom_highbd_hadamard_8x8_neon(const int16_t *src_diff, ptrdiff_t src_stride, int32_t *coeff);
#endif

void svt_aom_subtract_block_neon(int rows, int cols, int16_t *diff, ptrdiff_t diff_stride, const uint8_t *src, ptrdiff_t src_stride, const uint8_t *pred, ptrdiff_t pred_stride);
void svt_aom_highbd_subtract_block_neon(int rows, int cols, int16_t *diff_ptr, ptrdiff_t diff_stride, const uint8_t *src_ptr, ptrdiff_t src_stride, const uint8_t *pred_ptr, ptrdiff_t pred_stride, int bd);
void svt_av1_filter_intra_predictor_neon(uint8_t *dst, ptrdiff_t stride, TxSize tx_size, const uint8_t *above, const uint8_t *left, int mode);
void svt_av1_filter_intra_predictor_neon_i8mm(uint8_t *dst, ptrdiff_t stride, TxSize tx_size, const uint8_t *above, const uint8_t *left, int mode);

void svt_aom_highbd_smooth_v_predictor_4x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_4x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_4x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_8x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_8x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_8x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_8x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_16x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_16x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_16x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_16x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_16x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_32x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_32x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_32x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_32x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_64x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_64x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_v_predictor_64x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_h_predictor_4x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_4x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_4x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_8x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_8x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_8x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_8x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_16x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_16x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_16x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_16x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_16x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_32x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_32x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_32x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_32x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_64x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_64x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_h_predictor_64x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_predictor_4x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_4x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_4x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_8x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_8x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_8x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_8x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_16x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_16x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_16x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_16x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_16x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_32x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_32x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_32x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_32x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_64x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_64x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_smooth_predictor_64x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_v_predictor_4x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_4x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_4x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_8x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_8x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_8x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_8x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_16x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_16x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_16x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_16x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_16x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_32x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_32x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_32x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_32x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_64x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_64x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_v_predictor_64x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_h_predictor_4x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_4x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_4x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_8x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_8x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_8x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_8x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_16x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_16x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_16x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_16x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_16x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_32x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_32x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_32x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_32x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_64x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_64x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_h_predictor_64x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_paeth_predictor_4x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_paeth_predictor_4x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_paeth_predictor_4x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_paeth_predictor_8x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_paeth_predictor_8x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_paeth_predictor_8x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_paeth_predictor_8x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_paeth_predictor_16x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_paeth_predictor_16x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_paeth_predictor_16x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_paeth_predictor_16x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_paeth_predictor_16x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_paeth_predictor_32x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_paeth_predictor_32x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_paeth_predictor_32x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_paeth_predictor_32x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_paeth_predictor_64x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_paeth_predictor_64x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_paeth_predictor_64x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_predictor_4x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_4x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_4x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_8x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_8x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_8x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_8x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_16x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_16x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_16x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_16x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_16x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_32x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_32x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_32x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_32x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_64x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_64x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_predictor_64x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_left_predictor_4x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_4x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_4x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_8x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_8x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_8x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_8x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_16x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_16x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_16x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_16x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_16x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_32x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_32x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_32x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_32x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_64x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_64x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_left_predictor_64x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_top_predictor_4x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_4x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_4x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_8x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_8x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_8x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_8x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_16x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_16x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_16x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_16x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_16x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_32x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_32x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_32x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_32x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_64x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_64x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_top_predictor_64x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_128_predictor_4x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_4x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_4x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_8x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_8x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_8x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_8x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_16x4_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_16x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_16x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_16x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_16x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_32x8_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_32x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_32x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_32x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_64x16_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_64x32_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void svt_aom_highbd_dc_128_predictor_64x64_neon(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_lpf_vertical_4_neon(uint8_t *src, int stride, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);
void svt_aom_lpf_vertical_6_neon(uint8_t *src, int stride, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);
void svt_aom_lpf_vertical_8_neon(uint8_t *src, int stride, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);
void svt_aom_lpf_vertical_14_neon(uint8_t *src, int stride, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);
void svt_aom_lpf_horizontal_4_neon(uint8_t *src, int stride, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);
void svt_aom_lpf_horizontal_6_neon(uint8_t *src, int stride, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);
void svt_aom_lpf_horizontal_8_neon(uint8_t *src, int stride, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);
void svt_aom_lpf_horizontal_14_neon(uint8_t *src, int stride, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);

void svt_aom_highbd_lpf_horizontal_4_neon(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);
void svt_aom_highbd_lpf_horizontal_6_neon(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);
void svt_aom_highbd_lpf_horizontal_8_neon(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);
void svt_aom_highbd_lpf_horizontal_14_neon(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);
void svt_aom_highbd_lpf_vertical_4_neon(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);
void svt_aom_highbd_lpf_vertical_6_neon(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);
void svt_aom_highbd_lpf_vertical_8_neon(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);
void svt_aom_highbd_lpf_vertical_14_neon(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);

void svt_aom_dc_predictor_4x4_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_4x8_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_4x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_8x4_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_8x8_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_8x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_8x32_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_16x4_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_16x8_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_16x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_16x32_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_16x64_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_32x8_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_32x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_32x32_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_32x64_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_64x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_64x32_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_predictor_64x64_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_left_predictor_4x4_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_4x8_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_4x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_8x4_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_8x8_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_8x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_8x32_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_16x4_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_16x8_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_16x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_16x32_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_16x64_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_32x8_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_32x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_32x32_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_32x64_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_64x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_64x32_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_left_predictor_64x64_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_top_predictor_4x4_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_4x8_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_4x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_8x4_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_8x8_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_8x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_8x32_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_16x4_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_16x8_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_16x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_16x32_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_16x64_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_32x8_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_32x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_32x32_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_32x64_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_64x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_64x32_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_top_predictor_64x64_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_128_predictor_4x4_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_4x8_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_4x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_8x4_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_8x8_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_8x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_8x32_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_16x4_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_16x8_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_16x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_16x32_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_16x64_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_32x8_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_32x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_32x32_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_32x64_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_64x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_64x32_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);
void svt_aom_dc_128_predictor_64x64_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_predictor_4x4_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_predictor_4x8_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_predictor_4x16_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_predictor_8x4_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_predictor_8x8_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_predictor_8x16_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_predictor_8x32_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_predictor_16x4_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_predictor_16x8_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_predictor_16x16_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_predictor_16x32_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_predictor_16x64_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_predictor_32x8_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_predictor_32x16_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_predictor_32x32_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_predictor_32x64_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_predictor_64x16_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_predictor_64x32_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_predictor_64x64_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);

void svt_aom_smooth_h_predictor_4x4_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_h_predictor_4x8_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_h_predictor_4x16_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_h_predictor_8x4_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_h_predictor_8x8_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_h_predictor_8x16_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_h_predictor_8x32_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_h_predictor_16x4_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_h_predictor_16x8_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_h_predictor_16x16_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_h_predictor_16x32_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_h_predictor_16x64_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_h_predictor_32x8_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_h_predictor_32x16_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_h_predictor_32x32_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_h_predictor_32x64_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_h_predictor_64x16_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_h_predictor_64x32_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_h_predictor_64x64_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);

void svt_aom_smooth_v_predictor_4x4_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_v_predictor_4x8_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_v_predictor_4x16_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_v_predictor_8x4_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_v_predictor_8x8_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_v_predictor_8x16_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_v_predictor_8x32_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_v_predictor_16x4_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_v_predictor_16x8_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_v_predictor_16x16_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_v_predictor_16x32_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_v_predictor_16x64_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_v_predictor_32x8_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_v_predictor_32x16_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_v_predictor_32x32_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_v_predictor_32x64_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_v_predictor_64x16_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_v_predictor_64x32_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);
void svt_aom_smooth_v_predictor_64x64_neon(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above,const uint8_t *left);

void svt_aom_v_predictor_4x4_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_v_predictor_4x8_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_v_predictor_4x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_v_predictor_8x4_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_v_predictor_8x8_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_v_predictor_8x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_v_predictor_8x32_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_v_predictor_16x4_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_v_predictor_16x8_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_v_predictor_16x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_v_predictor_16x32_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_v_predictor_16x64_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_v_predictor_32x8_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_v_predictor_32x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_v_predictor_32x32_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_v_predictor_32x64_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_v_predictor_64x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_v_predictor_64x32_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_v_predictor_64x64_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);

void svt_aom_copy_rect8_8bit_to_16bit_neon(uint16_t *dst, int32_t dstride, const uint8_t *src, int32_t sstride,
                                            int32_t v, int32_t h);

void svt_aom_h_predictor_4x4_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_h_predictor_4x8_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_h_predictor_4x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_h_predictor_8x4_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_h_predictor_8x8_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_h_predictor_8x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_h_predictor_8x32_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_h_predictor_16x4_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_h_predictor_16x8_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_h_predictor_16x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_h_predictor_16x32_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_h_predictor_16x64_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_h_predictor_32x8_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_h_predictor_32x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_h_predictor_32x32_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_h_predictor_32x64_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_h_predictor_64x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_h_predictor_64x32_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_h_predictor_64x64_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);

void svt_aom_paeth_predictor_4x4_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_paeth_predictor_4x8_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_paeth_predictor_4x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_paeth_predictor_8x4_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_paeth_predictor_8x8_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_paeth_predictor_8x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_paeth_predictor_8x32_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_paeth_predictor_16x4_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_paeth_predictor_16x8_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_paeth_predictor_16x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_paeth_predictor_16x32_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_paeth_predictor_16x64_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_paeth_predictor_32x8_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_paeth_predictor_32x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_paeth_predictor_32x32_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_paeth_predictor_32x64_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_paeth_predictor_64x16_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_paeth_predictor_64x32_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);
void svt_aom_paeth_predictor_64x64_neon(uint8_t *dst, ptrdiff_t stride, const uint8_t *above,const uint8_t *left);

uint64_t svt_full_distortion_kernel16_bits_neon(uint8_t* input, uint32_t input_offset, uint32_t input_stride, uint8_t* recon, int32_t recon_offset, uint32_t recon_stride, uint32_t area_width, uint32_t area_height);
uint64_t svt_full_distortion_kernel16_bits_sve(uint8_t* input, uint32_t input_offset, uint32_t input_stride, uint8_t* recon, int32_t recon_offset, uint32_t recon_stride, uint32_t area_width, uint32_t area_height);
void svt_full_distortion_kernel32_bits_neon(int32_t *coeff, uint32_t coeff_stride, int32_t *recon_coeff, uint32_t recon_coeff_stride, uint64_t distortion_result[DIST_CALC_TOTAL], uint32_t area_width, uint32_t area_height);

void svt_full_distortion_kernel_cbf_zero32_bits_neon(int32_t *coeff, uint32_t coeff_stride,
                                                        uint64_t distortion_result[DIST_CALC_TOTAL], uint32_t area_width,
                                                        uint32_t area_height);

void svt_av1_inv_txfm2d_add_16x16_neon(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);
void svt_av1_inv_txfm2d_add_32x32_neon(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);
void svt_av1_inv_txfm2d_add_64x64_neon(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);

void svt_av1_highbd_wiener_convolve_add_src_neon(const uint8_t *const src, const ptrdiff_t src_stride, uint8_t *const dst, const ptrdiff_t dst_stride, const int16_t *const filter_x, const int16_t *const filter_y, const int32_t w, const int32_t h, const ConvolveParams *const conv_params, const int32_t bd);
void svt_av1_highbd_jnt_convolve_2d_neon(const uint16_t *src, int32_t src_stride, uint16_t *dst16,
                                        int32_t dst16_stride, int32_t w, int32_t h,
                                        const InterpFilterParams *filter_params_x,
                                        const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4,
                                        const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
void svt_av1_highbd_jnt_convolve_2d_sve2(const uint16_t *src, int32_t src_stride, uint16_t *dst16,
                                        int32_t dst16_stride, int32_t w, int32_t h,
                                        const InterpFilterParams *filter_params_x,
                                        const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4,
                                        const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
void svt_av1_highbd_jnt_convolve_y_neon(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
void svt_av1_highbd_jnt_convolve_y_sve2(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
void svt_av1_highbd_jnt_convolve_2d_copy_neon( const uint16_t *src, int32_t src_stride, uint16_t *dst0, int32_t dst_stride0, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
void svt_av1_highbd_convolve_x_sr_neon(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
void svt_av1_highbd_convolve_x_sr_sve(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
void svt_av1_highbd_convolve_y_sr_neon(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
void svt_av1_highbd_convolve_y_sr_sve2(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
void svt_av1_highbd_jnt_convolve_x_neon(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
void svt_av1_highbd_jnt_convolve_x_sve(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
void svt_enc_msb_un_pack2d_neon(uint16_t *in16_bit_buffer, uint32_t in_stride,
    uint8_t *out8_bit_buffer, uint8_t *outn_bit_buffer,
    uint32_t out8_stride, uint32_t outn_stride, uint32_t width,
    uint32_t height);

void svt_av1_highbd_convolve_2d_sr_neon(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);

void svt_av1_highbd_convolve_2d_sr_sve2(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);

void svt_av1_highbd_convolve_2d_copy_sr_neon(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);

void svt_compressed_packmsb_neon(uint8_t *in8_bit_buffer, uint32_t in8_stride, uint8_t *inn_bit_buffer,
                                    uint32_t inn_stride, uint16_t *out16_bit_buffer, uint32_t out_stride,
                                    uint32_t width, uint32_t height);

void svt_enc_msb_pack2d_neon(uint8_t *in8_bit_buffer, uint32_t in8_stride, uint8_t *inn_bit_buffer,
                                uint16_t *out16_bit_buffer, uint32_t inn_stride, uint32_t out_stride, uint32_t width,
                                uint32_t height);
void svt_av1_inv_txfm2d_add_4x4_neon(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);
void svt_av1_inv_txfm2d_add_8x8_neon(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);
void svt_av1_inv_txfm2d_add_4x8_neon(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t bd);
void svt_av1_inv_txfm2d_add_4x16_neon(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t bd);
void svt_av1_inv_txfm2d_add_8x4_neon(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t bd);
void svt_av1_inv_txfm2d_add_8x16_neon(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
void svt_av1_inv_txfm2d_add_8x32_neon(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
void svt_av1_inv_txfm2d_add_16x4_neon(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t bd);
void svt_av1_inv_txfm2d_add_16x8_neon(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
void svt_av1_inv_txfm2d_add_16x32_neon(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
void svt_av1_inv_txfm2d_add_16x64_neon(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
void svt_av1_inv_txfm2d_add_32x8_neon(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
void svt_av1_inv_txfm2d_add_32x16_neon(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
void svt_av1_inv_txfm2d_add_32x64_neon(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
void svt_av1_inv_txfm2d_add_64x16_neon(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
void svt_av1_inv_txfm2d_add_64x32_neon(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);

#endif

#ifdef ARCH_X86_64

void svt_aom_blend_a64_vmask_sse4_1(uint8_t *dst, uint32_t dst_stride, const uint8_t *src0, uint32_t src0_stride, const uint8_t *src1, uint32_t src1_stride, const uint8_t *mask, int w, int h);

void svt_aom_blend_a64_hmask_sse4_1(uint8_t *dst, uint32_t dst_stride, const uint8_t *src0, uint32_t src0_stride, const uint8_t *src1, uint32_t src1_stride, const uint8_t *mask, int w, int h);

void svt_aom_blend_a64_mask_sse4_1(uint8_t *dst, uint32_t dst_stride, const uint8_t *src0, uint32_t src0_stride, const uint8_t *src1, uint32_t src1_stride, const uint8_t *mask, uint32_t mask_stride, int w, int h, int subx, int suby);
void svt_aom_blend_a64_mask_avx2(uint8_t *dst, uint32_t dst_stride, const uint8_t *src0, uint32_t src0_stride, const uint8_t *src1, uint32_t src1_stride, const uint8_t *mask, uint32_t mask_stride, int w, int h, int subx, int suby);

void svt_aom_highbd_blend_a64_mask_8bit_sse4_1(uint8_t *dst, uint32_t dst_stride, const uint8_t *src0, uint32_t src0_stride, const uint8_t *src1, uint32_t src1_stride, const uint8_t *mask, uint32_t mask_stride, int w, int h, int subx, int suby, int bd);

void svt_aom_highbd_blend_a64_vmask_16bit_sse4_1(uint16_t *dst, uint32_t dst_stride, const uint16_t *src0, uint32_t src0_stride, const uint16_t *src1, uint32_t src1_stride, const uint8_t *mask, int w, int h, int bd);

void svt_aom_highbd_blend_a64_hmask_16bit_sse4_1(uint16_t *dst, uint32_t dst_stride, const uint16_t *src0, uint32_t src0_stride, const uint16_t *src1, uint32_t src1_stride, const uint8_t *mask, int w, int h, int bd);

void svt_av1_blend_a64_vmask_avx2(uint8_t *dst, uint32_t dst_stride, const uint8_t *src0, uint32_t src0_stride, const uint8_t *src1, uint32_t src1_stride, const uint8_t *mask, int w, int h);
void svt_av1_blend_a64_hmask_avx2(uint8_t *dst, uint32_t dst_stride, const uint8_t *src0, uint32_t src0_stride, const uint8_t *src1, uint32_t src1_stride, const uint8_t *mask, int w, int h);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_av1_highbd_blend_a64_hmask_16bit_avx2(uint16_t *dst, uint32_t dst_stride, const uint16_t *src0, uint32_t src0_stride, const uint16_t *src1, uint32_t src1_stride, const uint8_t *mask, int w, int h, int bd);
void svt_av1_highbd_blend_a64_vmask_16bit_avx2(uint16_t *dst, uint32_t dst_stride, const uint16_t *src0, uint32_t src0_stride, const uint16_t *src1, uint32_t src1_stride, const uint8_t *mask, int w, int h, int bd);
#endif

void svt_cfl_predict_lbd_avx2(const int16_t *pred_buf_q3, uint8_t *pred, int32_t pred_stride, uint8_t *dst, int32_t dst_stride, int32_t alpha_q3, int32_t bit_depth, int32_t width, int32_t height);

void svt_cfl_predict_hbd_avx2(const int16_t *pred_buf_q3, uint16_t *pred, int32_t pred_stride, uint16_t *dst, int32_t dst_stride, int32_t alpha_q3, int32_t bit_depth, int32_t width, int32_t height);

void svt_av1_filter_intra_predictor_sse4_1(uint8_t *dst, ptrdiff_t stride, TxSize tx_size, const uint8_t *above, const uint8_t *left, int mode);


void svt_av1_filter_intra_edge_sse4_1(uint8_t *p, int32_t sz, int32_t strength);

void svt_av1_filter_intra_edge_high_sse4_1(uint16_t *p, int32_t sz, int32_t strength);

void svt_av1_upsample_intra_edge_sse4_1(uint8_t *p, int32_t sz);

// AMIR
//void svt_av1_upsample_intra_edge_high_sse4_1(uint16_t *p, int32_t sz, int32_t bd);


void svt_av1_build_compound_diffwtd_mask_d16_sse4_1(uint8_t *mask, DIFFWTD_MASK_TYPE mask_type, const CONV_BUF_TYPE *src0, int src0_stride, const CONV_BUF_TYPE *src1, int src1_stride, int h, int w, ConvolveParams *conv_params, int bd);
void svt_av1_build_compound_diffwtd_mask_d16_avx2(uint8_t *mask, DIFFWTD_MASK_TYPE mask_type, const CONV_BUF_TYPE *src0, int src0_stride, const CONV_BUF_TYPE *src1, int src1_stride, int h, int w, ConvolveParams *conv_params, int bd);

/////////------------------------

void svt_av1_inv_txfm2d_add_4x4_sse4_1(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);
void svt_av1_inv_txfm2d_add_4x4_avx2(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);

void svt_av1_inv_txfm2d_add_8x8_sse4_1(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);
void svt_av1_inv_txfm2d_add_8x8_avx2(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);

void svt_av1_inv_txfm2d_add_16x16_sse4_1(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);
void svt_av1_inv_txfm2d_add_16x16_avx2(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);
void svt_av1_inv_txfm2d_add_16x16_avx512(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);

void svt_av1_inv_txfm2d_add_32x32_avx2(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);
void svt_av1_inv_txfm2d_add_32x32_avx512(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);

void svt_av1_inv_txfm2d_add_64x64_sse4_1(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);
void svt_av1_inv_txfm2d_add_64x64_avx2(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);
void svt_av1_inv_txfm2d_add_64x64_avx512(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);

void svt_av1_inv_txfm2d_add_32x32_sse4_1(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);
void svt_av1_highbd_inv_txfm_add_sse4_1(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
void svt_av1_highbd_inv_txfm_add_avx2(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);

void svt_dav1d_highbd_inv_txfm_add_avx2(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);
void svt_dav1d_inv_txfm2d_add_4x4_avx2(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);
void svt_dav1d_inv_txfm2d_add_4x8_avx2(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t bd);
void svt_dav1d_inv_txfm2d_add_4x16_avx2(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t bd);
void svt_dav1d_inv_txfm2d_add_8x4_avx2(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t bd);
void svt_dav1d_inv_txfm2d_add_16x4_avx2(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t bd);
void svt_dav1d_inv_txfm2d_add_8x8_avx2(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);
void svt_dav1d_inv_txfm2d_add_16x16_avx2(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);
void svt_dav1d_inv_txfm2d_add_32x32_avx2(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);
void svt_dav1d_inv_txfm2d_add_64x64_avx2(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, int32_t bd);


void svt_av1_inv_txfm2d_add_16x32_avx512(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);

void svt_av1_inv_txfm2d_add_32x16_avx512(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);



void svt_av1_inv_txfm2d_add_32x64_avx512(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);

void svt_av1_inv_txfm2d_add_64x32_avx512(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);

void svt_av1_inv_txfm2d_add_16x64_avx512(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);

void svt_av1_inv_txfm2d_add_64x16_avx512(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd);

void svt_av1_inv_txfm2d_add_4x8_sse4_1(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t bd);

void svt_av1_inv_txfm2d_add_8x4_sse4_1(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t bd);

void svt_av1_inv_txfm2d_add_4x16_sse4_1(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t bd);

void svt_av1_inv_txfm2d_add_16x4_sse4_1(const int32_t *input, uint16_t *output_r, int32_t stride_r, uint16_t *output_w, int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t bd);

void svt_av1_inv_txfm_add_ssse3(const TranLow *dqcoeff, uint8_t *dst_r, int32_t stride_r, uint8_t *dst_w, int32_t stride_w, const TxfmParam *txfm_param);
void svt_av1_inv_txfm_add_avx2(const TranLow *dqcoeff, uint8_t *dst_r, int32_t stride_r, uint8_t *dst_w, int32_t stride_w, const TxfmParam *txfm_param);
void svt_dav1d_inv_txfm_add_avx2(const TranLow *dqcoeff, uint8_t *dst_r, int32_t stride_r, uint8_t *dst_w, int32_t stride_w, const TxfmParam *txfm_param);

void svt_compressed_packmsb_sse4_1_intrin(uint8_t *in8_bit_buffer, uint32_t in8_stride,
    uint8_t *inn_bit_buffer, uint32_t inn_stride,
    uint16_t *out16_bit_buffer, uint32_t out_stride, uint32_t width,
    uint32_t height);
void svt_compressed_packmsb_avx2_intrin(uint8_t *in8_bit_buffer, uint32_t in8_stride,
    uint8_t *inn_bit_buffer, uint32_t inn_stride,
    uint16_t *out16_bit_buffer, uint32_t out_stride, uint32_t width,
    uint32_t height);
void svt_enc_msb_un_pack2d_sse2_intrin(uint16_t *in16_bit_buffer, uint32_t in_stride,
    uint8_t *out8_bit_buffer, uint8_t *outn_bit_buffer,
    uint32_t out8_stride, uint32_t outn_stride, uint32_t width,
    uint32_t height);
void svt_enc_msb_un_pack2d_avx2_intrin(uint16_t *in16_bit_buffer, uint32_t in_stride,
    uint8_t *out8_bit_buffer, uint8_t *outn_bit_buffer,
    uint32_t out8_stride, uint32_t outn_stride, uint32_t width,
    uint32_t height);
void svt_enc_msb_pack2d_sse2_intrin(uint8_t *in8_bit_buffer, uint32_t in8_stride,
    uint8_t *inn_bit_buffer, uint16_t *out16_bit_buffer,
    uint32_t inn_stride, uint32_t out_stride, uint32_t width,
    uint32_t height);
void svt_enc_msb_pack2d_avx2_intrin_al(uint8_t *in8_bit_buffer, uint32_t in8_stride,
    uint8_t *inn_bit_buffer, uint16_t *out16_bit_buffer,
    uint32_t inn_stride, uint32_t out_stride,
    uint32_t width, uint32_t height);
void svt_residual_kernel8bit_sse4_1(uint8_t *input, uint32_t input_stride, uint8_t *pred, uint32_t pred_stride, int16_t *residual, uint32_t residual_stride, uint32_t area_width, uint32_t area_height);
void svt_residual_kernel8bit_avx2(uint8_t *input, uint32_t input_stride, uint8_t *pred, uint32_t pred_stride, int16_t *residual, uint32_t residual_stride, uint32_t area_width, uint32_t area_height);
void svt_residual_kernel8bit_avx512(uint8_t *input, uint32_t input_stride, uint8_t *pred, uint32_t pred_stride, int16_t *residual, uint32_t residual_stride, uint32_t area_width, uint32_t area_height);

void svt_full_distortion_kernel_cbf_zero32_bits_avx2(int32_t *coeff, uint32_t coeff_stride,
    uint64_t distortion_result[DIST_CALC_TOTAL],
    uint32_t area_width, uint32_t area_height);
void svt_full_distortion_kernel_cbf_zero32_bits_sse4_1(int32_t *coeff, uint32_t coeff_stride,
    uint64_t distortion_result[DIST_CALC_TOTAL],
    uint32_t area_width, uint32_t area_height);
void svt_full_distortion_kernel32_bits_sse4_1(int32_t *coeff, uint32_t coeff_stride, int32_t *recon_coeff,
    uint32_t recon_coeff_stride,
    uint64_t distortion_result[DIST_CALC_TOTAL],
    uint32_t area_width, uint32_t area_height);
void svt_full_distortion_kernel32_bits_avx2(int32_t *coeff, uint32_t coeff_stride, int32_t *recon_coeff,
    uint32_t recon_coeff_stride,
    uint64_t distortion_result[DIST_CALC_TOTAL],
    uint32_t area_width, uint32_t area_height);
uint64_t svt_spatial_full_distortion_kernel_sse4_1(uint8_t *input, uint32_t input_offset, uint32_t input_stride, uint8_t *recon, int32_t recon_offset, uint32_t recon_stride, uint32_t area_width, uint32_t area_height);
uint64_t svt_spatial_full_distortion_kernel_avx2(uint8_t *input, uint32_t input_offset, uint32_t input_stride, uint8_t *recon, int32_t recon_offset, uint32_t recon_stride, uint32_t area_width, uint32_t area_height);
uint64_t svt_spatial_full_distortion_kernel_avx512(uint8_t *input, uint32_t input_offset, uint32_t input_stride, uint8_t *recon, int32_t recon_offset, uint32_t recon_stride, uint32_t area_width, uint32_t area_height);

uint64_t svt_full_distortion_kernel16_bits_sse4_1(uint8_t* input, uint32_t input_offset, uint32_t input_stride, uint8_t* recon, int32_t recon_offset, uint32_t recon_stride, uint32_t area_width, uint32_t area_height);
uint64_t svt_full_distortion_kernel16_bits_avx2(uint8_t* input, uint32_t input_offset, uint32_t input_stride, uint8_t* recon, int32_t recon_offset, uint32_t recon_stride, uint32_t area_width, uint32_t area_height);
extern void svt_residual_kernel16bit_sse2_intrin(uint16_t *input, uint32_t input_stride, uint16_t *pred,
    uint32_t pred_stride, int16_t *residual,
    uint32_t residual_stride, uint32_t area_width,
    uint32_t area_height);
extern void svt_residual_kernel16bit_avx2(uint16_t *input, uint32_t input_stride, uint16_t *pred,
    uint32_t pred_stride, int16_t *residual,
    uint32_t residual_stride, uint32_t area_width,
    uint32_t area_height);
void avc_style_luma_interpolation_filter_helper_ssse3(EbByte ref_pic, uint32_t src_stride,
    EbByte dst, uint32_t dst_stride,
    uint32_t pu_width, uint32_t pu_height,
    EbByte temp_buf,
    uint32_t frac_pos,
    uint8_t  fractional_position);
void svt_av1_wiener_convolve_add_src_sse2(const uint8_t *const src, const ptrdiff_t src_stride, uint8_t *const dst, const ptrdiff_t dst_stride, const int16_t *const filter_x, const int16_t *const filter_y, const int32_t w, const int32_t h, const ConvolveParams *const conv_params);
void svt_av1_wiener_convolve_add_src_avx2(const uint8_t *const src, const ptrdiff_t src_stride, uint8_t *const dst, const ptrdiff_t dst_stride, const int16_t *const filter_x, const int16_t *const filter_y, const int32_t w, const int32_t h, const ConvolveParams *const conv_params);
void svt_av1_wiener_convolve_add_src_avx512(const uint8_t *const src, const ptrdiff_t src_stride, uint8_t *const dst, const ptrdiff_t dst_stride, const int16_t *const filter_x, const int16_t *const filter_y, const int32_t w, const int32_t h, const ConvolveParams *const conv_params);

void svt_av1_highbd_wiener_convolve_add_src_ssse3(const uint8_t *const src, const ptrdiff_t src_stride, uint8_t *const dst, const ptrdiff_t dst_stride, const int16_t *const filter_x, const int16_t *const filter_y, const int32_t w, const int32_t h, const ConvolveParams *const conv_params, const int32_t bd);
void svt_av1_highbd_wiener_convolve_add_src_avx2(const uint8_t *const src, const ptrdiff_t src_stride, uint8_t *const dst, const ptrdiff_t dst_stride, const int16_t *const filter_x, const int16_t *const filter_y, const int32_t w, const int32_t h, const ConvolveParams *const conv_params, const int32_t bd);

void svt_apply_selfguided_restoration_sse4_1(const uint8_t *dat, int32_t width, int32_t height, int32_t stride, int32_t eps, const int32_t *xqd, uint8_t *dst, int32_t dst_stride, int32_t *tmpbuf, int32_t bit_depth, int32_t highbd);
void svt_apply_selfguided_restoration_avx2(const uint8_t *dat, int32_t width, int32_t height, int32_t stride, int32_t eps, const int32_t *xqd, uint8_t *dst, int32_t dst_stride, int32_t *tmpbuf, int32_t bit_depth, int32_t highbd);

void svt_av1_selfguided_restoration_sse4_1(const uint8_t *dgd8, int32_t width, int32_t height, int32_t dgd_stride, int32_t *flt0, int32_t *flt1, int32_t flt_stride, int32_t sgr_params_idx, int32_t bit_depth, int32_t highbd);
void svt_av1_selfguided_restoration_avx2(const uint8_t *dgd8, int32_t width, int32_t height, int32_t dgd_stride, int32_t *flt0, int32_t *flt1, int32_t flt_stride, int32_t sgr_params_idx, int32_t bit_depth, int32_t highbd);

void svt_av1_convolve_2d_copy_sr_sse2(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
void svt_av1_convolve_2d_copy_sr_avx2(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
void svt_av1_convolve_2d_copy_sr_avx512(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);

void svt_av1_convolve_2d_sr_sse2(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
void svt_av1_convolve_2d_sr_avx2(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
void svt_av1_convolve_2d_sr_avx512(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);

void svt_av1_jnt_convolve_2d_copy_sse2(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
void svt_av1_jnt_convolve_2d_copy_avx2(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
void svt_av1_jnt_convolve_2d_copy_avx512(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);

void svt_av1_convolve_x_sr_sse2(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
void svt_av1_convolve_x_sr_avx2(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
void svt_av1_convolve_x_sr_avx512(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);

void svt_av1_convolve_y_sr_sse2(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
void svt_av1_convolve_y_sr_avx2(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
void svt_av1_convolve_y_sr_avx512(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);

void svt_av1_convolve_2d_scale_sse4_1(const uint8_t *src, int src_stride, uint8_t *dst, int dst_stride, int w, int h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int subpel_x_qn, const int x_step_qn, const int subpel_y_q4, const int y_step_qn, ConvolveParams *conv_params);

void svt_av1_jnt_convolve_x_sse2(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
void svt_av1_jnt_convolve_x_avx2(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
void svt_av1_jnt_convolve_x_avx512(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);

void svt_av1_jnt_convolve_y_sse2(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
void svt_av1_jnt_convolve_y_avx2(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
void svt_av1_jnt_convolve_y_avx512(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);

void svt_av1_jnt_convolve_2d_sse2(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
void svt_av1_jnt_convolve_2d_ssse3(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
void svt_av1_jnt_convolve_2d_avx2(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);
void svt_av1_jnt_convolve_2d_avx512(const uint8_t *src, int32_t src_stride, uint8_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params);

void svt_av1_highbd_jnt_convolve_2d_copy_sse4_1( const uint16_t *src, int32_t src_stride, uint16_t *dst0, int32_t dst_stride0, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
void svt_av1_highbd_jnt_convolve_2d_sse4_1( const uint16_t *src, int32_t src_stride, uint16_t *dst16, int32_t dst16_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
void svt_av1_highbd_jnt_convolve_x_sse4_1(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
void svt_av1_highbd_jnt_convolve_y_sse4_1(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
void svt_av1_highbd_convolve_y_sr_ssse3(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
void svt_av1_highbd_convolve_x_sr_ssse3(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
void svt_av1_highbd_convolve_2d_sr_ssse3(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);
void svt_av1_highbd_convolve_2d_copy_sr_ssse3(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);

void svt_av1_highbd_convolve_2d_copy_sr_avx2(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);

void svt_av1_highbd_jnt_convolve_2d_copy_avx2(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);

void svt_av1_highbd_convolve_y_sr_avx2(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);

void svt_av1_highbd_convolve_2d_sr_avx2(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);

void svt_av1_highbd_convolve_2d_scale_sse4_1(const uint16_t *src, int src_stride, uint16_t *dst, int dst_stride, int w, int h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int subpel_x_q4, const int x_step_qn, const int subpel_y_q4, const int y_step_qn, ConvolveParams *conv_params, int bd);

void svt_av1_highbd_jnt_convolve_2d_avx2(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);

void svt_av1_highbd_jnt_convolve_x_avx2(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);

void svt_av1_highbd_jnt_convolve_y_avx2(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);

void svt_av1_highbd_convolve_x_sr_avx2(const uint16_t *src, int32_t src_stride, uint16_t *dst, int32_t dst_stride, int32_t w, int32_t h, const InterpFilterParams *filter_params_x, const InterpFilterParams *filter_params_y, const int32_t subpel_x_q4, const int32_t subpel_y_q4, ConvolveParams *conv_params, int32_t bd);

void svt_aom_convolve8_horiz_avx2(const uint8_t *src, ptrdiff_t src_stride, uint8_t *dst, ptrdiff_t dst_stride, const int16_t *filter_x, int x_step_q4, const int16_t *filter_y, int y_step_q4, int w, int h);

void svt_aom_convolve8_vert_avx2(const uint8_t *src, ptrdiff_t src_stride, uint8_t *dst, ptrdiff_t dst_stride, const int16_t *filter_x, int x_step_q4, const int16_t *filter_y, int y_step_q4, int w, int h);

void svt_aom_convolve8_horiz_ssse3(const uint8_t *src, ptrdiff_t src_stride, uint8_t *dst, ptrdiff_t dst_stride, const int16_t *filter_x, int x_step_q4, const int16_t *filter_y, int y_step_q4, int w, int h);
void svt_aom_convolve8_vert_ssse3(const uint8_t *src, ptrdiff_t src_stride, uint8_t *dst, ptrdiff_t dst_stride, const int16_t *filter_x, int x_step_q4, const int16_t *filter_y, int y_step_q4, int w, int h);

void svt_av1_build_compound_diffwtd_mask_sse4_1(uint8_t *mask, DIFFWTD_MASK_TYPE mask_type, const uint8_t *src0, int src0_stride, const uint8_t *src1, int src1_stride, int h, int w);
void svt_av1_build_compound_diffwtd_mask_avx2(uint8_t *mask, DIFFWTD_MASK_TYPE mask_type, const uint8_t *src0, int src0_stride, const uint8_t *src1, int src1_stride, int h, int w);
void svt_av1_build_compound_diffwtd_mask_highbd_ssse3(uint8_t *mask, DIFFWTD_MASK_TYPE mask_type, const uint8_t *src0, int src0_stride, const uint8_t *src1, int src1_stride, int h, int w, int bd);
void svt_av1_build_compound_diffwtd_mask_highbd_avx2(uint8_t *mask, DIFFWTD_MASK_TYPE mask_type, const uint8_t *src0, int src0_stride, const uint8_t *src1, int src1_stride, int h, int w, int bd);
uint64_t svt_av1_wedge_sse_from_residuals_sse2(const int16_t *r1, const int16_t *d, const uint8_t *m, int N);
uint64_t svt_av1_wedge_sse_from_residuals_avx2(const int16_t *r1, const int16_t *d, const uint8_t *m, int N);

void svt_aom_subtract_block_sse2(int rows, int cols, int16_t *diff_ptr, ptrdiff_t diff_stride, const uint8_t *src_ptr, ptrdiff_t src_stride, const uint8_t *pred_ptr, ptrdiff_t pred_stride);
void svt_aom_subtract_block_avx2(int rows, int cols, int16_t *diff_ptr, ptrdiff_t diff_stride, const uint8_t *src_ptr, ptrdiff_t src_stride, const uint8_t *pred_ptr, ptrdiff_t pred_stride);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_aom_highbd_subtract_block_sse2(int rows, int cols, int16_t *diff_ptr, ptrdiff_t diff_stride, const uint8_t *src_ptr, ptrdiff_t src_stride, const uint8_t *pred_ptr, ptrdiff_t pred_stride, int bd);
#endif

void svt_aom_lowbd_blend_a64_d16_mask_sse4_1(uint8_t *dst, uint32_t dst_stride, const CONV_BUF_TYPE *src0, uint32_t src0_stride, const CONV_BUF_TYPE *src1, uint32_t src1_stride, const uint8_t *mask, uint32_t mask_stride, int w, int h, int subw, int subh, ConvolveParams *conv_params);
void svt_aom_lowbd_blend_a64_d16_mask_avx2(uint8_t *dst, uint32_t dst_stride, const CONV_BUF_TYPE *src0, uint32_t src0_stride, const CONV_BUF_TYPE *src1, uint32_t src1_stride, const uint8_t *mask, uint32_t mask_stride, int w, int h, int subw, int subh, ConvolveParams *conv_params);

void svt_aom_highbd_blend_a64_d16_mask_sse4_1(uint8_t *dst, uint32_t dst_stride, const CONV_BUF_TYPE *src0, uint32_t src0_stride, const CONV_BUF_TYPE *src1, uint32_t src1_stride, const uint8_t *mask, uint32_t mask_stride, int w, int h, int subx, int suby, ConvolveParams *conv_params, const int bd);
void svt_aom_highbd_blend_a64_d16_mask_avx2(uint8_t *dst, uint32_t dst_stride, const CONV_BUF_TYPE *src0, uint32_t src0_stride, const CONV_BUF_TYPE *src1, uint32_t src1_stride, const uint8_t *mask, uint32_t mask_stride, int w, int h, int subx, int suby, ConvolveParams *conv_params, const int bd);

void svt_aom_highbd_dc_128_predictor_16x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_128_predictor_16x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_128_predictor_16x4_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_128_predictor_16x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_128_predictor_16x8_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_128_predictor_32x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_128_predictor_32x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_128_predictor_32x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_128_predictor_32x8_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_128_predictor_4x16_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_128_predictor_4x4_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_128_predictor_4x8_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_128_predictor_64x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_128_predictor_64x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_128_predictor_64x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_128_predictor_8x16_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_128_predictor_8x32_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_128_predictor_8x4_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_128_predictor_8x8_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_left_predictor_16x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_left_predictor_16x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_left_predictor_16x4_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_left_predictor_16x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_left_predictor_16x8_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);


void svt_aom_highbd_dc_left_predictor_32x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_dc_left_predictor_32x16_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_left_predictor_32x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_dc_left_predictor_32x32_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_left_predictor_32x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_dc_left_predictor_32x64_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_left_predictor_32x8_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_dc_left_predictor_32x8_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_left_predictor_4x16_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_left_predictor_4x4_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_left_predictor_4x8_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_left_predictor_64x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_dc_left_predictor_64x16_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_left_predictor_64x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_dc_left_predictor_64x32_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_left_predictor_64x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_dc_left_predictor_64x64_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_left_predictor_8x16_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_left_predictor_8x32_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_left_predictor_8x4_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_left_predictor_8x8_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_predictor_16x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_predictor_16x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_predictor_16x4_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_predictor_16x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_predictor_16x8_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);


void svt_aom_highbd_dc_predictor_32x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_dc_predictor_32x16_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_predictor_32x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_dc_predictor_32x32_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_predictor_32x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_dc_predictor_32x64_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_predictor_32x8_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_dc_predictor_32x8_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_predictor_4x16_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_predictor_4x4_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_predictor_4x8_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_predictor_64x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_dc_predictor_64x16_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_predictor_64x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_dc_predictor_64x32_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_predictor_64x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_dc_predictor_64x64_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_predictor_8x16_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_predictor_8x32_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_predictor_8x4_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_predictor_8x8_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_top_predictor_16x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_top_predictor_16x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_top_predictor_16x4_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_top_predictor_16x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_top_predictor_16x8_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_top_predictor_32x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_dc_top_predictor_32x16_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_top_predictor_32x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_dc_top_predictor_32x32_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_top_predictor_32x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_dc_top_predictor_32x64_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_top_predictor_32x8_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_dc_top_predictor_32x8_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_top_predictor_4x16_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_top_predictor_4x4_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_top_predictor_4x8_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_top_predictor_64x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_dc_top_predictor_64x16_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_top_predictor_64x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_dc_top_predictor_64x32_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_top_predictor_64x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_dc_top_predictor_64x64_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_top_predictor_8x16_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);


void svt_aom_highbd_dc_top_predictor_8x4_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_dc_top_predictor_8x8_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_h_predictor_16x16_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_h_predictor_16x32_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_h_predictor_16x4_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_h_predictor_16x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_h_predictor_16x8_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);


void svt_aom_highbd_h_predictor_32x16_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_h_predictor_32x16_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_h_predictor_32x32_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_h_predictor_32x32_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_h_predictor_32x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_h_predictor_32x64_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_h_predictor_32x8_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_h_predictor_32x8_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_h_predictor_4x16_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_h_predictor_4x4_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_h_predictor_4x8_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_h_predictor_64x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_h_predictor_64x16_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_h_predictor_64x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_h_predictor_64x32_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_h_predictor_64x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_h_predictor_64x64_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_h_predictor_8x16_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_h_predictor_8x32_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_h_predictor_8x4_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_h_predictor_8x8_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_h_predictor_16x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_h_predictor_16x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_h_predictor_16x4_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_h_predictor_16x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_h_predictor_16x8_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_h_predictor_32x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_smooth_h_predictor_32x16_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_h_predictor_32x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_smooth_h_predictor_32x32_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_h_predictor_32x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_smooth_h_predictor_32x64_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_h_predictor_32x8_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_smooth_h_predictor_32x8_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_h_predictor_4x16_ssse3(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_h_predictor_4x4_ssse3(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_h_predictor_4x8_ssse3(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_h_predictor_64x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_smooth_h_predictor_64x16_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_h_predictor_64x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_smooth_h_predictor_64x32_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_h_predictor_64x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_smooth_h_predictor_64x64_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_h_predictor_8x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_h_predictor_8x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_h_predictor_8x4_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_h_predictor_8x8_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_predictor_16x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_predictor_16x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_predictor_16x4_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_predictor_16x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_predictor_16x8_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);


void svt_aom_highbd_smooth_predictor_32x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_smooth_predictor_32x16_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_predictor_32x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_smooth_predictor_32x32_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_predictor_32x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_smooth_predictor_32x64_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_predictor_32x8_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_smooth_predictor_32x8_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_predictor_4x16_ssse3(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_predictor_4x4_ssse3(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_predictor_4x8_ssse3(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_predictor_64x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_smooth_predictor_64x16_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_predictor_64x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_smooth_predictor_64x32_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_predictor_64x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_smooth_predictor_64x64_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_predictor_8x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_predictor_8x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_predictor_8x4_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_predictor_8x8_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_v_predictor_16x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_v_predictor_16x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_v_predictor_16x4_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_v_predictor_16x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_v_predictor_16x8_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);


void svt_aom_highbd_smooth_v_predictor_32x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_smooth_v_predictor_32x16_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_v_predictor_32x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_smooth_v_predictor_32x32_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_v_predictor_32x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_smooth_v_predictor_32x64_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_v_predictor_32x8_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_smooth_v_predictor_32x8_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_v_predictor_4x16_ssse3(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_v_predictor_4x4_ssse3(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_v_predictor_4x8_ssse3(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_v_predictor_64x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_smooth_v_predictor_64x16_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_v_predictor_64x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_smooth_v_predictor_64x32_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_v_predictor_64x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_smooth_v_predictor_64x64_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_v_predictor_8x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_v_predictor_8x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_v_predictor_8x4_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_smooth_v_predictor_8x8_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_v_predictor_16x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_v_predictor_16x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_v_predictor_16x4_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_v_predictor_16x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_v_predictor_16x8_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);


void svt_aom_highbd_v_predictor_32x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_v_predictor_32x16_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_v_predictor_32x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_v_predictor_32x32_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_v_predictor_32x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_v_predictor_32x64_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_v_predictor_32x8_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_v_predictor_32x8_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_v_predictor_4x16_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_v_predictor_4x4_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_v_predictor_4x8_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_v_predictor_64x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_v_predictor_64x16_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_v_predictor_64x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_v_predictor_64x32_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_v_predictor_64x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);
void aom_highbd_v_predictor_64x64_avx512(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_v_predictor_8x16_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_v_predictor_8x32_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_v_predictor_8x4_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_aom_highbd_v_predictor_8x8_sse2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int32_t bd);

void svt_av1_dr_prediction_z1_avx2(uint8_t *dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t *above, const uint8_t *left, int32_t upsample_above, int32_t dx, int32_t dy);

void svt_av1_dr_prediction_z2_avx2(uint8_t *dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t *above, const uint8_t *left, int32_t upsample_above, int32_t upsample_left, int32_t dx, int32_t dy);

void svt_av1_dr_prediction_z3_avx2(uint8_t *dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t *above, const uint8_t *left, int32_t upsample_left, int32_t dx, int32_t dy);

void svt_av1_highbd_dr_prediction_z1_avx2(uint16_t *dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint16_t *above, const uint16_t *left, int32_t upsample_above, int32_t dx, int32_t dy, int32_t bd);

void svt_av1_highbd_dr_prediction_z2_avx2(uint16_t *dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint16_t *above, const uint16_t *left, int32_t upsample_above, int32_t upsample_left, int32_t dx, int32_t dy, int32_t bd);

void svt_av1_highbd_dr_prediction_z3_avx2(uint16_t *dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint16_t *above, const uint16_t *left, int32_t upsample_left, int32_t dx, int32_t dy, int32_t bd);

void svt_aom_dc_predictor_4x4_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_predictor_8x8_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_predictor_16x16_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_predictor_32x32_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_predictor_64x64_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_predictor_16x32_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_predictor_16x4_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_predictor_16x64_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_predictor_16x8_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_predictor_32x16_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_predictor_32x64_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_predictor_32x8_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_predictor_4x16_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_predictor_4x8_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_predictor_64x16_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_predictor_64x32_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_predictor_8x16_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_predictor_8x32_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_predictor_8x4_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

/* DC_PRED top */

void svt_aom_dc_left_predictor_4x4_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_left_predictor_8x8_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_left_predictor_16x16_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_left_predictor_32x32_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_left_predictor_64x64_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_left_predictor_16x32_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_left_predictor_16x4_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_left_predictor_16x64_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_left_predictor_16x8_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_left_predictor_32x16_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_left_predictor_32x64_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_left_predictor_32x8_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_left_predictor_4x16_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_left_predictor_4x8_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_left_predictor_64x16_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_left_predictor_64x32_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_left_predictor_8x16_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_left_predictor_8x32_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_left_predictor_8x4_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

/* DC_PRED top */

void svt_aom_dc_top_predictor_4x4_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_top_predictor_8x8_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_top_predictor_16x16_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_top_predictor_32x32_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_top_predictor_64x64_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_top_predictor_16x32_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_top_predictor_16x4_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_top_predictor_16x64_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_top_predictor_16x8_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_top_predictor_32x16_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_top_predictor_32x64_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_top_predictor_32x8_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_top_predictor_4x16_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_top_predictor_4x8_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_top_predictor_64x16_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_top_predictor_64x32_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_top_predictor_8x16_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_top_predictor_8x32_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_top_predictor_8x4_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

/* DC_PRED 128 */
void svt_aom_dc_128_predictor_4x4_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_128_predictor_8x8_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_128_predictor_16x16_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_128_predictor_32x32_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_128_predictor_64x64_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_128_predictor_16x32_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_128_predictor_16x4_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_128_predictor_16x64_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_128_predictor_16x8_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_128_predictor_32x16_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_128_predictor_32x64_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_128_predictor_32x8_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_128_predictor_4x16_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_128_predictor_4x8_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_128_predictor_64x16_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_128_predictor_64x32_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_128_predictor_8x16_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_128_predictor_8x32_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_dc_128_predictor_8x4_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

/* SMOOTH_H_PRED */
void svt_aom_smooth_h_predictor_64x64_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_h_predictor_32x32_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_h_predictor_16x16_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_h_predictor_8x8_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_h_predictor_4x4_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_h_predictor_16x32_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_h_predictor_16x4_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_h_predictor_16x64_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_h_predictor_16x8_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_h_predictor_32x16_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_h_predictor_32x64_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_h_predictor_32x8_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_h_predictor_4x16_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_h_predictor_4x8_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_h_predictor_64x16_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_h_predictor_64x32_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_h_predictor_8x16_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_h_predictor_8x32_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_h_predictor_8x4_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

/* SMOOTH_V_PRED */

void svt_aom_smooth_v_predictor_64x64_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_v_predictor_32x32_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_v_predictor_16x16_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_v_predictor_8x8_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_v_predictor_4x4_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_v_predictor_16x32_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_v_predictor_16x4_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_v_predictor_16x64_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_v_predictor_16x8_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_v_predictor_32x16_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_v_predictor_32x64_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_v_predictor_32x8_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_v_predictor_4x16_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_v_predictor_4x8_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_v_predictor_64x16_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_v_predictor_64x32_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_v_predictor_8x16_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_v_predictor_8x32_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_v_predictor_8x4_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

/* SMOOTH_PRED */

void svt_aom_smooth_predictor_64x64_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_predictor_32x32_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_predictor_16x16_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_predictor_8x8_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_predictor_4x4_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_predictor_16x32_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_predictor_16x4_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_predictor_16x64_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_predictor_16x8_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_predictor_32x16_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_predictor_32x64_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_predictor_32x8_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_predictor_4x16_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_predictor_4x8_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_predictor_64x16_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_predictor_64x32_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_predictor_8x16_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_predictor_8x32_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_smooth_predictor_8x4_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

/* V_PRED */
void svt_aom_v_predictor_4x4_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_v_predictor_8x8_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_v_predictor_16x16_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_v_predictor_32x32_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_v_predictor_64x64_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_v_predictor_16x32_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_v_predictor_16x4_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_v_predictor_16x64_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_v_predictor_16x8_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_v_predictor_32x16_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_v_predictor_32x64_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_v_predictor_32x8_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_v_predictor_4x16_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_v_predictor_4x8_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_v_predictor_64x16_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_v_predictor_64x32_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_v_predictor_8x16_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_v_predictor_8x32_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_v_predictor_8x4_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

/* H_PRED */

void svt_aom_h_predictor_4x4_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_h_predictor_8x8_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_h_predictor_16x16_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_h_predictor_32x32_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_h_predictor_64x64_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_h_predictor_16x32_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_h_predictor_16x4_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_h_predictor_16x64_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_h_predictor_16x8_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_h_predictor_32x16_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_h_predictor_32x64_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_h_predictor_32x8_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_h_predictor_4x16_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_h_predictor_4x8_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_h_predictor_64x16_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_h_predictor_64x32_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_h_predictor_8x16_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_h_predictor_8x32_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_h_predictor_8x4_sse2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_16x16_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_16x16_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_paeth_predictor_16x32_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_16x32_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_paeth_predictor_16x4_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_paeth_predictor_16x64_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_16x64_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_paeth_predictor_16x8_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_16x8_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_paeth_predictor_32x16_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_32x16_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_paeth_predictor_32x32_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_32x32_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_paeth_predictor_32x64_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_32x64_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_paeth_predictor_32x8_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_paeth_predictor_4x16_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_paeth_predictor_4x4_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_paeth_predictor_4x8_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_paeth_predictor_64x16_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_64x16_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_paeth_predictor_64x32_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_64x32_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_paeth_predictor_64x64_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);
void svt_aom_paeth_predictor_64x64_avx2(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_paeth_predictor_8x16_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_paeth_predictor_8x32_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_paeth_predictor_8x4_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_paeth_predictor_8x8_ssse3(uint8_t *dst, ptrdiff_t y_stride, const uint8_t *above, const uint8_t *left);

void svt_aom_highbd_paeth_predictor_16x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);

void svt_aom_highbd_paeth_predictor_16x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);

void svt_aom_highbd_paeth_predictor_16x4_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);

void svt_aom_highbd_paeth_predictor_16x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);

void svt_aom_highbd_paeth_predictor_16x8_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);

void svt_aom_highbd_paeth_predictor_2x2_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);

void svt_aom_highbd_paeth_predictor_32x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);

void svt_aom_highbd_paeth_predictor_32x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);

void svt_aom_highbd_paeth_predictor_32x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);

void svt_aom_highbd_paeth_predictor_32x8_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);

void svt_aom_highbd_paeth_predictor_4x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);

void svt_aom_highbd_paeth_predictor_4x4_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);

void svt_aom_highbd_paeth_predictor_4x8_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);

void svt_aom_highbd_paeth_predictor_64x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);

void svt_aom_highbd_paeth_predictor_64x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);

void svt_aom_highbd_paeth_predictor_64x64_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);

void svt_aom_highbd_paeth_predictor_8x16_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);

void svt_aom_highbd_paeth_predictor_8x32_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);

void svt_aom_highbd_paeth_predictor_8x4_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);

void svt_aom_highbd_paeth_predictor_8x8_avx2(uint16_t *dst, ptrdiff_t y_stride, const uint16_t *above, const uint16_t *left, int bd);

uint64_t svt_aom_sum_squares_i16_sse2(const int16_t *src, uint32_t N);

void svt_cdef_filter_block_avx2(uint8_t *dst8, uint16_t *dst16, int32_t dstride, const uint16_t *in, int32_t pri_strength, int32_t sec_strength, int32_t dir, int32_t pri_damping, int32_t sec_damping, int32_t bsize, int32_t coeff_shift, uint8_t subsampling_factor);
void svt_av1_cdef_filter_block_sse4_1(uint8_t *dst8, uint16_t *dst16, int32_t dstride, const uint16_t *in, int32_t pri_strength, int32_t sec_strength, int32_t dir, int32_t pri_damping, int32_t sec_damping, int32_t bsize, int32_t coeff_shift, uint8_t subsampling_factor);
void svt_cdef_filter_block_8xn_16_avx2(const uint16_t *const in, const int32_t pri_strength, const int32_t sec_strength, const int32_t dir, int32_t pri_damping, int32_t sec_damping, const int32_t coeff_shift, uint16_t *const dst, const int32_t dstride, uint8_t height, uint8_t subsampling_factor);
void svt_cdef_filter_block_8xn_16_avx512(const uint16_t *const in, const int32_t pri_strength, const int32_t sec_strength, const int32_t dir, int32_t pri_damping, int32_t sec_damping, const int32_t coeff_shift, uint16_t *const dst, const int32_t dstride, uint8_t height, uint8_t subsampling_factor);

void svt_av1_highbd_warp_affine_sse4_1(const int32_t *mat, const uint8_t *ref8b, const uint8_t *ref2b,  int width, int height, int stride8b, int stride2b, uint16_t *pred, int p_col, int p_row, int p_width, int p_height, int p_stride, int subsampling_x, int subsampling_y, int bd, ConvolveParams *conv_params, int16_t alpha, int16_t beta, int16_t gamma, int16_t delta);
void svt_av1_highbd_warp_affine_avx2(const int32_t *mat, const uint8_t *ref8b, const uint8_t *ref2b,  int width, int height, int stride8b, int stride2b, uint16_t *pred, int p_col, int p_row, int p_width, int p_height, int p_stride, int subsampling_x, int subsampling_y, int bd, ConvolveParams *conv_params, int16_t alpha, int16_t beta, int16_t gamma, int16_t delta);
void svt_av1_warp_affine_sse4_1(const int32_t *mat, const uint8_t *ref, int width, int height, int stride, uint8_t *pred, int p_col, int p_row, int p_width, int p_height, int p_stride, int subsampling_x, int subsampling_y, ConvolveParams *conv_params, int16_t alpha, int16_t beta, int16_t gamma, int16_t delta);
void svt_av1_warp_affine_avx2(const int32_t *mat, const uint8_t *ref, int width, int height, int stride, uint8_t *pred, int p_col, int p_row, int p_width, int p_height, int p_stride, int subsampling_x, int subsampling_y, ConvolveParams *conv_params, int16_t alpha, int16_t beta, int16_t gamma, int16_t delta);

void svt_aom_highbd_lpf_horizontal_14_sse2(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);

void svt_aom_highbd_lpf_horizontal_4_sse2(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);

void svt_aom_highbd_lpf_horizontal_6_sse2(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);

void svt_aom_highbd_lpf_horizontal_8_sse2(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);

void svt_aom_highbd_lpf_vertical_14_sse2(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);

void svt_aom_highbd_lpf_vertical_4_sse2(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);

void svt_aom_highbd_lpf_vertical_6_sse2(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);

void svt_aom_highbd_lpf_vertical_8_sse2(uint16_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh, int32_t bd);

void svt_aom_lpf_horizontal_14_sse2(uint8_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);

void svt_aom_lpf_horizontal_4_sse2(uint8_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);

void svt_aom_lpf_horizontal_6_sse2(uint8_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);

void svt_aom_lpf_horizontal_8_sse2(uint8_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);

void svt_aom_lpf_vertical_14_sse2(uint8_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);

void svt_aom_lpf_vertical_4_sse2(uint8_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);

void svt_aom_lpf_vertical_6_sse2(uint8_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);

void svt_aom_lpf_vertical_8_sse2(uint8_t *s, int32_t pitch, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh);

uint32_t Log2f_ASM(uint32_t x);

extern void svt_memcpy_intrin_sse (void  *dst_ptr, void  const *src_ptr, size_t size);

void svt_aom_hadamard_4x4_sse2(const int16_t *src_diff, ptrdiff_t src_stride, int32_t *coeff);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_aom_highbd_hadamard_8x8_avx2(const int16_t *src_diff, ptrdiff_t src_stride, int32_t *coeff);
#endif
#endif

#ifdef __cplusplus
}  // extern "C"
#endif

#endif

// clang-format on
