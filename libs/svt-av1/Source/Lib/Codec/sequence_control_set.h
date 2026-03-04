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

#ifndef EbSequenceControlSet_h
#define EbSequenceControlSet_h

#include "definitions.h"
#include "av1_structs.h"
#include "encode_context.h"
#include "object.h"
#include "firstpass.h"

#ifdef __cplusplus
extern "C" {
#endif
typedef enum EncPass {
    ENC_SINGLE_PASS, //single pass mode
    ENC_FIRST_PASS, // first pass of two pass mode
    ENC_SECOND_PASS, // Second pass of two pass mode
    MAX_ENCODE_PASS = 2,
} EncPass;

typedef struct BitstreamLevel {
    uint8_t major;
    uint8_t minor;
} BitstreamLevel;

typedef struct QpBasedThScaling {
    bool tf_me_qp_based_th_scaling;
    bool tf_ref_qp_based_th_scaling;
    bool depths_qp_based_th_scaling;
    bool hme_qp_based_th_scaling;
    bool me_qp_based_th_scaling;
    bool nsq_qp_based_th_scaling;
    bool nic_max_qp_based_th_scaling;
    bool nic_pruning_qp_based_th_scaling;
    bool pme_qp_based_th_scaling;
    bool txt_qp_based_th_scaling;
#if !CLN_DR
    bool i_depth_removal_qp_based_th_scaling;
#endif
    bool cap_max_size_qp_based_th_scaling;
#if !CLN_REMOVE_VAR_SUB_DEPTH
    bool var_skip_sub_depth_qp_based_th_scaling;
#endif
#if FTR_VLPD0
    bool lpd0_qp_based_th_scaling;
#endif
} QpBasedThScaling;

// Forward declaration for block geometry
struct BlockGeom;

/************************************
     * Sequence Control Set
     ************************************/
typedef struct SequenceControlSet {
    /*!< Pointer to the dtor of the struct*/
    EbDctor dctor;
    /*!< Encoding context pointer containing the handle pointer */
    EncodeContext* enc_ctx;
    /*!< 2ndpass enc mode, available at firstpass encoder */
    /*!< API structure */
    EbSvtAv1EncConfiguration static_config;
    /*!< Super block geomerty pointer */
    SbGeom* sb_geom;
    /*!< Array of superblock parameters computed at the resource coordination stage */
    B64Geom* b64_geom;
    /*!< Bitstream level */
    BitstreamLevel level[MAX_NUM_OPERATING_POINTS];
    /*!< Sequence header structure, common between the encoder and decoder */
    SeqHeader seq_header;

    /*!< Sequence coding parameters
            parameters/features are set to be set for the full stream
            but encoding decisions may still be taken at a picture / sub picture level
    */
    /*!< Maximum number of allowed temporal layers */
    uint32_t max_temporal_layers;
    /*!< Number of delay frames needed to implement future window
         for algorithms such as SceneChange or TemporalFiltering */
    uint32_t scd_delay;
    /*!<  */
    EbBlockMeanPrec block_mean_calc_prec;

    uint32_t svt_aom_geom_idx; //geometry type

    /*  1..15    | 17..31  | 33..47  |
              16 |       32|       48|
      lad mg=2: delay the first MG (1-16) until the next 2 MGs(17-48) are gop , TF, and ME ready
    */
    // delay all pictures within a given MG, until N future MGs are  gop , TF, and ME ready
    uint8_t lad_mg;
    // delay all pictures within a given MG, until N future MGs are  gop , TF, and ME ready used for
    // tpl
    uint8_t tpl_lad_mg;
    /*!< 1: Specifies that loop restoration filter should use boundary pixels in the search.  Must
       be set at the sequence level because it requires a buffer allocation to copy the pixels to be
       used in the search. 0: Specifies that loop restoration filter should not use boundary pixels
       in the search.*/
    uint8_t use_boundaries_in_rest_search;
    uint8_t enable_pic_mgr_dec_order; // if enabled: pic mgr starts pictures in dec order
    uint8_t enable_dec_order; // if enabled: encoding are in dec order
    /*!< Allow the usage of motion field motion vector in the stream
        (The signal changes per preset; 0: Enabled, 1: Disabled) Default is 1. */
    uint8_t mfmv_enabled;
    /*!< Enable dynamic GoP
        (The signal changes per preset; 0: Disabled, 1: Enabled) Default is 1. */
    uint8_t enable_dg;
    /*!< Film grain seed */
    uint16_t film_grain_random_seed;

    /*!< Sequence resolution parameters */
    uint32_t          chroma_format_idc;
    uint16_t          subsampling_x; // add chroma subsampling parameters
    uint16_t          subsampling_y;
    uint16_t          max_input_luma_width; // input luma width aligned to 8, this is used during encoding
    uint16_t          max_input_luma_height; // input luma height aligned to 8, this is used during encoding
    uint16_t          max_input_pad_bottom;
    uint16_t          max_input_pad_right;
    uint16_t          max_initial_input_luma_width; // max init time input luma width aligned to 8
    uint16_t          max_initial_input_luma_height; // max init time input luma height aligned to 8
    uint16_t          max_initial_input_pad_bottom; // max init time input pad bottom
    uint16_t          max_initial_input_pad_right; // max init time input pad right
    uint32_t          chroma_width;
    uint32_t          chroma_height;
    uint32_t          pad_right;
    uint32_t          pad_bottom;
    uint16_t          left_padding;
    uint16_t          top_padding;
    uint16_t          right_padding;
    uint16_t          bot_padding;
    double            frame_rate;
    uint32_t          encoder_bit_depth;
    EbInputResolution input_resolution;

    /*!< Super block parameters set for the stream */
    uint8_t  b64_size;
    uint16_t pic_width_in_b64;
    uint16_t pic_height_in_b64;
    uint16_t b64_total_count;
    uint16_t sb_size;
    uint16_t picture_width_in_sb;
    uint16_t picture_height_in_sb;
    uint16_t sb_total_count;
    uint16_t max_block_cnt;
    // Pointer to block geometry table (owned by EbEncHandle)
    struct BlockGeom* blk_geom_mds;
    /*!< Restoration Unit parameters set for the stream */
    int32_t rest_units_per_tile;
    /*!< Sub picture reagions for picture analysis */
    uint32_t picture_analysis_number_of_regions_per_width;
    uint32_t picture_analysis_number_of_regions_per_height;

    /*!< Tile group counts */
    uint8_t tile_group_col_count_array;
    uint8_t tile_group_row_count_array;

    /*!< Segements (sub picture) count for different processes */
    uint32_t me_segment_col_count_array;
    uint32_t me_segment_row_count_array;
    uint32_t enc_dec_segment_col_count_array;
    uint32_t enc_dec_segment_row_count_array;
    uint32_t tpl_segment_col_count_array;
    uint32_t tpl_segment_row_count_array;
    uint32_t cdef_segment_column_count;
    uint32_t cdef_segment_row_count;
    uint32_t rest_segment_column_count;
    uint32_t rest_segment_row_count;
    uint32_t tf_segment_column_count;
    uint32_t tf_segment_row_count;
    // level of parallelism determined based on the core count
    uint32_t lp;

    /*!< Picture, reference, recon and input output buffer count */
    uint32_t picture_control_set_pool_init_count;
    uint32_t me_pool_init_count;
    uint32_t picture_control_set_pool_init_count_child;
    uint32_t enc_dec_pool_init_count;
    uint32_t pa_reference_picture_buffer_init_count;
    uint32_t tpl_reference_picture_buffer_init_count;
    /* ref_buffer_available_semaphore is needed so that all REF pictures
    sent to PM will have an available ref buffer. If ref buffers are
    not available in PM, it will result in a deadlock.*/
    EbHandle ref_buffer_available_semaphore;
    uint32_t reference_picture_buffer_init_count;
    uint32_t input_buffer_fifo_init_count;
    uint32_t overlay_input_picture_buffer_init_count;
    uint32_t output_stream_buffer_fifo_init_count;
    uint32_t output_recon_buffer_fifo_init_count;

    /*!< Inter processes fifos count */
    uint32_t resource_coordination_fifo_init_count;
    uint32_t picture_analysis_fifo_init_count;
    uint32_t picture_decision_fifo_init_count;
    uint32_t motion_estimation_fifo_init_count;
    uint32_t initial_rate_control_fifo_init_count;
    uint32_t picture_demux_fifo_init_count;
    uint32_t tpl_disp_fifo_init_count;
    uint32_t rate_control_tasks_fifo_init_count;
    uint32_t rate_control_fifo_init_count;
    uint32_t mode_decision_configuration_fifo_init_count;
    uint32_t enc_dec_fifo_init_count;
    uint32_t entropy_coding_fifo_init_count;
    uint32_t dlf_fifo_init_count;
    uint32_t cdef_fifo_init_count;
    uint32_t rest_fifo_init_count;

    /*!< Thread count for each process */
    uint32_t picture_analysis_process_init_count;
    uint32_t motion_estimation_process_init_count;
    uint32_t source_based_operations_process_init_count;
    uint32_t mode_decision_configuration_process_init_count;
    uint32_t enc_dec_process_init_count;
    uint32_t entropy_coding_process_init_count;
    uint32_t dlf_process_init_count;
    uint32_t cdef_process_init_count;
    uint32_t rest_process_init_count;
    uint32_t tpl_disp_process_init_count;
    uint32_t total_process_init_count;
    int32_t  lap_rc;
    TWO_PASS twopass;
    /*!
    * Updated framerate for the current parallel frame.
    * cpi->framerate is updated with new_framerate during
    * post encode updates for parallel frames.
    */
    double       new_framerate;
    ScaleFactors sf_identity;
    VqCtrls      vq_ctrls;
    uint8_t      calc_hist;
    TfControls   tf_params_per_type[3]; // [I_SLICE][BASE][L1]
    MrpCtrls     mrp_ctrls;
    /*!< The RC stat generation pass mode (0: The default, 1: optimized)*/
    uint8_t rc_stat_gen_pass_mode;
#if TUNE_CQP_CHROMA_SSIM
    int cqp_base_q;
#endif
    // less than 200 frames or gop_constraint_rc is set, used in VBR and set in multipass encode
    uint8_t is_short_clip;
    uint8_t passes;
    // use downsampled input for first pass
    bool    first_pass_downsample;
    uint8_t final_pass_preset;
    /* Specifies whether to use 16bit pipeline.
    *
    * 0: 8 bit pipeline.
    * 1: 16 bit pipeline.
    * Now 16bit pipeline is only enabled in filter
    * Default is 0. */
    bool is_16bit_pipeline;

    /* Super block size (mm-signal)
    *
    * Default is 128. */
    uint32_t super_block_size;
    /* Picture based rate estimation
    *
    * Default is false. */
    bool pic_based_rate_est;

    // MD Parameters
    /* Enable the use of HBD (10-bit) for 10 bit content at the mode decision step
     *
     * 0 = 8bit mode decision
     * 1 = 10bit mode decision
     * 2 = Auto: 8bit & 10bit mode decision
     *
    * Default is 1. */
    int8_t enable_hbd_mode_decision;

    /* Enable picture QP scaling between hierarchical levels
    *
    * Default is null.*/
    int enable_qp_scaling_flag;

    /* Flag to enable the Speed Control functionality to achieve the real-time
    * encoding speed defined by dynamically changing the encoding preset to meet
    * the average speed defined in injectorFrameRate. When this parameter is set
    * to 1 it forces -inj to be 1 -inj-frm-rt to be set to the -fps.
    *
    * Default is 0. */
    int speed_control_flag;
    /* TPL
    *
    * 0: OFF, 1: ON. */
    uint8_t tpl;
    // If true, calculate and store the SB-based variance
    uint8_t calculate_variance;
    // Whether to modulation lambda using TPL stats or/and ME-stats or/and the percentage of INTRA selection at reference frame(s)
    bool stats_based_sb_lambda_modulation;
    // Desired dimensions for an externally triggered resize
    ResizePendingParams resize_pending_params;
    // Specifies whether to use List1 for BASE frame(s) or not
    bool list0_only_base;
    // Control if feature levels are directly modulated using the sequence QP.
    // 0: No seq QP modulation
    // 1: Enable only high-QP modulation (apply conservative offsets to high QP)
    // 2: (Default) Enable all QP modulation (apply conservative offsets to high QP, aggressive offsets to low QP)
    // 3: Enable only low-QP modulaiton (apply aggressive offsets to low QP)
    uint8_t seq_qp_mod;
    // Control per tool whether we use the qp in calculating the scaling factors for the exponential QP-based function
    // 0: Automatically assign 1 to ret_q_weight and to ret_q_weight_denom.
    // 1: Use the qp to calculate ret_q_weight and to ret_q_weight_denom.
    QpBasedThScaling qp_based_th_scaling_ctrls;
    // If true, intra_period_length is 0 and every frame is coded with intra tools only
    bool allintra;
    // If true, use a flat IPP pred structure, where each pic uses only the previous frame as ref
    bool use_flat_ipp;
    // If true, enables fast anti-alias aware screen detection
    bool fast_aa_aware_screen_detection_mode;
} SequenceControlSet;

typedef struct EbSequenceControlSetInstance {
    EbDctor             dctor;
    EncodeContext*      enc_ctx;
    SequenceControlSet* scs;
    EbHandle            config_mutex;
} EbSequenceControlSetInstance;

/**************************************
     * Extern Function Declarations
     **************************************/
EbErrorType svt_sequence_control_set_instance_ctor(EbSequenceControlSetInstance* object_ptr);

EbErrorType svt_aom_derive_input_resolution(EbInputResolution* input_resolution, uint32_t input_size);
EbErrorType copy_sequence_control_set(SequenceControlSet* dst, SequenceControlSet* src);
EbErrorType svt_aom_scs_set_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr);

#ifdef __cplusplus
}
#endif
#endif // EbSequenceControlSet_h
