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

#include "temporal_filtering.h"

// Division using multiplication and shifting. The C implementation does:
// modifier *= 3;
// modifier /= index;
// where 'modifier' is a set of summed values and 'index' is the number of
// summed values.
//
// This equation works out to (m * 3) / i which reduces to:
// m * 3/4
// m * 1/2
// m * 1/3
//
// by pairing the multiply with a down shift by 16 (_mm_mulhi_epu16):
// m * C / 65536
// we can create a C to replicate the division.
//
// m * 49152 / 65536 = m * 3/4
// m * 32758 / 65536 = m * 1/2
// m * 21846 / 65536 = m * 0.3333
//
// These are loaded using an instruction expecting int16_t values but are used
// with _mm_mulhi_epu16(), which treats them as unsigned.
#define NEIGHBOR_CONSTANT_4 (int16_t)49152
#define NEIGHBOR_CONSTANT_5 (int16_t)39322
#define NEIGHBOR_CONSTANT_6 (int16_t)32768
#define NEIGHBOR_CONSTANT_7 (int16_t)28087
#define NEIGHBOR_CONSTANT_8 (int16_t)24576
#define NEIGHBOR_CONSTANT_9 (int16_t)21846
#define NEIGHBOR_CONSTANT_10 (int16_t)19661
#define NEIGHBOR_CONSTANT_11 (int16_t)17874
#define NEIGHBOR_CONSTANT_13 (int16_t)15124

DECLARE_ALIGNED(16, static const int16_t, left_corner_neighbors_plus1[8]) = {NEIGHBOR_CONSTANT_5,
                                                                             NEIGHBOR_CONSTANT_7,
                                                                             NEIGHBOR_CONSTANT_7,
                                                                             NEIGHBOR_CONSTANT_7,
                                                                             NEIGHBOR_CONSTANT_7,
                                                                             NEIGHBOR_CONSTANT_7,
                                                                             NEIGHBOR_CONSTANT_7,
                                                                             NEIGHBOR_CONSTANT_7};

DECLARE_ALIGNED(16, static const int16_t, right_corner_neighbors_plus1[8]) = {NEIGHBOR_CONSTANT_7,
                                                                              NEIGHBOR_CONSTANT_7,
                                                                              NEIGHBOR_CONSTANT_7,
                                                                              NEIGHBOR_CONSTANT_7,
                                                                              NEIGHBOR_CONSTANT_7,
                                                                              NEIGHBOR_CONSTANT_7,
                                                                              NEIGHBOR_CONSTANT_7,
                                                                              NEIGHBOR_CONSTANT_5};

DECLARE_ALIGNED(16, static const int16_t, left_edge_neighbors_plus1[8]) = {NEIGHBOR_CONSTANT_7,
                                                                           NEIGHBOR_CONSTANT_10,
                                                                           NEIGHBOR_CONSTANT_10,
                                                                           NEIGHBOR_CONSTANT_10,
                                                                           NEIGHBOR_CONSTANT_10,
                                                                           NEIGHBOR_CONSTANT_10,
                                                                           NEIGHBOR_CONSTANT_10,
                                                                           NEIGHBOR_CONSTANT_10};

DECLARE_ALIGNED(16, static const int16_t, right_edge_neighbors_plus1[8]) = {NEIGHBOR_CONSTANT_10,
                                                                            NEIGHBOR_CONSTANT_10,
                                                                            NEIGHBOR_CONSTANT_10,
                                                                            NEIGHBOR_CONSTANT_10,
                                                                            NEIGHBOR_CONSTANT_10,
                                                                            NEIGHBOR_CONSTANT_10,
                                                                            NEIGHBOR_CONSTANT_10,
                                                                            NEIGHBOR_CONSTANT_7};

DECLARE_ALIGNED(16, static const int16_t, middle_edge_neighbors_plus1[8]) = {NEIGHBOR_CONSTANT_7,
                                                                             NEIGHBOR_CONSTANT_7,
                                                                             NEIGHBOR_CONSTANT_7,
                                                                             NEIGHBOR_CONSTANT_7,
                                                                             NEIGHBOR_CONSTANT_7,
                                                                             NEIGHBOR_CONSTANT_7,
                                                                             NEIGHBOR_CONSTANT_7,
                                                                             NEIGHBOR_CONSTANT_7};

DECLARE_ALIGNED(16, static const int16_t, middle_center_neighbors_plus1[8]) = {NEIGHBOR_CONSTANT_10,
                                                                               NEIGHBOR_CONSTANT_10,
                                                                               NEIGHBOR_CONSTANT_10,
                                                                               NEIGHBOR_CONSTANT_10,
                                                                               NEIGHBOR_CONSTANT_10,
                                                                               NEIGHBOR_CONSTANT_10,
                                                                               NEIGHBOR_CONSTANT_10,
                                                                               NEIGHBOR_CONSTANT_10};

DECLARE_ALIGNED(16, static const int16_t, left_corner_neighbors_plus2[8]) = {NEIGHBOR_CONSTANT_6,
                                                                             NEIGHBOR_CONSTANT_8,
                                                                             NEIGHBOR_CONSTANT_8,
                                                                             NEIGHBOR_CONSTANT_8,
                                                                             NEIGHBOR_CONSTANT_8,
                                                                             NEIGHBOR_CONSTANT_8,
                                                                             NEIGHBOR_CONSTANT_8,
                                                                             NEIGHBOR_CONSTANT_8};

DECLARE_ALIGNED(16, static const int16_t, right_corner_neighbors_plus2[8]) = {NEIGHBOR_CONSTANT_8,
                                                                              NEIGHBOR_CONSTANT_8,
                                                                              NEIGHBOR_CONSTANT_8,
                                                                              NEIGHBOR_CONSTANT_8,
                                                                              NEIGHBOR_CONSTANT_8,
                                                                              NEIGHBOR_CONSTANT_8,
                                                                              NEIGHBOR_CONSTANT_8,
                                                                              NEIGHBOR_CONSTANT_6};

DECLARE_ALIGNED(16, static const int16_t, left_edge_neighbors_plus2[8]) = {NEIGHBOR_CONSTANT_8,
                                                                           NEIGHBOR_CONSTANT_11,
                                                                           NEIGHBOR_CONSTANT_11,
                                                                           NEIGHBOR_CONSTANT_11,
                                                                           NEIGHBOR_CONSTANT_11,
                                                                           NEIGHBOR_CONSTANT_11,
                                                                           NEIGHBOR_CONSTANT_11,
                                                                           NEIGHBOR_CONSTANT_11};

DECLARE_ALIGNED(16, static const int16_t, right_edge_neighbors_plus2[8]) = {NEIGHBOR_CONSTANT_11,
                                                                            NEIGHBOR_CONSTANT_11,
                                                                            NEIGHBOR_CONSTANT_11,
                                                                            NEIGHBOR_CONSTANT_11,
                                                                            NEIGHBOR_CONSTANT_11,
                                                                            NEIGHBOR_CONSTANT_11,
                                                                            NEIGHBOR_CONSTANT_11,
                                                                            NEIGHBOR_CONSTANT_8};

DECLARE_ALIGNED(16, static const int16_t, middle_edge_neighbors_plus2[8]) = {NEIGHBOR_CONSTANT_8,
                                                                             NEIGHBOR_CONSTANT_8,
                                                                             NEIGHBOR_CONSTANT_8,
                                                                             NEIGHBOR_CONSTANT_8,
                                                                             NEIGHBOR_CONSTANT_8,
                                                                             NEIGHBOR_CONSTANT_8,
                                                                             NEIGHBOR_CONSTANT_8,
                                                                             NEIGHBOR_CONSTANT_8};

DECLARE_ALIGNED(16, static const int16_t, middle_center_neighbors_plus2[8]) = {NEIGHBOR_CONSTANT_11,
                                                                               NEIGHBOR_CONSTANT_11,
                                                                               NEIGHBOR_CONSTANT_11,
                                                                               NEIGHBOR_CONSTANT_11,
                                                                               NEIGHBOR_CONSTANT_11,
                                                                               NEIGHBOR_CONSTANT_11,
                                                                               NEIGHBOR_CONSTANT_11,
                                                                               NEIGHBOR_CONSTANT_11};

DECLARE_ALIGNED(16, static const int16_t, two_corner_neighbors_plus2[8]) = {NEIGHBOR_CONSTANT_6,
                                                                            NEIGHBOR_CONSTANT_8,
                                                                            NEIGHBOR_CONSTANT_8,
                                                                            NEIGHBOR_CONSTANT_8,
                                                                            NEIGHBOR_CONSTANT_8,
                                                                            NEIGHBOR_CONSTANT_8,
                                                                            NEIGHBOR_CONSTANT_8,
                                                                            NEIGHBOR_CONSTANT_6};

DECLARE_ALIGNED(16, static const int16_t, two_edge_neighbors_plus2[8]) = {NEIGHBOR_CONSTANT_8,
                                                                          NEIGHBOR_CONSTANT_11,
                                                                          NEIGHBOR_CONSTANT_11,
                                                                          NEIGHBOR_CONSTANT_11,
                                                                          NEIGHBOR_CONSTANT_11,
                                                                          NEIGHBOR_CONSTANT_11,
                                                                          NEIGHBOR_CONSTANT_11,
                                                                          NEIGHBOR_CONSTANT_8};

DECLARE_ALIGNED(16, static const int16_t, left_corner_neighbors_plus4[8]) = {NEIGHBOR_CONSTANT_8,
                                                                             NEIGHBOR_CONSTANT_10,
                                                                             NEIGHBOR_CONSTANT_10,
                                                                             NEIGHBOR_CONSTANT_10,
                                                                             NEIGHBOR_CONSTANT_10,
                                                                             NEIGHBOR_CONSTANT_10,
                                                                             NEIGHBOR_CONSTANT_10,
                                                                             NEIGHBOR_CONSTANT_10};

DECLARE_ALIGNED(16, static const int16_t, right_corner_neighbors_plus4[8]) = {NEIGHBOR_CONSTANT_10,
                                                                              NEIGHBOR_CONSTANT_10,
                                                                              NEIGHBOR_CONSTANT_10,
                                                                              NEIGHBOR_CONSTANT_10,
                                                                              NEIGHBOR_CONSTANT_10,
                                                                              NEIGHBOR_CONSTANT_10,
                                                                              NEIGHBOR_CONSTANT_10,
                                                                              NEIGHBOR_CONSTANT_8};

DECLARE_ALIGNED(16, static const int16_t, left_edge_neighbors_plus4[8]) = {NEIGHBOR_CONSTANT_10,
                                                                           NEIGHBOR_CONSTANT_13,
                                                                           NEIGHBOR_CONSTANT_13,
                                                                           NEIGHBOR_CONSTANT_13,
                                                                           NEIGHBOR_CONSTANT_13,
                                                                           NEIGHBOR_CONSTANT_13,
                                                                           NEIGHBOR_CONSTANT_13,
                                                                           NEIGHBOR_CONSTANT_13};

DECLARE_ALIGNED(16, static const int16_t, right_edge_neighbors_plus4[8]) = {NEIGHBOR_CONSTANT_13,
                                                                            NEIGHBOR_CONSTANT_13,
                                                                            NEIGHBOR_CONSTANT_13,
                                                                            NEIGHBOR_CONSTANT_13,
                                                                            NEIGHBOR_CONSTANT_13,
                                                                            NEIGHBOR_CONSTANT_13,
                                                                            NEIGHBOR_CONSTANT_13,
                                                                            NEIGHBOR_CONSTANT_10};

DECLARE_ALIGNED(16, static const int16_t, middle_edge_neighbors_plus4[8]) = {NEIGHBOR_CONSTANT_10,
                                                                             NEIGHBOR_CONSTANT_10,
                                                                             NEIGHBOR_CONSTANT_10,
                                                                             NEIGHBOR_CONSTANT_10,
                                                                             NEIGHBOR_CONSTANT_10,
                                                                             NEIGHBOR_CONSTANT_10,
                                                                             NEIGHBOR_CONSTANT_10,
                                                                             NEIGHBOR_CONSTANT_10};

DECLARE_ALIGNED(16, static const int16_t, middle_center_neighbors_plus4[8]) = {NEIGHBOR_CONSTANT_13,
                                                                               NEIGHBOR_CONSTANT_13,
                                                                               NEIGHBOR_CONSTANT_13,
                                                                               NEIGHBOR_CONSTANT_13,
                                                                               NEIGHBOR_CONSTANT_13,
                                                                               NEIGHBOR_CONSTANT_13,
                                                                               NEIGHBOR_CONSTANT_13,
                                                                               NEIGHBOR_CONSTANT_13};

DECLARE_ALIGNED(16, static const int16_t, two_corner_neighbors_plus4[8]) = {NEIGHBOR_CONSTANT_8,
                                                                            NEIGHBOR_CONSTANT_10,
                                                                            NEIGHBOR_CONSTANT_10,
                                                                            NEIGHBOR_CONSTANT_10,
                                                                            NEIGHBOR_CONSTANT_10,
                                                                            NEIGHBOR_CONSTANT_10,
                                                                            NEIGHBOR_CONSTANT_10,
                                                                            NEIGHBOR_CONSTANT_8};

DECLARE_ALIGNED(16, static const int16_t, two_edge_neighbors_plus4[8]) = {NEIGHBOR_CONSTANT_10,
                                                                          NEIGHBOR_CONSTANT_13,
                                                                          NEIGHBOR_CONSTANT_13,
                                                                          NEIGHBOR_CONSTANT_13,
                                                                          NEIGHBOR_CONSTANT_13,
                                                                          NEIGHBOR_CONSTANT_13,
                                                                          NEIGHBOR_CONSTANT_13,
                                                                          NEIGHBOR_CONSTANT_10};

static const int16_t* const luma_left_column_neighbors[2] = {left_corner_neighbors_plus2, left_edge_neighbors_plus2};

static const int16_t* const luma_middle_column_neighbors[2] = {middle_edge_neighbors_plus2,
                                                               middle_center_neighbors_plus2};

static const int16_t* const luma_right_column_neighbors[2] = {right_corner_neighbors_plus2, right_edge_neighbors_plus2};

static const int16_t* const chroma_no_ss_left_column_neighbors[2] = {left_corner_neighbors_plus1,
                                                                     left_edge_neighbors_plus1};

static const int16_t* const chroma_no_ss_middle_column_neighbors[2] = {middle_edge_neighbors_plus1,
                                                                       middle_center_neighbors_plus1};

static const int16_t* const chroma_no_ss_right_column_neighbors[2] = {right_corner_neighbors_plus1,
                                                                      right_edge_neighbors_plus1};

static const int16_t* const chroma_single_ss_left_column_neighbors[2] = {left_corner_neighbors_plus2,
                                                                         left_edge_neighbors_plus2};

static const int16_t* const chroma_single_ss_middle_column_neighbors[2] = {middle_edge_neighbors_plus2,
                                                                           middle_center_neighbors_plus2};

static const int16_t* const chroma_single_ss_right_column_neighbors[2] = {right_corner_neighbors_plus2,
                                                                          right_edge_neighbors_plus2};

static const int16_t* const chroma_single_ss_single_column_neighbors[2] = {two_corner_neighbors_plus2,
                                                                           two_edge_neighbors_plus2};

static const int16_t* const chroma_double_ss_left_column_neighbors[2] = {left_corner_neighbors_plus4,
                                                                         left_edge_neighbors_plus4};

static const int16_t* const chroma_double_ss_middle_column_neighbors[2] = {middle_edge_neighbors_plus4,
                                                                           middle_center_neighbors_plus4};

static const int16_t* const chroma_double_ss_right_column_neighbors[2] = {right_corner_neighbors_plus4,
                                                                          right_edge_neighbors_plus4};

static const int16_t* const chroma_double_ss_single_column_neighbors[2] = {two_corner_neighbors_plus4,
                                                                           two_edge_neighbors_plus4};

#define HIGHBD_NEIGHBOR_CONSTANT_4 (uint32_t)3221225472U
#define HIGHBD_NEIGHBOR_CONSTANT_5 (uint32_t)2576980378U
#define HIGHBD_NEIGHBOR_CONSTANT_6 (uint32_t)2147483648U
#define HIGHBD_NEIGHBOR_CONSTANT_7 (uint32_t)1840700270U
#define HIGHBD_NEIGHBOR_CONSTANT_8 (uint32_t)1610612736U
#define HIGHBD_NEIGHBOR_CONSTANT_9 (uint32_t)1431655766U
#define HIGHBD_NEIGHBOR_CONSTANT_10 (uint32_t)1288490189U
#define HIGHBD_NEIGHBOR_CONSTANT_11 (uint32_t)1171354718U
#define HIGHBD_NEIGHBOR_CONSTANT_13 (uint32_t)991146300U

DECLARE_ALIGNED(16, static const uint32_t, highbd_left_corner_neighbours_plus_1[4]) = {
    HIGHBD_NEIGHBOR_CONSTANT_5, HIGHBD_NEIGHBOR_CONSTANT_7, HIGHBD_NEIGHBOR_CONSTANT_7, HIGHBD_NEIGHBOR_CONSTANT_7};

DECLARE_ALIGNED(16, static const uint32_t, highbd_right_corner_neighbors_plus_1[4]) = {
    HIGHBD_NEIGHBOR_CONSTANT_7, HIGHBD_NEIGHBOR_CONSTANT_7, HIGHBD_NEIGHBOR_CONSTANT_7, HIGHBD_NEIGHBOR_CONSTANT_5};

DECLARE_ALIGNED(16, static const uint32_t, highbd_left_edge_neighbours_plus_1[4]) = {
    HIGHBD_NEIGHBOR_CONSTANT_7, HIGHBD_NEIGHBOR_CONSTANT_10, HIGHBD_NEIGHBOR_CONSTANT_10, HIGHBD_NEIGHBOR_CONSTANT_10};

DECLARE_ALIGNED(16, static const uint32_t, highbd_right_edge_neighbors_plus_1[4]) = {
    HIGHBD_NEIGHBOR_CONSTANT_10, HIGHBD_NEIGHBOR_CONSTANT_10, HIGHBD_NEIGHBOR_CONSTANT_10, HIGHBD_NEIGHBOR_CONSTANT_7};

DECLARE_ALIGNED(16, static const uint32_t, high_middle_edge_neighbors_plus_1[4]) = {
    HIGHBD_NEIGHBOR_CONSTANT_7, HIGHBD_NEIGHBOR_CONSTANT_7, HIGHBD_NEIGHBOR_CONSTANT_7, HIGHBD_NEIGHBOR_CONSTANT_7};

DECLARE_ALIGNED(16, static const uint32_t, highbd_middle_center_neighbors_plus_1[4]) = {
    HIGHBD_NEIGHBOR_CONSTANT_10, HIGHBD_NEIGHBOR_CONSTANT_10, HIGHBD_NEIGHBOR_CONSTANT_10, HIGHBD_NEIGHBOR_CONSTANT_10};

DECLARE_ALIGNED(16, static const uint32_t, highbd_left_corner_neighbours_plus_2[4]) = {
    HIGHBD_NEIGHBOR_CONSTANT_6, HIGHBD_NEIGHBOR_CONSTANT_8, HIGHBD_NEIGHBOR_CONSTANT_8, HIGHBD_NEIGHBOR_CONSTANT_8};

DECLARE_ALIGNED(16, static const uint32_t, highbd_right_corner_neighbors_plus_2[4]) = {
    HIGHBD_NEIGHBOR_CONSTANT_8, HIGHBD_NEIGHBOR_CONSTANT_8, HIGHBD_NEIGHBOR_CONSTANT_8, HIGHBD_NEIGHBOR_CONSTANT_6};

DECLARE_ALIGNED(16, static const uint32_t, highbd_left_edge_neighbours_plus_2[4]) = {
    HIGHBD_NEIGHBOR_CONSTANT_8, HIGHBD_NEIGHBOR_CONSTANT_11, HIGHBD_NEIGHBOR_CONSTANT_11, HIGHBD_NEIGHBOR_CONSTANT_11};

DECLARE_ALIGNED(16, static const uint32_t, highbd_right_edge_neighbors_plus_2[4]) = {
    HIGHBD_NEIGHBOR_CONSTANT_11, HIGHBD_NEIGHBOR_CONSTANT_11, HIGHBD_NEIGHBOR_CONSTANT_11, HIGHBD_NEIGHBOR_CONSTANT_8};

DECLARE_ALIGNED(16, static const uint32_t, high_middle_edge_neighbors_plus_2[4]) = {
    HIGHBD_NEIGHBOR_CONSTANT_8, HIGHBD_NEIGHBOR_CONSTANT_8, HIGHBD_NEIGHBOR_CONSTANT_8, HIGHBD_NEIGHBOR_CONSTANT_8};

DECLARE_ALIGNED(16, static const uint32_t, highbd_middle_center_neighbors_plus_2[4]) = {
    HIGHBD_NEIGHBOR_CONSTANT_11, HIGHBD_NEIGHBOR_CONSTANT_11, HIGHBD_NEIGHBOR_CONSTANT_11, HIGHBD_NEIGHBOR_CONSTANT_11};

DECLARE_ALIGNED(16, static const uint32_t, highbd_left_corner_neighbours_plus_4[4]) = {
    HIGHBD_NEIGHBOR_CONSTANT_8, HIGHBD_NEIGHBOR_CONSTANT_10, HIGHBD_NEIGHBOR_CONSTANT_10, HIGHBD_NEIGHBOR_CONSTANT_10};

DECLARE_ALIGNED(16, static const uint32_t, highbd_right_corner_neighbors_plus_4[4]) = {
    HIGHBD_NEIGHBOR_CONSTANT_10, HIGHBD_NEIGHBOR_CONSTANT_10, HIGHBD_NEIGHBOR_CONSTANT_10, HIGHBD_NEIGHBOR_CONSTANT_8};

DECLARE_ALIGNED(16, static const uint32_t, highbd_left_edge_neighbours_plus_4[4]) = {
    HIGHBD_NEIGHBOR_CONSTANT_10, HIGHBD_NEIGHBOR_CONSTANT_13, HIGHBD_NEIGHBOR_CONSTANT_13, HIGHBD_NEIGHBOR_CONSTANT_13};

DECLARE_ALIGNED(16, static const uint32_t, highbd_right_edge_neighbors_plus_4[4]) = {
    HIGHBD_NEIGHBOR_CONSTANT_13, HIGHBD_NEIGHBOR_CONSTANT_13, HIGHBD_NEIGHBOR_CONSTANT_13, HIGHBD_NEIGHBOR_CONSTANT_10};

DECLARE_ALIGNED(16, static const uint32_t, high_middle_edge_neighbors_plus_4[4]) = {
    HIGHBD_NEIGHBOR_CONSTANT_10, HIGHBD_NEIGHBOR_CONSTANT_10, HIGHBD_NEIGHBOR_CONSTANT_10, HIGHBD_NEIGHBOR_CONSTANT_10};

DECLARE_ALIGNED(16, static const uint32_t, highbd_middle_center_neighbors_plus_4[4]) = {
    HIGHBD_NEIGHBOR_CONSTANT_13, HIGHBD_NEIGHBOR_CONSTANT_13, HIGHBD_NEIGHBOR_CONSTANT_13, HIGHBD_NEIGHBOR_CONSTANT_13};

static const uint32_t* const highbd_luma_left_column_neighbors[2] = {highbd_left_corner_neighbours_plus_2,
                                                                     highbd_left_edge_neighbours_plus_2};

static const uint32_t* const highbd_luma_middle_column_neighbors[2] = {high_middle_edge_neighbors_plus_2,
                                                                       highbd_middle_center_neighbors_plus_2};

static const uint32_t* const highbd_luma_right_column_neighbors[2] = {highbd_right_corner_neighbors_plus_2,
                                                                      highbd_right_edge_neighbors_plus_2};

static const uint32_t* const highbd_chroma_no_ss_left_column_neighbors[2] = {highbd_left_corner_neighbours_plus_1,
                                                                             highbd_left_edge_neighbours_plus_1};

static const uint32_t* const highbd_chroma_no_ss_middle_column_neighbors[2] = {high_middle_edge_neighbors_plus_1,
                                                                               highbd_middle_center_neighbors_plus_1};

static const uint32_t* const highbd_chroma_no_ss_right_column_neighbors[2] = {highbd_right_corner_neighbors_plus_1,
                                                                              highbd_right_edge_neighbors_plus_1};

static const uint32_t* const highbd_chroma_single_ss_left_column_neighbors[2] = {highbd_left_corner_neighbours_plus_2,
                                                                                 highbd_left_edge_neighbours_plus_2};

static const uint32_t* const highbd_chroma_single_ss_middle_column_neighbors[2] = {
    high_middle_edge_neighbors_plus_2, highbd_middle_center_neighbors_plus_2};

static const uint32_t* const highbd_chroma_single_ss_right_column_neighbors[2] = {highbd_right_corner_neighbors_plus_2,
                                                                                  highbd_right_edge_neighbors_plus_2};

static const uint32_t* const highbd_chroma_double_ss_left_column_neighbors[2] = {highbd_left_corner_neighbours_plus_4,
                                                                                 highbd_left_edge_neighbours_plus_4};

static const uint32_t* const highbd_chroma_double_ss_middle_column_neighbors[2] = {
    high_middle_edge_neighbors_plus_4, highbd_middle_center_neighbors_plus_4};

static const uint32_t* const highbd_chroma_double_ss_right_column_neighbors[2] = {highbd_right_corner_neighbors_plus_4,
                                                                                  highbd_right_edge_neighbors_plus_4};

#define DIST_STRIDE ((BW) + 2)
