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

#ifndef EbSuperRes_h
#define EbSuperRes_h

#ifdef __cplusplus
extern "C" {
#endif

#define RS_SUBPEL_BITS 6
#define RS_SUBPEL_MASK ((1 << RS_SUBPEL_BITS) - 1)
#define RS_SCALE_SUBPEL_BITS 14
#define RS_SCALE_SUBPEL_MASK ((1 << RS_SCALE_SUBPEL_BITS) - 1)
#define RS_SCALE_EXTRA_BITS (RS_SCALE_SUBPEL_BITS - RS_SUBPEL_BITS)
#define RS_SCALE_EXTRA_OFF (1 << (RS_SCALE_EXTRA_BITS - 1))
#define UPSCALE_NORMATIVE_TAPS 8

extern const int16_t svt_av1_resize_filter_normative[(1 << RS_SUBPEL_BITS)][UPSCALE_NORMATIVE_TAPS];
// Filters for interpolation (full-band) - no filtering for integer pixels

void svt_av1_upscale_normative_rows(const Av1Common* cm, const uint8_t* src, int src_stride, uint8_t* dst,
                                    int dst_stride, int rows, int sub_x, int bd, bool is_16bit_pipeline);
#ifdef __cplusplus
}
#endif
#endif // EbSuperRes_h
