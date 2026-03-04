/*
* Copyright(c) 2019 Intel Corporation
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#include "definitions.h"

#if EN_AVX512_SUPPORT

#include <immintrin.h>
#include "aom_dsp_rtcd.h"
#include "cdef.h"
#include "memory_avx2.h"
#include "synonyms_avx512.h"

uint64_t svt_search_one_dual_avx512(int* lev0, int* lev1, int nb_strengths, uint64_t** mse[2], int sb_count,
                                    int start_gi, int end_gi) {
    const int start        = start_gi & ~7;
    uint64_t  best_tot_mse = (uint64_t)1 << 63;
    int32_t   best_id0     = 0;
    int32_t   best_id1     = 0;
    int32_t   i, j;
    DECLARE_ALIGNED(64, uint64_t, tot_mse[TOTAL_STRENGTHS][TOTAL_STRENGTHS]);

    memset(tot_mse + start_gi * TOTAL_STRENGTHS, 0, sizeof(tot_mse[0][0]) * (end_gi - start_gi) * TOTAL_STRENGTHS);

    for (i = 0; i < sb_count; i++) {
        int32_t  gi;
        uint64_t best_mse = (uint64_t)1 << 63;
        /* Find best mse among already selected options. */
        for (gi = 0; gi < nb_strengths; gi++) {
            uint64_t curr = mse[0][i][lev0[gi]];
            curr += mse[1][i][lev1[gi]];
            if (curr < best_mse) {
                best_mse = curr;
            }
        }

        const __m512i best_mse_ = _mm512_set1_epi64(best_mse);

        /* Find best mse when adding each possible new option. */
        for (j = start_gi; j < end_gi; ++j) {
            int32_t       k;
            const __m512i mse0 = _mm512_set1_epi64(mse[0][i][j]);

            for (k = start; k < end_gi; k += 8) {
                const __m512i mse1 = _mm512_loadu_si512((const __m512i*)&mse[1][i][k]);
                const __m512i tot  = zz_load_512(&tot_mse[j][k]);
                const __m512i curr = _mm512_add_epi64(mse0, mse1);
                const __m512i best = _mm512_min_epu64(best_mse_, curr);
                const __m512i d    = _mm512_add_epi64(tot, best);
                zz_store_512(&tot_mse[j][k], d);
            }
        }
    }

    for (j = start_gi; j < end_gi; j++) {
        int32_t k;
        for (k = start_gi; k < end_gi; k++) {
            if (tot_mse[j][k] < best_tot_mse) {
                best_tot_mse = tot_mse[j][k];
                best_id0     = j;
                best_id1     = k;
            }
        }
    }

    lev0[nb_strengths] = best_id0;
    lev1[nb_strengths] = best_id1;

    return best_tot_mse;
}

#endif // EN_AVX512_SUPPORT
