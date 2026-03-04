/*
 *  Copyright (c) 2019, Alliance for Open Media. All Rights Reserved.
 *
 *  Use of this source code is governed by a BSD-style license
 *  that can be found in the LICENSE file in the root of the source
 *  tree. An additional intellectual property rights grant can be found
 *  in the file PATENTS.  All contributing project authors may
 *  be found in the AUTHORS file in the root of the source tree.
 */

#include <algorithm>
#include <ostream>

#include "aom_dsp_rtcd.h"
#include "test/acm_random.h"
#include "util.h"

namespace {

using libaom_test::ACMRandom;

using HadamardFunc = void (*)(const int16_t *a, ptrdiff_t a_stride, int32_t *b);

template <typename OutputType>
void Hadamard4x4(const OutputType *a, OutputType *out) {
    OutputType b[8];
    for (int i = 0; i < 4; i += 2) {
        b[i + 0] = (a[i * 4] + a[(i + 1) * 4]) >> 1;
        b[i + 1] = (a[i * 4] - a[(i + 1) * 4]) >> 1;
    }

    out[0] = b[0] + b[2];
    out[1] = b[1] + b[3];
    out[2] = b[0] - b[2];
    out[3] = b[1] - b[3];
}

template <typename OutputType>
void ReferenceHadamard4x4(const int16_t *a, int a_stride, OutputType *b) {
    OutputType input[16];
    OutputType buf[16];
    for (int i = 0; i < 4; ++i) {
        for (int j = 0; j < 4; ++j) {
            input[i * 4 + j] = static_cast<OutputType>(a[i * a_stride + j]);
        }
    }
    for (int i = 0; i < 4; ++i)
        Hadamard4x4(input + i, buf + i * 4);
    for (int i = 0; i < 4; ++i)
        Hadamard4x4(buf + i, b + i * 4);

    // Extra transpose to match C and SSE2 behavior(i.e., svt_aom_hadamard_4x4).
    for (int i = 0; i < 4; i++) {
        for (int j = i + 1; j < 4; j++) {
            OutputType temp = b[j * 4 + i];
            b[j * 4 + i] = b[i * 4 + j];
            b[i * 4 + j] = temp;
        }
    }
}

template <typename OutputType>
void HadamardLoop(const OutputType *a, OutputType *out) {
    OutputType b[8];
    for (int i = 0; i < 8; i += 2) {
        b[i + 0] = a[i * 8] + a[(i + 1) * 8];
        b[i + 1] = a[i * 8] - a[(i + 1) * 8];
    }
    OutputType c[8];
    for (int i = 0; i < 8; i += 4) {
        c[i + 0] = b[i + 0] + b[i + 2];
        c[i + 1] = b[i + 1] + b[i + 3];
        c[i + 2] = b[i + 0] - b[i + 2];
        c[i + 3] = b[i + 1] - b[i + 3];
    }
    out[0] = c[0] + c[4];
    out[7] = c[1] + c[5];
    out[3] = c[2] + c[6];
    out[4] = c[3] + c[7];
    out[2] = c[0] - c[4];
    out[6] = c[1] - c[5];
    out[1] = c[2] - c[6];
    out[5] = c[3] - c[7];
}

template <typename OutputType>
void ReferenceHadamard8x8(const int16_t *a, int a_stride, OutputType *b) {
    OutputType input[64];
    OutputType buf[64];
    for (int i = 0; i < 8; ++i) {
        for (int j = 0; j < 8; ++j) {
            input[i * 8 + j] = static_cast<OutputType>(a[i * a_stride + j]);
        }
    }
    for (int i = 0; i < 8; ++i)
        HadamardLoop(input + i, buf + i * 8);
    for (int i = 0; i < 8; ++i)
        HadamardLoop(buf + i, b + i * 8);
}

template <typename OutputType>
void ReferenceHadamard16x16(const int16_t *a, int a_stride, OutputType *b) {
    /* The source is a 16x16 block. The destination is rearranged to 8x32.
     * Input is 9 bit. */
    ReferenceHadamard8x8(a + 0 + 0 * a_stride, a_stride, b + 0);
    ReferenceHadamard8x8(a + 8 + 0 * a_stride, a_stride, b + 64);
    ReferenceHadamard8x8(a + 0 + 8 * a_stride, a_stride, b + 128);
    ReferenceHadamard8x8(a + 8 + 8 * a_stride, a_stride, b + 192);

    /* Overlay the 8x8 blocks and combine. */
    for (int i = 0; i < 64; ++i) {
        /* 8x8 steps the range up to 15 bits. */
        const OutputType a0 = b[0];
        const OutputType a1 = b[64];
        const OutputType a2 = b[128];
        const OutputType a3 = b[192];

        /* Prevent the result from escaping int16_t. */
        const OutputType b0 = (a0 + a1) >> 1;
        const OutputType b1 = (a0 - a1) >> 1;
        const OutputType b2 = (a2 + a3) >> 1;
        const OutputType b3 = (a2 - a3) >> 1;

        /* Store a 16 bit value. */
        b[0] = b0 + b2;
        b[64] = b1 + b3;
        b[128] = b0 - b2;
        b[192] = b1 - b3;

        ++b;
    }
}

template <typename OutputType>
void ReferenceHadamard32x32(const int16_t *a, int a_stride, OutputType *b) {
    ReferenceHadamard16x16(a + 0 + 0 * a_stride, a_stride, b + 0);
    ReferenceHadamard16x16(a + 16 + 0 * a_stride, a_stride, b + 256);
    ReferenceHadamard16x16(a + 0 + 16 * a_stride, a_stride, b + 512);
    ReferenceHadamard16x16(a + 16 + 16 * a_stride, a_stride, b + 768);

    for (int i = 0; i < 256; ++i) {
        const OutputType a0 = b[0];
        const OutputType a1 = b[256];
        const OutputType a2 = b[512];
        const OutputType a3 = b[768];

        const OutputType b0 = (a0 + a1) >> 2;
        const OutputType b1 = (a0 - a1) >> 2;
        const OutputType b2 = (a2 + a3) >> 2;
        const OutputType b3 = (a2 - a3) >> 2;

        b[0] = b0 + b2;
        b[256] = b1 + b3;
        b[512] = b0 - b2;
        b[768] = b1 - b3;

        ++b;
    }
}

template <typename OutputType>
void ReferenceHadamard(const int16_t *a, int a_stride, OutputType *b, int bwh) {
    if (bwh == 32) {
        ReferenceHadamard32x32(a, a_stride, b);
    } else if (bwh == 16) {
        ReferenceHadamard16x16(a, a_stride, b);
    } else if (bwh == 8) {
        ReferenceHadamard8x8(a, a_stride, b);
    } else if (bwh == 4) {
        ReferenceHadamard4x4(a, a_stride, b);
    } else {
        GTEST_FAIL() << "Invalid Hadamard transform size " << bwh << std::endl;
    }
}

template <typename HadamardFuncType>
struct FuncWithSize {
    FuncWithSize(HadamardFuncType f, int s) : func(f), block_size(s) {
    }
    HadamardFuncType func;
    int block_size;
};

using HadamardFuncWithSize = FuncWithSize<HadamardFunc>;

template <typename HadamardFuncType>
std::ostream &operator<<(std::ostream &os,
                         const FuncWithSize<HadamardFuncType> &hfs) {
    return os << "block size: " << hfs.block_size;
}

template <typename OutputType, typename HadamardFuncType>
class HadamardTestBase
    : public ::testing::TestWithParam<FuncWithSize<HadamardFuncType>> {
  public:
    explicit HadamardTestBase(const FuncWithSize<HadamardFuncType> &func_param)
        : bwh_(func_param.block_size),
          block_size_(bwh_ * bwh_),
          h_func_(func_param.func) {
    }

    virtual void SetUp() {
        rnd_.Reset(ACMRandom::DeterministicSeed());
    }

    virtual int16_t Rand() = 0;

    void CompareReferenceRandom() {
        const int kMaxBlockSize = 32 * 32;
        DECLARE_ALIGNED(16, int16_t, a[kMaxBlockSize]);
        DECLARE_ALIGNED(16, OutputType, b[kMaxBlockSize]);
        memset(a, 0, sizeof(a));
        memset(b, 0, sizeof(b));

        OutputType b_ref[kMaxBlockSize];
        memset(b_ref, 0, sizeof(b_ref));

        for (int i = 0; i < block_size_; ++i)
            a[i] = Rand();

        ReferenceHadamard(a, bwh_, b_ref, bwh_);
        h_func_(a, bwh_, b);

        // The order of the output is not important. Sort before checking.
        std::sort(b, b + block_size_);
        std::sort(b_ref, b_ref + block_size_);
        EXPECT_EQ(memcmp(b, b_ref, sizeof(b)), 0);
    }

    void VaryStride() {
        const int kMaxBlockSize = 32 * 32;
        DECLARE_ALIGNED(16, int16_t, a[kMaxBlockSize * 8]);
        DECLARE_ALIGNED(16, OutputType, b[kMaxBlockSize]);
        memset(a, 0, sizeof(a));
        for (int i = 0; i < block_size_ * 8; ++i)
            a[i] = Rand();

        OutputType b_ref[kMaxBlockSize];
        for (int i = 8; i < 64; i += 8) {
            memset(b, 0, sizeof(b));
            memset(b_ref, 0, sizeof(b_ref));

            ReferenceHadamard(a, i, b_ref, bwh_);
            h_func_(a, i, b);

            // The order of the output is not important. Sort before checking.
            std::sort(b, b + block_size_);
            std::sort(b_ref, b_ref + block_size_);
            EXPECT_EQ(0, memcmp(b, b_ref, sizeof(b)));
        }
    }

    ACMRandom rnd_;

  private:
    int bwh_;
    int block_size_;
    HadamardFuncType h_func_;
};

class HadamardLowbdTest : public HadamardTestBase<int32_t, HadamardFunc> {
  public:
    HadamardLowbdTest() : HadamardTestBase(GetParam()) {
    }
    virtual int16_t Rand() {
        return rnd_.Rand9Signed();
    }
};

TEST_P(HadamardLowbdTest, CompareReferenceRandom) {
    CompareReferenceRandom();
}

TEST_P(HadamardLowbdTest, VaryStride) {
    VaryStride();
}

INSTANTIATE_TEST_SUITE_P(
    C, HadamardLowbdTest,
    ::testing::Values(HadamardFuncWithSize(&svt_aom_hadamard_4x4_c, 4),
                      HadamardFuncWithSize(&svt_aom_hadamard_8x8_c, 8),
                      HadamardFuncWithSize(&svt_aom_hadamard_16x16_c, 16),
                      HadamardFuncWithSize(&svt_aom_hadamard_32x32_c, 32)));

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    AVX2, HadamardLowbdTest,
    ::testing::Values(HadamardFuncWithSize(&svt_aom_hadamard_4x4_sse2, 4),
                      HadamardFuncWithSize(&svt_aom_hadamard_8x8_sse2, 8),
                      HadamardFuncWithSize(&svt_aom_hadamard_16x16_avx2, 16),
                      HadamardFuncWithSize(&svt_aom_hadamard_32x32_avx2, 32)));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, HadamardLowbdTest,
    ::testing::Values(HadamardFuncWithSize(&svt_aom_hadamard_4x4_neon, 4),
                      HadamardFuncWithSize(&svt_aom_hadamard_8x8_neon, 8),
                      HadamardFuncWithSize(&svt_aom_hadamard_16x16_neon, 16),
                      HadamardFuncWithSize(&svt_aom_hadamard_32x32_neon, 32)));
#endif  // ARCH_AARCH64

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
class HadamardHighbdTest : public HadamardTestBase<int32_t, HadamardFunc> {
  protected:
    HadamardHighbdTest() : HadamardTestBase(GetParam()) {
    }
    // Use values between -4095 (0xF001) and 4095 (0x0FFF)
    int16_t Rand() override {
        int16_t src = rnd_.Rand12();
        int16_t pred = rnd_.Rand12();
        return src - pred;
    }
};

TEST_P(HadamardHighbdTest, CompareReferenceRandom) {
    CompareReferenceRandom();
}

TEST_P(HadamardHighbdTest, VaryStride) {
    VaryStride();
}

INSTANTIATE_TEST_SUITE_P(
    C, HadamardHighbdTest,
    ::testing::Values(HadamardFuncWithSize(&svt_aom_highbd_hadamard_8x8_c, 8)));

#if ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(AVX2, HadamardHighbdTest,
                         ::testing::Values(HadamardFuncWithSize(
                             &svt_aom_highbd_hadamard_8x8_avx2, 8)));
#endif  // ARCH_X86_64

#if ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(NEON, HadamardHighbdTest,
                         ::testing::Values(HadamardFuncWithSize(
                             &svt_aom_highbd_hadamard_8x8_neon, 8)));
#endif  // ARCH_AARCH64

#endif  // CONFIG_ENABLE_HIGH_BIT_DEPTH

}  // namespace
