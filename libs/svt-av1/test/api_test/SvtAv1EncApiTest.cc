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
 * @file SvtAv1EncApiTest.cc
 *
 * @brief SVT-AV1 encoder api test, check invalid input
 *
 * @author Cidana-Edmond, Cidana-Ryan, Cidana-Wenyao
 *
 ******************************************************************************/
#include "EbSvtAv1Enc.h"
#include "gtest/gtest.h"
#include "SvtAv1EncApiTest.h"

using namespace svt_av1_test;

namespace {

/** @brief set_parameter_null_pointer is a death test case
 * EncApiDeathTest.set_parameter_null_pointer is a test case for reporting a
 * death condition lead to ececptions or signals
 *
 * Test strategy: <br>
 * Report a death caused by set a null pointer to function
 * svt_av1_enc_set_parameter, which should not happen <br>
 *
 * Expected result: <br>
 * Capture a signal of death in function svt_av1_enc_set_parameter <br>
 *
 * Test coverage:
 * svt_av1_enc_set_parameter.
 */
TEST(EncApiDeathTest, set_parameter_null_pointer) {
    // death tests: TODO: alert, fix me! fix me!! fix me!!!
    ::testing::FLAGS_gtest_death_test_style = "threadsafe";
    SvtAv1Context context;
    memset(&context, 0, sizeof(context));

    // initialize encoder and get handle
    EXPECT_EQ(EB_ErrorBadParameter,
              svt_av1_enc_init_handle(&context.enc_handle, nullptr));
    // watch out, function down
    EXPECT_EQ(EB_ErrorBadParameter,
              svt_av1_enc_set_parameter(context.enc_handle, nullptr));
    // destroy encoder handle
    EXPECT_EQ(EB_ErrorInvalidComponent, svt_av1_enc_deinit_handle(nullptr));
    SUCCEED();
}

/** @brief check_null_pointer is a api test case
 * EncApiTest.check_null_pointer is a api test case for checking null pointer
 * parameters setting into api functions and expect report for a
 * EB_ErrorBadParameter return
 *
 * Test strategy: <br>
 * Input nullptr to the encoder API and check the return value.
 *
 * Expected result: <br>
 * Encoder API should not crash and report EB_ErrorBadParameter.
 *
 * Test coverage:
 * All the encoder parameters.
 */
TEST(EncApiTest, check_null_pointer) {
    SvtAv1Context context;
    memset(&context, 0, sizeof(context));

    // initialize encoder and with all null pointer
    EXPECT_EQ(EB_ErrorBadParameter, svt_av1_enc_init_handle(nullptr, nullptr));
    // initialize encoder and with all null pointer and get handle
    EXPECT_EQ(EB_ErrorBadParameter,
              svt_av1_enc_init_handle(&context.enc_handle, nullptr));
    // setup encoder parameters with null pointer
    EXPECT_EQ(EB_ErrorBadParameter,
              svt_av1_enc_set_parameter(nullptr, nullptr));
    // TODO: Some function will crash with nullptr input,
    // and it will block test on linux platform. please refer to
    // EncApiDeathTest-->check_null_pointer
    // EXPECT_EQ(EB_ErrorBadParameter,
    //          svt_av1_enc_set_parameter(context.enc_handle,
    //          nullptr));
    // open encoder with null pointer
    EXPECT_EQ(EB_ErrorBadParameter, svt_av1_enc_init(nullptr));
    // get end of sequence NAL with null pointer
    // EXPECT_EQ(EB_ErrorBadParameter, svt_av1_enc_send_picture(nullptr,
    // nullptr)); EXPECT_EQ(EB_ErrorBadParameter,
    // svt_av1_enc_get_packet(nullptr, nullptr, 0));
    // EXPECT_EQ(EB_ErrorBadParameter, svt_av1_get_recon(nullptr, nullptr)); No
    // return value, just feed nullptr as parameter. release output buffer with
    // null pointer
    svt_av1_enc_release_out_buffer(nullptr);
    // close encoder with null pointer
    EXPECT_EQ(EB_ErrorBadParameter, svt_av1_enc_deinit(nullptr));
    // destroy encoder handle with null pointer
    EXPECT_EQ(EB_ErrorInvalidComponent, svt_av1_enc_deinit_handle(nullptr));
    SUCCEED();
}

/** @brief check_normal_setup is a api test case
 * EncApiTest.check_normal_setup is a api test case with a normal setup
 * parameters into api functions and expect report for return EB_ErrorNone
 *
 * Test strategy: <br>
 * Input normal parameters to the encoder API and check the return value.
 *
 * Expected result: <br>
 * Encoder API should not crash and report EB_ErrorNone.
 *
 * Test coverage:
 * All the encoder parameters.
 *
 * Comments:
 * Disabled for it will hang after test report, might be thread can not exit
 * only happens without in IDE debugging mode.
 */
TEST(EncApiTest, DISABLED_check_normal_setup) {
    SvtAv1Context context;
    memset(&context, 0, sizeof(context));

    const int width = 1280;
    const int height = 720;

    // initialize encoder and get handle
    EXPECT_EQ(EB_ErrorNone,
              svt_av1_enc_init_handle(&context.enc_handle, &context.enc_params))
        << "svt_av1_enc_init_handle failed";
    // setup source width/height with default value
    context.enc_params.source_width = width;
    context.enc_params.source_height = height;
    // setup encoder parameters with all default
    EXPECT_EQ(
        EB_ErrorNone,
        svt_av1_enc_set_parameter(context.enc_handle, &context.enc_params))
        << "svt_av1_enc_set_parameter failed";
    // open encoder
    EXPECT_EQ(EB_ErrorNone, svt_av1_enc_init(context.enc_handle))
        << "svt_av1_enc_init failed";
    // close encoder
    EXPECT_EQ(EB_ErrorNone, svt_av1_enc_deinit(context.enc_handle))
        << "svt_av1_enc_deinit failed";
    // destroy encoder
    EXPECT_EQ(EB_ErrorNone, svt_av1_enc_deinit_handle(context.enc_handle))
        << "svt_av1_enc_deinit_handle failed";
}

/** @brief repeat_normal_setup is a api test case
 * EncApiTest.repeat_normal_setup is a api test case of repeating test with a
 * default normal setup to check for a resource or memory leak
 *
 * Test strategy: <br>
 * Input normal parameters to the encoder API and repeat the processing of
 * initialize and destroy encoder handle and check the return value.
 *
 * Expected result: <br>
 * Encoder can initialize normally without any error reported.
 *
 * Test coverage:
 * Initialize and destroy APIs.
 *
 * Comments:
 * Disabled for it causes memory leak, and lead to effect other tests
 */
TEST(EncApiTest, DISABLED_repeat_normal_setup) {
    SvtAv1Context context;
    memset(&context, 0, sizeof(context));

    const int width = 1280;
    const int height = 720;

    for (size_t i = 0; i < 500; ++i) {
        // initialize encoder and get handle
        ASSERT_EQ(
            EB_ErrorNone,
            svt_av1_enc_init_handle(&context.enc_handle, &context.enc_params))
            << "svt_av1_enc_init_handle failed at " << i << " times";
        // setup source width/height with default value
        context.enc_params.source_width = width;
        context.enc_params.source_height = height;
        // setup encoder parameters with all default
        ASSERT_EQ(
            EB_ErrorNone,
            svt_av1_enc_set_parameter(context.enc_handle, &context.enc_params))
            << "svt_av1_enc_set_parameter failed at " << i << " times";
        // TODO:if not calls svt_av1_enc_deinit, there is huge memory leak, fix
        // me
        // ASSERT_EQ(EB_ErrorNone, svt_av1_enc_deinit(context.enc_handle))
        //    << "svt_av1_enc_deinit failed";
        // destroy encoder
        ASSERT_EQ(EB_ErrorNone, svt_av1_enc_deinit_handle(context.enc_handle))
            << "svt_av1_enc_deinit_handle failed at " << i << " times";
    }
}

}  // namespace
