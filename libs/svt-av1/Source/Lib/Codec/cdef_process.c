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

#include <stdlib.h>
#include "aom_dsp_rtcd.h"
#include "definitions.h"
#include "enc_handle.h"
#include "cdef_process.h"
#include "enc_dec_results.h"
#include "svt_threads.h"
#include "reference_object.h"
#include "enc_cdef.h"
#include "enc_dec_process.h"
#include "pic_buffer_desc.h"
#include "sequence_control_set.h"
#include "utility.h"
#include "pcs.h"
#include "resize.h"
#include "super_res.h"

static void set_unscaled_input_16bit(PictureControlSet* pcs) {
    EbPictureBufferDesc* input_pic  = pcs->ppcs->enhanced_unscaled_pic;
    EbPictureBufferDesc* output_pic = pcs->input_frame16bit;
    uint16_t             ss_x       = pcs->ppcs->scs->subsampling_x;
    uint16_t             ss_y       = pcs->ppcs->scs->subsampling_y;
    svt_aom_copy_buffer_info(input_pic, pcs->input_frame16bit);
    if (input_pic->bit_depth == EB_EIGHT_BIT) {
        svt_aom_convert_pic_8bit_to_16bit(input_pic, output_pic, ss_x, ss_y);
    } else {
        uint16_t* planes[3] = {
            (uint16_t*)output_pic->buffer_y + (output_pic->org_y * output_pic->stride_y) + (output_pic->org_x),
            (uint16_t*)output_pic->buffer_cb + (((output_pic->org_y) >> ss_y) * output_pic->stride_cb) +
                ((output_pic->org_x) >> ss_x),
            (uint16_t*)output_pic->buffer_cr + (((output_pic->org_y) >> ss_y) * output_pic->stride_cr) +
                ((output_pic->org_x) >> ss_x)};
        svt_aom_pack_2d_pic(input_pic, planes);
    }
}

static void derive_blk_pointers_enc(EbPictureBufferDesc* recon_picture_buf, int32_t plane, int32_t blk_col_px,
                                    int32_t blk_row_px, void** pp_blk_recon_buf, int32_t* recon_stride, int32_t sub_x,
                                    int32_t sub_y, bool use_highbd) {
    int32_t block_offset;

    if (plane == 0) {
        block_offset = (recon_picture_buf->org_y + blk_row_px) * recon_picture_buf->stride_y +
            (recon_picture_buf->org_x + blk_col_px);
        *recon_stride = recon_picture_buf->stride_y;
    } else if (plane == 1) {
        block_offset = ((recon_picture_buf->org_y >> sub_y) + blk_row_px) * recon_picture_buf->stride_cb +
            ((recon_picture_buf->org_x >> sub_x) + blk_col_px);
        *recon_stride = recon_picture_buf->stride_cb;
    } else {
        block_offset = ((recon_picture_buf->org_y >> sub_y) + blk_row_px) * recon_picture_buf->stride_cr +
            ((recon_picture_buf->org_x >> sub_x) + blk_col_px);
        *recon_stride = recon_picture_buf->stride_cr;
    }

    if (use_highbd) { //16bit
        if (plane == 0) {
            *pp_blk_recon_buf = (void*)((uint16_t*)recon_picture_buf->buffer_y + block_offset);
        } else if (plane == 1) {
            *pp_blk_recon_buf = (void*)((uint16_t*)recon_picture_buf->buffer_cb + block_offset);
        } else {
            *pp_blk_recon_buf = (void*)((uint16_t*)recon_picture_buf->buffer_cr + block_offset);
        }
    } else {
        if (plane == 0) {
            *pp_blk_recon_buf = (void*)((uint8_t*)recon_picture_buf->buffer_y + block_offset);
        } else if (plane == 1) {
            *pp_blk_recon_buf = (void*)((uint8_t*)recon_picture_buf->buffer_cb + block_offset);
        } else {
            *pp_blk_recon_buf = (void*)((uint8_t*)recon_picture_buf->buffer_cr + block_offset);
        }
    }
}

static EbErrorType copy_recon_enc(SequenceControlSet* scs, EbPictureBufferDesc* recon_picture_src,
                                  EbPictureBufferDesc* recon_picture_dst, int num_planes, int skip_copy) {
    recon_picture_dst->org_x        = recon_picture_src->org_x;
    recon_picture_dst->org_y        = recon_picture_src->org_y;
    recon_picture_dst->origin_bot_y = recon_picture_src->origin_bot_y;
    recon_picture_dst->width        = recon_picture_src->width;
    recon_picture_dst->height       = recon_picture_src->height;
    recon_picture_dst->max_width    = recon_picture_src->max_width;
    recon_picture_dst->max_height   = recon_picture_src->max_height;
    recon_picture_dst->bit_depth    = recon_picture_src->bit_depth;
    recon_picture_dst->color_format = recon_picture_src->color_format;

    recon_picture_dst->stride_y  = recon_picture_src->stride_y;
    recon_picture_dst->stride_cb = recon_picture_src->stride_cb;
    recon_picture_dst->stride_cr = recon_picture_src->stride_cr;

    recon_picture_dst->luma_size   = recon_picture_src->luma_size;
    recon_picture_dst->chroma_size = recon_picture_src->chroma_size;
    recon_picture_dst->packed_flag = recon_picture_src->packed_flag;

    recon_picture_dst->stride_bit_inc_y  = recon_picture_src->stride_bit_inc_y;
    recon_picture_dst->stride_bit_inc_cb = recon_picture_src->stride_bit_inc_cb;
    recon_picture_dst->stride_bit_inc_cr = recon_picture_src->stride_bit_inc_cr;

    recon_picture_dst->buffer_enable_mask = scs->seq_header.color_config.mono_chrome ? PICTURE_BUFFER_DESC_LUMA_MASK
                                                                                     : PICTURE_BUFFER_DESC_FULL_MASK;

    uint32_t bytesPerPixel = scs->is_16bit_pipeline ? 2 : 1;

    // Allocate the Picture Buffers (luma & chroma)
    if (recon_picture_dst->buffer_enable_mask & PICTURE_BUFFER_DESC_Y_FLAG) {
        EB_MALLOC_ALIGNED(recon_picture_dst->buffer_y, recon_picture_dst->luma_size * bytesPerPixel);
        svt_memset(recon_picture_dst->buffer_y, 0, recon_picture_dst->luma_size * bytesPerPixel);
    } else {
        recon_picture_dst->buffer_y = 0;
    }
    if (recon_picture_dst->buffer_enable_mask & PICTURE_BUFFER_DESC_Cb_FLAG) {
        EB_MALLOC_ALIGNED(recon_picture_dst->buffer_cb, recon_picture_dst->chroma_size * bytesPerPixel);
        svt_memset(recon_picture_dst->buffer_cb, 0, recon_picture_dst->chroma_size * bytesPerPixel);
    } else {
        recon_picture_dst->buffer_cb = 0;
    }
    if (recon_picture_dst->buffer_enable_mask & PICTURE_BUFFER_DESC_Cr_FLAG) {
        EB_MALLOC_ALIGNED(recon_picture_dst->buffer_cr, recon_picture_dst->chroma_size * bytesPerPixel);
        svt_memset(recon_picture_dst->buffer_cr, 0, recon_picture_dst->chroma_size * bytesPerPixel);
    } else {
        recon_picture_dst->buffer_cr = 0;
    }

    int use_highbd = scs->is_16bit_pipeline;

    if (!skip_copy) {
        for (int plane = 0; plane < num_planes; ++plane) {
            uint8_t *src_buf, *dst_buf;
            int32_t  src_stride, dst_stride;

            int sub_x = plane ? scs->subsampling_x : 0;
            int sub_y = plane ? scs->subsampling_y : 0;

            derive_blk_pointers_enc(
                recon_picture_src, plane, 0, 0, (void*)&src_buf, &src_stride, sub_x, sub_y, use_highbd);
            derive_blk_pointers_enc(
                recon_picture_dst, plane, 0, 0, (void*)&dst_buf, &dst_stride, sub_x, sub_y, use_highbd);

            int height = ((recon_picture_src->height + sub_y) >> sub_y);
            for (int row = 0; row < height; ++row) {
                svt_memcpy(
                    dst_buf, src_buf, ((recon_picture_src->width + sub_x) >> sub_x) * sizeof(*src_buf) << use_highbd);
                src_buf += src_stride << use_highbd;
                dst_buf += dst_stride << use_highbd;
            }
        }
    }

    return EB_ErrorNone;
}

static void svt_av1_superres_upscale_frame(struct Av1Common* cm, PictureControlSet* pcs, SequenceControlSet* scs) {
    // Set these parameters for testing since they are not correctly populated yet
    EbPictureBufferDesc* recon_ptr;

    bool is_16bit = scs->is_16bit_pipeline;

    svt_aom_get_recon_pic(pcs, &recon_ptr, is_16bit);

    uint16_t  ss_x       = scs->subsampling_x;
    uint16_t  ss_y       = scs->subsampling_y;
    const int num_planes = scs->seq_header.color_config.mono_chrome ? 1 : MAX_MB_PLANE;

    EbPictureBufferDesc  recon_pic_temp;
    EbPictureBufferDesc* ps_recon_pic_temp;
    ps_recon_pic_temp = &recon_pic_temp;

    EbErrorType return_error = copy_recon_enc(scs, recon_ptr, ps_recon_pic_temp, num_planes, 0);

    if (return_error != EB_ErrorNone) {
        ps_recon_pic_temp = NULL;
        assert(0);
    }

    EbPictureBufferDesc* src = ps_recon_pic_temp;
    EbPictureBufferDesc* dst = recon_ptr;

    // get the bit-depth from the encoder config instead of from the recon ptr
    int bit_depth = scs->static_config.encoder_bit_depth;

    for (int plane = 0; plane < num_planes; ++plane) {
        uint8_t *src_buf, *dst_buf;
        int32_t  src_stride, dst_stride;

        int sub_x = plane ? ss_x : 0;
        int sub_y = plane ? ss_y : 0;
        derive_blk_pointers_enc(src, plane, 0, 0, (void*)&src_buf, &src_stride, sub_x, sub_y, is_16bit);
        derive_blk_pointers_enc(dst, plane, 0, 0, (void*)&dst_buf, &dst_stride, sub_x, sub_y, is_16bit);

        svt_av1_upscale_normative_rows(cm,
                                       (const uint8_t*)src_buf,
                                       src_stride,
                                       dst_buf,
                                       dst_stride,
                                       (src->height + sub_y) >> sub_y,
                                       sub_x,
                                       bit_depth,
                                       is_16bit);
    }

    // free the memory
    EB_FREE_ALIGNED_ARRAY(ps_recon_pic_temp->buffer_y);
    EB_FREE_ALIGNED_ARRAY(ps_recon_pic_temp->buffer_cb);
    EB_FREE_ALIGNED_ARRAY(ps_recon_pic_temp->buffer_cr);
}

/**************************************
 * Cdef Context
 **************************************/
typedef struct CdefContext {
    EbFifo* cdef_input_fifo_ptr;
    EbFifo* cdef_output_fifo_ptr;
} CdefContext;

static void cdef_context_dctor(EbPtr p) {
    EbThreadContext* thread_ctx = (EbThreadContext*)p;
    CdefContext*     obj        = (CdefContext*)thread_ctx->priv;
    EB_FREE_ARRAY(obj);
}

/******************************************************
 * Cdef Context Constructor
 ******************************************************/
EbErrorType svt_aom_cdef_context_ctor(EbThreadContext* thread_ctx, const EbEncHandle* enc_handle_ptr, int index) {
    CdefContext* cdef_ctx;
    EB_CALLOC_ARRAY(cdef_ctx, 1);
    thread_ctx->priv  = cdef_ctx;
    thread_ctx->dctor = cdef_context_dctor;

    // Input/Output System Resource Manager FIFOs
    cdef_ctx->cdef_input_fifo_ptr  = svt_system_resource_get_consumer_fifo(enc_handle_ptr->dlf_results_resource_ptr,
                                                                          index);
    cdef_ctx->cdef_output_fifo_ptr = svt_system_resource_get_producer_fifo(enc_handle_ptr->cdef_results_resource_ptr,
                                                                           index);

    return EB_ErrorNone;
}

#define default_mse_uv 1040400

static uint64_t compute_cdef_dist(const EbByte dst, int32_t doffset, int32_t dstride, const uint8_t* src,
                                  const CdefList* dlist, int32_t cdef_count, BlockSize bsize, int32_t coeff_shift,
                                  uint8_t subsampling_factor, bool is_16bit) {
    uint64_t curr_mse = 0;
    if (is_16bit) {
        curr_mse = svt_compute_cdef_dist_16bit(((uint16_t*)dst) + doffset,
                                               dstride,
                                               (uint16_t*)src,
                                               dlist,
                                               cdef_count,
                                               bsize,
                                               coeff_shift,
                                               subsampling_factor);

    } else {
        curr_mse = svt_compute_cdef_dist_8bit(
            dst + doffset, dstride, src, dlist, cdef_count, bsize, coeff_shift, subsampling_factor);
    }
    return curr_mse;
}

/* Search for the best filter strength pair for each 64x64 filter block.
 *
 * For each 64x64 filter block and each plane, search the allowable filter strength pairs.
 * Call cdef_filter_fb() to perform filtering, then compute the MSE for each pair.
*/
static void cdef_seg_search(PictureControlSet* pcs, SequenceControlSet* scs, uint32_t segment_index) {
    PictureParentControlSet* ppcs     = pcs->ppcs;
    FrameHeader*             frm_hdr  = &ppcs->frm_hdr;
    Av1Common*               cm       = ppcs->av1_cm;
    const bool               is_16bit = scs->is_16bit_pipeline;
    uint32_t                 x_seg_idx;
    uint32_t                 y_seg_idx;
    const uint32_t           b64_pic_width  = (ppcs->aligned_width + 64 - 1) / 64;
    const uint32_t           b64_pic_height = (ppcs->aligned_height + 64 - 1) / 64;
    SEGMENT_CONVERT_IDX_TO_XY(segment_index, x_seg_idx, y_seg_idx, pcs->cdef_segments_column_count);
    const uint32_t x_b64_start_idx = SEGMENT_START_IDX(x_seg_idx, b64_pic_width, pcs->cdef_segments_column_count);
    const uint32_t x_b64_end_idx   = SEGMENT_END_IDX(x_seg_idx, b64_pic_width, pcs->cdef_segments_column_count);
    const uint32_t y_b64_start_idx = SEGMENT_START_IDX(y_seg_idx, b64_pic_height, pcs->cdef_segments_row_count);
    const uint32_t y_b64_end_idx   = SEGMENT_END_IDX(y_seg_idx, b64_pic_height, pcs->cdef_segments_row_count);

    const int32_t       mi_rows                    = cm->mi_rows;
    const int32_t       mi_cols                    = cm->mi_cols;
    CdefSearchControls* cdef_ctrls                 = &ppcs->cdef_search_ctrls;
    const int           first_pass_fs_num          = cdef_ctrls->first_pass_fs_num;
    const int           default_second_pass_fs_num = cdef_ctrls->default_second_pass_fs_num;
    EbByte              src[3];
    EbByte              ref[3];
    int32_t             stride_src[3];
    int32_t             stride_ref[3];
    int32_t             plane_bsize[3];
    int32_t             mi_wide_l2[3];
    int32_t             mi_high_l2[3];
    int32_t             xdec[3];
    int32_t             ydec[3];
    int32_t             cdef_count;
    const int32_t       coeff_shift = AOMMAX(scs->static_config.encoder_bit_depth - 8, 0);
    const int32_t       nvfb        = (mi_rows + MI_SIZE_64X64 - 1) / MI_SIZE_64X64;
    const int32_t       nhfb        = (mi_cols + MI_SIZE_64X64 - 1) / MI_SIZE_64X64;
    const int32_t       pri_damping = 3 + (frm_hdr->quantization_params.base_q_idx >> 6);
    const int32_t       sec_damping = pri_damping;
    const int32_t       num_planes  = 3;
    CdefList            dlist[MI_SIZE_128X128 * MI_SIZE_128X128];

    int32_t toff_prev  = CDEF_VBORDER;
    int32_t loff_prev  = CDEF_HBORDER;
    int32_t ysize_prev = (1 << MAX_SB_SIZE_LOG2) + 2 * CDEF_VBORDER;
    int32_t xsize_prev = (1 << MAX_SB_SIZE_LOG2) + 2 * CDEF_HBORDER;
    DECLARE_ALIGNED(32, uint16_t, inbuf[CDEF_INBUF_SIZE]);
    uint16_t* in = inbuf + CDEF_VBORDER * CDEF_BSTRIDE + CDEF_HBORDER;
    // tmp_dst is uint16_t to accommodate high bit depth content; 8bit will treat it as a uint8_t
    // buffer and will not use half of the buffer
    DECLARE_ALIGNED(32, uint16_t, tmp_dst[1 << (MAX_SB_SIZE_LOG2 * 2)]);

    EbPictureBufferDesc* input_pic = is_16bit ? pcs->input_frame16bit : ppcs->enhanced_pic;
    EbPictureBufferDesc* recon_pic;
    svt_aom_get_recon_pic(pcs, &recon_pic, is_16bit);

    for (int pli = 0; pli < num_planes; pli++) {
        const int subsampling_x = (pli == 0) ? 0 : 1;
        const int subsampling_y = (pli == 0) ? 0 : 1;
        xdec[pli]               = subsampling_x;
        ydec[pli]               = subsampling_y;
        // The checks are stubs for 4:2:2 and 4:4:4 support
        // cppcheck-suppress knownConditionTrueFalse
        plane_bsize[pli] = subsampling_y ? (subsampling_x ? BLOCK_4X4 : BLOCK_8X4)
                                         : (subsampling_x ? BLOCK_4X8 : BLOCK_8X8);
        mi_wide_l2[pli]  = MI_SIZE_LOG2 - subsampling_x;
        mi_high_l2[pli]  = MI_SIZE_LOG2 - subsampling_y;
        src[pli]         = pcs->cdef_input_recon[pli];
        ref[pli]         = pcs->cdef_input_source[pli];
        stride_src[pli]  = pli == 0 ? recon_pic->stride_y : (pli == 1 ? recon_pic->stride_cb : recon_pic->stride_cr);
        stride_ref[pli]  = pli == 0 ? input_pic->stride_y : (pli == 1 ? input_pic->stride_cb : input_pic->stride_cr);
    }

    // Loop over all filter blocks (64x64)
    for (uint32_t fbr = y_b64_start_idx; fbr < y_b64_end_idx; ++fbr) {
        for (uint32_t fbc = x_b64_start_idx; fbc < x_b64_end_idx; ++fbc) {
            int32_t           dirinit = 0;
            const uint32_t    lc      = MI_SIZE_64X64 * fbc;
            const uint32_t    lr      = MI_SIZE_64X64 * fbr;
            int               nhb     = AOMMIN(MI_SIZE_64X64, mi_cols - lc);
            int               nvb     = AOMMIN(MI_SIZE_64X64, mi_rows - lr);
            int               hb_step = 1; //these should be all time with 64x64 SBs
            int               vb_step = 1;
            BlockSize         bs      = BLOCK_64X64;
            const MbModeInfo* mbmi    = pcs->mi_grid_base[lr * cm->mi_stride + lc];
            const BlockSize   bsize   = mbmi->bsize;
            if (((fbc & 1) && (bsize == BLOCK_128X128 || bsize == BLOCK_128X64)) ||
                ((fbr & 1) && (bsize == BLOCK_128X128 || bsize == BLOCK_64X128))) {
                continue;
            }
            if (bsize == BLOCK_128X128 || bsize == BLOCK_128X64 || bsize == BLOCK_64X128) {
                bs = bsize;
            }

            if (bs == BLOCK_128X128 || bs == BLOCK_128X64) {
                nhb     = AOMMIN(MI_SIZE_128X128, cm->mi_cols - lc);
                hb_step = 2;
            }
            if (bs == BLOCK_128X128 || bs == BLOCK_64X128) {
                nvb     = AOMMIN(MI_SIZE_128X128, cm->mi_rows - lr);
                vb_step = 2;
            }
            const uint32_t fb_idx = fbr * nhfb + fbc;
            // No filtering if the entire filter block is skipped
            cdef_count = svt_sb_compute_cdef_list(pcs, cm, lr, lc, dlist, bs);
            if (cdef_count == 0) {
                pcs->skip_cdef_seg[fb_idx] = 1;
                continue;
            }
            pcs->skip_cdef_seg[fb_idx] = 0;

            int32_t toff = CDEF_VBORDER * (fbr != 0);
            int32_t loff = CDEF_HBORDER * (fbc != 0);
            int32_t boff = CDEF_VBORDER * ((int32_t)fbr + vb_step < nvfb);
            int32_t roff = CDEF_HBORDER * ((int32_t)fbc + hb_step < nhfb);

            uint8_t (*dir)[CDEF_NBLOCKS][CDEF_NBLOCKS] = &pcs->cdef_dir_data[fb_idx].dir;
            int32_t (*var)[CDEF_NBLOCKS][CDEF_NBLOCKS] = &pcs->cdef_dir_data[fb_idx].var;
            for (int pli = 0; pli < num_planes; pli++) {
                int32_t ysize = (nvb << mi_high_l2[pli]) + boff + toff;
                int32_t xsize = (nhb << mi_wide_l2[pli]) + roff + loff;
                /* We avoid filtering the pixels for which some of the pixels to
                   average are outside the frame. We could change the filter instead,
                   but it would add special cases for any future vectorization.
                   Avoid memset'ting when dirty rect is inside the new one.
                   TODO: this could be further optimized - fill out only borders, separate buffers for Y & UV */
                bool need_to_reset = toff_prev > toff || loff_prev > loff || ysize < ysize_prev || xsize < xsize_prev;
                if (need_to_reset) {
                    uint16_t* p = &in[(-toff_prev * CDEF_BSTRIDE - loff_prev)];
                    for (int r = 0; r < ysize_prev; r++) {
                        svt_memset(p, (uint8_t)CDEF_VERY_LARGE, sizeof(p[0]) * xsize_prev);
                        p += CDEF_BSTRIDE;
                    }
                }
                toff_prev  = toff;
                loff_prev  = loff;
                ysize_prev = ysize;
                xsize_prev = xsize;

                svt_aom_copy_sb8_16(&in[(-toff * CDEF_BSTRIDE - loff)],
                                    CDEF_BSTRIDE,
                                    src[pli],
                                    (lr << mi_high_l2[pli]) - toff,
                                    (lc << mi_wide_l2[pli]) - loff,
                                    stride_src[pli],
                                    ysize,
                                    xsize,
                                    is_16bit);

                uint8_t subsampling_factor = cdef_ctrls->subsampling_factor;
                /*
                Cap the subsampling for certain block sizes.

                The intrinsics process several lines simultaneously, so blocks can only be subsampled
                a finite amount before there is no more speed gain.  If the space between processed lines
                is too large, the intrinsics will begin accessing memory outside the block.
                */
                switch (plane_bsize[pli]) {
                case BLOCK_8X8:
                    subsampling_factor = MIN(subsampling_factor, 4);
                    break;
                case BLOCK_8X4:
                case BLOCK_4X8:
                    subsampling_factor = MIN(subsampling_factor, 2);
                    break;
                case BLOCK_4X4:
                    subsampling_factor = MIN(subsampling_factor, 1);
                    break;
                }

                /* first cdef stage
                 * Perform the pri_filter strength search for the current sub_block
                 */
                for (int gi = 0; gi < first_pass_fs_num; gi++) {
                    // Check if chroma filter is set to be tested
                    if (pli && (cdef_ctrls->default_first_pass_fs_uv[gi] == -1)) {
                        pcs->mse_seg[1][fb_idx][gi] = default_mse_uv * 64;
                        continue;
                    }

                    int32_t pri_strength = cdef_ctrls->default_first_pass_fs[gi] / CDEF_SEC_STRENGTHS;
                    int32_t sec_strength = cdef_ctrls->default_first_pass_fs[gi] % CDEF_SEC_STRENGTHS;

                    svt_cdef_filter_fb(is_16bit ? NULL : (uint8_t*)tmp_dst,
                                       is_16bit ? tmp_dst : NULL,
                                       0,
                                       in,
                                       xdec[pli],
                                       ydec[pli],
                                       *dir,
                                       &dirinit,
                                       *var,
                                       pli,
                                       dlist,
                                       cdef_count,
                                       pri_strength,
                                       sec_strength + (sec_strength == 3),
                                       pri_damping,
                                       sec_damping,
                                       coeff_shift,
                                       subsampling_factor);
                    uint64_t curr_mse = compute_cdef_dist(
                        ref[pli],
                        (lr << mi_high_l2[pli]) * stride_ref[pli] + (lc << mi_wide_l2[pli]),
                        stride_ref[pli],
                        (uint8_t*)tmp_dst,
                        dlist,
                        cdef_count,
                        (BlockSize)plane_bsize[pli],
                        coeff_shift,
                        subsampling_factor,
                        is_16bit);

                    if (pli < 2) {
                        pcs->mse_seg[pli][fb_idx][gi] = curr_mse * subsampling_factor;
                    } else {
                        pcs->mse_seg[1][fb_idx][gi] += (curr_mse * subsampling_factor);
                    }
                }

                /* second cdef stage
                 * Perform the sec_filter strength search for the current sub_block
                 */
                for (int gi = first_pass_fs_num; gi < first_pass_fs_num + default_second_pass_fs_num; gi++) {
                    // Check if chroma filter is set to be tested
                    if (pli && (cdef_ctrls->default_second_pass_fs_uv[gi - first_pass_fs_num] == -1)) {
                        pcs->mse_seg[1][fb_idx][gi] = default_mse_uv * 64;
                        continue;
                    }

                    int32_t pri_strength = cdef_ctrls->default_second_pass_fs[gi - first_pass_fs_num] /
                        CDEF_SEC_STRENGTHS;
                    int32_t sec_strength = cdef_ctrls->default_second_pass_fs[gi - first_pass_fs_num] %
                        CDEF_SEC_STRENGTHS;

                    svt_cdef_filter_fb(is_16bit ? NULL : (uint8_t*)tmp_dst,
                                       is_16bit ? tmp_dst : NULL,
                                       0,
                                       in,
                                       xdec[pli],
                                       ydec[pli],
                                       *dir,
                                       &dirinit,
                                       *var,
                                       pli,
                                       dlist,
                                       cdef_count,
                                       pri_strength,
                                       sec_strength + (sec_strength == 3),
                                       pri_damping,
                                       sec_damping,
                                       coeff_shift,
                                       subsampling_factor);
                    uint64_t curr_mse = compute_cdef_dist(
                        ref[pli],
                        (lr << mi_high_l2[pli]) * stride_ref[pli] + (lc << mi_wide_l2[pli]),
                        stride_ref[pli],
                        (uint8_t*)tmp_dst,
                        dlist,
                        cdef_count,
                        (BlockSize)plane_bsize[pli],
                        coeff_shift,
                        subsampling_factor,
                        is_16bit);

                    if (pli < 2) {
                        pcs->mse_seg[pli][fb_idx][gi] = curr_mse * subsampling_factor;
                    } else {
                        pcs->mse_seg[1][fb_idx][gi] += (curr_mse * subsampling_factor);
                    }
                }
            }
        }
    }
}

/******************************************************
 * CDEF Kernel
 ******************************************************/
void* svt_aom_cdef_kernel(void* input_ptr) {
    // Context & SCS & PCS
    EbThreadContext*    thread_ctx  = (EbThreadContext*)input_ptr;
    CdefContext*        context_ptr = (CdefContext*)thread_ctx->priv;
    PictureControlSet*  pcs;
    SequenceControlSet* scs;

    //// Input
    EbObjectWrapper* dlf_results_wrapper;
    DlfResults*      dlf_results;

    //// Output
    EbObjectWrapper* cdef_results_wrapper;
    CdefResults*     cdef_results;

    // SB Loop variables

    for (;;) {
        FrameHeader* frm_hdr;

        // Get DLF Results
        EB_GET_FULL_OBJECT(context_ptr->cdef_input_fifo_ptr, &dlf_results_wrapper);

        dlf_results                   = (DlfResults*)dlf_results_wrapper->object_ptr;
        pcs                           = (PictureControlSet*)dlf_results->pcs_wrapper->object_ptr;
        PictureParentControlSet* ppcs = pcs->ppcs;
        scs                           = pcs->scs;

        bool       is_16bit                   = scs->is_16bit_pipeline;
        Av1Common* cm                         = pcs->ppcs->av1_cm;
        frm_hdr                               = &pcs->ppcs->frm_hdr;
        CdefSearchControls* cdef_search_ctrls = &pcs->ppcs->cdef_search_ctrls;
#if OPT_Q_CDEF
        if (!cdef_search_ctrls->use_reference_cdef_fs && !cdef_search_ctrls->use_qp_strength) {
#else
        if (!cdef_search_ctrls->use_reference_cdef_fs) {
#endif
            if (scs->seq_header.cdef_level && pcs->ppcs->cdef_level) {
                cdef_seg_search(pcs, scs, dlf_results->segment_index);
            }
        }
        //all seg based search is done. update total processed segments. if all done, finish the search and perfrom application.
        svt_block_on_mutex(pcs->cdef_search_mutex);

        pcs->tot_seg_searched_cdef++;
        if (pcs->tot_seg_searched_cdef == pcs->cdef_segments_total_count) {
            pcs->cdef_dist_dev = -1;
            if (scs->seq_header.cdef_level && pcs->ppcs->cdef_level) {
                finish_cdef_search(pcs);
                if (ppcs->enable_restoration || pcs->ppcs->is_ref || scs->static_config.recon_enabled) {
                    // Do application iff there are non-zero filters
                    if (frm_hdr->cdef_params.cdef_y_strength[0] != 0 || frm_hdr->cdef_params.cdef_uv_strength[0] != 0 ||
                        pcs->ppcs->nb_cdef_strengths != 1) {
                        svt_av1_cdef_frame(scs, pcs);
                    }
                }
            } else {
                frm_hdr->cdef_params.cdef_bits           = 0;
                frm_hdr->cdef_params.cdef_y_strength[0]  = 0;
                pcs->ppcs->nb_cdef_strengths             = 1;
                frm_hdr->cdef_params.cdef_uv_strength[0] = 0;
            }

            if (pcs->ppcs->nb_cdef_strengths == 1 && frm_hdr->cdef_params.cdef_y_strength[0] == 0 &&
                frm_hdr->cdef_params.cdef_uv_strength[0] == 0) {
                pcs->cdef_dist_dev = 0;
            }

            //restoration prep
            bool is_lr = ppcs->enable_restoration && frm_hdr->allow_intrabc == 0;
            if (is_lr) {
                svt_av1_loop_restoration_save_boundary_lines(cm->frame_to_show, cm, 1);
                if (is_16bit) {
                    set_unscaled_input_16bit(pcs);
                }
            }

            // ------- start: Normative upscaling - super-resolution tool
            if (frm_hdr->allow_intrabc == 0 && pcs->ppcs->frame_superres_enabled) {
                svt_av1_superres_upscale_frame(cm, pcs, scs);
            }
            if (scs->static_config.resize_mode != RESIZE_NONE) {
                EbPictureBufferDesc* recon = NULL;
                svt_aom_get_recon_pic(pcs, &recon, is_16bit);
                recon->width  = pcs->ppcs->render_width;
                recon->height = pcs->ppcs->render_height;
                if (is_lr) {
                    EbPictureBufferDesc* input_pic = is_16bit ? pcs->input_frame16bit
                                                              : pcs->ppcs->enhanced_unscaled_pic;

                    svt_aom_assert_err(pcs->scaled_input_pic == NULL, "pcs_ptr->scaled_input_pic is not desctoried!");
                    EbPictureBufferDesc* scaled_input_pic = NULL;
                    // downscale input picture if recon is resized
                    bool is_resized = recon->width != input_pic->width || recon->height != input_pic->height;
                    if (is_resized) {
                        superres_params_type spr_params = {recon->width, recon->height, 0};
                        svt_aom_downscaled_source_buffer_desc_ctor(&scaled_input_pic, input_pic, spr_params);
                        svt_aom_resize_frame(input_pic,
                                             scaled_input_pic,
                                             scs->static_config.encoder_bit_depth,
                                             av1_num_planes(&scs->seq_header.color_config),
                                             scs->subsampling_x,
                                             scs->subsampling_y,
                                             input_pic->packed_flag,
                                             PICTURE_BUFFER_DESC_FULL_MASK,
                                             0); // is_2bcompress
                        pcs->scaled_input_pic = scaled_input_pic;
                    }
                }
            }
            // ------- end: Normative upscaling - super-resolution tool

            pcs->rest_segments_column_count = scs->rest_segment_column_count;
            pcs->rest_segments_row_count    = scs->rest_segment_row_count;
            pcs->rest_segments_total_count = (uint16_t)(pcs->rest_segments_column_count * pcs->rest_segments_row_count);
            pcs->tot_seg_searched_rest     = 0;
            pcs->ppcs->av1_cm->use_boundaries_in_rest_search = scs->use_boundaries_in_rest_search;
            pcs->rest_extend_flag[0]                         = false;
            pcs->rest_extend_flag[1]                         = false;
            pcs->rest_extend_flag[2]                         = false;

            uint32_t segment_index;
            for (segment_index = 0; segment_index < pcs->rest_segments_total_count; ++segment_index) {
                // Get Empty Cdef Results to Rest
                svt_get_empty_object(context_ptr->cdef_output_fifo_ptr, &cdef_results_wrapper);
                cdef_results                = (struct CdefResults*)cdef_results_wrapper->object_ptr;
                cdef_results->pcs_wrapper   = dlf_results->pcs_wrapper;
                cdef_results->segment_index = segment_index;
                // Post Cdef Results
                svt_post_full_object(cdef_results_wrapper);
            }
        }
        svt_release_mutex(pcs->cdef_search_mutex);

        // Release Dlf Results
        svt_release_object(dlf_results_wrapper);
    }

    return NULL;
}
