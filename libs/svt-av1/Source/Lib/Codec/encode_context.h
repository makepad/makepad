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

#ifndef EbEncodeContext_h
#define EbEncodeContext_h

#include <stdio.h>

#include "definitions.h"
#include "EbSvtAv1Enc.h"
#include "pd_reorder_queue.h"
#include "pd_queue.h"
#include "pic_manager_queue.h"
#include "packetization_reorder_queue.h"
#include "initial_rc_reorder_queue.h"
#include "cabac_context_model.h"
#include "md_rate_estimation.h"
#include "pred_structure.h"
#include "object.h"
#include "encoder.h"
#include "firstpass.h"
#include "rc_process.h"

// *Note - the queues are small for testing purposes.  They should be increased when they are done.
#define PRE_ASSIGNMENT_MAX_DEPTH 128 // should be large enough to hold an entire prediction period

#define TPL_PADX 32
#define TPL_PADY 32
// RC Groups: They should be a power of 2, so we can replace % by &.
// Instead of using x % y, we use x && (y-1)
#define PARALLEL_GOP_MAX_NUMBER 256

typedef struct FirstPassStatsOut {
    FIRSTPASS_STATS* stat;
    size_t           size;
    size_t           capability;
} FirstPassStatsOut;

typedef struct RateControlIntervalParamContext {
    EbDctor  dctor;
    uint64_t first_poc;
    // Projected total bits available for a key frame group of frames
    int64_t kf_group_bits;
    // Error score of frames still to be coded in kf group
    int64_t kf_group_error_left;
    uint8_t end_of_seq_seen;
    int32_t processed_frame_number;
    int32_t size;
    int64_t vbr_bits_off_target;
    int64_t vbr_bits_off_target_fast;
    int     rolling_target_bits;
    int     rolling_actual_bits;
    int     rate_error_estimate;
    int64_t total_actual_bits;
    int64_t total_target_bits;
    int     extend_minq;
    int     extend_maxq;
    int     extend_minq_fast;
} RateControlIntervalParamContext;

typedef struct EncodeContext {
    EbDctor dctor;
    // Callback Functions
    EbCallback* app_callback_ptr;

    EbHandle total_number_of_recon_frame_mutex;
    uint64_t total_number_of_recon_frames;
    EbHandle total_number_of_shown_frames_mutex;
    uint64_t total_number_of_shown_frames;

    // Overlay input picture fifo
    EbFifo* overlay_input_picture_pool_fifo_ptr;
    // Output Buffer Fifos
    EbFifo* stream_output_fifo_ptr;
    EbFifo* recon_output_fifo_ptr;

    // Picture Buffer Fifos
    EbFifo* reference_picture_pool_fifo_ptr;
    EbFifo* pa_reference_picture_pool_fifo_ptr;
    EbFifo* tpl_reference_picture_pool_fifo_ptr;

    // Picture Decision Reorder Queue
    PictureDecisionReorderEntry** picture_decision_reorder_queue;
    // max size of reorder queue
    uint32_t picture_decision_reorder_queue_size;
    uint32_t picture_decision_reorder_queue_head_index;
    //hold undisplayed frame for show existing frame. It's ordered with pts Descend.
    EbObjectWrapper* picture_decision_undisplayed_queue[UNDISP_QUEUE_SIZE];
    uint32_t         picture_decision_undisplayed_queue_count;
    // Picture Manager Pre-Assignment Buffer
    uint32_t          pre_assignment_buffer_intra_count;
    uint32_t          pre_assignment_buffer_idr_count;
    uint32_t          pre_assignment_buffer_eos_flag;
    uint64_t          decode_base_number;
    EbObjectWrapper** pre_assignment_buffer;
    uint32_t          pre_assignment_buffer_count;

    // Picture Decision decoded picture buffer - used to track PA refs
    PaReferenceEntry** pd_dpb;
    // Picture decision and Packetization process both access pd_dpb.
    // Mutex added for protection
    EbHandle pd_dpb_mutex;

    // Picture Manager List of input pics and total list size
    InputQueueEntry** pic_mgr_input_pic_list;
    // max size of iput pic list
    uint32_t pic_mgr_input_pic_list_size;
    // Picture Manager List
    ReferenceQueueEntry** ref_pic_list;
    uint32_t              ref_pic_list_length;
    // Picture manager and Packetization process both access ref_pic_list.
    // Mutex added for protection
    EbHandle ref_pic_list_mutex;

    // Packetization Reorder Queue
    PacketizationReorderEntry** packetization_reorder_queue;
    uint32_t                    packetization_reorder_queue_size;
    uint32_t                    packetization_reorder_queue_head_index;

    // GOP Counters
    uint32_t intra_period_position; // Current position in intra period
    uint32_t pred_struct_position; // Current position within a prediction structure
    uint32_t elapsed_non_idr_count;
    uint32_t elapsed_non_cra_count;
    bool     initial_picture;
    uint64_t last_idr_picture; // the most recently occurred IDR picture (in decode order)

    // Sequence Termination Flags
    uint64_t terminating_picture_number;
    bool     terminating_sequence_flag_received;

    // Prediction Structure
    PredictionStructureGroup* prediction_structure_group_ptr;

    // Speed Control
    int64_t  sc_buffer;
    int64_t  sc_frame_in;
    int64_t  sc_frame_out;
    EbHandle sc_buffer_mutex;
    EncMode  enc_mode;

    // Dynamic GOP
    uint32_t         previous_mini_gop_hierarchical_levels;
    uint64_t         mini_gop_cnt_per_gop;
    EbObjectWrapper* previous_picture_control_set_wrapper_ptr;
    uint64_t         picture_number_alt; // The picture number overlay includes all the overlay frames

    EbHandle stat_file_mutex;

    bool                 is_mini_gop_changed;
    uint64_t             poc_map_idx[MAX_TPL_LA_SW];
    EbPictureBufferDesc* mc_flow_rec_picture_buffer[MAX_TPL_LA_SW];
    EbPictureBufferDesc* mc_flow_rec_picture_buffer_noref;
    FrameInfo            frame_info;
    TwoPassCfg           two_pass_cfg; // two pass datarate control
    RATE_CONTROL         rc;
    RateControlCfg       rc_cfg;
    SwitchFrameCfg       sf_cfg;
    FIRSTPASS_STATS*     frame_stats_buffer;
    // Number of stats buffers required for look ahead
    int               num_lap_buffers;
    STATS_BUFFER_CTX  stats_buf_context;
    SvtAv1FixedBuf    rc_stats_buffer; // replaced oxcf->two_pass_cfg.stats_in in aom
    FirstPassStatsOut stats_out;
    RecodeLoopType    recode_loop;
    // This feature controls the tolerence vs target used in deciding whether to
    // recode a frame. It has no meaning if recode is disabled.
    int                               recode_tolerance;
    int32_t                           frame_updated;
    EbHandle                          frame_updated_mutex;
    RateControlIntervalParamContext** rc_param_queue;
    int32_t                           rc_param_queue_head_index;
    EbHandle                          rc_param_queue_mutex;
    // reference scaling random access event
    EbRefFrameScale resize_evt;
    //Superblock end index for cycling refresh through the frame.
    uint32_t         cr_sb_end;
    SvtAv1RoiMapEvt* roi_map_evt;
    Quants           quants_bd; // follows input bit depth
    Dequants         deq_bd; // follows input bit depth
    Quants           quants_8bit; // 8bit
    Dequants         deq_8bit; // 8bit
} EncodeContext;

typedef struct EncodeContextInitData {
    int32_t junk;
} EncodeContextInitData;

/**************************************
 * Extern Function Declarations
 **************************************/
EbErrorType svt_aom_encode_context_ctor(EncodeContext* enc_ctx, EbPtr object_init_data_ptr);
#endif // EbEncodeContext_h
