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

#include "app_input_y4m.h"
#include <string.h>
#if !defined(_WIN32) || !defined(HAVE_STRNLEN_S)
#include "third_party/safestringlib/safe_str_lib.h"
#endif
#define YFM_HEADER_MAX 4096 // Technically no max according to "spec"
#define PRINT_HEADER 0

/* only reads the y4m header, needed when we reach end of the file and loop to the beginning */
void read_and_skip_y4m_header(FILE* input_file) {
    int c = 0;
    while ((c = fgetc(input_file)) != '\n' && c != EOF) {}
}

/* reads the y4m header and parses the input parameters */
EbErrorType read_y4m_header(EbConfig* cfg) {
#define CHROMA_MAX 4
    char          buffer[YFM_HEADER_MAX];
    char*         tokstart = buffer;
    uint32_t      bitdepth = 8;
    unsigned long width = 0, height = 0;
    long          fr_n = 0, fr_d = 0, aspect_n = 0, aspect_d = 0;
    char          chroma[CHROMA_MAX] = "420", scan_type = 'p';

    /* get first line after YUV4MPEG2 */
    if (!fgets(buffer, sizeof(buffer), cfg->input_file)) {
        return EB_ErrorBadParameter;
    }

    /* print header */
    if (PRINT_HEADER) {
        fprintf(stderr, "y4m header:");
        fputs(buffer, stdout);
    }

    /* read header parameters */
    while (*tokstart != '\0') {
        switch (*tokstart++) {
        case 0x20:
            continue;
        case 'W': /* width, required. */
            width = strtol(tokstart, &tokstart, 10);
            if (PRINT_HEADER) {
                fprintf(stderr, "width = %lu\n", width);
            }
            break;
        case 'H': /* height, required. */
            height = strtol(tokstart, &tokstart, 10);
            if (PRINT_HEADER) {
                fprintf(stderr, "height = %lu\n", height);
            }
            break;
        case 'I': /* scan type, not required, default: 'p' */
            switch (*tokstart++) {
            case 'p':
                scan_type = 'p';
                break;
            case 't':
                scan_type = 't';
                break;
            case 'b':
                scan_type = 'b';
                break;
            case '?':
            default:
                fprintf(cfg->error_log_file, "interlace type not supported\n");
                return EB_ErrorBadParameter;
            }
            if (PRINT_HEADER) {
                fprintf(stderr, "scan_type = %c\n", scan_type);
            }
            break;
        case 'C': /* color space, not required: default "420" */
#define chroma_compare(a, b) !strncmp(a, b, sizeof(a) - 1)
            if (chroma_compare("420mpeg2", tokstart)) {
                strcpy_s(chroma, CHROMA_MAX, "420");
                // chroma left
                bitdepth = 8;
            } else if (chroma_compare("420paldv", tokstart)) {
                strcpy_s(chroma, CHROMA_MAX, "420");
                // chroma top-left
                bitdepth = 8;
            } else if (chroma_compare("420jpeg", tokstart)) {
                strcpy_s(chroma, CHROMA_MAX, "420");
                // chroma center
                bitdepth = 8;
            } else if (chroma_compare("420p16", tokstart)) {
                strcpy_s(chroma, CHROMA_MAX, "420");
                bitdepth = 16;
            } else if (chroma_compare("422p16", tokstart)) {
                strcpy_s(chroma, CHROMA_MAX, "422");
                bitdepth = 16;
            } else if (chroma_compare("444p16", tokstart)) {
                strcpy_s(chroma, CHROMA_MAX, "444");
                bitdepth = 16;
            } else if (chroma_compare("420p14", tokstart)) {
                strcpy_s(chroma, CHROMA_MAX, "420");
                bitdepth = 14;
            } else if (chroma_compare("422p14", tokstart)) {
                strcpy_s(chroma, CHROMA_MAX, "422");
                bitdepth = 14;
            } else if (chroma_compare("444p14", tokstart)) {
                strcpy_s(chroma, CHROMA_MAX, "444");
                bitdepth = 14;
            } else if (chroma_compare("420p12", tokstart)) {
                strcpy_s(chroma, CHROMA_MAX, "420");
                bitdepth = 12;
            } else if (chroma_compare("422p12", tokstart)) {
                strcpy_s(chroma, CHROMA_MAX, "422");
                bitdepth = 12;
            } else if (chroma_compare("444p12", tokstart)) {
                strcpy_s(chroma, CHROMA_MAX, "444");
                bitdepth = 12;
            } else if (chroma_compare("420p10", tokstart)) {
                strcpy_s(chroma, CHROMA_MAX, "420");
                bitdepth = 10;
            } else if (chroma_compare("422p10", tokstart)) {
                strcpy_s(chroma, CHROMA_MAX, "422");
                bitdepth = 10;
            } else if (chroma_compare("444p10", tokstart)) {
                strcpy_s(chroma, CHROMA_MAX, "444");
                bitdepth = 10;
            } else if (chroma_compare("420p9", tokstart)) {
                strcpy_s(chroma, CHROMA_MAX, "420");
                bitdepth = 9;
            } else if (chroma_compare("422p9", tokstart)) {
                strcpy_s(chroma, CHROMA_MAX, "422");
                bitdepth = 9;
            } else if (chroma_compare("444p9", tokstart)) {
                strcpy_s(chroma, CHROMA_MAX, "444");
                bitdepth = 9;
            } else if (chroma_compare("420", tokstart)) {
                strcpy_s(chroma, CHROMA_MAX, "420");
                bitdepth = 8;
            } else if (chroma_compare("411", tokstart)) {
                strcpy_s(chroma, CHROMA_MAX, "411");
                bitdepth = 8;
            } else if (chroma_compare("422", tokstart)) {
                strcpy_s(chroma, CHROMA_MAX, "422");
                bitdepth = 8;
            } else if (chroma_compare("444", tokstart)) {
                strcpy_s(chroma, CHROMA_MAX, "444");
                bitdepth = 8;
            } else if (chroma_compare("mono16", tokstart)) {
                strcpy_s(chroma, CHROMA_MAX, "400");
                bitdepth = 16;
            } else if (chroma_compare("mono12", tokstart)) {
                strcpy_s(chroma, CHROMA_MAX, "400");
                bitdepth = 12;
            } else if (chroma_compare("mono10", tokstart)) {
                strcpy_s(chroma, CHROMA_MAX, "400");
                bitdepth = 10;
            } else if (chroma_compare("mono9", tokstart)) {
                strcpy_s(chroma, CHROMA_MAX, "400");
                bitdepth = 9;
            } else if (chroma_compare("mono", tokstart)) {
                strcpy_s(chroma, CHROMA_MAX, "400");
                bitdepth = 8;
            } else {
                fprintf(cfg->error_log_file, "chroma format not supported\n");
                return EB_ErrorBadParameter;
            }
#undef chroma_compare
            while (*tokstart != 0x20 && *tokstart != '\n') {
                tokstart++;
            }
            if (PRINT_HEADER) {
                fprintf(stderr, "chroma = %s, bitdepth = %u\n", chroma, bitdepth);
            }
            break;
        case 'F': /* frame rate, required */
            fr_n = strtol(tokstart, &tokstart, 10);
            fr_d = strtol(++tokstart, &tokstart, 10);
            ++tokstart;
            if (PRINT_HEADER) {
                fprintf(stderr, "framerate_n = %ld\nframerate_d = %ld\n", fr_n, fr_d);
            }
            break;
        case 'A': /* aspect ratio, not required */
            aspect_n = strtol(tokstart, &tokstart, 10);
            aspect_d = strtol(++tokstart, &tokstart, 10);
            ++tokstart;
            if (PRINT_HEADER) {
                fprintf(stderr, "aspect_n = %ld\naspect_d = %ld\n", aspect_n, aspect_d);
            }
            break;
        default:
            /* Unknown section: skip it */
            while (*tokstart != 0x20 && *tokstart != '\0') {
                tokstart++;
            }
            break;
        }
    }

    /*check if required parameters were read*/
    if (width == 0) {
        fprintf(cfg->error_log_file, "width not found in y4m header\n");
        return EB_ErrorBadParameter;
    }
    if (height == 0) {
        fprintf(cfg->error_log_file, "height not found in y4m header\n");
        return EB_ErrorBadParameter;
    }
    if (fr_n == 0 || fr_d == 0) {
        fprintf(cfg->error_log_file, "frame rate not found in y4m header\n");
        return EB_ErrorBadParameter;
    }

    /* Assign parameters to cfg */
    cfg->config.source_width           = width;
    cfg->config.source_height          = height;
    cfg->config.frame_rate_numerator   = fr_n;
    cfg->config.frame_rate_denominator = fr_d;
    cfg->config.encoder_bit_depth      = bitdepth;
    cfg->mmap.y4m_seq_hdr              = ftell(cfg->input_file);

    /* TODO: when implemented, need to set input bit depth
        (instead of the encoder bit depth) and chroma format */

    if (strcmp("420", chroma)) {
        fprintf(cfg->error_log_file,
                "Error: %s chroma format not supported, only 420 is supported at this "
                "time.\n",
                chroma);
        return EB_ErrorBadParameter;
    }

    return EB_ErrorNone;
#undef CHROMA_MAX
}

/* read next line which contains the "FRAME" delimiter */
size_t read_y4m_frame_delimiter(FILE* input_file, FILE* error_log_file) {
    char buffer_y4m_header[YFM_HEADER_MAX] = {0};

    if (!fgets(buffer_y4m_header, sizeof(buffer_y4m_header), input_file)) {
        fprintf(error_log_file, "Failed to read y4m frame delimiter. Read broken. EOF: %i\n", feof(input_file));
        return 0;
    }
    if (strncmp(buffer_y4m_header, "FRAME", sizeof("FRAME") - 1)) {
        fprintf(error_log_file, "Failed to read proper y4m frame delimiter. Read broken.\n");
        return 0;
    }
    return strlen(buffer_y4m_header);
}

/* check if the input file is in YUV4MPEG2 (y4m) format */
bool check_if_y4m(EbConfig* cfg) {
    const size_t y4m_magic_size = 9;
    if (fread(cfg->y4m_buf, y4m_magic_size, 1, cfg->input_file) != 1) {
        return false;
    }
    if (cfg->input_file != stdin && !cfg->input_file_is_fifo) {
        fseek(cfg->input_file, 0, SEEK_SET);
    }
    return !strncmp(cfg->y4m_buf, "YUV4MPEG2", y4m_magic_size);
}
