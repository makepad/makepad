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

#ifndef EbAppContext_h
#define EbAppContext_h

#include "EbSvtAv1Enc.h"
#include "app_config.h"

/********************************
 * External Function
 ********************************/
EbErrorType init_encoder(EbConfig* app_cfg);
EbErrorType de_init_encoder(EbConfig* app_cfg);

#endif // EbAppContext_h
