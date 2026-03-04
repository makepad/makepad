/*
 * Copyright(c) 2019 Netflix, Inc.
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
#ifndef EbTemporalFiltering_h
#define EbTemporalFiltering_h

#include "pcs.h"
#include "me_process.h"

// ALT-REF debug-specific defines

#define COLOR_CHANNELS 3
#define C_Y 0
#define C_U 1
#define C_V 2

#define EDGE_THRESHOLD 50
#define SQRT_PI_BY_2_FP16 82137
#define SMOOTH_THRESHOLD 16
// Block size used in temporal filtering
#define BW 64
#define BH 64
#define BLK_PELS 4096 // Pixels in the block
// A scale factor used in plane-wise temporal filtering to raise the filter
// weight from `double` with range [0, 1] to `int` with range [0, 1000].
#define TF_PLANEWISE_FILTER_WEIGHT_SCALE 1000
// Hyper-parameters used to compute filtering weight. These hyper-parameters can
// be tuned for a better performance.
// 0. A scale factor used in temporal filtering to raise the filter weight from
//    `double` with range [0, 1] to `int` with range [0, 1000].
#define TF_WEIGHT_SCALE 1000
// 1. Weight factor used to balance the weighted-average between window error
//    and block error. The weight is for window error while the weight for block
//    error is always set as 1.
#define TF_WINDOW_BLOCK_BALANCE_WEIGHT 5
// 2. Threshold for using q to adjust the filtering weight. Concretely, when
//    using a small q (high bitrate), we would like to reduce the filtering
//    strength such that more detailed information can be preserved. Hence, when
//    q is smaller than this threshold, we will adjust the filtering weight
//    based on the q-value.
#define TF_Q_DECAY_THRESHOLD 20
// 3. Threshold for using motion search distance to adjust the filtering weight.
//    Concretely, larger motion search vector leads to a higher probability of
//    unreliable search. Hence, we would like to reduce the filtering strength
//    when the distance is large enough. Considering that the distance actually
//    relies on the frame size, this threshold is also a resolution-based
//    threshold. Taking 720p videos as an instance, if this field equals to 0.1,
//    then the actual threshold will be 720 * 0.1 = 72. Similarly, the threshold
//    for 360p videos will be 360 * 0.1 = 36.
#define TF_SEARCH_DISTANCE_THRESHOLD 0.1
// 4. Threshold to identify if the q is in a relative high range.
//    Above this cutoff q, a stronger filtering is applied.
//    For a high q, the quantization throws away more information, and thus a
//    stronger filtering is less likely to distort the encoded quality, while a
//    stronger filtering could reduce bit rates.
//    Ror a low q, more details are expected to be retained. Filtering is thus
//    more conservative.
#define TF_QINDEX_CUTOFF 128

#define N_8X8_BLOCKS 64
#define N_16X16_BLOCKS 16

#define LOW_ERROR_THRESHOLD 200
#define MED_ERROR_THRESHOLD 2000

#define OD_DIVU_DMAX (1024)
#ifdef __cplusplus
extern "C" {
#endif

EbErrorType svt_av1_init_temporal_filtering(PictureParentControlSet** pcs_list, PictureParentControlSet* centre_pcs,
                                            MotionEstimationContext_t* me_context_ptr, int32_t segment_index);
void        svt_av1_apply_zz_based_temporal_filter_planewise_medium_c(
           MeContext* me_ctx, const uint8_t* y_pre, int y_pre_stride, const uint8_t* u_pre, const uint8_t* v_pre,
           int uv_pre_stride, unsigned int block_width, unsigned int block_height, int ss_x, int ss_y, uint32_t* y_accum,
           uint16_t* y_count, uint32_t* u_accum, uint16_t* u_count, uint32_t* v_accum, uint16_t* v_count);

void svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_c(
    MeContext* me_ctx, const uint16_t* y_pre, int y_pre_stride, const uint16_t* u_pre, const uint16_t* v_pre,
    int uv_pre_stride, unsigned int block_width, unsigned int block_height, int ss_x, int ss_y, uint32_t* y_accum,
    uint16_t* y_count, uint32_t* u_accum, uint16_t* u_count, uint32_t* v_accum, uint16_t* v_count,
    uint32_t encoder_bit_depth);
void svt_av1_apply_temporal_filter_planewise_medium_c(MeContext* me_ctx, const uint8_t* y_src, int y_src_stride,
                                                      const uint8_t* y_pre, int y_pre_stride, const uint8_t* u_src,
                                                      const uint8_t* v_src, int uv_src_stride, const uint8_t* u_pre,
                                                      const uint8_t* v_pre, int uv_pre_stride, unsigned int block_width,
                                                      unsigned int block_height, int ss_x, int ss_y, uint32_t* y_accum,
                                                      uint16_t* y_count, uint32_t* u_accum, uint16_t* u_count,
                                                      uint32_t* v_accum, uint16_t* v_count);

void svt_av1_apply_temporal_filter_planewise_medium_hbd_c(
    MeContext* me_ctx, const uint16_t* y_src, int y_src_stride, const uint16_t* y_pre, int y_pre_stride,
    const uint16_t* u_src, const uint16_t* v_src, int uv_src_stride, const uint16_t* u_pre, const uint16_t* v_pre,
    int uv_pre_stride, unsigned int block_width, unsigned int block_height, int ss_x, int ss_y, uint32_t* y_accum,
    uint16_t* y_count, uint32_t* u_accum, uint16_t* u_count, uint32_t* v_accum, uint16_t* v_count,
    uint32_t encoder_bit_depth);

int32_t svt_aom_noise_log1p_fp16(int32_t noise_level_fp16);

typedef struct {
    uint8_t      subpel_pel_mode;
    signed short xd;
    signed short yd;
    signed short mv_x;
    signed short mv_y;
    uint32_t     interp_filters;
    uint16_t     pu_origin_x;
    uint16_t     pu_origin_y;
    uint16_t     local_origin_x;
    uint16_t     local_origin_y;
    uint32_t     bsize;
    uint8_t      is_highbd;
    uint8_t      encoder_bit_depth;
    uint8_t      subsampling_shift;
    uint32_t     idx_x;
    uint32_t     idx_y;
} TF_SUBPEL_SEARCH_PARAMS;
#ifdef __cplusplus
}
#endif
#endif //EbTemporalFiltering_h
