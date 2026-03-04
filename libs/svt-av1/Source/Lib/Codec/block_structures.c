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

#include "block_structures.h"

void svt_av1_tile_set_row(TileInfo* tile, TilesInfo* tiles_info, int32_t mi_rows, int row) {
    assert(row < tiles_info->tile_rows);
    int mi_row_start       = tiles_info->tile_row_start_mi[row];
    int mi_row_end         = tiles_info->tile_row_start_mi[row + 1];
    tile->tile_row         = row;
    tile->mi_row_start     = mi_row_start;
    tile->mi_row_end       = AOMMIN(mi_row_end, mi_rows);
    tile->tg_horz_boundary = 0;
    assert(tile->mi_row_end > tile->mi_row_start);
}

void svt_av1_tile_set_col(TileInfo* tile, const TilesInfo* tiles_info, int32_t mi_cols, int col) {
    assert(col < tiles_info->tile_cols);
    int mi_col_start   = tiles_info->tile_col_start_mi[col];
    int mi_col_end     = tiles_info->tile_col_start_mi[col + 1];
    tile->tile_col     = col;
    tile->mi_col_start = mi_col_start;
    tile->mi_col_end   = AOMMIN(mi_col_end, mi_cols);
    assert(tile->mi_col_end > tile->mi_col_start);
}
