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

#include "aom_dsp_rtcd.h"
#include "corner_match.h"

#define SEARCH_SZ 9
#define SEARCH_SZ_BY2 ((SEARCH_SZ - 1) / 2)

#define THRESHOLD_NCC 0.75

/* Compute var(im) * MATCH_SZ_SQ over a MATCH_SZ by MATCH_SZ window of im,
   centered at (x, y).
*/
static int32_t compute_variance(unsigned char* im, int stride, int x, int y, uint8_t match_sz) {
    const uint8_t match_sz_by2 = ((match_sz - 1) / 2);
    const uint8_t match_sz_sq  = (match_sz * match_sz);
    int           sum          = 0;
    int           sumsq        = 0;
    int           var;
    int           i, j;
    for (i = 0; i < match_sz; ++i) {
        for (j = 0; j < match_sz; ++j) {
            sum += im[(i + y - match_sz_by2) * stride + (j + x - match_sz_by2)];
            sumsq += im[(i + y - match_sz_by2) * stride + (j + x - match_sz_by2)] *
                im[(i + y - match_sz_by2) * stride + (j + x - match_sz_by2)];
        }
    }
    var = sumsq * match_sz_sq - sum * sum;

    return var;
}

/* Compute quad of corr(im1, im2) * MATCH_SZ * stddev(im1), where the
   correlation/standard deviation are taken over MATCH_SZ by MATCH_SZ windows
   of each image, centered at (x1, y1) and (x2, y2) respectively.
*/
double svt_av1_compute_cross_correlation_c(unsigned char* im1, int stride1, int x1, int y1, unsigned char* im2,
                                           int stride2, int x2, int y2, uint8_t match_sz) {
    const uint8_t match_sz_by2 = ((match_sz - 1) / 2);
    const uint8_t match_sz_sq  = (match_sz * match_sz);

    int v1, v2;
    int sum1   = 0;
    int sum2   = 0;
    int sumsq2 = 0;
    int cross  = 0;
    int var2, cov;
    int i, j;
    for (i = 0; i < match_sz; ++i) {
        for (j = 0; j < match_sz; ++j) {
            v1 = im1[(i + y1 - match_sz_by2) * stride1 + (j + x1 - match_sz_by2)];
            v2 = im2[(i + y2 - match_sz_by2) * stride2 + (j + x2 - match_sz_by2)];
            sum1 += v1;
            sum2 += v2;
            sumsq2 += v2 * v2;
            cross += v1 * v2;
        }
    }
    var2 = sumsq2 * match_sz_sq - sum2 * sum2;
    cov  = cross * match_sz_sq - sum1 * sum2;
    if (cov < 0) {
        return 0;
    }
    return ((double)cov * cov) / ((double)var2);
}

static INLINE int is_eligible_point(int pointx, int pointy, int width, int height, uint8_t match_sz) {
    const uint8_t match_sz_by2 = ((match_sz - 1) / 2);

    return (pointx >= match_sz_by2 && pointy >= match_sz_by2 && pointx + match_sz_by2 < width &&
            pointy + match_sz_by2 < height);
}

static INLINE int is_eligible_distance(int point1x, int point1y, int point2x, int point2y, int threshSqr) {
    const int xdist = point1x - point2x;
    const int ydist = point1y - point2y;
    return (xdist * xdist + ydist * ydist) <= threshSqr;
}

static void improve_correspondence(unsigned char* frm, unsigned char* ref, int width, int height, int frm_stride,
                                   int ref_stride, Correspondence* correspondences, int num_correspondences,
                                   uint8_t match_sz) {
    int       i;
    const int thresh    = (width < height ? height : width) >> 4;
    const int threshSqr = thresh * thresh;
    for (i = 0; i < num_correspondences; ++i) {
        int    x, y, best_x = 0, best_y = 0;
        double best_match_ncc = 0.0;
        for (y = -SEARCH_SZ_BY2; y <= SEARCH_SZ_BY2; ++y) {
            for (x = -SEARCH_SZ_BY2; x <= SEARCH_SZ_BY2; ++x) {
                double match_ncc;
                if (!is_eligible_point(correspondences[i].rx + x, correspondences[i].ry + y, width, height, match_sz)) {
                    continue;
                }
                if (!is_eligible_distance(correspondences[i].x,
                                          correspondences[i].y,
                                          correspondences[i].rx + x,
                                          correspondences[i].ry + y,
                                          threshSqr)) {
                    continue;
                }
                match_ncc = svt_av1_compute_cross_correlation(frm,
                                                              frm_stride,
                                                              correspondences[i].x,
                                                              correspondences[i].y,
                                                              ref,
                                                              ref_stride,
                                                              correspondences[i].rx + x,
                                                              correspondences[i].ry + y,
                                                              match_sz);
                if (match_ncc > best_match_ncc) {
                    best_match_ncc = match_ncc;
                    best_y         = y;
                    best_x         = x;
                }
            }
        }
        correspondences[i].rx += best_x;
        correspondences[i].ry += best_y;
    }
    for (i = 0; i < num_correspondences; ++i) {
        int    x, y, best_x = 0, best_y = 0;
        double best_match_ncc = 0.0;
        for (y = -SEARCH_SZ_BY2; y <= SEARCH_SZ_BY2; ++y) {
            for (x = -SEARCH_SZ_BY2; x <= SEARCH_SZ_BY2; ++x) {
                double match_ncc;
                if (!is_eligible_point(correspondences[i].x + x, correspondences[i].y + y, width, height, match_sz)) {
                    continue;
                }
                if (!is_eligible_distance(correspondences[i].x + x,
                                          correspondences[i].y + y,
                                          correspondences[i].rx,
                                          correspondences[i].ry,
                                          threshSqr)) {
                    continue;
                }
                match_ncc = svt_av1_compute_cross_correlation(ref,
                                                              ref_stride,
                                                              correspondences[i].rx,
                                                              correspondences[i].ry,
                                                              frm,
                                                              frm_stride,
                                                              correspondences[i].x + x,
                                                              correspondences[i].y + y,
                                                              match_sz);
                if (match_ncc > best_match_ncc) {
                    best_match_ncc = match_ncc;
                    best_y         = y;
                    best_x         = x;
                }
            }
        }
        correspondences[i].x += best_x;
        correspondences[i].y += best_y;
    }
}

int svt_av1_determine_correspondence(uint8_t* frm, int* frm_corners, int num_frm_corners, uint8_t* ref,
                                     int* ref_corners, int num_ref_corners, int width, int height, int frm_stride,
                                     int ref_stride, Correspondence* correspondences, uint8_t match_sz) {
    int       i, j;
    int       num_correspondences = 0;
    const int thresh              = (width < height ? height : width) >> 4;
    const int threshSqr           = thresh * thresh;
    for (i = 0; i < num_frm_corners; ++i) {
        double  best_match_ncc = 0.0;
        int32_t template_norm;
        int     best_match_j = -1;
        if (!is_eligible_point(frm_corners[2 * i], frm_corners[2 * i + 1], width, height, match_sz)) {
            continue;
        }
        for (j = 0; j < num_ref_corners; ++j) {
            double match_ncc;
            if (!is_eligible_point(ref_corners[2 * j], ref_corners[2 * j + 1], width, height, match_sz)) {
                continue;
            }
            if (!is_eligible_distance(frm_corners[2 * i],
                                      frm_corners[2 * i + 1],
                                      ref_corners[2 * j],
                                      ref_corners[2 * j + 1],
                                      threshSqr)) {
                continue;
            }
            match_ncc = svt_av1_compute_cross_correlation(frm,
                                                          frm_stride,
                                                          frm_corners[2 * i],
                                                          frm_corners[2 * i + 1],
                                                          ref,
                                                          ref_stride,
                                                          ref_corners[2 * j],
                                                          ref_corners[2 * j + 1],
                                                          match_sz);
            if (match_ncc > best_match_ncc) {
                best_match_ncc = match_ncc;
                best_match_j   = j;
            }
        }
        // Note: We want to test if the best correlation is >= THRESHOLD_NCC,
        // but need to account for the normalization in
        // av1_compute_cross_correlation.
        template_norm = compute_variance(frm, frm_stride, frm_corners[2 * i], frm_corners[2 * i + 1], match_sz);

        if (best_match_ncc > (template_norm * THRESHOLD_NCC * THRESHOLD_NCC)) {
            correspondences[num_correspondences].x  = frm_corners[2 * i];
            correspondences[num_correspondences].y  = frm_corners[2 * i + 1];
            correspondences[num_correspondences].rx = ref_corners[2 * best_match_j];
            correspondences[num_correspondences].ry = ref_corners[2 * best_match_j + 1];

            /*SVT_LOG("corresp: %d %d - %d %d\n",
             correspondences[num_correspondences].x,
             correspondences[num_correspondences].y,
             correspondences[num_correspondences].rx,
             correspondences[num_correspondences].ry);*/

            num_correspondences++;
        }
    }
    improve_correspondence(
        frm, ref, width, height, frm_stride, ref_stride, correspondences, num_correspondences, match_sz);
    return num_correspondences;
}
