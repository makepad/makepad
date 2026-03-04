#!/usr/bin/env python3
# Copyright(c) 2025 Meta Platforms, Inc. and affiliates.
#
# This source code is subject to the terms of the BSD 2 Clause License and
# the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
# was not distributed with this source code in the LICENSE file, you can
# obtain it at https://www.aomedia.org/license/software-license. If the
# Alliance for Open Media Patent License 1.0 was not distributed with this
# source code in the PATENTS file, you can obtain it at
# https://www.aomedia.org/license/patent-license.

import re
import matplotlib.pyplot as plt
import sys

if len(sys.argv) < 2:
    print(f"Usage: python3 {sys.argv[0]} <stats file>")
    sys.exit(0)

# Read the file
with open(sys.argv[1], "r") as f:
    lines = f.readlines()
vbv_delays = []
picture_numbers = []
# Regex to extract Picture Number and VBV delay
pattern = re.compile(r"Picture Number:\s*(\d+).*?VBV delay:\s*([\d.]+) s")
for line in lines:
    match = pattern.search(line)
    if match:
        picture_number = int(match.group(1))
        vbv_delay = float(match.group(2))
        picture_numbers.append(picture_number)
        vbv_delays.append(vbv_delay)
# Plot
plt.figure(figsize=(8, 4))
plt.plot(picture_numbers, vbv_delays, marker="o", markersize=4)
plt.xlabel("Picture Number")
plt.ylabel("VBV delay (s)")
plt.title("VBV delay per Picture")
plt.grid(True)
plt.tight_layout()
plt.show()
