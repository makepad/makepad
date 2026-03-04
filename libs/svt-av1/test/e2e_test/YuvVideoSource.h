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
 * @file YuvVideoSource.h
 *
 * @brief YuvVideoSource class for reading yuv file.
 *
 * @author Cidana-Ryan
 *
 ******************************************************************************/
#ifndef _SVT_TEST_YUV_VIDEO_SOURCE_H_
#define _SVT_TEST_YUV_VIDEO_SOURCE_H_
#include "VideoSource.h"
namespace svt_av1_video_source {
class YuvVideoSource : public VideoFileSource {
  public:
    YuvVideoSource(const std::string &file_name, const VideoColorFormat format,
                   const uint32_t width, const uint32_t height,
                   const uint8_t bit_depth);
    ~YuvVideoSource();

  protected:
    EbErrorType parse_file_info() override;
    EbErrorType seek_to_frame(const uint32_t index) override;
};
}  // namespace svt_av1_video_source
#endif  //_SVT_TEST_VIDEO_SOURCE_H_
