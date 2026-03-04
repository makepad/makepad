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

git clone https://github.com/libjpeg-turbo/libjpeg-turbo.git src

rm -rf src/build/*

cd src || exit
git apply ../buffered_fwrite.patch

mkdir -p build
cd build || exit

cmake -DCMAKE_BUILD_TYPE=Release -DENABLE_STATIC=TRUE -DENABLE_SHARED=FALSE -DWITH_SIMD=TRUE -DWITH_TURBOJPEG=TRUE -DWITH_MEM_SRCDST=TRUE -DWITH_JPEG8=TRUE -DWITH_ARITH_ENC=TRUE -DWITH_ARITH_DEC=TRUE ..
cmake --build . -- -j

cp cjpeg-static "$1"/
cp djpeg-static "$1"/
