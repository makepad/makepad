#
# Copyright (c) 2017, Alliance for Open Media. All rights reserved
#
# This source code is subject to the terms of the BSD 2 Clause License and the
# Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License was
# not distributed with this source code in the LICENSE file, you can obtain it
# at https://www.aomedia.org/license/software-license. If the Alliance for Open Media Patent
# License 1.0 was not distributed with this source code in the PATENTS file, you
# can obtain it at https://www.aomedia.org/license/patent-license.
#
include("${SVT_AV1_E2E_ROOT}/test_data_util.cmake")

# https://github.com/cheshirekow/cmake_format/issues/34
# cmake-format: off
if(NOT SVT_AV1_E2E_ROOT
    OR NOT SVT_AV1_TEST_FILE
    OR NOT AVT_AV1_TEST_CHECKSUM)
    message(FATAL_ERROR
        "SVT_AV1_E2E_ROOT, SVT_AV1_TEST_FILE and AVT_AV1_TEST_CHECKSUM must be
        defined.")
endif()
# cmake-format: on

set(SVT_AV1_TEST_DATA_URL "storage.googleapis.com/aom-test-data")

if(NOT SVT_AV1_STORE_PATH)
    set(SVT_AV1_STORE_PATH "$ENV{SVT_AV1_TEST_VECTOR_PATH}")
endif()

if("${SVT_AV1_STORE_PATH}" STREQUAL "")
    message(WARNING
        "Writing test data to ${SVT_AV1_E2E_ROOT}/../vectors/, set "
        "$SVT_AV1_TEST_VECTOR_PATH in your environment to avoid this warning.")
    set(SVT_AV1_STORE_PATH "${SVT_AV1_E2E_ROOT}/../vectors/")
endif()

if(NOT EXISTS "${SVT_AV1_STORE_PATH}")
    file(MAKE_DIRECTORY "${SVT_AV1_STORE_PATH}")
endif()

expand_test_file_paths("SVT_AV1_TEST_FILE" "${SVT_AV1_STORE_PATH}" "filepath")
expand_test_file_paths("SVT_AV1_TEST_FILE" "${SVT_AV1_TEST_DATA_URL}" "url")

check_file("${filepath}" "${AVT_AV1_TEST_CHECKSUM}" "needs_download")
if(needs_download)
    download_test_file("${url}" "${AVT_AV1_TEST_CHECKSUM}" "${filepath}")
endif()
