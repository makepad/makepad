/*
 * Copyright (c) 2016, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
 */

#include <limits.h>
#include <math.h>
#include <stdio.h>
#include "av1me.h"
#include "mcomp.h"
#include "utility.h"
#include "pcs.h"
#include "sequence_control_set.h"
#include "aom_dsp_rtcd.h"
#include "md_process.h"
#include "adaptive_mv_pred.h"

AomVarianceFnPtr svt_aom_mefn_ptr[BLOCK_SIZES_ALL];

void init_fn_ptr(void) {
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
#define BFP0(w, h)                                                                     \
    svt_aom_mefn_ptr[BLOCK_##w##X##h].sdf       = svt_aom_sad##w##x##h;                \
    svt_aom_mefn_ptr[BLOCK_##w##X##h].vf        = svt_aom_variance##w##x##h;           \
    svt_aom_mefn_ptr[BLOCK_##w##X##h].vf_hbd_10 = svt_aom_highbd_10_variance##w##x##h; \
    svt_aom_mefn_ptr[BLOCK_##w##X##h].svf       = svt_aom_sub_pixel_variance##w##x##h; \
    svt_aom_mefn_ptr[BLOCK_##w##X##h].sdx4df    = svt_aom_sad##w##x##h##x4d;
#else
#define BFP0(w, h)                                                                  \
    svt_aom_mefn_ptr[BLOCK_##w##X##h].sdf    = svt_aom_sad##w##x##h;                \
    svt_aom_mefn_ptr[BLOCK_##w##X##h].vf     = svt_aom_variance##w##x##h;           \
    svt_aom_mefn_ptr[BLOCK_##w##X##h].svf    = svt_aom_sub_pixel_variance##w##x##h; \
    svt_aom_mefn_ptr[BLOCK_##w##X##h].sdx4df = svt_aom_sad##w##x##h##x4d;
#endif
    BFP0(4, 16)
    BFP0(16, 4)
    BFP0(8, 32)
    BFP0(32, 8)
    BFP0(16, 64)
    BFP0(64, 16)
    BFP0(128, 128)
    BFP0(128, 64)
    BFP0(64, 128)
    BFP0(32, 16)
    BFP0(16, 32)
    BFP0(64, 32)
    BFP0(32, 64)
    BFP0(32, 32)
    BFP0(64, 64)
    BFP0(16, 16)
    BFP0(16, 8)
    BFP0(8, 16)
    BFP0(8, 8)
    BFP0(8, 4)
    BFP0(4, 8)
    BFP0(4, 4)
#if CONFIG_ENABLE_OBMC
#define OBFP(w, h)                                                           \
    svt_aom_mefn_ptr[BLOCK_##w##X##h].osdf = svt_aom_obmc_sad##w##x##h;      \
    svt_aom_mefn_ptr[BLOCK_##w##X##h].ovf  = svt_aom_obmc_variance##w##x##h; \
    svt_aom_mefn_ptr[BLOCK_##w##X##h].osvf = svt_aom_obmc_sub_pixel_variance##w##x##h;
    OBFP(128, 128)
    OBFP(128, 64)
    OBFP(64, 128)
    OBFP(64, 64)
    OBFP(64, 32)
    OBFP(32, 64)
    OBFP(32, 32)
    OBFP(32, 16)
    OBFP(16, 32)
    OBFP(16, 16)
    OBFP(16, 8)
    OBFP(8, 16)
    OBFP(8, 8)
    OBFP(4, 8)
    OBFP(8, 4)
    OBFP(4, 4)
    OBFP(4, 16)
    OBFP(16, 4)
    OBFP(8, 32)
    OBFP(32, 8)
    OBFP(16, 64)
    OBFP(64, 16)
#endif
}

static INLINE const uint8_t* get_buf_from_mv(const Buf2D* buf, const Mv* mv) {
    return &buf->buf[mv->y * buf->stride + mv->x];
}

void svt_av1_set_mv_search_range(MvLimits* mv_limits, const Mv* mv) {
    int col_min = (mv->x >> 3) - MAX_FULL_PEL_VAL + !!(mv->x & 7);
    int row_min = (mv->y >> 3) - MAX_FULL_PEL_VAL + !!(mv->y & 7);
    int col_max = (mv->x >> 3) + MAX_FULL_PEL_VAL;
    int row_max = (mv->y >> 3) + MAX_FULL_PEL_VAL;

    col_min = AOMMAX(col_min, (MV_LOW >> 3) + 1);
    row_min = AOMMAX(row_min, (MV_LOW >> 3) + 1);
    col_max = AOMMIN(col_max, (MV_UPP >> 3) - 1);
    row_max = AOMMIN(row_max, (MV_UPP >> 3) - 1);

    // Get intersection of UMV window and valid MV window to reduce # of checks
    // in diamond search.
    if (mv_limits->col_min < col_min) {
        mv_limits->col_min = col_min;
    }
    if (mv_limits->col_max > col_max) {
        mv_limits->col_max = col_max;
    }
    if (mv_limits->row_min < row_min) {
        mv_limits->row_min = row_min;
    }
    if (mv_limits->row_max > row_max) {
        mv_limits->row_max = row_max;
    }
}

#define PIXEL_TRANSFORM_ERROR_SCALE 4

int svt_aom_mv_err_cost_light(const Mv* mv, const Mv* ref) {
    const uint32_t factor     = 50;
    const uint32_t absmvdiffx = ABS(mv->x - ref->x);
    const uint32_t absmvdiffy = ABS(mv->y - ref->y);
    const uint32_t mv_rate    = 1296 + (factor * (absmvdiffx + absmvdiffy));
    return mv_rate;
}

static int mvsad_err_cost_light(const Mv* mv, const Mv* ref) {
    const uint32_t factor     = 50;
    const uint32_t absmvdiffx = ABS(mv->x - ref->x) * 8;
    const uint32_t absmvdiffy = ABS(mv->y - ref->y) * 8;
    const uint32_t mv_rate    = 1296 + (factor * (absmvdiffx + absmvdiffy));
    return mv_rate;
}

int svt_aom_mv_err_cost(const Mv* mv, const Mv* ref, const int* mvjcost, const int* mvcost[2], int error_per_bit) {
    if (mvcost) {
        const Mv diff = (Mv){{mv->x - ref->x, mv->y - ref->y}};
        return (int)ROUND_POWER_OF_TWO_64(
            (int64_t)svt_mv_cost(&diff, mvjcost, mvcost) * error_per_bit,
            RDDIV_BITS + AV1_PROB_COST_SHIFT - RD_EPB_SHIFT + PIXEL_TRANSFORM_ERROR_SCALE);
    }
    return 0;
}

static int mvsad_err_cost(const IntraBcContext* x, const Mv* mv, const Mv* ref, int sad_per_bit) {
    if (x->approx_inter_rate) {
        return mvsad_err_cost_light(mv, ref);
    }
    const Mv diff = (Mv){{(mv->x - ref->x) * 8, (mv->y - ref->y) * 8}};
    return ROUND_POWER_OF_TWO(
        (unsigned)svt_mv_cost(&diff, x->nmv_vec_cost, (const int* const*)x->mv_cost_stack) * sad_per_bit,
        AV1_PROB_COST_SHIFT);
}

void svt_av1_init3smotion_compensation(SearchSiteConfig* cfg, int stride) {
    int len, ss_count = 1;

    cfg->ss[0].mv.as_int = 0;
    cfg->ss[0].offset    = 0;

    for (len = MAX_FIRST_STEP; len > 0; len /= 2) {
        // Generate offsets for 8 search sites per step.
        const Mv ss_mvs[8] = {{{0, -len}},
                              {{0, len}},
                              {{-len, 0}},
                              {{len, 0}},
                              {{-len, -len}},
                              {{len, -len}},
                              {{-len, len}},
                              {{len, len}}};
        int      i;
        for (i = 0; i < 8; ++i) {
            SearchSite* const ss = &cfg->ss[ss_count++];
            ss->mv               = ss_mvs[i];
            ss->offset           = ss->mv.y * stride + ss->mv.x;
        }
    }

    cfg->ss_count          = ss_count;
    cfg->searches_per_step = 8;
}

static INLINE int is_mv_in(const MvLimits* mv_limits, const Mv* mv) {
    return (mv->x >= mv_limits->col_min) && (mv->x <= mv_limits->col_max) && (mv->y >= mv_limits->row_min) &&
        (mv->y <= mv_limits->row_max);
}

int svt_av1_get_mvpred_var(const IntraBcContext* x, const Mv* best_mv, const Mv* center_mv, const AomVarianceFnPtr* vfp,
                           int use_mvcost) {
    const Buf2D* const what    = &x->plane[0].src;
    const Buf2D* const in_what = &x->xdplane[0].pre[0];
    const Mv           mv      = {{best_mv->x * 8, best_mv->y * 8}};
    unsigned int       unused;
    if (x->approx_inter_rate) {
        return vfp->vf(what->buf, what->stride, get_buf_from_mv(in_what, best_mv), in_what->stride, &unused) +
            (use_mvcost ? svt_aom_mv_err_cost_light(&mv, center_mv) : 0);
    } else {
        return vfp->vf(what->buf, what->stride, get_buf_from_mv(in_what, best_mv), in_what->stride, &unused) +
            (use_mvcost ? svt_aom_mv_err_cost(&mv, center_mv, x->nmv_vec_cost, x->mv_cost_stack, x->errorperbit) : 0);
    }
}

// Exhaustive motion search around a given centre position with a given
// step size.
static int exhaustive_mesh_search(IntraBcContext* x, Mv* ref_mv, Mv* best_mv, int range, int step, int sad_per_bit,
                                  const AomVarianceFnPtr* fn_ptr, const Mv* center_mv) {
    const Buf2D* const what       = &x->plane[0].src;
    const Buf2D* const in_what    = &x->xdplane[0].pre[0];
    Mv                 fcenter_mv = {.as_int = center_mv->as_int};
    unsigned int       best_sad   = INT_MAX;
    int                r, c, i;
    int                start_col, end_col, start_row, end_row;
    int                col_step = (step > 1) ? step : 4;

    assert(step >= 1);

    clamp_mv(&fcenter_mv, x->mv_limits.col_min, x->mv_limits.col_max, x->mv_limits.row_min, x->mv_limits.row_max);
    *best_mv = fcenter_mv;
    best_sad = fn_ptr->sdf(what->buf, what->stride, get_buf_from_mv(in_what, &fcenter_mv), in_what->stride) +
        mvsad_err_cost(x, &fcenter_mv, ref_mv, sad_per_bit);
    start_row = AOMMAX(-range, x->mv_limits.row_min - fcenter_mv.y);
    start_col = AOMMAX(-range, x->mv_limits.col_min - fcenter_mv.x);
    end_row   = AOMMIN(range, x->mv_limits.row_max - fcenter_mv.y);
    end_col   = AOMMIN(range, x->mv_limits.col_max - fcenter_mv.x);

    for (r = start_row; r <= end_row; r += step) {
        for (c = start_col; c <= end_col; c += col_step) {
            // Step > 1 means we are not checking every location in this pass.
            if (step > 1) {
                const Mv     mv  = {{fcenter_mv.x + c, fcenter_mv.y + r}};
                unsigned int sad = fn_ptr->sdf(what->buf, what->stride, get_buf_from_mv(in_what, &mv), in_what->stride);
                if (sad < best_sad) {
                    sad += mvsad_err_cost(x, &mv, ref_mv, sad_per_bit);
                    if (sad < best_sad) {
                        best_sad          = sad;
                        x->second_best_mv = *best_mv;
                        *best_mv          = mv;
                    }
                }
            } else {
                // 4 sads in a single call if we are checking every location
                if (c + 3 <= end_col) {
                    unsigned int   sads[4];
                    const uint8_t* addrs[4];
                    for (i = 0; i < 4; ++i) {
                        const Mv mv = {{fcenter_mv.x + c + i, fcenter_mv.y + r}};
                        addrs[i]    = get_buf_from_mv(in_what, &mv);
                    }
                    fn_ptr->sdx4df(what->buf, what->stride, addrs, in_what->stride, sads);

                    for (i = 0; i < 4; ++i) {
                        if (sads[i] < best_sad) {
                            const Mv           mv  = {{fcenter_mv.x + c + i, fcenter_mv.y + r}};
                            const unsigned int sad = sads[i] + mvsad_err_cost(x, &mv, ref_mv, sad_per_bit);
                            if (sad < best_sad) {
                                best_sad          = sad;
                                x->second_best_mv = *best_mv;
                                *best_mv          = mv;
                            }
                        }
                    }
                } else {
                    for (i = 0; i < end_col - c; ++i) {
                        const Mv     mv  = {{fcenter_mv.x + c + i, fcenter_mv.y + r}};
                        unsigned int sad = fn_ptr->sdf(
                            what->buf, what->stride, get_buf_from_mv(in_what, &mv), in_what->stride);
                        if (sad < best_sad) {
                            sad += mvsad_err_cost(x, &mv, ref_mv, sad_per_bit);
                            if (sad < best_sad) {
                                best_sad          = sad;
                                x->second_best_mv = *best_mv;
                                *best_mv          = mv;
                            }
                        }
                    }
                }
            }
        }
    }

    return best_sad;
}

int svt_av1_diamond_search_sad_c(IntraBcContext* x, const SearchSiteConfig* cfg, Mv* ref_mv, Mv* best_mv,
                                 int search_param, int sad_per_bit, int* num00, const AomVarianceFnPtr* fn_ptr,
                                 const Mv* center_mv) {
    int i, j, step;

    uint8_t*       what        = x->plane[0].src.buf;
    const int      what_stride = x->plane[0].src.stride;
    const uint8_t* in_what;
    const int      in_what_stride = x->xdplane[0].pre[0].stride;
    const uint8_t* best_address;

    unsigned int bestsad;
    int          best_site = 0;
    int          last_site = 0;

    int ref_row;
    int ref_col;

    // search_param determines the length of the initial step and hence the number
    // of iterations.
    // 0 = initial step (MAX_FIRST_STEP) pel
    // 1 = (MAX_FIRST_STEP/2) pel,
    // 2 = (MAX_FIRST_STEP/4) pel...
    const SearchSite* ss        = &cfg->ss[search_param * cfg->searches_per_step];
    const int         tot_steps = (cfg->ss_count / cfg->searches_per_step) - search_param;

    const Mv fcenter_mv = {{center_mv->x >> 3, center_mv->y >> 3}};
    clamp_mv(ref_mv, x->mv_limits.col_min, x->mv_limits.col_max, x->mv_limits.row_min, x->mv_limits.row_max);
    ref_row    = ref_mv->y;
    ref_col    = ref_mv->x;
    *num00     = 0;
    best_mv->y = ref_row;
    best_mv->x = ref_col;

    // Work out the start point for the search
    in_what      = x->xdplane[0].pre[0].buf + ref_row * in_what_stride + ref_col;
    best_address = in_what;

    // Check the starting position
    bestsad = fn_ptr->sdf(what, what_stride, in_what, in_what_stride) +
        mvsad_err_cost(x, best_mv, &fcenter_mv, sad_per_bit);

    i = 1;

    for (step = 0; step < tot_steps; step++) {
        int all_in = 1;

        // All_in is true if every one of the points we are checking are within
        // the bounds of the image.
        all_in &= ((best_mv->y + ss[i].mv.y) > x->mv_limits.row_min);
        all_in &= ((best_mv->y + ss[i + 1].mv.y) < x->mv_limits.row_max);
        all_in &= ((best_mv->x + ss[i + 2].mv.x) > x->mv_limits.col_min);
        all_in &= ((best_mv->x + ss[i + 3].mv.x) < x->mv_limits.col_max);

        // If all the pixels are within the bounds we don't check whether the
        // search point is valid in this loop,  otherwise we check each point
        // for validity..
        if (all_in) {
            unsigned int sad_array[4];

            for (j = 0; j < cfg->searches_per_step; j += 4) {
                unsigned char const* block_offset[4];

                for (int t = 0; t < 4; t++) {
                    block_offset[t] = ss[i + t].offset + best_address;
                }

                fn_ptr->sdx4df(what, what_stride, block_offset, in_what_stride, sad_array);

                for (int t = 0; t < 4; t++, i++) {
                    if (sad_array[t] < bestsad) {
                        const Mv this_mv = {{best_mv->x + ss[i].mv.x, best_mv->y + ss[i].mv.y}};
                        sad_array[t] += mvsad_err_cost(x, &this_mv, &fcenter_mv, sad_per_bit);
                        if (sad_array[t] < bestsad) {
                            bestsad   = sad_array[t];
                            best_site = i;
                        }
                    }
                }
            }
        } else {
            for (j = 0; j < cfg->searches_per_step; j++) {
                // Trap illegal vectors
                const Mv this_mv = {{best_mv->x + ss[i].mv.x, best_mv->y + ss[i].mv.y}};

                if (is_mv_in(&x->mv_limits, &this_mv)) {
                    const uint8_t* const check_here = ss[i].offset + best_address;
                    unsigned int         thissad    = fn_ptr->sdf(what, what_stride, check_here, in_what_stride);

                    if (thissad < bestsad) {
                        thissad += mvsad_err_cost(x, &this_mv, &fcenter_mv, sad_per_bit);
                        if (thissad < bestsad) {
                            bestsad   = thissad;
                            best_site = i;
                        }
                    }
                }
                i++;
            }
        }
        if (best_site != last_site) {
            x->second_best_mv = *best_mv;
            best_mv->y += ss[best_site].mv.y;
            best_mv->x += ss[best_site].mv.x;
            best_address += ss[best_site].offset;
            last_site = best_site;
#if defined(NEW_DIAMOND_SEARCH)
            while (1) {
                const Mv this_mv = {{best_mv->x + ss[best_site].mv.x, best_mv->y + ss[best_site].mv.y}};
                if (is_mv_in(&x->mv_limits, &this_mv)) {
                    const uint8_t* const check_here = ss[best_site].offset + best_address;
                    unsigned int         thissad    = fn_ptr->sdf(what, what_stride, check_here, in_what_stride);
                    if (thissad < bestsad) {
                        thissad += mvsad_err_cost(x, &this_mv, &fcenter_mv, sad_per_bit);
                        if (thissad < bestsad) {
                            bestsad = thissad;
                            best_mv->y += ss[best_site].mv.y;
                            best_mv->x += ss[best_site].mv.x;
                            best_address += ss[best_site].offset;
                            continue;
                        }
                    }
                }
                break;
            }
#endif
        } else if (best_address == in_what) {
            (*num00)++;
        }
    }
    return bestsad;
}

static int svt_av1_refining_search_sad(IntraBcContext* x, Mv* ref_mv, int error_per_bit, int search_range,
                                       const AomVarianceFnPtr* fn_ptr, const Mv* center_mv) {
    const Mv           neighbors[4] = {{{0, -1}}, {{-1, 0}}, {{1, 0}}, {{0, 1}}};
    const Buf2D* const what         = &x->plane[0].src;
    const Buf2D* const in_what      = &x->xdplane[0].pre[0];
    const Mv           fcenter_mv   = {{center_mv->x >> 3, center_mv->y >> 3}};
    const uint8_t*     best_address = get_buf_from_mv(in_what, ref_mv);
    unsigned int       best_sad     = fn_ptr->sdf(what->buf, what->stride, best_address, in_what->stride) +
        mvsad_err_cost(x, ref_mv, &fcenter_mv, error_per_bit);
    for (int i = 0; i < search_range; i++) {
        int       best_site = -1;
        const int all_in    = (ref_mv->y - 1) > x->mv_limits.row_min && (ref_mv->y + 1) < x->mv_limits.row_max &&
            (ref_mv->x - 1) > x->mv_limits.col_min && (ref_mv->x + 1) < x->mv_limits.col_max;

        if (all_in) {
            unsigned int         sads[4];
            const uint8_t* const positions[4] = {
                best_address - in_what->stride, best_address - 1, best_address + 1, best_address + in_what->stride};

            fn_ptr->sdx4df(what->buf, what->stride, positions, in_what->stride, sads);

            for (int j = 0; j < 4; ++j) {
                if (sads[j] < best_sad) {
                    const Mv mv = {{ref_mv->x + neighbors[j].x, ref_mv->y + neighbors[j].y}};
                    sads[j] += mvsad_err_cost(x, &mv, &fcenter_mv, error_per_bit);
                    if (sads[j] < best_sad) {
                        best_sad  = sads[j];
                        best_site = j;
                    }
                }
            }
        } else {
            for (int j = 0; j < 4; ++j) {
                const Mv mv = {{ref_mv->x + neighbors[j].x, ref_mv->y + neighbors[j].y}};

                if (is_mv_in(&x->mv_limits, &mv)) {
                    unsigned int sad = fn_ptr->sdf(
                        what->buf, what->stride, get_buf_from_mv(in_what, &mv), in_what->stride);
                    if (sad < best_sad) {
                        sad += mvsad_err_cost(x, &mv, &fcenter_mv, error_per_bit);
                        if (sad < best_sad) {
                            best_sad  = sad;
                            best_site = j;
                        }
                    }
                }
            }
        }

        if (best_site == -1) {
            break;
        } else {
            x->second_best_mv = *ref_mv;
            ref_mv->y += neighbors[best_site].y;
            ref_mv->x += neighbors[best_site].x;
            best_address = get_buf_from_mv(in_what, ref_mv);
        }
    }

    return best_sad;
}

/* do_refine: If last step (1-away) of n-step search doesn't pick the center
              point as the best match, we will do a final 1-away diamond
              refining search  */
static int full_pixel_diamond(PictureControlSet* pcs, IntraBcContext /*MACROBLOCK*/* x, Mv* mvp_full, int step_param,
                              int sadpb, int further_steps, int do_refine, int* cost_list,
                              const AomVarianceFnPtr* fn_ptr, const Mv* ref_mv) {
    Mv  temp_mv;
    int thissme, n, num00 = 0;
    (void)cost_list;
    /*int bestsme = cpi->diamond_search_sad(x, &cpi->ss_cfg, mvp_full, &temp_mv,
                                        step_param, sadpb, &n, fn_ptr, ref_mv);*/
    int bestsme = svt_av1_diamond_search_sad_c(
        x, &pcs->ss_cfg, mvp_full, &temp_mv, step_param, sadpb, &n, fn_ptr, ref_mv);

    if (bestsme < INT_MAX) {
        bestsme = svt_av1_get_mvpred_var(x, &temp_mv, ref_mv, fn_ptr, 1);
    }
    x->best_mv = temp_mv;

    // If there won't be more n-step search, check to see if refining search is
    // needed.
    if (n > further_steps) {
        do_refine = 0;
    }

    while (n < further_steps) {
        ++n;

        if (num00) {
            num00--;
        } else {
            /*thissme = cpi->diamond_search_sad(x, &cpi->ss_cfg, mvp_full, &temp_mv,
                                        step_param + n, sadpb, &num00, fn_ptr,
                                        ref_mv);*/
            thissme = svt_av1_diamond_search_sad_c(
                x, &pcs->ss_cfg, mvp_full, &temp_mv, step_param + n, sadpb, &num00, fn_ptr, ref_mv);

            if (thissme < INT_MAX) {
                thissme = svt_av1_get_mvpred_var(x, &temp_mv, ref_mv, fn_ptr, 1);
            }

            // check to see if refining search is needed.
            if (num00 > further_steps - n) {
                do_refine = 0;
            }

            if (thissme < bestsme) {
                bestsme    = thissme;
                x->best_mv = temp_mv;
            }
        }
    }

    // final 1-away diamond refining search
    if (do_refine) {
        const int search_range = 8;
        Mv        best_mv      = x->best_mv;
        thissme                = svt_av1_refining_search_sad(x, &best_mv, sadpb, search_range, fn_ptr, ref_mv);
        if (thissme < INT_MAX) {
            thissme = svt_av1_get_mvpred_var(x, &best_mv, ref_mv, fn_ptr, 1);
        }
        if (thissme < bestsme) {
            bestsme    = thissme;
            x->best_mv = best_mv;
        }
    }

    // Return cost list.
    /* if (cost_list) {
    calc_int_cost_list(x, ref_mv, sadpb, fn_ptr, &x->best_mv.as_mv, cost_list);
  }*/
    return bestsme;
}

#define MIN_RANGE 7
#define MAX_RANGE 256
#define MIN_INTERVAL 1

// Runs an limited range exhaustive mesh search using a pattern set
// according to the encode speed profile.
static int full_pixel_exhaustive(PictureControlSet* pcs, IntraBcContext* x, const Mv* centre_mv_full, int sadpb,
                                 int* cost_list, const AomVarianceFnPtr* fn_ptr, const Mv* ref_mv, Mv* dst_mv) {
    UNUSED(cost_list);
    const SpeedFeatures* const sf       = &pcs->sf; // cpi->sf;
    Mv                         temp_mv  = {.as_int = centre_mv_full->as_int};
    Mv                         f_ref_mv = {{ref_mv->x >> 3, ref_mv->y >> 3}};
    int                        bestsme;
    int                        interval = sf->mesh_patterns[0].interval;
    int                        range    = sf->mesh_patterns[0].range;
    int                        baseline_interval_divisor;

    // Trap illegal values for interval and range for this function.
    if ((range < MIN_RANGE) || (range > MAX_RANGE) || (interval < MIN_INTERVAL) || (interval > range)) {
        return INT_MAX;
    }

    baseline_interval_divisor = range / interval;

    // Check size of proposed first range against magnitude of the centre
    // value used as a starting point.
    range    = AOMMAX(range, (5 * AOMMAX(abs(temp_mv.y), abs(temp_mv.x))) / 4);
    range    = AOMMIN(range, MAX_RANGE);
    interval = AOMMAX(interval, range / baseline_interval_divisor);

    // initial search
    bestsme = exhaustive_mesh_search(x, &f_ref_mv, &temp_mv, range, interval, sadpb, fn_ptr, &temp_mv);

    if ((interval > MIN_INTERVAL) && (range > MIN_RANGE)) {
        // Progressive searches with range and step size decreasing each time
        // till we reach a step size of 1. Then break out.
        for (int i = 1; i < MAX_MESH_STEP; ++i) {
            // First pass with coarser step and longer range
            bestsme = exhaustive_mesh_search(x,
                                             &f_ref_mv,
                                             &temp_mv,
                                             sf->mesh_patterns[i].range,
                                             sf->mesh_patterns[i].interval,
                                             sadpb,
                                             fn_ptr,
                                             &temp_mv);

            if (sf->mesh_patterns[i].interval == 1) {
                break;
            }
        }
    }

    if (bestsme < INT_MAX) {
        bestsme = svt_av1_get_mvpred_var(x, &temp_mv, ref_mv, fn_ptr, 1);
    }
    *dst_mv = temp_mv;

    // Return cost list.
    /* if (cost_list) {
    calc_int_cost_list(x, ref_mv, sadpb, fn_ptr, dst_mv, cost_list);
  }*/
    return bestsme;
}

#if CONFIG_ENABLE_OBMC
static int get_obmc_mvpred_var(const IntraBcContext* x, const int32_t* wsrc, const int32_t* mask, const Mv* best_mv,
                               const Mv* center_mv, const AomVarianceFnPtr* vfp, int use_mvcost, int is_second) {
    const Buf2D* in_what = (const Buf2D*)(&x->xdplane[0].pre[is_second]);
    const Mv     mv      = {{best_mv->x * 8, best_mv->y * 8}};
    unsigned int unused;
    if (x->approx_inter_rate) {
        return vfp->ovf(get_buf_from_mv((const Buf2D*)in_what, best_mv), in_what->stride, wsrc, mask, &unused) +
            (use_mvcost ? svt_aom_mv_err_cost_light(&mv, center_mv) : 0);
    } else {
        return vfp->ovf(get_buf_from_mv((const Buf2D*)in_what, best_mv), in_what->stride, wsrc, mask, &unused) +
            (use_mvcost ? svt_aom_mv_err_cost(&mv, center_mv, x->nmv_vec_cost, x->mv_cost_stack, x->errorperbit) : 0);
    }
}

static int obmc_refining_search_sad(const IntraBcContext* x, const int32_t* wsrc, const int32_t* mask, Mv* ref_mv,
                                    int error_per_bit, int search_range, const AomVarianceFnPtr* fn_ptr,
                                    const Mv* center_mv, int is_second, uint8_t search_diag) {
    const Mv     neighbors[8] = {{{0, -1}}, {{-1, 0}}, {{1, 0}}, {{0, 1}}, {{1, -1}}, {{1, 1}}, {{-1, 1}}, {{-1, -1}}};
    const Buf2D* in_what      = (const Buf2D*)(&x->xdplane[0].pre[is_second]);
    const Mv     fcenter_mv   = {{center_mv->x >> 3, center_mv->y >> 3}};
    unsigned int best_sad = fn_ptr->osdf(get_buf_from_mv((const Buf2D*)in_what, ref_mv), in_what->stride, wsrc, mask) +
        mvsad_err_cost(x, ref_mv, &fcenter_mv, error_per_bit);
    int i, j;

    for (i = 0; i < search_range; i++) {
        int best_site = -1;

        for (j = 0; j < (search_diag ? 8 : 4); j++) {
            const Mv mv = {{ref_mv->x + neighbors[j].x, ref_mv->y + neighbors[j].y}};
            if (is_mv_in(&x->mv_limits, &mv)) {
                unsigned int sad = fn_ptr->osdf(
                    get_buf_from_mv((const Buf2D*)in_what, &mv), in_what->stride, wsrc, mask);
                if (sad < best_sad) {
                    sad += mvsad_err_cost(x, &mv, &fcenter_mv, error_per_bit);
                    if (sad < best_sad) {
                        best_sad  = sad;
                        best_site = j;
                    }
                }
            }
        }

        if (best_site == -1) {
            break;
        } else {
            ref_mv->y += neighbors[best_site].y;
            ref_mv->x += neighbors[best_site].x;
        }
    }
    return best_sad;
}

int svt_av1_obmc_full_pixel_search(ModeDecisionContext* ctx, IntraBcContext* x, const Mv* mvp_full, int sadpb,
                                   const AomVarianceFnPtr* fn_ptr, const Mv* ref_mv, Mv* dst_mv, int is_second) {
    // obmc_full_pixel_diamond does not provide BDR gain on 360p
    const int32_t* wsrc         = ctx->wsrc_buf;
    const int32_t* mask         = ctx->mask_buf;
    const int      search_range = ctx->obmc_ctrls.fpel_search_range;
    *dst_mv                     = *mvp_full;
    x->approx_inter_rate        = ctx->approx_inter_rate;
    clamp_mv(dst_mv, x->mv_limits.col_min, x->mv_limits.col_max, x->mv_limits.row_min, x->mv_limits.row_max);
    clamp_mv(dst_mv, x->mv_limits.col_min, x->mv_limits.col_max, x->mv_limits.row_min, x->mv_limits.row_max);
    int thissme = obmc_refining_search_sad(
        x, wsrc, mask, dst_mv, sadpb, search_range, fn_ptr, ref_mv, is_second, ctx->obmc_ctrls.fpel_search_diag);
    if (thissme < INT_MAX) {
        thissme = get_obmc_mvpred_var(x, wsrc, mask, dst_mv, ref_mv, fn_ptr, 1, is_second);
    }

    return thissme;
}
#endif

#if CONFIG_ENABLE_OBMC
static INLINE void set_subpel_mv_search_range(const MvLimits* mv_limits, int* col_min, int* col_max, int* row_min,
                                              int* row_max, const Mv* ref_mv) {
    const int max_mv = MAX_FULL_PEL_VAL * 8;
    const int minc   = AOMMAX(mv_limits->col_min * 8, ref_mv->x - max_mv);
    const int maxc   = AOMMIN(mv_limits->col_max * 8, ref_mv->x + max_mv);
    const int minr   = AOMMAX(mv_limits->row_min * 8, ref_mv->y - max_mv);
    const int maxr   = AOMMIN(mv_limits->row_max * 8, ref_mv->y + max_mv);

    *col_min = AOMMAX(MV_LOW + 1, minc);
    *col_max = AOMMIN(MV_UPP - 1, maxc);
    *row_min = AOMMAX(MV_LOW + 1, minr);
    *row_max = AOMMIN(MV_UPP - 1, maxr);
}

static const Mv search_step_table[12] = {
    // left, right, up, down
    {{-4, 0}},
    {{4, 0}},
    {{0, -4}},
    {{0, 4}},
    {{-2, 0}},
    {{2, 0}},
    {{0, -2}},
    {{0, 2}},
    {{-1, 0}},
    {{1, 0}},
    {{0, -1}},
    {{0, 1}}};

static unsigned int setup_obmc_center_error(const int32_t* mask, const Mv* bestmv, const Mv* ref_mv, int error_per_bit,
                                            const AomVarianceFnPtr* vfp, const int32_t* const wsrc,
                                            const uint8_t* const y, int y_stride, int offset, int* mvjcost,
                                            const int* mvcost[2], unsigned int* sse1,
                                            uint8_t use_low_precision_cost_estimation, int* distortion) {
    unsigned int besterr;
    besterr     = vfp->ovf(y + offset, y_stride, wsrc, mask, sse1);
    *distortion = besterr;
    if (use_low_precision_cost_estimation) {
        besterr += svt_aom_mv_err_cost_light(bestmv, ref_mv);
    } else {
        besterr += svt_aom_mv_err_cost(bestmv, ref_mv, mvjcost, mvcost, error_per_bit);
    }
    return besterr;
}

/* returns subpixel variance error function */
#define DIST(r, c) vfp->osvf(pre(y, y_stride, r, c), y_stride, sp(c), sp(r), z, mask, &sse)
#define CHECK_BETTER(v, r, c, lp)                                                                     \
    do {                                                                                              \
        if (c >= minc && c <= maxc && r >= minr && r <= maxr) {                                       \
            thismse = (DIST(r, c));                                                                   \
                                                                                                      \
            if (lp)                                                                                   \
                v = svt_aom_mv_err_cost_light(&(const Mv){{c, r}}, ref_mv);                           \
            else                                                                                      \
                v = svt_aom_mv_err_cost(&(const Mv){{c, r}}, ref_mv, mvjcost, mvcost, error_per_bit); \
            if ((v + thismse) < besterr) {                                                            \
                besterr     = v + thismse;                                                            \
                br          = r;                                                                      \
                bc          = c;                                                                      \
                *distortion = thismse;                                                                \
                *sse1       = sse;                                                                    \
            }                                                                                         \
        } else                                                                                        \
            v = INT_MAX;                                                                              \
    } while (0)
#define CHECK_BETTER0(v, r, c, lp) CHECK_BETTER(v, r, c, lp)
#define CHECK_BETTER1(v, r, c, lp)                                                         \
    do {                                                                                   \
        if (c >= minc && c <= maxc && r >= minr && r <= maxr) {                            \
            Mv this_mv = {{c, r}};                                                         \
            thismse    = upsampled_obmc_pref_error(xd,                                     \
                                                cm,                                     \
                                                mi_row,                                 \
                                                mi_col,                                 \
                                                &this_mv,                               \
                                                mask,                                   \
                                                vfp,                                    \
                                                z,                                      \
                                                pre(y, y_stride, r, c),                 \
                                                y_stride,                               \
                                                sp(c),                                  \
                                                sp(r),                                  \
                                                w,                                      \
                                                h,                                      \
                                                &sse,                                   \
                                                use_accurate_subpel_search);            \
            if (lp)                                                                        \
                v = svt_aom_mv_err_cost_light(&this_mv, ref_mv);                           \
            else                                                                           \
                v = svt_aom_mv_err_cost(&this_mv, ref_mv, mvjcost, mvcost, error_per_bit); \
            if ((v + thismse) < besterr) {                                                 \
                besterr     = v + thismse;                                                 \
                br          = r;                                                           \
                bc          = c;                                                           \
                *distortion = thismse;                                                     \
                *sse1       = sse;                                                         \
            }                                                                              \
        } else                                                                             \
            v = INT_MAX;                                                                   \
    } while (0)
#define SECOND_LEVEL_CHECKS_BEST(k)                          \
    do {                                                     \
        unsigned int second;                                 \
        int          br0 = br;                               \
        int          bc0 = bc;                               \
        assert(tr == br || tc == bc);                        \
        if (tr == br && tc != bc)                            \
            kc = bc - tc;                                    \
        else if (tr != br && tc == bc)                       \
            kr = br - tr;                                    \
        CHECK_BETTER##k(second, br0 + kr, bc0, lp);          \
        CHECK_BETTER##k(second, br0, bc0 + kc, lp);          \
        if (br0 != br || bc0 != bc)                          \
            CHECK_BETTER##k(second, br0 + kr, bc0 + kc, lp); \
    } while (0)

static int upsampled_obmc_pref_error(MacroBlockD* xd, const Av1Common* const cm, int mi_row, int mi_col,
                                     const Mv* const mv, const int32_t* mask, const AomVarianceFnPtr* vfp,
                                     const int32_t* const wsrc, const uint8_t* const y, int y_stride, int subpel_x_q3,
                                     int subpel_y_q3, int w, int h, unsigned int* sse, int subpel_search) {
    unsigned int besterr;

    DECLARE_ALIGNED(16, uint8_t, pred[2 * MAX_SB_SQUARE]);
#if CONFIG_AV1_HIGHBITDEPTH
    if (is_cur_buf_hbd(xd)) {
        uint8_t* pred8 = CONVERT_TO_BYTEPTR(pred);
        aom_highbd_upsampled_pred(
            xd, cm, mi_row, mi_col, mv, pred8, w, h, subpel_x_q3, subpel_y_q3, y, y_stride, xd->bd, subpel_search);
        besterr = vfp->ovf(pred8, w, wsrc, mask, sse);
    } else {
        svt_aom_upsampled_pred(
            xd, cm, mi_row, mi_col, mv, pred, w, h, subpel_x_q3, subpel_y_q3, y, y_stride, subpel_search);

        besterr = vfp->ovf(pred, w, wsrc, mask, sse);
    }
#else
    svt_aom_upsampled_pred(xd,
                           (const struct AV1Common* const)cm,
                           mi_row,
                           mi_col,
                           mv,
                           pred,
                           w,
                           h,
                           subpel_x_q3,
                           subpel_y_q3,
                           y,
                           y_stride,
                           subpel_search);

    besterr = vfp->ovf(pred, w, wsrc, mask, sse);
#endif
    return besterr;
}

static unsigned int upsampled_setup_obmc_center_error(MacroBlockD* xd, const Av1Common* const cm, int mi_row,
                                                      int mi_col, const int32_t* mask, const Mv* bestmv,
                                                      const Mv* ref_mv, int error_per_bit, const AomVarianceFnPtr* vfp,
                                                      const int32_t* const wsrc, const uint8_t* const y, int y_stride,
                                                      int w, int h, int offset, int* mvjcost, const int* mvcost[2],
                                                      unsigned int* sse1, int* distortion,
                                                      uint8_t use_low_precision_cost_estimation, int subpel_search) {
    unsigned int besterr = upsampled_obmc_pref_error(
        xd, cm, mi_row, mi_col, bestmv, mask, vfp, wsrc, y + offset, y_stride, 0, 0, w, h, sse1, subpel_search);
    *distortion = besterr;
    if (use_low_precision_cost_estimation) {
        besterr += svt_aom_mv_err_cost_light(bestmv, ref_mv);
    } else {
        besterr += svt_aom_mv_err_cost(bestmv, ref_mv, mvjcost, mvcost, error_per_bit);
    }
    return besterr;
}

// convert motion vector component to offset for sv[a]f calc
static INLINE int sp(int x) {
    return x & 7;
}

static INLINE const uint8_t* pre(const uint8_t* buf, int stride, int r, int c) {
    const int offset = (r >> 3) * stride + (c >> 3);
    return buf + offset;
}

int svt_av1_find_best_obmc_sub_pixel_tree_up(ModeDecisionContext* ctx, IntraBcContext* x,
                                             const struct Av1Common* const cm, int mi_row, int mi_col, Mv* bestmv,
                                             const Mv* ref_mv, int allow_hp, int error_per_bit,
                                             const AomVarianceFnPtr* vfp, int forced_stop, int iters_per_step,
                                             int* mvjcost, const int* mvcost[2], int* distortion, unsigned int* sse1,
                                             int is_second, int use_accurate_subpel_search) {
    const int32_t*                 wsrc        = ctx->wsrc_buf;
    const int32_t*                 mask        = ctx->mask_buf;
    const int* const               z           = wsrc;
    const int* const               src_address = z;
    MacroBlockD*                   xd          = x->xd;
    struct MacroBlockDPlane* const pd          = &x->xdplane[0];
    unsigned int                   besterr     = INT_MAX;
    unsigned int                   sse;
    unsigned int                   thismse;
    int                            br    = bestmv->y * 8;
    int                            bc    = bestmv->x * 8;
    int                            hstep = 4;
    int                            round = 3 - forced_stop;
    int                            tr;
    int                            tc;
    const Mv*                      search_step = search_step_table;
    int                            best_idx    = -1;
    unsigned int                   cost_array[5];
    const int                      w  = block_size_wide[ctx->blk_geom->bsize];
    const int                      h  = block_size_high[ctx->blk_geom->bsize];
    const uint8_t                  lp = ctx->approx_inter_rate;
    int                            minc, maxc, minr, maxr;

    set_subpel_mv_search_range(&x->mv_limits, &minc, &maxc, &minr, &maxr, ref_mv);

    const uint8_t* y        = pd->pre[is_second].buf;
    int            y_stride = pd->pre[is_second].stride;
    int            offset   = bestmv->y * y_stride + bestmv->x;

    if (!allow_hp && round == 3) {
        round = 2;
    }

    bestmv->y *= 8;
    bestmv->x *= 8;
    // use_accurate_subpel_search can be 0 or 1 or 2
    besterr = use_accurate_subpel_search
        ? upsampled_setup_obmc_center_error(xd,
                                            cm,
                                            mi_row,
                                            mi_col,
                                            mask,
                                            bestmv,
                                            ref_mv,
                                            error_per_bit,
                                            vfp,
                                            z,
                                            y,
                                            y_stride,
                                            w,
                                            h,
                                            offset,
                                            mvjcost,
                                            mvcost,
                                            sse1,
                                            distortion,
                                            lp,
                                            use_accurate_subpel_search)
        : setup_obmc_center_error(
              mask, bestmv, ref_mv, error_per_bit, vfp, z, y, y_stride, offset, mvjcost, mvcost, sse1, lp, distortion);

    for (int iter = 0; iter < round; ++iter) {
        // Check vertical and horizontal sub-pixel positions.
        int idx = 0;
        for (; idx < 4; ++idx) {
            tr = br + search_step[idx].y;
            tc = bc + search_step[idx].x;
            if (tc >= minc && tc <= maxc && tr >= minr && tr <= maxr) {
                Mv this_mv = {{tc, tr}};
                thismse    = use_accurate_subpel_search
                       ? (unsigned)upsampled_obmc_pref_error(xd,
                                                          cm,
                                                          mi_row,
                                                          mi_col,
                                                          &this_mv,
                                                          mask,
                                                          vfp,
                                                          src_address,
                                                          pre(y, y_stride, tr, tc),
                                                          y_stride,
                                                          sp(tc),
                                                          sp(tr),
                                                          w,
                                                          h,
                                                          &sse,
                                                          use_accurate_subpel_search)
                       : vfp->osvf(pre(y, y_stride, tr, tc), y_stride, sp(tc), sp(tr), src_address, mask, &sse);
                if (lp) {
                    cost_array[idx] = thismse + svt_aom_mv_err_cost_light(&this_mv, ref_mv);
                } else {
                    cost_array[idx] = thismse + svt_aom_mv_err_cost(&this_mv, ref_mv, mvjcost, mvcost, error_per_bit);
                }
                if (cost_array[idx] < besterr) {
                    best_idx    = idx;
                    besterr     = cost_array[idx];
                    *distortion = thismse;
                    *sse1       = sse;
                }
            } else {
                cost_array[idx] = INT_MAX;
            }
        }

        // Check diagonal sub-pixel position
        int kc = (cost_array[0] <= cost_array[1] ? -hstep : hstep);
        int kr = (cost_array[2] <= cost_array[3] ? -hstep : hstep);

        tc = bc + kc;
        tr = br + kr;
        if (tc >= minc && tc <= maxc && tr >= minr && tr <= maxr) {
            Mv this_mv = {{tc, tr}};
            thismse    = use_accurate_subpel_search
                   ? (unsigned)upsampled_obmc_pref_error(xd,
                                                      cm,
                                                      mi_row,
                                                      mi_col,
                                                      &this_mv,
                                                      mask,
                                                      vfp,
                                                      src_address,
                                                      pre(y, y_stride, tr, tc),
                                                      y_stride,
                                                      sp(tc),
                                                      sp(tr),
                                                      w,
                                                      h,
                                                      &sse,
                                                      use_accurate_subpel_search)
                   : vfp->osvf(pre(y, y_stride, tr, tc), y_stride, sp(tc), sp(tr), src_address, mask, &sse);
            if (lp) {
                cost_array[4] = thismse + svt_aom_mv_err_cost_light(&this_mv, ref_mv);
            } else {
                cost_array[4] = thismse + svt_aom_mv_err_cost(&this_mv, ref_mv, mvjcost, mvcost, error_per_bit);
            }

            if (cost_array[4] < besterr) {
                best_idx    = 4;
                besterr     = cost_array[4];
                *distortion = thismse;
                *sse1       = sse;
            }
        } else {
            cost_array[idx] = INT_MAX;
        }

        if (best_idx < 4 && best_idx >= 0) {
            br += search_step[best_idx].y;
            bc += search_step[best_idx].x;
        } else if (best_idx == 4) {
            br = tr;
            bc = tc;
        }

        if (iters_per_step > 1 && best_idx != -1) {
            if (use_accurate_subpel_search) {
                SECOND_LEVEL_CHECKS_BEST(1);
            } else {
                SECOND_LEVEL_CHECKS_BEST(0);
            }
        }

        search_step += 4;
        hstep >>= 1;
        best_idx = -1;
    }

    bestmv->y = br;
    bestmv->x = bc;

    return besterr;
}
#endif

int svt_av1_full_pixel_search(PictureControlSet* pcs, IntraBcContext* x, BlockSize bsize, Mv* mvp_full, int step_param,
                              int error_per_bit, int* cost_list, const Mv* ref_mv, int x_pos, int y_pos, int intra) {
    int32_t ibc_shift = 0;
    //IBC Modes:   0: OFF 1:Slow   2:Faster   3:Fastest
    ibc_shift = pcs->ppcs->intraBC_ctrls.ibc_shift;

    SpeedFeatures* sf              = &pcs->sf;
    sf->exhaustive_searches_thresh = (1 << 25);
    const AomVarianceFnPtr* fn_ptr = &svt_aom_mefn_ptr[bsize];
    int                     var    = 0;

    if (cost_list) {
        cost_list[0] = INT_MAX;
        cost_list[1] = INT_MAX;
        cost_list[2] = INT_MAX;
        cost_list[3] = INT_MAX;
        cost_list[4] = INT_MAX;
    }

    var = full_pixel_diamond(
        pcs, x, mvp_full, step_param, error_per_bit, MAX_MVSEARCH_STEPS - 1 - step_param, 1, cost_list, fn_ptr, ref_mv);

    if (x->is_exhaustive_allowed) {
        int exhaustive_thr = sf->exhaustive_searches_thresh;
        exhaustive_thr >>= 10 - (mi_size_wide_log2[bsize] + mi_size_high_log2[bsize]);

        exhaustive_thr = exhaustive_thr << ibc_shift;

        if (var > exhaustive_thr) {
            int var_ex;
            Mv  tmp_mv_ex;
            var_ex = full_pixel_exhaustive(pcs, x, &x->best_mv, error_per_bit, cost_list, fn_ptr, ref_mv, &tmp_mv_ex);

            if (var_ex < var) {
                var        = var_ex;
                x->best_mv = tmp_mv_ex;
            }
        }
    }

    do {
        // already single ME
        // get block size and original buffer of current block
        const int block_height = block_size_high[bsize];
        const int block_width  = block_size_wide[bsize];
        if (block_height == block_width && x_pos >= 0 && y_pos >= 0 &&
            block_width <= pcs->ppcs->intraBC_ctrls.max_block_size_hash) {
            if (block_width == 4 || block_width == 8 || block_width == 16 || block_width == 32 || block_width == 64 ||
                block_width == 128) {
                uint8_t*  what        = x->plane[0].src.buf;
                const int what_stride = x->plane[0].src.stride;
                uint32_t  hash_value1, hash_value2;
                Mv        best_hash_mv;
                int       best_hash_cost = INT_MAX;

                // for the hashMap
                HashTable* ref_frame_hash = &pcs->hash_table;

                svt_av1_get_block_hash_value(what, what_stride, block_width, &hash_value1, &hash_value2, 0, pcs, x);

                const int count = svt_av1_hash_table_count(ref_frame_hash, hash_value1);
                // for intra, at least one matching can be found, itself.
                if (count <= (intra ? 1 : 0)) {
                    break;
                }
                Iterator iterator = svt_av1_hash_get_first_iterator(ref_frame_hash, hash_value1);
                for (int i = 0; i < count; i++, svt_aom_iterator_increment(&iterator)) {
                    BlockHash ref_block_hash = *(BlockHash*)(svt_aom_iterator_get(&iterator));
                    if (hash_value2 == ref_block_hash.hash_value2) {
                        // For intra, make sure the prediction is from valid area.
                        if (intra) {
                            const int mi_col = x_pos / MI_SIZE;
                            const int mi_row = y_pos / MI_SIZE;

                            const Mv dv = {{8 * (ref_block_hash.x - x_pos), 8 * (ref_block_hash.y - y_pos)}};
                            if (!svt_aom_is_dv_valid(
                                    dv, x->xd, mi_row, mi_col, bsize, pcs->ppcs->scs->seq_header.sb_size_log2)) {
                                continue;
                            }
                        }
                        Mv hash_mv;
                        hash_mv.x = ref_block_hash.x - x_pos;
                        hash_mv.y = ref_block_hash.y - y_pos;
                        if (!is_mv_in(&x->mv_limits, &hash_mv)) {
                            continue;
                        }
                        const int ref_cost = svt_av1_get_mvpred_var(x, &hash_mv, ref_mv, fn_ptr, 1);
                        if (ref_cost < best_hash_cost) {
                            best_hash_cost = ref_cost;
                            best_hash_mv   = hash_mv;
                        }
                    }
                }

                if (best_hash_cost < var) {
                    x->second_best_mv = x->best_mv;
                    x->best_mv        = best_hash_mv;
                    var               = best_hash_cost;
                }
            }
        }
    } while (0);

    return 0;
}
