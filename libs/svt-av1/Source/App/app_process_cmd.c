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

/***************************************
 * Includes
 ***************************************/
#include <stdio.h>
#include <stdlib.h>
#include <stdbool.h>
#include <string.h>
#include <math.h>
#include <ctype.h>
#include "app_context.h"
#include "app_config.h"
#include "EbSvtAv1ErrorCodes.h"
#include "app_input_y4m.h"
#include "svt_time.h"

#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <io.h>
#else
#include <sys/mman.h>
#endif

#include "app_output_ivf.h"

/***************************************
 * Macros
 ***************************************/
#define CLIP3(min_val, max_val, a) (((a) < (min_val)) ? (min_val) : (((a) > (max_val)) ? (max_val) : (a)))
#define YUV4MPEG2_IND_SIZE 9
extern volatile int32_t keep_running;

/***************************************
* Process Error Log
***************************************/
void log_error_output(FILE* error_log_file, uint32_t error_code) {
    switch (error_code) {
        // EB_ENC_AMVP_ERRORS:

    case EB_ENC_CL_ERROR2:
        fprintf(error_log_file, "Error: Unknown coding mode!\n");
        break;

    case EB_ENC_EC_ERROR2:
        fprintf(error_log_file, "Error: copy_payload: output buffer too small!\n");
        break;

    case EB_ENC_EC_ERROR29:
        fprintf(error_log_file, "Error: No more than 6 SAO types\n");
        break;

        // EB_ENC_ERRORS:
    case EB_ENC_ROB_OF_ERROR:
        fprintf(error_log_file, "Error: Recon Output Buffer Overflow!\n");
        break;

    case EB_ENC_RC_ERROR2:
        fprintf(error_log_file, "Error: RateControlProcess: No RC interval found!\n");
        break;

        // EB_ENC_PM_ERRORS:
    case EB_ENC_PM_ERROR10:
        fprintf(error_log_file, "Error: svt_aom_picture_manager_kernel: ref_entry should never be null!\n");
        break;

    case EB_ENC_PM_ERROR4:
        fprintf(error_log_file, "Error: PictureManagerProcess: Empty input queue!\n");
        break;

    case EB_ENC_PM_ERROR5:
        fprintf(error_log_file, "Error: PictureManagerProcess: Empty reference queue!\n");
        break;

    case EB_ENC_PM_ERROR6:
        fprintf(error_log_file,
                "Error: PictureManagerProcess: The capped elaspedNonIdrCount must be larger than "
                "the maximum supported delta ref poc!\n");
        break;

    case EB_ENC_PM_ERROR7:
        fprintf(error_log_file, "Error: PictureManagerProcess: Reference Picture Queue Full!\n");
        break;

    case EB_ENC_PM_ERROR8:
        fprintf(error_log_file,
                "Error: PictureManagerProcess: No reference match found - this will lead to a "
                "memory leak!\n");
        break;

    case EB_ENC_PM_ERROR9:
        fprintf(error_log_file, "Error: PictureManagerProcess: Unknown picture type!\n");

        break;
        // picture decision Errors
    case EB_ENC_PD_ERROR8:
        fprintf(error_log_file, "Error: PictureDecisionProcess: Picture Decision Reorder Queue overflow\n");
        break;

    default:
        fprintf(error_log_file, "Error: Others!\n");
        break;
    }

    return;
}

static void (*read_input)(EbConfig* app_cfg, uint8_t is_16bit, EbBufferHeaderType* header_ptr);

/* returns a RAM address from a memory mapped file  */
static void* svt_mmap(MemMapFile* h, size_t offset, size_t size) {
    if (offset + size > h->file_size) {
        return NULL;
    }

    // align our mapping to the page size
    const size_t align = offset & h->align_mask;
    offset -= align;
    size += align;
#ifdef _WIN32
    // split offset into high and low 32 bits
    const DWORD offset_high = (DWORD)(offset >> 32);
    const DWORD offset_low  = (DWORD)(offset & 0xFFFFFFFF);
    uint8_t*    base        = MapViewOfFile(h->map_handle, FILE_MAP_READ, offset_high, offset_low, size);
    if (base) {
        return base + align;
    }
#else
    uint8_t* base = mmap(NULL, size, PROT_READ, MAP_PRIVATE, h->fd, offset);
    if (base != MAP_FAILED) {
        return base + align;
    }
#endif
    return NULL;
}

/* release  memory mapped file  */
static void svt_munmap(MemMapFile* h, void* addr, int64_t size) {
    void* base = (void*)((intptr_t)addr & ~h->align_mask);
#ifdef _WIN32
    (void)size;
    UnmapViewOfFile(base);
#else
    munmap(base, size + (intptr_t)addr - (intptr_t)base);
#endif
}

/* release  memory mapped file  */
static void release_memory_mapped_file(EbConfig* app_cfg, uint8_t is_16bit, EbBufferHeaderType* header_ptr) {
    const uint32_t input_padded_width  = app_cfg->input_padded_width;
    const uint32_t input_padded_height = app_cfg->input_padded_height;
    uint64_t       luma_read_size      = (uint64_t)input_padded_width * input_padded_height << is_16bit;
    const uint8_t  color_format        = app_cfg->config.encoder_color_format;
    EbSvtIOFormat* input_ptr           = (EbSvtIOFormat*)header_ptr->p_buffer;
    svt_munmap(&app_cfg->mmap, input_ptr->luma, luma_read_size);
    svt_munmap(&app_cfg->mmap, input_ptr->cb, luma_read_size >> (3 - color_format));
    svt_munmap(&app_cfg->mmap, input_ptr->cr, luma_read_size >> (3 - color_format));
}

/**
 * Reads and extracts one qp from the qp_file
 * @param qp_file file to read a value from
 * @param qp_read_from_file boolean value to check if a value was read
 * @return long value of qp. -1 is returned if eof or eol is reached without reading anything.
 * A 0 may also be returned if a line starting with '#', '/', or '-' is found
 */
static long get_next_qp_from_qp_file(FILE* const qp_file, int* const qp_read_from_file) {
    long qp = 0;
    char line[512], *pos = line;
    // Read single line until \n
    if (!fgets(line, 512, qp_file)) {
        // eof
        return -1;
    }
    // Clear out beginning spaces
    while (isspace(*pos)) {
        ++pos;
    }
    if (!*pos) {
        // eol
        return -1;
    }
    switch (*pos) {
    case '#':
    case '/':
    case '-':
        return 0;
    }
    if (isdigit(*pos)) {
        qp = strtol(pos, NULL, 0);
    }
    if (qp > 0) {
        *qp_read_from_file = 1;
    }
    return qp;
}

static unsigned char send_qp_on_the_fly(FILE* const qp_file, bool* use_qp_file) {
    long tmp_qp            = 0;
    int  qp_read_from_file = 0;

    while (tmp_qp == 0 || (tmp_qp == -1 && qp_read_from_file)) {
        // get next qp
        tmp_qp = get_next_qp_from_qp_file(qp_file, &qp_read_from_file);
    }

    if (tmp_qp == -1) {
        *use_qp_file = false;
        fprintf(stderr, "\nWarning: QP File did not contain any valid QPs");
    }
    return (unsigned)CLIP3(0, 63, tmp_qp);
}

static void injector(uint64_t processed_frame_count, uint32_t injector_frame_rate) {
    static uint64_t start_times_seconds;
    static uint64_t start_timesu_seconds;
    static int      first_time = 0;

    if (first_time == 0) {
        first_time = 1;
        app_svt_av1_get_time(&start_times_seconds, &start_timesu_seconds);
    } else {
        uint64_t current_times_seconds, current_timesu_seconds;
        app_svt_av1_get_time(&current_times_seconds, &current_timesu_seconds);
        const double elapsed_time = app_svt_av1_compute_overall_elapsed_time(
            start_times_seconds, start_timesu_seconds, current_times_seconds, current_timesu_seconds);
        const int    buffer_frames     = 0; // How far ahead of time should we let it get
        const double injector_interval = (double)(1 << 16) / injector_frame_rate; // 1.0 / injector frame rate (in this
        // case, 1.0/encodRate)
        const double predicted_time  = (processed_frame_count - buffer_frames) * injector_interval;
        const int    milli_sec_ahead = (int)(1000 * (predicted_time - elapsed_time));
        if (milli_sec_ahead > 0) {
            app_svt_av1_sleep(milli_sec_ahead);
        }
    }
}

static bool is_forced_keyframe(const EbConfig* app_cfg, uint64_t pts) {
    if (app_cfg->forced_keyframes.frames) {
        for (size_t i = 0; i < app_cfg->forced_keyframes.count; ++i) {
            if (app_cfg->forced_keyframes.frames[i] == pts) {
                return true;
            }
            if (app_cfg->forced_keyframes.frames[i] > pts) {
                break;
            }
        }
    }
    return false;
}

bool process_skip(EbConfig* app_cfg, EbBufferHeaderType* header_ptr) {
    const bool is_16bit = app_cfg->config.encoder_bit_depth > 8;
    for (int64_t i = 0; i < app_cfg->frames_to_be_skipped; i++) {
        read_input(app_cfg, is_16bit, header_ptr);

        if (header_ptr->n_filled_len) {
            app_cfg->mmap.file_frame_it++;
            if (app_cfg->mmap.enable) {
                release_memory_mapped_file(app_cfg, is_16bit, header_ptr);
            }
        } else {
            return false;
        }
    }
    app_cfg->need_to_skip = false;
    return true;
}
#if FTR_RATE_ON_FLY_SAMPLE
// test_update_rate_info: sample test case for updating the rate info on the fly
static EbErrorType test_update_rate_info(uint64_t pic_num, EbBufferHeaderType* header_ptr) {
    SvtAv1RateInfo* data;
    int             interval = 180;
    if (pic_num == 0) {
        return EB_ErrorNone;
    } else if (pic_num % (5 * interval) == 0) {
        data = (SvtAv1RateInfo*)malloc(sizeof(SvtAv1RateInfo));
        memset(data, 0, sizeof(SvtAv1RateInfo));
        data->target_bit_rate = 2000;
    } else if (pic_num % (4 * interval) == 0) {
        data = (SvtAv1RateInfo*)malloc(sizeof(SvtAv1RateInfo));
        memset(data, 0, sizeof(SvtAv1RateInfo));
        data->target_bit_rate = 300;
    } else if (pic_num % (3 * interval) == 0) {
        data = (SvtAv1RateInfo*)malloc(sizeof(SvtAv1RateInfo));
        memset(data, 0, sizeof(SvtAv1RateInfo));
        data->target_bit_rate = 500;
    } else if (pic_num % (2 * interval) == 0) {
        data = (SvtAv1RateInfo*)malloc(sizeof(SvtAv1RateInfo));
        memset(data, 0, sizeof(SvtAv1RateInfo));
        data->target_bit_rate = 1000;
    } else if (pic_num % interval == 0) {
        data = (SvtAv1RateInfo*)malloc(sizeof(SvtAv1RateInfo));
        memset(data, 0, sizeof(SvtAv1RateInfo));
        data->target_bit_rate = 200;
    } else {
        return EB_ErrorNone;
    }
    data->target_bit_rate *= 1000;
    EbPrivDataNode* new_node = (EbPrivDataNode*)malloc(sizeof(EbPrivDataNode));
    new_node->size           = sizeof(SvtAv1RateInfo);
    new_node->node_type      = RATE_CHANGE_EVENT;
    new_node->data           = data;
    new_node->next           = NULL;

    // append to tail
    if (header_ptr->p_app_private == NULL) {
        header_ptr->p_app_private = new_node;
    } else {
        EbPrivDataNode* last = header_ptr->p_app_private;
        while (last->next != NULL) {
            last = last->next;
        }
        last->next = new_node;
    }

    return EB_ErrorNone;
}

// test_update_rate_info: sample test case for updating the QP on the fly
static EbErrorType test_update_qp_info(uint64_t pic_num, EbBufferHeaderType* header_ptr) {
    SvtAv1RateInfo* data;
    int             interval = 120;
    if (pic_num == 0) {
        return EB_ErrorNone;
    } else if (pic_num % (5 * interval) == 0) {
        data = (SvtAv1RateInfo*)malloc(sizeof(SvtAv1RateInfo));
        memset(data, 0, sizeof(SvtAv1RateInfo));
        data->seq_qp         = 20;
        header_ptr->pic_type = EB_AV1_KEY_PICTURE;
    } else if (pic_num % (4 * interval) == 0) {
        data = (SvtAv1RateInfo*)malloc(sizeof(SvtAv1RateInfo));
        memset(data, 0, sizeof(SvtAv1RateInfo));
        data->seq_qp         = 25;
        header_ptr->pic_type = EB_AV1_KEY_PICTURE;
    } else if (pic_num % (3 * interval) == 0) {
        data = (SvtAv1RateInfo*)malloc(sizeof(SvtAv1RateInfo));
        memset(data, 0, sizeof(SvtAv1RateInfo));
        data->seq_qp         = 45;
        header_ptr->pic_type = EB_AV1_KEY_PICTURE;
    } else if (pic_num % (2 * interval) == 0) {
        data = (SvtAv1RateInfo*)malloc(sizeof(SvtAv1RateInfo));
        memset(data, 0, sizeof(SvtAv1RateInfo));
        data->seq_qp         = 55;
        header_ptr->pic_type = EB_AV1_KEY_PICTURE;
    } else if (pic_num % interval == 0) {
        data = (SvtAv1RateInfo*)malloc(sizeof(SvtAv1RateInfo));
        memset(data, 0, sizeof(SvtAv1RateInfo));
        data->seq_qp         = 63;
        header_ptr->pic_type = EB_AV1_KEY_PICTURE;
    } else {
        return EB_ErrorNone;
    }
    EbPrivDataNode* new_node = (EbPrivDataNode*)malloc(sizeof(EbPrivDataNode));
    new_node->size           = sizeof(SvtAv1RateInfo);
    new_node->node_type      = RATE_CHANGE_EVENT;
    new_node->data           = data;
    new_node->next           = NULL;

    // append to tail
    if (header_ptr->p_app_private == NULL) {
        header_ptr->p_app_private = new_node;
    } else {
        EbPrivDataNode* last = header_ptr->p_app_private;
        while (last->next != NULL) {
            last = last->next;
        }
        last->next = new_node;
    }

    return EB_ErrorNone;
}
#endif
#if FTR_FRAME_RATE_ON_FLY_SAMPLE
// test_update_frame_rate_info: sample test case for updating the rate info on the fly
static EbErrorType test_update_frame_rate_info(uint64_t pic_num, EbBufferHeaderType* header_ptr) {
    SvtAv1FrameRateInfo* data;
    int                  interval = 500;
    if (pic_num == 0) {
        return EB_ErrorNone;
    } else if (pic_num % (5 * interval) == 0) {
        data = (SvtAv1FrameRateInfo*)malloc(sizeof(SvtAv1FrameRateInfo));
        memset(data, 0, sizeof(SvtAv1FrameRateInfo));
        data->frame_rate_numerator   = 24000;
        data->frame_rate_denominator = 1000;
    } else if (pic_num % (4 * interval) == 0) {
        data = (SvtAv1FrameRateInfo*)malloc(sizeof(SvtAv1FrameRateInfo));
        memset(data, 0, sizeof(SvtAv1FrameRateInfo));
        data->frame_rate_numerator   = 30000;
        data->frame_rate_denominator = 1000;
    } else if (pic_num % (3 * interval) == 0) {
        data = (SvtAv1FrameRateInfo*)malloc(sizeof(SvtAv1FrameRateInfo));
        memset(data, 0, sizeof(SvtAv1FrameRateInfo));
        data->frame_rate_numerator   = 15000;
        data->frame_rate_denominator = 1000;
    } else if (pic_num % (2 * interval) == 0) {
        data = (SvtAv1FrameRateInfo*)malloc(sizeof(SvtAv1FrameRateInfo));
        memset(data, 0, sizeof(SvtAv1FrameRateInfo));
        data->frame_rate_numerator   = 60000;
        data->frame_rate_denominator = 1000;
    } else if (pic_num % interval == 0) {
        data = (SvtAv1FrameRateInfo*)malloc(sizeof(SvtAv1FrameRateInfo));
        memset(data, 0, sizeof(SvtAv1FrameRateInfo));
        data->frame_rate_numerator   = 30000;
        data->frame_rate_denominator = 1000;
    } else {
        return EB_ErrorNone;
    }
    EbPrivDataNode* new_node = (EbPrivDataNode*)malloc(sizeof(EbPrivDataNode));
    new_node->size           = sizeof(SvtAv1FrameRateInfo);
    new_node->node_type      = FRAME_RATE_CHANGE_EVENT;
    new_node->data           = data;
    new_node->next           = NULL;

    // append to tail
    if (header_ptr->p_app_private == NULL) {
        header_ptr->p_app_private = new_node;
    } else {
        EbPrivDataNode* last = header_ptr->p_app_private;
        while (last->next != NULL) {
            last = last->next;
        }
        last->next = new_node;
    }

    return EB_ErrorNone;
}
#endif
#if FTR_PER_FRAME_QUALITY_SAMPLE
// test_update_psnr_per_frame_info: sample test case for computing PSNR per frame
static EbErrorType test_update_psnr_per_frame_info(uint64_t pic_num, EbBufferHeaderType* header_ptr) {
    int interval = 10;
    if (pic_num % (1 * interval) != 0) {
        return EB_ErrorNone;
    }
    SvtAv1ComputeQualityInfo* data = (SvtAv1ComputeQualityInfo*)malloc(sizeof(SvtAv1ComputeQualityInfo));
    data->compute_psnr             = true;
    data->compute_ssim             = false;

    EbPrivDataNode* new_node = (EbPrivDataNode*)malloc(sizeof(EbPrivDataNode));
    new_node->size           = sizeof(SvtAv1ComputeQualityInfo);
    new_node->node_type      = COMPUTE_QUALITY_EVENT;
    new_node->data           = data;
    new_node->next           = NULL;

    // append to tail
    if (header_ptr->p_app_private == NULL) {
        header_ptr->p_app_private = new_node;
    } else {
        EbPrivDataNode* last = header_ptr->p_app_private;
        while (last->next != NULL) {
            last = last->next;
        }
        last->next = new_node;
    }

    return EB_ErrorNone;
}
#endif
#if FTR_RES_ON_FLY_SAMPLE
// test_update_rate_info: sample test case for updating the resolution on the fly
static EbErrorType test_update_input_pic_def(uint64_t pic_num, EbBufferHeaderType* header_ptr, EbConfig* app_cfg) {
    SvtAv1InputPicDef* data;
    int                interval = 60;
    if (pic_num == 0) {
        return EB_ErrorNone;
    } else if (pic_num % (5 * interval) == 0) {
        data                    = (SvtAv1InputPicDef*)malloc(sizeof(SvtAv1InputPicDef));
        data->input_luma_height = 360;
        data->input_luma_width  = 640;
        data->input_pad_bottom  = 0;
        data->input_pad_right   = 0;
        header_ptr->pic_type    = EB_AV1_KEY_PICTURE;
    } else if (pic_num % (4 * interval) == 0) {
        data                    = (SvtAv1InputPicDef*)malloc(sizeof(SvtAv1InputPicDef));
        data->input_luma_height = 720;
        data->input_luma_width  = 1280;
        data->input_pad_bottom  = 0;
        data->input_pad_right   = 0;
        header_ptr->pic_type    = EB_AV1_KEY_PICTURE;
    } else if (pic_num % (3 * interval) == 0) {
        data                    = (SvtAv1InputPicDef*)malloc(sizeof(SvtAv1InputPicDef));
        data->input_luma_height = 1080;
        data->input_luma_width  = 1920;
        data->input_pad_bottom  = 0;
        data->input_pad_right   = 0;
        header_ptr->pic_type    = EB_AV1_KEY_PICTURE;
    } else if (pic_num % (2 * interval) == 0) {
        data                    = (SvtAv1InputPicDef*)malloc(sizeof(SvtAv1InputPicDef));
        data->input_luma_height = 720;
        data->input_luma_width  = 1280;
        data->input_pad_bottom  = 0;
        data->input_pad_right   = 0;
        header_ptr->pic_type    = EB_AV1_KEY_PICTURE;
    } else if (pic_num % (1 * interval) == 0) {
        data                    = (SvtAv1InputPicDef*)malloc(sizeof(SvtAv1InputPicDef));
        data->input_luma_height = 360;
        data->input_luma_width  = 640;
        data->input_pad_bottom  = 0;
        data->input_pad_right   = 0;
        header_ptr->pic_type    = EB_AV1_KEY_PICTURE;
    } else {
        return EB_ErrorNone;
    }
    EbPrivDataNode* new_node = (EbPrivDataNode*)malloc(sizeof(EbPrivDataNode));

    new_node->size               = sizeof(SvtAv1InputPicDef);
    new_node->node_type          = RES_CHANGE_EVENT;
    new_node->data               = data;
    new_node->next               = NULL;
    app_cfg->input_padded_width  = data->input_luma_width;
    app_cfg->input_padded_height = data->input_luma_height;

    // append to tail
    if (header_ptr->p_app_private == NULL) {
        header_ptr->p_app_private = new_node;
    } else {
        EbPrivDataNode* last = header_ptr->p_app_private;
        while (last->next != NULL) {
            last = last->next;
        }
        last->next = new_node;
    }

    return EB_ErrorNone;
}
#endif

static EbErrorType retrieve_roi_map_event(SvtAv1RoiMap* roi_map, uint64_t pic_num, EbBufferHeaderType* header_ptr) {
    if (roi_map == NULL || roi_map->evt_list == NULL) {
        return EB_ErrorNone;
    }
    SvtAv1RoiMapEvt* evt = roi_map->cur_evt != NULL ? roi_map->cur_evt->next : roi_map->evt_list;
    if (evt == NULL) {
        return EB_ErrorUndefined;
    }

    if (pic_num != evt->start_picture_number) {
        return EB_ErrorNone;
    } else {
        roi_map->cur_evt = evt;
    }

    EbPrivDataNode* new_node = (EbPrivDataNode*)malloc(sizeof(EbPrivDataNode));
    if (new_node == NULL) {
        return EB_ErrorInsufficientResources;
    }
    SvtAv1RoiMapEvt* data = evt; // shallow copy
    new_node->size        = sizeof(SvtAv1RoiMapEvt*);
    new_node->node_type   = ROI_MAP_EVENT;
    new_node->data        = data;
    new_node->next        = NULL;

    // append to tail
    if (header_ptr->p_app_private == NULL) {
        header_ptr->p_app_private = new_node;
    } else {
        EbPrivDataNode* last = header_ptr->p_app_private;
        while (last->next != NULL) {
            last = last->next;
        }
        last->next = new_node;
    }

    return EB_ErrorNone;
}

static void free_private_data_list(void* node_head) {
    while (node_head) {
        EbPrivDataNode* node = (EbPrivDataNode*)node_head;
        node_head            = node->next;
        if ((node->node_type != PRIVATE_DATA) && (node->node_type != ROI_MAP_EVENT)) {
            if (node->data) {
                free(node->data);
                node->data = NULL;
            }
        }
        free(node);
    };
}

//************************************/
// process_input_buffer
// Reads yuv frames from file and copy
// them into the input buffer
/************************************/

void process_input_buffer(EncChannel* channel) {
    EbConfig*           app_cfg          = channel->app_cfg;
    uint8_t             is_16bit         = (uint8_t)(app_cfg->config.encoder_bit_depth > 8);
    EbBufferHeaderType* header_ptr       = app_cfg->input_buffer_pool;
    EbComponentType*    component_handle = app_cfg->svt_encoder_handle;

    AppExitConditionType return_value = APP_ExitConditionNone;

    const uint64_t frames_to_be_encoded = (uint64_t)app_cfg->frames_to_be_encoded;

    if (channel->exit_cond_input != APP_ExitConditionNone) {
        return;
    }
    if (app_cfg->injector) {
        injector(app_cfg->processed_frame_count, app_cfg->injector_frame_rate);
    }

    if (frames_to_be_encoded != app_cfg->processed_frame_count && app_cfg->stop_encoder == false) {
        header_ptr->p_app_private = NULL;
        header_ptr->pic_type      = EB_AV1_INVALID_PICTURE;
#if FTR_RES_ON_FLY_SAMPLE
        test_update_input_pic_def(app_cfg->processed_frame_count, header_ptr, app_cfg);
#endif
        read_input(app_cfg, is_16bit, header_ptr);

        if (header_ptr->n_filled_len) {
            // Update the context parameters
            app_cfg->processed_byte_count += header_ptr->n_filled_len;
            app_cfg->mmap.file_frame_it++;
            app_cfg->frames_encoded = (int32_t)(++app_cfg->processed_frame_count);

            // Configuration parameters changed on the fly
            if (app_cfg->config.use_qp_file && app_cfg->qp_file) {
                header_ptr->qp = send_qp_on_the_fly(app_cfg->qp_file, &app_cfg->config.use_qp_file);
            }

            if (keep_running == 0 && !app_cfg->stop_encoder) {
                app_cfg->stop_encoder = true;
            }
            // Fill in Buffers Header control data
            header_ptr->pts      = app_cfg->processed_frame_count - 1;
            header_ptr->pic_type = is_forced_keyframe(app_cfg, header_ptr->pts) ? EB_AV1_KEY_PICTURE
                                                                                : header_ptr->pic_type;
            header_ptr->flags    = 0;
            header_ptr->metadata = NULL;
#if FTR_KF_ON_FLY_SAMPLE
            int interval = 19;
            if (header_ptr->pts % (interval) == 0) {
                header_ptr->pic_type = EB_AV1_KEY_PICTURE;
            }
#endif
#if FTR_RATE_ON_FLY_SAMPLE
            test_update_rate_info(header_ptr->pts, header_ptr);
            //  test_update_qp_info(header_ptr->pts, header_ptr);
#endif
#if FTR_FRAME_RATE_ON_FLY_SAMPLE
            test_update_frame_rate_info(header_ptr->pts, header_ptr);
#endif
#if FTR_PER_FRAME_QUALITY_SAMPLE
            test_update_psnr_per_frame_info(header_ptr->pts, header_ptr);
#endif
            retrieve_roi_map_event(app_cfg->roi_map, header_ptr->pts, header_ptr);
            // Send the picture
            if (svt_av1_enc_send_picture(component_handle, header_ptr) != EB_ErrorNone) {
                return_value = APP_ExitConditionFinished;
            }
            // p_app_private is deep copied so it's safe to free it now
            free_private_data_list(header_ptr->p_app_private);

            if (app_cfg->mmap.enable) {
                release_memory_mapped_file(app_cfg, is_16bit, header_ptr);
            }
        }
        if ((app_cfg->processed_frame_count == (uint64_t)app_cfg->frames_to_be_encoded) || app_cfg->stop_encoder) {
            header_ptr->flags = EB_BUFFERFLAG_EOS;
            svt_av1_enc_send_picture(component_handle,
                                     &(EbBufferHeaderType){
                                         .flags    = EB_BUFFERFLAG_EOS,
                                         .pic_type = EB_AV1_INVALID_PICTURE,
                                     });
            return_value = APP_ExitConditionFinished;
        }
    }

    channel->exit_cond_input = return_value;
}

#define SPEED_MEASUREMENT_INTERVAL 2000

double get_psnr(double sse, double max) {
    double psnr;
    if (sse == 0) {
        psnr = 10 * log10(max / 0.1);
    } else {
        psnr = 10 * log10(max / sse);
    }

    return psnr;
}

static void mmap_read_input_frames(EbConfig* app_cfg, uint8_t is_16bit, EbBufferHeaderType* header_ptr) {
    const uint32_t input_padded_width  = app_cfg->input_padded_width;
    const uint32_t input_padded_height = app_cfg->input_padded_height;
    EbSvtIOFormat* input_ptr           = (EbSvtIOFormat*)header_ptr->p_buffer;

    const uint8_t  color_format  = app_cfg->config.encoder_color_format;
    const uint8_t  subsampling_x = (color_format == EB_YUV444 ? 0 : 1);
    const uint8_t  subsampling_y = ((color_format == EB_YUV444 || color_format == EB_YUV422) ? 0 : 1);
    const uint32_t chroma_width  = (app_cfg->input_padded_width + subsampling_x) >> subsampling_x;
    const uint32_t chroma_height = (app_cfg->input_padded_height + subsampling_y) >> subsampling_y;

    input_ptr->y_stride  = input_padded_width;
    input_ptr->cr_stride = chroma_width;
    input_ptr->cb_stride = chroma_width;

    header_ptr->n_filled_len = 0;

    /* if input is a y4m file, read next line which contains "FRAME" */
    if (app_cfg->y4m_input && app_cfg->processed_frame_count == 0 && app_cfg->mmap.file_frame_it == 0) {
        app_cfg->mmap.y4m_frm_hdr = read_y4m_frame_delimiter(app_cfg->input_file, app_cfg->error_log_file);
    }
    size_t luma_read_size   = (size_t)input_padded_width * input_padded_height << is_16bit;
    size_t chroma_read_size = ((size_t)chroma_width * chroma_height << is_16bit);
    size_t read_size        = luma_read_size + 2 * chroma_read_size;

    app_cfg->mmap.cur_offset += app_cfg->mmap.y4m_frm_hdr;
    input_ptr->luma = svt_mmap(&app_cfg->mmap, app_cfg->mmap.cur_offset, luma_read_size);
    header_ptr->n_filled_len += (input_ptr->luma ? (uint32_t)luma_read_size : 0);
    app_cfg->mmap.cur_offset += (input_ptr->luma ? (uint32_t)luma_read_size : 0);

    input_ptr->cb = svt_mmap(&app_cfg->mmap, app_cfg->mmap.cur_offset, chroma_read_size);
    header_ptr->n_filled_len += (input_ptr->cb ? (uint32_t)chroma_read_size : 0);
    app_cfg->mmap.cur_offset += (input_ptr->cb ? chroma_read_size : 0);

    input_ptr->cr = svt_mmap(&app_cfg->mmap, app_cfg->mmap.cur_offset, chroma_read_size);
    header_ptr->n_filled_len += (input_ptr->cr ? (uint32_t)chroma_read_size : 0);
    app_cfg->mmap.cur_offset += (input_ptr->cr ? chroma_read_size : 0);

    if (read_size != header_ptr->n_filled_len) {
        app_cfg->mmap.file_frame_it = 0;
        app_cfg->mmap.cur_offset    = app_cfg->mmap.y4m_seq_hdr;
        ;

        app_cfg->mmap.cur_offset += app_cfg->mmap.y4m_frm_hdr;
        input_ptr->luma = svt_mmap(&app_cfg->mmap, app_cfg->mmap.cur_offset, luma_read_size);
        header_ptr->n_filled_len += (input_ptr->luma ? (uint32_t)luma_read_size : 0);
        app_cfg->mmap.cur_offset += (input_ptr->luma ? (uint32_t)luma_read_size : 0);

        input_ptr->cb = svt_mmap(&app_cfg->mmap, app_cfg->mmap.cur_offset, chroma_read_size);
        header_ptr->n_filled_len += (input_ptr->cb ? (uint32_t)chroma_read_size : 0);
        app_cfg->mmap.cur_offset += (input_ptr->cb ? chroma_read_size : 0);

        input_ptr->cr = svt_mmap(&app_cfg->mmap, app_cfg->mmap.cur_offset, chroma_read_size);
        header_ptr->n_filled_len += (input_ptr->cr ? (uint32_t)chroma_read_size : 0);
        app_cfg->mmap.cur_offset += (input_ptr->cr ? chroma_read_size : 0);
    }
}

static void normal_read_input_frames(EbConfig* app_cfg, uint8_t is_16bit, EbBufferHeaderType* header_ptr) {
    const uint32_t input_padded_width  = app_cfg->input_padded_width;
    const uint32_t input_padded_height = app_cfg->input_padded_height;
    FILE*          input_file          = app_cfg->input_file;
    EbSvtIOFormat* input_ptr           = (EbSvtIOFormat*)header_ptr->p_buffer;

    const uint8_t  color_format  = app_cfg->config.encoder_color_format;
    const uint8_t  subsampling_x = (color_format == EB_YUV444 ? 0 : 1);
    const uint8_t  subsampling_y = ((color_format == EB_YUV444 || color_format == EB_YUV422) ? 0 : 1);
    const uint32_t chroma_width  = (app_cfg->input_padded_width + subsampling_x) >> subsampling_x;
    const uint32_t chroma_height = (app_cfg->input_padded_height + subsampling_y) >> subsampling_y;

    input_ptr->y_stride  = input_padded_width;
    input_ptr->cr_stride = chroma_width;
    input_ptr->cb_stride = chroma_width;

    header_ptr->n_filled_len = 0;

    if (app_cfg->y4m_input) {
        /* if input is a y4m file, read next line which contains "FRAME" */
        read_y4m_frame_delimiter(app_cfg->input_file, app_cfg->error_log_file);
    }
    uint64_t luma_read_size   = (uint64_t)input_padded_width * input_padded_height << is_16bit;
    uint64_t chroma_read_size = chroma_width * chroma_height << is_16bit;
    uint64_t read_size        = luma_read_size + 2 * chroma_read_size;

    uint8_t* eb_input_ptr = input_ptr->luma;
    if (!app_cfg->y4m_input && app_cfg->processed_frame_count == 0 &&
        (app_cfg->input_file == stdin || app_cfg->input_file_is_fifo)) {
        /* 9 bytes were already buffered during the the YUV4MPEG2 header probe */
        memcpy(eb_input_ptr, app_cfg->y4m_buf, YUV4MPEG2_IND_SIZE);
        header_ptr->n_filled_len += YUV4MPEG2_IND_SIZE;
        eb_input_ptr += YUV4MPEG2_IND_SIZE;
        header_ptr->n_filled_len += (uint32_t)fread(eb_input_ptr, 1, luma_read_size - YUV4MPEG2_IND_SIZE, input_file);
    } else {
        header_ptr->n_filled_len += (uint32_t)fread(input_ptr->luma, 1, luma_read_size, input_file);
    }

    header_ptr->n_filled_len += (uint32_t)fread(input_ptr->cb, 1, chroma_read_size, input_file);
    header_ptr->n_filled_len += (uint32_t)fread(input_ptr->cr, 1, chroma_read_size, input_file);

    if (read_size != header_ptr->n_filled_len && !app_cfg->input_file_is_fifo) {
        fseek(input_file, 0, SEEK_SET);
        if (app_cfg->y4m_input == true) {
            read_and_skip_y4m_header(app_cfg->input_file);
            read_y4m_frame_delimiter(app_cfg->input_file, app_cfg->error_log_file);
        }
        header_ptr->n_filled_len = (uint32_t)fread(input_ptr->luma, 1, luma_read_size, input_file);
        header_ptr->n_filled_len += (uint32_t)fread(input_ptr->cb, 1, chroma_read_size, input_file);
        header_ptr->n_filled_len += (uint32_t)fread(input_ptr->cr, 1, chroma_read_size, input_file);
    }

    if (feof(input_file) != 0) {
        if ((input_file == stdin) || (app_cfg->input_file_is_fifo)) {
            //for a fifo, we only know this when we reach eof
            app_cfg->frames_to_be_encoded = app_cfg->frames_encoded;
            if (header_ptr->n_filled_len != read_size) {
                // not a completed frame
                header_ptr->n_filled_len = 0;
            }
        } else {
            // If we reached the end of file, loop over again
            fseek(input_file, 0, SEEK_SET);
        }
    }
}

static void buffered_read_input_frames(EbConfig* app_cfg, uint8_t is_16bit, EbBufferHeaderType* header_ptr) {
    const uint32_t input_padded_width  = app_cfg->input_padded_width;
    const uint32_t input_padded_height = app_cfg->input_padded_height;
    EbSvtIOFormat* input_ptr           = (EbSvtIOFormat*)header_ptr->p_buffer;

    const uint8_t  color_format  = app_cfg->config.encoder_color_format;
    const uint8_t  subsampling_x = (color_format == EB_YUV444 ? 0 : 1);
    const uint8_t  subsampling_y = ((color_format == EB_YUV444 || color_format == EB_YUV422) ? 0 : 1);
    const uint32_t chroma_width  = (app_cfg->input_padded_width + subsampling_x) >> subsampling_x;
    const uint32_t chroma_height = (app_cfg->input_padded_height + subsampling_y) >> subsampling_y;

    input_ptr->y_stride  = input_padded_width;
    input_ptr->cr_stride = chroma_width;
    input_ptr->cb_stride = chroma_width;

    //Normal unpacked mode:yuv420p10le yuv422p10le yuv444p10le
    const size_t luma_size   = (input_padded_width * input_padded_height) << is_16bit;
    const size_t chroma_size = chroma_width * chroma_height << is_16bit;

    uint8_t* base   = app_cfg->sequence_buffer[app_cfg->processed_frame_count % app_cfg->buffered_input];
    input_ptr->luma = base;
    input_ptr->cb   = base + luma_size;
    input_ptr->cr   = base + luma_size + chroma_size;

    header_ptr->n_filled_len = (uint32_t)(luma_size + 2 * chroma_size);
}

void init_reader(EbConfig* app_cfg) {
    if (app_cfg->buffered_input != -1) {
        read_input = buffered_read_input_frames;
    } else if (app_cfg->mmap.enable) {
        read_input = mmap_read_input_frames;
    } else {
        read_input = normal_read_input_frames;
    }
}

/***************************************
* Process Output STATISTICS Buffer
***************************************/
void process_output_statistics_buffer(EbBufferHeaderType* header_ptr, EbConfig* app_cfg) {
    double   max_pix_value = (app_cfg->config.encoder_bit_depth == 8) ? 255 : 1023;
    uint32_t source_width  = app_cfg->config.source_width;
    uint32_t source_height = app_cfg->config.source_height;
    uint32_t luma_size     = source_width * source_height;
    uint32_t chroma_size   = source_width / 2 * source_height / 2;

    double max_sse = max_pix_value * max_pix_value * luma_size;

    double luma_psnr = get_psnr((double)header_ptr->luma_sse, max_sse);

    max_sse = max_pix_value * max_pix_value * chroma_size;

    double cb_psnr = get_psnr((double)header_ptr->cb_sse, max_sse);
    double cr_psnr = get_psnr((double)header_ptr->cr_sse, max_sse);

    EbPerformanceContext* ctx = &app_cfg->performance_context;

    ctx->sum_luma_psnr += luma_psnr;
    ctx->sum_cr_psnr += cr_psnr;
    ctx->sum_cb_psnr += cb_psnr;

    ctx->sum_luma_sse += header_ptr->luma_sse;
    ctx->sum_cr_sse += header_ptr->cr_sse;
    ctx->sum_cb_sse += header_ptr->cb_sse;

    ctx->sum_qp += header_ptr->qp;
    ctx->sum_luma_ssim += header_ptr->luma_ssim;
    ctx->sum_cr_ssim += header_ptr->cr_ssim;
    ctx->sum_cb_ssim += header_ptr->cb_ssim;

    // VBV delays computation
    double frame_duration = 1.0 * app_cfg->config.frame_rate_denominator / app_cfg->config.frame_rate_numerator;
    double bits_sent      = frame_duration * app_cfg->config.target_bit_rate;
    ctx->vbv_buffer_bits  = ctx->vbv_buffer_bits + header_ptr->n_filled_len * 8 - bits_sent;
    if (ctx->vbv_buffer_bits < 0) {
        // no underflows
        ctx->vbv_buffer_bits = 0;
    }
    double vbv_delay_s = ctx->vbv_buffer_bits / app_cfg->config.target_bit_rate;
    if (ctx->vbv_delay_max_s < vbv_delay_s) {
        ctx->vbv_delay_max_s = vbv_delay_s;
    }
    ctx->vbv_delay_sum_s += vbv_delay_s;
    if (vbv_delay_s > 0.5) {
        ctx->vbv_delay_violations++;
    }

    // Write statistic Data to file
    if (app_cfg->stat_file) {
        fprintf(app_cfg->stat_file,
                "Picture Number: %4d\tTL: %4d\t QP: %4u\t Average QP: %4u  [ "
                "PSNR-Y: %.2f dB,\tPSNR-U: %.2f dB,\tPSNR-V: %.2f dB,\t"
                "MSE-Y: %.2f,\tMSE-U: %.2f,\tMSE-V: %.2f,\t"
                "SSIM-Y: %.5f,\tSSIM-U: %.5f,\tSSIM-V: %.5f"
                " ]\t %6d bytes\t VBV delay: %.3f s\n",
                (int)header_ptr->pts,
                (int)header_ptr->temporal_layer_index,
                header_ptr->qp,
                header_ptr->avg_qp,
                luma_psnr,
                cb_psnr,
                cr_psnr,
                (double)header_ptr->luma_sse / luma_size,
                (double)header_ptr->cb_sse / chroma_size,
                (double)header_ptr->cr_sse / chroma_size,
                header_ptr->luma_ssim,
                header_ptr->cb_ssim,
                header_ptr->cr_ssim,
                (int)header_ptr->n_filled_len,
                vbv_delay_s);
        fflush(app_cfg->stat_file);
    }
}

void process_output_stream_buffer(EncChannel* channel, EncApp* enc_app, int32_t* frame_count) {
    EbConfig*            app_cfg    = channel->app_cfg;
    AppPortActiveType*   port_state = &app_cfg->output_stream_port_active;
    EbBufferHeaderType*  header_ptr;
    EbComponentType*     component_handle = app_cfg->svt_encoder_handle;
    AppExitConditionType return_value     = APP_ExitConditionNone;
    // Per channel variables
    FILE* stream_file = app_cfg->bitstream_file;

    uint64_t* total_latency = &app_cfg->performance_context.total_latency;
    uint32_t* max_latency   = &app_cfg->performance_context.max_latency;

    // Local variables
    uint64_t finish_s_time = 0;
    uint64_t finish_u_time = 0;
    uint8_t  is_alt_ref    = 1;
    if (channel->exit_cond_output != APP_ExitConditionNone) {
        return;
    }
    uint8_t pic_send_done = (channel->exit_cond_input == APP_ExitConditionNone) ||
            (channel->exit_cond_recon == APP_ExitConditionNone)
        ? 0
        : 1;
    while (is_alt_ref) {
        is_alt_ref = 0;
        // If we are not in low-delay mode, this is a non-blocking call until all input frames are sent
        EbErrorType stream_status = svt_av1_enc_get_packet(component_handle, &header_ptr, pic_send_done);

        if (stream_status == EB_ErrorMax) {
            fprintf(stderr, "\n");
            log_error_output(app_cfg->error_log_file, header_ptr->flags);
            channel->exit_cond_output = APP_ExitConditionError;
            return;
        } else if (stream_status != EB_NoErrorEmptyQueue) {
            uint32_t flags = header_ptr->flags;

            if (flags & EB_BUFFERFLAG_EOS) {
                // Update Output Port Activity State
                *port_state  = APP_PortInactive;
                return_value = APP_ExitConditionFinished;
                // Release the output buffer
                svt_av1_enc_release_out_buffer(&header_ptr);

                if (app_cfg->config.pass == ENC_FIRST_PASS) {
                    SvtAv1FixedBuf first_pass_stat;
                    EbErrorType    ret = svt_av1_enc_get_stream_info(
                        component_handle, SVT_AV1_STREAM_INFO_FIRST_PASS_STATS_OUT, &first_pass_stat);
                    if (ret == EB_ErrorNone) {
                        if (app_cfg->output_stat_file) {
                            fwrite(first_pass_stat.buf, 1, first_pass_stat.sz, app_cfg->output_stat_file);
                        }
                        enc_app->rc_twopasses_stats.buf = realloc(enc_app->rc_twopasses_stats.buf, first_pass_stat.sz);
                        if (enc_app->rc_twopasses_stats.buf) {
                            memcpy(enc_app->rc_twopasses_stats.buf, first_pass_stat.buf, first_pass_stat.sz);
                            enc_app->rc_twopasses_stats.sz = first_pass_stat.sz;
                        }
                    }
                }
            } else {
                is_alt_ref = (flags & EB_BUFFERFLAG_IS_ALT_REF);
                if (!(flags & EB_BUFFERFLAG_IS_ALT_REF)) {
                    ++(app_cfg->performance_context.frame_count);
                }
                *total_latency += (uint64_t)header_ptr->n_tick_count;
                *max_latency = (header_ptr->n_tick_count > *max_latency) ? header_ptr->n_tick_count : *max_latency;
                app_svt_av1_get_time(&finish_s_time, &finish_u_time);

                // total execution time, inc init time
                app_cfg->performance_context.total_execution_time = app_svt_av1_compute_overall_elapsed_time(
                    app_cfg->performance_context.lib_start_time[0],
                    app_cfg->performance_context.lib_start_time[1],
                    finish_s_time,
                    finish_u_time);

                // total encode time
                app_cfg->performance_context.total_encode_time = app_svt_av1_compute_overall_elapsed_time(
                    app_cfg->performance_context.encode_start_time[0],
                    app_cfg->performance_context.encode_start_time[1],
                    finish_s_time,
                    finish_u_time);

                // Write Stream Data to file
                if (stream_file) {
                    if (app_cfg->performance_context.frame_count == 1 && !(flags & EB_BUFFERFLAG_IS_ALT_REF)) {
                        write_ivf_stream_header(
                            app_cfg, app_cfg->frames_to_be_encoded == -1 ? 0 : (int32_t)app_cfg->frames_to_be_encoded);
                    }
                    write_ivf_frame_header(app_cfg, header_ptr->n_filled_len);
                    fwrite(header_ptr->p_buffer, 1, header_ptr->n_filled_len, stream_file);
                }

                app_cfg->performance_context.byte_count += header_ptr->n_filled_len;

                if (app_cfg->config.stat_report && !(flags & EB_BUFFERFLAG_IS_ALT_REF)) {
                    process_output_statistics_buffer(header_ptr, app_cfg);
                }

                // Update Output Port Activity State
                return_value = APP_ExitConditionNone;
                // Release the output buffer
                svt_av1_enc_release_out_buffer(&header_ptr);

                ++*frame_count;
            }

            const double fps        = (double)*frame_count / app_cfg->performance_context.total_encode_time;
            const double frame_rate = (double)app_cfg->config.frame_rate_numerator /
                (double)app_cfg->config.frame_rate_denominator;

            switch (app_cfg->progress) {
            case 0:
                break;
            case 1:
                if (!(flags & EB_BUFFERFLAG_IS_ALT_REF)) {
                    fprintf(stderr, "\b\b\b\b\b\b\b\b\b%9d", *frame_count);
                }
                break;
            case 2: {
                // Detailed progress variables
                const double ete         = app_cfg->performance_context.total_encode_time;
                const int    ete_r       = (int)round(ete);
                const int    ete_hours   = ete_r / 3600;
                const int    ete_minutes = (ete_r - (ete_hours * 3600)) / 60;
                const int    ete_seconds = ete_r - (ete_hours * 3600) - (ete_minutes * 60);
                const double size        = ((double)app_cfg->performance_context.byte_count / 1000000);

                if ((int)app_cfg->frames_to_be_encoded == -1) {
                    // Encoder doesn't know how many frames are to be encoded, therefore an ETA can't be calculated
                    fprintf(stderr,
                            "\rEncoding: \x1b[33m%4d Frames\x1b[0m @ \x1b[32m%.2f\x1b[0m fp%c | \x1b[35m%.2f "
                            "kb/s\x1b[0m | Size: \x1b[31m%.2f MB\x1b[0m | Time: \x1b[36m%d:%02d:%02d\x1b[0m ",
                            *frame_count,
                            fps >= 1.0 ? fps : fps * 60,
                            fps >= 1.0 ? 's' : 'm',
                            ((double)(app_cfg->performance_context.byte_count << 3) * frame_rate /
                             (app_cfg->frames_encoded * 1000)),
                            size,
                            ete_hours,
                            ete_minutes,
                            ete_seconds);
                } else {
                    const double eta = (app_cfg->performance_context.total_encode_time / *frame_count) *
                        (app_cfg->frames_to_be_encoded - *frame_count);
                    const int    eta_r       = (int)round(eta);
                    const int    eta_hours   = eta_r / 3600;
                    const int    eta_minutes = (eta_r - (eta_hours * 3600)) / 60;
                    const int    eta_seconds = eta_r - (eta_hours * 3600) - (eta_minutes * 60);
                    const double estsz       = size * app_cfg->frames_to_be_encoded / *frame_count;

                    // Encoder knows how many frames are to be encoded, therefore an ETA can be calculated
                    fprintf(stderr,
                            "\rEncoding: \x1b[33m%4d/%d Frames\x1b[0m @ \x1b[32m%.2f\x1b[0m fp%c | \x1b[35m%.2f "
                            "kb/s\x1b[0m | Size: \x1b[31m%.2f MB\x1b[0m \x1b[38;5;248m[%.2f MB]\x1b[0m | Time: "
                            "\x1b[36m%d:%02d:%02d\x1b[0m \x1b[38;5;248m[-%d:%02d:%02d]\x1b[0m ",
                            *frame_count,
                            (int)app_cfg->frames_to_be_encoded,
                            fps >= 1.0 ? fps : fps * 60,
                            fps >= 1.0 ? 's' : 'm',
                            ((double)(app_cfg->performance_context.byte_count << 3) * frame_rate /
                             (app_cfg->frames_encoded * 1000)),
                            size,
                            estsz,
                            ete_hours,
                            ete_minutes,
                            ete_seconds,
                            eta_hours,
                            eta_minutes,
                            eta_seconds);
                }
            } break;
            default:
                break;
            }

            fflush(stderr);
            app_cfg->performance_context.average_speed = (double)app_cfg->performance_context.frame_count /
                app_cfg->performance_context.total_encode_time;
            app_cfg->performance_context.average_latency = (double)app_cfg->performance_context.total_latency /
                app_cfg->performance_context.frame_count;

            if (app_cfg->progress == 1 && !(*frame_count % SPEED_MEASUREMENT_INTERVAL)) {
                fprintf(stderr,
                        "\nAverage System Encoding Speed:        %.2f\n",
                        (double)*frame_count / app_cfg->performance_context.total_encode_time);
            }
        }
    }
    channel->exit_cond_output = return_value;
}

void process_output_recon_buffer(EncChannel* channel) {
    EbConfig*            app_cfg          = channel->app_cfg;
    EbBufferHeaderType*  header_ptr       = app_cfg->recon_buffer; // needs to change for buffered input
    EbComponentType*     component_handle = (EbComponentType*)app_cfg->svt_encoder_handle;
    AppExitConditionType return_value     = APP_ExitConditionNone;
    int32_t              fseek_return_val;
    if (channel->exit_cond_recon != APP_ExitConditionNone) {
        return;
    }
    // non-blocking call until all input frames are sent
    EbErrorType recon_status = svt_av1_get_recon(component_handle, header_ptr);

    if (recon_status == EB_ErrorMax) {
        fprintf(stderr, "\n");
        log_error_output(app_cfg->error_log_file, header_ptr->flags);
        channel->exit_cond_recon = APP_ExitConditionError;
        return;
    } else if (recon_status != EB_NoErrorEmptyQueue) {
        //Sets the File position to the beginning of the file.
        rewind(app_cfg->recon_file);
        uint64_t frame_num = header_ptr->pts;
        while (frame_num > 0) {
            fseek_return_val = fseeko(app_cfg->recon_file, header_ptr->n_filled_len, SEEK_CUR);

            if (fseek_return_val != 0) {
                fprintf(stderr, "Error in fseeko  returnVal %i\n", fseek_return_val);
                channel->exit_cond_recon = APP_ExitConditionError;
                return;
            }
            frame_num = frame_num - 1;
        }

        fwrite(header_ptr->p_buffer, 1, header_ptr->n_filled_len, app_cfg->recon_file);

        // Update Output Port Activity State
        return_value = (header_ptr->flags & EB_BUFFERFLAG_EOS) ? APP_ExitConditionFinished : APP_ExitConditionNone;
    }
    channel->exit_cond_recon = return_value;
}
