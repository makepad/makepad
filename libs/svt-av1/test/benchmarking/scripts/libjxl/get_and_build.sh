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

git clone https://github.com/libjxl/libjxl.git --recursive --shallow-submodules src

rm -rf src/build/*

cd src || exit
mkdir -p build
cd build || exit

cmake -DCMAKE_BUILD_TYPE=Release -DBUILD_TESTING=OFF -DJPEGXL_ENABLE_DEVTOOLS=ON -DJPEGXL_STATIC=ON ..
cmake --build . -- -j

cp tools/cjpegli "$1"/
cp tools/djpegli "$1"/
cp tools/cjxl "$1"/
cp tools/djxl "$1"/
cp tools/ssimulacra2 "$1"/
