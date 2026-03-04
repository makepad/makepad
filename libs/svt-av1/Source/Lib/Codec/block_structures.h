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

#ifndef EbBlockStructures_h
#define EbBlockStructures_h

#include "definitions.h"
#include "segmentation_params.h"
#include "av1_structs.h"
#include "mv.h"

#ifdef __cplusplus
extern "C" {
#endif

#define MAX_TILE_WIDTH (4096) // Max Tile width in pixels
#define MAX_TILE_AREA (4096 * 2304) // Maximum tile area in pixels

typedef struct TileInfo {
    int32_t mi_row_start, mi_row_end;
    int32_t mi_col_start, mi_col_end;
    int32_t tg_horz_boundary;
    int32_t tile_row;
    int32_t tile_col;
    int32_t tile_rs_index; //tile index in raster order
} TileInfo;

typedef struct BlockModeInfo {
    /*! \brief The prediction mode used */
    PredictionMode mode;
    /*! \brief The UV mode when intra is used */
    UvPredictionMode uv_mode; // Only for INTRA blocks

    /*****************************************************************************
   * \name Inter Mode Info
   ****************************************************************************/
    /**@{*/
    /*! \brief The motion vectors used by the current inter mode. Unipred MV stored
   in idx 0.*/
    Mv mv[2];
    /*! \brief The reference frames for the MV */
    MvReferenceFrame ref_frame[2];
    /*! \brief Filter used in subpel interpolation. */
    uint32_t interp_filters;
    /*! \brief Struct that stores the data used in interinter compound mode. */
    InterInterCompoundData interinter_comp;
    /*! \brief The motion mode used by the inter prediction. */
    MotionMode motion_mode;
    /*! \brief Number of samples used by warp causal */
    uint8_t num_proj_ref;
    /*! \brief The type of intra mode used by inter-intra */
    InterIntraMode interintra_mode;
    /*! \brief The type of wedge used in interintra mode. */
    int8_t interintra_wedge_index;

    /*****************************************************************************
     * \name Intra Mode Info
     ****************************************************************************/
    /**@{*/
    /*! \brief Directional mode delta: the angle is base angle + (angle_delta *
      * step). */
    int8_t angle_delta[PLANE_TYPES];
    /*! \brief The type of filter intra mode used (if applicable). */
    uint8_t filter_intra_mode;
    /*! \brief Chroma from Luma: Joint sign of alpha Cb and alpha Cr */
    uint8_t cfl_alpha_signs;
    /*! \brief Chroma from Luma: Index of the alpha Cb and alpha Cr combination */
    uint8_t cfl_alpha_idx;

    uint8_t tx_depth;
    uint8_t is_interintra_used : 1;
    uint8_t use_wedge_interintra : 1;
    /*! \brief Indicates if masked compound is used(1) or not (0). */
    uint8_t comp_group_idx : 1;
    /*!< 0 indicates that a distance based weighted scheme should be used for blending.
     *   1 indicates that the averaging scheme should be used for blending.*/
    uint8_t compound_idx : 1;
    // possible values: 0,1; skip coeff only. as defined in section 6.10.11 of the av1 text
    uint8_t skip : 1;

    /*!< 1 indicates that this block will use some default settings and skip mode info.
     * 0 indicates that the mode info is not skipped. */
    // possible values: 0,1; skip mode_info + coeff. as defined in section 6.10.10 of the av1 text
    uint8_t skip_mode : 1;
    /*! \brief Whether intrabc is used. */
    uint8_t use_intrabc : 1;
} BlockModeInfo;

typedef struct MbModeInfo {
    BlockModeInfo       block_mi;
    BlockSize           bsize;
    PartitionType       partition;
    uint8_t             segment_id;
    PaletteLumaModeInfo palette_mode_info;
    int8_t              cdef_strength;
} MbModeInfo;

static AOM_INLINE int has_second_ref(const BlockModeInfo* block_mi) {
    return block_mi->ref_frame[1] > INTRA_FRAME;
}

static AOM_INLINE int has_uni_comp_refs(const BlockModeInfo* block_mi) {
    return has_second_ref(block_mi) &&
        (!((block_mi->ref_frame[0] >= BWDREF_FRAME) ^ (block_mi->ref_frame[1] >= BWDREF_FRAME)));
}

static AOM_INLINE int is_intrabc_block(const BlockModeInfo* block_mi) {
    return block_mi->use_intrabc;
}

static AOM_INLINE int is_inter_block(const BlockModeInfo* block_mi) {
    return is_intrabc_block(block_mi) || block_mi->ref_frame[0] > INTRA_FRAME;
}

void svt_av1_tile_set_col(TileInfo* tile, const TilesInfo* tiles_info, int32_t mi_cols, int col);
void svt_av1_tile_set_row(TileInfo* tile, TilesInfo* tiles_info, int32_t mi_rows, int row);

static INLINE int32_t tile_log2(int32_t blk_size, int32_t target) {
    int32_t k;
    for (k = 0; (blk_size << k) < target; k++) {}
    return k;
}

#ifdef __cplusplus
}
#endif
#endif // EbBlockStructures_h
