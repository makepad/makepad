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
 * @file SvtAv1E2EParamsTest.cc
 *
 * @brief Implementation of encoder parameter coverage test in E2E test
 *
 * @author Cidana-Edmond
 *
 ******************************************************************************/

#include <map>
#include <cmath>
#include "gtest/gtest.h"
#include "SvtAv1E2EFramework.h"
#include "../api_test/params.h"
#include "RefDecoder.h"

/**
 * @brief SVT-AV1 encoder parameter coverage E2E test
 *
 * Test strategy:
 * Config SVT-AV1 encoder with individual parameter, run the
 * conformance test and analyze the Bitstream to check if the params
 * take effect.
 *
 * Expected result:
 * No error is reported in encoding progress. The reconstructed frame
 * data is same as the output frame from reference decoder.
 *
 * Test coverage:
 * Almost all the encoder parameters except frame_rate_numerator and
 * frame_rate_denominator.
 */

using namespace svt_av1_e2e_test;
using namespace svt_av1_e2e_test_vector;
using std::map;
using std::stoul;
using std::string;
using std::to_string;

/** copied from EbRateControlProcess.c */
static const uint8_t quantizer_to_qindex[] = {
    0,   4,   8,   12,  16,  20,  24,  28,  32,  36,  40,  44,  48,
    52,  56,  60,  64,  68,  72,  76,  80,  84,  88,  92,  96,  100,
    104, 108, 112, 116, 120, 124, 128, 132, 136, 140, 144, 148, 152,
    156, 160, 164, 168, 172, 176, 180, 184, 188, 192, 196, 200, 204,
    208, 212, 216, 220, 224, 228, 232, 236, 240, 244, 249, 255,
};

/** get qp value with the given qindex */
static uint32_t get_qp(const int16_t qindex) {
    if (qindex > 255) {
        printf("qindex is larger than 255!\n");
        return 63;
    }

    uint32_t qp = 0;
    for (const uint8_t index : quantizer_to_qindex) {
        if (index == qindex)
            return qp;
        else if (index > qindex) {
            if ((index - qindex) > (qindex - quantizer_to_qindex[qp - 1]))
                return qp - 1;
            break;
        }
        qp++;
    }
    return qp;
}

/* clang-format off */
static const std::vector<EncTestSetting> default_enc_settings = {
    // test intra period length
    {"IntraPeriodTest1", {{"IntraPeriod", "3"}}, default_test_vectors},

    // test different qp
    {"QpTest1",
     {{"RateControlMode", "0"}, {"QP", "20"}}, default_test_vectors},
    {"QpTest2",
     {{"RateControlMode", "0"}, {"QP", "32"}}, default_test_vectors},
    {"QpTest3",
     {{"RateControlMode", "0"}, {"QP", "44"}}, default_test_vectors},

    // test rc mode {2, 3}, with {1Mbps, 0.75Mbps, 0.5Mbps} setting with
    // 480p
    {"RcTest4",
     {{"RateControlMode", "1"}, {"TargetBitRate", "1000000"}},
     res_480p_test_vectors},
    {"RcTest5",
     {{"RateControlMode", "1"}, {"TargetBitRate", "750000"}},
     res_480p_test_vectors},
    {"RcTest6",
     {{"RateControlMode", "1"}, {"TargetBitRate", "500000"}},
     res_480p_test_vectors},

    // test high bitrate with big min_qp, or low bitrate with small max_qp
    {"RcQpTest4",
     {{"RateControlMode", "1"}, {"TargetBitRate", "1000000"}, {"MinQpAllowed", "20"}},
     res_480p_test_vectors},
    {"RcQpTest5",
     {{"RateControlMode", "1"}, {"TargetBitRate", "500000"}, {"MaxQpAllowed", "50"}},
     res_480p_test_vectors},
    {"RcQpTest6",
     {{"RateControlMode", "1"}, {"TargetBitRate", "750000"}, {"MaxQpAllowed", "50"}, {"MinQpAllowed", "20"}},
     res_480p_test_vectors},
};
/* clang-format on */

class CodingOptionTest : public SvtAv1E2ETestFramework {
  public:
    void config_test() override {
        enable_recon = true;
        enable_decoder = true;
        enable_analyzer = true;
        enable_config = true;
        SvtAv1E2ETestFramework::config_test();
    }

    void post_process() override {
        if (refer_dec_) {
            RefDecoder::StreamInfo *stream_info = refer_dec_->get_stream_info();
            validate_enc_setting(stream_info);
        }
    }

  protected:
    void validate_enc_setting(RefDecoder::StreamInfo *stream_info) {
        EbSvtAv1EncConfiguration *config = &av1enc_ctx_.enc_params;

        // check profile, level and tier
        EXPECT_EQ(config->profile, stream_info->profile)
            << "config profile: " << config->profile << "got "
            << stream_info->profile;

        // Verify bit depth
        EXPECT_EQ(config->encoder_bit_depth, stream_info->bit_depth)
            << "config bitdepth: " << config->encoder_bit_depth << " got "
            << stream_info->bit_depth;

        // verify the color format
        EXPECT_EQ(config->encoder_color_format,
                  setup_video_format(stream_info->format))
            << "color format is mismatch";

        if (config->intra_period_length > 0) {
            EXPECT_EQ(config->intra_period_length,
                      stream_info->max_intra_period)
                << "config intra period " << config->intra_period_length
                << " got " << stream_info->max_intra_period;
        }

        // verify QP Setting
        uint32_t actual_min_qp = get_qp(stream_info->min_qindex);
        uint32_t actual_max_qp = get_qp(stream_info->max_qindex);
        EXPECT_LE(config->min_qp_allowed, actual_min_qp)
            << "Min qp allowd " << config->min_qp_allowed << " actual "
            << actual_min_qp;
        EXPECT_GE(config->max_qp_allowed, actual_max_qp)
            << "Max qp allowd " << config->max_qp_allowed << " actual "
            << actual_max_qp;
        if (config->rate_control_mode == SVT_AV1_RC_MODE_CQP_OR_CRF) {
            EXPECT_EQ(actual_min_qp, actual_max_qp)
                << "QP fluctuate in const qp mode";
        }

        // verify tile row and tile column
        uint32_t expect_cols =
            (uint32_t)((video_src_->get_width_with_padding() >> 2) /
                       (1 << config->tile_columns));
        uint32_t expect_rows =
            (uint32_t)((video_src_->get_height_with_padding() >> 2) /
                       (1 << config->tile_rows));
        printf("expect_cols %d, expect_rows %d\n", expect_cols, expect_rows);
        printf("tile_cols %d, tile_rows %d\n",
               stream_info->tile_cols,
               stream_info->tile_rows);
        EXPECT_EQ(expect_cols, stream_info->tile_cols)
            << "Tile columns " << stream_info->tile_cols << " actual"
            << expect_cols;
        EXPECT_EQ(expect_rows, stream_info->tile_rows)
            << "Tile rows " << stream_info->tile_rows << " actual"
            << expect_rows;
    }
};

TEST_P(CodingOptionTest, CheckEncOptionsUsingBitstream) {
    run_death_test();
}

INSTANTIATE_TEST_SUITE_P(SvtAv1, CodingOptionTest,
                         ::testing::ValuesIn(default_enc_settings),
                         EncTestSetting::GetSettingName);
