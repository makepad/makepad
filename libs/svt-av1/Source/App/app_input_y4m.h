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

#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include <assert.h>

#include "app_config.h"

EbErrorType read_y4m_header(EbConfig* cfg);

void read_and_skip_y4m_header(FILE* input_file);

size_t read_y4m_frame_delimiter(FILE* input_file, FILE* error_log_file);

bool check_if_y4m(EbConfig* cfg);
