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

# ASM_AVX2 Directory CMakeLists.txt

# Include Encoder Subdirectories
include_directories(${PROJECT_SOURCE_DIR}/Source/API/
    ${PROJECT_SOURCE_DIR}/Source/Lib/Codec/
    ${PROJECT_SOURCE_DIR}/Source/Lib/C_DEFAULT/
    ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_SSE2/
    ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_SSSE3/
    ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_SSE4_1/
    ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_AVX2/
    ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_AVX512/)

check_both_flags_add(-mavx2)

if(CMAKE_C_COMPILER_ID STREQUAL "Intel")
    if(WIN32)
        # Intel Windows (*Note - The Warning level /W0 should be made to /W4 at some point)
        check_both_flags_add(/Qdiag-disable:10010,10148,10157 /W0)
    else()
        check_both_flags_add(-static-intel -w)
    endif()
endif()

if(MSVC)
    check_both_flags_add(/arch:AVX2)
endif()

set(all_files
    aom_subpixel_8t_intrin_avx2.c
    av1_inv_txfm_avx2.c
    av1_inv_txfm_avx2.h
    av1_quantize_avx2.c
    blend_a64_mask_avx2.c
    cdef_avx2.c
    cdef_block_avx2.c
    cfl_avx2.c
    comb_avg_sad_intrin_avx2.c
    compute_sad_avx2.h
    compute_sad_intrin_avx2.c
    convolve_2d_avx2.c
    convolve_avx2.c
    convolve_avx2.h
    corner_match_avx2.c
    encodetxb_avx2.c
    fft_avx2.c
    highbd_convolve_2d_avx2.c
    highbd_convolve_avx2.c
    highbd_fwd_txfm_avx2.c
    highbd_intra_pred_avx2.c
    highbd_inv_txfm_avx2.c
    highbd_hadamard_avx2.c
    highbd_jnt_convolve_avx2.c
    highbd_quantize_intrin_avx2.c
    highbd_variance_avx2.c
    highbd_warp_affine_avx2.c
    inter_prediction_com_avx2.c
    intra_pred_intrin_avx2.c
    itx_hbd.h
    itx_lbd.h
    jnt_convolve_2d_avx2.c
    jnt_convolve_avx2.c
    mc.h
    memory_avx2.h
    noise_model_avx2.c
    obmc_sad_avx2.c
    obmc_variance_avx2.c
    pack_unpack_intrinsic_avx2.c
    palette_avx2.c
    pickrst_avx2.c
    pickrst_avx2.h
    pic_operators_inline_avx2.h
    pic_operators_intrin_avx2.c
    resize_avx2.c
    restoration_pick_avx2.c
    selfguided_avx2.c
    sse_avx2.c
    ssim_avx2.c
    synonyms_avx2.h
    temporal_filtering_avx2.c
    transforms_intrin_avx2.c
    transpose_avx2.h
    txfm_common_avx2.h
    variance_avx2.c
    variance_impl_avx2.c
    warp_plane_avx2.c
    wiener_convolve_avx2.c
    wiener_convolve_avx2.h
    )

set(asm_files
    itx_avx2.asm
    itx16_avx2.asm
    mc_avx2.asm
    mc16_avx2.asm)
add_library(ASM_AVX2 OBJECT ${all_files})
asm_compile_to_target(ASM_AVX2 ${asm_files})
