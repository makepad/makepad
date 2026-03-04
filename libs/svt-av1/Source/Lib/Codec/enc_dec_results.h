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

#ifndef EbEncDecResults_h
#define EbEncDecResults_h

#include "definitions.h"
#include "sys_resource_manager.h"
#include "object.h"
#ifdef __cplusplus
extern "C" {
#endif
/**************************************
 * Process Results
 **************************************/
typedef struct EncDecResults {
    EbDctor          dctor;
    EbObjectWrapper* pcs_wrapper;
} EncDecResults;

typedef struct DlfResults {
    EbDctor          dctor;
    EbObjectWrapper* pcs_wrapper;
    uint32_t         segment_index;
} DlfResults;

typedef struct CdefResults {
    EbDctor          dctor;
    EbObjectWrapper* pcs_wrapper;
    uint32_t         segment_index;
} CdefResults;

typedef struct RestResults {
    EbDctor          dctor;
    EbObjectWrapper* pcs_wrapper;
    uint16_t         tile_index;
} RestResults;

typedef struct EncDecResultsInitData {
    uint32_t junk;
} EncDecResultsInitData;

/**************************************
     * Extern Function Declarations
     **************************************/
EbErrorType svt_aom_enc_dec_results_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr);

#ifdef __cplusplus
}
#endif
#endif // EbEncDecResults_h
