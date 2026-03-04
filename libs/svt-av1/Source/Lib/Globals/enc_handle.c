/*
* Copyright(c) 2019 Intel Corporation
*
* This source code is subject to the terms of the BSD 3-Clause Clear License and
* the Alliance for Open Media Patent License 1.0. If the BSD 3-Clause Clear License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/
// SUMMARY
//   Contains the API component functions

/**************************************
 * Includes
 **************************************/
#include <stdlib.h>
#include <stdio.h>
#include <stdint.h>

#include "EbVersion.h"
#include "svt_threads.h"
#include "utility.h"
#include "enc_handle.h"
#include "enc_settings.h"
#include "pcs.h"
#include "pic_operators.h"
#include "reference_object.h"
#include "resource_coordination_process.h"
#include "pic_analysis_process.h"
#include "pd_process.h"
#include "me_process.h"
#include "initial_rc_process.h"
#include "src_ops_process.h"
#include "pic_manager_process.h"
#include "rc_process.h"
#include "md_config_process.h"
#include "enc_dec_process.h"
#include "ec_process.h"
#include "packetization_process.h"
#include "resource_coordination_results.h"
#include "pic_analysis_results.h"
#include "pd_results.h"
#include "me_results.h"
#include "initial_rc_results.h"
#include "pic_demux_results.h"
#include "rc_tasks.h"
#include "enc_dec_tasks.h"
#include "enc_dec_results.h"
#include "ec_results.h"
#include "pred_structure.h"
#include "rest_process.h"
#include "cdef_process.h"
#include "dlf_process.h"
#include "rc_results.h"
#include "definitions.h"
#include "metadata_handle.h"

#include "pack_unpack_c.h"
#include "enc_mode_config.h"

#ifdef ARCH_X86_64
#include <immintrin.h>
#endif
#include "svt_log.h"

#ifdef _WIN32
#include <windows.h>
#else
#include <errno.h>
#include <unistd.h>
#endif

#include "aom_dsp_rtcd.h"
#include "common_dsp_rtcd.h"

/**************************************
  * Defines
  **************************************/
// clang-format off
// Config Set Initial Count
#define EB_SequenceControlSetPoolInitCount              10
// Process Instantiation Initial Counts
#define EB_ResourceCoordinationProcessInitCount         1
#define EB_PictureDecisionProcessInitCount              1
#define EB_InitialRateControlProcessInitCount           1
#define EB_PictureManagerProcessInitCount               1
#define EB_RateControlProcessInitCount                  1
#define EB_PacketizationProcessInitCount                1

// Output Buffer Transfer Parameters
#define TPL_INPUT_PORT_SOP                              0
#define TPL_INPUT_PORT_TPL                              1
#define TPL_INPUT_PORT_INVALID                          -1
#define ENCDEC_INPUT_PORT_MDC                           0
#define ENCDEC_INPUT_PORT_ENCDEC                        1
#define ENCDEC_INPUT_PORT_INVALID                       -1

#define ENCODE_FIRST_PASS                               1
// clang-format on

uint8_t svt_aom_get_tpl_synthesizer_block_size(int8_t tpl_level, uint32_t picture_width, uint32_t picture_height);

/* count number of refs in a steady state MG*/
static uint16_t get_num_refs_in_one_mg(uint32_t hierarchical_levels, uint32_t referencing_scheme) {
    if (hierarchical_levels == 0) {
        return 1;
    }

    // All internal layer pics will be used as references.  Only top layer pics can be
    // not used as refs.
    uint16_t tot_refs = 1 << (hierarchical_levels - 1);

    // Top layer pics start at pic_idx 0 and every second pic is a top layer pic
    for (uint16_t pic_idx = 0; pic_idx < (uint32_t)(1 << hierarchical_levels); pic_idx += 2) {
        tot_refs += svt_aom_is_pic_used_as_ref(
            hierarchical_levels, hierarchical_levels, pic_idx, referencing_scheme, 0);
    }

    return tot_refs;
}

static const char* get_asm_level_name_str(EbCpuFlags cpu_flags) {
    // clang-format off
    static const struct {
        const char *name;
        EbCpuFlags  flags;
    } param_maps[] = {
        {"c",            0},
#ifdef ARCH_X86_64
        {"mmx",          EB_CPU_FLAGS_MMX},
        {"sse",          EB_CPU_FLAGS_SSE},
        {"sse2",         EB_CPU_FLAGS_SSE2},
        {"sse3",         EB_CPU_FLAGS_SSE3},
        {"ssse3",        EB_CPU_FLAGS_SSSE3},
        {"sse4_1",       EB_CPU_FLAGS_SSE4_1},
        {"sse4_2",       EB_CPU_FLAGS_SSE4_2},
        {"avx",          EB_CPU_FLAGS_AVX},
        {"avx2",         EB_CPU_FLAGS_AVX2},
        {"avx512",       EB_CPU_FLAGS_AVX512F},
        {"avx512icl",    EB_CPU_FLAGS_AVX512ICL},
#elif defined(ARCH_AARCH64)
        {"neon",         EB_CPU_FLAGS_NEON},
        {"crc32",        EB_CPU_FLAGS_ARM_CRC32},
        {"neon_dotprod", EB_CPU_FLAGS_NEON_DOTPROD},
        {"neon_i8mm",    EB_CPU_FLAGS_NEON_I8MM},
        {"sve",          EB_CPU_FLAGS_SVE},
        {"sve2",         EB_CPU_FLAGS_SVE2}
#endif
    };
    // clang-format on
    static const uint32_t para_map_size = sizeof(param_maps) / sizeof(param_maps[0]);

    for (int32_t i = para_map_size - 1; i >= 0; --i) {
        if (param_maps[i].flags & cpu_flags) {
            return param_maps[i].name;
        }
    }
    return "c";
}

//Get Number of logical processors
static uint32_t get_num_processors() {
#ifdef _WIN32
    return GetActiveProcessorCount(ALL_PROCESSOR_GROUPS);
#else
    return sysconf(_SC_NPROCESSORS_ONLN);
#endif
}

void    svt_aom_asm_set_convolve_asm_table(void);
void    svt_aom_asm_set_convolve_hbd_asm_table(void);
void    svt_aom_init_intra_dc_predictors_c_internal(void);
void    svt_aom_init_intra_predictors_internal(void);
void    svt_av1_init_me_luts(void);
uint8_t svt_aom_get_tpl_group_level(uint8_t tpl, int8_t enc_mode);
uint8_t svt_aom_set_tpl_group(PictureParentControlSet* pcs, uint8_t tpl_group_level, uint32_t source_width,
                              uint32_t source_height);

static void enc_switch_to_real_time() {
#if !defined(_WIN32) && !defined(__EMSCRIPTEN__)
    if (!geteuid()) {
        (void)pthread_setschedparam(pthread_self(), SCHED_FIFO, &(struct sched_param){.sched_priority = 99});
    }
#endif
}

typedef enum ParallelLevel {
    PARALLEL_LEVEL_1     = 1,
    PARALLEL_LEVEL_2     = 2,
    PARALLEL_LEVEL_3     = 3,
    PARALLEL_LEVEL_4     = 4,
    PARALLEL_LEVEL_5     = 5,
    PARALLEL_LEVEL_6     = 6,
    PARALLEL_LEVEL_COUNT = 7
} ParallelLevel;

// When level of parallelism is not specified, the level will be determined
// based on the core count of the machine
#define PARALLEL_LEVEL_1_RANGE 1 // single core count
#define PARALLEL_LEVEL_2_RANGE 2
#define PARALLEL_LEVEL_3_RANGE 5
#define PARALLEL_LEVEL_4_RANGE 11
#define PARALLEL_LEVEL_5_RANGE 23
#define PARALLEL_LEVEL_6_RANGE 47

//return max wavefronts in a given picture
static uint32_t get_max_wavefronts(uint32_t width, uint32_t height, uint32_t blk_size) {
    assert(width > 0 && height > 0);
    return (MIN(width, height) + (blk_size - 1)) / blk_size;
}

/*
* When the picture dimension is a single SB, must use a single segment (EncDec segments
* assume a width of at least 2 SBs)
*
* Return true if the pic dimension is a single SB width
*/
static bool is_pic_dimension_single_sb(uint32_t sb_size, uint16_t pic_dimension) {
    return ((pic_dimension + sb_size - 1) / sb_size) == 1;
}

/*********************************************************************************
* set_segments_numbers: Set the segment numbers for difference processes
***********************************************************************************/
void set_segments_numbers(SequenceControlSet* scs) {
    const uint32_t lp = scs->lp;

    scs->enc_dec_segment_row_count_array =
        (lp == PARALLEL_LEVEL_1 || is_pic_dimension_single_sb(scs->super_block_size, scs->max_input_luma_width)) ? 1
        : (scs->super_block_size == 128) ? ((scs->max_input_luma_height + 64) / 128)
                                         : ((scs->max_input_luma_height + 32) / 64);
    scs->enc_dec_segment_col_count_array =
        (lp == PARALLEL_LEVEL_1 || is_pic_dimension_single_sb(scs->super_block_size, scs->max_input_luma_height)) ? 1
        : (scs->super_block_size == 128) ? ((scs->max_input_luma_width + 64) / 128)
                                         : ((scs->max_input_luma_width + 32) / 64);

    scs->me_segment_row_count_array = scs->tf_segment_row_count = (lp == PARALLEL_LEVEL_1) ? 1
        : (((scs->max_input_luma_height + 32) / BLOCK_SIZE_64) < 6)                        ? 1
                                                                                           : 8;
    scs->me_segment_col_count_array = scs->tf_segment_column_count = (lp == PARALLEL_LEVEL_1) ? 1
        : (((scs->max_input_luma_width + 32) / BLOCK_SIZE_64) < 10)                           ? 1
                                                                                              : 6;

    // Jing:
    // A tile group can be consisted by 1 tile or NxM tiles.
    // Segments will be parallelized within a tile group
    // We can use tile group to control the threads/parallelism in ED stage
    // NOTE:1 col will have better perf for segments for large resolutions
    //by default, do not use tile prallel. to enable, one can set one tile-group per tile.
    scs->tile_group_col_count_array = 1;
    scs->tile_group_row_count_array = 1;

    // TPL processed in 64x64 blocks, so check width against 64x64 block size (even if SB is 128x128)
    scs->tpl_segment_row_count_array = (lp == PARALLEL_LEVEL_1 ||
                                        is_pic_dimension_single_sb(64, scs->max_input_luma_width))
        ? 1
        : ((scs->max_input_luma_height + 32) / 64);

    scs->tpl_segment_col_count_array = (lp == PARALLEL_LEVEL_1) ? 1 : ((scs->max_input_luma_width + 32) / 64);

    scs->cdef_segment_row_count    = (lp == PARALLEL_LEVEL_1)          ? 1
           : (((scs->max_input_luma_height + 32) / BLOCK_SIZE_64) < 6) ? 1
           : (scs->input_resolution <= INPUT_SIZE_1080p_RANGE)         ? 2
                                                                       : 4;
    scs->cdef_segment_column_count = (lp == PARALLEL_LEVEL_1)       ? 1
        : (((scs->max_input_luma_width + 32) / BLOCK_SIZE_64) < 10) ? 1
        : (scs->input_resolution <= INPUT_SIZE_1080p_RANGE)         ? 3
                                                                    : 6;

    //since restoration unit size is same for Luma and Chroma, Luma segments and chroma segments do not correspond to the same area!
    //to keep proper processing, segments have to be configured based on chroma resolution.
    const uint32_t unit_size       = RESTORATION_UNITSIZE_MAX;
    const uint32_t rest_seg_w      = MAX((scs->max_input_luma_width / 2 + (unit_size >> 1)) / unit_size, 1);
    const uint32_t rest_seg_h      = MAX((scs->max_input_luma_height / 2 + (unit_size >> 1)) / unit_size, 1);
    scs->rest_segment_column_count = (lp == PARALLEL_LEVEL_1) ? 1
        : scs->input_resolution <= INPUT_SIZE_1080p_RANGE     ? MIN(rest_seg_w, 6)
                                                              : MIN(rest_seg_w, 9);
    scs->rest_segment_row_count    = (lp == PARALLEL_LEVEL_1) ? 1
           : scs->input_resolution <= INPUT_SIZE_1080p_RANGE  ? MIN(rest_seg_h, 4)
                                                              : MIN(rest_seg_h, 6);
}

static EbErrorType load_default_buffer_configuration_settings(SequenceControlSet* scs) {
    EbErrorType return_error = EB_ErrorNone;
    uint32_t    core_count   = get_num_processors();

    uint32_t lp = scs->static_config.level_of_parallelism;
    if (lp == 0) {
        // In the default config (lp == 0) the core count will determine the
        // amount of parallelism used
        if (core_count <= PARALLEL_LEVEL_1_RANGE) {
            lp = PARALLEL_LEVEL_1;
        } else if (core_count <= PARALLEL_LEVEL_2_RANGE) {
            lp = PARALLEL_LEVEL_2;
        } else if (core_count <= PARALLEL_LEVEL_3_RANGE) {
            lp = PARALLEL_LEVEL_3;
        } else if (core_count <= PARALLEL_LEVEL_4_RANGE) {
            lp = PARALLEL_LEVEL_4;
        } else if (core_count <= PARALLEL_LEVEL_5_RANGE) {
            lp = PARALLEL_LEVEL_5;
        } else {
            lp = PARALLEL_LEVEL_6;
        }
    }
    scs->lp = lp;
    set_segments_numbers(scs);

    const bool is_low_delay = (scs->static_config.pred_structure == LOW_DELAY);
    // adjust buffer count for superres
    uint32_t superres_count = (scs->static_config.superres_mode == SUPERRES_AUTO &&
                               (scs->static_config.superres_auto_search_type == SUPERRES_AUTO_DUAL ||
                                scs->static_config.superres_auto_search_type == SUPERRES_AUTO_ALL))
        ? 1
        : 0;

    //#====================== Data Structures and Picture Buffers ======================

    uint32_t min_input, min_parent, min_child, min_paref, min_ref, min_tpl_ref, min_overlay, min_recon, min_me;
    uint32_t max_input, max_parent, max_child, max_paref, max_me, max_recon;

    /*Look-Ahead. Picture-Decision outputs pictures by group of mini-gops so
        the needed pictures for a certain look-ahead distance (LAD) should be rounded up to the next multiple of MiniGopSize.*/
    uint32_t      mg_size = 1 << scs->static_config.hierarchical_levels;
    const uint8_t overlay = scs->static_config.enable_overlays ? 1 : 0;

    /*To accomodate FFMPEG EOS, 1 frame delay is needed in Resource coordination for RA (for the low delay mode, buffering for receiving EOS does not happen).
      Note that we have the option to not add 1 frame delay of Resource Coordination. In this case we have wait for first I frame
      to be released back to be able to start first base(16). Anyway poc16 needs to wait for poc0 to finish.*/
    const uint8_t eos_delay = is_low_delay ? 0 : 1;

    //Minimum input pictures needed in the pipeline
    uint16_t lad_mg_pictures = (1 + mg_size + overlay) *
        scs->lad_mg; //Unit= 1(provision for a potential delayI) + prediction struct + potential overlay        return_ppcs = (1 + mg_size) * (scs->lad_mg + 1)  + scs->scd_delay + eos_delay;
    uint32_t return_ppcs = (1 + mg_size) * (scs->lad_mg + 1) + scs->scd_delay + eos_delay;
    //scs->input_buffer_fifo_init_count = return_ppcs;

    min_input = return_ppcs;

    min_parent = return_ppcs;

    // If overlay frames are used, each input will be assigned 2 ppcs: one for the regular frame, and one for the potential alt-ref frame
    if (scs->static_config.enable_overlays) {
        min_parent *= 2;
    }

    //Pic-Manager will inject one child at a time.
    min_child                          = 1;
    const uint16_t num_ref_from_cur_mg = get_num_refs_in_one_mg(scs->static_config.hierarchical_levels,
                                                                scs->mrp_ctrls.referencing_scheme) +
        1; //+1: to accomodate one for a delayed-I
    const uint16_t num_ref_lad_mgs = num_ref_from_cur_mg * scs->lad_mg;
    const uint8_t  dpb_frames =
        REF_FRAMES; // up to dpb_frame refs from prev MGs can be used (AV1 spec allows holding up to 8 frames for references)
    min_ref     = (scs->enable_dec_order) ? dpb_frames + 1 : num_ref_from_cur_mg + num_ref_lad_mgs + dpb_frames;
    min_tpl_ref = scs->tpl ? dpb_frames + 1 : 0; // TPL pictures are processed in decode order
    if (scs->tpl) {
        // PictureDecisionContext.mg_size = mg_size + overlay; see EbPictureDecisionProcess.c line 5680
        min_me = 1 + // potential delay I
            lad_mg_pictures + // 16 + 1 ME data used in store_tpl_pictures() at line 5717
            (mg_size + overlay); // 16 + 1 ME data used in store_tpl_pictures() at line 5729
    } else {
        min_me = 1;
    }

    //PA REF
    const uint16_t num_pa_ref_from_cur_mg =
        mg_size; //ref+nref; nRef PA buffers are processed in PicAnalysis and used in TF
    min_paref = num_pa_ref_from_cur_mg + lad_mg_pictures + scs->scd_delay + eos_delay + dpb_frames;

    if (scs->static_config.enable_overlays) {
        // Need an extra PA ref buffer for each overlay picture. Overlay pics use the same DPB as
        // regular pics, so no need to allocate an extra dpb_frames buffers for the ref pics
        min_paref += num_pa_ref_from_cur_mg + lad_mg_pictures + scs->scd_delay + eos_delay;
    }
    //Overlays
    // Each input pic will assign a ppcs and for each potential overlay, will assign a buffer to store the unfiltered input picture
    min_overlay = scs->static_config.enable_overlays ? return_ppcs : 0;
    min_recon   = min_ref;

    if (is_low_delay) {
        uint32_t low_delay_tf_frames = scs->tf_params_per_type[1].max_num_past_pics;
        min_input = min_parent = 1 + low_delay_tf_frames + scs->scd_delay + eos_delay;
        min_child              = 1; // max_child is 1 for LD
        uint8_t max_refs       = dpb_frames;
        // For special, known, RPS structures and ref frame counts, we can reduce the number of ref buffers
        if (scs->use_flat_ipp) {
            max_refs = scs->mrp_ctrls.flat_max_refs;
            // For flat IPP the previous frame is always used as a reference. Therefore, that picture does
            // not require a special buffer for use as a TF ref.
            if (low_delay_tf_frames) {
                low_delay_tf_frames -= 1;
            }
        } else if (scs->mrp_ctrls.ld_reduce_ref_buffs == 1) {
            max_refs = 4;
        } else if (scs->mrp_ctrls.ld_reduce_ref_buffs == 2) {
            max_refs = 2;
        }

        min_ref   = 1 /*current pic*/ + max_refs;
        min_me    = 1;
        min_paref = 1 /*current pic*/ + low_delay_tf_frames + max_refs + scs->scd_delay + eos_delay;
    }
    //Configure max needed buffers to process 1+n_extra_mg Mini-Gops in the pipeline. n extra MGs to feed to picMgr on top of current one.
    // Low delay mode has no extra minigops to process.
    uint32_t n_extra_mg;
    if (lp <= PARALLEL_LEVEL_3 || is_low_delay) {
        n_extra_mg = 0;
    } else if (lp <= PARALLEL_LEVEL_4) {
        n_extra_mg = 1;
    } else if (lp <= PARALLEL_LEVEL_5) {
        n_extra_mg = 2;
    } else {
        n_extra_mg = scs->input_resolution <= INPUT_SIZE_4K_RANGE ? 7
            : scs->input_resolution <= INPUT_SIZE_8K_RANGE        ? 5
                                                                  : 0;
    }

    max_input  = min_input + (1 + mg_size) * n_extra_mg;
    max_parent = max_input;
    max_child  = (mg_size / 2) * (n_extra_mg + 1);
    max_child  = MAX(max_child, 1); //have at least one child for mg_size<2
    // In low delay mode, will only have one picture at a time to process
    if (is_low_delay) {
        max_child = 1;
    }

    // max_ref defines here to avoid cppcheck warning
    uint32_t max_ref = min_ref + num_ref_from_cur_mg * n_extra_mg;
    max_paref        = min_paref + (1 + mg_size) * n_extra_mg;
    max_me           = min_me + (1 + mg_size) * n_extra_mg;
    max_recon        = max_ref;
    // if tpl_la is disabled when super-res fix/random, input speed is much faster than recon output speed,
    // recon_output_fifo might be full and freeze at svt_aom_recon_output()
    if (!scs->tpl && scs->static_config.recon_enabled) {
        max_recon = min_recon = MAX(max_ref, 30);
    }

    //#====================== Process Buffers ======================
    scs->input_buffer_fifo_init_count            = clamp(max_input, min_input, max_input);
    scs->picture_control_set_pool_init_count     = clamp(max_parent, min_parent, max_parent);
    scs->pa_reference_picture_buffer_init_count  = clamp(max_paref, min_paref, max_paref);
    scs->tpl_reference_picture_buffer_init_count = min_tpl_ref;
    scs->output_recon_buffer_fifo_init_count = scs->reference_picture_buffer_init_count = clamp(
        max_recon, min_recon, max_recon);
    scs->me_pool_init_count                      = clamp(max_me, min_me, max_me);
    scs->overlay_input_picture_buffer_init_count = min_overlay;

    if (lp <= PARALLEL_LEVEL_1 || MIN_PIC_PARALLELIZATION) {
        scs->input_buffer_fifo_init_count              = min_input;
        scs->picture_control_set_pool_init_count       = min_parent;
        scs->pa_reference_picture_buffer_init_count    = min_paref;
        scs->tpl_reference_picture_buffer_init_count   = min_tpl_ref;
        scs->reference_picture_buffer_init_count       = min_ref;
        scs->picture_control_set_pool_init_count_child = min_child;
        scs->enc_dec_pool_init_count                   = min_child;
        scs->me_pool_init_count                        = min_me;
        scs->overlay_input_picture_buffer_init_count   = min_overlay;

        scs->output_recon_buffer_fifo_init_count = MAX(scs->reference_picture_buffer_init_count, min_recon);
    } else if (lp <= PARALLEL_LEVEL_2) {
        scs->picture_control_set_pool_init_count_child = scs->enc_dec_pool_init_count = clamp(2, min_child, max_child) +
            superres_count;
    } else if (lp <= PARALLEL_LEVEL_3) {
        scs->picture_control_set_pool_init_count_child = scs->enc_dec_pool_init_count = clamp(8, min_child, max_child) +
            superres_count;
    } else if (lp <= PARALLEL_LEVEL_4) {
        scs->picture_control_set_pool_init_count_child = scs->enc_dec_pool_init_count = clamp(
                                                                                            12, min_child, max_child) +
            superres_count;
    } else if (lp <= PARALLEL_LEVEL_5) {
        scs->picture_control_set_pool_init_count_child = scs->enc_dec_pool_init_count = clamp(
                                                                                            16, min_child, max_child) +
            superres_count;
    } else {
        const uint8_t pcs_processes                    = scs->static_config.rate_control_mode == SVT_AV1_RC_MODE_VBR &&
                scs->static_config.pass == ENC_SECOND_PASS
                               ? 24
                               : 20;
        scs->picture_control_set_pool_init_count_child = scs->enc_dec_pool_init_count =
            clamp(pcs_processes, min_child, max_child) + superres_count;
    }

    if (scs->static_config.avif) {
        scs->input_buffer_fifo_init_count              = 4;
        scs->picture_control_set_pool_init_count       = 4;
        scs->pa_reference_picture_buffer_init_count    = 4;
        scs->tpl_reference_picture_buffer_init_count   = 0;
        scs->output_recon_buffer_fifo_init_count       = 1;
        scs->reference_picture_buffer_init_count       = 2;
        scs->picture_control_set_pool_init_count_child = 1;
        scs->enc_dec_pool_init_count                   = 1;
        scs->me_pool_init_count                        = 1;
        scs->overlay_input_picture_buffer_init_count   = 0;
        scs->allintra                                  = true;
        scs->static_config.intra_period_length         = 0;
    }

    //#====================== Inter process Fifos ======================
    const uint32_t tot_tf_segs  = scs->tf_segment_column_count * scs->tf_segment_row_count;
    const uint32_t tot_me_segs  = scs->me_segment_col_count_array * scs->me_segment_row_count_array;
    const uint32_t tot_tpl_segs = scs->tpl_segment_col_count_array * scs->tpl_segment_row_count_array;
    // For VBR/Capped-CRF and superres, we may use a recode loop, in which case, the enc dec fifo queue should be increased
    // to account for the extra potential calls.
    const bool     allow_recode     = scs->static_config.recode_loop != DISALLOW_RECODE;
    const bool     is_superres      = scs->static_config.superres_mode != SUPERRES_NONE;
    const uint32_t tot_enc_dec_segs = scs->enc_dec_segment_col_count_array * scs->enc_dec_segment_row_count_array;
    const uint32_t tot_cdef_segs    = scs->cdef_segment_column_count * scs->cdef_segment_row_count;
    const uint32_t tot_rest_segs    = scs->rest_segment_column_count * scs->rest_segment_row_count;
    const uint32_t tot_tiles        = MIN(9,
                                   (1 << scs->static_config.tile_columns) *
                                       (1 << scs->static_config.tile_rows)); //Jing: Too many tiles may drain the fifo
    const uint32_t max_fifo         = 300;

    // Open loop
    scs->resource_coordination_fifo_init_count = MIN(
        max_fifo, scs->picture_control_set_pool_init_count); // outputs one pic @ a time to pic analysis (no segments)
    scs->picture_analysis_fifo_init_count = MIN(
        max_fifo, scs->picture_control_set_pool_init_count); // output from pic analysis to PD process (single threaded)
    scs->enc_ctx->picture_decision_reorder_queue_size = scs->picture_control_set_pool_init_count;
    scs->picture_decision_fifo_init_count             = MIN(
        max_fifo, scs->picture_control_set_pool_init_count * MAX(tot_me_segs, tot_tf_segs));
    scs->motion_estimation_fifo_init_count = MIN(
        max_fifo, scs->picture_control_set_pool_init_count); // output from ME to init_rc_process (single threaded)
    scs->initial_rate_control_fifo_init_count = MIN(
        max_fifo, scs->picture_control_set_pool_init_count); // output from irc to src_ops (single threaded)
    scs->tpl_disp_fifo_init_count = MIN(max_fifo,
                                        scs->picture_control_set_pool_init_count * tot_tpl_segs); // TPL dispenser

    // Pic manager (pass through from open loop to closed loop)
    scs->picture_demux_fifo_init_count = MIN(
        max_fifo,
        scs->picture_control_set_pool_init_count +
            2 * scs->picture_control_set_pool_init_count_child); // input to pic mgr from src ops, rest, and packetization
    scs->enc_ctx->pic_mgr_input_pic_list_size = scs->picture_control_set_pool_init_count;

    // Closed loop
    scs->rate_control_tasks_fifo_init_count = MIN(
        max_fifo,
        2 * scs->picture_control_set_pool_init_count_child); // inputs to rc form pic manager and EC/packetization
    scs->rate_control_fifo_init_count                = MIN(max_fifo,
                                            scs->picture_control_set_pool_init_count_child); // inputs to MDC from rc
    scs->mode_decision_configuration_fifo_init_count = MIN(
        max_fifo,
        scs->picture_control_set_pool_init_count_child * tot_tiles * tot_enc_dec_segs *
            (1 + allow_recode + is_superres)); // input to MD from MDC
    scs->enc_dec_fifo_init_count = MIN(max_fifo,
                                       scs->picture_control_set_pool_init_count_child); // TODO: Add DLF segments
    scs->dlf_fifo_init_count     = MIN(
        max_fifo, scs->picture_control_set_pool_init_count_child * tot_cdef_segs); // input to CDEF from DLF
    scs->cdef_fifo_init_count = MIN(
        max_fifo, scs->picture_control_set_pool_init_count_child * tot_rest_segs); // input to rest from CDEF
    scs->rest_fifo_init_count = MIN(
        max_fifo, scs->picture_control_set_pool_init_count_child * tot_tiles); // input to EC from rest
    scs->entropy_coding_fifo_init_count = MIN(
        max_fifo, scs->picture_control_set_pool_init_count_child); // EC outputs to packetization (single threaded)
    scs->enc_ctx->packetization_reorder_queue_size = scs->picture_control_set_pool_init_count;
    // bistream buffer will be allocated at run time. app will free the buffer once written to file.
    scs->output_stream_buffer_fifo_init_count = scs->picture_control_set_pool_init_count +
        2; // +2 b/c used to signal EOS @ resource coord and packetization
    //#====================== Processes number ======================
    scs->total_process_init_count = 0;

    uint32_t max_pa_proc, max_me_proc, max_tpl_proc, max_mdc_proc, max_md_proc, max_ec_proc, max_dlf_proc,
        max_cdef_proc, max_rest_proc;

    max_pa_proc  = max_input;
    max_me_proc  = max_me * tot_me_segs;
    max_tpl_proc = get_max_wavefronts(scs->max_input_luma_width, scs->max_input_luma_height, 64);
    max_mdc_proc = scs->picture_control_set_pool_init_count_child;
    max_md_proc  = scs->picture_control_set_pool_init_count_child *
        get_max_wavefronts(scs->max_input_luma_width, scs->max_input_luma_height, scs->super_block_size);
    max_ec_proc   = scs->picture_control_set_pool_init_count_child;
    max_dlf_proc  = scs->picture_control_set_pool_init_count_child;
    max_cdef_proc = scs->picture_control_set_pool_init_count_child * scs->cdef_segment_column_count *
        scs->cdef_segment_row_count;
    max_rest_proc = scs->picture_control_set_pool_init_count_child * scs->rest_segment_column_count *
        scs->rest_segment_row_count;

    if (lp <= PARALLEL_LEVEL_1) {
        scs->total_process_init_count += (scs->picture_analysis_process_init_count = 1);
        scs->total_process_init_count += (scs->motion_estimation_process_init_count = 1);
        scs->total_process_init_count += (scs->source_based_operations_process_init_count = 1);
        scs->total_process_init_count += (scs->tpl_disp_process_init_count = 1);
        scs->total_process_init_count += (scs->mode_decision_configuration_process_init_count = 1);
        scs->total_process_init_count += (scs->enc_dec_process_init_count = 1);
        scs->total_process_init_count += (scs->entropy_coding_process_init_count = 1);
        scs->total_process_init_count += (scs->dlf_process_init_count = 1);
        scs->total_process_init_count += (scs->cdef_process_init_count = 1);
        scs->total_process_init_count += (scs->rest_process_init_count = 1);
    } else if (lp <= PARALLEL_LEVEL_2) {
        const uint8_t pa_processes = scs->static_config.film_grain_denoise_strength ? 16 : 1;
        scs->total_process_init_count += (scs->source_based_operations_process_init_count = 1);
        scs->total_process_init_count += (scs->picture_analysis_process_init_count = clamp(
                                              pa_processes, 1, max_pa_proc));
        scs->total_process_init_count += (scs->motion_estimation_process_init_count = clamp(20, 1, max_me_proc));
        scs->total_process_init_count += (scs->tpl_disp_process_init_count = clamp(6, 1, max_tpl_proc));
        scs->total_process_init_count += (scs->mode_decision_configuration_process_init_count = clamp(
                                              1, 1, max_mdc_proc));
        scs->total_process_init_count += (scs->enc_dec_process_init_count = clamp(
                                              3, scs->picture_control_set_pool_init_count_child, max_md_proc));
        scs->total_process_init_count += (scs->entropy_coding_process_init_count = clamp(1, 1, max_ec_proc));
        scs->total_process_init_count += (scs->dlf_process_init_count = clamp(1, 1, max_dlf_proc));
        scs->total_process_init_count += (scs->cdef_process_init_count = clamp(6, 1, max_cdef_proc));
        scs->total_process_init_count += (scs->rest_process_init_count = clamp(1, 1, max_rest_proc));
    } else if (lp <= PARALLEL_LEVEL_3) {
        const uint8_t pa_processes = scs->static_config.film_grain_denoise_strength ? 16 : 1;
        scs->total_process_init_count += (scs->source_based_operations_process_init_count = 1);
        scs->total_process_init_count += (scs->picture_analysis_process_init_count = clamp(
                                              pa_processes, 1, max_pa_proc));
        scs->total_process_init_count += (scs->motion_estimation_process_init_count = clamp(25, 1, max_me_proc));
        scs->total_process_init_count += (scs->tpl_disp_process_init_count = clamp(6, 1, max_tpl_proc));
        scs->total_process_init_count += (scs->mode_decision_configuration_process_init_count = clamp(
                                              2, 1, max_mdc_proc));
        scs->total_process_init_count += (scs->enc_dec_process_init_count = clamp(
                                              5, scs->picture_control_set_pool_init_count_child, max_md_proc));
        scs->total_process_init_count += (scs->entropy_coding_process_init_count = clamp(2, 1, max_ec_proc));
        scs->total_process_init_count += (scs->dlf_process_init_count = clamp(2, 1, max_dlf_proc));
        scs->total_process_init_count += (scs->cdef_process_init_count = clamp(6, 1, max_cdef_proc));
        scs->total_process_init_count += (scs->rest_process_init_count = clamp(2, 1, max_rest_proc));
    } else if (lp <= PARALLEL_LEVEL_5 || scs->input_resolution <= INPUT_SIZE_1080p_RANGE) {
        uint8_t pa_processes = scs->static_config.film_grain_denoise_strength ? 20 : 4;
        if (scs->static_config.pass == ENC_FIRST_PASS) {
            pa_processes = lp <= PARALLEL_LEVEL_5 ? 12 : 20;
        }
        scs->total_process_init_count += (scs->source_based_operations_process_init_count = 1);
        scs->total_process_init_count += (scs->picture_analysis_process_init_count = clamp(
                                              pa_processes, 1, max_pa_proc));
        scs->total_process_init_count += (scs->motion_estimation_process_init_count = clamp(25, 1, max_me_proc));
        scs->total_process_init_count += (scs->tpl_disp_process_init_count = clamp(6, 1, max_tpl_proc));
        scs->total_process_init_count += (scs->mode_decision_configuration_process_init_count = clamp(
                                              3, 1, max_mdc_proc));
        scs->total_process_init_count += (scs->enc_dec_process_init_count = clamp(
                                              6, scs->picture_control_set_pool_init_count_child, max_md_proc));
        scs->total_process_init_count += (scs->entropy_coding_process_init_count = clamp(4, 1, max_ec_proc));
        scs->total_process_init_count += (scs->dlf_process_init_count = clamp(3, 1, max_dlf_proc));
        scs->total_process_init_count += (scs->cdef_process_init_count = clamp(6, 1, max_cdef_proc));
        scs->total_process_init_count += (scs->rest_process_init_count = clamp(4, 1, max_rest_proc));
    } else {
        const uint8_t pa_processes = (scs->static_config.pass == ENC_FIRST_PASS ||
                                      scs->static_config.film_grain_denoise_strength)
            ? 20
            : 16;
        scs->total_process_init_count += (scs->source_based_operations_process_init_count = 1);
        scs->total_process_init_count += (scs->picture_analysis_process_init_count = clamp(
                                              pa_processes, 1, max_pa_proc));
        scs->total_process_init_count += (scs->motion_estimation_process_init_count = clamp(25, 1, max_me_proc));
        scs->total_process_init_count += (scs->tpl_disp_process_init_count = clamp(12, 1, max_tpl_proc));
        scs->total_process_init_count += (scs->mode_decision_configuration_process_init_count = clamp(
                                              8, 1, max_mdc_proc));
        scs->total_process_init_count += (scs->enc_dec_process_init_count = clamp(
                                              8, scs->picture_control_set_pool_init_count_child, max_md_proc));
        scs->total_process_init_count += (scs->entropy_coding_process_init_count = clamp(10, 1, max_ec_proc));
        scs->total_process_init_count += (scs->dlf_process_init_count = clamp(8, 1, max_dlf_proc));
        scs->total_process_init_count += (scs->cdef_process_init_count = clamp(8, 1, max_cdef_proc));
        scs->total_process_init_count += (scs->rest_process_init_count = clamp(10, 1, max_rest_proc));
    }

    scs->total_process_init_count += 6; // single processes count
    if (scs->static_config.pass == 0 || scs->static_config.pass == 2) {
        SVT_INFO("Level of Parallelism: %u\n", lp);
        SVT_INFO("Number of PPCS %u\n", scs->picture_control_set_pool_init_count);

        /******************************************************************
        * Platform detection, limit cpu flags to hardware available CPU
        ******************************************************************/
#if defined ARCH_X86_64 || defined ARCH_AARCH64
        const EbCpuFlags cpu_flags        = svt_aom_get_cpu_flags();
        const EbCpuFlags cpu_flags_to_use = svt_aom_get_cpu_flags_to_use();
        scs->static_config.use_cpu_flags &= cpu_flags_to_use;
        SVT_INFO("[asm level on system : up to %s]\n", get_asm_level_name_str(cpu_flags));
        SVT_INFO("[asm level selected : up to %s]\n", get_asm_level_name_str(scs->static_config.use_cpu_flags));
#else
        scs->static_config.use_cpu_flags &= 0;
        SVT_INFO("[asm level on system : up to %s]\n", get_asm_level_name_str(0));
        SVT_INFO("[asm level selected : up to %s]\n", get_asm_level_name_str(scs->static_config.use_cpu_flags));
#endif
    }
    return return_error;
}

// clang-format off
static RateControlPorts rate_control_ports[] = {
    {RATE_CONTROL_INPUT_PORT_INLME,         0},
    {RATE_CONTROL_INPUT_PORT_PACKETIZATION, 0},
    {RATE_CONTROL_INPUT_PORT_INVALID,       0}
};

static PicMgrPorts pic_mgr_ports[] = {
    {PIC_MGR_INPUT_PORT_SOP,           0},
    {PIC_MGR_INPUT_PORT_PACKETIZATION, 0},
    {PIC_MGR_INPUT_PORT_REST,          0},
    {PIC_MGR_INPUT_PORT_INVALID,       0}
};

typedef struct {
    int32_t  type;
    uint32_t count;
} EncDecPorts_t;

static EncDecPorts_t enc_dec_ports[] = {
    {ENCDEC_INPUT_PORT_MDC,     0},
    {ENCDEC_INPUT_PORT_ENCDEC,  0},
    {ENCDEC_INPUT_PORT_INVALID, 0}
};
static EncDecPorts_t tpl_ports[] = {
    {TPL_INPUT_PORT_SOP,     0},
    {TPL_INPUT_PORT_TPL,     0},
    {TPL_INPUT_PORT_INVALID, 0}
};
// clang-format on

// Rate Control
static uint32_t rate_control_port_lookup(RateControlInputPortTypes type, uint32_t port_type_index) {
    uint32_t port_index = 0;
    uint32_t port_count = 0;

    while ((type != rate_control_ports[port_index].type) && (type != RATE_CONTROL_INPUT_PORT_INVALID)) {
        port_count += rate_control_ports[port_index++].count;
    }
    return (port_count + port_type_index);
}

// Rate Control
static uint32_t rate_control_port_total_count(void) {
    uint32_t port_index  = 0;
    uint32_t total_count = 0;

    while (rate_control_ports[port_index].type != RATE_CONTROL_INPUT_PORT_INVALID) {
        total_count += rate_control_ports[port_index++].count;
    }
    return total_count;
}

static uint32_t pic_mgr_port_lookup(PicMgrInputPortTypes type, uint32_t port_type_index) {
    uint32_t port_index = 0;
    uint32_t port_count = 0;

    while ((type != pic_mgr_ports[port_index].type) && (type != PIC_MGR_INPUT_PORT_INVALID)) {
        port_count += pic_mgr_ports[port_index++].count;
    }
    return (port_count + port_type_index);
}

static uint32_t pic_mgr_port_total_count(void) {
    uint32_t port_index  = 0;
    uint32_t total_count = 0;

    while (pic_mgr_ports[port_index].type != PIC_MGR_INPUT_PORT_INVALID) {
        total_count += pic_mgr_ports[port_index++].count;
    }
    return total_count;
}

// TPL
static uint32_t tpl_port_lookup(int32_t type, uint32_t port_type_index) {
    uint32_t port_index = 0;
    uint32_t port_count = 0;

    while ((type != tpl_ports[port_index].type) && (type != TPL_INPUT_PORT_INVALID)) {
        port_count += tpl_ports[port_index++].count;
    }
    return (port_count + port_type_index);
}

static uint32_t tpl_port_total_count(void) {
    uint32_t port_index  = 0;
    uint32_t total_count = 0;

    while (tpl_ports[port_index].type != TPL_INPUT_PORT_INVALID) {
        total_count += tpl_ports[port_index++].count;
    }
    return total_count;
}

/*****************************************
 * Input Port Lookup
 *****************************************/
// EncDec
static uint32_t enc_dec_port_lookup(int32_t type, uint32_t port_type_index) {
    uint32_t port_index = 0;
    uint32_t port_count = 0;

    while ((type != enc_dec_ports[port_index].type) && (type != ENCDEC_INPUT_PORT_INVALID)) {
        port_count += enc_dec_ports[port_index++].count;
    }
    return (port_count + port_type_index);
}

// EncDec
static uint32_t enc_dec_port_total_count(void) {
    uint32_t port_index  = 0;
    uint32_t total_count = 0;

    while (enc_dec_ports[port_index].type != ENCDEC_INPUT_PORT_INVALID) {
        total_count += enc_dec_ports[port_index++].count;
    }
    return total_count;
}

/*****************************************
 * Input Port Total Count
 *****************************************/

static void lib_svt_encoder_send_error_exit(EbPtr hComponent, uint32_t error_code);

static void svt_enc_handle_stop_threads(EbEncHandle* enc_handle_ptr) {
    SequenceControlSet* scs = enc_handle_ptr->scs_instance->scs;
    // Resource Coordination
    EB_DESTROY_THREAD(enc_handle_ptr->resource_coordination_thread_handle);
    EB_DESTROY_THREAD_ARRAY(enc_handle_ptr->picture_analysis_thread_handle_array,
                            scs->picture_analysis_process_init_count);

    // Picture Decision
    EB_DESTROY_THREAD(enc_handle_ptr->picture_decision_thread_handle);

    // Motion Estimation
    EB_DESTROY_THREAD_ARRAY(enc_handle_ptr->motion_estimation_thread_handle_array,
                            scs->motion_estimation_process_init_count);

    // Initial Rate Control
    EB_DESTROY_THREAD(enc_handle_ptr->initial_rate_control_thread_handle);

    // Source Based Oprations
    EB_DESTROY_THREAD_ARRAY(enc_handle_ptr->source_based_operations_thread_handle_array,
                            scs->source_based_operations_process_init_count);

    // TPL dispenser ME
    EB_DESTROY_THREAD_ARRAY(enc_handle_ptr->tpl_disp_thread_handle_array, scs->tpl_disp_process_init_count);

    // Picture Manager
    EB_DESTROY_THREAD(enc_handle_ptr->picture_manager_thread_handle);

    // Rate Control
    EB_DESTROY_THREAD(enc_handle_ptr->rate_control_thread_handle);

    // Mode Decision Configuration Process
    EB_DESTROY_THREAD_ARRAY(enc_handle_ptr->mode_decision_configuration_thread_handle_array,
                            scs->mode_decision_configuration_process_init_count);

    // EncDec Process
    EB_DESTROY_THREAD_ARRAY(enc_handle_ptr->enc_dec_thread_handle_array, scs->enc_dec_process_init_count);

    // Dlf Process
    EB_DESTROY_THREAD_ARRAY(enc_handle_ptr->dlf_thread_handle_array, scs->dlf_process_init_count);

    // Cdef Process
    EB_DESTROY_THREAD_ARRAY(enc_handle_ptr->cdef_thread_handle_array, scs->cdef_process_init_count);

    // Rest Process
    EB_DESTROY_THREAD_ARRAY(enc_handle_ptr->rest_thread_handle_array, scs->rest_process_init_count);

    // Entropy Coding Process
    EB_DESTROY_THREAD_ARRAY(enc_handle_ptr->entropy_coding_thread_handle_array, scs->entropy_coding_process_init_count);

    // Packetization
    EB_DESTROY_THREAD(enc_handle_ptr->packetization_thread_handle);
}

/**********************************
* Encoder Library Handle Deonstructor
**********************************/
static void svt_enc_handle_dctor(EbPtr p) {
    EbEncHandle* enc_handle_ptr = (EbEncHandle*)p;
    svt_enc_handle_stop_threads(enc_handle_ptr);
    EB_FREE(enc_handle_ptr->app_callback_ptr);
    EB_DELETE(enc_handle_ptr->scs_pool_ptr);
    EB_DELETE(enc_handle_ptr->picture_parent_control_set_pool_ptr);
    EB_DELETE(enc_handle_ptr->me_pool_ptr);
    EB_DELETE(enc_handle_ptr->picture_control_set_pool_ptr);
    EB_DELETE(enc_handle_ptr->enc_dec_pool_ptr);
    EB_DELETE(enc_handle_ptr->pa_reference_picture_pool_ptr);
    EB_DELETE(enc_handle_ptr->tpl_reference_picture_pool_ptr);
    EB_DELETE(enc_handle_ptr->overlay_input_picture_pool_ptr);
    EB_DELETE(enc_handle_ptr->reference_picture_pool_ptr);
    EB_DELETE(enc_handle_ptr->input_cmd_resource_ptr);
    EB_DELETE(enc_handle_ptr->input_y8b_buffer_resource_ptr);

    //all buffer_y have been redirected to y8b location that just got released.
    //to prevent releasing twice, we need to reset the buffer back to NULL
    if (enc_handle_ptr->input_buffer_resource_ptr) {
        for (uint32_t w_i = 0; w_i < enc_handle_ptr->input_buffer_resource_ptr->object_total_count; ++w_i) {
            EbObjectWrapper*     wrp  = enc_handle_ptr->input_buffer_resource_ptr->wrapper_ptr_pool[w_i];
            EbBufferHeaderType*  obj  = (EbBufferHeaderType*)wrp->object_ptr;
            EbPictureBufferDesc* desc = (EbPictureBufferDesc*)obj->p_buffer;
            desc->buffer_y            = 0;
        }
    }
    EB_DELETE(enc_handle_ptr->input_buffer_resource_ptr);
    EB_DELETE(enc_handle_ptr->output_stream_buffer_resource_ptr);
    EB_DELETE(enc_handle_ptr->output_recon_buffer_resource_ptr);
    EB_DELETE(enc_handle_ptr->resource_coordination_results_resource_ptr);
    EB_DELETE(enc_handle_ptr->picture_analysis_results_resource_ptr);
    EB_DELETE(enc_handle_ptr->picture_decision_results_resource_ptr);
    EB_DELETE(enc_handle_ptr->motion_estimation_results_resource_ptr);
    EB_DELETE(enc_handle_ptr->initial_rate_control_results_resource_ptr);
    EB_DELETE(enc_handle_ptr->picture_demux_results_resource_ptr);
    EB_DELETE(enc_handle_ptr->tpl_disp_res_srm);
    EB_DELETE(enc_handle_ptr->rate_control_tasks_resource_ptr);
    EB_DELETE(enc_handle_ptr->rate_control_results_resource_ptr);
    EB_DELETE(enc_handle_ptr->enc_dec_tasks_resource_ptr);
    EB_DELETE(enc_handle_ptr->enc_dec_results_resource_ptr);
    EB_DELETE(enc_handle_ptr->dlf_results_resource_ptr);
    EB_DELETE(enc_handle_ptr->cdef_results_resource_ptr);
    EB_DELETE(enc_handle_ptr->rest_results_resource_ptr);
    EB_DELETE(enc_handle_ptr->entropy_coding_results_resource_ptr);

    EB_DELETE(enc_handle_ptr->resource_coordination_context_ptr);
    SequenceControlSet* scs = enc_handle_ptr->scs_instance->scs;
    EB_DELETE_PTR_ARRAY(enc_handle_ptr->picture_analysis_context_ptr_array, scs->picture_analysis_process_init_count);
    EB_DELETE_PTR_ARRAY(enc_handle_ptr->motion_estimation_context_ptr_array, scs->motion_estimation_process_init_count);
    EB_DELETE_PTR_ARRAY(enc_handle_ptr->tpl_disp_context_ptr_array, scs->tpl_disp_process_init_count);
    EB_DELETE_PTR_ARRAY(enc_handle_ptr->source_based_operations_context_ptr_array,
                        scs->source_based_operations_process_init_count);
    EB_DELETE_PTR_ARRAY(enc_handle_ptr->mode_decision_configuration_context_ptr_array,
                        scs->mode_decision_configuration_process_init_count);
    EB_DELETE_PTR_ARRAY(enc_handle_ptr->enc_dec_context_ptr_array, scs->enc_dec_process_init_count);
    EB_DELETE_PTR_ARRAY(enc_handle_ptr->dlf_context_ptr_array, scs->dlf_process_init_count);
    EB_DELETE_PTR_ARRAY(enc_handle_ptr->cdef_context_ptr_array, scs->cdef_process_init_count);
    EB_DELETE_PTR_ARRAY(enc_handle_ptr->rest_context_ptr_array, scs->rest_process_init_count);
    EB_DELETE_PTR_ARRAY(enc_handle_ptr->entropy_coding_context_ptr_array, scs->entropy_coding_process_init_count);
    EB_DELETE(enc_handle_ptr->scs_instance);
    EB_DELETE(enc_handle_ptr->picture_decision_context_ptr);
    EB_DELETE(enc_handle_ptr->initial_rate_control_context_ptr);
    EB_DELETE(enc_handle_ptr->picture_manager_context_ptr);
    EB_DELETE(enc_handle_ptr->rate_control_context_ptr);
    EB_DELETE(enc_handle_ptr->packetization_context_ptr);
}

/**********************************
* Encoder Library Handle Constructor
**********************************/
static EbErrorType svt_enc_handle_ctor(EbEncHandle* enc_handle_ptr, EbComponentType* ebHandlePtr) {
    enc_handle_ptr->dctor = svt_enc_handle_dctor;

    // Initialize Callbacks
    EB_MALLOC_OBJECT(enc_handle_ptr->app_callback_ptr);
    enc_handle_ptr->app_callback_ptr->error_handler = lib_svt_encoder_send_error_exit;
    enc_handle_ptr->app_callback_ptr->handle        = ebHandlePtr;

    // Config Set Count
    enc_handle_ptr->scs_pool_total_count = EB_SequenceControlSetPoolInitCount;
    // Initialize Sequence Control Set Instance
    EB_NEW(enc_handle_ptr->scs_instance, svt_sequence_control_set_instance_ctor);

    enc_handle_ptr->eos_received   = false;
    enc_handle_ptr->eos_sent       = false;
    enc_handle_ptr->frame_received = false;
    enc_handle_ptr->is_prev_valid  = true;
    return EB_ErrorNone;
}

EbErrorType svt_input_buffer_header_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr);

EbErrorType svt_output_recon_buffer_header_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr);

EbErrorType svt_overlay_buffer_header_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr);

EbErrorType svt_output_buffer_header_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr);

void svt_input_buffer_header_destroyer(EbPtr p);
void svt_output_recon_buffer_header_destroyer(EbPtr p);
void svt_output_buffer_header_destroyer(EbPtr p);

EbErrorType svt_input_y8b_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr);
void        svt_input_y8b_destroyer(EbPtr p);

static EbErrorType in_cmd_ctor(InputCommand* context_ptr, EbPtr object_init_data_ptr) {
    (void)context_ptr;
    (void)object_init_data_ptr;

    return EB_ErrorNone;
}

/*
* Input Command Constructor
*/
EbErrorType svt_input_cmd_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr) {
    InputCommand* obj;

    *object_dbl_ptr = NULL;
    EB_NEW(obj, in_cmd_ctor, object_init_data_ptr);
    *object_dbl_ptr = obj;

    return EB_ErrorNone;
}

static EbErrorType dlf_results_ctor(DlfResults* context_ptr, EbPtr object_init_data_ptr) {
    (void)context_ptr;
    (void)object_init_data_ptr;

    return EB_ErrorNone;
}

static EbErrorType dlf_results_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr) {
    DlfResults* obj;

    *object_dbl_ptr = NULL;
    EB_NEW(obj, dlf_results_ctor, object_init_data_ptr);
    *object_dbl_ptr = obj;

    return EB_ErrorNone;
}

/*
   TPL results ctor
*/
EbErrorType tpl_disp_results_ctor(TplDispResults* context_ptr, EbPtr object_init_data_ptr) {
    (void)context_ptr;
    (void)object_init_data_ptr;

    return EB_ErrorNone;
}

/*
   TPL results creator
*/
static EbErrorType tpl_disp_results_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr) {
    TplDispResults* obj;

    *object_dbl_ptr = NULL;
    EB_NEW(obj, tpl_disp_results_ctor, object_init_data_ptr);
    *object_dbl_ptr = obj;

    return EB_ErrorNone;
}

static EbErrorType cdef_results_ctor(CdefResults* context_ptr, EbPtr object_init_data_ptr) {
    (void)context_ptr;
    (void)object_init_data_ptr;

    return EB_ErrorNone;
}

static EbErrorType cdef_results_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr) {
    CdefResults* obj;

    *object_dbl_ptr = NULL;
    EB_NEW(obj, cdef_results_ctor, object_init_data_ptr);
    *object_dbl_ptr = obj;

    return EB_ErrorNone;
}

EbErrorType rest_results_ctor(RestResults* context_ptr, EbPtr object_init_data_ptr) {
    (void)context_ptr;
    (void)object_init_data_ptr;

    return EB_ErrorNone;
}

static EbErrorType rest_results_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr) {
    RestResults* obj;

    *object_dbl_ptr = NULL;
    EB_NEW(obj, rest_results_ctor, object_init_data_ptr);
    *object_dbl_ptr = obj;

    return EB_ErrorNone;
}

static int create_pa_ref_buf_descs(EbEncHandle* enc_handle_ptr) {
    SequenceControlSet*             scs = enc_handle_ptr->scs_instance->scs;
    EbPaReferenceObjectDescInitData eb_pa_ref_obj_ect_desc_init_data_structure;
    EbPictureBufferDescInitData     ref_pic_buf_desc_init_data;
    EbPictureBufferDescInitData     quart_pic_buf_desc_init_data;
    EbPictureBufferDescInitData     sixteenth_pic_buf_desc_init_data;
    const bool                      allintra = scs->allintra;
    // PA Reference Picture Buffers
    // Currently, only Luma samples are needed in the PA
    ref_pic_buf_desc_init_data.max_width    = scs->max_input_luma_width;
    ref_pic_buf_desc_init_data.max_height   = scs->max_input_luma_height;
    ref_pic_buf_desc_init_data.bit_depth    = EB_EIGHT_BIT;
    ref_pic_buf_desc_init_data.color_format = EB_YUV420; //use 420 for picture analysis
    //No full-resolution pixel data is allocated for PA REF,
    // it points directly to the Luma input samples of the app data
    ref_pic_buf_desc_init_data.buffer_enable_mask = 0;

    ref_pic_buf_desc_init_data.left_padding        = scs->left_padding;
    ref_pic_buf_desc_init_data.right_padding       = scs->right_padding;
    ref_pic_buf_desc_init_data.top_padding         = scs->top_padding;
    ref_pic_buf_desc_init_data.bot_padding         = scs->bot_padding;
    ref_pic_buf_desc_init_data.split_mode          = false;
    ref_pic_buf_desc_init_data.rest_units_per_tile = scs->rest_units_per_tile;
    ref_pic_buf_desc_init_data.mfmv                = 0;
    ref_pic_buf_desc_init_data.is_16bit_pipeline   = false;
    ref_pic_buf_desc_init_data.sb_total_count      = scs->sb_total_count;

    quart_pic_buf_desc_init_data.max_width           = scs->max_input_luma_width >> 1;
    quart_pic_buf_desc_init_data.max_height          = scs->max_input_luma_height >> 1;
    quart_pic_buf_desc_init_data.bit_depth           = EB_EIGHT_BIT;
    quart_pic_buf_desc_init_data.color_format        = EB_YUV420;
    quart_pic_buf_desc_init_data.buffer_enable_mask  = allintra ? 0 : PICTURE_BUFFER_DESC_LUMA_MASK;
    quart_pic_buf_desc_init_data.left_padding        = scs->b64_size >> 1;
    quart_pic_buf_desc_init_data.right_padding       = scs->b64_size >> 1;
    quart_pic_buf_desc_init_data.top_padding         = scs->b64_size >> 1;
    quart_pic_buf_desc_init_data.bot_padding         = scs->b64_size >> 1;
    quart_pic_buf_desc_init_data.split_mode          = false;
    quart_pic_buf_desc_init_data.rest_units_per_tile = scs->rest_units_per_tile;
    quart_pic_buf_desc_init_data.mfmv                = 0;
    quart_pic_buf_desc_init_data.is_16bit_pipeline   = false;
    quart_pic_buf_desc_init_data.sb_total_count      = scs->sb_total_count;

    sixteenth_pic_buf_desc_init_data.max_width           = scs->max_input_luma_width >> 2;
    sixteenth_pic_buf_desc_init_data.max_height          = scs->max_input_luma_height >> 2;
    sixteenth_pic_buf_desc_init_data.bit_depth           = EB_EIGHT_BIT;
    sixteenth_pic_buf_desc_init_data.color_format        = EB_YUV420;
    sixteenth_pic_buf_desc_init_data.buffer_enable_mask  = allintra ? 0 : PICTURE_BUFFER_DESC_LUMA_MASK;
    sixteenth_pic_buf_desc_init_data.left_padding        = scs->b64_size >> 2;
    sixteenth_pic_buf_desc_init_data.right_padding       = scs->b64_size >> 2;
    sixteenth_pic_buf_desc_init_data.top_padding         = scs->b64_size >> 2;
    sixteenth_pic_buf_desc_init_data.bot_padding         = scs->b64_size >> 2;
    sixteenth_pic_buf_desc_init_data.split_mode          = false;
    sixteenth_pic_buf_desc_init_data.rest_units_per_tile = scs->rest_units_per_tile;
    sixteenth_pic_buf_desc_init_data.mfmv                = 0;
    sixteenth_pic_buf_desc_init_data.is_16bit_pipeline   = false;
    sixteenth_pic_buf_desc_init_data.sb_total_count      = scs->sb_total_count;

    eb_pa_ref_obj_ect_desc_init_data_structure.reference_picture_desc_init_data = ref_pic_buf_desc_init_data;
    eb_pa_ref_obj_ect_desc_init_data_structure.quarter_picture_desc_init_data   = quart_pic_buf_desc_init_data;
    eb_pa_ref_obj_ect_desc_init_data_structure.sixteenth_picture_desc_init_data = sixteenth_pic_buf_desc_init_data;
    // Reference Picture Buffers
    EB_NEW(enc_handle_ptr->pa_reference_picture_pool_ptr,
           svt_system_resource_ctor,
           scs->pa_reference_picture_buffer_init_count,
           EB_PictureDecisionProcessInitCount,
           0,
           svt_pa_reference_object_creator,
           &(eb_pa_ref_obj_ect_desc_init_data_structure),
           NULL);
    // Set the SequenceControlSet Picture Pool Fifo Ptrs
    enc_handle_ptr->scs_instance->enc_ctx->pa_reference_picture_pool_fifo_ptr = svt_system_resource_get_producer_fifo(
        enc_handle_ptr->pa_reference_picture_pool_ptr, 0);
#if SRM_REPORT
    enc_handle_ptr->scs_instance_array->enc_ctx->pa_reference_picture_pool_fifo_ptr->queue_ptr->log = 0;
#endif
    return 0;
}

static int create_tpl_ref_buf_descs(EbEncHandle* enc_handle_ptr) {
    SequenceControlSet*              scs = enc_handle_ptr->scs_instance->scs;
    EbTplReferenceObjectDescInitData eb_tpl_ref_obj_ect_desc_init_data_structure;
    EbPictureBufferDescInitData      ref_pic_buf_desc_init_data;
    // PA Reference Picture Buffers
    // Currently, only Luma samples are needed in the PA
    ref_pic_buf_desc_init_data.max_width    = scs->max_input_luma_width;
    ref_pic_buf_desc_init_data.max_height   = scs->max_input_luma_height;
    ref_pic_buf_desc_init_data.bit_depth    = EB_EIGHT_BIT;
    ref_pic_buf_desc_init_data.color_format = EB_YUV420; //use 420 for picture analysis

    // Allocate one ref pic to be used in TPL
    ref_pic_buf_desc_init_data.buffer_enable_mask = PICTURE_BUFFER_DESC_Y_FLAG;

    ref_pic_buf_desc_init_data.left_padding      = TPL_PADX; // scs->left_padding;
    ref_pic_buf_desc_init_data.right_padding     = TPL_PADX; // scs->right_padding;
    ref_pic_buf_desc_init_data.top_padding       = TPL_PADY; // scs->top_padding;
    ref_pic_buf_desc_init_data.bot_padding       = TPL_PADY; // scs->bot_padding;
    ref_pic_buf_desc_init_data.split_mode        = false;
    ref_pic_buf_desc_init_data.mfmv              = 0;
    ref_pic_buf_desc_init_data.is_16bit_pipeline = false;

    ref_pic_buf_desc_init_data.rest_units_per_tile = 0; // rest not needed in tpl scs->rest_units_per_tile;
    ref_pic_buf_desc_init_data.sb_total_count      = scs->sb_total_count;

    eb_tpl_ref_obj_ect_desc_init_data_structure.reference_picture_desc_init_data = ref_pic_buf_desc_init_data;

    // Reference Picture Buffers
    EB_NEW(enc_handle_ptr->tpl_reference_picture_pool_ptr,
           svt_system_resource_ctor,
           scs->tpl_reference_picture_buffer_init_count,
           EB_PictureDecisionProcessInitCount,
           0,
           svt_tpl_reference_object_creator,
           &(eb_tpl_ref_obj_ect_desc_init_data_structure),
           NULL);
    // Set the SequenceControlSet Picture Pool Fifo Ptrs
    enc_handle_ptr->scs_instance->enc_ctx->tpl_reference_picture_pool_fifo_ptr = svt_system_resource_get_producer_fifo(
        enc_handle_ptr->tpl_reference_picture_pool_ptr, 0);
#if SRM_REPORT
    enc_handle_ptr->scs_instance_array->enc_ctx->tpl_reference_picture_pool_fifo_ptr->queue_ptr->log = 0;
#endif
    return 0;
}

static int create_ref_buf_descs(EbEncHandle* enc_handle_ptr) {
    EbReferenceObjectDescInitData eb_ref_obj_ect_desc_init_data_structure;
    EbPictureBufferDescInitData   ref_pic_buf_desc_init_data;
    SequenceControlSet*           scs      = enc_handle_ptr->scs_instance->scs;
    bool                          is_16bit = scs->static_config.encoder_bit_depth > EB_EIGHT_BIT;
    // Initialize the various Picture types
    ref_pic_buf_desc_init_data.max_width           = scs->max_input_luma_width;
    ref_pic_buf_desc_init_data.max_height          = scs->max_input_luma_height;
    ref_pic_buf_desc_init_data.bit_depth           = scs->encoder_bit_depth;
    ref_pic_buf_desc_init_data.color_format        = scs->static_config.encoder_color_format;
    ref_pic_buf_desc_init_data.buffer_enable_mask  = PICTURE_BUFFER_DESC_FULL_MASK;
    ref_pic_buf_desc_init_data.rest_units_per_tile = scs->rest_units_per_tile;
    ref_pic_buf_desc_init_data.sb_total_count      = scs->b64_total_count;
    uint16_t padding                               = scs->super_block_size + 32;
    if (scs->static_config.superres_mode > SUPERRES_NONE || scs->static_config.resize_mode > RESIZE_NONE) {
        padding += scs->super_block_size;
    }

    ref_pic_buf_desc_init_data.left_padding      = padding;
    ref_pic_buf_desc_init_data.right_padding     = padding;
    ref_pic_buf_desc_init_data.top_padding       = padding;
    ref_pic_buf_desc_init_data.bot_padding       = padding;
    ref_pic_buf_desc_init_data.mfmv              = scs->mfmv_enabled;
    ref_pic_buf_desc_init_data.is_16bit_pipeline = scs->is_16bit_pipeline;
    // Hsan: split_mode is set @ eb_reference_object_ctor() as both unpacked reference and packed reference are needed for a 10BIT input; unpacked reference @ MD, and packed reference @ EP

    ref_pic_buf_desc_init_data.split_mode = false;
    if (is_16bit) {
        ref_pic_buf_desc_init_data.bit_depth = EB_TEN_BIT;
    }

    eb_ref_obj_ect_desc_init_data_structure.reference_picture_desc_init_data = ref_pic_buf_desc_init_data;
    eb_ref_obj_ect_desc_init_data_structure.hbd_md                           = scs->enable_hbd_mode_decision;
    eb_ref_obj_ect_desc_init_data_structure.static_config                    = &scs->static_config;
    // Reference Picture Buffers
    EB_NEW(enc_handle_ptr->reference_picture_pool_ptr,
           svt_system_resource_ctor,
           scs->reference_picture_buffer_init_count,
           EB_PictureManagerProcessInitCount,
           0,
           svt_reference_object_creator,
           &(eb_ref_obj_ect_desc_init_data_structure),
           NULL);

    // Create reference list for Picture Manager
    // When decode-order is not enforced at pic mgr, each reference picture must have an allocated reference buffer (for at least one mini-gop) so the
    // list can be enough to hold only the reference buffers.  When decode-order is enforced, only 9 reference buffers are used, so the list must be at least 1 mini-gop
    // otherwise ref_buffer_available_semaphore will block all required pics from being passed to pic mgr.
    const uint32_t ref_pic_list_length = scs->enable_dec_order ? scs->pa_reference_picture_buffer_init_count
                                                               : scs->reference_picture_buffer_init_count;
    enc_handle_ptr->scs_instance->enc_ctx->ref_pic_list_length = ref_pic_list_length;
    EB_ALLOC_PTR_ARRAY(enc_handle_ptr->scs_instance->enc_ctx->ref_pic_list, ref_pic_list_length);

    for (uint32_t idx = 0; idx < ref_pic_list_length; ++idx) {
        EB_NEW(enc_handle_ptr->scs_instance->enc_ctx->ref_pic_list[idx], svt_aom_reference_queue_entry_ctor);
    }
    EB_CREATE_SEMAPHORE(scs->ref_buffer_available_semaphore, ref_pic_list_length, ref_pic_list_length);
    enc_handle_ptr->scs_instance->enc_ctx->reference_picture_pool_fifo_ptr = svt_system_resource_get_producer_fifo(
        enc_handle_ptr->reference_picture_pool_ptr, 0);

#if SRM_REPORT
    enc_handle_ptr->scs_instance_array->enc_ctx->reference_picture_pool_fifo_ptr->queue_ptr->log = 0;
#endif

    return 0;
}

void init_fn_ptr(void);
void svt_av1_init_wedge_masks(void);
void init_ii_masks(void);

static ONCE_ROUTINE(init_global_tables) {
    svt_aom_asm_set_convolve_asm_table();
    svt_aom_init_intra_dc_predictors_c_internal();
    svt_aom_asm_set_convolve_hbd_asm_table();
    svt_aom_init_intra_predictors_internal();
    svt_av1_init_me_luts();
    init_fn_ptr();
    svt_av1_init_wedge_masks();
    init_ii_masks();
    ONCE_ROUTINE_EPILOG;
}

DEFINE_ONCE(global_tables_once);

/**********************************
* Initialize Encoder Library
**********************************/
EB_API EbErrorType svt_av1_enc_init(EbComponentType* svt_enc_component) {
    if (svt_enc_component == NULL) {
        return EB_ErrorBadParameter;
    }
    EbEncHandle*        enc_handle_ptr = (EbEncHandle*)svt_enc_component->p_component_private;
    EbErrorType         return_error   = EB_ErrorNone;
    SequenceControlSet* scs            = enc_handle_ptr->scs_instance->scs;
    EbColorFormat       color_format   = scs->static_config.encoder_color_format;

    svt_aom_setup_common_rtcd_internal(scs->static_config.use_cpu_flags);
    svt_aom_setup_rtcd_internal(scs->static_config.use_cpu_flags);
    svt_run_once(&global_tables_once, init_global_tables);

    // Per-instance block geometry table allocation
    EB_MALLOC_ARRAY(scs->blk_geom_mds, scs->max_block_cnt);
    svt_aom_build_blk_geom(scs->svt_aom_geom_idx, scs->blk_geom_mds);
    /************************************
     * Sequence Control Set
     ************************************/
    EB_NEW(enc_handle_ptr->scs_pool_ptr,
           svt_system_resource_ctor,
           enc_handle_ptr->scs_pool_total_count,
           1,
           0,
           svt_aom_scs_set_creator,
           NULL,
           NULL);
    /************************************
    * Picture Control Set: Parent
    ************************************/
    {
        // The segment Width & Height Arrays are in units of SBs, not samples
        PictureControlSetInitData input_data;
        input_data.picture_width        = scs->max_input_luma_width;
        input_data.picture_height       = scs->max_input_luma_height;
        input_data.left_padding         = scs->left_padding;
        input_data.right_padding        = scs->right_padding;
        input_data.top_padding          = scs->top_padding;
        input_data.bot_padding          = scs->bot_padding;
        input_data.color_format         = color_format;
        input_data.b64_size             = scs->b64_size;
        input_data.enc_mode             = scs->static_config.enc_mode;
        input_data.hbd_md               = scs->enable_hbd_mode_decision;
        input_data.bit_depth            = scs->static_config.encoder_bit_depth;
        input_data.log2_tile_rows       = scs->static_config.tile_rows;
        input_data.log2_tile_cols       = scs->static_config.tile_columns;
        input_data.log2_sb_size         = (scs->super_block_size == 128) ? 5 : 4;
        input_data.is_16bit_pipeline    = scs->is_16bit_pipeline;
        input_data.non_m8_pad_w         = scs->max_input_pad_right;
        input_data.non_m8_pad_h         = scs->max_input_pad_bottom;
        input_data.enable_tpl_la        = scs->tpl;
        input_data.enc_dec_segment_col  = (uint16_t)scs->tpl_segment_col_count_array;
        input_data.enc_dec_segment_row  = (uint16_t)scs->tpl_segment_row_count_array;
        MrpCtrls* mrp_ctrl              = &(scs->mrp_ctrls);
        input_data.ref_count_used_list0 = MAX(
            mrp_ctrl->sc_base_ref_list0_count,
            MAX(mrp_ctrl->base_ref_list0_count,
                MAX(mrp_ctrl->sc_non_base_ref_list0_count, mrp_ctrl->non_base_ref_list0_count)));

        input_data.ref_count_used_list1 = MAX(
            mrp_ctrl->sc_base_ref_list1_count,
            MAX(mrp_ctrl->base_ref_list1_count,
                MAX(mrp_ctrl->sc_non_base_ref_list1_count, mrp_ctrl->non_base_ref_list1_count)));
        input_data.tpl_synth_size = svt_aom_set_tpl_group(NULL,
                                                          svt_aom_get_tpl_group_level(1, scs->static_config.enc_mode),
                                                          input_data.picture_width,
                                                          input_data.picture_height);
        input_data.aq_mode        = scs->static_config.aq_mode;

        input_data.calculate_variance = scs->calculate_variance;
        input_data.calc_hist = scs->calc_hist = scs->allintra == false &&
            (scs->static_config.scene_change_detection || scs->vq_ctrls.sharpness_ctrls.scene_transition ||
             scs->tf_params_per_type[0].enabled || scs->tf_params_per_type[1].enabled ||
             scs->tf_params_per_type[2].enabled);
        input_data.tpl_lad_mg       = scs->tpl_lad_mg;
        input_data.input_resolution = scs->input_resolution;
        input_data.is_scale         = scs->static_config.superres_mode > SUPERRES_NONE ||
            scs->static_config.resize_mode > RESIZE_NONE;
        input_data.rtc_tune            = scs->static_config.rtc;
        input_data.variance_octile     = scs->static_config.variance_octile;
        input_data.adaptive_film_grain = scs->static_config.adaptive_film_grain;
        input_data.static_config       = scs->static_config;
        input_data.allintra            = scs->allintra;
        input_data.use_flat_ipp        = scs->use_flat_ipp;
        EB_NEW(enc_handle_ptr->picture_parent_control_set_pool_ptr,
               svt_system_resource_ctor,
               scs->picture_control_set_pool_init_count, //enc_handle_ptr->pcs_pool_total_count,
               1,
               0,
               svt_aom_picture_parent_control_set_creator,
               &input_data,
               NULL);
#if SRM_REPORT
        enc_handle_ptr->picture_parent_control_set_pool_ptr_array->empty_queue->log = 0;
#endif
        EB_NEW(enc_handle_ptr->me_pool_ptr,
               svt_system_resource_ctor,
               scs->me_pool_init_count,
               1,
               0,
               svt_aom_me_creator,
               &input_data,
               NULL);
#if SRM_REPORT
        enc_handle_ptr->me_pool_ptr_array->empty_queue->log = 0;
        dump_srm_content(enc_handle_ptr->me_pool_ptr_array, false);
#endif
    }

    /************************************
    * Enc Dec
    ************************************/
    {
        // The segment Width & Height Arrays are in units of SBs, not samples
        PictureControlSetInitData input_data;
        input_data.enc_dec_segment_col = (uint16_t)scs->enc_dec_segment_col_count_array;
        input_data.enc_dec_segment_row = (uint16_t)scs->enc_dec_segment_row_count_array;

        input_data.picture_width  = scs->max_input_luma_width;
        input_data.picture_height = scs->max_input_luma_height;
        input_data.left_padding   = scs->left_padding;
        input_data.right_padding  = scs->right_padding;
        input_data.top_padding    = scs->top_padding;
        input_data.bot_padding    = scs->bot_padding;
        input_data.bit_depth      = scs->encoder_bit_depth;
        input_data.color_format   = color_format;
        input_data.b64_size       = scs->b64_size;
        input_data.sb_size        = scs->super_block_size;
        input_data.hbd_md         = scs->enable_hbd_mode_decision;
        input_data.mfmv           = scs->mfmv_enabled;
        //Jing: Get tile info from parent_pcs
        PictureParentControlSet* parent_pcs =
            (PictureParentControlSet*)enc_handle_ptr->picture_parent_control_set_pool_ptr->wrapper_ptr_pool[0]
                ->object_ptr;
        input_data.tile_row_count    = parent_pcs->av1_cm->tiles_info.tile_rows;
        input_data.tile_column_count = parent_pcs->av1_cm->tiles_info.tile_cols;
        input_data.is_16bit_pipeline = scs->is_16bit_pipeline;
        input_data.av1_cm            = parent_pcs->av1_cm;
        input_data.enc_mode          = scs->static_config.enc_mode;

        input_data.input_resolution = scs->input_resolution;
        input_data.is_scale         = scs->static_config.superres_mode > SUPERRES_NONE ||
            scs->static_config.resize_mode > RESIZE_NONE;

        input_data.rtc_tune     = scs->static_config.rtc;
        input_data.allintra     = scs->allintra;
        input_data.use_flat_ipp = scs->use_flat_ipp;
        EB_NEW(enc_handle_ptr->enc_dec_pool_ptr,
               svt_system_resource_ctor,
               scs->enc_dec_pool_init_count, //EB_PictureControlSetPoolInitCountChild,
               1,
               0,
               svt_aom_recon_coef_creator,
               &input_data,
               NULL);
    }

    /************************************
        * Picture Control Set: Child
        ************************************/
    {
        // The segment Width & Height Arrays are in units of SBs, not samples
        PictureControlSetInitData input_data;
        input_data.enc_dec_segment_col = (uint16_t)scs->enc_dec_segment_col_count_array;
        input_data.enc_dec_segment_row = (uint16_t)scs->enc_dec_segment_row_count_array;
        input_data.picture_width       = scs->max_input_luma_width;
        input_data.picture_height      = scs->max_input_luma_height;
        input_data.left_padding        = scs->left_padding;
        input_data.right_padding       = scs->right_padding;
        input_data.top_padding         = scs->top_padding;
        input_data.bot_padding         = scs->bot_padding;
        input_data.bit_depth           = scs->encoder_bit_depth;
        input_data.color_format        = color_format;
        input_data.b64_size            = scs->b64_size;
        input_data.sb_size             = scs->super_block_size;
        input_data.hbd_md              = scs->enable_hbd_mode_decision;
        input_data.mfmv                = scs->mfmv_enabled;
        //Jing: Get tile info from parent_pcs
        PictureParentControlSet* parent_pcs =
            (PictureParentControlSet*)enc_handle_ptr->picture_parent_control_set_pool_ptr->wrapper_ptr_pool[0]
                ->object_ptr;
        input_data.tile_row_count    = parent_pcs->av1_cm->tiles_info.tile_rows;
        input_data.tile_column_count = parent_pcs->av1_cm->tiles_info.tile_cols;
        input_data.is_16bit_pipeline = scs->is_16bit_pipeline;
        input_data.av1_cm            = parent_pcs->av1_cm;
        input_data.enc_mode          = scs->static_config.enc_mode;
        input_data.static_config     = scs->static_config;

        input_data.input_resolution = scs->input_resolution;
        input_data.is_scale         = scs->static_config.superres_mode > SUPERRES_NONE ||
            scs->static_config.resize_mode > RESIZE_NONE;

        input_data.rtc_tune     = scs->static_config.rtc;
        input_data.allintra     = scs->allintra;
        input_data.use_flat_ipp = scs->use_flat_ipp;
        EB_NEW(enc_handle_ptr->picture_control_set_pool_ptr,
               svt_system_resource_ctor,
               scs->picture_control_set_pool_init_count_child, //EB_PictureControlSetPoolInitCountChild,
               1,
               0,
               svt_aom_picture_control_set_creator,
               &input_data,
               NULL);
    }

    /************************************
    * Picture Buffers
    ************************************/
    // Allocate Resource Arrays
    pic_mgr_ports[PIC_MGR_INPUT_PORT_SOP].count           = scs->source_based_operations_process_init_count;
    pic_mgr_ports[PIC_MGR_INPUT_PORT_PACKETIZATION].count = EB_PacketizationProcessInitCount;
    pic_mgr_ports[PIC_MGR_INPUT_PORT_REST].count          = scs->rest_process_init_count;
    // Rate Control
    rate_control_ports[RATE_CONTROL_INPUT_PORT_INLME].count         = EB_PictureManagerProcessInitCount;
    rate_control_ports[RATE_CONTROL_INPUT_PORT_PACKETIZATION].count = EB_PacketizationProcessInitCount;

    enc_dec_ports[ENCDEC_INPUT_PORT_MDC].count    = scs->mode_decision_configuration_process_init_count;
    enc_dec_ports[ENCDEC_INPUT_PORT_ENCDEC].count = scs->enc_dec_process_init_count;
    tpl_ports[TPL_INPUT_PORT_SOP].count           = scs->source_based_operations_process_init_count;
    tpl_ports[TPL_INPUT_PORT_TPL].count           = scs->tpl_disp_process_init_count;
    {
        // Must always allocate mem b/c don't know if restoration is on or off at this point
        // The restoration assumes only 1 tile is used, so only allocate for 1 tile... see svt_av1_alloc_restoration_struct()
        PictureControlSet* pcs =
            (PictureControlSet*)enc_handle_ptr->picture_control_set_pool_ptr->wrapper_ptr_pool[0]->object_ptr;
        scs->rest_units_per_tile = pcs->rst_info[0 /*Y-plane*/].units_per_tile;
        scs->b64_total_count     = pcs->b64_total_count;
        create_ref_buf_descs(enc_handle_ptr);
        if (scs->tpl) {
            create_tpl_ref_buf_descs(enc_handle_ptr);
        }

        create_pa_ref_buf_descs(enc_handle_ptr);

        if (scs->static_config.enable_overlays) {
            // Overlay Input Picture Buffers
            EB_NEW(enc_handle_ptr->overlay_input_picture_pool_ptr,
                   svt_system_resource_ctor,
                   scs->overlay_input_picture_buffer_init_count,
                   1,
                   0,
                   svt_overlay_buffer_header_creator,
                   scs,
                   svt_input_buffer_header_destroyer);
            // Set the SequenceControlSet Overlay input Picture Pool Fifo Ptrs
            enc_handle_ptr->scs_instance->enc_ctx->overlay_input_picture_pool_fifo_ptr =
                svt_system_resource_get_producer_fifo(enc_handle_ptr->overlay_input_picture_pool_ptr, 0);
        }
    }
    /************************************
    * System Resource Managers & Fifos
    ************************************/
    //SRM to link App to Ress-Coordination via Input commands. an Input Command holds 2 picture buffers: y8bit and rest(uv8b + yuv2b)
    EB_NEW(enc_handle_ptr->input_cmd_resource_ptr,
           svt_system_resource_ctor,
           scs->resource_coordination_fifo_init_count,
           1,
           EB_ResourceCoordinationProcessInitCount,
           svt_input_cmd_creator,
           scs,
           NULL);
    enc_handle_ptr->input_cmd_producer_fifo_ptr = svt_system_resource_get_producer_fifo(
        enc_handle_ptr->input_cmd_resource_ptr, 0);

    //Picture Buffer SRM to hold (uv8b + yuv2b)
    EB_NEW(enc_handle_ptr->input_buffer_resource_ptr,
           svt_system_resource_ctor,
           scs->input_buffer_fifo_init_count,
           1,
           0, //1/2 SRM; no consumer FIFO
           svt_input_buffer_header_creator,
           scs,
           svt_input_buffer_header_destroyer);
    enc_handle_ptr->input_buffer_producer_fifo_ptr = svt_system_resource_get_producer_fifo(
        enc_handle_ptr->input_buffer_resource_ptr, 0);

    //Picture Buffer SRM to hold y8b to be shared by Pcs->enhanced and Pa_ref
    EB_NEW(enc_handle_ptr->input_y8b_buffer_resource_ptr,
           svt_system_resource_ctor,
           MAX(scs->input_buffer_fifo_init_count, scs->pa_reference_picture_buffer_init_count),
           1,
           0, //1/2 SRM; no consumer FIFO
           svt_input_y8b_creator,
           scs,
           svt_input_y8b_destroyer);

#if SRM_REPORT
    enc_handle_ptr->input_y8b_buffer_resource_ptr->empty_queue->log = 1;
#endif
    enc_handle_ptr->input_y8b_buffer_producer_fifo_ptr = svt_system_resource_get_producer_fifo(
        enc_handle_ptr->input_y8b_buffer_resource_ptr, 0);

    // EbBufferHeaderType Output Stream
    {
        EB_NEW(enc_handle_ptr->output_stream_buffer_resource_ptr,
               svt_system_resource_ctor,
               scs->output_stream_buffer_fifo_init_count,
               scs->total_process_init_count, //EB_PacketizationProcessInitCount,
               1,
               svt_output_buffer_header_creator,
               &scs->static_config,
               svt_output_buffer_header_destroyer);
    }
    enc_handle_ptr->output_stream_buffer_consumer_fifo_ptr = svt_system_resource_get_consumer_fifo(
        enc_handle_ptr->output_stream_buffer_resource_ptr, 0);
    if (scs->static_config.recon_enabled) {
        // EbBufferHeaderType Output Recon
        {
            EB_NEW(enc_handle_ptr->output_recon_buffer_resource_ptr,
                   svt_system_resource_ctor,
                   scs->output_recon_buffer_fifo_init_count,
                   scs->enc_dec_process_init_count,
                   1,
                   svt_output_recon_buffer_header_creator,
                   scs,
                   svt_output_recon_buffer_header_destroyer);
        }
        enc_handle_ptr->output_recon_buffer_consumer_fifo_ptr = svt_system_resource_get_consumer_fifo(
            enc_handle_ptr->output_recon_buffer_resource_ptr, 0);
    }

    // Resource Coordination Results
    {
        ResourceCoordinationResultInitData resource_coordination_result_init_data;
        EB_NEW(enc_handle_ptr->resource_coordination_results_resource_ptr,
               svt_system_resource_ctor,
               scs->resource_coordination_fifo_init_count,
               EB_ResourceCoordinationProcessInitCount,
               scs->picture_analysis_process_init_count,
               svt_aom_resource_coordination_result_creator,
               &resource_coordination_result_init_data,
               NULL);
    }

    // Picture Analysis Results
    {
        PictureAnalysisResultInitData picture_analysis_result_init_data;
        EB_NEW(enc_handle_ptr->picture_analysis_results_resource_ptr,
               svt_system_resource_ctor,
               scs->picture_analysis_fifo_init_count,
               scs->picture_analysis_process_init_count,
               EB_PictureDecisionProcessInitCount,
               svt_aom_picture_analysis_result_creator,
               &picture_analysis_result_init_data,
               NULL);
    }

    // Picture Decision Results
    {
        PictureDecisionResultInitData picture_decision_result_init_data;
        EB_NEW(enc_handle_ptr->picture_decision_results_resource_ptr,
               svt_system_resource_ctor,
               scs->picture_decision_fifo_init_count,
               EB_PictureDecisionProcessInitCount +
                   2, // 1 for rate control, another 1 for packetization when superres recoding is on
               scs->motion_estimation_process_init_count,
               svt_aom_picture_decision_result_creator,
               &picture_decision_result_init_data,
               NULL);
        EB_ALLOC_PTR_ARRAY(scs->enc_ctx->picture_decision_reorder_queue,
                           scs->enc_ctx->picture_decision_reorder_queue_size);

        for (uint32_t picture_index = 0; picture_index < scs->enc_ctx->picture_decision_reorder_queue_size;
             ++picture_index) {
            EB_NEW(scs->enc_ctx->picture_decision_reorder_queue[picture_index],
                   svt_aom_picture_decision_reorder_entry_ctor,
                   picture_index);
        }
    }

    // Motion Estimation Results
    {
        MotionEstimationResultsInitData motion_estimation_result_init_data;
        EB_NEW(enc_handle_ptr->motion_estimation_results_resource_ptr,
               svt_system_resource_ctor,
               scs->motion_estimation_fifo_init_count,
               scs->motion_estimation_process_init_count,
               EB_InitialRateControlProcessInitCount,
               svt_aom_motion_estimation_results_creator,
               &motion_estimation_result_init_data,
               NULL);
    }

    // Initial Rate Control Results
    {
        InitialRateControlResultInitData initial_rate_control_result_init_data;
        EB_NEW(enc_handle_ptr->initial_rate_control_results_resource_ptr,
               svt_system_resource_ctor,
               scs->initial_rate_control_fifo_init_count,
               EB_InitialRateControlProcessInitCount,
               scs->source_based_operations_process_init_count,
               svt_aom_initial_rate_control_results_creator,
               &initial_rate_control_result_init_data,
               NULL);
    }

    // Picture Demux Results
    {
        PictureResultInitData picture_result_init_data;
        EB_NEW(enc_handle_ptr->picture_demux_results_resource_ptr,
               svt_system_resource_ctor,
               scs->picture_demux_fifo_init_count,
               pic_mgr_port_total_count(),
               EB_PictureManagerProcessInitCount,
               svt_aom_picture_results_creator,
               &picture_result_init_data,
               NULL);

        EB_ALLOC_PTR_ARRAY(scs->enc_ctx->pic_mgr_input_pic_list, scs->enc_ctx->pic_mgr_input_pic_list_size);

        for (uint32_t picture_index = 0; picture_index < scs->enc_ctx->pic_mgr_input_pic_list_size; ++picture_index) {
            EB_NEW(scs->enc_ctx->pic_mgr_input_pic_list[picture_index], svt_aom_input_queue_entry_ctor);
        }
    }

    // TPL dispenser Results
    {
        EntropyCodingResultsInitData tpl_disp_result_init_data;
        //TPL Dispenser tasks
        EB_NEW(enc_handle_ptr->tpl_disp_res_srm,
               svt_system_resource_ctor,
               scs->tpl_disp_fifo_init_count,
               tpl_port_total_count(),
               scs->tpl_disp_process_init_count,
               tpl_disp_results_creator,
               &tpl_disp_result_init_data,
               NULL);
    }

    // Rate Control Tasks
    {
        RateControlTasksInitData rate_control_tasks_init_data;
        EB_NEW(enc_handle_ptr->rate_control_tasks_resource_ptr,
               svt_system_resource_ctor,
               scs->rate_control_tasks_fifo_init_count,
               rate_control_port_total_count(),
               EB_RateControlProcessInitCount,
               svt_aom_rate_control_tasks_creator,
               &rate_control_tasks_init_data,
               NULL);
    }

    // Rate Control Results
    {
        RateControlResultsInitData rate_control_result_init_data;
        EB_NEW(enc_handle_ptr->rate_control_results_resource_ptr,
               svt_system_resource_ctor,
               scs->rate_control_fifo_init_count,
               EB_RateControlProcessInitCount,
               scs->mode_decision_configuration_process_init_count,
               svt_aom_rate_control_results_creator,
               &rate_control_result_init_data,
               NULL);
    }
    // EncDec Tasks
    {
        EncDecTasksInitData mode_decision_result_init_data;
        mode_decision_result_init_data.enc_dec_segment_row_count = scs->enc_dec_segment_row_count_array;
        EB_NEW(enc_handle_ptr->enc_dec_tasks_resource_ptr,
               svt_system_resource_ctor,
               scs->mode_decision_configuration_fifo_init_count,
               enc_dec_port_total_count(),
               scs->enc_dec_process_init_count,
               svt_aom_enc_dec_tasks_creator,
               &mode_decision_result_init_data,
               NULL);
    }

    // EncDec Results
    {
        EncDecResultsInitData enc_dec_result_init_data;
        EB_NEW(enc_handle_ptr->enc_dec_results_resource_ptr,
               svt_system_resource_ctor,
               scs->enc_dec_fifo_init_count,
               scs->enc_dec_process_init_count,
               scs->dlf_process_init_count,
               svt_aom_enc_dec_results_creator,
               &enc_dec_result_init_data,
               NULL);
    }

    //DLF results
    {
        EntropyCodingResultsInitData delf_result_init_data;
        EB_NEW(enc_handle_ptr->dlf_results_resource_ptr,
               svt_system_resource_ctor,
               scs->dlf_fifo_init_count,
               scs->dlf_process_init_count,
               scs->cdef_process_init_count,
               dlf_results_creator,
               &delf_result_init_data,
               NULL);
    }
    //CDEF results
    {
        EntropyCodingResultsInitData cdef_result_init_data;
        EB_NEW(enc_handle_ptr->cdef_results_resource_ptr,
               svt_system_resource_ctor,
               scs->cdef_fifo_init_count,
               scs->cdef_process_init_count,
               scs->rest_process_init_count,
               cdef_results_creator,
               &cdef_result_init_data,
               NULL);
    }
    //REST results
    {
        EntropyCodingResultsInitData rest_result_init_data;
        EB_NEW(enc_handle_ptr->rest_results_resource_ptr,
               svt_system_resource_ctor,
               scs->rest_fifo_init_count,
               scs->rest_process_init_count,
               scs->entropy_coding_process_init_count,
               rest_results_creator,
               &rest_result_init_data,
               NULL);
    }

    // Entropy Coding Results
    {
        EntropyCodingResultsInitData entropy_coding_results_init_data;
        EB_NEW(enc_handle_ptr->entropy_coding_results_resource_ptr,
               svt_system_resource_ctor,
               scs->entropy_coding_fifo_init_count,
               scs->entropy_coding_process_init_count,
               EB_PacketizationProcessInitCount,
               svt_aom_entropy_coding_results_creator,
               &entropy_coding_results_init_data,
               NULL);
        EB_ALLOC_PTR_ARRAY(scs->enc_ctx->packetization_reorder_queue, scs->enc_ctx->packetization_reorder_queue_size);

        for (uint32_t picture_index = 0; picture_index < scs->enc_ctx->packetization_reorder_queue_size;
             ++picture_index) {
            EB_NEW(scs->enc_ctx->packetization_reorder_queue[picture_index],
                   svt_aom_packetization_reorder_entry_ctor,
                   picture_index);
        }
    }

    /************************************
    * App Callbacks
    ************************************/
    enc_handle_ptr->scs_instance->enc_ctx->app_callback_ptr = enc_handle_ptr->app_callback_ptr;
    // svt Output Buffer Fifo Ptrs
    enc_handle_ptr->scs_instance->enc_ctx->stream_output_fifo_ptr = svt_system_resource_get_producer_fifo(
        enc_handle_ptr->output_stream_buffer_resource_ptr, 0);
    if (enc_handle_ptr->scs_instance->scs->static_config.recon_enabled) {
        enc_handle_ptr->scs_instance->enc_ctx->recon_output_fifo_ptr = svt_system_resource_get_producer_fifo(
            enc_handle_ptr->output_recon_buffer_resource_ptr, 0);
    }

    /************************************
    * Contexts
    ************************************/
    // Resource Coordination Context
    EB_NEW(
        enc_handle_ptr->resource_coordination_context_ptr, svt_aom_resource_coordination_context_ctor, enc_handle_ptr);

    // Picture Analysis Context
    EB_ALLOC_PTR_ARRAY(enc_handle_ptr->picture_analysis_context_ptr_array, scs->picture_analysis_process_init_count);
    for (uint32_t process_index = 0; process_index < scs->picture_analysis_process_init_count; process_index++) {
        EB_NEW(enc_handle_ptr->picture_analysis_context_ptr_array[process_index],
               svt_aom_picture_analysis_context_ctor,
               enc_handle_ptr,
               process_index);
    }

    // Picture Decision Context
    EB_NEW(enc_handle_ptr->picture_decision_context_ptr,
           svt_aom_picture_decision_context_ctor,
           enc_handle_ptr,
           scs->calc_hist);

    // Motion Analysis Context
    EB_ALLOC_PTR_ARRAY(enc_handle_ptr->motion_estimation_context_ptr_array, scs->motion_estimation_process_init_count);
    for (uint32_t process_index = 0; process_index < scs->motion_estimation_process_init_count; process_index++) {
        EB_NEW(enc_handle_ptr->motion_estimation_context_ptr_array[process_index],
               svt_aom_motion_estimation_context_ctor,
               enc_handle_ptr,
               process_index);
    }

    // Initial Rate Control Context
    EB_NEW(enc_handle_ptr->initial_rate_control_context_ptr,
           svt_aom_initial_rate_control_context_ctor,
           enc_handle_ptr,
           scs->picture_control_set_pool_init_count);

    // Source Based Operations Context
    EB_ALLOC_PTR_ARRAY(enc_handle_ptr->source_based_operations_context_ptr_array,
                       scs->source_based_operations_process_init_count);
    for (uint32_t process_index = 0; process_index < scs->source_based_operations_process_init_count; process_index++) {
        EB_NEW(enc_handle_ptr->source_based_operations_context_ptr_array[process_index],
               svt_aom_source_based_operations_context_ctor,
               enc_handle_ptr,
               tpl_port_lookup(TPL_INPUT_PORT_SOP, process_index),
               pic_mgr_port_lookup(PIC_MGR_INPUT_PORT_SOP, process_index));
    }

    // TPL dispenser
    EB_ALLOC_PTR_ARRAY(enc_handle_ptr->tpl_disp_context_ptr_array, scs->tpl_disp_process_init_count);
    for (uint32_t process_index = 0; process_index < scs->tpl_disp_process_init_count; process_index++) {
        EB_NEW(enc_handle_ptr->tpl_disp_context_ptr_array[process_index],
               svt_aom_tpl_disp_context_ctor,
               enc_handle_ptr,
               process_index,
               tpl_port_lookup(TPL_INPUT_PORT_TPL, process_index));
    }

    // Picture Manager Context
    EB_NEW(enc_handle_ptr->picture_manager_context_ptr,
           svt_aom_picture_manager_context_ctor,
           enc_handle_ptr,
           rate_control_port_lookup(RATE_CONTROL_INPUT_PORT_INLME, 0), //Pic-Mgr uses the first Port
           scs->picture_control_set_pool_init_count);

    // Rate Control Context
    EB_NEW(enc_handle_ptr->rate_control_context_ptr,
           svt_aom_rate_control_context_ctor,
           enc_handle_ptr,
           EB_PictureDecisionProcessInitCount); // me_port_index

    // Mode Decision Configuration Contexts
    EB_ALLOC_PTR_ARRAY(enc_handle_ptr->mode_decision_configuration_context_ptr_array,
                       scs->mode_decision_configuration_process_init_count);
    for (uint32_t process_index = 0; process_index < scs->mode_decision_configuration_process_init_count;
         process_index++) {
        EB_NEW(enc_handle_ptr->mode_decision_configuration_context_ptr_array[process_index],
               svt_aom_mode_decision_configuration_context_ctor,
               enc_handle_ptr,
               process_index,
               enc_dec_port_lookup(ENCDEC_INPUT_PORT_MDC, process_index));
    }

    // EncDec Contexts
    EB_ALLOC_PTR_ARRAY(enc_handle_ptr->enc_dec_context_ptr_array, scs->enc_dec_process_init_count);
    for (uint32_t process_index = 0; process_index < scs->enc_dec_process_init_count; process_index++) {
        EB_NEW(enc_handle_ptr->enc_dec_context_ptr_array[process_index],
               svt_aom_enc_dec_context_ctor,
               enc_handle_ptr,
               process_index,
               enc_dec_port_lookup(ENCDEC_INPUT_PORT_ENCDEC, process_index));
    }

    // Dlf Contexts
    EB_ALLOC_PTR_ARRAY(enc_handle_ptr->dlf_context_ptr_array, scs->dlf_process_init_count);
    for (uint32_t process_index = 0; process_index < scs->dlf_process_init_count; process_index++) {
        EB_NEW(enc_handle_ptr->dlf_context_ptr_array[process_index],
               svt_aom_dlf_context_ctor,
               enc_handle_ptr,
               process_index);
    }

    //CDEF Contexts
    EB_ALLOC_PTR_ARRAY(enc_handle_ptr->cdef_context_ptr_array, scs->cdef_process_init_count);
    for (uint32_t process_index = 0; process_index < scs->cdef_process_init_count; process_index++) {
        EB_NEW(enc_handle_ptr->cdef_context_ptr_array[process_index],
               svt_aom_cdef_context_ctor,
               enc_handle_ptr,
               process_index);
    }

    //Rest Contexts
    EB_ALLOC_PTR_ARRAY(enc_handle_ptr->rest_context_ptr_array, scs->rest_process_init_count);
    for (uint32_t process_index = 0; process_index < scs->rest_process_init_count; process_index++) {
        EB_NEW(enc_handle_ptr->rest_context_ptr_array[process_index],
               svt_aom_rest_context_ctor,
               enc_handle_ptr,
               process_index,
               pic_mgr_port_lookup(PIC_MGR_INPUT_PORT_REST, process_index));
    }

    // Entropy Coding Contexts
    EB_ALLOC_PTR_ARRAY(enc_handle_ptr->entropy_coding_context_ptr_array, scs->entropy_coding_process_init_count);
    for (uint32_t process_index = 0; process_index < scs->entropy_coding_process_init_count; process_index++) {
        EB_NEW(enc_handle_ptr->entropy_coding_context_ptr_array[process_index],
               svt_aom_entropy_coding_context_ctor,
               enc_handle_ptr,
               process_index);
    }

    // Packetization Context
    EB_NEW(enc_handle_ptr->packetization_context_ptr,
           svt_aom_packetization_context_ctor,
           enc_handle_ptr,
           rate_control_port_lookup(RATE_CONTROL_INPUT_PORT_PACKETIZATION, 0),
           pic_mgr_port_lookup(PIC_MGR_INPUT_PORT_PACKETIZATION, 0),
           EB_PictureDecisionProcessInitCount + EB_RateControlProcessInitCount); // me_port_index

    /************************************
    * Thread Handles
    ************************************/
    // Resource Coordination
    EB_CREATE_THREAD(enc_handle_ptr->resource_coordination_thread_handle,
                     svt_aom_resource_coordination_kernel,
                     enc_handle_ptr->resource_coordination_context_ptr);
    EB_CREATE_THREAD_ARRAY(enc_handle_ptr->picture_analysis_thread_handle_array,
                           scs->picture_analysis_process_init_count,
                           svt_aom_picture_analysis_kernel,
                           enc_handle_ptr->picture_analysis_context_ptr_array);

    // Picture Decision
    EB_CREATE_THREAD(enc_handle_ptr->picture_decision_thread_handle,
                     svt_aom_picture_decision_kernel,
                     enc_handle_ptr->picture_decision_context_ptr);

    // Motion Estimation
    EB_CREATE_THREAD_ARRAY(enc_handle_ptr->motion_estimation_thread_handle_array,
                           scs->motion_estimation_process_init_count,
                           svt_aom_motion_estimation_kernel,
                           enc_handle_ptr->motion_estimation_context_ptr_array);

    // Initial Rate Control
    EB_CREATE_THREAD(enc_handle_ptr->initial_rate_control_thread_handle,
                     svt_aom_initial_rate_control_kernel,
                     enc_handle_ptr->initial_rate_control_context_ptr);

    // Source Based Oprations
    EB_CREATE_THREAD_ARRAY(enc_handle_ptr->source_based_operations_thread_handle_array,
                           scs->source_based_operations_process_init_count,
                           svt_aom_source_based_operations_kernel,
                           enc_handle_ptr->source_based_operations_context_ptr_array);

    // TPL dispenser
    EB_CREATE_THREAD_ARRAY(enc_handle_ptr->tpl_disp_thread_handle_array,
                           scs->tpl_disp_process_init_count,
                           svt_aom_tpl_disp_kernel, //TODOOMK
                           enc_handle_ptr->tpl_disp_context_ptr_array);
    // Picture Manager
    EB_CREATE_THREAD(enc_handle_ptr->picture_manager_thread_handle,
                     svt_aom_picture_manager_kernel,
                     enc_handle_ptr->picture_manager_context_ptr);
    // Rate Control
    EB_CREATE_THREAD(enc_handle_ptr->rate_control_thread_handle,
                     svt_aom_rate_control_kernel,
                     enc_handle_ptr->rate_control_context_ptr);

    // Mode Decision Configuration Process
    EB_CREATE_THREAD_ARRAY(enc_handle_ptr->mode_decision_configuration_thread_handle_array,
                           scs->mode_decision_configuration_process_init_count,
                           svt_aom_mode_decision_configuration_kernel,
                           enc_handle_ptr->mode_decision_configuration_context_ptr_array);

    // EncDec Process
    EB_CREATE_THREAD_ARRAY(enc_handle_ptr->enc_dec_thread_handle_array,
                           scs->enc_dec_process_init_count,
                           svt_aom_mode_decision_kernel,
                           enc_handle_ptr->enc_dec_context_ptr_array);

    // Dlf Process
    EB_CREATE_THREAD_ARRAY(enc_handle_ptr->dlf_thread_handle_array,
                           scs->dlf_process_init_count,
                           svt_aom_dlf_kernel,
                           enc_handle_ptr->dlf_context_ptr_array);

    // Cdef Process
    EB_CREATE_THREAD_ARRAY(enc_handle_ptr->cdef_thread_handle_array,
                           scs->cdef_process_init_count,
                           svt_aom_cdef_kernel,
                           enc_handle_ptr->cdef_context_ptr_array);

    // Rest Process
    EB_CREATE_THREAD_ARRAY(enc_handle_ptr->rest_thread_handle_array,
                           scs->rest_process_init_count,
                           svt_aom_rest_kernel,
                           enc_handle_ptr->rest_context_ptr_array);

    // Entropy Coding Process
    EB_CREATE_THREAD_ARRAY(enc_handle_ptr->entropy_coding_thread_handle_array,
                           scs->entropy_coding_process_init_count,
                           svt_aom_entropy_coding_kernel,
                           enc_handle_ptr->entropy_coding_context_ptr_array);
    // Packetization
    EB_CREATE_THREAD(enc_handle_ptr->packetization_thread_handle,
                     svt_aom_packetization_kernel,
                     enc_handle_ptr->packetization_context_ptr);

    svt_print_memory_usage();

    return return_error;
}

static EbErrorType enc_drain_queue(EbComponentType* svt_enc_component) {
    bool eos = false;
    do {
        EbBufferHeaderType* receive_buffer = NULL;
        EbErrorType         return_error;
        switch ((return_error = svt_av1_enc_get_packet(svt_enc_component, &receive_buffer, 1))) {
        case EB_ErrorMax:
            return EB_ErrorMax;
        case EB_NoErrorEmptyQueue:
            eos = true;
            break;
        default:
            break;
        }
        if (receive_buffer) {
            eos = receive_buffer->flags & EB_BUFFERFLAG_EOS;
            svt_av1_enc_release_out_buffer(&receive_buffer);
            receive_buffer = NULL;
        }
    } while (!eos);
    return EB_ErrorNone;
}

/**********************************
* DeInitialize Encoder Library
**********************************/
EB_API EbErrorType svt_av1_enc_deinit(EbComponentType* svt_enc_component) {
    if (!svt_enc_component || !svt_enc_component->p_component_private) {
        return EB_ErrorBadParameter;
    }

    EbEncHandle* handle = svt_enc_component->p_component_private;

    if (handle->input_y8b_buffer_producer_fifo_ptr && handle->frame_received) {
        if (!handle->eos_received) {
            SVT_ERROR("deinit called without sending EOS!\n");
            svt_av1_enc_send_picture(svt_enc_component, &(EbBufferHeaderType){.flags = EB_BUFFERFLAG_EOS});
        }

        EbErrorType return_error = enc_drain_queue(svt_enc_component);
        if (return_error != EB_ErrorNone) {
            return return_error;
        }
    }

    // Free per-instance block geometry table
    if (handle->scs_instance && handle->scs_instance->scs && handle->scs_instance->scs->blk_geom_mds != NULL) {
        EB_FREE_ARRAY(handle->scs_instance->scs->blk_geom_mds);
    }

    svt_shutdown_process(handle->input_buffer_resource_ptr);
    svt_shutdown_process(handle->input_cmd_resource_ptr);
    svt_shutdown_process(handle->resource_coordination_results_resource_ptr);
    svt_shutdown_process(handle->picture_analysis_results_resource_ptr);
    svt_shutdown_process(handle->picture_decision_results_resource_ptr);
    svt_shutdown_process(handle->motion_estimation_results_resource_ptr);
    svt_shutdown_process(handle->initial_rate_control_results_resource_ptr);
    svt_shutdown_process(handle->picture_demux_results_resource_ptr);
    svt_shutdown_process(handle->tpl_disp_res_srm);
    svt_shutdown_process(handle->rate_control_tasks_resource_ptr);
    svt_shutdown_process(handle->rate_control_results_resource_ptr);
    svt_shutdown_process(handle->enc_dec_tasks_resource_ptr);
    svt_shutdown_process(handle->enc_dec_results_resource_ptr);
    svt_shutdown_process(handle->entropy_coding_results_resource_ptr);
    svt_shutdown_process(handle->dlf_results_resource_ptr);
    svt_shutdown_process(handle->cdef_results_resource_ptr);
    svt_shutdown_process(handle->rest_results_resource_ptr);

    return EB_ErrorNone;
}

static EbErrorType init_svt_av1_encoder_handle(EbComponentType* hComponent);

/**********************************
* GetHandle
**********************************/
EB_API EbErrorType svt_av1_enc_init_handle(
    EbComponentType**         p_handle, // Function to be called in the future for manipulating the component
    EbSvtAv1EncConfiguration* config_ptr) // pointer passed back to the client during callbacks

{
    if (p_handle == NULL) {
        return EB_ErrorBadParameter;
    }

    EB_MALLOC_OBJECT(*p_handle);
    // Init Component OS objects (threads, semaphores, etc.)
    // also links the various Component control functions
    EbErrorType return_error = init_svt_av1_encoder_handle(*p_handle);

    if (return_error == EB_ErrorNone) {
        return_error = svt_av1_set_default_params(config_ptr);
    }
    if (return_error != EB_ErrorNone) {
        svt_av1_enc_deinit(*p_handle);
        EB_FREE(*p_handle);
        *p_handle = NULL;
        return return_error;
    }
    svt_increase_component_count();
    return return_error;
}

/**********************************
* Encoder Componenet DeInit
**********************************/
EbErrorType svt_av1_enc_component_de_init(EbComponentType* svt_enc_component) {
    EbErrorType return_error = EB_ErrorNone;

    if (svt_enc_component->p_component_private) {
        EbEncHandle* handle = (EbEncHandle*)svt_enc_component->p_component_private;
        EB_DELETE(handle);
        svt_enc_component->p_component_private = NULL;
    } else {
        return_error = EB_ErrorUndefined;
    }
    return return_error;
}

/**********************************
* svt_av1_enc_deinit_handle
**********************************/
EB_API EbErrorType svt_av1_enc_deinit_handle(EbComponentType* svt_enc_component) {
    if (svt_enc_component) {
        EbErrorType return_error = svt_av1_enc_component_de_init(svt_enc_component);

        EB_FREE(svt_enc_component);
        svt_decrease_component_count();
        return return_error;
    }
    return EB_ErrorInvalidComponent;
}

// Sets the default intra period the closest possible to 1 second without breaking the minigop
static int32_t compute_default_intra_period(SequenceControlSet* scs) {
    EbSvtAv1EncConfiguration* config = &scs->static_config;

    double  fps           = scs->frame_rate;
    int32_t mini_gop_size = (1 << (config->hierarchical_levels));

    // use a 5-sec gop by default.
    int32_t intra_period = (int)((fps + mini_gop_size) / mini_gop_size) * mini_gop_size * 5;
    if (config->intra_refresh_type == 1) {
        intra_period -= 1;
    }

    return intra_period;
}

/*
Calculates the default LAD value
*/
static uint32_t compute_default_look_ahead(EbSvtAv1EncConfiguration* config) {
    int32_t  lad;
    uint32_t mg_size = 1 << config->hierarchical_levels;

    /*To accomodate FFMPEG EOS, 1 frame delay is needed in Resource coordination.
       note that we have the option to not add 1 frame delay of Resource Coordination. In this case we have wait for first I frame
       to be released back to be able to start first base(16). Anyway poc16 needs to wait for poc0 to finish.*/
    uint32_t eos_delay    = 1;
    uint32_t max_tf_delay = 6;

    if (config->rate_control_mode == SVT_AV1_RC_MODE_CQP_OR_CRF) {
        lad = (1 + mg_size) * (1 + MIN_LAD_MG) + max_tf_delay + eos_delay;
    } else {
        lad = (1 + mg_size) * (1 + RC_DEFAULT_LAD_MG) + max_tf_delay + eos_delay;
    }

    lad = lad > MAX_LAD ? MAX_LAD : lad; // clip to max allowed lad
    return lad;
}

/*
Updates the LAD value
*/
static void update_look_ahead(SequenceControlSet* scs) {
    /*To accomodate FFMPEG EOS, 1 frame delay is needed in Resource coordination.
           note that we have the option to not add 1 frame delay of Resource Coordination. In this case we have wait for first I frame
           to be released back to be able to start first base(16). Anyway poc16 needs to wait for poc0 to finish.*/
    uint32_t eos_delay = 1;

    uint32_t mg_size = 1 << scs->static_config.hierarchical_levels;
    if ((int32_t)(scs->static_config.look_ahead_distance - (eos_delay + scs->scd_delay)) < (int32_t)(mg_size + 1)) {
        // Not enough pictures to form the minigop. update mg_size
        scs->static_config.look_ahead_distance = mg_size + 1 + (eos_delay + scs->scd_delay);
        SVT_WARN("Minimum lookahead distance to run %dL with TF %d is %d. Force the look_ahead_distance to be %d\n",
                 scs->static_config.hierarchical_levels + 1,
                 scs->static_config.enable_tf,
                 scs->static_config.look_ahead_distance,
                 scs->static_config.look_ahead_distance);
    }

    int32_t picture_in_future = scs->static_config.look_ahead_distance;
    // Subtract pictures used for scd_delay and eos_delay
    picture_in_future = MAX(0, (int32_t)(picture_in_future - eos_delay - scs->scd_delay));
    // Subtract pictures used for minigop formation. Unit= 1(provision for a potential delayI)
    picture_in_future = MAX(0, (int32_t)(picture_in_future - (1 + mg_size)));
    // Specify the number of mini-gops to be used in the sliding window. 0: 1 mini-gop, 1: 2 mini-gops and 3: 3 mini-gops
    scs->lad_mg = (picture_in_future + (mg_size + 1) / 2) / (mg_size + 1);
    // Since TPL is tuned for 0, 1 and 2 mini-gops, we make sure lad_mg is not smaller than tpl_lad_mg
    if (scs->lad_mg < scs->tpl_lad_mg) {
        scs->lad_mg                            = scs->tpl_lad_mg;
        scs->static_config.look_ahead_distance = (1 + mg_size) * (scs->lad_mg + 1) + scs->scd_delay + eos_delay;
        SVT_WARN(
            "Lookahead distance is not long enough to get best bdrate trade off. Force the look_ahead_distance to be "
            "%d\n",
            scs->static_config.look_ahead_distance);
    } else if (scs->lad_mg > scs->tpl_lad_mg &&
               (scs->static_config.rate_control_mode == SVT_AV1_RC_MODE_CQP_OR_CRF ||
                scs->static_config.pass == ENC_FIRST_PASS || scs->static_config.pass == ENC_SECOND_PASS)) {
        scs->lad_mg                            = scs->tpl_lad_mg;
        scs->static_config.look_ahead_distance = (1 + mg_size) * (scs->lad_mg + 1) + scs->scd_delay + eos_delay;
        SVT_WARN(
            "For CRF or 2PASS RC mode, the maximum needed Lookahead distance is %d. Force the look_ahead_distance to "
            "be %d\n",
            scs->static_config.look_ahead_distance,
            scs->static_config.look_ahead_distance);
    }
}

/*
 * Control TF
 */
uint8_t svt_aom_tf_max_ref_per_struct(uint32_t hierarchical_levels, uint8_t type /*I_SLICE, BASE, L1*/,
                                      bool direction /*Past, Future*/) {
    uint8_t max_ref_per;
    (void)direction;
    if (type == 0) { // I_SLICE
        max_ref_per = 1 << hierarchical_levels;
    } else if (type == 1) { // BASE
        max_ref_per = TF_MAX_BASE_REF_PICS;
    } else { // L1
        max_ref_per = hierarchical_levels < 5 ? TF_MAX_L1_REF_PICS_SUB_6L : TF_MAX_L1_REF_PICS_6L;
    }

    return max_ref_per;
}

/******************************************************************************
* tf_ld_controls
* TF control functions for low delay mode
*******************************************************************************/
static void tf_ld_controls(SequenceControlSet* scs, uint8_t tf_level) {
    switch (tf_level) {
    case 0:
        // I_SLICE TF Params
        scs->tf_params_per_type[0].enabled = 0;

        // BASE TF Params
        scs->tf_params_per_type[1].enabled = 0;

        // L1 TF Params
        scs->tf_params_per_type[2].enabled = 0;
        break;

    case 1:
        // I_SLICE TF Params
        scs->tf_params_per_type[0].enabled = 0;
        // BASE TF Params
        scs->tf_params_per_type[1].enabled                 = 1;
        scs->tf_params_per_type[1].num_past_pics           = 1;
        scs->tf_params_per_type[1].num_future_pics         = 0;
        scs->tf_params_per_type[1].modulate_pics           = 0;
        scs->tf_params_per_type[1].max_num_past_pics       = 1;
        scs->tf_params_per_type[1].max_num_future_pics     = 0;
        scs->tf_params_per_type[1].hme_me_level            = 4;
        scs->tf_params_per_type[1].half_pel_mode           = 0;
        scs->tf_params_per_type[1].quarter_pel_mode        = 0;
        scs->tf_params_per_type[1].eight_pel_mode          = 0;
        scs->tf_params_per_type[1].chroma_lvl              = 1;
        scs->tf_params_per_type[1].pred_error_32x32_th     = 20 * 32 * 32;
        scs->tf_params_per_type[1].sub_sampling_shift      = 0;
        scs->tf_params_per_type[1].use_zz_based_filter     = 1;
        scs->tf_params_per_type[1].avoid_2d_qpel           = 0;
        scs->tf_params_per_type[1].use_2tap                = 0;
        scs->tf_params_per_type[1].use_intra_for_noise_est = 0;
        scs->tf_params_per_type[1].use_8bit_subpel         = 0;
        scs->tf_params_per_type[1].use_pred_64x64_only_th  = 0;
        scs->tf_params_per_type[1].me_exit_th              = 0;
        scs->tf_params_per_type[1].subpel_early_exit_th    = 1;
        scs->tf_params_per_type[1].ref_frame_factor        = 1;
        scs->tf_params_per_type[1].qp_opt                  = 0;
        // L1 TF Params
        scs->tf_params_per_type[2].enabled = 0;
        break;
    case 2:
        // I_SLICE TF Params
        scs->tf_params_per_type[0].enabled = 0;
        // BASE TF Params
        scs->tf_params_per_type[1].enabled                 = 1;
        scs->tf_params_per_type[1].num_past_pics           = 1;
        scs->tf_params_per_type[1].num_future_pics         = 0;
        scs->tf_params_per_type[1].modulate_pics           = 0;
        scs->tf_params_per_type[1].max_num_past_pics       = 1;
        scs->tf_params_per_type[1].max_num_future_pics     = 0;
        scs->tf_params_per_type[1].hme_me_level            = 4;
        scs->tf_params_per_type[1].half_pel_mode           = 0;
        scs->tf_params_per_type[1].quarter_pel_mode        = 0;
        scs->tf_params_per_type[1].eight_pel_mode          = 0;
        scs->tf_params_per_type[1].chroma_lvl              = 2;
        scs->tf_params_per_type[1].pred_error_32x32_th     = (uint64_t)~0;
        scs->tf_params_per_type[1].sub_sampling_shift      = 0;
        scs->tf_params_per_type[1].use_zz_based_filter     = 1;
        scs->tf_params_per_type[1].avoid_2d_qpel           = 0;
        scs->tf_params_per_type[1].use_2tap                = 0;
        scs->tf_params_per_type[1].use_intra_for_noise_est = 0;
        scs->tf_params_per_type[1].use_8bit_subpel         = 0;
        scs->tf_params_per_type[1].use_pred_64x64_only_th  = 0;
        scs->tf_params_per_type[1].me_exit_th              = 0;
        scs->tf_params_per_type[1].subpel_early_exit_th    = 0;
        scs->tf_params_per_type[1].ref_frame_factor        = 1;
        scs->tf_params_per_type[1].qp_opt                  = 0;
        // L1 TF Params
        scs->tf_params_per_type[2].enabled = 0;
        break;

    default:
        assert(0);
        break;
    }
    // 8x8 path not supported in LD TF
    scs->tf_params_per_type[0].enable_8x8_pred = 0;
    scs->tf_params_per_type[1].enable_8x8_pred = 0;
    scs->tf_params_per_type[2].enable_8x8_pred = 0;
}

void tf_controls(SequenceControlSet* scs, uint8_t tf_level) {
    switch (tf_level) {
    case 0:
        // I_SLICE TF Params
        scs->tf_params_per_type[0].enabled = 0;

        // BASE TF Params
        scs->tf_params_per_type[1].enabled = 0;

        // L1 TF Params
        scs->tf_params_per_type[2].enabled = 0;
        break;

    case 1:
        // I_SLICE TF Params
        scs->tf_params_per_type[0].enabled             = 1;
        scs->tf_params_per_type[0].num_future_pics     = 24;
        scs->tf_params_per_type[0].modulate_pics       = 1;
        scs->tf_params_per_type[0].max_num_future_pics = MIN(
            (1 << scs->static_config.hierarchical_levels),
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 0, 1));
        scs->tf_params_per_type[0].hme_me_level            = 1;
        scs->tf_params_per_type[0].half_pel_mode           = 1;
        scs->tf_params_per_type[0].quarter_pel_mode        = 1;
        scs->tf_params_per_type[0].eight_pel_mode          = 1;
        scs->tf_params_per_type[0].chroma_lvl              = 1;
        scs->tf_params_per_type[0].pred_error_32x32_th     = 0;
        scs->tf_params_per_type[0].enable_8x8_pred         = 1;
        scs->tf_params_per_type[0].sub_sampling_shift      = 0;
        scs->tf_params_per_type[0].avoid_2d_qpel           = 0;
        scs->tf_params_per_type[0].use_2tap                = 0;
        scs->tf_params_per_type[0].use_intra_for_noise_est = 0;
        scs->tf_params_per_type[0].use_8bit_subpel         = 1;
        scs->tf_params_per_type[0].use_pred_64x64_only_th  = 0;
        scs->tf_params_per_type[0].me_exit_th              = 0;
        scs->tf_params_per_type[0].subpel_early_exit_th    = 0;
        scs->tf_params_per_type[0].ref_frame_factor        = 1;
        scs->tf_params_per_type[0].qp_opt                  = 0;
        // BASE TF Params
        scs->tf_params_per_type[1].enabled           = 1;
        scs->tf_params_per_type[1].num_past_pics     = 1;
        scs->tf_params_per_type[1].num_future_pics   = 1;
        scs->tf_params_per_type[1].modulate_pics     = 1;
        scs->tf_params_per_type[1].max_num_past_pics = MIN(
            (1 << scs->static_config.hierarchical_levels),
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 1, 0));
        scs->tf_params_per_type[1].max_num_future_pics = MIN(
            (1 << scs->static_config.hierarchical_levels),
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 1, 1));
        scs->tf_params_per_type[1].hme_me_level            = 1;
        scs->tf_params_per_type[1].half_pel_mode           = 1;
        scs->tf_params_per_type[1].quarter_pel_mode        = 1;
        scs->tf_params_per_type[1].eight_pel_mode          = 1;
        scs->tf_params_per_type[1].chroma_lvl              = 1;
        scs->tf_params_per_type[1].pred_error_32x32_th     = 0;
        scs->tf_params_per_type[1].enable_8x8_pred         = 1;
        scs->tf_params_per_type[1].sub_sampling_shift      = 0;
        scs->tf_params_per_type[1].avoid_2d_qpel           = 0;
        scs->tf_params_per_type[1].use_2tap                = 0;
        scs->tf_params_per_type[1].use_intra_for_noise_est = 0;
        scs->tf_params_per_type[1].use_8bit_subpel         = 1;
        scs->tf_params_per_type[1].use_pred_64x64_only_th  = 0;
        scs->tf_params_per_type[1].me_exit_th              = 0;
        scs->tf_params_per_type[1].subpel_early_exit_th    = 0;
        scs->tf_params_per_type[1].ref_frame_factor        = 1;
        scs->tf_params_per_type[1].qp_opt                  = 0;
        // L1 TF Params
        scs->tf_params_per_type[2].enabled           = 1;
        scs->tf_params_per_type[2].num_past_pics     = 1;
        scs->tf_params_per_type[2].num_future_pics   = 1;
        scs->tf_params_per_type[2].modulate_pics     = 1;
        scs->tf_params_per_type[2].max_num_past_pics = MIN(
            (1 << scs->static_config.hierarchical_levels) / 2,
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 2, 0));
        scs->tf_params_per_type[2].max_num_future_pics = MIN(
            (1 << scs->static_config.hierarchical_levels) / 2,
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 2, 1));
        scs->tf_params_per_type[2].hme_me_level            = 1;
        scs->tf_params_per_type[2].half_pel_mode           = 1;
        scs->tf_params_per_type[2].quarter_pel_mode        = 1;
        scs->tf_params_per_type[2].eight_pel_mode          = 1;
        scs->tf_params_per_type[2].chroma_lvl              = 1;
        scs->tf_params_per_type[2].pred_error_32x32_th     = 0;
        scs->tf_params_per_type[2].enable_8x8_pred         = 1;
        scs->tf_params_per_type[2].sub_sampling_shift      = 0;
        scs->tf_params_per_type[2].avoid_2d_qpel           = 0;
        scs->tf_params_per_type[2].use_2tap                = 0;
        scs->tf_params_per_type[2].use_intra_for_noise_est = 0;
        scs->tf_params_per_type[2].use_8bit_subpel         = 1;
        scs->tf_params_per_type[2].use_pred_64x64_only_th  = 0;
        scs->tf_params_per_type[2].me_exit_th              = 0;
        scs->tf_params_per_type[2].subpel_early_exit_th    = 0;
        scs->tf_params_per_type[2].ref_frame_factor        = 1;
        scs->tf_params_per_type[2].qp_opt                  = 0;
        break;

    case 2:
        // I_SLICE TF Params
        scs->tf_params_per_type[0].enabled             = 1;
        scs->tf_params_per_type[0].num_future_pics     = 24;
        scs->tf_params_per_type[0].modulate_pics       = 1;
        scs->tf_params_per_type[0].max_num_future_pics = MIN(
            (1 << scs->static_config.hierarchical_levels),
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 0, 1));
        scs->tf_params_per_type[0].hme_me_level            = 1;
        scs->tf_params_per_type[0].half_pel_mode           = 1;
        scs->tf_params_per_type[0].quarter_pel_mode        = 1;
        scs->tf_params_per_type[0].eight_pel_mode          = 1;
        scs->tf_params_per_type[0].chroma_lvl              = 1;
        scs->tf_params_per_type[0].pred_error_32x32_th     = 8 * 32 * 32;
        scs->tf_params_per_type[0].enable_8x8_pred         = 1;
        scs->tf_params_per_type[0].sub_sampling_shift      = 0;
        scs->tf_params_per_type[0].avoid_2d_qpel           = 0;
        scs->tf_params_per_type[0].use_2tap                = 0;
        scs->tf_params_per_type[0].use_intra_for_noise_est = 0;
        scs->tf_params_per_type[0].use_8bit_subpel         = 1;
        scs->tf_params_per_type[0].use_pred_64x64_only_th  = 0;
        scs->tf_params_per_type[0].me_exit_th              = 0;
        scs->tf_params_per_type[0].subpel_early_exit_th    = 0;
        scs->tf_params_per_type[0].ref_frame_factor        = 1;
        scs->tf_params_per_type[0].qp_opt                  = 0;
        // BASE TF Params
        scs->tf_params_per_type[1].enabled           = 1;
        scs->tf_params_per_type[1].num_past_pics     = 1;
        scs->tf_params_per_type[1].num_future_pics   = 1;
        scs->tf_params_per_type[1].modulate_pics     = 2;
        scs->tf_params_per_type[1].max_num_past_pics = MIN(
            (1 << scs->static_config.hierarchical_levels),
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 1, 0));
        scs->tf_params_per_type[1].max_num_future_pics = MIN(
            (1 << scs->static_config.hierarchical_levels),
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 1, 1));
        scs->tf_params_per_type[1].hme_me_level            = 1;
        scs->tf_params_per_type[1].half_pel_mode           = 1;
        scs->tf_params_per_type[1].quarter_pel_mode        = 1;
        scs->tf_params_per_type[1].eight_pel_mode          = 1;
        scs->tf_params_per_type[1].chroma_lvl              = 1;
        scs->tf_params_per_type[1].pred_error_32x32_th     = 8 * 32 * 32;
        scs->tf_params_per_type[1].enable_8x8_pred         = 1;
        scs->tf_params_per_type[1].sub_sampling_shift      = 0;
        scs->tf_params_per_type[1].avoid_2d_qpel           = 0;
        scs->tf_params_per_type[1].use_2tap                = 0;
        scs->tf_params_per_type[1].use_intra_for_noise_est = 0;
        scs->tf_params_per_type[1].use_8bit_subpel         = 1;
        scs->tf_params_per_type[1].use_pred_64x64_only_th  = 0;
        scs->tf_params_per_type[1].me_exit_th              = 0;
        scs->tf_params_per_type[1].subpel_early_exit_th    = 0;
        scs->tf_params_per_type[1].ref_frame_factor        = 1;
        scs->tf_params_per_type[1].qp_opt                  = 0;
        // L1 TF Params
        scs->tf_params_per_type[2].enabled           = 1;
        scs->tf_params_per_type[2].num_past_pics     = 1;
        scs->tf_params_per_type[2].num_future_pics   = 1;
        scs->tf_params_per_type[2].modulate_pics     = 1;
        scs->tf_params_per_type[2].max_num_past_pics = MIN(
            (1 << scs->static_config.hierarchical_levels) / 2,
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 2, 0));
        scs->tf_params_per_type[2].max_num_future_pics = MIN(
            (1 << scs->static_config.hierarchical_levels) / 2,
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 2, 1));
        scs->tf_params_per_type[2].hme_me_level            = 1;
        scs->tf_params_per_type[2].half_pel_mode           = 1;
        scs->tf_params_per_type[2].quarter_pel_mode        = 1;
        scs->tf_params_per_type[2].eight_pel_mode          = 1;
        scs->tf_params_per_type[2].chroma_lvl              = 1;
        scs->tf_params_per_type[2].pred_error_32x32_th     = 8 * 32 * 32;
        scs->tf_params_per_type[2].enable_8x8_pred         = 1;
        scs->tf_params_per_type[2].sub_sampling_shift      = 0;
        scs->tf_params_per_type[2].avoid_2d_qpel           = 0;
        scs->tf_params_per_type[2].use_2tap                = 0;
        scs->tf_params_per_type[2].use_intra_for_noise_est = 0;
        scs->tf_params_per_type[2].use_8bit_subpel         = 1;
        scs->tf_params_per_type[2].use_pred_64x64_only_th  = 0;
        scs->tf_params_per_type[2].me_exit_th              = 0;
        scs->tf_params_per_type[2].subpel_early_exit_th    = 0;
        scs->tf_params_per_type[2].ref_frame_factor        = 1;
        scs->tf_params_per_type[2].qp_opt                  = 0;
        break;

    case 3:
        // I_SLICE TF Params
        scs->tf_params_per_type[0].enabled             = 1;
        scs->tf_params_per_type[0].num_future_pics     = 24;
        scs->tf_params_per_type[0].modulate_pics       = 1;
        scs->tf_params_per_type[0].max_num_future_pics = MIN(
            (1 << scs->static_config.hierarchical_levels),
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 0, 1));
        scs->tf_params_per_type[0].hme_me_level            = 1;
        scs->tf_params_per_type[0].half_pel_mode           = 1;
        scs->tf_params_per_type[0].quarter_pel_mode        = 1;
        scs->tf_params_per_type[0].eight_pel_mode          = 1;
        scs->tf_params_per_type[0].chroma_lvl              = 1;
        scs->tf_params_per_type[0].pred_error_32x32_th     = 8 * 32 * 32;
        scs->tf_params_per_type[0].enable_8x8_pred         = 1;
        scs->tf_params_per_type[0].sub_sampling_shift      = 0;
        scs->tf_params_per_type[0].avoid_2d_qpel           = 0;
        scs->tf_params_per_type[0].use_2tap                = 0;
        scs->tf_params_per_type[0].use_intra_for_noise_est = 0;
        scs->tf_params_per_type[0].use_8bit_subpel         = 1;
        scs->tf_params_per_type[0].use_pred_64x64_only_th  = 0;
        scs->tf_params_per_type[0].me_exit_th              = 0;
        scs->tf_params_per_type[0].subpel_early_exit_th    = 0;
        scs->tf_params_per_type[0].ref_frame_factor        = 1;
        scs->tf_params_per_type[0].qp_opt                  = 1;

        // BASE TF Params
        scs->tf_params_per_type[1].enabled           = 1;
        scs->tf_params_per_type[1].num_past_pics     = 1;
        scs->tf_params_per_type[1].num_future_pics   = 1;
        scs->tf_params_per_type[1].modulate_pics     = 2;
        scs->tf_params_per_type[1].max_num_past_pics = MIN(
            (1 << scs->static_config.hierarchical_levels),
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 1, 0));
        scs->tf_params_per_type[1].max_num_future_pics = MIN(
            (1 << scs->static_config.hierarchical_levels),
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 1, 1));
        scs->tf_params_per_type[1].hme_me_level            = 1;
        scs->tf_params_per_type[1].half_pel_mode           = 1;
        scs->tf_params_per_type[1].quarter_pel_mode        = 1;
        scs->tf_params_per_type[1].eight_pel_mode          = 1;
        scs->tf_params_per_type[1].chroma_lvl              = 1;
        scs->tf_params_per_type[1].pred_error_32x32_th     = 8 * 32 * 32;
        scs->tf_params_per_type[1].enable_8x8_pred         = 0;
        scs->tf_params_per_type[1].sub_sampling_shift      = 0;
        scs->tf_params_per_type[1].avoid_2d_qpel           = 0;
        scs->tf_params_per_type[1].use_2tap                = 0;
        scs->tf_params_per_type[1].use_intra_for_noise_est = 0;
        scs->tf_params_per_type[1].use_8bit_subpel         = 1;
        scs->tf_params_per_type[1].use_pred_64x64_only_th  = 0;
        scs->tf_params_per_type[1].me_exit_th              = 0;
        scs->tf_params_per_type[1].subpel_early_exit_th    = 0;
        scs->tf_params_per_type[1].ref_frame_factor        = 1;
        scs->tf_params_per_type[1].qp_opt                  = 1;

        // L1 TF Params
        scs->tf_params_per_type[2].enabled           = 1;
        scs->tf_params_per_type[2].num_past_pics     = 1;
        scs->tf_params_per_type[2].num_future_pics   = 1;
        scs->tf_params_per_type[2].modulate_pics     = 1;
        scs->tf_params_per_type[2].max_num_past_pics = MIN(
            (1 << scs->static_config.hierarchical_levels) / 2,
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 2, 0));
        scs->tf_params_per_type[2].max_num_future_pics = MIN(
            (1 << scs->static_config.hierarchical_levels) / 2,
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 2, 1));
        scs->tf_params_per_type[2].hme_me_level            = 1;
        scs->tf_params_per_type[2].half_pel_mode           = 1;
        scs->tf_params_per_type[2].quarter_pel_mode        = 1;
        scs->tf_params_per_type[2].eight_pel_mode          = 1;
        scs->tf_params_per_type[2].chroma_lvl              = 1;
        scs->tf_params_per_type[2].pred_error_32x32_th     = 8 * 32 * 32;
        scs->tf_params_per_type[2].enable_8x8_pred         = 0;
        scs->tf_params_per_type[2].sub_sampling_shift      = 0;
        scs->tf_params_per_type[2].avoid_2d_qpel           = 0;
        scs->tf_params_per_type[2].use_2tap                = 0;
        scs->tf_params_per_type[2].use_intra_for_noise_est = 0;
        scs->tf_params_per_type[2].use_8bit_subpel         = 1;
        scs->tf_params_per_type[2].use_pred_64x64_only_th  = 0;
        scs->tf_params_per_type[2].me_exit_th              = 0;
        scs->tf_params_per_type[2].subpel_early_exit_th    = 0;
        scs->tf_params_per_type[2].ref_frame_factor        = 1;
        scs->tf_params_per_type[2].qp_opt                  = 1;
        break;
    case 4:
        // I_SLICE TF Params
        scs->tf_params_per_type[0].enabled             = 1;
        scs->tf_params_per_type[0].num_future_pics     = 24;
        scs->tf_params_per_type[0].modulate_pics       = 1;
        scs->tf_params_per_type[0].max_num_future_pics = MIN(
            (1 << scs->static_config.hierarchical_levels),
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 0, 1));
        scs->tf_params_per_type[0].hme_me_level            = 2;
        scs->tf_params_per_type[0].half_pel_mode           = 1;
        scs->tf_params_per_type[0].quarter_pel_mode        = 1;
        scs->tf_params_per_type[0].eight_pel_mode          = 0;
        scs->tf_params_per_type[0].chroma_lvl              = 1;
        scs->tf_params_per_type[0].pred_error_32x32_th     = 8 * 32 * 32;
        scs->tf_params_per_type[0].enable_8x8_pred         = 0;
        scs->tf_params_per_type[0].sub_sampling_shift      = 0;
        scs->tf_params_per_type[0].avoid_2d_qpel           = 0;
        scs->tf_params_per_type[0].use_2tap                = 0;
        scs->tf_params_per_type[0].use_intra_for_noise_est = 0;
        scs->tf_params_per_type[0].use_8bit_subpel         = 1;
        scs->tf_params_per_type[0].use_pred_64x64_only_th  = 0;
        scs->tf_params_per_type[0].me_exit_th              = 0;
        scs->tf_params_per_type[0].subpel_early_exit_th    = 1;
        scs->tf_params_per_type[0].ref_frame_factor        = 1;
        scs->tf_params_per_type[0].qp_opt                  = 1;

        // BASE TF Params
        scs->tf_params_per_type[1].enabled           = 1;
        scs->tf_params_per_type[1].num_past_pics     = 1;
        scs->tf_params_per_type[1].num_future_pics   = 1;
        scs->tf_params_per_type[1].modulate_pics     = 2;
        scs->tf_params_per_type[1].max_num_past_pics = MIN(
            (1 << scs->static_config.hierarchical_levels),
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 1, 0));
        scs->tf_params_per_type[1].max_num_future_pics = MIN(
            (1 << scs->static_config.hierarchical_levels),
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 1, 1));
        scs->tf_params_per_type[1].hme_me_level            = 2;
        scs->tf_params_per_type[1].half_pel_mode           = 1;
        scs->tf_params_per_type[1].quarter_pel_mode        = 1;
        scs->tf_params_per_type[1].eight_pel_mode          = 0;
        scs->tf_params_per_type[1].chroma_lvl              = 1;
        scs->tf_params_per_type[1].pred_error_32x32_th     = 8 * 32 * 32;
        scs->tf_params_per_type[1].enable_8x8_pred         = 0;
        scs->tf_params_per_type[1].sub_sampling_shift      = 0;
        scs->tf_params_per_type[1].avoid_2d_qpel           = 0;
        scs->tf_params_per_type[1].use_2tap                = 0;
        scs->tf_params_per_type[1].use_intra_for_noise_est = 0;
        scs->tf_params_per_type[1].use_8bit_subpel         = 1;
        scs->tf_params_per_type[1].use_pred_64x64_only_th  = 0;
        scs->tf_params_per_type[1].me_exit_th              = 0;
        scs->tf_params_per_type[1].subpel_early_exit_th    = 1;
        scs->tf_params_per_type[1].ref_frame_factor        = 1;
        scs->tf_params_per_type[1].qp_opt                  = 1;

        // L1 TF Params
        scs->tf_params_per_type[2].enabled           = 1;
        scs->tf_params_per_type[2].num_past_pics     = 1;
        scs->tf_params_per_type[2].num_future_pics   = 1;
        scs->tf_params_per_type[2].modulate_pics     = 1;
        scs->tf_params_per_type[2].max_num_past_pics = MIN(
            (1 << scs->static_config.hierarchical_levels) / 2,
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 2, 0));
        scs->tf_params_per_type[2].max_num_future_pics = MIN(
            (1 << scs->static_config.hierarchical_levels) / 2,
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 2, 1));
        scs->tf_params_per_type[2].hme_me_level            = 2;
        scs->tf_params_per_type[2].half_pel_mode           = 1;
        scs->tf_params_per_type[2].quarter_pel_mode        = 1;
        scs->tf_params_per_type[2].eight_pel_mode          = 1;
        scs->tf_params_per_type[2].chroma_lvl              = 1;
        scs->tf_params_per_type[2].pred_error_32x32_th     = 8 * 32 * 32;
        scs->tf_params_per_type[2].enable_8x8_pred         = 0;
        scs->tf_params_per_type[2].sub_sampling_shift      = 0;
        scs->tf_params_per_type[2].avoid_2d_qpel           = 0;
        scs->tf_params_per_type[2].use_2tap                = 0;
        scs->tf_params_per_type[2].use_intra_for_noise_est = 0;
        scs->tf_params_per_type[2].use_8bit_subpel         = 1;
        scs->tf_params_per_type[2].use_pred_64x64_only_th  = 0;
        scs->tf_params_per_type[2].me_exit_th              = 0;
        scs->tf_params_per_type[2].subpel_early_exit_th    = 0;
        scs->tf_params_per_type[2].ref_frame_factor        = 1;
        scs->tf_params_per_type[2].qp_opt                  = 1;
        break;
    case 5:
        // I_SLICE TF Params
        scs->tf_params_per_type[0].enabled             = 1;
        scs->tf_params_per_type[0].num_future_pics     = 24;
        scs->tf_params_per_type[0].modulate_pics       = 1;
        scs->tf_params_per_type[0].max_num_future_pics = MIN(
            (1 << scs->static_config.hierarchical_levels),
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 0, 1));
        scs->tf_params_per_type[0].hme_me_level            = 2;
        scs->tf_params_per_type[0].half_pel_mode           = 2;
        scs->tf_params_per_type[0].quarter_pel_mode        = 1;
        scs->tf_params_per_type[0].eight_pel_mode          = 0;
        scs->tf_params_per_type[0].chroma_lvl              = 1;
        scs->tf_params_per_type[0].pred_error_32x32_th     = 20 * 32 * 32;
        scs->tf_params_per_type[0].enable_8x8_pred         = 0;
        scs->tf_params_per_type[0].sub_sampling_shift      = 0;
        scs->tf_params_per_type[0].avoid_2d_qpel           = 0;
        scs->tf_params_per_type[0].use_2tap                = 1;
        scs->tf_params_per_type[0].use_intra_for_noise_est = 0;
        scs->tf_params_per_type[0].use_8bit_subpel         = 1;
        scs->tf_params_per_type[0].use_pred_64x64_only_th  = 0;
        scs->tf_params_per_type[0].me_exit_th              = 0;
        scs->tf_params_per_type[0].subpel_early_exit_th    = 1;
        scs->tf_params_per_type[0].ref_frame_factor        = 1;
        scs->tf_params_per_type[0].qp_opt                  = 1;

        // BASE TF Params
        scs->tf_params_per_type[1].enabled           = 1;
        scs->tf_params_per_type[1].num_past_pics     = 1;
        scs->tf_params_per_type[1].num_future_pics   = 1;
        scs->tf_params_per_type[1].modulate_pics     = 3;
        scs->tf_params_per_type[1].max_num_past_pics = MIN(
            (1 << scs->static_config.hierarchical_levels),
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 1, 0));
        scs->tf_params_per_type[1].max_num_future_pics = MIN(
            (1 << scs->static_config.hierarchical_levels),
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 1, 1));
        scs->tf_params_per_type[1].hme_me_level            = 2;
        scs->tf_params_per_type[1].half_pel_mode           = 2;
        scs->tf_params_per_type[1].quarter_pel_mode        = 1;
        scs->tf_params_per_type[1].eight_pel_mode          = 0;
        scs->tf_params_per_type[1].chroma_lvl              = 1;
        scs->tf_params_per_type[1].pred_error_32x32_th     = 20 * 32 * 32;
        scs->tf_params_per_type[1].enable_8x8_pred         = 0;
        scs->tf_params_per_type[1].sub_sampling_shift      = 0;
        scs->tf_params_per_type[1].avoid_2d_qpel           = 0;
        scs->tf_params_per_type[1].use_2tap                = 1;
        scs->tf_params_per_type[1].use_intra_for_noise_est = 0;
        scs->tf_params_per_type[1].use_8bit_subpel         = 1;
        scs->tf_params_per_type[1].use_pred_64x64_only_th  = 0;
        scs->tf_params_per_type[1].me_exit_th              = 0;
        scs->tf_params_per_type[1].subpel_early_exit_th    = 1;
        scs->tf_params_per_type[1].ref_frame_factor        = 1;
        scs->tf_params_per_type[1].qp_opt                  = 1;

        // L1 TF Params
        scs->tf_params_per_type[2].enabled           = 1;
        scs->tf_params_per_type[2].num_past_pics     = 1;
        scs->tf_params_per_type[2].num_future_pics   = 1;
        scs->tf_params_per_type[2].modulate_pics     = 2;
        scs->tf_params_per_type[2].max_num_past_pics = MIN(
            (1 << scs->static_config.hierarchical_levels) / 2,
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 2, 0));
        scs->tf_params_per_type[2].max_num_future_pics = MIN(
            (1 << scs->static_config.hierarchical_levels) / 2,
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 2, 1));
        scs->tf_params_per_type[2].hme_me_level            = 2;
        scs->tf_params_per_type[2].half_pel_mode           = 2;
        scs->tf_params_per_type[2].quarter_pel_mode        = 1;
        scs->tf_params_per_type[2].eight_pel_mode          = 0;
        scs->tf_params_per_type[2].chroma_lvl              = 1;
        scs->tf_params_per_type[2].pred_error_32x32_th     = 20 * 32 * 32;
        scs->tf_params_per_type[2].enable_8x8_pred         = 0;
        scs->tf_params_per_type[2].sub_sampling_shift      = 0;
        scs->tf_params_per_type[2].avoid_2d_qpel           = 0;
        scs->tf_params_per_type[2].use_2tap                = 1;
        scs->tf_params_per_type[2].use_intra_for_noise_est = 0;
        scs->tf_params_per_type[2].use_8bit_subpel         = 1;
        scs->tf_params_per_type[2].use_pred_64x64_only_th  = 0;
        scs->tf_params_per_type[2].me_exit_th              = 0;
        scs->tf_params_per_type[2].subpel_early_exit_th    = 1;
        scs->tf_params_per_type[2].ref_frame_factor        = 1;
        scs->tf_params_per_type[2].qp_opt                  = 1;
        break;
    case 6:
        // I_SLICE TF Params
        scs->tf_params_per_type[0].enabled             = 1;
        scs->tf_params_per_type[0].num_future_pics     = (scs->static_config.hierarchical_levels < 5) ? 8 : 16;
        scs->tf_params_per_type[0].modulate_pics       = 0;
        scs->tf_params_per_type[0].max_num_future_pics = MIN(
            (1 << scs->static_config.hierarchical_levels),
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 0, 1));
        scs->tf_params_per_type[0].hme_me_level            = 2;
        scs->tf_params_per_type[0].half_pel_mode           = 2;
        scs->tf_params_per_type[0].quarter_pel_mode        = 1;
        scs->tf_params_per_type[0].eight_pel_mode          = 0;
        scs->tf_params_per_type[0].chroma_lvl              = 0;
        scs->tf_params_per_type[0].pred_error_32x32_th     = (uint64_t)~0;
        scs->tf_params_per_type[0].enable_8x8_pred         = 0;
        scs->tf_params_per_type[0].sub_sampling_shift      = 1;
        scs->tf_params_per_type[0].avoid_2d_qpel           = 1;
        scs->tf_params_per_type[0].use_2tap                = 1;
        scs->tf_params_per_type[0].use_intra_for_noise_est = 1;
        scs->tf_params_per_type[0].use_8bit_subpel         = 1;
        scs->tf_params_per_type[0].use_pred_64x64_only_th  = 0;
        scs->tf_params_per_type[0].me_exit_th              = 0;
        scs->tf_params_per_type[0].subpel_early_exit_th    = 1;
        scs->tf_params_per_type[0].ref_frame_factor        = 1;
        scs->tf_params_per_type[0].qp_opt                  = 1;
        // BASE TF Params
        scs->tf_params_per_type[1].enabled           = 1;
        scs->tf_params_per_type[1].num_past_pics     = 1;
        scs->tf_params_per_type[1].num_future_pics   = 1;
        scs->tf_params_per_type[1].modulate_pics     = 3;
        scs->tf_params_per_type[1].max_num_past_pics = MIN(
            (1 << scs->static_config.hierarchical_levels),
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 1, 0));
        scs->tf_params_per_type[1].max_num_future_pics = MIN(
            (1 << scs->static_config.hierarchical_levels),
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 1, 1));
        scs->tf_params_per_type[1].hme_me_level            = 2;
        scs->tf_params_per_type[1].half_pel_mode           = 2;
        scs->tf_params_per_type[1].quarter_pel_mode        = 1;
        scs->tf_params_per_type[1].eight_pel_mode          = 0;
        scs->tf_params_per_type[1].chroma_lvl              = 1;
        scs->tf_params_per_type[1].pred_error_32x32_th     = 20 * 32 * 32;
        scs->tf_params_per_type[1].enable_8x8_pred         = 0;
        scs->tf_params_per_type[1].sub_sampling_shift      = 0;
        scs->tf_params_per_type[1].avoid_2d_qpel           = 0;
        scs->tf_params_per_type[1].use_2tap                = 1;
        scs->tf_params_per_type[1].use_intra_for_noise_est = 1;
        scs->tf_params_per_type[1].use_8bit_subpel         = 1;
        scs->tf_params_per_type[1].use_pred_64x64_only_th  = 0;
        scs->tf_params_per_type[1].me_exit_th              = 0;
        scs->tf_params_per_type[1].subpel_early_exit_th    = 1;
        scs->tf_params_per_type[1].ref_frame_factor        = 1;
        scs->tf_params_per_type[1].qp_opt                  = 1;
        // L1 TF Params
        scs->tf_params_per_type[2].enabled = 0;
        break;
    case 7:
        // I_SLICE TF Params
        scs->tf_params_per_type[0].enabled             = 1;
        scs->tf_params_per_type[0].num_future_pics     = (scs->static_config.hierarchical_levels < 5) ? 8 : 16;
        scs->tf_params_per_type[0].modulate_pics       = 0;
        scs->tf_params_per_type[0].max_num_future_pics = MIN(
            (1 << scs->static_config.hierarchical_levels),
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 0, 1));
        scs->tf_params_per_type[0].hme_me_level            = 2;
        scs->tf_params_per_type[0].half_pel_mode           = 2;
        scs->tf_params_per_type[0].quarter_pel_mode        = 1;
        scs->tf_params_per_type[0].eight_pel_mode          = 0;
        scs->tf_params_per_type[0].chroma_lvl              = 0;
        scs->tf_params_per_type[0].pred_error_32x32_th     = (uint64_t)~0;
        scs->tf_params_per_type[0].enable_8x8_pred         = 0;
        scs->tf_params_per_type[0].sub_sampling_shift      = 1;
        scs->tf_params_per_type[0].avoid_2d_qpel           = 1;
        scs->tf_params_per_type[0].use_2tap                = 1;
        scs->tf_params_per_type[0].use_intra_for_noise_est = 1;
        scs->tf_params_per_type[0].use_8bit_subpel         = 1;
        scs->tf_params_per_type[0].use_pred_64x64_only_th  = 35;
        scs->tf_params_per_type[0].me_exit_th              = 16 * 16;
        scs->tf_params_per_type[0].subpel_early_exit_th    = 1;
        scs->tf_params_per_type[0].ref_frame_factor        = 1;
        scs->tf_params_per_type[0].qp_opt                  = 1;
        // BASE TF Params
        scs->tf_params_per_type[1].enabled           = 1;
        scs->tf_params_per_type[1].num_past_pics     = 1;
        scs->tf_params_per_type[1].num_future_pics   = 1;
        scs->tf_params_per_type[1].modulate_pics     = 3;
        scs->tf_params_per_type[1].max_num_past_pics = MIN(
            (1 << scs->static_config.hierarchical_levels),
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 1, 0));
        scs->tf_params_per_type[1].max_num_future_pics = MIN(
            (1 << scs->static_config.hierarchical_levels),
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 1, 1));
        scs->tf_params_per_type[1].hme_me_level            = 2;
        scs->tf_params_per_type[1].half_pel_mode           = 2;
        scs->tf_params_per_type[1].quarter_pel_mode        = 1;
        scs->tf_params_per_type[1].eight_pel_mode          = 0;
        scs->tf_params_per_type[1].chroma_lvl              = 1;
        scs->tf_params_per_type[1].pred_error_32x32_th     = 20 * 32 * 32;
        scs->tf_params_per_type[1].enable_8x8_pred         = 0;
        scs->tf_params_per_type[1].sub_sampling_shift      = 0;
        scs->tf_params_per_type[1].avoid_2d_qpel           = 0;
        scs->tf_params_per_type[1].use_2tap                = 1;
        scs->tf_params_per_type[1].use_intra_for_noise_est = 1;
        scs->tf_params_per_type[1].use_8bit_subpel         = 1;
        scs->tf_params_per_type[1].use_pred_64x64_only_th  = 35;
        scs->tf_params_per_type[1].me_exit_th              = 16 * 16;
        scs->tf_params_per_type[1].subpel_early_exit_th    = 1;
        scs->tf_params_per_type[1].ref_frame_factor        = 1;
        scs->tf_params_per_type[1].qp_opt                  = 1;
        // L1 TF Params
        scs->tf_params_per_type[2].enabled = 0;
        break;
    case 8:
        // I_SLICE TF Params
        scs->tf_params_per_type[0].enabled             = 1;
        scs->tf_params_per_type[0].num_future_pics     = 8;
        scs->tf_params_per_type[0].modulate_pics       = 0;
        scs->tf_params_per_type[0].max_num_future_pics = MIN(
            (1 << scs->static_config.hierarchical_levels),
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 0, 1));
        scs->tf_params_per_type[0].hme_me_level            = 2;
        scs->tf_params_per_type[0].half_pel_mode           = 2;
        scs->tf_params_per_type[0].quarter_pel_mode        = 1;
        scs->tf_params_per_type[0].eight_pel_mode          = 0;
        scs->tf_params_per_type[0].chroma_lvl              = 0;
        scs->tf_params_per_type[0].pred_error_32x32_th     = (uint64_t)~0;
        scs->tf_params_per_type[0].enable_8x8_pred         = 0;
        scs->tf_params_per_type[0].sub_sampling_shift      = 1;
        scs->tf_params_per_type[0].avoid_2d_qpel           = 1;
        scs->tf_params_per_type[0].use_2tap                = 1;
        scs->tf_params_per_type[0].use_intra_for_noise_est = 1;
        scs->tf_params_per_type[0].use_8bit_subpel         = 1;
        scs->tf_params_per_type[0].use_pred_64x64_only_th  = 35;
        scs->tf_params_per_type[0].me_exit_th              = 16 * 16;
        scs->tf_params_per_type[0].subpel_early_exit_th    = 4;
        scs->tf_params_per_type[0].ref_frame_factor        = 2;
        scs->tf_params_per_type[0].qp_opt                  = 1;
        // BASE TF Params
        scs->tf_params_per_type[1].enabled           = 1;
        scs->tf_params_per_type[1].num_past_pics     = 1;
        scs->tf_params_per_type[1].num_future_pics   = 1;
        scs->tf_params_per_type[1].modulate_pics     = 4;
        scs->tf_params_per_type[1].max_num_past_pics = MIN(
            (1 << scs->static_config.hierarchical_levels),
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 1, 0));
        scs->tf_params_per_type[1].max_num_future_pics = MIN(
            (1 << scs->static_config.hierarchical_levels),
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 1, 1));
        scs->tf_params_per_type[1].hme_me_level            = 2;
        scs->tf_params_per_type[1].half_pel_mode           = 2;
        scs->tf_params_per_type[1].quarter_pel_mode        = 1;
        scs->tf_params_per_type[1].eight_pel_mode          = 0;
        scs->tf_params_per_type[1].chroma_lvl              = 0;
        scs->tf_params_per_type[1].pred_error_32x32_th     = (uint64_t)~0;
        scs->tf_params_per_type[1].enable_8x8_pred         = 0;
        scs->tf_params_per_type[1].sub_sampling_shift      = 1;
        scs->tf_params_per_type[1].avoid_2d_qpel           = 1;
        scs->tf_params_per_type[1].use_2tap                = 1;
        scs->tf_params_per_type[1].use_intra_for_noise_est = 1;
        scs->tf_params_per_type[1].use_8bit_subpel         = 1;
        scs->tf_params_per_type[1].use_pred_64x64_only_th  = 35;
        scs->tf_params_per_type[1].me_exit_th              = 16 * 16;
        scs->tf_params_per_type[1].subpel_early_exit_th    = 4;
        scs->tf_params_per_type[1].ref_frame_factor        = 1;
        scs->tf_params_per_type[1].qp_opt                  = 1;
        // L1 TF Params
        scs->tf_params_per_type[2].enabled = 0;
        break;
    case 9:
        // I_SLICE TF Params
        scs->tf_params_per_type[0].enabled             = 1;
        scs->tf_params_per_type[0].num_future_pics     = 8;
        scs->tf_params_per_type[0].modulate_pics       = 0;
        scs->tf_params_per_type[0].max_num_future_pics = MIN(
            (1 << scs->static_config.hierarchical_levels),
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 0, 1));
        scs->tf_params_per_type[0].hme_me_level            = 3;
        scs->tf_params_per_type[0].half_pel_mode           = 2;
        scs->tf_params_per_type[0].quarter_pel_mode        = 1;
        scs->tf_params_per_type[0].eight_pel_mode          = 0;
        scs->tf_params_per_type[0].chroma_lvl              = 0;
        scs->tf_params_per_type[0].pred_error_32x32_th     = (uint64_t)~0;
        scs->tf_params_per_type[0].enable_8x8_pred         = 0;
        scs->tf_params_per_type[0].sub_sampling_shift      = 1;
        scs->tf_params_per_type[0].avoid_2d_qpel           = 1;
        scs->tf_params_per_type[0].use_2tap                = 1;
        scs->tf_params_per_type[0].use_intra_for_noise_est = 1;
        scs->tf_params_per_type[0].use_8bit_subpel         = 1;
        scs->tf_params_per_type[0].use_pred_64x64_only_th  = 35;
        scs->tf_params_per_type[0].me_exit_th              = 16 * 16;
        scs->tf_params_per_type[0].subpel_early_exit_th    = 4;
        scs->tf_params_per_type[0].ref_frame_factor        = 2;
        scs->tf_params_per_type[0].qp_opt                  = 1;
        // BASE TF Params
        scs->tf_params_per_type[1].enabled           = 1;
        scs->tf_params_per_type[1].num_past_pics     = 1;
        scs->tf_params_per_type[1].num_future_pics   = 1;
        scs->tf_params_per_type[1].modulate_pics     = 4;
        scs->tf_params_per_type[1].max_num_past_pics = MIN(
            (1 << scs->static_config.hierarchical_levels),
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 1, 0));
        scs->tf_params_per_type[1].max_num_future_pics = MIN(
            (1 << scs->static_config.hierarchical_levels),
            svt_aom_tf_max_ref_per_struct(scs->static_config.hierarchical_levels, 1, 1));
        scs->tf_params_per_type[1].hme_me_level            = 3;
        scs->tf_params_per_type[1].half_pel_mode           = 2;
        scs->tf_params_per_type[1].quarter_pel_mode        = 1;
        scs->tf_params_per_type[1].eight_pel_mode          = 0;
        scs->tf_params_per_type[1].chroma_lvl              = 0;
        scs->tf_params_per_type[1].pred_error_32x32_th     = (uint64_t)~0;
        scs->tf_params_per_type[1].enable_8x8_pred         = 0;
        scs->tf_params_per_type[1].sub_sampling_shift      = 1;
        scs->tf_params_per_type[1].avoid_2d_qpel           = 1;
        scs->tf_params_per_type[1].use_2tap                = 1;
        scs->tf_params_per_type[1].use_intra_for_noise_est = 1;
        scs->tf_params_per_type[1].use_8bit_subpel         = 1;
        scs->tf_params_per_type[1].use_pred_64x64_only_th  = 35;
        scs->tf_params_per_type[1].me_exit_th              = 16 * 16;
        scs->tf_params_per_type[1].subpel_early_exit_th    = 4;
        scs->tf_params_per_type[1].ref_frame_factor        = 1;
        scs->tf_params_per_type[1].qp_opt                  = 1;
        // L1 TF Params
        scs->tf_params_per_type[2].enabled = 0;
        break;
    default:
        assert(0);
        break;
    }
    scs->tf_params_per_type[0].use_zz_based_filter = 0;
    scs->tf_params_per_type[1].use_zz_based_filter = 0;
    scs->tf_params_per_type[2].use_zz_based_filter = 0;
}

/*
 * Derive tune Params; 0: use objective mode params, 1: use subjective mode params
 */
static void derive_vq_params(SequenceControlSet* scs) {
    VqCtrls* vq_ctrl = &scs->vq_ctrls;

    if (scs->static_config.tune == TUNE_VQ) {
        // Sharpness
        vq_ctrl->sharpness_ctrls.scene_transition = 1;
        vq_ctrl->sharpness_ctrls.tf               = 1;
        vq_ctrl->sharpness_ctrls.unipred_bias     = 1;
        vq_ctrl->sharpness_ctrls.ifs              = 1;
        vq_ctrl->sharpness_ctrls.cdef             = 1;
        vq_ctrl->sharpness_ctrls.restoration      = 1;
        vq_ctrl->sharpness_ctrls.rdoq             = 1;
    } else {
        // Sharpness
        vq_ctrl->sharpness_ctrls.scene_transition = 1;
        vq_ctrl->sharpness_ctrls.tf               = 0;
        vq_ctrl->sharpness_ctrls.unipred_bias     = 0;
        vq_ctrl->sharpness_ctrls.ifs              = 0;
        vq_ctrl->sharpness_ctrls.cdef             = 0;
        vq_ctrl->sharpness_ctrls.restoration      = 0;
        vq_ctrl->sharpness_ctrls.rdoq             = 0;
    }
    // Do not use scene_transition if LD or 1st pass or middle pass
    if (scs->static_config.pred_structure != RANDOM_ACCESS || scs->static_config.pass == ENC_FIRST_PASS) {
        vq_ctrl->sharpness_ctrls.scene_transition = 0;
    }
}

/*
 * Derive TF Params
 */
static void derive_tf_params(SequenceControlSet* scs) {
    const uint32_t hierarchical_levels = scs->static_config.hierarchical_levels;
    // Do not perform TF if LD or 1 Layer or 1st pass
    const bool    do_tf    = scs->static_config.enable_tf && hierarchical_levels >= 1 && !scs->static_config.lossless;
    const EncMode enc_mode = scs->static_config.enc_mode;
    uint8_t       tf_level = 0;
    if (scs->static_config.pred_structure == LOW_DELAY) {
        // For LD, only use TF for non-SC content in RTC mode; the TF is tuned for RTC content
        if (!do_tf || scs->static_config.screen_content_mode == 1 || !scs->static_config.rtc) {
            tf_level = 0;
        } else if ((!scs->use_flat_ipp && enc_mode <= ENC_M7) || (scs->use_flat_ipp && enc_mode <= ENC_M6)) {
            tf_level = 1;
        } else if ((!scs->use_flat_ipp && enc_mode <= ENC_M8) || (scs->use_flat_ipp && enc_mode <= ENC_M7)) {
            tf_level = 2;
        } else {
            tf_level = 0;
        }
        tf_ld_controls(scs, tf_level);
        return;
    }
    if (do_tf == 0) {
        tf_level = 0;
    } else if (enc_mode <= ENC_M1) {
        tf_level = 1;
    } else if (enc_mode <= ENC_M2) {
        tf_level = 2;
    } else if (enc_mode <= ENC_M7) {
        tf_level = 5;
    } else {
        tf_level = 9;
    }
    tf_controls(scs, tf_level);
}

/*
 * Set the MRP control
 */
static void set_mrp_ctrl(SequenceControlSet* scs, uint8_t mrp_level) {
    MrpCtrls* mrp_ctrl = &scs->mrp_ctrls;

    switch (mrp_level) {
    case 0:
        mrp_ctrl->referencing_scheme          = 0;
        mrp_ctrl->sc_base_ref_list0_count     = 1;
        mrp_ctrl->sc_base_ref_list1_count     = 0;
        mrp_ctrl->sc_non_base_ref_list0_count = 1;
        mrp_ctrl->sc_non_base_ref_list1_count = 0;
        mrp_ctrl->base_ref_list0_count        = 1;
        mrp_ctrl->base_ref_list1_count        = 0;
        mrp_ctrl->non_base_ref_list0_count    = 1;
        mrp_ctrl->non_base_ref_list1_count    = 0;
        mrp_ctrl->more_5L_refs                = 0;
        mrp_ctrl->safe_limit_nref             = 0;
        mrp_ctrl->safe_limit_zz_th            = 0;
        mrp_ctrl->only_l_bwd                  = 0;
        mrp_ctrl->pme_ref0_only               = 0;
        mrp_ctrl->use_best_references         = 0;
        break;

    case 1:
        mrp_ctrl->referencing_scheme          = 1;
        mrp_ctrl->sc_base_ref_list0_count     = 2;
        mrp_ctrl->sc_base_ref_list1_count     = 2;
        mrp_ctrl->sc_non_base_ref_list0_count = 2;
        mrp_ctrl->sc_non_base_ref_list1_count = 2;
        mrp_ctrl->base_ref_list0_count        = 4;
        mrp_ctrl->base_ref_list1_count        = 3;
        mrp_ctrl->non_base_ref_list0_count    = 4;
        mrp_ctrl->non_base_ref_list1_count    = 3;
        mrp_ctrl->more_5L_refs                = 1;
        mrp_ctrl->safe_limit_nref             = 0;
        mrp_ctrl->safe_limit_zz_th            = 0;
        mrp_ctrl->only_l_bwd                  = 0;
        mrp_ctrl->pme_ref0_only               = 0;
        mrp_ctrl->use_best_references         = 0;
        break;

    case 2:
        mrp_ctrl->referencing_scheme          = 1;
        mrp_ctrl->sc_base_ref_list0_count     = 2;
        mrp_ctrl->sc_base_ref_list1_count     = 2;
        mrp_ctrl->sc_non_base_ref_list0_count = 2;
        mrp_ctrl->sc_non_base_ref_list1_count = 2;
        mrp_ctrl->base_ref_list0_count        = 4;
        mrp_ctrl->base_ref_list1_count        = 3;
        mrp_ctrl->non_base_ref_list0_count    = 4;
        mrp_ctrl->non_base_ref_list1_count    = 3;
        mrp_ctrl->more_5L_refs                = 1;
        mrp_ctrl->safe_limit_nref             = 0;
        mrp_ctrl->safe_limit_zz_th            = 0;
        mrp_ctrl->only_l_bwd                  = 1;
        mrp_ctrl->pme_ref0_only               = 0;
        mrp_ctrl->use_best_references         = 0;
        break;
    case 3:
        mrp_ctrl->referencing_scheme          = 1;
        mrp_ctrl->sc_base_ref_list0_count     = 2;
        mrp_ctrl->sc_base_ref_list1_count     = 2;
        mrp_ctrl->sc_non_base_ref_list0_count = 2;
        mrp_ctrl->sc_non_base_ref_list1_count = 2;
        mrp_ctrl->base_ref_list0_count        = 4;
        mrp_ctrl->base_ref_list1_count        = 3;
        mrp_ctrl->non_base_ref_list0_count    = 4;
        mrp_ctrl->non_base_ref_list1_count    = 3;
        mrp_ctrl->more_5L_refs                = 1;
        mrp_ctrl->safe_limit_nref             = 0;
        mrp_ctrl->safe_limit_zz_th            = 0;
        mrp_ctrl->only_l_bwd                  = 1;
        mrp_ctrl->pme_ref0_only               = 0;
        mrp_ctrl->use_best_references         = 2;
        break;
    case 4:
        mrp_ctrl->referencing_scheme          = 1;
        mrp_ctrl->sc_base_ref_list0_count     = 2;
        mrp_ctrl->sc_base_ref_list1_count     = 2;
        mrp_ctrl->sc_non_base_ref_list0_count = 2;
        mrp_ctrl->sc_non_base_ref_list1_count = 2;
        mrp_ctrl->base_ref_list0_count        = 4;
        mrp_ctrl->base_ref_list1_count        = 3;
        mrp_ctrl->non_base_ref_list0_count    = 4;
        mrp_ctrl->non_base_ref_list1_count    = 3;
        mrp_ctrl->more_5L_refs                = 1;
        mrp_ctrl->safe_limit_nref             = 1;
        mrp_ctrl->safe_limit_zz_th            = 60000;
        mrp_ctrl->only_l_bwd                  = 1;
        mrp_ctrl->pme_ref0_only               = 1;
        mrp_ctrl->use_best_references         = 3;
        break;
    case 5:
        mrp_ctrl->referencing_scheme          = 0;
        mrp_ctrl->sc_base_ref_list0_count     = 2;
        mrp_ctrl->sc_base_ref_list1_count     = 2;
        mrp_ctrl->sc_non_base_ref_list0_count = 2;
        mrp_ctrl->sc_non_base_ref_list1_count = 2;
        mrp_ctrl->base_ref_list0_count        = 4;
        mrp_ctrl->base_ref_list1_count        = 3;
        mrp_ctrl->non_base_ref_list0_count    = 4;
        mrp_ctrl->non_base_ref_list1_count    = 3;
        mrp_ctrl->more_5L_refs                = 0;
        mrp_ctrl->safe_limit_nref             = 2;
        mrp_ctrl->safe_limit_zz_th            = 60000;
        mrp_ctrl->only_l_bwd                  = 1;
        mrp_ctrl->pme_ref0_only               = 1;
        mrp_ctrl->use_best_references         = 3;
        break;
    case 6:
        mrp_ctrl->referencing_scheme          = 0;
        mrp_ctrl->sc_base_ref_list0_count     = 2;
        mrp_ctrl->sc_base_ref_list1_count     = 2;
        mrp_ctrl->sc_non_base_ref_list0_count = 1;
        mrp_ctrl->sc_non_base_ref_list1_count = 1;
        mrp_ctrl->base_ref_list0_count        = 3;
        mrp_ctrl->base_ref_list1_count        = 2;
        mrp_ctrl->non_base_ref_list0_count    = 3;
        mrp_ctrl->non_base_ref_list1_count    = 2;
        mrp_ctrl->more_5L_refs                = 0;
        mrp_ctrl->safe_limit_nref             = 2;
        mrp_ctrl->safe_limit_zz_th            = 60000;
        mrp_ctrl->only_l_bwd                  = 1;
        mrp_ctrl->pme_ref0_only               = 1;
        mrp_ctrl->use_best_references         = 3;
        break;
    case 7:
        mrp_ctrl->referencing_scheme          = 0;
        mrp_ctrl->sc_base_ref_list0_count     = 2;
        mrp_ctrl->sc_base_ref_list1_count     = 2;
        mrp_ctrl->sc_non_base_ref_list0_count = 1;
        mrp_ctrl->sc_non_base_ref_list1_count = 1;
        mrp_ctrl->base_ref_list0_count        = 3;
        mrp_ctrl->base_ref_list1_count        = 2;
        mrp_ctrl->non_base_ref_list0_count    = 2;
        mrp_ctrl->non_base_ref_list1_count    = 2;
        mrp_ctrl->more_5L_refs                = 0;
        mrp_ctrl->safe_limit_nref             = 2;
        mrp_ctrl->safe_limit_zz_th            = 60000;
        mrp_ctrl->only_l_bwd                  = 1;
        mrp_ctrl->pme_ref0_only               = 1;
        mrp_ctrl->use_best_references         = 3;
        break;
    case 8:
        mrp_ctrl->referencing_scheme          = 0;
        mrp_ctrl->sc_base_ref_list0_count     = 2;
        mrp_ctrl->sc_base_ref_list1_count     = 2;
        mrp_ctrl->sc_non_base_ref_list0_count = 1;
        mrp_ctrl->sc_non_base_ref_list1_count = 1;
        mrp_ctrl->base_ref_list0_count        = 2;
        mrp_ctrl->base_ref_list1_count        = 2;
        mrp_ctrl->non_base_ref_list0_count    = 2;
        mrp_ctrl->non_base_ref_list1_count    = 2;
        mrp_ctrl->more_5L_refs                = 0;
        mrp_ctrl->safe_limit_nref             = 2;
        mrp_ctrl->safe_limit_zz_th            = 60000;
        mrp_ctrl->only_l_bwd                  = 1;
        mrp_ctrl->pme_ref0_only               = 1;
        mrp_ctrl->use_best_references         = 3;
        break;
    case 9:
        mrp_ctrl->referencing_scheme          = 0;
        mrp_ctrl->sc_base_ref_list0_count     = 2;
        mrp_ctrl->sc_base_ref_list1_count     = 2;
        mrp_ctrl->sc_non_base_ref_list0_count = 1;
        mrp_ctrl->sc_non_base_ref_list1_count = 1;
        mrp_ctrl->base_ref_list0_count        = 3;
        mrp_ctrl->base_ref_list1_count        = 2;
        mrp_ctrl->non_base_ref_list0_count    = 1;
        mrp_ctrl->non_base_ref_list1_count    = 1;
        mrp_ctrl->more_5L_refs                = 0;
        mrp_ctrl->safe_limit_nref             = 2;
        mrp_ctrl->safe_limit_zz_th            = 60000;
        mrp_ctrl->only_l_bwd                  = 1;
        mrp_ctrl->pme_ref0_only               = 1;
        mrp_ctrl->use_best_references         = 3;
        break;
    case 10:
        mrp_ctrl->referencing_scheme          = 0;
        mrp_ctrl->sc_base_ref_list0_count     = 2;
        mrp_ctrl->sc_base_ref_list1_count     = 2;
        mrp_ctrl->sc_non_base_ref_list0_count = 1;
        mrp_ctrl->sc_non_base_ref_list1_count = 1;
        mrp_ctrl->base_ref_list0_count        = 2;
        mrp_ctrl->base_ref_list1_count        = 2;
        mrp_ctrl->non_base_ref_list0_count    = 1;
        mrp_ctrl->non_base_ref_list1_count    = 1;
        mrp_ctrl->more_5L_refs                = 0;
        mrp_ctrl->safe_limit_nref             = 2;
        mrp_ctrl->safe_limit_zz_th            = 60000;
        mrp_ctrl->only_l_bwd                  = 1;
        mrp_ctrl->pme_ref0_only               = 1;
        mrp_ctrl->use_best_references         = 3;
        break;
    case 11:
        mrp_ctrl->referencing_scheme          = 0;
        mrp_ctrl->sc_base_ref_list0_count     = 1;
        mrp_ctrl->sc_base_ref_list1_count     = 1;
        mrp_ctrl->sc_non_base_ref_list0_count = 1;
        mrp_ctrl->sc_non_base_ref_list1_count = 1;
        mrp_ctrl->base_ref_list0_count        = 1;
        mrp_ctrl->base_ref_list1_count        = 1;
        mrp_ctrl->non_base_ref_list0_count    = 1;
        mrp_ctrl->non_base_ref_list1_count    = 1;
        mrp_ctrl->more_5L_refs                = 0;
        mrp_ctrl->safe_limit_nref             = 0;
        mrp_ctrl->safe_limit_zz_th            = 0;
        mrp_ctrl->only_l_bwd                  = 0;
        mrp_ctrl->pme_ref0_only               = 0;
        mrp_ctrl->use_best_references         = 0;
        break;
    default:
        assert(0);
        break;
    }
    // For low delay mode, list1 references are not used
    if (scs->static_config.pred_structure == LOW_DELAY && scs->static_config.rate_control_mode == SVT_AV1_RC_MODE_CBR) {
        mrp_ctrl->sc_base_ref_list1_count     = 0;
        mrp_ctrl->sc_non_base_ref_list1_count = 0;
        mrp_ctrl->base_ref_list1_count        = 0;
        mrp_ctrl->non_base_ref_list1_count    = 0;
        if (scs->use_flat_ipp) {
            mrp_ctrl->referencing_scheme  = 0;
            mrp_ctrl->more_5L_refs        = 0;
            mrp_ctrl->safe_limit_nref     = 0;
            mrp_ctrl->only_l_bwd          = 0;
            mrp_ctrl->pme_ref0_only       = 0;
            mrp_ctrl->use_best_references = 0;
        }
    }
    if (scs->static_config.pred_structure == LOW_DELAY) {
        if (scs->use_flat_ipp) {
            const uint8_t max_sc_refs = MAX(
                MAX(scs->mrp_ctrls.sc_non_base_ref_list0_count, scs->mrp_ctrls.sc_non_base_ref_list1_count),
                MAX(scs->mrp_ctrls.sc_base_ref_list0_count, scs->mrp_ctrls.sc_base_ref_list1_count));
            const uint8_t max_nsc_refs = MAX(
                MAX(scs->mrp_ctrls.base_ref_list0_count, scs->mrp_ctrls.base_ref_list1_count),
                MAX(scs->mrp_ctrls.non_base_ref_list0_count, scs->mrp_ctrls.non_base_ref_list1_count));
            mrp_ctrl->flat_max_refs = scs->static_config.screen_content_mode == 2 ? MAX(max_sc_refs, max_nsc_refs)
                : scs->static_config.screen_content_mode == 1                     ? max_sc_refs
                                                                                  : max_nsc_refs;
        }
        // If content type (SC/NSC) is known, can allocate refs based on settings, else consider worst-case
        if (scs->static_config.screen_content_mode == 1) {
            mrp_ctrl->ld_reduce_ref_buffs = (mrp_ctrl->sc_base_ref_list0_count <= 1 &&
                                             mrp_ctrl->sc_base_ref_list1_count <= 1 &&
                                             mrp_ctrl->sc_non_base_ref_list0_count <= 1 &&
                                             mrp_ctrl->sc_non_base_ref_list1_count <= 1)
                ? 2
                : (mrp_ctrl->sc_base_ref_list0_count <= 2 && mrp_ctrl->sc_base_ref_list1_count <= 2 &&
                   mrp_ctrl->sc_non_base_ref_list0_count <= 2 && mrp_ctrl->sc_non_base_ref_list1_count <= 2)
                ? 1
                : 0;
        } else if (scs->static_config.screen_content_mode == 0) {
            mrp_ctrl->ld_reduce_ref_buffs = (mrp_ctrl->base_ref_list0_count <= 1 &&
                                             mrp_ctrl->base_ref_list1_count <= 1 &&
                                             mrp_ctrl->non_base_ref_list0_count <= 1 &&
                                             mrp_ctrl->non_base_ref_list1_count <= 1)
                ? 2
                : (mrp_ctrl->base_ref_list0_count <= 2 && mrp_ctrl->base_ref_list1_count <= 2 &&
                   mrp_ctrl->non_base_ref_list0_count <= 2 && mrp_ctrl->non_base_ref_list1_count <= 2)
                ? 1
                : 0;
        } else {
            mrp_ctrl->ld_reduce_ref_buffs = (mrp_ctrl->sc_base_ref_list0_count <= 1 &&
                                             mrp_ctrl->sc_base_ref_list1_count <= 1 &&
                                             mrp_ctrl->sc_non_base_ref_list0_count <= 1 &&
                                             mrp_ctrl->sc_non_base_ref_list1_count <= 1) &&
                    (mrp_ctrl->base_ref_list0_count <= 1 && mrp_ctrl->base_ref_list1_count <= 1 &&
                     mrp_ctrl->non_base_ref_list0_count <= 1 && mrp_ctrl->non_base_ref_list1_count <= 1)
                ? 2
                : (mrp_ctrl->sc_base_ref_list0_count <= 2 && mrp_ctrl->sc_base_ref_list1_count <= 2 &&
                   mrp_ctrl->sc_non_base_ref_list0_count <= 2 && mrp_ctrl->sc_non_base_ref_list1_count <= 2) &&
                    (mrp_ctrl->base_ref_list0_count <= 2 && mrp_ctrl->base_ref_list1_count <= 2 &&
                     mrp_ctrl->non_base_ref_list0_count <= 2 && mrp_ctrl->non_base_ref_list1_count <= 2)
                ? 1
                : 0;
        }
    } else {
        mrp_ctrl->ld_reduce_ref_buffs = 0;
    }
}

static uint8_t get_tpl(uint8_t pred_structure, uint8_t superres_mode, uint8_t resize_mode, uint8_t aq_mode,
                       bool allintra) {
    if (allintra) {
        SVT_WARN("TPL is disabled for all-intra coding\n");
        return 0;
    } else if (aq_mode == 0) {
        SVT_WARN("TPL is disabled for aq_mode 0\n");
        return 0;
    } else if (pred_structure == LOW_DELAY) {
        SVT_WARN("TPL is disabled in low delay applications.\n");
        return 0;
    }
    // allow TPL with auto-dual and auto-all
    else if (superres_mode > SUPERRES_NONE && superres_mode != SUPERRES_AUTO && superres_mode != SUPERRES_QTHRESH) {
        SVT_WARN("TPL will be disabled when super resolution is enabled!\n");
        return 0;
    } else if (resize_mode > RESIZE_NONE) {
        SVT_WARN("TPL will be disabled when reference scalings (resize) is enabled!\n");
        return 0;
    } else {
        return 1;
    }
}

/*
* Set multi Pass Params
*/
void set_multi_pass_params(SequenceControlSet* scs) {
    EbSvtAv1EncConfiguration* config = &scs->static_config;

    // Update passes
    if (scs->static_config.pass != ENC_SINGLE_PASS) {
        scs->passes = MAX_ENCODE_PASS;
    } else {
        scs->passes = 1;
    }

    switch (config->pass) {
    case ENC_SINGLE_PASS: {
        scs->first_pass_downsample = false;
        scs->final_pass_preset     = config->enc_mode;
        break;
    }
    case ENC_FIRST_PASS: {
        scs->first_pass_downsample = config->enc_mode > ENC_M8;
        scs->final_pass_preset     = config->enc_mode;
        if (scs->final_pass_preset <= ENC_M6) {
            scs->static_config.enc_mode = ENC_M9;
        } else {
            scs->static_config.enc_mode = MAX_ENC_PRESET;
        }
        scs->static_config.rate_control_mode  = SVT_AV1_RC_MODE_CQP_OR_CRF;
        scs->static_config.qp                 = 43;
        scs->static_config.intra_refresh_type = SVT_AV1_KF_REFRESH;
        scs->static_config.max_bit_rate       = 0;
        break;
    }
    case ENC_SECOND_PASS: {
        scs->final_pass_preset                = config->enc_mode;
        scs->static_config.intra_refresh_type = SVT_AV1_KF_REFRESH;
        break;
    }
    default: {
        assert(0);
        break;
    }
    }

    int do_downsample = scs->first_pass_downsample && scs->max_input_luma_width >= 128 &&
            scs->max_input_luma_height >= 128
        ? 1
        : 0;

    if (do_downsample) {
        // To make sure the down scaled video has width and height of multiple of 2
        scs->max_input_luma_width  = (scs->max_input_luma_width >> 2) << 1;
        scs->max_input_luma_height = (scs->max_input_luma_height >> 2) << 1;
    }

    if (scs->lap_rc) {
        scs->static_config.intra_refresh_type = SVT_AV1_KF_REFRESH;
    }
    if (scs->static_config.pass == ENC_FIRST_PASS && scs->final_pass_preset > ENC_M6) {
        scs->rc_stat_gen_pass_mode = 1;
    } else {
        scs->rc_stat_gen_pass_mode = 0;
    }

    if (scs->static_config.recode_loop > 0 &&
        ((scs->static_config.rate_control_mode == SVT_AV1_RC_MODE_CQP_OR_CRF && scs->static_config.max_bit_rate == 0) ||
         (scs->static_config.rate_control_mode == SVT_AV1_RC_MODE_CBR))) {
        // Only allow re-encoding for VBR or capped CRF, otherwise force recode_loop to DISALLOW_RECODE or 0
        scs->static_config.recode_loop = DISALLOW_RECODE;
    } else if (scs->static_config.recode_loop == ALLOW_RECODE_DEFAULT) {
        // capped CRF has reencde enabled for base layer frames for all presets
        if (!scs->static_config.rate_control_mode && scs->static_config.max_bit_rate) {
            scs->static_config.recode_loop = ALLOW_RECODE_KFARFGF;
        } else {
            scs->static_config.recode_loop = scs->static_config.enc_mode <= ENC_M3 ? ALLOW_RECODE_KFARFGF
                                                                                   : ALLOW_RECODE_KFMAXBW;
        }
    }

    scs->enc_ctx->recode_loop = scs->static_config.recode_loop;
}

static void validate_scaling_params(SequenceControlSet* scs) {
    if (scs->static_config.superres_mode == SUPERRES_FIXED && scs->static_config.superres_denom == SCALE_NUMERATOR &&
        scs->static_config.superres_kf_denom == SCALE_NUMERATOR) {
        scs->static_config.superres_mode = SUPERRES_NONE;
    }
    if (scs->static_config.resize_mode == RESIZE_DYNAMIC) {
        if (scs->static_config.pred_structure != LOW_DELAY || scs->static_config.pass != ENC_SINGLE_PASS ||
            scs->static_config.rate_control_mode != SVT_AV1_RC_MODE_CBR) {
            SVT_WARN("Resize dynamic mode only works at 1-pass CBR low delay mode!\n");
            scs->static_config.resize_mode = RESIZE_NONE;
        }
    }
    if (scs->static_config.superres_mode == SUPERRES_QTHRESH && scs->static_config.superres_qthres == MAX_QP_VALUE &&
        scs->static_config.superres_kf_qthres == MAX_QP_VALUE) {
        scs->static_config.superres_mode = SUPERRES_NONE;
    }
    if (scs->static_config.resize_mode == RESIZE_FIXED && scs->static_config.resize_denom == SCALE_NUMERATOR &&
        scs->static_config.resize_kf_denom == SCALE_NUMERATOR) {
        scs->static_config.resize_mode = RESIZE_NONE;
    }
}

#if TUNE_STILL_IMAGE
void set_qp_based_th_scaling_ctrls_default(SequenceControlSet* scs) {
    QpBasedThScaling* qp_ctrls = &scs->qp_based_th_scaling_ctrls;
    const EncMode     enc_mode = scs->static_config.enc_mode;

    if (enc_mode <= ENC_MR) {
        qp_ctrls->tf_me_qp_based_th_scaling        = 0;
        qp_ctrls->tf_ref_qp_based_th_scaling       = 0;
        qp_ctrls->depths_qp_based_th_scaling       = 0;
        qp_ctrls->hme_qp_based_th_scaling          = 0;
        qp_ctrls->me_qp_based_th_scaling           = 0;
        qp_ctrls->nsq_qp_based_th_scaling          = 0;
        qp_ctrls->nic_max_qp_based_th_scaling      = 0;
        qp_ctrls->nic_pruning_qp_based_th_scaling  = 0;
        qp_ctrls->pme_qp_based_th_scaling          = 0;
        qp_ctrls->txt_qp_based_th_scaling          = 0;
        qp_ctrls->cap_max_size_qp_based_th_scaling = 1;
        qp_ctrls->lpd0_qp_based_th_scaling         = 1;
    } else {
        qp_ctrls->tf_me_qp_based_th_scaling        = 1;
        qp_ctrls->tf_ref_qp_based_th_scaling       = 1;
        qp_ctrls->depths_qp_based_th_scaling       = 1;
        qp_ctrls->hme_qp_based_th_scaling          = 1;
        qp_ctrls->me_qp_based_th_scaling           = 1;
        qp_ctrls->nsq_qp_based_th_scaling          = 1;
        qp_ctrls->nic_max_qp_based_th_scaling      = 1;
        qp_ctrls->nic_pruning_qp_based_th_scaling  = 1;
        qp_ctrls->pme_qp_based_th_scaling          = 1;
        qp_ctrls->txt_qp_based_th_scaling          = 1;
        qp_ctrls->cap_max_size_qp_based_th_scaling = 1;
        qp_ctrls->lpd0_qp_based_th_scaling         = 1;
    }
}

void set_qp_based_th_scaling_ctrls_rtc(SequenceControlSet* scs) {
    QpBasedThScaling* qp_ctrls = &scs->qp_based_th_scaling_ctrls;
    const EncMode     enc_mode = scs->static_config.enc_mode;

    if (enc_mode <= ENC_MR) {
        qp_ctrls->tf_me_qp_based_th_scaling        = 0;
        qp_ctrls->tf_ref_qp_based_th_scaling       = 0;
        qp_ctrls->depths_qp_based_th_scaling       = 0;
        qp_ctrls->hme_qp_based_th_scaling          = 0;
        qp_ctrls->me_qp_based_th_scaling           = 0;
        qp_ctrls->nsq_qp_based_th_scaling          = 0;
        qp_ctrls->nic_max_qp_based_th_scaling      = 0;
        qp_ctrls->nic_pruning_qp_based_th_scaling  = 0;
        qp_ctrls->pme_qp_based_th_scaling          = 0;
        qp_ctrls->txt_qp_based_th_scaling          = 0;
        qp_ctrls->cap_max_size_qp_based_th_scaling = 1;
        qp_ctrls->lpd0_qp_based_th_scaling         = 1;
    } else {
        qp_ctrls->tf_me_qp_based_th_scaling        = 1;
        qp_ctrls->tf_ref_qp_based_th_scaling       = 1;
        qp_ctrls->depths_qp_based_th_scaling       = 1;
        qp_ctrls->hme_qp_based_th_scaling          = 1;
        qp_ctrls->me_qp_based_th_scaling           = 1;
        qp_ctrls->nsq_qp_based_th_scaling          = 1;
        qp_ctrls->nic_max_qp_based_th_scaling      = 1;
        qp_ctrls->nic_pruning_qp_based_th_scaling  = 1;
        qp_ctrls->pme_qp_based_th_scaling          = 1;
        qp_ctrls->txt_qp_based_th_scaling          = 1;
        qp_ctrls->cap_max_size_qp_based_th_scaling = 1;
        qp_ctrls->lpd0_qp_based_th_scaling         = 1;
    }
}

void set_qp_based_th_scaling_ctrls_all_intra(SequenceControlSet* scs) {
    QpBasedThScaling* qp_ctrls = &scs->qp_based_th_scaling_ctrls;
    const EncMode     enc_mode = scs->static_config.enc_mode;

    if (enc_mode <= ENC_M3) {
        qp_ctrls->tf_me_qp_based_th_scaling        = 0;
        qp_ctrls->tf_ref_qp_based_th_scaling       = 0;
        qp_ctrls->depths_qp_based_th_scaling       = 0;
        qp_ctrls->hme_qp_based_th_scaling          = 0;
        qp_ctrls->me_qp_based_th_scaling           = 0;
        qp_ctrls->nsq_qp_based_th_scaling          = 0;
        qp_ctrls->nic_max_qp_based_th_scaling      = 1;
        qp_ctrls->nic_pruning_qp_based_th_scaling  = 1;
        qp_ctrls->pme_qp_based_th_scaling          = 0;
        qp_ctrls->txt_qp_based_th_scaling          = 1;
        qp_ctrls->cap_max_size_qp_based_th_scaling = 1;
        qp_ctrls->lpd0_qp_based_th_scaling         = 1;
    } else if (enc_mode <= ENC_M6) {
        qp_ctrls->tf_me_qp_based_th_scaling        = 0;
        qp_ctrls->tf_ref_qp_based_th_scaling       = 0;
        qp_ctrls->depths_qp_based_th_scaling       = 0;
        qp_ctrls->hme_qp_based_th_scaling          = 0;
        qp_ctrls->me_qp_based_th_scaling           = 0;
        qp_ctrls->nsq_qp_based_th_scaling          = 1;
        qp_ctrls->nic_max_qp_based_th_scaling      = 1;
        qp_ctrls->nic_pruning_qp_based_th_scaling  = 1;
        qp_ctrls->pme_qp_based_th_scaling          = 0;
        qp_ctrls->txt_qp_based_th_scaling          = 1;
        qp_ctrls->cap_max_size_qp_based_th_scaling = 1;
        qp_ctrls->lpd0_qp_based_th_scaling         = 1;
    } else {
        qp_ctrls->tf_me_qp_based_th_scaling        = 1;
        qp_ctrls->tf_ref_qp_based_th_scaling       = 1;
        qp_ctrls->depths_qp_based_th_scaling       = 1;
        qp_ctrls->hme_qp_based_th_scaling          = 1;
        qp_ctrls->me_qp_based_th_scaling           = 1;
        qp_ctrls->nsq_qp_based_th_scaling          = 1;
        qp_ctrls->nic_max_qp_based_th_scaling      = 1;
        qp_ctrls->nic_pruning_qp_based_th_scaling  = 1;
        qp_ctrls->pme_qp_based_th_scaling          = 1;
        qp_ctrls->txt_qp_based_th_scaling          = 1;
        qp_ctrls->cap_max_size_qp_based_th_scaling = 1;
        qp_ctrls->lpd0_qp_based_th_scaling         = 1;
    }
}
#else
void set_qp_based_th_scaling_ctrls(SequenceControlSet* scs) {
    QpBasedThScaling* qp_ctrls = &scs->qp_based_th_scaling_ctrls;
    const EncMode     enc_mode = scs->static_config.enc_mode;
    const bool        allintra = scs->allintra;
    if (allintra) {
        if (enc_mode <= ENC_M2) {
            qp_ctrls->tf_me_qp_based_th_scaling              = 0;
            qp_ctrls->tf_ref_qp_based_th_scaling             = 0;
            qp_ctrls->depths_qp_based_th_scaling             = 0;
            qp_ctrls->hme_qp_based_th_scaling                = 0;
            qp_ctrls->me_qp_based_th_scaling                 = 0;
            qp_ctrls->nsq_qp_based_th_scaling                = 0;
            qp_ctrls->nic_max_qp_based_th_scaling            = 1;
            qp_ctrls->nic_pruning_qp_based_th_scaling        = 1;
            qp_ctrls->pme_qp_based_th_scaling                = 0;
            qp_ctrls->txt_qp_based_th_scaling                = 1;
#if FTR_VLPD0
#if !CLN_DR
            qp_ctrls->i_depth_removal_qp_based_th_scaling = 1;
#endif
            qp_ctrls->cap_max_size_qp_based_th_scaling = 1;
#if !CLN_REMOVE_VAR_SUB_DEPTH
            qp_ctrls->var_skip_sub_depth_qp_based_th_scaling = 1;
#endif
            qp_ctrls->lpd0_qp_based_th_scaling = 1;
#else
            qp_ctrls->i_depth_removal_qp_based_th_scaling    = 0;
            qp_ctrls->cap_max_size_qp_based_th_scaling       = 0;
            qp_ctrls->var_skip_sub_depth_qp_based_th_scaling = 0;
#endif
        } else if (enc_mode <= ENC_M5) {
            qp_ctrls->tf_me_qp_based_th_scaling              = 0;
            qp_ctrls->tf_ref_qp_based_th_scaling             = 0;
            qp_ctrls->depths_qp_based_th_scaling             = 0;
            qp_ctrls->hme_qp_based_th_scaling                = 0;
            qp_ctrls->me_qp_based_th_scaling                 = 0;
            qp_ctrls->nsq_qp_based_th_scaling                = 1;
            qp_ctrls->nic_max_qp_based_th_scaling            = 1;
            qp_ctrls->nic_pruning_qp_based_th_scaling        = 1;
            qp_ctrls->pme_qp_based_th_scaling                = 0;
            qp_ctrls->txt_qp_based_th_scaling                = 1;
#if FTR_VLPD0
#if !CLN_DR
            qp_ctrls->i_depth_removal_qp_based_th_scaling = 1;
#endif
            qp_ctrls->cap_max_size_qp_based_th_scaling = 1;
#if !CLN_REMOVE_VAR_SUB_DEPTH
            qp_ctrls->var_skip_sub_depth_qp_based_th_scaling = 1;
#endif
            qp_ctrls->lpd0_qp_based_th_scaling = 1;
#else
            qp_ctrls->i_depth_removal_qp_based_th_scaling    = 0;
            qp_ctrls->cap_max_size_qp_based_th_scaling       = 0;
            qp_ctrls->var_skip_sub_depth_qp_based_th_scaling = 0;
#endif
        } else {
            qp_ctrls->tf_me_qp_based_th_scaling              = 1;
            qp_ctrls->tf_ref_qp_based_th_scaling             = 1;
            qp_ctrls->depths_qp_based_th_scaling             = 1;
            qp_ctrls->hme_qp_based_th_scaling                = 1;
            qp_ctrls->me_qp_based_th_scaling                 = 1;
            qp_ctrls->nsq_qp_based_th_scaling                = 1;
            qp_ctrls->nic_max_qp_based_th_scaling            = 1;
            qp_ctrls->nic_pruning_qp_based_th_scaling        = 1;
            qp_ctrls->pme_qp_based_th_scaling                = 1;
            qp_ctrls->txt_qp_based_th_scaling                = 1;
#if FTR_VLPD0
#if !CLN_DR
            qp_ctrls->i_depth_removal_qp_based_th_scaling = 1;
#endif
            qp_ctrls->cap_max_size_qp_based_th_scaling = 1;
#if !CLN_REMOVE_VAR_SUB_DEPTH
            qp_ctrls->var_skip_sub_depth_qp_based_th_scaling = 1;
#endif
            qp_ctrls->lpd0_qp_based_th_scaling = 1;
#else
            qp_ctrls->i_depth_removal_qp_based_th_scaling    = 1;
            qp_ctrls->cap_max_size_qp_based_th_scaling       = 1;
            qp_ctrls->var_skip_sub_depth_qp_based_th_scaling = 1;
#endif
        }
    } else {
        if (enc_mode <= ENC_MR) {
            qp_ctrls->tf_me_qp_based_th_scaling              = 0;
            qp_ctrls->tf_ref_qp_based_th_scaling             = 0;
            qp_ctrls->depths_qp_based_th_scaling             = 0;
            qp_ctrls->hme_qp_based_th_scaling                = 0;
            qp_ctrls->me_qp_based_th_scaling                 = 0;
            qp_ctrls->nsq_qp_based_th_scaling                = 0;
            qp_ctrls->nic_max_qp_based_th_scaling            = 0;
            qp_ctrls->nic_pruning_qp_based_th_scaling        = 0;
            qp_ctrls->pme_qp_based_th_scaling                = 0;
            qp_ctrls->txt_qp_based_th_scaling                = 0;
#if FTR_VLPD0
#if !CLN_DR
            qp_ctrls->i_depth_removal_qp_based_th_scaling = 1;
#endif
            qp_ctrls->cap_max_size_qp_based_th_scaling = 1;
#if !CLN_REMOVE_VAR_SUB_DEPTH
            qp_ctrls->var_skip_sub_depth_qp_based_th_scaling = 1;
#endif
            qp_ctrls->lpd0_qp_based_th_scaling = 1;
#else
            qp_ctrls->i_depth_removal_qp_based_th_scaling    = 0;
            qp_ctrls->cap_max_size_qp_based_th_scaling       = 0;
            qp_ctrls->var_skip_sub_depth_qp_based_th_scaling = 0;
#endif
        } else {
            qp_ctrls->tf_me_qp_based_th_scaling              = 1;
            qp_ctrls->tf_ref_qp_based_th_scaling             = 1;
            qp_ctrls->depths_qp_based_th_scaling             = 1;
            qp_ctrls->hme_qp_based_th_scaling                = 1;
            qp_ctrls->me_qp_based_th_scaling                 = 1;
            qp_ctrls->nsq_qp_based_th_scaling                = 1;
            qp_ctrls->nic_max_qp_based_th_scaling            = 1;
            qp_ctrls->nic_pruning_qp_based_th_scaling        = 1;
            qp_ctrls->pme_qp_based_th_scaling                = 1;
            qp_ctrls->txt_qp_based_th_scaling                = 1;
#if FTR_VLPD0
#if !CLN_DR
            qp_ctrls->i_depth_removal_qp_based_th_scaling = 1;
#endif
            qp_ctrls->cap_max_size_qp_based_th_scaling = 1;
#if !CLN_REMOVE_VAR_SUB_DEPTH
            qp_ctrls->var_skip_sub_depth_qp_based_th_scaling = 1;
#endif
            qp_ctrls->lpd0_qp_based_th_scaling = 1;
#else
            qp_ctrls->i_depth_removal_qp_based_th_scaling    = 0;
            qp_ctrls->cap_max_size_qp_based_th_scaling       = 0;
            qp_ctrls->var_skip_sub_depth_qp_based_th_scaling = 0;
#endif
        }
    }
}
#endif

static void set_param_based_on_input(SequenceControlSet* scs) {
    const bool allintra = scs->allintra;
#if TUNE_STILL_IMAGE
    const bool rtc_tune = scs->static_config.rtc;
#endif
    set_multi_pass_params(scs);

    // superres_mode and resize_mode may be updated,
    // so should call get_tpl_level() after validate_scaling_params()
    validate_scaling_params(scs);
    scs->tpl               = get_tpl(scs->static_config.pred_structure,
                       scs->static_config.superres_mode,
                       scs->static_config.resize_mode,
                       scs->static_config.aq_mode,
                       allintra);
    uint16_t subsampling_x = scs->subsampling_x;
    uint16_t subsampling_y = scs->subsampling_y;
    // Update picture width, and picture height
    if (scs->max_input_luma_width % MIN_BLOCK_SIZE) {
        scs->max_input_pad_right  = MIN_BLOCK_SIZE - (scs->max_input_luma_width % MIN_BLOCK_SIZE);
        scs->max_input_luma_width = scs->max_input_luma_width + scs->max_input_pad_right;
    } else {
        scs->max_input_pad_right = 0;
    }

    if (scs->max_input_luma_height % MIN_BLOCK_SIZE) {
        scs->max_input_pad_bottom  = MIN_BLOCK_SIZE - (scs->max_input_luma_height % MIN_BLOCK_SIZE);
        scs->max_input_luma_height = scs->max_input_luma_height + scs->max_input_pad_bottom;
    } else {
        scs->max_input_pad_bottom = 0;
    }
    scs->max_initial_input_luma_width  = scs->max_input_luma_width;
    scs->max_initial_input_luma_height = scs->max_input_luma_height;
    scs->max_initial_input_pad_bottom  = scs->max_input_pad_bottom;
    scs->max_initial_input_pad_right   = scs->max_input_pad_right;

    scs->chroma_width  = scs->max_input_luma_width >> subsampling_x;
    scs->chroma_height = scs->max_input_luma_height >> subsampling_y;

    scs->static_config.source_width  = scs->max_input_luma_width;
    scs->static_config.source_height = scs->max_input_luma_height;
    if (scs->static_config.superres_mode > SUPERRES_NONE) {
        if (scs->static_config.tile_rows || scs->static_config.tile_columns) {
            // disable tiles if super-res is on
            SVT_WARN("Tiles will be disabled when super resolution is enabled!\n");
            scs->static_config.tile_rows    = 0;
            scs->static_config.tile_columns = 0;
        }
        if (scs->static_config.superres_mode == SUPERRES_RANDOM) {
            SVT_WARN(
                "Super resolution random mode is designed for test and debugging purpose,\n"
                "it creates array of picture buffers for all scaling denominators (up to 8) of each reference frame.\n"
                "This mode retains a significant amount of memory, much more than other modes!\n");
        }
    }
    if (scs->static_config.resize_mode > RESIZE_NONE) {
        if (scs->static_config.tile_rows || scs->static_config.tile_columns) {
            // disable tiles if resize is on
            SVT_WARN("Tiles will be disabled when resize is enabled!\n");
            scs->static_config.tile_rows    = 0;
            scs->static_config.tile_columns = 0;
        }
        if (scs->static_config.resize_mode == RESIZE_RANDOM) {
            SVT_WARN(
                "Resize random mode is designed for test and debugging purpose,\n"
                "it creates array of picture buffers for all scaling denominators (up to 8) of each reference frame.\n"
                "This mode retains a significant amount of memory, much more than other modes!\n");
        }
    }
    // Set initial qp for vbr and middle pass
    if ((scs->static_config.rate_control_mode == SVT_AV1_RC_MODE_VBR) || (scs->static_config.pass == ENC_FIRST_PASS)) {
        if (scs->static_config.qp != DEFAULT_QP) {
            SVT_WARN("The input q value is ignored in vbr mode %d\n", scs->static_config.qp);
        }
        const uint8_t  tbr_bands[5]    = {1, 2, 4, 6, 8};
        const uint64_t src_samples     = (uint64_t)(scs->seq_header.max_frame_width * scs->seq_header.max_frame_height);
        const uint64_t target_bit_rate = (uint64_t)((double)scs->static_config.target_bit_rate * 60.0 /
                                                    (scs->frame_rate * (double)src_samples));
        if (target_bit_rate < tbr_bands[0]) {
            scs->static_config.qp = 55;
        } else if (target_bit_rate < tbr_bands[1]) {
            scs->static_config.qp = 50;
        } else if (target_bit_rate < tbr_bands[2]) {
            scs->static_config.qp = 45;
        } else if (target_bit_rate < tbr_bands[3]) {
            scs->static_config.qp = 40;
        } else if (target_bit_rate < tbr_bands[4]) {
            scs->static_config.qp = 35;
        } else {
            scs->static_config.qp = 30;
        }
    }
    svt_aom_derive_input_resolution(&scs->input_resolution, scs->max_input_luma_width * scs->max_input_luma_height);

    scs->seq_qp_mod = 2;

#if TUNE_STILL_IMAGE
    (allintra       ? set_qp_based_th_scaling_ctrls_all_intra
         : rtc_tune ? set_qp_based_th_scaling_ctrls_rtc
                    : set_qp_based_th_scaling_ctrls_default)(scs);
#else
    set_qp_based_th_scaling_ctrls(scs);
#endif

    // Set tune params
    derive_vq_params(scs);

    // Set TF level
    derive_tf_params(scs);

    //Future frames window in Scene Change Detection (SCD) / TemporalFiltering
    scs->scd_delay = 0;

    // Update the scd_delay based on the the number of future frames @ ISLICE
    // This case is needed for non-delayed Intra (intra_period_length == 0)
    uint32_t scd_delay_islice = 0;
    if (scs->static_config.intra_period_length == 0) {
        if (scs->tf_params_per_type[0].enabled) {
            scd_delay_islice = MIN(
                scs->tf_params_per_type[0].num_future_pics +
                    (scs->tf_params_per_type[0].modulate_pics
                         ? TF_MAX_EXTENSION
                         : 0), // number of future picture(s) used for ISLICE + max picture(s) after noise-based adjustement (=6)
                scs->tf_params_per_type[0].max_num_future_pics);
        }
    }

    // Update the scd_delay based on the the number of future frames @ BASE
    uint32_t scd_delay_base = 0;
    if (scs->tf_params_per_type[1].enabled) {
        scd_delay_base = MIN(
            scs->tf_params_per_type[1].num_future_pics +
                (scs->tf_params_per_type[1].modulate_pics
                     ? TF_MAX_EXTENSION
                     : 0), // number of future picture(s) used for BASE + max picture(s) after filtered adjustement (=3)
            scs->tf_params_per_type[1].max_num_future_pics);
    }
    scs->scd_delay = MAX(scd_delay_islice, scd_delay_base);
    // Update the scd_delay based on SCD, 1first pass
    // Delay needed for SCD , 1first pass of (2pass and 1pass VBR)
    if (scs->static_config.scene_change_detection || scs->vq_ctrls.sharpness_ctrls.scene_transition || scs->lap_rc) {
        scs->scd_delay = MAX(scs->scd_delay, 2);
    }

    // no future minigop is used for lowdelay prediction structure
    if (allintra || scs->static_config.pred_structure == LOW_DELAY) {
        scs->lad_mg = scs->tpl_lad_mg = 0;
    } else {
        uint8_t tpl_lad_mg =
            1; // Specify the number of mini-gops to be used as LAD. 0: 1 mini-gop, 1: 2 mini-gops and 3: 3 mini-gops
        uint32_t mg_size = 1 << scs->static_config.hierarchical_levels;
        // If the lookahead is specified to be less than one mini-gop, then use only the current mini-gop for TPL (the current MG is always required to encode).
        // Otherwise, set tpl_lad_mg to 1 when TPL is used, regardless of the specified lookahead, because TPL has been optimized to use 1 MG lookahead. Using
        // more lookahead MGs may result in disadvantageous trade-offs (speed/BDR/memory).
        if (scs->static_config.look_ahead_distance < mg_size) {
            tpl_lad_mg = 0;
        } else if (scs->tpl) {
            tpl_lad_mg = 1;
        } else {
            tpl_lad_mg = 0;
        }

        // special conditions for higher resolutions in order to decrease memory usage for tpl_lad_mg
        if (scs->input_resolution >= INPUT_SIZE_8K_RANGE) {
            tpl_lad_mg = 0;
        }
        scs->tpl_lad_mg = MIN(
            2, tpl_lad_mg); // lad_mg is capped to 2 because tpl was optimised only for 1,2 and 3 mini-gops
        if (scs->static_config.rate_control_mode == SVT_AV1_RC_MODE_CQP_OR_CRF) {
            scs->lad_mg = scs->tpl_lad_mg;
        } else {
            // update the look ahead size
            update_look_ahead(scs);
        }
    }
    // when resize mode is used, use sb 64 because of a r2r when 128 is used
    // In low delay mode, sb size is set to 64
    // in 240P resolution, sb size is set to 64
    if ((scs->static_config.fast_decode && scs->static_config.qp <= 56 &&
         !(scs->input_resolution <= INPUT_SIZE_360p_RANGE)) ||
        scs->static_config.resize_mode > RESIZE_NONE || scs->static_config.rtc ||
        (scs->input_resolution == INPUT_SIZE_240p_RANGE) || scs->static_config.enable_variance_boost) {
        scs->super_block_size = 64;
    } else if (allintra) {
#if TUNE_STILL_IMAGE
        if (scs->static_config.enc_mode <= ENC_M1) {
            scs->super_block_size = 128;
        } else {
            scs->super_block_size = 64;
        }
#else
        if (scs->input_resolution <= INPUT_SIZE_1080p_RANGE) {
            if (scs->static_config.enc_mode <= ENC_M0) {
                scs->super_block_size = 128;
            } else {
                scs->super_block_size = 64;
            }
        } else {
            if (scs->static_config.enc_mode <= ENC_M5) {
                scs->super_block_size = 128;
            } else {
                scs->super_block_size = 64;
            }
        }
#endif
    } else if (scs->static_config.enc_mode <= ENC_MR) {
        scs->super_block_size = 128;
    } else if (scs->static_config.enc_mode <= ENC_M5) {
        if (scs->static_config.qp <= 57) {
            scs->super_block_size = 64;
        } else {
            scs->super_block_size = 128;
        }
    } else {
        scs->super_block_size = 64;
    }
    // When switch frame is on, all renditions must have same super block size. See spec 5.5.1, 5.9.15.
    if (scs->static_config.sframe_dist != 0 || scs->static_config.sframe_posi.sframe_posis) {
        scs->super_block_size = 64;
    }
    // Set config info related to SB size
    if (scs->super_block_size == 128) {
        scs->seq_header.sb_size      = BLOCK_128X128;
        scs->sb_size                 = 128;
        scs->seq_header.sb_mi_size   = 32; // Size of the superblock in units of MI blocks
        scs->seq_header.sb_size_log2 = 5;
    } else {
        scs->seq_header.sb_size      = BLOCK_64X64;
        scs->sb_size                 = 64;
        scs->seq_header.sb_mi_size   = 16; // Size of the superblock in units of MI blocks
        scs->seq_header.sb_size_log2 = 4;
    }

    if (scs->static_config.enable_variance_boost && scs->static_config.rate_control_mode == SVT_AV1_RC_MODE_CBR) {
        scs->static_config.enable_variance_boost = false;
        SVT_WARN("Variance Boost is incompatible with CBR rate control, disabling Variance Boost\n");
    }
    if (scs->static_config.enable_variance_boost && scs->static_config.aq_mode == 1) {
        scs->static_config.enable_variance_boost = false;
        SVT_WARN("Variance AQ based on segmentation with Variance Boost not supported, disabling Variance Boost\n");
    }
    if (scs->static_config.variance_boost_strength >= 4) {
        SVT_WARN(
            "Aggressive Variance Boost strength used. This is a curve that's only useful under specific situations. "
            "Use with caution!\n");
    }
    if (scs->static_config.max_tx_size == 32 && scs->static_config.qp >= 25 && scs->static_config.tune != 3) {
        SVT_WARN(
            "Restricting transform sizes to a max of 32x32 might reduce coding efficiency at low to medium fidelity "
            "settings. Use with caution!\n");
    }
    if (scs->static_config.intra_refresh_type == SVT_AV1_FWDKF_REFRESH && scs->static_config.hierarchical_levels != 4) {
        scs->static_config.hierarchical_levels = 4;
        SVT_WARN(
            "Fwd key frame is only supported for hierarchical levels 4 at this point. Hierarchical levels are set to "
            "4\n");
    }
    bool    disallow_nsq  = true;
    uint8_t allow_HVA_HVB = 0;
    uint8_t allow_HV4     = 0;
    uint8_t h_v_only      = 1;
    uint8_t min_nsq_bsize = 0;
    uint8_t no_8x4_4x8    = 1;
    uint8_t no_16x8_8x16  = 1;
    for (uint8_t coeff_lvl = 0; coeff_lvl <= HIGH_LVL + 1; coeff_lvl++) {
#if TUNE_STILL_IMAGE
        uint8_t nsq_geom_level;
        if (scs->allintra) {
            nsq_geom_level = svt_aom_get_nsq_geom_level_allintra(scs->static_config.enc_mode);
        } else if (scs->static_config.rtc) {
            nsq_geom_level = svt_aom_get_nsq_geom_level_rtc(scs->static_config.enc_mode);
        } else {
            nsq_geom_level = svt_aom_get_nsq_geom_level_default(scs->static_config.enc_mode, coeff_lvl);
        }
#else
        uint8_t nsq_geom_level = svt_aom_get_nsq_geom_level(
            allintra, scs->input_resolution, scs->static_config.enc_mode, coeff_lvl, scs->static_config.rtc);
#endif
        disallow_nsq               = MIN(disallow_nsq, (nsq_geom_level == 0 ? 1 : 0));
        uint8_t temp_allow_HVA_HVB = 0, temp_allow_HV4 = 0;
        svt_aom_set_nsq_geom_ctrls(NULL, nsq_geom_level, &temp_allow_HVA_HVB, &temp_allow_HV4, &min_nsq_bsize);
        allow_HVA_HVB |= temp_allow_HVA_HVB;
        allow_HV4 |= temp_allow_HV4;
        h_v_only     = h_v_only && !allow_HVA_HVB && !allow_HV4;
        no_8x4_4x8   = no_8x4_4x8 && min_nsq_bsize >= 8;
        no_16x8_8x16 = no_16x8_8x16 && min_nsq_bsize >= 16;
    }

#if TUNE_STILL_IMAGE
    bool disallow_8x8;
    bool disallow_4x4;
    if (scs->allintra) {
        disallow_4x4 = svt_aom_get_disallow_4x4_allintra(scs->static_config.enc_mode);
        disallow_8x8 = svt_aom_get_disallow_8x8_allintra();
    } else if (scs->static_config.rtc) {
        disallow_4x4 = svt_aom_get_disallow_4x4_rtc(scs->static_config.enc_mode);
        disallow_8x8 = svt_aom_get_disallow_8x8_rtc(
            scs->static_config.enc_mode, scs->max_input_luma_width, scs->max_input_luma_height);
    } else {
        disallow_4x4 = svt_aom_get_disallow_4x4_default(scs->static_config.enc_mode);
        disallow_8x8 = svt_aom_get_disallow_8x8_default();
    }
#else
    bool disallow_4x4 = svt_aom_get_disallow_4x4(scs->static_config.enc_mode);
    bool disallow_8x8 = svt_aom_get_disallow_8x8(scs->static_config.enc_mode,
                                                 allintra,
                                                 scs->static_config.rtc,
                                                 scs->max_input_luma_width,
                                                 scs->max_input_luma_height);
#endif
    if (scs->super_block_size == 128) {
        if (!allow_HVA_HVB && disallow_4x4) {
            scs->svt_aom_geom_idx = GEOM_10;
            scs->max_block_cnt    = 2377;
        } else {
            scs->svt_aom_geom_idx = GEOM_9;
            scs->max_block_cnt    = 4421;
        }
    } else {
        //SB 64x64
        if (disallow_nsq && disallow_4x4 && disallow_8x8) {
            scs->svt_aom_geom_idx = GEOM_0;
            scs->max_block_cnt    = 21;
        } else if (h_v_only && disallow_4x4 && disallow_8x8 && no_16x8_8x16) {
            scs->svt_aom_geom_idx = GEOM_1;
            scs->max_block_cnt    = 41;
        } else if (disallow_nsq && disallow_4x4) {
            scs->svt_aom_geom_idx = GEOM_2;
            scs->max_block_cnt    = 85;
        } else if (h_v_only && disallow_4x4 && no_16x8_8x16) {
            scs->svt_aom_geom_idx = GEOM_3;
            scs->max_block_cnt    = 105;
        } else if (h_v_only && disallow_4x4 && no_8x4_4x8) {
            scs->svt_aom_geom_idx = GEOM_4;
            scs->max_block_cnt    = 169;
        } else if (h_v_only && disallow_4x4) {
            scs->svt_aom_geom_idx = GEOM_5;
            scs->max_block_cnt    = 425;
        } else if (h_v_only) {
            scs->svt_aom_geom_idx = GEOM_6;
            scs->max_block_cnt    = 681;
        } else if (!allow_HVA_HVB) {
            scs->svt_aom_geom_idx = GEOM_7;
            scs->max_block_cnt    = 849;
        } else {
            scs->svt_aom_geom_idx = GEOM_8;
            scs->max_block_cnt    = 1101;
        }
    }
    // Configure the padding
    scs->left_padding  = BLOCK_SIZE_64 + 4;
    scs->top_padding   = BLOCK_SIZE_64 + 4;
    scs->right_padding = BLOCK_SIZE_64 + 4;
    scs->bot_padding   = scs->super_block_size + 4;

    //for 10bit,  increase the pad of source from 68 to 72 (mutliple of 8) to accomodate 2bit-compression flow
    //we actually need to change the horizontal dimension only, but for simplicity/uniformity we do all directions
    // if (scs->static_config.encoder_bit_depth != EB_EIGHT_BIT)
    {
        scs->left_padding += 4;
        scs->top_padding += 4;
        scs->right_padding += 4;
        scs->bot_padding += 4;
    }

    scs->static_config.enable_overlays = !scs->static_config.enable_tf ||
            (scs->static_config.rate_control_mode != SVT_AV1_RC_MODE_CQP_OR_CRF)
        ? 0
        : scs->static_config.enable_overlays;

    // Enforce starting frame in decode order (at PicMgr)
    // Does not wait for feedback from PKT
    if (scs->static_config.level_of_parallelism == 1 || scs->static_config.pred_structure == LOW_DELAY) {
        scs->enable_pic_mgr_dec_order = 1;
    } else {
        scs->enable_pic_mgr_dec_order = 0;
    }
    // Enforce encoding frame in decode order
    // Wait for feedback from PKT
#if RC_NO_R2R
    scs->enable_dec_order = 1;
#else
    if (scs->static_config.level_of_parallelism == 1 || scs->static_config.pred_structure == LOW_DELAY) {
        scs->enable_dec_order = 1;
    } else {
        scs->enable_dec_order = 0;
    }
#endif
    // 1: Use boundary pixels in restoration filter search.
    // 0: Do not use boundary pixels in the restoration filter search.
    scs->use_boundaries_in_rest_search = 0;

    svt_aom_set_mfmv_config(scs);

    scs->list0_only_base = scs->static_config.enc_mode > ENC_M2;

    if (scs->static_config.rate_control_mode == SVT_AV1_RC_MODE_VBR ||
        scs->static_config.rate_control_mode == SVT_AV1_RC_MODE_CBR || scs->input_resolution >= INPUT_SIZE_4K_RANGE ||
        scs->static_config.pred_structure == LOW_DELAY || scs->static_config.pass != ENC_SINGLE_PASS) {
        scs->enable_dg = 0;
    } else {
        scs->enable_dg = scs->static_config.enable_dg;
    }
    // Set hbd_md OFF for high encode modes or bitdepth < 10
    if (scs->static_config.encoder_bit_depth < 10) {
        scs->enable_hbd_mode_decision = 0;
    }

    // Throws a warning when scene change is on, as the feature is not optimal and may produce false detections
    if (scs->static_config.scene_change_detection == 1) {
        SVT_WARN("Scene Change is not optimal and may produce suboptimal keyframe placements\n");
    }
    // MRP level
    uint8_t mrp_level;
    if (scs->static_config.rtc) {
        if (scs->static_config.enc_mode <= ENC_M8 || (!scs->use_flat_ipp && scs->static_config.enc_mode <= ENC_M10)) {
            mrp_level = 6;
        } else if ((scs->use_flat_ipp && scs->static_config.enc_mode <= ENC_M9) ||
                   (!scs->use_flat_ipp && scs->static_config.enc_mode <= ENC_M11)) {
            mrp_level = 9;
        } else {
            mrp_level = 0;
        }
    } else {
        if (scs->static_config.enc_mode <= ENC_MR) {
            mrp_level = 1;
        } else if (scs->static_config.enc_mode <= ENC_M2) {
            mrp_level = 2;
        } else if (scs->static_config.enc_mode <= ENC_M4) {
            mrp_level = 4;
        } else if (scs->static_config.enc_mode <= ENC_M8) {
            mrp_level = 6;
        } else if (scs->static_config.enc_mode <= ENC_M9) {
            mrp_level = scs->static_config.pred_structure == RANDOM_ACCESS ? 7 : 9;
        } else {
            if (scs->static_config.encoder_bit_depth == EB_EIGHT_BIT) {
                mrp_level = scs->static_config.pred_structure == RANDOM_ACCESS ? 11 : 0;
            } else {
                mrp_level = scs->static_config.pred_structure == RANDOM_ACCESS ? 7 : 0;
            }
        }
    }
    set_mrp_ctrl(scs, mrp_level);
    scs->is_short_clip = scs->static_config.gop_constraint_rc
        ? 1
        : 0; // set to 1 if multipass and less than 200 frames in resourcecordination
    if (allintra || scs->static_config.aq_mode == 1 || scs->static_config.scene_change_detection == 1 ||
        scs->vq_ctrls.sharpness_ctrls.tf == 1 || scs->static_config.enable_variance_boost) {
        scs->calculate_variance = 1;
    } else {
        scs->calculate_variance = 0;
    }

    scs->resize_pending_params.resize_state = ORIG;
    scs->resize_pending_params.resize_denom = SCALE_NUMERATOR;

    scs->stats_based_sb_lambda_modulation = 1;

    scs->fast_aa_aware_screen_detection_mode = (scs->static_config.enc_mode >= ENC_M3) ? 1 : 0;
}

static void copy_api_from_app(SequenceControlSet* scs, EbSvtAv1EncConfiguration* config_struct) {
    scs->max_input_luma_width  = config_struct->source_width;
    scs->max_input_luma_height = config_struct->source_height;
    // SB Definitions
    scs->static_config.pred_structure = config_struct->pred_structure;
    scs->static_config.rtc            = config_struct->rtc;
    if (scs->static_config.rtc && scs->static_config.pred_structure != LOW_DELAY) {
        scs->static_config.pred_structure = LOW_DELAY;
        SVT_WARN("Instance %u: Force low delay pred structure to be used for rtc.\n");
    }
    scs->enable_qp_scaling_flag = scs->allintra ? 0 : 1;
    // Set Picture Parameters for statistics gathering
    scs->picture_analysis_number_of_regions_per_width  = scs->max_input_luma_width >= 64
         ? HIGHER_THAN_CLASS_1_REGION_SPLIT_PER_WIDTH
         : 1;
    scs->picture_analysis_number_of_regions_per_height = scs->max_input_luma_height >= 64
        ? HIGHER_THAN_CLASS_1_REGION_SPLIT_PER_HEIGHT
        : 1;

    scs->pic_based_rate_est   = false;
    scs->block_mean_calc_prec = BLOCK_MEAN_PREC_SUB;
    scs->speed_control_flag   = 0;
    // Padding Offsets
    scs->b64_size                          = 64;
    scs->static_config.intra_period_length = config_struct->intra_period_length;
    scs->static_config.avif                = config_struct->avif;
    scs->allintra                          = (scs->static_config.intra_period_length == 0 || scs->static_config.avif);
    scs->static_config.multiply_keyint     = config_struct->multiply_keyint;
    scs->static_config.intra_refresh_type  = config_struct->intra_refresh_type;
    scs->static_config.enc_mode            = config_struct->enc_mode;
#if TUNE_STILL_IMAGE
    if (scs->allintra) {
        if (scs->static_config.enc_mode == ENC_MR) {
            SVT_WARN("The lowest supported preset for all-intra and still-image is M0.\n");
            scs->static_config.enc_mode = ENC_M0;
        } else if (scs->static_config.enc_mode > ENC_M9) {
            SVT_WARN("Preset M%d is mapped to M9.\n", scs->static_config.enc_mode);
            scs->static_config.enc_mode = ENC_M9;
        }
    } else if (scs->static_config.rtc) {
        if (scs->static_config.enc_mode > ENC_M12) {
            SVT_WARN("Preset M%d is mapped to M12.\n", scs->static_config.enc_mode);
            scs->static_config.enc_mode = ENC_M12;
        }
#else
    if (scs->static_config.rtc || scs->allintra) {
        if (scs->static_config.enc_mode > ENC_M12) {
            SVT_WARN("Preset M%d is mapped to M12.\n", scs->static_config.enc_mode);
            scs->static_config.enc_mode = ENC_M12;
        }

#endif
    }

    else if (scs->static_config.enc_mode > ENC_M11) {
        SVT_WARN("Preset M%d is mapped to M11.\n", scs->static_config.enc_mode);
        scs->static_config.enc_mode = ENC_M11;
    }

    EbInputResolution input_resolution;
    svt_aom_derive_input_resolution(&input_resolution, scs->max_input_luma_width * scs->max_input_luma_height);
    if (!scs->allintra && scs->static_config.pred_structure == RANDOM_ACCESS && scs->static_config.enc_mode > ENC_M9 &&
        input_resolution >= INPUT_SIZE_4K_RANGE) {
        scs->static_config.enc_mode = ENC_M9;
        SVT_WARN(
            "Setting preset to M9 as it is the highest supported preset for 4k and higher resolutions in Random Access "
            "mode\n");
    }

    scs->static_config.use_qp_file                    = config_struct->use_qp_file;
    scs->static_config.use_fixed_qindex_offsets       = config_struct->use_fixed_qindex_offsets;
    scs->static_config.key_frame_chroma_qindex_offset = config_struct->key_frame_chroma_qindex_offset;
    scs->static_config.key_frame_qindex_offset        = config_struct->key_frame_qindex_offset;
    if (scs->static_config.use_fixed_qindex_offsets) {
        scs->enable_qp_scaling_flag    = scs->static_config.use_fixed_qindex_offsets == 1
               ? 0
               : 1; // do not shut the auto QPS if use_fixed_qindex_offsets 2
        scs->static_config.use_qp_file = 0;
        memcpy(scs->static_config.qindex_offsets, config_struct->qindex_offsets, MAX_TEMPORAL_LAYERS * sizeof(int32_t));
    }
    memcpy(scs->static_config.chroma_qindex_offsets,
           config_struct->chroma_qindex_offsets,
           MAX_TEMPORAL_LAYERS * sizeof(int32_t));

    scs->static_config.lossless = config_struct->lossless;
    if (scs->static_config.lossless) {
        scs->static_config.luma_y_dc_qindex_offset   = 0;
        scs->static_config.chroma_u_dc_qindex_offset = 0;
        scs->static_config.chroma_u_ac_qindex_offset = 0;
        scs->static_config.chroma_v_dc_qindex_offset = 0;
        scs->static_config.chroma_v_ac_qindex_offset = 0;
    } else {
        scs->static_config.luma_y_dc_qindex_offset   = config_struct->luma_y_dc_qindex_offset;
        scs->static_config.chroma_u_dc_qindex_offset = config_struct->chroma_u_dc_qindex_offset;
        scs->static_config.chroma_u_ac_qindex_offset = config_struct->chroma_u_ac_qindex_offset;
        scs->static_config.chroma_v_dc_qindex_offset = config_struct->chroma_v_dc_qindex_offset;
        scs->static_config.chroma_v_ac_qindex_offset = config_struct->chroma_v_ac_qindex_offset;
    }
    memcpy(scs->static_config.lambda_scale_factors,
           config_struct->lambda_scale_factors,
           SVT_AV1_FRAME_UPDATE_TYPES * sizeof(int32_t));

    scs->static_config.rc_stats_buffer = config_struct->rc_stats_buffer;
    scs->static_config.pass            = config_struct->pass;
    // Deblock Filter
    scs->static_config.enable_dlf_flag = config_struct->enable_dlf_flag;

    // CDEF
    scs->static_config.cdef_level = config_struct->cdef_level;

    // Restoration filtering
    scs->static_config.enable_restoration_filtering = config_struct->enable_restoration_filtering;

    // motion field motion vector
    scs->static_config.enable_mfmv = config_struct->enable_mfmv;

    // Dynamic GoP
    scs->static_config.enable_dg = config_struct->enable_dg;

    // Decoder Optimization Flag
    scs->static_config.fast_decode = config_struct->fast_decode;

    //Film Grain
    scs->static_config.film_grain_denoise_strength = config_struct->film_grain_denoise_strength;
    scs->static_config.film_grain_denoise_apply    = config_struct->film_grain_denoise_apply;
    if (scs->static_config.film_grain_denoise_strength == 0 && scs->static_config.film_grain_denoise_apply == 1) {
        SVT_WARN("Film grain denoise apply signal is going to be ignored when film grain is off.\n");
    }
    scs->seq_header.film_grain_params_present = (uint8_t)(scs->static_config.film_grain_denoise_strength > 0);
    scs->static_config.fgs_table              = config_struct->fgs_table;

    // MD Parameters
    scs->enable_hbd_mode_decision = config_struct->encoder_bit_depth > 8 ? DEFAULT : 0;
    {
        if (config_struct->tile_rows == DEFAULT && config_struct->tile_columns == DEFAULT) {
            scs->static_config.tile_rows    = 0;
            scs->static_config.tile_columns = 0;

        } else {
            if (config_struct->tile_rows == DEFAULT) {
                scs->static_config.tile_rows    = 0;
                scs->static_config.tile_columns = config_struct->tile_columns;
            } else if (config_struct->tile_columns == DEFAULT) {
                scs->static_config.tile_rows    = config_struct->tile_rows;
                scs->static_config.tile_columns = 0;
            } else {
                scs->static_config.tile_rows    = config_struct->tile_rows;
                scs->static_config.tile_columns = config_struct->tile_columns;
            }
        }
    }

    // Rate Control
    scs->static_config.scene_change_detection = config_struct->scene_change_detection;
    if (config_struct->lossless && config_struct->rate_control_mode) {
        scs->static_config.rate_control_mode = SVT_AV1_RC_MODE_CQP_OR_CRF;
        SVT_WARN("Switched to CQP mode since lossless coding is enabled\n");
    } else {
        scs->static_config.rate_control_mode = config_struct->rate_control_mode;
    }
    if (scs->static_config.pass == ENC_SINGLE_PASS && scs->static_config.rtc) {
        if (scs->static_config.enc_mode < ENC_M7) {
            scs->static_config.enc_mode = ENC_M7;
            SVT_WARN("rtc mode only supports presets [7-%d]. Forcing preset to 7.\n", ENC_M13);
        }
    }
    scs->static_config.tune                = config_struct->tune;
    scs->static_config.hierarchical_levels = config_struct->hierarchical_levels;

    if (scs->static_config.rtc && scs->static_config.hierarchical_levels == 0) {
        scs->static_config.hierarchical_levels = HIERARCHICAL_LEVELS_AUTO;
        // Mimic flat prediction structure
        scs->use_flat_ipp = 1;
    }
    // Set the default hierarchical levels
    if (scs->static_config.hierarchical_levels == HIERARCHICAL_LEVELS_AUTO) {
        scs->static_config.hierarchical_levels = scs->static_config.pred_structure == LOW_DELAY &&
                (scs->static_config.rate_control_mode == SVT_AV1_RC_MODE_CBR ||
                 !(scs->static_config.enc_mode <= ENC_M9))
            ? 2
            : scs->static_config.pred_structure == LOW_DELAY ? 3
            : scs->static_config.rate_control_mode == SVT_AV1_RC_MODE_VBR ||
                scs->static_config.rate_control_mode == SVT_AV1_RC_MODE_CBR ||
                (input_resolution >= INPUT_SIZE_1080p_RANGE && scs->static_config.enc_mode >= ENC_M8) ||
                !(scs->static_config.enc_mode <= ENC_M8) || input_resolution >= INPUT_SIZE_8K_RANGE
            ? 4
            : 5;
    }
    if (scs->static_config.pass == ENC_SINGLE_PASS && scs->static_config.pred_structure == LOW_DELAY) {
        if (scs->static_config.rate_control_mode == SVT_AV1_RC_MODE_CBR &&
            scs->static_config.hierarchical_levels != 2) {
            scs->static_config.hierarchical_levels = 2;
            SVT_WARN("Forced Low delay CBR mode to use HierarchicalLevels = 2\n");
        }
    }
    // Set hierarchical_levels to 2 to reduce memory allocation; 2 is the minimum currently supported
    if (scs->allintra) {
        scs->static_config.hierarchical_levels = 2;
    }
    scs->max_temporal_layers                  = scs->static_config.hierarchical_levels;
    scs->static_config.look_ahead_distance    = config_struct->look_ahead_distance;
    scs->static_config.frame_rate_denominator = config_struct->frame_rate_denominator;
    scs->static_config.frame_rate_numerator   = config_struct->frame_rate_numerator;

    scs->static_config.target_bit_rate = config_struct->target_bit_rate;
    scs->static_config.max_bit_rate    = config_struct->max_bit_rate;
    //TODO: check RC mode and set only when RC is enabled in the final version.
    scs->static_config.aq_mode = scs->static_config.lossless ? 0 : config_struct->aq_mode;

    // TPL is disabled for allintra and LD encoding, and when aq_mode is 0
    if (scs->static_config.max_bit_rate &&
        (scs->static_config.aq_mode == 0 || scs->allintra || scs->static_config.pred_structure == LOW_DELAY)) {
        scs->static_config.max_bit_rate = 0;
        SVT_WARN("Maximum bit rate only supported with tpl on. max bit rate 0 is used instead.\n");
    }

    scs->static_config.max_qp_allowed = scs->static_config.lossless ? MIN_QP_VALUE : config_struct->max_qp_allowed;

    scs->static_config.min_qp_allowed      = scs->static_config.lossless ? MIN_QP_VALUE
             : config_struct->min_qp_allowed == MIN_QP_AUTO ? scs->static_config.rate_control_mode ? 4 : MIN_QP_VALUE
                                                            : config_struct->min_qp_allowed;
    scs->static_config.vbr_min_section_pct = config_struct->vbr_min_section_pct;
    scs->static_config.vbr_max_section_pct = config_struct->vbr_max_section_pct;
    scs->static_config.under_shoot_pct     = config_struct->under_shoot_pct;
    scs->static_config.over_shoot_pct      = config_struct->over_shoot_pct;
    if (scs->static_config.under_shoot_pct == (uint32_t)DEFAULT) {
        if (scs->static_config.rate_control_mode == SVT_AV1_RC_MODE_VBR) {
            scs->static_config.under_shoot_pct = 50;
        } else {
            scs->static_config.under_shoot_pct = 25;
        }
    }

    if (scs->static_config.over_shoot_pct == (uint32_t)DEFAULT) {
        scs->static_config.over_shoot_pct = 25;
    }
    scs->static_config.mbr_over_shoot_pct       = config_struct->mbr_over_shoot_pct;
    scs->static_config.gop_constraint_rc        = config_struct->gop_constraint_rc;
    scs->static_config.maximum_buffer_size_ms   = config_struct->maximum_buffer_size_ms;
    scs->static_config.starting_buffer_level_ms = config_struct->starting_buffer_level_ms;
    scs->static_config.optimal_buffer_level_ms  = config_struct->optimal_buffer_level_ms;
    scs->static_config.recode_loop              = config_struct->recode_loop;
    if (scs->static_config.rate_control_mode == SVT_AV1_RC_MODE_VBR && scs->static_config.pass == ENC_SINGLE_PASS) {
        scs->lap_rc = 1;
    } else {
        scs->lap_rc = 0;
    }
    // Misc
    scs->static_config.encoder_bit_depth    = config_struct->encoder_bit_depth;
    scs->static_config.encoder_color_format = config_struct->encoder_color_format;

    scs->chroma_format_idc = (uint32_t)(scs->static_config.encoder_color_format);
    scs->encoder_bit_depth = (uint32_t)(scs->static_config.encoder_bit_depth);
    //16bit pipeline
    scs->is_16bit_pipeline = ((config_struct->encoder_bit_depth) > EB_EIGHT_BIT) ? true : false;
    scs->subsampling_x     = (scs->chroma_format_idc == EB_YUV444 ? 0 : 1);
    scs->subsampling_y     = (scs->chroma_format_idc >= EB_YUV422 ? 0 : 1);
    // Force screen-content detection OFF when allintra
#if OPT_SC_ALLINTRA_DETECTION
    const bool allintra = scs->allintra;
    if (allintra) {
        scs->static_config.screen_content_mode = 0;
        SVT_WARN("SC is disabled for all-intra coding\n");
    } else {
        scs->static_config.screen_content_mode = config_struct->screen_content_mode;
    }
#else
    scs->static_config.screen_content_mode = scs->allintra ? 0 : config_struct->screen_content_mode;
#endif
    // Annex A parameters
    scs->static_config.profile     = config_struct->profile;
    scs->static_config.tier        = config_struct->tier;
    scs->static_config.level       = config_struct->level;
    scs->static_config.stat_report = config_struct->stat_report;

    // Buffers - Hardcoded(Cleanup)
    scs->static_config.use_cpu_flags = config_struct->use_cpu_flags;

    scs->static_config.level_of_parallelism = config_struct->level_of_parallelism;
    if (scs->static_config.level_of_parallelism >= PARALLEL_LEVEL_COUNT) {
        SVT_WARN("Level of parallelism supports levels [0-%d]. Setting maximum parallelism level.\n",
                 PARALLEL_LEVEL_COUNT - 1);
        SVT_WARN(
            "Level of parallelism does not correspond to a target number of processors to use. See Docs/Parameters.md "
            "for info.\n");
        scs->static_config.level_of_parallelism = PARALLEL_LEVEL_6;
    }

    scs->static_config.qp            = config_struct->qp;
    scs->static_config.recon_enabled = config_struct->recon_enabled;

    // Extract frame rate from Numerator and Denominator if not 0
    if (scs->static_config.frame_rate_numerator != 0 && scs->static_config.frame_rate_denominator != 0) {
        scs->frame_rate = (double)scs->static_config.frame_rate_numerator /
            (double)scs->static_config.frame_rate_denominator;
    }
    // Get Default Intra Period if not specified
    if (scs->static_config.intra_period_length == -2) {
        scs->static_config.intra_period_length = compute_default_intra_period(scs);
        scs->allintra = (scs->static_config.intra_period_length == 0 || scs->static_config.avif);
    } else if (scs->static_config.multiply_keyint) {
        const double fps = (double)scs->static_config.frame_rate_numerator / scs->static_config.frame_rate_denominator;
        scs->static_config.intra_period_length = (int32_t)(fps * scs->static_config.intra_period_length);
    }
    if (scs->static_config.look_ahead_distance == (uint32_t)~0) {
        scs->static_config.look_ahead_distance = compute_default_look_ahead(&scs->static_config);
    }
    scs->static_config.enable_tf          = scs->allintra ? 0 : config_struct->enable_tf;
    scs->static_config.enable_overlays    = config_struct->enable_overlays;
    scs->static_config.superres_mode      = config_struct->superres_mode;
    scs->static_config.superres_denom     = config_struct->superres_denom;
    scs->static_config.superres_kf_denom  = config_struct->superres_kf_denom;
    scs->static_config.superres_qthres    = config_struct->superres_qthres;
    scs->static_config.superres_kf_qthres = config_struct->superres_kf_qthres;

    if (scs->static_config.superres_mode == SUPERRES_AUTO) {
        // TODO: set search mode based on preset
        //scs->static_config.superres_auto_search_type = SUPERRES_AUTO_SOLO;
        scs->static_config.superres_auto_search_type = SUPERRES_AUTO_DUAL;
        //scs->static_config.superres_auto_search_type = SUPERRES_AUTO_ALL;
    }

    scs->static_config.resize_mode     = config_struct->resize_mode;
    scs->static_config.resize_denom    = config_struct->resize_denom;
    scs->static_config.resize_kf_denom = config_struct->resize_kf_denom;
    if (config_struct->frame_scale_evts.start_frame_nums) {
        EB_NO_THROW_MALLOC(scs->static_config.frame_scale_evts.start_frame_nums,
                           sizeof(int64_t) * config_struct->frame_scale_evts.evt_num);
        memcpy(scs->static_config.frame_scale_evts.start_frame_nums,
               config_struct->frame_scale_evts.start_frame_nums,
               sizeof(int64_t) * config_struct->frame_scale_evts.evt_num);
    }
    if (config_struct->frame_scale_evts.resize_kf_denoms) {
        EB_NO_THROW_MALLOC(scs->static_config.frame_scale_evts.resize_kf_denoms,
                           sizeof(int32_t) * config_struct->frame_scale_evts.evt_num);
        memcpy(scs->static_config.frame_scale_evts.resize_kf_denoms,
               config_struct->frame_scale_evts.resize_kf_denoms,
               sizeof(int32_t) * config_struct->frame_scale_evts.evt_num);
    }
    if (config_struct->frame_scale_evts.resize_denoms) {
        EB_NO_THROW_MALLOC(scs->static_config.frame_scale_evts.resize_denoms,
                           sizeof(int32_t) * config_struct->frame_scale_evts.evt_num);
        memcpy(scs->static_config.frame_scale_evts.resize_denoms,
               config_struct->frame_scale_evts.resize_denoms,
               sizeof(int32_t) * config_struct->frame_scale_evts.evt_num);
    }
    scs->static_config.frame_scale_evts.evt_num = config_struct->frame_scale_evts.evt_num;
    if (config_struct->sframe_posi.sframe_posis) {
        EB_NO_THROW_MALLOC(scs->static_config.sframe_posi.sframe_posis,
                           sizeof(uint64_t) * config_struct->sframe_posi.sframe_num);
        memcpy(scs->static_config.sframe_posi.sframe_posis,
               config_struct->sframe_posi.sframe_posis,
               sizeof(uint64_t) * config_struct->sframe_posi.sframe_num);
    }
    scs->static_config.sframe_posi.sframe_num = config_struct->sframe_posi.sframe_num;
    if (config_struct->sframe_posi.sframe_qps) {
        EB_NO_THROW_MALLOC(scs->static_config.sframe_posi.sframe_qps,
                           sizeof(config_struct->sframe_posi.sframe_qps[0]) * config_struct->sframe_posi.sframe_qp_num);
        memcpy(scs->static_config.sframe_posi.sframe_qps,
               config_struct->sframe_posi.sframe_qps,
               sizeof(config_struct->sframe_posi.sframe_qps[0]) * config_struct->sframe_posi.sframe_qp_num);
    }
    if (config_struct->sframe_posi.sframe_qp_offsets) {
        EB_NO_THROW_MALLOC(
            scs->static_config.sframe_posi.sframe_qp_offsets,
            sizeof(config_struct->sframe_posi.sframe_qp_offsets[0]) * config_struct->sframe_posi.sframe_qp_num);
        memcpy(scs->static_config.sframe_posi.sframe_qp_offsets,
               config_struct->sframe_posi.sframe_qp_offsets,
               sizeof(config_struct->sframe_posi.sframe_qp_offsets[0]) * config_struct->sframe_posi.sframe_qp_num);
    }
    scs->static_config.sframe_posi.sframe_qp_num = config_struct->sframe_posi.sframe_qp_num;

    // Color description
    scs->static_config.color_primaries          = config_struct->color_primaries;
    scs->static_config.transfer_characteristics = config_struct->transfer_characteristics;
    scs->static_config.matrix_coefficients      = config_struct->matrix_coefficients;
    scs->static_config.color_range              = config_struct->color_range;
    scs->static_config.chroma_sample_position   = config_struct->chroma_sample_position;
    scs->static_config.mastering_display        = config_struct->mastering_display;
    scs->static_config.content_light_level      = config_struct->content_light_level;

    // switch frame
    scs->static_config.sframe_dist      = config_struct->sframe_dist;
    scs->static_config.sframe_mode      = config_struct->sframe_mode;
    scs->static_config.sframe_qp        = config_struct->sframe_qp;
    scs->static_config.sframe_qp_offset = config_struct->sframe_qp_offset;
    scs->seq_header.max_frame_width  = config_struct->forced_max_frame_width > 0 ? config_struct->forced_max_frame_width
         : scs->static_config.sframe_dist > 0 || scs->static_config.sframe_posi.sframe_posis ? 16384
                                                                                             : scs->max_input_luma_width;
    scs->seq_header.max_frame_height = config_struct->forced_max_frame_height > 0
        ? config_struct->forced_max_frame_height
        : scs->static_config.sframe_dist > 0 || scs->static_config.sframe_posi.sframe_posis
        ? 8704
        : scs->max_input_luma_height;
    scs->static_config.force_key_frames = config_struct->force_key_frames;

    // QM
    scs->static_config.enable_qm           = config_struct->enable_qm;
    scs->static_config.min_qm_level        = config_struct->min_qm_level;
    scs->static_config.max_qm_level        = config_struct->max_qm_level;
    scs->static_config.min_chroma_qm_level = config_struct->min_chroma_qm_level;
    scs->static_config.max_chroma_qm_level = config_struct->max_chroma_qm_level;
    if (scs->static_config.enable_qm && scs->static_config.min_qm_level == 15 &&
        scs->static_config.max_qm_level == 15 && scs->static_config.min_chroma_qm_level == 15 &&
        scs->static_config.max_chroma_qm_level == 15) {
        SVT_WARN("Quantization matrices will be forced off since all min/max quant matrix levels are set to 15\n");
        scs->static_config.enable_qm = 0;
    }
    if (scs->static_config.enable_qm && scs->static_config.lossless) {
        SVT_WARN("Quantization matrices will be forced off since lossless coding is applied\n");
        scs->static_config.enable_qm = 0;
    }
    scs->static_config.startup_mg_size   = config_struct->startup_mg_size;
    scs->static_config.startup_qp_offset = config_struct->startup_qp_offset;
    scs->static_config.enable_roi_map    = config_struct->enable_roi_map;

    // Variance Boost
    scs->static_config.enable_variance_boost   = config_struct->enable_variance_boost;
    scs->static_config.variance_boost_strength = config_struct->variance_boost_strength;
#if OPT_OPERATIONS_BIS
    scs->static_config.variance_octile = scs->static_config.enable_variance_boost ? config_struct->variance_octile : 0;
#else
    scs->static_config.variance_octile = config_struct->variance_octile;
#endif
    scs->static_config.variance_boost_curve = config_struct->variance_boost_curve;

    // Temporal filtering strength
    scs->static_config.tf_strength = config_struct->tf_strength;

    // Frame-level luminance-based QP bias
    scs->static_config.luminance_qp_bias = config_struct->luminance_qp_bias;

    // Sharpness
    scs->static_config.sharpness = config_struct->sharpness;

    // QP scaling compression
    scs->static_config.qp_scale_compress_strength = config_struct->qp_scale_compress_strength;

    // Adaptive film grain
    scs->static_config.adaptive_film_grain = config_struct->adaptive_film_grain;

    // Max TX size
    scs->static_config.max_tx_size = config_struct->max_tx_size;

    // Extended CRF
    scs->static_config.extended_crf_qindex_offset = config_struct->extended_crf_qindex_offset;

    // AC bias
    scs->static_config.ac_bias = config_struct->ac_bias;

    // Override settings for Still IQ tune
    if (scs->static_config.tune == TUNE_IQ) {
        SVT_WARN(
            "Tune IQ overrides: sharpness, Var. Boost strength/curve, enable-qm and min/max level, max TX size and "
            "SCM\n");
        scs->static_config.enable_qm               = 1;
        scs->static_config.min_qm_level            = 4;
        scs->static_config.max_qm_level            = 10;
        scs->static_config.min_chroma_qm_level     = 4;
        scs->static_config.max_chroma_qm_level     = 10;
        scs->static_config.sharpness               = 7;
        scs->static_config.enable_variance_boost   = 1;
        scs->static_config.variance_boost_strength = 3;
        scs->static_config.variance_boost_curve    = 2;
        scs->static_config.max_tx_size             = scs->static_config.qp <= 45 ? 32 : 64;
        scs->static_config.screen_content_mode     = 3;
    } else if (scs->static_config.tune == TUNE_MS_SSIM) {
        SVT_WARN("Tune MS_SSIM overrides: sharpness, Var. Boost strength/curve, enable-qm and min/max level\n");
        scs->static_config.enable_qm               = 1;
        scs->static_config.min_qm_level            = 4;
        scs->static_config.max_qm_level            = 10;
        scs->static_config.min_chroma_qm_level     = 4;
        scs->static_config.max_chroma_qm_level     = 10;
        scs->static_config.sharpness               = 7;
        scs->static_config.enable_variance_boost   = 1;
        scs->static_config.variance_boost_strength = 3;
        scs->static_config.variance_boost_curve    = 2;
    }
    return;
}

/**********************************

* Set Parameter
**********************************/
EB_API EbErrorType svt_av1_enc_set_parameter(EbComponentType*          svt_enc_component,
                                             EbSvtAv1EncConfiguration* config_struct) {
    if (svt_enc_component == NULL) {
        return EB_ErrorBadParameter;
    }

    EbEncHandle*        enc_handle = (EbEncHandle*)svt_enc_component->p_component_private;
    SequenceControlSet* scs        = enc_handle->scs_instance->scs;
    copy_api_from_app(scs, config_struct);

    EbErrorType return_error = svt_av1_verify_settings(scs);

    if (return_error == EB_ErrorBadParameter) {
        return EB_ErrorBadParameter;
    }

    if (scs->static_config.avif) {
        scs->seq_header.still_picture                = 1;
        scs->seq_header.reduced_still_picture_header = 1;
    }
    set_param_based_on_input(scs);
    // Initialize the Prediction Structure Group
    EB_NO_THROW_NEW(enc_handle->scs_instance->enc_ctx->prediction_structure_group_ptr,
                    svt_aom_prediction_structure_group_ctor);
    if (!enc_handle->scs_instance->enc_ctx->prediction_structure_group_ptr) {
        return EB_ErrorInsufficientResources;
    }
    return_error = load_default_buffer_configuration_settings(scs);

    svt_av1_print_lib_params(scs);

    // free frame scale events after copy to encoder
    if (config_struct->frame_scale_evts.resize_denoms) {
        EB_FREE(config_struct->frame_scale_evts.resize_denoms);
    }
    if (config_struct->frame_scale_evts.resize_kf_denoms) {
        EB_FREE(config_struct->frame_scale_evts.resize_kf_denoms);
    }
    if (config_struct->frame_scale_evts.start_frame_nums) {
        EB_FREE(config_struct->frame_scale_evts.start_frame_nums);
    }
    memset(&config_struct->frame_scale_evts, 0, sizeof(SvtAv1FrameScaleEvts));

    // free sframe position list
    if (config_struct->sframe_posi.sframe_qps) {
        EB_FREE(config_struct->sframe_posi.sframe_qps);
    }
    if (config_struct->sframe_posi.sframe_qp_offsets) {
        EB_FREE(config_struct->sframe_posi.sframe_qp_offsets);
    }
    if (config_struct->sframe_posi.sframe_posis) {
        EB_FREE(config_struct->sframe_posi.sframe_posis);
    }
    memset(&config_struct->sframe_posi, 0, sizeof(SvtAv1SFramePositions));

    return return_error;
}

EB_API EbErrorType svt_av1_enc_stream_header(EbComponentType*     svt_enc_component,
                                             EbBufferHeaderType** output_stream_ptr) {
    EbErrorType return_error = EB_ErrorNone;

    if (!svt_enc_component) {
        return EB_ErrorBadParameter;
    }

    EbEncHandle*        enc_handle = (EbEncHandle*)svt_enc_component->p_component_private;
    SequenceControlSet* scs        = enc_handle->scs_instance->scs;
    Bitstream           bitstream;
    OutputBitstreamUnit output_bitstream;
    EbBufferHeaderType* output_stream_buffer;
    uint32_t output_buffer_size = svt_aom_get_out_buffer_size(scs->max_input_luma_width, scs->max_input_luma_height);
    memset(&bitstream, 0, sizeof(Bitstream));
    memset(&output_bitstream, 0, sizeof(OutputBitstreamUnit));
    bitstream.output_bitstream_ptr = &output_bitstream;
    EB_MALLOC_OBJECT(output_stream_buffer);
    EB_MALLOC_ARRAY_NO_CHECK(output_stream_buffer->p_buffer, output_buffer_size);
    if (!output_stream_buffer->p_buffer) {
        EB_FREE(output_stream_buffer);
        return EB_ErrorInsufficientResources;
    }

    output_stream_buffer->size          = sizeof(EbBufferHeaderType);
    output_stream_buffer->n_alloc_len   = output_buffer_size;
    output_stream_buffer->p_app_private = NULL;
    output_stream_buffer->pic_type      = EB_AV1_INVALID_PICTURE;
    output_stream_buffer->n_filled_len  = 0;

    bitstream.output_bitstream_ptr->buffer_begin_av1 = output_stream_buffer->p_buffer;

    svt_aom_output_bitstream_reset(bitstream.output_bitstream_ptr);

    // Code the SPS
    svt_aom_encode_sps_av1(&bitstream, scs);

    output_stream_buffer->n_filled_len = (uint32_t)(bitstream.output_bitstream_ptr->buffer_av1 -
                                                    bitstream.output_bitstream_ptr->buffer_begin_av1);

    *output_stream_ptr = output_stream_buffer;

    return return_error;
}

EB_API EbErrorType svt_av1_enc_stream_header_release(EbBufferHeaderType* stream_header_ptr) {
    EbErrorType return_error = EB_ErrorNone;

    if (!stream_header_ptr || !(stream_header_ptr->p_buffer)) {
        return EB_ErrorBadParameter;
    }

    EB_FREE_ARRAY(stream_header_ptr->p_buffer);
    EB_FREE(stream_header_ptr);

    return return_error;
}

/***********************************************
**** Copy the input buffer from the
**** sample application to the library buffers
************************************************/
/*
 Down sample and Copy the input buffer
from the sample application to the library buffers
*/
/********************************************
 * downsample_2d_c_16_zero2bit_skipall
 *      downsample the input by skipping three pixels and zero out the two LSB bit
 ********************************************/
static void downsample_2d_c_16_zero2bit_skipall(uint16_t* input_samples, // input parameter, input samples Ptr
                                                uint32_t  input_stride, // input parameter, input stride
                                                uint32_t  input_area_width, // input parameter, input area width
                                                uint32_t  input_area_height, // input parameter, input area height
                                                uint8_t*  decim_8b_samples, // output parameter, decimated samples Ptr
                                                uint32_t  decim_stride, // input parameter, output stride
                                                uint32_t  decim_step) // input parameter, decimation amount in pixels
{
    uint32_t       horizontal_index;
    uint32_t       vertical_index;
    uint32_t       input_stripe_stride = input_stride * decim_step;
    uint32_t       decim_horizontal_index;
    const uint32_t half_decim_step = decim_step >> 1;

    for (input_samples += half_decim_step * input_stride, vertical_index = half_decim_step;
         vertical_index < input_area_height;
         vertical_index += decim_step) {
        uint16_t* prev_input_line = input_samples - input_stride;
        for (horizontal_index = half_decim_step, decim_horizontal_index = 0; horizontal_index < input_area_width;
             horizontal_index += decim_step, decim_horizontal_index++) {
            decim_8b_samples[decim_horizontal_index] = (uint8_t)((prev_input_line[horizontal_index - 1]) >> 2);
        }
        input_samples += input_stripe_stride;
        decim_8b_samples += decim_stride;
    }

    return;
}

/********************************************
 * downsample_2d_c_skipall
 *      downsample the input by skipping three pixels
 ********************************************/
static void downsample_2d_c_skipall(uint8_t* input_samples, // input parameter, input samples Ptr
                                    uint32_t input_stride, // input parameter, input stride
                                    uint32_t input_area_width, // input parameter, input area width
                                    uint32_t input_area_height, // input parameter, input area height
                                    uint8_t* decim_samples, // output parameter, decimated samples Ptr
                                    uint32_t decim_stride, // input parameter, output stride
                                    uint32_t decim_step) // input parameter, decimation amount in pixels
{
    uint32_t       horizontal_index;
    uint32_t       vertical_index;
    uint32_t       input_stripe_stride = input_stride * decim_step;
    uint32_t       decim_horizontal_index;
    const uint32_t half_decim_step = decim_step >> 1;

    for (input_samples += half_decim_step * input_stride, vertical_index = half_decim_step;
         vertical_index < input_area_height;
         vertical_index += decim_step) {
        uint8_t* prev_input_line = input_samples - input_stride;
        for (horizontal_index = half_decim_step, decim_horizontal_index = 0; horizontal_index < input_area_width;
             horizontal_index += decim_step, decim_horizontal_index++) {
            decim_samples[decim_horizontal_index] = (uint32_t)prev_input_line[horizontal_index - 1];
        }
        input_samples += input_stripe_stride;
        decim_samples += decim_stride;
    }

    return;
}

/***********************************************
 Down sample and Copy the input buffer
from the sample application to the library buffers
************************************************/
static EbErrorType downsample_copy_frame_buffer(SequenceControlSet* scs, uint8_t* destination, uint8_t* destination_y8b,
                                                uint8_t* source, int pass) {
    EbErrorType return_error = EB_ErrorNone;

    EbPictureBufferDesc* input_pic             = (EbPictureBufferDesc*)destination;
    EbPictureBufferDesc* y8b_input_picture_ptr = (EbPictureBufferDesc*)destination_y8b;
    EbSvtIOFormat*       input_ptr             = (EbSvtIOFormat*)source;

    // Need to include for Interlacing on the fly with pictureScanType = 1
    uint32_t luma_buffer_offset   = input_pic->stride_y * scs->top_padding + scs->left_padding;
    uint32_t chroma_buffer_offset = input_pic->stride_cr * (scs->top_padding >> 1) + (scs->left_padding >> 1);
    uint32_t luma_width           = (uint32_t)(input_pic->width - scs->max_input_pad_right);
    uint32_t luma_height          = (uint32_t)(input_pic->height - scs->max_input_pad_bottom);

    const uint8_t  subsampling_x = (input_pic->color_format == EB_YUV444 ? 0 : 1);
    const uint8_t  subsampling_y = ((input_pic->color_format == EB_YUV444 || input_pic->color_format == EB_YUV422) ? 0
                                                                                                                   : 1);
    const uint32_t chroma_width  = (luma_width + subsampling_x) >> subsampling_x;
    const uint32_t chroma_height = (luma_height + subsampling_y) >> subsampling_y;

    if (scs->static_config.encoder_bit_depth == EB_EIGHT_BIT) {
        downsample_2d_c_skipall(input_ptr->luma,
                                input_ptr->y_stride,
                                luma_width << 1,
                                luma_height << 1,
                                y8b_input_picture_ptr->buffer_y + luma_buffer_offset,
                                input_pic->stride_y,
                                2);

        if (pass != ENCODE_FIRST_PASS) {
            downsample_2d_c_skipall(input_ptr->cb,
                                    input_ptr->cb_stride,
                                    chroma_width << 1,
                                    chroma_height << 1,
                                    input_pic->buffer_cb + chroma_buffer_offset,
                                    input_pic->stride_cb,
                                    2);
            downsample_2d_c_skipall(input_ptr->cr,
                                    input_ptr->cr_stride,
                                    chroma_width << 1,
                                    chroma_height << 1,
                                    input_pic->buffer_cr + chroma_buffer_offset,
                                    input_pic->stride_cr,
                                    2);
        }
    } else { // 10bit packed
        downsample_2d_c_16_zero2bit_skipall((uint16_t*)input_ptr->luma,
                                            input_ptr->y_stride,
                                            luma_width << 1,
                                            luma_height << 1,
                                            y8b_input_picture_ptr->buffer_y + luma_buffer_offset,
                                            y8b_input_picture_ptr->stride_y,
                                            2);

        memset(input_pic->buffer_bit_inc_y, 0, input_pic->luma_size / 4);

        if (pass != ENCODE_FIRST_PASS) {
            downsample_2d_c_16_zero2bit_skipall((uint16_t*)input_ptr->cb,
                                                input_ptr->cb_stride,
                                                chroma_width << 1,
                                                chroma_height << 1,
                                                input_pic->buffer_cb + chroma_buffer_offset,
                                                y8b_input_picture_ptr->stride_cb,
                                                2);

            memset(input_pic->buffer_bit_inc_cb, 0, input_pic->chroma_size / 4);

            downsample_2d_c_16_zero2bit_skipall((uint16_t*)input_ptr->cr,
                                                input_ptr->cr_stride,
                                                chroma_width << 1,
                                                chroma_height << 1,
                                                input_pic->buffer_cr + chroma_buffer_offset,
                                                y8b_input_picture_ptr->stride_cr,
                                                2);

            memset(input_pic->buffer_bit_inc_cr, 0, input_pic->chroma_size / 4);
        }
    }
    return return_error;
}

/*
 Copy the input buffer
from the sample application to the library buffers
*/

static EbErrorType copy_frame_buffer(SequenceControlSet* scs, uint8_t* destination, uint8_t* destination_y8b,
                                     uint8_t* source, int pass) {
    EbErrorType return_error = EB_ErrorNone;

    EbPictureBufferDesc* input_pic             = (EbPictureBufferDesc*)destination;
    EbPictureBufferDesc* y8b_input_picture_ptr = (EbPictureBufferDesc*)destination_y8b;
    EbSvtIOFormat*       input_ptr             = (EbSvtIOFormat*)source;

    // Need to include for Interlacing on the fly with pictureScanType = 1

    uint32_t luma_buffer_offset   = input_pic->stride_y * scs->top_padding + scs->left_padding;
    uint32_t chroma_buffer_offset = input_pic->stride_cr * (scs->top_padding >> 1) + (scs->left_padding >> 1);
    uint32_t luma_width           = (uint32_t)(input_pic->width - scs->max_input_pad_right);
    uint32_t luma_height          = (uint32_t)(input_pic->height - scs->max_input_pad_bottom);

    const uint8_t  subsampling_x = (input_pic->color_format == EB_YUV444 ? 0 : 1);
    const uint8_t  subsampling_y = ((input_pic->color_format == EB_YUV444 || input_pic->color_format == EB_YUV422) ? 0
                                                                                                                   : 1);
    const uint32_t chroma_width  = (luma_width + subsampling_x) >> subsampling_x;
    const uint32_t chroma_height = (luma_height + subsampling_y) >> subsampling_y;

    if (scs->static_config.encoder_bit_depth == EB_EIGHT_BIT) {
        svt_av1_copy_wxh_8bit(input_ptr->luma,
                              input_ptr->y_stride,
                              y8b_input_picture_ptr->buffer_y + luma_buffer_offset,
                              input_pic->stride_y,
                              luma_height,
                              luma_width);
        svt_av1_copy_wxh_8bit(input_ptr->cb,
                              input_ptr->cb_stride,
                              input_pic->buffer_cb + chroma_buffer_offset,
                              input_pic->stride_cb,
                              chroma_height,
                              chroma_width);
        svt_av1_copy_wxh_8bit(input_ptr->cr,
                              input_ptr->cr_stride,
                              input_pic->buffer_cr + chroma_buffer_offset,
                              input_pic->stride_cr,
                              chroma_height,
                              chroma_width);
    } else { // 10bit packed
        uint32_t comp_stride_y           = input_pic->stride_y / 4;
        uint32_t comp_luma_buffer_offset = comp_stride_y * input_pic->org_y + input_pic->org_x / 4;

        uint32_t comp_stride_uv            = input_pic->stride_cb / 4;
        uint32_t comp_chroma_buffer_offset = comp_stride_uv * (input_pic->org_y / 2) + input_pic->org_x / 2 / 4;

        svt_unpack_and_2bcompress((uint16_t*)input_ptr->luma,
                                  input_ptr->y_stride,
                                  y8b_input_picture_ptr->buffer_y + luma_buffer_offset,
                                  y8b_input_picture_ptr->stride_y,
                                  input_pic->buffer_bit_inc_y + comp_luma_buffer_offset,
                                  comp_stride_y,
                                  luma_width,
                                  luma_height);
        if (pass != ENCODE_FIRST_PASS) {
            svt_unpack_and_2bcompress((uint16_t*)input_ptr->cb,
                                      input_ptr->cb_stride,
                                      input_pic->buffer_cb + chroma_buffer_offset,
                                      input_pic->stride_cb,
                                      input_pic->buffer_bit_inc_cb + comp_chroma_buffer_offset,
                                      comp_stride_uv,
                                      chroma_width,
                                      chroma_height);

            svt_unpack_and_2bcompress((uint16_t*)input_ptr->cr,
                                      input_ptr->cr_stride,
                                      input_pic->buffer_cr + chroma_buffer_offset,
                                      input_pic->stride_cr,
                                      input_pic->buffer_bit_inc_cr + comp_chroma_buffer_offset,
                                      comp_stride_uv,
                                      chroma_width,
                                      chroma_height);
        }
    }
    return return_error;
}

static EbErrorType copy_private_data_list(EbBufferHeaderType* dst, EbBufferHeaderType* src) {
    EbErrorType     return_error = EB_ErrorNone;
    EbPrivDataNode* p_src_node   = (EbPrivDataNode*)src->p_app_private;
    EbPrivDataNode* p_first_node = NULL;
    EbPrivDataNode* p_new_node   = NULL;
    while (p_src_node) {
        // skip undefined data type and throw an error in debugging
        if (p_src_node->node_type < PRIVATE_DATA || p_src_node->node_type >= PRIVATE_DATA_TYPES) {
            svt_aom_assert_err(0, "unknown private data types inserted!");
            continue;
        }
        if (p_first_node == NULL) {
            EB_MALLOC(p_new_node, sizeof(*p_src_node));
            p_first_node = p_new_node;
        } else {
            EB_MALLOC(p_new_node->next, sizeof(*p_src_node));
            p_new_node = p_new_node->next;
        }
        p_new_node->node_type = p_src_node->node_type;
        p_new_node->size      = p_src_node->size;
        // not copy data from the private data pass through the encoder
        if (p_src_node->node_type == PRIVATE_DATA || p_src_node->node_type == ROI_MAP_EVENT) {
            p_new_node->data = p_src_node->data;
        } else {
            EB_MALLOC(p_new_node->data, p_src_node->size);
            memcpy(p_new_node->data, p_src_node->data, p_src_node->size);
        }
        p_new_node->next = NULL;
        p_src_node       = p_src_node->next;
    }
    dst->p_app_private = p_first_node;
    return return_error;
}

/**************************************
* svt_input_buffer_header_update: update the parameters in input_buffer_header for changing the resolution on the fly
**************************************/
EbErrorType svt_input_buffer_header_update(EbBufferHeaderType* input_buffer, SequenceControlSet* scs, bool noy8b) {
    EbPictureBufferDescInitData input_pic_buf_desc_init_data;
    EbSvtAv1EncConfiguration*   config   = &scs->static_config;
    uint8_t                     is_16bit = config->encoder_bit_depth > 8 ? 1 : 0;

    input_pic_buf_desc_init_data.max_width = !(scs->max_input_luma_width % 8)
        ? scs->max_input_luma_width
        : scs->max_input_luma_width + (scs->max_input_luma_width % 8);

    input_pic_buf_desc_init_data.max_height = !(scs->max_input_luma_height % 8)
        ? scs->max_input_luma_height
        : scs->max_input_luma_height + (scs->max_input_luma_height % 8);

    input_pic_buf_desc_init_data.bit_depth    = (EbBitDepth)config->encoder_bit_depth;
    input_pic_buf_desc_init_data.color_format = (EbColorFormat)config->encoder_color_format;

    input_pic_buf_desc_init_data.left_padding  = scs->left_padding;
    input_pic_buf_desc_init_data.right_padding = scs->right_padding;
    input_pic_buf_desc_init_data.top_padding   = scs->top_padding;
    input_pic_buf_desc_init_data.bot_padding   = scs->bot_padding;

    input_pic_buf_desc_init_data.split_mode = is_16bit ? true : false;

    input_pic_buf_desc_init_data.buffer_enable_mask = PICTURE_BUFFER_DESC_FULL_MASK;
    input_pic_buf_desc_init_data.is_16bit_pipeline  = 0;

    // Enhanced Picture Buffer
    if (!noy8b) {
        svt_picture_buffer_desc_update((EbPictureBufferDesc*)input_buffer->p_buffer,
                                       (EbPtr)&input_pic_buf_desc_init_data);
    } else {
        svt_picture_buffer_desc_noy8b_update((EbPictureBufferDesc*)input_buffer->p_buffer,
                                             (EbPtr)&input_pic_buf_desc_init_data);
    }

    return EB_ErrorNone;
}

/**************************************
* svt_input_y8b_update: update the parameters in input_y8b for changing the resolution on the fly
**************************************/
EbErrorType svt_input_y8b_update(EbBufferHeaderType* input_buffer, SequenceControlSet* scs) {
    EbPictureBufferDescInitData input_pic_buf_desc_init_data;
    EbSvtAv1EncConfiguration*   config   = &scs->static_config;
    uint8_t                     is_16bit = 0;

    input_pic_buf_desc_init_data.max_width = !(scs->max_input_luma_width % 8)
        ? scs->max_input_luma_width
        : scs->max_input_luma_width + (scs->max_input_luma_width % 8);

    input_pic_buf_desc_init_data.max_height   = !(scs->max_input_luma_height % 8)
          ? scs->max_input_luma_height
          : scs->max_input_luma_height + (scs->max_input_luma_height % 8);
    input_pic_buf_desc_init_data.bit_depth    = EB_EIGHT_BIT;
    input_pic_buf_desc_init_data.color_format = (EbColorFormat)config->encoder_color_format;

    input_pic_buf_desc_init_data.left_padding  = scs->left_padding;
    input_pic_buf_desc_init_data.right_padding = scs->right_padding;
    input_pic_buf_desc_init_data.top_padding   = scs->top_padding;
    input_pic_buf_desc_init_data.bot_padding   = scs->bot_padding;

    input_pic_buf_desc_init_data.split_mode = is_16bit ? true : false;

    input_pic_buf_desc_init_data.buffer_enable_mask = PICTURE_BUFFER_DESC_LUMA_MASK; //allocate for 8bit Luma only
    input_pic_buf_desc_init_data.is_16bit_pipeline  = 0;

    // Enhanced Picture Buffer
    svt_picture_buffer_desc_update((EbPictureBufferDesc*)input_buffer->p_buffer, (EbPtr)&input_pic_buf_desc_init_data);

    return EB_ErrorNone;
}

/*
    memset the library input buffer(s)
*/
static void memset_input_buffer(SequenceControlSet* scs, EbBufferHeaderType* dst, EbBufferHeaderType* dst_y8b,
                                EbBufferHeaderType* src, int pass) {
    // Copy the higher level structure
    dst->n_alloc_len  = src->n_alloc_len;
    dst->n_filled_len = src->n_filled_len;
    dst->flags        = src->flags;
    dst->pts          = src->pts;
    dst->n_tick_count = src->n_tick_count;
    dst->size         = src->size;
    dst->qp           = src->qp;
    dst->pic_type     = src->pic_type;
    if (scs->first_pass_downsample) {
        // memset the picture buffer
        if (src->p_buffer != NULL) {
            EbPictureBufferDesc*      y8b_input_picture_ptr = (EbPictureBufferDesc*)dst_y8b->p_buffer;
            EbPictureBufferDesc*      input_pic             = (EbPictureBufferDesc*)dst->p_buffer;
            EbSvtAv1EncConfiguration* config                = &scs->static_config;
            bool                      is_16bit_input        = config->encoder_bit_depth > EB_EIGHT_BIT;
            const uint8_t             subsampling_x         = (config->encoder_color_format == EB_YUV444 ? 0 : 1);
            const uint8_t             subsampling_y =
                ((config->encoder_color_format == EB_YUV444 || config->encoder_color_format == EB_YUV422) ? 0 : 1);
            const uint32_t chroma_width  = (input_pic->max_width + subsampling_x) >> subsampling_x;
            const uint32_t chroma_height = (input_pic->max_height + subsampling_y) >> subsampling_y;
            const uint32_t chroma_org_x  = (input_pic->org_x + subsampling_x) >> subsampling_x;
            const uint32_t chroma_org_y  = (input_pic->org_y + subsampling_y) >> subsampling_y;

            uint32_t y8b_input_size = ((y8b_input_picture_ptr->max_width + (y8b_input_picture_ptr->org_x << 1)) *
                                       (y8b_input_picture_ptr->max_height + (y8b_input_picture_ptr->org_y << 1)));
            memset(y8b_input_picture_ptr->buffer_y, 0, (y8b_input_size * sizeof(uint8_t)));

            uint32_t input_size = (input_pic->max_width + (input_pic->org_x << 1)) *
                (input_pic->max_height + (input_pic->org_y << 1));
            uint32_t chroma_input_size = (chroma_width + (chroma_org_x << 1)) * (chroma_height + (chroma_org_y << 1));

            memset(input_pic->buffer_cb, 128, chroma_input_size * sizeof(uint8_t));
            memset(input_pic->buffer_cr, 128, chroma_input_size * sizeof(uint8_t));
            if (is_16bit_input) {
                memset(input_pic->buffer_bit_inc_y, 0, ((input_size >> 2) * sizeof(uint8_t)));
                memset(input_pic->buffer_bit_inc_cb, 0, ((chroma_input_size >> 2) * sizeof(uint8_t)));
                memset(input_pic->buffer_bit_inc_cr, 0, ((chroma_input_size >> 2) * sizeof(uint8_t)));
            }
        }
    } else if (pass != ENCODE_FIRST_PASS) {
        // memset the picture buffer
        if (src->p_buffer != NULL) {
            EbPictureBufferDesc*      y8b_input_picture_ptr = (EbPictureBufferDesc*)dst_y8b->p_buffer;
            EbPictureBufferDesc*      input_pic             = (EbPictureBufferDesc*)dst->p_buffer;
            EbSvtAv1EncConfiguration* config                = &scs->static_config;
            bool                      is_16bit_input        = config->encoder_bit_depth > EB_EIGHT_BIT;
            const uint8_t             subsampling_x         = (config->encoder_color_format == EB_YUV444 ? 0 : 1);
            const uint8_t             subsampling_y =
                ((config->encoder_color_format == EB_YUV444 || config->encoder_color_format == EB_YUV422) ? 0 : 1);
            const uint32_t chroma_width  = (input_pic->max_width + subsampling_x) >> subsampling_x;
            const uint32_t chroma_height = (input_pic->max_height + subsampling_y) >> subsampling_y;
            const uint32_t chroma_org_x  = (input_pic->org_x + subsampling_x) >> subsampling_x;
            const uint32_t chroma_org_y  = (input_pic->org_y + subsampling_y) >> subsampling_y;

            uint32_t y8b_input_size = ((y8b_input_picture_ptr->max_width + (y8b_input_picture_ptr->org_x << 1)) *
                                       (y8b_input_picture_ptr->max_height + (y8b_input_picture_ptr->org_y << 1)));
            memset(y8b_input_picture_ptr->buffer_y, 0, (y8b_input_size * sizeof(uint8_t)));

            uint32_t input_size        = ((input_pic->max_width + (input_pic->org_x << 1)) *
                                   (input_pic->max_height + (input_pic->org_y << 1)));
            uint32_t chroma_input_size = (chroma_width + (chroma_org_x << 1)) * (chroma_height + (chroma_org_y << 1));

            memset(input_pic->buffer_cb, 128, chroma_input_size * sizeof(uint8_t));
            memset(input_pic->buffer_cr, 128, chroma_input_size * sizeof(uint8_t));
            if (is_16bit_input) {
                memset(input_pic->buffer_bit_inc_y, 0, ((input_size >> 2) * sizeof(uint8_t)));
                memset(input_pic->buffer_bit_inc_cb, 0, ((chroma_input_size >> 2) * sizeof(uint8_t)));
                memset(input_pic->buffer_bit_inc_cr, 0, ((chroma_input_size >> 2) * sizeof(uint8_t)));
            }
            // Copy the metadata array
            if (svt_aom_copy_metadata_buffer(dst, src->metadata) != EB_ErrorNone) {
                dst->metadata = NULL;
            }
        }
    }

    // Copy the private data list
    if (src->p_app_private) {
        copy_private_data_list(dst, src);
    } else {
        dst->p_app_private = NULL;
    }
}

/*
 Copy the input buffer header content
from the sample application to the library buffers
*/
static void copy_input_buffer(SequenceControlSet* scs, EbBufferHeaderType* dst, EbBufferHeaderType* dst_y8b,
                              EbBufferHeaderType* src, int pass) {
    // Copy the higher level structure
    dst->n_alloc_len  = src->n_alloc_len;
    dst->n_filled_len = src->n_filled_len;
    dst->flags        = src->flags;
    dst->pts          = src->pts;
    dst->n_tick_count = src->n_tick_count;
    dst->size         = src->size;
    dst->qp           = src->qp;
    dst->pic_type     = src->pic_type;
    if (scs->first_pass_downsample) {
        // Copy the picture buffer
        if (src->p_buffer != NULL) {
            downsample_copy_frame_buffer(scs, dst->p_buffer, dst_y8b->p_buffer, src->p_buffer, pass);
        }
    } else if (pass != ENCODE_FIRST_PASS) {
        // Bypass copy for the unecessary picture in IPPP pass
        // Copy the picture buffer
        if (src->p_buffer != NULL) {
            copy_frame_buffer(scs, dst->p_buffer, dst_y8b->p_buffer, src->p_buffer, pass);
            // Copy the metadata array
            if (svt_aom_copy_metadata_buffer(dst, src->metadata) != EB_ErrorNone) {
                dst->metadata = NULL;
            }
        }
    }

    // Copy the private data list
    if (src->p_app_private) {
        copy_private_data_list(dst, src);
    } else {
        dst->p_app_private = NULL;
    }
}

// Update the input picture definitions: resolution of the sequence
static EbErrorType validate_on_the_fly_settings(EbBufferHeaderType* input_ptr, SequenceControlSet* scs,
                                                EbHandle config_mutex) {
    EbPrivDataNode* node = (EbPrivDataNode*)input_ptr->p_app_private;
    while (node) {
        if (node->node_type == RES_CHANGE_EVENT) {
            SvtAv1InputPicDef* node_data = (SvtAv1InputPicDef*)node->data;
            if (input_ptr->pic_type != EB_AV1_KEY_PICTURE) {
                input_ptr->flags = EB_BUFFERFLAG_EOS;
                SVT_ERROR("Resolution change on the fly not supported for non key frames\n");
                return EB_ErrorBadParameter;
            } else if ((node_data->input_luma_height > scs->max_initial_input_luma_height) ||
                       (node_data->input_luma_width > scs->max_initial_input_luma_width)) {
                input_ptr->flags = EB_BUFFERFLAG_EOS;
                SVT_ERROR(
                    "Resolution cannot be changed to anything greater than the original picture width and height\n");
                return EB_ErrorBadParameter;
            } else if (scs->static_config.superres_mode > SUPERRES_NONE) {
                input_ptr->flags = EB_BUFFERFLAG_EOS;
                SVT_ERROR("Resolution change on the fly is not supported when Super-Resolution mode is on\n");
                return EB_ErrorBadParameter;
            } else if (scs->static_config.resize_mode != RESIZE_NONE) {
                input_ptr->flags = EB_BUFFERFLAG_EOS;
                SVT_ERROR("Resolution change on the fly is not supported when Reference Scaling mode is on\n");
                return EB_ErrorBadParameter;
            } else if (scs->static_config.pred_structure != LOW_DELAY) {
                input_ptr->flags = EB_BUFFERFLAG_EOS;
                SVT_ERROR("Resolution change on the fly is only supported for Low-Delay mode\n");
                return EB_ErrorBadParameter;
            } else if (scs->static_config.pass != ENC_SINGLE_PASS) {
                input_ptr->flags = EB_BUFFERFLAG_EOS;
                SVT_ERROR("Resolution change on the fly is only supported for single pass encoding\n");
                return EB_ErrorBadParameter;
            } else if (scs->static_config.tile_rows || scs->static_config.tile_columns) {
                input_ptr->flags = EB_BUFFERFLAG_EOS;
                SVT_ERROR("Resolution change on the fly is not supported when tiles are being used\n");
                return EB_ErrorBadParameter;
            } else if (scs->static_config.aq_mode == 1) {
                input_ptr->flags = EB_BUFFERFLAG_EOS;
                SVT_ERROR(
                    "Resolution change on the fly is not supported for segment based adaptive quantization (--aq-mode "
                    "== 1)\n");
                return EB_ErrorBadParameter;
            } else if (node_data->input_luma_width < 64) {
                input_ptr->flags = EB_BUFFERFLAG_EOS;
                SVT_ERROR("Resolution change on the fly is not supported for luma width less than 64\n");
                return EB_ErrorBadParameter;
            } else if (node_data->input_luma_height < 64) {
                input_ptr->flags = EB_BUFFERFLAG_EOS;
                SVT_ERROR("Resolution change on the fly is not supported for luma height less than 64\n");
                return EB_ErrorBadParameter;
            } else if (scs->static_config.encoder_bit_depth == EB_TEN_BIT) {
                input_ptr->flags = EB_BUFFERFLAG_EOS;
                SVT_ERROR("Resolution change on the fly is not supported for 10-bit encoding\n");
                return EB_ErrorBadParameter;
            } else {
                svt_aom_assert_err(node->size == sizeof(SvtAv1InputPicDef),
                                   "invalid private data of type RES_CHANGE_EVENT");
                SvtAv1InputPicDef* input_pic_def = (SvtAv1InputPicDef*)node->data;
                svt_block_on_mutex(config_mutex);
                // Check if a resolution change occurred
                scs->max_input_luma_width  = input_pic_def->input_luma_width;
                scs->max_input_luma_height = input_pic_def->input_luma_height;
                scs->max_input_pad_right   = input_pic_def->input_pad_right;
                scs->max_input_pad_bottom  = input_pic_def->input_pad_bottom;
                svt_release_mutex(config_mutex);
            }
        } else if (node->node_type == RATE_CHANGE_EVENT) {
            SvtAv1RateInfo* node_data = (SvtAv1RateInfo*)node->data;
            if ((scs->static_config.target_bit_rate != node_data->target_bit_rate) &&
                !((scs->static_config.pred_structure == LOW_DELAY) &&
                  (scs->static_config.rate_control_mode == SVT_AV1_RC_MODE_CBR))) {
                input_ptr->flags = EB_BUFFERFLAG_EOS;
                SVT_ERROR("TBR change on the fly not supported for any mode other than Low-Delay CBR\n");
                return EB_ErrorBadParameter;
            }
            if (node_data->seq_qp != 0) {
                if (node_data->seq_qp > MAX_QP_VALUE) {
                    input_ptr->flags = EB_BUFFERFLAG_EOS;
                    SVT_ERROR("QP change on the fly requires a QP value less than or equal to 63\n");
                    return EB_ErrorBadParameter;
                }
            }
            if (node_data->target_bit_rate > 100000000) {
                input_ptr->flags = EB_BUFFERFLAG_EOS;
                SVT_ERROR("TBR change on the fly requires that the target bit rate must be between [0, 100000] kbps\n");
                return EB_ErrorBadParameter;
            }
        } else if (node->node_type == FRAME_RATE_CHANGE_EVENT) {
            SvtAv1FrameRateInfo* node_data = (SvtAv1FrameRateInfo*)node->data;
            if (!((scs->static_config.pred_structure == LOW_DELAY) &&
                  (scs->static_config.rate_control_mode == SVT_AV1_RC_MODE_CBR))) {
                input_ptr->flags = EB_BUFFERFLAG_EOS;
                SVT_ERROR("Frame rate change on the fly not supported for any mode other than Low-Delay CBR\n");
                return EB_ErrorBadParameter;
            }
            if (node_data->frame_rate_numerator == 0 || node_data->frame_rate_denominator == 0) {
                input_ptr->flags = EB_BUFFERFLAG_EOS;
                SVT_ERROR(
                    "Frame rate change on the fly requires that he frame_rate_numerator and frame_rate_denominator "
                    "must be greater than 0\n");
                return EB_ErrorBadParameter;
            }
        }
        node = node->next;
    }
    return EB_ErrorNone;
}

/**********************************
* Empty This Buffer
**********************************/
EB_API EbErrorType svt_av1_enc_send_picture(EbComponentType* svt_enc_component, EbBufferHeaderType* p_buffer) {
    EbErrorType         return_val     = EB_ErrorNone;
    EbEncHandle*        enc_handle_ptr = (EbEncHandle*)svt_enc_component->p_component_private;
    EbObjectWrapper*    eb_wrapper_ptr;
    EbBufferHeaderType* app_hdr    = p_buffer;
    enc_handle_ptr->frame_received = true;

    SequenceControlSet* scs = enc_handle_ptr->scs_instance->scs;
    if (scs->static_config.avif && (p_buffer->flags & EB_BUFFERFLAG_EOS) != EB_BUFFERFLAG_EOS && p_buffer->pts == 3) {
        p_buffer->flags              = EB_BUFFERFLAG_EOS;
        p_buffer->pic_type           = EB_AV1_INVALID_PICTURE;
        enc_handle_ptr->eos_received = 1;
        return_val                   = EB_ErrorBadParameter;
        SVT_ERROR(
            "AVIF flag is specified, but more than 3 frames were sent. This will not produce an AVIF image sequence "
            "(avis)!\n");
    }

    // Exit the library if we detect an invalid API input buffer @ the previous library call
    if (enc_handle_ptr->is_prev_valid == false) {
        p_buffer->flags              = EB_BUFFERFLAG_EOS;
        p_buffer->pic_type           = EB_AV1_INVALID_PICTURE;
        enc_handle_ptr->eos_received = 1;
        return_val                   = EB_ErrorBadParameter;
        SVT_ERROR("Invalid API input buffer size detected. Please ignore the output stream\n");
    }

    // Get new Luma-8b buffer & a new (Chroma-8b + Luma-Chroma-2bit) buffers; Lib will release once done.
    EbObjectWrapper* y8b_wrapper;
    svt_get_empty_object(enc_handle_ptr->input_y8b_buffer_producer_fifo_ptr, &y8b_wrapper);
    // Update the input picture definitions: resolution of the sequence
    if (validate_on_the_fly_settings(p_buffer, scs, enc_handle_ptr->scs_instance->config_mutex)) {
        return_val                   = EB_ErrorBadParameter;
        enc_handle_ptr->eos_received = 1;
    }
    // if resolution has changed, and the y8b_wrapper settings do not match scs settings, update y8b_wrapper settings
    if (buffer_update_needed((EbBufferHeaderType*)y8b_wrapper->object_ptr, scs)) {
        svt_input_y8b_update((EbBufferHeaderType*)y8b_wrapper->object_ptr, scs);
    }
    //set live count to 1 to be decremented at the end of the encode in RC
    svt_object_inc_live_count(y8b_wrapper, 1);

    // svt_object_inc_live_count(y8b_wrapper, 1);

    svt_get_empty_object(enc_handle_ptr->input_buffer_producer_fifo_ptr, &eb_wrapper_ptr);
    // if resolution has changed, and the input_buffer settings do not match scs settings, update input_buffer settings
    if (buffer_update_needed((EbBufferHeaderType*)eb_wrapper_ptr->object_ptr, scs)) {
        svt_input_buffer_header_update((EbBufferHeaderType*)eb_wrapper_ptr->object_ptr, scs, true);
    }

    //set live count to 1 to be decremented at the end of the encode in RC, and released
    //this would also allow low delay TF to retain pictures
    svt_object_inc_live_count(eb_wrapper_ptr, 1);

    enc_handle_ptr->eos_received += p_buffer->flags & EB_BUFFERFLAG_EOS;

    // copy the Luma 8bit part into y8b buffer and the rest of samples into the regular buffer
    EbBufferHeaderType* lib_y8b_hdr = (EbBufferHeaderType*)y8b_wrapper->object_ptr;
    EbBufferHeaderType* lib_reg_hdr = (EbBufferHeaderType*)eb_wrapper_ptr->object_ptr;

    // check whether the n_filled_len has enough samples to be processed
    EbPictureBufferDesc*      input_pic      = (EbPictureBufferDesc*)lib_y8b_hdr->p_buffer;
    EbSvtAv1EncConfiguration* config         = &scs->static_config;
    bool                      is_16bit_input = config->encoder_bit_depth > EB_EIGHT_BIT;

    const uint8_t subsampling_x = (config->encoder_color_format == EB_YUV444 ? 0 : 1);
    const uint8_t subsampling_y =
        ((config->encoder_color_format == EB_YUV444 || config->encoder_color_format == EB_YUV422) ? 0 : 1);
    const size_t luma_width    = input_pic->width - scs->max_input_pad_right;
    const size_t luma_height   = input_pic->height - scs->max_input_pad_bottom;
    const size_t chroma_width  = (luma_width + subsampling_x) >> subsampling_x;
    const size_t chroma_height = (luma_height + subsampling_y) >> subsampling_y;
    const size_t read_size     = (luma_width * luma_height + 2 * chroma_width * chroma_height) << is_16bit_input;

    if (app_hdr->p_buffer != NULL && read_size > app_hdr->n_filled_len) {
        // memset the library input buffer(s) if the API input buffer is not large enough
        // this operation is necessary to avoid a potential crash when processing an invalid input
        // the library will still process the current input and then exit
        memset_input_buffer(scs, lib_reg_hdr, lib_y8b_hdr, app_hdr, 0);
        enc_handle_ptr->is_prev_valid = false;
    } else {
        copy_input_buffer(scs, lib_reg_hdr, lib_y8b_hdr, app_hdr, 0);
    }

    //Take a new App-RessCoord command
    EbObjectWrapper* input_cmd_wrp;
    svt_get_empty_object(enc_handle_ptr->input_cmd_producer_fifo_ptr, &input_cmd_wrp);
    InputCommand* input_cmd_obj = (InputCommand*)input_cmd_wrp->object_ptr;
    //Fill the command with two picture buffers
    input_cmd_obj->eb_input_wrapper_ptr = eb_wrapper_ptr;
    input_cmd_obj->y8b_wrapper          = y8b_wrapper;
    //Send to Lib
    svt_post_full_object(input_cmd_wrp);
    return return_val;
}

static void copy_output_recon_buffer(EbBufferHeaderType* dst, EbBufferHeaderType* src) {
    // copy output Bitstream fileds
    dst->size          = src->size;
    dst->n_alloc_len   = src->n_alloc_len;
    dst->n_filled_len  = src->n_filled_len;
    dst->p_app_private = src->p_app_private;
    dst->n_tick_count  = src->n_tick_count;
    dst->pts           = src->pts;
    dst->dts           = src->dts;
    dst->flags         = src->flags;
    dst->pic_type      = src->pic_type;

    // Copy the metadata array
    if (svt_aom_copy_metadata_buffer(dst, src->metadata) != EB_ErrorNone) {
        dst->metadata = NULL;
    }

    // Copy the picture buffer
    if (src->p_buffer) {
        svt_memcpy(dst->p_buffer, src->p_buffer, src->n_filled_len);
    }

    return;
}

/**********************************
* svt_av1_enc_get_packet sends out packet
**********************************/
EB_API EbErrorType svt_av1_enc_get_packet(EbComponentType* svt_enc_component, EbBufferHeaderType** p_buffer,
                                          unsigned char pic_send_done) {
    EbErrorType                     return_error   = EB_ErrorNone;
    EbEncHandle*                    enc_handle     = (EbEncHandle*)svt_enc_component->p_component_private;
    EbObjectWrapper*                eb_wrapper_ptr = NULL;
    EbBufferHeaderType*             packet;
    const EbSvtAv1EncConfiguration* cfg = &enc_handle->scs_instance->scs->static_config;

    // check if the user is claiming that the last picture has been sent
    // without actually signalling it through svt_av1_enc_send_picture()
    assert(!(!enc_handle->eos_received && pic_send_done));

    // if we have already sent out an EOS, then the user should not be calling
    // this function again, as it will just block inside svt_get_full_object()
    if (enc_handle->eos_sent) {
        *p_buffer = NULL;
        return EB_NoErrorEmptyQueue;
    }

    if (pic_send_done || cfg->pred_structure == LOW_DELAY) {
        svt_get_full_object(enc_handle->output_stream_buffer_consumer_fifo_ptr, &eb_wrapper_ptr);
    } else {
        svt_get_full_object_non_blocking(enc_handle->output_stream_buffer_consumer_fifo_ptr, &eb_wrapper_ptr);
    }

    if (eb_wrapper_ptr) {
        packet = (EbBufferHeaderType*)eb_wrapper_ptr->object_ptr;
        if (packet->flags & 0xfffffff0) {
            return_error = EB_ErrorMax;
        }
        // return the output stream buffer
        *p_buffer = packet;

        // check if we have reached the end of the output stream
        enc_handle->eos_sent += packet->flags & EB_BUFFERFLAG_EOS;

        // save the wrapper pointer for the release
        (*p_buffer)->wrapper_ptr = (void*)eb_wrapper_ptr;
    } else {
        return_error = EB_NoErrorEmptyQueue;
    }
    return return_error;
}

EB_API void svt_av1_enc_release_out_buffer(EbBufferHeaderType** p_buffer) {
    if (p_buffer && (*p_buffer)->wrapper_ptr) {
        if ((*p_buffer)->p_buffer) {
            EB_FREE((*p_buffer)->p_buffer);
        }
        // Release out put buffer back into the pool
        svt_release_object((EbObjectWrapper*)(*p_buffer)->wrapper_ptr);
    }
    return;
}

/**********************************
* Fill This Buffer
**********************************/
EB_API EbErrorType svt_av1_get_recon(EbComponentType* svt_enc_component, EbBufferHeaderType* p_buffer) {
    EbErrorType      return_error   = EB_ErrorNone;
    EbEncHandle*     enc_handle     = (EbEncHandle*)svt_enc_component->p_component_private;
    EbObjectWrapper* eb_wrapper_ptr = NULL;

    if (enc_handle->scs_instance->scs->static_config.recon_enabled) {
        svt_get_full_object_non_blocking(enc_handle->output_recon_buffer_consumer_fifo_ptr, &eb_wrapper_ptr);

        if (eb_wrapper_ptr) {
            EbBufferHeaderType* obj_ptr = (EbBufferHeaderType*)eb_wrapper_ptr->object_ptr;
            copy_output_recon_buffer(p_buffer, obj_ptr);

            if (p_buffer->flags != EB_BUFFERFLAG_EOS && p_buffer->flags != 0) {
                return_error = EB_ErrorMax;
            }
            if (obj_ptr->metadata) {
                svt_metadata_array_free(&obj_ptr->metadata);
            }
            svt_release_object((EbObjectWrapper*)eb_wrapper_ptr);
        } else {
            return_error = EB_NoErrorEmptyQueue;
        }
    } else {
        // recon is not enabled
        return_error = EB_ErrorMax;
    }

    return return_error;
}

/**********************************
* Encoder Error Handling
**********************************/
static void lib_svt_encoder_send_error_exit(EbPtr hComponent, uint32_t error_code) {
    EbComponentType*    svt_enc_component = (EbComponentType*)hComponent;
    EbEncHandle*        enc_handle        = (EbEncHandle*)svt_enc_component->p_component_private;
    EbObjectWrapper*    eb_wrapper_ptr    = NULL;
    EbBufferHeaderType* output_packet;

    svt_get_empty_object(enc_handle->output_stream_buffer_consumer_fifo_ptr, &eb_wrapper_ptr);

    output_packet = (EbBufferHeaderType*)eb_wrapper_ptr->object_ptr;

    output_packet->size     = 0;
    output_packet->flags    = error_code;
    output_packet->p_buffer = NULL;

    svt_post_full_object(eb_wrapper_ptr);
}

EB_API const char* svt_av1_get_version(void) {
    return SVT_AV1_CVS_VERSION;
}

EB_API void svt_av1_print_version(void) {
    SVT_INFO("-------------------------------------------\n");
    SVT_INFO("SVT [version]:\tSVT-AV1 Encoder Lib %s\n", SVT_AV1_CVS_VERSION);
    const char* compiler =
#if defined(__clang__)
        __VERSION__ "\t"
#elif defined(__GNUC__)
        "GCC " __VERSION__ "\t"
#elif defined(_MSC_VER) && (_MSC_VER >= 1930)
        "Visual Studio 2022"
#elif defined(_MSC_VER) && (_MSC_VER >= 1920)
        "Visual Studio 2019"
#elif defined(_MSC_VER) && (_MSC_VER >= 1910)
        "Visual Studio 2017"
#elif defined(_MSC_VER) && (_MSC_VER >= 1900)
        "Visual Studio 2015"
#elif defined(_MSC_VER)
        "Visual Studio (old)"
#else
        "unknown compiler"
#endif
        ;
    SVT_INFO("SVT [build]  :\t%s %zu bit\n", compiler, sizeof(void*) * 8);
#if !REPRODUCIBLE_BUILDS
    SVT_INFO("LIB Build date: %s %s\n", __DATE__, __TIME__);
#endif
    SVT_INFO("-------------------------------------------\n");
}

/**
 * Set log callback, wrapper around internal function to ensure public functions are stored in one place.
 */
EB_API void svt_av1_set_log_callback(SvtAv1LogCallback callback, void* context) {
#if !CONFIG_LOG_QUIET
    svt_aom_log_set_callback(callback, context);
#else
    UNUSED(callback);
    UNUSED(context);
#endif
}

/**********************************
* Encoder Handle Initialization
**********************************/
static EbErrorType init_svt_av1_encoder_handle(EbComponentType* hComponent) {
    EbErrorType      return_error      = EB_ErrorNone;
    EbComponentType* svt_enc_component = hComponent;
    EbEncHandle*     handle;
    svt_av1_print_version();

    enc_switch_to_real_time();

    // Set Component Size & Version
    svt_enc_component->size = sizeof(EbComponentType);

    EB_NEW(handle, svt_enc_handle_ctor, svt_enc_component);
    svt_enc_component->p_component_private = handle;

    return return_error;
}

static EbErrorType allocate_frame_buffer(SequenceControlSet* scs, EbBufferHeaderType* input_buffer, bool noy8b) {
    EbErrorType                 return_error = EB_ErrorNone;
    EbPictureBufferDescInitData input_pic_buf_desc_init_data;
    EbSvtAv1EncConfiguration*   config   = &scs->static_config;
    uint8_t                     is_16bit = config->encoder_bit_depth > 8 ? 1 : 0;

    input_pic_buf_desc_init_data.max_width = !(scs->max_input_luma_width % 8)
        ? scs->max_input_luma_width
        : scs->max_input_luma_width + (scs->max_input_luma_width % 8);

    input_pic_buf_desc_init_data.max_height = !(scs->max_input_luma_height % 8)
        ? scs->max_input_luma_height
        : scs->max_input_luma_height + (scs->max_input_luma_height % 8);

    input_pic_buf_desc_init_data.bit_depth    = (EbBitDepth)config->encoder_bit_depth;
    input_pic_buf_desc_init_data.color_format = (EbColorFormat)config->encoder_color_format;

    input_pic_buf_desc_init_data.left_padding  = scs->left_padding;
    input_pic_buf_desc_init_data.right_padding = scs->right_padding;
    input_pic_buf_desc_init_data.top_padding   = scs->top_padding;
    input_pic_buf_desc_init_data.bot_padding   = scs->bot_padding;

    input_pic_buf_desc_init_data.split_mode = is_16bit ? true : false;

    input_pic_buf_desc_init_data.buffer_enable_mask = PICTURE_BUFFER_DESC_FULL_MASK;
    input_pic_buf_desc_init_data.is_16bit_pipeline  = 0;

    // Enhanced Picture Buffer
    {
        EbPictureBufferDesc* buf;
        if (!noy8b) {
            EB_NEW(buf, svt_picture_buffer_desc_ctor, (EbPtr)&input_pic_buf_desc_init_data);
        } else {
            EB_NEW(buf, svt_picture_buffer_desc_ctor_noy8b, (EbPtr)&input_pic_buf_desc_init_data);
        }
        input_buffer->p_buffer = (uint8_t*)buf;
    }

    return return_error;
}

/*
  allocate an input sample Luma-8bit buffer
*/
static EbErrorType allocate_y8b_frame_buffer(SequenceControlSet* scs, EbBufferHeaderType* input_buffer) {
    EbErrorType                 return_error = EB_ErrorNone;
    EbPictureBufferDescInitData input_pic_buf_desc_init_data;
    EbSvtAv1EncConfiguration*   config   = &scs->static_config;
    uint8_t                     is_16bit = 0;

    input_pic_buf_desc_init_data.max_width = !(scs->max_input_luma_width % 8)
        ? scs->max_input_luma_width
        : scs->max_input_luma_width + (scs->max_input_luma_width % 8);

    input_pic_buf_desc_init_data.max_height   = !(scs->max_input_luma_height % 8)
          ? scs->max_input_luma_height
          : scs->max_input_luma_height + (scs->max_input_luma_height % 8);
    input_pic_buf_desc_init_data.bit_depth    = EB_EIGHT_BIT;
    input_pic_buf_desc_init_data.color_format = (EbColorFormat)config->encoder_color_format;

    input_pic_buf_desc_init_data.left_padding  = scs->left_padding;
    input_pic_buf_desc_init_data.right_padding = scs->right_padding;
    input_pic_buf_desc_init_data.top_padding   = scs->top_padding;
    input_pic_buf_desc_init_data.bot_padding   = scs->bot_padding;

    input_pic_buf_desc_init_data.split_mode = is_16bit ? true : false;

    input_pic_buf_desc_init_data.buffer_enable_mask = PICTURE_BUFFER_DESC_LUMA_MASK; //allocate for 8bit Luma only
    input_pic_buf_desc_init_data.is_16bit_pipeline  = 0;

    // Enhanced Picture Buffer
    {
        EbPictureBufferDesc* buf;
        EB_NEW(buf, svt_picture_buffer_desc_ctor, (EbPtr)&input_pic_buf_desc_init_data);
        input_buffer->p_buffer = (uint8_t*)buf;
    }

    return return_error;
}

/*
  create a luma 8bit buffer descriptor
*/
EbErrorType svt_input_y8b_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr) {
    EbBufferHeaderType* input_buffer;
    SequenceControlSet* scs = (SequenceControlSet*)object_init_data_ptr;

    *object_dbl_ptr = NULL;
    EB_CALLOC(input_buffer, 1, sizeof(EbBufferHeaderType));
    *object_dbl_ptr = (EbPtr)input_buffer;
    // Initialize Header
    input_buffer->size = sizeof(EbBufferHeaderType);

    EbErrorType return_error = allocate_y8b_frame_buffer(scs, input_buffer);
    if (return_error != EB_ErrorNone) {
        return return_error;
    }

    input_buffer->p_app_private = NULL;

    return EB_ErrorNone;
}

/*
  free a luma 8bit buffer descriptor
*/
void svt_input_y8b_destroyer(EbPtr p) {
    EbBufferHeaderType*  obj = (EbBufferHeaderType*)p;
    EbPictureBufferDesc* buf = (EbPictureBufferDesc*)obj->p_buffer;
    if (buf) {
        EB_FREE_ALIGNED_ARRAY(buf->buffer_bit_inc_y);
        EB_FREE_ALIGNED_ARRAY(buf->buffer_bit_inc_cb);
        EB_FREE_ALIGNED_ARRAY(buf->buffer_bit_inc_cr);
    }

    EB_DELETE(buf);
    EB_FREE(obj);
}

/**************************************
* EbBufferHeaderType Constructor
**************************************/
EbErrorType svt_input_buffer_header_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr) {
    EbBufferHeaderType* input_buffer;
    SequenceControlSet* scs = (SequenceControlSet*)object_init_data_ptr;

    *object_dbl_ptr = NULL;
    EB_CALLOC(input_buffer, 1, sizeof(EbBufferHeaderType));
    *object_dbl_ptr = (EbPtr)input_buffer;
    // Initialize Header
    input_buffer->size = sizeof(EbBufferHeaderType);

    EbErrorType return_error = allocate_frame_buffer(scs, input_buffer, true);
    if (return_error != EB_ErrorNone) {
        return return_error;
    }

    input_buffer->p_app_private = NULL;

    return EB_ErrorNone;
}

void svt_input_buffer_header_destroyer(EbPtr p) {
    EbBufferHeaderType*  obj = (EbBufferHeaderType*)p;
    EbPictureBufferDesc* buf = (EbPictureBufferDesc*)obj->p_buffer;
    if (buf) {
        EB_FREE_ALIGNED_ARRAY(buf->buffer_bit_inc_y);
        EB_FREE_ALIGNED_ARRAY(buf->buffer_bit_inc_cb);
        EB_FREE_ALIGNED_ARRAY(buf->buffer_bit_inc_cr);
    }

    EB_DELETE(buf);
    EB_FREE(obj);
}

EbErrorType svt_overlay_buffer_header_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr) {
    EbBufferHeaderType* input_buffer;
    SequenceControlSet* scs = (SequenceControlSet*)object_init_data_ptr;

    *object_dbl_ptr = NULL;
    EB_CALLOC(input_buffer, 1, sizeof(EbBufferHeaderType));
    *object_dbl_ptr = (EbPtr)input_buffer;
    // Initialize Header
    input_buffer->size = sizeof(EbBufferHeaderType);

    EbErrorType return_error = allocate_frame_buffer(scs, input_buffer, false);
    if (return_error != EB_ErrorNone) {
        return return_error;
    }

    input_buffer->p_app_private = NULL;

    return EB_ErrorNone;
}

/**************************************
* EbBufferHeaderType Constructor
**************************************/
EbErrorType svt_output_buffer_header_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr) {
    (void)object_init_data_ptr;
    EbBufferHeaderType* out_buf_ptr;

    *object_dbl_ptr = NULL;
    EB_CALLOC(out_buf_ptr, 1, sizeof(EbBufferHeaderType));
    *object_dbl_ptr = (EbPtr)out_buf_ptr;

    // Initialize Header
    out_buf_ptr->size = sizeof(EbBufferHeaderType);
    // p_buffer and n_alloc_len are dynamically set in EbPacketizationProcess
    // out_buf_ptr->n_alloc_len;
    out_buf_ptr->p_app_private = NULL;

    return EB_ErrorNone;
}

void svt_output_buffer_header_destroyer(EbPtr p) {
    EbBufferHeaderType* obj = (EbBufferHeaderType*)p;
    EB_FREE(obj);
}

/**************************************
* EbBufferHeaderType Constructor
**************************************/
EbErrorType svt_output_recon_buffer_header_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr) {
    EbBufferHeaderType* recon_buffer;
    SequenceControlSet* scs       = (SequenceControlSet*)object_init_data_ptr;
    const uint32_t      luma_size = scs->seq_header.max_frame_width * scs->seq_header.max_frame_height;
    // both u and v
    const uint32_t chroma_size = luma_size >> 1;
    const uint32_t ten_bit     = (scs->static_config.encoder_bit_depth > 8);
    const uint32_t frame_size  = (luma_size + chroma_size) << ten_bit;

    *object_dbl_ptr = NULL;
    EB_CALLOC(recon_buffer, 1, sizeof(EbBufferHeaderType));
    *object_dbl_ptr = (EbPtr)recon_buffer;

    // Initialize Header
    recon_buffer->size = sizeof(EbBufferHeaderType);

    // Assign the variables
    EB_MALLOC(recon_buffer->p_buffer, frame_size);

    recon_buffer->n_alloc_len   = frame_size;
    recon_buffer->p_app_private = NULL;

    return EB_ErrorNone;
}

void svt_output_recon_buffer_header_destroyer(EbPtr p) {
    EbBufferHeaderType* obj = (EbBufferHeaderType*)p;
    EB_FREE(obj->p_buffer);
    EB_FREE(obj);
}

/**********************************
* svt_av1_enc_get_stream_info get stream information from encoder
**********************************/
EB_API EbErrorType svt_av1_enc_get_stream_info(EbComponentType* svt_enc_component, uint32_t stream_info_id,
                                               void* info) {
    if (stream_info_id >= SVT_AV1_STREAM_INFO_END || stream_info_id < SVT_AV1_STREAM_INFO_START) {
        return EB_ErrorBadParameter;
    }
    EbEncHandle*    enc_handle       = svt_enc_component->p_component_private;
    EncodeContext*  context          = enc_handle->scs_instance->enc_ctx;
    SvtAv1FixedBuf* first_pass_stats = info;
    first_pass_stats->buf            = context->stats_out.stat;
    first_pass_stats->sz             = context->stats_out.size * sizeof(FIRSTPASS_STATS);
    return EB_ErrorNone;
}
