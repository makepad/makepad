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

# ASM_NEON_DOTPROD Directory CMakeLists.txt

# Include Encoder Subdirectories

check_both_flags_add(-march=armv8.2-a+dotprod)

add_library(ASM_NEON_DOTPROD OBJECT)
target_sources(
  ASM_NEON_DOTPROD
  PUBLIC aom_convolve8_neon_dotprod.c
  PUBLIC cdef_neon_dotprod.c
  PUBLIC compute_sad_neon_dotprod.c
  PUBLIC convolve_neon_dotprod.c
  PUBLIC convolve_scale_neon_dotprod.c
  PUBLIC corner_match_neon_dotprod.c
  PUBLIC jnt_convolve_neon_dotprod.c
  PUBLIC pic_analysis_neon_dotprod.c
  PUBLIC sad_neon_dotprod.c
  PUBLIC sse_neon_dotprod.c
  PUBLIC variance_neon_dotprod.c)

target_include_directories(
  ASM_NEON_DOTPROD
  PRIVATE ${PROJECT_SOURCE_DIR}/Source/API/
  PRIVATE ${PROJECT_SOURCE_DIR}/Source/Lib/Codec/
  PRIVATE ${PROJECT_SOURCE_DIR}/Source/Lib/C_DEFAULT/
  PRIVATE ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_NEON/)

