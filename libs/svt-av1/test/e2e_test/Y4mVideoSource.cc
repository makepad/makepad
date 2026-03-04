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
 * @file Y4mVideoSource.h
 *
 * @brief Impelmentation of Y4mVideoSource class for reading y4m file.
 *
 * @author Cidana-Ryan
 *
 ******************************************************************************/
#include <memory.h>
#include <stdlib.h>
#include <assert.h>
#include "Y4mVideoSource.h"
extern "C" {
#include "app_input_y4m.h"
}
using namespace svt_av1_video_source;

static const uint32_t Y4M_FRAME_HEADER_LEN = 6;
static const char Y4M_FRAME_HEADER[Y4M_FRAME_HEADER_LEN + 1] = {
    'F', 'R', 'A', 'M', 'E', 0x0A, 0};

Y4MVideoSource::Y4MVideoSource(const std::string& file_name,
                               const VideoColorFormat format,
                               const uint32_t width, const uint32_t height,
                               const uint8_t bit_depth)
    : VideoFileSource(file_name, format, width, height, bit_depth) {
    src_name_ = "Y4M Source";
    header_length_ = 0;
}

Y4MVideoSource::~Y4MVideoSource() {
}

EbErrorType Y4MVideoSource::parse_file_info() {
    if (file_handle_ == nullptr)
        return EB_ErrorBadParameter;

    // Get file length
    fseek(file_handle_, 0, SEEK_END);
    file_length_ = ftell(file_handle_);

    // Seek to begin
    fseek(file_handle_, 0, SEEK_SET);

    EbConfig cfg{};
    cfg.input_file = file_handle_;
    if (check_if_y4m(&cfg) != true)
        return EB_ErrorBadParameter;
    read_y4m_header(&cfg);

    width_ = cfg.config.source_width;
    height_ = cfg.config.source_height;
    bit_depth_ = cfg.config.encoder_bit_depth;

    // y4m video source use the color format type from test vector param for
    // "read_y4m_header" does not output color format info

    cal_yuv_plane_param();

    // Get header length
    header_length_ = ftell(file_handle_);

    uint32_t luma_size = width_ * height_ * bytes_per_sample_;
    frame_length_ = Y4M_FRAME_HEADER_LEN + luma_size +
                    ((luma_size >> (width_downsize_ + height_downsize_)) * 2);

    // Calculate frame count
    file_frames_ = (file_length_ - header_length_) / frame_length_;

    return EB_ErrorNone;
}

uint32_t Y4MVideoSource::read_input_frame() {
    char frame_header[Y4M_FRAME_HEADER_LEN] = {0};
    if (file_handle_ == nullptr) {
        printf("Error file handle\r\n");
        return 0;
    }

    if (feof(file_handle_) != 0) {
        printf("Reach file end\r\n");
        return 0;
    }

    if (Y4M_FRAME_HEADER_LEN !=
        fread(frame_header, 1, Y4M_FRAME_HEADER_LEN, file_handle_)) {
        printf("can not found frame header\r\n");
        return 0;
    }

    // Check frame header
    if (strncmp(Y4M_FRAME_HEADER, frame_header, Y4M_FRAME_HEADER_LEN)) {
        printf("Read frame error\n");
        return 0;
    }
    return VideoFileSource::read_input_frame();
}

EbErrorType Y4MVideoSource::seek_to_frame(const uint32_t index) {
    if (file_handle_ == nullptr) {
        printf("Error file handle\r\n");
        return EB_ErrorInsufficientResources;
    }
    uint32_t real_index = init_pos_ + index;
    if (real_index >= file_frames_)
        real_index = real_index % file_frames_;
    if (fseek(file_handle_,
              real_index * frame_length_ + header_length_,
              SEEK_SET) != 0)
        return EB_ErrorInsufficientResources;
    return EB_ErrorNone;
}
