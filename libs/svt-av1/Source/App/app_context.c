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

#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <limits.h>

#include "EbSvtAv1.h"
#include "app_context.h"
#include "app_config.h"
#if DEBUG_ROI
#include <inttypes.h>
#endif

/*************************************
**************************************
*** Helper functions Input / Output **
**************************************
**************************************/

static EbErrorType allocate_frame_buffer(EbConfig* app_cfg, EbSvtIOFormat* input_ptr) {
    EbSvtAv1EncConfiguration* cfg                 = &app_cfg->config;
    const int32_t             ten_bit_packed_mode = cfg->encoder_bit_depth > 8;

    // Chroma subsampling
    const EbColorFormat color_format  = (EbColorFormat)cfg->encoder_color_format;
    const uint8_t       subsampling_x = color_format == EB_YUV444 ? 0 : 1;
    const uint8_t       subsampling_y = (color_format == EB_YUV444 || color_format == EB_YUV422) ? 0 : 1;

    // Determine size of each plane
    const size_t luma_8bit_size = app_cfg->input_padded_width * app_cfg->input_padded_height *
        (1 << ten_bit_packed_mode);
    const uint32_t chroma_width     = (app_cfg->input_padded_width + subsampling_x) >> subsampling_x;
    const uint32_t chroma_height    = (app_cfg->input_padded_height + subsampling_y) >> subsampling_y;
    const uint32_t chroma_8bit_size = chroma_width * chroma_height * (1 << ten_bit_packed_mode);

    // Determine
    input_ptr->y_stride  = app_cfg->input_padded_width;
    input_ptr->cr_stride = chroma_width;
    input_ptr->cb_stride = chroma_width;

    input_ptr->luma = 0;
    input_ptr->cb   = 0;
    input_ptr->cr   = 0;
    if (luma_8bit_size) {
        input_ptr->luma = malloc(luma_8bit_size);
        if (input_ptr->luma == NULL) {
            return EB_ErrorInsufficientResources;
        }
    }

    if (chroma_8bit_size) {
        input_ptr->cb = malloc(chroma_8bit_size);
        input_ptr->cr = malloc(chroma_8bit_size);
        if (input_ptr->cb == NULL || input_ptr->cr == NULL) {
            free(input_ptr->luma);
            free(input_ptr->cb);
            free(input_ptr->cr);
            input_ptr->luma = 0;
            input_ptr->cb   = 0;
            input_ptr->cr   = 0;
            return EB_ErrorInsufficientResources;
        }
    }

    return EB_ErrorNone;
}

static EbErrorType allocate_input_buffers(EbConfig* app_cfg) {
    app_cfg->input_buffer_pool = malloc(sizeof(EbBufferHeaderType));
    if (app_cfg->input_buffer_pool == NULL) {
        return EB_ErrorInsufficientResources;
    }

    // Initialize Header
    app_cfg->input_buffer_pool->size = sizeof(EbBufferHeaderType);

    EbSvtIOFormat* p_buffer = malloc(sizeof(EbSvtIOFormat));

    if (p_buffer == NULL) {
        return EB_ErrorInsufficientResources;
    }

    // Allocate frame buffer for the p_buffer
    if (app_cfg->buffered_input == -1 && !app_cfg->mmap.enable &&
        allocate_frame_buffer(app_cfg, p_buffer) != EB_ErrorNone) {
        free(p_buffer);
        free(app_cfg->input_buffer_pool);
        app_cfg->input_buffer_pool = NULL;
        return EB_ErrorInsufficientResources;
    }
    app_cfg->input_buffer_pool->p_buffer = (uint8_t*)p_buffer;

    // Assign the variables
    app_cfg->input_buffer_pool->p_app_private = NULL;
    app_cfg->input_buffer_pool->pic_type      = EB_AV1_INVALID_PICTURE;

    return EB_ErrorNone;
}

static EbErrorType allocate_output_recon_buffers(EbConfig* app_cfg) {
    const uint8_t subsampling_x = app_cfg->config.encoder_color_format == EB_YUV444 ? 0 : 1;
    const uint8_t subsampling_y = (app_cfg->config.encoder_color_format == EB_YUV444 ||
                                   app_cfg->config.encoder_color_format == EB_YUV422)
        ? 0
        : 1;

    const size_t ten_bit       = (app_cfg->config.encoder_bit_depth > 8);
    const size_t luma_size     = app_cfg->input_padded_width * app_cfg->input_padded_height;
    const size_t chroma_width  = (app_cfg->input_padded_width + subsampling_x) >> subsampling_x;
    const size_t chroma_height = (app_cfg->input_padded_height + subsampling_y) >> subsampling_y;
    const size_t chroma_size   = chroma_width * chroma_height;

    // both u and v
    const size_t frame_size = (luma_size + 2 * chroma_size) << ten_bit;

    // Recon Port
    app_cfg->recon_buffer = malloc(sizeof(*app_cfg->recon_buffer));
    if (app_cfg->recon_buffer == NULL) {
        return EB_ErrorInsufficientResources;
    }

    // Initialize Header
    app_cfg->recon_buffer->size     = sizeof(*app_cfg->recon_buffer);
    app_cfg->recon_buffer->p_buffer = NULL;

    if (app_cfg->config.recon_enabled) {
        app_cfg->recon_buffer->p_buffer = (uint8_t*)malloc(frame_size * sizeof(*app_cfg->recon_buffer->p_buffer));
        if (app_cfg->recon_buffer->p_buffer == NULL) {
            free(app_cfg->recon_buffer);
            app_cfg->recon_buffer = NULL;
            return EB_ErrorInsufficientResources;
        }
    }

    app_cfg->recon_buffer->n_alloc_len   = (uint32_t)frame_size;
    app_cfg->recon_buffer->p_app_private = NULL;
    app_cfg->recon_buffer->metadata      = NULL;

    return EB_ErrorNone;
}

static EbErrorType preload_frames_info_ram(EbConfig* app_cfg) {
    EbErrorType   return_error        = EB_ErrorNone;
    int32_t       input_padded_width  = app_cfg->input_padded_width;
    int32_t       input_padded_height = app_cfg->input_padded_height;
    size_t        read_size;
    const uint8_t subsampling_x = (app_cfg->config.encoder_color_format == EB_YUV444 ? 0 : 1);
    const uint8_t subsampling_y = (app_cfg->config.encoder_color_format == EB_YUV444 ||
                                   app_cfg->config.encoder_color_format == EB_YUV422)
        ? 0
        : 1;
    const size_t  chroma_width  = (app_cfg->input_padded_width + subsampling_x) >> subsampling_x;
    const size_t  chroma_height = (app_cfg->input_padded_height + subsampling_y) >> subsampling_y;

    read_size = input_padded_width * input_padded_height; //Luma
    read_size += 2 * chroma_width * chroma_height; // Add Chroma
    if (app_cfg->config.encoder_bit_depth > 8) {
        read_size *= 2; //10 bit
    }
    app_cfg->sequence_buffer = calloc(app_cfg->buffered_input, sizeof(uint8_t*));
    if (app_cfg->sequence_buffer == NULL) {
        return EB_ErrorInsufficientResources;
    }

    for (int32_t processed_frame_count = 0; processed_frame_count < app_cfg->buffered_input; ++processed_frame_count) {
        app_cfg->sequence_buffer[processed_frame_count] = malloc(read_size);
        if (app_cfg->sequence_buffer[processed_frame_count] == NULL) {
            return EB_ErrorInsufficientResources;
        }

        // Fill the buffer with a complete frame
        size_t filled_len = fread(app_cfg->sequence_buffer[processed_frame_count], 1, read_size, app_cfg->input_file);

        if (read_size != filled_len) {
            fseek(app_cfg->input_file, 0, SEEK_SET);

            // Fill the buffer with a complete frame
            if (read_size !=
                fread(app_cfg->sequence_buffer[processed_frame_count], 1, read_size, app_cfg->input_file)) {
                return_error = EB_Corrupt_Frame;
            }
        }
    }

    return return_error;
}

static int compare_seg_qp(const void* qp_first, const void* qp_second) {
    if (*(const int16_t*)qp_first < *(const int16_t*)qp_second) {
        return 1;
    } else if (*(const int16_t*)qp_first == *(const int16_t*)qp_second) {
        return 0;
    } else {
        return -1;
    }
}

static EbErrorType parse_rio_map_file(EbConfig* app_cfg) {
    const int32_t MAX_SEGMENTS = 8;
    FILE*         file         = app_cfg->roi_map_file;
    if (file == NULL) {
        return EB_ErrorBadParameter;
    }

    // ROI map file format:
    // One ROI event per line. The event is in below format
    // <pic_num> <b64_qp_offset> <b64_qp_offset> ... <b64_qp_offset>\n
    // b64_qp_offset range -255 ~ 255
    EbErrorType      ret      = EB_ErrorNone;
    SvtAv1RoiMap*    roi_map  = calloc(1, sizeof(*app_cfg->roi_map));
    SvtAv1RoiMapEvt* last_evt = NULL;
    const size_t     b64_num = ((app_cfg->config.source_width + 63) / 64) * ((app_cfg->config.source_height + 63) / 64);
    // Multiplied by 5 because each qp_offset value requires at most 4 chars plus a space.
    // Multiplied by 2 to make some extra space.
    const size_t buf_size = b64_num * 5 * 2;
    char*        buf      = malloc(buf_size);
    int16_t*     qp_map   = malloc(sizeof(*qp_map) * b64_num);
    if (!roi_map || !buf || !qp_map) {
        ret = EB_ErrorInsufficientResources;
        goto fail;
    }
    roi_map->evt_num  = 0;
    roi_map->evt_list = NULL;
    roi_map->cur_evt  = NULL;
    roi_map->buf      = buf;
    roi_map->qp_map   = qp_map;
    app_cfg->roi_map  = roi_map;

    while (fgets(buf, (int)buf_size, file)) {
        if (strlen(buf) == buf_size - 1) {
            fprintf(stderr, "Warning - May exceed the line length limitation of ROI map file\n");
        }
        if (buf[0] != '\n') {
            char*    p              = buf;
            char*    end            = p;
            uint64_t picture_number = strtoull(p, &end, 10);
            if (end == p) {
                // no new value parsed
                break;
            }
            if (picture_number == ULLONG_MAX) {
                ret = EB_ErrorBadParameter;
                break;
            }

            // allocate a new ROI event
            SvtAv1RoiMapEvt* evt = calloc(1, sizeof(*evt));
            if (!evt) {
                ret = EB_ErrorInsufficientResources;
                goto fail;
            }
            evt->b64_seg_map = malloc(b64_num);
            if (!evt->b64_seg_map) {
                free(evt);
                ret = EB_ErrorInsufficientResources;
                goto fail;
            }
            if (roi_map->evt_list != NULL) {
                last_evt->next = evt;
            } else {
                roi_map->evt_list = evt;
            }
            last_evt = evt;

            evt->start_picture_number = picture_number;
            evt->max_seg_id           = -1;

            // 1. parsing qp offset
            // 2. decide qp offset of each segment from qp offset map
            // 3. translate qp offset map to a segment id map
            size_t i;
            for (i = 0; i < b64_num; ++i) {
                p            = end;
                long int val = strtol(p, &end, 10);
                if (end == p) {
                    // no new value parsed
                    break;
                }
                if (val <= 255 && val >= -255) {
                    qp_map[i] = (int16_t)val;
                    // map qp offset to segment id
                    int8_t seg_id = 0;
                    for (; seg_id <= evt->max_seg_id; ++seg_id) {
                        if (qp_map[i] == evt->seg_qp[seg_id]) {
                            break;
                        }
                    }
                    if (seg_id > evt->max_seg_id && evt->max_seg_id < MAX_SEGMENTS) {
                        evt->seg_qp[seg_id] = qp_map[i];
                        evt->max_seg_id     = seg_id;
                    } else if (seg_id > evt->max_seg_id && evt->max_seg_id >= MAX_SEGMENTS) {
                        ret = EB_ErrorBadParameter;
                        fprintf(stderr,
                                "Error: Invalid ROI map file - Maximum number of segment supported "
                                "by AV1 spec is eight\n");
                        break;
                    }
                } else {
                    ret = EB_ErrorBadParameter;
                    fprintf(stderr,
                            "Error: Invalid ROI map file - Invalid qp offset %ld. The expected "
                            "range is between -255 and 255\n",
                            val);
                    break;
                }
            }
            if (i < b64_num) {
                ret = EB_ErrorBadParameter;
                fprintf(stderr, "Error: Invalid ROI map file - not enough qp offset within a ROI event\n");
            }
            if (ret != EB_ErrorNone) {
                break;
            }

            // sort seg_qp array in descending order
            qsort(evt->seg_qp, evt->max_seg_id + 1, sizeof(evt->seg_qp[0]), compare_seg_qp);
            if (evt->seg_qp[0] < 0) {
                fprintf(stderr, "Warning: All qp offsets are negative may result in undecodable bitstream\n");
            }

            // translate the qp offset map provided in the ROI map file to a segment id map.
            for (i = 0; i < b64_num; ++i) {
                for (int seg_id = 0; seg_id <= evt->max_seg_id; ++seg_id) {
                    if (qp_map[i] == evt->seg_qp[seg_id]) {
                        evt->b64_seg_map[i] = seg_id;
                        break;
                    }
                }
            }

            ++roi_map->evt_num;
#if DEBUG_ROI
            fprintf(stdout,
                    "ROI map event %" PRIu32 ". start picture num %" PRIu64 "\n",
                    roi_map->evt_num,
                    evt->start_picture_number);
            fprintf(stdout, "qp_offset ");
            for (int i = 0; i <= evt->max_seg_id; ++i) {
                fprintf(stdout, "%d ", evt->seg_qp[i]);
            }
            fprintf(stdout, "\n");
            int column_b64 = (app_cfg->config.source_width + 63) / 64;
            int row_b64    = (app_cfg->config.source_height + 63) / 64;
            for (int i = 0; i < row_b64; ++i) {
                for (int j = 0; j < column_b64; ++j) {
                    fprintf(stdout, "%d ", evt->b64_seg_map[i * column_b64 + j]);
                }
                fprintf(stdout, "\n");
            }
            fprintf(stdout, "\n");
#endif
        }
    }

    if (roi_map->evt_num == 0 && ret == EB_ErrorNone) {
        // empty roi map file
        ret = EB_ErrorBadParameter;
    }
    return ret;
fail:
    if (last_evt) {
        for (SvtAv1RoiMapEvt* evt = roi_map->evt_list; evt != last_evt;) {
            SvtAv1RoiMapEvt* next = evt->next;
            free(evt->b64_seg_map);
            free(evt);
            evt = next;
        }
    }
    free(qp_map);
    free(buf);
    free(roi_map);
    return ret;
}

static void deallocate_buffers(EbConfig* app_cfg) {
    // Deallocate input buffers
    if (app_cfg->input_buffer_pool) {
        if (app_cfg->buffered_input == -1 && !app_cfg->mmap.enable) {
            EbSvtIOFormat* input_ptr = (EbSvtIOFormat*)app_cfg->input_buffer_pool->p_buffer;
            if (input_ptr) {
                free(input_ptr->luma);
                free(input_ptr->cb);
                free(input_ptr->cr);
            }
        }
        free(app_cfg->input_buffer_pool->p_buffer);
        free(app_cfg->input_buffer_pool);
    }

    // Deallocate output recon buffers
    if (app_cfg->recon_buffer) {
        free(app_cfg->recon_buffer->p_buffer);
        free(app_cfg->recon_buffer);
    }

    // Deallocate sequence buffer
    if (app_cfg->sequence_buffer) {
        for (int i = 0; i < app_cfg->buffered_input; ++i) {
            free(app_cfg->sequence_buffer[i]);
        }
        free(app_cfg->sequence_buffer);
    }
}

/***************************************
* Functions Implementation
***************************************/

/***********************************
 * Initialize Core & Component
 ***********************************/
EbErrorType init_encoder(EbConfig* app_cfg) {
    // Initialize Port Activity Flags
    app_cfg->output_stream_port_active = APP_PortActive;

    if (app_cfg->roi_map_file != NULL) {
        // Load ROI map data from file
        app_cfg->config.enable_roi_map = true;
        EbErrorType return_error       = parse_rio_map_file(app_cfg);
        if (return_error != EB_ErrorNone) {
            return return_error;
        }
    }

    // Send over all configuration parameters
    // Set the Parameters
    EbErrorType return_error = svt_av1_enc_set_parameter(app_cfg->svt_encoder_handle, &app_cfg->config);

    if (return_error != EB_ErrorNone) {
        return return_error;
    }
    // STEP 5: Init Encoder
    return_error = svt_av1_enc_init(app_cfg->svt_encoder_handle);

    if (return_error != EB_ErrorNone) {
        return return_error;
    }

    ///************************* LIBRARY INIT [END] *********************///

    ///********************** APPLICATION INIT [START] ******************///

    // STEP 6: Allocate input buffers carrying the yuv frames in
    return_error = allocate_input_buffers(app_cfg);

    if (return_error != EB_ErrorNone) {
        return return_error;
    }
    // STEP 7: Allocate output Recon Buffer
    return_error = allocate_output_recon_buffers(app_cfg);

    if (return_error != EB_ErrorNone) {
        return return_error;
    }
    // Allocate the Sequence Buffer
    if (app_cfg->buffered_input != -1) {
        // Preload frames into the ram for a faster yuv access time
        return_error = preload_frames_info_ram(app_cfg);
    } else {
        app_cfg->sequence_buffer = 0;
    }
    ///********************** APPLICATION INIT [END] ******************////////

    return return_error;
}

/***********************************
 * Deinit Components
 ***********************************/
EbErrorType de_init_encoder(EbConfig* app_cfg) {
    EbErrorType return_error = EB_ErrorNone;

    deallocate_buffers(app_cfg);

    // Destruct the component
    svt_av1_enc_deinit_handle(app_cfg->svt_encoder_handle);

    return return_error;
}
