/*
* Copyright(c) 2019 Intel Corporation
* Copyright (c) 2016, Alliance for Open Media. All rights reserved
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at www.aomedia.org/license/patent.
*/

#include <string.h>

#include "definitions.h"
#include "adaptive_mv_pred.h"
#include "EbSvtAv1.h"
#include "md_process.h"
#include "common_utils.h"
#include "entropy_coding.h"
#include "inter_prediction.h"
#include "aom_dsp_rtcd.h"
#include "md_process.h"

int  svt_av1_get_spatial_seg_prediction(PictureControlSet* pcs, MacroBlockD* xd, uint32_t blk_org_x, uint32_t blk_org_y,
                                        int* cdf_index);
void svt_av1_update_segmentation_map(PictureControlSet* pcs, BlockSize bsize, uint32_t blk_org_x, uint32_t blk_org_y,
                                     uint8_t segment_id);

#define MVREF_ROWS 3
#define MVREF_COLS 3

typedef struct Pos {
    int32_t row;
    int32_t col;
} Pos;

// clang-format on

static INLINE Mv get_block_mv(const MbModeInfo* candidate, int32_t which_mv) {
    return candidate->block_mi.mv[which_mv];
}

static INLINE int32_t is_inside(const TileInfo* const tile, int32_t mi_col, int32_t mi_row, const Pos* mi_pos) {
    return !(mi_row + mi_pos->row < tile->mi_row_start || mi_col + mi_pos->col < tile->mi_col_start ||
             mi_row + mi_pos->row >= tile->mi_row_end || mi_col + mi_pos->col >= tile->mi_col_end);
}

static INLINE void clamp_mv_ref(Mv* mv, int32_t bw, int32_t bh, const MacroBlockD* xd) {
    clamp_mv(mv,
             xd->mb_to_left_edge - bw * 8 - MV_BORDER,
             xd->mb_to_right_edge + bw * 8 + MV_BORDER,
             xd->mb_to_top_edge - bh * 8 - MV_BORDER,
             xd->mb_to_bottom_edge + bh * 8 + MV_BORDER);
}

static void add_ref_mv_candidate(const MbModeInfo* const candidate, const MvReferenceFrame rf[2], uint8_t* refmv_count,
                                 uint8_t* ref_match_count, uint8_t* newmv_count,
                                 CandidateMv ref_mv_stack[MAX_REF_MV_STACK_SIZE], int32_t len, Mv* gm_mv_candidates,
                                 const WarpedMotionParams* gm_params, int32_t weight) {
    if (!is_inter_block(&candidate->block_mi)) {
        return; // for intrabc
    }
    assert(weight % 2 == 0);

    if (rf[1] == NONE_FRAME) {
        // single reference frame
        for (int32_t ref = 0; ref < 2; ++ref) {
            if (candidate->block_mi.ref_frame[ref] == rf[0]) {
                Mv this_refmv;
                if (is_global_mv_block(candidate->block_mi.mode, candidate->bsize, gm_params[rf[0]].wmtype)) {
                    this_refmv = gm_mv_candidates[0];
                } else {
                    this_refmv = get_block_mv(candidate, ref);
                }
                int32_t index;
                for (index = 0; index < *refmv_count; ++index) {
                    if (ref_mv_stack[index].this_mv.as_int == this_refmv.as_int) {
                        ref_mv_stack[index].weight += weight * len;
                        break;
                    }
                }
                // Add a new item to the list.
                if (index == *refmv_count && *refmv_count < MAX_REF_MV_STACK_SIZE) {
                    ref_mv_stack[index].this_mv = this_refmv;
                    ref_mv_stack[index].weight  = weight * len;
                    ++(*refmv_count);
                }
                if (svt_aom_have_newmv_in_inter_mode(candidate->block_mi.mode)) {
                    ++*newmv_count;
                }
                ++*ref_match_count;
            }
        }
    } else {
        // compound reference frame
        if (candidate->block_mi.ref_frame[0] == rf[0] && candidate->block_mi.ref_frame[1] == rf[1]) {
            Mv this_refmv[2];

            for (int32_t ref = 0; ref < 2; ++ref) {
                if (is_global_mv_block(candidate->block_mi.mode, candidate->bsize, gm_params[rf[ref]].wmtype)) {
                    this_refmv[ref] = gm_mv_candidates[ref];
                } else {
                    this_refmv[ref] = get_block_mv(candidate, ref);
                }
            }
            int32_t index;
            for (index = 0; index < *refmv_count; ++index) {
                if ((ref_mv_stack[index].this_mv.as_int == this_refmv[0].as_int) &&
                    (ref_mv_stack[index].comp_mv.as_int == this_refmv[1].as_int)) {
                    ref_mv_stack[index].weight += weight * len;
                    break;
                }
            }
            // Add a new item to the list.
            if (index == *refmv_count && *refmv_count < MAX_REF_MV_STACK_SIZE) {
                ref_mv_stack[index].this_mv = this_refmv[0];
                ref_mv_stack[index].comp_mv = this_refmv[1];
                ref_mv_stack[index].weight  = weight * len;
                ++(*refmv_count);
            }
            if (svt_aom_have_newmv_in_inter_mode(candidate->block_mi.mode)) {
                ++*newmv_count;
            }
            ++*ref_match_count;
        }
    }
}

static void scan_row_mbmi(const Av1Common* cm, const MacroBlockD* xd, int32_t mi_col, const MvReferenceFrame rf[2],
                          int32_t row_offset, CandidateMv ref_mv_stack[MAX_REF_MV_STACK_SIZE], uint8_t* refmv_count,
                          uint8_t* ref_match_count, uint8_t* newmv_count, Mv* gm_mv_candidates,
                          const WarpedMotionParams* gm_params, int32_t max_row_offset, int32_t* processed_rows) {
    int32_t end_mi        = AOMMIN(xd->n8_w, cm->mi_cols - mi_col);
    end_mi                = AOMMIN(end_mi, mi_size_wide[BLOCK_64X64]);
    const int32_t n8_w_8  = mi_size_wide[BLOCK_8X8];
    const int32_t n8_w_16 = mi_size_wide[BLOCK_16X16];
    int32_t       i;
    int32_t       col_offset = 0;
    const int32_t shift      = 0;
    if (abs(row_offset) > 1) {
        col_offset = 1;
        if (mi_col & 0x01 && xd->n8_w < n8_w_8) {
            --col_offset;
        }
    }
    const int32_t      use_step_16   = (xd->n8_w >= 16);
    MbModeInfo** const candidate_mi0 = xd->mi + row_offset * xd->mi_stride;

    for (i = 0; i < end_mi;) {
        const MbModeInfo* const candidate       = candidate_mi0[col_offset + i];
        const int32_t           candidate_bsize = candidate->bsize;
        assert(candidate_bsize < BLOCK_SIZES_ALL);
        const int32_t n8_w = mi_size_wide[candidate_bsize];
        int32_t       len  = AOMMIN(xd->n8_w, n8_w);
        if (use_step_16) {
            len = AOMMAX(n8_w_16, len);
        } else if (abs(row_offset) > 1) {
            len = AOMMAX(len, n8_w_8);
        }

        int32_t weight = 2;
        if (xd->n8_w >= n8_w_8 && xd->n8_w <= n8_w) {
            int32_t inc = AOMMIN(-max_row_offset + row_offset + 1, mi_size_high[candidate_bsize]);
            // Obtain range used in weight calculation.
            weight = AOMMAX(weight, (inc << shift));
            // Update processed rows.
            *processed_rows = inc - row_offset - 1;
        }

        add_ref_mv_candidate(candidate,
                             rf,
                             refmv_count,
                             ref_match_count,
                             newmv_count,
                             ref_mv_stack,
                             len,
                             gm_mv_candidates,
                             gm_params,
                             weight);

        i += len;
    }
}

static void scan_col_mbmi(const Av1Common* cm, const MacroBlockD* xd, int32_t mi_row, const MvReferenceFrame rf[2],
                          int32_t col_offset, CandidateMv ref_mv_stack[MAX_REF_MV_STACK_SIZE], uint8_t* refmv_count,
                          uint8_t* ref_match_count, uint8_t* newmv_count, Mv* gm_mv_candidates,
                          const WarpedMotionParams* gm_params, int32_t max_col_offset, int32_t* processed_cols) {
    int32_t end_mi        = AOMMIN(xd->n8_h, cm->mi_rows - mi_row);
    end_mi                = AOMMIN(end_mi, mi_size_high[BLOCK_64X64]);
    const int32_t n8_h_8  = mi_size_high[BLOCK_8X8];
    const int32_t n8_h_16 = mi_size_high[BLOCK_16X16];
    int32_t       i;
    int32_t       row_offset = 0;
    const int32_t shift      = 0;
    if (abs(col_offset) > 1) {
        row_offset = 1;
        if (mi_row & 0x01 && xd->n8_h < n8_h_8) {
            --row_offset;
        }
    }
    const int32_t use_step_16 = (xd->n8_h >= 16);

    for (i = 0; i < end_mi;) {
        const MbModeInfo* const candidate       = xd->mi[(row_offset + i) * xd->mi_stride + col_offset];
        const int32_t           candidate_bsize = candidate->bsize;
        assert(candidate_bsize < BLOCK_SIZES_ALL);
        const int32_t n8_h = mi_size_high[candidate_bsize];
        int32_t       len  = AOMMIN(xd->n8_h, n8_h);
        if (use_step_16) {
            len = AOMMAX(n8_h_16, len);
        } else if (abs(col_offset) > 1) {
            len = AOMMAX(len, n8_h_8);
        }

        int32_t weight = 2;
        if (xd->n8_h >= n8_h_8 && xd->n8_h <= n8_h) {
            int32_t inc = AOMMIN(-max_col_offset + col_offset + 1, mi_size_wide[candidate_bsize]);
            // Obtain range used in weight calculation.
            weight = AOMMAX(weight, (inc << shift));
            // Update processed cols.
            *processed_cols = inc - col_offset - 1;
        }

        add_ref_mv_candidate(candidate,
                             rf,
                             refmv_count,
                             ref_match_count,
                             newmv_count,
                             ref_mv_stack,
                             len,
                             gm_mv_candidates,
                             gm_params,
                             weight);

        i += len;
    }
}

static void scan_blk_mbmi(const MacroBlockD* xd, const int32_t mi_row, const int32_t mi_col,
                          const MvReferenceFrame rf[2], int32_t row_offset, int32_t col_offset,
                          CandidateMv ref_mv_stack[MAX_REF_MV_STACK_SIZE], uint8_t* ref_match_count,
                          uint8_t* newmv_count, Mv* gm_mv_candidates, const WarpedMotionParams* gm_params,
                          uint8_t* refmv_count) {
    const TileInfo* const tile   = &xd->tile;
    Pos                   mi_pos = {row_offset, col_offset};

    // Analyze a single 8x8 block motion information.
    if (is_inside(tile, mi_col, mi_row, &mi_pos)) {
        const MbModeInfo* const candidate = xd->mi[mi_pos.row * xd->mi_stride + mi_pos.col];

        add_ref_mv_candidate(candidate,
                             rf,
                             refmv_count,
                             ref_match_count,
                             newmv_count,
                             ref_mv_stack,
                             mi_size_wide[BLOCK_8X8],
                             gm_mv_candidates,
                             gm_params,
                             2);
    }
}

static int32_t has_top_right(const BlockSize sb_size, const MacroBlockD* xd, int32_t mi_row, int32_t mi_col,
                             int32_t bs) {
    if (bs > mi_size_wide[BLOCK_64X64]) {
        return 0;
    }

    // The bottom of two horizontal rectangles never has a top right (as the block
    // to the right won't have been decoded)
    if (xd->n8_w > xd->n8_h) {
        if (xd->is_sec_rect) {
            return 0;
        }
    }

    // The left hand of two vertical rectangles always has a top right (as the
    // block above will have been decoded)
    if (xd->n8_w < xd->n8_h) {
        if (!xd->is_sec_rect) {
            return 1;
        }
    }

    // bs > 0 and bs is a power of 2
    assert(bs > 0 && !(bs & (bs - 1)));

    const int32_t sb_mi_size = mi_size_wide[sb_size];
    const int32_t mask_row   = mi_row & (sb_mi_size - 1);
    const int32_t mask_col   = mi_col & (sb_mi_size - 1);

    // In a split partition all apart from the bottom right has a top right
    int32_t has_tr = !((mask_row & bs) && (mask_col & bs));

    // For each 4x4 group of blocks, when the bottom right is decoded the blocks
    // to the right have not been decoded therefore the bottom right does
    // not have a top right
    while (bs < sb_mi_size) {
        if (mask_col & bs) {
            if ((mask_col & (2 * bs)) && (mask_row & (2 * bs))) {
                has_tr = 0;
                break;
            }
        } else {
            break;
        }
        bs <<= 1;
    }

    // The bottom left square of a Vertical A (in the old format) does
    // not have a top right as it is decoded before the right hand
    // rectangle of the partition
    if (xd->mi[0]->partition == PARTITION_VERT_A) {
        if (xd->n8_w == xd->n8_h) {
            if (mask_row & bs) {
                return 0;
            }
        }
    }

    return has_tr;
}

static INLINE int32_t find_valid_row_offset(const TileInfo* const tile, int32_t mi_row, int32_t row_offset) {
    return clamp(row_offset, tile->mi_row_start - mi_row, tile->mi_row_end - mi_row - 1);
}

static INLINE int32_t find_valid_col_offset(const TileInfo* const tile, int32_t mi_col, int32_t col_offset) {
    return clamp(col_offset, tile->mi_col_start - mi_col, tile->mi_col_end - mi_col - 1);
}

static INLINE int get_relative_dist(const OrderHintInfo* oh, int a, int b) {
    if (!oh->enable_order_hint) {
        return 0;
    }

    const int bits = oh->order_hint_bits;

    assert(bits >= 1);
    assert(a >= 0 && a < (1 << bits));
    assert(b >= 0 && b < (1 << bits));

    int       diff = a - b;
    const int m    = 1 << (bits - 1);
    diff           = (diff & (m - 1)) - (diff & m);
    return diff;
}

static int add_tpl_ref_mv(const Av1Common* cm, PictureControlSet* pcs, const MacroBlockD* xd, int mi_row, int mi_col,
                          MvReferenceFrame ref_frame, int blk_row, int blk_col, Mv* gm_mv_candidates,
                          uint8_t* const refmv_count, uint8_t two_symetric_refs, Mv* mv_ref0, int cur_offset_0,
                          int cur_offset_1,

                          CandidateMv ref_mv_stack[MAX_REF_MV_STACK_SIZE], int16_t* mode_context) {
    Pos mi_pos;
    mi_pos.row = (mi_row & 0x01) ? blk_row : blk_row + 1;
    mi_pos.col = (mi_col & 0x01) ? blk_col : blk_col + 1;

    if (!is_inside(&xd->tile, mi_col, mi_row, &mi_pos)) {
        return 0;
    }

    const TPL_MV_REF* prev_frame_mvs = pcs->tpl_mvs + ((mi_row + mi_pos.row) >> 1) * (cm->mi_stride >> 1) +
        ((mi_col + mi_pos.col) >> 1);
    if (prev_frame_mvs->mfmv0.as_int == INVALID_MV) {
        return 0;
    }

    const uint16_t weight_unit = 1;
    int            idx;

    Mv this_refmv;

    if (two_symetric_refs) {
        if (ref_frame == LAST_FRAME) {
            get_mv_projection(&this_refmv, prev_frame_mvs->mfmv0, cur_offset_0, prev_frame_mvs->ref_frame_offset);
            lower_mv_precision(&this_refmv, pcs->ppcs->frm_hdr.allow_high_precision_mv, 0);
            //store for future use
            (*mv_ref0) = this_refmv;
        } else {
            if (ref_frame == BWDREF_FRAME) {
                this_refmv.y = -mv_ref0->y;
                this_refmv.x = -mv_ref0->x;
            } else {
                this_refmv = (*mv_ref0);
            }
        }
    } else {
        get_mv_projection(&this_refmv, prev_frame_mvs->mfmv0, cur_offset_0, prev_frame_mvs->ref_frame_offset);
        lower_mv_precision(&this_refmv, pcs->ppcs->frm_hdr.allow_high_precision_mv, 0);
    }

    //single ref case could be detected by ref_frame
    if (ref_frame < LAST_BWD_FRAME) {
        if (blk_row == 0 && blk_col == 0) {
            if (abs(this_refmv.y - gm_mv_candidates[0].y) >= 16 || abs(this_refmv.x - gm_mv_candidates[0].x) >= 16) {
                *mode_context |= (1 << GLOBALMV_OFFSET);
            }
        }
        for (idx = 0; idx < *refmv_count; ++idx) {
            if (this_refmv.as_int == ref_mv_stack[idx].this_mv.as_int) {
                ref_mv_stack[idx].weight += 2 * weight_unit;
                break;
            }
        }
        if (idx == *refmv_count && *refmv_count < MAX_REF_MV_STACK_SIZE) {
            ref_mv_stack[idx].this_mv.as_int = this_refmv.as_int;
            ref_mv_stack[idx].weight         = 2 * weight_unit;
            ++(*refmv_count);
        }
    } else {
        // Process compound inter mode
        Mv comp_refmv;
        if (two_symetric_refs) {
            comp_refmv.y = -mv_ref0->y;
            comp_refmv.x = -mv_ref0->x;
        } else {
            get_mv_projection(&comp_refmv, prev_frame_mvs->mfmv0, cur_offset_1, prev_frame_mvs->ref_frame_offset);
            lower_mv_precision(&comp_refmv, pcs->ppcs->frm_hdr.allow_high_precision_mv, 0);
        }

        if (blk_row == 0 && blk_col == 0) {
            if (abs(this_refmv.y - gm_mv_candidates[0].y) >= 16 || abs(this_refmv.x - gm_mv_candidates[0].x) >= 16 ||
                abs(comp_refmv.y - gm_mv_candidates[1].y) >= 16 || abs(comp_refmv.x - gm_mv_candidates[1].x) >= 16) {
                *mode_context |= (1 << GLOBALMV_OFFSET);
            }
        }
        for (idx = 0; idx < *refmv_count; ++idx) {
            if (this_refmv.as_int == ref_mv_stack[idx].this_mv.as_int &&
                comp_refmv.as_int == ref_mv_stack[idx].comp_mv.as_int) {
                ref_mv_stack[idx].weight += 2 * weight_unit;
                break;
            }
        }
        if (idx == *refmv_count && *refmv_count < MAX_REF_MV_STACK_SIZE) {
            ref_mv_stack[idx].this_mv.as_int = this_refmv.as_int;
            ref_mv_stack[idx].comp_mv.as_int = comp_refmv.as_int;
            ref_mv_stack[idx].weight         = 2 * weight_unit;
            ++(*refmv_count);
        }
    }

    return 1;
}

// Rank the likelihood and assign nearest and near mvs.
void sort_mvp_table(CandidateMv ref_mv_stack[MAX_REF_MV_STACK_SIZE], uint8_t* refmv_count) {
    // Rank the likelihood and assign nearest and near mvs.
    uint8_t len = *refmv_count;
    while (len > 0) {
        uint8_t nr_len = 0;
        for (uint8_t idx = 1; idx < len; ++idx) {
            if (ref_mv_stack[idx - 1].weight < ref_mv_stack[idx].weight) {
                CandidateMv tmp_mv    = ref_mv_stack[idx - 1];
                ref_mv_stack[idx - 1] = ref_mv_stack[idx];
                ref_mv_stack[idx]     = tmp_mv;
                nr_len                = idx;
            }
        }
        len = nr_len;
    }
}

// Perform light scan (i.e. more relaxed constraints) of ROW-1 and COL-1.  This function is called
// at the end of MVP table generation if the ref_mv_stack is not full.
void scan_row_col_light(const Av1Common* cm, const MacroBlockD* xd, int32_t mi_row, int32_t mi_col,
                        const MvReferenceFrame rf[2], CandidateMv ref_mv_stack[MAX_REF_MV_STACK_SIZE],
                        uint8_t* refmv_count, Mv* gm_mv_candidates, int32_t max_row_offset, int32_t max_col_offset) {
    uint8_t mi_width  = AOMMIN(mi_size_wide[BLOCK_64X64], xd->n8_w);
    mi_width          = AOMMIN(mi_width, cm->mi_cols - mi_col);
    uint8_t mi_height = AOMMIN(mi_size_high[BLOCK_64X64], xd->n8_h);
    mi_height         = AOMMIN(mi_height, cm->mi_rows - mi_row);
    uint8_t mi_size   = AOMMIN(mi_width, mi_height);

    // Multiple ref frames path
    if (rf[1] > NONE_FRAME) {
        //CHKN we get here only when refMVCount=0 or 1
        Mv      ref_id[2][2], ref_diff[2][2];
        uint8_t ref_id_count[2] = {0}, ref_diff_count[2] = {0};

        //CHKN  scan ROW=-1 again but with more relaxed constraints
        for (int32_t idx = 0; ABS(max_row_offset) >= 1 && idx < mi_size;) {
            const MbModeInfo* const candidate       = xd->mi[-xd->mi_stride + idx];
            const int32_t           candidate_bsize = candidate->bsize;

            for (uint8_t rf_idx = 0; rf_idx < 2; ++rf_idx) {
                MvReferenceFrame can_rf = candidate->block_mi.ref_frame[rf_idx];

                for (uint8_t cmp_idx = 0; cmp_idx < 2; ++cmp_idx) {
                    if (can_rf == rf[cmp_idx] && ref_id_count[cmp_idx] < 2) {
                        ref_id[cmp_idx][ref_id_count[cmp_idx]] = candidate->block_mi.mv[rf_idx];
                        ++ref_id_count[cmp_idx];
                    } else if (can_rf > INTRA_FRAME && ref_diff_count[cmp_idx] < 2) {
                        Mv this_mv = candidate->block_mi.mv[rf_idx];
                        if (cm->ref_frame_sign_bias[can_rf] != cm->ref_frame_sign_bias[rf[cmp_idx]]) {
                            this_mv.y = -this_mv.y;
                            this_mv.x = -this_mv.x;
                        }
                        ref_diff[cmp_idx][ref_diff_count[cmp_idx]] = this_mv;
                        ++ref_diff_count[cmp_idx];
                    }
                }
            }
            idx += mi_size_wide[candidate_bsize];
        }

        //CHKN  scan COL=-1 again but with more relaxed constraints
        for (int32_t idx = 0; ABS(max_col_offset) >= 1 && idx < mi_size;) {
            const MbModeInfo* const candidate       = xd->mi[idx * xd->mi_stride - 1];
            const int32_t           candidate_bsize = candidate->bsize;

            for (uint8_t rf_idx = 0; rf_idx < 2; ++rf_idx) {
                MvReferenceFrame can_rf = candidate->block_mi.ref_frame[rf_idx];

                for (uint8_t cmp_idx = 0; cmp_idx < 2; ++cmp_idx) {
                    if (can_rf == rf[cmp_idx] && ref_id_count[cmp_idx] < 2) {
                        ref_id[cmp_idx][ref_id_count[cmp_idx]] = candidate->block_mi.mv[rf_idx];
                        ++ref_id_count[cmp_idx];
                    } else if (can_rf > INTRA_FRAME && ref_diff_count[cmp_idx] < 2) {
                        Mv this_mv = candidate->block_mi.mv[rf_idx];
                        if (cm->ref_frame_sign_bias[can_rf] != cm->ref_frame_sign_bias[rf[cmp_idx]]) {
                            this_mv.y = -this_mv.y;
                            this_mv.x = -this_mv.x;
                        }
                        ref_diff[cmp_idx][ref_diff_count[cmp_idx]] = this_mv;
                        ++ref_diff_count[cmp_idx];
                    }
                }
            }
            idx += mi_size_high[candidate_bsize];
        }

        // Build up the compound mv predictor
        Mv comp_list[MAX_MV_REF_CANDIDATES + 1][2];

        for (uint8_t idx = 0; idx < 2; ++idx) {
            uint8_t comp_idx = 0;
            for (uint8_t list_idx = 0; list_idx < ref_id_count[idx] && comp_idx < MAX_MV_REF_CANDIDATES;
                 ++list_idx, ++comp_idx) {
                comp_list[comp_idx][idx] = ref_id[idx][list_idx];
            }
            for (uint8_t list_idx = 0; list_idx < ref_diff_count[idx] && comp_idx < MAX_MV_REF_CANDIDATES;
                 ++list_idx, ++comp_idx) {
                comp_list[comp_idx][idx] = ref_diff[idx][list_idx];
            }
            for (; comp_idx < MAX_MV_REF_CANDIDATES; ++comp_idx) {
                comp_list[comp_idx][idx] = gm_mv_candidates[idx];
            }
        }

        //CHKN fill the stack, increment the counter
        if (*refmv_count) { //CHKN RefMvCount=1
            assert(*refmv_count == 1);
            if (comp_list[0][0].as_int == ref_mv_stack[0].this_mv.as_int &&
                comp_list[0][1].as_int == ref_mv_stack[0].comp_mv.as_int) {
                ref_mv_stack[*refmv_count].this_mv = comp_list[1][0];
                ref_mv_stack[*refmv_count].comp_mv = comp_list[1][1];
            } else {
                ref_mv_stack[*refmv_count].this_mv = comp_list[0][0];
                ref_mv_stack[*refmv_count].comp_mv = comp_list[0][1];
            }
            ref_mv_stack[*refmv_count].weight = 2;
            ++(*refmv_count);
        } else { //CHKN RefMvCount=0
            for (uint8_t idx = 0; idx < MAX_MV_REF_CANDIDATES; ++idx) {
                ref_mv_stack[*refmv_count].this_mv = comp_list[idx][0];
                ref_mv_stack[*refmv_count].comp_mv = comp_list[idx][1];
                ref_mv_stack[*refmv_count].weight  = 2;
                ++(*refmv_count);
            }
        }

        assert(*refmv_count >= 2);
    } else {
        // Handle single reference frame extension

        //CHKn if count is still < 2, re-scan ROW=-1 with less constraints.
        //     Order is already fixed. the added candidates are stored as we go at the bottom of the Stack.
        //CHKN TODO: confirm this could be avoided if we have already 2(DRL:OFF), or 4(DRL:ON) candidates
        for (int32_t idx = 0; ABS(max_row_offset) >= 1 && idx < mi_size && *refmv_count < MAX_MV_REF_CANDIDATES;) {
            const MbModeInfo* const candidate       = xd->mi[-xd->mi_stride + idx];
            const int32_t           candidate_bsize = candidate->bsize;

            for (int32_t rf_idx = 0; rf_idx < 2; ++rf_idx) {
                if (candidate->block_mi.ref_frame[rf_idx] > INTRA_FRAME) {
                    Mv this_mv = candidate->block_mi.mv[rf_idx];
                    if (cm->ref_frame_sign_bias[candidate->block_mi.ref_frame[rf_idx]] !=
                        cm->ref_frame_sign_bias[rf[0]]) {
                        this_mv.y = -this_mv.y;
                        this_mv.x = -this_mv.x;
                    }
                    int8_t stack_idx;
                    for (stack_idx = 0; stack_idx < *refmv_count; ++stack_idx) {
                        Mv stack_mv = ref_mv_stack[stack_idx].this_mv;
                        if (this_mv.as_int == stack_mv.as_int) {
                            break;
                        }
                    }

                    if (stack_idx == *refmv_count) {
                        ref_mv_stack[stack_idx].this_mv = this_mv;
                        ref_mv_stack[stack_idx].weight  = 2;
                        ++(*refmv_count);
                    }
                }
            }
            idx += mi_size_wide[candidate_bsize];
        }

        //CHKn if count is still < 2, re-scan COL=-1 with less constraints. the added candidates are stored as we go at the bottom of the Stack.
        for (int32_t idx = 0; ABS(max_col_offset) >= 1 && idx < mi_size && *refmv_count < MAX_MV_REF_CANDIDATES;) {
            const MbModeInfo* const candidate       = xd->mi[idx * xd->mi_stride - 1];
            const int32_t           candidate_bsize = candidate->bsize;

            for (uint8_t rf_idx = 0; rf_idx < 2; ++rf_idx) {
                if (candidate->block_mi.ref_frame[rf_idx] > INTRA_FRAME) {
                    Mv this_mv = candidate->block_mi.mv[rf_idx];
                    if (cm->ref_frame_sign_bias[candidate->block_mi.ref_frame[rf_idx]] !=
                        cm->ref_frame_sign_bias[rf[0]]) {
                        this_mv.y = -this_mv.y;
                        this_mv.x = -this_mv.x;
                    }
                    int8_t stack_idx;
                    for (stack_idx = 0; stack_idx < *refmv_count; ++stack_idx) {
                        Mv stack_mv = ref_mv_stack[stack_idx].this_mv;
                        if (this_mv.as_int == stack_mv.as_int) {
                            break;
                        }
                    }

                    if (stack_idx == *refmv_count) {
                        ref_mv_stack[stack_idx].this_mv = this_mv;
                        ref_mv_stack[stack_idx].weight  = 2;
                        ++(*refmv_count);
                    }
                }
            }
            idx += mi_size_high[candidate_bsize];
        }

        for (uint8_t idx = *refmv_count; idx < MAX_MV_REF_CANDIDATES; ++idx) {
            ref_mv_stack[idx].this_mv.as_int = gm_mv_candidates[0].as_int;
        }
    }
}

// Setup the MVP list for one ref frame
void setup_ref_mv_list(PictureControlSet* pcs, const Av1Common* cm, const MacroBlockD* xd, MvReferenceFrame ref_frame,
                       uint8_t* refmv_count, CandidateMv ref_mv_stack[MAX_REF_MV_STACK_SIZE], Mv* gm_mv_candidates,
                       const WarpedMotionParams* gm_params, int32_t mi_row, int32_t mi_col, ModeDecisionContext* ctx,
                       uint8_t symteric_refs, Mv* mv_ref0, int16_t* mode_context) {
    const int32_t         bs             = AOMMAX(xd->n8_w, xd->n8_h);
    const int32_t         has_tr         = has_top_right(pcs->scs->seq_header.sb_size, xd, mi_row, mi_col, bs);
    const TileInfo* const tile           = &xd->tile;
    int32_t               max_row_offset = 0, max_col_offset = 0;
    const int32_t         row_adj        = (xd->n8_h < mi_size_high[BLOCK_8X8]) && (mi_row & 0x01);
    const int32_t         col_adj        = (xd->n8_w < mi_size_wide[BLOCK_8X8]) && (mi_col & 0x01);
    int32_t               processed_rows = 0;
    int32_t               processed_cols = 0;

    MvReferenceFrame rf[2];
    av1_set_ref_frame(rf, ref_frame);
    *mode_context = 0;
    *refmv_count  = 0;

    // Find valid maximum row/col offset.
    if (xd->up_available) {
        max_row_offset = -(MVREF_ROWS << 1) + row_adj;

        if (xd->n8_h < mi_size_high[BLOCK_8X8]) {
            max_row_offset = -(2 << 1) + row_adj;
        }

        max_row_offset = find_valid_row_offset(tile, mi_row, max_row_offset);
    }

    if (xd->left_available) {
        max_col_offset = -(MVREF_COLS << 1) + col_adj;

        if (xd->n8_w < mi_size_wide[BLOCK_8X8]) {
            max_col_offset = -(2 << 1) + col_adj;
        }

        max_col_offset = find_valid_col_offset(tile, mi_col, max_col_offset);
    }

    uint8_t col_match_count = 0;
    uint8_t row_match_count = 0;
    uint8_t newmv_count     = 0;

    //CHKN-------------    ROW-1

    // Scan the first above row mode info. row_offset = -1;
    if (ABS(max_row_offset) >= 1) {
        scan_row_mbmi(cm,
                      xd,
                      mi_col,
                      rf,
                      -1,
                      ref_mv_stack,
                      refmv_count,
                      &row_match_count,
                      &newmv_count,
                      gm_mv_candidates,
                      gm_params,
                      max_row_offset,
                      &processed_rows);
    }

    //CHKN-------------    COL-1
    // Scan the first left column mode info. col_offset = -1;
    if (ABS(max_col_offset) >= 1) {
        scan_col_mbmi(cm,
                      xd,
                      mi_row,
                      rf,
                      -1,
                      ref_mv_stack,
                      refmv_count,
                      &col_match_count,
                      &newmv_count,
                      gm_mv_candidates,
                      gm_params,
                      max_col_offset,
                      &processed_cols);
    }

    //CHKN-------------    TOP-RIGHT

    // Check top-right boundary
    if (has_tr) {
        scan_blk_mbmi(xd,
                      mi_row,
                      mi_col,
                      rf,
                      -1,
                      xd->n8_w,
                      ref_mv_stack,
                      &row_match_count,
                      &newmv_count,
                      gm_mv_candidates,
                      gm_params,
                      refmv_count);
    }

    const uint8_t nearest_match = (row_match_count > 0) + (col_match_count > 0);

    for (int32_t idx = 0; idx < *refmv_count; ++idx) {
        ref_mv_stack[idx].weight += REF_CAT_LEVEL;
    }

    //CHKN  MFMV - get canididates from reference frames- orderHint has to be on, in order to scale the vectors.
    if (pcs->ppcs->frm_hdr.use_ref_frame_mvs) {
        int is_available = 0;

        int blk_row_end, blk_col_end, step_w, step_h, allow_extension;
        if (ctx->sb64_sq_no4xn_geom) {
            blk_row_end     = xd->n4_w;
            blk_col_end     = xd->n4_w;
            step_w          = (xd->n4_w >= MI_SIZE_W_64X64) ? MI_SIZE_W_16X16 : MI_SIZE_W_8X8;
            step_h          = step_w;
            allow_extension = (xd->n4_w >= MI_SIZE_W_8X8) && (xd->n4_w < MI_SIZE_W_64X64);
        } else {
            blk_row_end     = AOMMIN(xd->n4_h, mi_size_high[BLOCK_64X64]);
            blk_col_end     = AOMMIN(xd->n4_w, mi_size_wide[BLOCK_64X64]);
            allow_extension = (xd->n4_h >= mi_size_high[BLOCK_8X8]) && (xd->n4_h < mi_size_high[BLOCK_64X64]) &&
                (xd->n4_w >= mi_size_wide[BLOCK_8X8]) && (xd->n4_w < mi_size_wide[BLOCK_64X64]);
            step_h = (xd->n4_h >= mi_size_high[BLOCK_64X64]) ? mi_size_high[BLOCK_16X16] : mi_size_high[BLOCK_8X8];
            step_w = (xd->n4_w >= mi_size_wide[BLOCK_64X64]) ? mi_size_wide[BLOCK_16X16] : mi_size_high[BLOCK_8X8];
        }

        int     cur_offset_0;
        int     cur_offset_1 = 0;
        uint8_t list_idx0    = get_list_idx(rf[0]);
        uint8_t ref_idx_l0   = get_ref_frame_idx(rf[0]);

        const int cur_frame_index = pcs->ppcs->cur_order_hint;
        const int frame0_index =
            ((EbReferenceObject*)pcs->ref_pic_ptr_array[list_idx0][ref_idx_l0]->object_ptr)->order_hint;
        cur_offset_0 = get_relative_dist(&pcs->ppcs->scs->seq_header.order_hint_info, cur_frame_index, frame0_index);

        if (rf[1] != NONE_FRAME) {
            uint8_t   list_idx1  = get_list_idx(rf[1]);
            uint8_t   ref_idx_l1 = get_ref_frame_idx(rf[1]);
            const int frame1_index =
                ((EbReferenceObject*)pcs->ref_pic_ptr_array[list_idx1][ref_idx_l1]->object_ptr)->order_hint;
            cur_offset_1 = get_relative_dist(
                &pcs->ppcs->scs->seq_header.order_hint_info, cur_frame_index, frame1_index);
        }

        for (int blk_row = 0; blk_row < blk_row_end; blk_row += step_h) {
            for (int blk_col = 0; blk_col < blk_col_end; blk_col += step_w) {
                int ret = add_tpl_ref_mv(cm,
                                         pcs,
                                         xd,
                                         mi_row,
                                         mi_col,
                                         ref_frame,
                                         blk_row,
                                         blk_col,
                                         gm_mv_candidates,
                                         refmv_count,
                                         symteric_refs,
                                         mv_ref0,
                                         cur_offset_0,
                                         cur_offset_1,
                                         ref_mv_stack,
                                         mode_context);
                if (blk_row == 0 && blk_col == 0) {
                    is_available = ret;
                }

                mv_ref0++;
            }
        }

        if (is_available == 0) {
            *mode_context |= (1 << GLOBALMV_OFFSET);
        }

        if (allow_extension) {
            int voffset = ctx->sb64_sq_no4xn_geom ? xd->n4_h : AOMMAX(mi_size_high[BLOCK_8X8], xd->n4_h);
            int hoffset = ctx->sb64_sq_no4xn_geom ? xd->n4_h : AOMMAX(mi_size_wide[BLOCK_8X8], xd->n4_w);

            const int tpl_sample_pos[3][2] = {
                {voffset, -2},
                {voffset, hoffset},
                {voffset - 2, hoffset},
            };
            for (int i = 0; i < 3; ++i) {
                const int blk_row = tpl_sample_pos[i][0];
                const int blk_col = tpl_sample_pos[i][1];

                if (!check_sb_border(mi_row, mi_col, blk_row, blk_col)) {
                    continue;
                }
                add_tpl_ref_mv(cm,
                               pcs,
                               xd,
                               mi_row,
                               mi_col,
                               ref_frame,
                               blk_row,
                               blk_col,
                               gm_mv_candidates,
                               refmv_count,
                               symteric_refs,
                               mv_ref0,
                               cur_offset_0,
                               cur_offset_1,
                               ref_mv_stack,
                               mode_context);

                mv_ref0++;
            }
        }
    } // End temporal MVP

    //CHKN------------- TOP-LEFT
    uint8_t dummy_newmv_count = 0;

    // Scan the second outer area.
    scan_blk_mbmi(xd,
                  mi_row,
                  mi_col,
                  rf,
                  -1,
                  -1,
                  ref_mv_stack,
                  &row_match_count,
                  &dummy_newmv_count,
                  gm_mv_candidates,
                  gm_params,
                  refmv_count);

    //CHKN-------------    ROW-3  COL-3     ROW-5   COL-5
    for (int32_t idx = 2; idx <= MVREF_ROWS; ++idx) {
        const int32_t row_offset = -(idx << 1) + 1 + row_adj;
        const int32_t col_offset = -(idx << 1) + 1 + col_adj;

        if (ABS(row_offset) <= ABS(max_row_offset) && ABS(row_offset) > processed_rows) {
            scan_row_mbmi(cm,
                          xd,
                          mi_col,
                          rf,
                          row_offset,
                          ref_mv_stack,
                          refmv_count,
                          &row_match_count,
                          &dummy_newmv_count,
                          gm_mv_candidates,
                          gm_params,
                          max_row_offset,
                          &processed_rows);
        }

        if (ABS(col_offset) <= ABS(max_col_offset) && ABS(col_offset) > processed_cols) {
            scan_col_mbmi(cm,
                          xd,
                          mi_row,
                          rf,
                          col_offset,
                          ref_mv_stack,
                          refmv_count,
                          &col_match_count,
                          &dummy_newmv_count,
                          gm_mv_candidates,
                          gm_params,
                          max_col_offset,
                          &processed_cols);
        }
    }

    //---------- Mode Context Derivation based on 3 counters -------------
    const uint8_t ref_match_count = (row_match_count > 0) + (col_match_count > 0);

    switch (nearest_match) {
    case 0:
        if (ref_match_count >= 1) {
            *mode_context |= 1;
        }
        if (ref_match_count == 1) {
            *mode_context |= (1 << REFMV_OFFSET);
        } else if (ref_match_count >= 2) {
            *mode_context |= (2 << REFMV_OFFSET);
        }
        break;
    case 1:
        *mode_context |= (newmv_count > 0) ? 2 : 3;
        if (ref_match_count == 1) {
            *mode_context |= (3 << REFMV_OFFSET);
        } else if (ref_match_count >= 2) {
            *mode_context |= (4 << REFMV_OFFSET);
        }
        break;
    case 2:
    default:
        if (newmv_count >= 1) {
            *mode_context |= 4;
        } else {
            *mode_context |= 5;
        }

        *mode_context |= (5 << REFMV_OFFSET);
        break;
    }
    //---------- End Mode Context Derivation based on 3 counters -------------

    // Rank the likelihood and assign nearest and near mvs.
    if (*refmv_count > 1) {
        sort_mvp_table(ref_mv_stack, refmv_count);
    }

    //CHKN finish the Tables.  If table is not full, re-scan ROW-1 and COL-1
    if (*refmv_count < MAX_MV_REF_CANDIDATES) {
        scan_row_col_light(
            cm, xd, mi_row, mi_col, rf, ref_mv_stack, refmv_count, gm_mv_candidates, max_row_offset, max_col_offset);
    }

    // Clamp the final MVs
    for (uint8_t idx = 0; idx < *refmv_count; ++idx) {
        clamp_mv_ref(&ref_mv_stack[idx].this_mv, xd->n8_w << MI_SIZE_LOG2, xd->n8_h << MI_SIZE_LOG2, xd);

        if (rf[1] > NONE_FRAME) {
            clamp_mv_ref(&ref_mv_stack[idx].comp_mv, xd->n8_w << MI_SIZE_LOG2, xd->n8_h << MI_SIZE_LOG2, xd);
        }
    }
}

static INLINE int block_center_x(int mi_col, BlockSize bs) {
    const int bw = block_size_wide[bs];
    return mi_col * MI_SIZE + bw / 2 - 1;
}

static INLINE int block_center_y(int mi_row, BlockSize bs) {
    const int bh = block_size_high[bs];
    return mi_row * MI_SIZE + bh / 2 - 1;
}

Mv svt_aom_gm_get_motion_vector_enc(const WarpedMotionParams* gm, int32_t allow_hp, BlockSize bsize, int32_t mi_col,
                                    int32_t mi_row, int32_t is_integer) {
    Mv res;

    if (gm->wmtype == IDENTITY) {
        res.as_int = 0;
        return res;
    }

    if (gm->wmtype == TRANSLATION) {
        // All global motion vectors are stored with WARPEDMODEL_PREC_BITS (16)
        // bits of fractional precision. The offset for a translation is stored in
        // entries 0 and 1. For translations, all but the top three (two if
        // cm->allow_high_precision_mv is false) fractional bits are always zero.
        //
        // After the right shifts, there are 3 fractional bits of precision. If
        // allow_hp is false, the bottom bit is always zero (so we don't need a
        // call to convert_to_trans_prec here)
        //
        // Note: There is an AV1 specification bug here:
        //
        // gm->wmmat[0] is supposed to be the horizontal translation, and so should
        // go into res.as_mv.col, and gm->wmmat[1] is supposed to be the vertical
        // translation and so should go into res.as_mv.row
        //
        // However, in the spec, these assignments are accidentally reversed, and so
        // we must keep this incorrect logic to match the spec.
        //
        // See also: https://crbug.com/aomedia/3328
        res.y = gm->wmmat[0] >> GM_TRANS_ONLY_PREC_DIFF;
        res.x = gm->wmmat[1] >> GM_TRANS_ONLY_PREC_DIFF;
        assert(IMPLIES(1 & (res.y | res.x), allow_hp));
    } else {
        const int32_t* mat = gm->wmmat;
        const int      x   = block_center_x(mi_col, bsize);
        const int      y   = block_center_y(mi_row, bsize);

        assert(IMPLIES(gm->wmtype == ROTZOOM, gm->wmmat[5] == gm->wmmat[2]));
        assert(IMPLIES(gm->wmtype == ROTZOOM, gm->wmmat[4] == -gm->wmmat[3]));

        const int xc = (mat[2] - (1 << WARPEDMODEL_PREC_BITS)) * x + mat[3] * y + mat[0];
        const int yc = mat[4] * x + (mat[5] - (1 << WARPEDMODEL_PREC_BITS)) * y + mat[1];
        const int tx = convert_to_trans_prec(allow_hp, xc);
        const int ty = convert_to_trans_prec(allow_hp, yc);

        res.y = ty;
        res.x = tx;
    }

    if (is_integer) {
        integer_mv_precision(&res);
    }
    return res;
}

void svt_aom_init_xd(PictureControlSet* pcs, ModeDecisionContext* ctx) {
    TileInfo* tile = &ctx->sb_ptr->tile_info;

    int32_t       mi_row = ctx->blk_org_y >> MI_SIZE_LOG2;
    int32_t       mi_col = ctx->blk_org_x >> MI_SIZE_LOG2;
    Av1Common*    cm     = pcs->ppcs->av1_cm;
    MacroBlockD*  xd     = ctx->blk_ptr->av1xd;
    BlockSize     bsize  = ctx->blk_geom->bsize;
    const int32_t bw     = mi_size_wide[bsize];
    const int32_t bh     = mi_size_high[bsize];

    xd->n4_w = ctx->blk_geom->bwidth >> MI_SIZE_LOG2;
    xd->n4_h = ctx->blk_geom->bheight >> MI_SIZE_LOG2;

    xd->mi_row            = mi_row;
    xd->mi_col            = mi_col;
    xd->mb_to_top_edge    = -GET_MV_SUBPEL(mi_row * MI_SIZE);
    xd->mb_to_bottom_edge = GET_MV_SUBPEL((cm->mi_rows - bh - mi_row) * MI_SIZE);
    xd->mb_to_left_edge   = -GET_MV_SUBPEL(mi_col * MI_SIZE);
    xd->mb_to_right_edge  = GET_MV_SUBPEL((cm->mi_cols - bw - mi_col) * MI_SIZE);
    xd->up_available      = (mi_row > tile->mi_row_start);
    xd->left_available    = (mi_col > tile->mi_col_start);

    xd->n8_h        = bh;
    xd->n8_w        = bw;
    xd->is_sec_rect = 0;
    if (xd->n8_w < xd->n8_h) {
        // Only mark is_sec_rect as 1 for the last block.
        // For PARTITION_VERT_4, it would be (0, 0, 0, 1);
        // For other partitions, it would be (0, 1).
        if (!((mi_col + xd->n8_w) & (xd->n8_h - 1))) {
            xd->is_sec_rect = 1;
        }
    }

    if (xd->n8_w > xd->n8_h) {
        if (mi_row & (xd->n8_w - 1)) {
            xd->is_sec_rect = 1;
        }
    }

    xd->tile.mi_col_start = tile->mi_col_start;
    xd->tile.mi_col_end   = tile->mi_col_end;
    xd->tile.mi_row_start = tile->mi_row_start;
    xd->tile.mi_row_end   = tile->mi_row_end;

    xd->mi_stride        = pcs->mi_stride;
    const int32_t offset = mi_row * xd->mi_stride + mi_col;
    // mip offset may be different from grid offset when 4x4 blocks are disallowed
    const int32_t mip_offset = (mi_row >> (pcs->disallow_4x4_all_frames + pcs->disallow_8x8_all_frames)) *
            (xd->mi_stride >> (pcs->disallow_4x4_all_frames + pcs->disallow_8x8_all_frames)) +
        (mi_col >> (pcs->disallow_4x4_all_frames + pcs->disallow_8x8_all_frames));
    pcs->mi_grid_base[offset] = pcs->mip + mip_offset;
    xd->mi                    = pcs->mi_grid_base + offset;

    xd->above_mbmi = (xd->up_available) ? xd->mi[-(xd->mi_stride)] : NULL;
    xd->left_mbmi  = (xd->left_available) ? xd->mi[-1] : NULL;
    if (!ctx->skip_intra || ctx->inter_intra_comp_ctrls.enabled) {
        const uint8_t ss_x = 1, ss_y = 1;
        xd->chroma_up_available   = bh < 2 /*mi_size_wide[BLOCK_8X8]*/ ? (mi_row - 1) > xd->tile.mi_row_start
                                                                       : xd->up_available;
        xd->chroma_left_available = bw < 2 /*mi_size_high[BLOCK_8X8]*/ ? (mi_col - 1) > xd->tile.mi_col_start
                                                                       : xd->left_available;

        const int chroma_ref = ((mi_row & 0x01) || !(bh & 0x01)) && ((mi_col & 0x01) || !(bw & 0x01));

        // To help calculate the "above" and "left" chroma blocks, note that the
        // current block may cover multiple luma blocks (eg, if partitioned into
        // 4x4 luma blocks).
        // First, find the top-left-most luma block covered by this chroma block
        int32_t base_mbmi_offset = -(mi_row & ss_y) * xd->mi_stride - (mi_col & ss_x);

        // Then, we consider the luma region covered by the left or above 4x4 chroma
        // prediction. We want to point to the chroma reference block in that
        // region, which is the bottom-right-most mi unit.
        // This leads to the following offsets:
        xd->chroma_above_mbmi = (xd->chroma_up_available && chroma_ref)
            ? xd->mi[base_mbmi_offset - xd->mi_stride + ss_x]
            : NULL;

        xd->chroma_left_mbmi = (xd->chroma_left_available && chroma_ref)
            ? xd->mi[base_mbmi_offset + ss_y * xd->mi_stride - 1]
            : NULL;
    }
    xd->mi[0]->partition = from_shape_to_part[ctx->shape];
}

void svt_aom_generate_av1_mvp_table(ModeDecisionContext* ctx, BlkStruct* blk_ptr, const BlockGeom* blk_geom,
                                    uint16_t blk_org_x, uint16_t blk_org_y, MvReferenceFrame* ref_frames,
                                    uint32_t tot_refs, PictureControlSet* pcs) {
    int32_t      mi_row  = blk_org_y >> MI_SIZE_LOG2;
    int32_t      mi_col  = blk_org_x >> MI_SIZE_LOG2;
    Av1Common*   cm      = pcs->ppcs->av1_cm;
    FrameHeader* frm_hdr = &pcs->ppcs->frm_hdr;
    MacroBlockD* xd      = blk_ptr->av1xd;
    BlockSize    bsize   = blk_geom->bsize;

    uint8_t symteric_refs = 0;
    Mv      mv_ref0[64];
    if (pcs->temporal_layer_index > 0) {
        if (pcs->ppcs->scs->static_config.pred_structure == RANDOM_ACCESS) {
            if (tot_refs == 3 && ref_frames[0] == LAST_FRAME && ref_frames[1] == BWDREF_FRAME &&
                ref_frames[2] == LAST_BWD_FRAME) {
                symteric_refs = 1;
            }
        }
    }

    //128x128 OFF, 4xN OFF, SQ only

    uint32_t ref_it;
    for (ref_it = 0; ref_it < tot_refs; ++ref_it) {
        MvReferenceFrame ref_frame = ref_frames[ref_it];
        MvReferenceFrame rf[2];
        av1_set_ref_frame(rf, ref_frame);

        xd->ref_mv_count[ref_frame] = 0;
        svt_memset(ctx->ref_mv_stack[ref_frame], 0, sizeof(CandidateMv) * MAX_REF_MV_STACK_SIZE);

        Mv gm_mv[2];

        if (ref_frame == INTRA_FRAME) {
            gm_mv[0].as_int = gm_mv[1].as_int = 0;
        } else {
            if (ref_frame < REF_FRAMES) {
                gm_mv[0]        = svt_aom_gm_get_motion_vector_enc(&pcs->ppcs->global_motion[ref_frame],
                                                            frm_hdr->allow_high_precision_mv,
                                                            bsize,
                                                            mi_col,
                                                            mi_row,
                                                            frm_hdr->force_integer_mv);
                gm_mv[1].as_int = 0;
            } else {
                gm_mv[0] = svt_aom_gm_get_motion_vector_enc(&pcs->ppcs->global_motion[rf[0]],
                                                            frm_hdr->allow_high_precision_mv,
                                                            bsize,
                                                            mi_col,
                                                            mi_row,
                                                            frm_hdr->force_integer_mv);
                gm_mv[1] = svt_aom_gm_get_motion_vector_enc(&pcs->ppcs->global_motion[rf[1]],
                                                            frm_hdr->allow_high_precision_mv,
                                                            bsize,
                                                            mi_col,
                                                            mi_row,
                                                            frm_hdr->force_integer_mv);
            }
        }

        setup_ref_mv_list(pcs,
                          cm,
                          xd,
                          ref_frame,
                          &xd->ref_mv_count[ref_frame],
                          ctx->ref_mv_stack[ref_frame],
                          gm_mv,
                          pcs->ppcs->global_motion,
                          mi_row,
                          mi_col,
                          ctx,
                          symteric_refs,
                          mv_ref0,
                          &ctx->inter_mode_ctx[ref_frame]);
    }
}

void svt_aom_get_av1_mv_pred_drl(ModeDecisionContext* ctx, BlkStruct* blk_ptr, MvReferenceFrame ref_frame,
                                 uint8_t is_compound, PredictionMode mode,
                                 uint8_t drl_index, //valid value of drl_index
                                 Mv nearestmv[2], Mv nearmv[2], Mv ref_mv[2]) {
    MacroBlockD* xd = blk_ptr->av1xd;

    if (!is_compound && mode != GLOBALMV) {
        //av1_find_best_ref_mvs(allow_hp, ref_mvs[mbmi->ref_frame[0]], &nearestmv[0], &nearmv[0], cm->cur_frame_force_integer_mv);
        nearestmv[0] = ctx->ref_mv_stack[ref_frame][0].this_mv;
        nearmv[0]    = ctx->ref_mv_stack[ref_frame][1].this_mv;
    }

    if (is_compound && mode != GLOBAL_GLOBALMV) {
        int32_t ref_mv_idx = drl_index + 1;
        nearestmv[0]       = ctx->ref_mv_stack[ref_frame][0].this_mv;
        nearestmv[1]       = ctx->ref_mv_stack[ref_frame][0].comp_mv;
        nearmv[0]          = ctx->ref_mv_stack[ref_frame][ref_mv_idx].this_mv;
        nearmv[1]          = ctx->ref_mv_stack[ref_frame][ref_mv_idx].comp_mv;
    } else if (drl_index > 0 && mode == NEARMV) {
        assert((1 + drl_index) < MAX_REF_MV_STACK_SIZE);
        Mv cur_mv = ctx->ref_mv_stack[ref_frame][1 + drl_index].this_mv;
        nearmv[0] = cur_mv;
    }

    ref_mv[0] = nearestmv[0];
    ref_mv[1] = nearestmv[1];

    if (is_compound) {
        int32_t ref_mv_idx = drl_index;
        // Special case: NEAR_NEWMV and NEW_NEARMV modes use
        // 1 + mbmi->ref_mv_idx (like NEARMV) instead of
        // mbmi->ref_mv_idx (like NEWMV)
        if (mode == NEAR_NEWMV || mode == NEW_NEARMV) {
            ref_mv_idx = 1 + drl_index;
        }

        if (compound_ref0_mode(mode) == NEWMV) {
            ref_mv[0] = ctx->ref_mv_stack[ref_frame][ref_mv_idx].this_mv;
        }

        if (compound_ref1_mode(mode) == NEWMV) {
            ref_mv[1] = ctx->ref_mv_stack[ref_frame][ref_mv_idx].comp_mv;
        }
    } else {
        if (mode == NEWMV) {
            if (xd->ref_mv_count[ref_frame] > 1) {
                ref_mv[0] = ctx->ref_mv_stack[ref_frame][drl_index].this_mv;
            }
        }
    }
}

void svt_aom_update_mi_map_enc_dec(BlkStruct* blk_ptr, ModeDecisionContext* ctx, PictureControlSet* pcs) {
    // Update only the data in the top left block of the partition, because all other mi_blocks
    // point to the top left mi block of the partition
    MbModeInfo* mbmi         = blk_ptr->av1xd->mi[0];
    mbmi->block_mi.skip      = blk_ptr->block_has_coeff ? false : true;
    mbmi->block_mi.skip_mode = blk_ptr->block_mi.skip_mode;

    if (pcs->ppcs->frm_hdr.segmentation_params.segmentation_enabled) {
        int blk_org_x = ctx->blk_org_x;
        int blk_org_y = ctx->blk_org_y;
        if (!blk_ptr->block_has_coeff) {
            // predict and update segment id if current coding block has no ceoff
            int cdf_num;

            blk_ptr->segment_id = svt_av1_get_spatial_seg_prediction(
                pcs, blk_ptr->av1xd, blk_org_x, blk_org_y, &cdf_num);
        }
        // update segment id map so svt_av1_get_spatial_seg_prediction() can use the map to predict segment id.
        svt_av1_update_segmentation_map(pcs, ctx->blk_geom->bsize, blk_org_x, blk_org_y, blk_ptr->segment_id);
        mbmi->segment_id = blk_ptr->segment_id;
    }

    // update palette_colors mi map when input bit depth is 10bit and hbd mode decision is 0 (8bit MD)
    // palette_colors were scaled to 10bit in svt_aom_encode_decode so here we need to update mi map for entropy coding
    if (ctx->encoder_bit_depth > EB_EIGHT_BIT && ctx->hbd_md == 0) {
        if (mbmi->palette_mode_info.palette_size) {
            svt_memcpy(mbmi->palette_mode_info.palette_colors,
                       blk_ptr->palette_info->pmi.palette_colors,
                       sizeof(mbmi->palette_mode_info.palette_colors[0]) * PALETTE_MAX_SIZE);
        }
    }
}

void svt_copy_mi_map_grid_c(MbModeInfo** mi_grid_ptr, uint32_t mi_stride, uint8_t num_rows, uint8_t num_cols) {
    MbModeInfo* target = mi_grid_ptr[0];
    if (num_cols == 1) {
        for (uint8_t mi_y = 0; mi_y < num_rows; mi_y++) {
            const int32_t mi_idx = 0 + mi_y * mi_stride;
            // width is 1 block (corresponds to block width 4)
            mi_grid_ptr[mi_idx] = target;
        }
    } else if (num_cols == 2) {
        for (uint8_t mi_y = 0; mi_y < num_rows; mi_y++) {
            const int32_t mi_idx = 0 + mi_y * mi_stride;
            // width is 2 blocks, so can copy 2 at once (corresponds to block width 8)
            mi_grid_ptr[mi_idx]     = target;
            mi_grid_ptr[mi_idx + 1] = target;
        }
    } else {
        for (uint8_t mi_y = 0; mi_y < num_rows; mi_y++) {
            for (uint8_t mi_x = 0; mi_x < num_cols; mi_x += 4) {
                const int32_t mi_idx = mi_x + mi_y * mi_stride;
                // width is >=4 blocks, so can copy 4 at once; (corresponds to block width >=16).
                // All blocks >= 16 have widths that are divisible by 16, so it is ok to copy 4 blocks at once
                mi_grid_ptr[mi_idx]     = target;
                mi_grid_ptr[mi_idx + 1] = target;
                mi_grid_ptr[mi_idx + 2] = target;
                mi_grid_ptr[mi_idx + 3] = target;
            }
        }
    }
}

MbModeInfo* get_mbmi(PictureControlSet* pcs, uint32_t blk_org_x, uint32_t blk_org_y) {
    uint32_t mi_stride = pcs->mi_stride;
    int32_t  mi_row    = blk_org_y >> MI_SIZE_LOG2;
    int32_t  mi_col    = blk_org_x >> MI_SIZE_LOG2;

    const int32_t offset = mi_row * mi_stride + mi_col;

    // Reset the mi_grid (needs to be done here in case it was changed for NSQ blocks during MD - svt_aom_init_xd())
    // mip offset may be different from grid offset when 4x4 blocks are disallowed
    const int32_t mip_offset = (mi_row >> (pcs->disallow_4x4_all_frames + pcs->disallow_8x8_all_frames)) *
            (mi_stride >> (pcs->disallow_4x4_all_frames + pcs->disallow_8x8_all_frames)) +
        (mi_col >> (pcs->disallow_4x4_all_frames + pcs->disallow_8x8_all_frames));
    pcs->mi_grid_base[offset] = pcs->mip + mip_offset;

    MbModeInfo* mbmi = *(pcs->mi_grid_base + offset);

    return mbmi;
}

void svt_aom_update_mi_map(PictureControlSet* pcs, ModeDecisionContext* ctx, const PartitionType part,
                           const BlockSize bsize, const int mi_row, const int mi_col) {
    BlkStruct*     blk_ptr   = ctx->blk_ptr;
    const uint32_t mi_stride = pcs->mi_stride;
    const int      blk_org_y = mi_row << MI_SIZE_LOG2;
    const int      blk_org_x = mi_col << MI_SIZE_LOG2;
    const int      bwidth    = block_size_wide[bsize];
    const int      bheight   = block_size_high[bsize];

    const int32_t offset = mi_row * mi_stride + mi_col;

    // Reset the mi_grid (needs to be done here in case it was changed for NSQ blocks during MD - svt_aom_init_xd())
    // mip offset may be different from grid offset when 4x4 blocks are disallowed
    const int32_t mip_offset = (mi_row >> (pcs->disallow_4x4_all_frames + pcs->disallow_8x8_all_frames)) *
            (mi_stride >> (pcs->disallow_4x4_all_frames + pcs->disallow_8x8_all_frames)) +
        (mi_col >> (pcs->disallow_4x4_all_frames + pcs->disallow_8x8_all_frames));
    pcs->mi_grid_base[offset] = pcs->mip + mip_offset;

    MbModeInfo*    mbmi     = *(pcs->mi_grid_base + offset);
    BlockModeInfo* block_mi = &(mbmi->block_mi);

    // copy mbmi data
    svt_memcpy(block_mi, &blk_ptr->block_mi, sizeof(BlockModeInfo));

    if (svt_av1_allow_palette(pcs->ppcs->palette_level, bsize)) {
        mbmi->palette_mode_info.palette_size = blk_ptr->palette_size[0];
        svt_memcpy(mbmi->palette_mode_info.palette_colors,
                   blk_ptr->palette_info->pmi.palette_colors,
                   sizeof(mbmi->palette_mode_info.palette_colors[0]) * PALETTE_MAX_SIZE);
    } else {
        mbmi->palette_mode_info.palette_size = 0;
    }

    mbmi->bsize     = bsize;
    mbmi->partition = part;
    assert(IMPLIES(blk_ptr->block_mi.is_interintra_used, block_mi->ref_frame[1] == INTRA_FRAME));
    if (ctx->bypass_encdec && pcs->ppcs->frm_hdr.segmentation_params.segmentation_enabled) {
        if (!blk_ptr->block_has_coeff) {
            // predict and update segment id if current coding block has no ceoff
            int cdf_num;
            blk_ptr->segment_id = svt_av1_get_spatial_seg_prediction(
                pcs, blk_ptr->av1xd, blk_org_x, blk_org_y, &cdf_num);
        }
        // update segment id map so svt_av1_get_spatial_seg_prediction() can use the map to predict segment id.
        svt_av1_update_segmentation_map(pcs, bsize, blk_org_x, blk_org_y, blk_ptr->segment_id);
        mbmi->segment_id = blk_ptr->segment_id;
    }
    // The data copied into each mi block is the same; therefore, copy the data from the blk_ptr only for the first block_mi
    // then use change the mi block pointers of the remaining blocks ot point to the first block_mi. All data that
    // is used from block_mi should be updated above.
    svt_copy_mi_map_grid((pcs->mi_grid_base + offset), mi_stride, (bheight >> MI_SIZE_LOG2), (bwidth >> MI_SIZE_LOG2));
}

static INLINE void record_samples(MbModeInfo* mbmi, int* pts, int* pts_inref, int row_offset, int sign_r,
                                  int col_offset, int sign_c) {
    uint8_t bw = block_size_wide[mbmi->bsize];
    uint8_t bh = block_size_high[mbmi->bsize];
    int     x  = col_offset * MI_SIZE + sign_c * AOMMAX(bw, MI_SIZE) / 2 - 1;
    int     y  = row_offset * MI_SIZE + sign_r * AOMMAX(bh, MI_SIZE) / 2 - 1;

    pts[0]       = (x * 8);
    pts[1]       = (y * 8);
    pts_inref[0] = (x * 8) + mbmi->block_mi.mv[0].x;
    pts_inref[1] = (y * 8) + mbmi->block_mi.mv[0].y;
}

// Note: Samples returned are at 1/8-pel precision
// Sample are the neighbor block center point's coordinates relative to the
// left-top pixel of current block.
static uint8_t av1_find_samples(const Av1Common* cm, const BlockSize sb_size, MacroBlockD* xd, MvReferenceFrame rf0,
                                int* pts, int* pts_inref) {
    const int mi_row         = xd->mi_row;
    const int mi_col         = xd->mi_col;
    int       up_available   = xd->up_available;
    int       left_available = xd->left_available;
    int       i, mi_step = 1, np = 0;

    const TileInfo* const tile  = &xd->tile;
    int                   do_tl = 1;
    int                   do_tr = 1;

    // scan the nearest above rows
    if (up_available) {
        int         mi_row_offset = -1;
        MbModeInfo* mbmi          = xd->mi[mi_row_offset * xd->mi_stride];
        uint8_t     n4_w          = mi_size_wide[mbmi->bsize];

        if (xd->n4_w <= n4_w) {
            // Handle "current block width <= above block width" case.
            int col_offset = -mi_col % n4_w;

            if (col_offset < 0) {
                do_tl = 0;
            }
            if (col_offset + n4_w > xd->n4_w) {
                do_tr = 0;
            }

            if (mbmi->block_mi.ref_frame[0] == rf0 && mbmi->block_mi.ref_frame[1] == NONE_FRAME) {
                record_samples(mbmi, pts, pts_inref, 0, -1, col_offset, 1);
                pts += 2;
                pts_inref += 2;
                np++;
                if (np >= LEAST_SQUARES_SAMPLES_MAX) {
                    return LEAST_SQUARES_SAMPLES_MAX;
                }
            }
        } else {
            // Handle "current block width > above block width" case.
            for (i = 0; i < AOMMIN(xd->n4_w, cm->mi_cols - mi_col); i += mi_step) {
                int mi_col_offset = i;
                mbmi              = xd->mi[mi_col_offset + mi_row_offset * xd->mi_stride];
                n4_w              = mi_size_wide[mbmi->bsize];
                mi_step           = AOMMIN(xd->n4_w, n4_w);

                if (mbmi->block_mi.ref_frame[0] == rf0 && mbmi->block_mi.ref_frame[1] == NONE_FRAME) {
                    record_samples(mbmi, pts, pts_inref, 0, -1, i, 1);
                    pts += 2;
                    pts_inref += 2;
                    np++;
                    if (np >= LEAST_SQUARES_SAMPLES_MAX) {
                        return LEAST_SQUARES_SAMPLES_MAX;
                    }
                }
            }
        }
    }

    // scan the nearest left columns
    if (left_available) {
        int         mi_col_offset = -1;
        MbModeInfo* mbmi          = xd->mi[mi_col_offset];
        uint8_t     n4_h          = mi_size_high[mbmi->bsize];

        if (xd->n4_h <= n4_h) {
            // Handle "current block height <= above block height" case.
            int row_offset = -mi_row % n4_h;
            if (row_offset < 0) {
                do_tl = 0;
            }

            if (mbmi->block_mi.ref_frame[0] == rf0 && mbmi->block_mi.ref_frame[1] == NONE_FRAME) {
                record_samples(mbmi, pts, pts_inref, row_offset, 1, 0, -1);
                pts += 2;
                pts_inref += 2;
                np++;
                if (np >= LEAST_SQUARES_SAMPLES_MAX) {
                    return LEAST_SQUARES_SAMPLES_MAX;
                }
            }
        } else {
            // Handle "current block height > above block height" case.
            for (i = 0; i < AOMMIN(xd->n4_h, cm->mi_rows - mi_row); i += mi_step) {
                int mi_row_offset = i;
                mbmi              = xd->mi[mi_col_offset + mi_row_offset * xd->mi_stride];
                n4_h              = mi_size_high[mbmi->bsize];
                mi_step           = AOMMIN(xd->n4_h, n4_h);

                if (mbmi->block_mi.ref_frame[0] == rf0 && mbmi->block_mi.ref_frame[1] == NONE_FRAME) {
                    record_samples(mbmi, pts, pts_inref, i, 1, 0, -1);
                    pts += 2;
                    pts_inref += 2;
                    np++;
                    if (np >= LEAST_SQUARES_SAMPLES_MAX) {
                        return LEAST_SQUARES_SAMPLES_MAX;
                    }
                }
            }
        }
    }

    // Top-left block
    if (do_tl && left_available && up_available) {
        int         mi_row_offset = -1;
        int         mi_col_offset = -1;
        MbModeInfo* mbmi          = xd->mi[mi_col_offset + mi_row_offset * xd->mi_stride];

        if (mbmi->block_mi.ref_frame[0] == rf0 && mbmi->block_mi.ref_frame[1] == NONE_FRAME) {
            record_samples(mbmi, pts, pts_inref, 0, -1, 0, -1);
            pts += 2;
            pts_inref += 2;
            np++;
            if (np >= LEAST_SQUARES_SAMPLES_MAX) {
                return LEAST_SQUARES_SAMPLES_MAX;
            }
        }
    }

    // Top-right block
    if (do_tr && has_top_right(sb_size, xd, mi_row, mi_col, AOMMAX(xd->n4_w, xd->n4_h))) {
        Pos trb_pos = {-1, xd->n4_w};

        if (is_inside(tile, mi_col, mi_row, &trb_pos)) {
            int mi_row_offset = -1;
            int mi_col_offset = xd->n4_w;

            MbModeInfo* mbmi = xd->mi[mi_col_offset + mi_row_offset * xd->mi_stride];

            if (mbmi->block_mi.ref_frame[0] == rf0 && mbmi->block_mi.ref_frame[1] == NONE_FRAME) {
                record_samples(mbmi, pts, pts_inref, 0, -1, xd->n4_w, 1);
                np++;
                if (np >= LEAST_SQUARES_SAMPLES_MAX) {
                    return LEAST_SQUARES_SAMPLES_MAX;
                }
            }
        }
    }

    return np;
}

void svt_aom_init_wm_samples(PictureControlSet* pcs, ModeDecisionContext* ctx) {
    if (pcs->ppcs->frm_hdr.allow_warped_motion && is_motion_variation_allowed_bsize(ctx->blk_geom->bsize) &&
        has_overlappable_candidates(ctx->blk_ptr)) {
        for (uint32_t ref_it = 0; ref_it < ctx->tot_ref_frame_types; ++ref_it) {
            MvReferenceFrame ref_pair = ctx->ref_frame_type_arr[ref_it];
            MvReferenceFrame rf[2];
            av1_set_ref_frame(rf, ref_pair);
            //single ref/list
            if (rf[1] == NONE_FRAME) {
                ctx->wm_sample_info[rf[0]].num = av1_find_samples(pcs->ppcs->av1_cm,
                                                                  pcs->scs->seq_header.sb_size,
                                                                  ctx->blk_ptr->av1xd,
                                                                  rf[0],
                                                                  ctx->wm_sample_info[rf[0]].pts,
                                                                  ctx->wm_sample_info[rf[0]].pts_inref);
            }
        }
    } else {
        for (int i = 0; i < REF_FRAMES; i++) {
            ctx->wm_sample_info[i].num = 0;
        }
    }
}

bool svt_aom_warped_motion_parameters(ModeDecisionContext* ctx, const Mv mv, const BlockGeom* blk_geom,
                                      const MvReferenceFrame ref_frame, WarpedMotionParams* wm_params,
                                      uint8_t* num_samples, uint16_t lower_band_th, uint16_t upper_band_th,
                                      bool shut_approx) {
    BlkStruct*   blk_ptr  = ctx->blk_ptr;
    MacroBlockD* xd       = blk_ptr->av1xd;
    BlockSize    bsize    = blk_geom->bsize;
    bool         apply_wm = false;
    int          pts[SAMPLES_ARRAY_SIZE], pts_inref[SAMPLES_ARRAY_SIZE];

    const int mi_row = xd->mi_row;
    const int mi_col = xd->mi_col;

    *num_samples = 0;
    if (blk_geom->bwidth < 8 || blk_geom->bheight < 8) {
        return apply_wm;
    }

    assert(ref_frame < TOTAL_REFS_PER_FRAME);
    // samples are precomputed in svt_aom_init_wm_samples to avoid the following call
    // for each search:
    //uint8_t nsamples = av1_find_samples(pcs->ppcs->av1_cm,
    //                                    pcs->scs->seq_header.sb_size,
    //                                    blk_ptr->av1xd,
    //                                    rf[0],
    //                                    pts,
    //                                    pts_inref);
    uint8_t nsamples = ctx->wm_sample_info[ref_frame].num;
    memcpy(pts, ctx->wm_sample_info[ref_frame].pts, nsamples * sizeof(pts[0]) * 2);
    memcpy(pts_inref, ctx->wm_sample_info[ref_frame].pts_inref, nsamples * sizeof(pts_inref[0]) * 2);
    if (nsamples == 0) {
        return apply_wm;
    }
    if (nsamples > 1) {
        nsamples = svt_aom_select_samples(mv, pts, pts_inref, nsamples, bsize);
    }
    *num_samples = nsamples;

    apply_wm = !svt_find_projection((int)nsamples, pts, pts_inref, bsize, mv, wm_params, (int)mi_row, (int)mi_col);

    if (apply_wm && !shut_approx) {
        if ((abs(wm_params->alpha) + abs(wm_params->beta)) < lower_band_th &&
            (abs(wm_params->gamma) + abs(wm_params->delta)) < lower_band_th) {
            apply_wm = 0;
        }
        if ((4 * abs(wm_params->alpha) + 7 * abs(wm_params->beta) > upper_band_th) &&
            (4 * abs(wm_params->gamma) + 4 * abs(wm_params->delta) > upper_band_th)) {
            apply_wm = 0;
        }
    }
    return apply_wm;
}

//foreach_overlappable_nb_above
static uint32_t count_overlappable_nb_above(const Av1Common* cm, MacroBlockD* xd, int32_t mi_col, uint32_t nb_max) {
    uint32_t nb_count = 0;
    if (!xd->up_available) {
        return nb_count;
    }

    // prev_row_mi points into the mi array, starting at the beginning of the
    // previous row.
    MbModeInfo** prev_row_mi = xd->mi - mi_col - 1 * xd->mi_stride;
    const int    end_col     = MIN(mi_col + xd->n4_w, cm->mi_cols);
    uint8_t      mi_step;

    for (int above_mi_col = mi_col; above_mi_col < end_col && nb_count < nb_max; above_mi_col += mi_step) {
        MbModeInfo** above_mi = prev_row_mi + above_mi_col;
        mi_step               = MIN(mi_size_wide[above_mi[0]->bsize], mi_size_wide[BLOCK_64X64]);

        // If we're considering a block with width 4, it should be treated as
        // half of a pair of blocks with chroma information in the second. Move
        // above_mi_col back to the start of the pair if needed, set above_mbmi
        // to point at the block with chroma information, and set mi_step to 2 to
        // step over the entire pair at the end of the iteration.
        if (mi_step == 1) {
            above_mi_col &= ~1;
            above_mi = prev_row_mi + above_mi_col + 1;
            mi_step  = 2;
        }
        if (is_neighbor_overlappable(*above_mi)) {
            ++nb_count;
        }
    }

    return nb_count;
}

static uint32_t count_overlappable_nb_left(const Av1Common* cm, MacroBlockD* xd, int32_t mi_row, uint32_t nb_max) {
    uint32_t nb_count = 0;
    if (!xd->left_available) {
        return nb_count;
    }

    // prev_col_mi points into the mi array, starting at the top of the
    // previous column
    MbModeInfo** prev_col_mi = xd->mi - 1 - mi_row * xd->mi_stride;
    const int    end_row     = MIN(mi_row + xd->n4_h, cm->mi_rows);
    uint8_t      mi_step;

    for (int left_mi_row = mi_row; left_mi_row < end_row && nb_count < nb_max; left_mi_row += mi_step) {
        MbModeInfo** left_mi = prev_col_mi + left_mi_row * xd->mi_stride;
        mi_step              = MIN(mi_size_high[left_mi[0]->bsize], mi_size_high[BLOCK_64X64]);
        if (mi_step == 1) {
            left_mi_row &= ~1;
            left_mi = prev_col_mi + (left_mi_row + 1) * xd->mi_stride;
            mi_step = 2;
        }

        if (is_neighbor_overlappable(*left_mi)) {
            ++nb_count;
        }
    }

    return nb_count;
}

void svt_av1_count_overlappable_neighbors(const PictureControlSet* pcs, BlkStruct* blk_ptr, const BlockSize bsize,
                                          int32_t mi_row, int32_t mi_col) {
    Av1Common*   cm                 = pcs->ppcs->av1_cm;
    MacroBlockD* xd                 = blk_ptr->av1xd;
    blk_ptr->overlappable_neighbors = 0;

    if (!is_motion_variation_allowed_bsize(bsize)) {
        return;
    }

    blk_ptr->overlappable_neighbors = count_overlappable_nb_above(cm, xd, mi_col, UINT32_MAX);

    blk_ptr->overlappable_neighbors += count_overlappable_nb_left(cm, xd, mi_row, UINT32_MAX);
}

int svt_aom_is_dv_valid(const Mv dv, const MacroBlockD* xd, int mi_row, int mi_col, BlockSize bsize,
                        int mib_size_log2) {
    const int bw             = block_size_wide[bsize];
    const int bh             = block_size_high[bsize];
    const int scale_px_to_mv = 8;
    // Disallow subpixel for now
    // SUBPEL_MASK is not the correct scale
    if (((dv.y & (scale_px_to_mv - 1)) || (dv.x & (scale_px_to_mv - 1)))) {
        return 0;
    }

    const TileInfo* const tile = &xd->tile;
    // Is the source top-left inside the current tile?
    const int src_top_edge  = mi_row * MI_SIZE * scale_px_to_mv + dv.y;
    const int tile_top_edge = tile->mi_row_start * MI_SIZE * scale_px_to_mv;
    if (src_top_edge < tile_top_edge) {
        return 0;
    }
    const int src_left_edge  = mi_col * MI_SIZE * scale_px_to_mv + dv.x;
    const int tile_left_edge = tile->mi_col_start * MI_SIZE * scale_px_to_mv;
    if (src_left_edge < tile_left_edge) {
        return 0;
    }
    // Is the bottom right inside the current tile?
    const int src_bottom_edge  = (mi_row * MI_SIZE + bh) * scale_px_to_mv + dv.y;
    const int tile_bottom_edge = tile->mi_row_end * MI_SIZE * scale_px_to_mv;
    if (src_bottom_edge > tile_bottom_edge) {
        return 0;
    }
    const int src_right_edge  = (mi_col * MI_SIZE + bw) * scale_px_to_mv + dv.x;
    const int tile_right_edge = tile->mi_col_end * MI_SIZE * scale_px_to_mv;
    if (src_right_edge > tile_right_edge) {
        return 0;
    }

    // Special case for sub 8x8 chroma cases, to prevent referring to chroma
    // pixels outside current tile.
    for (int plane = 1; plane < 3 /* av1_num_planes(cm)*/; ++plane) {
        //const struct MacroBlockDPlane *const pd = &xd->plane[plane];

        if (is_chroma_reference(mi_row, mi_col, bsize, 1, 1/* pd->subsampling_x,
            pd->subsampling_y*/)) {
            if (bw < 8 /*&& pd->subsampling_x*/) {
                if (src_left_edge < tile_left_edge + 4 * scale_px_to_mv) {
                    return 0;
                }
            }
            if (bh < 8 /* && pd->subsampling_y*/) {
                if (src_top_edge < tile_top_edge + 4 * scale_px_to_mv) {
                    return 0;
                }
            }
        }
    }

    // Is the bottom right within an already coded SB? Also consider additional
    // constraints to facilitate HW decoder.
    const int max_mib_size       = 1 << mib_size_log2;
    const int active_sb_row      = mi_row >> mib_size_log2;
    const int active_sb64_col    = (mi_col * MI_SIZE) >> 6;
    const int sb_size            = max_mib_size * MI_SIZE;
    const int src_sb_row         = ((src_bottom_edge >> 3) - 1) / sb_size;
    const int src_sb64_col       = ((src_right_edge >> 3) - 1) >> 6;
    const int total_sb64_per_row = ((tile->mi_col_end - tile->mi_col_start - 1) >> 4) + 1;
    const int active_sb64        = active_sb_row * total_sb64_per_row + active_sb64_col;
    const int src_sb64           = src_sb_row * total_sb64_per_row + src_sb64_col;
    if (src_sb64 >= active_sb64 - INTRABC_DELAY_SB64) {
        return 0;
    }

    // Wavefront constraint: use only top left area of frame for reference.
    const int gradient  = 1 + INTRABC_DELAY_SB64 + (sb_size > 64);
    const int wf_offset = gradient * (active_sb_row - src_sb_row);
    if (src_sb_row > active_sb_row || src_sb64_col >= active_sb64_col - INTRABC_DELAY_SB64 + wf_offset) {
        return 0;
    }

    //add a SW-Wavefront constraint
    if (sb_size == 64) {
        if (src_sb64_col > active_sb64_col + (active_sb_row - src_sb_row)) {
            return 0;
        }
    } else {
        const int src_sb128_col    = ((src_right_edge >> 3) - 1) >> 7;
        const int active_sb128_col = (mi_col * MI_SIZE) >> 7;

        if (src_sb128_col > active_sb128_col + (active_sb_row - src_sb_row)) {
            return 0;
        }
    }

    return 1;
}

Mv svt_av1_get_ref_mv_from_stack(int ref_idx, const MvReferenceFrame* ref_frame, int ref_mv_idx,
                                 CandidateMv ref_mv_stack[][MAX_REF_MV_STACK_SIZE], MacroBlockD* xd
                                 /*const MB_MODE_INFO_EXT *mbmi_ext*/) {
    const int8_t       ref_frame_type = av1_ref_frame_type(ref_frame);
    const CandidateMv* curr_ref_mv_stack =
        /*mbmi_ext->*/ ref_mv_stack[ref_frame_type];
    Mv ref_mv;
    ref_mv.as_int = INVALID_MV;

    if (ref_frame[1] > INTRA_FRAME) {
        if (ref_idx == 0) {
            ref_mv = curr_ref_mv_stack[ref_mv_idx].this_mv;
        } else {
            assert(ref_idx == 1);
            ref_mv = curr_ref_mv_stack[ref_mv_idx].comp_mv;
        }
    } else {
        assert(ref_idx == 0);
        if (ref_mv_idx < /*mbmi_ext->*/ xd->ref_mv_count[ref_frame_type]) {
            ref_mv = curr_ref_mv_stack[ref_mv_idx].this_mv;
        } else {
            //CHKN got this from decoder read_intrabc_info global_mvs[ref_frame].as_int = INVALID_MV;
            ref_mv.as_int = INVALID_MV; // mbmi_ext->global_mvs[ref_frame_type];
        }
    }
    return ref_mv;
}

void svt_av1_find_best_ref_mvs_from_stack(int allow_hp,
                                          //const MB_MODE_INFO_EXT *mbmi_ext,
                                          CandidateMv ref_mv_stack[][MAX_REF_MV_STACK_SIZE], MacroBlockD* xd,
                                          MvReferenceFrame ref_frame, Mv* nearest_mv, Mv* near_mv, int is_integer) {
    const int        ref_idx       = 0;
    MvReferenceFrame ref_frames[2] = {ref_frame, NONE_FRAME};
    *nearest_mv = svt_av1_get_ref_mv_from_stack(ref_idx, ref_frames, 0, ref_mv_stack /*mbmi_ext*/, xd);
    lower_mv_precision(nearest_mv, allow_hp, is_integer);
    *near_mv = svt_av1_get_ref_mv_from_stack(ref_idx, ref_frames, 1, ref_mv_stack /*mbmi_ext*/, xd);
    lower_mv_precision(near_mv, allow_hp, is_integer);
}
