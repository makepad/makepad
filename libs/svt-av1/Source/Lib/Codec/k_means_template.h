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

#include <assert.h>
#include <stdint.h>
#include <string.h>
#include "utility.h"
#include "random.h"

#ifndef AV1_K_MEANS_DIM
#error "This template requires AV1_K_MEANS_DIM to be defined"
#endif

#define RENAME_(x, y) AV1_K_MEANS_RENAME(x, y)
#define RENAME(x) RENAME_(x, AV1_K_MEANS_DIM)

static INLINE int RENAME(calc_dist)(const int* p1, const int* p2) {
    int dist = 0;
    for (int i = 0; i < AV1_K_MEANS_DIM; ++i) {
        dist += SQR(p1[i] - p2[i]);
    }
    return dist;
}

void RENAME(svt_av1_calc_indices)(const int* data, const int* centroids, uint8_t* indices, int n, int k) {
    for (int i = 0; i < n; ++i) {
        int min_dist = RENAME(calc_dist)(data + i * AV1_K_MEANS_DIM, centroids);
        indices[i]   = 0;
        for (int j = 1; j < k; ++j) {
            const int this_dist = RENAME(calc_dist)(data + i * AV1_K_MEANS_DIM, centroids + j * AV1_K_MEANS_DIM);
            if (this_dist < min_dist) {
                min_dist   = this_dist;
                indices[i] = j;
            }
        }
    }
}

static INLINE void RENAME(calc_centroids)(const int* data, int* centroids, const uint8_t* indices, int n, int k) {
    int          i, j;
    int          count[PALETTE_MAX_SIZE] = {0};
    unsigned int rand_state              = (unsigned int)data[0];
    assert(n <= 32768);
    memset(centroids, 0, sizeof(centroids[0]) * k * AV1_K_MEANS_DIM);

    for (i = 0; i < n; ++i) {
        const int index = indices[i];
        assert(index < k);
        ++count[index];
        for (j = 0; j < AV1_K_MEANS_DIM; ++j) {
            centroids[index * AV1_K_MEANS_DIM + j] += data[i * AV1_K_MEANS_DIM + j];
        }
    }

    if (svt_memcpy != NULL) {
        for (i = 0; i < k; ++i) {
            if (count[i] == 0) {
                svt_memcpy(centroids + i * AV1_K_MEANS_DIM,
                           data + (lcg_rand16(&rand_state) % n) * AV1_K_MEANS_DIM,
                           sizeof(centroids[0]) * AV1_K_MEANS_DIM);
            } else {
                for (j = 0; j < AV1_K_MEANS_DIM; ++j) {
                    centroids[i * AV1_K_MEANS_DIM + j] = DIVIDE_AND_ROUND(centroids[i * AV1_K_MEANS_DIM + j], count[i]);
                }
            }
        }
    } else {
        for (i = 0; i < k; ++i) {
            if (count[i] == 0) {
                svt_memcpy_c(centroids + i * AV1_K_MEANS_DIM,
                             data + (lcg_rand16(&rand_state) % n) * AV1_K_MEANS_DIM,
                             sizeof(centroids[0]) * AV1_K_MEANS_DIM);
            } else {
                for (j = 0; j < AV1_K_MEANS_DIM; ++j) {
                    centroids[i * AV1_K_MEANS_DIM + j] = DIVIDE_AND_ROUND(centroids[i * AV1_K_MEANS_DIM + j], count[i]);
                }
            }
        }
    }
}

static INLINE int64_t RENAME(calc_total_dist)(const int* data, const int* centroids, const uint8_t* indices, int n,
                                              int k) {
    int64_t dist = 0;
    (void)k;
    for (int i = 0; i < n; ++i) {
        dist += RENAME(calc_dist)(data + i * AV1_K_MEANS_DIM, centroids + indices[i] * AV1_K_MEANS_DIM);
    }
    return dist;
}

void RENAME(svt_av1_k_means)(const int* data, int* centroids, uint8_t* indices, int n, int k, int max_itr) {
    int     pre_centroids[2 * PALETTE_MAX_SIZE];
    uint8_t pre_indices[MAX_SB_SQUARE];

    RENAME(svt_av1_calc_indices)(data, centroids, indices, n, k);
    int64_t this_dist = RENAME(calc_total_dist)(data, centroids, indices, n, k);

    if (svt_memcpy != NULL) {
        for (int i = 0; i < max_itr; ++i) {
            const int64_t pre_dist = this_dist;
            svt_memcpy(pre_centroids, centroids, sizeof(pre_centroids[0]) * k * AV1_K_MEANS_DIM);
            svt_memcpy(pre_indices, indices, sizeof(pre_indices[0]) * n);

            RENAME(calc_centroids)(data, centroids, indices, n, k);
            RENAME(svt_av1_calc_indices)(data, centroids, indices, n, k);
            this_dist = RENAME(calc_total_dist)(data, centroids, indices, n, k);

            if (this_dist > pre_dist) {
                svt_memcpy(centroids, pre_centroids, sizeof(pre_centroids[0]) * k * AV1_K_MEANS_DIM);
                svt_memcpy(indices, pre_indices, sizeof(pre_indices[0]) * n);
                break;
            }
            if (!memcmp(centroids, pre_centroids, sizeof(pre_centroids[0]) * k * AV1_K_MEANS_DIM)) {
                break;
            }
        }
    } else {
        for (int i = 0; i < max_itr; ++i) {
            const int64_t pre_dist = this_dist;
            svt_memcpy_c(pre_centroids, centroids, sizeof(pre_centroids[0]) * k * AV1_K_MEANS_DIM);
            svt_memcpy_c(pre_indices, indices, sizeof(pre_indices[0]) * n);

            RENAME(calc_centroids)(data, centroids, indices, n, k);
            RENAME(svt_av1_calc_indices)(data, centroids, indices, n, k);
            this_dist = RENAME(calc_total_dist)(data, centroids, indices, n, k);

            if (this_dist > pre_dist) {
                svt_memcpy_c(centroids, pre_centroids, sizeof(pre_centroids[0]) * k * AV1_K_MEANS_DIM);
                svt_memcpy_c(indices, pre_indices, sizeof(pre_indices[0]) * n);
                break;
            }
            if (!memcmp(centroids, pre_centroids, sizeof(pre_centroids[0]) * k * AV1_K_MEANS_DIM)) {
                break;
            }
        }
    }
}

#undef RENAME_
#undef RENAME
