/*
 * Copyright (c) 2024, Alliance for Open Media. All rights reserved.
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at www.aomedia.org/license/software. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at www.aomedia.org/license/patent.
 */

#ifndef NEON_SVE2_BRIDGE_H
#define NEON_SVE2_BRIDGE_H

#include <arm_neon_sve_bridge.h>

// We can access instructions exclusive to the SVE2 instruction set from a
// predominantly Neon context by making use of the Neon-SVE bridge intrinsics
// to reinterpret Neon vectors as SVE vectors - with the high part of the SVE
// vector (if it's longer than 128 bits) being "don't care".

// While sub-optimal on machines that have SVE vector length > 128-bit - as the
// remainder of the vector is unused - this approach is still beneficial when
// compared to a Neon-only solution.

static inline int16x8_t svt_tbl2_s16(int16x8_t s0, int16x8_t s1, uint16x8_t tbl) {
    svint16x2_t samples = svcreate2_s16(svset_neonq_s16(svundef_s16(), s0), svset_neonq_s16(svundef_s16(), s1));
    return svget_neonq_s16(svtbl2_s16(samples, svset_neonq_u16(svundef_u16(), tbl)));
}

#endif // NEON_SVE2_BRIDGE_H
