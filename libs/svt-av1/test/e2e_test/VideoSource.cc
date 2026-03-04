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
 * @file VideoSource.cc
 *
 * @brief Impelmentation class for video source & video file source.
 *
 * @author Cidana-Ryan
 *
 ******************************************************************************/
// for getenv on windows
#if defined(_WIN32) && !defined(_CRT_SECURE_NO_WARNINGS)
#define _CRT_SECURE_NO_WARNINGS
#endif
#include <stdio.h>
#include "definitions.h"
#include "VideoSource.h"
#include "random.h"

using namespace svt_av1_video_source;
VideoSource::VideoSource(const VideoColorFormat format, const uint32_t width,
                         const uint32_t height, const uint8_t bit_depth)
    : src_name_("Unknown Source"),
      width_(width),
      height_(height),
      width_with_padding_(width),
      height_with_padding_(height),
      bit_depth_(bit_depth),
      init_pos_(0),
      frame_count_(0),
      current_frame_index_(-1),
      frame_size_(0),
      frame_buffer_(nullptr),
      image_format_(format),
      width_downsize_(0),
      height_downsize_(0),
      bytes_per_sample_(1),
      blank_frame_interval_(0) {
    frame_qp_list_.clear();
}

VideoSource::~VideoSource() {
}

bool VideoSource::is_10bit_mode() {
    return bit_depth_ == 10;
}

void VideoSource::cal_yuv_plane_param() {
    // Determine size of each plane
    switch (image_format_) {
    case IMG_FMT_420P10_PACKED:
    case IMG_FMT_420:
        width_downsize_ = 1;
        height_downsize_ = 1;
        break;
    case IMG_FMT_422P10_PACKED:
    case IMG_FMT_422:
        width_downsize_ = 1;
        height_downsize_ = 0;
        break;
    case IMG_FMT_444P10_PACKED:
    case IMG_FMT_444:
        width_downsize_ = 0;
        height_downsize_ = 0;
        break;
    default: assert(0); break;
    }
    bytes_per_sample_ = (bit_depth_ > 8) ? 2 : 1;

    // encoder required 16 pixel alighed in width and height
    if (width_ % 16 != 0)
        width_with_padding_ = ((width_ >> 4) + 1) << 4;
    if (height_ % 16 != 0)
        height_with_padding_ = ((height_ >> 4) + 1) << 4;
}

void VideoSource::deinit_frame_buffer() {
    if (frame_buffer_ == nullptr)
        return;

    if (frame_buffer_->luma != nullptr) {
        free(frame_buffer_->luma);
        frame_buffer_->luma = nullptr;
    }
    if (frame_buffer_->cb != nullptr) {
        free(frame_buffer_->cb);
        frame_buffer_->cb = nullptr;
    }
    if (frame_buffer_->cr != nullptr) {
        free(frame_buffer_->cr);
        frame_buffer_->cr = nullptr;
    }

    free(frame_buffer_);
    frame_buffer_ = nullptr;
    frame_size_ = 0;
}

EbErrorType VideoSource::init_frame_buffer() {
    uint32_t luma_size =
        width_with_padding_ * height_with_padding_ * bytes_per_sample_;
    uint32_t chroma_size = luma_size >> (width_downsize_ + height_downsize_);

    // Determine
    if (frame_buffer_ == nullptr) {
        frame_buffer_ = (EbSvtIOFormat *)malloc(sizeof(EbSvtIOFormat));
        if (frame_buffer_ == nullptr)
            return EB_ErrorInsufficientResources;
    }

    memset(frame_buffer_, 0, sizeof(EbSvtIOFormat));
    // SVT-AV1 use pixel size as stride?
    frame_buffer_->y_stride = width_with_padding_;
    frame_buffer_->cb_stride = (width_with_padding_ >> width_downsize_);
    frame_buffer_->cr_stride = (width_with_padding_ >> width_downsize_);

    // Y
    frame_buffer_->luma = (uint8_t *)malloc(luma_size);
    if (frame_buffer_->luma == nullptr) {
        deinit_frame_buffer();
        return EB_ErrorInsufficientResources;
    }
    memset(frame_buffer_->luma, 0, luma_size);

    // Cb
    frame_buffer_->cb = (uint8_t *)malloc(chroma_size);
    if (frame_buffer_->cb == nullptr) {
        deinit_frame_buffer();
        return EB_ErrorInsufficientResources;
    }
    memset(frame_buffer_->cb, 0, chroma_size);

    // Cr
    frame_buffer_->cr = (uint8_t *)malloc(chroma_size);
    if (frame_buffer_->cr == nullptr) {
        deinit_frame_buffer();
        return EB_ErrorInsufficientResources;
    }
    memset(frame_buffer_->cr, 0, chroma_size);

    return EB_ErrorNone;
}

VideoFileSource::VideoFileSource(const std::string &file_name,
                                 const VideoColorFormat format,
                                 const uint32_t width, const uint32_t height,
                                 const uint8_t bit_depth)
    : VideoSource(format, width, height, bit_depth),
      file_name_(file_name),
      file_handle_(nullptr),
      file_length_(0),
      frame_length_(0),
      file_frames_(0) {
#ifdef ENABLE_DEBUG_MONITOR
    monitor = nullptr;
#endif
}

VideoFileSource::~VideoFileSource() {
#ifdef ENABLE_DEBUG_MONITOR
    if (monitor != nullptr)
        delete monitor;
#endif
}

/**
 * @brief      Use this function to get vector path defined by envrionment
 * variable SVT_AV1_TEST_VECTOR_PATH, or it will return a default path.
 *
 * @return     The vectors path.
 */
std::string VideoFileSource::get_vector_dir() {
    const char *const path = getenv("SVT_AV1_TEST_VECTOR_PATH");
    if (path == nullptr) {
#ifdef _WIN32
        return "../../../../test/vectors";
#else
        return "../../test/vectors";
#endif  // _WIN32
    }
    return path;
}

uint32_t VideoFileSource::read_input_frame() {
    uint32_t filled_len = 0;
    if (file_handle_ == nullptr) {
        printf("Error file handle\r\n");
        return 0;
    }

    if (feof(file_handle_) != 0) {
        printf("Reach file end\r\n");
        return 0;
    }

    // Read raw data from file
    size_t read_len = 0;
    uint32_t i;
    {
        uint8_t *eb_input_ptr = nullptr;
        // Y
        eb_input_ptr = frame_buffer_->luma;
        for (i = 0; i < height_; ++i) {
            read_len = fread(
                eb_input_ptr, 1, width_ * bytes_per_sample_, file_handle_);
            if (read_len != width_ * bytes_per_sample_)
                return 0;  // read file error.

            eb_input_ptr += frame_buffer_->y_stride * bytes_per_sample_;
            filled_len += frame_buffer_->y_stride * bytes_per_sample_;
        }

        // Cb
        eb_input_ptr = frame_buffer_->cb;
        for (i = 0; i < (height_ >> height_downsize_); ++i) {
            read_len = fread(eb_input_ptr,
                             1,
                             (width_ >> width_downsize_) * bytes_per_sample_,
                             file_handle_);
            if (read_len != (width_ >> width_downsize_) * bytes_per_sample_)
                return 0;  // read file error.

            eb_input_ptr += frame_buffer_->cb_stride * bytes_per_sample_;
            filled_len += frame_buffer_->cb_stride * bytes_per_sample_;
        }

        // Cr
        eb_input_ptr = frame_buffer_->cr;

        for (i = 0; i < (height_ >> height_downsize_); ++i) {
            read_len = fread(eb_input_ptr,
                             1,
                             (width_ >> width_downsize_) * bytes_per_sample_,
                             file_handle_);
            if (read_len != (width_ >> width_downsize_) * bytes_per_sample_)
                return 0;  // read file error.

            eb_input_ptr += frame_buffer_->cr_stride * bytes_per_sample_;
            filled_len += frame_buffer_->cr_stride * bytes_per_sample_;
        }
    }

    if (blank_frame_interval_ > 0 &&
        current_frame_index_ % blank_frame_interval_ == 0) {
        memset(frame_buffer_->luma,
               0,
               frame_buffer_->y_stride * bytes_per_sample_ * height_);
        memset(frame_buffer_->cb,
               0,
               frame_buffer_->cb_stride * bytes_per_sample_ *
                   (height_ >> height_downsize_));
        memset(frame_buffer_->cr,
               0,
               frame_buffer_->cr_stride * bytes_per_sample_ *
                   (height_ >> height_downsize_));
    }

    return filled_len;
}

/*!\brief Prepare stream. */
EbErrorType VideoFileSource::open_source(const uint32_t init_pos,
                                         const uint32_t frame_count) {
    EbErrorType return_error = EB_ErrorNone;
    if (file_handle_ != nullptr)
        return EB_ErrorNone;
    std::string full_path = get_vector_dir() + "/" + file_name_.c_str();

    FOPEN(file_handle_, full_path.c_str(), "rb");
    if (file_handle_ == nullptr) {
        printf(">>> Open video source %s failed!\r\n", full_path.c_str());
        printf(
            "    You can use CMake generated build target TestVectors to get "
            "correct test vectors before run this test.\r\n");
        printf(
            "    For unix like system, run 'make TestVectors', or build "
            "TestVectors project on "
            "VisualStudio.\r\n");
        return EB_ErrorBadParameter;
    }

    // Seek to begin
    fseek(file_handle_, 0, SEEK_SET);

    // Get file info before prepare buffer
    return_error = parse_file_info();
    if (return_error != EB_ErrorNone) {
        fclose(file_handle_);
        file_handle_ = nullptr;
        return return_error;
    }

    // Prepare buffer
    if (EB_ErrorNone != init_frame_buffer()) {
        fclose(file_handle_);
        file_handle_ = nullptr;
        return EB_ErrorInsufficientResources;
    }
    if (init_pos_ < file_frames_)
        init_pos_ = init_pos;
    else
        init_pos_ = init_pos_ % file_frames_;
    if (frame_count == 0)
        frame_count_ = file_frames_ - init_pos_;
    else
        frame_count_ = frame_count;

    // generate frame qp from random if failed from file
    if (true /* TODO: first from qp file */) {
        svt_av1_test_tool::SVTRandom rnd(1, 63);
        for (uint32_t i = 0; i < frame_count_; i++) {
            frame_qp_list_.push_back(rnd.random());
        }
    }

    if (seek_to_frame(init_pos_) != EB_ErrorNone) {
        fclose(file_handle_);
        file_handle_ = nullptr;
        return EB_ErrorInsufficientResources;
    }

    current_frame_index_ = -1;
#ifdef ENABLE_DEBUG_MONITOR
    if (monitor == nullptr) {
        monitor = new VideoMonitor(
            width_with_padding_,
            height_with_padding_,
            (bit_depth_ > 8) ? width_with_padding_ * 2 : width_with_padding_,
            bit_depth_,
            "Y4M Source");
    }
#endif
    return EB_ErrorNone;
}
/*!\brief Close stream. */
void VideoFileSource::close_source() {
    if (file_handle_) {
        fclose(file_handle_);
        file_handle_ = nullptr;
    }

#ifdef ENABLE_DEBUG_MONITOR
    if (monitor) {
        delete monitor;
        monitor = nullptr;
    }
#endif
    deinit_frame_buffer();
    init_pos_ = 0;
    frame_count_ = 0;
    file_frames_ = 0;
}

EbSvtIOFormat *VideoFileSource::get_frame_by_index(const uint32_t index) {
    if (index >= frame_count_)
        return nullptr;

    // Seek to frame by index
    if (seek_to_frame(index) != EB_ErrorNone)
        return nullptr;

    frame_size_ = read_input_frame();
    if (frame_size_ == 0)
        return nullptr;

    current_frame_index_ = index;
#ifdef ENABLE_DEBUG_MONITOR
    if (monitor != nullptr) {
        monitor->draw_frame(
            frame_buffer_->luma, frame_buffer_->cb, frame_buffer_->cr);
    }
#endif
    return frame_buffer_;
}

EbSvtIOFormat *VideoFileSource::get_next_frame() {
    // printf("Get Next Frame:%d\r\n", current_frame_index_ + 1);
    if ((uint32_t)(current_frame_index_ + 1) >= frame_count_)
        return nullptr;

    if (init_pos_ + (current_frame_index_ + 1) >= file_frames_ &&
        current_frame_index_ != -1) {
        // Seek to frame by index
        if (seek_to_frame(current_frame_index_ + 1) != EB_ErrorNone)
            return nullptr;
    }

    frame_size_ = read_input_frame();
    if (frame_size_ == 0)
        return nullptr;
    ++current_frame_index_;

#ifdef ENABLE_DEBUG_MONITOR
    if (monitor != nullptr) {
        monitor->draw_frame(
            frame_buffer_->luma, frame_buffer_->cb, frame_buffer_->cr);
    }
#endif
    return frame_buffer_;
}
