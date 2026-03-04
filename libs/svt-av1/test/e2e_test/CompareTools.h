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
 * @file CompareTools.h
 *
 * @brief Defines a tool set for compare recon frame and decoded frame.
 *
 * @author Cidana-Ryan Cidana-Edmond
 *
 ******************************************************************************/

#ifndef _COMPARE_TOOLS_H_
#define _COMPARE_TOOLS_H_

#include <stdint.h>
#include <math.h>
#include <float.h>
#include "FrameQueue.h"
#include "EbSvtAv1Enc.h"

namespace svt_av1_e2e_tools {
static inline bool compare_image(const VideoFrame *recon,
                                 const VideoFrame *ref_frame) {
    if (recon->disp_width != ref_frame->disp_width ||
        recon->disp_height != ref_frame->disp_height) {
        printf("compare failed for width(%u--%u) or height(%u--%u) different\n",
               recon->disp_width,
               ref_frame->disp_width,
               recon->disp_height,
               ref_frame->disp_height);
        return false;
    }

    if (recon->format != ref_frame->format) {
        printf("compare failed for format(%u--%u) different\n",
               recon->format,
               ref_frame->format);
        return false;
    }

    uint32_t subsample_x = 0;
    uint32_t subsample_y = 0;
    switch (recon->format) {
    case IMG_FMT_420P10_PACKED:
    case IMG_FMT_420:
        subsample_x = 1;
        subsample_y = 1;
        break;
    case IMG_FMT_422P10_PACKED:
    case IMG_FMT_422: subsample_x = 1; break;
    case IMG_FMT_444P10_PACKED:
    case IMG_FMT_444: break;
    default: assert(0); break;
    }

    uint32_t width = recon->disp_width;
    uint32_t height = recon->disp_height;

    // luma
    for (uint32_t l = 0; l < height; l++) {
        const uint8_t *s = recon->planes[0] + l * recon->stride[0];
        const uint8_t *d = ref_frame->planes[0] + l * ref_frame->stride[0];
        for (uint32_t r = 0; r < width; r++) {
            const uint16_t s_pixel = recon->bits_per_sample == 8
                                         ? s[r]
                                         : (((uint16_t *)s)[r] & 0x3FF);
            const uint16_t d_pixel = ref_frame->bits_per_sample == 8
                                         ? d[r]
                                         : (((uint16_t *)d)[r] & 0x3FF);
            if (s_pixel != d_pixel) {
                printf("pixel index(%u--%u) luma compare failed!\n", l, r);
                return false;
            }
        }
    }

    // cb
    for (uint32_t l = 0; l < (height + subsample_y) >> subsample_y; l++) {
        const uint8_t *s = recon->planes[1] + l * recon->stride[1];
        const uint8_t *d = ref_frame->planes[1] + l * ref_frame->stride[1];
        for (uint32_t r = 0; r < (width + subsample_x) >> subsample_x; r++) {
            const uint16_t s_pixel = recon->bits_per_sample == 8
                                         ? s[r]
                                         : (((uint16_t *)s)[r] & 0x3FF);
            const uint16_t d_pixel = ref_frame->bits_per_sample == 8
                                         ? d[r]
                                         : (((uint16_t *)d)[r] & 0x3FF);
            if (s_pixel != d_pixel) {
                printf("pixel index(%u--%u) cb compare failed!\n", l, r);
                return false;
            }
        }
    }

    // cr
    for (uint32_t l = 0; l < (height + subsample_y) >> subsample_y; l++) {
        const uint8_t *s = recon->planes[2] + l * recon->stride[2];
        const uint8_t *d = ref_frame->planes[2] + l * ref_frame->stride[2];
        for (uint32_t r = 0; r < (width + subsample_x) >> subsample_x; r++) {
            const uint16_t s_pixel = recon->bits_per_sample == 8
                                         ? s[r]
                                         : (((uint16_t *)s)[r] & 0x3FF);
            const uint16_t d_pixel = ref_frame->bits_per_sample == 8
                                         ? d[r]
                                         : (((uint16_t *)d)[r] & 0x3FF);
            if (s_pixel != d_pixel) {
                printf("pixel index(%u--%u) cr compare failed!\n", l, r);
                return false;
            }
        }
    }

    return true;
}

static inline double psnr_8bit(const uint8_t *p1, const uint8_t *p2,
                               const uint32_t size) {
    // Assert that, p1 p2 hase same size and no stirde issue.
    double mse = 0.1; /* avoid NaN issue */
    for (uint32_t i = 0; i < size; i++) {
        const uint8_t I = p1[i];
        const uint8_t K = p2[i];
        const int32_t diff = I - K;
        mse += (double)diff * diff;
    }
    mse /= size;

    double psnr = INFINITY;
    if (DBL_EPSILON < mse) {
        psnr = 10 * log10(((double)255 * 255) / mse);
    }
    return psnr;
}

static inline double psnr_8bit(const uint8_t *p1, const uint32_t stride1,
                               const uint8_t *p2, const uint32_t stride2,
                               const uint32_t width, const uint32_t height) {
    double mse = 0.1; /* avoid NaN issue */
    for (size_t y = 0; y < height; y++) {
        const uint8_t *s = p1 + (y * stride1);
        const uint8_t *d = p2 + (y * stride2);
        for (size_t x = 0; x < width; x++) {
            const int32_t diff = s[x] - d[x];
            mse += (double)diff * diff;
        }
    }
    mse /= (double)width * height;

    double psnr = INFINITY;
    if (DBL_EPSILON < mse) {
        psnr = 10 * log10(((double)255 * 255) / mse);
    }
    return psnr;
}

static inline double psnr_8bit_10bit(const uint8_t *p1, const uint32_t stride1,
                                     const uint16_t *p2, const uint32_t stride2,
                                     const uint32_t width,
                                     const uint32_t height) {
    double mse = 0.1; /* avoid NaN issue */
    for (size_t y = 0; y < height; y++) {
        const uint8_t *s = p1 + (y * stride1);
        const uint16_t *d = p2 + (y * stride2 / 2);
        for (size_t x = 0; x < width; x++) {
            const int32_t diff = s[x] - (d[x] & 0xFF);
            mse += (double)diff * diff;
        }
    }
    mse /= (double)width * height;

    double psnr = INFINITY;
    if (DBL_EPSILON < mse) {
        psnr = 10 * log10(((double)255 * 255) / mse);
    }
    return psnr;
}

static inline double psnr_10bit(const uint16_t *p1, const uint16_t *p2,
                                const uint32_t size) {
    // Assert that, p1 p2 hase same size and no stirde issue.
    double mse = 0.1; /* vaoid NaN issue */
    for (uint32_t i = 0; i < size; i++) {
        const uint16_t I = p1[i] & 0x3FF;
        const uint16_t K = p2[i] & 0x3FF;
        const int32_t diff = I - K;
        mse += (double)diff * diff;
    }
    mse /= size;

    double psnr = INFINITY;

    if (DBL_EPSILON < mse) {
        psnr = 10 * log10(((double)1023 * 1023) / mse);
    }
    return psnr;
}

static inline double psnr_10bit(const uint16_t *p1, const uint32_t stride1,
                                const uint16_t *p2, const uint32_t stride2,
                                const uint32_t width, const uint32_t height) {
    double mse = 0.1;
    for (size_t y = 0; y < height; y++) {
        const uint16_t *s = p1 + (y * stride1);
        const uint16_t *d = p2 + (y * stride2);
        for (size_t x = 0; x < width; x++) {
            const int32_t diff = (s[x] & 0x3FF) - (d[x] & 0x3FF);
            mse += (double)diff * diff;
        }
    }
    mse /= (double)width * height;

    double psnr = INFINITY;
    if (DBL_EPSILON < mse) {
        psnr = 10 * log10(((double)1023 * 1023) / mse);
    }
    return psnr;
}

static inline void psnr_frame(const EbSvtIOFormat *src_frame,
                              const uint32_t src_bit_depth,
                              const VideoFrame &frame, double &luma_psnr,
                              double &cb_psnr, double &cr_psnr) {
    if (src_bit_depth == 8) {
        if (frame.bits_per_sample == 8) {
            luma_psnr = psnr_8bit(src_frame->luma,
                                  src_frame->y_stride,
                                  frame.planes[0],
                                  frame.stride[0],
                                  frame.width,
                                  frame.height);
            cb_psnr = psnr_8bit(src_frame->cb,
                                src_frame->cb_stride,
                                frame.planes[1],
                                frame.stride[1],
                                frame.width >> 1,
                                frame.height >> 1);
            cr_psnr = psnr_8bit(src_frame->cr,
                                src_frame->cr_stride,
                                frame.planes[2],
                                frame.stride[2],
                                frame.width >> 1,
                                frame.height >> 1);
        } else {
            luma_psnr = psnr_8bit_10bit(src_frame->luma,
                                        src_frame->y_stride,
                                        (uint16_t *)frame.planes[0],
                                        frame.stride[0],
                                        frame.width,
                                        frame.height);
            cb_psnr = psnr_8bit_10bit(src_frame->cb,
                                      src_frame->cb_stride,
                                      (uint16_t *)frame.planes[1],
                                      frame.stride[1],
                                      frame.width >> 1,
                                      frame.height >> 1);
            cr_psnr = psnr_8bit_10bit(src_frame->cr,
                                      src_frame->cr_stride,
                                      (uint16_t *)frame.planes[2],
                                      frame.stride[2],
                                      frame.width >> 1,
                                      frame.height >> 1);
        }
    }
    if (src_bit_depth == 10) {
        if (frame.bits_per_sample == 8) {
            luma_psnr = psnr_8bit_10bit(frame.planes[0],
                                        frame.stride[0],
                                        (uint16_t *)src_frame->luma,
                                        src_frame->y_stride,
                                        frame.width,
                                        frame.height);
            cb_psnr = psnr_8bit_10bit(frame.planes[1],
                                      frame.stride[1],
                                      (uint16_t *)src_frame->cb,
                                      src_frame->cb_stride,
                                      frame.width >> 1,
                                      frame.height >> 1);
            cr_psnr = psnr_8bit_10bit(frame.planes[2],
                                      frame.stride[2],
                                      (uint16_t *)src_frame->cr,
                                      src_frame->cr_stride,
                                      frame.width >> 1,
                                      frame.height >> 1);
        } else {
            luma_psnr = psnr_10bit((const uint16_t *)src_frame->luma,
                                   src_frame->y_stride,
                                   (const uint16_t *)frame.planes[0],
                                   frame.stride[0] / 2,
                                   frame.width,
                                   frame.height);
            cb_psnr = psnr_10bit((const uint16_t *)src_frame->cb,
                                 src_frame->cb_stride,
                                 (const uint16_t *)frame.planes[1],
                                 frame.stride[1] / 2,
                                 frame.width >> 1,
                                 frame.height >> 1);
            cr_psnr = psnr_10bit((const uint16_t *)src_frame->cr,
                                 src_frame->cr_stride,
                                 (const uint16_t *)frame.planes[2],
                                 frame.stride[2] / 2,
                                 frame.width >> 1,
                                 frame.height >> 1);
        }
    }
}

class PsnrStatistics {
  public:
    PsnrStatistics() {
        reset();
    }
    ~PsnrStatistics() {
    }
    void add(const double psnr_luma, const double psnr_cb,
             const double psnr_cr) {
        psnr_luma_ += psnr_luma;
        psnr_cb_ += psnr_cb;
        psnr_cr_ += psnr_cr;
        psnr_total_ += (psnr_luma + psnr_cb + psnr_cr) / 3;
        ++count_;
    }

    void get_statistics(int &count, double &total, double &luma, double &cb,
                        double &cr) {
        count = count_;
        if (count != 0) {
            total = psnr_total_ / count_;
            luma = psnr_luma_ / count_;
            cb = psnr_cb_ / count_;
            cr = psnr_cr_ / count_;
        } else {
            total = 0;
            luma = 0;
            cb = 0;
            cr = 0;
        }
    }

    void reset() {
        psnr_total_ = 0;
        psnr_luma_ = 0;
        psnr_cb_ = 0;
        psnr_cr_ = 0;
        count_ = 0;
    }

  private:
    double psnr_total_;
    double psnr_luma_;
    double psnr_cb_;
    double psnr_cr_;
    int count_;
};

}  // namespace svt_av1_e2e_tools
#endif  // !_COMPARE_TOOLS_H_
