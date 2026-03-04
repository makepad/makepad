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

git clone https://github.com/AOMediaCodec/libavif.git src

# normal build
cd src || exit
mkdir -p build
cd build || exit

cmake .. -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF -DAVIF_CODEC_AOM=LOCAL -DAVIF_CODEC_SVT=LOCAL -DAVIF_CODEC_DAV1D=LOCAL -DAVIF_LIBYUV=LOCAL -DAVIF_LIBSHARPYUV=LOCAL -DAVIF_JPEG=LOCAL -DAVIF_ZLIBPNG=LOCAL -DAVIF_BUILD_APPS=ON -DAVIF_BUILD_TESTS=OFF -DAVIF_BUILD_EXAMPLES=OFF
cmake --build . --config Release --parallel

cp avifenc "$1"/avifenc
cp avifdec "$1"/avifdec
