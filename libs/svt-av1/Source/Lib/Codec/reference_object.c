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

#include <stdlib.h>
#include <string.h>

#include "svt_threads.h"
#include "reference_object.h"
#include "pic_buffer_desc.h"
#include "utility.h"
#include "enc_mode_config.h"

void initialize_samples_neighboring_reference_picture_8bit(EbByte recon_samples_buffer_ptr, uint16_t stride,
                                                           uint16_t recon_width, uint16_t recon_height,
                                                           uint16_t left_padding, uint16_t top_padding) {
    uint8_t* recon_samples_ptr;
    uint16_t sample_count;

    // 1. zero out the top row
    recon_samples_ptr = recon_samples_buffer_ptr + (top_padding - 1) * stride + left_padding - 1;
    svt_memset(recon_samples_ptr, 0, sizeof(uint8_t) * (1 + recon_width + 1));

    // 2. zero out the bottom row
    recon_samples_ptr = recon_samples_buffer_ptr + (top_padding + recon_height) * stride + left_padding - 1;
    svt_memset(recon_samples_ptr, 0, sizeof(uint8_t) * (1 + recon_width + 1));

    // 3. zero out the left column
    recon_samples_ptr = recon_samples_buffer_ptr + top_padding * stride + left_padding - 1;
    for (sample_count = 0; sample_count < recon_height; sample_count++) {
        recon_samples_ptr[sample_count * stride] = 0;
    }
    // 4. zero out the right column
    recon_samples_ptr = recon_samples_buffer_ptr + top_padding * stride + left_padding + recon_width;
    for (sample_count = 0; sample_count < recon_height; sample_count++) {
        recon_samples_ptr[sample_count * stride] = 0;
    }
}

static void initialize_samples_neighboring_reference_picture(
    EbReferenceObject* ref_object, EbPictureBufferDescInitData* picture_buffer_desc_init_data_ptr,
    EbBitDepth bit_depth) {
    UNUSED(bit_depth);
    {
        initialize_samples_neighboring_reference_picture_8bit(ref_object->reference_picture->buffer_y,
                                                              ref_object->reference_picture->stride_y,
                                                              ref_object->reference_picture->width,
                                                              ref_object->reference_picture->height,
                                                              picture_buffer_desc_init_data_ptr->left_padding,
                                                              picture_buffer_desc_init_data_ptr->top_padding);

        initialize_samples_neighboring_reference_picture_8bit(ref_object->reference_picture->buffer_cb,
                                                              ref_object->reference_picture->stride_cb,
                                                              ref_object->reference_picture->width >> 1,
                                                              ref_object->reference_picture->height >> 1,
                                                              picture_buffer_desc_init_data_ptr->left_padding >> 1,
                                                              picture_buffer_desc_init_data_ptr->top_padding >> 1);

        initialize_samples_neighboring_reference_picture_8bit(ref_object->reference_picture->buffer_cr,
                                                              ref_object->reference_picture->stride_cr,
                                                              ref_object->reference_picture->width >> 1,
                                                              ref_object->reference_picture->height >> 1,
                                                              picture_buffer_desc_init_data_ptr->left_padding >> 1,
                                                              picture_buffer_desc_init_data_ptr->top_padding >> 1);
    }
}

static void svt_reference_object_dctor(EbPtr p) {
    EbReferenceObject* obj = (EbReferenceObject*)p;

    EB_DELETE(obj->reference_picture);
    EB_FREE_2D(obj->unit_info);
    EB_FREE_ALIGNED_ARRAY(obj->mvs);
    EB_FREE_ARRAY(obj->sb_intra);
    EB_FREE_ARRAY(obj->sb_skip);
    EB_FREE_ARRAY(obj->sb_64x64_mvp);
    EB_FREE_ARRAY(obj->sb_me_64x64_dist);
    EB_FREE_ARRAY(obj->sb_me_8x8_cost_var);
    EB_FREE_ARRAY(obj->sb_min_sq_size);
    EB_FREE_ARRAY(obj->sb_max_sq_size);
    for (uint8_t sr_denom_idx = 0; sr_denom_idx < NUM_SR_SCALES + 1; sr_denom_idx++) {
        for (uint8_t resize_denom_idx = 0; resize_denom_idx < NUM_RESIZE_SCALES + 1; resize_denom_idx++) {
            if (obj->downscaled_reference_picture[sr_denom_idx][resize_denom_idx] != NULL) {
                EB_DELETE(obj->downscaled_reference_picture[sr_denom_idx][resize_denom_idx]);
            }
            EB_DESTROY_MUTEX(obj->resize_mutex[sr_denom_idx][resize_denom_idx]);
        }
    }
}

/*
svt_reference_param_update: update the parameters in EbReferenceObject for changing the resolution on the fly
*/
EbErrorType svt_reference_param_update(EbReferenceObject* ref_object, SequenceControlSet* scs) {
    EbPictureBufferDescInitData picture_buffer_desc_init_data_ptr;

    bool is_16bit = scs->static_config.encoder_bit_depth > EB_EIGHT_BIT;
    // Initialize the various Picture types
    picture_buffer_desc_init_data_ptr.max_width           = scs->max_input_luma_width;
    picture_buffer_desc_init_data_ptr.max_height          = scs->max_input_luma_height;
    picture_buffer_desc_init_data_ptr.bit_depth           = scs->encoder_bit_depth;
    picture_buffer_desc_init_data_ptr.color_format        = scs->static_config.encoder_color_format;
    picture_buffer_desc_init_data_ptr.buffer_enable_mask  = PICTURE_BUFFER_DESC_FULL_MASK;
    picture_buffer_desc_init_data_ptr.rest_units_per_tile = scs->rest_units_per_tile;
    picture_buffer_desc_init_data_ptr.sb_total_count      = scs->b64_total_count;
    uint16_t padding                                      = scs->super_block_size + 32;
    if (scs->static_config.superres_mode > SUPERRES_NONE || scs->static_config.resize_mode > RESIZE_NONE) {
        padding += scs->super_block_size;
    }

    picture_buffer_desc_init_data_ptr.left_padding      = padding;
    picture_buffer_desc_init_data_ptr.right_padding     = padding;
    picture_buffer_desc_init_data_ptr.top_padding       = padding;
    picture_buffer_desc_init_data_ptr.bot_padding       = padding;
    picture_buffer_desc_init_data_ptr.mfmv              = scs->mfmv_enabled;
    picture_buffer_desc_init_data_ptr.is_16bit_pipeline = scs->is_16bit_pipeline;

    picture_buffer_desc_init_data_ptr.split_mode = false;
    if (is_16bit) {
        picture_buffer_desc_init_data_ptr.bit_depth = EB_TEN_BIT;
    }

    EbPictureBufferDescInitData picture_buffer_desc_init_data_16bit_ptr = picture_buffer_desc_init_data_ptr;
    //TODO:12bit
    if (picture_buffer_desc_init_data_ptr.bit_depth == EB_TEN_BIT) {
        picture_buffer_desc_init_data_16bit_ptr.split_mode = true;
        svt_picture_buffer_desc_update(ref_object->reference_picture, (EbPtr)&picture_buffer_desc_init_data_16bit_ptr);
    } else {
        // Hsan: set split_mode to 0 to as 8BIT input
        picture_buffer_desc_init_data_ptr.split_mode = false;
        svt_picture_buffer_desc_update(ref_object->reference_picture, (EbPtr)&picture_buffer_desc_init_data_ptr);

        initialize_samples_neighboring_reference_picture(
            ref_object, &picture_buffer_desc_init_data_ptr, picture_buffer_desc_init_data_ptr.bit_depth);
    }

    ref_object->mi_rows = ref_object->reference_picture->height >> MI_SIZE_LOG2;
    ref_object->mi_cols = ref_object->reference_picture->width >> MI_SIZE_LOG2;
    return EB_ErrorNone;
}

/*****************************************
 * svt_picture_buffer_desc_ctor
 *  Initializes the Buffer Descriptor's
 *  values that are fixed for the life of
 *  the descriptor.
 *****************************************/
EbErrorType svt_reference_object_ctor(EbReferenceObject* ref_object, EbPtr object_init_data_ptr) {
    EbReferenceObjectDescInitData* ref_init_ptr = (EbReferenceObjectDescInitData*)object_init_data_ptr;
    EbPictureBufferDescInitData*   picture_buffer_desc_init_data_ptr = &ref_init_ptr->reference_picture_desc_init_data;
    EbPictureBufferDescInitData    picture_buffer_desc_init_data_16bit_ptr = *picture_buffer_desc_init_data_ptr;

    ref_object->dctor = svt_reference_object_dctor;
    //TODO:12bit
    if (picture_buffer_desc_init_data_16bit_ptr.bit_depth == EB_TEN_BIT) {
        // Hsan: set split_mode to 0 to construct the packed reference buffer (used @ EP)
        // Use 10bit here to use in MD
        picture_buffer_desc_init_data_16bit_ptr.split_mode = true;
        picture_buffer_desc_init_data_16bit_ptr.bit_depth  = EB_TEN_BIT;
        EB_NEW(ref_object->reference_picture,
               svt_picture_buffer_desc_ctor,
               (EbPtr)&picture_buffer_desc_init_data_16bit_ptr);
    } else {
        // Hsan: set split_mode to 0 to as 8BIT input
        picture_buffer_desc_init_data_ptr->split_mode = false;
        EB_NEW(ref_object->reference_picture, svt_picture_buffer_desc_ctor, (EbPtr)picture_buffer_desc_init_data_ptr);

        initialize_samples_neighboring_reference_picture(
            ref_object, picture_buffer_desc_init_data_ptr, picture_buffer_desc_init_data_16bit_ptr.bit_depth);
    }
    uint32_t mi_rows = ref_object->reference_picture->height >> MI_SIZE_LOG2;
    uint32_t mi_cols = ref_object->reference_picture->width >> MI_SIZE_LOG2;
    // there should be one unit info per plane and per rest unit
    EB_MALLOC_2D(ref_object->unit_info, MAX_MB_PLANE, picture_buffer_desc_init_data_ptr->rest_units_per_tile);

    if (picture_buffer_desc_init_data_ptr->mfmv) {
        //MFMV map is 8x8 based.
        const int mem_size = ((mi_rows + 1) >> 1) * ((mi_cols + 1) >> 1);
        EB_CALLOC_ALIGNED_ARRAY(ref_object->mvs, mem_size);
    }
    svt_memset(&ref_object->film_grain_params, 0, sizeof(ref_object->film_grain_params));
    // set all supplemental downscaled reference picture pointers to NULL
    for (uint8_t sr_denom_idx = 0; sr_denom_idx < NUM_SR_SCALES + 1; sr_denom_idx++) {
        for (uint8_t resize_denom_idx = 0; resize_denom_idx < NUM_RESIZE_SCALES + 1; resize_denom_idx++) {
            ref_object->downscaled_reference_picture[sr_denom_idx][resize_denom_idx] = NULL;
            ref_object->downscaled_picture_number[sr_denom_idx][resize_denom_idx]    = (uint64_t)~0;
            EB_CREATE_MUTEX(ref_object->resize_mutex[sr_denom_idx][resize_denom_idx]);
        }
    }

    ref_object->mi_rows = mi_rows;
    ref_object->mi_cols = mi_cols;
    EB_MALLOC_ARRAY(ref_object->sb_intra, picture_buffer_desc_init_data_ptr->sb_total_count);
    EB_MALLOC_ARRAY(ref_object->sb_skip, picture_buffer_desc_init_data_ptr->sb_total_count);
    EB_MALLOC_ARRAY(ref_object->sb_64x64_mvp, picture_buffer_desc_init_data_ptr->sb_total_count);
    EB_MALLOC_ARRAY(ref_object->sb_me_64x64_dist, picture_buffer_desc_init_data_ptr->sb_total_count);
    EB_MALLOC_ARRAY(ref_object->sb_me_8x8_cost_var, picture_buffer_desc_init_data_ptr->sb_total_count);
    EB_MALLOC_ARRAY(ref_object->sb_min_sq_size, picture_buffer_desc_init_data_ptr->sb_total_count);
    EB_MALLOC_ARRAY(ref_object->sb_max_sq_size, picture_buffer_desc_init_data_ptr->sb_total_count);
    return EB_ErrorNone;
}

EbErrorType svt_reference_object_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr) {
    EbReferenceObject* obj;

    *object_dbl_ptr = NULL;
    EB_NEW(obj, svt_reference_object_ctor, object_init_data_ptr);
    *object_dbl_ptr = obj;

    return EB_ErrorNone;
}

EbErrorType svt_reference_object_reset(EbReferenceObject* ref_object, SequenceControlSet* scs) {
    ref_object->mi_rows = scs->max_input_luma_height >> MI_SIZE_LOG2;
    ref_object->mi_cols = scs->max_input_luma_width >> MI_SIZE_LOG2;

    return EB_ErrorNone;
}

static void svt_pa_reference_object_dctor(EbPtr p) {
    EbPaReferenceObject* obj = (EbPaReferenceObject*)p;
    if (obj->dummy_obj) {
        return;
    }
    EB_DELETE(obj->input_padded_pic);
    EB_DELETE(obj->quarter_downsampled_picture_ptr);
    EB_DELETE(obj->sixteenth_downsampled_picture_ptr);
    for (uint8_t sr_denom_idx = 0; sr_denom_idx < NUM_SR_SCALES + 1; sr_denom_idx++) {
        for (uint8_t resize_denom_idx = 0; resize_denom_idx < NUM_RESIZE_SCALES + 1; resize_denom_idx++) {
            if (obj->downscaled_input_padded_picture_ptr[sr_denom_idx][resize_denom_idx] != NULL) {
                EB_DELETE(obj->downscaled_input_padded_picture_ptr[sr_denom_idx][resize_denom_idx]);
                EB_DELETE(obj->downscaled_quarter_downsampled_picture_ptr[sr_denom_idx][resize_denom_idx]);
                EB_DELETE(obj->downscaled_sixteenth_downsampled_picture_ptr[sr_denom_idx][resize_denom_idx]);
            }
            EB_DESTROY_MUTEX(obj->resize_mutex[sr_denom_idx][resize_denom_idx]);
        }
    }
}

static void svt_tpl_reference_object_dctor(EbPtr p) {
    EbTplReferenceObject* obj = (EbTplReferenceObject*)p;
    EB_DELETE(obj->ref_picture_ptr);
}

/*
svt_pa_reference_param_update: update the parameters in EbPaReferenceObject for changing the resolution on the fly
*/
EbErrorType svt_pa_reference_param_update(EbPaReferenceObject* pa_ref_obj, SequenceControlSet* scs) {
    EbPictureBufferDescInitData ref_pic_buf_desc_init_data;
    EbPictureBufferDescInitData quart_pic_buf_desc_init_data;
    EbPictureBufferDescInitData sixteenth_pic_buf_desc_init_data;
    // PA Reference Picture Buffers
    // Currently, only Luma samples are needed in the PA
    ref_pic_buf_desc_init_data.max_width    = scs->max_input_luma_width;
    ref_pic_buf_desc_init_data.max_height   = scs->max_input_luma_height;
    ref_pic_buf_desc_init_data.bit_depth    = EB_EIGHT_BIT;
    ref_pic_buf_desc_init_data.color_format = EB_YUV420; //use 420 for picture analysis
    //No full-resolution pixel data is allocated for PA REF,
    // it points directly to the Luma input samples of the app data
    ref_pic_buf_desc_init_data.buffer_enable_mask = 0;

    ref_pic_buf_desc_init_data.left_padding        = scs->left_padding;
    ref_pic_buf_desc_init_data.right_padding       = scs->right_padding;
    ref_pic_buf_desc_init_data.top_padding         = scs->top_padding;
    ref_pic_buf_desc_init_data.bot_padding         = scs->bot_padding;
    ref_pic_buf_desc_init_data.split_mode          = false;
    ref_pic_buf_desc_init_data.rest_units_per_tile = scs->rest_units_per_tile;
    ref_pic_buf_desc_init_data.mfmv                = 0;
    ref_pic_buf_desc_init_data.is_16bit_pipeline   = false;

    quart_pic_buf_desc_init_data.max_width           = scs->max_input_luma_width >> 1;
    quart_pic_buf_desc_init_data.max_height          = scs->max_input_luma_height >> 1;
    quart_pic_buf_desc_init_data.bit_depth           = EB_EIGHT_BIT;
    quart_pic_buf_desc_init_data.color_format        = EB_YUV420;
    quart_pic_buf_desc_init_data.buffer_enable_mask  = PICTURE_BUFFER_DESC_LUMA_MASK;
    quart_pic_buf_desc_init_data.left_padding        = scs->b64_size >> 1;
    quart_pic_buf_desc_init_data.right_padding       = scs->b64_size >> 1;
    quart_pic_buf_desc_init_data.top_padding         = scs->b64_size >> 1;
    quart_pic_buf_desc_init_data.bot_padding         = scs->b64_size >> 1;
    quart_pic_buf_desc_init_data.split_mode          = false;
    quart_pic_buf_desc_init_data.rest_units_per_tile = scs->rest_units_per_tile;
    quart_pic_buf_desc_init_data.mfmv                = 0;
    quart_pic_buf_desc_init_data.is_16bit_pipeline   = false;

    sixteenth_pic_buf_desc_init_data.max_width           = scs->max_input_luma_width >> 2;
    sixteenth_pic_buf_desc_init_data.max_height          = scs->max_input_luma_height >> 2;
    sixteenth_pic_buf_desc_init_data.bit_depth           = EB_EIGHT_BIT;
    sixteenth_pic_buf_desc_init_data.color_format        = EB_YUV420;
    sixteenth_pic_buf_desc_init_data.buffer_enable_mask  = PICTURE_BUFFER_DESC_LUMA_MASK;
    sixteenth_pic_buf_desc_init_data.left_padding        = scs->b64_size >> 2;
    sixteenth_pic_buf_desc_init_data.right_padding       = scs->b64_size >> 2;
    sixteenth_pic_buf_desc_init_data.top_padding         = scs->b64_size >> 2;
    sixteenth_pic_buf_desc_init_data.bot_padding         = scs->b64_size >> 2;
    sixteenth_pic_buf_desc_init_data.split_mode          = false;
    sixteenth_pic_buf_desc_init_data.rest_units_per_tile = scs->rest_units_per_tile;
    sixteenth_pic_buf_desc_init_data.mfmv                = 0;
    sixteenth_pic_buf_desc_init_data.is_16bit_pipeline   = false;

    // Reference picture constructor
    svt_picture_buffer_desc_update(pa_ref_obj->input_padded_pic, (EbPtr)&ref_pic_buf_desc_init_data);
    // Downsampled reference picture constructor
    svt_picture_buffer_desc_update(pa_ref_obj->quarter_downsampled_picture_ptr, (EbPtr)&quart_pic_buf_desc_init_data);
    svt_picture_buffer_desc_update(pa_ref_obj->sixteenth_downsampled_picture_ptr,
                                   (EbPtr)&sixteenth_pic_buf_desc_init_data);
    return EB_ErrorNone;
}

/*****************************************
 * svt_pa_reference_object_ctor
 *  Initializes the Buffer Descriptor's
 *  values that are fixed for the life of
 *  the descriptor.
 *****************************************/
EbErrorType svt_pa_reference_object_ctor(EbPaReferenceObject* pa_ref_obj_, EbPtr object_init_data_ptr) {
    EbPictureBufferDescInitData* picture_buffer_desc_init_data_ptr = (EbPictureBufferDescInitData*)object_init_data_ptr;

    pa_ref_obj_->dctor = svt_pa_reference_object_dctor;

    // Reference picture constructor
    EB_NEW(pa_ref_obj_->input_padded_pic, svt_picture_buffer_desc_ctor, (EbPtr)picture_buffer_desc_init_data_ptr);
    // Downsampled reference picture constructor
    if (picture_buffer_desc_init_data_ptr[1].buffer_enable_mask) {
        EB_NEW(pa_ref_obj_->quarter_downsampled_picture_ptr,
               svt_picture_buffer_desc_ctor,
               (EbPtr)(picture_buffer_desc_init_data_ptr + 1));
    }
    if (picture_buffer_desc_init_data_ptr[2].buffer_enable_mask) {
        EB_NEW(pa_ref_obj_->sixteenth_downsampled_picture_ptr,
               svt_picture_buffer_desc_ctor,
               (EbPtr)(picture_buffer_desc_init_data_ptr + 2));
    }
    // set all supplemental downscaled reference picture pointers to NULL
    for (uint8_t sr_down_idx = 0; sr_down_idx < NUM_SR_SCALES + 1; sr_down_idx++) {
        for (uint8_t resize_down_idx = 0; resize_down_idx < NUM_RESIZE_SCALES + 1; resize_down_idx++) {
            pa_ref_obj_->downscaled_input_padded_picture_ptr[sr_down_idx][resize_down_idx]          = NULL;
            pa_ref_obj_->downscaled_quarter_downsampled_picture_ptr[sr_down_idx][resize_down_idx]   = NULL;
            pa_ref_obj_->downscaled_sixteenth_downsampled_picture_ptr[sr_down_idx][resize_down_idx] = NULL;
            pa_ref_obj_->downscaled_picture_number[sr_down_idx][resize_down_idx]                    = (uint64_t)~0;
            EB_CREATE_MUTEX(pa_ref_obj_->resize_mutex[sr_down_idx][resize_down_idx]);
        }
    }

    return EB_ErrorNone;
}

EbErrorType svt_pa_reference_object_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr) {
    EbPaReferenceObject* obj;

    *object_dbl_ptr = NULL;
    EB_NEW(obj, svt_pa_reference_object_ctor, object_init_data_ptr);
    *object_dbl_ptr = obj;

    return EB_ErrorNone;
}

/*
svt_tpl_reference_param_update: update the parameters in tpl_ref_obj for changing the resolution on the fly
*/
EbErrorType svt_tpl_reference_param_update(EbTplReferenceObject* tpl_ref_obj, SequenceControlSet* scs) {
    EbPictureBufferDescInitData ref_pic_buf_desc_init_data;
    // PA Reference Picture Buffers
    // Currently, only Luma samples are needed in the PA
    ref_pic_buf_desc_init_data.max_width    = scs->max_input_luma_width;
    ref_pic_buf_desc_init_data.max_height   = scs->max_input_luma_height;
    ref_pic_buf_desc_init_data.bit_depth    = EB_EIGHT_BIT;
    ref_pic_buf_desc_init_data.color_format = EB_YUV420; //use 420 for picture analysis

    // Allocate one ref pic to be used in TPL
    ref_pic_buf_desc_init_data.buffer_enable_mask = PICTURE_BUFFER_DESC_Y_FLAG;

    ref_pic_buf_desc_init_data.left_padding      = TPL_PADX;
    ref_pic_buf_desc_init_data.right_padding     = TPL_PADX;
    ref_pic_buf_desc_init_data.top_padding       = TPL_PADY;
    ref_pic_buf_desc_init_data.bot_padding       = TPL_PADY;
    ref_pic_buf_desc_init_data.split_mode        = false;
    ref_pic_buf_desc_init_data.mfmv              = 0;
    ref_pic_buf_desc_init_data.is_16bit_pipeline = false;

    ref_pic_buf_desc_init_data.rest_units_per_tile = 0;
    ref_pic_buf_desc_init_data.sb_total_count      = scs->sb_total_count;

    // Reference picture constructor
    svt_picture_buffer_desc_update(tpl_ref_obj->ref_picture_ptr, (EbPtr)&ref_pic_buf_desc_init_data);

    return EB_ErrorNone;
}

EbErrorType svt_tpl_reference_object_ctor(EbTplReferenceObject* tpl_ref_obj_, EbPtr object_init_data_ptr) {
    EbPictureBufferDescInitData* picture_buffer_desc_init_data_ptr = (EbPictureBufferDescInitData*)object_init_data_ptr;

    tpl_ref_obj_->dctor = svt_tpl_reference_object_dctor;

    // Reference picture constructor
    EB_NEW(tpl_ref_obj_->ref_picture_ptr, svt_picture_buffer_desc_ctor, (EbPtr)picture_buffer_desc_init_data_ptr);

    return EB_ErrorNone;
}

EbErrorType svt_tpl_reference_object_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr) {
    EbTplReferenceObject* obj;

    *object_dbl_ptr = NULL;
    EB_NEW(obj, svt_tpl_reference_object_ctor, object_init_data_ptr);
    *object_dbl_ptr = obj;

    return EB_ErrorNone;
}

/************************************************
* Release Pa Reference Objects
** Check if reference pictures are needed
** release them when appropriate
************************************************/
void svt_aom_release_pa_reference_objects(SequenceControlSet* scs, PictureParentControlSet* pcs) {
    (void)scs;
    // PA Reference Pictures
    if (pcs->slice_type != I_SLICE) {
        // Release the PA reference Pictures from both lists
        for (REF_FRAME_MINUS1 ref = LAST; ref < ALT + 1; ref++) {
            const uint8_t list_idx = get_list_idx(ref + 1);
            const uint8_t ref_idx  = get_ref_frame_idx(ref + 1);
            if (pcs->ref_pa_pic_ptr_array[list_idx][ref_idx] != NULL) {
                svt_release_object(pcs->ref_pa_pic_ptr_array[list_idx][ref_idx]);
                if (pcs->ref_y8b_array[list_idx][ref_idx]) {
                    //y8b  needs to get decremented at the same time of pa ref
                    svt_release_object(pcs->ref_y8b_array[list_idx][ref_idx]);
                }
            }
        }
    }

    if (pcs->pa_ref_pic_wrapper != NULL) {
        //assert((int32_t)pcs->pa_ref_pic_wrapper->live_count > 0);
        svt_release_object(pcs->pa_ref_pic_wrapper);

        if (pcs->y8b_wrapper) {
            //y8b needs to get decremented at the same time of pa ref
            svt_release_object(pcs->y8b_wrapper);
        }
    }
    // Mark that the PCS released PA references
    pcs->reference_released = 1;
    return;
}
