#pragma once
// Minimal llama.cpp ggml-cuda/common.cuh stand-in so fattn-mma-f16.cuh and
// mma.cuh compile inside the llama executor. Not the shared ggml CUDA tree.

#include <cuda_fp16.h>
#include <cuda_runtime.h>
#include <cfloat>
#include <cmath>
#include <climits>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <limits>
#include <type_traits>

#define WARP_SIZE 32
#define GGML_CUDA_CC_PASCAL          600
#define GGML_CUDA_CC_DP4A            610
#define GGML_CUDA_CC_VOLTA           700
#define GGML_CUDA_CC_TURING          750
#define GGML_CUDA_CC_AMPERE          800
#define GGML_CUDA_CC_ADA_LOVELACE    890
#define GGML_CUDA_CC_BLACKWELL       1200
#define GGML_CUDA_CC_RUBIN           1300
#define GGML_CUDA_CC_OFFSET_AMD      0x1000000
#define GGML_CUDA_CC_OFFSET_MTHREADS 0x0100000
#define GGML_CUDA_CC_IS_NVIDIA(cc)   ((cc) < GGML_CUDA_CC_OFFSET_MTHREADS)
#define GGML_CUDA_CC_GCN4       (GGML_CUDA_CC_OFFSET_AMD + 0x803)
#define GGML_CUDA_CC_VEGA       (GGML_CUDA_CC_OFFSET_AMD + 0x900)
#define GGML_CUDA_CC_VEGA20     (GGML_CUDA_CC_OFFSET_AMD + 0x906)
#define GGML_CUDA_CC_CDNA1      (GGML_CUDA_CC_OFFSET_AMD + 0x908)
#define GGML_CUDA_CC_CDNA2      (GGML_CUDA_CC_OFFSET_AMD + 0x910)
#define GGML_CUDA_CC_CDNA3      (GGML_CUDA_CC_OFFSET_AMD + 0x942)
#define GGML_CUDA_CC_RDNA1      (GGML_CUDA_CC_OFFSET_AMD + 0x1010)
#define GGML_CUDA_CC_RDNA2      (GGML_CUDA_CC_OFFSET_AMD + 0x1030)
#define GGML_CUDA_CC_RDNA3      (GGML_CUDA_CC_OFFSET_AMD + 0x1100)
#define GGML_CUDA_CC_RDNA3_5    (GGML_CUDA_CC_OFFSET_AMD + 0x1150)
#define GGML_CUDA_CC_RDNA4      (GGML_CUDA_CC_OFFSET_AMD + 0x1200)
#define GGML_CUDA_CC_IS_AMD(cc)      ((cc) >= GGML_CUDA_CC_OFFSET_AMD)
#define GGML_CUDA_CC_IS_RDNA(cc)     ((cc) >= GGML_CUDA_CC_RDNA1)
#define GGML_CUDA_CC_IS_RDNA1(cc)    ((cc) >= GGML_CUDA_CC_RDNA1 && (cc) < GGML_CUDA_CC_RDNA2)
#define GGML_CUDA_CC_IS_RDNA2(cc)    ((cc) >= GGML_CUDA_CC_RDNA2 && (cc) < GGML_CUDA_CC_RDNA3)
#define GGML_CUDA_CC_IS_RDNA3_0(cc)  ((cc) >= GGML_CUDA_CC_RDNA3 && (cc) < GGML_CUDA_CC_RDNA3_5)
#define GGML_CUDA_CC_IS_RDNA3_5(cc)  ((cc) >= GGML_CUDA_CC_RDNA3_5 && (cc) < GGML_CUDA_CC_RDNA4)
#define GGML_CUDA_CC_IS_RDNA3(cc)    (GGML_CUDA_CC_IS_RDNA3_0(cc) || GGML_CUDA_CC_IS_RDNA3_5(cc))
#define GGML_CUDA_CC_IS_RDNA4(cc)    ((cc) >= GGML_CUDA_CC_RDNA4)
#define GGML_CUDA_CC_IS_GCN(cc)      ((cc) > GGML_CUDA_CC_OFFSET_AMD && (cc) < GGML_CUDA_CC_CDNA1)
#define GGML_CUDA_CC_IS_CDNA(cc)     ((cc) >= GGML_CUDA_CC_CDNA1 && (cc) < GGML_CUDA_CC_RDNA1)
#define GGML_CUDA_CC_IS_CDNA1(cc)    ((cc) >= GGML_CUDA_CC_CDNA1 && (cc) < GGML_CUDA_CC_CDNA2)
#define GGML_CUDA_CC_IS_CDNA2(cc)    ((cc) >= GGML_CUDA_CC_CDNA2 && (cc) < GGML_CUDA_CC_CDNA3)
#define GGML_CUDA_CC_IS_CDNA3(cc)    ((cc) >= GGML_CUDA_CC_CDNA3 && (cc) < GGML_CUDA_CC_RDNA1)
#define GGML_CUDA_CC_IS_MTHREADS(cc) ((cc) >= GGML_CUDA_CC_OFFSET_MTHREADS && (cc) < GGML_CUDA_CC_OFFSET_AMD)
#define GGML_PAD(x, n) (((x) + (n) - 1) & ~((n) - 1))
#define GGML_CUDA_MAX_DEVICES 1
#define GGML_COMMON_DECL_CUDA
#define GGML_COMMON_IMPL_CUDA
#include "ggml-common.h"

#ifndef STRINGIZE
#define STRINGIZE_IMPL(x) #x
#define STRINGIZE(x) STRINGIZE_IMPL(x)
#endif

#define GGML_UNUSED(x) (void)(x)
template <typename... Args>
static __host__ __device__ __forceinline__ void mkllm_unused_vars(Args && ...) {}
#define GGML_UNUSED_VARS(...) mkllm_unused_vars(__VA_ARGS__)
#define GGML_ASSERT(x) do { if (!(x)) { printf("GGML_ASSERT failed: %s\n", #x); } } while (0)
#define GGML_ABORT(...) do { printf("GGML_ABORT %s\n", #__VA_ARGS__); } while (0)

// Host constexpr (launch_bounds) must see Ampere configs. Device code still
// gates on __CUDA_ARCH__ inside mma.cuh PTX.
#if !defined(GGML_USE_HIP) && (!defined(__CUDA_ARCH__) || __CUDA_ARCH__ >= GGML_CUDA_CC_TURING)
#define TURING_MMA_AVAILABLE
#endif
#if !defined(GGML_USE_HIP) && (!defined(__CUDA_ARCH__) || __CUDA_ARCH__ >= GGML_CUDA_CC_AMPERE)
#define AMPERE_MMA_AVAILABLE
#define CP_ASYNC_AVAILABLE
#endif
#if !defined(GGML_USE_HIP) && (!defined(__CUDA_ARCH__) || __CUDA_ARCH__ >= GGML_CUDA_CC_VOLTA)
#define FAST_FP16_AVAILABLE
#define FP16_AVAILABLE
#endif

enum ggml_type {
    GGML_TYPE_F32     = 0,
    GGML_TYPE_F16     = 1,
    GGML_TYPE_Q4_0    = 2,
    GGML_TYPE_Q4_1    = 3,
    GGML_TYPE_Q5_0    = 6,
    GGML_TYPE_Q5_1    = 7,
    GGML_TYPE_Q8_0    = 8,
    GGML_TYPE_Q8_1    = 9,
    GGML_TYPE_Q2_K    = 10,
    GGML_TYPE_Q3_K    = 11,
    GGML_TYPE_Q4_K    = 12,
    GGML_TYPE_Q5_K    = 13,
    GGML_TYPE_Q6_K    = 14,
    GGML_TYPE_Q8_K    = 15,
    GGML_TYPE_IQ2_XXS = 16,
    GGML_TYPE_IQ2_XS  = 17,
    GGML_TYPE_IQ3_XXS = 18,
    GGML_TYPE_IQ1_S   = 19,
    GGML_TYPE_IQ4_NL  = 20,
    GGML_TYPE_IQ3_S   = 21,
    GGML_TYPE_IQ2_S   = 22,
    GGML_TYPE_IQ4_XS  = 23,
    GGML_TYPE_I8      = 24,
    GGML_TYPE_I16     = 25,
    GGML_TYPE_I32     = 26,
    GGML_TYPE_I64     = 27,
    GGML_TYPE_F64     = 28,
    GGML_TYPE_IQ1_M   = 29,
    GGML_TYPE_BF16    = 30,
    GGML_TYPE_TQ1_0   = 34,
    GGML_TYPE_TQ2_0   = 35,
    GGML_TYPE_MXFP4   = 39,
    GGML_TYPE_NVFP4   = 40,
    GGML_TYPE_COUNT   = 41,
};

enum ggml_glu_op {
    GGML_GLU_OP_REGLU,
    GGML_GLU_OP_GEGLU,
    GGML_GLU_OP_SWIGLU,
    GGML_GLU_OP_SWIGLU_OAI,
    GGML_GLU_OP_GEGLU_ERF,
    GGML_GLU_OP_GEGLU_QUICK,
    GGML_GLU_OP_COUNT,
};

struct ggml_cuda_mm_fusion_args_device {
    const void * x_bias = nullptr;
    const void * gate = nullptr;
    const void * gate_bias = nullptr;
    ggml_glu_op glu_op;
};
#if defined(TURING_MMA_AVAILABLE)
#define LDMATRIX_TRANS_AVAILABLE
#endif
#define FLASH_ATTN_AVAILABLE

#ifdef __CUDA_ARCH__
[[noreturn]] static __device__ void no_device_code(
        const char * file_name, const int line, const char * function_name,
        const int arch, const char * arch_list) {
    printf("%s:%d: ERROR: CUDA kernel %s has no device code for arch %d (compiled %s)\n",
        file_name, line, function_name, arch, arch_list);
    __trap();
#if defined(GGML_USE_MUSA)
    __builtin_unreachable();
#endif
}
#define NO_DEVICE_CODE no_device_code(__FILE__, __LINE__, __FUNCTION__, __CUDA_ARCH__, STRINGIZE(__CUDA_ARCH__))
#else
#define NO_DEVICE_CODE
#endif

static constexpr __device__ int ggml_cuda_get_physical_warp_size() {
    return 32;
}

static constexpr __device__ int ggml_cuda_get_max_cpy_bytes() {
#if defined(__CUDA_ARCH__) && __CUDA_ARCH__ >= GGML_CUDA_CC_VOLTA
    return 16;
#else
    return 8;
#endif
}

template <int n>
struct ggml_cuda_unroll {
    template <typename Func, typename... Args>
    __device__ void operator()(const Func & f, Args... args) const {
        f(n - 1, args...);
        ggml_cuda_unroll<n - 1>{}(f, args...);
    }
};
template <>
struct ggml_cuda_unroll<1> {
    template <typename Func, typename... Args>
    __device__ void operator()(const Func & f, Args... args) const {
        f(0, args...);
    }
};

template <int nbytes, int alignment = 0>
static __device__ __forceinline__ void ggml_cuda_memcpy_1(void * __restrict__ dst, const void * __restrict__ src) {
    static_assert(nbytes <= 16 || alignment == 0, "bad ggml_cuda_memcpy_1");
    if constexpr (alignment != 0) {
        static_assert(nbytes % alignment == 0, "bad alignment");
    }
    constexpr int nb_per_cpy = alignment == 0 ? nbytes : alignment;
#pragma unroll
    for (int i = 0; i < nbytes / nb_per_cpy; ++i) {
        if constexpr (nb_per_cpy == 1) {
            ((char *) dst)[i] = ((const char *) src)[i];
        } else if constexpr (nb_per_cpy == 2) {
            ((short *) dst)[i] = ((const short *) src)[i];
        } else if constexpr (nb_per_cpy == 4) {
            ((int *) dst)[i] = ((const int *) src)[i];
        } else if constexpr (nb_per_cpy == 8) {
            ((int2 *) dst)[i] = ((const int2 *) src)[i];
        } else if constexpr (nb_per_cpy == 16) {
            ((int4 *) dst)[i] = ((const int4 *) src)[i];
        }
    }
}

static inline uint3 init_fastdiv_values(uint64_t d_64) {
    uint32_t d = (uint32_t) d_64;
    if (d == 0) {
        return make_uint3(0, 0, 0);
    }
    uint32_t L = 0;
    while (L < 32 && (uint32_t{1} << L) < d) {
        L++;
    }
    uint32_t mp = (uint32_t) ((uint64_t{1} << 32) * ((uint64_t{1} << L) - d) / d + 1);
    return make_uint3(mp, L, d);
}

static __device__ __forceinline__ uint32_t fastdiv(uint32_t n, const uint3 fastdiv_values) {
    const uint32_t hi = __umulhi(n, fastdiv_values.x);
    return (hi + n) >> fastdiv_values.y;
}

static __device__ __forceinline__ uint32_t fastmodulo(uint32_t n, const uint3 fastdiv_values) {
    return n - fastdiv(n, fastdiv_values) * fastdiv_values.z;
}

static __device__ __forceinline__ float get_alibi_slope(
        const float max_bias, const uint32_t h, const uint32_t n_head_log2,
        const float m0, const float m1) {
    if (max_bias <= 0.0f) {
        return 1.0f;
    }
    const float base = h < n_head_log2 ? m0 : m1;
    const int exph = h < n_head_log2 ? (int) h + 1 : 2 * ((int) h - (int) n_head_log2) + 1;
    return powf(base, (float) exph);
}

static inline bool turing_mma_available(const int cc) {
    return GGML_CUDA_CC_IS_NVIDIA(cc) && cc >= GGML_CUDA_CC_TURING;
}
static inline bool ampere_mma_available(const int cc) {
    return GGML_CUDA_CC_IS_NVIDIA(cc) && cc >= GGML_CUDA_CC_AMPERE;
}
static inline bool cp_async_available(const int cc) {
    return GGML_CUDA_CC_IS_NVIDIA(cc) && cc >= GGML_CUDA_CC_AMPERE;
}
static inline bool amd_wmma_available(const int) { return false; }
static inline bool amd_mfma_available(const int) { return false; }
static inline bool volta_mma_available(const int cc) { return cc == GGML_CUDA_CC_VOLTA; }

static inline int ggml_cuda_highest_compiled_arch(const int cc) {
    return cc;
}

#define CUDA_CHECK(err) do { \
    cudaError_t err_ = (err); \
    if (err_ != cudaSuccess) { \
        printf("CUDA_CHECK %s\n", cudaGetErrorString(err_)); \
    } \
} while (0)

// llama.cpp common.cuh ggml_cuda_mad / warp_reduce (fattn-vec.cuh).
static __device__ __forceinline__ void ggml_cuda_mad(float & acc, const float v, const float u) {
    acc += v * u;
}
static __device__ __forceinline__ void ggml_cuda_mad(float & acc, const float2 v, const float2 u) {
    acc += v.x * u.x;
    acc += v.y * u.y;
}
static __device__ __forceinline__ void ggml_cuda_mad(float & acc, const half2 v, const half2 u) {
#ifdef FAST_FP16_AVAILABLE
    const float2 tmp = __half22float2(v * u);
    acc += tmp.x + tmp.y;
#else
    const float2 tmpv = __half22float2(v);
    const float2 tmpu = __half22float2(u);
    acc += tmpv.x * tmpu.x;
    acc += tmpv.y * tmpu.y;
#endif
}
static __device__ __forceinline__ void ggml_cuda_mad(half2 & acc, const half2 v, const half2 u) {
#ifdef FAST_FP16_AVAILABLE
    acc += v * u;
#else
    const float2 tmpv = __half22float2(v);
    const float2 tmpu = __half22float2(u);
    float2 tmpacc = __half22float2(acc);
    tmpacc.x += tmpv.x * tmpu.x;
    tmpacc.y += tmpv.y * tmpu.y;
    acc = make_half2(tmpacc.x, tmpacc.y);
#endif
}

template <int width = WARP_SIZE>
static __device__ __forceinline__ float warp_reduce_sum(float x) {
#pragma unroll
    for (int offset = width / 2; offset > 0; offset >>= 1) {
        x += __shfl_xor_sync(0xffffffff, x, offset, width);
    }
    return x;
}
template <int width = WARP_SIZE>
static __device__ __forceinline__ float warp_reduce_max(float x) {
#pragma unroll
    for (int offset = width / 2; offset > 0; offset >>= 1) {
        x = fmaxf(x, __shfl_xor_sync(0xffffffff, x, offset, width));
    }
    return x;
}

// llama.cpp common.cuh:592 block_reduce SUM, float only. Used by official
// rms_norm_f32 (norm.cu:134).
template <int block_size>
static __device__ __forceinline__ float block_reduce_sum(float val, float * shared_vals) {
    val = warp_reduce_sum(val);
    if (block_size > WARP_SIZE) {
        const int warp_id = threadIdx.x / WARP_SIZE;
        const int lane_id = threadIdx.x % WARP_SIZE;
        if (lane_id == 0) {
            shared_vals[warp_id] = val;
        }
        __syncthreads();
        val = 0.0f;
        if (lane_id < (block_size / WARP_SIZE)) {
            val = shared_vals[lane_id];
        }
        val = warp_reduce_sum(val);
    }
    return val;
}

static __device__ __forceinline__ int ggml_cuda_dp4a(const int a, const int b, int c) {
#if defined(__CUDA_ARCH__) && __CUDA_ARCH__ >= GGML_CUDA_CC_DP4A
    return __dp4a(a, b, c);
#else
    const int8_t * a8 = (const int8_t *) &a;
    const int8_t * b8 = (const int8_t *) &b;
    return c + a8[0]*b8[0] + a8[1]*b8[1] + a8[2]*b8[2] + a8[3]*b8[3];
#endif
}

static __device__ __forceinline__ float ggml_cuda_e8m0_to_fp32(uint8_t x) {
    uint32_t bits;
    if (x == 0) {
        bits = 0x00400000;
    } else {
        bits = (uint32_t) x << 23;
    }
    float result;
    memcpy(&result, &bits, sizeof(float));
    return result;
}

static __device__ __forceinline__ float ggml_cuda_ue4m3_to_fp32(uint8_t x) {
    if (x == 0 || x == 0x7F || x == 0xFF) {
        return 0.0f;
    }
    const int exp = (x >> 3) & 0xF;
    const int man = x & 0x7;
    float raw;
    if (exp == 0) {
        raw = ldexpf((float) man, -9);
    } else {
        raw = ldexpf(1.0f + (float) man / 8.0f, exp - 7);
    }
    return raw / 2.0f;
}

static __device__ __forceinline__ float ggml_cuda_op_silu_single(float x) {
    return x / (1.0f + expf(-x));
}

static __device__ __forceinline__ float ggml_cuda_op_gelu_single(float x) {
    const float GELU_COEF_A    = 0.044715f;
    const float SQRT_2_OVER_PI = 0.79788456080286535587989211986876f;
    return 0.5f * x * (1.0f + tanhf(SQRT_2_OVER_PI * x * (1.0f + GELU_COEF_A * x * x)));
}

static __device__ __forceinline__ float ggml_cuda_op_swiglu_oai_single(
        float x, float g, float alpha = 1.702f, float limit = 7.0f) {
    x = fminf(x, limit);
    g = fmaxf(fminf(g, limit), -limit);
    float out_glu = x / (1.0f + expf(-x * alpha));
    return out_glu * (1.0f + g);
}

template <ggml_type type>
struct ggml_cuda_type_traits;

template<> struct ggml_cuda_type_traits<GGML_TYPE_F16> {
    static constexpr int qk = 1;
    static constexpr int qr = 1;
};
template<> struct ggml_cuda_type_traits<GGML_TYPE_Q4_0> {
    static constexpr int qk = QK4_0; static constexpr int qr = QR4_0; static constexpr int qi = QI4_0;
};
template<> struct ggml_cuda_type_traits<GGML_TYPE_Q4_1> {
    static constexpr int qk = QK4_1; static constexpr int qr = QR4_1; static constexpr int qi = QI4_1;
};
template<> struct ggml_cuda_type_traits<GGML_TYPE_Q5_0> {
    static constexpr int qk = QK5_0; static constexpr int qr = QR5_0; static constexpr int qi = QI5_0;
};
template<> struct ggml_cuda_type_traits<GGML_TYPE_Q5_1> {
    static constexpr int qk = QK5_1; static constexpr int qr = QR5_1; static constexpr int qi = QI5_1;
};
template<> struct ggml_cuda_type_traits<GGML_TYPE_Q8_0> {
    static constexpr int qk = QK8_0; static constexpr int qr = QR8_0; static constexpr int qi = QI8_0;
};
template<> struct ggml_cuda_type_traits<GGML_TYPE_MXFP4> {
    static constexpr int qk = QK_MXFP4; static constexpr int qr = QR_MXFP4; static constexpr int qi = QI_MXFP4;
};
template<> struct ggml_cuda_type_traits<GGML_TYPE_NVFP4> {
    static constexpr int qk = QK_NVFP4; static constexpr int qr = QR_NVFP4; static constexpr int qi = QI_NVFP4;
};
template<> struct ggml_cuda_type_traits<GGML_TYPE_Q2_K> {
    static constexpr int qk = QK_K; static constexpr int qr = QR2_K; static constexpr int qi = QI2_K;
};
template<> struct ggml_cuda_type_traits<GGML_TYPE_Q3_K> {
    static constexpr int qk = QK_K; static constexpr int qr = QR3_K; static constexpr int qi = QI3_K;
};
template<> struct ggml_cuda_type_traits<GGML_TYPE_Q4_K> {
    static constexpr int qk = QK_K; static constexpr int qr = QR4_K; static constexpr int qi = QI4_K;
};
template<> struct ggml_cuda_type_traits<GGML_TYPE_Q5_K> {
    static constexpr int qk = QK_K; static constexpr int qr = QR5_K; static constexpr int qi = QI5_K;
};
template<> struct ggml_cuda_type_traits<GGML_TYPE_Q6_K> {
    static constexpr int qk = QK_K; static constexpr int qr = QR6_K; static constexpr int qi = QI6_K;
};
template<> struct ggml_cuda_type_traits<GGML_TYPE_IQ2_XXS> {
    static constexpr int qk = QK_K; static constexpr int qr = QR2_XXS; static constexpr int qi = QI2_XXS;
};
template<> struct ggml_cuda_type_traits<GGML_TYPE_IQ2_XS> {
    static constexpr int qk = QK_K; static constexpr int qr = QR2_XS; static constexpr int qi = QI2_XS;
};
template<> struct ggml_cuda_type_traits<GGML_TYPE_IQ2_S> {
    static constexpr int qk = QK_K; static constexpr int qr = QR2_S; static constexpr int qi = QI2_S;
};
template<> struct ggml_cuda_type_traits<GGML_TYPE_IQ3_XXS> {
    static constexpr int qk = QK_K; static constexpr int qr = QR3_XXS; static constexpr int qi = QI3_XXS;
};
template<> struct ggml_cuda_type_traits<GGML_TYPE_IQ1_S> {
    static constexpr int qk = QK_K; static constexpr int qr = QR1_S; static constexpr int qi = QI1_S;
};
template<> struct ggml_cuda_type_traits<GGML_TYPE_IQ1_M> {
    static constexpr int qk = QK_K; static constexpr int qr = QR1_M; static constexpr int qi = QI1_M;
};
template<> struct ggml_cuda_type_traits<GGML_TYPE_IQ4_NL> {
    static constexpr int qk = QK4_NL; static constexpr int qr = QR4_NL; static constexpr int qi = QI4_NL;
};
template<> struct ggml_cuda_type_traits<GGML_TYPE_IQ4_XS> {
    static constexpr int qk = QK_K; static constexpr int qr = QR4_XS; static constexpr int qi = QI4_XS;
};
template<> struct ggml_cuda_type_traits<GGML_TYPE_IQ3_S> {
    static constexpr int qk = QK_K; static constexpr int qr = QR3_S; static constexpr int qi = QI3_S;
};
