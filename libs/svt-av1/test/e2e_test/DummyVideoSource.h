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
 * @file DummyVideoSource.h
 *
 * @brief A dummy video source for testing different resolution without real
 *        source file.
 *        Dummy source will generate a color bars and keep moving to right side,
 *        FRAME_PER_LOOP defined the frame count in on loop.
 *
 * @author Cidana-Ryan
 *
 ******************************************************************************/
#ifndef _SVT_TEST_DUMMY_VIDEO_SOURCE_H_
#define _SVT_TEST_DUMMY_VIDEO_SOURCE_H_
#include "VideoSource.h"

namespace svt_av1_video_source {

#define FRAME_PER_LOOP 300

static const uint8_t color_bar[3][8] = {
    {255, 173, 131, 115, 96, 83, 34, 0},   /**< luma */
    {128, 52, 161, 87, 151, 80, 214, 128}, /**< cb */
    {128, 137, 34, 45, 198, 205, 134, 128} /**< cr */
};

class DummyVideoSource : public VideoSource {
  public:
    DummyVideoSource(const VideoColorFormat format, const uint32_t width,
                     const uint32_t height, const uint8_t bit_depth)
        : VideoSource(format, width, height, bit_depth) {
        src_name_ = "Dummy Source";
        memset(single_line_pattern, 0, sizeof(single_line_pattern));
    }

    virtual ~DummyVideoSource() {
        for (size_t i = 0; i < 3; i++) {
            if (single_line_pattern[i])
                delete[] single_line_pattern[i];
        }
    }

    EbErrorType open_source(const uint32_t init_pos,
                            const uint32_t frame_count) override {
        cal_yuv_plane_param();

        // create color bar parttern of single line
        const uint32_t luma_size = width_with_padding_ * height_with_padding_;
        for (int i = 0; i < 3; i++) {
            single_line_pattern[i] = new uint8_t[luma_size * bytes_per_sample_];
            if (bytes_per_sample_ > 1)
                create_pattern((uint16_t *)single_line_pattern[i], i);
            else
                create_pattern(single_line_pattern[i], i);
        }

        init_frame_buffer();

        init_pos_ = init_pos;
        if (frame_count == 0)
            frame_count_ = FRAME_PER_LOOP;
        else
            frame_count_ = frame_count;

        current_frame_index_ = -1;
        return EB_ErrorNone;
    }

    /*!\brief Close stream. */
    void close_source() override {
        for (size_t i = 0; i < 3; i++) {
            if (single_line_pattern[i])
                delete[] single_line_pattern[i];
        }
        memset(single_line_pattern, 0, sizeof(single_line_pattern));
        deinit_frame_buffer();
    }

    /*!\brief Get next frame. */
    EbSvtIOFormat *get_next_frame() override {
        if ((uint32_t)(current_frame_index_ + 1) >= frame_count_)
            return nullptr;

        // current_frame_index_ is start from -1, here plus 1 before generate
        generate_frame(current_frame_index_ + 1 + init_pos_);
        current_frame_index_++;
        return frame_buffer_;
    }

    /*!\brief Get frame by index. */
    EbSvtIOFormat *get_frame_by_index(const uint32_t index) override {
        if (index >= frame_count_)
            return nullptr;
        generate_frame(index + init_pos_);
        current_frame_index_ = index;
        return frame_buffer_;
    }

  protected:
    template <typename Sample>
    void create_pattern(Sample *buf, int plane_index) {
        uint32_t bar_width = width_with_padding_ / 8;
        if (plane_index > 0)
            bar_width = bar_width >> width_downsize_;

        for (int i = 0; i < 8; i++) {
            Sample value =
                ((Sample)color_bar[plane_index][i] << (bit_depth_ - 8));
            for (uint32_t c = 0; c < bar_width; c++)
                buf[i * bar_width + c] = value;
        }
    }

    void generate_frame(const uint32_t index) {
        // generate a color bar pattern, moving with FRAME_PER_LOOP frame as
        // one loop.
        uint8_t *src_p = nullptr;

        uint32_t luma_offset =
            (index % FRAME_PER_LOOP) * width_with_padding_ / FRAME_PER_LOOP;
        if (width_downsize_ > 0)
            luma_offset -= luma_offset % 2;
        luma_offset *= bytes_per_sample_;
        const uint32_t width_in_byte = width_with_padding_ * bytes_per_sample_;

        // luma
        src_p = frame_buffer_->luma;
        memcpy(src_p + luma_offset, single_line_pattern[0], width_in_byte);
        memcpy(src_p, src_p + width_in_byte, luma_offset);
        for (uint32_t l = 0; l < height_with_padding_; l++) {
            memcpy(src_p, frame_buffer_->luma, width_in_byte);
            src_p += width_in_byte;
        }

        const uint32_t chroma_offset = luma_offset >> width_downsize_;
        const uint32_t chroma_width_in_byte = width_in_byte >> width_downsize_;
        const uint32_t chroma_height = height_with_padding_ >> height_downsize_;
        // cb
        src_p = frame_buffer_->cb;
        memcpy(src_p + chroma_offset,
               single_line_pattern[1],
               chroma_width_in_byte);
        memcpy(src_p, src_p + chroma_width_in_byte, chroma_offset);
        for (uint32_t l = 0; l < chroma_height; l++) {
            memcpy(src_p, frame_buffer_->cb, chroma_width_in_byte);
            src_p += chroma_width_in_byte;
        }

        // cr
        src_p = frame_buffer_->cr;
        memcpy(src_p + chroma_offset,
               single_line_pattern[2],
               chroma_width_in_byte);
        memcpy(src_p, src_p + chroma_width_in_byte, chroma_offset);
        for (uint32_t l = 0; l < chroma_height; l++) {
            memcpy(src_p, frame_buffer_->cr, chroma_width_in_byte);
            src_p += chroma_width_in_byte;
        }

        frame_size_ = (width_in_byte * height_with_padding_) +
                      2 * (chroma_width_in_byte * chroma_height);
    }

  protected:
    uint8_t *single_line_pattern[3];
};
}  // namespace svt_av1_video_source
#endif  //_SVT_TEST_DUMMY_VIDEO_SOURCE_H_
