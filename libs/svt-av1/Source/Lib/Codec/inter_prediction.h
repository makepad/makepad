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

#ifndef EbInterPrediction_h
#define EbInterPrediction_h

#include "block_structures.h"
#include "av1_structs.h"
#include "object.h"
#include "filter.h"
#include "convolve.h"
#include "cabac_context_model.h"
#include "common_utils.h"

#ifdef __cplusplus
extern "C" {
#endif

#define REF_SCALE_SHIFT 14
#define REF_NO_SCALE (1 << REF_SCALE_SHIFT)
#define REF_INVALID_SCALE -1

#define MV_BORDER (16 << 3) // Allow 16 pels in 1/8th pel units

#define NELEMENTS(x) (int)(sizeof(x) / sizeof(x[0]))

#define INTRABC_DELAY_PIXELS 256 //  Delay of 256 pixels
#define INTRABC_DELAY_SB64 (INTRABC_DELAY_PIXELS / 64)

// HW does not support < 4x4 prediction. To limit the bandwidth requirement, if
// block-size of current plane is smaller than 8x8, always only blend with the
// left neighbor(s) (skip blending with the above side).
#define DISABLE_CHROMA_U8X8_OBMC 0 // 0: one-sided obmc; 1: disable

extern DECLARE_ALIGNED(256, const InterpKernel, sub_pel_filters_8[SUBPEL_SHIFTS]);
extern DECLARE_ALIGNED(256, const InterpKernel, sub_pel_filters_4[SUBPEL_SHIFTS]);
extern DECLARE_ALIGNED(256, const InterpKernel, sub_pel_filters_8sharp[SUBPEL_SHIFTS]);
extern DECLARE_ALIGNED(256, const InterpKernel, sub_pel_filters_8smooth[SUBPEL_SHIFTS]);
extern DECLARE_ALIGNED(256, const InterpKernel, bilinear_filters[SUBPEL_SHIFTS]);
extern DECLARE_ALIGNED(256, const InterpKernel, sub_pel_filters_4smooth[SUBPEL_SHIFTS]);

typedef struct SubpelParams {
    int32_t xs;
    int32_t ys;
    int32_t subpel_x;
    int32_t subpel_y;
} SubpelParams;

typedef struct PadBlock {
    int x0;
    int x1;
    int y0;
    int y1;
} PadBlock;

#define INTERINTRA_WEDGE_SIGN 0
#define MAX_INTERINTRA_SB_SQUARE 32 * 32

typedef uint8_t* WedgeMasksType[MAX_WEDGE_TYPES];

// Angles are with respect to horizontal anti-clockwise
typedef enum WedgeDirectionType {
    WEDGE_HORIZONTAL = 0,
    WEDGE_VERTICAL   = 1,
    WEDGE_OBLIQUE27  = 2,
    WEDGE_OBLIQUE63  = 3,
    WEDGE_OBLIQUE117 = 4,
    WEDGE_OBLIQUE153 = 5,
    WEDGE_DIRECTIONS
} WedgeDirectionType;

static const InterpFilterParams av1_interp_filter_params_list[SWITCHABLE_FILTERS + 1] = {
    {(const int16_t*)sub_pel_filters_8, SUBPEL_TAPS, SUBPEL_SHIFTS, EIGHTTAP_REGULAR},
    {(const int16_t*)sub_pel_filters_8smooth, SUBPEL_TAPS, SUBPEL_SHIFTS, EIGHTTAP_SMOOTH},
    {(const int16_t*)sub_pel_filters_8sharp, SUBPEL_TAPS, SUBPEL_SHIFTS, MULTITAP_SHARP},
    {(const int16_t*)bilinear_filters, SUBPEL_TAPS, SUBPEL_SHIFTS, BILINEAR}};

// 3-tuple: {direction, x_offset, y_offset}
typedef struct WedgeCodeType {
    WedgeDirectionType direction;
    int32_t            x_offset;
    int32_t            y_offset;
} WedgeCodeType;

typedef struct WedgeParamsType {
    int32_t              bits;
    const WedgeCodeType* codebook;
    uint8_t*             signflip;
    WedgeMasksType*      masks;
} WedgeParamsType;

void svt_inter_predictor_light_pd0(const uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride, int32_t w,
                                   int32_t h, SubpelParams* subpel_params, ConvolveParams* conv_params);
void svt_inter_predictor_light_pd1(uint8_t* src, uint8_t* src_2b, int32_t src_stride, uint8_t* dst, int32_t dst_stride,
                                   int32_t w, int32_t h, InterpFilters interp_filters, SubpelParams* subpel_params,
                                   ConvolveParams* conv_params, int32_t bd);
void svt_inter_predictor(const uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride,
                         const SubpelParams* subpel_params, const ScaleFactors* sf, int32_t w, int32_t h,
                         ConvolveParams* conv_params, InterpFilters interp_filters, int32_t is_intrabc);

void svt_highbd_inter_predictor(const uint16_t* src, int32_t src_stride, uint16_t* dst, int32_t dst_stride,
                                const SubpelParams* subpel_params, const ScaleFactors* sf, int32_t w, int32_t h,
                                ConvolveParams* conv_params, InterpFilters interp_filters, int32_t is_intrabc,
                                int32_t bd);

void svt_av1_dist_wtd_comp_weight_assign(SeqHeader* seq_header, int cur_frame_index, int bck_frame_index,
                                         int fwd_frame_index, int compound_idx, int order_idx, int* fwd_offset,
                                         int* bck_offset, int* use_dist_wtd_comp_avg, int is_compound);

void svt_aom_build_masked_compound_no_round(uint8_t* dst, int dst_stride, const CONV_BUF_TYPE* src0, int src0_stride,
                                            const CONV_BUF_TYPE* src1, int src1_stride,
                                            const InterInterCompoundData* const comp_data, uint8_t* seg_mask,
                                            BlockSize bsize, int h, int w, ConvolveParams* conv_params, uint8_t bd,
                                            bool is_16bit);

static const InterpFilterParams av1_interp_4tap[2] = {
    {(const int16_t*)sub_pel_filters_4, SUBPEL_TAPS, SUBPEL_SHIFTS, EIGHTTAP_REGULAR},
    {(const int16_t*)sub_pel_filters_4smooth, SUBPEL_TAPS, SUBPEL_SHIFTS, EIGHTTAP_SMOOTH}};

static INLINE InterpFilterParams av1_get_interp_filter_params_with_block_size(const InterpFilter interp_filter,
                                                                              const int32_t      w) {
    if (w <= 4 && (interp_filter == MULTITAP_SHARP || interp_filter == EIGHTTAP_REGULAR)) {
        return av1_interp_4tap[0];
    } else if (w <= 4 && interp_filter == EIGHTTAP_SMOOTH) {
        return av1_interp_4tap[1];
    }

    return av1_interp_filter_params_list[interp_filter];
}

static INLINE void av1_get_convolve_filter_params(uint32_t interp_filters, InterpFilterParams* params_x,
                                                  InterpFilterParams* params_y, int32_t w, int32_t h) {
    InterpFilter filter_x = av1_extract_interp_filter(interp_filters, 1);
    InterpFilter filter_y = av1_extract_interp_filter(interp_filters, 0);
    *params_x             = av1_get_interp_filter_params_with_block_size(filter_x, w);
    *params_y             = av1_get_interp_filter_params_with_block_size(filter_y, h);
};

/* Mapping of interintra to intra mode for use in the intra component */
static const PredictionMode interintra_to_intra_mode[INTERINTRA_MODES] = {DC_PRED, V_PRED, H_PRED, SMOOTH_PRED};

void svt_aom_combine_interintra(InterIntraMode mode, int8_t use_wedge_interintra, int wedge_index, int wedge_sign,
                                BlockSize bsize, BlockSize plane_bsize, uint8_t* comppred, int compstride,
                                const uint8_t* interpred, int interstride, const uint8_t* intrapred, int intrastride);

void svt_aom_combine_interintra_highbd(InterIntraMode mode, uint8_t use_wedge_interintra, uint8_t wedge_index,
                                       uint8_t wedge_sign, BlockSize bsize, BlockSize plane_bsize, uint8_t* comppred8,
                                       int compstride, const uint8_t* interpred8, int interstride,
                                       const uint8_t* intrapred8, int intrastride, int bd);

void svt_av1_setup_scale_factors_for_frame(ScaleFactors* sf, int other_w, int other_h, int this_w, int this_h);

static INLINE int av1_is_valid_scale(const struct ScaleFactors* sf) {
    return sf->x_scale_fp != REF_INVALID_SCALE && sf->y_scale_fp != REF_INVALID_SCALE;
}

static INLINE int av1_is_scaled(const struct ScaleFactors* sf) {
    return av1_is_valid_scale(sf) && (sf->x_scale_fp != REF_NO_SCALE || sf->y_scale_fp != REF_NO_SCALE);
}

static INLINE int valid_ref_frame_size(int ref_width, int ref_height, int this_width, int this_height) {
    return 2 * this_width >= ref_width && 2 * this_height >= ref_height && this_width <= 16 * ref_width &&
        this_height <= 16 * ref_height;
}

void svt_aom_pack_block(uint8_t* in8_bit_buffer, uint32_t in8_stride, uint8_t* inn_bit_buffer, uint32_t inn_stride,
                        uint16_t* out16_bit_buffer, uint32_t out_stride, uint32_t width, uint32_t height);

void highbd_convolve_2d_for_intrabc(const uint16_t* src, int src_stride, uint16_t* dst, int dst_stride, int w, int h,
                                    int subpel_x_q4, int subpel_y_q4, ConvolveParams* conv_params, int bd);

void convolve_2d_for_intrabc(const uint8_t* src, int src_stride, uint8_t* dst, int dst_stride, int w, int h,
                             int subpel_x_q4, int subpel_y_q4, ConvolveParams* conv_params);

extern aom_highbd_convolve_fn_t convolve_hbd[/*sub_x*/ 2][/*sub_y*/ 2][/*bi*/ 2];

extern AomConvolveFn svt_aom_convolve[/*sub_x*/ 2][/*sub_y*/ 2][/*bi*/ 2];

int svt_aom_is_interintra_wedge_used(BlockSize bsize);

int32_t svt_aom_get_wedge_bits_lookup(BlockSize bsize);

const uint8_t* svt_aom_get_contiguous_soft_mask(int wedge_index, int wedge_sign, BlockSize bsize);

int svt_aom_get_wedge_params_bits(BlockSize bsize);

int svt_aom_is_masked_compound_type(COMPOUND_TYPE type);

// Although we assign 32 bit integers, all the values are strictly under 14
// bits.
static int div_mult[32] = {0,    16384, 8192, 5461, 4096, 3276, 2730, 2340, 2048, 1820, 1638,
                           1489, 1365,  1260, 1170, 1092, 1024, 963,  910,  862,  819,  780,
                           744,  712,   682,  655,  630,  606,  585,  564,  546,  528};

static INLINE void integer_mv_precision(Mv* mv) {
    int mod = (mv->y % 8);
    if (mod != 0) {
        mv->y -= mod;
        if (abs(mod) > 4) {
            if (mod > 0) {
                mv->y += 8;
            } else {
                mv->y -= 8;
            }
        }
    }

    mod = (mv->x % 8);
    if (mod != 0) {
        mv->x -= mod;
        if (abs(mod) > 4) {
            if (mod > 0) {
                mv->x += 8;
            } else {
                mv->x -= 8;
            }
        }
    }
}

static INLINE void lower_mv_precision(Mv* mv, int allow_hp, int is_integer) {
    if (is_integer) {
        integer_mv_precision(mv);
    } else {
        if (!allow_hp) {
            if (mv->y & 1) {
                mv->y += (mv->y > 0 ? -1 : 1);
            }
            if (mv->x & 1) {
                mv->x += (mv->x > 0 ? -1 : 1);
            }
        }
    }
}

static INLINE void get_mv_projection(Mv* output, Mv ref, int num, int den) {
    den                 = AOMMIN(den, MAX_FRAME_DISTANCE);
    num                 = num > 0 ? AOMMIN(num, MAX_FRAME_DISTANCE) : AOMMAX(num, -MAX_FRAME_DISTANCE);
    const int mv_row    = ROUND_POWER_OF_TWO_SIGNED(ref.y * num * div_mult[den], 14);
    const int mv_col    = ROUND_POWER_OF_TWO_SIGNED(ref.x * num * div_mult[den], 14);
    const int clamp_max = MV_UPP - 1;
    const int clamp_min = MV_LOW + 1;
    output->y           = (int16_t)clamp(mv_row, clamp_min, clamp_max);
    output->x           = (int16_t)clamp(mv_col, clamp_min, clamp_max);
}

static INLINE int check_sb_border(const int mi_row, const int mi_col, const int row_offset, const int col_offset) {
    const int sb_mi_size = mi_size_wide[BLOCK_64X64];
    const int row        = mi_row & (sb_mi_size - 1);
    const int col        = mi_col & (sb_mi_size - 1);

    if (row + row_offset < 0 || row + row_offset >= sb_mi_size || col + col_offset < 0 ||
        col + col_offset >= sb_mi_size) {
        return 0;
    }

    return 1;
}

// AV1 spec 7.10.3
// overlappable neighbours not relevant for intra frames (i.e. where intra_bc is allowed)
// because no warp/obmc in intra frames.
static INLINE int is_neighbor_overlappable(const MbModeInfo* mbmi) {
    return mbmi->block_mi.ref_frame[0] > INTRA_FRAME;
}

static INLINE int32_t is_mv_valid(const Mv* mv) {
    return mv->y > MV_LOW && mv->y < MV_UPP && mv->x > MV_LOW && mv->x < MV_UPP;
}

#define CHECK_BACKWARD_REFS(ref_frame) (((ref_frame) >= BWDREF_FRAME) && ((ref_frame) <= ALTREF_FRAME))
#define IS_BACKWARD_REF_FRAME(ref_frame) CHECK_BACKWARD_REFS(ref_frame)

void svt_aom_find_ref_dv(Mv* ref_dv, const TileInfo* const tile, int mib_size, int mi_row, int mi_col);

static INLINE int32_t is_comp_ref_allowed(BlockSize bsize) {
    return AOMMIN(block_size_wide[bsize], block_size_high[bsize]) >= 8;
}

static INLINE int is_interinter_compound_used(CompoundType type, BlockSize bsize) {
    const int comp_allowed = is_comp_ref_allowed(bsize);
    switch (type) {
    case COMPOUND_AVERAGE:
    case COMPOUND_DISTWTD:
    case COMPOUND_DIFFWTD:
        return comp_allowed;
    case COMPOUND_WEDGE:
        return comp_allowed && svt_aom_get_wedge_params_bits(bsize) > 0;
    default:
        assert(0);
        return 0;
    }
}

static INLINE int is_any_masked_compound_used(BlockSize bsize) {
    CompoundType comp_type;
    int          i;
    if (!is_comp_ref_allowed(bsize)) {
        return 0;
    }
    for (i = 0; i < COMPOUND_TYPES; i++) {
        comp_type = (CompoundType)i;
        if (svt_aom_is_masked_compound_type(comp_type) && is_interinter_compound_used(comp_type, bsize)) {
            return 1;
        }
    }
    return 0;
}

static INLINE int bsize_to_tx_size_cat(BlockSize bsize) {
    TxSize tx_size = blocksize_to_txsize[bsize];
    assert(tx_size != TX_4X4);
    int depth = 0;
    while (tx_size != TX_4X4) {
        depth++;
        tx_size = eb_sub_tx_size_map[tx_size];
        assert(depth < 10);
    }
    assert(depth <= MAX_TX_CATS);
    return depth - 1;
}

static INLINE int bsize_to_max_depth(BlockSize bsize) {
    TxSize tx_size = blocksize_to_txsize[bsize];
    int    depth   = 0;
    while (depth < MAX_TX_DEPTH && tx_size != TX_4X4) {
        depth++;
        tx_size = eb_sub_tx_size_map[tx_size];
    }
    return depth;
}

static INLINE PredictionMode compound_ref0_mode(PredictionMode mode) {
    static PredictionMode lut[] = {
        MB_MODE_COUNT, // DC_PRED
        MB_MODE_COUNT, // V_PRED
        MB_MODE_COUNT, // H_PRED
        MB_MODE_COUNT, // D45_PRED
        MB_MODE_COUNT, // D135_PRED
        MB_MODE_COUNT, // D113_PRED
        MB_MODE_COUNT, // D157_PRED
        MB_MODE_COUNT, // D203_PRED
        MB_MODE_COUNT, // D67_PRED
        MB_MODE_COUNT, // SMOOTH_PRED
        MB_MODE_COUNT, // SMOOTH_V_PRED
        MB_MODE_COUNT, // SMOOTH_H_PRED
        MB_MODE_COUNT, // PAETH_PRED
        MB_MODE_COUNT, // NEARESTMV
        MB_MODE_COUNT, // NEARMV
        MB_MODE_COUNT, // GLOBALMV
        MB_MODE_COUNT, // NEWMV
        NEARESTMV, // NEAREST_NEARESTMV
        NEARMV, // NEAR_NEARMV
        NEARESTMV, // NEAREST_NEWMV
        NEWMV, // NEW_NEARESTMV
        NEARMV, // NEAR_NEWMV
        NEWMV, // NEW_NEARMV
        GLOBALMV, // GLOBAL_GLOBALMV
        NEWMV, // NEW_NEWMV
    };
    assert(NELEMENTS(lut) == MB_MODE_COUNT);
    assert(is_inter_compound_mode(mode));
    return lut[mode];
}

static INLINE PredictionMode compound_ref1_mode(PredictionMode mode) {
    static PredictionMode lut[] = {
        MB_MODE_COUNT, // DC_PRED
        MB_MODE_COUNT, // V_PRED
        MB_MODE_COUNT, // H_PRED
        MB_MODE_COUNT, // D45_PRED
        MB_MODE_COUNT, // D135_PRED
        MB_MODE_COUNT, // D113_PRED
        MB_MODE_COUNT, // D157_PRED
        MB_MODE_COUNT, // D203_PRED
        MB_MODE_COUNT, // D67_PRED
        MB_MODE_COUNT, // SMOOTH_PRED
        MB_MODE_COUNT, // SMOOTH_V_PRED
        MB_MODE_COUNT, // SMOOTH_H_PRED
        MB_MODE_COUNT, // PAETH_PRED
        MB_MODE_COUNT, // NEARESTMV
        MB_MODE_COUNT, // NEARMV
        MB_MODE_COUNT, // GLOBALMV
        MB_MODE_COUNT, // NEWMV
        NEARESTMV, // NEAREST_NEARESTMV
        NEARMV, // NEAR_NEARMV
        NEWMV, // NEAREST_NEWMV
        NEARESTMV, // NEW_NEARESTMV
        NEWMV, // NEAR_NEWMV
        NEARMV, // NEW_NEARMV
        GLOBALMV, // GLOBAL_GLOBALMV
        NEWMV, // NEW_NEWMV
    };
    assert(NELEMENTS(lut) == MB_MODE_COUNT);
    assert(is_inter_compound_mode(mode));
    return lut[mode];
}

static INLINE bool is_motion_variation_allowed_bsize(const BlockSize bsize) {
    return (block_size_wide[bsize] >= 8 && block_size_high[bsize] >= 8);
}

static INLINE int is_global_mv_block(const PredictionMode mode, const BlockSize bsize, TransformationType type) {
    return (mode == GLOBALMV || mode == GLOBAL_GLOBALMV) && type > TRANSLATION &&
        is_motion_variation_allowed_bsize(bsize);
}

static INLINE uint32_t have_nearmv_in_inter_mode(PredictionMode mode) {
    return (mode == NEARMV || mode == NEAR_NEARMV || mode == NEAR_NEWMV || mode == NEW_NEARMV);
}

#define n_elements(x) (int32_t)(sizeof(x) / sizeof(x[0]))

static INLINE MvReferenceFrame comp_ref0(int32_t ref_idx) {
    static const MvReferenceFrame lut[] = {
        LAST_FRAME, // LAST_LAST2_FRAMES,
        LAST_FRAME, // LAST_LAST3_FRAMES,
        LAST_FRAME, // LAST_GOLDEN_FRAMES,
        BWDREF_FRAME, // BWDREF_ALTREF_FRAMES,
        LAST2_FRAME, // LAST2_LAST3_FRAMES
        LAST2_FRAME, // LAST2_GOLDEN_FRAMES,
        LAST3_FRAME, // LAST3_GOLDEN_FRAMES,
        BWDREF_FRAME, // BWDREF_ALTREF2_FRAMES,
        ALTREF2_FRAME, // ALTREF2_ALTREF_FRAMES,
    };
    assert(n_elements(lut) == TOTAL_UNIDIR_COMP_REFS);
    return lut[ref_idx];
}

static INLINE MvReferenceFrame comp_ref1(int32_t ref_idx) {
    static const MvReferenceFrame lut[] = {
        LAST2_FRAME, // LAST_LAST2_FRAMES,
        LAST3_FRAME, // LAST_LAST3_FRAMES,
        GOLDEN_FRAME, // LAST_GOLDEN_FRAMES,
        ALTREF_FRAME, // BWDREF_ALTREF_FRAMES,
        LAST3_FRAME, // LAST2_LAST3_FRAMES
        GOLDEN_FRAME, // LAST2_GOLDEN_FRAMES,
        GOLDEN_FRAME, // LAST3_GOLDEN_FRAMES,
        ALTREF2_FRAME, // BWDREF_ALTREF2_FRAMES,
        ALTREF_FRAME, // ALTREF2_ALTREF_FRAMES,
    };
    assert(n_elements(lut) == TOTAL_UNIDIR_COMP_REFS);
    return lut[ref_idx];
}

static INLINE int8_t get_uni_comp_ref_idx(const MvReferenceFrame* const rf) {
    // Single ref pred
    if (rf[1] <= INTRA_FRAME) {
        return -1;
    }

    // Bi-directional comp ref pred
    if ((rf[0] < BWDREF_FRAME) && (rf[1] >= BWDREF_FRAME)) {
        return -1;
    }

    for (int8_t ref_idx = 0; ref_idx < TOTAL_UNIDIR_COMP_REFS; ++ref_idx) {
        if (rf[0] == comp_ref0(ref_idx) && rf[1] == comp_ref1(ref_idx)) {
            return ref_idx;
        }
    }
    return -1;
}

static INLINE int8_t av1_ref_frame_type(const MvReferenceFrame* const rf) {
    if (rf[1] > INTRA_FRAME) {
        const int8_t uni_comp_ref_idx = get_uni_comp_ref_idx(rf);
        if (uni_comp_ref_idx >= 0) {
            assert((TOTAL_REFS_PER_FRAME + FWD_REFS * BWD_REFS + uni_comp_ref_idx) < MODE_CTX_REF_FRAMES);
            return TOTAL_REFS_PER_FRAME + FWD_REFS * BWD_REFS + uni_comp_ref_idx;
        } else {
            return TOTAL_REFS_PER_FRAME + FWD_RF_OFFSET(rf[0]) + BWD_RF_OFFSET(rf[1]) * FWD_REFS;
        }
    }

    return rf[0];
}

static MvReferenceFrame ref_frame_map[TOTAL_COMP_REFS][2] = {
    {LAST_FRAME, BWDREF_FRAME},
    {LAST2_FRAME, BWDREF_FRAME},
    {LAST3_FRAME, BWDREF_FRAME},
    {GOLDEN_FRAME, BWDREF_FRAME},
    {LAST_FRAME, ALTREF2_FRAME},
    {LAST2_FRAME, ALTREF2_FRAME},
    {LAST3_FRAME, ALTREF2_FRAME},
    {GOLDEN_FRAME, ALTREF2_FRAME},
    {LAST_FRAME, ALTREF_FRAME},
    {LAST2_FRAME, ALTREF_FRAME},
    {LAST3_FRAME, ALTREF_FRAME},
    {GOLDEN_FRAME, ALTREF_FRAME},
    {LAST_FRAME, LAST2_FRAME},
    {LAST_FRAME, LAST3_FRAME},
    {LAST_FRAME, GOLDEN_FRAME},
    {BWDREF_FRAME, ALTREF_FRAME},
    // NOTE: Following reference frame pairs are not supported to be explicitly
    //       signalled, but they are possibly chosen by the use of skip_mode,
    //       which may use the most recent one-sided reference frame pair.
    {LAST2_FRAME, LAST3_FRAME},
    {LAST2_FRAME, GOLDEN_FRAME},
    {LAST3_FRAME, GOLDEN_FRAME},
    {BWDREF_FRAME, ALTREF2_FRAME},
    {ALTREF2_FRAME, ALTREF_FRAME}};

static INLINE void av1_set_ref_frame(MvReferenceFrame* rf, int8_t ref_frame_type) {
    if (ref_frame_type >= TOTAL_REFS_PER_FRAME) {
        rf[0] = ref_frame_map[ref_frame_type - TOTAL_REFS_PER_FRAME][0];
        rf[1] = ref_frame_map[ref_frame_type - TOTAL_REFS_PER_FRAME][1];
    } else {
        rf[0] = ref_frame_type;
        rf[1] = NONE_FRAME;
        // assert(ref_frame_type > NONE_FRAME); AMIR
    }
}

/*
      |----------------------------------------------------------------|
      | ref_idx          0            1           2            3       |
      | List0            LAST        LAST2        LAST3        GOLD    |
      | List1            BWD         ALT2         ALT                  |
      |----------------------------------------------------------------|
*/
static uint8_t ref_type_to_list_idx[REFS_PER_FRAME + 1] = {0, 0, 0, 0, 0, 1, 1, 1};

static INLINE uint8_t get_list_idx(uint8_t ref_type) {
    return ref_type_to_list_idx[ref_type];
}

static uint8_t ref_type_to_ref_idx[REFS_PER_FRAME + 1] = {0, 0, 1, 2, 3, 0, 1, 2};

static INLINE uint8_t get_ref_frame_idx(uint8_t ref_type) {
    return ref_type_to_ref_idx[ref_type];
};
#if CONFIG_ENABLE_OBMC
int svt_av1_skip_u4x4_pred_in_obmc(BlockSize bsize, int dir, int subsampling_x, int subsampling_y);
#endif
int     svt_aom_get_relative_dist_enc(SeqHeader* seq_header, int ref_hint, int order_hint);
int16_t svt_aom_mode_context_analyzer(int16_t mode_context, const MvReferenceFrame* const rf);

#ifdef __cplusplus
}
#endif
#endif //EbInterPrediction_h
