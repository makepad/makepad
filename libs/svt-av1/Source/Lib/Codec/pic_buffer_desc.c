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

#include <stdlib.h>

#include "pic_buffer_desc.h"

static void svt_picture_buffer_desc_dctor(EbPtr p) {
    EbPictureBufferDesc* obj = (EbPictureBufferDesc*)p;
    if (obj->buffer_enable_mask & PICTURE_BUFFER_DESC_Y_FLAG) {
        EB_FREE_ALIGNED_ARRAY(obj->buffer_y);
        EB_FREE_ALIGNED_ARRAY(obj->buffer_bit_inc_y);
    }
    if (obj->buffer_enable_mask & PICTURE_BUFFER_DESC_Cb_FLAG) {
        EB_FREE_ALIGNED_ARRAY(obj->buffer_cb);
        EB_FREE_ALIGNED_ARRAY(obj->buffer_bit_inc_cb);
    }
    if (obj->buffer_enable_mask & PICTURE_BUFFER_DESC_Cr_FLAG) {
        EB_FREE_ALIGNED_ARRAY(obj->buffer_cr);
        EB_FREE_ALIGNED_ARRAY(obj->buffer_bit_inc_cr);
    }
}

/*****************************************
 * svt_picture_buffer_desc_ctor
 *  Initializes the Buffer Descriptor's
 *  values that are fixed for the life of
 *  the descriptor.
 *****************************************/
EbErrorType svt_picture_buffer_desc_ctor_noy8b(EbPictureBufferDesc* pictureBufferDescPtr,
                                               const EbPtr          object_init_data_ptr) {
    const EbPictureBufferDescInitData* picture_buffer_desc_init_data_ptr = (EbPictureBufferDescInitData*)
        object_init_data_ptr;

    //for 10bit we force split mode + 2b-compressed
    uint32_t       bytes_per_pixel = 1;
    const uint16_t subsampling_x   = (picture_buffer_desc_init_data_ptr->color_format == EB_YUV444 ? 0 : 1);
    const uint16_t subsampling_y   = (picture_buffer_desc_init_data_ptr->color_format == EB_YUV444 ? 0 : 1);

    pictureBufferDescPtr->dctor = svt_picture_buffer_desc_dctor;

    // Set the Picture Buffer Static variables
    pictureBufferDescPtr->max_width         = picture_buffer_desc_init_data_ptr->max_width;
    pictureBufferDescPtr->max_height        = picture_buffer_desc_init_data_ptr->max_height;
    pictureBufferDescPtr->width             = picture_buffer_desc_init_data_ptr->max_width;
    pictureBufferDescPtr->height            = picture_buffer_desc_init_data_ptr->max_height;
    pictureBufferDescPtr->bit_depth         = picture_buffer_desc_init_data_ptr->bit_depth;
    pictureBufferDescPtr->is_16bit_pipeline = picture_buffer_desc_init_data_ptr->is_16bit_pipeline;
    pictureBufferDescPtr->color_format      = picture_buffer_desc_init_data_ptr->color_format;
    pictureBufferDescPtr->stride_y          = picture_buffer_desc_init_data_ptr->max_width +
        picture_buffer_desc_init_data_ptr->left_padding + picture_buffer_desc_init_data_ptr->right_padding;

    svt_aom_assert_err(pictureBufferDescPtr->stride_y % 8 == 0,
                       "Luma Stride should be n*8 to accommodate 2b-compression flow \n");

    pictureBufferDescPtr->stride_cb = pictureBufferDescPtr->stride_cr = (pictureBufferDescPtr->stride_y +
                                                                         subsampling_x) >>
        subsampling_x;
    pictureBufferDescPtr->org_x        = picture_buffer_desc_init_data_ptr->left_padding;
    pictureBufferDescPtr->org_y        = picture_buffer_desc_init_data_ptr->top_padding;
    pictureBufferDescPtr->origin_bot_y = picture_buffer_desc_init_data_ptr->bot_padding;

    pictureBufferDescPtr->luma_size = pictureBufferDescPtr->stride_y *
        (picture_buffer_desc_init_data_ptr->max_height + picture_buffer_desc_init_data_ptr->top_padding +
         picture_buffer_desc_init_data_ptr->bot_padding);
    pictureBufferDescPtr->chroma_size = pictureBufferDescPtr->stride_cb *
        ((picture_buffer_desc_init_data_ptr->max_height + subsampling_y +
          picture_buffer_desc_init_data_ptr->top_padding + picture_buffer_desc_init_data_ptr->bot_padding) >>
         subsampling_y);
    pictureBufferDescPtr->packed_flag = false;

    if (picture_buffer_desc_init_data_ptr->split_mode == true) {
        pictureBufferDescPtr->stride_bit_inc_y  = pictureBufferDescPtr->stride_y;
        pictureBufferDescPtr->stride_bit_inc_cb = pictureBufferDescPtr->stride_cb;
        pictureBufferDescPtr->stride_bit_inc_cr = pictureBufferDescPtr->stride_cr;
    }
    pictureBufferDescPtr->buffer_enable_mask = picture_buffer_desc_init_data_ptr->buffer_enable_mask;

    pictureBufferDescPtr->buffer_y = 0;

    // Allocate the Picture Buffers (luma & chroma)
    if (picture_buffer_desc_init_data_ptr->buffer_enable_mask & PICTURE_BUFFER_DESC_Y_FLAG) {
        //EB_CALLOC_ALIGNED_ARRAY(pictureBufferDescPtr->buffer_y,
        //    pictureBufferDescPtr->luma_size * bytes_per_pixel);

        pictureBufferDescPtr->buffer_bit_inc_y = 0;
        if (picture_buffer_desc_init_data_ptr->split_mode == true) {
            EB_CALLOC_ALIGNED_ARRAY(pictureBufferDescPtr->buffer_bit_inc_y,
                                    pictureBufferDescPtr->luma_size * bytes_per_pixel / 4);
        }
    }

    if (picture_buffer_desc_init_data_ptr->buffer_enable_mask & PICTURE_BUFFER_DESC_Cb_FLAG) {
        EB_CALLOC_ALIGNED_ARRAY(pictureBufferDescPtr->buffer_cb, pictureBufferDescPtr->chroma_size * bytes_per_pixel);
        pictureBufferDescPtr->buffer_bit_inc_cb = 0;
        if (picture_buffer_desc_init_data_ptr->split_mode == true) {
            EB_CALLOC_ALIGNED_ARRAY(pictureBufferDescPtr->buffer_bit_inc_cb,
                                    pictureBufferDescPtr->chroma_size * bytes_per_pixel / 4);
        }
    }

    if (picture_buffer_desc_init_data_ptr->buffer_enable_mask & PICTURE_BUFFER_DESC_Cr_FLAG) {
        EB_CALLOC_ALIGNED_ARRAY(pictureBufferDescPtr->buffer_cr, pictureBufferDescPtr->chroma_size * bytes_per_pixel);
        pictureBufferDescPtr->buffer_bit_inc_cr = 0;
        if (picture_buffer_desc_init_data_ptr->split_mode == true) {
            EB_CALLOC_ALIGNED_ARRAY(pictureBufferDescPtr->buffer_bit_inc_cr,
                                    pictureBufferDescPtr->chroma_size * bytes_per_pixel / 4);
        }
    }

    return EB_ErrorNone;
}

/*****************************************
 * svt_picture_buffer_desc_noy8b_update
 * update the parameters in EbPictureBufferDesc for changing the resolution
 * on the fly similar to svt_picture_buffer_desc_ctor_noy8b, but no allocation is done.
 *****************************************/
EbErrorType svt_picture_buffer_desc_noy8b_update(EbPictureBufferDesc* pictureBufferDescPtr,
                                                 const EbPtr          object_init_data_ptr) {
    const EbPictureBufferDescInitData* picture_buffer_desc_init_data_ptr = (EbPictureBufferDescInitData*)
        object_init_data_ptr;

    const uint16_t subsampling_x = (picture_buffer_desc_init_data_ptr->color_format == EB_YUV444 ? 0 : 1);
    const uint16_t subsampling_y = (picture_buffer_desc_init_data_ptr->color_format == EB_YUV444 ? 0 : 1);

    // Set the Picture Buffer Static variables
    pictureBufferDescPtr->max_width         = picture_buffer_desc_init_data_ptr->max_width;
    pictureBufferDescPtr->max_height        = picture_buffer_desc_init_data_ptr->max_height;
    pictureBufferDescPtr->width             = picture_buffer_desc_init_data_ptr->max_width;
    pictureBufferDescPtr->height            = picture_buffer_desc_init_data_ptr->max_height;
    pictureBufferDescPtr->bit_depth         = picture_buffer_desc_init_data_ptr->bit_depth;
    pictureBufferDescPtr->is_16bit_pipeline = picture_buffer_desc_init_data_ptr->is_16bit_pipeline;
    pictureBufferDescPtr->color_format      = picture_buffer_desc_init_data_ptr->color_format;
    pictureBufferDescPtr->stride_y          = picture_buffer_desc_init_data_ptr->max_width +
        picture_buffer_desc_init_data_ptr->left_padding + picture_buffer_desc_init_data_ptr->right_padding;

    svt_aom_assert_err(pictureBufferDescPtr->stride_y % 8 == 0,
                       "Luma Stride should be n*8 to accommodate 2b-compression flow \n");

    pictureBufferDescPtr->stride_cb = pictureBufferDescPtr->stride_cr = (pictureBufferDescPtr->stride_y +
                                                                         subsampling_x) >>
        subsampling_x;
    pictureBufferDescPtr->org_x        = picture_buffer_desc_init_data_ptr->left_padding;
    pictureBufferDescPtr->org_y        = picture_buffer_desc_init_data_ptr->top_padding;
    pictureBufferDescPtr->origin_bot_y = picture_buffer_desc_init_data_ptr->bot_padding;

    pictureBufferDescPtr->luma_size = pictureBufferDescPtr->stride_y *
        (picture_buffer_desc_init_data_ptr->max_height + picture_buffer_desc_init_data_ptr->top_padding +
         picture_buffer_desc_init_data_ptr->bot_padding);
    pictureBufferDescPtr->chroma_size = pictureBufferDescPtr->stride_cb *
        ((picture_buffer_desc_init_data_ptr->max_height + subsampling_y +
          picture_buffer_desc_init_data_ptr->top_padding + picture_buffer_desc_init_data_ptr->bot_padding) >>
         subsampling_y);
    pictureBufferDescPtr->packed_flag = false;

    if (picture_buffer_desc_init_data_ptr->split_mode == true) {
        pictureBufferDescPtr->stride_bit_inc_y  = pictureBufferDescPtr->stride_y;
        pictureBufferDescPtr->stride_bit_inc_cb = pictureBufferDescPtr->stride_cb;
        pictureBufferDescPtr->stride_bit_inc_cr = pictureBufferDescPtr->stride_cr;
    }
    pictureBufferDescPtr->buffer_enable_mask = picture_buffer_desc_init_data_ptr->buffer_enable_mask;

    return EB_ErrorNone;
}

/*
svt_picture_buffer_desc_update: update the parameters in EbPictureBufferDesc for changing the resolution on the fly
similar to svt_picture_buffer_desc_ctor, but no allocation is done.
*/
EbErrorType svt_picture_buffer_desc_update(EbPictureBufferDesc* pictureBufferDescPtr,
                                           const EbPtr          object_init_data_ptr) {
    const EbPictureBufferDescInitData* picture_buffer_desc_init_data_ptr = (EbPictureBufferDescInitData*)
        object_init_data_ptr;

    const uint16_t subsampling_x = (picture_buffer_desc_init_data_ptr->color_format == EB_YUV444 ? 0 : 1);
    const uint16_t subsampling_y = (picture_buffer_desc_init_data_ptr->color_format == EB_YUV444 ? 0 : 1);

    // Set the Picture Buffer Static variables
    pictureBufferDescPtr->max_width  = picture_buffer_desc_init_data_ptr->max_width;
    pictureBufferDescPtr->max_height = picture_buffer_desc_init_data_ptr->max_height;
    pictureBufferDescPtr->width      = picture_buffer_desc_init_data_ptr->max_width;
    pictureBufferDescPtr->height     = picture_buffer_desc_init_data_ptr->max_height;
    pictureBufferDescPtr->stride_y   = picture_buffer_desc_init_data_ptr->max_width +
        picture_buffer_desc_init_data_ptr->left_padding + picture_buffer_desc_init_data_ptr->right_padding;
    pictureBufferDescPtr->stride_cb = pictureBufferDescPtr->stride_cr = (pictureBufferDescPtr->stride_y +
                                                                         subsampling_x) >>
        subsampling_x;
    pictureBufferDescPtr->org_x        = picture_buffer_desc_init_data_ptr->left_padding;
    pictureBufferDescPtr->org_y        = picture_buffer_desc_init_data_ptr->top_padding;
    pictureBufferDescPtr->origin_bot_y = picture_buffer_desc_init_data_ptr->bot_padding;

    pictureBufferDescPtr->luma_size = pictureBufferDescPtr->stride_y *
        (picture_buffer_desc_init_data_ptr->max_height + picture_buffer_desc_init_data_ptr->top_padding +
         picture_buffer_desc_init_data_ptr->bot_padding);
    pictureBufferDescPtr->chroma_size = pictureBufferDescPtr->stride_cb *
        ((picture_buffer_desc_init_data_ptr->max_height + subsampling_y +
          picture_buffer_desc_init_data_ptr->top_padding + picture_buffer_desc_init_data_ptr->bot_padding) >>
         subsampling_y);

    if (picture_buffer_desc_init_data_ptr->split_mode == true) {
        pictureBufferDescPtr->stride_bit_inc_y  = pictureBufferDescPtr->stride_y;
        pictureBufferDescPtr->stride_bit_inc_cb = pictureBufferDescPtr->stride_cb;
        pictureBufferDescPtr->stride_bit_inc_cr = pictureBufferDescPtr->stride_cr;
    }

    return EB_ErrorNone;
}

EbErrorType svt_picture_buffer_desc_ctor(EbPictureBufferDesc* pictureBufferDescPtr, const EbPtr object_init_data_ptr) {
    const EbPictureBufferDescInitData* picture_buffer_desc_init_data_ptr = (EbPictureBufferDescInitData*)
        object_init_data_ptr;

    uint32_t       bytes_per_pixel = (picture_buffer_desc_init_data_ptr->bit_depth == EB_EIGHT_BIT) ? 1
              : (picture_buffer_desc_init_data_ptr->bit_depth <= EB_SIXTEEN_BIT)                    ? 2
                                                                                                    : 4;
    const uint16_t subsampling_x   = (picture_buffer_desc_init_data_ptr->color_format == EB_YUV444 ? 0 : 1);
    const uint16_t subsampling_y   = (picture_buffer_desc_init_data_ptr->color_format == EB_YUV444 ? 0 : 1);

    pictureBufferDescPtr->dctor = svt_picture_buffer_desc_dctor;

    if (picture_buffer_desc_init_data_ptr->bit_depth > EB_EIGHT_BIT &&
        picture_buffer_desc_init_data_ptr->bit_depth <= EB_SIXTEEN_BIT &&
        picture_buffer_desc_init_data_ptr->split_mode == true) {
        bytes_per_pixel = 1;
    }

    // Set the Picture Buffer Static variables
    pictureBufferDescPtr->max_width         = picture_buffer_desc_init_data_ptr->max_width;
    pictureBufferDescPtr->max_height        = picture_buffer_desc_init_data_ptr->max_height;
    pictureBufferDescPtr->width             = picture_buffer_desc_init_data_ptr->max_width;
    pictureBufferDescPtr->height            = picture_buffer_desc_init_data_ptr->max_height;
    pictureBufferDescPtr->bit_depth         = picture_buffer_desc_init_data_ptr->bit_depth;
    pictureBufferDescPtr->is_16bit_pipeline = picture_buffer_desc_init_data_ptr->is_16bit_pipeline;
    pictureBufferDescPtr->color_format      = picture_buffer_desc_init_data_ptr->color_format;
    pictureBufferDescPtr->stride_y          = picture_buffer_desc_init_data_ptr->max_width +
        picture_buffer_desc_init_data_ptr->left_padding + picture_buffer_desc_init_data_ptr->right_padding;
    pictureBufferDescPtr->stride_cb = pictureBufferDescPtr->stride_cr = (pictureBufferDescPtr->stride_y +
                                                                         subsampling_x) >>
        subsampling_x;
    pictureBufferDescPtr->org_x        = picture_buffer_desc_init_data_ptr->left_padding;
    pictureBufferDescPtr->org_y        = picture_buffer_desc_init_data_ptr->top_padding;
    pictureBufferDescPtr->origin_bot_y = picture_buffer_desc_init_data_ptr->bot_padding;

    pictureBufferDescPtr->luma_size = pictureBufferDescPtr->stride_y *
        (picture_buffer_desc_init_data_ptr->max_height + picture_buffer_desc_init_data_ptr->top_padding +
         picture_buffer_desc_init_data_ptr->bot_padding);
    pictureBufferDescPtr->chroma_size = pictureBufferDescPtr->stride_cb *
        ((picture_buffer_desc_init_data_ptr->max_height + subsampling_y +
          picture_buffer_desc_init_data_ptr->top_padding + picture_buffer_desc_init_data_ptr->bot_padding) >>
         subsampling_y);

    pictureBufferDescPtr->packed_flag = bytes_per_pixel > 1 ? true : false;

    if (picture_buffer_desc_init_data_ptr->split_mode == true) {
        pictureBufferDescPtr->stride_bit_inc_y  = pictureBufferDescPtr->stride_y;
        pictureBufferDescPtr->stride_bit_inc_cb = pictureBufferDescPtr->stride_cb;
        pictureBufferDescPtr->stride_bit_inc_cr = pictureBufferDescPtr->stride_cr;
    }
    pictureBufferDescPtr->buffer_enable_mask = picture_buffer_desc_init_data_ptr->buffer_enable_mask;

    // Allocate the Picture Buffers (luma & chroma)
    if (picture_buffer_desc_init_data_ptr->buffer_enable_mask & PICTURE_BUFFER_DESC_Y_FLAG) {
        EB_MALLOC_ALIGNED_ARRAY(pictureBufferDescPtr->buffer_y, pictureBufferDescPtr->luma_size * bytes_per_pixel);
        pictureBufferDescPtr->buffer_bit_inc_y = 0;
        if (picture_buffer_desc_init_data_ptr->split_mode == true) {
            EB_MALLOC_ALIGNED_ARRAY(pictureBufferDescPtr->buffer_bit_inc_y,
                                    pictureBufferDescPtr->luma_size * bytes_per_pixel);
        }
    }

    if (picture_buffer_desc_init_data_ptr->buffer_enable_mask & PICTURE_BUFFER_DESC_Cb_FLAG) {
        EB_MALLOC_ALIGNED_ARRAY(pictureBufferDescPtr->buffer_cb, pictureBufferDescPtr->chroma_size * bytes_per_pixel);
        pictureBufferDescPtr->buffer_bit_inc_cb = 0;
        if (picture_buffer_desc_init_data_ptr->split_mode == true) {
            EB_MALLOC_ALIGNED_ARRAY(pictureBufferDescPtr->buffer_bit_inc_cb,
                                    pictureBufferDescPtr->chroma_size * bytes_per_pixel);
        }
    }

    if (picture_buffer_desc_init_data_ptr->buffer_enable_mask & PICTURE_BUFFER_DESC_Cr_FLAG) {
        EB_MALLOC_ALIGNED_ARRAY(pictureBufferDescPtr->buffer_cr, pictureBufferDescPtr->chroma_size * bytes_per_pixel);
        pictureBufferDescPtr->buffer_bit_inc_cr = 0;
        if (picture_buffer_desc_init_data_ptr->split_mode == true) {
            EB_MALLOC_ALIGNED_ARRAY(pictureBufferDescPtr->buffer_bit_inc_cr,
                                    pictureBufferDescPtr->chroma_size * bytes_per_pixel);
        }
    }

    return EB_ErrorNone;
}

static void svt_recon_picture_buffer_desc_dctor(EbPtr p) {
    EbPictureBufferDesc* obj = (EbPictureBufferDesc*)p;
    if (obj->buffer_enable_mask & PICTURE_BUFFER_DESC_Y_FLAG) {
        EB_FREE_ALIGNED_ARRAY(obj->buffer_y);
    }
    if (obj->buffer_enable_mask & PICTURE_BUFFER_DESC_Cb_FLAG) {
        EB_FREE_ALIGNED_ARRAY(obj->buffer_cb);
    }
    if (obj->buffer_enable_mask & PICTURE_BUFFER_DESC_Cr_FLAG) {
        EB_FREE_ALIGNED_ARRAY(obj->buffer_cr);
    }
}

/*****************************************
Update the parameters in pictureBufferDescPtr for changing the resolution on the fly
similar to svt_recon_picture_buffer_desc_ctor, but no allocation is done.
 *****************************************/
EbErrorType svt_recon_picture_buffer_desc_update(EbPictureBufferDesc* pictureBufferDescPtr,
                                                 EbPtr                object_init_data_ptr) {
    EbPictureBufferDescInitData* picture_buffer_desc_init_data_ptr = (EbPictureBufferDescInitData*)object_init_data_ptr;
    const uint16_t               subsampling_x = (picture_buffer_desc_init_data_ptr->color_format == EB_YUV444 ? 0 : 1);
    const uint16_t               subsampling_y = (picture_buffer_desc_init_data_ptr->color_format == EB_YUV444 ? 0 : 1);

    // Set the Picture Buffer Static variables
    pictureBufferDescPtr->max_width    = picture_buffer_desc_init_data_ptr->max_width;
    pictureBufferDescPtr->max_height   = picture_buffer_desc_init_data_ptr->max_height;
    pictureBufferDescPtr->width        = picture_buffer_desc_init_data_ptr->max_width;
    pictureBufferDescPtr->height       = picture_buffer_desc_init_data_ptr->max_height;
    pictureBufferDescPtr->bit_depth    = picture_buffer_desc_init_data_ptr->bit_depth;
    pictureBufferDescPtr->color_format = picture_buffer_desc_init_data_ptr->color_format;
    pictureBufferDescPtr->stride_y     = picture_buffer_desc_init_data_ptr->max_width +
        picture_buffer_desc_init_data_ptr->left_padding + picture_buffer_desc_init_data_ptr->right_padding;
    pictureBufferDescPtr->stride_cb = pictureBufferDescPtr->stride_cr = (pictureBufferDescPtr->stride_y +
                                                                         subsampling_x) >>
        subsampling_x;
    pictureBufferDescPtr->org_x        = picture_buffer_desc_init_data_ptr->left_padding;
    pictureBufferDescPtr->org_y        = picture_buffer_desc_init_data_ptr->top_padding;
    pictureBufferDescPtr->origin_bot_y = picture_buffer_desc_init_data_ptr->bot_padding;

    pictureBufferDescPtr->luma_size = pictureBufferDescPtr->stride_y *
        (picture_buffer_desc_init_data_ptr->max_height + picture_buffer_desc_init_data_ptr->top_padding +
         picture_buffer_desc_init_data_ptr->bot_padding);
    pictureBufferDescPtr->chroma_size = pictureBufferDescPtr->stride_cb *
        ((picture_buffer_desc_init_data_ptr->max_height + subsampling_y +
          picture_buffer_desc_init_data_ptr->top_padding + picture_buffer_desc_init_data_ptr->bot_padding) >>
         subsampling_y);
    pictureBufferDescPtr->packed_flag = (picture_buffer_desc_init_data_ptr->bit_depth > EB_EIGHT_BIT) ? true : false;

    pictureBufferDescPtr->buffer_enable_mask = picture_buffer_desc_init_data_ptr->buffer_enable_mask;

    return EB_ErrorNone;
}

/*****************************************
 * svt_recon_picture_buffer_desc_ctor
 *  Initializes the Buffer Descriptor's
 *  values that are fixed for the life of
 *  the descriptor.
 *****************************************/
EbErrorType svt_recon_picture_buffer_desc_ctor(EbPictureBufferDesc* pictureBufferDescPtr, EbPtr object_init_data_ptr) {
    EbPictureBufferDescInitData* picture_buffer_desc_init_data_ptr = (EbPictureBufferDescInitData*)object_init_data_ptr;
    const uint16_t               subsampling_x = (picture_buffer_desc_init_data_ptr->color_format == EB_YUV444 ? 0 : 1);
    const uint16_t               subsampling_y = (picture_buffer_desc_init_data_ptr->color_format == EB_YUV444 ? 0 : 1);

    uint32_t bytes_per_pixel = (picture_buffer_desc_init_data_ptr->bit_depth == EB_EIGHT_BIT) ? 1 : 2;

    pictureBufferDescPtr->dctor = svt_recon_picture_buffer_desc_dctor;
    // Set the Picture Buffer Static variables
    pictureBufferDescPtr->max_width    = picture_buffer_desc_init_data_ptr->max_width;
    pictureBufferDescPtr->max_height   = picture_buffer_desc_init_data_ptr->max_height;
    pictureBufferDescPtr->width        = picture_buffer_desc_init_data_ptr->max_width;
    pictureBufferDescPtr->height       = picture_buffer_desc_init_data_ptr->max_height;
    pictureBufferDescPtr->bit_depth    = picture_buffer_desc_init_data_ptr->bit_depth;
    pictureBufferDescPtr->color_format = picture_buffer_desc_init_data_ptr->color_format;
    pictureBufferDescPtr->stride_y     = picture_buffer_desc_init_data_ptr->max_width +
        picture_buffer_desc_init_data_ptr->left_padding + picture_buffer_desc_init_data_ptr->right_padding;
    pictureBufferDescPtr->stride_cb = pictureBufferDescPtr->stride_cr = (pictureBufferDescPtr->stride_y +
                                                                         subsampling_x) >>
        subsampling_x;
    pictureBufferDescPtr->org_x        = picture_buffer_desc_init_data_ptr->left_padding;
    pictureBufferDescPtr->org_y        = picture_buffer_desc_init_data_ptr->top_padding;
    pictureBufferDescPtr->origin_bot_y = picture_buffer_desc_init_data_ptr->bot_padding;

    pictureBufferDescPtr->luma_size = pictureBufferDescPtr->stride_y *
        (picture_buffer_desc_init_data_ptr->max_height + picture_buffer_desc_init_data_ptr->top_padding +
         picture_buffer_desc_init_data_ptr->bot_padding);
    pictureBufferDescPtr->chroma_size = pictureBufferDescPtr->stride_cb *
        ((picture_buffer_desc_init_data_ptr->max_height + subsampling_y +
          picture_buffer_desc_init_data_ptr->top_padding + picture_buffer_desc_init_data_ptr->bot_padding) >>
         subsampling_y);
    pictureBufferDescPtr->packed_flag = (picture_buffer_desc_init_data_ptr->bit_depth > EB_EIGHT_BIT) ? true : false;

    pictureBufferDescPtr->buffer_enable_mask = picture_buffer_desc_init_data_ptr->buffer_enable_mask;

    // Allocate the Picture Buffers (luma & chroma)
    if (picture_buffer_desc_init_data_ptr->buffer_enable_mask & PICTURE_BUFFER_DESC_Y_FLAG) {
        EB_CALLOC_ALIGNED_ARRAY(pictureBufferDescPtr->buffer_y, pictureBufferDescPtr->luma_size * bytes_per_pixel);
    }
    if (picture_buffer_desc_init_data_ptr->buffer_enable_mask & PICTURE_BUFFER_DESC_Cb_FLAG) {
        EB_CALLOC_ALIGNED_ARRAY(pictureBufferDescPtr->buffer_cb, pictureBufferDescPtr->chroma_size * bytes_per_pixel);
    }
    if (picture_buffer_desc_init_data_ptr->buffer_enable_mask & PICTURE_BUFFER_DESC_Cr_FLAG) {
        EB_CALLOC_ALIGNED_ARRAY(pictureBufferDescPtr->buffer_cr, pictureBufferDescPtr->chroma_size * bytes_per_pixel);
    }
    return EB_ErrorNone;
}

void svt_aom_link_eb_to_aom_buffer_desc_8bit(EbPictureBufferDesc* picBuffDsc, Yv12BufferConfig* aomBuffDsc) {
    //forces an 8 bit version
    //NOTe:  Not all fileds are connected. add more connections as needed.
    {
        aomBuffDsc->y_buffer = picBuffDsc->buffer_y + picBuffDsc->org_x + (picBuffDsc->org_y * picBuffDsc->stride_y);
        aomBuffDsc->u_buffer = picBuffDsc->buffer_cb + picBuffDsc->org_x / 2 +
            (picBuffDsc->org_y / 2 * picBuffDsc->stride_cb);
        aomBuffDsc->v_buffer = picBuffDsc->buffer_cr + picBuffDsc->org_x / 2 +
            (picBuffDsc->org_y / 2 * picBuffDsc->stride_cb);

        aomBuffDsc->y_width  = picBuffDsc->width;
        aomBuffDsc->uv_width = picBuffDsc->width / 2;

        aomBuffDsc->y_height  = picBuffDsc->height;
        aomBuffDsc->uv_height = picBuffDsc->height / 2;

        aomBuffDsc->y_stride  = picBuffDsc->stride_y;
        aomBuffDsc->uv_stride = picBuffDsc->stride_cb;

        aomBuffDsc->border = picBuffDsc->org_x;

        aomBuffDsc->subsampling_x = 1;
        aomBuffDsc->subsampling_y = 1;

        aomBuffDsc->y_crop_width   = aomBuffDsc->y_width;
        aomBuffDsc->uv_crop_width  = aomBuffDsc->uv_width;
        aomBuffDsc->y_crop_height  = aomBuffDsc->y_height;
        aomBuffDsc->uv_crop_height = aomBuffDsc->uv_height;

        aomBuffDsc->flags = 0;
    }
}

void svt_aom_link_eb_to_aom_buffer_desc(EbPictureBufferDesc* picBuffDsc, Yv12BufferConfig* aomBuffDsc,
                                        uint16_t pad_right, uint16_t pad_bottom, bool is_16bit) {
    (void)is_16bit;

    const int32_t ss_x = 1, ss_y = 1;
    //NOTe:  Not all fileds are connected. add more connections as needed.
    if ((picBuffDsc->bit_depth == EB_EIGHT_BIT) && (picBuffDsc->is_16bit_pipeline != 1)) {
        aomBuffDsc->y_buffer = picBuffDsc->buffer_y + picBuffDsc->org_x + (picBuffDsc->org_y * picBuffDsc->stride_y);
        aomBuffDsc->u_buffer = picBuffDsc->buffer_cb + (picBuffDsc->org_x >> ss_x) +
            ((picBuffDsc->org_y >> ss_y) * picBuffDsc->stride_cb);
        aomBuffDsc->v_buffer = picBuffDsc->buffer_cr + (picBuffDsc->org_x >> ss_x) +
            ((picBuffDsc->org_y >> ss_y) * picBuffDsc->stride_cb);

        aomBuffDsc->y_width  = picBuffDsc->width;
        aomBuffDsc->uv_width = (picBuffDsc->width + ss_x) >> ss_x;

        aomBuffDsc->y_height  = picBuffDsc->height;
        aomBuffDsc->uv_height = (picBuffDsc->height + ss_y) >> ss_y;

        aomBuffDsc->y_stride  = picBuffDsc->stride_y;
        aomBuffDsc->uv_stride = picBuffDsc->stride_cb;

        aomBuffDsc->border = picBuffDsc->org_x;

        aomBuffDsc->subsampling_x = ss_x;
        aomBuffDsc->subsampling_y = ss_y;

        aomBuffDsc->y_crop_width   = aomBuffDsc->y_width - pad_right;
        aomBuffDsc->uv_crop_width  = (aomBuffDsc->y_crop_width + ss_x) >> ss_x;
        aomBuffDsc->y_crop_height  = aomBuffDsc->y_height - pad_bottom;
        aomBuffDsc->uv_crop_height = (aomBuffDsc->y_crop_height + ss_y) >> ss_y;

        aomBuffDsc->flags = 0;
    } else {
        /*
        Moving within a 16bit memory area: 2 possible mecanisms:

        1. to move from one location to another by an offset x, using 16bit pointers
           int32_t x;
           U16* Base16b;
           U16* NewAdd16b = Base16b + x
           int32_t data = NewAdd16b[0];

         2. to move from one location to another by an offset x, using 8bit pointers

            int32_t x;
            U16* Base16b;

            U16* baseAd8b = Base16b/2; //convert the base address into 8bit
            U16* newAd8b  = baseAd8b + x;

            then before reading the data, we need to convert the pointer back to 16b
            U16* NewAdd16b = newAd8b*2 ;
            int32_t data = NewAdd16b[0];

            NewAdd16b = Base16b + off
                      = Base16b_asInt + 2*off
                      =(Base16b_asInt/2 +off)*2
        */

        aomBuffDsc->y_buffer = CONVERT_TO_BYTEPTR(picBuffDsc->buffer_y);
        aomBuffDsc->u_buffer = CONVERT_TO_BYTEPTR(picBuffDsc->buffer_cb);
        aomBuffDsc->v_buffer = CONVERT_TO_BYTEPTR(picBuffDsc->buffer_cr);

        aomBuffDsc->y_buffer += picBuffDsc->org_x + (picBuffDsc->org_y * picBuffDsc->stride_y);
        aomBuffDsc->u_buffer += (picBuffDsc->org_x >> ss_x) + ((picBuffDsc->org_y >> ss_y) * picBuffDsc->stride_cb);
        aomBuffDsc->v_buffer += (picBuffDsc->org_x >> ss_x) + ((picBuffDsc->org_y >> ss_y) * picBuffDsc->stride_cb);

        aomBuffDsc->y_width  = picBuffDsc->width;
        aomBuffDsc->uv_width = (picBuffDsc->width + ss_x) >> ss_x;

        aomBuffDsc->y_height  = picBuffDsc->height;
        aomBuffDsc->uv_height = (picBuffDsc->height + ss_y) >> ss_y;

        aomBuffDsc->y_stride  = picBuffDsc->stride_y;
        aomBuffDsc->uv_stride = picBuffDsc->stride_cb;

        aomBuffDsc->border = picBuffDsc->org_x;

        aomBuffDsc->subsampling_x = ss_x;
        aomBuffDsc->subsampling_y = ss_y;

        aomBuffDsc->y_crop_width   = aomBuffDsc->y_width - pad_right;
        aomBuffDsc->uv_crop_width  = (aomBuffDsc->y_crop_width + ss_x) >> ss_x;
        aomBuffDsc->y_crop_height  = aomBuffDsc->y_height - pad_bottom;
        aomBuffDsc->uv_crop_height = (aomBuffDsc->y_crop_height + ss_y) >> ss_y;
        aomBuffDsc->flags          = YV12_FLAG_HIGHBITDEPTH;
    }
}

#define yv12_align_addr(addr, align) (void*)(((size_t)(addr) + ((align) - 1)) & (size_t)-(align))

int32_t svt_aom_realloc_frame_buffer(Yv12BufferConfig* ybf, int32_t width, int32_t height, int32_t ss_x, int32_t ss_y,
                                     int32_t use_highbitdepth, int32_t border, int32_t byte_alignment,
                                     AomCodecFrameBuffer* fb, AomGetFrameBufferCbFn cb, void* cb_priv) {
    if (ybf) {
        const int32_t  aom_byte_align = (byte_alignment == 0) ? 1 : byte_alignment;
        const int32_t  aligned_width  = (width + 7) & ~7;
        const int32_t  aligned_height = (height + 7) & ~7;
        const int32_t  y_stride       = ((aligned_width + 2 * border) + 31) & ~31;
        const uint64_t yplane_size    = (aligned_height + 2 * border) * (uint64_t)y_stride + byte_alignment;
        const int32_t  uv_width       = aligned_width >> ss_x;
        const int32_t  uv_height      = aligned_height >> ss_y;
        const int32_t  uv_stride      = y_stride >> ss_x;
        const int32_t  uv_border_w    = border >> ss_x;
        const int32_t  uv_border_h    = border >> ss_y;
        const uint64_t uvplane_size   = (uv_height + 2 * uv_border_h) * (uint64_t)uv_stride + byte_alignment;

        const uint64_t frame_size = (1 + use_highbitdepth) * (yplane_size + 2 * uvplane_size);

        uint8_t* buf = NULL;

        if (cb != NULL) {
            const int32_t  align_addr_extra_size = 31;
            const uint64_t external_frame_size   = frame_size + align_addr_extra_size;

            assert(fb != NULL);

            if (external_frame_size != (size_t)external_frame_size) {
                return -1;
            }

            // Allocation to hold larger frame, or first allocation.
            if (cb(cb_priv, (size_t)external_frame_size, fb) < 0) {
                return -1;
            }

            if (fb->data == NULL || fb->size < external_frame_size) {
                return -1;
            }

            ybf->buffer_alloc = (uint8_t*)yv12_align_addr(fb->data, 32);

#if defined(__has_feature)
#if __has_feature(memory_sanitizer)
            // This memset is needed for fixing the issue of using uninitialized
            // value in msan test. It will cause a perf loss, so only do this for
            // msan test.
            memset(ybf->buffer_alloc, 0, (int32_t)frame_size);
#endif
#endif
        } else if (frame_size > (size_t)ybf->buffer_alloc_sz) {
            // Allocation to hold larger frame, or first allocation.
            if (ybf->buffer_alloc_sz > 0) {
                EB_FREE_ARRAY(ybf->buffer_alloc);
            }
            if (frame_size != (size_t)frame_size) {
                return -1;
            }
            EB_MALLOC_ARRAY(ybf->buffer_alloc, frame_size);

            if (!ybf->buffer_alloc) {
                return -1;
            }

            ybf->buffer_alloc_sz = (size_t)frame_size;

            // This memset is needed for fixing valgrind error from C loop filter
            // due to access uninitialized memory in frame border. It could be
            // removed if border is totally removed.
            memset(ybf->buffer_alloc, 0, ybf->buffer_alloc_sz);
        }

        /* Only support allocating buffers that have a border that's a multiple
        * of 32. The border restriction is required to get 16-byte alignment of
        * the start of the chroma rows without introducing an arbitrary gap
        * between planes, which would break the semantics of things like
        * aom_img_set_rect(). */
        if (border & 0x1f) {
            return -3;
        }

        ybf->y_crop_width  = width;
        ybf->y_crop_height = height;
        ybf->y_width       = aligned_width;
        ybf->y_height      = aligned_height;
        ybf->y_stride      = y_stride;

        ybf->uv_crop_width  = (width + ss_x) >> ss_x;
        ybf->uv_crop_height = (height + ss_y) >> ss_y;
        ybf->uv_width       = uv_width;
        ybf->uv_height      = uv_height;
        ybf->uv_stride      = uv_stride;

        ybf->border        = border;
        ybf->frame_size    = (size_t)frame_size;
        ybf->subsampling_x = ss_x;
        ybf->subsampling_y = ss_y;

        buf = ybf->buffer_alloc;
        if (use_highbitdepth) {
            // Store uint16 addresses when using 16bit framebuffers
            buf        = CONVERT_TO_BYTEPTR(ybf->buffer_alloc);
            ybf->flags = YV12_FLAG_HIGHBITDEPTH;
        } else {
            ybf->flags = 0;
        }
        ybf->y_buffer = (uint8_t*)yv12_align_addr(buf + (border * y_stride) + border, aom_byte_align);
        ybf->u_buffer = (uint8_t*)yv12_align_addr(buf + yplane_size + (uv_border_h * uv_stride) + uv_border_w,
                                                  aom_byte_align);
        ybf->v_buffer = (uint8_t*)yv12_align_addr(
            buf + yplane_size + uvplane_size + (uv_border_h * uv_stride) + uv_border_w, aom_byte_align);

        ybf->use_external_refernce_buffers = 0;

        ybf->corrupted = 0; /* assume not corrupted by errors */
        return 0;
    }
    return -2;
}
