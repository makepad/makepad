#include <cuda_fp16.h>
#include <cuda_runtime.h>
#include <stdint.h>

static constexpr int WARP_SIZE = 32;
static constexpr uint32_t QK8_1 = 32;
static constexpr uint32_t QK_NVFP4 = 64;
static constexpr uint32_t QK_NVFP4_SUB = 16;
static constexpr uint32_t QI8_0 = 8;
static constexpr uint32_t QI8_1 = 8;
static constexpr uint32_t QI_NVFP4 = 8;
static constexpr int CUDA_QUANTIZE_BLOCK_SIZE_MMQ = 128;
static constexpr int MMQ_TILE_NE_K = 32;
static constexpr int MMQ_TILE_Y_K = MMQ_TILE_NE_K + MMQ_TILE_NE_K / QI8_1;
static constexpr int MMQ_ITER_K = 256;
static constexpr int MMQ_NWARPS = 8;
static constexpr int MMQ_WARP_THREADS = WARP_SIZE * MMQ_NWARPS;
static constexpr int MMQ_Y = 128;
static constexpr int MMQ_MMA_TILE_X_K_NVFP4 = 2 * MMQ_TILE_NE_K + MMQ_TILE_NE_K / 2 + 4;
static constexpr int BLOCK_Q8_1_MMQ_INTS = 36;

typedef struct {
    uint16_t d;
    uint16_t s;
    int8_t qs[QK8_1];
} __align__(4) block_q8_1;

typedef struct {
    union {
        float d4[4];
    };
    int8_t qs[4 * QK8_1];
} __align__(4) block_q8_1_mmq;

typedef struct {
    uint8_t d[QK_NVFP4 / QK_NVFP4_SUB];
    uint8_t qs[QK_NVFP4 / 2];
} __align__(4) block_nvfp4;

static_assert(sizeof(block_q8_1) == 36, "wrong q8_1 block size");
static_assert(sizeof(block_q8_1_mmq) == 4 * sizeof(block_q8_1), "wrong q8_1 mmq block size");
static_assert(sizeof(block_nvfp4) == 36, "wrong nvfp4 block size");
static_assert(BLOCK_Q8_1_MMQ_INTS == static_cast<int>(sizeof(block_q8_1_mmq) / sizeof(int)), "wrong q8_1 mmq int size");

__device__ __constant__ int8_t KVALUES_MXFP4_X2[16] = {
    0, 1, 2, 3, 4, 6, 8, 12, 0, -1, -2, -3, -4, -6, -8, -12,
};

template <int WIDTH>
static __device__ __forceinline__ float makepad_ggml_cuda_warp_reduce_sum(float x) {
#pragma unroll
    for (int offset = WIDTH / 2; offset > 0; offset >>= 1) {
        x += __shfl_xor_sync(0xffffffffu, x, offset, WIDTH);
    }
    return x;
}

template <int WIDTH>
static __device__ __forceinline__ float makepad_ggml_cuda_warp_reduce_max(float x) {
#pragma unroll
    for (int offset = WIDTH / 2; offset > 0; offset >>= 1) {
        x = fmaxf(x, __shfl_xor_sync(0xffffffffu, x, offset, WIDTH));
    }
    return x;
}

static __device__ __forceinline__ float makepad_ggml_cuda_ue4m3_to_fp32(uint8_t x) {
    if (x == 0 || x == 0x7F || x == 0xFF) {
        return 0.0f;
    }
    const int exp = (x >> 3) & 0xF;
    const int man = x & 0x7;
    const float raw = exp == 0 ? ldexpf((float) man, -9) : ldexpf(1.0f + (float) man / 8.0f, exp - 7);
    return raw * 0.5f;
}

static __device__ __forceinline__ int makepad_ggml_cuda_dp4a(const int a, const int b, int c) {
#if __CUDA_ARCH__ >= 610
    return __dp4a(a, b, c);
#else
    const int8_t * a8 = reinterpret_cast<const int8_t *>(&a);
    const int8_t * b8 = reinterpret_cast<const int8_t *>(&b);
    return c + a8[0] * b8[0] + a8[1] * b8[1] + a8[2] * b8[2] + a8[3] * b8[3];
#endif
}

static __device__ __forceinline__ int2 makepad_ggml_cuda_get_int_from_table_16(
        const int q4,
        const int8_t * table) {
    const uint32_t * table32 = reinterpret_cast<const uint32_t *>(table);
    uint32_t tmp[2];
    const uint32_t low_high_selection_indices = 0x32103210u | ((static_cast<uint32_t>(q4) & 0x88888888u) >> 1);

#pragma unroll
    for (uint32_t i = 0; i < 2; ++i) {
        const uint32_t shift = 16u * i;
        const uint32_t low = __byte_perm(table32[0], table32[1], static_cast<uint32_t>(q4) >> shift);
        const uint32_t high = __byte_perm(table32[2], table32[3], static_cast<uint32_t>(q4) >> shift);
        tmp[i] = __byte_perm(low, high, low_high_selection_indices >> shift);
    }

    return make_int2(
        __byte_perm(tmp[0], tmp[1], 0x6420),
        __byte_perm(tmp[0], tmp[1], 0x7531));
}

template <int vdr>
static __device__ __forceinline__ float makepad_ggml_cuda_vec_dot_q8_0_16_q8_1_impl(
        const int * v,
        const int * u,
        const float * d8_0,
        const float d8_1) {
    float sumf = 0.0f;

#pragma unroll
    for (int i0 = 0; i0 < vdr; i0 += QI8_0 / 2) {
        int sumi = 0;

#pragma unroll
        for (int i = i0; i < i0 + QI8_0 / 2; ++i) {
            sumi = makepad_ggml_cuda_dp4a(v[i], u[i], sumi);
        }

        sumf += d8_0[i0 / (QI8_0 / 2)] * static_cast<float>(sumi);
    }

    return d8_1 * sumf;
}

static __host__ __device__ __forceinline__ int makepad_ggml_cuda_pad(int value, int align) {
    return ((value + align - 1) / align) * align;
}

template <int NBYTES>
static __device__ __forceinline__ void makepad_ggml_cuda_memcpy_1(
        void * __restrict__ dst,
        const void * __restrict__ src) {
#pragma unroll
    for (int i = 0; i < NBYTES / 4; ++i) {
        reinterpret_cast<int *>(dst)[i] = reinterpret_cast<const int *>(src)[i];
    }
}

template <int I, int J>
struct makepad_ggml_cuda_mma_tile_int;

template <>
struct makepad_ggml_cuda_mma_tile_int<8, 4> {
    static constexpr int I_VALUE = 8;
    static constexpr int J_VALUE = 4;
    static constexpr int ne = I_VALUE * J_VALUE / WARP_SIZE;
    int x[ne] = {0};

    static __device__ __forceinline__ int get_i(int) {
        return threadIdx.x / 4;
    }

    static __device__ __forceinline__ int get_j(int) {
        return threadIdx.x % 4;
    }
};

template <>
struct makepad_ggml_cuda_mma_tile_int<16, 4> {
    static constexpr int I_VALUE = 16;
    static constexpr int J_VALUE = 4;
    static constexpr int ne = I_VALUE * J_VALUE / WARP_SIZE;
    int x[ne] = {0};

    static __device__ __forceinline__ int get_i(int l) {
        return l * 8 + threadIdx.x / 4;
    }

    static __device__ __forceinline__ int get_j(int) {
        return threadIdx.x % 4;
    }
};

template <>
struct makepad_ggml_cuda_mma_tile_int<16, 8> {
    static constexpr int I_VALUE = 16;
    static constexpr int J_VALUE = 8;
    static constexpr int ne = I_VALUE * J_VALUE / WARP_SIZE;
    int x[ne] = {0};

    static __device__ __forceinline__ int get_i(int l) {
        return (l / 2) * 8 + threadIdx.x / 4;
    }

    static __device__ __forceinline__ int get_j(int l) {
        return (threadIdx.x % 4) * 2 + (l % 2);
    }
};

template <int I, int J>
static __device__ __forceinline__ void makepad_ggml_cuda_load_generic(
        makepad_ggml_cuda_mma_tile_int<I, J> & tile,
        const int * __restrict__ src,
        int stride) {
#pragma unroll
    for (int l = 0; l < tile.ne; ++l) {
        tile.x[l] = src[tile.get_i(l) * stride + tile.get_j(l)];
    }
}

static __device__ __forceinline__ void makepad_ggml_cuda_load_ldmatrix(
        makepad_ggml_cuda_mma_tile_int<16, 4> & tile,
        const int * __restrict__ src,
        int stride) {
#if __CUDA_ARCH__ >= 750
    const int * xs = src + (threadIdx.x % tile.I_VALUE) * stride;
    asm volatile("ldmatrix.sync.aligned.m8n8.x2.b16 {%0, %1}, [%2];"
        : "=r"(tile.x[0]), "=r"(tile.x[1])
        : "l"(xs));
#else
    makepad_ggml_cuda_load_generic(tile, src, stride);
#endif
}

static __device__ __forceinline__ void makepad_ggml_cuda_load_ldmatrix(
        makepad_ggml_cuda_mma_tile_int<16, 8> & tile,
        const int * __restrict__ src,
        int stride) {
#if __CUDA_ARCH__ >= 750
    const int * xs =
        src + (threadIdx.x % tile.I_VALUE) * stride + (threadIdx.x / tile.I_VALUE) * (tile.J_VALUE / 2);
    asm volatile("ldmatrix.sync.aligned.m8n8.x4.b16 {%0, %1, %2, %3}, [%4];"
        : "=r"(tile.x[0]), "=r"(tile.x[1]), "=r"(tile.x[2]), "=r"(tile.x[3])
        : "l"(xs));
#else
    makepad_ggml_cuda_load_generic(tile, src, stride);
#endif
}

static __device__ __forceinline__ void makepad_ggml_cuda_mma(
        makepad_ggml_cuda_mma_tile_int<16, 8> & d,
        const makepad_ggml_cuda_mma_tile_int<16, 4> & a,
        const makepad_ggml_cuda_mma_tile_int<8, 4> & b) {
#if __CUDA_ARCH__ >= 800
    asm("mma.sync.aligned.m16n8k16.row.col.s32.s8.s8.s32 {%0, %1, %2, %3}, {%4, %5}, {%6}, {%0, %1, %2, %3};"
        : "+r"(d.x[0]), "+r"(d.x[1]), "+r"(d.x[2]), "+r"(d.x[3])
        : "r"(a.x[0]), "r"(a.x[1]), "r"(b.x[0]));
#elif __CUDA_ARCH__ >= 750
    asm("mma.sync.aligned.m8n8k16.row.col.s32.s8.s8.s32 {%0, %1}, {%2}, {%3}, {%0, %1};"
        : "+r"(d.x[0]), "+r"(d.x[1])
        : "r"(a.x[0]), "r"(b.x[0]));
    asm("mma.sync.aligned.m8n8k16.row.col.s32.s8.s8.s32 {%0, %1}, {%2}, {%3}, {%0, %1};"
        : "+r"(d.x[2]), "+r"(d.x[3])
        : "r"(a.x[1]), "r"(b.x[0]));
#endif
}

static __global__ void makepad_ggml_cuda_quantize_q8_1_mmq_f32_kernel(
        const float * __restrict__ input_f32,
        block_q8_1_mmq * __restrict__ output_q8_1_mmq,
        uint32_t n_cols,
        uint32_t n_rows) {
    const int64_t i0 = (static_cast<int64_t>(blockDim.x) * blockIdx.y + threadIdx.x) * 4;

    if (i0 >= n_cols || blockIdx.x >= n_rows) {
        return;
    }

    const int64_t row = blockIdx.x;
    const int64_t ib = (i0 / (4 * QK8_1)) * n_rows + row;
    const int64_t iqs = i0 % (4 * QK8_1);

    const float4 * input_f32x4 = reinterpret_cast<const float4 *>(input_f32);
    const float4 xi = i0 < n_cols
        ? input_f32x4[(row * n_cols + i0) / 4]
        : make_float4(0.0f, 0.0f, 0.0f, 0.0f);

    float amax = fabsf(xi.x);
    amax = fmaxf(amax, fabsf(xi.y));
    amax = fmaxf(amax, fabsf(xi.z));
    amax = fmaxf(amax, fabsf(xi.w));

#pragma unroll
    for (int offset = 4; offset > 0; offset >>= 1) {
        amax = fmaxf(amax, __shfl_xor_sync(0xFFFFFFFFu, amax, offset, WARP_SIZE));
    }

    const float d_inv = amax == 0.0f ? 0.0f : 127.0f / amax;
    char4 q;
    q.x = static_cast<int8_t>(roundf(xi.x * d_inv));
    q.y = static_cast<int8_t>(roundf(xi.y * d_inv));
    q.z = static_cast<int8_t>(roundf(xi.z * d_inv));
    q.w = static_cast<int8_t>(roundf(xi.w * d_inv));

    char4 * output_qs4 = reinterpret_cast<char4 *>(output_q8_1_mmq[ib].qs);
    output_qs4[iqs / 4] = q;

    if (iqs % QK8_1 != 0) {
        return;
    }

    output_q8_1_mmq[ib].d4[iqs / QK8_1] = amax / 127.0f;
}

template <bool need_check>
static __device__ __forceinline__ void makepad_ggml_cuda_load_tiles_nvfp4(
        const block_nvfp4 * __restrict__ weights_nvfp4,
        int * __restrict__ x_tile,
        const int kb0,
        const int i_max,
        const int stride) {
    int * x_qs = x_tile;
#if __CUDA_ARCH__ >= 750
    float * x_df = reinterpret_cast<float *>(x_qs + MMQ_TILE_NE_K * 2);
#else
    float * x_df = reinterpret_cast<float *>(x_qs + MMQ_Y * MMQ_TILE_NE_K * 2 + MMQ_Y);
#endif

    constexpr int threads_per_row = MMQ_ITER_K / QK_NVFP4;
    constexpr int rows_per_warp = WARP_SIZE / threads_per_row;
    const int kbx = threadIdx.x % threads_per_row;
    const int row_in_warp = threadIdx.x / threads_per_row;

#pragma unroll
    for (int i0 = 0; i0 < MMQ_Y; i0 += rows_per_warp * MMQ_NWARPS) {
        int i = i0 + threadIdx.y * rows_per_warp + row_in_warp;

        if constexpr (need_check) {
            i = min(i, i_max);
        }

        const block_nvfp4 * block = weights_nvfp4 + kb0 + i * stride + kbx;
        const uint32_t * src_qs = reinterpret_cast<const uint32_t *>(block->qs);
        const int kqs = 16 * kbx;
        const int ksc = 4 * kbx;

#pragma unroll
        for (int sub = 0; sub < static_cast<int>(QK_NVFP4 / QK_NVFP4_SUB); ++sub) {
            const int2 q0 = makepad_ggml_cuda_get_int_from_table_16(
                static_cast<int>(src_qs[2 * sub + 0]),
                KVALUES_MXFP4_X2);
            const int2 q1 = makepad_ggml_cuda_get_int_from_table_16(
                static_cast<int>(src_qs[2 * sub + 1]),
                KVALUES_MXFP4_X2);

#if __CUDA_ARCH__ >= 750
            x_qs[i * MMQ_MMA_TILE_X_K_NVFP4 + kqs + 4 * sub + 0] = q0.x;
            x_qs[i * MMQ_MMA_TILE_X_K_NVFP4 + kqs + 4 * sub + 1] = q1.x;
            x_qs[i * MMQ_MMA_TILE_X_K_NVFP4 + kqs + 4 * sub + 2] = q0.y;
            x_qs[i * MMQ_MMA_TILE_X_K_NVFP4 + kqs + 4 * sub + 3] = q1.y;
            x_df[i * MMQ_MMA_TILE_X_K_NVFP4 + ksc + sub] =
                makepad_ggml_cuda_ue4m3_to_fp32(block->d[sub]);
#else
            x_qs[i * (2 * MMQ_TILE_NE_K + 1) + kqs + 4 * sub + 0] = q0.x;
            x_qs[i * (2 * MMQ_TILE_NE_K + 1) + kqs + 4 * sub + 1] = q1.x;
            x_qs[i * (2 * MMQ_TILE_NE_K + 1) + kqs + 4 * sub + 2] = q0.y;
            x_qs[i * (2 * MMQ_TILE_NE_K + 1) + kqs + 4 * sub + 3] = q1.y;
            x_df[i * (2 * MMQ_TILE_NE_K * 2 / QI_NVFP4) + i / (QK_NVFP4_SUB / QI_NVFP4) + ksc + sub] =
                makepad_ggml_cuda_ue4m3_to_fp32(block->d[sub]);
#endif
        }
    }
}

template <int mmq_x>
static __device__ __forceinline__ void makepad_ggml_cuda_vec_dot_q8_0_16_q8_1_dp4a(
        const int * __restrict__ x,
        const int * __restrict__ y,
        float * __restrict__ sum,
        const int k00) {
    const int * x_qs = x;
    const float * x_df = reinterpret_cast<const float *>(x_qs + MMQ_Y * MMQ_TILE_NE_K * 2 + MMQ_Y);
    const int * y_qs = y + 4;
    const float * y_df = reinterpret_cast<const float *>(y);

    for (int k01 = 0; k01 < MMQ_TILE_NE_K; k01 += QI8_0) {
        const int k0 = k00 + k01;

#pragma unroll
        for (int j0 = 0; j0 < mmq_x; j0 += MMQ_NWARPS) {
            const int j = j0 + threadIdx.y;

#pragma unroll
            for (int i0 = 0; i0 < MMQ_Y; i0 += WARP_SIZE) {
                const int i = i0 + threadIdx.x;

                sum[j0 / MMQ_NWARPS * (MMQ_Y / WARP_SIZE) + i0 / WARP_SIZE] +=
                    makepad_ggml_cuda_vec_dot_q8_0_16_q8_1_impl<QI8_0>(
                        &x_qs[i * (2 * MMQ_TILE_NE_K + 1) + k0],
                        &y_qs[j * MMQ_TILE_Y_K + k01],
                        &x_df[i * (2 * MMQ_TILE_NE_K * 2 / QI8_0) + i / (QI8_0 / 4) + k0 / (QI8_0 / 2)],
                        y_df[j * MMQ_TILE_Y_K + k01 / QI8_1]);
            }
        }
    }
}

template <int mmq_x>
static __device__ __forceinline__ void makepad_ggml_cuda_vec_dot_q8_0_16_q8_1_mma(
        const int * __restrict__ x,
        const int * __restrict__ y,
        float * __restrict__ sum,
        const int k00) {
#if __CUDA_ARCH__ >= 750
    using tile_a = makepad_ggml_cuda_mma_tile_int<16, 4>;
    using tile_a8 = makepad_ggml_cuda_mma_tile_int<16, 8>;
    using tile_b = makepad_ggml_cuda_mma_tile_int<8, 4>;
    using tile_c = makepad_ggml_cuda_mma_tile_int<16, 8>;

    constexpr int granularity = 8;
    constexpr int rows_per_warp = 2 * granularity;
    constexpr int ntx = rows_per_warp / tile_c::I_VALUE;

    const int * x_qs = x;
    const float * x_df = reinterpret_cast<const float *>(x_qs) + 2 * MMQ_TILE_NE_K;
    const int * y_qs = y + 4;
    const float * y_df = reinterpret_cast<const float *>(y);
    const int i0 = (threadIdx.y / ntx) * (ntx * tile_a::I_VALUE);

    tile_a a[ntx][8];
    float d_a[ntx][tile_c::ne / 2][8];

#pragma unroll
    for (int n = 0; n < ntx; ++n) {
#pragma unroll
        for (int k01 = 0; k01 < MMQ_TILE_NE_K; k01 += 8) {
            const int k0 = k00 + k01;
            makepad_ggml_cuda_load_ldmatrix(
                reinterpret_cast<tile_a8 *>(a[n])[k01 / 8],
                x_qs + (i0 + n * tile_a::I_VALUE) * MMQ_MMA_TILE_X_K_NVFP4 + k0,
                MMQ_MMA_TILE_X_K_NVFP4);
        }

#pragma unroll
        for (int l = 0; l < tile_c::ne / 2; ++l) {
            const int i = i0 + n * tile_c::I_VALUE + tile_c::get_i(2 * l);

#pragma unroll
            for (int k01 = 0; k01 < MMQ_TILE_NE_K; k01 += 4) {
                const int k0 = k00 + k01;
                d_a[n][l][k01 / 4] = x_df[i * MMQ_MMA_TILE_X_K_NVFP4 + k0 / 4];
            }
        }
    }

#pragma unroll
    for (int j0 = 0; j0 < mmq_x; j0 += ntx * tile_c::J_VALUE) {
#pragma unroll
        for (int k01 = 0; k01 < MMQ_TILE_NE_K; k01 += 8) {
            tile_b b[2];
            float d_b[tile_c::ne / 2];

            makepad_ggml_cuda_load_generic(
                b[0],
                y_qs + j0 * MMQ_TILE_Y_K + k01,
                MMQ_TILE_Y_K);
            makepad_ggml_cuda_load_generic(
                b[1],
                y_qs + j0 * MMQ_TILE_Y_K + tile_b::J_VALUE + k01,
                MMQ_TILE_Y_K);

#pragma unroll
            for (int l = 0; l < tile_c::ne / 2; ++l) {
                const int j = j0 + tile_c::get_j(l);
                d_b[l] = y_df[j * MMQ_TILE_Y_K + k01 / QI8_1];
            }

#pragma unroll
            for (int n = 0; n < ntx; ++n) {
                tile_c c[2];
                makepad_ggml_cuda_mma(c[0], a[n][k01 / 4 + 0], b[0]);
                makepad_ggml_cuda_mma(c[1], a[n][k01 / 4 + 1], b[1]);

#pragma unroll
                for (int l = 0; l < tile_c::ne; ++l) {
                    sum[(j0 / tile_c::J_VALUE + n) * tile_c::ne + l] +=
                        d_b[l % 2] *
                        (c[0].x[l] * d_a[n][l / 2][k01 / 4 + 0] +
                         c[1].x[l] * d_a[n][l / 2][k01 / 4 + 1]);
                }
            }
        }
    }
#else
    (void) x;
    (void) y;
    (void) sum;
    (void) k00;
#endif
}

template <int mmq_x, bool need_check>
static __device__ __forceinline__ void makepad_ggml_cuda_mmq_write_back(
        const float * __restrict__ sum,
        float * __restrict__ dst,
        const int stride,
        const int i_max,
        const int j_max) {
#pragma unroll
    for (int j0 = 0; j0 < mmq_x; j0 += MMQ_NWARPS) {
        const int j = j0 + threadIdx.y;

        if (j > j_max) {
            return;
        }

#pragma unroll
        for (int i0 = 0; i0 < MMQ_Y; i0 += WARP_SIZE) {
            const int i = i0 + threadIdx.x;

            if constexpr (need_check) {
                if (i > i_max) {
                    continue;
                }
            }

            dst[j * stride + i] = sum[(j0 / MMQ_NWARPS) * (MMQ_Y / WARP_SIZE) + i0 / WARP_SIZE];
        }
    }
}

template <int mmq_x, bool need_check>
static __device__ __forceinline__ void makepad_ggml_cuda_mmq_write_back_mma(
        const float * __restrict__ sum,
        float * __restrict__ dst,
        const int stride,
        const int i_max,
        const int j_max) {
#if __CUDA_ARCH__ >= 750
    using tile_c = makepad_ggml_cuda_mma_tile_int<16, 8>;

#pragma unroll
    for (int j0 = 0; j0 < mmq_x; j0 += tile_c::J_VALUE) {
#pragma unroll
        for (int l = 0; l < tile_c::ne; ++l) {
            const int j = j0 + tile_c::get_j(l);
            if (j > j_max) {
                continue;
            }
            const int i = threadIdx.y * tile_c::I_VALUE + tile_c::get_i(l);
            if constexpr (need_check) {
                if (i > i_max) {
                    continue;
                }
            }
            dst[j * stride + i] = sum[(j0 / tile_c::J_VALUE) * tile_c::ne + l];
        }
    }
#else
    makepad_ggml_cuda_mmq_write_back<mmq_x, need_check>(sum, dst, stride, i_max, j_max);
#endif
}

template <int mmq_x, bool need_check>
__launch_bounds__(MMQ_WARP_THREADS, 1)
static __global__ void makepad_ggml_cuda_nvfp4_q8_1_mmq_matmul_kernel(
        const block_nvfp4 * __restrict__ weights_nvfp4,
        const int * __restrict__ input_q8_1_mmq,
        float * __restrict__ output_f32,
        const int ncols_x,
        const int out_rows,
        const int input_rows) {
    const int it = blockIdx.x;
    const int jt = blockIdx.y;

    const int i_max = out_rows - it * MMQ_Y - 1;
    const int j_max = input_rows - jt * mmq_x - 1;
    if (i_max < 0 || j_max < 0) {
        return;
    }

    const int tile_y_ints = makepad_ggml_cuda_pad(mmq_x * MMQ_TILE_Y_K, MMQ_WARP_THREADS);
    extern __shared__ int shared_data[];
    int * tile_y = shared_data;
    int * tile_x = shared_data + tile_y_ints;

    constexpr int qk = QK_NVFP4;
    constexpr int ne_block = 4 * QK8_1;
    constexpr int blocks_per_iter = MMQ_ITER_K / qk;
    constexpr int sum_elems = mmq_x * MMQ_Y / (MMQ_NWARPS * WARP_SIZE);

    float sum[sum_elems] = {0.0f};

    const int stride_row_x = ncols_x / qk;
    const int offset_x = it * MMQ_Y * stride_row_x;
    float * dst = output_f32 + jt * mmq_x * out_rows + it * MMQ_Y;

    for (int kb0 = 0; kb0 < stride_row_x; kb0 += blocks_per_iter) {
        makepad_ggml_cuda_load_tiles_nvfp4<need_check>(
            weights_nvfp4,
            tile_x,
            offset_x + kb0,
            i_max,
            stride_row_x);

        const int * by0 = input_q8_1_mmq + input_rows * (kb0 * qk / ne_block) * BLOCK_Q8_1_MMQ_INTS;

#pragma unroll
        for (int l0 = 0; l0 < mmq_x * MMQ_TILE_Y_K; l0 += MMQ_WARP_THREADS) {
            const int l = l0 + threadIdx.y * WARP_SIZE + threadIdx.x;
            if (l < mmq_x * MMQ_TILE_Y_K) {
                const int col = l / MMQ_TILE_Y_K;
                tile_y[l] = col < input_rows ? by0[l] : 0;
            }
        }

        __syncthreads();
#if __CUDA_ARCH__ >= 750
        makepad_ggml_cuda_vec_dot_q8_0_16_q8_1_mma<mmq_x>(tile_x, tile_y, sum, 0);
#else
        makepad_ggml_cuda_vec_dot_q8_0_16_q8_1_dp4a<mmq_x>(tile_x, tile_y, sum, 0);
#endif
        __syncthreads();

        const int * by1 = by0 + input_rows * BLOCK_Q8_1_MMQ_INTS;

#pragma unroll
        for (int l0 = 0; l0 < mmq_x * MMQ_TILE_Y_K; l0 += MMQ_WARP_THREADS) {
            const int l = l0 + threadIdx.y * WARP_SIZE + threadIdx.x;
            if (l < mmq_x * MMQ_TILE_Y_K) {
                const int col = l / MMQ_TILE_Y_K;
                tile_y[l] = col < input_rows ? by1[l] : 0;
            }
        }

        __syncthreads();
#if __CUDA_ARCH__ >= 750
        makepad_ggml_cuda_vec_dot_q8_0_16_q8_1_mma<mmq_x>(tile_x, tile_y, sum, MMQ_TILE_NE_K);
#else
        makepad_ggml_cuda_vec_dot_q8_0_16_q8_1_dp4a<mmq_x>(tile_x, tile_y, sum, MMQ_TILE_NE_K);
#endif
        __syncthreads();
    }

#if __CUDA_ARCH__ >= 750
    makepad_ggml_cuda_mmq_write_back_mma<mmq_x, need_check>(sum, dst, out_rows, i_max, j_max);
#else
    makepad_ggml_cuda_mmq_write_back<mmq_x, need_check>(sum, dst, out_rows, i_max, j_max);
#endif
}

static inline int makepad_ggml_cuda_select_mmq_x(uint32_t input_rows) {
    if (input_rows <= 8) {
        return 8;
    }
    if (input_rows <= 16) {
        return 16;
    }
    if (input_rows <= 24) {
        return 24;
    }
    return 32;
}

static inline int makepad_ggml_cuda_nvfp4_mmq_shared_bytes(int mmq_x) {
    const int tile_y_ints = makepad_ggml_cuda_pad(mmq_x * MMQ_TILE_Y_K, MMQ_WARP_THREADS);
    const int tile_x_ints = MMQ_Y * MMQ_MMA_TILE_X_K_NVFP4;
    return (tile_y_ints + tile_x_ints) * static_cast<int>(sizeof(int));
}

template <int mmq_x>
static cudaError_t makepad_ggml_cuda_launch_nvfp4_q8_1_mmq_matmul(
        const uint8_t * input_q8_1_mmq_bytes,
        const uint8_t * packed_weights_nvfp4_bytes,
        float * output_f32,
        uint32_t ncols_x,
        uint32_t out_rows,
        uint32_t input_rows,
        cudaStream_t stream) {
    const dim3 block(WARP_SIZE, MMQ_NWARPS, 1);
    const dim3 grid((out_rows + MMQ_Y - 1) / MMQ_Y, (input_rows + mmq_x - 1) / mmq_x, 1);
    const int shared_bytes = makepad_ggml_cuda_nvfp4_mmq_shared_bytes(mmq_x);

    if ((out_rows % MMQ_Y) == 0) {
        makepad_ggml_cuda_nvfp4_q8_1_mmq_matmul_kernel<mmq_x, false><<<grid, block, shared_bytes, stream>>>(
            reinterpret_cast<const block_nvfp4 *>(packed_weights_nvfp4_bytes),
            reinterpret_cast<const int *>(input_q8_1_mmq_bytes),
            output_f32,
            static_cast<int>(ncols_x),
            static_cast<int>(out_rows),
            static_cast<int>(input_rows));
    } else {
        makepad_ggml_cuda_nvfp4_q8_1_mmq_matmul_kernel<mmq_x, true><<<grid, block, shared_bytes, stream>>>(
            reinterpret_cast<const block_nvfp4 *>(packed_weights_nvfp4_bytes),
            reinterpret_cast<const int *>(input_q8_1_mmq_bytes),
            output_f32,
            static_cast<int>(ncols_x),
            static_cast<int>(out_rows),
            static_cast<int>(input_rows));
    }

    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_quantize_q8_1_mmq_f32(
        const float * input_f32,
        uint8_t * output_q8_1_mmq_bytes,
        uint32_t n_cols,
        uint32_t n_rows,
        cudaStream_t stream) {
    if (n_cols == 0 || n_rows == 0 || (n_cols % (4 * QK8_1)) != 0) {
        return cudaErrorInvalidValue;
    }

    const uint32_t block_num_y =
        (n_cols + 4 * CUDA_QUANTIZE_BLOCK_SIZE_MMQ - 1) / (4 * CUDA_QUANTIZE_BLOCK_SIZE_MMQ);
    const dim3 grid(n_rows, block_num_y, 1);
    const dim3 block(CUDA_QUANTIZE_BLOCK_SIZE_MMQ, 1, 1);
    makepad_ggml_cuda_quantize_q8_1_mmq_f32_kernel<<<grid, block, 0, stream>>>(
        input_f32,
        reinterpret_cast<block_q8_1_mmq *>(output_q8_1_mmq_bytes),
        n_cols,
        n_rows);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_nvfp4_q8_1_mmq_matmul(
        const uint8_t * input_q8_1_mmq_bytes,
        const uint8_t * packed_weights_nvfp4_bytes,
        float * output_f32,
        uint32_t n_cols,
        uint32_t out_rows,
        uint32_t input_rows,
        cudaStream_t stream) {
    if (n_cols == 0 || out_rows == 0 || input_rows == 0 || (n_cols % (4 * QK8_1)) != 0) {
        return cudaErrorInvalidValue;
    }

    switch (makepad_ggml_cuda_select_mmq_x(input_rows)) {
        case 8:
            return makepad_ggml_cuda_launch_nvfp4_q8_1_mmq_matmul<8>(
                input_q8_1_mmq_bytes,
                packed_weights_nvfp4_bytes,
                output_f32,
                n_cols,
                out_rows,
                input_rows,
                stream);
        case 16:
            return makepad_ggml_cuda_launch_nvfp4_q8_1_mmq_matmul<16>(
                input_q8_1_mmq_bytes,
                packed_weights_nvfp4_bytes,
                output_f32,
                n_cols,
                out_rows,
                input_rows,
                stream);
        case 24:
            return makepad_ggml_cuda_launch_nvfp4_q8_1_mmq_matmul<24>(
                input_q8_1_mmq_bytes,
                packed_weights_nvfp4_bytes,
                output_f32,
                n_cols,
                out_rows,
                input_rows,
                stream);
        case 32:
            return makepad_ggml_cuda_launch_nvfp4_q8_1_mmq_matmul<32>(
                input_q8_1_mmq_bytes,
                packed_weights_nvfp4_bytes,
                output_f32,
                n_cols,
                out_rows,
                input_rows,
                stream);
        default:
            return cudaErrorInvalidValue;
    }
}
