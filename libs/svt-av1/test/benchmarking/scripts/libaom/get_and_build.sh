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

git clone https://aomedia.googlesource.com/aom src

# normal build
cd src || exit
rm -rf CMakeCache.txt CMakeFiles
mkdir -p ../aom_build
cd ../aom_build || exit

cmake ../src -DCMAKE_BUILD_TYPE=Release -DCONFIG_REALTIME_ONLY=0 -DCONFIG_AV1_HIGHBITDEPTH=1 -DENABLE_DOCS=0 -DENABLE_TESTS=0
make -j

cp aomenc "$1"/
cp aomdec "$1"/

# RTC build
cd ../src || exit
rm -rf CMakeCache.txt CMakeFiles
mkdir -p ../aom_build
cd ../aom_build || exit

cmake ../src -DCMAKE_BUILD_TYPE=Release -DCONFIG_REALTIME_ONLY=1 -DCONFIG_AV1_HIGHBITDEPTH=0 -DENABLE_DOCS=0 -DENABLE_TESTS=0
make -j

cp aomenc "$1"/aomenc_rtc
