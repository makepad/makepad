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

#ifndef EbPictureDecision_h
#define EbPictureDecision_h

#include "definitions.h"
#include "pcs.h"
#include "sequence_control_set.h"
#include "utility.h"

/***************************************
 * Extern Function Declaration
 ***************************************/
EbErrorType svt_aom_picture_decision_context_ctor(EbThreadContext* thread_ctx, const EbEncHandle* enc_handle_ptr,
                                                  uint8_t calc_hist);
void*       svt_aom_picture_decision_kernel(void* input_ptr);

void svt_aom_pad_picture_to_multiple_of_min_blk_size_dimensions(SequenceControlSet*  scs,
                                                                EbPictureBufferDesc* input_pic);
void svt_aom_pad_picture_to_multiple_of_min_blk_size_dimensions_16bit(SequenceControlSet*  scs,
                                                                      EbPictureBufferDesc* input_pic);
void svt_aom_picture_pre_processing_operations(PictureParentControlSet* pcs, SequenceControlSet* scs);
void svt_aom_pad_picture_to_multiple_of_sb_dimensions(EbPictureBufferDesc* input_padded_pic);
void svt_aom_gathering_picture_statistics(SequenceControlSet* scs, PictureParentControlSet* pcs,
                                          EbPictureBufferDesc* input_padded_pic,
                                          EbPictureBufferDesc* sixteenth_decimated_picture_ptr);

void svt_aom_down_sample_chroma(EbPictureBufferDesc* input_pic, EbPictureBufferDesc* outputPicturePtr);

bool svt_aom_is_delayed_intra(PictureParentControlSet* pcs);

uint8_t     svt_aom_tf_max_ref_per_struct(uint32_t hierarchical_levels, uint8_t type /*I_SLICE, BASE, L1*/,
                                          bool direction /*Past, Future*/);
EbErrorType svt_aom_prediction_structure_group_ctor(PredictionStructureGroup* pred_struct_group_ptr);
bool        svt_aom_is_pic_used_as_ref(unsigned hierarchical_levels, unsigned temporal_layer, unsigned picture_index,
                                       unsigned referencing_scheme, bool is_overlay);
bool        svt_aom_is_incomp_mg_frame(PictureParentControlSet* pcs);

typedef struct DpbEntry {
    uint64_t picture_number;
    uint64_t decode_order;
    uint8_t  temporal_layer_index;
} DpbEntry;

/**************************************
 * Context
 **************************************/
typedef struct PictureDecisionContext {
    EbDctor dctor;
    EbFifo* picture_analysis_results_input_fifo_ptr;
    EbFifo* picture_decision_results_output_fifo_ptr;
    EbFifo* me_fifo_ptr;

    bool        reset_running_avg;
    int8_t      tf_motion_direction; // -1: invalid   0: horz  1: vert
    uint32_t*** prev_picture_histogram;
    uint64_t    prev_average_intensity_per_region[MAX_NUMBER_OF_REGIONS_IN_WIDTH][MAX_NUMBER_OF_REGIONS_IN_HEIGHT];
    uint32_t**  ahd_running_avg_cb;
    uint32_t**  ahd_running_avg_cr;
    uint32_t**  ahd_running_avg;
    bool        is_scene_change_detected;
    int8_t      transition_detected; // -1: not computed
    // The signal transition_detected is set for only the RA case, and used to derive transition_present flag
    // If the scene change happens during a complete mini-GOP, then transition_present is set to 1
    // for only the next BASE. However, if the scene change happens during an incomplete mini-GOP
    // then transition_present is set to 1 for all P(s) until the next BASE as they would not take advantage of
    // the next BASE boost since they only use past reference frame(s)
    // When transition_present is set to 1, different action(s) will be taken to mimic an I_SLICE (decrease the QP, better INTRA search level,
    // shut depth-removal, ..). The QP action is not applied if a P.
    // Dynamic GOP
    uint32_t total_number_of_mini_gops;

    uint32_t mini_gop_start_index[MINI_GOP_WINDOW_MAX_COUNT];
    uint32_t mini_gop_end_index[MINI_GOP_WINDOW_MAX_COUNT];
    uint32_t mini_gop_length[MINI_GOP_WINDOW_MAX_COUNT];
    uint32_t mini_gop_intra_count[MINI_GOP_WINDOW_MAX_COUNT];
    uint32_t mini_gop_idr_count[MINI_GOP_WINDOW_MAX_COUNT];
    uint32_t mini_gop_hierarchical_levels[MINI_GOP_WINDOW_MAX_COUNT];
    bool     mini_gop_activity_array[MINI_GOP_MAX_COUNT];
    uint32_t mini_gop_region_activity_cost_array[MINI_GOP_MAX_COUNT][MAX_NUMBER_OF_REGIONS_IN_WIDTH]
                                                [MAX_NUMBER_OF_REGIONS_IN_HEIGHT];

    uint32_t                 mini_gop_group_faded_in_pictures_count[MINI_GOP_MAX_COUNT];
    uint32_t                 mini_gop_group_faded_out_pictures_count[MINI_GOP_MAX_COUNT];
    uint8_t                  lay0_toggle; //3 way toggle 0->1->2
    uint8_t                  lay1_toggle; //2 way toggle 0->1
    uint8_t                  cut_short_ra_mg;
    DpbEntry                 dpb[REF_FRAMES];
    bool                     mini_gop_toggle; //mini GOP toggling since last Key Frame  K-0-1-0-1-0-K-0-1-0-1-K-0-1.....
    uint8_t                  last_i_picture_sc_class0;
    uint8_t                  last_i_picture_sc_class1;
    uint8_t                  last_i_picture_sc_class2;
    uint8_t                  last_i_picture_sc_class3;
    uint8_t                  last_i_picture_sc_class4;
    uint64_t                 last_long_base_pic;
    uint64_t                 key_poc;
    uint8_t                  tf_level;
    uint32_t                 tf_pic_arr_cnt;
    PictureParentControlSet* tf_pic_array[1 << MAX_TEMPORAL_LAYERS];
    PictureParentControlSet* mg_pictures_array[1 << MAX_TEMPORAL_LAYERS];
    PictureParentControlSet* prev_delayed_intra; //Key frame or I of LDP short MG
    uint32_t                 mg_size; //number of active pictures in above array
    PictureParentControlSet* mg_pictures_array_disp_order[1 << MAX_TEMPORAL_LAYERS];
    int64_t                  base_counter;
    bool                     gm_pp_last_detected;
    int64_t                  mg_progress_id;

    int32_t last_i_noise_levels_log1p_fp16[MAX_MB_PLANE];
    double  last_i_noise_levels[MAX_MB_PLANE];

    // for switch frame feature
    uint32_t ref_order_hint[REF_FRAMES]; // spec 6.8.2
    uint64_t sframe_poc;
    int32_t  sframe_due; // The flag indicates whether the next ARF will be made an s-frame
    uint8_t* sixteenth_b64_buffer;
    uint32_t sixteenth_b64_buffer_stride;
    uint64_t norm_dist;
    uint8_t  perc_cplx;
    uint8_t  perc_active;
    int16_t  mv_in_out_count;
    bool     enable_startup_mg;
    uint32_t filt_to_unfilt_diff;
    bool     list0_only;
    bool     is_startup_gop;
    int32_t  sframe_hier_lvls;
    uint64_t sframe_last_arf;
    bool     next_arf_is_s;
} PictureDecisionContext;

#endif // EbPictureDecision_h
