/*
 * Copyright (c) 2019, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
 */

#include <immintrin.h>
#include "common_dsp_rtcd.h"
#include "warped_motion.h"

/* This is a modified version of 'svt_aom_warped_filter' from warped_motion.c:
    * Each coefficient is stored in 8 bits instead of 16 bits
    * The coefficients are rearranged in the column order 0, 2, 4, 6, 1, 3, 5, 7

      This is done in order to avoid overflow: Since the tap with the largest
      coefficient could be any of taps 2, 3, 4 or 5, we can't use the summation
      order ((0 + 1) + (4 + 5)) + ((2 + 3) + (6 + 7)) used in the regular
      convolve functions.

      Instead, we use the summation order
      ((0 + 2) + (4 + 6)) + ((1 + 3) + (5 + 7)).
      The rearrangement of coefficients in this table is so that we can get the
      coefficients into the correct order more quickly.
 */
/* clang-format off */
DECLARE_ALIGNED(8, const int8_t,
eb_av1_filter_8bit[WARPEDPIXEL_PREC_SHIFTS * 3 + 1][8]) = {
#if WARPEDPIXEL_PREC_BITS == 6
        // [-1, 0)
        { 0, 127,   0, 0,   0,   1, 0, 0}, { 0, 127,   0, 0,  -1,   2, 0, 0},
        { 1, 127,  -1, 0,  -3,   4, 0, 0}, { 1, 126,  -2, 0,  -4,   6, 1, 0},
        { 1, 126,  -3, 0,  -5,   8, 1, 0}, { 1, 125,  -4, 0,  -6,  11, 1, 0},
        { 1, 124,  -4, 0,  -7,  13, 1, 0}, { 2, 123,  -5, 0,  -8,  15, 1, 0},
        { 2, 122,  -6, 0,  -9,  18, 1, 0}, { 2, 121,  -6, 0, -10,  20, 1, 0},
        { 2, 120,  -7, 0, -11,  22, 2, 0}, { 2, 119,  -8, 0, -12,  25, 2, 0},
        { 3, 117,  -8, 0, -13,  27, 2, 0}, { 3, 116,  -9, 0, -13,  29, 2, 0},
        { 3, 114, -10, 0, -14,  32, 3, 0}, { 3, 113, -10, 0, -15,  35, 2, 0},
        { 3, 111, -11, 0, -15,  37, 3, 0}, { 3, 109, -11, 0, -16,  40, 3, 0},
        { 3, 108, -12, 0, -16,  42, 3, 0}, { 4, 106, -13, 0, -17,  45, 3, 0},
        { 4, 104, -13, 0, -17,  47, 3, 0}, { 4, 102, -14, 0, -17,  50, 3, 0},
        { 4, 100, -14, 0, -17,  52, 3, 0}, { 4,  98, -15, 0, -18,  55, 4, 0},
        { 4,  96, -15, 0, -18,  58, 3, 0}, { 4,  94, -16, 0, -18,  60, 4, 0},
        { 4,  91, -16, 0, -18,  63, 4, 0}, { 4,  89, -16, 0, -18,  65, 4, 0},
        { 4,  87, -17, 0, -18,  68, 4, 0}, { 4,  85, -17, 0, -18,  70, 4, 0},
        { 4,  82, -17, 0, -18,  73, 4, 0}, { 4,  80, -17, 0, -18,  75, 4, 0},
        { 4,  78, -18, 0, -18,  78, 4, 0}, { 4,  75, -18, 0, -17,  80, 4, 0},
        { 4,  73, -18, 0, -17,  82, 4, 0}, { 4,  70, -18, 0, -17,  85, 4, 0},
        { 4,  68, -18, 0, -17,  87, 4, 0}, { 4,  65, -18, 0, -16,  89, 4, 0},
        { 4,  63, -18, 0, -16,  91, 4, 0}, { 4,  60, -18, 0, -16,  94, 4, 0},
        { 3,  58, -18, 0, -15,  96, 4, 0}, { 4,  55, -18, 0, -15,  98, 4, 0},
        { 3,  52, -17, 0, -14, 100, 4, 0}, { 3,  50, -17, 0, -14, 102, 4, 0},
        { 3,  47, -17, 0, -13, 104, 4, 0}, { 3,  45, -17, 0, -13, 106, 4, 0},
        { 3,  42, -16, 0, -12, 108, 3, 0}, { 3,  40, -16, 0, -11, 109, 3, 0},
        { 3,  37, -15, 0, -11, 111, 3, 0}, { 2,  35, -15, 0, -10, 113, 3, 0},
        { 3,  32, -14, 0, -10, 114, 3, 0}, { 2,  29, -13, 0,  -9, 116, 3, 0},
        { 2,  27, -13, 0,  -8, 117, 3, 0}, { 2,  25, -12, 0,  -8, 119, 2, 0},
        { 2,  22, -11, 0,  -7, 120, 2, 0}, { 1,  20, -10, 0,  -6, 121, 2, 0},
        { 1,  18,  -9, 0,  -6, 122, 2, 0}, { 1,  15,  -8, 0,  -5, 123, 2, 0},
        { 1,  13,  -7, 0,  -4, 124, 1, 0}, { 1,  11,  -6, 0,  -4, 125, 1, 0},
        { 1,   8,  -5, 0,  -3, 126, 1, 0}, { 1,   6,  -4, 0,  -2, 126, 1, 0},
        { 0,   4,  -3, 0,  -1, 127, 1, 0}, { 0,   2,  -1, 0,   0, 127, 0, 0},
        // [0, 1)
        { 0,   0,   1, 0, 0, 127,   0,  0}, { 0,  -1,   2, 0, 0, 127,   0,  0},
        { 0,  -3,   4, 1, 1, 127,  -2,  0}, { 0,  -5,   6, 1, 1, 127,  -2,  0},
        { 0,  -6,   8, 1, 2, 126,  -3,  0}, {-1,  -7,  11, 2, 2, 126,  -4, -1},
        {-1,  -8,  13, 2, 3, 125,  -5, -1}, {-1, -10,  16, 3, 3, 124,  -6, -1},
        {-1, -11,  18, 3, 4, 123,  -7, -1}, {-1, -12,  20, 3, 4, 122,  -7, -1},
        {-1, -13,  23, 3, 4, 121,  -8, -1}, {-2, -14,  25, 4, 5, 120,  -9, -1},
        {-1, -15,  27, 4, 5, 119, -10, -1}, {-1, -16,  30, 4, 5, 118, -11, -1},
        {-2, -17,  33, 5, 6, 116, -12, -1}, {-2, -17,  35, 5, 6, 114, -12, -1},
        {-2, -18,  38, 5, 6, 113, -13, -1}, {-2, -19,  41, 6, 7, 111, -14, -2},
        {-2, -19,  43, 6, 7, 110, -15, -2}, {-2, -20,  46, 6, 7, 108, -15, -2},
        {-2, -20,  49, 6, 7, 106, -16, -2}, {-2, -21,  51, 7, 7, 104, -16, -2},
        {-2, -21,  54, 7, 7, 102, -17, -2}, {-2, -21,  56, 7, 8, 100, -18, -2},
        {-2, -22,  59, 7, 8,  98, -18, -2}, {-2, -22,  62, 7, 8,  96, -19, -2},
        {-2, -22,  64, 7, 8,  94, -19, -2}, {-2, -22,  67, 8, 8,  91, -20, -2},
        {-2, -22,  69, 8, 8,  89, -20, -2}, {-2, -22,  72, 8, 8,  87, -21, -2},
        {-2, -21,  74, 8, 8,  84, -21, -2}, {-2, -22,  77, 8, 8,  82, -21, -2},
        {-2, -21,  79, 8, 8,  79, -21, -2}, {-2, -21,  82, 8, 8,  77, -22, -2},
        {-2, -21,  84, 8, 8,  74, -21, -2}, {-2, -21,  87, 8, 8,  72, -22, -2},
        {-2, -20,  89, 8, 8,  69, -22, -2}, {-2, -20,  91, 8, 8,  67, -22, -2},
        {-2, -19,  94, 8, 7,  64, -22, -2}, {-2, -19,  96, 8, 7,  62, -22, -2},
        {-2, -18,  98, 8, 7,  59, -22, -2}, {-2, -18, 100, 8, 7,  56, -21, -2},
        {-2, -17, 102, 7, 7,  54, -21, -2}, {-2, -16, 104, 7, 7,  51, -21, -2},
        {-2, -16, 106, 7, 6,  49, -20, -2}, {-2, -15, 108, 7, 6,  46, -20, -2},
        {-2, -15, 110, 7, 6,  43, -19, -2}, {-2, -14, 111, 7, 6,  41, -19, -2},
        {-1, -13, 113, 6, 5,  38, -18, -2}, {-1, -12, 114, 6, 5,  35, -17, -2},
        {-1, -12, 116, 6, 5,  33, -17, -2}, {-1, -11, 118, 5, 4,  30, -16, -1},
        {-1, -10, 119, 5, 4,  27, -15, -1}, {-1,  -9, 120, 5, 4,  25, -14, -2},
        {-1,  -8, 121, 4, 3,  23, -13, -1}, {-1,  -7, 122, 4, 3,  20, -12, -1},
        {-1,  -7, 123, 4, 3,  18, -11, -1}, {-1,  -6, 124, 3, 3,  16, -10, -1},
        {-1,  -5, 125, 3, 2,  13,  -8, -1}, {-1,  -4, 126, 2, 2,  11,  -7, -1},
        { 0,  -3, 126, 2, 1,   8,  -6,  0}, { 0,  -2, 127, 1, 1,   6,  -5,  0},
        { 0,  -2, 127, 1, 1,   4,  -3,  0}, { 0,   0, 127, 0, 0,   2,  -1,  0},
        // [1, 2)
        { 0, 0, 127,   0, 0,   1,   0, 0}, { 0, 0, 127,   0, 0,  -1,   2, 0},
        { 0, 1, 127,  -1, 0,  -3,   4, 0}, { 0, 1, 126,  -2, 0,  -4,   6, 1},
        { 0, 1, 126,  -3, 0,  -5,   8, 1}, { 0, 1, 125,  -4, 0,  -6,  11, 1},
        { 0, 1, 124,  -4, 0,  -7,  13, 1}, { 0, 2, 123,  -5, 0,  -8,  15, 1},
        { 0, 2, 122,  -6, 0,  -9,  18, 1}, { 0, 2, 121,  -6, 0, -10,  20, 1},
        { 0, 2, 120,  -7, 0, -11,  22, 2}, { 0, 2, 119,  -8, 0, -12,  25, 2},
        { 0, 3, 117,  -8, 0, -13,  27, 2}, { 0, 3, 116,  -9, 0, -13,  29, 2},
        { 0, 3, 114, -10, 0, -14,  32, 3}, { 0, 3, 113, -10, 0, -15,  35, 2},
        { 0, 3, 111, -11, 0, -15,  37, 3}, { 0, 3, 109, -11, 0, -16,  40, 3},
        { 0, 3, 108, -12, 0, -16,  42, 3}, { 0, 4, 106, -13, 0, -17,  45, 3},
        { 0, 4, 104, -13, 0, -17,  47, 3}, { 0, 4, 102, -14, 0, -17,  50, 3},
        { 0, 4, 100, -14, 0, -17,  52, 3}, { 0, 4,  98, -15, 0, -18,  55, 4},
        { 0, 4,  96, -15, 0, -18,  58, 3}, { 0, 4,  94, -16, 0, -18,  60, 4},
        { 0, 4,  91, -16, 0, -18,  63, 4}, { 0, 4,  89, -16, 0, -18,  65, 4},
        { 0, 4,  87, -17, 0, -18,  68, 4}, { 0, 4,  85, -17, 0, -18,  70, 4},
        { 0, 4,  82, -17, 0, -18,  73, 4}, { 0, 4,  80, -17, 0, -18,  75, 4},
        { 0, 4,  78, -18, 0, -18,  78, 4}, { 0, 4,  75, -18, 0, -17,  80, 4},
        { 0, 4,  73, -18, 0, -17,  82, 4}, { 0, 4,  70, -18, 0, -17,  85, 4},
        { 0, 4,  68, -18, 0, -17,  87, 4}, { 0, 4,  65, -18, 0, -16,  89, 4},
        { 0, 4,  63, -18, 0, -16,  91, 4}, { 0, 4,  60, -18, 0, -16,  94, 4},
        { 0, 3,  58, -18, 0, -15,  96, 4}, { 0, 4,  55, -18, 0, -15,  98, 4},
        { 0, 3,  52, -17, 0, -14, 100, 4}, { 0, 3,  50, -17, 0, -14, 102, 4},
        { 0, 3,  47, -17, 0, -13, 104, 4}, { 0, 3,  45, -17, 0, -13, 106, 4},
        { 0, 3,  42, -16, 0, -12, 108, 3}, { 0, 3,  40, -16, 0, -11, 109, 3},
        { 0, 3,  37, -15, 0, -11, 111, 3}, { 0, 2,  35, -15, 0, -10, 113, 3},
        { 0, 3,  32, -14, 0, -10, 114, 3}, { 0, 2,  29, -13, 0,  -9, 116, 3},
        { 0, 2,  27, -13, 0,  -8, 117, 3}, { 0, 2,  25, -12, 0,  -8, 119, 2},
        { 0, 2,  22, -11, 0,  -7, 120, 2}, { 0, 1,  20, -10, 0,  -6, 121, 2},
        { 0, 1,  18,  -9, 0,  -6, 122, 2}, { 0, 1,  15,  -8, 0,  -5, 123, 2},
        { 0, 1,  13,  -7, 0,  -4, 124, 1}, { 0, 1,  11,  -6, 0,  -4, 125, 1},
        { 0, 1,   8,  -5, 0,  -3, 126, 1}, { 0, 1,   6,  -4, 0,  -2, 126, 1},
        { 0, 0,   4,  -3, 0,  -1, 127, 1}, { 0, 0,   2,  -1, 0,   0, 127, 0},
        // dummy (replicate row index 191)
        { 0, 0,   2,  -1, 0,   0, 127, 0},

      #else
        // [-1, 0)
        { 0, 127,   0, 0,   0,   1, 0, 0}, { 1, 127,  -1, 0,  -3,   4, 0, 0},
        { 1, 126,  -3, 0,  -5,   8, 1, 0}, { 1, 124,  -4, 0,  -7,  13, 1, 0},
        { 2, 122,  -6, 0,  -9,  18, 1, 0}, { 2, 120,  -7, 0, -11,  22, 2, 0},
        { 3, 117,  -8, 0, -13,  27, 2, 0}, { 3, 114, -10, 0, -14,  32, 3, 0},
        { 3, 111, -11, 0, -15,  37, 3, 0}, { 3, 108, -12, 0, -16,  42, 3, 0},
        { 4, 104, -13, 0, -17,  47, 3, 0}, { 4, 100, -14, 0, -17,  52, 3, 0},
        { 4,  96, -15, 0, -18,  58, 3, 0}, { 4,  91, -16, 0, -18,  63, 4, 0},
        { 4,  87, -17, 0, -18,  68, 4, 0}, { 4,  82, -17, 0, -18,  73, 4, 0},
        { 4,  78, -18, 0, -18,  78, 4, 0}, { 4,  73, -18, 0, -17,  82, 4, 0},
        { 4,  68, -18, 0, -17,  87, 4, 0}, { 4,  63, -18, 0, -16,  91, 4, 0},
        { 3,  58, -18, 0, -15,  96, 4, 0}, { 3,  52, -17, 0, -14, 100, 4, 0},
        { 3,  47, -17, 0, -13, 104, 4, 0}, { 3,  42, -16, 0, -12, 108, 3, 0},
        { 3,  37, -15, 0, -11, 111, 3, 0}, { 3,  32, -14, 0, -10, 114, 3, 0},
        { 2,  27, -13, 0,  -8, 117, 3, 0}, { 2,  22, -11, 0,  -7, 120, 2, 0},
        { 1,  18,  -9, 0,  -6, 122, 2, 0}, { 1,  13,  -7, 0,  -4, 124, 1, 0},
        { 1,   8,  -5, 0,  -3, 126, 1, 0}, { 0,   4,  -3, 0,  -1, 127, 1, 0},
        // [0, 1)
        { 0,   0,   1, 0, 0, 127,   0,  0}, { 0,  -3,   4, 1, 1, 127,  -2,  0},
        { 0,  -6,   8, 1, 2, 126,  -3,  0}, {-1,  -8,  13, 2, 3, 125,  -5, -1},
        {-1, -11,  18, 3, 4, 123,  -7, -1}, {-1, -13,  23, 3, 4, 121,  -8, -1},
        {-1, -15,  27, 4, 5, 119, -10, -1}, {-2, -17,  33, 5, 6, 116, -12, -1},
        {-2, -18,  38, 5, 6, 113, -13, -1}, {-2, -19,  43, 6, 7, 110, -15, -2},
        {-2, -20,  49, 6, 7, 106, -16, -2}, {-2, -21,  54, 7, 7, 102, -17, -2},
        {-2, -22,  59, 7, 8,  98, -18, -2}, {-2, -22,  64, 7, 8,  94, -19, -2},
        {-2, -22,  69, 8, 8,  89, -20, -2}, {-2, -21,  74, 8, 8,  84, -21, -2},
        {-2, -21,  79, 8, 8,  79, -21, -2}, {-2, -21,  84, 8, 8,  74, -21, -2},
        {-2, -20,  89, 8, 8,  69, -22, -2}, {-2, -19,  94, 8, 7,  64, -22, -2},
        {-2, -18,  98, 8, 7,  59, -22, -2}, {-2, -17, 102, 7, 7,  54, -21, -2},
        {-2, -16, 106, 7, 6,  49, -20, -2}, {-2, -15, 110, 7, 6,  43, -19, -2},
        {-1, -13, 113, 6, 5,  38, -18, -2}, {-1, -12, 116, 6, 5,  33, -17, -2},
        {-1, -10, 119, 5, 4,  27, -15, -1}, {-1,  -8, 121, 4, 3,  23, -13, -1},
        {-1,  -7, 123, 4, 3,  18, -11, -1}, {-1,  -5, 125, 3, 2,  13,  -8, -1},
        { 0,  -3, 126, 2, 1,   8,  -6,  0}, { 0,  -2, 127, 1, 1,   4,  -3,  0},
        // [1, 2)
        { 0,  0, 127,   0, 0,   1,   0, 0}, { 0, 1, 127,  -1, 0,  -3,   4, 0},
        { 0,  1, 126,  -3, 0,  -5,   8, 1}, { 0, 1, 124,  -4, 0,  -7,  13, 1},
        { 0,  2, 122,  -6, 0,  -9,  18, 1}, { 0, 2, 120,  -7, 0, -11,  22, 2},
        { 0,  3, 117,  -8, 0, -13,  27, 2}, { 0, 3, 114, -10, 0, -14,  32, 3},
        { 0,  3, 111, -11, 0, -15,  37, 3}, { 0, 3, 108, -12, 0, -16,  42, 3},
        { 0,  4, 104, -13, 0, -17,  47, 3}, { 0, 4, 100, -14, 0, -17,  52, 3},
        { 0,  4,  96, -15, 0, -18,  58, 3}, { 0, 4,  91, -16, 0, -18,  63, 4},
        { 0,  4,  87, -17, 0, -18,  68, 4}, { 0, 4,  82, -17, 0, -18,  73, 4},
        { 0,  4,  78, -18, 0, -18,  78, 4}, { 0, 4,  73, -18, 0, -17,  82, 4},
        { 0,  4,  68, -18, 0, -17,  87, 4}, { 0, 4,  63, -18, 0, -16,  91, 4},
        { 0,  3,  58, -18, 0, -15,  96, 4}, { 0, 3,  52, -17, 0, -14, 100, 4},
        { 0,  3,  47, -17, 0, -13, 104, 4}, { 0, 3,  42, -16, 0, -12, 108, 3},
        { 0,  3,  37, -15, 0, -11, 111, 3}, { 0, 3,  32, -14, 0, -10, 114, 3},
        { 0,  2,  27, -13, 0,  -8, 117, 3}, { 0, 2,  22, -11, 0,  -7, 120, 2},
        { 0,  1,  18,  -9, 0,  -6, 122, 2}, { 0, 1,  13,  -7, 0,  -4, 124, 1},
        { 0,  1,   8,  -5, 0,  -3, 126, 1}, { 0, 0,   4,  -3, 0,  -1, 127, 1},
        // dummy (replicate row index 95)
        { 0, 0,   4,  -3, 0,  -1, 127, 1},
      #endif  // WARPEDPIXEL_PREC_BITS == 6
};
/* clang-format on */

DECLARE_ALIGNED(32, static const uint8_t, shuffle_alpha0_mask01_avx2[32]) = {
    0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1};

DECLARE_ALIGNED(32, static const uint8_t, shuffle_alpha0_mask23_avx2[32]) = {
    2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3};

DECLARE_ALIGNED(32, static const uint8_t, shuffle_alpha0_mask45_avx2[32]) = {
    4, 5, 4, 5, 4, 5, 4, 5, 4, 5, 4, 5, 4, 5, 4, 5, 4, 5, 4, 5, 4, 5, 4, 5, 4, 5, 4, 5, 4, 5, 4, 5};

DECLARE_ALIGNED(32, static const uint8_t, shuffle_alpha0_mask67_avx2[32]) = {
    6, 7, 6, 7, 6, 7, 6, 7, 6, 7, 6, 7, 6, 7, 6, 7, 6, 7, 6, 7, 6, 7, 6, 7, 6, 7, 6, 7, 6, 7, 6, 7};

DECLARE_ALIGNED(32, static const uint8_t, shuffle_gamma0_mask0_avx2[32]) = {
    0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3};

DECLARE_ALIGNED(32, static const uint8_t, shuffle_gamma0_mask1_avx2[32]) = {
    4, 5, 6, 7, 4, 5, 6, 7, 4, 5, 6, 7, 4, 5, 6, 7, 4, 5, 6, 7, 4, 5, 6, 7, 4, 5, 6, 7, 4, 5, 6, 7};

DECLARE_ALIGNED(32, static const uint8_t, shuffle_gamma0_mask2_avx2[32]) = {
    8, 9, 10, 11, 8, 9, 10, 11, 8, 9, 10, 11, 8, 9, 10, 11, 8, 9, 10, 11, 8, 9, 10, 11, 8, 9, 10, 11, 8, 9, 10, 11};

DECLARE_ALIGNED(32, static const uint8_t, shuffle_gamma0_mask3_avx2[32]) = {12, 13, 14, 15, 12, 13, 14, 15, 12, 13, 14,
                                                                            15, 12, 13, 14, 15, 12, 13, 14, 15, 12, 13,
                                                                            14, 15, 12, 13, 14, 15, 12, 13, 14, 15};

DECLARE_ALIGNED(32, static const uint8_t, shuffle_src0[32]) = {0, 2, 2, 4, 4, 6, 6, 8, 1, 3, 3, 5, 5, 7, 7, 9,
                                                               0, 2, 2, 4, 4, 6, 6, 8, 1, 3, 3, 5, 5, 7, 7, 9};

DECLARE_ALIGNED(32, static const uint8_t, shuffle_src1[32]) = {4, 6, 6, 8, 8, 10, 10, 12, 5, 7, 7, 9, 9, 11, 11, 13,
                                                               4, 6, 6, 8, 8, 10, 10, 12, 5, 7, 7, 9, 9, 11, 11, 13};

DECLARE_ALIGNED(32, static const uint8_t, shuffle_src2[32]) = {1, 3, 3, 5, 5, 7, 7, 9, 2, 4, 4, 6, 6, 8, 8, 10,
                                                               1, 3, 3, 5, 5, 7, 7, 9, 2, 4, 4, 6, 6, 8, 8, 10};

DECLARE_ALIGNED(32, static const uint8_t, shuffle_src3[32]) = {5, 7, 7, 9, 9, 11, 11, 13, 6, 8, 8, 10, 10, 12, 12, 14,
                                                               5, 7, 7, 9, 9, 11, 11, 13, 6, 8, 8, 10, 10, 12, 12, 14};

static INLINE void filter_src_pixels_avx2(const __m256i src, __m256i* horz_out, __m256i* coeff,
                                          const __m256i* shuffle_src, const __m256i* round_const, const __m128i* shift,
                                          int row) {
    const __m256i src_0 = _mm256_shuffle_epi8(src, shuffle_src[0]);
    const __m256i src_1 = _mm256_shuffle_epi8(src, shuffle_src[1]);
    const __m256i src_2 = _mm256_shuffle_epi8(src, shuffle_src[2]);
    const __m256i src_3 = _mm256_shuffle_epi8(src, shuffle_src[3]);

    const __m256i res_02 = _mm256_maddubs_epi16(src_0, coeff[0]);
    const __m256i res_46 = _mm256_maddubs_epi16(src_1, coeff[1]);
    const __m256i res_13 = _mm256_maddubs_epi16(src_2, coeff[2]);
    const __m256i res_57 = _mm256_maddubs_epi16(src_3, coeff[3]);

    const __m256i res_even = _mm256_add_epi16(res_02, res_46);
    const __m256i res_odd  = _mm256_add_epi16(res_13, res_57);
    const __m256i res      = _mm256_add_epi16(_mm256_add_epi16(res_even, res_odd), *round_const);
    horz_out[row]          = _mm256_srl_epi16(res, *shift);
}

static INLINE void prepare_horizontal_filter_coeff_avx2(int alpha, int beta, int sx, __m256i* coeff) {
    //AVX2 Calculation: idx_final[i] = (sx + i * alpha) >> WARPEDDIFF_PREC_BITS
    const __m256i idx       = _mm256_setr_epi32(0, 1, 2, 3, 4, 5, 6, 7);
    const __m256i sx_val    = _mm256_set1_epi32(sx);
    const __m256i sx_b_val  = _mm256_set1_epi32(sx + beta);
    __m256i       alpha_val = _mm256_set1_epi32(alpha);
    alpha_val               = _mm256_mullo_epi32(alpha_val, idx);

    DECLARE_ALIGNED(32, int32_t, idx_final[8]);
    _mm256_storeu_si256((__m256i*)idx_final,
                        _mm256_srli_epi32(_mm256_add_epi32(sx_val, alpha_val), WARPEDDIFF_PREC_BITS));

    __m128i tmp_0 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[0]]);
    __m128i tmp_1 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[1]]);
    __m128i tmp_2 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[2]]);
    __m128i tmp_3 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[3]]);
    __m128i tmp_4 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[4]]);
    __m128i tmp_5 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[5]]);
    __m128i tmp_6 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[6]]);
    __m128i tmp_7 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[7]]);

    tmp_0 = _mm_unpacklo_epi16(tmp_0, tmp_2);
    tmp_1 = _mm_unpacklo_epi16(tmp_1, tmp_3);
    tmp_4 = _mm_unpacklo_epi16(tmp_4, tmp_6);
    tmp_5 = _mm_unpacklo_epi16(tmp_5, tmp_7);

    //AVX2 Calculation: idx_final[i] = ((sx + beta) + i * alpha) >> WARPEDDIFF_PREC_BITS
    _mm256_storeu_si256((__m256i*)idx_final,
                        _mm256_srli_epi32(_mm256_add_epi32(sx_b_val, alpha_val), WARPEDDIFF_PREC_BITS));

    __m128i tmp_8  = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[0]]);
    __m128i tmp_9  = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[1]]);
    __m128i tmp_10 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[2]]);
    __m128i tmp_11 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[3]]);
    tmp_2          = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[4]]);
    tmp_3          = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[5]]);
    tmp_6          = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[6]]);
    tmp_7          = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[7]]);

    tmp_8 = _mm_unpacklo_epi16(tmp_8, tmp_10);
    tmp_2 = _mm_unpacklo_epi16(tmp_2, tmp_6);
    tmp_9 = _mm_unpacklo_epi16(tmp_9, tmp_11);
    tmp_3 = _mm_unpacklo_epi16(tmp_3, tmp_7);

    const __m256i tmp_12 = _mm256_inserti128_si256(_mm256_castsi128_si256(tmp_0), tmp_8, 0x1);
    const __m256i tmp_13 = _mm256_inserti128_si256(_mm256_castsi128_si256(tmp_1), tmp_9, 0x1);
    const __m256i tmp_14 = _mm256_inserti128_si256(_mm256_castsi128_si256(tmp_4), tmp_2, 0x1);
    const __m256i tmp_15 = _mm256_inserti128_si256(_mm256_castsi128_si256(tmp_5), tmp_3, 0x1);

    const __m256i res_0 = _mm256_unpacklo_epi32(tmp_12, tmp_14);
    const __m256i res_1 = _mm256_unpackhi_epi32(tmp_12, tmp_14);
    const __m256i res_2 = _mm256_unpacklo_epi32(tmp_13, tmp_15);
    const __m256i res_3 = _mm256_unpackhi_epi32(tmp_13, tmp_15);

    coeff[0] = _mm256_unpacklo_epi64(res_0, res_2);
    coeff[1] = _mm256_unpackhi_epi64(res_0, res_2);
    coeff[2] = _mm256_unpacklo_epi64(res_1, res_3);
    coeff[3] = _mm256_unpackhi_epi64(res_1, res_3);
}

static INLINE void prepare_horizontal_filter_coeff_beta0_avx2(int alpha, int sx, __m256i* coeff) {
    //AVX2 Calculation: idx_final[i] = (sx + i * alpha) >> WARPEDDIFF_PREC_BITS
    const __m256i idx       = _mm256_setr_epi32(0, 1, 2, 3, 4, 5, 6, 7);
    const __m256i sx_val    = _mm256_set1_epi32(sx);
    __m256i       alpha_val = _mm256_set1_epi32(alpha);
    alpha_val               = _mm256_mullo_epi32(alpha_val, idx);

    DECLARE_ALIGNED(32, int32_t, idx_final[8]);
    _mm256_storeu_si256((__m256i*)idx_final,
                        _mm256_srli_epi32(_mm256_add_epi32(sx_val, alpha_val), WARPEDDIFF_PREC_BITS));

    __m128i tmp_0 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[0]]);
    __m128i tmp_1 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[1]]);
    __m128i tmp_2 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[2]]);
    __m128i tmp_3 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[3]]);
    __m128i tmp_4 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[4]]);
    __m128i tmp_5 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[5]]);
    __m128i tmp_6 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[6]]);
    __m128i tmp_7 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[7]]);

    tmp_0 = _mm_unpacklo_epi16(tmp_0, tmp_2);
    tmp_1 = _mm_unpacklo_epi16(tmp_1, tmp_3);
    tmp_4 = _mm_unpacklo_epi16(tmp_4, tmp_6);
    tmp_5 = _mm_unpacklo_epi16(tmp_5, tmp_7);

    const __m256i tmp_12 = _mm256_broadcastsi128_si256(tmp_0);
    const __m256i tmp_13 = _mm256_broadcastsi128_si256(tmp_1);
    const __m256i tmp_14 = _mm256_broadcastsi128_si256(tmp_4);
    const __m256i tmp_15 = _mm256_broadcastsi128_si256(tmp_5);

    const __m256i res_0 = _mm256_unpacklo_epi32(tmp_12, tmp_14);
    const __m256i res_1 = _mm256_unpackhi_epi32(tmp_12, tmp_14);
    const __m256i res_2 = _mm256_unpacklo_epi32(tmp_13, tmp_15);
    const __m256i res_3 = _mm256_unpackhi_epi32(tmp_13, tmp_15);

    coeff[0] = _mm256_unpacklo_epi64(res_0, res_2);
    coeff[1] = _mm256_unpackhi_epi64(res_0, res_2);
    coeff[2] = _mm256_unpacklo_epi64(res_1, res_3);
    coeff[3] = _mm256_unpackhi_epi64(res_1, res_3);
}

static INLINE void prepare_horizontal_filter_coeff_alpha0_avx2(int beta, int sx, __m256i* coeff) {
    const __m128i tmp_0 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[sx >> WARPEDDIFF_PREC_BITS]);
    const __m128i tmp_1 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[(sx + beta) >> WARPEDDIFF_PREC_BITS]);

    const __m256i res_0 = _mm256_inserti128_si256(_mm256_castsi128_si256(tmp_0), tmp_1, 0x1);

    coeff[0] = _mm256_shuffle_epi8(res_0, _mm256_loadu_si256((__m256i*)shuffle_alpha0_mask01_avx2));
    coeff[1] = _mm256_shuffle_epi8(res_0, _mm256_loadu_si256((__m256i*)shuffle_alpha0_mask23_avx2));
    coeff[2] = _mm256_shuffle_epi8(res_0, _mm256_loadu_si256((__m256i*)shuffle_alpha0_mask45_avx2));
    coeff[3] = _mm256_shuffle_epi8(res_0, _mm256_loadu_si256((__m256i*)shuffle_alpha0_mask67_avx2));
}

static INLINE void horizontal_filter_avx2(const __m256i src, __m256i* horz_out, int sx, int alpha, int beta, int row,
                                          const __m256i* shuffle_src, const __m256i* round_const,
                                          const __m128i* shift) {
    __m256i coeff[4];
    prepare_horizontal_filter_coeff_avx2(alpha, beta, sx, coeff);
    filter_src_pixels_avx2(src, horz_out, coeff, shuffle_src, round_const, shift, row);
}

static INLINE void prepare_horizontal_filter_coeff(int alpha, int sx, __m256i* coeff) {
    //AVX2 Calculation: idx_final[i] = (sx + i * alpha) >> WARPEDDIFF_PREC_BITS
    const __m256i idx       = _mm256_setr_epi32(0, 1, 2, 3, 4, 5, 6, 7);
    const __m256i sx_val    = _mm256_set1_epi32(sx);
    __m256i       alpha_val = _mm256_set1_epi32(alpha);
    alpha_val               = _mm256_mullo_epi32(alpha_val, idx);

    DECLARE_ALIGNED(32, int32_t, idx_final[8]);
    _mm256_storeu_si256((__m256i*)idx_final,
                        _mm256_srli_epi32(_mm256_add_epi32(sx_val, alpha_val), WARPEDDIFF_PREC_BITS));

    __m128i tmp_0 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[0]]);
    __m128i tmp_1 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[1]]);
    __m128i tmp_2 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[2]]);
    __m128i tmp_3 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[3]]);
    __m128i tmp_4 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[4]]);
    __m128i tmp_5 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[5]]);
    __m128i tmp_6 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[6]]);
    __m128i tmp_7 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[idx_final[7]]);

    const __m128i tmp_8  = _mm_unpacklo_epi16(tmp_0, tmp_2);
    const __m128i tmp_9  = _mm_unpacklo_epi16(tmp_1, tmp_3);
    const __m128i tmp_10 = _mm_unpacklo_epi16(tmp_4, tmp_6);
    const __m128i tmp_11 = _mm_unpacklo_epi16(tmp_5, tmp_7);

    const __m128i tmp_12 = _mm_unpacklo_epi32(tmp_8, tmp_10);
    const __m128i tmp_13 = _mm_unpackhi_epi32(tmp_8, tmp_10);
    const __m128i tmp_14 = _mm_unpacklo_epi32(tmp_9, tmp_11);
    const __m128i tmp_15 = _mm_unpackhi_epi32(tmp_9, tmp_11);

    coeff[0] = _mm256_castsi128_si256(_mm_unpacklo_epi64(tmp_12, tmp_14));
    coeff[1] = _mm256_castsi128_si256(_mm_unpackhi_epi64(tmp_12, tmp_14));
    coeff[2] = _mm256_castsi128_si256(_mm_unpacklo_epi64(tmp_13, tmp_15));
    coeff[3] = _mm256_castsi128_si256(_mm_unpackhi_epi64(tmp_13, tmp_15));
}

static INLINE void warp_horizontal_filter_avx2(const uint8_t* ref, __m256i* horz_out, int stride, int32_t ix4,
                                               int32_t iy4, int32_t sx4, int alpha, int beta, int p_height, int height,
                                               int i, const __m256i* round_const, const __m128i* shift,
                                               const __m256i* shuffle_src) {
    int     k, iy, sx, row = 0;
    __m256i coeff[4];
    ref += ix4 - 7;
    for (k = -7; k <= (AOMMIN(8, p_height - i) - 2); k += 2) {
        iy                   = iy4 + k;
        iy                   = clamp(iy, 0, height - 1);
        const __m128i src_0  = _mm_loadu_si128((__m128i*)(ref + iy * stride));
        iy                   = iy4 + k + 1;
        iy                   = clamp(iy, 0, height - 1);
        const __m128i src_1  = _mm_loadu_si128((__m128i*)(ref + iy * stride));
        const __m256i src_01 = _mm256_inserti128_si256(_mm256_castsi128_si256(src_0), src_1, 0x1);
        sx                   = sx4 + beta * (k + 4);
        horizontal_filter_avx2(src_01, horz_out, sx, alpha, beta, row, shuffle_src, round_const, shift);
        row += 1;
    }
    iy                   = iy4 + k;
    iy                   = clamp(iy, 0, height - 1);
    const __m256i src_01 = _mm256_castsi128_si256(_mm_loadu_si128((__m128i*)(ref + iy * stride)));
    sx                   = sx4 + beta * (k + 4);
    prepare_horizontal_filter_coeff(alpha, sx, coeff);
    filter_src_pixels_avx2(src_01, horz_out, coeff, shuffle_src, round_const, shift, row);
}

static INLINE void warp_horizontal_filter_alpha0_avx2(const uint8_t* ref, __m256i* horz_out, int stride, int32_t ix4,
                                                      int32_t iy4, int32_t sx4, int alpha, int beta, int p_height,
                                                      int height, int i, const __m256i* round_const,
                                                      const __m128i* shift, const __m256i* shuffle_src) {
    (void)alpha;
    int     k, iy, sx, row = 0;
    __m256i coeff[4];
    ref += ix4 - 7;
    for (k = -7; k <= (AOMMIN(8, p_height - i) - 2); k += 2) {
        iy                   = iy4 + k;
        iy                   = clamp(iy, 0, height - 1);
        const __m128i src_0  = _mm_loadu_si128((__m128i*)(ref + iy * stride));
        iy                   = iy4 + k + 1;
        iy                   = clamp(iy, 0, height - 1);
        const __m128i src_1  = _mm_loadu_si128((__m128i*)(ref + iy * stride));
        const __m256i src_01 = _mm256_inserti128_si256(_mm256_castsi128_si256(src_0), src_1, 0x1);
        sx                   = sx4 + beta * (k + 4);
        prepare_horizontal_filter_coeff_alpha0_avx2(beta, sx, coeff);
        filter_src_pixels_avx2(src_01, horz_out, coeff, shuffle_src, round_const, shift, row);
        row += 1;
    }
    iy                   = iy4 + k;
    iy                   = clamp(iy, 0, height - 1);
    const __m256i src_01 = _mm256_castsi128_si256(_mm_loadu_si128((__m128i*)(ref + iy * stride)));
    sx                   = sx4 + beta * (k + 4);
    prepare_horizontal_filter_coeff_alpha0_avx2(beta, sx, coeff);
    filter_src_pixels_avx2(src_01, horz_out, coeff, shuffle_src, round_const, shift, row);
}

static INLINE void warp_horizontal_filter_beta0_avx2(const uint8_t* ref, __m256i* horz_out, int stride, int32_t ix4,
                                                     int32_t iy4, int32_t sx4, int alpha, int beta, int p_height,
                                                     int height, int i, const __m256i* round_const,
                                                     const __m128i* shift, const __m256i* shuffle_src) {
    (void)beta;
    int     k, iy, row = 0;
    __m256i coeff[4];
    prepare_horizontal_filter_coeff_beta0_avx2(alpha, sx4, coeff);
    ref += ix4 - 7;
    for (k = -7; k <= (AOMMIN(8, p_height - i) - 2); k += 2) {
        iy                   = iy4 + k;
        iy                   = clamp(iy, 0, height - 1);
        const __m128i src_0  = _mm_loadu_si128((__m128i*)(ref + iy * stride));
        iy                   = iy4 + k + 1;
        iy                   = clamp(iy, 0, height - 1);
        const __m128i src_1  = _mm_loadu_si128((__m128i*)(ref + iy * stride));
        const __m256i src_01 = _mm256_inserti128_si256(_mm256_castsi128_si256(src_0), src_1, 0x1);
        filter_src_pixels_avx2(src_01, horz_out, coeff, shuffle_src, round_const, shift, row);
        row += 1;
    }
    iy                   = iy4 + k;
    iy                   = clamp(iy, 0, height - 1);
    const __m256i src_01 = _mm256_castsi128_si256(_mm_loadu_si128((__m128i*)(ref + iy * stride)));
    filter_src_pixels_avx2(src_01, horz_out, coeff, shuffle_src, round_const, shift, row);
}

static INLINE void warp_horizontal_filter_alpha0_beta0_avx2(const uint8_t* ref, __m256i* horz_out, int stride,
                                                            int32_t ix4, int32_t iy4, int32_t sx4, int alpha, int beta,
                                                            int p_height, int height, int i, const __m256i* round_const,
                                                            const __m128i* shift, const __m256i* shuffle_src) {
    (void)alpha;
    int     k, iy, row = 0;
    __m256i coeff[4];
    prepare_horizontal_filter_coeff_alpha0_avx2(beta, sx4, coeff);
    ref += ix4 - 7;
    for (k = -7; k <= (AOMMIN(8, p_height - i) - 2); k += 2) {
        iy                   = iy4 + k;
        iy                   = clamp(iy, 0, height - 1);
        const __m128i src0   = _mm_loadu_si128((__m128i*)(ref + iy * stride));
        iy                   = iy4 + k + 1;
        iy                   = clamp(iy, 0, height - 1);
        const __m128i src1   = _mm_loadu_si128((__m128i*)(ref + iy * stride));
        const __m256i src_01 = _mm256_inserti128_si256(_mm256_castsi128_si256(src0), src1, 0x1);
        filter_src_pixels_avx2(src_01, horz_out, coeff, shuffle_src, round_const, shift, row);
        row += 1;
    }
    iy                   = iy4 + k;
    iy                   = clamp(iy, 0, height - 1);
    const __m256i src_01 = _mm256_castsi128_si256(_mm_loadu_si128((__m128i*)(ref + iy * stride)));
    filter_src_pixels_avx2(src_01, horz_out, coeff, shuffle_src, round_const, shift, row);
}

static INLINE void unpack_weights_and_set_round_const_avx2(ConvolveParams* conv_params, const int round_bits,
                                                           const int offset_bits, __m256i* res_sub_const,
                                                           __m256i* round_bits_const, __m256i* wt) {
    *res_sub_const    = _mm256_set1_epi16(-(1 << (offset_bits - conv_params->round_1)) -
                                       (1 << (offset_bits - conv_params->round_1 - 1)));
    *round_bits_const = _mm256_set1_epi16(((1 << round_bits) >> 1));

    const int     w0  = conv_params->fwd_offset;
    const int     w1  = conv_params->bck_offset;
    const __m256i wt0 = _mm256_set1_epi16(w0);
    const __m256i wt1 = _mm256_set1_epi16(w1);
    *wt               = _mm256_unpacklo_epi16(wt0, wt1);
}

static INLINE void prepare_vertical_filter_coeffs_avx2(int gamma, int delta, int sy, __m256i* coeffs) {
    //AVX2 Calculation: [0;7] idx_final[i] = ((sy + i * gamma) >> WARPEDDIFF_PREC_BITS)
    //AVX2 Calculation: [8;15] idx_final[i] = (((sy + delta) + i * gamma) >> WARPEDDIFF_PREC_BITS)
    const __m256i idx       = _mm256_setr_epi32(0, 1, 2, 3, 4, 5, 6, 7);
    const __m256i sy_val    = _mm256_set1_epi32(sy);
    const __m256i sy_d_val  = _mm256_set1_epi32(sy + delta);
    __m256i       gamma_val = _mm256_set1_epi32(gamma);
    gamma_val               = _mm256_mullo_epi32(gamma_val, idx);

    DECLARE_ALIGNED(32, int32_t, idx_final[16]);
    _mm256_storeu_si256((__m256i*)idx_final,
                        _mm256_srli_epi32(_mm256_add_epi32(sy_val, gamma_val), WARPEDDIFF_PREC_BITS));
    _mm256_storeu_si256((__m256i*)(idx_final + 8),
                        _mm256_srli_epi32(_mm256_add_epi32(sy_d_val, gamma_val), WARPEDDIFF_PREC_BITS));

    __m128i filt_00 = _mm_loadu_si128((__m128i*)(svt_aom_warped_filter + idx_final[0]));
    __m128i filt_01 = _mm_loadu_si128((__m128i*)(svt_aom_warped_filter + idx_final[2]));
    __m128i filt_02 = _mm_loadu_si128((__m128i*)(svt_aom_warped_filter + idx_final[4]));
    __m128i filt_03 = _mm_loadu_si128((__m128i*)(svt_aom_warped_filter + idx_final[6]));

    __m128i filt_10 = _mm_loadu_si128((__m128i*)(svt_aom_warped_filter + idx_final[8]));
    __m128i filt_11 = _mm_loadu_si128((__m128i*)(svt_aom_warped_filter + idx_final[10]));
    __m128i filt_12 = _mm_loadu_si128((__m128i*)(svt_aom_warped_filter + idx_final[12]));
    __m128i filt_13 = _mm_loadu_si128((__m128i*)(svt_aom_warped_filter + idx_final[14]));

    __m256i filt_0 = _mm256_inserti128_si256(_mm256_castsi128_si256(filt_00), filt_10, 0x1);
    __m256i filt_1 = _mm256_inserti128_si256(_mm256_castsi128_si256(filt_01), filt_11, 0x1);
    __m256i filt_2 = _mm256_inserti128_si256(_mm256_castsi128_si256(filt_02), filt_12, 0x1);
    __m256i filt_3 = _mm256_inserti128_si256(_mm256_castsi128_si256(filt_03), filt_13, 0x1);

    __m256i res_0 = _mm256_unpacklo_epi32(filt_0, filt_1);
    __m256i res_1 = _mm256_unpacklo_epi32(filt_2, filt_3);
    __m256i res_2 = _mm256_unpackhi_epi32(filt_0, filt_1);
    __m256i res_3 = _mm256_unpackhi_epi32(filt_2, filt_3);

    coeffs[0] = _mm256_unpacklo_epi64(res_0, res_1);
    coeffs[1] = _mm256_unpackhi_epi64(res_0, res_1);
    coeffs[2] = _mm256_unpacklo_epi64(res_2, res_3);
    coeffs[3] = _mm256_unpackhi_epi64(res_2, res_3);

    filt_00 = _mm_loadu_si128((__m128i*)(svt_aom_warped_filter + idx_final[1]));
    filt_01 = _mm_loadu_si128((__m128i*)(svt_aom_warped_filter + idx_final[3]));
    filt_02 = _mm_loadu_si128((__m128i*)(svt_aom_warped_filter + idx_final[5]));
    filt_03 = _mm_loadu_si128((__m128i*)(svt_aom_warped_filter + idx_final[7]));

    filt_10 = _mm_loadu_si128((__m128i*)(svt_aom_warped_filter + idx_final[9]));
    filt_11 = _mm_loadu_si128((__m128i*)(svt_aom_warped_filter + idx_final[11]));
    filt_12 = _mm_loadu_si128((__m128i*)(svt_aom_warped_filter + idx_final[13]));
    filt_13 = _mm_loadu_si128((__m128i*)(svt_aom_warped_filter + idx_final[15]));

    filt_0 = _mm256_inserti128_si256(_mm256_castsi128_si256(filt_00), filt_10, 0x1);
    filt_1 = _mm256_inserti128_si256(_mm256_castsi128_si256(filt_01), filt_11, 0x1);
    filt_2 = _mm256_inserti128_si256(_mm256_castsi128_si256(filt_02), filt_12, 0x1);
    filt_3 = _mm256_inserti128_si256(_mm256_castsi128_si256(filt_03), filt_13, 0x1);

    res_0 = _mm256_unpacklo_epi32(filt_0, filt_1);
    res_1 = _mm256_unpacklo_epi32(filt_2, filt_3);
    res_2 = _mm256_unpackhi_epi32(filt_0, filt_1);
    res_3 = _mm256_unpackhi_epi32(filt_2, filt_3);

    coeffs[4] = _mm256_unpacklo_epi64(res_0, res_1);
    coeffs[5] = _mm256_unpackhi_epi64(res_0, res_1);
    coeffs[6] = _mm256_unpacklo_epi64(res_2, res_3);
    coeffs[7] = _mm256_unpackhi_epi64(res_2, res_3);
}

static INLINE void prepare_vertical_filter_coeffs_delta0_avx2(int gamma, int sy, __m256i* coeffs) {
    //AVX2 Calculation: idx_final[i] = ((sy + i * gamma) >> WARPEDDIFF_PREC_BITS)
    const __m256i idx       = _mm256_setr_epi32(0, 1, 2, 3, 4, 5, 6, 7);
    const __m256i sy_val    = _mm256_set1_epi32(sy);
    __m256i       gamma_val = _mm256_set1_epi32(gamma);
    gamma_val               = _mm256_mullo_epi32(gamma_val, idx);

    DECLARE_ALIGNED(32, int32_t, idx_final[8]);
    _mm256_storeu_si256((__m256i*)idx_final,
                        _mm256_srli_epi32(_mm256_add_epi32(sy_val, gamma_val), WARPEDDIFF_PREC_BITS));

    __m128i filt_00 = _mm_loadu_si128((__m128i*)(svt_aom_warped_filter + idx_final[0]));
    __m128i filt_01 = _mm_loadu_si128((__m128i*)(svt_aom_warped_filter + idx_final[2]));
    __m128i filt_02 = _mm_loadu_si128((__m128i*)(svt_aom_warped_filter + idx_final[4]));
    __m128i filt_03 = _mm_loadu_si128((__m128i*)(svt_aom_warped_filter + idx_final[6]));

    __m256i filt_0 = _mm256_broadcastsi128_si256(filt_00);
    __m256i filt_1 = _mm256_broadcastsi128_si256(filt_01);
    __m256i filt_2 = _mm256_broadcastsi128_si256(filt_02);
    __m256i filt_3 = _mm256_broadcastsi128_si256(filt_03);

    __m256i res_0 = _mm256_unpacklo_epi32(filt_0, filt_1);
    __m256i res_1 = _mm256_unpacklo_epi32(filt_2, filt_3);
    __m256i res_2 = _mm256_unpackhi_epi32(filt_0, filt_1);
    __m256i res_3 = _mm256_unpackhi_epi32(filt_2, filt_3);

    coeffs[0] = _mm256_unpacklo_epi64(res_0, res_1);
    coeffs[1] = _mm256_unpackhi_epi64(res_0, res_1);
    coeffs[2] = _mm256_unpacklo_epi64(res_2, res_3);
    coeffs[3] = _mm256_unpackhi_epi64(res_2, res_3);

    filt_00 = _mm_loadu_si128((__m128i*)(svt_aom_warped_filter + idx_final[1]));
    filt_01 = _mm_loadu_si128((__m128i*)(svt_aom_warped_filter + idx_final[3]));
    filt_02 = _mm_loadu_si128((__m128i*)(svt_aom_warped_filter + idx_final[5]));
    filt_03 = _mm_loadu_si128((__m128i*)(svt_aom_warped_filter + idx_final[7]));

    filt_0 = _mm256_broadcastsi128_si256(filt_00);
    filt_1 = _mm256_broadcastsi128_si256(filt_01);
    filt_2 = _mm256_broadcastsi128_si256(filt_02);
    filt_3 = _mm256_broadcastsi128_si256(filt_03);

    res_0 = _mm256_unpacklo_epi32(filt_0, filt_1);
    res_1 = _mm256_unpacklo_epi32(filt_2, filt_3);
    res_2 = _mm256_unpackhi_epi32(filt_0, filt_1);
    res_3 = _mm256_unpackhi_epi32(filt_2, filt_3);

    coeffs[4] = _mm256_unpacklo_epi64(res_0, res_1);
    coeffs[5] = _mm256_unpackhi_epi64(res_0, res_1);
    coeffs[6] = _mm256_unpacklo_epi64(res_2, res_3);
    coeffs[7] = _mm256_unpackhi_epi64(res_2, res_3);
}

static INLINE void prepare_vertical_filter_coeffs_gamma0_avx2(int delta, int sy, __m256i* coeffs) {
    const __m128i filt_0 = _mm_loadu_si128((__m128i*)(svt_aom_warped_filter + (sy >> WARPEDDIFF_PREC_BITS)));
    const __m128i filt_1 = _mm_loadu_si128((__m128i*)(svt_aom_warped_filter + ((sy + delta) >> WARPEDDIFF_PREC_BITS)));

    __m256i res_0 = _mm256_inserti128_si256(_mm256_castsi128_si256(filt_0), filt_1, 0x1);

    coeffs[0] = _mm256_shuffle_epi8(res_0, _mm256_loadu_si256((__m256i*)shuffle_gamma0_mask0_avx2));
    coeffs[1] = _mm256_shuffle_epi8(res_0, _mm256_loadu_si256((__m256i*)shuffle_gamma0_mask1_avx2));
    coeffs[2] = _mm256_shuffle_epi8(res_0, _mm256_loadu_si256((__m256i*)shuffle_gamma0_mask2_avx2));
    coeffs[3] = _mm256_shuffle_epi8(res_0, _mm256_loadu_si256((__m256i*)shuffle_gamma0_mask3_avx2));

    coeffs[4] = coeffs[0];
    coeffs[5] = coeffs[1];
    coeffs[6] = coeffs[2];
    coeffs[7] = coeffs[3];
}

static INLINE void filter_src_pixels_vertical_avx2(__m256i* horz_out, __m256i* src, __m256i* coeffs, __m256i* res_lo,
                                                   __m256i* res_hi, int row) {
    const __m256i src_6 = horz_out[row + 3];
    const __m256i src_7 = _mm256_permute2x128_si256(horz_out[row + 3], horz_out[row + 4], 0x21);

    src[6] = _mm256_unpacklo_epi16(src_6, src_7);

    const __m256i res_0 = _mm256_madd_epi16(src[0], coeffs[0]);
    const __m256i res_2 = _mm256_madd_epi16(src[2], coeffs[1]);
    const __m256i res_4 = _mm256_madd_epi16(src[4], coeffs[2]);
    const __m256i res_6 = _mm256_madd_epi16(src[6], coeffs[3]);

    const __m256i res_even = _mm256_add_epi32(_mm256_add_epi32(res_0, res_2), _mm256_add_epi32(res_4, res_6));

    src[7] = _mm256_unpackhi_epi16(src_6, src_7);

    const __m256i res_1 = _mm256_madd_epi16(src[1], coeffs[4]);
    const __m256i res_3 = _mm256_madd_epi16(src[3], coeffs[5]);
    const __m256i res_5 = _mm256_madd_epi16(src[5], coeffs[6]);
    const __m256i res_7 = _mm256_madd_epi16(src[7], coeffs[7]);

    const __m256i res_odd = _mm256_add_epi32(_mm256_add_epi32(res_1, res_3), _mm256_add_epi32(res_5, res_7));

    // Rearrange pixels back into the order 0 ... 7
    *res_lo = _mm256_unpacklo_epi32(res_even, res_odd);
    *res_hi = _mm256_unpackhi_epi32(res_even, res_odd);
}

static INLINE void store_vertical_filter_output_avx2(const __m256i* res_lo, const __m256i* res_hi,
                                                     const __m256i* res_add_const, const __m256i* wt,
                                                     const __m256i* res_sub_const, const __m256i* round_bits_const,
                                                     uint8_t* pred, ConvolveParams* conv_params, int i, int j, int k,
                                                     const int reduce_bits_vert, int p_stride, int p_width,
                                                     const int round_bits) {
    __m256i res_lo_1 = *res_lo;
    __m256i res_hi_1 = *res_hi;

    if (conv_params->is_compound) {
        __m128i* const p_0 = (__m128i*)&conv_params->dst[(i + k + 4) * conv_params->dst_stride + j];
        __m128i* const p_1 = (__m128i*)&conv_params->dst[(i + (k + 1) + 4) * conv_params->dst_stride + j];

        res_lo_1 = _mm256_srai_epi32(_mm256_add_epi32(res_lo_1, *res_add_const), reduce_bits_vert);

        const __m256i temp_lo_16 = _mm256_packus_epi32(res_lo_1, res_lo_1);
        __m256i       res_lo_16;
        if (conv_params->do_average) {
            __m128i* const dst8_0 = (__m128i*)&pred[(i + k + 4) * p_stride + j];
            __m128i* const dst8_1 = (__m128i*)&pred[(i + (k + 1) + 4) * p_stride + j];
            const __m128i  p_16_0 = _mm_loadl_epi64(p_0);
            const __m128i  p_16_1 = _mm_loadl_epi64(p_1);
            const __m256i  p_16   = _mm256_inserti128_si256(_mm256_castsi128_si256(p_16_0), p_16_1, 1);
            if (conv_params->use_jnt_comp_avg) {
                const __m256i p_16_lo    = _mm256_unpacklo_epi16(p_16, temp_lo_16);
                const __m256i wt_res_lo  = _mm256_madd_epi16(p_16_lo, *wt);
                const __m256i shifted_32 = _mm256_srai_epi32(wt_res_lo, DIST_PRECISION_BITS);
                res_lo_16                = _mm256_packus_epi32(shifted_32, shifted_32);
            } else {
                res_lo_16 = _mm256_srai_epi16(_mm256_add_epi16(p_16, temp_lo_16), 1);
            }
            res_lo_16                = _mm256_add_epi16(res_lo_16, *res_sub_const);
            res_lo_16                = _mm256_srai_epi16(_mm256_add_epi16(res_lo_16, *round_bits_const), round_bits);
            const __m256i res_8_lo   = _mm256_packus_epi16(res_lo_16, res_lo_16);
            const __m128i res_8_lo_0 = _mm256_castsi256_si128(res_8_lo);
            const __m128i res_8_lo_1 = _mm256_extracti128_si256(res_8_lo, 1);
            *(uint32_t*)dst8_0       = _mm_cvtsi128_si32(res_8_lo_0);
            *(uint32_t*)dst8_1       = _mm_cvtsi128_si32(res_8_lo_1);
        } else {
            const __m128i temp_lo_16_0 = _mm256_castsi256_si128(temp_lo_16);
            const __m128i temp_lo_16_1 = _mm256_extracti128_si256(temp_lo_16, 1);
            _mm_storel_epi64(p_0, temp_lo_16_0);
            _mm_storel_epi64(p_1, temp_lo_16_1);
        }
        if (p_width > 4) {
            __m128i* const p4_0      = (__m128i*)&conv_params->dst[(i + k + 4) * conv_params->dst_stride + j + 4];
            __m128i* const p4_1      = (__m128i*)&conv_params->dst[(i + (k + 1) + 4) * conv_params->dst_stride + j + 4];
            res_hi_1                 = _mm256_srai_epi32(_mm256_add_epi32(res_hi_1, *res_add_const), reduce_bits_vert);
            const __m256i temp_hi_16 = _mm256_packus_epi32(res_hi_1, res_hi_1);
            __m256i       res_hi_16;
            if (conv_params->do_average) {
                __m128i* const dst8_4_0 = (__m128i*)&pred[(i + k + 4) * p_stride + j + 4];
                __m128i* const dst8_4_1 = (__m128i*)&pred[(i + (k + 1) + 4) * p_stride + j + 4];
                const __m128i  p4_16_0  = _mm_loadl_epi64(p4_0);
                const __m128i  p4_16_1  = _mm_loadl_epi64(p4_1);
                const __m256i  p4_16    = _mm256_inserti128_si256(_mm256_castsi128_si256(p4_16_0), p4_16_1, 1);
                if (conv_params->use_jnt_comp_avg) {
                    const __m256i p_16_hi    = _mm256_unpacklo_epi16(p4_16, temp_hi_16);
                    const __m256i wt_res_hi  = _mm256_madd_epi16(p_16_hi, *wt);
                    const __m256i shifted_32 = _mm256_srai_epi32(wt_res_hi, DIST_PRECISION_BITS);
                    res_hi_16                = _mm256_packus_epi32(shifted_32, shifted_32);
                } else {
                    res_hi_16 = _mm256_srai_epi16(_mm256_add_epi16(p4_16, temp_hi_16), 1);
                }
                res_hi_16              = _mm256_add_epi16(res_hi_16, *res_sub_const);
                res_hi_16              = _mm256_srai_epi16(_mm256_add_epi16(res_hi_16, *round_bits_const), round_bits);
                __m256i       res_8_hi = _mm256_packus_epi16(res_hi_16, res_hi_16);
                const __m128i res_8_hi_0 = _mm256_castsi256_si128(res_8_hi);
                const __m128i res_8_hi_1 = _mm256_extracti128_si256(res_8_hi, 1);
                *(uint32_t*)dst8_4_0     = _mm_cvtsi128_si32(res_8_hi_0);
                *(uint32_t*)dst8_4_1     = _mm_cvtsi128_si32(res_8_hi_1);
            } else {
                const __m128i temp_hi_16_0 = _mm256_castsi256_si128(temp_hi_16);
                const __m128i temp_hi_16_1 = _mm256_extracti128_si256(temp_hi_16, 1);
                _mm_storel_epi64(p4_0, temp_hi_16_0);
                _mm_storel_epi64(p4_1, temp_hi_16_1);
            }
        }
    } else {
        const __m256i res_lo_round = _mm256_srai_epi32(_mm256_add_epi32(res_lo_1, *res_add_const), reduce_bits_vert);
        const __m256i res_hi_round = _mm256_srai_epi32(_mm256_add_epi32(res_hi_1, *res_add_const), reduce_bits_vert);

        const __m256i res_16bit = _mm256_packs_epi32(res_lo_round, res_hi_round);
        const __m256i res_8bit  = _mm256_packus_epi16(res_16bit, res_16bit);
        const __m128i res_8bit0 = _mm256_castsi256_si128(res_8bit);
        const __m128i res_8bit1 = _mm256_extracti128_si256(res_8bit, 1);

        // Store, blending with 'pred' if needed
        __m128i* const p  = (__m128i*)&pred[(i + k + 4) * p_stride + j];
        __m128i* const p1 = (__m128i*)&pred[(i + (k + 1) + 4) * p_stride + j];

        if (p_width == 4) {
            *(uint32_t*)p  = _mm_cvtsi128_si32(res_8bit0);
            *(uint32_t*)p1 = _mm_cvtsi128_si32(res_8bit1);
        } else {
            _mm_storel_epi64(p, res_8bit0);
            _mm_storel_epi64(p1, res_8bit1);
        }
    }
}

static INLINE void warp_vertical_filter_avx2(uint8_t* pred, __m256i* horz_out, ConvolveParams* conv_params,
                                             int16_t gamma, int16_t delta, int p_height, int p_stride, int p_width,
                                             int i, int j, int sy4, const int reduce_bits_vert,
                                             const __m256i* res_add_const, const int round_bits,
                                             const __m256i* res_sub_const, const __m256i* round_bits_const,
                                             const __m256i* wt) {
    int           k, row = 0;
    __m256i       src[8];
    const __m256i src_0 = horz_out[0];
    const __m256i src_1 = _mm256_permute2x128_si256(horz_out[0], horz_out[1], 0x21);
    const __m256i src_2 = horz_out[1];
    const __m256i src_3 = _mm256_permute2x128_si256(horz_out[1], horz_out[2], 0x21);
    const __m256i src_4 = horz_out[2];
    const __m256i src_5 = _mm256_permute2x128_si256(horz_out[2], horz_out[3], 0x21);

    src[0] = _mm256_unpacklo_epi16(src_0, src_1);
    src[2] = _mm256_unpacklo_epi16(src_2, src_3);
    src[4] = _mm256_unpacklo_epi16(src_4, src_5);

    src[1] = _mm256_unpackhi_epi16(src_0, src_1);
    src[3] = _mm256_unpackhi_epi16(src_2, src_3);
    src[5] = _mm256_unpackhi_epi16(src_4, src_5);

    for (k = -4; k < AOMMIN(4, p_height - i - 4); k += 2) {
        int     sy = sy4 + delta * (k + 4);
        __m256i coeffs[8];
        prepare_vertical_filter_coeffs_avx2(gamma, delta, sy, coeffs);
        __m256i res_lo, res_hi;
        filter_src_pixels_vertical_avx2(horz_out, src, coeffs, &res_lo, &res_hi, row);
        store_vertical_filter_output_avx2(&res_lo,
                                          &res_hi,
                                          res_add_const,
                                          wt,
                                          res_sub_const,
                                          round_bits_const,
                                          pred,
                                          conv_params,
                                          i,
                                          j,
                                          k,
                                          reduce_bits_vert,
                                          p_stride,
                                          p_width,
                                          round_bits);
        src[0] = src[2];
        src[2] = src[4];
        src[4] = src[6];
        src[1] = src[3];
        src[3] = src[5];
        src[5] = src[7];

        row += 1;
    }
}

static INLINE void warp_vertical_filter_gamma0_avx2(uint8_t* pred, __m256i* horz_out, ConvolveParams* conv_params,
                                                    int16_t gamma, int16_t delta, int p_height, int p_stride,
                                                    int p_width, int i, int j, int sy4, const int reduce_bits_vert,
                                                    const __m256i* res_add_const, const int round_bits,
                                                    const __m256i* res_sub_const, const __m256i* round_bits_const,
                                                    const __m256i* wt) {
    (void)gamma;
    int           k, row = 0;
    __m256i       src[8];
    const __m256i src_0 = horz_out[0];
    const __m256i src_1 = _mm256_permute2x128_si256(horz_out[0], horz_out[1], 0x21);
    const __m256i src_2 = horz_out[1];
    const __m256i src_3 = _mm256_permute2x128_si256(horz_out[1], horz_out[2], 0x21);
    const __m256i src_4 = horz_out[2];
    const __m256i src_5 = _mm256_permute2x128_si256(horz_out[2], horz_out[3], 0x21);

    src[0] = _mm256_unpacklo_epi16(src_0, src_1);
    src[2] = _mm256_unpacklo_epi16(src_2, src_3);
    src[4] = _mm256_unpacklo_epi16(src_4, src_5);

    src[1] = _mm256_unpackhi_epi16(src_0, src_1);
    src[3] = _mm256_unpackhi_epi16(src_2, src_3);
    src[5] = _mm256_unpackhi_epi16(src_4, src_5);

    for (k = -4; k < AOMMIN(4, p_height - i - 4); k += 2) {
        int     sy = sy4 + delta * (k + 4);
        __m256i coeffs[8];
        prepare_vertical_filter_coeffs_gamma0_avx2(delta, sy, coeffs);
        __m256i res_lo, res_hi;
        filter_src_pixels_vertical_avx2(horz_out, src, coeffs, &res_lo, &res_hi, row);
        store_vertical_filter_output_avx2(&res_lo,
                                          &res_hi,
                                          res_add_const,
                                          wt,
                                          res_sub_const,
                                          round_bits_const,
                                          pred,
                                          conv_params,
                                          i,
                                          j,
                                          k,
                                          reduce_bits_vert,
                                          p_stride,
                                          p_width,
                                          round_bits);
        src[0] = src[2];
        src[2] = src[4];
        src[4] = src[6];
        src[1] = src[3];
        src[3] = src[5];
        src[5] = src[7];
        row += 1;
    }
}

static INLINE void warp_vertical_filter_delta0_avx2(uint8_t* pred, __m256i* horz_out, ConvolveParams* conv_params,
                                                    int16_t gamma, int16_t delta, int p_height, int p_stride,
                                                    int p_width, int i, int j, int sy4, const int reduce_bits_vert,
                                                    const __m256i* res_add_const, const int round_bits,
                                                    const __m256i* res_sub_const, const __m256i* round_bits_const,
                                                    const __m256i* wt) {
    (void)delta;
    int           k, row = 0;
    __m256i       src[8], coeffs[8];
    const __m256i src_0 = horz_out[0];
    const __m256i src_1 = _mm256_permute2x128_si256(horz_out[0], horz_out[1], 0x21);
    const __m256i src_2 = horz_out[1];
    const __m256i src_3 = _mm256_permute2x128_si256(horz_out[1], horz_out[2], 0x21);
    const __m256i src_4 = horz_out[2];
    const __m256i src_5 = _mm256_permute2x128_si256(horz_out[2], horz_out[3], 0x21);

    src[0] = _mm256_unpacklo_epi16(src_0, src_1);
    src[2] = _mm256_unpacklo_epi16(src_2, src_3);
    src[4] = _mm256_unpacklo_epi16(src_4, src_5);

    src[1] = _mm256_unpackhi_epi16(src_0, src_1);
    src[3] = _mm256_unpackhi_epi16(src_2, src_3);
    src[5] = _mm256_unpackhi_epi16(src_4, src_5);

    prepare_vertical_filter_coeffs_delta0_avx2(gamma, sy4, coeffs);

    for (k = -4; k < AOMMIN(4, p_height - i - 4); k += 2) {
        __m256i res_lo, res_hi;
        filter_src_pixels_vertical_avx2(horz_out, src, coeffs, &res_lo, &res_hi, row);
        store_vertical_filter_output_avx2(&res_lo,
                                          &res_hi,
                                          res_add_const,
                                          wt,
                                          res_sub_const,
                                          round_bits_const,
                                          pred,
                                          conv_params,
                                          i,
                                          j,
                                          k,
                                          reduce_bits_vert,
                                          p_stride,
                                          p_width,
                                          round_bits);
        src[0] = src[2];
        src[2] = src[4];
        src[4] = src[6];
        src[1] = src[3];
        src[3] = src[5];
        src[5] = src[7];
        row += 1;
    }
}

static INLINE void warp_vertical_filter_gamma0_delta0_avx2(
    uint8_t* pred, __m256i* horz_out, ConvolveParams* conv_params, int16_t gamma, int16_t delta, int p_height,
    int p_stride, int p_width, int i, int j, int sy4, const int reduce_bits_vert, const __m256i* res_add_const,
    const int round_bits, const __m256i* res_sub_const, const __m256i* round_bits_const, const __m256i* wt) {
    (void)gamma;
    int           k, row = 0;
    __m256i       src[8], coeffs[8];
    const __m256i src_0 = horz_out[0];
    const __m256i src_1 = _mm256_permute2x128_si256(horz_out[0], horz_out[1], 0x21);
    const __m256i src_2 = horz_out[1];
    const __m256i src_3 = _mm256_permute2x128_si256(horz_out[1], horz_out[2], 0x21);
    const __m256i src_4 = horz_out[2];
    const __m256i src_5 = _mm256_permute2x128_si256(horz_out[2], horz_out[3], 0x21);

    src[0] = _mm256_unpacklo_epi16(src_0, src_1);
    src[2] = _mm256_unpacklo_epi16(src_2, src_3);
    src[4] = _mm256_unpacklo_epi16(src_4, src_5);

    src[1] = _mm256_unpackhi_epi16(src_0, src_1);
    src[3] = _mm256_unpackhi_epi16(src_2, src_3);
    src[5] = _mm256_unpackhi_epi16(src_4, src_5);

    prepare_vertical_filter_coeffs_gamma0_avx2(delta, sy4, coeffs);

    for (k = -4; k < AOMMIN(4, p_height - i - 4); k += 2) {
        __m256i res_lo, res_hi;
        filter_src_pixels_vertical_avx2(horz_out, src, coeffs, &res_lo, &res_hi, row);
        store_vertical_filter_output_avx2(&res_lo,
                                          &res_hi,
                                          res_add_const,
                                          wt,
                                          res_sub_const,
                                          round_bits_const,
                                          pred,
                                          conv_params,
                                          i,
                                          j,
                                          k,
                                          reduce_bits_vert,
                                          p_stride,
                                          p_width,
                                          round_bits);
        src[0] = src[2];
        src[2] = src[4];
        src[4] = src[6];
        src[1] = src[3];
        src[3] = src[5];
        src[5] = src[7];
        row += 1;
    }
}

static INLINE void prepare_warp_vertical_filter_avx2(uint8_t* pred, __m256i* horz_out, ConvolveParams* conv_params,
                                                     int16_t gamma, int16_t delta, int p_height, int p_stride,
                                                     int p_width, int i, int j, int sy4, const int reduce_bits_vert,
                                                     const __m256i* res_add_const, const int round_bits,
                                                     const __m256i* res_sub_const, const __m256i* round_bits_const,
                                                     const __m256i* wt) {
    if (gamma == 0 && delta == 0) {
        warp_vertical_filter_gamma0_delta0_avx2(pred,
                                                horz_out,
                                                conv_params,
                                                gamma,
                                                delta,
                                                p_height,
                                                p_stride,
                                                p_width,
                                                i,
                                                j,
                                                sy4,
                                                reduce_bits_vert,
                                                res_add_const,
                                                round_bits,
                                                res_sub_const,
                                                round_bits_const,
                                                wt);
    } else if (gamma == 0 && delta != 0) {
        warp_vertical_filter_gamma0_avx2(pred,
                                         horz_out,
                                         conv_params,
                                         gamma,
                                         delta,
                                         p_height,
                                         p_stride,
                                         p_width,
                                         i,
                                         j,
                                         sy4,
                                         reduce_bits_vert,
                                         res_add_const,
                                         round_bits,
                                         res_sub_const,
                                         round_bits_const,
                                         wt);
    } else if (gamma != 0 && delta == 0) {
        warp_vertical_filter_delta0_avx2(pred,
                                         horz_out,
                                         conv_params,
                                         gamma,
                                         delta,
                                         p_height,
                                         p_stride,
                                         p_width,
                                         i,
                                         j,
                                         sy4,
                                         reduce_bits_vert,
                                         res_add_const,
                                         round_bits,
                                         res_sub_const,
                                         round_bits_const,
                                         wt);
    } else {
        warp_vertical_filter_avx2(pred,
                                  horz_out,
                                  conv_params,
                                  gamma,
                                  delta,
                                  p_height,
                                  p_stride,
                                  p_width,
                                  i,
                                  j,
                                  sy4,
                                  reduce_bits_vert,
                                  res_add_const,
                                  round_bits,
                                  res_sub_const,
                                  round_bits_const,
                                  wt);
    }
}

static INLINE void prepare_warp_horizontal_filter_avx2(const uint8_t* ref, __m256i* horz_out, int stride, int32_t ix4,
                                                       int32_t iy4, int32_t sx4, int alpha, int beta, int p_height,
                                                       int height, int i, const __m256i* round_const,
                                                       const __m128i* shift, const __m256i* shuffle_src) {
    if (alpha == 0 && beta == 0) {
        warp_horizontal_filter_alpha0_beta0_avx2(
            ref, horz_out, stride, ix4, iy4, sx4, alpha, beta, p_height, height, i, round_const, shift, shuffle_src);
    } else if (alpha == 0 && beta != 0) {
        warp_horizontal_filter_alpha0_avx2(
            ref, horz_out, stride, ix4, iy4, sx4, alpha, beta, p_height, height, i, round_const, shift, shuffle_src);
    } else if (alpha != 0 && beta == 0) {
        warp_horizontal_filter_beta0_avx2(
            ref, horz_out, stride, ix4, iy4, sx4, alpha, beta, p_height, height, i, round_const, shift, shuffle_src);
    } else {
        warp_horizontal_filter_avx2(
            ref, horz_out, stride, ix4, iy4, sx4, alpha, beta, p_height, height, i, round_const, shift, shuffle_src);
    }
}

void svt_av1_warp_affine_avx2(const int32_t* mat, const uint8_t* ref, int width, int height, int stride, uint8_t* pred,
                              int p_col, int p_row, int p_width, int p_height, int p_stride, int subsampling_x,
                              int subsampling_y, ConvolveParams* conv_params, int16_t alpha, int16_t beta,
                              int16_t gamma, int16_t delta) {
    __m256i   horz_out[8];
    int       i, j, k;
    const int bd                = 8;
    const int reduce_bits_horiz = conv_params->round_0;
    const int reduce_bits_vert  = conv_params->is_compound ? conv_params->round_1 : 2 * FILTER_BITS - reduce_bits_horiz;
    const int offset_bits_horiz = bd + FILTER_BITS - 1;
    assert(IMPLIES(conv_params->is_compound, conv_params->dst != NULL));

    const int     offset_bits_vert       = bd + 2 * FILTER_BITS - reduce_bits_horiz;
    const __m256i reduce_bits_vert_const = _mm256_set1_epi32(((1 << reduce_bits_vert) >> 1));
    const __m256i res_add_const          = _mm256_set1_epi32(1 << offset_bits_vert);
    const int     round_bits             = 2 * FILTER_BITS - conv_params->round_0 - conv_params->round_1;
    const int     offset_bits            = bd + 2 * FILTER_BITS - conv_params->round_0;
    assert(IMPLIES(conv_params->do_average, conv_params->is_compound));

    const __m256i round_const = _mm256_set1_epi16((1 << offset_bits_horiz) + ((1 << reduce_bits_horiz) >> 1));
    const __m128i shift       = _mm_cvtsi32_si128(reduce_bits_horiz);

    __m256i res_sub_const, round_bits_const, wt;
    unpack_weights_and_set_round_const_avx2(
        conv_params, round_bits, offset_bits, &res_sub_const, &round_bits_const, &wt);

    __m256i res_add_const_1;
    if (conv_params->is_compound == 1) {
        res_add_const_1 = _mm256_add_epi32(reduce_bits_vert_const, res_add_const);
    } else {
        res_add_const_1 = _mm256_set1_epi32(-(1 << (bd + reduce_bits_vert - 1)) + ((1 << reduce_bits_vert) >> 1));
    }
    const int32_t const1 = alpha * (-4) + beta * (-4) + (1 << (WARPEDDIFF_PREC_BITS - 1)) +
        (WARPEDPIXEL_PREC_SHIFTS << WARPEDDIFF_PREC_BITS);
    const int32_t const2 = gamma * (-4) + delta * (-4) + (1 << (WARPEDDIFF_PREC_BITS - 1)) +
        (WARPEDPIXEL_PREC_SHIFTS << WARPEDDIFF_PREC_BITS);
    const int32_t const3 = ((1 << WARP_PARAM_REDUCE_BITS) - 1);
    const int16_t const4 = (1 << (bd + FILTER_BITS - reduce_bits_horiz - 1));
    const int16_t const5 = (1 << (FILTER_BITS - reduce_bits_horiz));

    __m256i shuffle_src[4];
    shuffle_src[0] = _mm256_loadu_si256((__m256i*)shuffle_src0);
    shuffle_src[1] = _mm256_loadu_si256((__m256i*)shuffle_src1);
    shuffle_src[2] = _mm256_loadu_si256((__m256i*)shuffle_src2);
    shuffle_src[3] = _mm256_loadu_si256((__m256i*)shuffle_src3);

    for (i = 0; i < p_height; i += 8) {
        for (j = 0; j < p_width; j += 8) {
            const int32_t src_x = (p_col + j + 4) << subsampling_x;
            const int32_t src_y = (p_row + i + 4) << subsampling_y;
            const int32_t dst_x = mat[2] * src_x + mat[3] * src_y + mat[0];
            const int32_t dst_y = mat[4] * src_x + mat[5] * src_y + mat[1];
            const int32_t x4    = dst_x >> subsampling_x;
            const int32_t y4    = dst_y >> subsampling_y;

            int32_t ix4 = x4 >> WARPEDMODEL_PREC_BITS;
            int32_t sx4 = x4 & ((1 << WARPEDMODEL_PREC_BITS) - 1);
            int32_t iy4 = y4 >> WARPEDMODEL_PREC_BITS;
            int32_t sy4 = y4 & ((1 << WARPEDMODEL_PREC_BITS) - 1);

            // Add in all the constant terms, including rounding and offset
            sx4 += const1;
            sy4 += const2;

            sx4 &= ~const3;
            sy4 &= ~const3;

            // Horizontal filter
            // If the block is aligned such that, after clamping, every sample
            // would be taken from the leftmost/rightmost column, then we can
            // skip the expensive horizontal filter.

            if (ix4 <= -7) {
                int iy, row = 0;
                for (k = -7; k <= (AOMMIN(8, p_height - i) - 2); k += 2) {
                    iy                   = iy4 + k;
                    iy                   = clamp(iy, 0, height - 1);
                    const __m256i temp_0 = _mm256_set1_epi16(const4 + ref[iy * stride] * const5);
                    iy                   = iy4 + k + 1;
                    iy                   = clamp(iy, 0, height - 1);
                    const __m256i temp_1 = _mm256_set1_epi16(const4 + ref[iy * stride] * const5);
                    horz_out[row]        = _mm256_blend_epi32(temp_0, temp_1, 0xf0);
                    row += 1;
                }
                iy            = iy4 + k;
                iy            = clamp(iy, 0, height - 1);
                horz_out[row] = _mm256_set1_epi16(const4 + ref[iy * stride] * const5);
            } else if (ix4 >= width + 6) {
                int iy, row = 0;
                for (k = -7; k <= (AOMMIN(8, p_height - i) - 2); k += 2) {
                    iy                   = iy4 + k;
                    iy                   = clamp(iy, 0, height - 1);
                    const __m256i temp_0 = _mm256_set1_epi16(const4 + ref[iy * stride + (width - 1)] * const5);
                    iy                   = iy4 + k + 1;
                    iy                   = clamp(iy, 0, height - 1);
                    const __m256i temp_1 = _mm256_set1_epi16(const4 + ref[iy * stride + (width - 1)] * const5);
                    horz_out[row]        = _mm256_blend_epi32(temp_0, temp_1, 0xf0);
                    row += 1;
                }
                iy            = iy4 + k;
                iy            = clamp(iy, 0, height - 1);
                horz_out[row] = _mm256_set1_epi16(const4 + ref[iy * stride + (width - 1)] * const5);
            } else if (((ix4 - 7) < 0) || ((ix4 + 9) > width)) {
                const int out_of_boundary_left  = -(ix4 - 6);
                const int out_of_boundary_right = (ix4 + 8) - width;
                int       iy, sx, row = 0;
                for (k = -7; k <= (AOMMIN(8, p_height - i) - 2); k += 2) {
                    iy           = iy4 + k;
                    iy           = clamp(iy, 0, height - 1);
                    __m128i src0 = _mm_loadu_si128((__m128i*)(ref + iy * stride + ix4 - 7));
                    iy           = iy4 + k + 1;
                    iy           = clamp(iy, 0, height - 1);
                    __m128i src1 = _mm_loadu_si128((__m128i*)(ref + iy * stride + ix4 - 7));

                    if (out_of_boundary_left >= 0) {
                        const __m128i shuffle_reg_left = _mm_loadu_si128((__m128i*)warp_pad_left[out_of_boundary_left]);
                        src0                           = _mm_shuffle_epi8(src0, shuffle_reg_left);
                        src1                           = _mm_shuffle_epi8(src1, shuffle_reg_left);
                    }
                    if (out_of_boundary_right >= 0) {
                        const __m128i shuffle_reg_right = _mm_loadu_si128(
                            (__m128i*)warp_pad_right[out_of_boundary_right]);
                        src0 = _mm_shuffle_epi8(src0, shuffle_reg_right);
                        src1 = _mm_shuffle_epi8(src1, shuffle_reg_right);
                    }
                    sx                   = sx4 + beta * (k + 4);
                    const __m256i src_01 = _mm256_inserti128_si256(_mm256_castsi128_si256(src0), src1, 0x1);
                    horizontal_filter_avx2(src_01, horz_out, sx, alpha, beta, row, shuffle_src, &round_const, &shift);
                    row += 1;
                }
                iy          = iy4 + k;
                iy          = clamp(iy, 0, height - 1);
                __m128i src = _mm_loadu_si128((__m128i*)(ref + iy * stride + ix4 - 7));
                if (out_of_boundary_left >= 0) {
                    const __m128i shuffle_reg_left = _mm_loadu_si128((__m128i*)warp_pad_left[out_of_boundary_left]);
                    src                            = _mm_shuffle_epi8(src, shuffle_reg_left);
                }
                if (out_of_boundary_right >= 0) {
                    const __m128i shuffle_reg_right = _mm_loadu_si128((__m128i*)warp_pad_right[out_of_boundary_right]);
                    src                             = _mm_shuffle_epi8(src, shuffle_reg_right);
                }
                sx                   = sx4 + beta * (k + 4);
                const __m256i src_01 = _mm256_castsi128_si256(src);
                __m256i       coeff[4];
                prepare_horizontal_filter_coeff(alpha, sx, coeff);
                filter_src_pixels_avx2(src_01, horz_out, coeff, shuffle_src, &round_const, &shift, row);
            } else {
                prepare_warp_horizontal_filter_avx2(ref,
                                                    horz_out,
                                                    stride,
                                                    ix4,
                                                    iy4,
                                                    sx4,
                                                    alpha,
                                                    beta,
                                                    p_height,
                                                    height,
                                                    i,
                                                    &round_const,
                                                    &shift,
                                                    shuffle_src);
            }

            // Vertical filter
            prepare_warp_vertical_filter_avx2(pred,
                                              horz_out,
                                              conv_params,
                                              gamma,
                                              delta,
                                              p_height,
                                              p_stride,
                                              p_width,
                                              i,
                                              j,
                                              sy4,
                                              reduce_bits_vert,
                                              &res_add_const_1,
                                              round_bits,
                                              &res_sub_const,
                                              &round_bits_const,
                                              &wt);
        }
    }
}
