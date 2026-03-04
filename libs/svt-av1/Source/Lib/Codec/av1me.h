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

#ifndef AOM_AV1_ENCODER_ME_H_
#define AOM_AV1_ENCODER_ME_H_

#include "definitions.h"
#include "coding_unit.h"

#ifdef __cplusplus
extern "C" {
#endif

// The maximum number of steps in a step search given the largest
// allowed initial step
#define MAX_MVSEARCH_STEPS 11
// Max full pel mv specified in the unit of full pixel
// Enable the use of motion vector in range [-1023, 1023].
#define MAX_FULL_PEL_VAL ((1 << (MAX_MVSEARCH_STEPS - 1)) - 1)
// Maximum size of the first step in full pel units
#define MAX_FIRST_STEP (1 << (MAX_MVSEARCH_STEPS - 1))

// motion search site
typedef struct SearchSite {
    Mv  mv;
    int offset;
} SearchSite;

typedef struct SearchSiteConfig {
    SearchSite ss[8 * MAX_MVSEARCH_STEPS + 1];
    int        ss_count;
    int        searches_per_step;
} SearchSiteConfig;

typedef unsigned int (*AomObmcSadFn)(const uint8_t* pred, int pred_stride, const int32_t* wsrc, const int32_t* msk);
typedef unsigned int (*AomObmcVarianceFn)(const uint8_t* pred, int pred_stride, const int32_t* wsrc, const int32_t* msk,
                                          unsigned int* sse);
typedef unsigned int (*AomObmcSubpixvarianceFn)(const uint8_t* pred, int pred_stride, int xoffset, int yoffset,
                                                const int32_t* wsrc, const int32_t* msk, unsigned int* sse);
typedef unsigned int (*AomSadFn)(const uint8_t* a, int a_stride, const uint8_t* b, int b_stride);

typedef unsigned int (*AomVarianceFn)(const uint8_t* a, int a_stride, const uint8_t* b, int b_stride,
                                      unsigned int* sse);
typedef unsigned int (*AomSubpixVarianceFn)(const uint8_t* a, int a_stride, int xoffset, int yoffset, const uint8_t* b,
                                            int b_stride, unsigned int* sse);
typedef void (*AomSadMultiDFn)(const uint8_t* a, int a_stride, const uint8_t* const b_array[], int b_stride,
                               unsigned int* sad_array);

typedef struct aom_variance_vtable {
    AomSadFn                sdf;
    AomVarianceFn           vf;
    AomVarianceFn           vf_hbd_10;
    AomSubpixVarianceFn     svf;
    AomSadMultiDFn          sdx4df;
    AomObmcSadFn            osdf;
    AomObmcVarianceFn       ovf;
    AomObmcSubpixvarianceFn osvf;

} AomVarianceFnPtr;

extern AomVarianceFnPtr svt_aom_mefn_ptr[BLOCK_SIZES_ALL];

void av1_init_dsmotion_compensation(SearchSiteConfig* cfg, int stride);
void svt_av1_init3smotion_compensation(SearchSiteConfig* cfg, int stride);
void svt_av1_set_mv_search_range(MvLimits* mv_limits, const Mv* mv);

int svt_av1_full_pixel_search(struct PictureControlSet* pcs, IntraBcContext /*MACROBLOCK*/* x, BlockSize bsize,
                              Mv* mvp_full, int step_param, int error_per_bit, int* cost_list, const Mv* ref_mv,
                              int x_pos, int y_pos, int intra);
int svt_aom_mv_err_cost(const Mv* mv, const Mv* ref, const int* mvjcost, const int* mvcost[2], int error_per_bit);
int svt_aom_mv_err_cost_light(const Mv* mv, const Mv* ref);
#if CONFIG_ENABLE_OBMC
struct ModeDecisionContext;
struct Av1Common;
int svt_av1_obmc_full_pixel_search(struct ModeDecisionContext* ctx, IntraBcContext* x, const Mv* mvp_full, int sadpb,
                                   const AomVarianceFnPtr* fn_ptr, const Mv* ref_mv, Mv* dst_mv, int is_second);
int svt_av1_find_best_obmc_sub_pixel_tree_up(struct ModeDecisionContext* ctx, IntraBcContext* x,
                                             const struct Av1Common* const cm, int mi_row, int mi_col, Mv* bestmv,
                                             const Mv* ref_mv, int allow_hp, int error_per_bit,
                                             const AomVarianceFnPtr* vfp, int forced_stop, int iters_per_step,
                                             int* mvjcost, const int* mvcost[2], int* distortion, unsigned int* sse1,
                                             int is_second, int use_accurate_subpel_search);
#endif
#ifdef __cplusplus
} // extern "C"
#endif

#endif // AOM_AV1_ENCODER_ME_H_
