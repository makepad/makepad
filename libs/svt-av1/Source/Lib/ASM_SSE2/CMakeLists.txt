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


# ASM_SSE2 Directory CMakeLists.txt

# Include Encoder Subdirectories
include_directories(${PROJECT_SOURCE_DIR}/Source/API/
    ${PROJECT_SOURCE_DIR}/Source/Lib/Codec/
    ${PROJECT_SOURCE_DIR}/Source/Lib/C_DEFAULT/
    ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_SSE2/)

check_both_flags_add(-msse2)

if(CMAKE_C_COMPILER_ID STREQUAL "Intel" AND NOT WIN32)
    check_both_flags_add(-static-intel -w)
endif()

set(all_files
    av1_txfm_sse2.h
    compute_mean_intrin_sse2.c
    compute_mean_sse2.h
    convolve_2d_sse2.c
    convolve_sse2.c
    dlf_intrin_sse2.c
    dlf_sse2.h
    encodetxb_sse2.c
    fft_sse2.c
    highbd_intrapred_sse2.h
    highbd_intrapred_sse2.c
    highbd_subtract_sse2.c
    highbd_variance_sse2.c
    intra_pred_av1_intrin_sse2.c
    jnt_convolve_2d_sse2.c
    jnt_convolve_sse2.c
    lpf_common_sse2.h
    mcp_sse2.h
    me_sad_calc_intrin_sse2.c
    pack_unpack_intrin_sse2.c
    pic_operators_intrin_sse2.c
    picture_operators_sse2.h
    synonyms.h
    transpose_sse2.h
    variance_sse2.c
    variance_sse2.h
    wiener_convolve_sse2.c
    )

set(asm_files
    aom_subpixel_8t_sse2.asm
    highbd_intrapred_sse2_.asm
    highbd_variance_impl_sse2.asm
    intrapred_sse2.asm
    picture_operators_sse2.asm
    subpel_variance_sse2.asm
    subtract_sse2.asm
    )

add_library(ASM_SSE2 OBJECT ${all_files})

asm_compile_to_target(ASM_SSE2 ${asm_files})
