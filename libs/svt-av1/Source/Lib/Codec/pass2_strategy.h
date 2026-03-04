/*
 * Copyright (c) 2019, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at www.aomedia.org/license/software. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at www.aomedia.org/license/patent.
 */

#ifndef AOM_AV1_ENCODER_PASS2_STRATEGY_H_
#define AOM_AV1_ENCODER_PASS2_STRATEGY_H_

#include "encoder.h"
#include "av1_common.h"

#ifdef __cplusplus
extern "C" {
#endif

// structure of accumulated stats and features in a gf group
typedef struct {
    double     gf_group_err;
    StatStruct gf_stat_struct;
    double     gf_group_raw_error;
    double     gf_group_skip_pct;
    double     gf_group_inactive_zone_rows;
} GF_GROUP_STATS;

void svt_av1_init_second_pass(struct SequenceControlSet* scs);
void svt_av1_init_single_pass_lap(struct SequenceControlSet* scs);
void svt_av1_new_framerate(struct SequenceControlSet* scs, double framerate);
void svt_aom_process_rc_stat(struct PictureParentControlSet* ppcs);
void svt_av1_twopass_postencode_update(struct PictureParentControlSet* ppcs);
void svt_av1_twopass_postencode_update_gop_const(struct PictureParentControlSet* ppcs);
void svt_aom_set_rc_param(struct SequenceControlSet* scs);

#ifdef __cplusplus
} // extern "C"
#endif

#endif // AOM_AV1_ENCODER_PASS2_STRATEGY_H_
