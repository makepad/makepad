// clang-format off
/*
* Copyright(c) 2019 Intel Corporation
* Copyright (c) 2016, Alliance for Open Media. All rights reserved
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#if HAVE_VALGRIND_H
#include <valgrind/valgrind.h>
#else
// assume the system doesn't have access to valgrind if the header is missing
#define RUNNING_ON_VALGRIND 0
#endif

#define RTCD_C
#include "common_dsp_rtcd.h"
#include "pic_operators.h"
#include "pack_unpack_c.h"
#include "utility.h"
#include "svt_threads.h"

#if defined ARCH_X86_64
#define AOM_ARCH_X86_64 ARCH_X86_64
#include "third_party/aom/aom_ports/x86.h"
#endif

#if defined ARCH_AARCH64

#if defined(__linux__) || HAVE_ELF_AUX_INFO
// For reading the HWCAP flags
#include <sys/auxv.h>
#elif defined(__APPLE__)
#include <stdbool.h>
#include <sys/sysctl.h>
#elif defined(_MSC_VER)
#include <windows.h>
#endif
#endif // ARCH_AARCH64

// coeff: 16 bits, dynamic range [-32640, 32640].
// length: value range {16, 64, 256, 1024}.
int svt_aom_satd_c(const TranLow *coeff, int length) {
    int i;
    int satd = 0;
    for (i = 0; i < length; ++i) satd += abs(coeff[i]);

    // satd: 26 bits, dynamic range [-32640 * 1024, 32640 * 1024]
    return satd;
}

int64_t svt_av1_block_error_c(const TranLow *coeff, const TranLow *dqcoeff, intptr_t block_size, int64_t *ssz) {
    int     i;
    int64_t error = 0, sqcoeff = 0;

    for (i = 0; i < block_size; i++) {
        error += SQR(coeff[i] - dqcoeff[i]);
        sqcoeff += SQR(coeff[i]);
    }

    *ssz = sqcoeff;
    return error;
}

/**************************************
 * Instruction Set Support
 **************************************/
#ifdef ARCH_X86_64
EbCpuFlags svt_aom_get_cpu_flags() {
    EbCpuFlags flags     = 0;
    const int  aom_flags = x86_simd_caps();

    flags |= (aom_flags & HAS_MMX) ? EB_CPU_FLAGS_MMX : 0;
    flags |= (aom_flags & HAS_SSE) ? EB_CPU_FLAGS_SSE : 0;
    flags |= (aom_flags & HAS_SSE2) ? EB_CPU_FLAGS_SSE2 : 0;
    flags |= (aom_flags & HAS_SSE3) ? EB_CPU_FLAGS_SSE3 : 0;
    flags |= (aom_flags & HAS_SSSE3) ? EB_CPU_FLAGS_SSSE3 : 0;
    flags |= (aom_flags & HAS_SSE4_1) ? EB_CPU_FLAGS_SSE4_1 : 0;
    flags |= (aom_flags & HAS_SSE4_2) ? EB_CPU_FLAGS_SSE4_2 : 0;
    flags |= (aom_flags & HAS_AVX) ? EB_CPU_FLAGS_AVX : 0;
    flags |= (aom_flags & HAS_AVX2) ? EB_CPU_FLAGS_AVX2 : 0;
    // aom checks for {f,dq,cd,bw,vl} and also {vbmi,vbmi2,gfni,vaes,vpclmulqdq,vnni,bitalg,popcntdq}
    // for avx512 availability. We have the two sections separated since we still need to be able to
    // test avx512 on skylake, which has only the first set of features.
    const EbCpuFlags avx512_flags = EB_CPU_FLAGS_AVX512F | EB_CPU_FLAGS_AVX512CD | EB_CPU_FLAGS_AVX512DQ |
        EB_CPU_FLAGS_AVX512BW | EB_CPU_FLAGS_AVX512VL;
    flags |= (aom_flags & HAS_AVX512) ? avx512_flags : 0;
    flags |= (aom_flags & HAS_AVX512_DL) ? EB_CPU_FLAGS_AVX512ICL : 0;

    return flags;
}

EbCpuFlags svt_aom_get_cpu_flags_to_use() {
    EbCpuFlags flags = svt_aom_get_cpu_flags();
#if !EN_AVX512_SUPPORT
    /* Remove AVX512 flags. */
    flags &= (EB_CPU_FLAGS_AVX512F - 1);
#endif
    return flags;
}
#else
#if defined ARCH_AARCH64

#if defined(__linux__) || HAVE_ELF_AUX_INFO

static inline uint64_t read_midr_el1(void) {
    uint64_t v;
    __asm__ volatile ("mrs %0, midr_el1" : "=r"(v));
    return v;
}

// Define hwcap values ourselves: building with an old auxv header where these
// hwcap values are not defined should not prevent features from being enabled.
#define AOM_AARCH64_HWCAP_NEON (1 << 1)
#define AOM_AARCH64_HWCAP_CRC32 (1 << 7)
#define AOM_AARCH64_HWCAP_ASIMDDP (1 << 20)
#define AOM_AARCH64_HWCAP_SVE (1 << 22)
#define AOM_AARCH64_HWCAP2_SVE2 (1 << 1)
#define AOM_AARCH64_HWCAP2_I8MM (1 << 13)

EbCpuFlags svt_aom_get_cpu_flags(void) {
#if HAVE_ARM_CRC32 || HAVE_NEON_DOTPROD || HAVE_SVE
#if HAVE_ELF_AUX_INFO
    unsigned long hwcap = 0;
    elf_aux_info(AT_HWCAP, &hwcap, sizeof(hwcap));
#else
    unsigned long hwcap = getauxval(AT_HWCAP);
#endif
#endif
#if HAVE_NEON_I8MM || HAVE_SVE2
#if HAVE_ELF_AUX_INFO
    unsigned long hwcap2 = 0;
    elf_aux_info(AT_HWCAP2, &hwcap2, sizeof(hwcap2));
#else
    unsigned long hwcap2 = getauxval(AT_HWCAP2);
#endif
#endif

#if CONFIG_ARM_NEON_IS_GUARANTEED
    EbCpuFlags flags = EB_CPU_FLAGS_NEON; // Neon is mandatory in Armv8.0-A.
#else
    EbCpuFlags flags = 0;
    if (hwcap & AOM_AARCH64_HWCAP_NEON)
        flags |= EB_CPU_FLAGS_NEON;
#endif

#if HAVE_ARM_CRC32
    if (hwcap & AOM_AARCH64_HWCAP_CRC32)
        flags |= EB_CPU_FLAGS_ARM_CRC32;
#endif // HAVE_ARM_CRC32
#if HAVE_NEON_DOTPROD
    if (hwcap & AOM_AARCH64_HWCAP_ASIMDDP)
        flags |= EB_CPU_FLAGS_NEON_DOTPROD;
#endif // HAVE_NEON_DOTPROD
#if HAVE_NEON_I8MM
    if (hwcap2 & AOM_AARCH64_HWCAP2_I8MM)
        flags |= EB_CPU_FLAGS_NEON_I8MM;
#endif // HAVE_NEON_I8MM
#if HAVE_SVE
    if (hwcap & AOM_AARCH64_HWCAP_SVE)
        flags |= EB_CPU_FLAGS_SVE;
#endif // HAVE_SVE
#if HAVE_SVE2
    if (hwcap2 & AOM_AARCH64_HWCAP2_SVE2)
        flags |= EB_CPU_FLAGS_SVE2;
#endif // HAVE_SVE2

    const uint64_t midr = read_midr_el1();
    const unsigned implementer = (midr >> 24) & 0xFF;   // [31:24]
    const unsigned partnum     = (midr >> 4)  & 0xFFF;  // [15:4]

    if (implementer == 0x41 && partnum == 0xD4F) {
      flags |= EB_CPU_FLAGS_NEOVERSE_V2;
    }

    return flags;
}

#elif defined(__APPLE__) // end __linux__

// sysctlbyname() parameter documentation for instruction set characteristics:
// https://developer.apple.com/documentation/kernel/1387446-sysctlbyname/determining_instruction_set_characteristics
#if HAVE_ARM_CRC32 || HAVE_NEON_DOTPROD || HAVE_NEON_I8MM
static INLINE bool have_feature(const char *feature) {
    int64_t feature_present = 0;
    size_t  size            = sizeof(feature_present);
    if (sysctlbyname(feature, &feature_present, &size, NULL, 0) != 0) {
        return false;
    }
    return feature_present;
}
#endif

EbCpuFlags svt_aom_get_cpu_flags(void) {
#if CONFIG_ARM_NEON_IS_GUARANTEED
    EbCpuFlags flags = EB_CPU_FLAGS_NEON;
#else
    EbCpuFlags flags = 0;
    if (have_feature("hw.optional.neon"))
        flags |= EB_CPU_FLAGS_NEON;
#endif
#if HAVE_ARM_CRC32
    if (have_feature("hw.optional.armv8_crc32"))
        flags |= EB_CPU_FLAGS_ARM_CRC32;
#endif // HAVE_ARM_CRC32
#if HAVE_NEON_DOTPROD
    if (have_feature("hw.optional.arm.FEAT_DotProd"))
        flags |= EB_CPU_FLAGS_NEON_DOTPROD;
#endif // HAVE_NEON_DOTPROD
#if HAVE_NEON_I8MM
    if (have_feature("hw.optional.arm.FEAT_I8MM"))
        flags |= EB_CPU_FLAGS_NEON_I8MM;
#endif // HAVE_NEON_I8MM
    return flags;
}

#elif defined(_MSC_VER) // end __APPLE__

// IsProcessorFeaturePresent() parameter documentation:
// https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-isprocessorfeaturepresent#parameters
EbCpuFlags svt_aom_get_cpu_flags(void) {
#if CONFIG_ARM_NEON_IS_GUARANTEED
    EbCpuFlags flags = EB_CPU_FLAGS_NEON; // Neon is mandatory in Armv8.0-A.
#else
    EbCpuFlags flags = 0;
    if (IsProcessorFeaturePresent(PF_ARM_V8_INSTRUCTIONS_AVAILABLE)) {
        flags |= EB_CPU_FLAGS_NEON;
    }
#endif
#if HAVE_ARM_CRC32
    if (IsProcessorFeaturePresent(PF_ARM_V8_CRC32_INSTRUCTIONS_AVAILABLE)) {
        flags |= EB_CPU_FLAGS_ARM_CRC32;
    }
#endif // HAVE_ARM_CRC32
#if HAVE_NEON_DOTPROD
// Support for PF_ARM_V82_DP_INSTRUCTIONS_AVAILABLE was added in Windows SDK
// 20348, supported by Windows 11 and Windows Server 2022.
#if defined(PF_ARM_V82_DP_INSTRUCTIONS_AVAILABLE)
    if (IsProcessorFeaturePresent(PF_ARM_V82_DP_INSTRUCTIONS_AVAILABLE)) {
        flags |= EB_CPU_FLAGS_NEON_DOTPROD;
    }
#endif // defined(PF_ARM_V82_DP_INSTRUCTIONS_AVAILABLE)
#endif // HAVE_NEON_DOTPROD
    // No I8MM or SVE feature detection available on Windows at time of writing.
    return flags;
}

#else // end _MSC_VER

EbCpuFlags svt_aom_get_cpu_flags() {
    EbCpuFlags flags = 0;

    // safe to call multiple times, and threadsafe

#if CONFIG_ARM_NEON_IS_GUARANTEED
    flags |= EB_CPU_FLAGS_NEON;
#endif

    return flags;
}

#endif

EbCpuFlags svt_aom_get_cpu_flags_to_use() {
    EbCpuFlags flags = svt_aom_get_cpu_flags();

    // Restrict flags: FEAT_I8MM assumes that FEAT_DotProd is available.
    if (!(flags & EB_CPU_FLAGS_NEON_DOTPROD))
        flags &= ~EB_CPU_FLAGS_NEON_I8MM;

    // Restrict flags: SVE assumes that FEAT_{DotProd,I8MM} are available.
    if (!(flags & EB_CPU_FLAGS_NEON_DOTPROD))
        flags &= ~EB_CPU_FLAGS_SVE;
    if (!(flags & EB_CPU_FLAGS_NEON_I8MM))
        flags &= ~EB_CPU_FLAGS_SVE;

    // Restrict flags: SVE2 assumes that FEAT_SVE is available.
    if (!(flags & EB_CPU_FLAGS_SVE))
        flags &= ~EB_CPU_FLAGS_SVE2;

    return flags;
}

#else
EbCpuFlags svt_aom_get_cpu_flags_to_use() { return 0; }
#endif
#endif

/* Macros SET_* use local variable EbCpuFlags flags and bool check_pointer_was_set */
#define SET_FUNCTION(ptr, func, flag)                             \
    if ((uintptr_t)NULL != (uintptr_t)(func) && (flags & (flag))) \
        ptr = func;

#ifdef ARCH_X86_64
#if EN_AVX512_SUPPORT
#define SET_FUNCTION_AVX512(ptr, avx512) SET_FUNCTION(ptr, avx512, EB_CPU_FLAGS_AVX512F)
#else /* EN_AVX512_SUPPORT */
#define SET_FUNCTION_AVX512(ptr, avx512)
#endif /* EN_AVX512_SUPPORT */

#define SET_FUNCTIONS_X86(ptr, mmx, sse, sse2, sse3, ssse3, sse4_1, sse4_2, avx, avx2, avx512) \
    SET_FUNCTION(ptr, mmx, EB_CPU_FLAGS_MMX)                                                   \
    SET_FUNCTION(ptr, sse, EB_CPU_FLAGS_SSE)                                                   \
    SET_FUNCTION(ptr, sse2, EB_CPU_FLAGS_SSE2)                                                 \
    SET_FUNCTION(ptr, sse3, EB_CPU_FLAGS_SSE3)                                                 \
    SET_FUNCTION(ptr, ssse3, EB_CPU_FLAGS_SSSE3)                                               \
    SET_FUNCTION(ptr, sse4_1, EB_CPU_FLAGS_SSE4_1)                                             \
    SET_FUNCTION(ptr, sse4_2, EB_CPU_FLAGS_SSE4_2)                                             \
    SET_FUNCTION(ptr, avx, EB_CPU_FLAGS_AVX)                                                   \
    SET_FUNCTION(ptr, avx2, EB_CPU_FLAGS_AVX2)                                                 \
    SET_FUNCTION_AVX512(ptr, avx512)
#elif defined ARCH_AARCH64

#if HAVE_NEON_DOTPROD
#define SET_FUNCTION_NEON_DOTPROD(ptr, neon_dotprod) SET_FUNCTION(ptr, neon_dotprod, EB_CPU_FLAGS_NEON_DOTPROD)
#else // HAVE_NEON_DOTPROD
#define SET_FUNCTION_NEON_DOTPROD(ptr, neon_dotprod)
#endif // HAVE_NEON_DOTPROD

#if HAVE_NEON_I8MM
#define SET_FUNCTION_NEON_I8MM(ptr, neon_i8mm) SET_FUNCTION(ptr, neon_i8mm, EB_CPU_FLAGS_NEON_I8MM)
#else // HAVE_NEON_I8MM
#define SET_FUNCTION_NEON_I8MM(ptr, neon_i8mm)
#endif // HAVE_NEON_I8MM

#if HAVE_SVE
#define SET_FUNCTION_SVE(ptr, sve) SET_FUNCTION(ptr, sve, EB_CPU_FLAGS_SVE)
#else
#define SET_FUNCTION_SVE(ptr, sve)
#endif // HAVE_SVE

#if HAVE_SVE2
#define SET_FUNCTION_SVE2(ptr, sve2) SET_FUNCTION(ptr, sve2, EB_CPU_FLAGS_SVE2)
#else
#define SET_FUNCTION_SVE2(ptr, sve2)
#endif // HAVE_SVE2

#define SET_FUNCTIONS_AARCH64(ptr, neon, neon_dotprod, neon_i8mm, sve, sve2) \
    SET_FUNCTION(ptr, neon, EB_CPU_FLAGS_NEON)                               \
    SET_FUNCTION_NEON_DOTPROD(ptr, neon_dotprod)                             \
    SET_FUNCTION_NEON_I8MM(ptr, neon_i8mm)                                   \
    SET_FUNCTION_SVE(ptr, sve)                                               \
    SET_FUNCTION_SVE2(ptr, sve2)
#endif

#define CHECK_PTR_IS_NOT_SET(ptr)                                                         \
    if (check_pointer_was_set && (uintptr_t)NULL != (uintptr_t)ptr) {                     \
        SVT_ERROR("%s:%i: Pointer \"%s\" is set before!\n", __FILE__, EB_LINE_NUM, #ptr); \
        assert(0);                                                                        \
    }

#define CHECK_PTR_IS_SET(ptr)                                                               \
    if ((uintptr_t)NULL == (uintptr_t)ptr) {                                                \
        SVT_ERROR("%s:%i: Pointer \"%s\" is not assigned!\n", __FILE__, EB_LINE_NUM, #ptr); \
        assert(0);                                                                          \
    }

#define SET_FUNCTION_C(ptr, c)                                                           \
    if ((uintptr_t)NULL == (uintptr_t)c) {                                               \
        SVT_ERROR("%s:%i: Pointer \"%s\" on C is NULL!\n", __FILE__, EB_LINE_NUM, #ptr); \
        assert(0);                                                                       \
    }                                                                                    \
    ptr = c;

#ifdef ARCH_X86_64
// general function dispatcher
#define SET_FUNCTIONS(ptr, c, mmx, sse, sse2, sse3, ssse3, sse4_1, sse4_2, avx, avx2, avx512)  \
    do {                                                                                       \
        CHECK_PTR_IS_NOT_SET(ptr)                                                              \
        SET_FUNCTION_C(ptr, c)                                                                 \
        SET_FUNCTIONS_X86(ptr, mmx, sse, sse2, sse3, ssse3, sse4_1, sse4_2, avx, avx2, avx512) \
        CHECK_PTR_IS_SET(ptr)                                                                  \
    } while (0)

// special case when any optimization up to AVX2 is available
#if CONFIG_X86_AVX2_IS_GUARANTEED
// when AVX2 is guaranteed to be available - we can skip C function assignment
// and thus allow linker to strip C code from final binary to reduce size.
#define SET_FUNCTIONS_AVX2(ptr, c, mmx, sse, sse2, sse3, ssse3, sse4_1, sse4_2, avx, avx2, avx512) \
    do {                                                                                           \
        CHECK_PTR_IS_NOT_SET(ptr)                                                                  \
        SET_FUNCTIONS_X86(ptr, neon, neon_dotprod, neon_i8mm, sve, sve2)                           \
        CHECK_PTR_IS_SET(ptr)                                                                      \
    } while (0)
#else
#define SET_FUNCTIONS_AVX2(ptr, c, mmx, sse, sse2, sse3, ssse3, sse4_1, sse4_2, avx, avx2, avx512) \
    SET_FUNCTIONS(ptr, c, mmx, sse, sse2, sse3, ssse3, sse4_1, sse4_2, avx, avx2, avx512)
#endif

#elif defined ARCH_AARCH64

// general function dispatcher
#define SET_FUNCTIONS(ptr, c, neon, neon_dotprod, neon_i8mm, sve, sve2)      \
    do {                                                                     \
        CHECK_PTR_IS_NOT_SET(ptr)                                            \
        SET_FUNCTION_C(ptr, c)                                               \
        SET_FUNCTIONS_AARCH64(ptr, neon, neon_dotprod, neon_i8mm, sve, sve2) \
        CHECK_PTR_IS_SET(ptr)                                                \
    } while (0)

// special case when Neon optimization is available
#if CONFIG_ARM_NEON_IS_GUARANTEED
// when Neon is guaranteed to be available - we can skip C function assignment
// and thus allow linker to strip C code from final binary to reduce size.
#define SET_FUNCTIONS_NEON(ptr, c, neon, neon_dotprod, neon_i8mm, sve, sve2) \
    do {                                                                     \
        CHECK_PTR_IS_NOT_SET(ptr)                                            \
        SET_FUNCTIONS_AARCH64(ptr, neon, neon_dotprod, neon_i8mm, sve, sve2) \
        CHECK_PTR_IS_SET(ptr)                                                \
    } while (0)
#else
#define SET_FUNCTIONS_NEON(ptr, c, neon, neon_dotprod, neon_i8mm, sve, sve2) \
    SET_FUNCTIONS(ptr, c, neon, neon_dotprod, neon_i8mm, sve, sve2)
#endif
#endif

#define SET_ONLY_C(ptr, c)        \
    do {                          \
        CHECK_PTR_IS_NOT_SET(ptr) \
        SET_FUNCTION_C(ptr, c)    \
        CHECK_PTR_IS_SET(ptr)     \
    } while (0)

#ifdef ARCH_X86_64
#define SET_SSE2(ptr, c, sse2)                                  SET_FUNCTIONS_AVX2(ptr, c, 0, 0, sse2, 0, 0, 0, 0, 0, 0, 0)
#define SET_SSE2_AVX2(ptr, c, sse2, avx2)                       SET_FUNCTIONS_AVX2(ptr, c, 0, 0, sse2, 0, 0, 0, 0, 0, avx2, 0)
#define SET_SSE2_AVX512(ptr, c, sse2, avx512)                   SET_FUNCTIONS_AVX2(ptr, c, 0, 0, sse2, 0, 0, 0, 0, 0, 0, avx512)
#define SET_SSSE3(ptr, c, ssse3)                                SET_FUNCTIONS_AVX2(ptr, c, 0, 0, 0, 0, ssse3, 0, 0, 0, 0, 0)
#define SET_SSE41(ptr, c, sse4_1)                               SET_FUNCTIONS_AVX2(ptr, c, 0, 0, 0, 0, 0, sse4_1, 0, 0, 0, 0)
#define SET_SSE41_AVX2(ptr, c, sse4_1, avx2)                    SET_FUNCTIONS_AVX2(ptr, c, 0, 0, 0, 0, 0, sse4_1, 0, 0, avx2, 0)
#define SET_SSE41_AVX2_AVX512(ptr, c, sse4_1, avx2, avx512)     SET_FUNCTIONS_AVX2(ptr, c, 0, 0, 0, 0, 0, sse4_1, 0, 0, avx2, avx512)
#define SET_AVX2(ptr, c, avx2)                                  SET_FUNCTIONS_AVX2(ptr, c, 0, 0, 0, 0, 0, 0, 0, 0, avx2, 0)
#define SET_AVX2_AVX512(ptr, c, avx2, avx512)                   SET_FUNCTIONS_AVX2(ptr, c, 0, 0, 0, 0, 0, 0, 0, 0, avx2, avx512)
#define SET_SSE2_AVX2_AVX512(ptr, c, sse2, avx2, avx512)        SET_FUNCTIONS_AVX2(ptr, c, 0, 0, sse2, 0, 0, 0, 0, 0, avx2, avx512)
#define SET_SSE2_SSSE3_AVX2_AVX512(ptr, c, sse2, ssse3, avx2, avx512) SET_FUNCTIONS_AVX2(ptr, c, 0, 0, sse2, 0, ssse3, 0, 0, 0, avx2, avx512)
#define SET_SSSE3_AVX2(ptr, c, ssse3, avx2)                     SET_FUNCTIONS_AVX2(ptr, c, 0, 0, 0, 0, ssse3, 0, 0, 0, avx2, 0)
#elif defined ARCH_AARCH64
#define SET_NEON(ptr, c, neon)                                  SET_FUNCTIONS_NEON(ptr, c, neon, 0, 0, 0, 0)
#define SET_NEON_NEON_DOTPROD(ptr, c, neon, neon_dotprod)       SET_FUNCTIONS_NEON(ptr, c, neon, neon_dotprod, 0, 0, 0)
#define SET_NEON_NEON_DOTPROD_NEON_I8MM(ptr, c, neon, neon_dotprod, neon_i8mm) SET_FUNCTIONS_NEON(ptr, c, neon, neon_dotprod, neon_i8mm, 0, 0)
#define SET_NEON_NEON_I8MM(ptr, c, neon, neon_i8mm)             SET_FUNCTIONS_NEON(ptr, c, neon, 0, neon_i8mm, 0, 0)
#define SET_NEON_NEON_I8MM_SVE(ptr, c, neon, neon_i8mm, sve)    SET_FUNCTIONS_NEON(ptr, c, neon, 0, neon_i8mm, sve, 0)
#define SET_NEON_SVE(ptr, c, neon, sve)                         SET_FUNCTIONS_NEON(ptr, c, neon, 0, 0, sve, 0)
#define SET_NEON_SVE2(ptr, c, neon, sve2)                       SET_FUNCTIONS_NEON(ptr, c, neon, 0, 0, 0, sve2)
#endif

// Thread-safe RTCD initialization using lazily-initialized mutex
DEFINE_ONCE_MUTEX(common_rtcd_init_mutex);

void svt_aom_setup_common_rtcd_internal(EbCpuFlags flags) {
    RUN_ONCE_MUTEX(common_rtcd_init_mutex);
    svt_block_on_mutex(common_rtcd_init_mutex);

    /* Avoid check that pointer is set double, after first setup. */
    static bool first_call_setup = true;
    bool        check_pointer_was_set = first_call_setup;
    first_call_setup = false;
    /** Should be done during library initialization,
        but for safe limiting cpu flags again. */
#if defined ARCH_X86_64 || defined ARCH_AARCH64
    flags &= svt_aom_get_cpu_flags_to_use();
#else
    flags = 0;
    //to use C: flags=0
#endif

#ifdef ARCH_X86_64
    SET_SSE41_AVX2(svt_aom_blend_a64_mask, svt_aom_blend_a64_mask_c, svt_aom_blend_a64_mask_sse4_1, svt_aom_blend_a64_mask_avx2);
    SET_SSE41_AVX2(svt_aom_blend_a64_hmask, svt_aom_blend_a64_hmask_c, svt_aom_blend_a64_hmask_sse4_1, svt_av1_blend_a64_hmask_avx2);
    SET_SSE41_AVX2(svt_aom_blend_a64_vmask, svt_aom_blend_a64_vmask_c, svt_aom_blend_a64_vmask_sse4_1, svt_av1_blend_a64_vmask_avx2);
    SET_SSE41_AVX2(svt_aom_lowbd_blend_a64_d16_mask, svt_aom_lowbd_blend_a64_d16_mask_c, svt_aom_lowbd_blend_a64_d16_mask_sse4_1, svt_aom_lowbd_blend_a64_d16_mask_avx2);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_SSE41(svt_aom_highbd_blend_a64_mask, svt_aom_highbd_blend_a64_mask_c, svt_aom_highbd_blend_a64_mask_8bit_sse4_1);
    SET_SSE41_AVX2(svt_aom_highbd_blend_a64_vmask_16bit, svt_aom_highbd_blend_a64_vmask_16bit_c, svt_aom_highbd_blend_a64_vmask_16bit_sse4_1, svt_av1_highbd_blend_a64_vmask_16bit_avx2);
    SET_SSE41_AVX2(svt_aom_highbd_blend_a64_hmask_16bit, svt_aom_highbd_blend_a64_hmask_16bit_c, svt_aom_highbd_blend_a64_hmask_16bit_sse4_1, svt_av1_highbd_blend_a64_hmask_16bit_avx2);
    SET_SSE41_AVX2(svt_aom_highbd_blend_a64_d16_mask, svt_aom_highbd_blend_a64_d16_mask_c, svt_aom_highbd_blend_a64_d16_mask_sse4_1, svt_aom_highbd_blend_a64_d16_mask_avx2);
#endif
    SET_AVX2(svt_cfl_predict_lbd, svt_cfl_predict_lbd_c, svt_cfl_predict_lbd_avx2);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_AVX2(svt_cfl_predict_hbd, svt_cfl_predict_hbd_c, svt_cfl_predict_hbd_avx2);
#endif
    SET_SSE41(svt_av1_filter_intra_predictor, svt_av1_filter_intra_predictor_c, svt_av1_filter_intra_predictor_sse4_1);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_SSE41(svt_av1_filter_intra_edge_high, svt_av1_filter_intra_edge_high_c, svt_av1_filter_intra_edge_high_sse4_1);
#endif
    SET_SSE41(svt_av1_filter_intra_edge, svt_av1_filter_intra_edge_c, svt_av1_filter_intra_edge_sse4_1);
    SET_SSE41(svt_av1_upsample_intra_edge, svt_av1_upsample_intra_edge_c, svt_av1_upsample_intra_edge_sse4_1);
    SET_SSE41_AVX2(svt_av1_build_compound_diffwtd_mask_d16, svt_av1_build_compound_diffwtd_mask_d16_c, svt_av1_build_compound_diffwtd_mask_d16_sse4_1, svt_av1_build_compound_diffwtd_mask_d16_avx2);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_SSSE3_AVX2(svt_av1_highbd_wiener_convolve_add_src, svt_av1_highbd_wiener_convolve_add_src_c, svt_av1_highbd_wiener_convolve_add_src_ssse3, svt_av1_highbd_wiener_convolve_add_src_avx2);
#endif
    SET_SSE41_AVX2(svt_apply_selfguided_restoration, svt_apply_selfguided_restoration_c, svt_apply_selfguided_restoration_sse4_1, svt_apply_selfguided_restoration_avx2);
    SET_SSE41_AVX2(svt_av1_selfguided_restoration, svt_av1_selfguided_restoration_c, svt_av1_selfguided_restoration_sse4_1, svt_av1_selfguided_restoration_avx2);
    SET_SSE41_AVX2(svt_av1_inv_txfm2d_add_4x4, svt_av1_inv_txfm2d_add_4x4_c, svt_av1_inv_txfm2d_add_4x4_sse4_1, svt_dav1d_inv_txfm2d_add_4x4_avx2);
    SET_AVX2(svt_av1_inv_txfm2d_add_4x8, svt_av1_inv_txfm2d_add_4x8_c, svt_dav1d_inv_txfm2d_add_4x8_avx2);
    SET_AVX2(svt_av1_inv_txfm2d_add_4x16, svt_av1_inv_txfm2d_add_4x16_c, svt_dav1d_inv_txfm2d_add_4x16_avx2);
    SET_AVX2(svt_av1_inv_txfm2d_add_8x4, svt_av1_inv_txfm2d_add_8x4_c, svt_dav1d_inv_txfm2d_add_8x4_avx2);
    SET_SSE41_AVX2(svt_av1_inv_txfm2d_add_8x8, svt_av1_inv_txfm2d_add_8x8_c, svt_av1_inv_txfm2d_add_8x8_sse4_1, svt_dav1d_inv_txfm2d_add_8x8_avx2);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_SSE41_AVX2(svt_av1_inv_txfm2d_add_8x16, svt_av1_inv_txfm2d_add_8x16_c, svt_av1_highbd_inv_txfm_add_sse4_1, svt_dav1d_highbd_inv_txfm_add_avx2);
    SET_SSE41_AVX2(svt_av1_inv_txfm2d_add_8x32, svt_av1_inv_txfm2d_add_8x32_c, svt_av1_highbd_inv_txfm_add_sse4_1, svt_dav1d_highbd_inv_txfm_add_avx2);
#endif
    SET_AVX2(svt_av1_inv_txfm2d_add_16x4, svt_av1_inv_txfm2d_add_16x4_c, svt_dav1d_inv_txfm2d_add_16x4_avx2);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_SSE41_AVX2(svt_av1_inv_txfm2d_add_16x8, svt_av1_inv_txfm2d_add_16x8_c, svt_av1_highbd_inv_txfm_add_sse4_1, svt_dav1d_highbd_inv_txfm_add_avx2);
#endif
    SET_SSE41_AVX2_AVX512(svt_av1_inv_txfm2d_add_16x16, svt_av1_inv_txfm2d_add_16x16_c, svt_av1_inv_txfm2d_add_16x16_sse4_1, svt_dav1d_inv_txfm2d_add_16x16_avx2, svt_av1_inv_txfm2d_add_16x16_avx512);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_SSE41_AVX2_AVX512(svt_av1_inv_txfm2d_add_16x32, svt_av1_inv_txfm2d_add_16x32_c, svt_av1_highbd_inv_txfm_add_sse4_1, svt_dav1d_highbd_inv_txfm_add_avx2, svt_av1_inv_txfm2d_add_16x32_avx512);
    SET_SSE41_AVX2_AVX512(svt_av1_inv_txfm2d_add_16x64, svt_av1_inv_txfm2d_add_16x64_c, svt_av1_highbd_inv_txfm_add_sse4_1, svt_dav1d_highbd_inv_txfm_add_avx2, svt_av1_inv_txfm2d_add_16x64_avx512);
    SET_SSE41_AVX2(svt_av1_inv_txfm2d_add_32x8, svt_av1_inv_txfm2d_add_32x8_c, svt_av1_highbd_inv_txfm_add_sse4_1, svt_dav1d_highbd_inv_txfm_add_avx2);
    SET_SSE41_AVX2_AVX512(svt_av1_inv_txfm2d_add_32x16, svt_av1_inv_txfm2d_add_32x16_c, svt_av1_highbd_inv_txfm_add_sse4_1, svt_dav1d_highbd_inv_txfm_add_avx2, svt_av1_inv_txfm2d_add_32x16_avx512);
#endif
    SET_SSE41_AVX2_AVX512(svt_av1_inv_txfm2d_add_32x32, svt_av1_inv_txfm2d_add_32x32_c, svt_av1_inv_txfm2d_add_32x32_sse4_1, svt_dav1d_inv_txfm2d_add_32x32_avx2, svt_av1_inv_txfm2d_add_32x32_avx512);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_SSE41_AVX2_AVX512(svt_av1_inv_txfm2d_add_32x64, svt_av1_inv_txfm2d_add_32x64_c, svt_av1_highbd_inv_txfm_add_sse4_1, svt_dav1d_highbd_inv_txfm_add_avx2, svt_av1_inv_txfm2d_add_32x64_avx512);
    SET_SSE41_AVX2_AVX512(svt_av1_inv_txfm2d_add_64x16, svt_av1_inv_txfm2d_add_64x16_c, svt_av1_highbd_inv_txfm_add_sse4_1, svt_dav1d_highbd_inv_txfm_add_avx2, svt_av1_inv_txfm2d_add_64x16_avx512);
    SET_SSE41_AVX2_AVX512(svt_av1_inv_txfm2d_add_64x32, svt_av1_inv_txfm2d_add_64x32_c, svt_av1_highbd_inv_txfm_add_sse4_1, svt_dav1d_highbd_inv_txfm_add_avx2, svt_av1_inv_txfm2d_add_64x32_avx512);
#endif
    SET_SSE41_AVX2_AVX512(svt_av1_inv_txfm2d_add_64x64, svt_av1_inv_txfm2d_add_64x64_c, svt_av1_inv_txfm2d_add_64x64_sse4_1, svt_dav1d_inv_txfm2d_add_64x64_avx2, svt_av1_inv_txfm2d_add_64x64_avx512);

    // workaround for dav1d functions crashing valgrind's libVEX JIT compiler
    if (EB_UNLIKELY(RUNNING_ON_VALGRIND))
        SET_SSSE3_AVX2(svt_av1_inv_txfm_add, svt_av1_inv_txfm_add_c, svt_av1_inv_txfm_add_ssse3, svt_av1_inv_txfm_add_avx2);
    else
        SET_SSSE3_AVX2(svt_av1_inv_txfm_add, svt_av1_inv_txfm_add_c, svt_av1_inv_txfm_add_ssse3, svt_dav1d_inv_txfm_add_avx2);

    SET_SSE41_AVX2(svt_compressed_packmsb, svt_compressed_packmsb_c, svt_compressed_packmsb_sse4_1_intrin, svt_compressed_packmsb_avx2_intrin);
    SET_AVX2(svt_cfl_luma_subsampling_420_lbd, svt_cfl_luma_subsampling_420_lbd_c, svt_cfl_luma_subsampling_420_lbd_avx2);
    SET_AVX2(svt_cfl_luma_subsampling_420_hbd, svt_cfl_luma_subsampling_420_hbd_c, svt_cfl_luma_subsampling_420_hbd_avx2);
    SET_AVX2(svt_convert_8bit_to_16bit, svt_convert_8bit_to_16bit_c, svt_convert_8bit_to_16bit_avx2);
    SET_AVX2(svt_convert_16bit_to_8bit, svt_convert_16bit_to_8bit_c, svt_convert_16bit_to_8bit_avx2);
    SET_SSE2_AVX2(svt_pack2d_16_bit_src_mul4, svt_enc_msb_pack2_d, svt_enc_msb_pack2d_sse2_intrin, svt_enc_msb_pack2d_avx2_intrin_al);
    SET_SSE2_AVX2(svt_aom_un_pack2d_16_bit_src_mul4, svt_enc_msb_un_pack2_d, svt_enc_msb_un_pack2d_sse2_intrin, svt_enc_msb_un_pack2d_avx2_intrin);
    SET_SSE41_AVX2(svt_full_distortion_kernel_cbf_zero32_bits, svt_full_distortion_kernel_cbf_zero32_bits_c, svt_full_distortion_kernel_cbf_zero32_bits_sse4_1, svt_full_distortion_kernel_cbf_zero32_bits_avx2);
    SET_SSE41_AVX2(svt_full_distortion_kernel32_bits, svt_full_distortion_kernel32_bits_c, svt_full_distortion_kernel32_bits_sse4_1, svt_full_distortion_kernel32_bits_avx2);
    SET_SSE41_AVX2_AVX512(svt_spatial_full_distortion_kernel, svt_spatial_full_distortion_kernel_c, svt_spatial_full_distortion_kernel_sse4_1, svt_spatial_full_distortion_kernel_avx2, svt_spatial_full_distortion_kernel_avx512);
    SET_SSE41_AVX2(svt_full_distortion_kernel16_bits, svt_full_distortion_kernel16_bits_c, svt_full_distortion_kernel16_bits_sse4_1, svt_full_distortion_kernel16_bits_avx2);
    SET_SSE41_AVX2_AVX512(svt_residual_kernel8bit, svt_residual_kernel8bit_c, svt_residual_kernel8bit_sse4_1, svt_residual_kernel8bit_avx2, svt_residual_kernel8bit_avx512);
    SET_SSE2_AVX2(svt_residual_kernel16bit, svt_residual_kernel16bit_c, svt_residual_kernel16bit_sse2_intrin, svt_residual_kernel16bit_avx2);
    SET_SSE2_AVX2_AVX512(svt_av1_wiener_convolve_add_src, svt_av1_wiener_convolve_add_src_c, svt_av1_wiener_convolve_add_src_sse2, svt_av1_wiener_convolve_add_src_avx2, svt_av1_wiener_convolve_add_src_avx512);
    SET_SSE41(svt_av1_convolve_2d_scale, svt_av1_convolve_2d_scale_c, svt_av1_convolve_2d_scale_sse4_1);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_SSSE3_AVX2(svt_av1_highbd_convolve_y_sr, svt_av1_highbd_convolve_y_sr_c, svt_av1_highbd_convolve_y_sr_ssse3, svt_av1_highbd_convolve_y_sr_avx2);
    SET_SSSE3_AVX2(svt_av1_highbd_convolve_2d_sr, svt_av1_highbd_convolve_2d_sr_c, svt_av1_highbd_convolve_2d_sr_ssse3, svt_av1_highbd_convolve_2d_sr_avx2);
    SET_SSE41(svt_av1_highbd_convolve_2d_scale, svt_av1_highbd_convolve_2d_scale_c, svt_av1_highbd_convolve_2d_scale_sse4_1);
    SET_SSSE3_AVX2(svt_av1_highbd_convolve_2d_copy_sr, svt_av1_highbd_convolve_2d_copy_sr_c, svt_av1_highbd_convolve_2d_copy_sr_ssse3, svt_av1_highbd_convolve_2d_copy_sr_avx2);
    SET_SSE41_AVX2(svt_av1_highbd_jnt_convolve_2d, svt_av1_highbd_jnt_convolve_2d_c, svt_av1_highbd_jnt_convolve_2d_sse4_1, svt_av1_highbd_jnt_convolve_2d_avx2);
    SET_SSE41_AVX2(svt_av1_highbd_jnt_convolve_2d_copy, svt_av1_highbd_jnt_convolve_2d_copy_c, svt_av1_highbd_jnt_convolve_2d_copy_sse4_1, svt_av1_highbd_jnt_convolve_2d_copy_avx2);
    SET_SSE41_AVX2(svt_av1_highbd_jnt_convolve_x, svt_av1_highbd_jnt_convolve_x_c, svt_av1_highbd_jnt_convolve_x_sse4_1, svt_av1_highbd_jnt_convolve_x_avx2);
    SET_SSE41_AVX2(svt_av1_highbd_jnt_convolve_y, svt_av1_highbd_jnt_convolve_y_c, svt_av1_highbd_jnt_convolve_y_sse4_1, svt_av1_highbd_jnt_convolve_y_avx2);
    SET_SSSE3_AVX2(svt_av1_highbd_convolve_x_sr, svt_av1_highbd_convolve_x_sr_c, svt_av1_highbd_convolve_x_sr_ssse3, svt_av1_highbd_convolve_x_sr_avx2);
#endif
    SET_SSE2_AVX2_AVX512(svt_av1_convolve_2d_sr, svt_av1_convolve_2d_sr_c,svt_av1_convolve_2d_sr_sse2, svt_av1_convolve_2d_sr_avx2, svt_av1_convolve_2d_sr_avx512);
    SET_SSE2_AVX2_AVX512(svt_av1_convolve_2d_copy_sr, svt_av1_convolve_2d_copy_sr_c, svt_av1_convolve_2d_copy_sr_sse2, svt_av1_convolve_2d_copy_sr_avx2, svt_av1_convolve_2d_copy_sr_avx512);
    SET_SSE2_AVX2_AVX512(svt_av1_convolve_x_sr, svt_av1_convolve_x_sr_c, svt_av1_convolve_x_sr_sse2, svt_av1_convolve_x_sr_avx2, svt_av1_convolve_x_sr_avx512);
    SET_SSE2_AVX2_AVX512(svt_av1_convolve_y_sr, svt_av1_convolve_y_sr_c, svt_av1_convolve_y_sr_sse2, svt_av1_convolve_y_sr_avx2, svt_av1_convolve_y_sr_avx512);
    SET_SSE2_SSSE3_AVX2_AVX512(svt_av1_jnt_convolve_2d, svt_av1_jnt_convolve_2d_c, svt_av1_jnt_convolve_2d_sse2, svt_av1_jnt_convolve_2d_ssse3, svt_av1_jnt_convolve_2d_avx2, svt_av1_jnt_convolve_2d_avx512);
    SET_SSE2_AVX2_AVX512(svt_av1_jnt_convolve_2d_copy, svt_av1_jnt_convolve_2d_copy_c, svt_av1_jnt_convolve_2d_copy_sse2, svt_av1_jnt_convolve_2d_copy_avx2, svt_av1_jnt_convolve_2d_copy_avx512);
    SET_SSE2_AVX2_AVX512(svt_av1_jnt_convolve_x, svt_av1_jnt_convolve_x_c, svt_av1_jnt_convolve_x_sse2, svt_av1_jnt_convolve_x_avx2, svt_av1_jnt_convolve_x_avx512);
    SET_SSE2_AVX2_AVX512(svt_av1_jnt_convolve_y, svt_av1_jnt_convolve_y_c, svt_av1_jnt_convolve_y_sse2, svt_av1_jnt_convolve_y_avx2, svt_av1_jnt_convolve_y_avx512);
    SET_SSSE3_AVX2(svt_aom_convolve8_horiz, svt_aom_convolve8_horiz_c, svt_aom_convolve8_horiz_ssse3, svt_aom_convolve8_horiz_avx2);
    SET_SSSE3_AVX2(svt_aom_convolve8_vert, svt_aom_convolve8_vert_c, svt_aom_convolve8_vert_ssse3, svt_aom_convolve8_vert_avx2);
    SET_SSE41_AVX2(svt_av1_build_compound_diffwtd_mask, svt_av1_build_compound_diffwtd_mask_c, svt_av1_build_compound_diffwtd_mask_sse4_1, svt_av1_build_compound_diffwtd_mask_avx2);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_SSSE3_AVX2(svt_av1_build_compound_diffwtd_mask_highbd, svt_av1_build_compound_diffwtd_mask_highbd_c, svt_av1_build_compound_diffwtd_mask_highbd_ssse3, svt_av1_build_compound_diffwtd_mask_highbd_avx2);
#endif
    SET_SSE2_AVX2(svt_av1_wedge_sse_from_residuals, svt_av1_wedge_sse_from_residuals_c, svt_av1_wedge_sse_from_residuals_sse2, svt_av1_wedge_sse_from_residuals_avx2);
    SET_AVX2(svt_aom_subtract_block, svt_aom_subtract_block_c, svt_aom_subtract_block_avx2);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_SSE2(svt_aom_highbd_subtract_block, svt_aom_highbd_subtract_block_c, svt_aom_highbd_subtract_block_sse2);
    SET_SSSE3(svt_aom_highbd_smooth_v_predictor_4x4, svt_aom_highbd_smooth_v_predictor_4x4_c, svt_aom_highbd_smooth_v_predictor_4x4_ssse3);
    SET_SSSE3(svt_aom_highbd_smooth_v_predictor_4x8, svt_aom_highbd_smooth_v_predictor_4x8_c, svt_aom_highbd_smooth_v_predictor_4x8_ssse3);
    SET_SSSE3(svt_aom_highbd_smooth_v_predictor_4x16, svt_aom_highbd_smooth_v_predictor_4x16_c, svt_aom_highbd_smooth_v_predictor_4x16_ssse3);
    SET_AVX2(svt_aom_highbd_smooth_v_predictor_8x4, svt_aom_highbd_smooth_v_predictor_8x4_c, svt_aom_highbd_smooth_v_predictor_8x4_avx2);
    SET_AVX2(svt_aom_highbd_smooth_v_predictor_8x8, svt_aom_highbd_smooth_v_predictor_8x8_c, svt_aom_highbd_smooth_v_predictor_8x8_avx2);
    SET_AVX2(svt_aom_highbd_smooth_v_predictor_8x16, svt_aom_highbd_smooth_v_predictor_8x16_c, svt_aom_highbd_smooth_v_predictor_8x16_avx2);
    SET_AVX2(svt_aom_highbd_smooth_v_predictor_8x32, svt_aom_highbd_smooth_v_predictor_8x32_c, svt_aom_highbd_smooth_v_predictor_8x32_avx2);
    SET_AVX2(svt_aom_highbd_smooth_v_predictor_16x4, svt_aom_highbd_smooth_v_predictor_16x4_c, svt_aom_highbd_smooth_v_predictor_16x4_avx2);
    SET_AVX2(svt_aom_highbd_smooth_v_predictor_16x8, svt_aom_highbd_smooth_v_predictor_16x8_c, svt_aom_highbd_smooth_v_predictor_16x8_avx2);
    SET_AVX2(svt_aom_highbd_smooth_v_predictor_16x16, svt_aom_highbd_smooth_v_predictor_16x16_c, svt_aom_highbd_smooth_v_predictor_16x16_avx2);
    SET_AVX2(svt_aom_highbd_smooth_v_predictor_16x32, svt_aom_highbd_smooth_v_predictor_16x32_c, svt_aom_highbd_smooth_v_predictor_16x32_avx2);
    SET_AVX2(svt_aom_highbd_smooth_v_predictor_16x64, svt_aom_highbd_smooth_v_predictor_16x64_c, svt_aom_highbd_smooth_v_predictor_16x64_avx2);
    SET_AVX2_AVX512(svt_aom_highbd_smooth_v_predictor_32x8, svt_aom_highbd_smooth_v_predictor_32x8_c, svt_aom_highbd_smooth_v_predictor_32x8_avx2, aom_highbd_smooth_v_predictor_32x8_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_smooth_v_predictor_32x16, svt_aom_highbd_smooth_v_predictor_32x16_c, svt_aom_highbd_smooth_v_predictor_32x16_avx2, aom_highbd_smooth_v_predictor_32x16_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_smooth_v_predictor_32x32, svt_aom_highbd_smooth_v_predictor_32x32_c, svt_aom_highbd_smooth_v_predictor_32x32_avx2, aom_highbd_smooth_v_predictor_32x32_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_smooth_v_predictor_32x64, svt_aom_highbd_smooth_v_predictor_32x64_c, svt_aom_highbd_smooth_v_predictor_32x64_avx2, aom_highbd_smooth_v_predictor_32x64_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_smooth_v_predictor_64x16, svt_aom_highbd_smooth_v_predictor_64x16_c, svt_aom_highbd_smooth_v_predictor_64x16_avx2, aom_highbd_smooth_v_predictor_64x16_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_smooth_v_predictor_64x32, svt_aom_highbd_smooth_v_predictor_64x32_c, svt_aom_highbd_smooth_v_predictor_64x32_avx2, aom_highbd_smooth_v_predictor_64x32_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_smooth_v_predictor_64x64, svt_aom_highbd_smooth_v_predictor_64x64_c, svt_aom_highbd_smooth_v_predictor_64x64_avx2, aom_highbd_smooth_v_predictor_64x64_avx512);
#endif
    SET_AVX2(svt_av1_dr_prediction_z1, svt_av1_dr_prediction_z1_c, svt_av1_dr_prediction_z1_avx2);
    SET_AVX2(svt_av1_dr_prediction_z2, svt_av1_dr_prediction_z2_c, svt_av1_dr_prediction_z2_avx2);
    SET_AVX2(svt_av1_dr_prediction_z3, svt_av1_dr_prediction_z3_c, svt_av1_dr_prediction_z3_avx2);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_AVX2(svt_av1_highbd_dr_prediction_z1, svt_av1_highbd_dr_prediction_z1_c, svt_av1_highbd_dr_prediction_z1_avx2);
    SET_AVX2(svt_av1_highbd_dr_prediction_z2, svt_av1_highbd_dr_prediction_z2_c, svt_av1_highbd_dr_prediction_z2_avx2);
    SET_AVX2(svt_av1_highbd_dr_prediction_z3, svt_av1_highbd_dr_prediction_z3_c, svt_av1_highbd_dr_prediction_z3_avx2);
#endif
    SET_SSSE3(svt_aom_paeth_predictor_4x4, svt_aom_paeth_predictor_4x4_c, svt_aom_paeth_predictor_4x4_ssse3);
    SET_SSSE3(svt_aom_paeth_predictor_4x8, svt_aom_paeth_predictor_4x8_c, svt_aom_paeth_predictor_4x8_ssse3);
    SET_SSSE3(svt_aom_paeth_predictor_4x16, svt_aom_paeth_predictor_4x16_c, svt_aom_paeth_predictor_4x16_ssse3);
    SET_SSSE3(svt_aom_paeth_predictor_8x4, svt_aom_paeth_predictor_8x4_c, svt_aom_paeth_predictor_8x4_ssse3);
    SET_SSSE3(svt_aom_paeth_predictor_8x8, svt_aom_paeth_predictor_8x8_c, svt_aom_paeth_predictor_8x8_ssse3);
    SET_SSSE3(svt_aom_paeth_predictor_8x16, svt_aom_paeth_predictor_8x16_c, svt_aom_paeth_predictor_8x16_ssse3);
    SET_SSSE3(svt_aom_paeth_predictor_8x32, svt_aom_paeth_predictor_8x32_c, svt_aom_paeth_predictor_8x32_ssse3);
    SET_SSSE3(svt_aom_paeth_predictor_16x4, svt_aom_paeth_predictor_16x4_c, svt_aom_paeth_predictor_16x4_ssse3);
    SET_SSSE3_AVX2(svt_aom_paeth_predictor_16x8, svt_aom_paeth_predictor_16x8_c, svt_aom_paeth_predictor_16x8_ssse3, svt_aom_paeth_predictor_16x8_avx2);
    SET_SSSE3_AVX2(svt_aom_paeth_predictor_16x16, svt_aom_paeth_predictor_16x16_c, svt_aom_paeth_predictor_16x16_ssse3, svt_aom_paeth_predictor_16x16_avx2);
    SET_SSSE3_AVX2(svt_aom_paeth_predictor_16x32, svt_aom_paeth_predictor_16x32_c, svt_aom_paeth_predictor_16x32_ssse3, svt_aom_paeth_predictor_16x32_avx2);
    SET_SSSE3_AVX2(svt_aom_paeth_predictor_16x64, svt_aom_paeth_predictor_16x64_c, svt_aom_paeth_predictor_16x64_ssse3, svt_aom_paeth_predictor_16x64_avx2);
    SET_SSSE3(svt_aom_paeth_predictor_32x8, svt_aom_paeth_predictor_32x8_c, svt_aom_paeth_predictor_32x8_ssse3);
    SET_SSSE3_AVX2(svt_aom_paeth_predictor_32x16, svt_aom_paeth_predictor_32x16_c, svt_aom_paeth_predictor_32x16_ssse3, svt_aom_paeth_predictor_32x16_avx2);
    SET_SSSE3_AVX2(svt_aom_paeth_predictor_32x32, svt_aom_paeth_predictor_32x32_c, svt_aom_paeth_predictor_32x32_ssse3, svt_aom_paeth_predictor_32x32_avx2);
    SET_SSSE3_AVX2(svt_aom_paeth_predictor_32x64, svt_aom_paeth_predictor_32x64_c, svt_aom_paeth_predictor_32x64_ssse3, svt_aom_paeth_predictor_32x64_avx2);
    SET_SSSE3_AVX2(svt_aom_paeth_predictor_64x16, svt_aom_paeth_predictor_64x16_c, svt_aom_paeth_predictor_64x16_ssse3, svt_aom_paeth_predictor_64x16_avx2);
    SET_SSSE3_AVX2(svt_aom_paeth_predictor_64x32, svt_aom_paeth_predictor_64x32_c, svt_aom_paeth_predictor_64x32_ssse3, svt_aom_paeth_predictor_64x32_avx2);
    SET_SSSE3_AVX2(svt_aom_paeth_predictor_64x64, svt_aom_paeth_predictor_64x64_c, svt_aom_paeth_predictor_64x64_ssse3, svt_aom_paeth_predictor_64x64_avx2);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_AVX2(svt_aom_highbd_paeth_predictor_4x4, svt_aom_highbd_paeth_predictor_4x4_c, svt_aom_highbd_paeth_predictor_4x4_avx2);
    SET_AVX2(svt_aom_highbd_paeth_predictor_4x8, svt_aom_highbd_paeth_predictor_4x8_c, svt_aom_highbd_paeth_predictor_4x8_avx2);
    SET_AVX2(svt_aom_highbd_paeth_predictor_4x16, svt_aom_highbd_paeth_predictor_4x16_c, svt_aom_highbd_paeth_predictor_4x16_avx2);
    SET_AVX2(svt_aom_highbd_paeth_predictor_8x4, svt_aom_highbd_paeth_predictor_8x4_c, svt_aom_highbd_paeth_predictor_8x4_avx2);
    SET_AVX2(svt_aom_highbd_paeth_predictor_8x8, svt_aom_highbd_paeth_predictor_8x8_c, svt_aom_highbd_paeth_predictor_8x8_avx2);
    SET_AVX2(svt_aom_highbd_paeth_predictor_8x16, svt_aom_highbd_paeth_predictor_8x16_c, svt_aom_highbd_paeth_predictor_8x16_avx2);
    SET_AVX2(svt_aom_highbd_paeth_predictor_8x32, svt_aom_highbd_paeth_predictor_8x32_c, svt_aom_highbd_paeth_predictor_8x32_avx2);
    SET_AVX2(svt_aom_highbd_paeth_predictor_16x4, svt_aom_highbd_paeth_predictor_16x4_c, svt_aom_highbd_paeth_predictor_16x4_avx2);
    SET_AVX2(svt_aom_highbd_paeth_predictor_16x8, svt_aom_highbd_paeth_predictor_16x8_c, svt_aom_highbd_paeth_predictor_16x8_avx2);
    SET_AVX2(svt_aom_highbd_paeth_predictor_16x16, svt_aom_highbd_paeth_predictor_16x16_c, svt_aom_highbd_paeth_predictor_16x16_avx2);
    SET_AVX2(svt_aom_highbd_paeth_predictor_16x32, svt_aom_highbd_paeth_predictor_16x32_c, svt_aom_highbd_paeth_predictor_16x32_avx2);
    SET_AVX2(svt_aom_highbd_paeth_predictor_16x64, svt_aom_highbd_paeth_predictor_16x64_c, svt_aom_highbd_paeth_predictor_16x64_avx2);
    SET_AVX2(svt_aom_highbd_paeth_predictor_32x8, svt_aom_highbd_paeth_predictor_32x8_c, svt_aom_highbd_paeth_predictor_32x8_avx2);
    SET_AVX2(svt_aom_highbd_paeth_predictor_32x16, svt_aom_highbd_paeth_predictor_32x16_c, svt_aom_highbd_paeth_predictor_32x16_avx2);
    SET_AVX2(svt_aom_highbd_paeth_predictor_32x32, svt_aom_highbd_paeth_predictor_32x32_c, svt_aom_highbd_paeth_predictor_32x32_avx2);
    SET_AVX2(svt_aom_highbd_paeth_predictor_32x64, svt_aom_highbd_paeth_predictor_32x64_c, svt_aom_highbd_paeth_predictor_32x64_avx2);
    SET_AVX2(svt_aom_highbd_paeth_predictor_64x16, svt_aom_highbd_paeth_predictor_64x16_c, svt_aom_highbd_paeth_predictor_64x16_avx2);
    SET_AVX2(svt_aom_highbd_paeth_predictor_64x32, svt_aom_highbd_paeth_predictor_64x32_c, svt_aom_highbd_paeth_predictor_64x32_avx2);
    SET_AVX2(svt_aom_highbd_paeth_predictor_64x64, svt_aom_highbd_paeth_predictor_64x64_c, svt_aom_highbd_paeth_predictor_64x64_avx2);
#endif
    SET_SSE2(svt_aom_sum_squares_i16, svt_aom_sum_squares_i16_c, svt_aom_sum_squares_i16_sse2);
    SET_SSE2(svt_aom_dc_predictor_4x4, svt_aom_dc_predictor_4x4_c, svt_aom_dc_predictor_4x4_sse2);
    SET_SSE2(svt_aom_dc_predictor_4x8, svt_aom_dc_predictor_4x8_c, svt_aom_dc_predictor_4x8_sse2);
    SET_SSE2(svt_aom_dc_predictor_4x16, svt_aom_dc_predictor_4x16_c, svt_aom_dc_predictor_4x16_sse2);
    SET_SSE2(svt_aom_dc_predictor_8x4, svt_aom_dc_predictor_8x4_c, svt_aom_dc_predictor_8x4_sse2);
    SET_SSE2(svt_aom_dc_predictor_8x8, svt_aom_dc_predictor_8x8_c, svt_aom_dc_predictor_8x8_sse2);
    SET_SSE2(svt_aom_dc_predictor_8x16, svt_aom_dc_predictor_8x16_c, svt_aom_dc_predictor_8x16_sse2);
    SET_SSE2(svt_aom_dc_predictor_8x32, svt_aom_dc_predictor_8x32_c, svt_aom_dc_predictor_8x32_sse2);
    SET_SSE2(svt_aom_dc_predictor_16x4, svt_aom_dc_predictor_16x4_c, svt_aom_dc_predictor_16x4_sse2);
    SET_SSE2(svt_aom_dc_predictor_16x8, svt_aom_dc_predictor_16x8_c, svt_aom_dc_predictor_16x8_sse2);
    SET_SSE2(svt_aom_dc_predictor_16x16, svt_aom_dc_predictor_16x16_c, svt_aom_dc_predictor_16x16_sse2);
    SET_SSE2(svt_aom_dc_predictor_16x32, svt_aom_dc_predictor_16x32_c, svt_aom_dc_predictor_16x32_sse2);
    SET_SSE2(svt_aom_dc_predictor_16x64, svt_aom_dc_predictor_16x64_c, svt_aom_dc_predictor_16x64_sse2);
    SET_SSE2(svt_aom_dc_predictor_32x8, svt_aom_dc_predictor_32x8_c, svt_aom_dc_predictor_32x8_sse2);
    SET_AVX2(svt_aom_dc_predictor_32x16, svt_aom_dc_predictor_32x16_c, svt_aom_dc_predictor_32x16_avx2);
    SET_AVX2(svt_aom_dc_predictor_32x32, svt_aom_dc_predictor_32x32_c, svt_aom_dc_predictor_32x32_avx2);
    SET_AVX2(svt_aom_dc_predictor_32x64, svt_aom_dc_predictor_32x64_c, svt_aom_dc_predictor_32x64_avx2);
    SET_AVX2(svt_aom_dc_predictor_64x16, svt_aom_dc_predictor_64x16_c, svt_aom_dc_predictor_64x16_avx2);
    SET_AVX2(svt_aom_dc_predictor_64x32, svt_aom_dc_predictor_64x32_c, svt_aom_dc_predictor_64x32_avx2);
    SET_AVX2(svt_aom_dc_predictor_64x64, svt_aom_dc_predictor_64x64_c, svt_aom_dc_predictor_64x64_avx2);

    SET_SSE2(svt_aom_dc_top_predictor_4x4, svt_aom_dc_top_predictor_4x4_c, svt_aom_dc_top_predictor_4x4_sse2);
    SET_SSE2(svt_aom_dc_top_predictor_4x8, svt_aom_dc_top_predictor_4x8_c, svt_aom_dc_top_predictor_4x8_sse2);
    SET_SSE2(svt_aom_dc_top_predictor_4x16, svt_aom_dc_top_predictor_4x16_c, svt_aom_dc_top_predictor_4x16_sse2);
    SET_SSE2(svt_aom_dc_top_predictor_8x4, svt_aom_dc_top_predictor_8x4_c, svt_aom_dc_top_predictor_8x4_sse2);
    SET_SSE2(svt_aom_dc_top_predictor_8x8, svt_aom_dc_top_predictor_8x8_c, svt_aom_dc_top_predictor_8x8_sse2);
    SET_SSE2(svt_aom_dc_top_predictor_8x16, svt_aom_dc_top_predictor_8x16_c, svt_aom_dc_top_predictor_8x16_sse2);
    SET_SSE2(svt_aom_dc_top_predictor_8x32, svt_aom_dc_top_predictor_8x32_c, svt_aom_dc_top_predictor_8x32_sse2);
    SET_SSE2(svt_aom_dc_top_predictor_16x4, svt_aom_dc_top_predictor_16x4_c, svt_aom_dc_top_predictor_16x4_sse2);
    SET_SSE2(svt_aom_dc_top_predictor_16x8, svt_aom_dc_top_predictor_16x8_c, svt_aom_dc_top_predictor_16x8_sse2);
    SET_SSE2(svt_aom_dc_top_predictor_16x16, svt_aom_dc_top_predictor_16x16_c, svt_aom_dc_top_predictor_16x16_sse2);
    SET_SSE2(svt_aom_dc_top_predictor_16x32, svt_aom_dc_top_predictor_16x32_c, svt_aom_dc_top_predictor_16x32_sse2);
    SET_SSE2(svt_aom_dc_top_predictor_16x64, svt_aom_dc_top_predictor_16x64_c, svt_aom_dc_top_predictor_16x64_sse2);
    SET_SSE2(svt_aom_dc_top_predictor_32x8, svt_aom_dc_top_predictor_32x8_c, svt_aom_dc_top_predictor_32x8_sse2);
    SET_AVX2(svt_aom_dc_top_predictor_32x16, svt_aom_dc_top_predictor_32x16_c, svt_aom_dc_top_predictor_32x16_avx2);
    SET_AVX2(svt_aom_dc_top_predictor_32x32, svt_aom_dc_top_predictor_32x32_c, svt_aom_dc_top_predictor_32x32_avx2);
    SET_AVX2(svt_aom_dc_top_predictor_32x64, svt_aom_dc_top_predictor_32x64_c, svt_aom_dc_top_predictor_32x64_avx2);
    SET_AVX2(svt_aom_dc_top_predictor_64x16, svt_aom_dc_top_predictor_64x16_c, svt_aom_dc_top_predictor_64x16_avx2);
    SET_AVX2(svt_aom_dc_top_predictor_64x32, svt_aom_dc_top_predictor_64x32_c, svt_aom_dc_top_predictor_64x32_avx2);
    SET_AVX2(svt_aom_dc_top_predictor_64x64, svt_aom_dc_top_predictor_64x64_c, svt_aom_dc_top_predictor_64x64_avx2);

    SET_SSE2(svt_aom_dc_left_predictor_4x4, svt_aom_dc_left_predictor_4x4_c, svt_aom_dc_left_predictor_4x4_sse2);
    SET_SSE2(svt_aom_dc_left_predictor_4x8, svt_aom_dc_left_predictor_4x8_c, svt_aom_dc_left_predictor_4x8_sse2);
    SET_SSE2(svt_aom_dc_left_predictor_4x16, svt_aom_dc_left_predictor_4x16_c, svt_aom_dc_left_predictor_4x16_sse2);
    SET_SSE2(svt_aom_dc_left_predictor_8x4, svt_aom_dc_left_predictor_8x4_c, svt_aom_dc_left_predictor_8x4_sse2);
    SET_SSE2(svt_aom_dc_left_predictor_8x8, svt_aom_dc_left_predictor_8x8_c, svt_aom_dc_left_predictor_8x8_sse2);
    SET_SSE2(svt_aom_dc_left_predictor_8x16, svt_aom_dc_left_predictor_8x16_c, svt_aom_dc_left_predictor_8x16_sse2);
    SET_SSE2(svt_aom_dc_left_predictor_8x32, svt_aom_dc_left_predictor_8x32_c, svt_aom_dc_left_predictor_8x32_sse2);
    SET_SSE2(svt_aom_dc_left_predictor_16x4, svt_aom_dc_left_predictor_16x4_c, svt_aom_dc_left_predictor_16x4_sse2);
    SET_SSE2(svt_aom_dc_left_predictor_16x8, svt_aom_dc_left_predictor_16x8_c, svt_aom_dc_left_predictor_16x8_sse2);
    SET_SSE2(svt_aom_dc_left_predictor_16x16, svt_aom_dc_left_predictor_16x16_c, svt_aom_dc_left_predictor_16x16_sse2);
    SET_SSE2(svt_aom_dc_left_predictor_16x32, svt_aom_dc_left_predictor_16x32_c, svt_aom_dc_left_predictor_16x32_sse2);
    SET_SSE2(svt_aom_dc_left_predictor_16x64, svt_aom_dc_left_predictor_16x64_c, svt_aom_dc_left_predictor_16x64_sse2);
    SET_SSE2(svt_aom_dc_left_predictor_32x8, svt_aom_dc_left_predictor_32x8_c, svt_aom_dc_left_predictor_32x8_sse2);
    SET_AVX2(svt_aom_dc_left_predictor_32x16, svt_aom_dc_left_predictor_32x16_c, svt_aom_dc_left_predictor_32x16_avx2);
    SET_AVX2(svt_aom_dc_left_predictor_32x32, svt_aom_dc_left_predictor_32x32_c, svt_aom_dc_left_predictor_32x32_avx2);
    SET_AVX2(svt_aom_dc_left_predictor_32x64, svt_aom_dc_left_predictor_32x64_c, svt_aom_dc_left_predictor_32x64_avx2);
    SET_AVX2(svt_aom_dc_left_predictor_64x16, svt_aom_dc_left_predictor_64x16_c, svt_aom_dc_left_predictor_64x16_avx2);
    SET_AVX2(svt_aom_dc_left_predictor_64x32, svt_aom_dc_left_predictor_64x32_c, svt_aom_dc_left_predictor_64x32_avx2);
    SET_AVX2(svt_aom_dc_left_predictor_64x64, svt_aom_dc_left_predictor_64x64_c, svt_aom_dc_left_predictor_64x64_avx2);

    SET_SSE2(svt_aom_dc_128_predictor_4x4, svt_aom_dc_128_predictor_4x4_c, svt_aom_dc_128_predictor_4x4_sse2);
    SET_SSE2(svt_aom_dc_128_predictor_4x8, svt_aom_dc_128_predictor_4x8_c, svt_aom_dc_128_predictor_4x8_sse2);
    SET_SSE2(svt_aom_dc_128_predictor_4x16, svt_aom_dc_128_predictor_4x16_c, svt_aom_dc_128_predictor_4x16_sse2);
    SET_SSE2(svt_aom_dc_128_predictor_8x4, svt_aom_dc_128_predictor_8x4_c, svt_aom_dc_128_predictor_8x4_sse2);
    SET_SSE2(svt_aom_dc_128_predictor_8x8, svt_aom_dc_128_predictor_8x8_c, svt_aom_dc_128_predictor_8x8_sse2);
    SET_SSE2(svt_aom_dc_128_predictor_8x16, svt_aom_dc_128_predictor_8x16_c, svt_aom_dc_128_predictor_8x16_sse2);
    SET_SSE2(svt_aom_dc_128_predictor_8x32, svt_aom_dc_128_predictor_8x32_c, svt_aom_dc_128_predictor_8x32_sse2);
    SET_SSE2(svt_aom_dc_128_predictor_16x4, svt_aom_dc_128_predictor_16x4_c, svt_aom_dc_128_predictor_16x4_sse2);
    SET_SSE2(svt_aom_dc_128_predictor_16x8, svt_aom_dc_128_predictor_16x8_c, svt_aom_dc_128_predictor_16x8_sse2);
    SET_SSE2(svt_aom_dc_128_predictor_16x16, svt_aom_dc_128_predictor_16x16_c, svt_aom_dc_128_predictor_16x16_sse2);
    SET_SSE2(svt_aom_dc_128_predictor_16x32, svt_aom_dc_128_predictor_16x32_c, svt_aom_dc_128_predictor_16x32_sse2);
    SET_SSE2(svt_aom_dc_128_predictor_16x64, svt_aom_dc_128_predictor_16x64_c, svt_aom_dc_128_predictor_16x64_sse2);
    SET_SSE2(svt_aom_dc_128_predictor_32x8, svt_aom_dc_128_predictor_32x8_c, svt_aom_dc_128_predictor_32x8_sse2);
    SET_AVX2(svt_aom_dc_128_predictor_32x16, svt_aom_dc_128_predictor_32x16_c, svt_aom_dc_128_predictor_32x16_avx2);
    SET_AVX2(svt_aom_dc_128_predictor_32x32, svt_aom_dc_128_predictor_32x32_c, svt_aom_dc_128_predictor_32x32_avx2);
    SET_AVX2(svt_aom_dc_128_predictor_32x64, svt_aom_dc_128_predictor_32x64_c, svt_aom_dc_128_predictor_32x64_avx2);
    SET_AVX2(svt_aom_dc_128_predictor_64x16, svt_aom_dc_128_predictor_64x16_c, svt_aom_dc_128_predictor_64x16_avx2);
    SET_AVX2(svt_aom_dc_128_predictor_64x32, svt_aom_dc_128_predictor_64x32_c, svt_aom_dc_128_predictor_64x32_avx2);
    SET_AVX2(svt_aom_dc_128_predictor_64x64, svt_aom_dc_128_predictor_64x64_c, svt_aom_dc_128_predictor_64x64_avx2);

    SET_SSSE3(svt_aom_smooth_h_predictor_4x4, svt_aom_smooth_h_predictor_4x4_c, svt_aom_smooth_h_predictor_4x4_ssse3);
    SET_SSSE3(svt_aom_smooth_h_predictor_4x8, svt_aom_smooth_h_predictor_4x8_c, svt_aom_smooth_h_predictor_4x8_ssse3);
    SET_SSSE3(svt_aom_smooth_h_predictor_4x16, svt_aom_smooth_h_predictor_4x16_c, svt_aom_smooth_h_predictor_4x16_ssse3);
    SET_SSSE3(svt_aom_smooth_h_predictor_8x4, svt_aom_smooth_h_predictor_8x4_c, svt_aom_smooth_h_predictor_8x4_ssse3);
    SET_SSSE3(svt_aom_smooth_h_predictor_8x8, svt_aom_smooth_h_predictor_8x8_c, svt_aom_smooth_h_predictor_8x8_ssse3);
    SET_SSSE3(svt_aom_smooth_h_predictor_8x16, svt_aom_smooth_h_predictor_8x16_c, svt_aom_smooth_h_predictor_8x16_ssse3);
    SET_SSSE3(svt_aom_smooth_h_predictor_8x32, svt_aom_smooth_h_predictor_8x32_c, svt_aom_smooth_h_predictor_8x32_ssse3);
    SET_SSSE3(svt_aom_smooth_h_predictor_16x4, svt_aom_smooth_h_predictor_16x4_c, svt_aom_smooth_h_predictor_16x4_ssse3);
    SET_SSSE3(svt_aom_smooth_h_predictor_16x8, svt_aom_smooth_h_predictor_16x8_c, svt_aom_smooth_h_predictor_16x8_ssse3);
    SET_SSSE3(svt_aom_smooth_h_predictor_16x16, svt_aom_smooth_h_predictor_16x16_c, svt_aom_smooth_h_predictor_16x16_ssse3);
    SET_SSSE3(svt_aom_smooth_h_predictor_16x32, svt_aom_smooth_h_predictor_16x32_c, svt_aom_smooth_h_predictor_16x32_ssse3);
    SET_SSSE3(svt_aom_smooth_h_predictor_16x64, svt_aom_smooth_h_predictor_16x64_c, svt_aom_smooth_h_predictor_16x64_ssse3);
    SET_SSSE3(svt_aom_smooth_h_predictor_32x8, svt_aom_smooth_h_predictor_32x8_c, svt_aom_smooth_h_predictor_32x8_ssse3);
    SET_SSSE3(svt_aom_smooth_h_predictor_32x16, svt_aom_smooth_h_predictor_32x16_c, svt_aom_smooth_h_predictor_32x16_ssse3);
    SET_SSSE3(svt_aom_smooth_h_predictor_32x32, svt_aom_smooth_h_predictor_32x32_c, svt_aom_smooth_h_predictor_32x32_ssse3);
    SET_SSSE3(svt_aom_smooth_h_predictor_32x64, svt_aom_smooth_h_predictor_32x64_c, svt_aom_smooth_h_predictor_32x64_ssse3);
    SET_SSSE3(svt_aom_smooth_h_predictor_64x16, svt_aom_smooth_h_predictor_64x16_c, svt_aom_smooth_h_predictor_64x16_ssse3);
    SET_SSSE3(svt_aom_smooth_h_predictor_64x32, svt_aom_smooth_h_predictor_64x32_c, svt_aom_smooth_h_predictor_64x32_ssse3);
    SET_SSSE3(svt_aom_smooth_h_predictor_64x64, svt_aom_smooth_h_predictor_64x64_c, svt_aom_smooth_h_predictor_64x64_ssse3);

    SET_SSSE3(svt_aom_smooth_v_predictor_4x4, svt_aom_smooth_v_predictor_4x4_c, svt_aom_smooth_v_predictor_4x4_ssse3);
    SET_SSSE3(svt_aom_smooth_v_predictor_4x8, svt_aom_smooth_v_predictor_4x8_c, svt_aom_smooth_v_predictor_4x8_ssse3);
    SET_SSSE3(svt_aom_smooth_v_predictor_4x16, svt_aom_smooth_v_predictor_4x16_c, svt_aom_smooth_v_predictor_4x16_ssse3);
    SET_SSSE3(svt_aom_smooth_v_predictor_8x4, svt_aom_smooth_v_predictor_8x4_c, svt_aom_smooth_v_predictor_8x4_ssse3);
    SET_SSSE3(svt_aom_smooth_v_predictor_8x8, svt_aom_smooth_v_predictor_8x8_c, svt_aom_smooth_v_predictor_8x8_ssse3);
    SET_SSSE3(svt_aom_smooth_v_predictor_8x16, svt_aom_smooth_v_predictor_8x16_c, svt_aom_smooth_v_predictor_8x16_ssse3);
    SET_SSSE3(svt_aom_smooth_v_predictor_8x32, svt_aom_smooth_v_predictor_8x32_c, svt_aom_smooth_v_predictor_8x32_ssse3);
    SET_SSSE3(svt_aom_smooth_v_predictor_16x4, svt_aom_smooth_v_predictor_16x4_c, svt_aom_smooth_v_predictor_16x4_ssse3);
    SET_SSSE3(svt_aom_smooth_v_predictor_16x8, svt_aom_smooth_v_predictor_16x8_c, svt_aom_smooth_v_predictor_16x8_ssse3);
    SET_SSSE3(svt_aom_smooth_v_predictor_16x16, svt_aom_smooth_v_predictor_16x16_c, svt_aom_smooth_v_predictor_16x16_ssse3);
    SET_SSSE3(svt_aom_smooth_v_predictor_16x32, svt_aom_smooth_v_predictor_16x32_c, svt_aom_smooth_v_predictor_16x32_ssse3);
    SET_SSSE3(svt_aom_smooth_v_predictor_16x64, svt_aom_smooth_v_predictor_16x64_c, svt_aom_smooth_v_predictor_16x64_ssse3);
    SET_SSSE3(svt_aom_smooth_v_predictor_32x8, svt_aom_smooth_v_predictor_32x8_c, svt_aom_smooth_v_predictor_32x8_ssse3);
    SET_SSSE3(svt_aom_smooth_v_predictor_32x16, svt_aom_smooth_v_predictor_32x16_c, svt_aom_smooth_v_predictor_32x16_ssse3);
    SET_SSSE3(svt_aom_smooth_v_predictor_32x32, svt_aom_smooth_v_predictor_32x32_c, svt_aom_smooth_v_predictor_32x32_ssse3);
    SET_SSSE3(svt_aom_smooth_v_predictor_32x64, svt_aom_smooth_v_predictor_32x64_c, svt_aom_smooth_v_predictor_32x64_ssse3);
    SET_SSSE3(svt_aom_smooth_v_predictor_64x16, svt_aom_smooth_v_predictor_64x16_c, svt_aom_smooth_v_predictor_64x16_ssse3);
    SET_SSSE3(svt_aom_smooth_v_predictor_64x32, svt_aom_smooth_v_predictor_64x32_c, svt_aom_smooth_v_predictor_64x32_ssse3);
    SET_SSSE3(svt_aom_smooth_v_predictor_64x64, svt_aom_smooth_v_predictor_64x64_c, svt_aom_smooth_v_predictor_64x64_ssse3);

    SET_SSSE3(svt_aom_smooth_predictor_4x4, svt_aom_smooth_predictor_4x4_c, svt_aom_smooth_predictor_4x4_ssse3);
    SET_SSSE3(svt_aom_smooth_predictor_4x8, svt_aom_smooth_predictor_4x8_c, svt_aom_smooth_predictor_4x8_ssse3);
    SET_SSSE3(svt_aom_smooth_predictor_4x16, svt_aom_smooth_predictor_4x16_c, svt_aom_smooth_predictor_4x16_ssse3);
    SET_SSSE3(svt_aom_smooth_predictor_8x4, svt_aom_smooth_predictor_8x4_c, svt_aom_smooth_predictor_8x4_ssse3);
    SET_SSSE3(svt_aom_smooth_predictor_8x8, svt_aom_smooth_predictor_8x8_c, svt_aom_smooth_predictor_8x8_ssse3);
    SET_SSSE3(svt_aom_smooth_predictor_8x16, svt_aom_smooth_predictor_8x16_c, svt_aom_smooth_predictor_8x16_ssse3);
    SET_SSSE3(svt_aom_smooth_predictor_8x32, svt_aom_smooth_predictor_8x32_c, svt_aom_smooth_predictor_8x32_ssse3);
    SET_SSSE3(svt_aom_smooth_predictor_16x4, svt_aom_smooth_predictor_16x4_c, svt_aom_smooth_predictor_16x4_ssse3);
    SET_SSSE3(svt_aom_smooth_predictor_16x8, svt_aom_smooth_predictor_16x8_c, svt_aom_smooth_predictor_16x8_ssse3);
    SET_SSSE3(svt_aom_smooth_predictor_16x16, svt_aom_smooth_predictor_16x16_c, svt_aom_smooth_predictor_16x16_ssse3);
    SET_SSSE3(svt_aom_smooth_predictor_16x32, svt_aom_smooth_predictor_16x32_c, svt_aom_smooth_predictor_16x32_ssse3);
    SET_SSSE3(svt_aom_smooth_predictor_16x64, svt_aom_smooth_predictor_16x64_c, svt_aom_smooth_predictor_16x64_ssse3);
    SET_SSSE3(svt_aom_smooth_predictor_32x8, svt_aom_smooth_predictor_32x8_c, svt_aom_smooth_predictor_32x8_ssse3);
    SET_SSSE3(svt_aom_smooth_predictor_32x16, svt_aom_smooth_predictor_32x16_c, svt_aom_smooth_predictor_32x16_ssse3);
    SET_SSSE3(svt_aom_smooth_predictor_32x32, svt_aom_smooth_predictor_32x32_c, svt_aom_smooth_predictor_32x32_ssse3);
    SET_SSSE3(svt_aom_smooth_predictor_32x64, svt_aom_smooth_predictor_32x64_c, svt_aom_smooth_predictor_32x64_ssse3);
    SET_SSSE3(svt_aom_smooth_predictor_64x16, svt_aom_smooth_predictor_64x16_c, svt_aom_smooth_predictor_64x16_ssse3);
    SET_SSSE3(svt_aom_smooth_predictor_64x32, svt_aom_smooth_predictor_64x32_c, svt_aom_smooth_predictor_64x32_ssse3);
    SET_SSSE3(svt_aom_smooth_predictor_64x64, svt_aom_smooth_predictor_64x64_c, svt_aom_smooth_predictor_64x64_ssse3);

    SET_SSE2(svt_aom_v_predictor_4x4, svt_aom_v_predictor_4x4_c, svt_aom_v_predictor_4x4_sse2);
    SET_SSE2(svt_aom_v_predictor_4x8, svt_aom_v_predictor_4x8_c, svt_aom_v_predictor_4x8_sse2);
    SET_SSE2(svt_aom_v_predictor_4x16, svt_aom_v_predictor_4x16_c, svt_aom_v_predictor_4x16_sse2);
    SET_SSE2(svt_aom_v_predictor_8x4, svt_aom_v_predictor_8x4_c, svt_aom_v_predictor_8x4_sse2);
    SET_SSE2(svt_aom_v_predictor_8x8, svt_aom_v_predictor_8x8_c, svt_aom_v_predictor_8x8_sse2);
    SET_SSE2(svt_aom_v_predictor_8x16, svt_aom_v_predictor_8x16_c, svt_aom_v_predictor_8x16_sse2);
    SET_SSE2(svt_aom_v_predictor_8x32, svt_aom_v_predictor_8x32_c, svt_aom_v_predictor_8x32_sse2);
    SET_SSE2(svt_aom_v_predictor_16x4, svt_aom_v_predictor_16x4_c, svt_aom_v_predictor_16x4_sse2);
    SET_SSE2(svt_aom_v_predictor_16x8, svt_aom_v_predictor_16x8_c, svt_aom_v_predictor_16x8_sse2);
    SET_SSE2(svt_aom_v_predictor_16x16, svt_aom_v_predictor_16x16_c, svt_aom_v_predictor_16x16_sse2);
    SET_SSE2(svt_aom_v_predictor_16x32, svt_aom_v_predictor_16x32_c, svt_aom_v_predictor_16x32_sse2);
    SET_SSE2(svt_aom_v_predictor_16x64, svt_aom_v_predictor_16x64_c, svt_aom_v_predictor_16x64_sse2);
    SET_SSE2(svt_aom_v_predictor_32x8, svt_aom_v_predictor_32x8_c, svt_aom_v_predictor_32x8_sse2);
    SET_AVX2(svt_aom_v_predictor_32x16, svt_aom_v_predictor_32x16_c, svt_aom_v_predictor_32x16_avx2);
    SET_AVX2(svt_aom_v_predictor_32x32, svt_aom_v_predictor_32x32_c, svt_aom_v_predictor_32x32_avx2);
    SET_AVX2(svt_aom_v_predictor_32x64, svt_aom_v_predictor_32x64_c, svt_aom_v_predictor_32x64_avx2);
    SET_AVX2(svt_aom_v_predictor_64x16, svt_aom_v_predictor_64x16_c, svt_aom_v_predictor_64x16_avx2);
    SET_AVX2(svt_aom_v_predictor_64x32, svt_aom_v_predictor_64x32_c, svt_aom_v_predictor_64x32_avx2);
    SET_AVX2(svt_aom_v_predictor_64x64, svt_aom_v_predictor_64x64_c, svt_aom_v_predictor_64x64_avx2);

    SET_SSE2(svt_aom_h_predictor_4x4, svt_aom_h_predictor_4x4_c, svt_aom_h_predictor_4x4_sse2);
    SET_SSE2(svt_aom_h_predictor_4x8, svt_aom_h_predictor_4x8_c, svt_aom_h_predictor_4x8_sse2);
    SET_SSE2(svt_aom_h_predictor_4x16, svt_aom_h_predictor_4x16_c, svt_aom_h_predictor_4x16_sse2);
    SET_SSE2(svt_aom_h_predictor_8x4, svt_aom_h_predictor_8x4_c, svt_aom_h_predictor_8x4_sse2);
    SET_SSE2(svt_aom_h_predictor_8x8, svt_aom_h_predictor_8x8_c, svt_aom_h_predictor_8x8_sse2);
    SET_SSE2(svt_aom_h_predictor_8x16, svt_aom_h_predictor_8x16_c, svt_aom_h_predictor_8x16_sse2);
    SET_SSE2(svt_aom_h_predictor_8x32, svt_aom_h_predictor_8x32_c, svt_aom_h_predictor_8x32_sse2);
    SET_SSE2(svt_aom_h_predictor_16x4, svt_aom_h_predictor_16x4_c, svt_aom_h_predictor_16x4_sse2);
    SET_SSE2(svt_aom_h_predictor_16x8, svt_aom_h_predictor_16x8_c, svt_aom_h_predictor_16x8_sse2);
    SET_SSE2(svt_aom_h_predictor_16x16, svt_aom_h_predictor_16x16_c, svt_aom_h_predictor_16x16_sse2);
    SET_SSE2(svt_aom_h_predictor_16x32, svt_aom_h_predictor_16x32_c, svt_aom_h_predictor_16x32_sse2);
    SET_SSE2(svt_aom_h_predictor_16x64, svt_aom_h_predictor_16x64_c, svt_aom_h_predictor_16x64_sse2);
    SET_SSE2(svt_aom_h_predictor_32x8, svt_aom_h_predictor_32x8_c, svt_aom_h_predictor_32x8_sse2);
    SET_SSE2(svt_aom_h_predictor_32x16, svt_aom_h_predictor_32x16_c, svt_aom_h_predictor_32x16_sse2);
    SET_AVX2(svt_aom_h_predictor_32x32, svt_aom_h_predictor_32x32_c, svt_aom_h_predictor_32x32_avx2);
    SET_SSE2(svt_aom_h_predictor_32x64, svt_aom_h_predictor_32x64_c, svt_aom_h_predictor_32x64_sse2);
    SET_SSE2(svt_aom_h_predictor_64x16, svt_aom_h_predictor_64x16_c, svt_aom_h_predictor_64x16_sse2);
    SET_SSE2(svt_aom_h_predictor_64x32, svt_aom_h_predictor_64x32_c, svt_aom_h_predictor_64x32_sse2);
    SET_SSE2(svt_aom_h_predictor_64x64, svt_aom_h_predictor_64x64_c, svt_aom_h_predictor_64x64_sse2);
    SET_SSE41_AVX2(svt_aom_cdef_find_dir, svt_aom_cdef_find_dir_c, svt_aom_cdef_find_dir_sse4_1, svt_aom_cdef_find_dir_avx2);
    SET_SSE41_AVX2(svt_aom_cdef_find_dir_dual, svt_aom_cdef_find_dir_dual_c, svt_aom_cdef_find_dir_dual_sse4_1, svt_aom_cdef_find_dir_dual_avx2);
    SET_SSE41_AVX2(svt_cdef_filter_block, svt_cdef_filter_block_c, svt_av1_cdef_filter_block_sse4_1, svt_cdef_filter_block_avx2);
    /* No C version, use only internal in kerneal: svt_cdef_filter_block_avx2() */
    if (flags & EB_CPU_FLAGS_AVX2)    svt_cdef_filter_block_8xn_16 = svt_cdef_filter_block_8xn_16_avx2;
#if EN_AVX512_SUPPORT
    if (flags & EB_CPU_FLAGS_AVX512F) svt_cdef_filter_block_8xn_16 = svt_cdef_filter_block_8xn_16_avx512;
#endif
    SET_SSE41_AVX2(svt_aom_copy_rect8_8bit_to_16bit, svt_aom_copy_rect8_8bit_to_16bit_c, svt_aom_copy_rect8_8bit_to_16bit_sse4_1, svt_aom_copy_rect8_8bit_to_16bit_avx2);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_SSE41_AVX2(svt_av1_highbd_warp_affine, svt_av1_highbd_warp_affine_c, svt_av1_highbd_warp_affine_sse4_1, svt_av1_highbd_warp_affine_avx2);
#endif
    SET_SSE41_AVX2(svt_av1_warp_affine, svt_av1_warp_affine_c, svt_av1_warp_affine_sse4_1, svt_av1_warp_affine_avx2);

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_SSE2(svt_aom_highbd_lpf_horizontal_4, svt_aom_highbd_lpf_horizontal_4_c, svt_aom_highbd_lpf_horizontal_4_sse2);
    SET_SSE2(svt_aom_highbd_lpf_horizontal_6, svt_aom_highbd_lpf_horizontal_6_c, svt_aom_highbd_lpf_horizontal_6_sse2);
    SET_SSE2(svt_aom_highbd_lpf_horizontal_8, svt_aom_highbd_lpf_horizontal_8_c, svt_aom_highbd_lpf_horizontal_8_sse2);
    SET_SSE2(svt_aom_highbd_lpf_horizontal_14, svt_aom_highbd_lpf_horizontal_14_c, svt_aom_highbd_lpf_horizontal_14_sse2);
    SET_SSE2(svt_aom_highbd_lpf_vertical_4, svt_aom_highbd_lpf_vertical_4_c, svt_aom_highbd_lpf_vertical_4_sse2);
    SET_SSE2(svt_aom_highbd_lpf_vertical_6, svt_aom_highbd_lpf_vertical_6_c, svt_aom_highbd_lpf_vertical_6_sse2);
    SET_SSE2(svt_aom_highbd_lpf_vertical_8, svt_aom_highbd_lpf_vertical_8_c, svt_aom_highbd_lpf_vertical_8_sse2);
    SET_SSE2(svt_aom_highbd_lpf_vertical_14, svt_aom_highbd_lpf_vertical_14_c, svt_aom_highbd_lpf_vertical_14_sse2);
#endif
    SET_SSE2(svt_aom_lpf_horizontal_4, svt_aom_lpf_horizontal_4_c, svt_aom_lpf_horizontal_4_sse2);
    SET_SSE2(svt_aom_lpf_horizontal_6, svt_aom_lpf_horizontal_6_c, svt_aom_lpf_horizontal_6_sse2);
    SET_SSE2(svt_aom_lpf_horizontal_8, svt_aom_lpf_horizontal_8_c, svt_aom_lpf_horizontal_8_sse2);
    SET_SSE2(svt_aom_lpf_horizontal_14, svt_aom_lpf_horizontal_14_c, svt_aom_lpf_horizontal_14_sse2);
    SET_SSE2(svt_aom_lpf_vertical_4, svt_aom_lpf_vertical_4_c, svt_aom_lpf_vertical_4_sse2);
    SET_SSE2(svt_aom_lpf_vertical_6, svt_aom_lpf_vertical_6_c, svt_aom_lpf_vertical_6_sse2);
    SET_SSE2(svt_aom_lpf_vertical_8, svt_aom_lpf_vertical_8_c, svt_aom_lpf_vertical_8_sse2);
    SET_SSE2(svt_aom_lpf_vertical_14, svt_aom_lpf_vertical_14_c, svt_aom_lpf_vertical_14_sse2);

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    // svt_aom_highbd_v_predictor
    SET_SSE2(svt_aom_highbd_v_predictor_4x4, svt_aom_highbd_v_predictor_4x4_c, svt_aom_highbd_v_predictor_4x4_sse2);
    SET_SSE2(svt_aom_highbd_v_predictor_4x8, svt_aom_highbd_v_predictor_4x8_c, svt_aom_highbd_v_predictor_4x8_sse2);
    SET_SSE2(svt_aom_highbd_v_predictor_4x16, svt_aom_highbd_v_predictor_4x16_c, svt_aom_highbd_v_predictor_4x16_sse2);
    SET_SSE2(svt_aom_highbd_v_predictor_8x4, svt_aom_highbd_v_predictor_8x4_c, svt_aom_highbd_v_predictor_8x4_sse2);
    SET_SSE2(svt_aom_highbd_v_predictor_8x8, svt_aom_highbd_v_predictor_8x8_c, svt_aom_highbd_v_predictor_8x8_sse2);
    SET_SSE2(svt_aom_highbd_v_predictor_8x16, svt_aom_highbd_v_predictor_8x16_c, svt_aom_highbd_v_predictor_8x16_sse2);
    SET_SSE2(svt_aom_highbd_v_predictor_8x32, svt_aom_highbd_v_predictor_8x32_c, svt_aom_highbd_v_predictor_8x32_sse2);
    SET_AVX2(svt_aom_highbd_v_predictor_16x4, svt_aom_highbd_v_predictor_16x4_c, svt_aom_highbd_v_predictor_16x4_avx2);
    SET_AVX2(svt_aom_highbd_v_predictor_16x8, svt_aom_highbd_v_predictor_16x8_c, svt_aom_highbd_v_predictor_16x8_avx2);
    SET_AVX2(svt_aom_highbd_v_predictor_16x16, svt_aom_highbd_v_predictor_16x16_c, svt_aom_highbd_v_predictor_16x16_avx2);
    SET_AVX2(svt_aom_highbd_v_predictor_16x32, svt_aom_highbd_v_predictor_16x32_c, svt_aom_highbd_v_predictor_16x32_avx2);
    SET_AVX2(svt_aom_highbd_v_predictor_16x64, svt_aom_highbd_v_predictor_16x64_c, svt_aom_highbd_v_predictor_16x64_avx2);
    SET_AVX2_AVX512(svt_aom_highbd_v_predictor_32x8, svt_aom_highbd_v_predictor_32x8_c, svt_aom_highbd_v_predictor_32x8_avx2, aom_highbd_v_predictor_32x8_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_v_predictor_32x16, svt_aom_highbd_v_predictor_32x16_c, svt_aom_highbd_v_predictor_32x16_avx2, aom_highbd_v_predictor_32x16_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_v_predictor_32x32, svt_aom_highbd_v_predictor_32x32_c, svt_aom_highbd_v_predictor_32x32_avx2, aom_highbd_v_predictor_32x32_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_v_predictor_32x64, svt_aom_highbd_v_predictor_32x64_c, svt_aom_highbd_v_predictor_32x64_avx2, aom_highbd_v_predictor_32x64_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_v_predictor_64x16, svt_aom_highbd_v_predictor_64x16_c, svt_aom_highbd_v_predictor_64x16_avx2, aom_highbd_v_predictor_64x16_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_v_predictor_64x32, svt_aom_highbd_v_predictor_64x32_c, svt_aom_highbd_v_predictor_64x32_avx2, aom_highbd_v_predictor_64x32_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_v_predictor_64x64, svt_aom_highbd_v_predictor_64x64_c, svt_aom_highbd_v_predictor_64x64_avx2, aom_highbd_v_predictor_64x64_avx512);

    //aom_highbd_smooth_predictor
    SET_SSSE3(svt_aom_highbd_smooth_predictor_4x4, svt_aom_highbd_smooth_predictor_4x4_c, svt_aom_highbd_smooth_predictor_4x4_ssse3);
    SET_SSSE3(svt_aom_highbd_smooth_predictor_4x8, svt_aom_highbd_smooth_predictor_4x8_c, svt_aom_highbd_smooth_predictor_4x8_ssse3);
    SET_SSSE3(svt_aom_highbd_smooth_predictor_4x16, svt_aom_highbd_smooth_predictor_4x16_c, svt_aom_highbd_smooth_predictor_4x16_ssse3);
    SET_AVX2(svt_aom_highbd_smooth_predictor_8x4, svt_aom_highbd_smooth_predictor_8x4_c, svt_aom_highbd_smooth_predictor_8x4_avx2);
    SET_AVX2(svt_aom_highbd_smooth_predictor_8x8, svt_aom_highbd_smooth_predictor_8x8_c, svt_aom_highbd_smooth_predictor_8x8_avx2);
    SET_AVX2(svt_aom_highbd_smooth_predictor_8x16, svt_aom_highbd_smooth_predictor_8x16_c, svt_aom_highbd_smooth_predictor_8x16_avx2);
    SET_AVX2(svt_aom_highbd_smooth_predictor_8x32, svt_aom_highbd_smooth_predictor_8x32_c, svt_aom_highbd_smooth_predictor_8x32_avx2);
    SET_AVX2(svt_aom_highbd_smooth_predictor_16x4, svt_aom_highbd_smooth_predictor_16x4_c, svt_aom_highbd_smooth_predictor_16x4_avx2);
    SET_AVX2(svt_aom_highbd_smooth_predictor_16x8, svt_aom_highbd_smooth_predictor_16x8_c, svt_aom_highbd_smooth_predictor_16x8_avx2);
    SET_AVX2(svt_aom_highbd_smooth_predictor_16x16, svt_aom_highbd_smooth_predictor_16x16_c, svt_aom_highbd_smooth_predictor_16x16_avx2);
    SET_AVX2(svt_aom_highbd_smooth_predictor_16x32, svt_aom_highbd_smooth_predictor_16x32_c, svt_aom_highbd_smooth_predictor_16x32_avx2);
    SET_AVX2(svt_aom_highbd_smooth_predictor_16x64, svt_aom_highbd_smooth_predictor_16x64_c, svt_aom_highbd_smooth_predictor_16x64_avx2);
    SET_AVX2_AVX512(svt_aom_highbd_smooth_predictor_32x8, svt_aom_highbd_smooth_predictor_32x8_c, svt_aom_highbd_smooth_predictor_32x8_avx2, aom_highbd_smooth_predictor_32x8_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_smooth_predictor_32x16, svt_aom_highbd_smooth_predictor_32x16_c, svt_aom_highbd_smooth_predictor_32x16_avx2, aom_highbd_smooth_predictor_32x16_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_smooth_predictor_32x32, svt_aom_highbd_smooth_predictor_32x32_c, svt_aom_highbd_smooth_predictor_32x32_avx2, aom_highbd_smooth_predictor_32x32_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_smooth_predictor_32x64, svt_aom_highbd_smooth_predictor_32x64_c, svt_aom_highbd_smooth_predictor_32x64_avx2, aom_highbd_smooth_predictor_32x64_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_smooth_predictor_64x16, svt_aom_highbd_smooth_predictor_64x16_c, svt_aom_highbd_smooth_predictor_64x16_avx2, aom_highbd_smooth_predictor_64x16_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_smooth_predictor_64x32, svt_aom_highbd_smooth_predictor_64x32_c, svt_aom_highbd_smooth_predictor_64x32_avx2, aom_highbd_smooth_predictor_64x32_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_smooth_predictor_64x64, svt_aom_highbd_smooth_predictor_64x64_c, svt_aom_highbd_smooth_predictor_64x64_avx2, aom_highbd_smooth_predictor_64x64_avx512);

    //aom_highbd_smooth_h_predictor
    SET_SSSE3(svt_aom_highbd_smooth_h_predictor_4x4, svt_aom_highbd_smooth_h_predictor_4x4_c, svt_aom_highbd_smooth_h_predictor_4x4_ssse3);
    SET_SSSE3(svt_aom_highbd_smooth_h_predictor_4x8, svt_aom_highbd_smooth_h_predictor_4x8_c, svt_aom_highbd_smooth_h_predictor_4x8_ssse3);
    SET_SSSE3(svt_aom_highbd_smooth_h_predictor_4x16, svt_aom_highbd_smooth_h_predictor_4x16_c, svt_aom_highbd_smooth_h_predictor_4x16_ssse3);
    SET_AVX2(svt_aom_highbd_smooth_h_predictor_8x4, svt_aom_highbd_smooth_h_predictor_8x4_c, svt_aom_highbd_smooth_h_predictor_8x4_avx2);
    SET_AVX2(svt_aom_highbd_smooth_h_predictor_8x8, svt_aom_highbd_smooth_h_predictor_8x8_c, svt_aom_highbd_smooth_h_predictor_8x8_avx2);
    SET_AVX2(svt_aom_highbd_smooth_h_predictor_8x16, svt_aom_highbd_smooth_h_predictor_8x16_c, svt_aom_highbd_smooth_h_predictor_8x16_avx2);
    SET_AVX2(svt_aom_highbd_smooth_h_predictor_8x32, svt_aom_highbd_smooth_h_predictor_8x32_c, svt_aom_highbd_smooth_h_predictor_8x32_avx2);
    SET_AVX2(svt_aom_highbd_smooth_h_predictor_16x4, svt_aom_highbd_smooth_h_predictor_16x4_c, svt_aom_highbd_smooth_h_predictor_16x4_avx2);
    SET_AVX2(svt_aom_highbd_smooth_h_predictor_16x8, svt_aom_highbd_smooth_h_predictor_16x8_c, svt_aom_highbd_smooth_h_predictor_16x8_avx2);
    SET_AVX2(svt_aom_highbd_smooth_h_predictor_16x16, svt_aom_highbd_smooth_h_predictor_16x16_c, svt_aom_highbd_smooth_h_predictor_16x16_avx2);
    SET_AVX2(svt_aom_highbd_smooth_h_predictor_16x32, svt_aom_highbd_smooth_h_predictor_16x32_c, svt_aom_highbd_smooth_h_predictor_16x32_avx2);
    SET_AVX2(svt_aom_highbd_smooth_h_predictor_16x64, svt_aom_highbd_smooth_h_predictor_16x64_c, svt_aom_highbd_smooth_h_predictor_16x64_avx2);
    SET_AVX2_AVX512(svt_aom_highbd_smooth_h_predictor_32x8, svt_aom_highbd_smooth_h_predictor_32x8_c, svt_aom_highbd_smooth_h_predictor_32x8_avx2, aom_highbd_smooth_h_predictor_32x8_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_smooth_h_predictor_32x16, svt_aom_highbd_smooth_h_predictor_32x16_c, svt_aom_highbd_smooth_h_predictor_32x16_avx2, aom_highbd_smooth_h_predictor_32x16_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_smooth_h_predictor_32x32, svt_aom_highbd_smooth_h_predictor_32x32_c, svt_aom_highbd_smooth_h_predictor_32x32_avx2, aom_highbd_smooth_h_predictor_32x32_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_smooth_h_predictor_32x64, svt_aom_highbd_smooth_h_predictor_32x64_c, svt_aom_highbd_smooth_h_predictor_32x64_avx2, aom_highbd_smooth_h_predictor_32x64_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_smooth_h_predictor_64x16, svt_aom_highbd_smooth_h_predictor_64x16_c, svt_aom_highbd_smooth_h_predictor_64x16_avx2, aom_highbd_smooth_h_predictor_64x16_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_smooth_h_predictor_64x32, svt_aom_highbd_smooth_h_predictor_64x32_c, svt_aom_highbd_smooth_h_predictor_64x32_avx2, aom_highbd_smooth_h_predictor_64x32_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_smooth_h_predictor_64x64, svt_aom_highbd_smooth_h_predictor_64x64_c, svt_aom_highbd_smooth_h_predictor_64x64_avx2, aom_highbd_smooth_h_predictor_64x64_avx512);
#endif

    //aom_highbd_dc_128_predictor
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_SSE2(svt_aom_highbd_dc_128_predictor_4x4, svt_aom_highbd_dc_128_predictor_4x4_c, svt_aom_highbd_dc_128_predictor_4x4_sse2);
    SET_SSE2(svt_aom_highbd_dc_128_predictor_4x8, svt_aom_highbd_dc_128_predictor_4x8_c, svt_aom_highbd_dc_128_predictor_4x8_sse2);
    SET_SSE2(svt_aom_highbd_dc_128_predictor_4x16, svt_aom_highbd_dc_128_predictor_4x16_c, svt_aom_highbd_dc_128_predictor_4x16_sse2);
    SET_SSE2(svt_aom_highbd_dc_128_predictor_8x4, svt_aom_highbd_dc_128_predictor_8x4_c, svt_aom_highbd_dc_128_predictor_8x4_sse2);
    SET_SSE2(svt_aom_highbd_dc_128_predictor_8x8, svt_aom_highbd_dc_128_predictor_8x8_c, svt_aom_highbd_dc_128_predictor_8x8_sse2);
    SET_SSE2(svt_aom_highbd_dc_128_predictor_8x16, svt_aom_highbd_dc_128_predictor_8x16_c, svt_aom_highbd_dc_128_predictor_8x16_sse2);
    SET_SSE2(svt_aom_highbd_dc_128_predictor_8x32, svt_aom_highbd_dc_128_predictor_8x32_c, svt_aom_highbd_dc_128_predictor_8x32_sse2);
    SET_AVX2(svt_aom_highbd_dc_128_predictor_16x4, svt_aom_highbd_dc_128_predictor_16x4_c, svt_aom_highbd_dc_128_predictor_16x4_avx2);
    SET_AVX2(svt_aom_highbd_dc_128_predictor_16x8, svt_aom_highbd_dc_128_predictor_16x8_c, svt_aom_highbd_dc_128_predictor_16x8_avx2);
    SET_AVX2(svt_aom_highbd_dc_128_predictor_16x16, svt_aom_highbd_dc_128_predictor_16x16_c, svt_aom_highbd_dc_128_predictor_16x16_avx2);
    SET_AVX2(svt_aom_highbd_dc_128_predictor_16x32, svt_aom_highbd_dc_128_predictor_16x32_c, svt_aom_highbd_dc_128_predictor_16x32_avx2);
    SET_AVX2(svt_aom_highbd_dc_128_predictor_16x64, svt_aom_highbd_dc_128_predictor_16x64_c, svt_aom_highbd_dc_128_predictor_16x64_avx2);
    SET_AVX2(svt_aom_highbd_dc_128_predictor_32x8, svt_aom_highbd_dc_128_predictor_32x8_c, svt_aom_highbd_dc_128_predictor_32x8_avx2);
    SET_AVX2(svt_aom_highbd_dc_128_predictor_32x16, svt_aom_highbd_dc_128_predictor_32x16_c, svt_aom_highbd_dc_128_predictor_32x16_avx2);
    SET_AVX2(svt_aom_highbd_dc_128_predictor_32x32, svt_aom_highbd_dc_128_predictor_32x32_c, svt_aom_highbd_dc_128_predictor_32x32_avx2);
    SET_AVX2(svt_aom_highbd_dc_128_predictor_32x64, svt_aom_highbd_dc_128_predictor_32x64_c, svt_aom_highbd_dc_128_predictor_32x64_avx2);
    SET_AVX2(svt_aom_highbd_dc_128_predictor_64x16, svt_aom_highbd_dc_128_predictor_64x16_c, svt_aom_highbd_dc_128_predictor_64x16_avx2);
    SET_AVX2(svt_aom_highbd_dc_128_predictor_64x32, svt_aom_highbd_dc_128_predictor_64x32_c, svt_aom_highbd_dc_128_predictor_64x32_avx2);
    SET_AVX2(svt_aom_highbd_dc_128_predictor_64x64, svt_aom_highbd_dc_128_predictor_64x64_c, svt_aom_highbd_dc_128_predictor_64x64_avx2);

    //aom_highbd_dc_left_predictor
    SET_SSE2(svt_aom_highbd_dc_left_predictor_4x4, svt_aom_highbd_dc_left_predictor_4x4_c, svt_aom_highbd_dc_left_predictor_4x4_sse2);
    SET_SSE2(svt_aom_highbd_dc_left_predictor_4x8, svt_aom_highbd_dc_left_predictor_4x8_c, svt_aom_highbd_dc_left_predictor_4x8_sse2);
    SET_SSE2(svt_aom_highbd_dc_left_predictor_4x16, svt_aom_highbd_dc_left_predictor_4x16_c, svt_aom_highbd_dc_left_predictor_4x16_sse2);
    SET_SSE2(svt_aom_highbd_dc_left_predictor_8x4, svt_aom_highbd_dc_left_predictor_8x4_c, svt_aom_highbd_dc_left_predictor_8x4_sse2);
    SET_SSE2(svt_aom_highbd_dc_left_predictor_8x8, svt_aom_highbd_dc_left_predictor_8x8_c, svt_aom_highbd_dc_left_predictor_8x8_sse2);
    SET_SSE2(svt_aom_highbd_dc_left_predictor_8x16, svt_aom_highbd_dc_left_predictor_8x16_c, svt_aom_highbd_dc_left_predictor_8x16_sse2);
    SET_SSE2(svt_aom_highbd_dc_left_predictor_8x32, svt_aom_highbd_dc_left_predictor_8x32_c, svt_aom_highbd_dc_left_predictor_8x32_sse2);
    SET_AVX2(svt_aom_highbd_dc_left_predictor_16x4, svt_aom_highbd_dc_left_predictor_16x4_c, svt_aom_highbd_dc_left_predictor_16x4_avx2);
    SET_AVX2(svt_aom_highbd_dc_left_predictor_16x8, svt_aom_highbd_dc_left_predictor_16x8_c, svt_aom_highbd_dc_left_predictor_16x8_avx2);
    SET_AVX2(svt_aom_highbd_dc_left_predictor_16x16, svt_aom_highbd_dc_left_predictor_16x16_c, svt_aom_highbd_dc_left_predictor_16x16_avx2);
    SET_AVX2(svt_aom_highbd_dc_left_predictor_16x32, svt_aom_highbd_dc_left_predictor_16x32_c, svt_aom_highbd_dc_left_predictor_16x32_avx2);
    SET_AVX2(svt_aom_highbd_dc_left_predictor_16x64, svt_aom_highbd_dc_left_predictor_16x64_c, svt_aom_highbd_dc_left_predictor_16x64_avx2);
    SET_AVX2_AVX512(svt_aom_highbd_dc_left_predictor_32x8, svt_aom_highbd_dc_left_predictor_32x8_c, svt_aom_highbd_dc_left_predictor_32x8_avx2, aom_highbd_dc_left_predictor_32x8_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_dc_left_predictor_32x16, svt_aom_highbd_dc_left_predictor_32x16_c, svt_aom_highbd_dc_left_predictor_32x16_avx2, aom_highbd_dc_left_predictor_32x16_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_dc_left_predictor_32x32, svt_aom_highbd_dc_left_predictor_32x32_c, svt_aom_highbd_dc_left_predictor_32x32_avx2, aom_highbd_dc_left_predictor_32x32_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_dc_left_predictor_32x64, svt_aom_highbd_dc_left_predictor_32x64_c, svt_aom_highbd_dc_left_predictor_32x64_avx2, aom_highbd_dc_left_predictor_32x64_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_dc_left_predictor_64x16, svt_aom_highbd_dc_left_predictor_64x16_c, svt_aom_highbd_dc_left_predictor_64x16_avx2, aom_highbd_dc_left_predictor_64x16_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_dc_left_predictor_64x32, svt_aom_highbd_dc_left_predictor_64x32_c, svt_aom_highbd_dc_left_predictor_64x32_avx2, aom_highbd_dc_left_predictor_64x32_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_dc_left_predictor_64x64, svt_aom_highbd_dc_left_predictor_64x64_c, svt_aom_highbd_dc_left_predictor_64x64_avx2, aom_highbd_dc_left_predictor_64x64_avx512);

    SET_SSE2(svt_aom_highbd_dc_predictor_4x4, svt_aom_highbd_dc_predictor_4x4_c, svt_aom_highbd_dc_predictor_4x4_sse2);
    SET_SSE2(svt_aom_highbd_dc_predictor_4x8, svt_aom_highbd_dc_predictor_4x8_c, svt_aom_highbd_dc_predictor_4x8_sse2);
    SET_SSE2(svt_aom_highbd_dc_predictor_4x16, svt_aom_highbd_dc_predictor_4x16_c, svt_aom_highbd_dc_predictor_4x16_sse2);
    SET_SSE2(svt_aom_highbd_dc_predictor_8x4, svt_aom_highbd_dc_predictor_8x4_c, svt_aom_highbd_dc_predictor_8x4_sse2);
    SET_SSE2(svt_aom_highbd_dc_predictor_8x8, svt_aom_highbd_dc_predictor_8x8_c, svt_aom_highbd_dc_predictor_8x8_sse2);
    SET_SSE2(svt_aom_highbd_dc_predictor_8x16, svt_aom_highbd_dc_predictor_8x16_c, svt_aom_highbd_dc_predictor_8x16_sse2);
    SET_SSE2(svt_aom_highbd_dc_predictor_8x32, svt_aom_highbd_dc_predictor_8x32_c, svt_aom_highbd_dc_predictor_8x32_sse2);
    SET_AVX2(svt_aom_highbd_dc_predictor_16x4, svt_aom_highbd_dc_predictor_16x4_c, svt_aom_highbd_dc_predictor_16x4_avx2);
    SET_AVX2(svt_aom_highbd_dc_predictor_16x8, svt_aom_highbd_dc_predictor_16x8_c, svt_aom_highbd_dc_predictor_16x8_avx2);
    SET_AVX2(svt_aom_highbd_dc_predictor_16x16, svt_aom_highbd_dc_predictor_16x16_c, svt_aom_highbd_dc_predictor_16x16_avx2);
    SET_AVX2(svt_aom_highbd_dc_predictor_16x32, svt_aom_highbd_dc_predictor_16x32_c, svt_aom_highbd_dc_predictor_16x32_avx2);
    SET_AVX2(svt_aom_highbd_dc_predictor_16x64, svt_aom_highbd_dc_predictor_16x64_c, svt_aom_highbd_dc_predictor_16x64_avx2);
    SET_AVX2_AVX512(svt_aom_highbd_dc_predictor_32x8, svt_aom_highbd_dc_predictor_32x8_c, svt_aom_highbd_dc_predictor_32x8_avx2, aom_highbd_dc_predictor_32x8_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_dc_predictor_32x16, svt_aom_highbd_dc_predictor_32x16_c, svt_aom_highbd_dc_predictor_32x16_avx2, aom_highbd_dc_predictor_32x16_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_dc_predictor_32x32, svt_aom_highbd_dc_predictor_32x32_c, svt_aom_highbd_dc_predictor_32x32_avx2, aom_highbd_dc_predictor_32x32_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_dc_predictor_32x64, svt_aom_highbd_dc_predictor_32x64_c, svt_aom_highbd_dc_predictor_32x64_avx2, aom_highbd_dc_predictor_32x64_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_dc_predictor_64x16, svt_aom_highbd_dc_predictor_64x16_c, svt_aom_highbd_dc_predictor_64x16_avx2, aom_highbd_dc_predictor_64x16_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_dc_predictor_64x32, svt_aom_highbd_dc_predictor_64x32_c, svt_aom_highbd_dc_predictor_64x32_avx2, aom_highbd_dc_predictor_64x32_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_dc_predictor_64x64, svt_aom_highbd_dc_predictor_64x64_c, svt_aom_highbd_dc_predictor_64x64_avx2, aom_highbd_dc_predictor_64x64_avx512);

    //aom_highbd_dc_top_predictor
    SET_SSE2(svt_aom_highbd_dc_top_predictor_4x4, svt_aom_highbd_dc_top_predictor_4x4_c, svt_aom_highbd_dc_top_predictor_4x4_sse2);
    SET_SSE2(svt_aom_highbd_dc_top_predictor_4x8, svt_aom_highbd_dc_top_predictor_4x8_c, svt_aom_highbd_dc_top_predictor_4x8_sse2);
    SET_SSE2(svt_aom_highbd_dc_top_predictor_4x16, svt_aom_highbd_dc_top_predictor_4x16_c, svt_aom_highbd_dc_top_predictor_4x16_sse2);
    SET_SSE2(svt_aom_highbd_dc_top_predictor_8x4, svt_aom_highbd_dc_top_predictor_8x4_c, svt_aom_highbd_dc_top_predictor_8x4_sse2);
    SET_SSE2(svt_aom_highbd_dc_top_predictor_8x8, svt_aom_highbd_dc_top_predictor_8x8_c, svt_aom_highbd_dc_top_predictor_8x8_sse2);
    SET_SSE2(svt_aom_highbd_dc_top_predictor_8x16, svt_aom_highbd_dc_top_predictor_8x16_c, svt_aom_highbd_dc_top_predictor_8x16_sse2);
    SET_ONLY_C(svt_aom_highbd_dc_top_predictor_8x32, svt_aom_highbd_dc_top_predictor_8x32_c);
    SET_AVX2(svt_aom_highbd_dc_top_predictor_16x4, svt_aom_highbd_dc_top_predictor_16x4_c, svt_aom_highbd_dc_top_predictor_16x4_avx2);
    SET_AVX2(svt_aom_highbd_dc_top_predictor_16x8, svt_aom_highbd_dc_top_predictor_16x8_c, svt_aom_highbd_dc_top_predictor_16x8_avx2);
    SET_AVX2(svt_aom_highbd_dc_top_predictor_16x16, svt_aom_highbd_dc_top_predictor_16x16_c, svt_aom_highbd_dc_top_predictor_16x16_avx2);
    SET_AVX2(svt_aom_highbd_dc_top_predictor_16x32, svt_aom_highbd_dc_top_predictor_16x32_c, svt_aom_highbd_dc_top_predictor_16x32_avx2);
    SET_AVX2(svt_aom_highbd_dc_top_predictor_16x64, svt_aom_highbd_dc_top_predictor_16x64_c, svt_aom_highbd_dc_top_predictor_16x64_avx2);
    SET_AVX2_AVX512(svt_aom_highbd_dc_top_predictor_32x8, svt_aom_highbd_dc_top_predictor_32x8_c, svt_aom_highbd_dc_top_predictor_32x8_avx2, aom_highbd_dc_top_predictor_32x8_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_dc_top_predictor_32x16, svt_aom_highbd_dc_top_predictor_32x16_c, svt_aom_highbd_dc_top_predictor_32x16_avx2, aom_highbd_dc_top_predictor_32x16_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_dc_top_predictor_32x32, svt_aom_highbd_dc_top_predictor_32x32_c, svt_aom_highbd_dc_top_predictor_32x32_avx2, aom_highbd_dc_top_predictor_32x32_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_dc_top_predictor_32x64, svt_aom_highbd_dc_top_predictor_32x64_c, svt_aom_highbd_dc_top_predictor_32x64_avx2, aom_highbd_dc_top_predictor_32x64_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_dc_top_predictor_64x16, svt_aom_highbd_dc_top_predictor_64x16_c, svt_aom_highbd_dc_top_predictor_64x16_avx2, aom_highbd_dc_top_predictor_64x16_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_dc_top_predictor_64x32, svt_aom_highbd_dc_top_predictor_64x32_c, svt_aom_highbd_dc_top_predictor_64x32_avx2, aom_highbd_dc_top_predictor_64x32_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_dc_top_predictor_64x64, svt_aom_highbd_dc_top_predictor_64x64_c, svt_aom_highbd_dc_top_predictor_64x64_avx2, aom_highbd_dc_top_predictor_64x64_avx512);

    // svt_aom_highbd_h_predictor
    SET_SSE2(svt_aom_highbd_h_predictor_4x4, svt_aom_highbd_h_predictor_4x4_c, svt_aom_highbd_h_predictor_4x4_sse2);
    SET_SSE2(svt_aom_highbd_h_predictor_4x8, svt_aom_highbd_h_predictor_4x8_c, svt_aom_highbd_h_predictor_4x8_sse2);
    SET_SSE2(svt_aom_highbd_h_predictor_4x16, svt_aom_highbd_h_predictor_4x16_c, svt_aom_highbd_h_predictor_4x16_sse2);
    SET_SSE2(svt_aom_highbd_h_predictor_8x4, svt_aom_highbd_h_predictor_8x4_c, svt_aom_highbd_h_predictor_8x4_sse2);
    SET_SSE2(svt_aom_highbd_h_predictor_8x8, svt_aom_highbd_h_predictor_8x8_c, svt_aom_highbd_h_predictor_8x8_sse2);
    SET_SSE2(svt_aom_highbd_h_predictor_8x16, svt_aom_highbd_h_predictor_8x16_c, svt_aom_highbd_h_predictor_8x16_sse2);
    SET_SSE2(svt_aom_highbd_h_predictor_8x32, svt_aom_highbd_h_predictor_8x32_c, svt_aom_highbd_h_predictor_8x32_sse2);
    SET_AVX2(svt_aom_highbd_h_predictor_16x4, svt_aom_highbd_h_predictor_16x4_c, svt_aom_highbd_h_predictor_16x4_avx2);
    SET_SSE2(svt_aom_highbd_h_predictor_16x8, svt_aom_highbd_h_predictor_16x8_c, svt_aom_highbd_h_predictor_16x8_sse2);
    SET_SSE2(svt_aom_highbd_h_predictor_16x16, svt_aom_highbd_h_predictor_16x16_c, svt_aom_highbd_h_predictor_16x16_sse2);
    SET_SSE2(svt_aom_highbd_h_predictor_16x32, svt_aom_highbd_h_predictor_16x32_c, svt_aom_highbd_h_predictor_16x32_sse2);
    SET_AVX2(svt_aom_highbd_h_predictor_16x64, svt_aom_highbd_h_predictor_16x64_c, svt_aom_highbd_h_predictor_16x64_avx2);
    SET_AVX2_AVX512(svt_aom_highbd_h_predictor_32x8, svt_aom_highbd_h_predictor_32x8_c, svt_aom_highbd_h_predictor_32x8_avx2, aom_highbd_h_predictor_32x8_avx512);
    SET_SSE2_AVX512(svt_aom_highbd_h_predictor_32x16, svt_aom_highbd_h_predictor_32x16_c, svt_aom_highbd_h_predictor_32x16_sse2, aom_highbd_h_predictor_32x16_avx512);
    SET_SSE2_AVX512(svt_aom_highbd_h_predictor_32x32, svt_aom_highbd_h_predictor_32x32_c, svt_aom_highbd_h_predictor_32x32_sse2, aom_highbd_h_predictor_32x32_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_h_predictor_32x64, svt_aom_highbd_h_predictor_32x64_c, svt_aom_highbd_h_predictor_32x64_avx2, aom_highbd_h_predictor_32x64_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_h_predictor_64x16, svt_aom_highbd_h_predictor_64x16_c, svt_aom_highbd_h_predictor_64x16_avx2, aom_highbd_h_predictor_64x16_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_h_predictor_64x32, svt_aom_highbd_h_predictor_64x32_c, svt_aom_highbd_h_predictor_64x32_avx2, aom_highbd_h_predictor_64x32_avx512);
    SET_AVX2_AVX512(svt_aom_highbd_h_predictor_64x64, svt_aom_highbd_h_predictor_64x64_c, svt_aom_highbd_h_predictor_64x64_avx2, aom_highbd_h_predictor_64x64_avx512);
#endif
    SET_SSE2(svt_log2f, svt_aom_log2f_32, Log2f_ASM);
    SET_ONLY_C(svt_av1_copy_wxh_8bit, svt_av1_copy_wxh_8bit_c);
    SET_ONLY_C(svt_av1_copy_wxh_16bit, svt_av1_copy_wxh_16bit_c);
    SET_SSE2(svt_memcpy, svt_memcpy_c, svt_memcpy_intrin_sse);
    SET_ONLY_C(svt_memset, svt_memset_c);
    SET_AVX2(svt_aom_hadamard_32x32, svt_aom_hadamard_32x32_c, svt_aom_hadamard_32x32_avx2);
    SET_AVX2(svt_aom_hadamard_16x16, svt_aom_hadamard_16x16_c, svt_aom_hadamard_16x16_avx2);
    SET_SSE2(svt_aom_hadamard_8x8, svt_aom_hadamard_8x8_c, svt_aom_hadamard_8x8_sse2);
    SET_SSE2(svt_aom_hadamard_4x4, svt_aom_hadamard_4x4_c, svt_aom_hadamard_4x4_sse2);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_AVX2(svt_aom_highbd_hadamard_8x8, svt_aom_highbd_hadamard_8x8_c, svt_aom_highbd_hadamard_8x8_avx2);
#endif

#elif defined ARCH_AARCH64
    SET_NEON(svt_aom_blend_a64_mask, svt_aom_blend_a64_mask_c, svt_aom_blend_a64_mask_neon);
    SET_NEON(svt_aom_blend_a64_hmask, svt_aom_blend_a64_hmask_c, svt_aom_blend_a64_hmask_neon);
    SET_NEON(svt_aom_blend_a64_vmask, svt_aom_blend_a64_vmask_c, svt_aom_blend_a64_vmask_neon);
    SET_NEON(svt_aom_lowbd_blend_a64_d16_mask, svt_aom_lowbd_blend_a64_d16_mask_c, svt_aom_lowbd_blend_a64_d16_mask_neon);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_NEON(svt_aom_highbd_blend_a64_mask, svt_aom_highbd_blend_a64_mask_c, svt_aom_highbd_blend_a64_mask_neon);
    SET_NEON(svt_aom_highbd_blend_a64_vmask_16bit, svt_aom_highbd_blend_a64_vmask_16bit_c, svt_aom_highbd_blend_a64_vmask_16bit_neon);
    SET_NEON(svt_aom_highbd_blend_a64_hmask_16bit, svt_aom_highbd_blend_a64_hmask_16bit_c, svt_aom_highbd_blend_a64_hmask_16bit_neon);
    SET_NEON(svt_aom_highbd_blend_a64_d16_mask, svt_aom_highbd_blend_a64_d16_mask_c, svt_aom_highbd_blend_a64_d16_mask_neon);
#endif
    SET_NEON(svt_cfl_predict_lbd, svt_cfl_predict_lbd_c, svt_aom_cfl_predict_lbd_neon);
    SET_NEON(svt_cfl_predict_hbd, svt_cfl_predict_hbd_c, svt_cfl_predict_hbd_neon);
    SET_NEON_NEON_I8MM(svt_av1_filter_intra_predictor, svt_av1_filter_intra_predictor_c, svt_av1_filter_intra_predictor_neon, svt_av1_filter_intra_predictor_neon_i8mm);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_NEON(svt_av1_filter_intra_edge_high, svt_av1_filter_intra_edge_high_c, svt_av1_filter_intra_edge_high_neon);
#endif
    SET_NEON(svt_av1_filter_intra_edge, svt_av1_filter_intra_edge_c, svt_av1_filter_intra_edge_neon);
    SET_NEON(svt_av1_upsample_intra_edge, svt_av1_upsample_intra_edge_c, svt_av1_upsample_intra_edge_neon);
    SET_NEON(svt_av1_build_compound_diffwtd_mask_d16, svt_av1_build_compound_diffwtd_mask_d16_c, svt_av1_build_compound_diffwtd_mask_d16_neon);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_NEON(svt_av1_highbd_wiener_convolve_add_src, svt_av1_highbd_wiener_convolve_add_src_c, svt_av1_highbd_wiener_convolve_add_src_neon);
#endif
    SET_NEON(svt_apply_selfguided_restoration, svt_apply_selfguided_restoration_c, svt_aom_apply_selfguided_restoration_neon);
    SET_NEON(svt_av1_selfguided_restoration, svt_av1_selfguided_restoration_c, svt_av1_selfguided_restoration_neon);
    SET_NEON(svt_av1_inv_txfm2d_add_4x4, svt_av1_inv_txfm2d_add_4x4_c, svt_av1_inv_txfm2d_add_4x4_neon);
    SET_NEON(svt_av1_inv_txfm2d_add_4x8, svt_av1_inv_txfm2d_add_4x8_c, svt_av1_inv_txfm2d_add_4x8_neon);
    SET_NEON(svt_av1_inv_txfm2d_add_4x16, svt_av1_inv_txfm2d_add_4x16_c, svt_av1_inv_txfm2d_add_4x16_neon);
    SET_NEON(svt_av1_inv_txfm2d_add_8x4, svt_av1_inv_txfm2d_add_8x4_c, svt_av1_inv_txfm2d_add_8x4_neon);
    SET_NEON(svt_av1_inv_txfm2d_add_8x8, svt_av1_inv_txfm2d_add_8x8_c, svt_av1_inv_txfm2d_add_8x8_neon);
    SET_NEON(svt_av1_inv_txfm2d_add_8x16, svt_av1_inv_txfm2d_add_8x16_c, svt_av1_inv_txfm2d_add_8x16_neon);
    SET_NEON(svt_av1_inv_txfm2d_add_8x32, svt_av1_inv_txfm2d_add_8x32_c, svt_av1_inv_txfm2d_add_8x32_neon);
    SET_NEON(svt_av1_inv_txfm2d_add_16x4, svt_av1_inv_txfm2d_add_16x4_c, svt_av1_inv_txfm2d_add_16x4_neon);
    SET_NEON(svt_av1_inv_txfm2d_add_16x8, svt_av1_inv_txfm2d_add_16x8_c, svt_av1_inv_txfm2d_add_16x8_neon);
    SET_NEON(svt_av1_inv_txfm2d_add_16x16, svt_av1_inv_txfm2d_add_16x16_c, svt_av1_inv_txfm2d_add_16x16_neon);
    SET_NEON(svt_av1_inv_txfm2d_add_16x32, svt_av1_inv_txfm2d_add_16x32_c, svt_av1_inv_txfm2d_add_16x32_neon);
    SET_NEON(svt_av1_inv_txfm2d_add_16x64, svt_av1_inv_txfm2d_add_16x64_c, svt_av1_inv_txfm2d_add_16x64_neon);
    SET_NEON(svt_av1_inv_txfm2d_add_32x8, svt_av1_inv_txfm2d_add_32x8_c, svt_av1_inv_txfm2d_add_32x8_neon);
    SET_NEON(svt_av1_inv_txfm2d_add_32x16, svt_av1_inv_txfm2d_add_32x16_c, svt_av1_inv_txfm2d_add_32x16_neon);
    SET_NEON(svt_av1_inv_txfm2d_add_32x32, svt_av1_inv_txfm2d_add_32x32_c, svt_av1_inv_txfm2d_add_32x32_neon);
    SET_NEON(svt_av1_inv_txfm2d_add_32x64, svt_av1_inv_txfm2d_add_32x64_c, svt_av1_inv_txfm2d_add_32x64_neon);
    SET_NEON(svt_av1_inv_txfm2d_add_64x16, svt_av1_inv_txfm2d_add_64x16_c, svt_av1_inv_txfm2d_add_64x16_neon);
    SET_NEON(svt_av1_inv_txfm2d_add_64x32, svt_av1_inv_txfm2d_add_64x32_c, svt_av1_inv_txfm2d_add_64x32_neon);
    SET_NEON(svt_av1_inv_txfm2d_add_64x64, svt_av1_inv_txfm2d_add_64x64_c, svt_av1_inv_txfm2d_add_64x64_neon);
    SET_NEON(svt_av1_inv_txfm_add, svt_av1_inv_txfm_add_c, svt_dav1d_inv_txfm_add_neon);

    SET_NEON(svt_compressed_packmsb, svt_compressed_packmsb_c, svt_compressed_packmsb_neon);
    SET_NEON(svt_cfl_luma_subsampling_420_lbd, svt_cfl_luma_subsampling_420_lbd_c, svt_cfl_luma_subsampling_420_lbd_neon);
    SET_NEON(svt_cfl_luma_subsampling_420_hbd, svt_cfl_luma_subsampling_420_hbd_c, svt_cfl_luma_subsampling_420_hbd_neon);
    SET_ONLY_C(svt_convert_8bit_to_16bit, svt_convert_8bit_to_16bit_c);
    SET_ONLY_C(svt_convert_16bit_to_8bit, svt_convert_16bit_to_8bit_c);
    SET_NEON(svt_pack2d_16_bit_src_mul4, svt_enc_msb_pack2_d, svt_enc_msb_pack2d_neon);
    SET_NEON(svt_aom_un_pack2d_16_bit_src_mul4, svt_enc_msb_un_pack2_d, svt_enc_msb_un_pack2d_neon);
    SET_NEON(svt_full_distortion_kernel_cbf_zero32_bits, svt_full_distortion_kernel_cbf_zero32_bits_c, svt_full_distortion_kernel_cbf_zero32_bits_neon);
    SET_NEON(svt_full_distortion_kernel32_bits, svt_full_distortion_kernel32_bits_c, svt_full_distortion_kernel32_bits_neon);
    SET_NEON_NEON_DOTPROD(svt_spatial_full_distortion_kernel, svt_spatial_full_distortion_kernel_c, svt_spatial_full_distortion_kernel_neon, svt_spatial_full_distortion_kernel_neon_dotprod);
    SET_NEON_SVE(svt_full_distortion_kernel16_bits, svt_full_distortion_kernel16_bits_c, svt_full_distortion_kernel16_bits_neon, svt_full_distortion_kernel16_bits_sve);
    SET_NEON(svt_residual_kernel8bit, svt_residual_kernel8bit_c, svt_residual_kernel8bit_neon);
    SET_NEON(svt_residual_kernel16bit, svt_residual_kernel16bit_c, svt_residual_kernel16bit_neon);
    SET_NEON(svt_av1_wiener_convolve_add_src, svt_av1_wiener_convolve_add_src_c, svt_av1_wiener_convolve_add_src_neon);
    SET_NEON_NEON_DOTPROD_NEON_I8MM(svt_av1_convolve_2d_scale, svt_av1_convolve_2d_scale_c, svt_av1_convolve_2d_scale_neon, svt_av1_convolve_2d_scale_neon_dotprod, svt_av1_convolve_2d_scale_neon_i8mm);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_NEON_SVE2(svt_av1_highbd_convolve_2d_sr, svt_av1_highbd_convolve_2d_sr_c, svt_av1_highbd_convolve_2d_sr_neon, svt_av1_highbd_convolve_2d_sr_sve2);
    SET_NEON_SVE2(svt_av1_highbd_convolve_y_sr, svt_av1_highbd_convolve_y_sr_c, svt_av1_highbd_convolve_y_sr_neon, svt_av1_highbd_convolve_y_sr_sve2);
    SET_NEON(svt_av1_highbd_convolve_2d_scale, svt_av1_highbd_convolve_2d_scale_c, svt_av1_highbd_convolve_2d_scale_neon);
    SET_NEON(svt_av1_highbd_convolve_2d_copy_sr, svt_av1_highbd_convolve_2d_copy_sr_c, svt_av1_highbd_convolve_2d_copy_sr_neon);
    SET_NEON_SVE2(svt_av1_highbd_jnt_convolve_2d, svt_av1_highbd_jnt_convolve_2d_c, svt_av1_highbd_jnt_convolve_2d_neon, svt_av1_highbd_jnt_convolve_2d_sve2);
    SET_NEON(svt_av1_highbd_jnt_convolve_2d_copy, svt_av1_highbd_jnt_convolve_2d_copy_c, svt_av1_highbd_jnt_convolve_2d_copy_neon);
    SET_NEON_SVE2(svt_av1_highbd_jnt_convolve_y, svt_av1_highbd_jnt_convolve_y_c, svt_av1_highbd_jnt_convolve_y_neon, svt_av1_highbd_jnt_convolve_y_sve2);
    SET_NEON_SVE(svt_av1_highbd_jnt_convolve_x, svt_av1_highbd_jnt_convolve_x_c, svt_av1_highbd_jnt_convolve_x_neon, svt_av1_highbd_jnt_convolve_x_sve);
    SET_NEON_SVE(svt_av1_highbd_convolve_x_sr, svt_av1_highbd_convolve_x_sr_c, svt_av1_highbd_convolve_x_sr_neon, svt_av1_highbd_convolve_x_sr_sve);
#endif
    SET_NEON_NEON_DOTPROD_NEON_I8MM(svt_av1_convolve_2d_sr, svt_av1_convolve_2d_sr_c, svt_av1_convolve_2d_sr_neon, svt_av1_convolve_2d_sr_neon_dotprod, svt_av1_convolve_2d_sr_neon_i8mm);
    SET_NEON(svt_av1_convolve_2d_copy_sr, svt_av1_convolve_2d_copy_sr_c, svt_av1_convolve_2d_copy_sr_neon);
    SET_NEON_NEON_DOTPROD_NEON_I8MM(svt_av1_convolve_x_sr, svt_av1_convolve_x_sr_c, svt_av1_convolve_x_sr_neon, svt_av1_convolve_x_sr_neon_dotprod, svt_av1_convolve_x_sr_neon_i8mm);
    SET_NEON_NEON_DOTPROD_NEON_I8MM(svt_av1_convolve_y_sr, svt_av1_convolve_y_sr_c, svt_av1_convolve_y_sr_neon, svt_av1_convolve_y_sr_neon_dotprod, svt_av1_convolve_y_sr_neon_i8mm);
    SET_NEON_NEON_DOTPROD_NEON_I8MM(svt_av1_jnt_convolve_2d, svt_av1_jnt_convolve_2d_c, svt_av1_jnt_convolve_2d_neon, svt_av1_jnt_convolve_2d_neon_dotprod, svt_av1_jnt_convolve_2d_neon_i8mm);
    SET_NEON(svt_av1_jnt_convolve_2d_copy, svt_av1_jnt_convolve_2d_copy_c, svt_av1_jnt_convolve_2d_copy_neon);
    SET_NEON_NEON_DOTPROD_NEON_I8MM(svt_av1_jnt_convolve_x, svt_av1_jnt_convolve_x_c, svt_av1_jnt_convolve_x_neon, svt_av1_jnt_convolve_x_neon_dotprod, svt_av1_jnt_convolve_x_neon_i8mm);
    SET_NEON_NEON_DOTPROD_NEON_I8MM(svt_av1_jnt_convolve_y, svt_av1_jnt_convolve_y_c, svt_av1_jnt_convolve_y_neon, svt_av1_jnt_convolve_y_neon_dotprod, svt_av1_jnt_convolve_y_neon_i8mm);
    SET_NEON_NEON_DOTPROD_NEON_I8MM(svt_aom_convolve8_horiz, svt_aom_convolve8_horiz_c, svt_aom_convolve8_horiz_neon, svt_aom_convolve8_horiz_neon_dotprod, svt_aom_convolve8_horiz_neon_i8mm);
    SET_NEON_NEON_DOTPROD_NEON_I8MM(svt_aom_convolve8_vert, svt_aom_convolve8_vert_c, svt_aom_convolve8_vert_neon, svt_aom_convolve8_vert_neon_dotprod, svt_aom_convolve8_vert_neon_i8mm);
    SET_NEON(svt_av1_build_compound_diffwtd_mask, svt_av1_build_compound_diffwtd_mask_c, svt_av1_build_compound_diffwtd_mask_neon);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_NEON(svt_av1_build_compound_diffwtd_mask_highbd, svt_av1_build_compound_diffwtd_mask_highbd_c, svt_av1_build_compound_diffwtd_mask_highbd_neon);
#endif
    SET_NEON_SVE(svt_av1_wedge_sse_from_residuals, svt_av1_wedge_sse_from_residuals_c, svt_av1_wedge_sse_from_residuals_neon, svt_av1_wedge_sse_from_residuals_sve);
    SET_NEON(svt_aom_subtract_block, svt_aom_subtract_block_c, svt_aom_subtract_block_neon);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_NEON(svt_aom_highbd_subtract_block, svt_aom_highbd_subtract_block_c, svt_aom_highbd_subtract_block_neon);
    SET_NEON(svt_aom_highbd_smooth_v_predictor_4x4, svt_aom_highbd_smooth_v_predictor_4x4_c, svt_aom_highbd_smooth_v_predictor_4x4_neon);
    SET_NEON(svt_aom_highbd_smooth_v_predictor_4x8, svt_aom_highbd_smooth_v_predictor_4x8_c, svt_aom_highbd_smooth_v_predictor_4x8_neon);
    SET_NEON(svt_aom_highbd_smooth_v_predictor_4x16, svt_aom_highbd_smooth_v_predictor_4x16_c, svt_aom_highbd_smooth_v_predictor_4x16_neon);
    SET_NEON(svt_aom_highbd_smooth_v_predictor_8x4, svt_aom_highbd_smooth_v_predictor_8x4_c, svt_aom_highbd_smooth_v_predictor_8x4_neon);
    SET_NEON(svt_aom_highbd_smooth_v_predictor_8x8, svt_aom_highbd_smooth_v_predictor_8x8_c, svt_aom_highbd_smooth_v_predictor_8x8_neon);
    SET_NEON(svt_aom_highbd_smooth_v_predictor_8x16, svt_aom_highbd_smooth_v_predictor_8x16_c, svt_aom_highbd_smooth_v_predictor_8x16_neon);
    SET_NEON(svt_aom_highbd_smooth_v_predictor_8x32, svt_aom_highbd_smooth_v_predictor_8x32_c, svt_aom_highbd_smooth_v_predictor_8x32_neon);
    SET_NEON(svt_aom_highbd_smooth_v_predictor_16x4, svt_aom_highbd_smooth_v_predictor_16x4_c, svt_aom_highbd_smooth_v_predictor_16x4_neon);
    SET_NEON(svt_aom_highbd_smooth_v_predictor_16x8, svt_aom_highbd_smooth_v_predictor_16x8_c, svt_aom_highbd_smooth_v_predictor_16x8_neon);
    SET_NEON(svt_aom_highbd_smooth_v_predictor_16x16, svt_aom_highbd_smooth_v_predictor_16x16_c, svt_aom_highbd_smooth_v_predictor_16x16_neon);
    SET_NEON(svt_aom_highbd_smooth_v_predictor_16x32, svt_aom_highbd_smooth_v_predictor_16x32_c, svt_aom_highbd_smooth_v_predictor_16x32_neon);
    SET_NEON(svt_aom_highbd_smooth_v_predictor_16x64, svt_aom_highbd_smooth_v_predictor_16x64_c, svt_aom_highbd_smooth_v_predictor_16x64_neon);
    SET_NEON(svt_aom_highbd_smooth_v_predictor_32x8, svt_aom_highbd_smooth_v_predictor_32x8_c, svt_aom_highbd_smooth_v_predictor_32x8_neon);
    SET_NEON(svt_aom_highbd_smooth_v_predictor_32x16, svt_aom_highbd_smooth_v_predictor_32x16_c, svt_aom_highbd_smooth_v_predictor_32x16_neon);
    SET_NEON(svt_aom_highbd_smooth_v_predictor_32x32, svt_aom_highbd_smooth_v_predictor_32x32_c, svt_aom_highbd_smooth_v_predictor_32x32_neon);
    SET_NEON(svt_aom_highbd_smooth_v_predictor_32x64, svt_aom_highbd_smooth_v_predictor_32x64_c, svt_aom_highbd_smooth_v_predictor_32x64_neon);
    SET_NEON(svt_aom_highbd_smooth_v_predictor_64x16, svt_aom_highbd_smooth_v_predictor_64x16_c, svt_aom_highbd_smooth_v_predictor_64x16_neon);
    SET_NEON(svt_aom_highbd_smooth_v_predictor_64x32, svt_aom_highbd_smooth_v_predictor_64x32_c, svt_aom_highbd_smooth_v_predictor_64x32_neon);
    SET_NEON(svt_aom_highbd_smooth_v_predictor_64x64, svt_aom_highbd_smooth_v_predictor_64x64_c, svt_aom_highbd_smooth_v_predictor_64x64_neon);
#endif
    SET_NEON(svt_av1_dr_prediction_z1, svt_av1_dr_prediction_z1_c, svt_av1_dr_prediction_z1_neon);
    SET_NEON(svt_av1_dr_prediction_z2, svt_av1_dr_prediction_z2_c, svt_av1_dr_prediction_z2_neon);
    SET_NEON(svt_av1_dr_prediction_z3, svt_av1_dr_prediction_z3_c, svt_av1_dr_prediction_z3_neon);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_NEON(svt_av1_highbd_dr_prediction_z1, svt_av1_highbd_dr_prediction_z1_c, svt_av1_highbd_dr_prediction_z1_neon);
    SET_NEON(svt_av1_highbd_dr_prediction_z2, svt_av1_highbd_dr_prediction_z2_c, svt_av1_highbd_dr_prediction_z2_neon);
    SET_NEON(svt_av1_highbd_dr_prediction_z3, svt_av1_highbd_dr_prediction_z3_c, svt_av1_highbd_dr_prediction_z3_neon);
#endif
    SET_NEON(svt_aom_paeth_predictor_4x4, svt_aom_paeth_predictor_4x4_c, svt_aom_paeth_predictor_4x4_neon);
    SET_NEON(svt_aom_paeth_predictor_4x8, svt_aom_paeth_predictor_4x8_c, svt_aom_paeth_predictor_4x8_neon);
    SET_NEON(svt_aom_paeth_predictor_4x16, svt_aom_paeth_predictor_4x16_c, svt_aom_paeth_predictor_4x16_neon);
    SET_NEON(svt_aom_paeth_predictor_8x4, svt_aom_paeth_predictor_8x4_c, svt_aom_paeth_predictor_8x4_neon);
    SET_NEON(svt_aom_paeth_predictor_8x8, svt_aom_paeth_predictor_8x8_c, svt_aom_paeth_predictor_8x8_neon);
    SET_NEON(svt_aom_paeth_predictor_8x16, svt_aom_paeth_predictor_8x16_c, svt_aom_paeth_predictor_8x16_neon);
    SET_NEON(svt_aom_paeth_predictor_8x32, svt_aom_paeth_predictor_8x32_c, svt_aom_paeth_predictor_8x32_neon);
    SET_NEON(svt_aom_paeth_predictor_16x4, svt_aom_paeth_predictor_16x4_c,svt_aom_paeth_predictor_16x4_neon);
    SET_NEON(svt_aom_paeth_predictor_16x8, svt_aom_paeth_predictor_16x8_c, svt_aom_paeth_predictor_16x8_neon);
    SET_NEON(svt_aom_paeth_predictor_16x16, svt_aom_paeth_predictor_16x16_c, svt_aom_paeth_predictor_16x16_neon);
    SET_NEON(svt_aom_paeth_predictor_16x32, svt_aom_paeth_predictor_16x32_c, svt_aom_paeth_predictor_16x32_neon);
    SET_NEON(svt_aom_paeth_predictor_16x64, svt_aom_paeth_predictor_16x64_c, svt_aom_paeth_predictor_16x64_neon);
    SET_NEON(svt_aom_paeth_predictor_32x8, svt_aom_paeth_predictor_32x8_c, svt_aom_paeth_predictor_32x8_neon);
    SET_NEON(svt_aom_paeth_predictor_32x16, svt_aom_paeth_predictor_32x16_c, svt_aom_paeth_predictor_32x16_neon);
    SET_NEON(svt_aom_paeth_predictor_32x32, svt_aom_paeth_predictor_32x32_c, svt_aom_paeth_predictor_32x32_neon);
    SET_NEON(svt_aom_paeth_predictor_32x64, svt_aom_paeth_predictor_32x64_c, svt_aom_paeth_predictor_32x64_neon);
    SET_NEON(svt_aom_paeth_predictor_64x16, svt_aom_paeth_predictor_64x16_c, svt_aom_paeth_predictor_64x16_neon);
    SET_NEON(svt_aom_paeth_predictor_64x32, svt_aom_paeth_predictor_64x32_c, svt_aom_paeth_predictor_64x32_neon);
    SET_NEON(svt_aom_paeth_predictor_64x64, svt_aom_paeth_predictor_64x64_c, svt_aom_paeth_predictor_64x64_neon);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_NEON(svt_aom_highbd_paeth_predictor_4x4, svt_aom_highbd_paeth_predictor_4x4_c, svt_aom_highbd_paeth_predictor_4x4_neon);
    SET_NEON(svt_aom_highbd_paeth_predictor_4x8, svt_aom_highbd_paeth_predictor_4x8_c, svt_aom_highbd_paeth_predictor_4x8_neon);
    SET_NEON(svt_aom_highbd_paeth_predictor_4x16, svt_aom_highbd_paeth_predictor_4x16_c, svt_aom_highbd_paeth_predictor_4x16_neon);
    SET_NEON(svt_aom_highbd_paeth_predictor_8x4, svt_aom_highbd_paeth_predictor_8x4_c, svt_aom_highbd_paeth_predictor_8x4_neon);
    SET_NEON(svt_aom_highbd_paeth_predictor_8x8, svt_aom_highbd_paeth_predictor_8x8_c, svt_aom_highbd_paeth_predictor_8x8_neon);
    SET_NEON(svt_aom_highbd_paeth_predictor_8x16, svt_aom_highbd_paeth_predictor_8x16_c, svt_aom_highbd_paeth_predictor_8x16_neon);
    SET_NEON(svt_aom_highbd_paeth_predictor_8x32, svt_aom_highbd_paeth_predictor_8x32_c, svt_aom_highbd_paeth_predictor_8x32_neon);
    SET_NEON(svt_aom_highbd_paeth_predictor_16x4, svt_aom_highbd_paeth_predictor_16x4_c, svt_aom_highbd_paeth_predictor_16x4_neon);
    SET_NEON(svt_aom_highbd_paeth_predictor_16x8, svt_aom_highbd_paeth_predictor_16x8_c, svt_aom_highbd_paeth_predictor_16x8_neon);
    SET_NEON(svt_aom_highbd_paeth_predictor_16x16, svt_aom_highbd_paeth_predictor_16x16_c, svt_aom_highbd_paeth_predictor_16x16_neon);
    SET_NEON(svt_aom_highbd_paeth_predictor_16x32, svt_aom_highbd_paeth_predictor_16x32_c, svt_aom_highbd_paeth_predictor_16x32_neon);
    SET_NEON(svt_aom_highbd_paeth_predictor_16x64, svt_aom_highbd_paeth_predictor_16x64_c, svt_aom_highbd_paeth_predictor_16x64_neon);
    SET_NEON(svt_aom_highbd_paeth_predictor_32x8, svt_aom_highbd_paeth_predictor_32x8_c, svt_aom_highbd_paeth_predictor_32x8_neon);
    SET_NEON(svt_aom_highbd_paeth_predictor_32x16, svt_aom_highbd_paeth_predictor_32x16_c, svt_aom_highbd_paeth_predictor_32x16_neon);
    SET_NEON(svt_aom_highbd_paeth_predictor_32x32, svt_aom_highbd_paeth_predictor_32x32_c, svt_aom_highbd_paeth_predictor_32x32_neon);
    SET_NEON(svt_aom_highbd_paeth_predictor_32x64, svt_aom_highbd_paeth_predictor_32x64_c, svt_aom_highbd_paeth_predictor_32x64_neon);
    SET_NEON(svt_aom_highbd_paeth_predictor_64x16, svt_aom_highbd_paeth_predictor_64x16_c, svt_aom_highbd_paeth_predictor_64x16_neon);
    SET_NEON(svt_aom_highbd_paeth_predictor_64x32, svt_aom_highbd_paeth_predictor_64x32_c, svt_aom_highbd_paeth_predictor_64x32_neon);
    SET_NEON(svt_aom_highbd_paeth_predictor_64x64, svt_aom_highbd_paeth_predictor_64x64_c, svt_aom_highbd_paeth_predictor_64x64_neon);
#endif
    SET_NEON_SVE(svt_aom_sum_squares_i16, svt_aom_sum_squares_i16_c, svt_aom_sum_squares_i16_neon, svt_aom_sum_squares_i16_sve);

    SET_NEON(svt_aom_dc_predictor_4x4, svt_aom_dc_predictor_4x4_c, svt_aom_dc_predictor_4x4_neon );
    SET_NEON(svt_aom_dc_predictor_4x8, svt_aom_dc_predictor_4x8_c, svt_aom_dc_predictor_4x8_neon);
    SET_NEON(svt_aom_dc_predictor_4x16, svt_aom_dc_predictor_4x16_c, svt_aom_dc_predictor_4x16_neon);
    SET_NEON(svt_aom_dc_predictor_8x4, svt_aom_dc_predictor_8x4_c, svt_aom_dc_predictor_8x4_neon);
    SET_NEON(svt_aom_dc_predictor_8x8, svt_aom_dc_predictor_8x8_c, svt_aom_dc_predictor_8x8_neon);
    SET_NEON(svt_aom_dc_predictor_8x16, svt_aom_dc_predictor_8x16_c, svt_aom_dc_predictor_8x16_neon);
    SET_NEON(svt_aom_dc_predictor_8x32, svt_aom_dc_predictor_8x32_c, svt_aom_dc_predictor_8x32_neon);
    SET_NEON(svt_aom_dc_predictor_16x4, svt_aom_dc_predictor_16x4_c, svt_aom_dc_predictor_16x4_neon);
    SET_NEON(svt_aom_dc_predictor_16x8, svt_aom_dc_predictor_16x8_c, svt_aom_dc_predictor_16x8_neon);
    SET_NEON(svt_aom_dc_predictor_16x16, svt_aom_dc_predictor_16x16_c, svt_aom_dc_predictor_16x16_neon);
    SET_NEON(svt_aom_dc_predictor_16x32, svt_aom_dc_predictor_16x32_c, svt_aom_dc_predictor_16x32_neon);
    SET_NEON(svt_aom_dc_predictor_16x64, svt_aom_dc_predictor_16x64_c, svt_aom_dc_predictor_16x64_neon);
    SET_NEON(svt_aom_dc_predictor_32x8, svt_aom_dc_predictor_32x8_c, svt_aom_dc_predictor_32x8_neon);
    SET_NEON(svt_aom_dc_predictor_32x16, svt_aom_dc_predictor_32x16_c, svt_aom_dc_predictor_32x16_neon);
    SET_NEON(svt_aom_dc_predictor_32x32, svt_aom_dc_predictor_32x32_c,svt_aom_dc_predictor_32x32_neon);
    SET_NEON(svt_aom_dc_predictor_32x64, svt_aom_dc_predictor_32x64_c, svt_aom_dc_predictor_32x64_neon);
    SET_NEON(svt_aom_dc_predictor_64x16, svt_aom_dc_predictor_64x16_c, svt_aom_dc_predictor_64x16_neon);
    SET_NEON(svt_aom_dc_predictor_64x32, svt_aom_dc_predictor_64x32_c, svt_aom_dc_predictor_64x32_neon);
    SET_NEON(svt_aom_dc_predictor_64x64, svt_aom_dc_predictor_64x64_c,svt_aom_dc_predictor_64x64_neon);

    SET_NEON(svt_aom_dc_top_predictor_4x4, svt_aom_dc_top_predictor_4x4_c, svt_aom_dc_top_predictor_4x4_neon);
    SET_NEON(svt_aom_dc_top_predictor_4x8, svt_aom_dc_top_predictor_4x8_c, svt_aom_dc_top_predictor_4x8_neon);
    SET_NEON(svt_aom_dc_top_predictor_4x16, svt_aom_dc_top_predictor_4x16_c, svt_aom_dc_top_predictor_4x16_neon);
    SET_NEON(svt_aom_dc_top_predictor_8x4, svt_aom_dc_top_predictor_8x4_c, svt_aom_dc_top_predictor_8x4_neon);
    SET_NEON(svt_aom_dc_top_predictor_8x8, svt_aom_dc_top_predictor_8x8_c, svt_aom_dc_top_predictor_8x8_neon);
    SET_NEON(svt_aom_dc_top_predictor_8x16, svt_aom_dc_top_predictor_8x16_c, svt_aom_dc_top_predictor_8x16_neon);
    SET_NEON(svt_aom_dc_top_predictor_8x32, svt_aom_dc_top_predictor_8x32_c, svt_aom_dc_top_predictor_8x32_neon);
    SET_NEON(svt_aom_dc_top_predictor_16x4, svt_aom_dc_top_predictor_16x4_c, svt_aom_dc_top_predictor_16x4_neon);
    SET_NEON(svt_aom_dc_top_predictor_16x8, svt_aom_dc_top_predictor_16x8_c, svt_aom_dc_top_predictor_16x8_neon);
    SET_NEON(svt_aom_dc_top_predictor_16x16, svt_aom_dc_top_predictor_16x16_c, svt_aom_dc_top_predictor_16x16_neon);
    SET_NEON(svt_aom_dc_top_predictor_16x32, svt_aom_dc_top_predictor_16x32_c, svt_aom_dc_top_predictor_16x32_neon);
    SET_NEON(svt_aom_dc_top_predictor_16x64, svt_aom_dc_top_predictor_16x64_c, svt_aom_dc_top_predictor_16x64_neon);
    SET_NEON(svt_aom_dc_top_predictor_32x8, svt_aom_dc_top_predictor_32x8_c, svt_aom_dc_top_predictor_32x8_neon );
    SET_NEON(svt_aom_dc_top_predictor_32x16, svt_aom_dc_top_predictor_32x16_c, svt_aom_dc_top_predictor_32x16_neon);
    SET_NEON(svt_aom_dc_top_predictor_32x32, svt_aom_dc_top_predictor_32x32_c, svt_aom_dc_top_predictor_32x32_neon);
    SET_NEON(svt_aom_dc_top_predictor_32x64, svt_aom_dc_top_predictor_32x64_c, svt_aom_dc_top_predictor_32x64_neon);
    SET_NEON(svt_aom_dc_top_predictor_64x16, svt_aom_dc_top_predictor_64x16_c, svt_aom_dc_top_predictor_64x16_neon);
    SET_NEON(svt_aom_dc_top_predictor_64x32, svt_aom_dc_top_predictor_64x32_c, svt_aom_dc_top_predictor_64x32_neon);
    SET_NEON(svt_aom_dc_top_predictor_64x64, svt_aom_dc_top_predictor_64x64_c, svt_aom_dc_top_predictor_64x64_neon);

    SET_NEON(svt_aom_dc_left_predictor_4x4, svt_aom_dc_left_predictor_4x4_c, svt_aom_dc_left_predictor_4x4_neon);
    SET_NEON(svt_aom_dc_left_predictor_4x8, svt_aom_dc_left_predictor_4x8_c, svt_aom_dc_left_predictor_4x8_neon);
    SET_NEON(svt_aom_dc_left_predictor_4x16, svt_aom_dc_left_predictor_4x16_c, svt_aom_dc_left_predictor_4x16_neon);
    SET_NEON(svt_aom_dc_left_predictor_8x4, svt_aom_dc_left_predictor_8x4_c, svt_aom_dc_left_predictor_8x4_neon);
    SET_NEON(svt_aom_dc_left_predictor_8x8, svt_aom_dc_left_predictor_8x8_c, svt_aom_dc_left_predictor_8x8_neon);
    SET_NEON(svt_aom_dc_left_predictor_8x16, svt_aom_dc_left_predictor_8x16_c, svt_aom_dc_left_predictor_8x16_neon);
    SET_NEON(svt_aom_dc_left_predictor_8x32, svt_aom_dc_left_predictor_8x32_c, svt_aom_dc_left_predictor_8x32_neon);
    SET_NEON(svt_aom_dc_left_predictor_16x4, svt_aom_dc_left_predictor_16x4_c, svt_aom_dc_left_predictor_16x4_neon);
    SET_NEON(svt_aom_dc_left_predictor_16x8, svt_aom_dc_left_predictor_16x8_c, svt_aom_dc_left_predictor_16x8_neon);
    SET_NEON(svt_aom_dc_left_predictor_16x16, svt_aom_dc_left_predictor_16x16_c, svt_aom_dc_left_predictor_16x16_neon);
    SET_NEON(svt_aom_dc_left_predictor_16x32, svt_aom_dc_left_predictor_16x32_c, svt_aom_dc_left_predictor_16x32_neon);
    SET_NEON(svt_aom_dc_left_predictor_16x64, svt_aom_dc_left_predictor_16x64_c, svt_aom_dc_left_predictor_16x64_neon);
    SET_NEON(svt_aom_dc_left_predictor_32x8, svt_aom_dc_left_predictor_32x8_c, svt_aom_dc_left_predictor_32x8_neon);
    SET_NEON(svt_aom_dc_left_predictor_32x16, svt_aom_dc_left_predictor_32x16_c, svt_aom_dc_left_predictor_32x16_neon);
    SET_NEON(svt_aom_dc_left_predictor_32x32, svt_aom_dc_left_predictor_32x32_c, svt_aom_dc_left_predictor_32x32_neon);
    SET_NEON(svt_aom_dc_left_predictor_32x64, svt_aom_dc_left_predictor_32x64_c, svt_aom_dc_left_predictor_32x64_neon);
    SET_NEON(svt_aom_dc_left_predictor_64x16, svt_aom_dc_left_predictor_64x16_c, svt_aom_dc_left_predictor_64x16_neon);
    SET_NEON(svt_aom_dc_left_predictor_64x32, svt_aom_dc_left_predictor_64x32_c, svt_aom_dc_left_predictor_64x32_neon);
    SET_NEON(svt_aom_dc_left_predictor_64x64, svt_aom_dc_left_predictor_64x64_c, svt_aom_dc_left_predictor_64x64_neon);

    SET_NEON(svt_aom_dc_128_predictor_4x4, svt_aom_dc_128_predictor_4x4_c, svt_aom_dc_128_predictor_4x4_neon);
    SET_NEON(svt_aom_dc_128_predictor_4x8, svt_aom_dc_128_predictor_4x8_c, svt_aom_dc_128_predictor_4x8_neon);
    SET_NEON(svt_aom_dc_128_predictor_4x16, svt_aom_dc_128_predictor_4x16_c, svt_aom_dc_128_predictor_4x16_neon);
    SET_NEON(svt_aom_dc_128_predictor_8x4, svt_aom_dc_128_predictor_8x4_c, svt_aom_dc_128_predictor_8x4_neon);
    SET_NEON(svt_aom_dc_128_predictor_8x8, svt_aom_dc_128_predictor_8x8_c, svt_aom_dc_128_predictor_8x8_neon);
    SET_NEON(svt_aom_dc_128_predictor_8x16, svt_aom_dc_128_predictor_8x16_c, svt_aom_dc_128_predictor_8x16_neon);
    SET_NEON(svt_aom_dc_128_predictor_8x32, svt_aom_dc_128_predictor_8x32_c, svt_aom_dc_128_predictor_8x32_neon);
    SET_NEON(svt_aom_dc_128_predictor_16x4, svt_aom_dc_128_predictor_16x4_c, svt_aom_dc_128_predictor_16x4_neon);
    SET_NEON(svt_aom_dc_128_predictor_16x8, svt_aom_dc_128_predictor_16x8_c, svt_aom_dc_128_predictor_16x8_neon);
    SET_NEON(svt_aom_dc_128_predictor_16x16, svt_aom_dc_128_predictor_16x16_c,  svt_aom_dc_128_predictor_16x16_neon);
    SET_NEON(svt_aom_dc_128_predictor_16x32, svt_aom_dc_128_predictor_16x32_c, svt_aom_dc_128_predictor_16x32_neon);
    SET_NEON(svt_aom_dc_128_predictor_16x64, svt_aom_dc_128_predictor_16x64_c, svt_aom_dc_128_predictor_16x64_neon);
    SET_NEON(svt_aom_dc_128_predictor_32x8, svt_aom_dc_128_predictor_32x8_c, svt_aom_dc_128_predictor_32x8_neon);
    SET_NEON(svt_aom_dc_128_predictor_32x16, svt_aom_dc_128_predictor_32x16_c, svt_aom_dc_128_predictor_32x16_neon);
    SET_NEON(svt_aom_dc_128_predictor_32x32, svt_aom_dc_128_predictor_32x32_c, svt_aom_dc_128_predictor_32x32_neon);
    SET_NEON(svt_aom_dc_128_predictor_32x64, svt_aom_dc_128_predictor_32x64_c, svt_aom_dc_128_predictor_32x64_neon);
    SET_NEON(svt_aom_dc_128_predictor_64x16, svt_aom_dc_128_predictor_64x16_c, svt_aom_dc_128_predictor_64x16_neon);
    SET_NEON(svt_aom_dc_128_predictor_64x32, svt_aom_dc_128_predictor_64x32_c, svt_aom_dc_128_predictor_64x32_neon);
    SET_NEON(svt_aom_dc_128_predictor_64x64, svt_aom_dc_128_predictor_64x64_c, svt_aom_dc_128_predictor_64x64_neon);

    SET_NEON(svt_aom_smooth_h_predictor_4x4, svt_aom_smooth_h_predictor_4x4_c, svt_aom_smooth_h_predictor_4x4_neon);
    SET_NEON(svt_aom_smooth_h_predictor_4x8, svt_aom_smooth_h_predictor_4x8_c, svt_aom_smooth_h_predictor_4x8_neon);
    SET_NEON(svt_aom_smooth_h_predictor_4x16, svt_aom_smooth_h_predictor_4x16_c, svt_aom_smooth_h_predictor_4x16_neon);
    SET_NEON(svt_aom_smooth_h_predictor_8x4, svt_aom_smooth_h_predictor_8x4_c, svt_aom_smooth_h_predictor_8x4_neon);
    SET_NEON(svt_aom_smooth_h_predictor_8x8, svt_aom_smooth_h_predictor_8x8_c, svt_aom_smooth_h_predictor_8x8_neon);
    SET_NEON(svt_aom_smooth_h_predictor_8x16, svt_aom_smooth_h_predictor_8x16_c, svt_aom_smooth_h_predictor_8x16_neon);
    SET_NEON(svt_aom_smooth_h_predictor_8x32, svt_aom_smooth_h_predictor_8x32_c, svt_aom_smooth_h_predictor_8x32_neon);
    SET_NEON(svt_aom_smooth_h_predictor_16x4, svt_aom_smooth_h_predictor_16x4_c, svt_aom_smooth_h_predictor_16x4_neon);
    SET_NEON(svt_aom_smooth_h_predictor_16x8, svt_aom_smooth_h_predictor_16x8_c, svt_aom_smooth_h_predictor_16x8_neon);
    SET_NEON(svt_aom_smooth_h_predictor_16x16, svt_aom_smooth_h_predictor_16x16_c, svt_aom_smooth_h_predictor_16x16_neon);
    SET_NEON(svt_aom_smooth_h_predictor_16x32, svt_aom_smooth_h_predictor_16x32_c, svt_aom_smooth_h_predictor_16x32_neon);
    SET_NEON(svt_aom_smooth_h_predictor_16x64, svt_aom_smooth_h_predictor_16x64_c, svt_aom_smooth_h_predictor_16x64_neon);
    SET_NEON(svt_aom_smooth_h_predictor_32x8, svt_aom_smooth_h_predictor_32x8_c, svt_aom_smooth_h_predictor_32x8_neon);
    SET_NEON(svt_aom_smooth_h_predictor_32x16, svt_aom_smooth_h_predictor_32x16_c, svt_aom_smooth_h_predictor_32x16_neon);
    SET_NEON(svt_aom_smooth_h_predictor_32x32, svt_aom_smooth_h_predictor_32x32_c, svt_aom_smooth_h_predictor_32x32_neon);
    SET_NEON(svt_aom_smooth_h_predictor_32x64, svt_aom_smooth_h_predictor_32x64_c, svt_aom_smooth_h_predictor_32x64_neon);
    SET_NEON(svt_aom_smooth_h_predictor_64x16, svt_aom_smooth_h_predictor_64x16_c, svt_aom_smooth_h_predictor_64x16_neon);
    SET_NEON(svt_aom_smooth_h_predictor_64x32, svt_aom_smooth_h_predictor_64x32_c, svt_aom_smooth_h_predictor_64x32_neon);
    SET_NEON(svt_aom_smooth_h_predictor_64x64, svt_aom_smooth_h_predictor_64x64_c, svt_aom_smooth_h_predictor_64x64_neon);

    SET_NEON(svt_aom_smooth_v_predictor_4x4, svt_aom_smooth_v_predictor_4x4_c, svt_aom_smooth_v_predictor_4x4_neon);
    SET_NEON(svt_aom_smooth_v_predictor_4x8, svt_aom_smooth_v_predictor_4x8_c, svt_aom_smooth_v_predictor_4x8_neon);
    SET_NEON(svt_aom_smooth_v_predictor_4x16, svt_aom_smooth_v_predictor_4x16_c, svt_aom_smooth_v_predictor_4x16_neon);
    SET_NEON(svt_aom_smooth_v_predictor_8x4, svt_aom_smooth_v_predictor_8x4_c, svt_aom_smooth_v_predictor_8x4_neon);
    SET_NEON(svt_aom_smooth_v_predictor_8x8, svt_aom_smooth_v_predictor_8x8_c, svt_aom_smooth_v_predictor_8x8_neon);
    SET_NEON(svt_aom_smooth_v_predictor_8x16, svt_aom_smooth_v_predictor_8x16_c, svt_aom_smooth_v_predictor_8x16_neon);
    SET_NEON(svt_aom_smooth_v_predictor_8x32, svt_aom_smooth_v_predictor_8x32_c, svt_aom_smooth_v_predictor_8x32_neon);
    SET_NEON(svt_aom_smooth_v_predictor_16x4, svt_aom_smooth_v_predictor_16x4_c, svt_aom_smooth_v_predictor_16x4_neon);
    SET_NEON(svt_aom_smooth_v_predictor_16x8, svt_aom_smooth_v_predictor_16x8_c, svt_aom_smooth_v_predictor_16x8_neon);
    SET_NEON(svt_aom_smooth_v_predictor_16x16, svt_aom_smooth_v_predictor_16x16_c, svt_aom_smooth_v_predictor_16x16_neon);
    SET_NEON(svt_aom_smooth_v_predictor_16x32, svt_aom_smooth_v_predictor_16x32_c, svt_aom_smooth_v_predictor_16x32_neon);
    SET_NEON(svt_aom_smooth_v_predictor_16x64, svt_aom_smooth_v_predictor_16x64_c, svt_aom_smooth_v_predictor_16x64_neon);
    SET_NEON(svt_aom_smooth_v_predictor_32x8, svt_aom_smooth_v_predictor_32x8_c, svt_aom_smooth_v_predictor_32x8_neon);
    SET_NEON(svt_aom_smooth_v_predictor_32x16, svt_aom_smooth_v_predictor_32x16_c, svt_aom_smooth_v_predictor_32x16_neon);
    SET_NEON(svt_aom_smooth_v_predictor_32x32, svt_aom_smooth_v_predictor_32x32_c, svt_aom_smooth_v_predictor_32x32_neon);
    SET_NEON(svt_aom_smooth_v_predictor_32x64, svt_aom_smooth_v_predictor_32x64_c, svt_aom_smooth_v_predictor_32x64_neon);
    SET_NEON(svt_aom_smooth_v_predictor_64x16, svt_aom_smooth_v_predictor_64x16_c, svt_aom_smooth_v_predictor_64x16_neon);
    SET_NEON(svt_aom_smooth_v_predictor_64x32, svt_aom_smooth_v_predictor_64x32_c, svt_aom_smooth_v_predictor_64x32_neon);
    SET_NEON(svt_aom_smooth_v_predictor_64x64, svt_aom_smooth_v_predictor_64x64_c, svt_aom_smooth_v_predictor_64x64_neon);

    SET_NEON(svt_aom_smooth_predictor_4x4, svt_aom_smooth_predictor_4x4_c, svt_aom_smooth_predictor_4x4_neon);
    SET_NEON(svt_aom_smooth_predictor_4x8, svt_aom_smooth_predictor_4x8_c, svt_aom_smooth_predictor_4x8_neon);
    SET_NEON(svt_aom_smooth_predictor_4x16, svt_aom_smooth_predictor_4x16_c, svt_aom_smooth_predictor_4x16_neon);
    SET_NEON(svt_aom_smooth_predictor_8x4, svt_aom_smooth_predictor_8x4_c, svt_aom_smooth_predictor_8x4_neon);
    SET_NEON(svt_aom_smooth_predictor_8x8, svt_aom_smooth_predictor_8x8_c, svt_aom_smooth_predictor_8x8_neon);
    SET_NEON(svt_aom_smooth_predictor_8x16, svt_aom_smooth_predictor_8x16_c, svt_aom_smooth_predictor_8x16_neon);
    SET_NEON(svt_aom_smooth_predictor_8x32, svt_aom_smooth_predictor_8x32_c, svt_aom_smooth_predictor_8x32_neon);
    SET_NEON(svt_aom_smooth_predictor_16x4, svt_aom_smooth_predictor_16x4_c, svt_aom_smooth_predictor_16x4_neon);
    SET_NEON(svt_aom_smooth_predictor_16x8, svt_aom_smooth_predictor_16x8_c, svt_aom_smooth_predictor_16x8_neon);
    SET_NEON(svt_aom_smooth_predictor_16x16, svt_aom_smooth_predictor_16x16_c, svt_aom_smooth_predictor_16x16_neon);
    SET_NEON(svt_aom_smooth_predictor_16x32, svt_aom_smooth_predictor_16x32_c, svt_aom_smooth_predictor_16x32_neon);
    SET_NEON(svt_aom_smooth_predictor_16x64, svt_aom_smooth_predictor_16x64_c, svt_aom_smooth_predictor_16x64_neon);
    SET_NEON(svt_aom_smooth_predictor_32x8, svt_aom_smooth_predictor_32x8_c, svt_aom_smooth_predictor_32x8_neon);
    SET_NEON(svt_aom_smooth_predictor_32x16, svt_aom_smooth_predictor_32x16_c, svt_aom_smooth_predictor_32x16_neon);
    SET_NEON(svt_aom_smooth_predictor_32x32, svt_aom_smooth_predictor_32x32_c, svt_aom_smooth_predictor_32x32_neon);
    SET_NEON(svt_aom_smooth_predictor_32x64, svt_aom_smooth_predictor_32x64_c, svt_aom_smooth_predictor_32x64_neon);
    SET_NEON(svt_aom_smooth_predictor_64x16, svt_aom_smooth_predictor_64x16_c, svt_aom_smooth_predictor_64x16_neon);
    SET_NEON(svt_aom_smooth_predictor_64x32, svt_aom_smooth_predictor_64x32_c, svt_aom_smooth_predictor_64x32_neon);
    SET_NEON(svt_aom_smooth_predictor_64x64, svt_aom_smooth_predictor_64x64_c, svt_aom_smooth_predictor_64x64_neon);

    SET_NEON(svt_aom_v_predictor_4x4, svt_aom_v_predictor_4x4_c, svt_aom_v_predictor_4x4_neon);
    SET_NEON(svt_aom_v_predictor_4x8, svt_aom_v_predictor_4x8_c, svt_aom_v_predictor_4x8_neon);
    SET_NEON(svt_aom_v_predictor_4x16, svt_aom_v_predictor_4x16_c, svt_aom_v_predictor_4x16_neon);
    SET_NEON(svt_aom_v_predictor_8x4, svt_aom_v_predictor_8x4_c, svt_aom_v_predictor_8x4_neon);
    SET_NEON(svt_aom_v_predictor_8x8, svt_aom_v_predictor_8x8_c, svt_aom_v_predictor_8x8_neon);
    SET_NEON(svt_aom_v_predictor_8x16, svt_aom_v_predictor_8x16_c, svt_aom_v_predictor_8x16_neon);
    SET_NEON(svt_aom_v_predictor_8x32, svt_aom_v_predictor_8x32_c, svt_aom_v_predictor_8x32_neon);
    SET_NEON(svt_aom_v_predictor_16x4, svt_aom_v_predictor_16x4_c, svt_aom_v_predictor_16x4_neon);
    SET_NEON(svt_aom_v_predictor_16x8, svt_aom_v_predictor_16x8_c, svt_aom_v_predictor_16x8_neon);
    SET_NEON(svt_aom_v_predictor_16x16, svt_aom_v_predictor_16x16_c, svt_aom_v_predictor_16x16_neon);
    SET_NEON(svt_aom_v_predictor_16x32, svt_aom_v_predictor_16x32_c, svt_aom_v_predictor_16x32_neon);
    SET_NEON(svt_aom_v_predictor_16x64, svt_aom_v_predictor_16x64_c, svt_aom_v_predictor_16x64_neon);
    SET_NEON(svt_aom_v_predictor_32x8, svt_aom_v_predictor_32x8_c, svt_aom_v_predictor_32x8_neon);
    SET_NEON(svt_aom_v_predictor_32x16, svt_aom_v_predictor_32x16_c, svt_aom_v_predictor_32x16_neon);
    SET_NEON(svt_aom_v_predictor_32x32, svt_aom_v_predictor_32x32_c, svt_aom_v_predictor_32x32_neon);
    SET_NEON(svt_aom_v_predictor_32x64, svt_aom_v_predictor_32x64_c, svt_aom_v_predictor_32x64_neon);
    SET_NEON(svt_aom_v_predictor_64x16, svt_aom_v_predictor_64x16_c, svt_aom_v_predictor_64x16_neon);
    SET_NEON(svt_aom_v_predictor_64x32, svt_aom_v_predictor_64x32_c, svt_aom_v_predictor_64x32_neon);
    SET_NEON(svt_aom_v_predictor_64x64, svt_aom_v_predictor_64x64_c, svt_aom_v_predictor_64x64_neon);

    SET_NEON(svt_aom_h_predictor_4x4, svt_aom_h_predictor_4x4_c, svt_aom_h_predictor_4x4_neon);
    SET_NEON(svt_aom_h_predictor_4x8, svt_aom_h_predictor_4x8_c, svt_aom_h_predictor_4x8_neon);
    SET_NEON(svt_aom_h_predictor_4x16, svt_aom_h_predictor_4x16_c, svt_aom_h_predictor_4x16_neon);
    SET_NEON(svt_aom_h_predictor_8x4, svt_aom_h_predictor_8x4_c, svt_aom_h_predictor_8x4_neon);
    SET_NEON(svt_aom_h_predictor_8x8, svt_aom_h_predictor_8x8_c, svt_aom_h_predictor_8x8_neon);
    SET_NEON(svt_aom_h_predictor_8x16, svt_aom_h_predictor_8x16_c, svt_aom_h_predictor_8x16_neon);
    SET_NEON(svt_aom_h_predictor_8x32, svt_aom_h_predictor_8x32_c, svt_aom_h_predictor_8x32_neon);
    SET_NEON(svt_aom_h_predictor_16x4, svt_aom_h_predictor_16x4_c, svt_aom_h_predictor_16x4_neon);
    SET_NEON(svt_aom_h_predictor_16x8, svt_aom_h_predictor_16x8_c, svt_aom_h_predictor_16x8_neon);
    SET_NEON(svt_aom_h_predictor_16x16, svt_aom_h_predictor_16x16_c, svt_aom_h_predictor_16x16_neon);
    SET_NEON(svt_aom_h_predictor_16x32, svt_aom_h_predictor_16x32_c, svt_aom_h_predictor_16x32_neon);
    SET_NEON(svt_aom_h_predictor_16x64, svt_aom_h_predictor_16x64_c, svt_aom_h_predictor_16x64_neon);
    SET_NEON(svt_aom_h_predictor_32x8, svt_aom_h_predictor_32x8_c, svt_aom_h_predictor_32x8_neon);
    SET_NEON(svt_aom_h_predictor_32x16, svt_aom_h_predictor_32x16_c, svt_aom_h_predictor_32x16_neon);
    SET_NEON(svt_aom_h_predictor_32x32, svt_aom_h_predictor_32x32_c, svt_aom_h_predictor_32x32_neon);
    SET_NEON(svt_aom_h_predictor_32x64, svt_aom_h_predictor_32x64_c, svt_aom_h_predictor_32x64_neon);
    SET_NEON(svt_aom_h_predictor_64x16, svt_aom_h_predictor_64x16_c, svt_aom_h_predictor_64x16_neon);
    SET_NEON(svt_aom_h_predictor_64x32, svt_aom_h_predictor_64x32_c, svt_aom_h_predictor_64x32_neon);
    SET_NEON(svt_aom_h_predictor_64x64, svt_aom_h_predictor_64x64_c, svt_aom_h_predictor_64x64_neon);

    SET_NEON(svt_aom_cdef_find_dir, svt_aom_cdef_find_dir_c, svt_aom_cdef_find_dir_neon);
    SET_NEON(svt_aom_cdef_find_dir_dual, svt_aom_cdef_find_dir_dual_c, svt_aom_cdef_find_dir_dual_neon);
    SET_NEON(svt_cdef_filter_block, svt_cdef_filter_block_c, svt_cdef_filter_block_neon);

    SET_NEON(svt_aom_copy_rect8_8bit_to_16bit, svt_aom_copy_rect8_8bit_to_16bit_c, svt_aom_copy_rect8_8bit_to_16bit_neon);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_NEON_SVE(svt_av1_highbd_warp_affine, svt_av1_highbd_warp_affine_c, svt_av1_highbd_warp_affine_neon, svt_av1_highbd_warp_affine_sve);
#endif
    SET_NEON_NEON_I8MM_SVE(svt_av1_warp_affine, svt_av1_warp_affine_c, svt_av1_warp_affine_neon, svt_av1_warp_affine_neon_i8mm, svt_av1_warp_affine_sve);

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_NEON(svt_aom_highbd_lpf_horizontal_4, svt_aom_highbd_lpf_horizontal_4_c, svt_aom_highbd_lpf_horizontal_4_neon);
    SET_NEON(svt_aom_highbd_lpf_horizontal_6, svt_aom_highbd_lpf_horizontal_6_c, svt_aom_highbd_lpf_horizontal_6_neon);
    SET_NEON(svt_aom_highbd_lpf_horizontal_8, svt_aom_highbd_lpf_horizontal_8_c, svt_aom_highbd_lpf_horizontal_8_neon);
    SET_NEON(svt_aom_highbd_lpf_horizontal_14, svt_aom_highbd_lpf_horizontal_14_c, svt_aom_highbd_lpf_horizontal_14_neon);
    SET_NEON(svt_aom_highbd_lpf_vertical_4, svt_aom_highbd_lpf_vertical_4_c, svt_aom_highbd_lpf_vertical_4_neon);
    SET_NEON(svt_aom_highbd_lpf_vertical_6, svt_aom_highbd_lpf_vertical_6_c, svt_aom_highbd_lpf_vertical_6_neon);
    SET_NEON(svt_aom_highbd_lpf_vertical_8, svt_aom_highbd_lpf_vertical_8_c, svt_aom_highbd_lpf_vertical_8_neon);
    SET_NEON(svt_aom_highbd_lpf_vertical_14, svt_aom_highbd_lpf_vertical_14_c, svt_aom_highbd_lpf_vertical_14_neon);
#endif
    SET_NEON(svt_aom_lpf_horizontal_4, svt_aom_lpf_horizontal_4_c, svt_aom_lpf_horizontal_4_neon);
    SET_NEON(svt_aom_lpf_horizontal_6, svt_aom_lpf_horizontal_6_c, svt_aom_lpf_horizontal_6_neon);
    SET_NEON(svt_aom_lpf_horizontal_8, svt_aom_lpf_horizontal_8_c, svt_aom_lpf_horizontal_8_neon);
    SET_NEON(svt_aom_lpf_horizontal_14, svt_aom_lpf_horizontal_14_c, svt_aom_lpf_horizontal_14_neon);
    SET_NEON(svt_aom_lpf_vertical_4, svt_aom_lpf_vertical_4_c, svt_aom_lpf_vertical_4_neon);
    SET_NEON(svt_aom_lpf_vertical_6, svt_aom_lpf_vertical_6_c, svt_aom_lpf_vertical_6_neon);
    SET_NEON(svt_aom_lpf_vertical_8, svt_aom_lpf_vertical_8_c, svt_aom_lpf_vertical_8_neon);
    SET_NEON(svt_aom_lpf_vertical_14, svt_aom_lpf_vertical_14_c, svt_aom_lpf_vertical_14_neon);

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    // svt_aom_highbd_v_predictor
    SET_NEON(svt_aom_highbd_v_predictor_4x4, svt_aom_highbd_v_predictor_4x4_c, svt_aom_highbd_v_predictor_4x4_neon);
    SET_NEON(svt_aom_highbd_v_predictor_4x8, svt_aom_highbd_v_predictor_4x8_c, svt_aom_highbd_v_predictor_4x8_neon);
    SET_NEON(svt_aom_highbd_v_predictor_4x16, svt_aom_highbd_v_predictor_4x16_c, svt_aom_highbd_v_predictor_4x16_neon);
    SET_NEON(svt_aom_highbd_v_predictor_8x4, svt_aom_highbd_v_predictor_8x4_c, svt_aom_highbd_v_predictor_8x4_neon);
    SET_NEON(svt_aom_highbd_v_predictor_8x8, svt_aom_highbd_v_predictor_8x8_c, svt_aom_highbd_v_predictor_8x8_neon);
    SET_NEON(svt_aom_highbd_v_predictor_8x16, svt_aom_highbd_v_predictor_8x16_c, svt_aom_highbd_v_predictor_8x16_neon);
    SET_NEON(svt_aom_highbd_v_predictor_8x32, svt_aom_highbd_v_predictor_8x32_c, svt_aom_highbd_v_predictor_8x32_neon);
    SET_NEON(svt_aom_highbd_v_predictor_16x4, svt_aom_highbd_v_predictor_16x4_c, svt_aom_highbd_v_predictor_16x4_neon);
    SET_NEON(svt_aom_highbd_v_predictor_16x8, svt_aom_highbd_v_predictor_16x8_c, svt_aom_highbd_v_predictor_16x8_neon);
    SET_NEON(svt_aom_highbd_v_predictor_16x16, svt_aom_highbd_v_predictor_16x16_c, svt_aom_highbd_v_predictor_16x16_neon);
    SET_NEON(svt_aom_highbd_v_predictor_16x32, svt_aom_highbd_v_predictor_16x32_c, svt_aom_highbd_v_predictor_16x32_neon);
    SET_NEON(svt_aom_highbd_v_predictor_16x64, svt_aom_highbd_v_predictor_16x64_c, svt_aom_highbd_v_predictor_16x64_neon);
    SET_NEON(svt_aom_highbd_v_predictor_32x8, svt_aom_highbd_v_predictor_32x8_c, svt_aom_highbd_v_predictor_32x8_neon);
    SET_NEON(svt_aom_highbd_v_predictor_32x16, svt_aom_highbd_v_predictor_32x16_c, svt_aom_highbd_v_predictor_32x16_neon);
    SET_NEON(svt_aom_highbd_v_predictor_32x32, svt_aom_highbd_v_predictor_32x32_c, svt_aom_highbd_v_predictor_32x32_neon);
    SET_NEON(svt_aom_highbd_v_predictor_32x64, svt_aom_highbd_v_predictor_32x64_c, svt_aom_highbd_v_predictor_32x64_neon);
    SET_NEON(svt_aom_highbd_v_predictor_64x16, svt_aom_highbd_v_predictor_64x16_c, svt_aom_highbd_v_predictor_64x16_neon);
    SET_NEON(svt_aom_highbd_v_predictor_64x32, svt_aom_highbd_v_predictor_64x32_c, svt_aom_highbd_v_predictor_64x32_neon);
    SET_NEON(svt_aom_highbd_v_predictor_64x64, svt_aom_highbd_v_predictor_64x64_c, svt_aom_highbd_v_predictor_64x64_neon);

    //aom_highbd_smooth_predictor
    SET_NEON(svt_aom_highbd_smooth_predictor_4x4, svt_aom_highbd_smooth_predictor_4x4_c, svt_aom_highbd_smooth_predictor_4x4_neon);
    SET_NEON(svt_aom_highbd_smooth_predictor_4x8, svt_aom_highbd_smooth_predictor_4x8_c, svt_aom_highbd_smooth_predictor_4x8_neon);
    SET_NEON(svt_aom_highbd_smooth_predictor_4x16, svt_aom_highbd_smooth_predictor_4x16_c, svt_aom_highbd_smooth_predictor_4x16_neon);
    SET_NEON(svt_aom_highbd_smooth_predictor_8x4, svt_aom_highbd_smooth_predictor_8x4_c, svt_aom_highbd_smooth_predictor_8x4_neon);
    SET_NEON(svt_aom_highbd_smooth_predictor_8x8, svt_aom_highbd_smooth_predictor_8x8_c, svt_aom_highbd_smooth_predictor_8x8_neon);
    SET_NEON(svt_aom_highbd_smooth_predictor_8x16, svt_aom_highbd_smooth_predictor_8x16_c, svt_aom_highbd_smooth_predictor_8x16_neon);
    SET_NEON(svt_aom_highbd_smooth_predictor_8x32, svt_aom_highbd_smooth_predictor_8x32_c, svt_aom_highbd_smooth_predictor_8x32_neon);
    SET_NEON(svt_aom_highbd_smooth_predictor_16x4, svt_aom_highbd_smooth_predictor_16x4_c, svt_aom_highbd_smooth_predictor_16x4_neon);
    SET_NEON(svt_aom_highbd_smooth_predictor_16x8, svt_aom_highbd_smooth_predictor_16x8_c, svt_aom_highbd_smooth_predictor_16x8_neon);
    SET_NEON(svt_aom_highbd_smooth_predictor_16x16, svt_aom_highbd_smooth_predictor_16x16_c, svt_aom_highbd_smooth_predictor_16x16_neon);
    SET_NEON(svt_aom_highbd_smooth_predictor_16x32, svt_aom_highbd_smooth_predictor_16x32_c, svt_aom_highbd_smooth_predictor_16x32_neon);
    SET_NEON(svt_aom_highbd_smooth_predictor_16x64, svt_aom_highbd_smooth_predictor_16x64_c, svt_aom_highbd_smooth_predictor_16x64_neon);
    SET_NEON(svt_aom_highbd_smooth_predictor_32x8, svt_aom_highbd_smooth_predictor_32x8_c, svt_aom_highbd_smooth_predictor_32x8_neon);
    SET_NEON(svt_aom_highbd_smooth_predictor_32x16, svt_aom_highbd_smooth_predictor_32x16_c, svt_aom_highbd_smooth_predictor_32x16_neon);
    SET_NEON(svt_aom_highbd_smooth_predictor_32x32, svt_aom_highbd_smooth_predictor_32x32_c, svt_aom_highbd_smooth_predictor_32x32_neon);
    SET_NEON(svt_aom_highbd_smooth_predictor_32x64, svt_aom_highbd_smooth_predictor_32x64_c, svt_aom_highbd_smooth_predictor_32x64_neon);
    SET_NEON(svt_aom_highbd_smooth_predictor_64x16, svt_aom_highbd_smooth_predictor_64x16_c, svt_aom_highbd_smooth_predictor_64x16_neon);
    SET_NEON(svt_aom_highbd_smooth_predictor_64x32, svt_aom_highbd_smooth_predictor_64x32_c, svt_aom_highbd_smooth_predictor_64x32_neon);
    SET_NEON(svt_aom_highbd_smooth_predictor_64x64, svt_aom_highbd_smooth_predictor_64x64_c, svt_aom_highbd_smooth_predictor_64x64_neon);

    //aom_highbd_smooth_h_predictor
    SET_NEON(svt_aom_highbd_smooth_h_predictor_4x4, svt_aom_highbd_smooth_h_predictor_4x4_c, svt_aom_highbd_smooth_h_predictor_4x4_neon);
    SET_NEON(svt_aom_highbd_smooth_h_predictor_4x8, svt_aom_highbd_smooth_h_predictor_4x8_c, svt_aom_highbd_smooth_h_predictor_4x8_neon);
    SET_NEON(svt_aom_highbd_smooth_h_predictor_4x16, svt_aom_highbd_smooth_h_predictor_4x16_c, svt_aom_highbd_smooth_h_predictor_4x16_neon);
    SET_NEON(svt_aom_highbd_smooth_h_predictor_8x4, svt_aom_highbd_smooth_h_predictor_8x4_c, svt_aom_highbd_smooth_h_predictor_8x4_neon);
    SET_NEON(svt_aom_highbd_smooth_h_predictor_8x8, svt_aom_highbd_smooth_h_predictor_8x8_c, svt_aom_highbd_smooth_h_predictor_8x8_neon);
    SET_NEON(svt_aom_highbd_smooth_h_predictor_8x16, svt_aom_highbd_smooth_h_predictor_8x16_c, svt_aom_highbd_smooth_h_predictor_8x16_neon);
    SET_NEON(svt_aom_highbd_smooth_h_predictor_8x32, svt_aom_highbd_smooth_h_predictor_8x32_c, svt_aom_highbd_smooth_h_predictor_8x32_neon);
    SET_NEON(svt_aom_highbd_smooth_h_predictor_16x4, svt_aom_highbd_smooth_h_predictor_16x4_c, svt_aom_highbd_smooth_h_predictor_16x4_neon);
    SET_NEON(svt_aom_highbd_smooth_h_predictor_16x8, svt_aom_highbd_smooth_h_predictor_16x8_c, svt_aom_highbd_smooth_h_predictor_16x8_neon);
    SET_NEON(svt_aom_highbd_smooth_h_predictor_16x16, svt_aom_highbd_smooth_h_predictor_16x16_c, svt_aom_highbd_smooth_h_predictor_16x16_neon);
    SET_NEON(svt_aom_highbd_smooth_h_predictor_16x32, svt_aom_highbd_smooth_h_predictor_16x32_c, svt_aom_highbd_smooth_h_predictor_16x32_neon);
    SET_NEON(svt_aom_highbd_smooth_h_predictor_16x64, svt_aom_highbd_smooth_h_predictor_16x64_c, svt_aom_highbd_smooth_h_predictor_16x64_neon);
    SET_NEON(svt_aom_highbd_smooth_h_predictor_32x8, svt_aom_highbd_smooth_h_predictor_32x8_c, svt_aom_highbd_smooth_h_predictor_32x8_neon);
    SET_NEON(svt_aom_highbd_smooth_h_predictor_32x16, svt_aom_highbd_smooth_h_predictor_32x16_c, svt_aom_highbd_smooth_h_predictor_32x16_neon);
    SET_NEON(svt_aom_highbd_smooth_h_predictor_32x32, svt_aom_highbd_smooth_h_predictor_32x32_c, svt_aom_highbd_smooth_h_predictor_32x32_neon);
    SET_NEON(svt_aom_highbd_smooth_h_predictor_32x64, svt_aom_highbd_smooth_h_predictor_32x64_c, svt_aom_highbd_smooth_h_predictor_32x64_neon);
    SET_NEON(svt_aom_highbd_smooth_h_predictor_64x16, svt_aom_highbd_smooth_h_predictor_64x16_c, svt_aom_highbd_smooth_h_predictor_64x16_neon);
    SET_NEON(svt_aom_highbd_smooth_h_predictor_64x32, svt_aom_highbd_smooth_h_predictor_64x32_c, svt_aom_highbd_smooth_h_predictor_64x32_neon);
    SET_NEON(svt_aom_highbd_smooth_h_predictor_64x64, svt_aom_highbd_smooth_h_predictor_64x64_c, svt_aom_highbd_smooth_h_predictor_64x64_neon);

    //aom_highbd_dc_128_predictor
    SET_NEON(svt_aom_highbd_dc_128_predictor_4x4, svt_aom_highbd_dc_128_predictor_4x4_c, svt_aom_highbd_dc_128_predictor_4x4_neon);
    SET_NEON(svt_aom_highbd_dc_128_predictor_4x8, svt_aom_highbd_dc_128_predictor_4x8_c, svt_aom_highbd_dc_128_predictor_4x8_neon);
    SET_NEON(svt_aom_highbd_dc_128_predictor_4x16, svt_aom_highbd_dc_128_predictor_4x16_c, svt_aom_highbd_dc_128_predictor_4x16_neon);
    SET_NEON(svt_aom_highbd_dc_128_predictor_8x4, svt_aom_highbd_dc_128_predictor_8x4_c, svt_aom_highbd_dc_128_predictor_8x4_neon);
    SET_NEON(svt_aom_highbd_dc_128_predictor_8x8, svt_aom_highbd_dc_128_predictor_8x8_c, svt_aom_highbd_dc_128_predictor_8x8_neon);
    SET_NEON(svt_aom_highbd_dc_128_predictor_8x16, svt_aom_highbd_dc_128_predictor_8x16_c, svt_aom_highbd_dc_128_predictor_8x16_neon);
    SET_NEON(svt_aom_highbd_dc_128_predictor_8x32, svt_aom_highbd_dc_128_predictor_8x32_c, svt_aom_highbd_dc_128_predictor_8x32_neon);
    SET_NEON(svt_aom_highbd_dc_128_predictor_16x4, svt_aom_highbd_dc_128_predictor_16x4_c, svt_aom_highbd_dc_128_predictor_16x4_neon);
    SET_NEON(svt_aom_highbd_dc_128_predictor_16x8, svt_aom_highbd_dc_128_predictor_16x8_c, svt_aom_highbd_dc_128_predictor_16x8_neon);
    SET_NEON(svt_aom_highbd_dc_128_predictor_16x16, svt_aom_highbd_dc_128_predictor_16x16_c, svt_aom_highbd_dc_128_predictor_16x16_neon);
    SET_NEON(svt_aom_highbd_dc_128_predictor_16x32, svt_aom_highbd_dc_128_predictor_16x32_c, svt_aom_highbd_dc_128_predictor_16x32_neon);
    SET_NEON(svt_aom_highbd_dc_128_predictor_16x64, svt_aom_highbd_dc_128_predictor_16x64_c, svt_aom_highbd_dc_128_predictor_16x64_neon);
    SET_NEON(svt_aom_highbd_dc_128_predictor_32x8, svt_aom_highbd_dc_128_predictor_32x8_c, svt_aom_highbd_dc_128_predictor_32x8_neon);
    SET_NEON(svt_aom_highbd_dc_128_predictor_32x16, svt_aom_highbd_dc_128_predictor_32x16_c, svt_aom_highbd_dc_128_predictor_32x16_neon);
    SET_NEON(svt_aom_highbd_dc_128_predictor_32x32, svt_aom_highbd_dc_128_predictor_32x32_c, svt_aom_highbd_dc_128_predictor_32x32_neon);
    SET_NEON(svt_aom_highbd_dc_128_predictor_32x64, svt_aom_highbd_dc_128_predictor_32x64_c, svt_aom_highbd_dc_128_predictor_32x64_neon);
    SET_NEON(svt_aom_highbd_dc_128_predictor_64x16, svt_aom_highbd_dc_128_predictor_64x16_c, svt_aom_highbd_dc_128_predictor_64x16_neon);
    SET_NEON(svt_aom_highbd_dc_128_predictor_64x32, svt_aom_highbd_dc_128_predictor_64x32_c, svt_aom_highbd_dc_128_predictor_64x32_neon);
    SET_NEON(svt_aom_highbd_dc_128_predictor_64x64, svt_aom_highbd_dc_128_predictor_64x64_c, svt_aom_highbd_dc_128_predictor_64x64_neon);

    //aom_highbd_dc_left_predictor
    SET_NEON(svt_aom_highbd_dc_left_predictor_4x4, svt_aom_highbd_dc_left_predictor_4x4_c, svt_aom_highbd_dc_left_predictor_4x4_neon);
    SET_NEON(svt_aom_highbd_dc_left_predictor_4x8, svt_aom_highbd_dc_left_predictor_4x8_c, svt_aom_highbd_dc_left_predictor_4x8_neon);
    SET_NEON(svt_aom_highbd_dc_left_predictor_4x16, svt_aom_highbd_dc_left_predictor_4x16_c, svt_aom_highbd_dc_left_predictor_4x16_neon);
    SET_NEON(svt_aom_highbd_dc_left_predictor_8x4, svt_aom_highbd_dc_left_predictor_8x4_c, svt_aom_highbd_dc_left_predictor_8x4_neon);
    SET_NEON(svt_aom_highbd_dc_left_predictor_8x8, svt_aom_highbd_dc_left_predictor_8x8_c, svt_aom_highbd_dc_left_predictor_8x8_neon);
    SET_NEON(svt_aom_highbd_dc_left_predictor_8x16, svt_aom_highbd_dc_left_predictor_8x16_c, svt_aom_highbd_dc_left_predictor_8x16_neon);
    SET_NEON(svt_aom_highbd_dc_left_predictor_8x32, svt_aom_highbd_dc_left_predictor_8x32_c, svt_aom_highbd_dc_left_predictor_8x32_neon);
    SET_NEON(svt_aom_highbd_dc_left_predictor_16x4, svt_aom_highbd_dc_left_predictor_16x4_c, svt_aom_highbd_dc_left_predictor_16x4_neon);
    SET_NEON(svt_aom_highbd_dc_left_predictor_16x8, svt_aom_highbd_dc_left_predictor_16x8_c, svt_aom_highbd_dc_left_predictor_16x8_neon);
    SET_NEON(svt_aom_highbd_dc_left_predictor_16x16, svt_aom_highbd_dc_left_predictor_16x16_c, svt_aom_highbd_dc_left_predictor_16x16_neon);
    SET_NEON(svt_aom_highbd_dc_left_predictor_16x32, svt_aom_highbd_dc_left_predictor_16x32_c, svt_aom_highbd_dc_left_predictor_16x32_neon);
    SET_NEON(svt_aom_highbd_dc_left_predictor_16x64, svt_aom_highbd_dc_left_predictor_16x64_c, svt_aom_highbd_dc_left_predictor_16x64_neon);
    SET_NEON(svt_aom_highbd_dc_left_predictor_32x8, svt_aom_highbd_dc_left_predictor_32x8_c, svt_aom_highbd_dc_left_predictor_32x8_neon);
    SET_NEON(svt_aom_highbd_dc_left_predictor_32x16, svt_aom_highbd_dc_left_predictor_32x16_c, svt_aom_highbd_dc_left_predictor_32x16_neon);
    SET_NEON(svt_aom_highbd_dc_left_predictor_32x32, svt_aom_highbd_dc_left_predictor_32x32_c, svt_aom_highbd_dc_left_predictor_32x32_neon);
    SET_NEON(svt_aom_highbd_dc_left_predictor_32x64, svt_aom_highbd_dc_left_predictor_32x64_c, svt_aom_highbd_dc_left_predictor_32x64_neon);
    SET_NEON(svt_aom_highbd_dc_left_predictor_64x16, svt_aom_highbd_dc_left_predictor_64x16_c, svt_aom_highbd_dc_left_predictor_64x16_neon);
    SET_NEON(svt_aom_highbd_dc_left_predictor_64x32, svt_aom_highbd_dc_left_predictor_64x32_c, svt_aom_highbd_dc_left_predictor_64x32_neon);
    SET_NEON(svt_aom_highbd_dc_left_predictor_64x64, svt_aom_highbd_dc_left_predictor_64x64_c, svt_aom_highbd_dc_left_predictor_64x64_neon);

    SET_NEON(svt_aom_highbd_dc_predictor_4x4, svt_aom_highbd_dc_predictor_4x4_c, svt_aom_highbd_dc_predictor_4x4_neon);
    SET_NEON(svt_aom_highbd_dc_predictor_4x8, svt_aom_highbd_dc_predictor_4x8_c, svt_aom_highbd_dc_predictor_4x8_neon);
    SET_NEON(svt_aom_highbd_dc_predictor_4x16, svt_aom_highbd_dc_predictor_4x16_c, svt_aom_highbd_dc_predictor_4x16_neon);
    SET_NEON(svt_aom_highbd_dc_predictor_8x4, svt_aom_highbd_dc_predictor_8x4_c, svt_aom_highbd_dc_predictor_8x4_neon);
    SET_NEON(svt_aom_highbd_dc_predictor_8x8, svt_aom_highbd_dc_predictor_8x8_c, svt_aom_highbd_dc_predictor_8x8_neon);
    SET_NEON(svt_aom_highbd_dc_predictor_8x16, svt_aom_highbd_dc_predictor_8x16_c, svt_aom_highbd_dc_predictor_8x16_neon);
    SET_NEON(svt_aom_highbd_dc_predictor_8x32, svt_aom_highbd_dc_predictor_8x32_c, svt_aom_highbd_dc_predictor_8x32_neon);
    SET_NEON(svt_aom_highbd_dc_predictor_16x4, svt_aom_highbd_dc_predictor_16x4_c, svt_aom_highbd_dc_predictor_16x4_neon);
    SET_NEON(svt_aom_highbd_dc_predictor_16x8, svt_aom_highbd_dc_predictor_16x8_c, svt_aom_highbd_dc_predictor_16x8_neon);
    SET_NEON(svt_aom_highbd_dc_predictor_16x16, svt_aom_highbd_dc_predictor_16x16_c, svt_aom_highbd_dc_predictor_16x16_neon);
    SET_NEON(svt_aom_highbd_dc_predictor_16x32, svt_aom_highbd_dc_predictor_16x32_c, svt_aom_highbd_dc_predictor_16x32_neon);
    SET_NEON(svt_aom_highbd_dc_predictor_16x64, svt_aom_highbd_dc_predictor_16x64_c, svt_aom_highbd_dc_predictor_16x64_neon);
    SET_NEON(svt_aom_highbd_dc_predictor_32x8, svt_aom_highbd_dc_predictor_32x8_c, svt_aom_highbd_dc_predictor_32x8_neon);
    SET_NEON(svt_aom_highbd_dc_predictor_32x16, svt_aom_highbd_dc_predictor_32x16_c, svt_aom_highbd_dc_predictor_32x16_neon);
    SET_NEON(svt_aom_highbd_dc_predictor_32x32, svt_aom_highbd_dc_predictor_32x32_c, svt_aom_highbd_dc_predictor_32x32_neon);
    SET_NEON(svt_aom_highbd_dc_predictor_32x64, svt_aom_highbd_dc_predictor_32x64_c, svt_aom_highbd_dc_predictor_32x64_neon);
    SET_NEON(svt_aom_highbd_dc_predictor_64x16, svt_aom_highbd_dc_predictor_64x16_c, svt_aom_highbd_dc_predictor_64x16_neon);
    SET_NEON(svt_aom_highbd_dc_predictor_64x32, svt_aom_highbd_dc_predictor_64x32_c, svt_aom_highbd_dc_predictor_64x32_neon);
    SET_NEON(svt_aom_highbd_dc_predictor_64x64, svt_aom_highbd_dc_predictor_64x64_c, svt_aom_highbd_dc_predictor_64x64_neon);

    //aom_highbd_dc_top_predictor
    SET_NEON(svt_aom_highbd_dc_top_predictor_4x4, svt_aom_highbd_dc_top_predictor_4x4_c, svt_aom_highbd_dc_top_predictor_4x4_neon);
    SET_NEON(svt_aom_highbd_dc_top_predictor_4x8, svt_aom_highbd_dc_top_predictor_4x8_c, svt_aom_highbd_dc_top_predictor_4x8_neon);
    SET_NEON(svt_aom_highbd_dc_top_predictor_4x16, svt_aom_highbd_dc_top_predictor_4x16_c, svt_aom_highbd_dc_top_predictor_4x16_neon);
    SET_NEON(svt_aom_highbd_dc_top_predictor_8x4, svt_aom_highbd_dc_top_predictor_8x4_c, svt_aom_highbd_dc_top_predictor_8x4_neon);
    SET_NEON(svt_aom_highbd_dc_top_predictor_8x8, svt_aom_highbd_dc_top_predictor_8x8_c, svt_aom_highbd_dc_top_predictor_8x8_neon);
    SET_NEON(svt_aom_highbd_dc_top_predictor_8x16, svt_aom_highbd_dc_top_predictor_8x16_c, svt_aom_highbd_dc_top_predictor_8x16_neon);
    SET_NEON(svt_aom_highbd_dc_top_predictor_8x32, svt_aom_highbd_dc_top_predictor_8x32_c, svt_aom_highbd_dc_top_predictor_8x32_neon);
    SET_NEON(svt_aom_highbd_dc_top_predictor_16x4, svt_aom_highbd_dc_top_predictor_16x4_c, svt_aom_highbd_dc_top_predictor_16x4_neon);
    SET_NEON(svt_aom_highbd_dc_top_predictor_16x8, svt_aom_highbd_dc_top_predictor_16x8_c, svt_aom_highbd_dc_top_predictor_16x8_neon);
    SET_NEON(svt_aom_highbd_dc_top_predictor_16x16, svt_aom_highbd_dc_top_predictor_16x16_c, svt_aom_highbd_dc_top_predictor_16x16_neon);
    SET_NEON(svt_aom_highbd_dc_top_predictor_16x32, svt_aom_highbd_dc_top_predictor_16x32_c, svt_aom_highbd_dc_top_predictor_16x32_neon);
    SET_NEON(svt_aom_highbd_dc_top_predictor_16x64, svt_aom_highbd_dc_top_predictor_16x64_c, svt_aom_highbd_dc_top_predictor_16x64_neon);
    SET_NEON(svt_aom_highbd_dc_top_predictor_32x8, svt_aom_highbd_dc_top_predictor_32x8_c, svt_aom_highbd_dc_top_predictor_32x8_neon);
    SET_NEON(svt_aom_highbd_dc_top_predictor_32x16, svt_aom_highbd_dc_top_predictor_32x16_c, svt_aom_highbd_dc_top_predictor_32x16_neon);
    SET_NEON(svt_aom_highbd_dc_top_predictor_32x32, svt_aom_highbd_dc_top_predictor_32x32_c, svt_aom_highbd_dc_top_predictor_32x32_neon);
    SET_NEON(svt_aom_highbd_dc_top_predictor_32x64, svt_aom_highbd_dc_top_predictor_32x64_c, svt_aom_highbd_dc_top_predictor_32x64_neon);
    SET_NEON(svt_aom_highbd_dc_top_predictor_64x16, svt_aom_highbd_dc_top_predictor_64x16_c, svt_aom_highbd_dc_top_predictor_64x16_neon);
    SET_NEON(svt_aom_highbd_dc_top_predictor_64x32, svt_aom_highbd_dc_top_predictor_64x32_c, svt_aom_highbd_dc_top_predictor_64x32_neon);
    SET_NEON(svt_aom_highbd_dc_top_predictor_64x64, svt_aom_highbd_dc_top_predictor_64x64_c, svt_aom_highbd_dc_top_predictor_64x64_neon);

    // svt_aom_highbd_h_predictor
    SET_NEON(svt_aom_highbd_h_predictor_4x4, svt_aom_highbd_h_predictor_4x4_c, svt_aom_highbd_h_predictor_4x4_neon);
    SET_NEON(svt_aom_highbd_h_predictor_4x8, svt_aom_highbd_h_predictor_4x8_c, svt_aom_highbd_h_predictor_4x8_neon);
    SET_NEON(svt_aom_highbd_h_predictor_4x16, svt_aom_highbd_h_predictor_4x16_c, svt_aom_highbd_h_predictor_4x16_neon);
    SET_NEON(svt_aom_highbd_h_predictor_8x4, svt_aom_highbd_h_predictor_8x4_c, svt_aom_highbd_h_predictor_8x4_neon);
    SET_NEON(svt_aom_highbd_h_predictor_8x8, svt_aom_highbd_h_predictor_8x8_c, svt_aom_highbd_h_predictor_8x8_neon);
    SET_NEON(svt_aom_highbd_h_predictor_8x16, svt_aom_highbd_h_predictor_8x16_c, svt_aom_highbd_h_predictor_8x16_neon);
    SET_NEON(svt_aom_highbd_h_predictor_8x32, svt_aom_highbd_h_predictor_8x32_c, svt_aom_highbd_h_predictor_8x32_neon);
    SET_NEON(svt_aom_highbd_h_predictor_16x4, svt_aom_highbd_h_predictor_16x4_c, svt_aom_highbd_h_predictor_16x4_neon);
    SET_NEON(svt_aom_highbd_h_predictor_16x8, svt_aom_highbd_h_predictor_16x8_c, svt_aom_highbd_h_predictor_16x8_neon);
    SET_NEON(svt_aom_highbd_h_predictor_16x16, svt_aom_highbd_h_predictor_16x16_c, svt_aom_highbd_h_predictor_16x16_neon);
    SET_NEON(svt_aom_highbd_h_predictor_16x32, svt_aom_highbd_h_predictor_16x32_c, svt_aom_highbd_h_predictor_16x32_neon);
    SET_NEON(svt_aom_highbd_h_predictor_16x64, svt_aom_highbd_h_predictor_16x64_c, svt_aom_highbd_h_predictor_16x64_neon);
    SET_NEON(svt_aom_highbd_h_predictor_32x8, svt_aom_highbd_h_predictor_32x8_c, svt_aom_highbd_h_predictor_32x8_neon);
    SET_NEON(svt_aom_highbd_h_predictor_32x16, svt_aom_highbd_h_predictor_32x16_c, svt_aom_highbd_h_predictor_32x16_neon);
    SET_NEON(svt_aom_highbd_h_predictor_32x32, svt_aom_highbd_h_predictor_32x32_c, svt_aom_highbd_h_predictor_32x32_neon);
    SET_NEON(svt_aom_highbd_h_predictor_32x64, svt_aom_highbd_h_predictor_32x64_c, svt_aom_highbd_h_predictor_32x64_neon);
    SET_NEON(svt_aom_highbd_h_predictor_64x16, svt_aom_highbd_h_predictor_64x16_c, svt_aom_highbd_h_predictor_64x16_neon);
    SET_NEON(svt_aom_highbd_h_predictor_64x32, svt_aom_highbd_h_predictor_64x32_c, svt_aom_highbd_h_predictor_64x32_neon);
    SET_NEON(svt_aom_highbd_h_predictor_64x64, svt_aom_highbd_h_predictor_64x64_c, svt_aom_highbd_h_predictor_64x64_c);
#endif

    SET_ONLY_C(svt_log2f, svt_aom_log2f_32);
    SET_NEON(svt_av1_copy_wxh_8bit, svt_av1_copy_wxh_8bit_c, svt_av1_copy_wxh_8bit_neon);
    SET_NEON(svt_av1_copy_wxh_16bit, svt_av1_copy_wxh_16bit_c, svt_av1_copy_wxh_16bit_neon);
    SET_NEON(svt_memcpy, svt_memcpy_c, svt_memcpy_neon);
    SET_NEON(svt_memset, svt_memset_c, svt_memset_neon);
    SET_NEON(svt_aom_hadamard_32x32, svt_aom_hadamard_32x32_c, svt_aom_hadamard_32x32_neon);
    SET_NEON(svt_aom_hadamard_16x16, svt_aom_hadamard_16x16_c, svt_aom_hadamard_16x16_neon);
    SET_NEON(svt_aom_hadamard_8x8, svt_aom_hadamard_8x8_c, svt_aom_hadamard_8x8_neon);
    SET_NEON(svt_aom_hadamard_4x4, svt_aom_hadamard_4x4_c, svt_aom_hadamard_4x4_neon);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_NEON(svt_aom_highbd_hadamard_8x8, svt_aom_highbd_hadamard_8x8_c, svt_aom_highbd_hadamard_8x8_neon);
#endif
#else
    SET_ONLY_C(svt_aom_blend_a64_mask, svt_aom_blend_a64_mask_c);
    SET_ONLY_C(svt_aom_blend_a64_hmask, svt_aom_blend_a64_hmask_c);
    SET_ONLY_C(svt_aom_blend_a64_vmask, svt_aom_blend_a64_vmask_c);
    SET_ONLY_C(svt_aom_lowbd_blend_a64_d16_mask, svt_aom_lowbd_blend_a64_d16_mask_c);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_ONLY_C(svt_aom_highbd_blend_a64_mask, svt_aom_highbd_blend_a64_mask_c);
    SET_ONLY_C(svt_aom_highbd_blend_a64_vmask_16bit, svt_aom_highbd_blend_a64_vmask_16bit_c);
    SET_ONLY_C(svt_aom_highbd_blend_a64_hmask_16bit, svt_aom_highbd_blend_a64_hmask_16bit_c);
    SET_ONLY_C(svt_aom_highbd_blend_a64_d16_mask, svt_aom_highbd_blend_a64_d16_mask_c);
#endif
    SET_ONLY_C(svt_cfl_predict_lbd, svt_cfl_predict_lbd_c);
    SET_ONLY_C(svt_cfl_predict_hbd, svt_cfl_predict_hbd_c);
    SET_ONLY_C(svt_av1_filter_intra_predictor, svt_av1_filter_intra_predictor_c);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_ONLY_C(svt_av1_filter_intra_edge_high, svt_av1_filter_intra_edge_high_c);
#endif
    SET_ONLY_C(svt_av1_filter_intra_edge, svt_av1_filter_intra_edge_c);
    SET_ONLY_C(svt_av1_upsample_intra_edge, svt_av1_upsample_intra_edge_c);
    SET_ONLY_C(svt_av1_build_compound_diffwtd_mask_d16, svt_av1_build_compound_diffwtd_mask_d16_c);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_ONLY_C(svt_av1_highbd_wiener_convolve_add_src, svt_av1_highbd_wiener_convolve_add_src_c);
#endif
    SET_ONLY_C(svt_apply_selfguided_restoration, svt_apply_selfguided_restoration_c);
    SET_ONLY_C(svt_av1_selfguided_restoration, svt_av1_selfguided_restoration_c);
    SET_ONLY_C(svt_av1_inv_txfm2d_add_4x4, svt_av1_inv_txfm2d_add_4x4_c);
    SET_ONLY_C(svt_av1_inv_txfm2d_add_4x8, svt_av1_inv_txfm2d_add_4x8_c);
    SET_ONLY_C(svt_av1_inv_txfm2d_add_4x16, svt_av1_inv_txfm2d_add_4x16_c);
    SET_ONLY_C(svt_av1_inv_txfm2d_add_8x4, svt_av1_inv_txfm2d_add_8x4_c);
    SET_ONLY_C(svt_av1_inv_txfm2d_add_8x8, svt_av1_inv_txfm2d_add_8x8_c);
    SET_ONLY_C(svt_av1_inv_txfm2d_add_8x16, svt_av1_inv_txfm2d_add_8x16_c);
    SET_ONLY_C(svt_av1_inv_txfm2d_add_8x32, svt_av1_inv_txfm2d_add_8x32_c);
    SET_ONLY_C(svt_av1_inv_txfm2d_add_16x4, svt_av1_inv_txfm2d_add_16x4_c);
    SET_ONLY_C(svt_av1_inv_txfm2d_add_16x8, svt_av1_inv_txfm2d_add_16x8_c);
    SET_ONLY_C(svt_av1_inv_txfm2d_add_16x16, svt_av1_inv_txfm2d_add_16x16_c);
    SET_ONLY_C(svt_av1_inv_txfm2d_add_16x32, svt_av1_inv_txfm2d_add_16x32_c);
    SET_ONLY_C(svt_av1_inv_txfm2d_add_16x64, svt_av1_inv_txfm2d_add_16x64_c);
    SET_ONLY_C(svt_av1_inv_txfm2d_add_32x8, svt_av1_inv_txfm2d_add_32x8_c);
    SET_ONLY_C(svt_av1_inv_txfm2d_add_32x16, svt_av1_inv_txfm2d_add_32x16_c);
    SET_ONLY_C(svt_av1_inv_txfm2d_add_32x32, svt_av1_inv_txfm2d_add_32x32_c);
    SET_ONLY_C(svt_av1_inv_txfm2d_add_32x64, svt_av1_inv_txfm2d_add_32x64_c);
    SET_ONLY_C(svt_av1_inv_txfm2d_add_64x16, svt_av1_inv_txfm2d_add_64x16_c);
    SET_ONLY_C(svt_av1_inv_txfm2d_add_64x32, svt_av1_inv_txfm2d_add_64x32_c);
    SET_ONLY_C(svt_av1_inv_txfm2d_add_64x64, svt_av1_inv_txfm2d_add_64x64_c);
    SET_ONLY_C(svt_av1_inv_txfm_add, svt_av1_inv_txfm_add_c);
    SET_ONLY_C(svt_compressed_packmsb, svt_compressed_packmsb_c);
    SET_ONLY_C(svt_cfl_luma_subsampling_420_lbd, svt_cfl_luma_subsampling_420_lbd_c);
    SET_ONLY_C(svt_cfl_luma_subsampling_420_hbd, svt_cfl_luma_subsampling_420_hbd_c);
    SET_ONLY_C(svt_convert_8bit_to_16bit, svt_convert_8bit_to_16bit_c);
    SET_ONLY_C(svt_convert_16bit_to_8bit, svt_convert_16bit_to_8bit_c);
    SET_ONLY_C(svt_pack2d_16_bit_src_mul4, svt_enc_msb_pack2_d);
    SET_ONLY_C(svt_aom_un_pack2d_16_bit_src_mul4, svt_enc_msb_un_pack2_d);
    SET_ONLY_C(svt_full_distortion_kernel_cbf_zero32_bits, svt_full_distortion_kernel_cbf_zero32_bits_c);
    SET_ONLY_C(svt_full_distortion_kernel32_bits, svt_full_distortion_kernel32_bits_c);
    SET_ONLY_C(svt_spatial_full_distortion_kernel, svt_spatial_full_distortion_kernel_c);
    SET_ONLY_C(svt_full_distortion_kernel16_bits, svt_full_distortion_kernel16_bits_c);
    SET_ONLY_C(svt_residual_kernel8bit, svt_residual_kernel8bit_c);
    SET_ONLY_C(svt_residual_kernel16bit, svt_residual_kernel16bit_c);
    SET_ONLY_C(svt_av1_wiener_convolve_add_src, svt_av1_wiener_convolve_add_src_c);
    SET_ONLY_C(svt_av1_convolve_2d_scale, svt_av1_convolve_2d_scale_c);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_ONLY_C(svt_av1_highbd_convolve_y_sr, svt_av1_highbd_convolve_y_sr_c);
    SET_ONLY_C(svt_av1_highbd_convolve_2d_sr, svt_av1_highbd_convolve_2d_sr_c);
    SET_ONLY_C(svt_av1_highbd_convolve_2d_scale, svt_av1_highbd_convolve_2d_scale_c);
    SET_ONLY_C(svt_av1_highbd_convolve_2d_copy_sr, svt_av1_highbd_convolve_2d_copy_sr_c);
    SET_ONLY_C(svt_av1_highbd_jnt_convolve_2d, svt_av1_highbd_jnt_convolve_2d_c);
    SET_ONLY_C(svt_av1_highbd_jnt_convolve_2d_copy, svt_av1_highbd_jnt_convolve_2d_copy_c);
    SET_ONLY_C(svt_av1_highbd_jnt_convolve_x, svt_av1_highbd_jnt_convolve_x_c);
    SET_ONLY_C(svt_av1_highbd_jnt_convolve_y, svt_av1_highbd_jnt_convolve_y_c);
    SET_ONLY_C(svt_av1_highbd_convolve_x_sr, svt_av1_highbd_convolve_x_sr_c);
#endif
    SET_ONLY_C(svt_av1_convolve_2d_sr, svt_av1_convolve_2d_sr_c);
    SET_ONLY_C(svt_av1_convolve_2d_copy_sr, svt_av1_convolve_2d_copy_sr_c);
    SET_ONLY_C(svt_av1_convolve_x_sr, svt_av1_convolve_x_sr_c);
    SET_ONLY_C(svt_av1_convolve_y_sr, svt_av1_convolve_y_sr_c);
    SET_ONLY_C(svt_av1_jnt_convolve_2d, svt_av1_jnt_convolve_2d_c);
    SET_ONLY_C(svt_av1_jnt_convolve_2d_copy, svt_av1_jnt_convolve_2d_copy_c);
    SET_ONLY_C(svt_av1_jnt_convolve_x, svt_av1_jnt_convolve_x_c);
    SET_ONLY_C(svt_av1_jnt_convolve_y, svt_av1_jnt_convolve_y_c);
    SET_ONLY_C(svt_aom_convolve8_horiz, svt_aom_convolve8_horiz_c);
    SET_ONLY_C(svt_aom_convolve8_vert, svt_aom_convolve8_vert_c);
    SET_ONLY_C(svt_av1_build_compound_diffwtd_mask, svt_av1_build_compound_diffwtd_mask_c);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_ONLY_C(svt_av1_build_compound_diffwtd_mask_highbd, svt_av1_build_compound_diffwtd_mask_highbd_c);
#endif
    SET_ONLY_C(svt_av1_wedge_sse_from_residuals, svt_av1_wedge_sse_from_residuals_c);
    SET_ONLY_C(svt_aom_subtract_block, svt_aom_subtract_block_c);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_ONLY_C(svt_aom_highbd_subtract_block, svt_aom_highbd_subtract_block_c);
    SET_ONLY_C(svt_aom_highbd_smooth_v_predictor_4x4, svt_aom_highbd_smooth_v_predictor_4x4_c);
    SET_ONLY_C(svt_aom_highbd_smooth_v_predictor_4x8, svt_aom_highbd_smooth_v_predictor_4x8_c);
    SET_ONLY_C(svt_aom_highbd_smooth_v_predictor_4x16, svt_aom_highbd_smooth_v_predictor_4x16_c);
    SET_ONLY_C(svt_aom_highbd_smooth_v_predictor_8x4, svt_aom_highbd_smooth_v_predictor_8x4_c);
    SET_ONLY_C(svt_aom_highbd_smooth_v_predictor_8x8, svt_aom_highbd_smooth_v_predictor_8x8_c);
    SET_ONLY_C(svt_aom_highbd_smooth_v_predictor_8x16, svt_aom_highbd_smooth_v_predictor_8x16_c);
    SET_ONLY_C(svt_aom_highbd_smooth_v_predictor_8x32, svt_aom_highbd_smooth_v_predictor_8x32_c);
    SET_ONLY_C(svt_aom_highbd_smooth_v_predictor_16x4, svt_aom_highbd_smooth_v_predictor_16x4_c);
    SET_ONLY_C(svt_aom_highbd_smooth_v_predictor_16x8, svt_aom_highbd_smooth_v_predictor_16x8_c);
    SET_ONLY_C(svt_aom_highbd_smooth_v_predictor_16x16, svt_aom_highbd_smooth_v_predictor_16x16_c);
    SET_ONLY_C(svt_aom_highbd_smooth_v_predictor_16x32, svt_aom_highbd_smooth_v_predictor_16x32_c);
    SET_ONLY_C(svt_aom_highbd_smooth_v_predictor_16x64, svt_aom_highbd_smooth_v_predictor_16x64_c);
    SET_ONLY_C(svt_aom_highbd_smooth_v_predictor_32x8, svt_aom_highbd_smooth_v_predictor_32x8_c);
    SET_ONLY_C(svt_aom_highbd_smooth_v_predictor_32x16, svt_aom_highbd_smooth_v_predictor_32x16_c);
    SET_ONLY_C(svt_aom_highbd_smooth_v_predictor_32x32, svt_aom_highbd_smooth_v_predictor_32x32_c);
    SET_ONLY_C(svt_aom_highbd_smooth_v_predictor_32x64, svt_aom_highbd_smooth_v_predictor_32x64_c);
    SET_ONLY_C(svt_aom_highbd_smooth_v_predictor_64x16, svt_aom_highbd_smooth_v_predictor_64x16_c);
    SET_ONLY_C(svt_aom_highbd_smooth_v_predictor_64x32, svt_aom_highbd_smooth_v_predictor_64x32_c);
    SET_ONLY_C(svt_aom_highbd_smooth_v_predictor_64x64, svt_aom_highbd_smooth_v_predictor_64x64_c);
#endif
    SET_ONLY_C(svt_av1_dr_prediction_z1, svt_av1_dr_prediction_z1_c);
    SET_ONLY_C(svt_av1_dr_prediction_z2, svt_av1_dr_prediction_z2_c);
    SET_ONLY_C(svt_av1_dr_prediction_z3, svt_av1_dr_prediction_z3_c);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_ONLY_C(svt_av1_highbd_dr_prediction_z1, svt_av1_highbd_dr_prediction_z1_c);
    SET_ONLY_C(svt_av1_highbd_dr_prediction_z2, svt_av1_highbd_dr_prediction_z2_c);
    SET_ONLY_C(svt_av1_highbd_dr_prediction_z3, svt_av1_highbd_dr_prediction_z3_c);
#endif
    SET_ONLY_C(svt_aom_paeth_predictor_4x4, svt_aom_paeth_predictor_4x4_c);
    SET_ONLY_C(svt_aom_paeth_predictor_4x8, svt_aom_paeth_predictor_4x8_c);
    SET_ONLY_C(svt_aom_paeth_predictor_4x16, svt_aom_paeth_predictor_4x16_c);
    SET_ONLY_C(svt_aom_paeth_predictor_8x4, svt_aom_paeth_predictor_8x4_c);
    SET_ONLY_C(svt_aom_paeth_predictor_8x8, svt_aom_paeth_predictor_8x8_c);
    SET_ONLY_C(svt_aom_paeth_predictor_8x16, svt_aom_paeth_predictor_8x16_c);
    SET_ONLY_C(svt_aom_paeth_predictor_8x32, svt_aom_paeth_predictor_8x32_c);
    SET_ONLY_C(svt_aom_paeth_predictor_16x4, svt_aom_paeth_predictor_16x4_c);
    SET_ONLY_C(svt_aom_paeth_predictor_16x8, svt_aom_paeth_predictor_16x8_c);
    SET_ONLY_C(svt_aom_paeth_predictor_16x16, svt_aom_paeth_predictor_16x16_c);
    SET_ONLY_C(svt_aom_paeth_predictor_16x32, svt_aom_paeth_predictor_16x32_c);
    SET_ONLY_C(svt_aom_paeth_predictor_16x64, svt_aom_paeth_predictor_16x64_c);
    SET_ONLY_C(svt_aom_paeth_predictor_32x8, svt_aom_paeth_predictor_32x8_c);
    SET_ONLY_C(svt_aom_paeth_predictor_32x16, svt_aom_paeth_predictor_32x16_c);
    SET_ONLY_C(svt_aom_paeth_predictor_32x32, svt_aom_paeth_predictor_32x32_c);
    SET_ONLY_C(svt_aom_paeth_predictor_32x64, svt_aom_paeth_predictor_32x64_c);
    SET_ONLY_C(svt_aom_paeth_predictor_64x16, svt_aom_paeth_predictor_64x16_c);
    SET_ONLY_C(svt_aom_paeth_predictor_64x32, svt_aom_paeth_predictor_64x32_c);
    SET_ONLY_C(svt_aom_paeth_predictor_64x64, svt_aom_paeth_predictor_64x64_c);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_ONLY_C(svt_aom_highbd_paeth_predictor_4x4, svt_aom_highbd_paeth_predictor_4x4_c);
    SET_ONLY_C(svt_aom_highbd_paeth_predictor_4x8, svt_aom_highbd_paeth_predictor_4x8_c);
    SET_ONLY_C(svt_aom_highbd_paeth_predictor_4x16, svt_aom_highbd_paeth_predictor_4x16_c);
    SET_ONLY_C(svt_aom_highbd_paeth_predictor_8x4, svt_aom_highbd_paeth_predictor_8x4_c);
    SET_ONLY_C(svt_aom_highbd_paeth_predictor_8x8, svt_aom_highbd_paeth_predictor_8x8_c);
    SET_ONLY_C(svt_aom_highbd_paeth_predictor_8x16, svt_aom_highbd_paeth_predictor_8x16_c);
    SET_ONLY_C(svt_aom_highbd_paeth_predictor_8x32, svt_aom_highbd_paeth_predictor_8x32_c);
    SET_ONLY_C(svt_aom_highbd_paeth_predictor_16x4, svt_aom_highbd_paeth_predictor_16x4_c);
    SET_ONLY_C(svt_aom_highbd_paeth_predictor_16x8, svt_aom_highbd_paeth_predictor_16x8_c);
    SET_ONLY_C(svt_aom_highbd_paeth_predictor_16x16, svt_aom_highbd_paeth_predictor_16x16_c);
    SET_ONLY_C(svt_aom_highbd_paeth_predictor_16x32, svt_aom_highbd_paeth_predictor_16x32_c);
    SET_ONLY_C(svt_aom_highbd_paeth_predictor_16x64, svt_aom_highbd_paeth_predictor_16x64_c);
    SET_ONLY_C(svt_aom_highbd_paeth_predictor_32x8, svt_aom_highbd_paeth_predictor_32x8_c);
    SET_ONLY_C(svt_aom_highbd_paeth_predictor_32x16, svt_aom_highbd_paeth_predictor_32x16_c);
    SET_ONLY_C(svt_aom_highbd_paeth_predictor_32x32, svt_aom_highbd_paeth_predictor_32x32_c);
    SET_ONLY_C(svt_aom_highbd_paeth_predictor_32x64, svt_aom_highbd_paeth_predictor_32x64_c);
    SET_ONLY_C(svt_aom_highbd_paeth_predictor_64x16, svt_aom_highbd_paeth_predictor_64x16_c);
    SET_ONLY_C(svt_aom_highbd_paeth_predictor_64x32, svt_aom_highbd_paeth_predictor_64x32_c);
    SET_ONLY_C(svt_aom_highbd_paeth_predictor_64x64, svt_aom_highbd_paeth_predictor_64x64_c);
#endif
    SET_ONLY_C(svt_aom_sum_squares_i16, svt_aom_sum_squares_i16_c);
    SET_ONLY_C(svt_aom_dc_predictor_4x4, svt_aom_dc_predictor_4x4_c);
    SET_ONLY_C(svt_aom_dc_predictor_4x8, svt_aom_dc_predictor_4x8_c);
    SET_ONLY_C(svt_aom_dc_predictor_4x16, svt_aom_dc_predictor_4x16_c);
    SET_ONLY_C(svt_aom_dc_predictor_8x4, svt_aom_dc_predictor_8x4_c);
    SET_ONLY_C(svt_aom_dc_predictor_8x8, svt_aom_dc_predictor_8x8_c);
    SET_ONLY_C(svt_aom_dc_predictor_8x16, svt_aom_dc_predictor_8x16_c);
    SET_ONLY_C(svt_aom_dc_predictor_8x32, svt_aom_dc_predictor_8x32_c);
    SET_ONLY_C(svt_aom_dc_predictor_16x4, svt_aom_dc_predictor_16x4_c);
    SET_ONLY_C(svt_aom_dc_predictor_16x8, svt_aom_dc_predictor_16x8_c);
    SET_ONLY_C(svt_aom_dc_predictor_16x16, svt_aom_dc_predictor_16x16_c);
    SET_ONLY_C(svt_aom_dc_predictor_16x32, svt_aom_dc_predictor_16x32_c);
    SET_ONLY_C(svt_aom_dc_predictor_16x64, svt_aom_dc_predictor_16x64_c);
    SET_ONLY_C(svt_aom_dc_predictor_32x8, svt_aom_dc_predictor_32x8_c);
    SET_ONLY_C(svt_aom_dc_predictor_32x16, svt_aom_dc_predictor_32x16_c);
    SET_ONLY_C(svt_aom_dc_predictor_32x32, svt_aom_dc_predictor_32x32_c);
    SET_ONLY_C(svt_aom_dc_predictor_32x64, svt_aom_dc_predictor_32x64_c);
    SET_ONLY_C(svt_aom_dc_predictor_64x16, svt_aom_dc_predictor_64x16_c);
    SET_ONLY_C(svt_aom_dc_predictor_64x32, svt_aom_dc_predictor_64x32_c);
    SET_ONLY_C(svt_aom_dc_predictor_64x64, svt_aom_dc_predictor_64x64_c);

    SET_ONLY_C(svt_aom_dc_top_predictor_4x4, svt_aom_dc_top_predictor_4x4_c);
    SET_ONLY_C(svt_aom_dc_top_predictor_4x8, svt_aom_dc_top_predictor_4x8_c);
    SET_ONLY_C(svt_aom_dc_top_predictor_4x16, svt_aom_dc_top_predictor_4x16_c);
    SET_ONLY_C(svt_aom_dc_top_predictor_8x4, svt_aom_dc_top_predictor_8x4_c);
    SET_ONLY_C(svt_aom_dc_top_predictor_8x8, svt_aom_dc_top_predictor_8x8_c);
    SET_ONLY_C(svt_aom_dc_top_predictor_8x16, svt_aom_dc_top_predictor_8x16_c);
    SET_ONLY_C(svt_aom_dc_top_predictor_8x32, svt_aom_dc_top_predictor_8x32_c);
    SET_ONLY_C(svt_aom_dc_top_predictor_16x4, svt_aom_dc_top_predictor_16x4_c);
    SET_ONLY_C(svt_aom_dc_top_predictor_16x8, svt_aom_dc_top_predictor_16x8_c);
    SET_ONLY_C(svt_aom_dc_top_predictor_16x16, svt_aom_dc_top_predictor_16x16_c);
    SET_ONLY_C(svt_aom_dc_top_predictor_16x32, svt_aom_dc_top_predictor_16x32_c);
    SET_ONLY_C(svt_aom_dc_top_predictor_16x64, svt_aom_dc_top_predictor_16x64_c);
    SET_ONLY_C(svt_aom_dc_top_predictor_32x8, svt_aom_dc_top_predictor_32x8_c);
    SET_ONLY_C(svt_aom_dc_top_predictor_32x16, svt_aom_dc_top_predictor_32x16_c);
    SET_ONLY_C(svt_aom_dc_top_predictor_32x32, svt_aom_dc_top_predictor_32x32_c);
    SET_ONLY_C(svt_aom_dc_top_predictor_32x64, svt_aom_dc_top_predictor_32x64_c);
    SET_ONLY_C(svt_aom_dc_top_predictor_64x16, svt_aom_dc_top_predictor_64x16_c);
    SET_ONLY_C(svt_aom_dc_top_predictor_64x32, svt_aom_dc_top_predictor_64x32_c);
    SET_ONLY_C(svt_aom_dc_top_predictor_64x64, svt_aom_dc_top_predictor_64x64_c);

    SET_ONLY_C(svt_aom_dc_left_predictor_4x4, svt_aom_dc_left_predictor_4x4_c);
    SET_ONLY_C(svt_aom_dc_left_predictor_4x8, svt_aom_dc_left_predictor_4x8_c);
    SET_ONLY_C(svt_aom_dc_left_predictor_4x16, svt_aom_dc_left_predictor_4x16_c);
    SET_ONLY_C(svt_aom_dc_left_predictor_8x4, svt_aom_dc_left_predictor_8x4_c);
    SET_ONLY_C(svt_aom_dc_left_predictor_8x8, svt_aom_dc_left_predictor_8x8_c);
    SET_ONLY_C(svt_aom_dc_left_predictor_8x16, svt_aom_dc_left_predictor_8x16_c);
    SET_ONLY_C(svt_aom_dc_left_predictor_8x32, svt_aom_dc_left_predictor_8x32_c);
    SET_ONLY_C(svt_aom_dc_left_predictor_16x4, svt_aom_dc_left_predictor_16x4_c);
    SET_ONLY_C(svt_aom_dc_left_predictor_16x8, svt_aom_dc_left_predictor_16x8_c);
    SET_ONLY_C(svt_aom_dc_left_predictor_16x16, svt_aom_dc_left_predictor_16x16_c);
    SET_ONLY_C(svt_aom_dc_left_predictor_16x32, svt_aom_dc_left_predictor_16x32_c);
    SET_ONLY_C(svt_aom_dc_left_predictor_16x64, svt_aom_dc_left_predictor_16x64_c);
    SET_ONLY_C(svt_aom_dc_left_predictor_32x8, svt_aom_dc_left_predictor_32x8_c);
    SET_ONLY_C(svt_aom_dc_left_predictor_32x16, svt_aom_dc_left_predictor_32x16_c);
    SET_ONLY_C(svt_aom_dc_left_predictor_32x32, svt_aom_dc_left_predictor_32x32_c);
    SET_ONLY_C(svt_aom_dc_left_predictor_32x64, svt_aom_dc_left_predictor_32x64_c);
    SET_ONLY_C(svt_aom_dc_left_predictor_64x16, svt_aom_dc_left_predictor_64x16_c);
    SET_ONLY_C(svt_aom_dc_left_predictor_64x32, svt_aom_dc_left_predictor_64x32_c);
    SET_ONLY_C(svt_aom_dc_left_predictor_64x64, svt_aom_dc_left_predictor_64x64_c);

    SET_ONLY_C(svt_aom_dc_128_predictor_4x4, svt_aom_dc_128_predictor_4x4_c);
    SET_ONLY_C(svt_aom_dc_128_predictor_4x8, svt_aom_dc_128_predictor_4x8_c);
    SET_ONLY_C(svt_aom_dc_128_predictor_4x16, svt_aom_dc_128_predictor_4x16_c);
    SET_ONLY_C(svt_aom_dc_128_predictor_8x4, svt_aom_dc_128_predictor_8x4_c);
    SET_ONLY_C(svt_aom_dc_128_predictor_8x8, svt_aom_dc_128_predictor_8x8_c);
    SET_ONLY_C(svt_aom_dc_128_predictor_8x16, svt_aom_dc_128_predictor_8x16_c);
    SET_ONLY_C(svt_aom_dc_128_predictor_8x32, svt_aom_dc_128_predictor_8x32_c);
    SET_ONLY_C(svt_aom_dc_128_predictor_16x4, svt_aom_dc_128_predictor_16x4_c);
    SET_ONLY_C(svt_aom_dc_128_predictor_16x8, svt_aom_dc_128_predictor_16x8_c);
    SET_ONLY_C(svt_aom_dc_128_predictor_16x16, svt_aom_dc_128_predictor_16x16_c);
    SET_ONLY_C(svt_aom_dc_128_predictor_16x32, svt_aom_dc_128_predictor_16x32_c);
    SET_ONLY_C(svt_aom_dc_128_predictor_16x64, svt_aom_dc_128_predictor_16x64_c);
    SET_ONLY_C(svt_aom_dc_128_predictor_32x8, svt_aom_dc_128_predictor_32x8_c);
    SET_ONLY_C(svt_aom_dc_128_predictor_32x16, svt_aom_dc_128_predictor_32x16_c);
    SET_ONLY_C(svt_aom_dc_128_predictor_32x32, svt_aom_dc_128_predictor_32x32_c);
    SET_ONLY_C(svt_aom_dc_128_predictor_32x64, svt_aom_dc_128_predictor_32x64_c);
    SET_ONLY_C(svt_aom_dc_128_predictor_64x16, svt_aom_dc_128_predictor_64x16_c);
    SET_ONLY_C(svt_aom_dc_128_predictor_64x32, svt_aom_dc_128_predictor_64x32_c);
    SET_ONLY_C(svt_aom_dc_128_predictor_64x64, svt_aom_dc_128_predictor_64x64_c);

    SET_ONLY_C(svt_aom_smooth_h_predictor_4x4, svt_aom_smooth_h_predictor_4x4_c);
    SET_ONLY_C(svt_aom_smooth_h_predictor_4x8, svt_aom_smooth_h_predictor_4x8_c);
    SET_ONLY_C(svt_aom_smooth_h_predictor_4x16, svt_aom_smooth_h_predictor_4x16_c);
    SET_ONLY_C(svt_aom_smooth_h_predictor_8x4, svt_aom_smooth_h_predictor_8x4_c);
    SET_ONLY_C(svt_aom_smooth_h_predictor_8x8, svt_aom_smooth_h_predictor_8x8_c);
    SET_ONLY_C(svt_aom_smooth_h_predictor_8x16, svt_aom_smooth_h_predictor_8x16_c);
    SET_ONLY_C(svt_aom_smooth_h_predictor_8x32, svt_aom_smooth_h_predictor_8x32_c);
    SET_ONLY_C(svt_aom_smooth_h_predictor_16x4, svt_aom_smooth_h_predictor_16x4_c);
    SET_ONLY_C(svt_aom_smooth_h_predictor_16x8, svt_aom_smooth_h_predictor_16x8_c);
    SET_ONLY_C(svt_aom_smooth_h_predictor_16x16, svt_aom_smooth_h_predictor_16x16_c);
    SET_ONLY_C(svt_aom_smooth_h_predictor_16x32, svt_aom_smooth_h_predictor_16x32_c);
    SET_ONLY_C(svt_aom_smooth_h_predictor_16x64, svt_aom_smooth_h_predictor_16x64_c);
    SET_ONLY_C(svt_aom_smooth_h_predictor_32x8, svt_aom_smooth_h_predictor_32x8_c);
    SET_ONLY_C(svt_aom_smooth_h_predictor_32x16, svt_aom_smooth_h_predictor_32x16_c);
    SET_ONLY_C(svt_aom_smooth_h_predictor_32x32, svt_aom_smooth_h_predictor_32x32_c);
    SET_ONLY_C(svt_aom_smooth_h_predictor_32x64, svt_aom_smooth_h_predictor_32x64_c);
    SET_ONLY_C(svt_aom_smooth_h_predictor_64x16, svt_aom_smooth_h_predictor_64x16_c);
    SET_ONLY_C(svt_aom_smooth_h_predictor_64x32, svt_aom_smooth_h_predictor_64x32_c);
    SET_ONLY_C(svt_aom_smooth_h_predictor_64x64, svt_aom_smooth_h_predictor_64x64_c);

    SET_ONLY_C(svt_aom_smooth_v_predictor_4x4, svt_aom_smooth_v_predictor_4x4_c);
    SET_ONLY_C(svt_aom_smooth_v_predictor_4x8, svt_aom_smooth_v_predictor_4x8_c);
    SET_ONLY_C(svt_aom_smooth_v_predictor_4x16, svt_aom_smooth_v_predictor_4x16_c);
    SET_ONLY_C(svt_aom_smooth_v_predictor_8x4, svt_aom_smooth_v_predictor_8x4_c);
    SET_ONLY_C(svt_aom_smooth_v_predictor_8x8, svt_aom_smooth_v_predictor_8x8_c);
    SET_ONLY_C(svt_aom_smooth_v_predictor_8x16, svt_aom_smooth_v_predictor_8x16_c);
    SET_ONLY_C(svt_aom_smooth_v_predictor_8x32, svt_aom_smooth_v_predictor_8x32_c);
    SET_ONLY_C(svt_aom_smooth_v_predictor_16x4, svt_aom_smooth_v_predictor_16x4_c);
    SET_ONLY_C(svt_aom_smooth_v_predictor_16x8, svt_aom_smooth_v_predictor_16x8_c);
    SET_ONLY_C(svt_aom_smooth_v_predictor_16x16, svt_aom_smooth_v_predictor_16x16_c);
    SET_ONLY_C(svt_aom_smooth_v_predictor_16x32, svt_aom_smooth_v_predictor_16x32_c);
    SET_ONLY_C(svt_aom_smooth_v_predictor_16x64, svt_aom_smooth_v_predictor_16x64_c);
    SET_ONLY_C(svt_aom_smooth_v_predictor_32x8, svt_aom_smooth_v_predictor_32x8_c);
    SET_ONLY_C(svt_aom_smooth_v_predictor_32x16, svt_aom_smooth_v_predictor_32x16_c);
    SET_ONLY_C(svt_aom_smooth_v_predictor_32x32, svt_aom_smooth_v_predictor_32x32_c);
    SET_ONLY_C(svt_aom_smooth_v_predictor_32x64, svt_aom_smooth_v_predictor_32x64_c);
    SET_ONLY_C(svt_aom_smooth_v_predictor_64x16, svt_aom_smooth_v_predictor_64x16_c);
    SET_ONLY_C(svt_aom_smooth_v_predictor_64x32, svt_aom_smooth_v_predictor_64x32_c);
    SET_ONLY_C(svt_aom_smooth_v_predictor_64x64, svt_aom_smooth_v_predictor_64x64_c);

    SET_ONLY_C(svt_aom_smooth_predictor_4x4, svt_aom_smooth_predictor_4x4_c);
    SET_ONLY_C(svt_aom_smooth_predictor_4x8, svt_aom_smooth_predictor_4x8_c);
    SET_ONLY_C(svt_aom_smooth_predictor_4x16, svt_aom_smooth_predictor_4x16_c);
    SET_ONLY_C(svt_aom_smooth_predictor_8x4, svt_aom_smooth_predictor_8x4_c);
    SET_ONLY_C(svt_aom_smooth_predictor_8x8, svt_aom_smooth_predictor_8x8_c);
    SET_ONLY_C(svt_aom_smooth_predictor_8x16, svt_aom_smooth_predictor_8x16_c);
    SET_ONLY_C(svt_aom_smooth_predictor_8x32, svt_aom_smooth_predictor_8x32_c);
    SET_ONLY_C(svt_aom_smooth_predictor_16x4, svt_aom_smooth_predictor_16x4_c);
    SET_ONLY_C(svt_aom_smooth_predictor_16x8, svt_aom_smooth_predictor_16x8_c);
    SET_ONLY_C(svt_aom_smooth_predictor_16x16, svt_aom_smooth_predictor_16x16_c);
    SET_ONLY_C(svt_aom_smooth_predictor_16x32, svt_aom_smooth_predictor_16x32_c);
    SET_ONLY_C(svt_aom_smooth_predictor_16x64, svt_aom_smooth_predictor_16x64_c);
    SET_ONLY_C(svt_aom_smooth_predictor_32x8, svt_aom_smooth_predictor_32x8_c);
    SET_ONLY_C(svt_aom_smooth_predictor_32x16, svt_aom_smooth_predictor_32x16_c);
    SET_ONLY_C(svt_aom_smooth_predictor_32x32, svt_aom_smooth_predictor_32x32_c);
    SET_ONLY_C(svt_aom_smooth_predictor_32x64, svt_aom_smooth_predictor_32x64_c);
    SET_ONLY_C(svt_aom_smooth_predictor_64x16, svt_aom_smooth_predictor_64x16_c);
    SET_ONLY_C(svt_aom_smooth_predictor_64x32, svt_aom_smooth_predictor_64x32_c);
    SET_ONLY_C(svt_aom_smooth_predictor_64x64, svt_aom_smooth_predictor_64x64_c);

    SET_ONLY_C(svt_aom_v_predictor_4x4, svt_aom_v_predictor_4x4_c);
    SET_ONLY_C(svt_aom_v_predictor_4x8, svt_aom_v_predictor_4x8_c);
    SET_ONLY_C(svt_aom_v_predictor_4x16, svt_aom_v_predictor_4x16_c);
    SET_ONLY_C(svt_aom_v_predictor_8x4, svt_aom_v_predictor_8x4_c);
    SET_ONLY_C(svt_aom_v_predictor_8x8, svt_aom_v_predictor_8x8_c);
    SET_ONLY_C(svt_aom_v_predictor_8x16, svt_aom_v_predictor_8x16_c);
    SET_ONLY_C(svt_aom_v_predictor_8x32, svt_aom_v_predictor_8x32_c);
    SET_ONLY_C(svt_aom_v_predictor_16x4, svt_aom_v_predictor_16x4_c);
    SET_ONLY_C(svt_aom_v_predictor_16x8, svt_aom_v_predictor_16x8_c);
    SET_ONLY_C(svt_aom_v_predictor_16x16, svt_aom_v_predictor_16x16_c);
    SET_ONLY_C(svt_aom_v_predictor_16x32, svt_aom_v_predictor_16x32_c);
    SET_ONLY_C(svt_aom_v_predictor_16x64, svt_aom_v_predictor_16x64_c);
    SET_ONLY_C(svt_aom_v_predictor_32x8, svt_aom_v_predictor_32x8_c);
    SET_ONLY_C(svt_aom_v_predictor_32x16, svt_aom_v_predictor_32x16_c);
    SET_ONLY_C(svt_aom_v_predictor_32x32, svt_aom_v_predictor_32x32_c);
    SET_ONLY_C(svt_aom_v_predictor_32x64, svt_aom_v_predictor_32x64_c);
    SET_ONLY_C(svt_aom_v_predictor_64x16, svt_aom_v_predictor_64x16_c);
    SET_ONLY_C(svt_aom_v_predictor_64x32, svt_aom_v_predictor_64x32_c);
    SET_ONLY_C(svt_aom_v_predictor_64x64, svt_aom_v_predictor_64x64_c);

    SET_ONLY_C(svt_aom_h_predictor_4x4, svt_aom_h_predictor_4x4_c);
    SET_ONLY_C(svt_aom_h_predictor_4x8, svt_aom_h_predictor_4x8_c);
    SET_ONLY_C(svt_aom_h_predictor_4x16, svt_aom_h_predictor_4x16_c);
    SET_ONLY_C(svt_aom_h_predictor_8x4, svt_aom_h_predictor_8x4_c);
    SET_ONLY_C(svt_aom_h_predictor_8x8, svt_aom_h_predictor_8x8_c);
    SET_ONLY_C(svt_aom_h_predictor_8x16, svt_aom_h_predictor_8x16_c);
    SET_ONLY_C(svt_aom_h_predictor_8x32, svt_aom_h_predictor_8x32_c);
    SET_ONLY_C(svt_aom_h_predictor_16x4, svt_aom_h_predictor_16x4_c);
    SET_ONLY_C(svt_aom_h_predictor_16x8, svt_aom_h_predictor_16x8_c);
    SET_ONLY_C(svt_aom_h_predictor_16x16, svt_aom_h_predictor_16x16_c);
    SET_ONLY_C(svt_aom_h_predictor_16x32, svt_aom_h_predictor_16x32_c);
    SET_ONLY_C(svt_aom_h_predictor_16x64, svt_aom_h_predictor_16x64_c);
    SET_ONLY_C(svt_aom_h_predictor_32x8, svt_aom_h_predictor_32x8_c);
    SET_ONLY_C(svt_aom_h_predictor_32x16, svt_aom_h_predictor_32x16_c);
    SET_ONLY_C(svt_aom_h_predictor_32x32, svt_aom_h_predictor_32x32_c);
    SET_ONLY_C(svt_aom_h_predictor_32x64, svt_aom_h_predictor_32x64_c);
    SET_ONLY_C(svt_aom_h_predictor_64x16, svt_aom_h_predictor_64x16_c);
    SET_ONLY_C(svt_aom_h_predictor_64x32, svt_aom_h_predictor_64x32_c);
    SET_ONLY_C(svt_aom_h_predictor_64x64, svt_aom_h_predictor_64x64_c);
    SET_ONLY_C(svt_aom_cdef_find_dir, svt_aom_cdef_find_dir_c);
    SET_ONLY_C(svt_aom_cdef_find_dir_dual, svt_aom_cdef_find_dir_dual_c);
    SET_ONLY_C(svt_cdef_filter_block, svt_cdef_filter_block_c);
    SET_ONLY_C(svt_aom_copy_rect8_8bit_to_16bit, svt_aom_copy_rect8_8bit_to_16bit_c);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_ONLY_C(svt_av1_highbd_warp_affine, svt_av1_highbd_warp_affine_c);
#endif
    SET_ONLY_C(svt_av1_warp_affine, svt_av1_warp_affine_c);

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_ONLY_C(svt_aom_highbd_lpf_horizontal_4, svt_aom_highbd_lpf_horizontal_4_c);
    SET_ONLY_C(svt_aom_highbd_lpf_horizontal_6, svt_aom_highbd_lpf_horizontal_6_c);
    SET_ONLY_C(svt_aom_highbd_lpf_horizontal_8, svt_aom_highbd_lpf_horizontal_8_c);
    SET_ONLY_C(svt_aom_highbd_lpf_horizontal_14, svt_aom_highbd_lpf_horizontal_14_c);
    SET_ONLY_C(svt_aom_highbd_lpf_vertical_4, svt_aom_highbd_lpf_vertical_4_c);
    SET_ONLY_C(svt_aom_highbd_lpf_vertical_6, svt_aom_highbd_lpf_vertical_6_c);
    SET_ONLY_C(svt_aom_highbd_lpf_vertical_8, svt_aom_highbd_lpf_vertical_8_c);
    SET_ONLY_C(svt_aom_highbd_lpf_vertical_14, svt_aom_highbd_lpf_vertical_14_c);
#endif
    SET_ONLY_C(svt_aom_lpf_horizontal_4, svt_aom_lpf_horizontal_4_c);
    SET_ONLY_C(svt_aom_lpf_horizontal_6, svt_aom_lpf_horizontal_6_c);
    SET_ONLY_C(svt_aom_lpf_horizontal_8, svt_aom_lpf_horizontal_8_c);
    SET_ONLY_C(svt_aom_lpf_horizontal_14, svt_aom_lpf_horizontal_14_c);
    SET_ONLY_C(svt_aom_lpf_vertical_4, svt_aom_lpf_vertical_4_c);
    SET_ONLY_C(svt_aom_lpf_vertical_6, svt_aom_lpf_vertical_6_c);
    SET_ONLY_C(svt_aom_lpf_vertical_8, svt_aom_lpf_vertical_8_c);
    SET_ONLY_C(svt_aom_lpf_vertical_14, svt_aom_lpf_vertical_14_c);

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    // svt_aom_highbd_v_predictor
    SET_ONLY_C(svt_aom_highbd_v_predictor_4x4, svt_aom_highbd_v_predictor_4x4_c);
    SET_ONLY_C(svt_aom_highbd_v_predictor_4x8, svt_aom_highbd_v_predictor_4x8_c);
    SET_ONLY_C(svt_aom_highbd_v_predictor_4x16, svt_aom_highbd_v_predictor_4x16_c);
    SET_ONLY_C(svt_aom_highbd_v_predictor_8x4, svt_aom_highbd_v_predictor_8x4_c);
    SET_ONLY_C(svt_aom_highbd_v_predictor_8x8, svt_aom_highbd_v_predictor_8x8_c);
    SET_ONLY_C(svt_aom_highbd_v_predictor_8x16, svt_aom_highbd_v_predictor_8x16_c);
    SET_ONLY_C(svt_aom_highbd_v_predictor_8x32, svt_aom_highbd_v_predictor_8x32_c);
    SET_ONLY_C(svt_aom_highbd_v_predictor_16x4, svt_aom_highbd_v_predictor_16x4_c);
    SET_ONLY_C(svt_aom_highbd_v_predictor_16x8, svt_aom_highbd_v_predictor_16x8_c);
    SET_ONLY_C(svt_aom_highbd_v_predictor_16x16, svt_aom_highbd_v_predictor_16x16_c);
    SET_ONLY_C(svt_aom_highbd_v_predictor_16x32, svt_aom_highbd_v_predictor_16x32_c);
    SET_ONLY_C(svt_aom_highbd_v_predictor_16x64, svt_aom_highbd_v_predictor_16x64_c);
    SET_ONLY_C(svt_aom_highbd_v_predictor_32x8, svt_aom_highbd_v_predictor_32x8_c);
    SET_ONLY_C(svt_aom_highbd_v_predictor_32x16, svt_aom_highbd_v_predictor_32x16_c);
    SET_ONLY_C(svt_aom_highbd_v_predictor_32x32, svt_aom_highbd_v_predictor_32x32_c);
    SET_ONLY_C(svt_aom_highbd_v_predictor_32x64, svt_aom_highbd_v_predictor_32x64_c);
    SET_ONLY_C(svt_aom_highbd_v_predictor_64x16, svt_aom_highbd_v_predictor_64x16_c);
    SET_ONLY_C(svt_aom_highbd_v_predictor_64x32, svt_aom_highbd_v_predictor_64x32_c);
    SET_ONLY_C(svt_aom_highbd_v_predictor_64x64, svt_aom_highbd_v_predictor_64x64_c);

    //aom_highbd_smooth_predictor
    SET_ONLY_C(svt_aom_highbd_smooth_predictor_4x4, svt_aom_highbd_smooth_predictor_4x4_c);
    SET_ONLY_C(svt_aom_highbd_smooth_predictor_4x8, svt_aom_highbd_smooth_predictor_4x8_c);
    SET_ONLY_C(svt_aom_highbd_smooth_predictor_4x16, svt_aom_highbd_smooth_predictor_4x16_c);
    SET_ONLY_C(svt_aom_highbd_smooth_predictor_8x4, svt_aom_highbd_smooth_predictor_8x4_c);
    SET_ONLY_C(svt_aom_highbd_smooth_predictor_8x8, svt_aom_highbd_smooth_predictor_8x8_c);
    SET_ONLY_C(svt_aom_highbd_smooth_predictor_8x16, svt_aom_highbd_smooth_predictor_8x16_c);
    SET_ONLY_C(svt_aom_highbd_smooth_predictor_8x32, svt_aom_highbd_smooth_predictor_8x32_c);
    SET_ONLY_C(svt_aom_highbd_smooth_predictor_16x4, svt_aom_highbd_smooth_predictor_16x4_c);
    SET_ONLY_C(svt_aom_highbd_smooth_predictor_16x8, svt_aom_highbd_smooth_predictor_16x8_c);
    SET_ONLY_C(svt_aom_highbd_smooth_predictor_16x16, svt_aom_highbd_smooth_predictor_16x16_c);
    SET_ONLY_C(svt_aom_highbd_smooth_predictor_16x32, svt_aom_highbd_smooth_predictor_16x32_c);
    SET_ONLY_C(svt_aom_highbd_smooth_predictor_16x64, svt_aom_highbd_smooth_predictor_16x64_c);
    SET_ONLY_C(svt_aom_highbd_smooth_predictor_32x8, svt_aom_highbd_smooth_predictor_32x8_c);
    SET_ONLY_C(svt_aom_highbd_smooth_predictor_32x16, svt_aom_highbd_smooth_predictor_32x16_c);
    SET_ONLY_C(svt_aom_highbd_smooth_predictor_32x32, svt_aom_highbd_smooth_predictor_32x32_c);
    SET_ONLY_C(svt_aom_highbd_smooth_predictor_32x64, svt_aom_highbd_smooth_predictor_32x64_c);
    SET_ONLY_C(svt_aom_highbd_smooth_predictor_64x16, svt_aom_highbd_smooth_predictor_64x16_c);
    SET_ONLY_C(svt_aom_highbd_smooth_predictor_64x32, svt_aom_highbd_smooth_predictor_64x32_c);
    SET_ONLY_C(svt_aom_highbd_smooth_predictor_64x64, svt_aom_highbd_smooth_predictor_64x64_c);

    //aom_highbd_smooth_h_predictor
    SET_ONLY_C(svt_aom_highbd_smooth_h_predictor_4x4, svt_aom_highbd_smooth_h_predictor_4x4_c);
    SET_ONLY_C(svt_aom_highbd_smooth_h_predictor_4x8, svt_aom_highbd_smooth_h_predictor_4x8_c);
    SET_ONLY_C(svt_aom_highbd_smooth_h_predictor_4x16, svt_aom_highbd_smooth_h_predictor_4x16_c);
    SET_ONLY_C(svt_aom_highbd_smooth_h_predictor_8x4, svt_aom_highbd_smooth_h_predictor_8x4_c);
    SET_ONLY_C(svt_aom_highbd_smooth_h_predictor_8x8, svt_aom_highbd_smooth_h_predictor_8x8_c);
    SET_ONLY_C(svt_aom_highbd_smooth_h_predictor_8x16, svt_aom_highbd_smooth_h_predictor_8x16_c);
    SET_ONLY_C(svt_aom_highbd_smooth_h_predictor_8x32, svt_aom_highbd_smooth_h_predictor_8x32_c);
    SET_ONLY_C(svt_aom_highbd_smooth_h_predictor_16x4, svt_aom_highbd_smooth_h_predictor_16x4_c);
    SET_ONLY_C(svt_aom_highbd_smooth_h_predictor_16x8, svt_aom_highbd_smooth_h_predictor_16x8_c);
    SET_ONLY_C(svt_aom_highbd_smooth_h_predictor_16x16, svt_aom_highbd_smooth_h_predictor_16x16_c);
    SET_ONLY_C(svt_aom_highbd_smooth_h_predictor_16x32, svt_aom_highbd_smooth_h_predictor_16x32_c);
    SET_ONLY_C(svt_aom_highbd_smooth_h_predictor_16x64, svt_aom_highbd_smooth_h_predictor_16x64_c);
    SET_ONLY_C(svt_aom_highbd_smooth_h_predictor_32x8, svt_aom_highbd_smooth_h_predictor_32x8_c);
    SET_ONLY_C(svt_aom_highbd_smooth_h_predictor_32x16, svt_aom_highbd_smooth_h_predictor_32x16_c);
    SET_ONLY_C(svt_aom_highbd_smooth_h_predictor_32x32, svt_aom_highbd_smooth_h_predictor_32x32_c);
    SET_ONLY_C(svt_aom_highbd_smooth_h_predictor_32x64, svt_aom_highbd_smooth_h_predictor_32x64_c);
    SET_ONLY_C(svt_aom_highbd_smooth_h_predictor_64x16, svt_aom_highbd_smooth_h_predictor_64x16_c);
    SET_ONLY_C(svt_aom_highbd_smooth_h_predictor_64x32, svt_aom_highbd_smooth_h_predictor_64x32_c);
    SET_ONLY_C(svt_aom_highbd_smooth_h_predictor_64x64, svt_aom_highbd_smooth_h_predictor_64x64_c);

    //aom_highbd_dc_128_predictor
    SET_ONLY_C(svt_aom_highbd_dc_128_predictor_4x4, svt_aom_highbd_dc_128_predictor_4x4_c);
    SET_ONLY_C(svt_aom_highbd_dc_128_predictor_4x8, svt_aom_highbd_dc_128_predictor_4x8_c);
    SET_ONLY_C(svt_aom_highbd_dc_128_predictor_4x16, svt_aom_highbd_dc_128_predictor_4x16_c);
    SET_ONLY_C(svt_aom_highbd_dc_128_predictor_8x4, svt_aom_highbd_dc_128_predictor_8x4_c);
    SET_ONLY_C(svt_aom_highbd_dc_128_predictor_8x8, svt_aom_highbd_dc_128_predictor_8x8_c);
    SET_ONLY_C(svt_aom_highbd_dc_128_predictor_8x16, svt_aom_highbd_dc_128_predictor_8x16_c);
    SET_ONLY_C(svt_aom_highbd_dc_128_predictor_8x32, svt_aom_highbd_dc_128_predictor_8x32_c);
    SET_ONLY_C(svt_aom_highbd_dc_128_predictor_16x4, svt_aom_highbd_dc_128_predictor_16x4_c);
    SET_ONLY_C(svt_aom_highbd_dc_128_predictor_16x8, svt_aom_highbd_dc_128_predictor_16x8_c);
    SET_ONLY_C(svt_aom_highbd_dc_128_predictor_16x16, svt_aom_highbd_dc_128_predictor_16x16_c);
    SET_ONLY_C(svt_aom_highbd_dc_128_predictor_16x32, svt_aom_highbd_dc_128_predictor_16x32_c);
    SET_ONLY_C(svt_aom_highbd_dc_128_predictor_16x64, svt_aom_highbd_dc_128_predictor_16x64_c);
    SET_ONLY_C(svt_aom_highbd_dc_128_predictor_32x8, svt_aom_highbd_dc_128_predictor_32x8_c);
    SET_ONLY_C(svt_aom_highbd_dc_128_predictor_32x16, svt_aom_highbd_dc_128_predictor_32x16_c);
    SET_ONLY_C(svt_aom_highbd_dc_128_predictor_32x32, svt_aom_highbd_dc_128_predictor_32x32_c);
    SET_ONLY_C(svt_aom_highbd_dc_128_predictor_32x64, svt_aom_highbd_dc_128_predictor_32x64_c);
    SET_ONLY_C(svt_aom_highbd_dc_128_predictor_64x16, svt_aom_highbd_dc_128_predictor_64x16_c);
    SET_ONLY_C(svt_aom_highbd_dc_128_predictor_64x32, svt_aom_highbd_dc_128_predictor_64x32_c);
    SET_ONLY_C(svt_aom_highbd_dc_128_predictor_64x64, svt_aom_highbd_dc_128_predictor_64x64_c);

    //aom_highbd_dc_left_predictor
    SET_ONLY_C(svt_aom_highbd_dc_left_predictor_4x4, svt_aom_highbd_dc_left_predictor_4x4_c);
    SET_ONLY_C(svt_aom_highbd_dc_left_predictor_4x8, svt_aom_highbd_dc_left_predictor_4x8_c);
    SET_ONLY_C(svt_aom_highbd_dc_left_predictor_4x16, svt_aom_highbd_dc_left_predictor_4x16_c);
    SET_ONLY_C(svt_aom_highbd_dc_left_predictor_8x4, svt_aom_highbd_dc_left_predictor_8x4_c);
    SET_ONLY_C(svt_aom_highbd_dc_left_predictor_8x8, svt_aom_highbd_dc_left_predictor_8x8_c);
    SET_ONLY_C(svt_aom_highbd_dc_left_predictor_8x16, svt_aom_highbd_dc_left_predictor_8x16_c);
    SET_ONLY_C(svt_aom_highbd_dc_left_predictor_8x32, svt_aom_highbd_dc_left_predictor_8x32_c);
    SET_ONLY_C(svt_aom_highbd_dc_left_predictor_16x4, svt_aom_highbd_dc_left_predictor_16x4_c);
    SET_ONLY_C(svt_aom_highbd_dc_left_predictor_16x8, svt_aom_highbd_dc_left_predictor_16x8_c);
    SET_ONLY_C(svt_aom_highbd_dc_left_predictor_16x16, svt_aom_highbd_dc_left_predictor_16x16_c);
    SET_ONLY_C(svt_aom_highbd_dc_left_predictor_16x32, svt_aom_highbd_dc_left_predictor_16x32_c);
    SET_ONLY_C(svt_aom_highbd_dc_left_predictor_16x64, svt_aom_highbd_dc_left_predictor_16x64_c);
    SET_ONLY_C(svt_aom_highbd_dc_left_predictor_32x8, svt_aom_highbd_dc_left_predictor_32x8_c);
    SET_ONLY_C(svt_aom_highbd_dc_left_predictor_32x16, svt_aom_highbd_dc_left_predictor_32x16_c);
    SET_ONLY_C(svt_aom_highbd_dc_left_predictor_32x32, svt_aom_highbd_dc_left_predictor_32x32_c);
    SET_ONLY_C(svt_aom_highbd_dc_left_predictor_32x64, svt_aom_highbd_dc_left_predictor_32x64_c);
    SET_ONLY_C(svt_aom_highbd_dc_left_predictor_64x16, svt_aom_highbd_dc_left_predictor_64x16_c);
    SET_ONLY_C(svt_aom_highbd_dc_left_predictor_64x32, svt_aom_highbd_dc_left_predictor_64x32_c);
    SET_ONLY_C(svt_aom_highbd_dc_left_predictor_64x64, svt_aom_highbd_dc_left_predictor_64x64_c);

    SET_ONLY_C(svt_aom_highbd_dc_predictor_4x4, svt_aom_highbd_dc_predictor_4x4_c);
    SET_ONLY_C(svt_aom_highbd_dc_predictor_4x8, svt_aom_highbd_dc_predictor_4x8_c);
    SET_ONLY_C(svt_aom_highbd_dc_predictor_4x16, svt_aom_highbd_dc_predictor_4x16_c);
    SET_ONLY_C(svt_aom_highbd_dc_predictor_8x4, svt_aom_highbd_dc_predictor_8x4_c);
    SET_ONLY_C(svt_aom_highbd_dc_predictor_8x8, svt_aom_highbd_dc_predictor_8x8_c);
    SET_ONLY_C(svt_aom_highbd_dc_predictor_8x16, svt_aom_highbd_dc_predictor_8x16_c);
    SET_ONLY_C(svt_aom_highbd_dc_predictor_8x32, svt_aom_highbd_dc_predictor_8x32_c);
    SET_ONLY_C(svt_aom_highbd_dc_predictor_16x4, svt_aom_highbd_dc_predictor_16x4_c);
    SET_ONLY_C(svt_aom_highbd_dc_predictor_16x8, svt_aom_highbd_dc_predictor_16x8_c);
    SET_ONLY_C(svt_aom_highbd_dc_predictor_16x16, svt_aom_highbd_dc_predictor_16x16_c);
    SET_ONLY_C(svt_aom_highbd_dc_predictor_16x32, svt_aom_highbd_dc_predictor_16x32_c);
    SET_ONLY_C(svt_aom_highbd_dc_predictor_16x64, svt_aom_highbd_dc_predictor_16x64_c);
    SET_ONLY_C(svt_aom_highbd_dc_predictor_32x8, svt_aom_highbd_dc_predictor_32x8_c);
    SET_ONLY_C(svt_aom_highbd_dc_predictor_32x16, svt_aom_highbd_dc_predictor_32x16_c);
    SET_ONLY_C(svt_aom_highbd_dc_predictor_32x32, svt_aom_highbd_dc_predictor_32x32_c);
    SET_ONLY_C(svt_aom_highbd_dc_predictor_32x64, svt_aom_highbd_dc_predictor_32x64_c);
    SET_ONLY_C(svt_aom_highbd_dc_predictor_64x16, svt_aom_highbd_dc_predictor_64x16_c);
    SET_ONLY_C(svt_aom_highbd_dc_predictor_64x32, svt_aom_highbd_dc_predictor_64x32_c);
    SET_ONLY_C(svt_aom_highbd_dc_predictor_64x64, svt_aom_highbd_dc_predictor_64x64_c);

    //aom_highbd_dc_top_predictor
    SET_ONLY_C(svt_aom_highbd_dc_top_predictor_4x4, svt_aom_highbd_dc_top_predictor_4x4_c);
    SET_ONLY_C(svt_aom_highbd_dc_top_predictor_4x8, svt_aom_highbd_dc_top_predictor_4x8_c);
    SET_ONLY_C(svt_aom_highbd_dc_top_predictor_4x16, svt_aom_highbd_dc_top_predictor_4x16_c);
    SET_ONLY_C(svt_aom_highbd_dc_top_predictor_8x4, svt_aom_highbd_dc_top_predictor_8x4_c);
    SET_ONLY_C(svt_aom_highbd_dc_top_predictor_8x8, svt_aom_highbd_dc_top_predictor_8x8_c);
    SET_ONLY_C(svt_aom_highbd_dc_top_predictor_8x16, svt_aom_highbd_dc_top_predictor_8x16_c);
    SET_ONLY_C(svt_aom_highbd_dc_top_predictor_8x32, svt_aom_highbd_dc_top_predictor_8x32_c);
    SET_ONLY_C(svt_aom_highbd_dc_top_predictor_16x4, svt_aom_highbd_dc_top_predictor_16x4_c);
    SET_ONLY_C(svt_aom_highbd_dc_top_predictor_16x8, svt_aom_highbd_dc_top_predictor_16x8_c);
    SET_ONLY_C(svt_aom_highbd_dc_top_predictor_16x16, svt_aom_highbd_dc_top_predictor_16x16_c);
    SET_ONLY_C(svt_aom_highbd_dc_top_predictor_16x32, svt_aom_highbd_dc_top_predictor_16x32_c);
    SET_ONLY_C(svt_aom_highbd_dc_top_predictor_16x64, svt_aom_highbd_dc_top_predictor_16x64_c);
    SET_ONLY_C(svt_aom_highbd_dc_top_predictor_32x8, svt_aom_highbd_dc_top_predictor_32x8_c);
    SET_ONLY_C(svt_aom_highbd_dc_top_predictor_32x16, svt_aom_highbd_dc_top_predictor_32x16_c);
    SET_ONLY_C(svt_aom_highbd_dc_top_predictor_32x32, svt_aom_highbd_dc_top_predictor_32x32_c);
    SET_ONLY_C(svt_aom_highbd_dc_top_predictor_32x64, svt_aom_highbd_dc_top_predictor_32x64_c);
    SET_ONLY_C(svt_aom_highbd_dc_top_predictor_64x16, svt_aom_highbd_dc_top_predictor_64x16_c);
    SET_ONLY_C(svt_aom_highbd_dc_top_predictor_64x32, svt_aom_highbd_dc_top_predictor_64x32_c);
    SET_ONLY_C(svt_aom_highbd_dc_top_predictor_64x64, svt_aom_highbd_dc_top_predictor_64x64_c);

    // svt_aom_highbd_h_predictor
    SET_ONLY_C(svt_aom_highbd_h_predictor_4x4, svt_aom_highbd_h_predictor_4x4_c);
    SET_ONLY_C(svt_aom_highbd_h_predictor_4x8, svt_aom_highbd_h_predictor_4x8_c);
    SET_ONLY_C(svt_aom_highbd_h_predictor_4x16, svt_aom_highbd_h_predictor_4x16_c);
    SET_ONLY_C(svt_aom_highbd_h_predictor_8x4, svt_aom_highbd_h_predictor_8x4_c);
    SET_ONLY_C(svt_aom_highbd_h_predictor_8x8, svt_aom_highbd_h_predictor_8x8_c);
    SET_ONLY_C(svt_aom_highbd_h_predictor_8x16, svt_aom_highbd_h_predictor_8x16_c);
    SET_ONLY_C(svt_aom_highbd_h_predictor_8x32, svt_aom_highbd_h_predictor_8x32_c);
    SET_ONLY_C(svt_aom_highbd_h_predictor_16x4, svt_aom_highbd_h_predictor_16x4_c);
    SET_ONLY_C(svt_aom_highbd_h_predictor_16x8, svt_aom_highbd_h_predictor_16x8_c);
    SET_ONLY_C(svt_aom_highbd_h_predictor_16x16, svt_aom_highbd_h_predictor_16x16_c);
    SET_ONLY_C(svt_aom_highbd_h_predictor_16x32, svt_aom_highbd_h_predictor_16x32_c);
    SET_ONLY_C(svt_aom_highbd_h_predictor_16x64, svt_aom_highbd_h_predictor_16x64_c);
    SET_ONLY_C(svt_aom_highbd_h_predictor_32x8, svt_aom_highbd_h_predictor_32x8_c);
    SET_ONLY_C(svt_aom_highbd_h_predictor_32x16, svt_aom_highbd_h_predictor_32x16_c);
    SET_ONLY_C(svt_aom_highbd_h_predictor_32x32, svt_aom_highbd_h_predictor_32x32_c);
    SET_ONLY_C(svt_aom_highbd_h_predictor_32x64, svt_aom_highbd_h_predictor_32x64_c);
    SET_ONLY_C(svt_aom_highbd_h_predictor_64x16, svt_aom_highbd_h_predictor_64x16_c);
    SET_ONLY_C(svt_aom_highbd_h_predictor_64x32, svt_aom_highbd_h_predictor_64x32_c);
    SET_ONLY_C(svt_aom_highbd_h_predictor_64x64, svt_aom_highbd_h_predictor_64x64_c);
#endif
    SET_ONLY_C(svt_log2f, svt_aom_log2f_32);
    SET_ONLY_C(svt_av1_copy_wxh_8bit, svt_av1_copy_wxh_8bit_c);
    SET_ONLY_C(svt_av1_copy_wxh_16bit, svt_av1_copy_wxh_16bit_c);
    SET_ONLY_C(svt_memcpy, svt_memcpy_c);
    SET_ONLY_C(svt_memset, svt_memset_c);
    SET_ONLY_C(svt_aom_hadamard_32x32, svt_aom_hadamard_32x32_c);
    SET_ONLY_C(svt_aom_hadamard_16x16, svt_aom_hadamard_16x16_c);
    SET_ONLY_C(svt_aom_hadamard_8x8, svt_aom_hadamard_8x8_c);
    SET_ONLY_C(svt_aom_hadamard_4x4, svt_aom_hadamard_4x4_c);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    SET_ONLY_C(svt_aom_highbd_hadamard_8x8, svt_aom_highbd_hadamard_8x8_c);
#endif

#endif

    if(0 == flags)
    {
      (void) check_pointer_was_set;
    }
    (void)flags;

    svt_release_mutex(common_rtcd_init_mutex);
}

// clang-format on
