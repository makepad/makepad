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

#include <limits.h>
#include <math.h>
#include <stdio.h>

#include "aom_dsp_rtcd.h"
#include "definitions.h"
#include "rc_process.h"
#include "sequence_control_set.h"
#include "pcs.h"
#include "firstpass.h"
#include "svt_log.h"
#include "md_process.h"
#include "coding_loop.h"
#include "pd_process.h"
#include "md_config_process.h"
#include "mv.h"
#ifdef ARCH_X86_64
#include <xmmintrin.h>
#endif
#include "motion_estimation.h"
#include "pd_results.h"

#undef _MM_HINT_T2
#define _MM_HINT_T2 1

#define OUTPUT_FPF 0

#define INTRA_MODE_PENALTY 1024
#define NEW_MV_MODE_PENALTY 32
#define DARK_THRESH 64

#define NCOUNT_INTRA_THRESH 8192
#define NCOUNT_INTRA_FACTOR 3

#define STATS_CAPABILITY_INIT 100
//1.5 times larger than request.
#define STATS_CAPABILITY_GROW(s) (s * 3 / 2)

static EbErrorType realloc_stats_out(SequenceControlSet* scs, FirstPassStatsOut* out, uint64_t frame_number) {
    if (frame_number < out->size) {
        return EB_ErrorNone;
    }

    if ((int64_t)frame_number >= (int64_t)out->capability - 1) {
        size_t capability = (int64_t)frame_number >= (int64_t)STATS_CAPABILITY_INIT - 1
            ? STATS_CAPABILITY_GROW(frame_number)
            : STATS_CAPABILITY_INIT;
        if (scs->lap_rc) {
            //store the data points before re-allocation
            uint64_t stats_in_start_offset = 0;
            uint64_t stats_in_offset       = 0;
            uint64_t stats_in_end_offset   = 0;
            if (frame_number) {
                stats_in_start_offset = scs->twopass.stats_buf_ctx->stats_in_start - out->stat;
                stats_in_offset       = scs->twopass.stats_in - out->stat;
                stats_in_end_offset   = scs->twopass.stats_buf_ctx->stats_in_end_write - out->stat;
            }
            EB_REALLOC_ARRAY(out->stat, capability);
            // restore the pointers after re-allocation is done
            scs->twopass.stats_buf_ctx->stats_in_start     = out->stat + stats_in_start_offset;
            scs->twopass.stats_in                          = out->stat + stats_in_offset;
            scs->twopass.stats_buf_ctx->stats_in_end_write = out->stat + stats_in_end_offset;
        } else {
            EB_REALLOC_ARRAY(out->stat, capability);
        }
        out->capability = capability;
    }
    out->size = frame_number + 1;
    return EB_ErrorNone;
}

static AOM_INLINE void output_stats(SequenceControlSet* scs, const FIRSTPASS_STATS* stats, uint64_t frame_number) {
    FirstPassStatsOut* stats_out = &scs->enc_ctx->stats_out;
    svt_block_on_mutex(scs->enc_ctx->stat_file_mutex);
    if (realloc_stats_out(scs, stats_out, frame_number) != EB_ErrorNone) {
        SVT_ERROR("realloc_stats_out request %d entries failed failed\n", frame_number);
    } else {
        stats_out->stat[frame_number] = *stats;
    }

    // TEMP debug code
#if OUTPUT_FPF
    {
        FILE* fpfile;
        if (frame_number == 0) {
            fpfile = fopen("firstpass.stt", "w");
        } else {
            fpfile = fopen("firstpass.stt", "a");
        }
        fprintf(fpfile,
                "%12.0lf %12.4lf %12.0lf %12.0lf %12.0lf %12.4lf %12.4lf"
                "%12.4lf %12.4lf %12.4lf %12.4lf %12.4lf %12.4lf %12.4lf %12.4lf"
                "%12.4lf %12.4lf %12.0lf %12.4lf %12.4lf %12.4lf %12.4lf\n",
                stats->frame,
                stats->weight,
                stats->intra_error,
                stats->coded_error,
                stats->sr_coded_error,
                stats->pcnt_inter,
                stats->pcnt_motion,
                stats->pcnt_second_ref,
                stats->pcnt_neutral,
                stats->intra_skip_pct,
                stats->inactive_zone_rows,
                stats->inactive_zone_cols,
                stats->MVr,
                stats->mvr_abs,
                stats->MVc,
                stats->mvc_abs,
                stats->MVrv,
                stats->MVcv,
                stats->mv_in_out_count,
                stats->new_mv_count,
                stats->count,
                stats->duration);
        fclose(fpfile);
    }
#endif
    svt_release_mutex(scs->enc_ctx->stat_file_mutex);
}

void svt_av1_twopass_zero_stats(FIRSTPASS_STATS* section) {
    section->frame       = 0.0;
    section->coded_error = 0.0;
    section->count       = 0.0;
    section->duration    = 1.0;
    memset(&section->stat_struct, 0, sizeof(StatStruct));
}

void svt_av1_accumulate_stats(FIRSTPASS_STATS* section, const FIRSTPASS_STATS* frame) {
    section->frame += frame->frame;
    section->coded_error += frame->coded_error;
    section->count += frame->count;
    section->duration += frame->duration;
}

void svt_av1_end_first_pass(PictureParentControlSet* pcs) {
    SequenceControlSet* scs     = pcs->scs;
    TWO_PASS*           twopass = &scs->twopass;

    if (twopass->stats_buf_ctx->total_stats) {
        // add the total to the end of the file
        svt_block_on_mutex(twopass->stats_buf_ctx->stats_in_write_mutex);

        FIRSTPASS_STATS total_stats = *twopass->stats_buf_ctx->total_stats;
        output_stats(scs, &total_stats, pcs->picture_number + 1);

        svt_release_mutex(twopass->stats_buf_ctx->stats_in_write_mutex);
    }
}

// Updates the first pass stats of this frame.
// Input:
//   stats: stats accumulated for this frame.
//   frame_number: current frame number.
//   ts_duration: Duration of the frame / collection of frames.
// Updates:
//   twopass->total_stats: the accumulated stats.
//   twopass->stats_buf_ctx->stats_in_end: the pointer to the current stats,
//                                         update its value and its position
//                                         in the buffer.
void update_firstpass_stats(PictureParentControlSet* pcs, const int frame_number, const double ts_duration,
                            StatStruct* stat_struct) {
    SequenceControlSet* scs     = pcs->scs;
    TWO_PASS*           twopass = &scs->twopass;

    svt_block_on_mutex(twopass->stats_buf_ctx->stats_in_write_mutex);

    FIRSTPASS_STATS* this_frame_stats = twopass->stats_buf_ctx->stats_in_end_write;
    FIRSTPASS_STATS  fps;
    fps.frame       = frame_number;
    fps.coded_error = 0;
    fps.count       = 1.0;
    // TODO(paulwilkins):  Handle the case when duration is set to 0, or
    // something less than the full time between subsequent values of
    // cpi->source_time_stamp.
    fps.duration = (double)ts_duration;
    memset(&fps.stat_struct, 0, sizeof(StatStruct));
    fps.stat_struct.poc                  = stat_struct->poc;
    fps.stat_struct.qindex               = stat_struct->qindex;
    fps.stat_struct.total_num_bits       = stat_struct->total_num_bits;
    fps.stat_struct.temporal_layer_index = stat_struct->temporal_layer_index;
    fps.stat_struct.worst_qindex         = stat_struct->worst_qindex;
    // We will store the stats inside the persistent twopass struct (and NOT the
    // local variable 'fps'), and then cpi->output_pkt_list will point to it.
    *this_frame_stats = fps;
    output_stats(scs, &fps, pcs->picture_number);
    if (twopass->stats_buf_ctx->total_stats != NULL && scs->static_config.pass == ENC_FIRST_PASS) {
        svt_av1_accumulate_stats(twopass->stats_buf_ctx->total_stats, &fps);
    }
    /*In the case of two pass, first pass uses it as a circular buffer,
   * when LAP is enabled it is used as a linear buffer*/
    twopass->stats_buf_ctx->stats_in_end_write++;
    if (scs->static_config.pass == ENC_FIRST_PASS &&
        (twopass->stats_buf_ctx->stats_in_end_write >= twopass->stats_buf_ctx->stats_in_buf_end)) {
        twopass->stats_buf_ctx->stats_in_end_write = twopass->stats_buf_ctx->stats_in_start;
    }
    svt_release_mutex(twopass->stats_buf_ctx->stats_in_write_mutex);
}

void first_pass_frame_end_one_pass(PictureParentControlSet* pcs) {
    SequenceControlSet* scs     = pcs->scs;
    TWO_PASS*           twopass = &scs->twopass;

    svt_block_on_mutex(twopass->stats_buf_ctx->stats_in_write_mutex);
    FIRSTPASS_STATS* this_frame_stats = twopass->stats_buf_ctx->stats_in_end_write;
    FIRSTPASS_STATS  fps;
    memset(&fps, 0, sizeof(FIRSTPASS_STATS));
    memset(&fps.stat_struct, 0, sizeof(StatStruct));
    fps.frame    = (double)pcs->picture_number;
    fps.count    = 1.0;
    fps.duration = (double)pcs->ts_duration;

    // We will store the stats inside the persistent twopass struct (and NOT the
    // local variable 'fps'), and then cpi->output_pkt_list will point to it.
    *this_frame_stats = fps;
    output_stats(scs, &fps, pcs->picture_number);
    /*In the case of two pass, first pass uses it as a circular buffer,
   * when LAP is enabled it is used as a linear buffer*/
    twopass->stats_buf_ctx->stats_in_end_write++;
    svt_release_mutex(twopass->stats_buf_ctx->stats_in_write_mutex);
}
