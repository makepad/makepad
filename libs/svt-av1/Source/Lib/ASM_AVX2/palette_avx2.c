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

#include <immintrin.h>
#include "definitions.h"
#include "common_dsp_rtcd.h"
#include "random.h"
#define DIVIDE_AND_ROUND(x, y) (((x) + ((y) >> 1)) / (y))

/* That same calculation as: av1_calc_indices_dist_dim1_avx2(),
   but not calculate sum at the end. */
void svt_av1_calc_indices_dim1_avx2(const int* data, const int* centroids, uint8_t* indices, int n, int k) {
    int i = 0;
    int results[MAX_SB_SQUARE];
    memset(indices, 0, n * sizeof(uint8_t));

    __m256i centroids_0 = _mm256_set1_epi32(centroids[0]);
    for (i = 0; i < n; i += 8) {
        __m256i data_dd = _mm256_loadu_si256((__m256i*)(data + i));
        __m256i sub     = _mm256_sub_epi32(data_dd, centroids_0);
        __m256i dist    = _mm256_mullo_epi32(sub, sub);
        _mm256_storeu_si256((__m256i*)(results + i), dist);
    }

    for (int c = 1; c < k; c++) {
        centroids_0       = _mm256_set1_epi32(centroids[c]);
        __m256i indices_v = _mm256_set1_epi32(c);
        for (i = 0; i < n; i += 16) {
            __m256i data_d1 = _mm256_loadu_si256((__m256i*)(data + i));
            __m256i data_d2 = _mm256_loadu_si256((__m256i*)(data + i + 8));
            __m256i sub_1   = _mm256_sub_epi32(data_d1, centroids_0);
            __m256i sub_2   = _mm256_sub_epi32(data_d2, centroids_0);
            __m256i dist_1  = _mm256_mullo_epi32(sub_1, sub_1);
            __m256i dist_2  = _mm256_mullo_epi32(sub_2, sub_2);

            __m256i prev_1 = _mm256_loadu_si256((__m256i*)(results + i));
            __m256i prev_2 = _mm256_loadu_si256((__m256i*)(results + i + 8));
            __m256i cmp_1  = _mm256_cmpgt_epi32(prev_1, dist_1);
            __m256i cmp_2  = _mm256_cmpgt_epi32(prev_2, dist_2);

            _mm256_maskstore_epi32((results + i), cmp_1, dist_1);
            _mm256_maskstore_epi32((results + i + 8), cmp_2, dist_2);

            __m256i indices_v32_1 = _mm256_and_si256(indices_v, cmp_1);
            __m256i indices_v32_2 = _mm256_and_si256(indices_v, cmp_2);

            __m128i indices_v16_1 = _mm_packus_epi32(_mm256_castsi256_si128(indices_v32_1),
                                                     _mm256_extracti128_si256(indices_v32_1, 1));
            __m128i indices_v16_2 = _mm_packus_epi32(_mm256_castsi256_si128(indices_v32_2),
                                                     _mm256_extracti128_si256(indices_v32_2, 1));

            __m128i cmp8 = _mm_packs_epi16(indices_v16_1, indices_v16_2);

            __m128i load_ind = _mm_loadu_si128((__m128i*)(indices + i));

            load_ind = _mm_max_epi8(load_ind, cmp8);

            _mm_storeu_si128((__m128i*)(indices + i), load_ind);
        }
    }
}

static INLINE int64_t av1_calc_indices_dist_dim1_avx2(const int* data, const int* centroids, uint8_t* indices,
                                                      unsigned n, int k) {
    __m256i sum64 = _mm256_setzero_si256();
    int     results[MAX_SB_SQUARE];
    memset(indices, 0, n * sizeof(uint8_t));

    __m256i centroids_0 = _mm256_set1_epi32(centroids[0]);
    for (unsigned i = 0; i < n; i += 8) {
        __m256i data_dd = _mm256_loadu_si256((__m256i*)(data + i));
        __m256i sub     = _mm256_sub_epi32(data_dd, centroids_0);
        __m256i dist    = _mm256_mullo_epi32(sub, sub);
        _mm256_storeu_si256((__m256i*)(results + i), dist);
    }

    for (int c = 1; c < k; c++) {
        centroids_0       = _mm256_set1_epi32(centroids[c]);
        __m256i indices_v = _mm256_set1_epi32(c);
        for (unsigned i = 0; i < n; i += 16) {
            __m256i data_d1 = _mm256_loadu_si256((__m256i*)(data + i));
            __m256i data_d2 = _mm256_loadu_si256((__m256i*)(data + i + 8));
            __m256i sub_1   = _mm256_sub_epi32(data_d1, centroids_0);
            __m256i sub_2   = _mm256_sub_epi32(data_d2, centroids_0);
            __m256i dist_1  = _mm256_mullo_epi32(sub_1, sub_1);
            __m256i dist_2  = _mm256_mullo_epi32(sub_2, sub_2);

            __m256i prev_1 = _mm256_loadu_si256((__m256i*)(results + i));
            __m256i prev_2 = _mm256_loadu_si256((__m256i*)(results + i + 8));
            __m256i cmp_1  = _mm256_cmpgt_epi32(prev_1, dist_1);
            __m256i cmp_2  = _mm256_cmpgt_epi32(prev_2, dist_2);

            _mm256_maskstore_epi32((results + i), cmp_1, dist_1);
            _mm256_maskstore_epi32((results + i + 8), cmp_2, dist_2);

            __m256i indices_v32_1 = _mm256_and_si256(indices_v, cmp_1);
            __m256i indices_v32_2 = _mm256_and_si256(indices_v, cmp_2);

            __m128i indices_v16_1 = _mm_packus_epi32(_mm256_castsi256_si128(indices_v32_1),
                                                     _mm256_extracti128_si256(indices_v32_1, 1));
            __m128i indices_v16_2 = _mm_packus_epi32(_mm256_castsi256_si128(indices_v32_2),
                                                     _mm256_extracti128_si256(indices_v32_2, 1));

            __m128i cmp8 = _mm_packs_epi16(indices_v16_1, indices_v16_2);

            __m128i load_ind = _mm_loadu_si128((__m128i*)(indices + i));

            load_ind = _mm_max_epi8(load_ind, cmp8);

            _mm_storeu_si128((__m128i*)(indices + i), load_ind);
        }
    }

    for (unsigned i = 0; i < n; i += 8) {
        __m256i prev = _mm256_loadu_si256((__m256i*)(results + i));
        sum64        = _mm256_add_epi64(sum64, _mm256_unpacklo_epi32(prev, _mm256_setzero_si256()));
        sum64        = _mm256_add_epi64(sum64, _mm256_unpackhi_epi32(prev, _mm256_setzero_si256()));
    }

    __m128i s = _mm_add_epi64(_mm256_castsi256_si128(sum64), _mm256_extracti128_si256(sum64, 1));

    return _mm_extract_epi64(s, 0) + _mm_extract_epi64(s, 1);
}

static INLINE void calc_centroids_1_avx2(const int* data, int* centroids, const uint8_t* indices, int n, int k) {
    int          i;
    int          count[PALETTE_MAX_SIZE] = {0};
    unsigned int rand_state              = (unsigned int)data[0];
    assert(n <= 32768);
    memset(centroids, 0, sizeof(centroids[0]) * k);

    for (i = 0; i < n; ++i) {
        const int index = indices[i];
        assert(index < k);
        ++count[index];
        centroids[index] += data[i];
    }

    for (i = 0; i < k; ++i) {
        if (count[i] == 0) {
            centroids[i] = *(data + (lcg_rand16(&rand_state) % n));
        } else {
            centroids[i] = DIVIDE_AND_ROUND(centroids[i], count[i]);
        }
    }
}

void svt_av1_k_means_dim1_avx2(const int* data, int* centroids, uint8_t* indices, int n, int k, int max_itr) {
    int     pre_centroids[2 * PALETTE_MAX_SIZE];
    uint8_t pre_indices[MAX_SB_SQUARE];
    assert((n & 15) == 0);

    int64_t this_dist = av1_calc_indices_dist_dim1_avx2(data, centroids, indices, n, k);

    for (int i = 0; i < max_itr; ++i) {
        const int64_t pre_dist = this_dist;
        svt_memcpy_intrin_sse(pre_centroids, centroids, sizeof(pre_centroids[0]) * k);
        svt_memcpy_intrin_sse(pre_indices, indices, sizeof(pre_indices[0]) * n);

        calc_centroids_1_avx2(data, centroids, indices, n, k);
        this_dist = av1_calc_indices_dist_dim1_avx2(data, centroids, indices, n, k);

        if (this_dist > pre_dist) {
            svt_memcpy_intrin_sse(centroids, pre_centroids, sizeof(pre_centroids[0]) * k);
            svt_memcpy_intrin_sse(indices, pre_indices, sizeof(pre_indices[0]) * n);
            break;
        }
        if (!memcmp(centroids, pre_centroids, sizeof(pre_centroids[0]) * k)) {
            break;
        }
    }
}

/* That same calculation as: av1_calc_indices_dist_dim2_avx2(),
   but not calculate sum at the end. */
void svt_av1_calc_indices_dim2_avx2(const int* data, const int* centroids, uint8_t* indices, int n, int k) {
    int results[MAX_SB_SQUARE];
    memset(indices, 0, n * sizeof(uint8_t));

    __m256i centroids_01 = _mm256_set1_epi64x(*((uint64_t*)&centroids[0]));

    for (int i = 0; i < n; i += 8) {
        __m256i data_a = _mm256_loadu_si256((__m256i*)(data + 2 * i));
        __m256i sub_a  = _mm256_sub_epi32(data_a, centroids_01);
        __m256i dist_a = _mm256_mullo_epi32(sub_a, sub_a);

        __m256i data_b = _mm256_loadu_si256((__m256i*)(data + 2 * (i + 4)));
        __m256i sub_b  = _mm256_sub_epi32(data_b, centroids_01);
        __m256i dist_b = _mm256_mullo_epi32(sub_b, sub_b);

        __m256i dist = _mm256_hadd_epi32(dist_a, dist_b);
        dist         = _mm256_permute4x64_epi64(dist, 0xD8);
        _mm256_storeu_si256((__m256i*)(results + i), dist);
    }

    for (int j = 1; j < k; ++j) {
        centroids_01      = _mm256_set1_epi64x(*((uint64_t*)&centroids[2 * j]));
        __m256i indices_v = _mm256_set1_epi32(j);

        for (int i = 0; i < n; i += 16) {
            __m256i data_1 = _mm256_loadu_si256((__m256i*)(data + 2 * i));
            __m256i data_2 = _mm256_loadu_si256((__m256i*)(data + 2 * (i + 4)));
            __m256i data_3 = _mm256_loadu_si256((__m256i*)(data + 2 * (i + 8)));
            __m256i data_4 = _mm256_loadu_si256((__m256i*)(data + 2 * (i + 12)));

            __m256i sub_1 = _mm256_sub_epi32(data_1, centroids_01);
            __m256i sub_2 = _mm256_sub_epi32(data_2, centroids_01);
            __m256i sub_3 = _mm256_sub_epi32(data_3, centroids_01);
            __m256i sub_4 = _mm256_sub_epi32(data_4, centroids_01);

            __m256i dist_1 = _mm256_mullo_epi32(sub_1, sub_1);
            __m256i dist_2 = _mm256_mullo_epi32(sub_2, sub_2);
            __m256i dist_3 = _mm256_mullo_epi32(sub_3, sub_3);
            __m256i dist_4 = _mm256_mullo_epi32(sub_4, sub_4);

            __m256i dist12 = _mm256_hadd_epi32(dist_1, dist_2);
            dist12         = _mm256_permute4x64_epi64(dist12, 0xD8);

            __m256i dist34 = _mm256_hadd_epi32(dist_3, dist_4);
            dist34         = _mm256_permute4x64_epi64(dist34, 0xD8);

            __m256i prev_12 = _mm256_loadu_si256((__m256i*)(results + i));
            __m256i prev_34 = _mm256_loadu_si256((__m256i*)(results + i + 8));

            __m256i cmp_12 = _mm256_cmpgt_epi32(prev_12, dist12);
            __m256i cmp_34 = _mm256_cmpgt_epi32(prev_34, dist34);

            _mm256_maskstore_epi32((results + i), cmp_12, dist12);
            _mm256_maskstore_epi32((results + i + 8), cmp_34, dist34);

            __m256i indices_v32_1 = _mm256_and_si256(indices_v, cmp_12);
            __m256i indices_v32_2 = _mm256_and_si256(indices_v, cmp_34);

            __m128i indices_v16_1 = _mm_packus_epi32(_mm256_castsi256_si128(indices_v32_1),
                                                     _mm256_extracti128_si256(indices_v32_1, 1));
            __m128i indices_v16_2 = _mm_packus_epi32(_mm256_castsi256_si128(indices_v32_2),
                                                     _mm256_extracti128_si256(indices_v32_2, 1));

            __m128i cmp8     = _mm_packs_epi16(indices_v16_1, indices_v16_2);
            __m128i load_ind = _mm_loadu_si128((__m128i*)(indices + i));
            load_ind         = _mm_max_epi8(load_ind, cmp8);
            _mm_storeu_si128((__m128i*)(indices + i), load_ind);
        }
    }
}

static INLINE int64_t av1_calc_indices_dist_dim2_avx2(const int* data, const int* centroids, uint8_t* indices,
                                                      unsigned n, int k) {
    int results[MAX_SB_SQUARE];
    memset(indices, 0, n * sizeof(uint8_t));

    __m256i centroids_01 = _mm256_set1_epi64x(*((uint64_t*)&centroids[0]));

    for (unsigned i = 0; i < n; i += 8) {
        __m256i data_a = _mm256_loadu_si256((__m256i*)(data + 2 * i));
        __m256i sub_a  = _mm256_sub_epi32(data_a, centroids_01);
        __m256i dist_a = _mm256_mullo_epi32(sub_a, sub_a);

        __m256i data_b = _mm256_loadu_si256((__m256i*)(data + 2 * (i + 4)));
        __m256i sub_b  = _mm256_sub_epi32(data_b, centroids_01);
        __m256i dist_b = _mm256_mullo_epi32(sub_b, sub_b);

        __m256i dist = _mm256_hadd_epi32(dist_a, dist_b);
        dist         = _mm256_permute4x64_epi64(dist, 0xD8);
        _mm256_storeu_si256((__m256i*)(results + i), dist);
    }

    for (int j = 1; j < k; ++j) {
        centroids_01      = _mm256_set1_epi64x(*((uint64_t*)&centroids[2 * j]));
        __m256i indices_v = _mm256_set1_epi32(j);

        for (unsigned i = 0; i < n; i += 16) {
            __m256i data_1 = _mm256_loadu_si256((__m256i*)(data + 2 * i));
            __m256i data_2 = _mm256_loadu_si256((__m256i*)(data + 2 * (i + 4)));
            __m256i data_3 = _mm256_loadu_si256((__m256i*)(data + 2 * (i + 8)));
            __m256i data_4 = _mm256_loadu_si256((__m256i*)(data + 2 * (i + 12)));

            __m256i sub_1 = _mm256_sub_epi32(data_1, centroids_01);
            __m256i sub_2 = _mm256_sub_epi32(data_2, centroids_01);
            __m256i sub_3 = _mm256_sub_epi32(data_3, centroids_01);
            __m256i sub_4 = _mm256_sub_epi32(data_4, centroids_01);

            __m256i dist_1 = _mm256_mullo_epi32(sub_1, sub_1);
            __m256i dist_2 = _mm256_mullo_epi32(sub_2, sub_2);
            __m256i dist_3 = _mm256_mullo_epi32(sub_3, sub_3);
            __m256i dist_4 = _mm256_mullo_epi32(sub_4, sub_4);

            __m256i dist12 = _mm256_hadd_epi32(dist_1, dist_2);
            dist12         = _mm256_permute4x64_epi64(dist12, 0xD8);

            __m256i dist34 = _mm256_hadd_epi32(dist_3, dist_4);
            dist34         = _mm256_permute4x64_epi64(dist34, 0xD8);

            __m256i prev_12 = _mm256_loadu_si256((__m256i*)(results + i));
            __m256i prev_34 = _mm256_loadu_si256((__m256i*)(results + i + 8));

            __m256i cmp_12 = _mm256_cmpgt_epi32(prev_12, dist12);
            __m256i cmp_34 = _mm256_cmpgt_epi32(prev_34, dist34);

            _mm256_maskstore_epi32((results + i), cmp_12, dist12);
            _mm256_maskstore_epi32((results + i + 8), cmp_34, dist34);

            __m256i indices_v32_1 = _mm256_and_si256(indices_v, cmp_12);
            __m256i indices_v32_2 = _mm256_and_si256(indices_v, cmp_34);

            __m128i indices_v16_1 = _mm_packus_epi32(_mm256_castsi256_si128(indices_v32_1),
                                                     _mm256_extracti128_si256(indices_v32_1, 1));
            __m128i indices_v16_2 = _mm_packus_epi32(_mm256_castsi256_si128(indices_v32_2),
                                                     _mm256_extracti128_si256(indices_v32_2, 1));

            __m128i cmp8     = _mm_packs_epi16(indices_v16_1, indices_v16_2);
            __m128i load_ind = _mm_loadu_si128((__m128i*)(indices + i));
            load_ind         = _mm_max_epi8(load_ind, cmp8);
            _mm_storeu_si128((__m128i*)(indices + i), load_ind);
        }
    }

    int64_t dist  = 0;
    __m256i sum64 = _mm256_setzero_si256();
    for (unsigned i = 0; i < n; i += 8) {
        __m256i prev = _mm256_loadu_si256((__m256i*)(results + i));
        sum64        = _mm256_add_epi64(sum64, _mm256_unpacklo_epi32(prev, _mm256_setzero_si256()));
        sum64        = _mm256_add_epi64(sum64, _mm256_unpackhi_epi32(prev, _mm256_setzero_si256()));
    }

    __m128i s = _mm_add_epi64(_mm256_castsi256_si128(sum64), _mm256_extracti128_si256(sum64, 1));
    dist      = _mm_extract_epi64(s, 0) + _mm_extract_epi64(s, 1);

    return dist;
}

static INLINE void calc_centroids_2_avx2(const int* data, int* centroids, const uint8_t* indices, int n, int k) {
    int          i;
    int          count[PALETTE_MAX_SIZE] = {0};
    unsigned int rand_state              = (unsigned int)data[0];
    assert(n <= 32768);
    memset(centroids, 0, sizeof(centroids[0]) * k * 2);

    for (i = 0; i < n; ++i) {
        const int index = indices[i];
        assert(index < k);
        ++count[index];
        centroids[index * 2] += data[i * 2];
        centroids[index * 2 + 1] += data[i * 2 + 1];
    }

    for (i = 0; i < k; ++i) {
        if (count[i] == 0) {
            svt_memcpy_intrin_sse(
                centroids + i * 2, (void*)(data + (lcg_rand16(&rand_state) % n) * 2), sizeof(centroids[0]) * 2);
        } else {
            centroids[i * 2]     = DIVIDE_AND_ROUND(centroids[i * 2], count[i]);
            centroids[i * 2 + 1] = DIVIDE_AND_ROUND(centroids[i * 2 + 1], count[i]);
        }
    }
}

void svt_av1_k_means_dim2_avx2(const int* data, int* centroids, uint8_t* indices, int n, int k, int max_itr) {
    int     pre_centroids[2 * PALETTE_MAX_SIZE];
    uint8_t pre_indices[MAX_SB_SQUARE];

    assert((n & 15) == 0);

    int64_t this_dist = av1_calc_indices_dist_dim2_avx2(data, centroids, indices, n, k);

    for (int i = 0; i < max_itr; ++i) {
        const int64_t pre_dist = this_dist;
        svt_memcpy_intrin_sse(pre_centroids, centroids, sizeof(pre_centroids[0]) * k * 2);
        svt_memcpy_intrin_sse(pre_indices, indices, sizeof(pre_indices[0]) * n);

        calc_centroids_2_avx2(data, centroids, indices, n, k);
        this_dist = av1_calc_indices_dist_dim2_avx2(data, centroids, indices, n, k);

        if (this_dist > pre_dist) {
            svt_memcpy_intrin_sse(centroids, pre_centroids, sizeof(pre_centroids[0]) * k * 2);
            svt_memcpy_intrin_sse(indices, pre_indices, sizeof(pre_indices[0]) * n);
            break;
        }
        if (!memcmp(centroids, pre_centroids, sizeof(pre_centroids[0]) * k * 2)) {
            break;
        }
    }
}
