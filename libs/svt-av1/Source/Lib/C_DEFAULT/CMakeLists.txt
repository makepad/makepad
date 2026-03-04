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


# C_DEFAULT Directory CMakeLists.txt

# Include Encoder Subdirectories
include_directories(${PROJECT_SOURCE_DIR}/Source/API/
    ${PROJECT_SOURCE_DIR}/Source/Lib/Codec/
    ${PROJECT_SOURCE_DIR}/Source/Lib/C_DEFAULT/)

set(all_files
    blend_a64_mask_c.c
    cfl_c.c
    compute_sad_c.c
    compute_sad_c.h
    encode_txb_ref_c.c
    encode_txb_ref_c.h
    filterintra_c.c
    inter_prediction_c.c
    intra_prediction_c.c
    pack_unpack_c.c
    pack_unpack_c.h
    picture_operators_c.c
    picture_operators_c.h
    sad_av1.c
    variance.c
    )

add_library(C_DEFAULT OBJECT ${all_files})
