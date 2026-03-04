#
# Copyright (c) 2023, Alliance for Open Media. All rights reserved
#
# This source code is subject to the terms of the BSD 2 Clause License and the
# Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License was
# not distributed with this source code in the LICENSE file, you can obtain it
# at www.aomedia.org/license/software. If the Alliance for Open Media Patent
# License 1.0 was not distributed with this source code in the PATENTS file, you
# can obtain it at www.aomedia.org/license/patent.
#

# ASM_NEON Directory CMakeLists.txt

# Include Encoder Subdirectories
add_library(ASM_NEON OBJECT)
target_sources(
  ASM_NEON
  PUBLIC aom_convolve8_neon.c
  PUBLIC aom_sum_squares_neon.c
  PUBLIC av1_inv_txfm_neon.c
  PUBLIC av1_inv_txfm_neon.c
  PUBLIC av1_k_means_neon.c
  PUBLIC av1_quantize_neon.c
  PUBLIC blend_a64_mask_neon.c
  PUBLIC block_error_neon.c
  PUBLIC cdef_block_neon.c
  PUBLIC cdef_filter_block_neon.c
  PUBLIC cdef_neon.c
  PUBLIC cfl_neon.c
  PUBLIC compound_convolve_neon.c
  PUBLIC compute_sad_neon.c
  PUBLIC convolve_neon.c
  PUBLIC convolve_scale_neon.c
  PUBLIC corner_match_neon.c
  PUBLIC dav1d_asm.S
  PUBLIC dav1d_util.S
  PUBLIC deblocking_filter_intrinsic_neon.c
  PUBLIC encodetxb_neon.c
  PUBLIC hadamard_path_neon.c
  PUBLIC highbd_blend_a64_mask_neon.c
  PUBLIC highbd_convolve_neon.c
  PUBLIC highbd_convolve_scale_neon.c
  PUBLIC highbd_fwd_txfm_neon.c
  PUBLIC highbd_hadamard_neon.c
  PUBLIC highbd_inter_prediction_neon.c
  PUBLIC highbd_intra_prediction_neon.c
  PUBLIC highbd_inv_txfm_neon.c
  PUBLIC highbd_jnt_convolve_neon.c
  PUBLIC highbd_loopfilter_neon.c
  PUBLIC highbd_pickrst_neon.c
  PUBLIC highbd_quantize_neon.c
  PUBLIC highbd_sse_neon.c
  PUBLIC highbd_subtract_block_neon.c
  PUBLIC highbd_variance_neon.c
  PUBLIC highbd_warp_plane_neon.c
  PUBLIC highbd_wiener_convolve_neon.c
  PUBLIC inter_prediction_neon.c
  PUBLIC intra_prediction_neon.c
  PUBLIC itx.S
  PUBLIC mem_neon.c
  PUBLIC obmc_sad_neon.c
  PUBLIC obmc_variance_neon.c
  PUBLIC pack_unpack_intrin_neon.c
  PUBLIC pic_analysis_neon.c
  PUBLIC pickrst_neon.c
  PUBLIC picture_operators_intrinsic_neon.c
  PUBLIC restoration_pick_neon.c
  PUBLIC sad_neon.c
  PUBLIC selfguided_neon.c
  PUBLIC sse_neon.c
  PUBLIC subtract_block_neon.c
  PUBLIC temporal_filtering_neon.c
  PUBLIC transforms_intrin_neon.c
  PUBLIC upsampled_pred_neon.c
  PUBLIC variance_neon.c
  PUBLIC warp_plane_neon.c
  PUBLIC wedge_utils_neon.c
  PUBLIC wiener_convolve_neon.c)

target_include_directories(
  ASM_NEON
  PRIVATE ${PROJECT_SOURCE_DIR}/Source/API/
  PRIVATE ${PROJECT_SOURCE_DIR}/Source/Lib/Codec/
  PRIVATE ${PROJECT_SOURCE_DIR}/Source/Lib/C_DEFAULT/
  PRIVATE ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_NEON/)
# Causes the files to be explicitly compiled with `-x c`, which is wrong.
# https://cmake.org/cmake/help/git-master/policy/CMP0119.html
# set_source_files_properties(dav1d_asm.S dav1d_util.S itx.S PROPERTIES LANGUAGE
#                                                                       C)
