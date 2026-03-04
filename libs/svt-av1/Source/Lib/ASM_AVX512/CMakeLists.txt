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

# Common/ASM_AVX512 Directory CMakeLists.txt

# Include Encoder Subdirectories
include_directories(${PROJECT_SOURCE_DIR}/Source/API/
    ${PROJECT_SOURCE_DIR}/Source/Lib/Codec/
    ${PROJECT_SOURCE_DIR}/Source/Lib/C_DEFAULT/
    ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_SSE2/
    ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_SSSE3/
    ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_SSE4_1/
    ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_AVX2/
    ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_AVX512/)

check_both_flags_add(
    -mavx2
    -mavx512f
    -mavx512bw
    -mavx512dq
    -mavx512vl)

if(MSVC)
    check_both_flags_add(/arch:AVX512)
endif()

if(CMAKE_C_COMPILER_ID STREQUAL "Intel")
    if(WIN32)
        # Intel Windows (*Note - The Warning level /W0 should be made to /W4 at some point)
        check_both_flags_add(/Qdiag-disable:10010,10148,10157 /W0)
    else()
        check_both_flags_add(-static-intel -w)
    endif()
endif()

set(all_files
    cdef_avx512.c
    cdef_block_avx512.c
    compute_sad_intrin_avx512.c
    convolve_2d_avx512.c
    convolve_avx512.c
    convolve_avx512.h
    encodetxb_avx512.c
    highbd_fwd_txfm_AVX512.c
    highbd_intra_pred_avx512.c
    highbd_inv_txfm_avx512.c
    jnt_convolve_2d_avx512.c
    jnt_convolve_avx512.c
    pickrst_avx512.c
    pic_operators_intrin_avx512.c
    synonyms_avx512.h
    transpose_avx512.h
    transpose_encoder_avx512.h
    variance_avx512.c
    wiener_convolve_avx512.c
    )

add_library(ASM_AVX512 OBJECT ${all_files})
