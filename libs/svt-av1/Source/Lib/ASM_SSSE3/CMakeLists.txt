#
# Copyright(c) 2019 Intel Corporation
#
# This source code is subject to the terms of the BSD 2 Clause License and
# the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
# was not distributed with this source code in the LICENSE file, you can
# obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
# Media Patent License 1.0 was not distributed with this source code in the
# PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
#

# ASM_SSSE3 Directory CMakeLists.txt

# Include Encoder Subdirectories
include_directories(${PROJECT_SOURCE_DIR}/Source/API/
    ${PROJECT_SOURCE_DIR}/Source/Lib/Codec/
    ${PROJECT_SOURCE_DIR}/Source/Lib/C_DEFAULT/
    ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_SSE2/
    ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_SSSE3/
    ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_SSE4_1/
    ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_AVX2/
    ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_AVX512/)

check_both_flags_add(-mssse3)

if(CMAKE_C_COMPILER_ID STREQUAL "Intel" AND NOT WIN32)
    check_both_flags_add(-static-intel -w)
endif()

set(all_files
    aom_subpixel_8t_intrin_ssse3.c
    av1_inv_txfm_ssse3.c
    av1_inv_txfm_ssse3.h
    highbd_convolve_2d_ssse3.c
    highbd_convolve_ssse3.c
    highbd_intrapred_intrin_ssse3.c
    highbd_wiener_convolve_ssse3.c
    intrapred_ssse3.c
    jnt_convolve_ssse3.c
    reconinter_ssse3.c
    variance_impl_ssse3.c
    )

set(asm_files
    aom_subpixel_bilinear_ssse3.asm
    aom_subpixel_8t_ssse3.asm
    subpel_variance_ssse3.asm)

add_library(ASM_SSSE3 OBJECT ${all_files})

asm_compile_to_target(ASM_SSSE3 ${asm_files})
