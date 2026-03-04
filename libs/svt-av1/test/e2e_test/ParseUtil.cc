/*
 * Copyright(c) 2019 Netflix, Inc.
 * Copyright (c) 2016, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the
 * Alliance for Open Media Patent License 1.0 was not distributed with this
 * source code in the PATENTS file, you can obtain it at
 * https://www.aomedia.org/license/patent-license.
 */

/******************************************************************************
 * @file ParseUtil.cc
 *
 * @brief Impelmentation of sequence header parser.
 *
 ******************************************************************************/

#include "definitions.h"
#include "av1_structs.h"
#include "dec_bitstream.h"
#include "ParseUtil.h"
#include "gtest/gtest.h"

#define CONFIG_MAX_DECODE_PROFILE 2
#define SELECT_SCREEN_CONTENT_TOOLS 2
#define SELECT_INTEGER_MV 2

// Returns 1 when OBU type is valid, and 0 otherwise.
static int is_valid_obu_type(int obu_type) {
    int valid_type = 0;
    switch (obu_type) {
    case OBU_SEQUENCE_HEADER:
    case OBU_TEMPORAL_DELIMITER:
    case OBU_FRAME_HEADER:
    case OBU_TILE_GROUP:
    case OBU_METADATA:
    case OBU_FRAME:
    case OBU_REDUNDANT_FRAME_HEADER:
        // case OBU_TILE_LIST:
    case OBU_PADDING: valid_type = 1; break;
    default: break;
    }
    return valid_type;
}

// Read OBU header
static EbErrorType read_obu_header(Bitstrm *bs, ObuHeader *header) {
    if (!bs || !header)
        return EB_ErrorBadParameter;

    header->size = 1;

    if (svt_aom_dec_get_bits(bs, 1) != 0) {
        // obu_forbidden_bit must be set to 0.
        return EB_Corrupt_Frame;
    }
    header->obu_type = (ObuType)svt_aom_dec_get_bits(bs, 4);
    if (!is_valid_obu_type(header->obu_type))
        return EB_Corrupt_Frame;

    header->obu_extension_flag = svt_aom_dec_get_bits(bs, 1);
    header->obu_has_size_field = svt_aom_dec_get_bits(bs, 1);

    if (svt_aom_dec_get_bits(bs, 1) != 0) {
        // obu_reserved_1bit must be set to 0
        return EB_Corrupt_Frame;
    }

    if (header->obu_extension_flag) {
        header->size += 1;

        header->temporal_id = svt_aom_dec_get_bits(bs, 3);
        header->spatial_id = svt_aom_dec_get_bits(bs, 2);

        if (svt_aom_dec_get_bits(bs, 3) != 0) {
            // extension_header_reserved_3bits must be set to 0
            return EB_Corrupt_Frame;
        }
    } else
        header->temporal_id = header->spatial_id = 0;

    return EB_ErrorNone;
}

// Read OBU size
static EbErrorType read_obu_size(Bitstrm *bs, size_t bytes_available,
                                 size_t *const obu_size,
                                 size_t *const length_field_size) {
    size_t u_obu_size = 0;
    svt_aom_dec_get_bits_leb128(
        bs, bytes_available, &u_obu_size, length_field_size);

    if (u_obu_size > UINT32_MAX)
        return EB_Corrupt_Frame;
    *obu_size = u_obu_size;
    return EB_ErrorNone;
}

/** Reads OBU header and size */
static EbErrorType read_obu_header_size(Bitstrm *bs, ObuHeader *header,
                                        size_t size,
                                        size_t *const length_size) {
    EbErrorType status;

    status = read_obu_header(bs, header);
    if (status != EB_ErrorNone)
        return status;

    if (header->obu_has_size_field) {
        status = read_obu_size(bs, size, &header->payload_size, length_size);
        if (status != EB_ErrorNone)
            return status;
    }

    return EB_ErrorNone;
}

// Read Timing information
static void read_timing_info(Bitstrm *bs, EbTimingInfo *timing_info) {
    timing_info->num_units_in_display_tick = svt_aom_dec_get_bits(bs, 32);
    timing_info->time_scale = svt_aom_dec_get_bits(bs, 32);
    if (!timing_info->num_units_in_display_tick || !timing_info->time_scale)
        return;  // EB_DecUnsupportedBitstream;
    timing_info->equal_picture_interval = svt_aom_dec_get_bits(bs, 1);
    if (timing_info->equal_picture_interval) {
        timing_info->num_ticks_per_picture = svt_aom_dec_get_bits_uvlc(bs) + 1;
        if ((timing_info->num_ticks_per_picture) == UINT32_MAX)
            return;  // EB_DecUnsupportedBitstream;
    }
}

// Read Decoder model information
static void read_decoder_model_info(Bitstrm *bs, DecoderModelInfo *model_info) {
    model_info->buffer_delay_length_minus_1 = svt_aom_dec_get_bits(bs, 5);
    model_info->num_units_in_decoding_tick = svt_aom_dec_get_bits(bs, 32);
    model_info->buffer_removal_time_length_minus_1 =
        svt_aom_dec_get_bits(bs, 5);
    model_info->frame_presentation_time_length_minus_1 =
        svt_aom_dec_get_bits(bs, 5);
}

// Read Operating point parameters
static void read_operating_params_info(Bitstrm *bs,
                                       EbOperatingParametersInfo *op_info,
                                       DecoderModelInfo *model_info,
                                       int index) {
    if (index > MAX_NUM_OPERATING_POINTS)
        return;  // EB_DecUnsupportedBitstream;
    op_info->decoder_buffer_delay =
        svt_aom_dec_get_bits(bs, model_info->buffer_delay_length_minus_1 + 1);
    op_info->encoder_buffer_delay =
        svt_aom_dec_get_bits(bs, model_info->buffer_delay_length_minus_1 + 1);
    op_info->low_delay_mode_flag = svt_aom_dec_get_bits(bs, 1);
}

// Read bit depth
static void read_bit_depth(Bitstrm *bs, EbColorConfig *color_info,
                           SeqHeader *seq_header) {
    uint8_t high_bitdepth = svt_aom_dec_get_bits(bs, 1);
    if (seq_header->seq_profile == PROFESSIONAL_PROFILE && high_bitdepth) {
        uint8_t twelve_bit = svt_aom_dec_get_bits(bs, 1);
        color_info->bit_depth = twelve_bit ? EB_TWELVE_BIT : EB_TEN_BIT;
    } else if (seq_header->seq_profile <= PROFESSIONAL_PROFILE)
        color_info->bit_depth = high_bitdepth ? EB_TEN_BIT : EB_EIGHT_BIT;
    else
        return;  // EB_DecUnsupportedBitstream;
}

// Read Color configuration
static void read_color_config(Bitstrm *bs, EbColorConfig *color_info,
                              SeqHeader *seq_header) {
    read_bit_depth(bs, color_info, seq_header);
    color_info->mono_chrome = (seq_header->seq_profile != HIGH_PROFILE)
                                  ? svt_aom_dec_get_bits(bs, 1)
                                  : 0;
    color_info->color_primaries = EB_CICP_CP_UNSPECIFIED;
    color_info->transfer_characteristics = EB_CICP_TC_UNSPECIFIED;
    color_info->matrix_coefficients = EB_CICP_MC_UNSPECIFIED;
    if (color_info->mono_chrome) {
        color_info->color_range = (EbColorRange)svt_aom_dec_get_bits(bs, 1);
        color_info->subsampling_y = color_info->subsampling_x = 1;
        color_info->chroma_sample_position = EB_CSP_UNKNOWN;
        color_info->separate_uv_delta_q = 0;
        return;
    }

    color_info->color_range = (EbColorRange)svt_aom_dec_get_bits(bs, 1);
    if (seq_header->seq_profile == MAIN_PROFILE)
        color_info->subsampling_x = color_info->subsampling_y = 1;  // 420 only
    else if (seq_header->seq_profile == HIGH_PROFILE)
        color_info->subsampling_x = color_info->subsampling_y = 0;  // 444 only
    else {
        if (color_info->bit_depth == EB_TWELVE_BIT) {
            color_info->subsampling_x = svt_aom_dec_get_bits(bs, 1);
            if (color_info->subsampling_x) {
                color_info->subsampling_y = svt_aom_dec_get_bits(bs, 1);
            } else
                color_info->subsampling_y = 0;
        } else {  // 422
            color_info->subsampling_x = 1;
            color_info->subsampling_y = 0;
        }
    }
    if (color_info->subsampling_x && color_info->subsampling_y)
        color_info->chroma_sample_position =
            (EbChromaSamplePosition)svt_aom_dec_get_bits(bs, 2);

    color_info->separate_uv_delta_q = svt_aom_dec_get_bits(bs, 1);
}

/* Checks that the remaining bits start with a 1 and ends with 0s.
 * It consumes an additional byte, if already byte aligned before the check. */
static int av1_check_trailing_bits(Bitstrm *bs) {
    // bit_offset is set to 0 (mod 8) when the reader is already byte aligned
    int bits_before_alignment = 8 - bs->bit_ofst % 8;
    int trailing = svt_aom_dec_get_bits(bs, bits_before_alignment);
    if (trailing != (1 << (bits_before_alignment - 1)))
        return EB_Corrupt_Frame;
    return 0;
}

// Read Sequence header
EbErrorType read_sequence_header_obu(Bitstrm *bs, SeqHeader *seq_header) {
    EbErrorType status;

    seq_header->seq_profile = (EbAv1SeqProfile)svt_aom_dec_get_bits(bs, 3);
    if (seq_header->seq_profile > CONFIG_MAX_DECODE_PROFILE)
        return EB_Corrupt_Frame;

    seq_header->still_picture = svt_aom_dec_get_bits(bs, 1);
    seq_header->reduced_still_picture_header = svt_aom_dec_get_bits(bs, 1);

    // Video must have reduced_still_picture_header = 0
    if (!seq_header->still_picture && seq_header->reduced_still_picture_header)
        return EB_DecUnsupportedBitstream;

    if (seq_header->reduced_still_picture_header) {
        seq_header->timing_info.timing_info_present = 0;
        seq_header->decoder_model_info_present_flag = 0;
        seq_header->initial_display_delay_present_flag = 0;
        seq_header->operating_points_cnt_minus_1 = 0;
        seq_header->operating_point[0].op_idc = 0;
        seq_header->operating_point[0].seq_level_idx =
            svt_aom_dec_get_bits(bs, LEVEL_BITS);
        if (!is_valid_seq_level_idx(seq_header->operating_point->seq_level_idx))
            return EB_Corrupt_Frame;
        seq_header->operating_point[0].seq_tier = 0;
        seq_header->operating_point[0].decoder_model_present_for_this_op = 0;
        seq_header->operating_point[0]
            .initial_display_delay_present_for_this_op = 0;
    } else {
        seq_header->timing_info.timing_info_present =
            svt_aom_dec_get_bits(bs, 1);
        if (seq_header->timing_info.timing_info_present) {
            read_timing_info(bs, &seq_header->timing_info);
            seq_header->decoder_model_info_present_flag =
                svt_aom_dec_get_bits(bs, 1);
            if (seq_header->decoder_model_info_present_flag)
                read_decoder_model_info(bs, &seq_header->decoder_model_info);
        } else
            seq_header->decoder_model_info_present_flag = 0;
    }

    seq_header->initial_display_delay_present_flag =
        svt_aom_dec_get_bits(bs, 1);
    seq_header->operating_points_cnt_minus_1 =
        svt_aom_dec_get_bits(bs, OP_POINTS_CNT_MINUS_1_BITS);
    for (int i = 0; i <= seq_header->operating_points_cnt_minus_1; i++) {
        seq_header->operating_point[i].op_idc =
            svt_aom_dec_get_bits(bs, OP_POINTS_IDC_BITS);
        seq_header->operating_point[i].seq_level_idx =
            svt_aom_dec_get_bits(bs, LEVEL_BITS);
        if (!is_valid_seq_level_idx(
                seq_header->operating_point[i].seq_level_idx))
            return EB_Corrupt_Frame;
        if (seq_header->operating_point[i].seq_level_idx > 7) {
            seq_header->operating_point[i].seq_tier =
                svt_aom_dec_get_bits(bs, 1);
        } else
            seq_header->operating_point[i].seq_tier = 0;

        if (seq_header->decoder_model_info_present_flag) {
            seq_header->operating_point[i].decoder_model_present_for_this_op =
                svt_aom_dec_get_bits(bs, 1);
            if (seq_header->operating_point[i]
                    .decoder_model_present_for_this_op)
                read_operating_params_info(
                    bs,
                    &seq_header->operating_point[i].operating_parameters_info,
                    &seq_header->decoder_model_info,
                    i);
        } else
            seq_header->operating_point[i].decoder_model_present_for_this_op =
                0;

        if (seq_header->initial_display_delay_present_flag) {
            seq_header->operating_point[i]
                .initial_display_delay_present_for_this_op =
                svt_aom_dec_get_bits(bs, 1);
            if (seq_header->operating_point[i]
                    .initial_display_delay_present_for_this_op)
                seq_header->operating_point[i].initial_display_delay =
                    svt_aom_dec_get_bits(bs, 4) + 1;
        }
    }

    // TODO: operating_point = choose_operating_point( )

    seq_header->frame_width_bits = svt_aom_dec_get_bits(bs, 4) + 1;
    seq_header->frame_height_bits = svt_aom_dec_get_bits(bs, 4) + 1;
    seq_header->max_frame_width =
        svt_aom_dec_get_bits(bs, (seq_header->frame_width_bits)) + 1;
    seq_header->max_frame_height =
        svt_aom_dec_get_bits(bs, (seq_header->frame_height_bits)) + 1;

    if (seq_header->reduced_still_picture_header)
        seq_header->frame_id_numbers_present_flag = 0;
    else {
        seq_header->frame_id_numbers_present_flag = svt_aom_dec_get_bits(bs, 1);
    }
    if (seq_header->frame_id_numbers_present_flag) {
        seq_header->delta_frame_id_length = svt_aom_dec_get_bits(bs, 4) + 2;
        seq_header->frame_id_length = svt_aom_dec_get_bits(bs, 3) + 1;
        if (seq_header->frame_id_length - 1 > 16)
            return EB_Corrupt_Frame;
    }

    seq_header->use_128x128_superblock = svt_aom_dec_get_bits(bs, 1);
    seq_header->sb_size =
        seq_header->use_128x128_superblock ? BLOCK_128X128 : BLOCK_64X64;
    seq_header->sb_mi_size = seq_header->use_128x128_superblock ? 32 : 16;
    seq_header->sb_size_log2 = seq_header->use_128x128_superblock ? 7 : 6;
    seq_header->filter_intra_level = svt_aom_dec_get_bits(bs, 1);
    seq_header->enable_intra_edge_filter = svt_aom_dec_get_bits(bs, 1);

    if (seq_header->reduced_still_picture_header) {
        seq_header->enable_interintra_compound = 0;
        seq_header->enable_masked_compound = 0;
        seq_header->enable_warped_motion = 0;
        seq_header->enable_dual_filter = 0;
        seq_header->order_hint_info.enable_jnt_comp = 0;
        seq_header->order_hint_info.enable_ref_frame_mvs = 0;
        seq_header->seq_force_screen_content_tools = 2;
        seq_header->seq_force_integer_mv = 2;
        seq_header->order_hint_info.order_hint_bits = 0;
    } else {
        seq_header->enable_interintra_compound = svt_aom_dec_get_bits(bs, 1);
        seq_header->enable_masked_compound = svt_aom_dec_get_bits(bs, 1);
        seq_header->enable_warped_motion = svt_aom_dec_get_bits(bs, 1);
        seq_header->enable_dual_filter = svt_aom_dec_get_bits(bs, 1);
        seq_header->order_hint_info.enable_order_hint =
            svt_aom_dec_get_bits(bs, 1);
        if (seq_header->order_hint_info.enable_order_hint) {
            seq_header->order_hint_info.enable_jnt_comp =
                svt_aom_dec_get_bits(bs, 1);
            seq_header->order_hint_info.enable_ref_frame_mvs =
                svt_aom_dec_get_bits(bs, 1);
        } else {
            seq_header->order_hint_info.enable_jnt_comp = 0;
            seq_header->order_hint_info.enable_ref_frame_mvs = 0;
        }
        if (svt_aom_dec_get_bits(bs, 1)) {
            seq_header->seq_force_screen_content_tools =
                SELECT_SCREEN_CONTENT_TOOLS;
        } else
            seq_header->seq_force_screen_content_tools =
                svt_aom_dec_get_bits(bs, 1);
        if (seq_header->seq_force_screen_content_tools > 0) {
            if (svt_aom_dec_get_bits(bs, 1)) {
                seq_header->seq_force_integer_mv = SELECT_INTEGER_MV;
            } else
                seq_header->seq_force_integer_mv = svt_aom_dec_get_bits(bs, 1);
        } else
            seq_header->seq_force_integer_mv = SELECT_INTEGER_MV;

        if (seq_header->order_hint_info.enable_order_hint) {
            seq_header->order_hint_info.order_hint_bits =
                svt_aom_dec_get_bits(bs, 3) + 1;
        } else
            seq_header->order_hint_info.order_hint_bits = 0;
    }
    seq_header->enable_superres = svt_aom_dec_get_bits(bs, 1);
    seq_header->cdef_level = svt_aom_dec_get_bits(bs, 1);
    seq_header->enable_restoration = svt_aom_dec_get_bits(bs, 1);

    read_color_config(bs, &seq_header->color_config, seq_header);
    seq_header->film_grain_params_present = svt_aom_dec_get_bits(bs, 1);
    status = (EbErrorType)av1_check_trailing_bits(bs);
    if (status != EB_ErrorNone)
        return status;
    return EB_ErrorNone;
}

EbErrorType svt_get_sequence_info(const uint8_t *obu_data, size_t size,
                                  SeqHeader *sequence_info) {
    if (obu_data == NULL || size == 0 || sequence_info == NULL)
        return EB_ErrorBadParameter;
    const uint8_t *frame_buf = obu_data;
    size_t frame_sz = size;
    EbErrorType status = EB_ErrorNone;
    do {
        Bitstrm bs;
        svt_aom_dec_bits_init(&bs, frame_buf, frame_sz);

        ObuHeader ou;
        memset(&ou, 0, sizeof(ou));
        size_t length_size = 0;
        status = read_obu_header_size(&bs, &ou, frame_sz, &length_size);
        if (status != EB_ErrorNone)
            return status;

        frame_buf += ou.size + length_size;
        frame_sz -= (uint32_t)(ou.size + length_size);

        if (ou.obu_type == OBU_SEQUENCE_HEADER) {
            // check the ou type and parse sequence header
            status = read_sequence_header_obu(&bs, sequence_info);
            if (status == EB_ErrorNone)
                return status;
        }

        frame_buf += ou.payload_size;
        frame_sz -= ou.payload_size;
    } while (status == EB_ErrorNone && frame_sz > 0);
    return EB_ErrorUndefined;
}

namespace svt_av1_e2e_tools {

void SequenceHeaderParser::input_obu_data(const uint8_t *obu_data,
                                          const uint32_t size,
                                          RefDecoder::StreamInfo *stream_info) {
    SeqHeader seg_header;
    seg_header.seq_force_integer_mv = SELECT_INTEGER_MV;
    seg_header.order_hint_info.enable_ref_frame_mvs = 0;
    seg_header.order_hint_info.enable_jnt_comp = 0;
    seg_header.enable_dual_filter = 0;
    seg_header.enable_warped_motion = 0;
    seg_header.enable_masked_compound = 0;
    seg_header.enable_intra_edge_filter = 0;
    seg_header.filter_intra_level = 0;
    seg_header.decoder_model_info.buffer_delay_length_minus_1 = 0;

    if (svt_get_sequence_info(obu_data, size, &seg_header) == EB_ErrorNone) {
        profile_ = seg_header.seq_profile;
        switch (seg_header.sb_size) {
        case BLOCK_64X64: sb_size_ = 64; break;
        case BLOCK_128X128: sb_size_ = 128; break;
        default:
            ASSERT_TRUE(false)
                << "super block size is invalid in sequence header!";
            break;
        }
        printf("SPS header: profile(%u), sb_size(%u)\n", profile_, sb_size_);

        // update stream info
        stream_info->profile = seg_header.seq_profile;
        stream_info->still_pic = seg_header.still_picture == 1;
        stream_info->sb_size = sb_size_;
        stream_info->force_integer_mv = seg_header.seq_force_integer_mv;
        stream_info->enable_filter_intra = seg_header.filter_intra_level;
        stream_info->enable_intra_edge_filter =
            seg_header.enable_intra_edge_filter;
        stream_info->enable_masked_compound = seg_header.enable_masked_compound;
        stream_info->enable_dual_filter = seg_header.enable_dual_filter;
        stream_info->enable_jnt_comp =
            seg_header.order_hint_info.enable_jnt_comp;
        stream_info->enable_ref_frame_mvs =
            seg_header.order_hint_info.enable_ref_frame_mvs;
        stream_info->enable_warped_motion = seg_header.enable_warped_motion;
        stream_info->cdef_level = seg_header.cdef_level;
        stream_info->enable_restoration = seg_header.enable_restoration;
        stream_info->enable_superres = seg_header.enable_superres;
    }
}
}  // namespace svt_av1_e2e_tools
