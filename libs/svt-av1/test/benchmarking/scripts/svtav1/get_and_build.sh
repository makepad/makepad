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

git clone https://gitlab.com/AOMediaCodec/SVT-AV1.git src

cd src || exit

./Build/linux/build.sh -x --minimal-build --enable-lto

cp Bin/Release/SvtAv1EncApp "$1"/
