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

#include <math.h>

#include "svt_psnr.h"
#include "pcs.h"
#include "aom_dsp_rtcd.h"
#include "entropy_coding.h"
#include "restoration.h"
#include "restoration_pick.h"

#include "rest_process.h"
#include "svt_log.h"

// When set to RESTORE_WIENER or RESTORE_SGRPROJ only those are allowed.
// When set to RESTORE_TYPES we allow switchable.
//static const RestorationType force_restore_type = RESTORE_TYPES;

// Number of Wiener iterations
#define NUM_WIENER_ITERS 5
// Working precision for Wiener filter coefficients
#define WIENER_TAP_SCALE_FACTOR ((int64_t)1 << 16)

typedef int64_t (*SsePartExtractorType)(const Yv12BufferConfig* a, const Yv12BufferConfig* b, int32_t hstart,
                                        int32_t width, int32_t vstart, int32_t height);

#define NUM_EXTRACTORS (3 * (1 + 1))

static const SsePartExtractorType sse_part_extractors[NUM_EXTRACTORS] = {
    svt_aom_get_y_sse_part,
    svt_aom_get_u_sse_part,
    svt_aom_get_v_sse_part,
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    svt_aom_highbd_get_y_sse_part,
    svt_aom_highbd_get_u_sse_part,
    svt_aom_highbd_get_v_sse_part,
#endif
};
static int64_t sse_restoration_unit(const RestorationTileLimits* limits, const Yv12BufferConfig* src,
                                    const Yv12BufferConfig* dst, int32_t plane, int32_t highbd) {
    return sse_part_extractors[3 * highbd + plane](
        src, dst, limits->h_start, limits->h_end - limits->h_start, limits->v_start, limits->v_end - limits->v_start);
}

typedef struct {
    const Yv12BufferConfig* src;
    Yv12BufferConfig*       dst;

    Av1Common*          cm;
    const Macroblock*   x;
    int32_t             plane;
    int32_t             plane_width;
    int32_t             plane_height;
    RestUnitSearchInfo* rusi;
    RestUnitSearchInfo* rusi_pic;
    uint32_t            pic_num;
    Yv12BufferConfig*   org_frame_to_show;
    int32_t*            tmpbuf;

    uint8_t*       dgd_buffer;
    int32_t        dgd_stride;
    const uint8_t* src_buffer;
    int32_t        src_stride;

    // sse and bits are initialised by reset_rsc in search_rest_type
    int64_t sse;
    int64_t bits;
    int32_t tile_y0, tile_stripe0;

    // sgrproj and wiener are initialised by rsc_on_tile when starting the first
    // tile in the frame.
    SgrprojInfo sgrproj;
    WienerInfo  wiener;
} RestSearchCtxt;

static void rsc_on_tile(int32_t tile_row, int32_t tile_col, void* priv) {
    (void)tile_col;

    RestSearchCtxt* rsc = (RestSearchCtxt*)priv;
    set_default_sgrproj(&rsc->sgrproj);
    set_default_wiener(&rsc->wiener);

    rsc->tile_stripe0 = (tile_row == 0) ? 0 : rsc->cm->child_pcs->rst_end_stripe[tile_row - 1];
}

static void reset_rsc(RestSearchCtxt* rsc) {
    rsc->sse  = 0;
    rsc->bits = 0;
}

static void init_rsc_seg(Yv12BufferConfig* org_fts, const Yv12BufferConfig* src, Av1Common* cm, const Macroblock* x,
                         int32_t plane, RestUnitSearchInfo* rusi, Yv12BufferConfig* dst, RestSearchCtxt* rsc) {
    rsc->src   = src;
    rsc->dst   = dst;
    rsc->cm    = cm;
    rsc->x     = x;
    rsc->plane = plane;
    rsc->rusi  = rusi;

    rsc->org_frame_to_show = org_fts;

    const Yv12BufferConfig* dgd   = org_fts;
    const int32_t           is_uv = plane != AOM_PLANE_Y;
    rsc->plane_width              = src->crop_widths[is_uv];
    rsc->plane_height             = src->crop_heights[is_uv];
    rsc->src_buffer               = src->buffers[plane];
    rsc->src_stride               = src->strides[is_uv];
    rsc->dgd_buffer               = dgd->buffers[plane];
    rsc->dgd_stride               = dgd->strides[is_uv];
    assert(src->crop_widths[is_uv] == dgd->crop_widths[is_uv]);
    assert(src->crop_heights[is_uv] == dgd->crop_heights[is_uv]);
}

static int64_t try_restoration_unit_seg(const RestSearchCtxt* rsc, const RestorationTileLimits* limits,
                                        const Av1PixelRect* tile_rect, const RestorationUnitInfo* rui) {
    const Av1Common* const cm    = rsc->cm;
    const int32_t          plane = rsc->plane;
    const int32_t          is_uv = plane > 0;
    const RestorationInfo* rsi   = &cm->child_pcs->rst_info[plane];
    RestorationLineBuffers rlbs;
    const int32_t          bit_depth = cm->bit_depth;
    const int32_t          highbd    = cm->use_highbitdepth;

    const Yv12BufferConfig* fts = rsc->org_frame_to_show;

    const int32_t optimized_lr = 0;

    // If boundaries are enabled for filtering, recon gets updated using setup/restore
    // processing_stripe_bounadaries.  Many threads doing so will result in race condition.
    // Only use boundaries during the filter search if a copy of recon is made for each
    // thread (controlled with scs->seq_header.use_boundaries_in_rest_search).
    svt_av1_loop_restoration_filter_unit(cm->use_boundaries_in_rest_search,
                                         limits,
                                         rui,
                                         &rsi->boundaries,
                                         &rlbs,
                                         tile_rect,
                                         rsc->tile_stripe0,
                                         is_uv && cm->subsampling_x,
                                         is_uv && cm->subsampling_y,
                                         highbd,
                                         bit_depth,
                                         fts->buffers[plane],
                                         fts->strides[is_uv],
                                         rsc->dst->buffers[plane],
                                         rsc->dst->strides[is_uv],
                                         rsc->tmpbuf,
                                         optimized_lr);
    return sse_restoration_unit(limits, rsc->src, rsc->dst, plane, highbd);
}

int64_t svt_av1_lowbd_pixel_proj_error_c(const uint8_t* src8, int32_t width, int32_t height, int32_t src_stride,
                                         const uint8_t* dat8, int32_t dat_stride, int32_t* flt0, int32_t flt0_stride,
                                         int32_t* flt1, int32_t flt1_stride, const int32_t xq[2],
                                         const SgrParamsType* params) {
    int32_t        i, j;
    const uint8_t* src = src8;
    const uint8_t* dat = dat8;
    int64_t        err = 0;
    if (params->r[0] > 0 && params->r[1] > 0) {
        for (i = 0; i < height; ++i) {
            for (j = 0; j < width; ++j) {
                assert(flt1[j] < (1 << 15) && flt1[j] > -(1 << 15));
                assert(flt0[j] < (1 << 15) && flt0[j] > -(1 << 15));
                const int32_t u = (int32_t)(dat[j] << SGRPROJ_RST_BITS);
                int32_t       v = u << SGRPROJ_PRJ_BITS;
                v += xq[0] * (flt0[j] - u) + xq[1] * (flt1[j] - u);
                const int32_t e = ROUND_POWER_OF_TWO(v, SGRPROJ_RST_BITS + SGRPROJ_PRJ_BITS) - src[j];
                err += e * e;
            }
            dat += dat_stride;
            src += src_stride;
            flt0 += flt0_stride;
            flt1 += flt1_stride;
        }
    } else if (params->r[0] > 0) {
        for (i = 0; i < height; ++i) {
            for (j = 0; j < width; ++j) {
                assert(flt0[j] < (1 << 15) && flt0[j] > -(1 << 15));
                const int32_t u = (int32_t)(dat[j] << SGRPROJ_RST_BITS);
                int32_t       v = u << SGRPROJ_PRJ_BITS;
                v += xq[0] * (flt0[j] - u);
                const int32_t e = ROUND_POWER_OF_TWO(v, SGRPROJ_RST_BITS + SGRPROJ_PRJ_BITS) - src[j];
                err += e * e;
            }
            dat += dat_stride;
            src += src_stride;
            flt0 += flt0_stride;
        }
    } else if (params->r[1] > 0) {
        for (i = 0; i < height; ++i) {
            for (j = 0; j < width; ++j) {
                assert(flt1[j] < (1 << 15) && flt1[j] > -(1 << 15));
                const int32_t u = (int32_t)(dat[j] << SGRPROJ_RST_BITS);
                int32_t       v = u << SGRPROJ_PRJ_BITS;
                v += xq[1] * (flt1[j] - u);
                const int32_t e = ROUND_POWER_OF_TWO(v, SGRPROJ_RST_BITS + SGRPROJ_PRJ_BITS) - src[j];
                err += e * e;
            }
            dat += dat_stride;
            src += src_stride;
            flt1 += flt1_stride;
        }
    } else {
        for (i = 0; i < height; ++i) {
            for (j = 0; j < width; ++j) {
                const int32_t e = (int32_t)(dat[j]) - src[j];
                err += e * e;
            }
            dat += dat_stride;
            src += src_stride;
        }
    }

    return err;
}

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
int64_t svt_av1_highbd_pixel_proj_error_c(const uint8_t* src8, int32_t width, int32_t height, int32_t src_stride,
                                          const uint8_t* dat8, int32_t dat_stride, int32_t* flt0, int32_t flt0_stride,
                                          int32_t* flt1, int32_t flt1_stride, const int32_t xq[2],
                                          const SgrParamsType* params) {
    const uint16_t* src = CONVERT_TO_SHORTPTR(src8);
    const uint16_t* dat = CONVERT_TO_SHORTPTR(dat8);
    int32_t         i, j;
    int64_t         err  = 0;
    const int32_t   half = 1 << (SGRPROJ_RST_BITS + SGRPROJ_PRJ_BITS - 1);
    if (params->r[0] > 0 && params->r[1] > 0) {
        int32_t xq0 = xq[0];
        int32_t xq1 = xq[1];
        for (i = 0; i < height; ++i) {
            for (j = 0; j < width; ++j) {
                const int32_t d  = dat[j];
                const int32_t s  = src[j];
                const int32_t u  = (int32_t)(d << SGRPROJ_RST_BITS);
                int32_t       v0 = flt0[j] - u;
                int32_t       v1 = flt1[j] - u;
                int32_t       v  = half;
                v += xq0 * v0;
                v += xq1 * v1;
                const int32_t e = (v >> (SGRPROJ_RST_BITS + SGRPROJ_PRJ_BITS)) + d - s;
                err += e * e;
            }
            dat += dat_stride;
            flt0 += flt0_stride;
            flt1 += flt1_stride;
            src += src_stride;
        }
    } else if (params->r[0] > 0 || params->r[1] > 0) {
        int32_t  exq;
        int32_t* flt;
        int32_t  flt_stride;
        if (params->r[0] > 0) {
            exq        = xq[0];
            flt        = flt0;
            flt_stride = flt0_stride;
        } else {
            exq        = xq[1];
            flt        = flt1;
            flt_stride = flt1_stride;
        }
        for (i = 0; i < height; ++i) {
            for (j = 0; j < width; ++j) {
                const int32_t d = dat[j];
                const int32_t s = src[j];
                const int32_t u = (int32_t)(d << SGRPROJ_RST_BITS);
                int32_t       v = half;
                v += exq * (flt[j] - u);
                const int32_t e = (v >> (SGRPROJ_RST_BITS + SGRPROJ_PRJ_BITS)) + d - s;
                err += e * e;
            }
            dat += dat_stride;
            flt += flt_stride;
            src += src_stride;
        }
    } else {
        for (i = 0; i < height; ++i) {
            for (j = 0; j < width; ++j) {
                const int32_t d = dat[j];
                const int32_t s = src[j];
                const int32_t e = d - s;
                err += e * e;
            }
            dat += dat_stride;
            src += src_stride;
        }
    }
    return err;
}
#endif

static int64_t get_pixel_proj_error(const uint8_t* src8, int32_t width, int32_t height, int32_t src_stride,
                                    const uint8_t* dat8, int32_t dat_stride, int32_t use_highbitdepth, int32_t* flt0,
                                    int32_t flt0_stride, int32_t* flt1, int32_t flt1_stride, int32_t* xqd,
                                    const SgrParamsType* params) {
    int32_t xq[2];
    svt_decode_xq(xqd, xq, params);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    if (!use_highbitdepth)
#else
    UNUSED(use_highbitdepth);
#endif
    {
        return svt_av1_lowbd_pixel_proj_error(
            src8, width, height, src_stride, dat8, dat_stride, flt0, flt0_stride, flt1, flt1_stride, xq, params);
    }
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    else {
        return svt_av1_highbd_pixel_proj_error(
            src8, width, height, src_stride, dat8, dat_stride, flt0, flt0_stride, flt1, flt1_stride, xq, params);
    }
#endif
}

static int64_t finer_search_pixel_proj_error(const uint8_t* src8, int32_t width, int32_t height, int32_t src_stride,
                                             const uint8_t* dat8, int32_t dat_stride, int32_t use_highbitdepth,
                                             int32_t* flt0, int32_t flt0_stride, int32_t* flt1, int32_t flt1_stride,
                                             int32_t start_step, int32_t* xqd, int8_t do_refine,
                                             const SgrParamsType* params) {
    int64_t err = get_pixel_proj_error(src8,
                                       width,
                                       height,
                                       src_stride,
                                       dat8,
                                       dat_stride,
                                       use_highbitdepth,
                                       flt0,
                                       flt0_stride,
                                       flt1,
                                       flt1_stride,
                                       xqd,
                                       params);
    if (!do_refine) {
        return err;
    }

    (void)start_step;
    int64_t err2;
    int32_t tap_min[] = {SGRPROJ_PRJ_MIN0, SGRPROJ_PRJ_MIN1};
    int32_t tap_max[] = {SGRPROJ_PRJ_MAX0, SGRPROJ_PRJ_MAX1};
    for (int32_t s = start_step; s >= 1; s >>= 1) {
        for (int32_t p = 0; p < 2; ++p) {
            if ((params->r[0] == 0 && p == 0) || (params->r[1] == 0 && p == 1)) {
                continue;
            }
            int32_t skip = 0;
            do {
                if (xqd[p] - s >= tap_min[p]) {
                    xqd[p] -= s;
                    err2 = get_pixel_proj_error(src8,
                                                width,
                                                height,
                                                src_stride,
                                                dat8,
                                                dat_stride,
                                                use_highbitdepth,
                                                flt0,
                                                flt0_stride,
                                                flt1,
                                                flt1_stride,
                                                xqd,
                                                params);
                    if (err2 > err) {
                        xqd[p] += s;
                    } else {
                        err  = err2;
                        skip = 1;
                        // At the highest step size continue moving in the same direction
                        if (s == start_step) {
                            continue;
                        }
                    }
                }
                break;
            } while (1);
            if (skip) {
                break;
            }
            do {
                if (xqd[p] + s <= tap_max[p]) {
                    xqd[p] += s;
                    err2 = get_pixel_proj_error(src8,
                                                width,
                                                height,
                                                src_stride,
                                                dat8,
                                                dat_stride,
                                                use_highbitdepth,
                                                flt0,
                                                flt0_stride,
                                                flt1,
                                                flt1_stride,
                                                xqd,
                                                params);
                    if (err2 > err) {
                        xqd[p] -= s;
                    } else {
                        err = err2;
                        // At the highest step size continue moving in the same direction
                        if (s == start_step) {
                            continue;
                        }
                    }
                }
                break;
            } while (1);
        }
    }

    return err;
}

void svt_get_proj_subspace_c(const uint8_t* src8, int32_t width, int32_t height, int32_t src_stride,
                             const uint8_t* dat8, int32_t dat_stride, int32_t use_highbitdepth, int32_t* flt0,
                             int32_t flt0_stride, int32_t* flt1, int32_t flt1_stride, int32_t* xq,
                             const SgrParamsType* params) {
    int32_t       i, j;
    double        H[2][2] = {{0, 0}, {0, 0}};
    double        C[2]    = {0, 0};
    double        det;
    double        x[2];
    const int32_t size = width * height;

    // Default
    xq[0] = 0;
    xq[1] = 0;
    if (!use_highbitdepth) {
        const uint8_t* src = src8;
        const uint8_t* dat = dat8;
        for (i = 0; i < height; ++i) {
            for (j = 0; j < width; ++j) {
                const double u  = (double)(dat[i * dat_stride + j] << SGRPROJ_RST_BITS);
                const double s  = (double)(src[i * src_stride + j] << SGRPROJ_RST_BITS) - u;
                const double f1 = (params->r[0] > 0) ? (double)flt0[i * flt0_stride + j] - u : 0;
                const double f2 = (params->r[1] > 0) ? (double)flt1[i * flt1_stride + j] - u : 0;
                H[0][0] += f1 * f1;
                H[1][1] += f2 * f2;
                H[0][1] += f1 * f2;
                C[0] += f1 * s;
                C[1] += f2 * s;
            }
        }
    } else {
        const uint16_t* src = CONVERT_TO_SHORTPTR(src8);
        const uint16_t* dat = CONVERT_TO_SHORTPTR(dat8);
        for (i = 0; i < height; ++i) {
            for (j = 0; j < width; ++j) {
                const double u  = (double)(dat[i * dat_stride + j] << SGRPROJ_RST_BITS);
                const double s  = (double)(src[i * src_stride + j] << SGRPROJ_RST_BITS) - u;
                const double f1 = (params->r[0] > 0) ? (double)flt0[i * flt0_stride + j] - u : 0;
                const double f2 = (params->r[1] > 0) ? (double)flt1[i * flt1_stride + j] - u : 0;
                H[0][0] += f1 * f1;
                H[1][1] += f2 * f2;
                H[0][1] += f1 * f2;
                C[0] += f1 * s;
                C[1] += f2 * s;
            }
        }
    }
    H[0][0] /= size;
    H[0][1] /= size;
    H[1][1] /= size;
    H[1][0] = H[0][1];
    C[0] /= size;
    C[1] /= size;
    if (params->r[0] == 0) {
        // H matrix is now only the scalar H[1][1]
        // C vector is now only the scalar C[1]
        det = H[1][1];
        if (det < 1e-8) {
            return; // ill-posed, return default values
        }
        x[0] = 0;
        x[1] = C[1] / det;

        xq[0] = 0;
        xq[1] = (int32_t)rint(x[1] * (1 << SGRPROJ_PRJ_BITS));
    } else if (params->r[1] == 0) {
        // H matrix is now only the scalar H[0][0]
        // C vector is now only the scalar C[0]
        det = H[0][0];
        if (det < 1e-8) {
            return; // ill-posed, return default values
        }
        x[0] = C[0] / det;
        x[1] = 0;

        xq[0] = (int32_t)rint(x[0] * (1 << SGRPROJ_PRJ_BITS));
        xq[1] = 0;
    } else {
        det = (H[0][0] * H[1][1] - H[0][1] * H[1][0]);
        if (det < 1e-8) {
            return; // ill-posed, return default values
        }
        x[0] = (H[1][1] * C[0] - H[0][1] * C[1]) / det;
        x[1] = (H[0][0] * C[1] - H[1][0] * C[0]) / det;

        xq[0] = (int32_t)rint(x[0] * (1 << SGRPROJ_PRJ_BITS));
        xq[1] = (int32_t)rint(x[1] * (1 << SGRPROJ_PRJ_BITS));
    }
}

static INLINE void encode_xq(int32_t* xq, int32_t* xqd, const SgrParamsType* params) {
    if (params->r[0] == 0) {
        xqd[0] = 0;
        xqd[1] = clamp((1 << SGRPROJ_PRJ_BITS) - xq[1], SGRPROJ_PRJ_MIN1, SGRPROJ_PRJ_MAX1);
    } else if (params->r[1] == 0) {
        xqd[0] = clamp(xq[0], SGRPROJ_PRJ_MIN0, SGRPROJ_PRJ_MAX0);
        xqd[1] = clamp((1 << SGRPROJ_PRJ_BITS) - xqd[0], SGRPROJ_PRJ_MIN1, SGRPROJ_PRJ_MAX1);
    } else {
        xqd[0] = clamp(xq[0], SGRPROJ_PRJ_MIN0, SGRPROJ_PRJ_MAX0);
        xqd[1] = clamp((1 << SGRPROJ_PRJ_BITS) - xqd[0] - xq[1], SGRPROJ_PRJ_MIN1, SGRPROJ_PRJ_MAX1);
    }
}

// Apply the self-guided filter across an entire restoration unit.
static INLINE void apply_sgr(int32_t sgr_params_idx, const uint8_t* dat8, int32_t width, int32_t height,
                             int32_t dat_stride, int32_t use_highbd, int32_t bit_depth, int32_t pu_width,
                             int32_t pu_height, int32_t* flt0, int32_t* flt1, int32_t flt_stride) {
    for (int32_t i = 0; i < height; i += pu_height) {
        const int32_t  h        = AOMMIN(pu_height, height - i);
        int32_t*       flt0_row = flt0 + i * flt_stride;
        int32_t*       flt1_row = flt1 + i * flt_stride;
        const uint8_t* dat8_row = dat8 + i * dat_stride;

        // Iterate over the stripe in blocks of width pu_width
        for (int32_t j = 0; j < width; j += pu_width) {
            const int32_t w = AOMMIN(pu_width, width - j);

            //CHKN SSE
            svt_av1_selfguided_restoration(dat8_row + j,
                                           w,
                                           h,
                                           dat_stride,
                                           flt0_row + j,
                                           flt1_row + j,
                                           flt_stride,
                                           sgr_params_idx,
                                           bit_depth,
                                           use_highbd);
        }
    }
}

static SgrprojInfo search_selfguided_restoration(const uint8_t* dat8, int32_t width, int32_t height, int32_t dat_stride,
                                                 const uint8_t* src8, int32_t src_stride, int32_t use_highbitdepth,
                                                 int32_t bit_depth, int32_t pu_width, int32_t pu_height,
                                                 int32_t* rstbuf, SgFilterCtrls* ctrls, int32_t plane) {
    int32_t* flt0 = rstbuf;
    int32_t* flt1 = flt0 + RESTORATION_UNITPELS_MAX;
    int32_t  ep, bestep = 0;
    int64_t  besterr = -1;
    int32_t  exqd[2], bestxqd[2] = {0, 0};
    int32_t  flt_stride = ((width + 7) & ~7) + 8;
    assert(pu_width == (RESTORATION_PROC_UNIT_SIZE >> 1) || pu_width == RESTORATION_PROC_UNIT_SIZE);
    assert(pu_height == (RESTORATION_PROC_UNIT_SIZE >> 1) || pu_height == RESTORATION_PROC_UNIT_SIZE);
    plane            = !!plane; // plane > 0 ? 1 : 0;
    int8_t start_ep  = ctrls->start_ep[plane];
    int8_t end_ep    = ctrls->end_ep[plane];
    int8_t ep_inc    = ctrls->ep_inc[plane];
    int8_t do_refine = ctrls->refine[plane];
    for (ep = start_ep; ep < end_ep; ep += ep_inc) {
        int32_t exq[2];
        apply_sgr(ep,
                  dat8,
                  width,
                  height,
                  dat_stride,
                  use_highbitdepth,
                  bit_depth,
                  pu_width,
                  pu_height,
                  flt0,
                  flt1,
                  flt_stride);

        const SgrParamsType* const params = &svt_aom_eb_sgr_params[ep];
        svt_get_proj_subspace(src8,
                              width,
                              height,
                              src_stride,
                              dat8,
                              dat_stride,
                              use_highbitdepth,
                              flt0,
                              flt_stride,
                              flt1,
                              flt_stride,
                              exq,
                              params);

        encode_xq(exq, exqd, params);
        int64_t err = finer_search_pixel_proj_error(src8,
                                                    width,
                                                    height,
                                                    src_stride,
                                                    dat8,
                                                    dat_stride,
                                                    use_highbitdepth,
                                                    flt0,
                                                    flt_stride,
                                                    flt1,
                                                    flt_stride,
                                                    2,
                                                    exqd,
                                                    do_refine,
                                                    params);
        if (besterr == -1 || err < besterr) {
            bestep     = ep;
            besterr    = err;
            bestxqd[0] = exqd[0];
            bestxqd[1] = exqd[1];
        }
    }

    SgrprojInfo ret;
    ret.ep     = bestep;
    ret.xqd[0] = bestxqd[0];
    ret.xqd[1] = bestxqd[1];
    return ret;
}

int32_t svt_aom_count_primitive_refsubexpfin(uint16_t n, uint16_t k, uint16_t ref, uint16_t v);

static int32_t count_sgrproj_bits(SgrprojInfo* sgrproj_info, SgrprojInfo* ref_sgrproj_info) {
    int32_t              bits   = SGRPROJ_PARAMS_BITS;
    const SgrParamsType* params = &svt_aom_eb_sgr_params[sgrproj_info->ep];
    if (params->r[0] > 0) {
        bits += svt_aom_count_primitive_refsubexpfin(SGRPROJ_PRJ_MAX0 - SGRPROJ_PRJ_MIN0 + 1,
                                                     SGRPROJ_PRJ_SUBEXP_K,
                                                     (uint16_t)(ref_sgrproj_info->xqd[0] - SGRPROJ_PRJ_MIN0),
                                                     (uint16_t)(sgrproj_info->xqd[0] - SGRPROJ_PRJ_MIN0));
    }
    if (params->r[1] > 0) {
        bits += svt_aom_count_primitive_refsubexpfin(SGRPROJ_PRJ_MAX1 - SGRPROJ_PRJ_MIN1 + 1,
                                                     SGRPROJ_PRJ_SUBEXP_K,
                                                     (uint16_t)(ref_sgrproj_info->xqd[1] - SGRPROJ_PRJ_MIN1),
                                                     (uint16_t)(sgrproj_info->xqd[1] - SGRPROJ_PRJ_MIN1));
    }
    return bits;
}

void svt_av1_compute_stats_c(int32_t wiener_win, const uint8_t* dgd, const uint8_t* src, int32_t h_start, int32_t h_end,
                             int32_t v_start, int32_t v_end, int32_t dgd_stride, int32_t src_stride, int64_t* M,
                             int64_t* H) {
    int32_t       i, j, k, l;
    int16_t       y[WIENER_WIN2] = {0};
    const int32_t wiener_win2    = wiener_win * wiener_win;
    const int32_t wiener_halfwin = (wiener_win >> 1);
    uint8_t       avg            = find_average(dgd, h_start, h_end, v_start, v_end, dgd_stride);

    memset(M, 0, sizeof(*M) * wiener_win2);
    memset(H, 0, sizeof(*H) * wiener_win2 * wiener_win2);
    for (i = v_start; i < v_end; i++) {
        for (j = h_start; j < h_end; j++) {
            const int16_t x   = (int16_t)src[i * src_stride + j] - (int16_t)avg;
            int32_t       idx = 0;
            for (k = -wiener_halfwin; k <= wiener_halfwin; k++) {
                for (l = -wiener_halfwin; l <= wiener_halfwin; l++) {
                    y[idx] = (int16_t)dgd[(i + l) * dgd_stride + (j + k)] - (int16_t)avg;
                    idx++;
                }
            }
            assert(idx == wiener_win2);
            for (k = 0; k < wiener_win2; ++k) {
                M[k] += (int32_t)y[k] * x;
                for (l = k; l < wiener_win2; ++l) {
                    // H is a symmetric matrix, so we only need to fill out the upper
                    // triangle here. We can copy it down to the lower triangle outside
                    // the (i, j) loops.
                    H[k * wiener_win2 + l] += (int32_t)y[k] * y[l];
                }
            }
        }
    }
    for (k = 0; k < wiener_win2; ++k) {
        for (l = k + 1; l < wiener_win2; ++l) {
            H[l * wiener_win2 + k] = H[k * wiener_win2 + l];
        }
    }
}
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_av1_compute_stats_highbd_c(int32_t wiener_win, const uint8_t* dgd8, const uint8_t* src8, int32_t h_start,
                                    int32_t h_end, int32_t v_start, int32_t v_end, int32_t dgd_stride,
                                    int32_t src_stride, int64_t* M, int64_t* H, EbBitDepth bit_depth) {
    int32_t         i, j, k, l;
    int32_t         y[WIENER_WIN2] = {0};
    const int32_t   wiener_win2    = wiener_win * wiener_win;
    const int32_t   wiener_halfwin = (wiener_win >> 1);
    const uint16_t* src            = CONVERT_TO_SHORTPTR(src8);
    const uint16_t* dgd            = CONVERT_TO_SHORTPTR(dgd8);
    uint16_t        avg            = find_average_highbd(dgd, h_start, h_end, v_start, v_end, dgd_stride);

    uint8_t bit_depth_divider = 1;
    if (bit_depth == EB_TWELVE_BIT) {
        bit_depth_divider = 16;
    } else if (bit_depth == EB_TEN_BIT) {
        bit_depth_divider = 4;
    }

    memset(M, 0, sizeof(*M) * wiener_win2);
    memset(H, 0, sizeof(*H) * wiener_win2 * wiener_win2);
    for (i = v_start; i < v_end; i++) {
        for (j = h_start; j < h_end; j++) {
            const int32_t x   = (int32_t)src[i * src_stride + j] - (int32_t)avg;
            int32_t       idx = 0;
            for (k = -wiener_halfwin; k <= wiener_halfwin; k++) {
                for (l = -wiener_halfwin; l <= wiener_halfwin; l++) {
                    y[idx] = (int32_t)dgd[(i + l) * dgd_stride + (j + k)] - (int32_t)avg;
                    idx++;
                }
            }
            assert(idx == wiener_win2);
            for (k = 0; k < wiener_win2; ++k) {
                M[k] += (int64_t)y[k] * x;
                for (l = k; l < wiener_win2; ++l) {
                    // H is a symmetric matrix, so we only need to fill out the upper
                    // triangle here. We can copy it down to the lower triangle outside
                    // the (i, j) loops.
                    H[k * wiener_win2 + l] += (int64_t)y[k] * y[l];
                }
            }
        }
    }
    for (k = 0; k < wiener_win2; ++k) {
        M[k] /= bit_depth_divider;
        H[k * wiener_win2 + k] /= bit_depth_divider;
        for (l = k + 1; l < wiener_win2; ++l) {
            H[k * wiener_win2 + l] /= bit_depth_divider;
            H[l * wiener_win2 + k] = H[k * wiener_win2 + l];
        }
    }
}
#endif

static INLINE int32_t wrap_index(int32_t i, int32_t wiener_win) {
    const int32_t wiener_halfwin1 = (wiener_win >> 1) + 1;
    return (i >= wiener_halfwin1 ? wiener_win - 1 - i : i);
}

// Solve linear equations to find Wiener filter tap values
// Taps are output scaled by WIENER_FILT_STEP
static int32_t linsolve_wiener(int32_t n, int64_t* A, int32_t stride, int64_t* b, int32_t* x) {
    for (int32_t k = 0; k < n - 1; k++) {
        // Partial pivoting: bring the row with the largest pivot to the top
        for (int32_t i = n - 1; i > k; i--) {
            // If row i has a better (bigger) pivot than row (i-1), swap them
            if (llabs(A[(i - 1) * stride + k]) < llabs(A[i * stride + k])) {
                for (int32_t j = 0; j < n; j++) {
                    const int64_t c         = A[i * stride + j];
                    A[i * stride + j]       = A[(i - 1) * stride + j];
                    A[(i - 1) * stride + j] = c;
                }
                const int64_t c = b[i];
                b[i]            = b[i - 1];
                b[i - 1]        = c;
            }
        }
        // Forward elimination (convert A to row-echelon form)
        for (int32_t i = k; i < n - 1; i++) {
            if (A[k * stride + k] == 0) {
                return 0;
            }
            const int64_t c  = A[(i + 1) * stride + k];
            const int64_t cd = A[k * stride + k];
            for (int32_t j = 0; j < n; j++) {
                A[(i + 1) * stride + j] -= c / 256 * A[k * stride + j] / cd * 256;
            }
            b[i + 1] -= c * b[k] / cd;
        }
    }
    // Back-substitution
    for (int32_t i = n - 1; i >= 0; i--) {
        if (A[i * stride + i] == 0) {
            return 0;
        }
        int64_t c = 0;
        for (int32_t j = i + 1; j <= n - 1; j++) {
            c += A[i * stride + j] * x[j] / WIENER_TAP_SCALE_FACTOR;
        }
        // Store filter taps x in scaled form.
        x[i] = (int32_t)(WIENER_TAP_SCALE_FACTOR * (b[i] - c) / A[i * stride + i]);
    }

    return 1;
}

// Fix vector b, update vector a
static void update_a_sep_sym(int32_t wiener_win, int64_t** mc, int64_t** hc, int32_t* a, int32_t* b) {
    int32_t       i, j;
    int32_t       S[WIENER_WIN];
    int64_t       A[WIENER_HALFWIN1], B[WIENER_HALFWIN1 * WIENER_HALFWIN1];
    const int32_t wiener_win2     = wiener_win * wiener_win;
    const int32_t wiener_halfwin1 = (wiener_win >> 1) + 1;
    memset(A, 0, sizeof(A));
    memset(B, 0, sizeof(B));
    for (i = 0; i < wiener_win; i++) {
        for (j = 0; j < wiener_win; ++j) {
            const int32_t jj = wrap_index(j, wiener_win);
            A[jj] += mc[i][j] * b[i] / WIENER_TAP_SCALE_FACTOR;
        }
    }
    for (i = 0; i < wiener_win; i++) {
        for (j = 0; j < wiener_win; j++) {
            int32_t k, l;
            for (k = 0; k < wiener_win; ++k) {
                for (l = 0; l < wiener_win; ++l) {
                    const int32_t kk = wrap_index(k, wiener_win);
                    const int32_t ll = wrap_index(l, wiener_win);
                    B[ll * wiener_halfwin1 + kk] += hc[j * wiener_win + i][k * wiener_win2 + l] * b[i] /
                        WIENER_TAP_SCALE_FACTOR * b[j] / WIENER_TAP_SCALE_FACTOR;
                }
            }
        }
    }
    // Normalization enforcement in the system of equations itself
    assert(wiener_halfwin1 <= WIENER_HALFWIN1);
    int64_t a_halfwin_1 = A[wiener_halfwin1 - 1];
    for (i = 0; i < wiener_halfwin1 - 1; ++i) {
        A[i] -= a_halfwin_1 * 2 + B[i * wiener_halfwin1 + wiener_halfwin1 - 1] -
            2 * B[(wiener_halfwin1 - 1) * wiener_halfwin1 + (wiener_halfwin1 - 1)];
    }
    for (i = 0; i < wiener_halfwin1 - 1; ++i) {
        for (j = 0; j < wiener_halfwin1 - 1; ++j) {
            B[i * wiener_halfwin1 + j] -= 2 *
                (B[i * wiener_halfwin1 + (wiener_halfwin1 - 1)] + B[(wiener_halfwin1 - 1) * wiener_halfwin1 + j] -
                 2 * B[(wiener_halfwin1 - 1) * wiener_halfwin1 + (wiener_halfwin1 - 1)]);
        }
    }
    if (linsolve_wiener(wiener_halfwin1 - 1, B, wiener_halfwin1, A, S)) {
        S[wiener_halfwin1 - 1] = WIENER_TAP_SCALE_FACTOR;
        for (i = wiener_halfwin1; i < wiener_win; ++i) {
            S[i] = S[wiener_win - 1 - i];
            S[wiener_halfwin1 - 1] -= 2 * S[i];
        }
        svt_memcpy(a, S, wiener_win * sizeof(*a));
    }
}

// Fix vector a, update vector b
static void update_b_sep_sym(int32_t wiener_win, int64_t** Mc, int64_t** hc, int32_t* a, int32_t* b) {
    int32_t       i, j;
    int32_t       S[WIENER_WIN];
    int64_t       A[WIENER_HALFWIN1], B[WIENER_HALFWIN1 * WIENER_HALFWIN1];
    const int32_t wiener_win2     = wiener_win * wiener_win;
    const int32_t wiener_halfwin1 = (wiener_win >> 1) + 1;
    memset(A, 0, sizeof(A));
    memset(B, 0, sizeof(B));
    for (i = 0; i < wiener_win; i++) {
        const int32_t ii = wrap_index(i, wiener_win);
        for (j = 0; j < wiener_win; j++) {
            A[ii] += Mc[i][j] * a[j] / WIENER_TAP_SCALE_FACTOR;
        }
    }

    for (i = 0; i < wiener_win; i++) {
        for (j = 0; j < wiener_win; j++) {
            const int32_t ii = wrap_index(i, wiener_win);
            const int32_t jj = wrap_index(j, wiener_win);
            int32_t       k, l;
            for (k = 0; k < wiener_win; ++k) {
                for (l = 0; l < wiener_win; ++l) {
                    B[jj * wiener_halfwin1 + ii] += hc[i * wiener_win + j][k * wiener_win2 + l] * a[k] /
                        WIENER_TAP_SCALE_FACTOR * a[l] / WIENER_TAP_SCALE_FACTOR;
                }
            }
        }
    }
    // Normalization enforcement in the system of equations itself
    int64_t a_halfwin_1 = A[wiener_halfwin1 - 1];
    for (i = 0; i < wiener_halfwin1 - 1; ++i) {
        A[i] -= a_halfwin_1 * 2 + B[i * wiener_halfwin1 + wiener_halfwin1 - 1] -
            2 * B[(wiener_halfwin1 - 1) * wiener_halfwin1 + (wiener_halfwin1 - 1)];
    }
    for (i = 0; i < wiener_halfwin1 - 1; ++i) {
        for (j = 0; j < wiener_halfwin1 - 1; ++j) {
            B[i * wiener_halfwin1 + j] -= 2 *
                (B[i * wiener_halfwin1 + (wiener_halfwin1 - 1)] + B[(wiener_halfwin1 - 1) * wiener_halfwin1 + j] -
                 2 * B[(wiener_halfwin1 - 1) * wiener_halfwin1 + (wiener_halfwin1 - 1)]);
        }
    }
    if (linsolve_wiener(wiener_halfwin1 - 1, B, wiener_halfwin1, A, S)) {
        S[wiener_halfwin1 - 1] = WIENER_TAP_SCALE_FACTOR;
        for (i = wiener_halfwin1; i < wiener_win; ++i) {
            S[i] = S[wiener_win - 1 - i];
            S[wiener_halfwin1 - 1] -= 2 * S[i];
        }
        svt_memcpy(b, S, wiener_win * sizeof(*b));
    }
}

static void wiener_decompose_sep_sym(int32_t wiener_win, int64_t* M, int64_t* H, int32_t* a, int32_t* b) {
    static const int32_t init_filt[WIENER_WIN] = {
        WIENER_FILT_TAP0_MIDV,
        WIENER_FILT_TAP1_MIDV,
        WIENER_FILT_TAP2_MIDV,
        WIENER_FILT_TAP3_MIDV,
        WIENER_FILT_TAP2_MIDV,
        WIENER_FILT_TAP1_MIDV,
        WIENER_FILT_TAP0_MIDV,
    };
    int64_t*      hc[WIENER_WIN2];
    int64_t*      Mc[WIENER_WIN];
    int32_t       i, j, iter;
    const int32_t plane_off   = (WIENER_WIN - wiener_win) >> 1;
    const int32_t wiener_win2 = wiener_win * wiener_win;
    for (i = 0; i < wiener_win; i++) {
        a[i] = b[i] = WIENER_TAP_SCALE_FACTOR / WIENER_FILT_STEP * init_filt[i + plane_off];
    }
    for (i = 0; i < wiener_win; i++) {
        Mc[i] = M + i * wiener_win;
        for (j = 0; j < wiener_win; j++) {
            hc[i * wiener_win + j] = H + i * wiener_win * wiener_win2 + j * wiener_win;
        }
    }

    iter = 1;
    while (iter < NUM_WIENER_ITERS) {
        update_a_sep_sym(wiener_win, Mc, hc, a, b);
        update_b_sep_sym(wiener_win, Mc, hc, a, b);
        iter++;
    }
}

static int64_t compute_score(int32_t wiener_win, int64_t* M, int64_t* H, InterpKernel vfilt, InterpKernel hfilt) {
    int32_t       ab[WIENER_WIN * WIENER_WIN];
    int16_t       a[WIENER_WIN], b[WIENER_WIN];
    int64_t       P = 0, Q = 0;
    int64_t       i_p = 0, i_q = 0;
    int64_t       score, i_score;
    int32_t       i, k, l;
    const int32_t plane_off   = (WIENER_WIN - wiener_win) >> 1;
    const int32_t wiener_win2 = wiener_win * wiener_win;

    a[WIENER_HALFWIN] = b[WIENER_HALFWIN] = WIENER_FILT_STEP;
    for (i = 0; i < WIENER_HALFWIN; ++i) {
        a[i] = a[WIENER_WIN - i - 1] = vfilt[i];
        b[i] = b[WIENER_WIN - i - 1] = hfilt[i];
        a[WIENER_HALFWIN] -= 2 * a[i];
        b[WIENER_HALFWIN] -= 2 * b[i];
    }
    memset(ab, 0, sizeof(ab));
    for (k = 0; k < wiener_win; ++k) {
        for (l = 0; l < wiener_win; ++l) {
            ab[k * wiener_win + l] = a[l + plane_off] * b[k + plane_off];
        }
    }
    for (k = 0; k < wiener_win2; ++k) {
        P += ab[k] * M[k] / WIENER_FILT_STEP / WIENER_FILT_STEP;
        for (l = 0; l < wiener_win2; ++l) {
            Q += ab[k] * H[k * wiener_win2 + l] * ab[l] / WIENER_FILT_STEP / WIENER_FILT_STEP / WIENER_FILT_STEP /
                WIENER_FILT_STEP;
        }
    }
    score = Q - 2 * P;

    i_p     = M[wiener_win2 >> 1];
    i_q     = H[(wiener_win2 >> 1) * wiener_win2 + (wiener_win2 >> 1)];
    i_score = i_q - 2 * i_p;

    return score - i_score;
}

static void finalize_sym_filter(int32_t wiener_win, int32_t* f, InterpKernel fi) {
    int32_t       i;
    const int32_t wiener_halfwin = (wiener_win >> 1);

    for (i = 0; i < wiener_halfwin; ++i) {
        const int64_t dividend = (int64_t)f[i] * WIENER_FILT_STEP;
        const int64_t divisor  = WIENER_TAP_SCALE_FACTOR;
        // Perform this division with proper rounding rather than truncation
        if (dividend < 0) {
            fi[i] = (int16_t)((dividend - (divisor / 2)) / divisor);
        } else {
            fi[i] = (int16_t)((dividend + (divisor / 2)) / divisor);
        }
    }
    // Specialize for 7-tap filter
    if (wiener_win == WIENER_WIN) {
        fi[0] = CLIP(fi[0], WIENER_FILT_TAP0_MINV, WIENER_FILT_TAP0_MAXV);
        fi[1] = CLIP(fi[1], WIENER_FILT_TAP1_MINV, WIENER_FILT_TAP1_MAXV);
        fi[2] = CLIP(fi[2], WIENER_FILT_TAP2_MINV, WIENER_FILT_TAP2_MAXV);
    } else {
        fi[2] = CLIP(fi[1], WIENER_FILT_TAP2_MINV, WIENER_FILT_TAP2_MAXV);
        fi[1] = CLIP(fi[0], WIENER_FILT_TAP1_MINV, WIENER_FILT_TAP1_MAXV);
        fi[0] = 0;
    }
    // Satisfy filter constraints
    fi[WIENER_WIN - 1] = fi[0];
    fi[WIENER_WIN - 2] = fi[1];
    fi[WIENER_WIN - 3] = fi[2];
    // The central element has an implicit +WIENER_FILT_STEP
    fi[3] = -2 * (fi[0] + fi[1] + fi[2]);
}

static int32_t count_wiener_bits(int32_t wiener_win, WienerInfo* wiener_info, WienerInfo* ref_wiener_info) {
    int32_t bits = 0;
    if (wiener_win == WIENER_WIN) {
        bits += svt_aom_count_primitive_refsubexpfin(WIENER_FILT_TAP0_MAXV - WIENER_FILT_TAP0_MINV + 1,
                                                     WIENER_FILT_TAP0_SUBEXP_K,
                                                     ref_wiener_info->vfilter[0] - WIENER_FILT_TAP0_MINV,
                                                     wiener_info->vfilter[0] - WIENER_FILT_TAP0_MINV);
    }
    bits += svt_aom_count_primitive_refsubexpfin(WIENER_FILT_TAP1_MAXV - WIENER_FILT_TAP1_MINV + 1,
                                                 WIENER_FILT_TAP1_SUBEXP_K,
                                                 ref_wiener_info->vfilter[1] - WIENER_FILT_TAP1_MINV,
                                                 wiener_info->vfilter[1] - WIENER_FILT_TAP1_MINV);
    bits += svt_aom_count_primitive_refsubexpfin(WIENER_FILT_TAP2_MAXV - WIENER_FILT_TAP2_MINV + 1,
                                                 WIENER_FILT_TAP2_SUBEXP_K,
                                                 ref_wiener_info->vfilter[2] - WIENER_FILT_TAP2_MINV,
                                                 wiener_info->vfilter[2] - WIENER_FILT_TAP2_MINV);
    if (wiener_win == WIENER_WIN) {
        bits += svt_aom_count_primitive_refsubexpfin(WIENER_FILT_TAP0_MAXV - WIENER_FILT_TAP0_MINV + 1,
                                                     WIENER_FILT_TAP0_SUBEXP_K,
                                                     ref_wiener_info->hfilter[0] - WIENER_FILT_TAP0_MINV,
                                                     wiener_info->hfilter[0] - WIENER_FILT_TAP0_MINV);
    }
    bits += svt_aom_count_primitive_refsubexpfin(WIENER_FILT_TAP1_MAXV - WIENER_FILT_TAP1_MINV + 1,
                                                 WIENER_FILT_TAP1_SUBEXP_K,
                                                 ref_wiener_info->hfilter[1] - WIENER_FILT_TAP1_MINV,
                                                 wiener_info->hfilter[1] - WIENER_FILT_TAP1_MINV);
    bits += svt_aom_count_primitive_refsubexpfin(WIENER_FILT_TAP2_MAXV - WIENER_FILT_TAP2_MINV + 1,
                                                 WIENER_FILT_TAP2_SUBEXP_K,
                                                 ref_wiener_info->hfilter[2] - WIENER_FILT_TAP2_MINV,
                                                 wiener_info->hfilter[2] - WIENER_FILT_TAP2_MINV);
    return bits;
}

/* Perform refinement search around inital wiener filter coeffs passed in rui->wiener_info;
   compute and return the SSE of the best filter parameters.
*/
static int64_t finer_tile_search_wiener_seg(const RestSearchCtxt* rsc, const RestorationTileLimits* limits,
                                            const Av1PixelRect* tile, RestorationUnitInfo* rui, int32_t wiener_win) {
    const Av1Common* const cm           = rsc->cm;
    const int32_t          plane_off    = (WIENER_WIN - wiener_win) >> 1;
    int64_t                err          = try_restoration_unit_seg(rsc, limits, tile, rui);
    WienerInfo*            plane_wiener = &rui->wiener_info;

    if (cm->wn_filter_ctrls.use_refinement) {
        const int32_t start_step = 4;
        const int32_t end_step   = cm->wn_filter_ctrls.max_one_refinement_step ? 4 : 1;
        int64_t       err2;
        int32_t       tap_min[] = {WIENER_FILT_TAP0_MINV, WIENER_FILT_TAP1_MINV, WIENER_FILT_TAP2_MINV};
        int32_t       tap_max[] = {WIENER_FILT_TAP0_MAXV, WIENER_FILT_TAP1_MAXV, WIENER_FILT_TAP2_MAXV};
        for (int32_t s = start_step; s >= end_step; s >>= 1) {
            for (int32_t p = plane_off; p < WIENER_HALFWIN; ++p) {
                int32_t skip = 0;
                do {
                    if (plane_wiener->hfilter[p] - s >= tap_min[p]) {
                        plane_wiener->hfilter[p] -= (int16_t)s;
                        plane_wiener->hfilter[WIENER_WIN - p - 1] -= (int16_t)s;
                        plane_wiener->hfilter[WIENER_HALFWIN] += 2 * (int16_t)s;
                        err2 = try_restoration_unit_seg(rsc, limits, tile, rui);
                        if (err2 > err) {
                            plane_wiener->hfilter[p] += (int16_t)s;
                            plane_wiener->hfilter[WIENER_WIN - p - 1] += (int16_t)s;
                            plane_wiener->hfilter[WIENER_HALFWIN] -= 2 * (int16_t)s;
                        } else {
                            err  = err2;
                            skip = 1;
                            // At the highest step size continue moving in the same direction
                            if (s == start_step && !cm->wn_filter_ctrls.max_one_refinement_step) {
                                continue;
                            }
                        }
                    }
                    break;
                } while (1);
                if (skip) {
                    break;
                }
                do {
                    if (plane_wiener->hfilter[p] + s <= tap_max[p]) {
                        plane_wiener->hfilter[p] += (int16_t)s;
                        plane_wiener->hfilter[WIENER_WIN - p - 1] += (int16_t)s;
                        plane_wiener->hfilter[WIENER_HALFWIN] -= 2 * (int16_t)s;
                        err2 = try_restoration_unit_seg(rsc, limits, tile, rui);
                        if (err2 > err) {
                            plane_wiener->hfilter[p] -= (int16_t)s;
                            plane_wiener->hfilter[WIENER_WIN - p - 1] -= (int16_t)s;
                            plane_wiener->hfilter[WIENER_HALFWIN] += 2 * (int16_t)s;
                        } else {
                            err = err2;
                            // At the highest step size continue moving in the same direction
                            if (s == start_step && !cm->wn_filter_ctrls.max_one_refinement_step) {
                                continue;
                            }
                        }
                    }
                    break;
                } while (1);
            }
            for (int32_t p = plane_off; p < WIENER_HALFWIN; ++p) {
                int32_t skip = 0;
                do {
                    if (plane_wiener->vfilter[p] - s >= tap_min[p]) {
                        plane_wiener->vfilter[p] -= (int16_t)s;
                        plane_wiener->vfilter[WIENER_WIN - p - 1] -= (int16_t)s;
                        plane_wiener->vfilter[WIENER_HALFWIN] += 2 * (int16_t)s;
                        err2 = try_restoration_unit_seg(rsc, limits, tile, rui);
                        if (err2 > err) {
                            plane_wiener->vfilter[p] += (int16_t)s;
                            plane_wiener->vfilter[WIENER_WIN - p - 1] += (int16_t)s;
                            plane_wiener->vfilter[WIENER_HALFWIN] -= 2 * (int16_t)s;
                        } else {
                            err  = err2;
                            skip = 1;
                            // At the highest step size continue moving in the same direction
                            if (s == start_step && !cm->wn_filter_ctrls.max_one_refinement_step) {
                                continue;
                            }
                        }
                    }
                    break;
                } while (1);
                if (skip) {
                    break;
                }
                do {
                    if (plane_wiener->vfilter[p] + s <= tap_max[p]) {
                        plane_wiener->vfilter[p] += (int16_t)s;
                        plane_wiener->vfilter[WIENER_WIN - p - 1] += (int16_t)s;
                        plane_wiener->vfilter[WIENER_HALFWIN] -= 2 * (int16_t)s;
                        err2 = try_restoration_unit_seg(rsc, limits, tile, rui);
                        if (err2 > err) {
                            plane_wiener->vfilter[p] -= (int16_t)s;
                            plane_wiener->vfilter[WIENER_WIN - p - 1] -= (int16_t)s;
                            plane_wiener->vfilter[WIENER_HALFWIN] += 2 * (int16_t)s;
                        } else {
                            err = err2;
                            // At the highest step size continue moving in the same direction
                            if (s == start_step && !cm->wn_filter_ctrls.max_one_refinement_step) {
                                continue;
                            }
                        }
                    }
                    break;
                } while (1);
            }
        }
    }
    return err;
}

static void search_switchable(const RestorationTileLimits* limits, const Av1PixelRect* tile_rect, int32_t rest_unit_idx,
                              void* priv) {
    (void)limits;
    (void)tile_rect;
    RestSearchCtxt*     rsc  = (RestSearchCtxt*)priv;
    RestUnitSearchInfo* rusi = &rsc->rusi[rest_unit_idx];

    const Macroblock* const x = rsc->x;

    const int32_t wiener_win = (rsc->plane == AOM_PLANE_Y) ? WIENER_WIN : WIENER_WIN_CHROMA;

    double          best_cost  = 0;
    int64_t         best_bits  = 0;
    RestorationType best_rtype = RESTORE_NONE;

    //CHKN for (RestorationType r = 0; r < RESTORE_SWITCHABLE_TYPES; ++r) {
    for (int32_t rest_type = 0; rest_type < RESTORE_SWITCHABLE_TYPES; ++rest_type) {
        RestorationType r = (RestorationType)rest_type;

        // Check for the condition that wiener or sgrproj search could not
        // find a solution or the solution was worse than RESTORE_NONE.
        // In either case the best_rtype will be set as RESTORE_NONE. These
        // should be skipped from the test below.
        if (r > RESTORE_NONE) {
            if (rusi->best_rtype[r - 1] == RESTORE_NONE) {
                continue;
            }
        }
        const int64_t sse         = rusi->sse[r];
        int64_t       coeff_pcost = 0;
        switch (r) {
        case RESTORE_NONE:
            coeff_pcost = 0;
            break;
        case RESTORE_WIENER:
            coeff_pcost = count_wiener_bits(wiener_win, &rusi->wiener, &rsc->wiener);
            break;
        case RESTORE_SGRPROJ:
            coeff_pcost = count_sgrproj_bits(&rusi->sgrproj, &rsc->sgrproj);
            break;
        default:
            assert(0);
            break;
        }
        const int64_t coeff_bits = coeff_pcost << AV1_PROB_COST_SHIFT;
        const int64_t bits       = x->switchable_restore_cost[r] + coeff_bits;
        double        cost       = RDCOST_DBL(x->rdmult, bits >> 4, sse);
        if (r == 0 || cost < best_cost) {
            best_cost  = cost;
            best_bits  = bits;
            best_rtype = r;
        }
    }

    rusi->best_rtype[RESTORE_SWITCHABLE - 1] = best_rtype;

    rsc->sse += rusi->sse[best_rtype];
    rsc->bits += best_bits;
    if (best_rtype == RESTORE_WIENER) {
        rsc->wiener = rusi->wiener;
    }
    if (best_rtype == RESTORE_SGRPROJ) {
        rsc->sgrproj = rusi->sgrproj;
    }
}

static void copy_unit_info(RestorationType frame_rtype, const RestUnitSearchInfo* rusi, RestorationUnitInfo* rui) {
    if (frame_rtype >= 1) {
        rui->restoration_type = rusi->best_rtype[frame_rtype - 1];
    }
    if (rui->restoration_type == RESTORE_WIENER) {
        rui->wiener_info = rusi->wiener;
    } else {
        rui->sgrproj_info = rusi->sgrproj;
    }
}

static int32_t rest_tiles_in_plane(const Av1Common* cm, int32_t plane) {
    const RestorationInfo* rsi = &cm->child_pcs->rst_info[plane];
    return rsi->units_per_tile;
}

/* Perform search for the best self-guided filter parameters and compute the SSE. */
static void search_sgrproj_seg(const RestorationTileLimits* limits, const Av1PixelRect* tile, int32_t rest_unit_idx,
                               void* priv) {
    RestSearchCtxt*     rsc  = (RestSearchCtxt*)priv;
    RestUnitSearchInfo* rusi = &rsc->rusi[rest_unit_idx];

    Av1Common* const cm        = rsc->cm;
    const int32_t    highbd    = cm->use_highbitdepth;
    const int32_t    bit_depth = cm->bit_depth;

    uint8_t*       dgd_start = rsc->dgd_buffer + limits->v_start * rsc->dgd_stride + limits->h_start;
    const uint8_t* src_start = rsc->src_buffer + limits->v_start * rsc->src_stride + limits->h_start;

    const int32_t is_uv           = rsc->plane > 0;
    const int32_t ss_x            = is_uv && cm->subsampling_x;
    const int32_t ss_y            = is_uv && cm->subsampling_y;
    const int32_t procunit_width  = RESTORATION_PROC_UNIT_SIZE >> ss_x;
    const int32_t procunit_height = RESTORATION_PROC_UNIT_SIZE >> ss_y;

    rusi->sgrproj = search_selfguided_restoration(dgd_start,
                                                  limits->h_end - limits->h_start,
                                                  limits->v_end - limits->v_start,
                                                  rsc->dgd_stride,
                                                  src_start,
                                                  rsc->src_stride,
                                                  highbd,
                                                  bit_depth,
                                                  procunit_width,
                                                  procunit_height,
                                                  rsc->tmpbuf,
                                                  &cm->sg_filter_ctrls,
                                                  rsc->plane);
    RestorationUnitInfo rui;
    rui.restoration_type = RESTORE_SGRPROJ;
    rui.sgrproj_info     = rusi->sgrproj;

    rusi->sse[RESTORE_SGRPROJ] = try_restoration_unit_seg(rsc, limits, tile, &rui);
}

/* Get the cost/SSE/rate of using self-guided filtering for a given restoration unit. */
static void search_sgrproj_finish(const RestorationTileLimits* limits, const Av1PixelRect* tile, int32_t rest_unit_idx,
                                  void* priv) {
    (void)limits;
    (void)tile;
    RestSearchCtxt*     rsc  = (RestSearchCtxt*)priv;
    RestUnitSearchInfo* rusi = &rsc->rusi[rest_unit_idx];

    const Macroblock* const x = rsc->x;

    rusi->sse[RESTORE_SGRPROJ] = rsc->rusi_pic[rest_unit_idx].sse[RESTORE_SGRPROJ];
    rusi->sgrproj              = rsc->rusi_pic[rest_unit_idx].sgrproj;

    const int64_t bits_none = x->sgrproj_restore_cost[0];
    const int64_t bits_sgr  = x->sgrproj_restore_cost[1] +
        (count_sgrproj_bits(&rusi->sgrproj, &rsc->sgrproj) << AV1_PROB_COST_SHIFT);

    double cost_none = RDCOST_DBL(x->rdmult, bits_none >> 4, rusi->sse[RESTORE_NONE]);
    double cost_sgr  = RDCOST_DBL(x->rdmult, bits_sgr >> 4, rusi->sse[RESTORE_SGRPROJ]);

    RestorationType rtype                 = (cost_sgr < cost_none) ? RESTORE_SGRPROJ : RESTORE_NONE;
    rusi->best_rtype[RESTORE_SGRPROJ - 1] = rtype;

    rsc->sse += rusi->sse[rtype];
    rsc->bits += (cost_sgr < cost_none) ? bits_sgr : bits_none;
    if (cost_sgr < cost_none) {
        rsc->sgrproj = rusi->sgrproj;
    }
}

/*Get the best Wiender filter parameters and SSE.*/
static void search_wiener_seg(const RestorationTileLimits* limits, const Av1PixelRect* tile_rect, int32_t rest_unit_idx,
                              void* priv) {
    RestSearchCtxt*            rsc      = (RestSearchCtxt*)priv;
    RestUnitSearchInfo*        rusi     = &rsc->rusi[rest_unit_idx];
    const Av1Common* const     cm       = rsc->cm;
    const WnFilterCtrls* const wn_ctrls = &cm->wn_filter_ctrls;
    assert(wn_ctrls->filter_tap_lvl == 1 || wn_ctrls->filter_tap_lvl == 2);
    const int32_t wn_luma = wn_ctrls->filter_tap_lvl == 1 ? WIENER_WIN : WIENER_WIN_CHROMA;

    const int32_t wiener_win = rsc->plane == AOM_PLANE_Y ? wn_luma : WIENER_WIN_CHROMA;

    RestorationUnitInfo rui;
    memset(&rui, 0, sizeof(rui));
    rui.restoration_type = RESTORE_WIENER;
    // Check whether you can use the filter coeffs from previous frames; if not, must generate new coeffs
    if (wn_ctrls->use_prev_frame_coeffs && !frame_is_intra_only(cm->child_pcs->ppcs) &&
        cm->child_pcs->rst_info[rsc->plane].unit_info[rest_unit_idx].restoration_type == RESTORE_WIENER) {
        // Copy filter info, stored from previous frame(s)
        rui.wiener_info = cm->child_pcs->rst_info[rsc->plane].unit_info[rest_unit_idx].wiener_info;
    } else {
        EB_ALIGN(32) int64_t M[WIENER_WIN2];
        EB_ALIGN(32) int64_t H[WIENER_WIN2 * WIENER_WIN2];
        int32_t              vfilterd[WIENER_WIN], hfilterd[WIENER_WIN];

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
        if (cm->use_highbitdepth) {
            svt_av1_compute_stats_highbd(wiener_win,
                                         rsc->dgd_buffer,
                                         rsc->src_buffer,
                                         limits->h_start,
                                         limits->h_end,
                                         limits->v_start,
                                         limits->v_end,
                                         rsc->dgd_stride,
                                         rsc->src_stride,
                                         M,
                                         H,
                                         (EbBitDepth)cm->bit_depth);
        } else
#endif
        {
            svt_av1_compute_stats(wiener_win,
                                  rsc->dgd_buffer,
                                  rsc->src_buffer,
                                  limits->h_start,
                                  limits->h_end,
                                  limits->v_start,
                                  limits->v_end,
                                  rsc->dgd_stride,
                                  rsc->src_stride,
                                  M,
                                  H);
        }

        wiener_decompose_sep_sym(wiener_win, M, H, vfilterd, hfilterd);
        finalize_sym_filter(wiener_win, vfilterd, rui.wiener_info.vfilter);
        finalize_sym_filter(wiener_win, hfilterd, rui.wiener_info.hfilter);

        // Filter score computes the value of the function x'*A*x - x'*b for the
        // learned filter and compares it against identity filer. If there is no
        // reduction in the function, the filter is reverted back to identity
        if (compute_score(wiener_win, M, H, rui.wiener_info.vfilter, rui.wiener_info.hfilter) > 0) {
            rusi->sse[RESTORE_WIENER] = INT64_MAX;
            return;
        }
    }
    // Perform refinement search for filter coeffs and compute SSE
    rusi->sse[RESTORE_WIENER] = finer_tile_search_wiener_seg(rsc, limits, tile_rect, &rui, wiener_win);
    rusi->wiener              = rui.wiener_info;

    if (wiener_win != WIENER_WIN) {
        assert(rui.wiener_info.vfilter[0] == 0 && rui.wiener_info.vfilter[WIENER_WIN - 1] == 0);
        assert(rui.wiener_info.hfilter[0] == 0 && rui.wiener_info.hfilter[WIENER_WIN - 1] == 0);
    }
}

/* Get the cost/SSE/rate of using wiener filtering for a given restoration unit. */
static void search_wiener_finish(const RestorationTileLimits* limits, const Av1PixelRect* tile_rect,
                                 int32_t rest_unit_idx, void* priv) {
    (void)limits;
    (void)tile_rect;
    RestSearchCtxt*            rsc      = (RestSearchCtxt*)priv;
    RestUnitSearchInfo*        rusi     = &rsc->rusi[rest_unit_idx];
    const Av1Common* const     cm       = rsc->cm;
    const WnFilterCtrls* const wn_ctrls = &cm->wn_filter_ctrls;
    assert(wn_ctrls->filter_tap_lvl == 1 || wn_ctrls->filter_tap_lvl == 2);
    const int32_t           wn_luma    = wn_ctrls->filter_tap_lvl == 1 ? WIENER_WIN : WIENER_WIN_CHROMA;
    const int32_t           wiener_win = rsc->plane == AOM_PLANE_Y ? wn_luma : WIENER_WIN_CHROMA;
    const Macroblock* const x          = rsc->x;
    const int64_t           bits_none  = x->wiener_restore_cost[0];

    RestorationUnitInfo rui;
    memset(&rui, 0, sizeof(rui));
    rui.restoration_type = RESTORE_WIENER;

    // Filter score computes the value of the function x'*A*x - x'*b for the
    // learned filter and compares it against identity filer. If there is no
    // reduction in the function, the filter is reverted back to identity

    rusi->sse[RESTORE_WIENER] = rsc->rusi_pic[rest_unit_idx].sse[RESTORE_WIENER];
    if (rusi->sse[RESTORE_WIENER] == INT64_MAX) {
        rsc->bits += bits_none;
        rsc->sse += rusi->sse[RESTORE_NONE];
        rusi->best_rtype[RESTORE_NONE] = RESTORE_NONE;
        rusi->sse[RESTORE_WIENER]      = INT64_MAX;
        return;
    }

    rusi->wiener = rsc->rusi_pic[rest_unit_idx].wiener;

    const int64_t bits_wiener = x->wiener_restore_cost[1] +
        (count_wiener_bits(wiener_win, &rusi->wiener, &rsc->wiener) << AV1_PROB_COST_SHIFT);

    double cost_none   = RDCOST_DBL(x->rdmult, bits_none >> 4, rusi->sse[RESTORE_NONE]);
    double cost_wiener = RDCOST_DBL(x->rdmult, bits_wiener >> 4, rusi->sse[RESTORE_WIENER]);

    RestorationType rtype          = (cost_wiener < cost_none) ? RESTORE_WIENER : RESTORE_NONE;
    rusi->best_rtype[RESTORE_NONE] = rtype;

    rsc->sse += rusi->sse[rtype];
    rsc->bits += (cost_wiener < cost_none) ? bits_wiener : bits_none;
    if (cost_wiener < cost_none) {
        rsc->wiener = rusi->wiener;
    }
}

static void search_norestore_seg(const RestorationTileLimits* limits, const Av1PixelRect* tile_rect,
                                 int32_t rest_unit_idx, void* priv) {
    (void)tile_rect;

    RestSearchCtxt*     rsc  = (RestSearchCtxt*)priv;
    RestUnitSearchInfo* rusi = &rsc->rusi[rest_unit_idx];

    const int32_t highbd    = rsc->cm->use_highbitdepth;
    rusi->sse[RESTORE_NONE] = sse_restoration_unit(limits, rsc->src, rsc->cm->frame_to_show, rsc->plane, highbd);
}

// Get the SSE for a resotration unit with no filtering applied
static void search_norestore_finish(const RestorationTileLimits* limits, const Av1PixelRect* tile_rect,
                                    int32_t rest_unit_idx, void* priv) {
    (void)tile_rect;
    (void)limits;

    RestSearchCtxt*     rsc  = (RestSearchCtxt*)priv;
    RestUnitSearchInfo* rusi = &rsc->rusi[rest_unit_idx];

    rusi->sse[RESTORE_NONE] = rsc->rusi_pic[rest_unit_idx].sse[RESTORE_NONE];

    rsc->sse += rusi->sse[RESTORE_NONE];
}

/* Get the cost/SSE/rate for the entire frame associated with using the passed restoration filter type. */
static double search_rest_type_finish(RestSearchCtxt* rsc, RestorationType rtype) {
    static const RestUnitVisitor funs[RESTORE_TYPES] = {
        search_norestore_finish, search_wiener_finish, search_sgrproj_finish, search_switchable};

    reset_rsc(rsc);

    svt_aom_foreach_rest_unit_in_frame(rsc->cm, rsc->plane, rsc_on_tile, funs[rtype], rsc);

    return RDCOST_DBL(rsc->x->rdmult, rsc->bits >> 4, rsc->sse);
}

/* Search the available type of restoration filters: OFF, Wiener, and self-guided.

   The search will return the best parameters for each type of filter and their associated SSEs,
   to be used in the final decision for filtering the frame.
*/
void restoration_seg_search(int32_t* rst_tmpbuf, Yv12BufferConfig* org_fts, const Yv12BufferConfig* src,
                            Yv12BufferConfig* trial_frame_rst, PictureControlSet* pcs, uint32_t segment_index) {
    Av1Common* const cm = pcs->ppcs->av1_cm;
    Macroblock*      x  = pcs->ppcs->av1x;

    // If the restoration unit dimensions are not multiples of
    // rsi->restoration_unit_size then some elements of the rusi array may be
    // left uninitialised when we reach copy_unit_info(...). This is not a
    // problem, as these elements are ignored later, but in order to quiet
    // Valgrind's warnings we initialise the array  to zero.

    RestSearchCtxt  rsc; //this context is specific for this segment
    RestSearchCtxt* rsc_p = &rsc;

    const int32_t plane_start = AOM_PLANE_Y;
    const int32_t plane_end   = ((cm->wn_filter_ctrls.enabled && cm->wn_filter_ctrls.use_chroma) ||
                               (cm->sg_filter_ctrls.enabled && cm->sg_filter_ctrls.use_chroma))
          ? AOM_PLANE_V
          : AOM_PLANE_Y;

    for (int32_t plane = plane_start; plane <= plane_end; ++plane) {
        RestUnitSearchInfo* rusi = pcs->rusi_picture[plane];

        init_rsc_seg(org_fts, src, cm, x, plane, rusi, trial_frame_rst, &rsc);

        rsc_p->tmpbuf = rst_tmpbuf;

        const int32_t highbd = rsc.cm->use_highbitdepth;
        svt_block_on_mutex(pcs->rest_search_mutex);
        if (!pcs->rest_extend_flag[plane]) {
            // Add additional padding to align 16 pixel in width to address uninitialized memory warning in sanitizer.
            // There is a width extension of 16 pixel alignment in wiener_filter_stripe for SIMD, it causes warning of
            // access uninitialized memory reported in "C" code path when pictures are downscaled and are not aligned.
            // For the unscaled pictures, there will be no additional padding added.
            const int32_t align16_pad = (rsc.plane_width % 16) ? 16 - (rsc.plane_width % 16) : 0;
            // The horizontal padding is increased by 1 to address an uninitialized memory access when
            // using the "C" code path.  In svt_aom_horz_scalar_product, where the wiener filter is applied to the pixels,
            // the right-edge pixels will need 3 padded pixels to perform a 7-tap filter. However, the filter is applied
            // over 8 (SUBPEL_TAPS) pixels, with the final 8th weight being zero. Therefore, the extra right-most pixel
            // will not affect the result, but will cause a sanitizer failure if not initialized.
            svt_extend_frame(rsc.dgd_buffer,
                             rsc.plane_width,
                             rsc.plane_height,
                             rsc.dgd_stride,
                             RESTORATION_BORDER + 1 + align16_pad,
                             RESTORATION_BORDER,
                             highbd);
            pcs->rest_extend_flag[plane] = true;
        }
        svt_release_mutex(pcs->rest_search_mutex);

        svt_aom_foreach_rest_unit_in_frame_seg(rsc_p->cm,
                                               rsc_p->plane,
                                               rsc_on_tile,
                                               search_norestore_seg,
                                               rsc_p,
                                               pcs->rest_segments_column_count,
                                               pcs->rest_segments_row_count,
                                               segment_index);

        if (cm->wn_filter_ctrls.enabled && (!plane || cm->wn_filter_ctrls.use_chroma)) {
            svt_aom_foreach_rest_unit_in_frame_seg(rsc_p->cm,
                                                   rsc_p->plane,
                                                   rsc_on_tile,
                                                   search_wiener_seg,
                                                   rsc_p,
                                                   pcs->rest_segments_column_count,
                                                   pcs->rest_segments_row_count,
                                                   segment_index);
        }

        if (cm->sg_filter_ctrls.enabled && (!plane || cm->sg_filter_ctrls.use_chroma)) {
            svt_aom_foreach_rest_unit_in_frame_seg(rsc_p->cm,
                                                   rsc_p->plane,
                                                   rsc_on_tile,
                                                   search_sgrproj_seg,
                                                   rsc_p,
                                                   pcs->rest_segments_column_count,
                                                   pcs->rest_segments_row_count,
                                                   segment_index);
        }
    }
}

/* Given the best parameters for each type of filter and their associated SSEs,
   decide which filter should be used for each filter block.
*/
void rest_finish_search(PictureControlSet* pcs) {
    Macroblock*              x                    = pcs->ppcs->av1x;
    Av1Common* const         cm                   = pcs->ppcs->av1_cm;
    PictureParentControlSet* p_pcs_ptr            = pcs->ppcs;
    RestorationType          force_restore_type_d = (cm->wn_filter_ctrls.enabled)
                 ? ((cm->sg_filter_ctrls.enabled) ? RESTORE_TYPES : RESTORE_WIENER)
                 : ((cm->sg_filter_ctrls.enabled) ? RESTORE_SGRPROJ : RESTORE_NONE);
    int32_t                  ntiles[2];
    for (int32_t is_uv = 0; is_uv < 2; ++is_uv) {
        ntiles[is_uv] = rest_tiles_in_plane(cm, is_uv);
    }

    assert(ntiles[1] <= ntiles[0]);
    RestUnitSearchInfo* rusi = (RestUnitSearchInfo*)svt_aom_memalign(16, sizeof(*rusi) * ntiles[0]);

    // If the restoration unit dimensions are not multiples of
    // rsi->restoration_unit_size then some elements of the rusi array may be
    // left uninitialised when we reach copy_unit_info(...). This is not a
    // problem, as these elements are ignored later, but in order to quiet
    // Valgrind's warnings we initialise the array below.
    memset(rusi, 0, sizeof(*rusi) * ntiles[0]);

    RestSearchCtxt rsc;
    const int32_t  plane_start = AOM_PLANE_Y;
    const int32_t  plane_end   = ((cm->wn_filter_ctrls.enabled && cm->wn_filter_ctrls.use_chroma) ||
                               (cm->sg_filter_ctrls.enabled && cm->sg_filter_ctrls.use_chroma))
           ? AOM_PLANE_V
           : AOM_PLANE_Y;

    for (int32_t plane = plane_start; plane <= plane_end; ++plane) {
        //init rsc context for this plane
        rsc.cm       = cm;
        rsc.x        = x;
        rsc.plane    = plane;
        rsc.rusi     = rusi;
        rsc.pic_num  = (uint32_t)p_pcs_ptr->picture_number;
        rsc.rusi_pic = pcs->rusi_picture[plane];

        const int32_t         plane_ntiles = ntiles[plane > 0];
        const RestorationType num_rtypes   = (plane_ntiles > 1) ? RESTORE_TYPES : RESTORE_SWITCHABLE_TYPES;

        double          best_cost  = 0;
        RestorationType best_rtype = RESTORE_NONE;

        for (int32_t rest_type = 0; rest_type < num_rtypes; ++rest_type) {
            RestorationType r = (RestorationType)rest_type;

            if ((force_restore_type_d != RESTORE_TYPES) && (r != RESTORE_NONE) && (r != force_restore_type_d)) {
                continue;
            }

            if (plane) {
                // if current filter was not tested for chroma plane, do not check the cost
                if ((r == RESTORE_WIENER && !cm->wn_filter_ctrls.use_chroma) ||
                    (r == RESTORE_SGRPROJ && !cm->sg_filter_ctrls.use_chroma)) {
                    continue;
                }
            }

            double cost = search_rest_type_finish(&rsc, r);

            if (r == 0 || cost < best_cost) {
                best_cost  = cost;
                best_rtype = r;
            }
        }
        cm->child_pcs->rst_info[plane].frame_restoration_type = best_rtype;
        if (force_restore_type_d != RESTORE_TYPES) {
            assert(best_rtype == force_restore_type_d || best_rtype == RESTORE_NONE);
        }

        if (best_rtype != RESTORE_NONE) {
            for (int32_t u = 0; u < plane_ntiles; ++u) {
                copy_unit_info(best_rtype, &rusi[u], &cm->child_pcs->rst_info[plane].unit_info[u]);
            }
        }
    }

    // if restoration performed for luma only, set chroma to RESTORE_NONE
    if (plane_end == AOM_PLANE_Y) {
        pcs->rst_info[1].frame_restoration_type = RESTORE_NONE;
        pcs->rst_info[2].frame_restoration_type = RESTORE_NONE;
    }

    svt_aom_free(rusi);
}
