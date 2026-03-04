/*
 * Copyright(c) 2019 Intel Corporation
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the
 * Alliance for Open Media Patent License 1.0 was not distributed with this
 * source code in the PATENTS file, you can obtain it at
 * https://www.aomedia.org/license/patent-license.
 */

#pragma once
#ifndef EbHighbdIntraPredictionTests_h
#define EbHighbdIntraPredictionTests_h

#include "aom_dsp_rtcd.h"

#if EN_AVX512_SUPPORT
typedef void (*aom_highbd_dc_top_predictor_func)(uint16_t *dst,
                                                 ptrdiff_t stride,
                                                 const uint16_t *above,
                                                 const uint16_t *left,
                                                 int32_t bd);

static aom_highbd_dc_top_predictor_func aom_highbd_dc_top_funcptr_array_opt[7] =
    {aom_highbd_dc_top_predictor_32x8_avx512,
     aom_highbd_dc_top_predictor_32x16_avx512,
     aom_highbd_dc_top_predictor_32x32_avx512,
     aom_highbd_dc_top_predictor_32x64_avx512,
     aom_highbd_dc_top_predictor_64x16_avx512,
     aom_highbd_dc_top_predictor_64x32_avx512,
     aom_highbd_dc_top_predictor_64x64_avx512};

static aom_highbd_dc_top_predictor_func
    aom_highbd_dc_top_funcptr_array_base[7] = {
        svt_aom_highbd_dc_top_predictor_32x8_avx2,
        svt_aom_highbd_dc_top_predictor_32x16_avx2,
        svt_aom_highbd_dc_top_predictor_32x32_avx2,
        svt_aom_highbd_dc_top_predictor_32x64_avx2,
        svt_aom_highbd_dc_top_predictor_64x16_avx2,
        svt_aom_highbd_dc_top_predictor_64x32_avx2,
        svt_aom_highbd_dc_top_predictor_64x64_avx2};

typedef void (*aom_highbd_dc_left_predictor_func)(uint16_t *dst,
                                                  ptrdiff_t stride,
                                                  const uint16_t *above,
                                                  const uint16_t *left,
                                                  int32_t bd);

static aom_highbd_dc_left_predictor_func
    aom_highbd_dc_left_funcptr_array_opt[7] = {
        aom_highbd_dc_left_predictor_32x8_avx512,
        aom_highbd_dc_left_predictor_32x16_avx512,
        aom_highbd_dc_left_predictor_32x32_avx512,
        aom_highbd_dc_left_predictor_32x64_avx512,
        aom_highbd_dc_left_predictor_64x16_avx512,
        aom_highbd_dc_left_predictor_64x32_avx512,
        aom_highbd_dc_left_predictor_64x64_avx512};

static aom_highbd_dc_left_predictor_func
    aom_highbd_dc_left_funcptr_array_base[7] = {
        svt_aom_highbd_dc_left_predictor_32x8_avx2,
        svt_aom_highbd_dc_left_predictor_32x16_avx2,
        svt_aom_highbd_dc_left_predictor_32x32_avx2,
        svt_aom_highbd_dc_left_predictor_32x64_avx2,
        svt_aom_highbd_dc_left_predictor_64x16_avx2,
        svt_aom_highbd_dc_left_predictor_64x32_avx2,
        svt_aom_highbd_dc_left_predictor_64x64_avx2};

typedef void (*aom_highbd_dc_predictor_func)(uint16_t *dst, ptrdiff_t stride,
                                             const uint16_t *above,
                                             const uint16_t *left, int32_t bd);

static aom_highbd_dc_predictor_func aom_highbd_dc_pred_funcptr_array_opt[7] = {
    aom_highbd_dc_predictor_32x8_avx512,
    aom_highbd_dc_predictor_32x16_avx512,
    aom_highbd_dc_predictor_32x32_avx512,
    aom_highbd_dc_predictor_32x64_avx512,
    aom_highbd_dc_predictor_64x16_avx512,
    aom_highbd_dc_predictor_64x32_avx512,
    aom_highbd_dc_predictor_64x64_avx512};

static aom_highbd_dc_predictor_func aom_highbd_dc_pred_funcptr_array_base[7] = {
    svt_aom_highbd_dc_predictor_32x8_avx2,
    svt_aom_highbd_dc_predictor_32x16_avx2,
    svt_aom_highbd_dc_predictor_32x32_avx2,
    svt_aom_highbd_dc_predictor_32x64_avx2,
    svt_aom_highbd_dc_predictor_64x16_avx2,
    svt_aom_highbd_dc_predictor_64x32_avx2,
    svt_aom_highbd_dc_predictor_64x64_avx2};

typedef void (*aom_highbd_h_predictor_func)(uint16_t *dst, ptrdiff_t stride,
                                            const uint16_t *above,
                                            const uint16_t *left, int32_t bd);

static aom_highbd_h_predictor_func aom_highbd_h_pred_funcptr_array_opt[7] = {
    aom_highbd_h_predictor_32x8_avx512,
    aom_highbd_h_predictor_32x16_avx512,
    aom_highbd_h_predictor_32x32_avx512,
    aom_highbd_h_predictor_32x64_avx512,
    aom_highbd_h_predictor_64x16_avx512,
    aom_highbd_h_predictor_64x32_avx512,
    aom_highbd_h_predictor_64x64_avx512};

static aom_highbd_h_predictor_func aom_highbd_h_pred_funcptr_array_base[7] = {
    svt_aom_highbd_h_predictor_32x8_avx2,
    svt_aom_highbd_h_predictor_32x16_sse2,
    svt_aom_highbd_h_predictor_32x32_sse2,
    svt_aom_highbd_h_predictor_32x64_avx2,
    svt_aom_highbd_h_predictor_64x16_avx2,
    svt_aom_highbd_h_predictor_64x32_avx2,
    svt_aom_highbd_h_predictor_64x64_avx2};

typedef void (*aom_highbd_v_predictor_func)(uint16_t *dst, ptrdiff_t stride,
                                            const uint16_t *above,
                                            const uint16_t *left, int32_t bd);

static aom_highbd_v_predictor_func aom_highbd_v_pred_funcptr_array_opt[7] = {
    aom_highbd_v_predictor_32x8_avx512,
    aom_highbd_v_predictor_32x16_avx512,
    aom_highbd_v_predictor_32x32_avx512,
    aom_highbd_v_predictor_32x64_avx512,
    aom_highbd_v_predictor_64x16_avx512,
    aom_highbd_v_predictor_64x32_avx512,
    aom_highbd_v_predictor_64x64_avx512};

static aom_highbd_v_predictor_func aom_highbd_v_pred_funcptr_array_base[7] = {
    svt_aom_highbd_v_predictor_32x8_avx2,
    svt_aom_highbd_v_predictor_32x16_avx2,
    svt_aom_highbd_v_predictor_32x32_avx2,
    svt_aom_highbd_v_predictor_32x64_avx2,
    svt_aom_highbd_v_predictor_64x16_avx2,
    svt_aom_highbd_v_predictor_64x32_avx2,
    svt_aom_highbd_v_predictor_64x64_avx2};

typedef void (*aom_highbd_smooth_predictor_func)(uint16_t *dst,
                                                 ptrdiff_t stride,
                                                 const uint16_t *above,
                                                 const uint16_t *left,
                                                 int32_t bd);

static aom_highbd_smooth_predictor_func
    aom_smooth_predictor_funcptr_array_opt[7] = {
        aom_highbd_smooth_predictor_32x8_avx512,
        aom_highbd_smooth_predictor_32x16_avx512,
        aom_highbd_smooth_predictor_32x32_avx512,
        aom_highbd_smooth_predictor_32x64_avx512,
        aom_highbd_smooth_predictor_64x16_avx512,
        aom_highbd_smooth_predictor_64x32_avx512,
        aom_highbd_smooth_predictor_64x64_avx512};

static aom_highbd_smooth_predictor_func
    aom_smooth_predictor_funcptr_array_base[7] = {
        svt_aom_highbd_smooth_predictor_32x8_avx2,
        svt_aom_highbd_smooth_predictor_32x16_avx2,
        svt_aom_highbd_smooth_predictor_32x32_avx2,
        svt_aom_highbd_smooth_predictor_32x64_avx2,
        svt_aom_highbd_smooth_predictor_64x16_avx2,
        svt_aom_highbd_smooth_predictor_64x32_avx2,
        svt_aom_highbd_smooth_predictor_64x64_avx2};

typedef void (*aom_highbd_smooth_v_predictor_func)(uint16_t *dst,
                                                   ptrdiff_t stride,
                                                   const uint16_t *above,
                                                   const uint16_t *left,
                                                   int32_t bd);

static aom_highbd_smooth_v_predictor_func
    aom_highbd_smooth_v_predictor_funcptr_array_opt[7] = {
        aom_highbd_smooth_v_predictor_32x8_avx512,
        aom_highbd_smooth_v_predictor_32x16_avx512,
        aom_highbd_smooth_v_predictor_32x32_avx512,
        aom_highbd_smooth_v_predictor_32x64_avx512,
        aom_highbd_smooth_v_predictor_64x16_avx512,
        aom_highbd_smooth_v_predictor_64x32_avx512,
        aom_highbd_smooth_v_predictor_64x64_avx512};

static aom_highbd_smooth_v_predictor_func
    aom_highbd_smooth_v_predictor_funcptr_array_base[7] = {
        svt_aom_highbd_smooth_v_predictor_32x8_avx2,
        svt_aom_highbd_smooth_v_predictor_32x16_avx2,
        svt_aom_highbd_smooth_v_predictor_32x32_avx2,
        svt_aom_highbd_smooth_v_predictor_32x64_avx2,
        svt_aom_highbd_smooth_v_predictor_64x16_avx2,
        svt_aom_highbd_smooth_v_predictor_64x32_avx2,
        svt_aom_highbd_smooth_v_predictor_64x64_avx2};

typedef void (*aom_highbd_smooth_h_predictor_func)(uint16_t *dst,
                                                   ptrdiff_t stride,
                                                   const uint16_t *above,
                                                   const uint16_t *left,
                                                   int32_t bd);

static aom_highbd_smooth_h_predictor_func
    aom_highbd_smooth_h_predictor_funcptr_array_opt[7] = {
        aom_highbd_smooth_h_predictor_32x8_avx512,
        aom_highbd_smooth_h_predictor_32x16_avx512,
        aom_highbd_smooth_h_predictor_32x32_avx512,
        aom_highbd_smooth_h_predictor_32x64_avx512,
        aom_highbd_smooth_h_predictor_64x16_avx512,
        aom_highbd_smooth_h_predictor_64x32_avx512,
        aom_highbd_smooth_h_predictor_64x64_avx512};

static aom_highbd_smooth_h_predictor_func
    aom_highbd_smooth_h_predictor_funcptr_array_base[7] = {
        svt_aom_highbd_smooth_h_predictor_32x8_avx2,
        svt_aom_highbd_smooth_h_predictor_32x16_avx2,
        svt_aom_highbd_smooth_h_predictor_32x32_avx2,
        svt_aom_highbd_smooth_h_predictor_32x64_avx2,
        svt_aom_highbd_smooth_h_predictor_64x16_avx2,
        svt_aom_highbd_smooth_h_predictor_64x32_avx2,
        svt_aom_highbd_smooth_h_predictor_64x64_avx2};

#endif
#endif
