#
# Copyright(c) 2020 Intel Corporation
#
# This source code is subject to the terms of the BSD 2 Clause License and
# the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
# was not distributed with this source code in the LICENSE file, you can
# obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
# Media Patent License 1.0 was not distributed with this source code in the
# PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
#

# Get the current version from the git repo to generate a version header
#
# Following variables should be set by the caller:
# PACKAGE_VERSION_STRING: Initial value used if not a git checkout
# GIT_ROOT_DIR: Project root
# INPUT_FILE: Input file to replace version in
# OUTPUT_FILE: Output file to write the version header to
#
# The variable that should be used in the version header is PACKAGE_VERSION_STRING

set(PACKAGE_VERSION_STRING "v${PACKAGE_VERSION_STRING}")

find_package(Git QUIET)
if(Git_FOUND AND EXISTS "${GIT_ROOT_DIR}/.git")
    execute_process(COMMAND
        ${GIT_EXECUTABLE}  -C ${GIT_ROOT_DIR}
            describe
            --tags --dirty --abbrev
            --always --match "v[0-9].[0-9]*"
        RESULT_VARIABLE git_describe_status
        OUTPUT_VARIABLE git_describe_output
        ERROR_VARIABLE git_describe_error
        OUTPUT_STRIP_TRAILING_WHITESPACE
        ERROR_STRIP_TRAILING_WHITESPACE)

    if (git_describe_status)
        message(WARNING "Failure to get version from Git: ${git_describe_error}")
    else()
        set(PACKAGE_VERSION_STRING ${git_describe_output})
    endif()
endif()

message(STATUS "Configured version: ${PACKAGE_VERSION_STRING}")

configure_file(${INPUT_FILE} ${OUTPUT_FILE} @ONLY)
