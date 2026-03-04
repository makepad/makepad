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

#ifndef EbCabacContextModel_h
#define EbCabacContextModel_h

#include "definitions.h"
#include "common_dsp_rtcd.h"
#ifdef __cplusplus
extern "C" {
#endif

#define TOTAL_NUMBER_OF_QP_VALUES 64 // qp range is 0 to 63

#define TOTAL_NUMBER_OF_SLICE_TYPES 3 // I, P and b
/**********************************************************************************************************************/
/**********************************************************************************************************************/
/**********************************************************************************************************************/
/********************************************************************************************************************************/
//prob.h

typedef uint16_t AomCdfProb;

typedef struct {
    AomCdfProb* color_map_cdf;
    uint8_t     token;
} TOKENEXTRA;

#define CDF_SIZE(x) ((x) + 1)
#define CDF_PROB_BITS 15
#define CDF_PROB_TOP (1 << CDF_PROB_BITS)
#define CDF_INIT_TOP 32768
#define CDF_SHIFT (15 - CDF_PROB_BITS)
/*The value stored in an iCDF is CDF_PROB_TOP minus the actual cumulative
    probability (an "inverse" CDF).
    This function converts from one representation to the other (and is its own
    inverse).*/
#define AOM_ICDF(x) (CDF_PROB_TOP - (x))

#define SEG_TEMPORAL_PRED_CTXS 3
#define SPATIAL_PREDICTION_PROBS 3
#define SEG_TREE_PROBS (MAX_SEGMENTS - 1)

#if CDF_SHIFT == 0

#define AOM_CDF2(a0) AOM_ICDF(a0), AOM_ICDF(CDF_PROB_TOP), 0
#define AOM_CDF3(a0, a1) AOM_ICDF(a0), AOM_ICDF(a1), AOM_ICDF(CDF_PROB_TOP), 0
#define AOM_CDF4(a0, a1, a2) AOM_ICDF(a0), AOM_ICDF(a1), AOM_ICDF(a2), AOM_ICDF(CDF_PROB_TOP), 0
#define AOM_CDF5(a0, a1, a2, a3) \
    AOM_ICDF(a0)                 \
    , AOM_ICDF(a1), AOM_ICDF(a2), AOM_ICDF(a3), AOM_ICDF(CDF_PROB_TOP), 0
#define AOM_CDF6(a0, a1, a2, a3, a4) \
    AOM_ICDF(a0)                     \
    , AOM_ICDF(a1), AOM_ICDF(a2), AOM_ICDF(a3), AOM_ICDF(a4), AOM_ICDF(CDF_PROB_TOP), 0
#define AOM_CDF7(a0, a1, a2, a3, a4, a5) \
    AOM_ICDF(a0)                         \
    , AOM_ICDF(a1), AOM_ICDF(a2), AOM_ICDF(a3), AOM_ICDF(a4), AOM_ICDF(a5), AOM_ICDF(CDF_PROB_TOP), 0
#define AOM_CDF8(a0, a1, a2, a3, a4, a5, a6) \
    AOM_ICDF(a0)                             \
    , AOM_ICDF(a1), AOM_ICDF(a2), AOM_ICDF(a3), AOM_ICDF(a4), AOM_ICDF(a5), AOM_ICDF(a6), AOM_ICDF(CDF_PROB_TOP), 0
#define AOM_CDF9(a0, a1, a2, a3, a4, a5, a6, a7)                                                        \
    AOM_ICDF(a0)                                                                                        \
    , AOM_ICDF(a1), AOM_ICDF(a2), AOM_ICDF(a3), AOM_ICDF(a4), AOM_ICDF(a5), AOM_ICDF(a6), AOM_ICDF(a7), \
        AOM_ICDF(CDF_PROB_TOP), 0
#define AOM_CDF10(a0, a1, a2, a3, a4, a5, a6, a7, a8)                                                                 \
    AOM_ICDF(a0)                                                                                                      \
    , AOM_ICDF(a1), AOM_ICDF(a2), AOM_ICDF(a3), AOM_ICDF(a4), AOM_ICDF(a5), AOM_ICDF(a6), AOM_ICDF(a7), AOM_ICDF(a8), \
        AOM_ICDF(CDF_PROB_TOP), 0
#define AOM_CDF11(a0, a1, a2, a3, a4, a5, a6, a7, a8, a9)                                                             \
    AOM_ICDF(a0)                                                                                                      \
    , AOM_ICDF(a1), AOM_ICDF(a2), AOM_ICDF(a3), AOM_ICDF(a4), AOM_ICDF(a5), AOM_ICDF(a6), AOM_ICDF(a7), AOM_ICDF(a8), \
        AOM_ICDF(a9), AOM_ICDF(CDF_PROB_TOP), 0
#define AOM_CDF12(a0, a1, a2, a3, a4, a5, a6, a7, a8, a9, a10)                                                        \
    AOM_ICDF(a0)                                                                                                      \
    , AOM_ICDF(a1), AOM_ICDF(a2), AOM_ICDF(a3), AOM_ICDF(a4), AOM_ICDF(a5), AOM_ICDF(a6), AOM_ICDF(a7), AOM_ICDF(a8), \
        AOM_ICDF(a9), AOM_ICDF(a10), AOM_ICDF(CDF_PROB_TOP), 0
#define AOM_CDF13(a0, a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11)                                                   \
    AOM_ICDF(a0)                                                                                                      \
    , AOM_ICDF(a1), AOM_ICDF(a2), AOM_ICDF(a3), AOM_ICDF(a4), AOM_ICDF(a5), AOM_ICDF(a6), AOM_ICDF(a7), AOM_ICDF(a8), \
        AOM_ICDF(a9), AOM_ICDF(a10), AOM_ICDF(a11), AOM_ICDF(CDF_PROB_TOP), 0
#define AOM_CDF14(a0, a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12)                                              \
    AOM_ICDF(a0)                                                                                                      \
    , AOM_ICDF(a1), AOM_ICDF(a2), AOM_ICDF(a3), AOM_ICDF(a4), AOM_ICDF(a5), AOM_ICDF(a6), AOM_ICDF(a7), AOM_ICDF(a8), \
        AOM_ICDF(a9), AOM_ICDF(a10), AOM_ICDF(a11), AOM_ICDF(a12), AOM_ICDF(CDF_PROB_TOP), 0
#define AOM_CDF15(a0, a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12, a13)                                         \
    AOM_ICDF(a0)                                                                                                      \
    , AOM_ICDF(a1), AOM_ICDF(a2), AOM_ICDF(a3), AOM_ICDF(a4), AOM_ICDF(a5), AOM_ICDF(a6), AOM_ICDF(a7), AOM_ICDF(a8), \
        AOM_ICDF(a9), AOM_ICDF(a10), AOM_ICDF(a11), AOM_ICDF(a12), AOM_ICDF(a13), AOM_ICDF(CDF_PROB_TOP), 0
#define AOM_CDF16(a0, a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12, a13, a14)                                    \
    AOM_ICDF(a0)                                                                                                      \
    , AOM_ICDF(a1), AOM_ICDF(a2), AOM_ICDF(a3), AOM_ICDF(a4), AOM_ICDF(a5), AOM_ICDF(a6), AOM_ICDF(a7), AOM_ICDF(a8), \
        AOM_ICDF(a9), AOM_ICDF(a10), AOM_ICDF(a11), AOM_ICDF(a12), AOM_ICDF(a13), AOM_ICDF(a14),                      \
        AOM_ICDF(CDF_PROB_TOP), 0

#else
#define AOM_CDF2(a0)                                                                                                  \
    AOM_ICDF((((a0) - 1) * ((CDF_INIT_TOP >> CDF_SHIFT) - 2) + ((CDF_INIT_TOP - 2) >> 1)) / ((CDF_INIT_TOP - 2)) + 1) \
    , AOM_ICDF(CDF_PROB_TOP), 0
#define AOM_CDF3(a0, a1)                                                                                               \
    AOM_ICDF((((a0) - 1) * ((CDF_INIT_TOP >> CDF_SHIFT) - 3) + ((CDF_INIT_TOP - 3) >> 1)) / ((CDF_INIT_TOP - 3)) + 1)  \
    ,                                                                                                                  \
        AOM_ICDF((((a1) - 2) * ((CDF_INIT_TOP >> CDF_SHIFT) - 3) + ((CDF_INIT_TOP - 3) >> 1)) / ((CDF_INIT_TOP - 3)) + \
                 2),                                                                                                   \
        AOM_ICDF(CDF_PROB_TOP), 0
#define AOM_CDF4(a0, a1, a2)                                                                                           \
    AOM_ICDF((((a0) - 1) * ((CDF_INIT_TOP >> CDF_SHIFT) - 4) + ((CDF_INIT_TOP - 4) >> 1)) / ((CDF_INIT_TOP - 4)) + 1)  \
    ,                                                                                                                  \
        AOM_ICDF((((a1) - 2) * ((CDF_INIT_TOP >> CDF_SHIFT) - 4) + ((CDF_INIT_TOP - 4) >> 1)) / ((CDF_INIT_TOP - 4)) + \
                 2),                                                                                                   \
        AOM_ICDF((((a2) - 3) * ((CDF_INIT_TOP >> CDF_SHIFT) - 4) + ((CDF_INIT_TOP - 4) >> 1)) / ((CDF_INIT_TOP - 4)) + \
                 3),                                                                                                   \
        AOM_ICDF(CDF_PROB_TOP), 0
#define AOM_CDF5(a0, a1, a2, a3)                                                                                       \
    AOM_ICDF((((a0) - 1) * ((CDF_INIT_TOP >> CDF_SHIFT) - 5) + ((CDF_INIT_TOP - 5) >> 1)) / ((CDF_INIT_TOP - 5)) + 1)  \
    ,                                                                                                                  \
        AOM_ICDF((((a1) - 2) * ((CDF_INIT_TOP >> CDF_SHIFT) - 5) + ((CDF_INIT_TOP - 5) >> 1)) / ((CDF_INIT_TOP - 5)) + \
                 2),                                                                                                   \
        AOM_ICDF((((a2) - 3) * ((CDF_INIT_TOP >> CDF_SHIFT) - 5) + ((CDF_INIT_TOP - 5) >> 1)) / ((CDF_INIT_TOP - 5)) + \
                 3),                                                                                                   \
        AOM_ICDF((((a3) - 4) * ((CDF_INIT_TOP >> CDF_SHIFT) - 5) + ((CDF_INIT_TOP - 5) >> 1)) / ((CDF_INIT_TOP - 5)) + \
                 4),                                                                                                   \
        AOM_ICDF(CDF_PROB_TOP), 0
#define AOM_CDF6(a0, a1, a2, a3, a4)                                                                                   \
    AOM_ICDF((((a0) - 1) * ((CDF_INIT_TOP >> CDF_SHIFT) - 6) + ((CDF_INIT_TOP - 6) >> 1)) / ((CDF_INIT_TOP - 6)) + 1)  \
    ,                                                                                                                  \
        AOM_ICDF((((a1) - 2) * ((CDF_INIT_TOP >> CDF_SHIFT) - 6) + ((CDF_INIT_TOP - 6) >> 1)) / ((CDF_INIT_TOP - 6)) + \
                 2),                                                                                                   \
        AOM_ICDF((((a2) - 3) * ((CDF_INIT_TOP >> CDF_SHIFT) - 6) + ((CDF_INIT_TOP - 6) >> 1)) / ((CDF_INIT_TOP - 6)) + \
                 3),                                                                                                   \
        AOM_ICDF((((a3) - 4) * ((CDF_INIT_TOP >> CDF_SHIFT) - 6) + ((CDF_INIT_TOP - 6) >> 1)) / ((CDF_INIT_TOP - 6)) + \
                 4),                                                                                                   \
        AOM_ICDF((((a4) - 5) * ((CDF_INIT_TOP >> CDF_SHIFT) - 6) + ((CDF_INIT_TOP - 6) >> 1)) / ((CDF_INIT_TOP - 6)) + \
                 5),                                                                                                   \
        AOM_ICDF(CDF_PROB_TOP), 0
#define AOM_CDF7(a0, a1, a2, a3, a4, a5)                                                                               \
    AOM_ICDF((((a0) - 1) * ((CDF_INIT_TOP >> CDF_SHIFT) - 7) + ((CDF_INIT_TOP - 7) >> 1)) / ((CDF_INIT_TOP - 7)) + 1)  \
    ,                                                                                                                  \
        AOM_ICDF((((a1) - 2) * ((CDF_INIT_TOP >> CDF_SHIFT) - 7) + ((CDF_INIT_TOP - 7) >> 1)) / ((CDF_INIT_TOP - 7)) + \
                 2),                                                                                                   \
        AOM_ICDF((((a2) - 3) * ((CDF_INIT_TOP >> CDF_SHIFT) - 7) + ((CDF_INIT_TOP - 7) >> 1)) / ((CDF_INIT_TOP - 7)) + \
                 3),                                                                                                   \
        AOM_ICDF((((a3) - 4) * ((CDF_INIT_TOP >> CDF_SHIFT) - 7) + ((CDF_INIT_TOP - 7) >> 1)) / ((CDF_INIT_TOP - 7)) + \
                 4),                                                                                                   \
        AOM_ICDF((((a4) - 5) * ((CDF_INIT_TOP >> CDF_SHIFT) - 7) + ((CDF_INIT_TOP - 7) >> 1)) / ((CDF_INIT_TOP - 7)) + \
                 5),                                                                                                   \
        AOM_ICDF((((a5) - 6) * ((CDF_INIT_TOP >> CDF_SHIFT) - 7) + ((CDF_INIT_TOP - 7) >> 1)) / ((CDF_INIT_TOP - 7)) + \
                 6),                                                                                                   \
        AOM_ICDF(CDF_PROB_TOP), 0
#define AOM_CDF8(a0, a1, a2, a3, a4, a5, a6)                                                                           \
    AOM_ICDF((((a0) - 1) * ((CDF_INIT_TOP >> CDF_SHIFT) - 8) + ((CDF_INIT_TOP - 8) >> 1)) / ((CDF_INIT_TOP - 8)) + 1)  \
    ,                                                                                                                  \
        AOM_ICDF((((a1) - 2) * ((CDF_INIT_TOP >> CDF_SHIFT) - 8) + ((CDF_INIT_TOP - 8) >> 1)) / ((CDF_INIT_TOP - 8)) + \
                 2),                                                                                                   \
        AOM_ICDF((((a2) - 3) * ((CDF_INIT_TOP >> CDF_SHIFT) - 8) + ((CDF_INIT_TOP - 8) >> 1)) / ((CDF_INIT_TOP - 8)) + \
                 3),                                                                                                   \
        AOM_ICDF((((a3) - 4) * ((CDF_INIT_TOP >> CDF_SHIFT) - 8) + ((CDF_INIT_TOP - 8) >> 1)) / ((CDF_INIT_TOP - 8)) + \
                 4),                                                                                                   \
        AOM_ICDF((((a4) - 5) * ((CDF_INIT_TOP >> CDF_SHIFT) - 8) + ((CDF_INIT_TOP - 8) >> 1)) / ((CDF_INIT_TOP - 8)) + \
                 5),                                                                                                   \
        AOM_ICDF((((a5) - 6) * ((CDF_INIT_TOP >> CDF_SHIFT) - 8) + ((CDF_INIT_TOP - 8) >> 1)) / ((CDF_INIT_TOP - 8)) + \
                 6),                                                                                                   \
        AOM_ICDF((((a6) - 7) * ((CDF_INIT_TOP >> CDF_SHIFT) - 8) + ((CDF_INIT_TOP - 8) >> 1)) / ((CDF_INIT_TOP - 8)) + \
                 7),                                                                                                   \
        AOM_ICDF(CDF_PROB_TOP), 0
#define AOM_CDF9(a0, a1, a2, a3, a4, a5, a6, a7)                                                                       \
    AOM_ICDF((((a0) - 1) * ((CDF_INIT_TOP >> CDF_SHIFT) - 9) + ((CDF_INIT_TOP - 9) >> 1)) / ((CDF_INIT_TOP - 9)) + 1)  \
    ,                                                                                                                  \
        AOM_ICDF((((a1) - 2) * ((CDF_INIT_TOP >> CDF_SHIFT) - 9) + ((CDF_INIT_TOP - 9) >> 1)) / ((CDF_INIT_TOP - 9)) + \
                 2),                                                                                                   \
        AOM_ICDF((((a2) - 3) * ((CDF_INIT_TOP >> CDF_SHIFT) - 9) + ((CDF_INIT_TOP - 9) >> 1)) / ((CDF_INIT_TOP - 9)) + \
                 3),                                                                                                   \
        AOM_ICDF((((a3) - 4) * ((CDF_INIT_TOP >> CDF_SHIFT) - 9) + ((CDF_INIT_TOP - 9) >> 1)) / ((CDF_INIT_TOP - 9)) + \
                 4),                                                                                                   \
        AOM_ICDF((((a4) - 5) * ((CDF_INIT_TOP >> CDF_SHIFT) - 9) + ((CDF_INIT_TOP - 9) >> 1)) / ((CDF_INIT_TOP - 9)) + \
                 5),                                                                                                   \
        AOM_ICDF((((a5) - 6) * ((CDF_INIT_TOP >> CDF_SHIFT) - 9) + ((CDF_INIT_TOP - 9) >> 1)) / ((CDF_INIT_TOP - 9)) + \
                 6),                                                                                                   \
        AOM_ICDF((((a6) - 7) * ((CDF_INIT_TOP >> CDF_SHIFT) - 9) + ((CDF_INIT_TOP - 9) >> 1)) / ((CDF_INIT_TOP - 9)) + \
                 7),                                                                                                   \
        AOM_ICDF((((a7) - 8) * ((CDF_INIT_TOP >> CDF_SHIFT) - 9) + ((CDF_INIT_TOP - 9) >> 1)) / ((CDF_INIT_TOP - 9)) + \
                 8),                                                                                                   \
        AOM_ICDF(CDF_PROB_TOP), 0
#define AOM_CDF10(a0, a1, a2, a3, a4, a5, a6, a7, a8)                                                                 \
    AOM_ICDF((((a0) - 1) * ((CDF_INIT_TOP >> CDF_SHIFT) - 10) + ((CDF_INIT_TOP - 10) >> 1)) / ((CDF_INIT_TOP - 10)) + \
             1)                                                                                                       \
    ,                                                                                                                 \
        AOM_ICDF((((a1) - 2) * ((CDF_INIT_TOP >> CDF_SHIFT) - 10) + ((CDF_INIT_TOP - 10) >> 1)) /                     \
                     ((CDF_INIT_TOP - 10)) +                                                                          \
                 2),                                                                                                  \
        AOM_ICDF((((a2) - 3) * ((CDF_INIT_TOP >> CDF_SHIFT) - 10) + ((CDF_INIT_TOP - 10) >> 1)) /                     \
                     ((CDF_INIT_TOP - 10)) +                                                                          \
                 3),                                                                                                  \
        AOM_ICDF((((a3) - 4) * ((CDF_INIT_TOP >> CDF_SHIFT) - 10) + ((CDF_INIT_TOP - 10) >> 1)) /                     \
                     ((CDF_INIT_TOP - 10)) +                                                                          \
                 4),                                                                                                  \
        AOM_ICDF((((a4) - 5) * ((CDF_INIT_TOP >> CDF_SHIFT) - 10) + ((CDF_INIT_TOP - 10) >> 1)) /                     \
                     ((CDF_INIT_TOP - 10)) +                                                                          \
                 5),                                                                                                  \
        AOM_ICDF((((a5) - 6) * ((CDF_INIT_TOP >> CDF_SHIFT) - 10) + ((CDF_INIT_TOP - 10) >> 1)) /                     \
                     ((CDF_INIT_TOP - 10)) +                                                                          \
                 6),                                                                                                  \
        AOM_ICDF((((a6) - 7) * ((CDF_INIT_TOP >> CDF_SHIFT) - 10) + ((CDF_INIT_TOP - 10) >> 1)) /                     \
                     ((CDF_INIT_TOP - 10)) +                                                                          \
                 7),                                                                                                  \
        AOM_ICDF((((a7) - 8) * ((CDF_INIT_TOP >> CDF_SHIFT) - 10) + ((CDF_INIT_TOP - 10) >> 1)) /                     \
                     ((CDF_INIT_TOP - 10)) +                                                                          \
                 8),                                                                                                  \
        AOM_ICDF((((a8) - 9) * ((CDF_INIT_TOP >> CDF_SHIFT) - 10) + ((CDF_INIT_TOP - 10) >> 1)) /                     \
                     ((CDF_INIT_TOP - 10)) +                                                                          \
                 9),                                                                                                  \
        AOM_ICDF(CDF_PROB_TOP), 0
#define AOM_CDF11(a0, a1, a2, a3, a4, a5, a6, a7, a8, a9)                                                             \
    AOM_ICDF((((a0) - 1) * ((CDF_INIT_TOP >> CDF_SHIFT) - 11) + ((CDF_INIT_TOP - 11) >> 1)) / ((CDF_INIT_TOP - 11)) + \
             1)                                                                                                       \
    ,                                                                                                                 \
        AOM_ICDF((((a1) - 2) * ((CDF_INIT_TOP >> CDF_SHIFT) - 11) + ((CDF_INIT_TOP - 11) >> 1)) /                     \
                     ((CDF_INIT_TOP - 11)) +                                                                          \
                 2),                                                                                                  \
        AOM_ICDF((((a2) - 3) * ((CDF_INIT_TOP >> CDF_SHIFT) - 11) + ((CDF_INIT_TOP - 11) >> 1)) /                     \
                     ((CDF_INIT_TOP - 11)) +                                                                          \
                 3),                                                                                                  \
        AOM_ICDF((((a3) - 4) * ((CDF_INIT_TOP >> CDF_SHIFT) - 11) + ((CDF_INIT_TOP - 11) >> 1)) /                     \
                     ((CDF_INIT_TOP - 11)) +                                                                          \
                 4),                                                                                                  \
        AOM_ICDF((((a4) - 5) * ((CDF_INIT_TOP >> CDF_SHIFT) - 11) + ((CDF_INIT_TOP - 11) >> 1)) /                     \
                     ((CDF_INIT_TOP - 11)) +                                                                          \
                 5),                                                                                                  \
        AOM_ICDF((((a5) - 6) * ((CDF_INIT_TOP >> CDF_SHIFT) - 11) + ((CDF_INIT_TOP - 11) >> 1)) /                     \
                     ((CDF_INIT_TOP - 11)) +                                                                          \
                 6),                                                                                                  \
        AOM_ICDF((((a6) - 7) * ((CDF_INIT_TOP >> CDF_SHIFT) - 11) + ((CDF_INIT_TOP - 11) >> 1)) /                     \
                     ((CDF_INIT_TOP - 11)) +                                                                          \
                 7),                                                                                                  \
        AOM_ICDF((((a7) - 8) * ((CDF_INIT_TOP >> CDF_SHIFT) - 11) + ((CDF_INIT_TOP - 11) >> 1)) /                     \
                     ((CDF_INIT_TOP - 11)) +                                                                          \
                 8),                                                                                                  \
        AOM_ICDF((((a8) - 9) * ((CDF_INIT_TOP >> CDF_SHIFT) - 11) + ((CDF_INIT_TOP - 11) >> 1)) /                     \
                     ((CDF_INIT_TOP - 11)) +                                                                          \
                 9),                                                                                                  \
        AOM_ICDF((((a9) - 10) * ((CDF_INIT_TOP >> CDF_SHIFT) - 11) + ((CDF_INIT_TOP - 11) >> 1)) /                    \
                     ((CDF_INIT_TOP - 11)) +                                                                          \
                 10),                                                                                                 \
        AOM_ICDF(CDF_PROB_TOP), 0
#define AOM_CDF12(a0, a1, a2, a3, a4, a5, a6, a7, a8, a9, a10)                                                        \
    AOM_ICDF((((a0) - 1) * ((CDF_INIT_TOP >> CDF_SHIFT) - 12) + ((CDF_INIT_TOP - 12) >> 1)) / ((CDF_INIT_TOP - 12)) + \
             1)                                                                                                       \
    ,                                                                                                                 \
        AOM_ICDF((((a1) - 2) * ((CDF_INIT_TOP >> CDF_SHIFT) - 12) + ((CDF_INIT_TOP - 12) >> 1)) /                     \
                     ((CDF_INIT_TOP - 12)) +                                                                          \
                 2),                                                                                                  \
        AOM_ICDF((((a2) - 3) * ((CDF_INIT_TOP >> CDF_SHIFT) - 12) + ((CDF_INIT_TOP - 12) >> 1)) /                     \
                     ((CDF_INIT_TOP - 12)) +                                                                          \
                 3),                                                                                                  \
        AOM_ICDF((((a3) - 4) * ((CDF_INIT_TOP >> CDF_SHIFT) - 12) + ((CDF_INIT_TOP - 12) >> 1)) /                     \
                     ((CDF_INIT_TOP - 12)) +                                                                          \
                 4),                                                                                                  \
        AOM_ICDF((((a4) - 5) * ((CDF_INIT_TOP >> CDF_SHIFT) - 12) + ((CDF_INIT_TOP - 12) >> 1)) /                     \
                     ((CDF_INIT_TOP - 12)) +                                                                          \
                 5),                                                                                                  \
        AOM_ICDF((((a5) - 6) * ((CDF_INIT_TOP >> CDF_SHIFT) - 12) + ((CDF_INIT_TOP - 12) >> 1)) /                     \
                     ((CDF_INIT_TOP - 12)) +                                                                          \
                 6),                                                                                                  \
        AOM_ICDF((((a6) - 7) * ((CDF_INIT_TOP >> CDF_SHIFT) - 12) + ((CDF_INIT_TOP - 12) >> 1)) /                     \
                     ((CDF_INIT_TOP - 12)) +                                                                          \
                 7),                                                                                                  \
        AOM_ICDF((((a7) - 8) * ((CDF_INIT_TOP >> CDF_SHIFT) - 12) + ((CDF_INIT_TOP - 12) >> 1)) /                     \
                     ((CDF_INIT_TOP - 12)) +                                                                          \
                 8),                                                                                                  \
        AOM_ICDF((((a8) - 9) * ((CDF_INIT_TOP >> CDF_SHIFT) - 12) + ((CDF_INIT_TOP - 12) >> 1)) /                     \
                     ((CDF_INIT_TOP - 12)) +                                                                          \
                 9),                                                                                                  \
        AOM_ICDF((((a9) - 10) * ((CDF_INIT_TOP >> CDF_SHIFT) - 12) + ((CDF_INIT_TOP - 12) >> 1)) /                    \
                     ((CDF_INIT_TOP - 12)) +                                                                          \
                 10),                                                                                                 \
        AOM_ICDF((((a10) - 11) * ((CDF_INIT_TOP >> CDF_SHIFT) - 12) + ((CDF_INIT_TOP - 12) >> 1)) /                   \
                     ((CDF_INIT_TOP - 12)) +                                                                          \
                 11),                                                                                                 \
        AOM_ICDF(CDF_PROB_TOP), 0
#define AOM_CDF13(a0, a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11)                                                   \
    AOM_ICDF((((a0) - 1) * ((CDF_INIT_TOP >> CDF_SHIFT) - 13) + ((CDF_INIT_TOP - 13) >> 1)) / ((CDF_INIT_TOP - 13)) + \
             1)                                                                                                       \
    ,                                                                                                                 \
        AOM_ICDF((((a1) - 2) * ((CDF_INIT_TOP >> CDF_SHIFT) - 13) + ((CDF_INIT_TOP - 13) >> 1)) /                     \
                     ((CDF_INIT_TOP - 13)) +                                                                          \
                 2),                                                                                                  \
        AOM_ICDF((((a2) - 3) * ((CDF_INIT_TOP >> CDF_SHIFT) - 13) + ((CDF_INIT_TOP - 13) >> 1)) /                     \
                     ((CDF_INIT_TOP - 13)) +                                                                          \
                 3),                                                                                                  \
        AOM_ICDF((((a3) - 4) * ((CDF_INIT_TOP >> CDF_SHIFT) - 13) + ((CDF_INIT_TOP - 13) >> 1)) /                     \
                     ((CDF_INIT_TOP - 13)) +                                                                          \
                 4),                                                                                                  \
        AOM_ICDF((((a4) - 5) * ((CDF_INIT_TOP >> CDF_SHIFT) - 13) + ((CDF_INIT_TOP - 13) >> 1)) /                     \
                     ((CDF_INIT_TOP - 13)) +                                                                          \
                 5),                                                                                                  \
        AOM_ICDF((((a5) - 6) * ((CDF_INIT_TOP >> CDF_SHIFT) - 13) + ((CDF_INIT_TOP - 13) >> 1)) /                     \
                     ((CDF_INIT_TOP - 13)) +                                                                          \
                 6),                                                                                                  \
        AOM_ICDF((((a6) - 7) * ((CDF_INIT_TOP >> CDF_SHIFT) - 13) + ((CDF_INIT_TOP - 13) >> 1)) /                     \
                     ((CDF_INIT_TOP - 13)) +                                                                          \
                 7),                                                                                                  \
        AOM_ICDF((((a7) - 8) * ((CDF_INIT_TOP >> CDF_SHIFT) - 13) + ((CDF_INIT_TOP - 13) >> 1)) /                     \
                     ((CDF_INIT_TOP - 13)) +                                                                          \
                 8),                                                                                                  \
        AOM_ICDF((((a8) - 9) * ((CDF_INIT_TOP >> CDF_SHIFT) - 13) + ((CDF_INIT_TOP - 13) >> 1)) /                     \
                     ((CDF_INIT_TOP - 13)) +                                                                          \
                 9),                                                                                                  \
        AOM_ICDF((((a9) - 10) * ((CDF_INIT_TOP >> CDF_SHIFT) - 13) + ((CDF_INIT_TOP - 13) >> 1)) /                    \
                     ((CDF_INIT_TOP - 13)) +                                                                          \
                 10),                                                                                                 \
        AOM_ICDF((((a10) - 11) * ((CDF_INIT_TOP >> CDF_SHIFT) - 13) + ((CDF_INIT_TOP - 13) >> 1)) /                   \
                     ((CDF_INIT_TOP - 13)) +                                                                          \
                 11),                                                                                                 \
        AOM_ICDF((((a11) - 12) * ((CDF_INIT_TOP >> CDF_SHIFT) - 13) + ((CDF_INIT_TOP - 13) >> 1)) /                   \
                     ((CDF_INIT_TOP - 13)) +                                                                          \
                 12),                                                                                                 \
        AOM_ICDF(CDF_PROB_TOP), 0
#define AOM_CDF14(a0, a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12)                                              \
    AOM_ICDF((((a0) - 1) * ((CDF_INIT_TOP >> CDF_SHIFT) - 14) + ((CDF_INIT_TOP - 14) >> 1)) / ((CDF_INIT_TOP - 14)) + \
             1)                                                                                                       \
    ,                                                                                                                 \
        AOM_ICDF((((a1) - 2) * ((CDF_INIT_TOP >> CDF_SHIFT) - 14) + ((CDF_INIT_TOP - 14) >> 1)) /                     \
                     ((CDF_INIT_TOP - 14)) +                                                                          \
                 2),                                                                                                  \
        AOM_ICDF((((a2) - 3) * ((CDF_INIT_TOP >> CDF_SHIFT) - 14) + ((CDF_INIT_TOP - 14) >> 1)) /                     \
                     ((CDF_INIT_TOP - 14)) +                                                                          \
                 3),                                                                                                  \
        AOM_ICDF((((a3) - 4) * ((CDF_INIT_TOP >> CDF_SHIFT) - 14) + ((CDF_INIT_TOP - 14) >> 1)) /                     \
                     ((CDF_INIT_TOP - 14)) +                                                                          \
                 4),                                                                                                  \
        AOM_ICDF((((a4) - 5) * ((CDF_INIT_TOP >> CDF_SHIFT) - 14) + ((CDF_INIT_TOP - 14) >> 1)) /                     \
                     ((CDF_INIT_TOP - 14)) +                                                                          \
                 5),                                                                                                  \
        AOM_ICDF((((a5) - 6) * ((CDF_INIT_TOP >> CDF_SHIFT) - 14) + ((CDF_INIT_TOP - 14) >> 1)) /                     \
                     ((CDF_INIT_TOP - 14)) +                                                                          \
                 6),                                                                                                  \
        AOM_ICDF((((a6) - 7) * ((CDF_INIT_TOP >> CDF_SHIFT) - 14) + ((CDF_INIT_TOP - 14) >> 1)) /                     \
                     ((CDF_INIT_TOP - 14)) +                                                                          \
                 7),                                                                                                  \
        AOM_ICDF((((a7) - 8) * ((CDF_INIT_TOP >> CDF_SHIFT) - 14) + ((CDF_INIT_TOP - 14) >> 1)) /                     \
                     ((CDF_INIT_TOP - 14)) +                                                                          \
                 8),                                                                                                  \
        AOM_ICDF((((a8) - 9) * ((CDF_INIT_TOP >> CDF_SHIFT) - 14) + ((CDF_INIT_TOP - 14) >> 1)) /                     \
                     ((CDF_INIT_TOP - 14)) +                                                                          \
                 9),                                                                                                  \
        AOM_ICDF((((a9) - 10) * ((CDF_INIT_TOP >> CDF_SHIFT) - 14) + ((CDF_INIT_TOP - 14) >> 1)) /                    \
                     ((CDF_INIT_TOP - 14)) +                                                                          \
                 10),                                                                                                 \
        AOM_ICDF((((a10) - 11) * ((CDF_INIT_TOP >> CDF_SHIFT) - 14) + ((CDF_INIT_TOP - 14) >> 1)) /                   \
                     ((CDF_INIT_TOP - 14)) +                                                                          \
                 11),                                                                                                 \
        AOM_ICDF((((a11) - 12) * ((CDF_INIT_TOP >> CDF_SHIFT) - 14) + ((CDF_INIT_TOP - 14) >> 1)) /                   \
                     ((CDF_INIT_TOP - 14)) +                                                                          \
                 12),                                                                                                 \
        AOM_ICDF((((a12) - 13) * ((CDF_INIT_TOP >> CDF_SHIFT) - 14) + ((CDF_INIT_TOP - 14) >> 1)) /                   \
                     ((CDF_INIT_TOP - 14)) +                                                                          \
                 13),                                                                                                 \
        AOM_ICDF(CDF_PROB_TOP), 0
#define AOM_CDF15(a0, a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12, a13)                                         \
    AOM_ICDF((((a0) - 1) * ((CDF_INIT_TOP >> CDF_SHIFT) - 15) + ((CDF_INIT_TOP - 15) >> 1)) / ((CDF_INIT_TOP - 15)) + \
             1)                                                                                                       \
    ,                                                                                                                 \
        AOM_ICDF((((a1) - 2) * ((CDF_INIT_TOP >> CDF_SHIFT) - 15) + ((CDF_INIT_TOP - 15) >> 1)) /                     \
                     ((CDF_INIT_TOP - 15)) +                                                                          \
                 2),                                                                                                  \
        AOM_ICDF((((a2) - 3) * ((CDF_INIT_TOP >> CDF_SHIFT) - 15) + ((CDF_INIT_TOP - 15) >> 1)) /                     \
                     ((CDF_INIT_TOP - 15)) +                                                                          \
                 3),                                                                                                  \
        AOM_ICDF((((a3) - 4) * ((CDF_INIT_TOP >> CDF_SHIFT) - 15) + ((CDF_INIT_TOP - 15) >> 1)) /                     \
                     ((CDF_INIT_TOP - 15)) +                                                                          \
                 4),                                                                                                  \
        AOM_ICDF((((a4) - 5) * ((CDF_INIT_TOP >> CDF_SHIFT) - 15) + ((CDF_INIT_TOP - 15) >> 1)) /                     \
                     ((CDF_INIT_TOP - 15)) +                                                                          \
                 5),                                                                                                  \
        AOM_ICDF((((a5) - 6) * ((CDF_INIT_TOP >> CDF_SHIFT) - 15) + ((CDF_INIT_TOP - 15) >> 1)) /                     \
                     ((CDF_INIT_TOP - 15)) +                                                                          \
                 6),                                                                                                  \
        AOM_ICDF((((a6) - 7) * ((CDF_INIT_TOP >> CDF_SHIFT) - 15) + ((CDF_INIT_TOP - 15) >> 1)) /                     \
                     ((CDF_INIT_TOP - 15)) +                                                                          \
                 7),                                                                                                  \
        AOM_ICDF((((a7) - 8) * ((CDF_INIT_TOP >> CDF_SHIFT) - 15) + ((CDF_INIT_TOP - 15) >> 1)) /                     \
                     ((CDF_INIT_TOP - 15)) +                                                                          \
                 8),                                                                                                  \
        AOM_ICDF((((a8) - 9) * ((CDF_INIT_TOP >> CDF_SHIFT) - 15) + ((CDF_INIT_TOP - 15) >> 1)) /                     \
                     ((CDF_INIT_TOP - 15)) +                                                                          \
                 9),                                                                                                  \
        AOM_ICDF((((a9) - 10) * ((CDF_INIT_TOP >> CDF_SHIFT) - 15) + ((CDF_INIT_TOP - 15) >> 1)) /                    \
                     ((CDF_INIT_TOP - 15)) +                                                                          \
                 10),                                                                                                 \
        AOM_ICDF((((a10) - 11) * ((CDF_INIT_TOP >> CDF_SHIFT) - 15) + ((CDF_INIT_TOP - 15) >> 1)) /                   \
                     ((CDF_INIT_TOP - 15)) +                                                                          \
                 11),                                                                                                 \
        AOM_ICDF((((a11) - 12) * ((CDF_INIT_TOP >> CDF_SHIFT) - 15) + ((CDF_INIT_TOP - 15) >> 1)) /                   \
                     ((CDF_INIT_TOP - 15)) +                                                                          \
                 12),                                                                                                 \
        AOM_ICDF((((a12) - 13) * ((CDF_INIT_TOP >> CDF_SHIFT) - 15) + ((CDF_INIT_TOP - 15) >> 1)) /                   \
                     ((CDF_INIT_TOP - 15)) +                                                                          \
                 13),                                                                                                 \
        AOM_ICDF((((a13) - 14) * ((CDF_INIT_TOP >> CDF_SHIFT) - 15) + ((CDF_INIT_TOP - 15) >> 1)) /                   \
                     ((CDF_INIT_TOP - 15)) +                                                                          \
                 14),                                                                                                 \
        AOM_ICDF(CDF_PROB_TOP), 0
#define AOM_CDF16(a0, a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12, a13, a14)                                    \
    AOM_ICDF((((a0) - 1) * ((CDF_INIT_TOP >> CDF_SHIFT) - 16) + ((CDF_INIT_TOP - 16) >> 1)) / ((CDF_INIT_TOP - 16)) + \
             1)                                                                                                       \
    ,                                                                                                                 \
        AOM_ICDF((((a1) - 2) * ((CDF_INIT_TOP >> CDF_SHIFT) - 16) + ((CDF_INIT_TOP - 16) >> 1)) /                     \
                     ((CDF_INIT_TOP - 16)) +                                                                          \
                 2),                                                                                                  \
        AOM_ICDF((((a2) - 3) * ((CDF_INIT_TOP >> CDF_SHIFT) - 16) + ((CDF_INIT_TOP - 16) >> 1)) /                     \
                     ((CDF_INIT_TOP - 16)) +                                                                          \
                 3),                                                                                                  \
        AOM_ICDF((((a3) - 4) * ((CDF_INIT_TOP >> CDF_SHIFT) - 16) + ((CDF_INIT_TOP - 16) >> 1)) /                     \
                     ((CDF_INIT_TOP - 16)) +                                                                          \
                 4),                                                                                                  \
        AOM_ICDF((((a4) - 5) * ((CDF_INIT_TOP >> CDF_SHIFT) - 16) + ((CDF_INIT_TOP - 16) >> 1)) /                     \
                     ((CDF_INIT_TOP - 16)) +                                                                          \
                 5),                                                                                                  \
        AOM_ICDF((((a5) - 6) * ((CDF_INIT_TOP >> CDF_SHIFT) - 16) + ((CDF_INIT_TOP - 16) >> 1)) /                     \
                     ((CDF_INIT_TOP - 16)) +                                                                          \
                 6),                                                                                                  \
        AOM_ICDF((((a6) - 7) * ((CDF_INIT_TOP >> CDF_SHIFT) - 16) + ((CDF_INIT_TOP - 16) >> 1)) /                     \
                     ((CDF_INIT_TOP - 16)) +                                                                          \
                 7),                                                                                                  \
        AOM_ICDF((((a7) - 8) * ((CDF_INIT_TOP >> CDF_SHIFT) - 16) + ((CDF_INIT_TOP - 16) >> 1)) /                     \
                     ((CDF_INIT_TOP - 16)) +                                                                          \
                 8),                                                                                                  \
        AOM_ICDF((((a8) - 9) * ((CDF_INIT_TOP >> CDF_SHIFT) - 16) + ((CDF_INIT_TOP - 16) >> 1)) /                     \
                     ((CDF_INIT_TOP - 16)) +                                                                          \
                 9),                                                                                                  \
        AOM_ICDF((((a9) - 10) * ((CDF_INIT_TOP >> CDF_SHIFT) - 16) + ((CDF_INIT_TOP - 16) >> 1)) /                    \
                     ((CDF_INIT_TOP - 16)) +                                                                          \
                 10),                                                                                                 \
        AOM_ICDF((((a10) - 11) * ((CDF_INIT_TOP >> CDF_SHIFT) - 16) + ((CDF_INIT_TOP - 16) >> 1)) /                   \
                     ((CDF_INIT_TOP - 16)) +                                                                          \
                 11),                                                                                                 \
        AOM_ICDF((((a11) - 12) * ((CDF_INIT_TOP >> CDF_SHIFT) - 16) + ((CDF_INIT_TOP - 16) >> 1)) /                   \
                     ((CDF_INIT_TOP - 16)) +                                                                          \
                 12),                                                                                                 \
        AOM_ICDF((((a12) - 13) * ((CDF_INIT_TOP >> CDF_SHIFT) - 16) + ((CDF_INIT_TOP - 16) >> 1)) /                   \
                     ((CDF_INIT_TOP - 16)) +                                                                          \
                 13),                                                                                                 \
        AOM_ICDF((((a13) - 14) * ((CDF_INIT_TOP >> CDF_SHIFT) - 16) + ((CDF_INIT_TOP - 16) >> 1)) /                   \
                     ((CDF_INIT_TOP - 16)) +                                                                          \
                 14),                                                                                                 \
        AOM_ICDF((((a14) - 15) * ((CDF_INIT_TOP >> CDF_SHIFT) - 16) + ((CDF_INIT_TOP - 16) >> 1)) /                   \
                     ((CDF_INIT_TOP - 16)) +                                                                          \
                 15),                                                                                                 \
        AOM_ICDF(CDF_PROB_TOP), 0

#endif

static INLINE uint8_t get_prob(unsigned int num, unsigned int den) {
    assert(den != 0);
    const int p = (int)(((uint64_t)num * 256 + (den >> 1)) / den);
    // (p > 255) ? 255 : (p < 1) ? 1 : p;
    const int clipped_prob = p | ((255 - p) >> 23) | (p == 0);
    return (uint8_t)clipped_prob;
}

static INLINE void update_cdf(AomCdfProb* cdf, int8_t val, int nsymbs) {
    assert(nsymbs < 17);
    const int count = cdf[nsymbs];

    // rate is computed in the spec as:
    //  3 + ( cdf[N] > 15 ) + ( cdf[N] > 31 ) + Min(FloorLog2(N), 2)
    // In this case cdf[N] is |count|.
    // Min(FloorLog2(N), 2) is 1 for nsymbs == {2, 3} and 2 for all
    // nsymbs > 3. So the equation becomes:
    //  4 + (count > 15) + (count > 31) + (nsymbs > 3).
    // Note that the largest value for count is 32 (it is not incremented beyond
    // 32). So using that information:
    //  count >> 4 is 0 for count from 0 to 15.
    //  count >> 4 is 1 for count from 16 to 31.
    //  count >> 4 is 2 for count == 31.
    // Now, the equation becomes:
    //  4 + (count >> 4) + (nsymbs > 3).
    const int rate = 4 + (count >> 4) + (nsymbs > 3);

    int i = 0;
    do {
        if (i < val) {
            cdf[i] += (CDF_PROB_TOP - cdf[i]) >> rate;
        } else {
            cdf[i] -= cdf[i] >> rate;
        }
    } while (++i < nsymbs - 1);
    cdf[nsymbs] += (count < 32);
}

/**********************************************************************************************************************/
// entropy.h
#define TOKEN_CDF_Q_CTXS 4

#define TXB_SKIP_CONTEXTS 13
#define EOB_COEF_CONTEXTS 9
#define SIG_COEF_CONTEXTS_2D 26
#define SIG_COEF_CONTEXTS_1D 16
#define SIG_COEF_CONTEXTS_EOB 4
#define SIG_COEF_CONTEXTS (SIG_COEF_CONTEXTS_2D + SIG_COEF_CONTEXTS_1D)

#define DC_SIGN_CONTEXTS 3

#define LEVEL_CONTEXTS 21

#define NUM_BASE_LEVELS 2

#define BR_CDF_SIZE (4)
#define COEFF_BASE_RANGE (4 * (BR_CDF_SIZE - 1))

#define COEFF_CONTEXT_BITS 6
#define COEFF_CONTEXT_MASK ((1 << COEFF_CONTEXT_BITS) - 1)
#define MAX_BASE_BR_RANGE (COEFF_BASE_RANGE + NUM_BASE_LEVELS + 1)

/* Coefficients are predicted via a 3-dimensional probability table indexed on
* REF_TYPES, COEF_BANDS and COEF_CONTEXTS. */
#define REF_TYPES 2 // intra=0, inter=1

struct AV1Common;
struct FrameContexts;
void svt_av1_reset_cdf_symbol_counters(struct FrameContexts* fc);
void svt_av1_default_coef_probs(struct FrameContexts* fc, int32_t base_qindex);
void svt_aom_init_mode_probs(struct FrameContexts* fc);

struct FrameContexts;

//**********************************************************************************************************************//
// txb_Common.h
extern const TxClass tx_type_to_class[TX_TYPES];

/**********************************************************************************************************************/
// entropymv.h

#define MV_UPDATE_PROB 252

/* Symbols for coding which components are zero jointly */
#define MV_JOINTS 4

typedef enum MvJointType {
    MV_JOINT_ZERO   = 0, /* zero vector */
    MV_JOINT_HNZVZ  = 1, /* Vert zero, hor nonzero */
    MV_JOINT_HZVNZ  = 2, /* Hor zero, vert nonzero */
    MV_JOINT_HNZVNZ = 3, /* Both components nonzero */
} MvJointType;

static INLINE int32_t mv_joint_vertical(MvJointType type) {
    return type == MV_JOINT_HZVNZ || type == MV_JOINT_HNZVNZ;
}

static INLINE int32_t mv_joint_horizontal(MvJointType type) {
    return type == MV_JOINT_HNZVZ || type == MV_JOINT_HNZVNZ;
}

/* Symbols for coding magnitude class of nonzero components */
#define MV_CLASSES 11

typedef enum MvClassType {
    MV_CLASS_0  = 0, /* (0, 2]     integer pel */
    MV_CLASS_1  = 1, /* (2, 4]     integer pel */
    MV_CLASS_2  = 2, /* (4, 8]     integer pel */
    MV_CLASS_3  = 3, /* (8, 16]    integer pel */
    MV_CLASS_4  = 4, /* (16, 32]   integer pel */
    MV_CLASS_5  = 5, /* (32, 64]   integer pel */
    MV_CLASS_6  = 6, /* (64, 128]  integer pel */
    MV_CLASS_7  = 7, /* (128, 256] integer pel */
    MV_CLASS_8  = 8, /* (256, 512] integer pel */
    MV_CLASS_9  = 9, /* (512, 1024] integer pel */
    MV_CLASS_10 = 10, /* (1024,2048] integer pel */
} MvClassType;

#define CLASS0_BITS 1 /* bits at integer precision for class 0 */
#define CLASS0_SIZE (1 << CLASS0_BITS)
#define MV_OFFSET_BITS (MV_CLASSES + CLASS0_BITS - 2)
#define MV_BITS_CONTEXTS 6
#define MV_FP_SIZE 4

#define MV_MAX_BITS (MV_CLASSES + CLASS0_BITS + 2)
#define MV_MAX ((1 << MV_MAX_BITS) - 1)
#define MV_VALS ((MV_MAX << 1) + 1)

#define MV_IN_USE_BITS 14
#define MV_UPP (1 << MV_IN_USE_BITS)
#define MV_LOW (-(1 << MV_IN_USE_BITS))

typedef struct NmvComponent {
    AomCdfProb classes_cdf[CDF_SIZE(MV_CLASSES)];
    AomCdfProb class0_fp_cdf[CLASS0_SIZE][CDF_SIZE(MV_FP_SIZE)];
    AomCdfProb fp_cdf[CDF_SIZE(MV_FP_SIZE)];
    AomCdfProb sign_cdf[CDF_SIZE(2)];
    AomCdfProb class0_hp_cdf[CDF_SIZE(2)];
    AomCdfProb hp_cdf[CDF_SIZE(2)];
    AomCdfProb class0_cdf[CDF_SIZE(CLASS0_SIZE)];
    AomCdfProb bits_cdf[MV_OFFSET_BITS][CDF_SIZE(2)];
} NmvComponent;

typedef struct NmvContext {
    AomCdfProb   joints_cdf[CDF_SIZE(MV_JOINTS)];
    NmvComponent comps[2];
} NmvContext;

MvClassType svt_av1_get_mv_class(int32_t z, int32_t* offset);

typedef enum MvSubpelPrecision {
    MV_SUBPEL_NONE          = -1,
    MV_SUBPEL_LOW_PRECISION = 0,
    MV_SUBPEL_HIGH_PRECISION,
} MvSubpelPrecision;

/**********************************************************************************************************************/
// entropymode.h
#define BlockSize_GROUPS 4

#define TX_SIZE_CONTEXTS 3

#define INTER_OFFSET(mode) ((mode) - NEARESTMV)
#define INTER_COMPOUND_OFFSET(mode) (uint8_t)((mode) - NEAREST_NEARESTMV)

// Number of possible contexts for a color index.
// As can be seen from av1_get_palette_color_index_context(), the possible
// contexts are (2,0,0), (2,2,1), (3,2,0), (4,1,0), (5,0,0). These are mapped to
// a value from 0 to 4 using 'svt_aom_palette_color_index_context_lookup' table.
#define PALETTE_COLOR_INDEX_CONTEXTS 5

// Palette Y mode context for a block is determined by number of neighboring
// blocks (top and/or left) using a palette for Y plane. So, possible Y mode'
// context values are:
// 0 if neither left nor top block uses palette for Y plane,
// 1 if exactly one of left or top block uses palette for Y plane, and
// 2 if both left and top blocks use palette for Y plane.
#define PALETTE_Y_MODE_CONTEXTS 3

// Palette UV mode context for a block is determined by whether this block uses
// palette for the Y plane. So, possible values are:
// 0 if this block doesn't use palette for Y plane.
// 1 if this block uses palette for Y plane (i.e. Y palette size > 0).
#define PALETTE_UV_MODE_CONTEXTS 2

// Map the number of pixels in a block size to a context
//   16(BLOCK_4X4)                          -> 0
//   32(BLOCK_4X8, BLOCK_8X4)               -> 1
//   64(BLOCK_8X8, BLOCK_4x16, BLOCK_16X4)  -> 2
//   ...
// 4096(BLOCK_64X64)                        -> 8
#define PALATTE_BSIZE_CTXS 7

#define KF_MODE_CONTEXTS 5

#define SEG_TEMPORAL_PRED_CTXS 3
#define SPATIAL_PREDICTION_PROBS 3

typedef struct {
    const int16_t* scan;
    const int16_t* iscan;
} ScanOrder;

struct segmentation_probs {
    AomCdfProb tree_cdf[CDF_SIZE(MAX_SEGMENTS)];
    AomCdfProb pred_cdf[SEG_TEMPORAL_PRED_CTXS][CDF_SIZE(2)];
    AomCdfProb spatial_pred_seg_cdf[SPATIAL_PREDICTION_PROBS][CDF_SIZE(MAX_SEGMENTS)];
};

typedef struct FrameContexts {
    AomCdfProb txb_skip_cdf[TX_SIZES][TXB_SKIP_CONTEXTS][CDF_SIZE(2)];
    AomCdfProb eob_extra_cdf[TX_SIZES][PLANE_TYPES][EOB_COEF_CONTEXTS][CDF_SIZE(2)];
    AomCdfProb dc_sign_cdf[PLANE_TYPES][DC_SIGN_CONTEXTS][CDF_SIZE(2)];
    AomCdfProb eob_flag_cdf16[PLANE_TYPES][2][CDF_SIZE(5)];
    AomCdfProb eob_flag_cdf32[PLANE_TYPES][2][CDF_SIZE(6)];
    AomCdfProb eob_flag_cdf64[PLANE_TYPES][2][CDF_SIZE(7)];
    AomCdfProb eob_flag_cdf128[PLANE_TYPES][2][CDF_SIZE(8)];
    AomCdfProb eob_flag_cdf256[PLANE_TYPES][2][CDF_SIZE(9)];
    AomCdfProb eob_flag_cdf512[PLANE_TYPES][2][CDF_SIZE(10)];
    AomCdfProb eob_flag_cdf1024[PLANE_TYPES][2][CDF_SIZE(11)];
    AomCdfProb coeff_base_eob_cdf[TX_SIZES][PLANE_TYPES][SIG_COEF_CONTEXTS_EOB][CDF_SIZE(3)];
    AomCdfProb coeff_base_cdf[TX_SIZES][PLANE_TYPES][SIG_COEF_CONTEXTS][CDF_SIZE(4)];
    AomCdfProb coeff_br_cdf[TX_SIZES][PLANE_TYPES][LEVEL_CONTEXTS][CDF_SIZE(BR_CDF_SIZE)];

    AomCdfProb newmv_cdf[NEWMV_MODE_CONTEXTS][CDF_SIZE(2)];
    AomCdfProb zeromv_cdf[GLOBALMV_MODE_CONTEXTS][CDF_SIZE(2)];
    AomCdfProb refmv_cdf[REFMV_MODE_CONTEXTS][CDF_SIZE(2)];
    AomCdfProb drl_cdf[DRL_MODE_CONTEXTS][CDF_SIZE(2)];

    AomCdfProb inter_compound_mode_cdf[INTER_MODE_CONTEXTS][CDF_SIZE(INTER_COMPOUND_MODES)];
    AomCdfProb compound_type_cdf[BLOCK_SIZES_ALL][CDF_SIZE(MASKED_COMPOUND_TYPES)];
    AomCdfProb wedge_idx_cdf[BLOCK_SIZES_ALL][CDF_SIZE(16)];
    AomCdfProb interintra_cdf[BlockSize_GROUPS][CDF_SIZE(2)];
    AomCdfProb wedge_interintra_cdf[BLOCK_SIZES_ALL][CDF_SIZE(2)];
    AomCdfProb interintra_mode_cdf[BlockSize_GROUPS][CDF_SIZE(INTERINTRA_MODES)];
    AomCdfProb motion_mode_cdf[BLOCK_SIZES_ALL][CDF_SIZE(MOTION_MODES)];
    AomCdfProb obmc_cdf[BLOCK_SIZES_ALL][CDF_SIZE(2)];
    AomCdfProb palette_y_size_cdf[PALATTE_BSIZE_CTXS][CDF_SIZE(PALETTE_SIZES)];
    AomCdfProb palette_uv_size_cdf[PALATTE_BSIZE_CTXS][CDF_SIZE(PALETTE_SIZES)];
    AomCdfProb palette_y_color_index_cdf[PALETTE_SIZES][PALETTE_COLOR_INDEX_CONTEXTS][CDF_SIZE(PALETTE_COLORS)];
    AomCdfProb palette_uv_color_index_cdf[PALETTE_SIZES][PALETTE_COLOR_INDEX_CONTEXTS][CDF_SIZE(PALETTE_COLORS)];
    AomCdfProb palette_y_mode_cdf[PALATTE_BSIZE_CTXS][PALETTE_Y_MODE_CONTEXTS][CDF_SIZE(2)];
    AomCdfProb palette_uv_mode_cdf[PALETTE_UV_MODE_CONTEXTS][CDF_SIZE(2)];
    AomCdfProb comp_inter_cdf[COMP_INTER_CONTEXTS][CDF_SIZE(2)];
    AomCdfProb single_ref_cdf[REF_CONTEXTS][SINGLE_REFS - 1][CDF_SIZE(2)];
    AomCdfProb comp_ref_type_cdf[COMP_REF_TYPE_CONTEXTS][CDF_SIZE(2)];
    AomCdfProb uni_comp_ref_cdf[UNI_COMP_REF_CONTEXTS][UNIDIR_COMP_REFS - 1][CDF_SIZE(2)];

    AomCdfProb                comp_ref_cdf[REF_CONTEXTS][FWD_REFS - 1][CDF_SIZE(2)];
    AomCdfProb                comp_bwdref_cdf[REF_CONTEXTS][BWD_REFS - 1][CDF_SIZE(2)];
    AomCdfProb                txfm_partition_cdf[TXFM_PARTITION_CONTEXTS][CDF_SIZE(2)];
    AomCdfProb                compound_index_cdf[COMP_INDEX_CONTEXTS][CDF_SIZE(2)];
    AomCdfProb                comp_group_idx_cdf[COMP_GROUP_IDX_CONTEXTS][CDF_SIZE(2)];
    AomCdfProb                skip_mode_cdfs[SKIP_CONTEXTS][CDF_SIZE(2)];
    AomCdfProb                skip_cdfs[SKIP_CONTEXTS][CDF_SIZE(2)];
    AomCdfProb                intra_inter_cdf[INTRA_INTER_CONTEXTS][CDF_SIZE(2)];
    NmvContext                nmvc;
    NmvContext                ndvc;
    AomCdfProb                intrabc_cdf[CDF_SIZE(2)];
    struct segmentation_probs seg;
    AomCdfProb                filter_intra_cdfs[BLOCK_SIZES_ALL][CDF_SIZE(2)];
    AomCdfProb                filter_intra_mode_cdf[CDF_SIZE(FILTER_INTRA_MODES)];
    AomCdfProb                switchable_restore_cdf[CDF_SIZE(RESTORE_SWITCHABLE_TYPES)];
    AomCdfProb                wiener_restore_cdf[CDF_SIZE(2)];
    AomCdfProb                sgrproj_restore_cdf[CDF_SIZE(2)];
    AomCdfProb                y_mode_cdf[BlockSize_GROUPS][CDF_SIZE(INTRA_MODES)];
    AomCdfProb                uv_mode_cdf[CFL_ALLOWED_TYPES][INTRA_MODES][CDF_SIZE(UV_INTRA_MODES)];
    AomCdfProb                partition_cdf[PARTITION_CONTEXTS][CDF_SIZE(EXT_PARTITION_TYPES)];

    AomCdfProb switchable_interp_cdf[SWITCHABLE_FILTER_CONTEXTS][CDF_SIZE(SWITCHABLE_FILTERS)];
    /* kf_y_cdf is discarded after use, so does not require persistent storage.
       However, we keep it with the other CDFs in this struct since it needs to
       be copied to each tile to support parallelism just like the others.
       */
    AomCdfProb kf_y_cdf[KF_MODE_CONTEXTS][KF_MODE_CONTEXTS][CDF_SIZE(INTRA_MODES)];

    AomCdfProb angle_delta_cdf[DIRECTIONAL_MODES][CDF_SIZE(2 * MAX_ANGLE_DELTA + 1)];

    AomCdfProb tx_size_cdf[MAX_TX_CATS][TX_SIZE_CONTEXTS][CDF_SIZE(MAX_TX_DEPTH + 1)];
    AomCdfProb delta_q_cdf[CDF_SIZE(DELTA_Q_PROBS + 1)];
    AomCdfProb delta_lf_multi_cdf[FRAME_LF_COUNT][CDF_SIZE(DELTA_LF_PROBS + 1)];
    AomCdfProb delta_lf_cdf[CDF_SIZE(DELTA_LF_PROBS + 1)];
    AomCdfProb intra_ext_tx_cdf[EXT_TX_SETS_INTRA][EXT_TX_SIZES][INTRA_MODES][CDF_SIZE(TX_TYPES)];
    AomCdfProb inter_ext_tx_cdf[EXT_TX_SETS_INTER][EXT_TX_SIZES][CDF_SIZE(TX_TYPES)];
    AomCdfProb cfl_sign_cdf[CDF_SIZE(CFL_JOINT_SIGNS)];
    AomCdfProb cfl_alpha_cdf[CFL_ALPHA_CONTEXTS][CDF_SIZE(CFL_ALPHABET_SIZE)];
    int32_t    initialized;
} FRAME_CONTEXT;

extern const int32_t av1_ext_tx_ind[EXT_TX_SET_TYPES][TX_TYPES];
extern const int32_t av1_ext_tx_inv[EXT_TX_SET_TYPES][TX_TYPES];

void av1_set_default_ref_deltas(int8_t* ref_deltas);
void av1_set_default_mode_deltas(int8_t* mode_deltas);
void av1_setup_frame_contexts(struct AV1Common* cm);
void av1_setup_past_independence(struct AV1Common* cm);

static INLINE int32_t av1_ceil_log2(int32_t n) {
    if (n < 2) {
        return 0;
    }
    return svt_log2f(n - 1) + 1;
}

static AomCdfProb cdf_element_prob(const AomCdfProb* const cdf, size_t element) {
    assert(cdf != NULL);
    return (element > 0 ? cdf[element - 1] : CDF_PROB_TOP) - cdf[element];
}

static INLINE void partition_gather_horz_alike(AomCdfProb* out, const AomCdfProb* const in, BlockSize bsize) {
    out[0] = CDF_PROB_TOP;
    out[0] -= cdf_element_prob(in, PARTITION_HORZ);
    out[0] -= cdf_element_prob(in, PARTITION_SPLIT);
    out[0] -= cdf_element_prob(in, PARTITION_HORZ_A);
    out[0] -= cdf_element_prob(in, PARTITION_HORZ_B);
    out[0] -= cdf_element_prob(in, PARTITION_VERT_A);
    if (bsize != BLOCK_128X128) {
        out[0] -= cdf_element_prob(in, PARTITION_HORZ_4);
    }
    out[0] = AOM_ICDF(out[0]);
    out[1] = AOM_ICDF(CDF_PROB_TOP);
    out[2] = 0;
}

static INLINE void partition_gather_vert_alike(AomCdfProb* out, const AomCdfProb* const in, BlockSize bsize) {
    out[0] = CDF_PROB_TOP;
    out[0] -= cdf_element_prob(in, PARTITION_VERT);
    out[0] -= cdf_element_prob(in, PARTITION_SPLIT);
    out[0] -= cdf_element_prob(in, PARTITION_HORZ_A);
    out[0] -= cdf_element_prob(in, PARTITION_VERT_A);
    out[0] -= cdf_element_prob(in, PARTITION_VERT_B);
    if (bsize != BLOCK_128X128) {
        out[0] -= cdf_element_prob(in, PARTITION_VERT_4);
    }
    out[0] = AOM_ICDF(out[0]);
    out[1] = AOM_ICDF(CDF_PROB_TOP);
    out[2] = 0;
}

/**********************************************************************************************************************/
// onyxc_int.h

/**********************************************************************************************************************/
int svt_aom_get_palette_color_index_context_optimized(const uint8_t* color_map, int stride, int r, int c,
                                                      int* color_idx);
/**********************************************************************************************************************/
/**********************************************************************************************************************/

#ifdef __cplusplus
}
#endif
#endif //EbCabacContextModel_h
