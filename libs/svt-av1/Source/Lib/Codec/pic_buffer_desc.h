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

#ifndef EbPictureBuffer_h
#define EbPictureBuffer_h

#include <stdio.h>

#include "definitions.h"
#include "EbSvtAv1Formats.h"
#include "object.h"
#ifdef __cplusplus
extern "C" {
#endif
#define PICTURE_BUFFER_DESC_Y_FLAG (1 << 0)
#define PICTURE_BUFFER_DESC_Cb_FLAG (1 << 1)
#define PICTURE_BUFFER_DESC_Cr_FLAG (1 << 2)
#define PICTURE_BUFFER_DESC_LUMA_MASK PICTURE_BUFFER_DESC_Y_FLAG
#define PICTURE_BUFFER_DESC_CHROMA_MASK (PICTURE_BUFFER_DESC_Cb_FLAG | PICTURE_BUFFER_DESC_Cr_FLAG)
#define PICTURE_BUFFER_DESC_FULL_MASK \
    (PICTURE_BUFFER_DESC_Y_FLAG | PICTURE_BUFFER_DESC_Cb_FLAG | PICTURE_BUFFER_DESC_Cr_FLAG)

/************************************
     * EbPictureBufferDesc
     ************************************/
typedef struct EbPictureBufferDesc {
    EbDctor dctor;
    // Buffer Ptrs
    EbByte buffer_y; // pointer to the Y luma buffer
    EbByte buffer_cb; // pointer to the U chroma buffer
    EbByte buffer_cr; // pointer to the V chroma buffer
    //Bit increment
    EbByte buffer_bit_inc_y; // pointer to the Y luma buffer Bit increment
    EbByte buffer_bit_inc_cb; // pointer to the U chroma buffer Bit increment
    EbByte buffer_bit_inc_cr; // pointer to the V chroma buffer Bit increment

    uint16_t stride_y; // pointer to the Y luma buffer
    uint16_t stride_cb; // pointer to the U chroma buffer
    uint16_t stride_cr; // pointer to the V chroma buffer

    uint16_t stride_bit_inc_y; // pointer to the Y luma buffer Bit increment
    uint16_t stride_bit_inc_cb; // pointer to the U chroma buffer Bit increment
    uint16_t stride_bit_inc_cr; // pointer to the V chroma buffer Bit increment

    // Picture Parameters
    uint16_t      org_x; // Horizontal padding distance
    uint16_t      org_y; // Vertical padding distance
    uint16_t      origin_bot_y; // Vertical bottom padding distance
    uint16_t      width; // Luma picture width which excludes the padding
    uint16_t      height; // Luma picture height which excludes the padding
    uint16_t      max_width; // input Luma picture width
    uint16_t      max_height; // input Luma picture height
    EbBitDepth    bit_depth; // Pixel Bit Depth
    EbColorFormat color_format; // Chroma Subsumpling

    // Buffer Parameters
    uint32_t luma_size; // Size of the luma buffer
    uint32_t chroma_size; // Size of the chroma buffers
    bool     packed_flag; // Indicates if sample buffers are packed or not

    bool     film_grain_flag; // Indicates if film grain parameters are present for the frame
    uint32_t buffer_enable_mask;

    // internal bit-depth: when equals 1 internal bit-depth is 16bits regardless of the input
    // bit-depth
    bool is_16bit_pipeline;
} EbPictureBufferDesc;

#define YV12_FLAG_HIGHBITDEPTH 8

typedef struct Yv12BufferConfig {
    union {
        struct {
            int32_t y_width;
            int32_t uv_width;
            int32_t alpha_width;
        };

        int32_t widths[3];
    };

    union {
        struct {
            int32_t y_height;
            int32_t uv_height;
            int32_t alpha_height;
        };

        int32_t heights[3];
    };

    union {
        struct {
            int32_t y_crop_width;
            int32_t uv_crop_width;
        };

        int32_t crop_widths[2];
    };

    union {
        struct {
            int32_t y_crop_height;
            int32_t uv_crop_height;
        };

        int32_t crop_heights[2];
    };

    union {
        struct {
            int32_t y_stride;
            int32_t uv_stride;
            int32_t alpha_stride;
        };

        int32_t strides[3];
    };

    union {
        struct {
            uint8_t* y_buffer;
            uint8_t* u_buffer;
            uint8_t* v_buffer;
            uint8_t* alpha_buffer;
        };

        uint8_t* buffers[4];
    };

    // Indicate whether y_buffer, u_buffer, and v_buffer points to the internally
    // allocated memory or external buffers.
    int32_t use_external_refernce_buffers;
    // This is needed to store y_buffer, u_buffer, and v_buffer when set reference
    // uses an external refernece, and restore those buffer pointers after the
    // external reference frame is no longer used.
    uint8_t* store_buf_adr[3];

    // If the frame is stored in a 16-bit buffer, this stores an 8-bit version
    // for use in global motion detection. It is allocated on-demand.
    uint8_t* y_buffer_8bit;
    int32_t  buf_8bit_valid;

    uint8_t*                  buffer_alloc;
    size_t                    buffer_alloc_sz;
    int32_t                   border;
    size_t                    frame_size;
    int32_t                   subsampling_x;
    int32_t                   subsampling_y;
    uint32_t                  bit_depth;
    EbColorPrimaries          color_primaries;
    EbTransferCharacteristics transfer_characteristics;
    EbMatrixCoefficients      matrix_coefficients;
    int32_t                   monochrome;
    EbChromaSamplePosition    chroma_sample_position;
    EbColorRange              color_range;
    int32_t                   render_width;
    int32_t                   render_height;

    int32_t corrupted;
    int32_t flags;
} Yv12BufferConfig;

void svt_aom_link_eb_to_aom_buffer_desc(EbPictureBufferDesc* picBuffDsc, Yv12BufferConfig* aomBuffDsc,
                                        uint16_t pad_right, uint16_t pad_bottom, bool is_16bit);

void svt_aom_link_eb_to_aom_buffer_desc_8bit(EbPictureBufferDesc* picBuffDsc, Yv12BufferConfig* aomBuffDsc);

typedef struct AomCodecFrameBuffer {
    uint8_t* data; /**< pointer to the data buffer */
    size_t   size; /**< Size of data in bytes */
    void*    priv; /**< Frame's private data */
} AomCodecFrameBuffer;

/*!\brief get frame buffer callback prototype
    *
    * This callback is invoked by the decoder to retrieve data for the frame
    * buffer in order for the decode call to complete. The callback must
    * allocate at least min_size in bytes and assign it to fb->data. The callback
    * must zero out all the data allocated. Then the callback must set fb->size
    * to the allocated size. The application does not need to align the allocated
    * data. The callback is triggered when the decoder needs a frame buffer to
    * decode a compressed image into. This function may be called more than once
    * for every call to aom_codec_decode. The application may set fb->priv to
    * some data which will be passed back in the ximage and the release function
    * call. |fb| is guaranteed to not be NULL. On success the callback must
    * return 0. Any failure the callback must return a value less than 0.
    *
    * \param[in] priv         Callback's private data
    * \param[in] new_size     Size in bytes needed by the buffer
    * \param[in,out] fb       pointer to AomCodecFrameBuffer
    */
typedef int32_t (*AomGetFrameBufferCbFn)(void* priv, size_t min_size, AomCodecFrameBuffer* fb);

#define AOM_BORDER_IN_PIXELS 288

/************************************
 * EbPictureBufferDesc Init Data
 ************************************/
typedef struct EbPictureBufferDescInitData {
    uint16_t      max_width;
    uint16_t      max_height;
    EbBitDepth    bit_depth;
    EbColorFormat color_format;
    uint32_t      buffer_enable_mask;
    int32_t       rest_units_per_tile;
    uint16_t      left_padding;
    uint16_t      right_padding;
    uint16_t      top_padding;
    uint16_t      bot_padding;
    bool          split_mode; //ON: allocate 8bit data separately from nbit data

    uint8_t mfmv;
    bool    is_16bit_pipeline;
    int32_t sb_total_count;
} EbPictureBufferDescInitData;

/**************************************
     * Extern Function Declarations
     **************************************/

EbErrorType svt_picture_buffer_desc_ctor_noy8b(EbPictureBufferDesc* object_ptr, const EbPtr object_init_data_ptr);
EbErrorType svt_picture_buffer_desc_ctor(EbPictureBufferDesc* object_ptr, const EbPtr object_init_data_ptr);

EbErrorType svt_recon_picture_buffer_desc_ctor(EbPictureBufferDesc* object_ptr, EbPtr object_init_data_ptr);
EbErrorType svt_picture_buffer_desc_noy8b_update(EbPictureBufferDesc* object_ptr, const EbPtr object_init_data_ptr);
EbErrorType svt_picture_buffer_desc_update(EbPictureBufferDesc* pictureBufferDescPtr, const EbPtr object_init_data_ptr);
EbErrorType svt_recon_picture_buffer_desc_update(EbPictureBufferDesc* object_ptr, EbPtr object_init_data_ptr);

int32_t svt_aom_realloc_frame_buffer(Yv12BufferConfig* ybf, int32_t width, int32_t height, int32_t ss_x, int32_t ss_y,
                                     int32_t use_highbitdepth, int32_t border, int32_t byte_alignment,
                                     AomCodecFrameBuffer* fb, AomGetFrameBufferCbFn cb, void* cb_priv);
#ifdef __cplusplus
}
#endif
#endif // EbPictureBuffer_h
