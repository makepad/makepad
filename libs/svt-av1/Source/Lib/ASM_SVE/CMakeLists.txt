#
# Copyright (c) 2024, Alliance for Open Media. All rights reserved
#
# This source code is subject to the terms of the BSD 2 Clause License and the
# Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License was
# not distributed with this source code in the LICENSE file, you can obtain it
# at www.aomedia.org/license/software. If the Alliance for Open Media Patent
# License 1.0 was not distributed with this source code in the PATENTS file, you
# can obtain it at www.aomedia.org/license/patent.
#

# ASM_SVE Directory CMakeLists.txt

# Include Encoder Subdirectories
add_library(ASM_SVE OBJECT)

check_both_flags_add(-march=armv8.2-a+dotprod+i8mm+sve)

target_sources(
  ASM_SVE
  PUBLIC aom_sum_squares_sve.c
  PUBLIC av1_quantize_sve.c
  PUBLIC block_error_sve.c
  PUBLIC cdef_sve.c
  PUBLIC compute_sad_sve.c
  PUBLIC compute_sad_neoverse_v2.c
  PUBLIC corner_match_sve.c
  PUBLIC highbd_convolve_sve.c
  PUBLIC highbd_jnt_convolve_sve.c
  PUBLIC highbd_pickrst_sve.c
  PUBLIC highbd_sse_sve.c
  PUBLIC highbd_variance_sve.c
  PUBLIC highbd_warp_plane_sve.c
  PUBLIC pickrst_sve.c
  PUBLIC sad_sve.c
  PUBLIC temporal_filtering_sve.c
  PUBLIC warp_plane_sve.c
  PUBLIC wedge_utils_sve.c)

target_include_directories(
  ASM_SVE
  PRIVATE ${PROJECT_SOURCE_DIR}/Source/API/
  PRIVATE ${PROJECT_SOURCE_DIR}/Source/Lib/Codec/
  PRIVATE ${PROJECT_SOURCE_DIR}/Source/Lib/C_DEFAULT/
  PRIVATE ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_NEON/
  PRIVATE ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_NEON_DOTPROD/
  PRIVATE ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_NEON_I8MM/
  PRIVATE ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_SVE/)
