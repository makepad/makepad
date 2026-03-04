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

# Globals Directory CMakeLists.txt

# Include Encoder Subdirectories
include_directories(${PROJECT_SOURCE_DIR}/Source/Lib/API
        ${PROJECT_BINARY_DIR}/Source/Lib/Codec/
        ${PROJECT_SOURCE_DIR}/Source/Lib/C_DEFAULT/
        ${PROJECT_SOURCE_DIR}/third_party/fastfeat/
        )

if(NOT COMPILE_C_ONLY AND HAVE_X86_PLATFORM)
# Include Encoder Subdirectories
include_directories(
        ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_SSE2/
        ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_SSSE3/
        ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_SSE4_1/
        ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_AVX2/
        ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_AVX512/
        )
elseif(NOT COMPILE_C_ONLY AND HAVE_ARM_PLATFORM)
include_directories(
        ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_NEON/)
endif()

set(all_files
        enc_handle.c
        enc_handle.h
        enc_settings.c
        enc_settings.h
        metadata_handle.c
        metadata_handle.h
        )

add_library(GLOBALS OBJECT ${all_files})
if (BUILD_SHARED_LIBS)
        target_compile_definitions(GLOBALS
                PUBLIC EB_DLL
                PRIVATE EB_BUILDING_SHARED_LIBS)
endif()
add_dependencies(GLOBALS EbVersionHeaderGen)
