/*
 * Copyright(c) 2019 Netflix, Inc.
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the
 * Alliance for Open Media Patent License 1.0 was not distributed with this
 * source code in the PATENTS file, you can obtain it at
 * https://www.aomedia.org/license/patent-license.
 */

/******************************************************************************
 * @file RefDecoder.cc
 *
 * @brief Impelmentation of reference decoder
 *
 * @author Cidana-Edmond
 *
 ******************************************************************************/
#include <stdlib.h>
#include <cmath>
#include <algorithm>
#include "aom/aom_decoder.h"
#include "aom/aomdx.h"
#include "aom/inspection.h"
#include "gtest/gtest.h"
#include "RefDecoder.h"
#include "ParseUtil.h"
#ifdef _MSC_VER
// The given function is local and not referenced in the body of the module;
// therefore, the function is dead code.
// Disable it since the aom_codec_control_xxx is not used.
#pragma warning(disable : 4505)
#endif

/** from aom/common/blockd.h */
typedef enum {
    KEY_FRAME = 0,
    INTER_FRAME = 1,
    INTRA_ONLY_FRAME = 2,  // replaces intra-only
    S_FRAME = 3,
    FRAME_TYPES,
} FRAME_TYPE;

/** count intra period length from the frame serialization */
static int get_max_intra_period_length(const std::vector<int>& frame_type_vec) {
    int period_max = 0;
    int period = 0;
    for (int frame_type : frame_type_vec) {
        switch (frame_type) {
        case KEY_FRAME:
        case INTRA_ONLY_FRAME:
            period_max = std::max(period, period_max);
            period = 0;
            break;
        case INTER_FRAME:
        case S_FRAME: period++; break;
        default: printf("found unknown frame type: %d\n", frame_type); break;
        }
    }
    // if no intra, it should return -1
    if (period_max == 0)
        period_max = -1;
    return period_max;
}

/** from aom/common/enums.h */
typedef enum ATTRIBUTE_PACKED {
    BLOCK_4X4,
    BLOCK_4X8,
    BLOCK_8X4,
    BLOCK_8X8,
    BLOCK_8X16,
    BLOCK_16X8,
    BLOCK_16X16,
    BLOCK_16X32,
    BLOCK_32X16,
    BLOCK_32X32,
    BLOCK_32X64,
    BLOCK_64X32,
    BLOCK_64X64,
    BLOCK_64X128,
    BLOCK_128X64,
    BLOCK_128X128,
    BLOCK_4X16,
    BLOCK_16X4,
    BLOCK_8X32,
    BLOCK_32X8,
    BLOCK_16X64,
    BLOCK_64X16,
    BLOCK_SIZES_ALL,
    BLOCK_SIZES = BLOCK_4X16,
    BLOCK_INVALID = 255,
    BLOCK_LARGEST = (BLOCK_SIZES - 1)
} BLOCK_SIZE;

/** get the minimum block size from super block size type */
static uint32_t get_min_block_size(const uint32_t sb_type) {
    switch ((BLOCK_SIZE)sb_type) {
    case BLOCK_4X4: return 4;
    case BLOCK_4X8:
    case BLOCK_8X4:
    case BLOCK_8X8: return 8;
    case BLOCK_8X16:
    case BLOCK_16X8:
    case BLOCK_16X16:
    case BLOCK_4X16:
    case BLOCK_16X4: return 16;
    case BLOCK_16X32:
    case BLOCK_32X16:
    case BLOCK_32X32:
    case BLOCK_8X32:
    case BLOCK_32X8: return 32;
    case BLOCK_32X64:
    case BLOCK_64X32:
    case BLOCK_64X64:
    case BLOCK_16X64:
    case BLOCK_64X16: return 64;
    case BLOCK_64X128:
    case BLOCK_128X64:
    case BLOCK_128X128: return 128;
    default: assert(0); break;
    }
    return 0;
}

/** check the block type is a square or rectangle*/
static bool is_ext_block(const uint32_t sb_type) {
    switch ((BLOCK_SIZE)sb_type) {
    case BLOCK_4X4:
    case BLOCK_8X8:
    case BLOCK_16X16:
    case BLOCK_32X32:
    case BLOCK_64X64:
    case BLOCK_128X128: return false;
    default: break;
    }
    return true;
}

/** from aom/common/enums.h */
// Note: All directional predictors must be between V_PRED and D67_PRED (both
// inclusive).
typedef enum ATTRIBUTE_PACKED {
    DC_PRED,        // Average of above and left pixels
    V_PRED,         // Vertical
    H_PRED,         // Horizontal
    D45_PRED,       // Directional 45  degree
    D135_PRED,      // Directional 135 degree
    D113_PRED,      // Directional 113 degree
    D157_PRED,      // Directional 157 degree
    D203_PRED,      // Directional 203 degree
    D67_PRED,       // Directional 67  degree
    SMOOTH_PRED,    // Combination of horizontal and vertical interpolation
    SMOOTH_V_PRED,  // Vertical interpolation
    SMOOTH_H_PRED,  // Horizontal interpolation
    PAETH_PRED,     // Predict from the direction of smallest gradient
    NEARESTMV,
    NEARMV,
    GLOBALMV,
    NEWMV,
    // Compound ref compound modes
    NEAREST_NEARESTMV,
    NEAR_NEARMV,
    NEAREST_NEWMV,
    NEW_NEARESTMV,
    NEAR_NEWMV,
    NEW_NEARMV,
    GLOBAL_GLOBALMV,
    NEW_NEWMV,
    MB_MODE_COUNT,
    INTRA_MODE_START = DC_PRED,
    INTRA_MODE_END = NEARESTMV,
    INTRA_MODE_NUM = INTRA_MODE_END - INTRA_MODE_START,
    SINGLE_INTER_MODE_START = NEARESTMV,
    SINGLE_INTER_MODE_END = NEAREST_NEARESTMV,
    SINGLE_INTER_MODE_NUM = SINGLE_INTER_MODE_END - SINGLE_INTER_MODE_START,
    COMP_INTER_MODE_START = NEAREST_NEARESTMV,
    COMP_INTER_MODE_END = MB_MODE_COUNT,
    COMP_INTER_MODE_NUM = COMP_INTER_MODE_END - COMP_INTER_MODE_START,
    INTRA_MODES = PAETH_PRED + 1,  // PAETH_PRED has to be the last intra mode.
    INTRA_INVALID = MB_MODE_COUNT  // For uv_mode in inter blocks
} PredictionMode;

// TODO(ltrudeau) Do we really want to pack this?
// TODO(ltrudeau) Do we match with PredictionMode?
typedef enum ATTRIBUTE_PACKED {
    UV_DC_PRED,        // Average of above and left pixels
    UV_V_PRED,         // Vertical
    UV_H_PRED,         // Horizontal
    UV_D45_PRED,       // Directional 45  degree
    UV_D135_PRED,      // Directional 135 degree
    UV_D113_PRED,      // Directional 113 degree
    UV_D157_PRED,      // Directional 157 degree
    UV_D203_PRED,      // Directional 203 degree
    UV_D67_PRED,       // Directional 67  degree
    UV_SMOOTH_PRED,    // Combination of horizontal and vertical interpolation
    UV_SMOOTH_V_PRED,  // Vertical interpolation
    UV_SMOOTH_H_PRED,  // Horizontal interpolation
    UV_PAETH_PRED,     // Predict from the direction of smallest gradient
    UV_CFL_PRED,       // Chroma-from-Luma
    UV_INTRA_MODES,
    UV_MODE_INVALID,  // For uv_mode in inter blocks
} UvPredictionMode;

typedef enum ATTRIBUTE_PACKED {
    SIMPLE_TRANSLATION,
    OBMC_CAUSAL,    // 2-sided OBMC
    WARPED_CAUSAL,  // 2-sided WARPED
    MOTION_MODES
} MotionMode;

using namespace svt_av1_e2e_tools;

RefDecoder* create_reference_decoder(bool enable_analyzer /* = false*/) {
    RefDecoder::RefDecoderErr ret = RefDecoder::REF_CODEC_OK;
    RefDecoder* decoder = new RefDecoder(ret, enable_analyzer);
    if (decoder && ret != RefDecoder::REF_CODEC_OK) {
        // decoder object is create but init failed
        delete decoder;
        decoder = nullptr;
    }
    return decoder;
}

// callback function to get frame data and mi data
void RefDecoder::inspect_cb(void* pbi, void* data) {
    RefDecoder* pThis = (RefDecoder*)data;
    if (pThis == nullptr)
        return;

    if (!pThis->insp_frame_data_ && pThis->video_param_.width) {
        pThis->insp_frame_data_ = new insp_frame_data();
        if (pThis->insp_frame_data_) {
            ifd_init((insp_frame_data*)pThis->insp_frame_data_,
                     pThis->video_param_.width,
                     pThis->video_param_.height);
        }
    }
    insp_frame_data* inspect_data = (insp_frame_data*)pThis->insp_frame_data_;
    if (!pThis->insp_frame_data_) {
        printf("inspect frame data structure is not ready!\n");
        return;
    }

    /* Fetch frame data. */
    ifd_inspect(inspect_data, pbi);
    pThis->parse_frame_info();
}

// parse the inspect data from decoder to get frame info, and update stream
// info.
void RefDecoder::parse_frame_info() {
    insp_frame_data* inspect_data = (insp_frame_data*)insp_frame_data_;
    ASSERT_NE(inspect_data, nullptr) << "inspection frame data is not ready";

    // get frame info
    stream_info_.tile_cols = inspect_data->tile_mi_cols;
    stream_info_.tile_rows = inspect_data->tile_mi_rows;
    int16_t min_qindex = 255;
    int16_t max_qindex = 0;
    uint32_t min_block_size = 128;
    size_t mi_count = inspect_data->mi_cols * inspect_data->mi_rows;
    for (size_t i = 0; i < mi_count; i++) {
        min_block_size =
            std::min(min_block_size,
                     get_min_block_size(inspect_data->mi_grid[i].sb_type));
        if (!stream_info_.ext_block_flag)
            stream_info_.ext_block_flag =
                is_ext_block(inspect_data->mi_grid[i].sb_type) ? 1 : 0;

        if (inspect_data->mi_grid[i].current_qindex > max_qindex)
            max_qindex = inspect_data->mi_grid[i].current_qindex;
        if (inspect_data->mi_grid[i].current_qindex < min_qindex)
            min_qindex = inspect_data->mi_grid[i].current_qindex;
    }

    // save to frame_type_list
    stream_info_.frame_type_list.push_back(inspect_data->frame_type);

    // update overall stream info
    stream_info_.min_block_size =
        std::min(min_block_size, stream_info_.min_block_size);
    if (min_qindex < stream_info_.min_qindex)
        stream_info_.min_qindex = min_qindex;
    if (max_qindex > stream_info_.max_qindex)
        stream_info_.max_qindex = max_qindex;
    stream_info_.max_intra_period =
        get_max_intra_period_length(stream_info_.frame_type_list);
}

static VideoColorFormat trans_video_format(aom_img_fmt_t fmt) {
    switch (fmt) {
    case AOM_IMG_FMT_YV12: return IMG_FMT_YV12;
    case AOM_IMG_FMT_I420: return IMG_FMT_I420;
    case AOM_IMG_FMT_AOMYV12: return IMG_FMT_YV12_CUSTOM_COLOR_SPACE;
    case AOM_IMG_FMT_AOMI420: return IMG_FMT_I420_CUSTOM_COLOR_SPACE;
    case AOM_IMG_FMT_I422: return IMG_FMT_422;
    case AOM_IMG_FMT_I444: return IMG_FMT_444;
    case AOM_IMG_FMT_444A: return IMG_FMT_444A;
    case AOM_IMG_FMT_I42016: return IMG_FMT_420;
    case AOM_IMG_FMT_I42216: return IMG_FMT_422;
    case AOM_IMG_FMT_I44416: return IMG_FMT_444;
    default: break;
    }
    return IMG_FMT_422;
}

RefDecoder::RefDecoder(RefDecoder::RefDecoderErr& ret, bool enable_analyzer)
    : codec_handle_(new aom_codec_ctx_t()),
      dec_frame_cnt_(0),
      init_timestamp_(0),
      frame_interval_(1),
      insp_frame_data_(nullptr),
      video_param_(VideoFrameParam()),
      parser_(nullptr),
      enc_bytes_(0),
      burst_bytes_(0) {
    aom_codec_ctx_t* codec_ = (aom_codec_ctx_t*)codec_handle_;
    aom_codec_err_t err =
        aom_codec_dec_init(codec_, aom_codec_av1_dx(), nullptr, 0);
    if (err != AOM_CODEC_OK) {
        printf("can not create refernece decoder!!\n");
    }
    ret = (RefDecoderErr)(0 - err);

    // setup parsers including sequence header parser and inspection
    // callback.
    if (enable_analyzer) {
        parser_ = new SequenceHeaderParser();
        if (parser_ == nullptr)
            printf("parser create failed!\n");

        // setup inspection callback
        aom_inspect_init ii;
        ii.inspect_cb = inspect_cb;
        ii.inspect_ctx = this;
        err = aom_codec_control(
            (aom_codec_ctx_t*)codec_handle_, AV1_SET_INSPECTION_CALLBACK, &ii);
        if (err != AOM_CODEC_OK)
            printf("inspection watch create failed!!\n");
    }
}

RefDecoder::~RefDecoder() {
    if (insp_frame_data_) {
        ifd_clear((insp_frame_data*)insp_frame_data_);
        delete (insp_frame_data*)insp_frame_data_;
        insp_frame_data_ = nullptr;
    }
    if (parser_) {
        delete (SequenceHeaderParser*)parser_;
        parser_ = nullptr;
    }

    aom_codec_ctx_t* codec_ = (aom_codec_ctx_t*)codec_handle_;
    aom_codec_destroy(codec_);
    free(codec_handle_);
}

RefDecoder::RefDecoderErr RefDecoder::decode(const uint8_t* data,
                                             const uint32_t size) {
    // send to parser
    if (parser_) {
        ((SequenceHeaderParser*)parser_)
            ->input_obu_data(data, size, &stream_info_);
    }

    aom_codec_ctx_t* codec_ = (aom_codec_ctx_t*)codec_handle_;
    aom_codec_err_t err = aom_codec_decode(codec_, data, size, nullptr);
    if (err != AOM_CODEC_OK) {
        printf("decoder decode error: %d!", err);
        return (RefDecoderErr)(0 - err);
    }
    enc_bytes_ += size;
    burst_bytes_ = std::max(burst_bytes_, size);
    return REF_CODEC_OK;
}

RefDecoder::RefDecoderErr RefDecoder::get_frame(VideoFrame& frame) {
    aom_codec_ctx_t* codec_ = (aom_codec_ctx_t*)codec_handle_;

    aom_image_t* img =
        aom_codec_get_frame(codec_, (aom_codec_iter_t*)&frame.context);
    if (img == nullptr)
        return REF_CODEC_NEED_MORE_INPUT;

    trans_video_frame(img, frame);
    video_param_ = (VideoFrameParam)frame;
    dec_frame_cnt_++;
    stream_info_.frame_bit_rate = enc_bytes_ / dec_frame_cnt_ * 8;
    stream_info_.format = frame.format;
    return REF_CODEC_OK;
}

void RefDecoder::trans_video_frame(const void* image_handle,
                                   VideoFrame& frame) {
    if (image_handle == nullptr)
        return;

    const aom_image_t* image = (const aom_image_t*)image_handle;
    frame.format = trans_video_format(image->fmt);
    frame.width = image->w;
    frame.height = image->h;
    frame.disp_width = image->d_w;
    frame.disp_height = image->d_h;
    memcpy(frame.stride, image->stride, sizeof(frame.stride));
    memcpy(frame.planes, image->planes, sizeof(frame.planes));
    frame.bits_per_sample = image->bit_depth;
    // there is mismatch between "bit_depth" and "fmt", following is a patch
    if (image->fmt & AOM_IMG_FMT_HIGHBITDEPTH)
        frame.bits_per_sample = 10;
    frame.timestamp =
        init_timestamp_ + ((uint64_t)dec_frame_cnt_ * frame_interval_);
}

void RefDecoder::control(int ctrl_id, int arg) {
    const aom_codec_err_t res =
        aom_codec_control_((aom_codec_ctx_t*)codec_handle_, ctrl_id, arg);
    ASSERT_EQ(AOM_CODEC_OK, res) << RefDecoderErr();
}

void RefDecoder::set_invert_tile_decoding_order() {
    this->control(AV1_INVERT_TILE_DECODE_ORDER, 1);
    this->control(AV1_SET_DECODE_TILE_ROW, -1);
    this->control(AV1_SET_DECODE_TILE_COL, -1);
    this->control(AV1_SET_TILE_MODE, 0);
}
