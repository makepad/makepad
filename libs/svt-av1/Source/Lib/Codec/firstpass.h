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

#ifndef AOM_AV1_ENCODER_FIRSTPASS_H_
#define AOM_AV1_ENCODER_FIRSTPASS_H_

#include "definitions.h"
#include "rc_process.h"
#include "pcs.h"

#ifdef __cplusplus
extern "C" {
#endif

#define DOUBLE_DIVIDE_CHECK(x) ((x) < 0 ? (x) - 0.000001 : (x) + 0.000001)

#define MAX_LAG_BUFFERS 35

/*!
 * \brief The structure of accumulated frame stats in the first pass.
 */
typedef struct {
    /*!
   * Frame number in display order, if stats are for a single frame.
   * No real meaning for a collection of frames.
   */
    double frame;
    /*!
   * Best of intra pred error and inter pred error using last frame as ref.
   */
    double coded_error;
    /*!
   * Duration of the frame / collection of frames.
   */
    double duration;
    /*!
   * 1.0 if stats are for a single frame, OR
   * Number of frames in this collection for which the stats are accumulated.
   */
    double     count;
    StatStruct stat_struct;
} FIRSTPASS_STATS;

/*!\cond */

#define FC_ANIMATION_THRESH 0.15

enum { FC_NORMAL = 0, FC_GRAPHICS_ANIMATION = 1, FRAME_CONTENT_TYPES = 2 } UENUM1BYTE(FRAME_CONTENT_TYPE);

typedef struct {
    FIRSTPASS_STATS* stats_in_start;
    // used when writing the stat.i.e in the first pass
    FIRSTPASS_STATS* stats_in_end_write;
    FIRSTPASS_STATS* stats_in_end;
    FIRSTPASS_STATS* stats_in_buf_end;
    FIRSTPASS_STATS* total_stats;
    FIRSTPASS_STATS* total_left_stats;
    int64_t          last_frame_accumulated;
    EbHandle         stats_in_write_mutex; // mutex for write point protection
} STATS_BUFFER_CTX;

/*!\endcond */

/*!
 * \brief Two pass status and control data.
 */
typedef struct {
    // Circular queue of first pass stats stored for most recent frames.
    // cpi->output_pkt_list[i].data.twopass_stats.buf points to actual data stored
    // here.
    const FIRSTPASS_STATS* stats_in;
    STATS_BUFFER_CTX*      stats_buf_ctx;
    int                    first_pass_done;
    int64_t                bits_left;
    double                 modified_error_min;
    double                 modified_error_max;
    double                 modified_error_left;

    // Projected total bits available for a key frame group of frames
    int64_t kf_group_bits;

    // Error score of frames still to be coded in kf group
    int64_t kf_group_error_left;

    int     kf_zeromotion_pct;
    int     extend_minq;
    int     extend_maxq;
    int     extend_minq_fast;
    uint8_t passes;
    /*!\endcond */
} TWO_PASS;

/*!\cond */

void svt_av1_twopass_zero_stats(FIRSTPASS_STATS* section);
void svt_av1_accumulate_stats(FIRSTPASS_STATS* section, const FIRSTPASS_STATS* frame);
/*!\endcond */

#ifdef __cplusplus
} // extern "C"
#endif

#endif // AOM_AV1_ENCODER_FIRSTPASS_H_
