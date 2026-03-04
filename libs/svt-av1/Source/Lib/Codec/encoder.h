/*
 * Copyright (c) 2016, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at www.aomedia.org/license/software. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at www.aomedia.org/license/patent.
 */

/*!\file
 * \brief Declares top-level encoder structures and functions.
 */
#ifndef AOM_AV1_ENCODER_ENCODER_H_
#define AOM_AV1_ENCODER_ENCODER_H_

#include <stdbool.h>
#include <stdio.h>

#include "definitions.h"
#include "av1_common.h"
#include "rc_process.h"

#ifdef __cplusplus
extern "C" {
#endif

//**********************************************************************************************************************//
// aom_codec.h
/*!\brief Rate control mode */
enum aom_rc_mode {
    AOM_VBR, /**< Variable Bit Rate (VBR) mode */
    AOM_CBR, /**< Constant Bit Rate (CBR) mode */
    AOM_Q, /**< Constant Quality (Q) mode */
};

//**********************************************************************************************************************//

/*!\endcond */
/*!
 * \brief Encoder rate control configuration parameters
 */
typedef struct {
    /*!\cond */
    // BUFFERING PARAMETERS
    // Indicates the amount of data that will be buffered by the decoding
    // application prior to beginning playback, and is expressed in units of
    // time(milliseconds).
    int64_t starting_buffer_level_ms;
    // Indicates the amount of data that the encoder should try to maintain in the
    // decoder's buffer, and is expressed in units of time(milliseconds).
    int64_t optimal_buffer_level_ms;
    // Indicates the maximum amount of data that may be buffered by the decoding
    // application, and is expressed in units of time(milliseconds).
    int64_t maximum_buffer_size_ms;

    // Indicates the maximum allowed bitrate for any intra frame as % of bitrate
    // target.
    unsigned int max_intra_bitrate_pct;
    // Indicates the maximum allowed bitrate for any inter frame as % of bitrate
    // target.
    unsigned int max_inter_bitrate_pct;
    // min_cr / 100 indicates the target minimum compression ratio for each frame.
    unsigned int min_cr;
    // under_shoot_pct indicates the tolerance of the VBR algorithm to undershoot
    // and is used as a trigger threshold for more agressive adaptation of Q. It's
    // value can range from 0-100.
    int under_shoot_pct;
    // over_shoot_pct indicates the tolerance of the VBR algorithm to overshoot
    // and is used as a trigger threshold for more agressive adaptation of Q. It's
    // value can range from 0-1000.
    int over_shoot_pct;
    // Indicates the maximum qindex that can be used by the quantizer i.e. the
    // worst quality qindex.
    int worst_allowed_q;
    // Indicates the minimum qindex that can be used by the quantizer i.e. the
    // best quality qindex.
    int best_allowed_q;
    // Indicates if the encoding mode is vbr, cbr, constrained quality or constant
    // quality.
    enum aom_rc_mode mode;
    /*!\endcond */
} RateControlCfg;

typedef struct {
    int        frame_width;
    int        frame_height;
    int        mb_rows;
    int        mb_cols;
    int        num_mbs;
    EbBitDepth bit_depth;
    int        subsampling_x;
    int        subsampling_y;
} FrameInfo;

typedef struct {
    // stats_in buffer contains all of the stats packets produced in the first
    // pass, concatenated.
    //aom_fixed_buf_t stats_in;

    // Indicates the minimum bitrate to be used for a single GOP as a percentage
    // of the target bitrate.
    int vbrmin_section;
    // Indicates the maximum bitrate to be used for a single GOP as a percentage
    // of the target bitrate.
    int vbrmax_section;
} TwoPassCfg;

typedef struct SwitchFrameCfg {
    // Indicates the number of frames after which a frame may be coded as an S-Frame.
    int32_t sframe_dist;
    // 1: the considered frame will be made into an S-Frame only if it is an altref frame.
    // 2: the next altref frame will be made into an S-Frame.
    EbSFrameMode sframe_mode;
} SwitchFrameCfg;

// Function return size of frame stats buffer
static INLINE int get_stats_buf_size(int num_lap_buffer, int num_lag_buffer) {
    /* if lookahead is enabled return num_lap_buffers else num_lag_buffers */
    return (num_lap_buffer > 0 ? num_lap_buffer + 1 : num_lag_buffer);
}

#ifdef __cplusplus
} // extern "C"
#endif

#endif // AOM_AV1_ENCODER_ENCODER_H_
