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

#ifndef AV1_COMMON_RESTORATION_H_
#define AV1_COMMON_RESTORATION_H_

#include <math.h>
#include "definitions.h"
#include "pic_buffer_desc.h"
#include "av1_structs.h"

#ifdef __cplusplus
extern "C" {
#endif

void svt_apply_selfguided_restoration_c(const uint8_t* dat8, int32_t width, int32_t height, int32_t stride, int32_t eps,
                                        const int32_t* xqd, uint8_t* dst8, int32_t dst_stride, int32_t* tmpbuf,
                                        int32_t bit_depth, int32_t highbd);

// Border for Loop restoration buffer
#define AOM_RESTORATION_FRAME_BORDER 32

#define CLIP(x, lo, hi) ((x) < (lo) ? (lo) : (x) > (hi) ? (hi) : (x))
#define RINT(x) ((x) < 0 ? (int32_t)((x) - 0.5) : (int32_t)((x) + 0.5))

#define REAL_PTR(hbd, d) ((hbd) ? (uint8_t*)CONVERT_TO_SHORTPTR(d) : (d))

#define RESTORATION_PROC_UNIT_SIZE 64

// Filter tile grid offset upwards compared to the superblock grid
#define RESTORATION_UNIT_OFFSET 8

#define SGRPROJ_BORDER_VERT 3 // Vertical border used for Sgr
#define SGRPROJ_BORDER_HORZ 3 // Horizontal border used for Sgr

#define WIENER_BORDER_VERT 2 // Vertical border used for Wiener
#define WIENER_HALFWIN 3
#define WIENER_BORDER_HORZ (WIENER_HALFWIN) // Horizontal border for Wiener

// RESTORATION_BORDER_VERT determines line buffer requirement for LR.
// Should be set at the max of SGRPROJ_BORDER_VERT and WIENER_BORDER_VERT.
// Note the line buffer needed is twice the value of this macro.
#if SGRPROJ_BORDER_VERT >= WIENER_BORDER_VERT
#define RESTORATION_BORDER_VERT (SGRPROJ_BORDER_VERT)
#else
#define RESTORATION_BORDER_VERT (WIENER_BORDER_VERT)
#endif // SGRPROJ_BORDER_VERT >= WIENER_BORDER_VERT

#if SGRPROJ_BORDER_HORZ >= WIENER_BORDER_HORZ
#define RESTORATION_BORDER_HORZ (SGRPROJ_BORDER_HORZ)
#else
#define RESTORATION_BORDER_HORZ (WIENER_BORDER_HORZ)
#endif // SGRPROJ_BORDER_VERT >= WIENER_BORDER_VERT

// How many border pixels do we need for each processing unit?
#define RESTORATION_BORDER 3

// How many rows of deblocked pixels do we save above/below each processing
// stripe?
#define RESTORATION_CTX_VERT 2

// Additional pixels to the left and right in above/below buffers
// It is RESTORATION_BORDER_HORZ rounded up to get nicer buffer alignment
#define RESTORATION_EXTRA_HORZ 4

// Pad up to 20 more (may be much less is needed)
#define RESTORATION_PADDING 20
#define RESTORATION_PROC_UNIT_PELS                                                      \
    ((RESTORATION_PROC_UNIT_SIZE + RESTORATION_BORDER_HORZ * 2 + RESTORATION_PADDING) * \
     (RESTORATION_PROC_UNIT_SIZE + RESTORATION_BORDER_VERT * 2 + RESTORATION_PADDING))

#define RESTORATION_UNITSIZE_MAX 256
#define RESTORATION_UNITPELS_HORZ_MAX (RESTORATION_UNITSIZE_MAX * 3 / 2 + 2 * RESTORATION_BORDER_HORZ + 16)
#define RESTORATION_UNITPELS_VERT_MAX \
    ((RESTORATION_UNITSIZE_MAX * 3 / 2 + 2 * RESTORATION_BORDER_VERT + RESTORATION_UNIT_OFFSET))
#define RESTORATION_UNITPELS_MAX (RESTORATION_UNITPELS_HORZ_MAX * RESTORATION_UNITPELS_VERT_MAX)

// Two 32-bit buffers needed for the restored versions from two filters
#define SGRPROJ_TMPBUF_SIZE (RESTORATION_UNITPELS_MAX * 2 * sizeof(int32_t))

#define SGRPROJ_EXTBUF_SIZE (0)
#define SGRPROJ_PARAMS_BITS 4
#define SGRPROJ_PARAMS (1 << SGRPROJ_PARAMS_BITS)

// Precision bits for projection
#define SGRPROJ_PRJ_BITS 7
// Restoration precision bits generated higher than source before projection
#define SGRPROJ_RST_BITS 4
// Internal precision bits for core selfguided_restoration
#define SGRPROJ_SGR_BITS 8
#define SGRPROJ_SGR (1 << SGRPROJ_SGR_BITS)

#define SGRPROJ_PRJ_MIN0 (-(1 << SGRPROJ_PRJ_BITS) * 3 / 4)
#define SGRPROJ_PRJ_MAX0 (SGRPROJ_PRJ_MIN0 + (1 << SGRPROJ_PRJ_BITS) - 1)
#define SGRPROJ_PRJ_MIN1 (-(1 << SGRPROJ_PRJ_BITS) / 4)
#define SGRPROJ_PRJ_MAX1 (SGRPROJ_PRJ_MIN1 + (1 << SGRPROJ_PRJ_BITS) - 1)

#define SGRPROJ_PRJ_SUBEXP_K 4

#define SGRPROJ_BITS (SGRPROJ_PRJ_BITS * 2 + SGRPROJ_PARAMS_BITS)

#define MAX_RADIUS 2 // Only 1, 2, 3 allowed
#define MAX_NELEM ((2 * MAX_RADIUS + 1) * (2 * MAX_RADIUS + 1))
#define SGRPROJ_MTABLE_BITS 20
#define SGRPROJ_RECIP_BITS 12

#define WIENER_HALFWIN1 (WIENER_HALFWIN + 1)
#define WIENER_WIN (2 * WIENER_HALFWIN + 1)
#define WIENER_WIN2 ((WIENER_WIN) * (WIENER_WIN))
#define WIENER_TMPBUF_SIZE (0)
#define WIENER_EXTBUF_SIZE (0)

// If WIENER_WIN_CHROMA == WIENER_WIN - 2, that implies 5x5 filters are used for
// chroma. To use 7x7 for chroma set WIENER_WIN_CHROMA to WIENER_WIN.
#define WIENER_WIN_CHROMA (WIENER_WIN - 2)
#define WIENER_WIN2_CHROMA ((WIENER_WIN_CHROMA) * (WIENER_WIN_CHROMA))
#define WIENER_FILT_PREC_BITS 7
#define WIENER_FILT_STEP (1 << WIENER_FILT_PREC_BITS)

// Central values for the taps
#define WIENER_FILT_TAP0_MIDV (3)
#define WIENER_FILT_TAP1_MIDV (-7)
#define WIENER_FILT_TAP2_MIDV (15)
#define WIENER_FILT_TAP3_MIDV \
    (WIENER_FILT_STEP - 2 * (WIENER_FILT_TAP0_MIDV + WIENER_FILT_TAP1_MIDV + WIENER_FILT_TAP2_MIDV))

#define WIENER_FILT_TAP0_BITS 4
#define WIENER_FILT_TAP1_BITS 5
#define WIENER_FILT_TAP2_BITS 6

#define WIENER_FILT_BITS ((WIENER_FILT_TAP0_BITS + WIENER_FILT_TAP1_BITS + WIENER_FILT_TAP2_BITS) * 2)

#define WIENER_FILT_TAP0_MINV (WIENER_FILT_TAP0_MIDV - (1 << WIENER_FILT_TAP0_BITS) / 2)
#define WIENER_FILT_TAP1_MINV (WIENER_FILT_TAP1_MIDV - (1 << WIENER_FILT_TAP1_BITS) / 2)
#define WIENER_FILT_TAP2_MINV (WIENER_FILT_TAP2_MIDV - (1 << WIENER_FILT_TAP2_BITS) / 2)

#define WIENER_FILT_TAP0_MAXV (WIENER_FILT_TAP0_MIDV - 1 + (1 << WIENER_FILT_TAP0_BITS) / 2)
#define WIENER_FILT_TAP1_MAXV (WIENER_FILT_TAP1_MIDV - 1 + (1 << WIENER_FILT_TAP1_BITS) / 2)
#define WIENER_FILT_TAP2_MAXV (WIENER_FILT_TAP2_MIDV - 1 + (1 << WIENER_FILT_TAP2_BITS) / 2)

#define WIENER_FILT_TAP0_SUBEXP_K 1
#define WIENER_FILT_TAP1_SUBEXP_K 2
#define WIENER_FILT_TAP2_SUBEXP_K 3

// Max of SGRPROJ_TMPBUF_SIZE, DOMAINTXFMRF_TMPBUF_SIZE, WIENER_TMPBUF_SIZE
#define RESTORATION_TMPBUF_SIZE (SGRPROJ_TMPBUF_SIZE)

// Max of SGRPROJ_EXTBUF_SIZE, WIENER_EXTBUF_SIZE
#define RESTORATION_EXTBUF_SIZE (WIENER_EXTBUF_SIZE)

// Check the assumptions of the existing code
#if SUBPEL_TAPS != WIENER_WIN + 1
//#error "Wiener filter currently only works if SUBPEL_TAPS == WIENER_WIN + 1"
#endif
#if WIENER_FILT_PREC_BITS != 7
#error "Wiener filter currently only works if WIENER_FILT_PREC_BITS == 7"
#endif

typedef struct WienerInfo {
    DECLARE_ALIGNED(16, InterpKernel, vfilter);
    DECLARE_ALIGNED(16, InterpKernel, hfilter);
} WienerInfo;

typedef struct SgrprojInfo {
    int32_t ep;
    int32_t xqd[2];
} SgrprojInfo;

// Similarly, the column buffers (used when we're at a vertical tile edge
// that we can't filter across) need space for one processing unit's worth
// of pixels, plus the top/bottom border width
#define RESTORATION_COLBUFFER_HEIGHT (RESTORATION_PROC_UNIT_SIZE + 2 * RESTORATION_BORDER)

typedef struct RestorationUnitInfo {
    RestorationType restoration_type;
    WienerInfo      wiener_info;
    SgrprojInfo     sgrproj_info;
} RestorationUnitInfo;

typedef struct WienerUnitInfo {
    RestorationType restoration_type;
    WienerInfo      wiener_info;
} WienerUnitInfo;

typedef struct Av1PixelRect {
    int32_t left, top, right, bottom;
} Av1PixelRect;

// A restoration line buffer needs space for two lines plus a horizontal filter
// margin of RESTORATION_EXTRA_HORZ on each side.
#define RESTORATION_LINEBUFFER_WIDTH (RESTORATION_UNITSIZE_MAX * 3 / 2 + 2 * RESTORATION_EXTRA_HORZ)

// Similarly, the column buffers (used when we're at a vertical tile edge
// that we can't filter across) need space for one processing unit's worth
// of pixels, plus the top/bottom border width
#define RESTORATION_COLBUFFER_HEIGHT (RESTORATION_PROC_UNIT_SIZE + 2 * RESTORATION_BORDER)

typedef struct RestorationLineBuffers {
    // Temporary buffers to save/restore 3 lines above/below the restoration
    // stripe.
    uint16_t tmp_save_above[RESTORATION_BORDER][RESTORATION_LINEBUFFER_WIDTH];
    uint16_t tmp_save_below[RESTORATION_BORDER][RESTORATION_LINEBUFFER_WIDTH];

    // Temporary buffers to save/restore 4 column left/right of a processing unit.
    uint16_t tmp_save_cdef[MAX_SB_SIZE + RESTORATION_UNIT_OFFSET][RESTORATION_EXTRA_HORZ];
    uint16_t tmp_save_lr[MAX_SB_SIZE + RESTORATION_UNIT_OFFSET][RESTORATION_EXTRA_HORZ];
} RestorationLineBuffers;

typedef struct RestorationStripeBoundaries {
    uint8_t* stripe_boundary_above;
    uint8_t* stripe_boundary_below;
    int32_t  stripe_boundary_stride;
    int32_t  stripe_boundary_size;
} RestorationStripeBoundaries;

typedef struct RestorationInfo {
    RestorationType frame_restoration_type;
    int32_t         restoration_unit_size;

    // Fields below here are allocated and initialised by
    // svt_av1_alloc_restoration_struct. (horz_)units_per_tile give the number of
    // restoration units in (one row of) the largest tile in the frame. The data
    // in unit_info is laid out with units_per_tile entries for each tile, which
    // have stride horz_units_per_tile.
    //
    // Even if there are tiles of different sizes, the data in unit_info is laid
    // out as if all tiles are of full size.
    int32_t                     units_per_tile;
    int32_t                     vert_units_per_tile, horz_units_per_tile;
    RestorationUnitInfo*        unit_info;
    RestorationStripeBoundaries boundaries;
    int32_t                     optimized_lr;
} RestorationInfo;

static INLINE void set_default_sgrproj(SgrprojInfo* sgrproj_info) {
    sgrproj_info->xqd[0] = (SGRPROJ_PRJ_MIN0 + SGRPROJ_PRJ_MAX0) / 2;
    sgrproj_info->xqd[1] = (SGRPROJ_PRJ_MIN1 + SGRPROJ_PRJ_MAX1) / 2;
}

static INLINE void set_default_wiener(WienerInfo* wiener_info) {
    wiener_info->vfilter[0] = wiener_info->hfilter[0] = WIENER_FILT_TAP0_MIDV;
    wiener_info->vfilter[1] = wiener_info->hfilter[1] = WIENER_FILT_TAP1_MIDV;
    wiener_info->vfilter[2] = wiener_info->hfilter[2] = WIENER_FILT_TAP2_MIDV;
    wiener_info->vfilter[WIENER_HALFWIN] = wiener_info->hfilter[WIENER_HALFWIN] = -2 *
        (WIENER_FILT_TAP2_MIDV + WIENER_FILT_TAP1_MIDV + WIENER_FILT_TAP0_MIDV);
    wiener_info->vfilter[4] = wiener_info->hfilter[4] = WIENER_FILT_TAP2_MIDV;
    wiener_info->vfilter[5] = wiener_info->hfilter[5] = WIENER_FILT_TAP1_MIDV;
    wiener_info->vfilter[6] = wiener_info->hfilter[6] = WIENER_FILT_TAP0_MIDV;
}

typedef struct RestorationTileLimits {
    int32_t h_start, h_end, v_start, v_end;
} RestorationTileLimits;

extern const SgrParamsType svt_aom_eb_sgr_params[SGRPROJ_PARAMS];
extern const int32_t       svt_aom_eb_x_by_xplus1[256];
extern const int32_t       svt_aom_eb_one_by_x[MAX_NELEM];

//void svt_av1_alloc_restoration_struct(struct Av1Common *cm, RestorationInfo *rsi,
//                                      int32_t is_uv);
void svt_extend_frame(uint8_t* data, int32_t width, int32_t height, int32_t stride, int32_t border_horz,
                      int32_t border_vert, int32_t highbd);
void svt_decode_xq(const int32_t* xqd, int32_t* xq, const SgrParamsType* params);

// Filter a single loop restoration unit.
//
// limits is the limits of the unit. rui gives the mode to use for this unit
// and its coefficients. If striped loop restoration is enabled, rsb contains
// deblocked pixels to use for stripe boundaries; rlbs is just some space to
// use as a scratch buffer. tile_rect gives the limits of the tile containing
// this unit. tile_stripe0 is the index of the first stripe in this tile.
//
// ss_x and ss_y are flags which should be 1 if this is a plane with
// horizontal/vertical subsampling, respectively. highbd is a flag which should
// be 1 in high bit depth mode, in which case bit_depth is the bit depth.
//
// data8 is the frame data (pointing at the top-left corner of the frame, not
// the restoration unit) and stride is its stride. dst8 is the buffer where the
// results will be written and has stride dst_stride. Like data8, dst8 should
// point at the top-left corner of the frame.
//
// Finally tmpbuf is a scratch buffer used by the sgrproj filter which should
// be at least SGRPROJ_TMPBUF_SIZE big.
void svt_av1_loop_restoration_filter_unit(uint8_t need_bounadaries, const RestorationTileLimits* limits,
                                          const RestorationUnitInfo* rui, const RestorationStripeBoundaries* rsb,
                                          RestorationLineBuffers* rlbs, const Av1PixelRect* tile_rect,
                                          int32_t tile_stripe0, int32_t ss_x, int32_t ss_y, int32_t highbd,
                                          int32_t bit_depth, uint8_t* data8, int32_t stride, uint8_t* dst8,
                                          int32_t dst_stride, int32_t* tmpbuf, int32_t optimized_lr);

// Returns 1 if a superres upscaled frame is unscaled and 0 otherwise.
static INLINE int32_t av1_superres_unscaled(const FrameSize* frm_size) {
    return (frm_size->frame_width == frm_size->superres_upscaled_width);
}

//void svt_av1_loop_restoration_filter_frame(Yv12BufferConfig *frame,
//                                           Av1Common *cm, int32_t optimized_lr);
typedef void (*RestUnitVisitor)(const RestorationTileLimits* limits, const Av1PixelRect* tile_rect,
                                int32_t rest_unit_idx, void* priv);

typedef void (*RestTileStartVisitor)(int32_t tile_row, int32_t tile_col, void* priv);

// Call on_rest_unit for each loop restoration unit in the frame. At the start
// of each tile, call on_tile.
//void svt_aom_foreach_rest_unit_in_frame(Av1Common *cm, int32_t plane,
//                                    RestTileStartVisitor on_tile,
//                                    RestUnitVisitor on_rest_unit,
//                                    void *priv);

// Return 1 iff the block at mi_row, mi_col with size bsize is a
// top-level superblock containing the top-left corner of at least one
// loop restoration unit.
//
// If the block is a top-level superblock, the function writes to
// *rcol0, *rcol1, *rrow0, *rrow1. The rectangle of restoration unit
// indices given by [*rcol0, *rcol1) x [*rrow0, *rrow1) are relative
// to the current tile, whose starting index is returned as
// *tile_tl_idx.
struct Av1Common;
int32_t svt_av1_loop_restoration_corners_in_sb(struct Av1Common* cm, SeqHeader* seq_header_p, int32_t plane,
                                               int32_t mi_row, int32_t mi_col, BlockSize bsize, int32_t* rcol0,
                                               int32_t* rcol1, int32_t* rrow0, int32_t* rrow1, int32_t* tile_tl_idx);

void svt_av1_loop_restoration_save_boundary_lines(const Yv12BufferConfig* frame, struct Av1Common* cm,
                                                  int32_t after_cdef);
void svt_aom_foreach_rest_unit_in_frame(struct Av1Common* cm, int32_t plane, RestTileStartVisitor on_tile,
                                        RestUnitVisitor on_rest_unit, void* priv);
void svt_aom_foreach_rest_unit_in_frame_seg(struct Av1Common* cm, int32_t plane, RestTileStartVisitor on_tile,
                                            RestUnitVisitor on_rest_unit, void* priv,
                                            uint8_t rest_segments_column_count, uint8_t rest_segments_row_count,
                                            uint32_t segment_index);

#define RDDIV_BITS 7
#define RD_EPB_SHIFT 6

#define RDCOST_DBL(RM, R, D) \
    (((((double)(R)) * (RM)) / (double)(1 << AV1_PROB_COST_SHIFT)) + ((double)(D) * (1 << RDDIV_BITS)))

typedef struct RestUnitSearchInfo {
    // The best coefficients for Wiener or Sgrproj restoration
    WienerInfo  wiener;
    SgrprojInfo sgrproj;

    // The sum of squared errors for this rtype.
    int64_t sse[RESTORE_SWITCHABLE_TYPES];

    // The rtype to use for this unit given a frame rtype as
    // index. Indices: WIENER, SGRPROJ, SWITCHABLE.
    RestorationType best_rtype[RESTORE_TYPES - 1];
} RestUnitSearchInfo;

#ifdef __cplusplus
} // extern "C"
#endif

#endif // AV1_COMMON_RESTORATION_H_
