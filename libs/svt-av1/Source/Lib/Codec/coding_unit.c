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

#include <stdlib.h>
#include <stdint.h>

#include "pcs.h"
#include "utility.h"
#include "enc_mode_config.h"

void svt_aom_largest_coding_unit_dctor(EbPtr p) {
    SuperBlock* obj = (SuperBlock*)p;
    EB_FREE_ARRAY(obj->av1xd);
    EB_FREE_ARRAY(obj->final_blk_arr);
    EB_FREE_ARRAY(obj->ptree);
}

static void setup_ptree(PARTITION_TREE* pc_tree, int index, BlockSize bsize, const int min_sq_size) {
    pc_tree->bsize = bsize;
    pc_tree->index = index;

    // If applicable, add split depths
    const int sq_size = block_size_wide[bsize];
    if (sq_size > min_sq_size) {
        const BlockSize subsize             = get_partition_subsize(bsize, PARTITION_SPLIT);
        const int       sq_subsize          = block_size_wide[subsize];
        int             blocks_per_subdepth = (sq_subsize / min_sq_size) * (sq_subsize / min_sq_size);
        int             blocks_to_skip      = 0;

        for (int i = min_sq_size; i <= sq_subsize; i <<= 1, blocks_per_subdepth >>= 2) {
            blocks_to_skip += blocks_per_subdepth;
        }

        for (int i = 0; i < SUB_PARTITIONS_SPLIT; ++i) {
            pc_tree->sub_tree[i] = pc_tree + i * blocks_to_skip + 1;
            setup_ptree(pc_tree->sub_tree[i], i, subsize, min_sq_size);
        }
    }
}

/*
Tasks & Questions
    -Need a GetEmptyChain function for testing sub partitions.  Tie it to an Itr?
    -How many empty CUs do we need?  We need to have enough for the max CU count AND
       enough for temp storage when calculating a subdivision.
    -Where do we store temp reconstructed picture data while deciding to subdivide or not?
    -Need a ReconPicture for each candidate.
    -I don't see a way around doing the copies in temp memory and then copying it in...
*/
#if TUNE_STILL_IMAGE
EbErrorType svt_aom_largest_coding_unit_ctor(SuperBlock* larget_coding_unit_ptr, uint8_t sb_size_pix,
                                             uint16_t sb_origin_x, uint16_t sb_origin_y, uint16_t sb_index,
                                             EncMode enc_mode, bool rtc, bool allintra, PictureControlSet* pcs) {
#else
EbErrorType svt_aom_largest_coding_unit_ctor(SuperBlock* larget_coding_unit_ptr, uint8_t sb_size_pix,
                                             uint16_t sb_origin_x, uint16_t sb_origin_y, uint16_t sb_index,
                                             EncMode enc_mode, bool rtc, bool allintra,
                                             ResolutionRange input_resolution, PictureControlSet* picture_control_set) {
#endif
    larget_coding_unit_ptr->dctor = svt_aom_largest_coding_unit_dctor;

    // ************ SB ***************
    // Which borderLargestCuSize is not a power of two

    // Which borderLargestCuSize is not a power of two
#if TUNE_STILL_IMAGE
    larget_coding_unit_ptr->pcs = pcs;
#else
    larget_coding_unit_ptr->pcs = picture_control_set;
#endif

    larget_coding_unit_ptr->org_x = sb_origin_x;
    larget_coding_unit_ptr->org_y = sb_origin_y;

    larget_coding_unit_ptr->index = sb_index;
    bool disallow_sub_8x8_nsq     = true;
    bool disallow_sub_16x16_nsq   = true;
    for (uint8_t coeff_lvl = 0; coeff_lvl <= HIGH_LVL + 1; coeff_lvl++) {
#if TUNE_STILL_IMAGE
        uint8_t nsq_geom_lvl = allintra ? svt_aom_get_nsq_geom_level_allintra(enc_mode)
            : rtc                       ? svt_aom_get_nsq_geom_level_rtc(enc_mode)
                                        : svt_aom_get_nsq_geom_level_default(enc_mode, coeff_lvl);
#else
        const uint8_t nsq_geom_lvl = svt_aom_get_nsq_geom_level(allintra, input_resolution, enc_mode, coeff_lvl, rtc);
#endif
        // nsq_geom_lvl level 0 means NSQ shapes are disallowed so don't adjust based on the level
        if (nsq_geom_lvl) {
            uint8_t allow_HVA_HVB, allow_HV4, min_nsq_bsize;
            svt_aom_set_nsq_geom_ctrls(NULL, nsq_geom_lvl, &allow_HVA_HVB, &allow_HV4, &min_nsq_bsize);
            if (min_nsq_bsize < 8) {
                disallow_sub_8x8_nsq = false;
            }
            if (min_nsq_bsize < 16) {
                disallow_sub_16x16_nsq = false;
            }
        }
    }
#if TUNE_STILL_IMAGE
    bool disallow_4x4 = allintra ? svt_aom_get_disallow_4x4_allintra(enc_mode)
        : rtc                    ? svt_aom_get_disallow_4x4_rtc(enc_mode)
                                 : svt_aom_get_disallow_4x4_default(enc_mode);
    bool disallow_8x8 = allintra ? svt_aom_get_disallow_8x8_allintra()
        : rtc                    ? svt_aom_get_disallow_8x8_rtc(enc_mode, pcs->frame_width, pcs->frame_height)
                                 : svt_aom_get_disallow_8x8_default();
#else
    bool disallow_4x4 = svt_aom_get_disallow_4x4(enc_mode);
    bool disallow_8x8 = svt_aom_get_disallow_8x8(
        enc_mode, allintra, rtc, picture_control_set->frame_width, picture_control_set->frame_height);
#endif
    uint32_t tot_blk_num;
    if (sb_size_pix == 128) {
        if (disallow_8x8 && disallow_sub_16x16_nsq) {
            tot_blk_num = 64;
        } else if (disallow_8x8 || (disallow_4x4 && disallow_sub_8x8_nsq)) {
            tot_blk_num = 256;
        } else if (disallow_4x4) {
            tot_blk_num = 512;
        } else {
            tot_blk_num = 1024;
        }
    } else if (disallow_8x8 && disallow_sub_16x16_nsq) {
        tot_blk_num = 16;
    } else if (disallow_8x8 || (disallow_4x4 && disallow_sub_8x8_nsq)) {
        tot_blk_num = 64;
    } else if (disallow_4x4) {
        tot_blk_num = 128;
    } else {
        tot_blk_num = 256;
    }
    // Do NOT initialize the final_blk_arr here
    // Malloc maximum but only initialize it only when actually used.
    // This will help to same actually memory usage
    EB_MALLOC_ARRAY(larget_coding_unit_ptr->final_blk_arr, tot_blk_num);
    EB_MALLOC_ARRAY(larget_coding_unit_ptr->av1xd, 1);

    // Alloc ptree, which is used to store final block data/mode info for the SB that is passed
    // from encdec to EC
    uint8_t min_bsize        = disallow_8x8 ? 16 : disallow_4x4 ? 8 : 4;
    int     blocks_per_depth = (sb_size_pix / min_bsize) * (sb_size_pix / min_bsize);
    int     blocks_to_alloc  = 0;

    for (int i = min_bsize; i <= sb_size_pix; i <<= 1, blocks_per_depth >>= 2) {
        blocks_to_alloc += blocks_per_depth;
    }
    EB_CALLOC_ARRAY(larget_coding_unit_ptr->ptree, blocks_to_alloc);
    setup_ptree(larget_coding_unit_ptr->ptree, 0, sb_size_pix == 128 ? BLOCK_128X128 : BLOCK_64X64, min_bsize);

    return EB_ErrorNone;
}
