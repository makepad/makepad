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

#ifndef EbMetadataHandle_h
#define EbMetadataHandle_h

#include "EbSvtAv1.h"
#include "EbSvtAv1Metadata.h"

EbErrorType svt_aom_copy_metadata_buffer(EbBufferHeaderType* dst, const SvtMetadataArrayT* const src);

#endif
