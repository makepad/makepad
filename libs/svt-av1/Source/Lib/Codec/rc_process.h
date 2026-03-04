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

#ifndef EbRateControl_h
#define EbRateControl_h

#include "EbDebugMacros.h"
#include "definitions.h"
#include "sys_resource_manager.h"
#include "EbSvtAv1Enc.h"
#include "pcs.h"
#include "object.h"

#define MINQ_ADJ_LIMIT 48
#define HIGH_UNDERSHOOT_RATIO 2

// Bits Per MB at different Q (Multiplied by 512)
#define BPER_MB_NORMBITS 9

#define FRAME_OVERHEAD_BITS 200

// Threshold used to define if a KF group is static (e.g. a slide show).
// Essentially, this means that no frame in the group has more than 1% of MBs
// that are not marked as coded with 0,0 motion in the first pass.
#define STATIC_KF_GROUP_THRESH 99

#define MAX_GF_INTERVAL 32
#define MAX_ARF_LAYERS 6

typedef enum rate_factor_level {
    INTER_NORMAL       = 0,
    INTER_LOW          = 1,
    INTER_HIGH         = 2,
    GF_ARF_LOW         = 3,
    GF_ARF_STD         = 4,
    KF_STD             = 5,
    RATE_FACTOR_LEVELS = 6
} rate_factor_level;

#define CODED_FRAMES_STAT_QUEUE_MAX_DEPTH 2000
// max bit rate average period
#define MAX_RATE_AVG_PERIOD (CODED_FRAMES_STAT_QUEUE_MAX_DEPTH >> 1)
#define CRITICAL_BUFFER_LEVEL 15
#define OPTIMAL_BUFFER_LEVEL 70

#define MAX_GFUBOOST_FACTOR 10.0

#define MIN_BPB_FACTOR 0.005
#define MAX_BPB_FACTOR 50

#define BOOST_GF_HIGH_TPL_LA 2400
#define BOOST_GF_LOW_TPL_LA 300
#define BOOST_KF_HIGH 5000
#define BOOST_KF_LOW 400

#define CR_SEGMENT_ID_BASE 0
#define CR_SEGMENT_ID_BOOST1 1
#define CR_SEGMENT_ID_BOOST2 2

extern const int svt_av1_non_base_qindex_weight_ref[EB_MAX_TEMPORAL_LAYERS];
extern const int svt_av1_non_base_qindex_weight_wq[EB_MAX_TEMPORAL_LAYERS];

extern const double svt_av1_tpl_hl_islice_div_factor[EB_MAX_TEMPORAL_LAYERS];
extern const double svt_av1_tpl_hl_base_frame_div_factor[EB_MAX_TEMPORAL_LAYERS];

extern const double svt_av1_r0_weight[3];
extern const double svt_av1_qp_scale_compress_weight[4];

extern const double            svt_av1_rate_factor_deltas[RATE_FACTOR_LEVELS];
extern const rate_factor_level svt_av1_rate_factor_levels[SVT_AV1_FRAME_UPDATE_TYPES];

/**************************************
 * Coded Frames Stats
 **************************************/
typedef struct coded_frames_stats_entry {
    EbDctor  dctor;
    uint64_t picture_number;
    int64_t  frame_total_bit_actual;
    bool     end_of_sequence_flag;
} coded_frames_stats_entry;

typedef enum {
    NO_RESIZE      = 0,
    DOWN_THREEFOUR = 1, // From orig to 3/4.
    DOWN_ONEHALF   = 2, // From orig or 3/4 to 1/2.
    UP_THREEFOUR   = -1, // From 1/2 to 3/4.
    UP_ORIG        = -2, // From 1/2 or 3/4 to orig.
} RESIZE_ACTION;

typedef enum { ORIG = 0, THREE_QUARTER = 1, ONE_HALF = 2 } RESIZE_STATE;

/*!
 * \brief Desired dimensions for an externally triggered resize.
 *
 * When resize is triggered externally, the desired dimensions are stored in
 * this struct until used in the next frame to be coded. These values are
 * effective only for one frame and are reset after they are used.
 */
typedef struct {
    RESIZE_STATE resize_state;
    uint8_t      resize_denom;
} ResizePendingParams;

EbErrorType svt_aom_rate_control_coded_frames_stats_context_ctor(coded_frames_stats_entry* entry_ptr,
                                                                 uint64_t                  picture_number);

typedef struct RATE_CONTROL {
    int     last_boosted_qindex; // Last boosted GF/KF/ARF q
    int     gfu_boost;
    int     kf_boost;
    double  rate_correction_factors[MAX_TEMPORAL_LAYERS + 1];
    int     baseline_gf_interval;
    int     constrained_gf_group;
    int     frames_to_key;
    int     frames_since_key;
    int     this_key_frame_forced;
    int     avg_frame_bandwidth; // Average frame size target for clip
    int     max_frame_bandwidth; // Maximum burst rate allowed for a frame.
    int     avg_frame_qindex[FRAME_TYPES];
    int64_t buffer_level;
    int64_t bits_off_target;
    int64_t vbr_bits_off_target;
    int64_t vbr_bits_off_target_fast;
    int     rolling_target_bits;
    int     rolling_actual_bits;
    int     rate_error_estimate;

    int64_t total_actual_bits;
    int64_t total_target_bits;

    int worst_quality;
    int best_quality;

    // Track amount of low motion in scene
    int     avg_frame_low_motion;
    int64_t starting_buffer_level;
    int64_t optimal_buffer_level;
    int64_t maximum_buffer_size;

    // rate control history for last frame(1) and the frame before(2).
    // -1: undershot
    //  1: overshoot
    //  0: not initialized.
    int rc_1_frame;
    int rc_2_frame;
    int q_1_frame;
    int q_2_frame;
    /*!
     * Active adjustment delta for cyclic refresh for rate control.
     */
    int percent_refresh_adjustment;
    /*!
    * Active adjustment of qdelta rate ratio for enhanced rate control
    */
    double rate_ratio_qdelta_adjustment;
    // Q index used for ALT frame
    int arf_q;

    // real for TWOPASS_RC
    int prev_avg_frame_bandwidth; //only for CBR?
    int active_worst_quality;
    int active_best_quality[MAX_ARF_LAYERS + 1];

    // gop bit budget
    int64_t gf_group_bits;
    // Rate Control stat Queue
    coded_frames_stats_entry** coded_frames_stat_queue;
    uint32_t                   coded_frames_stat_queue_head_index;

#if DEBUG_RC_CAP_LOG
    uint64_t max_bit_actual_per_gop;
    uint64_t min_bit_actual_per_gop;
#endif
    uint64_t rate_average_periodin_frames;

    EbHandle rc_mutex;
    // For dynamic resize, 1 pass cbr.
    RESIZE_STATE resize_state;
    int32_t      resize_avg_qp;
    int32_t      resize_buffer_underflow;
    int32_t      resize_count;
    int32_t      last_q[FRAME_TYPES]; // Q used on last encoded frame of the given type.

    // current and previous average base layer ME distortion
    uint32_t cur_avg_base_me_dist;
    uint32_t prev_avg_base_me_dist;
} RATE_CONTROL;

/**************************************
 * Input Port Types
 **************************************/
typedef enum RateControlInputPortTypes {
    RATE_CONTROL_INPUT_PORT_INLME         = 0,
    RATE_CONTROL_INPUT_PORT_PACKETIZATION = 1,
    RATE_CONTROL_INPUT_PORT_TOTAL_COUNT   = 2,
    RATE_CONTROL_INPUT_PORT_INVALID       = ~0,
} RateControlInputPortTypes;

/**************************************
 * Input Port Config
 **************************************/
typedef struct RateControlPorts {
    RateControlInputPortTypes type;
    uint32_t                  count;
} RateControlPorts;

typedef enum PicMgrInputPortTypes {
    PIC_MGR_INPUT_PORT_SOP           = 0,
    PIC_MGR_INPUT_PORT_PACKETIZATION = 1,
    PIC_MGR_INPUT_PORT_REST          = 2,
    PIC_MGR_INPUT_PORT_TOTAL_COUNT   = 3,
    PIC_MGR_INPUT_PORT_INVALID       = ~0,
} PicMgrInputPortTypes;

typedef struct PicMgrPorts {
    PicMgrInputPortTypes type;
    uint32_t             count;
} PicMgrPorts;

/**************************************
 * Extern Function Declarations
 **************************************/
struct PictureControlSet;
struct PictureParentControlSet;
struct SequenceControlSet;

// AQ
void svt_av1_rc_init_sb_qindex(struct PictureControlSet* pcs, struct SequenceControlSet* scs);
void svt_av1_variance_adjust_qp(struct PictureControlSet* pcs);
void svt_aom_sb_qp_derivation_tpl_la(struct PictureControlSet* pcs);
void svt_av1_normalize_sb_delta_q(struct PictureControlSet* pcs);

int32_t svt_av1_convert_qindex_to_q_fp8(int32_t qindex, EbBitDepth bit_depth);
int32_t svt_av1_compute_qdelta_fp(int32_t qstart_fp8, int32_t qtarget_fp8, EbBitDepth bit_depth);

void svt_aom_cyclic_refresh_init(struct PictureParentControlSet* ppcs);

// CQP/CRF
void svt_av1_rc_calc_qindex_crf_cqp(struct PictureControlSet* pcs, struct SequenceControlSet* scs);
void svt_av1_coded_frames_stat_calc(struct PictureParentControlSet* ppcs);

// VBR/CBR
void svt_av1_rc_process_rate_allocation(struct PictureControlSet* pcs, struct SequenceControlSet* scs);
void svt_av1_rc_calc_qindex_rate_control(struct PictureControlSet* pcs, struct SequenceControlSet* scs);
void svt_av1_rc_postencode_update_gop_const(struct PictureParentControlSet* ppcs);
void svt_av1_rc_postencode_update(struct PictureParentControlSet* ppcs);

// common stuff
void    svt_av1_rc_init(struct SequenceControlSet* scs);
int32_t svt_av1_compute_qdelta(double qstart, double qtarget, EbBitDepth bit_depth);
double  svt_av1_convert_qindex_to_q(int32_t qindex, EbBitDepth bit_depth);
int     svt_av1_calculate_boost_bits(int frame_count, int boost, int64_t total_group_bits);
int     svt_av1_compute_deltaq(struct PictureParentControlSet* ppcs, int q, double rate_ratio_qdelta);

int svt_aom_frame_is_kf_gf_arf(struct PictureParentControlSet* ppcs);

int svt_av1_rc_bits_per_mb(FrameType frame_type, int qindex, double correction_factor, int bit_depth,
                           int is_screen_content_type);
int svt_av1_get_q_index_from_qstep_ratio(int leaf_qindex, double qstep_ratio, int bit_depth);
int svt_av1_compute_qdelta_by_rate(struct RATE_CONTROL* rc, FrameType frame_type, int qindex, double rate_target_ratio,
                                   int bit_depth, int is_screen_content_type);

int svt_av1_get_cqp_kf_boost_from_r0(double r0, int frames_to_key, EbInputResolution input_resolution);
int svt_av1_get_gfu_boost_from_r0_lap(double min_factor, double max_factor, double r0, int frames_to_key);

uint32_t svt_aom_compute_rd_mult(struct PictureControlSet* pcs, uint8_t q_index, uint8_t me_q_index,
                                 EbBitDepth bit_depth);
uint32_t svt_aom_compute_fast_lambda(struct PictureControlSet* pcs, uint8_t q_index, uint8_t me_q_index,
                                     EbBitDepth bit_depth);

void capped_crf_reencode(struct PictureParentControlSet* ppcs, int* const q);

int  svt_aom_compute_rd_mult_based_on_qindex(EbBitDepth bit_depth, SvtAv1FrameUpdateType update_type, int qindex);
void svt_aom_lambda_assign(struct PictureControlSet* pcs, uint32_t* fast_lambda, uint32_t* full_lambda,
                           EbBitDepth bit_depth, uint8_t qp_index, bool multiply_lambda);
void recode_loop_update_q(struct PictureParentControlSet* ppcs, bool* const loop, int* const q, int* const q_low,
                          int* const q_high, const int top_index, const int bottom_index, int* const undershoot_seen,
                          int* const overshoot_seen, int* const low_cr_seen, const int loop_count);

EbErrorType svt_aom_rate_control_context_ctor(EbThreadContext* thread_ctx, const EbEncHandle* enc_handle_ptr,
                                              int me_port_index);

void* svt_aom_rate_control_kernel(void* input_ptr);

#endif // EbRateControl_h
