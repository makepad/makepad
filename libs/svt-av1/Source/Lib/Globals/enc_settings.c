/*
* Copyright(c) 2022 Intel Corporation
*
* This source code is subject to the terms of the BSD 3-Clause Clear License and
* the Alliance for Open Media Patent License 1.0. If the BSD 3-Clause Clear License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/
// SUMMARY
//   Contains the encoder settings API functions

/**************************************
 * Includes
 **************************************/
#include <stdbool.h>
#include <stdlib.h>
#include <stdio.h>
#include <stdint.h>
#include "EbVersion.h"
#include "definitions.h"
#include "EbSvtAv1Enc.h"
#include "EbSvtAv1Metadata.h"
#include "enc_settings.h"
#include "entropy_coding.h"

#include "svt_log.h"
#include "utility.h"

#ifdef _WIN32
#include <windows.h>
#else
#include <errno.h>
#include <pthread.h>
#include <unistd.h>
#endif

/******************************************
* Verify Settings
******************************************/
EbErrorType svt_av1_verify_settings(SequenceControlSet* scs) {
    EbErrorType               return_error = EB_ErrorNone;
    EbSvtAv1EncConfiguration* config       = &scs->static_config;
    if (config->enc_mode > MAX_ENC_PRESET || config->enc_mode < -1) {
        SVT_ERROR("EncoderMode must be in the range of [-1-%d]\n", MAX_ENC_PRESET);
        return_error = EB_ErrorBadParameter;
    }
    if (scs->max_input_luma_width < 4) {
        SVT_ERROR("Source Width must be at least 4\n");
        return_error = EB_ErrorBadParameter;
    }
    if (scs->max_input_luma_height < 4) {
        SVT_ERROR("Source Height must be at least 4\n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->pred_structure > RANDOM_ACCESS || config->pred_structure < LOW_DELAY) {
        SVT_ERROR("Pred Structure must be [%d (low delay) or %d (random access)]\n", LOW_DELAY, RANDOM_ACCESS);
        return_error = EB_ErrorBadParameter;
    }
    if (config->pred_structure == LOW_DELAY && config->pass > 0) {
        SVT_ERROR("Multi-passes is not support with Low Delay mode \n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->maximum_buffer_size_ms < 20 || config->maximum_buffer_size_ms > 10000) {
        SVT_ERROR("The maximum buffer size must be between [20, 10000]\n");
        return_error = EB_ErrorBadParameter;
    }

    if (config->starting_buffer_level_ms < 20 || config->starting_buffer_level_ms > 10000) {
        SVT_ERROR("The initial buffer size must be between [20, 10000] \n");
        return_error = EB_ErrorBadParameter;
    } else if (config->starting_buffer_level_ms >= config->maximum_buffer_size_ms) {
        SVT_WARN(
            "The initial buffer size must be less than maximum buffer size. Defaulting optimal "
            "buffer size to maximum buffer size - 1 (%u)\n",
            config->maximum_buffer_size_ms - 1);
        config->starting_buffer_level_ms = (config->maximum_buffer_size_ms - 1);
    }

    if (config->optimal_buffer_level_ms < 20 || config->optimal_buffer_level_ms > 10000) {
        SVT_ERROR("The optimal buffer size must be between [20, 10000]\n");
        return_error = EB_ErrorBadParameter;
    } else if (config->optimal_buffer_level_ms >= config->maximum_buffer_size_ms) {
        SVT_WARN(
            "The optimal buffer size must be less than maximum buffer size. Defaulting optimal "
            "buffer size to maximum buffer size - 1 (%u)\n",
            config->maximum_buffer_size_ms - 1);
        config->optimal_buffer_level_ms = (config->maximum_buffer_size_ms - 1);
    }

    if (config->over_shoot_pct > 100) {
        SVT_ERROR("The overshoot percentage must be between [0, 100] \n");
        return_error = EB_ErrorBadParameter;
    }

    if (config->mbr_over_shoot_pct > 100) {
        SVT_ERROR("The max bitrate overshoot percentage must be between [0, 100] \n");
        return_error = EB_ErrorBadParameter;
    }

    if (config->under_shoot_pct > 100) {
        SVT_ERROR("The undershoot percentage must be between [0, 100] \n");
        return_error = EB_ErrorBadParameter;
    }

    if (config->target_bit_rate > 100000000) {
        SVT_ERROR("The target bit rate must be between [0, 100000] kbps \n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->max_bit_rate > 100000000) {
        SVT_ERROR("The maximum bit rate must be between [0, 100000] kbps \n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->vbr_max_section_pct > 10000) {
        SVT_ERROR("The max section percentage must be between [0, 10000] \n");
        return_error = EB_ErrorBadParameter;
    }

    if (config->vbr_min_section_pct > 100) {
        SVT_ERROR("he min section percentage must be between [0, 100] \n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->gop_constraint_rc &&
        ((config->rate_control_mode != SVT_AV1_RC_MODE_VBR) || config->intra_period_length < 119)) {
        SVT_ERROR(
            "Gop constraint rc is only supported with VBR mode when Gop size is "
            "greater than 119 \n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->gop_constraint_rc) {
        SVT_WARN(
            "The GoP constraint RC mode is a work-in-progress project, and is only "
            "available for demos, experimentation, and further development uses and should not be "
            "used for benchmarking until fully implemented.\n");
    }

    if (config->force_key_frames &&
        (config->rate_control_mode == SVT_AV1_RC_MODE_CBR || config->pred_structure != RANDOM_ACCESS)) {
        SVT_WARN(
            "Force key frames is now supported for lowdelay but the force_key_frames flag"
            " does not need to be set be on. Please follow the app samples shown by the FTR_KF_ON_FLY_SAMPLE"
            " macro on how to use it. force_key_frames will now be set to 0 \n");
        config->force_key_frames = 0;
    }
    if (config->force_key_frames && config->rate_control_mode == SVT_AV1_RC_MODE_VBR) {
        SVT_ERROR("Force key frames is not supported for VBR mode \n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->rate_control_mode != SVT_AV1_RC_MODE_CQP_OR_CRF && (config->max_bit_rate != 0)) {
        SVT_ERROR("Max Bitrate only supported with CRF mode\n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->rate_control_mode == SVT_AV1_RC_MODE_CBR && config->pred_structure == RANDOM_ACCESS) {
        SVT_ERROR("CBR Rate control is currently not supported for RANDOM_ACCESS, use VBR mode\n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->rate_control_mode == SVT_AV1_RC_MODE_VBR && config->pred_structure == LOW_DELAY) {
        SVT_ERROR("VBR Rate control is currently not supported for LOW_DELAY, use CBR mode\n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->rate_control_mode == SVT_AV1_RC_MODE_CQP_OR_CRF && config->target_bit_rate != DEFAULT_TBR) {
        SVT_ERROR("Target Bitrate only supported when --rc is  1/2 (VBR/CBR). Current --rc: %d\n",
                  config->rate_control_mode);
        return_error = EB_ErrorBadParameter;
    }

    if (scs->max_input_luma_width > 16384) {
        SVT_ERROR("Source Width must be less than or equal to 16384\n");
        return_error = EB_ErrorBadParameter;
    }

    if (scs->max_input_luma_height > 8704) {
        SVT_ERROR("Source Height must be less than or equal to 8704)\n");
        return_error = EB_ErrorBadParameter;
    }

    if (scs->seq_header.max_frame_width < 4) {
        SVT_ERROR("Forced Max Width must be at least 4\n");
        return_error = EB_ErrorBadParameter;
    }
    if (scs->seq_header.max_frame_height < 4) {
        SVT_ERROR("Forced Max Height must be at least 4\n");
        return_error = EB_ErrorBadParameter;
    }
    if (scs->seq_header.max_frame_width > 16384) {
        SVT_ERROR("Forced Max Width must be less than or equal to 16384\n");
        return_error = EB_ErrorBadParameter;
    }
    if (scs->seq_header.max_frame_height > 8704) {
        SVT_ERROR("Forced Max Height must be less than or equal to 8704)\n");
        return_error = EB_ErrorBadParameter;
    }

    // This is not an AV1 spec limitation, but an implementation limitation in the encoder
    // This check will stay in place until restoration filtering can handle these dimensions
    if ((scs->max_input_luma_width >= 4 && scs->max_input_luma_width < 64) ||
        (scs->max_input_luma_height >= 4 && scs->max_input_luma_height < 64)) {
        if (config->aq_mode != 0) {
            SVT_WARN("AQ mode %i is unsupported with source dimensions (%i / %i), setting AQ mode to 0\n",
                     config->aq_mode,
                     scs->max_input_luma_width,
                     scs->max_input_luma_height);
            config->aq_mode = 0;
        }
        if (config->enable_restoration_filtering != 0) {
            SVT_WARN(
                "Restoration Filtering is unsupported with source dimensions (%i / %i), disabling "
                "Restoration Filtering\n",
                scs->max_input_luma_width,
                scs->max_input_luma_height);
            config->enable_restoration_filtering = 0;
        }
    }

    if ((scs->max_input_luma_width > scs->seq_header.max_frame_width) ||
        (scs->max_input_luma_height > scs->seq_header.max_frame_height)) {
        SVT_ERROR(
            "Source Width/Height must be less than or equal to Forced Max "
            "Width/Height\n");
        return_error = EB_ErrorBadParameter;
    }

    if (config->level != 0) {
        BitstreamLevel bl;
        bl.major = config->level / 10;
        bl.minor = config->level % 10;
        // Defined AV1 levels only have major versions 2-9 and minor versions 0-3
        if (bl.minor > LEVEL_MINOR_MAX || bl.major < LEVEL_MAJOR_MIN || bl.major > LEVEL_MAJOR_MAX) {
            SVT_ERROR("Invalid or undefined level specified: %d.%d. See AV1 spec Annex A for defined levels.\n",
                      bl.major,
                      bl.minor);
            return_error = EB_ErrorBadParameter;
        } else {
            // Some levels in the allowable range are undefined by the spec.
            const uint8_t seq_level_idx = major_minor_to_seq_level_idx(bl);
            if (!is_valid_seq_level_idx(seq_level_idx)) {
                SVT_ERROR("Invalid or undefined level specified: %d.%d. See AV1 spec Annex A for defined levels.\n",
                          bl.major,
                          bl.minor);
                return_error = EB_ErrorBadParameter;
            }
        }
    }

    if (config->qp > MAX_QP_VALUE) {
        SVT_ERROR("%s must be [0 - %d]\n", config->aq_mode ? "CRF" : "QP", MAX_QP_VALUE);
        return_error = EB_ErrorBadParameter;
    }

    if ((config->qp == MAX_QP_VALUE && config->extended_crf_qindex_offset > (7 * 4))) {
        SVT_ERROR("%s must be [0 - %d]\n", "CRF", 70);
        return_error = EB_ErrorBadParameter;
    }

    if (config->hierarchical_levels > 5) {
        SVT_ERROR("Hierarchical Levels supported [0-5]\n");
        return_error = EB_ErrorBadParameter;
    }
    if ((config->intra_period_length < -2 || config->intra_period_length > 2 * ((1 << 30) - 1)) &&
        config->rate_control_mode == SVT_AV1_RC_MODE_CQP_OR_CRF) {
        SVT_ERROR("The intra period must be [-2, 2^31-2]\n");
        return_error = EB_ErrorBadParameter;
    }
    if ((config->intra_period_length < 0) && config->rate_control_mode == SVT_AV1_RC_MODE_VBR) {
        SVT_ERROR("The intra period must be > 0 for RateControlMode %d\n", config->rate_control_mode);
        return_error = EB_ErrorBadParameter;
    }

    if (config->intra_refresh_type > 2 || config->intra_refresh_type < 1) {
        SVT_ERROR("Invalid intra Refresh Type [1-2]\n");
        return_error = EB_ErrorBadParameter;
    }

    if (config->enable_dlf_flag > 2) {
        SVT_ERROR("Invalid LoopFilterEnable. LoopFilterEnable must be [0 - 2]\n");
        return_error = EB_ErrorBadParameter;
    }

    if (config->rate_control_mode > SVT_AV1_RC_MODE_CBR &&
        (config->pass == ENC_FIRST_PASS || config->rc_stats_buffer.buf)) {
        SVT_ERROR("Only rate control mode 0~2 are supported for 2-pass \n");
        return_error = EB_ErrorBadParameter;
    }
    // Stats file is checked by the app in handle_stats_file, but must be re-checked here since ffmpeg calls
    // the library but not the app
    if (config->rate_control_mode == SVT_AV1_RC_MODE_VBR && config->pass == ENC_SECOND_PASS) {
        if (!config->rc_stats_buffer.buf) {
            SVT_ERROR("RC stats buffer not available \n");
            return_error = EB_ErrorBadParameter;
        } else if (config->rc_stats_buffer.sz == 0) {
            SVT_ERROR("RC stats buffer size is 0 \n");
            return_error = EB_ErrorBadParameter;
        }
    }
    if (config->profile > 2) {
        SVT_ERROR("The maximum allowed profile value is 2 \n");
        return_error = EB_ErrorBadParameter;
    }

    // Check if the current input video is conformant with the Level constraint
    if (scs->frame_rate > 240) {
        SVT_ERROR("The maximum allowed frame rate is 240 fps\n");
        return_error = EB_ErrorBadParameter;
    }
    // Check that the frame_rate is non-zero
    if (!scs->frame_rate) {
        SVT_ERROR("The frame rate should be greater than 0 fps \n");
        return_error = EB_ErrorBadParameter;
    }
    if (scs->static_config.frame_rate_numerator == 0 || scs->static_config.frame_rate_denominator == 0) {
        SVT_ERROR("The frame_rate_numerator and frame_rate_denominator must be greater than 0\n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->recode_loop > 4) {
        SVT_ERROR("The recode_loop must be [0 - 4] \n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->rate_control_mode > SVT_AV1_RC_MODE_CBR) {
        SVT_ERROR("The rate control mode must be [0 - 2] \n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->look_ahead_distance > MAX_LAD && config->look_ahead_distance != (uint32_t)~0) {
        SVT_ERROR("The lookahead distance must be [0 - %d] \n", MAX_LAD);

        return_error = EB_ErrorBadParameter;
    }
    if ((unsigned)config->tile_rows > 6 || (unsigned)config->tile_columns > 6) {
        SVT_ERROR("Log2Tile rows/cols must be [0 - 6] \n");
        return_error = EB_ErrorBadParameter;
    }
    if ((1u << config->tile_rows) * (1u << config->tile_columns) > 128 || config->tile_columns > 4) {
        SVT_ERROR("MaxTiles is 128 and MaxTileCols is 16 (Annex A.3) \n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->max_qp_allowed > MAX_QP_VALUE) {
        SVT_ERROR("MaxQpAllowed must be [0 - %d]\n", MAX_QP_VALUE);
        return_error = EB_ErrorBadParameter;
    } else if (config->min_qp_allowed > MAX_QP_VALUE) {
        SVT_ERROR("MinQpAllowed must be [0 - %d]\n", MAX_QP_VALUE);
        return_error = EB_ErrorBadParameter;
    } else if (config->min_qp_allowed > config->max_qp_allowed) {
        SVT_ERROR("MinQpAllowed must be smaller than or equal to MaxQpAllowed\n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->use_fixed_qindex_offsets > 2) {
        SVT_ERROR("The use_fixed_qindex_offsets must be [0 - 2] \n");
        return_error = EB_ErrorBadParameter;
    }

    if (config->key_frame_qindex_offset < -64 || config->key_frame_qindex_offset > 63) {
        SVT_ERROR("Invalid key_frame_qindex_offset. key_frame_qindex_offset must be [-64 - 63]\n");
        return_error = EB_ErrorBadParameter;
    }

    for (uint8_t i = 0; i < config->hierarchical_levels + 1; ++i) {
        if (config->qindex_offsets[i] < -64 || config->qindex_offsets[i] > 63) {
            SVT_ERROR("Invalid qindex_offsets. qindex_offsets must be [-64 - 63]\n");
            return_error = EB_ErrorBadParameter;
        }
    }
    if (config->key_frame_chroma_qindex_offset < -64 || config->key_frame_chroma_qindex_offset > 63) {
        SVT_ERROR(
            "Invalid key_frame_chroma_qindex_offset. key_frame_chroma_qindex_offset "
            "must be [-64 - 63]\n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->luma_y_dc_qindex_offset < -64 || config->luma_y_dc_qindex_offset > 63) {
        SVT_ERROR(
            "Invalid luma_y_dc_qindex_offset. luma_y_dc_qindex_offset "
            "must be [-64 - 63]\n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->chroma_u_dc_qindex_offset < -64 || config->chroma_u_dc_qindex_offset > 63) {
        SVT_ERROR(
            "Invalid chroma_u_dc_qindex_offset. chroma_u_dc_qindex_offset "
            "must be [-64 - 63]\n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->chroma_u_ac_qindex_offset < -64 || config->chroma_u_ac_qindex_offset > 63) {
        SVT_ERROR(
            "Invalid chroma_u_ac_qindex_offset. chroma_u_ac_qindex_offset "
            "must be [-64 - 63]\n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->chroma_v_dc_qindex_offset < -64 || config->chroma_v_dc_qindex_offset > 63) {
        SVT_ERROR(
            "Invalid chroma_v_dc_qindex_offset. chroma_v_dc_qindex_offset "
            "must be [-64 - 63]\n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->chroma_v_ac_qindex_offset < -64 || config->chroma_v_ac_qindex_offset > 63) {
        SVT_ERROR(
            "Invalid chroma_v_ac_qindex_offset. chroma_v_ac_qindex_offset "
            "must be [-64 - 63]\n");
        return_error = EB_ErrorBadParameter;
    }

    for (uint8_t i = 0; i < config->hierarchical_levels + 1; ++i) {
        if (config->chroma_qindex_offsets[i] < -64 || config->chroma_qindex_offsets[i] > 63) {
            SVT_ERROR("Invalid chroma_qindex_offsets. chroma_qindex_offsets must be [-64 - 63]\n");
            return_error = EB_ErrorBadParameter;
        }
    }
    if (config->startup_qp_offset < -63 || config->startup_qp_offset > 63) {
        SVT_ERROR("Invalid startup_qp_offset. startup_qp_offset must be [-63 - 63]\n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->stat_report == 1) {
        SVT_WARN("Enabling StatReport can decrease encoding speed\n");
    }

    if (config->stat_report > 1) {
        SVT_ERROR("Invalid StatReport. StatReport must be [0 - 1]\n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->screen_content_mode > 3) {
        SVT_ERROR("Invalid screen_content_mode. screen_content_mode must be [0 - 3]\n");
        return_error = EB_ErrorBadParameter;
    }

    if (scs->static_config.aq_mode > 2) {
        SVT_ERROR("Invalid adaptive quantization (AQ) mode. AQ mode must be [0-2]\n");
        return_error = EB_ErrorBadParameter;
    }

    if ((config->encoder_bit_depth != 8) && (config->encoder_bit_depth != 10)) {
        SVT_ERROR("Encoder Bit Depth shall be only 8 or 10 \n");
        return_error = EB_ErrorBadParameter;
    }
    // Check if the EncoderBitDepth is conformant with the Profile constraint
    if ((config->profile == 0 || config->profile == 1) && config->encoder_bit_depth > 10) {
        SVT_ERROR("The encoder bit depth shall be equal to 8 or 10 for Main/High Profile\n");
        return_error = EB_ErrorBadParameter;
    }

    if (config->encoder_color_format != EB_YUV420) {
        SVT_ERROR("Only support 420 now \n");
        return_error = EB_ErrorBadParameter;
    }

    if (config->profile == 0 && config->encoder_color_format > EB_YUV420) {
        SVT_ERROR("Non 420 color format requires profile 1 or 2\n");
        return_error = EB_ErrorBadParameter;
    }

    if (config->profile == 1 && config->encoder_color_format != EB_YUV444) {
        SVT_ERROR("Profile 1 requires 4:4:4 color format\n");
        return_error = EB_ErrorBadParameter;
    }

    if (config->profile == 2 && config->encoder_bit_depth <= 10 && config->encoder_color_format != EB_YUV422) {
        SVT_ERROR("Profile 2 bit-depth < 10 requires 4:2:2 color format\n");
        return_error = EB_ErrorBadParameter;
    }

    if (config->use_cpu_flags & EB_CPU_FLAGS_INVALID) {
#ifdef ARCH_AARCH64
        SVT_ERROR(
            "param '--asm' have invalid value.\n"
            "Value should be [0 - 6] or [c, neon, crc32, neon_dotprod, neon_i8mm, sve, sve2, max]\n");
#else
        SVT_ERROR(
            "param '--asm' have invalid value.\n"
            "Value should be [0 - 11] or [c, mmx, sse, sse2, sse3, ssse3, sse4_1, sse4_2, avx, "
            "avx2, avx512, avx512icl, max]\n");
#endif
        return_error = EB_ErrorBadParameter;
    }

    // HBD mode decision
    if (scs->enable_hbd_mode_decision < (int8_t)(-1) || scs->enable_hbd_mode_decision > 2) {
        SVT_ERROR("Invalid HBD mode decision flag [-1 - 2], your input: %d\n", scs->enable_hbd_mode_decision);
        return_error = EB_ErrorBadParameter;
    }

    // CDEF
    if (config->cdef_level > 4 || config->cdef_level < -1) {
        SVT_ERROR("Invalid CDEF level [0 - 4, -1 for auto], your input: %d\n", config->cdef_level);
        return_error = EB_ErrorBadParameter;
    }

    // Restoration Filtering
    if (config->enable_restoration_filtering != 0 && config->enable_restoration_filtering != 1 &&
        config->enable_restoration_filtering != -1) {
        SVT_ERROR("Invalid restoration flag [0 - 1, -1 for auto], your input: %d\n",
                  config->enable_restoration_filtering);
        return_error = EB_ErrorBadParameter;
    }

    if (config->enable_mfmv != 0 && config->enable_mfmv != 1 && config->enable_mfmv != -1) {
        SVT_ERROR(
            "Invalid motion field motion vector flag [0/1 or -1 for auto], your "
            "input: %d\n",
            config->enable_mfmv);
        return_error = EB_ErrorBadParameter;
    }
    if (config->fast_decode > 2) {
        SVT_ERROR(
            "Invalid fast decode flag [0 - 2, 0 for no decoder-targeted optimization], your "
            "input: %d\n",
            config->fast_decode);
        return_error = EB_ErrorBadParameter;
    }
    if (config->tune > TUNE_MS_SSIM) {
        SVT_ERROR(
            "Invalid tune flag [0 - 4, 0 for VQ, 1 for PSNR, 2 for SSIM, 3 for IQ, and 4 for MS_SSIM], your input: "
            "%d\n",
            config->tune);
        return_error = EB_ErrorBadParameter;
    }
    if (config->tune == TUNE_SSIM || config->tune == TUNE_IQ || config->tune == TUNE_MS_SSIM) {
        if (config->rate_control_mode != 0 || config->pred_structure != RANDOM_ACCESS) {
            SVT_ERROR("tune %s only supports CRF rate control mode currently\n",
                      config->tune == TUNE_SSIM     ? "SSIM"
                          : config->tune == TUNE_IQ ? "IQ"
                                                    : "MS_SSIM");
            return_error = EB_ErrorBadParameter;
        }
    }

    if (config->superres_mode > SUPERRES_AUTO) {
        SVT_ERROR("invalid superres-mode %d, should be in the range [%d - %d]\n",
                  config->superres_mode,
                  SUPERRES_NONE,
                  SUPERRES_AUTO);
        return_error = EB_ErrorBadParameter;
    }
    if (config->superres_mode > 0 && ((config->rc_stats_buffer.sz || config->pass == ENC_FIRST_PASS))) {
        SVT_ERROR("superres is not supported for 2-pass\n");
        return_error = EB_ErrorBadParameter;
    }

    if (config->superres_qthres > MAX_QP_VALUE) {
        SVT_ERROR("Invalid superres-qthres %d, should be in the range [%d - %d]\n",
                  config->superres_qthres,
                  MIN_QP_VALUE,
                  MAX_QP_VALUE);
        return_error = EB_ErrorBadParameter;
    }

    if (config->superres_kf_qthres > MAX_QP_VALUE) {
        SVT_ERROR("Invalid superres-kf-qthres %d, should be in the range [%d - %d]\n",
                  config->superres_kf_qthres,
                  MIN_QP_VALUE,
                  MAX_QP_VALUE);
        return_error = EB_ErrorBadParameter;
    }

    if (config->superres_kf_denom < MIN_SUPERRES_DENOM || config->superres_kf_denom > MAX_SUPERRES_DENOM) {
        SVT_ERROR("Invalid superres-kf-denom %d, should be in the range [%d - %d]\n",
                  config->superres_kf_denom,
                  MIN_SUPERRES_DENOM,
                  MAX_SUPERRES_DENOM);
        return_error = EB_ErrorBadParameter;
    }

    if (config->superres_denom < MIN_SUPERRES_DENOM || config->superres_denom > MAX_SUPERRES_DENOM) {
        SVT_ERROR("Invalid superres-denom %d, should be in the range [%d - %d]\n",
                  config->superres_denom,
                  MIN_SUPERRES_DENOM,
                  MAX_SUPERRES_DENOM);
        return_error = EB_ErrorBadParameter;
    }

    if (config->resize_mode > RESIZE_RANDOM_ACCESS) {
        SVT_ERROR("Invalid resize-mode %d, should be in the range [%d - %d]\n",
                  config->resize_mode,
                  RESIZE_NONE,
                  RESIZE_RANDOM_ACCESS);
        return_error = EB_ErrorBadParameter;
    }

    if (config->resize_kf_denom < MIN_RESIZE_DENOM || config->resize_kf_denom > MAX_RESIZE_DENOM) {
        SVT_ERROR("Invalid resize-kf-denom %d, should be in the range [%d - %d]\n",
                  config->resize_kf_denom,
                  MIN_RESIZE_DENOM,
                  MAX_RESIZE_DENOM);
        return_error = EB_ErrorBadParameter;
    }

    if (config->resize_denom < MIN_RESIZE_DENOM || config->resize_denom > MAX_RESIZE_DENOM) {
        SVT_ERROR("Invalid resize-denom %d, should be in the range [%d - %d]\n",
                  config->resize_denom,
                  MIN_RESIZE_DENOM,
                  MAX_RESIZE_DENOM);
        return_error = EB_ErrorBadParameter;
    }

    if (config->matrix_coefficients == 0 && config->encoder_color_format != EB_YUV444) {
        SVT_ERROR(
            "Identity matrix (matrix_coefficient = 0) may be used only with 4:4:4 "
            "color format.\n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->hierarchical_levels < 2 || config->hierarchical_levels > 5) {
        SVT_ERROR("Only hierarchical levels 2-5 is currently supported.\n");
        return_error = EB_ErrorBadParameter;
    }

    if (config->rate_control_mode == SVT_AV1_RC_MODE_VBR && config->intra_period_length == -1) {
        SVT_ERROR(
            "keyint = -1 is not supported for modes other than CRF rate control "
            "encoding modes.\n");
        return_error = EB_ErrorBadParameter;
    }
    // Block the use of M4 or lower for resolutions higher than 4K, unless allintra coding is used (due to memory constraints)
    if (!scs->allintra && (uint64_t)(scs->max_input_luma_width * scs->max_input_luma_height) > INPUT_SIZE_4K_TH &&
        config->enc_mode <= ENC_M4) {
        SVT_ERROR("8k+ resolution support is limited to M5 and faster presets.\n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->pass > 0 && scs->static_config.enable_overlays) {
        SVT_ERROR(
            "The overlay frames feature is currently not supported with multi-pass "
            "encoding\n");
        return_error = EB_ErrorBadParameter;
    }
    int pass = config->pass;

    if (pass < 0 || pass > 2) {
        SVT_ERROR("%d pass encode is not supported. --pass has a range of [0-2]\n", pass);
        return_error = EB_ErrorBadParameter;
    }

    if (config->intra_refresh_type != 2 && pass > 0) {
        SVT_ERROR("Multi-pass encode only supports closed-gop configurations.\n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->pass > 1 && config->rate_control_mode == SVT_AV1_RC_MODE_CQP_OR_CRF) {
        SVT_ERROR("CRF does not support multi-pass. Use single pass.\n");
        return_error = EB_ErrorBadParameter;
    }

    if (config->aq_mode == 0 && config->rate_control_mode) {
        SVT_ERROR("Adaptive quantization can not be turned OFF when RC ON\n");
        return_error = EB_ErrorBadParameter;
    }

    if (config->sframe_dist < 0) {
        SVT_ERROR("Switch frame interval must be >= 0\n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->sframe_dist > 0 && config->hierarchical_levels == 0) {
        SVT_ERROR("Switch frame feature does not support flat IPPP\n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->sframe_dist > 0 &&
        (config->sframe_mode < SFRAME_STRICT_BASE || config->sframe_mode > SFRAME_DEC_POSI_BASE)) {
        SVT_ERROR("Invalid switch frame mode %d, should be in the range [%d - %d]\n",
                  config->sframe_mode,
                  SFRAME_STRICT_BASE,
                  SFRAME_DEC_POSI_BASE);
        return_error = EB_ErrorBadParameter;
    }
    if (config->sframe_posi.sframe_posis && !IS_SFRAME_FLEXIBLE_INSERT(config->sframe_mode)) {
        SVT_ERROR("S-Frame positions are only supported in S-Frame Flexible ARF mode\n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->sframe_posi.sframe_qp_num && config->rate_control_mode != SVT_AV1_RC_MODE_CQP_OR_CRF) {
        SVT_ERROR("S-Frame QP feature only supports CRF/CQP rate control mode\n");
        return_error = EB_ErrorBadParameter;
    }
    if ((config->sframe_posi.sframe_qps && config->sframe_posi.sframe_qp_offsets) ||
        (config->sframe_qp > 0 && config->sframe_qp_offset != 0)) {
        SVT_ERROR("S-Frame QP feature cannot support QP value and QP offset at same time\n");
        return_error = EB_ErrorBadParameter;
    }

    /* Warnings about the use of features that are incomplete */
    if (config->aq_mode == 1) {
        SVT_WARN(
            "The adaptive quantization mode using segmentation is at a support level "
            "only to be available for demos, experimentation, and further development uses and "
            "should not be used for benchmarking until fully implemented.\n");
    }

    // color description
    if (config->color_primaries == 0 || config->color_primaries == 3 ||
        (config->color_primaries >= 13 && config->color_primaries <= 21) || config->color_primaries > 22) {
        SVT_WARN("Value %u for color_primaries is reserved and not recommended for usage.\n", config->color_primaries);
    }
    if (config->transfer_characteristics == 0 || config->transfer_characteristics == 3 ||
        config->transfer_characteristics > 18) {
        SVT_WARN("Value %u for transfer_characteristics is reserved and not recommended for usage.\n",
                 config->transfer_characteristics);
    }

    if (config->matrix_coefficients == 3 || config->matrix_coefficients > 14) {
        SVT_WARN("Value %u for matrix_coefficients is reserved and not recommended for usage.\n",
                 config->matrix_coefficients);
    }

    if (config->chroma_sample_position < EB_CSP_UNKNOWN || config->chroma_sample_position > EB_CSP_COLOCATED) {
        if (config->chroma_sample_position != EB_CSP_RESERVED) {
            SVT_ERROR("Chroma sample position %d is unknown.\n", config->chroma_sample_position);
            return_error = EB_ErrorBadParameter;
        } else {
            SVT_WARN(
                "Value %d for chroma_sample_position is reserved "
                "and not recommended for usage.\n",
                config->chroma_sample_position);
        }
    }

    if (config->film_grain_denoise_strength > 0 && config->enc_mode > 6) {
        SVT_WARN(
            "It is recommended to not use Film Grain for presets greater than 6 as it "
            "produces a significant compute overhead. This combination should only be used for "
            "debug purposes.\n");
    }

    if (config->film_grain_denoise_strength > 50) {
        SVT_ERROR(
            "Film grain denoise strength is only supported for values between "
            "[0,50]\n");
        return_error = EB_ErrorBadParameter;
    }

    if (config->film_grain_denoise_apply != 0 && config->film_grain_denoise_apply != 1) {
        SVT_ERROR("The film grain denoise apply signal can only have a value of 0 or 1\n");
        return_error = EB_ErrorBadParameter;
    }

    // Limit 8K & 16K support
    if ((uint64_t)(scs->max_input_luma_width * scs->max_input_luma_height) > INPUT_SIZE_4K_TH) {
        SVT_WARN(
            "8K and higher resolution support is currently a work-in-progress "
            "project, and is only available for demos, experimentation, and further development "
            "uses and should not be used for benchmarking until fully implemented.\n");
    }

    if (config->pred_structure == LOW_DELAY) {
        if (config->tune == TUNE_VQ) {
            SVT_WARN("Tune 0 is not applicable for low-delay, tune will be forced to 1.\n");
            config->tune = TUNE_PSNR;
        }

        if (config->superres_mode != 0) {
            SVT_ERROR("Superres is not supported for low-delay.\n");
            return_error = EB_ErrorBadParameter;
        }

        if (config->enable_overlays) {
            SVT_ERROR("Overlay is not supported for low-delay.\n");
            return_error = EB_ErrorBadParameter;
        }
    }

    if (!config->rtc && config->enc_mode >= ENC_M10) {
        SVT_WARN("Non-RTC M10+ are meant for automation tooling usage. Visual artifacts may occur otherwise.\n");
    }

    if (scs->static_config.scene_change_detection) {
        scs->static_config.scene_change_detection = 0;
        SVT_WARN(
            "SVT-AV1 has an integrated mode decision mechanism to handle scene changes and will "
            "not insert a key frame at scene changes\n");
    }
    if ((config->tile_columns > 0 || config->tile_rows > 0)) {
        SVT_WARN(
            "If you are using tiles with the intent of increasing the decoder speed, please also "
            "consider using --fast-decode 1 or 2, especially if the intended decoder is running with "
            "limited multi-threading capabilities.\n");
    }
    if (config->tune == TUNE_VQ && config->fast_decode > 0) {
        SVT_WARN(
            "--fast-decode has been developed and optimized with --tune 1. "
            "Please use it with caution when encoding with --tune 0. You can also consider using "
            "--tile-columns 1 if you are targeting a high quality encode and a multi-core "
            "high-performance decoder HW\n");
    }
    if (config->enable_qm && config->min_qm_level > config->max_qm_level) {
        SVT_ERROR("Min quant matrix level must not greater than max quant matrix level\n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->enable_qm && config->min_chroma_qm_level > config->max_chroma_qm_level) {
        SVT_ERROR("Min chroma quant matrix level must not greater than max chroma quant matrix level\n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->startup_mg_size != 0 && config->startup_mg_size != 2 && config->startup_mg_size != 3 &&
        config->startup_mg_size != 4) {
        SVT_ERROR("Startup MG size supported [0, 2, 3, 4]\n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->startup_mg_size > config->hierarchical_levels) {
        SVT_ERROR("Startup MG size must less than or equal to hierarchical levels\n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->startup_mg_size != 0 && config->rate_control_mode != SVT_AV1_RC_MODE_CQP_OR_CRF) {
        SVT_ERROR("Startup MG size feature only supports CRF/CQP rate control mode\n");
        return_error = EB_ErrorBadParameter;
    }
    if (config->startup_qp_offset != 0 && config->rate_control_mode != 0) {
        SVT_ERROR("Startup QP offset only supports CRF/CQP rate control mode\n");
        return_error = EB_ErrorBadParameter;
    }

    if (config->variance_boost_strength < 1 || config->variance_boost_strength > 4) {
        SVT_ERROR("Variance Boost strength must be between 1 and 4\n");
        return_error = EB_ErrorBadParameter;
    }

    if (config->variance_octile < 1 || config->variance_octile > 8) {
        SVT_ERROR("Variance Boost octile must be between 1 and 8\n");
        return_error = EB_ErrorBadParameter;
    }

    if (config->tf_strength > 4) {
        SVT_ERROR("Temporal filtering strength must be between 0 and 4\n");
        return_error = EB_ErrorBadParameter;
    }

    if (config->variance_boost_curve > 2) {
        SVT_ERROR("Variance Boost curve must be between 0 and 2\n");
        return_error = EB_ErrorBadParameter;
    }

    if (config->luminance_qp_bias > 100) {
        SVT_ERROR("Luminance-based QP bias value must be between 0 and 100\n");
        return_error = EB_ErrorBadParameter;
    }

    if (config->sharpness > 7 || config->sharpness < -7) {
        SVT_ERROR("Sharpness level must be between -7 and 7\n");
        return_error = EB_ErrorBadParameter;
    }

    if (config->qp_scale_compress_strength > 3) {
        SVT_ERROR("QP scale compress strength must be between 0 and 3\n");
        return_error = EB_ErrorBadParameter;
    }

    if (config->max_tx_size != 32 && config->max_tx_size != 64) {
        SVT_ERROR("Supported Max TX size values are 32 and 64\n");
        return_error = EB_ErrorBadParameter;
    }

    if (config->ac_bias > 8.0 || config->ac_bias < 0.0) {
        SVT_ERROR("AC bias strength must be between 0.0 and 8.0\n");
        return_error = EB_ErrorBadParameter;
    }

    return return_error;
}

/**********************************
Set Default Library Params
**********************************/
EbErrorType svt_av1_set_default_params(EbSvtAv1EncConfiguration* config_ptr) {
    EbErrorType return_error = EB_ErrorNone;

    if (!config_ptr) {
        SVT_ERROR("The EbSvtAv1EncConfiguration structure is empty!\n");
        return EB_ErrorBadParameter;
    }
    config_ptr->frame_rate_numerator     = 60000;
    config_ptr->frame_rate_denominator   = 1000;
    config_ptr->encoder_bit_depth        = 8;
    config_ptr->source_width             = 0;
    config_ptr->source_height            = 0;
    config_ptr->forced_max_frame_width   = 0;
    config_ptr->forced_max_frame_height  = 0;
    config_ptr->stat_report              = 0;
    config_ptr->tile_rows                = DEFAULT;
    config_ptr->tile_columns             = DEFAULT;
    config_ptr->qp                       = DEFAULT_QP;
    config_ptr->use_qp_file              = false;
    config_ptr->use_fixed_qindex_offsets = 0;
    memset(config_ptr->qindex_offsets, 0, sizeof(config_ptr->qindex_offsets));
    config_ptr->key_frame_chroma_qindex_offset = 0;
    config_ptr->key_frame_qindex_offset        = 0;
    memset(config_ptr->chroma_qindex_offsets, 0, sizeof(config_ptr->chroma_qindex_offsets));
    config_ptr->luma_y_dc_qindex_offset   = 0;
    config_ptr->chroma_u_dc_qindex_offset = 0;
    config_ptr->chroma_u_ac_qindex_offset = 0;
    config_ptr->chroma_v_dc_qindex_offset = 0;
    config_ptr->chroma_v_ac_qindex_offset = 0;

    for (int i = 0; i < SVT_AV1_FRAME_UPDATE_TYPES; i++) {
        config_ptr->lambda_scale_factors[i] = 128;
    }

    config_ptr->scene_change_detection       = 0;
    config_ptr->rate_control_mode            = SVT_AV1_RC_MODE_CQP_OR_CRF;
    config_ptr->look_ahead_distance          = (uint32_t)~0;
    config_ptr->target_bit_rate              = DEFAULT_TBR;
    config_ptr->max_bit_rate                 = 0;
    config_ptr->max_qp_allowed               = 63;
    config_ptr->min_qp_allowed               = MIN_QP_AUTO;
    config_ptr->aq_mode                      = 2;
    config_ptr->enc_mode                     = ENC_M8;
    config_ptr->intra_period_length          = -2;
    config_ptr->multiply_keyint              = false;
    config_ptr->intra_refresh_type           = 2;
    config_ptr->hierarchical_levels          = HIERARCHICAL_LEVELS_AUTO;
    config_ptr->pred_structure               = RANDOM_ACCESS;
    config_ptr->enable_dlf_flag              = 1;
    config_ptr->cdef_level                   = DEFAULT;
    config_ptr->enable_restoration_filtering = DEFAULT;
    config_ptr->enable_mfmv                  = DEFAULT;
    config_ptr->enable_dg                    = 1;
    config_ptr->fast_decode                  = 0;
    config_ptr->encoder_color_format         = EB_YUV420;
    config_ptr->rtc                          = 0;
    // Rate control options
    // Set the default value toward more flexible rate allocation
    config_ptr->vbr_min_section_pct      = 0;
    config_ptr->vbr_max_section_pct      = 2000;
    config_ptr->under_shoot_pct          = (uint32_t)DEFAULT;
    config_ptr->over_shoot_pct           = (uint32_t)DEFAULT;
    config_ptr->mbr_over_shoot_pct       = 50;
    config_ptr->gop_constraint_rc        = 0;
    config_ptr->maximum_buffer_size_ms   = 1000; // default settings for CBR
    config_ptr->starting_buffer_level_ms = 600; // default settings for CBR
    config_ptr->optimal_buffer_level_ms  = 600; // default settings for CBR
    config_ptr->recode_loop              = ALLOW_RECODE_DEFAULT;
    config_ptr->screen_content_mode      = 2;

    // Annex A parameters
    config_ptr->profile = 0;
    config_ptr->tier    = 0;
    config_ptr->level   = 0;

    // Film grain denoising
    config_ptr->film_grain_denoise_strength = 0;
    config_ptr->film_grain_denoise_apply    = 0;

    // CPU Flags
    config_ptr->use_cpu_flags = EB_CPU_FLAGS_ALL;

    // Channel info
    config_ptr->level_of_parallelism = 0;

    // Debug info
    config_ptr->recon_enabled = 0;

    // Alt-Ref default values
    config_ptr->enable_tf       = 1;
    config_ptr->enable_overlays = false;
    config_ptr->tune            = 1;
    // Super-resolution default values
    config_ptr->superres_mode      = SUPERRES_NONE;
    config_ptr->superres_denom     = SCALE_NUMERATOR;
    config_ptr->superres_kf_denom  = SCALE_NUMERATOR;
    config_ptr->superres_qthres    = 43; // random threshold, change
    config_ptr->superres_kf_qthres = 43; // random threshold, change

    // Reference Scaling default values
    config_ptr->resize_mode     = RESIZE_NONE;
    config_ptr->resize_denom    = SCALE_NUMERATOR;
    config_ptr->resize_kf_denom = SCALE_NUMERATOR;

    // Color description default values
    config_ptr->color_primaries          = 2;
    config_ptr->transfer_characteristics = 2;
    config_ptr->matrix_coefficients      = 2;
    config_ptr->color_range              = EB_CR_STUDIO_RANGE;
    config_ptr->chroma_sample_position   = EB_CSP_UNKNOWN;
    config_ptr->pass                     = 0;
    memset(&config_ptr->mastering_display, 0, sizeof(config_ptr->mastering_display));
    memset(&config_ptr->content_light_level, 0, sizeof(config_ptr->content_light_level));

    // Switch frame default values
    config_ptr->sframe_dist      = 0;
    config_ptr->sframe_mode      = SFRAME_NEAREST_BASE;
    config_ptr->force_key_frames = 0;

    // Quant Matrices (QM)
    config_ptr->enable_qm           = 0;
    config_ptr->min_qm_level        = 8;
    config_ptr->max_qm_level        = 15;
    config_ptr->min_chroma_qm_level = 8;
    config_ptr->max_chroma_qm_level = 15;

    config_ptr->startup_mg_size                   = 0;
    config_ptr->startup_qp_offset                 = 0;
    config_ptr->frame_scale_evts.evt_num          = 0;
    config_ptr->frame_scale_evts.resize_denoms    = NULL;
    config_ptr->frame_scale_evts.resize_kf_denoms = NULL;
    config_ptr->frame_scale_evts.start_frame_nums = NULL;
    config_ptr->enable_roi_map                    = false;
    config_ptr->fgs_table                         = NULL;
    config_ptr->enable_variance_boost             = false;
    config_ptr->variance_boost_strength           = 2;
    config_ptr->variance_octile                   = 5;
    config_ptr->tf_strength                       = 3;
    config_ptr->variance_boost_curve              = 0;
    config_ptr->luminance_qp_bias                 = 0;
    config_ptr->sharpness                         = 0;
    config_ptr->lossless                          = false;
    config_ptr->avif                              = false;
    config_ptr->qp_scale_compress_strength        = 0;
    config_ptr->sframe_posi.sframe_num            = 0;
    config_ptr->sframe_posi.sframe_posis          = NULL;
    config_ptr->sframe_posi.sframe_qp_num         = 0;
    config_ptr->sframe_posi.sframe_qps            = NULL;
    config_ptr->sframe_posi.sframe_qp_offsets     = NULL;
    config_ptr->sframe_qp                         = 0;
    config_ptr->sframe_qp_offset                  = 0;
    config_ptr->adaptive_film_grain               = true;
    config_ptr->max_tx_size                       = 64;
    config_ptr->extended_crf_qindex_offset        = 0;
    config_ptr->ac_bias                           = 0.0;
    return return_error;
}

static const char* tier_to_str(unsigned in) {
    if (!in) {
        return "(auto)";
    }
    static char ret[11];
    snprintf(ret, 11, "%u", in);
    return ret;
}

static const char* level_to_str(unsigned in) {
    if (!in) {
        return "(auto)";
    }
    static char ret[313];
    snprintf(ret, 313, "%.1f", in / 10.0);
    return ret;
}

static double get_extended_crf(EbSvtAv1EncConfiguration* config_ptr) {
    return (double)config_ptr->qp + (double)config_ptr->extended_crf_qindex_offset / 4;
}

//#define DEBUG_BUFFERS
void svt_av1_print_lib_params(SequenceControlSet* scs) {
    EbSvtAv1EncConfiguration* config = &scs->static_config;

    SVT_INFO("-------------------------------------------\n");
    if (config->pass == ENC_FIRST_PASS) {
        SVT_INFO("SVT [config]: preset \t\t\t\t\t\t\t: Pass 1\n");
    } else {
        SVT_INFO("SVT [config]: %s\ttier %s\tlevel %s\n",
                 config->profile == MAIN_PROFILE               ? "main profile"
                     : config->profile == HIGH_PROFILE         ? "high profile"
                     : config->profile == PROFESSIONAL_PROFILE ? "professional profile"
                                                               : "Unknown profile",
                 tier_to_str(config->tier),
                 level_to_str(config->level));
        SVT_INFO(
            "SVT [config]: width / height / fps numerator / fps denominator \t\t: %d / %d / %d / "
            "%d\n",
            config->source_width,
            config->source_height,
            config->frame_rate_numerator,
            config->frame_rate_denominator);
        SVT_INFO(
            "SVT [config]: bit-depth / color format \t\t\t\t\t: %d / "
            "%s\n",
            config->encoder_bit_depth,
            config->encoder_color_format == EB_YUV400       ? "YUV400"
                : config->encoder_color_format == EB_YUV420 ? "YUV420"
                : config->encoder_color_format == EB_YUV422 ? "YUV422"
                : config->encoder_color_format == EB_YUV444 ? "YUV444"
                                                            : "Unknown color format");

        SVT_INFO("SVT [config]: preset / tune / pred struct \t\t\t\t\t: %d / %s / %s\n",
                 config->enc_mode,
                 config->tune == TUNE_VQ            ? "VQ"
                     : config->tune == TUNE_PSNR    ? "PSNR"
                     : config->tune == TUNE_SSIM    ? "SSIM"
                     : config->tune == TUNE_MS_SSIM ? "MS_SSIM"
                                                    : "IQ",
                 config->pred_structure == LOW_DELAY           ? "low delay"
                     : config->pred_structure == RANDOM_ACCESS ? "random access"
                                                               : "Unknown pred structure");
        SVT_INFO(
            "SVT [config]: gop size / mini-gop size / key-frame type \t\t\t: "
            "%d / %d / %s\n",
            config->intra_period_length + 1,
            (1 << config->hierarchical_levels),
            config->intra_refresh_type == SVT_AV1_FWDKF_REFRESH    ? "FWD key frame"
                : config->intra_refresh_type == SVT_AV1_KF_REFRESH ? "key frame"
                                                                   : "Unknown key frame type");
        if (config->lossless) {
            SVT_INFO("SVT [config]: BRC mode\t\t\t\t\t\t\t: Lossless Coding \n");
        } else {
            switch (config->rate_control_mode) {
            case SVT_AV1_RC_MODE_CQP_OR_CRF:
                if (config->max_bit_rate) {
                    SVT_INFO(
                        "SVT [config]: BRC mode / %s / max bitrate (kbps)\t\t\t: %s / %.2f / "
                        "%d\n",
                        scs->tpl || scs->static_config.enable_variance_boost ? "rate factor" : "CQP Assignment",
                        scs->tpl || scs->static_config.enable_variance_boost ? "capped CRF" : "CQP",
                        get_extended_crf(config),
                        (int)config->max_bit_rate / 1000);
                } else {
                    SVT_INFO("SVT [config]: BRC mode / %s \t\t\t\t\t: %s / %.2f \n",
                             scs->tpl || scs->static_config.enable_variance_boost ? "rate factor" : "CQP Assignment",
                             scs->tpl || scs->static_config.enable_variance_boost ? "CRF" : "CQP",
                             get_extended_crf(config));
                }
                break;
            case SVT_AV1_RC_MODE_VBR:
                SVT_INFO("SVT [config]: BRC mode / target bitrate (kbps)\t\t\t\t: VBR / %d \n",
                         (int)config->target_bit_rate / 1000);
                break;
            case SVT_AV1_RC_MODE_CBR:
                SVT_INFO(
                    "SVT [config]: BRC mode / target bitrate (kbps)\t\t\t\t: CBR "
                    "/ %d\n",
                    (int)config->target_bit_rate / 1000);
                break;
            }
        }
        if (config->rate_control_mode != SVT_AV1_RC_MODE_CBR) {
            if (!config->enable_variance_boost) {
                SVT_INFO("SVT [config]: AQ mode / Variance Boost \t\t\t\t\t: %d / %d\n",
                         config->aq_mode,
                         config->enable_variance_boost);
            } else {
                SVT_INFO("SVT [config]: AQ mode / Variance Boost strength / octile / curve \t\t: %d / %d / %d / %d\n",
                         config->aq_mode,
                         config->variance_boost_strength,
                         config->variance_octile,
                         config->variance_boost_curve);
            }
        }

        if (config->film_grain_denoise_strength != 0) {
            if (config->adaptive_film_grain) {
                SVT_INFO(
                    "SVT [config]: film grain synth / denoising / level / adaptive blocksize \t: %d / %d / %d / True\n",
                    1,
                    config->film_grain_denoise_apply,
                    config->film_grain_denoise_strength);
            } else {
                SVT_INFO(
                    "SVT [config]: film grain synth / denoising / level / adaptive blocksize \t: %d / %d / %d / "
                    "False\n",
                    1,
                    config->film_grain_denoise_apply,
                    config->film_grain_denoise_strength);
            }
        }
        SVT_INFO("SVT [config]: sharpness / luminance-based QP bias \t\t\t\t: %d / %d\n",
                 config->sharpness,
                 config->luminance_qp_bias);

        switch (config->enable_tf) {
        case 1:
            if (config->tf_strength != 3) {
                SVT_INFO("SVT [config]: temporal filtering strength \t\t\t\t\t: %d\n", config->tf_strength);
            }
            break;
        case 2:
            SVT_INFO("SVT [config]: temporal filtering strength \t\t\t\t\t: auto\n");
            break;
        default:
            break;
        }

        SVT_INFO("SVT [config]: QP scale compress strength \t\t\t\t\t: %d\n", config->qp_scale_compress_strength);

        if (config->ac_bias) {
            SVT_INFO("SVT [config]: AC Bias Strength \t\t\t\t\t\t: %.2f\n", config->ac_bias);
        }
    }
#if DEBUG_BUFFERS
    SVT_INFO("SVT [config]: INPUT / OUTPUT \t\t\t\t\t\t\t: %d / %d\n",
             scs->input_buffer_fifo_init_count,
             scs->output_stream_buffer_fifo_init_count);
    SVT_INFO("SVT [config]: CPCS / PAREF / REF / ME\t\t\t\t\t\t: %d / %d / %d / %d\n",
             scs->picture_control_set_pool_init_count_child,
             scs->pa_reference_picture_buffer_init_count,
             scs->reference_picture_buffer_init_count,
             scs->me_pool_init_count);
    SVT_INFO("SVT [config]: ME_SEG_W / ME_SEG_H \t\t\t: %u / %u\n",
             scs->me_segment_col_count_array,
             scs->me_segment_row_count_array);
    SVT_INFO("SVT [config]: ENC_DEC_SEG_W / ENC_DEC_SEG_H \t\t\t: %u / %u\n",
             scs->enc_dec_segment_col_count_array,
             scs->enc_dec_segment_row_count_array);
    SVT_INFO(
        "SVT [config]: PA_P / ME_P / SBO_P / MDC_P / ED_P / EC_P \t\t\t: %d / %d / %d / %d / %d / "
        "%d\n",
        scs->picture_analysis_process_init_count,
        scs->motion_estimation_process_init_count,
        scs->source_based_operations_process_init_count,
        scs->mode_decision_configuration_process_init_count,
        scs->enc_dec_process_init_count,
        scs->entropy_coding_process_init_count);
    SVT_INFO("SVT [config]: DLF_P / CDEF_P / REST_P \t\t\t\t\t\t: %d / %d / %d\n",
             scs->dlf_process_init_count,
             scs->cdef_process_init_count,
             scs->rest_process_init_count);
#endif
    SVT_INFO("-------------------------------------------\n");

    fflush(stdout);
}

/**********************************
* Parse Single Parameter
**********************************/

static EbErrorType str_to_int64(const char* nptr, int64_t* out, char** nextptr) {
    char*   endptr;
    int64_t val;

    val = strtoll(nptr, &endptr, 0);

    if (endptr == nptr || (!nextptr && *endptr)) {
        return EB_ErrorBadParameter;
    }

    *out = val;
    if (nextptr) {
        *nextptr = endptr;
    }
    return EB_ErrorNone;
}

static EbErrorType str_to_int(const char* nptr, int32_t* out, char** nextptr) {
    char*   endptr;
    int32_t val;

    val = strtol(nptr, &endptr, 0);

    if (endptr == nptr || (!nextptr && *endptr)) {
        return EB_ErrorBadParameter;
    }

    *out = val;
    if (nextptr) {
        *nextptr = endptr;
    }
    return EB_ErrorNone;
}

static EbErrorType str_to_uint64(const char* nptr, uint64_t* out, char** nextptr) {
    char*    endptr;
    uint64_t val;

    if (strtoll(nptr, NULL, 0) < 0) {
        return EB_ErrorBadParameter;
    }

    val = strtoull(nptr, &endptr, 0);

    if (endptr == nptr || (!nextptr && *endptr)) {
        return EB_ErrorBadParameter;
    }

    *out = val;
    if (nextptr) {
        *nextptr = endptr;
    }
    return EB_ErrorNone;
}

static EbErrorType str_to_uint(const char* nptr, uint32_t* out, char** nextptr) {
    char*    endptr;
    uint32_t val;

    if (strtol(nptr, NULL, 0) < 0) {
        return EB_ErrorBadParameter;
    }

    val = strtoul(nptr, &endptr, 0);

    if (endptr == nptr || (!nextptr && *endptr)) {
        return EB_ErrorBadParameter;
    }

    *out = val;
    if (nextptr) {
        *nextptr = endptr;
    }
    return EB_ErrorNone;
}

static EbErrorType str_to_int8(const char* nptr, int8_t* out, char** nextptr) {
    char*   endptr;
    int32_t val;

    val = strtol(nptr, &endptr, 0);

    if (endptr == nptr || (!nextptr && *endptr)) {
        return EB_ErrorBadParameter;
    }

    // check for the range
    if (val < INT8_MIN || val > INT8_MAX) {
        return EB_ErrorBadParameter;
    }

    *out = (int8_t)val;
    if (nextptr) {
        *nextptr = endptr;
    }
    return EB_ErrorNone;
}

static EbErrorType str_to_uint8(const char* nptr, uint8_t* out, char** nextptr) {
    char*    endptr;
    uint32_t val;

    if (strtol(nptr, NULL, 0) < 0) {
        return EB_ErrorBadParameter;
    }

    val = strtoul(nptr, &endptr, 0);

    if (endptr == nptr || (!nextptr && *endptr)) {
        return EB_ErrorBadParameter;
    }

    // check for the range
    if (val > UINT8_MAX) {
        return EB_ErrorBadParameter;
    }

    *out = (uint8_t)val;
    if (nextptr) {
        *nextptr = endptr;
    }
    return EB_ErrorNone;
}

#define str_to_int32 str_to_int
#define str_to_uint32 str_to_uint

static EbErrorType str_to_double(const char* nptr, double* out, char** nextptr) {
    char*  endptr;
    double val;

    val = strtod(nptr, &endptr);

    if (endptr == nptr || (!nextptr && *endptr)) {
        return EB_ErrorBadParameter;
    }

    *out = val;
    if (nextptr) {
        *nextptr = endptr;
    }
    return EB_ErrorNone;
}

//assume the input list of values are in the format of "[v1,v2,v3,...]"
#define PARSE_LIST(list_type)                                                                    \
    static EbErrorType parse_list_##list_type(const char* nptr, list_type##_t* list, size_t n) { \
        const char* ptr = nptr;                                                                  \
        char*       endptr;                                                                      \
        size_t      i = 0;                                                                       \
        memset(list, 0, n * sizeof(*list));                                                      \
        while (*ptr) {                                                                           \
            if (*ptr == '[' || *ptr == ']') {                                                    \
                ptr++;                                                                           \
                continue;                                                                        \
            }                                                                                    \
            list_type##_t rawval;                                                                \
            EbErrorType   err = str_to_##list_type(ptr, &rawval, &endptr);                       \
            if (err != EB_ErrorNone)                                                             \
                return err;                                                                      \
            if (i >= n) {                                                                        \
                return EB_ErrorBadParameter;                                                     \
            } else if (*endptr == ',' || *endptr == ']') {                                       \
                endptr++;                                                                        \
            } else if (*endptr) {                                                                \
                return EB_ErrorBadParameter;                                                     \
            }                                                                                    \
            list[i++] = rawval;                                                                  \
            ptr       = endptr;                                                                  \
        }                                                                                        \
        return EB_ErrorNone;                                                                     \
    }
PARSE_LIST(int8)
PARSE_LIST(uint8)
PARSE_LIST(int32)
PARSE_LIST(uint32)
PARSE_LIST(uint64)

static uint32_t count_params(const char* nptr) {
    const char* ptr = nptr;
    char*       endptr;
    uint32_t    i = 0;
    while (*ptr) {
        if (*ptr == '[' || *ptr == ']') {
            ptr++;
            continue;
        }

        strtoll(ptr, &endptr, 10);
        if (*endptr == ',' || *endptr == ']') {
            endptr++;
        } else if (*endptr) {
            return i;
        }
        i++;
        ptr = endptr;
    }
    return i;
}

#ifdef _MSC_VER
#define strcasecmp _stricmp
#endif
static EbErrorType str_to_bool(const char* nptr, bool* out) {
    bool val;
    if (!strcmp(nptr, "1") || !strcasecmp(nptr, "true") || !strcasecmp(nptr, "yes")) {
        val = true;
    } else if (!strcmp(nptr, "0") || !strcasecmp(nptr, "false") || !strcasecmp(nptr, "no")) {
        val = false;
    } else {
        return EB_ErrorBadParameter;
    }

    *out = val;
    return EB_ErrorNone;
}

static EbErrorType str_to_crf(const char* nptr, EbSvtAv1EncConfiguration* config_struct) {
    double      crf;
    EbErrorType return_error;

    return_error = str_to_double(nptr, &crf, NULL);

    if (return_error == EB_ErrorBadParameter) {
        return return_error;
    }
    if (crf < 0) {
        return EB_ErrorBadParameter;
    }

    uint32_t extended_q_index           = (uint32_t)(crf * 4);
    uint32_t qp                         = AOMMIN(MAX_QP_VALUE, (uint32_t)crf);
    uint32_t extended_crf_qindex_offset = extended_q_index - qp * 4;

    config_struct->qp                         = qp;
    config_struct->rate_control_mode          = SVT_AV1_RC_MODE_CQP_OR_CRF;
    config_struct->aq_mode                    = 2;
    config_struct->extended_crf_qindex_offset = extended_crf_qindex_offset;

    return EB_ErrorNone;
}

static EbErrorType str_to_keyint(const char* nptr, int32_t* out, bool* multi) {
    char*      suff;
    const long keyint = strtol(nptr, &suff, 0);

    if (keyint > INT32_MAX || keyint < -2) {
        return EB_ErrorBadParameter;
    }

    switch (*suff) {
    case 's':
        // signal we need to multiply keyint * frame_rate
        *multi = true;
        *out   = keyint;
        break;
    case '\0':
        *multi = false;
        *out   = keyint < 0 ? keyint : keyint - 1;
        break;
    default:
        // else leave as untouched, we have an invalid keyint
        SVT_ERROR("Invalid keyint value: %s\n", nptr);
        return EB_ErrorBadParameter;
    }

    return EB_ErrorNone;
}

static EbErrorType str_to_bitrate(const char* nptr, uint32_t* out) {
    char*        suff;
    const double bitrate = strtod(nptr, &suff);

    if (bitrate < 0 || bitrate > UINT32_MAX) {
        SVT_ERROR("Invalid bitrate value: %s\n", nptr);
        return EB_ErrorBadParameter;
    }

    switch (*suff) {
    case 'b':
    case 'B':
        *out = (uint32_t)bitrate;
        break;
    case '\0':
    case 'k':
    case 'K':
        *out = (uint32_t)(1000 * bitrate);
        break;
    case 'm':
    case 'M':
        *out = (uint32_t)(1000000 * bitrate);
        break;
    default:
        return EB_ErrorBadParameter;
    }
    if (*out > 100000000) {
        *out = 100000000;
        SVT_WARN("Bitrate value: %s has been set to 100000000\n", nptr);
    }
    return EB_ErrorNone;
}

static EbErrorType str_to_profile(const char* nptr, EbAv1SeqProfile* out) {
    const struct {
        const char*     name;
        EbAv1SeqProfile profile;
    } profiles[] = {
        {"main", MAIN_PROFILE},
        {"high", HIGH_PROFILE},
        {"professional", PROFESSIONAL_PROFILE},
    };

    const size_t profiles_size = sizeof(profiles) / sizeof(profiles[0]);

    for (size_t i = 0; i < profiles_size; i++) {
        if (!strcmp(nptr, profiles[i].name)) {
            *out = profiles[i].profile;
            return EB_ErrorNone;
        }
    }

    return EB_ErrorBadParameter;
}

static EbErrorType str_to_color_fmt(const char* nptr, EbColorFormat* out) {
    const struct {
        const char*   name;
        EbColorFormat fmt;
    } color_formats[] = {
        {"mono", EB_YUV400},
        {"400", EB_YUV400},
        {"420", EB_YUV420},
        {"422", EB_YUV422},
        {"444", EB_YUV444},
    };

    const size_t color_format_size = sizeof(color_formats) / sizeof(color_formats[0]);

    for (size_t i = 0; i < color_format_size; i++) {
        if (!strcmp(nptr, color_formats[i].name)) {
            *out = color_formats[i].fmt;
            return EB_ErrorNone;
        }
    }

    return EB_ErrorBadParameter;
}

static EbErrorType str_to_intra_rt(const char* nptr, SvtAv1IntraRefreshType* out) {
    const struct {
        const char*            name;
        SvtAv1IntraRefreshType type;
    } refresh_types[] = {
        {"cra", SVT_AV1_FWDKF_REFRESH},
        {"fwdkf", SVT_AV1_FWDKF_REFRESH},
        {"idr", SVT_AV1_KF_REFRESH},
        {"kf", SVT_AV1_KF_REFRESH},
    };

    const size_t refresh_type_size = sizeof(refresh_types) / sizeof(refresh_types[0]);

    for (size_t i = 0; i < refresh_type_size; i++) {
        if (!strcmp(nptr, refresh_types[i].name)) {
            *out = refresh_types[i].type;
            return EB_ErrorNone;
        }
    }

    return EB_ErrorBadParameter;
}

static EbErrorType str_to_asm(const char* nptr, EbCpuFlags* out) {
    // need to handle numbers in here since the numbers to no match the
    // internal representation
    const struct {
        const char* name;
        EbCpuFlags  flag;
    } simds[] = {
        {"c", 0},
        {"0", 0},
#ifdef ARCH_X86_64
        {"mmx", (EB_CPU_FLAGS_MMX << 1) - 1},
        {"1", (EB_CPU_FLAGS_MMX << 1) - 1},
        {"sse", (EB_CPU_FLAGS_SSE << 1) - 1},
        {"2", (EB_CPU_FLAGS_SSE << 1) - 1},
        {"sse2", (EB_CPU_FLAGS_SSE2 << 1) - 1},
        {"3", (EB_CPU_FLAGS_SSE2 << 1) - 1},
        {"sse3", (EB_CPU_FLAGS_SSE3 << 1) - 1},
        {"4", (EB_CPU_FLAGS_SSE3 << 1) - 1},
        {"ssse3", (EB_CPU_FLAGS_SSSE3 << 1) - 1},
        {"5", (EB_CPU_FLAGS_SSSE3 << 1) - 1},
        {"sse4_1", (EB_CPU_FLAGS_SSE4_1 << 1) - 1},
        {"6", (EB_CPU_FLAGS_SSE4_1 << 1) - 1},
        {"sse4_2", (EB_CPU_FLAGS_SSE4_2 << 1) - 1},
        {"7", (EB_CPU_FLAGS_SSE4_2 << 1) - 1},
        {"avx", (EB_CPU_FLAGS_AVX << 1) - 1},
        {"8", (EB_CPU_FLAGS_AVX << 1) - 1},
        {"avx2", (EB_CPU_FLAGS_AVX2 << 1) - 1},
        {"9", (EB_CPU_FLAGS_AVX2 << 1) - 1},
        {"avx512", (EB_CPU_FLAGS_AVX512VL << 1) - 1},
        {"10", (EB_CPU_FLAGS_AVX512VL << 1) - 1},
        {"avx512icl", (EB_CPU_FLAGS_AVX512ICL << 1) - 1},
        {"11", (EB_CPU_FLAGS_AVX512ICL << 1) - 1},
#elif defined(ARCH_AARCH64)
        {"neon", (EB_CPU_FLAGS_NEON << 1) - 1},
        {"1", (EB_CPU_FLAGS_NEON << 1) - 1},
        {"crc32", (EB_CPU_FLAGS_ARM_CRC32 << 1) - 1},
        {"2", (EB_CPU_FLAGS_ARM_CRC32 << 1) - 1},
        {"neon_dotprod", (EB_CPU_FLAGS_NEON_DOTPROD << 1) - 1},
        {"3", (EB_CPU_FLAGS_NEON_DOTPROD << 1) - 1},
        {"neon_i8mm", (EB_CPU_FLAGS_NEON_I8MM << 1) - 1},
        {"4", (EB_CPU_FLAGS_NEON_I8MM << 1) - 1},
        {"sve", (EB_CPU_FLAGS_SVE << 1) - 1},
        {"5", (EB_CPU_FLAGS_SVE << 1) - 1},
        {"sve2", (EB_CPU_FLAGS_SVE2 << 1) - 1},
        {"6", (EB_CPU_FLAGS_SVE2 << 1) - 1},
#endif
        {"max", EB_CPU_FLAGS_ALL},
        {"100", EB_CPU_FLAGS_ALL},
    };
    const size_t simds_size = sizeof(simds) / sizeof(simds[0]);

    for (size_t i = 0; i < simds_size; i++) {
        if (!strcmp(nptr, simds[i].name)) {
            *out = simds[i].flag;
            return EB_ErrorNone;
        }
    }

    *out = EB_CPU_FLAGS_INVALID;

    return EB_ErrorBadParameter;
}

static EbErrorType str_to_color_primaries(const char* nptr, EbColorPrimaries* out) {
    const struct {
        const char*      name;
        EbColorPrimaries primaries;
    } color_primaries[] = {
        {"bt709", EB_CICP_CP_BT_709},
        {"bt470m", EB_CICP_CP_BT_470_M},
        {"bt470bg", EB_CICP_CP_BT_470_B_G},
        {"bt601", EB_CICP_CP_BT_601},
        {"smpte240", EB_CICP_CP_SMPTE_240},
        {"film", EB_CICP_CP_GENERIC_FILM},
        {"bt2020", EB_CICP_CP_BT_2020},
        {"xyz", EB_CICP_CP_XYZ},
        {"smpte431", EB_CICP_CP_SMPTE_431},
        {"smpte432", EB_CICP_CP_SMPTE_432},
        {"ebu3213", EB_CICP_CP_EBU_3213},
    };

    const size_t color_primaries_size = sizeof(color_primaries) / sizeof(color_primaries[0]);

    for (size_t i = 0; i < color_primaries_size; i++) {
        if (!strcmp(nptr, color_primaries[i].name)) {
            *out = color_primaries[i].primaries;
            return EB_ErrorNone;
        }
    }

    return EB_ErrorBadParameter;
}

static EbErrorType str_to_transfer_characteristics(const char* nptr, EbTransferCharacteristics* out) {
    const struct {
        const char*               name;
        EbTransferCharacteristics tfc;
    } transfer_characteristics[] = {
        {"bt709", EB_CICP_TC_BT_709},
        {"bt470m", EB_CICP_TC_BT_470_M},
        {"bt470bg", EB_CICP_TC_BT_470_B_G},
        {"bt601", EB_CICP_TC_BT_601},
        {"smpte240", EB_CICP_TC_SMPTE_240},
        {"linear", EB_CICP_TC_LINEAR},
        {"log100", EB_CICP_TC_LOG_100},
        {"log100-sqrt10", EB_CICP_TC_LOG_100_SQRT10},
        {"iec61966", EB_CICP_TC_IEC_61966},
        {"bt1361", EB_CICP_TC_BT_1361},
        {"srgb", EB_CICP_TC_SRGB},
        {"bt2020-10", EB_CICP_TC_BT_2020_10_BIT},
        {"bt2020-12", EB_CICP_TC_BT_2020_12_BIT},
        {"smpte2084", EB_CICP_TC_SMPTE_2084},
        {"smpte428", EB_CICP_TC_SMPTE_428},
        {"hlg", EB_CICP_TC_HLG},
    };

    const size_t transfer_characteristics_size = sizeof(transfer_characteristics) / sizeof(transfer_characteristics[0]);

    for (size_t i = 0; i < transfer_characteristics_size; i++) {
        if (!strcmp(nptr, transfer_characteristics[i].name)) {
            *out = transfer_characteristics[i].tfc;
            return EB_ErrorNone;
        }
    }

    return EB_ErrorBadParameter;
}

static EbErrorType str_to_matrix_coefficients(const char* nptr, EbMatrixCoefficients* out) {
    const struct {
        const char*          name;
        EbMatrixCoefficients coeff;
    } matrix_coefficients[] = {
        {"identity", EB_CICP_MC_IDENTITY},
        {"bt709", EB_CICP_MC_BT_709},
        {"fcc", EB_CICP_MC_FCC},
        {"bt470bg", EB_CICP_MC_BT_470_B_G},
        {"bt601", EB_CICP_MC_BT_601},
        {"smpte240", EB_CICP_MC_SMPTE_240},
        {"ycgco", EB_CICP_MC_SMPTE_YCGCO},
        {"bt2020-ncl", EB_CICP_MC_BT_2020_NCL},
        {"bt2020-cl", EB_CICP_MC_BT_2020_CL},
        {"smpte2085", EB_CICP_MC_SMPTE_2085},
        {"chroma-ncl", EB_CICP_MC_CHROMAT_NCL},
        {"chroma-cl", EB_CICP_MC_CHROMAT_CL},
        {"ictcp", EB_CICP_MC_ICTCP},
    };

    const size_t matrix_coefficients_size = sizeof(matrix_coefficients) / sizeof(matrix_coefficients[0]);

    for (size_t i = 0; i < matrix_coefficients_size; i++) {
        if (!strcmp(nptr, matrix_coefficients[i].name)) {
            *out = matrix_coefficients[i].coeff;
            return EB_ErrorNone;
        }
    }

    return EB_ErrorBadParameter;
}

static EbErrorType str_to_color_range(const char* nptr, EbColorRange* out) {
    const struct {
        const char*  name;
        EbColorRange range;
    } color_range[] = {
        {"studio", EB_CR_STUDIO_RANGE},
        {"full", EB_CR_FULL_RANGE},
    };

    const size_t color_range_size = sizeof(color_range) / sizeof(color_range[0]);

    for (size_t i = 0; i < color_range_size; i++) {
        if (!strcmp(nptr, color_range[i].name)) {
            *out = color_range[i].range;
            return EB_ErrorNone;
        }
    }

    return EB_ErrorBadParameter;
}

static EbErrorType str_to_chroma_sample_position(const char* nptr, EbChromaSamplePosition* out) {
    const struct {
        const char*            name;
        EbChromaSamplePosition pos;
    } chroma_sample_positions[] = {
        {"unknown", EB_CSP_UNKNOWN},
        {"vertical", EB_CSP_VERTICAL},
        {"left", EB_CSP_VERTICAL},
        {"colocated", EB_CSP_COLOCATED},
        {"topleft", EB_CSP_COLOCATED},
    };

    const size_t chroma_sample_positions_size = sizeof(chroma_sample_positions) / sizeof(chroma_sample_positions[0]);

    for (size_t i = 0; i < chroma_sample_positions_size; i++) {
        if (!strcmp(nptr, chroma_sample_positions[i].name)) {
            *out = chroma_sample_positions[i].pos;
            return EB_ErrorNone;
        }
    }

    return EB_ErrorBadParameter;
}

static EbErrorType str_to_sframe_mode(const char* nptr, EbSFrameMode* out) {
    const struct {
        const char*  name;
        EbSFrameMode mode;
    } sframe_mode[] = {
        {"strict", SFRAME_STRICT_BASE},
        {"nearest", SFRAME_NEAREST_BASE},
        {"flexible", SFRAME_FLEXIBLE_BASE},
        {"decposi", SFRAME_DEC_POSI_BASE},
    };

    const size_t sframe_mode_size = sizeof(sframe_mode) / sizeof(sframe_mode[0]);

    for (size_t i = 0; i < sframe_mode_size; i++) {
        if (!strcmp(nptr, sframe_mode[i].name)) {
            *out = sframe_mode[i].mode;
            return EB_ErrorNone;
        }
    }

    return EB_ErrorBadParameter;
}

static EbErrorType str_to_rc_mode(const char* nptr, uint8_t* out, uint8_t* aq_mode) {
    // separate rc mode enum to distinguish between cqp and crf modes
    enum rc_modes {
        RC_MODE_ZERO = 0, // unique mode in case user passes a literal 0
        RC_MODE_CQP,
        RC_MODE_CRF,
        RC_MODE_VBR,
        RC_MODE_CBR,
        RC_MODE_INVALID,
    };

    const struct {
        const char* name;
        uint32_t    mode;
    } rc_mode[] = {
        {"0", RC_MODE_ZERO},
        {"1", RC_MODE_VBR},
        {"2", RC_MODE_CBR},
        {"cqp", RC_MODE_CQP},
        {"crf", RC_MODE_CRF},
        {"vbr", RC_MODE_VBR},
        {"cbr", RC_MODE_CBR},
    };

    const size_t rc_mode_size = sizeof(rc_mode) / sizeof(rc_mode[0]);

    enum rc_modes mode = RC_MODE_INVALID;

    for (size_t i = 0; i < rc_mode_size; i++) {
        if (!strcmp(nptr, rc_mode[i].name)) {
            mode = rc_mode[i].mode;
            break;
        }
    }

    switch (mode) {
    case RC_MODE_ZERO:
        *out = 0;
        break;
    case RC_MODE_CQP:
        *out     = SVT_AV1_RC_MODE_CQP_OR_CRF;
        *aq_mode = 0;
        break;
    case RC_MODE_CRF:
        *out     = SVT_AV1_RC_MODE_CQP_OR_CRF;
        *aq_mode = 2;
        break;
    case RC_MODE_VBR:
        *out = SVT_AV1_RC_MODE_VBR;
        break;
    case RC_MODE_CBR:
        *out = SVT_AV1_RC_MODE_CBR;
        break;
    default:
        SVT_ERROR("Invalid rc mode: %s\n", nptr);
        return EB_ErrorBadParameter;
    }
    return EB_ErrorNone;
}

static EbErrorType str_to_frm_resz_evts(const char* nptr, SvtAv1FrameScaleEvts* evts) {
    const uint32_t param_count = count_params(nptr);
    if ((evts->evt_num != 0 && evts->evt_num != param_count) || param_count == 0) {
        SVT_ERROR("Error: Size for the list passed to %s doesn't match %u\n", "frame-resz-events", evts->evt_num);
        return EB_ErrorBadParameter;
    }
    if (evts->start_frame_nums) {
        EB_FREE(evts->start_frame_nums);
    }
    EB_MALLOC(evts->start_frame_nums, param_count * sizeof(uint64_t));
    evts->evt_num = param_count;
    return parse_list_uint64(nptr, evts->start_frame_nums, param_count);
}

static EbErrorType str_to_resz_kf_denoms(const char* nptr, SvtAv1FrameScaleEvts* evts) {
    const uint32_t param_count = count_params(nptr);
    if ((evts->evt_num != 0 && evts->evt_num != param_count) || param_count == 0) {
        SVT_ERROR("Error: Size for the list passed to %s doesn't match %u\n", "frame-resz-kf-denoms", evts->evt_num);
        return EB_ErrorBadParameter;
    }
    if (evts->resize_kf_denoms) {
        EB_FREE(evts->resize_kf_denoms);
    }
    EB_MALLOC(evts->resize_kf_denoms, param_count * sizeof(uint32_t));
    evts->evt_num = param_count;
    return parse_list_uint32(nptr, evts->resize_kf_denoms, param_count);
}

static EbErrorType str_to_resz_denoms(const char* nptr, SvtAv1FrameScaleEvts* evts) {
    const uint32_t param_count = count_params(nptr);
    if ((evts->evt_num != 0 && evts->evt_num != param_count) || param_count == 0) {
        SVT_ERROR("Error: Size for the list passed to %s doesn't match %u\n", "frame-resz-denoms", evts->evt_num);
        return EB_ErrorBadParameter;
    }
    if (evts->resize_denoms) {
        EB_FREE(evts->resize_denoms);
    }
    EB_MALLOC(evts->resize_denoms, param_count * sizeof(uint32_t));
    evts->evt_num = param_count;
    return parse_list_uint32(nptr, evts->resize_denoms, param_count);
}

static EbErrorType str_to_sframe_posi(const char* nptr, SvtAv1SFramePositions* posis) {
    const uint32_t param_count = count_params(nptr);
    if ((posis->sframe_num != 0 && posis->sframe_num != param_count) || param_count == 0) {
        SVT_ERROR("Error: Size for the list passed to %s doesn't match %u\n", "sframe-posi", posis->sframe_num);
        return EB_ErrorBadParameter;
    }
    if (posis->sframe_posis) {
        EB_FREE(posis->sframe_posis);
    }
    EB_MALLOC(posis->sframe_posis, param_count * sizeof(uint64_t));
    posis->sframe_num = param_count;
    return parse_list_uint64(nptr, posis->sframe_posis, param_count);
}

static EbErrorType str_to_sframe_qp(const char* nptr, SvtAv1SFramePositions* posis, uint8_t* qp) {
    const uint32_t param_count = count_params(nptr);
    if ((posis->sframe_num != 0 && posis->sframe_num != param_count && param_count != 1) || param_count == 0) {
        SVT_ERROR("Error: Size for the list passed to %s doesn't match %u\n", "sframe-qp", posis->sframe_num);
        return EB_ErrorBadParameter;
    }
    if (posis->sframe_qps) {
        EB_FREE(posis->sframe_qps);
    }
    EB_MALLOC(posis->sframe_qps, param_count * sizeof(uint8_t));
    posis->sframe_qp_num = param_count;
    EbErrorType err      = parse_list_uint8(nptr, posis->sframe_qps, param_count);
    // check if the QP values are valid
    for (uint32_t i = 0; i < posis->sframe_qp_num; ++i) {
        if (posis->sframe_qps[i] < 1 || posis->sframe_qps[i] > 63) {
            SVT_ERROR("Error: Invalid S-Frame QP value. QPs must be [1 - 63]\n");
            EB_FREE(posis->sframe_qps);
            posis->sframe_qp_num = 0;
            err                  = EB_ErrorBadParameter;
        }
    }
    // If sframe-qp parameter contains only one value, apply it to all S-frames
    if (err == EB_ErrorNone && param_count == 1) {
        *qp = posis->sframe_qps[0];
    }
    return err;
}

static EbErrorType str_to_sframe_qp_offset(const char* nptr, SvtAv1SFramePositions* posis, int8_t* qp_offset) {
    const uint32_t param_count = count_params(nptr);
    if ((posis->sframe_num != 0 && posis->sframe_num != param_count && param_count != 1) || param_count == 0) {
        SVT_ERROR("Error: Size for the list passed to %s doesn't match %u\n", "sframe-qp-offset", posis->sframe_num);
        return EB_ErrorBadParameter;
    }
    if (posis->sframe_qp_offsets) {
        EB_FREE(posis->sframe_qp_offsets);
    }
    EB_MALLOC(posis->sframe_qp_offsets, param_count * sizeof(int8_t));
    posis->sframe_qp_num = param_count;
    EbErrorType err      = parse_list_int8(nptr, posis->sframe_qp_offsets, param_count);
    // check if the QP offset values are valid
    for (uint32_t i = 0; i < posis->sframe_qp_num; ++i) {
        if (posis->sframe_qp_offsets[i] < -63 || posis->sframe_qp_offsets[i] > 63) {
            SVT_ERROR("Error: Invalid S-Frame QP offset value. QP offsets must be [-63 - 63]\n");
            EB_FREE(posis->sframe_qp_offsets);
            posis->sframe_qp_num = 0;
            err                  = EB_ErrorBadParameter;
        }
    }
    // If sframe-qp-offset parameter contains only one value, apply it to all S-frames
    if (err == EB_ErrorNone && param_count == 1) {
        *qp_offset = posis->sframe_qp_offsets[0];
    }
    return err;
}

#define COLOR_OPT(par, opt)                                          \
    do {                                                             \
        if (!strcmp(name, par)) {                                    \
            return_error = str_to_##opt(value, &config_struct->opt); \
            if (return_error == EB_ErrorNone)                        \
                return return_error;                                 \
            uint32_t val;                                            \
            return_error = str_to_uint(value, &val, NULL);           \
            if (return_error == EB_ErrorNone)                        \
                config_struct->opt = val;                            \
            return return_error;                                     \
        }                                                            \
    } while (0)

#define COLOR_METADATA_OPT(par, opt)                                                                      \
    do {                                                                                                  \
        if (!strcmp(name, par))                                                                           \
            return svt_aom_parse_##opt(&config_struct->opt, value) ? EB_ErrorNone : EB_ErrorBadParameter; \
    } while (0)

EB_API EbErrorType svt_av1_enc_parse_parameter(EbSvtAv1EncConfiguration* config_struct, const char* name,
                                               const char* value) {
    if (config_struct == NULL || name == NULL || value == NULL) {
        return EB_ErrorBadParameter;
    }

    EbErrorType return_error = EB_ErrorBadParameter;

    if (!strcmp(name, "keyint")) {
        return str_to_keyint(value, &config_struct->intra_period_length, &config_struct->multiply_keyint);
    }

    if (!strcmp(name, "tbr")) {
        return str_to_bitrate(value, &config_struct->target_bit_rate);
    }

    if (!strcmp(name, "mbr")) {
        return str_to_bitrate(value, &config_struct->max_bit_rate);
    }

    // options updating more than one field
    if (!strcmp(name, "crf")) {
        return str_to_crf(value, config_struct);
    }

    if (!strcmp(name, "rc")) {
        return str_to_rc_mode(value, &config_struct->rate_control_mode, &config_struct->aq_mode);
    }

    // custom enum fields
    if (!strcmp(name, "profile")) {
        return str_to_profile(value, &config_struct->profile) == EB_ErrorBadParameter
            ? str_to_uint(value, (uint32_t*)&config_struct->profile, NULL)
            : EB_ErrorNone;
    }

    if (!strcmp(name, "color-format")) {
        return str_to_color_fmt(value, &config_struct->encoder_color_format) == EB_ErrorBadParameter
            ? str_to_uint(value, (uint32_t*)&config_struct->encoder_color_format, NULL)
            : EB_ErrorNone;
    }

    if (!strcmp(name, "irefresh-type")) {
        return str_to_intra_rt(value, &config_struct->intra_refresh_type) == EB_ErrorBadParameter
            ? str_to_uint(value, (uint32_t*)&config_struct->intra_refresh_type, NULL)
            : EB_ErrorNone;
    }

    if (!strcmp(name, "sframe-mode")) {
        return str_to_sframe_mode(value, &config_struct->sframe_mode) == EB_ErrorBadParameter
            ? str_to_uint(value, (uint32_t*)&config_struct->sframe_mode, NULL)
            : EB_ErrorNone;
    }

    if (!strcmp(name, "asm")) {
        return str_to_asm(value, &config_struct->use_cpu_flags);
    }

    COLOR_OPT("color-primaries", color_primaries);
    COLOR_OPT("transfer-characteristics", transfer_characteristics);
    COLOR_OPT("matrix-coefficients", matrix_coefficients);
    COLOR_OPT("color-range", color_range);
    COLOR_OPT("chroma-sample-position", chroma_sample_position);

    // custom struct fields
    COLOR_METADATA_OPT("mastering-display", mastering_display);
    COLOR_METADATA_OPT("content-light", content_light_level);

    // arrays
    if (!strcmp(name, "qindex-offsets")) {
        return parse_list_int32(value, config_struct->qindex_offsets, EB_MAX_TEMPORAL_LAYERS);
    }

    if (!strcmp(name, "chroma-qindex-offsets")) {
        return parse_list_int32(value, config_struct->chroma_qindex_offsets, EB_MAX_TEMPORAL_LAYERS);
    }

    if (!strcmp(name, "lambda-scale-factors")) {
        return parse_list_int32(value, config_struct->lambda_scale_factors, SVT_AV1_FRAME_UPDATE_TYPES);
    }

    if (!strcmp(name, "frame-resz-events")) {
        return str_to_frm_resz_evts(value, &config_struct->frame_scale_evts);
    }

    if (!strcmp(name, "frame-resz-kf-denoms")) {
        return str_to_resz_kf_denoms(value, &config_struct->frame_scale_evts);
    }

    if (!strcmp(name, "frame-resz-denoms")) {
        return str_to_resz_denoms(value, &config_struct->frame_scale_evts);
    }

    if (!strcmp(name, "sframe-posi")) {
        return str_to_sframe_posi(value, &config_struct->sframe_posi);
    }

    if (!strcmp(name, "sframe-qp")) {
        return str_to_sframe_qp(value, &config_struct->sframe_posi, &config_struct->sframe_qp);
    }

    if (!strcmp(name, "sframe-qp-offset")) {
        return str_to_sframe_qp_offset(value, &config_struct->sframe_posi, &config_struct->sframe_qp_offset);
    }

    // uint32_t fields
    const struct {
        const char* name;
        uint32_t*   out;
    } uint_opts[] = {
        {"w", &config_struct->source_width},
        {"width", &config_struct->source_width},
        {"h", &config_struct->source_height},
        {"height", &config_struct->source_height},
        {"q", &config_struct->qp},
        {"qp", &config_struct->qp},
        {"film-grain", &config_struct->film_grain_denoise_strength},
        {"hierarchical-levels", &config_struct->hierarchical_levels},
        {"tier", &config_struct->tier},
        {"level", &config_struct->level},
        {"lp", &config_struct->level_of_parallelism},
        {"fps-num", &config_struct->frame_rate_numerator},
        {"fps-denom", &config_struct->frame_rate_denominator},
        {"lookahead", &config_struct->look_ahead_distance},
        {"scd", &config_struct->scene_change_detection},
        {"max-qp", &config_struct->max_qp_allowed},
        {"min-qp", &config_struct->min_qp_allowed},
        {"minsection-pct", &config_struct->vbr_min_section_pct},
        {"maxsection-pct", &config_struct->vbr_max_section_pct},
        {"undershoot-pct", &config_struct->under_shoot_pct},
        {"overshoot-pct", &config_struct->over_shoot_pct},
        {"mbr-overshoot-pct", &config_struct->mbr_over_shoot_pct},
        {"recode-loop", &config_struct->recode_loop},
        {"enable-stat-report", &config_struct->stat_report},
        {"scm", &config_struct->screen_content_mode},
        {"input-depth", &config_struct->encoder_bit_depth},
        {"forced-max-frame-width", &config_struct->forced_max_frame_width},
        {"forced-max-frame-height", &config_struct->forced_max_frame_height},
    };

    const size_t uint_opts_size = sizeof(uint_opts) / sizeof(uint_opts[0]);

    for (size_t i = 0; i < uint_opts_size; i++) {
        if (!strcmp(name, uint_opts[i].name)) {
            return str_to_uint(value, uint_opts[i].out, NULL);
        }
    }

    // uint8_t fields
    const struct {
        const char* name;
        uint8_t*    out;
    } uint8_opts[] = {
        {"pred-struct", &config_struct->pred_structure},
        {"aq-mode", &config_struct->aq_mode},
        {"superres-mode", &config_struct->superres_mode},
        {"superres-qthres", &config_struct->superres_qthres},
        {"superres-kf-qthres", &config_struct->superres_kf_qthres},
        {"superres-denom", &config_struct->superres_denom},
        {"superres-kf-denom", &config_struct->superres_kf_denom},
        {"tune", &config_struct->tune},
        {"film-grain-denoise", &config_struct->film_grain_denoise_apply},
        {"enable-dlf", &config_struct->enable_dlf_flag},
        {"resize-mode", &config_struct->resize_mode},
        {"resize-denom", &config_struct->resize_denom},
        {"resize-kf-denom", &config_struct->resize_kf_denom},
        {"qm-min", &config_struct->min_qm_level},
        {"qm-max", &config_struct->max_qm_level},
        {"chroma-qm-min", &config_struct->min_chroma_qm_level},
        {"chroma-qm-max", &config_struct->max_chroma_qm_level},
        {"use-fixed-qindex-offsets", &config_struct->use_fixed_qindex_offsets},
        {"startup-mg-size", &config_struct->startup_mg_size},
        {"variance-boost-strength", &config_struct->variance_boost_strength},
        {"variance-octile", &config_struct->variance_octile},
        {"variance-boost-curve", &config_struct->variance_boost_curve},
        {"qp-scale-compress-strength", &config_struct->qp_scale_compress_strength},
        {"fast-decode", &config_struct->fast_decode},
        {"luminance-qp-bias", &config_struct->luminance_qp_bias},
        {"enable-tf", &config_struct->enable_tf},
        {"tf-strength", &config_struct->tf_strength},
        {"max-tx-size", &config_struct->max_tx_size},
    };

    const size_t uint8_opts_size = sizeof(uint8_opts) / sizeof(uint8_opts[0]);

    for (size_t i = 0; i < uint8_opts_size; i++) {
        if (!strcmp(name, uint8_opts[i].name)) {
            uint32_t val;
            return_error = str_to_uint(value, &val, NULL);
            if (return_error == EB_ErrorNone) {
                // add protection if the input param is roll-over
                if (val > 255) {
                    return EB_ErrorBadParameter;
                }
                *uint8_opts[i].out = val;
            }
            return return_error;
        }
    }

    // int64_t fields
    const struct {
        const char* name;
        int64_t*    out;
    } int64_opts[] = {
        {"buf-initial-sz", &config_struct->starting_buffer_level_ms},
        {"buf-optimal-sz", &config_struct->optimal_buffer_level_ms},
        {"buf-sz", &config_struct->maximum_buffer_size_ms},
    };

    const size_t int64_opts_size = sizeof(int64_opts) / sizeof(int64_opts[0]);

    for (size_t i = 0; i < int64_opts_size; i++) {
        if (!strcmp(name, int64_opts[i].name)) {
            return str_to_int64(value, int64_opts[i].out, NULL);
        }
    }

    // double fields
    const struct {
        const char* name;
        double*     out;
    } double_opts[] = {
        {"ac-bias", &config_struct->ac_bias},
    };

    const size_t double_opts_size = sizeof(double_opts) / sizeof(double_opts[0]);

    for (size_t i = 0; i < double_opts_size; i++) {
        if (!strcmp(name, double_opts[i].name)) {
            return str_to_double(value, double_opts[i].out, NULL);
        }
    }

    // int32_t fields
    const struct {
        const char* name;
        int32_t*    out;
    } int_opts[] = {
        {"key-frame-chroma-qindex-offset", &config_struct->key_frame_chroma_qindex_offset},
        {"key-frame-qindex-offset", &config_struct->key_frame_qindex_offset},
        {"luma-y-dc-qindex-offset", &config_struct->luma_y_dc_qindex_offset},
        {"chroma-u-dc-qindex-offset", &config_struct->chroma_u_dc_qindex_offset},
        {"chroma-u-ac-qindex-offset", &config_struct->chroma_u_ac_qindex_offset},
        {"chroma-v-dc-qindex-offset", &config_struct->chroma_v_dc_qindex_offset},
        {"chroma-v-ac-qindex-offset", &config_struct->chroma_v_ac_qindex_offset},
        {"pass", &config_struct->pass},
        {"enable-cdef", &config_struct->cdef_level},
        {"enable-restoration", &config_struct->enable_restoration_filtering},
        {"enable-mfmv", &config_struct->enable_mfmv},
        {"intra-period", &config_struct->intra_period_length},
        {"tile-rows", &config_struct->tile_rows},
        {"tile-columns", &config_struct->tile_columns},
        {"sframe-dist", &config_struct->sframe_dist},
    };

    const size_t int_opts_size = sizeof(int_opts) / sizeof(int_opts[0]);

    for (size_t i = 0; i < int_opts_size; i++) {
        if (!strcmp(name, int_opts[i].name)) {
            return str_to_int(value, int_opts[i].out, NULL);
        }
    }

    // int8_t fields
    const struct {
        const char* name;
        int8_t*     out;
    } int8_opts[] = {
        {"preset", &config_struct->enc_mode},
        {"sharpness", &config_struct->sharpness},
        {"startup-qp-offset", &config_struct->startup_qp_offset},
    };

    const size_t int8_opts_size = sizeof(int8_opts) / sizeof(int8_opts[0]);

    for (size_t i = 0; i < int8_opts_size; i++) {
        if (!strcmp(name, int8_opts[i].name)) {
            int32_t val;
            return_error = str_to_int(value, &val, NULL);
            if (return_error == EB_ErrorNone) {
                // add protection if the input param is roll-over
                if (val > 127 || val < -128) {
                    return EB_ErrorBadParameter;
                }
                *int8_opts[i].out = val;
            }
            return return_error;
        }
    }

    // bool fields
    const struct {
        const char* name;
        bool*       out;
    } bool_opts[] = {
        {"use-q-file", &config_struct->use_qp_file},
        {"enable-overlays", &config_struct->enable_overlays},
        {"enable-force-key-frames", &config_struct->force_key_frames},
#if CONFIG_ENABLE_QUANT_MATRIX
        {"enable-qm", &config_struct->enable_qm},
#endif
        {"enable-dg", &config_struct->enable_dg},
        {"gop-constraint-rc", &config_struct->gop_constraint_rc},
        {"enable-variance-boost", &config_struct->enable_variance_boost},
        {"lossless", &config_struct->lossless},
        {"avif", &config_struct->avif},
        {"rtc", &config_struct->rtc},
        {"adaptive-film-grain", &config_struct->adaptive_film_grain},
    };
    const size_t bool_opts_size = sizeof(bool_opts) / sizeof(bool_opts[0]);

    for (size_t i = 0; i < bool_opts_size; i++) {
        if (!strcmp(name, bool_opts[i].name)) {
            return str_to_bool(value, bool_opts[i].out);
        }
    }

    return return_error;
}
