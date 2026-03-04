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

#include "global_me_cost.h"
#include "entropy_coding.h"
#include "global_motion.h"

static int aom_count_signed_primitive_refsubexpfin(uint16_t n, uint16_t k, int16_t ref, int16_t v) {
    ref += n - 1;
    v += n - 1;
    const uint16_t scaled_n = (n << 1) - 1;
    return svt_aom_count_primitive_refsubexpfin(scaled_n, k, ref, v);
}

int svt_aom_gm_get_params_cost(const WarpedMotionParams* gm, const WarpedMotionParams* ref_gm, int allow_hp) {
    int params_cost = 0;
    int trans_bits, trans_prec_diff;
    switch (gm->wmtype) {
    case AFFINE:
    case ROTZOOM:
        params_cost += aom_count_signed_primitive_refsubexpfin(
            GM_ALPHA_MAX + 1,
            SUBEXPFIN_K,
            (ref_gm->wmmat[2] >> GM_ALPHA_PREC_DIFF) - (1 << GM_ALPHA_PREC_BITS),
            (gm->wmmat[2] >> GM_ALPHA_PREC_DIFF) - (1 << GM_ALPHA_PREC_BITS));
        params_cost += aom_count_signed_primitive_refsubexpfin(GM_ALPHA_MAX + 1,
                                                               SUBEXPFIN_K,
                                                               (ref_gm->wmmat[3] >> GM_ALPHA_PREC_DIFF),
                                                               (gm->wmmat[3] >> GM_ALPHA_PREC_DIFF));
        if (gm->wmtype >= AFFINE) {
            params_cost += aom_count_signed_primitive_refsubexpfin(GM_ALPHA_MAX + 1,
                                                                   SUBEXPFIN_K,
                                                                   (ref_gm->wmmat[4] >> GM_ALPHA_PREC_DIFF),
                                                                   (gm->wmmat[4] >> GM_ALPHA_PREC_DIFF));
            params_cost += aom_count_signed_primitive_refsubexpfin(
                GM_ALPHA_MAX + 1,
                SUBEXPFIN_K,
                (ref_gm->wmmat[5] >> GM_ALPHA_PREC_DIFF) - (1 << GM_ALPHA_PREC_BITS),
                (gm->wmmat[5] >> GM_ALPHA_PREC_DIFF) - (1 << GM_ALPHA_PREC_BITS));
        }
        AOM_FALLTHROUGH_INTENDED;
    case TRANSLATION:
        trans_bits      = (gm->wmtype == TRANSLATION) ? GM_ABS_TRANS_ONLY_BITS - !allow_hp : GM_ABS_TRANS_BITS;
        trans_prec_diff = (gm->wmtype == TRANSLATION) ? GM_TRANS_ONLY_PREC_DIFF + !allow_hp : GM_TRANS_PREC_DIFF;
        params_cost += aom_count_signed_primitive_refsubexpfin((1 << trans_bits) + 1,
                                                               SUBEXPFIN_K,
                                                               (ref_gm->wmmat[0] >> trans_prec_diff),
                                                               (gm->wmmat[0] >> trans_prec_diff));
        params_cost += aom_count_signed_primitive_refsubexpfin((1 << trans_bits) + 1,
                                                               SUBEXPFIN_K,
                                                               (ref_gm->wmmat[1] >> trans_prec_diff),
                                                               (gm->wmmat[1] >> trans_prec_diff));
        AOM_FALLTHROUGH_INTENDED;
    case IDENTITY:
        break;
    default:
        assert(0);
    }
    return (params_cost << AV1_PROB_COST_SHIFT);
}
