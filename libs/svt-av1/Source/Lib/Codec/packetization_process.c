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

#include "enc_handle.h"
#include "packetization_process.h"
#include "ec_results.h"
#include "sequence_control_set.h"
#include "pcs.h"
#include "entropy_coding.h"
#include "rc_tasks.h"
#include "svt_time.h"
#include "md_process.h"
#include "pic_demux_results.h"
#include "svt_log.h"
#include "EbSvtAv1ErrorCodes.h"
#include "pd_results.h"
#include "restoration.h" // RDCOST_DBL
#include "rc_process.h"
#include "enc_mode_config.h"

#define RDCOST_DBL_WITH_NATIVE_BD_DIST(RM, R, D, BD) RDCOST_DBL((RM), (R), (double)((D) >> (2 * (BD - 8))))

/**************************************
 * Context
 **************************************/
typedef struct PacketizationContext {
    EbDctor dctor;
    EbFifo* entropy_coding_input_fifo_ptr;
    EbFifo* rate_control_tasks_output_fifo_ptr;
    EbFifo* picture_decision_results_output_fifo_ptr; // to motion estimation process
    EbFifo* picture_demux_fifo_ptr; // to picture manager process
#if DETAILED_FRAME_OUTPUT
    uint64_t dpb_disp_order[8], dpb_dec_order[8];
#endif
    uint64_t tot_shown_frames;
    uint64_t disp_order_continuity_count;
} PacketizationContext;

void        free_temporal_filtering_buffer(PictureControlSet* pcs, SequenceControlSet* scs);
void        svt_aom_recon_output(PictureControlSet* pcs, SequenceControlSet* scs);
void        svt_aom_init_resize_picture(SequenceControlSet* scs, PictureParentControlSet* pcs);
void        pad_ref_and_set_flags(PictureControlSet* pcs, SequenceControlSet* scs);
void        svt_aom_update_rc_counts(PictureParentControlSet* ppcs);
EbErrorType svt_aom_ssim_calculations(PictureControlSet* pcs, SequenceControlSet* scs, bool free_memory);

static void packetization_context_dctor(EbPtr p) {
    EbThreadContext*      thread_ctx = (EbThreadContext*)p;
    PacketizationContext* obj        = (PacketizationContext*)thread_ctx->priv;
    EB_FREE_ARRAY(obj);
}

EbErrorType svt_aom_packetization_context_ctor(EbThreadContext* thread_ctx, const EbEncHandle* enc_handle_ptr,
                                               int rate_control_index, int demux_index, int me_port_index) {
    PacketizationContext* context_ptr;
    EB_CALLOC_ARRAY(context_ptr, 1);
    thread_ctx->priv  = context_ptr;
    thread_ctx->dctor = packetization_context_dctor;

    context_ptr->dctor                         = packetization_context_dctor;
    context_ptr->entropy_coding_input_fifo_ptr = svt_system_resource_get_consumer_fifo(
        enc_handle_ptr->entropy_coding_results_resource_ptr, 0);
    context_ptr->rate_control_tasks_output_fifo_ptr = svt_system_resource_get_producer_fifo(
        enc_handle_ptr->rate_control_tasks_resource_ptr, rate_control_index);
    context_ptr->picture_demux_fifo_ptr = svt_system_resource_get_producer_fifo(
        enc_handle_ptr->picture_demux_results_resource_ptr, demux_index);
    context_ptr->picture_decision_results_output_fifo_ptr = svt_system_resource_get_producer_fifo(
        enc_handle_ptr->picture_decision_results_resource_ptr, me_port_index);

    return EB_ErrorNone;
}

static inline int get_reorder_queue_pos(const EncodeContext* enc_ctx, int delta) {
    return (enc_ctx->packetization_reorder_queue_head_index + delta) % enc_ctx->packetization_reorder_queue_size;
}

static inline PacketizationReorderEntry* get_reorder_queue_entry(const EncodeContext* enc_ctx, int delta) {
    int pos = get_reorder_queue_pos(enc_ctx, delta);
    return enc_ctx->packetization_reorder_queue[pos];
}

static uint32_t count_frames_in_next_tu(const EncodeContext* enc_ctx, uint32_t* data_size) {
    uint32_t i = 0;
    *data_size = 0;
    do {
        const PacketizationReorderEntry* queue_entry_ptr = get_reorder_queue_entry(enc_ctx, i);
        const EbObjectWrapper*           wrapper         = queue_entry_ptr->output_stream_wrapper_ptr;
        // not a completed td
        if (!wrapper) {
            return 0;
        }

        const EbBufferHeaderType* output_stream_ptr = (EbBufferHeaderType*)wrapper->object_ptr;
        *data_size += output_stream_ptr->n_filled_len;

        i++;
        //we have a td when we got a displable frame
        if (queue_entry_ptr->show_frame) {
            break;
        }
        if (output_stream_ptr->flags & EB_BUFFERFLAG_EOS) {
            break;
        }

    } while (i < enc_ctx->packetization_reorder_queue_size);
    return i;
}

static int pts_descend(const void* pa, const void* pb) {
    EbObjectWrapper*    a  = *(EbObjectWrapper**)pa;
    EbObjectWrapper*    b  = *(EbObjectWrapper**)pb;
    EbBufferHeaderType* ba = (EbBufferHeaderType*)(a->object_ptr);
    EbBufferHeaderType* bb = (EbBufferHeaderType*)(b->object_ptr);
    return (int)(bb->pts - ba->pts);
}

static void push_undisplayed_frame(EncodeContext* enc_ctx, EbObjectWrapper* wrapper) {
    if (enc_ctx->picture_decision_undisplayed_queue_count >= UNDISP_QUEUE_SIZE) {
        SVT_ERROR("bug, too many frames in undisplayed queue");
        return;
    }
    uint32_t count                                       = enc_ctx->picture_decision_undisplayed_queue_count;
    enc_ctx->picture_decision_undisplayed_queue[count++] = wrapper;
    enc_ctx->picture_decision_undisplayed_queue_count    = count;
}

static EbObjectWrapper* pop_undisplayed_frame(EncodeContext* enc_ctx) {
    uint32_t count = enc_ctx->picture_decision_undisplayed_queue_count;
    if (!count) {
        SVT_ERROR("bug, no frame in undisplayed queue");
        return NULL;
    }
    count--;
    EbObjectWrapper* ret                              = enc_ctx->picture_decision_undisplayed_queue[count];
    enc_ctx->picture_decision_undisplayed_queue_count = count;
    return ret;
}

static void sort_undisplayed_frame(EncodeContext* enc_ctx) {
    qsort(&enc_ctx->picture_decision_undisplayed_queue[0],
          enc_ctx->picture_decision_undisplayed_queue_count,
          sizeof(EbObjectWrapper*),
          pts_descend);
}

#if DETAILED_FRAME_OUTPUT
static void print_detailed_frame_info(PacketizationContext*            context_ptr,
                                      const PacketizationReorderEntry* queue_entry_ptr) {
    int32_t i;
    uint8_t showTab[] = {'H', 'V'};

    //Countinuity count check of visible frames
    if (queue_entry_ptr->show_frame) {
        if (context_ptr->disp_order_continuity_count == queue_entry_ptr->poc) {
            context_ptr->disp_order_continuity_count++;
        } else {
            SVT_ERROR("disp_order_continuity_count Error1 POC:%i\n", (int32_t)queue_entry_ptr->poc);
            exit(0);
        }
    }

    if (queue_entry_ptr->has_show_existing) {
        if (context_ptr->disp_order_continuity_count ==
            context_ptr->dpb_disp_order[queue_entry_ptr->show_existing_frame]) {
            context_ptr->disp_order_continuity_count++;
        } else {
            SVT_ERROR("disp_order_continuity_count Error2 POC:%i\n", (int32_t)queue_entry_ptr->poc);
            exit(0);
        }
    }

    //update total number of shown frames
    if (queue_entry_ptr->show_frame) {
        context_ptr->tot_shown_frames++;
    }
    if (queue_entry_ptr->has_show_existing) {
        context_ptr->tot_shown_frames++;
    }

    //implement the GOP here - Serial dec order
    if (queue_entry_ptr->frame_type == KEY_FRAME) {
        //reset the DPB on a Key frame
        for (i = 0; i < 8; i++) {
            context_ptr->dpb_dec_order[i]  = queue_entry_ptr->picture_number;
            context_ptr->dpb_disp_order[i] = queue_entry_ptr->poc;
        }
        SVT_LOG("%i  %i  %c ****KEY***** %i frames\n",
                (int32_t)queue_entry_ptr->picture_number,
                (int32_t)queue_entry_ptr->poc,
                showTab[queue_entry_ptr->show_frame],
                (int32_t)context_ptr->tot_shown_frames);
    } else {
        int32_t LASTrefIdx = queue_entry_ptr->av1_ref_signal.ref_dpb_index[0];
        int32_t BWDrefIdx  = queue_entry_ptr->av1_ref_signal.ref_dpb_index[4];

        if (queue_entry_ptr->frame_type == INTER_FRAME) {
            if (queue_entry_ptr->has_show_existing) {
                SVT_LOG("%i (%i  %i)    %i  (%i  %i)   %c  showEx: %i   %i frames\n",
                        (int32_t)queue_entry_ptr->picture_number,
                        (int32_t)context_ptr->dpb_dec_order[LASTrefIdx],
                        (int32_t)context_ptr->dpb_dec_order[BWDrefIdx],
                        (int32_t)queue_entry_ptr->poc,
                        (int32_t)context_ptr->dpb_disp_order[LASTrefIdx],
                        (int32_t)context_ptr->dpb_disp_order[BWDrefIdx],
                        showTab[queue_entry_ptr->show_frame],
                        (int32_t)context_ptr->dpb_disp_order[queue_entry_ptr->show_existing_frame],
                        (int32_t)context_ptr->tot_shown_frames);
            } else {
                SVT_LOG("%i (%i  %i)    %i  (%i  %i)   %c  %i frames\n",
                        (int32_t)queue_entry_ptr->picture_number,
                        (int32_t)context_ptr->dpb_dec_order[LASTrefIdx],
                        (int32_t)context_ptr->dpb_dec_order[BWDrefIdx],
                        (int32_t)queue_entry_ptr->poc,
                        (int32_t)context_ptr->dpb_disp_order[LASTrefIdx],
                        (int32_t)context_ptr->dpb_disp_order[BWDrefIdx],
                        showTab[queue_entry_ptr->show_frame],
                        (int32_t)context_ptr->tot_shown_frames);
            }

            if (queue_entry_ptr->ref_poc_list0 != context_ptr->dpb_disp_order[LASTrefIdx]) {
                SVT_LOG("L0 MISMATCH POC:%i\n", (int32_t)queue_entry_ptr->poc);
                exit(0);
            }

            for (int rr = 0; rr < 7; rr++) {
                uint8_t dpb_spot = queue_entry_ptr->av1_ref_signal.ref_dpb_index[rr];

                if (queue_entry_ptr->ref_poc_array[rr] != context_ptr->dpb_disp_order[dpb_spot]) {
                    SVT_LOG("REF_POC MISMATCH POC:%i  ref:%i\n", (int32_t)queue_entry_ptr->poc, rr);
                }
            }
        } else {
            if (queue_entry_ptr->has_show_existing) {
                SVT_LOG("%i  %i  %c   showEx: %i ----INTRA---- %i frames \n",
                        (int32_t)queue_entry_ptr->picture_number,
                        (int32_t)queue_entry_ptr->poc,
                        showTab[queue_entry_ptr->show_frame],
                        (int32_t)context_ptr->dpb_disp_order[queue_entry_ptr->show_existing_frame],
                        (int32_t)context_ptr->tot_shown_frames);
            } else {
                SVT_LOG("%i  %i  %c   ----INTRA---- %i frames\n",
                        (int32_t)queue_entry_ptr->picture_number,
                        (int32_t)queue_entry_ptr->poc,
                        (int32_t)showTab[queue_entry_ptr->show_frame],
                        (int32_t)context_ptr->tot_shown_frames);
            }
        }

        //Update the DPB
        for (i = 0; i < 8; i++) {
            if ((queue_entry_ptr->av1_ref_signal.refresh_frame_mask >> i) & 1) {
                context_ptr->dpb_dec_order[i]  = queue_entry_ptr->picture_number;
                context_ptr->dpb_disp_order[i] = queue_entry_ptr->poc;
            }
        }
    }
}
#endif

static void collect_frames_info(PacketizationContext* context_ptr, const EncodeContext* enc_ctx, int frames) {
    for (int i = 0; i < frames; i++) {
        PacketizationReorderEntry* queue_entry_ptr   = get_reorder_queue_entry(enc_ctx, i);
        EbBufferHeaderType*        output_stream_ptr = (EbBufferHeaderType*)
                                                    queue_entry_ptr->output_stream_wrapper_ptr->object_ptr;
#if DETAILED_FRAME_OUTPUT
        print_detailed_frame_info(context_ptr, queue_entry_ptr);
#else
        (void)context_ptr;

#endif
        // Calculate frame latency in milliseconds
        uint64_t finish_time_seconds   = 0;
        uint64_t finish_time_u_seconds = 0;
        svt_av1_get_time(&finish_time_seconds, &finish_time_u_seconds);

        output_stream_ptr->n_tick_count = (uint32_t)svt_av1_compute_overall_elapsed_time_ms(
            queue_entry_ptr->start_time_seconds,
            queue_entry_ptr->start_time_u_seconds,
            finish_time_seconds,
            finish_time_u_seconds);
        output_stream_ptr->p_app_private = (EbBufferHeaderType*)NULL;
        if (queue_entry_ptr->is_alt_ref) {
            output_stream_ptr->flags |= (uint32_t)EB_BUFFERFLAG_IS_ALT_REF;
        }
    }
}

#define TD_SIZE 2

// a tu start with a td, + 0 more not displable frame, + 1 display frame
static EbErrorType encode_tu(EncodeContext* enc_ctx, int frames, uint32_t total_bytes,
                             EbBufferHeaderType* output_stream_ptr) {
    total_bytes += TD_SIZE;
    if (total_bytes > output_stream_ptr->n_alloc_len) {
        uint8_t* pbuff;
        EB_MALLOC(pbuff, total_bytes);
        if (!pbuff) {
            SVT_ERROR("failed to allocate more memory in encode_tu");
            return EB_ErrorInsufficientResources;
        }
        svt_memcpy(pbuff,
                   output_stream_ptr->p_buffer,
                   output_stream_ptr->n_alloc_len > total_bytes ? total_bytes : output_stream_ptr->n_alloc_len);
        EB_FREE(output_stream_ptr->p_buffer);
        output_stream_ptr->p_buffer    = pbuff;
        output_stream_ptr->n_alloc_len = total_bytes;
    }
    uint8_t* dst = output_stream_ptr->p_buffer + total_bytes;
    // we use last frame's output_stream_ptr to hold entire tu, so we need copy backward.
    for (int i = frames - 1; i >= 0; i--) {
        PacketizationReorderEntry* queue_entry_ptr = get_reorder_queue_entry(enc_ctx, i);
        EbObjectWrapper*           wrapper         = queue_entry_ptr->output_stream_wrapper_ptr;
        EbBufferHeaderType*        src_stream_ptr  = (EbBufferHeaderType*)wrapper->object_ptr;
        uint32_t                   size            = src_stream_ptr->n_filled_len;
        dst -= size;
        memmove(dst, src_stream_ptr->p_buffer, size);
        // 1. The last frame is a displayable frame, others are undisplayed.
        // 2. We do not push alt ref frame since the overlay frame will carry the pts.
        // 3. Release alt ref stream buffer here for it will not be sent out
        if (i != frames - 1 && !queue_entry_ptr->is_alt_ref) {
            push_undisplayed_frame(enc_ctx, wrapper);
        } else if (queue_entry_ptr->is_alt_ref) {
            EB_FREE(src_stream_ptr->p_buffer);
            svt_release_object(wrapper);
        }
    }
    if (frames > 1) {
        sort_undisplayed_frame(enc_ctx);
    }
    dst -= TD_SIZE;
    svt_aom_encode_td_av1(dst);
    output_stream_ptr->n_filled_len = total_bytes;
    output_stream_ptr->flags |= EB_BUFFERFLAG_HAS_TD;
    return EB_ErrorNone;
}

static EbErrorType copy_data_from_bitstream(EncodeContext* enc_ctx, Bitstream* bitstream_ptr,
                                            EbBufferHeaderType* output_stream_ptr) {
    EbErrorType return_error = EB_ErrorNone;
    int         size         = svt_aom_bitstream_get_bytes_count(bitstream_ptr);

    CHECK_REPORT_ERROR((size + output_stream_ptr->n_filled_len < output_stream_ptr->n_alloc_len),
                       enc_ctx->app_callback_ptr,
                       EB_ENC_EC_ERROR2);

    void* dest = output_stream_ptr->p_buffer + output_stream_ptr->n_filled_len;
    svt_aom_bitstream_copy(bitstream_ptr, dest, size);
    output_stream_ptr->n_filled_len += size;

    return return_error;
}

static void encode_show_existing(EncodeContext* enc_ctx, PacketizationReorderEntry* queue_entry_ptr,
                                 EbBufferHeaderType* output_stream_ptr) {
    uint8_t* dst = output_stream_ptr->p_buffer;

    svt_aom_encode_td_av1(dst);
    output_stream_ptr->n_filled_len = TD_SIZE;

    copy_data_from_bitstream(enc_ctx, queue_entry_ptr->bitstream_ptr, output_stream_ptr);

    output_stream_ptr->flags |= (EB_BUFFERFLAG_SHOW_EXT | EB_BUFFERFLAG_HAS_TD);
}

static void release_frames(EncodeContext* enc_ctx, int frames) {
    for (int i = 0; i < frames; i++) {
        PacketizationReorderEntry* queue_entry_ptr = get_reorder_queue_entry(enc_ctx, i);
        // Reset the Reorder Queue Entry
        queue_entry_ptr->picture_number += enc_ctx->packetization_reorder_queue_size;
        queue_entry_ptr->output_stream_wrapper_ptr = (EbObjectWrapper*)NULL;
    }
    enc_ctx->packetization_reorder_queue_head_index = get_reorder_queue_pos(enc_ctx, frames);
}

// Release the pd_dpb and ref_pic_list at the end of the sequence
void release_references_eos(SequenceControlSet* scs) {
    EncodeContext* enc_ctx = scs->enc_ctx;
    // At the end of the sequence release all the refs (needed for MacOS CI tests)
    svt_block_on_mutex(enc_ctx->pd_dpb_mutex);
    for (uint8_t i = 0; i < REF_FRAMES; i++) {
        // Get the current entry at that spot in the DPB
        PaReferenceEntry* input_entry = enc_ctx->pd_dpb[i];

        // If DPB entry is occupied, release the current entry
        if (input_entry->is_valid) {
            // Release the entry at that DPB spot
            // Release the nominal live_count value
            svt_release_object(input_entry->input_object_ptr);

            if (input_entry->y8b_wrapper) {
                //y8b needs to get decremented at the same time of pa ref
                svt_release_object(input_entry->y8b_wrapper);
            }

            input_entry->input_object_ptr = (EbObjectWrapper*)NULL;
            input_entry->is_valid         = false;
        }
    }
    svt_release_mutex(enc_ctx->pd_dpb_mutex);

    svt_block_on_mutex(enc_ctx->ref_pic_list_mutex);
    ReferenceQueueEntry* ref_entry = NULL;
    for (uint32_t i = 0; i < enc_ctx->ref_pic_list_length; i++) {
        ref_entry = enc_ctx->ref_pic_list[i];
        if (ref_entry->is_valid && ref_entry->reference_object_ptr) {
            // Remove the entry & release the reference if there are no remaining references
            // Release the nominal live_count value
            svt_release_object(ref_entry->reference_object_ptr);
            ref_entry->reference_object_ptr  = (EbObjectWrapper*)NULL;
            ref_entry->reference_available   = false;
            ref_entry->is_ref                = false;
            ref_entry->is_valid              = false;
            ref_entry->frame_context_updated = false;
            ref_entry->feedback_arrived      = false;
            svt_post_semaphore(scs->ref_buffer_available_semaphore);
        }
    }
    svt_release_mutex(enc_ctx->ref_pic_list_mutex);
}

inline static void clear_eos_flag(EbBufferHeaderType* output_stream_ptr) {
    output_stream_ptr->flags &= ~EB_BUFFERFLAG_EOS;
}

inline static void set_eos_flag(EbBufferHeaderType* output_stream_ptr) {
    output_stream_ptr->flags |= EB_BUFFERFLAG_EOS;
}

/* Wrapper function to capture the return of EB_MALLOC */
static inline EbErrorType malloc_p_buffer(EbBufferHeaderType* output_stream_ptr) {
    EB_MALLOC(output_stream_ptr->p_buffer, output_stream_ptr->n_alloc_len);
    return EB_ErrorNone;
}

void update_firstpass_stats(PictureParentControlSet* pcs, const int frame_number, const double ts_duration,
                            StatStruct* stat_struct);
void svt_av1_end_first_pass(PictureParentControlSet* pcs);

/* Realloc when bitstream pointer size is not enough to write data of size sz */
static EbErrorType realloc_output_bitstream(Bitstream* bitstream_ptr, uint32_t sz) {
    if (bitstream_ptr && sz > 0) {
        OutputBitstreamUnit* output_bitstream_ptr = bitstream_ptr->output_bitstream_ptr;
        if (output_bitstream_ptr) {
            output_bitstream_ptr->size = sz;
            EB_REALLOC_ARRAY(output_bitstream_ptr->buffer_begin_av1, output_bitstream_ptr->size);
            output_bitstream_ptr->buffer_av1 = output_bitstream_ptr->buffer_begin_av1;
        }
    }
    return EB_ErrorNone;
}

void* svt_aom_packetization_kernel(void* input_ptr) {
    // Context
    EbThreadContext*      thread_ctx  = (EbThreadContext*)input_ptr;
    PacketizationContext* context_ptr = (PacketizationContext*)thread_ctx->priv;

    // Input
    EbObjectWrapper* entropy_coding_results_wrapper_ptr;

    // Output
    EbObjectWrapper* rate_control_tasks_wrapper_ptr;
    EbObjectWrapper* picture_manager_results_wrapper_ptr;

    context_ptr->tot_shown_frames            = 0;
    context_ptr->disp_order_continuity_count = 0;

    for (;;) {
        // Get EntropyCoding Results
        EB_GET_FULL_OBJECT(context_ptr->entropy_coding_input_fifo_ptr, &entropy_coding_results_wrapper_ptr);

        EntropyCodingResults* entropy_coding_results_ptr = (EntropyCodingResults*)
                                                               entropy_coding_results_wrapper_ptr->object_ptr;
        PictureControlSet*       pcs      = (PictureControlSet*)entropy_coding_results_ptr->pcs_wrapper->object_ptr;
        SequenceControlSet*      scs      = pcs->scs;
        EncodeContext*           enc_ctx  = scs->enc_ctx;
        FrameHeader*             frm_hdr  = &pcs->ppcs->frm_hdr;
        Av1Common* const         cm       = pcs->ppcs->av1_cm;
        uint16_t                 tile_cnt = cm->tiles_info.tile_rows * cm->tiles_info.tile_cols;
        PictureParentControlSet* ppcs     = (PictureParentControlSet*)pcs->ppcs;

        if (ppcs->superres_total_recode_loop > 0 && ppcs->superres_recode_loop < ppcs->superres_total_recode_loop) {
            // Reset the Bitstream before writing to it
            svt_aom_bitstream_reset(pcs->bitstream_ptr);
            svt_aom_write_frame_header_av1(pcs->bitstream_ptr, scs, pcs, 0);
            int64_t bits = (int64_t)svt_aom_bitstream_get_bytes_count(pcs->bitstream_ptr) << 3;
            int64_t rate = bits << 5; // To match scale.
            svt_aom_bitstream_reset(pcs->bitstream_ptr);
            int64_t    sse       = ppcs->luma_sse;
            EbBitDepth bit_depth = pcs->hbd_md ? EB_TEN_BIT : EB_EIGHT_BIT;
            uint8_t    qindex    = ppcs->frm_hdr.quantization_params.base_q_idx;
            int32_t    rdmult    = svt_aom_compute_rd_mult(pcs, qindex, qindex, bit_depth);

            double rdcost = RDCOST_DBL_WITH_NATIVE_BD_DIST(rdmult, rate, sse, scs->static_config.encoder_bit_depth);

#if DEBUG_SUPERRES_RECODE
            printf(
                "\n####### %s - frame %d, loop %d/%d, denom %d, rate %I64d, sse %I64d, rdcost "
                "%.2f, qindex %d, rdmult %d\n",
                __FUNCTION__,
                (int)ppcs->picture_number,
                ppcs->superres_recode_loop,
                ppcs->superres_total_recode_loop,
                ppcs->superres_denom,
                rate,
                sse,
                rdcost,
                qindex,
                rdmult);
#endif

            assert(ppcs->superres_total_recode_loop <= SCALE_NUMERATOR + 1);
            ppcs->superres_rdcost[ppcs->superres_recode_loop] = rdcost;
            ++ppcs->superres_recode_loop;

            if (ppcs->superres_recode_loop <= ppcs->superres_total_recode_loop) {
                bool do_recode = false;
                if (ppcs->superres_recode_loop == ppcs->superres_total_recode_loop) {
                    // compare rdcosts to determine whether need to recode again
                    // rdcost is the smaller the better
                    int best_index = 0;
                    for (int i = 1; i < ppcs->superres_total_recode_loop; ++i) {
                        double rdcost1 = ppcs->superres_rdcost[best_index];
                        double rdcost2 = ppcs->superres_rdcost[i];
                        if (rdcost2 < rdcost1) {
                            best_index = i;
                        }
                    }

                    if (best_index != ppcs->superres_total_recode_loop - 1) {
                        do_recode            = true;
                        ppcs->superres_denom = ppcs->superres_denom_array[best_index];
#if DEBUG_SUPERRES_RECODE
                        printf("\n####### %s - frame %d, extra loop, pick denom %d\n",
                               __FUNCTION__,
                               (int)ppcs->picture_number,
                               ppcs->superres_denom);
#endif
                    }
                } else {
                    do_recode = true;
                }

                if (do_recode) {
                    svt_aom_init_resize_picture(scs, ppcs);

                    // reset gm based on super-res on/off
                    bool super_res_off = ppcs->frame_superres_enabled == false &&
                        scs->static_config.resize_mode == RESIZE_NONE;
                    svt_aom_set_gm_controls(ppcs, svt_aom_derive_gm_level(ppcs, super_res_off));
                    // Initialize Segments as picture decision process
                    ppcs->me_segments_completion_count = 0;
                    ppcs->me_processed_b64_count       = 0;

                    if (ppcs->ref_pic_wrapper != NULL) {
                        // update mi_rows and mi_cols for the reference pic wrapper (used in mfmv
                        // for other pictures)
                        EbReferenceObject* ref_object = ppcs->ref_pic_wrapper->object_ptr;
                        svt_reference_object_reset(ref_object, scs);
                    }
#if DEBUG_SUPERRES_RECODE
                    printf("\n%s - send superres recode task to open loop ME. Frame %d, denom %d\n",
                           __FUNCTION__,
                           (int)ppcs->picture_number,
                           ppcs->superres_denom);
#endif

                    for (uint32_t segment_index = 0; segment_index < ppcs->me_segments_total_count; ++segment_index) {
                        // Get Empty Results Object
                        EbObjectWrapper* out_results_wrapper;
                        svt_get_empty_object(context_ptr->picture_decision_results_output_fifo_ptr,
                                             &out_results_wrapper);

                        PictureDecisionResults* out_results = (PictureDecisionResults*)out_results_wrapper->object_ptr;
                        out_results->pcs_wrapper            = ppcs->p_pcs_wrapper_ptr;
                        out_results->segment_index          = segment_index;
                        out_results->task_type              = TASK_SUPERRES_RE_ME;
                        // Post the Full Results Object
                        svt_post_full_object(out_results_wrapper);
                    }

                    // Release the Entropy Coding Result
                    svt_release_object(entropy_coding_results_wrapper_ptr);
                    continue;
                }
            }
        }

        if (ppcs->superres_total_recode_loop > 0) {
            // Release pa_ref_objs
            // Delayed call from Initial Rate Control process / Source Based Operations process
            if (ppcs->tpl_ctrls.enable) {
                if (ppcs->temporal_layer_index == 0) {
                    for (uint32_t i = 0; i < ppcs->tpl_group_size; i++) {
                        if (svt_aom_is_incomp_mg_frame(ppcs->tpl_group[i])) {
                            if (ppcs->tpl_group[i]->ext_mg_id == ppcs->ext_mg_id + 1) {
                                svt_aom_release_pa_reference_objects(scs, ppcs->tpl_group[i]);
                            }
                        } else {
                            if (ppcs->tpl_group[i]->ext_mg_id == ppcs->ext_mg_id) {
                                svt_aom_release_pa_reference_objects(scs, ppcs->tpl_group[i]);
                            }
                        }
                    }
                }
            } else {
                svt_aom_release_pa_reference_objects(scs, ppcs);
            }

            // Delayed call from Rate Control process for multiple coding loop frames
            if (scs->static_config.rate_control_mode) {
                svt_aom_update_rc_counts(ppcs);
            }

            // Release pa me ptr. For non-superres-recode, it's released in svt_aom_mode_decision_kernel
            assert(pcs->ppcs->me_data_wrapper != NULL);
            assert(pcs->ppcs->pa_me_data != NULL);
            svt_release_object(pcs->ppcs->me_data_wrapper);
            pcs->ppcs->me_data_wrapper = NULL;
            pcs->ppcs->pa_me_data      = NULL;

            // Delayed call from Rest process
            {
                if (pcs->ppcs->compute_ssim) {
                    // memory is freed in the svt_aom_ssim_calculations call
                    svt_aom_ssim_calculations(pcs, scs, true);
                } else {
                    // free memory used by psnr_calculations
                    free_temporal_filtering_buffer(pcs, scs);
                }

                if (scs->static_config.recon_enabled) {
                    svt_aom_recon_output(pcs, scs);
                }

                if (ppcs->is_ref) {
                    EbObjectWrapper*     picture_demux_results_wrapper_ptr;
                    PictureDemuxResults* picture_demux_results_rtr;

                    // Get Empty PicMgr Results
                    svt_get_empty_object(context_ptr->picture_demux_fifo_ptr, &picture_demux_results_wrapper_ptr);

                    picture_demux_results_rtr = (PictureDemuxResults*)picture_demux_results_wrapper_ptr->object_ptr;
                    picture_demux_results_rtr->ref_pic_wrapper = ppcs->ref_pic_wrapper;
                    picture_demux_results_rtr->scs             = ppcs->scs;
                    picture_demux_results_rtr->picture_number  = ppcs->picture_number;
                    picture_demux_results_rtr->picture_type    = EB_PIC_REFERENCE;

                    // Post Reference Picture
                    svt_post_full_object(picture_demux_results_wrapper_ptr);
                }
            }

            // Delayed call from Entropy Coding process
            {
                // Release the reference Pictures from both lists
                for (REF_FRAME_MINUS1 ref = LAST; ref < ALT + 1; ref++) {
                    const uint8_t list_idx = get_list_idx(ref + 1);
                    const uint8_t ref_idx  = get_ref_frame_idx(ref + 1);
                    if (pcs->ref_pic_ptr_array[list_idx][ref_idx] != NULL) {
                        svt_release_object(pcs->ref_pic_ptr_array[list_idx][ref_idx]);
                    }
                }

                //free palette data
                if (pcs->tile_tok[0][0]) {
                    EB_FREE_ARRAY(pcs->tile_tok[0][0]);
                }
            }
        } else if (!(pcs->ppcs->compute_psnr || pcs->ppcs->compute_ssim)) {
            free_temporal_filtering_buffer(pcs, scs);
        }
        //****************************************************
        // Input Entropy Results into Reordering Queue
        //****************************************************
        // get a new entry spot
        int32_t queue_entry_index = pcs->ppcs->decode_order % enc_ctx->packetization_reorder_queue_size;
        PacketizationReorderEntry* queue_entry_ptr = enc_ctx->packetization_reorder_queue[queue_entry_index];
        queue_entry_ptr->start_time_seconds        = pcs->ppcs->start_time_seconds;
        queue_entry_ptr->start_time_u_seconds      = pcs->ppcs->start_time_u_seconds;
        queue_entry_ptr->is_alt_ref                = pcs->ppcs->is_alt_ref;
        svt_get_empty_object(scs->enc_ctx->stream_output_fifo_ptr, &pcs->ppcs->output_stream_wrapper_ptr);
        EbObjectWrapper*    output_stream_wrapper_ptr = pcs->ppcs->output_stream_wrapper_ptr;
        EbBufferHeaderType* output_stream_ptr         = (EbBufferHeaderType*)output_stream_wrapper_ptr->object_ptr;

        if (frm_hdr->frame_type == KEY_FRAME) {
            if (scs->static_config.mastering_display.max_luma) {
                svt_add_metadata(pcs->ppcs->input_ptr,
                                 EB_AV1_METADATA_TYPE_HDR_MDCV,
                                 (const uint8_t*)&scs->static_config.mastering_display,
                                 sizeof(scs->static_config.mastering_display));
            }
            if (scs->static_config.content_light_level.max_cll) {
                svt_add_metadata(pcs->ppcs->input_ptr,
                                 EB_AV1_METADATA_TYPE_HDR_CLL,
                                 (const uint8_t*)&scs->static_config.content_light_level,
                                 sizeof(scs->static_config.content_light_level));
            }
        }

        output_stream_ptr->flags        = 0;
        output_stream_ptr->n_filled_len = 0;
        output_stream_ptr->pts          = pcs->ppcs->input_ptr->pts;
        // we output one temporal unit a time, so dts alwasy equals to pts.
        output_stream_ptr->dts = output_stream_ptr->pts;
        if (pcs->ppcs->idr_flag) {
            output_stream_ptr->pic_type = EB_AV1_KEY_PICTURE;
        } else if (pcs->slice_type == I_SLICE) {
            output_stream_ptr->pic_type = EB_AV1_INTRA_ONLY_PICTURE;
        } else if (pcs->ppcs->is_ref) {
            output_stream_ptr->pic_type = EB_AV1_INTER_PICTURE;
        } else {
            output_stream_ptr->pic_type = EB_AV1_NON_REF_PICTURE;
        }
        output_stream_ptr->p_app_private        = pcs->ppcs->input_ptr->p_app_private;
        output_stream_ptr->temporal_layer_index = pcs->ppcs->temporal_layer_index;
        output_stream_ptr->qp                   = pcs->ppcs->picture_qp;
        output_stream_ptr->avg_qp               = pcs->ppcs->avg_qp;
        if (pcs->ppcs->compute_psnr) {
            output_stream_ptr->luma_sse = pcs->ppcs->luma_sse;
            output_stream_ptr->cr_sse   = pcs->ppcs->cr_sse;
            output_stream_ptr->cb_sse   = pcs->ppcs->cb_sse;
        } else {
            output_stream_ptr->luma_sse = 0;
            output_stream_ptr->cr_sse   = 0;
            output_stream_ptr->cb_sse   = 0;
        }
        if (pcs->ppcs->compute_ssim) {
            output_stream_ptr->luma_ssim = pcs->ppcs->luma_ssim;
            output_stream_ptr->cr_ssim   = pcs->ppcs->cr_ssim;
            output_stream_ptr->cb_ssim   = pcs->ppcs->cb_ssim;
        } else {
            output_stream_ptr->luma_ssim = 0;
            output_stream_ptr->cr_ssim   = 0;
            output_stream_ptr->cb_ssim   = 0;
        }

        // Get Empty Rate Control Input Tasks
        svt_get_empty_object(context_ptr->rate_control_tasks_output_fifo_ptr, &rate_control_tasks_wrapper_ptr);
        RateControlTasks* rc_tasks = (RateControlTasks*)rate_control_tasks_wrapper_ptr->object_ptr;
        rc_tasks->pcs_wrapper      = pcs->ppcs_wrapper;
        rc_tasks->task_type        = RC_PACKETIZATION_FEEDBACK_RESULT;
        if (scs->enable_dec_order || (pcs->ppcs->is_ref == true && pcs->ppcs->ref_pic_wrapper)) {
            if (pcs->ppcs->is_ref == true &&
                // Force each frame to update their data so future frames can use it,
                // even if the current frame did not use it.  This enables REF frames to
                // have the feature off, while NREF frames can have it on.  Used for
                // multi-threading.
                pcs->ppcs->ref_pic_wrapper) {
                for (uint16_t tile_idx = 0; tile_idx < tile_cnt; tile_idx++) {
                    svt_av1_reset_cdf_symbol_counters(pcs->ec_info[tile_idx]->ec->fc);
                    ((EbReferenceObject*)pcs->ppcs->ref_pic_wrapper->object_ptr)->frame_context =
                        (*pcs->ec_info[tile_idx]->ec->fc);
                }
            }
            // Get Empty Results Object
            svt_get_empty_object(context_ptr->picture_demux_fifo_ptr, &picture_manager_results_wrapper_ptr);

            PictureDemuxResults* picture_manager_results_ptr = (PictureDemuxResults*)
                                                                   picture_manager_results_wrapper_ptr->object_ptr;
            picture_manager_results_ptr->picture_number = pcs->picture_number;
            picture_manager_results_ptr->picture_type   = EB_PIC_FEEDBACK;
            picture_manager_results_ptr->decode_order   = pcs->ppcs->decode_order;
            picture_manager_results_ptr->scs            = pcs->scs;
        }
        // Reset the Bitstream before writing to it
        svt_aom_bitstream_reset(pcs->bitstream_ptr);

        size_t metadata_sz = 0;

        // Code the SPS
        if (frm_hdr->frame_type == KEY_FRAME) {
            svt_aom_encode_sps_av1(pcs->bitstream_ptr, scs);
            // Add CLL and MDCV meta when frame is keyframe and SPS is written
            svt_aom_write_metadata_av1(
                pcs->bitstream_ptr, pcs->ppcs->input_ptr->metadata, EB_AV1_METADATA_TYPE_HDR_CLL);
            svt_aom_write_metadata_av1(
                pcs->bitstream_ptr, pcs->ppcs->input_ptr->metadata, EB_AV1_METADATA_TYPE_HDR_MDCV);
        }

        if (frm_hdr->show_frame) {
            // Add HDR10+ dynamic metadata when show frame flag is enabled
            svt_aom_write_metadata_av1(
                pcs->bitstream_ptr, pcs->ppcs->input_ptr->metadata, EB_AV1_METADATA_TYPE_ITUT_T35);
            svt_metadata_array_free(&pcs->ppcs->input_ptr->metadata);
        } else {
            // Copy metadata pointer to the queue entry related to current frame number
            uint64_t current_picture_number = pcs->picture_number;
            // TODO: Shouldn't entry be indexed based on decode order, not picture number
            PacketizationReorderEntry* temp_entry =
                enc_ctx
                    ->packetization_reorder_queue[current_picture_number % enc_ctx->packetization_reorder_queue_size];
            temp_entry->metadata           = pcs->ppcs->input_ptr->metadata;
            pcs->ppcs->input_ptr->metadata = NULL;
            metadata_sz                    = svt_metadata_size(temp_entry->metadata, EB_AV1_METADATA_TYPE_ITUT_T35);
        }

        svt_aom_write_frame_header_av1(pcs->bitstream_ptr, scs, pcs, 0);

        output_stream_ptr->n_alloc_len = (uint32_t)(svt_aom_bitstream_get_bytes_count(pcs->bitstream_ptr) + TD_SIZE +
                                                    metadata_sz);
        malloc_p_buffer(output_stream_ptr);

        assert(output_stream_ptr->p_buffer != NULL && "bit-stream memory allocation failure");

        copy_data_from_bitstream(enc_ctx, pcs->bitstream_ptr, output_stream_ptr);

        if (pcs->ppcs->has_show_existing) {
            uint64_t next_picture_number = pcs->picture_number + 1;
            // TODO: Shouldn't entry be indexed based on decode order, not picture number
            PacketizationReorderEntry* temp_entry =
                enc_ctx->packetization_reorder_queue[next_picture_number % enc_ctx->packetization_reorder_queue_size];
            // Check if the temporal entry has metadata
            if (temp_entry->metadata) {
                // Get bitstream from queue entry
                Bitstream* bitstream_ptr       = queue_entry_ptr->bitstream_ptr;
                size_t     current_metadata_sz = svt_metadata_size(temp_entry->metadata, EB_AV1_METADATA_TYPE_ITUT_T35);
                // 16 bytes for frame header
                realloc_output_bitstream(bitstream_ptr, (uint32_t)(current_metadata_sz + 16));
            }
            // Reset the Bitstream before writing to it
            svt_aom_bitstream_reset(queue_entry_ptr->bitstream_ptr);
            svt_aom_write_metadata_av1(
                queue_entry_ptr->bitstream_ptr, temp_entry->metadata, EB_AV1_METADATA_TYPE_ITUT_T35);
            svt_aom_write_frame_header_av1(queue_entry_ptr->bitstream_ptr, scs, pcs, 1);
            svt_metadata_array_free(&temp_entry->metadata);
        }

        // Send the number of bytes per frame to RC
        pcs->ppcs->total_num_bits = output_stream_ptr->n_filled_len << 3;
        if (scs->passes == 2 && scs->static_config.pass == ENC_FIRST_PASS) {
            StatStruct stat_struct;
            stat_struct.poc = pcs->picture_number;
            if (scs->first_pass_downsample) {
                stat_struct.total_num_bits = pcs->ppcs->total_num_bits * DS_SC_FACT / 10;
            } else {
                stat_struct.total_num_bits = pcs->ppcs->total_num_bits;
            }
            stat_struct.qindex       = frm_hdr->quantization_params.base_q_idx;
            stat_struct.worst_qindex = quantizer_to_qindex[(uint8_t)scs->static_config.qp];
            if (svt_aom_is_pic_skipped(pcs->ppcs)) {
                stat_struct.total_num_bits = 0;
            }
            stat_struct.temporal_layer_index = pcs->temporal_layer_index;
            update_firstpass_stats(pcs->ppcs, (const int)pcs->picture_number, pcs->ppcs->ts_duration, &stat_struct);
            if (ppcs->end_of_sequence_flag) {
                svt_av1_end_first_pass(ppcs);
            }
        }
        queue_entry_ptr->total_num_bits = pcs->ppcs->total_num_bits;
        queue_entry_ptr->frame_type     = frm_hdr->frame_type;
        queue_entry_ptr->poc            = pcs->picture_number;
        svt_memcpy(&queue_entry_ptr->av1_ref_signal, &pcs->ppcs->av1_ref_signal, sizeof(Av1RpsNode));

        queue_entry_ptr->slice_type = pcs->slice_type;
#if DETAILED_FRAME_OUTPUT
        queue_entry_ptr->ref_poc_list0 = pcs->ppcs->ref_pic_poc_array[REF_LIST_0][0];
        queue_entry_ptr->ref_poc_list1 = pcs->ppcs->ref_pic_poc_array[REF_LIST_1][0];
        svt_memcpy(queue_entry_ptr->ref_poc_array, pcs->ppcs->av1_ref_signal.ref_poc_array, 7 * sizeof(uint64_t));
#endif
        queue_entry_ptr->show_frame          = frm_hdr->show_frame;
        queue_entry_ptr->has_show_existing   = pcs->ppcs->has_show_existing;
        queue_entry_ptr->show_existing_frame = frm_hdr->show_existing_frame;

        //Store the output buffer in the Queue
        queue_entry_ptr->output_stream_wrapper_ptr = output_stream_wrapper_ptr;

        // Note: last chance here to add more output meta data for an encoded picture -->

        if (scs->speed_control_flag) {
            // update speed control variables
            svt_block_on_mutex(enc_ctx->sc_buffer_mutex);
            enc_ctx->sc_frame_out++;
            svt_release_mutex(enc_ctx->sc_buffer_mutex);
        }

        if (scs->enable_dec_order || (pcs->ppcs->is_ref == true && pcs->ppcs->ref_pic_wrapper)) {
            // Post the Full Results Object
            svt_post_full_object(picture_manager_results_wrapper_ptr);
        }
        // Post Rate Control Task. Be done after postig to PM as RC might release ppcs
        svt_post_full_object(rate_control_tasks_wrapper_ptr);
        if (pcs->ppcs->frm_hdr.allow_intrabc) {
            svt_av1_hash_table_destroy(&pcs->hash_table);
        }
        svt_release_object(pcs->ppcs->enc_dec_ptr->enc_dec_wrapper); // Child
        // Release the Parent PCS then the Child PCS
        assert(entropy_coding_results_ptr->pcs_wrapper->live_count == 1);
        svt_release_object(entropy_coding_results_ptr->pcs_wrapper); // Child
        // Release the Entropy Coding Result
        svt_release_object(entropy_coding_results_wrapper_ptr);

        //****************************************************
        // Process the head of the queue
        //****************************************************
        // Look at head of queue and see if we got a td
        uint32_t frames, total_bytes;
        svt_block_on_mutex(enc_ctx->total_number_of_shown_frames_mutex);
        bool eos = false;
        while ((frames = count_frames_in_next_tu(enc_ctx, &total_bytes))) {
            collect_frames_info(context_ptr, enc_ctx, frames);
            // last frame in a termporal unit is a displable frame. only the last frame has pts.
            // so we collect all bitstream to last frames' output buffer.
            queue_entry_ptr           = get_reorder_queue_entry(enc_ctx, frames - 1);
            output_stream_wrapper_ptr = queue_entry_ptr->output_stream_wrapper_ptr;
            output_stream_ptr         = (EbBufferHeaderType*)output_stream_wrapper_ptr->object_ptr;
            eos                       = output_stream_ptr->flags & EB_BUFFERFLAG_EOS;
            encode_tu(enc_ctx, frames, total_bytes, output_stream_ptr);

            if (eos && queue_entry_ptr->has_show_existing) {
                clear_eos_flag(output_stream_ptr);
            }

            svt_post_full_object(output_stream_wrapper_ptr);
            if (queue_entry_ptr->has_show_existing) {
                EbObjectWrapper* existed = pop_undisplayed_frame(enc_ctx);
                if (existed) {
                    EbBufferHeaderType* existed_output_stream_ptr = (EbBufferHeaderType*)existed->object_ptr;
                    encode_show_existing(enc_ctx, queue_entry_ptr, existed_output_stream_ptr);
                    if (eos) {
                        set_eos_flag(existed_output_stream_ptr);
                    }
                    svt_post_full_object(existed);
                }
            }

            if (queue_entry_ptr->show_frame) {
                enc_ctx->total_number_of_shown_frames++;
            }
            if (queue_entry_ptr->has_show_existing) {
                enc_ctx->total_number_of_shown_frames++;
            }
            eos = (enc_ctx->total_number_of_shown_frames == enc_ctx->terminating_picture_number + 1) ? 1 : 0;
            release_frames(enc_ctx, frames);
        }
        if (eos) {
            EbObjectWrapper* tmp_out_str_wrp;
            svt_get_empty_object(scs->enc_ctx->stream_output_fifo_ptr, &tmp_out_str_wrp);
            EbBufferHeaderType* tmp_out_str = (EbBufferHeaderType*)tmp_out_str_wrp->object_ptr;

            tmp_out_str->flags        = EB_BUFFERFLAG_EOS;
            tmp_out_str->n_filled_len = 0;

            svt_post_full_object(tmp_out_str_wrp);
            release_references_eos(scs);
        }
        svt_release_mutex(enc_ctx->total_number_of_shown_frames_mutex);
    }
    return NULL;
}
