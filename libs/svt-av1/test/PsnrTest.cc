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
 * @file PsnrTest.cc
 *
 * @brief Unit test of PSNR calculation:
 * - svt_aom_get_y_sse_part
 * - svt_aom_get_u_sse_part
 * - svt_aom_get_v_sse_part
 * - svt_aom_highbd_get_y_sse_part
 * - svt_aom_highbd_get_u_sse_part
 * - svt_aom_highbd_get_v_sse_part
 *
 * @author Cidana-Edmond
 *
 ******************************************************************************/

#include "gtest/gtest.h"
#include "svt_psnr.h"
#include "random.h"
#include "util.h"
#include "pic_buffer_desc.h"

namespace {
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
using std::make_tuple;
using svt_av1_test_tool::SVTRandom;

/** setup_test_env is implemented in test/TestEnv.c */
extern "C" void setup_test_env();

using SseCalcPartFunc = int64_t (*)(const Yv12BufferConfig*,
                                    const Yv12BufferConfig*, int32_t, int32_t,
                                    int32_t, int32_t);
using PsnrCalcParam = std::tuple<int32_t, /**< width */
                                 int32_t  /**< height */
                                 >;
using PsnrCalcHbdParam = std::tuple<PsnrCalcParam, /**< basic params */
                                    uint32_t       /**< bit-depth */
                                    >;
using PsnrCalcFuncs = struct {
    SseCalcPartFunc y_sse_part_func;
    SseCalcPartFunc u_sse_part_func;
    SseCalcPartFunc v_sse_part_func;
};

static void svt_aom_free_frame_buffer(Yv12BufferConfig* ybf) {
    if (ybf->buffer_alloc_sz > 0)
        free(ybf->buffer_alloc);
    memset(ybf, 0, sizeof(Yv12BufferConfig));
}

template <typename Sample, typename ParamType>
class PsnrCalcTest : public ::testing::TestWithParam<ParamType> {
  public:
    PsnrCalcTest() : rnd_(8, false) {
        bd_ = 8;
        use_hbd_ = 0;
        memset(&tst_src_, 0, sizeof(tst_src_));
        memset(&tst_ref_, 0, sizeof(tst_ref_));
        memset(&lbd_src_, 0, sizeof(lbd_src_));
        memset(&lbd_ref_, 0, sizeof(lbd_ref_));
        setup_test_env();
    }
    virtual ~PsnrCalcTest() {
        svt_aom_free_frame_buffer(&tst_src_);
        svt_aom_free_frame_buffer(&tst_ref_);
        svt_aom_free_frame_buffer(&lbd_src_);
        svt_aom_free_frame_buffer(&lbd_ref_);
    }

  protected:
    void SetUp() override {
        rnd_.reset();
        prepare_buffer();
    }

    /**< prepare frame buffers for test */
    void prepare_buffer() {
        ASSERT_EQ(svt_aom_realloc_frame_buffer(&tst_src_,
                                               width_,
                                               height_,
                                               1,
                                               1,
                                               use_hbd_,
                                               32,
                                               16,
                                               NULL,
                                               NULL,
                                               NULL),
                  0);
        ASSERT_EQ(svt_aom_realloc_frame_buffer(&tst_ref_,
                                               width_,
                                               height_,
                                               1,
                                               1,
                                               use_hbd_,
                                               32,
                                               16,
                                               NULL,
                                               NULL,
                                               NULL),
                  0);
        ASSERT_EQ(
            svt_aom_realloc_frame_buffer(
                &lbd_src_, width_, height_, 1, 1, 0, 32, 16, NULL, NULL, NULL),
            0);
        ASSERT_EQ(
            svt_aom_realloc_frame_buffer(
                &lbd_ref_, width_, height_, 1, 1, 0, 32, 16, NULL, NULL, NULL),
            0);
    }

    /**< fill video frame buffer with random value */
    void fill_buffer(Yv12BufferConfig* ybf) {
        for (int32_t i = 0; i < 3; i++) {
            for (int32_t h = 0; h < ybf->heights[i]; h++) {
                for (int32_t w = 0; w < ybf->strides[i]; w++) {
                    if (ybf->flags & YV12_FLAG_HIGHBITDEPTH) {
                        uint16_t* buf16 = CONVERT_TO_SHORTPTR(ybf->buffers[i]);
                        buf16[h * ybf->strides[i] + w] = (uint16_t)rnd_.random()
                                                         << (bd_ - 8);
                    } else {
                        ybf->buffers[i][h * ybf->strides[i] + w] =
                            (uint8_t)rnd_.random();
                    }
                }
            }
        }
    }

    /**< fill video frame buffer with fixed value */
    void fill_buffer(Yv12BufferConfig* ybf, uint8_t fixed_value) {
        for (int32_t i = 0; i < 3; i++) {
            for (int32_t h = 0; h < ybf->heights[i]; h++) {
                for (int32_t w = 0; w < ybf->strides[i]; w++) {
                    if (ybf->flags & YV12_FLAG_HIGHBITDEPTH) {
                        uint16_t* buf16 = CONVERT_TO_SHORTPTR(ybf->buffers[i]);
                        buf16[h * ybf->strides[i] + w] = (uint16_t)fixed_value
                                                         << (bd_ - 8);
                    } else {
                        ybf->buffers[i][h * ybf->strides[i] + w] = fixed_value;
                    }
                }
            }
        }
    }

    /**< copy the content from video frame to other video frame */
    virtual void copy_buffer(Yv12BufferConfig* src, Yv12BufferConfig* dst) {
        for (size_t i = 0; i < 3; i++) {
            if (src->flags & YV12_FLAG_HIGHBITDEPTH) {
                memcpy(CONVERT_TO_SHORTPTR(dst->buffers[i]),
                       CONVERT_TO_SHORTPTR(src->buffers[i]),
                       src->strides[i] * src->heights[i] * 2);
            } else {
                memcpy(dst->buffers[i],
                       src->buffers[i],
                       src->strides[i] * src->heights[i]);
            }
        }
    }

  protected:
    int32_t width_;            /**< width*/
    int32_t height_;           /**< height*/
    uint32_t bd_;              /**< bit-depth */
    SVTRandom rnd_;            /**< random tools */
    int32_t use_hbd_;          /**< flag of use hbd */
    PsnrCalcFuncs calc_;       /**< psnr calculation functions */
    Yv12BufferConfig tst_src_; /**< video frame buffer for lbd/hbd test */
    Yv12BufferConfig tst_ref_; /**< video frame buffer has difference with
                                  tst_src_ for lbd/hbd test */
    Yv12BufferConfig lbd_src_; /**< video frame buffer in 8-bit for reference */
    Yv12BufferConfig lbd_ref_; /**< video frame buffer has difference with
                                  lbd_src_ for reference*/
};

static const PsnrCalcParam psnr_test_vector[] = {
    PsnrCalcParam(3840, 2160),
    PsnrCalcParam(1920, 1080),
    PsnrCalcParam(720, 480),
    PsnrCalcParam(360, 240),
    PsnrCalcParam(180, 120),
    PsnrCalcParam(64, 64),
};

class PsnrCalcHbdTest : public PsnrCalcTest<uint16_t, PsnrCalcHbdParam> {
  public:
    enum DistortionLvl {
        DistortionHigh = 2,
        DistortionMid = 4,
        DistortionLow = 6,
    };

  protected:
    PsnrCalcHbdTest() {
        width_ = std::get<0>(TEST_GET_PARAM(0));
        height_ = std::get<1>(TEST_GET_PARAM(0));
        bd_ = TEST_GET_PARAM(1);
        use_hbd_ = 1;
        calc_.y_sse_part_func = svt_aom_highbd_get_y_sse_part;
        calc_.u_sse_part_func = svt_aom_highbd_get_u_sse_part;
        calc_.v_sse_part_func = svt_aom_highbd_get_v_sse_part;
    }

    void run_accuracy_check(int32_t part_w = 0, int32_t part_h = 0) {
        PsnrCalcTest::fill_buffer(&lbd_src_, 128);
        copy_buffer(&lbd_src_, &tst_src_);

        /** this test adds a lot of distortion in dst buffer*/
        fill_buffer(&lbd_ref_, 128, DistortionHigh, true);
        copy_buffer(&lbd_ref_, &tst_ref_);
        if (part_w)
            calc_psnr_part_and_check(part_w, part_h);

        /** this test subtracts a lot of distortion in dst buffer*/
        fill_buffer(&lbd_ref_, 128, DistortionHigh, false);
        copy_buffer(&lbd_ref_, &tst_ref_);
        if (part_w)
            calc_psnr_part_and_check(part_w, part_h);

        /** this test adds less distortion in dst buffer*/
        fill_buffer(&lbd_ref_, 128, DistortionMid, true);
        copy_buffer(&lbd_ref_, &tst_ref_);
        if (part_w)
            calc_psnr_part_and_check(part_w, part_h);

        /** this test subtracts less distortion in dst buffer*/
        fill_buffer(&lbd_ref_, 128, DistortionMid, false);
        copy_buffer(&lbd_ref_, &tst_ref_);
        if (part_w)
            calc_psnr_part_and_check(part_w, part_h);

        /** this test adds a little distortion in dst buffer*/
        fill_buffer(&lbd_ref_, 128, DistortionLow, true);
        copy_buffer(&lbd_ref_, &tst_ref_);
        if (part_w)
            calc_psnr_part_and_check(part_w, part_h);

        /** this test subtracts a little distortion in dst buffer*/
        fill_buffer(&lbd_ref_, 128, DistortionLow, false);
        copy_buffer(&lbd_ref_, &tst_ref_);
        if (part_w)
            calc_psnr_part_and_check(part_w, part_h);
    }

#define MAX_PSNR 100.0
    double sse_to_psnr(double samples, double peak, double sse) {
        if (sse > 0.0) {
            const double psnr = 10.0 * log10(samples * peak * peak / sse);
            return psnr > MAX_PSNR ? MAX_PSNR : psnr;
        } else
            return MAX_PSNR;
    }

    /**< calculate psnr of partial video frame and check if the difference is
     * less than threshold */
    void calc_psnr_part_and_check(int32_t part_w, int32_t part_h) {
        ASSERT_NE(part_h, 0) << "search part width can not be zero";
        ASSERT_NE(part_w, 0) << "search part height can not be zero";
        ASSERT_LE(part_h, height_) << "search part is higher than source";
        ASSERT_LE(part_w, width_) << "search part is wider than source";

        int32_t part_hw = part_w >> 1, part_hh = part_h >> 1;
        for (int32_t h = 0; h <= height_ - part_h; h += part_h) {
            for (int32_t w = 0; w <= width_ - part_w; w += part_w) {
                int32_t hw = w >> 1, hh = h >> 1;
                double ref_y_sse = static_cast<double>(svt_aom_get_y_sse_part(
                    &lbd_src_, &lbd_ref_, w, part_w, h, part_h));
                double ref_u_sse = static_cast<double>(svt_aom_get_u_sse_part(
                    &lbd_src_, &lbd_ref_, hw, part_hw, hh, part_hh));
                double ref_v_sse = static_cast<double>(svt_aom_get_v_sse_part(
                    &lbd_src_, &lbd_ref_, hw, part_hw, hh, part_hh));
                double y_sse = static_cast<double>(calc_.y_sse_part_func(
                    &tst_src_, &tst_ref_, w, part_w, h, part_h));
                double u_sse = static_cast<double>(calc_.u_sse_part_func(
                    &tst_src_, &tst_ref_, hw, part_hw, hh, part_hh));
                double v_sse = static_cast<double>(calc_.v_sse_part_func(
                    &tst_src_, &tst_ref_, hw, part_hw, hh, part_hh));

                uint32_t peak = (1 << bd_) - 1;
                double size = (double)part_h * part_w;
                const double threshold = 0.3f;
                ASSERT_LE(fabs(sse_to_psnr(size, 255.0f, ref_y_sse) -
                               sse_to_psnr(size, peak, y_sse)),
                          threshold)
                    << "sse y error found in (" << w << "," << h << ")";
                ASSERT_LE(fabs(sse_to_psnr(size / 4, 255.0f, ref_u_sse) -
                               sse_to_psnr(size / 4, peak, u_sse)),
                          threshold)
                    << "sse u error found in (" << w << "," << h << ")";
                ASSERT_LE(fabs(sse_to_psnr(size / 4, 255.0f, ref_v_sse) -
                               sse_to_psnr(size / 4, peak, v_sse)),
                          threshold)
                    << "sse v error found in (" << w << "," << h << ")";
            }
        }
    }

    /**< fill the video frame buffer with fixed value has a distortion */
    void fill_buffer(Yv12BufferConfig* ybf, uint8_t fixed_value,
                     DistortionLvl lvl, bool positive) {
        for (int32_t i = 0; i < 3; i++) {
            for (int32_t h = 0; h < ybf->heights[i]; h++) {
                for (int32_t w = 0; w < ybf->strides[i]; w++) {
                    int8_t distortion =
                        (rnd_.random() >> lvl) * (positive ? 1 : -1);
                    if (ybf->flags & YV12_FLAG_HIGHBITDEPTH) {
                        uint16_t* buf16 = CONVERT_TO_SHORTPTR(ybf->buffers[i]);
                        buf16[h * ybf->strides[i] + w] =
                            (uint16_t)(fixed_value + distortion) << (bd_ - 8);
                    } else {
                        ybf->buffers[i][h * ybf->strides[i] + w] =
                            fixed_value + distortion;
                    }
                }
            }
        }
    }

    /**< copy the content from a HBD video frame to a LBD video frame */
    virtual void copy_buffer(Yv12BufferConfig* src,
                             Yv12BufferConfig* dst) override {
        for (int32_t i = 0; i < 3; i++) {
            for (int32_t h = 0; h < src->heights[i]; h++) {
                for (int32_t w = 0; w < src->strides[i]; w++) {
                    uint16_t* buf16 = CONVERT_TO_SHORTPTR(dst->buffers[i]);
                    buf16[h * dst->strides[i] + w] =
                        (uint16_t)src->buffers[i][h * src->strides[i] + w]
                        << (bd_ - 8);
                }
            }
        }
    }
};

TEST_P(PsnrCalcHbdTest, RunPartialAccuracyCheck) {
    int32_t part_w = width_ >> 1;
    int32_t part_h = height_ >> 1;
    run_accuracy_check(part_w, part_h);
}

static const uint32_t bit_depth_table[] = {8, 10, 12};

INSTANTIATE_TEST_SUITE_P(
    AV1, PsnrCalcHbdTest,
    ::testing::Combine(::testing::ValuesIn(psnr_test_vector),
                       ::testing::ValuesIn(bit_depth_table)));

#endif

}  // namespace
