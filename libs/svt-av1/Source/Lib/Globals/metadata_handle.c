/*
* Copyright (c) 2021, Alliance for Open Media. All rights reserved
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#include <stdio.h>
#include <stdlib.h>
#include <math.h>
#include <string.h>

#include "EbSvtAv1Metadata.h"
#include "EbSvtAv1Enc.h"
#include "svt_log.h"
#include "svt_malloc.h"

EB_API SvtMetadataT* svt_metadata_alloc(const uint32_t type, const uint8_t* data, const size_t sz) {
    if (!data || sz == 0) {
        return NULL;
    }
    SvtMetadataT* metadata;
    EB_MALLOC_OBJECT_NO_CHECK(metadata);
    if (!metadata) {
        return NULL;
    }
    metadata->type = type;
    EB_MALLOC_ARRAY_NO_CHECK(metadata->payload, sz);
    if (!metadata->payload) {
        EB_FREE(metadata);
        return NULL;
    }
    memcpy(metadata->payload, data, sz);
    metadata->sz = sz;
    return metadata;
}

EB_API void svt_metadata_free(void* ptr) {
    SvtMetadataT** metadata = (SvtMetadataT**)ptr;
    if (*metadata) {
        if ((*metadata)->payload) {
            EB_FREE_ARRAY((*metadata)->payload);
            (*metadata)->payload = NULL;
        }
        EB_FREE(*metadata);
        *metadata = NULL;
    }
}

EB_API SvtMetadataArrayT* svt_metadata_array_alloc(const size_t sz) {
    SvtMetadataArrayT* arr;
    EB_CALLOC_ARRAY_NO_CHECK(arr, 1);
    if (!arr) {
        return NULL;
    }
    if (sz > 0) {
        EB_CALLOC_ARRAY_NO_CHECK(arr->metadata_array, sz);
        if (!arr->metadata_array) {
            svt_metadata_array_free(&arr);
            return NULL;
        }
        arr->sz = sz;
    }
    return arr;
}

EB_API void svt_metadata_array_free(void* arr) {
    SvtMetadataArrayT** metadata = (SvtMetadataArrayT**)arr;
    if (*metadata) {
        if ((*metadata)->metadata_array) {
            for (size_t i = 0; i < (*metadata)->sz; i++) {
                svt_metadata_free(&((*metadata)->metadata_array[i]));
            }
            EB_FREE((*metadata)->metadata_array);
        }
        EB_FREE(*metadata);
    }
    *metadata = NULL;
}

EB_API int svt_add_metadata(EbBufferHeaderType* buffer, const uint32_t type, const uint8_t* data, const size_t sz) {
    if (!buffer) {
        return -1;
    }
    if (!buffer->metadata) {
        buffer->metadata = svt_metadata_array_alloc(0);
        if (!buffer->metadata) {
            return -1;
        }
    }
    SvtMetadataT* metadata = svt_metadata_alloc(type, data, sz);
    if (!metadata) {
        return -1;
    }
    EB_REALLOC_ARRAY_NO_CHECK(buffer->metadata->metadata_array, buffer->metadata->sz + 1);
    if (!buffer->metadata->metadata_array) {
        svt_metadata_free(&metadata);
        return -1;
    }
    buffer->metadata->metadata_array[buffer->metadata->sz] = metadata;
    buffer->metadata->sz++;
    return 0;
}

EbErrorType svt_aom_copy_metadata_buffer(EbBufferHeaderType* dst, const SvtMetadataArrayT* const src) {
    if (!dst || !src) {
        return EB_ErrorBadParameter;
    }
    EbErrorType return_error = EB_ErrorNone;
    for (size_t i = 0; i < src->sz; ++i) {
        SvtMetadataT*  current_metadata = src->metadata_array[i];
        const uint32_t type             = current_metadata->type;
        const uint8_t* payload          = current_metadata->payload;
        const size_t   sz               = current_metadata->sz;

        if (svt_add_metadata(dst, type, payload, sz)) {
            return_error = EB_ErrorInsufficientResources;
            SVT_ERROR("Metadata of type %d could not be added to the buffer.\n", type);
        }
    }
    return return_error;
}

EB_API size_t svt_metadata_size(SvtMetadataArrayT* metadata, const EbAv1MetadataType type) {
    size_t sz = 0;
    if (!metadata || !metadata->metadata_array || metadata->sz == 0) {
        return 0;
    } else {
        for (size_t i = 0; i < metadata->sz; i++) {
            SvtMetadataT* current_metadata = metadata->metadata_array[i];
            if (current_metadata && current_metadata->payload && current_metadata->type == type) {
                sz += current_metadata->sz + 1 //obu type
                    + 1 //trailing byte
                    + 1 //header size
                    + 1; //length field size
            }
        }
    }
    return sz;
}

static inline uint16_t intswap16(uint16_t x) {
    return x << 8 | x >> 8;
}

static inline uint32_t intswap32(uint32_t x) {
    return x >> 24 | (x >> 8 & 0xff00) | (x << 8 & 0xff0000) | x << 24;
}

static inline uint16_t clip16be(double x) {
    return intswap16(x > 65535 ? 65535 : (uint16_t)x);
}

// Parses "(d1,d2)" into two double values and returns the pointer to after the closing parenthesis.
// returns NULL if it fails
static inline char* parse_double(const char* p, double* d1, double* d2) {
    char* endptr;
    if (*p != '(') {
        return NULL;
    }
    *d1 = strtod(p + 1, &endptr);
    if (*endptr != ',') {
        return NULL;
    }
    *d2 = strtod(endptr + 1, &endptr);
    return *endptr == ')' ? endptr + 1 : NULL;
}

EB_API int svt_aom_parse_mastering_display(struct EbSvtAv1MasteringDisplayInfo* mdi, const char* md_str) {
    if (!mdi || !md_str) {
        return 0;
    }
    double gx = 0, gy = 0, bx = 0, by = 0, rx = 0, ry = 0, wx = 0, wy = 0, max_luma = 0, min_luma = 0;
    while (md_str && *md_str) {
        switch (*md_str) {
        case 'G':
        case 'g':
            md_str = parse_double(md_str + 1, &gx, &gy);
            break;
        case 'B':
        case 'b':
            md_str = parse_double(md_str + 1, &bx, &by);
            break;
        case 'R':
        case 'r':
            md_str = parse_double(md_str + 1, &rx, &ry);
            break;
        case 'W':
        case 'w':
            md_str = parse_double(md_str + 2, &wx, &wy);
            break;
        case 'L':
        case 'l':
            md_str = parse_double(md_str + 1, &max_luma, &min_luma);
            break;
        default:
            break;
        }
    }
#define between1(x) (x >= 0.0 && x <= 1.0)
    if (!between1(gx) || !between1(gy) || !between1(bx) || !between1(by) || !between1(rx) || !between1(ry) ||
        !between1(wx) || !between1(wy)) {
        SVT_WARN("Invalid mastering display info will be clipped to 0.0 to 1.0\n");
    }
#undef between1
    memset(mdi, 0, sizeof(*mdi));
    rx       = round(rx * (1 << 16));
    ry       = round(ry * (1 << 16));
    gx       = round(gx * (1 << 16));
    gy       = round(gy * (1 << 16));
    bx       = round(bx * (1 << 16));
    by       = round(by * (1 << 16));
    wx       = round(wx * (1 << 16));
    wy       = round(wy * (1 << 16));
    max_luma = round(max_luma * (1 << 8));
    min_luma = round(min_luma * (1 << 14));

    mdi->r = (struct EbSvtAv1ChromaPoints){
        .x = clip16be(rx),
        .y = clip16be(ry),
    };
    mdi->g = (struct EbSvtAv1ChromaPoints){
        .x = clip16be(gx),
        .y = clip16be(gy),
    };
    mdi->b = (struct EbSvtAv1ChromaPoints){
        .x = clip16be(bx),
        .y = clip16be(by),
    };
    mdi->white_point = (struct EbSvtAv1ChromaPoints){
        .x = clip16be(wx),
        .y = clip16be(wy),
    };
    mdi->max_luma = intswap32((uint32_t)max_luma);
    mdi->min_luma = intswap32((uint32_t)min_luma);
    return 1;
}

EB_API int svt_aom_parse_content_light_level(struct EbContentLightLevel* cll, const char* cll_str) {
    if (!cll || !cll_str) {
        return 0;
    }
    char*  endptr;
    double max_cll = strtod(cll_str, &endptr);
    if (*endptr != ',') {
        goto fail;
    }
    double max_fall = strtod(endptr + 1, &endptr);
    if (*endptr) {
        goto fail;
    }
    cll->max_cll  = clip16be(max_cll);
    cll->max_fall = clip16be(max_fall);
    return 1;

fail:
    SVT_WARN("Invalid cll provided\n");
    return 0;
}
