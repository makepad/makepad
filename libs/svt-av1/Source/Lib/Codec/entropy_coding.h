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

#ifndef EbEntropyCoding_h
#define EbEntropyCoding_h

#include "definitions.h"
#include "ec_process.h"
#include "coding_unit.h"
#include "pic_buffer_desc.h"
#include "sequence_control_set.h"
#include "pcs.h"
#include "cabac_context_model.h"
#include "mode_decision.h"
#include "enc_intra_prediction.h"
#include "bitstream_unit.h"
#include "packetization_process.h"
#include "md_process.h"
#include "inter_prediction.h"
#include "EbSvtAv1Metadata.h"
#include "common_utils.h"
#ifdef __cplusplus
extern "C" {
#endif

/**************************************
 * Extern Function Declarations
 **************************************/
void svt_aom_write_modes_sb(EntropyCodingContext* ec_ctx, SuperBlock* sb_ptr, PictureControlSet* pcs, uint16_t tile_idx,
                            EntropyCoder* ec, EbPictureBufferDesc* coeff_ptr, struct PARTITION_TREE* ptree, int mi_row,
                            int mi_col);

int svt_aom_get_wedge_params_bits(BlockSize bsize);

EbErrorType svt_aom_encode_slice_finish(EntropyCoder* ec);

EbErrorType svt_aom_reset_entropy_coder(EncodeContext* enc_ctx, EntropyCoder* ec, uint32_t qp, SliceType slice_type);
EbErrorType svt_aom_txb_estimate_coeff_bits(ModeDecisionContext* ctx, uint8_t allow_update_cdf, FRAME_CONTEXT* ec_ctx,
                                            PictureControlSet* pcs, ModeDecisionCandidateBuffer* cand_bf,
                                            uint32_t txb_origin_index, uint32_t txb_chroma_origin_index,
                                            EbPictureBufferDesc* coeff_buffer_sb, uint32_t y_eob, uint32_t cb_eob,
                                            uint32_t cr_eob, uint64_t* y_txb_coeff_bits, uint64_t* cb_txb_coeff_bits,
                                            uint64_t* cr_txb_coeff_bits, TxSize txsize, TxSize txsize_uv,
                                            TxType tx_type, TxType tx_type_uv, COMPONENT_TYPE component_type);

EbErrorType svt_aom_txb_estimate_coeff_bits_light_pd0(ModeDecisionContext* ctx, ModeDecisionCandidateBuffer* cand_bf,
                                                      uint32_t txb_origin_index, EbPictureBufferDesc* coeff_buffer_sb,
                                                      uint32_t y_eob, uint64_t* y_txb_coeff_bits, TxSize txsize);

//**********************************************************************************************************//
// onyxc_int.h
static INLINE int32_t frame_is_intra_only(const PictureParentControlSet* const pcs) {
    return pcs->frm_hdr.frame_type == KEY_FRAME || pcs->frm_hdr.frame_type == INTRA_ONLY_FRAME;
}

static INLINE int32_t frame_is_sframe(const PictureParentControlSet* pcs) {
    return pcs->frm_hdr.frame_type == S_FRAME;
}

// Returns 1 if this frame might allow mvs from some reference frame.

static INLINE int32_t frame_might_allow_ref_frame_mvs(const PictureParentControlSet* pcs, SequenceControlSet* scs) {
    return !pcs->frm_hdr.error_resilient_mode && scs->seq_header.order_hint_info.enable_ref_frame_mvs &&
        scs->seq_header.order_hint_info.enable_order_hint && !frame_is_intra_only(pcs);
}

// Returns 1 if this frame might use warped_motion
static INLINE int32_t frame_might_allow_warped_motion(const PictureParentControlSet* pcs, SequenceControlSet* scs) {
    return !pcs->frm_hdr.error_resilient_mode && !frame_is_intra_only(pcs) && scs->seq_header.enable_warped_motion;
}

static INLINE uint8_t major_minor_to_seq_level_idx(BitstreamLevel bl) {
    assert(bl.major >= LEVEL_MAJOR_MIN && bl.major <= LEVEL_MAJOR_MAX);
    return ((bl.major - LEVEL_MAJOR_MIN) << LEVEL_MINOR_BITS) + bl.minor;
}

static INLINE void set_dc_sign(int32_t* cul_level, int32_t dc_val) {
    if (dc_val < 0) {
        *cul_level |= 1 << COEFF_CONTEXT_BITS;
    } else if (dc_val > 0) {
        *cul_level += 2 << COEFF_CONTEXT_BITS;
    }
}

extern const uint8_t eob_to_pos_small[33];
extern const int16_t eob_group_start[12];
extern const int16_t svt_aom_eob_offset_bits[12];
extern const uint8_t eob_to_pos_large[17];

static INLINE int get_eob_pos_token(const int eob, int* const extra) {
    int t;

    if (eob < 33) {
        t = eob_to_pos_small[eob];
    } else {
        const int e = MIN((eob - 1) >> 5, 16);
        t           = eob_to_pos_large[e];
    }

    *extra = eob - eob_group_start[t];

    return t;
}

//**********************************************************************************************************//
//encoder.h
static INLINE int32_t get_ref_frame_map_idx(const PictureParentControlSet* pcs, MvReferenceFrame ref_frame) {
    return pcs->av1_ref_signal.ref_dpb_index[ref_frame - LAST_FRAME]; //LAST-LAST2-LAST3-GOLDEN-BWD-ALT2-ALT
}

static INLINE TxSize get_txsize_entropy_ctx(TxSize txsize) {
    return (TxSize)((txsize_sqr_map[txsize] + txsize_sqr_up_map[txsize] + 1) >> 1);
}

//*******************************************************************************************//
// bitwriter_buffer.h
typedef struct AomWriteBitBuffer {
    uint8_t* bit_buffer;
    uint32_t bit_offset;
} AomWriteBitBuffer;

int32_t  svt_aom_wb_is_byte_aligned(const AomWriteBitBuffer* wb);
uint32_t svt_aom_wb_bytes_written(const AomWriteBitBuffer* wb);

void svt_aom_wb_write_bit(AomWriteBitBuffer* wb, int32_t bit);
void svt_aom_wb_write_literal(AomWriteBitBuffer* wb, int32_t data, int32_t bits);

void svt_aom_wb_write_inv_signed_literal(AomWriteBitBuffer* wb, int32_t data, int32_t bits);

//*******************************************************************************************//
// blockd.h

void svt_aom_get_txb_ctx(PictureControlSet* pcs, const int32_t plane,
                         NeighborArrayUnit* dc_sign_level_coeff_neighbor_array, uint32_t blk_org_x, uint32_t blk_org_y,
                         const BlockSize plane_bsize, const TxSize tx_size, int16_t* const txb_skip_ctx,
                         int16_t* const dc_sign_ctx);

void svt_aom_collect_neighbors_ref_counts_new(MacroBlockD* const xd);

// == Context functions for comp ref ==
//
// Returns a context number for the given MB prediction signal
// Signal the first reference frame for a compound mode be either
// GOLDEN/LAST3, or LAST/LAST2.
int32_t svt_av1_get_pred_context_comp_ref_p(const MacroBlockD* xd);

// Returns a context number for the given MB prediction signal
// Signal the first reference frame for a compound mode be LAST,
// conditioning on that it is known either LAST/LAST2.
int32_t svt_av1_get_pred_context_comp_ref_p1(const MacroBlockD* xd);

// Returns a context number for the given MB prediction signal
// Signal the first reference frame for a compound mode be GOLDEN,
// conditioning on that it is known either GOLDEN or LAST3.
int32_t svt_av1_get_pred_context_comp_ref_p2(const MacroBlockD* xd);

// Signal the 2nd reference frame for a compound mode be either
// ALTREF, or ALTREF2/BWDREF.
int32_t svt_av1_get_pred_context_comp_bwdref_p(const MacroBlockD* xd);

// Signal the 2nd reference frame for a compound mode be either
// ALTREF2 or BWDREF.
int32_t svt_av1_get_pred_context_comp_bwdref_p1(const MacroBlockD* xd);
// == Context functions for single ref ==
//
// For the bit to signal whether the single reference is a forward reference
// frame or a backward reference frame.
int32_t svt_av1_get_pred_context_single_ref_p1(const MacroBlockD* xd);

// For the bit to signal whether the single reference is ALTREF_FRAME or
// non-ALTREF backward reference frame, knowing that it shall be either of
// these 2 choices.
int32_t svt_av1_get_pred_context_single_ref_p2(const MacroBlockD* xd);

// For the bit to signal whether the single reference is LAST3/GOLDEN or
// LAST2/LAST, knowing that it shall be either of these 2 choices.
int32_t svt_av1_get_pred_context_single_ref_p3(const MacroBlockD* xd);

// For the bit to signal whether the single reference is LAST2_FRAME or
// LAST_FRAME, knowing that it shall be either of these 2 choices.
int32_t svt_av1_get_pred_context_single_ref_p4(const MacroBlockD* xd);

// For the bit to signal whether the single reference is GOLDEN_FRAME or
// LAST3_FRAME, knowing that it shall be either of these 2 choices.
int32_t svt_av1_get_pred_context_single_ref_p5(const MacroBlockD* xd);

// For the bit to signal whether the single reference is ALTREF2_FRAME or
// BWDREF_FRAME, knowing that it shall be either of these 2 choices.
int32_t svt_av1_get_pred_context_single_ref_p6(const MacroBlockD* xd);

/*!\brief Writes a valid metadata object to the AV1 bitstream.
 * \param[in]    bitstream_ptr       AV1 bitstream
 * \param[in]    metadata            Metadata array object
 * \param[in]    type                Metadata type descriptor
 */
EbErrorType svt_aom_write_metadata_av1(Bitstream* bitstream_ptr, SvtMetadataArrayT* metadata,
                                       const EbAv1MetadataType type);
EbErrorType svt_aom_write_frame_header_av1(Bitstream* bitstream_ptr, SequenceControlSet* scs, PictureControlSet* pcs,
                                           uint8_t show_existing);
EbErrorType svt_aom_encode_td_av1(uint8_t* bitstream_ptr);
EbErrorType svt_aom_encode_sps_av1(Bitstream* bitstream_ptr, SequenceControlSet* scs);

//*******************************************************************************************//
MotionMode svt_aom_motion_mode_allowed(const PictureControlSet* pcs, uint16_t num_proj_ref,
                                       uint32_t overlappable_neighbors, const BlockSize bsize, MvReferenceFrame rf0,
                                       MvReferenceFrame rf1, PredictionMode mode);
int        svt_aom_is_masked_compound_type(COMPOUND_TYPE type);

int32_t svt_aom_count_primitive_subexpfin(uint16_t n, uint16_t k, uint16_t v);
int32_t svt_aom_count_primitive_refsubexpfin(uint16_t n, uint16_t k, uint16_t ref, uint16_t v);
int     svt_aom_get_comp_index_context_enc(PictureParentControlSet* pcs, int cur_frame_index, int bck_frame_index,
                                           int fwd_frame_index, const MacroBlockD* xd);
int     svt_aom_get_pred_context_switchable_interp(MvReferenceFrame rf0, MvReferenceFrame rf1, const MacroBlockD* xd,
                                                   int dir);
int     svt_aom_is_nontrans_global_motion(const BlockModeInfo* block_mi, const BlockSize bsize,
                                          PictureParentControlSet* pcs);
uint8_t svt_av1_get_intra_inter_context(const MacroBlockD* xd);
void    svt_aom_get_kf_y_mode_ctx(const MacroBlockD* xd, uint8_t* above_ctx, uint8_t* left_ctx);
uint8_t av1_get_skip_mode_context(const MacroBlockD* xd);
uint8_t av1_get_skip_context(const MacroBlockD* xd);
void    svt_av1_reset_loop_restoration(EntropyCodingContext* ctx);

#ifdef __cplusplus
}
#endif
#endif //EbEntropyCoding_h
