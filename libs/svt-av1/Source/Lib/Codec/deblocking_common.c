/*
* Copyright(c) 2019 Intel Corporation
* Copyright (c) 2016, Alliance for Open Media. All rights reserved
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#include "deblocking_common.h"
#include "common_utils.h"

static const int delta_lf_id_lut[MAX_MB_PLANE][2] = {{0, 1}, {2, 2}, {3, 3}};

static const SEG_LVL_FEATURES seg_lvl_lf_lut[MAX_MB_PLANE][2] = {{SEG_LVL_ALT_LF_Y_V, SEG_LVL_ALT_LF_Y_H},
                                                                 {SEG_LVL_ALT_LF_U, SEG_LVL_ALT_LF_U},
                                                                 {SEG_LVL_ALT_LF_V, SEG_LVL_ALT_LF_V}};

static INLINE int svt_aom_seg_feature_active(SegmentationParams* seg, int segment_id, SEG_LVL_FEATURES feature_id) {
    return seg->segmentation_enabled && seg->feature_enabled[segment_id][feature_id];
}

static INLINE int get_segdata(SegmentationParams* seg, int segment_id, SEG_LVL_FEATURES feature_id) {
    return seg->feature_data[segment_id][feature_id];
}

static INLINE int8_t signed_char_clamp(int32_t t) {
    return (int8_t)clamp(t, -128, 127);
}

static INLINE int16_t signed_char_clamp_high(int32_t t, int32_t bd) {
    switch (bd) {
    case 10:
        return (int16_t)clamp(t, -128 * 4, 128 * 4 - 1);
    case 12:
        return (int16_t)clamp(t, -128 * 16, 128 * 16 - 1);
    case 8:
    default:
        return (int16_t)clamp(t, -128, 128 - 1);
    }
}

uint8_t svt_aom_get_filter_level_delta_lf(FrameHeader* frm_hdr, const int32_t dir_idx, int32_t plane,
                                          int32_t* sb_delta_lf, uint8_t seg_id, PredictionMode pred_mode,
                                          MvReferenceFrame ref_frame_0) {
    int32_t delta_lf = -1;
    if (frm_hdr->delta_lf_params.delta_lf_multi) {
        const int32_t delta_lf_idx = delta_lf_id_lut[plane][dir_idx];
        delta_lf                   = sb_delta_lf[delta_lf_idx];
    } else {
        delta_lf = sb_delta_lf[0];
    }
    int32_t base_level;
    if (plane == 0) {
        base_level = frm_hdr->loop_filter_params.filter_level[dir_idx];
    } else if (plane == 1) {
        base_level = frm_hdr->loop_filter_params.filter_level_u;
    } else {
        base_level = frm_hdr->loop_filter_params.filter_level_v;
    }
    int32_t lvl_seg = clamp(delta_lf + base_level, 0, MAX_LOOP_FILTER);
    assert(plane >= 0 && plane <= 2);
    const int32_t seg_lf_feature_id = seg_lvl_lf_lut[plane][dir_idx];
    if (svt_aom_seg_feature_active(&frm_hdr->segmentation_params, seg_id, seg_lf_feature_id)) {
        const int32_t data = get_segdata(&frm_hdr->segmentation_params, seg_id, seg_lf_feature_id);
        lvl_seg            = clamp(lvl_seg + data, 0, MAX_LOOP_FILTER);
    }

    if (frm_hdr->loop_filter_params.mode_ref_delta_enabled) {
        const int32_t scale = 1 << (lvl_seg >> 5);
        lvl_seg += frm_hdr->loop_filter_params.ref_deltas[ref_frame_0] * scale;
        if (ref_frame_0 > INTRA_FRAME) {
            lvl_seg += frm_hdr->loop_filter_params.mode_deltas[mode_lf_lut[pred_mode]] * scale;
        }
        lvl_seg = clamp(lvl_seg, 0, MAX_LOOP_FILTER);
    }
    return lvl_seg;
}

// Update the loop filter for the current frame.
// This should be called before loop_filter_rows(),
// svt_av1_loop_filter_frame() calls this function directly.
void svt_av1_loop_filter_frame_init(FrameHeader* frm_hdr, LoopFilterInfoN* lfi, int32_t plane_start,
                                    int32_t plane_end) {
    int32_t filt_lvl[MAX_MB_PLANE], filt_lvl_r[MAX_MB_PLANE];
    int32_t plane;
    int32_t seg_id;
    // n_shift is the multiplier for lf_deltas
    // the multiplier is 1 for when filter_lvl is between 0 and 31;
    // 2 when filter_lvl is between 32 and 63

    LoopFilter* const lf = &frm_hdr->loop_filter_params;
    // const struct segmentation *const seg = &pcs->ppcs->seg;

    // update sharpness limits
    svt_aom_update_sharpness(lfi, lf->sharpness_level);

    filt_lvl[0] = frm_hdr->loop_filter_params.filter_level[0];
    filt_lvl[1] = frm_hdr->loop_filter_params.filter_level_u;
    filt_lvl[2] = frm_hdr->loop_filter_params.filter_level_v;

    filt_lvl_r[0] = frm_hdr->loop_filter_params.filter_level[1];
    filt_lvl_r[1] = frm_hdr->loop_filter_params.filter_level_u;
    filt_lvl_r[2] = frm_hdr->loop_filter_params.filter_level_v;

    for (plane = plane_start; plane < plane_end; plane++) {
        if (plane == 0 && !filt_lvl[0] && !filt_lvl_r[0]) {
            break;
        } else if (plane == 1 && !filt_lvl[1]) {
            continue;
        } else if (plane == 2 && !filt_lvl[2]) {
            continue;
        }

        for (seg_id = 0; seg_id < MAX_SEGMENTS; seg_id++) {
            for (int32_t dir = 0; dir < 2; ++dir) {
                int32_t lvl_seg = (dir == 0) ? filt_lvl[plane] : filt_lvl_r[plane];
                assert(plane >= 0 && plane <= 2);
                const int32_t seg_lf_feature_id = seg_lvl_lf_lut[plane][dir];
                if (svt_aom_seg_feature_active(&frm_hdr->segmentation_params, seg_id, seg_lf_feature_id)) {
                    const int32_t data = get_segdata(&frm_hdr->segmentation_params, seg_id, seg_lf_feature_id);
                    lvl_seg            = clamp(lvl_seg + data, 0, MAX_LOOP_FILTER);
                }

                if (!lf->mode_ref_delta_enabled) {
                    // we could get rid of this if we assume that deltas are set to
                    // zero when not in use; encoder always uses deltas
                    memset(lfi->lvl[plane][seg_id][dir], lvl_seg, sizeof(lfi->lvl[plane][seg_id][dir]));
                } else {
                    int32_t       ref, mode;
                    const int32_t scale                          = 1 << (lvl_seg >> 5);
                    const int32_t intra_lvl                      = lvl_seg + lf->ref_deltas[INTRA_FRAME] * scale;
                    lfi->lvl[plane][seg_id][dir][INTRA_FRAME][0] = (uint8_t)clamp(intra_lvl, 0, MAX_LOOP_FILTER);

                    for (ref = LAST_FRAME; ref < REF_FRAMES; ++ref) {
                        for (mode = 0; mode < MAX_MODE_LF_DELTAS; ++mode) {
                            const int32_t inter_lvl = lvl_seg + lf->ref_deltas[ref] * scale +
                                lf->mode_deltas[mode] * scale;
                            lfi->lvl[plane][seg_id][dir][ref][mode] = (uint8_t)clamp(inter_lvl, 0, MAX_LOOP_FILTER);
                        }
                    }
                }
            }
        }
    }
}

// should we apply any filter at all: 11111111 yes, 00000000 no
static INLINE int8_t filter_mask2(uint8_t limit, uint8_t blimit, uint8_t p1, uint8_t p0, uint8_t q0, uint8_t q1) {
    int8_t mask = 0;
    mask |= (abs(p1 - p0) > limit) * -1;
    mask |= (abs(q1 - q0) > limit) * -1;
    mask |= (abs(p0 - q0) * 2 + abs(p1 - q1) / 2 > blimit) * -1;
    return ~mask;
}

static INLINE int8_t filter_mask(uint8_t limit, uint8_t blimit, uint8_t p3, uint8_t p2, uint8_t p1, uint8_t p0,
                                 uint8_t q0, uint8_t q1, uint8_t q2, uint8_t q3) {
    int8_t mask = 0;
    mask |= (abs(p3 - p2) > limit) * -1;
    mask |= (abs(p2 - p1) > limit) * -1;
    mask |= (abs(p1 - p0) > limit) * -1;
    mask |= (abs(q1 - q0) > limit) * -1;
    mask |= (abs(q2 - q1) > limit) * -1;
    mask |= (abs(q3 - q2) > limit) * -1;
    mask |= (abs(p0 - q0) * 2 + abs(p1 - q1) / 2 > blimit) * -1;
    return ~mask;
}

static INLINE int8_t filter_mask3_chroma(uint8_t limit, uint8_t blimit, uint8_t p2, uint8_t p1, uint8_t p0, uint8_t q0,
                                         uint8_t q1, uint8_t q2) {
    int8_t mask = 0;
    mask |= (abs(p2 - p1) > limit) * -1;
    mask |= (abs(p1 - p0) > limit) * -1;
    mask |= (abs(q1 - q0) > limit) * -1;
    mask |= (abs(q2 - q1) > limit) * -1;
    mask |= (abs(p0 - q0) * 2 + abs(p1 - q1) / 2 > blimit) * -1;
    return ~mask;
}

static INLINE int8_t flat_mask3_chroma(uint8_t thresh, uint8_t p2, uint8_t p1, uint8_t p0, uint8_t q0, uint8_t q1,
                                       uint8_t q2) {
    int8_t mask = 0;
    mask |= (abs(p1 - p0) > thresh) * -1;
    mask |= (abs(q1 - q0) > thresh) * -1;
    mask |= (abs(p2 - p0) > thresh) * -1;
    mask |= (abs(q2 - q0) > thresh) * -1;
    return ~mask;
}

static INLINE int8_t highbd_flat_mask3_chroma(uint8_t thresh, uint16_t p2, uint16_t p1, uint16_t p0, uint16_t q0,
                                              uint16_t q1, uint16_t q2, int bd) {
    int8_t  mask     = 0;
    int16_t thresh16 = (uint16_t)thresh << (bd - 8);
    mask |= (abs(p1 - p0) > thresh16) * -1;
    mask |= (abs(q1 - q0) > thresh16) * -1;
    mask |= (abs(p2 - p0) > thresh16) * -1;
    mask |= (abs(q2 - q0) > thresh16) * -1;
    return ~mask;
}

static INLINE int8_t flat_mask4(uint8_t thresh, uint8_t p3, uint8_t p2, uint8_t p1, uint8_t p0, uint8_t q0, uint8_t q1,
                                uint8_t q2, uint8_t q3) {
    int8_t mask = 0;
    mask |= (abs(p1 - p0) > thresh) * -1;
    mask |= (abs(q1 - q0) > thresh) * -1;
    mask |= (abs(p2 - p0) > thresh) * -1;
    mask |= (abs(q2 - q0) > thresh) * -1;
    mask |= (abs(p3 - p0) > thresh) * -1;
    mask |= (abs(q3 - q0) > thresh) * -1;
    return ~mask;
}

// is there high edge variance internal edge: 11111111 yes, 00000000 no
static INLINE int8_t hev_mask(uint8_t thresh, uint8_t p1, uint8_t p0, uint8_t q0, uint8_t q1) {
    int8_t hev = 0;
    hev |= (abs(p1 - p0) > thresh) * -1;
    hev |= (abs(q1 - q0) > thresh) * -1;
    return hev;
}

static INLINE void filter4(int8_t mask, uint8_t thresh, uint8_t* op1, uint8_t* op0, uint8_t* oq0, uint8_t* oq1) {
    int8_t       filter1, filter2;
    const int8_t ps1 = (int8_t)(*op1 ^ 0x80);
    const int8_t ps0 = (int8_t)(*op0 ^ 0x80);
    const int8_t qs0 = (int8_t)(*oq0 ^ 0x80);
    const int8_t qs1 = (int8_t)(*oq1 ^ 0x80);
    const int8_t hev = hev_mask(thresh, *op1, *op0, *oq0, *oq1);

    // add outer taps if we have high edge variance
    int8_t filter = signed_char_clamp(ps1 - qs1) & hev;

    // inner taps
    filter = signed_char_clamp(filter + 3 * (qs0 - ps0)) & mask;

    // save bottom 3 bits so that we round one side +4 and the other +3
    // if it equals 4 we'll set to adjust by -1 to account for the fact
    // we'd round 3 the other way
    filter1 = signed_char_clamp(filter + 4) >> 3;
    filter2 = signed_char_clamp(filter + 3) >> 3;
    *oq0    = (uint8_t)(signed_char_clamp(qs0 - filter1) ^ 0x80);
    *op0    = (uint8_t)(signed_char_clamp(ps0 + filter2) ^ 0x80);

    // outer tap adjustments
    filter = ROUND_POWER_OF_TWO(filter1, 1) & ~hev;
    *oq1   = (uint8_t)(signed_char_clamp(qs1 - filter) ^ 0x80);
    *op1   = (uint8_t)(signed_char_clamp(ps1 + filter) ^ 0x80);
}

void svt_aom_lpf_horizontal_4_c(uint8_t* s, int32_t p /* pitch */, const uint8_t* blimit, const uint8_t* limit,
                                const uint8_t* thresh) {
    int32_t i;
    int32_t count = 4;

    // loop filter designed to work using chars so that we can make maximum use
    // of 8 bit simd instructions.
    for (i = 0; i < count; ++i) {
        const uint8_t p1 = s[-2 * p], p0 = s[-p];
        const uint8_t q0 = s[0 * p], q1 = s[1 * p];
        const int8_t  mask = filter_mask2(*limit, *blimit, p1, p0, q0, q1);
        filter4(mask, *thresh, s - 2 * p, s - 1 * p, s, s + 1 * p);
        ++s;
    }
}

void svt_aom_lpf_vertical_4_c(uint8_t* s, int32_t pitch, const uint8_t* blimit, const uint8_t* limit,
                              const uint8_t* thresh) {
    int32_t i;
    int32_t count = 4;

    // loop filter designed to work using chars so that we can make maximum use
    // of 8 bit simd instructions.
    for (i = 0; i < count; ++i) {
        const uint8_t p1 = s[-2], p0 = s[-1];
        const uint8_t q0 = s[0], q1 = s[1];
        const int8_t  mask = filter_mask2(*limit, *blimit, p1, p0, q0, q1);
        filter4(mask, *thresh, s - 2, s - 1, s, s + 1);
        s += pitch;
    }
}

static INLINE void filter6(int8_t mask, uint8_t thresh, int8_t flat, uint8_t* op2, uint8_t* op1, uint8_t* op0,
                           uint8_t* oq0, uint8_t* oq1, uint8_t* oq2) {
    if (flat && mask) {
        const uint8_t p2 = *op2, p1 = *op1, p0 = *op0;
        const uint8_t q0 = *oq0, q1 = *oq1, q2 = *oq2;

        // 5-tap filter [1, 2, 2, 2, 1]
        *op1 = ROUND_POWER_OF_TWO(p2 * 3 + p1 * 2 + p0 * 2 + q0, 3);
        *op0 = ROUND_POWER_OF_TWO(p2 + p1 * 2 + p0 * 2 + q0 * 2 + q1, 3);
        *oq0 = ROUND_POWER_OF_TWO(p1 + p0 * 2 + q0 * 2 + q1 * 2 + q2, 3);
        *oq1 = ROUND_POWER_OF_TWO(p0 + q0 * 2 + q1 * 2 + q2 * 3, 3);
    } else {
        filter4(mask, thresh, op1, op0, oq0, oq1);
    }
}

static INLINE void filter8(int8_t mask, uint8_t thresh, int8_t flat, uint8_t* op3, uint8_t* op2, uint8_t* op1,
                           uint8_t* op0, uint8_t* oq0, uint8_t* oq1, uint8_t* oq2, uint8_t* oq3) {
    if (flat && mask) {
        const uint8_t p3 = *op3, p2 = *op2, p1 = *op1, p0 = *op0;
        const uint8_t q0 = *oq0, q1 = *oq1, q2 = *oq2, q3 = *oq3;

        // 7-tap filter [1, 1, 1, 2, 1, 1, 1]
        *op2 = ROUND_POWER_OF_TWO(p3 + p3 + p3 + 2 * p2 + p1 + p0 + q0, 3);
        *op1 = ROUND_POWER_OF_TWO(p3 + p3 + p2 + 2 * p1 + p0 + q0 + q1, 3);
        *op0 = ROUND_POWER_OF_TWO(p3 + p2 + p1 + 2 * p0 + q0 + q1 + q2, 3);
        *oq0 = ROUND_POWER_OF_TWO(p2 + p1 + p0 + 2 * q0 + q1 + q2 + q3, 3);
        *oq1 = ROUND_POWER_OF_TWO(p1 + p0 + q0 + 2 * q1 + q2 + q3 + q3, 3);
        *oq2 = ROUND_POWER_OF_TWO(p0 + q0 + q1 + 2 * q2 + q3 + q3 + q3, 3);
    } else {
        filter4(mask, thresh, op1, op0, oq0, oq1);
    }
}

void svt_aom_lpf_horizontal_6_c(uint8_t* s, int32_t p, const uint8_t* blimit, const uint8_t* limit,
                                const uint8_t* thresh) {
    int32_t i;
    int32_t count = 4;

    // loop filter designed to work using chars so that we can make maximum use
    // of 8 bit simd instructions.
    for (i = 0; i < count; ++i) {
        const uint8_t p2 = s[-3 * p], p1 = s[-2 * p], p0 = s[-p];
        const uint8_t q0 = s[0 * p], q1 = s[1 * p], q2 = s[2 * p];

        const int8_t mask = filter_mask3_chroma(*limit, *blimit, p2, p1, p0, q0, q1, q2);
        const int8_t flat = flat_mask3_chroma(1, p2, p1, p0, q0, q1, q2);
        filter6(mask, *thresh, flat, s - 3 * p, s - 2 * p, s - 1 * p, s, s + 1 * p, s + 2 * p);
        ++s;
    }
}

void svt_aom_lpf_vertical_6_c(uint8_t* s, int32_t pitch, const uint8_t* blimit, const uint8_t* limit,
                              const uint8_t* thresh) {
    int32_t i;
    int32_t count = 4;

    // loop filter designed to work using chars so that we can make maximum use
    // of 8 bit simd instructions.
    for (i = 0; i < count; ++i) {
        const uint8_t p2 = s[-3], p1 = s[-2], p0 = s[-1];
        const uint8_t q0 = s[0], q1 = s[1], q2 = s[2];

        const int8_t mask = filter_mask3_chroma(*limit, *blimit, p2, p1, p0, q0, q1, q2);
        const int8_t flat = flat_mask3_chroma(1, p2, p1, p0, q0, q1, q2);
        filter6(mask, *thresh, flat, s - 3, s - 2, s - 1, s, s + 1, s + 2);
        s += pitch;
    }
}

void svt_aom_lpf_horizontal_8_c(uint8_t* s, int32_t p, const uint8_t* blimit, const uint8_t* limit,
                                const uint8_t* thresh) {
    int32_t i;
    int32_t count = 4;

    // loop filter designed to work using chars so that we can make maximum use
    // of 8 bit simd instructions.
    for (i = 0; i < count; ++i) {
        const uint8_t p3 = s[-4 * p], p2 = s[-3 * p], p1 = s[-2 * p], p0 = s[-p];
        const uint8_t q0 = s[0 * p], q1 = s[1 * p], q2 = s[2 * p], q3 = s[3 * p];

        const int8_t mask = filter_mask(*limit, *blimit, p3, p2, p1, p0, q0, q1, q2, q3);
        const int8_t flat = flat_mask4(1, p3, p2, p1, p0, q0, q1, q2, q3);
        filter8(mask, *thresh, flat, s - 4 * p, s - 3 * p, s - 2 * p, s - 1 * p, s, s + 1 * p, s + 2 * p, s + 3 * p);
        ++s;
    }
}

void svt_aom_lpf_vertical_8_c(uint8_t* s, int32_t pitch, const uint8_t* blimit, const uint8_t* limit,
                              const uint8_t* thresh) {
    int32_t i;
    int32_t count = 4;

    for (i = 0; i < count; ++i) {
        const uint8_t p3 = s[-4], p2 = s[-3], p1 = s[-2], p0 = s[-1];
        const uint8_t q0 = s[0], q1 = s[1], q2 = s[2], q3 = s[3];
        const int8_t  mask = filter_mask(*limit, *blimit, p3, p2, p1, p0, q0, q1, q2, q3);
        const int8_t  flat = flat_mask4(1, p3, p2, p1, p0, q0, q1, q2, q3);
        filter8(mask, *thresh, flat, s - 4, s - 3, s - 2, s - 1, s, s + 1, s + 2, s + 3);
        s += pitch;
    }
}

// Should we apply any filter at all: 11111111 yes, 00000000 no ?
static INLINE int8_t highbd_filter_mask2(uint8_t limit, uint8_t blimit, uint16_t p1, uint16_t p0, uint16_t q0,
                                         uint16_t q1, int32_t bd) {
    int8_t  mask     = 0;
    int16_t limit16  = (uint16_t)limit << (bd - 8);
    int16_t blimit16 = (uint16_t)blimit << (bd - 8);
    mask |= (abs(p1 - p0) > limit16) * -1;
    mask |= (abs(q1 - q0) > limit16) * -1;
    mask |= (abs(p0 - q0) * 2 + abs(p1 - q1) / 2 > blimit16) * -1;
    return ~mask;
}

// Should we apply any filter at all: 11111111 yes, 00000000 no ?
static INLINE int8_t highbd_filter_mask(uint8_t limit, uint8_t blimit, uint16_t p3, uint16_t p2, uint16_t p1,
                                        uint16_t p0, uint16_t q0, uint16_t q1, uint16_t q2, uint16_t q3, int32_t bd) {
    int8_t  mask     = 0;
    int16_t limit16  = (uint16_t)limit << (bd - 8);
    int16_t blimit16 = (uint16_t)blimit << (bd - 8);
    mask |= (abs(p3 - p2) > limit16) * -1;
    mask |= (abs(p2 - p1) > limit16) * -1;
    mask |= (abs(p1 - p0) > limit16) * -1;
    mask |= (abs(q1 - q0) > limit16) * -1;
    mask |= (abs(q2 - q1) > limit16) * -1;
    mask |= (abs(q3 - q2) > limit16) * -1;
    mask |= (abs(p0 - q0) * 2 + abs(p1 - q1) / 2 > blimit16) * -1;
    return ~mask;
}

static INLINE int8_t highbd_flat_mask4(uint8_t thresh, uint16_t p3, uint16_t p2, uint16_t p1, uint16_t p0, uint16_t q0,
                                       uint16_t q1, uint16_t q2, uint16_t q3, int32_t bd) {
    int8_t  mask     = 0;
    int16_t thresh16 = (uint16_t)thresh << (bd - 8);
    mask |= (abs(p1 - p0) > thresh16) * -1;
    mask |= (abs(q1 - q0) > thresh16) * -1;
    mask |= (abs(p2 - p0) > thresh16) * -1;
    mask |= (abs(q2 - q0) > thresh16) * -1;
    mask |= (abs(p3 - p0) > thresh16) * -1;
    mask |= (abs(q3 - q0) > thresh16) * -1;
    return ~mask;
}

// Is there high edge variance internal edge:
// 11111111_11111111 yes, 00000000_00000000 no ?
static INLINE int16_t highbd_hev_mask(uint8_t thresh, uint16_t p1, uint16_t p0, uint16_t q0, uint16_t q1, int32_t bd) {
    int16_t hev      = 0;
    int16_t thresh16 = (uint16_t)thresh << (bd - 8);
    hev |= (abs(p1 - p0) > thresh16) * -1;
    hev |= (abs(q1 - q0) > thresh16) * -1;
    return hev;
}

static INLINE void highbd_filter4(int8_t mask, uint8_t thresh, uint16_t* op1, uint16_t* op0, uint16_t* oq0,
                                  uint16_t* oq1, int32_t bd) {
    int16_t filter1, filter2;
    // ^0x80 equivalent to subtracting 0x80 from the values to turn them
    // into -128 to +127 instead of 0 to 255.
    int32_t       shift = bd - 8;
    const int16_t ps1   = (int16_t)*op1 - (0x80 << shift);
    const int16_t ps0   = (int16_t)*op0 - (0x80 << shift);
    const int16_t qs0   = (int16_t)*oq0 - (0x80 << shift);
    const int16_t qs1   = (int16_t)*oq1 - (0x80 << shift);
    const int16_t hev   = highbd_hev_mask(thresh, *op1, *op0, *oq0, *oq1, bd);

    // Add outer taps if we have high edge variance.
    int16_t filter = signed_char_clamp_high(ps1 - qs1, bd) & hev;

    // Inner taps.
    filter = signed_char_clamp_high(filter + 3 * (qs0 - ps0), bd) & mask;

    // Save bottom 3 bits so that we round one side +4 and the other +3
    // if it equals 4 we'll set to adjust by -1 to account for the fact
    // we'd round 3 the other way.
    filter1 = signed_char_clamp_high(filter + 4, bd) >> 3;
    filter2 = signed_char_clamp_high(filter + 3, bd) >> 3;

    *oq0 = signed_char_clamp_high(qs0 - filter1, bd) + (0x80 << shift);
    *op0 = signed_char_clamp_high(ps0 + filter2, bd) + (0x80 << shift);

    // Outer tap adjustments.
    filter = ROUND_POWER_OF_TWO(filter1, 1) & ~hev;

    *oq1 = signed_char_clamp_high(qs1 - filter, bd) + (0x80 << shift);
    *op1 = signed_char_clamp_high(ps1 + filter, bd) + (0x80 << shift);
}

void svt_aom_highbd_lpf_horizontal_4_c(uint16_t* s, int32_t p /* pitch */, const uint8_t* blimit, const uint8_t* limit,
                                       const uint8_t* thresh, int32_t bd) {
    int32_t i;
    int32_t count = 4;

    // loop filter designed to work using chars so that we can make maximum use
    // of 8 bit simd instructions.
    for (i = 0; i < count; ++i) {
        const uint16_t p1   = s[-2 * p];
        const uint16_t p0   = s[-p];
        const uint16_t q0   = s[0 * p];
        const uint16_t q1   = s[1 * p];
        const int8_t   mask = highbd_filter_mask2(*limit, *blimit, p1, p0, q0, q1, bd);
        highbd_filter4(mask, *thresh, s - 2 * p, s - 1 * p, s, s + 1 * p, bd);
        ++s;
    }
}

void svt_aom_highbd_lpf_vertical_4_c(uint16_t* s, int32_t pitch, const uint8_t* blimit, const uint8_t* limit,
                                     const uint8_t* thresh, int32_t bd) {
    int32_t i;
    int32_t count = 4;

    // loop filter designed to work using chars so that we can make maximum use
    // of 8 bit simd instructions.
    for (i = 0; i < count; ++i) {
        const uint16_t p1 = s[-2], p0 = s[-1];
        const uint16_t q0 = s[0], q1 = s[1];
        const int8_t   mask = highbd_filter_mask2(*limit, *blimit, p1, p0, q0, q1, bd);
        highbd_filter4(mask, *thresh, s - 2, s - 1, s, s + 1, bd);
        s += pitch;
    }
}

static INLINE void highbd_filter8(int8_t mask, uint8_t thresh, int8_t flat, uint16_t* op3, uint16_t* op2, uint16_t* op1,
                                  uint16_t* op0, uint16_t* oq0, uint16_t* oq1, uint16_t* oq2, uint16_t* oq3,
                                  int32_t bd) {
    if (flat && mask) {
        const uint16_t p3 = *op3, p2 = *op2, p1 = *op1, p0 = *op0;
        const uint16_t q0 = *oq0, q1 = *oq1, q2 = *oq2, q3 = *oq3;

        // 7-tap filter [1, 1, 1, 2, 1, 1, 1]
        *op2 = ROUND_POWER_OF_TWO(p3 + p3 + p3 + 2 * p2 + p1 + p0 + q0, 3);
        *op1 = ROUND_POWER_OF_TWO(p3 + p3 + p2 + 2 * p1 + p0 + q0 + q1, 3);
        *op0 = ROUND_POWER_OF_TWO(p3 + p2 + p1 + 2 * p0 + q0 + q1 + q2, 3);
        *oq0 = ROUND_POWER_OF_TWO(p2 + p1 + p0 + 2 * q0 + q1 + q2 + q3, 3);
        *oq1 = ROUND_POWER_OF_TWO(p1 + p0 + q0 + 2 * q1 + q2 + q3 + q3, 3);
        *oq2 = ROUND_POWER_OF_TWO(p0 + q0 + q1 + 2 * q2 + q3 + q3 + q3, 3);
    } else {
        highbd_filter4(mask, thresh, op1, op0, oq0, oq1, bd);
    }
}

void svt_aom_highbd_lpf_horizontal_8_c(uint16_t* s, int32_t p, const uint8_t* blimit, const uint8_t* limit,
                                       const uint8_t* thresh, int32_t bd) {
    int32_t i;
    int32_t count = 4;

    // loop filter designed to work using chars so that we can make maximum use
    // of 8 bit simd instructions.
    for (i = 0; i < count; ++i) {
        const uint16_t p3 = s[-4 * p], p2 = s[-3 * p], p1 = s[-2 * p], p0 = s[-p];
        const uint16_t q0 = s[0 * p], q1 = s[1 * p], q2 = s[2 * p], q3 = s[3 * p];

        const int8_t mask = highbd_filter_mask(*limit, *blimit, p3, p2, p1, p0, q0, q1, q2, q3, bd);
        const int8_t flat = highbd_flat_mask4(1, p3, p2, p1, p0, q0, q1, q2, q3, bd);
        highbd_filter8(
            mask, *thresh, flat, s - 4 * p, s - 3 * p, s - 2 * p, s - 1 * p, s, s + 1 * p, s + 2 * p, s + 3 * p, bd);
        ++s;
    }
}

void svt_aom_highbd_lpf_vertical_8_c(uint16_t* s, int32_t pitch, const uint8_t* blimit, const uint8_t* limit,
                                     const uint8_t* thresh, int32_t bd) {
    int32_t i;
    int32_t count = 4;

    for (i = 0; i < count; ++i) {
        const uint16_t p3 = s[-4], p2 = s[-3], p1 = s[-2], p0 = s[-1];
        const uint16_t q0 = s[0], q1 = s[1], q2 = s[2], q3 = s[3];
        const int8_t   mask = highbd_filter_mask(*limit, *blimit, p3, p2, p1, p0, q0, q1, q2, q3, bd);
        const int8_t   flat = highbd_flat_mask4(1, p3, p2, p1, p0, q0, q1, q2, q3, bd);
        highbd_filter8(mask, *thresh, flat, s - 4, s - 3, s - 2, s - 1, s, s + 1, s + 2, s + 3, bd);
        s += pitch;
    }
}

//**********************************************************************************************************************//

//static const SEG_LVL_FEATURES seg_lvl_lf_lut[MAX_MB_PLANE][2] = {
//    { SEG_LVL_ALT_LF_Y_V, SEG_LVL_ALT_LF_Y_H },
//    { SEG_LVL_ALT_LF_U, SEG_LVL_ALT_LF_U },
//    { SEG_LVL_ALT_LF_V, SEG_LVL_ALT_LF_V }
//};

void svt_aom_update_sharpness(LoopFilterInfoN* lfi, int32_t sharpness_lvl) {
    int32_t lvl;

    // For each possible value for the loop filter fill out limits
    for (lvl = 0; lvl <= MAX_LOOP_FILTER; lvl++) {
        // Set loop filter parameters that control sharpness.
        int32_t block_inside_limit = lvl >> ((sharpness_lvl > 0) + (sharpness_lvl > 4));

        if (sharpness_lvl > 0) {
            if (block_inside_limit > (9 - sharpness_lvl)) {
                block_inside_limit = (9 - sharpness_lvl);
            }
        }

        if (block_inside_limit < 1) {
            block_inside_limit = 1;
        }

        memset(lfi->lfthr[lvl].lim, block_inside_limit, SIMD_WIDTH);
        memset(lfi->lfthr[lvl].mblim, (2 * (lvl + 2) + block_inside_limit), SIMD_WIDTH);
    }
}

static INLINE void highbd_filter14(int8_t mask, uint8_t thresh, int8_t flat, int8_t flat2, uint16_t* op6, uint16_t* op5,
                                   uint16_t* op4, uint16_t* op3, uint16_t* op2, uint16_t* op1, uint16_t* op0,
                                   uint16_t* oq0, uint16_t* oq1, uint16_t* oq2, uint16_t* oq3, uint16_t* oq4,
                                   uint16_t* oq5, uint16_t* oq6, int bd) {
    if (flat2 && flat && mask) {
        const uint16_t p6 = *op6;
        const uint16_t p5 = *op5;
        const uint16_t p4 = *op4;
        const uint16_t p3 = *op3;
        const uint16_t p2 = *op2;
        const uint16_t p1 = *op1;
        const uint16_t p0 = *op0;
        const uint16_t q0 = *oq0;
        const uint16_t q1 = *oq1;
        const uint16_t q2 = *oq2;
        const uint16_t q3 = *oq3;
        const uint16_t q4 = *oq4;
        const uint16_t q5 = *oq5;
        const uint16_t q6 = *oq6;

        // 13-tap filter [1, 1, 1, 1, 1, 2, 2, 2, 1, 1, 1, 1, 1]
        *op5 = ROUND_POWER_OF_TWO(p6 * 7 + p5 * 2 + p4 * 2 + p3 + p2 + p1 + p0 + q0, 4);
        *op4 = ROUND_POWER_OF_TWO(p6 * 5 + p5 * 2 + p4 * 2 + p3 * 2 + p2 + p1 + p0 + q0 + q1, 4);
        *op3 = ROUND_POWER_OF_TWO(p6 * 4 + p5 + p4 * 2 + p3 * 2 + p2 * 2 + p1 + p0 + q0 + q1 + q2, 4);
        *op2 = ROUND_POWER_OF_TWO(p6 * 3 + p5 + p4 + p3 * 2 + p2 * 2 + p1 * 2 + p0 + q0 + q1 + q2 + q3, 4);
        *op1 = ROUND_POWER_OF_TWO(p6 * 2 + p5 + p4 + p3 + p2 * 2 + p1 * 2 + p0 * 2 + q0 + q1 + q2 + q3 + q4, 4);
        *op0 = ROUND_POWER_OF_TWO(p6 + p5 + p4 + p3 + p2 + p1 * 2 + p0 * 2 + q0 * 2 + q1 + q2 + q3 + q4 + q5, 4);
        *oq0 = ROUND_POWER_OF_TWO(p5 + p4 + p3 + p2 + p1 + p0 * 2 + q0 * 2 + q1 * 2 + q2 + q3 + q4 + q5 + q6, 4);
        *oq1 = ROUND_POWER_OF_TWO(p4 + p3 + p2 + p1 + p0 + q0 * 2 + q1 * 2 + q2 * 2 + q3 + q4 + q5 + q6 * 2, 4);
        *oq2 = ROUND_POWER_OF_TWO(p3 + p2 + p1 + p0 + q0 + q1 * 2 + q2 * 2 + q3 * 2 + q4 + q5 + q6 * 3, 4);
        *oq3 = ROUND_POWER_OF_TWO(p2 + p1 + p0 + q0 + q1 + q2 * 2 + q3 * 2 + q4 * 2 + q5 + q6 * 4, 4);
        *oq4 = ROUND_POWER_OF_TWO(p1 + p0 + q0 + q1 + q2 + q3 * 2 + q4 * 2 + q5 * 2 + q6 * 5, 4);
        *oq5 = ROUND_POWER_OF_TWO(p0 + q0 + q1 + q2 + q3 + q4 * 2 + q5 * 2 + q6 * 7, 4);
    } else {
        highbd_filter8(mask, thresh, flat, op3, op2, op1, op0, oq0, oq1, oq2, oq3, bd);
    }
}

static void highbd_mb_lpf_horizontal_edge_w(uint16_t* s, int p, const uint8_t* blimit, const uint8_t* limit,
                                            const uint8_t* thresh, int count, int bd) {
    int i;
    int step = 4;

    // loop filter designed to work using chars so that we can make maximum use
    // of 8 bit simd instructions.
    for (i = 0; i < step * count; ++i) {
        const uint16_t p3   = s[-4 * p];
        const uint16_t p2   = s[-3 * p];
        const uint16_t p1   = s[-2 * p];
        const uint16_t p0   = s[-p];
        const uint16_t q0   = s[0 * p];
        const uint16_t q1   = s[1 * p];
        const uint16_t q2   = s[2 * p];
        const uint16_t q3   = s[3 * p];
        const int8_t   mask = highbd_filter_mask(*limit, *blimit, p3, p2, p1, p0, q0, q1, q2, q3, bd);
        const int8_t   flat = highbd_flat_mask4(1, p3, p2, p1, p0, q0, q1, q2, q3, bd);

        const int8_t flat2 = highbd_flat_mask4(
            1, s[-7 * p], s[-6 * p], s[-5 * p], p0, q0, s[4 * p], s[5 * p], s[6 * p], bd);

        highbd_filter14(mask,
                        *thresh,
                        flat,
                        flat2,
                        s - 7 * p,
                        s - 6 * p,
                        s - 5 * p,
                        s - 4 * p,
                        s - 3 * p,
                        s - 2 * p,
                        s - 1 * p,
                        s,
                        s + 1 * p,
                        s + 2 * p,
                        s + 3 * p,
                        s + 4 * p,
                        s + 5 * p,
                        s + 6 * p,
                        bd);
        ++s;
    }
}

void svt_aom_highbd_lpf_horizontal_14_c(uint16_t* s, int pitch, const uint8_t* blimit, const uint8_t* limit,
                                        const uint8_t* thresh, int bd) {
    highbd_mb_lpf_horizontal_edge_w(s, pitch, blimit, limit, thresh, 1, bd);
}

static INLINE int8_t highbd_filter_mask3_chroma(uint8_t limit, uint8_t blimit, uint16_t p2, uint16_t p1, uint16_t p0,
                                                uint16_t q0, uint16_t q1, uint16_t q2, int bd) {
    int8_t  mask     = 0;
    int16_t limit16  = (uint16_t)limit << (bd - 8);
    int16_t blimit16 = (uint16_t)blimit << (bd - 8);
    mask |= (abs(p2 - p1) > limit16) * -1;
    mask |= (abs(p1 - p0) > limit16) * -1;
    mask |= (abs(q1 - q0) > limit16) * -1;
    mask |= (abs(q2 - q1) > limit16) * -1;
    mask |= (abs(p0 - q0) * 2 + abs(p1 - q1) / 2 > blimit16) * -1;
    return ~mask;
}

static INLINE void highbd_filter6(int8_t mask, uint8_t thresh, int8_t flat, uint16_t* op2, uint16_t* op1, uint16_t* op0,
                                  uint16_t* oq0, uint16_t* oq1, uint16_t* oq2, int bd) {
    if (flat && mask) {
        const uint16_t p2 = *op2, p1 = *op1, p0 = *op0;
        const uint16_t q0 = *oq0, q1 = *oq1, q2 = *oq2;

        // 5-tap filter [1, 2, 2, 2, 1]
        *op1 = ROUND_POWER_OF_TWO(p2 * 3 + p1 * 2 + p0 * 2 + q0, 3);
        *op0 = ROUND_POWER_OF_TWO(p2 + p1 * 2 + p0 * 2 + q0 * 2 + q1, 3);
        *oq0 = ROUND_POWER_OF_TWO(p1 + p0 * 2 + q0 * 2 + q1 * 2 + q2, 3);
        *oq1 = ROUND_POWER_OF_TWO(p0 + q0 * 2 + q1 * 2 + q2 * 3, 3);
    } else {
        highbd_filter4(mask, thresh, op1, op0, oq0, oq1, bd);
    }
}

void svt_aom_highbd_lpf_vertical_6_c(uint16_t* s, int pitch, const uint8_t* blimit, const uint8_t* limit,
                                     const uint8_t* thresh, int bd) {
    int i;
    int count = 4;

    for (i = 0; i < count; ++i) {
        const uint16_t p2 = s[-3], p1 = s[-2], p0 = s[-1];
        const uint16_t q0 = s[0], q1 = s[1], q2 = s[2];
        const int8_t   mask = highbd_filter_mask3_chroma(*limit, *blimit, p2, p1, p0, q0, q1, q2, bd);
        const int8_t   flat = highbd_flat_mask3_chroma(1, p2, p1, p0, q0, q1, q2, bd);
        highbd_filter6(mask, *thresh, flat, s - 3, s - 2, s - 1, s, s + 1, s + 2, bd);
        s += pitch;
    }
}

void svt_aom_highbd_lpf_horizontal_6_c(uint16_t* s, int p, const uint8_t* blimit, const uint8_t* limit,
                                       const uint8_t* thresh, int bd) {
    int i;
    int count = 4;

    // loop filter designed to work using chars so that we can make maximum use
    // of 8 bit simd instructions.
    for (i = 0; i < count; ++i) {
        const uint16_t p2 = s[-3 * p], p1 = s[-2 * p], p0 = s[-p];
        const uint16_t q0 = s[0 * p], q1 = s[1 * p], q2 = s[2 * p];

        const int8_t mask = highbd_filter_mask3_chroma(*limit, *blimit, p2, p1, p0, q0, q1, q2, bd);
        const int8_t flat = highbd_flat_mask3_chroma(1, p2, p1, p0, q0, q1, q2, bd);
        highbd_filter6(mask, *thresh, flat, s - 3 * p, s - 2 * p, s - 1 * p, s, s + 1 * p, s + 2 * p, bd);
        ++s;
    }
}

static void highbd_mb_lpf_vertical_edge_w(uint16_t* s, int p, const uint8_t* blimit, const uint8_t* limit,
                                          const uint8_t* thresh, int count, int bd) {
    int i;

    for (i = 0; i < count; ++i) {
        const uint16_t p3    = s[-4];
        const uint16_t p2    = s[-3];
        const uint16_t p1    = s[-2];
        const uint16_t p0    = s[-1];
        const uint16_t q0    = s[0];
        const uint16_t q1    = s[1];
        const uint16_t q2    = s[2];
        const uint16_t q3    = s[3];
        const int8_t   mask  = highbd_filter_mask(*limit, *blimit, p3, p2, p1, p0, q0, q1, q2, q3, bd);
        const int8_t   flat  = highbd_flat_mask4(1, p3, p2, p1, p0, q0, q1, q2, q3, bd);
        const int8_t   flat2 = highbd_flat_mask4(1, s[-7], s[-6], s[-5], p0, q0, s[4], s[5], s[6], bd);

        highbd_filter14(mask,
                        *thresh,
                        flat,
                        flat2,
                        s - 7,
                        s - 6,
                        s - 5,
                        s - 4,
                        s - 3,
                        s - 2,
                        s - 1,
                        s,
                        s + 1,
                        s + 2,
                        s + 3,
                        s + 4,
                        s + 5,
                        s + 6,
                        bd);
        s += p;
    }
}

void svt_aom_highbd_lpf_vertical_14_c(uint16_t* s, int p, const uint8_t* blimit, const uint8_t* limit,
                                      const uint8_t* thresh, int bd) {
    highbd_mb_lpf_vertical_edge_w(s, p, blimit, limit, thresh, 4, bd);
}

static INLINE void filter14(int8_t mask, uint8_t thresh, int8_t flat, int8_t flat2, uint8_t* op6, uint8_t* op5,
                            uint8_t* op4, uint8_t* op3, uint8_t* op2, uint8_t* op1, uint8_t* op0, uint8_t* oq0,
                            uint8_t* oq1, uint8_t* oq2, uint8_t* oq3, uint8_t* oq4, uint8_t* oq5, uint8_t* oq6) {
    if (flat2 && flat && mask) {
        const uint8_t p6 = *op6, p5 = *op5, p4 = *op4, p3 = *op3, p2 = *op2, p1 = *op1, p0 = *op0;
        const uint8_t q0 = *oq0, q1 = *oq1, q2 = *oq2, q3 = *oq3, q4 = *oq4, q5 = *oq5, q6 = *oq6;

        // 13-tap filter [1, 1, 1, 1, 1, 2, 2, 2, 1, 1, 1, 1, 1]
        *op5 = ROUND_POWER_OF_TWO(p6 * 7 + p5 * 2 + p4 * 2 + p3 + p2 + p1 + p0 + q0, 4);
        *op4 = ROUND_POWER_OF_TWO(p6 * 5 + p5 * 2 + p4 * 2 + p3 * 2 + p2 + p1 + p0 + q0 + q1, 4);
        *op3 = ROUND_POWER_OF_TWO(p6 * 4 + p5 + p4 * 2 + p3 * 2 + p2 * 2 + p1 + p0 + q0 + q1 + q2, 4);
        *op2 = ROUND_POWER_OF_TWO(p6 * 3 + p5 + p4 + p3 * 2 + p2 * 2 + p1 * 2 + p0 + q0 + q1 + q2 + q3, 4);
        *op1 = ROUND_POWER_OF_TWO(p6 * 2 + p5 + p4 + p3 + p2 * 2 + p1 * 2 + p0 * 2 + q0 + q1 + q2 + q3 + q4, 4);
        *op0 = ROUND_POWER_OF_TWO(p6 + p5 + p4 + p3 + p2 + p1 * 2 + p0 * 2 + q0 * 2 + q1 + q2 + q3 + q4 + q5, 4);
        *oq0 = ROUND_POWER_OF_TWO(p5 + p4 + p3 + p2 + p1 + p0 * 2 + q0 * 2 + q1 * 2 + q2 + q3 + q4 + q5 + q6, 4);
        *oq1 = ROUND_POWER_OF_TWO(p4 + p3 + p2 + p1 + p0 + q0 * 2 + q1 * 2 + q2 * 2 + q3 + q4 + q5 + q6 * 2, 4);
        *oq2 = ROUND_POWER_OF_TWO(p3 + p2 + p1 + p0 + q0 + q1 * 2 + q2 * 2 + q3 * 2 + q4 + q5 + q6 * 3, 4);
        *oq3 = ROUND_POWER_OF_TWO(p2 + p1 + p0 + q0 + q1 + q2 * 2 + q3 * 2 + q4 * 2 + q5 + q6 * 4, 4);
        *oq4 = ROUND_POWER_OF_TWO(p1 + p0 + q0 + q1 + q2 + q3 * 2 + q4 * 2 + q5 * 2 + q6 * 5, 4);
        *oq5 = ROUND_POWER_OF_TWO(p0 + q0 + q1 + q2 + q3 + q4 * 2 + q5 * 2 + q6 * 7, 4);
    } else {
        filter8(mask, thresh, flat, op3, op2, op1, op0, oq0, oq1, oq2, oq3);
    }
}

static void mb_lpf_horizontal_edge_w(uint8_t* s, int p, const uint8_t* blimit, const uint8_t* limit,
                                     const uint8_t* thresh, int count) {
    int i;
    int step = 4;

    // loop filter designed to work using chars so that we can make maximum use
    // of 8 bit simd instructions.
    for (i = 0; i < step * count; ++i) {
        const uint8_t p6 = s[-7 * p], p5 = s[-6 * p], p4 = s[-5 * p], p3 = s[-4 * p], p2 = s[-3 * p], p1 = s[-2 * p],
                      p0 = s[-p];
        const uint8_t q0 = s[0 * p], q1 = s[1 * p], q2 = s[2 * p], q3 = s[3 * p], q4 = s[4 * p], q5 = s[5 * p],
                      q6   = s[6 * p];
        const int8_t mask  = filter_mask(*limit, *blimit, p3, p2, p1, p0, q0, q1, q2, q3);
        const int8_t flat  = flat_mask4(1, p3, p2, p1, p0, q0, q1, q2, q3);
        const int8_t flat2 = flat_mask4(1, p6, p5, p4, p0, q0, q4, q5, q6);

        filter14(mask,
                 *thresh,
                 flat,
                 flat2,
                 s - 7 * p,
                 s - 6 * p,
                 s - 5 * p,
                 s - 4 * p,
                 s - 3 * p,
                 s - 2 * p,
                 s - 1 * p,
                 s,
                 s + 1 * p,
                 s + 2 * p,
                 s + 3 * p,
                 s + 4 * p,
                 s + 5 * p,
                 s + 6 * p);
        ++s;
    }
}

void svt_aom_lpf_horizontal_14_c(uint8_t* s, int p, const uint8_t* blimit, const uint8_t* limit,
                                 const uint8_t* thresh) {
    mb_lpf_horizontal_edge_w(s, p, blimit, limit, thresh, 1);
}

static void mb_lpf_vertical_edge_w(uint8_t* s, int p, const uint8_t* blimit, const uint8_t* limit,
                                   const uint8_t* thresh, int count) {
    int i;

    for (i = 0; i < count; ++i) {
        const uint8_t p6 = s[-7], p5 = s[-6], p4 = s[-5], p3 = s[-4], p2 = s[-3], p1 = s[-2], p0 = s[-1];
        const uint8_t q0 = s[0], q1 = s[1], q2 = s[2], q3 = s[3], q4 = s[4], q5 = s[5], q6 = s[6];
        const int8_t  mask  = filter_mask(*limit, *blimit, p3, p2, p1, p0, q0, q1, q2, q3);
        const int8_t  flat  = flat_mask4(1, p3, p2, p1, p0, q0, q1, q2, q3);
        const int8_t  flat2 = flat_mask4(1, p6, p5, p4, p0, q0, q4, q5, q6);

        filter14(mask,
                 *thresh,
                 flat,
                 flat2,
                 s - 7,
                 s - 6,
                 s - 5,
                 s - 4,
                 s - 3,
                 s - 2,
                 s - 1,
                 s,
                 s + 1,
                 s + 2,
                 s + 3,
                 s + 4,
                 s + 5,
                 s + 6);
        s += p;
    }
}

void svt_aom_lpf_vertical_14_c(uint8_t* s, int p, const uint8_t* blimit, const uint8_t* limit, const uint8_t* thresh) {
    mb_lpf_vertical_edge_w(s, p, blimit, limit, thresh, 4);
}
