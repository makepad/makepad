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

#ifndef EbFullLoop_h
#define EbFullLoop_h

#include "md_process.h"
#include "common_utils.h"
#include "inv_transforms.h"
#include "transforms.h"
#include "definitions.h"

#ifdef __cplusplus
extern "C" {
#endif

void    svt_aom_full_loop_chroma_light_pd1(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                           ModeDecisionCandidateBuffer* cand_bf, EbPictureBufferDesc* input_pic,
                                           uint32_t input_cb_origin_in_index, uint32_t blk_chroma_origin_index,
                                           COMPONENT_TYPE component_type, uint32_t chroma_qindex,
                                           uint64_t cb_full_distortion[DIST_CALC_TOTAL],
                                           uint64_t cr_full_distortion[DIST_CALC_TOTAL], uint64_t* cb_coeff_bits,
                                           uint64_t* cr_coeff_bits);
void    svt_aom_full_loop_uv(PictureControlSet* pcs, ModeDecisionContext* ctx, ModeDecisionCandidateBuffer* cand_bf,
                             EbPictureBufferDesc* input_pic, COMPONENT_TYPE component_type, uint32_t chroma_qindex,
                             uint64_t cb_full_distortion[DIST_TOTAL][DIST_CALC_TOTAL],
                             uint64_t cr_full_distortion[DIST_TOTAL][DIST_CALC_TOTAL], uint64_t* cb_coeff_bits,
                             uint64_t* cr_coeff_bits, bool is_full_loop);
void    svt_aom_inv_transform_recon_wrapper(PictureControlSet* pcs, ModeDecisionContext* ctx, uint8_t* pred_buffer,
                                            uint32_t pred_offset, uint32_t pred_stride, uint8_t* rec_buffer,
                                            uint32_t rec_offset, uint32_t rec_stride, int32_t* rec_coeff_buffer,
                                            uint32_t coeff_offset, bool hbd, TxSize txsize, TxType transform_type,
                                            PlaneType component_type, uint32_t eob);
uint8_t svt_aom_do_md_recon(PictureParentControlSet* pcs, ModeDecisionContext* ctx);

extern const int av1_get_tx_scale_tab[TX_SIZES_ALL];
#ifdef __cplusplus
}
#endif
#endif // EbFullLoop_h
