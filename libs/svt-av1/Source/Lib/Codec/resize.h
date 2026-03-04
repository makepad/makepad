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

#ifndef EbResize_h
#define EbResize_h

#include "definitions.h"
#include "pic_buffer_desc.h"
#include "inter_prediction.h"
#include "sequence_control_set.h"
#include "super_res.h"
#include "reference_object.h"

#ifdef __cplusplus
extern "C" {
#endif

extern const int16_t      svt_aom_av1_down2_symeven_half_filter[4];
extern const int16_t      av1_down2_symodd_half_filter[4];
extern const InterpKernel svt_aom_av1_filteredinterp_filters500[(1 << RS_SUBPEL_BITS)];
extern const InterpKernel svt_aom_av1_filteredinterp_filters625[(1 << RS_SUBPEL_BITS)];
extern const InterpKernel svt_aom_av1_filteredinterp_filters750[(1 << RS_SUBPEL_BITS)];
extern const InterpKernel svt_aom_av1_filteredinterp_filters875[(1 << RS_SUBPEL_BITS)];

typedef struct {
    uint16_t encoding_width;
    uint16_t encoding_height;
    uint8_t  superres_denom;
} superres_params_type;

void scale_source_references(SequenceControlSet* scs, PictureParentControlSet* pcs, EbPictureBufferDesc* input_pic);

void svt_aom_scale_rec_references(PictureControlSet* pcs, EbPictureBufferDesc* input_pic);

void svt_aom_use_scaled_rec_refs_if_needed(PictureControlSet* pcs, EbPictureBufferDesc* input_pic,
                                           EbReferenceObject* ref_obj, EbPictureBufferDesc** ref_pic, uint8_t hbd_md);

void svt_aom_use_scaled_source_refs_if_needed(PictureParentControlSet* pcs, EbPictureBufferDesc* input_pic,
                                              EbPaReferenceObject* ref_obj, EbPictureBufferDesc** ref_pic_ptr,
                                              EbPictureBufferDesc** quarter_ref_pic_ptr,
                                              EbPictureBufferDesc** sixteenth_ref_pic_ptr);

void scale_pcs_params(SequenceControlSet* scs, PictureParentControlSet* pcs, superres_params_type spr_params,
                      uint16_t source_width, uint16_t source_height);

// resize picture for both super-res and scaling-ref
void svt_aom_init_resize_picture(SequenceControlSet* scs, PictureParentControlSet* pcs);

void svt_aom_reset_resized_picture(SequenceControlSet* scs, PictureParentControlSet* pcs,
                                   EbPictureBufferDesc* input_pic);

uint8_t svt_aom_get_denom_idx(uint8_t scale_denom);

EbErrorType svt_aom_downscaled_source_buffer_desc_ctor(EbPictureBufferDesc** picture_ptr,
                                                       EbPictureBufferDesc*  picture_ptr_for_reference,
                                                       superres_params_type  spr_params);

EbErrorType svt_aom_resize_frame(const EbPictureBufferDesc* src, EbPictureBufferDesc* dst, int bd, const int num_planes,
                                 const uint32_t ss_x, const uint32_t ss_y, uint8_t is_packed,
                                 uint32_t buffer_enable_mask, uint8_t is_2bcompress);

static INLINE int coded_to_superres_mi(int mi_col, int denom) {
    return (mi_col * denom + SCALE_NUMERATOR / 2) / SCALE_NUMERATOR;
}

#define filteredinterp_filters1000 svt_av1_resize_filter_normative

#ifdef __cplusplus
}
#endif
#endif // EbResize_h
