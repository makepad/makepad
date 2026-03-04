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
 * @file SvtAv1EncParamsTest.cc
 *
 * @brief SVT-AV1 encoder parameter configuration test
 *
 * @author Cidana-Edmond
 *
 ******************************************************************************/

#include "EbSvtAv1Enc.h"
#include "gtest/gtest.h"
#include "params.h"
#include "SvtAv1EncApiTest.h"

using namespace svt_av1_test_params;
using namespace svt_av1_test;

namespace {
/**
 * @brief SVT-AV1 encoder parameter configuration test
 *
 * Test strategy:
 * Feed default values, vaild values and invalid values of individual param
 * to the encoder and check if encoder return correctly.
 *
 * Expected result:
 * For default value and valid values, encoder should return EB_ErrorNone.
 * For invalid value, encoder should return EB_ErrorBadParameter.
 *
 * Test coverage:
 * Almost all the encoder parameters except frame_rate_numerator and
 * frame_rate_denominator
 */

/** @breif EncParamTestBase is the basic class framework to test individual
 * parameter
 */
class EncParamTestBase : public ::testing::Test {
  public:
    EncParamTestBase() {
        memset(&ctxt_, 0, sizeof(ctxt_));
        param_name_str_ = "";
    }
    EncParamTestBase(const std::string &param_name) {
        memset(&ctxt_, 0, sizeof(ctxt_));
        param_name_str_ = param_name;
    }
    virtual ~EncParamTestBase() {
    }

  public:
    /** Interfaces */
    /** Interface of running parameter check with default value*/
    virtual void run_default_param_check() = 0;
    /** Interface of running parameter check with valid value*/
    virtual void run_valid_param_check() = 0;
    /** Interface of running parameter check with invalid value*/
    virtual void run_invalid_param_check() = 0;
    /** Interface of running parameter check with special value*/
    virtual void run_special_param_check() = 0;

  protected:
    // Sets up the test fixture.
    virtual void SetUp() override {
        // initialize encoder and get handle
        ASSERT_EQ(EB_ErrorNone,
                  svt_av1_enc_init_handle(&ctxt_.enc_handle, &ctxt_.enc_params))
            << "svt_av1_enc_init_handle failed";
        // setup encoder parameters with all default
        ASSERT_NE(nullptr, ctxt_.enc_handle) << "enc_handle is invalid";
        // setup source width/height with default if not in test source_width or
        // source_height
        if (param_name_str_.compare("source_width")) {
            const int width = 1280;
            ctxt_.enc_params.source_width = width;
        }
        if (param_name_str_.compare("source_height")) {
            const int height = 720;
            ctxt_.enc_params.source_height = height;
        }
    }

    // Tears down the test fixture.
    virtual void TearDown() override {
        // TODO: svt_av1_enc_deinit should not be called here, for this test
        // does not call svt_av1_enc_init, but there is huge memory leak if only
        // calls svt_av1_enc_deinit_handle. please remove it after we pass
        // EncApiTest-->repeat_normal_setup
        ASSERT_EQ(EB_ErrorNone, svt_av1_enc_deinit(ctxt_.enc_handle))
            << "svt_av1_enc_deinit failed";
        // destroy encoder
        ASSERT_EQ(EB_ErrorNone, svt_av1_enc_deinit_handle(ctxt_.enc_handle))
            << "svt_av1_enc_deinit_handle failed";
    }

    /** setup some of the params with related params modified before set
     * to encoder */
    void config_enc_param() {
        // special cases for parameter
        if (!param_name_str_.compare("max_qp_allowed")) {
            ctxt_.enc_params.rate_control_mode = SVT_AV1_RC_MODE_VBR;
            ctxt_.enc_params.min_qp_allowed = MIN_QP_VALUE;
        } else if (!param_name_str_.compare("min_qp_allowed")) {
            ctxt_.enc_params.rate_control_mode = SVT_AV1_RC_MODE_VBR;
            ctxt_.enc_params.max_qp_allowed = MAX_QP_VALUE;
        } else if (!param_name_str_.compare("profile")) {
            if (ctxt_.enc_params.profile == 0) {
                /** profile(0) requires YUV420 */
                ctxt_.enc_params.encoder_color_format = EB_YUV420;
            } else if (ctxt_.enc_params.profile == 1) {
                /** profile(1) requires 8-bit YUV444 */
                ctxt_.enc_params.encoder_bit_depth = 8;
                ctxt_.enc_params.encoder_color_format = EB_YUV444;
            } else if (ctxt_.enc_params.profile == 2) {
                /** profile(2) requires 8-bit/10-bit YUV422 */
                ctxt_.enc_params.encoder_bit_depth = 8;
                ctxt_.enc_params.encoder_color_format = EB_YUV422;
            }
        } else if (!param_name_str_.compare("target_bit_rate")) {
            ctxt_.enc_params.rate_control_mode = SVT_AV1_RC_MODE_VBR;
        }
    }

  protected:
    SvtAv1Context ctxt_;         /**< context of encoder */
    std::string param_name_str_; /**< name of parameter for test */
};

/** Marcro defininition of printing parameter name when in failed */
#define PRINT_PARAM_FATAL(p) \
    << "svt_av1_enc_set_parameter " << #p << ": " << (int)(p) << " failed"

/** Marcro defininition of printing 2 parameters name when in failed */
#define PRINT_2PARAM_FATAL(p1, p2)                                       \
    << "svt_av1_enc_set_parameter " << #p1 << ": " << (int)(p1) << " + " \
    << #p2 << ": " << (int)(p2) << " failed"

/** Marcro defininition of batch processing check for default, valid, invalid
 * and special parameter check*/
#define PARAM_TEST(param_test)               \
    TEST_F(param_test, run_paramter_check) { \
        run_default_param_check();           \
        run_valid_param_check();             \
        run_invalid_param_check();           \
        run_special_param_check();           \
    }

/** @breif This class is a template based on EncParamTestBase to test each
 * parameter
 */
#define DEFINE_PARAM_TEST_CLASS(test_name, param_name)                        \
    class test_name : public EncParamTestBase {                               \
      public:                                                                 \
        test_name() : EncParamTestBase(#param_name) {                         \
        }                                                                     \
        virtual void run_default_param_check() override {                     \
            EncParamTestBase::SetUp();                                        \
            ASSERT_EQ(ctxt_.enc_params.param_name,                            \
                      GET_DEFAULT_PARAM(param_name));                         \
            EncParamTestBase::TearDown();                                     \
        }                                                                     \
        virtual void run_valid_param_check() override {                       \
            for (size_t i = 0; i < SIZE_VALID_PARAM(param_name); ++i) {       \
                EncParamTestBase::SetUp();                                    \
                ctxt_.enc_params.param_name = GET_VALID_PARAM(param_name, i); \
                config_enc_param();                                           \
                EXPECT_EQ(EB_ErrorNone,                                       \
                          svt_av1_enc_set_parameter(ctxt_.enc_handle,         \
                                                    &ctxt_.enc_params))       \
                PRINT_PARAM_FATAL(ctxt_.enc_params.param_name);               \
                EncParamTestBase::TearDown();                                 \
            }                                                                 \
        }                                                                     \
        virtual void run_invalid_param_check() override {                     \
            for (size_t i = 0; i < SIZE_INVALID_PARAM(param_name); ++i) {     \
                EncParamTestBase::SetUp();                                    \
                ctxt_.enc_params.param_name =                                 \
                    GET_INVALID_PARAM(param_name, i);                         \
                config_enc_param();                                           \
                EXPECT_EQ(EB_ErrorBadParameter,                               \
                          svt_av1_enc_set_parameter(ctxt_.enc_handle,         \
                                                    &ctxt_.enc_params))       \
                PRINT_PARAM_FATAL(ctxt_.enc_params.param_name);               \
                EncParamTestBase::TearDown();                                 \
            }                                                                 \
        }                                                                     \
        virtual void run_special_param_check() override {                     \
            /*do nothing for special cases*/                                  \
        }                                                                     \
                                                                              \
      protected:                                                              \
        virtual void SetUp() override { /* skip EncParamTestBase::SetUp() */  \
        }                                                                     \
        virtual void TearDown() override {                                    \
            /* skip EncParamTestBase::TearDown() */                           \
        }                                                                     \
    };

/** Test case for enc_mode*/
DEFINE_PARAM_TEST_CLASS(EncParamEncModeTest, enc_mode);
PARAM_TEST(EncParamEncModeTest);

/** Test case for intra_period_length*/
DEFINE_PARAM_TEST_CLASS(EncParamIntraPeridLenTest, intra_period_length);
PARAM_TEST(EncParamIntraPeridLenTest);

/** Test case for intra_refresh_type*/
DEFINE_PARAM_TEST_CLASS(EncParamIntraRefreshTypeTest, intra_refresh_type);
PARAM_TEST(EncParamIntraRefreshTypeTest);

/** Test case for hierarchical_levels*/
DEFINE_PARAM_TEST_CLASS(EncParamHierarchicalLvlTest, hierarchical_levels);
PARAM_TEST(EncParamHierarchicalLvlTest);

/** Test case for pred_structure*/
DEFINE_PARAM_TEST_CLASS(EncParamPredStructTest, pred_structure);
PARAM_TEST(EncParamPredStructTest);

/** Test case for source_width*/
DEFINE_PARAM_TEST_CLASS(EncParamSrcWidthTest, source_width);
PARAM_TEST(EncParamSrcWidthTest);

/** Test case for source_height*/
DEFINE_PARAM_TEST_CLASS(EncParamSrcHeightTest, source_height);
PARAM_TEST(EncParamSrcHeightTest);

/** Test case for encoder_bit_depth*/
DEFINE_PARAM_TEST_CLASS(EncParamEncBitDepthTest, encoder_bit_depth);
PARAM_TEST(EncParamEncBitDepthTest);

/** Test case for qp*/
DEFINE_PARAM_TEST_CLASS(EncParamQPTest, qp);
PARAM_TEST(EncParamQPTest);

/** Test case for use_qp_file*/
DEFINE_PARAM_TEST_CLASS(EncParamUseQPFileTest, use_qp_file);
PARAM_TEST(EncParamUseQPFileTest);

/** Test case for enable_dlf_flag*/
DEFINE_PARAM_TEST_CLASS(EncParamEnableDlfTest, enable_dlf_flag);
PARAM_TEST(EncParamEnableDlfTest);

/** Test case for film_grain_denoise_strength*/
DEFINE_PARAM_TEST_CLASS(EncParamFilmGrainDenoiseStrTest,
                        film_grain_denoise_strength);
PARAM_TEST(EncParamFilmGrainDenoiseStrTest);

/** Test case for rate_control_mode*/
DEFINE_PARAM_TEST_CLASS(EncParamRateCtrlModeTest, rate_control_mode);
PARAM_TEST(EncParamRateCtrlModeTest);

/** Test case for scene_change_detection*/
DEFINE_PARAM_TEST_CLASS(EncParamSceneChangeDectTest, scene_change_detection);
PARAM_TEST(EncParamSceneChangeDectTest);

/** Test case for target_bit_rate*/
DEFINE_PARAM_TEST_CLASS(EncParamTargetBitRateTest, target_bit_rate);
PARAM_TEST(EncParamTargetBitRateTest);

/** Test case for max_qp_allowed*/
DEFINE_PARAM_TEST_CLASS(EncParamMaxQPAllowTest, max_qp_allowed);
PARAM_TEST(EncParamMaxQPAllowTest);

/** Test case for min_qp_allowed*/
DEFINE_PARAM_TEST_CLASS(EncParamMinQPAllowTest, min_qp_allowed);
PARAM_TEST(EncParamMinQPAllowTest);
/** Test case for profile, requiure YUV 422 or 444 which is unsupported now */
// DEFINE_PARAM_TEST_CLASS(EncParamProfileTest, profile);
// PARAM_TEST(EncParamProfileTest);
#if !FIX_TIER
/** Test case for tier*/
DEFINE_PARAM_TEST_CLASS(EncParamTierTest, tier);
PARAM_TEST(EncParamTierTest);
#endif
/** Test case for level*/
DEFINE_PARAM_TEST_CLASS(EncParamLevelTest, level);
PARAM_TEST(EncParamLevelTest);

/** Test case for use_cpu_flags*/
DEFINE_PARAM_TEST_CLASS(EncParamOplLevelTest, use_cpu_flags);
PARAM_TEST(EncParamOplLevelTest);

/** Test case for logical_processors*/
DEFINE_PARAM_TEST_CLASS(EncParamLevelOfParallelismTest, level_of_parallelism);
PARAM_TEST(EncParamLevelOfParallelismTest);

/** Test case for recon_enabled*/
DEFINE_PARAM_TEST_CLASS(EncParamReconEnabledTest, recon_enabled);
PARAM_TEST(EncParamReconEnabledTest);

#if TILES
/** Test case for tile_columns*/
DEFINE_PARAM_TEST_CLASS(EncParamTileColsTest, tile_columns);
PARAM_TEST(EncParamTileColsTest);

/** Test case for tile_rows*/
DEFINE_PARAM_TEST_CLASS(EncParamTileRowsTest, tile_rows);
PARAM_TEST(EncParamTileRowsTest);
#endif

/** Test case for screen_content_mode*/
DEFINE_PARAM_TEST_CLASS(EncParamScreenContentModeTest, screen_content_mode);
PARAM_TEST(EncParamScreenContentModeTest);

/** Test case for enable_tf*/
DEFINE_PARAM_TEST_CLASS(EncParamEnableAltRefsTest, enable_tf);
PARAM_TEST(EncParamEnableAltRefsTest);

/** Test case for enable_overlays*/
DEFINE_PARAM_TEST_CLASS(EncParamEnableOverlaysTest, enable_overlays);
PARAM_TEST(EncParamEnableOverlaysTest);

/** Test case for color_range*/
DEFINE_PARAM_TEST_CLASS(EncParamColorRangeTest, color_range);
PARAM_TEST(EncParamColorRangeTest);

/** Test case for color_primaries*/
DEFINE_PARAM_TEST_CLASS(EncParamColorPrimariesTest, color_primaries);
PARAM_TEST(EncParamColorPrimariesTest);

/** Test case for transfer_characteristics*/
DEFINE_PARAM_TEST_CLASS(EncParamTransferCharacteristicsTest,
                        transfer_characteristics);
PARAM_TEST(EncParamTransferCharacteristicsTest);

/** Test case for matrix_coefficients*/
DEFINE_PARAM_TEST_CLASS(EncParamMatrixCoefficientsTest, matrix_coefficients);
PARAM_TEST(EncParamMatrixCoefficientsTest);

}  // namespace
