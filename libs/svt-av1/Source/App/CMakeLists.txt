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

# APP Directory CMakeLists.txt

# Include Subdirectories
include_directories(${PROJECT_SOURCE_DIR}/Source/API/)

set(all_files
    ../API/EbConfigMacros.h
    ../API/EbDebugMacros.h
    ../API/EbSvtAv1.h
    ../API/EbSvtAv1Enc.h
    ../API/EbSvtAv1ErrorCodes.h
    ../API/EbSvtAv1Formats.h
    ../API/EbSvtAv1Metadata.h
    app_config.c
    app_config.h
    app_context.c
    app_context.h
    app_input_y4m.c
    app_input_y4m.h
    app_main.c
    app_output_ivf.c
    app_output_ivf.h
    app_process_cmd.c
    svt_time.c
    svt_time.h
    )

# App Source Files
add_executable(SvtAv1EncApp
    ${all_files})

if(TARGET safestringlib)
    target_sources(SvtAv1EncApp PRIVATE $<TARGET_OBJECTS:safestringlib>)
endif()

#********** SET COMPILE FLAGS************
# Link the Encoder App
target_link_libraries(SvtAv1EncApp PRIVATE SvtAv1Enc)

install(TARGETS SvtAv1EncApp RUNTIME COMPONENT Runtime DESTINATION ${CMAKE_INSTALL_BINDIR})
