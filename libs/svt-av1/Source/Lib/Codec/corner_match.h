/*
 * Copyright (c) 2016, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
 */
#ifndef AOM_AV1_ENCODER_CORNER_MATCH_H_
#define AOM_AV1_ENCODER_CORNER_MATCH_H_

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <memory.h>
#include "EbDebugMacros.h"
#include "definitions.h"
#include "global_motion.h"

int svt_av1_determine_correspondence(uint8_t* frm, int* frm_corners, int num_frm_corners, uint8_t* ref,
                                     int* ref_corners, int num_ref_corners, int width, int height, int frm_stride,
                                     int ref_stride, Correspondence* correspondences, uint8_t match_sz);

DECLARE_ALIGNED(16, extern const uint8_t, svt_aom_compute_cross_byte_mask[8][16]);

#endif // AOM_AV1_ENCODER_CORNER_MATCH_H_
