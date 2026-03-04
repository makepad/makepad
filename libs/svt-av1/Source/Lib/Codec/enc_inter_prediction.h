/*
* Copyright(c) 2019 Intel Corporation
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#ifndef EbEncInterPrediction_h
#define EbEncInterPrediction_h

#include "mode_decision.h"
#include "filter.h"
#include "convolve.h"
#include "inter_prediction.h"
#include "enc_intra_prediction.h"

#ifdef __cplusplus
extern "C" {
#endif

extern aom_highbd_convolve_fn_t svt_aom_convolveHbd[/*subX*/ 2][/*subY*/ 2][/*bi*/ 2];

struct calc_target_weighted_pred_ctxt {
    int32_t*       mask_buf;
    int32_t*       wsrc_buf;
    const uint8_t* tmp;
    int            tmp_stride;
    int            overlap;
};

EbErrorType svt_aom_simple_luma_unipred(SequenceControlSet* scs, ScaleFactors sf_identity, uint32_t interp_filters,
                                        BlkStruct* blk_ptr, Mv mv, uint16_t pu_origin_x, uint16_t pu_origin_y,
                                        uint8_t bwidth, uint8_t bheight, EbPictureBufferDesc* ref_pic_list0,
                                        EbPictureBufferDesc* prediction_ptr, uint16_t dst_origin_x,
                                        uint16_t dst_origin_y, uint8_t bit_depth, uint8_t subsampling_shift);
EbErrorType svt_aom_inter_prediction(SequenceControlSet* scs, PictureControlSet* pcs, BlockModeInfo* block_mi,
                                     WarpedMotionParams* wm_params_0, WarpedMotionParams* wm_params_1,
                                     BlkStruct* blk_ptr, const BlockSize bsize, const Part shape,
                                     bool use_precomputed_obmc, bool use_precomputed_ii,
                                     struct ModeDecisionContext* ctx, NeighborArrayUnit* recon_neigh_y,
                                     NeighborArrayUnit* recon_neigh_cb, NeighborArrayUnit* recon_neigh_cr,
                                     EbPictureBufferDesc* ref_pic_0, EbPictureBufferDesc* ref_pic_1,
                                     uint16_t ref_origin_x, uint16_t ref_origin_y, EbPictureBufferDesc* pred_pic,
                                     uint16_t dst_origin_x, uint16_t dst_origin_y, uint32_t component_mask,
                                     uint8_t bit_depth, uint8_t is_16bit_pipeline);
void        svt_aom_search_compound_diff_wedge(PictureControlSet* pcs, struct ModeDecisionContext* ctx,
                                               ModeDecisionCandidate* cand);
bool        svt_aom_calc_pred_masked_compound(PictureControlSet* pcs, struct ModeDecisionContext* ctx,
                                              ModeDecisionCandidate* cand);

EbErrorType svt_aom_inter_pu_prediction_av1_light_pd0(uint8_t hbd_md, struct ModeDecisionContext* ctx,
                                                      PictureControlSet* pcs, ModeDecisionCandidateBuffer* cand_bf);
EbErrorType svt_aom_inter_pu_prediction_av1_light_pd1(uint8_t hbd_md, struct ModeDecisionContext* ctx,
                                                      PictureControlSet* pcs, ModeDecisionCandidateBuffer* cand_bf);
EbErrorType svt_aom_inter_pu_prediction_av1(uint8_t hbd_md, struct ModeDecisionContext* ctx, PictureControlSet* pcs,
                                            ModeDecisionCandidateBuffer* cand_bf);

void    svt_aom_precompute_obmc_data(PictureControlSet* pcs, struct ModeDecisionContext* ctx, uint32_t component_mask);
int64_t pick_wedge_fixed_sign(PictureControlSet* pcs, struct ModeDecisionContext* ctx, const BlockSize bsize,
                              const int16_t* const residual1, const int16_t* const diff10, const int8_t wedge_sign,
                              int8_t* const best_wedge_index);

void model_rd_for_sb_with_curvfit(PictureControlSet* pcs, struct ModeDecisionContext* ctx, BlockSize bsize, int bw,
                                  int bh, uint8_t* src_buf, uint32_t src_stride, uint8_t* pred_buf,
                                  uint32_t pred_stride, int plane_from, int plane_to, int mi_row, int mi_col,
                                  int* out_rate_sum, int64_t* out_dist_sum, int* plane_rate, int64_t* plane_sse,
                                  int64_t* plane_dist);
const uint8_t* svt_av1_get_obmc_mask(int length);

void model_rd_from_sse(BlockSize bsize, int16_t quantizer, uint8_t bit_depth, uint64_t sse, uint32_t* rate,
                       uint64_t* dist, uint8_t simple_model_rd_from_var);
void svt_aom_enc_make_inter_predictor(SequenceControlSet* scs, uint8_t* src_ptr, uint8_t* src_ptr_2b, uint8_t* dst_ptr,
                                      int16_t pre_y, int16_t pre_x, Mv mv, const struct ScaleFactors* const sf,
                                      ConvolveParams* conv_params, InterpFilters interp_filters,
                                      const InterInterCompoundData* const interinter_comp, uint8_t* seg_mask,
                                      uint16_t frame_width, uint16_t frame_height, uint8_t blk_width,
                                      uint8_t blk_height, BlockSize bsize, MacroBlockD* av1xd, int32_t src_stride,
                                      int32_t dst_stride, uint8_t plane, const uint32_t ss_y, const uint32_t ss_x,
                                      uint8_t bit_depth, uint8_t use_intrabc, uint8_t is_masked_compound,
                                      uint8_t is16bit, bool is_wm, WarpedMotionParams* wm_params);

EbPictureBufferDesc* svt_aom_get_ref_pic_buffer(PictureControlSet* pcs, MvReferenceFrame rf);
void                 svt_aom_get_recon_pic(PictureControlSet* pcs, EbPictureBufferDesc** recon_ptr, bool is_highbd);
EbErrorType          svt_aom_inter_pu_prediction_av1_obmc(uint8_t hbd_md, struct ModeDecisionContext* ctx,
                                                          PictureControlSet* pcs, ModeDecisionCandidateBuffer* cand_bf);

#ifdef __cplusplus
}
#endif
#endif //EbEncInterPrediction_h
