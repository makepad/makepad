/*
 * Copyright(c) 2019 Netflix, Inc.
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the
 * Alliance for Open Media Patent License 1.0 was not distributed with this
 * source code in the PATENTS file, you can obtain it at
 * https://www.aomedia.org/license/patent-license.
 */

/******************************************************************************
 * @file SvtAv1EncApiTest.h
 *
 * @brief Define SvtAv1Context struct.
 *
 * @author Cidana-Edmond
 *
 ******************************************************************************/
#include "EbSvtAv1Enc.h"
#include "gtest/gtest.h"

/** @defgroup svt_av1_test Enums and Structures definition of encoder api test
 *  Defines enumerates and sructures of data/type refered in encoder tests
 *  @{
 */
namespace svt_av1_test {

/** SvtAv1Context is a set of encoder contexts when creation and setup */
typedef struct {
    EbComponentType*
        enc_handle; /**< encoder handle, created from encoder library */
    EbSvtAv1EncConfiguration enc_params; /**< encoder parameter set */
} SvtAv1Context;

}  // namespace svt_av1_test

/** @} */  // end of svt_av1_test
