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

#include "cdef.h"
#include "common_dsp_rtcd.h"
#include "bitstream_unit.h"

static INLINE int32_t sign(int32_t i) {
    return i < 0 ? -1 : 1;
}

static INLINE int32_t constrain(int32_t diff, int32_t threshold, int32_t damping) {
    if (!threshold) {
        return 0;
    }

    const int32_t shift = AOMMAX(0, damping - get_msb(threshold));
    return sign(diff) * AOMMIN(abs(diff), AOMMAX(0, threshold - (abs(diff) >> shift)));
}

/*
This is Cdef_Directions (section 7.15.3) with 2 padding entries at the
beginning and end of the table. The cdef direction range is [0, 7] and the
first index is offset +/-2. This removes the need to constrain the first
index to the same range using e.g., & 7.
*/
DECLARE_ALIGNED(16, const int, eb_cdef_directions_padded[12][2]) = {
    /* Padding: svt_aom_eb_cdef_directions[6] */
    {1 * CDEF_BSTRIDE + 0, 2 * CDEF_BSTRIDE + 0},
    /* Padding: svt_aom_eb_cdef_directions[7] */
    {1 * CDEF_BSTRIDE + 0, 2 * CDEF_BSTRIDE - 1},

    /* Begin svt_aom_eb_cdef_directions */
    {-1 * CDEF_BSTRIDE + 1, -2 * CDEF_BSTRIDE + 2},
    {0 * CDEF_BSTRIDE + 1, -1 * CDEF_BSTRIDE + 2},
    {0 * CDEF_BSTRIDE + 1, 0 * CDEF_BSTRIDE + 2},
    {0 * CDEF_BSTRIDE + 1, 1 * CDEF_BSTRIDE + 2},
    {1 * CDEF_BSTRIDE + 1, 2 * CDEF_BSTRIDE + 2},
    {1 * CDEF_BSTRIDE + 0, 2 * CDEF_BSTRIDE + 1},
    {1 * CDEF_BSTRIDE + 0, 2 * CDEF_BSTRIDE + 0},
    {1 * CDEF_BSTRIDE + 0, 2 * CDEF_BSTRIDE - 1},
    /* End svt_aom_eb_cdef_directions */

    /* Padding: svt_aom_eb_cdef_directions[0] */
    {-1 * CDEF_BSTRIDE + 1, -2 * CDEF_BSTRIDE + 2},
    /* Padding: svt_aom_eb_cdef_directions[1] */
    {0 * CDEF_BSTRIDE + 1, -1 * CDEF_BSTRIDE + 2},
};

const int (*const svt_aom_eb_cdef_directions)[2] = eb_cdef_directions_padded + 2;

/* Compute the primary filter strength for an 8x8 block based on the
directional variance difference. A high variance difference means
that we have a highly directional pattern (e.g. a high contrast
edge), so we can apply more deringing. A low variance means that we
either have a low contrast edge, or a non-directional texture, so
we want to be careful not to blur. */
static INLINE int32_t adjust_strength(int32_t strength, int32_t var) {
    const int32_t i = (var >> 6) ? AOMMIN(get_msb(var >> 6), 12) : 0;
    /* We use the variance of 8x8 blocks to adjust the strength. */
    return var ? (strength * (4 + i) + 8) >> 4 : 0;
}

void svt_aom_copy_rect8_8bit_to_16bit_c(uint16_t* dst, int32_t dstride, const uint8_t* src, int32_t sstride, int32_t v,
                                        int32_t h) {
    for (int32_t i = 0; i < v; i++) {
        for (int32_t j = 0; j < h; j++) {
            dst[i * dstride + j] = src[i * sstride + j];
        }
    }
}

/* Detect direction. 0 means 45-degree up-right, 2 is horizontal, and so on.
The search minimizes the weighted variance along all the lines in a
particular direction, i.e. the squared error between the input and a
"predicted" block where each pixel is replaced by the average along a line
in a particular direction. Since each direction have the same sum(x^2) term,
that term is never computed. See Section 2, step 2, of:
http://jmvalin.ca/notes/intra_paint.pdf */
uint8_t svt_aom_cdef_find_dir_c(const uint16_t* img, int32_t stride, int32_t* var, int32_t coeff_shift) {
    int32_t cost[8]        = {0};
    int32_t partial[8][15] = {{0}};
    int32_t best_cost      = 0;
    uint8_t i;
    uint8_t best_dir = 0;
    /* Instead of dividing by n between 2 and 8, we multiply by 3*5*7*8/n.
    The output is then 840 times larger, but we don't care for finding
    the max. */
    static const int32_t div_table[] = {0, 840, 420, 280, 210, 168, 140, 120, 105};
    for (i = 0; i < 8; i++) {
        int32_t j;
        for (j = 0; j < 8; j++) {
            int32_t x;
            /* We subtract 128 here to reduce the maximum range of the squared
            partial sums. */
            x = (img[i * stride + j] >> coeff_shift) - 128;
            partial[0][i + j] += x;
            partial[1][i + j / 2] += x;
            partial[2][i] += x;
            partial[3][3 + i - j / 2] += x;
            partial[4][7 + i - j] += x;
            partial[5][3 - i / 2 + j] += x;
            partial[6][j] += x;
            partial[7][i / 2 + j] += x;
        }
    }
    for (i = 0; i < 8; i++) {
        cost[2] += partial[2][i] * partial[2][i];
        cost[6] += partial[6][i] * partial[6][i];
    }
    cost[2] *= div_table[8];
    cost[6] *= div_table[8];
    for (i = 0; i < 7; i++) {
        cost[0] += (partial[0][i] * partial[0][i] + partial[0][14 - i] * partial[0][14 - i]) * div_table[i + 1];
        cost[4] += (partial[4][i] * partial[4][i] + partial[4][14 - i] * partial[4][14 - i]) * div_table[i + 1];
    }
    cost[0] += partial[0][7] * partial[0][7] * div_table[8];
    cost[4] += partial[4][7] * partial[4][7] * div_table[8];
    for (i = 1; i < 8; i += 2) {
        int32_t j;
        for (j = 0; j < 4 + 1; j++) {
            cost[i] += partial[i][3 + j] * partial[i][3 + j];
        }
        cost[i] *= div_table[8];
        for (j = 0; j < 4 - 1; j++) {
            cost[i] += (partial[i][j] * partial[i][j] + partial[i][10 - j] * partial[i][10 - j]) * div_table[2 * j + 2];
        }
    }
    for (i = 0; i < 8; i++) {
        if (cost[i] > best_cost) {
            best_cost = cost[i];
            best_dir  = i;
        }
    }
    /* Difference between the optimal variance and the variance along the
    orthogonal direction. Again, the sum(x^2) terms cancel out. */
    *var = best_cost - cost[(best_dir + 4) & 7];
    /* We'd normally divide by 840, but dividing by 1024 is close enough
    for what we're going to do with this. */
    *var >>= 10;
    return best_dir;
}

void svt_aom_cdef_find_dir_dual_c(const uint16_t* img1, const uint16_t* img2, int stride, int32_t* var1, int32_t* var2,
                                  int32_t coeff_shift, uint8_t* out1, uint8_t* out2) {
    *out1 = svt_aom_cdef_find_dir_c(img1, stride, var1, coeff_shift);
    *out2 = svt_aom_cdef_find_dir_c(img2, stride, var2, coeff_shift);
}

static AOM_INLINE void cdef_find_dir(uint16_t* in, CdefList* dlist, int32_t var[CDEF_NBLOCKS][CDEF_NBLOCKS],
                                     int32_t cdef_count, int32_t coeff_shift, uint8_t dir[CDEF_NBLOCKS][CDEF_NBLOCKS]) {
    int bi;

    // Find direction of two 8x8 blocks together.
    for (bi = 0; bi < cdef_count - 1; bi += 2) {
        const uint8_t by   = dlist[bi].by;
        const uint8_t bx   = dlist[bi].bx;
        const uint8_t by2  = dlist[bi + 1].by;
        const uint8_t bx2  = dlist[bi + 1].bx;
        const int     pos1 = 8 * by * CDEF_BSTRIDE + 8 * bx;
        const int     pos2 = 8 * by2 * CDEF_BSTRIDE + 8 * bx2;
        svt_aom_cdef_find_dir_dual(&in[pos1],
                                   &in[pos2],
                                   CDEF_BSTRIDE,
                                   &var[by][bx],
                                   &var[by2][bx2],
                                   coeff_shift,
                                   &dir[by][bx],
                                   &dir[by2][bx2]);
    }

    // Process remaining 8x8 blocks here. One 8x8 at a time.
    if (cdef_count % 2) {
        const uint8_t by = dlist[bi].by;
        const uint8_t bx = dlist[bi].bx;
        dir[by][bx]      = svt_aom_cdef_find_dir(
            &in[8 * by * CDEF_BSTRIDE + 8 * bx], CDEF_BSTRIDE, &var[by][bx], coeff_shift);
    }
}

const int32_t svt_aom_eb_cdef_pri_taps[2][2] = {{4, 2}, {3, 3}};
const int32_t svt_aom_eb_cdef_sec_taps[2][2] = {{2, 1}, {2, 1}};

/* Smooth in the direction detected. */
void svt_cdef_filter_block_c(uint8_t* dst8, uint16_t* dst16, int32_t dstride, const uint16_t* in, int32_t pri_strength,
                             int32_t sec_strength, int32_t dir, int32_t pri_damping, int32_t sec_damping, int32_t bsize,
                             int32_t coeff_shift, uint8_t subsampling_factor) {
    int32_t        i, j, k;
    const int32_t  s        = CDEF_BSTRIDE;
    const int32_t* pri_taps = svt_aom_eb_cdef_pri_taps[(pri_strength >> coeff_shift) & 1];
    const int32_t* sec_taps = svt_aom_eb_cdef_sec_taps[(pri_strength >> coeff_shift) & 1];

    for (i = 0; i < (4 << (int32_t)(bsize == BLOCK_8X8 || bsize == BLOCK_4X8)); i += subsampling_factor) {
        for (j = 0; j < (4 << (int32_t)(bsize == BLOCK_8X8 || bsize == BLOCK_8X4)); j++) {
            int16_t sum = 0;
            int16_t y;
            int16_t x   = in[i * s + j];
            int32_t max = x;
            int32_t min = x;
            for (k = 0; k < 2; k++) {
                int16_t p0 = in[i * s + j + svt_aom_eb_cdef_directions[dir][k]];
                int16_t p1 = in[i * s + j - svt_aom_eb_cdef_directions[dir][k]];
                sum += (int16_t)(pri_taps[k] * constrain(p0 - x, pri_strength, pri_damping));
                sum += (int16_t)(pri_taps[k] * constrain(p1 - x, pri_strength, pri_damping));
                if (p0 != CDEF_VERY_LARGE) {
                    max = AOMMAX(p0, max);
                }
                if (p1 != CDEF_VERY_LARGE) {
                    max = AOMMAX(p1, max);
                }
                min        = AOMMIN(p0, min);
                min        = AOMMIN(p1, min);
                int16_t s0 = in[i * s + j + svt_aom_eb_cdef_directions[(dir + 2)][k]];
                int16_t s1 = in[i * s + j - svt_aom_eb_cdef_directions[(dir + 2)][k]];
                int16_t s2 = in[i * s + j + svt_aom_eb_cdef_directions[(dir - 2)][k]];
                int16_t s3 = in[i * s + j - svt_aom_eb_cdef_directions[(dir - 2)][k]];
                if (s0 != CDEF_VERY_LARGE) {
                    max = AOMMAX(s0, max);
                }
                if (s1 != CDEF_VERY_LARGE) {
                    max = AOMMAX(s1, max);
                }
                if (s2 != CDEF_VERY_LARGE) {
                    max = AOMMAX(s2, max);
                }
                if (s3 != CDEF_VERY_LARGE) {
                    max = AOMMAX(s3, max);
                }
                min = AOMMIN(s0, min);
                min = AOMMIN(s1, min);
                min = AOMMIN(s2, min);
                min = AOMMIN(s3, min);
                sum += (int16_t)(sec_taps[k] * constrain(s0 - x, sec_strength, sec_damping));
                sum += (int16_t)(sec_taps[k] * constrain(s1 - x, sec_strength, sec_damping));
                sum += (int16_t)(sec_taps[k] * constrain(s2 - x, sec_strength, sec_damping));
                sum += (int16_t)(sec_taps[k] * constrain(s3 - x, sec_strength, sec_damping));
            }
            y = (int16_t)clamp((int16_t)x + ((8 + sum - (sum < 0)) >> 4), min, max);
            if (dst8) {
                dst8[i * dstride + j] = (uint8_t)y;
            } else {
                dst16[i * dstride + j] = (uint16_t)y;
            }
        }
    }
}

void svt_aom_copy_sb8_16(uint16_t* dst, int32_t dstride, const uint8_t* src, int32_t src_voffset, int32_t src_hoffset,
                         int32_t sstride, int32_t vsize, int32_t hsize, bool is_16bit) {
    if (is_16bit) {
        const uint16_t* base = ((uint16_t*)src) + (src_voffset * sstride + src_hoffset);
        for (int r = 0; r < vsize; r++) {
            svt_memcpy(dst, base, 2 * hsize);
            dst += dstride;
            base += sstride;
        }
    } else {
        const uint8_t* base = &src[src_voffset * sstride + src_hoffset];
        svt_aom_copy_rect8_8bit_to_16bit(dst, dstride, base, sstride, vsize, hsize);
    }
}

/*
 * Loop over the non-skip 8x8 blocks.  For each block, find the CDEF direction, then apply the specified filter.
*/
void svt_cdef_filter_fb(uint8_t* dst8, uint16_t* dst16, int32_t dstride, uint16_t* in, int32_t xdec, int32_t ydec,
                        uint8_t dir[CDEF_NBLOCKS][CDEF_NBLOCKS], int32_t* dirinit,
                        int32_t var[CDEF_NBLOCKS][CDEF_NBLOCKS], int32_t pli, CdefList* dlist, int32_t cdef_count,
                        int32_t level, int32_t sec_strength, int32_t pri_damping, int32_t sec_damping,
                        int32_t coeff_shift, uint8_t subsampling_factor) {
    int32_t bi;
    int32_t pri_strength = level << coeff_shift;
    sec_strength <<= coeff_shift;
    sec_damping += coeff_shift - (pli != AOM_PLANE_Y);
    pri_damping += coeff_shift - (pli != AOM_PLANE_Y);

    int32_t bsize  = ydec ? (xdec ? BLOCK_4X4 : BLOCK_8X4) : (xdec ? BLOCK_4X8 : BLOCK_8X8);
    int32_t bsizex = 3 - xdec;
    int32_t bsizey = 3 - ydec;

    if (!dstride && pri_strength == 0 && sec_strength == 0) {
        // If we're here, both primary and secondary strengths are 0, and
        // we still haven't written anything to y[] yet, so we just copy
        // the input to y[]. This is necessary only for svt_av1_cdef_search()
        // and only svt_av1_cdef_search() sets dirinit.
        for (bi = 0; bi < cdef_count; bi++) {
            int32_t   by = dlist[bi].by << bsizey;
            int32_t   bx = dlist[bi].bx << bsizex;
            int32_t   iy;
            uint16_t* src_16 = in + (by * CDEF_BSTRIDE + bx);
            if (dst8) {
                uint8_t* dst_8 = dst8 + (bi << (bsizex + bsizey));
                //size 2x2 and 3x3, no gain to use SIMD
                for (iy = 0; iy < 1 << bsizey; iy += subsampling_factor) {
                    for (int32_t ix = 0; ix < 1 << bsizex; ix++) {
                        dst_8[(iy << bsizex) + ix] = (uint8_t)src_16[iy * CDEF_BSTRIDE + ix];
                    }
                }
            } else {
                uint16_t* dst_16 = dst16 + (bi << (bsizex + bsizey));
                for (iy = 0; iy < 1 << bsizey; iy += subsampling_factor) {
                    memcpy(dst_16 + (iy << bsizex),
                           src_16 + iy * CDEF_BSTRIDE,
                           (uint32_t)(1 << bsizex) * sizeof(uint16_t));
                }
            }
        }
        return;
    }

    if (pli == 0) {
        if (!dirinit || !*dirinit) {
            cdef_find_dir(in, dlist, var, cdef_count, coeff_shift, dir);
            if (dirinit) {
                *dirinit = 1;
            }
        }
    } else if (pli == 1 && xdec != ydec) {
        for (bi = 0; bi < cdef_count; bi++) {
            static const uint8_t conv422[8] = {7, 0, 2, 4, 5, 6, 6, 6};
            static const uint8_t conv440[8] = {1, 2, 2, 2, 3, 4, 6, 0};

            int32_t by  = dlist[bi].by;
            int32_t bx  = dlist[bi].bx;
            dir[by][bx] = (xdec ? conv422 : conv440)[dir[by][bx]];
        }
    }

    for (bi = 0; bi < cdef_count; bi++) {
        int32_t by = dlist[bi].by;
        int32_t bx = dlist[bi].bx;
        int32_t t  = pli ? pri_strength : adjust_strength(pri_strength, var[by][bx]);
        int32_t k  = dstride ? (by << bsizey) * dstride + (bx << bsizex) : bi << (bsizex + bsizey);
        svt_cdef_filter_block(dst8 ? &dst8[k] : NULL,
                              dst8 ? NULL : &dst16[k],
                              dstride ? dstride : 1 << bsizex,
                              &in[(by * CDEF_BSTRIDE << bsizey) + (bx << bsizex)],
                              t,
                              sec_strength,
                              pri_strength ? dir[by][bx] : 0,
                              pri_damping,
                              sec_damping,
                              bsize,
                              coeff_shift,
                              subsampling_factor);
    }
}
