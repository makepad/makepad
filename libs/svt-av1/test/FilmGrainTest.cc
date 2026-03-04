/*
 * Copyright(c) 2019 Netflix, Inc.
 * Copyright (c) 2016, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the
 * Alliance for Open Media Patent License 1.0 was not distributed with this
 * source code in the PATENTS file, you can obtain it at
 * https://www.aomedia.org/license/patent-license.
 */
#include <stdlib.h>

#include "definitions.h"
#include "grainSynthesis.h"
#include "gtest/gtest.h"
#include "utility.h"
#include "FilmGrainExpectedResult.h"
#include "acm_random.h"
#include "noise_model.h"
#include "aom_dsp_rtcd.h"

#if CONFIG_ENABLE_FILM_GRAIN

static AomFilmGrain film_grain_test_vectors[3] = {
    /* Test 1 */
    {
        1 /* apply_grain */,
        1 /* update_parameters */,
        {{16, 0},
         {25, 136},
         {33, 144},
         {41, 160},
         {48, 168},
         {56, 136},
         {67, 128},
         {82, 144},
         {97, 152},
         {113, 144},
         {128, 176},
         {143, 168},
         {158, 176},
         {178, 184}},
        14 /* num_points_y */,
        {{16, 0},
         {20, 64},
         {28, 88},
         {60, 104},
         {90, 136},
         {105, 160},
         {134, 168},
         {168, 208}},
        8 /* num_cb_points */,
        {{16, 0},
         {28, 96},
         {56, 80},
         {66, 96},
         {80, 104},
         {108, 96},
         {122, 112},
         {137, 112},
         {169, 176}},
        9 /* num_cr_points */,
        11 /* scaling_shift */,
        2 /* ar_coeff_lag */,
        {0, 0, -58, 0, 0, 0, -76, 100, -43, 0, -51, 82},
        {0, 0, -49, 0, 0, 0, -36, 22, -30, 0, -38, 7, 39},
        {0, 0, -47, 0, 0, 0, -31, 31, -25, 0, -32, 13, -100},
        8 /* ar_coeff_shift */,
        247 /* cb_mult */,
        192 /* cb_luma_mult */,
        18 /* cb_offset */,
        229 /* cr_mult */,
        192 /* cr_luma_mult */,
        54 /* cr_offset */,
        0 /* overlap_flag */,
        1 /* clip_to_restricted_range */,
        8 /* bit_depth */,
        0 /* chroma_scaling_from_luma*/,
        0 /* grain_scale_shift*/,
        45231 /* random_seed */,
        0 /* ignore_ref */
    },
    /* Test 2 */
    {
        1 /* apply_grain */,
        0 /* update_parameters */,
        {{16, 0},
         {25, 136},
         {33, 144},
         {41, 160},
         {48, 168},
         {56, 136},
         {67, 128},
         {82, 144},
         {97, 152},
         {113, 144},
         {128, 176},
         {143, 168},
         {158, 176},
         {178, 184}},
        14 /* num_points_y */,
        {{16, 0},
         {20, 64},
         {28, 88},
         {60, 104},
         {90, 136},
         {105, 160},
         {134, 168},
         {168, 208}},
        8 /* num_cb_points */,
        {{16, 0},
         {28, 96},
         {56, 80},
         {66, 96},
         {80, 104},
         {108, 96},
         {122, 112},
         {137, 112},
         {169, 176}},
        9 /* num_cr_points */,
        11 /* scaling_shift */,
        2 /* ar_coeff_lag */,
        {0, 0, -58, 0, 0, 0, -76, 100, -43, 0, -51, 82},
        {0, 0, -49, 0, 0, 0, -36, 22, -30, 0, -38, 7, 39},
        {0, 0, -47, 0, 0, 0, -31, 31, -25, 0, -32, 13, -100},
        8 /* ar_coeff_shift */,
        247 /* cb_mult */,
        192 /* cb_luma_mult */,
        18 /* cb_offset */,
        229 /* cr_mult */,
        192 /* cr_luma_mult */,
        54 /* cr_offset */,
        0 /* overlap_flag */,
        1 /* clip_to_restricted_range */,
        8 /* bit_depth */,
        0 /* chroma_scaling_from_luma*/,
        0 /* grain_scale_shift*/,
        36007 /* random_seed */,
        0 /* ignore_ref */
    },

    /* Test 3 */
    {
        1 /* apply_grain */,
        1 /* update_parameters */,
        {{0, 96}, {255, 96}},
        2 /* num_points_y */,
        {{0, 64}, {255, 64}},
        2 /* num_cb_points */,
        {{0, 64}, {255, 64}},
        2 /* num_cr_points */,
        11 /* scaling_shift */,
        3 /* ar_coeff_lag */,
        {
            4, 1,   3, 0,   1,  -3, 8,  -3, 7,  -23, 1, -25,
            0, -10, 6, -17, -4, 53, 36, 5,  -5, -17, 8, 66,
        },
        {
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,   0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 127,
        },
        {
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,   0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 127,
        },
        7 /* ar_coeff_shift */,
        128 /* cb_mult */,
        192 /* cb_luma_mult */,
        256 /* cb_offset */,
        128 /* cr_mult */,
        192 /* cr_luma_mult */,
        256 /* cr_offset */,
        1 /* overlap_flag */,
        0 /* clip_to_restricted_range */,
        8 /* bit_depth */,
        0 /*chroma_scaling_from_luma*/,
        0 /* grain_scale_shift*/,
        45231 /* random_seed */,
        0 /* ignore_ref */
    },
};

TEST(FilmGrain, parameters_equality) {
    /* Film grain parameters equality should not depend on random_seed and
     * update_parameters values */
    EXPECT_EQ(svt_aom_film_grain_params_equal(film_grain_test_vectors,
                                              film_grain_test_vectors + 1),
              1);

    /* These two instances of film grain parameters are different */
    EXPECT_EQ(svt_aom_film_grain_params_equal(film_grain_test_vectors,
                                              film_grain_test_vectors + 2),
              0);
}

class AddFilmGrainTest : public ::testing::Test {
  public:
    static const int kWidth = 128;
    static const int kHeight = 128;
    static const int luma_size = kWidth * kHeight;
    static const int chroma_size = luma_size >> 2;

    void SetUp() override {
        luma_ = (uint8_t *)svt_aom_malloc(luma_size);
        cb_ = (uint8_t *)svt_aom_malloc(chroma_size);
        cr_ = (uint8_t *)svt_aom_malloc(chroma_size);
    }

    void TearDown() override {
        svt_aom_free(luma_);
        svt_aom_free(cb_);
        svt_aom_free(cr_);
    }

  protected:
    void init_data() {
        memset(luma_, 0, luma_size);
        memset(cb_, 0, chroma_size);
        memset(cr_, 0, chroma_size);
    }

    void check_output(int idx) {
        int fail_y = 0, fail_cb = 0, fail_cr = 0;
        for (int i = 0; i < luma_size; ++i)
            if (luma_[i] != add_grain_expected_luma[idx][i])
                ++fail_y;

        for (int i = 0; i < chroma_size; ++i)
            if (cb_[i] != add_grain_expected_cb[idx][i])
                ++fail_cb;

        for (int i = 0; i < chroma_size; ++i)
            if (cr_[i] != add_grain_expected_cr[idx][i])
                ++fail_cr;

        EXPECT_EQ(fail_y, 0);
        EXPECT_EQ(fail_cb, 0);
        EXPECT_EQ(fail_cr, 0);
    }

  protected:
    uint8_t *luma_;
    uint8_t *cb_;
    uint8_t *cr_;
};

TEST_F(AddFilmGrainTest, MatchTest) {
    for (int i = 0; i < 3; ++i) {
        init_data();
        svt_av1_add_film_grain_run(film_grain_test_vectors + i,
                                   luma_,
                                   cb_,
                                   cr_,
                                   kHeight,
                                   kWidth,
                                   kWidth,     /* luma stride */
                                   kWidth / 2, /* chroma stride */
                                   0,
                                   1,
                                   1);
        check_output(i);
        EXPECT_FALSE(HasFailure());
    }
}

extern "C" {
#include "pcs.h"
#include "pic_buffer_desc.h"
#include "pic_analysis_process.h"
}

static void svt_picture_buffer_desc_dctor(EbPtr p) {
    EbPictureBufferDesc *obj = (EbPictureBufferDesc *)p;
    if (obj->buffer_enable_mask & PICTURE_BUFFER_DESC_Y_FLAG) {
        EB_FREE_ALIGNED_ARRAY(obj->buffer_y);
        EB_FREE_ALIGNED_ARRAY(obj->buffer_bit_inc_y);
    }
    if (obj->buffer_enable_mask & PICTURE_BUFFER_DESC_Cb_FLAG) {
        EB_FREE_ALIGNED_ARRAY(obj->buffer_cb);
        EB_FREE_ALIGNED_ARRAY(obj->buffer_bit_inc_cb);
    }
    if (obj->buffer_enable_mask & PICTURE_BUFFER_DESC_Cb_FLAG) {
        EB_FREE_ALIGNED_ARRAY(obj->buffer_cr);
        EB_FREE_ALIGNED_ARRAY(obj->buffer_bit_inc_cr);
    }
}

// Return normally distrbuted values with standard deviation of sigma.
double randn(libaom_test::ACMRandom *random, double sigma) {
    while (1) {
        const double u = 2.0 * ((double)random->Rand31() /
                                testing::internal::Random::kMaxRange) -
                         1.0;
        const double v = 2.0 * ((double)random->Rand31() /
                                testing::internal::Random::kMaxRange) -
                         1.0;
        const double s = u * u + v * v;
        if (s > 0 && s < 1) {
            return sigma * (u * sqrt(-2.0 * log(s) / s));
        }
    }
    return 0;
}

static void denoise_and_model_dctor(EbPtr p) {
    AomDenoiseAndModel *obj = (AomDenoiseAndModel *)p;

    free(obj->flat_blocks);
    for (int32_t i = 0; i < 3; ++i) {
        EB_FREE_ARRAY(obj->denoised[i]);
        EB_FREE_ARRAY(obj->packed[i]);
    }
    svt_aom_noise_model_free(&obj->noise_model);
    svt_aom_flat_block_finder_free(&obj->flat_block_finder);
}

/* clang-format off */
static AomFilmGrain expected_film_grain = {
    1 /* apply_grain */,
    1 /* update_parameters */,
    {{0, 33}, {255, 34}, {0, 0}, {0, 0}, },
    2 /* num_y_points */,
    {{0, 21}, {94, 22}, {228, 23}, {255, 23}, {0, 0}, },
    4 /* num_cb_points */,
    {{0, 21}, {94, 22}, {228, 23}, {255, 23}, {0, 0}, },
    4 /* num_cr_points */,
    11,
    3,
    {-3, -5, -6, -5, -4, -2, -2, -3, -7, -4, -7, -6, -4, -1, -5, -6, -7, -15, -5, -1, -3, -2, -6, -15},
    {-3, -1, -10, -7, -6, -3, -3, -4, -9, -17, -13, -12, -4, 1, -7, -12, -26, -29, -17, -5, 2, 1, -9, -28, 111},
    {-3, -1, -10, -7, -6, -3, -3, -4, -9, -17, -13, -12, -4, 1, -7, -12, -26, -29, -17, -5, 2, 1, -9, -28, 111},
    7 /* ar_coeff_shift */,
    128 /* cb_mult */,
    192 /* cb_luma_mult */,
    256 /* cb_offset */,
    128 /* cr_mult */,
    192 /* cr_luma_mult */,
    256 /* cr_offset */,
    1 /* overlap_flag */,
    0 /* clip_to_restricted_range */,
    8 /* bit_depth */,
    0 /* chroma_scaling_from_luma */,
    0 /* grain_scale_shift */,
    0 /* random_seed */,
    0 /* ignore_ref */
};
/* clang-format on */

class DenoiseModelRunTest : public ::testing::Test {
  public:
    static const int width_ = 128;
    static const int height_ = 128;

    DenoiseModelRunTest() {
        for (int i = 0; i < 3; ++i)
            data_ptr_[i] = denoised_ptr_[i] = nullptr;
        memset(&output_film_grain, 0, sizeof(output_film_grain));

        // create EbPictureBufferDesc
        EbPictureBufferDescInitData pbd_init_data;
        pbd_init_data.max_width = width_;
        pbd_init_data.max_height = height_;
        pbd_init_data.bit_depth = EB_EIGHT_BIT;
        // allocate all the components
        pbd_init_data.buffer_enable_mask = PICTURE_BUFFER_DESC_FULL_MASK;
        pbd_init_data.left_padding = 0;
        pbd_init_data.right_padding = 0;
        pbd_init_data.top_padding = 0;
        pbd_init_data.bot_padding = 0;
        pbd_init_data.color_format = EB_YUV420;
        pbd_init_data.split_mode = false;
        pbd_init_data.is_16bit_pipeline = false;

        subsampling_x_ = (pbd_init_data.color_format == EB_YUV444 ? 0 : 1);
        subsampling_y_ = (pbd_init_data.color_format >= EB_YUV422 ? 0 : 1);

        EbErrorType err =
            svt_picture_buffer_desc_ctor(&in_pic_, &pbd_init_data);
        EXPECT_EQ(err, 0) << "create input pic fail";

        // create the denoise and noise model
        DenoiseAndModelInitData fg_init_data;
        fg_init_data.adaptive_film_grain = true;
        fg_init_data.encoder_bit_depth = EB_EIGHT_BIT;
        fg_init_data.encoder_color_format = EB_YUV420;
        fg_init_data.noise_level = 4;  // TODO: check the range;
        fg_init_data.denoise_apply = false;
        fg_init_data.width = width_;
        fg_init_data.height = height_;
        fg_init_data.stride_y = width_;
        fg_init_data.stride_cb = fg_init_data.stride_cr =
            fg_init_data.stride_y >> subsampling_x_;

        noise_model = {};
        err = svt_aom_denoise_and_model_ctor(&noise_model, &fg_init_data);
        EXPECT_EQ(err, 0) << "svt_aom_denoise_and_model_ctor fail";
    }

    ~DenoiseModelRunTest() {
        svt_picture_buffer_desc_dctor(&in_pic_);
        denoise_and_model_dctor(&noise_model);
    }

    void SetUp() override {
        random_.Reset(100171);
        data_ptr_[0] = in_pic_.buffer_y;
        data_ptr_[1] = in_pic_.buffer_cb;
        data_ptr_[2] = in_pic_.buffer_cr;

        memset(&output_film_grain, 0, sizeof(output_film_grain));

#if defined(ARCH_X86_64) || defined(ARCH_AARCH64)
        EbCpuFlags cpu_flags = svt_aom_get_cpu_flags_to_use();
#else
        EbCpuFlags cpu_flags = 0;
#endif
        svt_aom_setup_rtcd_internal(cpu_flags);
    }

    void init_data() {
        for (int y = 0; y < height_; ++y) {
            for (int x = 0; x < width_; ++x) {
                data_ptr_[0][y * width_ + x] =
                    int(64 + y + randn(&this->random_, 1));
                // Make the chroma planes completely correlated with the Y plane
                for (int c = 1; c < 3; ++c) {
                    data_ptr_[c][(y >> 1) * (width_ >> 1) + (x >> 1)] =
                        data_ptr_[0][y * width_ + x];
                }
            }
        }
    }

    void check_filmgrain() {
        EXPECT_EQ(svt_aom_film_grain_params_equal(&output_film_grain,
                                                  &expected_film_grain),
                  1);
    }

    void run_test() {
        init_data();

        svt_aom_denoise_and_model_run(
            &noise_model, &in_pic_, &output_film_grain, 0);
    }

  protected:
    int subsampling_x_;
    int subsampling_y_;
    EbPictureBufferDesc in_pic_;
    AomDenoiseAndModel noise_model;
    AomFilmGrain output_film_grain;
    libaom_test::ACMRandom random_;
    uint8_t *data_ptr_[3];
    uint8_t *denoised_ptr_[3];
};

TEST_F(DenoiseModelRunTest, OutputFilmGrainCheck) {
    run_test();
    check_filmgrain();
    EXPECT_FALSE(HasFailure());
}

#endif
