#include <cuda_fp16.h>
#include <cuda_runtime.h>
#include <math_constants.h>
#include <stdint.h>

struct __align__(4) block_q8_1 {
    half d;
    half s;
    int8_t qs[32];
};

static_assert(sizeof(block_q8_1) == 36, "wrong q8_1 block size");

__device__ __forceinline__ float makepad_ggml_cuda_bf16_to_f32(uint16_t word) {
    const uint32_t bits = static_cast<uint32_t>(word) << 16;
    return __uint_as_float(bits);
}

__device__ __forceinline__ float makepad_ggml_cuda_f16_to_f32(uint16_t word) {
    return __half2float(*reinterpret_cast<const half *>(&word));
}

__device__ __forceinline__ uint16_t makepad_ggml_cuda_f32_to_f16_bits(float value) {
    const half h = __float2half_rn(value);
    return *reinterpret_cast<const uint16_t *>(&h);
}

__device__ __forceinline__ float makepad_ggml_cuda_bf16_round(float value) {
    const uint32_t bits = __float_as_uint(value);
    const uint32_t lsb = (bits >> 16) & 1u;
    const uint32_t rounded = (bits + 0x7FFFu + lsb) & 0xFFFF0000u;
    return __uint_as_float(rounded);
}

template <typename T>
__device__ __forceinline__ T makepad_ggml_cuda_warp_reduce_sum(T value) {
    for (int offset = warpSize / 2; offset > 0; offset >>= 1) {
        value += __shfl_down_sync(0xffffffffu, value, offset);
    }
    return value;
}

template <typename T>
__device__ __forceinline__ T makepad_ggml_cuda_warp_reduce_max(T value) {
    for (int offset = warpSize / 2; offset > 0; offset >>= 1) {
        const T other = __shfl_down_sync(0xffffffffu, value, offset);
        value = value > other ? value : other;
    }
    return value;
}

template <typename T>
__device__ __forceinline__ T makepad_ggml_cuda_block_reduce_sum(T value) {
    __shared__ T shared[32];
    const int lane = threadIdx.x & 31;
    const int warp = threadIdx.x >> 5;
    value = makepad_ggml_cuda_warp_reduce_sum(value);
    if (lane == 0) {
        shared[warp] = value;
    }
    __syncthreads();
    value = threadIdx.x < (blockDim.x + 31) / 32 ? shared[lane] : T(0);
    if (warp == 0) {
        value = makepad_ggml_cuda_warp_reduce_sum(value);
    }
    return value;
}

template <typename T>
__device__ __forceinline__ T makepad_ggml_cuda_block_reduce_max(T value) {
    __shared__ T shared[32];
    const int lane = threadIdx.x & 31;
    const int warp = threadIdx.x >> 5;
    value = makepad_ggml_cuda_warp_reduce_max(value);
    if (lane == 0) {
        shared[warp] = value;
    }
    __syncthreads();
    value = threadIdx.x < (blockDim.x + 31) / 32 ? shared[lane] : -CUDART_INF_F;
    if (warp == 0) {
        value = makepad_ggml_cuda_warp_reduce_max(value);
    }
    return value;
}

static __global__ void makepad_ggml_cuda_quantize_q8_1_f32_kernel(
        const float * __restrict__ input,
        block_q8_1 * __restrict__ output,
        uint32_t block_count) {
    const uint32_t block_idx = blockIdx.x;
    const uint32_t lane = threadIdx.x;
    if (block_idx >= block_count || lane >= 32) {
        return;
    }

    const float xi = input[block_idx * 32 + lane];
    float amax = fabsf(xi);
    float sum = xi;
    amax = makepad_ggml_cuda_warp_reduce_max(amax);
    sum = makepad_ggml_cuda_warp_reduce_sum(sum);
    const float d = amax / 127.0f;
    const float id = d != 0.0f ? 1.0f / d : 0.0f;
    const int8_t q = amax == 0.0f ? 0 : static_cast<int8_t>(lrintf(xi * id));
    output[block_idx].qs[lane] = q;
    if (lane == 0) {
        output[block_idx].d = __float2half_rn(d);
        output[block_idx].s = __float2half_rn(sum);
    }
}

static __global__ void makepad_ggml_cuda_scale_f32_kernel(
        float * __restrict__ values,
        float scale,
        uint32_t n) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx >= n) {
        return;
    }
    values[idx] = makepad_ggml_cuda_bf16_round(values[idx] * scale);
}

static __global__ void makepad_ggml_cuda_add_f32_kernel(
        const float * __restrict__ left,
        const float * __restrict__ right,
        float * __restrict__ out,
        uint32_t n) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx >= n) {
        return;
    }
    out[idx] = makepad_ggml_cuda_bf16_round(left[idx] + right[idx]);
}

static __global__ void makepad_ggml_cuda_geglu_split_f32_kernel(
        const float * __restrict__ gate_up,
        float * __restrict__ out,
        uint32_t n,
        uint32_t split_offset) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx >= n) {
        return;
    }
    const float gate = gate_up[idx];
    const float up = gate_up[split_offset + idx];
    const float squared = makepad_ggml_cuda_bf16_round(gate * gate);
    const float cubic = makepad_ggml_cuda_bf16_round(squared * gate);
    const float poly = makepad_ggml_cuda_bf16_round(gate + makepad_ggml_cuda_bf16_round(0.044715f * cubic));
    const float tanh_input = makepad_ggml_cuda_bf16_round(0.7978846f * poly);
    const float tanh_value = makepad_ggml_cuda_bf16_round(tanhf(tanh_input));
    const float half = makepad_ggml_cuda_bf16_round(0.5f * gate);
    const float gelu = makepad_ggml_cuda_bf16_round(half * makepad_ggml_cuda_bf16_round(1.0f + tanh_value));
    out[idx] = makepad_ggml_cuda_bf16_round(gelu * up);
}

static __global__ void makepad_ggml_cuda_geglu_split_f32_rows_kernel(
        const float * __restrict__ gate_up,
        float * __restrict__ out,
        uint32_t row_count,
        uint32_t row_stride,
        uint32_t n,
        uint32_t split_offset) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    const uint32_t total = row_count * n;
    if (idx >= total) {
        return;
    }
    const uint32_t row = idx / n;
    const uint32_t col = idx % n;
    const float * row_in = gate_up + row * row_stride;
    const float gate = row_in[col];
    const float up = row_in[split_offset + col];
    const float squared = makepad_ggml_cuda_bf16_round(gate * gate);
    const float cubic = makepad_ggml_cuda_bf16_round(squared * gate);
    const float poly = makepad_ggml_cuda_bf16_round(gate + makepad_ggml_cuda_bf16_round(0.044715f * cubic));
    const float tanh_input = makepad_ggml_cuda_bf16_round(0.7978846f * poly);
    const float tanh_value = makepad_ggml_cuda_bf16_round(tanhf(tanh_input));
    const float half = makepad_ggml_cuda_bf16_round(0.5f * gate);
    const float gelu = makepad_ggml_cuda_bf16_round(half * makepad_ggml_cuda_bf16_round(1.0f + tanh_value));
    out[idx] = makepad_ggml_cuda_bf16_round(gelu * up);
}

static __global__ void makepad_ggml_cuda_rms_norm_row_weighted_f32_kernel(
        const float * __restrict__ input,
        const uint16_t * __restrict__ weights_bf16,
        float * __restrict__ output,
        uint32_t n,
        float eps) {
    float sum = 0.0f;
    for (uint32_t idx = threadIdx.x; idx < n; idx += blockDim.x) {
        const float v = input[idx];
        sum += v * v;
    }
    sum = makepad_ggml_cuda_block_reduce_sum(sum);
    __shared__ float inv_rms;
    if (threadIdx.x == 0) {
        inv_rms = rsqrtf(sum / static_cast<float>(n) + eps);
    }
    __syncthreads();
    for (uint32_t idx = threadIdx.x; idx < n; idx += blockDim.x) {
        const float normalized = makepad_ggml_cuda_bf16_round(input[idx] * inv_rms);
        const float weight = makepad_ggml_cuda_bf16_to_f32(weights_bf16[idx]);
        output[idx] = makepad_ggml_cuda_bf16_round(normalized * weight);
    }
}

static __global__ void makepad_ggml_cuda_rms_norm_rows_weighted_f32_kernel(
        const float * __restrict__ input,
        const uint16_t * __restrict__ weights_bf16,
        float * __restrict__ output,
        uint32_t row_count,
        uint32_t row_stride,
        uint32_t n,
        float eps) {
    const uint32_t row = blockIdx.x;
    if (row >= row_count) {
        return;
    }
    const float * row_in = input + row * row_stride;
    float * row_out = output + row * row_stride;
    float sum = 0.0f;
    for (uint32_t idx = threadIdx.x; idx < n; idx += blockDim.x) {
        const float v = row_in[idx];
        sum += v * v;
    }
    sum = makepad_ggml_cuda_block_reduce_sum(sum);
    __shared__ float inv_rms;
    if (threadIdx.x == 0) {
        inv_rms = rsqrtf(sum / static_cast<float>(n) + eps);
    }
    __syncthreads();
    for (uint32_t idx = threadIdx.x; idx < n; idx += blockDim.x) {
        const float normalized = makepad_ggml_cuda_bf16_round(row_in[idx] * inv_rms);
        const float weight = makepad_ggml_cuda_bf16_to_f32(weights_bf16[idx]);
        row_out[idx] = makepad_ggml_cuda_bf16_round(normalized * weight);
    }
}

static __global__ void makepad_ggml_cuda_rms_norm_rows_no_scale_f32_kernel(
        const float * __restrict__ input,
        float * __restrict__ output,
        uint32_t row_count,
        uint32_t row_stride,
        uint32_t n,
        float eps) {
    const uint32_t row = blockIdx.x;
    if (row >= row_count) {
        return;
    }
    const float * row_in = input + row * row_stride;
    float * row_out = output + row * row_stride;
    float sum = 0.0f;
    for (uint32_t idx = threadIdx.x; idx < n; idx += blockDim.x) {
        const float v = row_in[idx];
        sum += v * v;
    }
    sum = makepad_ggml_cuda_block_reduce_sum(sum);
    __shared__ float inv_rms;
    if (threadIdx.x == 0) {
        inv_rms = rsqrtf(sum / static_cast<float>(n) + eps);
    }
    __syncthreads();
    for (uint32_t idx = threadIdx.x; idx < n; idx += blockDim.x) {
        row_out[idx] = makepad_ggml_cuda_bf16_round(row_in[idx] * inv_rms);
    }
}

static __global__ void makepad_ggml_cuda_rope_rows_f32_kernel(
        const float * __restrict__ input,
        float * __restrict__ output,
        uint32_t row_count,
        uint32_t row_stride,
        uint32_t head_dim,
        uint32_t rotary_dim,
        float base,
        uint32_t position) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    const uint32_t total = row_count * head_dim;
    if (idx >= total) {
        return;
    }
    const uint32_t row = idx / row_stride;
    const uint32_t col = idx % row_stride;
    if (row >= row_count || col >= head_dim) {
        return;
    }
    const uint32_t half = head_dim / 2;
    const uint32_t rotary_pairs = rotary_dim / 2;
    const float * row_in = input + row * row_stride;
    float * row_out = output + row * row_stride;

    if (col < rotary_pairs) {
        const float exponent = (2.0f * static_cast<float>(col)) / static_cast<float>(head_dim);
        const float inv_freq = powf(base, -exponent);
        const float theta = static_cast<float>(position) * inv_freq;
        const float cos_theta = cosf(theta);
        const float sin_theta = sinf(theta);
        const float left = row_in[col];
        const float right = row_in[half + col];
        row_out[col] = makepad_ggml_cuda_bf16_round(left * cos_theta - right * sin_theta);
    } else if (col >= half && col < half + rotary_pairs) {
        const uint32_t pair = col - half;
        const float exponent = (2.0f * static_cast<float>(pair)) / static_cast<float>(head_dim);
        const float inv_freq = powf(base, -exponent);
        const float theta = static_cast<float>(position) * inv_freq;
        const float cos_theta = cosf(theta);
        const float sin_theta = sinf(theta);
        const float left = row_in[pair];
        const float right = row_in[col];
        row_out[col] = makepad_ggml_cuda_bf16_round(left * sin_theta + right * cos_theta);
    } else {
        row_out[col] = row_in[col];
    }
}

static __global__ void makepad_ggml_cuda_rope_rows_f32_device_u32_kernel(
        const float * __restrict__ input,
        float * __restrict__ output,
        uint32_t row_count,
        uint32_t row_stride,
        uint32_t head_dim,
        uint32_t rotary_dim,
        float base,
        const uint32_t * __restrict__ position_device_u32) {
    const uint32_t position = *position_device_u32;
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    const uint32_t total = row_count * head_dim;
    if (idx >= total) {
        return;
    }
    const uint32_t row = idx / row_stride;
    const uint32_t col = idx % row_stride;
    if (row >= row_count || col >= head_dim) {
        return;
    }
    const uint32_t half = head_dim / 2;
    const uint32_t rotary_pairs = rotary_dim / 2;
    const float * row_in = input + row * row_stride;
    float * row_out = output + row * row_stride;

    if (col < rotary_pairs) {
        const float exponent = (2.0f * static_cast<float>(col)) / static_cast<float>(head_dim);
        const float inv_freq = powf(base, -exponent);
        const float theta = static_cast<float>(position) * inv_freq;
        const float cos_theta = cosf(theta);
        const float sin_theta = sinf(theta);
        const float left = row_in[col];
        const float right = row_in[half + col];
        row_out[col] = makepad_ggml_cuda_bf16_round(left * cos_theta - right * sin_theta);
    } else if (col >= half && col < half + rotary_pairs) {
        const uint32_t pair = col - half;
        const float exponent = (2.0f * static_cast<float>(pair)) / static_cast<float>(head_dim);
        const float inv_freq = powf(base, -exponent);
        const float theta = static_cast<float>(position) * inv_freq;
        const float cos_theta = cosf(theta);
        const float sin_theta = sinf(theta);
        const float left = row_in[pair];
        const float right = row_in[col];
        row_out[col] = makepad_ggml_cuda_bf16_round(left * sin_theta + right * cos_theta);
    } else {
        row_out[col] = row_in[col];
    }
}

static __global__ void makepad_ggml_cuda_kv_append_f32_kernel(
        const float * __restrict__ keys,
        const float * __restrict__ values,
        float * __restrict__ key_cache,
        float * __restrict__ value_cache,
        uint32_t kv_head_count,
        uint32_t head_dim,
        uint32_t max_tokens,
        uint32_t slot) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    const uint32_t total = kv_head_count * head_dim;
    if (idx >= total) {
        return;
    }
    const uint32_t head = idx / head_dim;
    const uint32_t dim = idx % head_dim;
    const uint32_t row_base = head * max_tokens * head_dim;
    const uint32_t cache_idx = row_base + slot * head_dim + dim;
    key_cache[cache_idx] = keys[idx];
    value_cache[cache_idx] = values[idx];
}

static __global__ void makepad_ggml_cuda_kv_append_f32_device_u32_kernel(
        const float * __restrict__ keys,
        const float * __restrict__ values,
        float * __restrict__ key_cache,
        float * __restrict__ value_cache,
        uint32_t kv_head_count,
        uint32_t head_dim,
        uint32_t max_tokens,
        const uint32_t * __restrict__ slot_device_u32) {
    const uint32_t slot = *slot_device_u32;
    if (slot >= max_tokens) {
        return;
    }
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    const uint32_t total = kv_head_count * head_dim;
    if (idx >= total) {
        return;
    }
    const uint32_t head = idx / head_dim;
    const uint32_t dim = idx % head_dim;
    const uint32_t row_base = head * max_tokens * head_dim;
    const uint32_t cache_idx = row_base + slot * head_dim + dim;
    key_cache[cache_idx] = keys[idx];
    value_cache[cache_idx] = values[idx];
}

static __device__ __forceinline__ float makepad_ggml_cuda_weighted_norm_f32(
        const float * __restrict__ row_in,
        const uint16_t * __restrict__ weights_bf16,
        uint32_t idx,
        float inv_rms) {
    const float normalized = makepad_ggml_cuda_bf16_round(row_in[idx] * inv_rms);
    const float weight = makepad_ggml_cuda_bf16_to_f32(weights_bf16[idx]);
    return makepad_ggml_cuda_bf16_round(normalized * weight);
}

static __global__ void makepad_ggml_cuda_qkv_norm_rope_cache_f32_kernel(
        const float * __restrict__ qkv,
        const uint16_t * __restrict__ q_weights_bf16,
        const uint16_t * __restrict__ k_weights_bf16,
        float * __restrict__ q_out,
        float * __restrict__ key_cache,
        float * __restrict__ value_cache,
        uint32_t q_head_count,
        uint32_t k_head_count,
        uint32_t head_dim,
        uint32_t q_offset,
        uint32_t k_offset,
        uint32_t v_offset,
        uint32_t rotary_dim,
        float base,
        uint32_t position,
        float eps,
        uint32_t max_tokens,
        uint32_t slot) {
    const uint32_t row = blockIdx.x;
    const uint32_t total_rows = q_head_count + 2u * k_head_count;
    if (row >= total_rows) {
        return;
    }

    const bool is_q = row < q_head_count;
    const bool is_k = row >= q_head_count && row < q_head_count + k_head_count;
    const uint32_t local_row = is_q ? row : (row - q_head_count) % k_head_count;
    const uint32_t source_offset = is_q ? q_offset : (is_k ? k_offset : v_offset);
    const float * row_in = qkv + source_offset + local_row * head_dim;

    float sum = 0.0f;
    for (uint32_t idx = threadIdx.x; idx < head_dim; idx += blockDim.x) {
        const float value = row_in[idx];
        sum += value * value;
    }
    sum = makepad_ggml_cuda_block_reduce_sum(sum);

    __shared__ float inv_rms;
    if (threadIdx.x == 0) {
        inv_rms = rsqrtf(sum / static_cast<float>(head_dim) + eps);
    }
    __syncthreads();

    const uint32_t half = head_dim / 2u;
    const uint32_t rotary_pairs = rotary_dim / 2u;
    const uint32_t cache_row_base = local_row * max_tokens * head_dim + slot * head_dim;

    for (uint32_t idx = threadIdx.x; idx < head_dim; idx += blockDim.x) {
        if (!is_q && !is_k) {
            value_cache[cache_row_base + idx] = makepad_ggml_cuda_bf16_round(row_in[idx] * inv_rms);
            continue;
        }

        const uint16_t * weights_bf16 = is_q ? q_weights_bf16 : k_weights_bf16;
        float out_value = 0.0f;
        if (idx < rotary_pairs) {
            const float exponent = (2.0f * static_cast<float>(idx)) / static_cast<float>(head_dim);
            const float inv_freq = powf(base, -exponent);
            const float theta = static_cast<float>(position) * inv_freq;
            const float cos_theta = cosf(theta);
            const float sin_theta = sinf(theta);
            const float left = makepad_ggml_cuda_weighted_norm_f32(row_in, weights_bf16, idx, inv_rms);
            const float right = makepad_ggml_cuda_weighted_norm_f32(row_in, weights_bf16, half + idx, inv_rms);
            out_value = makepad_ggml_cuda_bf16_round(left * cos_theta - right * sin_theta);
        } else if (idx >= half && idx < half + rotary_pairs) {
            const uint32_t pair = idx - half;
            const float exponent = (2.0f * static_cast<float>(pair)) / static_cast<float>(head_dim);
            const float inv_freq = powf(base, -exponent);
            const float theta = static_cast<float>(position) * inv_freq;
            const float cos_theta = cosf(theta);
            const float sin_theta = sinf(theta);
            const float left = makepad_ggml_cuda_weighted_norm_f32(row_in, weights_bf16, pair, inv_rms);
            const float right = makepad_ggml_cuda_weighted_norm_f32(row_in, weights_bf16, idx, inv_rms);
            out_value = makepad_ggml_cuda_bf16_round(left * sin_theta + right * cos_theta);
        } else {
            out_value = makepad_ggml_cuda_weighted_norm_f32(row_in, weights_bf16, idx, inv_rms);
        }

        if (is_q) {
            q_out[local_row * head_dim + idx] = out_value;
        } else {
            key_cache[cache_row_base + idx] = out_value;
        }
    }
}

static __global__ void makepad_ggml_cuda_qkv_norm_rope_cache_f32_device_u32_kernel(
        const float * __restrict__ qkv,
        const uint16_t * __restrict__ q_weights_bf16,
        const uint16_t * __restrict__ k_weights_bf16,
        float * __restrict__ q_out,
        float * __restrict__ key_cache,
        float * __restrict__ value_cache,
        uint32_t q_head_count,
        uint32_t k_head_count,
        uint32_t head_dim,
        uint32_t q_offset,
        uint32_t k_offset,
        uint32_t v_offset,
        uint32_t rotary_dim,
        float base,
        const uint32_t * __restrict__ position_device_u32,
        float eps,
        uint32_t max_tokens) {
    const uint32_t position = *position_device_u32;
    if (position >= max_tokens) {
        return;
    }

    const uint32_t row = blockIdx.x;
    const uint32_t total_rows = q_head_count + 2u * k_head_count;
    if (row >= total_rows) {
        return;
    }

    const bool is_q = row < q_head_count;
    const bool is_k = row >= q_head_count && row < q_head_count + k_head_count;
    const uint32_t local_row = is_q ? row : (row - q_head_count) % k_head_count;
    const uint32_t source_offset = is_q ? q_offset : (is_k ? k_offset : v_offset);
    const float * row_in = qkv + source_offset + local_row * head_dim;

    float sum = 0.0f;
    for (uint32_t idx = threadIdx.x; idx < head_dim; idx += blockDim.x) {
        const float value = row_in[idx];
        sum += value * value;
    }
    sum = makepad_ggml_cuda_block_reduce_sum(sum);

    __shared__ float inv_rms;
    if (threadIdx.x == 0) {
        inv_rms = rsqrtf(sum / static_cast<float>(head_dim) + eps);
    }
    __syncthreads();

    const uint32_t half = head_dim / 2u;
    const uint32_t rotary_pairs = rotary_dim / 2u;
    const uint32_t cache_row_base = local_row * max_tokens * head_dim + position * head_dim;

    for (uint32_t idx = threadIdx.x; idx < head_dim; idx += blockDim.x) {
        if (!is_q && !is_k) {
            value_cache[cache_row_base + idx] = makepad_ggml_cuda_bf16_round(row_in[idx] * inv_rms);
            continue;
        }

        const uint16_t * weights_bf16 = is_q ? q_weights_bf16 : k_weights_bf16;
        float out_value = 0.0f;
        if (idx < rotary_pairs) {
            const float exponent = (2.0f * static_cast<float>(idx)) / static_cast<float>(head_dim);
            const float inv_freq = powf(base, -exponent);
            const float theta = static_cast<float>(position) * inv_freq;
            const float cos_theta = cosf(theta);
            const float sin_theta = sinf(theta);
            const float left = makepad_ggml_cuda_weighted_norm_f32(row_in, weights_bf16, idx, inv_rms);
            const float right = makepad_ggml_cuda_weighted_norm_f32(row_in, weights_bf16, half + idx, inv_rms);
            out_value = makepad_ggml_cuda_bf16_round(left * cos_theta - right * sin_theta);
        } else if (idx >= half && idx < half + rotary_pairs) {
            const uint32_t pair = idx - half;
            const float exponent = (2.0f * static_cast<float>(pair)) / static_cast<float>(head_dim);
            const float inv_freq = powf(base, -exponent);
            const float theta = static_cast<float>(position) * inv_freq;
            const float cos_theta = cosf(theta);
            const float sin_theta = sinf(theta);
            const float left = makepad_ggml_cuda_weighted_norm_f32(row_in, weights_bf16, pair, inv_rms);
            const float right = makepad_ggml_cuda_weighted_norm_f32(row_in, weights_bf16, idx, inv_rms);
            out_value = makepad_ggml_cuda_bf16_round(left * sin_theta + right * cos_theta);
        } else {
            out_value = makepad_ggml_cuda_weighted_norm_f32(row_in, weights_bf16, idx, inv_rms);
        }

        if (is_q) {
            q_out[local_row * head_dim + idx] = out_value;
        } else {
            key_cache[cache_row_base + idx] = out_value;
        }
    }
}

static __global__ void makepad_ggml_cuda_attention_logits_seq_f32_kernel(
        const float * __restrict__ q,
        const float * __restrict__ key_cache,
        float * __restrict__ logits,
        uint32_t q_head_count,
        uint32_t q_heads_per_kv,
        uint32_t head_dim,
        uint32_t kv_row_stride,
        uint32_t seq_len,
        uint32_t start_slot,
        uint32_t capacity,
        uint32_t logits_row_stride) {
    const uint32_t q_head = blockIdx.x;
    const uint32_t token = blockIdx.y;
    if (q_head >= q_head_count || token >= seq_len) {
        return;
    }
    const uint32_t kv_head = q_head / q_heads_per_kv;
    const uint32_t slot = (start_slot + token) % capacity;
    const float * q_row = q + q_head * head_dim;
    const float * k_row = key_cache + kv_head * kv_row_stride + slot * head_dim;
    float sum = 0.0f;
    for (uint32_t dim = threadIdx.x; dim < head_dim; dim += blockDim.x) {
        sum += q_row[dim] * k_row[dim];
    }
    sum = makepad_ggml_cuda_block_reduce_sum(sum);
    if (threadIdx.x == 0) {
        logits[q_head * logits_row_stride + token] = makepad_ggml_cuda_bf16_round(sum);
    }
}

static __global__ void makepad_ggml_cuda_attention_logits_seq_f32_device_u32_kernel(
        const float * __restrict__ q,
        const float * __restrict__ key_cache,
        float * __restrict__ logits,
        uint32_t q_head_count,
        uint32_t q_heads_per_kv,
        uint32_t head_dim,
        uint32_t kv_row_stride,
        const uint32_t * __restrict__ seq_len_device_u32,
        uint32_t capacity,
        uint32_t logits_row_stride) {
    const uint32_t seq_len = *seq_len_device_u32;
    const uint32_t q_head = blockIdx.x;
    const uint32_t token = blockIdx.y;
    if (q_head >= q_head_count || token >= seq_len) {
        return;
    }
    const uint32_t kv_head = q_head / q_heads_per_kv;
    const float * q_row = q + q_head * head_dim;
    const float * k_row = key_cache + kv_head * kv_row_stride + token * head_dim;
    float sum = 0.0f;
    for (uint32_t dim = threadIdx.x; dim < head_dim; dim += blockDim.x) {
        sum += q_row[dim] * k_row[dim];
    }
    sum = makepad_ggml_cuda_block_reduce_sum(sum);
    if (threadIdx.x == 0) {
        logits[q_head * logits_row_stride + token] = makepad_ggml_cuda_bf16_round(sum);
    }
}

static __global__ void makepad_ggml_cuda_softmax_rows_f32_kernel(
        const float * __restrict__ logits,
        float * __restrict__ probs,
        uint32_t row_count,
        uint32_t row_stride,
        uint32_t seq_len) {
    const uint32_t row = blockIdx.x;
    if (row >= row_count) {
        return;
    }
    const float * row_in = logits + row * row_stride;
    float * row_out = probs + row * row_stride;

    float max_value = -CUDART_INF_F;
    for (uint32_t idx = threadIdx.x; idx < seq_len; idx += blockDim.x) {
        const float value = row_in[idx];
        max_value = value > max_value ? value : max_value;
    }
    max_value = makepad_ggml_cuda_block_reduce_max(max_value);
    __shared__ float shared_max;
    __shared__ float shared_sum;
    if (threadIdx.x == 0) {
        shared_max = max_value;
    }
    __syncthreads();

    float sum = 0.0f;
    for (uint32_t idx = threadIdx.x; idx < seq_len; idx += blockDim.x) {
        sum += expf(row_in[idx] - shared_max);
    }
    sum = makepad_ggml_cuda_block_reduce_sum(sum);
    if (threadIdx.x == 0) {
        shared_sum = sum;
    }
    __syncthreads();

    for (uint32_t idx = threadIdx.x; idx < seq_len; idx += blockDim.x) {
        row_out[idx] = makepad_ggml_cuda_bf16_round(expf(row_in[idx] - shared_max) / shared_sum);
    }
}

static __global__ void makepad_ggml_cuda_softmax_rows_f32_device_u32_kernel(
        const float * __restrict__ logits,
        float * __restrict__ probs,
        uint32_t row_count,
        uint32_t row_stride,
        const uint32_t * __restrict__ seq_len_device_u32) {
    const uint32_t seq_len = *seq_len_device_u32;
    const uint32_t row = blockIdx.x;
    if (row >= row_count) {
        return;
    }
    const float * row_in = logits + row * row_stride;
    float * row_out = probs + row * row_stride;

    float max_value = -CUDART_INF_F;
    for (uint32_t idx = threadIdx.x; idx < seq_len; idx += blockDim.x) {
        const float value = row_in[idx];
        max_value = value > max_value ? value : max_value;
    }
    max_value = makepad_ggml_cuda_block_reduce_max(max_value);
    __shared__ float shared_max;
    __shared__ float shared_sum;
    if (threadIdx.x == 0) {
        shared_max = max_value;
    }
    __syncthreads();

    float sum = 0.0f;
    for (uint32_t idx = threadIdx.x; idx < seq_len; idx += blockDim.x) {
        sum += expf(row_in[idx] - shared_max);
    }
    sum = makepad_ggml_cuda_block_reduce_sum(sum);
    if (threadIdx.x == 0) {
        shared_sum = sum;
    }
    __syncthreads();

    for (uint32_t idx = threadIdx.x; idx < seq_len; idx += blockDim.x) {
        row_out[idx] = makepad_ggml_cuda_bf16_round(expf(row_in[idx] - shared_max) / shared_sum);
    }
}

static __global__ void makepad_ggml_cuda_attention_weighted_sum_f32_kernel(
        const float * __restrict__ probs,
        const float * __restrict__ value_cache,
        float * __restrict__ out,
        uint32_t q_head_count,
        uint32_t q_heads_per_kv,
        uint32_t head_dim,
        uint32_t kv_row_stride,
        uint32_t seq_len,
        uint32_t start_slot,
        uint32_t capacity,
        uint32_t probs_row_stride,
        uint32_t out_row_stride) {
    const uint32_t q_head = blockIdx.y;
    const uint32_t dim = blockIdx.x * blockDim.x + threadIdx.x;
    if (q_head >= q_head_count || dim >= head_dim) {
        return;
    }
    const uint32_t kv_head = q_head / q_heads_per_kv;
    const float * row_probs = probs + q_head * probs_row_stride;
    const float * value_row = value_cache + kv_head * kv_row_stride;
    float acc = 0.0f;
    for (uint32_t token = 0; token < seq_len; ++token) {
        const uint32_t slot = (start_slot + token) % capacity;
        const float value = value_row[slot * head_dim + dim];
        acc = makepad_ggml_cuda_bf16_round(acc + makepad_ggml_cuda_bf16_round(row_probs[token] * value));
    }
    out[q_head * out_row_stride + dim] = acc;
}

static __global__ void makepad_ggml_cuda_attention_weighted_sum_f32_device_u32_kernel(
        const float * __restrict__ probs,
        const float * __restrict__ value_cache,
        float * __restrict__ out,
        uint32_t q_head_count,
        uint32_t q_heads_per_kv,
        uint32_t head_dim,
        uint32_t kv_row_stride,
        const uint32_t * __restrict__ seq_len_device_u32,
        uint32_t capacity,
        uint32_t probs_row_stride,
        uint32_t out_row_stride) {
    const uint32_t seq_len = *seq_len_device_u32;
    const uint32_t q_head = blockIdx.y;
    const uint32_t dim = blockIdx.x * blockDim.x + threadIdx.x;
    if (q_head >= q_head_count || dim >= head_dim) {
        return;
    }
    const uint32_t kv_head = q_head / q_heads_per_kv;
    const float * row_probs = probs + q_head * probs_row_stride;
    const float * value_row = value_cache + kv_head * kv_row_stride;
    float acc = 0.0f;
    for (uint32_t token = 0; token < seq_len; ++token) {
        const float value = value_row[token * head_dim + dim];
        acc = makepad_ggml_cuda_bf16_round(acc + makepad_ggml_cuda_bf16_round(row_probs[token] * value));
    }
    out[q_head * out_row_stride + dim] = acc;
}

static __global__ void makepad_ggml_cuda_argmax_f32_kernel(
        const float * __restrict__ logits,
        uint32_t * __restrict__ out_index,
        uint32_t n) {
    float best_value = -CUDART_INF_F;
    uint32_t best_index = 0;
    for (uint32_t idx = threadIdx.x; idx < n; idx += blockDim.x) {
        const float value = logits[idx];
        if (value > best_value || (value == best_value && idx < best_index)) {
            best_value = value;
            best_index = idx;
        }
    }

    __shared__ float shared_values[256];
    __shared__ uint32_t shared_indices[256];
    shared_values[threadIdx.x] = best_value;
    shared_indices[threadIdx.x] = best_index;
    __syncthreads();

    for (uint32_t stride = blockDim.x / 2; stride > 0; stride >>= 1) {
        if (threadIdx.x < stride) {
            const float other_value = shared_values[threadIdx.x + stride];
            const uint32_t other_index = shared_indices[threadIdx.x + stride];
            const float self_value = shared_values[threadIdx.x];
            const uint32_t self_index = shared_indices[threadIdx.x];
            if (other_value > self_value || (other_value == self_value && other_index < self_index)) {
                shared_values[threadIdx.x] = other_value;
                shared_indices[threadIdx.x] = other_index;
            }
        }
        __syncthreads();
    }

    if (threadIdx.x == 0) {
        *out_index = shared_indices[0];
    }
}

static __device__ __forceinline__ bool makepad_ggml_cuda_token_is_disallowed(
        uint32_t token_id,
        const uint32_t * __restrict__ disallowed_token_ids,
        uint32_t disallowed_count) {
    for (uint32_t index = 0; index < disallowed_count; ++index) {
        if (disallowed_token_ids[index] == token_id) {
            return true;
        }
    }
    return false;
}

static __device__ __forceinline__ bool makepad_ggml_cuda_argmax_candidate_is_better(
        float candidate_value,
        uint32_t candidate_index,
        float current_value,
        uint32_t current_index) {
    if (candidate_index == UINT32_MAX) {
        return false;
    }
    if (current_index == UINT32_MAX) {
        return true;
    }
    return candidate_value > current_value
        || (candidate_value == current_value && candidate_index < current_index);
}

static __global__ void makepad_ggml_cuda_masked_argmax_f32_kernel(
        const float * __restrict__ logits,
        const uint32_t * __restrict__ disallowed_token_ids,
        uint32_t disallowed_count,
        uint32_t * __restrict__ out_index,
        uint32_t n) {
    float best_value = -CUDART_INF_F;
    uint32_t best_index = UINT32_MAX;
    for (uint32_t idx = threadIdx.x; idx < n; idx += blockDim.x) {
        if (makepad_ggml_cuda_token_is_disallowed(idx, disallowed_token_ids, disallowed_count)) {
            continue;
        }
        const float value = logits[idx];
        if (makepad_ggml_cuda_argmax_candidate_is_better(value, idx, best_value, best_index)) {
            best_value = value;
            best_index = idx;
        }
    }

    __shared__ float shared_values[256];
    __shared__ uint32_t shared_indices[256];
    shared_values[threadIdx.x] = best_value;
    shared_indices[threadIdx.x] = best_index;
    __syncthreads();

    for (uint32_t stride = blockDim.x / 2; stride > 0; stride >>= 1) {
        if (threadIdx.x < stride) {
            const float other_value = shared_values[threadIdx.x + stride];
            const uint32_t other_index = shared_indices[threadIdx.x + stride];
            const float self_value = shared_values[threadIdx.x];
            const uint32_t self_index = shared_indices[threadIdx.x];
            if (makepad_ggml_cuda_argmax_candidate_is_better(
                    other_value,
                    other_index,
                    self_value,
                    self_index)) {
                shared_values[threadIdx.x] = other_value;
                shared_indices[threadIdx.x] = other_index;
            }
        }
        __syncthreads();
    }

    if (threadIdx.x == 0) {
        *out_index = shared_indices[0];
    }
}

static __global__ void makepad_ggml_cuda_masked_argmax_f32_device_u32_kernel(
        const float * __restrict__ logits,
        const uint32_t * __restrict__ disallowed_token_ids,
        const uint32_t * __restrict__ disallowed_count_device_u32,
        uint32_t * __restrict__ out_index,
        uint32_t n) {
    const uint32_t disallowed_count = *disallowed_count_device_u32;
    float best_value = -CUDART_INF_F;
    uint32_t best_index = UINT32_MAX;
    for (uint32_t idx = threadIdx.x; idx < n; idx += blockDim.x) {
        if (makepad_ggml_cuda_token_is_disallowed(idx, disallowed_token_ids, disallowed_count)) {
            continue;
        }
        const float value = logits[idx];
        if (makepad_ggml_cuda_argmax_candidate_is_better(value, idx, best_value, best_index)) {
            best_value = value;
            best_index = idx;
        }
    }

    __shared__ float shared_values[256];
    __shared__ uint32_t shared_indices[256];
    shared_values[threadIdx.x] = best_value;
    shared_indices[threadIdx.x] = best_index;
    __syncthreads();

    for (uint32_t stride = blockDim.x / 2; stride > 0; stride >>= 1) {
        if (threadIdx.x < stride) {
            const float other_value = shared_values[threadIdx.x + stride];
            const uint32_t other_index = shared_indices[threadIdx.x + stride];
            const float self_value = shared_values[threadIdx.x];
            const uint32_t self_index = shared_indices[threadIdx.x];
            if (makepad_ggml_cuda_argmax_candidate_is_better(
                    other_value,
                    other_index,
                    self_value,
                    self_index)) {
                shared_values[threadIdx.x] = other_value;
                shared_indices[threadIdx.x] = other_index;
            }
        }
        __syncthreads();
    }

    if (threadIdx.x == 0) {
        *out_index = shared_indices[0];
    }
}

extern "C" cudaError_t makepad_ggml_cuda_quantize_q8_1_f32(
        const float * input_f32,
        uint8_t * output_q8_1_bytes,
        uint32_t n,
        cudaStream_t stream) {
    if (n == 0 || (n % 32) != 0) {
        return cudaErrorInvalidValue;
    }
    const uint32_t block_count = n / 32;
    makepad_ggml_cuda_quantize_q8_1_f32_kernel<<<block_count, 32, 0, stream>>>(
        input_f32,
        reinterpret_cast<block_q8_1 *>(output_q8_1_bytes),
        block_count);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_scale_f32_inplace(
        float * values,
        float scale,
        uint32_t n,
        cudaStream_t stream) {
    if (n == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid((n + block.x - 1) / block.x, 1, 1);
    makepad_ggml_cuda_scale_f32_kernel<<<grid, block, 0, stream>>>(values, scale, n);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_add_f32(
        const float * left,
        const float * right,
        float * out,
        uint32_t n,
        cudaStream_t stream) {
    if (n == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid((n + block.x - 1) / block.x, 1, 1);
    makepad_ggml_cuda_add_f32_kernel<<<grid, block, 0, stream>>>(left, right, out, n);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_geglu_split_f32(
        const float * gate_up,
        float * out,
        uint32_t n,
        uint32_t split_offset,
        cudaStream_t stream) {
    if (n == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid((n + block.x - 1) / block.x, 1, 1);
    makepad_ggml_cuda_geglu_split_f32_kernel<<<grid, block, 0, stream>>>(gate_up, out, n, split_offset);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_geglu_split_f32_rows(
        const float * gate_up,
        float * out,
        uint32_t row_count,
        uint32_t row_stride,
        uint32_t n,
        uint32_t split_offset,
        cudaStream_t stream) {
    if (row_count == 0 || n == 0) {
        return cudaSuccess;
    }
    const uint32_t total = row_count * n;
    const dim3 block(256, 1, 1);
    const dim3 grid((total + block.x - 1) / block.x, 1, 1);
    makepad_ggml_cuda_geglu_split_f32_rows_kernel<<<grid, block, 0, stream>>>(
        gate_up,
        out,
        row_count,
        row_stride,
        n,
        split_offset);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_rms_norm_row_weighted_f32(
        const float * input,
        const uint16_t * weights_bf16,
        float * output,
        uint32_t n,
        float eps,
        cudaStream_t stream) {
    if (n == 0) {
        return cudaErrorInvalidValue;
    }
    makepad_ggml_cuda_rms_norm_row_weighted_f32_kernel<<<1, 256, 0, stream>>>(input, weights_bf16, output, n, eps);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_rms_norm_rows_weighted_f32(
        const float * input,
        const uint16_t * weights_bf16,
        float * output,
        uint32_t row_count,
        uint32_t row_stride,
        uint32_t n,
        float eps,
        cudaStream_t stream) {
    if (row_count == 0 || n == 0 || row_stride < n) {
        return cudaErrorInvalidValue;
    }
    makepad_ggml_cuda_rms_norm_rows_weighted_f32_kernel<<<row_count, 256, 0, stream>>>(
        input, weights_bf16, output, row_count, row_stride, n, eps);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_rms_norm_rows_no_scale_f32(
        const float * input,
        float * output,
        uint32_t row_count,
        uint32_t row_stride,
        uint32_t n,
        float eps,
        cudaStream_t stream) {
    if (row_count == 0 || n == 0 || row_stride < n) {
        return cudaErrorInvalidValue;
    }
    makepad_ggml_cuda_rms_norm_rows_no_scale_f32_kernel<<<row_count, 256, 0, stream>>>(
        input, output, row_count, row_stride, n, eps);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_rope_rows_f32(
        const float * input,
        float * output,
        uint32_t row_count,
        uint32_t row_stride,
        uint32_t head_dim,
        uint32_t rotary_dim,
        float base,
        uint32_t position,
        cudaStream_t stream) {
    if (row_count == 0 || head_dim == 0 || row_stride < head_dim || rotary_dim > head_dim || (rotary_dim & 1u) != 0u) {
        return cudaErrorInvalidValue;
    }
    const uint32_t total = row_count * row_stride;
    const dim3 block(256, 1, 1);
    const dim3 grid((total + block.x - 1) / block.x, 1, 1);
    makepad_ggml_cuda_rope_rows_f32_kernel<<<grid, block, 0, stream>>>(
        input, output, row_count, row_stride, head_dim, rotary_dim, base, position);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_rope_rows_f32_device_u32(
        const float * input,
        float * output,
        uint32_t row_count,
        uint32_t row_stride,
        uint32_t head_dim,
        uint32_t rotary_dim,
        float base,
        const uint32_t * position_device_u32,
        cudaStream_t stream) {
    if (row_count == 0 || head_dim == 0 || row_stride < head_dim || rotary_dim > head_dim || (rotary_dim & 1u) != 0u) {
        return cudaErrorInvalidValue;
    }
    const uint32_t total = row_count * row_stride;
    const dim3 block(256, 1, 1);
    const dim3 grid((total + block.x - 1) / block.x, 1, 1);
    makepad_ggml_cuda_rope_rows_f32_device_u32_kernel<<<grid, block, 0, stream>>>(
        input, output, row_count, row_stride, head_dim, rotary_dim, base, position_device_u32);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_kv_append_f32(
        const float * keys,
        const float * values,
        float * key_cache,
        float * value_cache,
        uint32_t kv_head_count,
        uint32_t head_dim,
        uint32_t max_tokens,
        uint32_t slot,
        cudaStream_t stream) {
    if (kv_head_count == 0 || head_dim == 0 || max_tokens == 0 || slot >= max_tokens) {
        return cudaErrorInvalidValue;
    }
    const uint32_t total = kv_head_count * head_dim;
    const dim3 block(256, 1, 1);
    const dim3 grid((total + block.x - 1) / block.x, 1, 1);
    makepad_ggml_cuda_kv_append_f32_kernel<<<grid, block, 0, stream>>>(
        keys, values, key_cache, value_cache, kv_head_count, head_dim, max_tokens, slot);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_kv_append_f32_device_u32(
        const float * keys,
        const float * values,
        float * key_cache,
        float * value_cache,
        uint32_t kv_head_count,
        uint32_t head_dim,
        uint32_t max_tokens,
        const uint32_t * slot_device_u32,
        cudaStream_t stream) {
    if (kv_head_count == 0 || head_dim == 0 || max_tokens == 0) {
        return cudaErrorInvalidValue;
    }
    const uint32_t total = kv_head_count * head_dim;
    const dim3 block(256, 1, 1);
    const dim3 grid((total + block.x - 1) / block.x, 1, 1);
    makepad_ggml_cuda_kv_append_f32_device_u32_kernel<<<grid, block, 0, stream>>>(
        keys, values, key_cache, value_cache, kv_head_count, head_dim, max_tokens, slot_device_u32);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_qkv_norm_rope_cache_f32(
        const float * qkv,
        const uint16_t * q_weights_bf16,
        const uint16_t * k_weights_bf16,
        float * q_out,
        float * key_cache,
        float * value_cache,
        uint32_t q_head_count,
        uint32_t k_head_count,
        uint32_t head_dim,
        uint32_t q_offset,
        uint32_t k_offset,
        uint32_t v_offset,
        uint32_t rotary_dim,
        float base,
        uint32_t position,
        float eps,
        uint32_t max_tokens,
        uint32_t slot,
        cudaStream_t stream) {
    if (q_head_count == 0 || k_head_count == 0 || head_dim == 0 || rotary_dim > head_dim ||
            (rotary_dim & 1u) != 0u || max_tokens == 0 || slot >= max_tokens) {
        return cudaErrorInvalidValue;
    }
    const uint32_t total_rows = q_head_count + 2u * k_head_count;
    makepad_ggml_cuda_qkv_norm_rope_cache_f32_kernel<<<total_rows, 256, 0, stream>>>(
        qkv,
        q_weights_bf16,
        k_weights_bf16,
        q_out,
        key_cache,
        value_cache,
        q_head_count,
        k_head_count,
        head_dim,
        q_offset,
        k_offset,
        v_offset,
        rotary_dim,
        base,
        position,
        eps,
        max_tokens,
        slot);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_qkv_norm_rope_cache_f32_device_u32(
        const float * qkv,
        const uint16_t * q_weights_bf16,
        const uint16_t * k_weights_bf16,
        float * q_out,
        float * key_cache,
        float * value_cache,
        uint32_t q_head_count,
        uint32_t k_head_count,
        uint32_t head_dim,
        uint32_t q_offset,
        uint32_t k_offset,
        uint32_t v_offset,
        uint32_t rotary_dim,
        float base,
        const uint32_t * position_device_u32,
        float eps,
        uint32_t max_tokens,
        cudaStream_t stream) {
    if (q_head_count == 0 || k_head_count == 0 || head_dim == 0 || rotary_dim > head_dim ||
            (rotary_dim & 1u) != 0u || max_tokens == 0) {
        return cudaErrorInvalidValue;
    }
    const uint32_t total_rows = q_head_count + 2u * k_head_count;
    makepad_ggml_cuda_qkv_norm_rope_cache_f32_device_u32_kernel<<<total_rows, 256, 0, stream>>>(
        qkv,
        q_weights_bf16,
        k_weights_bf16,
        q_out,
        key_cache,
        value_cache,
        q_head_count,
        k_head_count,
        head_dim,
        q_offset,
        k_offset,
        v_offset,
        rotary_dim,
        base,
        position_device_u32,
        eps,
        max_tokens);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_attention_logits_seq_f32(
        const float * q,
        const float * key_cache,
        float * logits,
        uint32_t q_head_count,
        uint32_t q_heads_per_kv,
        uint32_t head_dim,
        uint32_t kv_row_stride,
        uint32_t seq_len,
        uint32_t start_slot,
        uint32_t capacity,
        uint32_t logits_row_stride,
        cudaStream_t stream) {
    if (q_head_count == 0 || q_heads_per_kv == 0 || head_dim == 0 || seq_len == 0 || capacity == 0 || start_slot >= capacity || logits_row_stride < seq_len) {
        return cudaErrorInvalidValue;
    }
    const dim3 grid(q_head_count, seq_len, 1);
    makepad_ggml_cuda_attention_logits_seq_f32_kernel<<<grid, 256, 0, stream>>>(
        q, key_cache, logits, q_head_count, q_heads_per_kv, head_dim, kv_row_stride, seq_len, start_slot, capacity, logits_row_stride);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_attention_logits_seq_f32_device_u32(
        const float * q,
        const float * key_cache,
        float * logits,
        uint32_t q_head_count,
        uint32_t q_heads_per_kv,
        uint32_t head_dim,
        uint32_t kv_row_stride,
        const uint32_t * seq_len_device_u32,
        uint32_t capacity,
        uint32_t logits_row_stride,
        cudaStream_t stream) {
    if (q_head_count == 0 || q_heads_per_kv == 0 || head_dim == 0 || capacity == 0 || logits_row_stride < capacity) {
        return cudaErrorInvalidValue;
    }
    const dim3 grid(q_head_count, capacity, 1);
    makepad_ggml_cuda_attention_logits_seq_f32_device_u32_kernel<<<grid, 256, 0, stream>>>(
        q, key_cache, logits, q_head_count, q_heads_per_kv, head_dim, kv_row_stride, seq_len_device_u32, capacity, logits_row_stride);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_softmax_rows_f32(
        const float * logits,
        float * probs,
        uint32_t row_count,
        uint32_t row_stride,
        uint32_t seq_len,
        cudaStream_t stream) {
    if (row_count == 0 || row_stride < seq_len || seq_len == 0) {
        return cudaErrorInvalidValue;
    }
    makepad_ggml_cuda_softmax_rows_f32_kernel<<<row_count, 256, 0, stream>>>(
        logits, probs, row_count, row_stride, seq_len);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_softmax_rows_f32_device_u32(
        const float * logits,
        float * probs,
        uint32_t row_count,
        uint32_t row_stride,
        const uint32_t * seq_len_device_u32,
        cudaStream_t stream) {
    if (row_count == 0 || row_stride == 0) {
        return cudaErrorInvalidValue;
    }
    makepad_ggml_cuda_softmax_rows_f32_device_u32_kernel<<<row_count, 256, 0, stream>>>(
        logits, probs, row_count, row_stride, seq_len_device_u32);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_attention_weighted_sum_f32(
        const float * probs,
        const float * value_cache,
        float * out,
        uint32_t q_head_count,
        uint32_t q_heads_per_kv,
        uint32_t head_dim,
        uint32_t kv_row_stride,
        uint32_t seq_len,
        uint32_t start_slot,
        uint32_t capacity,
        uint32_t probs_row_stride,
        uint32_t out_row_stride,
        cudaStream_t stream) {
    if (q_head_count == 0 || q_heads_per_kv == 0 || head_dim == 0 || seq_len == 0 || capacity == 0 || start_slot >= capacity || probs_row_stride < seq_len || out_row_stride < head_dim) {
        return cudaErrorInvalidValue;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid((head_dim + block.x - 1) / block.x, q_head_count, 1);
    makepad_ggml_cuda_attention_weighted_sum_f32_kernel<<<grid, block, 0, stream>>>(
        probs, value_cache, out, q_head_count, q_heads_per_kv, head_dim, kv_row_stride, seq_len, start_slot, capacity, probs_row_stride, out_row_stride);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_attention_weighted_sum_f32_device_u32(
        const float * probs,
        const float * value_cache,
        float * out,
        uint32_t q_head_count,
        uint32_t q_heads_per_kv,
        uint32_t head_dim,
        uint32_t kv_row_stride,
        const uint32_t * seq_len_device_u32,
        uint32_t capacity,
        uint32_t probs_row_stride,
        uint32_t out_row_stride,
        cudaStream_t stream) {
    if (q_head_count == 0 || q_heads_per_kv == 0 || head_dim == 0 || capacity == 0 || probs_row_stride < capacity || out_row_stride < head_dim) {
        return cudaErrorInvalidValue;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid((head_dim + block.x - 1) / block.x, q_head_count, 1);
    makepad_ggml_cuda_attention_weighted_sum_f32_device_u32_kernel<<<grid, block, 0, stream>>>(
        probs, value_cache, out, q_head_count, q_heads_per_kv, head_dim, kv_row_stride, seq_len_device_u32, capacity, probs_row_stride, out_row_stride);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_argmax_f32(
        const float * logits,
        uint32_t * out_index,
        uint32_t n,
        cudaStream_t stream) {
    if (n == 0) {
        return cudaErrorInvalidValue;
    }
    makepad_ggml_cuda_argmax_f32_kernel<<<1, 256, 0, stream>>>(logits, out_index, n);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_masked_argmax_f32(
        const float * logits,
        const uint32_t * disallowed_token_ids,
        uint32_t disallowed_count,
        uint32_t * out_index,
        uint32_t n,
        cudaStream_t stream) {
    if (n == 0) {
        return cudaErrorInvalidValue;
    }
    makepad_ggml_cuda_masked_argmax_f32_kernel<<<1, 256, 0, stream>>>(
        logits,
        disallowed_token_ids,
        disallowed_count,
        out_index,
        n);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_masked_argmax_f32_device_u32(
        const float * logits,
        const uint32_t * disallowed_token_ids,
        const uint32_t * disallowed_count_device_u32,
        uint32_t * out_index,
        uint32_t n,
        cudaStream_t stream) {
    if (n == 0) {
        return cudaErrorInvalidValue;
    }
    makepad_ggml_cuda_masked_argmax_f32_device_u32_kernel<<<1, 256, 0, stream>>>(
        logits,
        disallowed_token_ids,
        disallowed_count_device_u32,
        out_index,
        n);
    return cudaGetLastError();
}
