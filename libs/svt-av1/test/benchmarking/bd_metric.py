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

import math
import traceback
from operator import itemgetter
from typing import List, Tuple, Union

import numpy as np
import scipy.interpolate


def non_decreasing(L: List[float]) -> bool:
    return all(x <= y for x, y in zip(L, L[1:]))


def check_monotonicity(RDPoints: List[Tuple[float, float]]) -> bool:
    """
    check if the input list of RD points are monotonic, assuming the input
    has been sorted in the quality value non-decreasing order. expect the bit
    rate should also be in the non-decreasing order
    """
    br = [RDPoints[i][0] for i in range(len(RDPoints))]
    qty = [RDPoints[i][1] for i in range(len(RDPoints))]
    return non_decreasing(br) and non_decreasing(qty)


# BJONTEGAARD    Bjontegaard metric
# PCHIP method - Piecewise Cubic Hermite Interpolating Polynomial interpolation
def bd_rate_v2(
    qty_type: str,
    br1: List[float],
    qtyMtrc1: List[float],
    br2: List[float],
    qtyMtrc2: List[float],
) -> Tuple[int, Union[float, str]]:
    brqtypairs1 = []
    brqtypairs2 = []
    for i in range(min(len(qtyMtrc1), len(br1))):
        if br1[i] != "" and qtyMtrc1[i] != "":
            brqtypairs1.append((br1[i], qtyMtrc1[i]))
    for i in range(min(len(qtyMtrc2), len(br2))):
        if br2[i] != "" and qtyMtrc2[i] != "":
            brqtypairs2.append((br2[i], qtyMtrc2[i]))

    if not brqtypairs1 or not brqtypairs2:
        return (-1, "one of input lists is empty!")

    # sort the pair based on quality metric values in increasing order
    # if quality metric values are the same, then sort the bit rate in increasing order
    brqtypairs1.sort(key=itemgetter(1, 0))
    brqtypairs2.sort(key=itemgetter(1, 0))

    rd1_monotonic = check_monotonicity(brqtypairs1)
    rd2_monotonic = check_monotonicity(brqtypairs2)
    if rd1_monotonic is False or rd2_monotonic is False:
        return (
            -1,
            f"Metric {qty_type}: Non-monotonic Error: {brqtypairs1} & {brqtypairs2}",
        )

    try:
        logbr1 = [math.log(x[0]) for x in brqtypairs1]
        qmetrics1 = [100.0 if x[1] == float("inf") else x[1] for x in brqtypairs1]
        logbr2 = [math.log(x[0]) for x in brqtypairs2]
        qmetrics2 = [100.0 if x[1] == float("inf") else x[1] for x in brqtypairs2]
    except ValueError:
        traceback.print_exc()
        return (-1, "Invalid Input")

    # remove duplicated quality metric value, the RD point with higher bit rate is removed
    dup_idx = [i for i in range(1, len(qmetrics1)) if qmetrics1[i - 1] == qmetrics1[i]]
    for idx in sorted(dup_idx, reverse=True):
        del qmetrics1[idx]
        del logbr1[idx]
    dup_idx = [i for i in range(1, len(qmetrics2)) if qmetrics2[i - 1] == qmetrics2[i]]
    for idx in sorted(dup_idx, reverse=True):
        del qmetrics2[idx]
        del logbr2[idx]

    # find max and min of quality metrics
    min_int = max(min(qmetrics1), min(qmetrics2))
    max_int = min(max(qmetrics1), max(qmetrics2))
    if min_int >= max_int:
        return (
            -1,
            f"Metric {qty_type} has no overlap from input 2 lists of quality metrics!: {qmetrics1} & {qmetrics2}",
        )

    # generate samples between max and min of quality metrics
    lin = np.linspace(min_int, max_int, num=100, retstep=True)
    interval = lin[1]
    samples = lin[0]

    # interpolation
    v1 = scipy.interpolate.pchip_interpolate(qmetrics1, logbr1, samples)
    v2 = scipy.interpolate.pchip_interpolate(qmetrics2, logbr2, samples)

    # Calculate the integral using the trapezoid method on the samples.
    int1 = np.trapezoid(v1, dx=interval)
    int2 = np.trapezoid(v2, dx=interval)

    # find avg diff
    avg_exp_diff = (int2 - int1) / (max_int - min_int)
    avg_diff = (math.exp(avg_exp_diff) - 1) * 100

    return (0, round(avg_diff, 4))
