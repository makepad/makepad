#!/bin/sh
set -eu
cd "$(dirname "$0")"
c++ -O2 -std=c++17 -DNDEBUG -DXA_DEBUG=0 -DXA_MULTITHREADED=0 \
    -o xatlas_oracle parametrize.cpp ../vendor/xatlas.cpp
echo "built $(pwd)/xatlas_oracle"
