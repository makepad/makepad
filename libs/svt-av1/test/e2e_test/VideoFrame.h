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
 * @file VideoFrame.h
 *
 * @brief Defines video frame types and structure
 *
 * @author Cidana-Edmond
 *
 ******************************************************************************/
#ifndef _SVT_TEST_VIDEO_FRAME_H_
#define _SVT_TEST_VIDEO_FRAME_H_

#include <memory.h>
#include <stdint.h>
#include <stdio.h>
#include <assert.h>

#define INVALID_QP (0xFF)

#ifndef PLANE_NUM
#define PLANE_NUM (4)
#define PLANE_Y (0)
#define PLANE_U (1)
#define PLANE_V (2)
#define PLANE_A (3)
#endif  // !PLANE_NUM

/** VideoColorFormat defines the format of YUV video */
typedef enum VideoColorFormat {
    IMG_FMT_YV12,
    IMG_FMT_420 = IMG_FMT_YV12,
    IMG_FMT_422,
    IMG_FMT_444,
    IMG_FMT_420P10_PACKED,
    IMG_FMT_422P10_PACKED,
    IMG_FMT_444P10_PACKED,
    IMG_FMT_I420,
    IMG_FMT_YV12_CUSTOM_COLOR_SPACE,
    IMG_FMT_I420_CUSTOM_COLOR_SPACE,
    IMG_FMT_444A,
} VideoColorFormat;

/** VideoFrameParam defines the basic parameters of video frame */
typedef struct VideoFrameParam {
    VideoColorFormat format;  /**< video format type */
    uint32_t width;           /**< width of video frame in pixel */
    uint32_t height;          /**< height of video frame in pixel */
    uint32_t bits_per_sample; /**< bit for each sample, default is 8, more for
                                 packed formats */
    VideoFrameParam() {
        format = IMG_FMT_YV12;
        width = 0;
        height = 0;
        bits_per_sample = 8;
    }
} VideoFrameParam;

/** VideoFrame defines the full parameters of video frame */
typedef struct VideoFrame : public VideoFrameParam {
    uint32_t disp_width;        /**< width to display*/
    uint32_t disp_height;       /**< height to display*/
    uint32_t stride[PLANE_NUM]; /**< stride of bytes in array */
    uint8_t *planes[PLANE_NUM]; /**< plane pointer to buffer address*/
    void *context;              /**< private data context from creator */
    uint64_t timestamp;         /**< timestamp(index) of this frame */
    uint8_t *buffer;            /**< self own buffer */
    uint32_t buf_size;          /**< buffer size in bytes */
    uint32_t qp;                /**< qp of this frame */
    bool compress_10bit;        /**< compressed 10-bit mode, not supported */
    uint8_t *ext_planes[PLANE_NUM]; /**< extended plane pointer of packed
                               10-bit, not supported */

    VideoFrame() {
        disp_width = 0;
        disp_height = 0;
        memset(stride, 0, sizeof(stride));
        memset(planes, 0, sizeof(planes));
        memset(ext_planes, 0, sizeof(ext_planes));
        context = nullptr;
        timestamp = 0;
        buffer = nullptr;
        buf_size = 0;
        qp = INVALID_QP;
        compress_10bit = false;
    }
    explicit VideoFrame(const VideoFrameParam &param) {
        // copy basic info from param
        *(VideoFrameParam *)this = param;
        disp_width = param.width;
        disp_height = param.height;
        memset(stride, 0, sizeof(stride));
        memset(planes, 0, sizeof(planes));
        memset(ext_planes, 0, sizeof(ext_planes));
        bits_per_sample = param.bits_per_sample;
        context = nullptr;
        timestamp = 0;
        buffer = nullptr;
        buf_size = 0;
        qp = INVALID_QP;
        switch (format) {
        case IMG_FMT_420P10_PACKED:
        case IMG_FMT_422P10_PACKED:
        case IMG_FMT_444P10_PACKED: compress_10bit = true; break;
        default: compress_10bit = false; break;
        }

        // allocate memory for new frame
        uint32_t max_size = calculate_max_frame_size(param);
        buffer = new uint8_t[max_size];
        if (buffer) {
            memset(buffer, 0, max_size);
            planes[PLANE_Y] = buffer;
            planes[PLANE_U] = buffer + (max_size >> 2);
            planes[PLANE_V] = buffer + (max_size >> 1);
            planes[PLANE_A] = buffer + (max_size * 3 / 4);
            calculate_strides(param, stride);
            buf_size = max_size;
        } else
            printf("video frame buffer is out of memory!!\n");
    }
    VideoFrame &operator=(const VideoFrame &other) {
        if (this != &other) {
            // copy information from other
            *static_cast<VideoFrameParam *>(this) =
                *static_cast<const VideoFrameParam *>(&other);
            disp_width = other.disp_width;
            disp_height = other.disp_height;
            memcpy(stride, other.stride, sizeof(stride));
            memcpy(planes, other.planes, sizeof(planes));
            context = other.context;
            timestamp = other.timestamp;
            buffer = other.buffer;
            buf_size = other.buf_size;
            qp = other.qp;
            compress_10bit = other.compress_10bit;
            memcpy(ext_planes, other.ext_planes, sizeof(ext_planes));
        }
        return *this;
    };
    VideoFrame(const VideoFrame &origin) {
        // copy information from origin
        *this = origin;

        // calculate height downsize form format
        uint32_t height_downsize = 0;
        if (format == IMG_FMT_420 || format == IMG_FMT_420P10_PACKED)
            height_downsize = 1;

        /** create video frame buffer with maximun size in 4 planes */
        const uint32_t max_size = calculate_max_frame_size(origin);
        buffer = new uint8_t[max_size];
        if (buffer) {
            memset(buffer, 0, max_size);
            for (size_t i = 0; i < PLANE_NUM; ++i) {
                if (origin.planes[i]) {
                    planes[i] = buffer + (i * max_size / PLANE_NUM);
                    const uint32_t plane_size =
                        stride[i] * ((i == PLANE_Y || i == PLANE_A)
                                         ? height
                                         : (height >> height_downsize));
                    memcpy(planes[i], origin.planes[i], plane_size);
                }
            }
            buf_size = max_size;
        } else
            printf("video frame buffer is out of memory!!\n");
    }
    ~VideoFrame() {
        if (buf_size) {
            delete[] buffer;
            buffer = nullptr;
            memset(&planes, 0, sizeof(planes));
            buf_size = 0;
        }
    }
    /** Trim video frame buffer size for memory useage */
    void trim_buffer() {
        if (buf_size) {
            delete[] buffer;
            buffer = nullptr;
            buf_size = 0;
            memset(planes, 0, sizeof(planes));
        }
    }
    static uint32_t calculate_luma_size(const VideoFrameParam &param) {
        return param.width * param.height * (param.bits_per_sample > 8 ? 2 : 1);
    }
    static uint32_t calculate_max_frame_size(const VideoFrameParam &param) {
        return PLANE_NUM * calculate_luma_size(param);
    }
    static uint32_t calculate_max_frame_size(const VideoFrame &frame) {
        uint32_t luma_size = frame.stride[PLANE_Y] * frame.height;
        return PLANE_NUM * luma_size;
    }
    static void calculate_strides(const VideoFrameParam &param,
                                  uint32_t strides[PLANE_NUM]) {
        strides[PLANE_Y] = param.width;
        strides[PLANE_A] = 0;
        switch (param.format) {
        case IMG_FMT_420:
        case IMG_FMT_420P10_PACKED:
        case IMG_FMT_I420:
        case IMG_FMT_YV12_CUSTOM_COLOR_SPACE:
        case IMG_FMT_I420_CUSTOM_COLOR_SPACE:
        case IMG_FMT_422:
        case IMG_FMT_422P10_PACKED:
            strides[PLANE_U] = strides[PLANE_V] = param.width >> 1;
            break;
        case IMG_FMT_444A: strides[PLANE_A] = param.width;
        // fall through
        case IMG_FMT_444:
        case IMG_FMT_444P10_PACKED:
            strides[PLANE_U] = strides[PLANE_V] = param.width;
            break;
        default: assert(0); break;
        }
        if (param.bits_per_sample > 8) {
            // this strides are the strides in bytes
            strides[PLANE_Y] *= 2;
            strides[PLANE_U] *= 2;
            strides[PLANE_V] *= 2;
            strides[PLANE_A] *= 2;
        }
    }

} VideoFrame;

#endif  //_SVT_TEST_VIDEO_FRAME_H_
