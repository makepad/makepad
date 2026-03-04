#!/bin/bash
# Copyright(c) 2025 Meta Platforms, Inc. and affiliates.
#
# This source code is subject to the terms of the BSD 2 Clause License and
# the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
# was not distributed with this source code in the LICENSE file, you can
# obtain it at https://www.aomedia.org/license/software-license. If the
# Alliance for Open Media Patent License 1.0 was not distributed with this
# source code in the PATENTS file, you can obtain it at
# https://www.aomedia.org/license/patent-license.

DEF_CONFIG=configs/test_image_config.yaml
CONFIG=${1:-${DEF_CONFIG}}

conda activate codec_eval

python3 encode.py "${CONFIG}"
python3 decode_and_qm.py "${CONFIG}"
python3 summary.py "${CONFIG}" --use_enc_qm_logs
