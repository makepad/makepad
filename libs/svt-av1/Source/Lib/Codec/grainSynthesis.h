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

/*!\file
  * \brief Describes film grain parameters and film grain synthesis
  *
  */
#ifndef AOM_AOM_GRAIN_SYNTHESIS_H_
#define AOM_AOM_GRAIN_SYNTHESIS_H_

#include "definitions.h"
#include "common_dsp_rtcd.h"
#ifdef __cplusplus
extern "C" {
#endif

#include <stdio.h>

int32_t svt_aom_film_grain_params_equal(AomFilmGrain* pars_a, AomFilmGrain* pars_b);

/*!\brief Add film grain
     *
     * Add film grain to an image
     *
     * \param[in]    grain_params     Grain parameters
     * \param[in]    luma             luma plane
     * \param[in]    cb               cb plane
     * \param[in]    cr               cr plane
     * \param[in]    height           luma plane height
     * \param[in]    width            luma plane width
     * \param[in]    luma_stride      luma plane stride
     * \param[in]    chroma_stride    chroma plane stride
     */
void svt_av1_add_film_grain_run(AomFilmGrain* grain_params, uint8_t* luma, uint8_t* cb, uint8_t* cr, int32_t height,
                                int32_t width, int32_t luma_stride, int32_t chroma_stride, int32_t use_high_bit_depth,
                                int32_t chroma_subsamp_y, int32_t chroma_subsamp_x);

void svt_aom_fgn_copy_rect(uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride, int32_t width,
                           int32_t height, int32_t use_high_bit_depth);

#ifdef __cplusplus
} // extern "C"
#endif

#endif // AOM_AOM_GRAIN_SYNTHESIS_H_
