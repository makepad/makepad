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
 * @file params.h
 *
 * @brief Define the default values, valid values and invalid values for
 * individual params of encoder configuration.
 *
 * @author Cidana-Edmond
 *
 * @version 1.0 <br>
 * 04-2019 Cidana-Edmond
 *
 ******************************************************************************/

#ifndef _TEST_PARAMS_H_
#define _TEST_PARAMS_H_

#include <vector>
#include "EbSvtAv1Enc.h"
#include "gtest/gtest.h"
#include "definitions.h"

using std::vector;

/** @brief Definitions of Marco to help get test value from test vectors for
 * individual parameter of encoder configuration
 *
 */
#define SIZE_VALID_PARAM(p) (svt_av1_test_params::valid_##p.size())
#define SIZE_INVALID_PARAM(p) (svt_av1_test_params::invalid_##p.size())
#define SIZE_SPECIAL_PARAM(p) (svt_av1_test_params::special_##p.size())
#define GET_DEFAULT_PARAM(p) (svt_av1_test_params::default_##p[0])
#define GET_VALID_PARAM(p, n) (svt_av1_test_params::valid_##p[n])
#define GET_INVALID_PARAM(p, n) (svt_av1_test_params::invalid_##p[n])
#define GET_SPECIAL_PARAM(p, n) (svt_av1_test_params::special_##p[n])

/** @defgroup svt_av1_test_params Test vectors of individual params test
 *  Defines test values of encoder params in test vectors, includes default,
 * valid, invalid and special case in vector
 *  @{
 */

namespace svt_av1_test_params {

/** @brief Test vectors for parameter enc_mode, inludes default, valid and
 * invalid <br>
 * paramter reference: <br>
 * A preset defining the quality vs density tradeoff point that the
 * encoding is to be performed at. 0 is the highest quality mode, 3 is
 * the highest density mode. <br>
 *
 * Default is defined as MAX_ENC_PRESET.
 *
 */
static const vector<uint8_t> default_enc_mode = {
    MAX_ENC_PRESET,
};
static const vector<uint8_t> valid_enc_mode = {
    ENC_M0, /**< highest quality mode */
    ENC_M1,
    ENC_M2,
    ENC_M3,
    ENC_M4,
    ENC_M5,
    ENC_M6,
    ENC_M7,
    MAX_ENC_PRESET,
};
static const vector<uint8_t> invalid_enc_mode = {
    MAX_ENC_PRESET + 1,
};

/* The intra period defines the interval of frames after which you insert an
 * Intra refresh. It is strongly recommended to set the value to multiple of
 * 8 minus 1 the closest to 1 second (e.g. 55, 47, 31, 23 should be used for
 * 60, 50, 30, (24 or 25) respectively.
 *
 * -1 = no intra update.
 * -2 = auto.
 *
 * Deault is -2. */
static const vector<int32_t> default_intra_period_length = {
    -2,
};
static const vector<int32_t> valid_intra_period_length = {
    23,  // 24, 25 fps
    31,  // 30 fps
    47,  // 50 fps
    55,  // 60 fps
    -1,  // no intra update
    -2,  // auto
    0,   // all I frame
};
static const vector<int32_t> invalid_intra_period_length = {
    -3,   // < -2
    256,  // > 255
};

/* Random access.
 *
 * 1 = CRA, open GOP.
 * 2 = IDR, closed GOP.
 *
 * Default is 2. */
static const vector<SvtAv1IntraRefreshType> default_intra_refresh_type = {
    SVT_AV1_KF_REFRESH,
};
static const vector<SvtAv1IntraRefreshType> valid_intra_refresh_type = {
    SVT_AV1_FWDKF_REFRESH,  // CRA, open GOP
    SVT_AV1_KF_REFRESH,     // IDR, closed GOP
};
static const vector<SvtAv1IntraRefreshType> invalid_intra_refresh_type = {};

/* Number of hierarchical layers used to construct GOP.
 * Minigop size = 2^HierarchicalLevels.
 *
 * Default is 3. */
static const vector<uint32_t> default_hierarchical_levels = {
    3,
};
static const vector<uint32_t> valid_hierarchical_levels = {3, 4, 5};
static const vector<uint32_t> invalid_hierarchical_levels = {
    0,
    1,
    2,
    6,  // ...
};

/* Prediction structure used to construct GOP. There are two main structures
 * supported, which are: Low Delay and Random Access.
 *
 * In Low Delay structure, pictures within a mini GOP refer to the previously
 * encoded pictures in display order. In other words, pictures with display
 * order N can only be referenced by pictures with display order greater than
 * N, and it can only refer pictures with picture order lower than N. The Low
 * Delay structure can be flat structured (e.g. IPPPPPPP...) or hierarchically
 * structured. b/b pictures can be used instead of P/p pictures. However, the
 * reference picture list 0 and the reference picture list 1 will contain the
 * same reference picture.
 *
 * Following values are supported and defined in definitions.h
 * #define LOW_DELAY       1
 * #define RANDOM_ACCESS   2

 * In Random Access structure, the b/b pictures can refer to reference pictures
 * from both directions (past and future).
 *
 * Default is 2. */
static const vector<uint8_t> default_pred_structure = {
    RANDOM_ACCESS,
};
static const vector<uint8_t> valid_pred_structure = {LOW_DELAY, RANDOM_ACCESS};
static const vector<uint8_t> invalid_pred_structure = {
    /* _pred_structure override in code
    SVT_AV1_PRED_TOTAL_COUNT, SVT_AV1_PRED_TOTAL_COUNT + 1, EB_PRED_INVALID*/};

// Input Info
/* The width of input source in units of picture luma pixels.
 *
 * Default is 0. */
static const vector<uint32_t> default_source_width = {
    0,
};
static const vector<uint32_t> valid_source_width = {
    64,
    320,
    640,
    800,
    1280,
    1920,
    2560,
    3840,
    4096,  // ...
};
static const vector<uint32_t> invalid_source_width = {
    0,
    1,
    2,
    4,
    8,
    16,
    32,
    63,
    65,
    4097,  // ...
};

/* The height of input source in units of picture luma pixels.
 *
 * Default is 0. */
static const vector<uint32_t> default_source_height = {
    0,
};
static const vector<uint32_t> valid_source_height = {
    64,
    240,
    480,
    720,
    1080,
    1440,
    1600,
    2160,  // ...
};
static const vector<uint32_t> invalid_source_height = {
    0,
    1,
    2,
    4,
    8,
    16,
    32,
    63,
    65,
    2161,  // ...
};

// TODO: follwoing two parameters should be a combination test
/* Frame rate numerator. When zero, the encoder will use -fps if
 * FrameRateDenominator is also zero, otherwise an error is returned.
 *
 * Default is 0. */
static const vector<uint32_t> default_frame_rate_numerator = {
    0,
};
static const vector<uint32_t> valid_frame_rate_numerator = {
    1,
    2,
    3,
    4,
    5,
    6,
    8,
    10,
    20,
    30,
    50,
    60,
    90,
    120,
    240,
};
static const vector<uint32_t> invalid_frame_rate_numerator = {
    0xFFFFFFFF,
};
static const vector<uint32_t> special_frame_rate_numerator = {
    0,
};

/* Frame rate denominator. When zero, the encoder will use -fps if
 * FrameRateNumerator is also zero, otherwise an error is returned.
 *
 * Default is 0. */
static const vector<uint32_t> default_frame_rate_denominator = {
    0,
};
static const vector<uint32_t> valid_frame_rate_denominator = {
    1,
    2,
    3,
    4,
    5,
    6,
    10,
    30,
    60,
};
static const vector<uint32_t> invalid_frame_rate_denominator = {
    0xFFFFFFFF,
};
static const vector<uint32_t> special_frame_rate_denominator = {
    0,
};

/* Specifies the bit depth of input video.
 *
 * 8 = 8 bit.
 * 10 = 10 bit.
 *
 * Default is 8. */
static const vector<uint32_t> default_encoder_bit_depth = {
    8,
};
static const vector<uint32_t> valid_encoder_bit_depth = {
    8,   // 8 bit.
    10,  // 10 bit.
};
static const vector<uint32_t> invalid_encoder_bit_depth = {
    0,
    1,
    2,
    11,  // ...
};

/* Offline packing of the 2bits: requires two bits packed input.
 *
 * Default is 0. */
static const vector<uint32_t> default_compressed_ten_bit_format = {
    0,
};
static const vector<uint32_t> valid_compressed_ten_bit_format = {
    0,
    // 1, Not supported in this version
};
static const vector<uint32_t> invalid_compressed_ten_bit_format = {
    2,
    10,  // ...
};

/* Number of frames of sequence to be encoded. If number of frames is greater
 * than the number of frames in file, the encoder will loop to the beginning
 * and continue the encode.
 *
 * 0 = encodes the full clip.
 *
 * Default is 0. */
static const vector<uint64_t> default_frames_to_be_encoded = {
    0,
};
static const vector<uint64_t> valid_frames_to_be_encoded = {
    0,
    1,
    10,
    100,
    10000,
    (uint64_t)0xFFFFFFFFFFFFFFFF,  // ...
};
static const vector<uint64_t> invalid_frames_to_be_encoded = {
    // none
};

/* Super block size for motion estimation
 *
 * Default is 64. */
static const vector<uint32_t> default_sb_sz = {
    64,
};
static const vector<uint32_t> valid_sb_sz = {
    4,
    8,
    16,
    32,
    64,
    MAX_SB_SIZE,
};
static const vector<uint32_t> invalid_sb_sz = {
    /* sb_sz override in code
    0,
    (MAX_SB_SIZE + 1),
    (MAX_SB_SIZE << 1),
    */
};

/* Super block size
 * Only support 64 and 128
 * Default is 128. */
static const vector<uint32_t> default_super_block_size = {
    128,
};
static const vector<uint32_t> valid_super_block_size = {
    64,
    MAX_SB_SIZE,
};
static const vector<uint32_t> invalid_super_block_size = {
    /* super_block_size override in code
    0,
    (MAX_SB_SIZE + 1),
    (MAX_SB_SIZE << 1),
    */
};

// Quantization
/* Initial quantization parameter for the Intra pictures used under constant
 * qp rate control mode.
 *
 * Default is 50. */
static const vector<uint32_t> default_qp = {
    50,
};
static const vector<uint32_t> valid_qp = {
    MIN_QP_VALUE,
    1,
    2,
    10,
    25,
    32,
    50,
    56,
    62,
    MAX_QP_VALUE,
};
static const vector<uint32_t> invalid_qp = {
    (MAX_QP_VALUE + 1),
};

/* force qp values for every picture that are passed in the header pointer
 *
 * Default is 0.*/
static const vector<bool> default_use_qp_file = {
    false,
};
static const vector<bool> valid_use_qp_file = {
    false,
    true,
};
static const vector<bool> invalid_use_qp_file = {
    // none
};

// Deblock Filter
/* Flag to enable the Deblocking Loop Filtering.
 *
 * Default is true. */
static const vector<uint8_t> default_enable_dlf_flag = {
    1,
};
static const vector<uint8_t> valid_enable_dlf_flag = {
    0,
    1,
    2,
};
static const vector<uint8_t> invalid_enable_dlf_flag = {3};

/* Film grain denoising the input picture
 * Flag to enable the denoising
 *
 * Default is 0. */
// TODO: the description of this parameter is incorrect, refer to source code,
// it should be a bool
static const vector<uint32_t> default_film_grain_denoise_strength = {
    false,
};
static const vector<uint32_t> valid_film_grain_denoise_strength = {
    false,
    true,
};
static const vector<uint32_t> invalid_film_grain_denoise_strength = {
    // none
};

/* Warped motion
 *
 * Default is 0. */
static const vector<bool> default_enable_warped_motion = {
    true,
};
static const vector<bool> valid_enable_warped_motion = {
    false,
    true,
};
static const vector<bool> invalid_enable_warped_motion = {
    // none
};

/* Global motion
 *
 * Default is 1. */
static const vector<bool> default_enable_global_motion = {
    true,
};
static const vector<bool> valid_enable_global_motion = {
    false,
    true,
};
static const vector<bool> invalid_enable_global_motion = {
    // none
};

/* Flag to enable the use of default ME HME parameters.
 *
 * Default is 1. */
static const vector<bool> default_use_default_me_hme = {
    true,
};
static const vector<bool> valid_use_default_me_hme = {
    false,
    true,
};
static const vector<bool> invalid_use_default_me_hme = {
    // none
};

/* Flag to enable Hierarchical Motion Estimation.
 *
 * Default is 1. */
static const vector<bool> default_enable_hme_flag = {
    true,
};
static const vector<bool> valid_enable_hme_flag = {
    false,
    true,
};
static const vector<bool> invalid_enable_hme_flag = {
    // none
};

/* Flag to enable the use of non-swaure partitions
 *
 * Default is 0. */
static const vector<bool> default_ext_block_flag = {
    false,
};
static const vector<bool> valid_ext_block_flag = {
    false,
    true,
};
static const vector<bool> invalid_ext_block_flag = {
    // none
};

// ME Parameters
/* Number of search positions in the horizontal direction.
 *
 * Default depends on input resolution. */
static const vector<uint32_t> default_search_area_width = {
    16,  // 0,
};
static const vector<uint32_t> valid_search_area_width = {
    1,
    2,
    3,
    4,
    8,
    10,
    16,
    32,
    64,
    128,
    256,  // ...
};
static const vector<uint32_t> invalid_search_area_width = {
    0,
    257,
    1000,  // ...
};

/* Number of search positions in the vertical direction.
 *
 * Default depends on input resolution. */
static const vector<uint32_t> default_search_area_height = {
    7,  // 0,
};
static const vector<uint32_t> valid_search_area_height = {
    1,
    2,
    3,
    4,
    8,
    10,
    16,
    32,
    64,
    128,
    256,  // ...
};
static const vector<uint32_t> invalid_search_area_height = {
    0,
    257,
    1000,  // ...
};

// MD Parameters
/* Palette Mode
 *-1:Auto Mode(ON at level6 when SC is detected)
 * 0:OFF
 * 1:Slow    NIC=7/4/4
 * 2:        NIC=7/2/2
 * 3:        NIC=7/2/2 + No K means for non ref
 * 4:        NIC=4/2/1
 * 5:        NIC=4/2/1 + No K means for Inter frame
 * 6:Fastest NIC=4/2/1 + No K means for non base + step for non base for
 * most dominant
 * Default is -1. */
static const vector<int32_t> default_palette_level = {-1};
static const vector<int32_t> valid_palette_level = {-1, 0, 1, 2, 3, 4, 5, 6};
static const vector<int32_t> invalid_palette_level = {-2, 7};

/* Enable the use of Constrained Intra, which yields sending two picture
 * parameter sets in the elementary streams .
 *
 * Default is 0. */
static const vector<bool> default_constrained_intra = {
    false,
};
static const vector<bool> valid_constrained_intra = {
    false,
    true,
};
static const vector<bool> invalid_constrained_intra = {
    // none
};

// Rate Control
/* Rate control mode.
 *
 * 0 = Constant QP.
 * 1 = Average BitRate.
 *
 * Default is 0. */
static const vector<uint8_t> default_rate_control_mode = {0};
static const vector<uint8_t> valid_rate_control_mode = {0, 1, 2};
static const vector<uint8_t> invalid_rate_control_mode = {3, 4};
/* Flag to enable the scene change detection algorithm.
 *
 * Default is 0. */
static const vector<uint32_t> default_scene_change_detection = {
    0,
};
static const vector<uint32_t> valid_scene_change_detection = {
    0,
    1,
};
static const vector<uint32_t> invalid_scene_change_detection = {
    2,
};

/* Target bitrate in bits/second, only apllicable when rate control mode is
 * set to 1.
 *
 * Default is 7000000. */
static const vector<uint32_t> default_target_bit_rate = {
    7000000,
};
static const vector<uint32_t> valid_target_bit_rate = {
    0,
    1,
    100,
    1000,
    10000,
    100000,
    1000000,
    7000000,
    10000000,
    0xFFFFFFFF,
};
static const vector<uint32_t> invalid_target_bit_rate = {
    // none
};

/* Maxium QP value allowed for rate control use, only applicable when rate
 * control mode is set to 1. It has to be greater or equal to minQpAllowed.
 *
 * Default is 63. */
static const vector<uint32_t> default_max_qp_allowed = {
    MAX_QP_VALUE,
};
static const vector<uint32_t> valid_max_qp_allowed = {
    MIN_QP_VALUE,
    1,
    2,
    10,
    25,
    32,
    50,
    56,
    62,
    MAX_QP_VALUE,
};
static const vector<uint32_t> invalid_max_qp_allowed = {
    (MAX_QP_VALUE + 1),
};

/* Minimum QP value allowed for rate control use, only applicable when rate
 * control mode is set to 1. It has to be smaller or equal to maxQpAllowed.
 *
 * Default is 0. */
/*
 * There is a value check for min_qp_allowed in EbEncHandle.c :
 * else if (config->min_qp_allowed >= MAX_QP_VALUE) {
 *     SVT_LOG("Error instance %u: MinQpAllowed must be [0 - %d]\n",
 *         channel_number + 1, MAX_QP_VALUE-1); return_error =
 *         EB_ErrorBadParameter;
 * }
 * The maximum valid value should be MAX_QP_VALUE - 10.
 */
static const vector<uint32_t> default_min_qp_allowed = {
    10,
};
static const vector<uint32_t> valid_min_qp_allowed = {
    MIN_QP_VALUE,
    1,
    2,
    10,
    25,
    32,
    50,
    56,
    MAX_QP_VALUE - 1,
};
static const vector<uint32_t> invalid_min_qp_allowed = {
    MAX_QP_VALUE,
};

// Tresholds
/* Flag to signal that the input yuv is HDR10 BT2020 using SMPTE ST2048,
 * requires
 *
 * Default is 0. */
static const vector<uint32_t> default_high_dynamic_range_input = {
    0,
};
static const vector<uint32_t> valid_high_dynamic_range_input = {
    0,
    1,
};
static const vector<uint32_t> invalid_high_dynamic_range_input = {
    2,
};

/* Defined set of coding tools to create Bitstream.
 *
 * 1 = Main, allows bit depth of 8.
 * 2 = Main 10, allows bit depth of 8 to 10.
 *
 * Default is 0. */
static const vector<uint32_t> default_profile = {
    PROFILE_0,
};
static const vector<uint32_t> valid_profile = {
    PROFILE_0,
    PROFILE_1,
    PROFILE_2,
};
static const vector<uint32_t> invalid_profile = {
    MAX_PROFILES,
};

/* Constraints for Bitstream in terms of max bitrate and max buffer size.
 *
 * 0 = Main, for most applications.
 * 1 = High, for demanding applications.
 *
 * Default is 0. */
static const vector<uint32_t> default_tier = {
    0,
};
static const vector<uint32_t> valid_tier = {
    0,
    1,
};
static const vector<uint32_t> invalid_tier = {
    /* tier override in code
    2,
    */
};

/* Constraints for Bitstream in terms of max bitrate and max buffer size.
 *
 * 0 = auto determination.
 *
 * Default is 0. */
static const vector<uint32_t> default_level = {
    0,
};
static const vector<uint32_t> valid_level = {0, 1, 2, 10, 64, 100, 0xFFFFFFFF};
static const vector<uint32_t> invalid_level = {
    // none
};

/* Assembly instruction set used by encoder.
 *
 * 0 = non-SIMD, C only.
 * EB_CPU_FLAGS_ALL = auto-select highest assembly instruction set supported.
 *
 * Default is EB_CPU_FLAGS_ALL. */
static const vector<EbCpuFlags> default_use_cpu_flags = {
    EB_CPU_FLAGS_ALL,
};
static const vector<EbCpuFlags> valid_use_cpu_flags = {
#ifdef ARCH_X86_64
    EB_CPU_FLAGS_MMX,
    EB_CPU_FLAGS_SSE,
    EB_CPU_FLAGS_SSE2,
    EB_CPU_FLAGS_SSE3,
    EB_CPU_FLAGS_SSSE3,
    EB_CPU_FLAGS_SSE4_1,
    EB_CPU_FLAGS_SSE4_2,
    EB_CPU_FLAGS_AVX,
    EB_CPU_FLAGS_AVX2,
    EB_CPU_FLAGS_AVX512F,
    EB_CPU_FLAGS_AVX512CD,
    EB_CPU_FLAGS_AVX512DQ,
    EB_CPU_FLAGS_AVX512BW,
    EB_CPU_FLAGS_AVX512VL,
    EB_CPU_FLAGS_AVX512ICL,
#elif defined(ARCH_AARCH64)
    EB_CPU_FLAGS_NEON,
    EB_CPU_FLAGS_ARM_CRC32,
    EB_CPU_FLAGS_NEON_DOTPROD,
    EB_CPU_FLAGS_NEON_I8MM,
    EB_CPU_FLAGS_SVE,
    EB_CPU_FLAGS_SVE2,
#endif
};
static const vector<EbCpuFlags> invalid_use_cpu_flags = {EB_CPU_FLAGS_INVALID};

/* Flag to enable the Speed Control functionality to achieve the real-time
 * encoding speed defined by dynamically changing the encoding preset to meet
 * the average speed defined in injectorFrameRate. When this parameter is set
 * to 1 it forces -inj to be 1 -inj-frm-rt to be set to the -fps.
 *
 * Default is 0. */
static const vector<uint32_t> default_speed_control_flag = {
    0,
};
static const vector<uint32_t> valid_speed_control_flag = {
    0,
    1,
};
static const vector<uint32_t> invalid_speed_control_flag = {
    2,
};

// Threads management

/* The level of parallelism the encoder employs (how many threads and pictures
 * to create). */
static const vector<uint32_t> default_level_of_parallelism = {
    0,
};
static const vector<uint32_t> valid_level_of_parallelism = {
    0,
    1,
    2,
    3,
    4,
    5,
    6,
    7,
    8,
    9,
    10,
    20,
    40,
    1000,
    0xFFFFFFFF,
};
static const vector<uint32_t> invalid_level_of_parallelism = {
    // ...
};

// Debug tools

/* Output reconstructed yuv used for debug purposes. The value is set through
 * ReconFile token (-o) and using the feature will affect the speed of encoder.
 *
 * Default is 0. */
static const vector<uint32_t> default_recon_enabled = {false};
static const vector<uint32_t> valid_recon_enabled = {false, true};
static const vector<uint32_t> invalid_recon_enabled = {/** none */};

#if TILES
/* Log 2 Tile Rows and columns . 0 means no tiling,1 means that we split the
 * dimension into 2 Default is 0. */
static const vector<int32_t> default_tile_columns = {
    0,
};
static const vector<int32_t> valid_tile_columns = {
    0,
    1,
    2,
    3,
    4,
    5,
    6,
};
static const vector<int32_t> invalid_tile_columns = {
    -1,
    7,
};

static const vector<int32_t> default_tile_rows = {
    0,
};
static const vector<int32_t> valid_tile_rows = {
    0,
    1,
    2,
    3,
    4,
    5,
    6,
};
static const vector<int32_t> invalid_tile_rows = {
    -1,
    7,
};
#endif

/* Flag to signal the content being a screen sharing content type
 *
 * Default is 2. */
static const vector<uint32_t> default_screen_content_mode = {2};
static const vector<uint32_t> valid_screen_content_mode = {0, 1, 2, 3};
static const vector<uint32_t> invalid_screen_content_mode = {4};

/* Variables to control the use of ALT-REF (temporally filtered frames)
 */
static const vector<uint8_t> default_enable_tf = {1};
static const vector<uint8_t> valid_enable_tf = {0, 1, 2};
static const vector<uint8_t> invalid_enable_tf = {3};

static const vector<uint8_t> default_altref_strength = {5};
static const vector<uint8_t> valid_altref_strength = {0, 1, 2, 3, 4, 5, 6};
static const vector<uint8_t> invalid_altref_strength = {7};

static const vector<uint8_t> default_altref_nframes = {7};
static const vector<uint8_t> valid_altref_nframes = {
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10};
static const vector<uint8_t> invalid_altref_nframes = {11};

static const vector<bool> default_enable_overlays = {false};
static const vector<bool> valid_enable_overlays = {false, true};
static const vector<bool> invalid_enable_overlays = {/*none*/};

/* Variables to control the super-resolution tool
 */
static const vector<uint8_t> default_superres_mode = {0};
static const vector<uint8_t> valid_superres_mode = {0, 1, 2};
static const vector<uint8_t> invalid_superres_mode = {3};

static const vector<uint8_t> default_superres_denom = {8};
static const vector<uint8_t> valid_superres_denom = {
    8, 9, 10, 11, 12, 13, 14, 15, 16};
static const vector<uint8_t> invalid_superres_denom = {7};

static const vector<uint8_t> default_superres_kf_denom = {8};
static const vector<uint8_t> valid_superres_kf_denom = {
    8, 9, 10, 11, 12, 13, 14, 15, 16};
static const vector<uint8_t> invalid_superres_kf_denom = {7};

// Color description
/* Color range
 */
static const vector<EbColorRange> default_color_range = {EB_CR_STUDIO_RANGE};
static const vector<EbColorRange> valid_color_range = {EB_CR_STUDIO_RANGE,
                                                       EB_CR_FULL_RANGE};
static const vector<EbColorRange> invalid_color_range = {};

/* Color primaries
 */
static const vector<EbColorPrimaries> default_color_primaries = {
    EB_CICP_CP_UNSPECIFIED};
static const vector<EbColorPrimaries> valid_color_primaries = {
    EB_CICP_CP_BT_709,
    EB_CICP_CP_UNSPECIFIED,
    EB_CICP_CP_BT_470_M,
    EB_CICP_CP_BT_470_B_G,
    EB_CICP_CP_BT_601,
    EB_CICP_CP_SMPTE_240,
    EB_CICP_CP_GENERIC_FILM,
    EB_CICP_CP_BT_2020,
    EB_CICP_CP_XYZ,
    EB_CICP_CP_SMPTE_431,
    EB_CICP_CP_SMPTE_432,
    EB_CICP_CP_EBU_3213,
};
static const vector<EbColorPrimaries> invalid_color_primaries = {/*none*/};

/* Transfer characteristics
 */
static const vector<EbTransferCharacteristics>
    default_transfer_characteristics = {
        EB_CICP_TC_UNSPECIFIED,
};
static const vector<EbTransferCharacteristics> valid_transfer_characteristics =
    {
        EB_CICP_TC_BT_709,
        EB_CICP_TC_UNSPECIFIED,
        EB_CICP_TC_BT_470_M,
        EB_CICP_TC_BT_470_B_G,
        EB_CICP_TC_BT_601,
        EB_CICP_TC_SMPTE_240,
        EB_CICP_TC_LINEAR,
        EB_CICP_TC_LOG_100,
        EB_CICP_TC_LOG_100_SQRT10,
        EB_CICP_TC_IEC_61966,
        EB_CICP_TC_BT_1361,
        EB_CICP_TC_SRGB,
        EB_CICP_TC_BT_2020_10_BIT,
        EB_CICP_TC_BT_2020_12_BIT,
        EB_CICP_TC_SMPTE_2084,
        EB_CICP_TC_SMPTE_428,
        EB_CICP_TC_HLG,
};
static const vector<EbTransferCharacteristics>
    invalid_transfer_characteristics = {/*none*/};

/* Matrix coeffricients
 */
static const vector<EbMatrixCoefficients> default_matrix_coefficients = {
    EB_CICP_MC_UNSPECIFIED,
};
static const vector<EbMatrixCoefficients> valid_matrix_coefficients = {
    EB_CICP_MC_BT_709,
    EB_CICP_MC_UNSPECIFIED,
    EB_CICP_MC_FCC,
    EB_CICP_MC_BT_470_B_G,
    EB_CICP_MC_BT_601,
    EB_CICP_MC_SMPTE_240,
    EB_CICP_MC_SMPTE_YCGCO,
    EB_CICP_MC_BT_2020_NCL,
    EB_CICP_MC_BT_2020_CL,
    EB_CICP_MC_SMPTE_2085,
    EB_CICP_MC_CHROMAT_NCL,
    EB_CICP_MC_CHROMAT_CL,
    EB_CICP_MC_ICTCP,
};
static const vector<EbMatrixCoefficients> invalid_matrix_coefficients = {
    EB_CICP_MC_IDENTITY,  // not actually invalid, but requires 4:4:4
};

}  // namespace svt_av1_test_params

/** @} */  // end of svt_av1_test_params

#endif  // _TEST_PARAMS_H_
