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

# ASM_SVE2 Directory CMakeLists.txt

# Include Encoder Subdirectories
add_library(ASM_SVE2 OBJECT)

check_both_flags_add(-march=armv9-a+i8mm+sve2)

target_sources(
  ASM_SVE2
  PUBLIC highbd_convolve_sve2.c
  PUBLIC highbd_jnt_convolve_sve2.c)

target_include_directories(
  ASM_SVE2
  PRIVATE ${PROJECT_SOURCE_DIR}/Source/API/
  PRIVATE ${PROJECT_SOURCE_DIR}/Source/Lib/Codec/
  PRIVATE ${PROJECT_SOURCE_DIR}/Source/Lib/C_DEFAULT/
  PRIVATE ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_NEON/
  PRIVATE ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_SVE/
  PRIVATE ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_SVE2/)

