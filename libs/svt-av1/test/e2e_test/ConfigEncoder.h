/*
 * Copyright(c) 2019 Intel Corporation
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the
 * Alliance for Open Media Patent License 1.0 was not distributed with this
 * source code in the PATENTS file, you can obtain it at
 * https://www.aomedia.org/license/patent-license.
 */

#ifndef _SVT_AV1_CONFIG_ENCODER_H_
#define _SVT_AV1_CONFIG_ENCODER_H_
#include "EbSvtAv1Enc.h"

void *create_enc_config();
void release_enc_config(void *config_ptr);
void set_enc_config(void *config, const char *name, const char *value);
int copy_enc_param(EbSvtAv1EncConfiguration *dst_enc_config, void *config_ptr);
std::string get_enc_token(const char *name);

#endif
