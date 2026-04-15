#include <cuda_fp16.h>
#include <cuda_runtime.h>
#if CUDART_VERSION >= 11080
#include <cuda_fp8.h>
#define MAKEPAD_GGML_CUDA_FP8_AVAILABLE
#endif
#include <stdint.h>

static constexpr uint32_t QK8_1 = 32;
static constexpr uint32_t QK_NVFP4 = 64;
static constexpr uint32_t QK_NVFP4_SUB = 16;
static constexpr uint32_t QI8_1 = 8;

typedef struct {
    uint16_t d;
    uint16_t s;
    int8_t qs[QK8_1];
} __align__(4) block_q8_1;

typedef struct {
    uint8_t d[QK_NVFP4 / QK_NVFP4_SUB];
    uint8_t qs[QK_NVFP4 / 2];
} __align__(4) block_nvfp4;

static_assert(sizeof(block_q8_1) == 36, "wrong q8_1 block size");
static_assert(sizeof(block_nvfp4) == 36, "wrong nvfp4 block size");

__device__ __forceinline__ float makepad_ggml_cuda_f16_to_f32(uint16_t word) {
    __half_raw raw;
    raw.x = word;
    return __half2float(*reinterpret_cast<__half *>(&raw));
}

static __device__ __forceinline__ float makepad_ggml_cuda_ue4m3_to_fp32(uint8_t x) {
#if defined(MAKEPAD_GGML_CUDA_FP8_AVAILABLE)
    const uint32_t bits = x * (x != 0x7F && x != 0xFF);
    const __nv_fp8_e4m3 xf = *reinterpret_cast<const __nv_fp8_e4m3 *>(&bits);
    return static_cast<float>(xf) * 0.5f;
#else
    if (x == 0 || x == 0x7F || x == 0xFF) {
        return 0.0f;
    }
    const int exp = (x >> 3) & 0xF;
    const int man = x & 0x7;
    const float raw = exp == 0 ? ldexpf((float) man, -9) : ldexpf(1.0f + (float) man / 8.0f, exp - 7);
    return raw * 0.5f;
#endif
}

__device__ __constant__ int8_t KVALUES_MXFP4_X2[16] = {
    0, 1, 2, 3, 4, 6, 8, 12, 0, -1, -2, -3, -4, -6, -8, -12,
};

static __device__ __forceinline__ int makepad_ggml_cuda_get_int_b4(const void * x, const int i32) {
    return reinterpret_cast<const int *>(x)[i32];
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

static __device__ __forceinline__ int makepad_ggml_cuda_dp4a(const int a, const int b, int c) {
#if __CUDA_ARCH__ >= 610
    return __dp4a(a, b, c);
#else
    const int8_t * a8 = reinterpret_cast<const int8_t *>(&a);
    const int8_t * b8 = reinterpret_cast<const int8_t *>(&b);
    return c + a8[0] * b8[0] + a8[1] * b8[1] + a8[2] * b8[2] + a8[3] * b8[3];
#endif
}

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

static __device__ __forceinline__ float makepad_ggml_cuda_e4m3fn_to_fp32(uint8_t x) {
#if defined(MAKEPAD_GGML_CUDA_FP8_AVAILABLE)
    const uint32_t bits = x * (x != 0x7F && x != 0xFF);
    const __nv_fp8_e4m3 xf = *reinterpret_cast<const __nv_fp8_e4m3 *>(&bits);
    return static_cast<float>(xf);
#else
    if (x == 0 || x == 0x7F || x == 0xFF) {
        return 0.0f;
    }
    const int exp = (x >> 3) & 0xF;
    const int man = x & 0x7;
    if (exp == 0) {
        return ldexpf(static_cast<float>(man), -9);
    }
    return ldexpf(1.0f + static_cast<float>(man) / 8.0f, exp - 7);
#endif
}

static __device__ __forceinline__ uint8_t makepad_ggml_cuda_fp32_to_e4m3fn(float x) {
#if CUDART_VERSION >= 12080
    return __nv_cvt_float_to_fp8(x, __NV_SATFINITE, __NV_E4M3);
#else
    if (!(x > 0.0f)) {
        return 0;
    }

    uint8_t best = 0;
    float best_err = x;
    for (uint32_t i = 1; i < 0x7F; ++i) {
        const uint8_t e4m3 = static_cast<uint8_t>(i);
        const float v = makepad_ggml_cuda_e4m3fn_to_fp32(e4m3);
        const float err = fabsf(v - x);
        if (err < best_err) {
            best = e4m3;
            best_err = err;
        }
    }
    return best;
#endif
}

static __device__ __forceinline__ uint8_t makepad_ggml_cuda_float_to_fp4_e2m1(float x, float e) {
    const uint8_t sign_bit = static_cast<uint8_t>((x < 0.0f) << 3);
    const float ax = fabsf(x) * e;
    static constexpr float POS_LUT[8] = { 0.0f, 0.5f, 1.0f, 1.5f, 2.0f, 3.0f, 4.0f, 6.0f };

    int best_i = 0;
    float best_err = fabsf(ax - POS_LUT[0]);
#pragma unroll
    for (int i = 1; i < 8; ++i) {
        const float err = fabsf(ax - POS_LUT[i]);
        if (err < best_err) {
            best_err = err;
            best_i = i;
        }
    }
    return static_cast<uint8_t>(best_i | sign_bit);
}

static __device__ __forceinline__ float makepad_ggml_cuda_vec_dot_nvfp4_q8_1(
        const void * __restrict__ vbq,
        const block_q8_1 * __restrict__ bq8_1,
        const int32_t kbx,
        const int32_t iqs) {
    const block_nvfp4 * bq4 = reinterpret_cast<const block_nvfp4 *>(vbq) + kbx;
    float sum = 0.0f;

#pragma unroll
    for (int i = 0; i < 2; ++i) {
        const int32_t iqs0 = iqs + 2 * i;
        const int32_t iqs1 = iqs0 + 1;
        const int32_t is = iqs0 >> 1;
        const int2 v0 = makepad_ggml_cuda_get_int_from_table_16(
            makepad_ggml_cuda_get_int_b4(bq4->qs, iqs0),
            KVALUES_MXFP4_X2);
        const int2 v1 = makepad_ggml_cuda_get_int_from_table_16(
            makepad_ggml_cuda_get_int_b4(bq4->qs, iqs1),
            KVALUES_MXFP4_X2);
        const block_q8_1 * bq8 = bq8_1 + (is >> 1);
        const int32_t i8 = ((is & 1) << 2);

        int sumi = makepad_ggml_cuda_dp4a(v0.x, makepad_ggml_cuda_get_int_b4(bq8->qs, i8 + 0), 0);
        sumi = makepad_ggml_cuda_dp4a(v0.y, makepad_ggml_cuda_get_int_b4(bq8->qs, i8 + 2), sumi);
        sumi = makepad_ggml_cuda_dp4a(v1.x, makepad_ggml_cuda_get_int_b4(bq8->qs, i8 + 1), sumi);
        sumi = makepad_ggml_cuda_dp4a(v1.y, makepad_ggml_cuda_get_int_b4(bq8->qs, i8 + 3), sumi);

        const float d = makepad_ggml_cuda_ue4m3_to_fp32(bq4->d[is]) * makepad_ggml_cuda_f16_to_f32(bq8->d);
        sum += d * static_cast<float>(sumi);
    }

    return sum;
}

static __device__ __forceinline__ float makepad_ggml_cuda_vec_dot_nvfp4_nvfp4_modelopt(
        const void * __restrict__ vx,
        const block_nvfp4 * __restrict__ vy,
        const int32_t kbx,
        const int32_t iqs) {
    const block_nvfp4 * bx = reinterpret_cast<const block_nvfp4 *>(vx) + kbx;
    float sum = 0.0f;

#pragma unroll
    for (int i = 0; i < 2; ++i) {
        const int32_t iqs0 = iqs + 2 * i;
        const int32_t iqs1 = iqs0 + 1;
        const int32_t is = iqs0 >> 1;
        const int2 x0 = makepad_ggml_cuda_get_int_from_table_16(
            makepad_ggml_cuda_get_int_b4(bx->qs, iqs0),
            KVALUES_MXFP4_X2);
        const int2 x1 = makepad_ggml_cuda_get_int_from_table_16(
            makepad_ggml_cuda_get_int_b4(bx->qs, iqs1),
            KVALUES_MXFP4_X2);
        const int2 y0 = makepad_ggml_cuda_get_int_from_table_16(
            makepad_ggml_cuda_get_int_b4(vy->qs, iqs0),
            KVALUES_MXFP4_X2);
        const int2 y1 = makepad_ggml_cuda_get_int_from_table_16(
            makepad_ggml_cuda_get_int_b4(vy->qs, iqs1),
            KVALUES_MXFP4_X2);

        int sumi = makepad_ggml_cuda_dp4a(x0.x, y0.x, 0);
        sumi = makepad_ggml_cuda_dp4a(x0.y, y0.y, sumi);
        sumi = makepad_ggml_cuda_dp4a(x1.x, y1.x, sumi);
        sumi = makepad_ggml_cuda_dp4a(x1.y, y1.y, sumi);

        const float d = makepad_ggml_cuda_ue4m3_to_fp32(bx->d[is]) *
            (0.5f * makepad_ggml_cuda_e4m3fn_to_fp32(vy->d[is]));
        sum += d * static_cast<float>(sumi);
    }

    return sum;
}

static __launch_bounds__(128, 1) __global__ void makepad_ggml_cuda_nvfp4_q8_1_matvec_kernel(
        const block_q8_1 * __restrict__ input_q8_1,
        const block_nvfp4 * __restrict__ packed_weights_nvfp4,
        float * __restrict__ output_f32,
        uint32_t blocks_per_row,
        uint32_t out_rows) {
    constexpr int warp_size = 32;
    constexpr int nwarps = 4;
    constexpr int qi = QK_NVFP4 / (4 * 2);
    constexpr int vdr = 4;
    constexpr int blocks_per_iter = vdr * nwarps * warp_size / qi;

    const uint32_t row = blockIdx.x;
    if (row >= out_rows) {
        return;
    }

    const int lane = threadIdx.x;
    const int warp = threadIdx.y;
    const int tid = warp_size * warp + lane;
    const int blocks_per_q8 = QK_NVFP4 / QK8_1;
    const int kbx_offset = static_cast<int>(row * blocks_per_row);

    float tmp = 0.0f;
    for (int kbx = tid / 2; kbx < static_cast<int>(blocks_per_row); kbx += blocks_per_iter) {
        const int kby = kbx * blocks_per_q8;
        const int kqs = vdr * (tid % 2);
        tmp += makepad_ggml_cuda_vec_dot_nvfp4_q8_1(
            packed_weights_nvfp4,
            input_q8_1 + kby,
            kbx_offset + kbx,
            kqs);
    }

    __shared__ float tmp_shared[nwarps - 1][warp_size];
    if (warp > 0) {
        tmp_shared[warp - 1][lane] = tmp;
    }
    __syncthreads();

    if (warp > 0) {
        return;
    }

#pragma unroll
    for (int w = 0; w < nwarps - 1; ++w) {
        tmp += tmp_shared[w][lane];
    }
    tmp = makepad_ggml_cuda_warp_reduce_sum<warp_size>(tmp);

    if (lane == 0) {
        output_f32[row] = tmp;
    }
}

static __launch_bounds__(128, 1) __global__ void makepad_ggml_cuda_nvfp4_q8_1_matmul_kernel(
        const block_q8_1 * __restrict__ input_q8_1,
        const block_nvfp4 * __restrict__ packed_weights_nvfp4,
        float * __restrict__ output_f32,
        uint32_t blocks_per_row,
        uint32_t out_rows,
        uint32_t input_rows) {
    constexpr int warp_size = 32;
    constexpr int nwarps = 4;
    constexpr int qi = QK_NVFP4 / (4 * 2);
    constexpr int vdr = 4;
    constexpr int blocks_per_iter = vdr * nwarps * warp_size / qi;

    const uint32_t row = blockIdx.x;
    const uint32_t input_row = blockIdx.y;
    if (row >= out_rows || input_row >= input_rows) {
        return;
    }

    const int lane = threadIdx.x;
    const int warp = threadIdx.y;
    const int tid = warp_size * warp + lane;
    const int blocks_per_q8 = QK_NVFP4 / QK8_1;
    const int q8_blocks_per_input_row = static_cast<int>(blocks_per_row * blocks_per_q8);
    const int kbx_offset = static_cast<int>(row * blocks_per_row);
    const block_q8_1 * input_row_q8_1 =
        input_q8_1 + static_cast<int>(input_row) * q8_blocks_per_input_row;

    float tmp = 0.0f;
    for (int kbx = tid / 2; kbx < static_cast<int>(blocks_per_row); kbx += blocks_per_iter) {
        const int kby = kbx * blocks_per_q8;
        const int kqs = vdr * (tid % 2);
        tmp += makepad_ggml_cuda_vec_dot_nvfp4_q8_1(
            packed_weights_nvfp4,
            input_row_q8_1 + kby,
            kbx_offset + kbx,
            kqs);
    }

    __shared__ float tmp_shared[nwarps - 1][warp_size];
    if (warp > 0) {
        tmp_shared[warp - 1][lane] = tmp;
    }
    __syncthreads();

    if (warp > 0) {
        return;
    }

#pragma unroll
    for (int w = 0; w < nwarps - 1; ++w) {
        tmp += tmp_shared[w][lane];
    }
    tmp = makepad_ggml_cuda_warp_reduce_sum<warp_size>(tmp);

    if (lane == 0) {
        output_f32[input_row * out_rows + row] = tmp;
    }
}

template <int rows_per_block>
static __launch_bounds__(128, 1) __global__ void makepad_ggml_cuda_nvfp4_nvfp4_matvec_kernel(
        const block_nvfp4 * __restrict__ input_nvfp4,
        const block_nvfp4 * __restrict__ packed_weights_nvfp4,
        float input_scale,
        float * __restrict__ output_f32,
        uint32_t blocks_per_row,
        uint32_t out_rows) {
    constexpr int warp_size = 32;
    constexpr int nwarps = 4;
    constexpr int qi = QK_NVFP4 / (4 * 2);
    constexpr int vdr = 4;
    constexpr int blocks_per_iter = vdr * nwarps * warp_size / qi;

    const uint32_t row0 = blockIdx.x * rows_per_block;
    if (row0 >= out_rows) {
        return;
    }

    const int lane = threadIdx.x;
    const int warp = threadIdx.y;
    const int tid = warp_size * warp + lane;

    float tmp[rows_per_block] = {0.0f};
    for (int kbx = tid / 2; kbx < static_cast<int>(blocks_per_row); kbx += blocks_per_iter) {
        const int kqs = vdr * (tid % 2);
        const block_nvfp4 * input_block = input_nvfp4 + kbx;
#pragma unroll
        for (int row_idx = 0; row_idx < rows_per_block; ++row_idx) {
            const uint32_t row = row0 + row_idx;
            if (row < out_rows) {
                tmp[row_idx] += makepad_ggml_cuda_vec_dot_nvfp4_nvfp4_modelopt(
                    packed_weights_nvfp4,
                    input_block,
                    static_cast<int>(row * blocks_per_row) + kbx,
                    kqs);
            }
        }
    }

    __shared__ float tmp_shared[nwarps - 1][rows_per_block][warp_size];
    if (warp > 0) {
#pragma unroll
        for (int row_idx = 0; row_idx < rows_per_block; ++row_idx) {
            tmp_shared[warp - 1][row_idx][lane] = tmp[row_idx];
        }
    }
    __syncthreads();

    if (warp > 0) {
        return;
    }

#pragma unroll
    for (int row_idx = 0; row_idx < rows_per_block; ++row_idx) {
        float row_sum = tmp[row_idx];
#pragma unroll
        for (int w = 0; w < nwarps - 1; ++w) {
            row_sum += tmp_shared[w][row_idx][lane];
        }
        row_sum = makepad_ggml_cuda_warp_reduce_sum<warp_size>(row_sum) * input_scale;
        if (lane == 0) {
            const uint32_t row = row0 + row_idx;
            if (row < out_rows) {
                output_f32[row] = row_sum;
            }
        }
    }
}

static __launch_bounds__(128, 1) __global__ void makepad_ggml_cuda_nvfp4_nvfp4_matmul_kernel(
        const block_nvfp4 * __restrict__ input_nvfp4,
        const block_nvfp4 * __restrict__ packed_weights_nvfp4,
        float input_scale,
        float * __restrict__ output_f32,
        uint32_t blocks_per_row,
        uint32_t out_rows,
        uint32_t input_rows) {
    constexpr int warp_size = 32;
    constexpr int nwarps = 4;
    constexpr int qi = QK_NVFP4 / (4 * 2);
    constexpr int vdr = 4;
    constexpr int blocks_per_iter = vdr * nwarps * warp_size / qi;

    const uint32_t row = blockIdx.x;
    const uint32_t input_row = blockIdx.y;
    if (row >= out_rows || input_row >= input_rows) {
        return;
    }

    const int lane = threadIdx.x;
    const int warp = threadIdx.y;
    const int tid = warp_size * warp + lane;
    const int kbx_offset = static_cast<int>(row * blocks_per_row);
    const block_nvfp4 * input_row_nvfp4 =
        input_nvfp4 + static_cast<int>(input_row) * static_cast<int>(blocks_per_row);

    float tmp = 0.0f;
    for (int kbx = tid / 2; kbx < static_cast<int>(blocks_per_row); kbx += blocks_per_iter) {
        const int kqs = vdr * (tid % 2);
        tmp += makepad_ggml_cuda_vec_dot_nvfp4_nvfp4_modelopt(
            packed_weights_nvfp4,
            input_row_nvfp4 + kbx,
            kbx_offset + kbx,
            kqs);
    }

    __shared__ float tmp_shared[nwarps - 1][warp_size];
    if (warp > 0) {
        tmp_shared[warp - 1][lane] = tmp;
    }
    __syncthreads();

    if (warp > 0) {
        return;
    }

#pragma unroll
    for (int w = 0; w < nwarps - 1; ++w) {
        tmp += tmp_shared[w][lane];
    }
    tmp = makepad_ggml_cuda_warp_reduce_sum<warp_size>(tmp) * input_scale;

    if (lane == 0) {
        output_f32[input_row * out_rows + row] = tmp;
    }
}

static __launch_bounds__(QK_NVFP4, 1) __global__ void makepad_ggml_cuda_quantize_nvfp4_f32_kernel(
        const float * __restrict__ input_f32,
        float input_scale,
        block_nvfp4 * __restrict__ output_nvfp4) {
    const uint32_t block_idx = blockIdx.x;
    const uint32_t lane = threadIdx.x;
    const uint32_t base = block_idx * QK_NVFP4;
    const uint32_t sub = lane / QK_NVFP4_SUB;
    const uint32_t lane_sub = lane % QK_NVFP4_SUB;

    __shared__ uint8_t q_shared[QK_NVFP4];
    __shared__ float d_shared[QK_NVFP4 / QK_NVFP4_SUB];

    const float in_s = input_scale != 0.0f ? input_scale : 1.0f;
    const float in_s_inv = 1.0f / in_s;
    const float xi = input_f32[base + lane];

    float amax = fabsf(xi);
    amax = makepad_ggml_cuda_warp_reduce_max<QK_NVFP4_SUB>(amax);
    if (lane_sub == 0) {
        const uint8_t e4m3 = makepad_ggml_cuda_fp32_to_e4m3fn((amax / 6.0f) * in_s_inv);
        output_nvfp4[block_idx].d[sub] = e4m3;
        d_shared[sub] = makepad_ggml_cuda_e4m3fn_to_fp32(e4m3);
    }

    __syncthreads();

    const float d = d_shared[sub] * in_s;
    const float d_inv = d != 0.0f ? 1.0f / d : 0.0f;
    q_shared[lane] = makepad_ggml_cuda_float_to_fp4_e2m1(xi, d_inv) & 0x0F;

    __syncthreads();

    if (lane_sub < QK_NVFP4_SUB / 2) {
        output_nvfp4[block_idx].qs[sub * (QK_NVFP4_SUB / 2) + lane_sub] =
            q_shared[sub * QK_NVFP4_SUB + lane_sub] |
            (q_shared[sub * QK_NVFP4_SUB + lane_sub + QK_NVFP4_SUB / 2] << 4);
    }
}

static __global__ void makepad_ggml_cuda_nvfp4_get_row_f32_kernel(
        const block_nvfp4 * __restrict__ packed_weights_nvfp4,
        float * __restrict__ output_f32) {
    const uint32_t block_idx = blockIdx.x;
    const uint32_t tid = threadIdx.x;
    const block_nvfp4 & xb = packed_weights_nvfp4[block_idx];

    const uint32_t sub = tid / (QK_NVFP4_SUB / 2);
    const uint32_t j = tid % (QK_NVFP4_SUB / 2);
    const float d = makepad_ggml_cuda_ue4m3_to_fp32(xb.d[sub]);
    const uint8_t packed = xb.qs[sub * (QK_NVFP4_SUB / 2) + j];
    const uint32_t base = block_idx * QK_NVFP4 + sub * QK_NVFP4_SUB;

    output_f32[base + j] = d * (float) KVALUES_MXFP4_X2[packed & 0x0F];
    output_f32[base + QK_NVFP4_SUB / 2 + j] = d * (float) KVALUES_MXFP4_X2[packed >> 4];
}

static __global__ void makepad_ggml_cuda_nvfp4_get_row_f32_device_u32_kernel(
        const block_nvfp4 * __restrict__ packed_weights_nvfp4,
        const uint32_t * __restrict__ row_index_device_u32,
        uint32_t blocks_per_row,
        float * __restrict__ output_f32) {
    const uint32_t row_index = *row_index_device_u32;
    const uint32_t block_idx = blockIdx.x;
    const uint32_t tid = threadIdx.x;
    const block_nvfp4 & xb = packed_weights_nvfp4[row_index * blocks_per_row + block_idx];

    const uint32_t sub = tid / (QK_NVFP4_SUB / 2);
    const uint32_t j = tid % (QK_NVFP4_SUB / 2);
    const float d = makepad_ggml_cuda_ue4m3_to_fp32(xb.d[sub]);
    const uint8_t packed = xb.qs[sub * (QK_NVFP4_SUB / 2) + j];
    const uint32_t base = block_idx * QK_NVFP4 + sub * QK_NVFP4_SUB;

    output_f32[base + j] = d * (float) KVALUES_MXFP4_X2[packed & 0x0F];
    output_f32[base + QK_NVFP4_SUB / 2 + j] = d * (float) KVALUES_MXFP4_X2[packed >> 4];
}

static __global__ void makepad_ggml_cuda_nvfp4_get_rows_f32_device_u32_kernel(
        const block_nvfp4 * __restrict__ packed_weights_nvfp4,
        const uint32_t * __restrict__ row_indices_device_u32,
        uint32_t blocks_per_row,
        float * __restrict__ output_f32,
        uint32_t output_row_stride) {
    const uint32_t block_idx = blockIdx.x;
    const uint32_t row_slot = blockIdx.y;
    const uint32_t tid = threadIdx.x;
    const uint32_t row_index = row_indices_device_u32[row_slot];
    const block_nvfp4 & xb = packed_weights_nvfp4[row_index * blocks_per_row + block_idx];

    const uint32_t sub = tid / (QK_NVFP4_SUB / 2);
    const uint32_t j = tid % (QK_NVFP4_SUB / 2);
    const float d = makepad_ggml_cuda_ue4m3_to_fp32(xb.d[sub]);
    const uint8_t packed = xb.qs[sub * (QK_NVFP4_SUB / 2) + j];
    const uint32_t base = row_slot * output_row_stride + block_idx * QK_NVFP4 + sub * QK_NVFP4_SUB;

    output_f32[base + j] = d * (float) KVALUES_MXFP4_X2[packed & 0x0F];
    output_f32[base + QK_NVFP4_SUB / 2 + j] = d * (float) KVALUES_MXFP4_X2[packed >> 4];
}

extern "C" cudaError_t makepad_ggml_cuda_nvfp4_q8_1_matvec(
        const uint8_t * input_q8_1_bytes,
        const uint8_t * packed_weights_nvfp4_bytes,
        float * output_f32,
        uint32_t q8_1_blocks,
        uint32_t out_rows,
        cudaStream_t stream) {
    if (q8_1_blocks == 0 || (q8_1_blocks % 2) != 0) {
        return cudaErrorInvalidValue;
    }
    const uint32_t blocks_per_row = q8_1_blocks / 2;
    const dim3 grid(out_rows, 1, 1);
    const dim3 block(32, 4, 1);
    makepad_ggml_cuda_nvfp4_q8_1_matvec_kernel<<<grid, block, 0, stream>>>(
        reinterpret_cast<const block_q8_1 *>(input_q8_1_bytes),
        reinterpret_cast<const block_nvfp4 *>(packed_weights_nvfp4_bytes),
        output_f32,
        blocks_per_row,
        out_rows
    );
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_nvfp4_q8_1_matmul(
        const uint8_t * input_q8_1_bytes,
        const uint8_t * packed_weights_nvfp4_bytes,
        float * output_f32,
        uint32_t q8_1_blocks,
        uint32_t out_rows,
        uint32_t input_rows,
        cudaStream_t stream) {
    if (q8_1_blocks == 0 || (q8_1_blocks % 2) != 0 || input_rows == 0) {
        return cudaErrorInvalidValue;
    }
    const uint32_t blocks_per_row = q8_1_blocks / 2;
    const dim3 grid(out_rows, input_rows, 1);
    const dim3 block(32, 4, 1);
    makepad_ggml_cuda_nvfp4_q8_1_matmul_kernel<<<grid, block, 0, stream>>>(
        reinterpret_cast<const block_q8_1 *>(input_q8_1_bytes),
        reinterpret_cast<const block_nvfp4 *>(packed_weights_nvfp4_bytes),
        output_f32,
        blocks_per_row,
        out_rows,
        input_rows
    );
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_quantize_nvfp4_f32(
        const float * input_f32,
        float input_scale,
        uint8_t * output_nvfp4_bytes,
        uint32_t n,
        cudaStream_t stream) {
    if (n == 0 || (n % QK_NVFP4) != 0) {
        return cudaErrorInvalidValue;
    }
    makepad_ggml_cuda_quantize_nvfp4_f32_kernel<<<n / QK_NVFP4, QK_NVFP4, 0, stream>>>(
        input_f32,
        input_scale,
        reinterpret_cast<block_nvfp4 *>(output_nvfp4_bytes)
    );
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_nvfp4_nvfp4_matvec(
        const uint8_t * input_nvfp4_bytes,
        const uint8_t * packed_weights_nvfp4_bytes,
        float input_scale,
        float * output_f32,
        uint32_t nvfp4_blocks,
        uint32_t out_rows,
        cudaStream_t stream) {
    if (nvfp4_blocks == 0) {
        return cudaErrorInvalidValue;
    }
    const dim3 block(32, 4, 1);
    if (out_rows >= 32768) {
        constexpr uint32_t rows_per_block = 4;
        const dim3 grid((out_rows + rows_per_block - 1) / rows_per_block, 1, 1);
        makepad_ggml_cuda_nvfp4_nvfp4_matvec_kernel<rows_per_block><<<grid, block, 0, stream>>>(
            reinterpret_cast<const block_nvfp4 *>(input_nvfp4_bytes),
            reinterpret_cast<const block_nvfp4 *>(packed_weights_nvfp4_bytes),
            input_scale,
            output_f32,
            nvfp4_blocks,
            out_rows
        );
    } else {
        constexpr uint32_t rows_per_block = 1;
        const dim3 grid((out_rows + rows_per_block - 1) / rows_per_block, 1, 1);
        makepad_ggml_cuda_nvfp4_nvfp4_matvec_kernel<rows_per_block><<<grid, block, 0, stream>>>(
            reinterpret_cast<const block_nvfp4 *>(input_nvfp4_bytes),
            reinterpret_cast<const block_nvfp4 *>(packed_weights_nvfp4_bytes),
            input_scale,
            output_f32,
            nvfp4_blocks,
            out_rows
        );
    }
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_nvfp4_nvfp4_matmul(
        const uint8_t * input_nvfp4_bytes,
        const uint8_t * packed_weights_nvfp4_bytes,
        float input_scale,
        float * output_f32,
        uint32_t nvfp4_blocks,
        uint32_t out_rows,
        uint32_t input_rows,
        cudaStream_t stream) {
    if (nvfp4_blocks == 0 || input_rows == 0) {
        return cudaErrorInvalidValue;
    }
    const dim3 grid(out_rows, input_rows, 1);
    const dim3 block(32, 4, 1);
    makepad_ggml_cuda_nvfp4_nvfp4_matmul_kernel<<<grid, block, 0, stream>>>(
        reinterpret_cast<const block_nvfp4 *>(input_nvfp4_bytes),
        reinterpret_cast<const block_nvfp4 *>(packed_weights_nvfp4_bytes),
        input_scale,
        output_f32,
        nvfp4_blocks,
        out_rows,
        input_rows
    );
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_nvfp4_get_row_f32(
        const uint8_t * packed_weights_nvfp4_bytes,
        float * output_f32,
        uint32_t n_cols,
        uint32_t row_index,
        cudaStream_t stream) {
    if ((n_cols % QK_NVFP4) != 0) {
        return cudaErrorInvalidValue;
    }
    const uint32_t blocks_per_row = n_cols / QK_NVFP4;
    const block_nvfp4 * row_ptr =
        reinterpret_cast<const block_nvfp4 *>(packed_weights_nvfp4_bytes) + row_index * blocks_per_row;
    makepad_ggml_cuda_nvfp4_get_row_f32_kernel<<<blocks_per_row, 32, 0, stream>>>(
        row_ptr,
        output_f32
    );
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_nvfp4_get_row_f32_device_u32(
        const uint8_t * packed_weights_nvfp4_bytes,
        float * output_f32,
        uint32_t n_cols,
        const uint32_t * row_index_device_u32,
        cudaStream_t stream) {
    if ((n_cols % QK_NVFP4) != 0) {
        return cudaErrorInvalidValue;
    }
    const uint32_t blocks_per_row = n_cols / QK_NVFP4;
    makepad_ggml_cuda_nvfp4_get_row_f32_device_u32_kernel<<<blocks_per_row, 32, 0, stream>>>(
        reinterpret_cast<const block_nvfp4 *>(packed_weights_nvfp4_bytes),
        row_index_device_u32,
        blocks_per_row,
        output_f32
    );
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_nvfp4_get_rows_f32_device_u32(
        const uint8_t * packed_weights_nvfp4_bytes,
        const uint32_t * row_indices_device_u32,
        float * output_f32,
        uint32_t n_cols,
        uint32_t row_count,
        uint32_t output_row_stride,
        cudaStream_t stream) {
    if ((n_cols % QK_NVFP4) != 0 || row_count == 0 || output_row_stride < n_cols) {
        return cudaErrorInvalidValue;
    }
    const uint32_t blocks_per_row = n_cols / QK_NVFP4;
    const dim3 grid(blocks_per_row, row_count, 1);
    makepad_ggml_cuda_nvfp4_get_rows_f32_device_u32_kernel<<<grid, 32, 0, stream>>>(
        reinterpret_cast<const block_nvfp4 *>(packed_weights_nvfp4_bytes),
        row_indices_device_u32,
        blocks_per_row,
        output_f32,
        output_row_stride
    );
    return cudaGetLastError();
}
