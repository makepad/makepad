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

#ifndef EbReferenceObject_h
#define EbReferenceObject_h

#include "definitions.h"
#include "object.h"
#include "cabac_context_model.h"
#include "coding_unit.h"
#include "sequence_control_set.h"

typedef struct EbReferenceObject {
    EbDctor              dctor;
    EbPictureBufferDesc* reference_picture;
    EbPictureBufferDesc* downscaled_reference_picture[NUM_SR_SCALES + 1][NUM_RESIZE_SCALES + 1];
    uint64_t             downscaled_picture_number[NUM_SR_SCALES + 1]
                                      [NUM_RESIZE_SCALES + 1]; // save the picture_number for each denom
    EbHandle           resize_mutex[NUM_SR_SCALES + 1][NUM_RESIZE_SCALES + 1];
    uint64_t           ref_poc;
    uint8_t            base_q_idx;
    SliceType          slice_type;
    uint8_t            intra_coded_area; //percentage of intra coded area 0-100%
    uint8_t            skip_coded_area;
    uint8_t            hp_coded_area;
    uint8_t            is_mfmv_used;
    uint8_t            tmp_layer_idx;
    bool               is_scene_change;
    uint16_t           pic_avg_variance;
    AomFilmGrain       film_grain_params; //Film grain parameters for a reference frame
    FRAME_CONTEXT      frame_context;
    WarpedMotionParams global_motion[TOTAL_REFS_PER_FRAME];
    MV_REF*            mvs;
    FrameType          frame_type;
    uint32_t           order_hint;
    uint32_t           ref_order_hint[7];
    double             r0;
    int32_t            filter_level[2];
    int32_t            filter_level_u;
    int32_t            filter_level_v;
    int32_t            dlf_dist_dev;
    int32_t            cdef_dist_dev;
    uint32_t           ref_cdef_strengths_num;
    uint8_t            ref_cdef_strengths[2][TOTAL_STRENGTHS];
    uint8_t*           sb_intra;
    uint8_t*           sb_skip;
    uint8_t*           sb_64x64_mvp;
    uint32_t*          sb_me_64x64_dist;
    uint32_t*          sb_me_8x8_cost_var;
    uint8_t*           sb_min_sq_size;
    uint8_t*           sb_max_sq_size;
    int32_t            mi_cols;
    int32_t            mi_rows;
    WienerUnitInfo**   unit_info; // per plane, per rest. unit; used for fwding wiener info to future frames
} EbReferenceObject;

typedef struct EbReferenceObjectDescInitData {
    EbPictureBufferDescInitData reference_picture_desc_init_data;
    int8_t                      hbd_md;
    EbSvtAv1EncConfiguration*   static_config;
} EbReferenceObjectDescInitData;

typedef struct EbPaReferenceObject {
    EbDctor              dctor;
    EbPictureBufferDesc* input_padded_pic;
    EbPictureBufferDesc* quarter_downsampled_picture_ptr;
    EbPictureBufferDesc* sixteenth_downsampled_picture_ptr;
    // downscaled reference pointers
    // [super-res scales][resize scales]
    EbPictureBufferDesc* downscaled_input_padded_picture_ptr[NUM_SR_SCALES + 1][NUM_RESIZE_SCALES + 1];
    EbPictureBufferDesc* downscaled_quarter_downsampled_picture_ptr[NUM_SR_SCALES + 1][NUM_RESIZE_SCALES + 1];
    EbPictureBufferDesc* downscaled_sixteenth_downsampled_picture_ptr[NUM_SR_SCALES + 1][NUM_RESIZE_SCALES + 1];
    uint64_t             downscaled_picture_number[NUM_SR_SCALES + 1]
                                      [NUM_RESIZE_SCALES + 1]; // save the picture_number for each denom
    EbHandle resize_mutex[NUM_SR_SCALES + 1][NUM_RESIZE_SCALES + 1];
    uint64_t picture_number;
    uint64_t avg_luma;
    uint8_t  dummy_obj;
} EbPaReferenceObject;

typedef struct EbPaReferenceObjectDescInitData {
    EbPictureBufferDescInitData reference_picture_desc_init_data;
    EbPictureBufferDescInitData quarter_picture_desc_init_data;
    EbPictureBufferDescInitData sixteenth_picture_desc_init_data;
} EbPaReferenceObjectDescInitData;

typedef struct EbTplReferenceObject {
    EbDctor              dctor;
    EbPictureBufferDesc* ref_picture_ptr;
} EbTplReferenceObject;

typedef struct EbTplReferenceObjectDescInitData {
    EbPictureBufferDescInitData reference_picture_desc_init_data;
} EbTplReferenceObjectDescInitData;

/**************************************
 * Extern Function Declarations
 **************************************/
EbErrorType svt_reference_object_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr);
EbErrorType svt_reference_object_reset(EbReferenceObject* obj, SequenceControlSet* scs);

EbErrorType svt_pa_reference_object_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr);
EbErrorType svt_tpl_reference_object_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr);
void        svt_aom_release_pa_reference_objects(SequenceControlSet* scs, PictureParentControlSet* pcs);
EbErrorType svt_pa_reference_param_update(EbPaReferenceObject* pa_ref_obj_, SequenceControlSet* scs);
EbErrorType svt_tpl_reference_param_update(EbTplReferenceObject* tpl_ref_obj, SequenceControlSet* scs);
EbErrorType svt_reference_param_update(EbReferenceObject* ref_object, SequenceControlSet* scs);

#endif //EbReferenceObject_h
