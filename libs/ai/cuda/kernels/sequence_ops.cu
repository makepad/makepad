// Fixed-workspace and captured sequence operations. No model-specific state.
#include <cuda_fp16.h>
#include <cuda_bf16.h>
#include <cuda_runtime.h>
#include <stdint.h>
#include "launch_profile.cuh"

// Same reduction tree and launch widths as the precise weighted RMSNorm
// in ops.cu. In particular, do not replace this with a warp-per-head norm:
// changing the reduction changes the long autoregressive trajectory.
static __device__ __forceinline__ float sequence_warp_sum(float value) {
    for (int offset = 16; offset > 0; offset >>= 1) {
        value += __shfl_down_sync(0xffffffffu, value, offset);
    }
    return value;
}

static __device__ __forceinline__ float sequence_block_sum(float value) {
    __shared__ float shared[32];
    const uint32_t lane = threadIdx.x & 31;
    const uint32_t warp = threadIdx.x >> 5;
    value = sequence_warp_sum(value);
    if (lane == 0) shared[warp] = value;
    __syncthreads();
    value = threadIdx.x < (blockDim.x + 31) / 32 ? shared[lane] : 0.0f;
    if (warp == 0) {
        value = sequence_warp_sum(value);
    }
    return value;
}

static __device__ __forceinline__ uint16_t sequence_bf16_rn(float x) {
    const __nv_bfloat16 h = __float2bfloat16_rn(x);
    return *reinterpret_cast<const uint16_t *>(&h);
}

static __device__ __forceinline__ float sequence_round_bf16(float x) {
    // Preserve non-finite values exactly, matching the host ODE rounding.
    if ((__float_as_uint(x) & 0x7f800000u) == 0x7f800000u) return x;
    return __uint_as_float(static_cast<uint32_t>(sequence_bf16_rn(x)) << 16);
}

static __global__ void add_rms_norm_kernel(
    const float *x, const float *update, const float *weight,
    float *residual, float *normed, uint32_t cols, float eps) {
    const size_t base = (size_t)blockIdx.x * cols;
    float sum = 0.0f;
    for (uint32_t i = threadIdx.x; i < cols; i += blockDim.x) {
        const float v = update ? __fadd_rn(x[base + i], update[base + i]) : x[base + i];
        residual[base + i] = v;
        sum += v * v;
    }
    sum = sequence_block_sum(sum);
    __shared__ float inv_rms;
    if (threadIdx.x == 0) inv_rms = rsqrtf(sum / (float)cols + eps);
    __syncthreads();
    for (uint32_t i = threadIdx.x; i < cols; i += blockDim.x) {
        normed[base + i] = residual[base + i] * inv_rms * weight[i];
    }
}

extern "C" cudaError_t makepad_cuda_add_rms_norm_into_f32(
    const float *x, const float *update, const float *weight,
    float *residual, float *normed, uint32_t rows, uint32_t cols,
    float eps, cudaStream_t stream) {
    if (!rows || !cols) return cudaErrorInvalidValue;
    MakepadLaunchTiming timing("add_rms_norm", stream, rows, cols);
    add_rms_norm_kernel<<<rows, cols < 1024 ? 256 : 1024, 0, stream>>>(
        x, update, weight, residual, normed, cols, eps);
    return cudaGetLastError();
}

// One token's packed [q | k | v] projection. Each Q/K head owns a CTA;
// K CTAs also copy the corresponding V head into the cache. Position is
// a device u32, so a captured launch can write a different row each time.
static __device__ __forceinline__ void sequence_cache_store(float *dst, float value) {
    *dst = value;
}

static __device__ __forceinline__ void sequence_cache_store(uint16_t *dst, float value) {
    *dst = sequence_bf16_rn(value);
}

template <typename Cache>
static __global__ void qk_rms_norm_rope_cache_kernel(
    const float *qkv, const float *q_weight, const float *k_weight,
    const float *cos_table, const float *sin_table, const uint32_t *position,
    float *query, Cache *key_cache, Cache *value_cache,
    uint32_t heads, uint32_t kv_heads, uint32_t head_dim,
    uint32_t capacity, uint32_t table_rows, float eps) {
    const uint32_t pos = *position;
    // The caller checks the context before replay. Avoid an invalid write
    // even if a device input is corrupt; poison Q so the problem is visible.
    const uint32_t h = blockIdx.x;
    const bool is_key = h >= heads;
    const uint32_t local_head = is_key ? h - heads : h;
    if (pos >= capacity || pos >= table_rows) {
        if (!is_key) {
            for (uint32_t i = threadIdx.x; i < head_dim; i += blockDim.x) {
                query[(size_t)h * head_dim + i] = __uint_as_float(0x7fc00000u);
            }
        }
        return;
    }
    const float *input = qkv + (size_t)h * head_dim;
    const float *weight = is_key ? k_weight : q_weight;
    float sum = 0.0f;
    for (uint32_t i = threadIdx.x; i < head_dim; i += blockDim.x) {
        const float v = input[i];
        sum += v * v;
    }
    sum = sequence_block_sum(sum);
    __shared__ float inv_rms;
    if (threadIdx.x == 0) inv_rms = rsqrtf(sum / (float)head_dim + eps);
    __syncthreads();
    const uint32_t half = head_dim / 2;
    const size_t cache_base = ((size_t)pos * kv_heads + local_head) * head_dim;
    for (uint32_t i = threadIdx.x; i < half; i += blockDim.x) {
        const float a = input[i] * inv_rms * weight[i];
        const float b = input[half + i] * inv_rms * weight[half + i];
        const float c = cos_table[(size_t)pos * half + i];
        const float s = sin_table[(size_t)pos * half + i];
        // Match rope_half_f32, including nvcc's ordinary FMA contraction.
        const float r0 = a * c - b * s;
        const float r1 = b * c + a * s;
        if (is_key) {
            sequence_cache_store(key_cache + cache_base + i, r0);
            sequence_cache_store(key_cache + cache_base + half + i, r1);
        } else {
            query[(size_t)h * head_dim + i] = r0;
            query[(size_t)h * head_dim + half + i] = r1;
        }
    }
    if (is_key) {
        const float *v = qkv + (size_t)(heads + kv_heads + local_head) * head_dim;
        Cache *dst = value_cache + cache_base;
        for (uint32_t i = threadIdx.x; i < head_dim; i += blockDim.x) sequence_cache_store(dst + i, v[i]);
    }
}

extern "C" cudaError_t makepad_cuda_qk_rms_norm_rope_cache_f32(
    const float *qkv, const float *q_weight, const float *k_weight,
    const float *cos_table, const float *sin_table, const uint32_t *position,
    float *query, float *key_cache, float *value_cache,
    uint32_t heads, uint32_t kv_heads, uint32_t head_dim,
    uint32_t capacity, uint32_t table_rows, float eps, cudaStream_t stream) {
    if (!heads || !kv_heads || !head_dim || head_dim % 2 || !capacity || !table_rows) {
        return cudaErrorInvalidValue;
    }
    MakepadLaunchTiming timing("qk_norm_rope_cache", stream, heads + kv_heads, head_dim);
    qk_rms_norm_rope_cache_kernel<float><<<heads + kv_heads, head_dim < 1024 ? 256 : 1024, 0, stream>>>(
        qkv, q_weight, k_weight, cos_table, sin_table, position,
        query, key_cache, value_cache, heads, kv_heads, head_dim, capacity, table_rows, eps);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_cuda_qk_rms_norm_rope_cache_bf16(
    const float *qkv, const float *q_weight, const float *k_weight,
    const float *cos_table, const float *sin_table, const uint32_t *position,
    float *query, uint16_t *key_cache, uint16_t *value_cache,
    uint32_t heads, uint32_t kv_heads, uint32_t head_dim,
    uint32_t capacity, uint32_t table_rows, float eps, cudaStream_t stream) {
    if (!heads || !kv_heads || !head_dim || head_dim % 2 || !capacity || !table_rows) {
        return cudaErrorInvalidValue;
    }
    MakepadLaunchTiming timing("qk_norm_rope_cache_bf16", stream, heads + kv_heads, head_dim);
    qk_rms_norm_rope_cache_kernel<uint16_t><<<heads + kv_heads, head_dim < 1024 ? 256 : 1024, 0, stream>>>(
        qkv, q_weight, k_weight, cos_table, sin_table, position,
        query, key_cache, value_cache, heads, kv_heads, head_dim, capacity, table_rows, eps);
    return cudaGetLastError();
}

static __global__ void increment_u32_kernel(uint32_t *values, uint32_t count, uint32_t amount) {
    const uint32_t i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i < count) values[i] += amount;
}

extern "C" cudaError_t makepad_cuda_increment_u32(
    uint32_t *values, uint32_t count, uint32_t amount, cudaStream_t stream) {
    if (!count) return cudaErrorInvalidValue;
    MakepadLaunchTiming timing("increment", stream, count, 1);
    increment_u32_kernel<<<(count + 255) / 256, 256, 0, stream>>>(values, count, amount);
    return cudaGetLastError();
}

// Read Q|K|V without split copies. Each block owns a query or key head;
// key blocks also store V. Prefix rows of the destination are never touched.
static __global__ void qkv_rms_rope_bf16_kernel(
        const float *qkv, const float *qw, const float *kw,
        const float *cos_table, const float *sin_table,
        uint16_t *q, uint16_t *k, uint16_t *v,
        uint32_t heads, uint32_t kv_heads, uint32_t dim, uint32_t kv_offset, float eps) {
    const uint32_t group = blockIdx.x;
    const uint32_t token = group / (heads + kv_heads);
    const uint32_t head = group % (heads + kv_heads);
    const bool is_key = head >= heads;
    const uint32_t h = is_key ? head - heads : head;
    const size_t src = static_cast<size_t>(token) * (heads + 2 * kv_heads) * dim + head * dim;
    const float *weight = is_key ? kw : qw;
    float sum = 0.0f;
    for (uint32_t c = threadIdx.x; c < dim; c += blockDim.x) {
        const float x = qkv[src + c];
        sum += x * x;
    }
    sum = sequence_block_sum(sum);
    __shared__ float inv;
    if (threadIdx.x == 0) inv = rsqrtf(sum / static_cast<float>(dim) + eps);
    __syncthreads();
    const uint32_t half = dim / 2;
    const size_t table = static_cast<size_t>(token) * half;
    const size_t dst = is_key
        ? (static_cast<size_t>(token + kv_offset) * kv_heads + h) * dim
        : (static_cast<size_t>(token) * heads + h) * dim;
    uint16_t *out = is_key ? k : q;
    for (uint32_t c = threadIdx.x; c < half; c += blockDim.x) {
        const float a = __fmul_rn(__fmul_rn(qkv[src + c], inv), weight[c]);
        const float b = __fmul_rn(__fmul_rn(qkv[src + c + half], inv), weight[c + half]);
        const float co = cos_table[table + c], si = sin_table[table + c];
        out[dst + c] = sequence_bf16_rn(a * co - b * si);
        out[dst + c + half] = sequence_bf16_rn(b * co + a * si);
    }
    if (is_key) {
        for (uint32_t c = threadIdx.x; c < dim; c += blockDim.x) {
            v[dst + c] = sequence_bf16_rn(qkv[src + kv_heads * dim + c]);
        }
    }
}

extern "C" cudaError_t makepad_cuda_qkv_rms_rope_into_bf16(
        const float *qkv, const float *qw, const float *kw,
        const float *cos_table, const float *sin_table,
        uint16_t *q, uint16_t *k, uint16_t *v, uint32_t rows,
        uint32_t heads, uint32_t kv_heads, uint32_t dim, uint32_t kv_offset,
        float eps, cudaStream_t stream) {
    if (!rows || !heads || !kv_heads || !dim || dim % 2) return cudaErrorInvalidValue;
    const uint32_t block = dim < 1024 ? 256 : 1024;
    qkv_rms_rope_bf16_kernel<<<rows * (heads + kv_heads), block, 0, stream>>>(
        qkv, qw, kw, cos_table, sin_table, q, k, v, heads, kv_heads, dim, kv_offset, eps);
    return cudaGetLastError();
}

static __global__ void add_row_position_kernel(
        const float *x, const float *row, const float *position,
        float *out, size_t n, uint32_t cols) {
    const size_t i = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (i < n) out[i] = __fadd_rn(__fadd_rn(x[i], row[i % cols]), position[i]);
}

extern "C" cudaError_t makepad_cuda_add_row_position_into_f32(
        const float *x, const float *row, const float *position,
        float *out, uint32_t rows, uint32_t cols, cudaStream_t stream) {
    const size_t n = static_cast<size_t>(rows) * cols;
    if (!n) return cudaErrorInvalidValue;
    add_row_position_kernel<<<(n + 255) / 256, 256, 0, stream>>>(x, row, position, out, n, cols);
    return cudaGetLastError();
}

static __global__ void scaled_sub_bf16_kernel(
        const float *state, const float *velocity, float *out, size_t n, float step) {
    const size_t i = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (i < n) {
        const float delta = sequence_round_bf16(__fmul_rn(sequence_round_bf16(velocity[i]), step));
        out[i] = sequence_round_bf16(__fsub_rn(state[i], delta));
    }
}

extern "C" cudaError_t makepad_cuda_scaled_sub_into_bf16_f32(
        const float *state, const float *velocity, float *out,
        uint32_t n, float step, cudaStream_t stream) {
    if (!n) return cudaErrorInvalidValue;
    scaled_sub_bf16_kernel<<<(n + 255) / 256, 256, 0, stream>>>(state, velocity, out, n, step);
    return cudaGetLastError();
}

// Time-major im2col, [time, tap, channel]. Zero padding and the f16 operand
// conversion happen in this pass, without a padded tensor or host zeros.
static __global__ void conv1d_im2col_f16_kernel(
        const float *x, __half *col, size_t n, uint32_t len, uint32_t channels,
        uint32_t taps, uint32_t dilation, uint32_t padding) {
    const size_t i = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (i >= n) return;
    const uint32_t c = i % channels;
    const size_t time = i / channels;
    // Amortize coordinate division across taps. Adjacent lanes still read
    // and write adjacent channels for every tap, including dilation 3/9.
    for (uint32_t tap = 0; tap < taps; ++tap) {
        const int64_t source = static_cast<int64_t>(time)
            + static_cast<int64_t>(tap) * dilation - padding;
        const float value = source >= 0 && source < len ? x[source * channels + c] : 0.0f;
        col[(time * taps + tap) * channels + c] = __float2half_rn(value);
    }
}

extern "C" cudaError_t makepad_cuda_conv1d_im2col_f16(
        const float *x, uint16_t *col, uint32_t len, uint32_t out_len,
        uint32_t channels, uint32_t taps, uint32_t dilation, uint32_t padding,
        cudaStream_t stream) {
    const size_t n = static_cast<size_t>(out_len) * channels;
    if (!n || !len || !taps || !dilation) return cudaErrorInvalidValue;
    conv1d_im2col_f16_kernel<<<(n + 255) / 256, 256, 0, stream>>>(
        x, reinterpret_cast<__half *>(col), n, len, channels, taps, dilation, padding);
    return cudaGetLastError();
}
